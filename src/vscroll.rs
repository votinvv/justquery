//!
//! Custom scrollbars for virtual scrolling in local coordinates.
//!
//! egui's ScrollArea builds one giant virtual canvas (rows × height), and all egui
//! coordinates are f32: across hundreds of millions of pixels the representable step
//! reaches 32px — rows overlap, empty rows appear, clicks miss. So the editor and grid
//! scroll themselves: the position is f64 pixels from the start of the content, only the
//! visible window is drawn, and large coordinates never exist. This module handles the
//! drawing and interaction of the bars themselves.

use crate::theme::p;
use eframe::egui;
use egui::{CornerRadius, Rect, Vec2};

/// Scrollbar thickness (solid, flush against the edge; the width is reserved while its bar is shown).
pub const BAR: f32 = 8.0;

/// Bar axis: vertical (scrolls Y) or horizontal (scrolls X).
#[derive(Clone, Copy)]
enum Axis {
    Vertical,
    Horizontal,
}

impl Axis {
    /// Track length along this axis (f64).
    #[inline]
    fn track_len(self, track: Rect) -> f64 {
        match self {
            Axis::Vertical => track.height() as f64,
            Axis::Horizontal => track.width() as f64,
        }
    }
    /// Pointer coordinate along this axis, relative to the track start (f32).
    #[inline]
    fn pointer(self, pp: egui::Pos2, track: Rect) -> f32 {
        match self {
            Axis::Vertical => pp.y - track.top(),
            Axis::Horizontal => pp.x - track.left(),
        }
    }
}

/// Scrollbar within the `track`. `offset` is in f64 pixels, clamped to `[0, content - view]`.
/// `view` is the true scrollable viewport length; it may be SHORTER than the track (the grid's overlay
/// bars run the full edge — over the header / the "#" gutter — so the handle size + travel come from
/// `view`/`content` while its position maps along the possibly-longer `track`; the editor passes
/// `view == track`). `alpha` (0..1) fades the handle — solid for the editor's reserved bars, animated
/// for the grid's disappearing overlay bars. Drag/click use absolute mapping (handle center to
/// pointer): predictable at any length. One rule for both vertical and horizontal bars (differ only in axis).
#[allow(clippy::too_many_arguments)]
fn bar(
    ui: &mut egui::Ui,
    track: Rect,
    id: egui::Id,
    offset: &mut f64,
    content: f64,
    view: f64,
    axis: Axis,
    alpha: f32,
) {
    let max_off = (content - view).max(0.0);
    *offset = offset.clamp(0.0, max_off);
    if max_off <= 0.0 {
        return; // fits entirely — no bar needed
    }
    let track_len = axis.track_len(track) as f32;
    let frac = (view / content).clamp(0.05, 1.0) as f32;
    let len = (track_len * frac).max(24.0).min(track_len);
    let range = (track_len - len).max(1.0);

    let resp = ui.interact(track, id, egui::Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(pp) = resp.interact_pointer_pos() {
            let t = ((axis.pointer(pp, track) - len * 0.5) / range).clamp(0.0, 1.0);
            *offset = t as f64 * max_off;
        }
    }
    let pos = ((*offset / max_off) as f32 * range).clamp(0.0, range);
    let handle = match axis {
        Axis::Vertical => Rect::from_min_size(
            egui::pos2(track.left(), track.top() + pos),
            Vec2::new(track.width(), len),
        ),
        Axis::Horizontal => Rect::from_min_size(
            egui::pos2(track.left() + pos, track.top()),
            Vec2::new(len, track.height()),
        ),
    };
    let color = if resp.dragged() {
        p().scroll_pressed
    } else if resp.hovered() {
        p().scroll_hot
    } else {
        p().scroll_dormant
    };
    ui.painter().rect_filled(handle, CornerRadius::same(4), color.gamma_multiply(alpha));
}

/// Vertical bar (scrolls `offset.y`). `view_h` is the scrollable viewport height (may differ from
/// the track height). `alpha` fades the handle (1 = solid). See [`bar`].
pub fn vbar(
    ui: &mut egui::Ui,
    track: Rect,
    id: egui::Id,
    offset: &mut f64,
    content_h: f64,
    view_h: f64,
    alpha: f32,
) {
    bar(ui, track, id, offset, content_h, view_h, Axis::Vertical, alpha);
}

/// Horizontal bar (scrolls `offset.x`). `view_w` is the scrollable viewport width (may differ from
/// the track width). `alpha` fades the handle (1 = solid). See [`bar`].
pub fn hbar(
    ui: &mut egui::Ui,
    track: Rect,
    id: egui::Id,
    offset: &mut f64,
    content_w: f64,
    view_w: f64,
    alpha: f32,
) {
    bar(ui, track, id, offset, content_w, view_w, Axis::Horizontal, alpha);
}

/// Wheel/touchpad scroll delta for this frame, if the pointer is over `rect`
/// (kinetic.rs injects the same wheel events — works transparently).
pub fn wheel_delta(ui: &egui::Ui, rect: Rect) -> Vec2 {
    if !ui.rect_contains_pointer(rect) {
        return Vec2::ZERO;
    }
    ui.ctx().input(|i| {
        let mut d = i.smooth_scroll_delta;
        // shift+wheel → horizontal (if egui hasn't already swapped the axes)
        if i.modifiers.shift && d.x == 0.0 {
            d = Vec2::new(d.y, 0.0);
        }
        d
    })
}

/// Fade state for a **disappearing overlay scrollbar**. Lives in the CONSUMER's own per-instance state
/// (next to its scroll offset), so each grid / editor tab fades independently — no shared id, no
/// cross-tab bleed. Call [`Self::alpha`] once per frame; it folds in this frame's activity and returns
/// the handle opacity to pass to [`vbar`]/[`hbar`].
#[derive(Default, Clone, Copy)]
pub struct Fade {
    last_active: f64, // time of the last activity (scroll, or pointer motion / press inside the area)
    vis: f32,         // eased visibility, 0 (hidden) .. 1 (fully shown)
}

impl Fade {
    /// Handle opacity for this frame (0 = hidden). Snaps toward visible on activity — `scrolled` this
    /// frame, or the pointer moving / pressed inside `area` — then eases out after a short idle. Schedules
    /// its own repaints and settles to a stable value, so the UI idles once the bar is hidden or fully
    /// shown (no per-frame repaint during the hold).
    pub fn alpha(&mut self, ui: &egui::Ui, area: Rect, scrolled: bool) -> f32 {
        const HOLD: f64 = 0.9; // fully visible this long after the last activity
        const FADE: f32 = 0.22; // then ease over this
        const OPACITY: f32 = 0.65; // semi-transparent, so the content shows through
        let now = ui.input(|i| i.time);
        let moved = ui.rect_contains_pointer(area)
            && ui.input(|i| i.pointer.delta() != Vec2::ZERO || i.pointer.any_down());
        if scrolled || moved {
            self.last_active = now;
        }
        let idle = now - self.last_active;
        let target = if idle < HOLD { 1.0 } else { 0.0 };
        let step = ui.input(|i| i.stable_dt).min(0.1) / FADE; // clamp dt so the first frame after an idle can't jump
        self.vis = if self.vis < target {
            (self.vis + step).min(target)
        } else {
            (self.vis - step).max(target)
        };
        if self.vis != target {
            ui.ctx().request_repaint(); // mid-fade (in or out)
        } else if idle < HOLD {
            // settled fully shown, still holding — wake once when the hold expires to begin the fade-out
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(HOLD - idle));
        }
        self.vis * OPACITY
    }
}
