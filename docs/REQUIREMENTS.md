# JustQuery — Software Requirements Specification (SRS)

The single source of truth for **what** the product must do (functional requirements)
and **what** it must be like (non-functional requirements). The document describes the target
functionality; the actual implementation status is marked explicitly (see the legend).

- **Product:** JustQuery — a minimalist desktop IDE for working with PostgreSQL.
- **Related documents:** `README.md` (user-facing overview), `docs/DESIGN_SYSTEM.md`
  (visual canon), `CLAUDE.md` (working context and architecture).
- **Audience:** product owner, developer, reviewers.

> **Separation principle.** The "Functional requirements" part describes the system's behaviour
> in domain terms and **does not mention implementation technologies** (language, framework,
> libraries). Everything technological belongs in the "Non-functional requirements" part.

## Status legend

| Status | Meaning |
|--------|----------|
| ✅ | Implemented |
| 🟡 | Partially implemented / present as a stub (UI exists, behaviour disabled) |
| ⏳ | Planned (on the roadmap, not yet implemented) |
| ⏸ | Deferred by the owner's decision |

## Identifier convention

- Functional requirements: **FR-<area>-<number>** (for example, `FR-EXEC-3`).
- Non-functional requirements: **NFR-<category>-<number>** (for example, `NFR-PERF-2`).
- Identifiers are **stable**: when a requirement is removed, its number is not reused.
- The wording "the system **shall**" denotes a mandatory requirement; "**may**" denotes an optional one.

---

# Part I. Functional requirements

What the system does from the perspective of the user and the domain. Without reference to technologies.

## 1. Database connections (CONN)

Scope: creation, storage and selection of named connections; test connection.

| ID | Requirement | Status |
|----|------------|--------|
| FR-CONN-1 | The system shall store an arbitrary number of **named connections**; a connection name is unique. | ✅ |
| FR-CONN-2 | A connection shall be described by a set of credentials: name, host, port, database, user and password. The secure channel (TLS) is negotiated opportunistically (`sslmode=prefer`) and is not separately configurable. | ✅ |
| FR-CONN-3 | A connection password shall be stored **in encrypted form** and shall not be available in plaintext in storage. | ✅ |
| FR-CONN-4 | The system shall provide a **connection manager** side panel listing the saved connections. | ✅ |
| FR-CONN-5 | The user shall be able to select one or more connections (single selection, as well as multi-selection with modifiers). | ✅ |
| FR-CONN-6 | The user shall be able to **create**, **delete** and **rename** a connection; renaming is available both as an inline edit and in the settings card. The manager shall also **import** a connection from a `.conn` file, and a connection tab shall **export** itself (Save As) to a `.conn` file. Exported files carry **no password** (and import ignores one), so credentials never travel with the file. | ✅ |
| FR-CONN-7 | Opening a connection (by double-click, or via the status-bar identity chip for the active one) shall show a **settings card** titled with the connection's **name**, with all editable fields, including the name. The **active (live) connection** is read-only here — its fields are **locked**, it is marked **green** in the manager list, a green `● active` / red `● disconnected` marker sits by its title, and its card shows a live **Session** block (server / database / since / user / pid / ssl). | ✅ |
| FR-CONN-8 | When attempting to set a name that matches an existing one, the system shall require the collision to be resolved and shall not allow duplicates. | ✅ |
| FR-CONN-9 | The **"Test connection"** feature shall perform a real connection to the server and report the result: the server version and the state of the secure channel — or the error text. Missing required fields (host / database / user) shall be reported **up front**, before any connection attempt (so a half-filled form fails fast instead of hanging until the connect timeout). | ✅ |
| FR-CONN-10 | The test result and errors of connection operations shall be shown in a **modal**, not in the status bar. | ✅ |

## 2. Main connection and sessions (SESS)

Scope: lifecycle of the active connection and the isolation of tabs by session.

