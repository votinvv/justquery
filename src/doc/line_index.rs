//!
//! Lazy chunked line index for files up to ~1 GB.

#![allow(dead_code)] // document model: this build does not use the whole API
//!
//! Stores the positions of line starts. So that an edit does not shift the whole array
//! (tens of millions of lines), the offsets within a chunk are relative, and the chunk base
//! is a lazily recomputed prefix sum of chunk lengths. An edit rebuilds only the affected chunks.
//!
//! Line convention: a line start is byte 0 and the byte right after each `\n`.
//! So `"abc"` → 1 line; `"abc\n"` → 2 lines (the second one empty). `\r` is not treated
//! as a separator during indexing (CR-only EOL is handled when reading the line in document).

/// Target number of lines per chunk.
const TARGET: usize = 8192;

/// A chunk of lines: relative offsets of line starts + length in bytes and characters.
struct Chunk {
    rel: Vec<u32>, // rel[0] == 0; relative offsets within the chunk
    byte_len: u64,
    chars_len: u64, // code points in the chunk (for the caret position in characters)
}

/// Find the absolute offsets of line starts in `data` (including the first one — `base`).
pub(super) fn scan_starts(data: &[u8], base: u64) -> Vec<u64> {
    let mut starts = Vec::with_capacity(data.len() / 64 + 1);
    starts.push(base);
    for idx in memchr::memchr_iter(b'\n', data) {
        starts.push(base + idx as u64 + 1);
    }
    starts
}

/// Split the flat array of absolute line starts into chunks of `TARGET` lines.
/// `data` — the bytes of the range `[data_base, data_base + len)` covering all chunks
/// (for counting characters in each chunk).
fn chunk_starts(starts: &[u64], total: u64, data: &[u8], data_base: u64) -> Vec<Chunk> {
    let chars_of = |from: u64, to: u64| -> u64 {
        let a = (from - data_base) as usize;
        let b = ((to - data_base) as usize).min(data.len());
        if a >= b {
            0
        } else {
            super::count_chars(&data[a..b]) as u64
        }
    };
    let mut chunks = Vec::with_capacity(starts.len() / TARGET + 1);
    let nlines = starts.len();
    let mut i = 0usize;
    while i < nlines {
        let j = (i + TARGET).min(nlines);
        let base = starts[i];
        let rel: Vec<u32> = starts[i..j]
            .iter()
            .map(|&s| u32::try_from(s - base).expect("чанк длиннее 4 ГБ не поддерживается"))
            .collect();
        let next_base = if j < nlines { starts[j] } else { total };
        chunks.push(Chunk {
            rel,
            byte_len: next_base - base,
            chars_len: chars_of(base, next_base),
        });
        i = j;
    }
    if chunks.is_empty() {
        chunks.push(Chunk { rel: vec![0], byte_len: total, chars_len: chars_of(0, total) });
    }
    chunks
}

/// Line-start index with lazy prefix sums over chunks.
pub struct LineIndex {
    chunks: Vec<Chunk>,
    byte_base: Vec<u64>,   // byte_base[i] = bytes before chunk i
    line_base: Vec<usize>, // line_base[i] = lines before chunk i
    char_base: Vec<u64>,   // char_base[i] = characters before chunk i
    total_bytes: u64,
    line_count: usize,
    dirty: bool,
}

impl LineIndex {
    fn from_chunks(chunks: Vec<Chunk>) -> Self {
        Self {
            chunks,
            byte_base: Vec::new(),
            line_base: Vec::new(),
            char_base: Vec::new(),
            total_bytes: 0,
            line_count: 0,
            dirty: true,
        }
    }

    /// Build the index by scanning the entire buffer `data` for `\n`.
    #[allow(dead_code)] // used in tests; production goes through from_starts
    pub fn build(data: &[u8]) -> Self {
        let starts = scan_starts(data, 0);
        Self::from_chunks(chunk_starts(&starts, data.len() as u64, data, 0))
    }

    /// Build the index from a ready-made array of line starts (background loading with progress).
    pub fn from_starts(starts: &[u64], total: u64, data: &[u8]) -> Self {
        Self::from_chunks(chunk_starts(starts, total, data, 0))
    }

    /// Index for an empty document (one empty line).
    pub fn empty() -> Self {
        Self::from_chunks(vec![Chunk { rel: vec![0], byte_len: 0, chars_len: 0 }])
    }

    fn refresh(&mut self) {
        if !self.dirty {
            return;
        }
        self.byte_base.clear();
        self.line_base.clear();
        self.char_base.clear();
        let mut sb = 0u64;
        let mut sl = 0usize;
        let mut sc = 0u64;
        for c in &self.chunks {
            self.byte_base.push(sb);
            self.line_base.push(sl);
            self.char_base.push(sc);
            sb += c.byte_len;
            sl += c.rel.len();
            sc += c.chars_len;
        }
        self.total_bytes = sb;
        self.line_count = sl;
        self.dirty = false;
    }

