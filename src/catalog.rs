//! PostgreSQL catalog introspection probes used by the metadata subsystem
//! ([`crate::meta_collector`] / [`crate::meta_details`]): schema and object lists, per-schema
//! change fingerprints, row-count budgeting and per-object column pulls. Every function takes a
//! live `postgres::Client` so callers (the background actors) reuse their connections. Catalog
//! names travel as bound parameters (`$1`, `= ANY($1)`), never spliced into the SQL text.

use crate::connections::err_chain;
use postgres::types::ToSql;

/// Run a catalog query with bound parameters and map every row through `f` — the common shape of
/// every probe below. The error is the flattened driver error text.
fn query_map<T>(
    client: &mut postgres::Client,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
    f: impl Fn(&postgres::Row) -> T,
) -> Result<Vec<T>, String> {
    Ok(client
        .query(sql, params)
        .map_err(|e| err_chain(&e))?
        .iter()
        .map(f)
        .collect())
}

/// Schemas for the Metadata Manager dropdown + scan set. Includes the system schemas
/// `pg_catalog` / `information_schema` (lots of objects — useful to browse/test); only the internal
/// `pg_toast*` / `pg_temp*` are excluded (toast tables don't match our relkind filter anyway).
pub(crate) fn list_schemas(client: &mut postgres::Client) -> Result<Vec<String>, String> {
    let sql = "SELECT nspname::text FROM pg_namespace \
         WHERE nspname NOT LIKE 'pg_toast%' AND nspname NOT LIKE 'pg_temp%' ORDER BY 1";
    query_map(client, sql, &[], |r| r.get(0))
}

/// All objects in one schema as `(type-folder label, name)`. Functions carry their full signature
/// `name(argtypes)` (every overload listed). Per-schema so the collector can budget incrementally.
pub(crate) fn list_objects_in_schema(
    client: &mut postgres::Client,
    schema: &str,
) -> Result<Vec<(String, String)>, String> {
    let sql = "SELECT CASE c.relkind WHEN 'r' THEN 'Tables' WHEN 'p' THEN 'Tables' \
                     WHEN 'v' THEN 'Views' WHEN 'm' THEN 'Materialized Views' \
                     WHEN 'S' THEN 'Sequences' WHEN 'f' THEN 'Foreign Tables' END, c.relname::text \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind IN ('r','p','v','m','S','f') AND n.nspname::text = $1 \
         UNION ALL \
         SELECT 'Functions', p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')' \
         FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname::text = $1 \
         ORDER BY 1, 2";
    query_map(client, sql, &[&schema], |r| (r.get(0), r.get(1)))
}

/// One column as returned by the catalog probes: `(name, type, nullable, default)`.
pub(crate) type ColTuple = (String, String, bool, String);
/// One object with its columns: `(type-folder label, name, columns)`.
pub(crate) type ObjWithCols = (String, String, Vec<ColTuple>);

/// Parse one `name / format_type / attnotnull / default` row into a [`ColTuple`], reading from
/// column `base` on (shared by [`scan_schema`] and [`object_columns`]).
fn col_tuple(r: &postgres::Row, base: usize) -> ColTuple {
    let notnull: bool = r.get(base + 2);
    (r.get(base), r.get(base + 1), !notnull, r.get(base + 3))
}

/// The attribute columns every column probe selects (name, type, NOT NULL flag, default text),
/// over `pg_attribute a` LEFT JOIN `pg_attrdef d`.
const ATTR_COLS: &str = "a.attname::text, format_type(a.atttypid, a.atttypmod), a.attnotnull, \
     COALESCE(pg_get_expr(d.adbin, d.adrelid), '')";

