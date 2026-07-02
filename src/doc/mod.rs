//!
//! Document facade [`Document`] — buffer, line index, undo, snapshots.
//!
//! Position coordinates are `Pos = (line, col)`, 0-based, with `col` in code points.
//! The internal representation is UTF-8 bytes (piece table + line index). The model does not
//! hold the whole text as a single string: reads are line-by-line/range-only, the file is mapped
//! via mmap and is NOT loaded in full.


pub mod encodings;
pub mod line_index;
pub mod piece_table;

pub use encodings::Eol;
use line_index::LineIndex;
use piece_table::{OriginBuf, PieceSnapshot, PieceTable};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub type Pos = (usize, usize);

const MAX_RANGE_BYTES: usize = 256 * 1024 * 1024; // get_text_range / copy limit
const LINE_CACHE_MAX: usize = 10_000;
const MERGE_WINDOW_S: f64 = 1.0; // merge window for consecutive typing
/// Maximum number of transactions in the undo log: the oldest are evicted so the log does not grow
/// without bound over a very long editing session (each transaction holds copies of old/new bytes).
/// Generous depth.
const UNDO_MAX: usize = 4000;
/// Files larger than this are opened in the background (with progress in the status bar).
pub const ASYNC_THRESHOLD: u64 = 4 * 1024 * 1024;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// The application's temporary files directory.
pub fn temp_dir() -> PathBuf {
    let d = std::env::temp_dir().join(crate::brand::EXE_BASE);
    let _ = std::fs::create_dir_all(&d);
    d
}

/// A unique temporary file name.
pub fn temp_file(ext: &str) -> PathBuf {
    let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    temp_dir().join(format!("{pid}-{t}-{n}.{ext}"))
}

/// Description of an edit for listeners (the editor invalidates its cache from `start_line`).
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // fields are the event's informational API
pub struct ChangeEvent {
    pub start_line: usize,
    pub removed_lines: usize,
    pub added_lines: usize,
}

/// A primitive reversible edit.
struct EditItem {
    offset: usize,
    old: Vec<u8>,
    new: Vec<u8>,
}

/// Count code points in UTF-8 (bytes that are not continuation bytes).
/// `\r` and `\n` are ordinary characters: CRLF counts as two.
pub fn count_chars(bytes: &[u8]) -> usize {
    bytes.iter().filter(|b| (**b & 0xC0) != 0x80).count()
}

