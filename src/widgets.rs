//! Reusable painted UI helpers and the custom window chrome (decorations = false):
//! icon buttons, tabs, the white "sheet", the styled scrollbar, plus the caption bar,
//! window border and resize handles we have to draw ourselves since the OS frame is off.

use crate::{
    ACCENT, ACC_BG, ACC_BG2, BORDER_STRONG, CHROME_PAD, DIAG_BOXES, DISABLED, PANEL2,
    SCROLL_DORMANT, SCROLL_HOT, SCROLL_PRESSED, SELECT, TEXT, TEXTDIM,
};
use eframe::egui;
use egui::{Color32, Margin, RichText, CornerRadius, Stroke, Vec2};

const ICON_GLYPH: f32 = 17.5;
const ICON_BTN_W: f32 = 27.0;
/// Smaller icon metrics for the work-area sub-toolbars (a touch smaller than the main toolbar).
const SM_ICON_GLYPH: f32 = 15.0;
const SM_ICON_BTN_W: f32 = 23.0;

/// Full-screen dim backdrop for modal dialogs (translucent black that swallows clicks). Shared by
/// every modal so they all dim identically. `id` must be unique per modal. Returns whether the
/// backdrop (i.e. outside the dialog box) was clicked this frame, so a modal can close on it.
pub(crate) fn dim(ctx: &egui::Context, id: &str) -> bool {
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Middle)
        .fixed_pos(screen.left_top())
        .show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(120));
            ui.allocate_rect(screen, egui::Sense::click()).clicked()
        })
        .inner
}

/// Dismissal gestures for [`show_modal`]: which "close me" inputs fired this frame. The caller
/// decides what state to clear — most dialogs close on their own button / × inside, some also on
/// Escape or a click outside.
pub(crate) struct ModalDismiss {
    pub escape: bool, // Escape was pressed
}

/// The one centered modal-dialog scaffold every dialog shares: the dim backdrop + a foreground box
/// framed by the common [`crate::theme::modal_frame`] with the modal widget style applied. Runs
/// `contents` inside the box (fixed `width`) and returns the dismissal gestures so the caller can
/// close the right state. `id` must be unique per modal.
pub(crate) fn show_modal(
    ctx: &egui::Context,
    id: &str,
    width: f32,
    contents: impl FnOnce(&mut egui::Ui),
) -> ModalDismiss {
    dim(ctx, &format!("{id}_dim")); // draw the dim backdrop + swallow outside clicks
    egui::Area::new(egui::Id::new(id).with("box"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            crate::theme::modal_frame().show(ui, |ui| {
                ui.set_width(width);
                crate::theme::style_modal_widgets(ui);
                contents(ui);
            });
        });
    ModalDismiss {
        escape: ctx.input(|i| i.key_pressed(egui::Key::Escape)),
    }
}

/// Frameless icon button (size-parameterized): no background, glyph turns coral on hover.
fn qbtn_sized(
    ui: &mut egui::Ui,
    icon: &str,
    color: Color32,
    tip: &str,
    glyph: f32,
    btn_w: f32,
) -> egui::Response {
    let size = Vec2::new(btn_w, ui.max_rect().height());
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.hovered() || crate::DIAG_BOXES {
        let box_rect = rect.shrink2(Vec2::new(0.0, CHROME_PAD));
        ui.painter().rect_filled(box_rect, CornerRadius::ZERO, ACC_BG);
    }
    let col = if resp.hovered() { ACCENT } else { color };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(glyph),
        col,
    );
    resp.on_hover_text(tip)
}

/// Frameless icon button: no background, glyph turns coral on hover. Fills the row height.
pub fn qbtn(ui: &mut egui::Ui, icon: &str, color: Color32, tip: &str) -> egui::Response {
    qbtn_sized(ui, icon, color, tip, ICON_GLYPH, ICON_BTN_W)
}

/// Smaller frameless icon button — for the work-area sub-toolbars.
pub fn qbtn_sm(ui: &mut egui::Ui, icon: &str, color: Color32, tip: &str) -> egui::Response {
    qbtn_sized(ui, icon, color, tip, SM_ICON_GLYPH, SM_ICON_BTN_W)
}

