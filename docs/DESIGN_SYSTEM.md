# JustQuery — Design System (v2.2 «Two Colours»)

The single source of truth for the JustQuery look. This document is the merge of the full
lineage — v2 «Warm Studio» → v2.1 «Matte» → v2.2 «Two Colours» — resolved to the current
state; where versions disagreed, the later rule won. Token values below are verified against
the code.

The implementation seam: `src/theme.rs` (dual `Palette` + runtime `p()`, `apply()`) and
`src/widgets.rs` (`crisp_border`, `primary_button`, `show_modal`, `form_row`).
egui **0.34** (`CornerRadius` is `u8`).

---

## 1. Principles

1. **One loop, no clutter.** The center of gravity is *type a query → get a result*. Chrome
   stays quiet; the editor and the data are the loudest things on screen.
2. **One accent, meaning only.** Coral — the colour of the app's own logo — appears only on:
   the primary action, the current selection, the focus ring, the caret, and links. Hover is
   always neutral. If coral shows up, it means *chosen / main / here*.
3. **Two surface colours.** Statically, a theme has exactly two surface colours — SURFACE and
   CHROME (§2). Everything else is a derived or interaction state.
4. **One border.** A single 1.0-logical-px `border_strong` stroke frames every island, field,
   popup and menu — drawn as one shape together with the fill (§4).
5. **One rhythm.** All spacing comes from the `SPACE_*` scale; labels and fields share
   columns. No magic numbers at call sites.
6. **Gentle depth.** Raised surfaces carry a soft shadow (`island_shadow()`) — the only
   decoration the app allows itself. The window outline stays square.
7. **Thin controls.** Every one-line control is `CONTROL_H = 24` px tall (§5).
8. **Matte dark theme.** Dark is not "lights off" — it's a warm studio: brown-graphite
   surfaces, muted warm-grey text, no pure white anywhere (including text on coral buttons).
   Built for long sessions.

The PostgreSQL elephant lives on as a wink: `syn_fn` (functions in SQL highlighting) stays
blue in both themes.

---

## 2. Surface model: SURFACE / CHROME

Statically a theme has exactly **two** surface colours:

- **SURFACE** — content-bearing: managers/docks, editor, grid, input fields, combos,
  dropdown lists, popups and menus.
- **CHROME** — everything else: window background, tab strip, toolbars and subbars,
  status bar, modal bodies (fields on them are SURFACE), grid header.

**Law of both themes: CHROME is always darker than SURFACE.** In light — a tint darker than
white; in dark — blackness darker than the muted content. Panels with the same role share the
same token, so they match across themes automatically.

Derived exceptions (computed from the pair, not a "third colour"): grid zebra (`row_alt`) and
the line-number gutter. Plus interaction states: hover / select / accent.

| | Light | Dark |
|---|---|---|
| SURFACE | `#FDFCF9` | `#2B2622` |
| CHROME | `#F1EEE9` | `#1F1B18` |

---

## 3. Colour tokens

All values live in `theme.rs` (`LIGHT` / `DARK`); call sites read `theme::p().<field>` —
never a hex, never a bare const.

