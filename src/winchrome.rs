//!
//! Custom window chrome (decorations = false): the caption bar plumbing, window border and
//! resize handles we have to draw ourselves since the OS frame is off. The painted-widget
//! library proper lives in [`crate::widgets`].

use crate::theme::p;
use crate::widgets::crisp_border_r;
use eframe::egui;
use egui::{Color32, CornerRadius, Stroke, Vec2};

/// Empty areas of the current panel drag the window; double-click toggles maximize.
pub fn enable_window_drag(ui: &mut egui::Ui, ctx: &egui::Context) {
    let id = ui.id().with("window_drag");
    let resp = ui.interact(ui.max_rect(), id, egui::Sense::click_and_drag());
    if resp.double_clicked() {
        let maximized = ui.input(|i| i.viewport().maximized).unwrap_or(false);
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    } else if resp.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}

/// Caption frame: warm fill, left padding only (controls sit flush in the top-right corner).
pub fn caption_frame() -> egui::Frame {
    egui::Frame::new().fill(p().panel2).inner_margin(egui::Margin {
        left: 8,
        right: 0,
        top: 0,
        bottom: 0,
    })
}

#[derive(Clone, Copy)]
enum Cap {
    Min,
    Max,
    Close,
}

/// Standard Windows window buttons (call inside a right_to_left layout).
pub fn caption_buttons(ui: &mut egui::Ui, ctx: &egui::Context) {
    let maximized = ui.input(|i| i.viewport().maximized).unwrap_or(false);
    caption_button(ui, ctx, Cap::Close, maximized);
    caption_button(ui, ctx, Cap::Max, maximized);
    caption_button(ui, ctx, Cap::Min, maximized);
}

fn caption_button(ui: &mut egui::Ui, ctx: &egui::Context, kind: Cap, maximized: bool) {
    let size = Vec2::new(40.0, ui.max_rect().height());
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let hovered = resp.hovered();
    let hover_bg = p().acc_bg; // neutral silvery hover (was a warm coral-era tint)
    if hovered {
        let bg = match kind {
            Cap::Close => Color32::from_rgb(0xc4, 0x2b, 0x1c), // standard Win11 red
            _ => hover_bg,
        };
        ui.painter().rect_filled(rect, CornerRadius::ZERO, bg);
    }
    let col = if matches!(kind, Cap::Close) && hovered {
        Color32::WHITE
    } else {
        p().text
    };
    let stroke = Stroke::new(1.2, col);
    let c = rect.center();
    let s = 4.0;
    let pt = ui.painter();
    match kind {
        Cap::Min => {
            pt.line_segment([egui::pos2(c.x - s, c.y), egui::pos2(c.x + s, c.y)], stroke);
        }
        Cap::Max => {
            if maximized {
                // restore: two overlapping squares (smaller than the maximize square)
                let rs = 3.0;
                let o = 1.5;
                let back = egui::Rect::from_min_size(
                    egui::pos2(c.x - rs + o, c.y - rs - o),
                    Vec2::new(2.0 * rs, 2.0 * rs),
                );
                let front = egui::Rect::from_min_size(
                    egui::pos2(c.x - rs - o, c.y - rs + o),
                    Vec2::new(2.0 * rs, 2.0 * rs),
                );
                pt.rect_stroke(back, CornerRadius::ZERO, stroke, egui::StrokeKind::Middle);
                pt.rect_filled(front, CornerRadius::ZERO, if hovered { hover_bg } else { p().panel2 });
                pt.rect_stroke(front, CornerRadius::ZERO, stroke, egui::StrokeKind::Middle);
            } else {
                pt.rect_stroke(
                    egui::Rect::from_center_size(c, Vec2::new(2.0 * s, 2.0 * s)),
                    CornerRadius::ZERO,
                    stroke,
                    egui::StrokeKind::Middle,
                );
            }
        }
        Cap::Close => {
            pt.line_segment([egui::pos2(c.x - s, c.y - s), egui::pos2(c.x + s, c.y + s)], stroke);
            pt.line_segment([egui::pos2(c.x - s, c.y + s), egui::pos2(c.x + s, c.y - s)], stroke);
        }
    }
    if resp.clicked() {
        match kind {
            Cap::Min => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            Cap::Max => ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized)),
            Cap::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }
    let tip = match kind {
        Cap::Min => "Minimize",
        Cap::Max => {
            if maximized {
                "Restore"
            } else {
                "Maximize"
            }
        }
        Cap::Close => "Close",
    };
    resp.on_hover_text(tip);
}

