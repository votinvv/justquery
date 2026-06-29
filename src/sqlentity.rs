//! Extract the "key entity" of a SQL statement — used as the result-tab label.
//!
//! First cut (heuristic; meant to be refined). Priority:
//!   1. the table after the **last** `FROM`;
//!   2. else the DML/DDL target — `UPDATE x`, `INSERT INTO x`, `MERGE INTO x`, `CREATE … x`,
//!      `ALTER … x`, `DROP … x`, `TRUNCATE x`;
//!   3. else the bare leading verb, lowercased (`select 1` → `select`);
//!   4. else `Result`.
//!
//! A tokenizer skips comments, string literals and dollar-quoted bodies (so a procedure body never
//! leaks its own `FROM`), and treats double-quoted identifiers as plain names (never keywords). A
//! schema-qualified name keeps only its last component (`public.orders` → `orders`).

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// An identifier or keyword. `quoted` marks a "double-quoted" identifier — never a keyword.
    Word { text: String, quoted: bool },
    Dot,
    Sym(char),
}

/// Words that are never the entity name right after an anchor — object types + modifiers — so they
/// are skipped while reaching for the name (`create or replace procedure foo` → `foo`). Not
/// exhaustive; extended as the heuristic is refined.
const SKIP: &[&str] = &[
    "into", "only", "or", "replace", "temp", "temporary", "unlogged", "global", "local", "if",
    "not", "exists", "concurrently", "materialized", "unique", "recursive", "table", "view",
    "procedure", "function", "index", "sequence", "trigger", "schema", "type", "database", "role",
    "domain", "aggregate", "operator", "policy", "rule", "tablespace", "publication", "subscription",
    "server", "extension", "foreign", "data", "wrapper", "column", "constraint",
];

/// DML/DDL leading verbs whose target is the key entity (when there is no `FROM`).
const VERBS: &[&str] = &["update", "insert", "merge", "create", "alter", "drop", "truncate"];

/// The key entity of `sql` (see the module docs).
pub fn key_entity(sql: &str) -> String {
    let toks = tokenize(sql);

    // 1) the table after the LAST `from`
    let last_from = toks.iter().enumerate().filter(|(_, t)| is_kw(t, "from")).map(|(i, _)| i).last();
    if let Some(i) = last_from {
        if let Some(name) = name_after(&toks, i + 1) {
            return name;
        }
    }

    // 2) the DML/DDL target after the first such verb
    if let Some(i) = toks.iter().position(|t| VERBS.iter().any(|v| is_kw(t, v))) {
        if let Some(name) = name_after(&toks, i + 1) {
            return name;
        }
    }

    // 3) the bare leading verb (e.g. `select 1` → `select`)
    if let Some(Tok::Word { text, .. }) = toks.iter().find(|t| matches!(t, Tok::Word { .. })) {
        return text.to_ascii_lowercase();
    }

    // 4) nothing recognisable
    "Result".to_owned()
}

/// The qualified name starting at `start` (skipping leading [`SKIP`] filler words); returns its last
/// dotted component. `None` when no identifier follows (e.g. `from (subquery`).
fn name_after(toks: &[Tok], start: usize) -> Option<String> {
    let mut k = start;
    while let Some(Tok::Word { text, quoted }) = toks.get(k) {
        if !quoted && SKIP.iter().any(|s| text.eq_ignore_ascii_case(s)) {
            k += 1;
        } else {
            break;
        }
    }
    let Some(Tok::Word { text, .. }) = toks.get(k) else { return None };
    let mut last = text.clone();
    let mut j = k + 1;
    while matches!(toks.get(j), Some(Tok::Dot)) {
        if let Some(Tok::Word { text, .. }) = toks.get(j + 1) {
            last = text.clone();
            j += 2;
        } else {
            break;
        }
    }
    Some(last)
}

/// `t` is the (unquoted) keyword `kw`, case-insensitively.
fn is_kw(t: &Tok, kw: &str) -> bool {
    matches!(t, Tok::Word { text, quoted } if !quoted && text.eq_ignore_ascii_case(kw))
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || (c as u32) >= 0x80
}
fn is_ident_cont(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$' || (c as u32) >= 0x80
}

