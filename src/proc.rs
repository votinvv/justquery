//! Background-process framework for a tab: the document search.
//!
//! At most one process can run on a tab; tabs are independent. The worker lives
//! in its own thread, sends messages over a channel (polled in the update loop), and stops
//! on an `AtomicBool`. Results accumulate in memory; once the 100 MB limit is exceeded the process
//! stops, what has been accumulated is shown, and an error is reported to the status bar.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Memory limit for a process's accumulated results (results only, not the file's data).
pub const RESULTS_CAP_BYTES: usize = 100 * 1024 * 1024;

/// A search match. `line`/`col` are 0-based; `col`/`len` are in code points.
pub struct SearchMatch {
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub preview: String,
}

impl SearchMatch {
    /// Estimated memory for the entry (for the 100 MB limit).
    pub fn approx_bytes(&self) -> usize {
        self.preview.capacity() + std::mem::size_of::<Self>()
    }
}

/// Messages from the process worker.
pub enum ProcMsg {
    SearchBatch(Vec<SearchMatch>),
    /// Successful completion.
    Done,
    /// The worker saw the cancel flag and exited.
    Cancelled,
    /// Fatal process error.
    Failed(String),
}

/// A process currently running on a tab.
pub struct RunningProc {
    pub rx: std::sync::mpsc::Receiver<ProcMsg>,
    pub cancel: Arc<AtomicBool>,
    /// The results limit was exceeded — the process was stopped forcibly.
    pub capped: bool,
}

impl RunningProc {
    pub fn new(rx: std::sync::mpsc::Receiver<ProcMsg>, cancel: Arc<AtomicBool>) -> Self {
        Self { rx, cancel, capped: false }
    }
}

/// Accumulated results of the last search on a tab.
pub struct Results {
    pub matches: Vec<SearchMatch>,
    /// Grid display model (columns/widths/order).
    pub grid: crate::grid::GridModel,
    /// Estimated memory used (for the limit).
    pub bytes: usize,
    /// The limit was exceeded — only part is shown.
    pub truncated: bool,
    /// Grid scroll (f64 pixels on both axes) — lives with this result-panel tab.
    pub scroll: (f64, f64),
    /// Disappearing-overlay scrollbar fade — lives with the tab, next to `scroll`.
    pub fade: crate::vscroll::Fade,
}

impl Results {
    pub fn new() -> Self {
        let grid = crate::grid::GridModel::new(&[
            ("Line", 80.0),
            ("Col", 80.0),
            ("Match", 640.0),
        ]);
        Self { matches: Vec::new(), grid, bytes: 0, truncated: false, scroll: (0.0, 0.0), fade: crate::vscroll::Fade::default() }
    }
    pub fn len(&self) -> usize {
        self.matches.len()
    }

    /// Cell values of row `r` in column order (single source for the grid and the heights).
    pub fn row_values(&self, r: usize) -> Vec<String> {
        let m = &self.matches[r];
        vec![(m.line + 1).to_string(), (m.col + 1).to_string(), m.preview.clone()]
    }
}
