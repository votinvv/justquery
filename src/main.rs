#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! JustQuery — a native PostgreSQL IDE (silvery light theme). Live connections run over TLS on
//! background threads; each tab executes on its own session connection. The grid shows real
//! query results.
//!
//! `main.rs` holds the application state and all screen-level layout (`JustQueryApp`); the
//! self-contained pieces live in sibling modules:
//!   - [`widgets`]  — painted UI helpers + the custom window chrome (decorations = false)
//!   - [`codeeditor`] — the virtualized SQL editor (renders only visible lines; caret, selection,
//!     undo, kinetic scrolling); [`complete`] — F6 autocomplete + Smart Enter/Tab;
//!     [`highlight`] — the SQL syntax highlighter (per visible line)
//!   - [`connections`] — saved connections, the live connect, and per-tab query execution
//!   - [`metadata`] / [`meta_collector`] / [`meta_details`] / [`meta_manager_modal`] — the
//!     Metadata Manager: shared object store, background SCANER, on-demand columns, the SCAN chip

use eframe::egui;
use egui::{Align, Color32, Layout, Margin, RichText, CornerRadius, Stroke, Vec2};
use std::path::PathBuf;
use std::sync::Mutex;

mod codeeditor;
mod complete;
mod connections;
mod crypt;
mod dialog;
mod fileops;
mod find;
mod grid;
mod highlight;
mod meta_collector;
mod meta_details;
mod meta_manager_modal;
mod metadata;
#[cfg(test)]
mod sample; // demo data for the result-grid tests only (not shipped in the product)
mod sqlfmt;
mod icons;
mod theme;
mod update;
mod widgets;
#[cfg(test)]
mod tests;

use connections::Connection;
// The result grid + its data model live in `grid`; re-export `ResultSet` so the worker / catalog
// code can keep naming it `crate::ResultSet`.
pub(crate) use grid::ResultSet;
use grid::GridSel;
// All semantic colours + style metrics live in `theme`; re-export so the whole crate (and the
// custom-painted widgets via `crate::PANEL2`, …) can use them by name.
pub use theme::*;
use widgets::*;

/// Last captured panic message (shown in the status bar instead of crashing).
static LAST_PANIC: Mutex<Option<String>> = Mutex::new(None);

/// Semantic aliases over the JustQuery glyph font (`src/icons.rs`). Call sites keep their
/// action-named constants; the drawing language is the single authored set.
mod ic {
    use crate::icons;
    pub const NEW: &str = icons::NEW_QUERY;
    pub const OPEN: &str = icons::OPEN;
    pub const SAVE: &str = icons::SAVE;
    pub const CONNECT: &str = icons::PLUG; // the toolbar connection toggle
    pub const PLAY: &str = icons::RUN;
    pub const STOP: &str = icons::STOP;
    pub const COMMIT: &str = icons::COMMIT;
    pub const ROLLBACK: &str = icons::ROLLBACK;
    pub const FETCH_NEXT: &str = icons::CHEVRON_DOWN;
    pub const FETCH_ALL: &str = icons::CHEVRONS_DOWN;
    pub const REFRESH: &str = icons::REFRESH;
    pub const EXPAND: &str = icons::CHEVRONS_UP;
    pub const COLLAPSE: &str = icons::CHEVRONS_DOWN;
    pub const MANAGER: &str = icons::PANEL_LEFT;
    pub const META: &str = icons::PANEL_TREE;
    pub const PLUS: &str = icons::PLUS;
    pub const SEARCH: &str = icons::FIND;
    pub const VALIDATE: &str = icons::KEY; // the spare glyph stands in for "house rules pass"
    pub const FORMAT: &str = icons::FORMAT;
    // SCAN chip: ONE refresh glyph in every state — the colour carries the state
    // (icons/README: "refresh — metadata dock: rescan; статус scan")
    pub const SCAN_OK: &str = icons::REFRESH;
    pub const SCAN_SLEEP: &str = icons::REFRESH;
    pub const SCAN_FAIL: &str = icons::REFRESH;
    pub const SCAN_OFF: &str = icons::REFRESH;
    // Metadata tree disclosure markers
    pub const TREE_COLLAPSED: &str = icons::CHEVRON_RIGHT;
    pub const TREE_EXPANDED: &str = icons::CHEVRON_DOWN;
    // Metadata object-type icons
    pub const OBJ_TABLE: &str = icons::TABLE;
    pub const OBJ_VIEW: &str = icons::VIEW;
    pub const OBJ_MATVIEW: &str = icons::VIEW;
    pub const OBJ_SEQUENCE: &str = icons::SEQUENCE;
    pub const OBJ_FUNCTION: &str = icons::FUNCTION;
    pub const OBJ_OTHER: &str = icons::DATABASE;
    pub const DELETE: &str = icons::TRASH;
}

fn main() -> eframe::Result<()> {
    // Capture panics into LAST_PANIC so the UI can show them instead of dying.
    std::panic::set_hook(Box::new(|info| {
        let msg = info.to_string();
        if let Ok(mut g) = LAST_PANIC.lock() {
            *g = Some(msg);
        }
    }));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("JustQuery")
            .with_icon(std::sync::Arc::new(app_icon()))
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([760.0, 480.0])
            .with_visible(false) // stay hidden until maximized, so it appears full-size at once
            .with_decorations(false), // custom caption bar instead of the OS frame
        ..Default::default()
    };
    eframe::run_native(
        "JustQuery",
        options,
        Box::new(|cc| {
            theme::setup_fonts(&cc.egui_ctx);
            // restore the persisted theme BEFORE the first frame so there's no light flash
            theme::set_theme(&cc.egui_ctx, load_saved_theme());
            let mut app = JustQueryApp::default();
            app.connections = connections::load(); // restore saved connections
            update::startup_cleanup(); // remove any leftover justquery.old from a prior update
            app.start_update_check(); // background GitHub version check (fills the status chip)
            Ok(Box::new(app))
        }),
    )
}

// ============================================================
// App settings (settings.json next to the connections store)
// ============================================================

/// `%APPDATA%\JustQuery\settings.json` — tiny hand-rolled JSON, same no-serde policy as
/// the rest of the app (see update.rs). Currently holds only `{"theme":"light|dark"}`.
fn settings_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("JustQuery").join("settings.json"))
}

fn load_saved_theme() -> theme::AppTheme {
    let dark = settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.contains("\"theme\"") && s.contains("\"dark\""))
        .unwrap_or(false);
    if dark { theme::AppTheme::Dark } else { theme::AppTheme::Light }
}

fn save_theme(t: theme::AppTheme) {
    let Some(path) = settings_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let name = match t {
        theme::AppTheme::Dark => "dark",
        theme::AppTheme::Light => "light",
    };
    let _ = std::fs::write(path, format!("{{\"theme\":\"{name}\"}}\n"));
}

// ============================================================
// App icon (coral rounded square — matches the in-app logo)
// ============================================================
fn app_icon() -> egui::IconData {
    let size: usize = 64;
    let ss: u32 = 3;
    let s = size as f32;
    let margin = s * 0.06;
    let (x0, y0, x1, y1) = (margin, margin, s - margin, s - margin);
    let r = (x1 - x0) * 0.22;

    // The "JQ" monogram, matching widgets::logo: clay rounded square + a white "J" polyline + a
    // "Q" (ring + diagonal tail). Coordinates are normalised over the full square (×s).
    let n = |v: f32| v * s;
    let j_pts: [(f32, f32); 10] = [
        (0.27, 0.30), (0.46, 0.30),
        (0.43, 0.30), (0.43, 0.60),
        (0.43, 0.635), (0.415, 0.685), (0.375, 0.715),
        (0.325, 0.722), (0.275, 0.700), (0.255, 0.655),
    ];
    let j_hw = 0.092 * s * 0.5; // J stroke half-width
    let (qcx, qcy) = (n(0.66), n(0.50));
    let q_rad = 0.12 * s;
    let q_hw = 0.09 * s * 0.5; // ring stroke half-width
    let (tx0, ty0, tx1, ty1) = (n(0.685), n(0.585), n(0.795), n(0.715));
    let tail_hw = 0.092 * s * 0.5;

    let seg_d = |px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32| -> f32 {
        let (vx, vy) = (bx - ax, by - ay);
        let len2 = vx * vx + vy * vy;
        let t = if len2 > 0.0 { (((px - ax) * vx + (py - ay) * vy) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let (cx, cy) = (ax + t * vx, ay + t * vy);
        ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
    };
    let on_glyph = |fx: f32, fy: f32| -> bool {
        for w in j_pts.windows(2) {
            if seg_d(fx, fy, n(w[0].0), n(w[0].1), n(w[1].0), n(w[1].1)) <= j_hw {
                return true;
            }
        }
        let dr = ((fx - qcx).powi(2) + (fy - qcy).powi(2)).sqrt();
        if (dr - q_rad).abs() <= q_hw {
            return true;
        }
        seg_d(fx, fy, tx0, ty0, tx1, ty1) <= tail_hw
    };

    let mut rgba = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let mut sq_acc = 0u32; // subsamples inside the rounded square
            let mut wh_acc = 0u32; // subsamples on a white glyph stroke (within the square)
            for sy in 0..ss {
                let fy = y as f32 + (sy as f32 + 0.5) / ss as f32;
                for sx in 0..ss {
                    let fx = x as f32 + (sx as f32 + 0.5) / ss as f32;
                    let cx = fx.clamp(x0 + r, x1 - r);
                    let cy = fy.clamp(y0 + r, y1 - r);
                    let (dx, dy) = (fx - cx, fy - cy);
                    if dx * dx + dy * dy <= r * r {
                        sq_acc += 1;
                        if on_glyph(fx, fy) {
                            wh_acc += 1;
                        }
                    }
                }
            }
            let tot = (ss * ss) as f32;
            let a = (255.0 * sq_acc as f32 / tot) as u8;
            let wf = if sq_acc > 0 { wh_acc as f32 / sq_acc as f32 } else { 0.0 };
            let blend = |clay: f32| (clay * (1.0 - wf) + 255.0 * wf) as u8;
            let i = (y * size + x) * 4;
            rgba[i] = blend(0xC9 as f32);
            rgba[i + 1] = blend(0x64 as f32);
            rgba[i + 2] = blend(0x42 as f32);
            rgba[i + 3] = a;
        }
    }
    egui::IconData {
        rgba,
        width: size as u32,
        height: size as u32,
    }
}

/// Format a running duration as days / hours / minutes / seconds (only the relevant leading units),
/// e.g. `7s`, `1m 05s`, `2h 03m 09s`, `1d 04h 12m 30s`.
fn fmt_elapsed(d: std::time::Duration) -> String {
    let s = d.as_secs();
    let (days, h, m, sec) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60, s % 60);
    if days > 0 {
        format!("{days}d {h:02}h {m:02}m {sec:02}s")
    } else if h > 0 {
        format!("{h}h {m:02}m {sec:02}s")
    } else if m > 0 {
        format!("{m}m {sec:02}s")
    } else {
        format!("{sec}s")
    }
}

