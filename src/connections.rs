//! Saved database connections: the data model, on-disk persistence (passwords encrypted with
//! DPAPI), and the live postgres plumbing (TLS, connect, statement splitting/execution). The
//! dialogs, the Connection Manager dock and the connect/disconnect orchestration live in
//! [`crate::connections_ui`]; the catalog introspection probes in [`crate::catalog`].

use crate::crypt;
use native_tls::TlsConnector;
use postgres::Config;
use postgres_native_tls::MakeTlsConnector;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

/// One saved connection. `id` is the connection's identity — persisted in the store, linking the
/// sidebar list, open editor tabs and the on-disk entry (names may duplicate). `password` is kept
/// in clear in memory but encrypted on disk.
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
    /// alphabetically. Persisted in the connection store so it survives renames.
    pub created: u64,
    // ---- Metadata Manager settings (per-connection, persisted in the connection store) ----
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
            // no preset port: a brand-new connection starts empty (parse_conn backfills 5432
            // when a stored file carries no port= line, so loading is unaffected)
            port: String::new(),
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

/// The connection store: one JSON file holding every saved connection —
/// `%APPDATA%\JustQuery\connections.json`. Each entry carries its own `id` (the connection's
/// identity), so names may duplicate.
fn store_path() -> Option<PathBuf> {
    Some(crate::appdata_dir()?.join("connections.json"))
}

/// The legacy store (one `<name>.conn` file per connection): `%APPDATA%\JustQuery\connections\`.
/// Migrated into the JSON store on first load, then removed.
fn legacy_dir() -> Option<PathBuf> {
    Some(crate::appdata_dir()?.join("connections"))
}

/// Turn a connection name into a safe file stem (the suggested name for an Export `.conn` file).
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
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
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

/// Case-folded comparison key for a connection name, used by the manager's auto-naming ("New
/// connection N" picks the smallest variant no saved connection has, case-insensitively).
pub(crate) fn name_key(name: &str) -> String {
    safe_name(name).to_lowercase()
}

/// DPAPI-encrypt a password into hex for storage; empty stays empty (a missing password is
/// meaningful — it is not a failed encryption). `None` = DPAPI failed: the caller must not
/// persist a silently blanked password.
fn encrypt_pass(p: &str) -> Option<String> {
    if p.is_empty() {
        Some(String::new())
    } else {
        crypt::protect(p.as_bytes()).map(|b| crypt::to_hex(&b))
    }
}

/// Decrypt a stored DPAPI-hex password: blank on any failure (e.g. a value written on another
/// machine/user — the ciphertext is bound to the origin, so the field just clears).
fn decrypt_pass(v: &str) -> String {
    if v.is_empty() {
        String::new()
    } else {
        crypt::from_hex(v)
            .and_then(|b| crypt::unprotect(&b))
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default()
    }
}

/// Parse one `.conn` file body into a `Connection` (the name comes from the file stem, not the
/// body). The inverse of [`conn_to_text`]; used by the legacy-store [`migrate_legacy`] (kept in
/// step with Export, which still writes this format). `created` is taken from the body if
/// present (0 otherwise — the caller backfills).
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
            "password" => c.password = decrypt_pass(v),
            _ => {}
        }
    }
    c
}

/// Serialize a `Connection` to the `.conn` file body (`key=value` lines; password DPAPI-encrypted
/// hex) — the Export format. The inverse of [`parse_conn`]. A password DPAPI can't encrypt is
/// left out (Export strips it anyway).
pub(crate) fn conn_to_text(c: &Connection) -> String {
    let pass = encrypt_pass(&c.password).unwrap_or_default();
    let schemas = schema_list_text(&c.meta_schemas);
    format!(
        "host={}\nport={}\ndatabase={}\nuser={}\npassword={}\ncreated={}\n\
         meta_enabled={}\nmeta_interval={}\nmeta_budget_objects={}\nmeta_idle={}\nmeta_schemas={}\n",
        c.host, c.port, c.db, c.user, pass, c.created,
        c.meta_enabled, c.meta_interval, c.meta_budget, c.meta_idle, schemas
    )
}

/// One entry of the connection store (`connections.json`). A mirror of [`Connection`] whose field
/// names and encodings match the legacy `.conn` body (`database`, `meta_budget_objects`;
/// `password` is DPAPI-encrypted hex; `meta_schemas` is `*` or a comma list).
#[derive(Serialize, Deserialize)]
struct ConnJson {
    id: u64,
    name: String,
    host: String,
    port: String,
    database: String,
    user: String,
    password: String,
    created: u64,
    meta_enabled: bool,
    meta_interval: u64,
    meta_budget_objects: usize,
    meta_idle: u64,
    meta_schemas: String,
}

/// The inverse of [`parse_schema_list`]: `None` (all schemas) → `*`, otherwise a comma list.
fn schema_list_text(schemas: &Option<Vec<String>>) -> String {
    match schemas {
        None => "*".to_owned(),
        Some(list) => list.join(","),
    }
}

impl TryFrom<&Connection> for ConnJson {
    type Error = String;

    /// Fails only when DPAPI can't encrypt the password — persisting the entry anyway would
    /// silently drop the password on the next load.
    fn try_from(c: &Connection) -> Result<Self, String> {
        let password = encrypt_pass(&c.password)
            .ok_or_else(|| format!("Could not encrypt the password of \"{}\".", c.name))?;
        Ok(Self {
            id: c.id,
            name: c.name.clone(),
            host: c.host.clone(),
            port: c.port.clone(),
            database: c.db.clone(),
            user: c.user.clone(),
            password,
            created: c.created,
            meta_enabled: c.meta_enabled,
            meta_interval: c.meta_interval,
            meta_budget_objects: c.meta_budget,
            meta_idle: c.meta_idle,
            meta_schemas: schema_list_text(&c.meta_schemas),
        })
    }
}

impl From<ConnJson> for Connection {
    fn from(j: ConnJson) -> Self {
        Connection {
            id: j.id,
            name: j.name,
            host: j.host,
            port: j.port,
            db: j.database,
            user: j.user,
            password: decrypt_pass(&j.password),
            created: j.created,
            meta_enabled: j.meta_enabled,
            meta_interval: j.meta_interval,
            meta_budget: j.meta_budget_objects,
            meta_idle: j.meta_idle,
            meta_schemas: parse_schema_list(&j.meta_schemas),
        }
    }
}

