//! Saved database connections: the data model, on-disk persistence (passwords encrypted with
//! DPAPI), and the live postgres plumbing (TLS, connect, statement splitting/execution). The
//! dialogs, the Connection Manager dock and the connect/disconnect orchestration live in
//! [`crate::connections_ui`]; the catalog introspection probes in [`crate::catalog`].

use crate::crypt;
use native_tls::TlsConnector;
use postgres::Config;
use postgres_native_tls::MakeTlsConnector;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

/// One saved connection. `id` is a stable session handle linking the sidebar list to an open
/// editor tab. `password` is kept in clear in memory but encrypted on disk.
#[derive(Clone)]
pub struct Connection {
    pub id: u64,
    pub name: String,
    pub host: String,
    pub port: String,
    pub db: String,
    pub user: String,
    pub password: String,
    /// Creation order key (ms since epoch, stamped once): the manager list and the Connect
    /// dropdown are sorted by this so connections appear in the order they were created, not
    /// alphabetically. Persisted in the `.conn` file so it survives renames.
    pub created: u64,
    // ---- Metadata Manager settings (per-connection, persisted in the .conn file) ----
    /// Background metadata collector enabled (the periodic object-list scan).
    pub meta_enabled: bool,
    /// Pause between scans, seconds.
    pub meta_interval: u64,
    /// Memory budget as a max object count (the collector stops growing past it).
    pub meta_budget: usize,
    /// Seconds of user inactivity before the scanner sleeps.
    pub meta_idle: u64,
    /// Schemas to monitor; `None` = all user schemas.
    pub meta_schemas: Option<Vec<String>>,
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            host: String::new(),
            port: "5432".to_owned(),
            db: String::new(),
            user: String::new(),
            password: String::new(),
            created: 0,
            meta_enabled: true,
            meta_interval: 30,
            meta_budget: 1_000_000,
            meta_idle: 300,
            meta_schemas: None,
        }
    }
}

/// Milliseconds since the Unix epoch — the creation-order stamp for a new connection.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Directory holding one file per saved connection: `%APPDATA%\JustQuery\connections\`.
fn config_dir() -> Option<PathBuf> {
    Some(crate::appdata_dir()?.join("connections"))
}

/// Turn a connection name into a safe file stem (the file name is the connection's identity,
/// which is what gives us automatic uniqueness).
pub(crate) fn safe_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() {
        return "unnamed".to_owned();
    }
    // Windows reserved device names (CON, NUL, COM1…) can't be file stems even with an extension —
    // prefix an underscore so e.g. a connection named "CON" maps to a usable "_CON.conn".
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&s.to_ascii_uppercase().as_str()) {
        format!("_{s}")
    } else {
        s
    }
}

/// Parse a `meta_schemas` value: empty or `*` → `None` (monitor all schemas); otherwise the
/// comma-separated list of schema names (trimmed, empties dropped).
fn parse_schema_list(v: &str) -> Option<Vec<String>> {
    let v = v.trim();
    if v.is_empty() || v == "*" {
        return None;
    }
    let list: Vec<String> = v
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}

/// Uniqueness key for a connection name: the on-disk file stem, case-folded. Because the file name
/// IS the connection's identity, two display names that sanitise to the same file (e.g. "a/b" and
/// "a:b" → "a_b.conn") must be treated as duplicates, or one would silently overwrite the other.
pub(crate) fn name_key(name: &str) -> String {
    safe_name(name).to_lowercase()
}

/// Drop a trailing " (N)" suffix so "foo (2)" → "foo" (used to build the next free variant).
pub(crate) fn strip_paren_suffix(name: &str) -> String {
    let t = name.trim();
    if t.ends_with(')') {
        if let Some(open) = t.rfind('(') {
            let inner = &t[open + 1..t.len() - 1];
            if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit()) {
                return t[..open].trim_end().to_string();
            }
        }
    }
    t.to_string()
}

/// Parse one `.conn` file body into a `Connection` (the name comes from the file stem, not the
/// body). The inverse of [`conn_to_text`]; shared by the folder [`load`] and by connection Import.
/// `created` is taken from the body if present (0 otherwise — the caller backfills).
pub(crate) fn parse_conn(text: &str, name: String) -> Connection {
    let mut c = Connection {
        name,
        port: "5432".into(),
        ..Default::default()
    };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "host" => c.host = v.to_owned(),
            "port" => c.port = v.to_owned(),
            "database" => c.db = v.to_owned(),
            "user" => c.user = v.to_owned(),
            "created" => c.created = v.parse().unwrap_or(0),
            "meta_enabled" => c.meta_enabled = v != "false" && v != "0",
            "meta_interval" => c.meta_interval = v.parse().unwrap_or(30),
            "meta_budget_objects" => c.meta_budget = v.parse().unwrap_or(1_000_000),
            "meta_idle" => c.meta_idle = v.parse().unwrap_or(300),
            "meta_schemas" => c.meta_schemas = parse_schema_list(v),
            "password" => {
                // DPAPI-decrypt: blank on any failure (e.g. a file imported from another
                // machine/user — the ciphertext is bound to the origin, so the field just clears).
                c.password = if v.is_empty() {
                    String::new()
                } else {
                    crypt::from_hex(v)
                        .and_then(|b| crypt::unprotect(&b))
                        .and_then(|b| String::from_utf8(b).ok())
                        .unwrap_or_default()
                };
            }
            _ => {}
        }
    }
    c
}

/// Serialize a `Connection` to the `.conn` file body (`key=value` lines; password DPAPI-encrypted
/// hex). The inverse of [`parse_conn`]; shared by [`save`] and by connection Export.
pub(crate) fn conn_to_text(c: &Connection) -> String {
    let pass = if c.password.is_empty() {
        String::new()
    } else {
        crypt::protect(c.password.as_bytes())
            .map(|b| crypt::to_hex(&b))
            .unwrap_or_default()
    };
    let schemas = match &c.meta_schemas {
        None => "*".to_owned(),
        Some(list) => list.join(","),
    };
    format!(
        "host={}\nport={}\ndatabase={}\nuser={}\npassword={}\ncreated={}\n\
         meta_enabled={}\nmeta_interval={}\nmeta_budget_objects={}\nmeta_idle={}\nmeta_schemas={}\n",
        c.host, c.port, c.db, c.user, pass, c.created,
        c.meta_enabled, c.meta_interval, c.meta_budget, c.meta_idle, schemas
    )
}

/// Load every connection: one `*.conn` file per connection in the connections directory. The file
/// name (without extension) is the connection name. Password is DPAPI-encrypted hex; the rest is
/// plain `key=value` lines.
pub fn load() -> Vec<Connection> {
    let Some(dir) = config_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("conn"))
        .collect();
    let mut out = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_owned();
        let mut c = parse_conn(&text, name);
        // pre-feature files have no `created` stamp → backfill from the file's own creation time so
        // they still order by when they were made (persisted on the next save)
        if c.created == 0 {
            c.created = std::fs::metadata(&path)
                .and_then(|m| m.created())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
        }
        out.push(c);
    }
    // creation order; name as a stable tiebreaker for equal/zero stamps
    out.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.name.cmp(&b.name)));
    for (i, c) in out.iter_mut().enumerate() {
        c.id = (i + 1) as u64;
    }
    out
}