/// 1-based line / column of a character index within `s` (for "Ln X, Col Y" status messages).
fn line_col_at(s: &str, pos: usize) -> (usize, usize) {
    let (mut ln, mut col) = (1usize, 1usize);
    for ch in s.chars().take(pos) {
        if ch == '\n' {
            ln += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (ln, col)
}

// ============================================================
// App state
// ============================================================
#[derive(Clone, PartialEq)]
enum ConfirmAction {
    ExitApp,
    CloseTab(usize),
    DeleteConnections(Vec<u64>),
}

/// What to do once the "running queries / uncommitted work" prompt is resolved by killing it.
#[derive(Clone, Copy, PartialEq)]
enum PendingConn {
    Connect,
    Disconnect,
}

/// Which manager occupies the left dock. Only one shows at a time (one active toolbar button);
/// `left_panel == None` means the dock is closed.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum LeftPanel {
    Database,
    Metadata,
}

/// Streamed from the background query thread: result sets and message lines as each statement
/// completes, then `Done` returns the connection so the app can reclaim it.
enum ExecMsg {
    Ready(postgres::CancelToken), // worker has a live client → here's its cancel token (for Stop)
    Result(ResultSet),
    Log(LogEntry),
    Done(Option<Box<postgres::Client>>), // hand the tab's session connection back (None = no client)
}

/// One row of the execution log shown in the Messages grid: when the statement ran, its status,
/// timings, row count, message and the statement text.
struct LogEntry {
    time: String,    // local HH:MM:SS
    status: String,  // OK / Error
    exec: f64,       // execution seconds
    fetch: f64,      // fetch seconds
    rows: i64,       // rows returned/affected (-1 = n/a)
    message: String, // note or error text
    sql: String,     // the statement
}

/// One editor window. Usually a SQL editor; when `conn` is set it is instead a connection-
/// settings tab (the form rendered by `connection_tab`).
struct Tab {
    id: u64, // stable id → egui remembers caret + scroll per tab
    title: String,
    sql: String,
    path: Option<PathBuf>,    // backing .sql file, if opened from / saved to disk
    conn: Option<Connection>, // Some → this tab edits a database connection, not SQL
    meta: Option<metadata::MetaObject>, // Some → this tab views an object's metadata, not SQL
    dirty: bool,
    executed: bool,
    result_tab: usize,         // 0 = Messages, 1.. = results[result_tab - 1]
    results: Vec<ResultSet>,   // one per result-producing statement in the last run
    log: Vec<LogEntry>,        // the Messages grid: one row per executed statement
    result_height: f32, // result-panel height lives with the tab, not globally
    result_full: bool,  // result panel maximized — also per-tab, not shared
    running: bool,      // a query is executing on this tab's session connection
    tx_open: bool,      // this tab left an uncommitted transaction open on the connection
    // this tab's own session connection (None until the first query is run on it; kept open
    // afterwards so SET / temp tables / prepared statements persist between queries). It is
    // checked out into the worker thread while a query runs.
    client: Option<postgres::Client>,
    exec_rx: Option<std::sync::mpsc::Receiver<ExecMsg>>, // in-flight query stream for this tab
    exec_cancel: Option<postgres::CancelToken>,          // out-of-band Stop for this tab's query
    exec_start: Option<std::time::Instant>,              // query timer for this tab
    // Some(i) while a single-result Refresh is in flight → the streamed Result replaces
    // results[i] in place instead of being appended (and the log isn't cleared)
    refresh_idx: Option<usize>,
    ed: codeeditor::EditorState, // caret / selection / undo / line index for the SQL editor
}

impl Tab {
    fn new(id: u64, title: String) -> Self {
        Self {
            id,
            title,
            sql: String::new(),
            path: None,
            conn: None,
            meta: None,
            dirty: false,
            executed: false,
            result_tab: 0,
            results: Vec::new(),
            log: Vec::new(),
            result_height: 300.0,
            result_full: false,
            running: false,
            tx_open: false,
            client: None,
            exec_rx: None,
            exec_cancel: None,
            exec_start: None,
            refresh_idx: None,
            ed: codeeditor::EditorState::default(),
        }
    }

    /// True for an ordinary SQL editor tab (not a connection-settings or metadata tab; About/Scan
    /// are modals now). Used to gate the SQL toolbar, status-bar Ln/Col, execution, etc.
    fn is_sql(&self) -> bool {
        self.conn.is_none() && self.meta.is_none()
    }

    // ---- SQL editor edit ops (Edit menu) — operate on this tab's buffer + editor state ----
    fn ed_undo(&mut self) -> bool {
        self.ed.sync(&self.sql);
        self.ed.undo_op(&mut self.sql)
    }
    fn ed_redo(&mut self) -> bool {
        self.ed.sync(&self.sql);
        self.ed.redo_op(&mut self.sql)
    }
    fn ed_paste(&mut self, t: &str) {
        self.ed.sync(&self.sql);
        let norm = t.replace("\r\n", "\n").replace('\r', "\n");
        self.ed.replace(&mut self.sql, &norm, codeeditor::EditKind::None);
    }
    fn ed_cut(&mut self) -> Option<String> {
        self.ed.sync(&self.sql);
        if !self.ed.has_sel() {
            return None;
        }
        let s = self.ed.selection_text(&self.sql);
        self.ed.replace(&mut self.sql, "", codeeditor::EditKind::Delete);
        Some(s)
    }
    fn ed_copy(&self) -> Option<String> {
        if !self.ed.has_sel() {
            return None;
        }
        Some(self.ed.selection_text(&self.sql))
    }

    /// Drop this tab's session connection and abandon any in-flight query, returning the tab to
    /// the "no session" state. Shared by connect / disconnect / kill so a (re)connect starts clean
    /// (dropping `exec_rx` makes the worker's sends fail, so it drops its client when it finishes).
    fn reset_session(&mut self) {
        self.client = None;
        self.exec_rx = None;
        self.exec_cancel = None;
        self.exec_start = None;
        self.running = false;
        self.tx_open = false;
        self.refresh_idx = None;
    }
}

/// Background worker shared by Execute (whole buffer) and Refresh (one statement): open/reuse the
/// tab's session connection, hand back a cancel token, run each statement and stream its result
/// sets + a log row, then return the (still-open) connection. Each result set carries the SQL that
/// produced it so a later per-result Refresh can re-run exactly that statement.
fn run_statements_worker(
    existing: Option<postgres::Client>,
    params: connections::ConnParams,
    statements: Vec<String>,
    tx: std::sync::mpsc::Sender<ExecMsg>,
) {
    let mut client = match existing {
        Some(c) => c,
        None => match connections::connect_session(&params) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(ExecMsg::Log(LogEntry {
                    time: dialog::now_hms(),
                    status: "Error".to_owned(),
                    exec: 0.0,
                    fetch: 0.0,
                    rows: -1,
                    message: format!("Connection failed: {e}"),
                    sql: String::new(),
                }));
                let _ = tx.send(ExecMsg::Done(None));
                return;
            }
        },
    };
    // hand back a cancel token now that we have a live client (enables Stop)
    let _ = tx.send(ExecMsg::Ready(client.cancel_token()));
    for stmt in statements {
        let time = dialog::now_hms();
        let t0 = std::time::Instant::now();
        let outs = connections::run_statement(&mut client, &stmt);
        let exec = t0.elapsed().as_secs_f64();
        let mut rows: i64 = -1;
        let mut status = "OK";
        let mut message = String::new();
        for out in outs {
            match out {
                connections::SqlOut::Rows(mut rs) => {
                    rs.sql = stmt.clone(); // remember the source statement (for Refresh)
                    rows = rs.rows.len() as i64;
                    if tx.send(ExecMsg::Result(rs)).is_err() {
                        return;
                    }
                }
                connections::SqlOut::Note(s) => {
                    if s.starts_with("Error") {
                        status = "Error";
                    }
                    if !message.is_empty() {
                        message.push_str("; ");
                    }
                    message.push_str(&s);
                }
            }
        }
        if message.is_empty() && rows >= 0 {
            message = format!("{rows} rows");
        }
        let entry = LogEntry {
            time,
            status: status.to_owned(),
            exec,
            fetch: 0.0,
            rows,
            message,
            sql: stmt,
        };
        if tx.send(ExecMsg::Log(entry)).is_err() {
            return;
        }
    }
    // hand the (still-open) session connection back to the tab to reuse next time
    let _ = tx.send(ExecMsg::Done(Some(Box::new(client))));
}

struct JustQueryApp {
    // saved connections + dialogs
    connections: Vec<Connection>,
    active_label: String, // "user@db" — shown in the status-bar connection chip while connected
    conn_broken: bool,    // was connected, then the connection dropped (chip turns red)
    did_startup_connect: bool, // the one-time "open the Connect dialog on launch" has fired
    window_title: String,      // last OS window title we pushed (avoid re-sending every frame)
    about_open: bool,          // the About / Updates modal is up (replaces the old About tab)
    scan_open: bool,           // the Scan manager modal is up (replaces the old Scan tab)
    connect_open: bool,
    connect_sel: usize,
    connect_user: String,
    connect_pass: String,
    connect_error: Option<String>, // last connection failure — shown inside the Connect modal
    left_panel: Option<LeftPanel>, // which manager occupies the left dock (None = closed)
    dbmgr_rename: Option<u64>,   // id of the connection whose name is being edited inline
    dbmgr_rename_buf: String,    // inline-rename text buffer
    dbmgr_rename_focus: bool,    // request focus for the rename field next frame
    dbmgr_conflict: Option<(u64, String)>, // (id, suggested free name) — duplicate-name prompt
    conn_sel: Vec<u64>,          // selected connection ids (left-click; Ctrl/Shift multi-select)
    conn_anchor: Option<usize>,  // Shift-range anchor into the connection list
    // a second single-click on the already-selected connection arms rename (fires after the
    // double-click window so a real double-click opens instead) — Windows Explorer behaviour
    conn_rename_armed: Option<(u64, std::time::Instant)>,
    // the connection currently held down + whether it was already the sole selection at press
    // start (so a plain click on the already-selected row can arm rename on release)
    conn_pressed: Option<(u64, bool)>,
    no_conn_open: bool,
    // ---- Metadata Manager ----
    collector: Option<meta_collector::CollectorHandle>, // background object-list scanner
    details: Option<meta_details::DetailsHandle>,       // on-demand attribute fetcher
    meta_store: std::sync::Arc<metadata::SharedStore>,  // live store shared with the collector thread
    meta_view: metadata::MetaStore,                     // displayed snapshot (refreshed on demand)
    meta_view_gen: u64,                                 // store generation captured in meta_view
    collector_status: metadata::CollectorStatus,
    collector_log: std::collections::VecDeque<metadata::LogLine>,
    meta_schema_sel: Option<String>,                   // schema picked in the panel dropdown
    meta_folders_open: std::collections::HashSet<String>, // expanded type folders (per schema/label)
    meta_obj_sel: Vec<String>,    // selected object keys "schema/kind/name" (Ctrl/Shift multi-select)
    meta_obj_anchor: Option<usize>, // Shift-range anchor into the visible object list
    meta_req_seq: u64,                                  // correlates details requests ↔ replies
    last_activity_ping: Option<std::time::Instant>,    // throttle for the scanner activity ping
    pending_meta_settings: Option<metadata::CollectorSettings>, // captured at Connect, used on success
    active_conn_id: Option<u64>,                        // connected connection's id (for settings persist)
    edit_interval: u64,                                 // modal edit buffers
    edit_budget: usize,
    edit_idle: u64,
    edit_schemas: Option<Vec<String>>,
    // Scan modal transfer-list selection: the highlighted rows + a Shift-range anchor, per pane
    meta_sel_avail: Vec<String>,
    meta_sel_mon: Vec<String>,
    meta_anchor_avail: Option<usize>,
    meta_anchor_mon: Option<usize>,
    // tabs — each carries its own editor text and result state
    tabs: Vec<Tab>,
    active_tab: usize,
    next_tab_id: u64,
    cursor_ln: usize,
    cursor_col: usize,
    focus_editor: bool,
    // result / connection
    connected: bool,
    main_conn: Option<postgres::Client>, // the control connection (held open; tabs run their own)
    // in-flight main connect (background thread) → Ok(client) / Err(message)
    connect_rx: Option<std::sync::mpsc::Receiver<Result<postgres::Client, String>>>,
    pending_label: String,             // "user@db" to show once the in-flight connect succeeds
    busy_prompt: Option<PendingConn>,  // connect/disconnect waiting on a "kill running work?" prompt
    // resolved credentials of the active connection, captured at Connect time. `main_conn` is the
    // control connection; each tab opens its OWN session connection (lazily, on first run) from
    // these params so tabs execute independently and keep session state between queries.
    conn_params: Option<connections::ConnParams>,
    grid_sel: Option<GridSel>,         // cell selection in the active result grid (for copy)
    show_result: bool,
    page: usize, // incremental-fetch step (rows revealed per "fetch more")
    // window
    startup_frame: u8, // 0..: maximize first, then reveal the window (hidden until full-size)
    confirm: Option<ConfirmAction>,
    disconnect_confirm: bool, // the plug toggle asked to disconnect — confirm modal is up
    allow_close: bool,
    // in-app update: background GitHub version check + self-update (see `update` module)
    update_status: update::UpdateStatus, // transient op + About-page state
    // last completed check's verdict, persisted to disk; drives the status chip so it stays
    // LATEST / NOT LATEST through checks, downloads and errors (None = unknown → shown as LATEST)
    update_outdated: Option<bool>,
    update_rx: Option<std::sync::mpsc::Receiver<update::UpdateMsg>>,
    last_error: Option<String>, // only the panic-recovery message (shown in the status bar)
    // validate / format outcome shown in the status bar: (message, is_error)
    fmt_status: Option<(String, bool)>,
    error_modal: Option<String>, // operation errors go to a modal, not the status bar
    test_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>, // in-flight Test Connection
    test_result: Option<Result<String, String>>, // Test Connection outcome → modal
    editor_rect: egui::Rect, // sheet rect of the editor (to anchor the find bar)
    // editor find bar (first iteration — search only, no replace)
    caret: usize, // primary caret position in the active editor, as a char index
    find_open: bool,
    find_query: String,
    find_focus: bool, // request focus into the find field next frame
    find_case: bool,  // case-sensitive matching
    find_whole_word: bool,  // match whole words only
    find_wrap: bool,        // wrap around at the ends
    find_count: usize,
    find_index: usize,                    // 1-based index of the current match (0 = none)
    find_match_start: Option<usize>,      // char start of the currently highlighted match
    pending_find: Option<(usize, usize)>, // char range to select + scroll to next frame
    tab_scroll: f32,                      // pending horizontal scroll for the editor tab strip
    tab_overflow: bool,                   // editor tabs don't fit → show the ‹ › scroll buttons
    ac: complete::Autocomplete,           // F6 completion popup state
    // virtualized editor: per-line highlight galley cache (keyed by line content)
    line_cache: codeeditor::LineCache,
    // theme the previous frame was painted with — detects a live theme switch so the galley
    // cache can be dropped on the FIRST frame of the new theme (see update_inner)
    painted_theme: theme::AppTheme,
    scroll_active_until: f64, // keep repainting until this time (smooth trackpad-flick momentum)
    // custom kinetic scrolling for the trackpad (Windows delivers flick inertia as one delayed
    // lump — we ignore it and run our own momentum from the finger-lift velocity instead)
    scroll_vel: egui::Vec2,           // current momentum velocity (wheel "lines"/s, both axes)
    scroll_recent: Vec<(f64, egui::Vec2)>, // recent finger deltas (time, delta) for lift-velocity
    scroll_last_touch_t: f64,         // time of the last finger (fractional) wheel event
    scroll_touch_active: bool,        // a finger gesture is in progress (events arriving)
    scroll_prev_t: f64,               // previous frame time (for dt)
    focus_grace: u8, // frames to keep re-grabbing editor focus after new/open (survive click-clear)
    prev_focused: bool, // viewport focus last frame (edge-detect alt-tab return in raw_input_hook)
    last_pointer: Option<egui::Pos2>, // last valid cursor pos, re-seeded on focus regain
}

