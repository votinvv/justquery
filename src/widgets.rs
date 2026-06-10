//! Reusable painted UI helpers and the custom window chrome (decorations = false):
//! icon buttons, tabs, the white "sheet", the styled scrollbar, plus the caption bar,
//! window border and resize handles we have to draw ourselves since the OS frame is off.

use crate::theme::p;
use crate::{CHROME_PAD, DIAG_BOXES, RADIUS_CONTROL, RADIUS_ISLAND, SPACE_1};
use eframe::egui;
use egui::{Color32, Margin, CornerRadius, Stroke, Vec2};

const ICON_GLYPH: f32 = 17.5;
const ICON_BTN_W: f32 = 27.0;
/// Smaller icon metrics for the work-area sub-toolbars (a touch smaller than the main toolbar).
const SM_ICON_GLYPH: f32 = 15.0;
const SM_ICON_BTN_W: f32 = 23.0;

/// Full-screen dim backdrop for modal dialogs (translucent black that swallows clicks). Shared by
/// every modal so they all dim identically. `id` must be unique per modal.
fn dim(ctx: &egui::Context, id: &str) {
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Middle)
        .fixed_pos(screen.left_top())
        .show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(120));
            ui.allocate_rect(screen, egui::Sense::click()); // swallow clicks outside the box
        });
}

/// Dismissal gestures for [`show_modal`]: which "close me" inputs fired this frame. The caller
/// decides what state to clear — most dialogs close on their own button / × inside, some also on
/// Escape.
///
/// The modal key contract (Design Delta v2.1 §5): **Enter presses the primary/destructive
/// button, Esc presses Cancel** — every modal wires `enter` to its one primary action.
pub(crate) struct ModalDismiss {
    pub escape: bool, // Escape was pressed → Cancel/close
    pub enter: bool,  // Enter was pressed → the primary (or destructive-primary) action
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
        enter: ctx.input(|i| i.key_pressed(egui::Key::Enter)),
    }
}

/// Frameless icon button (size-parameterized): neutral soft box on hover, glyph keeps its colour.
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
        ui.painter().rect_filled(box_rect, CornerRadius::ZERO, p().acc_bg);
    }
    // hover is neutral: the soft box above is the affordance, the glyph keeps its colour (accent
    // is reserved for committed/meaningful state, never hover — Design System §2).
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(glyph),
        color,
    );
    resp.on_hover_text(tip)
}

/// Frameless icon button: neutral soft box on hover, glyph keeps its colour. Fills the row height.
pub fn qbtn(ui: &mut egui::Ui, icon: &str, tip: &str) -> egui::Response {
    qbtn_sized(ui, icon, p().text, tip, ICON_GLYPH, ICON_BTN_W)
}

/// Panel icon button with the neutral state ramp (icons/README): `text_dim` at rest,
/// `text` on hover — for toolbar actions that carry no state colour. Currently unused (the plug
/// toggle moved to full-strength `qbtn`), kept for the icon-button family.
#[allow(dead_code)]
pub fn qbtn_dim(ui: &mut egui::Ui, icon: &str, tip: &str) -> egui::Response {
    let size = Vec2::new(ICON_BTN_W, ui.max_rect().height());
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.hovered() || crate::DIAG_BOXES {
        let box_rect = rect.shrink2(Vec2::new(0.0, CHROME_PAD));
        ui.painter().rect_filled(box_rect, CornerRadius::ZERO, p().acc_bg);
    }
    let col = if resp.hovered() { p().text } else { p().text_dim };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(ICON_GLYPH),
        col,
    );
    resp.on_hover_text(tip)
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
        p().acc_bg2
    } else if resp.hovered() || DIAG_BOXES {
        p().acc_bg
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
        p().text,
    );
    resp.on_hover_text(tip)
}

/// Disabled (non-clickable) icon (size-parameterized) — dimmed, no hover effect.
fn qbtn_off_sized(ui: &mut egui::Ui, icon: &str, tip: &str, glyph: f32, btn_w: f32) {
    let size = Vec2::new(btn_w, ui.max_rect().height());
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    if DIAG_BOXES {
        let box_rect = rect.shrink2(Vec2::new(0.0, CHROME_PAD));
        ui.painter().rect_filled(box_rect, CornerRadius::ZERO, p().acc_bg);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(glyph),
        p().disabled,
    );
    resp.on_hover_text(tip);
}

