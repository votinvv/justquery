//! Потоковый pretty-printer XML без потери данных.
//!
//! Работает по снимку piece table через quick-xml: память O(глубины дерева) — буферизуется
//! только прямое инлайн-содержимое текущего элемента, дети пишутся по мере обхода.
//!
//! Правила:
//! - отступ 2 пробела на уровень, EOL — LF;
//! - XML-декларация `<?xml version="1.0" encoding="UTF-8"?>` первой строкой;
//! - элемент без детей и без значащего текста → `<a/>`;
//! - элемент только с текстом → `<a>значение</a>` в одну строку (значение не нормализуется);
//! - элемент с детьми — дети с новой строки с отступом; whitespace-only текст между детьми
//!   отбрасывается;
//! - CDATA, комментарии, PI сохраняются (текст и атрибуты — в исходной экранированной форме).
//!
//! Не well-formed вход → `ProcMsg::FormatErr` с номером строки; выходной файл удаляется.

use crate::doc::piece_table::PieceSnapshot;
use crate::doc::temp_file;
use crate::proc::ProcMsg;
use quick_xml::events::Event;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// Часть инлайн-содержимого элемента: сырой (экранированный) текст или CDATA.
enum Inline {
    Text(Vec<u8>),
    CData(Vec<u8>),
}

struct Frame {
    name: Vec<u8>,
    children: bool,
    inline: Vec<Inline>,
}

/// Запустить фоновое форматирование снимка документа во временный файл.
pub fn spawn_format(snap: PieceSnapshot, cancel: Arc<AtomicBool>, tx: Sender<ProcMsg>) {
    std::thread::spawn(move || {
        let out_path = temp_file("utf8");
        let msg = match run(&snap, &out_path, &cancel, &tx) {
            Ok(()) => ProcMsg::FormatOk { out_path },
            Err(e) => {
                let _ = std::fs::remove_file(&out_path);
                match e {
                    RunErr::Cancelled => ProcMsg::Cancelled,
                    RunErr::Xml { pos, msg } => {
                        let (line, _col) = line_col_at(&snap, pos);
                        ProcMsg::FormatErr {
                            line,
                            msg: format!("document is not well-formed XML: {msg}"),
                        }
                    }
                    RunErr::Io(e) => ProcMsg::Failed(format!("write error: {e}")),
                }
            }
        };
        let _ = tx.send(msg);
    });
}

enum RunErr {
    Cancelled,
    Xml { pos: u64, msg: String },
    Io(std::io::Error),
}

impl From<std::io::Error> for RunErr {
    fn from(e: std::io::Error) -> Self {
        RunErr::Io(e)
    }
}

