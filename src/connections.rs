//! Saved database connections: the data model, on-disk persistence (passwords encrypted with
//! DPAPI), the Connect / Connection Manager dialogs, and the live main connection (opened on a
//! background thread, with a "kill in-flight work?" guard around connect / disconnect).

use crate::widgets::{
    close_x, crisp_border, focus_field, manager_row, primary_button, qbtn_off_sm, qbtn_sm,
    secondary_button, select_click, show_modal, style_scrollbar, styled_combo,
};
use crate::theme::p;
use crate::{crypt, ic, theme, PendingConn, JustQueryApp, Tab};
use crate::{CHROME_PAD, SPACE_2, SPACE_3, SPACE_4, SPACE_5, SUBBAR_H, TABBAR_H};
use eframe::egui;
use egui::{Align, Layout, Margin, RichText, CornerRadius, Stroke, Vec2};
use native_tls::TlsConnector;
use postgres::Config;
use postgres_native_tls::MakeTlsConnector;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Directory holding one file per saved connection: `%APPDATA%\JustQuery\connections\`.
fn config_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("JustQuery").join("connections"))
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
fn name_key(name: &str) -> String {
    safe_name(name).to_lowercase()
}

/// Drop a trailing " (N)" suffix so "foo (2)" → "foo" (used to build the next free variant).
fn strip_paren_suffix(name: &str) -> String {
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
fn connect_client(
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
fn make_tls() -> Result<MakeTlsConnector, String> {
    let tls = TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|e| err_chain(&e))?;
    Ok(MakeTlsConnector::new(tls))
}

/// Parse a port string: blank → the Postgres default 5432, otherwise a u16 (or an error).
fn parse_port(port: &str) -> Result<u16, String> {
    let p = port.trim();
    if p.is_empty() {
        Ok(5432)
    } else {
        p.parse::<u16>().map_err(|_| format!("Invalid port: \"{port}\""))
    }
}

fn try_connect(
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
                                        .unwrap_or_else(|| "(null)".to_owned())
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
        Err(e) => out.push(SqlOut::Note(format!("Error: {}", err_chain(&e)))),
    }
    out
}

/// SQL-literal escape for a catalog identifier embedded as a string literal (doubled quotes) —
/// catalog names can contain quotes, so we never interpolate them raw.
fn sql_lit(s: &str) -> String {
    s.replace('\'', "''")
}

/// Schemas for the Metadata Manager dropdown + scan set. Includes the system schemas
/// `pg_catalog` / `information_schema` (lots of objects — useful to browse/test); only the internal
/// `pg_toast*` / `pg_temp*` are excluded (toast tables don't match our relkind filter anyway).
/// Client-taking so the caller (the collector actor) reuses its own persistent connection.
pub(crate) fn list_schemas(client: &mut postgres::Client) -> Result<Vec<String>, String> {
    use postgres::SimpleQueryMessage;
    let sql = "SELECT nspname FROM pg_namespace \
         WHERE nspname NOT LIKE 'pg_toast%' AND nspname NOT LIKE 'pg_temp%' ORDER BY 1";
    let mut out = Vec::new();
    for m in client.simple_query(sql).map_err(|e| err_chain(&e))? {
        if let SimpleQueryMessage::Row(r) = m {
            out.push(r.get(0).unwrap_or("").to_owned());
        }
    }
    Ok(out)
}

/// All objects in one schema as `(type-folder label, name)`. Functions carry their full signature
/// `name(argtypes)` (every overload listed). Per-schema so the collector can budget incrementally.
pub(crate) fn list_objects_in_schema(
    client: &mut postgres::Client,
    schema: &str,
) -> Result<Vec<(String, String)>, String> {
    use postgres::SimpleQueryMessage;
    let sql = format!(
        "SELECT CASE c.relkind WHEN 'r' THEN 'Tables' WHEN 'p' THEN 'Tables' \
                     WHEN 'v' THEN 'Views' WHEN 'm' THEN 'Materialized Views' \
                     WHEN 'S' THEN 'Sequences' WHEN 'f' THEN 'Foreign Tables' END, c.relname \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind IN ('r','p','v','m','S','f') AND n.nspname = '{s}' \
         UNION ALL \
         SELECT 'Functions', p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')' \
         FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname = '{s}' \
         ORDER BY 1, 2",
        s = sql_lit(schema)
    );
    let mut out = Vec::new();
    for m in client.simple_query(&sql).map_err(|e| err_chain(&e))? {
        if let SimpleQueryMessage::Row(r) = m {
            out.push((r.get(0).unwrap_or("").to_owned(), r.get(1).unwrap_or("").to_owned()));
        }
    }
    Ok(out)
}

/// One column as returned by the catalog probes: `(name, type, nullable, default)`.
pub(crate) type ColTuple = (String, String, bool, String);
/// One object with its columns: `(type-folder label, name, columns)`.
pub(crate) type ObjWithCols = (String, String, Vec<ColTuple>);

/// SQL `IN (...)` body listing the given schema names as quoted literals, or `(NULL)` when empty
/// (matches nothing). Used to scope the catalog probes below to the monitored schemas.
fn in_list(schemas: &[String]) -> String {
    if schemas.is_empty() {
        return "(NULL)".to_owned();
    }
    let items: Vec<String> = schemas.iter().map(|s| format!("'{}'", sql_lit(s))).collect();
    format!("({})", items.join(","))
}

/// Per-schema change fingerprint over the *monitored* schemas: one cheap query that md5-hashes the
/// identity + version (`xmin`) of every relation, attribute, default and function in each schema.
/// Because attribute/attrdef rows are folded in, the digest also changes on `ALTER COLUMN`/`ADD
/// COLUMN`/`SET DEFAULT` — not just create/drop. A schema whose objects all vanished simply drops
/// out of the result (callers treat a missing schema as the empty fingerprint).
pub(crate) fn schema_fingerprints(
    client: &mut postgres::Client,
    schemas: &[String],
) -> Result<std::collections::HashMap<String, String>, String> {
    use postgres::SimpleQueryMessage;
    let inl = in_list(schemas);
    let sql = format!(
        "SELECT nspname, md5(string_agg(sig, '|' ORDER BY sig)) FROM ( \
           SELECT n.nspname AS nspname, \
                  'c:'||c.oid::text||':'||c.relkind::text||':'||c.relname||':'||c.xmin::text AS sig \
           FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
           WHERE c.relkind IN ('r','p','v','m','S','f') AND n.nspname IN {inl} \
           UNION ALL \
           SELECT n.nspname, \
                  'a:'||a.attrelid::text||':'||a.attnum::text||':'||a.atttypid::text||':'|| \
                       a.atttypmod::text||':'||a.attnotnull::text||':'||a.xmin::text \
           FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid \
                JOIN pg_namespace n ON n.oid = c.relnamespace \
           WHERE c.relkind IN ('r','p','v','m','f') AND a.attnum > 0 AND NOT a.attisdropped \
                 AND n.nspname IN {inl} \
           UNION ALL \
           SELECT n.nspname, \
                  'd:'||d.adrelid::text||':'||d.adnum::text||':'||d.xmin::text \
           FROM pg_attrdef d JOIN pg_class c ON c.oid = d.adrelid \
                JOIN pg_namespace n ON n.oid = c.relnamespace \
           WHERE n.nspname IN {inl} \
           UNION ALL \
           SELECT n.nspname, \
                  'p:'||p.oid::text||':'||p.proname||':'||p.xmin::text \
           FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
           WHERE n.nspname IN {inl} \
         ) s GROUP BY nspname"
    );
    let mut out = std::collections::HashMap::new();
    for m in client.simple_query(&sql).map_err(|e| err_chain(&e))? {
        if let SimpleQueryMessage::Row(r) = m {
            out.insert(r.get(0).unwrap_or("").to_owned(), r.get(1).unwrap_or("").to_owned());
        }
    }
    Ok(out)
}