impl Default for JustQueryApp {
    fn default() -> Self {
        Self {
            connections: Vec::new(), // loaded from disk in main()
            active_label: String::new(),
            conn_broken: false,
            did_startup_connect: false,
            window_title: String::new(),
            about_open: false,
            scan_open: false,
            connect_open: false,
            connect_sel: 0,
            connect_user: String::new(),
            connect_pass: String::new(),
            connect_error: None,
            left_panel: None,
            dbmgr_rename: None,
            dbmgr_rename_buf: String::new(),
            dbmgr_rename_focus: false,
            dbmgr_conflict: None,
            conn_sel: Vec::new(),
            conn_anchor: None,
            conn_rename_armed: None,
            conn_pressed: None,
            no_conn_open: false,
            collector: None,
            details: None,
            meta_store: std::sync::Arc::new(metadata::SharedStore::default()),
            meta_view: metadata::MetaStore::default(),
            meta_view_gen: 0,
            collector_status: metadata::CollectorStatus::default(),
            collector_log: std::collections::VecDeque::new(),
            meta_schema_sel: None,
            meta_folders_open: std::collections::HashSet::new(),
            meta_obj_sel: Vec::new(),
            meta_obj_anchor: None,
            meta_req_seq: 0,
            last_activity_ping: None,
            pending_meta_settings: None,
            active_conn_id: None,
            edit_interval: 30,
            edit_budget: 1_000_000,
            edit_idle: 300,
            edit_schemas: None,
            meta_sel_avail: Vec::new(),
            meta_sel_mon: Vec::new(),
            meta_anchor_avail: None,
            meta_anchor_mon: None,
            tabs: vec![],
            active_tab: 0,
            next_tab_id: 0,
            cursor_ln: 1,
            cursor_col: 1,
            focus_editor: false,
            connected: false,
            main_conn: None,
            connect_rx: None,
            pending_label: String::new(),
            busy_prompt: None,
            conn_params: None,
            grid_sel: None,
            show_result: true,
            page: 100,
            startup_frame: 0,
            confirm: None,
            disconnect_confirm: false,
            allow_close: false,
            update_status: update::UpdateStatus::NeverChecked,
            update_outdated: None,
            update_rx: None,
            last_error: None,
            fmt_status: None,
            error_modal: None,
            test_rx: None,
            test_result: None,
            editor_rect: egui::Rect::ZERO,
            caret: 0,
            find_open: false,
            find_query: String::new(),
            find_focus: false,
            find_case: false,
            find_whole_word: false,
            find_wrap: true,
            find_count: 0,
            find_index: 0,
            find_match_start: None,
            pending_find: None,
            tab_scroll: 0.0,
            tab_overflow: false,
            ac: complete::Autocomplete::default(),
            line_cache: codeeditor::LineCache::default(),
            painted_theme: theme::current_theme(),
            scroll_active_until: 0.0,
            scroll_vel: egui::Vec2::ZERO,
            scroll_recent: Vec::new(),
            scroll_last_touch_t: 0.0,
            scroll_touch_active: false,
            scroll_prev_t: 0.0,
            focus_grace: 0,
            prev_focused: true,
            last_pointer: None,
        }
    }
}

impl JustQueryApp {
    fn cur(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }
    fn cur_mut(&mut self) -> Option<&mut Tab> {
        let a = self.active_tab;
        self.tabs.get_mut(a)
    }
    /// The text currently selected in the active SQL editor, if any (for "execute selection").
    fn editor_selection(&self) -> Option<String> {
        let t = self.cur()?;
        if !t.is_sql() || !t.ed.has_sel() {
            return None;
        }
        Some(t.ed.selection_text(&t.sql))
    }
    /// The active SQL editor tab (not a connection / metadata tab), mutably.
    fn ed_active_mut(&mut self) -> Option<&mut Tab> {
        let i = self.active_tab;
        self.tabs.get_mut(i).filter(|t| t.is_sql())
    }
    /// Select the whole editor buffer (Edit ▸ Select All).
    fn editor_select_all(&mut self) {
        if let Some(t) = self.ed_active_mut() {
            t.ed.sync(&t.sql);
            t.ed.select_all();
        }
        self.focus_editor = true;
    }
    /// Validate the active SQL tab against the house rules WITHOUT reformatting (Validate button).
    /// The outcome goes to the status bar; on the first violation the caret jumps onto it.
    fn validate_active(&mut self) {
        if !self.is_sql_tab() {
            return;
        }
        let src = match self.cur() {
            Some(t) => t.sql.clone(),
            None => return,
        };
        match sqlfmt::validate(&src) {
            Ok(()) => self.fmt_status = Some(("Validation passed".into(), false)),
            Err(errs) => self.report_fmt_error(&src, &errs),
        }
    }

    /// Format the active SQL tab in the house style (Format button / Tools ▸ Format SQL / F5). The
    /// formatter is all-or-nothing: any rule violation aborts the run, reports to the status bar and
    /// moves the caret onto the first problem, leaving the buffer untouched. On success the whole
    /// buffer is replaced as one undo step.
    fn format_active(&mut self) {
        if !self.is_sql_tab() {
            return;
        }
        let src = match self.cur() {
            Some(t) => t.sql.clone(),
            None => return,
        };
        match sqlfmt::format(&src) {
            Ok(formatted) => {
                let changed = self.cur().map_or(false, |t| t.sql != formatted);
                if changed {
                    if let Some(t) = self.ed_active_mut() {
                        t.ed.sync(&t.sql);
                        t.ed.select_all();
                        t.ed.replace(&mut t.sql, &formatted, codeeditor::EditKind::None);
                        t.dirty = true;
                    }
                }
                self.focus_editor = true;
                let msg = if changed { "Formatted" } else { "Formatted (no changes)" };
                self.fmt_status = Some((msg.into(), false));
            }
            Err(errs) => self.report_fmt_error(&src, &errs),
        }
    }

    /// Surface the first formatter/validator violation: message + `Ln/Col` in the status bar, and
    /// select the offending fragment in the editor (the editor scrolls it into view next frame).
    fn report_fmt_error(&mut self, src: &str, errs: &[sqlfmt::FmtError]) {
        let Some(e) = errs.first() else { return };
        let (ln, col) = line_col_at(src, e.pos);
        let extra = if errs.len() > 1 {
            format!("  (+{} more)", errs.len() - 1)
        } else {
            String::new()
        };
        self.fmt_status = Some((format!("Ln {ln}, Col {col}: {}{extra}", e.msg), true));
        self.pending_find = Some((e.pos, e.pos + e.len));
        self.focus_editor = true;
    }

    /// The active tab's result-panel tab labels: Messages + one per result set.
    fn result_tab_names(&self) -> Vec<String> {
        let mut names = vec!["Messages".to_owned()];
        let n = self.cur().map_or(0, |t| t.results.len());
        for i in 0..n {
            names.push(format!("Result {}", i + 1));
        }
        names
    }
    /// The result set the active result tab is showing (None on the Messages tab).
    fn cur_result(&self) -> Option<&ResultSet> {
        let t = self.cur()?;
        if t.result_tab == 0 {
            return None;
        }
        t.results.get(t.result_tab - 1)
    }
    /// True when a data grid (not Messages) is shown — i.e. the active result tab has a result set.
    fn is_data_tab(&self) -> bool {
        self.cur_result().is_some()
    }
    /// Row count of the active result set (0 on Messages).
    fn cur_total(&self) -> usize {
        self.cur_result().map_or(0, |r| r.rows.len())
    }
    /// The active result set, mutable (for incremental fetch).
    fn cur_result_mut(&mut self) -> Option<&mut ResultSet> {
        let a = self.active_tab;
        let t = self.tabs.get_mut(a)?;
        let i = t.result_tab.checked_sub(1)?;
        t.results.get_mut(i)
    }
    /// True when the active tab is a SQL editor (not a connection / metadata / About tab).
    fn is_sql_tab(&self) -> bool {
        self.cur().map_or(false, |t| t.is_sql())
    }
    /// True when the active tab is savable: a connection-settings tab, or any SQL tab (an empty
    /// editor counts — saving an empty file is allowed). With no tabs open, or on an About/Scan
    /// page, there is nothing to save, so Save (toolbar / menu / Ctrl+S) is disabled.
    fn can_save(&self) -> bool {
        match self.cur() {
            Some(t) if t.conn.is_some() => true,
            // an empty SQL tab is still savable — saving an empty file is sometimes wanted
            Some(t) => t.is_sql(),
            None => false,
        }
    }
    /// True when an open (uncommitted) transaction exists — enables Commit / Rollback.
    fn in_transaction(&self) -> bool {
        self.connected && self.tabs.iter().any(|t| t.tx_open)
    }

    /// Run the active SQL tab on ITS OWN session connection, on a background thread. The connection
    /// is opened lazily on the first run and kept open afterwards (so session state persists), which
    /// also lets other tabs run concurrently. The result panel opens at once on Messages; result
    /// sets stream in and the user drives the tabs. The UI stays responsive during the query.
    fn execute(&mut self, _ctx: &egui::Context) {
        if !self.is_sql_tab() {
            return;
        }
        let idx = self.active_tab;
        if self.tabs.get(idx).map_or(true, |t| t.exec_rx.is_some()) {
            return; // a query is already running on THIS tab
        }
        let Some(params) = self.conn_params.clone() else {
            self.error_modal = Some("Not connected. Connect to a database first.".to_owned());
            return;
        };
        // run the selection if there is one, otherwise the whole tab
        let sql = self
            .editor_selection()
            .unwrap_or_else(|| self.cur().map(|t| t.sql.clone()).unwrap_or_default());
        if sql.trim().is_empty() {
            return;
        }
        self.grid_sel = None;
        self.show_result = true;
        // reuse this tab's session connection if it already has one (checked out into the worker)
        let existing = {
            let t = &mut self.tabs[idx];
            t.results.clear();
            t.log.clear();
            t.executed = true;
            t.running = true;
            t.result_tab = 0; // park on Messages; the user drives the tabs from here
            t.exec_start = Some(std::time::Instant::now());
            t.client.take()
        };
        let statements = connections::split_statements(&sql);
        let (tx, rx) = std::sync::mpsc::channel();
        self.tabs[idx].exec_rx = Some(rx);
        std::thread::spawn(move || run_statements_worker(existing, params, statements, tx));
    }

    /// Refresh ONLY the active result tab's grid: re-run the single statement that produced it on
    /// the tab's session connection, and replace that result set in place (other grids untouched).
    /// No-op on the Messages tab or while a query is already running on the tab.
    fn refresh_current_result(&mut self, _ctx: &egui::Context) {
        let idx = self.active_tab;
        let Some(t) = self.tabs.get(idx) else { return };
        if t.exec_rx.is_some() {
            return; // a query is already running on this tab
        }
        let Some(ri) = t.result_tab.checked_sub(1) else { return }; // 0 = Messages → nothing to refresh
        let Some(rs) = t.results.get(ri) else { return };
        let sql = rs.sql.clone();
        if sql.trim().is_empty() {
            return;
        }
        let Some(params) = self.conn_params.clone() else {
            self.error_modal = Some("Not connected. Connect to a database first.".to_owned());
            return;
        };
        self.grid_sel = None;
        let existing = {
            let t = &mut self.tabs[idx];
            t.running = true;
            t.exec_start = Some(std::time::Instant::now());
            t.refresh_idx = Some(ri); // streamed Result replaces results[ri] in place
            t.client.take()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.tabs[idx].exec_rx = Some(rx);
        std::thread::spawn(move || run_statements_worker(existing, params, vec![sql], tx));
    }
    fn fetch_more(&mut self) {
        let page = self.page;
        if let Some(rs) = self.cur_result_mut() {
            rs.visible = (rs.visible + page).min(rs.rows.len());
        }
    }
    fn fetch_all_toggle(&mut self) {
        let page = self.page;
        if let Some(rs) = self.cur_result_mut() {
            if rs.loading {
                rs.loading = false;
            } else {
                if rs.visible == 0 {
                    rs.visible = page.min(rs.rows.len());
                }
                rs.loading = true;
            }
        }
        self.show_result = true;
    }

    fn new_tab(&mut self) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(Tab::new(id, "Untitled".to_owned()));
        self.active_tab = self.tabs.len() - 1;
        self.focus_editor = true; // focus the editor with the caret at the start
        self.cursor_ln = 1;
        self.cursor_col = 1;
    }

    /// Open (or focus) the single About / Updates tab. Replaces the old About modal.
    fn open_about_tab(&mut self) {
        // About is a modal now (not a tab) — open it and kick a first update check if needed.
        self.about_open = true;
        if matches!(self.update_status, update::UpdateStatus::NeverChecked) {
            self.start_update_check();
        }
    }

