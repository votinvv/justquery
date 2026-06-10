//! Metadata Manager — the in-memory object model, the left-dock panel (schema dropdown + a
//! simplified type→object tree rendered from memory), and the per-object metadata tab whose
//! attributes are fetched on demand. The two background workers live in
//! [`crate::meta_collector`] (periodic object-list scan) and [`crate::meta_details`]
//! (on-demand attributes); this module owns the shared data types and all the UI/state glue.

use crate::widgets::{
    close_x, manager_row, qbtn_off_sm, qbtn_sm, select_click, style_scrollbar,
};
use crate::theme::p;
use crate::{ic, LeftPanel, JustQueryApp, Tab};
use crate::{CHROME_PAD, SUBBAR_H, TABBAR_H};

/// Icon glyph for an object-type folder / its leaf objects.
fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "Tables" | "Foreign Tables" => ic::OBJ_TABLE,
        "Views" => ic::OBJ_VIEW,
        "Materialized Views" => ic::OBJ_MATVIEW,
        "Sequences" => ic::OBJ_SEQUENCE,
        "Functions" => ic::OBJ_FUNCTION,
        _ => ic::OBJ_OTHER,
    }
}
use eframe::egui;
use egui::{Align, Layout, Margin, RichText};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

/// Display order of the object-type folders within a schema.
pub(crate) const FOLDER_ORDER: &[&str] = &[
    "Tables",
    "Views",
    "Materialized Views",
    "Foreign Tables",
    "Sequences",
    "Functions",
];

/// True for object types that have columns to show in the metadata tab.
pub(crate) fn is_relation(kind: &str) -> bool {
    matches!(
        kind,
        "Tables" | "Views" | "Materialized Views" | "Foreign Tables"
    )
}

/// Per-connection collector settings (built from the active [`crate::connections::Connection`]).
#[derive(Clone)]
pub(crate) struct CollectorSettings {
    pub enabled: bool,
    pub interval: u64,                 // seconds, pause between scans
    pub budget: usize,                 // max object count
    pub idle: u64,                     // seconds of inactivity before the scanner sleeps
    pub schemas: Option<Vec<String>>, // None = all user schemas
}

impl Default for CollectorSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            interval: 30,
            budget: 1_000_000,
            idle: 300,
            schemas: None,
        }
    }
}

/// Status the collector reports (drives the status-bar indicator + the manager modal).
#[derive(Clone, Default)]
pub(crate) struct CollectorStatus {
    pub running: bool,        // a scan is in progress right now
    pub paused: bool,        // collector paused / disabled by the user
    pub asleep: bool,        // idle-paused: no user activity, sleeping until the next ping
    pub over_budget: bool,   // catalog exceeded the budget → scanner stopped (see last_error)
    pub last_error: Option<String>,
}

/// One line of the collector's short activity log.
#[derive(Clone)]
pub(crate) struct LogLine {
    pub time: String,
    pub text: String,
}

/// The in-memory object list (one source of truth; the tree/dropdown read a snapshot of it).
#[derive(Clone, Default)]
pub(crate) struct MetaStore {
    pub schemas: Vec<String>,      // all known user schemas (for the dropdown)
    pub objects: Vec<MetaObjRow>,  // flat list across schemas
}

#[derive(Clone)]
pub(crate) struct MetaObjRow {
    pub schema: String,
    pub kind: String, // folder label
    pub name: String, // functions carry their full signature: name(argtypes)
    pub cols: Vec<MetaCol>, // relations: their columns (for hints); empty for sequences/functions
}

/// The live store shared between the collector thread (writer) and the UI thread (reader). The
/// collector replaces objects per-schema under the write lock; the UI snapshots it into `meta_view`
/// under the read lock (on Refresh / first load) so the tree never shifts mid-frame. `generation`
/// bumps on every write — the UI compares it against the snapshot's to show the "new data" marker.
#[derive(Default)]
pub(crate) struct SharedStore {
    pub store: RwLock<MetaStore>,
    pub generation: AtomicU64,
}

impl SharedStore {
    /// Current write generation (0 = nothing scanned yet).
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
}

/// One column of a relation, shown on the metadata tab.
#[derive(Clone)]
pub(crate) struct MetaCol {
    pub name: String,
    pub ty: String,
    pub nullable: bool,
    pub default: String,
}

/// State of a metadata tab: which object it views + its on-demand attributes.
pub(crate) struct MetaObject {
    pub schema: String,
    pub name: String,
    pub kind: String,
    pub state: MetaState,
}