| Token | Light | Dark | Use |
|---|---|---|---|
| `ivory` | `#FDFCF9` | `#2B2622` | SURFACE — islands, modals, menus, popups |
| `panel` / `panel2` | `#F1EEE9` | `#1F1B18` | CHROME — caption, toolbars, tab strip, docks |
| `grid_header` | `#F1EEE9` | `#1F1B18` | CHROME — sticky grid header |
| `data_bg` | `#F1EEE9` | `#1F1B18` | CHROME — status bar, form backdrops |
| `row_alt` | `#F7F5F1` | `#26221F` | Zebra rows (derived) |
| `field_bg` | `#FDFCF9` | `#2B2622` | SURFACE — field interiors AND the editor background |
| `border` / `divider` | `#DCD8D1` | `#3A342E` | Soft dividers |
| `border_strong` / `menu_border` | `#C5BFB6` | `#453E37` | THE canonical frame |
| `text` | `#2A2723` | `#D5CEC3` | Primary text (dark: matte warm, not white) |
| `text_dim` | `#8F897F` | `#9D958A` | Labels, captions, secondary |
| `disabled` | `#C9C3BA` | `#615A50` | Inert controls |
| `accent` | `#C05A33` | `#D97757` | Primary fill, selection bars, active-tab marks, caret |
| `accent_hi` | `#B5522D` | `#E58D69` | Accent as text/lines/icons (readable on `panel`) |
| `accent_press` | `#A84A28` | `#C26344` | Primary button pressed |
| `accent_soft` | `#F5E5DC` | `#3C2E26` | Tinted chip bg (active tab, version chip) |
| `on_accent` | `#FFFFFF` | `#FBF2EC` | Text on accent fill (dark: cream, not white) |
| `hover` | `#E6E1DA` | `#34302A` | Neutral hover fill (readable on both surfaces) |
| `select` | `#F3DDD2` | `#423126` | Selected row / strong tint |
| `acc_bg` / `acc_bg2` | `#E6E1DA` / `#DDD7CF` | `#332E28` / `#3B352E` | Darker neutral fills |
| `ok` | `#61863C` | `#9DBE7B` | Connected / success |
| `warn` | `#C28A1A` | `#D6A65A` | Warnings / asleep |
| `danger` | `#B7352A` | `#E2625A` | Errors / destructive — kept apart from coral |
| `scroll_dormant/hot/pressed` | `#C9C4BB` / `#AFA9A0` / `#9D978D` | `#453E36` / `#524A41` / `#5F564C` | Scrollbar pill states (muted — auto-hidden) |
| `grip_hot` | `#BCB6AC` | `#4A4239` | Resize grip |
| `gutter` | `#F7F5F1` | `#26221F` | Line-number gutter (derived) |
| `active_line` | `#FAEFE6` | `#332B24` | Current editor line |
| `editor_sel` | `#F4D8C8` | `#503A2C` | Text selection |
| `shadow` | `rgba(4,3,2,.08)` | `rgba(0,0,0,.28)` | Island shadow colour |

SQL syntax — light: kw `#A04A26`, str `#5B7A45`, com `#A8A191`, num `#8A6D2F`, fn `#3F6F9F`.
Dark: kw `#E08A66`, str `#9CBF7A`, com `#756D60`, num `#CFAF72`, fn `#7FA9D0`.

Status-chip backgrounds are derived, not stored: `tint(p().panel, p().ok, 0.16)`.

Field state colours: **focus = `accent` border (coral), error = `danger` border (red)**.
A field can be both — then danger wins.

---

## 4. Shape & depth

| Token | Value | Applies to |
|---|---|---|
| `RADIUS_CONTROL` | **4** | Buttons, fields, combos, menu items, tabs, chips |
| `RADIUS_ISLAND` | **6** | Islands, modals, popups/menus, grid frame, status bar |
| window | 0 | Square — custom chrome and resize hit-testing depend on it |
| `island_shadow()` | offset [0,1], blur 4 | Every raised surface; modals use blur 8 |

There are no pills (`RADIUS_PILL` is gone) and no radius larger than 6. Shadows are subtle in
both themes — depth, not glow; nothing else gets a shadow.

**Borders: one shape, no pixel dogma.** The old "exactly 1 physical pixel, pixel-snapped"
rule is dead — it produced seams and ragged fills at rounded corners. The law now: fill and
stroke of any island/field/popup are **one `RectShape`** (fill + stroke,
`StrokeKind::Inside`), stroke width **1.0 logical**, anti-aliasing on. No manual pixel
snapping; `crisp_border` is a trivial wrapper over that single shape (+ shadow before it).

---

## 5. Controls: one geometry

| Parameter | Value |
|---|---|
| Height of EVERY one-line control (buttons, fields, combos, menu rows) | **`CONTROL_H = 24`** |
| Radius | 4 |
| Button horizontal padding | **14** (`button_padding = Vec2::new(14.0, 4.0)`) |
| Field/combo inner text inset, left and right | **8** (text never touches the rounding) |

`BTN_H` and `FIELD_H` are aliases of `CONTROL_H`.

| Control | Spec |
|---|---|
| Button (secondary) | `field_bg` fill, 1px `border_strong`, radius 4. Hover → `hover` fill. |
| Button (primary) | Same geometry; `accent` fill, `on_accent` text; pressed → `accent_press`. **Exactly one per dialog.** |
| Button (destructive primary) | Same geometry; `danger` fill, `on_accent` text. A modal has either a primary or a destructive primary — never both. |
| Icon button | 24×24, transparent, neutral hover, radius 4. |
| Text field / combo | `field_bg`, 1px `border_strong`, radius 4; focused → `accent` border; error → `danger` border. |
| Checkbox / radio | egui default sizes, accent when checked. |

