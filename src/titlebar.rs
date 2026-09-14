//! The window caption bar: logo, the icon toolbar (pulled up into the caption when the text
//! menus went away — ONE chrome row instead of two), the centered active-tab title and the
//! window buttons. Pure chrome content — screen-level layout stays in `main.rs`.

use crate::theme::p;
use crate::winchrome::{caption_buttons, caption_frame, enable_window_drag};
use crate::JustQueryApp;
use crate::{CAPTION_H, ICON_GAP};
use eframe::egui;
use egui::{Align, Layout, Vec2};

impl JustQueryApp {
    /// Window caption: logo + the icon toolbar + the centered active-tab title + the window
    /// buttons — one chrome row (the text menus are gone; every verb lives in the toolbar).
    pub(crate) fn titlebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::top("titlebar")
            .frame(caption_frame())
            .exact_size(CAPTION_H)
            .show_separator_line(false)
            .show(ui, |ui| {
                // empty caption areas drag the window; double-click (un)maximizes
                enable_window_drag(ui, ctx);
                let full = ui.max_rect();
                let mut strip_end = full.left() + 220.0;
                let controls_w = 3.0 * 40.0; // close / max / minimize
                ui.horizontal_centered(|ui| {
                    // ONE rhythm for the whole row: logo → first icon → … → window buttons all
                    // step with ICON_GAP. The logo occupies the SAME square slot as an icon
                    // button (row height, 18px glyph centered) — it reads as one more button,
                    // not a foreign glyph glued to the strip.
                    ui.spacing_mut().item_spacing.x = ICON_GAP;
                    let lh = ui.available_height();
                    let (rect, _) =
                        ui.allocate_exact_size(egui::Vec2::new(lh, lh), egui::Sense::hover());
                    let inner =
                        egui::Rect::from_center_size(rect.center(), egui::Vec2::new(18.0, 18.0));
                    crate::brand::paint_logo(ui.painter(), inner, 1.0);
                    // The toolbar strip lives in a width-capped, clipped child so the window
                    // buttons ALWAYS keep their zone on the right — in a narrow window the strip
                    // clips instead of pushing Min/Max/Close out of the caption.
                    let zone = (ui.available_width() - controls_w - 4.0).max(0.0);
                    let row_h = ui.available_height();
                    strip_end = ui
                        .allocate_ui_with_layout(
                            Vec2::new(zone, row_h),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                                self.icon_toolbar(ui, ctx);
                                ui.min_rect().right()
                            },
                        )
                        .inner;
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        caption_buttons(ui, ctx);
                    });
                });

                // window "title": the active tab's name, centered between the toolbar strip and
                // the window buttons (empty when no tab is open). In a narrow window the zone
                // shrinks — the title ellipsizes, then disappears, never overlapping the icons.
                let zone_left = strip_end + 12.0;
                let zone_right = full.right() - controls_w - 8.0;
                let avail = zone_right - zone_left;
                let cx = (zone_left + zone_right) * 0.5;
                let cy = full.center().y;
                if avail > 24.0 {
                    if let Some(title) =
                        self.cur().map(|t| t.title.clone()).filter(|s| !s.is_empty())
                    {
                        let shown = crate::widgets::truncate_to_width(ui, &title, 13.0, avail);
                        ui.painter().text(
                            egui::pos2(cx, cy),
                            egui::Align2::CENTER_CENTER,
                            shown,
                            egui::FontId::proportional(crate::theme::BODY_SIZE),
                            p().text_dim,
                        );
                    }
                }
            });
    }
}
