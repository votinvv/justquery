//!
//! Собственные скроллбары для виртуальной прокрутки в локальных координатах.
//!
//! egui-ScrollArea строит гигантское виртуальное полотно (строки × высота), а все координаты
//! egui — f32: на сотнях миллионов пикселей шаг представления достигает 32px — строки
//! налезают друг на друга, появляются пустые ряды, клики мажут. Поэтому редактор и грид
//! скроллят сами: позиция — f64-пиксели от начала контента, рисуется только видимое окно,
//! больших координат не существует. Здесь — отрисовка и взаимодействие самих полос.

use crate::theme::p;
use eframe::egui;
use egui::{CornerRadius, Rect, Vec2};

/// Толщина полосы прокрутки (солидная, прижата к краю, место резервируется).
pub const BAR: f32 = 8.0;

/// Ось полосы: вертикальная (скроллит Y) или горизонтальная (скроллит X).
#[derive(Clone, Copy)]
enum Axis {
    Vertical,
    Horizontal,
}

impl Axis {
    /// Длина трека по этой оси (f64).
    #[inline]
    fn track_len(self, track: Rect) -> f64 {
        match self {
            Axis::Vertical => track.height() as f64,
            Axis::Horizontal => track.width() as f64,
        }
    }
    /// Координата указателя по этой оси относительно старта трека (f32).
    #[inline]
    fn pointer(self, pp: egui::Pos2, track: Rect) -> f32 {
        match self {
            Axis::Vertical => pp.y - track.top(),
            Axis::Horizontal => pp.x - track.left(),
        }
    }
}

/// Полоса прокрутки в треке `track`. `offset` — f64-пиксели, клампится к
/// `[0, content - view]`. Перетаскивание/клик — абсолютный маппинг (центр хэндла
/// к указателю): предсказуемо на любых длинах. Одно правило для вертикальной и
/// горизонтальной полос (отличаются только осью).
fn bar(
    ui: &mut egui::Ui,
    track: Rect,
    id: egui::Id,
    offset: &mut f64,
    content: f64,
    axis: Axis,
) {
    let view = axis.track_len(track);
    let max_off = (content - view).max(0.0);
    *offset = offset.clamp(0.0, max_off);
    if max_off <= 0.0 {
        return; // влезает целиком — полоса не нужна (место уже зарезервировано)
    }
    let frac = (view / content).clamp(0.05, 1.0) as f32;
    let track_len = view as f32;
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
    ui.painter().rect_filled(handle, CornerRadius::same(4), color);
}

/// Вертикальная полоса (скроллит `offset.y`). См. [`bar`].
pub fn vbar(ui: &mut egui::Ui, track: Rect, id: egui::Id, offset: &mut f64, content_h: f64) {
    bar(ui, track, id, offset, content_h, Axis::Vertical);
}

/// Горизонтальная полоса (скроллит `offset.x`). См. [`bar`].
pub fn hbar(ui: &mut egui::Ui, track: Rect, id: egui::Id, offset: &mut f64, content_w: f64) {
    bar(ui, track, id, offset, content_w, Axis::Horizontal);
}

/// Скролл-дельта колеса/тачпада этого кадра, если указатель над `rect`
/// (kinetic.rs инжектит те же wheel-события — работает прозрачно).
pub fn wheel_delta(ui: &egui::Ui, rect: Rect) -> Vec2 {
    if !ui.rect_contains_pointer(rect) {
        return Vec2::ZERO;
    }
    ui.ctx().input(|i| {
        let mut d = i.smooth_scroll_delta;
        // shift+колесо → горизонталь (если egui ещё не поменял оси)
        if i.modifiers.shift && d.x == 0.0 {
            d = Vec2::new(d.y, 0.0);
        }
        d
    })
}
