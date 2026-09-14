//! Connection UI: the Connect (Execute on a connection page) / Disconnect / busy / test /
//! conflict dialogs, the Connection Manager dock, the per-connection settings tab, and the
//! connect/disconnect orchestration. The data model, on-disk persistence and the live postgres
//! plumbing stay in [`crate::connections`].

use crate::connections::{
    conn_to_text, connect_client_probed, name_key, now_ms, parse_port, safe_name, save,
    spawn_cancel, strip_paren_suffix, try_connect, ConnParams, Connection,
};
use crate::theme::p;
use crate::widgets::{
    close_x, destructive_button_w, empty_hint, manager_row_fg, modal_header, primary_button,
    primary_button_w, qbtn_off_sm, qbtn_sm, secondary_button_w, select_click, show_modal,
    style_scrollbar, subbar, uniform_button_width,
};
use crate::{ic, theme, JustQueryApp, PendingConn, Tab, TabKind};
use crate::{SPACE_2, SPACE_3, SPACE_4, SPACE_5, TABBAR_H};
use eframe::egui;
use egui::{Align, Layout, Margin, RichText};

/// The six settings-form fields — the only connection fields the form (and thus the tab-level
/// undo) ever touches. Identity (id/created) and scan settings never travel through undo.
fn conn_forms_differ(a: &Connection, b: &Connection) -> bool {
    a.name != b.name
        || a.host != b.host
        || a.port != b.port
        || a.db != b.db
        || a.user != b.user
        || a.password != b.password
}

/// Copy the six form fields `src → dst`, leaving identity and scan settings untouched (a
/// snapshot taken before the first Save must not zero the id the Save assigned).
fn conn_apply_form(dst: &mut Connection, src: &Connection) {
    dst.name = src.name.clone();
    dst.host = src.host.clone();
    dst.port = src.port.clone();
    dst.db = src.db.clone();
    dst.user = src.user.clone();
    dst.password = src.password.clone();
}

/// Cap on the per-tab form-undo depth (snapshots are small; the cap just bounds a long session).
const CONN_UNDO_MAX: usize = 100;

/// One undo step per field edit SESSION: focus opens a session (snapshotting the form at
/// entry), focus moving away (to another field or out of the form) closes it — the snapshot
/// is pushed only if the form actually changed. `focus_now` is the field holding focus THIS
/// frame (None = the form is unfocused); the undo/redo verbs pass `None` to force-close an
/// open session before stepping.
fn conn_step_session(
    session: &mut Option<(usize, Connection)>,
    undo: &mut Vec<Connection>,
    redo: &mut Vec<Connection>,
    c: &mut Connection,
    focus_now: Option<usize>,
) {
    let same_field =
        matches!((session.as_ref().map(|(f, _)| *f), focus_now), (Some(a), Some(b)) if a == b);
    if !same_field {
        // the session's field lost focus: commit it if the form moved away from its snapshot
        if let Some((_, snap)) = session.take() {
            if conn_forms_differ(c, &snap) {
                undo.push(snap);
                if undo.len() > CONN_UNDO_MAX {
                    undo.remove(0);
                }
                redo.clear();
            }
        }
    }
    if let Some(f) = focus_now {
        if session.is_none() {
            *session = Some((f, c.clone()));
        }
    }
}

impl JustQueryApp {
    /// Connect with the given connection's values (a settings page's form, or the manager's
    /// selected row): pre-flight the required fields, guard in-flight work, then open the real
    /// main connection on a background thread.
    pub(crate) fn connect_connection(&mut self, c: &Connection) {
        // Pre-flight the required fields. A connect against a half-filled form (e.g. an empty
        // host) has no fast failure — it would pin the "Connecting…" overlay until
        // `connect_timeout` — so reject the obvious gaps up front with a clear message.
        let mut missing = Vec::new();
        if c.host.trim().is_empty() {
            missing.push("Host");
        }
        if c.db.trim().is_empty() {
            missing.push("Database");
        }
        if c.user.trim().is_empty() {
            missing.push("User");
        }
        if !missing.is_empty() {
            self.error_modal = Some(format!(
                "Fill in the required fields before connecting: {}.",
                missing.join(", ")
            ));
            return;
        }
        // connecting (which replaces the main connection) while a query runs or a result stream is
        // open would disrupt it — prompt first, and remember the target for "Kill & connect"
        if !self.busy_tabs().is_empty() {
            self.busy_prompt = Some(PendingConn::Connect);
            self.pending_connect = Some(c.clone());
            return;
        }
        self.start_main_connect(c);
    }

