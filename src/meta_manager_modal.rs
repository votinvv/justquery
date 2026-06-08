//! The collector status-bar indicator + the collector manager modal (enable / pause / rescan,
//! interval, budget, a two-pane monitored-schema picker, and a short activity log). Setting edits
//! are staged and pushed to the running collector + persisted to the active `.conn` file on Apply/OK.

use crate::widgets::{close_x, crisp_border, dim, list_pane, style_scrollbar, transfer_btn};
use crate::{connections, ic, metadata, theme, JustQueryApp};
use crate::{BORDER_STRONG, DANGER, DISABLED, OK, TEXT, TEXTDIM, WARN};
use eframe::egui;
use egui::{Align, Color32, CornerRadius, Id, Layout, Margin, RichText, Sense, Vec2};

/// The scanner lifecycle as a single (icon, word, colour, tooltip), shared by the status-bar chip
/// and the modal header so the two never drift. **active** green covers both an in-progress scan
/// and the "перекур" wait between scans; **asleep** yellow is the idle-pause; **failed** red is an
/// error / over budget; **disabled** grey is user-paused.
fn scan_state(st: &metadata::CollectorStatus) -> (&'static str, &'static str, Color32, &'static str) {
    if st.over_budget || st.last_error.is_some() {
        (ic::SCAN_FAIL, "failed", DANGER, "Scan — stopped (error / over budget); click to manage")
    } else if st.paused {
        (ic::SCAN_OFF, "disabled", DISABLED, "Scan — disabled; click to manage")
    } else if st.asleep {
        (ic::SCAN_SLEEP, "asleep", WARN, "Scan — asleep (idle); click to manage")
    } else {
        (ic::SCAN_OK, "active", OK, "Scan — active; click to manage")
    }
}

