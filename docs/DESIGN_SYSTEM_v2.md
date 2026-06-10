# JustQuery — Design System v2 «Тёплая студия»

Supersedes v1. One visual language, two palettes (warm light + Warm Studio dark), one accent:
**coral** — the colour of the app's own logo. The implementation seam is unchanged:
`src/theme.rs` (now dual `Palette` + runtime `p()`) and `src/widgets.rs` (`crisp_border`,
`primary_button`, `show_modal`). egui **0.34**.

---

## 1. Philosophy

The dark theme is not "lights off" — it's a warm studio: brown-graphite surfaces like good
leather, layered panels with soft shadows, and a coral accent that grows out of the base
instead of fighting it (coral and warm brown are neighbours on the colour wheel). The light
theme is the same room in daylight: warm paper neutrals, same layers, same coral.

Unchanged principles from v1, restated because they still rule everything:
1. The editor and the data are the loudest things on screen; chrome stays quiet.
2. **One accent, meaning only**: primary action, current selection, focus ring, caret, links.
   Hover is always neutral. If coral shows up, it means *chosen / main / here*.
3. **One border**: a single 1-physical-px `border_strong` frames every island, field and menu.
4. **One rhythm**: all spacing from the `SPACE_*` scale; shared label/field columns.
5. New in v2 — **gentle depth**: raised surfaces carry a soft shadow (`island_shadow()`),
   the only decoration the app allows itself. The window outline stays square.
6. New in v2 — **thin controls**: buttons 24px, fields 28px (DBVis density).

The PostgreSQL elephant lives on as a wink: `syn_fn` (functions in SQL highlighting) stays
blue in both themes.

---

## 2. Colour tokens

All values live in `theme.rs` (`LIGHT` / `DARK`); call sites read `theme::p().<field>`.

| Token | Light | Dark | Use |
|---|---|---|---|
| `ivory` | `#FBFAF8` | `#262220` | Raised surfaces: islands, modals, menus, popups |
| `panel` | `#F4F2EF` | `#1E1B18` | App chrome (caption, menus, toolbar, tab strip, docks) |
| `grid_header` | `#EDEAE5` | `#2B2724` | Sticky grid header |
| `data_bg` | `#F4F2EF` | `#1C1916` | Status bar, form backdrops |
| `row_alt` | `#F7F5F1` | `#201C19` | Zebra rows |
| `field_bg` | `#FFFFFF` | `#171412` | Field interiors AND the editor background |
| `border` | `#DDD9D2` | `#332E28` | Soft dividers |
| `border_strong` | `#C6C0B7` | `#3F3933` | THE canonical 1px frame |
| `text` | `#2A2723` | `#E8E2D9` | Primary text |
| `text_dim` | `#8F897F` | `#9C948A` | Labels, captions, secondary |
| `disabled` | `#C9C3BA` | `#5E574D` | Inert controls |
| `accent` | `#C05A33` | `#D97757` | Primary fill, selection bars, active-tab marks |
| `accent_hi` | `#B5522D` | `#E8916C` | Accent as text/lines/icons (readable on `panel`) |
| `accent_press` | `#A84A28` | `#C26344` | Primary button pressed |
| `accent_soft` | `#F6E7DF` | `#332620` | Tinted pill/chip bg (active tab, version chip) |
| `on_accent` | `#FFFFFF` | `#FFFFFF` | Text on accent fill |
| `hover` | `#EFECE7` | `#2B2724` | Neutral hover fill |
| `select` | `#F3DDD2` | `#3A2A21` | Selected row / strong tint |
| `ok` | `#61863C` | `#A3C47E` | Connected / success |
| `warn` | `#C28A1A` | `#D9A95C` | Warnings / asleep |
| `danger` | `#BE3B2F` | `#E07A6B` | Errors / destructive |
| `editor_sel` | `#F4D8C8` | `#4A3326` | Text selection |
| `active_line` | `#FBEFE7` | `#221D19` | Current editor line |
| `gutter` | `#F7F5F1` | `#1C1916` | Line-number gutter |
| `shadow` | `rgba(45,35,25,.09)` | `rgba(0,0,0,.45)` | Island shadow colour |

