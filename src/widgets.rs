//!
//! Reusable painted UI helpers: icon buttons, tabs, modals, the white "sheet", the styled
//! scrollbar. The custom window chrome (caption bar, border, resize handles) lives in
//! [`crate::winchrome`].
#![allow(dead_code)] // JustQuery's widget library — not every helper is in use

use crate::theme::p;
use crate::{DIAG_BOXES, RADIUS_CONTROL, RADIUS_ISLAND};
use eframe::egui;
use egui::{Color32, Margin, CornerRadius, Pos2, Stroke, Vec2};

const ICON_GLYPH: f32 = 17.5;
/// Smaller icon glyph for the work-area sub-toolbars (a touch smaller than the main toolbar).
const SM_ICON_GLYPH: f32 = 15.0;
// Icon-button width is no longer fixed: the button is SQUARE — its side = the height of its own row
// (`ui.max_rect().height()`), so the box is larger in the main toolbar than in the sub-toolbars. The
// `qchevron` arrows are square too, so the reserve for a pair = `2 * row_h` is computed at the call site.

/// Full-screen dim backdrop for modal dialogs (translucent black that swallows clicks). Shared by
/// every modal so they all dim identically. `id` must be unique per modal.
/// Backdrop alpha for modal dialogs (translucent black that dims the app behind). The single raw
/// color value shared by every modal's dim layer.
const MODAL_DIM_ALPHA: u8 = 120;

fn dim(ctx: &egui::Context, id: &str) {
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Middle)
        .fixed_pos(screen.left_top())
        .show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(MODAL_DIM_ALPHA));
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
            crate::theme::modal_frame(ctx).show(ui, |ui| {
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
/// A modal's title row: the 15pt bold heading on the left and a close-× on the right. Returns true
/// when the × was clicked, so the caller clears its own open-state. The single source of the
/// modal-title metrics — dialogs use it instead of re-typing the `horizontal` / `RichText` /
/// `right_to_left` + `close_x` block. (Plain title-only modals just use a bare bold label.)
pub fn modal_header(ui: &mut egui::Ui, title: &str) -> bool {
    let mut closed = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).size(crate::HEADING_SIZE).strong().color(p().text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if close_x(ui, "Close") {
                closed = true;
            }
        });
    });
    closed
}

/// What a confirmation is about — drives the confirm button's colour: `Danger` = a destructive /
/// irreversible action (coral button), `Normal` = the accent primary. The single classifier every
/// yes/no dialog shares.
#[derive(Clone, Copy)]
pub(crate) enum ConfirmKind {
    Normal,
    Danger,
}

/// Outcome of a [`confirm_modal`] frame: the user pressed confirm (Enter included) or dismissed
/// (Cancel / Esc). Both false → the modal stays open and the caller keeps its open-state.
pub(crate) struct ConfirmOutcome {
    pub confirmed: bool,
    pub cancelled: bool,
}

/// The one confirmation dialog every yes/no prompt routes through: a fixed-width modal with a bold
/// title, a wrapped message and a right-aligned `[cancel] [confirm]` pair of uniform width. `kind`
/// colours the confirm button. Honors the modal key contract — Enter = confirm, Esc = cancel — so
/// every confirmation shares one size, layout and button metrics.
pub(crate) fn confirm_modal(
    ctx: &egui::Context,
    id: &str,
    kind: ConfirmKind,
    title: &str,
    message: &str,
    confirm_label: &str,
    cancel_label: &str,
) -> ConfirmOutcome {
    const W: f32 = 360.0;
    let mut confirmed = false;
    let mut cancelled = false;
    let r = show_modal(ctx, id, W, |ui| {
        ui.label(egui::RichText::new(title).size(crate::HEADING_SIZE).strong().color(p().text));
        ui.add_space(crate::SPACE_2);
        ui.label(egui::RichText::new(message).size(crate::BODY_SIZE).color(p().text));
        ui.add_space(crate::SPACE_4);
        // right_to_left → the first widget lands on the far right: confirm is rightmost, cancel to
        // its left, reading [cancel] [confirm] left-to-right.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let bw = uniform_button_width(ui, &[cancel_label, confirm_label]);
            let hit = match kind {
                ConfirmKind::Danger => destructive_button_w(ui, confirm_label, true, bw),
                ConfirmKind::Normal => primary_button_w(ui, confirm_label, true, bw),
            };
            if hit {
                confirmed = true;
            }
            ui.add_space(crate::SPACE_2);
            if secondary_button_w(ui, cancel_label, true, bw) {
                cancelled = true;
            }
        });
    });
    if r.enter {
        confirmed = true;
    }
    if r.escape {
        cancelled = true;
    }
    ConfirmOutcome { confirmed, cancelled }
}

