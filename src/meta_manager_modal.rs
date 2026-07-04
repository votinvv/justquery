//! The metadata-collector pages, split into two singleton tabs. **Session** is a live view of the
//! control connection (server / db / user / pid / since / ssl); **Scan** is the metadata-scan
//! controls — enable / disable, interval, budget, a two-pane monitored-schema picker and a short
//! activity log. Both render on the silvery data sheet and carry NO buttons of their own — the
//! actions live on the main toolbar (Scan: Execute/Stop = enable/disable, Save = Apply). Setting
//! edits are staged in `self.edit_*` and pushed to the running collector + persisted to the active
//! `.conn` file on Apply. The status-bar `scan` chip opens the Scan tab; the connection chip opens
//! the Session tab.

use crate::theme::p;
use crate::widgets::{list_pane, style_scrollbar, transfer_btn};
use crate::{connections, ic, metadata, theme, JustQueryApp};
use crate::SPACE_2;
use eframe::egui;
use egui::{Align, Color32, Id, Layout, Margin, RichText, Sense, Vec2};

/// The scanner lifecycle as a single (icon, word, colour, tooltip). Used by the Scan tab and by the
/// status-bar `scan` chip. **active** green covers both an in-progress scan and the "cooldown" wait
/// between scans; **asleep** is also green — it's a healthy idle that resumes on activity (distinct
/// from a problem); **stopped** red covers everything that halted scanning — user-disabled, over
/// budget or a scan error. Just the state word, no reasons: the reason for a stop is the last
/// activity-log line.
pub(crate) fn scan_state(
    st: &metadata::CollectorStatus,
) -> (&'static str, &'static str, Color32, &'static str) {
    if st.stopped || st.last_error.is_some() {
        (ic::SCAN, "stopped", p().danger, "Scan — stopped")
    } else if st.asleep {
        (ic::SCAN, "asleep", p().ok, "Scan — asleep")
    } else {
        (ic::SCAN, "active", p().ok, "Scan — active")
    }
}

