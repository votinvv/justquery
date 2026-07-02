# JustQuery — Design System

The single source of truth for the JustQuery look. Token values below are verified against the code.

Implementation seam: `src/theme.rs` (dual `Palette` + runtime `p()`, `apply()`) and
`src/widgets.rs` (`island`, `crisp_border`, `island_box`, `primary_button`/`*_button_w`,
`uniform_button_width`, `status_chip`, `show_modal`, `form_row`, `manager_row`, `tab_strip`).
egui **0.35** (`CornerRadius` is `u8`).

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
5. **One border, crisp.** A single `border_strong` frame, exactly **one device pixel** wide
   (`widgets::hairline` = `1/pixels_per_point`) on a pixel-snapped rect, so it stays razor-sharp at
   any DPI while feathering keeps the rounded corners smooth. For a static field/popup it is one
   `RectShape` (fill+stroke). For a content-filled island (editor, result grid, manager/tree lists)
   the fill and border share **one snapped rect** — `widgets::island_panel` paints fill under the
   content and border on top — so there is neither a corner seam nor a hairline halo (§4).
6. **One rhythm.** All spacing from the `SPACE_*` scale; shared label/field columns.
7. **Gentle depth.** Raised surfaces carry a soft `island_shadow()` — the only decoration. The
   restored window is rounded by the OS (Win11 DWM); a maximized window stays square (Windows
   squares it, by convention — it fills the work area).
8. **Thin controls.** Every one-line control is `CONTROL_H = 22` px tall (§5).
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
| `gutter` | `#F1EEE9` | `#1F1B18` | = CHROME (panel): the editor & result-grid line-number gutter, no third tone |
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
| `RADIUS_ISLAND` | **4** | Islands, modals, popups/menus, grid frame |
| `RADIUS_ICON` | **2** | Toolbar icon hover-boxes (softer than 4 on a small square — 4 reads as a bubble) |
| window | OS | Rounded when restored (Win11 DWM `DWMWCP_ROUND`, set in `startup::apply_rounded_corners`); square when maximized |
| `island_shadow()` | offset [0,1], blur 4 | Raised surfaces; modals use blur 8 |

No pills, no glow. Radii live on one short scale — **4** (controls and islands) and **2** (icon
hover-boxes); `island_shadow()` is the only shadow.

**Crisp 1px lines.** Every line — island/button/field borders, dividers, grid separators, the
window outline — is **one device pixel** (`widgets::hairline` = `1/pixels_per_point`), drawn on a
pixel-snapped path (`widgets::snap_rect` for rects, `Painter::round_to_pixel_center` for
`vline`/`hline`). A `1.0`-*logical* stroke smeared across ~1.5 device pixels at 125/150% scale; this
stays sharp at any DPI. `feathering` is left at the egui default, so the **rounded corners stay
smooth**. The native egui strokes match — `theme::apply` (menu/popup/window borders, hover borders,
selection) and `modal_frame()` are also 1 device px, computed at apply-time `pixels_per_point`
(re-applied on a theme switch).

**Two border idioms:**
- *Static field / popup / chip* — fill + inside stroke as ONE pixel-snapped `RectShape`
  (`widgets::island_box`, `Frame.stroke`). One shape ⇒ no fill/stroke divergence at the corners.
