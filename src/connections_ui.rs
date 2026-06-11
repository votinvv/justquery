//! Connection UI: the Connect / Disconnect / busy / test / conflict dialogs, the Connection
//! Manager dock, the per-connection settings tab, and the connect/disconnect orchestration.
//! The data model, on-disk persistence and the live postgres plumbing stay in
//! [`crate::connections`].

use crate::connections::{
    connect_client, make_tls, name_key, now_ms, parse_port, save, strip_paren_suffix,
    try_connect, Connection, ConnParams,
};
use crate::widgets::{
    close_x, destructive_button_w, focus_field, manager_row, primary_button, primary_button_w,
    qbtn_off_sm, qbtn_sm, secondary_button_w, select_click, show_modal, style_scrollbar,
    styled_combo, subbar, uniform_button_width,
};
use crate::theme::p;
use crate::{ic, theme, PendingConn, JustQueryApp, Tab};
use crate::{CHROME_PAD, SPACE_2, SPACE_3, SPACE_4, SPACE_5, TABBAR_H};
use eframe::egui;
use egui::{Align, Layout, Margin, RichText, Stroke};
use std::time::{Duration, Instant};

impl JustQueryApp {
    /// Open the Connect dialog (or nudge the user to the manager when nothing is saved).
    pub(crate) fn open_connect(&mut self) {
        if self.connections.is_empty() {
            self.no_conn_open = true;
            return;
        }
        self.connect_sel = self.connect_sel.min(self.connections.len() - 1);
        let c = &self.connections[self.connect_sel];
        self.connect_user = c.user.clone();
        self.connect_pass = c.password.clone();
        self.connect_error = None;
        self.connect_open = true;
    }

    /// Modal "Connect" button: guard against in-flight work, then open the real main connection.
    fn do_connect(&mut self) {
        // connecting (which replaces the main connection) while a query runs or a transaction is
        // open would disrupt it — prompt first
        if !self.busy_tabs().is_empty() {
            self.connect_open = false;
            self.busy_prompt = Some(PendingConn::Connect);
            return;
        }
        self.start_main_connect();
    }

    /// Close the dialog and open the connection on a background thread (the UI is blocked by the
    /// "Connecting…" overlay until it resolves).
    fn start_main_connect(&mut self) {
        let Some(c) = self.connections.get(self.connect_sel).cloned() else {
            return;
        };
        let user = self.connect_user.trim().to_string();
        let pass = self.connect_pass.clone();
        self.pending_label = format!("{}@{}", user, c.db);
        // capture the resolved credentials so each tab can open its own session connection
        self.conn_params = Some(ConnParams {
            host: c.host.clone(),
            port: c.port.clone(),
            db: c.db.clone(),
            user: user.clone(),
            password: pass.clone(),
        });
        // a (re)connect may target a different database — drop any existing tab session connections
        // so they re-open lazily with the new credentials (no tabs are running here: the busy guard
        // in do_connect already prompted, or kill_all cleared them)
        for t in &mut self.tabs {
            t.reset_session();
        }
        // a (re)connect may target a different database → drop any running metadata workers/store
        self.stop_meta_actors();
        // capture this connection's metadata settings + id, applied once the connect succeeds
        self.active_conn_id = Some(c.id);
        self.pending_meta_settings = Some(crate::metadata::CollectorSettings {
            enabled: c.meta_enabled,
            interval: c.meta_interval,
            budget: c.meta_budget,
            idle: c.meta_idle,
            schemas: c.meta_schemas.clone(),
        });
        self.edit_interval = c.meta_interval;
        self.edit_budget = c.meta_budget;
        self.edit_idle = c.meta_idle;
        self.edit_schemas = c.meta_schemas.clone();
        self.connect_error = None;
        // keep the Connect dialog open and show a spinner inside it; success closes it, a failure
        // leaves it open with the error message
        self.connect_open = true;
        let (tx, rx) = std::sync::mpsc::channel();
        self.connect_rx = Some(rx);
        std::thread::spawn(move || {
            let res = match parse_port(&c.port) {
                Ok(p) => connect_client(&c.host, p, &c.db, &user, &pass),
                Err(e) => Err(e),
            };
            let _ = tx.send(res);
        });
    }

    /// Plug toggle / menu "Disconnect": ONE confirmation, not two — busy tabs go straight to the
    /// "Work in progress → Kill & disconnect" prompt (it is itself a confirmation), an idle
    /// session to the plain Disconnect confirm.
    pub(crate) fn request_disconnect(&mut self) {
        if self.busy_tabs().is_empty() {
            self.disconnect_confirm = true;
        } else {
            self.busy_prompt = Some(PendingConn::Disconnect);
        }
    }

    /// Confirmed "Disconnect": re-check the in-flight-work guard, then drop the connection.
    pub(crate) fn do_disconnect(&mut self) {
        if !self.busy_tabs().is_empty() {
            self.busy_prompt = Some(PendingConn::Disconnect);
            return;
        }
        self.disconnect_now();
    }

    /// Send an out-of-band cancel for the running query (a fresh PostgreSQL CancelRequest). The
    /// worker thread still returns the moved client via `ExecMsg::Done`, so the main connection is
    /// preserved — only the in-flight statement is aborted.
    pub(crate) fn cancel_running_query(&mut self) {
        let cancel = self.cur_mut().and_then(|t| t.exec_cancel.take());
        if let Some(cancel) = cancel {
            if let Ok(tls) = make_tls() {
                std::thread::spawn(move || {
                    let _ = cancel.cancel_query(tls);
                });
            }
        }
    }