/// Empty-state hint with a real left indent (SPACE_2) instead of leading ASCII spaces, so the
/// hint aligns with the row glyphs in the same island and stays stable under any font.
pub fn empty_hint(ui: &mut egui::Ui, text: &str) {
    let galley = ui.painter().layout(
        text.to_owned(),
        egui::FontId::proportional(crate::theme::BODY_SIZE),
        p().text_dim,
        f32::INFINITY,
    );
    let h = galley.size().y;
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), h),
        egui::Layout::left_to_right(egui::Align::TOP),
        |ui| {
            ui.add_space(crate::SPACE_2);
            ui.label(egui::RichText::new(text).color(p().text_dim));
        },
    );
}

/// A work-area sub-toolbar strip: a chrome band (same fill as the surrounding chrome, no border
/// of its own) holding a centred icon row. top:2 compensates for the work-area sheet's top seam
/// below (1px frame margin + 1px border) — without it the icon row reads a hair high. Shared by
/// the editor, result-panel, connection-manager and connection-tab toolbars.
/// A structural spacer row: an empty `CHROME_GUTTER`-tall strip in the chrome colour. An explicit
/// 4px gap between horizontal strips — used instead of top margins so the gaps don't add up. In the
/// split zone (under the dock) `Panel::top` occupies only the RIGHT part (over the editor), which is
/// exactly what we want.
pub fn vgap(ui: &mut egui::Ui, id: &'static str) {
    egui::Panel::top(id)
        .exact_size(crate::CHROME_GUTTER)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(p().panel2))
        .show(ui, |_ui| {});
}

pub fn subbar(ui: &mut egui::Ui, id: &'static str, left: i8, add: impl FnOnce(&mut egui::Ui)) {
    egui::Panel::top(id)
        .exact_size(crate::SUBBAR_H)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
            // `left` is set by the caller: in the dock — the full gutter (window edge); in the result
            // panel — `dock_left()` (0 under the dock), so the sub-toolbar doesn't drift right of the
            // grid body and the tabs.
            left,
            right: crate::CHROME_GUTTER as i8,
            // 4px gap below the header/tabs. The pill is flat now (no inset), so there's no doubling —
            // this is the only gap above the sub-toolbar.
            top: crate::CHROME_GUTTER as i8,
            bottom: 0,
        }))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                // a single inter-icon gap shared by every toolbar (main/managers/results)
                ui.spacing_mut().item_spacing.x = crate::ICON_GAP;
                add(ui);
            });
        });
}

/// Frameless icon button (size-parameterized): neutral soft box on hover, glyph keeps its colour.
/// The single icon-button engine (size-parameterized). `enabled=false` → inert, dimmed, no
/// hover box (except under DIAG_BOXES); the glyph takes the `disabled` colour and the `color`
/// argument is ignored. Returns a Response (the off wrappers ignore it). There used to be two
/// nearly identical engines.
fn qbtn_glyph(
    ui: &mut egui::Ui,
    icon: &str,
    color: Color32,
    tip: &str,
    glyph: f32,
    enabled: bool,
) -> egui::Response {
    // SQUARE button: side = the row height (≈30 in the main toolbar, ≈22 in the sub-toolbars)
    let h = ui.max_rect().height();
    let size = Vec2::new(h, h);
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    // hover soft box fades in/out (~0.1s) — only for the active one; disabled shows it only under DIAG_BOXES
    let t = if crate::DIAG_BOXES {
        1.0
    } else if enabled {
        ui.ctx().animate_bool(resp.id, resp.hovered())
    } else {
        0.0
    };
    if t > 0.0 {
        let box_rect = rect;
        ui.painter().rect_filled(
            box_rect,
            CornerRadius::same(crate::RADIUS_ICON),
            p().acc_bg.gamma_multiply(t),
        );
    }
    // hover is neutral: the soft box is the affordance, the glyph keeps its colour (accent is
    // reserved for committed/meaningful state, never hover — Design System §2).
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(glyph),
        if enabled { color } else { p().disabled },
    );
    resp.on_hover_text(tip)
}

/// Frameless icon button: neutral soft box on hover, glyph keeps its colour. Fills the row height.
pub fn qbtn(ui: &mut egui::Ui, icon: &str, tip: &str) -> egui::Response {
    qbtn_glyph(ui, icon, p().text, tip, ICON_GLYPH, true)
}