    /// Execute (▶ / F8) on a connection page: connect with the form's CURRENT values — unsaved
    /// edits included, the same "run what you see" rule as executing an unsaved script.
    pub(crate) fn connect_from_page(&mut self) {
        let Some(c) = self.cur().and_then(|t| t.conn().cloned()) else {
            return;
        };
        self.connect_connection(&c);
    }

    /// Ctrl+Z on a connection page: undo the last form edit SESSION across the whole tab (a
    /// field, focus → blur) — not egui's per-field buffer. A locked page (the active live
    /// connection) has nothing to undo: its form is read-only.
    pub(crate) fn conn_tab_undo(&mut self) {
        let idx = self.active_tab;
        let (connected, active_id) = (self.connected, self.active_conn_id);
        let mut applied = false;
        if let Some(t) = self.tabs.get_mut(idx) {
            let TabKind::Connection(c) = &mut t.kind else {
                return;
            };
            if connected && active_id == Some(c.id) {
                return; // the live connection's form is locked
            }
            // force-close the open edit session first — in-progress typing is itself a step
            conn_step_session(
                &mut t.conn_session,
                &mut t.conn_undo,
                &mut t.conn_redo,
                c,
                None,
            );
            if let Some(prev) = t.conn_undo.pop() {
                t.conn_redo.push(c.clone());
                conn_apply_form(c, &prev);
                applied = true;
            }
        }
        if applied {
            self.reconcile_conn_dirty(idx);
        }
    }

    /// Ctrl+Shift+Z / Ctrl+Y on a connection page: redo a form step. If closing the open edit
    /// session commits a fresh change, THIS press just does that (classic redo-after-edit is
    /// unavailable) — the redo stack was cleared by the commit.
    pub(crate) fn conn_tab_redo(&mut self) {
        let idx = self.active_tab;
        let (connected, active_id) = (self.connected, self.active_conn_id);
        let mut applied = false;
        if let Some(t) = self.tabs.get_mut(idx) {
            let TabKind::Connection(c) = &mut t.kind else {
                return;
            };
            if connected && active_id == Some(c.id) {
                return; // the live connection's form is locked
            }
            let depth = t.conn_undo.len();
            conn_step_session(
                &mut t.conn_session,
                &mut t.conn_undo,
                &mut t.conn_redo,
                c,
                None,
            );
            if t.conn_undo.len() > depth {
                return; // the session commit consumed this press
            }
            if let Some(next) = t.conn_redo.pop() {
                t.conn_undo.push(c.clone());
                conn_apply_form(c, &next);
                applied = true;
            }
        }
        if applied {
            self.reconcile_conn_dirty(idx);
        }
    }

    /// After an undo/redo step on a connection page, recompute its dirty flag: clean when the
    /// form is back at the persisted state (the `*` goes out), dirty otherwise.
    fn reconcile_conn_dirty(&mut self, idx: usize) {
        let differ = self.tabs.get(idx).and_then(|t| t.conn()).map(|c| {
            match self.connections.iter().find(|s| s.id == c.id) {
                Some(s) => conn_forms_differ(c, s),
                None => true,
            }
        });
        if let (Some(t), Some(d)) = (self.tabs.get_mut(idx), differ) {
            t.conn_dirty = d;
        }
    }

    /// Open a connection on a background thread (the UI is blocked by the "Connecting…" overlay
    /// until it resolves). Everything is STAGED here (`pending_*`) and only applied by
    /// [`Self::finish_main_connect`] — a failed or cancelled attempt must leave the app exactly
    /// as it was (no connection gets marked active, no session/metadata state is torn down).
    /// `c` carries the connection page's current form values.
    fn start_main_connect(&mut self, c: &Connection) {
        let user = c.user.trim().to_string();
        let pass = c.password.clone();
        // status-bar identity: login@<connection name> (not the db name)
        self.pending_label = format!("{}@{}", user, c.name);
        // the resolved credentials each tab will open its own session connection from
        let params = ConnParams {
            host: c.host.clone(),
            port: c.port.clone(),
            db: c.db.clone(),
            user,
            password: pass,
        };
        self.pending_params = Some(params.clone());
        // this connection's id + metadata settings, applied once the connect succeeds
        self.pending_conn_id = Some(c.id);
        self.pending_meta_settings = Some(crate::metadata::CollectorSettings {
            enabled: c.meta_enabled,
            interval: c.meta_interval,
            budget: c.meta_budget,
            idle: c.meta_idle,
            schemas: c.meta_schemas.clone(),
        });
        self.spawn_probe_connect(params);
    }