/// Length (in chars) of an opening dollar-quote tag at `cs[i]` (`$$` or `$tag$`), or `None` when the
/// `$` doesn't open one (e.g. a `$1` parameter placeholder).
fn dollar_tag_len(cs: &[char], i: usize) -> Option<usize> {
    let n = cs.len();
    let mut j = i + 1;
    while j < n && (cs[j].is_alphanumeric() || cs[j] == '_') {
        j += 1;
    }
    (j < n && cs[j] == '$').then_some(j - i + 1)
}

/// Reduce `sql` to a token stream, discarding comments / strings / dollar-quoted bodies.
fn tokenize(sql: &str) -> Vec<Tok> {
    let cs: Vec<char> = sql.chars().collect();
    let n = cs.len();
    let mut i = 0;
    let mut out = Vec::new();
    while i < n {
        let c = cs[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '-' if i + 1 < n && cs[i + 1] == '-' => {
                i += 2;
                while i < n && cs[i] != '\n' {
                    i += 1;
                }
            }
            '/' if i + 1 < n && cs[i + 1] == '*' => {
                let mut depth = 1;
                i += 2;
                while i < n && depth > 0 {
                    if i + 1 < n && cs[i] == '/' && cs[i + 1] == '*' {
                        depth += 1;
                        i += 2;
                    } else if i + 1 < n && cs[i] == '*' && cs[i + 1] == '/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            '\'' => {
                i += 1;
                while i < n {
                    if cs[i] == '\'' {
                        if i + 1 < n && cs[i + 1] == '\'' {
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            '"' => {
                i += 1;
                let mut s = String::new();
                while i < n {
                    if cs[i] == '"' {
                        if i + 1 < n && cs[i + 1] == '"' {
                            s.push('"');
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        s.push(cs[i]);
                        i += 1;
                    }
                }
                out.push(Tok::Word { text: s, quoted: true });
            }
            '$' => {
                if let Some(tag_len) = dollar_tag_len(&cs, i) {
                    let tag = &cs[i..i + tag_len];
                    let mut j = i + tag_len;
                    let mut closed = false;
                    while j + tag.len() <= n {
                        if cs[j..j + tag.len()] == *tag {
                            j += tag.len();
                            closed = true;
                            break;
                        }
                        j += 1;
                    }
                    i = if closed { j } else { n };
                } else {
                    i += 1;
                }
            }
            '.' => {
                out.push(Tok::Dot);
                i += 1;
            }
            c if is_ident_start(c) => {
                let start = i;
                i += 1;
                while i < n && is_ident_cont(cs[i]) {
                    i += 1;
                }
                out.push(Tok::Word { text: cs[start..i].iter().collect(), quoted: false });
            }
            c if c.is_ascii_digit() => {
                i += 1;
                while i < n && (cs[i].is_ascii_digit() || cs[i] == '.') {
                    i += 1;
                }
            }
            c => {
                out.push(Tok::Sym(c));
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::key_entity as k;

    #[test]
    fn from_simple() {
        assert_eq!(k("select * from users"), "users");
    }
    #[test]
    fn from_schema_qualified_and_alias() {
        assert_eq!(k("SELECT o.* FROM public.orders o WHERE o.id = 1"), "orders");
    }
    #[test]
    fn last_from_wins() {
        assert_eq!(k("select * from a where x in (select id from b)"), "b");
    }
    #[test]
    fn join_takes_first_table() {
        assert_eq!(k("select * from a join b on a.id = b.id"), "a");
    }
    #[test]
    fn select_without_from() {
        assert_eq!(k("select 1"), "select");
        assert_eq!(k("SELECT now()"), "select");
    }
    #[test]
    fn update_target() {
        assert_eq!(k("update accounts set bal = 0 where id = 2"), "accounts");
    }
    #[test]
    fn insert_target() {
        assert_eq!(k("insert into logs (msg) values ('x')"), "logs");
    }
    #[test]
    fn create_table_if_not_exists() {
        assert_eq!(k("create table if not exists t1 (id int)"), "t1");
    }
    #[test]
    fn create_procedure_body_ignored() {
        // the `from secret` inside the dollar-quoted body must NOT win
        assert_eq!(
            k("create or replace procedure do_it() language sql as $$ select * from secret $$"),
            "do_it"
        );
    }
    #[test]
    fn quoted_keyword_as_table_name() {
        assert_eq!(k("select * from \"from\""), "from");
    }
    #[test]
    fn leading_comment_skipped() {
        assert_eq!(k("-- pick from here\nselect * from real_tbl"), "real_tbl");
    }
    #[test]
    fn empty_is_result() {
        assert_eq!(k("   /* nothing */  "), "Result");
    }
}