/// Toggle icon button: darker background while `active` (pressed), lighter on hover — same
/// accent policy as the tabs / menus. Returns the click response.
pub fn qbtn_toggle(ui: &mut egui::Ui, icon: &str, active: bool, tip: &str) -> egui::Response {
    let size = Vec2::new(ICON_BTN_W, ui.max_rect().height());
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let bg = if active {
        ACC_BG2
    } else if resp.hovered() || DIAG_BOXES {
        ACC_BG
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect.shrink2(Vec2::new(0.0, CHROME_PAD)), CornerRadius::ZERO, bg);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(ICON_GLYPH),
        TEXT,
    );
    resp.on_hover_text(tip)
}

/// Disabled (non-clickable) icon (size-parameterized) — dimmed, no hover effect.
fn qbtn_off_sized(ui: &mut egui::Ui, icon: &str, tip: &str, glyph: f32, btn_w: f32) {
    let size = Vec2::new(btn_w, ui.max_rect().height());
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    if DIAG_BOXES {
        let box_rect = rect.shrink2(Vec2::new(0.0, CHROME_PAD));
        ui.painter().rect_filled(box_rect, CornerRadius::ZERO, ACC_BG);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(glyph),
        DISABLED,
    );
    resp.on_hover_text(tip);
}

/// Disabled (non-clickable) icon — dimmed, no hover effect.
pub fn qbtn_off(ui: &mut egui::Ui, icon: &str, tip: &str) {
    qbtn_off_sized(ui, icon, tip, ICON_GLYPH, ICON_BTN_W);
}

/// Smaller disabled icon — for the work-area sub-toolbars.
pub fn qbtn_off_sm(ui: &mut egui::Ui, icon: &str, tip: &str) {
    qbtn_off_sized(ui, icon, tip, SM_ICON_GLYPH, SM_ICON_BTN_W);
}

/// Small painted close "×" (coral on hover). `half` = arm length.
pub fn close_x(ui: &mut egui::Ui, w: f32, half: f32, tip: &str) -> bool {
    let h = ui.max_rect().height();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let col = if resp.hovered() { ACCENT } else { TEXTDIM };
    let c = rect.center();
    let st = Stroke::new(1.4, col);
    ui.painter()
        .line_segment([egui::pos2(c.x - half, c.y - half), egui::pos2(c.x + half, c.y + half)], st);
    ui.painter()
        .line_segment([egui::pos2(c.x - half, c.y + half), egui::pos2(c.x + half, c.y - half)], st);
    resp.on_hover_text(tip).clicked()
}

