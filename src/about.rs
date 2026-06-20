//! The About / Updates page (a singleton tab) and the update-flow UI state: the version chip, the
//! background check/download kickoffs and the per-frame channel drain. The HTTP/self-update layer
//! lives in [`crate::update`].

use crate::brand::logo;
use crate::theme::p;
use crate::widgets::{secondary_button_w, style_scrollbar, uniform_button_width};
use crate::{ic, theme, update, widgets, JustQueryApp, Tab, TabKind};
use crate::{SPACE_2, SPACE_3, SPACE_4};
use eframe::egui;
use egui::{Margin, RichText};

impl JustQueryApp {
    /// Open the About / Updates tab, kicking a first version check if none has run yet. At most one
    /// exists: if it's already open this just re-selects it; otherwise a fresh About tab is created.
    pub(crate) fn open_about(&mut self) {
        if matches!(self.update_status, update::UpdateStatus::NeverChecked) {
            self.start_update_check();
        }
        if let Some(i) = self.tabs.iter().position(|t| matches!(t.kind, TabKind::About)) {
            self.active_tab = i;
            return;
        }
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id, "About".to_owned());
        tab.kind = TabKind::About;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
    }

    /// Kick the background version check (no-op if a check/download is already running).
    pub(crate) fn start_update_check(&mut self) {
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
    /// the latest build, amber when a newer release exists. Click opens the About tab.
    pub(crate) fn version_chip(&mut self, ui: &mut egui::Ui, sz: f32) {
        let outdated = self.update_outdated == Some(true);
        let (fg, tip) = if outdated {
            (p().warn, "A newer version is available — click to view")
        } else {
            (p().ok, "You're on the latest version")
        };
        // чип-кнопка с hover-подсветкой, чтобы было понятно, что это кликабельно
        let resp = crate::widgets::chip_button(ui, update::CURRENT_VERSION, fg, sz);
        if resp.on_hover_text(tip).clicked() {
            self.open_about();
        }
    }

    /// Drain the in-flight update check / download channel into `update_status` / `update_outdated`.
    pub(crate) fn poll_update(&mut self, ctx: &egui::Context) {
        // poll the in-flight update check / download (background thread)
        if self.update_rx.is_some() {
            let mut release = false;
            loop {
                let msg = match self.update_rx.as_ref().unwrap().try_recv() {
                    Ok(m) => m,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        // ~10 Hz poll while the check/download runs (it fires on every launch —
                        // a bare request_repaint would pin max FPS until GitHub answers)
                        crate::request_poll(ctx);
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
    }

    /// The About / Updates page (a singleton tab; replaces the old About modal). Shows the version
    /// and, driven by `self.update_status`, the check / download / restart controls in the footer —
    /// all of the page's actions live here in the body (they used to be mirrored into the toolbar,
    /// but the toolbar is now a static strip shared by every tab).
    pub(crate) fn about_tab(&mut self, ui: &mut egui::Ui) {
        let status = self.update_status.clone();
        // health colour for the version chip — same as the status-bar version chip (green on the
        // latest build, amber when a newer one exists)
        let ver_fg = if self.update_outdated == Some(true) { p().warn } else { p().ok };
        let mut do_check = false;
        let mut do_download = false;

        // ---- body: the About content, on the silvery data sheet with normal tab scrolling ----
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(self.island_margin()))
            .show_inside(ui, |ui| {
                let sheet = ui.max_rect();
                widgets::island_shadow_under(ui.painter(), sheet);
                widgets::island_box(ui.painter(), sheet, p().data_bg, crate::RADIUS_ISLAND);
                style_scrollbar(ui);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(18, 16))
                        .show(ui, |ui| {
                            theme::style_modal_widgets(ui);
                            ui.set_max_width(440.0); // keep the informational column readable
                            // header: logo + app name (the tab's own × closes the page)
                            ui.horizontal(|ui| {
                                logo(ui, 28.0);
                                ui.add_space(SPACE_3);
                                ui.label(
                                    RichText::new("JustQuery")
                                        .font(theme::ui_bold_font(20.0))
                                        .color(p().text),
                                );
                            });
                            ui.add_space(SPACE_3);
                            // version chip coloured like the status-bar one (green/amber)
                            widgets::status_chip(
                                ui,
                                &format!("Version {}", update::CURRENT_VERSION),
                                ver_fg,
                                theme::tint(p().panel, ver_fg, 0.16),
                                12.0,
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
                            // status line — flows at its natural height (in a tab the footer follows
                            // the content, so there's no fixed reservation that would pad the page)
                            {
                                // status line — describes the current state (or the error in red)
                                match &status {
                                    update::UpdateStatus::Checking => {
                                        ui.horizontal(|ui| {
                                            ui.spinner();
                                            ui.add_space(8.0);
                                            ui.label(
                                                RichText::new("Checking for updates…")
                                                    .color(p().text_dim),
                                            );
                                        });
                                    }
                                    update::UpdateStatus::Latest => {
                                        ui.label(
                                            RichText::new(format!(
                                                "{}  You're on the latest version.",
                                                ic::SCAN_OK
                                            ))
                                            .color(p().ok),
                                        );
                                    }
                                    update::UpdateStatus::Available { latest } => {
                                        ui.label(
                                            RichText::new(format!("Version {latest} is available."))
                                                .color(p().warn),
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
                                                    RichText::new(format!(
                                                        "Downloading… {} KB",
                                                        done / 1024
                                                    ))
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
                            }

                            // footer right under the content (a `with_layout` would grab the full
                            // remaining height and float the button mid-page); left-aligned with the
                            // content. The adaptive update action mirrors the toolbar icon; no Close
                            // button — the tab's own × dismisses the page.
                            ui.add_space(SPACE_2);
                            ui.horizontal(|ui| {
                                let bw = uniform_button_width(
                                    ui,
                                    &["Download & Install", "Check for updates"],
                                );
                                match &status {
                                    update::UpdateStatus::Available { .. }
                                    | update::UpdateStatus::Error { retry_download: true, .. } => {
                                        if secondary_button_w(ui, "Download & Install", true, bw) {
                                            do_download = true;
                                        }
                                    }
                                    update::UpdateStatus::Checking => {
                                        secondary_button_w(ui, "Check for updates", false, bw);
                                    }
                                    update::UpdateStatus::Downloading { .. }
                                    | update::UpdateStatus::Applying
                                    | update::UpdateStatus::PendingRestart => {
                                        // nothing actionable while a download/install is in flight
                                    }
                                    // NeverChecked, Latest, Error { retry_download: false }
                                    _ => {
                                        if secondary_button_w(ui, "Check for updates", true, bw) {
                                            do_check = true;
                                        }
                                    }
                                }
                            });
                        });
                });
            });

        if do_check {
            self.start_update_check();
        }
        if do_download {
            self.start_update_download();
        }
    }
}