---

## 6. Typography & spacing

Segoe UI 13 body/button · 11 small (field labels, captions, status bar) · 16 semibold
heading; JetBrains Mono 13 for code and grids (**bold** face in the editor; non-highlighted
tokens regular). `ui_bold_font` for emphasised chips (egui's `.strong()` only recolours).
App title on About is a one-off ~22px via `ui_bold_font`.

Spacing scale: `SPACE_1..5` = **4 / 8 / 12 / 16 / 24**.

Chrome metrics, frozen — do not casually change: `CAPTION_H = TABBAR_H = 30`,
`CHROME_PAD = 4` (equal blank gaps between chrome rows = 2×`CHROME_PAD`), `SUBBAR_H = 26`.

**Gutter law.** A single **8px** gutter (`SPACE_2`) between any neighbouring islands and
between an island and the window edge, at any window size. Margins do not stack. Islands
never touch the status bar.

**Form law (`form_row`).** Label (`Small`, `text_dim`, line height 16) → **4px** → control →
**16px** to the next row. The label must visually stick to its own field — closer to it than
to anyone else's. One helper, `widgets::form_row`; hand-assembling label+field in forms is
forbidden. Applies to Connect, Scan, dock search, and every future form.

**Rounded-content law.** Any scrollable island: inner padding ≥ 6px on all sides AND content
clipped to the rounded rect. The first log/code line cannot touch the frame. The editor gets
4px top padding.

---

## 7. Components

**Tabs.** Active tab: `accent_soft` fill, `accent_hi` text, radius 4 rectangle. Inactive:
transparent, `text_dim`, neutral hover. No underline bars. Same for editor tabs and result
tabs (Messages / Result N). Close `×` only on hover or on the active tab.

**No-buttons-on-tabs contract.** A tab page is pure content. All page actions live in its
subbar (the same `SUBBAR_H` row as run/format for the editor):
- **Scan**: Disable, Rescan now — thin secondary buttons on the left; **Apply — primary, on
  the right of the same subbar**.
- **About**: Check for updates — in the subbar.
- Future pages — the same. **If a planned button has nowhere to go but the tab body, it's
  not a tab — it's a modal.** Modals are the only place for buttons in content (footer:
  Cancel + primary/destructive).

Scan layout (hard rule): subbar fixed on top; ALL tab content always inside a vertical
`ScrollArea` (unconditionally — conditional scrolls were the source of drift); the activity
log stretches to the remaining height, minimum 64px, with its own inner scroll.

**Menus & popups.** SURFACE body, 1px `border_strong`, radius 6, `island_shadow()`. Items
radius 4, `hover` fill on hover, selected item `select` bg. Custom popups (e.g. the Metadata
schema dropdown) are islands too: radius 6 with content clipped to the rounding — no square
bottom corners.

**Lists & trees.** Row ~26px. Selected row: `select` bg + 2px `accent` left bar; icon and
text take `text` (icon may take `accent_hi`). Hover (unselected): `hover` bg. Leading type
icon in `text_dim`.

**Result grid.** Frame: 1px `border_strong`, radius 6, clipped. Sticky header and pinned `#`
column `grid_header`. Zebra `row_alt` — the official derived exception, kept barely visible.
Cell selection `editor_sel`. Virtualization untouched — colours/radii only.

**Messages grid.** Rows with Status = Error/Fatal: status cell text `danger` + 2px `danger`
left bar on the row; plain rows unchanged.

**Result-panel actions.** `chevrons-up` / `chevrons-down` / `close` icons live in a reserved
fixed-width zone right of the result tab strip (with a fade under it). Tabs cannot slide
under the icons.

**Editor.** Background `field_bg`, gutter `gutter`, caret `accent` (the coral caret is a
signature), selection `editor_sel`, current line `active_line`. Empty state: one centered
`text_dim` line — «Ctrl+N — new query» — visible only when no tabs are open; connecting adds
nothing.

