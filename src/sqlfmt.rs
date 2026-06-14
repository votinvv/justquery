//! Opinionated SQL formatter — *our* house style, not a generic pretty-printer.
//!
//! It runs a strict **pre-flight linter** first. If the query breaks any house rule the formatter
//! refuses to run and returns the list of violations instead (the caller shows them in a modal):
//!   1. no subqueries written without a `WITH` (every nested SELECT must be a CTE);
//!   2. every table in FROM/JOIN must carry an alias;
//!   3. every column reference must be qualified `alias.column`;
//!   4. every select-list item must be named with `AS …`;
//!   5. no `EXISTS` / `NOT EXISTS`.
//!
//! When the query is clean it is re-emitted with the house layout:
//!   - `with` / `select` / `from` / `where` … — the leading keyword is right-aligned so its last
//!     letter sits on **column 6**; the clause content then starts on **column 8**;
//!   - select-list items: a leading comma on column 6, the expression on column 8, and all the
//!     `as` keywords within one select aligned into a column;
//!   - CTE names go on column 8 with the separating comma on column 6 (same shape as the list);
//!   - everything is lower-cased except the text inside string literals (and quoted identifiers).
//!
//! Pure logic, unit-tested. `format(sql)` is the only entry point.
//!
//! ВРЕМЕННО ПРИПАРКОВАН: кнопки SQL Refact/Inspect убраны из UI (будут переосмыслены), поэтому в
//! релизной сборке движок ничем не вызывается — `#![allow(dead_code)]` гасит предупреждения. Движок
//! и его тесты сохранены; правила — в `docs/SQL_FORMAT_RULES.md`.
#![allow(dead_code)]

// ============================================================ tokenizer

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Kw,      // a reserved word (lower-cased)
    Ident,   // a bare identifier (lower-cased)
    Qident,  // a "quoted identifier" (case preserved)
    Num,
    Str,     // '...' literal (case preserved)
    Dollar,  // $$ … $$ literal (case preserved)
    Op,      // operator: = < > <= >= <> != || :: + - * / % etc.
    Comma,
    Dot,
    LParen,
    RParen,
    Semi,
    Comment,
}

#[derive(Clone, Debug)]
struct Tok {
    kind: Kind,
    text: String,
    pos: usize, // char index of the token start in the source (for caret placement)
}

impl Tok {
    fn is(&self, kind: Kind) -> bool {
        self.kind == kind
    }
    /// A keyword matching `w` (case-insensitive — `text` is already lower-cased for Kw).
    fn kw(&self, w: &str) -> bool {
        self.kind == Kind::Kw && self.text == w
    }
    /// Length of the token in characters (for selecting it).
    fn len(&self) -> usize {
        self.text.chars().count()
    }
    /// Char index just past the token.
    fn end(&self) -> usize {
        self.pos + self.len()
    }
}

/// A rule violation, carrying the source location so the caller can move the caret onto it.
#[derive(Debug)]
pub struct FmtError {
    pub pos: usize, // char index of the offending token
    #[allow(dead_code)] // длина выделения; пока находки прыгают по строке, не по диапазону
    pub len: usize, // char length to select
    pub msg: String,
}

impl PartialEq for FmtError {
    fn eq(&self, o: &Self) -> bool {
        self.pos == o.pos && self.msg == o.msg
    }
}

/// The reserved words we recognise. Anything else that looks like a word is an `Ident`.
const KEYWORDS: &[&str] = &[
    "select", "distinct", "all", "from", "where", "group", "by", "order", "having", "limit",
    "offset", "with", "recursive", "as", "on", "using", "join", "inner", "left", "right", "full",
    "outer", "cross", "natural", "and", "or", "not", "in", "is", "null", "like", "ilike",
    "between", "exists", "case", "when", "then", "else", "end", "asc", "desc", "nulls", "first",
    "last", "union", "intersect", "except", "over", "partition", "filter", "within", "interval",
    "true", "false", "cast", "lateral",
];

/// Type names treated as types (not column references) when qualifying-rule checking.
const TYPES: &[&str] = &[
    "int", "int2", "int4", "int8", "integer", "smallint", "bigint", "serial", "bigserial",
    "numeric", "decimal", "real", "double", "precision", "float", "money", "bool", "boolean",
    "char", "varchar", "character", "varying", "text", "bytea", "uuid", "date", "time",
    "timestamp", "timestamptz", "timetz", "interval", "json", "jsonb", "xml", "inet", "cidr",
    "macaddr", "point", "line", "box", "circle", "oid", "name", "regclass",
];