fn run(
    snap: &PieceSnapshot,
    out_path: &std::path::Path,
    cancel: &AtomicBool,
    tx: &Sender<ProcMsg>,
) -> Result<(), RunErr> {
    let total = snap.len().max(1) as u64;
    let mut reader =
        quick_xml::Reader::from_reader(std::io::BufReader::new(snap.reader()));
    reader.config_mut().check_end_names = true;
    let mut out = std::io::BufWriter::new(std::fs::File::create(out_path)?);
    let mut buf: Vec<u8> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut pending_start = false; // старт-тег верхнего элемента не закрыт '>'
    let mut prog = crate::proc::ProgressThrottle::new();

    out.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")?;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(RunErr::Cancelled);
        }
        let pos = reader.buffer_position();
        let ev = reader.read_event_into(&mut buf).map_err(|e| RunErr::Xml {
            pos: reader.error_position(),
            msg: e.to_string(),
        })?;
        match ev {
            Event::Decl(_) => {} // пишем свою декларацию
            Event::DocType(d) => {
                out.write_all(b"<!DOCTYPE")?;
                out.write_all(d.as_ref())?;
                out.write_all(b">\n")?;
            }
            Event::PI(pi) => {
                before_child(&mut out, &mut stack, &mut pending_start)?;
                indent(&mut out, stack.len())?;
                out.write_all(b"<?")?;
                out.write_all(pi.as_ref())?;
                out.write_all(b"?>\n")?;
                if let Some(f) = stack.last_mut() {
                    f.children = true;
                }
            }
            Event::Comment(c) => {
                before_child(&mut out, &mut stack, &mut pending_start)?;
                indent(&mut out, stack.len())?;
                out.write_all(b"<!--")?;
                out.write_all(c.as_ref())?;
                out.write_all(b"-->\n")?;
                if let Some(f) = stack.last_mut() {
                    f.children = true;
                }
            }
            Event::Start(e) => {
                before_child(&mut out, &mut stack, &mut pending_start)?;
                indent(&mut out, stack.len())?;
                write_start_tag(&mut out, &e, &reader, pos)?;
                stack.push(Frame {
                    name: e.name().as_ref().to_vec(),
                    children: false,
                    inline: Vec::new(),
                });
                pending_start = true;
            }
            Event::Empty(e) => {
                before_child(&mut out, &mut stack, &mut pending_start)?;
                indent(&mut out, stack.len())?;
                write_start_tag(&mut out, &e, &reader, pos)?;
                out.write_all(b"/>\n")?;
                if let Some(f) = stack.last_mut() {
                    f.children = true;
                }
            }
            Event::End(_) => {
                let Some(frame) = stack.pop() else {
                    return Err(RunErr::Xml {
                        pos,
                        msg: "лишний закрывающий тег".to_owned(),
                    });
                };
                let depth = stack.len();
                if pending_start {
                    pending_start = false;
                    if inline_is_empty(&frame.inline) {
                        out.write_all(b"/>\n")?;
                    } else {
                        out.write_all(b">")?;
                        render_inline(&mut out, &frame.inline)?;
                        out.write_all(b"</")?;
                        out.write_all(&frame.name)?;
                        out.write_all(b">\n")?;
                    }
                } else {
                    // блочный элемент: сбросить хвостовой межэлементный текст
                    flush_inline_as_lines(&mut out, &frame.inline, depth + 1)?;
                    indent(&mut out, depth)?;
                    out.write_all(b"</")?;
                    out.write_all(&frame.name)?;
                    out.write_all(b">\n")?;
                }
                if let Some(f) = stack.last_mut() {
                    f.children = true;
                }
            }
            Event::Text(t) => {
                if let Some(f) = stack.last_mut() {
                    f.inline.push(Inline::Text(t.as_ref().to_vec()));
                }
            }
            // quick-xml ≥0.38: `&имя;` приходит отдельным событием — возвращаем его в
            // текст В ИСХОДНОЙ экранированной форме (форматтер ничего не перекодирует)
            Event::GeneralRef(r) => {
                if let Some(f) = stack.last_mut() {
                    let mut raw = Vec::with_capacity(r.as_ref().len() + 2);
                    raw.push(b'&');
                    raw.extend_from_slice(r.as_ref());
                    raw.push(b';');
                    f.inline.push(Inline::Text(raw));
                }
            }
            Event::CData(c) => {
                if let Some(f) = stack.last_mut() {
                    f.inline.push(Inline::CData(c.as_ref().to_vec()));
                }
            }
            Event::Eof => break,
        }
        buf.clear();
        // прогресс по потреблённым байтам
        let p = (reader.buffer_position() as f32 / total as f32) * 100.0;
        prog.maybe_send(tx, p, 99.0);
    }
    if !stack.is_empty() {
        return Err(RunErr::Xml {
            pos: reader.buffer_position(),
            msg: format!(
                "не закрытый элемент <{}>",
                String::from_utf8_lossy(&stack.last().unwrap().name)
            ),
        });
    }
    out.flush()?;
    Ok(())
}

