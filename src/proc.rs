//! Background-process framework for a tab: search / validation / formatting.
//!
//! At most one process can run on a tab; tabs are independent. The worker lives
//! in its own thread, sends messages over a channel (polled in the update loop), and stops
//! on an `AtomicBool`. Results accumulate in memory; once the 100 MB limit is exceeded the process
//! stops, what has been accumulated is shown, and an error is reported to the status bar.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Memory limit for a process's accumulated results (results only, not the file's data).
pub const RESULTS_CAP_BYTES: usize = 100 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProcKind {
    Search,
    Validate,
    Format,
}

impl ProcKind {
    pub fn title(self) -> &'static str {
        match self {
            ProcKind::Search => "Search",
            ProcKind::Validate => "Inspect",
            ProcKind::Format => "Format",
        }
    }
    pub fn stopped_word(self) -> &'static str {
        "stopped"
    }
}

/// Throttles the worker's progress updates: sends `Progress` only when it grows by ≥1 %, capping
/// the value at `cap` (validation/format hold at 99, search at 100, so only the final step sets 100 %).
pub struct ProgressThrottle {
    last: f32,
}

impl ProgressThrottle {
    pub fn new() -> Self {
        Self { last: -1.0 }
    }
    /// Send progress `pct` (0..100) if it has grown by at least 1 % since the last send.
    pub fn maybe_send(&mut self, tx: &std::sync::mpsc::Sender<ProcMsg>, pct: f32, cap: f32) {
        if pct - self.last >= 1.0 {
            self.last = pct;
            let _ = tx.send(ProcMsg::Progress(pct.min(cap)));
        }
    }
}

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

/// Severity of a validation finding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "Error",
            Severity::Warning => "Warning",
        }
    }
}

/// A validation finding (XSD or model rule).
pub struct Finding {
    pub severity: Severity,
    /// 1-based line; 0 means unknown.
    pub line: usize,
    pub code: String,
    pub message: String,
    /// Source: «XSD», «clause 5.6», «sec. 6, p. 83»… (not shown in the grid — duplicates the «Code» column).
    #[allow(dead_code)]
    pub source: String,
}

impl Finding {
    pub fn approx_bytes(&self) -> usize {
        self.code.capacity()
            + self.message.capacity()
            + self.source.capacity()
            + std::mem::size_of::<Self>()
    }
}

/// Messages from the process worker.
pub enum ProcMsg {
    Progress(f32), // 0..100
    SearchBatch(Vec<SearchMatch>),
    Findings(Vec<Finding>),
    /// Formatting succeeded: path to the temporary UTF-8 result file.
    FormatOk { out_path: PathBuf },
    /// Formatting error: malformed XML. `line`/`col` are 1-based.
    FormatErr { line: usize, msg: String },
    /// Successful completion (search/validation).
    Done,
    /// The worker saw the cancel flag and exited.
    Cancelled,
    /// Fatal process error.
    Failed(String),
}

/// A process currently running on a tab.
pub struct RunningProc {
    pub kind: ProcKind,
    pub rx: std::sync::mpsc::Receiver<ProcMsg>,
    pub cancel: Arc<AtomicBool>,
    pub started: std::time::Instant,
    pub progress: f32,
    /// Model id for the validation label; empty for the other processes.
    pub schema: String,
    /// The results limit was exceeded — the process was stopped forcibly.
    pub capped: bool,
}

impl RunningProc {
    /// Create a running process with the common prologue (start timer, progress 0, limit not exceeded).
    pub fn new(
        kind: ProcKind,
        rx: std::sync::mpsc::Receiver<ProcMsg>,
        cancel: Arc<AtomicBool>,
        schema: String,
    ) -> Self {
        Self {
            kind,
            rx,
            cancel,
            started: std::time::Instant::now(),
            progress: 0.0,
            schema,
            capped: false,
        }
    }

    /// Process label for the status bar: the validation label (with the model id) or the search label.
    pub fn label(&self) -> String {
        if self.schema.is_empty() {
            self.kind.title().to_owned()
        } else {
            format!("{} (схема {})", self.kind.title(), self.schema)
        }
    }
}

/// Accumulated results of the last search/validation on a tab.
pub enum ResultsKind {
    Search(Vec<SearchMatch>),
    Validation(Vec<Finding>),
}

pub struct Results {
    pub kind: ResultsKind,
    /// Grid display model (columns/widths/order) — each kind has its own table.
    pub grid: crate::grid::GridModel,
    /// Estimated memory used (for the limit).
    pub bytes: usize,
    /// The limit was exceeded — only part is shown.
    pub truncated: bool,
    /// Grid scroll (f64 pixels on both axes) — lives with this result-panel tab.
    pub scroll: (f64, f64),
}

impl Results {
    pub fn new_search() -> Self {
        let grid = crate::grid::GridModel::new(&[
            ("Line", 80.0),
            ("Col", 80.0),
            ("Match", 640.0),
        ]);
        Self { kind: ResultsKind::Search(Vec::new()), grid, bytes: 0, truncated: false, scroll: (0.0, 0.0) }
    }
    pub fn new_validation() -> Self {
        let grid = crate::grid::GridModel::new(&[
            ("Type", 90.0),
            ("Line", 80.0),
            ("Code", 100.0),
            ("Message", 640.0),
        ]);
        Self { kind: ResultsKind::Validation(Vec::new()), grid, bytes: 0, truncated: false, scroll: (0.0, 0.0) }
    }
    pub fn len(&self) -> usize {
        match &self.kind {
            ResultsKind::Search(v) => v.len(),
            ResultsKind::Validation(v) => v.len(),
        }
    }

    /// Cell values of row `r` in column order (single source for the grid and the heights).
    pub fn row_values(&self, r: usize) -> Vec<String> {
        match &self.kind {
            ResultsKind::Search(v) => {
                let m = &v[r];
                vec![(m.line + 1).to_string(), (m.col + 1).to_string(), m.preview.clone()]
            }
            ResultsKind::Validation(v) => {
                let f = &v[r];
                vec![
                    f.severity.label().to_owned(),
                    if f.line > 0 { f.line.to_string() } else { "—".to_owned() },
                    f.code.clone(),
                    f.message.clone(),
                ]
            }
        }
    }

    /// Row `r` is an error (for the red bar/text in the validation grid).
    pub fn row_is_err(&self, r: usize) -> bool {
        matches!(&self.kind, ResultsKind::Validation(v) if v[r].severity == Severity::Error)
    }
}
