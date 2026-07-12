# JustQuery — Architecture

A description of the program's components, the contracts between them, and the cross-cutting
development patterns. This document describes **how** the system is built; **what** it does is in
[`REQUIREMENTS.md`](REQUIREMENTS.md), **how it looks** is in [`DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md).

Canon of behaviour: the wording here reflects the current state of the code. When a doc-comment in
the source disagrees with this document, the code wins; such places are flagged.

---

## 1. Overview and general principles

JustQuery is a native desktop application for Windows. Key architectural decisions:

- **Immediate-mode GUI.** The interface is repainted every frame from a single source of state,
  `JustQueryApp`. There is no widget tree with its own state — the state lives in the application
  model, and widgets merely draw it and return reactions.
- **A single self-contained binary.** Fonts and icons are embedded via `include_bytes!`; the
  runtime needs no external resources.
- **Custom virtualization.** The editor and the grid draw only the visible region and work in
  local f64 coordinates — rendering cost is O(visible) for files/result sets of any size (egui's
  stock widgets are not suited to this, see §6–§7).
- **Background threads + channels.** Any long-running work (a query, catalog collection,
  search, the update check) goes off to a separate thread, talks to the UI over an
  `mpsc` channel, and is cancelled via `AtomicBool` / `CancelRequest`. The UI frame never
  blocks (see §14).
- **Single sources of truth.** Colours/metrics live in `theme.rs`; the work area's left inset is
  `JustQueryApp::dock_left()`; the screen gutter is `CHROME_GUTTER`. There is no hardcoding at
  call sites.

---

## 2. Module map

Frame and screen:

| Module | Responsibility |
|--------|-----------------|
| `main.rs` | The `JustQueryApp` state, screen-level layout, the `update` loop, the toolbar, dispatch by tab kind |
| `menubar.rs` | The caption bar: logo, text menus (File/Edit/Search/Database/Tools/Window/Help), the active tab's title, window buttons |
| `winchrome.rs` | Custom window chrome: drag-to-move, border, resize grips, caption buttons (OS decorations are disabled) |
| `startup.rs` | Launching the window with no visible "unfold" (hidden window + warm-up), OS corner rounding (DWM), the themed bitmap I-beam cursor (egui `set_cursor_image`) |
| `theme.rs` | Palette (`Palette` light/dark, runtime `p()`/`apply()`), metrics, fonts, egui style (the scrollbar default is an inert fallback — every app scroll area styles itself via `widgets::style_scrollbar` → a disappearing floating overlay) |
| `widgets.rs` | Reusable painted helpers: islands (`island`/`island_panel`), crisp 1-device-px lines (`hairline`, `crisp_border`, `snap_rect`), buttons, `show_modal`, `form_row`, `manager_row`, `tab_strip`, scrollbar styles (`style_scrollbar` = a disappearing floating overlay with the grid/editor idle-timer `Fade`, used by every egui scroll area; `style_scrollbar_overlay` = same + the edge-fade gradient for manager lists; the editor & grid use the custom `vscroll`) |
| `brand.rs` | The `logo` logotype (J polyline + Q ring) and brand strings |
| `icons.rs` | The icon glyph set (Ionicons → `assets/justquery-icons.ttf`, fixed codepoints U+E900..) |
| `dialog.rs` | Win32 FFI: system Open/Save dialogs, clipboard, local time |
| `kinetic.rs` | Kinetic (momentum) trackpad scrolling |
| `vscroll.rs` | Custom f64 disappearing-overlay scrollbars for virtual scrolling (grid + editor), with the per-tab `Fade` state |

Document and editor:

| Module | Responsibility |
|--------|-----------------|
| `doc/mod.rs` | The `Document` facade: buffer, line index, undo, snapshots; `Pos=(line,col)` 0-based |
| `doc/piece_table.rs` | Piece table + mmap of the source buffer; the `PieceSnapshot` snapshot for background passes |
| `doc/line_index.rs` | An index of line starts (navigation/range reads without a full load) |
| `doc/encodings.rs` | Encoding and `Eol` detection, transcoding |
| `codeeditor.rs` | The virtualized editor: visible lines, caret/selection/undo, galley cache, the `Highlighter` contract |
| `highlight.rs` | SQL highlighting (a per-line lexer, invoked by the editor on visible lines) |
| `complete.rs` | F6 autocomplete (schemas/tables/columns by FROM alias) + Smart Enter/Tab |
| `find.rs` | The find bar (Ctrl+F) — methods on `JustQueryApp` |
| `search.rs` | The background search engine over a document snapshot |
| `fileops.rs` | Open/Save/Save As — methods on `JustQueryApp` |
| `proc.rs` | The frame for a tab's background processes (search): messages, cancellation, the results cap |

Grid and results:

| Module | Responsibility |
|--------|-----------------|
| `grid.rs` | The virtualized result grid (fixed-height rows): `GridModel` + `result_grid`, pinned `#`, sticky header, cell + whole-row selection (Ctrl/Alt) and TSV copy, sort-marker header (the sort state itself lives on `ResultSet` in `main.rs`), column resize/reorder |
| `sample.rs` | Demo grid data (test builds only) |

Connections and catalog:

| Module | Responsibility |
|--------|-----------------|
| `connections.rs` | Saved connections (DPAPI files), live connect, `run_statements_worker` (buffered + lazy COPY-streamed fetch; batch aborts at the first error/cancel), query cancellation |
| `sqlentity.rs` | `key_entity(sql)` — the query's key entity (table after `FROM` / DML-DDL target / verb) used as the result-tab label; tokenizer skips comments/strings/dollar-quotes (heuristic, first cut) |
| `connections_ui.rs` | The Connection Manager (dock) + the connection-settings tab |
| `crypt.rs` | DPAPI password encryption (crypt32 FFI), `crypt::to_hex` |
| `catalog.rs` | System-catalog introspection: schema/object lists, change fingerprints, budget, column fetch |

Metadata:

| Module | Responsibility |
|--------|-----------------|
| `metadata.rs` | The Metadata Manager: the shared `SharedStore`, the tree, the object tab |
| `meta_collector.rs` | The background SCANER thread: incremental fingerprint-diff into `SharedStore` |
| `meta_details.rs` | On-demand fetch of an object's columns (its own connection) |
| `meta_manager_modal.rs` | The Scan tab (collector settings + log); the status-bar `scan` chip's colour helper |

Updates:

| Module | Responsibility |
|--------|-----------------|
| `update.rs` | An HTTP update check on GitHub + self-update |
| `about.rs` | The About/Updates tab and the UI state of the update process |

Other: `tests.rs` — regression tests (logic + headless render); `build.rs` — embedding the
application icon (`winresource`).

---

## 3. Application lifecycle and the frame

- **State.** All mutable state lives on the `JustQueryApp` struct (`main.rs`): tabs, the active
  connection, the collector state, the update state, the find bar, kinetics, and so on.
- **The frame (`update`).** Each frame: input handling → screen-level layout (caption → toolbar →
  tab strip → work area → result panel → status bar) → draining the background-task channels →
  requesting a repaint when needed.
- **Input and kinetics.** `raw_input_hook`/`filter_input` intercept the trackpad wheel: the
  finger phase runs 1:1, the delayed inertia "lump" from Windows is discarded, after which the
  custom velocity engine takes over (`kinetic.rs`).
- **Frame robustness.** A panic during a frame is caught and surfaced in the **error modal** rather
  than crashing the application (deduped on the message — `last_error` doubles as the "already
  surfaced" latch — so a panic recurring every frame can't re-trap the modal). See `REQUIREMENTS` NFR-REL-2.
- **Start.** The window is created hidden and at once sized to the work area; show+maximize are
  sent after a short warm-up (stabilizing ppp and the font atlas) — with no visible "unfold" from
  a small window (`startup.rs`).

---

## 4. The tab model

The tab kind is a single flat enum:

```
enum TabKind { Sql, Connection(_), Meta(_), About, Scan }
```

- `Connection` / `Meta` carry a payload; the accessors `Tab::conn()/conn_mut()/
  meta()/meta_mut()` give type-safe access.
- The content dispatcher is `editor()`; the kind predicates are `is_sql_tab` /
  `is_connection_tab` / `is_meta_tab` / `is_about_tab` / `is_scan_tab` (+ `is_editor_tab`).
- The **active connection's settings tab** doubles as the live control-connection view: when the
  open `Connection` tab is the connected one (its `id` == `active_conn_id`) its fields lock, a green
  `● active` / red `● disconnected` marker sits by the title, and a **Session block** (server / db /
  user / pid / since / ssl) renders below the form. The status-bar identity chip (`login@<connection
  name>`) opens it. The `Scan` tab is the metadata collector's settings + log, opened from the
  `scan` chip.
- Service tabs (`About`, `Scan`) are singletons: reopening switches to the existing one.
- **Tab-strip auto-scroll.** The active pill scrolls into view when the active tab changes for a
  non-click reason (open / new / from a manager / Ctrl+Tab) or when the strip viewport width changes
  (a dock opens/closes/resizes, or the window resizes) — opening a dock slides the leading tabs under
  it instead of pushing the active tab off-screen. Overflow arrows (`‹ ›`) are judged against the
  full strip width, so they don't stay stuck after a dock closes when the tabs would now fit.

**The action toolbar is static.** The `editor_action_group` group always draws the same set
`Refact · Inspect · Execute · Stop` straight into the main `icon_toolbar` (there is no
separate band under the tabs). Only a button's liveness (active/dimmed) and its meaning depend on
the tab kind, the toolbar never "jumps":

- **Refact** — a stub (future automatic SQL refactor, F9) → dimmed, tooltip;
- **Inspect** = Connection → Test connection (F10); dimmed elsewhere (SQL Inspect is a stub);
- **Execute** = SQL (F8; green when armed) / Scan = Enable the collector (when stopped);
- **Stop** (F7) — red while anything runs on the tab / Scan = Disable the collector (when running).

Each tab also feeds the shared **Save** / **Save As** slots by kind: Connection → save / export the
connection, Scan → **Apply** the staged scan settings (Save).
Connection/About/Scan/Meta tabs otherwise add nothing to the toolbar; the dock managers
keep their own subbars (New / Import / Delete, …).

---

## 5. The document model (`doc/`)

- **Representation.** A piece table over a memory-mapped source buffer + a line index. The text is
  **not** materialized as one string: reads are line-by-line/range-only. Large files open in the
  background and are not loaded in full.
- **Coordinates.** `Pos = (line, col)`, 0-based, `col` in code points; internal storage is UTF-8
  bytes.
- **Encoding/EOL.** Detected on open (`encodings.rs`), saving is kept consistent.
- **Undo.** History with a **bounded depth** (protection against unbounded memory growth).
  **"Modified" is a save-point, not a bool:** the document reads as modified iff its current undo
  position `(undo_epoch, undo.len())` differs from the saved point (`saved_point`, set on save/open).
  `undo_epoch` bumps when a fresh edit discards a redo branch; an evicted save-point counts as
  modified. So *new file → type → Save → Ctrl+Z* correctly shows the unsaved star — the undo lands
  past the save-point, where the buffer no longer matches disk.
- **Line index.** Chunked byte offsets of line starts (`line_index.rs`): an edit rebuilds only the
  chunks it overlaps; the chunk prefix sums are recomputed lazily. The index stores byte geometry
  only — character positions are derived per line on demand (and cached with the line text).
- **Snapshots.** `PieceSnapshot` — an immutable snapshot for background passes (search):
  the worker reads a consistent copy while the user keeps editing.
- **Load state.** A tab's document is modelled as `TabDoc` (ready / loading) — the UI gates
  actions until it is ready.

---

## 6. The code editor (`codeeditor.rs`)

- **Virtualization.** Only the visible lines are laid out and drawn → O(visible) on files of any
  size. A per-line galley cache; custom scrolling on both axes (f64, see §7).
- **Caret.** Single-line; its x is taken from the line's galley (not "column × char width"), so the
  caret sits on the real text even on tabs/wide glyphs. While typing it keeps a few characters of
  "air" from the right edge.
- **Language neutrality — the `Highlighter` contract.** The editor knows nothing about the
  language being edited. Highlighting arrives via the `Highlighter { line, advance }` callback:
  - `line` — colouring of a single visible line;
  - `advance` — advancing the lexical state through a line without colouring it (for lines above
    the visible window).
- **The lexer invariant (critical).** The per-line lexer **ALWAYS advances `i`**, and the full
  pass and `*_state_only` must **converge on the line's final state**. A violation → a divergence
  of state between the visible window and the tail → a **frame hang**. Any highlighting change must
  hold this invariant (see `REQUIREMENTS` NFR-PERF-5).
- **Editing.** Smart Enter (keeps the indentation), Smart Tab (aligns to the "hook" on the line
  above), Unicode-aware word navigation/selection/deletion (Ctrl+←/→, Ctrl+Backspace/Delete — all
  share the `word_boundary` rule), edit commands via the active editor. The editor
  is **never blocked** — typing stays live during a query, a lazy result stream, or a background
  search (the SQL text is not used after launch; the search reads a snapshot).

---

## 7. The grid and scrollbars (`grid.rs`, `vscroll.rs`)

- **Why custom scrollbars.** egui's coordinates are f32; on a virtual canvas hundreds of millions
  of pixels wide the representation step reaches 32px (lines overlap, clicks miss). So the editor
  and the grid scroll themselves: the position is **f64 pixels from the start of content**, only
  the visible window is drawn, and large coordinates never exist. `vscroll.rs` draws and handles
  the bars themselves.
- **One scrollbar style (egui areas).** Every egui scroll area — the form sheets, scan log, multiline
  fields, manager lists (Connection / Metadata), the combo/autocomplete popups — uses
  `widgets::style_scrollbar`: a **disappearing floating overlay** that reserves **no** width (rows stay
  edge-to-edge and a bar appearing never reflows them) and **fades on the same idle timer as the
  grid/editor** bars, not on mere hover. It folds a per-area `vscroll::Fade` (kept in `ctx.data`, keyed
  by the ui id — safe since each area is a singleton) into egui's handle opacity each frame, so the bar
  eases out even while the pointer rests inside. `style_scrollbar_overlay` is the same + egui's content
  edge-fade gradient (which an overlay bar lets span the full width). Riding egui's default scrolling.
- **The grid's and editor's own bars (`vscroll`).** **Disappearing overlays** that reserve **no** space:
  the content fills the whole island and the frozen chrome — the grid's `#` gutter + header, the editor's
  line-number gutter — fills it edge-to-edge to the rounded corners, with the handles floating
  semi-transparent **on** the content. A single-pass `need_v`/`need_h` says which axis shows one. Each
  handle **fades** — shown on activity (scroll, any pointer movement inside the area, or a drag), easing
  out after a short idle. The fade is `vscroll::Fade` (a small `Copy` struct: a `last_active` stamp + an
  eased `vis`); it lives **in the caller's per-tab state** next to the scroll offset (`ResultSet` /
  `proc::Results` / `EditorState`), so tabs fade independently and the UI idles when hidden. Tracks are
  **confined to the data region** (below the header, right of the gutter) so a handle never rides onto the
  chrome. **Only when both axes scroll** is each viewport shortened by one `BAR` (`vview`/`hview`): that
  extends the scroll range so the last row / column / line slides **clear of the perpendicular handle** at
  the very end (a clearance strip only there, never a permanent gutter) and keeps the two tracks one bar
  short of the shared corner. `vscroll::bar` takes an explicit `view` (the scrolled viewport = the confined
  track length) and an `alpha` (the fade opacity from `Fade`). The editor's horizontal extent is the
  longest line in **characters** (`Document::max_line_chars` — chars, not bytes, which over-count multibyte
  UTF-8), kept exact for docs up to a few MB (recomputed on shrinking edits) and grow-only above.
- **The grid.** `ResultSet` + an O(visible) data grid: a pinned `#` column (width tracks the largest
  row number, like the editor gutter; its tone fills to the content edge, row numbers clipped to the
  data area) and a sticky header (they stay put while the data scrolls), cell selection
  (click/rectangle) and copy as TSV (Ctrl+C), mouse column reorder and resize. A **1px `border`
  grid** (hairline, pixel-snapped) rules the body — a separator under each row and at each column's
  right edge — drawn in one pass after the zebra/cells/gutter (so the zebra can't shave it) and
  bounded by the **end of the table**, not the panel edge. Selections are a flat `editor_sel` fill,
  no outline.
- **Whole-row selection.** Clicking the `#` gutter selects a row (Ctrl toggles several, Alt extends a
  range from the anchor); held in `JustQueryApp.grid_rows` (visible-row indices), mutually exclusive
  with the cell selection, and Ctrl+C copies the full rows.
- **Client-side sort.** Clicking a header cycles asc → desc → cleared (Ctrl makes it multi-column).
  It does **not** re-query: the sort keeps a permutation `ResultSet.view` (visible → data) instead of
  reordering `rows`, the comparator is numeric-aware with NULLs last, and it covers only the rows
  fetched so far. Incremental fetch (`LazyRows`) drops the sort marks and appends new rows at the end
  rather than re-sorting (it stops *guaranteeing* the order, never reshuffles what is on screen).

---

## 8. Connections and the catalog

- **Storage.** Each connection is a separate file, the file name = the connection name (names are
  unique). The password is encrypted with **DPAPI** (`crypt.rs`, crypt32 FFI; `to_hex` is a shared
  helper).
- **Live connection.** Connect opens a connection in the background behind a blocking "Connecting…"
  overlay. Everything the attempt targets — connection id, resolved credentials, metadata settings,
  the identity label — is **staged** (`pending_*`) and applied only on success
  (`finish_main_connect`), which is also when the previous session state (tab sessions, metadata
  actors) is torn down. A failed or cancelled attempt (`fail_main_connect`) drops the staged
  identity and changes nothing else, so any previous connection stays live and active; the failure
  surfaces inside the Connect modal. TLS is the Windows system stack (SChannel) via `native-tls`.
- **Control + sessions.** After Connect, `main_conn` is a *control* connection, while **each tab
  opens its own session** (lazily on its first run, kept thereafter): this preserves
  `SET`/temp tables/prepared statements between queries and lets tabs run concurrently.
- **Execution / cancellation.** A Run goes to `run_statements_worker` on a background thread that
  owns the tab's session connection for the run; results stream (see §11), and the last row-returning
  statement is then served lazily. Stop sends a real PostgreSQL `CancelRequest`, **preserving the
  session connection**.
- **`catalog.rs`.** Catalog probes take a live `postgres::Client` (reusing the persistent
  connections of background actors): schema/object lists, a **per-schema fingerprint** (folding in
  the `xmin` of relations/attributes/defaults/functions → catches `ALTER/ADD COLUMN`), a budget
  counter, an object's column fetch. Catalog identifiers are escaped, not interpolated raw.

---

## 9. Metadata (`metadata.rs`, `meta_collector.rs`, `meta_details.rs`)

- **The source of truth is the system catalog.** No server-side objects (event triggers,
  extensions). `SharedStore` is the shared in-memory object model; the UI takes it under a lock for
  a tree snapshot.
- **Incremental collection (an actor).** A background thread reads the per-schema fingerprint and
  re-reads **only the changed schemas** (full objects + columns) into `SharedStore`. Collection
  runs **on an interval** (30s by default, the "smoke break") while the user is active and **sleeps
  after 5 min idle** (no DB load). The budget is ~1,000,000 objects+attributes; exceeding it — or a
  failed scan (connect/catalog error) — **stops** the collector with the error until it is
  re-enabled (toolbar Execute); the reason is the last activity-log line.
- **On-demand fetch.** `meta_details.rs` pulls an object's columns on its own connection when a
  metadata tab opens.
- **UI.** A dock tree (type folders → objects), a collector indicator in the status bar
  (green active/asleep, red stopped; hidden while disconnected), a collection-management tab
  (settings + a two-pane schema picker + a log). The settings (interval/sleep/budget/monitored
  schemas) persist to the connection file.

---

## 10. A tab's background processes (`proc.rs`)

- **Model.** A tab has at most **one** process (`Tab.proc`); tabs are independent. The only
  process today is the background search. The worker lives in its own thread, sends `ProcMsg`
  over a channel (polled in `update`), and stops via `AtomicBool`.
- **Gating.** `tab_busy()` (a process running, or a query/fetch actively churning) blocks
  **launching** another process and SQL Execute on that tab — but NOT typing (the editor stays
  editable; a parked lazy result stream is not "busy"). Polling/completion is `poll_procs` /
  `finish_proc`.
- **Cap.** A process's accumulated results are capped at **100 MB** (`RESULTS_CAP_BYTES`); on
  exceeding it the process stops, what was accumulated is shown, and an error goes to the status
  bar.

---

## 11. The result panel

- **One bottom panel** per tab: `Tab.panel: Vec<ResultTab>` (`Data(ResultSet)` | `Probe`), the
  active one is `panel_active`.
- **Filling.** SQL Run **clears and fills** the panel with sheets (Messages + one sheet per
  row-returning statement, streamed as they become ready). Find **adds** a sheet.
- **Lazy fetch (доскролл).** Every row-returning statement is streamed via `COPY (<sql>) TO STDOUT`
  on the tab's session connection — values come back as text (like the buffered path) and
  **parallelism is preserved** (COPY plans with `CURSOR_OPT_PARALLEL_OK`, unlike a server cursor).
  Uniform first step (`buffer_rows`): **every** statement reads its **first page — the panel's
  measured row capacity (`Tab.last_visible_rows`, `DEFAULT_RESULT_ROWS` before the first render) —
  into memory BEFORE its grid appears**; the tab keeps its **running spinner** during the read, so a
  slow statement reads honestly as "still running" instead of flashing a partial page.
  - A **non-last** statement (`copy_head`) can't hold the connection open, so its COPY subselect is
  **capped server-side**: `SELECT * FROM (<stmt>) __jq_head LIMIT first_page+1` (`head_capped_sql`;
  the +1 probes "there is more" so an exactly-one-page result isn't falsely marked partial). The
  server stops producing right after the preview — the stream ends BY ITSELF, freeing the connection
  with **no cancels, no tail transfer, no races**. This is the ONE place the app adapts the user's
  SQL, and only inside that COPY — the sheet's `sql` (Refresh) keeps the original text; if the
  capped form fails to prepare (an exotic-but-valid shape) the original runs uncapped and the tail
  is drained (rare, slow-but-correct). A capped grid is flagged `truncated` + `stale`.
  - The **last** statement (`lazy_copy_stream`) shows its first page and stays **live**: a background
  worker serves `FetchCmd` {More, All, Rest, Close} — **Fetch next page** pulls the page size
  **frozen at run time** (`Tab.fetch_page`, not the live capacity), **Fetch to end** pulls up to
  **+100 MB of raw source bytes** then pauses **with a warning** (PL/SQL-Developer style: everything
  fetched is kept in memory, going further may exhaust it — **Continue** sends `FetchCmd::Rest` =
  fetch to the very end with no budget, **Cancel** leaves the stream parked with the buttons armed),
  and **Stop** *pauses* any fetch (the stream stays open; a later fetch resumes it).
  - **Closing an abandoned parked stream (`end_copy_early`).** The wire protocol has no client-side
  "stop a COPY-out" verb (only: drain to EOF / an out-of-band `CancelRequest` / dropping the
  connection), and a stray CancelRequest is UNORDERED — fired blindly it can land on the session's
  NEXT statement (the old `pg_class; pg_sleep` bug). So the WORKER closes the stream: fire ONE
  cancel and KEEP READING until the stream ends — **57014 proves the cancel was consumed by this
  very stream** (clean); a **natural EOF** means the signal may still be in flight → a **1 s grace**
  before the connection is reused, so it dies on the idle backend; any other read error → the
  session is dead (`Handled::Fatal` → the client is dropped, the tab's next run reconnects). A LOST
  cancel just means the drain transfers the tail — slower, still correct. Close points: a **new run /
  Refresh on the editor tab** (ANY of its result sheets — the tab has one session connection), the
  **editor tab closing**, the **results panel ×**, the 5-minute **idle timeout** (silent), and
  **disconnect**; `close_lazy_stream` also trips the lightning flag first, so an in-flight fetch
  pauses promptly instead of finishing a 100 MB pull before the worker sees `Close`.
  - Whenever a live stream is lost **before EOF** — the idle timeout, a Refresh / new run reclaiming
  the connection, a disconnect, or a dead worker (all funneled through `disarm_lazy_grids`), or a
  stream error at `LazyEnd` — the grid is marked `ResultSet.stale`. `stale` fetch buttons stay **live
  permanently** — together with the tab's `…` they are the standing indicator of a partial grid; every
  click shows the "re-run to load the full result" modal. A clean EOF leaves them **inert** (everything
  was fetched — the `stale`-vs-inert split keys off whether the grid was still `lazy` at teardown).
  DML/DDL and data-modifying CTEs stay on the buffered path.
  - **Stop while executing.** The first click fires a `CancelRequest` and flips the activity pill to
  **"Cancelling…"**; for 5 s the cancel is auto-re-fired once a second (nervous re-clicks add shots).
  Past 5 s the next Stop click opens the **Force stop** confirm — `pg_terminate_backend` (session
  state is lost; the modal says so); if even that doesn't settle the worker in 3 s the session is
  **abandoned** (tab freed, zombie worker parks on its dead socket) with an explanatory modal. The
  tab's next run auto-reconnects.
  - **Refresh** of a data sheet blanks it immediately into the running look (rows cleared → the
  activity pill + a ticking timer), instead of freezing the stale grid until the new data lands; the
  refreshed sheet's cumulative load time is rebased (`rebase_refresh_elapsed`) so the timer shows the
  refresh run's real duration.
  - **Grid geometry.** The default panel snaps to `grid::panel_height_for(DEFAULT_RESULT_ROWS)` =
  header + **10 clean rows** (`grid::rows_fit` is the inverse and single source of truth; the measured
  capacity is **per-tab**). The horizontal bar's **groove is ALWAYS the bottom strip of the field**:
  the bar floats OVER the bottom data row while rows continue below the fold. What keeps the groove
  clear of data once the end of the rows is on screen is a one-`BAR` reserve BELOW the last row — the
  in-content **home strip** (appended after the last row, only when the grid also overflows
  horizontally) when there is vertical overflow, or simply the spare field space when everything fits.
  So with fewer rows than the field the bar rests at the **field bottom** with empty space above it (it
  never sticks up under the last row); with exactly a viewport-full of rows the strip adds a small
  BAR-high scroll range that frees the bottom row from under the bar.
- **Background-process status** (Find) shows in the **status bar**
  (`Tab.proc_status`, bound to the editor tab); SQL run state and the (now removed) stream-idle notice
  are **not** pushed there — run state lives on the tabs, and a lost stream drives `stale` instead.
- **Run-state model (tabs).** Every tab strip pill carries a leading `widgets::TabMark`
  (`{spinning, glyph, tint: Option<Color32>}`): a small hand-painted `widgets::spinner` (egui's
  `Painter` can't rotate a glyph) while a query runs on the tab, else the glyph. Glyph **and** label
  share one colour: `tint` (when `Some`) tints the **whole pill** to carry a result's state, **derived
  live** (no stored outcome) via `ResultSet::status_color` (err → danger, **else `None`** — neutral, no
  green); with `tint: None` glyph + label are normal `text` on the **active** tab and `text_dim`
  otherwise — **never the accent colour** (the active pill's `accent_soft` background and the brighter
  text mark it), so an active tab and its close `×` never read as a red error tab. Editor tabs are
  always `tint: None`. Each result tab is **named after the query's key entity**
  (`sqlentity::key_entity` — the table after the last `FROM`, the DML/DDL target, or the bare verb); the
  name (`Tab.run_labels`, computed at spawn) shows **immediately**, even on the spinner placeholder, and
  carries its **row count** in `[…]` (`[1500]` / `[1500…]`). The **run timer** — the **active result
  tab's own** time in seconds, microsecond precision (`[3.123456 sec]`), live — is in the **status
  bar**, not the tabs: `App::run_timer_text` reads the active `ResultSet` = its execution
  (`load_elapsed`, kept cumulative, **minus the previous Data sheet's** so it isn't a sum across
  statements) plus the doscroll fetches accrued on it (`ResultSet.fetch_elapsed` + the live
  `Tab.fetch_start`). Each tab keeps its own time; doscrolling one grows only its value.
  `Tab.run_timing` gates the editor + result spinners to the **initial** run (a доскролл re-sets
  `running` but not `run_timing`, so the result tab animates its growing row count instead of spinning;
  a Refresh sets the sheet's `fetching` so its spinner restarts).
  `App::result_strip` emits **one spinner placeholder per not-yet-produced statement**
  (`data_sheets < Tab.run_stmt_count`) so each tab of a slow multi-statement run appears in turn. A
  user **Stop** during execution (`run_timing`) cancels the query → a **red `Query cancelled`** error;
  `begin_copy` returns `CopyStart::Failed` on a cancelled/failed COPY and the worker **shows the error
  instead of re-running** the statement on the buffered path (which would restart a slow query). During
  a doscroll fetch the same button **pauses** instead, keeping the partial result green. The worker's
  statement loop returns `Handled{Yes,Error,Fallback}` per statement and **breaks on `Error`**, so a
  **failed or cancelled statement stops the rest of the batch** (later statements don't run). The
  `stop_requested` flag is set only by a Stop **while the query is executing** and cleared at the
  execution→fetch transition (`LazyMore` / `send_fetch`), so an error arriving with it set **is** the
  cancel — the read-error text (from `pump`) is unreliable, so it isn't consulted. The worker keeps a
  statement's own label even on error (the red pill conveys it). While a sheet is still loading
  its first rows — **or** the active result tab is a pending statement's placeholder
  (`panel_active >= panel.len()`, no sheet yet) — `result_body` paints the centered "Running" pill
  (`widgets::running_overlay`) instead of clamping to the previous statement's grid.
- **Resizing** the panel — grab across the full width of the grab strip above the panel; each
  result's height/expanded state/scroll **and its measured on-screen row count** are kept per-tab.

---

## 12. Search (`search.rs`, `find.rs`)

A single background engine. Ctrl+F → bar (`find.rs`) → Enter → a pass over the
`PieceSnapshot` (the UI never blocks, edits don't interfere): lines are cut on `\n`, comparison is
per-character with a simple case-fold (exact match columns), and very long lines are processed in
segments. Matches stream into the result grid in batches; clicking a row jumps to it. The volume is
capped.

---

## 13. Updates (`update.rs`, `about.rs`)

`update.rs` is the application's only reach into the external network (besides the DB server): an
HTTP client (`ureq`, native-tls — reusing the tree's TLS stack) checks the latest release on
GitHub and performs the self-update. The flow is **download-silent, install-explicit**: a check
that finds a newer build **auto-downloads** it in the background (no user action), staging the new
exe; the **install** is a separate, user-triggered step (a click on the About page, since applying
may need elevation). `Retry` distinguishes a check/download failure (retry re-checks) from an
install failure (retry re-applies the staged exe). `about.rs` is the UI state of the process (the
version chip; the status line is clickable *content*, never a button — install / retry); on the
latest build the page stays quiet (no "you're current" line, just the green version label).

---

## 14. Cross-cutting patterns and contracts

- **Extension via `impl` modules.** `find.rs`/`fileops.rs`/`about.rs`/`menubar.rs` add methods to
  `JustQueryApp` through `impl` blocks in child modules — that way they can see the struct's
  private state (a private item is visible to the owning module and its descendants), while
  `main.rs` does not grow.
- **The "background task" contract.** A worker receives a **snapshot** of the input data
  (`PieceSnapshot` / a live `Client`), sends progress/result over an `mpsc` channel, and is
  cancelled via `AtomicBool` (or `CancelRequest` for a query). The UI **drains the channel every
  frame** and calls `request_repaint()` on a state change. The invariant: the UI frame never waits
  on the network/disk.
- **The `Highlighter` contract** and **the lexer invariant** — see §6 (violating the invariant = a
  hang).
- **O(visible) virtualization** in f64 coordinates — the editor and the grid (§6–§7); do not touch
  without need.
- **`SharedStore` under a lock** — background collection writes, the UI takes a tree snapshot (§9).
- **Single sources of truth.** `theme::p()` (colours), `SPACE_*`/`RADIUS_*`/`CONTROL_H` (metrics),
  `dock_left()` (the work area's left inset), `CHROME_GUTTER` (the screen gutter). No hex/gaps at
  call sites — see `DESIGN_SYSTEM.md`.
- **Dispatch by `TabKind`** + the static action toolbar (§4): behaviour branches on the kind
  predicates, but the screen layout never "jumps" by kind.
- **Correct tab closing.** Closing cancels the associated background work (threads, mmap, temp
  files) — with no zombie threads or leaks (see `REQUIREMENTS` NFR-REL-3).

---

## 15. Building

- The **MSVC** toolchain (`stable-x86_64-pc-windows-msvc`) + VS 2022 Build Tools (the linker).
- `cargo build --release` → `target\release\justquery.exe`; `cargo test --release` — logic +
  headless render.
- A single self-contained exe: the fonts (JetBrains Mono regular/bold) and the icons are embedded
  via `include_bytes!`; `build.rs` embeds the application icon (`winresource`).
- Dependencies are minimal, with no duplicate crate versions in the tree — see `REQUIREMENTS`
  NFR-TECH-9.

---

## Appendix. Key external dependencies

| Crate | Role |
|-------|------|
| `eframe`/`egui` (0.35, wgpu) | The window and the immediate-mode GUI; the wgpu backend (DX12/Vulkan, WARP fallback) |
| `postgres` / `postgres-native-tls` / `native-tls` | The PostgreSQL client + TLS via SChannel |
| `memmap2` / `encoding_rs` / `memchr` | The document model: mmap, encodings, fast byte search |
| `ureq` | The HTTP client for the update check/download |
| `winresource` (build) | The application icon in the exe |
