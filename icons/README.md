# JustQuery icons — an Ionicons-based set (31 glyphs)

Since 2026-06 the set is based on **Ionicons** (MIT) — thin lines. Source grid
**512×512**, stroke **~32–48**, rounded caps/joins, colour — **currentColor**.
The command glyphs `run`/`stop` are **filled** (Ionicons `play` / `flash`), the colour
carries meaning (Execute green, Stop red); the rest are outlined (`*-outline`). Semantic
substitutions where Ionicons has no direct match: `database / Connection mgr → server`,
`Metadata mgr → library`, `schema / XML model → document-text`, `table → grid`,
`sequence → reorder-four`, `function → code-slash`, `format → code <>`,
`connect → enter`, `disconnect → exit`, `trigger → flash`. Hand-drawn
(not from Ionicons): `save-as` (a floppy + a small "+" badge), `chevrons-up`/`chevrons-down`
(double chevrons shaped from the single `chevron-down`). The only glyph still drawn
entirely in code is `paint_cross` (the close cross — it has no codepoint in the font).

The glyphs are compiled into `assets/justquery-icons.ttf`; the source SVGs live in `icons/*.svg`,
and the "name → codepoint" map is in `icons/codepoints.json`.

## Name → purpose → codepoint (PUA)

| File | Purpose | Codepoint |
|---|---|---|
| new-query | toolbar: new query | U+E900 |
| open | toolbar: open file | U+E901 |
| save | toolbar: save | U+E902 |
| database | toolbar/trees: database, connection object | U+E903 |
| schema | metadata tree: schema | U+E904 |
| table | tree: table | U+E905 |
| view | tree: view | U+E906 |
| sequence | tree: sequence | U+E907 |
| function | tree: function | U+E908 |
| trigger | tree: trigger | U+E909 |
| run | editor sub-toolbar: execute (filled) | U+E90A |
| stop | sub-toolbar: stop fetch (filled) | U+E90B |
| commit | sub-toolbar: commit | U+E90C |
| rollback | sub-toolbar: rollback | U+E90D |
| format | sub-toolbar: format SQL | U+E90E |
| find | search / find bar | U+E90F |
| panel-left | toolbar: toggle Connection Manager | U+E910 |
| panel-tree | toolbar: toggle Metadata Manager | U+E911 |
| close | crosses: tabs, docks, result panel, modals | U+E912 |
| chevron-right | tree disclosure (closed), tab-strip scroll → | U+E913 |
| chevron-down | tree disclosure (open), combo boxes | U+E914 |
| chevrons-up | result panel: collapse/expand upward | U+E915 |
| chevrons-down | result panel: return downward | U+E916 |
| plus | docks: add connection | U+E917 |
| trash | docks: delete connection | U+E918 |
| refresh | metadata dock: rescan; status scan | U+E919 |
| plug | toolbar: connect/disconnect toggle | U+E91A |
| plug-off | toggle variant (if we decide to show state by icon) | U+E91B |
| key | spare (if we want a "key" for connect) | U+E91C |
| check | Inspect/Validate | U+E91D |
| save-as | Save As (floppy + "+") | U+E91E |

## Accent rules (from the design system)

- Toolbar/panel icons: `text_dim`; hover → `text`; disabled → `disabled`.
- **Command icons carry colour**: Execute — `ok` (green) when a run is available;
  Stop — `danger` (red) while something is running.
- Connection toggle: the `plug` glyph always; colour = state (`text_dim` disconnected,
  `ok` connected); tooltip "Connect…" / "Disconnect".
- Panel actions (chevrons-up/down, close) over the grid: `text_dim` → `text` on hover,
  never accent.

## egui integration

1. `assets/justquery-icons.ttf` is loaded in `theme.rs::setup_fonts` as the `jq-icons` fallback
   (pushed to the tail of Proportional and Monospace) so the PUA codepoints render in any text run.
2. The codepoint constants live in `src/icons.rs` (`pub const RUN: &str = "\u{E90A}";` etc.,
   per the map above). The only glyph drawn in code is `icons::paint_cross`.
