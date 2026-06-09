# JustQuery — Design System

A single source of truth for the JustQuery look. It exists so the UI stops drifting:
every colour, size, gap and corner has one defined value and one reason. Hand this to Claude
Code together with `IMPLEMENTATION_PLAN.md`; the plan turns each section here into a task.

The implementation seam is `src/theme.rs` (the `Palette` + `apply()`) and the hand-painted
borders in `src/widgets.rs` (`crisp_border`). egui version: **0.34** (`CornerRadius` is `u8`).

---

## 1. Principles

1. **One loop, no clutter.** The center of gravity is *type a query → get a result*. Chrome
   stays quiet so the editor and the data are the loudest things on screen.
2. **One accent, used only for meaning.** The PostgreSQL "elephant" blue `#336791` appears only
   on: the primary action button, the current selection (tab / list row / tree node), the focus
   ring, and links. Never on hover, never as decoration. If blue shows up, it means *this is the
   committed / chosen thing*.
3. **One border, everywhere.** A single thin `border_strong` (`#BEC1C8`), drawn as exactly one
   physical pixel, frames every island, sheet, modal, field, dropdown and menu. No second border
   style, no shadows — the app is flat.
4. **One rhythm.** All spacing comes from the scale in §4. No magic numbers at call sites. Labels
   and fields align to shared columns so nothing looks hand-placed.
5. **Calm, rounded, native.** Corners are softly rounded (controls 6px, containers 8px). UI text
   is Segoe UI for OS crispness; code is JetBrains Mono. The window itself stays square (custom
   chrome + resize hit-testing assume square corners).
6. **Hover is neutral; pressed is blue.** Hover = soft grey fill + the canonical border. Pressed /
   selected = the accent. This keeps a busy toolbar quiet while still giving clear feedback.

---

## 2. Colour tokens

All live in `Palette` (`src/theme.rs`). Use the semantic name, never the hex, at call sites.

### Surfaces
| Token | Hex | Use |
|---|---|---|
| `ivory` | `#FCFCFD` | Lightest surface — editor, islands, modals, popups |
| `panel` / `panel2` | `#F3F4F6` | Chrome fill (caption, menu, toolbar, tabs, side panels) |
| `grid_header` | `#EEF0F2` | Sticky result-grid header |
| `data_bg` | `#F3F4F6` | Data-bearing surfaces (status bar, form backdrop) |
| `row_alt` | `#F6F7F9` | Zebra striping in tables |
| white | `#FFFFFF` | Field interiors (text edits, dropdown bodies) |

### Borders
| Token | Hex | Use |
|---|---|---|
| `border` | `#DADDE2` | Soft inner dividers, row separators, tab strip underline |
| `border_strong` | `#BEC1C8` | **The** canonical 1px frame (islands, modals, fields, menus, window) |
| `divider` | `#DADDE2` | Thin line between UI blocks (softened from the old heavy grey) |

### Text
| Token | Hex | Use |
|---|---|---|
| `text` | `#24262B` | Primary text |
| `text_dim` | `#8A8E96` | Labels, captions, secondary/disabled-but-readable text |
| `disabled` | `#C4C7CE` | Truly inert controls |

### Accent (elephant blue) — meaning only
| Token | Hex | Use |
|---|---|---|
| `accent` | `#336791` | Primary button fill, focus ring, active-tab bar, selection bar, links |
| `ACCENT_PRESS` | `#2A5577` | Primary button while pressed |
| `select` | `#E3EDF7` | Selected row / active-tab background (light elephant tint) |
| `editor_sel` | `#CDE1F8` | Text selection background |

### Hover / pressed (neutral)
| Token | Hex | Use |
|---|---|---|
| `hover` | `#EEF0F3` | Hover fill for any interactive box |
| `acc_bg` / `acc_bg2` | `#E8EAEF` / `#DDE0E6` | Darker neutral fills (menu/tab states, find bar) |

