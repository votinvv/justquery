//!
//! Central place for the app's look: palettes, metrics, fonts and the egui style.
//! Everything visual is defined here so the rest of the code refers to semantic colours
//! instead of raw hex values.
//!
//! ── v2 «Тёплая студия» ───────────────────────────────────────────────────────────────
//! What changed vs v1:
//!   1. TWO palettes: a warm-shifted LIGHT and the new DARK ("Warm Studio"), switchable at
//!      runtime via [`set_theme`] / read via [`p()`].
//!   2. The accent moved from PostgreSQL blue to CORAL — it now matches the app logo. Blue
//!      survives as a wink in `syn_fn` (functions in SQL highlight). Accent is still used
//!      ONLY for meaning: primary action, selection, focus, caret, links. Hover stays neutral.
//!   3. Thin DBVis-style controls: unified 22px ([`CONTROL_H`] = [`BTN_H`] = [`FIELD_H`]).
//!   4. Soft depth: subtle shadows on islands/menus/modals ([`island_shadow`]); a single 4px
//!      corner radius for islands, controls and modals ([`RADIUS_ISLAND`] = [`RADIUS_CONTROL`]).
//!   5. New semantic fields: `field_bg`, `accent_hi`, `accent_press`, `accent_soft`,
//!      `on_accent`, `shadow`.

use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use egui::{Color32, CornerRadius, Margin, Stroke, Vec2};

/// A complete set of semantic UI colours (one theme).
#[derive(Clone, Copy)]
pub struct Palette {
    // surfaces
    pub ivory: Color32,   // raised surfaces: islands, modals, popups, menus
    pub panel: Color32,   // app chrome fill (caption, menu, toolbar, tab strip, side panels)
    pub panel2: Color32,
    pub grid_header: Color32,
    pub data_bg: Color32, // data-bearing backdrops (status bar, form backdrop)
    pub row_alt: Color32, // zebra striping
    pub field_bg: Color32, // interiors of text edits / combos AND the editor background
    // borders
    pub border: Color32,        // soft inner dividers
    pub border_strong: Color32, // THE canonical 1px frame
    pub menu_border: Color32,
    pub divider: Color32,
    // text
    pub text: Color32,
    pub text_dim: Color32,
    pub disabled: Color32,
    // accent (coral) — meaning only: primary, selected, focused, caret, links
    pub accent: Color32,       // fills: primary button, selection bars, active-tab underline
    pub accent_hi: Color32,    // lines/text/icons in accent (≥ readable on `panel`)
    pub accent_press: Color32, // primary button pressed
    pub accent_soft: Color32,  // tinted pill/chip background (active tab, version chip)
    pub on_accent: Color32,    // text on `accent` fill
    // neutral interaction states
    pub hover: Color32,
    pub select: Color32, // selected row / strong tinted background
    pub acc_bg: Color32,
    pub acc_bg2: Color32,
    // status
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
    // scrollbars / grips
    pub scroll_dormant: Color32,
    pub scroll_hot: Color32,
    pub scroll_pressed: Color32,
    pub grip_hot: Color32,
    // misc
    pub find_hl: Color32,
    pub gutter: Color32,
    pub active_line: Color32,
    pub editor_sel: Color32,
    pub shadow: Color32, // colour of the soft island shadow
    // SQL syntax
    pub syn_kw: Color32,
    pub syn_str: Color32,
    pub syn_com: Color32,
    pub syn_num: Color32,
    pub syn_fn: Color32, // stays BLUE in both themes — the elephant's wink
}

