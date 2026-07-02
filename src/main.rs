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
mod proc; // background-process scaffold for a tab (search)
mod search; // background document search → grid
mod sqlentity; // SQL → result-tab label (the query's key entity; heuristic, first cut)
#[cfg(test)]
mod sample; // demo data for the result-grid tests only (not shipped in the product)
mod icons;
mod startup;
mod theme;
mod update;
mod vscroll;
mod widgets;
mod winchrome;
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
    pub const FETCH_NEXT: &str = icons::CHEVRON_DOWN;
    pub const FETCH_ALL: &str = icons::CHEVRONS_DOWN;
    pub const REFRESH: &str = icons::REFRESH;
    pub const EXPAND: &str = icons::CHEVRONS_UP;
    pub const COLLAPSE: &str = icons::CHEVRONS_DOWN;
    pub const MANAGER: &str = icons::PANEL_LEFT;
    pub const META: &str = icons::PANEL_TREE;
    pub const PLUS: &str = icons::PLUS;
    pub const SEARCH: &str = icons::FIND;
    // SCAN chip: ONE refresh glyph in every state — the COLOUR carries the state, not the glyph
    // (icons/README: "refresh — metadata dock: rescan; status scan"). Used by the About tab and the
    // collector indicator in the meta-manager modal.
    pub const SCAN: &str = icons::REFRESH;
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
            .with_icon(std::sync::Arc::new(app_icon()))
            .with_min_inner_size(egui::vec2(1024.0, 600.0)),
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
                doc::cleanup_temp_dir(24 * 3600); // sweep orphaned doc temp files (older than a day)
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

/// Polling cadence for the background channels in the update loop: while a worker hasn't answered
/// yet we repaint at this delay (~10 Hz) instead of pinning max FPS. One constant for every poll
/// site (exec / test / connect / metadata / proc / file-load / update) — "100" used to be
/// hardcoded in ten places. The workers use `recv_timeout` internally with the same interval.
pub(crate) const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Ask egui to repaint after [`POLL_INTERVAL`] — the standard "still waiting on a background reply"
/// tail of a poll loop. Takes `&Context` (not `&Ui`) so it can be called from methods that only have ctx.
pub(crate) fn request_poll(ctx: &egui::Context) {
    ctx.request_repaint_after(POLL_INTERVAL);
}

/// An integer with digit-group separators (narrow no-break space) — for finding / match counters.
fn fmt_thousands(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(*ch as char);
    }
    out
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

/// One grid in the result panel — a SQL result set or a one-row command/error status. The grid
/// display model (columns/widths/order) + every fetched row (each cell already a
/// string). `title` is its tab label; `err` paints rows in danger; `goto_line` links a click back
/// to a 1-based editor line; `sql` (non-empty) makes it Refresh-able.
pub(crate) struct ResultSet {
    pub title: String,
    pub gm: grid::GridModel, // columns / widths / order — the grid model
    pub rows: Vec<Vec<String>>, // every row fetched SO FAR (a lazy grid grows this on demand)
    // ---- lazy fetch (the last SELECT of a run, streamed on demand via COPY TO STDOUT) ----
    pub lazy: bool,  // this grid is a live lazy stream (fetch buttons apply) vs a fully-buffered grid
    pub more: bool,  // more rows remain on the server (the stream isn't exhausted) — buttons armed
    pub fetching: bool, // a fetch command is in flight (buttons disabled until it settles)
    pub stale: bool,    // this result advertises more server-side rows but has NO live stream to fetch
    // them — an idle-closed lazy stream, or a non-last statement's capped first page (drained so the
    // batch could move on). The fetch buttons stay ACTIVE as an affordance, but clicking one shows a
    // modal explaining there's nothing to fetch here and then disarms them (clears `stale`). Distinct
    // from a fully-fetched result (nothing more) and from a live, still-fetchable stream.
    pub sql: String,    // the statement that produced this result (for per-result Refresh; "" = none)
    pub scroll: (f64, f64), // grid scroll (f64 px on both axes)
    pub fade: vscroll::Fade, // disappearing-overlay scrollbar fade — per result-tab, next to `scroll`
    pub err: bool,          // paint rows as an error (the error status grid)
    pub goto_line: Option<usize>, // clicking a row jumps to this 1-based editor line
    pub truncated: bool,    // only part is shown (100 MB cap, or a non-last batch SELECT capped to a page)
    // ---- client-side sort (over the rows fetched so far; the lazy stream re-sorts as rows arrive) ----
    pub sort: Vec<(usize, bool)>, // active sort keys (data column index, descending); order = priority
    pub view: Vec<usize>,         // visible row → index into `rows`; empty = identity (unsorted)
    pub view_dirty: bool,         // `view` needs rebuilding (sort changed, or rows grew under a sort)
    // ---- status model (result-tab glyph / count / timer) ----
    pub is_status: bool, // a synthetic command/error status grid (not real data → counts as "0 rows")
    pub load_elapsed: Option<std::time::Duration>, // CUMULATIVE wall time (run start → this result's first page)
    pub fetch_elapsed: std::time::Duration, // doscroll-fetch time accrued on THIS sheet (status-bar timer)
}

/// Idle timeout for a live lazy stream: after this long with no fetch/interaction we cancel the
/// stream (it holds a server snapshot/locks the whole time) but keep the connection. Future: a
/// per-connection setting.
const LAZY_IDLE_SECS: u64 = 300;

/// Default on-screen rows for a fresh result panel: the panel auto-sizes so exactly this many rows
/// fit, until the user drags the panel to their own size.
const DEFAULT_RESULT_ROWS: usize = 10;

/// The live lazy grid in a panel (the most recent `Data` sheet flagged `lazy`), if any — the target
/// for streamed `LazyRows` and the fetch-state flags.
fn lazy_grid_mut(panel: &mut [ResultTab]) -> Option<&mut ResultSet> {
    panel.iter_mut().rev().find_map(|s| match s {
        ResultTab::Data(rs) if rs.lazy => Some(rs),
        _ => None,
    })
}

/// Width of each column = widest of (header, first 200 values) in chars → points (clamped).
fn grid_widths(columns: &[String], rows: &[Vec<String>]) -> Vec<f32> {
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
    widths
}

impl ResultSet {
    pub(crate) fn new(columns: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        let widths = grid_widths(&columns, &rows);
        let col_order = (0..columns.len()).collect();
        Self {
            title: String::new(),
            gm: grid::GridModel { columns, widths, col_order },
            rows,
            lazy: false,
            more: false,
            fetching: false,
            stale: false,
            sql: String::new(),
            scroll: (0.0, 0.0),
            fade: vscroll::Fade::default(),
            err: false,
            goto_line: None,
            truncated: false,
            sort: Vec::new(),
            view: Vec::new(),
            view_dirty: false,
            is_status: false,
            load_elapsed: None,
            fetch_elapsed: std::time::Duration::ZERO,
        }
    }

    /// Whole-pill tint for this sheet's result tab: **error → red**; everything else → **neutral**
    /// (`None` = the default active/inactive colour, like the editor tabs — no green).
    fn status_color(&self) -> Option<egui::Color32> {
        if self.err { Some(p().danger) } else { None }
    }

    /// A one-row status grid: a command outcome (`ok`) or an error (`!ok`). Columns Status / Line /
    /// Message; the row links back to `line` (1-based; 0 = unknown). Tab label = `label`.
    fn status(ok: bool, label: &str, line: usize, text: &str) -> Self {
        let cols = vec!["Status".to_owned(), "Line".to_owned(), "Message".to_owned()];
        let ln = if line > 0 { line.to_string() } else { "—".to_owned() };
        let st = if ok { "OK" } else { "Error" };
        let mut rs = Self::new(cols, vec![vec![st.to_owned(), ln, text.to_owned()]]);
        rs.err = !ok;
        rs.is_status = true; // synthetic status grid → its tab reads "0 rows", not the synthetic row count
        rs.goto_line = (line > 0).then_some(line);
        rs.title = label.to_owned();
        rs.sql = String::new(); // a status row isn't refreshed by the Refresh button
        rs
    }

    /// Cycle the sort on a column. Plain click (`!additive`): a fresh column → ascending; the same
    /// single column → asc → desc → cleared. Ctrl/Cmd click (`additive`): toggle just this column
    /// asc → desc → removed, keeping the other keys (a new column joins at the lowest priority).
    pub(crate) fn toggle_sort(&mut self, col: usize, additive: bool) {
        let existing = self.sort.iter().position(|&(c, _)| c == col);
        if additive {
            match existing {
                Some(i) if !self.sort[i].1 => self.sort[i].1 = true,
                Some(i) => {
                    self.sort.remove(i);
                }
                None => self.sort.push((col, false)),
            }
        } else if self.sort.len() == 1 && existing == Some(0) {
            if !self.sort[0].1 {
                self.sort[0].1 = true;
            } else {
                self.sort.clear();
            }
        } else {
            self.sort.clear();
            self.sort.push((col, false));
        }
        self.view_dirty = true;
    }

    /// Rebuild the display permutation from the active sort keys (stable; numeric-aware; NULL "—"
    /// last on ascending). Empty `sort` → identity (`view` cleared). Clears the dirty flag.
    pub(crate) fn rebuild_view(&mut self) {
        self.view_dirty = false;
        if self.sort.is_empty() {
            self.view.clear();
            return;
        }
        let (rows, keys) = (&self.rows, &self.sort);
        let mut idx: Vec<usize> = (0..rows.len()).collect();
        idx.sort_by(|&a, &b| cmp_rows(&rows[a], &rows[b], keys));
        self.view = idx;
    }

    /// Map a visible row index to its underlying `rows` index (identity when unsorted).
    #[inline]
    pub(crate) fn data_row(&self, r: usize) -> usize {
        if self.view.is_empty() {
            r
        } else {
            self.view.get(r).copied().unwrap_or(r)
        }
    }
}

/// Order two rows by the sort keys (data column index, descending) — first key wins, ties fall
/// through to the next.
fn cmp_rows(a: &[String], b: &[String], keys: &[(usize, bool)]) -> std::cmp::Ordering {
    for &(c, desc) in keys {
        let ord = cmp_cell(a.get(c).map_or("", |s| s.as_str()), b.get(c).map_or("", |s| s.as_str()));
        let ord = if desc { ord.reverse() } else { ord };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

/// Compare two cell strings: NULL ("—") sorts greatest (so it lands last when ascending, matching
/// PostgreSQL's default NULLS LAST); otherwise numeric when both parse as numbers, else byte-wise.
fn cmp_cell(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a == "—", b == "—") {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            if let (Ok(x), Ok(y)) = (a.trim().parse::<i128>(), b.trim().parse::<i128>()) {
                return x.cmp(&y);
            }
            if let (Ok(x), Ok(y)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
                if let Some(o) = x.partial_cmp(&y) {
                    return o;
                }
            }
            a.cmp(b)
        }
    }
}

/// One sheet of the bottom result panel — all kinds live in a single `Vec` shown by one tab strip.
/// Run (SQL Execute) clears the list; Find owns a single "Find" sheet
/// (re-running replaces it, never piles up).
pub(crate) enum ResultTab {
    /// A data grid: SQL result set, or a one-row command/error status.
    Data(ResultSet),
    /// The search-results sheet (labelled "Find") — clicking a row jumps to that editor line.
    Probe(proc::Results),
}

impl ResultTab {
    /// The label of the single Find sheet in the result tab strip.
    const FIND_TITLE: &'static str = "Find";

    /// The tab-strip label.
    fn title(&self) -> String {
        match self {
            ResultTab::Data(rs) => {
                if rs.title.is_empty() { "Result".to_owned() } else { rs.title.clone() }
            }
            ResultTab::Probe(_) => Self::FIND_TITLE.to_owned(),
        }
    }
}

/// A tab's document: loading in the background or ready. `Detached` — temporarily taken by the editor.
pub(crate) enum TabDoc {
    Loading { rx: std::sync::mpsc::Receiver<doc::LoadMsg>, progress: u8 },
    Ready(Box<doc::Document>),
    Detached,
}

/// What kind of tab this is — the single source of truth, a flat list. The SQL editor, a
/// connection-settings form, an object-metadata view, and the two singleton pages (About,
/// Scan). The Connection / Meta variants carry their own payload (no separate option fields).
enum TabKind {
    Sql,
    Connection(Connection),
    Meta(metadata::MetaObject),
    About,
    /// The metadata-scan controls (enable/disable/apply, interval/budget, monitored schemas, log).
    /// Opened from the status-bar `scan` chip; a singleton. (The live control-connection view lives
    /// on the active connection's settings tab, not a separate page.)
    Scan,
}

/// A run deferred until the tab's current lazy stream closes: the split statements (each with its
/// 1-based source line) plus the optional result-sheet index to refresh in place.
type PendingRun = (Vec<(String, usize)>, Option<usize>);