/// Disabled (dimmed, inert) icon at the main-toolbar size — the counterpart of [`qbtn`].
pub fn qbtn_off(ui: &mut egui::Ui, icon: &str, tip: &str) {
    qbtn_off_sized(ui, icon, tip, ICON_GLYPH, ICON_BTN_W);
}

/// Smaller disabled icon — for the work-area sub-toolbars.
pub fn qbtn_off_sm(ui: &mut egui::Ui, icon: &str, tip: &str) {
    qbtn_off_sized(ui, icon, tip, SM_ICON_GLYPH, SM_ICON_BTN_W);
}

/// Paint a chevron (or double chevron) pointing left/right, centred in `rect` — the icon set
/// has no left-pointing glyphs, so directional arrows are drawn in the set's own language
/// (1.8 stroke, matching the 24-grid proportions).
pub fn paint_chevron(
    painter: &egui::Painter,
    rect: egui::Rect,
    left: bool,
    double: bool,
    color: Color32,
) {
    let c = rect.center();
    let (half_h, half_w) = (4.5_f32, 2.5_f32);
    let dir = if left { -1.0 } else { 1.0 };
    let st = Stroke::new(1.8, color);
    let tip = |cx: f32| {
        painter.line_segment(
            [egui::pos2(cx - dir * half_w, c.y - half_h), egui::pos2(cx + dir * half_w, c.y)],
            st,
        );
        painter.line_segment(
            [egui::pos2(cx + dir * half_w, c.y), egui::pos2(cx - dir * half_w, c.y + half_h)],
            st,
        );
    };
    if double {
        tip(c.x - dir * 2.5);
        tip(c.x + dir * 2.5);
    } else {
        tip(c.x);
    }
}

/// Chevron icon button in the chrome-row style of [`qbtn`] (neutral hover box, glyph keeps
/// its colour) — for the tab-strip scroll arrows.
pub fn qchevron(ui: &mut egui::Ui, left: bool, tip: &str) -> egui::Response {
    let size = Vec2::new(ICON_BTN_W, ui.max_rect().height());
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.hovered() || DIAG_BOXES {
        let box_rect = rect.shrink2(Vec2::new(0.0, CHROME_PAD));
        ui.painter().rect_filled(box_rect, CornerRadius::ZERO, p().acc_bg);
    }
    paint_chevron(ui.painter(), rect, left, false, p().text);
    resp.on_hover_text(tip)
}

/// Small painted close "×" — neutral at rest, `danger` red on hover (destructive action).
pub fn close_x(ui: &mut egui::Ui, tip: &str) -> bool {
    const W: f32 = 22.0; // click-area width
    const HALF: f32 = 4.0; // half-length of each × arm
    let h = ui.max_rect().height();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(W, h), egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let col = if resp.hovered() { p().danger } else { p().text_dim };
    let c = rect.center();
    let st = Stroke::new(1.4, col);
    ui.painter()
        .line_segment([egui::pos2(c.x - HALF, c.y - HALF), egui::pos2(c.x + HALF, c.y + HALF)], st);
    ui.painter()
        .line_segment([egui::pos2(c.x - HALF, c.y + HALF), egui::pos2(c.x + HALF, c.y - HALF)], st);
    resp.on_hover_text(tip).clicked()
}