impl JustQueryApp {
    /// The SCAN chip for the status bar: a status glyph + bold **SCAN**, coloured by the scanner
    /// lifecycle (see [`scan_state`]). Grey and **not clickable** with no connection. Click → Scan
    /// modal. The caller reserves the file-status slot to its right, so the chip keeps its place.
    pub(crate) fn meta_status_indicator(&mut self, ui: &mut egui::Ui, sz: f32) {
        if !self.connected {
            ui.label(
                RichText::new(format!("{} SCAN", ic::SCAN_OFF))
                    .font(theme::ui_bold_font(sz))
                    .color(DISABLED),
            ); // grey, inert
            return;
        }
        let (icon, _, color, tip) = scan_state(&self.collector_status);
        let resp = ui.add(
            egui::Label::new(
                RichText::new(format!("{icon} SCAN")).font(theme::ui_bold_font(sz)).color(color),
            )
            .sense(Sense::click()),
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.on_hover_text(tip).clicked() {
            self.meta_mgr_open = true;
        }
    }

    /// The collector manager modal.
    pub(crate) fn meta_manager_modal(&mut self, ctx: &egui::Context) {
        if !self.meta_mgr_open {
            return;
        }
        // the modal shows live scanner status + activity log; keep waking the UI so background
        // scans (which arrive on the worker's own timer) are drained and shown without needing input
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        dim(ctx, "meta_mgr_dim");
        let mut close = false;
        let mut apply = false; // flush staged settings to the collector + disk
        // staged edits applied after the closure (avoid borrowing self twice)
        let mut do_toggle_enabled: Option<bool> = None;
        let mut do_rescan = false;
        let mut set_schemas: Option<Option<Vec<String>>> = None;

        let st = self.collector_status.clone();
        let (state_icon, _, state_color, _) = scan_state(&st);
        // settings are staged in self.edit_* and only pushed on Apply/OK; bind locals, write back
        let mut interval = self.edit_interval;
        let mut budget = self.edit_budget;
        let mut idle = self.edit_idle;

        egui::Area::new(Id::new("meta_mgr_box"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                theme::modal_frame().show(ui, |ui| {
                    ui.set_width(420.0);
                    theme::style_modal_widgets(ui);
                    // the content width every full-width block (settings, schema panes, log) aligns to
                    let content_w = ui.available_width();

                    // ---- header: status glyph + title + live status word, all coloured ----
                    ui.horizontal(|ui| {
                        // glyph + title, both coloured by the lifecycle (green active / yellow
                        // asleep / red failed / grey disabled) — the colour + glyph say the state,
                        // so no separate status word
                        ui.label(RichText::new(state_icon).size(15.0).color(state_color));
                        ui.add_space(3.0);
                        ui.label(RichText::new("Scan").font(theme::ui_bold_font(15.0)).color(state_color));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if close_x(ui, 22.0, 4.0, "Close") {
                                close = true;
                            }
                        });
                    });
                    // the live error (if any) — the size/object counts now live in the activity log
                    if let Some(e) = &st.last_error {
                        ui.add_space(4.0);
                        ui.label(RichText::new(format!("Last error: {e}")).color(DANGER).size(11.0));
                    }
                    ui.add_space(12.0);

                    // ---- settings: one field per row (Scan / Sleep / Budget), right-aligned values,
                    // all fields starting on a common column and filling to the content's right edge ----
                    {
                        let s = ui.spacing_mut();
                        s.interact_size.y = 20.0; // shorter input boxes
                        s.button_padding.y = 1.0;
                    }
                    const FIELD_H: f32 = 20.0;
                    const FIELD_W: f32 = 78.0; // fits ~9 digits right-aligned, no wasted width
                    let gap = 8.0;
                    // label column just wide enough for the longest label + a small gap, so the
                    // field starts right after "Budget (objects + attrs):" instead of out at the edge
                    let label_w = ui
                        .painter()
                        .layout_no_wrap(
                            "Budget (objects + attrs):".to_owned(),
                            egui::FontId::proportional(12.0),
                            TEXTDIM,
                        )
                        .size()
                        .x
                        + 8.0;
                    // a labelled, right-aligned number row with a fixed-width field
                    let srow = |ui: &mut egui::Ui, text: &str, key: &str, v: u64, lo: u64, hi: u64| -> u64 {
                        let mut out = v;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.allocate_ui_with_layout(
                                Vec2::new(label_w, FIELD_H),
                                Layout::left_to_right(Align::Center),
                                |ui| {
                                    ui.set_min_width(label_w);
                                    ui.label(RichText::new(text).color(TEXTDIM).size(12.0));
                                },
                            );
                            out = num_field(ui, key, Vec2::new(FIELD_W, FIELD_H), v, lo, hi);
                        });
                        out
                    };
                    interval = srow(ui, "Scan interval, s:", "interval", interval, 5, 3600);
                    ui.add_space(6.0);
                    idle = srow(ui, "Sleep after idle, s:", "idle", idle, 60, 7200);
                    ui.add_space(6.0);
                    budget = srow(ui, "Budget (objects + attrs):", "budget", budget as u64, 1000, 100_000_000) as usize;
                    ui.add_space(12.0);

                    // ---- monitored schemas: a two-pane transfer picker (available ⇄ monitored),
                    // the whole block spanning the same width as the activity-log box ----
                    ui.label(RichText::new("Monitored schemas:").color(TEXTDIM).size(12.0));
                    ui.add_space(2.0);
                    let all_schemas: Vec<String> = self
                        .meta_store
                        .store
                        .read()
                        .map(|s| s.schemas.clone())
                        .unwrap_or_default();
                    // current selection (None = all monitored), split into the two panes in
                    // catalogue order
                    let mon_set: std::collections::HashSet<&String> = match &self.edit_schemas {
                        None => all_schemas.iter().collect(),
                        Some(list) => list.iter().collect(),
                    };
                    let monitored: Vec<String> =
                        all_schemas.iter().filter(|s| mon_set.contains(*s)).cloned().collect();
                    let available: Vec<String> =
                        all_schemas.iter().filter(|s| !mon_set.contains(*s)).cloned().collect();
                    // None when every schema is monitored, Some(list) otherwise (Some(empty) = none)
                    let normalize = |m: Vec<String>| -> Option<Vec<String>> {
                        let set: std::collections::HashSet<String> = m.into_iter().collect();
                        if set.len() == all_schemas.len() {
                            None
                        } else {
                            Some(all_schemas.iter().filter(|s| set.contains(*s)).cloned().collect())
                        }
                    };

                    const PANE_H: f32 = 122.0;
                    const BTN: Vec2 = Vec2::new(36.0, 26.0);
                    // pane width derived from the captured content width so the block lines up
                    // edge-to-edge with the activity-log box below it
                    let pane_w = ((content_w - BTN.x - 2.0 * gap) / 2.0).floor();
                    let pane = Vec2::new(pane_w, PANE_H);
                    // collect picks that survive the current pane contents
                    let add_sel: Vec<String> =
                        self.meta_sel_avail.iter().filter(|p| available.contains(p)).cloned().collect();
                    let rem_sel: Vec<String> =
                        self.meta_sel_mon.iter().filter(|p| monitored.contains(p)).cloned().collect();
                    let mut avail_rect = None;
                    let mut mon_rect = None;
                    let mut dbl_avail = None;
                    let mut dbl_mon = None;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        // left: available (not monitored)
                        let (r, d) = list_pane(
                            ui, "scan_avail", pane, &available,
                            &mut self.meta_sel_avail, &mut self.meta_anchor_avail,
                        );
                        avail_rect = Some(r);
                        dbl_avail = d;
                        // middle: four transfer buttons placed at exact rects, evenly spread top→
                        // bottom across the full pane height (egui's auto-layout sized them shorter,
                        // leaving them off-centre — explicit rects keep top/bottom flush with the panes)
                        let (col, _) =
                            ui.allocate_exact_size(Vec2::new(BTN.x, PANE_H), Sense::hover());
                        // nudge the group down a touch: a bit more top inset than bottom
                        let top_off = 8.0;
                        let bot_off = 2.0;
                        let span = PANE_H - top_off - bot_off;
                        let bgap = (span - 4.0 * BTN.y) / 3.0;
                        let brect = |i: usize| {
                            egui::Rect::from_min_size(
                                egui::pos2(col.left(), col.top() + top_off + i as f32 * (BTN.y + bgap)),
                                BTN,
                            )
                        };
                        let tbtn = |ui: &mut egui::Ui, i: usize, g: &str, on: bool, tip: &str| {
                            let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(brect(i)));
                            transfer_btn(&mut cui, BTN, g, 20.0, on, tip)
                        };
                        if tbtn(ui, 0, ic::MOVE_ALL, !available.is_empty(), "Monitor all") {
                            set_schemas = Some(None);
                            self.meta_sel_avail.clear();
                            self.meta_sel_mon.clear();
                        }
                        if tbtn(ui, 1, ic::MOVE_ONE, !add_sel.is_empty(), "Monitor selected") {
                            let mut m = monitored.clone();
                            m.extend(add_sel.iter().cloned());
                            set_schemas = Some(normalize(m));
                            self.meta_sel_avail.clear();
                            self.meta_anchor_avail = None;
                        }
                        if tbtn(ui, 2, ic::BACK_ONE, !rem_sel.is_empty(), "Stop monitoring selected") {
                            let m: Vec<String> =
                                monitored.iter().filter(|x| !rem_sel.contains(x)).cloned().collect();
                            set_schemas = Some(normalize(m));
                            self.meta_sel_mon.clear();
                            self.meta_anchor_mon = None;
                        }
                        if tbtn(ui, 3, ic::BACK_ALL, !monitored.is_empty(), "Monitor none") {
                            set_schemas = Some(Some(Vec::new()));
                            self.meta_sel_avail.clear();
                            self.meta_sel_mon.clear();
                        }
                        // right: monitored
                        let (r, d) = list_pane(
                            ui, "scan_mon", pane, &monitored,
                            &mut self.meta_sel_mon, &mut self.meta_anchor_mon,
                        );
                        mon_rect = Some(r);
                        dbl_mon = d;
                    });
                    // double-click a schema → move it across
                    if let Some(s) = dbl_avail {
                        let mut m = monitored.clone();
                        m.push(s);
                        set_schemas = Some(normalize(m));
                        self.meta_sel_avail.clear();
                        self.meta_anchor_avail = None;
                    }
                    if let Some(s) = dbl_mon {
                        let m: Vec<String> = monitored.iter().filter(|x| **x != s).cloned().collect();
                        set_schemas = Some(normalize(m));
                        self.meta_sel_mon.clear();
                        self.meta_anchor_mon = None;
                    }
                    // a click anywhere outside a pane clears that pane's selection
                    if let Some(p) = ctx.input(|i| {
                        i.pointer.primary_clicked().then(|| i.pointer.interact_pos()).flatten()
                    }) {
                        if avail_rect.map_or(false, |r| !r.contains(p)) {
                            self.meta_sel_avail.clear();
                            self.meta_anchor_avail = None;
                        }
                        if mon_rect.map_or(false, |r| !r.contains(p)) {
                            self.meta_sel_mon.clear();
                            self.meta_anchor_mon = None;
                        }
                    }
                    ui.add_space(12.0);

                    // ---- activity log (newest at the bottom; each scan line carries the estimate) ----
                    ui.label(RichText::new("Activity log:").color(TEXTDIM).size(12.0));
                    ui.add_space(2.0);
                    boxed(ui, 110.0, true, |ui| {
                        if self.collector_log.is_empty() {
                            ui.label(RichText::new("—").color(TEXTDIM).size(11.0));
                        }
                        // each entry is laid out like a 2-column table (time | text) without drawing
                        // one: the time sits in a fixed column and the wrapped text hangs under itself
                        const TIME_W: f32 = 54.0;
                        for l in &self.collector_log {
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                ui.allocate_ui_with_layout(
                                    Vec2::new(TIME_W, 14.0),
                                    Layout::left_to_right(Align::Min),
                                    |ui| {
                                        ui.set_min_width(TIME_W);
                                        ui.label(RichText::new(&l.time).color(TEXTDIM).size(11.0));
                                    },
                                );
                                ui.add(
                                    egui::Label::new(RichText::new(&l.text).color(TEXT).size(11.0))
                                        .wrap(),
                                );
                            });
                        }
                    });
                    ui.add_space(14.0);

                    // ---- bottom button row: actions on the left, Apply / OK on the right,
                    // every button the same size ----
                    let bsz = Vec2::new(96.0, 26.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let (label, on) = if st.paused { ("Enable", true) } else { ("Disable", false) };
                        if ui.add_sized(bsz, egui::Button::new(label)).clicked() {
                            do_toggle_enabled = Some(on);
                        }
                        if ui
                            .add_enabled(!st.paused, egui::Button::new("Rescan now").min_size(bsz))
                            .clicked()
                        {
                            do_rescan = true;
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add_sized(bsz, egui::Button::new("OK")).clicked() {
                                apply = true;
                                close = true;
                            }
                            if ui.add_sized(bsz, egui::Button::new("Apply")).clicked() {
                                apply = true;
                            }
                        });
                    });
                });
            });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        // write staged field edits back to the buffers (in-memory only until Apply/OK)
        self.edit_interval = interval;
        self.edit_budget = budget;
        self.edit_idle = idle;
        if let Some(s) = set_schemas {
            self.edit_schemas = s;
        }
        // immediate actions (not "settings"): enable/disable + rescan act on the live collector
        if let Some(on) = do_toggle_enabled {
            if let Some(h) = &self.collector {
                if on {
                    h.resume();
                } else {
                    h.pause();
                }
            }
            self.apply_meta_setting(|c| c.meta_enabled = on);
        }
        if do_rescan {
            if let Some(h) = &self.collector {
                h.rescan();
            }
        }
        if apply {
            self.apply_meta_edits();
        }
        if close {
            // discard any unapplied staged edits so a re-open shows the real settings
            if !apply {
                self.reload_meta_edits();
            }
            self.meta_sel_avail.clear();
            self.meta_sel_mon.clear();
            self.meta_anchor_avail = None;
            self.meta_anchor_mon = None;
            self.meta_mgr_open = false;
        }
    }

    /// Push the staged setting edits (`edit_*`) to the live collector and persist them.
    fn apply_meta_edits(&mut self) {
        let (i, b, d, s) =
            (self.edit_interval, self.edit_budget, self.edit_idle, self.edit_schemas.clone());
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

    /// Reload the staged setting buffers from the active connection (discard unapplied edits).
    fn reload_meta_edits(&mut self) {
        let Some(id) = self.active_conn_id else { return };
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
    let mut buf = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_default();
    if !ui.memory(|m| m.has_focus(id)) {
        buf = value.to_string(); // not being edited → reflect the live value
    }
    let resp = ui.add_sized(
        size,
        egui::TextEdit::singleline(&mut buf).id(id).horizontal_align(Align::Max),
    );
    let out = buf.trim().parse::<u64>().map(|v| v.clamp(min, max)).unwrap_or(value);
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
    ui.painter().rect_filled(rect, CornerRadius::ZERO, Color32::WHITE);
    // The scroll area spans the FULL box so its bar reaches the very bottom; only the content (text)
    // is clipped 1px inside the border (set inside the closure → the bar itself isn't clipped).
    let mut child = ui.new_child(
        egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)),
    );
    style_scrollbar(&mut child);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .stick_to_bottom(stick)
        .show(&mut child, |ui| {
            ui.set_clip_rect(rect.shrink(1.0)); // keep text off the border (bar unaffected)
            ui.set_width(ui.available_width());
            theme::style_modal_widgets(ui);
            // small text inset so rows don't hug the border
            egui::Frame::new()
                .inner_margin(Margin { left: 5, right: 0, top: 3, bottom: 3 })
                .show(ui, |ui| add(ui));
        });
    crisp_border(ui.painter(), rect, BORDER_STRONG);
}