/// Persist connections — one `<name>.conn` file each. Orphaned files (from renames/deletes) are
/// removed so the directory mirrors the list exactly. Same name ⇒ same file ⇒ automatic uniqueness.
pub fn save(conns: &[Connection]) {
    let Some(dir) = config_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let mut keep: std::collections::HashSet<String> = std::collections::HashSet::new();
    for c in conns {
        let fname = format!("{}.conn", safe_name(&c.name));
        keep.insert(fname.clone());
        let _ = std::fs::write(dir.join(&fname), conn_to_text(c));
    }
    // drop files that no longer correspond to a connection
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some("conn") {
                if let Some(fname) = p.file_name().and_then(|s| s.to_str()) {
                    if !keep.contains(fname) {
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
        }
    }
}

/// Build a TLS config and open a real Postgres connection, returning the live client. Shared by
/// Test Connection (which then reads the server version) and the main connect. Self-signed/dev
/// certs are accepted (no verification) — see the SSL note below.
pub(crate) fn connect_client(
    host: &str,
    port: u16,
    db: &str,
    user: &str,
    password: &str,
) -> Result<postgres::Client, String> {
    let mut cfg = Config::new();
    cfg.host(host)
        .port(port)
        .dbname(db)
        .user(user)
        .ssl_mode(postgres::config::SslMode::Prefer)
        .connect_timeout(Duration::from_secs(8))
        // keep the socket alive so idle session connections (the per-tab ones especially) aren't
        // silently dropped by the OS / a firewall between queries
        .keepalives(true)
        .keepalives_idle(Duration::from_secs(30));
    if !password.is_empty() {
        cfg.password(password);
    }
    cfg.connect(make_tls()?).map_err(|e| err_chain(&e))
}

/// Open a control connection and, in ONE round-trip, capture its backend pid + ssl flag (the same
/// `pg_stat_ssl` probe the Test-Connection dialog runs). The Session tab shows these live
/// attributes. Used by the Connect background thread (which used to inline this probe verbatim).
/// Returns `(client, Some(pid), Some(ssl))` on success.
pub(crate) fn connect_client_probed(
    host: &str,
    port: u16,
    db: &str,
    user: &str,
    password: &str,
) -> Result<(postgres::Client, Option<i32>, Option<bool>), String> {
    connect_client(host, port, db, user, password).map(|mut client| {
        let row: Result<(i32, bool), _> = client
            .query_one(
                "SELECT pg_backend_pid(), \
                 (SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid())",
                &[],
            )
            .map(|r| (r.get(0), r.get::<_, Option<bool>>(1).unwrap_or(false)));
        let (pid, ssl) = row.unwrap_or((0, false));
        (client, Some(pid), Some(ssl))
    })
}

/// Resolved credentials of the active connection, captured at Connect time so each editor tab can
/// open its own independent session connection (same server/db/user, possibly an overridden login).
#[derive(Clone)]
pub struct ConnParams {
    pub host: String,
    pub port: String,
    pub db: String,
    pub user: String,
    pub password: String,
}

/// Open a fresh session connection for a tab from the captured params (blocking — call off-thread).
pub(crate) fn connect_session(p: &ConnParams) -> Result<postgres::Client, String> {
    let port = parse_port(&p.port)?;
    connect_client(&p.host, port, &p.db, &p.user, &p.password)
}

/// Open a metadata-worker session and apply `statement_timeout` once (0 = unlimited) — the prologue
/// every metadata worker runs as its first step. The collector passes 0 (a full catalog scan must
/// not be cut off mid-read); the details worker passes its configured per-query timeout.
pub(crate) fn connect_session_with_timeout(
    p: &ConnParams,
    stmt_timeout_ms: u64,
) -> Result<postgres::Client, String> {
    let mut client = connect_session(p)?;
    let _ = client.batch_execute(&format!("SET statement_timeout = {stmt_timeout_ms}"));
    Ok(client)
}

/// The shared TLS connector (lenient cert/hostname checks — we connect to user-configured DBs).
/// Used both for opening connections and for sending out-of-band query-cancel requests.
pub(crate) fn make_tls() -> Result<MakeTlsConnector, String> {
    let tls = TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|e| err_chain(&e))?;
    Ok(MakeTlsConnector::new(tls))
}

/// Fire a PostgreSQL `CancelRequest` for `token` on a throwaway thread so the UI thread never
/// blocks on the network. A fresh TLS connector is built per call (matches `make_tls`). Result is
/// discarded: the in-flight statement is aborted server-side; the worker still returns its moved
/// client, so the connection is preserved.
pub(crate) fn spawn_cancel(token: postgres::CancelToken) {
    if let Ok(tls) = make_tls() {
        std::thread::spawn(move || {
            let _ = token.cancel_query(tls);
        });
    }
}

/// Parse a port string: blank → the Postgres default 5432, otherwise a u16 (or an error).
pub(crate) fn parse_port(port: &str) -> Result<u16, String> {
    let p = port.trim();
    if p.is_empty() {
        Ok(5432)
    } else {
        p.parse::<u16>().map_err(|_| format!("Invalid port: \"{port}\""))
    }
}

pub(crate) fn try_connect(
    host: &str,
    port: u16,
    db: &str,
    user: &str,
    password: &str,
) -> Result<String, String> {
    let mut client = connect_client(host, port, db, user, password)?;
    // The connect phase is bounded by `connect_timeout` (8s); bound the probe queries too, so a
    // server that accepts the TCP/auth handshake but then hangs on a query can't pin the
    // Test-Connection spinner indefinitely (the modal owns the lifecycle — no Stop verb). Same
    // mechanism as `connect_session_with_timeout`.
    let _ = client.batch_execute("SET statement_timeout = 5000");
    let ver: String = client
        .query_one("SELECT version()", &[])
        .map_err(|e| err_chain(&e))?
        .get(0);
    let tls_on: bool = client
        .query_one(
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            &[],
        )
        .map(|r| r.get(0))
        .unwrap_or(false);
    let short = ver.lines().next().unwrap_or(&ver);
    Ok(format!("{short}\nTLS: {}", if tls_on { "on" } else { "off" }))
}

/// Flatten an error + its `source()` chain into one readable line (tokio-postgres hides the useful
/// part — "invalid configuration", "db error", … — in the source).
pub(crate) fn err_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(inner) = src {
        let t = inner.to_string();
        if !s.ends_with(&t) {
            s.push_str(": ");
            s.push_str(&t);
        }
        src = inner.source();
    }
    s
}

/// Strip the redundant wrappers a Postgres error is often buried under
/// (`"db error: ERROR: …"` → the actual message). The grid's red Status column already flags it as
/// an error, so saying it once is enough.
fn pg_error_msg(e: &dyn std::error::Error) -> String {
    let chain = err_chain(e);
    let msg = chain.strip_prefix("db error: ").unwrap_or(&chain);
    msg.strip_prefix("ERROR: ").unwrap_or(msg).to_owned()
}

/// One outcome of running a single statement: a result set (rows returned) or a message line
/// (a command's affected-row count, or an error).
pub(crate) enum SqlOut {
    // boxed — `ResultSet` is large (~0.2 KB) and `Note` is a single String, so an unboxed `Rows`
    // would bloat every `SqlOut` in the returned `Vec` to the grid's size (clippy large_enum_variant)
    Rows(Box<crate::ResultSet>),
    Note(String),
}