pub(crate) enum MetaState {
    Loading(u64),        // awaiting the details reply with this req id
    Loaded(Vec<MetaCol>),
    NoColumns,           // sequences / functions: no column view (v1)
    Deleted,             // the object no longer exists
    Failed(String),      // fetch error / timeout
}

impl JustQueryApp {
    /// Start the collector + details workers for the just-opened connection.
    pub(crate) fn start_meta_actors(&mut self) {
        let Some(p) = self.conn_params.clone() else {
            return;
        };
        let settings = self.pending_meta_settings.take().unwrap_or_default();
        let shared = std::sync::Arc::new(SharedStore::default());
        self.meta_store = shared.clone();
        self.meta_view = MetaStore::default();
        self.meta_view_gen = 0;
        self.collector_status = CollectorStatus::default();
        self.collector_log.clear();
        self.meta_schema_sel = None;
        // details: page attributes get an 8s statement timeout; the socket idles closed after the
        // same inactivity window as the scanner (updates on the next reconnect)
        self.details = Some(crate::meta_details::start(
            p.clone(),
            8000,
            std::time::Duration::from_secs(settings.idle.max(30)),
        ));
        self.collector = Some(crate::meta_collector::start(p, settings, shared));
    }

    /// Stop both workers and drop the in-memory metadata (on disconnect / before reconnect).
    pub(crate) fn stop_meta_actors(&mut self) {
        self.collector = None; // Drop sends Shutdown
        self.details = None;
        self.meta_store = std::sync::Arc::new(SharedStore::default());
        self.meta_view = MetaStore::default();
        self.meta_view_gen = 0;
        self.collector_status = CollectorStatus::default();
        self.collector_log.clear();
        self.meta_schema_sel = None;
    }

    /// Append a line to the DB-monitor activity log (shown in the SCANER modal), capped to 200.
    pub(crate) fn log_monitor(&mut self, text: String) {
        self.collector_log.push_back(LogLine {
            time: crate::dialog::now_hms(),
            text,
        });
        while self.collector_log.len() > 200 {
            self.collector_log.pop_front();
        }
    }

    /// Drain both workers' channels: collector → store/status/log, details → the waiting tabs.
    pub(crate) fn poll_metadata(&mut self, ctx: &egui::Context) {
        use crate::meta_collector::CollectorMsg;
        use crate::meta_details::DetailsErr;
        // --- collector --- (store is shared memory now; the channel only carries status + log)
        let mut new_status = None;
        let mut logs: Vec<LogLine> = Vec::new();
        if let Some(h) = &self.collector {
            while let Ok(msg) = h.rx.try_recv() {
                match msg {
                    CollectorMsg::Status(st) => new_status = Some(st),
                    CollectorMsg::Log(l) => logs.push(l),
                }
            }
        }
        // first data lands → auto-snapshot it into the view so the tree shows something immediately.
        // Later scans only bump the generation (the "new data" marker); the user pulls them via Refresh.
        let gen = self.meta_store.generation();
        if gen != self.meta_view_gen
            && self.meta_view.schemas.is_empty()
            && self.meta_view.objects.is_empty()
        {
            if let Ok(s) = self.meta_store.store.read() {
                self.meta_view = s.clone();
                self.meta_view_gen = self.meta_store.generation(); // read under the lock → consistent
            }
            // pick a sensible default schema (public, else the first)
            if self
                .meta_schema_sel
                .as_ref()
                .map_or(true, |s| !self.meta_view.schemas.contains(s))
            {
                self.meta_schema_sel = self
                    .meta_view
                    .schemas
                    .iter()
                    .find(|s| s.as_str() == "public")
                    .cloned()
                    .or_else(|| self.meta_view.schemas.first().cloned());
            }
        }
        if let Some(st) = new_status {
            self.collector_status = st;
        }
        for l in logs {
            self.collector_log.push_back(l);
            while self.collector_log.len() > 200 {
                self.collector_log.pop_front();
            }
        }
        // --- details replies → matching tabs ---
        let mut replies = Vec::new();
        if let Some(h) = &self.details {
            while let Ok(r) = h.rx.try_recv() {
                replies.push(r);
            }
        }
        for reply in replies {
            for t in &mut self.tabs {
                let Some(m) = t.meta.as_mut() else { continue };
                if !matches!(m.state, MetaState::Loading(id) if id == reply.req_id) {
                    continue;
                }
                m.state = match &reply.result {
                    Ok(cols) => MetaState::Loaded(cols.clone()),
                    Err(DetailsErr::Deleted) => MetaState::Deleted,
                    Err(DetailsErr::Timeout) => {
                        MetaState::Failed("Не смогли достать метаданные (таймаут).".to_owned())
                    }
                    Err(DetailsErr::Other(e)) => MetaState::Failed(e.clone()),
                };
            }
        }
        // keep animating while a scan runs or a tab is loading
        if self.collector_status.running
            || self
                .tabs
                .iter()
                .any(|t| matches!(t.meta.as_ref().map(|m| &m.state), Some(MetaState::Loading(_))))
        {
            ctx.request_repaint();
        }
    }

