//! Introspection through the SQL-standard `INFORMATION_SCHEMA` views, for
//! engines whose own catalogs differ (CockroachDB, H2, DuckDB). Each engine
//! adds its own index query, since indexes aren't part of the standard.

use crate::{build_catalog, cell, collect, truthy, DbConnection, Extras};
use databug_core::{Catalog, ForeignKey, IndexInfo};

pub(crate) struct Opts<'a> {
    /// Schemas left out of the catalog (compared case-insensitively).
    pub system_schemas: &'a [&'a str],
    /// SQL expression naming the current catalog, to leave out other attached
    /// databases (DuckDB lists every attached file in its views).
    pub catalog: Option<&'a str>,
    /// Query returning `schema, table, index, unique, primary, column` for
    /// each index column, ordered by index and column position.
    pub indexes: Option<&'a str>,
    pub default_schema: String,
}

fn schema_filter(col: &str, o: &Opts<'_>) -> String {
    let list = o.system_schemas.iter().map(|s| format!("'{}'", s.to_ascii_uppercase().replace('\'', "''"))).collect::<Vec<_>>().join(", ");
    if list.is_empty() {
        "1 = 1".into()
    } else {
        format!("UPPER({col}) NOT IN ({list})")
    }
}

pub(crate) fn columns_sql(o: &Opts<'_>) -> String {
    let catalog = o.catalog.map(|c| format!(" AND c.TABLE_CATALOG = {c}")).unwrap_or_default();
    format!(
        "SELECT c.TABLE_SCHEMA, c.TABLE_NAME, t.TABLE_TYPE, c.COLUMN_NAME, c.DATA_TYPE, c.IS_NULLABLE, c.COLUMN_DEFAULT, \
CASE WHEN EXISTS (SELECT 1 FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE k \
ON k.CONSTRAINT_SCHEMA = tc.CONSTRAINT_SCHEMA AND k.CONSTRAINT_NAME = tc.CONSTRAINT_NAME AND k.TABLE_NAME = tc.TABLE_NAME \
WHERE tc.CONSTRAINT_TYPE = 'PRIMARY KEY' AND k.TABLE_SCHEMA = c.TABLE_SCHEMA AND k.TABLE_NAME = c.TABLE_NAME AND k.COLUMN_NAME = c.COLUMN_NAME) THEN 'true' ELSE 'false' END \
FROM INFORMATION_SCHEMA.COLUMNS c JOIN INFORMATION_SCHEMA.TABLES t ON t.TABLE_CATALOG = c.TABLE_CATALOG AND t.TABLE_SCHEMA = c.TABLE_SCHEMA AND t.TABLE_NAME = c.TABLE_NAME \
WHERE {}{catalog} ORDER BY c.TABLE_SCHEMA, c.TABLE_NAME, c.ORDINAL_POSITION",
        schema_filter("c.TABLE_SCHEMA", o)
    )
}

fn foreign_keys_sql(o: &Opts<'_>) -> String {
    format!(
        "SELECT k.TABLE_SCHEMA, k.TABLE_NAME, k.CONSTRAINT_NAME, k.COLUMN_NAME, u.TABLE_SCHEMA, u.TABLE_NAME, u.COLUMN_NAME, r.DELETE_RULE \
FROM INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS r \
JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE k ON k.CONSTRAINT_SCHEMA = r.CONSTRAINT_SCHEMA AND k.CONSTRAINT_NAME = r.CONSTRAINT_NAME \
JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE u ON u.CONSTRAINT_SCHEMA = r.UNIQUE_CONSTRAINT_SCHEMA AND u.CONSTRAINT_NAME = r.UNIQUE_CONSTRAINT_NAME AND u.ORDINAL_POSITION = k.POSITION_IN_UNIQUE_CONSTRAINT \
WHERE {} ORDER BY k.TABLE_SCHEMA, k.TABLE_NAME, k.CONSTRAINT_NAME, k.ORDINAL_POSITION",
        schema_filter("k.TABLE_SCHEMA", o)
    )
}

/// Group `schema, table, name, unique, primary, column` rows into indexes.
pub(crate) fn group_indexes(rows: Vec<Vec<Option<String>>>) -> Vec<(String, String, IndexInfo)> {
    let mut out: Vec<(String, String, IndexInfo)> = Vec::new();
    for r in rows {
        let (schema, table, name) = (cell(&r, 0), cell(&r, 1), cell(&r, 2));
        let col = cell(&r, 5);
        match out.last_mut() {
            Some((s, t, i)) if *s == schema && *t == table && i.name == name => {
                if !col.is_empty() {
                    i.columns.push(col);
                }
            }
            _ => out.push((schema, table, IndexInfo { name, unique: truthy(r.get(3).and_then(|v| v.as_deref())), primary: truthy(r.get(4).and_then(|v| v.as_deref())), columns: if col.is_empty() { vec![] } else { vec![col] } })),
        }
    }
    out
}