/// Warm light: silvery base shifted toward warm paper so it's family with the dark studio.
pub const LIGHT: Palette = Palette {
    // SURFACE = #FDFCF9, CHROME = #F1EEE9
    ivory: Color32::from_rgb(0xfd, 0xfc, 0xf9),       // SURFACE
    panel: Color32::from_rgb(0xf1, 0xee, 0xe9),       // CHROME
    panel2: Color32::from_rgb(0xf1, 0xee, 0xe9),      // CHROME
    grid_header: Color32::from_rgb(0xf1, 0xee, 0xe9), // CHROME
    data_bg: Color32::from_rgb(0xf1, 0xee, 0xe9),     // CHROME
    row_alt: Color32::from_rgb(0xf7, 0xf5, 0xf1),     // производная (зебра)
    field_bg: Color32::from_rgb(0xfd, 0xfc, 0xf9),    // SURFACE

    border: Color32::from_rgb(0xdc, 0xd8, 0xd1),
    border_strong: Color32::from_rgb(0xc5, 0xbf, 0xb6),
    menu_border: Color32::from_rgb(0xc5, 0xbf, 0xb6),
    divider: Color32::from_rgb(0xdc, 0xd8, 0xd1),

    text: Color32::from_rgb(0x2a, 0x27, 0x23),
    text_dim: Color32::from_rgb(0x8f, 0x89, 0x7f),
    disabled: Color32::from_rgb(0xc9, 0xc3, 0xba),

    accent: Color32::from_rgb(0xc0, 0x5a, 0x33),
    accent_hi: Color32::from_rgb(0xb5, 0x52, 0x2d),
    accent_press: Color32::from_rgb(0xa8, 0x4a, 0x28),
    accent_soft: Color32::from_rgb(0xf5, 0xe5, 0xdc),
    on_accent: Color32::from_rgb(0xff, 0xff, 0xff),

    hover: Color32::from_rgb(0xe6, 0xe1, 0xda),  // читается и на SURFACE, и на CHROME
    select: Color32::from_rgb(0xf3, 0xdd, 0xd2),
    acc_bg: Color32::from_rgb(0xe6, 0xe1, 0xda),
    acc_bg2: Color32::from_rgb(0xdd, 0xd7, 0xcf),

    ok: Color32::from_rgb(0x61, 0x86, 0x3c),
    warn: Color32::from_rgb(0xc2, 0x8a, 0x1a),
    danger: Color32::from_rgb(0xb7, 0x35, 0x2a),

    scroll_dormant: Color32::from_rgb(0xc9, 0xc4, 0xbb),
    scroll_hot: Color32::from_rgb(0xaf, 0xa9, 0xa0),
    scroll_pressed: Color32::from_rgb(0x9d, 0x97, 0x8d),
    grip_hot: Color32::from_rgb(0xbc, 0xb6, 0xac),

    find_hl: Color32::from_rgba_premultiplied(0x8a, 0x6a, 0x14, 0x55),
    gutter: Color32::from_rgb(0xf1, 0xee, 0xe9),       // = CHROME (panel): the two-colour pair, no third tone
    active_line: Color32::from_rgb(0xfa, 0xef, 0xe6),
    editor_sel: Color32::from_rgb(0xf4, 0xd8, 0xc8),
    shadow: Color32::from_rgba_premultiplied(4, 3, 2, 20),

    syn_kw: Color32::from_rgb(0xa0, 0x4a, 0x26),
    syn_str: Color32::from_rgb(0x5b, 0x7a, 0x45),
    syn_com: Color32::from_rgb(0xa8, 0xa1, 0x91),
    syn_num: Color32::from_rgb(0x8a, 0x6d, 0x2f),
    syn_fn: Color32::from_rgb(0x3f, 0x6f, 0x9f),
};