/// Pill tabs — the studio signature (Design System v2 §6). Active tab is a pill:
/// `accent_soft` fill, radius 4 rectangle (v2.2 — no pills), `accent_hi` text, blur-2 shadow.
/// Inactive: transparent, `text_dim`, a neutral hover pill. No underline bars.
/// An × shows on the active tab when `closable`.
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
    let font = egui::FontId::proportional(13.0);
    let pad = 10.0; // a touch more side padding — pills read better with air around the label
    // fixed-width leading slot for the marker, reserved on every tab so the width never jumps
    let mark_w = if markers.is_some() { 16.0 } else { 0.0 };
    let pill_radius = CornerRadius::same(RADIUS_CONTROL);
    let mut select = None;
    let mut close = None;
    for (i, label) in labels.iter().enumerate() {
        let is_active = i == active;
        let busy = markers.is_some_and(|m| m.get(i).copied().unwrap_or(false));
        let galley = ui.painter().layout_no_wrap(label.clone(), font.clone(), p().text);
        // reserve the close-× width on every closable tab (not just the active one) so the
        // strip doesn't jump when the active tab changes
        let close_w = if closable { 6.0 + 12.0 } else { 0.0 };
        let cell_w = pad + mark_w + galley.size().x + close_w + pad;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(cell_w, h), egui::Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        // the pill floats inside the row with the chrome inset on BOTH edges
        let pill_rect = rect.shrink2(Vec2::new(0.0, CHROME_PAD));
        if is_active {
            // subtle lift: offset [0,1], blur 2 — softer than the island shadow
            ui.painter().add(
                egui::epaint::Shadow { offset: [0, 1], blur: 2, spread: 0, color: p().shadow }
                    .as_shape(pill_rect, pill_radius),
            );
            ui.painter().rect_filled(pill_rect, pill_radius, p().accent_soft);
        } else if resp.hovered() || DIAG_BOXES {
            ui.painter().rect_filled(pill_rect, pill_radius, p().hover);
        }
        // leading marker: a small dim dot at rest, the "working" glyph while a query runs
        if markers.is_some() {
            let my = pill_rect.center().y;
            if busy {
                ui.painter().text(
                    egui::pos2(rect.left() + pad, my),
                    egui::Align2::LEFT_CENTER,
                    crate::ic::REFRESH,
                    egui::FontId::proportional(12.0),
                    p().accent_hi,
                );
            } else {
                ui.painter()
                    .circle_filled(egui::pos2(rect.left() + pad + 5.0, my), 2.5, p().text_dim);
            }
        }
        ui.painter().text(
            egui::pos2(rect.left() + pad + mark_w, pill_rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font.clone(),
            if is_active { p().accent_hi } else { p().text_dim },
        );
        // close × on every tab (own hit-area so it doesn't trigger a tab switch) — always visible,
        // not just the active one, so it's always one click away (a dirty tab still confirms first)
        let mut close_hit = false;
        if closable {
            let cc = egui::pos2(rect.right() - pad - 6.0, pill_rect.center().y);
            let xr = egui::Rect::from_center_size(cc, Vec2::new(14.0, h));
            let xresp = ui.interact(xr, ui.id().with(("tab_close", i)), egui::Sense::click());
            if xresp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let col = if xresp.hovered() {
                p().danger
            } else if is_active {
                p().accent_hi
            } else {
                p().text_dim
            };
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
    }
    (select, close)
}

/// White result/editing sheet (field_bg fill, soft shadow). The 1px frame is drawn ON TOP of the
/// content — not as a Frame stroke behind it — because the grid fills its own background a hair
/// inside the frame, and a behind-the-content stroke left that field_bg hairline showing as a
/// white halo on all four sides. Drawing the border last covers it.
pub fn island<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let inner = egui::Frame::new()
        .fill(p().field_bg)
        .corner_radius(CornerRadius::same(RADIUS_ISLAND))
        .shadow(crate::theme::island_shadow())
        .inner_margin(Margin::ZERO)
        .show(ui, add);
    crisp_border(ui.painter(), inner.response.rect, p().border_strong);
    inner.inner
}

/// One form row — THE form law, hard numbers (Design Delta v2.2 §6): a 16px label line
/// (`Small`/`text_dim`) → exactly 4px → the control → exactly 16px to the next row. Vertical
/// item-spacing is zeroed inside so nothing pads the gaps — the label visually belongs to
/// ITS control. Hand-rolled label+field stacks in forms are forbidden; use this.
pub fn form_row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 16.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| ui.label(egui::RichText::new(label).color(p().text_dim).size(11.0)),
        );
        ui.add_space(4.0);
        let r = add(ui);
        ui.add_space(16.0);
        r
    })
    .inner
}