impl JustQueryApp {
    /// Render the Scan tab: the metadata-collector controls on the silvery data sheet — live scan
    /// status, the three numeric settings (interval / sleep-after-idle / budget) laid out
    /// horizontally, a two-pane monitored-schema transfer picker, and a short activity log. The
    /// actions live on the main toolbar — Execute/Stop = enable/disable the scanner, Save = Apply
    /// the staged settings — so the body carries no buttons. Disabled wholesale when the connection
    /// is broken (nothing to manage on a dead collector).
    pub(crate) fn scan_tab(&mut self, ui: &mut egui::Ui) {
        // keep waking the UI so background scans (arriving on the worker's own timer) are drained
        // and shown without needing input
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));

        // staged edits applied after the closures (avoid borrowing self twice)
        let mut set_schemas: Option<Option<Vec<String>>> = None;
        let st = self.collector_status.clone();
        let broken = self.conn_broken;
        // settings are staged in self.edit_* and only pushed on Apply (toolbar Save); bind locals,
        // write back after the render
        let mut interval = self.edit_interval;
        let mut budget = self.edit_budget;
        let mut idle = self.edit_idle;

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(self.island_margin()))
            .show(ui, |ui| {
                let sheet = ui.max_rect();
                crate::widgets::island_shadow_under(ui.painter(), sheet);
                crate::widgets::island_box(ui.painter(), sheet, p().data_bg, crate::RADIUS_ISLAND);
                let inner = sheet.shrink(1.0);
                let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
                ui.set_clip_rect(inner);
                style_scrollbar(&mut ui);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(&mut ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(18, 16))
                        .show(ui, |ui| {
                            theme::style_modal_widgets(ui);
                            ui.set_max_width(600.0);

                            // ---- Scan block: disabled entirely when the connection is broken ----
                            ui.vertical(|ui| {
                                if broken {
                                    ui.disable();
                                }
                                // ---- title row: "Scan" + the live state beside it, like the
                                // connection tab's "name ● active" (no reasons — the activity log
                                // below carries them). The page lays flat on the data sheet: the
                                // old inner "Scan" island was a leftover from when this page also
                                // hosted the general connection info.
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Scan")
                                            .font(theme::ui_bold_font(crate::HEADING_SIZE))
                                            .color(p().text),
                                    );
                                    ui.add_space(SPACE_2);
                                    let (_, word, col, _) = scan_state(&st);
                                    ui.label(RichText::new("●").color(col));
                                    ui.label(RichText::new(word).color(col).size(crate::LABEL_SIZE));
                                });
                                if broken {
                                    ui.add_space(2.0);
                                    ui.label(
                                        RichText::new(
                                            "disabled — reconnect from the toolbar to manage",
                                        )
                                        .color(p().text_dim)
                                        .size(crate::LABEL_SIZE),
                                    );
                                }

                                // the live scan error (if any) — counts live in the activity log
                                if let Some(e) = &st.last_error {
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new(format!("Last error: {e}"))
                                            .color(p().danger)
                                            .size(crate::LABEL_SIZE),
                                    );
                                }
                                ui.add_space(12.0);

                                // the content width every full-width block aligns to
                                let content_w = ui.available_width();

                                // ---- settings: the three numeric values laid out HORIZONTALLY ----
                                const FIELD_H: f32 = theme::FIELD_H;
                                const FIELD_W: f32 = 96.0;
                                let gap = 8.0;
                                let num_col =
                                    |ui: &mut egui::Ui, label: &str, key: &str, v: u64, lo: u64, hi: u64| -> u64 {
                                        ui.vertical(|ui| {
                                            ui.label(RichText::new(label).color(p().text_dim).size(crate::LABEL_SIZE));
                                            ui.add_space(4.0);
                                            num_field(ui, key, Vec2::new(FIELD_W, FIELD_H), v, lo, hi)
                                        })
                                        .inner
                                    };
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = 16.0;
                                    interval = num_col(ui, "Scan interval, s", "interval", interval, 5, 3600);
                                    idle = num_col(ui, "Sleep after idle, s", "idle", idle, 60, 7200);
                                    budget =
                                        num_col(ui, "Budget (objects + attrs)", "budget", budget as u64, 1000, 100_000_000)
                                            as usize;
                                });

                                // ---- monitored schemas: a two-pane transfer picker ----
                                ui.label(RichText::new("Monitored schemas").color(p().text_dim).size(crate::LABEL_SIZE));
                                ui.add_space(4.0);
                                let all_schemas: Vec<String> = self
                                    .meta_store
                                    .store
                                    .read()
                                    .map(|s| s.schemas.clone())
                                    .unwrap_or_default();
                                let mon_set: std::collections::HashSet<&String> =
                                    match &self.edit_schemas {
                                        None => all_schemas.iter().collect(),
                                        Some(list) => list.iter().collect(),
                                    };
                                let monitored: Vec<String> = all_schemas
                                    .iter()
                                    .filter(|s| mon_set.contains(*s))
                                    .cloned()
                                    .collect();
                                let available: Vec<String> = all_schemas
                                    .iter()
                                    .filter(|s| !mon_set.contains(*s))
                                    .cloned()
                                    .collect();
                                let normalize = |m: Vec<String>| -> Option<Vec<String>> {
                                    let set: std::collections::HashSet<String> =
                                        m.into_iter().collect();
                                    if set.len() == all_schemas.len() {
                                        None
                                    } else {
                                        Some(
                                            all_schemas
                                                .iter()
                                                .filter(|s| set.contains(*s))
                                                .cloned()
                                                .collect(),
                                        )
                                    }
                                };

                                const PANE_H: f32 = 122.0;
                                const BTN: Vec2 = Vec2::new(24.0, 24.0);
                                let pane_w = ((content_w - BTN.x - 2.0 * gap) / 2.0).floor();
                                let pane = Vec2::new(pane_w, PANE_H);
                                let add_sel: Vec<String> = self
                                    .meta_sel_avail
                                    .iter()
                                    .filter(|p| available.contains(p))
                                    .cloned()
                                    .collect();
                                let rem_sel: Vec<String> = self
                                    .meta_sel_mon
                                    .iter()
                                    .filter(|p| monitored.contains(p))
                                    .cloned()
                                    .collect();
                                let mut avail_rect = None;
                                let mut mon_rect = None;
                                let mut dbl_avail = None;
                                let mut dbl_mon = None;
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = gap;
                                    let (r, d) = list_pane(
                                        ui,
                                        "scan_avail",
                                        pane,
                                        &available,
                                        &mut self.meta_sel_avail,
                                        &mut self.meta_anchor_avail,
                                    );
                                    avail_rect = Some(r);
                                    dbl_avail = d;
                                    let (col, _) = ui
                                        .allocate_exact_size(Vec2::new(BTN.x, PANE_H), Sense::hover());
                                    let group_h = 4.0 * BTN.y + 3.0 * SPACE_2;
                                    let top_off = ((PANE_H - group_h) / 2.0).max(0.0);
                                    let brect = |i: usize| {
                                        egui::Rect::from_min_size(
                                            egui::pos2(
                                                col.left(),
                                                col.top() + top_off + i as f32 * (BTN.y + SPACE_2),
                                            ),
                                            BTN,
                                        )
                                    };
                                    let tbtn =
                                        |ui: &mut egui::Ui, i: usize, left: bool, double: bool, on: bool, tip: &str| {
                                            let mut cui =
                                                ui.new_child(egui::UiBuilder::new().max_rect(brect(i)));
                                            transfer_btn(&mut cui, BTN, left, double, on, tip)
                                        };
                                    if tbtn(ui, 0, false, true, !available.is_empty(), "Monitor all") {
                                        set_schemas = Some(None);
                                        self.meta_sel_avail.clear();
                                        self.meta_sel_mon.clear();
                                    }
                                    if tbtn(ui, 1, false, false, !add_sel.is_empty(), "Monitor selected") {
                                        let mut m = monitored.clone();
                                        m.extend(add_sel.iter().cloned());
                                        set_schemas = Some(normalize(m));
                                        self.meta_sel_avail.clear();
                                        self.meta_anchor_avail = None;
                                    }
                                    if tbtn(ui, 2, true, false, !rem_sel.is_empty(), "Stop monitoring selected") {
                                        let m: Vec<String> = monitored
                                            .iter()
                                            .filter(|x| !rem_sel.contains(x))
                                            .cloned()
                                            .collect();
                                        set_schemas = Some(normalize(m));
                                        self.meta_sel_mon.clear();
                                        self.meta_anchor_mon = None;
                                    }
                                    if tbtn(ui, 3, true, true, !monitored.is_empty(), "Monitor none") {
                                        set_schemas = Some(Some(Vec::new()));
                                        self.meta_sel_avail.clear();
                                        self.meta_sel_mon.clear();
                                    }
                                    let (r, d) = list_pane(
                                        ui,
                                        "scan_mon",
                                        pane,
                                        &monitored,
                                        &mut self.meta_sel_mon,
                                        &mut self.meta_anchor_mon,
                                    );
                                    mon_rect = Some(r);
                                    dbl_mon = d;
                                });
                                if let Some(s) = dbl_avail {
                                    let mut m = monitored.clone();
                                    m.push(s);
                                    set_schemas = Some(normalize(m));
                                    self.meta_sel_avail.clear();
                                    self.meta_anchor_avail = None;
                                }
                                if let Some(s) = dbl_mon {
                                    let m: Vec<String> =
                                        monitored.iter().filter(|x| **x != s).cloned().collect();
                                    set_schemas = Some(normalize(m));
                                    self.meta_sel_mon.clear();
                                    self.meta_anchor_mon = None;
                                }
                                if let Some(p) = ui.input(|i| {
                                    i.pointer
                                        .primary_clicked()
                                        .then(|| i.pointer.interact_pos())
                                        .flatten()
                                }) {
                                    if avail_rect.is_some_and(|r| !r.contains(p)) {
                                        self.meta_sel_avail.clear();
                                        self.meta_anchor_avail = None;
                                    }
                                    if mon_rect.is_some_and(|r| !r.contains(p)) {
                                        self.meta_sel_mon.clear();
                                        self.meta_anchor_mon = None;
                                    }
                                }
                                ui.add_space(12.0);

                                // ---- activity log ----
                                ui.label(RichText::new("Activity log").color(p().text_dim).size(crate::LABEL_SIZE));
                                ui.add_space(4.0);
                                const LOG_ROWS: usize = 5;
                                let log_h = 100.0;
                                boxed(ui, log_h, true, |ui| {
                                    let mono = theme::code_font_regular(11.0);
                                    if self.collector_log.is_empty() {
                                        ui.label(RichText::new("—").color(p().text_dim).font(mono.clone()));
                                    }
                                    const TIME_W: f32 = 56.0;
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    let skip = self.collector_log.len().saturating_sub(LOG_ROWS);
                                    for l in self.collector_log.iter().skip(skip) {
                                        ui.horizontal_top(|ui| {
                                            ui.spacing_mut().item_spacing.x = SPACE_2;
                                            let (trect, _) = ui.allocate_exact_size(
                                                Vec2::new(TIME_W, 15.0),
                                                Sense::hover(),
                                            );
                                            ui.painter().rect_filled(trect, 0.0, p().row_alt);
                                            ui.painter().text(
                                                egui::pos2(trect.left() + 4.0, trect.center().y),
                                                egui::Align2::LEFT_CENTER,
                                                &l.time,
                                                mono.clone(),
                                                p().text_dim,
                                            );
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&l.text).color(p().text).font(mono.clone()),
                                                )
                                                .wrap(),
                                            );
                                        });
                                    }
                                });
                            }); // page disabled-when-broken wrapper
                        });
                    });
                });

        // write staged field edits back to the buffers (in-memory until Apply on the toolbar)
        self.edit_interval = interval;
        self.edit_budget = budget;
        self.edit_idle = idle;
        if let Some(s) = set_schemas {
            self.edit_schemas = s;
        }
    }

    /// Enable/disable the live collector and persist the choice to the active connection. The Scan
    /// tab's toolbar Execute (enable) / Stop (disable) call this.
    pub(crate) fn set_collector_enabled(&mut self, on: bool) {
        if let Some(h) = &self.collector {
            if on {
                h.resume();
            } else {
                h.pause();
            }
        }
        self.apply_meta_setting(|c| c.meta_enabled = on);
    }

    /// Push the staged setting edits (`edit_*`) to the live collector and persist them. The Scan
    /// tab's toolbar Save (Apply) calls this.
    pub(crate) fn apply_meta_edits(&mut self) {
        let (i, b, d, s) = (
            self.edit_interval,
            self.edit_budget,
            self.edit_idle,
            self.edit_schemas.clone(),
        );
        if let Some(h) = &self.collector {
            h.set_interval(i);
            h.set_budget(b);
            h.set_idle(d);
            h.set_schemas(s.clone());
        }
        self.apply_meta_setting(move |c| {
            c.meta_interval = i;
            c.meta_budget = b;
            c.meta_idle = d;
            c.meta_schemas = s.clone();
        });
    }

    /// True when the staged scan settings (`edit_*`) differ from what's persisted on the active
    /// connection — i.e. there's something for Apply (the Scan tab's toolbar Save) to do. With no
    /// active connection, or while the connection is broken (the body is disabled then — nothing to
    /// apply to a dead collector), there's nothing to apply.
    pub(crate) fn can_apply_scan(&self) -> bool {
        if self.conn_broken {
            return false;
        }
        let stored = self
            .active_conn_id
            .and_then(|id| self.connections.iter().find(|c| c.id == id));
        stored.is_some_and(|c| {
            self.edit_interval != c.meta_interval
                || self.edit_budget != c.meta_budget
                || self.edit_idle != c.meta_idle
                || self.edit_schemas != c.meta_schemas
        })
    }

    /// Reload the staged setting buffers from the active connection (discard unapplied edits).
    pub(crate) fn reload_meta_edits(&mut self) {
        let Some(id) = self.active_conn_id else {
            return;
        };
        if let Some(c) = self.connections.iter().find(|c| c.id == id) {
            self.edit_interval = c.meta_interval;
            self.edit_budget = c.meta_budget;
            self.edit_idle = c.meta_idle;
            self.edit_schemas = c.meta_schemas.clone();
        }
    }

    /// Apply a change to the active connection's metadata settings and persist all connections.
    fn apply_meta_setting(&mut self, f: impl Fn(&mut connections::Connection)) {
        let Some(id) = self.active_conn_id else {
            return;
        };
        if let Some(c) = self.connections.iter_mut().find(|c| c.id == id) {
            f(c);
            connections::save(&self.connections);
        }
    }
}

