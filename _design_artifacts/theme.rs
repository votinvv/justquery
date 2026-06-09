//! Central place for the app's look: the colour palette, a few style metrics, the bundled
//! fonts and the egui style. Everything visual is defined here so the rest of the code refers
//! to semantic colours (`crate::PANEL2`, `crate::ACCENT`, …) instead of raw hex values.
//!
//! The colours live in a [`Palette`] struct and the active palette is [`LIGHT`]. This is the
//! seam for a future light/dark switch: add a `DARK` palette and make [`apply`] (and the
//! custom-painted widgets) read from a runtime palette instead of the `LIGHT`-derived consts.
//!
//! REDESIGN NOTES (read before changing values):
//!   * The base stays the original "silvery light" identity. Only three things changed vs the
//!     pre-redesign version: (1) rounded corners replace the sharp ones, (2) the neutral-dark
//!     accent became the PostgreSQL "elephant" blue used ONLY for meaning (primary action,
//!     selection, focus, links), and (3) a deliberate type + spacing scale was added so layouts
//!     stop drifting. Hover stays neutral on purpose — blue is reserved for *committed* state
//!     (pressed / selected / primary), so the accent never becomes visual noise.
//!   * Public names (every `pub const`, every `pub fn`, the `Palette` fields) are unchanged, so
//!     the rest of the crate compiles untouched. New items are additive (RADIUS_*, SPACE_*,
//!     ACCENT_PRESS, SELECT_BAR).

use eframe::egui;
use egui::{Color32, CornerRadius, Margin, Stroke, Vec2};

/// A complete set of semantic UI colours (one theme).
#[derive(Clone, Copy)]
pub struct Palette {
    // surfaces
    pub ivory: Color32,
    pub panel: Color32,
    pub panel2: Color32,
    pub grid_header: Color32,
    pub data_bg: Color32, // surfaces showing data (not editor text): grid, connection form, status
    pub row_alt: Color32,
    // borders
    pub border: Color32,
    pub border_strong: Color32,
    pub menu_border: Color32,
    pub divider: Color32, // thin dark line between UI blocks (panel separators)
    // text
    pub text: Color32,
    pub text_dim: Color32,
    pub disabled: Color32,
    // accents
    pub accent: Color32,
    pub hover: Color32,
    pub select: Color32,
    pub acc_bg: Color32,  // darker hover background (tabs / menus / find)
    pub acc_bg2: Color32, // darker still: pressed / active
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
    pub gutter: Color32,      // editor line-number gutter background
    pub active_line: Color32, // current-line tint (translucent, drawn over the text)
    pub editor_sel: Color32,  // editor text selection background
    // SQL syntax
    pub syn_kw: Color32,
    pub syn_str: Color32,
    pub syn_com: Color32,
    pub syn_num: Color32,
    pub syn_fn: Color32,
}

/// The "Claude Code Light" palette, refined: silvery neutral surfaces + one PostgreSQL-blue accent.
pub const LIGHT: Palette = Palette {
    // Surfaces: near-white silvery neutrals. `ivory` is the lightest (islands/modals/editor),
    // `panel` the chrome fill, white is the data/field surface.
    ivory: Color32::from_rgb(0xfc, 0xfc, 0xfd),
    panel: Color32::from_rgb(0xf3, 0xf4, 0xf6),
    panel2: Color32::from_rgb(0xf3, 0xf4, 0xf6),
    grid_header: Color32::from_rgb(0xee, 0xf0, 0xf2),
    data_bg: Color32::from_rgb(0xf3, 0xf4, 0xf6),
    row_alt: Color32::from_rgb(0xf6, 0xf7, 0xf9),

    // Borders: one neutral family. `border_strong` is the single canonical frame used everywhere.
    border: Color32::from_rgb(0xda, 0xdd, 0xe2),
    border_strong: Color32::from_rgb(0xbe, 0xc1, 0xc8),
    menu_border: Color32::from_rgb(0xbe, 0xc1, 0xc8), // unified with border_strong (was lighter/odd)
    divider: Color32::from_rgb(0xda, 0xdd, 0xe2), // softer divider (was the heavy #a3a7ae)

    // Text
    text: Color32::from_rgb(0x24, 0x26, 0x2b),
    text_dim: Color32::from_rgb(0x8a, 0x8e, 0x96),
    disabled: Color32::from_rgb(0xc4, 0xc7, 0xce),

    // Accent: PostgreSQL "elephant" blue. Used ONLY for committed state (primary / selected /
    // focused / links). Never for hover, never decorative.
    accent: Color32::from_rgb(0x33, 0x67, 0x91),
    hover: Color32::from_rgb(0xee, 0xf0, 0xf3),  // neutral soft hover fill
    select: Color32::from_rgb(0xe3, 0xed, 0xf7), // light elephant tint for selected rows/tabs
    acc_bg: Color32::from_rgb(0xe8, 0xea, 0xef),
    acc_bg2: Color32::from_rgb(0xdd, 0xe0, 0xe6),

    // Status (kept muted so they don't fight the calm base)
    ok: Color32::from_rgb(0x5a, 0x8a, 0x3c),
    warn: Color32::from_rgb(0xc2, 0x8a, 0x1a),
    danger: Color32::from_rgb(0xbe, 0x3b, 0x2f),

    // Scrollbars / grips
    scroll_dormant: Color32::from_rgb(0xc6, 0xc9, 0xcf),
    scroll_hot: Color32::from_rgb(0xab, 0xae, 0xb6),
    scroll_pressed: Color32::from_rgb(0x9a, 0x9e, 0xa6),
    grip_hot: Color32::from_rgb(0xb7, 0xba, 0xc1),

    // Misc
    find_hl: Color32::from_rgba_premultiplied(0x8a, 0x6a, 0x14, 0x55),
    gutter: Color32::from_rgb(0xf6, 0xf7, 0xf9),
    active_line: Color32::from_rgb(0xe8, 0xf1, 0xfd),
    editor_sel: Color32::from_rgb(0xcd, 0xe1, 0xf8),

    // SQL syntax
    syn_kw: Color32::from_rgb(0xa0, 0x4a, 0x26),
    syn_str: Color32::from_rgb(0x5b, 0x7a, 0x45),
    syn_com: Color32::from_rgb(0xa8, 0xa1, 0x91),
    syn_num: Color32::from_rgb(0x8a, 0x6d, 0x2f),
    syn_fn: Color32::from_rgb(0x3f, 0x6f, 0x9f),
};

