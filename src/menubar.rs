//! The window caption bar: logo, the File/Edit/Search/Database/Tools/Window/Help text menus
//! (with the menu-bar roll-over hack), the centered active-tab title and the window buttons.
//! Pure chrome content — screen-level layout stays in `main.rs`.

use crate::brand::logo;
use crate::theme::p;
use crate::winchrome::{caption_buttons, caption_frame, enable_window_drag};
use crate::{dialog, save_theme, theme, JustQueryApp};
use crate::{CAPTION_H, DIAG_BOXES, RADIUS_CONTROL};
use eframe::egui;
use egui::{Align, Color32, CornerRadius, Layout, RichText, Stroke, Vec2};

impl JustQueryApp {
    /// Window caption: logo + text menus + centered connection string + window buttons.
    pub(crate) fn titlebar(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        egui::Panel::top("titlebar")
            .frame(caption_frame())
            .exact_size(CAPTION_H)
            .show_separator_line(false)
            .show_inside(ui, |ui| {
                // empty caption areas drag the window; double-click (un)maximizes
                enable_window_drag(ui, ctx);
                let full = ui.max_rect();

                // menu labels: text stays TEXT; the accent is a darker background box on
                // hover, kept (darker still) while the menu is open / pressed
                {
                    let s = ui.style_mut();
                    s.visuals.override_text_color = None;
                    let w = &mut s.visuals.widgets;
                    w.inactive.weak_bg_fill = if DIAG_BOXES { p().acc_bg } else { Color32::TRANSPARENT };
                    w.inactive.bg_stroke = Stroke::NONE;
                    w.inactive.fg_stroke = Stroke::new(1.0, p().text);
                    w.inactive.corner_radius = CornerRadius::ZERO;
                    w.hovered.weak_bg_fill = p().acc_bg;
                    w.hovered.bg_stroke = Stroke::NONE;
                    w.hovered.fg_stroke = Stroke::new(1.0, p().text);
                    w.hovered.corner_radius = CornerRadius::ZERO;
                    w.active.weak_bg_fill = p().acc_bg2;
                    w.active.bg_stroke = Stroke::NONE;
                    w.active.fg_stroke = Stroke::new(1.0, p().text);
                    w.active.corner_radius = CornerRadius::ZERO;
                    w.open.weak_bg_fill = p().acc_bg2;
                    w.open.bg_stroke = Stroke::NONE;
                    w.open.fg_stroke = Stroke::new(1.0, p().text);
                    w.open.corner_radius = CornerRadius::ZERO;
                }

                let mut menu_end = full.left() + 220.0;
                let controls_w = 3.0 * 40.0; // close / max / minimize
                ui.horizontal_centered(|ui| {
                    // tight menu row: small gap between items, 14px side padding. Hover-боксы ПЛОСКИЕ
                    // (заполняют высоту строки — см. interact_size ниже); 4px-воздух до тулбара даёт
                    // распорка-ряд (gap_below_caption), а не инсет бокса.
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.spacing_mut().button_padding = Vec2::new(14.0, 0.0);
                    logo(ui, 18.0);
                    ui.add_space(8.0);
                    // The menus live in a width-capped, clipped child so the window buttons
                    // ALWAYS keep their zone on the right — in a narrow window the menu row
                    // clips instead of pushing Min/Max/Close out of the caption.
                    let menu_zone = (ui.available_width() - controls_w - 4.0).max(0.0);
                    let row_h = ui.available_height();
                    menu_end = ui
                        .allocate_ui_with_layout(
                            Vec2::new(menu_zone, row_h),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                                // hover-бокс меню на всю высоту строки (плоский, как иконки тулбара)
                                ui.spacing_mut().interact_size.y = ui.available_height();
                            // one menu row: label + right-aligned shortcut; returns whether it was clicked
                            let item_en = |ui: &mut egui::Ui, label: &str, shortcut: &str, enabled: bool| -> bool {
                                let mut btn = egui::Button::new(label);
                                if !shortcut.is_empty() {
                                    btn = btn.shortcut_text(shortcut);
                                }
                                let clicked = ui.add_enabled(enabled, btn).clicked();
                                if clicked {
                                    ui.close();
                                }
                                clicked
                            };
                            let item = |ui: &mut egui::Ui, label: &str, shortcut: &str| -> bool {
                                item_en(ui, label, shortcut, true)
                            };
                            let can_save = self.can_save();
                            // collect each top-level button so we can implement menu-bar roll-over below
                            let mut menu_btns: Vec<(egui::Response, bool)> = Vec::new();
                            for m in ["File", "Edit", "Search", "Database", "Tools", "Window", "Help"] {
                                let mb = ui.menu_button(RichText::new(m).size(13.0), |ui| {
                                    // bigger, darker hover/active background for dropdown rows
                                    ui.spacing_mut().button_padding = Vec2::new(12.0, 6.0);
                                    ui.spacing_mut().item_spacing.y = 0.0; // tight rows; separators keep the logical blocks apart
                                    {
                                        // menu rows are neutral hover-pills, 7px radius (Design System v2 §6)
                                        let w = &mut ui.style_mut().visuals.widgets;
                                        w.hovered.weak_bg_fill = p().hover;
                                        w.hovered.bg_stroke = Stroke::NONE;
                                        w.hovered.corner_radius = CornerRadius::same(RADIUS_CONTROL);
                                        w.active.weak_bg_fill = p().acc_bg2;
                                        w.active.bg_stroke = Stroke::NONE;
                                        w.active.corner_radius = CornerRadius::same(RADIUS_CONTROL);
                                    }
                                    match m {
                                    "File" => {
                                        if item(ui, "New SQL Window", "Ctrl+N") {
                                            self.new_tab();
                                        }
                                        if item(ui, "Open SQL File…", "Ctrl+O") {
                                            self.open_file();
                                        }
                                        ui.separator();
                                        if item_en(ui, "Save", "Ctrl+S", can_save) {
                                            self.save_active();
                                        }
                                        if item_en(ui, "Save As…", "Ctrl+Shift+S", can_save) {
                                            self.save_active_as();
                                        }
                                        ui.separator();
                                        if item(ui, "Close Tab", "Ctrl+W") && !self.tabs.is_empty() {
                                            self.request_close_tab(self.active_tab);
                                        }
                                        ui.separator();
                                        if item(ui, "Exit", "Alt+F4") {
                                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                        }
                                    }
                                    "Edit" => {
                                        // dirty-флаг ведёт документ (undo до нуля снова «чистый»)
                                        if item(ui, "Undo", "Ctrl+Z") {
                                            if let Some(t) = self.ed_active_mut() {
                                                t.ed_undo();
                                            }
                                            self.focus_editor = true;
                                        }
                                        if item(ui, "Redo", "Ctrl+Shift+Z") {
                                            if let Some(t) = self.ed_active_mut() {
                                                t.ed_redo();
                                            }
                                            self.focus_editor = true;
                                        }
                                        ui.separator();
                                        if item(ui, "Cut", "Ctrl+X") {
                                            if let Some(s) =
                                                self.ed_active_mut().and_then(|t| t.ed_cut())
                                            {
                                                ctx.copy_text(s);
                                            }
                                            self.focus_editor = true;
                                        }
                                        if item(ui, "Copy", "Ctrl+C") {
                                            if let Some(s) =
                                                self.ed_active_mut().and_then(|t| t.ed_copy())
                                            {
                                                ctx.copy_text(s);
                                            }
                                        }
                                        if item(ui, "Paste", "Ctrl+V") {
                                            if let Some(txt) = dialog::clipboard_text() {
                                                if let Some(t) = self.ed_active_mut() {
                                                    t.ed_paste(&txt);
                                                }
                                                self.focus_editor = true;
                                            }
                                        }
                                        ui.separator();
                                        if item(ui, "Select All", "Ctrl+A") {
                                            self.editor_select_all();
                                        }
                                    }
                                    "Search" => {
                                        // Find… launches a background search → results grid
                                        // (one engine for SQL and XML tabs); a match clicked in the
                                        // grid jumps to it in the editor.
                                        if item(ui, "Find…", "Ctrl+F") {
                                            self.open_find();
                                        }
                                        item(ui, "Replace…", "Ctrl+H");
                                        ui.separator();
                                        item(ui, "Go to Line…", "Ctrl+G");
                                    }
                                    "Database" => {
                                        if item(ui, "Connect…", "") {
                                            self.open_connect();
                                        }
                                        if item(ui, "Disconnect", "") && self.connected {
                                            self.request_disconnect(); // never disconnect silently
                                        }
                                        ui.separator();
                                        item(ui, "Commit", "");
                                        item(ui, "Rollback", "");
                                    }
                                    "Tools" => {
                                        item_en(ui, "Execute", "F8", self.is_sql_tab());
                                        item_en(ui, "Stop", "Esc", self.cur().is_some_and(|t| t.exec_rx.is_some() || t.proc.is_some()));
                                        ui.separator();
                                        // Format/Inspect: XML today; SQL Refact/Inspect parked
                                        // (placeholders). Same keys as the toolbar buttons.
                                        item_en(ui, "Format", "F9", self.is_xml_tab());
                                        item_en(ui, "Inspect", "F5", self.is_xml_tab() && self.cur().is_some_and(|t| t.model_id.is_some()));
                                        ui.separator();
                                        item(ui, "Export Result…", "");
                                        ui.separator();
                                        // Appearance: Light / Dark radio pair (the check marks the active one)
                                        let cur = theme::current_theme();
                                        let mut switch_to: Option<theme::AppTheme> = None;
                                        ui.menu_button("Appearance", |ui| {
                                            ui.spacing_mut().button_padding = Vec2::new(12.0, 6.0);
                                            ui.spacing_mut().item_spacing.y = 0.0;
                                            let mut pick = |ui: &mut egui::Ui, label, t: theme::AppTheme| {
                                                let mark = if cur == t { "●" } else { " " };
                                                if item(ui, label, mark) && cur != t {
                                                    switch_to = Some(t);
                                                }
                                            };
                                            pick(ui, "Light", theme::AppTheme::Light);
                                            pick(ui, "Dark", theme::AppTheme::Dark);
                                        });
                                        if let Some(t) = switch_to {
                                            theme::set_theme(ctx, t);
                                            save_theme(t);
                                            // NOTE: do NOT clear line_cache here — the editor repaints
                                            // (and re-caches against the old font atlas) later this same
                                            // frame. update_inner drops the cache on the next frame,
                                            // after egui has rebuilt the atlas for the new theme.
                                        }
                                        ui.separator();
                                        item(ui, "Preferences…", "");
                                    }
                                    "Window" => {
                                        item(ui, "Next Tab", "Ctrl+Tab");
                                        item(ui, "Previous Tab", "Ctrl+Shift+Tab");
                                        ui.separator();
                                        let has_panel =
                                            self.cur().is_some_and(|t| !t.panel.is_empty());
                                        if has_panel && item(ui, "Hide Result Panel", "F4") {
                                            if let Some(t) = self.cur_mut() {
                                                t.clear_panel();
                                                t.search_hl.clear();
                                            }
                                        }
                                        ui.separator();
                                        item(ui, "Close All Tabs", "");
                                    }
                                    "Help" => {
                                        item(ui, "Documentation", "F1");
                                        item(ui, "Keyboard Shortcuts", "");
                                        ui.separator();
                                        if item(ui, "About JustQuery", "") {
                                            self.open_about();
                                        }
                                    }
                                    _ => {}
                                    }
                                });
                                menu_btns.push((mb.response, mb.inner.is_some()));
                            }
                            // Menu-bar roll-over: egui's top-level MenuButton only opens on click. Once ANY
                            // menu is open, hovering a different top-level button switches to it (open_id
                            // opens that popup and closes the rest), so the menus behave like a desktop
                            // menu bar — drag across File/Edit/… and the dropdown follows the cursor.
                            if menu_btns.iter().any(|(_, open)| *open) {
                                for (resp, open) in &menu_btns {
                                    if !*open && resp.hovered() {
                                        egui::Popup::open_id(ctx, resp.id.with("popup"));
                                    }
                                }
                            }
                                ui.min_rect().right()
                            },
                        )
                        .inner;
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        caption_buttons(ui, ctx);
                    });
                });

                // window "title": the active tab's name, centered between the menu end and the
                // window buttons (empty when no tab is open). In a narrow window the zone
                // shrinks — the title ellipsizes, then disappears, never overlapping the menus.
                let zone_left = menu_end + 12.0;
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