/// Group foreign key rows (one per column) into foreign keys.
pub(crate) fn group_foreign_keys(rows: Vec<Vec<Option<String>>>) -> Vec<(String, String, ForeignKey)> {
    let mut out: Vec<(String, String, ForeignKey)> = Vec::new();
    for r in rows {
        let (schema, table, name) = (cell(&r, 0), cell(&r, 1), cell(&r, 2));
        match out.last_mut() {
            Some((s, t, fk)) if *s == schema && *t == table && fk.name == name => {
                fk.columns.push(cell(&r, 3));
                fk.ref_columns.push(cell(&r, 6));
            }
            _ => {
                let rule = cell(&r, 7).to_ascii_uppercase();
                let on_delete = if rule == "NO ACTION" { String::new() } else { rule };
                out.push((schema, table, ForeignKey { name, columns: vec![cell(&r, 3)], ref_schema: cell(&r, 4), ref_table: cell(&r, 5), ref_columns: vec![cell(&r, 6)], on_delete }));
            }
        }
    }
    out
}

pub(crate) async fn introspect<C: DbConnection + ?Sized>(conn: &mut C, o: Opts<'_>) -> anyhow::Result<Catalog> {
    let rows = collect(conn, &columns_sql(&o)).await?;
    let catalog = o.catalog.map(|c| format!(" AND CATALOG_NAME = {c}")).unwrap_or_default();
    let schemas = collect(conn, &format!("SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA WHERE {}{catalog} ORDER BY 1", schema_filter("SCHEMA_NAME", &o))).await.unwrap_or_default();
    let mut extras = Extras::default();
    // Foreign keys and indexes are extras: an engine missing one of these
    // views still gets its tables and columns.
    if let Ok(fk) = collect(conn, &foreign_keys_sql(&o)).await {
        extras.foreign_keys = group_foreign_keys(fk);
    }
    if let Some(q) = o.indexes {
        if let Ok(ix) = collect(conn, q).await {
            extras.indexes = group_indexes(ix);
        }
    }
    let mut cat = build_catalog(rows, o.default_schema, schemas.into_iter().filter_map(|r| r.into_iter().next().flatten()).collect(), extras);
    for s in &mut cat.schemas {
        for t in &mut s.tables {
            // a primary key found through the constraint views but missing from the index query
            if !t.indexes.iter().any(|i| i.primary) {
                let pk: Vec<String> = t.columns.iter().filter(|c| c.primary_key).map(|c| c.name.clone()).collect();
                if !pk.is_empty() {
                    t.indexes.insert(0, IndexInfo { name: format!("{}_pkey", t.name), columns: pk, unique: true, primary: true });
                }
            }
        }
    }
    Ok(cat)
}

/// `CREATE VIEW` text from `INFORMATION_SCHEMA.VIEWS`.
pub(crate) async fn view_source<C: DbConnection + ?Sized>(conn: &mut C, d: databug_core::Dialect, schema: &str, name: &str) -> anyhow::Result<String> {
    let def = crate::scalar(conn, &format!("SELECT VIEW_DEFINITION FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {}", d.quote_literal(schema), d.quote_literal(name)))
        .await?
        .ok_or_else(|| anyhow::anyhow!("View {schema}.{name} not found"))?;
    let def = def.trim().trim_end_matches(';');
    // some engines store the whole statement, others only the query
    if def.to_ascii_uppercase().starts_with("CREATE") {
        Ok(format!("{};\n", crate::replace_create(def, d, "CREATE OR REPLACE")))
    } else {
        Ok(format!("CREATE OR REPLACE VIEW {}.{} AS\n{def};\n", d.quote_ident(schema), d.quote_ident(name)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(v: &[&str]) -> Vec<Option<String>> {
        v.iter().map(|s| if s.is_empty() { None } else { Some(s.to_string()) }).collect()
    }

    #[test]
    fn groups_index_columns_in_order() {
        let ix = group_indexes(vec![
            row(&["public", "orders", "orders_pkey", "true", "true", "id"]),
            row(&["public", "orders", "orders_by_customer", "false", "false", "customer_id"]),
            row(&["public", "orders", "orders_by_customer", "false", "false", "placed_at"]),
            row(&["public", "items", "orders_by_customer", "false", "false", "x"]),
        ]);
        assert_eq!(ix.len(), 3);
        assert!(ix[0].2.primary && ix[0].2.unique);
        assert_eq!(ix[1].2.columns, vec!["customer_id", "placed_at"]);
        assert_eq!(ix[2].1, "items");
    }

    #[test]
    fn groups_composite_foreign_keys() {
        let fk = group_foreign_keys(vec![
            row(&["s", "order_items", "fk_order", "order_id", "s", "orders", "id", "CASCADE"]),
            row(&["s", "order_items", "fk_prod", "product_id", "s", "products", "id", "NO ACTION"]),
            row(&["s", "order_items", "fk_prod", "variant", "s", "products", "variant", "NO ACTION"]),
        ]);
        assert_eq!(fk.len(), 2);
        assert_eq!(fk[0].2.on_delete, "CASCADE");
        assert_eq!(fk[1].2.columns, vec!["product_id", "variant"]);
        assert_eq!(fk[1].2.ref_columns, vec!["id", "variant"]);
        assert_eq!(fk[1].2.on_delete, "");
    }

    #[test]
    fn column_query_filters_system_schemas_and_catalog() {
        let o = Opts { system_schemas: &["information_schema", "pg_catalog"], catalog: Some("current_database()"), indexes: None, default_schema: "main".into() };
        let q = columns_sql(&o);
        assert!(q.contains("UPPER(c.TABLE_SCHEMA) NOT IN ('INFORMATION_SCHEMA', 'PG_CATALOG')"));
        assert!(q.contains("c.TABLE_CATALOG = current_database()"));
    }
}