/// A tinted chip (r4 rectangle, coloured bold label) — since the v2.3 flat status bar the one
/// remaining use is the version chip in the About modal.
/// Use `theme::tint(p().panel, colour, 0.16)` for the background.
pub fn status_chip(ui: &mut egui::Ui, label: &str, fg: Color32, bg: Color32, sz: f32) {
    let font = crate::theme::ui_bold_font(sz);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), fg);
    // tight horizontal padding (6px each side) so the chip hugs its label
    let size = Vec2::new(galley.size().x + 12.0, galley.size().y + 5.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), bg);
    ui.painter().text(
        egui::pos2(rect.left() + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        fg,
    );
}

/// Paint the soft studio shadow under a hand-drawn island. Call BEFORE the island's fill —
/// the shadow is a blurred rect that would otherwise darken the island itself.
pub fn island_shadow_under(painter: &egui::Painter, rect: egui::Rect) {
    painter.add(
        crate::theme::island_shadow().as_shape(rect, CornerRadius::same(RADIUS_ISLAND)),
    );
}

/// Stroke-only overlay border (focus/danger rings, the square window outline, selection
/// frames). The v2.2 border law: 1.0 LOGICAL stroke, `StrokeKind::Inside`, no pixel snapping
/// (the old physical-pixel snap produced seams at rounded corners). Static island borders must
/// NOT use this over a separate fill — pair fill+stroke in one shape via [`island_box`].
pub fn crisp_border(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    crisp_border_r(painter, rect, color, crate::theme::RADIUS_ISLAND);
}

/// [`crisp_border`] with an explicit radius (0 = the square window outline).
pub fn crisp_border_r(painter: &egui::Painter, rect: egui::Rect, color: Color32, radius: u8) {
    painter.rect_stroke(
        rect,
        CornerRadius::same(radius),
        Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );
}

/// An island/field/popup body: fill + 1.0 inside stroke as ONE `RectShape` (Design Delta
/// v2.2 §3) — the single-shape law kills the seams the split fill/stroke passes used to leave
/// around rounded corners. Shadow (if any) goes UNDER this via [`island_shadow_under`].
pub fn island_box(painter: &egui::Painter, rect: egui::Rect, fill: Color32, radius: u8) {
    painter.add(egui::epaint::RectShape::new(
        rect,
        CornerRadius::same(radius),
        fill,
        Stroke::new(1.0, p().border_strong),
        egui::StrokeKind::Inside,
    ));
}

fn button_size(ui: &egui::Ui, label: &str) -> Vec2 {
    let galley =
        ui.painter().layout_no_wrap(label.to_owned(), egui::FontId::proportional(13.0), p().text);
    // unified controls: CONTROL_H tall, 14px side padding (Design Delta v2.2 §4)
    Vec2::new(galley.size().x + 14.0 * 2.0, crate::theme::CONTROL_H)
}

/// The widest a button needs to be to fit any of `labels` at the standard geometry. Use it to give
/// every button on one modal the same width (Design System §7 Modals: uniform, right-aligned, at
/// the bottom) — measure the modal's labels once, then render each with the `*_button_w` variant.
pub fn uniform_button_width(ui: &egui::Ui, labels: &[&str]) -> f32 {
    labels
        .iter()
        .fold(crate::theme::CONTROL_H, |w, l| w.max(button_size(ui, l).x))
}

/// The single filled (accent) button a dialog is allowed: white text, [`RADIUS_CONTROL`], and
/// [`ACCENT_PRESS`] while held. Sizes to its label. Returns true on click.
pub fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    primary_button_w(ui, label, enabled, button_size(ui, label).x)
}

/// [`primary_button`] at an explicit width (for uniform modal button bars).
pub fn primary_button_w(ui: &mut egui::Ui, label: &str, enabled: bool, width: f32) -> bool {
    let size = Vec2::new(width, crate::theme::CONTROL_H);
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    // confirm buttons accentuate on hover, darken further while held (matches destructive_button)
    let fill = if !enabled {
        p().disabled
    } else if resp.is_pointer_button_down_on() {
        p().accent_press
    } else if resp.hovered() {
        crate::theme::tint(p().accent, Color32::BLACK, 0.10)
    } else {
        p().accent
    };
    let pt = ui.painter();
    pt.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), fill);
    pt.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        crate::theme::ui_bold_font(13.0), // ~weight 500 — the primary action reads a touch heavier
        p().on_accent,
    );
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    enabled && resp.clicked()
}

