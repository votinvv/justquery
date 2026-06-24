//! The collector status-bar indicator + the collector manager page (the Session tab: a live view
//! of the control connection — server/db/user/pid/since/ssl — plus the metadata scan settings:
//! enable / pause / rescan, interval, budget, a two-pane monitored-schema picker, and a short
//! activity log). Setting edits are staged and pushed to the running collector + persisted to the
//! active `.conn` file on Apply/OK.

use crate::widgets::{
    list_pane, secondary_button_w, style_scrollbar, transfer_btn,
    uniform_button_width,
};
use crate::theme::p;
use crate::{connections, ic, metadata, theme, JustQueryApp};
use crate::{SPACE_2, SPACE_4};
use eframe::egui;
use egui::{Align, Color32, Id, Layout, Margin, RichText, Sense, Vec2};

/// The scanner lifecycle as a single (icon, word, colour, tooltip). Used only inside the Session
/// tab now (the status-bar chip was dropped in favour of a clickable connection chip). **active**
/// green covers both an in-progress scan and the "перекур" wait between scans; **asleep** is also
/// green — it's a healthy idle that resumes on activity (distinct from a problem); **failed** red
/// is an error / over budget; **disabled** grey is user-paused.
fn scan_state(
    st: &metadata::CollectorStatus,
) -> (&'static str, &'static str, Color32, &'static str) {
    if st.over_budget || st.last_error.is_some() {
        (ic::SCAN_FAIL, "failed", p().danger, "Scan — stopped (buffer full / error)")
    } else if st.paused {
        (ic::SCAN_OFF, "paused", p().warn, "Scan — paused")
    } else if st.asleep {
        (ic::SCAN_SLEEP, "asleep", p().ok, "Scan — asleep (idle, resumes on activity)")
    } else {
        (ic::SCAN_OK, "active", p().ok, "Scan — active")
    }
}