    /// Open the Scan-manager modal (replaces the old Scan tab).
    pub(crate) fn open_scan_tab(&mut self) {
        self.reload_meta_edits(); // sync the staged settings from the active connection
        self.scan_open = true;
    }

    /// Kick the background version check (no-op if a check/download is already running).
    fn start_update_check(&mut self) {
        if self.update_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.update_rx = Some(rx);
        self.update_status = update::UpdateStatus::Checking;
        update::spawn_check(tx);
    }

    /// Kick the background download + apply (no-op if a check/download is already running).
    fn start_update_download(&mut self) {
        if self.update_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.update_rx = Some(rx);
        self.update_status = update::UpdateStatus::Downloading { done: 0, total: 0 };
        update::spawn_download_and_install(tx);
    }

    /// Status-bar version label (plain text, same font/size as the rest of the bar): green when on
    /// the latest build, amber when a newer release exists. Click opens the About modal.
    fn version_chip(&mut self, ui: &mut egui::Ui, sz: f32) {
        let outdated = self.update_outdated == Some(true);
        let (fg, tip) = if outdated {
            (p().warn, "A newer version is available — click to view")
        } else {
            (p().ok, "You're on the latest version")
        };
        let resp = ui.add(
            egui::Label::new(RichText::new(format!("v{}", update::CURRENT_VERSION)).size(sz).color(fg))
                .sense(egui::Sense::click()),
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.on_hover_text(tip).clicked() {
            self.open_about_tab();
        }
    }

    /// Status-bar connection chip: "user@db" in quiet `text_dim` while connected (the SCAN chip
    /// already signals health), `danger` if the connection dropped. Renders nothing when never
    /// connected or deliberately disconnected (handled by the caller, which also owns the separator).
    fn conn_chip(&mut self, ui: &mut egui::Ui, sz: f32) {
        let color = if self.connected {
            p().ok // identity reads green while the session is live (Design Delta v2.1 §5)
        } else if self.conn_broken {
            p().danger
        } else {
            return;
        };
        if !self.active_label.is_empty() {
            ui.label(RichText::new(&self.active_label).size(sz).color(color));
        }
    }

    fn request_close_tab(&mut self, i: usize) {
        if self.tabs.get(i).map_or(false, |t| t.dirty) {
            self.confirm = Some(ConfirmAction::CloseTab(i));
        } else {
            self.close_tab(i);
        }
    }

    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        self.tabs.remove(i);
        if self.tabs.is_empty() {
            self.active_tab = 0;
        } else {
            // the previous tab becomes active (its caret/scroll restore by tab id)
            self.active_tab = i.saturating_sub(1).min(self.tabs.len() - 1);
            self.focus_editor = true;
        }
    }

}

// ============================================================
// Update loop
// ============================================================
impl eframe::App for JustQueryApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        p().panel2.to_normalized_gamma_f32()
    }

    /// Work around a winit quirk: after alt-tab back, the window regains focus but no `CursorMoved`
    /// is sent, so egui's pointer stays `None` and clicks/scroll do nothing until the mouse moves
    /// (keyboard already works, since it doesn't need the pointer). On the focus-regain edge, inject
    /// a synthetic `PointerMoved` at the last known position to re-seed the pointer immediately.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let focused = raw_input.focused;
        if focused && !self.prev_focused {
            if let Some(p) = self.last_pointer {
                raw_input.events.push(egui::Event::PointerMoved(p));
            }
        }
        self.prev_focused = focused;

        // ---- custom kinetic (momentum) scrolling for the trackpad ----
        // Windows delivers a flick's inertia as ONE big wheel event ~0.5s after the finger lifts
        // (verified by tracing), which reads as "scroll, stall, jump". So: scroll the finger phase
        // 1:1, drop that delayed lump, and run our OWN velocity-based momentum from the moment the
        // finger lifts. Mouse-wheel (integer deltas) and zoom (ctrl/⌘) are passed through untouched.
        const GAIN: f32 = 2.5; // finger-lift velocity → momentum (tune for flick distance)
        const DECAY: f32 = 0.04; // per-second velocity retention (lower = stops sooner)
        const LIFT_GAP: f64 = 0.05; // no finger event for this long ⇒ finger lifted
        const LUMP_GAP: f64 = 0.25; // a big delta this long after the finger ⇒ OS inertia lump
        const LUMP_MIN: f32 = 10.0; // …and at least this many lines

        let now = raw_input.time.unwrap_or(self.scroll_prev_t + 0.016);
        let dt = ((now - self.scroll_prev_t).clamp(0.001, 0.1)) as f32;
        self.scroll_prev_t = now;

        let got_finger = {
            let recent = &mut self.scroll_recent;
            let vel = &mut self.scroll_vel;
            let last_touch = &mut self.scroll_last_touch_t;
            let mut finger = false;
            raw_input.events.retain(|ev| {
                if let egui::Event::MouseWheel { delta, modifiers, .. } = ev {
                    if modifiers.command || modifiers.ctrl {
                        return true; // zoom — leave alone
                    }
                    // a precision trackpad reports fractional deltas on the axis it scrolls; a
                    // mouse wheel moves in whole "notches". Treat the event as a finger gesture if
                    // EITHER axis is fractional, so a purely horizontal swipe (dy ≈ 0, dx
                    // fractional) rides our momentum too instead of the OS's delayed lump.
                    let frac = |v: f32| (v - v.round()).abs() > 0.01;
                    let fractional = frac(delta.x) || frac(delta.y);
                    if fractional {
                        if now - *last_touch > LUMP_GAP
                            && delta.x.abs().max(delta.y.abs()) > LUMP_MIN
                        {
                            return false; // drop the OS's delayed inertia lump — we do our own
                        }
                        finger = true;
                        recent.push((now, *delta));
                        *last_touch = now;
                        return true; // finger phase: apply 1:1
                    }
                    *vel = egui::Vec2::ZERO; // a wheel notch cancels momentum, then passes through
                }
                true
            });
            finger
        };
        self.scroll_recent.retain(|(t, _)| now - *t < 0.09);

        if got_finger {
            self.scroll_vel = egui::Vec2::ZERO; // finger down → track 1:1, no momentum
            self.scroll_touch_active = true;
        } else if self.scroll_touch_active && now - self.scroll_last_touch_t > LIFT_GAP {
            // finger just lifted → launch momentum from the recent finger velocity (no OS pause)
            self.scroll_touch_active = false;
            if let Some((t0, _)) = self.scroll_recent.first().copied() {
                let span = (now - t0).max(0.001) as f32;
                let sum = self.scroll_recent.iter().fold(egui::Vec2::ZERO, |a, (_, d)| a + *d);
                self.scroll_vel = (sum / span) * GAIN;
            }
            self.scroll_recent.clear();
        }

        // step the momentum: inject a synthetic wheel delta (both axes) for this frame, then decay
        if self.scroll_vel.length() > 2.0 {
            raw_input.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: self.scroll_vel * dt,
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            });
            self.scroll_vel *= DECAY.powf(dt);
            ctx.request_repaint(); // keep frames flowing for the whole glide
        } else {
            self.scroll_vel = egui::Vec2::ZERO;
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Catch a panic in the frame and surface it in the status bar instead of crashing.
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.update_inner(ui, frame);
        }));
        if res.is_err() {
            if let Ok(g) = LAST_PANIC.lock() {
                self.last_error = g.clone();
            }
            ctx.request_repaint();
        }
    }
}

impl JustQueryApp {
    fn update_inner(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        // Theme switched since the last painted frame? The switch happens mid-frame (menu code
        // runs before the editor), so the editor re-fills its galley cache against the OLD font
        // atlas during the rest of that frame. egui then notices the changed Visuals
        // `text_options` (light/dark text rendering differs since 0.32) at the START of the next
        // frame and rebuilds Fonts — atlas AND all glyph UVs — wholesale (epaint
        // `Fonts::begin_pass`). Any galley we cached before this point now points into the
        // discarded atlas and renders as garbage, so this is the one correct moment to drop them.
        let cur_theme = theme::current_theme();
        if cur_theme != self.painted_theme {
            self.painted_theme = cur_theme;
            self.line_cache.clear();
        }
        // The window starts hidden: maximize it first (the OS fits it to the work area), then
        // reveal it a few frames later, so it appears already full-size instead of visibly
        // unfolding from a small window.
        if self.startup_frame < 7 {
            if self.startup_frame == 0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            }
            if self.startup_frame == 6 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            }
            self.startup_frame += 1;
            ctx.request_repaint();
        }

        // once the window is up, offer to connect straight away (no connections → "create one")
        if self.startup_frame >= 6 && !self.did_startup_connect {
            self.did_startup_connect = true;
            if !self.connected {
                self.open_connect();
            }
        }

        // keep the OS window title (taskbar / alt-tab) on the active tab's name
        let title = self.cur().map(|t| t.title.clone()).unwrap_or_default();
        if title != self.window_title {
            self.window_title = title.clone();
            let shown = if title.is_empty() { "JustQuery".to_owned() } else { title };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(shown));
        }

        // remember the last known pointer position while it's valid — used by `raw_input_hook` to
        // re-seed egui's pointer after alt-tab (winit doesn't re-send the cursor pos on focus gain,
        // so clicks/scroll stay dead until the mouse moves, even though keyboard already works).
        if let Some(p) = ctx.input(|i| i.pointer.latest_pos()) {
            self.last_pointer = Some(p);
        }

        // Keep frames flowing for a short window after ANY scroll signal. eframe repaints reactively,
        // but a trackpad *flick* delivers inertial events in bursts with gaps — between them no
        // repaint is requested, so the smoothing animation freezes → visible stutter (CPU is ~0.3ms,
        // so this is purely a frame-cadence problem, not a rendering-cost one). A 0.3s grace bridges
        // the gaps so the whole momentum phase animates smoothly.
        let now = ctx.input(|i| i.time);
        let scroll_signal = ctx.input(|i| {
            i.is_scrolling()
                || i.smooth_scroll_delta != egui::Vec2::ZERO
                || i.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. }))
        });
        if scroll_signal {
            self.scroll_active_until = now + 0.3;
        }
        if now < self.scroll_active_until {
            ctx.request_repaint();
        }

        // keep the Scan worker awake while the user is active: ping it on input, throttled to ~2s.
        // When the app is idle there are no input events (and no frames), so no pings arrive and the
        // scanner idles itself after IDLE_SECS — no DB churn while the user is away.
        if self.connected {
            let active = ctx.input(|i| !i.events.is_empty());
            if active {
                let due = self
                    .last_activity_ping
                    .map_or(true, |t| t.elapsed() >= std::time::Duration::from_secs(2));
                if due {
                    if let Some(h) = &self.collector {
                        h.activity();
                    }
                    self.last_activity_ping = Some(std::time::Instant::now());
                }
            }
        }

        // intercept window close while there are unsaved tabs
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
            && self.tabs.iter().any(|t| t.dirty)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(ConfirmAction::ExitApp);
        }

        // progressive, cancellable "fetch all" — per result set. Step in larger, fixed-count
        // chunks (~16 frames) so the grid settles quickly instead of nudging every frame.
        for t in &mut self.tabs {
            for rs in &mut t.results {
                if !rs.loading {
                    continue;
                }
                let step = (rs.rows.len() / 16).max(64);
                rs.visible = (rs.visible + step).min(rs.rows.len());
                if rs.visible >= rs.rows.len() {
                    rs.loading = false;
                }
                ctx.request_repaint();
            }
        }

        // poll each tab's in-flight query thread (tabs run concurrently on their own session
        // connections). Stream result sets / messages into the tab; on Done, reclaim and KEEP the
        // tab's session connection open for reuse. We never move the selected result tab
        // programmatically — execute() parks it on Messages and the user drives it from there.
        for i in 0..self.tabs.len() {
            if self.tabs[i].exec_rx.is_none() {
                continue;
            }
            let mut incoming = Vec::new();
            let mut ready: Option<postgres::CancelToken> = None;
            let mut done: Option<Option<postgres::Client>> = None;
            if let Some(rx) = &self.tabs[i].exec_rx {
                loop {
                    match rx.try_recv() {
                        Ok(ExecMsg::Done(c)) => {
                            done = Some(c.map(|b| *b));
                            break;
                        }
                        Ok(ExecMsg::Ready(tok)) => ready = Some(tok),
                        Ok(m) => incoming.push(m),
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            done = Some(None);
                            break;
                        }
                    }
                }
            }
            let t = &mut self.tabs[i];
            if let Some(tok) = ready {
                t.exec_cancel = Some(tok);
            }
            for m in incoming {
                match m {
                    ExecMsg::Result(rs) => match t.refresh_idx {
                        // single-result Refresh → replace that grid in place
                        Some(ri) if ri < t.results.len() => t.results[ri] = rs,
                        _ => t.results.push(rs),
                    },
                    ExecMsg::Log(entry) => t.log.push(entry),
                    _ => {}
                }
            }
            match done {
                Some(client) => {
                    t.client = client; // reclaim & keep the session connection (None if it failed)
                    t.exec_rx = None;
                    t.exec_cancel = None;
                    t.exec_start = None;
                    t.running = false;
                    t.refresh_idx = None;
                }
                None => ctx.request_repaint(), // still running
            }
        }

        // poll in-flight metadata loads (the object tree + each metadata tab's columns)
        self.poll_metadata(ctx);

        // poll an in-flight Test Connection (runs on a background thread)
        if let Some(rx) = &self.test_rx {
            match rx.try_recv() {
                Ok(res) => {
                    self.test_result = Some(res);
                    self.test_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.test_result = Some(Err("Test thread stopped unexpectedly.".to_owned()));
                    self.test_rx = None;
                }
            }
        }

        // poll the in-flight update check / download (background thread)
        if self.update_rx.is_some() {
            let mut release = false;
            loop {
                let msg = match self.update_rx.as_ref().unwrap().try_recv() {
                    Ok(m) => m,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint();
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        release = true;
                        break;
                    }
                };
                match msg {
                    update::UpdateMsg::CheckDone(Ok(r)) => {
                        // remember the verdict for the chip (in-memory only, not persisted)
                        self.update_outdated = Some(r.is_newer);
                        self.update_status = if r.is_newer {
                            update::UpdateStatus::Available { latest: r.latest_tag }
                        } else {
                            update::UpdateStatus::Latest
                        };
                        release = true;
                        break;
                    }
                    update::UpdateMsg::CheckDone(Err(e)) => {
                        self.update_status =
                            update::UpdateStatus::Error { msg: e, retry_download: false };
                        release = true;
                        break;
                    }
                    update::UpdateMsg::Progress { done, total } => {
                        self.update_status = update::UpdateStatus::Downloading { done, total };
                    }
                    update::UpdateMsg::Applying => {
                        self.update_status = update::UpdateStatus::Applying;
                    }
                    update::UpdateMsg::Applied => {
                        self.update_status = update::UpdateStatus::PendingRestart;
                        release = true;
                        break;
                    }
                    update::UpdateMsg::Failed(e) => {
                        self.update_status =
                            update::UpdateStatus::Error { msg: e, retry_download: true };
                        release = true;
                        break;
                    }
                }
            }
            if release {
                self.update_rx = None;
            }
        }

        // poll an in-flight main connection (background thread) → live client or an error modal
        if let Some(rx) = &self.connect_rx {
            match rx.try_recv() {
                Ok(Ok(client)) => {
                    self.main_conn = Some(client);
                    self.connected = true;
                    self.conn_broken = false;
                    self.active_label = std::mem::take(&mut self.pending_label);
                    self.connect_rx = None;
                    self.connect_open = false; // success → close the Connect dialog
                    self.start_meta_actors(); // begin background metadata collection
                }
                Ok(Err(msg)) => {
                    // surface the failure inside the Connect modal (reopen it) instead of a
                    // separate error modal
                    self.connect_error = Some(msg);
                    self.connect_open = true;
                    self.connect_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.connect_error = Some("Connection thread stopped unexpectedly.".to_owned());
                    self.connect_open = true;
                    self.connect_rx = None;
                }
            }
        }

        self.main_screen(ui);

        if self.confirm.is_some() {
            self.confirm_modal(ctx);
        }
        self.about_modal(ctx);
        self.scan_modal(ctx);
        self.connect_modal(ctx);
        self.disconnect_modal(ctx);
        self.no_conn_modal(ctx);
        self.conflict_modal(ctx);
        self.conn_test_modal(ctx);
        self.busy_modal(ctx);
        self.connecting_modal(ctx);
        self.error_modal_box(ctx);

        // window edge-resize handles + our own 1px border (OS chrome is off)
        resize_handles(ctx);
        window_border(ctx);
    }
}

