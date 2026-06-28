//! Search entry: the top "find" bar. Ctrl+F (or the menu) opens it; Enter (or the magnifier)
//! launches a BACKGROUND search over the document snapshot — matches stream into the results grid
//! (see [`crate::proc`] / `start_search`), the same engine for SQL and XML tabs. While the search
//! runs the tab is read-only and other processes are blocked (one process per tab).

use crate::theme::p;
use crate::widgets::close_x;
use crate::JustQueryApp;
use eframe::egui;
use egui::{Align, CornerRadius, Margin, Stroke, Vec2};

impl JustQueryApp {
    pub(crate) fn open_find(&mut self) {
        if !self.is_editor_tab() {
            return;
        }
        self.find_open = true;
        self.find_focus = true;
    }

    pub(crate) fn close_find(&mut self) {
        self.find_open = false;
        // return focus to the editor so the next keystroke acts on the buffer, not the closed bar
        self.focus_editor = true;
    }

    /// The find bar: a field + magnifier, floated top-right of the work area. Enter / click runs the
    /// background search (results stream into the grid) and closes the bar.
    pub(crate) fn find_bar(&mut self, ctx: &egui::Context) {
        if !self.find_open {
            return;
        }
        let y = self.editor_rect.top() + 10.0;
        let mut commit = false;
        let mut close = false;
        egui::Area::new(egui::Id::new("find_bar"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, y))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(p().ivory)
                    .stroke(Stroke::new(1.0 / ui.ctx().pixels_per_point(), p().border_strong)) // crisp 1 device px
                    .corner_radius(CornerRadius::same(crate::RADIUS_ISLAND))
                    .shadow(crate::theme::island_shadow())
                    .inner_margin(Margin::symmetric(6, 5))
                    .show(ui, |ui| {
                        ui.style_mut().visuals.override_text_color = None;
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let pad: i8 = 5;
                        let row_h = ui.text_style_height(&egui::TextStyle::Body);
                        let h = row_h + 2.0 * pad as f32;
                        ui.visuals_mut().widgets.inactive.expansion = 0.0;
                        ui.visuals_mut().widgets.hovered.expansion = 0.0;
                        ui.visuals_mut().widgets.active.expansion = 0.0;
                        let row_w = 190.0 + 12.0 + 4.0 + h + 4.0 + 22.0;
                        ui.allocate_ui_with_layout(
                            Vec2::new(row_w, h),
                            egui::Layout::left_to_right(Align::Center),
                            |ui| {
                                let resp = ui.add(
                                    egui::TextEdit::singleline(&mut self.find_query)
                                        .desired_width(190.0)
                                        .margin(egui::Margin {
                                            left: 6,
                                            right: 6,
                                            top: pad,
                                            bottom: pad,
                                        })
                                        .hint_text("Search")
                                        .vertical_align(Align::Center),
                                );
                                let fr = resp.rect;
                                if self.find_focus {
                                    resp.request_focus();
                                    self.find_focus = false;
                                }
                                if resp.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    commit = true;
                                }
                                // hand-drawn magnifier button, pixel-exact to the field box
                                let (hit, b_resp) = ui.allocate_exact_size(
                                    Vec2::new(fr.height(), fr.height()),
                                    egui::Sense::click(),
                                );
                                let box_rect = egui::Rect::from_min_max(
                                    egui::pos2(hit.left(), fr.top()),
                                    egui::pos2(hit.right(), fr.bottom()),
                                );
                                let vis = ui.style().interact(&b_resp);
                                ui.painter().rect_filled(
                                    box_rect,
                                    CornerRadius::ZERO,
                                    vis.weak_bg_fill,
                                );
                                if vis.bg_stroke.width > 0.0 {
                                    ui.painter().rect_stroke(
                                        box_rect,
                                        CornerRadius::ZERO,
                                        vis.bg_stroke,
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                ui.painter().text(
                                    box_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    crate::ic::SEARCH,
                                    egui::FontId::proportional(15.0),
                                    p().text,
                                );
                                if b_resp.clicked() {
                                    commit = true;
                                }
                                b_resp.on_hover_text("Search (Enter)");
                                if close_x(ui, "Close (Esc)") {
                                    close = true;
                                }
                            },
                        );
                    });
            });
        if close {
            self.close_find();
        }
        if commit {
            let q = self.find_query.clone();
            self.start_search(q); // background search → result grid; the tab is read-only meanwhile
            self.find_open = false;
            self.focus_editor = true;
        }
    }
}
