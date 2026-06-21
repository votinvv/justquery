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

/// `{..}` — Format: two curly braces with two dots between them.
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

/// 💾+ — Save As: a save glyph with a small «+» badge in the corner (same 24×24 grid, 1.4 stroke as
/// the other hand-drawn glyphs). Drawn rather than font-backed because the icon TTF has no save-as
/// codepoint, and rebuilding it needs the Node toolchain (icons/README §сборка).
pub fn draw_save_as(painter: &egui::Painter, rect: Rect, color: Color32) {
    let st = Stroke::new(1.4, color);
    // floppy outline: M(0.22,0.08) → top-right corner cut → right edge → bottom-right → bottom-left
    // → top-left → close, matching icons/save.svg at 1/4 inset.
    painter.add(Shape::line(
        vec![
            at(rect, 0.22, 0.08), at(rect, 0.64, 0.08), at(rect, 0.92, 0.36),
            at(rect, 0.92, 0.88), at(rect, 0.22, 0.88),
        ],
        st,
    ));
    painter.add(Shape::line(
        vec![at(rect, 0.22, 0.08), at(rect, 0.22, 0.88)],
        st,
    ));
    // the «label» slot near the top
    painter.add(Shape::line(
        vec![at(rect, 0.34, 0.08), at(rect, 0.34, 0.34), at(rect, 0.62, 0.34), at(rect, 0.62, 0.08)],
        st,
    ));
    // «+» badge bottom-right
    let bx = 0.80;
    let by = 0.74;
    let bh = 0.10;
    painter.add(Shape::line(
        vec![at(rect, bx - bh, by), at(rect, bx + bh, by)],
        st,
    ));
    painter.add(Shape::line(
        vec![at(rect, bx, by - bh), at(rect, bx, by + bh)],
        st,
    ));
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

/// ✕ — a close/cross glyph: two diagonal segments through `center`, each arm `half` long.
/// The single source for every "×" in the app (modal close, tab close, caption close).
pub fn paint_cross(painter: &egui::Painter, center: Pos2, half: f32, stroke: Stroke) {
    painter.line_segment(
        [egui::pos2(center.x - half, center.y - half), egui::pos2(center.x + half, center.y + half)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(center.x - half, center.y + half), egui::pos2(center.x + half, center.y - half)],
        stroke,
    );
}