// ============================================================
// Main screen
// ============================================================
impl JustQueryApp {
    fn main_screen(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        self.handle_shortcuts(ctx);
        // full-width chrome first (caption + toolbar on top, status on the bottom)…
        self.titlebar(ui);
        self.icon_toolbar(ui);
        self.statusbar(ui);
        // The work area now sits flush against the status bar — the editor / managers run right
        // down to the bar with no chrome gutter between them.
        // …then the left dock claims the work area's left edge, pushing the tabs/editor right.
        // A pending inline rename only makes sense while the Connection Manager is showing; if the
        // dock switched away (or closed), drop it so it doesn't reappear stuck on return.
        if self.dbmgr_rename.is_some() && self.left_panel != Some(LeftPanel::Database) {
            self.dbmgr_rename = None;
            self.dbmgr_conflict = None;
            self.conn_rename_armed = None;
        }
        // Only one of these renders per frame (each early-returns unless it owns the dock).
        self.database_manager_panel(ui);
        self.metadata_manager_panel(ui);
        self.tabbar(ui);
        // editor work-area toolbar — a chrome strip under the tabs (only for SQL tabs)
        self.editor_toolbar_bar(ui);
        // result panel lives with the active tab — only when it has been executed
        if self.show_result && self.cur().map_or(false, |t| t.executed) {
            self.result_panel(ui);
        }
        self.editor(ui);
        self.find_bar(ctx);
    }

