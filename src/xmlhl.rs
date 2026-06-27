//! Single-line XML highlighter for the virtual editor.
//!
//! Each visible line is highlighted independently, taking into account the state in which the
//! previous line ended (`LineState`) — this way multi-line comments and CDATA are highlighted
//! correctly. The state is part of the galley cache key in the editor.

use crate::theme::{code_font_regular, p};
use eframe::egui;
use egui::text::{LayoutJob, TextFormat};
use egui::Color32;

/// Lexer state at a line boundary.
#[derive(Clone, Copy, PartialEq, Eq, Default, Hash, Debug)]
pub enum LineState {
    /// Ordinary content (text between tags).
    #[default]
    Text,
    /// Inside `<!-- … -->`.
    Comment,
    /// Inside `<![CDATA[ … ]]>`.
    CData,
    /// Inside a tag `<… >` (name/attributes) that spilled onto the next line.
    Tag,
}

impl LineState {
    /// Stable byte code for the state — the editor's `LexState` and the galley cache key.
    pub fn key(self) -> u8 {
        match self {
            LineState::Text => 0,
            LineState::Comment => b'c',
            LineState::CData => b'd',
            LineState::Tag => b'g',
        }
    }

    /// Inverse of [`key`]: editor code → state (0 and unknown codes → Text).
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

/// Highlight a single line, starting in state `state`; return the job and the state at the end of the line.
pub fn highlight_xml(text: &str, state: LineState, size: f32) -> (LayoutJob, LineState) {
    let pal = p();
    let mut job = LayoutJob::default();
    let mut out = Push { job: &mut job, font: code_font_regular(size) };
    let b = text.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    let mut st = state;

    // colors (tags — blue, attributes — dark cyan, values — green,
    // comments — gray, CDATA — "sand")
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
                // inside a tag: the name has already been seen; highlight attributes up to '>'
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
                        // declaration / PI / DOCTYPE — the whole thing up to '>' in one color
                        let end = find(b, i, b">").map(|e| e + 1).unwrap_or(n);
                        out.add(&text[i..end], c_punct);
                        i = end;
                    } else {
                        // opening or closing tag: '<' [/] name
                        let mut j = i + 1;
                        if j < n && b[j] == b'/' {
                            j += 1;
                        }
                        let name_start = j;
                        // Stop the name at a quote too (not just ws/'>'/'/'), so a '<' immediately
                        // followed by a quote (e.g. `<">`) is handled by the in-tag quote logic
                        // exactly as the state-only pass does — otherwise the two passes diverge on
                        // the line's exit state (full → Text, state-only → Tag) and the lines below
                        // stay mis-highlighted as if inside an open tag. See state_only_matches_full_fuzz.
                        while j < n
                            && !b[j].is_ascii_whitespace()
                            && b[j] != b'>'
                            && b[j] != b'/'
                            && b[j] != b'"'
                            && b[j] != b'\''
                        {
                            j += 1;
                        }
                        out.add(&text[i..name_start], c_punct);
                        out.add(&text[name_start..j], c_tag);
                        i = j;
                        st = LineState::Tag;
                    }
                } else {
                    // text content up to the next '<'
                    let end = find(b, i, b"<").unwrap_or(n);
                    out.add(&text[i..end], c_text);
                    i = end;
                }
            }
        }
    }
    (job, st)
}

/// Highlighting inside a tag (attributes and values) starting at `i`; returns (new i, state).
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
            // A lone '/' (not part of '/>') — punctuation. We MUST advance i:
            // the attribute-name scan below excludes '/', so without this i would get stuck
            // in place → infinite loop. This happens when the line is highlighted already
            // in the Tag state and starts with a closing tag `</…>` (for example,
            // the user deleted the '>' on the previous line) — see test tag_state_recovers.
            out.add(&text[i..i + 1], c_punct);
            i += 1;
            continue;
        }
        if ch == b'"' || ch == b'\'' {
            // attribute value; an unclosed quote runs to the end of the line
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
        // attribute name
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
        // Safeguard against looping: any unaccounted-for character must advance i
        // by at least one byte (the invariant "the line lexer always advances").
        let j = j.max(i + 1);
        out.add(&text[i..j], c_attr);
        i = j;
    }
    (i, LineState::Tag)
}

/// State transition over a line only, without building a LayoutJob — for lazily
/// advancing the state cache (LexCache) within a per-frame budget.
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
                // skip attributes up to '>', accounting for quotes
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

    /// The lightweight state-only pass must produce the same transitions as the full one.
    /// The full pass must ALWAYS terminate (the line lexer does not loop):
    /// if `highlight_xml` hung on some input, this test would never return.
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
            // a lone '/' inside the Tag state — used to loop lex_in_tag:
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

    /// Exhaustive convergence: for EVERY short byte string over the XML metacharacter alphabet and
    /// every start state, the full pass and the state-only pass must exit in the SAME `LineState`.
    /// The editor propagates inter-line state via the state-only `advance` path only, so a divergence
    /// is not a hang here — it surfaces as the lines below being highlighted as if still inside an
    /// open tag/comment until a re-anchor. Brute force is the only way to guarantee the invariant for
    /// every line (this is the regression for the `<"`/`<'` divergence).
    #[test]
    fn state_only_matches_full_fuzz() {
        // All ASCII, so every prefix is valid UTF-8.
        const ALPHA: &[u8] = b"<>/!-[]?=\"' aCDAT";
        fn rec(prefix: &mut Vec<u8>, depth: usize) {
            let s = std::str::from_utf8(prefix).expect("ASCII alphabet → valid UTF-8");
            for st in [LineState::Text, LineState::Comment, LineState::CData, LineState::Tag] {
                let full = highlight_xml(s, st, 13.0).1;
                let fast = highlight_xml_state_only(s, st);
                assert_eq!(full, fast, "line={s:?} start={st:?}");
            }
            if depth == 0 {
                return;
            }
            for &c in ALPHA {
                prefix.push(c);
                rec(prefix, depth - 1);
                prefix.pop();
            }
        }
        rec(&mut Vec::new(), 4);
    }

    /// Hang regression (invalid_1gb.xml): the user deletes the '>' on a line
    /// ending with a tag — the next line is highlighted already in the Tag state and
    /// starts with a closing tag `</…>`. The full lexer must terminate and
    /// highlight the whole line (not get stuck on the leading '/').
    #[test]
    fn tag_state_recovers_on_closing_tag() {
        // line 10 after deleting '>' ends with an unclosed tag → Tag state
        let prev_end = highlight_xml(
            "      <sourceCreditInfoDate>2026-06-10</sourceCreditInfoDate",
            LineState::Text,
            13.0,
        )
        .1;
        assert_eq!(prev_end, LineState::Tag, "unclosed tag → Tag");

        // the next line — a closing tag, highlighted in the Tag state
        let (job, end) = highlight_xml("    </FL_46_UL_36_OrgSource>", prev_end, 13.0);
        assert_eq!(end, LineState::Text, "'>' closes the tag → Text");
        // the whole line is highlighted (sum of section lengths == line length in bytes)
        let covered: usize = job.sections.iter().map(|s| s.byte_range.len()).sum();
        assert_eq!(covered, "    </FL_46_UL_36_OrgSource>".len(), "the whole line is highlighted");
    }
}
