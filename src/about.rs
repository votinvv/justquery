//! The About page (a singleton tab) and the status-bar version chip. There is no in-app update
//! check: new versions ship as the same single exe — download it from GitHub Releases and
//! replace the running file (the page links there).

use crate::brand::{self, logo};
use crate::theme::p;
use crate::widgets::style_scrollbar;
use crate::{theme, widgets, JustQueryApp, Tab, TabKind};
use crate::{SPACE_3, SPACE_4};
use eframe::egui;
use egui::{Margin, RichText};

impl JustQueryApp {
    /// Open the About tab. At most one exists: if it's already open this just re-selects it;
    /// otherwise a fresh About tab is created.
    pub(crate) fn open_about(&mut self) {
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| matches!(t.kind, TabKind::About))
        {
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

    /// Status-bar version label (plain text, same font/size as the rest of the bar). Click opens
    /// the About tab.
    pub(crate) fn version_chip(&mut self, ui: &mut egui::Ui, sz: f32) {
        let resp = crate::widgets::chip_button(ui, brand::CURRENT_VERSION, p().text_dim, sz);
        if resp.on_hover_text("About").clicked() {
            self.open_about();
        }
    }

    /// The About page (a singleton tab): the app identity, the version, and where new versions
    /// come from. All of the page's actions live here in the body (the toolbar is a static strip
    /// shared by every tab).
    pub(crate) fn about_tab(&mut self, ui: &mut egui::Ui) {
        // ---- body: the About content, on the silvery data sheet with normal tab scrolling ----
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p().panel2)
                    .inner_margin(self.island_margin()),
            )
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
                                ui.label(
                                    RichText::new(format!("Version {}", brand::CURRENT_VERSION))
                                        .color(p().text),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    RichText::new(
                                        "A native PostgreSQL IDE for Windows, in Rust + egui.",
                                    )
                                    .color(p().text_dim),
                                );
                                ui.add_space(2.0);
                                ui.label(
                                    RichText::new(
                                        "Fonts: JetBrains Mono (OFL) · JustQuery icon set",
                                    )
                                    .color(p().text_dim)
                                    .size(crate::LABEL_SIZE),
                                );
                                ui.add_space(SPACE_4);
                                ui.separator();
                                ui.add_space(SPACE_3);
                                // new versions: just download the exe — no in-app updater
                                ui.label(
                                    RichText::new(
                                        "New versions ship as the same single exe — download the \
                                         latest release and replace this file:",
                                    )
                                    .color(p().text_dim),
                                );
                                ui.hyperlink_to(
                                    "github.com/votinvv/justquery/releases/latest",
                                    brand::RELEASES_URL,
                                );
                            });
                    });
            });
    }
}