/// Full-size icon button with an explicit glyph colour — for the main toolbar's stateful actions
/// (green Execute, red Stop, amber Download). Same hover box as [`qbtn`].
pub fn qbtn_col(ui: &mut egui::Ui, icon: &str, color: Color32, tip: &str) -> egui::Response {
    qbtn_glyph(ui, icon, color, tip, ICON_GLYPH, true)
}

/// Smaller frameless icon button — for the work-area sub-toolbars.
pub fn qbtn_sm(ui: &mut egui::Ui, icon: &str, color: Color32, tip: &str) -> egui::Response {
    qbtn_glyph(ui, icon, color, tip, SM_ICON_GLYPH, true)
}

/// Toggle icon button: darker background while `active` (pressed), lighter on hover — same
/// accent policy as the tabs / menus. Returns the click response.
pub fn qbtn_toggle(ui: &mut egui::Ui, icon: &str, active: bool, tip: &str) -> egui::Response {
    let h = ui.max_rect().height();
    let size = Vec2::new(h, h); // square, like the other icon buttons
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let box_rect = rect;
    let r = CornerRadius::same(crate::RADIUS_ICON);
    if active {
        // committed state — solid, instant
        ui.painter().rect_filled(box_rect, r, p().acc_bg2);
    } else {
        // hover soft box fades in/out (~0.1s) — cheap, via egui's per-id animation
        let t = if DIAG_BOXES { 1.0 } else { ui.ctx().animate_bool(resp.id, resp.hovered()) };
        if t > 0.0 {
            ui.painter().rect_filled(box_rect, r, p().acc_bg.gamma_multiply(t));
        }
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

/// Disabled (dimmed, inert) icon at the main-toolbar size — the counterpart of [`qbtn`].
pub fn qbtn_off(ui: &mut egui::Ui, icon: &str, tip: &str) {
    qbtn_glyph(ui, icon, p().text, tip, ICON_GLYPH, false);
}

/// Smaller disabled icon — for the work-area sub-toolbars.
pub fn qbtn_off_sm(ui: &mut egui::Ui, icon: &str, tip: &str) {
    qbtn_glyph(ui, icon, p().text, tip, SM_ICON_GLYPH, false);
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
    let h = ui.max_rect().height();
    let size = Vec2::new(h, h); // square: a pair of arrows flush = 2 * row_h (see the reserve at the call sites)
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.hovered() || DIAG_BOXES {
        let box_rect = rect;
        ui.painter().rect_filled(box_rect, CornerRadius::same(crate::RADIUS_ICON), p().acc_bg);
    }
    paint_chevron(ui.painter(), rect, left, false, p().text);
    resp.on_hover_text(tip)
}

/// Small painted close "×" — neutral at rest, `danger` red on hover (destructive action).
/// A square hit-area (side = the row height), like the other icon buttons, so it sits flush in the row.
pub fn close_x(ui: &mut egui::Ui, tip: &str) -> bool {
    const HALF: f32 = 4.0; // half-length of each × arm
    let h = ui.max_rect().height();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(h, h), egui::Sense::click());
    let col = if resp.hovered() { p().danger } else { p().text_dim };
    crate::icons::paint_cross(ui.painter(), rect.center(), HALF, Stroke::new(1.4, col));
    resp.on_hover_text(tip).clicked()
}

/// One tab's leading status mark in [`tab_strip`]: a spinning loader (a query is running on this tab)
/// or a static glyph. `tint` (when `Some`) colours the WHOLE pill — glyph **and** label — to carry a
/// result's state; `None` keeps a neutral glyph and the default active/inactive label colour. The
/// fixed 16px slot is reserved on every tab so the strip never jumps between the two.
pub struct TabMark {
    pub spinning: bool,
    pub glyph: &'static str,
    pub tint: Option<Color32>,
}

/// Pill tabs — the studio signature (Design System v2 §6). Active tab is a pill:
/// `accent_soft` fill, radius 4 rectangle (v2.2 — no pills), `accent_hi` text, blur-2 shadow.
/// Inactive: transparent, `text_dim`, a neutral hover pill. No underline bars.
/// An × shows on the active tab when `closable`.
/// Returns `(selected, closed)` indices.
#[allow(clippy::too_many_arguments)]
pub fn tab_strip(
    ui: &mut egui::Ui,
    labels: &[String],
    active: usize,
    closable: bool,
    marks: Option<&[TabMark]>, // Some → leading status mark per tab (spinner while busy / state glyph)
    gap: f32,                  // inter-tab spacing (0 for the result strip, a touch of air for editors)
    reorderable: bool,         // drag a tab to reorder it (editor tabs)
    scroll_active_into_view: bool, // scroll the active pill into view (the row just changed tab non-click)
) -> (Option<usize>, Option<usize>, Option<(usize, usize)>) {
    ui.spacing_mut().item_spacing.x = gap;
    let mut drag_end: Option<usize> = None; // a tab whose drag just finished (→ reorder on drop)
    let mut centers: Vec<f32> = Vec::with_capacity(labels.len()); // cell-center x, for the drop target
    let h = ui.max_rect().height();
    let font = egui::FontId::proportional(crate::theme::BODY_SIZE);
    let pad = 10.0; // a touch more side padding — pills read better with air around the label
    // fixed-width leading slot for the marker, reserved on every tab so the width never jumps
    let mark_w = if marks.is_some() { 16.0 } else { 0.0 };
    let pill_radius = CornerRadius::same(RADIUS_CONTROL);
    let mut select = None;
    let mut close = None;
    for (i, label) in labels.iter().enumerate() {
        let is_active = i == active;
        let galley = ui.painter().layout_no_wrap(label.clone(), font.clone(), p().text);
        // reserve the close-× width on every closable tab (not just the active one) so the
        // strip doesn't jump when the active tab changes
        let close_w = if closable { 6.0 + 12.0 } else { 0.0 };
        let cell_w = pad + mark_w + galley.size().x + close_w + pad;
        let sense = if reorderable { egui::Sense::click_and_drag() } else { egui::Sense::click() };
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(cell_w, h), sense);
        centers.push(rect.center().x);
        if reorderable {
            if resp.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            if resp.drag_stopped() {
                drag_end = Some(i);
            }
        }
        // the pill fills the whole row: top/bottom air comes from the spacer rows (vgap), not an inset
        let pill_rect = rect;
        if is_active && scroll_active_into_view {
            // keep the active pill in view when the row overflows (open/new/from a manager/Ctrl-Tab).
            // `None` = scroll the minimum needed → no tug when it's already visible.
            resp.scroll_to_me(None);
        }
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
        // glyph + label share ONE colour: a mark's `tint` (an error) colours the whole pill red;
        // otherwise the active tab is normal `text`, inactive `text_dim` — NOT the accent colour, so an
        // active tab never gets confused with a red error tab (the pill background also marks active)
        let mk = marks.and_then(|m| m.get(i));
        let mk_col = mk
            .and_then(|m| m.tint)
            .unwrap_or(if is_active { p().text } else { p().text_dim });
        // leading marker: a small spinning loader while a query runs on this tab, otherwise the glyph
        if marks.is_some() {
            let my = pill_rect.center().y;
            if mk.is_some_and(|m| m.spinning) {
                let tsec = ui.input(|inp| inp.time) as f32;
                // small ring (≈ cap height), right edge aligned to where the rest-state glyph ends so
                // the gap to the label matches the at-rest glyph→label gap
                spinner(ui.painter(), egui::pos2(rect.left() + pad + 6.5, my), 4.5, p().accent_hi, p().border, tsec);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
            } else if let Some(m) = mk {
                ui.painter().text(
                    egui::pos2(rect.left() + pad, my),
                    egui::Align2::LEFT_CENTER,
                    m.glyph,
                    egui::FontId::proportional(12.0),
                    mk_col,
                );
            }
        }
        ui.painter().text(
            egui::pos2(rect.left() + pad + mark_w, pill_rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font.clone(),
            mk_col,
        );
        // close × on every tab (own hit-area so it doesn't trigger a tab switch) — always visible,
        // not just the active one, so it's always one click away (a dirty tab still confirms first)
        let mut close_hit = false;
        if closable {
            let cc = egui::pos2(rect.right() - pad - 6.0, pill_rect.center().y);
            let xr = egui::Rect::from_center_size(cc, Vec2::new(14.0, h));
            let xresp = ui.interact(xr, ui.id().with(("tab_close", i)), egui::Sense::click());
            // match the label colour (normal on the active tab, dim otherwise) — never accent, only
            // danger on hover; so the active tab's × isn't highlighted in the accent colour
            let col = if xresp.hovered() {
                p().danger
            } else if is_active {
                p().text
            } else {
                p().text_dim
            };
            crate::icons::paint_cross(ui.painter(), cc, 3.0, Stroke::new(1.4, col));
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
    // a tab drag has finished → where to insert it (by pointer X relative to the cell centers)
    let mut reorder = None;
    if let Some(from) = drag_end {
        if let Some(px) = ui.input(|i| i.pointer.interact_pos().or(i.pointer.latest_pos())) {
            let to = centers.iter().position(|&cx| px.x < cx).unwrap_or(centers.len());
            // to == from or from+1 — the same position (insert before itself / right after) → don't move
            if to != from && to != from + 1 {
                reorder = Some((from, to));
            }
        }
    }
    (select, close, reorder)
}

/// A one-sector loading ring drawn via `painter` (so it fits a tab-marker slot): a faint full-circle
/// track plus a bright ~0.3-turn arc that rotates with `t` (seconds). The caller must keep requesting
/// repaints while it spins. Hand-rolled because egui's `Painter` cannot rotate a glyph.
pub fn spinner(painter: &egui::Painter, center: Pos2, radius: f32, arc: Color32, track: Color32, t: f32) {
    use std::f32::consts::TAU;
    let w = (hairline(painter.ctx()) * 2.0).max(1.4);
    let ring: Vec<Pos2> = (0..=40)
        .map(|k| {
            let a = (k as f32 / 40.0) * TAU;
            center + Vec2::new(a.cos(), a.sin()) * radius
        })
        .collect();
    painter.add(egui::Shape::line(ring, Stroke::new(w, track)));
    let a0 = (t * 2.6) % TAU;
    let sweep = TAU * 0.3;
    let arc_pts: Vec<Pos2> = (0..=14)
        .map(|k| {
            let a = a0 + sweep * (k as f32 / 14.0);
            center + Vec2::new(a.cos(), a.sin()) * radius
        })
        .collect();
    painter.add(egui::Shape::line(arc_pts, Stroke::new(w, arc)));
}

/// The "Running" pill centred over an (empty / still-loading) result area: accent-filled like the
/// primary button, non-interactive, with animated trailing dots. Sized to the widest state so the
/// dots don't jitter it. The caller must keep requesting repaints while it shows.
pub fn running_overlay(ui: &mut egui::Ui, area: egui::Rect, t: f32) {
    let dots = ".".repeat(((t * 2.0) as usize) % 4);
    let font = crate::theme::ui_bold_font(13.0);
    let painter = ui.painter().clone();
    let g = painter.layout_no_wrap("Running...".to_owned(), font.clone(), p().on_accent);
    let pad = Vec2::new(14.0, 3.0);
    let rect = snap_rect(&painter, egui::Rect::from_center_size(area.center(), g.size() + pad * 2.0));
    painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), p().accent);
    painter.text(
        egui::pos2(rect.left() + pad.x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("Running{dots}"),
        font,
        p().on_accent,
    );
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
}

/// A clickable status-bar chip (version / scan): the text plus a hover pill so it reads as a button.
/// Keeps the plain arrow cursor (the chip sits over the window-edge resize strip). Returns the click
/// response so the caller can attach a tooltip and act on `.clicked()`.
pub fn chip_button(ui: &mut egui::Ui, text: &str, color: egui::Color32, sz: f32) -> egui::Response {
    let font = egui::FontId::proportional(sz);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let padding = Vec2::new(6.0, 1.0);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + padding * 2.0, egui::Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), p().hover);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// White result/editing sheet (field_bg fill, soft shadow). See [`island_panel`].
pub fn island<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    island_panel(ui, p().field_bg, add)
}