/// One tab. Most are SQL text editors; the `kind` discriminates the connection-settings form,
/// the metadata view and the About/Scan pages. The editor state fields (`doc`, `ed`, `lex`,
/// `panel`, …) are kept flat and are simply unused by the non-editor kinds.
struct Tab {
    id: u64, // stable id → egui remembers caret + scroll per tab
    title: String,
    doc: TabDoc, // the text lives in the document model (piece table + mmap)
    kind: TabKind,         // SQL / Connection / Meta / About / Scan — the tab's type
    path: Option<PathBuf>, // backing file (.sql), if opened from / saved to disk
    conn_dirty: bool, // unsaved edits to the connection FORM (SQL tabs check doc.modified())
    // ---- the single bottom result panel: SQL grids, status/errors, search results ----
    // Run (SQL Execute) clears the list; Search adds a sheet.
    panel: Vec<ResultTab>,
    panel_active: usize, // index of the active sheet in `panel`
    result_height: f32, // result-panel height lives with the tab, not globally
    result_height_user_set: bool, // the user dragged the panel size → stop auto-sizing it to fit 10 rows
    result_full: bool,  // result panel maximized — also per-tab, not shared
    // whole rows that fit in THIS tab's result grid, captured on each render. Drives the lazy
    // first-page size + доскролл page; per-tab so each tab remembers its own panel capacity (a wider
    // panel on one tab no longer dictates another's first page). 0 until its grid has rendered once.
    last_visible_rows: usize,
    // The first-page row count captured when the current lazy stream was launched. `Fetch next` pulls
    // exactly this many EVERY time — a stable page size that does NOT drift when the panel is resized
    // after the run (unlike last_visible_rows, which recomputes per-frame from the live geometry).
    fetch_page: usize,
    running: bool,      // a query is executing on this tab's session connection
    // this tab's own session connection (None until the first query is run on it; kept open
    // afterwards so SET / temp tables / prepared statements persist between queries). It is
    // checked out into the worker thread while a query runs.
    client: Option<postgres::Client>,
    exec_rx: Option<std::sync::mpsc::Receiver<connections::ExecMsg>>, // in-flight query stream for this tab
    exec_cancel: Option<postgres::CancelToken>,          // out-of-band Stop for this tab's query
    exec_start: Option<std::time::Instant>,              // query timer for this tab
    // ---- status model (tab glyphs / live timer) ----
    run_timing: bool, // the INITIAL run is in flight: gates the editor spinner + live timer. A doscroll
    // re-sets `running` but NOT this, so the editor timer doesn't re-tick on fetch.
    stop_requested: bool, // a Stop while executing → render the cancel as a RED "Query cancelled" result
    // (a doscroll-phase Stop is a pause instead; this flag is cleared at the execution→fetch transition)
    run_stmt_count: usize, // statements in the current run → a spinner tab for each not-yet-produced one
    run_labels: Vec<String>, // per-statement entity labels of the current run → named placeholder tabs
    fetch_start: Option<std::time::Instant>, // the current doscroll fetch's start (folded into the sheet's fetch_elapsed)
    // ---- lazy stream control: the last SELECT of a run is fetched on demand via COPY TO STDOUT ----
    fetch_tx: Option<std::sync::mpsc::Sender<connections::FetchCmd>>, // commands to the live stream worker
    fetch_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>, // lightning pause for "fetch all"
    last_fetch: Option<std::time::Instant>, // last fetch / grid interaction (drives the idle timeout)
    // A run (Execute/Refresh) issued while a lazy stream is still open is deferred here; it fires once
    // the worker returns the session connection via `Done` (so the new run reuses the same session).
    pending_exec: Option<PendingRun>,
    // Some(i) while a single-result Refresh is in flight → the streamed Result replaces
    // panel[i] in place instead of being appended
    refresh_idx: Option<usize>,
    ed: codeeditor::EditorState, // caret / selection / scroll for the SQL editor
    lex: codeeditor::LexCache,   // highlight states at line boundaries
    /// Search-match highlight: line → [(column, length in characters)].
    search_hl: std::collections::HashMap<usize, Vec<(usize, usize)>>,
    /// Editor jump/selection (anchor, caret) on the next frame (0-based).
    pending_goto: Option<(doc::Pos, doc::Pos)>,
    // ---- background processes ----
    /// The tab's current background process (search); at most one.
    proc: Option<proc::RunningProc>,
    /// The `panel` sheet the current process appends search matches into.
    proc_target: Option<usize>,
    /// This tab's process execution status for the status bar: (text, error?). Bound to the editor
    /// tab, not to a result sheet (SQL run / Find).
    proc_status: Option<(String, bool)>,
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
            panel: Vec::new(),
            panel_active: 0,
            result_height: 300.0,
            result_height_user_set: false,
            result_full: false,
            last_visible_rows: 0,
            fetch_page: DEFAULT_RESULT_ROWS,
            running: false,
            client: None,
            exec_rx: None,
            exec_cancel: None,
            exec_start: None,
            run_timing: false,
            stop_requested: false,
            run_stmt_count: 0,
            run_labels: Vec::new(),
            fetch_start: None,
            fetch_tx: None,
            fetch_stop: None,
            last_fetch: None,
            pending_exec: None,
            refresh_idx: None,
            ed: codeeditor::EditorState::default(),
            lex: codeeditor::LexCache::default(),
            search_hl: std::collections::HashMap::new(),
            pending_goto: None,
            proc: None,
            proc_target: None,
            proc_status: None,
        }
    }

    /// The active result panel sheet, if any.
    fn cur_panel(&self) -> Option<&ResultTab> {
        self.panel.get(self.panel_active.min(self.panel.len().saturating_sub(1)))
    }
    /// The active result panel sheet as a data grid (None for a Probe sheet or empty panel).
    fn cur_data(&self) -> Option<&ResultSet> {
        match self.cur_panel()? {
            ResultTab::Data(rs) => Some(rs),
            ResultTab::Probe(_) => None,
        }
    }
    fn cur_data_mut(&mut self) -> Option<&mut ResultSet> {
        let i = self.panel_active.min(self.panel.len().saturating_sub(1));
        match self.panel.get_mut(i)? {
            ResultTab::Data(rs) => Some(rs),
            ResultTab::Probe(_) => None,
        }
    }
    /// The panel is shown while a query runs, while a process runs, or whenever it holds a sheet.
    fn panel_visible(&self) -> bool {
        self.running || self.proc.is_some() || !self.panel.is_empty()
    }

    /// This tab's leading mark in the editor tab strip: a neutral tab-kind glyph (+ a spinner while a
    /// SQL run is in flight). The run outcome is shown on the **result** tabs (whole-pill colour), not
    /// on the editor glyph.
    fn editor_mark(&self) -> widgets::TabMark {
        let glyph = match &self.kind {
            TabKind::Sql => ic::NEW,
            TabKind::Connection(_) => ic::CONNECT,
            TabKind::Meta(_) => ic::META,
            TabKind::About => ic::SCAN,
            TabKind::Scan => ic::SCAN,
        };
        let spinning = matches!(self.kind, TabKind::Sql) && self.run_timing;
        widgets::TabMark { spinning, glyph, tint: None }
    }

    /// This tab's editor-strip label: `title *` (the unsaved marker). `unsaved` is supplied by the
    /// caller (`App::tab_unsaved`) rather than read from `dirty()` here, so the Scan tab — whose
    /// "unsaved" state lives in `App`, not the `Tab` — gets a `*` too. The run timer now lives in
    /// the status bar, not the tab title; the spinner still marks a running tab.
    fn editor_tab_label(&self, unsaved: bool) -> String {
        if unsaved {
            format!("{} *", self.title)
        } else {
            self.title.clone()
        }
    }
    /// Drop every result sheet (Run starts fresh; closing the panel clears it).
    fn clear_panel(&mut self) {
        self.panel.clear();
        self.panel_active = 0;
    }

    /// Land a streamed result sheet: a single-result Refresh (`refresh_idx` targets an existing
    /// data sheet) replaces that grid in place, keeping its title and scroll; otherwise the sheet
    /// is appended. Shared by `ExecMsg::Result` and `ExecMsg::LazyBegin`.
    fn land_result(&mut self, mut rs: ResultSet) {
        match self.refresh_idx {
            Some(ri) if matches!(self.panel.get(ri), Some(ResultTab::Data(_))) => {
                if let Some(ResultTab::Data(old)) = self.panel.get(ri) {
                    rs.title = old.title.clone();
                    rs.scroll = old.scroll;
                }
                self.panel[ri] = ResultTab::Data(rs);
            }
            _ => self.panel.push(ResultTab::Data(rs)),
        }
    }
    /// Append a result sheet and make it active; returns its index.
    fn push_panel(&mut self, sheet: ResultTab) -> usize {
        self.panel.push(sheet);
        self.panel_active = self.panel.len() - 1;
        self.panel_active
    }

    /// Insert or replace the single Find sheet (a re-run replaces it, never piles up).
    /// Makes it active; returns its index.
    fn upsert_probe(&mut self, res: proc::Results) -> usize {
        let at = self.panel.iter().position(|s| matches!(s, ResultTab::Probe(_)));
        match at {
            Some(i) => {
                self.panel[i] = ResultTab::Probe(res);
                self.panel_active = i;
                i
            }
            None => self.push_panel(ResultTab::Probe(res)),
        }
    }

    /// True for an ordinary text-editor tab (SQL) — i.e. not a connection-settings,
    /// metadata, About or Scan tab. Used to gate Ln/Col, Save, find, the editor toolbar.
    fn is_editor(&self) -> bool {
        matches!(self.kind, TabKind::Sql)
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

    /// Temporarily take the document (for code_editor, to avoid conflicting with &mut self).
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

    /// Unsaved changes: the connection form has its own flag, the editor uses the document; the
    /// About / Scan / Meta pages don't edit a document, so they are always "clean".
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

    /// The full text of the SQL buffer (for execution). None — the document is busy/huge.
    fn full_sql(&mut self) -> Option<String> {
        let Tab { doc, .. } = self;
        let TabDoc::Ready(d) = doc else { return None };
        let last = d.line_count() - 1;
        let e = d.line_length(last);
        d.get_text_range((0, 0), (last, e)).ok()
    }

    /// Clear the run/status-model flags that the exec-stream drain loop normally resets on a terminal
    /// message (`Done` / `LazyEnd` / `LazyMore`). Must be called whenever a run is abandoned WITHOUT
    /// draining the stream (see `reset_session`), or the tab keeps a stale spinner / placeholder tab /
    /// timer forever (those readers are gated on `run_timing`).
    fn clear_run_state(&mut self) {
        self.run_timing = false;
        self.stop_requested = false;
        self.run_stmt_count = 0;
        self.run_labels.clear();
        self.fetch_start = None;
    }

    /// Drop this tab's session connection and abandon any in-flight query, returning the tab to
    /// the "no session" state. Shared by connect / disconnect / kill so a (re)connect starts clean
    /// (dropping `exec_rx` makes the worker's sends fail, so it drops its client when it finishes).
    fn reset_session(&mut self) {
        // abort an in-flight query / lazy COPY stream server-side (the worker may be blocked reading)
        if let Some(c) = self.exec_cancel.take() {
            connections::spawn_cancel(c);
        }
        self.client = None;
        self.exec_rx = None;
        self.exec_cancel = None;
        self.exec_start = None;
        self.fetch_tx = None; // drop the command sender → a parked lazy worker unblocks and exits
        self.fetch_stop = None;
        self.last_fetch = None;
        self.pending_exec = None; // a deferred run can't fire once the session is gone — drop it
        self.running = false;
        self.refresh_idx = None;
        // the worker's `Done` (which clears these) never arrives once `exec_rx` is dropped, so reset
        // the status-model fields here — otherwise a tab killed mid-run spins / shows a phantom timer.
        self.clear_run_state();
    }
}