    /// The in-flight main connect resolved with a live client: only NOW does the app switch
    /// identity. Tear down the previous session state (the connect may target a different
    /// database), then swap in the staged `pending_*` connection.
    pub(crate) fn finish_main_connect(
        &mut self,
        client: postgres::Client,
        pid: Option<i32>,
        ssl: Option<bool>,
    ) {
        // drop any existing tab session connections so they re-open lazily with the new
        // credentials (no tabs are running here: the busy guard in connect_from_page already
        // prompted, or reset_all_sessions cleared them), and any running metadata workers/store
        self.reset_all_sessions();
        self.stop_meta_actors();
        self.main_conn = Some(client);
        self.main_pid = pid;
        self.main_conn_since = Some(crate::dialog::now_hms());
        self.main_ssl = ssl;
        self.connected = true;
        self.conn_broken = false;
        self.active_label = std::mem::take(&mut self.pending_label);
        self.active_conn_id = self.pending_conn_id.take();
        self.conn_params = self.pending_params.take();
        // sync the staged Scan-tab buffers to the new connection's persisted settings
        if let Some(s) = &self.pending_meta_settings {
            self.edit_interval = s.interval;
            self.edit_budget = s.budget;
            self.edit_idle = s.idle;
            self.edit_schemas = s.schemas.clone();
        }
        self.pending_connect = None; // staged target consumed
        self.start_meta_actors(); // begin background metadata collection
    }

    /// The in-flight main connect failed: surface the error in the standard error modal and drop
    /// the staged identity — the app stays exactly as it was (a previous connection, if any,
    /// remains live and active).
    pub(crate) fn fail_main_connect(&mut self, msg: String) {
        self.error_modal = Some(msg);
        self.pending_label.clear();
        self.pending_conn_id = None;
        self.pending_params = None;
        self.pending_meta_settings = None;
        self.pending_connect = None;
    }

