//! PostgreSQL catalog introspection probes used by the metadata subsystem
//! ([`crate::meta_collector`] / [`crate::meta_details`]): schema and object lists, per-schema
//! change fingerprints, row-count budgeting and per-object column pulls. Every function takes a
//! live `postgres::Client` so callers (the background actors) reuse their persistent connections.

use crate::connections::err_chain;

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