    /// Base of the chunk containing byte `offset`: (byte start of the chunk, characters before the chunk).
    /// Position in characters = base + count_chars(bytes from the chunk start to offset).
    pub fn char_base_for_byte(&mut self, offset: u64) -> (u64, u64) {
        let ci = self.chunk_for_byte(offset);
        (self.byte_base[ci], self.char_base[ci])
    }

    pub fn line_count(&mut self) -> usize {
        self.refresh();
        self.line_count
    }

    pub fn total_bytes(&mut self) -> u64 {
        self.refresh();
        self.total_bytes
    }

    fn chunk_for_line(&mut self, n: usize) -> usize {
        self.refresh();
        if n == 0 {
            return 0;
        }
        if n >= self.line_count {
            return self.chunks.len() - 1;
        }
        self.line_base.partition_point(|&l| l <= n) - 1
    }

    fn chunk_for_byte(&mut self, offset: u64) -> usize {
        self.refresh();
        if offset == 0 {
            return 0;
        }
        if offset >= self.total_bytes {
            return self.chunks.len() - 1;
        }
        self.byte_base.partition_point(|&b| b <= offset) - 1
    }

    /// Byte offset of the start of line `n` (0-based). For `n >= line_count` — the end of the document.
    pub fn line_start(&mut self, n: usize) -> u64 {
        self.refresh();
        if n == 0 {
            return 0;
        }
        if n >= self.line_count {
            return self.total_bytes;
        }
        let ci = self.chunk_for_line(n);
        let local = n - self.line_base[ci];
        self.byte_base[ci] + self.chunks[ci].rel[local] as u64
    }

    /// Line number (0-based) of the line containing byte `offset`.
    pub fn line_for_offset(&mut self, offset: u64) -> usize {
        self.refresh();
        if offset == 0 {
            return 0;
        }
        if offset >= self.total_bytes {
            return self.line_count.saturating_sub(1);
        }
        let ci = self.chunk_for_byte(offset);
        let local_off = u32::try_from(offset - self.byte_base[ci]).expect("чанк > 4 ГБ");
        let rel = &self.chunks[ci].rel;
        let local = rel.partition_point(|&r| r <= local_off) - 1;
        self.line_base[ci] + local
    }

