//! Однострочный XML-подсветчик для виртуального редактора.
//!
//! Каждая видимая строка размечается отдельно, с учётом состояния, в котором закончилась
//! предыдущая строка (`LineState`) — так многострочные комментарии и CDATA подсвечиваются
//! корректно. Состояние входит в ключ кэша galley в редакторе.

use crate::theme::{code_font_regular, p};
use eframe::egui;
use egui::text::{LayoutJob, TextFormat};
use egui::Color32;

/// Состояние лексера на границе строки.
#[derive(Clone, Copy, PartialEq, Eq, Default, Hash, Debug)]
pub enum LineState {
    /// Обычный контент (текст между тегами).
    #[default]
    Text,
    /// Внутри `<!-- … -->`.
    Comment,
    /// Внутри `<![CDATA[ … ]]>`.
    CData,
    /// Внутри тега `<… >` (имя/атрибуты), перенесённого на следующую строку.
    Tag,
}

impl LineState {
    /// Стабильный байтовый код состояния — `LexState` редактора и ключ кэша galley.
    pub fn key(self) -> u8 {
        match self {
            LineState::Text => 0,
            LineState::Comment => b'c',
            LineState::CData => b'd',
            LineState::Tag => b'g',
        }
    }

    /// Обратное к [`key`]: код редактора → состояние (0 и незнакомые коды → Text).
    pub fn from_key(k: u8) -> Self {
        match k {
            b'c' => LineState::Comment,
            b'd' => LineState::CData,
            b'g' => LineState::Tag,
            _ => LineState::Text,
        }
    }
}

struct Push<'a> {
    job: &'a mut LayoutJob,
    font: egui::FontId,
}

impl Push<'_> {
    fn add(&mut self, s: &str, color: Color32) {
        if s.is_empty() {
            return;
        }
        self.job.append(s, 0.0, TextFormat { font_id: self.font.clone(), color, ..Default::default() });
    }
}

/// Разметить одну строку, начиная в состоянии `state`; вернуть job и состояние на конце строки.
pub fn highlight_xml(text: &str, state: LineState, size: f32) -> (LayoutJob, LineState) {
    let pal = p();
    let mut job = LayoutJob::default();
    let mut out = Push { job: &mut job, font: code_font_regular(size) };
    let b = text.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    let mut st = state;

    // цвета (теги — синие, атрибуты — тёмно-голубые, значения — зелёные,
    // комментарии — серые, CDATA — «песочный»)
    let c_punct = pal.text_dim;
    let c_tag = pal.syn_fn;
    let c_attr = pal.syn_kw;
    let c_val = pal.syn_str;
    let c_com = pal.syn_com;
    let c_cdata = pal.syn_num;
    let c_text = pal.text;

    while i < n {
        match st {
            LineState::Comment => {
                if let Some(end) = find(b, i, b"-->") {
                    out.add(&text[i..end + 3], c_com);
                    i = end + 3;
                    st = LineState::Text;
                } else {
                    out.add(&text[i..], c_com);
                    i = n;
                }
            }
            LineState::CData => {
                if let Some(end) = find(b, i, b"]]>") {
                    out.add(&text[i..end], c_cdata);
                    out.add(&text[end..end + 3], c_punct);
                    i = end + 3;
                    st = LineState::Text;
                } else {
                    out.add(&text[i..], c_cdata);
                    i = n;
                }
            }
            LineState::Tag => {
                // внутри тега: имя уже было; размечаем атрибуты до '>'
                let (advanced, new_st) = lex_in_tag(text, b, i, &mut out, c_punct, c_attr, c_val);
                i = advanced;
                st = new_st;
            }
            LineState::Text => {
                if b[i] == b'<' {
                    if starts(b, i, b"<!--") {
                        out.add("<!--", c_com);
                        i += 4;
                        st = LineState::Comment;
                    } else if starts(b, i, b"<![CDATA[") {
                        out.add("<![CDATA[", c_punct);
                        i += 9;
                        st = LineState::CData;
                    } else if starts(b, i, b"<?") || starts(b, i, b"<!") {
                        // декларация / PI / DOCTYPE — целиком до '>' одним цветом
                        let end = find(b, i, b">").map(|e| e + 1).unwrap_or(n);
                        out.add(&text[i..end], c_punct);
                        i = end;
                    } else {
                        // открывающий или закрывающий тег: '<' [/] имя
                        let mut j = i + 1;
                        if j < n && b[j] == b'/' {
                            j += 1;
                        }
                        let name_start = j;
                        while j < n && !b[j].is_ascii_whitespace() && b[j] != b'>' && b[j] != b'/' {
                            j += 1;
                        }
                        out.add(&text[i..name_start], c_punct);
                        out.add(&text[name_start..j], c_tag);
                        i = j;
                        st = LineState::Tag;
                    }
                } else {
                    // текстовый контент до следующего '<'
                    let end = find(b, i, b"<").unwrap_or(n);
                    out.add(&text[i..end], c_text);
                    i = end;
                }
            }
        }
    }
    (job, st)
}

