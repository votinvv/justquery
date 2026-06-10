//! The About / Updates modal and the update-flow UI state: the version chip, the background
//! check/download kickoffs and the per-frame channel drain. The HTTP/self-update layer lives
//! in [`crate::update`].

use crate::theme::p;
use crate::widgets::{
    close_x, logo, primary_button_w, secondary_button_w, show_modal, uniform_button_width,
};
use crate::{ic, theme, update, widgets, JustQueryApp};
use crate::{SPACE_2, SPACE_3, SPACE_4};
use eframe::egui;
use egui::{Align, Layout, RichText, Vec2};

impl JustQueryApp {
    /// Open the About / Updates modal, kicking a first version check if none has run yet.
    pub(crate) fn open_about_tab(&mut self) {
        self.about_open = true;
        if matches!(self.update_status, update::UpdateStatus::NeverChecked) {
            self.start_update_check();
        }
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
    /// the latest build, amber when a newer release exists. Click opens the About modal.
    pub(crate) fn version_chip(&mut self, ui: &mut egui::Ui, sz: f32) {
        let outdated = self.update_outdated == Some(true);
        let (fg, tip) = if outdated {
            (p().warn, "A newer version is available — click to view")
        } else {
            (p().ok, "You're on the latest version")
        };
        let resp = ui.add(
            egui::Label::new(RichText::new(format!("v{}", update::CURRENT_VERSION)).size(sz).color(fg))
                .sense(egui::Sense::click()),
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.on_hover_text(tip).clicked() {
            self.open_about_tab();
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
                        ctx.request_repaint();
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

    /// The About / Updates modal (replaces the old About tab). Shows the version and, driven by
    /// `self.update_status`, the check / download / restart controls in the footer (buttons at the
    /// bottom — Design System §7). Informational, so Enter/Esc both just close it.
    pub(crate) fn about_modal(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let status = self.update_status.clone();
        // health colour for the version chip — same as the status-bar version chip (green on the
        // latest build, amber when a newer one exists)
        let ver_fg = if self.update_outdated == Some(true) { p().warn } else { p().ok };
        let mut do_check = false;
        let mut do_download = false;
        let mut close = false;
        let r = show_modal(ctx, "about", 440.0, |ui| {
            // header: logo + app name + ×
            ui.horizontal(|ui| {
                logo(ui, 28.0);
                ui.add_space(SPACE_3);
                ui.label(
                    RichText::new("JustQuery").font(theme::ui_bold_font(20.0)).color(p().text),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_x(ui, "Close") {
                        close = true;
                    }
                });
            });
            ui.add_space(SPACE_3);
            // version chip coloured like the status-bar one (green/amber), tinted from the panel
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
            // Fixed-height status region: the modal is centre-anchored, so a status line that grows
            // (spinner / progress / hint) would shove the footer down on each state change. Reserve
            // a constant height and render the status into it.
            let (info_rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 60.0), egui::Sense::hover());
            let mut iui = ui.new_child(
                egui::UiBuilder::new().max_rect(info_rect).layout(Layout::top_down(Align::Min)),
            );
            {
                let ui = &mut iui;
                // status line — describes the current state (or the error in red)
                match &status {
                update::UpdateStatus::Checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.add_space(8.0);
                        ui.label(RichText::new("Checking for updates…").color(p().text_dim));
                    });
                }
                update::UpdateStatus::Latest => {
                    ui.label(
                        RichText::new(format!("{}  You're on the latest version.", ic::SCAN_OK))
                            .color(p().ok),
                    );
                }
                update::UpdateStatus::Available { latest } => {
                    ui.label(
                        RichText::new(format!("Version {latest} is available.")).color(p().warn),
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
                                RichText::new(format!("Downloading… {} KB", done / 1024))
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
            } // end fixed-height status region

            ui.add_space(SPACE_4);
            // footer: Close is the primary (accent), rightmost; the adaptive update action sits to
            // its left as a secondary button. Uniform width, right-aligned at the bottom.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let bw = uniform_button_width(
                    ui,
                    &["Download & Install", "Check for updates", "Close"],
                );
                if primary_button_w(ui, "Close", true, bw) {
                    close = true;
                }
                // A secondary update action sits left of Close only when one applies. While
                // downloading / installing — and once installed (PendingRestart) — only Close is
                // shown; to apply a downloaded update the user restarts JustQuery (per the message).
                match &status {
                    update::UpdateStatus::Available { .. }
                    | update::UpdateStatus::Error { retry_download: true, .. } => {
                        ui.add_space(SPACE_2);
                        if secondary_button_w(ui, "Download & Install", true, bw) {
                            do_download = true;
                        }
                    }
                    update::UpdateStatus::Checking => {
                        ui.add_space(SPACE_2);
                        secondary_button_w(ui, "Check for updates", false, bw);
                    }
                    update::UpdateStatus::Downloading { .. }
                    | update::UpdateStatus::Applying
                    | update::UpdateStatus::PendingRestart => {
                        // only Close
                    }
                    // NeverChecked, Latest, Error { retry_download: false }
                    _ => {
                        ui.add_space(SPACE_2);
                        if secondary_button_w(ui, "Check for updates", true, bw) {
                            do_check = true;
                        }
                    }
                }
            });
        });
        // informational dialog — Enter and Esc both just close it
        if close || r.escape || r.enter {
            self.about_open = false;
        }
        if do_check {
            self.start_update_check();
        }
        if do_download {
            self.start_update_download();
        }
    }
}
