# JustQuery — Design System (v2.3)

The single source of truth for the JustQuery look. Lineage: v2 «Warm Studio» → v2.1 «Matte»
→ v2.2 «Two Colours» → v2.3 (this revision — flattened status bar, modal Scan/About, borders
drawn over content, no status-bar chips). Where revisions disagreed, the later rule wins.
Token values below are verified against the code.

Implementation seam: `src/theme.rs` (dual `Palette` + runtime `p()`, `apply()`) and
`src/widgets.rs` (`island`, `crisp_border`, `island_box`, `primary_button`/`*_button_w`,
`uniform_button_width`, `status_chip`, `show_modal`, `form_row`, `manager_row`, `tab_strip`).
egui **0.34** (`CornerRadius` is `u8`).

---

## 1. Principles

1. **One loop, no clutter.** The center of gravity is *type a query → get a result*. Chrome
   stays quiet; the editor and the data are the loudest things on screen.
2. **Coral = the chosen thing.** Coral (the logo colour) marks the committed/primary action,
   the current selection, the focus ring, the caret, and links. Never hover, never decoration.
3. **Green = healthy / go.** `ok`-green marks live, working state: the connected identity, an
   active scanner, the *armed* Execute button, the on-the-latest version. Amber = needs
   attention (scan asleep, update available); red (`danger`) = error / destructive.
4. **Two surface colours.** Statically a theme has exactly two surface colours — SURFACE and
   CHROME (§2). Everything else is a derived or interaction state.
5. **One border.** A single 1.0-logical-px `border_strong` frame. For a static field/popup it
   is one `RectShape` (fill+stroke). For an island whose content paints to its own edge
   (editor, result grid, manager/tree lists) the border is drawn **on top** of the content so
   the fill can't leave a hairline halo (§4).
6. **One rhythm.** All spacing from the `SPACE_*` scale; shared label/field columns.
7. **Gentle depth.** Raised surfaces carry a soft `island_shadow()` — the only decoration. The
   window outline stays square.
8. **Thin controls.** Every one-line control is `CONTROL_H = 24` px tall (§5).
9. **Matte dark theme.** Dark is a warm studio: brown-graphite surfaces, muted warm-grey text,
   no pure white anywhere (including text on coral). Built for long sessions.

The PostgreSQL elephant lives on as a wink: `syn_fn` (SQL functions) stays blue in both themes.

---

## 2. Surface model: SURFACE / CHROME

Statically a theme has exactly **two** surface colours:

- **SURFACE** — content-bearing: managers/docks, editor, result grid, input fields, combos,
  dropdown lists, custom popups.
- **CHROME** — everything else: window background, tab strip, toolbars and subbars, the status
  bar, modal bodies (fields on them are SURFACE), grid header, **the line-number gutter**, and
  **native menus / tooltips** (`window_fill`, §7).

**Law of both themes: CHROME is always darker than SURFACE.** Panels with the same role share
the same token, so they match across themes automatically.

The only derived exception is the grid zebra (`row_alt`). Interaction states (hover / select /
accent) sit on top of the two surfaces.

| | Light | Dark |
|---|---|---|
| SURFACE | `#FDFCF9` | `#2B2622` |
| CHROME | `#F1EEE9` | `#1F1B18` |

---

## 3. Colour tokens

All values live in `theme.rs` (`LIGHT` / `DARK`); call sites read `theme::p().<field>` — never
a hex, never a bare const.