/// A rounded, shadowed island hosting (scrollable) content. The shadow and the `fill` go UNDER the
/// content and the crisp 1px border goes ON TOP of it — all keyed to ONE pixel-snapped rect, so:
///   * the border covers any content fill that runs to the edge (no white halo / corner triangles);
///   * fill and border trace the SAME snapped rounded path, so there is no corner seam (the trap the
///     old "snap only the overlay border over an unsnapped Frame fill" pattern fell into);
///   * straight edges are razor-sharp on the device grid at any DPI.
///
/// The content closure runs inside a frameless `Frame` (we paint the fill/shadow ourselves, snapped).
pub fn island_panel<R>(
    ui: &mut egui::Ui,
    fill: Color32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let r = CornerRadius::same(RADIUS_ISLAND);
    // Reserve the shadow + fill slots BEFORE the content so they paint beneath it; backfill them
    // once the content's rect (and thus the snapped rect) is known.
    let sh = ui.painter().add(egui::Shape::Noop);
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner = egui::Frame::new()
        .corner_radius(r)
        .inner_margin(Margin::ZERO)
        .show(ui, add);
    let rect = snap_rect(ui.painter(), inner.response.rect);
    ui.painter().set(sh, crate::theme::island_shadow().as_shape(rect, r));
    ui.painter().set(bg, egui::Shape::rect_filled(rect, r, fill));
    crisp_border(ui.painter(), rect, p().border_strong);
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
            |ui| ui.label(egui::RichText::new(label).color(p().text_dim).size(crate::LABEL_SIZE)),
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

/// Width of a physically-crisp hairline: exactly ONE device pixel expressed in logical units. A
/// `Stroke` this wide, laid on a pixel-snapped path, renders as a single hard device pixel at any
/// DPI — unlike a `1.0` *logical* stroke, which smears across ~1.5 device pixels at 125/150% scale.
pub fn hairline(ctx: &egui::Context) -> f32 {
    1.0 / ctx.pixels_per_point()
}

/// Stroke-only overlay border (focus/danger rings, the square window outline, selection
/// frames). Crisp-1px experiment (V1): the rect is snapped to whole device pixels and the stroke is
/// exactly one device pixel ([`hairline`]) with `StrokeKind::Inside`, so straight edges land razor
/// sharp on the grid while feathering keeps the rounded corners smooth. Static island borders that
/// sit over a SEPARATE fill must key both to the SAME snapped rect (see [`island_panel`]) — pairing
/// fill+stroke in one shape ([`island_box`]) is the seam-free alternative.
pub fn crisp_border(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    crisp_border_r(painter, rect, color, crate::theme::RADIUS_ISLAND);
}

/// [`crisp_border`] with an explicit radius (0 = the square window outline).
pub fn crisp_border_r(painter: &egui::Painter, rect: egui::Rect, color: Color32, radius: u8) {
    let rect = snap_rect(painter, rect);
    painter.rect_stroke(
        rect,
        CornerRadius::same(radius),
        Stroke::new(hairline(painter.ctx()), color),
        egui::StrokeKind::Inside,
    );
}

/// An island/field/popup body: fill + a 1-device-px inside stroke as ONE `RectShape` (Design Delta
/// v2.2 §3) — the single-shape law kills the seams the split fill/stroke passes used to leave
/// around rounded corners. The rect is pixel-snapped so the fill edge AND the border are crisp.
/// Shadow (if any) goes UNDER this via [`island_shadow_under`].
pub fn island_box(painter: &egui::Painter, rect: egui::Rect, fill: Color32, radius: u8) {
    let rect = snap_rect(painter, rect);
    painter.add(egui::epaint::RectShape::new(
        rect,
        CornerRadius::same(radius),
        fill,
        Stroke::new(hairline(painter.ctx()), p().border_strong),
        egui::StrokeKind::Inside,
    ));
}

/// Measure a button's size the way it will actually be painted. `bold` selects the label font so
/// the measured width matches what the bold families (primary/destructive) need — measuring with the
/// regular font starved the wider bold glyphs of their side padding.
fn button_size(ui: &egui::Ui, label: &str, bold: bool) -> Vec2 {
    let font = if bold {
        crate::theme::ui_bold_font(13.0)
    } else {
        egui::FontId::proportional(crate::theme::BODY_SIZE)
    };
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, p().text);
    // unified controls: CONTROL_H tall, 14px side padding (Design Delta v2.2 §4)
    Vec2::new(galley.size().x + 14.0 * 2.0, crate::theme::CONTROL_H)
}