// ---- Backward-compatible semantic consts (derived from the active light palette) ----
// The rest of the app refers to these names; custom-painted widgets use them directly.
pub const IVORY: Color32 = LIGHT.ivory;
pub const PANEL: Color32 = LIGHT.panel;
pub const PANEL2: Color32 = LIGHT.panel2;
pub const GRID_HEADER: Color32 = LIGHT.grid_header;
pub const DATA_BG: Color32 = LIGHT.data_bg;
pub const ROWALT: Color32 = LIGHT.row_alt;
pub const BORDER: Color32 = LIGHT.border;
pub const BORDER_STRONG: Color32 = LIGHT.border_strong;
pub const MENU_BORDER: Color32 = LIGHT.menu_border;
pub const DIVIDER: Color32 = LIGHT.divider;
pub const TEXT: Color32 = LIGHT.text;
pub const TEXTDIM: Color32 = LIGHT.text_dim;
pub const DISABLED: Color32 = LIGHT.disabled;
pub const ACCENT: Color32 = LIGHT.accent;
pub const HOVER: Color32 = LIGHT.hover;
pub const SELECT: Color32 = LIGHT.select;
pub const ACC_BG: Color32 = LIGHT.acc_bg;
pub const ACC_BG2: Color32 = LIGHT.acc_bg2;
pub const OK: Color32 = LIGHT.ok;
pub const WARN: Color32 = LIGHT.warn;
pub const DANGER: Color32 = LIGHT.danger;
pub const SCROLL_DORMANT: Color32 = LIGHT.scroll_dormant;
pub const SCROLL_HOT: Color32 = LIGHT.scroll_hot;
pub const SCROLL_PRESSED: Color32 = LIGHT.scroll_pressed;
pub const GRIP_HOT: Color32 = LIGHT.grip_hot;
pub const FIND_HL: Color32 = LIGHT.find_hl;
pub const GUTTER: Color32 = LIGHT.gutter;
pub const ACTIVE_LINE: Color32 = LIGHT.active_line;
pub const EDITOR_SEL: Color32 = LIGHT.editor_sel;
pub const SYN_KW: Color32 = LIGHT.syn_kw;
pub const SYN_STR: Color32 = LIGHT.syn_str;
pub const SYN_COM: Color32 = LIGHT.syn_com;
pub const SYN_NUM: Color32 = LIGHT.syn_num;
pub const SYN_FN: Color32 = LIGHT.syn_fn;

// ---- New accent helpers (additive) ----
/// Darker elephant for the *pressed* state of the primary button (e.g. Connect, OK, Apply).
pub const ACCENT_PRESS: Color32 = Color32::from_rgb(0x2a, 0x55, 0x77);
/// Solid elephant bar used as the 2px accent on a selected list row / active tab.
pub const SELECT_BAR: Color32 = LIGHT.accent;

// ---- Style metrics ----

