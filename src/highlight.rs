//! Minimal SQL syntax highlighter (purely visual) used by the editor's text layouter.

use crate::theme::p;
use eframe::egui;
use egui::Color32;

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

/// Tokenise `text` into a colored `LayoutJob` (keywords, strings, comments, numbers, calls).
/// Char-index based, so it is safe with non-ASCII content (e.g. Cyrillic comments).
pub fn highlight_sql(text: &str, size: f32) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let mono = crate::code_font(size); // bold — highlighted tokens
    let mono_reg = crate::code_font_regular(size); // normal weight — plain text
    let mut job = LayoutJob::default();
    let cs: Vec<(usize, char)> = text.char_indices().collect();
    let len = cs.len();
    let byte_at = |k: usize| if k < len { cs[k].0 } else { text.len() };
    let push = |job: &mut LayoutJob, s: &str, color: Color32| {
        // only the syntax-coloured tokens read bold; default (TEXT) text stays regular weight
        let font_id = if color == p().text { mono_reg.clone() } else { mono.clone() };
        job.append(
            s,
            0.0,
            TextFormat {
                font_id,
                color,
                ..Default::default()
            },
        );
    };

    let mut k = 0usize;
    while k < len {
        let c = cs[k].1;
        let start = k;
        if c == '-' && k + 1 < len && cs[k + 1].1 == '-' {
            // line comment
            let mut e = k + 2;
            while e < len && cs[e].1 != '\n' {
                e += 1;
            }
            push(&mut job, &text[byte_at(start)..byte_at(e)], p().syn_com);
            k = e;
        } else if c == '/' && k + 1 < len && cs[k + 1].1 == '*' {
            // block comment
            let mut e = k + 2;
            while e + 1 < len && !(cs[e].1 == '*' && cs[e + 1].1 == '/') {
                e += 1;
            }
            e = (e + 2).min(len);
            push(&mut job, &text[byte_at(start)..byte_at(e)], p().syn_com);
            k = e;
        } else if c == '\'' {
            // string literal (handles '' escape)
            let mut e = k + 1;
            while e < len {
                if cs[e].1 == '\'' {
                    if e + 1 < len && cs[e + 1].1 == '\'' {
                        e += 2;
                        continue;
                    }
                    e += 1;
                    break;
                }
                e += 1;
            }
            push(&mut job, &text[byte_at(start)..byte_at(e)], p().syn_str);
            k = e;
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
            let e = k + 1;
            push(&mut job, &text[byte_at(start)..byte_at(e)], p().text);
            k = e;
        }
    }
    job
}