/// A right-aligned numeric text field. Shows `value`, lets the user type a new one (parsed and
/// clamped to `min..=max`), and snaps back to the clamped value on blur. The in-progress text is
/// kept in egui temp memory keyed by `key`, so typing isn't fought by the live value each frame.
fn num_field(ui: &mut egui::Ui, key: &str, size: Vec2, value: u64, min: u64, max: u64) -> u64 {
    let id = Id::new(("scan_num", key));
    let mut buf = ui
        .data_mut(|d| d.get_temp::<String>(id))
        .unwrap_or_default();
    if !ui.memory(|m| m.has_focus(id)) {
        buf = value.to_string(); // not being edited → reflect the live value
    }
    let resp = ui.add_sized(
        size,
        egui::TextEdit::singleline(&mut buf)
            .id(id)
            // shared field inset + vertical centring (theme.rs), so the scan numeric fields match
            // every other input
            .margin(crate::theme::field_margin())
            .vertical_align(Align::Center)
            .horizontal_align(Align::Min),
    );
    let out = buf
        .trim()
        .parse::<u64>()
        .map(|v| v.clamp(min, max))
        .unwrap_or(value);
    if resp.lost_focus() {
        buf = out.to_string(); // snap to the clamped value once editing ends
    }
    ui.data_mut(|d| d.insert_temp(id, buf));
    out
}