/// Split a SQL script into individual statements on top-level `;`. Semicolons inside single-quoted
/// strings, dollar-quoted blocks (`$$ … $$`, `$tag$ … $tag$` — function bodies / `DO` blocks) and
/// `--` / `/* */` comments are kept, so those statements aren't torn apart.
///
/// Thin wrapper over [`split_statements_lines`] (one source of truth for the quoting/comment rules);
/// test-only — production uses the lined variant.
#[cfg(test)]
pub(crate) fn split_statements(sql: &str) -> Vec<String> {
    split_statements_lines(sql).into_iter().map(|(s, _)| s).collect()
}

/// 1-based source line of char offset `off` (newlines before it + 1).
fn off_to_line(chars: &[char], off: usize) -> usize {
    1 + chars[..off.min(chars.len())].iter().filter(|c| **c == '\n').count()
}

/// Like [`split_statements`], but also returns each statement's 1-based start line in `sql` — so a
/// per-statement error (execution) can link back to the editor. Same quoting/comment
/// rules; semicolons inside literals / dollar-quotes / comments don't split.
#[allow(unused_assignments)] // the final flush! clears `start`, which is no longer read afterwards
pub(crate) fn split_statements_lines(sql: &str) -> Vec<(String, usize)> {
    let chars: Vec<char> = sql.chars().collect();
    let n = chars.len();
    let mut out: Vec<(String, usize)> = Vec::new();
    let mut cur = String::new();
    let mut start: Option<usize> = None; // char offset of the current statement's first non-ws char
    let mut i = 0;
    macro_rules! flush {
        () => {
            if !cur.trim().is_empty() {
                out.push((cur.trim().to_owned(), off_to_line(&chars, start.unwrap_or(0))));
            }
            cur.clear();
            start = None;
        };
    }
    while i < n {
        let c = chars[i];
        if !c.is_whitespace() && start.is_none() {
            start = Some(i);
        }
        // line comment
        if c == '-' && i + 1 < n && chars[i + 1] == '-' {
            while i < n && chars[i] != '\n' {
                cur.push(chars[i]);
                i += 1;
            }
            continue;
        }
        // block comment
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            cur.push('/');
            cur.push('*');
            i += 2;
            while i < n && !(chars[i] == '*' && i + 1 < n && chars[i + 1] == '/') {
                cur.push(chars[i]);
                i += 1;
            }
            if i < n {
                cur.push('*');
                cur.push('/');
                i += 2;
            }
            continue;
        }
        // single-quoted string (with the '' escape)
        if c == '\'' {
            cur.push(c);
            i += 1;
            while i < n {
                if chars[i] == '\'' {
                    cur.push('\'');
                    i += 1;
                    if i < n && chars[i] == '\'' {
                        cur.push('\'');
                        i += 1;
                        continue;
                    }
                    break;
                }
                cur.push(chars[i]);
                i += 1;
            }
            continue;
        }
        // dollar-quoted block
        if c == '$' {
            // one canonical PG dollar-quote rule (shared with the entity tokenizer)
            if let Some(len) = crate::sqlentity::dollar_tag_len(&chars, i) {
                let tag: String = chars[i..i + len].iter().collect();
                cur.push_str(&tag);
                i += len;
                let tag_chars: Vec<char> = tag.chars().collect();
                let tl = tag_chars.len();
                while i < n {
                    if chars[i] == '$' && i + tl <= n && chars[i..i + tl] == tag_chars[..] {
                        cur.push_str(&tag);
                        i += tl;
                        break;
                    }
                    cur.push(chars[i]);
                    i += 1;
                }
                continue;
            }
        }
        if c == ';' {
            flush!();
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    flush!();
    out
}

/// Run ONE statement on the live client via the simple-query protocol (every value comes back as
/// text — exactly what the grid shows). Returns the outcomes in order so the caller can stream them.
pub(crate) fn run_statement(client: &mut postgres::Client, stmt: &str) -> Vec<SqlOut> {
    use postgres::SimpleQueryMessage;
    let mut out = Vec::new();
    let mut cols: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    match client.simple_query(stmt) {
        Ok(msgs) => {
            for m in msgs {
                match m {
                    SimpleQueryMessage::Row(row) => {
                        if cols.is_empty() {
                            cols = row.columns().iter().map(|c| c.name().to_owned()).collect();
                        }
                        rows.push(
                            (0..cols.len())
                                .map(|i| {
                                    row.get(i)
                                        .map(|s| s.to_owned())
                                        .unwrap_or_else(|| "—".to_owned())
                                })
                                .collect(),
                        );
                    }
                    SimpleQueryMessage::CommandComplete(n) => {
                        if cols.is_empty() {
                            let s = if n == 1 { "" } else { "s" };
                            out.push(SqlOut::Note(format!("{n} row{s} affected")));
                        } else {
                            out.push(SqlOut::Rows(Box::new(crate::ResultSet::new(
                                std::mem::take(&mut cols),
                                std::mem::take(&mut rows),
                            ))));
                        }
                    }
                    _ => {}
                }
            }
        }
        Err(e) => {
            out.push(SqlOut::Note(format!("Error: {}", pg_error_msg(&e))));
        }
    }
    out
}

// ============================================================
// Background statement execution — the tab's worker
// ============================================================
//
// The Api's query path: the UI takes the tab's session connection, hands it (and the split
// statements) to `run_statements_worker` on a background thread, and drains `ExecMsg` back to
// stream result grids, status lines and — finally — the connection itself. The connection is OWNED
// by the worker for the run and RETURNED via `Done` (not shared), which is what lets every tab keep
// its own independent session and run concurrently with the control connection, the scan collector
// and the details fetcher — none of them block each other.

/// Streamed from the background query thread: a data grid per row-returning statement, a one-row
/// status grid per non-row statement (a command outcome or an error with its source line), then
/// `Done` returns the connection so the app can reclaim it.
pub(crate) enum ExecMsg {
    /// Worker has a live client: its cancel token (for Stop) and the backend PID (for the UI's
    /// force-stop `pg_terminate_backend`; 0 when the probe failed).
    Ready { cancel: postgres::CancelToken, pid: i32 },
    Result(crate::ResultSet),
    /// Non-row outcome: a command note (`ok = true`) or an error (`ok = false`), with the tab
    /// label (`Verb_N`) and 1-based source line (click → jump to the editor).
    Status { ok: bool, label: String, line: usize, text: String },
    // ---- lazy stream (the last row-returning SELECT, fetched on demand via COPY TO STDOUT) ----
    /// Open a lazy result grid: columns are known (from `prepare`), no rows yet. The grid appears
    /// at once; rows arrive via `LazyRows` as the user scrolls/fetches.
    LazyBegin { cols: Vec<String>, sql: String, label: String },
    /// A batch of freshly fetched rows to append to the lazy grid.
    LazyRows(Vec<Vec<String>>),
    /// A fetch settled but more rows remain on the server (a page filled / the 100 MB pause /
    /// lightning) — the fetch buttons re-arm. `budget_hit` = the pause was the fetch-to-end byte
    /// budget → the UI warns that fetching further may exhaust memory (Continue / Cancel).
    LazyMore { budget_hit: bool },
    /// The lazy stream ended — fetch buttons go inert. `error = None` is a clean exhaustion; `Some(e)`
    /// means a read error mid-stream, so the shown rows are INCOMPLETE (flag the grid, don't imply done).
    LazyEnd { error: Option<String> },
    Done(Option<Box<postgres::Client>>), // hand the tab's session connection back (None = no client)
}

/// Commands the UI sends to a live lazy-stream worker (the last SELECT of a run, streamed via COPY).
pub(crate) enum FetchCmd {
    /// Pull up to `n` more rows (one screenful) and pause.
    More(usize),
    /// Pull rows until the end OR until ~100 MB of raw data has been appended this call, then pause
    /// (the UI then warns before going further).
    All,
    /// Pull rows to the very end with NO byte budget — the user confirmed the 100 MB warning.
    /// A Stop (lightning) still pauses it.
    Rest,
    /// Close the stream (new run / refresh / tab close / idle timeout): the worker drops the reader
    /// and hands the connection back via `Done`.
    Close,
}

/// Per-row cap when streaming a `LazyRows` batch to the UI (keeps repaints smooth on a big fetch).
const LAZY_BATCH: usize = 256;
/// Raw-data budget for one "fetch to end" call: it pulls at most this many SOURCE bytes (the COPY
/// text as it comes off the wire), then pauses and the UI warns that fetching further may exhaust
/// memory (Continue → [`FetchCmd::Rest`], to the very end / Cancel → stay paused, buttons armed).
const LAZY_BYTE_BUDGET: usize = 100 * 1024 * 1024;
/// Grace after a stream that finished BY ITSELF while our CancelRequest was still in flight: the
/// stray signal gets this long to reach the now-idle backend and be discarded (PG drops cancels
/// arriving between queries) before anything else runs on this session. A cancel consumed by the
/// stream itself (57014) needs no grace.
const CANCEL_GRACE_MS: u64 = 1000;

/// The first alphanumeric word of `s` (the leading SQL keyword), or `""` if it starts oddly.
/// Used by [`copy_eligible`] (the lazy-stream gate).
fn leading_word(s: &str) -> &str {
    s.split(|c: char| !c.is_alphanumeric()).find(|w| !w.is_empty()).unwrap_or_default()
}

/// Unescape one COPY TEXT field. COPY TO emits the backslash sequences `\b \f \n \r \t \v` and `\\`;
/// it never emits octal/hex. (A field equal to `\N` means SQL NULL — handled by the caller.)
fn copy_field(field: &str) -> String {
    if !field.contains('\\') {
        return field.to_owned();
    }
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('b') => out.push('\u{08}'),
            Some('f') => out.push('\u{0C}'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('v') => out.push('\u{0B}'),
            Some('\\') => out.push('\\'),
            Some(other) => out.push(other), // COPY TO won't emit other escapes; pass through verbatim
            None => out.push('\\'),
        }
    }
    out
}

