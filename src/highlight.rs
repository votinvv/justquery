//! Minimal stateful SQL syntax highlighter (purely visual) used by the virtual editor.
//!
//! Каждая строка размечается отдельно, с учётом состояния, в котором закончилась
//! предыдущая ([`LineState`]) — так многострочные `/* … */` и строковые литералы
//! `'…'` подсвечиваются корректно. Состояние входит в ключ кэша galley редактора.

use crate::theme::p;
use eframe::egui;
use egui::text::{LayoutJob, TextFormat};
use egui::Color32;

/// Состояние лексера на границе строки.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum LineState {
    /// Обычный SQL-текст.
    #[default]
    Text,
    /// Внутри блочного комментария `/* … */`.
    BlockComment,
    /// Внутри строкового литерала `'…'` (в Postgres может переноситься).
    Str,
}

impl LineState {
    /// Стабильный байтовый код состояния — `LexState` редактора и ключ кэша galley.
    pub fn key(self) -> u8 {
        match self {
            LineState::Text => 0,
            LineState::BlockComment => b'c',
            LineState::Str => b's',
        }
    }

    /// Обратное к [`key`]: код редактора → состояние (0 и незнакомые коды → Text).
    pub fn from_key(k: u8) -> Self {
        match k {
            b'c' => LineState::BlockComment,
            b's' => LineState::Str,
            _ => LineState::Text,
        }
    }
}

fn is_keyword(w: &str) -> bool {
    const KW: &[&str] = &[
        "select", "from", "where", "and", "or", "not", "null", "as", "join", "left", "right",
        "inner", "outer", "full", "cross", "on", "using", "group", "by", "order", "having",
        "limit", "offset", "with", "distinct", "insert", "into", "values", "update", "set",
        "delete", "create", "table", "view", "index", "drop", "alter", "add", "column",
        "primary", "key", "foreign", "references", "default", "case", "when", "then", "else",
        "end", "over", "partition", "asc", "desc", "interval", "in", "exists", "between",
        "like", "ilike", "is", "union", "all", "returning", "begin", "commit", "rollback",
        "true", "false",
    ];
    let lw = w.to_ascii_lowercase();
    KW.contains(&lw.as_str())
}

/// Разметить одну строку, начиная в состоянии `state`; вернуть job и состояние на конце.
/// Char-index based, so it is safe with non-ASCII content (e.g. Cyrillic comments).
pub fn highlight_sql(text: &str, state: LineState, size: f32) -> (LayoutJob, LineState) {
    let mono = crate::code_font(size); // bold — highlighted tokens
    let mono_reg = crate::code_font_regular(size); // normal weight — plain text
    let mut job = LayoutJob::default();
    let cs: Vec<(usize, char)> = text.char_indices().collect();
    let len = cs.len();
    let byte_at = |k: usize| if k < len { cs[k].0 } else { text.len() };
    let push = |job: &mut LayoutJob, s: &str, color: Color32| {
        if s.is_empty() {
            return;
        }
        // only the syntax-coloured tokens read bold; default (TEXT) text stays regular weight
        let font_id = if color == p().text { mono_reg.clone() } else { mono.clone() };
        job.append(s, 0.0, TextFormat { font_id, color, ..Default::default() });
    };

    let mut st = state;
    let mut k = 0usize;
    while k < len {
        match st {
            LineState::BlockComment => {
                let mut e = k;
                let mut closed = false;
                while e < len {
                    if cs[e].1 == '*' && e + 1 < len && cs[e + 1].1 == '/' {
                        e += 2;
                        closed = true;
                        break;
                    }
                    e += 1;
                }
                push(&mut job, &text[byte_at(k)..byte_at(e)], p().syn_com);
                k = e;
                if closed {
                    st = LineState::Text;
                }
            }
            LineState::Str => {
                // закрывающая кавычка с учётом '' экранирования
                let mut e = k;
                let mut closed = false;
                while e < len {
                    if cs[e].1 == '\'' {
                        if e + 1 < len && cs[e + 1].1 == '\'' {
                            e += 2;
                            continue;
                        }
                        e += 1;
                        closed = true;
                        break;
                    }
                    e += 1;
                }
                push(&mut job, &text[byte_at(k)..byte_at(e)], p().syn_str);
                k = e;
                if closed {
                    st = LineState::Text;
                }
            }
            LineState::Text => {
                let c = cs[k].1;
                let start = k;
                if c == '-' && k + 1 < len && cs[k + 1].1 == '-' {
                    // line comment до конца строки
                    push(&mut job, &text[byte_at(start)..], p().syn_com);
                    k = len;
                } else if c == '/' && k + 1 < len && cs[k + 1].1 == '*' {
                    push(&mut job, &text[byte_at(start)..byte_at(start + 2)], p().syn_com);
                    k += 2;
                    st = LineState::BlockComment;
                } else if c == '\'' {
                    push(&mut job, &text[byte_at(start)..byte_at(start + 1)], p().syn_str);
                    k += 1;
                    st = LineState::Str;
                } else if c.is_ascii_digit() {
                    let mut e = k + 1;
                    while e < len && (cs[e].1.is_ascii_digit() || cs[e].1 == '.') {
                        e += 1;
                    }
                    push(&mut job, &text[byte_at(start)..byte_at(e)], p().syn_num);
                    k = e;
                } else if c.is_alphabetic() || c == '_' {
                    let mut e = k + 1;
                    while e < len && (cs[e].1.is_alphanumeric() || cs[e].1 == '_') {
                        e += 1;
                    }
                    let word = &text[byte_at(start)..byte_at(e)];
                    let color = if is_keyword(word) {
                        p().syn_kw
                    } else if e < len && cs[e].1 == '(' {
                        p().syn_fn
                    } else {
                        p().text
                    };
                    push(&mut job, word, color);
                    k = e;
                } else {
                    push(&mut job, &text[byte_at(start)..byte_at(k + 1)], p().text);
                    k += 1;
                }
            }
        }
    }
    (job, st)
}