/// Frameless tabs in the document style: inner padding, hover/active coral, a 2px
/// underline under the active tab, and an × on the active tab when `closable`.
/// Returns `(selected, closed)` indices.
pub fn tab_strip(
    ui: &mut egui::Ui,
    labels: &[String],
    active: usize,
    closable: bool,
    markers: Option<&[bool]>, // Some → leading status dot per tab (gear/working while busy)
) -> (Option<usize>, Option<usize>) {
    ui.spacing_mut().item_spacing.x = 0.0;
    let h = ui.max_rect().height();
    let bottom = ui.max_rect().bottom();
    let font = egui::FontId::proportional(13.0);
    let pad = 8.0;
    // fixed-width leading slot for the marker, reserved on every tab so the width never jumps
    let mark_w = if markers.is_some() { 16.0 } else { 0.0 };
    let mut select = None;
    let mut close = None;
    for (i, label) in labels.iter().enumerate() {
        let is_active = i == active;
        let busy = markers.map_or(false, |m| m.get(i).copied().unwrap_or(false));
        let galley = ui.painter().layout_no_wrap(label.clone(), font.clone(), TEXT);
        // reserve the close-× width on every closable tab (not just the active one) so the
        // strip doesn't jump when the active tab changes
        let close_w = if closable { 6.0 + 12.0 } else { 0.0 };
        let cell_w = pad + mark_w + galley.size().x + close_w + pad;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(cell_w, h), egui::Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        // accent = darker background on hover, kept (darker still) while active
        let bg = if is_active {
            ACC_BG2
        } else if resp.hovered() || DIAG_BOXES {
            ACC_BG
        } else {
            Color32::TRANSPARENT
        };
        // box inset at the top (flush bottom so the active underline stays on the seam) — same
        // top padding as the menu/toolbar boxes, so the blank gaps above each row match
        let box_rect =
            egui::Rect::from_min_max(egui::pos2(rect.left(), rect.top() + CHROME_PAD), rect.max);
        if bg != Color32::TRANSPARENT {
            ui.painter().rect_filled(box_rect, CornerRadius::ZERO, bg);
        }
        // leading marker: a small dim dot at rest, the "working" glyph while a query runs
        if markers.is_some() {
            let my = box_rect.center().y;
            if busy {
                ui.painter().text(
                    egui::pos2(rect.left() + pad, my),
                    egui::Align2::LEFT_CENTER,
                    crate::ic::REFRESH,
                    egui::FontId::proportional(12.0),
                    ACCENT,
                );
            } else {
                ui.painter()
                    .circle_filled(egui::pos2(rect.left() + pad + 5.0, my), 2.5, TEXTDIM);
            }
        }
        ui.painter().text(
            egui::pos2(rect.left() + pad + mark_w, box_rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font.clone(),
            TEXT,
        );
        // close × on the active tab (own hit-area so it doesn't trigger a tab switch)
        let mut close_hit = false;
        if closable && is_active {
            let cc = egui::pos2(rect.right() - pad - 6.0, box_rect.center().y);
            let xr = egui::Rect::from_center_size(cc, Vec2::new(14.0, h));
            let xresp = ui.interact(xr, ui.id().with(("tab_close", i)), egui::Sense::click());
            if xresp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let col = if xresp.hovered() { ACCENT } else { TEXT };
            let s = 3.0;
            let st = Stroke::new(1.4, col);
            ui.painter()
                .line_segment([egui::pos2(cc.x - s, cc.y - s), egui::pos2(cc.x + s, cc.y + s)], st);
            ui.painter()
                .line_segment([egui::pos2(cc.x - s, cc.y + s), egui::pos2(cc.x + s, cc.y - s)], st);
            if xresp.clicked() {
                close = Some(i);
                close_hit = true;
            }
            xresp.on_hover_text("Close tab");
        }
        if resp.clicked() && !close_hit {
            select = Some(i);
        }
        // 2px underline on the active tab, flush with the bottom edge
        if is_active {
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), bottom - 2.0),
                    egui::pos2(rect.right(), bottom),
                ),
                CornerRadius::ZERO,
                ACCENT,
            );
        }
    }
    (select, close)
}

/// White editing/result sheet. Its edge reads against the (darker) beige chrome — no
/// border, no shadow, no white padding: text fills the sheet and clips exactly at its edge.
pub fn island<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .corner_radius(CornerRadius::ZERO)
        .inner_margin(Margin::ZERO)
        .show(ui, add)
        .inner
}

/// Draw a crisp 1-physical-pixel border just inside `rect`. Plain `rect_stroke` centers the line on
/// the rect edge, which anti-aliases into a fuzzy ~2px line whenever the edge doesn't land on a
/// pixel boundary; snapping each side to a pixel centre keeps every border identical and razor-thin.
pub fn crisp_border(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    let ppp = painter.ctx().pixels_per_point();
    // Exactly ONE physical pixel wide. A 1.0-*point* stroke is 1.5 physical px at 150% scaling, so
    // it anti-aliases into a fuzzy ~2px line whose softness depends on sub-pixel position (that's
    // why some borders looked crisp and others smeared). 1 physical px on a pixel centre is razor-sharp.
    let st = Stroke::new(1.0 / ppp, color);
    let h = 0.5 / ppp; // half a physical pixel, in points
    let l = painter.round_to_pixel_center(rect.left() + h);
    let r = painter.round_to_pixel_center(rect.right() - h);
    let t = painter.round_to_pixel_center(rect.top() + h);
    let b = painter.round_to_pixel_center(rect.bottom() - h);
    painter.vline(l, t..=b, st);
    painter.vline(r, t..=b, st);
    painter.hline(l..=r, t, st);
    painter.hline(l..=r, b, st);
}