/// A white, thin-bordered scroll box of fixed height whose scrollbar hugs the right edge
/// (non-floating, full-width content) and only appears when the content overflows. The activity log.
fn boxed(ui: &mut egui::Ui, height: f32, stick: bool, add: impl FnOnce(&mut egui::Ui)) {
    // a fixed-height white box drawn like the manager islands: crisp 1px border, content clipped
    // 1px inside it, and the scrollbar spanning the full height (so it reaches the very bottom)
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, height), egui::Sense::hover());
    crate::widgets::island_shadow_under(ui.painter(), rect);
    crate::widgets::island_box(ui.painter(), rect, p().field_bg, crate::RADIUS_ISLAND);
    // The scroll area spans the FULL box so its bar reaches the very bottom; only the content (text)
    // is clipped 1px inside the border (set inside the closure → the bar itself isn't clipped).
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    style_scrollbar(&mut child);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .stick_to_bottom(stick)
        .show(&mut child, |ui| {
            // rounded-content law (Design Delta v2.1 §3): ≥6px padding on every side and a clip
            // that keeps text clear of the rounded corners — the first/last line may never touch
            // the frame (the scrollbar lives outside this clip and still spans the full box)
            ui.set_clip_rect(rect.shrink2(Vec2::new(1.0, 6.0)));
            ui.set_width(ui.available_width());
            theme::style_modal_widgets(ui);
            egui::Frame::new()
                .inner_margin(Margin {
                    left: 6,
                    right: 6,
                    top: 6,
                    bottom: 6,
                })
                .show(ui, |ui| add(ui));
        });
}