/// Parse one COPY TEXT line into a row of cell strings (TAB-separated; a `\N` field = SQL NULL →
/// the grid's "—" sentinel, matching the buffered path). Padded/truncated to `ncols` for safety.
fn parse_copy_line(line: &str, ncols: usize) -> Vec<String> {
    let mut row: Vec<String> = line
        .split('\t')
        .map(|f| if f == "\\N" { "—".to_owned() } else { copy_field(f) })
        .collect();
    if ncols > 0 && row.len() != ncols {
        row.resize(ncols, String::new());
    }
    row
}

/// Skip leading whitespace and `--` / `/* */` comments so a statement that opens with a comment
/// (common in saved scripts) is still classified by its real first keyword. Comment delimiters and
/// whitespace are ASCII, so the returned slice always starts on a char boundary.
fn strip_leading_noise(s: &str) -> &str {
    let b = s.as_bytes();
    let mut i = 0;
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i + 1 < b.len() && b[i] == b'-' && b[i + 1] == b'-' {
            i += 2;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        break;
    }
    &s[i.min(s.len())..]
}

/// Whether `hay` (already uppercased) contains `word` as a whole word (not part of an identifier).
fn word_present(hay: &str, word: &str) -> bool {
    let bytes = hay.as_bytes();
    let mut from = 0;
    while let Some(rel) = hay[from..].find(word) {
        let i = from + rel;
        let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
        let after_ok = bytes.get(i + word.len()).is_none_or(|&c| !is_ident_byte(c));
        if before_ok && after_ok {
            return true;
        }
        from = i + word.len();
    }
    false
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Whether `stmt` can be streamed lazily via `COPY (...) TO STDOUT`. Conservative: the plain
/// row-returning forms COPY accepts as a subquery — SELECT / VALUES / TABLE, plus WITH (CTE) UNLESS
/// it wraps a data-modifying statement (then COPY would run the INSERT/UPDATE/DELETE/MERGE and a
/// mid-stream cancel could race its commit — route those to the buffered path, which runs them once,
/// atomically, with no cancel). Everything else (DML/DDL/SET/CALL/…) is buffered too; any
/// COPY/prepare failure also falls back.
fn copy_eligible(stmt: &str) -> bool {
    let body = strip_leading_noise(stmt);
    let verb = leading_word(body).to_ascii_uppercase();
    match verb.as_str() {
        "SELECT" | "VALUES" | "TABLE" => true,
        "WITH" => {
            let up = body.to_ascii_uppercase();
            !["INSERT", "UPDATE", "DELETE", "MERGE"].iter().any(|kw| word_present(&up, kw))
        }
        _ => false,
    }
}

/// How a `pump` / `buffer_rows` call ended.
enum PumpEnd {
    /// A pause with more rows remaining on the server. `budget_hit` = it was the fetch-to-end BYTE
    /// budget that stopped it (→ the UI warns about memory before continuing); false = a row page
    /// filled / the initial buffer filled / a lightning pause — no warning.
    Paused { budget_hit: bool },
    Eof,           // the stream is exhausted (clean end — no more rows)
    Error(String), // a read error (server ERROR mid-COPY, network drop) — the result is INCOMPLETE
}

/// Pull rows from the COPY stream until `max_rows` / `max_bytes` is hit, EOF, `stop` is set
/// (lightning pause), or a read error, sending them to the UI in `LazyRows` batches.
fn pump<R: BufRead>(
    buf: &mut R,
    ncols: usize,
    max_rows: usize,
    max_bytes: usize,
    tx: &Sender<ExecMsg>,
    stop: &Arc<AtomicBool>,
) -> PumpEnd {
    let mut batch: Vec<Vec<String>> = Vec::new();
    let mut rows_done = 0usize;
    let mut bytes_done = 0usize;
    let mut line = String::new();
    let end = loop {
        if rows_done >= max_rows || bytes_done >= max_bytes {
            // page filled or the byte budget hit — the flag tells the UI which (only the byte
            // budget triggers the "may exhaust memory" warning)
            break PumpEnd::Paused { budget_hit: bytes_done >= max_bytes };
        }
        if stop.load(Ordering::Relaxed) {
            break PumpEnd::Paused { budget_hit: false }; // lightning
        }
        line.clear();
        match buf.read_line(&mut line) {
            Ok(0) => break PumpEnd::Eof,
            Ok(n) => {
                let trimmed = line.strip_suffix('\n').unwrap_or(&line);
                // the budget counts RAW source bytes as read off the COPY stream — the user-facing
                // "+100 MB of data"
                bytes_done += n;
                batch.push(parse_copy_line(trimmed, ncols));
                rows_done += 1;
                if batch.len() >= LAZY_BATCH
                    && tx.send(ExecMsg::LazyRows(std::mem::take(&mut batch))).is_err()
                {
                    break PumpEnd::Eof; // UI gone — stop (treat as done)
                }
            }
            // a server ERROR mid-COPY or a network drop: the result is INCOMPLETE — surface it, don't
            // pass it off as a clean end
            Err(e) => break PumpEnd::Error(copy_err_text(&e)),
        }
    };
    if !batch.is_empty() {
        let _ = tx.send(ExecMsg::LazyRows(batch));
    }
    end
}

/// Read up to `max_rows` COPY rows INTO MEMORY (EOF, `stop`/lightning, or a read error end it
/// early) — returning the buffered rows and how it ended. Unlike [`pump`] it emits nothing: every
/// statement primes its grid with one of these first, so the tab shows a running spinner during
/// the read instead of dribbling rows in.
fn buffer_rows<R: BufRead>(
    buf: &mut R,
    ncols: usize,
    max_rows: usize,
    stop: &Arc<AtomicBool>,
) -> (Vec<Vec<String>>, PumpEnd) {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut line = String::new();
    let end = loop {
        if rows.len() >= max_rows || stop.load(Ordering::Relaxed) {
            break PumpEnd::Paused { budget_hit: false }; // buffer full / lightning — more remains
        }
        line.clear();
        match buf.read_line(&mut line) {
            Ok(0) => break PumpEnd::Eof,
            Ok(_) => {
                let trimmed = line.strip_suffix('\n').unwrap_or(&line);
                rows.push(parse_copy_line(trimmed, ncols));
            }
            Err(e) => break PumpEnd::Error(copy_err_text(&e)),
        }
    };
    (rows, end)
}

/// The shared `COPY (<stmt>) TO STDOUT` setup for the lazy and head streamers: `prepare` for the
/// column names (a Parse/Describe round-trip — no rows fetched), bail if the statement isn't
/// actually row-returning, then open the COPY reader wrapped in a `BufReader`. Returns `None` (→ the
/// caller runs the buffered fallback) on a prepare / empty-columns / copy_out failure. The body is
/// newline-wrapped so a trailing `-- comment` on the last line can't comment out `) TO STDOUT`.
/// The short DB message from a postgres error (e.g. `canceling statement due to user request`),
/// falling back to the full error string when there is no structured DB error.
fn err_text(e: &postgres::Error) -> String {
    e.as_db_error().map(|d| d.message().to_owned()).unwrap_or_else(|| e.to_string())
}

/// Outcome of starting `COPY (stmt) TO STDOUT`: a ready reader; "not row-returning" (the caller runs
/// the buffered path); or a failure — a prepare/copy error OR a user cancel mid-execution. On failure
/// the caller shows the error rather than re-running the statement on the buffered path (which would
/// restart a slow query the user just cancelled).
enum CopyStart<'c> {
    Ready(Vec<String>, BufReader<postgres::CopyOutReader<'c>>),
    NotRowReturning,
    Failed(String),
}

fn begin_copy<'c>(client: &'c mut postgres::Client, stmt: &str) -> CopyStart<'c> {
    let prep = match client.prepare(stmt) {
        Ok(p) => p,
        Err(e) => return CopyStart::Failed(err_text(&e)),
    };
    let cols: Vec<String> = prep.columns().iter().map(|c| c.name().to_owned()).collect();
    if cols.is_empty() {
        return CopyStart::NotRowReturning;
    }
    let copy_sql = format!("COPY (\n{}\n) TO STDOUT", stmt.trim().trim_end_matches(';').trim());
    match client.copy_out(copy_sql.as_str()) {
        Ok(reader) => CopyStart::Ready(cols, BufReader::new(reader)),
        Err(e) => CopyStart::Failed(err_text(&e)),
    }
}

