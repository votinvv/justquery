//! In-editor text search: the search logic and the top "find" bar. All methods hang off
//! [`crate::JustQueryApp`].

use crate::widgets::close_x;
use crate::theme::p;
use crate::JustQueryApp;
use eframe::egui;
use egui::{Align, Margin, CornerRadius, Stroke, Vec2};

impl JustQueryApp {
    pub(crate) fn open_find(&mut self) {
        if self.tabs.is_empty() || !self.is_sql_tab() {
            return;
        }
        self.find_open = true;
        self.find_focus = true;
        self.find_match_start = None;
        if !self.find_query.is_empty() {
            self.find_run(false);
        }
    }

    pub(crate) fn close_find(&mut self) {
        self.find_open = false;
        self.pending_find = None;
        // return focus to the editor so any current selection can be acted on right away (e.g.
        // Del deletes it) instead of the keystroke being swallowed by the (now-closed) find bar
        self.focus_editor = true;
        // keep find_query / find_match_start so F3 keeps working and the selection persists
    }

    /// Move to the next/previous match and select it in the (focused) editor.
    pub(crate) fn find_step(&mut self, backward: bool) {
        self.find_run(backward);
        if self.find_match_start.is_some() {
            self.focus_editor = true;
        }
    }

    /// Recompute matches and move to the next (or previous) one relative to the current match
    /// (or the editor caret when there is no current match yet).
    pub(crate) fn find_run(&mut self, backward: bool) {
        let query = self.find_query.clone();
        let text = match self.cur() {
            Some(t) => t.sql.clone(),
            None => {
                self.find_count = 0;
                self.find_index = 0;
                self.pending_find = None;
                return;
            }
        };
        let case = self.find_case;
        let whole = self.find_whole_word;
        // Unicode-aware case fold (to_ascii_lowercase ignores Cyrillic etc.)
        let fold = |c: char| {
            if case {
                c
            } else {
                c.to_lowercase().next().unwrap_or(c)
            }
        };
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let hay: Vec<char> = text.chars().collect();
        let needle: Vec<char> = query.chars().collect();
        let (n, m) = (hay.len(), needle.len());
        if m == 0 || m > n {
            self.find_count = 0;
            self.find_index = 0;
            self.find_match_start = None;
            self.pending_find = None;
            return;
        }
        let mut matches: Vec<usize> = Vec::new();
        let mut i = 0;
        while i + m <= n {
            let hit = (0..m).all(|k| fold(hay[i + k]) == fold(needle[k]));
            let word_ok = !whole
                || ((i == 0 || !is_word(hay[i - 1])) && (i + m == n || !is_word(hay[i + m])));
            if hit && word_ok {
                matches.push(i);
            }
            i += 1;
        }
        self.find_count = matches.len();
        if matches.is_empty() {
            self.find_index = 0;
            self.find_match_start = None;
            self.pending_find = None;
            return;
        }
        let wrap = self.find_wrap;
        let pick = match self.find_match_start {
            // fresh search → the match nearest to the caret
            None => {
                let caret = self.caret.min(n) as i64;
                matches.iter().copied().min_by_key(|&s| (s as i64 - caret).abs())
            }
            // navigating → the next / previous relative to the current match
            Some(cur) => {
                if backward {
                    matches
                        .iter()
                        .rev()
                        .copied()
                        .find(|&s| s < cur)
                        .or_else(|| wrap.then(|| *matches.last().unwrap()))
                } else {
                    matches
                        .iter()
                        .copied()
                        .find(|&s| s > cur)
                        .or_else(|| wrap.then(|| matches[0]))
                }
            }
        };
        let Some(pick) = pick else {
            // no match in this direction and wrap is off — keep the current one
            return;
        };
        self.find_match_start = Some(pick);
        self.find_index = matches.iter().position(|&s| s == pick).map_or(0, |p| p + 1);
        self.pending_find = Some((pick, pick + m));
    }

    /// Simple find bar (current scenario): magnetised to the top edge with a small gap from the
    /// right. Just a field + a "Find" button. Searches only on Enter / Find, jumps to the match
    /// nearest the caret and selects it. Ctrl+. / Ctrl+, (and F3 / Shift+F3) step next/previous.
    pub(crate) fn find_bar(&mut self, ctx: &egui::Context) {
        if !self.find_open {
            return;
        }
        // float it as its own bordered box, equally inset (~10px) from the top and right edges of
        // the work area, so it no longer overlaps the work-area border
        let y = self.editor_rect.top() + 10.0;
        let mut commit = false;
        let mut close = false;
        egui::Area::new(egui::Id::new("find_bar"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, Vec2::new(-16.0, y))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(p().ivory) // raised studio sheet, not chrome (state 09)
                    .stroke(Stroke::new(1.0, p().border_strong))
                    .corner_radius(CornerRadius::same(crate::RADIUS_ISLAND))
                    .shadow(crate::theme::island_shadow())
                    .inner_margin(Margin::symmetric(6, 5))
                    .show(ui, |ui| {
                        ui.style_mut().visuals.override_text_color = None;
                        ui.spacing_mut().item_spacing.x = 4.0;
                        // Systemic row sizing: pick ONE panel height = text row height + 2×pad, then
                        // make every element exactly that tall. The field fills it via a matching
                        // vertical margin (a TextEdit frame is content-height, so add_sized wouldn't
                        // stretch it), the icon button is a square of the same height, and close_x
                        // reads the row height. Zero the widget expansion so the button box doesn't
                        // bulge past h on hover.
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
                                        .hint_text("Find")
                                        .vertical_align(Align::Center),
                                );
                                // the field's actual rendered box — the alignment reference
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
                                // the search trigger — a square magnifier button drawn by hand so its
                                // box is pixel-exact to the field's top/bottom (an egui Button sized
                                // separately rounds ~1px off the field on fractional-DPI displays)
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
                                    p().text, // neutral hover: the box is the affordance, no accent glyph
                                );
                                if b_resp.clicked() {
                                    commit = true;
                                }
                                b_resp.on_hover_text("Find (Enter)");
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
            // fresh search → nearest match to the caret, select it, then close the dialog
            self.find_match_start = None;
            self.find_run(false);
            if self.find_match_start.is_some() {
                self.focus_editor = true;
            }
            self.find_open = false;
        }
        if self.pending_find.is_some() {
            ctx.request_repaint();
        }
    }
}