- *Content-filled island* (editor, result grid, manager/tree list) — the content paints its own
  background right to the frame. `widgets::island_panel` keys the shadow, fill and border to **one
  snapped rect**: shadow + fill go under the content, the border is drawn **last, over the content**
  (so an edge-to-edge fill can't leave a white hairline halo), and because fill and border trace the
  same snapped path there is no corner seam.

**Editor frame** rounds all four outer corners. The line-number **gutter** (CHROME) rounds only
its outer-left corners; the **gutter↔text seam stays square** (it's a divider, not a corner).
The first code line sits flush at the top (no top padding); the active-line highlight rounds its
top corners on line 0 so it never spills past the frame.

---

## 5. Controls & buttons

| Parameter | Value |
|---|---|
| Height of every one-line control | **`CONTROL_H = 22`** (`BTN_H`/`FIELD_H` alias it) |
| Radius | 4 |
| Button horizontal padding | **14** (`button_padding = Vec2::new(14.0, 3.0)`) |
| Text inset — every text-bearing control (both sides) | **`TEXT_INSET = 4`** (§6 Text-inset law) |
| Vertical field inset | **`FIELD_PAD_V = 2`** (`theme::field_margin()` = `(TEXT_INSET, FIELD_PAD_V)`) |

**Single-line fields & dropdowns are vertically centred.** Every one-line input — text fields,
combos (closed value **and** dropdown rows), inline editors, the find input — and every painted
single-line row — list / `manager_row` / grid cells — sits its text **dead-centre** in a `FIELD_H`
(22) box. Painted text centres on `rect.center().y` (`Align2::LEFT_CENTER`); a `TextEdit` reaches the
same place with **`.vertical_align(Align::Center)` + `theme::field_margin()` + `add_sized(FIELD_H)`**
(egui's default is `LEFT_TOP`, which would sit the text high — every field overrides it). The small
`FIELD_PAD_V = 2` keeps a field's min height within `FIELD_H` so `add_sized` pins it to exactly 22.
**All combos share one font** (`BODY_SIZE = 13`) so none reads a hair low next to its neighbours.

**Buttons react** — primary darkens on hover and again while pressed (`accent`→darker→
`accent_press`); destructive likewise on `danger`; secondary fills `hover`. A flat, inert fill
reads as "not clickable".

**Modal button bars (the rule):** every button on one modal is the **same width** — measured
from the longest label via `uniform_button_width`, rendered with the `*_button_w` variants —
**right-aligned, at the bottom**. A modal has exactly one accent (primary/destructive) button;
the rest are secondary. Enter presses the accent button, Esc/× closes. The singleton tabs
(Scan / About) carry **no footer buttons** — their actions live on the main toolbar and
the tab's own × dismisses.

**Main-menu bar buttons** match the action-button geometry (padding 14×4).

---

## 6. Typography & spacing

**One type scale (Segoe UI), each step a named token:** `BODY_SIZE = 13` (body / buttons / tabs /
list rows / **every editable field, combo and editor**) · `LABEL_SIZE = 12` (field & section
**labels**, key/value rows, status words, short validation messages — one step quieter than the
value it captions) · **10** fine-print (the deliberately-tiny hints under a field) · `HEADING_SIZE =
18` (**every** bold title — page / modal / confirm / empty-state / About). In-page sub-section
captions are `BODY_SIZE`-strong (a quieter tier than a title). The result grid (`GRID_SIZE = 12`) and
the status bar (`STATUSBAR_SIZE = 11`) keep their own size **by role**, not the `LABEL_SIZE` token. JetBrains Mono 13
for code/grids (in the SQL editor **only keywords are bold**; functions, numbers, strings, comments
and plain identifiers stay regular — colour, not weight, distinguishes them). `ui_bold_font` (the
`ui-bold` family — carries the icon glyphs as a fallback) for emphasis. Spacing `SPACE_1..5` =
**4/8/12/16/24**.

**Two heights, one constant.** `CAPTION_H = 30` is the full-width top bands (text-menu + main
toolbar). Everything below the main toolbar shares **one content height = the button height
`CONTROL_H = 22`**: `TABBAR_H = CONTROL_H` (editor/result tab strips, dock-manager headers) and
the manager/result sub-toolbars (`SUBBAR_H = CHROME_GUTTER + CONTROL_H = 26` band = a 4px top
spacer + a 22px icon row). So tabs, dock headers, toolbar icons, buttons and fields are all 22
tall — moving the one constant (`CONTROL_H`) moves the whole layer. On screen there are exactly
two heights: **30** (top chrome) and **22** (everything else).

**Toolbar icon buttons are square.** Every icon hover-box (`qbtn`/`qbtn_toggle`/`qchevron`/`close_x`)
fills its row as a **square** — side = the row height (≈30 in the main toolbar, **22** in the tab
strips / dock headers / sub-toolbars), corner radius `RADIUS_ICON = 2`. One inter-icon gap
`ICON_GAP = 2` across the main / manager / result toolbars; the group divider `|` is a bare
hairline, so `ICON_GAP` is the air on each side of it too (icon→divider == icon→icon).

**Form law (`form_row`):** label (`Small`, `text_dim`, 16) → 4px → control → 16px to next row.
The label sticks to its own field. Hand-assembled label+field stacks are forbidden. (Compact
forms — e.g. the three Scan numeric settings — may lay their columns out horizontally instead.)

**Gutter law:** one **4px** gutter (`CHROME_GUTTER`, theme.rs) for every gap on the main screen —
window edges of caption/toolbar/tabs/status, dock islands and sub-toolbars, the editor island, and
the gaps between tabs and between chrome rows — so every band is the same width. Change the one
constant to retune the whole screen's density.

**Text-inset law:** one constant — **`TEXT_INSET = 4`** (`= SPACE_1`, theme.rs) — is the horizontal
gap from the edge of **every text-bearing control** to its text, so a glyph sits the same 4px from
its frame everywhere. The value is the one the connection-settings / scan `TextEdit` fields
always carried (egui's built-in default margin); the rest of the app was unified down to it. It
drives: form fields & combos (the closed value **and** every dropdown option row), list / tree /
**`manager_row`** rows, **result-grid cells**, the **code editor** text (the gap from the
line-number gutter to the first glyph — `codeeditor::PAD_L`), inline rename/edit fields, the **find**
input and the **completion popup**. **Every** `TextEdit` passes an explicit
`.margin(TEXT_INSET, …)` — none rely on egui's default, so the one knob really moves them all. An
inline editor that overlays a static label carries the standard `field_margin()` and widens its
frame `TEXT_INSET` to the left of `widgets::MGR_LABEL_X` (`= TEXT_INSET + MGR_GLYPH_COL`), so the
field keeps the shared 4px inset **and** its first glyph still lands exactly on the label. The **line-number /
`#` gutter** is the other side of the divider, not a text inset, and keeps its OWN roomier geometry —
`GUTTER_PAD_L = 6` (edge→number), `GUTTER_PAD_R = 8` (number→divider), shared by the editor gutter
and the grid `#` column so the two read identically and the numbers don't crowd the divider when the
text inset is tightened. **Filled-label** controls keep their own, deliberately roomier label-padding
scale — button **14**, tab pill **10**, status chip **6** — and are *not* `TEXT_INSET` (a flat field
and a filled pill want different air).

**Components are flat; spacer-rows give the air.** A component never draws its own surrounding
space (no vertical inset around hover-boxes / pills): the hover box of a toolbar icon (`qbtn`,
`qbtn_toggle`, `qchevron`), a menu item, and the active tab **pill** all FILL their row. The 4px
air between rows is an explicit spacer-row — `widgets::vgap` (a `CHROME_GUTTER`-tall empty panel):
`gap_below_caption` (menu↔toolbar), `gap_below_toolbar` (toolbar↔tabs/headers), `gap_below_tabs`
(tabs↔editor), the `result_grab` strip (editor↔results, doubles as the resize handle), and the
result sub-toolbar↔grid gap. Horizontal seams use the same idea — only **one** side owns the 4px
so it never doubles: against an open dock the work-area left = 0 (the dock's own right gutter is
the seam). The split-zone left inset is single-sourced by **`JustQueryApp::dock_left()`** (`0`
under an open dock, else `CHROME_GUTTER`) — used by the tab strip, the editor/content islands
(`island_margin`), **and the result-panel header + its sub-toolbar**, so the result tabs/icons
line up with the grid and the editor tabs above (no extra 4px indent when the dock is open).

**Tab-strip scroll chevrons (`‹ ›`).** Shown only when tabs overflow. The pair is flush (no gap
between them): the chevrons are square (side = row height), so the reservation is exactly
`2 × row_h` with zero `item_spacing.x` before drawing them, so the right chevron lands exactly on
the strip's right edge. In the **editor** strip the reservation is `2 × row_h + vscroll::BAR`, so
the right chevron sits on the editor's *text* border (the editor reserves `vscroll::BAR` = 8px on
the right for its scrollbar gutter) rather than on the outer sheet edge by the window. In the
**result** header the action glyphs `⌄ ×` (maximize/restore + close) sit a toolbar gap apart
(`item_spacing.x = ICON_GAP` on the `right_to_left` row, same 22px squares and 2px gap as the
toolbar icons); only the scroll-arrow **pair** `‹ ›` stays flush (its own `item_spacing.x = 0`).
In the **editor** strip, overflow is judged against the **full** strip width (not the arrow-reserved
viewport), so the arrows never stay stuck after a dock closes when the tabs would now fit. The
**result** header still detects overflow with a one-frame lag (`arrows_w` keys off last frame's
result), so on a state change it calls `request_repaint()` and the chevrons appear/disappear
immediately after a background query, not only after the next mouse move.

Exceptions: the dock **title** is indented `DOCK_TITLE_INDENT` (8px) so it doesn't hug the edge;
the status bar's right margin is `RESIZE_GRIP_W` (22px) in a restored window to clear the OS corner
resize-grip; the work area runs **flush to the status bar** (no chrome strip between them); and
bordered data islands (Meta / Scan) inset their scroll content by 1px so rows never paint
over the island border.

---

## 7. Components

**Window chrome.** Caption (logo + menus + window buttons), then the icon toolbar, then the tab
strip — all CHROME, 30px rows. Menu-bar items open on click and **roll over**: with one menu
open, hovering another switches to it. Toolbar icons are `text`; the connection toggle is a
**full-strength `text`** plug / plug-off (never dimmed) — one of connect/disconnect is always
live; click connects or opens the disconnect-confirm.

The icon toolbar is **merged and static**: after the global actions (New / Open / Save, the
connection toggle) it carries one fixed **action group** — `Refact · Inspect · Execute ·
Stop` — drawn straight into the toolbar (there is **no separate per-tab band** under the tabs).
The set never changes between tabs; only each icon's live/dimmed state depends on the tab kind:
Refact = SQL refactor (parked → dimmed); Inspect = Test connection on a Connection tab (dimmed
elsewhere; SQL Inspect is parked); Execute = SQL (live when
connected + non-empty + idle, `ok`-green when armed); Stop = red while anything runs on the tab.
A connection-settings tab drives the same toolbar's Test / Save per-tab; the work-area sheet sits
flush under the tab strip.

**Tabs.** Active = `accent_soft` pill, `accent_hi` text, radius 4, subtle lift. Inactive =
transparent, `text_dim`, neutral hover. The close **× shows on every tab** (active + inactive):
`text_dim` at rest, `accent_hi` on the active tab, `danger` on hover. A dirty tab confirms first.
Editor tabs carry a small inter-tab gap and are **drag-reorderable** (drop position by pointer x
vs tab centres); **Ctrl+Tab / Ctrl+Shift+Tab** cycle forward/back. (`widgets::tab_strip`.) The
active pill **auto-scrolls into view** when the active tab changes programmatically (open / new /
manager selection / Ctrl+Tab) or when the strip viewport width changes (a dock opens/closes/resizes,
or the window resizes) — so opening a dock slides the leading tabs under it instead of pushing the
active tab past the right edge.

**Menus & native popups.** `window_fill` = **CHROME (the darker tone)** — menus and tooltips
read as dark sheets, radius 4, `island_shadow`, items radius 4 with `hover` fill. Custom combos
(`styled_combo`) keep their SURFACE body; the popup is an island (radius 4) with rows clipped to
the rounding and the first/last row's hover/selection rounded to the frame.

**Lists & trees (`manager_row`).** Selection = `select` fill, hover = `hover` fill — full width,
**edge-to-edge**, with the first/last row (where it meets the island frame) rounded to match
(auto-detected from the clip rect). **No accent left bar.** The list island fills to the frame
and draws its border on top (§4).

**Result grid.** `island` sheet: `field_bg` fill (light, like the editor — the base sheet and the
area to the right of / below the table are `field_bg`, **not** the header tint), border on top,
radius 4. Sticky **header** `grid_header`, filling the **full island width** (top corners rounded). The **`#`
column** is a flat `gutter` tone (no zebra in it, like the editor's line-number gutter) whose width
tracks the largest row number; it fills the **full island height**, its bottom-left corner rounded to
`RADIUS_ISLAND` — the header and the `#` column own the whole L-frame of the island, and the disappearing
overlay bars float over the data between them. Row numbers stay clipped to the data area so a half-scrolled
last row's number never shows below the last row. A **thin 1px grid**
(`border` hairline, pixel-snapped like every other line) rules the body: a horizontal separator
under each row (crossing the `#` gutter too) and a vertical separator at each column's right edge
(the last one closes the table) — both run only to the **end of the table** (last column's right
edge), never to the panel edge; past the table the base sheet stays blank. **Zebra** `row_alt`
likewise stops at the end of the table. **Cell** and **whole-row selection** (the latter via the
`#` gutter) are a plain `editor_sel` fill — **no outline** (a clipped accent border read
inconsistently at the data edges). The **active sort** is marked in the header by an `accent` arrow
↑/↓ plus its priority number when several columns are sorted. A fresh panel auto-sizes to fit
**exactly** `DEFAULT_RESULT_ROWS` (10) whole rows — no partial row — until the user drags it.
Messages rows with Status = Error/Fatal: `danger` text + 2px `danger` left bar.

**Editor.** Background `field_bg`, gutter CHROME, caret `accent`, selection `editor_sel`,
current line `active_line`; **Execute icon is `ok`-green when armed**. Empty state: one centered
`text_dim` line, "Ctrl+N — new query".

**Status bar.** A **plain bottom strip** (CHROME fill, no island, no border, no top divider), flush
under the work area — `STATUSBAR_H` (20px) tall, the one chrome row deliberately **off** the 22px
control grid so it packs dense. Every element is the same font/size (Segoe 11, `STATUSBAR_SIZE`) on
one centred axis, in plain `text`; segments are split by a **vertical hairline** (`toolbar_divider`
— the same `|` the main toolbar uses), never a dot. Left: `UTF-8 | LF | Ln 12 Col 4 | <transient
message>` — encoding, then EOL, then the caret (line, column — no absolute position). The transient
message is the active editor tab's process status (SQL run / Find), `text`
normally, `danger` on error — green is reserved for health. Right, flush to the editor's right margin
(the 4px gutter; 22px in a restored window for the resize-grip): `run-timer | scan | login@connection
| version`. `scan`, `login@connection` and `version` are clickable — coloured labels with **no
resting background**, just a hover accent (a soft 4px halo); the timer / caret / encoding are inert
labels. `scan` only while connected, coloured by scanner state; `login@connection` (the **connection
name**, not the db) green when live / red if dropped; `version` green on the latest build, amber when
an update exists. Click `scan` → Scan tab, `login@connection` → the **active connection's settings
tab** (its live Session block), `version` → About tab. **Air:** a uniform **5px** clears each divider
on both sides — the right chips reach it as 1px item-spacing + their 4px hover-accent padding, the
bare left labels (and the run timer, boxed to match) as a flat 5px item-spacing.

**Docks (Connection / Metadata).** Left panel, **min width = the header title**
(never narrower; no truncation). Header = title + close ×; a subbar holds the page actions. The
**Connection** manager carries New / **Import** (file → list, OPEN icon) / Delete
— **Export is the open tab's Save As**, not a dock button (one direction in the dock, the other on
the toolbar). In the **Connection** manager the **active (live) connection** reads **green** — its
plug glyph *and* its name take `ok`-green (the selection fill still shows when you click it). The
connection-settings **tab** contributes a **Test-connection** icon to the main toolbar; Save / Save
As are the toolbar's own icons (Save persists the connection — validating the required fields; Save
As exports it to a `.conn` file) — no buttons in the body.

**Connection-settings tab.** Title = the connection's **name** (18pt bold), the form (Name / Host /
Port / Database / User / Password) below it. The **active connection** (its `id` == the live
`active_conn_id`) is special: a green **`● active`** marker (red **`● disconnected`** + the failure
reason if the session dropped — the Scan-tab pattern) sits beside the title, its form fields are
**locked** (dimmed, non-interactive — a live connection can't be re-edited), and a **Session**
section below the form shows the runtime attributes — `Server · Database · Since` ｜ `User · Pid ·
SSL` (two columns). A non-active connection's tab is the plain editable form with just its title.

**Singleton tabs (Scan / About).** Opened from the status bar (`scan` / `version`); at most one of
each exists — reopening re-selects the existing tab. Like every tab, each maps its actions onto the
**main toolbar** (reusing the shared Save / Execute / Stop slots — no new icons), then renders the
page body on the silvery data sheet with normal tab scrolling. The body carries **no buttons of its
own**. Closing is the tab's own ×. (The live control-connection view is **not** a singleton tab — it
lives on the active connection's settings tab, above.)
- **Scan** — toolbar actions: Execute = **Enable** the collector (when paused), Stop = **Disable**
  it (when running), Save = **Apply** the staged settings (dimmed when nothing is staged). Body:
  the three numeric scan settings laid out **horizontally**, the monitored-schema transfer, and a
  short activity log (fixed box).
- **About** — the update status. A check that finds a newer build downloads it silently; the
  **install** is a click on the status *line* (clickable content, never a button). Body: logo +
  name + version label (green/amber) and a fixed-height update status region. On the latest build
  the status region is empty.

**Modals.** Reserved for connecting and action confirmations. `modal_frame()`: SURFACE fill, 1px
border, radius 4, shadow, 20px margin. Title row (heading + ×), content, then the right-aligned
uniform button bar (§5). A status region whose content changes size (spinner ↔ result ↔ progress)
is given a **fixed height** so the footer never shifts. The dim backdrop swallows outside clicks;
Enter/Esc per §5.
- **Scan / About** are **tabs**, not modals — see "Singleton tabs" above; the live connection view
  is part of the active connection's settings tab.
- **Test connection** — one modal: a spinner + "Testing…" + inert OK; on completion the result
  fills the same fixed-height area in place and OK activates (no rebuild/resize). × cancels.

**Scrollbars.** 8px, radius 4, no track. **Solid, reserved gutter** (`widgets::style_scrollbar`): the
form sheets, the scan activity-log box, and the multiline fields — the bar takes its own width so content
never sits under it, and egui's edge fade-gradient is disabled (it would cut off at a solid bar). **The
result grid's and code editor's custom `vscroll` bars** are **disappearing overlays** — they reserve
**no** width (the content fills the whole island; the frozen chrome — the grid's `#` gutter + header, the
editor's line-number gutter — runs edge-to-edge to the rounded corners) and float semi-transparent **on**
the content. Each handle **fades** in on activity (scroll / any pointer move inside the island / drag) and
out after a short idle (`vscroll::Fade`, whose state lives per-tab next to the scroll offset — self-
terminating so the UI idles); the tracks are **confined to the data region** (below the header, right of
the gutter) so a handle never rides onto the frozen chrome. Only when **both** axes scroll does each
viewport shrink by one bar, so the last row / column / line can slide **clear of the perpendicular handle**
at the very end (a clearance strip only there, never a permanent gutter) — the same shortening stops the
two tracks one bar short of the shared corner. **Floating overlay**: the manager lists
(Connection / Metadata, `widgets::style_scrollbar_overlay`) — the bar rides over the content
and reserves **no** width, so rows stay edge-to-edge (the selection accent reaches the frame) and a bar
appearing never reflows them; egui's edge fade-gradient is enabled there and spans the full width. The
global default (theme.rs) is solid + fade-off; managers opt in.

**App / taskbar icon.** The clay "JQ" monogram (`app_icon` rasterises the same J polyline +
Q ring/tail as `brand::logo`).

---

## 8. Motion

`animation_time = 0.15` (snappy hover/state animations). No modal fades, no scroll easing (custom
kinetic scroll owns it).

---

## 9. Don'ts

- No coral on hover, headers, borders-at-rest, or decoration. Coral = the chosen thing; green =
  healthy/go; amber = attention; red = error/destructive.
- No third static surface colour — map new surfaces to SURFACE or CHROME (only `row_alt` is
  derived).
- No pure white (`#FFFFFF`) anywhere in the dark theme, including text on coral.
- For a content-filled island, never leave the border behind the content — draw it on top, or a
  SURFACE-fill hairline shows as a white halo.
- No resting chip/pill background or glyphs in the status bar — the clickable items are coloured
  labels with only a hover accent (a 4px halo); all one size, split by vertical hairlines not dots.
- No buttons in a tab/page body — a tab's actions go to the main toolbar's static action group, a
  dock page's to its subbar, or it's a modal.
- A modal's buttons are uniform width, right-aligned, at the bottom; a modal whose body changes
  size pins a fixed-height region so the footer never moves.
- Content never touches rounded frames; scrollbars never touch frames; the gutter↔text seam is
  square, not rounded.
- No pills; radii stay on the 4 / 2 scale (4 controls & islands, 2 icon hover-boxes). Don't change
  chrome row heights. (The *window* outline is the one exception: the OS rounds it when restored —
  see §3/§4.)
- No hardcoded hex/gaps at call sites — `theme::p()`, `SPACE_*`, `RADIUS_*`, `CONTROL_H`, `TEXT_INSET`.
- No bespoke text inset on a field/row/cell/editor — route the horizontal gap through `TEXT_INSET`
  (§6 Text-inset law); only the line-number/`#` gutter (`GUTTER_PAD_*`) and filled-label controls
  (button/tab/chip) are exempt.
- No top-aligned single-line field. A `TextEdit` must set `.vertical_align(Align::Center)` +
  `theme::field_margin()` + `add_sized(FIELD_H)` (egui defaults to `LEFT_TOP`); combos/rows centre on
  `rect.center().y`. Combos all use `BODY_SIZE` (§5 — vertical alignment).
- Don't let a surface skip the theme switch; don't touch virtualization/scroll/caret logic in
  the editor and grid.