/// The widest a button needs to be to fit any of `labels` at the standard geometry. Use it to give
/// every button on one modal the same width (Design System §7 Modals: uniform, right-aligned, at
/// the bottom) — measure the modal's labels once, then render each with the `*_button_w` variant.
///
/// Measures each label with BOTH the regular and the bold font and takes the max, so the width fits
/// whichever family actually paints the widest label (primary/destructive paint bold).
pub fn uniform_button_width(ui: &egui::Ui, labels: &[&str]) -> f32 {
    labels.iter().fold(crate::theme::CONTROL_H, |w, l| {
        w.max(button_size(ui, l, false).x).max(button_size(ui, l, true).x)
    })
}

/// The single filled (accent) button a dialog is allowed: white text, [`RADIUS_CONTROL`], and
/// [`ACCENT_PRESS`] while held. Sizes to its label. Returns true on click.
pub fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    primary_button_w(ui, label, enabled, button_size(ui, label, true).x)
}

/// The shared core of the two filled modal buttons (primary / destructive): identical geometry, text
/// (`on_accent`, bold) and disabled look; only the rest / hover / press fill triple differs.
fn filled_button_w(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    width: f32,
    rest: Color32,
    hover: Color32,
    press: Color32,
) -> bool {
    let size = Vec2::new(width, crate::theme::CONTROL_H);
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if !enabled {
        paint_disabled_button(ui.painter(), rect, label);
        return false;
    }
    let fill = if resp.is_pointer_button_down_on() {
        press
    } else if resp.hovered() {
        hover
    } else {
        rest
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
    resp.clicked()
}

/// [`primary_button`] at an explicit width (for uniform modal button bars). Accent fill, darkening
/// on hover/press.
pub fn primary_button_w(ui: &mut egui::Ui, label: &str, enabled: bool, width: f32) -> bool {
    filled_button_w(
        ui,
        label,
        enabled,
        width,
        p().accent,
        crate::theme::tint(p().accent, Color32::BLACK, 0.10),
        p().accent_press,
    )
}

/// One inactive-button look, shared by every button kind (Design System: an inactive button reads
/// like the disabled **Apply** on the Scan page — light field fill, 1px border, grey text; never a
/// heavy solid-grey fill).
fn paint_disabled_button(painter: &egui::Painter, rect: egui::Rect, label: &str) {
    island_box(painter, rect, p().field_bg, RADIUS_CONTROL);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(crate::theme::BODY_SIZE),
        p().disabled,
    );
}