/// Разметка внутри тега (атрибуты и значения) начиная с `i`; возвращает (новый i, состояние).
fn lex_in_tag(
    text: &str,
    b: &[u8],
    mut i: usize,
    out: &mut Push,
    c_punct: Color32,
    c_attr: Color32,
    c_val: Color32,
) -> (usize, LineState) {
    let n = b.len();
    while i < n {
        let ch = b[i];
        if ch == b'>' {
            out.add(&text[i..i + 1], c_punct);
            return (i + 1, LineState::Text);
        }
        if ch == b'/' && i + 1 < n && b[i + 1] == b'>' {
            out.add(&text[i..i + 2], c_punct);
            return (i + 2, LineState::Text);
        }
        if ch == b'/' {
            // Одиночный '/' (не часть '/>') — пунктуация. ОБЯЗАТЕЛЬНО двигаем i:
            // скан имени атрибута ниже исключает '/', поэтому без этого i застрял бы
            // на месте → бесконечный цикл. Случается, когда строка размечается уже
            // в состоянии Tag и начинается с закрывающего тега `</…>` (например,
            // пользователь стёр '>' у предыдущей строки) — см. тест tag_state_recovers.
            out.add(&text[i..i + 1], c_punct);
            i += 1;
            continue;
        }
        if ch == b'"' || ch == b'\'' {
            // значение атрибута; незакрытая кавычка дотягивается до конца строки
            let q = ch;
            let mut j = i + 1;
            while j < n && b[j] != q {
                j += 1;
            }
            let end = (j + 1).min(n);
            out.add(&text[i..end], c_val);
            i = end;
            continue;
        }
        if ch == b'=' {
            out.add(&text[i..i + 1], c_punct);
            i += 1;
            continue;
        }
        if ch.is_ascii_whitespace() {
            let mut j = i;
            while j < n && b[j].is_ascii_whitespace() {
                j += 1;
            }
            out.add(&text[i..j], c_punct);
            i = j;
            continue;
        }
        // имя атрибута
        let mut j = i;
        while j < n
            && !b[j].is_ascii_whitespace()
            && b[j] != b'='
            && b[j] != b'>'
            && b[j] != b'/'
            && b[j] != b'"'
            && b[j] != b'\''
        {
            j += 1;
        }
        // Страховка от зацикливания: любой неучтённый символ обязан продвинуть i
        // хотя бы на байт (инвариант «строчный лексер всегда движется»).
        let j = j.max(i + 1);
        out.add(&text[i..j], c_attr);
        i = j;
    }
    (i, LineState::Tag)
}

/// Только переход состояния по строке, без построения LayoutJob — для ленивого
/// продвижения кэша состояний (LexCache) с бюджетом на кадр.
pub fn highlight_xml_state_only(text: &str, state: LineState) -> LineState {
    let b = text.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    let mut st = state;
    while i < n {
        match st {
            LineState::Comment => match find(b, i, b"-->") {
                Some(end) => {
                    i = end + 3;
                    st = LineState::Text;
                }
                None => i = n,
            },
            LineState::CData => match find(b, i, b"]]>") {
                Some(end) => {
                    i = end + 3;
                    st = LineState::Text;
                }
                None => i = n,
            },
            LineState::Tag => {
                // пропустить атрибуты до '>' с учётом кавычек
                while i < n {
                    let ch = b[i];
                    if ch == b'>' {
                        i += 1;
                        st = LineState::Text;
                        break;
                    }
                    if ch == b'"' || ch == b'\'' {
                        let q = ch;
                        i += 1;
                        while i < n && b[i] != q {
                            i += 1;
                        }
                        i = (i + 1).min(n);
                        continue;
                    }
                    i += 1;
                }
            }
            LineState::Text => {
                if b[i] == b'<' {
                    if starts(b, i, b"<!--") {
                        i += 4;
                        st = LineState::Comment;
                    } else if starts(b, i, b"<![CDATA[") {
                        i += 9;
                        st = LineState::CData;
                    } else if starts(b, i, b"<?") || starts(b, i, b"<!") {
                        i = find(b, i, b">").map(|e| e + 1).unwrap_or(n);
                    } else {
                        i += 1;
                        st = LineState::Tag;
                    }
                } else {
                    i = find(b, i, b"<").unwrap_or(n);
                }
            }
        }
    }
    st
}