/// Displayed character count of a line segment, excluding a trailing `\r?\n` — the visual width of a
/// line in the monospace editor (unlike [`count_chars`], which counts the EOL bytes as characters).
fn line_chars(seg: &[u8]) -> usize {
    let mut end = seg.len();
    if end > 0 && seg[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && seg[end - 1] == b'\r' {
        end -= 1;
    }
    count_chars(&seg[..end])
}

/// Above this document size we stop recomputing the exact longest line on every shrinking edit (a full
/// scan is O(lines)); the horizontal extent then only grows, and a stale h-scroll clears on reopen.
/// Normal SQL docs are far below this, so their scroll extent always shrinks correctly.
const MAX_LINE_SCAN_BYTES: u64 = 4_000_000;

/// A document with cheap edits for files up to 1 GB.
pub struct Document {
    pt: PieceTable,
    index: LineIndex,
    origin_path: Option<PathBuf>, // the mmapped file (original or temp)
    owns_origin: bool,            // origin_path is our own temporary file
    pub path: Option<PathBuf>,    // the document's logical path
    /// Save-point tracking — the document is "modified" iff the current history position
    /// `(undo_epoch, undo.len())` differs from where it was last saved/opened. A plain `bool`
    /// can't express this: saving in the MIDDLE of the history and then undoing lands on a
    /// DIFFERENT content than the save-point (e.g. new file → type → save → undo empties the
    /// buffer, which is NOT what's on disk) and must read as modified. `undo_epoch` bumps whenever
    /// a fresh edit discards a non-empty redo branch, so a save-point on an abandoned branch can
    /// never match again. `saved_point == None` means the save-point was evicted from the undo log
    /// (no longer reachable) → the document then always counts as modified.
    undo_epoch: u64,
    saved_point: Option<(u64, usize)>,
    pub encoding_label: String,
    pub eol: Eol,
    char_count: usize,
    /// Longest line in DISPLAYED characters (excl. EOL) — the horizontal scroll extent ≈ this × the
    /// monospace advance. Chars, NOT bytes: bytes over-count multibyte UTF-8 (Cyrillic ×2) and tripped
    /// the h-scroll at half the real width. Kept EXACT for docs up to `MAX_LINE_SCAN_BYTES` (recomputed
    /// on any shrinking edit); above that it only grows, and a stale h-scroll clears on reopen.
    max_line_chars: usize,

    undo: Vec<Vec<EditItem>>,
    redo: Vec<Vec<EditItem>>,
    open_txn: Option<Vec<EditItem>>,
    compound_depth: usize,
    last_edit_time: Option<Instant>,
    last_edit_was_typing: bool,

    line_cache: HashMap<usize, (String, usize)>, // line n → (text without EOL, char count)
    temp_files: Vec<PathBuf>,
    /// Accumulator for the "first changed line" since the last read (for cache invalidation).
    change_start: Option<usize>,
}

/// Messages for background file loading.
pub enum LoadMsg {
    Progress(u8),
    Done(Box<Document>),
    Failed(String),
}

impl Document {
    /// An empty document.
    pub fn new_empty() -> Self {
        Self {
            pt: PieceTable::empty(),
            index: LineIndex::empty(),
            origin_path: None,
            owns_origin: false,
            path: None,
            undo_epoch: 0,
            saved_point: Some((0, 0)), // an empty new document = "as opened", not modified
            encoding_label: "UTF-8".to_owned(),
            eol: Eol::Crlf, // default for a new file on Windows
            char_count: 0,
            max_line_chars: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            open_txn: None,
            compound_depth: 0,
            last_edit_time: None,
            last_edit_was_typing: false,
            line_cache: HashMap::new(),
            temp_files: Vec::new(),
            change_start: None,
        }
    }

    /// Take and reset the "first changed line" accumulator.
    pub fn take_change_start(&mut self) -> Option<usize> {
        self.change_start.take()
    }

    // ====================================================================
    //  Opening / loading
    // ====================================================================

    /// Synchronous open (small files). `progress` is a 0..100 callback.
    pub fn open_sync(
        path: &Path,
        mut progress: Option<&mut dyn FnMut(u8)>,
    ) -> std::io::Result<Self> {
        let mut doc = Self::new_empty();
        doc.path = Some(path.to_owned());
        let det = encodings::detect(path)?;
        doc.encoding_label = det.label.clone();
        let (origin_path, owns) = if det.is_utf8_no_bom {
            (path.to_owned(), false)
        } else {
            let dst = temp_file("utf8");
            let enc = det.encoding.unwrap_or(encoding_rs::UTF_8);
            let mut cb = progress.as_deref_mut().map(|cb| {
                move |p: u8| cb((p as f32 * 0.4) as u8)
            });
            encodings::transcode_to_utf8(
                path,
                &dst,
                enc,
                cb.as_mut().map(|c| c as &mut dyn FnMut(u8)),
            )?;
            doc.temp_files.push(dst.clone());
            (dst, true)
        };
        doc.attach_origin(&origin_path, owns)?;
        // EOL inferred from a sample of the first ~1 MB of UTF-8 content
        let sample = doc.pt.read(0, 1024 * 1024);
        doc.eol = encodings::detect_eol(&sample);
        // line index + char count in a single pass over the mmap
        doc.build_index_and_chars(progress);
        Ok(doc)
    }

    /// Background open: the worker builds the whole document and sends it over the channel.
    pub fn spawn_open(path: PathBuf) -> std::sync::mpsc::Receiver<LoadMsg> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let mut cb = move |p: u8| {
                let _ = tx2.send(LoadMsg::Progress(p));
            };
            match Document::open_sync(&path, Some(&mut cb)) {
                Ok(doc) => {
                    let _ = tx.send(LoadMsg::Done(Box::new(doc)));
                }
                Err(e) => {
                    let _ = tx.send(LoadMsg::Failed(e.to_string()));
                }
            }
        });
        rx
    }

    /// Open the origin file read-only AND lock it against writes/deletion by other processes
    /// (`FILE_SHARE_READ`). The handle is kept alive in [`OriginBuf::Mmap`] for the entire tab
    /// lifetime; others can only read. If the file is currently open for writing by someone, the
    /// open fails with a sharing violation (which is safer than mmapping a changing file).
    fn open_origin_locked(path: &Path) -> std::io::Result<std::fs::File> {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(path)
    }

    fn attach_origin(&mut self, origin_path: &Path, owns: bool) -> std::io::Result<()> {
        self.detach_origin();
        let size = std::fs::metadata(origin_path)?.len();
        if size > 0 {
            let fh = Self::open_origin_locked(origin_path)?;
            // SAFETY: the file is opened read-only (open_origin_locked holds a shared lock),
            // so the mmap mapping is consistent. Mmap creates no mutable references to backing memory;
            // Rust cannot observe external mutations because the file is locked against writes. The
            // `self.pt` mutex guards content access against races between the UI and background threads.
            let mm = unsafe { memmap2::Mmap::map(&fh)? };
            self.pt = PieceTable::new(Arc::new(OriginBuf::Mmap(mm, fh)));
        } else {
            self.pt = PieceTable::empty();
        }
        self.origin_path = Some(origin_path.to_owned());
        self.owns_origin = owns;
        Ok(())
    }

    fn detach_origin(&mut self) {
        // reset the piece table to drop the reference to the mmap before it is closed
        self.pt = PieceTable::empty();
    }

    /// A single pass over the origin: line starts (progress 40..100) + char count.
    fn build_index_and_chars(&mut self, mut progress: Option<&mut dyn FnMut(u8)>) {
        let origin = self.pt.origin_arc();
        let data = origin.as_slice();
        let total = data.len().max(1);
        const BLOCK: usize = 8 * 1024 * 1024;
        let mut starts: Vec<u64> = Vec::with_capacity(data.len() / 64 + 1);
        starts.push(0);
        let mut chars = 0usize;
        let mut pos = 0usize;
        while pos < data.len() {
            let end = (pos + BLOCK).min(data.len());
            let block = &data[pos..end];
            for idx in memchr::memchr_iter(b'\n', block) {
                starts.push((pos + idx + 1) as u64);
            }
            chars += count_chars(block);
            pos = end;
            if let Some(cb) = progress.as_deref_mut() {
                cb(40 + ((pos * 60) / total).min(60) as u8);
            }
        }
        self.index = LineIndex::from_starts(&starts, data.len() as u64);
        self.char_count = chars;
        // longest line in DISPLAYED characters (excl. EOL) — the horizontal scroll extent
        let mut maxc = 0usize;
        for w in starts.windows(2) {
            maxc = maxc.max(line_chars(&data[w[0] as usize..w[1] as usize]));
        }
        maxc = maxc.max(line_chars(&data[*starts.last().unwrap_or(&0) as usize..]));
        self.max_line_chars = maxc;
    }

    /// Longest line in CHARACTERS (excl. EOL) — the horizontal scroll extent (chars, not bytes).
    pub fn max_line_chars(&self) -> usize {
        self.max_line_chars
    }

    /// Exact longest line in DISPLAYED characters — a pruned full scan: it char-counts only lines whose
    /// byte span exceeds the running max (chars ≤ bytes, so a shorter span can't win), so it reads just
    /// the long lines. O(lines); callers gate it to small docs (`MAX_LINE_SCAN_BYTES`) so a huge file
    /// never scans on the edit hot path.
    fn recompute_max_line_chars(&mut self) -> usize {
        let n = self.index.line_count();
        let mut max = 0usize;
        let mut s = self.index.line_start(0);
        for line in 0..n {
            let e = self.index.line_start(line + 1);
            let span = (e - s) as usize;
            if span > max {
                max = max.max(line_chars(&self.pt.read(s as usize, span)));
            }
            s = e;
        }
        max
    }

    // ====================================================================
    //  Properties
    // ====================================================================

    pub fn modified(&self) -> bool {
        // Modified unless the current history position is exactly the saved/opened one. `None`
        // save-point (evicted from the log) never matches → always modified.
        self.saved_point != Some((self.undo_epoch, self.undo.len()))
    }

    pub fn line_count(&mut self) -> usize {
        self.index.line_count()
    }

    pub fn char_count(&self) -> usize {
        self.char_count
    }

    // ====================================================================
    //  Reading
    // ====================================================================

    fn strip_eol(raw: &[u8]) -> &[u8] {
        if raw.ends_with(b"\r\n") {
            &raw[..raw.len() - 2]
        } else if raw.ends_with(b"\n") || raw.ends_with(b"\r") {
            &raw[..raw.len() - 1]
        } else {
            raw
        }
    }

    /// Read line `n` into the cache (if not already there) and return a REFERENCE — no cloning. The
    /// hot path for measurement/scanning: on gigabyte-sized lines, cloning the line in `get_line`
    /// (even from the cache) was the main source of lag (a double click pulled the line several
    /// times per frame).
    fn ensure_line_cached(&mut self, n: usize) -> &str {
        if !self.line_cache.contains_key(&n) {
            let start = self.index.line_start(n);
            let end = self.index.line_start(n + 1);
            let raw = self.pt.read(start as usize, (end - start) as usize);
            let text = String::from_utf8_lossy(Self::strip_eol(&raw)).into_owned();
            let chars = text.chars().count(); // compute the length once and cache it alongside
            if self.line_cache.len() >= LINE_CACHE_MAX {
                self.line_cache.clear(); // simple strategy: overflowed — cleared
            }
            self.line_cache.insert(n, (text, chars));
        }
        self.line_cache.get(&n).map(|(s, _)| s.as_str()).unwrap_or("")
    }

    /// The text of line `n` (0-based) without the line break.
    pub fn get_line(&mut self, n: usize) -> String {
        if n >= self.line_count() {
            return String::new();
        }
        self.ensure_line_cached(n).to_owned()
    }

    /// A reference to line `n` without cloning (valid until the next document access). The hot path
    /// for rendering/highlighting: cloning the line for every visible line every frame was wasteful.
    pub(crate) fn line_ref(&mut self, n: usize) -> &str {
        if n >= self.line_count() {
            return "";
        }
        self.ensure_line_cached(n)
    }

    /// The length of line `n` in code points (without EOL). O(1) after the first access — the char
    /// count is cached alongside the line (important for touch drag: `set_from_line_x` calls this
    /// every frame).
    pub fn line_length(&mut self, n: usize) -> usize {
        if n >= self.line_count() {
            return 0;
        }
        self.ensure_line_cached(n);
        self.line_cache.get(&n).map_or(0, |(_, c)| *c)
    }

    /// Word boundaries (in code points) around column `col` in line `n`, or `None` if there is no
    /// word under the click. The scan is bounded by a window around the click, so a double click in
    /// a very long line does NOT materialize or scan the whole line (lag fix on ~1 GB files).
    pub fn word_bounds_at(
        &mut self,
        n: usize,
        col: usize,
        is_word: impl Fn(char) -> bool,
    ) -> Option<(usize, usize)> {
        if n >= self.line_count() {
            return None;
        }
        const W: usize = 1024; // word-scan window around the click (chars to each side)
        let lo = col.saturating_sub(W);
        let hi = col.saturating_add(W);
        let s = self.ensure_line_cached(n);
        // char window [lo..=hi]; win[j] corresponds to column `lo + j`
        let win: Vec<char> = s
            .chars()
            .enumerate()
            .skip_while(|(i, _)| *i < lo)
            .take_while(|(i, _)| *i <= hi)
            .map(|(_, ch)| ch)
            .collect();
        let c = col - lo; // index of the clicked char within the window
        // word under the click: the char at `c` or (as editors do) the one to its left
        let start_local = if c < win.len() && is_word(win[c]) {
            c
        } else if c > 0 && c <= win.len() && is_word(win[c - 1]) {
            c - 1
        } else {
            return None;
        };
        let (mut s0, mut e0) = (start_local, start_local);
        while s0 > 0 && is_word(win[s0 - 1]) {
            s0 -= 1;
        }
        while e0 < win.len() && is_word(win[e0]) {
            e0 += 1;
        }
        Some((lo + s0, lo + e0))
    }

    /// The text between two positions (for copying). A range > 256 MB is rejected.
    pub fn get_text_range(&mut self, start: Pos, end: Pos) -> Result<String, String> {
        let a = self.pos_to_byte(start);
        let b = self.pos_to_byte(end);
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        if (b - a) as usize > MAX_RANGE_BYTES {
            return Err("Range too large to copy (> 256 MB)".to_owned());
        }
        let raw = self.pt.read(a as usize, (b - a) as usize);
        Ok(String::from_utf8_lossy(&raw).into_owned())
    }

    /// A content snapshot for background processes (search).
    pub fn snapshot(&self) -> PieceSnapshot {
        self.pt.snapshot()
    }

    // ====================================================================
    //  Coordinate conversion
    // ====================================================================

    pub fn pos_to_byte(&mut self, pos: Pos) -> u64 {
        let (line, col) = pos;
        let line = line.min(self.line_count().saturating_sub(1));
        let start = self.index.line_start(line);
        if col == 0 {
            return start;
        }
        let text = self.ensure_line_cached(line); // &str from the cache — no string clone
        let byte_col: usize = text
            .char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(text.len());
        start + byte_col as u64
    }

    /// Convert a byte offset to `(line, col)`.
    pub fn byte_to_pos(&mut self, offset: u64) -> Pos {
        let offset = offset.min(self.index.total_bytes());
        let line = self.index.line_for_offset(offset);
        let start = self.index.line_start(line);
        let raw = self.pt.read(start as usize, (offset - start) as usize);
        let stripped = Self::strip_eol(&raw);
        (line, count_chars(stripped))
    }

    // ====================================================================
    //  Edits
    // ====================================================================

    /// Begin a compound operation (grouped into a single undo step).
    #[allow(dead_code)] // public model API (used in tests)
    pub fn begin_compound(&mut self) {
        if self.compound_depth == 0 {
            self.open_txn = Some(Vec::new());
        }
        self.compound_depth += 1;
    }

    #[allow(dead_code)]
    pub fn end_compound(&mut self) {
        if self.compound_depth == 0 {
            return;
        }
        self.compound_depth -= 1;
        if self.compound_depth == 0 {
            if let Some(txn) = self.open_txn.take() {
                if !txn.is_empty() {
                    self.diverge();
                    self.push_undo(txn);
                }
            }
        }
    }

    /// Replace the text in the range `[start, end)` with `text`. Return the change event.
    pub fn replace_range(&mut self, start: Pos, end: Pos, text: &str) -> ChangeEvent {
        let a = self.pos_to_byte(start);
        let b = self.pos_to_byte(end);
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let new = text.as_bytes().to_vec();
        let (ev, old) = self.apply_primitive(a as usize, (b - a) as usize, &new);
        self.record(EditItem { offset: a as usize, old, new });
        ev
    }

    /// Apply an edit to the buffer and the index; return (event, old bytes).
    fn apply_primitive(
        &mut self,
        offset: usize,
        old_len: usize,
        new_bytes: &[u8],
    ) -> (ChangeEvent, Vec<u8>) {
        let first_line = self.index.line_for_offset(offset as u64);
        let old = if old_len > 0 { self.pt.delete(offset, old_len) } else { Vec::new() };
        let removed_lines = if old_len > 0 {
            // lines in old coordinates — counted from the removed bytes
            memchr::memchr_iter(b'\n', &old).count()
        } else {
            0
        };
        if !new_bytes.is_empty() {
            self.pt.insert(offset, new_bytes);
        }
        let pt = &mut self.pt;
        self.index.apply_edit(offset as u64, old_len as u64, new_bytes.len() as u64, |o, l| {
            pt.read(o as usize, l)
        });
        let added_lines = memchr::memchr_iter(b'\n', new_bytes).count();
        self.char_count = self.char_count + count_chars(new_bytes) - count_chars(&old);
        // Longest line in DISPLAYED characters (excl. EOL): grow from the insert's interior + boundary
        // lines (cheap, no full scan). A DELETION can only SHRINK it, so on removal we recompute exactly —
        // but gated to small docs so a huge file never pays a full scan on the edit hot path.
        let mut maxc = self.max_line_chars;
        let mut prevc = 0usize;
        for idx in memchr::memchr_iter(b'\n', new_bytes) {
            maxc = maxc.max(line_chars(&new_bytes[prevc..idx]));
            prevc = idx + 1;
        }
        maxc = maxc.max(line_chars(&new_bytes[prevc..]));
        for line in [first_line, first_line + added_lines] {
            let (s, e) = (self.index.line_start(line), self.index.line_start(line + 1));
            maxc = maxc.max(line_chars(&pt.read(s as usize, (e - s) as usize)));
        }
        self.max_line_chars = maxc;
        if old_len > 0 && self.index.total_bytes() <= MAX_LINE_SCAN_BYTES {
            self.max_line_chars = self.recompute_max_line_chars();
        }
        self.invalidate_cache_from(first_line);
        self.change_start =
            Some(self.change_start.map_or(first_line, |c| c.min(first_line)));
        (ChangeEvent { start_line: first_line, removed_lines, added_lines }, old)
    }

    /// Push a transaction onto the undo log, evicting the oldest when `UNDO_MAX` overflows.
    fn push_undo(&mut self, txn: Vec<EditItem>) {
        self.undo.push(txn);
        if self.undo.len() > UNDO_MAX {
            let excess = self.undo.len() - UNDO_MAX;
            self.undo.drain(0..excess);
            // The save-point is an index into the undo log; eviction shifts it down. If it was
            // among the evicted transactions it can no longer be reached → drop it (the document
            // then stays modified until the next save).
            if let Some((ep, n)) = self.saved_point {
                if ep == self.undo_epoch {
                    self.saved_point = if n >= excess { Some((ep, n - excess)) } else { None };
                }
            }
        }
    }

    /// A fresh edit branches off the current history position: the redo stack (the "future") is
    /// discarded. If it was non-empty we were sitting below the top of the log, so this opens a NEW
    /// branch — bump the epoch so a save-point left on the abandoned branch can never compare equal
    /// again (its content is gone). Call right before pushing the new transaction.
    fn diverge(&mut self) {
        if !self.redo.is_empty() {
            self.undo_epoch += 1;
            self.redo.clear();
        }
    }

    /// Put an edit into the undo log (with auto-merging of typing).
    fn record(&mut self, item: EditItem) {
        if let Some(txn) = self.open_txn.as_mut() {
            txn.push(item);
            return;
        }
        let now = Instant::now();
        let merged = self.try_merge_typing(&item, now);
        if !merged {
            // "typing" — insertion of a short fragment without a line break
            self.last_edit_was_typing =
                item.old.is_empty() && !item.new.contains(&b'\n') && item.new.len() <= 4;
            self.diverge(); // discard the redo branch (bumps the epoch if we were diverging)
            self.push_undo(vec![item]);
        }
        // (a merge keeps the same transaction; the redo stack is already empty there — a merge can't
        // follow an undo/redo, which clears `last_edit_was_typing`.)
        self.last_edit_time = Some(now);
    }

    fn try_merge_typing(&mut self, item: &EditItem, now: Instant) -> bool {
        if !self.last_edit_was_typing || self.undo.is_empty() {
            return false;
        }
        let Some(t0) = self.last_edit_time else { return false };
        if now.duration_since(t0).as_secs_f64() > MERGE_WINDOW_S {
            return false;
        }
        if !item.old.is_empty() || item.new.contains(&b'\n') || item.new.len() > 4 {
            return false;
        }
        let last_txn = self.undo.last_mut().unwrap();
        if last_txn.len() != 1 {
            return false;
        }
        let prev = &mut last_txn[0];
        if !prev.old.is_empty() || prev.offset + prev.new.len() != item.offset {
            return false;
        }
        prev.new.extend_from_slice(&item.new);
        true
    }

    fn invalidate_cache_from(&mut self, line: usize) {
        self.line_cache.retain(|&k, _| k < line);
    }

    // ====================================================================
    //  Undo / Redo
    // ====================================================================

    /// Revert the last operation. Return the cursor position after the revert.
    pub fn undo(&mut self) -> Option<Pos> {
        let txn = self.undo.pop()?;
        let mut cursor = None;
        let mut new_txn = Vec::with_capacity(txn.len());
        for item in txn.into_iter().rev() {
            let (item, pos) = self.revert(item);
            cursor = Some(pos);
            new_txn.push(item);
        }
        new_txn.reverse();
        self.redo.push(new_txn);
        // `modified` is now derived from the history position vs the save-point (see `modified()`),
        // so undo/redo need not touch it: an empty undo log no longer implies "clean" — the
        // save-point may sit mid-history (e.g. typed → saved → undo lands on the empty buffer,
        // which differs from the saved content and MUST show as modified).
        self.last_edit_was_typing = false;
        cursor
    }

    /// Reapply an undone operation.
    pub fn redo(&mut self) -> Option<Pos> {
        let txn = self.redo.pop()?;
        let mut cursor = None;
        let mut new_txn = Vec::with_capacity(txn.len());
        for item in txn {
            let (item, pos) = self.reapply(item);
            cursor = Some(pos);
            new_txn.push(item);
        }
        self.push_undo(new_txn);
        // redo re-applies onto the same branch (epoch unchanged), so redoing back up to the
        // save-point correctly reads as clean again — no manual flag needed (see `modified()`).
        self.last_edit_was_typing = false;
        cursor
    }

    fn revert(&mut self, item: EditItem) -> (EditItem, Pos) {
        // remove new, restore old
        let (_, _) = self.apply_primitive(item.offset, item.new.len(), &item.old);
        let pos = self.byte_to_pos((item.offset + item.old.len()) as u64);
        (item, pos)
    }

    fn reapply(&mut self, item: EditItem) -> (EditItem, Pos) {
        let (_, _) = self.apply_primitive(item.offset, item.old.len(), &item.new);
        let pos = self.byte_to_pos((item.offset + item.new.len()) as u64);
        (item, pos)
    }

    // ====================================================================
    //  Snapshots / saving
    // ====================================================================

    /// Save the document as UTF-8 without a BOM. With no `path`, save to the current path.
    pub fn save(&mut self, path: Option<&Path>) -> std::io::Result<()> {
        let target: PathBuf = match path.or(self.path.as_deref()) {
            Some(p) => p.to_owned(),
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Save path not set",
                ))
            }
        };
        let same_as_origin = self
            .origin_path
            .as_ref()
            .is_some_and(|op|

                std::path::absolute(&target).ok() == std::path::absolute(op).ok());
        // The temp file is a sibling of the target (NOT under temp_dir()), so it is not reaped by
        // cleanup_temp_dir / Drop — every error path below must remove it itself, or a failed save
        // would orphan a `*.tmp-N` next to the user's file.
        let tmp = target.with_extension(format!(
            "tmp-{}",
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        if let Err(e) = (|| -> std::io::Result<()> {
            let f = std::fs::File::create(&tmp)?;
            let mut w = std::io::BufWriter::new(f);
            self.pt.write_to(&mut w)?;
            use std::io::Write;
            w.flush()?;
            w.get_ref().sync_all()?;
            Ok(())
        })() {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        if same_as_origin {
            // Release the mmap (Windows won't rename over a file we hold open), swap the temp in,
            // then re-map; the content is identical, so the line index and undo log stay valid.
            // On ANY failure we re-map whatever is on disk at `target` so `self.pt` is never left
            // empty — the on-disk file is intact (the original on a failed rename, the new identical
            // content on a failed reattach). Without this rollback a failed save emptied the editor,
            // and an unconditional second Ctrl+S would then write the empty buffer over the file.
            self.detach_origin();
            let swap = std::fs::rename(&tmp, &target)
                .and_then(|()| self.reattach_same_content(&target));
            if let Err(e) = swap {
                let _ = self.reattach_same_content(&target);
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        } else if let Err(e) = std::fs::rename(&tmp, &target) {
            // the origin stays the same (same content), the document path changes
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        self.path = Some(target);
        // after saving, the source encoding is irrelevant — on disk it is UTF-8
        self.encoding_label = "UTF-8".to_owned();
        // Mark the current history position as the clean one.
        self.saved_point = Some((self.undo_epoch, self.undo.len()));
        // Freeze the current typing run: the next keystroke must start a NEW transaction rather
        // than merging into the just-saved one — otherwise a later undo would skip past the
        // save-point in one step (and the merged-in text would never read as modified).
        self.last_edit_was_typing = false;
        Ok(())
    }

    /// Reopen the origin onto a file with the same content (after saving).
    fn reattach_same_content(&mut self, path: &Path) -> std::io::Result<()> {
        let size = std::fs::metadata(path)?.len();
        if size > 0 {
            let fh = Self::open_origin_locked(path)?;
            // SAFETY: same as attach_origin — open_origin_locked opens the file read-only with
            // share_mode=FILE_SHARE_READ (external writes are forbidden while the handle is alive),
            // so the mmap backing memory is stable for the entire lifetime of the buffer.
            let mm = unsafe { memmap2::Mmap::map(&fh)? };
            self.pt = PieceTable::new(Arc::new(OriginBuf::Mmap(mm, fh)));
        } else {
            self.pt = PieceTable::empty();
        }
        self.origin_path = Some(path.to_owned());
        self.owns_origin = false;
        self.line_cache.clear();
        // do not rebuild the line index: the content has not changed
        Ok(())
    }

    /// The full text (tests / small files only).
    #[cfg(test)]
    pub fn full_text(&mut self) -> String {
        let n = self.pt.len();
        String::from_utf8_lossy(&self.pt.read(0, n)).into_owned()
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.detach_origin();
        for p in &self.temp_files {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Delete old temporary files from previous runs (best effort).
pub fn cleanup_temp_dir(max_age_s: u64) {
    let d = temp_dir();
    let now = std::time::SystemTime::now();
    let Ok(entries) = std::fs::read_dir(&d) else { return };
    for e in entries.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        if now.duration_since(modified).map(|d| d.as_secs()).unwrap_or(0) > max_age_s {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_from(text: &str) -> Document {
        let mut d = Document::new_empty();
        d.replace_range((0, 0), (0, 0), text);
        // reset the history, simulating a "freshly opened" document at its clean save-point
        d.undo.clear();
        d.redo.clear();
        d.undo_epoch = 0;
        d.saved_point = Some((0, 0));
        d.last_edit_was_typing = false;
        d
    }

    #[test]
    fn empty_doc_basics() {
        let mut d = Document::new_empty();
        assert_eq!(d.line_count(), 1);
        assert_eq!(d.get_line(0), "");
        assert_eq!(d.char_count(), 0);
    }

    #[test]
    fn insert_and_read_lines() {
        let mut d = doc_from("ab\nсд\nef");
        assert_eq!(d.line_count(), 3);
        assert_eq!(d.get_line(0), "ab");
        assert_eq!(d.get_line(1), "сд");
        assert_eq!(d.get_line(2), "ef");
        assert_eq!(d.char_count(), 8);
    }

    #[test]
    fn replace_range_multibyte() {
        let mut d = doc_from("привет мир");
        d.replace_range((0, 7), (0, 10), "world");
        assert_eq!(d.full_text(), "привет world");
        assert_eq!(d.char_count(), 12);
    }

    #[test]
    fn crlf_lines_strip_eol() {
        let mut d = doc_from("ab\r\ncd\r\n");
        assert_eq!(d.line_count(), 3);
        assert_eq!(d.get_line(0), "ab");
        assert_eq!(d.get_line(1), "cd");
        assert_eq!(d.get_line(2), "");
    }

    #[test]
    fn crlf_counts_as_two_chars() {
        // \r and \n are ordinary code points: CRLF = two characters
        let mut d = Document::new_empty(); // EOL defaults to CRLF
        d.replace_range((0, 0), (0, 0), "\r\n");
        assert_eq!(d.char_count(), 2);
        let d = doc_from("a\r\nb");
        assert_eq!(d.char_count(), 4);
    }

    #[test]
    fn pos_byte_round_trip() {
        let mut d = doc_from("аб\nвгд\ne");
        for (line, col) in [(0, 0), (0, 1), (0, 2), (1, 0), (1, 3), (2, 1)] {
            let b = d.pos_to_byte((line, col));
            assert_eq!(d.byte_to_pos(b), (line, col), "({line},{col})");
        }
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut d = doc_from("hello");
        d.replace_range((0, 5), (0, 5), " world");
        assert_eq!(d.full_text(), "hello world");
        assert!(d.modified());
        let pos = d.undo();
        assert_eq!(d.full_text(), "hello");
        assert_eq!(pos, Some((0, 5)));
        let pos = d.redo();
        assert_eq!(d.full_text(), "hello world");
        assert_eq!(pos, Some((0, 11)));
    }

    #[test]
    fn typing_merges_into_one_undo() {
        let mut d = doc_from("");
        d.replace_range((0, 0), (0, 0), "a");
        d.replace_range((0, 1), (0, 1), "b");
        d.replace_range((0, 2), (0, 2), "c");
        assert_eq!(d.full_text(), "abc");
        d.undo();
        assert_eq!(d.full_text(), "");
    }

    #[test]
    fn max_line_chars_counts_display_chars() {
        // Cyrillic is 2 bytes/char but 1 column wide — the h-scroll extent must be in CHARS, not bytes.
        assert_eq!(doc_from("привет").max_line_chars(), 6); // 6 chars, 12 bytes
        // the longest line wins and its EOL (\r\n) is excluded
        assert_eq!(doc_from("ab\r\nпривет\r\nx").max_line_chars(), 6);
        // grows as a longer line is typed
        let mut d = doc_from("a");
        assert_eq!(d.max_line_chars(), 1);
        d.replace_range((0, 1), (0, 1), "0123456789"); // line 0 → "a0123456789" = 11 chars
        assert_eq!(d.max_line_chars(), 11);
        // …and SHRINKS again when the long content is deleted (small doc → recomputed exactly, no phantom
        // scroll). Regression for "type a lot, select-all, delete → h-scroll stays".
        d.replace_range((0, 0), (0, 11), ""); // delete all of line 0 → one empty line
        assert_eq!(d.max_line_chars(), 0);
    }

    #[test]
    fn compound_groups_edits() {
        let mut d = doc_from("abc");
        d.begin_compound();
        d.replace_range((0, 0), (0, 1), "X");
        d.replace_range((0, 2), (0, 3), "Y");
        d.end_compound();
        assert_eq!(d.full_text(), "XbY");
        d.undo();
        assert_eq!(d.full_text(), "abc");
        d.redo();
        assert_eq!(d.full_text(), "XbY");
    }

    #[test]
    fn char_count_tracks_edits() {
        let mut d = doc_from("привет");
        assert_eq!(d.char_count(), 6);
        d.replace_range((0, 0), (0, 2), "x");
        assert_eq!(d.full_text(), "xивет");
        assert_eq!(d.char_count(), 5);
        d.undo();
        assert_eq!(d.char_count(), 6);
    }

    #[test]
    fn open_save_round_trip() {
        let dir = temp_dir();
        let p = dir.join("test-open-save.xml");
        std::fs::write(&p, "<a>\nпривет\n</a>").unwrap();
        let mut d = Document::open_sync(&p, None).unwrap();
        assert_eq!(d.line_count(), 3);
        assert_eq!(d.get_line(1), "привет");
        assert_eq!(d.encoding_label, "UTF-8");
        d.replace_range((1, 0), (1, 6), "hello");
        d.save(None).unwrap();
        drop(d);
        let out = std::fs::read_to_string(&p).unwrap();
        assert_eq!(out, "<a>\nhello\n</a>");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn open_cp1251_transcodes() {
        let dir = temp_dir();
        let p = dir.join("test-cp1251.xml");
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("<а>текст</а>");
        std::fs::write(&p, &bytes).unwrap();
        let mut d = Document::open_sync(&p, None).unwrap();
        assert_eq!(d.encoding_label, "Windows-1251");
        assert_eq!(d.get_line(0), "<а>текст</а>");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn save_over_origin_keeps_working() {
        let dir = temp_dir();
        let p = dir.join("test-save-origin.xml");
        std::fs::write(&p, "line1\nline2").unwrap();
        let mut d = Document::open_sync(&p, None).unwrap();
        d.replace_range((0, 0), (0, 5), "first");
        d.save(None).unwrap(); // saving over the mmapped file
        assert_eq!(d.get_line(0), "first");
        // the document stays editable after the mmap is reopened
        d.replace_range((1, 0), (1, 5), "second");
        assert_eq!(d.get_line(1), "second");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn failed_save_keeps_content_and_leaves_no_temp() {
        // Saving into a missing directory fails (temp create / rename); the in-memory buffer must
        // survive (not be emptied) and no stray `*.tmp-N` may be left behind — regression for the
        // save error-path cleanup + rollback.
        let mut d = doc_from("keep me\nsafe");
        let parent = temp_dir().join("jq_no_such_dir_for_save_test");
        let _ = std::fs::remove_dir_all(&parent);
        let target = parent.join("out.txt");
        assert!(d.save(Some(&target)).is_err(), "save into a missing dir must fail");
        // content preserved (the buffer was not detached/emptied)
        assert_eq!(d.full_text(), "keep me\nsafe");
        assert_eq!(d.line_count(), 2);
        // no orphan temp if the parent happens to exist (it shouldn't here, but be defensive)
        if let Ok(rd) = std::fs::read_dir(&parent) {
            assert!(
                rd.flatten().all(|e| !e.file_name().to_string_lossy().contains("tmp-")),
                "a failed save left an orphan temp file"
            );
        }
    }

    #[test]
    fn successful_save_leaves_no_temp_sibling() {
        // The happy path must not leave a `*.tmp-N` next to the saved file.
        let dir = temp_dir();
        let p = dir.join("test-save-no-temp.txt");
        let mut d = doc_from("alpha\nbeta");
        d.save(Some(&p)).unwrap();
        let stem = "test-save-no-temp";
        let orphan = std::fs::read_dir(&dir).unwrap().flatten().any(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with(stem) && n.contains("tmp-")
        });
        assert!(!orphan, "a successful save left an orphan temp file");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn save_then_undo_marks_modified() {
        // Regression: new file → type → Save → Ctrl+Z. The undo empties the buffer, which differs
        // from what was just written to disk, so the document MUST read as modified (unsaved-star).
        let dir = temp_dir();
        let p = dir.join("test-savepoint-undo.txt");
        let mut d = Document::new_empty();
        assert!(!d.modified(), "a fresh empty document is clean");
        d.replace_range((0, 0), (0, 0), "abc");
        assert!(d.modified());
        d.save(Some(&p)).unwrap();
        assert!(!d.modified(), "clean immediately after saving");
        d.undo();
        assert_eq!(d.full_text(), "");
        assert!(d.modified(), "undo past the save-point: buffer differs from disk → modified");
        d.redo();
        assert_eq!(d.full_text(), "abc");
        assert!(!d.modified(), "redo back onto the save-point is clean again");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn save_then_undo_then_new_edit_diverges() {
        // Two transactions, saved at the second; undo to the first, then a fresh edit. The new
        // branch sits at the SAME undo depth as the save-point but holds different content — the
        // epoch bump must keep it reading as modified.
        let dir = temp_dir();
        let p = dir.join("test-savepoint-diverge.txt");
        let mut d = Document::new_empty();
        d.replace_range((0, 0), (0, 0), "alpha\n"); // own txn (has '\n' → no typing-merge)
        d.replace_range((1, 0), (1, 0), "beta\n"); // second txn
        d.save(Some(&p)).unwrap();
        assert!(!d.modified());
        d.undo();
        assert!(d.modified(), "below the save-point");
        d.replace_range((1, 0), (1, 0), "gamma\n"); // diverges: discards the redo branch
        assert!(d.modified(), "a different branch at the same depth still counts as modified");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn save_freezes_typing_run() {
        // Saving mid-typing must end the merge run: the next keystroke starts a NEW transaction, so
        // undo lands exactly on the saved content (not before it).
        let dir = temp_dir();
        let p = dir.join("test-savepoint-typing.txt");
        let mut d = Document::new_empty();
        d.replace_range((0, 0), (0, 0), "ab"); // a typing-eligible fragment
        d.save(Some(&p)).unwrap();
        assert!(!d.modified());
        d.replace_range((0, 2), (0, 2), "c"); // must NOT merge into the saved transaction
        assert!(d.modified());
        d.undo();
        assert_eq!(d.full_text(), "ab", "undo stops at the saved content");
        assert!(!d.modified(), "back on the save-point → clean");
        let _ = std::fs::remove_file(p);
    }
}
