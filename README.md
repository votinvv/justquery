# JustQuery

A native desktop **PostgreSQL IDE** for Windows, written in Rust with [egui](https://github.com/emilk/egui).
Inspired by PL/SQL Developer — the goal is the same focused, fast, rock-solid workflow,
but for Postgres, which has no tool of that quality.

> ⚠️ **Pre-release — for developers only, NOT for production.** JustQuery is early, rough,
> rapidly-changing software aimed at developers experimenting locally. It is **not intended for
> use in production environments**. Back up anything you care about and **use it entirely at your
> own risk** — see the [Disclaimer of warranty](#disclaimer-of-warranty) below.

> **Status:** working PostgreSQL front-end. A **live connection** (TLS via Windows SChannel) opens
> on a background thread; **queries run for real** (F8 / Execute) — each tab on its own session
> connection, on a worker thread, so the UI stays responsive and tabs run concurrently — streaming
> result tabs in as each statement finishes, with incremental fetch and working
> **query cancellation** (Stop). Still to come: explicit transactions (commit / rollback).

## Install

Windows — download `justquery.exe` from the
[latest release](https://github.com/votinvv/justquery/releases/latest) and run it (the exe is
the whole app; no installer). To update, download the new exe and replace the old file.

If the app ever fails to launch, the reason is shown in a message box and appended to
`%APPDATA%\JustQuery\startup-error.log`.

## System requirements

- **OS:** Windows 10 or 11, **64-bit**. (Windows 8.1 and older are not supported.)
- **CPU:** any x64 processor; 2+ cores recommended — queries, the metadata scanner and the
  update check all run on background threads.
- **RAM:** the app idles at roughly **300 MB**; 4 GB of system memory is a comfortable minimum.
  Query results are held in memory, so very large result sets add to that.
- **GPU: none required.** JustQuery renders through [wgpu](https://github.com/gfx-rs/wgpu)
  (DirectX 12 / Vulkan) and falls back to WARP — the software rasterizer built into every
  Windows 10+ install — so it also runs on virtual machines, over remote desktop, and on systems
  without a dedicated GPU driver. Any GPU with a working driver simply makes rendering free.
- **Disk:** ~50 MB.
- **Display:** anything 1024×600 or larger (the window's minimum size is 1024×600).
- **Network:** reachability of your PostgreSQL server — nothing else is ever contacted (there is
  no telemetry and no in-app update check).
- **Server:** PostgreSQL — any version speaking protocol 3.0 (PostgreSQL 7.4+; tested against
  current major versions).

## Design goals

- **Ruthless minimalism.** The center of gravity is one loop: *type a query → get a result.*
- **Speed and stability.** Native desktop, no web runtime; query cancellation must be bulletproof.
- **Windows-only, Postgres-only, single-developer** — scope kept deliberately narrow.

The default visual theme is a cool, **silvery light** scheme: near-white panels frame the white editor,
Connection Manager and result *islands*, plus a neutral dark accent, sharp-cornered controls, and a
soft highlight for hovered/active tabs, popups and buttons. **One canonical border everywhere** —
islands, sheets, modals, fields, dropdowns, menus and the window outline all use a single thin
`border_strong` frame drawn as exactly **one physical pixel** ([`crisp_border`](src/widgets.rs),
pixel-snapped so it stays razor-sharp at any DPI scale). Components are otherwise **flat**; the only
depth is a single soft `island_shadow` under raised surfaces (islands, popups, modals). The chrome
rows (caption + toolbar, tabs) share one height and one element padding, so the
blank gaps between them come out equal. UI text uses the native **Segoe UI** for crispness; the
editor is **JetBrains Mono**, with SQL **keywords in bold** and everything else regular weight. The
UI is **English-only**. All colours live in one
[`Palette`](src/theme.rs); JustQuery ships **Light** and **Dark** palettes, switchable at runtime with
**Ctrl+Shift+D** and remembered across launches (`%APPDATA%\JustQuery\settings.json`).

## Features in the shell

- Custom window chrome (the OS title bar is disabled): caption buttons, drag-to-move,
  double-click-to-maximize, edge/corner resize handles, and a 1px window border — all hand-drawn.
- Multiple editor tabs (named *Untitled*), each preserving its own SQL text, caret and scroll;
  tab strips scroll with ‹ › buttons when they overflow.
- **Virtualized code editor** (custom, not egui's `TextEdit`): only the visible lines are laid out
  and painted, so editing/scrolling stay O(visible) on files of any size. Line-number gutter, a
  subtle active-line band, a one-row caret, mouse click/drag selection, and **custom kinetic
  scrolling on both axes** (velocity-based momentum on a trackpad flick — Windows delivers flick
  inertia as one delayed lump, which we ignore in favour of our own). The caret keeps a few
  characters of breathing room from the right edge as you go. Every column ↔ x mapping — the caret,
  mouse clicks, the selection and Find highlights — goes through the line's galley (not
  `column × char-width`), so it stays on the real text even where a tab or an odd-width glyph makes a
  character render wider than one cell.
- SQL syntax highlighting (keywords, strings, comments, numbers, function calls); non-highlighted
  text is regular-weight, highlighted tokens are bold.
- **Smart Enter** (keeps the line's indentation) and **Smart Tab** (pads to the next "hook" on the
  line above, PL/SQL-Developer style).
- **F6 autocomplete**: schemas on an empty context, a schema's relations after `schema.`, and a
  relation's columns after `alias.` (alias resolved from the statement's FROM/JOIN list). It reads
  the live metadata store the scanner keeps current, so a table created a minute ago completes.
- **File operations** via native Win32 dialogs (owned by the app window, so they stay on top):
  Open / Save / Save As, per-tab file path, dirty marker, "already open" de-duplication. A file is
  **saved in the encoding it was opened with** (UTF-8 with or without a BOM, UTF-16 LE/BE,
  Windows-1251, KOI8-R, …); a character the original encoding can't hold (an emoji in a cp1251
  file) saves the file as UTF-8 instead of mangling it, and the status bar shows the change.
- **Find** (Ctrl+F): a small floating bar (with a × close) that searches on Enter via a single
  background engine (Unicode-aware, case-insensitive) over a memory-mapped snapshot, so
  multi-gigabyte files stay responsive; matches stream into the result grid and clicking a result
  row jumps the editor to it. A 100 MB results cap protects against searching a frequent term in a
  huge dump.
- Unicode-aware word navigation in the editor (Ctrl+←/→ and Ctrl+Shift+←/→ work on Cyrillic etc.,
  not just ASCII — the editor implements its own Unicode-aware word boundaries, see `doc/mod.rs`);
  the same boundaries drive **word deletion** — Ctrl+Backspace (word before) and Ctrl+Delete (after).
- **One static toolbar** in the caption row (there are no text menus): file actions, the two dock
  toggles and the per-tab verbs — **Execute** (F8), **Execute in separate sessions** (Ctrl+F8),
  **Stop** (F7), Refact (F9, reserved) and **Inspect** (F10 — Test connection on a connection page).
  Every icon keeps its slot on every tab; only whether it is live or dimmed changes. Undo / Redo
  (Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y) act on the active tab. About and Scan open as singleton tabs.
  Modals are reserved for connecting and for action confirmations.
- A **Connection Manager** side panel (resizable, vertically scrolled): single click selects a
  connection (Ctrl/Shift for multi-select; selection commits on press so the accent moves instantly),
  the panel toolbar adds (**+**), connects (▶), disconnects and deletes the selection, and a
  double-click opens the connection's settings tab, where every field — including the **Name** — is
  editable (names may repeat: a connection's identity is its id). **Test Connection really
  connects** to Postgres over TLS and reports the server version + SSL state (or the error) in a
  modal. All connections live in one store, `%APPDATA%\JustQuery\connections.json`, written
  atomically (a crash mid-save keeps the previous file) with the passwords DPAPI-encrypted — bound to
  your Windows user. Save As on a connection page exports it (without the password) as a `.conn`
  file.
- A live **connection**: Execute on a connection page connects with the form's current values on a
  background thread behind a blocking "Connecting…" overlay; the status-bar identity chip turns
  green on success. A failed attempt changes nothing — any previous connection stays live and
  active. The **control connection** then lives on its own thread: it pings the server every 15 s
  (a lost server turns the chip red and the connection page shows why) and is the always-free kill
  switch for a query that ignores cancellation. Connect / Disconnect first guard against tabs with a
  running query or an open result stream (offering to go back or kill the work).
- **Query execution** (F8 / ▶): runs the selection if there is one, otherwise the whole tab, on a
  background worker thread. **Each tab opens its own session connection** — lazily on its first run
  and kept open afterwards (so SET / temp tables / prepared statements persist between queries) —
  which also lets tabs run concurrently. The script is split on top-level `;` (quotes, `E'…'`
  strings, `"…"` identifiers, `$tag$` bodies and nested comments are respected). The result panel
  opens on the first per-statement sheet — a **Result** grid for a row-returning statement, or a
  one-row status sheet (Status / Line / Message — click it to jump to the statement); each statement
  streams in as its own tab (you drive the tab selection), while a spinning indicator on the running
  tab (editor and result tabs) and a live `[N.NNNNNN sec]` timer (seconds, microsecond precision) in
  the status bar show progress. **Stop** cancels the active tab's running query with a real
  PostgreSQL CancelRequest (the session connection is preserved), escalating to a Force stop
  (`pg_terminate_backend`) when the query ignores it.
- **Multi-statement scripts preview, they don't stream:** in a session run of several statements,
  every row-returning statement shows its **first page** (the panel's row capacity), capped
  server-side with `LIMIT` so the session is free for the next statement. Note what that means: a
  `SELECT` with side effects in the middle of a script — `SELECT my_func(id) FROM t`,
  `SELECT pg_terminate_backend(pid) FROM pg_stat_activity` — runs only for the rows of that first
  page. Run such a statement on its own (or use **Execute in separate sessions**, where every
  statement keeps a live, fully fetchable stream).
- *(Planned)* a unified execution-log **Messages** tab: one row per statement (time, exec/fetch
  duration, row count, SQL).
- A virtualized result grid: a pinned **#** row-number column and sticky header (both stay put while
  the data scrolls), and custom scrollbars that run the full edge (over the header / **#** column)
  and take up space only while shown. Only the visible rows *and columns* are laid out, and a huge
  cell value shows its first 600 characters. **NULL** is a real state (shown as a dim `—`, distinct
  from the text `—` and from the empty string; copied as an empty field). Cells are selectable
  (click / drag a rectangle, or whole rows via the **#** gutter) and copyable as TSV with Ctrl+C;
  headers sort client-side (Ctrl for multi-column; numeric columns sort numerically, NULLs last).
- PL/SQL-Developer-style incremental fetch (per result set): Fetch next page / Fetch to end (pausing
  every +100 MB with a memory warning), with Stop as a pause.
- The Execute action is context-gated: it needs a live connection, non-empty text, and an idle tab.
  (Explicit transaction control is not implemented yet — every statement autocommits.)
- A **Metadata Manager** side panel + background **scanner**: the system catalog is the single
  source of truth, polled with no server-side objects (no event triggers / extensions). Each scan
  reads a one-query **per-schema fingerprint** (folding in the `xmin` of relations, attributes,
  defaults and functions, so `ALTER COLUMN`/`ADD COLUMN` register too) and re-pulls **only the
  schemas whose fingerprint moved** — full objects + columns — into a shared in-memory store. It
  scans once per **interval** (default 30s) while the user is active and **sleeps after 5 min idle**
  (no DB churn); a cheap count guards a **1,000,000 objects+attributes budget**, stopping with an
  error past it. The panel shows a schema dropdown + Refresh and a virtualized one-level tree: type
  folders with a disclosure chevron over per-type object rows (table / view / sequence / function
  icons) that select on click (Ctrl/Shift multi-select) and open a metadata tab on double-click. The
  status-bar **scan** chip is a coloured label, green (active) / yellow (asleep) / red (failed) /
  grey (off); clicking it opens the **Scan** tab — live status, the interval / sleep / budget
  settings, a two-pane *available ⇄ monitored* schema picker (Ctrl/Shift multi-select, double-click
  to move) and an activity log; Save applies. A metadata tab fetches an object's columns on demand.
  All settings (interval, budget, idle, monitored schemas) persist with the connection.
- Each editor tab keeps its **own** result-panel height, maximized state and per-result scroll.
- Operation errors (file I/O, connection failures, name validation) surface in **modals**, not the
  status bar — as does a caught frame **panic** (surfaced once in the error modal instead of
  crashing). The status bar carries only editor state (encoding / EOL / caret), the connection and
  scan chips, the run timer and Find status.

## Project layout

```
src/
  main.rs        Application state (JustQueryApp, Tab) + screen-level layout + update loop + toolbar
  titlebar.rs    The caption row: logo, the icon toolbar, the centered active-tab title, window buttons
  winchrome.rs   Custom window chrome: drag-to-move, border, resize grips, caption buttons
  startup.rs     Hidden-window warm-up launch, DWM corner rounding, the themed I-beam cursor
  theme.rs       Palette (all colours), style metrics, fonts, egui style — the look in one place
  widgets.rs     Reusable painted UI helpers (islands, buttons, `show_modal`, tab strips, scrollbars)
  brand.rs       The logotype (J polyline + Q ring) and brand strings
  icons.rs       The icon glyph set (Ionicons → assets/justquery-icons.ttf)
  dialog.rs      Native Win32 helpers: Open/Save dialogs, message box, local time (FFI)
  kinetic.rs     Kinetic (momentum) trackpad scrolling
  vscroll.rs     Custom f64 scrollbars for the virtual editor/grid
  codeeditor.rs  Virtualized SQL editor: renders only visible lines (O(visible) for any file
                 size); owns caret/selection + per-line galley cache + custom scrolling
  highlight.rs   SQL syntax highlighter (run per visible line by the editor)
  complete.rs    F6 autocomplete (schemas/tables/columns via FROM-alias) + Smart Tab
  find.rs        The find bar (Ctrl+F) — methods on JustQueryApp
  search.rs      Shared background search engine
  fileops.rs     Open / Save / Save As (impl JustQueryApp)
  doc/           Document model: piece-table + mmap, line index, undo, encoding detection + re-encode
  result.rs      The result data model: Row (compact, real NULLs), ResultSet (+ client-side sort),
                 ResultTab
  grid.rs        The virtualized result grid: pinned "#" column / sticky header, cell + row
                 selection, TSV copy, mouse column reorder/resize
  sample.rs      Demo grid data (test builds only)
  connections.rs Saved connections (connections.json), live connect, statement splitting, the run
                 workers (buffered + lazy COPY-streamed fetch), query cancellation
  connections_ui.rs  The Connection Manager dock, the connection-settings tab, connect/disconnect
  control.rs     The control connection's thread: liveness ping + the pg_terminate_backend kill switch
  crypt.rs       DPAPI password encrypt/decrypt for connections (crypt32 FFI, no extra crates)
  sqlentity.rs   The query's key entity (result-tab label) + the shared SQL lexing rules
  catalog.rs     System-catalog probes: schema/object lists, per-schema fingerprints, column fetch
  metadata.rs    Metadata Manager: shared in-memory object model (SharedStore) + tree/tab UI
  meta_collector.rs  Background scanner thread: incremental fingerprint-diff scan into SharedStore
  meta_details.rs    On-demand attribute fetcher (a metadata tab's columns) on its own connection
  meta_manager_modal.rs  Status-bar scan chip + the Scan tab (settings + activity log)
  proc.rs        Per-tab background processes (Search) + gating
  about.rs       The About tab + the status-bar version chip
  tests.rs       Regression tests: logic + headless render smoke tests
build.rs         Embeds the Windows app icon via winresource
assets/          Bundled fonts (JetBrains Mono regular + bold, the JustQuery icon set) and the app icon
tools/           generate_icon.py — regenerates assets/justquery.ico (pure Python)
```

The `find.rs` / `fileops.rs` modules add methods to `JustQueryApp` via `impl` blocks; they live in
child modules so they can use the struct's private state (a private item is visible to the
module that defines it *and its descendants*). Fonts and the icon are embedded via
`include_bytes!`, so the build is a single self-contained `justquery.exe`.

## Building & testing

Requires the **MSVC** Rust toolchain (`stable-x86_64-pc-windows-msvc`, Rust 1.88+ — the crate is
edition 2024) and the Visual Studio 2022 Build Tools (VCTools workload) for the linker.

```powershell
cargo build --release   # result: target\release\justquery.exe
cargo test --release    # logic + headless render smoke tests
```

The tests marked `#[ignore]` need a live PostgreSQL (`postgres` / `postgres` @ `localhost`, port
5432 or `JQ_TEST_PORT`). A throwaway server beside your own:

```powershell
docker run -d --rm --name jq-test-pg -e POSTGRES_PASSWORD=postgres -p 55432:5432 postgres:18
$env:JQ_TEST_PORT = 55432; cargo test --release -- --ignored --skip live_make_bigtest
docker stop jq-test-pg
```

(`live_make_bigtest` only builds a 10M-row table for manual testing.)

Run with `cargo run --release`, or launch the built `justquery.exe` directly.

## Roadmap

1. ✅ Pick a design from the web mockups (Claude Code Light).
2. ✅ Refine the UI in the browser prototypes.
3. ✅ Port the UI to egui (this shell).
4. ⏳ Wire up the MVP: connection management ✅, live connection (TLS) ✅, async query execution
   with streaming results and incremental fetch ✅, per-tab session connections ✅, query
   cancellation ✅ are in; still to do — explicit transactions (commit / rollback).

## Development & credits

JustQuery is built by a single developer and system architect: **Claude Opus 4.8**. All of the
code was — and continues to be — written with **[Claude Code](https://www.anthropic.com/claude-code)**.

Product direction comes from **votinvv**: system design, the interface/navigation design, and the
functional specification of the features implemented in the application.

## Disclaimer of warranty

JustQuery is provided **"AS IS", without warranty of any kind**, express or implied. To the maximum
extent permitted by law, the author accepts **no liability** for any damage, data loss, downtime or
other consequences arising from the use of (or inability to use) this software. **You use it at your
own risk.** This restates, in plain terms, the warranty and liability provisions already granted by
the Apache License 2.0 (Sections 7 and 8).

## License / third-party

JustQuery is licensed under the **[Apache License 2.0](LICENSE)** — free to use, modify and
redistribute, including commercially, provided the license and copyright notice are kept.

Bundled assets:

- **JetBrains Mono** (regular + bold) — SIL Open Font License (editor & grid).
- **Segoe UI** — loaded from the OS at runtime for UI text (not bundled); falls back to the
  built-in font if unavailable.