/// 1px outline around the window when it is not maximized (own border, since OS chrome is off).
pub fn window_border(ctx: &egui::Context) {
    if ctx.input(|i| i.viewport().maximized).unwrap_or(false) {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("window_border"),
    ));
    // crisp pixel-snapped 1px outline. The window outline stays SQUARE (radius 0): the custom
    // chrome + resize hit-testing assume square corners.
    crisp_border_r(&painter, ctx.content_rect(), p().border_strong, 0);
}

/// Invisible strips along the window edges for resizing (suppressed while maximized).
/// The top is left alone (window buttons live there); we resize from the left/right/bottom
/// edges and the two bottom corners, and paint a small grip in the bottom-right corner.
pub fn resize_handles(ctx: &egui::Context) {
    use egui::viewport::ResizeDirection as Dir;
    use egui::{pos2, Area, CursorIcon, Id, Order, Rect, Sense, ViewportCommand};

    if ctx.input(|i| i.viewport().maximized).unwrap_or(false) {
        return;
    }
    let r = ctx.content_rect();
    let t = 5.0_f32;
    // The right edge is left to the editor's vertical scrollbar — width is dragged from corners.
    let specs: [(&str, Rect, Dir, CursorIcon); 6] = [
        (
            "rz_n",
            Rect::from_min_max(pos2(r.left() + t, r.top()), pos2(r.right() - t, r.top() + t)),
            Dir::North,
            CursorIcon::ResizeVertical,
        ),
        (
            "rz_w",
            Rect::from_min_max(pos2(r.left(), r.top() + t), pos2(r.left() + t, r.bottom() - t)),
            Dir::West,
            CursorIcon::ResizeHorizontal,
        ),
        (
            "rz_e",
            Rect::from_min_max(pos2(r.right() - t, r.top() + t), pos2(r.right(), r.bottom() - t)),
            Dir::East,
            CursorIcon::ResizeHorizontal,
        ),
        (
            "rz_s",
            Rect::from_min_max(pos2(r.left() + t, r.bottom() - t), pos2(r.right() - t, r.bottom())),
            Dir::South,
            CursorIcon::ResizeVertical,
        ),
        (
            "rz_sw",
            Rect::from_min_max(pos2(r.left(), r.bottom() - t), pos2(r.left() + t, r.bottom())),
            Dir::SouthWest,
            CursorIcon::ResizeNeSw,
        ),
        (
            "rz_se",
            Rect::from_min_max(pos2(r.right() - t, r.bottom() - t), r.right_bottom()),
            Dir::SouthEast,
            CursorIcon::ResizeNwSe,
        ),
    ];
    for (id, rect, dir, cursor) in specs {
        Area::new(Id::new(id))
            .order(Order::Foreground)
            .fixed_pos(rect.min)
            .show(ctx, |ui| {
                let resp = ui.allocate_response(rect.size(), Sense::drag());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(cursor);
                }
                if resp.drag_started() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(dir));
                }
            });
    }

    // visible resize grip in the bottom-right corner
    let grip = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("rz_grip")));
    for k in 0..3 {
        let o = 4.0 + k as f32 * 4.0;
        grip.line_segment(
            [pos2(r.right() - o, r.bottom() - 2.0), pos2(r.right() - 2.0, r.bottom() - o)],
            Stroke::new(1.0, p().text_dim),
        );
    }
}
