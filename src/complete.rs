//! SQL editor assistance: the F6 completion popup's data layer + the Smart-Enter / Smart-Tab
//! buffer edits. Pure logic only — the editor (`main.rs::editor`) owns all rendering and key
//! plumbing and calls into here.
//!
//! The completer reads the live in-memory catalog (`JustQueryApp::meta_view`): on an empty context it
//! offers schemas; after `schema.` it offers that schema's relations; after `alias.` it resolves the
//! alias against the statement's FROM/JOIN list and offers that relation's columns.

use crate::{metadata, JustQueryApp};

/// What a suggestion represents (drives its colour in the popup).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum AcKind {
    Schema,
    Table,
    Column,
}

/// One row in the completion popup.
#[derive(Clone)]
pub(crate) struct AcItem {
    pub label: String,  // shown text (columns carry their type)
    pub insert: String, // what gets written into the buffer
    pub kind: AcKind,
}

/// The completion popup's whole state (one editor at a time).
#[derive(Default)]
pub(crate) struct Autocomplete {
    pub open: bool,
    pub request: bool, // F6 was pressed → (re)build the list on the next editor pass
    pub tab: u64,      // the tab the popup belongs to (close it if the active tab changes)
    pub start: usize,  // char index where the editable prefix begins (anchor for replace + popup)
    pub sel: usize,    // selected row in `items`
    pub all: Vec<AcItem>,   // the full list for this context (re-filtered as the user types)
    pub items: Vec<AcItem>, // `all` filtered by the current prefix
    pub accept: Option<usize>, // a mouse click in the popup picked this row → apply after show
}

impl Autocomplete {
    pub fn close(&mut self) {
        self.open = false;
        self.all.clear();
        self.items.clear();
        self.sel = 0;
        self.accept = None;
    }
}

use crate::codeeditor::is_word;

/// Char index of the start of the line containing `ch`.
pub(crate) fn line_start(chars: &[char], ch: usize) -> usize {
    let mut i = ch.min(chars.len());
    while i > 0 && chars[i - 1] != '\n' {
        i -= 1;
    }
    i
}

/// Smart Enter: the string to insert at the caret — a newline that re-creates the leading
/// whitespace of the line containing char `a` (the selection start). The caller replaces any
/// selection with this.
pub(crate) fn enter_indent(text: &str, a: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let a = a.min(chars.len());
    let ls = line_start(&chars, a);
    // copy the run of spaces/tabs at the line start, but not past the caret
    let indent: String = chars[ls..a]
        .iter()
        .take_while(|c| **c == ' ' || **c == '\t')
        .collect();
    format!("\n{indent}")
}

/// Smart Tab (PL/SQL-Developer style): the spaces to insert at the caret to reach the next "hook" —
/// the start of the next word on the previous line that lies right of the caret. With no such hook
/// (or no previous line) it falls back to the next 4-column tab stop.
pub(crate) fn tab_spaces(text: &str, caret: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    let ls = line_start(&chars, caret);
    let col = caret - ls;
    let mut target: Option<usize> = None;
    if ls > 0 {
        // chars[ls - 1] is the '\n' ending the previous line; that line is [pls, ls - 1)
        let pls = line_start(&chars, ls - 1);
        let prev = &chars[pls..ls - 1];
        let mut j = 0;
        while j < prev.len() {
            let ws = prev[j] == ' ' || prev[j] == '\t';
            let after_ws = j == 0 || prev[j - 1] == ' ' || prev[j - 1] == '\t';
            if !ws && after_ws && j > col {
                target = Some(j);
                break;
            }
            j += 1;
        }
    }
    let n = match target {
        Some(j) => j - col,
        None => ((col / 4) + 1) * 4 - col,
    };
    " ".repeat(n)
}