/// Total `objects + attributes` held for the monitored schemas: relations + functions (objects)
/// plus every live attribute (columns). This is the figure the collector budgets against; it runs
/// before pulling anything so an oversized catalog can be refused cheaply.
pub(crate) fn count_meta_rows(
    client: &mut postgres::Client,
    schemas: &[String],
) -> Result<usize, String> {
    use postgres::SimpleQueryMessage;
    let inl = in_list(schemas);
    let sql = format!(
        "SELECT \
          (SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
            WHERE c.relkind IN ('r','p','v','m','S','f') AND n.nspname IN {inl}) \
        + (SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
            WHERE n.nspname IN {inl}) \
        + (SELECT count(*) FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid \
            JOIN pg_namespace n ON n.oid = c.relnamespace \
            WHERE c.relkind IN ('r','p','v','m','f') AND a.attnum > 0 AND NOT a.attisdropped \
              AND n.nspname IN {inl})"
    );
    for m in client.simple_query(&sql).map_err(|e| err_chain(&e))? {
        if let SimpleQueryMessage::Row(r) = m {
            return Ok(r.get(0).and_then(|v| v.parse().ok()).unwrap_or(0));
        }
    }
    Ok(0)
}

/// One schema's full object set as `(type-folder label, name, columns)`. Relations carry their
/// columns `(name, type, nullable, default)`; sequences and functions carry an empty column list.
/// Functions list every overload with its signature `name(argtypes)`. This is the heavy per-schema
/// pull the collector runs only for schemas whose fingerprint diverged.
pub(crate) fn scan_schema(
    client: &mut postgres::Client,
    schema: &str,
) -> Result<Vec<ObjWithCols>, String> {
    use postgres::SimpleQueryMessage;
    let lit = sql_lit(schema);
    // 1) the object list (relations + functions), folder + name
    let objs = list_objects_in_schema(client, schema)?;
    // 2) columns for every relation in the schema, in one query, grouped by relation name
    let cols_sql = format!(
        "SELECT c.relname, a.attname, format_type(a.atttypid, a.atttypmod), a.attnotnull, \
                COALESCE(pg_get_expr(d.adbin, d.adrelid), '') \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
              JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
              LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE n.nspname = '{lit}' AND c.relkind IN ('r','p','v','m','f') \
         ORDER BY c.relname, a.attnum"
    );
    let mut cols: std::collections::HashMap<String, Vec<(String, String, bool, String)>> =
        std::collections::HashMap::new();
    for m in client.simple_query(&cols_sql).map_err(|e| err_chain(&e))? {
        if let SimpleQueryMessage::Row(r) = m {
            let notnull = r.get(3) == Some("t");
            cols.entry(r.get(0).unwrap_or("").to_owned()).or_default().push((
                r.get(1).unwrap_or("").to_owned(),
                r.get(2).unwrap_or("").to_owned(),
                !notnull,
                r.get(4).unwrap_or("").to_owned(),
            ));
        }
    }
    // 3) stitch columns onto their relation rows (functions/sequences keep the empty list)
    let out = objs
        .into_iter()
        .map(|(kind, name)| {
            let c = cols.get(&name).cloned().unwrap_or_default();
            (kind, name, c)
        })
        .collect();
    Ok(out)
}

/// A relation's columns as `(name, type, nullable, default)`. `Ok(None)` = the relation does not
/// exist (deleted). Client-taking so the details actor reuses its persistent connection.
pub(crate) fn object_columns(
    client: &mut postgres::Client,
    schema: &str,
    name: &str,
) -> Result<Option<Vec<ColTuple>>, String> {
    use postgres::SimpleQueryMessage;
    let oid_sql = format!(
        "SELECT c.oid FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = '{}' AND c.relname = '{}'",
        sql_lit(schema),
        sql_lit(name)
    );
    let mut found = false;
    for m in client.simple_query(&oid_sql).map_err(|e| err_chain(&e))? {
        if let SimpleQueryMessage::Row(_) = m {
            found = true;
        }
    }
    if !found {
        return Ok(None); // relation gone
    }
    let sql = format!(
        "SELECT a.attname, format_type(a.atttypid, a.atttypmod), a.attnotnull, \
                COALESCE(pg_get_expr(d.adbin, d.adrelid), '') \
         FROM pg_attribute a \
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE a.attrelid = (SELECT c.oid FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                             WHERE n.nspname = '{}' AND c.relname = '{}') \
           AND a.attnum > 0 AND NOT a.attisdropped \
         ORDER BY a.attnum",
        sql_lit(schema),
        sql_lit(name)
    );
    let mut out = Vec::new();
    for m in client.simple_query(&sql).map_err(|e| err_chain(&e))? {
        if let SimpleQueryMessage::Row(r) = m {
            let notnull = r.get(2) == Some("t");
            out.push((
                r.get(0).unwrap_or("").to_owned(),
                r.get(1).unwrap_or("").to_owned(),
                !notnull,
                r.get(3).unwrap_or("").to_owned(),
            ));
        }
    }
    Ok(Some(out))
}