/// Per-schema change fingerprint over the *monitored* schemas: one cheap query that md5-hashes the
/// identity + version (`xmin`) of every relation, attribute, default and function in each schema.
/// Because attribute/attrdef rows are folded in, the digest also changes on `ALTER COLUMN`/`ADD
/// COLUMN`/`SET DEFAULT` — not just create/drop. A schema whose objects all vanished simply drops
/// out of the result (callers treat a missing schema as the empty fingerprint).
pub(crate) fn schema_fingerprints(
    client: &mut postgres::Client,
    schemas: &[String],
) -> Result<std::collections::HashMap<String, String>, String> {
    let sql = "SELECT nspname::text, md5(string_agg(sig, '|' ORDER BY sig)) FROM ( \
           SELECT n.nspname AS nspname, \
                  'c:'||c.oid::text||':'||c.relkind::text||':'||c.relname||':'||c.xmin::text AS sig \
           FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
           WHERE c.relkind IN ('r','p','v','m','S','f') AND n.nspname::text = ANY($1) \
           UNION ALL \
           SELECT n.nspname, \
                  'a:'||a.attrelid::text||':'||a.attnum::text||':'||a.atttypid::text||':'|| \
                       a.atttypmod::text||':'||a.attnotnull::text||':'||a.xmin::text \
           FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid \
                JOIN pg_namespace n ON n.oid = c.relnamespace \
           WHERE c.relkind IN ('r','p','v','m','f') AND a.attnum > 0 AND NOT a.attisdropped \
                 AND n.nspname::text = ANY($1) \
           UNION ALL \
           SELECT n.nspname, \
                  'd:'||d.adrelid::text||':'||d.adnum::text||':'||d.xmin::text \
           FROM pg_attrdef d JOIN pg_class c ON c.oid = d.adrelid \
                JOIN pg_namespace n ON n.oid = c.relnamespace \
           WHERE n.nspname::text = ANY($1) \
           UNION ALL \
           SELECT n.nspname, \
                  'p:'||p.oid::text||':'||p.proname||':'||p.xmin::text \
           FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
           WHERE n.nspname::text = ANY($1) \
         ) s GROUP BY nspname";
    Ok(
        query_map(client, sql, &[&schemas], |r| (r.get(0), r.get(1)))?
            .into_iter()
            .collect(),
    )
}

/// Total `objects + attributes` held for the monitored schemas: relations + functions (objects)
/// plus every live attribute (columns). This is the figure the collector budgets against; it runs
/// before pulling anything so an oversized catalog can be refused cheaply.
pub(crate) fn count_meta_rows(
    client: &mut postgres::Client,
    schemas: &[String],
) -> Result<usize, String> {
    let sql = "SELECT \
          (SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
            WHERE c.relkind IN ('r','p','v','m','S','f') AND n.nspname::text = ANY($1)) \
        + (SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
            WHERE n.nspname::text = ANY($1)) \
        + (SELECT count(*) FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid \
            JOIN pg_namespace n ON n.oid = c.relnamespace \
            WHERE c.relkind IN ('r','p','v','m','f') AND a.attnum > 0 AND NOT a.attisdropped \
              AND n.nspname::text = ANY($1))";
    let total: Vec<i64> = query_map(client, sql, &[&schemas], |r| r.get(0))?;
    Ok(total.first().copied().unwrap_or(0).max(0) as usize)
}

/// One schema's full object set as `(type-folder label, name, columns)`. Relations carry their
/// columns `(name, type, nullable, default)`; sequences and functions carry an empty column list.
/// Functions list every overload with its signature `name(argtypes)`. This is the heavy per-schema
/// pull the collector runs only for schemas whose fingerprint diverged.
pub(crate) fn scan_schema(
    client: &mut postgres::Client,
    schema: &str,
) -> Result<Vec<ObjWithCols>, String> {
    // 1) the object list (relations + functions), folder + name
    let objs = list_objects_in_schema(client, schema)?;
    // 2) columns for every relation in the schema, in one query, grouped by relation name
    let cols_sql = format!(
        "SELECT c.relname::text, {ATTR_COLS} \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
              JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
              LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE n.nspname::text = $1 AND c.relkind IN ('r','p','v','m','f') \
         ORDER BY c.relname, a.attnum"
    );
    let mut cols: std::collections::HashMap<String, Vec<ColTuple>> =
        std::collections::HashMap::new();
    for (rel, col) in query_map(client, &cols_sql, &[&schema], |r| {
        (r.get::<_, String>(0), col_tuple(r, 1))
    })? {
        cols.entry(rel).or_default().push(col);
    }
    // 3) stitch columns onto their relation rows (functions/sequences keep the empty list)
    Ok(objs
        .into_iter()
        .map(|(kind, name)| {
            let c = cols.remove(&name).unwrap_or_default();
            (kind, name, c)
        })
        .collect())
}

/// A relation's columns as `(name, type, nullable, default)`. `Ok(None)` = the relation does not
/// exist (deleted). Client-taking so the details actor reuses its persistent connection.
pub(crate) fn object_columns(
    client: &mut postgres::Client,
    schema: &str,
    name: &str,
) -> Result<Option<Vec<ColTuple>>, String> {
    let oid_sql = "SELECT c.oid FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname::text = $1 AND c.relname::text = $2";
    let oids: Vec<postgres::types::Oid> =
        query_map(client, oid_sql, &[&schema, &name], |r| r.get(0))?;
    let Some(oid) = oids.first() else {
        return Ok(None); // relation gone
    };
    let sql = format!(
        "SELECT {ATTR_COLS} FROM pg_attribute a \
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped \
         ORDER BY a.attnum"
    );
    Ok(Some(query_map(client, &sql, &[oid], |r| col_tuple(r, 0))?))
}