    /// Drop the control connection and every tab's session connection, clearing all connected
    /// state (including the header label).
    fn disconnect_now(&mut self) {
        self.main_conn = None; // dropping the client closes the control connection
        self.connected = false;
        self.conn_broken = false; // deliberate disconnect → the chip shows nothing, not red
        self.conn_params = None;
        self.active_label.clear();
        // drop every tab's session connection and abandon any in-flight query
        for t in &mut self.tabs {
            t.reset_session();
        }
        self.stop_meta_actors();
        self.active_conn_id = None;
    }

    /// Tabs holding the connection busy — a running query or an open transaction — as
    /// (tab title, reason). Empty until real query execution sets these flags.
    fn busy_tabs(&self) -> Vec<(String, &'static str)> {
        self.tabs
            .iter()
            .filter_map(|t| {
                if t.running {
                    Some((t.title.clone(), "running query"))
                } else if t.tx_open {
                    Some((t.title.clone(), "uncommitted transaction"))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Kill in-flight work so a connect/disconnect can proceed: abandon every tab's running query
    /// (dropping the receiver ends its worker) and drop the session connections.
    fn kill_all(&mut self) {
        for t in &mut self.tabs {
            t.reset_session();
        }
    }

    /// Connect dialog: pick a saved connection, override login/password, connect.
    pub(crate) fn connect_modal(&mut self, ctx: &egui::Context) {
        if !self.connect_open {
            return;
        }
        let mut connect_now = false;
        let r = show_modal(ctx, "connect", 280.0, |ui| {
            // ---- title row: heading + close × ----
            ui.horizontal(|ui| {
                ui.label(RichText::new("Connect").size(16.0).strong().color(p().text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_x(ui, "Close") {
                        self.connect_open = false;
                    }
                });
            });
            ui.add_space(SPACE_4);

            let connecting = self.connect_rx.is_some();
            // every field/combo shares one width and one left edge
            let w = ui.available_width();

            let failed = self.connect_error.as_deref().is_some_and(|s| !s.is_empty());
            // after a failed attempt the credential fields carry a danger ring until edited
            let danger_ring = |ui: &mut egui::Ui, r: &egui::Response| {
                if failed && !r.has_focus() {
                    crate::widgets::crisp_border_r(
                        ui.painter(),
                        r.rect,
                        p().danger,
                        crate::RADIUS_CONTROL,
                    );
                }
            };

            let mut picked = None;
            crate::widgets::form_row(ui, "Connection", |ui| {
                let names: Vec<String> = self.connections.iter().map(|c| c.name.clone()).collect();
                picked = styled_combo(
                    ui, "conn_pick", w, 13.0, !names.is_empty(), Some(self.connect_sel), &names,
                );
            });
            if let Some(i) = picked {
                let prev = self.connect_sel;
                self.connect_sel = i;
                // switching the picked connection always reloads its saved login/password
                if i != prev {
                    if let Some(c) = self.connections.get(i) {
                        self.connect_user = c.user.clone();
                        self.connect_pass = c.password.clone();
                    }
                }
            }
            crate::widgets::form_row(ui, "Login", |ui| {
                let r = focus_field(ui, &mut self.connect_user, false, w);
                danger_ring(ui, &r);
            });
            crate::widgets::form_row(ui, "Password", |ui| {
                let r = focus_field(ui, &mut self.connect_pass, true, w);
                danger_ring(ui, &r);
            });

            // ---- button bar with the footer-error pattern (Design Delta v2.1 §5): the error is
            // ONE Small/danger line on the LEFT of the button row, ellipsized with the full text
            // on hover — the modal's height never changes when it appears or goes away.
            ui.add_space(SPACE_5 - 14.0); // form_row already left 14px after the last row
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let bw = uniform_button_width(ui, &["Connect", "Connecting…", "Cancel"]);
                let connect_label = if connecting { "Connecting…" } else { "Connect" };
                if primary_button_w(ui, connect_label, !connecting, bw) {
                    connect_now = true;
                }
                ui.add_space(SPACE_2);
                if secondary_button_w(ui, "Cancel", !connecting, bw) {
                    self.connect_open = false;
                }
                if let Some(err) = self.connect_error.clone().filter(|s| !s.is_empty()) {
                    ui.add_space(SPACE_2);
                    let avail = ui.available_width();
                    let one_line = err.replace(['\r', '\n'], " ");
                    let shown = crate::widgets::truncate_to_width(ui, &one_line, 11.0, avail);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::Label::new(
                            RichText::new(shown).color(p().danger).size(11.0),
                        ))
                        .on_hover_text(&err);
                    });
                }
            });
        });
        // modal key contract: Enter = Connect, Esc = Cancel
        if (connect_now || r.enter) && self.connect_rx.is_none() {
            self.do_connect();
        }
        if r.escape {
            self.connect_open = false;
        }
    }

    /// Confirm-disconnect modal (the plug toggle never disconnects silently). Destructive
    /// primary per Design Delta v2.1 §5; Enter = Disconnect, Esc = Cancel.
    pub(crate) fn disconnect_modal(&mut self, ctx: &egui::Context) {
        if !self.disconnect_confirm {
            return;
        }
        let identity = self
            .conn_params
            .as_ref()
            .map(|cp| format!("{}@{}", cp.user, cp.host))
            .unwrap_or_else(|| self.active_label.clone());
        let mut go = false;
        let r = show_modal(ctx, "disconnect", 320.0, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Disconnect").size(16.0).strong().color(p().text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_x(ui, "Close") {
                        self.disconnect_confirm = false;
                    }
                });
            });
            ui.add_space(SPACE_3);
            ui.label(RichText::new(format!("Disconnect from {identity}?")).color(p().text_dim));
            ui.add_space(SPACE_5);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let bw = uniform_button_width(ui, &["Disconnect", "Cancel"]);
                if destructive_button_w(ui, "Disconnect", true, bw) {
                    go = true;
                }
                ui.add_space(SPACE_2);
                if secondary_button_w(ui, "Cancel", true, bw) {
                    self.disconnect_confirm = false;
                }
            });
        });
        if go || r.enter {
            self.disconnect_confirm = false;
            self.do_disconnect();
        }
        if r.escape {
            self.disconnect_confirm = false;
        }
    }

    /// "No saved connections" prompt → points the user at the manager.
    pub(crate) fn no_conn_modal(&mut self, ctx: &egui::Context) {
        if !self.no_conn_open {
            return;
        }
        let r = show_modal(ctx, "noconn", 300.0, |ui| {
            ui.label(RichText::new("No connections yet").size(15.0).strong().color(p().text));
            ui.add_space(10.0);
            ui.label(RichText::new("Create one in the Connection Manager first.").color(p().text_dim));
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let bw = uniform_button_width(ui, &["Open Manager", "Close"]);
                if primary_button_w(ui, "Open Manager", true, bw) {
                    self.no_conn_open = false;
                    self.left_panel = Some(crate::LeftPanel::Database);
                }
                ui.add_space(SPACE_2);
                if secondary_button_w(ui, "Close", true, bw) {
                    self.no_conn_open = false;
                }
            });
        });
        if r.enter {
            // modal key contract: Enter presses the primary action
            self.no_conn_open = false;
            self.left_panel = Some(crate::LeftPanel::Database);
        }
        if r.escape {
            self.no_conn_open = false;
        }
    }

    /// The Connection Manager side panel (toggled from the toolbar): connection list + New /
    /// Delete. Single click selects (Ctrl/Shift multi-select); double-click opens a settings tab.
    pub(crate) fn database_manager_panel(&mut self, ui: &mut egui::Ui) {
        if self.left_panel != Some(crate::LeftPanel::Database) {
            return;
        }
        let mut add = false;
        let mut do_delete = false; // toolbar trash → delete the selected connection(s)
        let mut commit_rename = false;
        let mut cancel_rename = false;
        let mut close_panel = false; // header × closes the dock
        let mut open_cid: Option<u64> = None; // connection to open on a double-click
        // Blank the resize line for this panel's ui (see widgets::hush_resize_line). The dock width
        // is shared by both managers via the common panel id "left_panel".
        let saved_style = crate::widgets::hush_resize_line(ui);
        egui::Panel::left("left_panel")
            .resizable(true)
            .default_size(220.0)
            // min width fits the "Connection Manager" / "Metadata Manager" header + the × so the
            // title is never clipped (we forbid narrowing instead of truncating)
            .size_range(196.0..=460.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin::ZERO))
            .show_inside(ui, |ui| {
                ui.style_mut().visuals.override_text_color = None;
                // header: same height as the tab bar, with a close × on the right
                egui::Panel::top("dbmgr_header")
                    .exact_size(TABBAR_H)
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: 10,
                        right: 6,
                        // shift the header down so its text lines up with the tab labels (which
                        // sit CHROME_PAD/2 lower because their box is inset at the top)
                        top: CHROME_PAD as i8,
                        bottom: 0,
                    }))
                    .show_inside(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            // the dock can't be narrowed past this title (size_range below), so the
                            // label always fits — no truncation needed
                            ui.label(RichText::new("Connection Manager").size(13.0).strong().color(p().text));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if close_x(ui, "Close panel") {
                                    close_panel = true;
                                }
                            });
                        });
                    });
                // work-area toolbar — a chrome strip under the header, holding New "+" and
                // Delete (when rows are selected)
                subbar(ui, "dbmgr_toolbar", |ui| {
                    if qbtn_sm(ui, ic::PLUS, p().text, "New connection").clicked() {
                        add = true;
                    }
                    if self.conn_sel.is_empty() {
                        qbtn_off_sm(ui, ic::DELETE, "Delete (select a connection)");
                    } else if qbtn_sm(ui, ic::DELETE, p().text, "Delete selected").clicked() {
                        do_delete = true;
                    }
                });
                // list in a white island — only a left beige strip, so its top edge lines up
                // with the editor sheet (both sit directly under the chrome rows)
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: 6, // 6px left border (screen edge); the editor's 6px left is the seam
                        right: 6, // match the toolbar/header right edge (no overhang)
                        top: 1, // 1px gap matching the editor sheet so their top borders line up
                        bottom: 0,
                    }))
                    .show_inside(ui, |ui| {
                        let ids: Vec<u64> = self.connections.iter().map(|c| c.id).collect();
                        let (ctrl, shift) =
                            ui.input(|i| (i.modifiers.ctrl, i.modifiers.shift));
                        // white work-area island (connection list), vertically scrollable
                        let island = egui::Frame::new()
                            .fill(p().ivory)
                            .corner_radius(egui::CornerRadius::same(crate::RADIUS_ISLAND))
                            .shadow(crate::theme::island_shadow())
                            .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            // rows fill to the very frame (no 1px inset gap) — the border is redrawn
                            // ON TOP after the list, so edge-row fills never leave a corner gap
                            let clip = ui.max_rect();
                            ui.set_clip_rect(clip);
                            style_scrollbar(ui);
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 0.0; // tight rows — no gap between connections
                            let conns: Vec<(u64, String)> = self
                                .connections
                                .iter()
                                .map(|c| (c.id, c.name.clone()))
                                .collect();
                            if conns.is_empty() {
                                ui.add_space(6.0);
                                ui.colored_label(p().text_dim, "  No connections.\n  Click + to add.");
                            }
                            for (i, (cid, n)) in conns.iter().enumerate() {
                                let renaming = self.dbmgr_rename == Some(*cid);
                                let selected = self.conn_sel.contains(cid);
                                let label = if renaming {
                                    "" // the inline editor covers the label while renaming
                                } else if n.is_empty() {
                                    "(unnamed)"
                                } else {
                                    n.as_str()
                                };
                                // shared manager row (icon + name); selected/renaming → tint
                                let resp =
                                    manager_row(ui, 0.0, ic::CONNECT, label, selected || renaming);
                                let rect = resp.rect;
                                if renaming {
                                    // inline name editor over the row, drawn in a NON-allocating
                                    // child ui (`new_child`) so it never moves the parent cursor —
                                    // the row keeps its fixed height and the rows below don't jump
                                    // while editing. Bordered field with the accent focus ring.
                                    let edit_rect = egui::Rect::from_min_max(
                                        egui::pos2(rect.left() + 28.0, rect.top() + 1.0),
                                        egui::pos2(rect.right() - 4.0, rect.bottom() - 1.0),
                                    );
                                    let mut fui = ui.new_child(
                                        egui::UiBuilder::new()
                                            .max_rect(edit_rect)
                                            .layout(Layout::left_to_right(Align::Center)),
                                    );
                                    fui.visuals_mut().extreme_bg_color = p().field_bg;
                                    fui.visuals_mut().selection.stroke =
                                        Stroke::new(2.0, p().accent);
                                    {
                                        let w = &mut fui.visuals_mut().widgets;
                                        w.inactive.expansion = 0.0;
                                        w.hovered.expansion = 0.0;
                                        w.active.expansion = 0.0;
                                        w.inactive.bg_stroke = Stroke::new(1.0, p().border_strong);
                                        w.hovered.bg_stroke = Stroke::new(1.0, p().border_strong);
                                    }
                                    let r = fui.add(
                                        egui::TextEdit::singleline(&mut self.dbmgr_rename_buf)
                                            .margin(egui::Margin::symmetric(5, 2))
                                            .desired_width(f32::INFINITY)
                                            .text_color(p().text)
                                            .font(egui::FontId::proportional(13.0)),
                                    );
                                    if self.dbmgr_rename_focus {
                                        r.request_focus();
                                        self.dbmgr_rename_focus = false;
                                        // Windows convention: the prefilled name starts fully
                                        // selected so typing REPLACES it instead of appending
                                        // (matches Explorer's create/F2 rename behaviour)
                                        if let Some(mut st) =
                                            egui::TextEdit::load_state(ui.ctx(), r.id)
                                        {
                                            let end = egui::text::CCursor::new(
                                                self.dbmgr_rename_buf.chars().count(),
                                            );
                                            st.cursor.set_char_range(Some(
                                                egui::text::CCursorRange::two(
                                                    egui::text::CCursor::new(0),
                                                    end,
                                                ),
                                            ));
                                            st.store(ui.ctx(), r.id);
                                        }
                                    }
                                    // resolve only while no conflict prompt is open
                                    if self.dbmgr_conflict.is_none() {
                                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                            cancel_rename = true;
                                        } else if r.lost_focus() {
                                            commit_rename = true;
                                        }
                                    }
                                } else {
                                    // Double-click opens the settings tab. A plain click selects on
                                    // PRESS (so the previous row's accent drops instantly, not after
                                    // the whole button-hold). A plain click on the already-sole-
                                    // selected row arms a rename on release (Windows-style — fires
                                    // after the double-click window so a real double-click opens).
                                    let new_press = resp.is_pointer_button_down_on()
                                        && self.conn_pressed.is_none_or(|(c, _)| c != *cid);
                                    if resp.double_clicked() {
                                        open_cid = Some(*cid);
                                        self.conn_rename_armed = None;
                                        self.conn_pressed = None;
                                    } else if new_press && !ctrl && !shift {
                                        let was_sole = self.conn_sel.as_slice() == [*cid];
                                        select_click(
                                            &mut self.conn_sel,
                                            &mut self.conn_anchor,
                                            &ids,
                                            i,
                                            false,
                                            false,
                                        );
                                        self.conn_pressed = Some((*cid, was_sole));
                                        ui.ctx().request_repaint();
                                    } else if resp.clicked() {
                                        if ctrl || shift {
                                            select_click(
                                                &mut self.conn_sel,
                                                &mut self.conn_anchor,
                                                &ids,
                                                i,
                                                ctrl,
                                                shift,
                                            );
                                            self.conn_rename_armed = None;
                                        } else if let Some((c, was_sole)) = self.conn_pressed.take() {
                                            // plain click released: arm rename only if it began on
                                            // the already-sole-selected row
                                            self.conn_rename_armed = if c == *cid && was_sole {
                                                Some((*cid, Instant::now()))
                                            } else {
                                                None
                                            };
                                        }
                                        ui.ctx().request_repaint();
                                    }
                                }
                            }
                                });
                        });
                        // crisp 1px frame on top of the list, so selection/hover row fills can run
                        // edge-to-edge under it without a gap (matches the combo dropdown)
                        crate::widgets::crisp_border(ui.painter(), island.response.rect, p().border_strong);
                    });
            });
        ui.set_style(saved_style);
        // forget the held row once the button is up (so a press that ended off-row doesn't block
        // the next press on the same row)
        if !ui.input(|i| i.pointer.primary_down()) {
            self.conn_pressed = None;
        }
        if close_panel {
            self.left_panel = None;
        }
        if do_delete && !self.conn_sel.is_empty() {
            self.confirm = Some(crate::ConfirmAction::DeleteConnections(self.conn_sel.clone()));
        }
        // F2 renames the single selected connection
        if self.dbmgr_rename.is_none()
            && self.conn_sel.len() == 1
            && ui.input(|i| i.key_pressed(egui::Key::F2))
        {
            let id = self.conn_sel[0];
            self.start_conn_rename(id);
        }
        // armed rename fires once the double-click window has passed with no double-click
        if let Some((id, t)) = self.conn_rename_armed {
            if self.dbmgr_rename.is_some() {
                self.conn_rename_armed = None;
            } else if t.elapsed() >= Duration::from_millis(450) {
                self.conn_rename_armed = None;
                self.start_conn_rename(id);
            } else {
                // keep rendering until the window elapses so the rename fires on its own — a single
                // request_repaint_after didn't reliably wake egui from idle (it waited for input)
                ui.ctx().request_repaint();
            }
        }
        if add {
            // add a free-named connection straight to the list and start inline rename (no tab)
            let name = self.next_new_conn_name();
            let id = self.connections.iter().map(|c| c.id).max().unwrap_or(0) + 1;
            self.connections.push(Connection {
                id,
                name: name.clone(),
                port: "5432".into(),
                ..Default::default()
            });
            save(&self.connections);
            self.dbmgr_rename = Some(id);
            self.dbmgr_rename_buf = name;
            self.dbmgr_rename_focus = true;
        }
        if cancel_rename {
            self.dbmgr_rename = None; // keep the connection's current name
        }
        if commit_rename {
            self.finalize_rename();
        }
        // single click opens the connection's settings tab immediately
        if let Some(cid) = open_cid {
            if let Some(c) = self.connections.iter().find(|c| c.id == cid).cloned() {
                self.open_conn_tab(c);
            }
        }
    }

    /// Smallest free "New connection N" name (case-insensitive).
    fn next_new_conn_name(&self) -> String {
        let mut x = 1u32;
        loop {
            let cand = format!("New connection {x}");
            let key = name_key(&cand);
            if !self.connections.iter().any(|c| name_key(&c.name) == key) {
                return cand;
            }
            x += 1;
        }
    }

    /// Windows-style free variant of `name`: "foo" → "foo (2)" → "foo (3)" … (excluding `exclude_id`).
    fn free_variant(&self, name: &str, exclude_id: u64) -> String {
        let stem = strip_paren_suffix(name);
        let mut m = 2u32;
        loop {
            let cand = format!("{stem} ({m})");
            let key = name_key(&cand);
            if !self
                .connections
                .iter()
                .any(|c| c.id != exclude_id && name_key(&c.name) == key)
            {
                return cand;
            }
            m += 1;
        }
    }

    /// Begin inline-renaming the given connection (loads its name into the edit buffer + focuses).
    fn start_conn_rename(&mut self, id: u64) {
        if let Some(c) = self.connections.iter().find(|c| c.id == id) {
            self.dbmgr_rename = Some(id);
            self.dbmgr_rename_buf = c.name.clone();
            self.dbmgr_rename_focus = true;
            self.conn_rename_armed = None;
        }
    }

    /// Finish inline rename: empty → revert; duplicate → open the conflict prompt; unique → save.
    fn finalize_rename(&mut self) {
        let Some(id) = self.dbmgr_rename else {
            return;
        };
        let name = self.dbmgr_rename_buf.trim().to_string();
        if name.is_empty() {
            self.dbmgr_rename = None; // keep the existing (auto) name
            return;
        }
        let key = name_key(&name);
        let dup = self
            .connections
            .iter()
            .any(|c| c.id != id && name_key(&c.name) == key);
        if dup {
            let suggestion = self.free_variant(&name, id);
            self.dbmgr_conflict = Some((id, suggestion)); // stay in rename; prompt resolves it
            return;
        }
        if let Some(c) = self.connections.iter_mut().find(|c| c.id == id) {
            c.name = name.clone();
        }
        self.apply_rename_to_tabs(id, &name);
        save(&self.connections);
        self.dbmgr_rename = None;
    }

    /// Reflect a connection rename in any open settings tab (its Name field + the tab title).
    fn apply_rename_to_tabs(&mut self, id: u64, name: &str) {
        for t in &mut self.tabs {
            if let Some(c) = t.conn.as_mut() {
                if c.id == id {
                    c.name = name.to_owned();
                    t.title = name.to_owned();
                }
            }
        }
    }

    /// Duplicate-name prompt during inline rename (Windows-style "(2)" suggestion).
    pub(crate) fn conflict_modal(&mut self, ctx: &egui::Context) {
        let Some((id, suggestion)) = self.dbmgr_conflict.clone() else {
            return;
        };
        let taken = self.dbmgr_rename_buf.trim().to_string();
        let mut do_rename = false;
        let mut keep_editing = false;
        let r = show_modal(ctx, "conflict", 330.0, |ui| {
            ui.label(RichText::new("Name already in use").size(15.0).strong().color(p().text));
            ui.add_space(10.0);
            ui.label(
                RichText::new(format!(
                    "A connection named \"{taken}\" already exists. Rename it to \"{suggestion}\"?"
                ))
                .color(p().text_dim),
            );
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let bw = uniform_button_width(ui, &["Rename", "Keep editing"]);
                if primary_button_w(ui, "Rename", true, bw) {
                    do_rename = true;
                }
                ui.add_space(SPACE_2);
                if secondary_button_w(ui, "Keep editing", true, bw) {
                    keep_editing = true;
                }
            });
        });
        if r.enter {
            do_rename = true; // modal key contract: Enter presses the primary action
        }
        if r.escape {
            keep_editing = true;
        }
        if do_rename {
            if self.dbmgr_rename == Some(id) {
                // inline rename in the manager list
                if let Some(c) = self.connections.iter_mut().find(|c| c.id == id) {
                    c.name = suggestion.clone();
                }
                self.apply_rename_to_tabs(id, &suggestion);
                save(&self.connections);
                self.dbmgr_rename = None;
            } else {
                // rename came from the settings-tab Save — take the name and commit all fields
                if let Some(t) = self.cur_mut() {
                    if let Some(c) = t.conn.as_mut() {
                        c.name = suggestion.clone();
                    }
                }
                self.commit_conn_tab();
                save(&self.connections);
            }
            self.dbmgr_conflict = None;
        }
        if keep_editing {
            self.dbmgr_conflict = None;
            self.dbmgr_rename_focus = true; // back to editing the field
        }
    }

    /// Open (or focus) an editor tab that edits the given connection.
    pub(crate) fn open_conn_tab(&mut self, conn: Connection) {
        if conn.id != 0 {
            if let Some(i) = self
                .tabs
                .iter()
                .position(|t| t.conn.as_ref().is_some_and(|c| c.id == conn.id))
            {
                self.active_tab = i;
                self.focus_editor = true;
                return;
            }
        }
        let is_new = conn.id == 0;
        let title = if conn.name.is_empty() {
            "New connection".to_owned()
        } else {
            conn.name.clone()
        };
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id, title);
        tab.conn = Some(conn);
        tab.dirty = is_new; // a brand-new connection is unsaved
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        self.focus_editor = true;
    }

    /// Save the active connection tab: validate (name required + unique), commit, persist to disk.
    /// One connection == one file named after it, so duplicate names are rejected here.
    pub(crate) fn save_conn_tab(&mut self) {
        let idx = self.active_tab;
        let Some(conn) = self.tabs.get(idx).and_then(|t| t.conn.clone()) else {
            return;
        };
        let name = conn.name.trim().to_string();
        if name.is_empty() {
            self.error_modal = Some("Connection name is required.".to_owned());
            return;
        }
        if conn.host.trim().is_empty() || conn.port.trim().is_empty() || conn.db.trim().is_empty() {
            self.error_modal = Some("Host, port and database are required.".to_owned());
            return;
        }
        let duplicate = self
            .connections
            .iter()
            .any(|c| c.id != conn.id && name_key(&c.name) == name_key(&name));
        if duplicate {
            // same prompt as the inline rename: offer a free "(2)" variant
            let suggestion = self.free_variant(&name, conn.id);
            self.dbmgr_rename_buf = name.clone(); // shown as the "taken" name in the prompt
            self.dbmgr_conflict = Some((conn.id, suggestion));
            return;
        }
        self.commit_conn_tab();
        save(&self.connections);
    }

    /// In-memory part of saving a connection tab (no disk) — assign an id for a new connection,
    /// update the saved list, retitle the tab and clear its dirty flag.
    pub(crate) fn commit_conn_tab(&mut self) {
        let idx = self.active_tab;
        let Some(mut conn) = self.tabs.get(idx).and_then(|t| t.conn.clone()) else {
            return;
        };
        if let Some(existing) = self
            .connections
            .iter_mut()
            .find(|c| c.id != 0 && c.id == conn.id)
        {
            *existing = conn.clone();
        } else {
            conn.id = self.connections.iter().map(|c| c.id).max().unwrap_or(0) + 1;
            // stamp creation order so it sorts after existing connections (and persists)
            if conn.created == 0 {
                conn.created = now_ms().max(
                    self.connections.iter().map(|c| c.created).max().unwrap_or(0) + 1,
                );
            }
            self.connections.push(conn.clone());
        }
        let title = if conn.name.is_empty() {
            "Connection".to_owned()
        } else {
            conn.name.clone()
        };
        if let Some(t) = self.tabs.get_mut(idx) {
            t.conn = Some(conn);
            t.title = title;
            t.dirty = false;
        }
    }

    /// Kick off a real Test Connection for the given tab on a background thread.
    pub(crate) fn start_conn_test(&mut self, idx: usize) {
        let Some(c) = self.tabs.get(idx).and_then(|t| t.conn.clone()) else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.test_rx = Some(rx);
        self.test_result = None;
        std::thread::spawn(move || {
            let res = match parse_port(&c.port) {
                Ok(p) => try_connect(&c.host, p, &c.db, &c.user, &c.password),
                Err(e) => Err(e),
            };
            let _ = tx.send(res);
        });
    }

    /// Delete a connection: drop it from the list, prune its file, and close any open settings tab.
    pub(crate) fn delete_connection(&mut self, id: u64) {
        self.connections.retain(|c| c.id != id);
        save(&self.connections); // rewrites the dir and prunes the now-orphaned file
        let idxs: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.conn.as_ref().is_some_and(|c| c.id == id))
            .map(|(i, _)| i)
            .collect();
        for i in idxs.into_iter().rev() {
            self.close_tab(i);
        }
    }

    /// One Test-Connection modal: a spinner + disabled OK while the test runs in the background;
    /// when it finishes the result fills the (fixed-height) status area in place and OK becomes
    /// active — the modal never rebuilds or resizes. × cancels; OK/Enter/Esc dismiss once done.
    pub(crate) fn conn_test_modal(&mut self, ctx: &egui::Context) {
        let testing = self.test_rx.is_some();
        let res = self.test_result.clone();
        if !testing && res.is_none() {
            return;
        }
        let mut close = false;
        let r = show_modal(ctx, "test", 400.0, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Test connection").size(16.0).strong().color(p().text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_x(ui, "Close") {
                        close = true;
                    }
                });
            });
            ui.add_space(SPACE_4);
            // fixed-height status area → no resize between the spinner and the result; the result
            // message wraps in place (no truncation, so no hover-tooltip — that was doubling up)
            let (rect, _) =
                ui.allocate_exact_size(egui::Vec2::new(ui.available_width(), 80.0), egui::Sense::hover());
            let mut sui = ui.new_child(
                egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)),
            );
            {
                let ui = &mut sui;
                match &res {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.add_space(8.0);
                            ui.label(RichText::new("Testing connection…").color(p().text_dim));
                        });
                    }
                    Some(Ok(msg)) => {
                        ui.label(RichText::new("Connection successful").strong().color(p().ok));
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(RichText::new(msg).color(p().text_dim).size(12.0)).wrap());
                    }
                    Some(Err(msg)) => {
                        ui.label(RichText::new("Connection failed").strong().color(p().danger));
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(RichText::new(msg).color(p().text_dim).size(12.0)).wrap());
                    }
                }
            }
            ui.add_space(SPACE_5);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let bw = uniform_button_width(ui, &["OK"]);
                if primary_button_w(ui, "OK", res.is_some(), bw) {
                    close = true;
                }
            });
        });
        if testing {
            // ~10 Hz poll while the test runs (a bare request_repaint would pin max FPS)
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // × cancels at any time; OK / Enter / Esc dismiss once the result is in
        if close || (res.is_some() && (r.enter || r.escape)) {
            self.test_rx = None;
            self.test_result = None;
        }
    }

    /// Generic operation-error modal (errors go here instead of the status bar).
    pub(crate) fn error_modal_box(&mut self, ctx: &egui::Context) {
        let Some(msg) = self.error_modal.clone() else {
            return;
        };
        let mut close = false;
        let r = show_modal(ctx, "err", 360.0, |ui| {
            ui.label(RichText::new("Error").size(15.0).strong().color(p().danger));
            ui.add_space(8.0);
            ui.label(RichText::new(msg).color(p().text));
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if primary_button(ui, "OK", true) {
                    close = true;
                }
            });
        });
        if r.escape || r.enter {
            close = true; // single-button modal: Enter and Esc both dismiss
        }
        if close {
            self.error_modal = None;
        }
    }

    /// "Work in progress" prompt: shown when a connect/disconnect would interrupt a running query
    /// or an open transaction. Lists the affected tabs and offers Go back or Kill & proceed.
    pub(crate) fn busy_modal(&mut self, ctx: &egui::Context) {
        let Some(action) = self.busy_prompt else {
            return;
        };
        let busy = self.busy_tabs();
        if busy.is_empty() {
            self.busy_prompt = None; // nothing busy anymore — nothing to prompt about
            return;
        }
        let verb = match action {
            PendingConn::Connect => "connect",
            PendingConn::Disconnect => "disconnect",
        };
        let mut go_back = false;
        let mut kill = false;
        let r = show_modal(ctx, "busy", 380.0, |ui| {
            ui.label(RichText::new("Work in progress").size(15.0).strong().color(p().text));
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("Some tabs are still busy — {verb}ing will interrupt them:"))
                    .color(p().text_dim),
            );
            ui.add_space(8.0);
            for (name, reason) in &busy {
                ui.label(RichText::new(format!("•  {name} — {reason}")).color(p().text));
            }
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let kill_label = format!("Kill & {verb}");
                let bw = uniform_button_width(ui, &[kill_label.as_str(), "Go back"]);
                if primary_button_w(ui, &kill_label, true, bw) {
                    kill = true;
                }
                ui.add_space(SPACE_2);
                if secondary_button_w(ui, "Go back", true, bw) {
                    go_back = true;
                }
            });
        });
        if r.enter {
            kill = true; // modal key contract: Enter presses the primary action
        }
        if r.escape {
            go_back = true;
        }
        if go_back {
            self.busy_prompt = None;
        } else if kill {
            self.busy_prompt = None;
            self.kill_all();
            match action {
                PendingConn::Connect => self.start_main_connect(),
                PendingConn::Disconnect => self.disconnect_now(),
            }
        }
    }

    /// Blocking "Connecting…" overlay while the main connection is being opened on the bg thread.
    pub(crate) fn connecting_modal(&mut self, ctx: &egui::Context) {
        // while the Connect dialog is open the spinner lives inside it; this standalone overlay is
        // only for connects started without the dialog (e.g. a busy-prompt "Kill & connect")
        if self.connect_rx.is_none() || self.connect_open {
            return;
        }
        show_modal(ctx, "connecting", 240.0, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.add_space(8.0);
                ui.label(RichText::new("Connecting…").color(p().text));
            });
        });
        // ~10 Hz poll while the connect runs (a bare request_repaint would pin max FPS)
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// Render the connection-settings form for the active tab: label/field rows on the data sheet.
    pub(crate) fn connection_tab(&mut self, ui: &mut egui::Ui) {
        let idx = self.active_tab.min(self.tabs.len().saturating_sub(1));
        // Save is available whenever the required fields (Name + host/port/db) are filled — so an
        // opened connection can be re-saved without a throwaway edit (re-saving is idempotent).
        // Otherwise there is "nothing to save" and the Save icon is inactive.
        let can_save = self.tabs.get(idx).and_then(|t| t.conn.as_ref()).is_some_and(|c| {
            !c.name.trim().is_empty()
                && !c.host.trim().is_empty()
                && !c.port.trim().is_empty()
                && !c.db.trim().is_empty()
        });
        let testing = self.test_rx.is_some();
        let mut changed = false;
        let mut test = false;
        let mut do_save = false;

        // Page actions live in a subbar like the editor's (the no-buttons-on-tab-bodies contract):
        // Save + Test connection as icons, Save inactive when there's nothing to save.
        subbar(ui, "conn_toolbar", |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if can_save {
                if qbtn_sm(ui, ic::SAVE, p().text, "Save connection").clicked() {
                    do_save = true;
                }
            } else {
                qbtn_off_sm(ui, ic::SAVE, "Save (fill Name, Host, Port and Database)");
            }
            if testing {
                qbtn_off_sm(ui, ic::CONNECT, "Testing connection…");
            } else if qbtn_sm(ui, ic::CONNECT, p().text, "Test connection").clicked() {
                test = true;
            }
        });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(self.island_margin()))
            .show_inside(ui, |ui| {
                // silvery data sheet inside the side borders, with a thin border of its own
                let sheet = ui.max_rect();
                crate::widgets::island_shadow_under(ui.painter(), sheet);
                crate::widgets::island_box(ui.painter(), sheet, p().data_bg, crate::RADIUS_ISLAND);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(18, 16))
                        .show(ui, |ui| {
                            theme::style_modal_widgets(ui); // fields use the shared border
                            if let Some(c) = self.tabs.get_mut(idx).and_then(|t| t.conn.as_mut()) {
                                egui::Grid::new("conn_form")
                                    .num_columns(2)
                                    .spacing([12.0, 8.0])
                                    .min_col_width(64.0)
                                    .show(ui, |ui| {
                                        let mut row =
                                            |label: &str, v: &mut String, pw: bool, editable: bool| {
                                                ui.label(
                                                    RichText::new(label).color(p().text_dim).size(12.0),
                                                );
                                                let mut te = egui::TextEdit::singleline(v)
                                                    .desired_width(280.0);
                                                if pw {
                                                    te = te.password(true);
                                                }
                                                if !editable {
                                                    // dim it so it visibly reads as "locked / not editable"
                                                    te = te.interactive(false).text_color(p().text_dim);
                                                }
                                                if ui.add(te).changed() {
                                                    changed = true;
                                                }
                                                ui.end_row();
                                            };
                                        // Name is editable; Save validates uniqueness (duplicate
                                        // → conflict prompt) and renames the backing file
                                        row("Name", &mut c.name, false, true);
                                        row("Host", &mut c.host, false, true);
                                        row("Port", &mut c.port, false, true);
                                        row("Database", &mut c.db, false, true);
                                        row("User", &mut c.user, false, true);
                                        row("Password", &mut c.password, true, true);
                                    });
                            }
                        });
                });
                if changed {
                    if let Some(t) = self.tabs.get_mut(idx) {
                        t.dirty = true;
                    }
                }
            });

        if test {
            self.start_conn_test(idx);
        }
        if do_save {
            self.save_conn_tab();
        }
    }
}