/// Stream the last row-returning SELECT lazily via `COPY (<stmt>) TO STDOUT`. Like every statement
/// it first buffers up to [`INITIAL_BUFFER_BYTES`] into memory (the tab shows its running spinner
/// meanwhile — no early dribble), shows that whole first block at once, then serves `FetchCmd`s
/// (More / All / Close) on demand for the rest — this one stays LIVE past the initial buffer.
/// `Fallback` if it produced no output (not actually row-returning) so the caller runs the buffered
/// path; `Error` on a prepare/copy failure or a user cancel. Parallelism is preserved — COPY plans
/// with `CURSOR_OPT_PARALLEL_OK`.
fn lazy_copy_stream(
    client: &mut postgres::Client,
    stmt: &str,
    label: &str,
    tx: &Sender<ExecMsg>,
    cmd_rx: &Receiver<FetchCmd>,
    stop: &Arc<AtomicBool>,
    first_page: usize,
) -> Handled {
    let token = client.cancel_token(); // before begin_copy: `buf` borrows the client below
    // columns + COPY reader up front (no rows fetched)
    let (cols, mut buf) = match begin_copy(client, stmt) {
        CopyStart::Ready(c, b) => (c, b),
        CopyStart::NotRowReturning => return Handled::Fallback, // not a grid → buffered fallback
        CopyStart::Failed(e) => {
            // prepare/copy failed OR a user cancel during execution → show the error; do NOT fall
            // back to the buffered path (which would re-run a slow query the user just cancelled)
            let _ = tx.send(ExecMsg::Status { ok: false, label: label.to_owned(), line: 0, text: e });
            return Handled::Error;
        }
    };
    let ncols = cols.len();
    // buffer the first screenful INTO MEMORY before announcing the grid — the tab spins during the
    // read, exactly like the intermediate path, so a slow last statement doesn't fake completion
    let (first_rows, mut end) = buffer_rows(&mut buf, ncols, first_page.max(1), stop);
    if tx
        .send(ExecMsg::LazyBegin { cols, sql: stmt.to_owned(), label: label.to_owned() })
        .is_err()
    {
        // UI gone (tab closed mid-run) — close the stream before returning the client
        return match end_copy_early(&mut buf, &token) {
            EarlyEnd::Dead => Handled::Fatal,
            _ => Handled::Yes,
        };
    }
    if !first_rows.is_empty() && tx.send(ExecMsg::LazyRows(first_rows)).is_err() {
        return match end_copy_early(&mut buf, &token) {
            EarlyEnd::Dead => Handled::Fatal,
            _ => Handled::Yes,
        };
    }
    loop {
        let budget_hit = match end {
            PumpEnd::Eof => {
                let _ = tx.send(ExecMsg::LazyEnd { error: None });
                break;
            }
            PumpEnd::Error(e) => {
                let _ = tx.send(ExecMsg::LazyEnd { error: Some(e) }); // incomplete — flag it, not a clean end
                break;
            }
            PumpEnd::Paused { budget_hit } => budget_hit,
        };
        if tx.send(ExecMsg::LazyMore { budget_hit }).is_err() {
            // UI gone — close the abandoned stream before returning the client
            if let EarlyEnd::Dead = end_copy_early(&mut buf, &token) {
                return Handled::Fatal;
            }
            break;
        }
        match cmd_rx.recv() {
            // More / All / Rest all clear a prior lightning pause — otherwise a fetch after a
            // pause would be a permanent no-op (the stop flag would still be set).
            Ok(FetchCmd::More(n)) => {
                stop.store(false, Ordering::Relaxed);
                end = pump(&mut buf, ncols, n.max(1), usize::MAX, tx, stop);
            }
            Ok(FetchCmd::All) => {
                stop.store(false, Ordering::Relaxed);
                end = pump(&mut buf, ncols, usize::MAX, LAZY_BYTE_BUDGET, tx, stop);
            }
            // the user confirmed the 100 MB warning → to the very end, no budget (Stop still pauses)
            Ok(FetchCmd::Rest) => {
                stop.store(false, Ordering::Relaxed);
                end = pump(&mut buf, ncols, usize::MAX, usize::MAX, tx, stop);
            }
            // Close / UI dropped the sender (a new run / Refresh reclaiming the connection, the
            // editor tab or the results panel closed, idle timeout, disconnect): the COPY may still
            // be mid-stream — close it via `end_copy_early`, which owns the CancelRequest (the UI
            // no longer fires one here): 57014 proves the cancel was consumed by THIS stream; a
            // natural EOF applies the grace so a stray signal can't hit the session's next query.
            Ok(FetchCmd::Close) | Err(_) => {
                if let EarlyEnd::Dead = end_copy_early(&mut buf, &token) {
                    return Handled::Fatal;
                }
                break;
            }
        }
    }
    Handled::Yes
    // buf drops here → the COPY reader drops → the client is usable again for Done
}