/// Destructive primary (Design Delta v2.1 §5): the confirming button of a destructive modal —
/// `danger` fill, `on_accent` text, the exact primary geometry. A modal carries either a primary
/// OR a destructive primary, never both. Width is explicit (for uniform modal button bars).
/// Returns true on click.
pub fn destructive_button_w(ui: &mut egui::Ui, label: &str, enabled: bool, width: f32) -> bool {
    filled_button_w(
        ui,
        label,
        enabled,
        width,
        p().danger,
        crate::theme::tint(p().danger, Color32::BLACK, 0.10),
        crate::theme::tint(p().danger, Color32::BLACK, 0.22),
    )
}

/// Outline (secondary) button: white fill, 1px `border_strong`, text colour, neutral `hover` fill.
/// Sizes to its label. Returns true on click.
pub fn secondary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    // measure bold (matches how it's painted) so the auto-width isn't starved by the bold glyphs
    secondary_button_w(ui, label, enabled, button_size(ui, label, true).x)
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
        crate::theme::ui_bold_font(13.0), // bold — same weight as primary/destructive in a uniform bar
        text_col,
    );
    enabled && resp.clicked()
}

/// A bare single-line input sized to the shared field height, with the accent focus ring drawn
/// over its border when it holds keyboard focus (Design System §6 Text fields). The caller paints
/// the label and the gap; this is just the field, so spacing comes from the `SPACE_*` scale.
pub fn focus_field(ui: &mut egui::Ui, value: &mut String, password: bool, width: f32) -> egui::Response {
    let h = crate::theme::FIELD_H; // shared field height so a form's controls line up exactly
    let mut te = egui::TextEdit::singleline(value)
        .desired_width(width)
        // the canonical field inset (never hugs the rounding) + vertical centring, so the text sits
        // dead-centre like every combo / list row — one knob in theme.rs
        .margin(crate::theme::field_margin())
        .vertical_align(egui::Align::Center);
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
/// which was already cloned. Call before `Panel::…show(ui, …)`, then `ui.set_style(saved)`.
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
    // Solid scrollbars: always visible, pinned to the edge, taking up their own space
    // (they don't cover the content or overlap in the corner).
    st.spacing.scroll.floating = false;
    st.spacing.scroll.bar_width = 8.0;
    st.spacing.scroll.bar_inner_margin = 0.0;
    st.spacing.scroll.bar_outer_margin = 0.0;
    st.spacing.scroll.dormant_background_opacity = 0.0;
    st.spacing.scroll.dormant_handle_opacity = 1.0;
    st.spacing.scroll.active_background_opacity = 0.0;
    st.spacing.scroll.interact_background_opacity = 0.0;
    st.spacing.scroll.active_handle_opacity = 1.0;
    st.spacing.scroll.interact_handle_opacity = 1.0;
}