/// The shared text button for dialogs / pages: white fill + 1px strong border, accent fill on
/// hover/press, and the label centred on BOTH axes. `enabled == false` greys it out and ignores
/// clicks. Use this everywhere instead of a raw `egui::Button` so buttons look and behave alike.
pub fn ui_button(ui: &mut egui::Ui, label: &str, size: Vec2, enabled: bool) -> egui::Response {
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let (fill, border, text_col) = if !enabled {
        (Color32::WHITE, crate::BORDER, DISABLED)
    } else if resp.is_pointer_button_down_on() {
        (ACC_BG2, BORDER_STRONG, TEXT)
    } else if resp.hovered() {
        (ACC_BG, BORDER_STRONG, TEXT)
    } else {
        (Color32::WHITE, BORDER_STRONG, TEXT)
    };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::ZERO, fill);
    crisp_border(p, rect, border);
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(13.0),
        text_col,
    );
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Snap a rect's edges to whole physical pixels. A `rect_filled` whose edge lands on a fractional
/// pixel (e.g. a white sheet whose left = a fractionally-wide side panel) anti-aliases that edge
/// into a soft smear; snapping the rect first keeps the fill edge — and the crisp_border laid over
/// it — razor sharp on every side.
pub fn snap_rect(painter: &egui::Painter, rect: egui::Rect) -> egui::Rect {
    let ppp = painter.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    egui::Rect::from_min_max(
        egui::pos2(snap(rect.left()), snap(rect.top())),
        egui::pos2(snap(rect.right()), snap(rect.bottom())),
    )
}

/// Blank the side-panel resize line for the current Ui and return the previous style to restore
/// afterwards. egui 0.34 paints that line from THIS ui's `widgets.{hovered,active}.fg_stroke`
/// (panel.rs) — mutating the global/ctx style has no effect because the panel reads `ui.style()`,
/// which was already cloned. Call before `Panel::…show_inside(ui, …)`, then `ui.set_style(saved)`.
pub fn hush_resize_line(ui: &mut egui::Ui) -> std::sync::Arc<egui::Style> {
    let saved = ui.style().clone();
    let s = ui.style_mut();
    s.visuals.widgets.hovered.fg_stroke = Stroke::NONE;
    s.visuals.widgets.active.fg_stroke = Stroke::NONE;
    saved
}

/// Style egui's native scrollbar for the current Ui: fixed (non-floating), muted handle,
/// sharp corners, current thickness. No arrows (egui doesn't provide them).
pub fn style_scrollbar(ui: &mut egui::Ui) {
    let st = ui.style_mut();
    for (wv, c) in [
        (&mut st.visuals.widgets.inactive, SCROLL_DORMANT),
        (&mut st.visuals.widgets.hovered, SCROLL_HOT),
        (&mut st.visuals.widgets.active, SCROLL_PRESSED),
        (&mut st.visuals.widgets.noninteractive, SCROLL_DORMANT),
    ] {
        wv.bg_fill = c;
        wv.weak_bg_fill = c;
        wv.fg_stroke = Stroke::new(1.0, c);
        wv.bg_stroke = Stroke::NONE;
        wv.corner_radius = CornerRadius::ZERO;
    }
    st.spacing.scroll.floating = false;
    st.spacing.scroll.bar_width = 12.0;
    st.spacing.scroll.bar_inner_margin = 2.0;
    st.spacing.scroll.bar_outer_margin = 0.0;
}

/// Coral rounded-square app logo, drawn at `size` px.
pub fn logo(ui: &mut egui::Ui, size: f32) {
    // The "JQ" monogram — same geometry as the app icon (tools/generate_icon.py): a clay rounded
    // square with a white "J" polyline and a "Q" drawn as a ring + diagonal tail (a magnifying
    // glass). Glyph coordinates are normalised to [0,1] over the full square.
    const CLAY: Color32 = Color32::from_rgb(0xC9, 0x64, 0x42);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(size, size), egui::Sense::hover());
    let p = ui.painter();
    // normalised (nx, ny) -> screen position within the allocated square
    let at = |nx: f32, ny: f32| rect.min + Vec2::new(nx * size, ny * size);

    // clay rounded square (6% margin, 22% corner radius — matches the .ico)
    let sq = egui::Rect::from_min_max(at(0.06, 0.06), at(0.94, 0.94));
    let corner = (sq.width() * 0.22) as u8;
    p.rect_filled(sq, CornerRadius::same(corner), CLAY);

    // "J": a single polyline (top bar -> stem -> bottom hook), rounded stroke
    let j_pts: Vec<egui::Pos2> = [
        (0.27, 0.30), (0.46, 0.30),
        (0.43, 0.30), (0.43, 0.60),
        (0.43, 0.635), (0.415, 0.685), (0.375, 0.715),
        (0.325, 0.722), (0.275, 0.700), (0.255, 0.655),
    ]
    .iter()
    .map(|&(x, y)| at(x, y))
    .collect();
    p.add(egui::Shape::line(j_pts, Stroke::new(0.092 * size, Color32::WHITE)));

    // "Q": ring (lens) + short diagonal tail (handle)
    p.circle_stroke(at(0.66, 0.50), 0.12 * size, Stroke::new(0.09 * size, Color32::WHITE));
    p.line_segment(
        [at(0.685, 0.585), at(0.795, 0.715)],
        Stroke::new(0.092 * size, Color32::WHITE),
    );
}