/// Drain a COPY reader to its end (discarding rows). Used when abandoning an un-finished stream so
/// the connection resyncs to ReadyForQuery before reuse. With a CancelRequest in flight this ends
/// promptly; on a clean EOF it returns at once.
fn drain_to_end<R: BufRead>(buf: &mut R) {
    let mut sink = String::new();
    loop {
        sink.clear();
        match buf.read_line(&mut sink) {
            Ok(0) => break,  // EOF — COPY done
            Ok(_) => continue,
            Err(_) => break, // cancelled / connection error — stream ended
        }
    }
}

/// The structured Postgres error inside a COPY read error, if any. The reader wraps
/// `postgres::Error` as a custom io error whose Display is an opaque "db error" (no SQLSTATE, no
/// message) — so classification and user-facing text must dig the `DbError` out instead.
fn copy_err_db(e: &std::io::Error) -> Option<&postgres::error::DbError> {
    e.get_ref()?.downcast_ref::<postgres::Error>()?.as_db_error()
}

/// Human text for a COPY read error: the structured DB message when present, else the io text.
fn copy_err_text(e: &std::io::Error) -> String {
    copy_err_db(e).map(|d| d.message().to_owned()).unwrap_or_else(|| e.to_string())
}

/// True when a COPY read error is a query cancellation landing (SQLSTATE 57014) — the expected,
/// clean way an early-closed stream ends: OUR cancel was consumed by OUR statement.
fn is_cancel_err(e: &std::io::Error) -> bool {
    if let Some(db) = copy_err_db(e) {
        return db.code() == &postgres::error::SqlState::QUERY_CANCELED;
    }
    // belt and braces for errors that arrive without a structured DbError
    let text = e.to_string().to_ascii_lowercase();
    text.contains("57014") || text.contains("canceling statement")
}

/// Last-resort kill for the close ladder / the UI force-stop: a short-lived side connection runs
/// `pg_terminate_backend(pid)`. Errors are ignored — if this fails too, the caller's escalation
/// abandons the socket.
pub(crate) fn terminate_backend(params: &ConnParams, pid: i32) {
    if pid <= 0 {
        return;
    }
    if let Ok(mut c) = connect_session_with_timeout(params, 5000) {
        let _ = c.execute("SELECT pg_terminate_backend($1)", &[&pid]);
    }
}

/// How an early-closed COPY stream ended.
enum EarlyEnd {
    Cancelled,    // our CancelRequest landed inside the stream (57014) — consumed, nothing strays
    Eof,          // the stream finished by itself — a stray cancel MAY be in flight (grace applied)
    Dead,      // the connection broke / was terminated — the session is unusable
}

/// Close an abandoned parked COPY stream (the last statement's doscroll source, when a new run /
/// Refresh / tab or panel close / idle timeout / disconnect gives it up): fire ONE CancelRequest
/// and read the stream to its end. Outcomes:
///   * `Cancelled` (57014) — the cancel was provably consumed by THIS stream: clean, no grace;
///   * `Eof` — the stream outran the cancel: sleep [`CANCEL_GRACE_MS`] so the stray signal lands
///     on the idle backend and is discarded, instead of hitting the session's NEXT statement;
///   * `Dead` — the read failed for any other reason: the session is unusable, drop the client.
/// A LOST cancel simply means the drain transfers the tail in full — slower, still correct. A
/// query that ignores cancellation outright is the user-driven Force-stop's territory (the UI
/// escalates to `pg_terminate_backend` there), not this background path's.
fn end_copy_early<R: BufRead>(buf: &mut R, token: &postgres::CancelToken) -> EarlyEnd {
    spawn_cancel(token.clone()); // off-thread: its TCP connect must not stall the drain
    let mut sink = String::new();
    let end = loop {
        sink.clear();
        match buf.read_line(&mut sink) {
            Ok(0) => break EarlyEnd::Eof,
            Ok(_) => continue,
            Err(e) => {
                break if is_cancel_err(&e) {
                    EarlyEnd::Cancelled
                } else {
                    EarlyEnd::Dead
                };
            }
        }
    };
    if matches!(end, EarlyEnd::Eof) {
        std::thread::sleep(Duration::from_millis(CANCEL_GRACE_MS));
    }
    end
}

/// How a statement's handler ended — drives the batch: a failed or cancelled statement stops the
/// rest of the run (later statements don't execute).
enum Handled {
    Yes,      // produced a result / status (success) → run the next statement
    Error,    // failed or cancelled → stop the batch
    Fallback, // not eligible for the COPY path → run the buffered path
    Fatal,    // the session is unusable (terminated / hung close) → stop the batch, DROP the client
}

/// The COPY subselect for a non-last statement, capped server-side: with `LIMIT n` the server
/// stops producing right after the preview — the stream ends by itself, so freeing the connection
/// for the next statement needs no cancels and transfers no tail. This is the ONE place the app
/// adapts the user's SQL, and only inside this COPY — the sheet's `sql` (Refresh) keeps the
/// original text. Newline-wrapped so a trailing `-- comment` can't swallow the closing paren.
fn head_capped_sql(stmt: &str, n: usize) -> String {
    format!(
        "SELECT * FROM (\n{}\n) __jq_head LIMIT {n}",
        stmt.trim().trim_end_matches(';').trim()
    )
}

