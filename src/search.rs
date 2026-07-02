//! Background document search: plain text, case-insensitive.
//!
//! Works over a piece table snapshot (the UI is not blocked, edits don't interfere). Streaming
//! pass over the bytes: lines are split on `\n`, comparison is char-by-char with a simple case fold
//! (first code point of `to_lowercase`), so match columns are exact. Very long
//! lines are processed in segments with the tail carried over — memory stays bounded.

use crate::doc::piece_table::PieceSnapshot;
use crate::proc::{ProcMsg, SearchMatch};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

const BATCH: usize = 200; // matches per batch
const SEGMENT: usize = 4 * 1024 * 1024; // segmentation threshold for a very long line
const PREVIEW_BEFORE: usize = 30; // chars of context before the match
const PREVIEW_AFTER: usize = 50; // chars after

/// Simple case fold: first code point of to_lowercase (1:1 for Cyrillic/Latin).
#[inline]
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Start a background search for `query` over the document snapshot.
pub fn spawn_search(
    snap: PieceSnapshot,
    query: String,
    cancel: Arc<AtomicBool>,
    tx: Sender<ProcMsg>,
) {
    std::thread::spawn(move || {
        let needle: Vec<char> = query.chars().map(fold).collect();
        if needle.is_empty() {
            let _ = tx.send(ProcMsg::Done);
            return;
        }
        let mut st = State {
            needle,
            tx: &tx,
            cancel: &cancel,
            batch: Vec::new(),
            line: 0,
            col_base: 0,
            carry: Vec::new(),
            last_flush: std::time::Instant::now(),
            cancelled: false,
        };

        snap.for_each_chunk(|chunk| {
            if st.cancelled {
                return;
            }
            st.feed(chunk);
        });
        if !st.cancelled {
            // tail without a trailing \n — the last line
            st.flush_line();
        }
        if !st.batch.is_empty() {
            let _ = tx.send(ProcMsg::SearchBatch(std::mem::take(&mut st.batch)));
        }
        let _ = tx.send(if st.cancelled { ProcMsg::Cancelled } else { ProcMsg::Done });
    });
}

struct State<'a> {
    needle: Vec<char>,
    tx: &'a Sender<ProcMsg>,
    cancel: &'a AtomicBool,
    batch: Vec<SearchMatch>,
    line: usize,     // current line (0-based)
    col_base: usize, // chars already processed in the current line (segmentation)
    carry: Vec<u8>,  // unfinished line (bytes)
    last_flush: std::time::Instant,
    cancelled: bool,
}

impl State<'_> {
    /// Snapshot pieces can be huge (an entire mmap) — split into blocks
    /// so that carry doesn't grow to the size of the file.
    fn feed(&mut self, data: &[u8]) {
        for block in data.chunks(SEGMENT) {
            if self.check_cancel() {
                return;
            }
            self.feed_block(block);
        }
    }

    fn feed_block(&mut self, data: &[u8]) {
        let mut pos = 0usize;
        for nl in memchr::memchr_iter(b'\n', data) {
            if self.check_cancel() {
                return;
            }
            self.carry.extend_from_slice(&data[pos..nl]);
            self.flush_line();
            self.line += 1;
            self.col_base = 0;
            pos = nl + 1;
        }
        self.carry.extend_from_slice(&data[pos..]);
        // very long line: process a segment, leaving the tail to join with the next one
        while self.carry.len() > SEGMENT {
            if self.check_cancel() {
                return;
            }
            self.process_segment();
        }
        // matches are emitted as they appear: an incomplete batch is flushed about every 150 ms
        if !self.batch.is_empty() && self.last_flush.elapsed().as_millis() >= 150 {
            let _ = self.tx.send(ProcMsg::SearchBatch(std::mem::take(&mut self.batch)));
            self.last_flush = std::time::Instant::now();
        }
    }

    fn check_cancel(&mut self) -> bool {
        if self.cancel.load(Ordering::Relaxed) {
            self.cancelled = true;
        }
        self.cancelled
    }

    /// A full line is assembled in carry: search it and flush.
    fn flush_line(&mut self) {
        if self.carry.is_empty() {
            return;
        }
        // strip the \r of CRLF
        if self.carry.last() == Some(&b'\r') {
            self.carry.pop();
        }
        let bytes = std::mem::take(&mut self.carry);
        let text = String::from_utf8_lossy(&bytes);
        self.scan(&text, usize::MAX);
    }

    /// A segment of a very long line: search the prefix, carry the tail
    /// (needle length − 1 chars) over to the next segment.
    fn process_segment(&mut self) {
        let bytes = std::mem::take(&mut self.carry);
        // UTF-8 boundary: don't cut in the middle of a character
        let mut cut = bytes.len().min(SEGMENT);
        while cut > 0 && (bytes[cut] & 0xC0) == 0x80 {
            cut -= 1;
        }
        let (seg, rest) = bytes.split_at(cut);
        let text = String::from_utf8_lossy(seg).into_owned();
        let total_chars = text.chars().count();
        let keep_chars = self.needle.len().saturating_sub(1).min(total_chars);
        let scan_limit = total_chars - keep_chars; // starts only within the prefix
        self.scan(&text, scan_limit);
        // carry over the last keep_chars chars + the leftover bytes
        let keep_byte = text
            .char_indices()
            .nth(scan_limit)
            .map(|(b, _)| b)
            .unwrap_or(text.len());
        self.col_base += scan_limit;
        self.carry = text.as_bytes()[keep_byte..].to_vec();
        self.carry.extend_from_slice(rest);
    }

    /// Find occurrences in `text`; match starts only up to `start_limit` (in chars).
    fn scan(&mut self, text: &str, start_limit: usize) {
        let n = self.needle.len();
        let chars: Vec<char> = text.chars().collect();
        if chars.len() < n {
            return;
        }
        let folded: Vec<char> = chars.iter().map(|&c| fold(c)).collect();
        let first = self.needle[0];
        let mut limit = chars.len() - n + 1;
        if start_limit != usize::MAX {
            limit = limit.min(start_limit);
        }
        let mut i = 0usize;
        while i < limit {
            if folded[i] == first && folded[i..i + n] == self.needle[..] {
                let col = self.col_base + i;
                let preview = make_preview(&chars, i, n);
                self.batch.push(SearchMatch { line: self.line, col, len: n, preview });
                if self.batch.len() >= BATCH {
                    let _ =
                        self.tx.send(ProcMsg::SearchBatch(std::mem::take(&mut self.batch)));
                    if self.check_cancel() {
                        return;
                    }
                }
                i += n; // non-overlapping matches
            } else {
                i += 1;
            }
        }
    }
}