/// Load every saved connection from the store. The entries keep the ids they were saved with (the
/// id is the identity — names may duplicate). When the store doesn't exist yet but the legacy
/// per-name `.conn` directory does, that directory is migrated: read, written to the store, then
/// removed. A store that fails to parse is set aside as `connections.corrupt.json` and the list
/// starts empty (rather than crash-looping on every launch).
pub fn load() -> Vec<Connection> {
    let Some(path) = store_path() else {
        return Vec::new();
    };
    if let Ok(text) = std::fs::read_to_string(&path) {
        match serde_json::from_str::<Vec<ConnJson>>(&text) {
            Ok(list) => {
                // a live store outranks any legacy leftovers (e.g. a crash between the
                // migration's write and its cleanup)
                remove_legacy_dir();
                return list.into_iter().map(Connection::from).collect();
            }
            Err(_) => {
                let _ = std::fs::rename(&path, path.with_extension("corrupt.json"));
            }
        }
    }
    migrate_legacy()
}

/// One-time migration from the legacy store: every `*.conn` file in the legacy directory is
/// parsed, ids are assigned in creation order (1..=n — these ids persist from now on), the list
/// is written to the store, and the legacy files are removed only after that write.
fn migrate_legacy() -> Vec<Connection> {
    let Some(dir) = legacy_dir() else {
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
    // these ids are the connections' identity from now on — assigned once, here
    for (i, c) in out.iter_mut().enumerate() {
        c.id = (i + 1) as u64;
    }
    // the legacy files go only once the store really holds them — a failed write keeps them for
    // the next launch's retry instead of losing every connection
    if !out.is_empty() && save(&out).is_ok() {
        remove_legacy_dir();
    }
    out
}

/// Best-effort removal of the legacy per-connection directory once the store has taken over.
/// Only `*.conn` files go; the directory itself only if that left it empty (a stray user file
/// keeps it, untouched).
fn remove_legacy_dir() {
    if let Some(dir) = legacy_dir() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("conn") {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
        let _ = std::fs::remove_dir(&dir);
    }
}

/// Persist connections — the whole list as one `connections.json`, written atomically (a crash
/// mid-write keeps the previous store instead of a truncated one that the next launch would set
/// aside as corrupt). Names may duplicate; each entry's `id` is its identity. The error is
/// user-facing text.
pub fn save(conns: &[Connection]) -> Result<(), String> {
    let path = store_path().ok_or("%APPDATA% is not set — connections can't be saved.")?;
    let list = conns
        .iter()
        .map(ConnJson::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let text = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    crate::write_atomic(&path, text.as_bytes())
        .map_err(|e| format!("Could not save connections ({}): {e}", path.display()))
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
        p.parse::<u16>()
            .map_err(|_| format!("Invalid port: \"{port}\""))
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
    Ok(format!(
        "{short}\nTLS: {}",
        if tls_on { "on" } else { "off" }
    ))
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

/// One outcome of a successfully run statement: a result set (rows returned) or a command note
/// (its affected-row count). A failure is the `Err` of [`run_statement`] instead.
pub(crate) enum SqlOut {
    // boxed — `ResultSet` is large (~0.2 KB) and `Note` is a single String, so an unboxed `Rows`
    // would bloat every `SqlOut` in the returned `Vec` to the grid's size (clippy large_enum_variant)
    Rows(Box<crate::ResultSet>),
    Note(String),
}

/// The status note for a completed non-row command: DML reports its affected-row count; DDL /
/// SET / … (whose CommandComplete carries no count) just report success — not a misleading
/// "0 rows affected".
fn command_note(stmt: &str, n: u64) -> String {
    let verb = leading_word(strip_leading_noise(stmt)).to_ascii_uppercase();
    match verb.as_str() {
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "COPY" | "WITH" => {
            let s = if n == 1 { "" } else { "s" };
            format!("{n} row{s} affected")
        }
        _ => "Done".to_owned(),
    }
}

/// Split a SQL script into individual statements on top-level `;`. Semicolons inside single-quoted
/// strings, dollar-quoted blocks (`$$ … $$`, `$tag$ … $tag$` — function bodies / `DO` blocks) and
/// `--` / `/* */` comments are kept, so those statements aren't torn apart.
///
/// Thin wrapper over [`split_statements_lines`] (one source of truth for the quoting/comment rules);
/// test-only — production uses the lined variant.
#[cfg(test)]
pub(crate) fn split_statements(sql: &str) -> Vec<String> {
    split_statements_lines(sql)
        .into_iter()
        .map(|(s, _)| s)
        .collect()
}

/// Like [`split_statements`], but also returns each statement's 1-based start line in `sql` — so a
/// per-statement error (execution) can link back to the editor. Semicolons inside `'…'` / `E'…'`
/// strings, `"…"` identifiers, `$tag$…$tag$` bodies (function bodies / `DO` blocks) and `--` /
/// nested `/* */` comments don't split — the lexing rules are shared with the entity tokenizer
/// ([`crate::sqlentity::skip_opaque`]). One pass: lines are counted incrementally, never re-scanned
/// from the top per statement (that was quadratic on a big script).
pub(crate) fn split_statements_lines(sql: &str) -> Vec<(String, usize)> {
    use crate::sqlentity::{skip_opaque, skip_quoted};
    let cs: Vec<char> = sql.chars().collect();
    let n = cs.len();
    let mut out: Vec<(String, usize)> = Vec::new();
    // newlines before char `counted` → `line`; statement starts only move forward, so the count
    // advances monotonically over the whole script
    let (mut line, mut counted) = (1usize, 0usize);
    let mut push = |from: usize, to: usize, out: &mut Vec<(String, usize)>| {
        let Some(first) = (from..to).find(|&k| !cs[k].is_whitespace()) else {
            return; // blank between two semicolons
        };
        line += cs[counted..first].iter().filter(|&&c| c == '\n').count();
        counted = first;
        let text: String = cs[first..to].iter().collect();
        out.push((text.trim_end().to_owned(), line));
    };
    let mut seg = 0usize; // char offset where the current statement begins
    let mut i = 0usize;
    while i < n {
        if let Some(j) = skip_opaque(&cs, i) {
            i = j;
            continue;
        }
        match cs[i] {
            '"' => i = skip_quoted(&cs, i, '"', false),
            ';' => {
                push(seg, i, &mut out);
                i += 1;
                seg = i;
            }
            _ => i += 1,
        }
    }
    push(seg, n, &mut out);
    out
}

/// Run ONE statement on the live client via the simple-query protocol (every value comes back as
/// text — exactly what the grid shows). Returns the outcomes in order so the caller can stream
/// them; `Err` is the server's error message. A row-returning statement that returns no rows still
/// yields its (empty) grid with headers — `RowDescription` carries the columns.
pub(crate) fn run_statement(
    client: &mut postgres::Client,
    stmt: &str,
) -> Result<Vec<SqlOut>, String> {
    use postgres::SimpleQueryMessage;
    let mut out = Vec::new();
    let mut cols: Option<Vec<String>> = None; // Some once the statement described its row shape
    let mut rows: Vec<crate::Row> = Vec::new();
    for m in client.simple_query(stmt).map_err(|e| pg_error_msg(&e))? {
        match m {
            SimpleQueryMessage::RowDescription(desc) => {
                cols = Some(desc.iter().map(|c| c.name().to_owned()).collect());
            }
            SimpleQueryMessage::Row(row) => {
                let n = cols.get_or_insert_with(|| {
                    row.columns().iter().map(|c| c.name().to_owned()).collect()
                });
                rows.push(crate::Row::from_cells((0..n.len()).map(|i| row.get(i))));
            }
            SimpleQueryMessage::CommandComplete(n) => match cols.take() {
                Some(c) => out.push(SqlOut::Rows(Box::new(crate::ResultSet::new(
                    c,
                    std::mem::take(&mut rows),
                )))),
                None => out.push(SqlOut::Note(command_note(stmt, n))),
            },
            _ => {}
        }
    }
    Ok(out)
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
    Ready {
        cancel: postgres::CancelToken,
        pid: i32,
    },
    Result(crate::ResultSet),
    /// Non-row outcome: a command note (`ok = true`) or an error (`ok = false`), with the tab
    /// label (`Verb_N`) and 1-based source line (click → jump to the editor).
    Status {
        ok: bool,
        label: String,
        line: usize,
        text: String,
    },
    // ---- lazy streams (a parked COPY TO STDOUT, fetched on demand; one per run in session mode —
    // the last row-returning statement — and one per PARKED statement in multisession mode) ----
    /// Open a lazy result grid: columns are known (from `prepare`), no rows yet. The grid appears
    /// at once; rows arrive via `LazyRows` as the user scrolls/fetches. Carries everything the UI
    /// needs to drive THIS stream afterwards: its id (tagging the lazy messages below), the fetch
    /// command channel (worker-created) and the lightning-pause flag.
    LazyBegin {
        stream: u32,
        cols: Vec<String>,
        sql: String,
        label: String,
        cmd: std::sync::mpsc::Sender<FetchCmd>,
        stop: Arc<std::sync::atomic::AtomicBool>,
    },
    /// A batch of freshly fetched rows to append to that stream's lazy grid.
    LazyRows {
        stream: u32,
        rows: Vec<crate::Row>,
    },
    /// A fetch settled but more rows remain on the server (a page filled / the 100 MB pause /
    /// lightning) — that stream's fetch buttons re-arm. `budget_hit` = the pause was the
    /// fetch-to-end byte budget → the UI warns that fetching further may exhaust memory
    /// (Continue / Cancel).
    LazyMore {
        stream: u32,
        budget_hit: bool,
    },
    /// The lazy stream ended — that grid's fetch buttons go inert. `error = None` is a clean
    /// exhaustion; `Some(e)` means a read error mid-stream, so the shown rows are INCOMPLETE
    /// (flag the grid, don't imply done).
    LazyEnd {
        stream: u32,
        error: Option<String>,
    },
    /// Multisession run: the sequential start phase is over — every statement has landed its
    /// grid / error. Parked per-statement streams keep serving on their own threads through this
    /// same channel; no session comes back to the tab (the tab's own session was never used).
    BatchDone,
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
    s.split(|c: char| !c.is_alphanumeric())
        .find(|w| !w.is_empty())
        .unwrap_or_default()
}

/// Unescape one COPY TEXT field onto `out`. COPY TO emits the backslash sequences `\b \f \n \r \t
/// \v` and `\\`; it never emits octal/hex. (A field equal to `\N` means SQL NULL — handled by the
/// caller.)
fn copy_field(out: &mut String, field: &str) {
    if !field.contains('\\') {
        out.push_str(field);
        return;
    }
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
}

/// Parse one COPY TEXT line into a [`crate::Row`] (TAB-separated; a `\N` field = SQL NULL), the
/// fields unescaped straight into the row's one buffer. Padded / truncated to `ncols` for safety.
fn parse_copy_line(line: &str, ncols: usize) -> crate::Row {
    let mut b = crate::result::RowBuilder::default();
    for f in line.split('\t') {
        if ncols > 0 && b.len() == ncols {
            break;
        }
        if f == "\\N" {
            b.push_null();
        } else {
            b.push_with(|out| copy_field(out, f));
        }
    }
    while b.len() < ncols {
        b.push("");
    }
    b.finish()
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
            !["INSERT", "UPDATE", "DELETE", "MERGE"]
                .iter()
                .any(|kw| word_present(&up, kw))
        }
        _ => false,
    }
}

/// How a `pump` / `buffer_rows` call ended.
enum PumpEnd {
    /// A pause with more rows remaining on the server. `budget_hit` = it was the fetch-to-end BYTE
    /// budget that stopped it (→ the UI warns about memory before continuing); false = a row page
    /// filled / the initial buffer filled / a lightning pause — no warning.
    Paused {
        budget_hit: bool,
    },
    Eof,           // the stream is exhausted (clean end — no more rows)
    Error(String), // a read error (server ERROR mid-COPY, network drop) — the result is INCOMPLETE
}

/// Pull rows from the COPY stream until `max_rows` / `max_bytes` is hit, EOF, `stop` is set
/// (lightning pause), or a read error, sending them to the UI in `LazyRows` batches tagged
/// with `stream`.
fn pump<R: BufRead>(
    buf: &mut R,
    ncols: usize,
    stream: u32,
    max_rows: usize,
    max_bytes: usize,
    tx: &Sender<ExecMsg>,
    stop: &Arc<AtomicBool>,
) -> PumpEnd {
    let mut batch: Vec<crate::Row> = Vec::new();
    let mut rows_done = 0usize;
    let mut bytes_done = 0usize;
    let mut line = String::new();
    let end = loop {
        if rows_done >= max_rows || bytes_done >= max_bytes {
            // page filled or the byte budget hit — the flag tells the UI which (only the byte
            // budget triggers the "may exhaust memory" warning)
            break PumpEnd::Paused {
                budget_hit: bytes_done >= max_bytes,
            };
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
                    && tx
                        .send(ExecMsg::LazyRows {
                            stream,
                            rows: std::mem::take(&mut batch),
                        })
                        .is_err()
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
        let _ = tx.send(ExecMsg::LazyRows {
            stream,
            rows: batch,
        });
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
) -> (Vec<crate::Row>, PumpEnd) {
    let mut rows: Vec<crate::Row> = Vec::new();
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
    e.as_db_error()
        .map(|d| d.message().to_owned())
        .unwrap_or_else(|| e.to_string())
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

/// Prepare `stmt` (columns, no rows fetched) and open its COPY reader. `capped` is a server-side
/// capped form to try FIRST ([`copy_head`]): if it doesn't prepare (an exotic-but-valid statement
/// shape rejects the subselect wrap) the user's own text runs instead — one prepare per form, the
/// prepare doubling as the probe.
fn begin_copy<'c>(
    client: &'c mut postgres::Client,
    stmt: &str,
    capped: Option<&str>,
) -> CopyStart<'c> {
    let prepared = capped.and_then(|c| prepare_columns(client, c).ok().map(|cols| (c, cols)));
    let (sql, cols) = match prepared {
        Some(p) => p,
        None => match prepare_columns(client, stmt) {
            Ok(cols) => (stmt, cols),
            Err(e) => return CopyStart::Failed(err_text(&e)),
        },
    };
    if cols.is_empty() {
        return CopyStart::NotRowReturning;
    }
    match open_copy(client, sql) {
        Ok(reader) => CopyStart::Ready(cols, reader),
        Err(e) => CopyStart::Failed(err_text(&e)),
    }
}

/// One statement being run: its text, result-tab label, 1-based source line, and the UI channel.
struct Stmt<'a> {
    sql: &'a str,
    label: &'a str,
    line: usize,
    tx: &'a Sender<ExecMsg>,
}

impl Stmt<'_> {
    /// Report this statement's outcome as its one-row status sheet (linked to its source line).
    /// `false` when the UI is gone.
    fn status(&self, ok: bool, text: String) -> bool {
        self.tx
            .send(ExecMsg::Status {
                ok,
                label: self.label.to_owned(),
                line: self.line,
                text,
            })
            .is_ok()
    }

    /// A finished grid of this statement (Refresh-able: it carries the statement text).
    fn grid(&self, cols: Vec<String>, rows: Vec<crate::Row>) -> crate::ResultSet {
        self.grid_from(crate::ResultSet::new(cols, rows))
    }

    /// Tag a result set as this statement's: its tab label + the text Refresh re-runs.
    fn grid_from(&self, mut rs: crate::ResultSet) -> crate::ResultSet {
        rs.title = self.label.to_owned();
        rs.sql = self.sql.to_owned();
        rs
    }
}