/// Shared height of every chrome row: the caption/menu bar, the icon toolbar, and the tab /
/// connection-manager rows. Keeping them identical means the blank gaps between rows come out
/// equal (see `CHROME_PAD`). DO NOT change without re-checking the equal-gap math.
pub const CAPTION_H: f32 = 30.0;

/// Height of a tab row (shared by the editor and result tabs so they match). Equal to CAPTION_H
/// so the menu / toolbar / tabs panels are all the same height.
pub const TABBAR_H: f32 = 30.0;

/// Vertical padding inset for every interactive chrome box (menu item, toolbar icon, tab). With
/// all rows the same height, the blank gap between two adjacent rows' boxes is exactly 2×this.
pub const CHROME_PAD: f32 = 4.0;

/// Height of a work-area sub-toolbar (the icon strip under each work area's tabs/header).
pub const SUBBAR_H: f32 = 26.0;

/// Diagnostic: force every chrome box visible (not just on hover). `false` for normal behaviour.
pub const DIAG_BOXES: bool = false;

/// Editor code font size (bold JetBrains Mono).
pub const CODE_SIZE: f32 = 13.0;

// ---- Corner radii (the new rounded look; egui 0.34 CornerRadius is u8-based) ----
/// Radius for interactive controls: buttons, fields, dropdowns, menu items, toolbar boxes, tabs.
pub const RADIUS_CONTROL: u8 = 6;
/// Radius for containers: islands, panels, modals/sheets, result-grid frame, popups/menus.
pub const RADIUS_ISLAND: u8 = 8;

// ---- Spacing scale (use these everywhere instead of magic numbers) ----
/// 4px — tight inner gaps (icon ↔ label).
pub const SPACE_1: f32 = 4.0;
/// 8px — default gap between sibling controls and label↔field.
pub const SPACE_2: f32 = 8.0;
/// 12px — gap between grouped rows inside a section.
pub const SPACE_3: f32 = 12.0;
/// 16px — gap between sections / dialog content padding.
pub const SPACE_4: f32 = 16.0;
/// 24px — major separation (section ↔ button bar).
pub const SPACE_5: f32 = 24.0;

/// The frame used by all modal dialog boxes (panel fill, one canonical border, rounded island).
pub fn modal_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(IVORY) // lightest surface so the modal reads as a raised sheet over the panel
        .stroke(Stroke::new(1.0, BORDER_STRONG)) // same frame border as the islands/sheets
        .corner_radius(CornerRadius::same(RADIUS_ISLAND))
        .inner_margin(Margin::same(SPACE_4 as i8 + 4)) // 20px
}

/// Local widget style for buttons/inputs inside a modal: white field, one canonical border,
/// rounded corners, and the app's accent on the *committed* (pressed) state.
pub fn style_modal_widgets(ui: &mut egui::Ui) {
    ui.style_mut().visuals.override_text_color = None;
    let bw = 1.0 / ui.ctx().pixels_per_point(); // 1 physical px border (crisp at any DPI)
    let r = CornerRadius::same(RADIUS_CONTROL);
    let w = &mut ui.style_mut().visuals.widgets;
    // fields/buttons/dropdowns share the one frame border (border_strong), same as islands/sheets
    w.inactive.weak_bg_fill = Color32::WHITE;
    w.inactive.bg_stroke = Stroke::new(bw, BORDER_STRONG);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = r;
    // hover stays neutral (soft fill + the same border) — blue is reserved for the pressed state
    w.hovered.weak_bg_fill = HOVER;
    w.hovered.bg_stroke = Stroke::new(bw, BORDER_STRONG);
    w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.corner_radius = r;
    // active (pressed) — accent outline so the click reads as a committed action
    w.active.weak_bg_fill = SELECT;
    w.active.bg_stroke = Stroke::new(bw, ACCENT);
    w.active.fg_stroke = Stroke::new(1.0, TEXT);
    w.active.corner_radius = r;
}

