# JustQuery — UI Refinement: Implementation Plan (for Claude Code)

You are refining the JustQuery UI to match `DESIGN_SYSTEM.md`. Work in **phases, in order**.
After each phase: `cargo build --release` (and `cargo test` where noted), then visually verify the
acceptance criteria before moving on. Make one focused commit per phase.

**Stack:** Rust + egui/eframe **0.34** (Windows, MSVC). `CornerRadius` is `u8`-based.
**Seam:** `src/theme.rs` (Palette + `apply()`), `src/widgets.rs` (`crisp_border`, `show_modal`),
`src/main.rs` (screen layout). Read `DESIGN_SYSTEM.md` first — it defines every value you'll use.

## Global guardrails (apply to every phase)
- **Do not** change `CAPTION_H` / `TABBAR_H` / `CHROME_PAD` / `SUBBAR_H` values or their
  relationships — the equal-gap chrome math depends on them.
- **Do not** alter the virtualized editor (`codeeditor.rs`) or result grid (`grid.rs`) layout /
  scrolling / caret logic. You may change only colours and corner radii there.
- Keep every border **one physical pixel**, pixel-snapped (the `crisp_border` approach). Rounding
  must not blur it.
- Keep the public API of `theme.rs` stable (all existing `pub const`/`pub fn` names). The provided
  `theme.rs` already does this; new items are additive.
- Accent blue is for meaning only (primary / selected / focus / link). Never hover, never decoration.
- No drop shadows. The window outline stays square.

---

## Phase 0 — Baseline
1. Build and run the current app; screenshot Connect dialog, Scan tab, About tab, main editor with a
   result grid, and an open menu. Keep these as "before" references.
2. Confirm egui version in `Cargo.toml` is 0.34.x. If it differs, note it — `CornerRadius` API may
   change (older `Rounding`); adapt the provided `theme.rs` accordingly.

**Acceptance:** clean build; before-screenshots captured.

---

## Phase 1 — Drop in the new theme
1. Replace `src/theme.rs` with the provided `theme.rs` (rounded corners, elephant accent, type +
   spacing scale, neutral hover). It preserves all public names, so the crate should still compile.
2. `cargo build --release`. Fix only compile breakages (there should be none). Do **not** chase
   visual call-site issues yet.

**Acceptance:** builds; app launches; corners are now rounded on standard egui widgets (buttons,
combos, menus); hover is neutral grey; pressing a button flashes elephant blue.

---

## Phase 2 — Round the hand-painted borders
The islands, sheets and window outline are painted manually, so `Visuals` rounding doesn't reach
them. Find `crisp_border` in `src/widgets.rs`.
1. Give `crisp_border` a corner-radius parameter (default to `theme::RADIUS_ISLAND`). Paint the
   frame with `Painter::rect_stroke(rect, CornerRadius::same(r), stroke, StrokeKind::Inside)` (or the
   0.34 equivalent), keeping the existing pixel-snapping of `rect`.
2. Update call sites: islands / sheets / result-grid frame / modal backdrops → `RADIUS_ISLAND`;
   any field/control frame painted by hand → `RADIUS_CONTROL`.
3. Leave the **window outline** call square (radius 0).
4. If any rounded fill bleeds past its rounded border, clip the content rect or paint the fill with
   the same radius first.

**Acceptance:** every island, modal and grid frame is rounded 8px with a crisp 1px border at 100%
and 150% DPI; the window outline stays square; no fill bleeds past a corner.

---

## Phase 3 — Connect dialog (`connections.rs` / wherever the Connect modal is built; `widgets.rs::show_modal`)
Target = the approved mockup. Implement per `DESIGN_SYSTEM.md §6 Modals` and `§7`.
1. Call `style_modal_widgets(ui)` for the dialog's controls.
2. Layout top-to-bottom: title row (`Connect` as 16px heading + `×`), `SPACE_4`; then for each of
   Connection / Login / Password: a `Small`/`text_dim` label, `SPACE_2`, a full-width field/combo
   (all three share one width), `SPACE_3` between rows.