/// Show a non-last row-returning statement via a server-side-capped COPY ([`head_capped_sql`],
/// `first_page + 1` rows — the +1 only probes "there IS more", so a result of exactly one page
/// isn't falsely marked partial). If the capped form fails to prepare (an exotic-but-valid
/// statement shape) the user's text runs uncapped and the tail is drained instead — the rare
/// slow-but-correct path. A capped grid is flagged `truncated` + `stale`: its fetch buttons stay
/// live as a permanent partial-data indicator, every click explaining a Refresh re-runs it in full.
fn copy_head(
    client: &mut postgres::Client,
    stmt: &str,
    label: &str,
    tx: &Sender<ExecMsg>,
    stop: &Arc<AtomicBool>,
    first_page: usize,
) -> Handled {
    let fp = first_page.max(1);
    let capped_sql = head_capped_sql(stmt, fp + 1);
    // cheap probe: does the capped form prepare? An exotic-but-valid statement shape may reject
    // the subselect wrap — then the user's text runs as-is and the tail is drained instead.
    let use_cap = client.prepare(&capped_sql).is_ok();
    let sql = if use_cap { capped_sql.as_str() } else { stmt };
    let (cols, mut buf) = match begin_copy(client, sql) {
        CopyStart::Ready(c, b) => (c, b),
        CopyStart::NotRowReturning => return Handled::Fallback, // not a grid → buffered fallback
        CopyStart::Failed(e) => {
            // prepare/copy failed OR a user cancel → show the error and stop the batch (don't
            // re-run on the buffered path, and don't run the following statements)
            let _ = tx.send(ExecMsg::Status { ok: false, label: label.to_owned(), line: 0, text: e });
            return Handled::Error;
        }
    };
    let ncols = cols.len();
    let (mut rows, end) = buffer_rows(&mut buf, ncols, fp + 1, stop);
    // a read error — a user cancel (the COPY read is where a slow `pg_sleep`-style statement blocks),
    // a statement runtime error surfacing mid-stream, or a network drop → resync, show the error, and
    // STOP the batch so the following statements don't run. The UI turns the PG "canceling…" text
    // into "Query cancelled".
    if let PumpEnd::Error(e) = end {
        drain_to_end(&mut buf); // best-effort resync past the failed read
        drop(buf);
        let _ = tx.send(ExecMsg::Status { ok: false, label: label.to_owned(), line: 0, text: e });
        return Handled::Error;
    }
    let capped = rows.len() > fp; // the probe row arrived → more rows exist on the server
    rows.truncate(fp);
    // finish the stream: with the LIMIT cap this is an instant EOF (the server already stopped);
    // on the uncapped fallback it transfers the tail (rare path — correctness over speed)
    if matches!(end, PumpEnd::Paused { .. }) {
        drain_to_end(&mut buf);
    }
    drop(buf); // release the borrow on the client before the next statement runs
    let mut rs = crate::ResultSet::new(cols, rows);
    rs.title = label.to_owned();
    rs.sql = stmt.to_owned();
    if capped {
        // more rows remain on the server but this is NOT a live stream — a trailing `…` marks it
        // partial and `stale` routes a fetch click to the "re-run to load the full result" modal.
        rs.truncated = true;
        rs.stale = true;
    }
    if tx.send(ExecMsg::Result(rs)).is_ok() { Handled::Yes } else { Handled::Error }
}

/// Run `statements` on the tab's session connection (opening it lazily on the first run), streaming
/// one [`ExecMsg`] per outcome and handing the still-open connection back in the final `Done`. Runs
/// on a worker thread the UI spawns; the connection is owned (not shared) for the run. The LAST
/// row-returning statement (if a plain SELECT/WITH/VALUES/TABLE) is streamed lazily via COPY —
/// the worker then stays alive serving `cmd_rx` until the stream is closed.
pub(crate) fn run_statements_worker(
    existing: Option<postgres::Client>,
    params: ConnParams,
    statements: Vec<(String, usize)>, // (statement, 1-based source line)
    tx: std::sync::mpsc::Sender<ExecMsg>,
    cmd_rx: Receiver<FetchCmd>,
    stop: Arc<AtomicBool>,
    first_page: usize, // rows the result panel can show — the preview size for EVERY statement
) {
    let mut client = match existing {
        Some(c) => c,
        None => match connect_session(&params) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(ExecMsg::Status {
                    ok: false,
                    label: "Result".to_owned(), // general naming — the red result tab conveys the error
                    line: 0,
                    text: format!("Connection failed: {e}"),
                });
                let _ = tx.send(ExecMsg::Done(None));
                return;
            }
        },
    };
    // the backend PID — the close ladder's / the UI force-stop's pg_terminate_backend target
    // (0 when the probe fails: the terminate step is then skipped)
    let pid: i32 = client.query_one("SELECT pg_backend_pid()", &[]).map(|r| r.get(0)).unwrap_or(0);
    // hand back a cancel token now that we have a live client (enables Stop / idle-timeout cancel)
    let _ = tx.send(ExecMsg::Ready { cancel: client.cancel_token(), pid });
    let n = statements.len();
    let mut session_dead = false; // a Fatal close (terminated / hung) → the client must be dropped
    for (idx, (stmt, line)) in statements.into_iter().enumerate() {
        // tab label = the query's key entity (table after FROM, DML/DDL target, …); see `sqlentity`
        let label = crate::sqlentity::key_entity(&stmt);
        let last = idx + 1 == n;
        // both eligible paths show a first-page preview (the panel's capacity); the LAST one then
        // stays LIVE and streams the rest on demand, a NON-last one is capped server-side
        // (`head_capped_sql`) so its stream ends by itself; else the buffered path
        let flow = if copy_eligible(&stmt) {
            if last {
                lazy_copy_stream(&mut client, &stmt, &label, &tx, &cmd_rx, &stop, first_page)
            } else {
                copy_head(&mut client, &stmt, &label, &tx, &stop, first_page)
            }
        } else {
            Handled::Fallback
        };
        let flow = match flow {
            Handled::Fallback => {
                // buffered path: run to completion and emit the result / status grid
                let outs = run_statement(&mut client, &stmt);
                let mut produced_rows = false;
                let mut is_err = false;
                let mut message = String::new();
                for out in outs {
                    match out {
                        SqlOut::Rows(mut rs) => {
                            rs.sql = stmt.clone(); // remember the source statement (for Refresh)
                            rs.title = label.clone(); // tab keyed by the statement
                            produced_rows = true;
                            if tx.send(ExecMsg::Result(*rs)).is_err() {
                                return; // UI gone
                            }
                        }
                        SqlOut::Note(s) => {
                            if s.starts_with("Error") {
                                is_err = true;
                            }
                            if !message.is_empty() {
                                message.push_str("; ");
                            }
                            // show "Error: …" without the prefix — the status grid is already flagged err
                            message.push_str(s.strip_prefix("Error: ").unwrap_or(&s));
                        }
                    }
                }
                // row-returning statements ARE their result grid; everything else → a status grid. Keep
                // the statement's own label even on error — the red result tab conveys it.
                if (is_err || !produced_rows)
                    && tx
                        .send(ExecMsg::Status { ok: !is_err, label: label.clone(), line, text: message })
                        .is_err()
                {
                    return; // UI gone
                }
                if is_err { Handled::Error } else { Handled::Yes }
            }
            other => other,
        };
        // a failed OR cancelled statement stops the rest of the batch — later statements don't run
        if matches!(flow, Handled::Fatal) {
            session_dead = true;
            break;
        }
        if matches!(flow, Handled::Error) {
            break;
        }
    }
    // hand the (still-open) session connection back to the tab to reuse next time; a session that
    // had to be terminated / hung is dropped instead — the tab's next run opens a fresh one
    let _ = tx.send(ExecMsg::Done(if session_dead {
        drop(client);
        None
    } else {
        Some(Box::new(client))
    }));
}

