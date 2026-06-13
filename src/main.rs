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
//!     Metadata Manager: shared object store, background scanner, on-demand columns, the scan label

use eframe::egui;
use egui::{Align, Layout, Margin, RichText, CornerRadius, Vec2};
use std::path::PathBuf;
use std::sync::Mutex;

mod about;
mod brand;
mod catalog;
mod codeeditor;
mod complete;
mod connections;
mod connections_ui;
mod crypt;
mod dialog;
mod doc;
mod fileops;
mod find;
mod grid;
mod highlight;
mod kinetic;
mod menubar;
mod meta_collector;
mod meta_details;
mod meta_manager_modal;
mod metadata;
mod format; // XML-режим: форматтер
mod proc; // XML-режим: каркас фоновых процессов (форматирование/валидация/поиск)
mod rules; // XML-режим: правила валидации (разделы 5/6)
mod search; // фоновый поиск по документу → грид (общий для SQL и XML)
mod validate; // XML-режим: валидатор XSD + правила
mod xsd; // XML-режим: модель XSD (схемы 5.0/5.1, NFA, фасеты)
#[cfg(test)]
mod sample; // demo data for the result-grid tests only (not shipped in the product)
mod sqlfmt;
mod icons;
mod startup;
mod theme;
mod update;
mod vscroll;
mod widgets;
mod winchrome;
mod xmlhl;
#[cfg(test)]
mod tests;

use connections::Connection;
use grid::GridSel;
// All semantic colours + style metrics live in `theme`; re-export so the whole crate (and the
// custom-painted widgets via `crate::PANEL2`, …) can use them by name.
pub use theme::*;
use widgets::*;
use winchrome::*;

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
    pub const DOWNLOAD: &str = icons::CHEVRONS_DOWN; // self-update download (double-chevron = "get")
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

    // The window appears already full-size with no visible "unfolding" (see startup): created
    // hidden and pre-sized to the monitor work area; maximize/reveal happen after a short warmup.
    let viewport = startup::full_size_hidden_viewport(
        egui::ViewportBuilder::default()
            .with_title("JustQuery")
            .with_icon(std::sync::Arc::new(app_icon())),
    );
    let options = eframe::NativeOptions {
        viewport,
        // wgpu, not glow: OpenGL 3.x is missing on bare VMs / RDP / driverless machines (a real
        // customer launch failure), while wgpu's DX12 path + WARP software fallback is part of
        // every Windows 10+ install. egui produces identical output on either backend.
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let run = move || {
        eframe::run_native(
            "JustQuery",
            options,
            Box::new(|cc| {
                theme::setup_fonts(&cc.egui_ctx);
                // restore the persisted theme BEFORE the first frame so there's no light flash
                theme::set_theme(&cc.egui_ctx, load_saved_theme());
                let mut app = JustQueryApp {
                    connections: connections::load(), // restore saved connections
                    ..Default::default()
                };
                update::startup_cleanup(); // remove any leftover justquery.old from a prior update
                app.start_update_check(); // background GitHub version check (fills the status chip)
                Ok(Box::new(app))
            }),
        )
    };

    // `run_native` can fail two ways before any window appears — both invisible in a release build
    // (no console; window subsystem). One is an Err from the wgpu backend when no usable graphics
    // adapter exists (should be rare now: DX12+WARP ship with Windows 10+); a panic during app
    // setup is the other. Catch both and surface a message box + log so a launch failure stops
    // being a silent "nothing happens" and tells us (and the user) the actual cause.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            report_startup_failure(&e.to_string());
            Err(e)
        }
        Err(_) => {
            let panic = LAST_PANIC.lock().ok().and_then(|g| g.clone());
            report_startup_failure(&format!(
                "Panicked during startup: {}",
                panic.as_deref().unwrap_or("(no message captured)")
            ));
            std::process::exit(1);
        }
    }
}

/// Surface a fatal startup failure (no window yet, no console in release): append the details to
/// `%APPDATA%\JustQuery\startup-error.log` and show a native message box. Best-effort throughout —
/// we are already on the failure path, so any further error is swallowed.
fn report_startup_failure(detail: &str) {
    let mut body = String::from("JustQuery couldn't start.\n\n");
    body.push_str(&format!("Error: {detail}\n"));
    if let Some(panic) = LAST_PANIC.lock().ok().and_then(|g| g.clone()) {
        // include the captured panic too, unless `detail` already is that panic
        if !detail.contains(&panic) {
            body.push_str(&format!("Panic: {panic}\n"));
        }
    }
    body.push_str(
        "\nThis is most often a graphics problem: JustQuery renders through DirectX 12 / Vulkan \
         (with a software fallback that ships with Windows 10 and later), so a failure here \
         usually means a very old or damaged Windows installation. Installing Windows updates \
         and the GPU vendor's driver usually fixes it.\n",
    );

    if let Some(path) = appdata_dir().map(|d| d.join("startup-error.log")) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "===== [{}] =====\n{body}", dialog::now_datetime());
        }
        body.push_str(&format!("\nDetails were also written to:\n{}", path.display()));
    }

    dialog::message_box("JustQuery — startup error", &body);
}

// ============================================================
// App settings (settings.json next to the connections store)
// ============================================================

/// `%APPDATA%\JustQuery` — the app's data root (settings, saved connections, update staging).
pub(crate) fn appdata_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("JustQuery"))
}