/// The COPY streamers' shared prepare probe: the column names (a Parse/Describe round-trip — no
/// rows fetched). `Err` is the statement's own prepare failure.
fn prepare_columns(
    client: &mut postgres::Client,
    stmt: &str,
) -> Result<Vec<String>, postgres::Error> {
    let prep = client.prepare(stmt)?;
    Ok(prep.columns().iter().map(|c| c.name().to_owned()).collect())
}

/// Open the `COPY (<stmt>) TO STDOUT` reader. The body is newline-wrapped so a trailing
/// `-- comment` on the last line can't comment out `) TO STDOUT`.
fn open_copy<'c>(
    client: &'c mut postgres::Client,
    stmt: &str,
) -> Result<BufReader<postgres::CopyOutReader<'c>>, postgres::Error> {
    let copy_sql = format!(
        "COPY (\n{}\n) TO STDOUT",
        stmt.trim().trim_end_matches(';').trim()
    );
    client.copy_out(copy_sql.as_str()).map(BufReader::new)
}

/// Stream a single-statement run's SELECT lazily via `COPY (<stmt>) TO STDOUT`. Like every
/// statement it first reads its first page (`first_page` rows) into memory (the tab shows its
/// running spinner meanwhile — no early dribble), shows that whole page at once, then serves
/// `FetchCmd`s (More / All / Rest / Close) on demand for the rest — this one stays LIVE past the
/// first page. `Fallback` if it isn't actually row-returning so the caller runs the buffered path;
/// `Error` on a prepare/copy failure or a user cancel. Parallelism is preserved — COPY plans with
/// `CURSOR_OPT_PARALLEL_OK`.
fn lazy_copy_stream(
    client: &mut postgres::Client,
    st: &Stmt,
    stop: &Arc<AtomicBool>,
    first_page: usize,
) -> Handled {
    let tx = st.tx;
    let token = client.cancel_token(); // before begin_copy: `buf` borrows the client below
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<FetchCmd>();
    // columns + COPY reader up front (no rows fetched)
    let (cols, mut buf) = match begin_copy(client, st.sql, None) {
        CopyStart::Ready(c, b) => (c, b),
        CopyStart::NotRowReturning => return Handled::Fallback, // not a grid → buffered fallback
        CopyStart::Failed(e) => {
            // prepare/copy failed OR a user cancel during execution → show the error; do NOT fall
            // back to the buffered path (which would re-run a slow query the user just cancelled)
            st.status(false, e);
            return Handled::Error;
        }
    };
    let ncols = cols.len();
    // buffer the first screenful INTO MEMORY before announcing the grid — the tab spins during the
    // read, exactly like the intermediate path, so a slow last statement doesn't fake completion
    let (first_rows, end) = buffer_rows(&mut buf, ncols, first_page.max(1), stop);
    if tx
        .send(ExecMsg::LazyBegin {
            stream: 0,
            cols,
            sql: st.sql.to_owned(),
            label: st.label.to_owned(),
            cmd: cmd_tx,
            stop: Arc::clone(stop),
        })
        .is_err()
    {
        // UI gone (tab closed mid-run) — close the stream before returning the client
        return match end_copy_early(&mut buf, &token) {
            EarlyEnd::Dead => Handled::Fatal,
            _ => Handled::Yes,
        };
    }
    if !first_rows.is_empty()
        && tx
            .send(ExecMsg::LazyRows {
                stream: 0,
                rows: first_rows,
            })
            .is_err()
    {
        return match end_copy_early(&mut buf, &token) {
            EarlyEnd::Dead => Handled::Fatal,
            _ => Handled::Yes,
        };
    }
    let fatal = serve_lazy(&mut buf, ncols, 0, end, tx, &cmd_rx, stop, &token);
    if fatal {
        Handled::Fatal
    } else {
        Handled::Yes
    }
    // buf drops here → the COPY reader drops → the client is usable again for Done
}