/// «Тёплая студия»: warm brown-graphite, layered surfaces, coral that glows out of the base.
pub const DARK: Palette = Palette {
    // SURFACE = #2B2622 (приглушённый тёплый контент), CHROME = #1F1B18 (чернота)
    ivory: Color32::from_rgb(0x2b, 0x26, 0x22),       // SURFACE
    panel: Color32::from_rgb(0x1f, 0x1b, 0x18),       // CHROME
    panel2: Color32::from_rgb(0x1f, 0x1b, 0x18),      // CHROME
    grid_header: Color32::from_rgb(0x1f, 0x1b, 0x18), // CHROME
    data_bg: Color32::from_rgb(0x1f, 0x1b, 0x18),     // CHROME
    row_alt: Color32::from_rgb(0x26, 0x22, 0x1f),     // производная (зебра)
    field_bg: Color32::from_rgb(0x2b, 0x26, 0x22),    // SURFACE

    border: Color32::from_rgb(0x3a, 0x34, 0x2e),
    border_strong: Color32::from_rgb(0x45, 0x3e, 0x37),
    menu_border: Color32::from_rgb(0x45, 0x3e, 0x37),
    divider: Color32::from_rgb(0x3a, 0x34, 0x2e),

    text: Color32::from_rgb(0xd5, 0xce, 0xc3),
    text_dim: Color32::from_rgb(0x9d, 0x95, 0x8a),
    disabled: Color32::from_rgb(0x61, 0x5a, 0x50),

    accent: Color32::from_rgb(0xd9, 0x77, 0x57),
    accent_hi: Color32::from_rgb(0xe5, 0x8d, 0x69),
    accent_press: Color32::from_rgb(0xc2, 0x63, 0x44),
    accent_soft: Color32::from_rgb(0x3c, 0x2e, 0x26),
    on_accent: Color32::from_rgb(0xfb, 0xf2, 0xec),

    hover: Color32::from_rgb(0x33, 0x2e, 0x28), // == acc_bg: one neutral-hover colour (matches Light)
    select: Color32::from_rgb(0x42, 0x31, 0x26),
    acc_bg: Color32::from_rgb(0x33, 0x2e, 0x28),
    acc_bg2: Color32::from_rgb(0x3b, 0x35, 0x2e),

    ok: Color32::from_rgb(0x9d, 0xbe, 0x7b),
    warn: Color32::from_rgb(0xd6, 0xa6, 0x5a),
    danger: Color32::from_rgb(0xe2, 0x62, 0x5a),

    // притушены (с автоскрытием они появляются редко и не должны блестеть)
    scroll_dormant: Color32::from_rgb(0x45, 0x3e, 0x36),
    scroll_hot: Color32::from_rgb(0x52, 0x4a, 0x41),
    scroll_pressed: Color32::from_rgb(0x5f, 0x56, 0x4c),
    grip_hot: Color32::from_rgb(0x4a, 0x42, 0x39),

    find_hl: Color32::from_rgba_premultiplied(58, 41, 15, 64),
    gutter: Color32::from_rgb(0x1f, 0x1b, 0x18),       // = CHROME (panel): the two-colour pair, no third tone
    active_line: Color32::from_rgb(0x33, 0x2b, 0x24),
    editor_sel: Color32::from_rgb(0x50, 0x3a, 0x2c),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 71),

    syn_kw: Color32::from_rgb(0xe0, 0x8a, 0x66),
    syn_str: Color32::from_rgb(0x9c, 0xbf, 0x7a),
    syn_com: Color32::from_rgb(0x75, 0x6d, 0x60),
    syn_num: Color32::from_rgb(0xcf, 0xaf, 0x72),
    syn_fn: Color32::from_rgb(0x7f, 0xa9, 0xd0),
};

// ── Runtime theme ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppTheme {
    Light,
    Dark,
}

static DARK_MODE: AtomicBool = AtomicBool::new(false);

/// The active palette. All painting code should read colours through this.
#[inline]
pub fn p() -> &'static Palette {
    if DARK_MODE.load(Ordering::Relaxed) { &DARK } else { &LIGHT }
}

pub fn current_theme() -> AppTheme {
    if DARK_MODE.load(Ordering::Relaxed) { AppTheme::Dark } else { AppTheme::Light }
}

/// Switch theme at runtime and re-apply the egui style. Persisting the choice is the
/// caller's job (`save_theme` in main.rs).
pub fn set_theme(ctx: &egui::Context, t: AppTheme) {
    DARK_MODE.store(matches!(t, AppTheme::Dark), Ordering::Relaxed);
    apply(ctx, p());
    ctx.request_repaint();
}

// ── Style metrics ─────────────────────────────────────────────────────────────────────

/// Shared height of every chrome row (caption/menu, toolbar, tab rows). The equal-gap math
/// in the chrome depends on these being identical — do not change casually.
pub const CAPTION_H: f32 = 30.0;
pub const TABBAR_H: f32 = 30.0;
pub const CHROME_PAD: f32 = 4.0;
pub const SUBBAR_H: f32 = 26.0;
pub const DIAG_BOXES: bool = false;
/// Proportional UI text size (body): tabs, buttons, list rows, status bar, form labels. The single
/// source for the `13.0` that ~9 sites used as a literal.
pub const BODY_SIZE: f32 = 13.0;
pub const CODE_SIZE: f32 = 13.0;
/// Result-grid font size. A touch smaller than the editor (`CODE_SIZE`): dense data grids read
/// cleaner at 12pt, and the column-width heuristic `chars * 7.0 + 18.0` is calibrated to it.
pub const GRID_SIZE: f32 = 12.0;