/// Старт-тег с атрибутами: имя как есть, значения в исходной экранированной форме,
/// кавычки нормализуются к двойным (одинарные внутри значения переэкранируются).
fn write_start_tag<W: Write, R>(
    out: &mut W,
    e: &quick_xml::events::BytesStart,
    _reader: &quick_xml::Reader<R>,
    pos: u64,
) -> Result<(), RunErr> {
    out.write_all(b"<")?;
    out.write_all(e.name().as_ref())?;
    for attr in e.attributes().with_checks(true) {
        let attr = attr.map_err(|err| RunErr::Xml { pos, msg: err.to_string() })?;
        out.write_all(b" ")?;
        out.write_all(attr.key.as_ref())?;
        out.write_all(b"=\"")?;
        // сырое значение уже экранировано; заменить только незаэкранированные '"',
        // возможные при одинарных кавычках в исходнике
        let raw = attr.value.as_ref();
        if raw.contains(&b'"') {
            for &b in raw {
                if b == b'"' {
                    out.write_all(b"&quot;")?;
                } else {
                    out.write_all(&[b])?;
                }
            }
        } else {
            out.write_all(raw)?;
        }
        out.write_all(b"\"")?;
    }
    Ok(())
}

fn indent<W: Write>(out: &mut W, depth: usize) -> std::io::Result<()> {
    const SPACES: &[u8] = b"                                                                ";
    let mut n = depth * 2;
    while n > 0 {
        let take = n.min(SPACES.len());
        out.write_all(&SPACES[..take])?;
        n -= take;
    }
    Ok(())
}

fn render_inline<W: Write>(out: &mut W, parts: &[Inline]) -> std::io::Result<()> {
    for p in parts {
        match p {
            Inline::Text(t) => out.write_all(t)?,
            Inline::CData(c) => {
                out.write_all(b"<![CDATA[")?;
                out.write_all(c)?;
                out.write_all(b"]]>")?;
            }
        }
    }
    Ok(())
}

/// Пусто, если нет CDATA и весь текст — пробельный.
fn inline_is_empty(parts: &[Inline]) -> bool {
    parts.iter().all(|p| match p {
        Inline::CData(_) => false,
        Inline::Text(t) => t.iter().all(|b| b.is_ascii_whitespace()),
    })
}