/// Scrollbar style for the MANAGER LISTS (Connection / Metadata / Model managers): a FLOATING
/// overlay bar. Unlike the solid [`style_scrollbar`], it paints OVER the content and reserves no
/// width (`floating_allocated_width = 0`), so:
///   * list rows fill edge-to-edge — the selection/hover accent reaches the frame instead of being
///     cut short by a reserved gutter;
///   * a bar appearing/disappearing never reflows the rows (`current_bar_use` stays 0), so the
///     manager window never jitters when the list starts/stops overflowing;
///   * egui's edge fade-gradient (re-enabled here; it's globally off) spans the full width instead
///     of cutting off at the bar.
/// `floating_width == bar_width` keeps the bar a constant 8px (no thin-when-idle pill), and full
/// handle opacity keeps it visible whenever there's something to scroll.
pub fn style_scrollbar_overlay(ui: &mut egui::Ui) {
    style_scrollbar(ui); // muted handle colours + sharp corners; then flip to a floating overlay
    let sc = &mut ui.style_mut().spacing.scroll;
    sc.floating = true;
    sc.floating_width = 8.0;
    sc.floating_allocated_width = 0.0; // reserve NO width → no reflow, accent reaches the frame
    sc.fade = Default::default(); // bring back egui's edge fade (overlay → no cut-off at the bar)
}

