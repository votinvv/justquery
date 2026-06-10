//! The collector status-bar indicator + the collector manager modal (enable / pause / rescan,
//! interval, budget, a two-pane monitored-schema picker, and a short activity log). Setting edits
//! are staged and pushed to the running collector + persisted to the active `.conn` file on Apply/OK.

use crate::widgets::{
    close_x, list_pane, primary_button, primary_button_w, secondary_button_w, show_modal,
    style_scrollbar, transfer_btn, uniform_button_width,
};
use crate::theme::p;
use crate::{connections, ic, metadata, theme, JustQueryApp};
use crate::{SPACE_2, SPACE_4, SPACE_5};
use eframe::egui;
use egui::{Align, Color32, Id, Layout, Margin, RichText, Sense, Vec2};

/// The scanner lifecycle as a single (icon, word, colour, tooltip), shared by the status-bar chip
/// and the modal header so the two never drift. **active** green covers both an in-progress scan
/// and the "перекур" wait between scans; **asleep** yellow is the idle-pause; **failed** red is an
/// error / over budget; **disabled** grey is user-paused.
fn scan_state(
    st: &metadata::CollectorStatus,
) -> (&'static str, &'static str, Color32, &'static str) {
    if st.over_budget || st.last_error.is_some() {
        (
            ic::SCAN_FAIL,
            "failed",
            p().danger,
            "Scan — stopped (buffer full / error); click to manage",
        )
    } else if st.paused {
        (
            ic::SCAN_OFF,
            "paused",
            p().warn,
            "Scan — paused; click to manage",
        )
    } else if st.asleep {
        (
            ic::SCAN_SLEEP,
            "asleep",
            p().ok,
            "Scan — asleep (idle, resumes on activity); click to manage",
        )
    } else {
        (ic::SCAN_OK, "active", p().ok, "Scan — active; click to manage")
    }
}