3. Remove the empty band. After the last field, `SPACE_5`, then a **right-aligned** button bar:
   `Cancel` (secondary) then `Connect` (primary). Make a small `primary_button` helper in
   `widgets.rs` (accent fill, white text, `ACCENT_PRESS` on press, `RADIUS_CONTROL`) and use it here.
4. Focused field shows the accent border (Design System §6 Text fields).

**Acceptance:** matches the mockup; no dead vertical space; one filled (blue) button; fields aligned;
keyboard focus shows a blue ring.

---

## Phase 4 — Scan tab (`meta_manager_modal.rs`)
1. Settings (`Scan interval`, `Sleep after idle`, `Budget`): lay out as a 2-column grid — label
   column (right-padded, `text_dim`, `Small`) + field column with one shared field width and left
   edge. Rows separated by `SPACE_3`.
2. Transfer list: two equal-width list boxes framed as sheets (`RADIUS_ISLAND`); the 4 arrow buttons
   in a vertically-centered column between them, equal `SPACE_2` gaps, as icon buttons.
3. Activity log: frame as a sheet (`RADIUS_ISLAND`), monospace text, `row_alt`-tinted timestamp gutter.
4. Button bar on one baseline: `Disable` + `Rescan now` (secondary, left), `Apply` (primary, right).

**Acceptance:** labels/fields align to shared columns; arrows centered as a group; log framed; button
bar aligned; spacing all from `SPACE_*`.

---

## Phase 5 — Chrome polish (`main.rs`, `widgets.rs`)
1. Toolbar: group icons; separate groups with a 1px `border` divider (16px tall, `SPACE_1` margins).
   Icon boxes rounded `RADIUS_CONTROL`, neutral hover.
2. Tabs: active tab = `ivory` fill + 2px `accent` bottom bar + top corners rounded `RADIUS_CONTROL`;
   inactive = `text_dim`, transparent. Close `×` on active/hover only.
3. Menus already inherit `menu_corner_radius`/`window_corner_radius` from Phase 1 — verify items use
   neutral hover and selected combo entries use `select` bg.

**Acceptance:** toolbar reads as tidy groups; the active tab is obvious via the blue bar; menus rounded.

---

## Phase 6 — Lists, trees, grid selection
1. Connection Manager rows and Metadata tree nodes: selected = `select` bg + 2px `accent` left bar;
   hover = `hover` bg; type icons `text_dim` (→ `text` when selected).
2. Result grid: header `grid_header`, zebra `row_alt`, selection rectangle `editor_sel`, frame from
   Phase 2. Colours only — no layout changes.

**Acceptance:** selection is unmistakable (tint + blue bar) but quiet; grid recoloured, still
virtualized and smooth.

---

## Phase 7 — About tab + status bar
1. About: align content to one left margin (`SPACE_5` from edge); logo + `JustQuery` at heading size;
   `SPACE_4` between blocks; soft `border` divider; `Check for updates` is a secondary button.
2. Status bar: right-aligned group per Design System §6; `SCAN` chip via `ui_bold_font`, coloured by
   state; version in `accent`.

**Acceptance:** About reads aligned and calm; status bar matches spec.

---

## Phase 8 — Consistency audit & sign-off
1. Grep for hardcoded radii/gaps at call sites (`CornerRadius::same(`, raw `Vec2::new`/pixel gaps in
   layout code) and replace with `RADIUS_*` / `SPACE_*` where they represent design spacing.
2. Confirm no `CornerRadius::ZERO` remains except the window outline.
3. Confirm accent blue appears ONLY on: primary buttons, active tab bar, selected row/tree bar,
   focused-field ring, links, version. Anywhere else → make it neutral.
4. Re-screenshot all surfaces; diff against `DESIGN_SYSTEM.md`. `cargo test` (logic + headless render
   smoke tests) must pass.

**Acceptance:** no stray sharp corners, no stray blue, spacing all tokenised, tests green, screens
match the design system.

---

## Suggested commit messages
`theme: rounded corners + elephant accent + type/spacing scale` · `widgets: round crisp_border` ·
`ui: redesign Connect dialog` · `ui: align Scan tab` · `ui: chrome polish (toolbar/tabs/menus)` ·
`ui: list/tree/grid selection accent` · `ui: About + status bar` · `ui: consistency audit`