    /// Open a control connection in the background and route the outcome to `connect_rx`: parse the
    /// port, then connect and capture pid + ssl in ONE round-trip (the same `pg_stat_ssl` probe the
    /// Test-Connection dialog runs, reused so the Session tab can show the control connection's live
    /// attributes without a second query on the UI thread). The single place this probe-connect
    /// thread is shaped — Execute on a connection page goes through it.
    fn spawn_probe_connect(&mut self, p: ConnParams) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.connect_rx = Some(rx);
        std::thread::spawn(move || {
            let res = match parse_port(&p.port) {
                Ok(port) => connect_client_probed(&p.host, port, &p.db, &p.user, &p.password),
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
        let Some(t) = self.cur_mut() else { return };
        t.stop_requested = true; // the cancel's result is rendered as a red "Query cancelled" status
        let now = std::time::Instant::now();
        if t.stop_since.is_none() {
            t.stop_since = Some(now); // first click opens the 5 s auto-refire window
        }
        t.last_cancel_shot = Some(now);
        // keep the token (clone, don't take): the auto-refire window and nervous re-clicks reuse it
        if let Some(cancel) = t.exec_cancel.clone() {
            spawn_cancel(cancel);
        }
    }

    /// Abort every tab's long-running SQL queries with a server CancelRequest and reset their
    /// sessions. Without the cancel the worker stays blocked in `run_statement` until the server
    /// replies (zombie thread + a busy server session) while the tabs are torn down en masse on
    /// reconnect/disconnect.
    fn reset_all_sessions(&mut self) {
        for t in &mut self.tabs {
            if let Some(cancel) = t.exec_cancel.take() {
                spawn_cancel(cancel);
            }
            t.reset_session();
        }
    }

    /// Drop the control connection and every tab's session connection, clearing all connected
    /// state (including the header label).
    fn disconnect_now(&mut self) {
        self.main_conn = None; // dropping the client closes the control connection
        self.connected = false;
        self.conn_broken = false; // deliberate disconnect → the chip shows nothing, not red
        self.main_pid = None;
        self.main_conn_since = None;
        self.main_ssl = None;
        self.conn_params = None;
        self.active_label.clear();
        // drop every tab's session connection and abandon any in-flight query
        self.reset_all_sessions();
        self.stop_meta_actors();
        self.active_conn_id = None;
    }

    /// Tabs holding the connection busy — a running query or a parked result stream — as
    /// (tab title, reason). Empty until real query execution sets these flags.
    fn busy_tabs(&self) -> Vec<(String, &'static str)> {
        self.tabs
            .iter()
            .filter_map(|t| {
                if t.running {
                    Some((t.title.clone(), "running query"))
                } else if t.fetch_tx.is_some() {
                    // a parked lazy stream still pins a server snapshot/locks on this session
                    Some((t.title.clone(), "open result stream"))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Confirm-disconnect modal (the lightning never disconnects silently). Destructive
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
            if modal_header(ui, "Disconnect") {
                self.disconnect_confirm = false;
            }
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

    /// The Connection Manager side panel (toggled from the toolbar): connection list + New /
    /// Connect / Disconnect / Delete. Single click selects (Ctrl/Shift multi-select);
    /// double-click opens a settings tab; renaming lives on that tab (not inline in the list).
    pub(crate) fn database_manager_panel(&mut self, ui: &mut egui::Ui) {
        if self.left_panel != Some(crate::LeftPanel::Database) {
            return;
        }
        let mut add = false;
        let mut do_delete = false; // toolbar trash → delete the selected connection(s)
        let mut sel_connect: Option<Connection> = None; // toolbar ▶ → connect to this one
        let mut sel_disconnect = false; // toolbar lightning → disconnect the active connection
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
            .frame(
                egui::Frame::new()
                    .fill(p().panel2)
                    .inner_margin(Margin::ZERO),
            )
            .show(ui, |ui| {
                ui.style_mut().visuals.override_text_color = None;
                // header: same height as the tab bar, with a close × on the right
                egui::Panel::top("dbmgr_header")
                    .exact_size(TABBAR_H)
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: crate::DOCK_TITLE_INDENT as i8,
                        right: crate::CHROME_GUTTER as i8,
                        // centre the title in the full row so it lines up with the tab labels
                        // (which are vertically centred in the same TABBAR_H height)
                        top: 0,
                        bottom: 0,
                    }))
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            // the dock can't be narrowed past this title (size_range below), so the
                            // label always fits — no truncation needed. Same size/weight as the tab
                            // labels (BODY_SIZE, regular) so title and tabs read as one chrome band.
                            ui.label(
                                RichText::new("Connection Manager")
                                    .size(13.0)
                                    .color(p().text),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if close_x(ui, "Close panel") {
                                    close_panel = true;
                                }
                            });
                        });
                    });
                // work-area toolbar — a chrome strip under the header: New "+", then the
                // per-selection connection verbs ▶ Connect / lightning Disconnect (the same verbs
                // and guards as the connection's settings page, scoped to the manager's selection
                // instead of the active tab), then Delete. One uniform icon gap throughout — no
                // group separators (the strip is small; grouping reads from the order itself).
                // Export has no button here: it's the toolbar's Save As on the connection tab.
                subbar(ui, "dbmgr_toolbar", crate::CHROME_GUTTER as i8, |ui| {
                    if qbtn_sm(ui, ic::PLUS, p().text, "New connection").clicked() {
                        add = true;
                    }
                    // the verbs act on the SINGLE selected connection: a non-active one → ▶ is
                    // live (connect/switch); the active one → the lightning is live (disconnect).
                    // Multi/none selected or a connect in flight → both dim.
                    let sel: Option<Connection> = if self.conn_sel.len() == 1 {
                        self.connections
                            .iter()
                            .find(|c| c.id == self.conn_sel[0])
                            .cloned()
                    } else {
                        None
                    };
                    let connecting = self.connect_rx.is_some();
                    if let Some(c) = &sel {
                        let is_active = self.connected && self.active_conn_id == Some(c.id);
                        if connecting {
                            qbtn_off_sm(ui, ic::PLAY, "Connect (already connecting)");
                        } else if is_active {
                            qbtn_off_sm(
                                ui,
                                ic::PLAY,
                                "Connect (this connection is already active)",
                            );
                        } else if qbtn_sm(ui, ic::PLAY, p().ok, "Connect").clicked() {
                            sel_connect = Some(c.clone());
                        }
                        if is_active {
                            if qbtn_sm(ui, ic::STOP, p().danger, "Disconnect").clicked() {
                                sel_disconnect = true;
                            }
                        } else {
                            qbtn_off_sm(ui, ic::STOP, "Disconnect (this connection is not active)");
                        }
                    } else {
                        qbtn_off_sm(ui, ic::PLAY, "Connect (select one connection)");
                        qbtn_off_sm(ui, ic::STOP, "Disconnect (select one connection)");
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
                        left: crate::CHROME_GUTTER as i8, // single gutter (window edge)
                        right: crate::CHROME_GUTTER as i8,
                        top: crate::CHROME_GUTTER as i8, // 4px between the sub-toolbar and the data island
                        bottom: 0,
                    }))
                    .show(ui, |ui| {
                        let ids: Vec<u64> = self.connections.iter().map(|c| c.id).collect();
                        let (ctrl, shift) = ui.input(|i| (i.modifiers.ctrl, i.modifiers.shift));
                        // white work-area island (connection list), vertically scrollable
                        crate::widgets::island_panel(ui, p().ivory, |ui| {
                            ui.set_min_size(ui.available_size());
                            // rows fill to the very frame (no 1px inset gap) — the border is redrawn
                            // ON TOP after the list, so edge-row fills never leave a corner gap
                            let clip = ui.max_rect();
                            ui.set_clip_rect(clip);
                            // Floating overlay bar (see the Metadata Manager): reserves no width, so
                            // rows fill edge-to-edge and a bar toggling never reflows them.
                            crate::widgets::style_scrollbar_overlay(ui);
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
                                        ui.add_space(crate::SPACE_2);
                                        empty_hint(ui, "No connections.\nClick + to add.");
                                    }
                                    for (i, (cid, n)) in conns.iter().enumerate() {
                                        let selected = self.conn_sel.contains(cid);
                                        let label = if n.is_empty() {
                                            "(unnamed)"
                                        } else {
                                            n.as_str()
                                        };
                                        // shared manager row (icon + name); selected → tint. The
                                        // live/active connection (its session is up) reads green — glyph + name.
                                        let fg = if self.connected
                                            && self.active_conn_id == Some(*cid)
                                        {
                                            Some(p().ok)
                                        } else {
                                            None
                                        };
                                        let resp = manager_row_fg(
                                            ui,
                                            0.0,
                                            ic::CONNECT,
                                            label,
                                            selected,
                                            fg,
                                        );
                                        // Double-click opens the settings tab. A plain click selects on
                                        // PRESS (so the previous row's accent drops instantly, not after
                                        // the whole button-hold); Ctrl/Shift multi-select on release.
                                        // Renaming happens on the settings tab (Save), not in the list.
                                        if resp.double_clicked() {
                                            open_cid = Some(*cid);
                                        } else if resp.is_pointer_button_down_on()
                                            && !ctrl
                                            && !shift
                                        {
                                            select_click(
                                                &mut self.conn_sel,
                                                &mut self.conn_anchor,
                                                &ids,
                                                i,
                                                false,
                                                false,
                                            );
                                            ui.ctx().request_repaint();
                                        } else if resp.clicked() && (ctrl || shift) {
                                            select_click(
                                                &mut self.conn_sel,
                                                &mut self.conn_anchor,
                                                &ids,
                                                i,
                                                ctrl,
                                                shift,
                                            );
                                            ui.ctx().request_repaint();
                                        }
                                    }
                                });
                        });
                    });
            });
        ui.set_style(saved_style);
        if close_panel {
            self.left_panel = None;
        }
        if do_delete && !self.conn_sel.is_empty() {
            self.confirm = Some(crate::ConfirmAction::DeleteConnections(
                self.conn_sel.clone(),
            ));
        }
        if add {
            // "+" adds the entry to the list AT ONCE — persisted with just its free name, no
            // preset field values — selects it, and opens the settings tab on it as a saved
            // connection. The tab goes dirty as the fields are filled; Save persists them.
            let mut c = Connection {
                name: self.next_new_conn_name(),
                port: String::new(), // Default carries "5432" — a new entry starts truly empty
                ..Default::default()
            };
            c.id = self.connections.iter().map(|c| c.id).max().unwrap_or(0) + 1;
            // stamp creation order so it sorts after existing connections (and persists)
            c.created = now_ms().max(
                self.connections
                    .iter()
                    .map(|c| c.created)
                    .max()
                    .unwrap_or(0)
                    + 1,
            );
            self.connections.push(c.clone());
            save(&self.connections);
            self.conn_sel = vec![c.id]; // the new row is the selection (drops any stale one)
            self.conn_anchor = Some(self.connections.len() - 1);
            self.open_conn_tab(c);
        }
        if let Some(c) = sel_connect {
            self.connect_connection(&c);
        }
        if sel_disconnect {
            self.request_disconnect();
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

    /// Duplicate-name prompt when saving a settings tab (Windows-style "(2)" suggestion):
    /// Rename takes the suggested free name and commits all fields; Keep editing returns to
    /// the tab's Name field.
    pub(crate) fn conflict_modal(&mut self, ctx: &egui::Context) {
        let Some((_id, suggestion)) = self.dbmgr_conflict.clone() else {
            return;
        };
        let taken = self.conflict_taken.trim().to_string();
        let mut do_rename = false;
        let mut keep_editing = false;
        let r = show_modal(ctx, "conflict", 360.0, |ui| {
            ui.label(
                RichText::new("Name already in use")
                    .size(crate::HEADING_SIZE)
                    .strong()
                    .color(p().text),
            );
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
            // the rename came from the settings-tab Save — take the name and commit all fields
            if let Some(t) = self.cur_mut() {
                if let Some(c) = t.conn_mut() {
                    c.name = suggestion.clone();
                }
            }
            self.commit_conn_tab();
            save(&self.connections);
            self.dbmgr_conflict = None;
        }
        if keep_editing {
            self.dbmgr_conflict = None;
        }
    }

    /// Open (or focus) an editor tab that edits the given connection.
    pub(crate) fn open_conn_tab(&mut self, conn: Connection) {
        if conn.id != 0 {
            if let Some(i) = self
                .tabs
                .iter()
                .position(|t| t.conn().is_some_and(|c| c.id == conn.id))
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
        tab.kind = TabKind::Connection(conn);
        tab.conn_dirty = is_new; // a brand-new connection is unsaved
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        self.focus_editor = true;
    }

    /// Save the active connection tab: validate (name required + unique), commit, persist to disk.
    /// One connection == one file named after it, so duplicate names are rejected here.
    pub(crate) fn save_conn_tab(&mut self) {
        let idx = self.active_tab;
        let Some(conn) = self.tabs.get(idx).and_then(|t| t.conn().cloned()) else {
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
            // offer a free "(2)" variant via the duplicate-name prompt
            let suggestion = self.free_variant(&name, conn.id);
            self.conflict_taken = name.clone(); // shown as the "taken" name in the prompt
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
        let Some(mut conn) = self.tabs.get(idx).and_then(|t| t.conn().cloned()) else {
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
                    self.connections
                        .iter()
                        .map(|c| c.created)
                        .max()
                        .unwrap_or(0)
                        + 1,
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
            t.kind = TabKind::Connection(conn);
            t.title = title;
            t.conn_dirty = false;
        }
    }

    /// Kick off a real Test Connection for the active connection tab on a background thread.
    pub(crate) fn start_conn_test(&mut self) {
        let Some(c) = self.cur().and_then(|t| t.conn().cloned()) else {
            return;
        };
        // Pre-flight the required fields. A Test against a half-filled form (e.g. an empty host)
        // has no fast failure — it would pin the Test spinner until `connect_timeout` (8s) — so we
        // reject the obvious gaps up front with a clear message instead of spinning the modal.
        let mut missing = Vec::new();
        if c.host.trim().is_empty() {
            missing.push("Host");
        }
        if c.db.trim().is_empty() {
            missing.push("Database");
        }
        if c.user.trim().is_empty() {
            missing.push("User");
        }
        if !missing.is_empty() {
            self.error_modal = Some(format!(
                "Fill in the required fields before testing: {}.",
                missing.join(", ")
            ));
            return;
        }
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

    /// Export the active connection tab to a chosen `.conn` file (the Save As verb on a connection
    /// tab). Writes the same on-disk format as the connection store, minus the password — exports
    /// carry no credentials, so the file is safe to hand around.
    pub(crate) fn export_active_conn(&mut self) {
        let Some(mut c) = self.cur().and_then(|t| t.conn().cloned()) else {
            return;
        };
        // the password never leaves the machine: an export carries every setting EXCEPT it (the
        // DPAPI ciphertext is machine-bound, but a copy would still import on THIS machine)
        c.password = String::new();
        let suggested = format!("{}.conn", safe_name(&c.name));
        let Some(dest) = crate::dialog::save_file(Some(&suggested)) else {
            return;
        };
        if let Err(e) = std::fs::write(&dest, conn_to_text(&c)) {
            self.error_modal = Some(format!("Export failed: {e}"));
        }
    }

    /// Delete a connection: drop it from the list, prune its file, and close any open settings tab.
    pub(crate) fn delete_connection(&mut self, id: u64) {
        self.connections.retain(|c| c.id != id);
        save(&self.connections); // rewrites the dir and prunes the now-orphaned file
        let idxs: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.conn().is_some_and(|c| c.id == id))
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
            if modal_header(ui, "Test connection") {
                close = true;
            }
            ui.add_space(SPACE_4);
            // fixed-height status area → no resize between the spinner and the result; the result
            // message wraps in place (no truncation, so no hover-tooltip — that was doubling up)
            let (rect, _) = ui.allocate_exact_size(
                egui::Vec2::new(ui.available_width(), 80.0),
                egui::Sense::hover(),
            );
            let mut sui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(Layout::top_down(Align::Min)),
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
                        ui.label(
                            RichText::new("Connection successful")
                                .strong()
                                .color(p().ok),
                        );
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(msg)
                                    .color(p().text_dim)
                                    .size(crate::LABEL_SIZE),
                            )
                            .wrap(),
                        );
                    }
                    Some(Err(msg)) => {
                        ui.label(
                            RichText::new("Connection failed")
                                .strong()
                                .color(p().danger),
                        );
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(msg)
                                    .color(p().text_dim)
                                    .size(crate::LABEL_SIZE),
                            )
                            .wrap(),
                        );
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
            crate::request_poll(ctx);
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
            ui.label(
                RichText::new("Error")
                    .size(crate::HEADING_SIZE)
                    .strong()
                    .color(p().danger),
            );
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
    /// or an open result stream. Lists the affected tabs and offers Go back or Kill & proceed.
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
        let r = show_modal(ctx, "busy", 360.0, |ui| {
            ui.label(
                RichText::new("Work in progress")
                    .size(crate::HEADING_SIZE)
                    .strong()
                    .color(p().text),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "Some tabs are still busy — {verb}ing will interrupt them:"
                ))
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
            // abandon every tab's running query / parked stream and drop the session connections
            self.reset_all_sessions();
            match action {
                PendingConn::Connect => {
                    // the page's connection, staged when the prompt went up
                    if let Some(c) = self.pending_connect.take() {
                        self.start_main_connect(&c);
                    }
                }
                PendingConn::Disconnect => self.disconnect_now(),
            }
        }
    }

    /// Blocking "Connecting…" overlay while the main connection is being opened on the bg thread.
    pub(crate) fn connecting_modal(&mut self, ctx: &egui::Context) {
        if self.connect_rx.is_none() {
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
        crate::request_poll(ctx);
    }

    /// The connection-settings page: the form on the data sheet, plus a footer with the page
    /// actions (Test connection · Save). All of the page's actions live here in the footer (they
    /// used to be mirrored into the toolbar, but the toolbar is now a static strip shared by every
    /// tab, and a connection tab adds nothing to it).
    pub(crate) fn connection_tab(&mut self, ui: &mut egui::Ui) {
        let idx = self.active_tab.min(self.tabs.len().saturating_sub(1));
        // Actions (Save · Test connection) live on the main toolbar now — the page carries no
        // buttons of its own (a Page only ever holds clickable content, never widget buttons).
        let mut changed = false;
        // the form field holding focus this frame (its ordinal) — reported by the rows below,
        // consumed by the tab-level undo's session bookkeeping after the panel
        let mut focused_field: Option<usize> = None;

        // Live-connection state for the ACTIVE connection's page — captured before the form borrows
        // the tab's Connection mutably, so reading these can't clash with that &mut.
        let active_id = self.active_conn_id;
        let broken = self.conn_broken;
        let last_error = self.last_error.clone();
        let conn_params = self.conn_params.clone();
        let main_pid = self.main_pid;
        let main_since = self.main_conn_since.clone();
        let main_ssl = self.main_ssl;

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p().panel2)
                    .inner_margin(self.island_margin()),
            )
            .show(ui, |ui| {
                // silvery data sheet inside the side borders, with a thin border of its own
                let sheet = ui.max_rect();
                crate::widgets::island_shadow_under(ui.painter(), sheet);
                crate::widgets::island_box(ui.painter(), sheet, p().data_bg, crate::RADIUS_ISLAND);
                style_scrollbar(ui);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(Margin::symmetric(18, 16))
                            .show(ui, |ui| {
                                theme::style_modal_widgets(ui); // fields use the shared border
                                ui.set_max_width(600.0);
                                // field ordinal counter for `focused_field` (rows report in call order)
                                let mut field_i = 0usize;
                                if let Some(c) = self.tabs.get_mut(idx).and_then(|t| t.conn_mut()) {
                                    // the active (live) connection: its session is up, its settings are
                                    // locked, and its page carries the runtime Session block below.
                                    let is_active = active_id == Some(c.id);

                                    // ---- title = connection name (+ active / disconnected marker) ----
                                    let title = if c.name.trim().is_empty() {
                                        "(unnamed)".to_owned()
                                    } else {
                                        c.name.clone()
                                    };
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(&title)
                                                .font(theme::ui_bold_font(crate::HEADING_SIZE))
                                                .color(p().text),
                                        );
                                        if is_active {
                                            // green "● active" while live; red "● disconnected" if dropped
                                            // (same pattern as the Scan tab)
                                            ui.add_space(SPACE_2);
                                            let (word, col) = if broken {
                                                ("disconnected", p().danger)
                                            } else {
                                                ("active", p().ok)
                                            };
                                            ui.label(RichText::new("●").color(col));
                                            ui.label(
                                                RichText::new(word)
                                                    .color(col)
                                                    .size(crate::LABEL_SIZE),
                                            );
                                        }
                                    });
                                    // the failure reason while the active connection is dropped
                                    if is_active && broken {
                                        if let Some(e) = &last_error {
                                            ui.add_space(4.0);
                                            ui.label(
                                                RichText::new(e)
                                                    .color(p().danger)
                                                    .size(crate::LABEL_SIZE),
                                            );
                                        }
                                    }
                                    ui.add_space(SPACE_3);

                                    // ---- settings form: LOCKED while this is the active connection ----
                                    let editable = !is_active;
                                    egui::Grid::new("conn_form")
                                        .num_columns(2)
                                        .spacing([12.0, 8.0])
                                        .min_col_width(64.0)
                                        .show(ui, |ui| {
                                            let mut row =
                                                |label: &str, v: &mut String, pw: bool| {
                                                    ui.label(
                                                        RichText::new(label)
                                                            .color(p().text_dim)
                                                            .size(crate::LABEL_SIZE),
                                                    );
                                                    let mut te = egui::TextEdit::singleline(v)
                                                        // shared field inset + vertical centring (theme.rs)
                                                        .margin(crate::theme::field_margin())
                                                        .vertical_align(Align::Center);
                                                    if pw {
                                                        te = te.password(true);
                                                    }
                                                    if !editable {
                                                        // dim it so it visibly reads as "locked / not editable"
                                                        te = te
                                                            .interactive(false)
                                                            .text_color(p().text_dim);
                                                    }
                                                    // add_sized pins the field to FIELD_H so the centred text
                                                    // lines up with every other field
                                                    let r = ui.add_sized(
                                                        egui::Vec2::new(
                                                            280.0,
                                                            crate::theme::FIELD_H,
                                                        ),
                                                        te,
                                                    );
                                                    if r.changed() {
                                                        changed = true;
                                                    }
                                                    if r.has_focus() {
                                                        focused_field = Some(field_i);
                                                    }
                                                    ui.end_row();
                                                    field_i += 1;
                                                };
                                            // Name is editable; Save validates uniqueness (duplicate
                                            // → conflict prompt) and renames the backing file
                                            row("Name", &mut c.name, false);
                                            row("Host", &mut c.host, false);
                                            row("Port", &mut c.port, false);
                                            row("Database", &mut c.db, false);
                                            row("User", &mut c.user, false);
                                            row("Password", &mut c.password, true);
                                        });

                                    // ---- live Session: the active connection's physical/runtime data
                                    // (server · db · since · user · pid · ssl), moved off the old Session
                                    // tab. Shown only for the connection whose session is (or was) up. ----
                                    if is_active {
                                        ui.add_space(SPACE_3);
                                        ui.label(
                                            RichText::new("Session")
                                                .size(crate::BODY_SIZE)
                                                .strong()
                                                .color(p().text),
                                        );
                                        ui.add_space(SPACE_2);
                                        let kv = |ui: &mut egui::Ui, k: &str, v: String| {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new(k)
                                                        .color(p().text_dim)
                                                        .size(crate::LABEL_SIZE),
                                                );
                                                ui.add_space(SPACE_2);
                                                ui.label(
                                                    RichText::new(v)
                                                        .color(p().text)
                                                        .size(crate::LABEL_SIZE),
                                                );
                                            });
                                        };
                                        let pp = conn_params.as_ref();
                                        ui.columns(2, |cols| {
                                            kv(
                                                &mut cols[0],
                                                "Server",
                                                format!(
                                                    "{}:{}",
                                                    pp.map(|p| p.host.clone()).unwrap_or_default(),
                                                    pp.map(|p| p.port.clone()).unwrap_or_default(),
                                                ),
                                            );
                                            kv(
                                                &mut cols[0],
                                                "Database",
                                                pp.map(|p| p.db.clone())
                                                    .unwrap_or_else(|| "—".to_owned()),
                                            );
                                            kv(
                                                &mut cols[0],
                                                "Since",
                                                main_since
                                                    .clone()
                                                    .unwrap_or_else(|| "—".to_owned()),
                                            );
                                            kv(
                                                &mut cols[1],
                                                "User",
                                                pp.map(|p| p.user.clone())
                                                    .unwrap_or_else(|| "—".to_owned()),
                                            );
                                            kv(
                                                &mut cols[1],
                                                "Pid",
                                                main_pid
                                                    .map(|n| n.to_string())
                                                    .unwrap_or_else(|| "—".to_owned()),
                                            );
                                            kv(
                                                &mut cols[1],
                                                "SSL",
                                                main_ssl
                                                    .map(|b| if b { "on" } else { "off" })
                                                    .unwrap_or("—")
                                                    .to_owned(),
                                            );
                                        });
                                    }
                                }
                            });
                    });
                if changed {
                    if let Some(t) = self.tabs.get_mut(idx) {
                        t.conn_dirty = true;
                    }
                }
                // tab-level undo bookkeeping: close/open the field edit session around what
                // the form just rendered (the panel above only reported `focused_field`)
                if let Some(t) = self.tabs.get_mut(idx) {
                    if let TabKind::Connection(c) = &mut t.kind {
                        conn_step_session(
                            &mut t.conn_session,
                            &mut t.conn_undo,
                            &mut t.conn_redo,
                            c,
                            focused_field,
                        );
                    }
                }
            });
    }
}