/// `%APPDATA%\JustQuery\settings.json` — tiny hand-rolled JSON, same no-serde policy as
/// the rest of the app (see update.rs). Currently holds only `{"theme":"light|dark"}`.
fn settings_path() -> Option<PathBuf> {
    Some(appdata_dir()?.join("settings.json"))
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

    // The "JQ" monogram, matching brand::paint_logo: clay rounded square + a white "J" polyline + a
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

/// Версии XSD-схемы, доступные для валидации XML (индекс — `Tab::schema_idx`).
pub(crate) const SCHEMA_VERSIONS: [&str; 2] = ["5.0", "5.1"];

/// Целое с разделителями разрядов (узкий неразрывный пробел) — для счётчиков находок/совпадений.
fn fmt_thousands(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(*ch as char);
    }
    out
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

/// One result set from a query: the grid display model (columns/widths/order) + every fetched
/// row (each cell already formatted to a string by the simple-query protocol).
pub(crate) struct ResultSet {
    pub gm: grid::GridModel, // колонки/ширины/порядок — модель грида
    pub rows: Vec<Vec<String>>,
    pub visible: usize, // rows revealed so far (incremental fetch) — lives with the result set
    pub loading: bool,  // a "fetch all" reveal is in progress for this result set
    pub sql: String,    // the statement that produced this result (for per-result Refresh)
    pub scroll: (f64, f64), // прокрутка грида (f64-px по обеим осям)
}

impl ResultSet {
    pub(crate) fn new(columns: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        // width = widest of (header, first 200 values) in chars → points (clamped)
        let mut widths: Vec<f32> = columns.iter().map(|c| c.chars().count() as f32).collect();
        for row in rows.iter().take(200) {
            for (i, v) in row.iter().enumerate() {
                if let Some(w) = widths.get_mut(i) {
                    *w = w.max(v.chars().count() as f32);
                }
            }
        }
        for w in &mut widths {
            *w = (*w * 7.0 + 18.0).clamp(54.0, 380.0);
        }
        let visible = rows.len().min(100); // first page
        let col_order = (0..columns.len()).collect();
        Self {
            gm: grid::GridModel { columns, widths, col_order },
            rows,
            visible,
            loading: false,
            sql: String::new(),
            scroll: (0.0, 0.0),
        }
    }
}

/// Документ вкладки: грузится в фоне или готов. `Detached` — временно взят редактором.
pub(crate) enum TabDoc {
    Loading { rx: std::sync::mpsc::Receiver<doc::LoadMsg>, progress: u8 },
    Ready(Box<doc::Document>),
    Detached,
}

/// What kind of tab this is — the single source of truth, a flat list. SQL / XML editors, a
/// connection-settings form, an object-metadata view, and the two singleton pages (About, Scan).
/// SQL vs XML is fixed at open/save time **by file extension** (a `.xml` file → [`TabKind::Xml`]),
/// never sniffed live from the buffer — a fresh tab is always SQL until saved as `.xml`. The
/// Connection / Meta variants carry their own payload (no separate option fields).
enum TabKind {
    Sql,
    Xml,
    Connection(Connection),
    Meta(metadata::MetaObject),
    About,
    Scan,
}

/// One tab. Most are SQL/XML text editors; the `kind` discriminates the connection-settings form,
/// the metadata view and the About/Scan pages. The editor state fields (`doc`, `ed`, `lex`,
/// `results`, …) are kept flat and are simply unused by the non-editor kinds.
struct Tab {
    id: u64, // stable id → egui remembers caret + scroll per tab
    title: String,
    doc: TabDoc, // текст живёт в документной модели (piece table + mmap)
    kind: TabKind,         // SQL / XML / Connection / Meta / About / Scan — the tab's type
    path: Option<PathBuf>, // backing file (.sql / .xml), if opened from / saved to disk
    conn_dirty: bool, // несохранённые правки ФОРМЫ подключения (SQL-вкладки смотрят doc.modified())
    executed: bool,
    result_tab: usize,         // 0 = Messages, 1.. = results[result_tab - 1]
    results: Vec<ResultSet>,   // one per result-producing statement in the last run
    log: Vec<LogEntry>,        // the Messages grid: one row per executed statement
    log_scroll: (f64, f64),    // прокрутка грида Messages
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
    ed: codeeditor::EditorState, // caret / selection / scroll for the SQL editor
    lex: codeeditor::LexCache,   // состояния подсветки на границах строк
    /// Подсветка совпадений поиска: строка → [(колонка, длина в символах)].
    search_hl: std::collections::HashMap<usize, Vec<(usize, usize)>>,
    /// Прыжок/выделение редактора (anchor, caret) на следующем кадре (0-based).
    pending_goto: Option<(doc::Pos, doc::Pos)>,
    // ---- XML-режим ----
    /// Текущий фоновый процесс вкладки (форматирование/валидация/поиск); не более одного.
    proc: Option<proc::RunningProc>,
    /// Накопленные находки валидации / совпадения поиска — для панели результатов (last-action-wins).
    findings: Option<proc::Results>,
    /// Индекс выбранной версии схемы (SCHEMA_VERSIONS); автодетект по `schemaVersion` при открытии.
    schema_idx: usize,
    /// Прокрутка грида находок/поиска (f64-пиксели по обеим осям).
    findings_scroll: (f64, f64),
}

impl Tab {
    fn new(id: u64, title: String) -> Self {
        Self {
            id,
            title,
            doc: TabDoc::Ready(Box::new(doc::Document::new_empty())),
            kind: TabKind::Sql,
            path: None,
            conn_dirty: false,
            executed: false,
            result_tab: 0,
            results: Vec::new(),
            log: Vec::new(),
            log_scroll: (0.0, 0.0),
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
            lex: codeeditor::LexCache::default(),
            search_hl: std::collections::HashMap::new(),
            pending_goto: None,
            proc: None,
            findings: None,
            schema_idx: 0,
            findings_scroll: (0.0, 0.0),
        }
    }

    /// True for an ordinary text-editor tab (SQL or XML) — i.e. not a connection-settings,
    /// metadata, About or Scan tab. Used to gate Ln/Col, Save, find, the editor toolbar.
    fn is_editor(&self) -> bool {
        matches!(self.kind, TabKind::Sql | TabKind::Xml)
    }

    /// True for an XML editor tab specifically (highlighter / XML toolbar / schema picker).
    fn is_xml(&self) -> bool {
        matches!(self.kind, TabKind::Xml)
    }

    /// The connection this tab edits, if it is a connection-settings tab (else `None`).
    fn conn(&self) -> Option<&Connection> {
        if let TabKind::Connection(c) = &self.kind { Some(c) } else { None }
    }
    fn conn_mut(&mut self) -> Option<&mut Connection> {
        if let TabKind::Connection(c) = &mut self.kind { Some(c) } else { None }
    }
    /// The object this tab views, if it is a metadata tab (else `None`).
    fn meta(&self) -> Option<&metadata::MetaObject> {
        if let TabKind::Meta(m) = &self.kind { Some(m) } else { None }
    }
    fn meta_mut(&mut self) -> Option<&mut metadata::MetaObject> {
        if let TabKind::Meta(m) = &mut self.kind { Some(m) } else { None }
    }

    pub fn doc_mut(&mut self) -> Option<&mut doc::Document> {
        match &mut self.doc {
            TabDoc::Ready(d) => Some(d),
            _ => None,
        }
    }

    /// Временно забрать документ (для code_editor, чтобы не конфликтовать с &mut self).
    pub fn take_doc(&mut self) -> Option<Box<doc::Document>> {
        match std::mem::replace(&mut self.doc, TabDoc::Detached) {
            TabDoc::Ready(d) => Some(d),
            other => {
                self.doc = other;
                None
            }
        }
    }

    pub fn put_doc(&mut self, d: Box<doc::Document>) {
        self.doc = TabDoc::Ready(d);
    }

    /// Несохранённые изменения: форма подключения — свой флаг, редактор — документ; страницы
    /// About/Scan/Meta документа не правят, поэтому всегда «чистые».
    fn dirty(&self) -> bool {
        if matches!(self.kind, TabKind::Connection(_)) {
            return self.conn_dirty;
        }
        matches!(&self.doc, TabDoc::Ready(d) if d.modified())
    }

    // ---- SQL editor edit ops (Edit menu) — operate on this tab's document + editor state ----
    fn ed_undo(&mut self) -> bool {
        let Tab { doc, ed, .. } = self;
        match doc {
            TabDoc::Ready(d) => ed.undo_op(d),
            _ => false,
        }
    }
    fn ed_redo(&mut self) -> bool {
        let Tab { doc, ed, .. } = self;
        match doc {
            TabDoc::Ready(d) => ed.redo_op(d),
            _ => false,
        }
    }
    fn ed_paste(&mut self, t: &str) {
        let Tab { doc, ed, .. } = self;
        if let TabDoc::Ready(d) = doc {
            let eol = std::str::from_utf8(d.eol.bytes()).unwrap_or("\n").to_owned();
            let norm = t.replace("\r\n", "\n").replace('\r', "\n").replace('\n', &eol);
            ed.replace(d, &norm);
        }
    }
    fn ed_cut(&mut self) -> Option<String> {
        let Tab { doc, ed, .. } = self;
        let TabDoc::Ready(d) = doc else { return None };
        if !ed.has_sel() {
            return None;
        }
        let s = ed.selection_text(d).ok()?;
        ed.replace(d, "");
        Some(s)
    }
    fn ed_copy(&mut self) -> Option<String> {
        let Tab { doc, ed, .. } = self;
        let TabDoc::Ready(d) = doc else { return None };
        if !ed.has_sel() {
            return None;
        }
        ed.selection_text(d).ok()
    }

    /// Полный текст SQL-буфера (для выполнения/форматирования). None — документ занят/огромен.
    fn full_sql(&mut self) -> Option<String> {
        let Tab { doc, .. } = self;
        let TabDoc::Ready(d) = doc else { return None };
        let last = d.line_count() - 1;
        let e = d.line_length(last);
        d.get_text_range((0, 0), (last, e)).ok()
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
    // last completed check's verdict (in-memory only); drives the status chip so it stays
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
    // search bar (Ctrl+F): Enter launches a background search → results grid (proc/start_search)
    find_open: bool,
    find_query: String,
    find_focus: bool, // request focus into the find field next frame
    tab_scroll: f32,                      // pending horizontal scroll for the editor tab strip
    tab_overflow: bool,                   // editor tabs don't fit → show the ‹ › scroll buttons
    ac: complete::Autocomplete,           // F6 completion popup state
    // virtualized editor: per-line highlight galley cache (keyed by line content)
    line_cache: codeeditor::LineCache,
    // theme the previous frame was painted with — detects a live theme switch so the galley
    // cache can be dropped on the FIRST frame of the new theme (see update_inner)
    painted_theme: theme::AppTheme,
    // custom kinetic scrolling for the trackpad (Windows delivers flick inertia as one delayed
    // lump — we ignore it and run our own momentum from the finger-lift velocity instead)
    kinetic: kinetic::KineticScroll,
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
            find_open: false,
            find_query: String::new(),
            find_focus: false,
            tab_scroll: 0.0,
            tab_overflow: false,
            ac: complete::Autocomplete::default(),
            line_cache: codeeditor::LineCache::default(),
            painted_theme: theme::current_theme(),
            kinetic: kinetic::KineticScroll::default(),
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
    fn editor_selection(&mut self) -> Option<String> {
        let i = self.active_tab;
        let t = self.tabs.get_mut(i)?;
        if !t.is_editor() {
            return None;
        }
        let Tab { doc, ed, .. } = t;
        let TabDoc::Ready(d) = doc else { return None };
        if !ed.has_sel() {
            return None;
        }
        ed.selection_text(d).ok()
    }
    /// The active text-editor tab (SQL or XML; not a connection / metadata tab), mutably.
    fn ed_active_mut(&mut self) -> Option<&mut Tab> {
        let i = self.active_tab;
        self.tabs.get_mut(i).filter(|t| t.is_editor())
    }
    /// Select the whole editor buffer (Edit ▸ Select All).
    fn editor_select_all(&mut self) {
        if let Some(t) = self.ed_active_mut() {
            let Tab { doc, ed, .. } = t;
            if let TabDoc::Ready(d) = doc {
                ed.select_all(d);
            }
        }
        self.focus_editor = true;
    }
    /// Validate the active SQL tab against the house rules WITHOUT reformatting (Validate button).
    /// The outcome goes to the status bar; on the first violation the caret jumps onto it.
    fn validate_active(&mut self) {
        if !self.is_sql_tab() {
            return;
        }
        let Some(src) = self.cur_mut().and_then(|t| t.full_sql()) else { return };
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
        let Some(src) = self.cur_mut().and_then(|t| t.full_sql()) else { return };
        match sqlfmt::format(&src) {
            Ok(formatted) => {
                let changed = src != formatted;
                if changed {
                    if let Some(t) = self.ed_active_mut() {
                        let Tab { doc, ed, .. } = t;
                        if let TabDoc::Ready(d) = doc {
                            // EOL документа сохраняем: форматтер выдаёт \n
                            let eol =
                                std::str::from_utf8(d.eol.bytes()).unwrap_or("\n").to_owned();
                            let norm = formatted.replace('\n', &eol);
                            ed.select_all(d);
                            ed.replace(d, &norm);
                        }
                    }
                }
                self.focus_editor = true;
                let msg = if changed { "Formatted" } else { "Formatted (no changes)" };
                self.fmt_status = Some((msg.into(), false));
            }
            Err(errs) => self.report_fmt_error(&src, &errs),
        }
    }

    /// Surface the first formatter/validator violation: message + `Ln/Col` in the status bar,
    /// select the offending fragment in the editor and flash its line (the editor scrolls it
    /// into view next frame).
    fn report_fmt_error(&mut self, src: &str, errs: &[sqlfmt::FmtError]) {
        let Some(e) = errs.first() else { return };
        let (ln, col) = line_col_at(src, e.pos);
        let (ln2, col2) = line_col_at(src, e.pos + e.len);
        let extra = if errs.len() > 1 {
            format!("  (+{} more)", errs.len() - 1)
        } else {
            String::new()
        };
        self.fmt_status = Some((format!("Ln {ln}, Col {col}: {}{extra}", e.msg), true));
        if let Some(t) = self.cur_mut() {
            t.pending_goto = Some(((ln - 1, col - 1), (ln2 - 1, col2 - 1)));
        }
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
    /// True when the active tab is a text editor (SQL or XML; not a connection / metadata tab).
    fn is_editor_tab(&self) -> bool {
        self.cur().is_some_and(|t| t.is_editor())
    }
    /// True when the active tab is a SQL editor specifically (gates Execute / SQL Validate / SQL
    /// Format / autocomplete). An XML editor tab is `is_editor_tab()` but not this.
    fn is_sql_tab(&self) -> bool {
        self.cur().is_some_and(|t| matches!(t.kind, TabKind::Sql))
    }
    /// True when the active tab is an XML editor (gates XML Format / Validate / schema picker).
    fn is_xml_tab(&self) -> bool {
        self.cur().is_some_and(|t| matches!(t.kind, TabKind::Xml))
    }
    /// True when the active tab is a connection-settings form.
    fn is_connection_tab(&self) -> bool {
        self.cur().is_some_and(|t| matches!(t.kind, TabKind::Connection(_)))
    }
    /// True when the active tab is an object-metadata view.
    fn is_meta_tab(&self) -> bool {
        self.cur().is_some_and(|t| matches!(t.kind, TabKind::Meta(_)))
    }
    /// True when the active tab is the About / Updates page.
    fn is_about_tab(&self) -> bool {
        self.cur().is_some_and(|t| matches!(t.kind, TabKind::About))
    }
    /// True when the active tab is the Scan manager page.
    fn is_scan_tab(&self) -> bool {
        self.cur().is_some_and(|t| matches!(t.kind, TabKind::Scan))
    }

    /// True if `path` names an XML file (`.xml`, case-insensitive). The sole signal that decides a
    /// tab's SQL/XML kind — set at open / save-as time, never sniffed from the buffer.
    fn is_xml_path(path: &std::path::Path) -> bool {
        path.extension().and_then(|s| s.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("xml"))
    }

    /// Индекс версии схемы по атрибуту `schemaVersion="5.x"` в начале документа.
    fn detect_schema_idx(head: &str) -> Option<usize> {
        let pos = head.find("schemaVersion")?;
        let rest = &head[pos..head.len().min(pos + 64)];
        SCHEMA_VERSIONS.iter().position(|v| rest.contains(v))
    }
    /// True when the active tab is savable: a connection-settings tab, or any editor tab (SQL or
    /// XML — an empty editor counts, saving an empty file is allowed). With no tabs open, or on a
    /// Meta / About / Scan page, there is nothing to save, so Save (toolbar / menu / Ctrl+S) is off.
    fn can_save(&self) -> bool {
        self.cur()
            .is_some_and(|t| t.is_editor() || matches!(t.kind, TabKind::Connection(_)))
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
        if !self.is_sql_tab() || self.tab_busy() {
            return; // не SQL-вкладка, или на ней уже идёт процесс (поиск/запрос блокирует запуск)
        }
        let idx = self.active_tab;
        let Some(params) = self.conn_params.clone() else {
            self.error_modal = Some("Not connected. Connect to a database first.".to_owned());
            return;
        };
        // run the selection if there is one, otherwise the whole tab
        let sql = self
            .editor_selection()
            .or_else(|| self.cur_mut().and_then(|t| t.full_sql()))
            .unwrap_or_default();
        if sql.trim().is_empty() {
            return;
        }
        self.grid_sel = None;
        self.show_result = true;
        self.fmt_status = None; // a run supersedes any old Validate/Format verdict
        if let Some(t) = self.cur_mut() {
            t.findings = None; // last-action-wins: SQL-результаты вытесняют находки/поиск
            t.search_hl.clear();
        }
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
        self.fmt_status = None; // a run supersedes any old Validate/Format verdict
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


    /// Open the Scan (metadata collector) manager tab. At most one exists: if it's already open
    /// this just re-selects it; otherwise a fresh Scan tab is created. Staged settings are synced
    /// from the active connection on open.
    pub(crate) fn open_scan(&mut self) {
        self.reload_meta_edits(); // sync the staged settings from the active connection
        if let Some(i) = self.tabs.iter().position(|t| matches!(t.kind, TabKind::Scan)) {
            self.active_tab = i;
            return;
        }
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id, "Scan".to_owned());
        tab.kind = TabKind::Scan;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
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
        if self.tabs.get(i).is_some_and(|t| t.dirty()) {
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
            // all tabs closed → the empty-state hint (Ctrl+N / Ctrl+O) shows in the work area
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

        // custom kinetic (momentum) scrolling for the trackpad — see `kinetic`
        self.kinetic.filter_input(ctx, raw_input);
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
        // The window is created hidden and already work-area-sized (see main()/startup): warm up
        // a few frames, then maximize + reveal it as one — no visible unfold from a small window.
        startup::reveal_after_warmup(ctx, &mut self.startup_frame);

        // once the window is up, offer to connect straight away (no connections → "create one")
        if startup::revealed(self.startup_frame) && !self.did_startup_connect {
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

        // keep frames flowing through trackpad-flick gaps so the momentum animates smoothly
        self.kinetic.grace_repaint(ctx);

        // keep the Scan worker awake while the user is active: ping it on input, throttled to ~2s.
        // When the app is idle there are no input events (and no frames), so no pings arrive and the
        // scanner idles itself after IDLE_SECS — no DB churn while the user is away.
        if self.connected {
            let active = ctx.input(|i| !i.events.is_empty());
            if active {
                let due = self
                    .last_activity_ping
                    .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(2));
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
            && self.tabs.iter().any(|t| t.dirty())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(ConfirmAction::ExitApp);
        }

        // фоновая загрузка больших файлов в открываемые вкладки
        self.poll_loading(ctx);
        self.poll_procs(ctx);

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
                // still running — poll at ~10 Hz (a bare request_repaint would pin max FPS)
                None => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
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
                // ~10 Hz poll while waiting (a bare request_repaint would pin max FPS)
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.test_result = Some(Err("Test thread stopped unexpectedly.".to_owned()));
                    self.test_rx = None;
                }
            }
        }

        // poll the in-flight update check / download (background thread)
        self.poll_update(ctx);

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
                // ~10 Hz poll while waiting (a bare request_repaint would pin max FPS)
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
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
        // per-tab work-area toolbar — a chrome strip under the tabs (varies by tab kind; absent
        // for kinds with no actions, e.g. a metadata view)
        self.tab_toolbar_bar(ui);
        // одна нижняя панель, last-action-wins: находки/поиск (XML) перекрывают SQL-результаты
        if self.cur().is_some_and(|t| t.findings.is_some()) {
            self.findings_panel(ui);
        } else if self.show_result && self.cur().is_some_and(|t| t.executed) {
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
            if self.is_xml_tab() {
                self.start_xml_validate();
            } else {
                self.execute(ctx);
            }
        }
        // F5 → format: XML pretty-print on an XML tab, else SQL house-style format
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F5)) {
            if self.is_xml_tab() {
                self.start_xml_format();
            } else {
                self.format_active();
            }
        }
        // F6 → open the completion popup (built in `editor` where the live caret is known)
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F6)) {
            self.ac.request = true;
            self.focus_editor = true;
        }
        if self.find_open && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.close_find();
        }
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
                    if qbtn(ui, ic::NEW, "New tab").clicked() {
                        self.new_tab();
                    }
                    if qbtn(ui, ic::OPEN, "Open").clicked() {
                        self.open_file();
                    }
                    if self.can_save() {
                        if qbtn(ui, ic::SAVE, "Save").clicked() {
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
                        if qbtn(ui, icons::PLUG_OFF, "Disconnect").clicked() {
                            self.request_disconnect();
                        }
                    } else if qbtn(ui, icons::PLUG, "Connect…").clicked() {
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
                        if t.dirty() {
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
        // Right margin = the editor island's right margin (8) so the chips line up exactly with
        // the editor's right border; in a restored window the corner resize grip (~14px of
        // diagonals) lives there too, so pad past it instead of drawing the version under it.
        let maximized = ui.input(|i| i.viewport().maximized).unwrap_or(false);
        egui::Panel::bottom("status")
            .exact_size(24.0)
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                left: 10,
                right: if maximized { 8 } else { 22 },
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
                            // hard-clip the left block to the space the right group left over, so
                            // a long message never overdraws scan/connection/version when narrow
                            ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                            if let Some(t) = self.cur().filter(|t| t.is_editor()) {
                                // кодировка/EOL — из документа (реальные, а не предположение)
                                let (enc, eol) = match &t.doc {
                                    TabDoc::Ready(d) => (d.encoding_label.clone(), d.eol.label()),
                                    _ => ("UTF-8".to_owned(), "—"),
                                };
                                ui.label(
                                    RichText::new(format!("Ln {}, Col {}", self.cursor_ln, self.cursor_col))
                                        .size(sz)
                                        .color(p().text),
                                );
                                ui.label(RichText::new("·").size(sz).color(p().disabled));
                                ui.label(RichText::new(enc).size(sz).color(p().text));
                                ui.label(RichText::new("·").size(sz).color(p().disabled));
                                ui.label(RichText::new(eol).size(sz).color(p().text));
                            }
                            // transient message: error > SQL running > XML process > verdict > rows
                            let editor_tab = self.is_editor_tab();
                            let proc_info = self
                                .cur()
                                .and_then(|t| t.proc.as_ref())
                                .map(|rp| (rp.label(), rp.started.elapsed(), rp.progress));
                            let has_msg = self.last_error.is_some()
                                || self.cur().and_then(|t| t.exec_start).is_some()
                                || proc_info.is_some()
                                || self.fmt_status.is_some()
                                || (self.show_result && self.cur_result().is_some());
                            if editor_tab && has_msg {
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
                            } else if let Some((label, el, pct)) = proc_info {
                                // XML фоновый процесс (формат/валидация/поиск) — прогресс + таймер
                                ui.label(
                                    RichText::new(format!(
                                        "{label}: выполняется…  {}  ·  {pct:.0}%",
                                        fmt_elapsed(el)
                                    ))
                                    .size(sz)
                                    .color(p().text_dim),
                                );
                            } else if let Some((msg, is_err)) =
                                // вердикт Validate/Format касается буфера редактора — показываем
                                // на любой редактор-вкладке (SQL или XML), но не на conn/meta
                                self.fmt_status.clone().filter(|_| editor_tab)
                            {
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

    /// XML-режим: панель находок валидации / совпадений поиска активной вкладки (одна панель,
    /// last-action-wins). Один грид; клик по строке → прыжок к строке документа.
    fn findings_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        let max_h = (ctx.content_rect().height() - 180.0).max(160.0);
        let rh = self.cur().map_or(300.0, |t| t.result_height).clamp(120.0, max_h);
        let i = self.active_tab;
        let Some(mut res) = self.tabs.get_mut(i).and_then(|t| t.findings.take()) else { return };
        let sel = self.grid_sel;
        let mut scroll = self.tabs[i].findings_scroll;
        let mut close = false;
        let mut out: Option<grid::GridOutput> = None;
        let margin = self.island_margin();
        let (title, err_col) = match &res.kind {
            proc::ResultsKind::Search(_) => ("Результаты поиска", None),
            proc::ResultsKind::Validation(_) => ("Находки валидации", Some(0usize)),
        };
        let count = res.len();
        let truncated = res.truncated;
        egui::Panel::bottom("xml_findings")
            .resizable(false)
            .exact_size(rh)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p().panel2))
            .show_inside(ui, |ui| {
                egui::Panel::top("findings_bar")
                    .exact_size(TABBAR_H)
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: 10,
                        right: 6,
                        top: 0,
                        bottom: 0,
                    }))
                    .show_inside(ui, |ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if close_x(ui, "Закрыть панель находок") {
                                close = true;
                            }
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                let note = if truncated { " · превышен лимит 100 МБ" } else { "" };
                                ui.label(
                                    RichText::new(format!("{title}: {}{note}", fmt_thousands(count)))
                                        .size(12.0)
                                        .color(if truncated { p().danger } else { p().text }),
                                );
                            });
                        });
                    });
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::Frame::new().inner_margin(margin).show(ui, |ui| {
                    island(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        let row = |r: usize| res.row_values(r);
                        let err = |r: usize| res.row_is_err(r);
                        out = Some(grid::result_grid(
                            ui, &res.grid, count, sel, &row, &err, err_col, false, None, &mut scroll,
                        ));
                    });
                });
            });
        if let Some(out) = out {
            if let Some(c) = out.copy.clone() {
                ctx.copy_text(c);
            }
            self.grid_sel = out.sel;
            res.grid.apply(&out); // reorder/resize живут в GridModel внутри `res`
            if let Some(r) = out.clicked_row {
                let goto = match &res.kind {
                    proc::ResultsKind::Search(v) => v.get(r).map(|m| (m.line, m.col)),
                    proc::ResultsKind::Validation(v) => {
                        v.get(r).filter(|f| f.line > 0).map(|f| (f.line - 1, 0))
                    }
                };
                if let Some(g) = goto {
                    self.tabs[i].pending_goto = Some((g, g));
                    self.focus_editor = true;
                }
            }
        }
        self.tabs[i].findings_scroll = scroll;
        if close {
            self.tabs[i].search_hl.clear();
        } else {
            self.tabs[i].findings = Some(res);
        }
    }

    fn result_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        let max_h = (ctx.content_rect().height() - 180.0).max(160.0);
        // height lives with the active tab, so resizing one tab's result panel doesn't
        // bleed into the others
        let mut rh = self.cur().map_or(300.0, |t| t.result_height).clamp(120.0, max_h);
        // maximize state also lives with the tab, so one tab's full-screen result doesn't
        // carry over to another
        let mut full = self.cur().is_some_and(|t| t.result_full);
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
                            if close_x(ui, "Close") {
                                do_close = true;
                            }
                            let (icon, tip) = if full {
                                (ic::COLLAPSE, "Restore result panel")
                            } else {
                                (ic::EXPAND, "Maximize result panel")
                            };
                            if qbtn(ui, icon, tip).clicked() {
                                full = !full;
                            }
                            // remaining space (left→right): tabs, then the resize grab. The panel
                            // action icons (maximize/close) already reserved their fixed zone on
                            // the right; the tab lane scrolls inside what's left, with the same
                            // ‹ › buttons as the editor tab strip once it overflows — every
                            // result tab stays reachable (Delta v2.1 §5: never under the icons).
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                let names = self.result_tab_names();
                                let row_h = ui.max_rect().height();
                                // last frame's overflow decides whether ‹ › reserve their slot
                                // (one-frame lag, same as the editor tab strip's arrows)
                                let overflow_id = egui::Id::new("result_lane_overflow");
                                let scroll_id = egui::Id::new("result_lane_scroll");
                                let was_overflow: bool = ui
                                    .ctx()
                                    .data_mut(|d| d.get_temp(overflow_id).unwrap_or(false));
                                let arrows_w = if was_overflow { 56.0 } else { 0.0 };
                                let scroll_w = (ui.available_width() - arrows_w).max(0.0);
                                let mut sel = None;
                                let out = ui
                                    .allocate_ui(Vec2::new(scroll_w, row_h), |ui| {
                                        egui::ScrollArea::horizontal()
                                            .auto_shrink([false, false])
                                            .scroll_bar_visibility(
                                                egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                            )
                                            .show(ui, |ui| {
                                                let pend: f32 = ui.ctx().data_mut(|d| {
                                                    d.get_temp(scroll_id).unwrap_or(0.0)
                                                });
                                                if pend != 0.0 {
                                                    ui.scroll_with_delta(Vec2::new(pend, 0.0));
                                                    ui.ctx().data_mut(|d| {
                                                        d.insert_temp(scroll_id, 0.0f32)
                                                    });
                                                }
                                                ui.horizontal_centered(|ui| {
                                                    let (s, _) = tab_strip(
                                                        ui, &names, active_rt, false, None,
                                                    );
                                                    sel = s;
                                                });
                                            })
                                    })
                                    .inner;
                                let overflow =
                                    out.content_size.x > out.inner_rect.width() + 1.0;
                                ui.ctx().data_mut(|d| d.insert_temp(overflow_id, overflow));
                                if was_overflow {
                                    if qchevron(ui, true, "Scroll result tabs left").clicked() {
                                        ui.ctx()
                                            .data_mut(|d| d.insert_temp(scroll_id, 90.0f32));
                                    }
                                    if qchevron(ui, false, "Scroll result tabs right").clicked() {
                                        ui.ctx()
                                            .data_mut(|d| d.insert_temp(scroll_id, -90.0f32));
                                    }
                                }
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

                // result work-area toolbar — a chrome strip under the result tabs
                subbar(ui, "result_toolbar_bar", |ui| self.result_toolbar(ui));

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
        let executed = self.cur().is_some_and(|t| t.executed);
        let visible = self.cur_result().map_or(0, |r| r.visible);
        let loading = self.cur_result().is_some_and(|r| r.loading);
        let total = self.cur_total();
        let running = self.cur().is_some_and(|t| t.exec_rx.is_some());
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

    /// The per-tab work-area toolbar: a chrome strip under the tabs holding the active tab's
    /// action icons. Every tab kind gets one, with its own icon set; a kind that has no actions
    /// (Meta, or no tab at all) draws no strip at all — the editor sheet then sits flush under the
    /// tabs. The main icon-toolbar and the menu stay static; only this strip varies by kind.
    fn tab_toolbar_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        if self.is_sql_tab() {
            subbar(ui, "tab_toolbar", |ui| self.editor_toolbar(ui, ctx));
        } else if self.is_xml_tab() {
            subbar(ui, "tab_toolbar", |ui| self.xml_editor_toolbar(ui, ctx));
        } else if self.is_connection_tab() {
            subbar(ui, "tab_toolbar", |ui| self.conn_toolbar(ui));
        } else if self.is_about_tab() {
            subbar(ui, "tab_toolbar", |ui| self.about_toolbar(ui));
        } else if self.is_scan_tab() {
            subbar(ui, "tab_toolbar", |ui| self.scan_toolbar(ui));
        }
        // Meta tab / no tab → no toolbar strip
    }

    /// XML work-area toolbar: Format / Validate / schema version / Stop. Format and Stop are live
    /// (P2); Validate + schema picker land in P3 (shown disabled). Actions are gated while a
    /// background process runs on the tab (one process per tab).
    fn xml_editor_toolbar(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        ui.spacing_mut().item_spacing.x = 2.0;
        let busy = self.tab_busy();
        let running = self.cur().is_some_and(|t| t.proc.is_some());
        if !busy {
            if qbtn_sm(ui, ic::FORMAT, p().text, "Форматировать XML (F5)").clicked() {
                self.start_xml_format();
            }
            if qbtn_sm(ui, ic::VALIDATE, p().text, "Проверить XML (F8)").clicked() {
                self.start_xml_validate();
            }
        } else {
            qbtn_off_sm(ui, ic::FORMAT, "Форматировать (выполняется процесс)");
            qbtn_off_sm(ui, ic::VALIDATE, "Проверить (выполняется процесс)");
        }
        if running {
            if qbtn_sm(ui, ic::STOP, p().danger, "Остановить").clicked() {
                self.stop_active_proc();
            }
        } else {
            qbtn_off_sm(ui, ic::STOP, "Останавливать нечего");
        }
        // версия схемы (per-tab) — выпадающий список 5.0 / 5.1
        ui.add_space(6.0);
        let opts: Vec<String> = SCHEMA_VERSIONS.iter().map(|s| (*s).to_owned()).collect();
        let sel = self.cur().map(|t| t.schema_idx);
        if let Some(picked) = styled_combo(ui, "xml_schema_ver", 60.0, 13.0, !busy, sel, &opts) {
            if let Some(t) = self.cur_mut() {
                t.schema_idx = picked;
            }
        }
    }

    /// Editor work-area toolbar icons (Execute / Stop / Commit / Rollback). Identical for every
    /// SQL tab but enabled/disabled per the active tab's state (tabs run independently on their
    /// own session connections).
    fn editor_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.spacing_mut().item_spacing.x = 2.0;
        // Execute needs a SQL tab + a live connection + some SQL + this tab not already running.
        let active_running = self.cur().is_some_and(|t| t.exec_rx.is_some());
        let has_sql = self
            .cur()
            .is_some_and(|t| matches!(&t.doc, TabDoc::Ready(d) if d.char_count() > 0));
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
        let fetching = self.cur_result().is_some_and(|r| r.loading);
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
        if !self.cur().is_some_and(|t| t.executed) {
            ui.vertical_centered(|ui| {
                ui.add_space(34.0);
                ui.colored_label(p().text_dim, "Press ▶ Execute to get results");
            });
            return;
        }
        let sel = self.grid_sel;
        let messages = self.cur().is_none_or(|t| t.result_tab == 0);
        let (out, scroll, messages) = if messages {
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
            if self.cur().is_some_and(|t| t.running) {
                rows.push(vec!["".into(), "Running…".into(), "".into(), "".into(), "".into(), "".into(), "".into()]);
            }
            let rs = ResultSet::new(cols, rows);
            // a row whose Status column says Error/Fatal gets a 2px danger bar + danger text
            let status = 1usize; // колонка Status
            let row = |i: usize| rs.rows[i].clone();
            let err = |i: usize| {
                rs.rows[i].get(status).is_some_and(|v| v == "Error" || v == "Fatal")
            };
            let mut scroll = self.cur().map_or((0.0, 0.0), |t| t.log_scroll);
            let out = grid::result_grid(
                ui, &rs.gm, rs.rows.len(), sel, &row, &err, Some(status), false, None,
                &mut scroll,
            );
            (out, scroll, true)
        } else {
            let Some(t) = self.cur() else {
                return;
            };
            let Some(rs) = t.results.get(t.result_tab - 1) else {
                return;
            };
            let rows = rs.visible.min(rs.rows.len());
            let row = |i: usize| rs.rows[i].clone();
            let err = |_: usize| false; // у результатов данных нет колонки статуса
            let mut scroll = rs.scroll;
            let out = grid::result_grid(
                ui, &rs.gm, rows, sel, &row, &err, None, false, None, &mut scroll,
            );
            (out, scroll, false)
        };
        if let Some(c) = out.copy.clone() {
            ui.ctx().copy_text(c);
        }
        // apply scroll/resize/reorder back to the real result set (the Messages grid is rebuilt
        // each frame, so only its scroll persists — on the tab itself)
        if messages {
            if let Some(t) = self.cur_mut() {
                t.log_scroll = scroll;
            }
        } else if let Some(rs) = self.cur_result_mut() {
            rs.scroll = scroll;
            rs.gm.apply(&out);
        }
        if out.reorder.is_some() {
            self.grid_sel = None; // display positions no longer map to the same data
        } else {
            self.grid_sel = out.sel;
        }
    }

    fn editor(&mut self, ui: &mut egui::Ui) {
        // each non-editor kind renders its own body instead of the SQL/XML editor
        if self.is_connection_tab() {
            self.connection_tab(ui);
            return;
        }
        if self.is_meta_tab() {
            self.metadata_tab(ui);
            return;
        }
        if self.is_about_tab() {
            self.about_tab(ui);
            return;
        }
        if self.is_scan_tab() {
            self.scan_tab(ui);
            return;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(self.island_margin()))
            .show_inside(ui, |ui| {
                if self.tabs.is_empty() {
                    // empty state: a centred hint with the two ways to get a tab; gone as soon as
                    // one opens (Design Delta v2.1 §5)
                    ui.painter().text(
                        ui.max_rect().center(),
                        egui::Align2::CENTER_CENTER,
                        "Ctrl+N — new query     Ctrl+O — open file",
                        egui::FontId::proportional(13.0),
                        p().text_dim,
                    );
                    return;
                }
                // the white sheet = the central content rect, snapped to whole pixels
                let sheet = snap_rect(ui.painter(), ui.max_rect());
                self.editor_rect = sheet; // anchor for the find bar
                if let Some(pct) = self.cur().and_then(|t| match &t.doc {
                    TabDoc::Loading { progress, .. } => Some(*progress),
                    _ => None,
                }) {
                    // документ ещё грузится — лист с подписью прогресса вместо редактора
                    ui.painter().rect_filled(
                        sheet,
                        egui::CornerRadius::same(RADIUS_ISLAND),
                        p().field_bg,
                    );
                    crisp_border(ui.painter(), sheet, p().border_strong);
                    ui.painter().text(
                        sheet.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("Loading file… {pct}%"),
                        egui::FontId::proportional(13.0),
                        p().text_dim,
                    );
                    return;
                }
                self.code_editor(ui, sheet);
            });
    }

    /// Обёртка редактора: собирает [`codeeditor::EditorCtx`] из активной вкладки
    /// (SQL-подсветка, Smart-Tab, F6-автокомплит) и раскладывает [`codeeditor::EditorOut`]
    /// обратно по состоянию приложения.
    fn code_editor(&mut self, ui: &mut egui::Ui, sheet: egui::Rect) {
        let ctx = &ui.ctx().clone();
        let idx = self.active_tab.min(self.tabs.len() - 1);
        let tab_id = self.tabs[idx].id;
        let ed_id = egui::Id::new(("code_editor", tab_id));
        let Some(mut doc) = self.tabs[idx].take_doc() else { return };
        let mut ed = std::mem::take(&mut self.tabs[idx].ed);
        let is_xml = self.tabs[idx].is_xml();
        // на время фонового процесса вкладки (формат/валидация/поиск) правки запрещены
        let read_only = self.tabs[idx].exec_rx.is_some() || self.tabs[idx].proc.is_some();

        // автокомплит ПЕРЕД редактором (только SQL, не во время процесса)
        let focused = ctx.memory(|m| m.has_focus(ed_id));
        let mut edited = if focused && !is_xml && !read_only {
            self.editor_completion(&mut doc, &mut ed, ctx, tab_id)
        } else {
            false
        };

        // Подсветка выбирается по языку вкладки; сам редактор языко-нейтрален (берёт колбэк).
        let sql_line = |text: &str, st: codeeditor::LexState| {
            let (job, end) =
                highlight::highlight_sql(text, highlight::LineState::from_key(st), CODE_SIZE);
            (job, end.key())
        };
        let sql_advance = |text: &str, st: codeeditor::LexState| {
            highlight::highlight_sql_state_only(text, highlight::LineState::from_key(st)).key()
        };
        let xml_line = |text: &str, st: codeeditor::LexState| {
            let (job, end) = xmlhl::highlight_xml(text, xmlhl::LineState::from_key(st), CODE_SIZE);
            (job, end.key())
        };
        let xml_advance = |text: &str, st: codeeditor::LexState| {
            xmlhl::highlight_xml_state_only(text, xmlhl::LineState::from_key(st)).key()
        };
        // Tab: в КОНЦЕ строки — всегда ровно два пробела (без сетки/подсчёта). Внутри строки —
        // прежнее выравнивание: SQL — smart-«хук»/4-колоночные стопы; XML — 2-колоночная сетка.
        let sql_tab_insert = |d: &mut doc::Document, (l, c): doc::Pos| {
            if c >= d.line_length(l) {
                return "  ".to_owned();
            }
            let prev = if l > 0 { Some(d.get_line(l - 1)) } else { None };
            complete::tab_spaces(prev.as_deref(), c)
        };
        let xml_tab_insert = |d: &mut doc::Document, (l, c): doc::Pos| {
            if c >= d.line_length(l) {
                "  ".to_owned()
            } else {
                " ".repeat(2 - (c % 2))
            }
        };

        let hl = if is_xml {
            codeeditor::Highlighter { line: &xml_line, advance: &xml_advance }
        } else {
            codeeditor::Highlighter { line: &sql_line, advance: &sql_advance }
        };
        let tab_insert: &dyn Fn(&mut doc::Document, doc::Pos) -> String =
            if is_xml { &xml_tab_insert } else { &sql_tab_insert };

        let out = {
            let Self { tabs, line_cache, focus_editor, focus_grace, .. } = self;
            let t = &mut tabs[idx];
            codeeditor::code_editor(
                ui,
                sheet,
                codeeditor::EditorCtx {
                    doc: &mut doc,
                    ed: &mut ed,
                    lex: &mut t.lex,
                    line_cache,
                    search_hl: &mut t.search_hl,
                    pending_goto: &mut t.pending_goto,
                    focus_request: focus_editor,
                    focus_grace,
                    read_only,
                    ed_id,
                    hl,
                    tab_insert,
                },
            )
        };
        edited |= out.edited;
        self.cursor_ln = out.caret.0 + 1;
        self.cursor_col = out.caret.1 + 1;
        if let Some(e) = out.error {
            self.error_modal = Some(e);
        }

        // ---- completion popup (якорь от геометрии редактора этого кадра) ----
        if self.ac.open && !self.ac.items.is_empty() && self.ac.tab == tab_id {
            let (line, scol) = self.ac.start;
            let ax = out.origin.x + scol as f32 * out.char_w;
            let ay = out.origin.y + (line + 1) as f32 * out.row_h + 2.0;
            self.completion_popup(ctx, tab_id, egui::pos2(ax, ay));
        }
        if let Some(i) = self.ac.accept.take() {
            if i < self.ac.items.len() {
                let ins = self.ac.items[i].insert.clone();
                let (sl, sc) = self.ac.start;
                let chars: Vec<char> = doc.get_line(sl).chars().collect();
                let mut e = sc.min(chars.len());
                while e < chars.len() && codeeditor::is_word(chars[e]) {
                    e += 1;
                }
                ed.select_range(&mut doc, (sl, sc), (sl, e));
                ed.replace(&mut doc, &ins);
                self.focus_editor = true;
                edited = true;
            }
            self.ac.close();
        }

        if edited {
            // the buffer changed → any Validate/Format verdict in the status bar is stale
            self.fmt_status = None;
        }
        self.tabs[idx].ed = ed;
        self.tabs[idx].put_doc(doc);
    }

    // ============================================================
    //  XML-режим: фоновые процессы (форматирование / валидация / поиск)
    // ============================================================

    /// Активная вкладка занята фоновой работой (SQL-запрос ИЛИ XML-процесс): гейтит запуск
    /// других процессов и делает редактор read-only на время.
    fn tab_busy(&self) -> bool {
        self.cur().is_some_and(|t| t.exec_rx.is_some() || t.proc.is_some())
    }

    /// Запустить XML-форматирование активной XML-вкладки (фон; результат заменяет содержимое
    /// одной undo-операцией через `swap_origin`).
    fn start_xml_format(&mut self) {
        if !self.is_xml_tab() || self.tab_busy() {
            return;
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let Some(t) = self.cur_mut() else { return };
        let Some(d) = t.doc_mut() else { return };
        format::spawn_format(d.snapshot(), std::sync::Arc::clone(&cancel), tx);
        t.search_hl.clear();
        t.proc = Some(proc::RunningProc {
            kind: proc::ProcKind::Format,
            rx,
            cancel,
            started: std::time::Instant::now(),
            progress: 0.0,
            schema: String::new(),
            capped: false,
        });
        self.fmt_status = None;
    }

    /// Запустить валидацию активной XML-вкладки против XSD + правил выбранной версии (фон).
    fn start_xml_validate(&mut self) {
        if !self.is_xml_tab() || self.tab_busy() {
            return;
        }
        let version = self
            .cur()
            .map(|t| SCHEMA_VERSIONS[t.schema_idx.min(SCHEMA_VERSIONS.len() - 1)].to_owned())
            .unwrap_or_default();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let Some(t) = self.cur_mut() else { return };
        let Some(d) = t.doc_mut() else { return };
        validate::spawn_validate(d.snapshot(), version.clone(), std::sync::Arc::clone(&cancel), tx);
        t.search_hl.clear();
        t.findings = Some(proc::Results::new_validation()); // last-action-wins: панель покажет находки
        t.proc = Some(proc::RunningProc {
            kind: proc::ProcKind::Validate,
            rx,
            cancel,
            started: std::time::Instant::now(),
            progress: 0.0,
            schema: version,
            capped: false,
        });
        self.fmt_status = None;
    }

    /// Запустить фоновый поиск `query` по активной вкладке (SQL или XML) → грид результатов.
    /// На время поиска вкладка read-only и другие процессы заблокированы (как валидация/формат).
    fn start_search(&mut self, query: String) {
        if query.is_empty() || !self.is_editor_tab() || self.tab_busy() {
            return;
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let Some(t) = self.cur_mut() else { return };
        let Some(d) = t.doc_mut() else { return };
        search::spawn_search(d.snapshot(), query, std::sync::Arc::clone(&cancel), tx);
        t.search_hl.clear();
        t.findings = Some(proc::Results::new_search()); // last-action-wins: панель покажет совпадения
        t.proc = Some(proc::RunningProc {
            kind: proc::ProcKind::Search,
            rx,
            cancel,
            started: std::time::Instant::now(),
            progress: 0.0,
            schema: String::new(),
            capped: false,
        });
        self.fmt_status = None;
    }

    /// Остановить процесс на активной вкладке (мягкая отмена через флаг).
    fn stop_active_proc(&mut self) {
        if let Some(rp) = self.cur().and_then(|t| t.proc.as_ref()) {
            rp.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Опрос каналов выполняющихся XML-процессов всех вкладок (поиск/валидация/форматирование).
    fn poll_procs(&mut self, ctx: &egui::Context) {
        for i in 0..self.tabs.len() {
            let Tab { proc: proc_slot, findings, search_hl, .. } = &mut self.tabs[i];
            let Some(rp) = proc_slot.as_mut() else { continue };
            let mut fin: Option<proc::ProcMsg> = None;
            loop {
                match rp.rx.try_recv() {
                    Ok(proc::ProcMsg::Progress(p)) => rp.progress = p,
                    Ok(proc::ProcMsg::SearchBatch(batch)) => {
                        if Self::append_search(findings, search_hl, batch) {
                            rp.capped = true;
                            rp.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Ok(proc::ProcMsg::Findings(batch)) => {
                        if Self::append_findings(findings, batch) {
                            rp.capped = true;
                            rp.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Ok(m) => {
                        fin = Some(m);
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        fin = Some(proc::ProcMsg::Failed(
                            "the process thread ended unexpectedly".to_owned(),
                        ));
                        break;
                    }
                }
            }
            let Some(fin) = fin else { continue };
            self.finish_proc(ctx, i, fin);
        }
    }

    /// Добавить батч совпадений поиска с учётом лимита памяти. true → лимит превышен.
    fn append_search(
        findings: &mut Option<proc::Results>,
        search_hl: &mut std::collections::HashMap<usize, Vec<(usize, usize)>>,
        batch: Vec<proc::SearchMatch>,
    ) -> bool {
        let r = findings.get_or_insert_with(proc::Results::new_search);
        if r.truncated {
            return true;
        }
        for m in batch {
            r.bytes += m.approx_bytes();
            search_hl.entry(m.line).or_default().push((m.col, m.len));
            if let proc::ResultsKind::Search(v) = &mut r.kind {
                v.push(m);
            }
            if r.bytes > proc::RESULTS_CAP_BYTES {
                r.truncated = true;
                return true;
            }
        }
        false
    }

    /// Добавить батч находок валидации с учётом лимита. true → лимит превышен.
    fn append_findings(findings: &mut Option<proc::Results>, batch: Vec<proc::Finding>) -> bool {
        let r = findings.get_or_insert_with(proc::Results::new_validation);
        if r.truncated {
            return true;
        }
        for f in batch {
            r.bytes += f.approx_bytes();
            if let proc::ResultsKind::Validation(v) = &mut r.kind {
                v.push(f);
            }
            if r.bytes > proc::RESULTS_CAP_BYTES {
                r.truncated = true;
                return true;
            }
        }
        false
    }

    /// Завершение процесса вкладки `i`: итоговое сообщение в статус-бар; для форматирования —
    /// применение результата (`swap_origin`) или прыжок+вспышка на строку ошибки.
    fn finish_proc(&mut self, ctx: &egui::Context, i: usize, fin: proc::ProcMsg) {
        let Some(rp) = self.tabs[i].proc.take() else { return };
        let label = rp.label();
        let kind = rp.kind;
        let capped = rp.capped;
        let secs = rp.started.elapsed().as_secs_f32();
        let dur = format!("{secs:.1}").replace('.', ",");
        let msg: (String, bool) = match fin {
            proc::ProcMsg::Done | proc::ProcMsg::Cancelled if capped => (
                format!(
                    "{label}: {} — превышен лимит результатов 100 МБ, показано накопленное",
                    kind.stopped_word()
                ),
                true,
            ),
            proc::ProcMsg::Done => match kind {
                proc::ProcKind::Search => {
                    let n = self.tabs[i].findings.as_ref().map_or(0, |r| r.len());
                    (
                        format!(
                            "{label}: {} за {dur} с — найдено: {}",
                            kind.finished_word(),
                            fmt_thousands(n)
                        ),
                        false,
                    )
                }
                proc::ProcKind::Validate => {
                    let (errs, warns) = match self.tabs[i].findings.as_ref().map(|r| &r.kind) {
                        Some(proc::ResultsKind::Validation(v)) => {
                            let e =
                                v.iter().filter(|f| f.severity == proc::Severity::Error).count();
                            (e, v.len() - e)
                        }
                        _ => (0, 0),
                    };
                    let text = if errs == 0 && warns == 0 {
                        format!("{label}: {} за {dur} с — ошибок нет", kind.finished_word())
                    } else {
                        format!(
                            "{label}: {} за {dur} с — ошибок: {}, предупреждений: {}",
                            kind.finished_word(),
                            fmt_thousands(errs),
                            fmt_thousands(warns)
                        )
                    };
                    (text, errs > 0)
                }
                proc::ProcKind::Format => (format!("{label}: успешно за {dur} с"), false),
            },
            proc::ProcMsg::Cancelled => {
                (format!("{label}: {} пользователем", kind.stopped_word()), true)
            }
            proc::ProcMsg::Failed(e) => (format!("{label}: ошибка — {e}"), true),
            proc::ProcMsg::FormatOk { out_path, changed } => {
                let mut text = if changed {
                    format!("{label}: успешно за {dur} с")
                } else {
                    format!("{label}: успешно за {dur} с (без изменений)")
                };
                let mut is_err = false;
                if changed {
                    match self.tabs[i].doc_mut() {
                        Some(d) => {
                            if let Err(e) = d.swap_origin(&out_path) {
                                text = format!("{label}: ошибка применения — {e}");
                                is_err = true;
                            }
                        }
                        None => {
                            text = format!("{label}: документ недоступен");
                            is_err = true;
                        }
                    }
                }
                (text, is_err)
            }
            proc::ProcMsg::FormatErr { line, col, msg } => {
                let l0 = line.saturating_sub(1);
                let g = (l0, col.saturating_sub(1));
                self.tabs[i].pending_goto = Some((g, g));
                self.tabs[i].ed.flash_line = Some((l0, ctx.input(|inp| inp.time)));
                if i == self.active_tab {
                    self.focus_editor = true;
                }
                (format!("{label}: ошибка в строке {line} — {msg}"), true)
            }
            _ => (format!("{label}: завершено"), false),
        };
        self.fmt_status = Some(msg);
        ctx.request_repaint();
    }

    /// Опрос каналов фоновой загрузки файлов: прогресс / готово / ошибка.
    fn poll_loading(&mut self, ctx: &egui::Context) {
        for i in 0..self.tabs.len() {
            let TabDoc::Loading { rx, progress } = &mut self.tabs[i].doc else { continue };
            let mut done: Option<Result<Box<doc::Document>, String>> = None;
            loop {
                match rx.try_recv() {
                    Ok(doc::LoadMsg::Progress(p)) => *progress = p,
                    Ok(doc::LoadMsg::Done(d)) => {
                        done = Some(Ok(d));
                        break;
                    }
                    Ok(doc::LoadMsg::Failed(e)) => {
                        done = Some(Err(e));
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        // ~10 Гц опрос, пока грузится
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        done = Some(Err("the load thread ended unexpectedly".to_owned()));
                        break;
                    }
                }
            }
            match done {
                Some(Ok(mut d)) => {
                    // a large .xml finished loading → now auto-detect its schema version from the head
                    if self.tabs[i].is_xml() {
                        if let Some(si) =
                            Self::detect_schema_idx(&String::from_utf8_lossy(&d.read_bytes(0, 4096)))
                        {
                            self.tabs[i].schema_idx = si;
                        }
                    }
                    self.tabs[i].doc = TabDoc::Ready(d);
                    if i == self.active_tab {
                        self.focus_editor = true;
                    }
                }
                Some(Err(e)) => {
                    self.tabs[i].doc = TabDoc::Ready(Box::new(doc::Document::new_empty()));
                    self.error_modal = Some(format!("Open failed: {e}"));
                }
                None => {}
            }
        }
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
                        // the box hugs the actual item count (≤ 9 rows scroll) — no empty
                        // reserved rows under a short list
                        egui::ScrollArea::vertical()
                            .max_height(n.min(max_rows) as f32 * row_h)
                            .auto_shrink([false, true])
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
                    if close_x(ui, "Close") {
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

}
