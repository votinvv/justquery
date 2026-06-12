//! In-editor text search: the search logic and the top "find" bar. All methods hang off
//! [`crate::JustQueryApp`]. Ищет построчно по [`crate::doc::Document`]; все совпадения
//! подсвечиваются через `search_hl` SHARED-редактора, текущее — выделяется.

use crate::doc::Pos;
use crate::widgets::close_x;
use crate::theme::p;
use crate::{JustQueryApp, TabDoc};
use eframe::egui;
use egui::{Align, Margin, CornerRadius, Stroke, Vec2};

/// Потолок числа совпадений за один поиск (защита от «найти 'a' в гигантском дампе»).
const MAX_MATCHES: usize = 100_000;

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
        // return focus to the editor so any current selection can be acted on right away (e.g.
        // Del deletes it) instead of the keystroke being swallowed by the (now-closed) find bar
        self.focus_editor = true;
        // keep find_query / find_match_start so Ctrl+. / Ctrl+, keep working and the selection persists
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
        let is_word = crate::codeeditor::is_word;
        let needle: Vec<char> = query.chars().collect();
        let m = needle.len();

        let idx = self.active_tab;
        let reset = |s: &mut Self| {
            s.find_count = 0;
            s.find_index = 0;
            s.find_match_start = None;
        };
        let mut matches: Vec<Pos> = Vec::new();
        {
            let Some(t) = self.tabs.get_mut(idx).filter(|t| t.is_sql()) else {
                return reset(self);
            };
            let TabDoc::Ready(doc) = &mut t.doc else { return reset(self) };
            if m > 0 {
                let nl = doc.line_count();
                'scan: for line in 0..nl {
                    let chars: Vec<char> = doc.get_line(line).chars().collect();
                    let n = chars.len();
                    if m > n {
                        continue;
                    }
                    for i in 0..=(n - m) {
                        let hit = (0..m).all(|k| fold(chars[i + k]) == fold(needle[k]));
                        let word_ok = !whole
                            || ((i == 0 || !is_word(chars[i - 1]))
                                && (i + m == n || !is_word(chars[i + m])));
                        if hit && word_ok {
                            matches.push((line, i));
                            if matches.len() >= MAX_MATCHES {
                                break 'scan;
                            }
                        }
                    }
                }
            }
        }
        self.find_count = matches.len();
        if matches.is_empty() {
            reset(self);
            if let Some(t) = self.tabs.get_mut(idx) {
                t.search_hl.clear(); // подсветка прежнего запроса устарела
            }
            return;
        }
        let wrap = self.find_wrap;
        let pick = match self.find_match_start {
            // fresh search → the match nearest to the caret (по строке, затем по колонке)
            None => {
                let cl = self.cursor_ln.saturating_sub(1) as i64;
                let cc = self.cursor_col.saturating_sub(1) as i64;
                matches
                    .iter()
                    .copied()
                    .min_by_key(|&(l, c)| ((l as i64 - cl).abs(), (c as i64 - cc).abs()))
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
        if let Some(t) = self.tabs.get_mut(idx) {
            // подсветить ВСЕ совпадения + выделить текущее (прыжок на следующем кадре)
            t.search_hl.clear();
            for &(l, c) in &matches {
                t.search_hl.entry(l).or_default().push((c, m));
            }
            t.pending_goto = Some((pick, (pick.0, pick.1 + m)));
        }
    }

    /// Simple find bar (current scenario): magnetised to the top edge with a small gap from the
    /// right. Just a field + a "Find" button. Searches only on Enter / Find, jumps to the match
    /// nearest the caret and selects it. Ctrl+. / Ctrl+, step next/previous (F3 is not bound).
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
        if self.cur().is_some_and(|t| t.pending_goto.is_some()) {
            ctx.request_repaint();
        }
    }
}