impl JustQueryApp {
    /// Open the Connect dialog (or nudge the user to the manager when nothing is saved).
    pub(crate) fn open_connect(&mut self) {
        if self.connections.is_empty() {
            self.no_conn_open = true;
            return;
        }
        self.connect_sel = self.connect_sel.min(self.connections.len() - 1);
        let c = &self.connections[self.connect_sel];
        self.connect_user = c.user.clone();
        self.connect_pass = c.password.clone();
        self.connect_error = None;
        self.connect_open = true;
    }

    /// Modal "Connect" button: guard against in-flight work, then open the real main connection.
    fn do_connect(&mut self) {
        // connecting (which replaces the main connection) while a query runs or a transaction is
        // open would disrupt it — prompt first
        if !self.busy_tabs().is_empty() {
            self.connect_open = false;
            self.busy_prompt = Some(PendingConn::Connect);
            return;
        }
        self.start_main_connect();
    }

    /// Close the dialog and open the connection on a background thread (the UI is blocked by the
    /// "Connecting…" overlay until it resolves).
    fn start_main_connect(&mut self) {
        let Some(c) = self.connections.get(self.connect_sel).cloned() else {
            return;
        };
        let user = self.connect_user.trim().to_string();
        let pass = self.connect_pass.clone();
        self.pending_label = format!("{}@{}", user, c.db);
        // capture the resolved credentials so each tab can open its own session connection
        self.conn_params = Some(ConnParams {
            host: c.host.clone(),
            port: c.port.clone(),
            db: c.db.clone(),
            user: user.clone(),
            password: pass.clone(),
        });
        // a (re)connect may target a different database — drop any existing tab session connections
        // so they re-open lazily with the new credentials (no tabs are running here: the busy guard
        // in do_connect already prompted, or kill_all cleared them)
        for t in &mut self.tabs {
            t.reset_session();
        }
        // a (re)connect may target a different database → drop any running metadata workers/store
        self.stop_meta_actors();
        // capture this connection's metadata settings + id, applied once the connect succeeds
        self.active_conn_id = Some(c.id);
        self.pending_meta_settings = Some(crate::metadata::CollectorSettings {
            enabled: c.meta_enabled,
            interval: c.meta_interval,
            budget: c.meta_budget,
            idle: c.meta_idle,
            schemas: c.meta_schemas.clone(),
        });
        self.edit_interval = c.meta_interval;
        self.edit_budget = c.meta_budget;
        self.edit_idle = c.meta_idle;
        self.edit_schemas = c.meta_schemas.clone();
        self.connect_error = None;
        // keep the Connect dialog open and show a spinner inside it; success closes it, a failure
        // leaves it open with the error message
        self.connect_open = true;
        let (tx, rx) = std::sync::mpsc::channel();
        self.connect_rx = Some(rx);
        std::thread::spawn(move || {
            let res = match parse_port(&c.port) {
                Ok(p) => connect_client(&c.host, p, &c.db, &user, &pass),
                Err(e) => Err(e),
            };
            let _ = tx.send(res);
        });
    }

    /// Toolbar "Disconnect": same in-flight-work guard, then drop the connection.
    pub(crate) fn do_disconnect(&mut self) {
        if !self.busy_tabs().is_empty() {
            self.busy_prompt = Some(PendingConn::Disconnect);
            return;
        }
        self.disconnect_now();
    }

    /// Send an out-of-band cancel for the running query (a fresh PostgreSQL CancelRequest). The
    /// worker thread still returns the moved client via `ExecMsg::Done`, so the main connection is
    /// preserved — only the in-flight statement is aborted.
    pub(crate) fn cancel_running_query(&mut self) {
        let cancel = self.cur_mut().and_then(|t| t.exec_cancel.take());
        if let Some(cancel) = cancel {
            if let Ok(tls) = make_tls() {
                std::thread::spawn(move || {
                    let _ = cancel.cancel_query(tls);
                });
            }
        }
    }

    /// Drop the control connection and every tab's session connection, clearing all connected
    /// state (including the header label).
    fn disconnect_now(&mut self) {
        self.main_conn = None; // dropping the client closes the control connection
        self.connected = false;
        self.conn_broken = false; // deliberate disconnect → the chip shows nothing, not red
        self.conn_params = None;
        self.active_label.clear();
        // drop every tab's session connection and abandon any in-flight query
        for t in &mut self.tabs {
            t.reset_session();
        }
        self.stop_meta_actors();
        self.active_conn_id = None;
    }