/// Serve `FetchCmd`s for one parked COPY stream after its first page (see [`lazy_copy_stream`]):
/// pump pages on demand (every lazy message tagged with `stream`), report pauses / the end, and
/// own the early-close ladder (`end_copy_early` — a Close or a gone UI abandons the stream).
/// Returns `true` when the connection died during an early close — the session is unusable, the
/// caller must drop the client.
#[allow(clippy::too_many_arguments)] // the stream's full context: reader, shape, id, UI, commands, pause, cancel
fn serve_lazy<R: BufRead>(
    buf: &mut R,
    ncols: usize,
    stream: u32,
    mut end: PumpEnd,
    tx: &Sender<ExecMsg>,
    cmd_rx: &Receiver<FetchCmd>,
    stop: &Arc<AtomicBool>,
    token: &postgres::CancelToken,
) -> bool {
    loop {
        let budget_hit = match end {
            PumpEnd::Eof => {
                let _ = tx.send(ExecMsg::LazyEnd {
                    stream,
                    error: None,
                });
                return false;
            }
            PumpEnd::Error(e) => {
                let _ = tx.send(ExecMsg::LazyEnd {
                    stream,
                    error: Some(e),
                }); // incomplete — flag it, not a clean end
                return false;
            }
            PumpEnd::Paused { budget_hit } => budget_hit,
        };
        if tx.send(ExecMsg::LazyMore { stream, budget_hit }).is_err() {
            // UI gone — close the abandoned stream before returning the client
            return matches!(end_copy_early(buf, token), EarlyEnd::Dead);
        }
        match cmd_rx.recv() {
            // More / All / Rest all clear a prior lightning pause — otherwise a fetch after a
            // pause would be a permanent no-op (the stop flag would still be set).
            Ok(FetchCmd::More(n)) => {
                stop.store(false, Ordering::Relaxed);
                end = pump(buf, ncols, stream, n.max(1), usize::MAX, tx, stop);
            }
            Ok(FetchCmd::All) => {
                stop.store(false, Ordering::Relaxed);
                end = pump(buf, ncols, stream, usize::MAX, LAZY_BYTE_BUDGET, tx, stop);
            }
            // the user confirmed the 100 MB warning → to the very end, no budget (Stop still pauses)
            Ok(FetchCmd::Rest) => {
                stop.store(false, Ordering::Relaxed);
                end = pump(buf, ncols, stream, usize::MAX, usize::MAX, tx, stop);
            }
            // Close / UI dropped the sender (a new run / Refresh reclaiming the connection, the
            // editor tab or the results panel closed, idle timeout, disconnect): the COPY may still
            // be mid-stream — close it via `end_copy_early`, which owns the CancelRequest (the UI
            // no longer fires one here): 57014 proves the cancel was consumed by THIS stream; a
            // natural EOF applies the grace so a stray signal can't hit the session's next query.
            Ok(FetchCmd::Close) | Err(_) => {
                return matches!(end_copy_early(buf, token), EarlyEnd::Dead);
            }
        }
    }
}