impl JustQueryApp {
    /// Render the Session tab: a two-block island layout — Connection (live attributes of the
    /// control connection) on top, Scan (the metadata collector settings + log) below. In the
    /// broken state the Connection block carries the failure reason + a Reconnect button, and the
    /// whole Scan block is disabled. A deliberate disconnect or never-connected state never opens
    /// this tab (the connection chip is absent then).
    pub(crate) fn session_tab(&mut self, ui: &mut egui::Ui) {
        // keep waking the UI so background scans (arriving on the worker's own timer) are drained
        // and shown without needing input
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));

        // staged edits applied after the closures (avoid borrowing self twice)
        let mut apply = false; // flush staged settings to the collector + disk
        let mut do_toggle_enabled: Option<bool> = None;
        let mut do_rescan = false;
        let mut do_reconnect = false;
        let mut set_schemas: Option<Option<Vec<String>>> = None;

        let st = self.collector_status.clone();
        let broken = self.conn_broken;
        let conn_params = self.conn_params.clone();
        let main_pid = self.main_pid;
        let main_ssl = self.main_ssl;
        let main_since = self.main_conn_since.clone();
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
        let last_error = self.last_error.clone();

        // ---- body: the two islands on the silvery data sheet, normal tab scrolling ----
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(self.island_margin()))
            .show_inside(ui, |ui| {
                let sheet = ui.max_rect();
                crate::widgets::island_shadow_under(ui.painter(), sheet);
                crate::widgets::island_box(ui.painter(), sheet, p().data_bg, crate::RADIUS_ISLAND);
                // прокрутка ВНУТРИ рамки острова: иначе строки при скролле наезжают на верх/низ рамки
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

                            // ---- Connection block ----
                            crate::widgets::island(ui, |ui| {
                                egui::Frame::new()
                                    .inner_margin(Margin::symmetric(14, 12))
                                    .show(ui, |ui| {
                                        // header: title + status dot/word
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new("Connection")
                                                    .font(theme::ui_bold_font(13.0))
                                                    .color(p().text),
                                            );
                                            ui.add_space(SPACE_2);
                                            let (dot, word, col) = if broken {
                                                ("●", "Disconnected", p().danger)
                                            } else {
                                                ("●", "Connected", p().ok)
                                            };
                                            ui.label(RichText::new(dot).color(col));
                                            ui.label(
                                                RichText::new(word).color(col).size(11.0),
                                            );
                                        });
                                        // the failure reason, when the connection dropped
                                        if broken {
                                            if let Some(e) = &last_error {
                                                ui.add_space(4.0);
                                                ui.label(
                                                    RichText::new(e).color(p().danger).size(11.0),
                                                );
                                            }
                                        }
                                        ui.add_space(SPACE_2);
                                        // two-column key/value pairs (label/text_dim : value/text)
                                        let p_ = self.conn_params.clone();
                                        let kv = |ui: &mut egui::Ui, k: &str, v: String| {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new(k)
                                                        .color(p().text_dim)
                                                        .size(11.0),
                                                );
                                                ui.add_space(SPACE_2);
                                                ui.label(RichText::new(v).color(p().text).size(11.0));
                                            });
                                        };
                                        ui.columns(2, |cols| {
                                            kv(&mut cols[0], "Server", format!(
                                                "{}:{}",
                                                p_.as_ref().map(|p| p.host.clone()).unwrap_or_default(),
                                                p_.as_ref().map(|p| p.port.clone()).unwrap_or_default(),
                                            ));
                                            kv(&mut cols[0], "Database",
                                                p_.as_ref().map(|p| p.db.clone()).unwrap_or_else(|| "—".to_string()));
                                            kv(&mut cols[0], "Since",
                                                main_since.clone().unwrap_or_else(|| "—".to_string()));
                                            kv(&mut cols[1], "User",
                                                p_.as_ref().map(|p| p.user.clone()).unwrap_or_else(|| "—".to_string()));
                                            kv(&mut cols[1], "Pid",
                                                main_pid.map(|n| n.to_string()).unwrap_or_else(|| "—".to_string()));
                                            kv(&mut cols[1], "SSL",
                                                main_ssl.map(|b| if b { "on" } else { "off" }).unwrap_or_else(|| "—").to_string());
                                        });
                                        // Reconnect button — only in the broken state. No modal:
                                        // drop the dead control connection and reconnect with the
                                        // same captured credentials (start_main_connect).
                                        if broken {
                                            ui.add_space(SPACE_2);
                                            ui.horizontal(|ui| {
                                                let bw = uniform_button_width(ui, &["Reconnect"]);
                                                if secondary_button_w(ui, "Reconnect", true, bw) {
                                                    do_reconnect = true;
                                                }
                                            });
                                        }
                                    });
                            });
                            ui.add_space(SPACE_4);

                            // ---- Scan block: disabled entirely when the connection is broken ----
                            ui.vertical(|ui| {
                                if broken {
                                    ui.disable();
                                }
                                crate::widgets::island(ui, |ui| {
                                    egui::Frame::new()
                                        .inner_margin(Margin::symmetric(14, 12))
                                        .show(ui, |ui| {
                                            ui.label(
                                                RichText::new("Scan")
                                                    .font(theme::ui_bold_font(13.0))
                                                    .color(p().text),
                                            );
                                            ui.add_space(SPACE_2);
                                            ui.horizontal(|ui| {
                                                let (_, word, col, tip) = scan_state(&st);
                                                ui.label(RichText::new("●").color(col));
                                                ui.label(
                                                    RichText::new(word).color(col).size(11.0),
                                                );
                                                ui.add_space(SPACE_2);
                                                ui.label(RichText::new(tip).color(p().text_dim).size(11.0));
                                            });
                                            if broken {
                                                ui.add_space(2.0);
                                                ui.label(
                                                    RichText::new(
                                                        "disabled — reconnect to manage",
                                                    )
                                                    .color(p().text_dim)
                                                    .size(11.0),
                                                );
                                            }

                            // the live scan error (if any) — counts live in the activity log
                            if let Some(e) = &st.last_error {
                                ui.add_space(4.0);
                                ui.label(
                                    RichText::new(format!("Last error: {e}"))
                                        .color(p().danger)
                                        .size(11.0),
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

                            // ---- monitored schemas: a two-pane transfer picker ----
                            ui.label(RichText::new("Monitored schemas").color(p().text_dim).size(11.0));
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
                            ui.label(RichText::new("Activity log").color(p().text_dim).size(11.0));
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
                            // footer: page actions inside the Scan block (order matches the toolbar:
                            // Enable/Disable · Rescan · Apply). Apply is disabled when the staged
                            // edits match what's saved. Hidden entirely when the connection is
                            // broken (nothing to apply to a dead collector).
                            if !broken {
                                let eff_schemas =
                                    set_schemas.as_ref().unwrap_or(&edit_schemas0);
                                let can_apply = stored.as_ref().is_some_and(|(i, b, d, s)| {
                                    interval != *i || budget != *b || idle != *d || eff_schemas != s
                                });
                                ui.add_space(SPACE_2);
                                ui.horizontal(|ui| {
                                    let bw = uniform_button_width(
                                        ui,
                                        &["Apply", "Disable", "Enable", "Rescan now"],
                                    );
                                    let (label, on) =
                                        if st.paused { ("Enable", true) } else { ("Disable", false) };
                                    if secondary_button_w(ui, label, true, bw) {
                                        do_toggle_enabled = Some(on);
                                    }
                                    ui.add_space(SPACE_2);
                                    if secondary_button_w(ui, "Rescan now", !st.paused, bw) {
                                        do_rescan = true;
                                    }
                                    ui.add_space(SPACE_2);
                                    if secondary_button_w(ui, "Apply", can_apply, bw) {
                                        apply = true;
                                    }
                                });
                            }
                                        }); // Scan island frame
                                }); // Scan island
                            }); // Scan disabled-when-broken wrapper
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
            self.set_collector_enabled(on);
        }
        if do_rescan {
            self.rescan_now();
        }
        if apply {
            self.apply_meta_edits();
        }
        // Reconnect: drop the dead control connection and reconnect with the same credentials. No
        // modal — start_main_connect reuses the captured conn_params.
        if do_reconnect && conn_params.is_some() {
            self.reconnect_now();
        }
    }

    /// Re-establish the control connection with the currently-captured credentials (no dialog).
    /// Used by the Session tab's Reconnect button after a dropped connection.
    fn reconnect_now(&mut self) {
        let Some(p) = self.conn_params.clone() else {
            return;
        };
        // drop whatever's left of the dead connection + its metadata workers, keep the same label
        self.main_conn = None;
        self.stop_meta_actors();
        self.pending_label = self.active_label.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.connect_rx = Some(rx);
        std::thread::spawn(move || {
            let res = match connections::parse_port(&p.port) {
                Ok(port) => {
                    connections::connect_client_probed(&p.host, port, &p.db, &p.user, &p.password)
                }
                Err(e) => Err(e),
            };
            let _ = tx.send(res);
        });
    }

    /// Enable/disable the live collector and persist the choice to the active connection.
    fn set_collector_enabled(&mut self, on: bool) {
        if let Some(h) = &self.collector {
            if on {
                h.resume();
            } else {
                h.pause();
            }
        }
        self.apply_meta_setting(|c| c.meta_enabled = on);
    }

    /// Trigger an immediate rescan on the live collector.
    fn rescan_now(&mut self) {
        if let Some(h) = &self.collector {
            h.rescan();
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