/// Destructive primary (Design Delta v2.1 §5): the confirming button of a destructive modal —
/// `danger` fill, `on_accent` text, the exact primary geometry. A modal carries either a primary
/// OR a destructive primary, never both. Returns true on click.
#[allow(dead_code)] // API counterpart of primary/secondary_button; callers use the _w variant
pub fn destructive_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    destructive_button_w(ui, label, enabled, button_size(ui, label).x)
}

/// [`destructive_button`] at an explicit width (for uniform modal button bars).
pub fn destructive_button_w(ui: &mut egui::Ui, label: &str, enabled: bool, width: f32) -> bool {
    let size = Vec2::new(width, crate::theme::CONTROL_H);
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    // React like the primary button — darken on hover, darker still while held. A flat, inert
    // fill read as "not clickable"; the press feedback makes the affordance unmistakable.
    let fill = if !enabled {
        p().disabled
    } else if resp.is_pointer_button_down_on() {
        crate::theme::tint(p().danger, Color32::BLACK, 0.22)
    } else if resp.hovered() {
        crate::theme::tint(p().danger, Color32::BLACK, 0.10)
    } else {
        p().danger
    };
    let pt = ui.painter();
    pt.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), fill);
    pt.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        crate::theme::ui_bold_font(13.0),
        p().on_accent,
    );
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    enabled && resp.clicked()
}

/// Outline (secondary) button: white fill, 1px `border_strong`, text colour, neutral `hover` fill.
/// Sizes to its label. Returns true on click.
#[allow(dead_code)] // API counterpart of primary_button; modal callers use the _w variant
pub fn secondary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    secondary_button_w(ui, label, enabled, button_size(ui, label).x)
}

/// [`secondary_button`] at an explicit width (for uniform modal button bars).
pub fn secondary_button_w(ui: &mut egui::Ui, label: &str, enabled: bool, width: f32) -> bool {
    let size = Vec2::new(width, crate::theme::CONTROL_H);
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let (fill, text_col) = if !enabled {
        (p().field_bg, p().disabled)
    } else if resp.hovered() {
        (p().hover, p().text)
    } else {
        (p().field_bg, p().text)
    };
    let pt = ui.painter();
    island_box(pt, rect, fill, RADIUS_CONTROL);
    pt.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(13.0),
        text_col,
    );
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    enabled && resp.clicked()
}

/// A bare single-line input sized to the shared field height, with the accent focus ring drawn
/// over its border when it holds keyboard focus (Design System §6 Text fields). The caller paints
/// the label and the gap; this is just the field, so spacing comes from the `SPACE_*` scale.
pub fn focus_field(ui: &mut egui::Ui, value: &mut String, password: bool, width: f32) -> egui::Response {
    let h = crate::theme::FIELD_H; // shared field height so a form's controls line up exactly
    let mut te = egui::TextEdit::singleline(value)
        .desired_width(width)
        .margin(Margin::symmetric(8, 4)); // 8px text inset (v2.2 §4 — never hugs the rounding)
    if password {
        te = te.password(true);
    }
    let r = ui.add_sized(Vec2::new(width, h), te);
    if r.has_focus() {
        crisp_border_r(ui.painter(), r.rect, p().accent, RADIUS_CONTROL);
    }
    r
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
        (&mut st.visuals.widgets.inactive, p().scroll_dormant),
        (&mut st.visuals.widgets.hovered, p().scroll_hot),
        (&mut st.visuals.widgets.active, p().scroll_pressed),
        (&mut st.visuals.widgets.noninteractive, p().scroll_dormant),
    ] {
        wv.bg_fill = c;
        wv.weak_bg_fill = c;
        wv.fg_stroke = Stroke::new(1.0, c);
        wv.bg_stroke = Stroke::NONE;
        // pill handle: radius = half the 8px bar width (Design Delta v2.1 §4)
        wv.corner_radius = CornerRadius::same(4);
    }
    // Auto-hiding floating pills (v2.2 §8): 8px wide, 4px inset off the island frame,
    // invisible at rest — the trough never paints (background opacity 0), so the old
    // transparent-extreme_bg hack is gone.
    st.spacing.scroll.floating = true;
    st.spacing.scroll.bar_width = 8.0;
    st.spacing.scroll.floating_width = 8.0;
    st.spacing.scroll.floating_allocated_width = 0.0;
    st.spacing.scroll.bar_inner_margin = 4.0;
    st.spacing.scroll.bar_outer_margin = 4.0;
    st.spacing.scroll.dormant_background_opacity = 0.0;
    st.spacing.scroll.dormant_handle_opacity = 0.0;
    st.spacing.scroll.active_background_opacity = 0.0;
    st.spacing.scroll.interact_background_opacity = 0.0;
    st.spacing.scroll.active_handle_opacity = 0.8;
    st.spacing.scroll.interact_handle_opacity = 1.0;
}