/// A white, thin-bordered single-select list of fixed `size`, styled like the connection
/// dropdown's option rows: full-width rows flush to the edges, no inter-row gap, hover + selected
/// highlight, and a styled scrollbar that only appears when the rows overflow. Clicking a row
/// stores it in `selected`.
pub fn list_pane(
    ui: &mut egui::Ui,
    id: &str,
    size: Vec2,
    items: &[String],
    sel: &mut Vec<String>,
    anchor: &mut Option<usize>,
) -> (egui::Rect, Option<String>) {
    // Reserve an exact box in the parent layout (so a horizontal parent advances by size.x), then
    // draw into a fresh top-down child — don't inherit the parent's layout direction. Returns the
    // pane rect (so the caller can clear the selection on an outside click) + a double-clicked item.
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, Color32::WHITE);
    let (ctrl, shift) = ui.input(|i| (i.modifiers.ctrl, i.modifiers.shift));
    let mut dbl: Option<String> = None;
    let mut child = ui.new_child(
        egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(rect);
    style_scrollbar(&mut child);
    egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(&mut child, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO; // flush rows, no gaps
            let row_h = 22.0;
            for (i, it) in items.iter().enumerate() {
                let is_sel = sel.iter().any(|s| s == it);
                let (r, resp) = ui
                    .allocate_exact_size(Vec2::new(ui.available_width(), row_h), egui::Sense::click());
                if is_sel {
                    ui.painter().rect_filled(r, CornerRadius::ZERO, SELECT);
                } else if resp.contains_pointer() {
                    // contains_pointer (not hovered) so the accent persists while the button is held
                    ui.painter().rect_filled(r, CornerRadius::ZERO, ACC_BG);
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                ui.painter().text(
                    egui::pos2(r.left() + 6.0, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    it,
                    egui::FontId::proportional(13.0),
                    TEXT,
                );
                if resp.double_clicked() {
                    dbl = Some(it.clone());
                } else if resp.clicked() {
                    select_click(sel, anchor, items, i, ctrl, shift);
                    ui.ctx().request_repaint(); // render the new accent next frame, not on mouse-move
                }
            }
            // clicking the empty space below the rows clears this pane's selection
            let rem = ui.available_height();
            if rem > 0.0 {
                let (_, resp) = ui
                    .allocate_exact_size(Vec2::new(ui.available_width(), rem), egui::Sense::click());
                if resp.clicked() {
                    sel.clear();
                    *anchor = None;
                }
            }
        });
    crisp_border(ui.painter(), rect, BORDER_STRONG);
    (rect, dbl)
}

/// A square transfer button with a painter-centred icon glyph (exactly centred regardless of the
/// font's side-bearings). White field + thin border to match the modal's buttons; greyed and inert
/// when `enabled` is false. Returns true on click.
pub fn transfer_btn(
    ui: &mut egui::Ui,
    size: Vec2,
    glyph: &str,
    glyph_size: f32,
    enabled: bool,
    tip: &str,
) -> bool {
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let (bg, fg) = if !enabled {
        (Color32::WHITE, DISABLED)
    } else if resp.is_pointer_button_down_on() {
        (ACC_BG2, TEXT)
    } else if resp.hovered() {
        (ACC_BG, TEXT)
    } else {
        (Color32::WHITE, TEXT)
    };
    ui.painter().rect_filled(rect, CornerRadius::ZERO, bg);
    crisp_border(ui.painter(), rect, BORDER_STRONG);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(glyph_size),
        fg,
    );
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    enabled && resp.on_hover_text(tip).clicked()
}

