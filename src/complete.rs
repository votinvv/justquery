//! SQL editor assistance: the F6 completion popup's data layer + the Smart-Tab edit and the
//! completion key plumbing that runs BEFORE the virtual editor each frame.
//!
//! The completer reads the live in-memory catalog (`JustQueryApp::meta_view`): on an empty context it
//! offers schemas; after `schema.` it offers that schema's relations; after `alias.` it resolves the
//! alias against the statement's FROM/JOIN list and offers that relation's columns.

use crate::codeeditor;
use crate::doc::{Document, Pos};
use crate::{metadata, JustQueryApp};
use eframe::egui;

/// What a suggestion represents (drives its colour in the popup).
#[derive(Clone, Copy)]
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
    pub start: Pos,    // (line, col) where the editable prefix begins (anchor for replace + popup)
    pub sel: usize,    // selected row in `items`
    pub all: Vec<AcItem>,   // the full list for this context (re-filtered as the user types)
    pub items: Vec<AcItem>, // `all` filtered by the current prefix
    pub last_prefix: Option<String>, // префикс, по которому собраны `items` (гейт лишних пересборок)
    pub accept: Option<usize>, // a mouse click in the popup picked this row → apply after show
}

impl Autocomplete {
    pub fn close(&mut self) {
        self.open = false;
        self.all.clear();
        self.items.clear();
        self.last_prefix = None;
        self.sel = 0;
        self.accept = None;
    }
}

use crate::codeeditor::is_word;

/// Smart Tab (PL/SQL-Developer style): the spaces to insert at column `col` to reach the next
/// "hook" — the start of the next word on the previous line `prev` that lies right of the caret.
/// With no such hook (or no previous line) it falls back to the next 4-column tab stop.
pub(crate) fn tab_spaces(prev: Option<&str>, col: usize) -> String {
    let mut target: Option<usize> = None;
    if let Some(prev) = prev {
        let chars: Vec<char> = prev.chars().collect();
        let mut j = 0;
        while j < chars.len() {
            let ws = chars[j] == ' ' || chars[j] == '\t';
            let after_ws = j == 0 || chars[j - 1] == ' ' || chars[j - 1] == '\t';
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

/// Текст окна вокруг каретки для разбора FROM/JOIN: ±400 строк — длиннее statement на
/// практике не бывает, а разбирать весь гигантский дамп ради алиасов незачем.
fn context_text(doc: &mut Document, caret: Pos) -> String {
    const WINDOW: usize = 400;
    let last = doc.line_count() - 1;
    let a = caret.0.saturating_sub(WINDOW);
    let b = (caret.0 + WINDOW).min(last);
    let e = doc.line_length(b);
    doc.get_text_range((a, 0), (b, e)).unwrap_or_default()
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
    pub(crate) fn ac_build(&mut self, doc: &mut Document, caret: Pos, tab_id: u64) {
        let line = caret.0.min(doc.line_count() - 1);
        let chars: Vec<char> = doc.get_line(line).chars().collect();
        let caret_col = caret.1.min(chars.len());
        let mut ws = caret_col;
        while ws > 0 && is_word(chars[ws - 1]) {
            ws -= 1;
        }
        let prefix: String = chars[ws..caret_col].iter().collect();
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
            } else {
                let ctx_text = context_text(doc, caret);
                if let Some((schema, table)) = self.resolve_alias(&ctx_text, &qual) {
                    self.columns_items(schema.as_deref(), &table)
                } else {
                    // fall back to treating the qualifier as a bare table name
                    self.columns_items(None, &qual)
                }
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
        self.ac.start = (line, ws);
        self.ac.all = all;
        self.ac.last_prefix = None; // новый контекст → форсируем первую фильтрацию
        self.ac.sel = 0;
        self.ac_refilter(&prefix);
        self.ac.open = !self.ac.items.is_empty();
    }

    /// F6 completion: build on request, track the prefix, drive the popup keys, accept on
    /// Enter/Tab. Runs BEFORE the editor's input pass each frame (consumes the keys it
    /// uses). Returns true if it edited the buffer (accept).
    pub(crate) fn editor_completion(
        &mut self,
        doc: &mut Document,
        ed: &mut codeeditor::EditorState,
        ctx: &egui::Context,
        tab_id: u64,
    ) -> bool {
        use egui::{Key, Modifiers};
        if self.ac.open && self.ac.tab != tab_id {
            self.ac.close();
        }
        if std::mem::take(&mut self.ac.request) {
            self.ac_build(doc, ed.caret(), tab_id);
        }
        if !self.ac.open {
            return false;
        }
        // keep the prefix in sync; close on a context change
        let caret = ed.caret();
        let line = caret.0.min(doc.line_count() - 1);
        let chars: Vec<char> = doc.get_line(line).chars().collect();
        let caret_col = caret.1.min(chars.len());
        let mut ws = caret_col;
        while ws > 0 && is_word(chars[ws - 1]) {
            ws -= 1;
        }
        if (line, ws) != self.ac.start || ed.has_sel() {
            self.ac.close();
            return false;
        }
        let prefix: String = chars[ws..caret_col].iter().collect();
        self.ac_refilter(&prefix);
        if self.ac.items.is_empty() {
            self.ac.close();
            return false;
        }
        let (up, down, enter, tab, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Tab),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if up {
            self.ac.sel = self.ac.sel.saturating_sub(1);
        }
        if down {
            self.ac.sel = (self.ac.sel + 1).min(self.ac.items.len().saturating_sub(1));
        }
        if esc {
            self.ac.close();
        }
        if (enter || tab) && self.ac.sel < self.ac.items.len() {
            let ins = self.ac.items[self.ac.sel].insert.clone();
            let mut e = ws;
            while e < chars.len() && is_word(chars[e]) {
                e += 1;
            }
            ed.select_range(doc, (line, ws), (line, e));
            ed.replace(doc, &ins);
            self.ac.close();
            return true;
        }
        false
    }

    /// Re-filter `ac.all` by `prefix` (case-insensitive prefix match) into `ac.items`.
    /// Префикс не изменился с прошлого вызова → `items` уже актуальны (popup перерисовывается
    /// каждый кадр, без гейта это был бы клон всего отфильтрованного списка на кадр).
    pub(crate) fn ac_refilter(&mut self, prefix: &str) {
        if self.ac.last_prefix.as_deref() == Some(prefix) {
            return;
        }
        let p = prefix.to_lowercase();
        self.ac.items = self
            .ac
            .all
            .iter()
            .filter(|it| it.insert.to_lowercase().starts_with(&p))
            .cloned()
            .collect();
        self.ac.last_prefix = Some(prefix.to_owned());
        if self.ac.sel >= self.ac.items.len() {
            self.ac.sel = self.ac.items.len().saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_tab_aligns_under_previous_word() {
        // previous line "select a, b"; caret at col 0 of the empty next line:
        // next hook after col 0 is the 'a' at column 7
        assert_eq!(tab_spaces(Some("select a, b"), 0), " ".repeat(7));
    }

    #[test]
    fn smart_tab_falls_back_to_four_stops() {
        // no previous line, caret at col 3 → next 4-stop = 1 space
        assert_eq!(tab_spaces(None, 3), " ");
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