### Status (muted)
| Token | Hex | Use |
|---|---|---|
| `ok` | `#5A8A3C` | Connected, scan-active, success |
| `warn` | `#C28A1A` | Scan asleep, warnings |
| `danger` | `#BE3B2F` | Errors, destructive actions |

### SQL syntax (unchanged — already balanced)
`syn_kw #A04A26` · `syn_str #5B7A45` · `syn_com #A8A191` · `syn_num #8A6D2F` · `syn_fn #3F6F9F`

---

## 3. Typography

Set as egui `text_styles` in `apply()`.

| Style | Family | Size | Weight | Use |
|---|---|---|---|---|
| Heading | Segoe UI | 16 | semibold | Section titles ("Scan", "Updates"), tab-panel headers |
| Body / Button | Segoe UI | 13 | regular | Default UI text, labels' values, buttons |
| Small | Segoe UI | 11 | regular | Field labels, captions, status-bar text |
| Monospace | JetBrains Mono | 13 | — | Result grid, messages, anything tabular/data |
| Code (editor) | JetBrains Mono **bold** | 13 | bold | The SQL editor; non-highlighted tokens use the regular face |

App title ("JustQuery" on the About tab) is a one-off larger heading (~22px via `ui_bold_font`).
Emphasised chips (status-bar `SCAN`) use `ui_bold_font` so they actually read bold (egui's
`.strong()` only recolours).

---

## 4. Spacing scale

One scale, named in `theme.rs`. Replace ad-hoc gaps with these.

| Token | px | Use |
|---|---|---|
| `SPACE_1` | 4 | Icon ↔ label, tightest inner gap |
| `SPACE_2` | 8 | Default gap between controls; label ↔ field |
| `SPACE_3` | 12 | Between grouped rows within a section |
| `SPACE_4` | 16 | Between sections; modal content padding base |
| `SPACE_5` | 24 | Major separation (last field ↔ button bar) |

Chrome metrics stay fixed and must not be casually changed: `CAPTION_H = TABBAR_H = 30`,
`CHROME_PAD = 4` (so the equal blank gaps between rows = `2×CHROME_PAD`), `SUBBAR_H = 26`.

---

## 5. Corner radii

| Token | px | Applies to |
|---|---|---|
| `RADIUS_CONTROL` | 6 | Buttons, text fields, dropdowns, menu items, toolbar boxes, tab tops |
| `RADIUS_ISLAND` | 8 | Islands/panels, modals & sheets, result-grid frame, popups & menus |
| (window) | 0 | The window outline stays square (custom chrome + resize) |

Rounding must stay pixel-snapped to keep the 1px border razor-sharp at any DPI (see `crisp_border`).

---

## 6. Components

For each: the spec, then the concrete fix for the current screenshots.

### Window chrome (caption / menu / toolbar / tabs)
- All four rows are 30px tall; interactive boxes inset by `CHROME_PAD`, rounded `RADIUS_CONTROL`,
  neutral `hover` fill on hover only.
- Toolbar icon groups are separated by a 1px `border` divider, 16px tall, with `SPACE_1` on each
  side. Icons are 15–16px Lucide glyphs in `text` colour.

### Tabs (editor & result)
- Inactive: `text_dim`, transparent, leading status dot.
- **Active: `ivory` fill, `text` colour, a 2px `accent` bar along the bottom edge, top corners
  rounded `RADIUS_CONTROL`.** This is the elephant accent earning its place.
- Close `×` only on hover or on the active tab.

### Menus & dropdowns
- Body `ivory`, 1px `border_strong`, `RADIUS_ISLAND`, no shadow. Items rounded `RADIUS_CONTROL`,
  `hover` fill on hover. Selected item (e.g. current connection in a combo) uses `select` bg.

### Buttons
- **Primary** (one per dialog — Connect / OK / Apply): `accent` fill, white text, `RADIUS_CONTROL`,
  padding `SPACE_2`×(`SPACE_4`+2). Pressed → `ACCENT_PRESS`. This is the only filled button.