// Shared metrics for the side-dock manager lists (Connection / Metadata, and the future Git /
// File managers) so every manager renders rows identically.
pub const MGR_ROW_H: f32 = 24.0; // row height
const MGR_LPAD: f32 = 8.0; // left padding before the leading glyph
/// Glyph-column width (chevron OR type icon); the label starts after it. Also the indent step:
/// a child row passes `indent = MGR_GLYPH_COL` so its icon lines up under the parent's label.
pub const MGR_GLYPH_COL: f32 = 20.0;
const MGR_GLYPH_SIZE: f32 = 14.0;
const MGR_LABEL_SIZE: f32 = 13.0;

/// One manager-list row: a leading glyph (a disclosure chevron or a type icon) + a label, painted
/// at the shared height / font. `indent` shifts the whole row right (managers are one level deep,
/// so a leaf passes `MGR_GLYPH_COL` to sit under the parent label). `selected` paints the selection
/// tint; hover paints the soft accent. Returns the click response (use `.rect` for an inline
/// editor). Pass `label = ""` to draw only the glyph.
pub fn manager_row(
    ui: &mut egui::Ui,
    indent: f32,
    glyph: Option<&str>,
    label: &str,
    selected: bool,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), MGR_ROW_H), egui::Sense::click());
    // `contains_pointer`, not `hovered`: while the button is held down egui reports the row as
    // pressed (hovered == false), which would blank the accent for the press frames — the new row
    // appears to lose its accent the instant you click it. contains_pointer stays true on press.
    let bg = if selected {
        SELECT
    } else if resp.contains_pointer() {
        ACC_BG
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, bg);
    }
    // A click changes the selection AFTER this row is drawn, so the new accent would only show on
    // the next frame — which egui (reactive) won't render until the next input. Force that frame so
    // the accent moves instantly instead of waiting for the mouse to move.
    if resp.clicked() {
        ui.ctx().request_repaint();
    }
    // plain arrow cursor in the managers (no pointing hand)
    let x0 = rect.left() + MGR_LPAD + indent;
    if let Some(g) = glyph {
        ui.painter().text(
            egui::pos2(x0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            g,
            egui::FontId::proportional(MGR_GLYPH_SIZE),
            TEXTDIM,
        );
    }
    if !label.is_empty() {
        ui.painter().text(
            egui::pos2(x0 + MGR_GLYPH_COL, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(MGR_LABEL_SIZE),
            TEXT,
        );
    }
    resp
}

/// Standard list-selection click handling: plain click selects only the row, Ctrl toggles it,
/// Shift selects the anchor→row range. `sel` is the selected set, `anchor` the Shift pivot index.
pub fn select_click<T: Clone + PartialEq>(
    sel: &mut Vec<T>,
    anchor: &mut Option<usize>,
    items: &[T],
    i: usize,
    ctrl: bool,
    shift: bool,
) {
    if shift && anchor.is_some() {
        let a = anchor.unwrap().min(items.len().saturating_sub(1));
        let (lo, hi) = (a.min(i), a.max(i));
        *sel = items[lo..=hi].to_vec();
    } else if ctrl {
        if let Some(p) = sel.iter().position(|x| x == &items[i]) {
            sel.remove(p);
        } else {
            sel.push(items[i].clone());
        }
        *anchor = Some(i);
    } else {
        *sel = vec![items[i].clone()];
        *anchor = Some(i);
    }
}

/// A compact dropdown styled like the connect dialog's: white field + thin border, dark text in
/// every state (so the value never vanishes on hover), flush option rows with no gaps or shadow.
/// `font_size` scales the closed value and the rows, so the same control fits toolbars of any size.
/// `current` is the selected index (highlighted in the list); returns the newly picked index.
pub fn styled_combo(
    ui: &mut egui::Ui,
    id: &str,
    width: f32,
    font_size: f32,
    enabled: bool,
    current: Option<usize>,
    options: &[String],
) -> Option<usize> {
    let mut picked = None;
    let sel_text = current.and_then(|i| options.get(i)).cloned().unwrap_or_default();
    let px = 1.0 / ui.ctx().pixels_per_point(); // one physical pixel, for crisp popup borders
    ui.add_enabled_ui(enabled, |ui| {
        // dark text in inactive/hovered/active so the closed value stays visible on hover
        crate::theme::style_modal_widgets(ui);
        // no egui border on the closed button — egui's rect_stroke blurs at fractional DPI; we draw
        // a pixel-snapped crisp_border over it ourselves instead
        {
            let w = &mut ui.style_mut().visuals.widgets;
            w.inactive.bg_stroke = Stroke::NONE;
            w.hovered.bg_stroke = Stroke::NONE;
            w.active.bg_stroke = Stroke::NONE;
            w.open.bg_stroke = Stroke::NONE;
        }
        ui.spacing_mut().button_padding = Vec2::new(6.0, 2.0);
        let inner = egui::ComboBox::from_id_salt(id)
            .width(width)
            .selected_text(RichText::new(sel_text).size(font_size))
            .popup_style(egui::style::StyleModifier::new(move |s| {
                s.spacing.menu_margin = Margin::ZERO;
                s.spacing.item_spacing.y = 0.0;
                s.spacing.button_padding = Vec2::new(6.0, 3.0);
                s.visuals.widgets.inactive.expansion = 0.0;
                s.visuals.widgets.hovered.expansion = 0.0;
                s.visuals.widgets.active.expansion = 0.0;
                s.visuals.popup_shadow = egui::epaint::Shadow::NONE;
                s.visuals.window_stroke = Stroke::new(px, BORDER_STRONG); // 1 physical px
            }))
            .show_ui(ui, |ui| {
                for (i, o) in options.iter().enumerate() {
                    if ui
                        .selectable_label(Some(i) == current, RichText::new(o).size(font_size))
                        .clicked()
                    {
                        picked = Some(i);
                    }
                }
            });
        // crisp 1px frame over the closed combo button (white fill comes from the widget bg)
        crisp_border(ui.painter(), inner.response.rect, BORDER_STRONG);
    });
    picked
}

/// A filled frame with symmetric inner padding — used for the toolbars/strips.
pub fn panel_frame(fill: Color32, x: f32, y: f32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .inner_margin(Margin::symmetric(x as i8, y as i8))
}

/// Labelled single-line input used on the connect screen.
pub fn field(ui: &mut egui::Ui, label: &str, value: &mut String, password: bool) -> egui::Response {
    ui.label(RichText::new(label).color(TEXTDIM).size(11.0));
    let mut te = egui::TextEdit::singleline(value).desired_width(f32::INFINITY);
    if password {
        te = te.password(true);
    }
    let r = ui.add(te);
    ui.add_space(9.0);
    r
}

// ============================================================
// Custom window chrome (decorations = false)
// ============================================================

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
    egui::Frame::new().fill(PANEL2).inner_margin(egui::Margin {
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
    let hover_bg = ACC_BG; // neutral silvery hover (was a warm coral-era tint)
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
        TEXT
    };
    let stroke = Stroke::new(1.2, col);
    let c = rect.center();
    let s = 4.0;
    let p = ui.painter();
    match kind {
        Cap::Min => {
            p.line_segment([egui::pos2(c.x - s, c.y), egui::pos2(c.x + s, c.y)], stroke);
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
                p.rect_stroke(back, CornerRadius::ZERO, stroke, egui::StrokeKind::Middle);
                p.rect_filled(front, CornerRadius::ZERO, if hovered { hover_bg } else { PANEL2 });
                p.rect_stroke(front, CornerRadius::ZERO, stroke, egui::StrokeKind::Middle);
            } else {
                p.rect_stroke(
                    egui::Rect::from_center_size(c, Vec2::new(2.0 * s, 2.0 * s)),
                    CornerRadius::ZERO,
                    stroke,
                    egui::StrokeKind::Middle,
                );
            }
        }
        Cap::Close => {
            p.line_segment([egui::pos2(c.x - s, c.y - s), egui::pos2(c.x + s, c.y + s)], stroke);
            p.line_segment([egui::pos2(c.x - s, c.y + s), egui::pos2(c.x + s, c.y - s)], stroke);
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
    // crisp pixel-snapped 1px outline (matches the islands/sheets — rect_stroke would blur)
    crisp_border(&painter, ctx.content_rect(), BORDER_STRONG);
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
            Stroke::new(1.0, TEXTDIM),
        );
    }
}