#[cfg(test)]
mod tests {
    use super::{buffer_rows, copy_eligible, copy_field, parse_copy_line, pump, ExecMsg, PumpEnd};
    use std::io::Cursor;
    use std::sync::atomic::AtomicBool;
    use std::sync::{mpsc, Arc};

    #[test]
    fn head_capped_sql_wraps_and_trims() {
        // the cap is a subselect wrapper: trailing semicolon dropped, newline-wrapped body so a
        // trailing line comment can't swallow the closing paren, LIMIT = page + probe
        assert_eq!(
            super::head_capped_sql("select * from t; ", 11),
            "SELECT * FROM (\nselect * from t\n) __jq_head LIMIT 11"
        );
        let s = super::head_capped_sql("select 1 -- c", 3);
        assert!(s.contains("select 1 -- c\n"), "comment stays on its own line: {s}");
        assert!(s.ends_with("LIMIT 3"));
    }

    #[test]
    fn buffer_rows_caps_at_row_budget() {
        let stop = Arc::new(AtomicBool::new(false));
        // 5 rows in the stream, budget 3 → 3 buffered, paused (more remains, no byte warning)
        let mut buf = Cursor::new("1\n2\n3\n4\n5\n".to_owned());
        let (rows, end) = buffer_rows(&mut buf, 1, 3, &stop);
        assert_eq!(rows.len(), 3);
        assert!(matches!(end, PumpEnd::Paused { budget_hit: false }));
        // the remaining 2 rows fit the budget → a clean EOF
        let (rows, end) = buffer_rows(&mut buf, 1, 3, &stop);
        assert_eq!(rows.len(), 2);
        assert!(matches!(end, PumpEnd::Eof));
    }

    #[test]
    fn pump_flags_byte_budget_pause_but_not_page_fill() {
        let stop = Arc::new(AtomicBool::new(false));
        // 10 raw bytes per row; a 25-byte budget stops after the 3rd row WITH the warning flag
        let (tx, rx) = mpsc::channel();
        let mut buf = Cursor::new("aaaaaaaaa\n".repeat(10));
        let end = pump(&mut buf, 1, usize::MAX, 25, &tx, &stop);
        assert!(matches!(end, PumpEnd::Paused { budget_hit: true }));
        drop(tx);
        let sent: usize =
            rx.iter().map(|m| if let ExecMsg::LazyRows(b) = m { b.len() } else { 0 }).sum();
        assert_eq!(sent, 3);
        // a row-page fill pauses WITHOUT the flag (no warning on a normal page)
        let (tx, _rx) = mpsc::channel();
        let mut buf = Cursor::new("aaaaaaaaa\n".repeat(10));
        let end = pump(&mut buf, 1, 2, usize::MAX, &tx, &stop);
        assert!(matches!(end, PumpEnd::Paused { budget_hit: false }));
    }

    #[test]
    fn copy_field_unescapes_all_sequences() {
        assert_eq!(copy_field("plain"), "plain");
        assert_eq!(copy_field("a\\tb"), "a\tb"); // tab
        assert_eq!(copy_field("a\\nb"), "a\nb"); // newline
        assert_eq!(copy_field("a\\rb"), "a\rb"); // carriage return
        assert_eq!(copy_field("a\\\\b"), "a\\b"); // literal backslash
        assert_eq!(copy_field("a\\bb"), "a\u{08}b"); // backspace
        assert_eq!(copy_field("a\\fb"), "a\u{0C}b"); // form feed
        assert_eq!(copy_field("a\\vb"), "a\u{0B}b"); // vertical tab
    }

    #[test]
    fn parse_copy_line_splits_and_nulls() {
        // a literal `\N` field is SQL NULL → the grid's "—" sentinel; real values pass through
        let row = parse_copy_line("1\t\\N\thello", 3);
        assert_eq!(row, vec!["1".to_owned(), "—".to_owned(), "hello".to_owned()]);
        // an escaped tab inside a value stays one cell
        let row = parse_copy_line("a\\tb\tc", 2);
        assert_eq!(row, vec!["a\tb".to_owned(), "c".to_owned()]);
        // a literal `\N` as data is escaped by COPY as `\\N` → unescapes to "\N" (NOT null)
        let row = parse_copy_line("\\\\N\tx", 2);
        assert_eq!(row, vec!["\\N".to_owned(), "x".to_owned()]);
    }

    #[test]
    fn parse_copy_line_pads_to_ncols() {
        assert_eq!(parse_copy_line("a", 3), vec!["a".to_owned(), String::new(), String::new()]);
    }

    #[test]
    fn copy_eligibility_is_conservative() {
        assert!(copy_eligible("SELECT 1"));
        assert!(copy_eligible("  select * from t"));
        assert!(copy_eligible("WITH x AS (SELECT 1) SELECT * FROM x"));
        assert!(copy_eligible("VALUES (1),(2)"));
        assert!(copy_eligible("TABLE foo"));
        assert!(!copy_eligible("INSERT INTO t VALUES (1)")); // DML without RETURNING → buffered
        assert!(!copy_eligible("UPDATE t SET a=1"));
        assert!(!copy_eligible("CREATE TABLE t (a int)"));
        assert!(!copy_eligible("SET search_path TO x"));
        assert!(!copy_eligible("CALL proc()"));
    }

    #[test]
    fn copy_eligibility_skips_leading_comments() {
        // a leading line/block comment must not hide the real verb (common in saved scripts)
        assert!(copy_eligible("-- a note\nSELECT 1"));
        assert!(copy_eligible("/* header */ SELECT 1"));
        assert!(copy_eligible("\n\n  -- x\n  SELECT 1"));
        assert!(copy_eligible("/* multi\nline */\nWITH x AS (SELECT 1) SELECT * FROM x"));
        // an unterminated block comment leaves no verb → not eligible (buffered fallback)
        assert!(!copy_eligible("/* never closed SELECT 1"));
    }

    #[test]
    fn copy_eligibility_excludes_data_modifying_cte() {
        // a WITH wrapping INSERT/UPDATE/DELETE/MERGE must NOT be COPY-capped (cancel could race commit)
        assert!(!copy_eligible("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d"));
        assert!(!copy_eligible("WITH u AS (UPDATE t SET a=1 RETURNING *) SELECT * FROM u"));
        assert!(!copy_eligible("with i as (insert into t values (1) returning *) select * from i"));
        assert!(!copy_eligible("WITH m AS (MERGE INTO t ... RETURNING *) SELECT * FROM m"));
        // a plain read-only CTE stays eligible; a column merely NAMED like a keyword-substring is fine
        assert!(copy_eligible("WITH inserted_view AS (SELECT 1) SELECT * FROM inserted_view"));
    }
}