/// Coral rounded-square app logo, drawn at `size` px.
pub fn logo(ui: &mut egui::Ui, size: f32) {
    // The "JQ" monogram — same geometry as the app icon (tools/generate_icon.py): a clay rounded
    // square with a white "J" polyline and a "Q" drawn as a ring + diagonal tail (a magnifying
    // glass). Glyph coordinates are normalised to [0,1] over the full square.
    const CLAY: Color32 = Color32::from_rgb(0xC9, 0x64, 0x42);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(size, size), egui::Sense::hover());
    let pt = ui.painter();
    // normalised (nx, ny) -> screen position within the allocated square
    let at = |nx: f32, ny: f32| rect.min + Vec2::new(nx * size, ny * size);

    // clay rounded square (6% margin, 22% corner radius — matches the .ico)
    let sq = egui::Rect::from_min_max(at(0.06, 0.06), at(0.94, 0.94));
    let corner = (sq.width() * 0.22) as u8;
    pt.rect_filled(sq, CornerRadius::same(corner), CLAY);

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
    pt.add(egui::Shape::line(j_pts, Stroke::new(0.092 * size, Color32::WHITE)));

    // "Q": ring (lens) + short diagonal tail (handle)
    pt.circle_stroke(at(0.66, 0.50), 0.12 * size, Stroke::new(0.09 * size, Color32::WHITE));
    pt.line_segment(
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
    island_shadow_under(ui.painter(), rect);
    island_box(ui.painter(), rect, p().field_bg, RADIUS_ISLAND);
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
                    ui.painter().rect_filled(r, CornerRadius::ZERO, p().select);
                } else if resp.contains_pointer() {
                    // contains_pointer (not hovered) so the accent persists while the button is held
                    ui.painter().rect_filled(r, CornerRadius::ZERO, p().acc_bg);
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                ui.painter().text(
                    egui::pos2(r.left() + 6.0, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    it,
                    egui::FontId::proportional(13.0),
                    p().text,
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
    (rect, dbl)
}

/// A square transfer button with a painter-centred icon glyph (exactly centred regardless of the
/// font's side-bearings). White field + thin border to match the modal's buttons; greyed and inert
/// when `enabled` is false. Returns true on click.
pub fn transfer_btn(
    ui: &mut egui::Ui,
    size: Vec2,
    left: bool,
    double: bool,
    enabled: bool,
    tip: &str,
) -> bool {
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let (bg, fg) = if !enabled {
        (p().field_bg, p().disabled)
    } else if resp.is_pointer_button_down_on() {
        (p().acc_bg2, p().text)
    } else if resp.hovered() {
        (p().acc_bg, p().text)
    } else {
        (p().field_bg, p().text)
    };
    island_box(ui.painter(), rect, bg, RADIUS_CONTROL);
    paint_chevron(ui.painter(), rect, left, double, fg);
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
    glyph: &str,
    label: &str,
    selected: bool,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), MGR_ROW_H), egui::Sense::click());
    // `contains_pointer`, not `hovered`: while the button is held down egui reports the row as
    // pressed (hovered == false), which would blank the accent for the press frames — the new row
    // appears to lose its accent the instant you click it. contains_pointer stays true on press.
    let bg = if selected {
        p().select
    } else if resp.contains_pointer() {
        p().hover
    } else {
        Color32::TRANSPARENT
    };
    // Round the fill where the row meets the island's rounded frame — the top row rounds its top
    // corners, the bottom row its bottom — so selection/hover reaches the corner like the combo
    // dropdown, instead of leaving the white corner triangles. Detected from the clip rect (the
    // island interior), so it works for both the connection list and the object tree.
    let clip = ui.clip_rect();
    let r = RADIUS_ISLAND;
    let round_top = rect.top() <= clip.top() + 0.5;
    let round_bot = rect.bottom() >= clip.bottom() - 0.5;
    let cr = CornerRadius {
        nw: if round_top { r } else { 0 },
        ne: if round_top { r } else { 0 },
        sw: if round_bot { r } else { 0 },
        se: if round_bot { r } else { 0 },
    };
    if bg != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, cr, bg);
    }
    // A click changes the selection AFTER this row is drawn, so the new accent would only show on
    // the next frame — which egui (reactive) won't render until the next input. Force that frame so
    // the accent moves instantly instead of waiting for the mouse to move.
    if resp.clicked() {
        ui.ctx().request_repaint();
    }
    // plain arrow cursor in the managers (no pointing hand)
    let x0 = rect.left() + MGR_LPAD + indent;
    if !glyph.is_empty() {
        ui.painter().text(
            egui::pos2(x0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            glyph,
            egui::FontId::proportional(MGR_GLYPH_SIZE),
            if selected { p().text } else { p().text_dim },
        );
    }
    if !label.is_empty() {
        ui.painter().text(
            egui::pos2(x0 + MGR_GLYPH_COL, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(MGR_LABEL_SIZE),
            p().text,
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

/// Trim `text` (adding an ellipsis) so it fits within `max_w` points at `font_size`. Returns the
/// text unchanged if it already fits. Used to clip a dropdown's closed value instead of letting it
/// stretch the control.
pub fn truncate_to_width(ui: &egui::Ui, text: &str, font_size: f32, max_w: f32) -> String {
    if max_w <= 0.0 || text.is_empty() {
        return String::new();
    }
    let font = egui::FontId::proportional(font_size);
    let measure = |s: &str| {
        ui.ctx().fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font.clone(), Color32::BLACK).size().x)
    };
    if measure(text) <= max_w {
        return text.to_owned();
    }
    let ell_w = measure("…");
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        let candidate: String = chars[..n].iter().collect();
        if measure(&candidate) + ell_w <= max_w {
            return format!("{candidate}…");
        }
        n -= 1;
    }
    "…".to_owned()
}

/// A compact dropdown — our own, not `egui::ComboBox`, so the open list is pixel-exact to the field
/// (same rect, same `crisp_border`) and tracks panel resizes both ways with no popup-sizing quirks.
/// White field + thin border, the value clipped with an ellipsis, a down arrow, and flush option
/// rows (hover + selected highlight, ellipsis + full-name tooltip when a name is wider than the
/// field). `current` is the selected index; returns the newly picked index. Closes on pick, Escape,
/// or a click outside.
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
    let open_id = ui.make_persistent_id(("combo_open", id));
    let mut open = enabled && ui.ctx().data(|d| d.get_temp::<bool>(open_id).unwrap_or(false));

    // ---- closed field ----
    let h = crate::theme::FIELD_H; // same thin height as every form field
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, h), sense);
    let pt = ui.painter().clone();
    island_box(&pt, rect, p().field_bg, RADIUS_CONTROL);
    let text_col = if enabled { p().text } else { p().disabled };
    let sel_full = current.and_then(|i| options.get(i)).cloned().unwrap_or_default();
    // leave room for the left pad (6) and the arrow (~16)
    let sel_text = truncate_to_width(ui, &sel_full, font_size, (width - 24.0).max(0.0));
    pt.text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        sel_text,
        egui::FontId::proportional(font_size),
        text_col,
    );
    // down arrow (filled triangle), right-aligned
    let (cx, cy) = (rect.right() - 12.0, rect.center().y);
    pt.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(cx - 4.0, cy - 2.5),
            egui::pos2(cx + 4.0, cy - 2.5),
            egui::pos2(cx, cy + 3.0),
        ],
        text_col,
        Stroke::NONE,
    ));
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.clicked() {
        open = !open;
    }

    // ---- open list: our own Area, anchored to the field's bottom-left, exactly `width` wide ----
    if open && !options.is_empty() {
        let row_h = 24.0;
        const MAX_VIS: usize = 10;
        let list_h = (options.len().min(MAX_VIS) as f32) * row_h;
        let area = egui::Area::new(ui.make_persistent_id(("combo_popup", id)))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_bottom())
            .constrain(true)
            .show(ui.ctx(), |ui| {
                // use the field's SNAPPED width, and snap the popup rect too → identical edges
                let (prect, _) =
                    ui.allocate_exact_size(Vec2::new(rect.width(), list_h), egui::Sense::hover());
                island_shadow_under(ui.painter(), prect);
                island_box(ui.painter(), prect, p().ivory, RADIUS_ISLAND);
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(prect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                // clip just inside the 1px border; the first/last row fills round to match the
                // frame corners (below), so the hover/selection reaches the edge without leaving
                // the white corner triangles that read as a bug (v2.2 §10)
                child.set_clip_rect(prect.shrink(1.0));
                style_scrollbar(&mut child);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                    .show(&mut child, |ui| {
                        let aw = ui.available_width();
                        ui.set_width(aw);
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        for (i, o) in options.iter().enumerate() {
                            let (rr, rresp) = ui
                                .allocate_exact_size(Vec2::new(aw, row_h), egui::Sense::click());
                            let hovered = rresp.hovered();
                            let selected = Some(i) == current;
                            // round the fill on the first row's top and the last row's bottom so it
                            // follows the popup's rounded frame; middle rows stay square
                            let rr_top = i == 0;
                            let rr_bot = i + 1 == options.len();
                            let fr = RADIUS_ISLAND;
                            let cr = CornerRadius {
                                nw: if rr_top { fr } else { 0 },
                                ne: if rr_top { fr } else { 0 },
                                sw: if rr_bot { fr } else { 0 },
                                se: if rr_bot { fr } else { 0 },
                            };
                            if hovered {
                                ui.painter().rect_filled(rr, cr, p().hover);
                            } else if selected {
                                ui.painter().rect_filled(rr, cr, p().select);
                            }
                            let label = truncate_to_width(ui, o, font_size, (rr.width() - 16.0).max(0.0));
                            ui.painter().text(
                                egui::pos2(rr.left() + 8.0, rr.center().y),
                                egui::Align2::LEFT_CENTER,
                                &label,
                                egui::FontId::proportional(font_size),
                                p().text,
                            );
                            if hovered {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            let clicked = rresp.clicked();
                            if label.as_str() != o.as_str() {
                                rresp.on_hover_text(o); // full name on hover when clipped
                            }
                            if clicked {
                                picked = Some(i);
                            }
                        }
                    });
                crisp_border(ui.painter(), prect, p().border_strong);
            });
        let popup_rect = area.response.rect;
        if picked.is_some() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        // a press outside both the field and the list dismisses it
        if ui.input(|i| i.pointer.any_pressed()) {
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                if !popup_rect.contains(pos) && !rect.contains(pos) {
                    open = false;
                }
            }
        }
    } else {
        open = false;
    }

    ui.ctx().data_mut(|d| d.insert_temp(open_id, open));
    picked
}

/// A 1px vertical divider that separates toolbar icon groups: 16px tall, centred in the chrome
/// row, with `SPACE_1` of breathing room on each side (Design System §6 Window chrome).
pub fn toolbar_divider(ui: &mut egui::Ui) {
    let h = ui.max_rect().height();
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(SPACE_1 * 2.0 + 1.0, h), egui::Sense::hover());
    let cx = ui.painter().round_to_pixel_center(rect.center().x);
    let cy = rect.center().y;
    let half = 8.0; // 16px tall
    ui.painter().vline(
        cx,
        (cy - half)..=(cy + half),
        Stroke::new(1.0 / ui.ctx().pixels_per_point(), p().border),
    );
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