// (The app logo lives in the per-project `crate::brand` — see brand::logo / brand::paint_logo.)

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
            let row_h = MGR_ROW_H; // every selectable row (lists, tree, combo popup) shares one height
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
                ui.painter().text(
                    egui::pos2(r.left() + crate::theme::TEXT_INSET, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    it,
                    egui::FontId::proportional(crate::theme::BODY_SIZE),
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
    enabled && resp.on_hover_text(tip).clicked()
}

// Shared metrics for the side-dock manager lists (Connection / Metadata, and the future Git /
// File managers) so every manager renders rows identically.
pub const MGR_ROW_H: f32 = 24.0; // row height
const MGR_LPAD: f32 = crate::theme::TEXT_INSET; // left padding before the leading glyph (the shared text inset)
/// Glyph-column width (chevron OR type icon); the label starts after it. Also the indent step:
/// a child row passes `indent = MGR_GLYPH_COL` so its icon lines up under the parent's label.
pub const MGR_GLYPH_COL: f32 = 20.0;
/// Screen x (relative to a row's left) where a manager row's LABEL text begins: the left pad plus the
/// glyph column. Single-sources the position so an inline rename editor can align its first glyph to
/// exactly where the static label sits (see `connections_ui`); moves with [`crate::theme::TEXT_INSET`].
pub const MGR_LABEL_X: f32 = MGR_LPAD + MGR_GLYPH_COL;
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
    manager_row_fg(ui, indent, glyph, label, selected, None)
}

/// Like [`manager_row`] but with an optional foreground colour for the glyph + label — used to mark
/// the live/active connection green (`Some(p().ok)`). `None` keeps the default text colours.
pub fn manager_row_fg(
    ui: &mut egui::Ui,
    indent: f32,
    glyph: &str,
    label: &str,
    selected: bool,
    fg: Option<Color32>,
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
            fg.unwrap_or(if selected { p().text } else { p().text_dim }),
        );
    }
    if !label.is_empty() {
        ui.painter().text(
            egui::pos2(x0 + MGR_GLYPH_COL, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(MGR_LABEL_SIZE),
            fg.unwrap_or(p().text),
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
    // leave room for the left text inset and the down-arrow gutter (~16)
    let sel_text =
        truncate_to_width(ui, &sel_full, font_size, (width - crate::theme::TEXT_INSET - 16.0).max(0.0));
    pt.text(
        egui::pos2(rect.left() + crate::theme::TEXT_INSET, rect.center().y),
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
    if resp.clicked() {
        open = !open;
    }

    // ---- open list: our own Area, anchored to the field's bottom-left, exactly `width` wide ----
    if open && !options.is_empty() {
        let row_h = MGR_ROW_H; // same height as every other selectable row
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
                            let label = truncate_to_width(ui, o, font_size, (rr.width() - 2.0 * crate::theme::TEXT_INSET).max(0.0));
                            ui.painter().text(
                                egui::pos2(rr.left() + crate::theme::TEXT_INSET, rr.center().y),
                                egui::Align2::LEFT_CENTER,
                                &label,
                                egui::FontId::proportional(font_size),
                                p().text,
                            );
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
/// row. The widget itself is only the line (1px); the air on both sides comes from the toolbar's
/// `item_spacing` (= [`crate::ICON_GAP`]), so the gap before/after the divider matches the gap
/// between icons.
pub fn toolbar_divider(ui: &mut egui::Ui) {
    let h = ui.max_rect().height();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, h), egui::Sense::hover());
    let cx = ui.painter().round_to_pixel_center(rect.center().x);
    let cy = rect.center().y;
    let half = 8.0; // 16px tall
    ui.painter().vline(
        cx,
        (cy - half)..=(cy + half),
        Stroke::new(1.0 / ui.ctx().pixels_per_point(), p().border),
    );
}