// Unified control geometry (Design Delta v2.2 §4): EVERY single-line control — buttons,
// fields, combos, menu rows — is exactly this tall, radius RADIUS_CONTROL.
// 22px — компактные контролы (единое значение обоих проектов).
pub const CONTROL_H: f32 = 22.0;
/// Alias of [`CONTROL_H`] (v2.2 merged the button height into the unified control height).
pub const BTN_H: f32 = CONTROL_H;
/// Alias of [`CONTROL_H`] (v2.2 merged the field height into the unified control height).
pub const FIELD_H: f32 = CONTROL_H;

// Corner radii (egui 0.34: CornerRadius is u8)
// Единый радиус 4 везде (кнопки, острова, модалки).
pub const RADIUS_CONTROL: u8 = 4;
pub const RADIUS_ISLAND: u8 = 4;

// Spacing scale — use these instead of magic numbers
pub const SPACE_1: f32 = 4.0;
pub const SPACE_2: f32 = 8.0;
pub const SPACE_3: f32 = 12.0;
pub const SPACE_4: f32 = 16.0;
pub const SPACE_5: f32 = 24.0;

/// Единый горизонтальный gutter ВСЕГО главного экрана: края окна у caption/toolbar/tabs/status,
/// островки доков, остров редактора и зазор между вкладками — все «полосы» одного размера.
/// Одно значение → одна точка настройки плотности интерфейса.
pub const CHROME_GUTTER: f32 = SPACE_1;

/// Левый отступ заголовка дока («Connection Manager» и т.п.) — чуть больше gutter'а, чтобы титул
/// не липнул к краю панели.
pub const DOCK_TITLE_INDENT: f32 = 8.0;
/// Ширина под угловой resize-grip ОС: правый отступ статус-бара в ВОССТАНОВЛЕННОМ окне, чтобы
/// версия не пряталась под диагональю грипа (в развёрнутом окне грипа нет → обычный gutter).
pub const RESIZE_GRIP_W: f32 = 22.0;

/// The soft drop shadow under raised surfaces (islands, menus, modals).
/// Painted manually for hand-drawn islands: add `island_shadow().as_shape(rect, radius)`
/// to the painter BEFORE the fill/stroke.
pub fn island_shadow() -> egui::epaint::Shadow {
    egui::epaint::Shadow { offset: [0, 1], blur: 4, spread: 0, color: p().shadow }
}

/// Mix `c` into `base` by factor `t` (0..=1). For deriving soft status-chip backgrounds:
/// `tint(p().panel, p().ok, 0.16)`.
pub fn tint(base: Color32, c: Color32, t: f32) -> Color32 {
    let m = |a: u8, b: u8| -> u8 { (a as f32 + (b as f32 - a as f32) * t).round() as u8 };
    Color32::from_rgb(m(base.r(), c.r()), m(base.g(), c.g()), m(base.b(), c.b()))
}

/// The frame used by all modal dialog boxes. Modal BODIES are CHROME in the two-surface
/// model (Design Delta v2.2 §1) — the fields sitting on them are SURFACE.
pub fn modal_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(p().panel)
        .stroke(Stroke::new(1.0, p().border_strong))
        .corner_radius(CornerRadius::same(RADIUS_ISLAND))
        .shadow(island_shadow())
        .inner_margin(Margin::same(SPACE_4 as i8 + 4)) // 20px
}

