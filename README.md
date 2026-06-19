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
> result tabs in as each statement finishes, with incremental fetch, an execution log, and working
> **query cancellation** (Stop). Still to come: transactions and server-side cursors.

## Install

Windows, via [winget](https://learn.microsoft.com/windows/package-manager/):

```powershell
winget install votinvv.JustQuery
```

Or download the installer from the [latest release](https://github.com/votinvv/justquery/releases/latest).

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
- **Display:** anything 800×600 or larger (the window's minimum size is 760×480).
- **Network:** reachability of your PostgreSQL server; `github.com` is contacted only by the
  in-app update check.
- **Server:** PostgreSQL — any version speaking protocol 3.0 (PostgreSQL 7.4+; tested against
  current major versions).

## Design goals

- **Ruthless minimalism.** The center of gravity is one loop: *type a query → get a result.*
- **Speed and stability.** Native desktop, no web runtime; query cancellation must be bulletproof.
- **Windows-only, Postgres-only, single-developer** — scope kept deliberately narrow.

The visual theme is a cool, **silvery light** scheme: near-white panels frame the white editor,
Connection Manager and result *islands*, plus a neutral dark accent, sharp-cornered controls, and a
soft highlight for hovered/active tabs, menus and buttons. **One canonical border everywhere** —
islands, sheets, modals, fields, dropdowns, menus and the window outline all use a single thin
`border_strong` frame drawn as exactly **one physical pixel** ([`crisp_border`](src/widgets.rs),
pixel-snapped so it stays razor-sharp at any DPI scale), and **no drop shadows anywhere** (the app
is flat). The chrome rows (menu, toolbar, tabs) share one height and one element padding, so the
blank gaps between them come out equal. UI text uses the native **Segoe UI** for crispness; the
editor is **bold JetBrains Mono**. The UI is **English-only**. All colours live in one
[`Palette`](src/theme.rs) — the seam for a future light/dark switch.

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
  characters of breathing room from the right edge as you go, and its x is taken from the line's
  galley (not `column × char-width`), so it stays on the real text even where a tab or an odd-width
  glyph makes a character render wider than one cell.
- SQL syntax highlighting (keywords, strings, comments, numbers, function calls); non-highlighted
  text is regular-weight, highlighted tokens are bold.
- **Smart Enter** (keeps the line's indentation) and **Smart Tab** (pads to the next "hook" on the
  line above, PL/SQL-Developer style).
- **F6 autocomplete**: schemas on an empty context, a schema's relations after `schema.`, and a
  relation's columns after `alias.` (alias resolved from the statement's FROM/JOIN list).
- **File operations** via native Win32 dialogs: Open / Save / Save As, per-tab file path, dirty
  marker, "already open" de-duplication.
- **Find** (Ctrl+F): a small floating bar (with a × close) that searches on Enter, jumps to the
  match nearest the caret (Unicode-aware, case-insensitive), selects it, and steps with Ctrl+. /
  Ctrl+, (no F3).
- Unicode-aware word navigation in the editor (Ctrl+←/→ and Ctrl+Shift+←/→ work on Cyrillic etc.,
  not just ASCII — egui 0.34's built-in word boundary is Unicode-aware).
- Main menus (File/Edit/Search/Database/Tools/Window/Help); the **Edit** menu (undo/redo/cut/copy/
  paste/select-all) is wired to the focused editor. About and Scan open as singleton tabs (each at
  most once; reopening re-selects the existing tab). Modals are reserved for connecting and for
  action confirmations.
- A **Connection Manager** side panel (resizable, vertically scrolled): single click selects a
  connection (Ctrl/Shift for multi-select; selection commits on press so the accent moves instantly),
  a toolbar **Delete** removes the selected one(s), **F2** or a second click on the already-selected
  row renames it inline (Windows-style — a real double-click *opens* instead), and double-click opens
  the connection's settings tab where every field — including the **Name** — is editable (renaming
  moves the backing file; a Windows-style prompt resolves duplicate names). **Test Connection really
  connects** to Postgres over TLS and reports the server version + SSL state (or the error) in a
  modal. Each connection is stored as one DPAPI-encrypted file named after it, so names are unique.
- A live **main connection**: Connect opens it on a background thread behind a blocking
  "Connecting…" overlay, lights the green title-bar indicator on success (nothing is shown until
  connected) and routes failures to an error modal. Connect / Disconnect first guard against tabs
  with a running query or an open transaction (offering to go back or kill the work).
- **Query execution** (F8 / ▶): runs the selection if there is one, otherwise the whole tab, on a
  background worker thread. After Connect, `main_conn` is a *control* connection and **each tab
  opens its own session connection** — lazily on its first run and kept open afterwards (so SET /
  temp tables / prepared statements persist between queries) — which also lets tabs run
  concurrently. The result panel opens at once on **Messages**; each row-returning statement streams
  in as its own **Result** tab (you drive the tab selection), while a running indicator on the
  editor tab and a live d/h/m/s timer in the status bar show progress. **Stop** cancels the active
  tab's running query with a real PostgreSQL CancelRequest (the session connection is preserved).
- An execution-log **Messages** grid: one row per statement (Time / Status / Exec / Fetch / Rows /
  Message / SQL).
- A virtualized result grid: a pinned **#** row-number column and sticky header (both stay put while
  the data scrolls), a full-height vertical scrollbar, and styled native scrollbars. Cells are
  selectable (click / drag a rectangle) and copyable as TSV with Ctrl+C.
- PL/SQL-Developer-style incremental fetch (per result set) with a stop button.
- DB-action buttons are context-gated: Execute needs a live connection; Commit / Rollback need an
  open transaction.
- A **Metadata Manager** side panel + background **SCANER**: the system catalog is the single source
  of truth, polled with no server-side objects (no event triggers / extensions). Each scan reads a
  one-query **per-schema fingerprint** (folding in the `xmin` of relations, attributes, defaults and
  functions, so `ALTER COLUMN`/`ADD COLUMN` register too) and re-pulls **only the schemas whose
  fingerprint moved** — full objects + columns — into a shared in-memory store (locked for the UI's
  tree snapshot). It scans once per **interval** (default 30s, the "перекур") while the user is
  active and **sleeps after 5 min idle** (no DB churn); a cheap count guards a **1,000,000
  objects+attributes budget**, stopping with an error past it. The panel (vertically scrolled) shows
  a schema dropdown + Refresh and a one-level tree: type folders with a disclosure chevron over
  per-type object rows (table / view / sequence / function icons) that select on click (Ctrl/Shift
  multi-select) and open a metadata tab on double-click. The status-bar **SCAN** chip is a bold
  glyph + label, coloured green (active, the "перекур" included) / yellow (asleep) / red (failed) /
  grey (off); clicking it opens the **Scan** modal — live status, the interval / sleep / budget
  settings, a two-pane *available ⇄ monitored* schema picker (Ctrl/Shift multi-select, double-click
  to move), an activity log, and Apply / OK. A metadata tab fetches an object's columns on demand.
  All settings (interval, budget, idle, monitored schemas) persist to the connection file.
- Each editor tab keeps its **own** result-panel height, maximized state and per-result scroll.
- Operation errors (file I/O, connection failures, name validation) surface in **modals**, not the
  status bar; the status bar is reserved for panic recovery (a panic in the frame is caught and
  shown there instead of crashing).

## Project layout

```
src/
  main.rs        Application state (JustQueryApp) + screen-level layout + update loop
  theme.rs       Palette (all colours), style metrics, fonts, egui style — the look in one place
  widgets.rs     Reusable painted UI helpers (incl. the shared `show_modal` dialog scaffold) +
                 the custom window chrome
  codeeditor.rs  Virtualized SQL editor: renders only visible lines (O(visible) for any file size);
                 owns caret/selection/undo + per-line galley cache + custom kinetic scrolling
  grid.rs        The virtualized result grid (ResultSet + the O(visible) data grid): pinned "#"
                 column / sticky header, cell selection + TSV copy, mouse column reorder/resize
  complete.rs    F6 autocomplete (schemas/tables/columns via FROM-alias) + Smart Enter/Tab
  highlight.rs   SQL syntax highlighter (run per visible line by the editor)
  find.rs        Search logic + the find bar (impl JustQueryApp)
  fileops.rs     Open / Save / Save As (impl JustQueryApp)
  dialog.rs      Native Win32 helpers: Open/Save dialogs, clipboard read, local time (FFI)
  connections.rs Saved connections (DPAPI files) + Connection Manager + live connect + run_sql
                 + the catalog probes (list/fingerprint/count/scan_schema/object_columns)
  metadata.rs    Metadata Manager: shared in-memory object model (SharedStore) + tree/tab UI
  meta_collector.rs  Background SCANER thread: incremental fingerprint-diff scan into SharedStore
  meta_details.rs    On-demand attribute fetcher (a metadata tab's columns) on its own connection
  meta_manager_modal.rs  Status-bar SCAN chip + the Scan manager tab (settings + activity log)
  update.rs      In-app GitHub update check + self-update (status chip, About/Updates tab)
  crypt.rs       DPAPI password encrypt/decrypt for connections (crypt32 FFI, no extra crates)
  sample.rs      Demo data for the result-grid tests (test builds only)
  tests.rs       Regression tests: search/tab logic + headless render smoke tests
build.rs         Embeds the Windows app icon via winresource
assets/          Bundled fonts (JetBrains Mono regular + bold, the JustQuery icon set) and the app icon
tools/           generate_icon.py — regenerates assets/justquery.ico (pure Python)
```

The `find.rs` / `fileops.rs` modules add methods to `JustQueryApp` via `impl` blocks; they live in
child modules so they can use the struct's private state (a private item is visible to the
module that defines it *and its descendants*). Fonts and the icon are embedded via
`include_bytes!`, so the build is a single self-contained `justquery.exe`.