SQL syntax — light: kw `#A04A26`, str `#5B7A45`, com `#A8A191`, num `#8A6D2F`, fn `#3F6F9F`.
Dark: kw `#E8916C`, str `#9CBF7A`, com `#6F675C`, num `#D3B273`, fn `#7FA9D0`.

Status-chip backgrounds are derived, not stored: `tint(p().panel, p().ok, 0.16)`.

---

## 3. Typography & spacing

Unchanged from v1: Segoe UI 13 body / 11 small / 16 heading; JetBrains Mono 13 for code and
grids (bold face in the editor); `ui_bold_font` for emphasised chips. Spacing scale
`SPACE_1..5` = 4/8/12/16/24. Chrome metrics frozen: `CAPTION_H = TABBAR_H = 30`,
`CHROME_PAD = 4`, `SUBBAR_H = 26`.

## 4. Shape & depth

| Token | Value | Applies to |
|---|---|---|
| `RADIUS_CONTROL` | 7 | Buttons, fields, combos, menu items, toolbar boxes |
| `RADIUS_ISLAND` | 10 | Islands, modals, menus, grid frame |
| `RADIUS_PILL` | 100 | Tabs (active pill), status chips |
| window | 0 | Square — custom chrome and resize hit-testing |
| `island_shadow()` | offset [0,1], blur 4 | Every raised surface (islands, menus); modals use blur 8 |

Shadows are subtle in both themes — depth, not glow. Nothing else gets a shadow.

## 5. Controls (thin, DBVis density)

| Control | Spec |
|---|---|
| Button (secondary) | **24px tall** (`BTN_H`), padding 11×3, `field_bg` fill, 1px `border_strong`, radius 7. Hover → `hover` fill. |
| Button (primary) | Same geometry; `accent` fill, `on_accent` text, weight 500; pressed → `accent_press`. **Exactly one per dialog.** |
| Icon button | 24×24, transparent, neutral hover, radius 7. |
| Text field / combo | **28px tall** (`FIELD_H`), `field_bg`, 1px `border_strong`, radius 7; focused → 1px `accent` border. |
| Checkbox/radio | egui default sizes, accent when checked. |

## 6. Components delta vs v1

**Tabs** — the studio signature. Active tab is a **pill**: `accent_soft` fill, `RADIUS_PILL`,
text `accent_hi`, subtle shadow (offset [0,1] blur 2). Inactive: transparent, `text_dim`,
neutral hover pill. No underline bars anymore. Same for editor tabs and result tabs
(Messages / Result N).

**Lists & trees** — selected row: `select` bg + 2px `accent` left bar; icon and text take
`text` (icon may take `accent_hi`). Hover: `hover`.

**Islands / docks / grid frame** — `ivory` fill, 1px `border_strong`, radius 10, soft shadow.
Content clipped to the rounded rect.

**Menus & popups** — inherit `window_*`/`menu_corner_radius` + `popup_shadow`: raised sheets.

**Status bar** — chips instead of plain text: `SCAN` pill (`tint(panel, ok, .16)` bg, `ok`
text, `RADIUS_PILL`), `postgres@user` in `text_dim`, version pill (`accent_soft` bg,
`accent_hi` text).

**Editor** — bg `field_bg`, gutter `gutter`, caret `accent` (coral caret = signature),
selection `editor_sel`, current line `active_line`.

**Messages grid (errors)** — rows with Status = Error: status cell text `danger` + 2px
`danger` left bar on the row; Fatal same; plain rows unchanged.

**Modals** — as v1 layout rules (title row, label→field rhythm, right-aligned button bar,
no dead space), now with thin controls, `modal_frame()` shadow, focused-field coral ring.

## 7. Don'ts (v2)

- No coral on hover, headers, borders-at-rest, or decoration. Accent = meaning.
- No second border colour; no shadows beyond `island_shadow()`; no glow.
- Don't round the window outline; don't change chrome row heights.
- Don't hardcode hex/gaps at call sites — `theme::p()`, `SPACE_*`, `RADIUS_*` only.
- Don't let any surface skip the theme switch (no lingering `LIGHT.`/const usage).
- Don't touch virtualization/scrolling/caret logic in editor and grid.