/// Result of the background control-session connect: `(client, backend_pid, ssl)` or an error string.
type ConnectResult = Result<(postgres::Client, Option<i32>, Option<bool>), String>;

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
    // live attributes of `main_conn`, captured once at connect time and shown in the Session block
    // on the active connection's settings tab.
    // `main_pid` is the backend pid of the CONTROL connection (always free — the kill-switch);
    // `main_conn_since` is its wall-clock start as "HH:MM:SS" (absolute, captured at connect via
    // `dialog::now_hms()` — no chrono dependency); `main_ssl` is whether that connection is over
    // TLS. All three reset to None on disconnect / break / reconnect (a reconnect opens a new
    // session → new pid).
    main_pid: Option<i32>,
    main_conn_since: Option<String>,
    main_ssl: Option<bool>,
    // in-flight main connect (background thread) → Ok((client, pid, ssl)) / Err(message)
    connect_rx: Option<std::sync::mpsc::Receiver<ConnectResult>>,
    pending_label: String,             // "user@db" to show once the in-flight connect succeeds
    busy_prompt: Option<PendingConn>,  // connect/disconnect waiting on a "kill running work?" prompt
    // resolved credentials of the active connection, captured at Connect time. `main_conn` is the
    // control connection; each tab opens its OWN session connection (lazily, on first run) from
    // these params so tabs execute independently and keep session state between queries.
    conn_params: Option<connections::ConnParams>,
    grid_sel: Option<GridSel>,         // cell selection in the active result grid (for copy)
    grid_rows: std::collections::BTreeSet<usize>, // whole-row selection (visible indices) via the # gutter
    grid_row_anchor: Option<usize>,    // anchor row for Alt range-extend of the row selection
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
    last_error: Option<String>, // last caught panic message — a latch to surface it once in the error modal
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
    // last active-tab index the strip scrolled to view. When `active_tab` changes for any reason
    // OTHER than a click on a visible pill (open / new / from a manager / Ctrl-Tab), it won't match
    // → the strip scrolls the now-active pill into view so its header is visible even when the row
    // overflows. A pill click keeps them in sync (the clicked pill is already visible), so clicks
    // never tug the row.
    tab_last_active: usize,
    // last tab-strip viewport width. When it shrinks (the dock opens/resizes, or the window
    // resizes) the active pill can be pushed off the right edge, so a change here also scrolls it
    // back into view — the leading tabs slide out of sight ("under" the dock) instead.
    tab_last_view_w: f32,
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
            main_pid: None,
            main_conn_since: None,
            main_ssl: None,
            connect_rx: None,
            pending_label: String::new(),
            busy_prompt: None,
            conn_params: None,
            grid_sel: None,
            grid_rows: std::collections::BTreeSet::new(),
            grid_row_anchor: None,
            startup_frame: 0,
            confirm: None,
            disconnect_confirm: false,
            allow_close: false,
            update_status: update::UpdateStatus::NeverChecked,
            update_outdated: None,
            update_rx: None,
            last_error: None,
            error_modal: None,
            test_rx: None,
            test_result: None,
            editor_rect: egui::Rect::ZERO,
            find_open: false,
            find_query: String::new(),
            find_focus: false,
            tab_scroll: 0.0,
            tab_overflow: false,
            tab_last_active: 0,
            tab_last_view_w: 0.0,
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
    /// The selected text in the active SQL editor plus its 0-based start line (to offset
    /// per-statement source lines when only a selection is executed/validated).
    fn editor_selection_lined(&mut self) -> Option<(String, usize)> {
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
        let text = ed.selection_text(d).ok()?;
        Some((text, ed.sel_start_line()))
    }
    /// The active text-editor tab (SQL; not a connection / metadata tab), mutably.
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
    /// The active tab's result-panel labels + leading marks (one per sheet). A SQL data sheet gets a
    /// table glyph tinted by state with `(N rows)` / `(N… rows)`; while it loads the first page of the
    /// initial run it spins and shows `(elapsed)`. The Probe (Find) sheet keeps the find glyph.
    fn result_strip(&self) -> (Vec<String>, Vec<widgets::TabMark>) {
        let Some(t) = self.cur() else { return (Vec::new(), Vec::new()) };
        let mut labels = Vec::with_capacity(t.panel.len() + 1);
        let mut marks = Vec::with_capacity(t.panel.len() + 1);
        let mut data_count = 0usize;
        for sheet in &t.panel {
            match sheet {
                ResultTab::Data(rs) => {
                    data_count += 1;
                    let base = if rs.title.is_empty() { "Result" } else { rs.title.as_str() };
                    if rs.fetching && t.run_timing {
                        // still loading (initial first page, or a Refresh) → spinner; name only (the
                        // run timer lives in the status bar now)
                        labels.push(base.to_owned());
                        marks.push(widgets::TabMark { spinning: true, glyph: ic::OBJ_TABLE, tint: None });
                    } else {
                        // settled sheet: name + the row count in brackets — `0` for a command/error/empty
                        // set, `…` when more rows remain on the server. Pill is neutral; only errors are red.
                        let n = if rs.is_status { 0 } else { rs.rows.len() };
                        let ell = if !rs.is_status && (rs.more || rs.truncated) { "…" } else { "" };
                        labels.push(format!("{base} [{n}{ell}]"));
                        marks.push(widgets::TabMark {
                            spinning: false,
                            glyph: ic::OBJ_TABLE,
                            tint: rs.status_color(),
                        });
                    }
                }
                ResultTab::Probe(_) => {
                    labels.push(sheet.title());
                    marks.push(widgets::TabMark { spinning: false, glyph: icons::FIND, tint: None });
                }
            }
        }
        // a statement that has started but not yet produced its sheet (a slow query still computing
        // its first row) still gets ONE spinner tab — so each pending statement appears immediately,
        // not only after it finishes
        // once a statement has errored the batch stops there, so the remaining statements won't run —
        // suppress the pending placeholder (avoids a phantom spinner tab in the frame before Done lands)
        let aborted = matches!(t.panel.last(), Some(ResultTab::Data(rs)) if rs.err);
        if t.run_timing && !aborted && data_count < t.run_stmt_count {
            // name the pending tab from the statement's entity immediately (not a generic "Result")
            let base = t.run_labels.get(data_count).map(|s| s.as_str()).unwrap_or("Result");
            labels.push(base.to_owned());
            marks.push(widgets::TabMark { spinning: true, glyph: ic::OBJ_TABLE, tint: None });
        }
        (labels, marks)
    }
    /// The status-bar run timer for the **active result tab** of the active SQL tab: that one result's
    /// own time in seconds (microsecond precision) — its execution plus the doscroll fetches accrued on
    /// it (live while loading / fetching, frozen after). It is **not** a sum across statements: each
    /// result tab carries its own time, and doscrolling one only grows that tab's value. `None` until a
    /// run has started, or on a non-SQL tab.
    fn run_timer_text(&self) -> Option<String> {
        use std::time::Duration;
        let t = self.cur()?;
        if !matches!(t.kind, TabKind::Sql) {
            return None;
        }
        let n = t.panel.len();
        // the active "tab" may be the pending spinner placeholder for a statement that has STARTED but
        // not yet produced its sheet — its index sits just past the real sheets (`panel_active >= n`,
        // exactly as `result_body` detects it to show the Running pill). Its timer is the live elapsed
        // of the currently-executing statement, so it must tick even while the user sits on that tab.
        let on_pending = t.run_timing && t.panel_active >= n;
        let active = t.panel_active.min(n.saturating_sub(1));
        // cumulative load time of the Data sheet BEFORE the active one (0 if none) — or the last
        // completed sheet when on the pending placeholder — subtracting it leaves the active
        // statement's OWN execution time, so the timer never sums across statements.
        let prev = t
            .panel
            .iter()
            .take(if on_pending { n } else { active })
            .filter_map(|s| match s {
                ResultTab::Data(rs) => rs.load_elapsed,
                _ => None,
            })
            .next_back()
            .unwrap_or_default();
        let live_exec = t.exec_start.map(|s| s.elapsed().saturating_sub(prev)).unwrap_or_default();
        let total = if on_pending {
            live_exec // viewing the spinner placeholder of the statement still executing → ticking
        } else {
            match t.panel.get(active) {
                Some(ResultTab::Data(rs)) => {
                    if rs.fetching && rs.load_elapsed.is_none() {
                        live_exec // still loading the first page → ticking
                    } else {
                        let own = rs.load_elapsed.map_or(Duration::ZERO, |c| c.saturating_sub(prev));
                        let live_fetch = if rs.fetching {
                            t.fetch_start.map(|s| s.elapsed()).unwrap_or_default() // a doscroll of THIS tab
                        } else {
                            Duration::ZERO
                        };
                        own + rs.fetch_elapsed + live_fetch
                    }
                }
                // no Data sheet yet (a slow query still computing) → the current statement's elapsed
                _ if t.run_timing => live_exec,
                _ => return None,
            }
        };
        Some(format!("[{:.6} sec]", total.as_secs_f64()))
    }
    /// The active result sheet as a data grid (None on a Probe sheet / empty panel).
    fn cur_data(&self) -> Option<&ResultSet> {
        self.cur().and_then(|t| t.cur_data())
    }
    /// Mutable form of [`cur_data`].
    fn cur_data_mut(&mut self) -> Option<&mut ResultSet> {
        self.cur_mut().and_then(|t| t.cur_data_mut())
    }
    /// True when the active tab is a text editor (SQL; not a connection / metadata tab).
    fn is_editor_tab(&self) -> bool {
        self.cur().is_some_and(|t| t.is_editor())
    }
    /// True when the active tab is a SQL editor specifically (gates Execute / autocomplete).
    fn is_sql_tab(&self) -> bool {
        self.cur().is_some_and(|t| matches!(t.kind, TabKind::Sql))
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
    /// True when the active tab is the Scan page (the metadata-collector controls).
    fn is_scan_tab(&self) -> bool {
        self.cur().is_some_and(|t| matches!(t.kind, TabKind::Scan))
    }

    /// True when the active tab has UNSAVED work to save — Save (toolbar / menu / Ctrl+S) is dimmed
    /// otherwise, so it never offers to save a tab with no pending changes. By kind: SQL → the
    /// document is modified; Connection → the form is edited (`conn_dirty`); Scan → there are
    /// staged settings to Apply. Meta / About have nothing to save.
    fn can_save(&self) -> bool {
        let Some(t) = self.cur() else { return false };
        match &t.kind {
            TabKind::Sql => t.dirty(),
            TabKind::Connection(_) => t.conn_dirty,
            TabKind::Scan => self.can_apply_scan(),
            TabKind::Meta(_) | TabKind::About => false,
        }
    }

    /// True when tab `i` has unsaved work: the document / form dirty flag (`Tab::dirty`), OR — for
    /// the Scan tab, whose pending state lives in `App` not the `Tab` — staged scan settings not yet
    /// applied. Drives the `*` strip marker, the close-confirm and the quit-confirm, so a Scan tab
    /// with a lit Save isn't closed silently.
    fn tab_unsaved(&self, i: usize) -> bool {
        self.tabs.get(i).is_some_and(|t| {
            t.dirty() || (matches!(t.kind, TabKind::Scan) && self.can_apply_scan())
        })
    }

    /// True when the active tab has a "Save As" / Export target: a text editor (SQL → save to a
    /// new file) or a connection tab (→ Export the connection to a `.conn` file). Other pages
    /// (About, Scan, metadata) have no Save-As target → the toolbar slot is dimmed.
    fn can_save_as(&self) -> bool {
        self.cur().is_some_and(|t| {
            t.is_editor() || matches!(t.kind, TabKind::Connection(_))
        })
    }

    /// Run the active SQL tab on ITS OWN session connection, on a background thread. The connection
    /// is opened lazily on the first run and kept open afterwards (so session state persists), which
    /// also lets other tabs run concurrently. The panel fills with one sheet per statement (data
    /// grids / one-row status sheets) as they stream in; the panel opens on the first. The UI stays
    /// responsive during the query.
    fn execute(&mut self, _ctx: &egui::Context) {
        if !self.is_sql_tab() || self.tab_busy() {
            return; // not a SQL tab, or a process is already running on it (search/query blocks the launch)
        }
        let idx = self.active_tab;
        if self.conn_params.is_none() {
            self.error_modal = Some("Not connected. Connect to a database first.".to_owned());
            return;
        }
        // run the selection if there is one, otherwise the whole tab (carry the selection's base
        // line so per-statement source lines map back to the right editor line)
        let (sql, base_line) = match self.editor_selection_lined() {
            Some((s, l)) => (s, l),
            None => (self.cur_mut().and_then(|t| t.full_sql()).unwrap_or_default(), 0),
        };
        if sql.trim().is_empty() {
            return;
        }
        self.clear_grid_selection();
        // (statement, 1-based DOC line) — split line is within the executed text; offset by the base
        let statements: Vec<(String, usize)> = connections::split_statements_lines(&sql)
            .into_iter()
            .map(|(s, l)| (s, base_line + l))
            .collect();
        // a PARKED lazy stream still holds this tab's session connection in its worker; close it and
        // defer the run until the connection returns via Done, so the new run reuses the same session.
        if self.tabs[idx].fetch_tx.is_some() {
            self.tabs[idx].pending_exec = Some((statements, None));
            self.tabs[idx].running = true; // mark busy NOW so a second launch can't race in before Done
            self.close_lazy_stream(idx);
            return;
        }
        self.spawn_exec(idx, statements, None);
    }

    /// Launch the background worker for `statements` on tab `idx`, reusing its session connection and
    /// wiring up the result stream + the lazy-fetch command channel + the pause flag. `refresh_idx`
    /// replaces that sheet in place (Refresh) instead of clearing the whole panel.
    fn spawn_exec(&mut self, idx: usize, statements: Vec<(String, usize)>, refresh_idx: Option<usize>) {
        let Some(params) = self.conn_params.clone() else { return };
        // first page = the measured on-screen capacity (the panel auto-sizes to fit DEFAULT_RESULT_ROWS,
        // so this lands exactly); DEFAULT_RESULT_ROWS on the very first run before anything is measured.
        let lvr = self.tabs[idx].last_visible_rows;
        let first_page = if lvr > 0 { lvr } else { DEFAULT_RESULT_ROWS };
        self.tabs[idx].fetch_page = first_page; // freeze the page size for this run's доскролл (п.5)
        let stmt_count = statements.len(); // for the per-statement spinner placeholder tabs
        let (tx, rx) = std::sync::mpsc::channel();
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let existing = {
            let t = &mut self.tabs[idx];
            if refresh_idx.is_none() {
                t.clear_panel(); // Run clears all result sheets (#9)
                t.search_hl.clear();
            }
            t.running = true;
            t.run_timing = true; // the initial run is in flight (editor spinner + live timer)
            t.stop_requested = false;
            t.run_stmt_count = stmt_count;
            // entity label per statement now, so a pending tab shows its name immediately (not "Result")
            t.run_labels = statements.iter().map(|(s, _)| crate::sqlentity::key_entity(s)).collect();
            t.fetch_start = None; // no doscroll in flight at run start
            t.proc_status = None; // run state lives on the tabs now, not the status bar
            t.exec_start = Some(std::time::Instant::now());
            t.refresh_idx = refresh_idx;
            if let Some(ri) = refresh_idx {
                if let Some(ResultTab::Data(rs)) = t.panel.get_mut(ri) {
                    rs.fetching = true; // a Refresh re-runs the spinner + live timer on that result tab
                }
            }
            t.exec_rx = Some(rx);
            t.fetch_tx = Some(cmd_tx);
            t.fetch_stop = Some(std::sync::Arc::clone(&stop));
            t.last_fetch = Some(std::time::Instant::now());
            t.client.take()
        };
        std::thread::spawn(move || {
            connections::run_statements_worker(existing, params, statements, tx, cmd_rx, stop, first_page)
        });
    }

    /// Tell the tab's live lazy stream to close (new run / refresh / disconnect): send `Close` and
    /// fire a CancelRequest in case the worker is blocked mid-fetch. The worker returns the session
    /// connection via `Done`, where any deferred run picks it up.
    fn close_lazy_stream(&mut self, idx: usize) {
        let Some(t) = self.tabs.get_mut(idx) else { return };
        if let Some(tx) = t.fetch_tx.take() {
            let _ = tx.send(connections::FetchCmd::Close);
        }
        if let Some(c) = t.exec_cancel.take() {
            connections::spawn_cancel(c);
        }
        t.fetch_stop = None;
        // the old stream is dead → its grid is no longer a lazy target (so a later stream's LazyRows
        // can't be misrouted to it) and its fetch buttons go inert
        for sheet in &mut t.panel {
            if let ResultTab::Data(rs) = sheet {
                rs.lazy = false;
                rs.more = false;
                rs.fetching = false;
            }
        }
    }

    /// Refresh ONLY the active result sheet: re-run the single statement that produced it on the
    /// tab's session connection, replacing that grid in place (other sheets untouched). No-op on a
    /// non-data sheet or while a query is already running on the tab.
    fn refresh_current_result(&mut self, _ctx: &egui::Context) {
        let idx = self.active_tab;
        let Some(t) = self.tabs.get(idx) else { return };
        if t.running {
            return; // a query / fetch is actively churning on this tab
        }
        let ri = t.panel_active.min(t.panel.len().saturating_sub(1));
        let Some(ResultTab::Data(rs)) = t.panel.get(ri) else { return };
        let sql = rs.sql.clone();
        if sql.trim().is_empty() {
            return; // a status/table/findings sheet isn't refreshed
        }
        if self.conn_params.is_none() {
            self.error_modal = Some("Not connected. Connect to a database first.".to_owned());
            return;
        }
        self.clear_grid_selection();
        let statements = vec![(sql, 0usize)];
        // a parked lazy stream holds the connection — close it and defer the refresh until Done
        if self.tabs[idx].fetch_tx.is_some() {
            self.tabs[idx].pending_exec = Some((statements, Some(ri)));
            self.tabs[idx].running = true; // mark busy NOW so a second launch can't race in before Done
            self.close_lazy_stream(idx);
            return;
        }
        self.spawn_exec(idx, statements, Some(ri));
    }

    /// "Fetch next page": ask the live lazy stream for one more screenful of rows. Uses the page size
    /// frozen at run time (`fetch_page`), NOT the live panel capacity — so every доскролл pulls exactly
    /// as many rows as the first page did, even if the panel has been resized since (п.5).
    fn fetch_more(&mut self) {
        let page = self.cur().map_or(1, |t| t.fetch_page).max(1);
        self.send_fetch(connections::FetchCmd::More(page));
    }

    /// "Fetch to end": pull rows until the end OR the +100 MB pause (the model's bounded "fetch all").
    fn fetch_all(&mut self) {
        self.send_fetch(connections::FetchCmd::All);
    }

    /// Send a fetch command to the active tab's live lazy stream and arm the churn flags so the drain
    /// loop polls the worker and the Stop button becomes a pause.
    fn send_fetch(&mut self, cmd: connections::FetchCmd) {
        let a = self.active_tab;
        let Some(t) = self.tabs.get_mut(a) else { return };
        let Some(tx) = t.fetch_tx.as_ref() else { return };
        if tx.send(cmd).is_err() {
            return;
        }
        t.running = true;
        t.stop_requested = false; // a doscroll fetch starts fresh — a Stop here is a pause, not a cancel
        let now = std::time::Instant::now();
        t.fetch_start = Some(now); // time this doscroll fetch (folded into fetch_elapsed when it settles)
        t.last_fetch = Some(now);
        if let Some(rs) = t.cur_data_mut() {
            rs.fetching = true;
        }
    }

    /// "Lightning": pause an in-flight "fetch to end" — the stream stays open; a later fetch resumes it.
    fn pause_fetch(&mut self) {
        if let Some(t) = self.tabs.get(self.active_tab) {
            if let Some(stop) = &t.fetch_stop {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
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


    /// Open (or re-select) the ACTIVE connection's settings tab — the status-bar identity chip links
    /// here. That page shows the live Session block and locks the connection's fields (it is the
    /// connection whose `id` == [`Self::active_conn_id`]). Does nothing if there is no active
    /// connection or it's no longer in the saved list.
    pub(crate) fn open_active_conn_tab(&mut self) {
        if let Some(id) = self.active_conn_id {
            if let Some(conn) = self.connections.iter().find(|c| c.id == id).cloned() {
                self.open_conn_tab(conn);
            }
        }
    }

    /// Open the Scan (metadata collector) tab. At most one exists: if it's already open this just
    /// re-selects it — **keeping any staged, unapplied edits** (re-selecting must not discard work,
    /// like a normal editor tab keeps its buffer). Only a freshly-created tab syncs the staged
    /// settings from the active connection (so the body opens showing what's actually persisted).
    pub(crate) fn open_scan(&mut self) {
        if let Some(i) = self.tabs.iter().position(|t| matches!(t.kind, TabKind::Scan)) {
            self.active_tab = i;
            return;
        }
        self.reload_meta_edits(); // fresh tab → show what's persisted on the active connection
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id, "Scan".to_owned());
        tab.kind = TabKind::Scan;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
    }


    /// Status-bar connection chip: "login@connection" as a clickable pill. `ok` while connected,
    /// `danger` if the connection dropped. Click → open the active connection's settings tab (which
    /// carries the live Session block). Renders nothing when never connected or deliberately
    /// disconnected (the caller owns the separator and decides whether to call this at all).
    fn conn_chip(&mut self, ui: &mut egui::Ui, sz: f32) {
        let color = if self.connected {
            p().ok // identity reads green while the session is live (Design Delta v2.1 §5)
        } else if self.conn_broken {
            p().danger
        } else {
            return;
        };
        if self.active_label.is_empty() {
            return;
        }
        let tip = format!("{} — open connection", self.active_label);
        let resp = crate::widgets::chip_button(ui, &self.active_label, color, sz);
        if resp.on_hover_text(&tip).clicked() {
            self.open_active_conn_tab();
        }
    }

    /// Status-bar `scan` chip: a clickable "scan" label coloured by the collector's lifecycle
    /// (green active/asleep, amber paused, red failed). Click → open the Scan tab. Shown only while
    /// connected (no live collector to report otherwise); the caller owns the leading separator.
    fn scan_chip(&mut self, ui: &mut egui::Ui, sz: f32) {
        if !self.connected {
            return;
        }
        let (_, _, color, tip) = crate::meta_manager_modal::scan_state(&self.collector_status);
        let resp = crate::widgets::chip_button(ui, "scan", color, sz);
        if resp.on_hover_text(tip).clicked() {
            self.open_scan();
        }
    }

    fn request_close_tab(&mut self, i: usize) {
        if self.tab_unsaved(i) {
            self.confirm = Some(ConfirmAction::CloseTab(i));
        } else {
            self.close_tab(i);
        }
    }

    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        // cancel the tab's background search: otherwise the worker stays a "zombie" — it keeps
        // burning a core and holding the mmap snapshot of the original until the run ends
        if let Some(rp) = self.tabs[i].proc.as_ref() {
            rp.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        // abort a long SQL query with a server-side CancelRequest (the worker returns the client via
        // ExecMsg::Done, but we discard it along with the tab)
        if let Some(cancel) = self.tabs[i].exec_cancel.take() {
            connections::spawn_cancel(cancel);
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
        // Catch a panic in the frame and surface it in the error modal instead of crashing.
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.update_inner(ui, frame);
        }));
        if res.is_err() {
            if let Ok(g) = LAST_PANIC.lock() {
                // A frame panicked and was caught. Surface it ONCE in the error modal — it's rare and
                // alarming, not routine status-bar noise. Dedupe on the message (`last_error` doubles as
                // the "already surfaced" latch) so a panic recurring every frame can't re-trap the modal
                // the instant it's dismissed.
                if self.last_error != *g {
                    self.last_error = g.clone();
                    if let Some(msg) = g.as_ref() {
                        let line = msg.lines().next().unwrap_or("Internal error");
                        self.error_modal = Some(format!(
                            "Something went wrong and the last action was skipped.\n\n{line}"
                        ));
                    }
                }
            }
            ui.ctx().request_repaint();
        }
        // After the whole frame is built, `cursor_icon` is final: if the pointer is over text
        // (our editor or any TextEdit) swap in the themed bitmap I-beam, otherwise the normal
        // cursor stands. Outside the panic guard so a failed frame never leaves a stale cursor.
        #[cfg(windows)]
        startup::apply_themed_ibeam(ui.ctx());
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
            && (0..self.tabs.len()).any(|i| self.tab_unsaved(i))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(ConfirmAction::ExitApp);
        }

        // background loading of large files into the tabs being opened
        self.poll_loading(ctx);
        self.poll_procs(ctx);

        // idle timeout for live lazy streams: a parked stream holds a server snapshot + ACCESS SHARE
        // locks the whole time, and the backend is "active" (no server timeout reaps it) — so WE
        // cancel it after LAZY_IDLE_SECS of no fetch, keeping the connection (the session survives).
        // The grid freezes on what was already fetched; the fetch buttons go inert.
        let idle: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.fetch_tx.is_some()
                    && !t.running
                    && t.last_fetch
                        .is_some_and(|l| l.elapsed() >= std::time::Duration::from_secs(LAZY_IDLE_SECS))
            })
            .map(|(i, _)| i)
            .collect();
        for i in idle {
            // mark the (soon-to-be-closed) lazy grid(s) stale FIRST, so the fetch buttons stay live as
            // an affordance — a click then explains the stream was dropped (no status-bar message).
            for sheet in &mut self.tabs[i].panel {
                if let ResultTab::Data(rs) = sheet {
                    if rs.lazy {
                        rs.stale = true;
                    }
                }
            }
            // close_lazy_stream then clears lazy/more/fetching on the tab's grids (stale survives it)
            self.close_lazy_stream(i);
        }

        // poll each tab's in-flight query thread (tabs run concurrently on their own session
        // connections). Stream data grids / status sheets into the panel; on Done, reclaim and KEEP
        // the tab's session connection open for reuse. We never move the active sheet programmatically
        // mid-run (no focus/scroll steal, #13) — except a fresh Run, which starts on its first sheet.
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
                        Ok(connections::ExecMsg::Done(c)) => {
                            done = Some(c.map(|b| *b));
                            break;
                        }
                        Ok(connections::ExecMsg::Ready(tok)) => ready = Some(tok),
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
                    connections::ExecMsg::Result(mut rs) => {
                        rs.load_elapsed = t.exec_start.map(|s| s.elapsed());
                        // the label is already set by the worker (Select_1, …)
                        t.land_result(rs);
                    }
                    connections::ExecMsg::Status { ok, label, line, text } => {
                        // a user Stop during execution → a clean "Query cancelled" message.
                        // `stop_requested` is set only by a Stop while executing and cleared on the
                        // execution→fetch transition, so an error with it set IS the cancel.
                        let cancelled = !ok && t.stop_requested;
                        let msg = if cancelled { "Query cancelled".to_owned() } else { text };
                        let mut rs = ResultSet::status(ok, &label, line, &msg);
                        rs.load_elapsed = t.exec_start.map(|s| s.elapsed());
                        t.panel.push(ResultTab::Data(rs));
                    }
                    // ---- lazy stream (COPY) for the last SELECT of the run ----
                    connections::ExecMsg::LazyBegin { cols, sql, label } => {
                        let mut rs = ResultSet::new(cols, Vec::new());
                        rs.lazy = true;
                        rs.more = true;
                        rs.fetching = true; // the first screenful is loading
                        rs.sql = sql;
                        rs.title = label;
                        t.land_result(rs);
                    }
                    connections::ExecMsg::LazyRows(batch) => {
                        if let Some(rs) = lazy_grid_mut(&mut t.panel) {
                            let was_empty = rs.rows.is_empty();
                            rs.rows.extend(batch);
                            if was_empty && !rs.rows.is_empty() {
                                // first data arrived — size columns to content (header-only was narrow)
                                rs.gm.widths = grid_widths(&rs.gm.columns, &rs.rows);
                            }
                            // doscroll stops guaranteeing the sort: drop the markers but keep the
                            // already-shown order (the kept `view` permutation), and let the new rows
                            // fall in at the end in natural order (`data_row` is identity past `view`).
                            rs.sort.clear();
                        }
                    }
                    connections::ExecMsg::LazyMore => {
                        let elapsed = t.exec_start.map(|s| s.elapsed());
                        let fetch_delta = t.fetch_start.take().map(|s| s.elapsed());
                        if let Some(rs) = lazy_grid_mut(&mut t.panel) {
                            rs.fetching = false;
                            if rs.load_elapsed.is_none() {
                                rs.load_elapsed = elapsed; // first page → this sheet's cumulative load time
                            }
                            if let Some(fd) = fetch_delta {
                                rs.fetch_elapsed += fd; // a doscroll of THIS sheet settled → add its time to it
                            }
                        }
                        t.running = false; // parked — waiting for the user to fetch more
                        t.stop_requested = false; // execution over → a later doscroll Stop is a pause, not a cancel
                        if t.run_timing {
                            t.run_timing = false; // first page in → the initial run is "done"
                        }
                    }
                    connections::ExecMsg::LazyEnd { error } => {
                        let elapsed = t.exec_start.map(|s| s.elapsed());
                        let fetch_delta = t.fetch_start.take().map(|s| s.elapsed());
                        // a user Stop during execution → cancel. `stop_requested` is set only by a Stop
                        // while the query is executing and cleared at the execution→fetch transition, so
                        // an error arriving with it set IS the cancel (the read error's text is unreliable
                        // — it may not mention "cancel")
                        let cancelled = t.stop_requested && error.is_some();
                        if cancelled {
                            // replace the partial/empty lazy grid with a red "Query cancelled" error
                            let lazy_idx =
                                t.panel.iter().position(|s| matches!(s, ResultTab::Data(rs) if rs.lazy));
                            if let Some(idx) = lazy_idx {
                                let label = match &t.panel[idx] {
                                    ResultTab::Data(rs) => rs.title.clone(),
                                    _ => String::new(),
                                };
                                let mut err_sheet = ResultSet::status(false, &label, 0, "Query cancelled");
                                err_sheet.load_elapsed = elapsed;
                                t.panel[idx] = ResultTab::Data(err_sheet);
                            }
                        } else if let Some(rs) = lazy_grid_mut(&mut t.panel) {
                            rs.fetching = false;
                            rs.more = false;
                            rs.lazy = false; // ended → no longer a lazy target / fetch buttons off
                            if rs.load_elapsed.is_none() {
                                rs.load_elapsed = elapsed;
                            }
                            if let Some(fd) = fetch_delta {
                                rs.fetch_elapsed += fd; // a doscroll of THIS sheet settled → add its time to it
                            }
                            if error.is_some() {
                                // a genuine stream error → keep the partial rows, flag partial (`…`)
                                rs.truncated = true;
                            }
                        }
                        t.running = false;
                        t.stop_requested = false; // run/fetch ended → flag consumed
                        if t.run_timing {
                            t.run_timing = false;
                        }
                    }
                    _ => {}
                }
            }
            let finished = done.is_some();
            match done {
                Some(client) => {
                    if t.run_timing {
                        t.run_timing = false; // a buffered run (no lazy park) finishes here
                    }
                    t.client = client; // reclaim & keep the session connection (None if it failed)
                    t.exec_rx = None;
                    t.exec_cancel = None;
                    t.exec_start = None;
                    t.fetch_tx = None;
                    t.fetch_stop = None;
                    t.last_fetch = None;
                    t.running = false;
                    t.stop_requested = false;
                    t.refresh_idx = None;
                    // the worker is gone → no lazy grid can still be live; disarm any lingering flags
                    for sheet in &mut t.panel {
                        if let ResultTab::Data(rs) = sheet {
                            rs.lazy = false;
                            rs.more = false;
                            rs.fetching = false;
                        }
                    }
                }
                // churning → poll at ~10 Hz; a parked lazy stream → a slow tick to check the idle timeout
                None => {
                    if t.running {
                        request_poll(ctx);
                    } else {
                        ctx.request_repaint_after(std::time::Duration::from_millis(1000));
                    }
                }
            }
            // a run deferred while a lazy stream was closing fires now, reusing the returned connection
            if finished {
                if let Some((statements, ri)) = self.tabs[i].pending_exec.take() {
                    self.spawn_exec(i, statements, ri);
                }
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
                    request_poll(ctx);
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
                Ok(Ok((client, pid, ssl))) => {
                    self.main_conn = Some(client);
                    self.main_pid = pid;
                    self.main_conn_since = Some(crate::dialog::now_hms());
                    self.main_ssl = ssl;
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
                    request_poll(ctx);
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
        // 4px menu↔toolbar spacer: components are flat (no inset), all the air comes from spacer rows
        crate::widgets::vgap(ui, "gap_below_caption");
        self.icon_toolbar(ui, ctx);
        // 4px toolbar↔header/tab strip spacer
        crate::widgets::vgap(ui, "gap_below_toolbar");
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
        // 4px spacer below the editor tabs (air under the flat pill)
        crate::widgets::vgap(ui, "gap_below_tabs");
        // The per-tab work-area toolbar is gone: the active tab's actions now live in the main
        // icon-toolbar (see icon_toolbar / editor_action_group), so the editor sheet sits flush
        // under the tabs with no chrome band between them.
        // the single bottom result panel: SQL grids, status/errors, search results —
        // all in one tab strip (Run clears, Search adds)
        if self.cur().is_some_and(|t| t.panel_visible()) {
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
        if ctx.input_mut(|i| i.consume_key(cmd_shift, Key::S)) && self.can_save_as() {
            self.save_active_as();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, Key::S)) && self.can_save() {
            self.save_active();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, Key::F)) {
            self.open_find();
        }
        // Ctrl+Shift+Tab / Ctrl+Tab — cycle through the tabs (check shift first)
        if !self.tabs.is_empty() {
            if ctx.input_mut(|i| i.consume_key(cmd_shift, Key::Tab)) {
                self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
                self.focus_editor = true;
            } else if ctx.input_mut(|i| i.consume_key(cmd, Key::Tab)) {
                self.active_tab = (self.active_tab + 1) % self.tabs.len();
                self.focus_editor = true;
            }
        }
        // F8 → Execute (SQL).
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F8)) && self.is_sql_tab() {
            self.execute(ctx);
        }
        // F5 → Inspect: Connection = Test connection.
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F5))
            && self.is_connection_tab()
            && self.test_rx.is_none()
        {
            self.start_conn_test(self.active_tab);
        }
        // F6 → open the completion popup (built in `editor` where the live caret is known)
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F6)) {
            self.ac.request = true;
            self.focus_editor = true;
        }
        if self.find_open && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.close_find();
        }
        // Ctrl+Z / Ctrl+Shift+Z (and Ctrl+Y) — undo/redo of the active editor (SQL). On
        // non-editor tabs (Connection/About) — a no-op: an egui TextEdit has its own input buffer.
        if ctx.input_mut(|i| i.consume_key(cmd, Key::Z)) {
            if let Some(t) = self.cur_mut() {
                t.ed_undo();
            }
        }
        if ctx.input_mut(|i| i.consume_key(cmd_shift, Key::Z)) || ctx.input_mut(|i| i.consume_key(cmd, Key::Y)) {
            if let Some(t) = self.cur_mut() {
                t.ed_redo();
            }
        }
    }

    /// Icon toolbar (below the caption): a STATIC strip — global quick actions (file ops, connect,
    /// the manager toggles) PLUS the editor/process action group, all merged into one band. The
    /// layout never changes from tab to tab: every icon keeps its slot, and what changes is only
    /// whether each is live or dimmed (by the active tab's kind and state). See
    /// [`editor_action_group`]. (Connection / About / Scan / Meta tabs add nothing here — their
    /// actions live on the tabs themselves.)
    fn icon_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::top("icontoolbar")
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(egui::Margin {
                left: CHROME_GUTTER as i8,
                right: CHROME_GUTTER as i8,
                top: 0,
                bottom: 0,
            }))
            .exact_size(CAPTION_H)
            .show_separator_line(false) // caption + toolbar are one block, no line below
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    // one uniform gap between icons (and the air around the `|` divider) — as in the sub-toolbars
                    ui.spacing_mut().item_spacing.x = ICON_GAP;
                    // ── 1. File actions ────────────────────────────────────────────────
                    if qbtn(ui, ic::NEW, "New tab").clicked() {
                        self.new_tab();
                    }
                    if qbtn(ui, ic::OPEN, "Open").clicked() {
                        self.open_file();
                    }
                    // Save — file (SQL), the connection store, or the staged scan settings (Apply),
                    // by tab kind. Dimmed when there's nothing pending.
                    if self.can_save() {
                        let tip = if self.is_connection_tab() {
                            "Save connection (Ctrl+S)"
                        } else if self.is_scan_tab() {
                            "Apply scan settings (Ctrl+S)"
                        } else {
                            "Save (Ctrl+S)"
                        };
                        if qbtn(ui, ic::SAVE, tip).clicked() {
                            self.save_active();
                        }
                    } else {
                        qbtn_off(ui, ic::SAVE, "Nothing to save");
                    }
                    // Save As — a new file (SQL), or Export the connection to a `.conn` file.
                    if self.can_save_as() {
                        let tip = if self.is_connection_tab() {
                            "Export connection…"
                        } else {
                            "Save As… (Ctrl+Shift+S)"
                        };
                        if qbtn(ui, icons::SAVE_AS, tip).clicked() {
                            self.save_active_as();
                        }
                    } else {
                        qbtn_off(ui, icons::SAVE_AS, "Save As (SQL / connection tab)");
                    }
                    // ── 2. Manager toggles ─────────────────────────────────────────────
                    // Left-dock toggles. Only one manager shows at a time; clicking the active one
                    // closes the dock.
                    toolbar_divider(ui);
                    let db_on = self.left_panel == Some(LeftPanel::Database);
                    if qbtn_toggle(ui, ic::MANAGER, db_on, "Connection Manager").clicked() {
                        self.left_panel = if db_on { None } else { Some(LeftPanel::Database) };
                    }
                    let meta_on = self.left_panel == Some(LeftPanel::Metadata);
                    if qbtn_toggle(ui, ic::META, meta_on, "Metadata Manager").clicked() {
                        self.left_panel = if meta_on { None } else { Some(LeftPanel::Metadata) };
                    }
                    // ── 3. Connection toggle ───────────────────────────────────────────
                    // The glyph shows the ACTION a click performs, not the state (play/pause
                    // convention): offline → plug («Connect…»), connected → plug-off
                    // («Disconnect»). Always full-strength `text` (one of the two actions is always
                    // live), like the file icons — never dimmed.
                    toolbar_divider(ui);
                    if self.connected {
                        if qbtn(ui, icons::PLUG_OFF, "Disconnect").clicked() {
                            self.request_disconnect();
                        }
                    } else if qbtn(ui, icons::PLUG, "Connect…").clicked() {
                        self.open_connect();
                    }
                    // ── 4. Editor / process actions ────────────────────────────────────
                    // The editor/process action group. The toolbar is STATIC: every icon keeps its
                    // place in every tab — what changes from tab to tab is only whether it is live
                    // or dimmed, never whether it is drawn. So the layout never jumps as you switch
                    // tabs. See `editor_action_group` for the per-button logic.
                    toolbar_divider(ui);
                    self.editor_action_group(ui, ctx);
                });
            });
    }

    fn tabbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("tabs")
            // bottom margin 0: the active-tab underline sits flush against the editor sheet
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(egui::Margin {
                // under the dock the left edge = 0 (its right gutter is the seam); with no dock — the full gutter
                left: self.dock_left(),
                right: CHROME_GUTTER as i8,
                top: 0,
                bottom: 0,
            }))
            .exact_size(TABBAR_H)
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.style_mut().visuals.override_text_color = None;
                let marks: Vec<widgets::TabMark> = self.tabs.iter().map(|t| t.editor_mark()).collect();
                let labels: Vec<String> = (0..self.tabs.len())
                    .map(|i| self.tabs[i].editor_tab_label(self.tab_unsaved(i)))
                    .collect();
                ui.horizontal_centered(|ui| {
                    // reserve room for the ‹ › buttons on the right only when tabs overflow.
                    // Reserve = the two arrows flush + the editor scrollbar gutter (`vscroll::BAR`):
                    // this way the right arrow lands on the editor's TEXT boundary (the scrollbar takes
                    // the outermost BAR px of the sheet) instead of the sheet's outer edge — otherwise
                    // it "drifts" toward the window edge, BAR px to the right of the visible editor border.
                    let row_h = ui.max_rect().height();
                    // a pair of square arrows flush = 2 * row_h; + `vscroll::BAR` so the right arrow
                    // lands exactly on the editor's TEXT boundary, not the sheet's outer edge
                    let arrows_w = if self.tab_overflow {
                        2.0 * row_h + crate::vscroll::BAR
                    } else {
                        0.0
                    };
                    // FULL strip width, before reserving the arrow slot — overflow is judged against
                    // this (see below), not the narrowed viewport.
                    let full_avail = ui.available_width();
                    let scroll_w = (full_avail - arrows_w).max(0.0);
                    // the strip viewport width changed (dock opened / closed / resized, or the window
                    // resized): the active pill may have been pushed off the right edge, so re-assert
                    // its visibility this frame (the dock "pushes" the tabs only until the active one
                    // no longer fits, then the leading tabs scroll out of view under the dock).
                    let view_changed = (scroll_w - self.tab_last_view_w).abs() > 0.5;
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
                                        // scroll the active pill into view when it changed for a
                                        // non-click reason (open / new / from a manager / Ctrl-Tab) OR
                                        // when the strip narrowed (dock opened/resized) and pushed it
                                        // off-screen. A click is handled below and keeps `tab_last_active`
                                        // in sync, so it never counts as a change here.
                                        let scroll_active = self.active_tab != self.tab_last_active || view_changed;
                                        // spacing between tabs + drag-to-reorder
                                        let (select, close, reorder) = tab_strip(
                                            ui, &labels, self.active_tab, true, Some(&marks), CHROME_GUTTER, true, scroll_active,
                                        );
                                        if let Some(i) = select {
                                            if i != self.active_tab {
                                                self.active_tab = i;
                                                self.focus_editor = true;
                                            }
                                        }
                                        if let Some((from, to)) = reorder {
                                            if from < self.tabs.len() {
                                                let active_id = self.cur().map(|t| t.id);
                                                let tab = self.tabs.remove(from);
                                                let ins = (if to > from { to - 1 } else { to })
                                                    .min(self.tabs.len());
                                                self.tabs.insert(ins, tab);
                                                // the active tab follows its id
                                                if let Some(aid) = active_id {
                                                    if let Some(pos) =
                                                        self.tabs.iter().position(|t| t.id == aid)
                                                    {
                                                        self.active_tab = pos;
                                                    }
                                                }
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
                    // Overflow = the tabs don't fit the FULL strip width. Judging against the
                    // arrow-reserved (narrower) viewport would be self-sustaining: the arrows reserve
                    // width → content "overflows" → arrows stay. So after a dock closes (full width
                    // restored) the arrows would never disappear even when the tabs now fit. Measuring
                    // against `full_avail` breaks that loop.
                    self.tab_overflow = out.content_size.x > full_avail + 1.0;
                    // ‹ › scroll buttons on the right (only shown while overflowing). Gap = 0:
                    // the arrows are flush, and the right one lands exactly on the editor's right
                    // border (reserve arrows_w = 2 * row_h = exactly two square arrows, no overhang).
                    if self.tab_overflow {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if qchevron(ui, true, "Scroll tabs left").clicked() {
                            self.tab_scroll = 90.0;
                        }
                        if qchevron(ui, false, "Scroll tabs right").clicked() {
                            self.tab_scroll = -90.0;
                        }
                    }
                    // remember where the strip is now pointed: next frame, any active-tab change that
                    // ISN'T a click on a visible pill (the click above already moved us here) — or a
                    // viewport-width change — will differ from this and trigger the scroll-into-view.
                    self.tab_last_active = self.active_tab;
                    self.tab_last_view_w = scroll_w;
                });
            });
    }

    /// The chrome left inset IN THE WORK AREA (the split zone right of the dock): under an open dock
    /// = 0 — the 4px seam comes from the dock's own right gutter (otherwise two would stack = 8px);
    /// with no dock — the full gutter. One source for the invariant shared by the tabs, the result
    /// header / sub-toolbar and the islands. Full-width title/toolbar/status bar are NOT covered here
    /// — they always use the full gutter.
    pub(crate) fn dock_left(&self) -> i8 {
        if self.left_panel.is_some() {
            0
        } else {
            CHROME_GUTTER as i8
        }
    }

    /// The inner inset of the content islands (editor / result panel / tab pages). Horizontal: left
    /// edge = `dock_left()` (0 under the dock — its right gutter is the seam), right = gutter. Top =
    /// 0: the gap below the tabs/header comes from a spacer row (vgap / subbar top); without it the
    /// island would sit 1px below the schema combo in the dock (a misalignment).
    pub(crate) fn island_margin(&self) -> Margin {
        Margin {
            left: self.dock_left(),
            right: CHROME_GUTTER as i8,
            top: 0,
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
            // The one chrome row deliberately off the 22px control grid: STATUSBAR_H (20) packs the
            // bottom info line down to the minimum air around an 11px mono caps line.
            .exact_size(STATUSBAR_H)
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                left: CHROME_GUTTER as i8,
                right: if maximized { CHROME_GUTTER as i8 } else { RESIZE_GRIP_W as i8 },
                top: 0,
                bottom: 0,
            }))
            .show_separator_line(false)
            .show(ui, |ui| {
                let sz = STATUSBAR_SIZE;
                // One font override for the whole bar (swap Proportional↔Monospace to toggle the look:
                // Monospace matches the editor and holds a fixed digit width; Proportional reads softer).
                // Chips set their own family to match — keep the two in sync.
                ui.style_mut().override_font_id =
                    Some(egui::FontId::new(sz, egui::FontFamily::Proportional));
                ui.horizontal_centered(|ui| {
                    // The right group is the OUTER, full-width right_to_left so it hugs the far-right
                    // edge; the left status labels fill the remaining space in a nested left_to_right.
                    // scan · connection · version — reading left to right. The connection chip
                    // (→ active connection tab) shows while connected / broken; the scan chip (→ Scan tab)
                    // shows only while connected. In right_to_left, code order is right-to-left.
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 1.0; // 1px bonus; chips add 4px accent → 5px to a divider
                            self.version_chip(ui, sz); // rightmost — links to the About/version page
                            // divider only when conn_chip will actually paint (avoiding an orphan
                            // divider when conn_broken && active_label is empty).
                            if (self.connected || self.conn_broken) && !self.active_label.is_empty()
                            {
                                toolbar_divider(ui);
                                self.conn_chip(ui, sz);
                            }
                            // scan — left of the connection chip, while connected (a live collector
                            // to report). Its divider sits to its right, between scan and the chip.
                            if self.connected {
                                toolbar_divider(ui);
                                self.scan_chip(ui, sz);
                            }
                            // the active SQL tab's run timer (total, seconds) — left of the connection
                            if let Some(timer) = self.run_timer_text() {
                                toolbar_divider(ui);
                                // The timer is a bare reading, not a chip, so it lacks the chips' 4px
                                // accent padding — give it a matching 4px inset so it clears the divider
                                // by the same 5px as the chips beside it.
                                egui::Frame::new()
                                    .inner_margin(egui::Margin::symmetric(4, 0))
                                    .show(ui, |ui| {
                                        ui.label(RichText::new(timer).size(sz).color(p().text));
                                    });
                                if self.cur().is_some_and(|t| t.running) {
                                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
                                }
                            }
                        // LEFT — editor status: caret position + encoding (SQL tabs), then the active
                        // tab's process status (SQL run / Find). Crash text goes to a modal, not here.
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            // hard-clip the left block to the space the right group left over, so
                            // a long message never overdraws scan/connection/version when narrow
                            ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                            // Right chips clear a divider by 5px (1px item-spacing + their own 4px
                            // accent padding); the bare left labels have no padding, so they take the
                            // full 5px as item-spacing to sit the same distance off their dividers.
                            ui.spacing_mut().item_spacing.x = 5.0;
                            if let Some(t) = self.cur().filter(|t| t.is_editor()) {
                                // left-to-right order: Encoding, EOL, Ln Col
                                let (enc, eol) = match &t.doc {
                                    TabDoc::Ready(d) => (d.encoding_label.clone(), d.eol.label()),
                                    _ => ("UTF-8".to_owned(), "—"),
                                };
                                ui.label(RichText::new(enc).size(sz).color(p().text));
                                toolbar_divider(ui);
                                ui.label(RichText::new(eol).size(sz).color(p().text));
                                toolbar_divider(ui);
                                ui.label(
                                    RichText::new(format!(
                                        "Ln {} Col {}",
                                        self.cursor_ln, self.cursor_col
                                    ))
                                    .size(sz)
                                    .color(p().text),
                                );
                            }
                            // the active tab's process execution status (SQL run / Find):
                            // success/error/progress — bound to the editor tab
                            if let Some((msg, is_err)) = self.cur().and_then(|t| t.proc_status.clone())
                            {
                                toolbar_divider(ui);
                                let color = if is_err { p().danger } else { p().text };
                                ui.label(RichText::new(msg).size(sz).color(color));
                            }
                            // (the panic-recovery message no longer lives here — it goes to the error
                            // modal now, see `ui()`. The status bar carries no crash text.)
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
            .show(ui, |ui| {
                // one compact bar: tabs (left) · resize grab (fill) · maximize · close (right)
                egui::Panel::top("result_bar")
                    .exact_size(TABBAR_H)
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        // under the dock left=0, like the editor tabs and the grid body — otherwise
                        // the result header (pills) would drift 4px to the right of everything else
                        left: self.dock_left(),
                        right: CHROME_GUTTER as i8,
                        top: 0,
                        bottom: 0,
                    }))
                    .show(ui, |ui| {
                        ui.style_mut().visuals.override_text_color = None;
                        let active_rt = self.cur().map_or(0, |t| t.panel_active);
                        let mut do_close = false;
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            // maximize/restore ↔ close — 22px squares with the same `ICON_GAP` as the
                            // toolbar icons. The `‹ ›` arrow pair inside the tab lane stays tight
                            // (item_spacing.x=0 in its own block) — its `2 * row_h` reserve depends on
                            // that; this gap only pushes the action cluster away from the lane/arrows.
                            ui.spacing_mut().item_spacing.x = ICON_GAP;
                            if close_x(ui, "Close results panel") {
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
                                let (names, marks) = self.result_strip();
                                let row_h = ui.max_rect().height();
                                // last frame's overflow decides whether ‹ › reserve their slot
                                // (one-frame lag, same as the editor tab strip's arrows)
                                let overflow_id = egui::Id::new("result_lane_overflow");
                                let scroll_id = egui::Id::new("result_lane_scroll");
                                let was_overflow: bool = ui
                                    .ctx()
                                    .data_mut(|d| d.get_temp(overflow_id).unwrap_or(false));
                                // a pair of square arrows flush = 2 * row_h
                                let arrows_w =
                                    if was_overflow { 2.0 * row_h } else { 0.0 };
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
                                                    let (s, _, _) = tab_strip(
                                                        ui, &names, active_rt, false, Some(&marks), CHROME_GUTTER, false, false,
                                                    );
                                                    sel = s;
                                                });
                                            })
                                    })
                                    .inner;
                                let overflow =
                                    out.content_size.x > out.inner_rect.width() + 1.0;
                                ui.ctx().data_mut(|d| d.insert_temp(overflow_id, overflow));
                                // overflow is detected with a one-frame lag; on a state CHANGE we
                                // force the next frame, otherwise after a background query the arrows
                                // would only appear after the mouse moves (no input → no repaint)
                                if overflow != was_overflow {
                                    ui.ctx().request_repaint();
                                }
                                if was_overflow {
                                    // gap=0: the arrows are flush; the group = 2 * row_h, butts up
                                    // against the tab lane's right edge and no longer overlaps maximize/close
                                    ui.spacing_mut().item_spacing.x = 0.0;
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
                                    self.clear_grid_selection(); // selection belongs to the old sheet
                                    if let Some(t) = self.cur_mut() {
                                        // just switch — each sheet keeps its own revealed-row
                                        // count / scroll, so coming back shows where you left off
                                        t.panel_active = i;
                                    }
                                }
                                // remaining width left for the tab lane; resize lives on the
                                // full-width grab strip above the panel (see below)
                            });
                        });
                        if do_close {
                            if let Some(t) = self.cur_mut() {
                                t.clear_panel();
                                t.search_hl.clear();
                            }
                            full = false;
                        }
                    });

                // result work-area toolbar — a chrome strip under the result tabs
                subbar(ui, "result_toolbar_bar", self.dock_left(), |ui| {
                    self.result_toolbar(ui)
                });

                // 4px sub-toolbar → grid spacer (island_margin top is now 0; the gap comes from a
                // spacer, as with the data islands in the managers)
                crate::widgets::vgap(ui, "gap_result_grid");
                ui.spacing_mut().item_spacing.y = 0.0;
                // body — the active sheet within the side gutters
                egui::Frame::new()
                    .inner_margin(self.island_margin())
                    .show(ui, |ui| {
                        island(ui, |ui| self.result_body(ui));
                    });
            });
        // resize grab + visible gap between the editor and the result panel: a thin full-width
        // chrome strip; drag it anywhere to resize (no resize while maximized)
        if !full {
            // the grab strip = the single gutter (4px), like every strip; it is the ONLY gap between
            // the editor and the result header (editor island bottom:0 → grab → result_bar top:0)
            const RESULT_GAP: f32 = CHROME_GUTTER;
            let inner = egui::Panel::bottom("result_grab")
                .resizable(false)
                .exact_size(RESULT_GAP)
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(p().panel))
                .show(ui, |ui| {
                    ui.allocate_exact_size(ui.available_size(), egui::Sense::drag())
                })
                .inner
                .1;
            if inner.hovered() || inner.dragged() {
                ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical);
            }
            if inner.dragged() {
                rh = (rh - inner.drag_delta().y).clamp(120.0, max_h);
                // the user chose a size → stop auto-sizing the panel to fit the default row count
                if let Some(t) = self.cur_mut() {
                    t.result_height_user_set = true;
                }
            }
        }
        // auto-size a fresh (never user-resized) panel to the EXACT height that fits DEFAULT_RESULT_ROWS
        // whole data rows — snap straight to the target pixel height instead of nudging by whole rows
        // (a floor row-count settles anywhere within a 22px band, leaving a partial row peeking ≈10.5).
        // target = fixed chrome above the grid (tabs + sub-toolbar + spacer) + grid::panel_height_for
        // (the grid's header band + N data rows + one BAR reserved below for the horizontal scroll's
        // home, so the last row is never covered and the default height is static regardless of column
        // count — the exact inverse of grid::rows_fit). Once the user drags, this stops.
        let user_set = self.cur().is_some_and(|t| t.result_height_user_set);
        if !user_set {
            let target = (TABBAR_H + SUBBAR_H + CHROME_GUTTER
                + grid::panel_height_for(DEFAULT_RESULT_ROWS))
            .clamp(120.0, max_h);
            if (rh - target).abs() > 0.5 {
                rh = target;
                ctx.request_repaint(); // settle on the next frame
            }
        }
        // persist the (possibly dragged / auto-sized) height + maximize state back onto the active tab
        if let Some(t) = self.cur_mut() {
            t.result_height = rh;
            t.result_full = full;
        }
    }

    /// Result work-area toolbar icons (Refresh / Fetch next / Fetch all). Sits in a chrome strip
    /// under the result tabs; enabled per the active sheet's state (only refresh-able data grids).
    fn result_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.x = ICON_GAP;
        let busy = self.tab_busy();
        let has_sheet = self.cur().is_some_and(|t| !t.panel.is_empty());
        // Refresh — re-run the action that produced the active sheet (data / Find)
        if has_sheet && !busy {
            if qbtn_sm(ui, ic::REFRESH, p().text, "Refresh").clicked() {
                self.refresh_active_output(ui.ctx());
            }
        } else {
            qbtn_off_sm(ui, ic::REFRESH, "Refresh");
        }
        // Fetch (lazy load) — drawn ALWAYS so the toolbar is complete (dimmed by default); only a LIVE
        // lazy grid (the COPY-streamed last SELECT with rows still on the server) arms them. A fully
        // buffered grid already shows everything, so its fetch buttons stay inert.
        let armed = self.cur_data().is_some_and(|r| r.lazy && r.more && !r.fetching);
        // a stale result (idle-dropped stream, or a non-last statement's capped page): buttons LOOK
        // live, but a click explains there's nothing to fetch (then disarms). Mutually exclusive with
        // `armed` (stale grids are never lazy+more), and the `else if` only reaches it when !armed.
        // A fully-fetched grid (lazy=false, more=false, !stale) falls through to the inert branch.
        let stale = self.cur_data().is_some_and(|r| r.stale);
        if armed {
            if qbtn_sm(ui, ic::FETCH_NEXT, p().text, "Fetch next page").clicked() {
                self.fetch_more();
            }
            if qbtn_sm(ui, ic::FETCH_ALL, p().text, "Fetch to end (+100 MB)").clicked() {
                self.fetch_all();
            }
        } else if stale {
            if qbtn_sm(ui, ic::FETCH_NEXT, p().text, "Fetch next page").clicked() {
                self.fetch_closed_notice();
            }
            if qbtn_sm(ui, ic::FETCH_ALL, p().text, "Fetch to end (+100 MB)").clicked() {
                self.fetch_closed_notice();
            }
        } else {
            qbtn_off_sm(ui, ic::FETCH_NEXT, "Fetch next page");
            qbtn_off_sm(ui, ic::FETCH_ALL, "Fetch to end");
        }
    }

    /// A fetch button was clicked on a `stale` grid — one with more rows on the server but no live
    /// stream to pull them (an idle-closed stream, or an earlier statement's capped first page).
    /// Explain it in a modal, then disarm the buttons (clear `stale` → they go inert).
    fn fetch_closed_notice(&mut self) {
        self.error_modal = Some(
            "No more rows can be fetched for this result — it isn't a live stream (an earlier \
             statement's page, or a stream closed after idling). Re-run the statement on its own \
             (Refresh) to load the full result."
                .to_owned(),
        );
        if let Some(rs) = self.cur_data_mut() {
            rs.stale = false;
        }
    }

    /// Re-run the action that produced the active panel sheet: data grid → its statement;
    /// the Find sheet → the search.
    fn refresh_active_output(&mut self, ctx: &egui::Context) {
        if matches!(self.cur().and_then(|t| t.cur_panel()), Some(ResultTab::Probe(_))) {
            let q = self.find_query.clone();
            self.start_search(q);
        } else if self.cur_data().is_some_and(|r| !r.sql.is_empty()) {
            self.refresh_current_result(ctx);
        }
    }

    /// The static editor/process action group, drawn inline in the main icon-toolbar (see
    /// [`icon_toolbar`]). **The set never changes from tab to tab** — every icon keeps its slot;
    /// only whether it is live or dimmed changes. So the toolbar never jumps as you switch tabs.
    ///
    /// Layout: `Refact · Inspect · Execute · Stop`.
    /// - Refact (F9) — a parked placeholder (future automatic SQL refactor) → dimmed.
    /// - Inspect (F5) — Connection = Test connection. Dimmed on SQL (placeholder). Tooltips stay
    ///   tab-neutral so they don't mislead when SQL Inspect is wired up later.
    /// - Execute (F8) is live on a SQL tab with text, connected and idle.
    /// - Stop is red while anything runs on the active tab (a SQL query, a fetch-all reveal, or a
    ///   background search) and dispatches to the matching cancellation; dimmed otherwise.
    ///
    /// Connection / About / Meta tabs add nothing here — their actions live on the tabs
    /// themselves. (An earlier design mirrored them into the toolbar; that was dropped as redundant.)
    fn editor_action_group(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // One uniform spacing step between neighbouring icons (SPACE_1 = 4px) — Stop sits on the same
        // rhythm as the rest (the earlier extra SPACE_2 gap before it read as stray empty space).
        ui.spacing_mut().item_spacing.x = SPACE_1;
        let is_sql = self.is_sql_tab();
        // Scan tab maps the metadata-collector controls onto the existing slots: Execute (play) =
        // Enable the scanner, Stop = Disable it (both live only while connected, so there's a live
        // collector to drive); Apply is the toolbar Save. `scan_paused` = currently disabled.
        let is_scan = self.is_scan_tab();
        let scan_live = is_scan && self.connected;
        let scan_paused = self.collector_status.paused;
        let active_running = self.cur().is_some_and(|t| t.running); // a query/fetch actively churning
        let bg_proc = self.cur().is_some_and(|t| t.proc.is_some());
        // a live lazy fetch on THIS TAB (any of its grids) — not the active sub-tab only, so switching
        // result sub-tabs during "fetch to end" doesn't flip Stop from pause to a hard cancel
        let lazy_fetching = self.cur().is_some_and(|t| {
            t.panel.iter().any(|s| matches!(s, ResultTab::Data(rs) if rs.lazy && rs.fetching))
        });
        let run_timing = self.cur().is_some_and(|t| t.run_timing); // the INITIAL query is still executing
        let has_sql = self
            .cur()
            .is_some_and(|t| matches!(&t.doc, TabDoc::Ready(d) if d.char_count() > 0));

        // Refact — parked placeholder (future automatic SQL refactor, F9) → always dimmed.
        // The Format glyph stays: it's the natural visual for "reformat source", and the tooltip
        // clarifies the SQL meaning (Refact).
        let why = if is_sql { "Refact (F9) — coming soon" } else { "Refact (SQL tab)" };
        qbtn_off(ui, icons::FORMAT, why);

        // Inspect — "check that the current thing is correct / works". Connection = Test connection
        // (probe the server, result in the Test modal). SQL = parked. Dimmed when the action doesn't
        // apply. Tooltip stays tab-neutral so it ages well when SQL Inspect lands.
        let is_conn = self.is_connection_tab();
        let conn_testing = self.test_rx.is_some();
        if is_conn && !conn_testing {
            if qbtn(ui, icons::CHECK, "Test connection (F5)").clicked() {
                self.start_conn_test(self.active_tab);
            }
        } else {
            let why = if is_sql {
                "Inspect (F5) — coming soon"
            } else if is_conn {
                "Test connection (a test is already running)"
            } else {
                "Inspect (Connection tab)"
            };
            qbtn_off(ui, icons::CHECK, why);
        }

        // Execute — THE action of the loop (green when armed). Live for SQL today; on the Scan tab
        // it Enables the (paused) metadata collector instead.
        if is_sql && self.connected && !active_running && has_sql {
            if qbtn_col(ui, ic::PLAY, p().ok, "Execute selection / all (F8)").clicked() {
                self.execute(ctx);
            }
        } else if scan_live && scan_paused {
            if qbtn_col(ui, ic::PLAY, p().ok, "Enable scan").clicked() {
                self.set_collector_enabled(true);
            }
        } else {
            let why = if is_scan && !self.connected {
                "Enable scan (connect first)"
            } else if is_scan {
                "Enable scan (already running)"
            } else if !is_sql {
                "Execute (SQL tab)"
            } else if !self.connected {
                "Execute (connect first)"
            } else if active_running {
                "Execute (a query is already running on this tab)"
            } else if !has_sql {
                "Execute (the editor is empty)"
            } else {
                "Execute"
            };
            qbtn_off(ui, ic::PLAY, why);
        }

        // Stop — always the last icon. Red while anything runs. For a LIVE lazy fetch it PAUSES the
        // stream (lightning — stays open, a later fetch resumes it); for a churning buffered run /
        // initial load it sends a server CancelRequest; for a background search it cancels it.
        if active_running || bg_proc {
            // during the INITIAL execution Stop ABORTS the query (→ a red "Query cancelled" error); a
            // doscroll fetch (run_timing already cleared) PAUSES instead, keeping the partial result
            let pausing = active_running && !run_timing && lazy_fetching;
            let tip = if pausing {
                "Pause fetch"
            } else if active_running {
                "Stop query"
            } else {
                "Stop process"
            };
            if qbtn_col(ui, icons::STOP, p().danger, tip).clicked() {
                if pausing {
                    self.pause_fetch();
                } else if active_running {
                    self.cancel_running_query();
                } else {
                    self.stop_active_proc();
                }
            }
        } else if scan_live && !scan_paused {
            // Scan tab: the scanner is running → Stop = Disable it (pause the collector)
            if qbtn_col(ui, icons::STOP, p().danger, "Disable scan").clicked() {
                self.set_collector_enabled(false);
            }
        } else {
            qbtn_off(ui, icons::STOP, "Nothing to stop");
        }
    }

    /// Drop both cell and whole-row selection in the result grid (the active sheet changed, or a
    /// re-sort/re-run made the old indices meaningless).
    fn clear_grid_selection(&mut self) {
        self.grid_sel = None;
        self.grid_rows.clear();
        self.grid_row_anchor = None;
    }

    /// Update the whole-row selection after a click on the "#" gutter (visible row index). Plain
    /// click: just this row. Ctrl: toggle this row in/out, keeping the rest. Alt: extend a range from
    /// the anchor to this row (additive when Ctrl is also held).
    fn update_row_sel(&mut self, r: usize, ctrl: bool, alt: bool) {
        if alt {
            let a = self.grid_row_anchor.unwrap_or(r);
            if !ctrl {
                self.grid_rows.clear();
            }
            for x in a.min(r)..=a.max(r) {
                self.grid_rows.insert(x);
            }
            // keep the anchor so the range can be re-dragged from the same origin
        } else if ctrl {
            if !self.grid_rows.remove(&r) {
                self.grid_rows.insert(r);
            }
            self.grid_row_anchor = Some(r);
        } else {
            self.grid_rows.clear();
            self.grid_rows.insert(r);
            self.grid_row_anchor = Some(r);
        }
    }

    /// Fold a grid frame's selection output into the app state — one rule shared by the data and
    /// probe sheets: a "#"-gutter click drives the whole-row selection, a column reorder just drops
    /// the cell selection, anything else adopts the grid's cell selection (clearing the row
    /// selection when a fresh cell interaction superseded it).
    fn apply_grid_selection(&mut self, out: &grid::GridOutput) {
        if let Some((r, ctrl, alt)) = out.row_click {
            self.update_row_sel(r, ctrl, alt);
            self.grid_sel = None; // row selection supersedes cell selection
        } else if out.reorder.is_some() {
            self.grid_sel = None;
        } else {
            if out.clear_rows {
                self.grid_rows.clear();
                self.grid_row_anchor = None;
            }
            self.grid_sel = out.sel;
        }
    }

    /// Render the active result sheet: a data grid (SQL result / status) or a probe
    /// (search matches). A click links back to the editor (status error line, search match
    /// position). During a still-empty run it shows a soft placeholder.
    fn result_body(&mut self, ui: &mut egui::Ui) {
        ui.set_min_size(ui.available_size());
        // rows the body can show, from the shared grid::rows_fit (full − header − the reserved bottom
        // scroll strip) — so the panel can auto-size to fit the default count even before the first rows
        // arrive (no flash), and the pre-loading capacity lands the rows in a fit panel.
        let cap = grid::rows_fit(ui.max_rect().height());
        let i = self.active_tab;
        let (n_sheets, active, busy) = match self.tabs.get(i) {
            Some(t) => (t.panel.len(), t.panel_active, t.running || t.proc.is_some()),
            None => return,
        };
        // the active result tab has no backing sheet — the panel is empty, OR the user switched to a
        // pending statement's placeholder tab (its sheet isn't produced yet). While a run is in flight
        // show the centered Running pill, NOT a stale earlier sheet (the old `.min(len-1)` clamp showed
        // the previous statement's grid here).
        if active >= n_sheets {
            if busy {
                if cap > 0 {
                    self.tabs[i].last_visible_rows = cap; // pre-size during loading so rows land in a fit panel
                }
                let area = ui.max_rect();
                let tsec = ui.input(|inp| inp.time) as f32;
                widgets::running_overlay(ui, area, tsec); // accent pill, centred, animated dots
                return;
            }
            if n_sheets == 0 {
                return; // empty + idle → nothing
            }
            // idle but the active index is stale (a pending tab that never materialized, e.g. an
            // aborted batch) → fall through and clamp to the last real sheet
        }
        let idx = active.min(n_sheets - 1);
        // take the sheet out for the duration of drawing, put it back at the end
        let mut sheet =
            std::mem::replace(&mut self.tabs[i].panel[idx], ResultTab::Data(ResultSet::new(Vec::new(), Vec::new())));
        let sel = self.grid_sel;
        let mut goto: Option<(usize, usize)> = None;
        let mut reset_idle = false; // a lazy grid interacted with → reset its idle timeout
        let grid_rows_fit: usize; // whole rows the data area can show (for the next first page)
        match &mut sheet {
            ResultTab::Data(rs) => {
                if rs.view_dirty {
                    rs.rebuild_view(); // sort changed, or rows grew under an active sort
                }
                let rows = rs.rows.len(); // show every row fetched so far (the grid is virtualized)
                let err_flag = rs.err;
                let err_col = if rs.err { Some(0usize) } else { None };
                let row = |r: usize| std::borrow::Cow::Borrowed(rs.rows[rs.data_row(r)].as_slice());
                let err = |_r: usize| err_flag;
                let mut scroll = rs.scroll;
                let mut fade = rs.fade;
                let out = grid::result_grid(
                    ui, &rs.gm, rows, sel, &row, &err, err_col, &rs.sort, &self.grid_rows,
                    &mut scroll, &mut fade,
                );
                grid_rows_fit = out.rows_fit;
                if let Some(c) = out.copy.clone() {
                    ui.ctx().copy_text(c);
                }
                // any interaction with a live lazy grid keeps its stream alive (resets idle timeout)
                reset_idle = rs.lazy
                    && (scroll != rs.scroll
                        || out.copy.is_some()
                        || out.clicked_row.is_some()
                        || out.sel.is_some()
                        || out.sort_click.is_some()
                        || out.row_click.is_some());
                rs.scroll = scroll;
                rs.fade = fade;
                rs.gm.apply(&out);
                if let Some((col, additive)) = out.sort_click {
                    rs.toggle_sort(col, additive);
                    rs.scroll.0 = 0.0; // jump to the top so the new order is visible
                    self.clear_grid_selection(); // row order changed → both selections invalid
                } else {
                    self.apply_grid_selection(&out);
                }
                if out.clicked_row.is_some() {
                    if let Some(line) = rs.goto_line {
                        goto = Some((line.saturating_sub(1), 0));
                    }
                }
            }
            ResultTab::Probe(res) => {
                let count = res.len();
                let row = |r: usize| std::borrow::Cow::Owned(res.row_values(r));
                let err = |_r: usize| false;
                let mut scroll = res.scroll;
                let mut fade = res.fade;
                let out = grid::result_grid(
                    ui, &res.grid, count, sel, &row, &err, None, &[], &self.grid_rows,
                    &mut scroll, &mut fade,
                );
                grid_rows_fit = out.rows_fit;
                if let Some(c) = out.copy.clone() {
                    ui.ctx().copy_text(c);
                }
                res.scroll = scroll;
                res.fade = fade;
                res.grid.apply(&out);
                self.apply_grid_selection(&out);
                if let Some(r) = out.clicked_row {
                    goto = res.matches.get(r).map(|m| (m.line, m.col));
                }
            }
        }
        self.tabs[i].panel[idx] = sheet; // put the sheet back
        // a still-loading grid (the lazy first page, no rows yet) shows the Running pill over its body
        let loading = self.tabs[i].running
            && self.tabs[i].cur_data().is_some_and(|rs| rs.rows.is_empty());
        if loading {
            let area = ui.max_rect();
            let tsec = ui.input(|inp| inp.time) as f32;
            widgets::running_overlay(ui, area, tsec);
        }
        if grid_rows_fit > 0 {
            // remember THIS tab's on-screen row capacity (persists while the panel is closed) so the
            // next run's first page fills the visible area exactly. NOTE: we deliberately do NOT
            // auto-fetch more rows when the panel is enlarged — the user drives доскролл with buttons.
            self.tabs[i].last_visible_rows = grid_rows_fit;
        }
        if reset_idle {
            self.tabs[i].last_fetch = Some(std::time::Instant::now());
        }
        if let Some(g) = goto {
            self.tabs[i].pending_goto = Some((g, g));
            self.focus_editor = true;
        }
    }

    fn editor(&mut self, ui: &mut egui::Ui) {
        // each non-editor kind renders its own body instead of the SQL editor
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
            .show(ui, |ui| {
                if self.tabs.is_empty() {
                    // empty state: a centred hint with the two ways to get a tab; gone as soon as
                    // one opens (Design Delta v2.1 §5)
                    ui.painter().text(
                        ui.max_rect().center(),
                        egui::Align2::CENTER_CENTER,
                        "Ctrl+N — new query     Ctrl+O — open file",
                        egui::FontId::proportional(BODY_SIZE),
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
                    // the document is still loading — a sheet with a progress caption instead of the editor
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
                        egui::FontId::proportional(BODY_SIZE),
                        p().text_dim,
                    );
                    return;
                }
                self.code_editor(ui, sheet);
            });
    }

    /// Editor wrapper: assembles a [`codeeditor::EditorCtx`] from the active tab (SQL highlight,
    /// Smart-Tab, F6 autocomplete) and unpacks the [`codeeditor::EditorOut`] back into the app state.
    fn code_editor(&mut self, ui: &mut egui::Ui, sheet: egui::Rect) {
        let ctx = &ui.ctx().clone();
        let idx = self.active_tab.min(self.tabs.len() - 1);
        let tab_id = self.tabs[idx].id;
        let ed_id = egui::Id::new(("code_editor", tab_id));
        let Some(mut doc) = self.tabs[idx].take_doc() else { return };
        let mut ed = std::mem::take(&mut self.tabs[idx].ed);
        // The editor is NEVER blocked — typing/autocomplete stay live even while a query, lazy fetch
        // or background search runs (the SQL text isn't used after launch; Search only reads a
        // snapshot).

        // autocomplete BEFORE the editor
        let focused = ctx.memory(|m| m.has_focus(ed_id));
        let mut edited = if focused {
            self.editor_completion(&mut doc, &mut ed, ctx, tab_id)
        } else {
            false
        };

        // The SQL highlighter; the editor itself is language-neutral (takes a callback).
        let sql_line = |text: &str, st: codeeditor::LexState| {
            let (job, end) =
                highlight::highlight_sql(text, highlight::LineState::from_key(st), CODE_SIZE);
            (job, end.key())
        };
        let sql_advance = |text: &str, st: codeeditor::LexState| {
            highlight::highlight_sql_state_only(text, highlight::LineState::from_key(st)).key()
        };
        // Tab: at the END of a line — always exactly two spaces (no grid/count). Inside a line —
        // the prior alignment: smart "hook" / 4-column stops.
        let sql_tab_insert = |d: &mut doc::Document, (l, c): doc::Pos| {
            if c >= d.line_length(l) {
                return "  ".to_owned();
            }
            let prev = if l > 0 { Some(d.get_line(l - 1)) } else { None };
            complete::tab_spaces(prev.as_deref(), c)
        };

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
                    ed_id,
                    hl: codeeditor::Highlighter { line: &sql_line, advance: &sql_advance },
                    tab_insert: &sql_tab_insert,
                },
            )
        };
        edited |= out.edited;
        self.cursor_ln = out.caret.0 + 1;
        self.cursor_col = out.caret.1 + 1;
        if let Some(e) = out.error {
            self.error_modal = Some(e);
        }

        // ---- completion popup (anchored to this frame's editor geometry) ----
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
            // the buffer changed → a finished Find verdict in the status bar is now
            // stale; drop it (but keep a live "…ing" message while a process is still running)
            let t = &mut self.tabs[idx];
            if t.proc.is_none() {
                t.proc_status = None;
            }
        }
        self.tabs[idx].ed = ed;
        self.tabs[idx].put_doc(doc);
    }

    // ============================================================
    //  Background processes (search)
    // ============================================================

    /// The active tab is busy with background work ACTIVELY churning (a query/fetch in progress or a
    /// background search): gates the launch of other processes. A PARKED lazy stream (worker alive, waiting
    /// for the user to fetch more) is deliberately NOT busy — Execute/Refresh stay live and
    /// transparently close + replace it (the run is deferred until the connection returns via Done).
    fn tab_busy(&self) -> bool {
        self.cur().is_some_and(|t| t.running || t.proc.is_some())
    }

    /// Start a background search for `query` over the active tab → a results sheet (ADD).
    /// The editor stays live during the search (it reads a snapshot); other processes and
    /// Execute are blocked on this tab until it finishes (one process per tab).
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
        let tgt = t.upsert_probe(proc::Results::new());
        t.proc_target = Some(tgt);
        t.proc = Some(proc::RunningProc::new(rx, cancel));
        t.proc_status = Some(("Searching…".to_owned(), false));
        self.focus_editor = true; // focus stays in the editor
    }

    /// Stop the process on the active tab (a soft cancel via the flag).
    fn stop_active_proc(&mut self) {
        if let Some(rp) = self.cur().and_then(|t| t.proc.as_ref()) {
            rp.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Poll the channels of all tabs' running processes (search). Match batches are appended into
    /// the target panel sheet (`proc_target`).
    fn poll_procs(&mut self, ctx: &egui::Context) {
        for i in 0..self.tabs.len() {
            let Tab { proc: proc_slot, proc_target, panel, search_hl, .. } = &mut self.tabs[i];
            let Some(rp) = proc_slot.as_mut() else { continue };
            // the target search sheet (Probe) the batches are appended into
            let mut target: Option<&mut proc::Results> = match *proc_target {
                Some(ti) => match panel.get_mut(ti) {
                    Some(ResultTab::Probe(res)) => Some(res),
                    _ => None,
                },
                None => None,
            };
            let mut fin: Option<proc::ProcMsg> = None;
            loop {
                match rp.rx.try_recv() {
                    Ok(proc::ProcMsg::SearchBatch(batch)) => {
                        if let Some(r) = target.as_deref_mut() {
                            if Self::append_search(r, search_hl, batch) {
                                rp.capped = true;
                                rp.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    }
                    Ok(m) => {
                        fin = Some(m);
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        request_poll(ctx);
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

    /// Append a batch of search matches into sheet `r`, honoring the memory cap. true → cap exceeded.
    fn append_search(
        r: &mut proc::Results,
        search_hl: &mut std::collections::HashMap<usize, Vec<(usize, usize)>>,
        batch: Vec<proc::SearchMatch>,
    ) -> bool {
        if r.truncated {
            return true;
        }
        for m in batch {
            r.bytes += m.approx_bytes();
            search_hl.entry(m.line).or_default().push((m.col, m.len));
            r.matches.push(m);
            if r.bytes > proc::RESULTS_CAP_BYTES {
                r.truncated = true;
                return true;
            }
        }
        false
    }

    /// Completion of tab `i`'s process: execution status → the tab's status bar; the RESULT
    /// (matches) is already in the panel.
    fn finish_proc(&mut self, ctx: &egui::Context, i: usize, fin: proc::ProcMsg) {
        let Some(rp) = self.tabs[i].proc.take() else { return };
        let target = self.tabs[i].proc_target.take();
        let capped = rp.capped;
        // the number of matches in the target sheet (for the summary message)
        let n = target
            .and_then(|ti| self.tabs[i].panel.get(ti))
            .map_or(0, |s| match s {
                ResultTab::Probe(res) => res.len(),
                _ => 0,
            });
        let msg: (String, bool) = match fin {
            proc::ProcMsg::Done | proc::ProcMsg::Cancelled if capped => {
                ("Search: stopped — 100 MB result cap reached".to_owned(), true)
            }
            proc::ProcMsg::Done => (format!("Search: {} found", fmt_thousands(n)), false),
            proc::ProcMsg::Cancelled => ("Search: stopped by user".to_owned(), true),
            proc::ProcMsg::Failed(e) => {
                // a process error — status bar only (no result sheet created)
                (format!("Search: error — {e}"), true)
            }
            // unreachable: batches are consumed by poll_procs and never passed as `fin`
            proc::ProcMsg::SearchBatch(_) => ("Search: done".to_owned(), false),
        };
        // the process status — into the status bar, bound to the editor tab (not the result sheet)
        self.tabs[i].proc_status = Some(msg);
        ctx.request_repaint();
    }

    /// Poll the background file-load channels: progress / done / error.
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
                        // ~10 Hz poll while loading
                        request_poll(ctx);
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        done = Some(Err("the load thread ended unexpectedly".to_owned()));
                        break;
                    }
                }
            }
            match done {
                Some(Ok(d)) => {
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
                    .stroke(egui::Stroke::new(1.0 / ui.ctx().pixels_per_point(), p().border_strong)) // crisp 1 device px
                    .corner_radius(CornerRadius::ZERO)
                    // rows run edge-to-edge (the row text carries the shared TEXT_INSET below, like the
                    // combo dropdown); only the top/bottom keep a little air
                    .inner_margin(egui::Margin { left: 0, right: 0, top: 4, bottom: 4 })
                    .show(ui, |ui| {
                        ui.set_width(w);
                        // the box hugs the actual item count (≤ 9 rows scroll) — no empty
                        // reserved rows under a short list
                        crate::widgets::style_scrollbar(ui); // disappearing overlay bar, like everywhere else
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
                                        rect.left_center() + egui::vec2(crate::theme::TEXT_INSET, 0.0),
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
            if crate::widgets::modal_header(ui, title) {
                self.confirm = None;
            }
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