    /// Update the index after an edit of bytes `[offset, offset+old_len)` → `new_len` bytes.
    ///
    /// `read_fn(o, l)` reads the ALREADY MODIFIED bytes of the document. Only the chunks
    /// overlapped by the edit are rebuilt; the trailing chunks shift lazily thanks to the
    /// changed length of the rebuilt chunks.
    pub fn apply_edit(
        &mut self,
        offset: u64,
        old_len: u64,
        new_len: u64,
        read_fn: impl FnOnce(u64, usize) -> Vec<u8>,
    ) {
        self.refresh();
        let delta = new_len as i64 - old_len as i64;
        let old_total = self.total_bytes;
        let old_end = offset + old_len;

        // chunks overlapped by the edit (in old coordinates). c1 via chunk_for_byte(old_end):
        // if old_end is on a chunk boundary — include it too (a delete up to the boundary erases '\n').
        let c0 = self.chunk_for_byte(offset);
        let c1 = if old_total == 0 { 0 } else { self.chunk_for_byte(old_end.min(old_total)) };

        let b0 = self.byte_base[c0]; // start of the first affected chunk (does not change)
        let is_last = c1 == self.chunks.len() - 1;
        let b1_old =
            if is_last { old_total } else { self.byte_base[c1] + self.chunks[c1].byte_len };
        let b1_new = (b1_old as i64 + delta) as u64;

        let region_len = (b1_new - b0) as usize;
        let region = if region_len > 0 { read_fn(b0, region_len) } else { Vec::new() };

        // new absolute line starts within the rebuilt region
        let mut new_starts = scan_starts(&region, b0);
        if !is_last {
            // drop the spurious "trailing" start exactly on the boundary of the next chunk
            while new_starts.len() > 1 && *new_starts.last().unwrap() >= b1_new {
                new_starts.pop();
            }
        }
        // last chunk: a start exactly at b1_new is a legitimate empty last line.

        let new_chunks = chunk_starts(&new_starts, b1_new, &region, b0);
        self.chunks.splice(c0..=c1, new_chunks);
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference model: rebuild the index from scratch and compare all line starts.
    fn assert_matches(ix: &mut LineIndex, data: &[u8]) {
        let want = scan_starts(data, 0);
        assert_eq!(ix.line_count(), want.len(), "line_count");
        for (n, &w) in want.iter().enumerate() {
            assert_eq!(ix.line_start(n), w, "line_start({n})");
        }
        assert_eq!(ix.total_bytes(), data.len() as u64);
        // line_for_offset on every byte
        for off in 0..=data.len() as u64 {
            let line = want.partition_point(|&s| s <= off).saturating_sub(1);
            let expect = if off >= data.len() as u64 { want.len() - 1 } else { line };
            assert_eq!(ix.line_for_offset(off), expect, "line_for_offset({off})");
        }
    }

    #[test]
    fn build_simple() {
        let data = b"ab\ncd\n\nx";
        let mut ix = LineIndex::build(data);
        assert_eq!(ix.line_count(), 4);
        assert_eq!(ix.line_start(0), 0);
        assert_eq!(ix.line_start(1), 3);
        assert_eq!(ix.line_start(2), 6);
        assert_eq!(ix.line_start(3), 7);
        assert_matches(&mut ix, data);
    }

    #[test]
    fn trailing_newline_makes_empty_line() {
        let data = b"abc\n";
        let mut ix = LineIndex::build(data);
        assert_eq!(ix.line_count(), 2);
        assert_eq!(ix.line_start(1), 4);
    }

    #[test]
    fn empty_doc() {
        let mut ix = LineIndex::empty();
        assert_eq!(ix.line_count(), 1);
        assert_eq!(ix.line_start(0), 0);
        assert_eq!(ix.total_bytes(), 0);
    }

    /// Pseudo-random edits: the model is a Vec<u8>, the index is updated via apply_edit.
    #[test]
    fn random_edits_match_model() {
        let mut model: Vec<u8> = b"one\ntwo\nthree\nfour\nfive".to_vec();
        let mut ix = LineIndex::build(&model);
        // simple deterministic "randomness"
        let mut seed = 0x12345678u64;
        let mut rnd = move |m: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % m.max(1)
        };
        for step in 0..500 {
            let insert = step % 3 != 0 || model.is_empty();
            if insert {
                let pos = rnd(model.len() + 1);
                let what: &[u8] = match rnd(5) {
                    0 => b"\n",
                    1 => b"x",
                    2 => b"ab\ncd",
                    3 => b"\n\n",
                    _ => b"longer text without newline",
                };
                model.splice(pos..pos, what.iter().copied());
                let m = model.clone();
                ix.apply_edit(pos as u64, 0, what.len() as u64, |o, l| {
                    m[o as usize..(o as usize + l).min(m.len())].to_vec()
                });
            } else {
                let pos = rnd(model.len());
                let len = rnd((model.len() - pos).min(7)) + 1;
                model.drain(pos..pos + len);
                let m = model.clone();
                ix.apply_edit(pos as u64, len as u64, 0, |o, l| {
                    m[o as usize..(o as usize + l).min(m.len())].to_vec()
                });
            }
            // do a full check on every 25th step (a full check is expensive)
            if step % 25 == 0 {
                let want = scan_starts(&model, 0);
                assert_eq!(ix.line_count(), want.len(), "step {step}");
                for (n, &w) in want.iter().enumerate() {
                    assert_eq!(ix.line_start(n), w, "step {step}, line {n}");
                }
            }
        }
        assert_matches(&mut ix, &model.clone());
    }

    /// Edits at chunk boundaries: the document is larger than TARGET lines.
    #[test]
    fn edits_across_chunk_boundary() {
        // 3 chunks of TARGET lines
        let mut model: Vec<u8> = Vec::new();
        for i in 0..(TARGET * 2 + 100) {
            model.extend_from_slice(format!("line{i}\n").as_bytes());
        }
        let mut ix = LineIndex::build(&model);
        let total_lines = ix.line_count();

        // delete the '\n' exactly on the boundary of chunk TARGET
        let boundary = ix.line_start(TARGET); // start of line TARGET
        let pos = (boundary - 1) as usize; // the '\n' byte before it
        model.remove(pos);
        let m = model.clone();
        ix.apply_edit(pos as u64, 1, 0, |o, l| {
            m[o as usize..(o as usize + l).min(m.len())].to_vec()
        });
        assert_eq!(ix.line_count(), total_lines - 1);
        let want = scan_starts(&model, 0);
        for (n, &w) in want.iter().enumerate() {
            assert_eq!(ix.line_start(n), w, "after boundary delete, line {n}");
        }

        // insert a block of lines into the middle of the first chunk
        let ins = b"a\nb\nc\n";
        model.splice(10..10, ins.iter().copied());
        let m = model.clone();
        ix.apply_edit(10, 0, ins.len() as u64, |o, l| {
            m[o as usize..(o as usize + l).min(m.len())].to_vec()
        });
        let want = scan_starts(&model, 0);
        assert_eq!(ix.line_count(), want.len());
        for (n, &w) in want.iter().enumerate() {
            assert_eq!(ix.line_start(n), w, "after insert, line {n}");
        }
    }
}
