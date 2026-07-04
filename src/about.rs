//! The About / Updates page (a singleton tab) and the update-flow UI state: the version chip, the
//! background check/download kickoffs and the per-frame channel drain. The HTTP/self-update layer
//! lives in [`crate::update`].

use crate::brand::logo;
use crate::theme::p;
use crate::widgets::style_scrollbar;
use crate::{ic, theme, update, widgets, JustQueryApp, Tab, TabKind};
use crate::{SPACE_3, SPACE_4};
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

    /// Kick the SILENT background download (no install yet; no-op if an update op is already
    /// running). Fired automatically by `poll_update` when a check finds a newer build — the user
    /// never presses anything to download. The install is a separate explicit step.
    fn start_update_download(&mut self) {
        if self.update_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.update_rx = Some(rx);
        self.update_status = update::UpdateStatus::Downloading { done: 0, total: 0 };
        update::spawn_download(tx);
    }

    /// Kick the install of the already-downloaded exe — the user's explicit "click to install" on
    /// the About page (a Windows UAC prompt may appear). No-op if an update op is already running.
    fn start_update_install(&mut self) {
        if self.update_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.update_rx = Some(rx);
        self.update_status = update::UpdateStatus::Applying;
        update::spawn_install(tx);
    }

    /// Status-bar version label (plain text, same font/size as the rest of the bar): green when on
    /// the latest build, amber when a newer release exists. Click opens the About tab.
    pub(crate) fn version_chip(&mut self, ui: &mut egui::Ui, sz: f32) {
        let downloaded = matches!(self.update_status, update::UpdateStatus::Downloaded { .. });
        let outdated = self.update_outdated == Some(true);
        // amber once a newer build exists (and stays amber through download/ready); green otherwise
        let (fg, tip) = if downloaded {
            (p().warn, "Update downloaded — click to install")
        } else if outdated {
            (p().warn, "A newer version is available — click to view")
        } else {
            (p().ok, "You're on the latest version")
        };
        // chip button with a hover highlight so it reads as clickable
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
            let mut kick_download = false; // a check found a newer build → auto-download it (silent)
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
                        if r.is_newer {
                            self.update_status =
                                update::UpdateStatus::Available { latest: r.latest_tag };
                            kick_download = true; // download it automatically, silently
                        } else {
                            self.update_status = update::UpdateStatus::Latest;
                        }
                        release = true;
                        break;
                    }
                    update::UpdateMsg::CheckDone(Err(e)) => {
                        self.update_status =
                            update::UpdateStatus::Error { msg: e, retry: update::Retry::Check };
                        release = true;
                        break;
                    }
                    update::UpdateMsg::Progress { done, total } => {
                        self.update_status = update::UpdateStatus::Downloading { done, total };
                    }
                    update::UpdateMsg::Downloaded(latest) => {
                        // staged and ready — the chip yellows; the install waits for the user's click
                        self.update_outdated = Some(true);
                        self.update_status = update::UpdateStatus::Downloaded { latest };
                        release = true;
                        break;
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
                            update::UpdateStatus::Error { msg: e, retry: update::Retry::Check };
                        release = true;
                        break;
                    }
                    update::UpdateMsg::InstallFailed(e) => {
                        self.update_status =
                            update::UpdateStatus::Error { msg: e, retry: update::Retry::Install };
                        release = true;
                        break;
                    }
                }
            }
            if release {
                self.update_rx = None;
            }
            // auto-download once the check channel is released (start_update_download bails while
            // update_rx is still Some, so this must run after the drain above clears it)
            if kick_download {
                self.start_update_download();
            }
        }
    }

    /// The About / Updates page (a singleton tab; replaces the old About modal). Shows the version
    /// and, driven by `self.update_status`, the check / download / restart controls in the footer —
    /// all of the page's actions live here in the body (they used to be mirrored into the toolbar,
    /// but the toolbar is now a static strip shared by every tab).
    pub(crate) fn about_tab(&mut self, ui: &mut egui::Ui) {
        let status = self.update_status.clone();
        // Actionable states use CLICKABLE content (a Page carries no buttons): the user clicks the
        // status line to install the staged update or to retry after a failure.
        let mut do_install = false;
        let mut do_retry: Option<update::Retry> = None;

        // ---- body: the About content, on the silvery data sheet with normal tab scrolling ----
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(self.island_margin()))
            .show(ui, |ui| {
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
                                        .font(theme::ui_bold_font(crate::HEADING_SIZE))
                                        .color(p().text),
                                );
                            });
                            ui.add_space(SPACE_3);
                            // plain version line — the health colour lives on the status-bar
                            // version chip only; here the update section below tells the story
                            ui.label(
                                RichText::new(format!("Version {}", update::CURRENT_VERSION))
                                    .color(p().text),
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
                                    .size(crate::LABEL_SIZE),
                            );
                            ui.add_space(SPACE_4);
                            ui.separator();
                            ui.add_space(SPACE_3);
                            // status line — flows at its natural height (in a tab the footer follows
                            // the content, so there's no fixed reservation that would pad the page)
                            {
                                // status line — describes the current state. Actionable states are
                                // CLICKABLE content (underlined + pointing-hand cursor), never buttons:
                                // the Page has no buttons of its own.
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
                                    // On the latest build there's nothing to report — the page stays
                                    // quiet (the green version label above already says "current").
                                    update::UpdateStatus::Latest => {}
                                    update::UpdateStatus::Available { latest } => {
                                        // a newer build was found — the download starts automatically
                                        // (silent); this is passive, there's nothing to click yet.
                                        ui.horizontal(|ui| {
                                            ui.spinner();
                                            ui.add_space(8.0);
                                            ui.label(
                                                RichText::new(format!(
                                                    "Version {latest} found — downloading…"
                                                ))
                                                .color(p().text_dim),
                                            );
                                        });
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
                                    update::UpdateStatus::Downloaded { latest } => {
                                        // staged and ready — click the line to install (UAC may appear)
                                        let r = ui.add(
                                            egui::Label::new(
                                                RichText::new(format!(
                                                    "Version {latest} downloaded — click to install"
                                                ))
                                                .strong()
                                                .underline()
                                                .color(p().warn),
                                            )
                                            .sense(egui::Sense::click()),
                                        );
                                        if r
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .on_hover_text("Install the downloaded update")
                                            .clicked()
                                        {
                                            do_install = true;
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
                                                ic::SCAN
                                            ))
                                            .size(14.0)
                                            .strong()
                                            .color(p().ok),
                                        );
                                    }
                                    update::UpdateStatus::NeverChecked => {}
                                    update::UpdateStatus::Error { msg, retry } => {
                                        ui.label(RichText::new(msg).color(p().danger));
                                        ui.add_space(4.0);
                                        let label = match retry {
                                            update::Retry::Check => "Click to check again",
                                            update::Retry::Install => "Click to retry the install",
                                        };
                                        let r = ui.add(
                                            egui::Label::new(
                                                RichText::new(label).underline().color(p().warn),
                                            )
                                            .sense(egui::Sense::click()),
                                        );
                                        if r
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .clicked()
                                        {
                                            do_retry = Some(*retry);
                                        }
                                    }
                                }

                                // UAC hint while an update is downloading / staged (install needs it)
                                if matches!(
                                    status,
                                    update::UpdateStatus::Available { .. }
                                        | update::UpdateStatus::Downloaded { .. }
                                ) {
                                    ui.add_space(8.0);
                                    ui.label(
                                        RichText::new(
                                            "A Windows permission prompt (UAC) may appear to install into \
                                             Program Files. After it finishes, restart JustQuery.",
                                        )
                                        .color(p().text_dim)
                                        .size(crate::LABEL_SIZE),
                                    );
                                }
                            }
                        });
                });
            });

        if do_install {
            self.start_update_install();
        }
        if let Some(r) = do_retry {
            match r {
                update::Retry::Check => self.start_update_check(),
                update::Retry::Install => self.start_update_install(),
            }
        }
    }
}