| Token | Light | Dark | Use |
|---|---|---|---|
| `ivory` | `#FDFCF9` | `#2B2622` | SURFACE — islands, modals, menus, popups |
| `panel` / `panel2` | `#F1EEE9` | `#1F1B18` | CHROME — caption, toolbars, tab strip, docks |
| `grid_header` | `#F1EEE9` | `#1F1B18` | CHROME — sticky grid header |
| `data_bg` | `#F1EEE9` | `#1F1B18` | CHROME — status-bar/form backdrops |
| `row_alt` | `#F7F5F1` | `#26221F` | Zebra rows (the one derived tone) |
| `field_bg` | `#FDFCF9` | `#2B2622` | SURFACE — field interiors AND the editor/grid background |
| `gutter` | `#F1EEE9` | `#1F1B18` | = CHROME (panel): the line-number gutter, no third tone |
| `border` / `divider` | `#DCD8D1` | `#3A342E` | Soft dividers |
| `border_strong` / `menu_border` | `#C5BFB6` | `#453E37` | THE canonical frame |
| `text` | `#2A2723` | `#D5CEC3` | Primary text |
| `text_dim` | `#8F897F` | `#9D958A` | Labels, captions, secondary, transient status |
| `disabled` | `#C9C3BA` | `#615A50` | Inert controls, separators |
| `accent` | `#C05A33` | `#D97757` | Primary fill, selection, active-tab marks, caret |
| `accent_hi` | `#B5522D` | `#E58D69` | Accent as text/lines/icons (readable on `panel`) |
| `accent_press` | `#A84A28` | `#C26344` | Primary button pressed |
| `accent_soft` | `#F5E5DC` | `#3C2E26` | Active-tab pill fill |
| `on_accent` | `#FFFFFF` | `#FBF2EC` | Text on accent/danger fill (dark: cream) |
| `hover` | `#E6E1DA` | `#34302A` | Neutral hover fill |
| `select` | `#F3DDD2` | `#423126` | Selected list/tree row |
| `acc_bg` / `acc_bg2` | `#E6E1DA` / `#DDD7CF` | `#332E28` / `#3B352E` | Darker neutral fills (menu-bar/toggle states) |
| `ok` | `#61863C` | `#9DBE7B` | Connected / scanning / armed Run / latest version |
| `warn` | `#C28A1A` | `#D6A65A` | Scan asleep / update available |
| `danger` | `#B7352A` | `#E2625A` | Errors / destructive — kept apart from coral |
| `gutter`/`active_line`/`editor_sel` | … | … | Editor: gutter (CHROME), current line, selection |
| `shadow` | `rgba(4,3,2,.08)` | `rgba(0,0,0,.28)` | Island shadow |

SQL syntax — light: kw `#A04A26`, str `#5B7A45`, com `#A8A191`, num `#8A6D2F`, fn `#3F6F9F`.
Dark: kw `#E08A66`, str `#9CBF7A`, com `#756D60`, num `#CFAF72`, fn `#7FA9D0`.

Field state: **focus = `accent` border, error = `danger` border** (danger wins if both).

---

## 4. Shape, depth & borders

| Token | Value | Applies to |
|---|---|---|
| `RADIUS_CONTROL` | **4** | Buttons, fields, combos, menu items, tabs |
| `RADIUS_ISLAND` | **6** | Islands, modals, popups/menus, grid frame |
| window | 0 | Square — custom chrome & resize hit-testing |
| `island_shadow()` | offset [0,1], blur 4 | Raised surfaces; modals use blur 8 |

No pills, no radius > 6, no glow. `island_shadow()` is the only shadow.

**Two border idioms:**
- *Static field / popup / chip* — fill + 1.0 inside stroke as ONE `RectShape`
  (`widgets::island_box`, `Frame.stroke`). No manual pixel snapping.
- *Content-filled island* (editor, result grid, manager/tree list) — the content paints its
  own background right to the frame, so the 1px border is drawn **last, over the content**
  (`crisp_border` / `rect_stroke` after the body, e.g. `widgets::island`). A stroke behind the
  content would leave the SURFACE fill showing as a white hairline halo on all four sides.