/// Local widget style for buttons/inputs inside a modal: field surface, the canonical
/// border, thin heights, coral on the committed (pressed) state.
pub fn style_modal_widgets(ui: &mut egui::Ui) {
    let pal = p();
    ui.style_mut().visuals.override_text_color = None;
    // 1.0 LOGICAL — matches the canonical border weight set in `apply` (HiDPI-safe).
    let r = CornerRadius::same(RADIUS_CONTROL);
    let w = &mut ui.style_mut().visuals.widgets;
    w.inactive.weak_bg_fill = pal.field_bg;
    w.inactive.bg_stroke = Stroke::new(1.0, pal.border_strong);
    w.inactive.fg_stroke = Stroke::new(1.0, pal.text);
    w.inactive.corner_radius = r;
    w.hovered.weak_bg_fill = pal.hover;
    w.hovered.bg_stroke = Stroke::new(1.0, pal.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.0, pal.text);
    w.hovered.corner_radius = r;
    w.active.weak_bg_fill = pal.accent_soft;
    w.active.bg_stroke = Stroke::new(1.0, pal.accent);
    w.active.fg_stroke = Stroke::new(1.0, pal.text);
    w.active.corner_radius = r;
}

/// Bundle the embedded fonts (JetBrains Mono for code, the JustQuery icon font for glyphs —
/// see `src/icons.rs`).
pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "jetbrains".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/JetBrainsMono-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "jetbrains-bold".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/JetBrainsMono-Bold.ttf"
        ))),
    );
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "jetbrains".to_owned());

    // JustQuery's own icon font (29 authored glyphs, U+E900..E91C; src/icons.rs) — appended
    // as a fallback so the PUA codepoints render inside any text run.
    fonts.font_data.insert(
        "jq-icons".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/justquery-icons.ttf"
        ))),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push("jq-icons".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("jq-icons".to_owned());

    let mut have_segoe = false;
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
        fonts.font_data.insert(
            "segoe".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "segoe".to_owned());
        have_segoe = true;
    }

    let bold_key = if std::fs::read(r"C:\Windows\Fonts\seguisb.ttf")
        .map(|bytes| {
            fonts.font_data.insert(
                "segoe-sb".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
        })
        .is_ok()
    {
        Some("segoe-sb")
    } else if have_segoe {
        Some("segoe")
    } else {
        None
    };

    fonts.families.insert(
        egui::FontFamily::Name("code".into()),
        vec![
            "jetbrains-bold".to_owned(),
            "jetbrains".to_owned(),
            "jq-icons".to_owned(),
        ],
    );
    fonts.families.insert(
        egui::FontFamily::Name("code-regular".into()),
        vec!["jetbrains".to_owned(), "jq-icons".to_owned()],
    );

    let mut ui_bold: Vec<String> = Vec::new();
    if let Some(k) = bold_key {
        ui_bold.push(k.to_owned());
    }
    ui_bold.push("jq-icons".to_owned());
    ui_bold.push("jetbrains-bold".to_owned());
    fonts
        .families
        .insert(egui::FontFamily::Name("ui-bold".into()), ui_bold);

    ctx.set_fonts(fonts);
}

pub fn code_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("code".into()))
}

pub fn code_font_regular(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("code-regular".into()))
}

pub fn ui_bold_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("ui-bold".into()))
}