| ID | Requirement | Status |
|----|------------|--------|
| FR-SESS-1 | The user shall be able to **establish an active (main) connection** from a selected saved connection. | ✅ |
| FR-SESS-2 | Establishing a connection shall happen without blocking the interface, with progress indication ("Connecting…") and a successful-connection indicator. | ✅ |
| FR-SESS-3 | A connection failure shall be reported in a modal; the "connected" indicator shall not be shown until a successful connection. | ✅ |
| FR-SESS-4 | Each editor tab shall work in its **own session**, created on the first query run and kept afterwards, so that `SET`s, temporary tables and prepared statements persist between queries. | ✅ |
| FR-SESS-5 | Separate sessions shall allow tabs to run queries **concurrently** without blocking one another. | ✅ |
| FR-SESS-6 | **Connect/Disconnect** shall warn about tabs with a running query or an open result stream and offer to cancel the operation or kill the work and proceed. | ✅ |

## 3. Code editor (EDIT)

Scope: input and navigation within the query/document text.

| ID | Requirement | Status |
|----|------------|--------|
| FR-EDIT-1 | The system shall provide a **multi-tab** editor; each tab keeps its own text, caret position and scroll. | ✅ |
| FR-EDIT-2 | The editor shall stay responsive on files of **any size** (editing and scrolling do not degrade as the file grows). | ✅ |
| FR-EDIT-3 | The editor shall show **line numbers**, active-line highlight, a caret, mouse selection, and smooth (kinetic) scrolling on both axes. | ✅ |
| FR-EDIT-4 | **Syntax highlighting** shall work for SQL (keywords, strings, comments, numbers, function calls). | ✅ |
| FR-EDIT-5 | **Smart Enter** (preserving the line's indentation) and **Smart Tab** (aligning to the "hook" of the line above) shall work. | ✅ |
| FR-EDIT-6 | **Autocomplete** (on request) shall work: schemas in an empty context, schema objects after `schema.`, columns of a relation after `alias.` (the alias is resolved from the FROM/JOIN list of the current statement). | ✅ |
| FR-EDIT-7 | Word navigation and word selection shall work correctly for non-ASCII text (including Cyrillic). | ✅ |
| FR-EDIT-8 | The caret shall be positioned by the real text geometry (not by "column × character width") and shall keep an offset from the right edge while typing. | ✅ |
| FR-EDIT-9 | Editing commands (undo/redo/cut/copy/paste/select-all) shall be bound to the active editor. | ✅ |
| FR-EDIT-10 | The undo history depth shall be **bounded** to avoid unbounded memory growth. | ✅ |
| FR-EDIT-11 | **Word deletion** shall work: Ctrl+Backspace deletes the word before the caret, Ctrl+Delete the word after it, using the same Unicode-aware word boundaries as word navigation (FR-EDIT-7). With a selection both delete the selection. | ✅ |

## 4. Query execution (EXEC)

Scope: running SQL, cancellation, streaming results, log.

| ID | Requirement | Status |
|----|------------|--------|
| FR-EXEC-1 | The **Execute** command shall run the selected fragment, or — when there is no selection — the entire tab text. | ✅ |
| FR-EXEC-2 | Execution shall run **in the background** without blocking the interface; a **spinning running indicator** shall be shown on the tab, and a live elapsed timer for the **active result tab** shall be shown in the **status bar** (see FR-RES-8 / FR-RES-11). | ✅ |
| FR-EXEC-3 | Each row-returning statement shall appear as a **separate result tab** as it becomes ready (streaming output). | ✅ |
| FR-EXEC-4 | The **Stop** command shall **reliably cancel** the active tab's running query while preserving its session connection. A cancel **during execution** shows a `Query cancelled` error (red) — the pill flips to **Cancelling…** at once, the CancelRequest is auto-re-fired once a second for 5 s, and past that the next Stop click offers **Force stop** (`pg_terminate_backend`; the session and its state are lost, the next run reconnects; a session that ignores even that is abandoned after 3 more seconds). **During a doscroll fetch** Stop pauses, keeping the partial result (green). | ✅ |
| FR-EXEC-5 | The Execute button shall be available only when there is an active connection, non-empty text, and no operation already running on the tab. | ✅ |
| FR-EXEC-6 | The system shall support **transactions** (Commit/Rollback); the actions are available only with an open transaction. | ⏳ |
| FR-EXEC-7 | The system shall support **server-side cursors**. | ⏳ |
| FR-EXEC-8 | In a multi-statement run, a statement that **fails or is cancelled** shall stop the remaining statements — the batch aborts at the first error / cancel and later statements do not run. | ✅ |

## 5. Query results (RES)

Scope: data grid, message log, incremental fetch.

| ID | Requirement | Status |
|----|------------|--------|
| FR-RES-1 | Results shall be shown in a **single bottom panel** with a set of tabs (data results and service ones). | ✅ |
| FR-RES-2 | After a run, each statement shall produce its own sheet in the panel — a data grid for a row-returning statement, or a one-row status sheet (Status / Line / Message) for a command or an error; the panel opens on the first such sheet. A unified **"Messages"** execution-log tab (time, exec/fetch duration, row count, SQL text per statement) is planned. | 🟡 |
| FR-RES-3 | The data grid shall have a pinned **row-number** column and a sticky header that stay in place while the data scrolls. | ✅ |
| FR-RES-4 | The grid shall stay responsive on large sets (only the visible area is rendered). | ✅ |
| FR-RES-5 | Cells shall be **selectable** (by click and by rectangle) and **copyable** in tabular form (TSV). **Whole rows** shall be selectable by clicking the row-number gutter — with **Ctrl** (toggle several non-adjacent rows) and **Alt** (extend a range from the anchor); a copy then yields the full selected rows. | ✅ |
| FR-RES-6 | Result sets shall be fetched **incrementally on demand** — lazily from the server **without disabling that query's parallel execution**. **Every** statement's first page equals the result panel's **on-screen row capacity** (10 rows by default — the panel auto-sizes to 10 whole rows plus the horizontal scrollbar's groove below the last one), read into memory before the grid appears (spinner meanwhile). The **last** statement then stays live: **fetch-next-page** (the page size frozen at run time), **fetch-to-end**, and **pause/resume**; fetch-to-end pulls up to **+100 MB of raw data**, then **warns** that everything fetched is kept in memory and fetching further may exhaust it — **Continue** fetches to the very end, **Cancel** keeps the fetch buttons armed. An **earlier** statement of a batch is **capped server-side** (its COPY subselect gets `LIMIT page+1`; the +1 probes "there is more" so an exactly-one-page result isn't falsely marked partial) — the stream ends by itself, with no cancels and no tail transfer; the user's own statement text is preserved everywhere (Refresh re-runs the original). A capped grid is marked partial — its fetch buttons stay **armed permanently** as the partial-data indicator (with the `…` on the result tab), each click explaining in a modal that the statement must be re-run (Refresh) to load the full result. **Refresh** of a sheet blanks it into the running look at once (pill + ticking timer). | ✅ |
| FR-RES-7 | Each editor tab shall remember **its own** result-panel height, maximized state, the scroll of each result, and its **on-screen row count** — so a new tab defaults to the standard row count independently of any other tab's panel size. | ✅ |
| FR-RES-8 | Background-process status (Find) shall be shown in the **status bar** (bound to the tab); the SQL **run timer** — the **active result tab's own** time in seconds, e.g. `[3.123456 sec]`, updated live (not summed across statements) — shows on the right of the status bar, while the rest of the SQL run state (row counts, outcome) is carried on the tabs (FR-RES-11). | ✅ |
| FR-RES-9 | The data grid shall support **client-side sorting** by clicking a column header (ascending → descending → cleared), and **multi-column** sorting with **Ctrl** (each added column shows an arrow and its priority number). Sorting applies to the rows **fetched so far**; further incremental fetch does not re-sort — it drops the sort marks and appends new rows at the end. | ✅ |
| FR-RES-10 | The result-panel height shall be adjustable by dragging the grab strip above the panel. | ✅ |
| FR-RES-11 | Run state shall be carried on the tabs. Each result tab is named after the query's **key entity** (the table after the last `FROM`; else the DML/DDL target; else the bare verb, e.g. `select 1` → `select`) — the name shows **immediately**, even on the spinner placeholder — and shows its **row count** in brackets (`[100500]`, `[100500…]` when more rows remain on the server, `[0]` for a command / error / empty set). The pill is **neutral except errors**, which are **red** (including a query the user cancelled **during execution**, whose grid shows a `Query cancelled` error); the active tab is marked by its pill background and a normal (brighter) text colour, **never** the accent colour (so an active tab — and its close `×` — never reads as a red error). A **small spinning loader** replaces the glyph while a query runs on that tab (editor and result tabs both); every statement that has started but not yet produced its sheet shows its own spinner tab immediately, so a slow multi-statement run shows each tab appear in turn. The **run timer** — the **active result tab's own** time in seconds with microsecond precision (`[3.123456 sec]`): that statement's execution plus the doscroll fetches accrued **on that tab**, updated live, **not summed across statements** (doscrolling one tab grows only its value) — is shown in the **status bar** (right side, after the connection and version chips); the editor tab carries only its dirty marker (no timer). Cancelling **during a doscroll fetch** keeps the partial result. While a sheet is still loading its first rows, a centered, non-interactive **"Running" pill** (animated dots) is shown over the empty area. | ✅ |

## 6. Metadata and background catalog scanning (META)

Scope: metadata manager, catalog scanner, budgets and schedule.

| ID | Requirement | Status |
|----|------------|--------|
| FR-META-1 | The single source of truth about the DB schema shall be the **system catalog**; scanning shall not require server-side objects (event triggers, extensions). | ✅ |
| FR-META-2 | Scanning shall be **incremental**: by a per-schema fingerprint, only the changed schemas are re-read (including tracking of `ALTER/ADD COLUMN`). | ✅ |
| FR-META-3 | The system shall provide a **metadata manager** side panel with a schema picker, a refresh button and a single-level object tree (folders by type → objects: tables/views/sequences/functions). | ✅ |
| FR-META-4 | Tree objects shall be selectable on click (with multi-selection) and open a **metadata tab** on double-click; an object's columns are fetched on demand. | ✅ |
| FR-META-5 | Scanning shall run **on an interval** while the user is active and **fall asleep** after an idle period, creating no DB load while idle. | ✅ |
| FR-META-6 | The system shall respect a **budget** on the catalog volume and stop scanning with an error when it is exceeded. | ✅ |
| FR-META-7 | The status bar shall carry two clickable labels: a **connection chip** (`login@<connection name>`, green while connected / red when dropped) opening the **active connection's settings tab** (which shows the live connection details), and a **`scan` label** (shown while connected, coloured by scanner lifecycle: active / asleep / stopped — the stop reason is the last activity-log line) opening the **Scan tab**. | ✅ |
| FR-META-8 | The **Scan tab** shall provide: live scan status, settings (interval / sleep-after-idle / budget), a two-pane transfer picker of monitored schemas (available ⇄ monitored, with monitor-all / monitor-none), an activity log, and **Enable / Disable** (toolbar Execute / Stop) and **Apply** (toolbar Save) actions. | ✅ |
| FR-META-9 | The scan settings (interval, budget, idle, monitored schemas) shall be **saved** together with the connection. | ✅ |

## 7. File operations (FILE)

| ID | Requirement | Status |
|----|------------|--------|
| FR-FILE-1 | **Open / Save / Save As** operations via system dialogs shall work; a tab remembers its path and the unsaved-changes marker. | ✅ |
| FR-FILE-2 | When opening an already-open file, the system shall **reuse** the existing tab instead of opening a duplicate. | ✅ |
| FR-FILE-3 | Large files shall open **in the background** and shall not be loaded into memory in full. | ✅ |
| FR-FILE-4 | The system shall detect the **encoding and line-ending type** on opening and save the document consistently. | ✅ |

## 8. Search (FIND)

| ID | Requirement | Status |
|----|------------|--------|
| FR-FIND-1 | There shall be a **search** in the editor (invoked by a hotkey): a compact bar with a close, searching on Enter. | ✅ |
| FR-FIND-2 | Matching shall be Unicode-aware and case-insensitive; very long lines are processed in segments. | ✅ |
| FR-FIND-3 | The search shall be a **single background** engine: for large documents the matches are streamed into the result grid, and clicking a row jumps to the match. | ✅ |
| FR-FIND-4 | The volume of search results shall be **bounded** to protect against searching a frequent term in a huge dump. | ✅ |

## 9. Shell, window and tabs (SHELL)

| ID | Requirement | Status |
|----|------------|--------|
| FR-SHELL-1 | The window shall have its **own chrome** (the system title bar is disabled): caption buttons, drag-to-move, double-click to maximize, edge/corner resize, a window border. | ✅ |
| FR-SHELL-2 | Tabs shall be **drag-reorderable** and switchable from the keyboard; on overflow the tab strip scrolls with arrows. | ✅ |
| FR-SHELL-3 | The main menu shall be **static** (File/Edit/Search/Database/Tools/Window/Help) and shall not change with the tab kind. | ✅ |
| FR-SHELL-4 | Service screens (About, scan control) shall open as **singleton tabs** (reopening switches to the existing one). The live connection view is part of the active connection's settings tab, not a separate screen. | ✅ |
| FR-SHELL-5 | The **editor actions** group in the toolbar shall be static (`Refact · Inspect · Execute · Stop`) and shall not "jump" with the tab kind — only the buttons' liveness changes (live/dimmed). | ✅ |
| FR-SHELL-6 | The status bar shall show **encoding, EOL, line/column** (segments split by a vertical divider). | ✅ |
| FR-SHELL-7 | Operation errors (files, connections, name validation) — and a caught frame-failure panic — shall be shown in **modals**; the status bar carries only editor state and background-task (Find) status. | ✅ |
| FR-SHELL-8 | The **Format** command for SQL (refactoring) shall format the SQL text. | 🟡 (stub: button dimmed, tooltip "coming soon") |
| FR-SHELL-9 | The **Inspect** command for SQL shall perform a static SQL check. | 🟡 (cut out pending redesign) |

## 10. Application updates (UPD)

| ID | Requirement | Status |
|----|------------|--------|
| FR-UPD-1 | The system shall perform a **built-in update check** and report an available new version (a state indicator). | ✅ |
| FR-UPD-2 | The system shall provide the ability to **self-update** (download and install a new version) from within the application. | ✅ |
| FR-UPD-3 | The update check is the application's only access to the external network (besides the DB server itself). | ✅ |

---

# Part II. Non-functional requirements

What the product must be like: qualities, constraints and the technology platform. Here
**implementation technologies are stated explicitly** — they are requirements on the product.

## 13. Technology platform (TECH)

| ID | Requirement | Status |
|----|------------|--------|
| NFR-TECH-1 | The product shall be implemented in the **Rust** language (2021 edition, the MSVC toolchain `stable-x86_64-pc-windows-msvc`). | ✅ |
| NFR-TECH-2 | The user interface shall be built on **egui/eframe (0.35)** — an immediate-mode GUI with no web runtime. | ✅ |
| NFR-TECH-3 | Rendering shall go through **wgpu** (DirectX 12 / Vulkan) with a software **WARP** fallback available on any Windows 10+ install — so the application runs without a dedicated GPU driver (virtual machines, remote desktop). | ✅ |
| NFR-TECH-4 | PostgreSQL access — via the `postgres` client; the secure channel — via the Windows system TLS (SChannel) through `native-tls`. | ✅ |
| NFR-TECH-5 | The editor and the grid shall be **custom virtualized** components (not egui's stock `TextEdit`), to keep rendering complexity O(visible). | ✅ |
| NFR-TECH-6 | The document model shall be a **piece table + memory-mapped** source buffer with encoding/EOL detection, so the file is not loaded in full. | ✅ |
| NFR-TECH-8 | The build shall produce a **single self-contained** executable (fonts and icons embedded in the binary). | ✅ |
| NFR-TECH-9 | Dependencies shall be kept minimal; duplicate versions of the product's own **direct** dependencies are not allowed. Duplicates may remain only in transitive, non-Windows-target deps. | ✅ |

## 14. Performance (PERF)

| ID | Requirement | Status |
|----|------------|--------|
| NFR-PERF-1 | Editor editing and scrolling shall be **O(visible lines)** regardless of file size (multi-gigabyte files stay responsive). | ✅ |
| NFR-PERF-2 | The result grid shall render **only the visible area**; large sets shall not degrade scrolling. | ✅ |
| NFR-PERF-3 | All long-running operations (query execution, metadata scanning, search, update check) shall run on **background threads**; the UI frame shall not be blocked. | ✅ |
| NFR-PERF-4 | Heavy file operations (search) shall work on a **snapshot via mmap**, without loading the file into memory in full. | ✅ |
| NFR-PERF-5 | The per-line highlighting lexer shall **always advance** through the text; the full pass and `*_state_only` shall converge on the same final state (otherwise a frame hang) — an invariant of performance correctness. | ✅ |
| NFR-PERF-6 | Background metadata scanning shall create no DB load while idle: falling asleep after idle and re-reading only the changed schemas. | ✅ |

## 15. Reliability and resilience (REL)

| ID | Requirement | Status |
|----|------------|--------|
| NFR-REL-1 | Query cancellation (Stop) shall be **reliable** — this is an explicit product goal; cancellation is implemented by a real PostgreSQL `CancelRequest` while preserving the session. | ✅ |
| NFR-REL-2 | A panic in the frame shall be **caught** and shown in an error modal, rather than leading to a crash. | ✅ |
| NFR-REL-3 | Closing a tab shall **correctly cancel** the associated background work (threads, mmap, temporary files) — with no zombie threads or leaks. | ✅ |
| NFR-REL-4 | Metadata scanning shall respect a **budget** (on the order of 1,000,000 objects+attributes) and stop with an error when exceeded. | ✅ |
| NFR-REL-6 | On a startup failure the reason shall be shown in a message box and appended to `%APPDATA%\JustQuery\startup-error.log`. | ✅ |
| NFR-REL-7 | The product is distributed as a **pre-release "as is"** for developers and is **not intended** for production use (see the disclaimer of warranty). | ✅ |

## 16. Security (SEC)

| ID | Requirement | Status |
|----|------------|--------|
| NFR-SEC-1 | Connection passwords shall be stored encrypted by the OS (**DPAPI**, bound to the user/machine); there shall be no plaintext password storage. | ✅ |
| NFR-SEC-2 | The DB connection shall support **TLS** via the Windows system stack (SChannel). | ✅ |
| NFR-SEC-3 | Each connection is stored in a **separate file** whose name matches the connection name (names are unique). | ✅ |
| NFR-SEC-4 | The application shall not access the external network except for the DB server and the update check (`github.com`). | ✅ |

## 17. Compatibility and system requirements (COMP)

| ID | Requirement | Status |
|----|------------|--------|
| NFR-COMP-1 | **OS:** Windows 10 or 11, 64-bit (Windows 8.1 and older are not supported). | ✅ |
| NFR-COMP-2 | **CPU:** any x64; 2+ cores recommended (background threads for queries/scanning/updates). | ✅ |
| NFR-COMP-3 | **GPU:** not required (WARP fallback); any GPU with a working driver makes rendering "free". | ✅ |
| NFR-COMP-4 | **Server:** PostgreSQL of any version speaking protocol 3.0 (PostgreSQL 7.4+; tested against current major versions). | ✅ |
| NFR-COMP-5 | **Display:** from 1024×600; the minimum window size is 1024×600. | ✅ |
| NFR-COMP-6 | Installation via **winget** (`votinvv.JustQuery`) and via the installer from a GitHub Release. | ✅ |

## 18. Resources (RES-NF)

| ID | Requirement | Status |
|----|------------|--------|
| NFR-RES-1 | **RAM:** idle usage on the order of 300 MB; a comfortable minimum of system memory is 4 GB. Results are held in memory — large sets increase usage. | ✅ |
| NFR-RES-2 | **Disk:** on the order of 50 MB. | ✅ |
| NFR-RES-3 | Storage of user data (connections, settings) — in `%APPDATA%\JustQuery`. | ✅ |

## 19. Usability and interface consistency (UX)

| ID | Requirement | Status |
|----|------------|--------|
| NFR-UX-1 | The guiding principle is **ruthless minimalism**: a single center of gravity — "type a query → get a result". | ✅ |
| NFR-UX-2 | The appearance shall follow the **design system** (`docs/DESIGN_SYSTEM.md`): two surfaces (SURFACE/CHROME), one canonical 1px border everywhere, flat components with a single soft `island_shadow()` under raised surfaces (islands/menus/modals), and a single spacing rhythm. | ✅ |
| NFR-UX-3 | Colour shall carry meaning: coral — selected/primary action/focus; green — "live/healthy"; amber — "needs attention"; red — "error/destructive". | ✅ |
| NFR-UX-4 | Below the main toolbar there shall be exactly **two row heights** on screen: top chrome (30) and everything else (22, "like the buttons"). | ✅ |
| NFR-UX-5 | Interface text — the OS system font for crispness; the editor — monospace (JetBrains Mono). | ✅ |
| NFR-UX-6 | The I-beam cursor shall be **themed** (1px, colour by theme) and shall not stick white in a GPU window. | ✅ |

## 20. Localization (L10N)

| ID | Requirement | Status |
|----|------------|--------|
| NFR-L10N-1 | The application interface — **English only**. | ✅ |
| NFR-L10N-2 | All colours and metrics are concentrated in a single palette/theme module (`theme.rs`) that ships **two complete palettes** with a runtime **Light/Dark switch** (Appearance menu, persisted across launches). | ✅ |

## 21. Maintainability (MAINT)

| ID | Requirement | Status |
|----|------------|--------|
| NFR-MAINT-1 | The architecture shall be **modular** with an explicit separation of concerns (document model, editor, grid, connections, metadata — see `README.md`). | ✅ |
| NFR-MAINT-2 | The tab kind is a single flat `enum`. | ✅ |
| NFR-MAINT-3 | Highlighting arrives at the editor via a callback (the editor is language-neutral), which isolates language specifics. | ✅ |
| NFR-MAINT-4 | The codebase shall be maintained with **regression tests** (logic + headless render); building a release and running tests — `cargo build --release` / `cargo test --release`. | ✅ |
| NFR-MAINT-5 | The design canon, working context and requirements are kept in the repository as documents (`docs/DESIGN_SYSTEM.md`, `CLAUDE.md`, this document). | ✅ |

## 22. Legal and delivery (LEGAL)

| ID | Requirement | Status |
|----|------------|--------|
| NFR-LEGAL-1 | The product license — **Apache License 2.0**. | ✅ |
| NFR-LEGAL-2 | The bundled JetBrains Mono fonts are supplied under the **SIL Open Font License**; Segoe UI — loaded from the OS, not distributed with the product. | ✅ |
| NFR-LEGAL-3 | The product is supplied **without warranty** ("as is"), with a disclaimer of liability to the extent permitted by law (echoing Sections 7–8 of Apache 2.0). | ✅ |

---

## Appendix A. What is deliberately out of scope

- **Cross-platform support.** Windows only. PostgreSQL only. The narrow scope is intentional.
- **SQL Refact / SQL Inspect.** Cut out pending a "from scratch" redesign; in the toolbar — dimmed stubs.

## Appendix B. Document change history

| Date | Change |
|------|-----------|
| 2026-06-26 | First edition: FRs (sections 1–12) and NFRs (sections 13–22) extracted from README/CLAUDE.md/the design system. |
| 2026-06-28 | Code-accuracy review against the implementation. Per-tab result row memory (FR-RES-7); fixed the duplicate `FR-RES-9` → `FR-RES-10`. Corrected: FR-CONN-2 (TLS opportunistic, not configurable), FR-SESS-6 (open result stream, not transaction), FR-EXEC-2 (no live timer), FR-RES-2 (per-statement sheets; unified Messages log planned → 🟡), FR-META-7/8 (connection chip + Session-tab scan block), FR-MODEL-2/5/8/10 (no rule enable/disable; manifest without `name`; panel create), NFR-TECH-9 (transitive Wayland dup allowed), NFR-L10N-2 (Light/Dark switch shipped), NFR-UX-2 (soft `island_shadow`, not "no shadows"). |
| 2026-06-28 | Added FR-EDIT-11: word deletion in the editor (Ctrl+Backspace / Ctrl+Delete). |
| 2026-07-02 | **XML mode and XML models removed from the product** (the owner's decision to simplify). Deleted: sections "9. XML mode (XML)" and "10. XML models (MODEL)" — the FR-XML-* and FR-MODEL-* series are retired (numbers not reused); NFR-TECH-7 (XML parsing deps) and NFR-REL-5 (model checksum). The SHELL/UPD sections renumbered 11–12 → 9–10. Updated: FR-RES-8, FR-FIND-3, FR-SHELL-5/6, NFR-PERF-3/4, NFR-RES-3, NFR-MAINT-1/2, NFR-TECH-9, Appendix A. |
| 2026-07-03 | FR-META-7: the scanner lifecycle merged **failed/paused → stopped** (one red state; the reason is the last activity-log line); a failed scan now **parks** the collector until Enable scan (previously it retried each interval). Save is allowed for a never-saved tab (routes to Save As), so an **empty file can be saved**; the Metadata Manager **Refresh** button is dimmed unless a newer scan is waiting (green tint removed). FR-RES-6: **every** row-returning statement buffers a bounded block (1000 rows) into memory (spinner meanwhile) before its grid appears — consistent across the last and intermediate statements; fetch-to-end warns at +100 MB (Continue → to the end / Cancel); partial grids keep their fetch buttons **armed permanently** as the incompleteness indicator; the h-scrollbar's home strip moved into the grid content under the **last** row (floating over rows until scrolled to the end). FR-CONN-6: **exports carry no password** (and import ignores one). Toolbar verbs got function keys: **F7 Stop / F8 Run / F9 Refact / F10 Inspect** (Inspect moved off F5). Later the same day, simplified to the final shape: first page = the panel's **screen capacity** for every statement (default panel = 10 rows + the h-scroll groove); intermediate statements are **capped server-side** (COPY subselect `LIMIT page+1` — the one sanctioned adaptation of the SQL, invisible outside that COPY) so their stream ends by itself; abandoned parked streams close via a single worker-owned cancel + drain (+1 s grace on EOF; `close_lazy_stream` trips the lightning flag so an in-flight fetch yields promptly). FR-EXEC-4: Stop escalates — Cancelling…, 5 s of auto-re-fired cancels, then a **Force stop** confirm (`pg_terminate_backend`, abandon after 3 s more). Refresh blanks the sheet into the running look with a ticking timer. |
| 2026-07-12 | Connect lifecycle hardened: a failed or cancelled Connect attempt no longer marks the target connection active (identity is staged and applied only on success — see ARCHITECTURE §8). **Winget publishing automated**: a `winget` job in `release.yml` submits each release via `wingetcreate` (the standalone `winget.yml` never fired and was removed); dropped the "automated winget publishing is out of scope" item from Appendix A. |