/// Значащий инлайн-текст между детьми (mixed content) — отдельной строкой с отступом.
fn flush_inline_as_lines<W: Write>(
    out: &mut W,
    parts: &[Inline],
    depth: usize,
) -> std::io::Result<()> {
    if inline_is_empty(parts) {
        return Ok(());
    }
    let mut rendered = Vec::new();
    render_inline(&mut rendered, parts)?;
    let trimmed = trim_ascii(&rendered);
    if !trimmed.is_empty() {
        indent(out, depth)?;
        out.write_all(trimmed)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

fn trim_ascii(b: &[u8]) -> &[u8] {
    let start = b.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(b.len());
    let end = b.iter().rposition(|c| !c.is_ascii_whitespace()).map(|i| i + 1).unwrap_or(start);
    &b[start..end]
}

/// Закрыть pending старт-тег как блочный / сбросить межэлементный текст перед ребёнком.
fn before_child<W: Write>(
    out: &mut W,
    stack: &mut [Frame],
    pending_start: &mut bool,
) -> std::io::Result<()> {
    let depth = stack.len();
    if *pending_start {
        let frame = stack.last_mut().expect("pending_start ⇒ стек не пуст");
        out.write_all(b">\n")?;
        frame.children = true;
        *pending_start = false;
        let parts = std::mem::take(&mut frame.inline);
        flush_inline_as_lines(out, &parts, depth)?;
    } else if let Some(frame) = stack.last_mut() {
        if frame.children {
            let parts = std::mem::take(&mut frame.inline);
            flush_inline_as_lines(out, &parts, depth)?;
        }
    }
    Ok(())
}

/// (строка, колонка) 1-based для байтового смещения `pos` в снимке.
fn line_col_at(snap: &PieceSnapshot, pos: u64) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    let mut seen = 0u64;
    let mut done = false;
    snap.for_each_chunk(|chunk| {
        if done {
            return;
        }
        for &b in chunk {
            if seen >= pos {
                done = true;
                return;
            }
            seen += 1;
            if b == b'\n' {
                line += 1;
                col = 1;
            } else if (b & 0xC0) != 0x80 {
                col += 1;
            }
        }
    });
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Document;
    use std::sync::mpsc::channel;

    /// Прогнать форматирование синхронно; вернуть результат или ошибку (line, msg).
    fn fmt(text: &str) -> Result<String, (usize, String)> {
        let mut d = Document::new_empty();
        d.replace_range((0, 0), (0, 0), text);
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        spawn_format(d.snapshot(), cancel, tx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(ProcMsg::FormatOk { out_path, .. }) => {
                    let s = std::fs::read_to_string(&out_path)
                        .map_err(|e| (0, format!("cannot read formatter output: {e}")))?;
                    let _ = std::fs::remove_file(out_path);
                    return Ok(s);
                }
                Ok(ProcMsg::FormatErr { line, msg }) => return Err((line, msg)),
                Ok(ProcMsg::Failed(e)) => return Err((0, format!("formatter failed: {e}"))),
                Ok(_) => {}
                Err(_) if std::time::Instant::now() > deadline => {
                    return Err((0, "format timed out".to_string()));
                }
                Err(_) => {}
            }
        }
    }

    #[test]
    fn basic_pretty_print() {
        let out = fmt("<a><b>1</b><c/></a>").unwrap();
        assert_eq!(
            out,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<a>\n  <b>1</b>\n  <c/>\n</a>\n"
        );
    }

    #[test]
    fn empty_element_collapses() {
        let out = fmt("<a>\n   \n</a>").unwrap();
        assert_eq!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<a/>\n");
    }

    #[test]
    fn text_only_inline() {
        let out = fmt("<a>  значение  </a>").unwrap();
        // значение не нормализуется и не обрезается
        assert!(out.contains("<a>  значение  </a>"));
    }

    #[test]
    fn attributes_preserved() {
        let out = fmt(r#"<a x="1" y="&lt;&quot;"><b/></a>"#).unwrap();
        assert!(out.contains(r#"<a x="1" y="&lt;&quot;">"#), "{out}");
    }

    #[test]
    fn single_quoted_attr_normalized() {
        let out = fmt(r#"<a x='say "hi"'/>"#).unwrap();
        assert!(out.contains(r#"x="say &quot;hi&quot;""#), "{out}");
    }

    #[test]
    fn cdata_and_comments_preserved() {
        let src = "<a><!-- комментарий --><b><![CDATA[raw <data> &]]></b></a>";
        let out = fmt(src).unwrap();
        assert!(out.contains("<!-- комментарий -->"), "{out}");
        assert!(out.contains("<![CDATA[raw <data> &]]>"), "{out}");
    }

    #[test]
    fn mixed_content_kept_as_lines() {
        let out = fmt("<a>головной текст<b>1</b>хвост</a>").unwrap();
        assert!(out.contains("  головной текст\n"), "{out}");
        assert!(out.contains("  хвост\n"), "{out}");
    }

    #[test]
    fn entity_refs_preserved() {
        let out = fmt("<a>a &amp; b &lt; c</a>").unwrap();
        assert!(out.contains("<a>a &amp; b &lt; c</a>"), "{out}");
    }

    #[test]
    fn malformed_reports_line() {
        let err = fmt("<a>\n<b>\n</wrong>\n</a>").unwrap_err();
        assert_eq!(err.0, 3, "{err:?}");
    }

    #[test]
    fn unclosed_reports_error() {
        assert!(fmt("<a><b></b>").is_err());
    }

    #[test]
    fn processing_instruction_preserved() {
        let out = fmt("<a><?target data?></a>").unwrap();
        assert!(out.contains("<?target data?>"), "{out}");
    }

    #[test]
    fn deep_nesting_indents() {
        let out = fmt("<a><b><c><d>x</d></c></b></a>").unwrap();
        assert!(out.contains("      <d>x</d>"), "{out}");
    }

    #[test]
    fn crlf_input_normalized_to_lf() {
        let out = fmt("<a>\r\n<b>1</b>\r\n</a>").unwrap();
        assert!(!out.contains('\r'), "{out}");
    }
}
