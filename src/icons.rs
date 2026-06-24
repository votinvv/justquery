//!
//! The JustQuery icon set — 30 glyphs based on **Ionicons** (outline / thin lines,
//! MIT) compiled into `assets/justquery-icons.ttf`
//! (512×512 grid, ~1.5px stroke expanded to outlines; every glyph is a thin line).
//! Codepoints are FIXED at U+E900..U+E91D — rebuilds never reshuffle them.
//! Source SVGs live in `icons/*.svg`.
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
pub const CHECK: &str = "\u{e91d}"; // Inspect / Validate
pub const SAVE_AS: &str = "\u{e91e}"; // Save As — `save` floppy + a small «+» badge (icons/save-as.svg)

// ---- vector-drawn glyphs ----------------------------------------------------------------------
// Every command/toolbar glyph is font-backed now (the `icons::*` codepoints above). The only shape
// still drawn in code is the close "×" (`paint_cross`): the icon TTF has no cross codepoint, and a
// single source keeps every modal/tab/caption close identical.
use eframe::egui;
use egui::{Pos2, Stroke};

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