    /// Tabs holding the connection busy — a running query or an open transaction — as
    /// (tab title, reason). Empty until real query execution sets these flags.
    fn busy_tabs(&self) -> Vec<(String, &'static str)> {
        self.tabs
            .iter()
            .filter_map(|t| {
                if t.running {
                    Some((t.title.clone(), "running query"))
                } else if t.tx_open {
                    Some((t.title.clone(), "uncommitted transaction"))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Kill in-flight work so a connect/disconnect can proceed: abandon every tab's running query
    /// (dropping the receiver ends its worker) and drop the session connections.
    fn kill_all(&mut self) {
        for t in &mut self.tabs {
            t.reset_session();
        }
    }

    /// Connect dialog: pick a saved connection, override login/password, connect.
    pub(crate) fn connect_modal(&mut self, ctx: &egui::Context) {
        if !self.connect_open {
            return;
        }
        let mut connect_now = false;
        let r = show_modal(ctx, "connect", 280.0, |ui| {
            // ---- title row: heading + close × ----
            ui.horizontal(|ui| {
                ui.label(RichText::new("Connect").size(16.0).strong().color(p().text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_x(ui, 22.0, 4.0, "Close") {
                        self.connect_open = false;
                    }
                });
            });
            ui.add_space(SPACE_4);

            let connecting = self.connect_rx.is_some();
            // every field/combo shares one width and one left edge
            let w = ui.available_width();

            let failed = self.connect_error.as_deref().map_or(false, |s| !s.is_empty());
            // after a failed attempt the credential fields carry a danger ring until edited
            let danger_ring = |ui: &mut egui::Ui, r: &egui::Response| {
                if failed && !r.has_focus() {
                    crate::widgets::crisp_border_r(
                        ui.painter(),
                        r.rect,
                        p().danger,
                        crate::RADIUS_CONTROL,
                    );
                }
            };

            let mut picked = None;
            crate::widgets::form_row(ui, "Connection", |ui| {
                let names: Vec<String> = self.connections.iter().map(|c| c.name.clone()).collect();
                picked = styled_combo(
                    ui, "conn_pick", w, 13.0, !names.is_empty(), Some(self.connect_sel), &names,
                );
            });
            if let Some(i) = picked {
                let prev = self.connect_sel;
                self.connect_sel = i;
                // switching the picked connection always reloads its saved login/password
                if i != prev {
                    if let Some(c) = self.connections.get(i) {
                        self.connect_user = c.user.clone();
                        self.connect_pass = c.password.clone();
                    }
                }
            }
            crate::widgets::form_row(ui, "Login", |ui| {
                let r = focus_field(ui, &mut self.connect_user, false, w);
                danger_ring(ui, &r);
            });
            crate::widgets::form_row(ui, "Password", |ui| {
                let r = focus_field(ui, &mut self.connect_pass, true, w);
                danger_ring(ui, &r);
            });

            // ---- button bar with the footer-error pattern (Design Delta v2.1 §5): the error is
            // ONE Small/danger line on the LEFT of the button row, ellipsized with the full text
            // on hover — the modal's height never changes when it appears or goes away.
            ui.add_space(SPACE_5 - 14.0); // form_row already left 14px after the last row
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let connect_label = if connecting { "Connecting…" } else { "Connect" };
                if primary_button(ui, connect_label, !connecting) {
                    connect_now = true;
                }
                ui.add_space(SPACE_2);
                if secondary_button(ui, "Cancel", !connecting) {
                    self.connect_open = false;
                }
                if let Some(err) = self.connect_error.clone().filter(|s| !s.is_empty()) {
                    ui.add_space(SPACE_2);
                    let avail = ui.available_width();
                    let one_line = err.replace(['\r', '\n'], " ");
                    let shown = crate::widgets::truncate_to_width(ui, &one_line, 11.0, avail);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::Label::new(
                            RichText::new(shown).color(p().danger).size(11.0),
                        ))
                        .on_hover_text(&err);
                    });
                }
            });
        });
        // modal key contract: Enter = Connect, Esc = Cancel
        if (connect_now || r.enter) && !self.connect_rx.is_some() {
            self.do_connect();
        }
        if r.escape {
            self.connect_open = false;
        }
    }

    /// Confirm-disconnect modal (the plug toggle never disconnects silently). Destructive
    /// primary per Design Delta v2.1 §5; Enter = Disconnect, Esc = Cancel.
    pub(crate) fn disconnect_modal(&mut self, ctx: &egui::Context) {
        if !self.disconnect_confirm {
            return;
        }
        let identity = self
            .conn_params
            .as_ref()
            .map(|cp| format!("{}@{}", cp.user, cp.host))
            .unwrap_or_else(|| self.active_label.clone());
        let mut go = false;
        let r = show_modal(ctx, "disconnect", 320.0, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Disconnect").size(16.0).strong().color(p().text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_x(ui, 22.0, 4.0, "Close") {
                        self.disconnect_confirm = false;
                    }
                });
            });
            ui.add_space(SPACE_3);
            ui.label(RichText::new(format!("Disconnect from {identity}?")).color(p().text_dim));
            ui.add_space(SPACE_5);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if crate::widgets::destructive_button(ui, "Disconnect", true) {
                    go = true;
                }
                ui.add_space(SPACE_2);
                if secondary_button(ui, "Cancel", true) {
                    self.disconnect_confirm = false;
                }
            });
        });
        if go || r.enter {
            self.disconnect_confirm = false;
            self.do_disconnect();
        }
        if r.escape {
            self.disconnect_confirm = false;
        }
    }

    /// "No saved connections" prompt → points the user at the manager.
    pub(crate) fn no_conn_modal(&mut self, ctx: &egui::Context) {
        if !self.no_conn_open {
            return;
        }
        let r = show_modal(ctx, "noconn", 300.0, |ui| {
            ui.label(RichText::new("No connections yet").size(15.0).strong().color(p().text));
            ui.add_space(10.0);
            ui.label(RichText::new("Create one in the Connection Manager first.").color(p().text_dim));
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if primary_button(ui, "Open Manager", true) {
                    self.no_conn_open = false;
                    self.left_panel = Some(crate::LeftPanel::Database);
                }
                ui.add_space(SPACE_2);
                if secondary_button(ui, "Close", true) {
                    self.no_conn_open = false;
                }
            });
        });
        if r.enter {
            // modal key contract: Enter presses the primary action
            self.no_conn_open = false;
            self.left_panel = Some(crate::LeftPanel::Database);
        }
        if r.escape {
            self.no_conn_open = false;
        }
    }

    /// The Connection Manager side panel (toggled from the toolbar): connection list + New /
    /// Delete. Single click selects (Ctrl/Shift multi-select); double-click opens a settings tab.
    pub(crate) fn database_manager_panel(&mut self, ui: &mut egui::Ui) {
        if self.left_panel != Some(crate::LeftPanel::Database) {
            return;
        }
        let mut add = false;
        let mut do_delete = false; // toolbar trash → delete the selected connection(s)
        let mut commit_rename = false;
        let mut cancel_rename = false;
        let mut close_panel = false; // header × closes the dock
        let mut open_cid: Option<u64> = None; // connection to open on a double-click
        // Blank the resize line for this panel's ui (see widgets::hush_resize_line). The dock width
        // is shared by both managers via the common panel id "left_panel".
        let saved_style = crate::widgets::hush_resize_line(ui);
        egui::Panel::left("left_panel")
            .resizable(true)
            .default_size(220.0)
            .size_range(150.0..=460.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin::ZERO))
            .show_inside(ui, |ui| {
                ui.style_mut().visuals.override_text_color = None;
                // header: same height as the tab bar, with a close × on the right
                egui::Panel::top("dbmgr_header")
                    .exact_size(TABBAR_H)
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: 10,
                        right: 6,
                        // shift the header down so its text lines up with the tab labels (which
                        // sit CHROME_PAD/2 lower because their box is inset at the top)
                        top: CHROME_PAD as i8,
                        bottom: 0,
                    }))
                    .show_inside(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.label(RichText::new("Connection Manager").size(13.0).strong().color(p().text));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if close_x(ui, 22.0, 4.0, "Close panel") {
                                    close_panel = true;
                                }
                            });
                        });
                    });
                // work-area toolbar — a chrome strip under the header (same beige as the chrome,
                // no fill or border of its own), holding New "+" and Delete (when rows are selected)
                egui::Panel::top("dbmgr_toolbar")
                    .exact_size(SUBBAR_H)
                    .show_separator_line(false)
                    // top:2 — same seam compensation as the editor toolbar (see editor_toolbar_bar)
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: 6,
                        right: 6,
                        top: 2,
                        bottom: 0,
                    }))
                    .show_inside(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            if qbtn_sm(ui, ic::PLUS, p().text, "New connection").clicked() {
                                add = true;
                            }
                            if self.conn_sel.is_empty() {
                                qbtn_off_sm(ui, ic::DELETE, "Delete (select a connection)");
                            } else if qbtn_sm(ui, ic::DELETE, p().text, "Delete selected").clicked() {
                                do_delete = true;
                            }
                        });
                    });
                // list in a white island — only a left beige strip, so its top edge lines up
                // with the editor sheet (both sit directly under the chrome rows)
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: 6, // 6px left border (screen edge); the editor's 6px left is the seam
                        right: 6, // match the toolbar/header right edge (no overhang)
                        top: 1, // 1px gap matching the editor sheet so their top borders line up
                        bottom: 0,
                    }))
                    .show_inside(ui, |ui| {
                        let ids: Vec<u64> = self.connections.iter().map(|c| c.id).collect();
                        let (ctrl, shift) =
                            ui.input(|i| (i.modifiers.ctrl, i.modifiers.shift));
                        // white work-area island (connection list), vertically scrollable
                        let island = egui::Frame::new()
                            .fill(p().ivory)
                            .corner_radius(egui::CornerRadius::same(crate::RADIUS_ISLAND))
                            .shadow(crate::theme::island_shadow())
                            .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            // keep rows strictly inside the 1px border so scrolled text never
                            // bleeds over it (same guard as the editor sheet)
                            let clip = ui.max_rect().shrink(1.0);
                            ui.set_clip_rect(clip);
                            style_scrollbar(ui);
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 0.0; // tight rows — no gap between connections
                            let conns: Vec<(u64, String)> = self
                                .connections
                                .iter()
                                .map(|c| (c.id, c.name.clone()))
                                .collect();
                            if conns.is_empty() {
                                ui.add_space(6.0);
                                ui.colored_label(p().text_dim, "  No connections.\n  Click + to add.");
                            }
                            for (i, (cid, n)) in conns.iter().enumerate() {
                                let renaming = self.dbmgr_rename == Some(*cid);
                                let selected = self.conn_sel.contains(cid);
                                let label = if renaming {
                                    "" // the inline editor covers the label while renaming
                                } else if n.is_empty() {
                                    "(unnamed)"
                                } else {
                                    n.as_str()
                                };
                                // shared manager row (icon + name); selected/renaming → tint
                                let resp =
                                    manager_row(ui, 0.0, Some(ic::CONNECT), label, selected || renaming);
                                let rect = resp.rect;
                                if renaming {
                                    // inline name editor over the row, drawn in a NON-allocating
                                    // child ui (`new_child`) so it never moves the parent cursor —
                                    // the row keeps its fixed height and the rows below don't jump
                                    // while editing. Bordered field with the accent focus ring.
                                    let edit_rect = egui::Rect::from_min_max(
                                        egui::pos2(rect.left() + 28.0, rect.top() + 1.0),
                                        egui::pos2(rect.right() - 4.0, rect.bottom() - 1.0),
                                    );
                                    let mut fui = ui.new_child(
                                        egui::UiBuilder::new()
                                            .max_rect(edit_rect)
                                            .layout(Layout::left_to_right(Align::Center)),
                                    );
                                    fui.visuals_mut().extreme_bg_color = p().field_bg;
                                    fui.visuals_mut().selection.stroke =
                                        Stroke::new(2.0, p().accent);
                                    {
                                        let w = &mut fui.visuals_mut().widgets;
                                        w.inactive.expansion = 0.0;
                                        w.hovered.expansion = 0.0;
                                        w.active.expansion = 0.0;
                                        w.inactive.bg_stroke = Stroke::new(1.0, p().border_strong);
                                        w.hovered.bg_stroke = Stroke::new(1.0, p().border_strong);
                                    }
                                    let r = fui.add(
                                        egui::TextEdit::singleline(&mut self.dbmgr_rename_buf)
                                            .margin(egui::Margin::symmetric(5, 2))
                                            .desired_width(f32::INFINITY)
                                            .text_color(p().text)
                                            .font(egui::FontId::proportional(13.0)),
                                    );
                                    if self.dbmgr_rename_focus {
                                        r.request_focus();
                                        self.dbmgr_rename_focus = false;
                                    }
                                    // resolve only while no conflict prompt is open
                                    if self.dbmgr_conflict.is_none() {
                                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                            cancel_rename = true;
                                        } else if r.lost_focus() {
                                            commit_rename = true;
                                        }
                                    }
                                } else {
                                    // Double-click opens the settings tab. A plain click selects on
                                    // PRESS (so the previous row's accent drops instantly, not after
                                    // the whole button-hold). A plain click on the already-sole-
                                    // selected row arms a rename on release (Windows-style — fires
                                    // after the double-click window so a real double-click opens).
                                    let new_press = resp.is_pointer_button_down_on()
                                        && self.conn_pressed.map_or(true, |(c, _)| c != *cid);
                                    if resp.double_clicked() {
                                        open_cid = Some(*cid);
                                        self.conn_rename_armed = None;
                                        self.conn_pressed = None;
                                    } else if new_press && !ctrl && !shift {
                                        let was_sole = self.conn_sel.as_slice() == [*cid];
                                        select_click(
                                            &mut self.conn_sel,
                                            &mut self.conn_anchor,
                                            &ids,
                                            i,
                                            false,
                                            false,
                                        );
                                        self.conn_pressed = Some((*cid, was_sole));
                                        ui.ctx().request_repaint();
                                    } else if resp.clicked() {
                                        if ctrl || shift {
                                            select_click(
                                                &mut self.conn_sel,
                                                &mut self.conn_anchor,
                                                &ids,
                                                i,
                                                ctrl,
                                                shift,
                                            );
                                            self.conn_rename_armed = None;
                                        } else if let Some((c, was_sole)) = self.conn_pressed.take() {
                                            // plain click released: arm rename only if it began on
                                            // the already-sole-selected row
                                            self.conn_rename_armed = if c == *cid && was_sole {
                                                Some((*cid, Instant::now()))
                                            } else {
                                                None
                                            };
                                        }
                                        ui.ctx().request_repaint();
                                    }
                                }
                            }
                                });
                        });
                        // crisp 1px border on top of the island
                        crisp_border(ui.painter(), island.response.rect, p().border_strong);
                    });
            });
        ui.set_style(saved_style);
        // forget the held row once the button is up (so a press that ended off-row doesn't block
        // the next press on the same row)
        if !ui.input(|i| i.pointer.primary_down()) {
            self.conn_pressed = None;
        }
        if close_panel {
            self.left_panel = None;
        }
        if do_delete && !self.conn_sel.is_empty() {
            self.confirm = Some(crate::ConfirmAction::DeleteConnections(self.conn_sel.clone()));
        }
        // F2 renames the single selected connection
        if self.dbmgr_rename.is_none()
            && self.conn_sel.len() == 1
            && ui.input(|i| i.key_pressed(egui::Key::F2))
        {
            let id = self.conn_sel[0];
            self.start_conn_rename(id);
        }
        // armed rename fires once the double-click window has passed with no double-click
        if let Some((id, t)) = self.conn_rename_armed {
            if self.dbmgr_rename.is_some() {
                self.conn_rename_armed = None;
            } else if t.elapsed() >= Duration::from_millis(450) {
                self.conn_rename_armed = None;
                self.start_conn_rename(id);
            } else {
                // keep rendering until the window elapses so the rename fires on its own — a single
                // request_repaint_after didn't reliably wake egui from idle (it waited for input)
                ui.ctx().request_repaint();
            }
        }
        if add {
            // add a free-named connection straight to the list and start inline rename (no tab)
            let name = self.next_new_conn_name();
            let id = self.connections.iter().map(|c| c.id).max().unwrap_or(0) + 1;
            self.connections.push(Connection {
                id,
                name: name.clone(),
                port: "5432".into(),
                ..Default::default()
            });
            save(&self.connections);
            self.dbmgr_rename = Some(id);
            self.dbmgr_rename_buf = name;
            self.dbmgr_rename_focus = true;
        }
        if cancel_rename {
            self.dbmgr_rename = None; // keep the connection's current name
        }
        if commit_rename {
            self.finalize_rename();
        }
        // single click opens the connection's settings tab immediately
        if let Some(cid) = open_cid {
            if let Some(c) = self.connections.iter().find(|c| c.id == cid).cloned() {
                self.open_conn_tab(c);
            }
        }
    }

    /// Smallest free "New connection N" name (case-insensitive).
    fn next_new_conn_name(&self) -> String {
        let mut x = 1u32;
        loop {
            let cand = format!("New connection {x}");
            let key = name_key(&cand);
            if !self.connections.iter().any(|c| name_key(&c.name) == key) {
                return cand;
            }
            x += 1;
        }
    }

    /// Windows-style free variant of `name`: "foo" → "foo (2)" → "foo (3)" … (excluding `exclude_id`).
    fn free_variant(&self, name: &str, exclude_id: u64) -> String {
        let stem = strip_paren_suffix(name);
        let mut m = 2u32;
        loop {
            let cand = format!("{stem} ({m})");
            let key = name_key(&cand);
            if !self
                .connections
                .iter()
                .any(|c| c.id != exclude_id && name_key(&c.name) == key)
            {
                return cand;
            }
            m += 1;
        }
    }

    /// Finish inline rename: empty → revert; duplicate → open the conflict prompt; unique → save.
    /// Begin inline-renaming the given connection (loads its name into the edit buffer + focuses).
    fn start_conn_rename(&mut self, id: u64) {
        if let Some(c) = self.connections.iter().find(|c| c.id == id) {
            self.dbmgr_rename = Some(id);
            self.dbmgr_rename_buf = c.name.clone();
            self.dbmgr_rename_focus = true;
            self.conn_rename_armed = None;
        }
    }

    fn finalize_rename(&mut self) {
        let Some(id) = self.dbmgr_rename else {
            return;
        };
        let name = self.dbmgr_rename_buf.trim().to_string();
        if name.is_empty() {
            self.dbmgr_rename = None; // keep the existing (auto) name
            return;
        }
        let key = name_key(&name);
        let dup = self
            .connections
            .iter()
            .any(|c| c.id != id && name_key(&c.name) == key);
        if dup {
            let suggestion = self.free_variant(&name, id);
            self.dbmgr_conflict = Some((id, suggestion)); // stay in rename; prompt resolves it
            return;
        }
        if let Some(c) = self.connections.iter_mut().find(|c| c.id == id) {
            c.name = name.clone();
        }
        self.apply_rename_to_tabs(id, &name);
        save(&self.connections);
        self.dbmgr_rename = None;
    }

    /// Reflect a connection rename in any open settings tab (its Name field + the tab title).
    fn apply_rename_to_tabs(&mut self, id: u64, name: &str) {
        for t in &mut self.tabs {
            if let Some(c) = t.conn.as_mut() {
                if c.id == id {
                    c.name = name.to_owned();
                    t.title = name.to_owned();
                }
            }
        }
    }

    /// Duplicate-name prompt during inline rename (Windows-style "(2)" suggestion).
    pub(crate) fn conflict_modal(&mut self, ctx: &egui::Context) {
        let Some((id, suggestion)) = self.dbmgr_conflict.clone() else {
            return;
        };
        let taken = self.dbmgr_rename_buf.trim().to_string();
        let mut do_rename = false;
        let mut keep_editing = false;
        let r = show_modal(ctx, "conflict", 330.0, |ui| {
            ui.label(RichText::new("Name already in use").size(15.0).strong().color(p().text));
            ui.add_space(10.0);
            ui.label(
                RichText::new(format!(
                    "A connection named \"{taken}\" already exists. Rename it to \"{suggestion}\"?"
                ))
                .color(p().text_dim),
            );
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if primary_button(ui, "Rename", true) {
                    do_rename = true;
                }
                ui.add_space(SPACE_2);
                if secondary_button(ui, "Keep editing", true) {
                    keep_editing = true;
                }
            });
        });
        if r.enter {
            do_rename = true; // modal key contract: Enter presses the primary action
        }
        if r.escape {
            keep_editing = true;
        }
        if do_rename {
            if self.dbmgr_rename == Some(id) {
                // inline rename in the manager list
                if let Some(c) = self.connections.iter_mut().find(|c| c.id == id) {
                    c.name = suggestion.clone();
                }
                self.apply_rename_to_tabs(id, &suggestion);
                save(&self.connections);
                self.dbmgr_rename = None;
            } else {
                // rename came from the settings-tab Save — take the name and commit all fields
                if let Some(t) = self.cur_mut() {
                    if let Some(c) = t.conn.as_mut() {
                        c.name = suggestion.clone();
                    }
                }
                self.commit_conn_tab();
                save(&self.connections);
            }
            self.dbmgr_conflict = None;
        }
        if keep_editing {
            self.dbmgr_conflict = None;
            self.dbmgr_rename_focus = true; // back to editing the field
        }
    }

    /// Open (or focus) an editor tab that edits the given connection.
    pub(crate) fn open_conn_tab(&mut self, conn: Connection) {
        if conn.id != 0 {
            if let Some(i) = self
                .tabs
                .iter()
                .position(|t| t.conn.as_ref().map_or(false, |c| c.id == conn.id))
            {
                self.active_tab = i;
                self.focus_editor = true;
                return;
            }
        }
        let is_new = conn.id == 0;
        let title = if conn.name.is_empty() {
            "New connection".to_owned()
        } else {
            conn.name.clone()
        };
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id, title);
        tab.conn = Some(conn);
        tab.dirty = is_new; // a brand-new connection is unsaved
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        self.focus_editor = true;
    }

    /// Save the active connection tab: validate (name required + unique), commit, persist to disk.
    /// One connection == one file named after it, so duplicate names are rejected here.
    pub(crate) fn save_conn_tab(&mut self) {
        let idx = self.active_tab;
        let Some(conn) = self.tabs.get(idx).and_then(|t| t.conn.clone()) else {
            return;
        };
        let name = conn.name.trim().to_string();
        if name.is_empty() {
            self.error_modal = Some("Connection name is required.".to_owned());
            return;
        }
        if conn.host.trim().is_empty() || conn.port.trim().is_empty() || conn.db.trim().is_empty() {
            self.error_modal = Some("Host, port and database are required.".to_owned());
            return;
        }
        let duplicate = self
            .connections
            .iter()
            .any(|c| c.id != conn.id && name_key(&c.name) == name_key(&name));
        if duplicate {
            // same prompt as the inline rename: offer a free "(2)" variant
            let suggestion = self.free_variant(&name, conn.id);
            self.dbmgr_rename_buf = name.clone(); // shown as the "taken" name in the prompt
            self.dbmgr_conflict = Some((conn.id, suggestion));
            return;
        }
        self.commit_conn_tab();
        save(&self.connections);
    }

    /// In-memory part of saving a connection tab (no disk) — assign an id for a new connection,
    /// update the saved list, retitle the tab and clear its dirty flag.
    pub(crate) fn commit_conn_tab(&mut self) {
        let idx = self.active_tab;
        let Some(mut conn) = self.tabs.get(idx).and_then(|t| t.conn.clone()) else {
            return;
        };
        if let Some(existing) = self
            .connections
            .iter_mut()
            .find(|c| c.id != 0 && c.id == conn.id)
        {
            *existing = conn.clone();
        } else {
            conn.id = self.connections.iter().map(|c| c.id).max().unwrap_or(0) + 1;
            // stamp creation order so it sorts after existing connections (and persists)
            if conn.created == 0 {
                conn.created = now_ms().max(
                    self.connections.iter().map(|c| c.created).max().unwrap_or(0) + 1,
                );
            }
            self.connections.push(conn.clone());
        }
        let title = if conn.name.is_empty() {
            "Connection".to_owned()
        } else {
            conn.name.clone()
        };
        if let Some(t) = self.tabs.get_mut(idx) {
            t.conn = Some(conn);
            t.title = title;
            t.dirty = false;
        }
    }

    /// Kick off a real Test Connection for the given tab on a background thread.
    pub(crate) fn start_conn_test(&mut self, idx: usize) {
        let Some(c) = self.tabs.get(idx).and_then(|t| t.conn.clone()) else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.test_rx = Some(rx);
        self.test_result = None;
        std::thread::spawn(move || {
            let res = match parse_port(&c.port) {
                Ok(p) => try_connect(&c.host, p, &c.db, &c.user, &c.password),
                Err(e) => Err(e),
            };
            let _ = tx.send(res);
        });
    }

    /// Delete a connection: drop it from the list, prune its file, and close any open settings tab.
    pub(crate) fn delete_connection(&mut self, id: u64) {
        self.connections.retain(|c| c.id != id);
        save(&self.connections); // rewrites the dir and prunes the now-orphaned file
        let idxs: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.conn.as_ref().map_or(false, |c| c.id == id))
            .map(|(i, _)| i)
            .collect();
        for i in idxs.into_iter().rev() {
            self.close_tab(i);
        }
    }

    /// "Testing…" spinner while a Test Connection is in flight, then a success/failure result modal.
    pub(crate) fn conn_test_modal(&mut self, ctx: &egui::Context) {
        if self.test_rx.is_some() {
            show_modal(ctx, "test", 260.0, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.add_space(8.0);
                    ui.label(RichText::new("Testing connection…").color(p().text));
                });
            });
            ctx.request_repaint();
            return;
        }
        let Some(res) = self.test_result.clone() else {
            return;
        };
        let mut close = false;
        let r = show_modal(ctx, "test", 400.0, |ui| {
            match &res {
                Ok(msg) => {
                    ui.label(RichText::new("Connection successful").size(15.0).strong().color(p().ok));
                    ui.add_space(8.0);
                    ui.label(RichText::new(msg).color(p().text_dim));
                }
                Err(msg) => {
                    ui.label(RichText::new("Connection failed").size(15.0).strong().color(p().danger));
                    ui.add_space(8.0);
                    ui.label(RichText::new(msg).color(p().text_dim));
                }
            }
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if primary_button(ui, "OK", true) {
                    close = true;
                }
            });
        });
        if r.escape || r.enter {
            close = true; // single-button modal: Enter and Esc both dismiss
        }
        if close {
            self.test_result = None;
        }
    }

    /// Generic operation-error modal (errors go here instead of the status bar).
    pub(crate) fn error_modal_box(&mut self, ctx: &egui::Context) {
        let Some(msg) = self.error_modal.clone() else {
            return;
        };
        let mut close = false;
        let r = show_modal(ctx, "err", 360.0, |ui| {
            ui.label(RichText::new("Error").size(15.0).strong().color(p().danger));
            ui.add_space(8.0);
            ui.label(RichText::new(msg).color(p().text));
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if primary_button(ui, "OK", true) {
                    close = true;
                }
            });
        });
        if r.escape || r.enter {
            close = true; // single-button modal: Enter and Esc both dismiss
        }
        if close {
            self.error_modal = None;
        }
    }

    /// "Work in progress" prompt: shown when a connect/disconnect would interrupt a running query
    /// or an open transaction. Lists the affected tabs and offers Go back or Kill & proceed.
    pub(crate) fn busy_modal(&mut self, ctx: &egui::Context) {
        let Some(action) = self.busy_prompt else {
            return;
        };
        let busy = self.busy_tabs();
        if busy.is_empty() {
            self.busy_prompt = None; // nothing busy anymore — nothing to prompt about
            return;
        }
        let verb = match action {
            PendingConn::Connect => "connect",
            PendingConn::Disconnect => "disconnect",
        };
        let mut go_back = false;
        let mut kill = false;
        let r = show_modal(ctx, "busy", 380.0, |ui| {
            ui.label(RichText::new("Work in progress").size(15.0).strong().color(p().text));
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("Some tabs are still busy — {verb}ing will interrupt them:"))
                    .color(p().text_dim),
            );
            ui.add_space(8.0);
            for (name, reason) in &busy {
                ui.label(RichText::new(format!("•  {name} — {reason}")).color(p().text));
            }
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if primary_button(ui, &format!("Kill & {verb}"), true) {
                    kill = true;
                }
                ui.add_space(SPACE_2);
                if secondary_button(ui, "Go back", true)
                {
                    go_back = true;
                }
            });
        });
        if r.enter {
            kill = true; // modal key contract: Enter presses the primary action
        }
        if r.escape {
            go_back = true;
        }
        if go_back {
            self.busy_prompt = None;
        } else if kill {
            self.busy_prompt = None;
            self.kill_all();
            match action {
                PendingConn::Connect => self.start_main_connect(),
                PendingConn::Disconnect => self.disconnect_now(),
            }
        }
    }

    /// Blocking "Connecting…" overlay while the main connection is being opened on the bg thread.
    pub(crate) fn connecting_modal(&mut self, ctx: &egui::Context) {
        // while the Connect dialog is open the spinner lives inside it; this standalone overlay is
        // only for connects started without the dialog (e.g. a busy-prompt "Kill & connect")
        if self.connect_rx.is_none() || self.connect_open {
            return;
        }
        show_modal(ctx, "connecting", 240.0, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.add_space(8.0);
                ui.label(RichText::new("Connecting…").color(p().text));
            });
        });
        ctx.request_repaint();
    }

    /// Render the connection-settings form for the active tab: label/field rows on the data sheet.
    pub(crate) fn connection_tab(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                left: 6,
                right: 6,
                top: 1, // 1px gap so the active-tab underline isn't flush against the sheet
                bottom: 0,
            }))
            .show_inside(ui, |ui| {
                // silvery data sheet inside the side borders, with a thin border of its own
                let sheet = ui.max_rect();
                crate::widgets::island_shadow_under(ui.painter(), sheet);
                ui.painter().rect_filled(sheet, CornerRadius::same(crate::RADIUS_ISLAND), p().data_bg);
                crisp_border(ui.painter(), sheet, p().border_strong);
                let idx = self.active_tab.min(self.tabs.len().saturating_sub(1));
                let mut changed = false;
                let mut test = false;
                let mut do_save = false;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(18, 16))
                        .show(ui, |ui| {
                            theme::style_modal_widgets(ui); // fields/buttons use the shared border
                            let mut field_right = None;
                            if let Some(c) = self.tabs.get_mut(idx).and_then(|t| t.conn.as_mut()) {
                                egui::Grid::new("conn_form")
                                    .num_columns(2)
                                    .spacing([12.0, 8.0])
                                    .min_col_width(64.0)
                                    .show(ui, |ui| {
                                        let mut row =
                                            |label: &str, v: &mut String, pw: bool, editable: bool| {
                                                ui.label(
                                                    RichText::new(label).color(p().text_dim).size(12.0),
                                                );
                                                let mut te = egui::TextEdit::singleline(v)
                                                    .desired_width(280.0);
                                                if pw {
                                                    te = te.password(true);
                                                }
                                                if !editable {
                                                    // dim it so it visibly reads as "locked / not editable"
                                                    te = te.interactive(false).text_color(p().text_dim);
                                                }
                                                let r = ui.add(te);
                                                // track the fields' right edge so the action
                                                // buttons below can line up with it exactly
                                                let right = r.rect.right();
                                                field_right = Some(
                                                    field_right.map_or(right, |v: f32| v.max(right)),
                                                );
                                                if r.changed() {
                                                    changed = true;
                                                }
                                                ui.end_row();
                                            };
                                        // Name is editable; Save validates uniqueness (duplicate
                                        // → conflict prompt) and renames the backing file
                                        row("Name", &mut c.name, false, true);
                                        row("Host", &mut c.host, false, true);
                                        row("Port", &mut c.port, false, true);
                                        row("Database", &mut c.db, false, true);
                                        row("User", &mut c.user, false, true);
                                        row("Password", &mut c.password, true, true);
                                    });
                            }
                            // Save is available whenever the required fields (Name + host/port/db)
                            // are filled — so an opened connection can be re-saved without first
                            // having to make a throwaway edit. (Re-saving is idempotent.)
                            let _ = changed;
                            let can_save = self.tabs.get(idx).and_then(|t| t.conn.as_ref()).map_or(
                                false,
                                |c| {
                                    !c.name.trim().is_empty()
                                        && !c.host.trim().is_empty()
                                        && !c.port.trim().is_empty()
                                        && !c.db.trim().is_empty()
                                },
                            );
                            let testing = self.test_rx.is_some();
                            // Stretch the three buttons to span the field box so their right
                            // edge lines up with the input fields' right edge.
                            let row_left = ui.max_rect().left();
                            let total = field_right
                                // +4: button border sits ~4px inside its rect vs the field frame
                                .map(|r| (r - row_left + 4.0).max(200.0))
                                .unwrap_or(376.0);
                            let bw = (total - 8.0) / 2.0; // one 8px gap between two buttons
                            let btn = Vec2::new(bw, 28.0);
                            ui.add_space(16.0);
                            ui.horizontal(|ui| {
                                // explicit 8px gaps only — no extra inter-item spacing, so the
                                // two buttons exactly span row_left..field_right (Delete lives in
                                // the Connection Manager toolbar now)
                                ui.spacing_mut().item_spacing.x = 0.0;
                                if ui
                                    .add_enabled(
                                        !testing,
                                        egui::Button::new(if testing {
                                            "Testing…"
                                        } else {
                                            "Test connection"
                                        })
                                        .min_size(btn),
                                    )
                                    .clicked()
                                {
                                    test = true;
                                }
                                ui.add_space(8.0);
                                if ui
                                    .add_enabled(
                                        can_save,
                                        egui::Button::new("Save").min_size(btn),
                                    )
                                    .clicked()
                                {
                                    do_save = true;
                                }
                            });
                        });
                });
                if changed {
                    if let Some(t) = self.tabs.get_mut(idx) {
                        t.dirty = true;
                    }
                }
                if test {
                    self.start_conn_test(idx);
                }
                if do_save {
                    self.save_conn_tab();
                }
            });
    }
}