/// Drain a COPY reader to its end (discarding rows). Used when abandoning an un-finished stream so
/// the connection resyncs to ReadyForQuery before reuse. With a CancelRequest in flight this ends
/// promptly; on a clean EOF it returns at once.
fn drain_to_end<R: BufRead>(buf: &mut R) {
    let mut sink = String::new();
    loop {
        sink.clear();
        match buf.read_line(&mut sink) {
            Ok(0) => break, // EOF — COPY done
            Ok(_) => continue,
            Err(_) => break, // cancelled / connection error — stream ended
        }
    }
}

/// The structured Postgres error inside a COPY read error, if any. The reader wraps
/// `postgres::Error` as a custom io error whose Display is an opaque "db error" (no SQLSTATE, no
/// message) — so classification and user-facing text must dig the `DbError` out instead.
fn copy_err_db(e: &std::io::Error) -> Option<&postgres::error::DbError> {
    e.get_ref()?
        .downcast_ref::<postgres::Error>()?
        .as_db_error()
}

/// Human text for a COPY read error: the structured DB message when present, else the io text.
fn copy_err_text(e: &std::io::Error) -> String {
    copy_err_db(e)
        .map(|d| d.message().to_owned())
        .unwrap_or_else(|| e.to_string())
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

/// How an early-closed COPY stream ended.
enum EarlyEnd {
    Cancelled, // our CancelRequest landed inside the stream (57014) — consumed, nothing strays
    Eof,       // the stream finished by itself — a stray cancel MAY be in flight (grace applied)
    Dead,      // the connection broke / was terminated — the session is unusable
}

/// Close an abandoned parked COPY stream (the last statement's incremental-fetch source, when a new run /
/// Refresh / tab or panel close / idle timeout / disconnect gives it up): fire ONE CancelRequest
/// and read the stream to its end. Outcomes:
///   * `Cancelled` (57014) — the cancel was provably consumed by THIS stream: clean, no grace;
///   * `Eof` — the stream outran the cancel: sleep [`CANCEL_GRACE_MS`] so the stray signal lands
///     on the idle backend and is discarded, instead of hitting the session's NEXT statement;
///   * `Dead` — the read failed for any other reason: the session is unusable, drop the client.
///
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
    st: &Stmt,
    stop: &Arc<AtomicBool>,
    first_page: usize,
) -> Handled {
    let fp = first_page.max(1);
    let capped_sql = head_capped_sql(st.sql, fp + 1);
    let (cols, mut buf) = match begin_copy(client, st.sql, Some(&capped_sql)) {
        CopyStart::Ready(c, b) => (c, b),
        CopyStart::NotRowReturning => return Handled::Fallback, // not a grid → buffered fallback
        CopyStart::Failed(e) => {
            // prepare/copy failed OR a user cancel → show the error and stop the batch (don't
            // re-run on the buffered path, and don't run the following statements)
            st.status(false, e);
            return Handled::Error;
        }
    };
    let (mut rows, end) = buffer_rows(&mut buf, cols.len(), fp + 1, stop);
    // a read error — a user cancel (the COPY read is where a slow `pg_sleep`-style statement blocks),
    // a statement runtime error surfacing mid-stream, or a network drop → resync, show the error, and
    // STOP the batch so the following statements don't run. The UI turns the PG "canceling…" text
    // into "Query cancelled".
    if let PumpEnd::Error(e) = end {
        drain_to_end(&mut buf); // best-effort resync past the failed read
        drop(buf);
        st.status(false, e);
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
    let mut rs = st.grid(cols, rows);
    if capped {
        // more rows remain on the server but this is NOT a live stream — a trailing `…` marks it
        // partial and `stale` routes a fetch click to the "re-run to load the full result" modal.
        rs.truncated = true;
        rs.stale = true;
    }
    if st.tx.send(ExecMsg::Result(rs)).is_ok() {
        Handled::Yes
    } else {
        Handled::Error
    }
}

/// Run one statement to completion on the buffered path (simple query, autocommit) and emit its
/// grids / status sheets. `Handled::Error` when the statement failed (or the UI is gone) — the
/// batch stops. Shared by both workers.
fn run_buffered(client: &mut postgres::Client, st: &Stmt) -> Handled {
    // row-returning statements ARE their result grid; everything else → a status grid. Keep
    // the statement's own label even on error — the red result tab conveys it.
    let outs = match run_statement(client, st.sql) {
        Ok(outs) => outs,
        Err(e) => {
            st.status(false, e);
            return Handled::Error;
        }
    };
    let mut notes: Vec<String> = Vec::new();
    let mut produced_rows = false;
    for out in outs {
        match out {
            SqlOut::Rows(rs) => {
                produced_rows = true;
                if st.tx.send(ExecMsg::Result(st.grid_from(*rs))).is_err() {
                    return Handled::Error; // UI gone
                }
            }
            SqlOut::Note(n) => notes.push(n),
        }
    }
    // every statement lands exactly one sheet — even one with no outcome at all (a lone comment)
    if !produced_rows && notes.is_empty() {
        notes.push("Done".to_owned());
    }
    if !notes.is_empty() && !st.status(true, notes.join("; ")) {
        return Handled::Error; // UI gone
    }
    Handled::Yes
}

/// Run `statements` on the tab's session connection (opening it lazily on the first run), streaming
/// one [`ExecMsg`] per outcome and handing the still-open connection back in the final `Done`. Runs
/// on a worker thread the UI spawns; the connection is owned (not shared) for the run. A
/// SINGLE-statement run (if a plain SELECT/WITH/VALUES/TABLE) is streamed lazily via COPY — the
/// worker then stays alive serving that stream's fetch commands until it is closed. A
/// multi-statement run caps EVERY statement server-side (`copy_head`): nothing parks on the
/// session, so the connection comes back with `Done` as soon as the batch ends.
pub(crate) fn run_statements_worker(
    existing: Option<postgres::Client>,
    params: ConnParams,
    statements: Vec<(String, usize)>, // (statement, 1-based source line)
    tx: std::sync::mpsc::Sender<ExecMsg>,
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
    let pid: i32 = client
        .query_one("SELECT pg_backend_pid()", &[])
        .map(|r| r.get(0))
        .unwrap_or(0);
    // hand back a cancel token now that we have a live client (enables Stop / idle-timeout cancel)
    let _ = tx.send(ExecMsg::Ready {
        cancel: client.cancel_token(),
        pid,
    });
    // the run's lightning-pause flag + the lazy stream's command channel (created here; the UI
    // learns both from `LazyBegin` and drives the stream through them)
    let stop = Arc::new(AtomicBool::new(false));
    let single = statements.len() == 1; // a single-statement run parks the defetchable COPY stream
    let mut session_dead = false; // a Fatal close (terminated / hung) → the client must be dropped
    for (sql, line) in statements {
        // tab label = the query's key entity (table after FROM, DML/DDL target, …); see `sqlentity`
        let label = crate::sqlentity::key_entity(&sql);
        let st = Stmt {
            sql: &sql,
            label: &label,
            line,
            tx: &tx,
        };
        // both eligible paths show a first-page preview (the panel's capacity); the single-statement
        // run then stays LIVE and streams the rest on demand, while every statement of a
        // multi-statement run is capped server-side (`head_capped_sql`) so its stream ends by
        // itself and the session comes back free; else the buffered path
        let flow = if !copy_eligible(&sql) {
            Handled::Fallback
        } else if single {
            lazy_copy_stream(&mut client, &st, &stop, first_page)
        } else {
            copy_head(&mut client, &st, &stop, first_page)
        };
        let flow = match flow {
            Handled::Fallback => run_buffered(&mut client, &st),
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

// ============================================================
// Multisession execution — one fresh session per statement
// ============================================================

/// The multisession start-phase handshake: what a statement's servant thread tells the coordinator
/// once the statement has landed (its grid / status went out over `tx`).
enum StartOutcome {
    /// The statement produced its outcome — run the next one.
    Ok,
    /// It failed (connect error / execution error / cancel / the UI is gone) — stop the batch.
    Failed,
}

/// One statement's whole life on its OWN fresh session connection (multisession mode). Runs on a
/// thread of its own — owning the session sidesteps the COPY reader borrowing it — and behaves
/// exactly like the session worker's per-statement paths: a row-returning statement opens the lazy
/// COPY stream, lands its first page, tells the coordinator to proceed (the handshake — sequential
/// start), then PARKS: the session + stream stay on this thread serving `serve_lazy` until EOF /
/// Close / error, and the session closes with the thread. A result that ends within the first
/// page, a non-row statement or any error frees the session at once instead.
fn multisession_statement(
    params: ConnParams,
    stmt: String,
    line: usize,
    tx: Sender<ExecMsg>,
    started: Sender<StartOutcome>,
    first_page: usize,
) {
    let label = crate::sqlentity::key_entity(&stmt);
    let st = Stmt {
        sql: &stmt,
        label: &label,
        line,
        tx: &tx,
    };
    let fail = |text: String| {
        st.status(false, text);
        let _ = started.send(StartOutcome::Failed);
    };
    let mut client = match connect_session(&params) {
        Ok(c) => c,
        // a failed connect is an ORDINARY error: shown as the statement's status, batch stops
        Err(e) => return fail(format!("Connection failed: {e}")),
    };
    let pid: i32 = client
        .query_one("SELECT pg_backend_pid()", &[])
        .map(|r| r.get(0))
        .unwrap_or(0);
    // Stop targets whatever statement is executing NOW: every fresh session re-arms the token
    let _ = tx.send(ExecMsg::Ready {
        cancel: client.cancel_token(),
        pid,
    });
    if !copy_eligible(&stmt) {
        // buffered path (DML/DDL/SET/…): runs to completion in autocommit, then the session closes
        let flow = run_buffered(&mut client, &st);
        let _ = started.send(start_outcome(flow));
        return;
    }
    // prepare probe first (like `copy_head`): the columns, and whether this is a grid at all.
    // Kept separate from `open_copy` so the buffered fallback runs with the client free — the
    // COPY reader would borrow it for as long as it lives.
    let cols = match prepare_columns(&mut client, &stmt) {
        Ok(c) => c,
        Err(e) => return fail(err_text(&e)),
    };
    if cols.is_empty() {
        // not a grid after all → the buffered fallback on this same session
        let flow = run_buffered(&mut client, &st);
        let _ = started.send(start_outcome(flow));
        return;
    }
    let ncols = cols.len();
    let token = client.cancel_token(); // before open_copy: `buf` borrows the client below
    let mut buf = match open_copy(&mut client, &stmt) {
        Ok(b) => b,
        // copy failed OR a user cancel during execution → show the error; don't re-run the
        // statement (which would restart a slow query the user just cancelled)
        Err(e) => return fail(err_text(&e)),
    };
    let stop = Arc::new(AtomicBool::new(false)); // this stream's lightning flag
    let (first_rows, end) = buffer_rows(&mut buf, ncols, first_page.max(1), &stop);
    match end {
        // the whole result fit the first page → a complete buffered grid; the stream is over, so
        // the session frees AT ONCE (nothing is parked)
        PumpEnd::Eof => {
            drop(buf); // release the client borrow — the COPY already ended at EOF
            let flow = if tx.send(ExecMsg::Result(st.grid(cols, first_rows))).is_err() {
                Handled::Error
            } else {
                Handled::Yes
            };
            let _ = started.send(start_outcome(flow));
        }
        // a read error — a user cancel (the COPY read is where a slow statement blocks), a runtime
        // error mid-stream or a network drop: resync, show the error, and stop the batch
        PumpEnd::Error(e) => {
            drain_to_end(&mut buf); // best-effort resync past the failed read
            drop(buf);
            fail(e);
        }
        PumpEnd::Paused { .. } => {
            // park: every lazy message from here on is tagged with this statement's stream id
            let sid = next_stream_id();
            let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<FetchCmd>();
            if tx
                .send(ExecMsg::LazyBegin {
                    stream: sid,
                    cols,
                    sql: stmt.clone(),
                    label: label.clone(),
                    cmd: cmd_tx,
                    stop: Arc::clone(&stop),
                })
                .is_err()
            {
                // UI gone — close the never-served stream and stop the batch
                end_copy_early(&mut buf, &token);
                let _ = started.send(StartOutcome::Failed);
                return;
            }
            if !first_rows.is_empty() {
                let _ = tx.send(ExecMsg::LazyRows {
                    stream: sid,
                    rows: first_rows,
                });
            }
            let _ = tx.send(ExecMsg::LazyMore {
                stream: sid,
                budget_hit: false,
            });
            // first page landed — the coordinator may start the next statement NOW; this thread
            // stays parked on the stream until it ends
            let _ = started.send(StartOutcome::Ok);
            let dead = serve_lazy(&mut buf, ncols, sid, end, &tx, &cmd_rx, &stop, &token);
            if dead {
                // the connection broke during the close — the grid is incomplete; say so instead
                // of leaving it parked forever
                let _ = tx.send(ExecMsg::LazyEnd {
                    stream: sid,
                    error: Some("connection lost while closing the stream".to_owned()),
                });
            }
        }
    }
    // whatever remains (the session of a finished statement) drops here when the thread ends
}

/// Map a statement handler's outcome to the coordinator handshake.
fn start_outcome(flow: Handled) -> StartOutcome {
    if matches!(flow, Handled::Error | Handled::Fatal) {
        StartOutcome::Failed
    } else {
        StartOutcome::Ok
    }
}

/// Stream ids for multisession lazy streams: unique across the process (the UI routes fetch
/// commands by id; a run's tab may hold several parked streams at once).
fn next_stream_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1); // 0 is the session worker's single stream
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Multisession run: every statement gets its OWN fresh session connection, so every grid keeps a
/// live defetchable stream — not just the last statement. Statements still START in order, one at
/// a time: the coordinator waits for each servant's handshake (first page landed / statement
/// finished / failed) before launching the next, and a DML statement runs to completion
/// (autocommit) on its session before the next one starts — so read-your-writes within the script
/// is preserved while nothing shares a session (temp tables / SET don't carry over). A failed
/// statement stops the batch, exactly like the session worker; a parked stream is unaffected by
/// what happens to later statements. The tab's own session connection is never used, and nothing
/// is returned to it — these sessions are disposable.
pub(crate) fn run_statements_multisession(
    params: ConnParams,
    statements: Vec<(String, usize)>, // (statement, 1-based source line)
    tx: std::sync::mpsc::Sender<ExecMsg>,
    first_page: usize, // rows the result panel can show — the preview size for EVERY statement
) {
    for (stmt, line) in statements {
        let (started_tx, started_rx) = std::sync::mpsc::channel::<StartOutcome>();
        let servant_tx = tx.clone();
        let servant_params = params.clone();
        let fp = first_page;
        std::thread::spawn(move || {
            multisession_statement(servant_params, stmt, line, servant_tx, started_tx, fp)
        });
        // wait for THIS statement to land before starting the next (sequential start; a DML
        // finishes fully, a SELECT parks after its first page)
        if !matches!(started_rx.recv(), Ok(StartOutcome::Ok)) {
            break; // failed, or the servant died without reporting — later statements don't run
        }
    }
    let _ = tx.send(ExecMsg::BatchDone);
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
        assert!(
            s.contains("select 1 -- c\n"),
            "comment stays on its own line: {s}"
        );
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
        let end = pump(&mut buf, 1, 0, usize::MAX, 25, &tx, &stop);
        assert!(matches!(end, PumpEnd::Paused { budget_hit: true }));
        drop(tx);
        let sent: usize = rx
            .iter()
            .map(|m| {
                if let ExecMsg::LazyRows { rows, .. } = m {
                    rows.len()
                } else {
                    0
                }
            })
            .sum();
        assert_eq!(sent, 3);
        // a row-page fill pauses WITHOUT the flag (no warning on a normal page)
        let (tx, _rx) = mpsc::channel();
        let mut buf = Cursor::new("aaaaaaaaa\n".repeat(10));
        let end = pump(&mut buf, 1, 0, 2, usize::MAX, &tx, &stop);
        assert!(matches!(end, PumpEnd::Paused { budget_hit: false }));
    }

    #[test]
    fn copy_field_unescapes_all_sequences() {
        let unescape = |f: &str| {
            let mut s = String::new();
            copy_field(&mut s, f);
            s
        };
        assert_eq!(unescape("plain"), "plain");
        assert_eq!(unescape("a\\tb"), "a\tb"); // tab
        assert_eq!(unescape("a\\nb"), "a\nb"); // newline
        assert_eq!(unescape("a\\rb"), "a\rb"); // carriage return
        assert_eq!(unescape("a\\\\b"), "a\\b"); // literal backslash
        assert_eq!(unescape("a\\bb"), "a\u{08}b"); // backspace
        assert_eq!(unescape("a\\fb"), "a\u{0C}b"); // form feed
        assert_eq!(unescape("a\\vb"), "a\u{0B}b"); // vertical tab
    }

    #[test]
    fn parse_copy_line_splits_and_nulls() {
        use crate::Row;
        // a literal `\N` field is SQL NULL; real values pass through
        let row = parse_copy_line("1\t\\N\thello", 3);
        assert_eq!(row, Row::from_cells([Some("1"), None, Some("hello")]));
        // an escaped tab inside a value stays one cell
        let row = parse_copy_line("a\\tb\tc", 2);
        assert_eq!(row, Row::from_texts(["a\tb", "c"]));
        // a literal `\N` as data is escaped by COPY as `\\N` → unescapes to "\N" (NOT null)
        let row = parse_copy_line("\\\\N\tx", 2);
        assert_eq!(row, Row::from_texts(["\\N", "x"]));
        // extra fields past the column count are dropped
        assert_eq!(parse_copy_line("a\tb\tc", 2), Row::from_texts(["a", "b"]));
    }

    #[test]
    fn parse_copy_line_pads_to_ncols() {
        assert_eq!(
            parse_copy_line("a", 3),
            crate::Row::from_texts(["a", "", ""])
        );
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
        assert!(copy_eligible(
            "/* multi\nline */\nWITH x AS (SELECT 1) SELECT * FROM x"
        ));
        // an unterminated block comment leaves no verb → not eligible (buffered fallback)
        assert!(!copy_eligible("/* never closed SELECT 1"));
    }

    #[test]
    fn copy_eligibility_excludes_data_modifying_cte() {
        // a WITH wrapping INSERT/UPDATE/DELETE/MERGE must NOT be COPY-capped (cancel could race commit)
        assert!(!copy_eligible(
            "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d"
        ));
        assert!(!copy_eligible(
            "WITH u AS (UPDATE t SET a=1 RETURNING *) SELECT * FROM u"
        ));
        assert!(!copy_eligible(
            "with i as (insert into t values (1) returning *) select * from i"
        ));
        assert!(!copy_eligible(
            "WITH m AS (MERGE INTO t ... RETURNING *) SELECT * FROM m"
        ));
        // a plain read-only CTE stays eligible; a column merely NAMED like a keyword-substring is fine
        assert!(copy_eligible(
            "WITH inserted_view AS (SELECT 1) SELECT * FROM inserted_view"
        ));
    }
}

#[cfg(test)]
mod store_tests {
    use super::{ConnJson, Connection};

    #[test]
    fn conn_store_round_trip() {
        let c = Connection {
            id: 7,
            name: "local «dev»".to_owned(),
            host: "h".to_owned(),
            port: "5433".to_owned(),
            db: "d".to_owned(),
            user: "u".to_owned(),
            password: "s3cret!".to_owned(),
            created: 42,
            meta_enabled: false,
            meta_interval: 11,
            meta_budget: 123,
            meta_idle: 22,
            meta_schemas: Some(vec!["a".to_owned(), "b".to_owned()]),
        };
        let back = Connection::from(ConnJson::try_from(&c).unwrap());
        assert_eq!(back.id, c.id);
        assert_eq!(back.name, c.name);
        assert_eq!(back.host, c.host);
        assert_eq!(back.port, c.port);
        assert_eq!(back.db, c.db);
        assert_eq!(back.user, c.user);
        // DPAPI round-trips on the same machine/user
        assert_eq!(back.password, c.password);
        assert_eq!(back.created, c.created);
        assert_eq!(back.meta_enabled, c.meta_enabled);
        assert_eq!(back.meta_interval, c.meta_interval);
        assert_eq!(back.meta_budget, c.meta_budget);
        assert_eq!(back.meta_idle, c.meta_idle);
        assert_eq!(back.meta_schemas, c.meta_schemas);
    }

    #[test]
    fn conn_store_json_text_round_trip() {
        let c = Connection {
            id: 3,
            name: "n".to_owned(),
            ..Default::default()
        };
        let text = serde_json::to_string(&ConnJson::try_from(&c).unwrap()).unwrap();
        let j: ConnJson = serde_json::from_str(&text).unwrap();
        assert_eq!(Connection::from(j).name, "n");
    }

    #[test]
    fn conn_store_empty_password_stays_empty() {
        let c = Connection::default();
        let j = ConnJson::try_from(&c).unwrap();
        assert_eq!(j.password, "");
        assert_eq!(Connection::from(j).password, "");
    }
}
