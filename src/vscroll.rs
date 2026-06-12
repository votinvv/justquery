//! SHARED pedant↔justquery — править синхронно (список общих файлов — в README).
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

/// Вертикальная полоса в треке `track`. `offset` — f64-пиксели, клампится к
/// `[0, content_h - view_h]`. Перетаскивание/клик — абсолютный маппинг (центр хэндла
/// к указателю): предсказуемо на любых длинах.
pub fn vbar(ui: &mut egui::Ui, track: Rect, id: egui::Id, offset: &mut f64, content_h: f64) {
    let view_h = track.height() as f64;
    let max_off = (content_h - view_h).max(0.0);
    *offset = offset.clamp(0.0, max_off);
    if max_off <= 0.0 {
        return; // влезает целиком — полоса не нужна (место уже зарезервировано)
    }
    let frac = (view_h / content_h).clamp(0.05, 1.0) as f32;
    let len = (track.height() * frac).max(24.0).min(track.height());
    let range = (track.height() - len).max(1.0);

    let resp = ui.interact(track, id, egui::Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(pp) = resp.interact_pointer_pos() {
            let t = ((pp.y - track.top() - len * 0.5) / range).clamp(0.0, 1.0);
            *offset = t as f64 * max_off;
        }
    }
    let pos = ((*offset / max_off) as f32 * range).clamp(0.0, range);
    let handle = Rect::from_min_size(
        egui::pos2(track.left(), track.top() + pos),
        Vec2::new(track.width(), len),
    );
    let color = if resp.dragged() {
        p().scroll_pressed
    } else if resp.hovered() {
        p().scroll_hot
    } else {
        p().scroll_dormant
    };
    ui.painter().rect_filled(handle, CornerRadius::same(4), color);
}

/// Горизонтальная полоса — та же f64-арифметика, что и вертикальная (системно).
pub fn hbar(ui: &mut egui::Ui, track: Rect, id: egui::Id, offset: &mut f64, content_w: f64) {
    let view_w = track.width() as f64;
    let max_off = (content_w - view_w).max(0.0);
    *offset = offset.clamp(0.0, max_off);
    if max_off <= 0.0 {
        return;
    }
    let frac = (view_w / content_w).clamp(0.05, 1.0) as f32;
    let len = (track.width() * frac).max(24.0).min(track.width());
    let range = (track.width() - len).max(1.0);

    let resp = ui.interact(track, id, egui::Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(pp) = resp.interact_pointer_pos() {
            let t = ((pp.x - track.left() - len * 0.5) / range).clamp(0.0, 1.0);
            *offset = t as f64 * max_off;
        }
    }
    let pos = (((*offset / max_off) as f32) * range).clamp(0.0, range);
    let handle = Rect::from_min_size(
        egui::pos2(track.left() + pos, track.top()),
        Vec2::new(len, track.height()),
    );
    let color = if resp.dragged() {
        p().scroll_pressed
    } else if resp.hovered() {
        p().scroll_hot
    } else {
        p().scroll_dormant
    };
    ui.painter().rect_filled(handle, CornerRadius::same(4), color);
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