fn tokenize(src: &str) -> Result<Vec<Tok>, FmtError> {
    let cs: Vec<char> = src.chars().collect();
    let n = cs.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    let push = |out: &mut Vec<Tok>, kind: Kind, s: String, pos: usize| {
        out.push(Tok { kind, text: s, pos })
    };

    while i < n {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // line comment
        if c == '-' && i + 1 < n && cs[i + 1] == '-' {
            let s = i;
            while i < n && cs[i] != '\n' {
                i += 1;
            }
            push(&mut out, Kind::Comment, cs[s..i].iter().collect(), s);
            continue;
        }
        // block comment
        if c == '/' && i + 1 < n && cs[i + 1] == '*' {
            let s = i;
            i += 2;
            let mut closed = false;
            while i + 1 < n {
                if cs[i] == '*' && cs[i + 1] == '/' {
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                return Err(FmtError { pos: s, len: 1, msg: "unterminated block comment".into() });
            }
            i += 2;
            push(&mut out, Kind::Comment, cs[s..i].iter().collect(), s);
            continue;
        }
        // string literal (with '' escape)
        if c == '\'' {
            let s = i;
            i += 1;
            loop {
                if i >= n {
                    return Err(FmtError { pos: s, len: 1, msg: "unterminated string literal".into() });
                }
                if cs[i] == '\'' {
                    if i + 1 < n && cs[i + 1] == '\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            push(&mut out, Kind::Str, cs[s..i].iter().collect(), s);
            continue;
        }
        // quoted identifier
        if c == '"' {
            let s = i;
            i += 1;
            while i < n && cs[i] != '"' {
                i += 1;
            }
            i = (i + 1).min(n);
            push(&mut out, Kind::Qident, cs[s..i].iter().collect(), s);
            continue;
        }
        // dollar-quoted string  $$ … $$  /  $tag$ … $tag$
        if c == '$' {
            if let Some(tag_end) = dollar_tag(&cs, i) {
                let s = i;
                let tag: String = cs[i..tag_end].iter().collect();
                let mut j = tag_end;
                let mut closed = false;
                while j < n {
                    if cs[j] == '$' && cs[j..].iter().collect::<String>().starts_with(&tag) {
                        j += tag.len();
                        closed = true;
                        break;
                    }
                    j += 1;
                }
                if !closed {
                    return Err(FmtError {
                        pos: s,
                        len: 1,
                        msg: "unterminated dollar-quoted string".into(),
                    });
                }
                push(&mut out, Kind::Dollar, cs[s..j].iter().collect(), s);
                i = j;
                continue;
            }
        }
        // number
        if c.is_ascii_digit() || (c == '.' && i + 1 < n && cs[i + 1].is_ascii_digit()) {
            let s = i;
            i += 1;
            while i < n && (cs[i].is_ascii_digit() || cs[i] == '.') {
                i += 1;
            }
            push(&mut out, Kind::Num, cs[s..i].iter().collect(), s);
            continue;
        }
        // identifier / keyword
        if c.is_alphabetic() || c == '_' {
            let s = i;
            i += 1;
            while i < n && (cs[i].is_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            let raw: String = cs[s..i].iter().collect();
            let low = raw.to_ascii_lowercase();
            if KEYWORDS.contains(&low.as_str()) {
                push(&mut out, Kind::Kw, low, s);
            } else {
                push(&mut out, Kind::Ident, low, s);
            }
            continue;
        }
        // punctuation / operators
        let s = i;
        match c {
            ',' => {
                push(&mut out, Kind::Comma, ",".into(), s);
                i += 1;
            }
            '.' => {
                push(&mut out, Kind::Dot, ".".into(), s);
                i += 1;
            }
            '(' => {
                push(&mut out, Kind::LParen, "(".into(), s);
                i += 1;
            }
            ')' => {
                push(&mut out, Kind::RParen, ")".into(), s);
                i += 1;
            }
            ';' => {
                push(&mut out, Kind::Semi, ";".into(), s);
                i += 1;
            }
            _ => {
                // multi-char operators first
                let two: String = cs[i..(i + 2).min(n)].iter().collect();
                if matches!(two.as_str(), "<=" | ">=" | "<>" | "!=" | "||" | "::") {
                    push(&mut out, Kind::Op, two, s);
                    i += 2;
                } else {
                    push(&mut out, Kind::Op, c.to_string(), s);
                    i += 1;
                }
            }
        }
    }
    Ok(out)
}

/// If a `$` at `i` opens a dollar-quote tag, return the index just past the opening `$tag$`.
fn dollar_tag(cs: &[char], i: usize) -> Option<usize> {
    let n = cs.len();
    let mut j = i + 1;
    while j < n && (cs[j].is_alphanumeric() || cs[j] == '_') {
        j += 1;
    }
    if j < n && cs[j] == '$' {
        Some(j + 1)
    } else {
        None
    }
}

// ============================================================ parse model

#[derive(Default)]
struct Select {
    distinct: bool,
    items: Vec<Item>,
    from: Vec<FromItem>,
    where_: Vec<Tok>,
    group_by: Vec<Tok>,
    having: Vec<Tok>,
    order_by: Vec<Tok>,
    tail: Vec<Tok>, // limit / offset / fetch — kept verbatim on one line
}

struct Item {
    expr: Vec<Tok>,
    alias: String,
}

struct FromItem {
    join: String,     // "" for the first table, else "join" / "left join" / …
    table: Vec<Tok>,  // a plain table ref (ident, schema.table or func(...))
    alias: String,
    on: Vec<Tok>,
}

struct Cte {
    name: String,
    body: Select,
}

struct Query {
    ctes: Vec<Cte>,
    body: Select,
    /// Set-operation tail: `(op, select)` for each `union [all] / intersect / except [all]` that
    /// follows `body`. Empty for a plain SELECT.
    set_ops: Vec<(String, Select)>,
}

// ============================================================ parser + linter

struct Parser<'a> {
    t: &'a [Tok],
    p: usize,
    errs: Vec<FmtError>,
}

impl<'a> Parser<'a> {
    /// Парсер строит модель и сообщает только о «жёстких» ошибках (синтаксис / `exists` /
    /// не-SELECT). Семейство алиасов (таблица без алиаса, колонка без квалификации, элемент без
    /// `as`) НЕ ошибка разбора — его чинит [`autofix_query`], а неустранимое уходит варнингами.
    fn new(t: &'a [Tok]) -> Self {
        Self { t, p: 0, errs: Vec::new() }
    }

    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.p)
    }
    fn at_end(&self) -> bool {
        self.p >= self.t.len()
    }
    /// Report an error at the current token (or at the end of input if exhausted).
    fn err(&mut self, m: impl Into<String>) {
        let (pos, len) = match self.t.get(self.p) {
            Some(t) => (t.pos, t.len()),
            None => (self.t.last().map_or(0, |t| t.end()), 0),
        };
        self.errs.push(FmtError { pos, len, msg: m.into() });
    }
    fn parse_query(&mut self) -> Query {
        let mut ctes = Vec::new();
        if self.peek().is_some_and(|t| t.kw("with")) {
            self.p += 1;
            if self.peek().is_some_and(|t| t.kw("recursive")) {
                self.p += 1;
            }
            loop {
                let name = match self.peek() {
                    Some(t) if t.is(Kind::Ident) || t.is(Kind::Qident) => {
                        let n = t.text.clone();
                        self.p += 1;
                        n
                    }
                    _ => {
                        self.err("expected a CTE name after `with`");
                        break;
                    }
                };
                if self.peek().is_some_and(|t| t.kw("as")) {
                    self.p += 1;
                } else {
                    self.err(format!("CTE `{name}` must be written `{name} as ( … )`"));
                }
                // the parenthesised CTE body
                let body = if self.peek().is_some_and(|t| t.is(Kind::LParen)) {
                    let inner = self.take_parens();
                    let mut sub = Parser::new(&inner);
                    let q = sub.parse_query();
                    self.errs.append(&mut sub.errs);
                    if !q.ctes.is_empty() {
                        self.err(format!("nested `with` inside CTE `{name}` is not supported yet"));
                    }
                    q.body
                } else {
                    self.err(format!("CTE `{name}` is missing its `( … )` body"));
                    Select::default()
                };
                ctes.push(Cte { name, body });
                if self.peek().is_some_and(|t| t.is(Kind::Comma)) {
                    self.p += 1;
                    continue;
                }
                break;
            }
        }
        let body = self.parse_select();
        // set-operation tail: `union [all] / intersect [all] / except [all]` + another SELECT
        let mut set_ops = Vec::new();
        while self.peek().is_some_and(is_set_op) {
            let mut op = self.t[self.p].text.clone();
            self.p += 1;
            if self.peek().is_some_and(|t| t.kw("all")) {
                op.push_str(" all");
                self.p += 1;
            }
            set_ops.push((op, self.parse_select()));
        }
        Query { ctes, body, set_ops }
    }

    /// Consume a balanced `( … )` and return the tokens *inside* the parentheses.
    fn take_parens(&mut self) -> Vec<Tok> {
        let mut depth = 0;
        let mut inner = Vec::new();
        while let Some(t) = self.peek() {
            match t.kind {
                Kind::LParen => {
                    if depth > 0 {
                        inner.push(t.clone());
                    }
                    depth += 1;
                }
                Kind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        self.p += 1;
                        break;
                    }
                    inner.push(t.clone());
                }
                _ => inner.push(t.clone()),
            }
            self.p += 1;
        }
        inner
    }

    fn parse_select(&mut self) -> Select {
        let mut s = Select::default();
        if !self.peek().is_some_and(|t| t.kw("select")) {
            self.err("only SELECT / WITH statements can be formatted");
            return s;
        }
        self.p += 1;
        if self.peek().is_some_and(|t| t.kw("distinct")) {
            s.distinct = true;
            self.p += 1;
        }
        // select list (until FROM / a clause end / statement end)
        let list = self.collect_until(&["from"]);
        for raw in split_top_commas(&list) {
            s.items.push(self.make_item(raw));
        }
        if s.items.is_empty() {
            self.err("expected a column after `select`");
        }
        // from. A derived-table subquery `from ( … ) x` is lifted to a CTE by [`autofix_query`].
        if self.peek().is_some_and(|t| t.kw("from")) {
            self.p += 1;
            let from = self.collect_until(&["where", "group", "having", "order", "limit", "offset"]);
            if from.is_empty() {
                self.err("expected a table after `from`");
            } else {
                self.parse_from(&from, &mut s);
            }
        }
        if self.peek().is_some_and(|t| t.kw("where")) {
            self.p += 1;
            s.where_ = self.collect_until(&["group", "having", "order", "limit", "offset"]);
            if s.where_.is_empty() {
                self.err("expected a condition after `where`");
            }
        }
        if self.peek().is_some_and(|t| t.kw("group")) {
            self.p += 1;
            if self.peek().is_some_and(|t| t.kw("by")) {
                self.p += 1;
            }
            s.group_by = self.collect_until(&["having", "order", "limit", "offset"]);
            if s.group_by.is_empty() {
                self.err("expected an expression after `group by`");
            }
        }
        if self.peek().is_some_and(|t| t.kw("having")) {
            self.p += 1;
            s.having = self.collect_until(&["order", "limit", "offset"]);
            if s.having.is_empty() {
                self.err("expected a condition after `having`");
            }
        }
        if self.peek().is_some_and(|t| t.kw("order")) {
            self.p += 1;
            if self.peek().is_some_and(|t| t.kw("by")) {
                self.p += 1;
            }
            s.order_by = self.collect_until(&["limit", "offset"]);
            if s.order_by.is_empty() {
                self.err("expected an expression after `order by`");
            }
        }
        // limit / offset / anything trailing → kept verbatim (но не set-операции — они новый SELECT)
        while !self.at_end()
            && !self.peek().is_some_and(|t| t.is(Kind::Semi) || is_set_op(t))
        {
            let t = self.t[self.p].clone();
            s.tail.push(t);
            self.p += 1;
        }
        s
    }

    /// Collect tokens up to (but not consuming) a top-level keyword in `stops`, a set-op
    /// (`union`/`intersect`/`except` — ends the SELECT), the end, or a `;`. Subqueries are not a
    /// parse error here — FROM derived tables are lifted to CTEs by [`autofix_query`]; WHERE / scalar
    /// subqueries are formatted as-is and reported by validation ([`subquery_lines`]).
    fn collect_until(&mut self, stops: &[&str]) -> Vec<Tok> {
        let mut out = Vec::new();
        let mut depth = 0;
        while self.p < self.t.len() {
            let t = self.t[self.p].clone();
            if depth == 0 {
                if t.is(Kind::Semi) || is_set_op(&t) {
                    break;
                }
                if t.kind == Kind::Kw && stops.contains(&t.text.as_str()) {
                    break;
                }
            }
            match t.kind {
                Kind::LParen => {
                    depth += 1;
                }
                Kind::RParen => depth -= 1,
                _ => {}
            }
            out.push(t);
            self.p += 1;
        }
        out
    }

    /// Build a select-list item, enforcing the explicit-`as` rule and the qualified-column rule.
    fn make_item(&mut self, raw: &[Tok]) -> Item {
        let real: Vec<&Tok> = raw.iter().filter(|t| !t.is(Kind::Comment)).collect();
        // find the top-level `as`
        let mut depth = 0;
        let mut as_at = None;
        for (i, t) in real.iter().enumerate() {
            match t.kind {
                Kind::LParen => depth += 1,
                Kind::RParen => depth -= 1,
                Kind::Kw if depth == 0 && t.text == "as" => {
                    as_at = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let (expr, alias) = match as_at {
            Some(i) if i + 1 < real.len() => {
                let expr: Vec<Tok> = real[..i].iter().map(|t| (*t).clone()).collect();
                (expr, real[i + 1].text.clone())
            }
            // элемент без `as` (включая `*` / выражения) — не ошибка: имя выдаёт autofix (column1) /
            // звёздочку оставляет / квалифицированную колонку именует
            _ => (raw.to_vec(), String::new()),
        };
        Item { expr, alias }
    }

    /// Parse the FROM token run into a base table + joins (alias / qualification handled by autofix).
    fn parse_from(&mut self, toks: &[Tok], s: &mut Select) {
        let real: Vec<Tok> = toks.iter().filter(|t| !t.is(Kind::Comment)).cloned().collect();
        // split into segments at top-level join keywords / commas
        let mut segs: Vec<(String, Vec<Tok>)> = Vec::new();
        let mut cur_join = String::new();
        let mut cur: Vec<Tok> = Vec::new();
        let mut depth = 0;
        let mut i = 0;
        while i < real.len() {
            let t = &real[i];
            match t.kind {
                Kind::LParen => depth += 1,
                Kind::RParen => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                // a comma at top level → another (cross) join
                if t.is(Kind::Comma) {
                    segs.push((std::mem::take(&mut cur_join), std::mem::take(&mut cur)));
                    cur_join = "join".into();
                    i += 1;
                    continue;
                }
                // a join keyword starts a new segment
                if let Some(jk) = join_phrase(&real, i) {
                    if !cur.is_empty() || !segs.is_empty() {
                        segs.push((std::mem::take(&mut cur_join), std::mem::take(&mut cur)));
                    }
                    cur_join = jk.0;
                    i = jk.1;
                    continue;
                }
            }
            cur.push(t.clone());
            i += 1;
        }
        segs.push((cur_join, cur));

        for (join, seg) in segs {
            if seg.is_empty() {
                continue;
            }
            // split off an `on …` condition
            let mut on: Vec<Tok> = Vec::new();
            let mut head = seg.clone();
            let mut d = 0;
            for (k, t) in seg.iter().enumerate() {
                match t.kind {
                    Kind::LParen => d += 1,
                    Kind::RParen => d -= 1,
                    Kind::Kw if d == 0 && (t.text == "on" || t.text == "using") => {
                        head = seg[..k].to_vec();
                        on = seg[k + 1..].to_vec();
                        break;
                    }
                    _ => {}
                }
            }
            // производная таблица `( … ) x` остаётся FromItem'ом — [`autofix_query`] выносит её в CTE;
            // алиас таблицы / квалификацию `on` тоже доводит autofix. table ref = до алиаса.
            let (table, alias) = split_table_alias(&head);
            s.from.push(FromItem { join, table, alias, on });
        }
    }
}

/// A set-operation keyword that joins two SELECTs (`union` / `intersect` / `except`).
fn is_set_op(t: &Tok) -> bool {
    t.kw("union") || t.kw("intersect") || t.kw("except")
}

/// A bare column reference at `r[i]` — an `Ident` that is not the alias/column of a qualified
/// `alias.col`, not a function name, not a type, not a cast target. Used by the auto-qualifier
/// ([`qualify_run`]).
fn is_bare_column(r: &[&Tok], i: usize) -> bool {
    if r[i].kind != Kind::Ident {
        return false;
    }
    let prev = if i > 0 { Some(r[i - 1]) } else { None };
    let next = r.get(i + 1).copied();
    if next.is_some_and(|t| t.is(Kind::Dot)) {
        return false; // alias in alias.column
    }
    if prev.is_some_and(|t| t.is(Kind::Dot)) {
        return false; // column in alias.column
    }
    if next.is_some_and(|t| t.is(Kind::LParen)) {
        return false; // function call
    }
    if TYPES.contains(&r[i].text.as_str()) {
        return false; // a type name (in a cast etc.)
    }
    if prev.is_some_and(|t| t.kw("as") || (t.is(Kind::Op) && t.text == "::")) {
        return false; // cast target type
    }
    true
}

/// A select-item that is `*` or `alias.*` — exempt from the must-be-named / qualified rules (it is a
/// validation *warning* instead, see [`select_star_lines`]).
fn is_star_expr(expr: &[Tok]) -> bool {
    let real: Vec<&Tok> = expr.iter().filter(|t| !t.is(Kind::Comment)).collect();
    match real.last() {
        Some(t) if t.is(Kind::Op) && t.text == "*" => {
            real.len() == 1 || real[real.len() - 2].is(Kind::Dot)
        }
        _ => false,
    }
}

/// True if a token run contains a nested subquery (`( select … )` / `( with … )`). Such a run is
/// left verbatim by [`qualify_run`] so the auto-qualifier never rewrites a subquery's internals.
fn run_has_subquery(run: &[Tok]) -> bool {
    let real: Vec<&Tok> = run.iter().filter(|t| !t.is(Kind::Comment)).collect();
    real.windows(2).any(|w| w[0].is(Kind::LParen) && (w[1].kw("select") || w[1].kw("with")))
}

/// 1-based source lines with a subquery written without `with`, EXCLUDING FROM derived tables
/// (`from ( … )` / `join ( … )`, which the formatter lifts to a CTE). These are the WHERE / scalar /
/// `in ( … )` subqueries the formatter leaves in place — validation reports them as errors.
pub fn subquery_lines(sql: &str) -> Vec<usize> {
    let toks = match tokenize(sql) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let real: Vec<&Tok> = toks.iter().filter(|t| !t.is(Kind::Comment)).collect();
    let mut lines = Vec::new();
    for i in 0..real.len() {
        if !real[i].is(Kind::LParen) {
            continue;
        }
        let opens_subquery = real.get(i + 1).is_some_and(|n| n.kw("select") || n.kw("with"));
        if !opens_subquery {
            continue;
        }
        // FROM derived table (`from ( … )` / `… join ( … )`) — лифтится в CTE, не ошибка
        let prev_is_from = i
            .checked_sub(1)
            .map(|j| real[j])
            .is_some_and(|p| p.kw("from") || p.kw("join"));
        if prev_is_from {
            continue;
        }
        lines.push(1 + sql.chars().take(real[i].pos).filter(|c| *c == '\n').count());
    }
    lines.dedup();
    lines
}

/// 1-based source lines that use `select *` / `select alias.*` (a `*` standing as a select item — not
/// multiplication, not `count(*)`). Used by the validators to emit a warning even when the query
/// executes. Returns lines sorted & de-duplicated.
pub fn select_star_lines(sql: &str) -> Vec<usize> {
    let toks = match tokenize(sql) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let real: Vec<&Tok> = toks.iter().filter(|t| !t.is(Kind::Comment)).collect();
    let mut lines = Vec::new();
    for i in 0..real.len() {
        if !(real[i].is(Kind::Op) && real[i].text == "*") {
            continue;
        }
        // `*` is a select-all when it starts a select item: after select / distinct / a comma, or as
        // the `*` in `alias.*` (after a dot). Multiplication `a * b` has a value before it.
        let is_select_all = match i.checked_sub(1).map(|j| real[j]) {
            None => true,
            Some(p) => p.kw("select") || p.kw("distinct") || p.is(Kind::Comma) || p.is(Kind::Dot),
        };
        if is_select_all {
            lines.push(1 + sql.chars().take(real[i].pos).filter(|c| *c == '\n').count());
        }
    }
    lines.dedup();
    lines
}

// ============================================================ auto-refactor (format)
//
// `format` строит модель и авто-чинит её здесь семантически безопасными правками: добавляет алиас
// таблице, квалифицирует колонки ТОЛЬКО в одно-табличном select (источник один — однозначно), именует
// элементы select (`as col`, иначе `columnN`), выносит FROM-подзапросы в CTE. То, что безопасно
// починить нельзя (колонка без квалификации при join'ах), НЕ ошибка формата — её позиция уходит в
// `warns` (валидация покажет это предупреждением). Жёсткие ошибки (синтаксис/`exists`) — в `errs`.

/// Авто-починка всего запроса: выносим FROM-подзапросы в CTE, затем чиним алиасы во всех телах.
/// `warns` — позиции неквалифицируемых (multi-table) колонок для валидации.
fn autofix_query(q: &mut Query, errs: &mut Vec<FmtError>, warns: &mut Vec<usize>) {
    // вынести производные таблицы `from ( … ) x` в CTE (чистое, эквивалентное преобразование)
    let mut names: std::collections::HashSet<String> =
        q.ctes.iter().map(|c| c.name.clone()).collect();
    let mut extracted = extract_from_subqueries(&mut q.body, &mut names, errs);
    q.ctes.append(&mut extracted); // вынесенные CTE идут после существующих, перед телом
    for c in &mut q.ctes {
        autofix_select(&mut c.body, warns);
    }
    autofix_select(&mut q.body, warns);
    for (_, sel) in &mut q.set_ops {
        autofix_select(sel, warns);
    }
}

/// Вынести FROM-подзапросы (`from ( select … ) x`) `s` в CTE. Возвращает новые CTE; в `s.from`
/// подзапрос заменяется ссылкой на CTE (имя = алиас производной таблицы, домовой стиль `from x x`).
/// Подзапросы со своим `with` внутри не трогаем (редко); они остаются производной таблицей.
fn extract_from_subqueries(
    s: &mut Select,
    names: &mut std::collections::HashSet<String>,
    errs: &mut Vec<FmtError>,
) -> Vec<Cte> {
    let mut out = Vec::new();
    let mut counter = 0usize;
    for f in &mut s.from {
        let is_paren = f.table.first().is_some_and(|t| t.is(Kind::LParen))
            && f.table.last().is_some_and(|t| t.is(Kind::RParen));
        if !is_paren {
            continue;
        }
        // алиас обязателен (Postgres требует его у производной таблицы); коллизия с именем CTE → не
        // трогаем (переименование сломало бы ссылки `alias.col`)
        if !f.alias.is_empty() && names.contains(&f.alias) {
            continue;
        }
        let inner: Vec<Tok> = f.table[1..f.table.len() - 1].to_vec();
        let mut sub = Parser::new(&inner);
        let subq = sub.parse_query();
        errs.append(&mut sub.errs);
        if !subq.ctes.is_empty() {
            continue; // вложенный with внутри производной таблицы — оставляем как есть
        }
        let name = if !f.alias.is_empty() {
            f.alias.clone()
        } else {
            counter += 1;
            let mut n = format!("cte{counter}");
            while names.contains(&n) {
                counter += 1;
                n = format!("cte{counter}");
            }
            f.alias = n.clone(); // дать безымянной производной таблице алиас (домовой стиль)
            n
        };
        names.insert(name.clone());
        let pos = f.table.first().map_or(0, |t| t.pos);
        out.push(Cte { name: name.clone(), body: subq.body });
        f.table = vec![Tok { kind: Kind::Ident, text: name, pos }]; // `from <cte> <alias>`
    }
    out
}

/// Привести один SELECT к домовому стилю: алиасы таблиц → квалификация колонок (если таблица одна)
/// → именование элементов select. Неквалифицируемые (multi-table) колонки — позициями в `warns`.
fn autofix_select(s: &mut Select, warns: &mut Vec<usize>) {
    if s.items.is_empty() && s.from.is_empty() {
        return; // не-SELECT / пустой
    }
    // 1) алиасы таблиц
    let mut used: std::collections::HashSet<String> =
        s.from.iter().filter(|f| !f.alias.is_empty()).map(|f| f.alias.clone()).collect();
    for f in &mut s.from {
        if !f.alias.is_empty() {
            continue;
        }
        if f.table.first().is_some_and(|t| t.is(Kind::LParen)) {
            continue; // подзапрос в FROM — это правило 1 (ловится отдельно), алиасом не лечим
        }
        let base = derive_alias(&f.table);
        f.alias = unique_alias(base, &mut used);
    }
    // 2) квалификация колонок безопасна только при единственном источнике
    let single = s.from.len() == 1 && !s.from[0].table.first().is_some_and(|t| t.is(Kind::LParen));
    let alias0 = if single { s.from[0].alias.clone() } else { String::new() };
    for it in &mut s.items {
        qualify_run(&mut it.expr, single, &alias0, warns);
    }
    qualify_run(&mut s.where_, single, &alias0, warns);
    qualify_run(&mut s.group_by, single, &alias0, warns);
    qualify_run(&mut s.having, single, &alias0, warns);
    qualify_run(&mut s.order_by, single, &alias0, warns);
    for f in &mut s.from {
        // условия join (`on …`) есть лишь при нескольких таблицах → квалифицировать нельзя, в warns
        qualify_run(&mut f.on, false, "", warns);
    }
    // 3) именование элементов select (после квалификации — видим уже `alias.col`). Имя не выводимо
    //    (агрегаты/выражения) → не ругаемся, а сами даём `column1`, `column2`, …
    let mut auto = 0usize;
    for it in &mut s.items {
        if !it.alias.is_empty() || is_star_expr(&it.expr) {
            continue; // звёздочку не именуем (это warning валидации, не ошибка формата)
        }
        it.alias = derive_item_name(&it.expr).unwrap_or_else(|| {
            auto += 1;
            format!("column{auto}")
        });
    }
}

/// Короткий алиас-основа из ссылки на таблицу — первая буква имени (домовой стиль: `from foo f`).
/// Имя берём от функции (если это вызов) или от последнего идентификатора (`schema.table` →
/// `table`); запасной — `t`. Уникальность обеспечивает [`unique_alias`].
fn derive_alias(table: &[Tok]) -> String {
    let is_call = table.iter().any(|t| t.is(Kind::LParen));
    let name = if is_call {
        table.iter().find(|t| matches!(t.kind, Kind::Ident | Kind::Qident))
    } else {
        table.iter().rev().find(|t| matches!(t.kind, Kind::Ident | Kind::Qident))
    };
    let raw = name.map(|t| t.text.as_str()).unwrap_or("t");
    raw.chars()
        .find(|c| c.is_alphabetic())
        .map(|c| c.to_lowercase().to_string())
        .unwrap_or_else(|| "t".to_owned())
}

/// Сделать алиас уникальным среди `used` (добавляя номер), затем зарегистрировать его.
fn unique_alias(base: String, used: &mut std::collections::HashSet<String>) -> String {
    let mut cand = base.clone();
    let mut n = 1;
    while used.contains(&cand) {
        cand = format!("{base}{n}");
        n += 1;
    }
    used.insert(cand.clone());
    cand
}

/// Квалифицировать все «голые» колонки в наборе токенов. При `single` каждую `col` превращаем в
/// `alias0 . col`; иначе (несколько источников) — оставляем как есть, а позицию колонки кладём в
/// `warns` (валидация покажет предупреждением — однозначно дополнить нельзя).
fn qualify_run(run: &mut Vec<Tok>, single: bool, alias0: &str, warns: &mut Vec<usize>) {
    if run.is_empty() {
        return;
    }
    if run_has_subquery(run) {
        return; // не трогаем выражения с вложенным select — его внутренности не наши
    }
    // индексы голых колонок считаем по «реальным» токенам, но правим исходный run (с комментариями)
    let real: Vec<&Tok> = run.iter().filter(|t| !t.is(Kind::Comment)).collect();
    let mut hits: Vec<usize> = Vec::new(); // позиции голых колонок В real-индексации
    for i in 0..real.len() {
        if is_bare_column(&real, i) {
            hits.push(i);
        }
    }
    if hits.is_empty() {
        return;
    }
    if !single {
        for &i in &hits {
            warns.push(real[i].pos); // char-позиция → строку посчитает `unqualified_lines`
        }
        return;
    }
    // single: вставить `alias0 .` перед каждой голой колонкой. Идём по исходному run, сопоставляя
    // его не-комментарии с real по порядку.
    let hitset: std::collections::HashSet<usize> = hits.into_iter().collect();
    let mut out: Vec<Tok> = Vec::with_capacity(run.len() + hitset.len() * 2);
    let mut ri = 0; // индекс в real
    for t in run.drain(..) {
        if !t.is(Kind::Comment) {
            if hitset.contains(&ri) {
                out.push(Tok { kind: Kind::Ident, text: alias0.to_owned(), pos: t.pos });
                out.push(Tok { kind: Kind::Dot, text: ".".to_owned(), pos: t.pos });
            }
            ri += 1;
        }
        out.push(t);
    }
    *run = out;
}

/// Имя для элемента select без `as`: для `alias.col` или одиночного `col` — это `col`; для прочего
/// (вызовы, выражения) — `None` (безопасно не назвать).
fn derive_item_name(expr: &[Tok]) -> Option<String> {
    let real: Vec<&Tok> = expr.iter().filter(|t| !t.is(Kind::Comment)).collect();
    let last = real.last()?;
    if !matches!(last.kind, Kind::Ident | Kind::Qident) {
        return None;
    }
    if real.len() == 1 {
        return Some(last.text.clone()); // одиночная колонка
    }
    if real.len() >= 2 && real[real.len() - 2].is(Kind::Dot) {
        return Some(last.text.clone()); // колонка в alias.col
    }
    None
}

/// If a join phrase starts at `i`, return (phrase, index past it). Handles
/// `join`, `inner/left/right/full [outer] join`, `cross join`, `natural … join`.
fn join_phrase(t: &[Tok], i: usize) -> Option<(String, usize)> {
    if !t.get(i).is_some_and(|x| x.kind == Kind::Kw) {
        return None;
    }
    let w = t[i].text.as_str();
    let mut words = Vec::new();
    let mut j = i;
    let starters = ["join", "inner", "left", "right", "full", "cross", "natural"];
    if !starters.contains(&w) {
        return None;
    }
    while j < t.len() && t[j].kind == Kind::Kw {
        let ww = t[j].text.as_str();
        if ["inner", "left", "right", "full", "cross", "natural", "outer", "join"].contains(&ww) {
            words.push(ww.to_string());
            let done = ww == "join";
            j += 1;
            if done {
                break;
            }
        } else {
            break;
        }
    }
    if words.last().map(|s| s.as_str()) == Some("join") {
        Some((words.join(" "), j))
    } else {
        None
    }
}

/// Split a table reference `name [as] alias` into (table tokens, alias). The alias is the trailing
/// identifier (after an optional `as`); the rest is the table ref (ident / schema.table / func(…)).
fn split_table_alias(head: &[Tok]) -> (Vec<Tok>, String) {
    if head.is_empty() {
        return (Vec::new(), String::new());
    }
    let last = &head[head.len() - 1];
    let is_name = last.is(Kind::Ident) || last.is(Kind::Qident);
    if !is_name {
        return (head.to_vec(), String::new());
    }
    // `as alias`
    if head.len() >= 2 && head[head.len() - 2].kw("as") {
        return (head[..head.len() - 2].to_vec(), last.text.clone());
    }
    // bare `name alias` — but only if what precedes ends a table ref (ident or `)` or `.ident`)
    if head.len() >= 2 {
        let before = &head[head.len() - 2];
        if matches!(before.kind, Kind::Ident | Kind::Qident | Kind::RParen) {
            return (head[..head.len() - 1].to_vec(), last.text.clone());
        }
        if before.is(Kind::Dot) {
            // `schema.table` with no alias
            return (head.to_vec(), String::new());
        }
    }
    // a single token = table with no alias
    (head.to_vec(), String::new())
}

/// Split a token run on top-level commas.
fn split_top_commas(toks: &[Tok]) -> Vec<&[Tok]> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, t) in toks.iter().enumerate() {
        match t.kind {
            Kind::LParen => depth += 1,
            Kind::RParen => depth -= 1,
            Kind::Comma if depth == 0 => {
                out.push(&toks[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < toks.len() {
        out.push(&toks[start..]);
    }
    out
}

// ============================================================ syntax checks

/// Postgres-dialect syntax: parentheses must balance. Reports each stray `)` and each unclosed `(`
/// at its source position. (Unterminated string / dollar-quote / block-comment literals are caught
/// earlier, in [`tokenize`].)
fn check_parens(toks: &[Tok], errs: &mut Vec<FmtError>) {
    let mut open: Vec<usize> = Vec::new(); // positions of unmatched `(`
    for t in toks {
        match t.kind {
            Kind::LParen => open.push(t.pos),
            Kind::RParen
                if open.pop().is_none() => {
                    errs.push(FmtError { pos: t.pos, len: 1, msg: "unmatched `)`".into() });
                }
            _ => {}
        }
    }
    for pos in open {
        errs.push(FmtError { pos, len: 1, msg: "unclosed `(` — missing `)`".into() });
    }
}

// ============================================================ EXISTS check

/// Rule 5: reject `exists` / `not exists` anywhere in the statement.
fn check_no_exists(toks: &[Tok], errs: &mut Vec<FmtError>) {
    for t in toks.iter().filter(|t| t.kw("exists")) {
        errs.push(FmtError {
            pos: t.pos,
            len: t.len(),
            msg: "`exists` / `not exists` is not allowed".into(),
        });
    }
}

// ============================================================ printer

const KW_END: usize = 6; // a clause keyword's last letter sits on this column
const EXPR_COL: usize = 8; // clause content starts here

/// Right-align a clause keyword so its last letter lands on column `KW_END`.
fn lead(kw: &str) -> String {
    format!("{kw:>width$}", width = KW_END)
}

/// Render a token run back to a single line, with house spacing rules.
fn render(toks: &[Tok]) -> String {
    let r: Vec<&Tok> = toks.iter().filter(|t| !t.is(Kind::Comment)).collect();
    let mut out = String::new();
    for i in 0..r.len() {
        let t = r[i];
        if i > 0 {
            let prev = r[i - 1];
            if space_between(prev, t) {
                out.push(' ');
            }
        }
        out.push_str(&t.text);
    }
    out
}

fn space_between(prev: &Tok, cur: &Tok) -> bool {
    // typed literal: `date'…'` / `interval'…'` / `timestamp'…'` — no gap before the string
    if cur.is(Kind::Str) && prev.is(Kind::Ident) && TYPES.contains(&prev.text.as_str()) {
        return false;
    }
    // array subscript brackets `arr[i]` — no surrounding space
    if cur.is(Kind::Op) && (cur.text == "[" || cur.text == "]") {
        return false;
    }
    if prev.is(Kind::Op) && prev.text == "[" {
        return false;
    }
    // never a space before these
    if matches!(cur.kind, Kind::Comma | Kind::Dot | Kind::Semi) {
        return false;
    }
    if cur.is(Kind::Op) && cur.text == "::" {
        return false;
    }
    // house style `( … )`: no space before `)` (`arg)`), except an empty pair keeps it (`( )`)
    if cur.is(Kind::RParen) {
        return prev.is(Kind::LParen);
    }
    // never a space after these
    if prev.is(Kind::Dot) {
        return false;
    }
    if prev.is(Kind::Op) && prev.text == "::" {
        return false;
    }
    // house style: a space BEFORE `(` — `func ( …)`, `in ( …)`, `values ( …)` (but not `((`).
    // A space AFTER `(` (`( arg`) is the default (no rule suppresses it).
    if cur.is(Kind::LParen) {
        return !prev.is(Kind::LParen);
    }
    true
}

/// Print one SELECT body at the base (column 1) indent.
fn print_select(s: &Select, out: &mut String) {
    // ---- select list, with the `as` keywords aligned ----
    let exprs: Vec<String> = s.items.iter().map(|it| render(&it.expr)).collect();
    let w = exprs.iter().map(|e| e.chars().count()).max().unwrap_or(0);
    for (i, it) in s.items.iter().enumerate() {
        let expr = &exprs[i];
        let pad = " ".repeat(w - expr.chars().count());
        let tail = if it.alias.is_empty() {
            String::new()
        } else {
            format!("{pad} as {}", it.alias)
        };
        if i == 0 {
            let kw = if s.distinct { "select distinct" } else { "select" };
            // first item sits on column 8 (after `select ` / right-aligned keyword + space)
            if s.distinct {
                out.push_str(&format!("{kw} {expr}{tail}\n"));
            } else {
                out.push_str(&format!("{} {expr}{tail}\n", lead("select")));
            }
        } else {
            // leading comma on column 6, a space, expression on column 8
            out.push_str(&format!("{}, {expr}{tail}\n", " ".repeat(KW_END - 1)));
        }
    }
    // ---- from / joins ----
    for f in &s.from {
        if f.join.is_empty() {
            out.push_str(&format!("{} {}", lead("from"), render(&f.table)));
        } else {
            let (first, rest) = split_first_word(&f.join);
            out.push_str(&lead(first));
            out.push(' ');
            if !rest.is_empty() {
                out.push_str(rest);
                out.push(' ');
            }
            out.push_str(&render(&f.table));
        }
        if !f.alias.is_empty() {
            out.push(' ');
            out.push_str(&f.alias);
        }
        if !f.on.is_empty() {
            out.push_str(" on ");
            out.push_str(&render(&f.on));
        }
        out.push('\n');
    }
    // ---- where, split on top-level and/or, each connector right-aligned ----
    if !s.where_.is_empty() {
        print_bool_clause("where", &s.where_, out);
    }
    if !s.group_by.is_empty() {
        out.push_str(&format!("{} by {}\n", lead("group"), render(&s.group_by)));
    }
    if !s.having.is_empty() {
        print_bool_clause("having", &s.having, out);
    }
    if !s.order_by.is_empty() {
        out.push_str(&format!("{} by {}\n", lead("order"), render(&s.order_by)));
    }
    if !s.tail.is_empty() {
        out.push_str(&format!("{}\n", render_tail(&s.tail)));
    }
}

/// `where` / `having`: break on top-level AND / OR, right-aligning each connector to column 6.
fn print_bool_clause(kw: &str, toks: &[Tok], out: &mut String) {
    let real: Vec<Tok> = toks.iter().filter(|t| !t.is(Kind::Comment)).cloned().collect();
    // split into (connector, segment) pairs
    let mut segs: Vec<(String, Vec<Tok>)> = Vec::new();
    let mut conn = kw.to_string();
    let mut cur: Vec<Tok> = Vec::new();
    let mut depth = 0;
    for t in &real {
        match t.kind {
            Kind::LParen => depth += 1,
            Kind::RParen => depth -= 1,
            _ => {}
        }
        if depth == 0 && (t.kw("and") || t.kw("or")) {
            segs.push((std::mem::take(&mut conn), std::mem::take(&mut cur)));
            conn = t.text.clone();
            continue;
        }
        cur.push(t.clone());
    }
    segs.push((conn, cur));
    for (c, seg) in segs {
        out.push_str(&format!("{} {}\n", lead(&c), render(&seg)));
    }
}

/// limit / offset tail — keep `limit` aligned like a clause keyword if it leads.
fn render_tail(toks: &[Tok]) -> String {
    if toks.first().is_some_and(|t| t.kw("limit")) {
        let rest = render(&toks[1..]);
        return format!("{} {}", lead("limit"), rest);
    }
    render(toks)
}

fn split_first_word(s: &str) -> (&str, &str) {
    match s.find(' ') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    }
}

/// Print a whole query (optional WITH block + the main SELECT).
fn print_query(q: &Query, out: &mut String) {
    if !q.ctes.is_empty() {
        out.push_str(&format!("{}\n", lead("with")));
        for (i, c) in q.ctes.iter().enumerate() {
            // CTE name on column 8, the separating comma on column 6 (same shape as the list)
            if i == 0 {
                // CTE name on column 8
                out.push_str(&format!("{}{} as (\n", " ".repeat(EXPR_COL - 1), c.name));
            } else {
                // separating comma on column 6, name on column 8
                out.push_str(&format!("{}, {} as (\n", " ".repeat(KW_END - 1), c.name));
            }
            print_select(&c.body, out);
            out.push_str(")\n");
        }
    }
    print_select(&q.body, out);
    // set-operation tail: keyword right-aligned on its own line, then the next SELECT
    for (op, sel) in &q.set_ops {
        let (first, rest) = split_first_word(op);
        if rest.is_empty() {
            out.push_str(&format!("{}\n", lead(first)));
        } else {
            out.push_str(&format!("{} {rest}\n", lead(first)));
        }
        print_select(sel, out);
    }
}

// ============================================================ entry points

/// Tokenize, split into statements, parse and auto-refactor (alias-family). Returns the parsed
/// statements (each with its leading-comment header) **and** the positions of unqualifiable
/// (multi-table) bare columns for validation, or the hard errors (syntax / `exists` / non-SELECT).
fn parse_all(sql: &str) -> Result<(Vec<(Vec<String>, Query)>, Vec<usize>), Vec<FmtError>> {
    // Postgres-dialect syntax first (unterminated literals → here; unbalanced parens → next). A
    // malformed statement makes the structural rule checks unreliable, so bail on the syntax error.
    let toks = tokenize(sql).map_err(|e| vec![e])?;
    let mut syntax = Vec::new();
    check_parens(&toks, &mut syntax);
    if !syntax.is_empty() {
        syntax.sort_by_key(|e| e.pos);
        return Err(syntax);
    }
    if toks.iter().all(|t| t.is(Kind::Comment)) {
        return Err(vec![FmtError { pos: 0, len: 0, msg: "nothing to format".into() }]);
    }

    // split into top-level statements on `;`
    let mut stmts: Vec<Vec<Tok>> = Vec::new();
    let mut cur = Vec::new();
    for t in &toks {
        if t.is(Kind::Semi) {
            if !cur.is_empty() {
                stmts.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(t.clone());
        }
    }
    if !cur.is_empty() {
        stmts.push(cur);
    }

    let mut errs = Vec::new();
    let mut warns: Vec<usize> = Vec::new(); // позиции неквалифицируемых колонок (для валидации)
    // each entry: (verbatim leading comments, parsed query). Inline comments inside a statement are
    // not preserved yet (v1) — only a leading comment block is kept, printed as a header.
    let mut parsed: Vec<(Vec<String>, Query)> = Vec::new();
    for st in &stmts {
        let mut k = 0;
        let mut header = Vec::new();
        while k < st.len() && st[k].is(Kind::Comment) {
            header.push(st[k].text.clone());
            k += 1;
        }
        let body: Vec<Tok> = st[k..].iter().filter(|t| !t.is(Kind::Comment)).cloned().collect();
        if body.is_empty() {
            // comment-only chunk → keep it as a standalone header
            if !header.is_empty() {
                parsed.push((header, Query { ctes: Vec::new(), body: Select::default(), set_ops: Vec::new() }));
            }
            continue;
        }
        check_no_exists(&body, &mut errs);
        let mut p = Parser::new(&body);
        let mut q = p.parse_query();
        errs.append(&mut p.errs);
        autofix_query(&mut q, &mut errs, &mut warns);
        parsed.push((header, q));
    }
    // de-duplicate while preserving order (the same rule can trip many times). The errors are sorted
    // by source position so the caller can jump to the *first* problem in the text.
    errs.dedup();
    errs.sort_by_key(|e| e.pos);
    if !errs.is_empty() {
        return Err(errs);
    }
    Ok((parsed, warns))
}

/// Hard-error check used by the offline Validate fallback: syntax / `exists` only (the alias-family
/// is auto-fixed by [`format`]; non-SELECT statements pass through). `Ok` = nothing to block on.
pub fn validate(sql: &str) -> Result<(), Vec<FmtError>> {
    format(sql).map(|_| ())
}

/// True if `sql`'s first word (after leading whitespace/comments) is `select` / `with` — a query we
/// structurally format. Textual, not token-based, so a syntax error inside the query still routes it
/// to [`format_query_block`] (which surfaces the error) instead of silently passing it through.
fn is_query_text(sql: &str) -> bool {
    let cs: Vec<char> = sql.chars().collect();
    let n = cs.len();
    let mut i = 0;
    loop {
        while i < n && cs[i].is_whitespace() {
            i += 1;
        }
        if i + 1 < n && cs[i] == '-' && cs[i + 1] == '-' {
            while i < n && cs[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < n && cs[i] == '/' && cs[i + 1] == '*' {
            i += 2;
            while i + 1 < n && !(cs[i] == '*' && cs[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(n);
            continue;
        }
        break;
    }
    let word: String = cs[i..].iter().take_while(|c| c.is_alphabetic()).collect();
    let w = word.to_ascii_lowercase();
    w == "select" || w == "with"
}

/// 1-based source lines of bare columns the formatter cannot safely qualify (more than one source
/// table → ambiguous). Validation surfaces these as warnings. Empty on a hard parse error.
pub fn unqualified_lines(sql: &str) -> Vec<usize> {
    let mut lines: Vec<usize> = match parse_all(sql) {
        Ok((_, positions)) => positions
            .into_iter()
            .map(|pos| 1 + sql.chars().take(pos).filter(|c| *c == '\n').count())
            .collect(),
        Err(_) => Vec::new(),
    };
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// Format a whole script: each top-level statement that is a query (SELECT/WITH) is structurally
/// reformatted ([`format_query_block`]); every other statement (DDL/DML/procedure/…) is passed
/// through verbatim — except complete SQL inside `$tag$…$tag$` dollar-quotes, which is formatted in
/// place (incomplete/interpolated fragments and non-SQL stay as-is, [`format_dollar_blocks_in`]).
/// Aborts only when a query statement has a hard error (`exists` / syntax).
pub fn format(sql: &str) -> Result<String, Vec<FmtError>> {
    let mut parts: Vec<String> = Vec::new();
    let mut errs: Vec<FmtError> = Vec::new();
    for raw in crate::connections::split_statements(sql) {
        let stmt = raw.trim();
        if stmt.is_empty() {
            continue;
        }
        if is_query_text(stmt) {
            match format_query_block(stmt) {
                Ok(f) => parts.push(f),
                Err(mut e) => errs.append(&mut e),
            }
        } else if let Some((prefix, query)) = embedded_query_split(stmt) {
            // `create … as select …` / `insert … ( cols) select …`: префикс одной строкой,
            // встроенный SELECT/WITH — нашим принтером. Не вышло разобрать → весь statement как есть.
            match format_query_block(&query) {
                Ok(f) => parts.push(format!("{}\n{f}", render(&prefix))),
                Err(_) => parts.push(format!("{};", format_dollar_blocks_in(stmt))),
            }
        } else {
            // не-запрос без встроенного SELECT: как есть, но форматируем SQL внутри $tag$…$tag$
            parts.push(format!("{};", format_dollar_blocks_in(stmt)));
        }
    }
    if !errs.is_empty() {
        errs.dedup();
        errs.sort_by_key(|e| e.pos);
        return Err(errs);
    }
    // операторы разделяем одним переводом строки — без лишних пустых строк (завершающий перевод
    // в хвосте добавляет вызывающий, и только если его нет)
    Ok(parts.join("\n"))
}

/// If `stmt` is a statement that wraps a top-level query — `create … as select …`,
/// `insert … ( cols) select/with …` — return `(prefix tokens, query text)`. The query starts at the
/// first `select`/`with` keyword at paren-depth 0 (so a scalar subquery inside `( … )` is ignored).
/// `None` for pure DDL/DML with no embedded query (e.g. `create table ( … )`, `insert … values`).
fn embedded_query_split(stmt: &str) -> Option<(Vec<Tok>, String)> {
    let toks = tokenize(stmt).ok()?;
    let mut depth = 0;
    for (idx, t) in toks.iter().enumerate() {
        match t.kind {
            Kind::LParen => depth += 1,
            Kind::RParen => depth -= 1,
            Kind::Kw if depth == 0 && (t.kw("select") || t.kw("with")) => {
                if t.pos == 0 {
                    return None; // чистый запрос — обрабатывается отдельно (is_query_text)
                }
                let cs: Vec<char> = stmt.chars().collect();
                let query: String = cs[t.pos..].iter().collect();
                return Some((toks[..idx].to_vec(), query));
            }
            _ => {}
        }
    }
    None
}

/// Structurally format ONE query statement (SELECT / WITH, with set-ops). Used by [`format`] and by
/// the dollar-quote formatter. Returns the house-style text (ending with `;`) or the hard errors.
fn format_query_block(sql: &str) -> Result<String, Vec<FmtError>> {
    let (parsed, _warns) = parse_all(sql)?;
    let mut out = String::new();
    for (i, (header, q)) in parsed.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        for c in header {
            out.push_str(c);
            out.push('\n');
        }
        let is_query = !q.ctes.is_empty() || !q.body.items.is_empty();
        if is_query {
            print_query(q, &mut out);
            if out.ends_with('\n') {
                out.pop();
            }
            out.push_str(";\n");
        }
    }
    if out.ends_with('\n') {
        out.pop();
    }
    Ok(out)
}

/// Replace each `$tag$ … $tag$` dollar-quoted block in `text` with a formatted version IF its content
/// is a complete query (SELECT/WITH); otherwise the block is kept verbatim. Everything outside the
/// blocks is copied unchanged. Interpolated fragments (incomplete SQL) and non-SQL text stay as-is.
fn format_dollar_blocks_in(text: &str) -> String {
    let cs: Vec<char> = text.chars().collect();
    let n = cs.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        if cs[i] == '$' {
            if let Some(open_end) = dollar_tag(&cs, i) {
                let tag: Vec<char> = cs[i..open_end].to_vec();
                let tl = tag.len();
                // найти закрывающий такой же тег
                let mut j = open_end;
                let mut close = None;
                while j + tl <= n {
                    if cs[j] == '$' && cs[j..j + tl] == tag[..] {
                        close = Some(j);
                        break;
                    }
                    j += 1;
                }
                if let Some(cl) = close {
                    let tag_s: String = tag.iter().collect();
                    let inner: String = cs[open_end..cl].iter().collect();
                    out.push_str(&format_dollar_block(&tag_s, &inner));
                    i = cl + tl;
                    continue;
                }
            }
        }
        out.push(cs[i]);
        i += 1;
    }
    out
}

/// Format the content of one dollar-quoted block: a complete query → formatted; a plpgsql block
/// (`begin … end`) → its inner SQL statements reformatted in place ([`format_plpgsql`]); anything
/// else (non-SQL text, interpolated fragments) → kept verbatim.
fn format_dollar_block(tag: &str, inner: &str) -> String {
    let body = inner.trim();
    if is_query_text(body) {
        if let Ok(f) = format_query_block(body) {
            // не добавляем `;`, если его не было в исходном фрагменте
            let f = if body.ends_with(';') { f } else { f.trim_end_matches(';').to_owned() };
            return format!("{tag}\n{f}\n{tag}");
        }
    } else if is_plpgsql_block(body) {
        return format!("{tag}{}{tag}", format_plpgsql(inner));
    }
    format!("{tag}{inner}{tag}")
}

/// True if `sql` begins a plpgsql block — first word is `begin` or `declare`.
fn is_plpgsql_block(sql: &str) -> bool {
    let w: String = sql.trim_start().chars().take_while(|c| c.is_alphabetic()).collect();
    let w = w.to_ascii_lowercase();
    w == "begin" || w == "declare"
}

/// Reformat the SQL statements inside a plpgsql body **in place**: each top-level statement that is a
/// query (`select` / `with` / `create … as select` / `insert … select`) is reformatted and indented
/// to its original column; everything else — `begin`/`end`, `perform`, `analyze`, `commit`, control
/// flow, and hand-aligned dynamic SQL — is kept byte-for-byte. Conservative: on any tokenize error
/// the body is returned unchanged. A leading `begin` on the first statement is stepped over so only
/// the SQL after it is touched.
fn format_plpgsql(inner: &str) -> String {
    let toks = match tokenize(inner) {
        Ok(t) => t,
        Err(_) => return inner.to_owned(),
    };
    // top-level `;` positions (paren-depth 0; quotes/dollar/comments already handled by the lexer)
    let mut semis: Vec<usize> = Vec::new();
    let mut depth = 0i32;
    for t in &toks {
        match t.kind {
            Kind::LParen => depth += 1,
            Kind::RParen => depth -= 1,
            Kind::Semi if depth == 0 => semis.push(t.pos),
            _ => {}
        }
    }
    let cs: Vec<char> = inner.chars().collect();
    let n = cs.len();
    let mut out = String::new();
    let mut copied = 0usize;
    let mut start = 0usize;
    for semi in semis.into_iter().chain(std::iter::once(n)) {
        let stmt_end = semi; // core excludes the `;`
        let mut s = start;
        while s < stmt_end && cs[s].is_whitespace() {
            s += 1;
        }
        if s < stmt_end {
            let sql_off = skip_leading_begin(&cs, s, stmt_end);
            let cand: String = cs[sql_off..stmt_end].iter().collect();
            let cand = cand.trim();
            let formattable =
                !cand.is_empty() && (is_query_text(cand) || embedded_query_split(cand).is_some());
            if formattable {
                if let Ok(f) = format(cand) {
                    out.extend(cs[copied..sql_off].iter()); // verbatim up to the SQL start
                    out.push_str(&indent_after_first(&f, column_of(&cs, sql_off)));
                    copied = (semi + 1).min(n); // `f` already ends with `;` — consume the original
                    start = (semi + 1).min(n);
                    continue;
                }
            }
        }
        start = (semi + 1).min(n);
    }
    out.extend(cs[copied..].iter());
    out
}

/// If the statement at `cs[s..end]` starts with the word `begin`, return the index of the first SQL
/// token after it (so the `begin` keyword stays verbatim); otherwise return `s`.
fn skip_leading_begin(cs: &[char], s: usize, end: usize) -> usize {
    let word: String = cs[s..end].iter().take_while(|c| c.is_alphabetic()).collect();
    if word.eq_ignore_ascii_case("begin") {
        let mut j = s + word.chars().count();
        while j < end && cs[j].is_whitespace() {
            j += 1;
        }
        j
    } else {
        s
    }
}

/// Column (chars since the previous newline) of char index `idx`.
fn column_of(cs: &[char], idx: usize) -> usize {
    let mut k = idx;
    let mut c = 0;
    while k > 0 && cs[k - 1] != '\n' {
        k -= 1;
        c += 1;
    }
    c
}

/// Keep `f`'s first line where it is, indent every following line by `indent` spaces.
fn indent_after_first(f: &str, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let mut out = String::new();
    for (i, line) in f.lines().enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(&pad);
        }
        out.push_str(line);
    }
    out
}

// ============================================================ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn msgs(e: &[FmtError]) -> Vec<&str> {
        e.iter().map(|m| m.msg.as_str()).collect()
    }

    // ---- auto-refactor (format чинит семейство алиасов вместо отказа) ----

    #[test]
    fn autofix_adds_table_alias() {
        // одна таблица без алиаса → получает короткий алиас (первая буква), колонки квалифицируются
        let out = format("select label as label from foo").unwrap();
        assert!(out.lines().any(|l| l.starts_with("  from foo f")), "{out}");
        assert!(out.contains("f.label as label"), "{out}");
    }

    #[test]
    fn autofix_qualifies_single_table_columns() {
        let out = format("select label, amount from accounts").unwrap();
        // (между выражением и `as` printer добавляет выравнивающие пробелы — проверяем по фрагментам)
        assert!(out.contains("a.label") && out.contains("as label"), "{out}");
        assert!(out.contains("a.amount") && out.contains("as amount"), "{out}");
        assert!(out.lines().any(|l| l.starts_with("  from accounts a")), "{out}");
    }

    #[test]
    fn autofix_names_select_items() {
        let out = format("select f.a from foo f").unwrap();
        assert!(out.contains("f.a as a"), "{out}");
    }

    #[test]
    fn autofix_names_multi_table_bare_column_no_error() {
        // при join'е голую колонку квалифицировать неоднозначно → НЕ ошибка: Refact даёт `kue as kue`
        let out = format("select d.pol as pol, kue from dual d join manual on true").unwrap();
        assert!(out.contains("kue") && out.contains("as kue"), "{out}"); // именована, не ошибка
        assert!(out.lines().any(|l| l.starts_with("  join manual m")), "{out}"); // алиас таблицы
    }

    #[test]
    fn unqualified_lines_flags_multi_table_bare_column() {
        // та же голая `kue` всплывает предупреждением валидации (single-table — нет: там Refact чинит)
        assert_eq!(
            unqualified_lines("select d.pol as pol\n     , kue\n  from dual d\n  join manual\n    on true"),
            vec![2]
        );
        assert!(unqualified_lines("select a from foo").is_empty()); // одна таблица → Refact квалифицирует
    }

    #[test]
    fn autofix_names_unnameable_item_as_column_n() {
        // агрегат/выражение без `as` теперь не ошибка — Refact сам даёт column1, column2, …
        let out = format("select count(f.x), sum(f.y) from foo f").unwrap();
        assert!(out.contains("count ( f.x)") && out.contains("as column1"), "{out}");
        assert!(out.contains("sum ( f.y)") && out.contains("as column2"), "{out}");
    }

    // ---- select * (формат пропускает; валидация даёт warning отдельно) ----

    #[test]
    fn format_allows_select_star() {
        let out = format("select * from foo").unwrap();
        assert!(out.contains('*'), "{out}");
        assert!(out.lines().any(|l| l.starts_with("  from foo f")), "{out}");
    }

    #[test]
    fn format_allows_qualified_star() {
        let out = format("select t.* from foo t").unwrap();
        assert!(out.contains("t.*"), "{out}");
    }

    #[test]
    fn select_star_lines_finds_stars_not_multiplication() {
        assert_eq!(select_star_lines("select *\nfrom t"), vec![1]);
        assert_eq!(select_star_lines("select a,\n  b.*\nfrom t"), vec![2]);
        assert!(select_star_lines("select a*b as c from t t").is_empty()); // умножение — не *
        assert!(select_star_lines("select count(*) from t t").is_empty()); // count(*) — не select *
    }

    // ---- FROM-подзапросы → CTE ----

    #[test]
    fn autofix_lifts_from_subquery_to_cte() {
        let out = format("select x.a as a from (select t.b as b from t t) x").unwrap();
        assert!(out.lines().next().unwrap() == "  with", "{out}");
        assert!(out.contains("x as ("), "{out}");
        assert!(out.contains("t.b as b"), "{out}"); // тело CTE
        assert!(out.lines().any(|l| l.starts_with("  from x x")), "{out}"); // ссылка на CTE
    }

    #[test]
    fn where_subquery_formats_without_corruption() {
        // WHERE-подзапрос Format больше не блокирует: форматирует как есть, не трогая внутренности
        let out = format("select f.a as a from foo f where f.a in (select g.x as x from bar g)")
            .unwrap();
        assert!(out.contains("in ( select g.x as x from bar g)"), "{out}"); // house `( …)` spacing
        assert!(out.lines().any(|l| l.starts_with("select f.a as a")), "{out}");
    }

    #[test]
    fn subquery_lines_flags_where_not_from() {
        // WHERE-подзапрос флагуется валидацией; FROM-подзапрос (его лифтит Format) — нет
        assert_eq!(subquery_lines("select f.a as a\nfrom foo f\nwhere f.a in (select g.x from bar g)"), vec![3]);
        assert!(subquery_lines("select x.a as a from (select t.b as b from t t) x").is_empty());
    }

    // ---- validate = только жёсткие ошибки (синтаксис/exists/не-SELECT); семейство алиасов чинит Refact ----

    #[test]
    fn validate_passes_autofixable_rules() {
        // то, что Refact чинит сам, валидацию НЕ роняет
        assert!(validate("select t.a as a from foo").is_ok()); // таблица без алиаса
        assert!(validate("select a as a from foo f").is_ok()); // колонка без квалификации (1 таблица)
        assert!(validate("select f.a from foo f").is_ok()); // элемент без `as`
    }

    #[test]
    fn rejects_exists() {
        let e = format("select f.a as a from foo f where exists (select g.x as x from bar g)")
            .unwrap_err();
        assert!(msgs(&e).iter().any(|m| m.contains("exists")), "{:?}", msgs(&e));
    }

    #[test]
    fn syntax_error_carries_source_position() {
        // лишняя `)` — позиция ошибки должна указывать именно на скобку
        let sql = "select f.a as a from foo f where f.a > 0)";
        let e = validate(sql).unwrap_err();
        let unmatched = e.iter().find(|x| x.msg.contains("unmatched")).expect("unmatched error");
        assert_eq!(sql.chars().nth(unmatched.pos), Some(')'), "{:?}", msgs(&e));
    }

    #[test]
    fn validate_ok_on_clean_query() {
        assert!(validate("select f.a as a from foo f").is_ok());
    }

    #[test]
    fn syntax_unmatched_paren() {
        let e = validate("select f.a as a from foo f where (f.a > 0").unwrap_err();
        assert!(msgs(&e).iter().any(|m| m.contains("unclosed `(`")), "{:?}", msgs(&e));
    }

    #[test]
    fn syntax_stray_close_paren() {
        let e = validate("select f.a as a from foo f where f.a > 0)").unwrap_err();
        assert!(msgs(&e).iter().any(|m| m.contains("unmatched `)`")), "{:?}", msgs(&e));
    }

    #[test]
    fn syntax_unterminated_string() {
        let e = validate("select f.a as a from foo f where f.s = 'oops").unwrap_err();
        assert!(msgs(&e).iter().any(|m| m.contains("unterminated string")), "{:?}", msgs(&e));
    }

    #[test]
    fn syntax_empty_where() {
        let e = validate("select f.a as a from foo f where").unwrap_err();
        assert!(msgs(&e).iter().any(|m| m.contains("expected a condition")), "{:?}", msgs(&e));
    }

    #[test]
    fn validate_clean_with_autofixable_table_alias() {
        // чистый синтаксис + только авто-чинимое нарушение (таблица без алиаса) → валидация ок
        assert!(validate("select f.a as a from foo").is_ok());
    }

    #[test]
    fn formats_simple_select() {
        let out = format("SELECT F.A as col1, F.B as col2 FROM foo F WHERE F.A > 0").unwrap();
        let lines: Vec<&str> = out.lines().collect();
        // keyword right-aligned to column 6, content on column 8, comma on column 6
        assert_eq!(lines[0], "select f.a as col1");
        assert!(lines.contains(&"     , f.b as col2"), "{out}");
        assert!(lines.iter().any(|l| l.starts_with("  from foo f")), "{out}");
        assert!(lines.iter().any(|l| l.starts_with(" where f.a > 0")), "{out}");
    }

    #[test]
    fn formats_with_cte() {
        let src = "with c as (select x.a as a from t x) select c.a as a from c c";
        let out = format(src).unwrap();
        assert!(out.lines().next().unwrap() == "  with", "{out}");
        assert!(out.contains("       c as ("), "{out}");
    }

    // ---- B (foundation): pass-through non-SELECT + format SQL inside $tag$ blocks ----

    #[test]
    fn passes_through_non_select_statements() {
        // DDL/DML не роняют Refact — остаются как есть
        let out = format("create table t ( a int)").unwrap();
        assert!(out.contains("create table t ( a int)"), "{out}");
    }

    #[test]
    fn formats_query_inside_dollar_block() {
        // полноценный SELECT внутри $sql$…$sql$ форматируется
        let out = format("perform p($sql$select f.a as a from foo f$sql$)").unwrap();
        assert!(out.contains("$sql$\nselect f.a as a"), "{out}");
        assert!(out.contains("  from foo f\n$sql$"), "{out}"); // без добавленного `;`
    }

    #[test]
    fn plpgsql_body_formats_sql_keeps_rest_verbatim() {
        // SQL внутри $$ begin … end $$ форматируется на месте; perform/analyze/begin/end — как есть
        let src = "create procedure x() language plpgsql as $$\n\
                   begin\n  \
                   create temp table change as select e.x as x from t e;\n  \
                   perform proc ( a, b);\n\
                   end;\n$$";
        let out = format(src).unwrap();
        assert!(out.contains("begin\n"), "{out}"); // begin сохранён
        assert!(out.contains("create temp table change as\n"), "{out}"); // префикс одной строкой
        assert!(out.contains("  select e.x as x"), "{out}"); // тело отформатировано, отступ 2
        assert!(out.contains("perform proc ( a, b);"), "{out}"); // perform — байт-в-байт
        assert!(out.contains("end;"), "{out}");
    }

    #[test]
    fn leaves_non_sql_dollar_fragment_verbatim() {
        // незавершённый/интерполированный фрагмент — не SQL → остаётся как есть
        let frag = "perform p($sql$ insert into t where x = $sql$ || v)";
        let out = format(frag).unwrap();
        assert!(out.contains("$sql$ insert into t where x = $sql$"), "{out}");
    }

    #[test]
    fn no_blank_lines_between_statements() {
        let out = format("select f.a as a from foo f; select g.b as b from bar g").unwrap();
        assert!(!out.contains("\n\n"), "лишняя пустая строка между операторами: {out:?}");
    }

    #[test]
    fn mixed_script_formats_select_passes_rest() {
        let out = format("create table t ( a int); select f.a as a from foo f").unwrap();
        assert!(out.contains("create table t ( a int)"), "{out}"); // verbatim
        assert!(out.lines().any(|l| l.starts_with("select f.a as a")), "{out}"); // formatted
    }

    #[test]
    fn formats_embedded_select_in_create_as() {
        let out = format("create temp table x as select a.b as b from t a").unwrap();
        assert!(out.lines().any(|l| l == "create temp table x as"), "{out}"); // префикс одной строкой
        assert!(out.lines().any(|l| l.starts_with("select a.b as b")), "{out}"); // тело отформатировано
        assert!(out.lines().any(|l| l.starts_with("  from t a")), "{out}");
    }

    #[test]
    fn formats_embedded_select_in_insert() {
        let out = format("INSERT INTO t (c) select a.c as c from u a").unwrap();
        assert!(out.lines().any(|l| l == "insert into t ( c)"), "{out}"); // префикс: lower + `( …)`
        assert!(out.lines().any(|l| l.starts_with("select a.c as c")), "{out}");
    }

    #[test]
    fn delete_with_scalar_subquery_passes_through() {
        // select внутри скобок — не тело statement'а; delete остаётся как есть
        let out = format("delete from t where t.a in (select u.a from u)").unwrap();
        assert!(out.contains("delete from t where t.a in (select u.a from u)"), "{out}");
    }

    // ---- A: house paren/function spacing `( … )` + casts ----

    #[test]
    fn paren_spacing_function_calls() {
        let out = format("select coalesce(f.a,0) as a from foo f").unwrap();
        assert!(out.contains("coalesce ( f.a, 0)"), "{out}"); // `func ( a, b)`
    }

    #[test]
    fn paren_spacing_empty_parens() {
        let out = format("select row_number() over() as n from foo f").unwrap();
        assert!(out.contains("over ( )"), "{out}"); // empty parens keep a space
    }

    #[test]
    fn cast_and_typed_literal_no_space() {
        let out = format("select null::boolean as b, f.d as d from foo f").unwrap();
        assert!(out.contains("null::boolean"), "{out}"); // no space around ::
    }

    // ---- C: set operations ----

    #[test]
    fn set_op_union_formats_each_select() {
        let out =
            format("select f.a as a from foo f union all select b.a as a from bar b").unwrap();
        assert!(out.lines().any(|l| l == " union all"), "{out}"); // op on its own line, col-6
        assert!(out.lines().filter(|l| l.starts_with("select ")).count() == 2, "{out}");
        assert!(out.lines().any(|l| l.starts_with("  from foo f")), "{out}");
        assert!(out.lines().any(|l| l.starts_with("  from bar b")), "{out}");
    }

    #[test]
    fn lowercases_keywords_keeps_string_case() {
        let out = format("SELECT F.A as a FROM foo F WHERE F.s = 'KeepMe'").unwrap();
        assert!(out.contains("'KeepMe'"), "{out}");
        assert!(out.contains("select"), "{out}");
    }
}