/// Parse the statement's table references: `[schema.]table [[AS] alias]`, including comma lists
/// after FROM. Deliberately small — enough to resolve an alias/table to a relation, not a full SQL
/// grammar. Returns `(schema, table, alias)` triples.
fn collect_refs(text: &str) -> Vec<(Option<String>, String, Option<String>)> {
    #[derive(PartialEq)]
    enum T {
        W(String),
        Dot,
        Comma,
    }
    let cs: Vec<char> = text.chars().collect();
    let n = cs.len();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < n {
        let c = cs[i];
        if is_word(c) {
            let s = i;
            while i < n && is_word(cs[i]) {
                i += 1;
            }
            toks.push(T::W(cs[s..i].iter().collect()));
        } else if c == '.' {
            toks.push(T::Dot);
            i += 1;
        } else if c == ',' {
            toks.push(T::Comma);
            i += 1;
        } else {
            i += 1;
        }
    }
    // keywords that end a table reference (so they're never mistaken for a table or an alias)
    let is_kw = |w: &str| {
        const K: &[&str] = &[
            "from", "join", "inner", "left", "right", "outer", "full", "cross", "on", "using",
            "where", "group", "order", "having", "limit", "offset", "union", "select", "as", "and",
            "or", "set", "values", "returning", "into", "by", "with", "window", "fetch", "for",
        ];
        let l = w.to_ascii_lowercase();
        K.contains(&l.as_str())
    };

    let mut refs = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let trigger = matches!(&toks[i], T::W(w) if { let l = w.to_ascii_lowercase(); l == "from" || l == "join" });
        if !trigger {
            i += 1;
            continue;
        }
        i += 1;
        loop {
            while i < toks.len() && toks[i] == T::Comma {
                i += 1;
            }
            // name: word ('.' word)*
            let mut parts = Vec::new();
            match toks.get(i) {
                Some(T::W(w)) if !is_kw(w) => {
                    parts.push(w.clone());
                    i += 1;
                }
                _ => break,
            }
            while i + 1 < toks.len() && toks[i] == T::Dot {
                if let T::W(w) = &toks[i + 1] {
                    parts.push(w.clone());
                    i += 2;
                } else {
                    break;
                }
            }
            let (schema, table) = if parts.len() >= 2 {
                (
                    Some(parts[parts.len() - 2].clone()),
                    parts[parts.len() - 1].clone(),
                )
            } else {
                (None, parts[0].clone())
            };
            // optional AS
            if let Some(T::W(w)) = toks.get(i) {
                if w.eq_ignore_ascii_case("as") {
                    i += 1;
                }
            }
            // optional alias (any non-keyword word)
            let mut alias = None;
            if let Some(T::W(w)) = toks.get(i) {
                if !is_kw(w) {
                    alias = Some(w.clone());
                    i += 1;
                }
            }
            refs.push((schema, table, alias));
            if i < toks.len() && toks[i] == T::Comma {
                continue;
            }
            break;
        }
    }
    refs
}

impl JustQueryApp {
    /// Resolve `qual` (the identifier before a `.`) to a `(schema, table)` via the statement's
    /// FROM/JOIN list: first as an alias, then as a bare table name. Per spec, when several
    /// references match, the first one wins.
    fn resolve_alias(&self, text: &str, qual: &str) -> Option<(Option<String>, String)> {
        let refs = collect_refs(text);
        for (s, t, a) in &refs {
            if a.as_deref().is_some_and(|al| al.eq_ignore_ascii_case(qual)) {
                return Some((s.clone(), t.clone()));
            }
        }
        for (s, t, _) in &refs {
            if t.eq_ignore_ascii_case(qual) {
                return Some((s.clone(), t.clone()));
            }
        }
        None
    }

    /// True if `name` is a schema we know about — either in the dropdown list or carrying scanned
    /// objects (so system schemas like `pg_catalog` resolve even when hidden from the list).
    fn is_known_schema(&self, name: &str) -> bool {
        self.meta_view.schemas.iter().any(|s| s.eq_ignore_ascii_case(name))
            || self
                .meta_view
                .objects
                .iter()
                .any(|o| o.schema.eq_ignore_ascii_case(name))
    }

