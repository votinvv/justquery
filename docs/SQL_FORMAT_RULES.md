# JustQuery — SQL formatter rules (house style)

The "Refact" button (F9) reformats SQL to the **house style**. This is an authorial, opinionated
formatter — not a generic pretty-printer. It is built **case by case**: we point it at a real file,
see what's off, add the rule, and write it down here.

Engine: `src/sqlfmt.rs` (hand-written tokenizer + recursive-descent parser for SELECT/WITH, a
token-run printer, an auto-refactor pass, and a script orchestrator). The whole script is loaded
into memory — unlike the XML formatter there is no streaming (SQL scripts are never gigabytes).

Two surfaces share these rules:
- **Refact (F9 / button)** — rewrites the buffer in house style. Auto-fixes what it safely can;
  aborts only on hard errors (`exists`, syntax, …). Adds a trailing newline.
- **Validate (button)** — server `PREPARE` (executability) online, local syntax check offline,
  **plus** the "house-rule" findings that Refact does *not* auto-fix (see §6), as warnings.

Statuses: ✅ done · 🚧 partial · 📝 planned.

---

## 1. Statement routing (orchestrator) ✅

`format()` splits the script into top-level statements (on `;`, respecting `'…'`, `$tag$…$tag$`,
`-- …`, `/* … */`) and routes each:

| Statement | Behaviour |
|---|---|
| `select …` / `with …` | structurally formatted (§2) |
| `create … as select …`, `insert … ( cols) select/with …` | prefix → one line (lowercased, house parens); embedded `SELECT`/`WITH` formatted (§2). Scalar subqueries inside `( … )` are NOT the body. |
| any other (DDL / DML / `delete` / `update` / `grant` / `analyze` / `commit` / `perform` / `create procedure …`) | **passed through verbatim**, except SQL inside `$tag$…$tag$` (§5) |

Refact never errors on a non-SELECT script anymore; it formats what it understands and leaves the
rest untouched. Statements are re-joined with a **single** newline — no extra blank lines are
inserted. Trailing newlines at the end of the buffer are **preserved exactly as the user had them**
(none added, none removed).

---

## 2. Query layout (SELECT / WITH) ✅

- Leading clause keyword **right-aligned so its last letter is on column 6**; clause content starts
  on **column 8** (`  from`, ` where`, `select`).
- Select-list / CTE / column lists use a **leading comma on column 6**, item on column 8.
- All `as` aliases within one select are **aligned into a column**.
- `where` / `having` break on top-level `and` / `or`, each connector right-aligned to column 6.
- Everything lower-cased except string literals and `"quoted identifiers"`.
- CTEs: `with` on its own line; first CTE name on column 8; subsequent names with the leading comma
  on column 6; body printed indented; `)` closes the CTE.

---

## 3. Auto-refactor (Refact fixes these) ✅

Semantically-safe rewrites applied during formatting:

- **Table alias** — a FROM/JOIN table with no alias gets a short one (first letter of the name,
  de-duplicated): `from foo` → `from foo f`.
- **Single-table column qualification** — when there is exactly one source table, bare columns are
  qualified: `select name` → `select f.name`. (Multiple tables → can't disambiguate → §6 warning.)
- **Select-item naming** — every item gets `as …`. Derived from the column name (`f.a` → `as a`);
  if not derivable (aggregate / expression) → synthesized `as column1`, `as column2`, …
  `*` / `alias.*` are exempt (they are a §6 warning, never named).
- **FROM-subquery → CTE** — `from ( select … ) x` is lifted to `with x as ( … ) … from x x`
  (pure, equivalence-preserving). Subqueries with their own nested `with` are left as derived tables.

---

## 4. Spacing & tokens ✅

House paren style — `func ( args)`:
- a space **before** `(` (function call or keyword: `func (`, `in (`, `values (`, `with (`);
- a space **after** `(` (`( arg`);
- **no** space before `)` (`arg)`); an empty pair keeps the space (`( )`).
- `array[i]` — no space around the subscript brackets.

Casts / typed literals — no space: `null::boolean`, `date'2000-01-01'`, `interval'1d'`.

Commas: no space before, one space after. Binary operators get surrounding spaces (`a + b`).

---

## 5. Set operations ✅

`union` / `union all` / `intersect` / `except [all]` between SELECTs: the keyword sits on its own
line, right-aligned to column 6 (` union all`); each SELECT is formatted independently.

---

## 6. Rules moved to Validation (warnings, not Refact errors) ✅

Refact does **not** block on these; Validate reports them (with a line link) so the author decides:

- **`select *` / `alias.*`** — `STAR` warning ("list the columns explicitly").
- **subquery without `with`** in WHERE / scalar position — `SUBQUERY` warning ("lift it into a CTE").
  (FROM derived tables are auto-lifted by Refact, §3 — not warned.)
- **unqualified column under a join** (more than one source table, can't auto-qualify) — `QUALIFY`
  warning ("qualify it as `alias.column`").

`exists` / `not exists` and syntax errors remain **hard errors** that abort Refact.

---

## 7. plpgsql blocks `$$ begin … end $$` 🚧

`create procedure/function … as $$ … $$` is passed through (§1), but the dollar-quoted body is
descended into: a `begin … end` block is recognised and its **individual SQL statements**
(create-as-select / insert-select / select) are reformatted **in place**, while everything else
(`begin`/`end`, `perform`, `analyze`, `commit`, control flow, hand-aligned dynamic SQL) is kept
**byte-for-byte** so nicely hand-formatted code is never made worse.

📝 Planned refinements (case by case): `declare` sections, nested `begin`/`if`/`loop`/`case`,
canonical body indentation, `raise`/`return`/`open`/`fetch`.

---

## 8. Dollar-quoted SQL `$tag$ … $tag$` 🚧✅

Detection rule: try to parse the block content as SQL.
- a **complete** query (`$sql$ select … $sql$`) → formatted in place (no `;` added if absent);
- a plpgsql body (`$$ begin … end $$`) → §7;
- anything else (non-SQL text, or an **interpolated fragment** like
  `$sql$ … where x = $sql$ || v || $sql$ … $sql$`) → **left verbatim**.

📝 Planned: interpolated fragments — if a fragment is shorter than *X* chars, treat it as an inline
constant and keep analysing the surrounding text (TBD threshold).

---

## 9. Known limits

- **Interpolated dynamic SQL** (`$sql$ frag $sql$ || expr || $sql$ frag $sql$`) cannot be formatted:
  each fragment is incomplete SQL (cut mid-expression), not parseable standalone. Left as-is by
  design.
- **`delete` / `update`** are passed through (their WHERE/SET are not yet structured). 📝
- **`create table ( col type, … )`** column definitions are passed through (not yet leading-comma
  formatted). 📝
- Group-by / order-by split (`group` ⏎ `   by`), `left` ⏎ `join` + `on`/`and`, multi-line `case` and
  window `over ( … )` — 📝 planned (D / E).

---

## 10. Tests

All rules above are covered by unit tests in `src/sqlfmt.rs` (`mod tests`). When a new case is
added, add a focused test next to the related rule and update the status here.