/// A snippet of the line around the match for the result grid.
fn make_preview(chars: &[char], start: usize, len: usize) -> String {
    let from = start.saturating_sub(PREVIEW_BEFORE);
    let to = (start + len + PREVIEW_AFTER).min(chars.len());
    let mut s = String::with_capacity((to - from) * 2 + 2);
    if from > 0 {
        s.push('…');
    }
    s.extend(&chars[from..to]);
    if to < chars.len() {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Document;
    use std::sync::mpsc::channel;

    /// Run the search synchronously and collect the matches.
    fn run_search(text: &str, query: &str) -> Vec<SearchMatch> {
        let mut d = Document::new_empty();
        d.replace_range((0, 0), (0, 0), text);
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        spawn_search(d.snapshot(), query.to_owned(), cancel, tx);
        let mut out = Vec::new();
        // wait for completion (with a safety timeout)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(ProcMsg::SearchBatch(b)) => out.extend(b),
                Ok(ProcMsg::Done) | Ok(ProcMsg::Cancelled) => break,
                Ok(_) => {}
                Err(_) if std::time::Instant::now() > deadline => panic!("search hung"),
                Err(_) => {}
            }
        }
        out
    }

    #[test]
    fn finds_case_insensitive() {
        let m = run_search("Hello\nwORLD hello\n", "hello");
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].line, m[0].col), (0, 0));
        assert_eq!((m[1].line, m[1].col), (1, 6));
    }

    #[test]
    fn finds_cyrillic() {
        let m = run_search("привет ПРИВЕТ\nещё Привет", "привет");
        assert_eq!(m.len(), 3);
        assert_eq!((m[1].line, m[1].col), (0, 7));
        assert_eq!((m[2].line, m[2].col), (1, 4));
    }

    #[test]
    fn cols_in_chars_not_bytes() {
        let m = run_search("ффф abc", "abc");
        assert_eq!(m[0].col, 4);
    }

    #[test]
    fn no_overlapping_matches() {
        let m = run_search("aaaa", "aa");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].col, 0);
        assert_eq!(m[1].col, 2);
    }

    #[test]
    fn preview_has_context() {
        let m = run_search("<element>значение</element>", "значение");
        assert!(m[0].preview.contains("значение"));
        assert!(m[0].preview.contains("<element>"));
    }

    #[test]
    fn crlf_lines() {
        let m = run_search("ab\r\ncd ab\r\n", "ab");
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].line, m[0].col), (0, 0));
        assert_eq!((m[1].line, m[1].col), (1, 3));
    }

    #[test]
    fn long_line_segments() {
        // line longer than the segmentation threshold: matches at the start, middle, and the join
        let mut text = String::new();
        text.push_str("needle ");
        text.push_str(&"x".repeat(SEGMENT));
        text.push_str(" needle");
        let m = run_search(&text, "needle");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].col, 0);
        assert_eq!(m[1].col, 7 + SEGMENT + 1);
    }

    #[test]
    fn match_straddles_segment_boundary() {
        // match landing exactly across a segment boundary
        let pre = SEGMENT - 3; // «nee» before the boundary, «dle» after
        let mut text = "y".repeat(pre);
        text.push_str("needle");
        text.push_str(&"y".repeat(64));
        let m = run_search(&text, "needle");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].col, pre);
    }
}