/// Только переход состояния по строке, без построения LayoutJob — для ленивого
/// продвижения кэша состояний (LexCache) с бюджетом на кадр.
pub fn highlight_sql_state_only(text: &str, state: LineState) -> LineState {
    let cs: Vec<char> = text.chars().collect();
    let len = cs.len();
    let mut st = state;
    let mut k = 0usize;
    while k < len {
        match st {
            LineState::BlockComment => {
                while k < len {
                    if cs[k] == '*' && k + 1 < len && cs[k + 1] == '/' {
                        k += 2;
                        st = LineState::Text;
                        break;
                    }
                    k += 1;
                }
            }
            LineState::Str => {
                while k < len {
                    if cs[k] == '\'' {
                        if k + 1 < len && cs[k + 1] == '\'' {
                            k += 2;
                            continue;
                        }
                        k += 1;
                        st = LineState::Text;
                        break;
                    }
                    k += 1;
                }
            }
            LineState::Text => {
                let c = cs[k];
                if c == '-' && k + 1 < len && cs[k + 1] == '-' {
                    k = len; // line comment до конца строки
                } else if c == '/' && k + 1 < len && cs[k + 1] == '*' {
                    k += 2;
                    st = LineState::BlockComment;
                } else if c == '\'' {
                    k += 1;
                    st = LineState::Str;
                } else {
                    k += 1;
                }
            }
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_after(line: &str, st: LineState) -> LineState {
        highlight_sql_state_only(line, st)
    }

    #[test]
    fn plain_sql_round_trip() {
        assert_eq!(state_after("select 1 from t -- x", LineState::Text), LineState::Text);
    }

    #[test]
    fn block_comment_spans_lines() {
        assert_eq!(state_after("select /* start", LineState::Text), LineState::BlockComment);
        assert_eq!(state_after("still inside", LineState::BlockComment), LineState::BlockComment);
        assert_eq!(state_after("end */ 1", LineState::BlockComment), LineState::Text);
    }

    #[test]
    fn string_spans_lines_and_escapes() {
        assert_eq!(state_after("select 'multi", LineState::Text), LineState::Str);
        assert_eq!(state_after("line'", LineState::Str), LineState::Text);
        // '' внутри литерала не закрывает его
        assert_eq!(state_after("select 'it''s open", LineState::Text), LineState::Str);
        assert_eq!(state_after("select 'closed'", LineState::Text), LineState::Text);
    }

    #[test]
    fn line_comment_hides_transitions() {
        assert_eq!(state_after("-- /* not a comment start", LineState::Text), LineState::Text);
        assert_eq!(state_after("-- 'not a string", LineState::Text), LineState::Text);
    }

    /// Лёгкий state-only проход обязан давать те же переходы, что и полный.
    #[test]
    fn state_only_matches_full() {
        let lines = [
            "select * from t where a = 'x'",
            "select /* start",
            "middle */",
            "'tail",
            "it''s still open",
            "closed' rest /* y",
            "-- comment 'with quote",
            "plain text",
        ];
        for st in [LineState::Text, LineState::BlockComment, LineState::Str] {
            for line in lines {
                let full = highlight_sql(line, st, 13.0).1;
                let fast = highlight_sql_state_only(line, st);
                assert_eq!(full, fast, "line={line:?} start={st:?}");
            }
        }
    }
}