impl JustQueryApp {
    /// The "scan" status-bar label: plain text, same font/size as the rest of the bar, coloured by
    /// the scanner lifecycle (see [`scan_state`]). No glyph. Rendered only while connected. Click →
    /// Scan modal.
    pub(crate) fn meta_status_indicator(&mut self, ui: &mut egui::Ui, sz: f32) {
        if !self.connected {
            return;
        }
        let (_, _, color, tip) = scan_state(&self.collector_status);
        let resp = ui.add(
            egui::Label::new(RichText::new("scan").size(sz).color(color))
                .sense(egui::Sense::click()),
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.on_hover_text(tip).clicked() {
            self.open_scan_tab();
        }
    }

    /// The Scan (metadata collector) manager modal (replaces the old Scan tab). Enable / pause /
    /// rescan, interval / budget / idle settings, a two-pane monitored-schema picker, and a live
    /// activity log. Numeric settings are staged and pushed to the collector + persisted on Apply.
    /// Page actions live in the footer (Design System §7); Enter = Apply, Esc = close.
    pub(crate) fn scan_modal(&mut self, ctx: &egui::Context) {
        if !self.scan_open {
            return;
        }
        // keep waking the UI so background scans (arriving on the worker's own timer) are drained
        // and shown without needing input
        ctx.request_repaint_after(std::time::Duration::from_millis(500));

        // staged edits applied after the closure (avoid borrowing self twice)
        let mut apply = false; // flush staged settings to the collector + disk
        let mut do_toggle_enabled: Option<bool> = None;
        let mut do_rescan = false;
        let mut set_schemas: Option<Option<Vec<String>>> = None;
        let mut close = false;

        let st = self.collector_status.clone();
        let connected = self.connected;
        // settings are staged in self.edit_* and only pushed on Apply; bind locals, write back
        let mut interval = self.edit_interval;
        let mut budget = self.edit_budget;
        let mut idle = self.edit_idle;
        // the active connection's persisted settings — Apply is enabled only when the staged edits
        // differ from these ("nothing to apply" → disabled)
        let stored = self
            .active_conn_id
            .and_then(|id| self.connections.iter().find(|c| c.id == id))
            .map(|c| (c.meta_interval, c.meta_budget, c.meta_idle, c.meta_schemas.clone()));
        let edit_schemas0 = self.edit_schemas.clone();

        let r = show_modal(ctx, "scan", 600.0, |ui| {
            theme::style_modal_widgets(ui);
            // header: title + × (no status glyph)
            ui.horizontal(|ui| {
                ui.label(RichText::new("Scan").font(theme::ui_bold_font(16.0)).color(p().text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_x(ui, 22.0, 4.0, "Close") {
                        close = true;
                    }
                });
            });

            // no connection → nothing to manage; just a hint + Close
            if !connected {
                ui.add_space(SPACE_4);
                ui.label(
                    RichText::new("Connect to a database to manage the metadata scanner.")
                        .color(p().text_dim),
                );
                ui.add_space(SPACE_5);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_button(ui, "Close", true) {
                        close = true;
                    }
                });
                return;
            }

            // the content width every full-width block (settings, panes, log) aligns to
            let content_w = ui.available_width();

                            // the live error (if any) — the size/object counts now live in the activity log
                            if let Some(e) = &st.last_error {
                                ui.add_space(4.0);
                                ui.label(
                                    RichText::new(format!("Last error: {e}"))
                                        .color(p().danger)
                                        .size(11.0),
                                );
                            }
                            ui.add_space(12.0);

                            // ---- settings: the three numeric values laid out HORIZONTALLY to keep
                            // the modal short (so it fits even a squeezed window without a scroll) ----
                            const FIELD_H: f32 = theme::FIELD_H;
                            const FIELD_W: f32 = 96.0; // fits ~9 digits, no wasted width
                            let gap = 8.0;
                            let num_col =
                                |ui: &mut egui::Ui, label: &str, key: &str, v: u64, lo: u64, hi: u64| -> u64 {
                                    ui.vertical(|ui| {
                                        ui.label(RichText::new(label).color(p().text_dim).size(11.0));
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

                            // ---- monitored schemas: a two-pane transfer picker (available ⇄ monitored),
                            // the whole block spanning the same width as the activity-log box ----
                            // (label per the form law: Small/text_dim, 4px to the control)
                            ui.label(RichText::new("Monitored schemas").color(p().text_dim).size(11.0));
                            ui.add_space(4.0);
                            let all_schemas: Vec<String> = self
                                .meta_store
                                .store
                                .read()
                                .map(|s| s.schemas.clone())
                                .unwrap_or_default();
                            // current selection (None = all monitored), split into the two panes in
                            // catalogue order
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
                            // None when every schema is monitored, Some(list) otherwise (Some(empty) = none)
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
                            // 24×24 icon buttons, 14px glyphs (Design System v2 §5 Icon button)
                            const BTN: Vec2 = Vec2::new(24.0, 24.0);
                            // pane width derived from the captured content width so the block lines up
                            // edge-to-edge with the activity-log box below it
                            let pane_w = ((content_w - BTN.x - 2.0 * gap) / 2.0).floor();
                            let pane = Vec2::new(pane_w, PANE_H);
                            // collect picks that survive the current pane contents
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
                                // left: available (not monitored)
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
                                // middle: four transfer buttons placed at exact rects, evenly spread top→
                                // bottom across the full pane height (egui's auto-layout sized them shorter,
                                // leaving them off-centre — explicit rects keep top/bottom flush with the panes)
                                let (col, _) = ui
                                    .allocate_exact_size(Vec2::new(BTN.x, PANE_H), Sense::hover());
                                // the 4 buttons are one group, vertically centred in the pane with
                                // equal SPACE_2 gaps (Design System §6 Transfer list)
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
                                // right: monitored
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
                            // double-click a schema → move it across
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
                            // a click anywhere outside a pane clears that pane's selection
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

                            // ---- activity log (newest at the bottom; each scan line carries the estimate) ----
                            ui.label(RichText::new("Activity log").color(p().text_dim).size(11.0));
                            ui.add_space(4.0);
                            // only the last few entries are shown, so a short fixed box fits them
                            // without scrolling — keeps the modal compact in a squeezed window
                            const LOG_ROWS: usize = 5;
                            let log_h = 100.0;
                            boxed(ui, log_h, true, |ui| {
                                // log is "data" → monospace (Design System §3); timestamps sit in a
                                // row_alt-tinted gutter column, the wrapped text hangs to its right
                                let mono = theme::code_font_regular(11.0);
                                if self.collector_log.is_empty() {
                                    ui.label(RichText::new("—").color(p().text_dim).font(mono.clone()));
                                }
                                const TIME_W: f32 = 56.0;
                                ui.spacing_mut().item_spacing.y = 2.0; // tight rows → continuous gutter
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
                        // footer: Close (primary/accent) + Apply on the right; Disable/Enable +
                        // Rescan now on the left — page actions at the bottom (Design System §7).
                        // Apply is disabled when the staged edits match what's saved (nothing to apply).
                        let eff_schemas = set_schemas.as_ref().unwrap_or(&edit_schemas0);
                        let can_apply = stored.as_ref().is_some_and(|(i, b, d, s)| {
                            interval != *i || budget != *b || idle != *d || eff_schemas != s
                        });
                        ui.add_space(SPACE_4);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let bw = uniform_button_width(
                                ui,
                                &["Apply", "Close", "Disable", "Enable", "Rescan now"],
                            );
                            if primary_button_w(ui, "Close", true, bw) {
                                close = true;
                            }
                            ui.add_space(SPACE_2);
                            if secondary_button_w(ui, "Apply", can_apply, bw) {
                                apply = true;
                            }
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                let (label, on) =
                                    if st.paused { ("Enable", true) } else { ("Disable", false) };
                                if secondary_button_w(ui, label, true, bw) {
                                    do_toggle_enabled = Some(on);
                                }
                                ui.add_space(SPACE_2);
                                if secondary_button_w(ui, "Rescan now", !st.paused, bw) {
                                    do_rescan = true;
                                }
                            });
                        });
                });

        // write staged field edits back to the buffers (in-memory until Apply)
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
        // Close is the accented/primary action now, so Enter closes (Esc/× too); Apply is a click
        if close || r.escape || r.enter {
            self.scan_open = false;
        }
    }

    /// Push the staged setting edits (`edit_*`) to the live collector and persist them.
    fn apply_meta_edits(&mut self) {
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
