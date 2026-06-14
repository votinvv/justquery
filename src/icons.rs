//!
//! The JustQuery icon set — 29 author-drawn glyphs compiled into
//! `assets/justquery-icons.ttf` (24×24 grid, 1.8 stroke, round caps/joins;
//! `run`/`stop` are the only filled glyphs).
//! Codepoints are FIXED at U+E900..U+E91C — rebuilds never reshuffle them.
#![allow(dead_code)] // the full map stays addressable even before every glyph has a call site

pub const NEW_QUERY: &str = "\u{e900}";
pub const OPEN: &str = "\u{e901}";
pub const SAVE: &str = "\u{e902}";
pub const DATABASE: &str = "\u{e903}";
pub const SCHEMA: &str = "\u{e904}";
pub const TABLE: &str = "\u{e905}";
pub const VIEW: &str = "\u{e906}";
pub const SEQUENCE: &str = "\u{e907}";
pub const FUNCTION: &str = "\u{e908}";
pub const TRIGGER: &str = "\u{e909}";
pub const RUN: &str = "\u{e90a}";
pub const STOP: &str = "\u{e90b}";
pub const COMMIT: &str = "\u{e90c}";
pub const ROLLBACK: &str = "\u{e90d}";
pub const FORMAT: &str = "\u{e90e}";
pub const FIND: &str = "\u{e90f}";
pub const PANEL_LEFT: &str = "\u{e910}";
pub const PANEL_TREE: &str = "\u{e911}";
pub const CLOSE: &str = "\u{e912}";
pub const CHEVRON_RIGHT: &str = "\u{e913}";
pub const CHEVRON_DOWN: &str = "\u{e914}";
pub const CHEVRONS_UP: &str = "\u{e915}";
pub const CHEVRONS_DOWN: &str = "\u{e916}";
pub const PLUS: &str = "\u{e917}";
pub const TRASH: &str = "\u{e918}";
pub const REFRESH: &str = "\u{e919}";
pub const PLUG: &str = "\u{e91a}";
pub const PLUG_OFF: &str = "\u{e91b}";
pub const KEY: &str = "\u{e91c}";

// ---- vector-drawn glyphs ----------------------------------------------------------------------
// Icons we draw by hand with the painter (not shipped in the TTF). Each fills the given `rect`
// (the button's centred glyph square) in `color`, matching the set's ~1.6 stroke / round look.
use eframe::egui;
use egui::{Color32, Pos2, Rect, Shape, Stroke};

/// Point at fractional (fx, fy) inside `rect` — (0,0) top-left … (1,1) bottom-right.
fn at(rect: Rect, fx: f32, fy: f32) -> Pos2 {
    egui::pos2(rect.left() + fx * rect.width(), rect.top() + fy * rect.height())
}

/// `{..}` — Refact & Format: two curly braces with two dots between them.
pub fn draw_format(painter: &egui::Painter, rect: Rect, color: Color32) {
    let st = Stroke::new(1.4, color);
    painter.add(Shape::line(
        vec![
            at(rect, 0.30, 0.08), at(rect, 0.18, 0.16), at(rect, 0.18, 0.42),
            at(rect, 0.07, 0.50), at(rect, 0.18, 0.58), at(rect, 0.18, 0.84),
            at(rect, 0.30, 0.92),
        ],
        st,
    ));
    painter.add(Shape::line(
        vec![
            at(rect, 0.70, 0.08), at(rect, 0.82, 0.16), at(rect, 0.82, 0.42),
            at(rect, 0.93, 0.50), at(rect, 0.82, 0.58), at(rect, 0.82, 0.84),
            at(rect, 0.70, 0.92),
        ],
        st,
    ));
    let dr = rect.width() * 0.058;
    painter.circle_filled(at(rect, 0.42, 0.52), dr, color);
    painter.circle_filled(at(rect, 0.58, 0.52), dr, color);
}

/// ✓ — Validate: a checkmark.
pub fn draw_check(painter: &egui::Painter, rect: Rect, color: Color32) {
    painter.add(Shape::line(
        vec![at(rect, 0.14, 0.52), at(rect, 0.40, 0.80), at(rect, 0.86, 0.20)],
        Stroke::new(1.7, color),
    ));
}

/// ⚡ — Stop: a filled lightning bolt (one of the set's filled glyphs, like run/stop).
pub fn draw_stop(painter: &egui::Painter, rect: Rect, color: Color32) {
    let pts = vec![
        at(rect, 0.58, 0.04), at(rect, 0.22, 0.54), at(rect, 0.46, 0.54),
        at(rect, 0.40, 0.96), at(rect, 0.78, 0.42), at(rect, 0.52, 0.42),
    ];
    painter.add(Shape::Path(egui::epaint::PathShape {
        points: pts,
        closed: true,
        fill: color,
        stroke: Stroke::NONE.into(),
    }));
}