    /// Open (or focus) a metadata tab for an object, kicking off its attribute fetch for relations.
    pub(crate) fn open_meta_object(&mut self, schema: String, name: String, kind: String) {
        if let Some(i) = self.tabs.iter().position(|t| {
            t.meta
                .as_ref()
                .map_or(false, |m| m.schema == schema && m.name == name && m.kind == kind)
        }) {
            self.active_tab = i;
            return;
        }
        let mut log_line: Option<String> = None;
        let state = if is_relation(&kind) {
            if let Some(h) = &self.details {
                let rid = self.meta_req_seq;
                self.meta_req_seq += 1;
                h.columns(rid, schema.clone(), name.clone());
                log_line = Some(format!("lookup {schema}.{name}")); // record the detail request
                MetaState::Loading(rid)
            } else {
                MetaState::Failed("Нет соединения с базой.".to_owned())
            }
        } else {
            MetaState::NoColumns
        };
        if let Some(t) = log_line {
            self.log_monitor(t);
        }
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id, name.clone());
        tab.meta = Some(MetaObject {
            schema,
            name,
            kind,
            state,
        });
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
    }

    /// The Metadata Manager side panel: schema dropdown + Refresh, then a type→object tree drawn
    /// from the in-memory snapshot (`meta_view`). The tree never shifts under the cursor — Refresh
    /// re-snapshots the latest scan from memory (no DB round-trip).
    pub(crate) fn metadata_manager_panel(&mut self, ui: &mut egui::Ui) {
        if self.left_panel != Some(LeftPanel::Metadata) {
            return;
        }
        let connected = self.connected;
        let mut close_panel = false;
        let mut refresh = false;
        let mut open_obj: Option<(String, String, String)> = None;
        // "new data" marker: the live store has been written since we snapshotted the view
        let stale = self.meta_store.generation() != self.meta_view_gen;
        let saved_style = crate::widgets::hush_resize_line(ui);
        egui::Panel::left("left_panel")
            .resizable(true)
            .default_size(220.0)
            .size_range(150.0..=460.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin::ZERO))
            .show_inside(ui, |ui| {
                ui.style_mut().visuals.override_text_color = None;
                egui::Panel::top("meta_header")
                    .exact_size(TABBAR_H)
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: 10,
                        right: 6,
                        top: CHROME_PAD as i8,
                        bottom: 0,
                    }))
                    .show_inside(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.label(RichText::new("Metadata Manager").size(13.0).strong().color(p().text));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if close_x(ui, 22.0, 4.0, "Close panel") {
                                    close_panel = true;
                                }
                            });
                        });
                    });
                // toolbar: schema dropdown (left) + Refresh
                egui::Panel::top("meta_toolbar")
                    .exact_size(SUBBAR_H)
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: 8,  // window edge: the full 8px gutter
                        right: 4, // pairs with the content island's 4 → 8 total
                        top: 2,
                        bottom: 0,
                    }))
                    .show_inside(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.style_mut().visuals.override_text_color = None;
                            // Refresh (left), then the schema dropdown filling the rest of the row
                            if connected {
                                let tip = if stale {
                                    "Refresh tree (new data available)"
                                } else {
                                    "Refresh tree (from memory)"
                                };
                                let col = if stale { p().ok } else { p().text };
                                if qbtn_sm(ui, ic::REFRESH, col, tip).clicked() {
                                    refresh = true;
                                }
                            } else {
                                qbtn_off_sm(ui, ic::REFRESH, "Refresh (connect first)");
                            }
                            // schema dropdown — fills the remaining width so the panel shrinks to
                            // the same minimum as the Database Manager; disabled with no connection
                            let schemas = self.meta_view.schemas.clone();
                            let cur = self
                                .meta_schema_sel
                                .as_ref()
                                .and_then(|s| schemas.iter().position(|x| x == s));
                            let w = ui.available_width().max(60.0);
                            // make the combo button fill the row height so its text centres on the
                            // same line as the Refresh glyph (which is drawn at the full-row centre)
                            ui.spacing_mut().interact_size.y = ui.available_height();
                            if let Some(i) = crate::widgets::styled_combo(
                                ui,
                                "meta_schema",
                                w,
                                12.0,
                                connected && !schemas.is_empty(),
                                cur,
                                &schemas,
                            ) {
                                self.meta_schema_sel = schemas.get(i).cloned();
                            }
                        });
                    });
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: 8,  // window edge: the full 8px gutter
                        right: 4, // pairs with the content island's 4 → 8 total
                        top: 1,
                        bottom: 0,
                    }))
                    .show_inside(ui, |ui| {
                        let _island = egui::Frame::new()
                            .fill(p().ivory)
                            .stroke(egui::Stroke::new(1.0, p().border_strong)) // one shape (v2.2 §3)
                            .corner_radius(egui::CornerRadius::same(crate::RADIUS_ISLAND))
                            .shadow(crate::theme::island_shadow())
                            .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            // keep rows strictly inside the 1px border (no bleed on scroll)
                            let clip = ui.max_rect().shrink(1.0);
                            ui.set_clip_rect(clip);
                            style_scrollbar(ui);
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    open_obj = self.metadata_tree_body(ui, connected);
                                });
                        });
                    });
            });
        ui.set_style(saved_style);
        if close_panel {
            self.left_panel = None;
        }
        if refresh {
            if let Ok(s) = self.meta_store.store.read() {
                self.meta_view = s.clone();
                self.meta_view_gen = self.meta_store.generation(); // read under the lock → consistent
            }
            if self
                .meta_schema_sel
                .as_ref()
                .map_or(true, |s| !self.meta_view.schemas.contains(s))
            {
                self.meta_schema_sel = self.meta_view.schemas.first().cloned();
            }
        }
        if let Some((s, n, k)) = open_obj {
            self.open_meta_object(s, n, k);
        }
    }

    /// Render the simplified tree (type folders → objects) for the selected schema, from `meta_view`.
    /// Returns the object to open on double-click, if any.
    fn metadata_tree_body(
        &mut self,
        ui: &mut egui::Ui,
        connected: bool,
    ) -> Option<(String, String, String)> {
        if !connected {
            ui.add_space(6.0);
            ui.colored_label(p().text_dim, "  Connect to a database\n  to browse metadata.");
            return None;
        }
        let Some(schema) = self.meta_schema_sel.clone() else {
            ui.add_space(6.0);
            ui.colored_label(p().text_dim, "  Scanning metadata…");
            return None;
        };
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut open_obj = None;
        let (ctrl, shift) = ui.input(|i| (i.modifiers.ctrl, i.modifiers.shift));

        // One tree row: optional disclosure chevron, optional object icon, then the label. Painted
        // (not laid out) so rows are flush; returns the click response.
        // group the selected schema's objects by folder, in FOLDER_ORDER
        let mut folders: Vec<(&str, Vec<&str>)> =
            FOLDER_ORDER.iter().map(|f| (*f, Vec::new())).collect();
        for o in &self.meta_view.objects {
            if o.schema != schema {
                continue;
            }
            if let Some(slot) = folders.iter_mut().find(|(label, _)| *label == o.kind) {
                slot.1.push(o.name.as_str());
            }
        }
        let any = folders.iter().any(|(_, v)| !v.is_empty());
        if !any {
            ui.add_space(6.0);
            ui.colored_label(p().text_dim, "  No objects in this schema.");
            return None;
        }
        // flat list of visible object keys ("schema/kind/name"), in render order, for Shift-range
        let mut visible: Vec<String> = Vec::new();
        for (label, objs) in &folders {
            if objs.is_empty() {
                continue;
            }
            if self.meta_folders_open.contains(&format!("{schema}/{label}")) {
                for name in objs {
                    visible.push(format!("{schema}/{label}/{name}"));
                }
            }
        }
        let mut plain: Option<usize> = None; // plain-pressed object (flat index into `visible`)
        let mut modified: Option<usize> = None; // Ctrl/Shift-clicked object
        let mut k = 0usize;
        for (label, objs) in &folders {
            if objs.is_empty() {
                continue;
            }
            let key = format!("{schema}/{label}");
            let open = self.meta_folders_open.contains(&key);
            let chev = if open { ic::TREE_EXPANDED } else { ic::TREE_COLLAPSED };
            if manager_row(ui, 0.0, Some(chev), &format!("{label} ({})", objs.len()), false).clicked() {
                if open {
                    self.meta_folders_open.remove(&key);
                } else {
                    self.meta_folders_open.insert(key.clone());
                }
            }
            if !open {
                continue;
            }
            let icon = kind_icon(label);
            for name in objs {
                let okey = format!("{schema}/{label}/{name}");
                let selected = self.meta_obj_sel.contains(&okey);
                // indent one glyph-column so the object icon lines up under the folder's label
                let resp = manager_row(ui, crate::widgets::MGR_GLYPH_COL, Some(icon), name, selected);
                if resp.double_clicked() {
                    open_obj = Some((schema.clone(), (*name).to_owned(), (*label).to_owned()));
                } else if resp.is_pointer_button_down_on() && !ctrl && !shift {
                    // plain select on PRESS (not release): the old row's accent drops the instant
                    // you press the new one, instead of lingering for the whole button-hold
                    plain = Some(k);
                } else if resp.clicked() {
                    modified = Some(k); // Ctrl/Shift resolve on release (toggle / range)
                }
                k += 1;
            }
        }
        if let Some(i) = plain {
            let key = visible[i].clone();
            if self.meta_obj_sel.as_slice() != [key.clone()] {
                self.meta_obj_sel = vec![key];
            }
            self.meta_obj_anchor = Some(i);
            ui.ctx().request_repaint();
        } else if let Some(i) = modified {
            select_click(&mut self.meta_obj_sel, &mut self.meta_obj_anchor, &visible, i, ctrl, shift);
            ui.ctx().request_repaint();
        }
        open_obj
    }

    /// Render the active metadata tab: object identity + columns (fetched on demand) / note / error.
    pub(crate) fn metadata_tab(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(self.island_margin()))
            .show_inside(ui, |ui| {
                let sheet = ui.max_rect();
                crate::widgets::island_shadow_under(ui.painter(), sheet);
                crate::widgets::island_box(ui.painter(), sheet, p().data_bg, crate::RADIUS_ISLAND);
                let idx = self.active_tab.min(self.tabs.len().saturating_sub(1));
                let Some(m) = self.tabs.get(idx).and_then(|t| t.meta.as_ref()) else {
                    return;
                };
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(18, 16))
                        .show(ui, |ui| {
                            ui.style_mut().visuals.override_text_color = None;
                            ui.label(
                                RichText::new(format!("{}.{}", m.schema, m.name))
                                    .size(15.0)
                                    .strong()
                                    .color(p().text),
                            );
                            ui.label(RichText::new(&m.kind).color(p().text_dim).size(12.0));
                            ui.add_space(12.0);
                            match &m.state {
                                MetaState::Loading(_) => {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.add_space(8.0);
                                        ui.label(RichText::new("Loading columns…").color(p().text_dim));
                                    });
                                }
                                MetaState::Deleted => {
                                    ui.colored_label(p().danger, "This object no longer exists (deleted).");
                                }
                                MetaState::Failed(e) => {
                                    ui.colored_label(p().danger, e);
                                }
                                MetaState::NoColumns => {
                                    ui.colored_label(
                                        p().text_dim,
                                        "No column metadata for this object type.",
                                    );
                                }
                                MetaState::Loaded(cols) => {
                                    if cols.is_empty() {
                                        ui.colored_label(p().text_dim, "No columns.");
                                    } else {
                                        egui::Grid::new("meta_cols")
                                            .num_columns(4)
                                            .striped(true)
                                            .spacing([18.0, 6.0])
                                            .show(ui, |ui| {
                                                for h in ["Column", "Type", "Nullable", "Default"] {
                                                    ui.label(
                                                        RichText::new(h)
                                                            .strong()
                                                            .color(p().text_dim)
                                                            .size(12.0),
                                                    );
                                                }
                                                ui.end_row();
                                                for c in cols {
                                                    ui.label(RichText::new(&c.name).color(p().text));
                                                    ui.label(RichText::new(&c.ty).color(p().text));
                                                    ui.label(
                                                        RichText::new(if c.nullable {
                                                            "yes"
                                                        } else {
                                                            "no"
                                                        })
                                                        .color(p().text),
                                                    );
                                                    ui.label(
                                                        RichText::new(&c.default).color(p().text_dim),
                                                    );
                                                    ui.end_row();
                                                }
                                            });
                                    }
                                }
                            }
                        });
                });
            });
    }
}