    /// Global editor shortcuts (file ops + find). Consumes the keys so widgets don't also act.
    ///
    /// Word navigation (Ctrl+←/→, Ctrl+Shift+←/→) is left to egui: since 0.34 its word boundary
    /// is Unicode-aware (`is_word_char` = `c.is_alphanumeric() || c == '_'`, over
    /// `split_word_bound_indices`), so Cyrillic etc. move word-by-word natively — no custom
    /// interception needed (it was an ASCII-only-egui workaround).
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        if ctx.input_mut(|i| i.consume_key(cmd, Key::N)) {
            self.new_tab();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, Key::O)) {
            self.open_file();
        }
        // consume the key regardless so it never leaks to the editor, but only act when there's
        // something to save (mirrors the disabled toolbar / menu Save)
        if ctx.input_mut(|i| i.consume_key(cmd_shift, Key::S)) && self.can_save() {
            self.save_active_as();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, Key::S)) && self.can_save() {
            self.save_active();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, Key::F)) {
            self.open_find();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F8)) {
            self.execute(ctx);
        }
        // F5 → format the active SQL tab in the house style (refuses on a rule violation)
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F5)) {
            self.format_active();
        }
        // F6 → open the completion popup (built in `editor` where the live caret is known)
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F6)) {
            self.ac.request = true;
            self.focus_editor = true;
        }
        // next / previous match — work whenever there's a query (even with the dialog closed):
        // Ctrl+. / Ctrl+, (the > / < keys). (F3 intentionally not bound.)
        if !self.find_query.is_empty() {
            let next = ctx.input_mut(|i| {
                i.consume_key(cmd, Key::Period) || i.consume_key(cmd_shift, Key::Period)
            });
            if next {
                self.find_step(false);
            }
            let prev = ctx.input_mut(|i| {
                i.consume_key(cmd, Key::Comma) || i.consume_key(cmd_shift, Key::Comma)
            });
            if prev {
                self.find_step(true);
            }
        }
        if self.find_open && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.close_find();
        }
    }

    /// Window caption: logo + text menus + centered connection string + window buttons.
    fn titlebar(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        egui::Panel::top("titlebar")
            .frame(caption_frame())
            .exact_size(CAPTION_H)
            .show_separator_line(false)
            .show_inside(ui, |ui| {
                // empty caption areas drag the window; double-click (un)maximizes
                enable_window_drag(ui, ctx);
                let full = ui.max_rect();

                // menu labels: text stays TEXT; the accent is a darker background box on
                // hover, kept (darker still) while the menu is open / pressed
                {
                    let s = ui.style_mut();
                    s.visuals.override_text_color = None;
                    let w = &mut s.visuals.widgets;
                    w.inactive.weak_bg_fill = if DIAG_BOXES { p().acc_bg } else { Color32::TRANSPARENT };
                    w.inactive.bg_stroke = Stroke::NONE;
                    w.inactive.fg_stroke = Stroke::new(1.0, p().text);
                    w.inactive.corner_radius = CornerRadius::ZERO;
                    w.hovered.weak_bg_fill = p().acc_bg;
                    w.hovered.bg_stroke = Stroke::NONE;
                    w.hovered.fg_stroke = Stroke::new(1.0, p().text);
                    w.hovered.corner_radius = CornerRadius::ZERO;
                    w.active.weak_bg_fill = p().acc_bg2;
                    w.active.bg_stroke = Stroke::NONE;
                    w.active.fg_stroke = Stroke::new(1.0, p().text);
                    w.active.corner_radius = CornerRadius::ZERO;
                    w.open.weak_bg_fill = p().acc_bg2;
                    w.open.bg_stroke = Stroke::NONE;
                    w.open.fg_stroke = Stroke::new(1.0, p().text);
                    w.open.corner_radius = CornerRadius::ZERO;
                }

                let mut menu_end = full.left() + 220.0;
                ui.horizontal_centered(|ui| {
                    // tight menu row: small gap between items, modest box padding. The vertical
                    // padding makes the menu-item boxes inset by ~CHROME_PAD, lining their blank
                    // gap up with the toolbar/tab boxes below.
                    ui.spacing_mut().item_spacing.x = 2.0;
                    // match the system's action buttons (primary/secondary): 14px side padding,
                    // 4px vertical → the same ~CONTROL_H box height
                    ui.spacing_mut().button_padding = Vec2::new(14.0, 4.0);
                    logo(ui, 18.0);
                    ui.add_space(8.0);
                    // one menu row: label + right-aligned shortcut; returns whether it was clicked
                    let item_en = |ui: &mut egui::Ui, label: &str, shortcut: &str, enabled: bool| -> bool {
                        let mut btn = egui::Button::new(label);
                        if !shortcut.is_empty() {
                            btn = btn.shortcut_text(shortcut);
                        }
                        let clicked = ui.add_enabled(enabled, btn).clicked();
                        if clicked {
                            ui.close();
                        }
                        clicked
                    };
                    let item = |ui: &mut egui::Ui, label: &str, shortcut: &str| -> bool {
                        item_en(ui, label, shortcut, true)
                    };
                    let can_save = self.can_save();
                    // collect each top-level button so we can implement menu-bar roll-over below
                    let mut menu_btns: Vec<(egui::Response, bool)> = Vec::new();
                    for m in ["File", "Edit", "Search", "Database", "Tools", "Window", "Help"] {
                        let mb = ui.menu_button(RichText::new(m).size(13.0), |ui| {
                            // bigger, darker hover/active background for dropdown rows
                            ui.spacing_mut().button_padding = Vec2::new(12.0, 6.0);
                            ui.spacing_mut().item_spacing.y = 0.0; // tight rows; separators keep the logical blocks apart
                            {
                                // menu rows are neutral hover-pills, 7px radius (Design System v2 §6)
                                let w = &mut ui.style_mut().visuals.widgets;
                                w.hovered.weak_bg_fill = p().hover;
                                w.hovered.bg_stroke = Stroke::NONE;
                                w.hovered.corner_radius = CornerRadius::same(RADIUS_CONTROL);
                                w.active.weak_bg_fill = p().acc_bg2;
                                w.active.bg_stroke = Stroke::NONE;
                                w.active.corner_radius = CornerRadius::same(RADIUS_CONTROL);
                            }
                            match m {
                            "File" => {
                                if item(ui, "New SQL Window", "Ctrl+N") {
                                    self.new_tab();
                                }
                                if item(ui, "Open SQL File…", "Ctrl+O") {
                                    self.open_file();
                                }
                                ui.separator();
                                if item_en(ui, "Save", "Ctrl+S", can_save) {
                                    self.save_active();
                                }
                                if item_en(ui, "Save As…", "Ctrl+Shift+S", can_save) {
                                    self.save_active_as();
                                }
                                ui.separator();
                                if item(ui, "Close Tab", "Ctrl+W") && !self.tabs.is_empty() {
                                    self.request_close_tab(self.active_tab);
                                }
                                ui.separator();
                                if item(ui, "Exit", "Alt+F4") {
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                            }
                            "Edit" => {
                                if item(ui, "Undo", "Ctrl+Z") {
                                    if let Some(t) = self.ed_active_mut() {
                                        if t.ed_undo() {
                                            t.dirty = true;
                                        }
                                    }
                                    self.focus_editor = true;
                                }
                                if item(ui, "Redo", "Ctrl+Shift+Z") {
                                    if let Some(t) = self.ed_active_mut() {
                                        if t.ed_redo() {
                                            t.dirty = true;
                                        }
                                    }
                                    self.focus_editor = true;
                                }
                                ui.separator();
                                if item(ui, "Cut", "Ctrl+X") {
                                    if let Some(t) = self.ed_active_mut() {
                                        if let Some(s) = t.ed_cut() {
                                            ctx.copy_text(s);
                                            t.dirty = true;
                                        }
                                    }
                                    self.focus_editor = true;
                                }
                                if item(ui, "Copy", "Ctrl+C") {
                                    if let Some(s) = self.cur().and_then(|t| t.ed_copy()) {
                                        ctx.copy_text(s);
                                    }
                                }
                                if item(ui, "Paste", "Ctrl+V") {
                                    if let Some(txt) = dialog::clipboard_text() {
                                        if let Some(t) = self.ed_active_mut() {
                                            t.ed_paste(&txt);
                                            t.dirty = true;
                                        }
                                        self.focus_editor = true;
                                    }
                                }
                                ui.separator();
                                if item(ui, "Select All", "Ctrl+A") {
                                    self.editor_select_all();
                                }
                            }
                            "Search" => {
                                if item(ui, "Find…", "Ctrl+F") {
                                    self.open_find();
                                }
                                // step through matches without opening the find bar
                                if item(ui, "Find Next", "Ctrl+>") {
                                    self.find_step(false);
                                }
                                if item(ui, "Find Previous", "Ctrl+<") {
                                    self.find_step(true);
                                }
                                item(ui, "Replace…", "Ctrl+H");
                                ui.separator();
                                item(ui, "Go to Line…", "Ctrl+G");
                            }
                            "Database" => {
                                if item(ui, "Connect…", "") {
                                    self.open_connect();
                                }
                                if item(ui, "Disconnect", "") && self.connected {
                                    self.disconnect_confirm = true; // never disconnect silently
                                }
                                ui.separator();
                                item(ui, "Commit", "");
                                item(ui, "Rollback", "");
                            }
                            "Tools" => {
                                item(ui, "Execute", "F8");
                                item(ui, "Stop", "Esc");
                                ui.separator();
                                if item(ui, "Format SQL", "F5") {
                                    self.format_active();
                                }
                                item(ui, "Export Result…", "");
                                ui.separator();
                                // Appearance: Light / Dark radio pair (the check marks the active one)
                                let cur = theme::current_theme();
                                let mut switch_to: Option<theme::AppTheme> = None;
                                ui.menu_button("Appearance", |ui| {
                                    ui.spacing_mut().button_padding = Vec2::new(12.0, 6.0);
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    let mut pick = |ui: &mut egui::Ui, label, t: theme::AppTheme| {
                                        let mark = if cur == t { "●" } else { " " };
                                        if item(ui, label, mark) && cur != t {
                                            switch_to = Some(t);
                                        }
                                    };
                                    pick(ui, "Light", theme::AppTheme::Light);
                                    pick(ui, "Dark", theme::AppTheme::Dark);
                                });
                                if let Some(t) = switch_to {
                                    theme::set_theme(ctx, t);
                                    save_theme(t);
                                    // NOTE: do NOT clear line_cache here — the editor repaints
                                    // (and re-caches against the old font atlas) later this same
                                    // frame. update_inner drops the cache on the next frame,
                                    // after egui has rebuilt the atlas for the new theme.
                                }
                                ui.separator();
                                item(ui, "Preferences…", "");
                            }
                            "Window" => {
                                item(ui, "Next Tab", "Ctrl+Tab");
                                item(ui, "Previous Tab", "Ctrl+Shift+Tab");
                                ui.separator();
                                let toggle = if self.show_result {
                                    "Hide Result Panel"
                                } else {
                                    "Show Result Panel"
                                };
                                if item(ui, toggle, "F4") {
                                    self.show_result = !self.show_result;
                                }
                                ui.separator();
                                item(ui, "Close All Tabs", "");
                            }
                            "Help" => {
                                item(ui, "Documentation", "F1");
                                item(ui, "Keyboard Shortcuts", "");
                                ui.separator();
                                if item(ui, "About JustQuery", "") {
                                    self.open_about_tab();
                                }
                            }
                            _ => {}
                            }
                        });
                        menu_btns.push((mb.response, mb.inner.is_some()));
                    }
                    // Menu-bar roll-over: egui's top-level MenuButton only opens on click. Once ANY
                    // menu is open, hovering a different top-level button switches to it (open_id
                    // opens that popup and closes the rest), so the menus behave like a desktop
                    // menu bar — drag across File/Edit/… and the dropdown follows the cursor.
                    if menu_btns.iter().any(|(_, open)| *open) {
                        for (resp, open) in &menu_btns {
                            if !*open && resp.hovered() {
                                egui::Popup::open_id(ctx, resp.id.with("popup"));
                            }
                        }
                    }
                    menu_end = ui.min_rect().right();
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        caption_buttons(ui, ctx);
                    });
                });

                // window "title": the active tab's name, centered between the menu end and the
                // window buttons (empty when no tab is open)
                let controls_w = 3.0 * 40.0; // close / max / minimize
                let zone_left = menu_end + 12.0;
                let zone_right = full.right() - controls_w - 8.0;
                let cx = (zone_left + zone_right) * 0.5;
                let cy = full.center().y;
                if let Some(title) = self.cur().map(|t| t.title.clone()).filter(|s| !s.is_empty()) {
                    ui.painter().text(
                        egui::pos2(cx, cy),
                        egui::Align2::CENTER_CENTER,
                        title,
                        egui::FontId::proportional(13.0),
                        p().text_dim,
                    );
                }
            });
    }

    /// Icon toolbar (below the caption): global quick actions only (file ops, connect, the
    /// Database Manager toggle). Area-specific actions live in each work area's own sub-toolbar.
    fn icon_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("icontoolbar")
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 0,
                bottom: 0,
            }))
            .exact_size(CAPTION_H)
            .show_separator_line(false) // caption + toolbar are one block, no line below
            .show_inside(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    // global file actions
                    if qbtn(ui, ic::NEW, p().text, "New tab").clicked() {
                        self.new_tab();
                    }
                    if qbtn(ui, ic::OPEN, p().text, "Open").clicked() {
                        self.open_file();
                    }
                    if self.can_save() {
                        if qbtn(ui, ic::SAVE, p().text, "Save").clicked() {
                            self.save_active();
                        }
                    } else {
                        qbtn_off(ui, ic::SAVE, "Nothing to save");
                    }
                    // divider, then the connection TOGGLE. The glyph shows the ACTION a click
                    // performs, not the state (play/pause convention): offline → plug («Connect…»),
                    // connected → plug-off («Disconnect»). Always full-strength `text` (one of the
                    // two actions is always live), like the file icons — never dimmed.
                    toolbar_divider(ui);
                    if self.connected {
                        if qbtn(ui, icons::PLUG_OFF, p().text, "Disconnect").clicked() {
                            self.disconnect_confirm = true;
                        }
                    } else if qbtn(ui, icons::PLUG, p().text, "Connect…").clicked() {
                        self.open_connect();
                    }
                    // (Execute / Stop / Commit / Rollback now live in the editor's work-area
                    // toolbar; Refresh / Fetch in the result panel's; the New-connection "+" in
                    // the Database Manager's — each action sits with the area it acts on.)
                    // Left-dock toggles — at the tail of the toolbar, after a divider. Only one
                    // manager shows at a time; clicking the active one closes the dock.
                    toolbar_divider(ui);
                    let db_on = self.left_panel == Some(LeftPanel::Database);
                    if qbtn_toggle(ui, ic::MANAGER, db_on, "Connection Manager").clicked() {
                        self.left_panel = if db_on { None } else { Some(LeftPanel::Database) };
                    }
                    let meta_on = self.left_panel == Some(LeftPanel::Metadata);
                    if qbtn_toggle(ui, ic::META, meta_on, "Metadata Manager").clicked() {
                        self.left_panel = if meta_on { None } else { Some(LeftPanel::Metadata) };
                    }
                });
            });
    }

    fn tabbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("tabs")
            // bottom margin 0: the active-tab underline sits flush against the editor sheet
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(egui::Margin {
                left: 6,
                right: 6,
                top: 0,
                bottom: 0,
            }))
            .exact_size(TABBAR_H)
            .show_separator_line(false)
            .show_inside(ui, |ui| {
                ui.style_mut().visuals.override_text_color = None;
                let running: Vec<bool> = self.tabs.iter().map(|t| t.running).collect();
                let labels: Vec<String> = self
                    .tabs
                    .iter()
                    .map(|t| {
                        if t.dirty {
                            format!("{} *", t.title)
                        } else {
                            t.title.clone()
                        }
                    })
                    .collect();
                ui.horizontal_centered(|ui| {
                    // reserve room for the ‹ › buttons on the right only when tabs overflow
                    let row_h = ui.max_rect().height();
                    let arrows_w = if self.tab_overflow { 56.0 } else { 0.0 };
                    let scroll_w = (ui.available_width() - arrows_w).max(0.0);
                    let out = ui
                        .allocate_ui(Vec2::new(scroll_w, row_h), |ui| {
                            egui::ScrollArea::horizontal()
                                .auto_shrink([false, false])
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                )
                                .show(ui, |ui| {
                                    if self.tab_scroll != 0.0 {
                                        ui.scroll_with_delta(Vec2::new(self.tab_scroll, 0.0));
                                        self.tab_scroll = 0.0;
                                    }
                                    ui.horizontal_centered(|ui| {
                                        let (select, close) =
                                            tab_strip(ui, &labels, self.active_tab, true, Some(&running));
                                        if let Some(i) = select {
                                            if i != self.active_tab {
                                                self.active_tab = i;
                                                self.focus_editor = true;
                                            }
                                        }
                                        if let Some(i) = close {
                                            self.request_close_tab(i);
                                        }
                                        // empty space: double-click to open a new tab
                                        let empty = ui.allocate_response(
                                            ui.available_size(),
                                            egui::Sense::click(),
                                        );
                                        if empty.double_clicked() {
                                            self.new_tab();
                                        }
                                        empty.on_hover_text("Double-click to add a tab");
                                    });
                                })
                        })
                        .inner;
                    // overflow when the content is wider than the viewport
                    self.tab_overflow = out.content_size.x > out.inner_rect.width() + 1.0;
                    // ‹ › scroll buttons on the right (only shown while overflowing)
                    if self.tab_overflow {
                        if qchevron(ui, true, "Scroll tabs left").clicked() {
                            self.tab_scroll = 90.0;
                        }
                        if qchevron(ui, false, "Scroll tabs right").clicked() {
                            self.tab_scroll = -90.0;
                        }
                    }
                });
            });
    }

    /// Inner margin for content islands (editor / result panel / tab pages): the uniform
    /// 8px gutter law (Design Delta v2.2 §7) without doubling — 4px against an open dock
    /// (the dock contributes its own 4), 8px against the window edge.
    pub(crate) fn island_margin(&self) -> Margin {
        Margin {
            left: if self.left_panel.is_some() { 4 } else { 8 },
            right: 8,
            top: 1,
            bottom: 0,
        }
    }

    fn statusbar(&mut self, ui: &mut egui::Ui) {
        // Plain bottom strip — no frame, no island, no top divider line: just the status texts on
        // the chrome fill, vertically centred. scan / version carry no chip background.
        egui::Panel::bottom("status")
            .exact_size(24.0)
            // right margin = the editor island's right margin (8) so the chips line up exactly with
            // the editor's right border, not a few px short of it
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                left: 10,
                right: 8,
                top: 0,
                bottom: 0,
            }))
            .show_separator_line(false)
            .show_inside(ui, |ui| {
                let sz = 12.0;
                ui.horizontal_centered(|ui| {
                    // The right group is the OUTER, full-width right_to_left so it hugs the far-right
                    // edge; the left status labels fill the remaining space in a nested left_to_right.
                    // version · connection · scan — the scan/connection chips and their separators
                    // only exist while connected.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 3.0; // tight: scan · login@conn · version
                        self.version_chip(ui, sz); // rightmost — links to the About/version page
                        if self.connected || self.conn_broken {
                            ui.label(RichText::new("·").size(sz).color(p().disabled));
                            self.conn_chip(ui, sz);
                        }
                        if self.connected {
                            ui.label(RichText::new("·").size(sz).color(p().disabled));
                            self.meta_status_indicator(ui, sz);
                        }
                        // LEFT — editor status: caret position + encoding (SQL tabs), then any
                        // transient editor message (validation / panic / running timer / row count)
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            if let Some(t) = self.cur().filter(|t| t.is_sql()) {
                                let eol = if t.sql.contains("\r\n") { "CRLF" } else { "LF" };
                                ui.label(
                                    RichText::new(format!("Ln {}, Col {}", self.cursor_ln, self.cursor_col))
                                        .size(sz)
                                        .color(p().text),
                                );
                                ui.label(RichText::new("·").size(sz).color(p().disabled));
                                ui.label(RichText::new("UTF-8").size(sz).color(p().text));
                                ui.label(RichText::new("·").size(sz).color(p().disabled));
                                ui.label(RichText::new(eol).size(sz).color(p().text));
                            }
                            // separator between the caret/encoding block and any transient message
                            let sql_tab = self.cur().map_or(false, |t| t.is_sql());
                            let has_msg = self.last_error.is_some()
                                || self.cur().and_then(|t| t.exec_start).is_some()
                                || self.fmt_status.is_some()
                                || (self.show_result && self.cur_result().is_some());
                            if sql_tab && has_msg {
                                ui.label(RichText::new("·").size(sz).color(p().disabled));
                            }
                            if let Some(err) = self.last_error.clone() {
                                let line = err.lines().next().unwrap_or("error").to_owned();
                                ui.label(RichText::new(line).size(sz).color(p().danger));
                            } else if let Some(start) = self.cur().and_then(|t| t.exec_start) {
                                // transient messages are quiet text_dim — green is reserved for the
                                // connection identity
                                ui.label(
                                    RichText::new(format!("Running…  {}", fmt_elapsed(start.elapsed())))
                                        .size(sz)
                                        .color(p().text_dim),
                                );
                            } else if let Some((msg, is_err)) = self.fmt_status.clone() {
                                let color = if is_err { p().danger } else { p().text_dim };
                                ui.label(RichText::new(msg).size(sz).color(color));
                            } else if self.show_result {
                                if let Some(rs) = self.cur_result() {
                                    let n = rs.visible.min(rs.rows.len());
                                    let word = if n == 1 { "row" } else { "rows" };
                                    let more = if n < rs.rows.len() { " (more…)" } else { "" };
                                    ui.label(
                                        RichText::new(format!("{n} {word}{more}")).size(sz).color(p().text),
                                    );
                                }
                            }
                        });
                    });
                });
            });
    }

    fn result_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        let max_h = (ctx.content_rect().height() - 180.0).max(160.0);
        // height lives with the active tab, so resizing one tab's result panel doesn't
        // bleed into the others
        let mut rh = self.cur().map_or(300.0, |t| t.result_height).clamp(120.0, max_h);
        // maximize state also lives with the tab, so one tab's full-screen result doesn't
        // carry over to another
        let mut full = self.cur().map_or(false, |t| t.result_full);
        // full mode: fill exactly the remaining work area (above the status bar + bottom border),
        // so the editor collapses and the result never overlaps the status bar.
        let panel_h = if full {
            ui.available_height()
        } else {
            rh
        };
        egui::Panel::bottom("result")
            .resizable(false)
            .exact_size(panel_h)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p().panel2))
            .show_inside(ui, |ui| {
                // one compact bar: tabs (left) · resize grab (fill) · maximize · close (right)
                egui::Panel::top("result_bar")
                    .exact_size(TABBAR_H)
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: 6,
                        right: 6,
                        top: 0,
                        bottom: 0,
                    }))
                    .show_inside(ui, |ui| {
                        ui.style_mut().visuals.override_text_color = None;
                        let active_rt = self.cur().map_or(0, |t| t.result_tab);
                        let mut do_close = false;
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if close_x(ui, 22.0, 4.0, "Close") {
                                do_close = true;
                            }
                            let (icon, tip) = if full {
                                (ic::COLLAPSE, "Restore result panel")
                            } else {
                                (ic::EXPAND, "Maximize result panel")
                            };
                            if qbtn(ui, icon, p().text, tip).clicked() {
                                full = !full;
                            }
                            // remaining space (left→right): tabs, then the resize grab. The panel
                            // action icons (chevrons/close) already reserved their fixed zone on
                            // the right; the tab lane is CLIPPED to what's left and fades out when
                            // it overflows — tabs can never slide under the icons (Delta v2.1 §5).
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                let names = self.result_tab_names();
                                let lane = ui.available_rect_before_wrap();
                                let mut lane_ui = ui.new_child(
                                    egui::UiBuilder::new()
                                        .max_rect(lane)
                                        .layout(Layout::left_to_right(Align::Center)),
                                );
                                lane_ui.set_clip_rect(lane.intersect(ui.clip_rect()));
                                let (sel, _) = tab_strip(&mut lane_ui, &names, active_rt, false, None);
                                let used = lane_ui.min_rect().width();
                                if used > lane.width() {
                                    // fade the lane's right edge into the chrome (transparent → panel2)
                                    let fw = 28.0_f32.min(lane.width());
                                    let fr = egui::Rect::from_min_max(
                                        egui::pos2(lane.right() - fw, lane.top()),
                                        lane.right_bottom(),
                                    );
                                    let mut mesh = egui::Mesh::default();
                                    let c0 = egui::Color32::TRANSPARENT;
                                    let c1 = p().panel2;
                                    let i0 = mesh.vertices.len() as u32;
                                    for (pos, c) in [
                                        (fr.left_top(), c0),
                                        (fr.right_top(), c1),
                                        (fr.right_bottom(), c1),
                                        (fr.left_bottom(), c0),
                                    ] {
                                        mesh.vertices.push(egui::epaint::Vertex {
                                            pos,
                                            uv: egui::epaint::WHITE_UV,
                                            color: c,
                                        });
                                    }
                                    mesh.indices.extend([i0, i0 + 1, i0 + 2, i0, i0 + 2, i0 + 3]);
                                    ui.painter().add(egui::Shape::mesh(mesh));
                                }
                                // advance the parent past the lane content (capped at the lane)
                                ui.allocate_rect(
                                    egui::Rect::from_min_size(
                                        lane.min,
                                        Vec2::new(used.min(lane.width()), lane.height()),
                                    ),
                                    egui::Sense::hover(),
                                );
                                if let Some(i) = sel {
                                    self.grid_sel = None; // selection belongs to the old grid
                                    if let Some(t) = self.cur_mut() {
                                        if i != t.result_tab {
                                            // just switch — each result keeps its own revealed-row
                                            // count, so coming back shows where you left off
                                            t.result_tab = i;
                                        }
                                    }
                                }
                                // the whole remaining line is the resize handle (no dotted grip)
                                let row_h = ui.max_rect().height();
                                let (_hrect, hresp) = ui.allocate_exact_size(
                                    Vec2::new(ui.available_width().max(0.0), row_h),
                                    egui::Sense::drag(),
                                );
                                if !full && (hresp.hovered() || hresp.dragged()) {
                                    ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical);
                                }
                                if hresp.dragged() && !full {
                                    rh = (rh - hresp.drag_delta().y).clamp(120.0, max_h);
                                }
                            });
                        });
                        if do_close {
                            self.show_result = false;
                            full = false;
                        }
                    });

                // result work-area toolbar — a chrome strip under the result tabs (same beige as
                // the surrounding chrome, no fill or border of its own)
                egui::Panel::top("result_toolbar_bar")
                    .exact_size(SUBBAR_H)
                    .show_separator_line(false)
                    // top:2 — same seam compensation as the editor toolbar (see editor_toolbar_bar)
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: 6,
                        right: 6,
                        top: 2,
                        bottom: 0,
                    }))
                    .show_inside(ui, |ui| {
                        ui.horizontal_centered(|ui| self.result_toolbar(ui));
                    });

                // kill the vertical item-spacing so the gap below is exactly our 1px margin
                ui.spacing_mut().item_spacing.y = 0.0;
                // body — table inside the 6px side borders, with a 1px gap under the toolbar
                egui::Frame::new()
                    .inner_margin(self.island_margin())
                    .show(ui, |ui| {
                        island(ui, |ui| self.result_table(ui));
                    });
            });
        // persist the (possibly dragged) height + maximize state back onto the active tab
        if let Some(t) = self.cur_mut() {
            t.result_height = rh;
            t.result_full = full;
        }
    }

    /// Result work-area toolbar icons (Refresh / Fetch next / Fetch all). Sits in a chrome strip
    /// under the result tabs; enabled per the active result tab's state. Caller wraps it in a
    /// horizontal layout.
    fn result_toolbar(&mut self, ui: &mut egui::Ui) {
        let is_data = self.is_data_tab();
        let executed = self.cur().map_or(false, |t| t.executed);
        let visible = self.cur_result().map_or(0, |r| r.visible);
        let loading = self.cur_result().map_or(false, |r| r.loading);
        let total = self.cur_total();
        let running = self.cur().map_or(false, |t| t.exec_rx.is_some());
        ui.spacing_mut().item_spacing.x = 2.0;
        // Refresh — re-run ONLY this result's statement; only on a result tab (not Messages)
        if is_data && self.connected && !running {
            if qbtn_sm(ui, ic::REFRESH, p().text, "Refresh this result").clicked() {
                let ctx = ui.ctx().clone();
                self.refresh_current_result(&ctx);
            }
        } else {
            let why = if !is_data {
                "Refresh (open a result tab)"
            } else if running {
                "Refresh (a query is running)"
            } else {
                "Refresh (connect first)"
            };
            qbtn_off_sm(ui, ic::REFRESH, why);
        }
        let more = is_data && executed && visible < total && !loading;
        if more {
            if qbtn_sm(ui, ic::FETCH_NEXT, p().text, "Fetch next page").clicked() {
                self.fetch_more();
            }
        } else {
            qbtn_off_sm(ui, ic::FETCH_NEXT, "Fetch next page");
        }
        if more {
            if qbtn_sm(ui, ic::FETCH_ALL, p().text, "Fetch all rows").clicked() {
                self.fetch_all_toggle();
            }
        } else {
            qbtn_off_sm(ui, ic::FETCH_ALL, "Fetch all rows");
        }
    }

    /// Editor work-area toolbar: a chrome strip (same beige as the surrounding chrome — no fill or
    /// border of its own) under the tabs, holding the editor's icons. Shown only for SQL tabs.
    fn editor_toolbar_bar(&mut self, ui: &mut egui::Ui) {
        if !self.is_sql_tab() {
            return;
        }
        let ctx = &ui.ctx().clone();
        egui::Panel::top("editor_toolbar")
            .exact_size(SUBBAR_H)
            .show_separator_line(false)
            // top:2 compensates for the work-area sheet's top seam below (1px frame margin + 1px
            // border): without it the centered icon row reads a hair high in the tabs↔sheet band
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                left: 6,
                right: 6,
                top: 2,
                bottom: 0,
            }))
            .show_inside(ui, |ui| {
                ui.horizontal_centered(|ui| self.editor_toolbar(ui, ctx));
            });
    }

    /// Editor work-area toolbar icons (Execute / Stop / Commit / Rollback). Identical for every
    /// SQL tab but enabled/disabled per the active tab's state (tabs run independently on their
    /// own session connections).
    fn editor_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.spacing_mut().item_spacing.x = 2.0;
        // Execute needs a SQL tab + a live connection + some SQL + this tab not already running.
        let active_running = self.cur().map_or(false, |t| t.exec_rx.is_some());
        let has_sql = self.cur().map_or(false, |t| !t.sql.trim().is_empty());
        if self.is_sql_tab() && self.connected && !active_running && has_sql {
            // Run is THE action of the whole loop — green when armed (go!)
            if qbtn_sm(ui, ic::PLAY, p().ok, "Execute selection / all (F8)").clicked() {
                self.execute(ctx);
            }
        } else {
            let why = if !self.connected {
                "Execute (connect first)"
            } else if active_running {
                "Execute (a query is already running on this tab)"
            } else if !has_sql {
                "Execute (the editor is empty)"
            } else {
                "Execute"
            };
            qbtn_off_sm(ui, ic::PLAY, why);
        }
        // Stop — cancel THIS tab's running query, or stop a fetch-all reveal if one is in progress
        let fetching = self.cur_result().map_or(false, |r| r.loading);
        if active_running || fetching {
            let tip = if active_running { "Stop query" } else { "Stop loading" };
            if qbtn_sm(ui, ic::STOP, p().danger, tip).clicked() {
                if active_running {
                    self.cancel_running_query();
                } else if let Some(rs) = self.cur_result_mut() {
                    rs.loading = false;
                }
            }
        } else {
            qbtn_off_sm(ui, ic::STOP, "Stop (disabled)");
        }
        // Commit / Rollback — only inside an open transaction
        if self.in_transaction() {
            qbtn_sm(ui, ic::COMMIT, p().text, "Commit");
            qbtn_sm(ui, ic::ROLLBACK, p().text, "Rollback");
        } else {
            qbtn_off_sm(ui, ic::COMMIT, "Commit (no transaction)");
            qbtn_off_sm(ui, ic::ROLLBACK, "Rollback (no transaction)");
        }
        // divider, then SQL tooling: Validate (house rules) + Format (F5). Enabled when there's SQL.
        ui.add_space(4.0);
        ui.label(RichText::new("|").size(14.0).color(p().disabled));
        ui.add_space(4.0);
        if has_sql {
            if qbtn_sm(ui, ic::VALIDATE, p().text, "Validate against the house rules").clicked() {
                self.validate_active();
            }
            if qbtn_sm(ui, ic::FORMAT, p().text, "Format SQL (F5)").clicked() {
                self.format_active();
            }
        } else {
            qbtn_off_sm(ui, ic::VALIDATE, "Validate (the editor is empty)");
            qbtn_off_sm(ui, ic::FORMAT, "Format (the editor is empty)");
        }
    }

    fn result_table(&mut self, ui: &mut egui::Ui) {
        // fill the whole island (so the Messages view isn't a tiny box)
        ui.set_min_size(ui.available_size());
        if !self.cur().map_or(false, |t| t.executed) {
            ui.vertical_centered(|ui| {
                ui.add_space(34.0);
                ui.colored_label(p().text_dim, "Press ▶ Execute to get results");
            });
            return;
        }
        let sel = self.grid_sel;
        let (new_sel, copy, reorder, resize) = if self.cur().map_or(true, |t| t.result_tab == 0) {
            // Messages tab — the execution log as a grid (Time / Status / Exec / Fetch / Rows /
            // Message / SQL), rendered through the same grid as result sets
            let cols = ["Time", "Status", "Exec", "Fetch", "Rows", "Message", "SQL/Command"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
            let mut rows: Vec<Vec<String>> = self.cur().map_or_else(Vec::new, |t| {
                t.log
                    .iter()
                    .map(|e| {
                        vec![
                            e.time.clone(),
                            e.status.clone(),
                            format!("{:.3}", e.exec),
                            format!("{:.3}", e.fetch),
                            if e.rows < 0 { "—".to_owned() } else { e.rows.to_string() },
                            e.message.clone(),
                            e.sql.replace('\n', " "),
                        ]
                    })
                    .collect()
            });
            if self.cur().map_or(false, |t| t.running) {
                rows.push(vec!["".into(), "Running…".into(), "".into(), "".into(), "".into(), "".into(), "".into()]);
            }
            let rs = ResultSet::new(cols, rows);
            grid::result_grid(ui, &rs, rs.rows.len(), sel)
        } else {
            let Some(t) = self.cur() else {
                return;
            };
            let Some(rs) = t.results.get(t.result_tab - 1) else {
                return;
            };
            let rows = rs.visible.min(rs.rows.len());
            grid::result_grid(ui, rs, rows, sel)
        };
        if let Some(c) = copy {
            ui.ctx().copy_text(c);
        }
        // live column resize → commit the new width onto the real result set (Messages is rebuilt
        // each frame, so its widths aren't persisted — cur_result_mut is None there)
        if let Some((d, w)) = resize {
            if let Some(rs) = self.cur_result_mut() {
                if d < rs.widths.len() {
                    rs.widths[d] = w;
                }
            }
        }
        // a column drag (display `from` → insertion index `to`) only persists for real result
        // sets — the Messages grid is rebuilt every frame, so reordering it would be pointless
        if let Some((from, to)) = reorder {
            if let Some(rs) = self.cur_result_mut() {
                let n = rs.columns.len();
                if rs.col_order.len() != n {
                    rs.col_order = (0..n).collect();
                }
                if from < rs.col_order.len() {
                    let item = rs.col_order.remove(from);
                    let to = if to > from { to - 1 } else { to };
                    rs.col_order.insert(to.min(rs.col_order.len()), item);
                }
            }
            self.grid_sel = None; // display positions no longer map to the same data
        } else {
            self.grid_sel = new_sel;
        }
    }

    fn editor(&mut self, ui: &mut egui::Ui) {
        // a connection-settings tab renders its own form instead of the SQL editor
        if self.cur().map_or(false, |t| t.conn.is_some()) {
            self.connection_tab(ui);
            return;
        }
        // a metadata tab renders the selected object's metadata instead of the SQL editor
        if self.cur().map_or(false, |t| t.meta.is_some()) {
            self.metadata_tab(ui);
            return;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(self.island_margin()))
            .show_inside(ui, |ui| {
                if self.tabs.is_empty() {
                    // empty state: ONE honest hint, centred; gone as soon as a tab opens
                    // (Design Delta v2.1 §5)
                    ui.painter().text(
                        ui.max_rect().center(),
                        egui::Align2::CENTER_CENTER,
                        "Ctrl+N — new query",
                        egui::FontId::proportional(13.0),
                        p().text_dim,
                    );
                    return;
                }
                // the white sheet = the central content rect, snapped to whole pixels
                let sheet = snap_rect(ui.painter(), ui.max_rect());
                self.editor_rect = sheet; // anchor for the find bar
                self.code_editor(ui, sheet);
            });
    }

    /// The F6 completion popup: a floating list anchored at `anchor`. Mouse hover moves the
    /// selection; a click records the row in `ac.accept` (applied by the editor once `text` is back
    /// in scope). Keyboard navigation is handled in `editor`, before the TextEdit runs.
    fn completion_popup(&mut self, ctx: &egui::Context, tab_id: u64, anchor: egui::Pos2) {
        use complete::AcKind;
        let row_h = 20.0;
        let max_rows = 9usize;
        let n = self.ac.items.len();
        let dw = ctx.fonts_mut(|f| f.glyph_width(&code_font(CODE_SIZE), '0'));
        let maxlen = self
            .ac
            .items
            .iter()
            .map(|it| it.label.chars().count())
            .max()
            .unwrap_or(8);
        let w = (maxlen as f32 * dw + 28.0).clamp(150.0, 460.0);

        egui::Area::new(egui::Id::new(("ac_popup", tab_id)))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(p().panel2)
                    .stroke(egui::Stroke::new(1.0, p().border_strong))
                    .corner_radius(CornerRadius::ZERO)
                    .inner_margin(egui::Margin::same(4))
                    .show(ui, |ui| {
                        ui.set_width(w);
                        egui::ScrollArea::vertical()
                            .max_height(max_rows as f32 * row_h)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(w);
                                let sel = self.ac.sel;
                                for i in 0..n {
                                    let (label, kind) = {
                                        let it = &self.ac.items[i];
                                        (it.label.clone(), it.kind)
                                    };
                                    let (rect, resp) = ui.allocate_exact_size(
                                        egui::vec2(w, row_h),
                                        egui::Sense::click(),
                                    );
                                    let hov = resp.hovered();
                                    if i == sel {
                                        ui.painter().rect_filled(rect, CornerRadius::ZERO, p().acc_bg2);
                                    } else if hov {
                                        ui.painter().rect_filled(rect, CornerRadius::ZERO, p().acc_bg);
                                    }
                                    let col = match kind {
                                        AcKind::Schema => p().syn_kw,
                                        AcKind::Table => p().syn_fn,
                                        AcKind::Column => p().text,
                                    };
                                    ui.painter().text(
                                        rect.left_center() + egui::vec2(6.0, 0.0),
                                        egui::Align2::LEFT_CENTER,
                                        label,
                                        code_font(CODE_SIZE),
                                        col,
                                    );
                                    if hov {
                                        self.ac.sel = i;
                                    }
                                    if resp.clicked() {
                                        self.ac.accept = Some(i);
                                    }
                                }
                            });
                    });
            });
    }

    fn confirm_modal(&mut self, ctx: &egui::Context) {
        let action = match self.confirm.clone() {
            Some(a) => a,
            None => return,
        };
        let (title, msg, ok_label): (&str, String, &str) = match &action {
            ConfirmAction::ExitApp => (
                "Unsaved changes",
                "You have unsaved tabs. Exit without saving?".to_owned(),
                "Exit",
            ),
            ConfirmAction::CloseTab(_) => (
                "Unsaved changes",
                "This tab has unsaved changes. Close it anyway?".to_owned(),
                "Close tab",
            ),
            ConfirmAction::DeleteConnections(ids) => {
                let msg = if ids.len() == 1 {
                    let name = self
                        .connections
                        .iter()
                        .find(|c| c.id == ids[0])
                        .map(|c| c.name.clone())
                        .unwrap_or_default();
                    format!("Delete the connection \"{name}\"? This removes its saved file.")
                } else {
                    format!("Delete {} connections? This removes their saved files.", ids.len())
                };
                ("Delete connection", msg, "Delete")
            }
        };
        let mut go = false;
        let r = show_modal(ctx, "confirm", 360.0, |ui| {
            // header: warning icon + title + close ×
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(15.0).strong().color(p().text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_x(ui, 22.0, 4.0, "Close") {
                        self.confirm = None;
                    }
                });
            });
            ui.add_space(12.0);
            ui.label(RichText::new(msg).color(p().text_dim));
            ui.add_space(18.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let bw = uniform_button_width(ui, &[ok_label, "Cancel"]);
                if primary_button_w(ui, ok_label, true, bw) {
                    go = true;
                }
                ui.add_space(8.0);
                if secondary_button_w(ui, "Cancel", true, bw) {
                    self.confirm = None;
                }
            });
        });
        // modal key contract: Enter = the confirming action, Esc = Cancel
        if go || r.enter {
            match action {
                ConfirmAction::ExitApp => {
                    self.allow_close = true;
                    self.confirm = None;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ConfirmAction::CloseTab(i) => {
                    self.close_tab(i);
                    self.confirm = None;
                }
                ConfirmAction::DeleteConnections(ids) => {
                    for id in ids {
                        self.delete_connection(id);
                    }
                    self.conn_sel.clear();
                    self.conn_anchor = None;
                    self.confirm = None;
                }
            }
        } else if r.escape {
            self.confirm = None;
        }
    }

    /// The About / Updates modal (replaces the old About tab). Shows the version and, driven by
    /// `self.update_status`, the check / download / restart controls in the footer (buttons at the
    /// bottom — Design System §7). Informational, so Enter/Esc both just close it.
    fn about_modal(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let status = self.update_status.clone();
        // health colour for the version chip — same as the status-bar version chip (green on the
        // latest build, amber when a newer one exists)
        let ver_fg = if self.update_outdated == Some(true) { p().warn } else { p().ok };
        let mut do_check = false;
        let mut do_download = false;
        let mut close = false;
        let r = show_modal(ctx, "about", 440.0, |ui| {
            // header: logo + app name + ×
            ui.horizontal(|ui| {
                logo(ui, 28.0);
                ui.add_space(SPACE_3);
                ui.label(
                    RichText::new("JustQuery").font(theme::ui_bold_font(20.0)).color(p().text),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_x(ui, 22.0, 4.0, "Close") {
                        close = true;
                    }
                });
            });
            ui.add_space(SPACE_3);
            // version chip coloured like the status-bar one (green/amber), tinted from the panel
            widgets::status_chip(
                ui,
                None,
                &format!("Version {}", update::CURRENT_VERSION),
                ver_fg,
                theme::tint(p().panel, ver_fg, 0.16),
                12.0,
                false,
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("A native PostgreSQL IDE for Windows, in Rust + egui.")
                    .color(p().text_dim),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new("Fonts: JetBrains Mono (OFL) · JustQuery icon set")
                    .color(p().text_dim)
                    .size(12.0),
            );
            ui.add_space(SPACE_4);
            ui.separator();
            ui.add_space(SPACE_3);
            ui.label(RichText::new("Updates").size(16.0).strong().color(p().text));
            ui.add_space(SPACE_2);
            // Fixed-height status region: the modal is centre-anchored, so a status line that grows
            // (spinner / progress / hint) would shove the footer down on each state change. Reserve
            // a constant height and render the status into it.
            let (info_rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 60.0), egui::Sense::hover());
            let mut iui = ui.new_child(
                egui::UiBuilder::new().max_rect(info_rect).layout(Layout::top_down(Align::Min)),
            );
            {
                let ui = &mut iui;
                // status line — describes the current state (or the error in red)
                match &status {
                update::UpdateStatus::Checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.add_space(8.0);
                        ui.label(RichText::new("Checking for updates…").color(p().text_dim));
                    });
                }
                update::UpdateStatus::Latest => {
                    ui.label(
                        RichText::new(format!("{}  You're on the latest version.", ic::SCAN_OK))
                            .color(p().ok),
                    );
                }
                update::UpdateStatus::Available { latest } => {
                    ui.label(
                        RichText::new(format!("Version {latest} is available.")).color(p().warn),
                    );
                }
                update::UpdateStatus::Downloading { done, total } => {
                    if *total > 0 {
                        let frac = *done as f32 / *total as f32;
                        ui.add(
                            egui::ProgressBar::new(frac)
                                .desired_width(280.0)
                                .text(format!("{:.0}%", frac * 100.0)),
                        );
                    } else {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(format!("Downloading… {} KB", done / 1024))
                                    .color(p().text_dim),
                            );
                        });
                    }
                }
                update::UpdateStatus::Applying => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(
                                "Installing… (approve the permission prompt if it appears)",
                            )
                            .color(p().text_dim),
                        );
                    });
                }
                update::UpdateStatus::PendingRestart => {
                    ui.label(
                        RichText::new(format!(
                            "{}  Update installed. Restart JustQuery to finish.",
                            ic::SCAN_OK
                        ))
                        .size(14.0)
                        .strong()
                        .color(p().ok),
                    );
                }
                update::UpdateStatus::NeverChecked => {}
                update::UpdateStatus::Error { msg, .. } => {
                    ui.label(RichText::new(msg).color(p().danger));
                }
            }

            // UAC hint, only while an update is actually available
            if matches!(status, update::UpdateStatus::Available { .. }) {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "A Windows permission prompt (UAC) may appear to install into \
                         Program Files. After it finishes, restart JustQuery.",
                    )
                    .color(p().text_dim)
                    .size(12.0),
                );
            }
            } // end fixed-height status region

            ui.add_space(SPACE_4);
            // footer: Close is the primary (accent), rightmost; the adaptive update action sits to
            // its left as a secondary button. Uniform width, right-aligned at the bottom.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let bw = uniform_button_width(
                    ui,
                    &["Download & Install", "Check for updates", "Close"],
                );
                if primary_button_w(ui, "Close", true, bw) {
                    close = true;
                }
                // A secondary update action sits left of Close only when one applies. While
                // downloading / installing — and once installed (PendingRestart) — only Close is
                // shown; to apply a downloaded update the user restarts JustQuery (per the message).
                match &status {
                    update::UpdateStatus::Available { .. }
                    | update::UpdateStatus::Error { retry_download: true, .. } => {
                        ui.add_space(SPACE_2);
                        if secondary_button_w(ui, "Download & Install", true, bw) {
                            do_download = true;
                        }
                    }
                    update::UpdateStatus::Checking => {
                        ui.add_space(SPACE_2);
                        secondary_button_w(ui, "Check for updates", false, bw);
                    }
                    update::UpdateStatus::Downloading { .. }
                    | update::UpdateStatus::Applying
                    | update::UpdateStatus::PendingRestart => {
                        // only Close
                    }
                    // NeverChecked, Latest, Error { retry_download: false }
                    _ => {
                        ui.add_space(SPACE_2);
                        if secondary_button_w(ui, "Check for updates", true, bw) {
                            do_check = true;
                        }
                    }
                }
            });
        });
        // informational dialog — Enter and Esc both just close it
        if close || r.escape || r.enter {
            self.about_open = false;
        }
        if do_check {
            self.start_update_check();
        }
        if do_download {
            self.start_update_download();
        }
    }
}
