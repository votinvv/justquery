//! Saved database connections: the data model, on-disk persistence (passwords encrypted with
//! DPAPI), and the live postgres plumbing (TLS, connect, statement splitting/execution). The
//! dialogs, the Connection Manager dock and the connect/disconnect orchestration live in
//! [`crate::connections_ui`]; the catalog introspection probes in [`crate::catalog`].

use crate::crypt;
use native_tls::TlsConnector;
use postgres::Config;
use postgres_native_tls::MakeTlsConnector;
use std::path::PathBuf;
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
fn safe_name(name: &str) -> String {
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
        let body = format!(
            "host={}\nport={}\ndatabase={}\nuser={}\npassword={}\ncreated={}\n\
             meta_enabled={}\nmeta_interval={}\nmeta_budget_objects={}\nmeta_idle={}\nmeta_schemas={}\n",
            c.host, c.port, c.db, c.user, pass, c.created,
            c.meta_enabled, c.meta_interval, c.meta_budget, c.meta_idle, schemas
        );
        let _ = std::fs::write(dir.join(&fname), body);
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

/// One outcome of running a single statement: a result set (rows returned) or a message line
/// (a command's affected-row count, or an error).
pub(crate) enum SqlOut {
    Rows(crate::ResultSet),
    Note(String),
}

/// Length (in chars) of a dollar-quote tag starting at `i` (`chars[i] == '$'`), i.e. `$tag$` where
/// `tag` is empty or a valid identifier (letters / digits / `_`, not starting with a digit), or
/// `None` if `i` is not the start of one. Counts both delimiting `$`.
fn dollar_tag_len(chars: &[char], i: usize) -> Option<usize> {
    let n = chars.len();
    let mut j = i + 1;
    while j < n && chars[j] != '$' {
        let c = chars[j];
        let ok = c.is_alphanumeric() || c == '_';
        // a tag can't start with a digit (then it's `$1` etc — a parameter placeholder, not a tag)
        if !ok || (j == i + 1 && c.is_ascii_digit()) {
            return None;
        }
        j += 1;
    }
    if j < n {
        Some(j - i + 1)
    } else {
        None
    }
}

/// Split a SQL script into individual statements on top-level `;`. Semicolons inside single-quoted
/// strings, dollar-quoted blocks (`$$ … $$`, `$tag$ … $tag$` — function bodies / `DO` blocks) and
/// `--` / `/* */` comments are kept, so those statements aren't torn apart.
#[allow(dead_code)] // выполнение перешло на split_statements_lines; функция живёт для юнит-тестов
pub(crate) fn split_statements(sql: &str) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        // line comment: copy to end of line
        if c == '-' && i + 1 < n && chars[i + 1] == '-' {
            while i < n && chars[i] != '\n' {
                cur.push(chars[i]);
                i += 1;
            }
            continue;
        }
        // block comment: copy through the closing */
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
                        cur.push('\''); // doubled quote = escaped, stay in the string
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
        // dollar-quoted block: copy through the matching closing tag
        if c == '$' {
            if let Some(len) = dollar_tag_len(&chars, i) {
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
        // top-level statement separator
        if c == ';' {
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_owned());
            }
            cur.clear();
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_owned());
    }
    out
}

/// 1-based source line of char offset `off` (newlines before it + 1).
fn off_to_line(chars: &[char], off: usize) -> usize {
    1 + chars[..off.min(chars.len())].iter().filter(|c| **c == '\n').count()
}

/// Like [`split_statements`], but also returns each statement's 1-based start line in `sql` — so a
/// per-statement error (execution or PREPARE) can link back to the editor. Same quoting/comment
/// rules; semicolons inside literals / dollar-quotes / comments don't split.
#[allow(unused_assignments)] // финальный flush! сбрасывает `start`, который больше не читается
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
            if let Some(len) = dollar_tag_len(&chars, i) {
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

/// PREPARE one statement on the server WITHOUT running it (extended-protocol Parse + Describe) — our
/// "will this execute?" check. `None` = parses cleanly; `Some(msg)` = the server's error (unwrapped).
/// Note: each statement is prepared independently, so a statement that depends on an earlier one's
/// side effects in the same batch (e.g. a temp table created just above) may report a false error.
#[allow(dead_code)] // припаркован вместе с SQL Inspect (кнопки убраны, фича будет переосмыслена)
pub(crate) fn prepare_error(client: &mut postgres::Client, stmt: &str) -> Option<String> {
    match client.prepare(stmt) {
        Ok(_) => None,
        Err(e) => {
            let chain = err_chain(&e);
            let msg = chain.strip_prefix("db error: ").unwrap_or(&chain);
            Some(msg.strip_prefix("ERROR: ").unwrap_or(msg).to_owned())
        }
    }
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
                            out.push(SqlOut::Rows(crate::ResultSet::new(
                                std::mem::take(&mut cols),
                                std::mem::take(&mut rows),
                            )));
                        }
                    }
                    _ => {}
                }
            }
        }
        Err(e) => {
            // strip the redundant wrappers ("db error: ERROR: …" → the actual message) — the
            // grid's red Status column already says it's an error, once is enough
            let chain = err_chain(&e);
            let msg = chain.strip_prefix("db error: ").unwrap_or(&chain);
            let msg = msg.strip_prefix("ERROR: ").unwrap_or(msg);
            out.push(SqlOut::Note(format!("Error: {msg}")));
        }
    }
    out
}