- **Secondary** (Cancel, Rescan now, Disable): white fill, 1px `border_strong`, `text` colour,
  same radius/padding. Hover → `hover` fill.
- **Icon button** (toolbar, transfer arrows): transparent, neutral `hover`, rounded.
- A dialog has **exactly one** primary button; everything else is secondary.

### Text fields & dropdowns
- White fill, 1px `border_strong`, `RADIUS_CONTROL`, 34px tall, `SPACE_2`+2 horizontal padding.
- **Focused: border becomes `accent` (1px).** That's the focus ring.
- Numeric fields (Scan interval, Budget) are left-aligned text in a fixed-width field; all fields
  in a form share one width and one left edge.

### Lists & trees (Connection Manager, Metadata tree)
- Row height ~26px. Selected row: `select` bg + a 2px `accent` left bar (`SELECT_BAR`); the bar is
  what makes selection unmistakable without shouting. Hover (unselected): `hover` bg.
- Leading type icon in `text_dim`; selected row icon + text in `text`.

### Result grid
- Frame: 1px `border_strong`, `RADIUS_ISLAND`, clipped so rounded corners hold.
- Sticky header `grid_header`, pinned `#` column same. Zebra rows `row_alt`. Cell selection
  rectangle uses `editor_sel`. Keep the virtualization untouched — only colours/radii change.

### Modals / dialogs
- `modal_frame()`: `ivory` fill, 1px `border_strong`, `RADIUS_ISLAND`, 20px inner margin.
- Layout: title row (16px heading + `×`), `SPACE_4` gap, content (label `Small`/`text_dim` →
  field, repeated with `SPACE_3` between rows), `SPACE_5` gap, then a **right-aligned** button bar
  (secondary then primary, `SPACE_2` apart). No dead vertical space — the modal hugs its content.

### Status bar
- 26px, `data_bg`, 1px top `border`. Right-aligned group: `SCAN` chip (bold glyph + label, colour
  by state: `ok`/`warn`/`danger`/`text_dim`), `text_dim` separators, `postgres@user` in `text_dim`,
  version in `accent`. Left side reserved for transient panic-recovery messages only.

### Transfer list (available ⇄ monitored, Scan modal)
- Two equal-width list boxes (1px `border_strong`, `RADIUS_ISLAND`), a centered column of 4 icon
  buttons between them (`»` `›` `‹` `«`), vertically centered as a group with equal `SPACE_2` gaps.

---

## 7. Screen fixes (what "разброд и шатание" means here)

**Connect dialog (image 1).** Remove the big gap between Password and the buttons. Equalise field
widths. Right-align a button bar: `Cancel` (secondary) + `Connect` (primary, accent fill). Consistent
label→field spacing (`SPACE_2`) and row→row spacing (`SPACE_3`). Modal hugs content (no empty band).

**Scan tab (image 2).** Align the three numeric settings to a label column + field column; give the
fields one width and left edge. Center the transfer arrows as a group. Frame the activity log like a
sheet (`RADIUS_ISLAND`). Button bar: `Disable`/`Rescan now` left, `Apply` (primary) right, all on one
baseline with `SPACE_2` gaps.

**About tab (image 3).** Align all content to one left margin (`SPACE_5` from the panel edge). Title
row: logo + app name at the heading size. `SPACE_4` between blocks; the divider is the soft `border`.

---

## 8. Motion
`animation_time = 0.05` (snappy hover/state). No easing on scrolling (custom kinetic scroll owns it).
No fades on modals — they appear instantly; the app should feel immediate, not animated.

---

## 9. Don'ts
- Don't use blue for hover, headers, borders, or "to look nice." Accent = meaning only.
- Don't introduce a second border colour or any drop shadow.
- Don't hardcode gaps/sizes at call sites — pull from `SPACE_*` / `RADIUS_*`.
- Don't round the window outline or change the chrome row heights.
- Don't touch the virtualized editor/grid layout logic — only its colours and corner radii.
