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
}

// ============================================================ parser + linter

struct Parser<'a> {
    t: &'a [Tok],
    p: usize,
    errs: Vec<FmtError>,
}

impl<'a> Parser<'a> {
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
    /// Report an error pointing at a specific token.
    fn err_tok(&mut self, t: &Tok, m: impl Into<String>) {
        self.errs.push(FmtError { pos: t.pos, len: t.len(), msg: m.into() });
    }
    /// Report an error spanning a run of tokens (selects the whole offending fragment).
    fn err_span(&mut self, toks: &[Tok], m: impl Into<String>) {
        match (toks.first(), toks.last()) {
            (Some(f), Some(l)) => {
                self.errs.push(FmtError { pos: f.pos, len: l.end() - f.pos, msg: m.into() })
            }
            _ => self.err(m),
        }
    }

    fn parse_query(&mut self) -> Query {
        let mut ctes = Vec::new();
        if self.peek().map_or(false, |t| t.kw("with")) {
            self.p += 1;
            if self.peek().map_or(false, |t| t.kw("recursive")) {
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
                if self.peek().map_or(false, |t| t.kw("as")) {
                    self.p += 1;
                } else {
                    self.err(format!("CTE `{name}` must be written `{name} as ( … )`"));
                }
                // the parenthesised CTE body
                let body = if self.peek().map_or(false, |t| t.is(Kind::LParen)) {
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
                if self.peek().map_or(false, |t| t.is(Kind::Comma)) {
                    self.p += 1;
                    continue;
                }
                break;
            }
        }
        let body = self.parse_select();
        Query { ctes, body }
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
        if !self.peek().map_or(false, |t| t.kw("select")) {
            self.err("only SELECT / WITH statements can be formatted");
            return s;
        }
        self.p += 1;
        if self.peek().map_or(false, |t| t.kw("distinct")) {
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
        // from
        if self.peek().map_or(false, |t| t.kw("from")) {
            self.p += 1;
            let from = self.collect_until(&["where", "group", "having", "order", "limit", "offset"]);
            if from.is_empty() {
                self.err("expected a table after `from`");
            } else {
                self.parse_from(&from, &mut s);
            }
        }
        if self.peek().map_or(false, |t| t.kw("where")) {
            self.p += 1;
            s.where_ = self.collect_until(&["group", "having", "order", "limit", "offset"]);
            if s.where_.is_empty() {
                self.err("expected a condition after `where`");
            }
        }
        if self.peek().map_or(false, |t| t.kw("group")) {
            self.p += 1;
            if self.peek().map_or(false, |t| t.kw("by")) {
                self.p += 1;
            }
            s.group_by = self.collect_until(&["having", "order", "limit", "offset"]);
            if s.group_by.is_empty() {
                self.err("expected an expression after `group by`");
            }
        }
        if self.peek().map_or(false, |t| t.kw("having")) {
            self.p += 1;
            s.having = self.collect_until(&["order", "limit", "offset"]);
            if s.having.is_empty() {
                self.err("expected a condition after `having`");
            }
        }
        if self.peek().map_or(false, |t| t.kw("order")) {
            self.p += 1;
            if self.peek().map_or(false, |t| t.kw("by")) {
                self.p += 1;
            }
            s.order_by = self.collect_until(&["limit", "offset"]);
            if s.order_by.is_empty() {
                self.err("expected an expression after `order by`");
            }
        }
        // limit / offset / anything trailing → kept verbatim
        while !self.at_end() && !self.peek().map_or(false, |t| t.is(Kind::Semi)) {
            let t = self.t[self.p].clone();
            s.tail.push(t);
            self.p += 1;
        }
        s
    }

    /// Collect tokens up to (but not consuming) a top-level keyword in `stops`, the end, or a `;`.
    /// A nested SELECT inside parens here means a subquery written without `with` → rule 1.
    fn collect_until(&mut self, stops: &[&str]) -> Vec<Tok> {
        let mut out = Vec::new();
        let mut depth = 0;
        while self.p < self.t.len() {
            let t = self.t[self.p].clone();
            if depth == 0 {
                if t.is(Kind::Semi) {
                    break;
                }
                if t.kind == Kind::Kw && stops.contains(&t.text.as_str()) {
                    break;
                }
            }
            match t.kind {
                Kind::LParen => {
                    // subquery detection: `( select … )` / `( with … )` that isn't a CTE
                    if self
                        .t
                        .get(self.p + 1)
                        .map_or(false, |n| n.kw("select") || n.kw("with"))
                    {
                        self.err("subquery written without a `with` — lift it into a CTE");
                    }
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
            _ => {
                let shown = render(raw);
                self.err_span(raw, format!("select item `{shown}` must be named with `as …`"));
                (raw.to_vec(), String::new())
            }
        };
        self.check_qualified(&expr);
        Item { expr, alias }
    }

    /// Rule 3: every column reference must be `alias.column`. Heuristic — flag a bare identifier
    /// that is not a function name, not a type, and not part of a qualified reference.
    fn check_qualified(&mut self, expr: &[Tok]) {
        let r: Vec<&Tok> = expr.iter().filter(|t| !t.is(Kind::Comment)).collect();
        for i in 0..r.len() {
            if r[i].kind != Kind::Ident {
                continue;
            }
            let prev = if i > 0 { Some(r[i - 1]) } else { None };
            let next = r.get(i + 1).copied();
            if next.map_or(false, |t| t.is(Kind::Dot)) {
                continue; // it's the alias in alias.column
            }
            if prev.map_or(false, |t| t.is(Kind::Dot)) {
                continue; // it's the column in alias.column
            }
            if next.map_or(false, |t| t.is(Kind::LParen)) {
                continue; // function call
            }
            if TYPES.contains(&r[i].text.as_str()) {
                continue; // a type name (in a cast etc.)
            }
            if prev.map_or(false, |t| t.kw("as") || (t.is(Kind::Op) && t.text == "::")) {
                continue; // cast target type
            }
            self.err_tok(
                r[i],
                format!("column `{}` must be qualified as `alias.{}`", r[i].text, r[i].text),
            );
        }
    }

    /// Parse the FROM token run into a base table + joins, enforcing alias rule + no subqueries.
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
            if head.first().map_or(false, |t| t.is(Kind::LParen)) {
                self.err_span(&head, "subquery in FROM — lift it into a CTE");
                continue;
            }
            // table ref = leading tokens until the alias; alias = last identifier
            let (table, alias) = split_table_alias(&head);
            if alias.is_empty() {
                let shown = render(&table);
                self.err_span(&table, format!("table `{shown}` must have an alias"));
            }
            self.check_qualified(&on);
            s.from.push(FromItem { join, table, alias, on });
        }
    }
}

/// If a join phrase starts at `i`, return (phrase, index past it). Handles
/// `join`, `inner/left/right/full [outer] join`, `cross join`, `natural … join`.
fn join_phrase(t: &[Tok], i: usize) -> Option<(String, usize)> {
    if !t.get(i).map_or(false, |x| x.kind == Kind::Kw) {
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
            Kind::RParen => {
                if open.pop().is_none() {
                    errs.push(FmtError { pos: t.pos, len: 1, msg: "unmatched `)`".into() });
                }
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
    // never a space before these
    if matches!(cur.kind, Kind::Comma | Kind::Dot | Kind::RParen | Kind::Semi) {
        return false;
    }
    if cur.is(Kind::Op) && cur.text == "::" {
        return false;
    }
    // never a space after these
    if matches!(prev.kind, Kind::Dot | Kind::LParen) {
        return false;
    }
    if prev.is(Kind::Op) && prev.text == "::" {
        return false;
    }
    // function call: name( with no gap. `keyword (` keeps the space (e.g. `in (`).
    if cur.is(Kind::LParen) && matches!(prev.kind, Kind::Ident | Kind::Qident) {
        return false;
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
    if toks.first().map_or(false, |t| t.kw("limit")) {
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
}

// ============================================================ entry points

/// Tokenize, split into statements and run the linter. Shared by [`validate`] and [`format`].
/// Returns the parsed statements (each with its leading-comment header) or the rule violations.
fn parse_all(sql: &str) -> Result<Vec<(Vec<String>, Query)>, Vec<FmtError>> {
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
                parsed.push((header, Query { ctes: Vec::new(), body: Select::default() }));
            }
            continue;
        }
        check_no_exists(&body, &mut errs);
        let mut p = Parser::new(&body);
        let q = p.parse_query();
        errs.append(&mut p.errs);
        parsed.push((header, q));
    }
    // de-duplicate while preserving order (the same rule can trip many times). The errors are sorted
    // by source position so the caller can jump to the *first* problem in the text.
    errs.dedup();
    errs.sort_by_key(|e| e.pos);
    if !errs.is_empty() {
        return Err(errs);
    }
    Ok(parsed)
}

/// Check `sql` against the house rules (and, later, Postgres-dialect correctness) WITHOUT
/// reformatting. Returns the violations, sorted by source position.
pub fn validate(sql: &str) -> Result<(), Vec<FmtError>> {
    parse_all(sql).map(|_| ())
}

/// Format `sql` in the house style. Runs [`validate`] first: any violation aborts the whole run
/// (the formatter is all-or-nothing) and is returned instead of formatted text.
pub fn format(sql: &str) -> Result<String, Vec<FmtError>> {
    let parsed = parse_all(sql)?;
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

// ============================================================ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn msgs(e: &[FmtError]) -> Vec<&str> {
        e.iter().map(|m| m.msg.as_str()).collect()
    }

    #[test]
    fn rejects_unaliased_table() {
        let e = format("select t.a as a from foo").unwrap_err();
        assert!(msgs(&e).iter().any(|m| m.contains("must have an alias")), "{:?}", msgs(&e));
    }

    #[test]
    fn rejects_unqualified_column() {
        let e = format("select a as a from foo f").unwrap_err();
        assert!(msgs(&e).iter().any(|m| m.contains("must be qualified")), "{:?}", msgs(&e));
    }

    #[test]
    fn rejects_missing_as() {
        let e = format("select f.a from foo f").unwrap_err();
        assert!(msgs(&e).iter().any(|m| m.contains("named with `as")), "{:?}", msgs(&e));
    }

    #[test]
    fn rejects_exists() {
        let e = format("select f.a as a from foo f where exists (select g.x as x from bar g)")
            .unwrap_err();
        assert!(msgs(&e).iter().any(|m| m.contains("exists")), "{:?}", msgs(&e));
    }

    #[test]
    fn rejects_subquery_without_with() {
        let e = format("select f.a as a from foo f where f.a in (select g.x as x from bar g)")
            .unwrap_err();
        assert!(msgs(&e).iter().any(|m| m.contains("subquery")), "{:?}", msgs(&e));
    }

    #[test]
    fn error_carries_source_position() {
        // the unqualified column `a` sits right after `select ` (char index 7)
        let e = validate("select a as a from foo f").unwrap_err();
        assert_eq!(e[0].pos, 7, "{:?}", msgs(&e));
        assert_eq!(e[0].len, 1);
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
    fn syntax_runs_with_house_rules_one_button() {
        // a clean-syntax query that still breaks a house rule (missing alias) → rule error surfaces
        let e = validate("select f.a as a from foo").unwrap_err();
        assert!(msgs(&e).iter().any(|m| m.contains("must have an alias")), "{:?}", msgs(&e));
    }

    #[test]
    fn formats_simple_select() {
        let out = format("SELECT F.A as col1, F.B as col2 FROM foo F WHERE F.A > 0").unwrap();
        let lines: Vec<&str> = out.lines().collect();
        // keyword right-aligned to column 6, content on column 8, comma on column 6
        assert_eq!(lines[0], "select f.a as col1");
        assert!(lines.iter().any(|l| *l == "     , f.b as col2"), "{out}");
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

    #[test]
    fn lowercases_keywords_keeps_string_case() {
        let out = format("SELECT F.A as a FROM foo F WHERE F.s = 'KeepMe'").unwrap();
        assert!(out.contains("'KeepMe'"), "{out}");
        assert!(out.contains("select"), "{out}");
    }
}