## Building & testing

Requires the **MSVC** Rust toolchain (`stable-x86_64-pc-windows-msvc`) and the Visual Studio
2022 Build Tools (VCTools workload) for the linker.

```powershell
cargo build --release   # result: target\release\justquery.exe
cargo test              # logic + headless render smoke tests
```

Run with `cargo run --release`, or launch the built `justquery.exe` directly.

## Roadmap

1. ✅ Pick a design from the web mockups (Claude Code Light).
2. ✅ Refine the UI in the browser prototypes.
3. ✅ Port the UI to egui (this shell).
4. ⏳ Wire up the MVP: connection management ✅, live connection (TLS) ✅, async query execution
   with streaming results and incremental fetch ✅, per-tab session connections ✅, query
   cancellation ✅ are in; still to do — transactions (commit/rollback) and server-side cursors.

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

## XML mode

Open a **`.xml`** file and JustQuery opens it as an **XML tab** — XML syntax highlighting and the
XML toolbar. The kind is decided by the file extension, not by sniffing the buffer: a fresh tab is
always SQL (even if you paste an `<?xml …` declaration), and becomes XML only once saved as `.xml`.
In XML mode the toolbar — the active tab's actions, merged into the main icon toolbar — offers:

- **Format** (F9) — streaming pretty-printer (quick-xml); entities/CDATA/comments preserved verbatim,
  applied as one undo step; on a not-well-formed document it jumps to the offending line.
- **Inspect** (F8) — XSD + the business rules (sections 5/6) for the selected schema version
  (**5.0 / 5.1**, picked from the toolbar combo between Format and Inspect); findings stream into the
  results grid (Type / Line / Code / Message); a finding clicked jumps to its line.

Both run in the background on a memory-mapped snapshot, so multi-gigabyte files stay responsive; the
tab is read-only while a process runs and Stop cancels it.

**Search** (Ctrl+F) is the same background engine for SQL and XML tabs: type, Enter, and every match
streams into the results grid (click a row to jump). The 100 MB results cap protects against
searching a frequent term in a huge dump.