**Status bar — an island.** Radius 6, 8px margins left/right/bottom, CHROME fill, island
shadow, no hairline border. All elements center on one axis; chip icons are text-sized.
Chips are tinted from panel: `bg = tint(p().panel, colour, 0.16)`, text in the colour
itself — no chips lighter than panel in the dark theme. `scan` chip in lowercase, colour by
state (`ok`/`warn`/`danger`/`text_dim`). Identity `postgres@host`: `ok` when connected,
`text_dim` otherwise. Version chip: `accent_soft` bg, `accent_hi` text. Resize grip keeps a
6px gap from the version chip. Transient messages on the left («Formatted» etc.) are
`text_dim`, never green — green in the bar is reserved for connection state.

**Connection toggle (toolbar).** One button whose icon shows the *action on click*
(play/pause convention): disconnected → glyph `plug` (U+E91A), tooltip «Connect…»;
connected → glyph `plug-off` (U+E91B), tooltip «Disconnect». Neutral colour like every other
toolbar icon (`text_dim` → `text` on hover) — no green state on the icon. Clicking while
connected opens a confirmation modal: title «Disconnect», line
«Disconnect from postgres@localhost?», Cancel + Disconnect (destructive).

**Modals.** `modal_frame()`: SURFACE fill, 1px `border_strong`, radius 6, shadow, 20px inner
margin. Layout: title row (16px heading + `×`), `SPACE_4` gap, content (`form_row` rhythm),
`SPACE_5` gap, right-aligned button bar (secondary then primary, `SPACE_2` apart). No dead
vertical space — the modal hugs its content and **never changes size**.
- *Footer-error pattern:* an error adds no height — one `Small`/`danger` line on the left of
  the button row, ellipsised, full text in a tooltip; offending fields get the danger border.
- *Keyboard contract:* Enter presses the primary/destructive, Esc cancels/closes. Works in
  every modal via `show_modal`.

**Transfer list** (available ⇄ monitored, Scan). Two equal-width list boxes (1px
`border_strong`, radius 6), a centered column of 4 icon buttons between them
(`»` `›` `‹` `«`), vertically centered as a group with equal `SPACE_2` gaps.

**Scrollbars.** Pills: width **8px**, radius 4, **inset 4px** from the island frame and from
the track ends — a pill never meets a rounded corner (touching the frame reads as a bug).
No track painted. **Auto-hide**: appear on scroll or zone hover, fade out after ~0.6s idle
(opacity animation). Colours from `scroll_dormant/hot/pressed` (muted — they show rarely).
Applies to egui scrolls (`ScrollStyle`) and to all custom ones (grid, editor, activity log).
Island radii are NOT reduced for scrollbars — the inset resolves the conflict.

**Icons.** The in-house icon font from `icons/` (glyph map and build — `icons/README.md`).
Lucide is fully removed. Toolbar icon groups separated by a 1px `border` divider, 16px tall.

---

## 8. Motion

`animation_time = 0.05` (snappy hover/state). No easing on scrolling (custom kinetic scroll
owns it). No fades on modals — they appear instantly. The only deliberate animation is the
scrollbar auto-hide fade (§7).

---

## 9. Don'ts

- No coral on hover, headers, borders-at-rest, or decoration. Accent = meaning.
- No third static surface colour: new surfaces map to SURFACE or CHROME only (derived:
  zebra and gutter; states: hover/select/accent).
- No pure white (`#FFFFFF`) anywhere in the dark theme, including text on coral.
- No chips lighter than `panel` in the dark theme.
- `danger` ≠ `accent`: the hues are deliberately set apart — don't drift them back together.
- No second border colour; no shadows beyond `island_shadow()`; no glow.
- No buttons on tab pages — actions go to the subbar, or it becomes a modal.
- Content never touches rounded frames (padding + clip); scrollbars never touch frames.
- Border and fill are never drawn as separate shapes; no manual pixel snapping.
- No pills; no radius larger than 6. Don't round the window outline.
- No hardcoded hex/gaps at call sites — `theme::p()`, `SPACE_*`, `RADIUS_*`, `CONTROL_H`.
- Don't change chrome row heights (`CAPTION_H`/`TABBAR_H`/`CHROME_PAD`/`SUBBAR_H`).
- Don't let any surface skip the theme switch (no lingering `LIGHT.`/const usage).
- Don't touch virtualization/scrolling/caret logic in the editor and grid.