/// Bundle the embedded fonts (JetBrains Mono for code, Lucide for icon glyphs).
pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // JetBrains Mono (SIL OFL) — editor / grid monospace
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

    // Lucide (ISC) — icon glyphs; appended as fallback so PUA codepoints render anywhere
    fonts.font_data.insert(
        "lucide".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/lucide.ttf"
        ))),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push("lucide".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("lucide".to_owned());

    // Native Windows UI font (Segoe UI) for proportional text — hinted, crisp at small sizes.
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

    // Segoe UI Semibold for emphasised UI text (used via the "ui-bold" family below).
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

    // custom "code" family for the editor: bold JetBrains Mono (regular + lucide as fallbacks)
    fonts.families.insert(
        egui::FontFamily::Name("code".into()),
        vec![
            "jetbrains-bold".to_owned(),
            "jetbrains".to_owned(),
            "lucide".to_owned(),
        ],
    );
    // "code-regular": normal-weight JetBrains Mono — for non-highlighted editor text.
    fonts.families.insert(
        egui::FontFamily::Name("code-regular".into()),
        vec!["jetbrains".to_owned(), "lucide".to_owned()],
    );

    // "ui-bold" family: semibold Segoe (or whatever's available) + lucide + bold JetBrains fallback.
    let mut ui_bold: Vec<String> = Vec::new();
    if let Some(k) = bold_key {
        ui_bold.push(k.to_owned());
    }
    ui_bold.push("lucide".to_owned());
    ui_bold.push("jetbrains-bold".to_owned());
    fonts.families.insert(egui::FontFamily::Name("ui-bold".into()), ui_bold);

    ctx.set_fonts(fonts);
}

/// The editor's code font: bold JetBrains Mono (the custom "code" family).
pub fn code_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("code".into()))
}

/// Normal-weight JetBrains Mono — for non-highlighted editor text (same metrics as [`code_font`]).
pub fn code_font_regular(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("code-regular".into()))
}

/// A bold/semibold proportional UI font (the "ui-bold" family) — for emphasised chips and titles.
pub fn ui_bold_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("ui-bold".into()))
}

/// Apply `p` as the egui style. Pins the Light theme slot so the palette isn't overwritten by
/// the system theme (see the comment in `set_visuals`).
pub fn apply(ctx: &egui::Context, p: &Palette) {
    let px = 1.0 / ctx.pixels_per_point(); // one physical pixel, for crisp 1px egui strokes
    let mut v = egui::Visuals::light();
    v.override_text_color = Some(p.text);
    v.panel_fill = p.panel;
    v.window_fill = p.ivory; // dropdown menus / popups read as light sheets
    v.extreme_bg_color = Color32::WHITE; // text-edit background
    v.faint_bg_color = p.row_alt; // striped table rows

    // One canonical frame border everywhere (islands, sheets, modals, menus/popups): thin, crisp,
    // border_strong. Rounded now. No drop shadows anywhere — the app is flat.
    v.window_stroke = Stroke::new(px, p.border_strong);
    v.window_corner_radius = CornerRadius::same(RADIUS_ISLAND);
    v.menu_corner_radius = CornerRadius::same(RADIUS_ISLAND);
    v.popup_shadow = egui::epaint::Shadow::NONE;
    v.window_shadow = egui::epaint::Shadow::NONE;

    let r = CornerRadius::same(RADIUS_CONTROL);
    let w = &mut v.widgets;
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    // used by panel separators and ui.separator() — a thin neutral line between blocks
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.divider);

    // flat by default — buttons show no chrome until hovered
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    w.inactive.bg_fill = Color32::TRANSPARENT;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;
    w.inactive.expansion = 0.0;

    // hovered — NEUTRAL soft highlight (no blue). Blue is reserved for committed state.
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_fill = p.hover;
    w.hovered.bg_stroke = Stroke::new(px, p.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    // active (pressed) — accent fill: the one place a generic control turns elephant-blue
    w.active.weak_bg_fill = p.accent;
    w.active.bg_fill = p.accent;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    // open menus
    w.open.weak_bg_fill = p.hover;
    w.open.bg_fill = p.hover;
    w.open.bg_stroke = Stroke::new(px, p.border_strong);
    w.open.corner_radius = r;

    v.selection.bg_fill = p.editor_sel; // text selection — soft blue, matches the accent family
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.hyperlink_color = p.accent;
    v.text_cursor.stroke = Stroke::new(1.0, p.text); // thin neutral caret
    v.text_cursor.preview = false;

    // Pin the Light theme and write our visuals into the Light slot.
    ctx.set_theme(egui::ThemePreference::Light);
    ctx.set_visuals_of(egui::Theme::Light, v);

    ctx.global_style_mut(|s| {
        // Deliberate type scale (Segoe UI for proportional, JetBrains Mono for code). Keeping
        // Body at 13 matches the OS look and the chrome row heights.
        use egui::{FontFamily, FontId, TextStyle};
        s.text_styles = [
            (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(16.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(CODE_SIZE, FontFamily::Monospace)),
        ]
        .into();

        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.item_spacing = Vec2::new(SPACE_2, 6.0); // 8px horizontal rhythm; 6px vertical keeps chrome math
        s.animation_time = 0.05; // snappier hover / state transitions
        // Instant scroll: momentum/inertia is computed ourselves in `raw_input_hook`.
        s.scroll_animation = egui::style::ScrollAnimation::none();
    });
}