    /// Column suggestions for the relation named `table` (optionally in `schema`). First match wins.
    fn columns_items(&self, schema: Option<&str>, table: &str) -> Vec<AcItem> {
        self.meta_view
            .objects
            .iter()
            .find(|o| {
                metadata::is_relation(&o.kind)
                    && o.name.eq_ignore_ascii_case(table)
                    && schema.is_none_or(|sc| o.schema.eq_ignore_ascii_case(sc))
            })
            .map(|o| {
                o.cols
                    .iter()
                    .map(|c| AcItem {
                        label: format!("{}  {}", c.name, c.ty),
                        insert: c.name.clone(),
                        kind: AcKind::Column,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Relation-name suggestions inside `schema` (deduplicated, sorted).
    fn table_items(&self, schema: &str) -> Vec<AcItem> {
        let mut seen = std::collections::HashSet::new();
        let mut v: Vec<AcItem> = self
            .meta_view
            .objects
            .iter()
            .filter(|o| o.schema.eq_ignore_ascii_case(schema) && metadata::is_relation(&o.kind))
            .filter(|o| seen.insert(o.name.clone()))
            .map(|o| AcItem {
                label: o.name.clone(),
                insert: o.name.clone(),
                kind: AcKind::Table,
            })
            .collect();
        v.sort_by_key(|a| a.label.to_lowercase());
        v
    }

    /// Build the completion list for the caret context (called on F6). Sets `ac.start`/`ac.all` and
    /// opens the popup if anything matched the current prefix.
    pub(crate) fn ac_build(&mut self, text: &str, caret: usize, tab_id: u64) {
        let chars: Vec<char> = text.chars().collect();
        let caret = caret.min(chars.len());
        let mut ws = caret;
        while ws > 0 && is_word(chars[ws - 1]) {
            ws -= 1;
        }
        let prefix: String = chars[ws..caret].iter().collect();
        let dotted = ws > 0 && chars[ws - 1] == '.';

        let all: Vec<AcItem> = if dotted {
            // qualifier = the word just before the dot
            let qe = ws - 1;
            let mut qs = qe;
            while qs > 0 && is_word(chars[qs - 1]) {
                qs -= 1;
            }
            let qual: String = chars[qs..qe].iter().collect();
            // schema first: `schema.` → that schema's relations (works for system schemas like
            // pg_catalog too, even when they aren't in the dropdown's `schemas` list)
            if self.is_known_schema(&qual) {
                self.table_items(&qual)
            } else if let Some((schema, table)) = self.resolve_alias(text, &qual) {
                self.columns_items(schema.as_deref(), &table)
            } else {
                // fall back to treating the qualifier as a bare table name
                self.columns_items(None, &qual)
            }
        } else {
            self.meta_view
                .schemas
                .iter()
                .map(|s| AcItem {
                    label: s.clone(),
                    insert: s.clone(),
                    kind: AcKind::Schema,
                })
                .collect()
        };

        self.ac.tab = tab_id;
        self.ac.start = ws;
        self.ac.all = all;
        self.ac.sel = 0;
        self.ac_refilter(&prefix);
        self.ac.open = !self.ac.items.is_empty();
    }

    /// Re-filter `ac.all` by `prefix` (case-insensitive prefix match) into `ac.items`.
    pub(crate) fn ac_refilter(&mut self, prefix: &str) {
        let p = prefix.to_lowercase();
        self.ac.items = self
            .ac
            .all
            .iter()
            .filter(|it| it.insert.to_lowercase().starts_with(&p))
            .cloned()
            .collect();
        if self.ac.sel >= self.ac.items.len() {
            self.ac.sel = self.ac.items.len().saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_enter_copies_indentation() {
        let s = "    select 1".to_string();
        let caret = s.chars().count();
        assert_eq!(enter_indent(&s, caret), "\n    ");
    }

    #[test]
    fn smart_tab_aligns_under_previous_word() {
        // previous line "select a, b"; caret at col 0 of the empty next line
        let s = "select a, b\n".to_string();
        let caret = s.chars().count();
        // next hook after col 0 is the 'a' at column 7
        assert_eq!(tab_spaces(&s, caret), " ".repeat(7));
    }

    #[test]
    fn smart_tab_falls_back_to_four_stops() {
        let s = "abc".to_string(); // no previous line, caret at col 3
        assert_eq!(tab_spaces(&s, 3), " "); // 3 → next 4-stop = 1 space
    }

    #[test]
    fn collect_refs_parses_schema_table_alias() {
        let refs = collect_refs("select * from public.users u join orders o on u.id = o.uid");
        assert_eq!(
            refs,
            vec![
                (Some("public".to_string()), "users".to_string(), Some("u".to_string())),
                (None, "orders".to_string(), Some("o".to_string())),
            ]
        );
    }

    #[test]
    fn collect_refs_handles_comma_list_and_as() {
        let refs = collect_refs("select * from a x, b as y where x.k = y.k");
        assert_eq!(
            refs,
            vec![
                (None, "a".to_string(), Some("x".to_string())),
                (None, "b".to_string(), Some("y".to_string())),
            ]
        );
    }
}