/// Apply `pal` as the egui style. Called on startup and from [`set_theme`].
pub fn apply(ctx: &egui::Context, pal: &Palette) {
    let _ = ctx; // ctx reserved for future theme hooks; borders are 1.0 logical (HiDPI-safe)
    let dark = DARK_MODE.load(Ordering::Relaxed);
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.override_text_color = Some(pal.text);
    v.panel_fill = pal.panel;
    v.window_fill = pal.panel; // menus / popups: the darker CHROME tone (the two-colour pair)
    v.extreme_bg_color = pal.field_bg;
    v.faint_bg_color = pal.row_alt;

    // One canonical frame everywhere; islands now carry a soft shadow (the studio depth).
    // 1.0 LOGICAL pixel everywhere (StrokeKind::Inside is set per-widget at the draw sites) — at any
    // HiDPI scale every border renders at the same weight. The earlier `px = 1/pp` here made
    // hovered/open/window borders a different thickness than the rest at 125/150% scale.
    v.window_stroke = Stroke::new(1.0, pal.border_strong);
    v.window_corner_radius = CornerRadius::same(RADIUS_ISLAND);
    v.menu_corner_radius = CornerRadius::same(RADIUS_ISLAND);
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 1], blur: 4, spread: 0, color: pal.shadow };
    v.window_shadow = egui::epaint::Shadow { offset: [0, 2], blur: 8, spread: 0, color: pal.shadow };

    let r = CornerRadius::same(RADIUS_CONTROL);
    let w = &mut v.widgets;
    w.noninteractive.fg_stroke = Stroke::new(1.0, pal.text);
    w.noninteractive.bg_stroke = Stroke::new(1.0, pal.divider);

    // flat until hovered
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    w.inactive.bg_fill = Color32::TRANSPARENT;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, pal.text);
    w.inactive.corner_radius = r;
    w.inactive.expansion = 0.0;

    // hover — NEUTRAL; coral is reserved for committed state
    w.hovered.weak_bg_fill = pal.hover;
    w.hovered.bg_fill = pal.hover;
    w.hovered.bg_stroke = Stroke::new(1.0, pal.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.0, pal.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    // pressed — coral
    w.active.weak_bg_fill = pal.accent;
    w.active.bg_fill = pal.accent;
    w.active.bg_stroke = Stroke::new(1.0, pal.accent);
    w.active.fg_stroke = Stroke::new(1.0, pal.on_accent);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.weak_bg_fill = pal.hover;
    w.open.bg_fill = pal.hover;
    w.open.bg_stroke = Stroke::new(1.0, pal.border_strong);
    w.open.corner_radius = r;

    v.selection.bg_fill = pal.editor_sel;
    v.selection.stroke = Stroke::new(1.0, pal.accent);
    v.hyperlink_color = pal.accent_hi;
    // фирменная коралловая каретка во ВСЕХ текстовых полях (2px — 1px терялся). Цвет — accent
    // палитры (видна и в светлой, и в тёмной теме); apply вызывается из set_theme при смене темы.
    v.text_cursor.stroke = Stroke::new(2.0, pal.accent);
    v.text_cursor.preview = false;

    let slot = if dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.set_theme(if dark { egui::ThemePreference::Dark } else { egui::ThemePreference::Light });
    ctx.set_visuals_of(slot, v);

    ctx.global_style_mut(|s| {
        use egui::{FontFamily, FontId, TextStyle};
        s.text_styles = [
            (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(16.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(CODE_SIZE, FontFamily::Monospace)),
        ]
        .into();

        // Unified 22px controls, longer DBVis-style buttons (v2.2 §4)
        s.spacing.button_padding = Vec2::new(14.0, 3.0);
        s.spacing.interact_size = Vec2::new(40.0, CONTROL_H);
        // Horizontal follows the SPACE scale (8 = SPACE_2). Vertical is an off-scale 6px on
        // purpose: vertical stacks of controls (forms, modal fields) read less cramped than the
        // 8px horizontal would make them, and tighter than 4px (SPACE_1) keeps multi-row layouts
        // breathable. Kept as a literal so it isn't mistaken for a SPACE step.
        s.spacing.item_spacing = Vec2::new(SPACE_2, 6.0);
        // Solid-скроллы: всегда видимые, прижаты к краю, со СВОИМ местом
        // (floating=false), поэтому не накрывают последнюю строку/столбец и не
        // пересекаются в углу (egui резервирует угол).
        s.spacing.scroll.floating = false;
        s.spacing.scroll.bar_width = 8.0;
        s.spacing.scroll.bar_inner_margin = 0.0;
        s.spacing.scroll.bar_outer_margin = 0.0;
        s.spacing.scroll.dormant_background_opacity = 0.0;
        s.spacing.scroll.dormant_handle_opacity = 1.0;
        s.spacing.scroll.active_background_opacity = 0.0;
        s.spacing.scroll.interact_background_opacity = 0.0;
        s.spacing.scroll.active_handle_opacity = 1.0;
        s.spacing.scroll.interact_handle_opacity = 1.0;
        // 0.15s: the closest workable stand-in for the delta's ~0.6s fade — scrollbar fades
        // share this clock with every hover animation, and 0.6 would make those feel sluggish.
        s.animation_time = 0.15;
        s.scroll_animation = egui::style::ScrollAnimation::none();
    });
}