**Editor frame** rounds all four outer corners. The line-number **gutter** (CHROME) rounds only
its outer-left corners; the **gutter↔text seam stays square** (it's a divider, not a corner).
The first code line sits flush at the top (no top padding); the active-line highlight rounds its
top corners on line 0 so it never spills past the frame.

---

## 5. Controls & buttons

| Parameter | Value |
|---|---|
| Height of every one-line control | **`CONTROL_H = 24`** (`BTN_H`/`FIELD_H` alias it) |
| Radius | 4 |
| Button horizontal padding | **14** (`button_padding = Vec2::new(14.0, 4.0)`) |
| Field/combo text inset (both sides) | **8** |

**Buttons react** — primary darkens on hover and again while pressed (`accent`→darker→
`accent_press`); destructive likewise on `danger`; secondary fills `hover`. A flat, inert fill
read as "not clickable".

**Modal button bars (the rule):** every button on one modal is the **same width** — measured
from the longest label via `uniform_button_width`, rendered with the `*_button_w` variants —
**right-aligned, at the bottom**. A modal has exactly one accent (primary/destructive) button;
the rest are secondary. In an *informational* modal (About) and where the action is staged
rather than committed (Scan), **Close is the accent/primary** (rightmost) and the page action
sits to its left as a secondary button. Enter presses the accent button, Esc/× closes.

**Main-menu bar buttons** match the action-button geometry (padding 14×4).

---

## 6. Typography & spacing

Segoe UI 13 body/button · 11 small · 16 semibold heading; JetBrains Mono 13 for code/grids
(bold in the editor). `ui_bold_font` (the `ui-bold` family — carries the icon glyphs as a
fallback) for emphasis. Spacing `SPACE_1..5` = **4/8/12/16/24**.

Chrome metrics, frozen: `CAPTION_H = TABBAR_H = 30`, `CHROME_PAD = 4`, `SUBBAR_H = 26`.

**Form law (`form_row`):** label (`Small`, `text_dim`, 16) → 4px → control → 16px to next row.
The label sticks to its own field. Hand-assembled label+field stacks are forbidden. (Compact
forms — e.g. the three Scan numeric settings — may lay their columns out horizontally instead.)

**Gutter law:** an **8px** gutter between neighbouring islands and the window edge — *except* the
work area now runs **flush to the status bar** (no chrome strip between them).

---

## 7. Components

**Window chrome.** Caption (logo + menus + window buttons), then the icon toolbar, then the tab
strip — all CHROME, 30px rows. Menu-bar items open on click and **roll over**: with one menu
open, hovering another switches to it. Toolbar icons are `text`; the connection toggle is a
**full-strength `text`** plug / plug-off (never dimmed) — one of connect/disconnect is always
live; click connects or opens the disconnect-confirm.

**Tabs.** Active = `accent_soft` pill, `accent_hi` text, radius 4, subtle lift. Inactive =
transparent, `text_dim`, neutral hover. The close **× shows on every tab** (active + inactive):
`text_dim` at rest, `accent_hi` on the active tab, `danger` on hover. A dirty tab confirms first.

**Menus & native popups.** `window_fill` = **CHROME (the darker tone)** — menus and tooltips
read as dark sheets, radius 6, `island_shadow`, items radius 4 with `hover` fill. Custom combos
(`styled_combo`) keep their SURFACE body; the popup is an island (radius 6) with rows clipped to
the rounding and the first/last row's hover/selection rounded to the frame.

**Lists & trees (`manager_row`).** Selection = `select` fill, hover = `hover` fill — full width,
**edge-to-edge**, with the first/last row (where it meets the island frame) rounded to match
(auto-detected from the clip rect). **No accent left bar.** The list island fills to the frame
and draws its border on top (§4).

**Result grid.** `island` sheet: `field_bg` fill, border on top, radius 6 (the grid's own
`grid_header` base fill is rounded to match — no corner halo). Sticky header + `#` column
`grid_header`; zebra `row_alt`; cell selection `editor_sel`. Messages rows with Status =
Error/Fatal: `danger` text + 2px `danger` left bar.

**Editor.** Background `field_bg`, gutter CHROME, caret `accent`, selection `editor_sel`,
current line `active_line`; **Execute icon is `ok`-green when armed**. Empty state: one centered
`text_dim` line, "Ctrl+N — new query".

**Status bar.** A **plain bottom strip** (CHROME fill, no island, no border, no top divider),
flush under the work area. Every element is the same font/size (Segoe 12) on one centred axis.
Left: `Ln, Col · UTF-8 · LF · <transient message>` (the `·` separator only when both a caret
block and a message are present; transient messages are `text_dim` — green is reserved for
health). Right, flush to the editor's right margin (8px): `scan · login@conn · version` — all
**plain coloured labels** (no chip background, no glyph). `scan` only while connected, coloured
by scanner state; `login@conn` green when live / red if dropped; `version` green on the latest
build, amber when an update exists. Click `scan` → Scan modal, `version` → About modal.

**Docks (Connection / Metadata Manager).** Left panel, **min width = the header title** (never
narrower; no truncation). Header = title + close ×; a subbar holds the page actions (+/trash;
schema combo/refresh). The connection-settings page has its own editor-style subbar with **Save
+ Test-connection icons** (Save disabled when the required fields aren't filled) — no buttons in
the body.

**Modals.** `modal_frame()`: SURFACE fill, 1px border, radius 6, shadow, 20px margin. Title row
(heading + ×), content, then the right-aligned uniform button bar (§5). A status region whose
content changes size (spinner ↔ result ↔ progress) is given a **fixed height** so the footer
never shifts. The dim backdrop swallows outside clicks; Enter/Esc per §5.
- **Scan** (was a tab) — header, the three numeric settings laid out **horizontally**, the
  monitored-schema transfer, a short activity log (last few entries, fixed box). Footer:
  Disable/Enable + Rescan now (left) · Apply (secondary, **disabled when nothing is staged**) +
  Close (accent, right).
- **About** (was a tab) — logo + name + version label (green/amber), then a fixed-height update
  status region. Footer: Close (accent) + an adaptive update action — **Download & Install** when
  one is available; while downloading / installing / pending-restart **only Close** shows (the
  user restarts manually, per the message).
- **Test connection** — one modal: a spinner + "Testing…" + inert OK; on completion the result
  fills the same fixed-height area in place and OK activates (no rebuild/resize). × cancels.

**Scrollbars.** Floating 8px pills, radius 4, 4px inset, auto-hide (fade with the global
`animation_time`). No track. egui scrolls + all custom ones (grid, editor, log).

**App / taskbar icon.** The clay "JQ" monogram (`app_icon` rasterises the same J polyline +
Q ring/tail as `widgets::logo`).

---

## 8. Motion

`animation_time = 0.15` (drives the scrollbar auto-hide fade; snappy hover/state otherwise). No
modal fades, no scroll easing (custom kinetic scroll owns it).

---

## 9. Don'ts

- No coral on hover, headers, borders-at-rest, or decoration. Coral = the chosen thing; green =
  healthy/go; amber = attention; red = error/destructive.
- No third static surface colour — map new surfaces to SURFACE or CHROME (only `row_alt` is
  derived).
- No pure white (`#FFFFFF`) anywhere in the dark theme, including text on coral.
- For a content-filled island, never leave the border behind the content — draw it on top, or a
  SURFACE-fill hairline shows as a white halo.
- No status-bar chips/pills or glyphs — the right group is plain coloured labels, all one size.
- No buttons in a tab/page body — actions go to a subbar, or it's a modal.
- A modal's buttons are uniform width, right-aligned, at the bottom; a modal whose body changes
  size pins a fixed-height region so the footer never moves.
- Content never touches rounded frames; scrollbars never touch frames; the gutter↔text seam is
  square, not rounded.
- No pills; no radius > 6. Don't round the window outline. Don't change chrome row heights.
- No hardcoded hex/gaps at call sites — `theme::p()`, `SPACE_*`, `RADIUS_*`, `CONTROL_H`.
- Don't let a surface skip the theme switch; don't touch virtualization/scroll/caret logic in
  the editor and grid.