fn starts(b: &[u8], i: usize, pat: &[u8]) -> bool {
    b.len() >= i + pat.len() && &b[i..i + pat.len()] == pat
}

fn find(b: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    if pat.len() == 1 {
        return b[from..].iter().position(|&c| c == pat[0]).map(|p| from + p);
    }
    b.get(from..)?
        .windows(pat.len())
        .position(|w| w == pat)
        .map(|p| from + p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_after(line: &str, st: LineState) -> LineState {
        highlight_xml(line, st, 13.0).1
    }

    #[test]
    fn plain_tag_round_trip() {
        assert_eq!(state_after("<a b=\"c\">text</a>", LineState::Text), LineState::Text);
    }

    #[test]
    fn comment_spans_lines() {
        assert_eq!(state_after("<a><!-- start", LineState::Text), LineState::Comment);
        assert_eq!(state_after("still inside", LineState::Comment), LineState::Comment);
        assert_eq!(state_after("end --><b>", LineState::Comment), LineState::Text);
    }

    #[test]
    fn cdata_spans_lines() {
        assert_eq!(state_after("<a><![CDATA[raw", LineState::Text), LineState::CData);
        assert_eq!(state_after("more ]]> tail", LineState::CData), LineState::Text);
    }

    #[test]
    fn open_tag_spans_lines() {
        assert_eq!(state_after("<Document attr=\"1\"", LineState::Text), LineState::Tag);
        assert_eq!(state_after("  next=\"2\">", LineState::Tag), LineState::Text);
    }

    /// Лёгкий state-only проход обязан давать те же переходы, что и полный.
    /// Полный проход обязан ВСЕГДА завершаться (строчный лексер не зацикливается):
    /// если бы `highlight_xml` зависал на каком-то входе, этот тест бы не вернулся.
    #[test]
    fn state_only_matches_full() {
        let lines = [
            "<a b=\"c\">text</a>",
            "<a><!-- start",
            "middle -->",
            "<![CDATA[x",
            "tail ]]><b",
            "attr=\"v\" />",
            "plain text only",
            "<?xml version=\"1.0\"?>",
            "<x y='unclosed",
            "-->]]>",
            // одиночный '/' внутри состояния Tag — раньше зацикливал lex_in_tag:
            "</FL_46_UL_36_OrgSource>",
            "  </Source>",
            "a/b>",
            "/ ",
            "</x",
            "<self />",
            "</a></b>",
        ];
        for st in [LineState::Text, LineState::Comment, LineState::CData, LineState::Tag] {
            for line in lines {
                let full = highlight_xml(line, st, 13.0).1;
                let fast = highlight_xml_state_only(line, st);
                assert_eq!(full, fast, "line={line:?} start={st:?}");
            }
        }
    }

    /// Регрессия зависания (invalid_1gb.xml): пользователь стирает '>' у строки,
    /// заканчивающейся тегом, — следующая строка размечается уже в состоянии Tag и
    /// начинается с закрывающего тега `</…>`. Полный лексер обязан завершиться и
    /// разметить всю строку (не застрять на ведущем '/').
    #[test]
    fn tag_state_recovers_on_closing_tag() {
        // строка 10 после удаления '>' заканчивается незакрытым тегом → состояние Tag
        let prev_end = highlight_xml(
            "      <sourceCreditInfoDate>2026-06-10</sourceCreditInfoDate",
            LineState::Text,
            13.0,
        )
        .1;
        assert_eq!(prev_end, LineState::Tag, "незакрытый тег → Tag");

        // следующая строка — закрывающий тег, размечается в состоянии Tag
        let (job, end) = highlight_xml("    </FL_46_UL_36_OrgSource>", prev_end, 13.0);
        assert_eq!(end, LineState::Text, "'>' закрывает тег → Text");
        // вся строка размечена (сумма длин секций == длине строки в байтах)
        let covered: usize = job.sections.iter().map(|s| s.byte_range.len()).sum();
        assert_eq!(covered, "    </FL_46_UL_36_OrgSource>".len(), "строка размечена целиком");
    }
}
