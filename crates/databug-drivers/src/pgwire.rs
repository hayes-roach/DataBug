//! Engines that speak the PostgreSQL wire protocol but have their own
//! catalogs: CockroachDB, TimescaleDB, QuestDB and H2 (in its `-pg` server
//! mode). They share `PgConn` for connecting and running statements; this
//! module holds what differs per engine.

use crate::pg::{pg_error, PgConn};
use crate::{cell, collect, collect_named, infoschema, scalar, SourceRef};
use databug_core::{Capabilities, Catalog, ColumnInfo, DbKind, DbObject, Dialect, IndexInfo, ObjectKind, Plan, SchemaInfo, TableInfo};
use tokio_postgres::SimpleQueryMessage;

pub(crate) fn capabilities(kind: DbKind) -> Capabilities {
    match kind {
        DbKind::QuestDb => Capabilities { schemas: false, transactions: false, explain: true, explain_analyze: false, server_cancel: false, sql: true },
        DbKind::H2 => Capabilities { schemas: true, transactions: true, explain: true, explain_analyze: true, server_cancel: false, sql: true },
        // Postgres, TimescaleDB, CockroachDB
        _ => Capabilities { schemas: true, transactions: true, explain: true, explain_analyze: true, server_cancel: true, sql: true },
    }
}

/// Statement that makes the session read-only, where the engine has one.
/// Elsewhere the app's read-only check is the only guard.
pub(crate) fn read_only_sql(kind: DbKind) -> Option<&'static str> {
    match kind {
        DbKind::Postgres | DbKind::TimescaleDb => Some("SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY"),
        DbKind::CockroachDb => Some("SET default_transaction_read_only = on"),
        _ => None,
    }
}

const CRDB_SYSTEM: &[&str] = &["information_schema", "pg_catalog", "crdb_internal", "pg_extension"];

const CRDB_INDEXES: &str = "SELECT s.table_schema, s.table_name, s.index_name, (s.non_unique = 'NO')::text, \
EXISTS (SELECT 1 FROM information_schema.table_constraints tc WHERE tc.constraint_type = 'PRIMARY KEY' AND tc.table_schema = s.table_schema AND tc.table_name = s.table_name AND tc.constraint_name = s.index_name)::text, s.column_name \
FROM information_schema.statistics s WHERE s.storing = 'NO' AND s.implicit = 'NO' AND s.table_schema NOT IN ('information_schema', 'pg_catalog', 'crdb_internal', 'pg_extension') \
ORDER BY 1, 2, 3, s.seq_in_index";

const H2_SYSTEM: &[&str] = &["INFORMATION_SCHEMA", "PG_CATALOG"];

const H2_INDEXES: &str = "SELECT i.TABLE_SCHEMA, i.TABLE_NAME, i.INDEX_NAME, \
CASE WHEN i.INDEX_TYPE_NAME IN ('PRIMARY KEY', 'UNIQUE INDEX') THEN 'true' ELSE 'false' END, \
CASE WHEN i.INDEX_TYPE_NAME = 'PRIMARY KEY' THEN 'true' ELSE 'false' END, c.COLUMN_NAME \
FROM INFORMATION_SCHEMA.INDEXES i JOIN INFORMATION_SCHEMA.INDEX_COLUMNS c ON c.INDEX_SCHEMA = i.INDEX_SCHEMA AND c.INDEX_NAME = i.INDEX_NAME AND c.TABLE_NAME = i.TABLE_NAME \
WHERE i.TABLE_SCHEMA NOT IN ('INFORMATION_SCHEMA', 'PG_CATALOG') ORDER BY 1, 2, 3, c.ORDINAL_POSITION";

/// Introspection for every PostgreSQL-protocol engine except Postgres itself.
pub(crate) async fn introspect(c: &mut PgConn) -> anyhow::Result<Catalog> {
    match c.kind {
        DbKind::TimescaleDb => {
            let mut cat = c.introspect_pg().await?;
            timescale_adjust(c, &mut cat).await;
            Ok(cat)
        }
        DbKind::CockroachDb => {
            let default = scalar(c, "SELECT current_schema()").await?.unwrap_or_else(|| "public".into());
            let mut cat = infoschema::introspect(c, infoschema::Opts { system_schemas: CRDB_SYSTEM, catalog: None, indexes: Some(CRDB_INDEXES), default_schema: default }).await?;
            let seqs = collect(c, "SELECT sequence_schema, sequence_name FROM information_schema.sequences").await.unwrap_or_default();
            add_objects(&mut cat, seqs, ObjectKind::Sequence);
            Ok(cat)
        }
        DbKind::H2 => {
            let default = scalar(c, "SELECT CURRENT_SCHEMA").await.ok().flatten().unwrap_or_else(|| "PUBLIC".into());
            let mut cat = infoschema::introspect(c, infoschema::Opts { system_schemas: H2_SYSTEM, catalog: None, indexes: Some(H2_INDEXES), default_schema: default }).await?;
            let seqs = collect(c, "SELECT SEQUENCE_SCHEMA, SEQUENCE_NAME FROM INFORMATION_SCHEMA.SEQUENCES WHERE SEQUENCE_SCHEMA NOT IN ('INFORMATION_SCHEMA', 'PG_CATALOG')").await.unwrap_or_default();
            add_objects(&mut cat, seqs, ObjectKind::Sequence);
            let trig = collect(c, "SELECT TRIGGER_SCHEMA, TRIGGER_NAME, EVENT_OBJECT_TABLE FROM INFORMATION_SCHEMA.TRIGGERS").await.unwrap_or_default();
            for r in trig {
                if let Some(s) = cat.schemas.iter_mut().find(|s| s.name == cell(&r, 0)) {
                    s.objects.push(DbObject { name: cell(&r, 1), kind: ObjectKind::Trigger, detail: format!("on {}", cell(&r, 2)), ident: String::new(), table: Some(cell(&r, 2)) });
                }
            }
            Ok(cat)
        }
        DbKind::QuestDb => questdb_introspect(c).await,
        k => anyhow::bail!("{} isn't a PostgreSQL-protocol engine", k.label()),
    }
}

fn add_objects(cat: &mut Catalog, rows: Vec<Vec<Option<String>>>, kind: ObjectKind) {
    for r in rows {
        let schema = cell(&r, 0);
        let si = match cat.schemas.iter().position(|s| s.name == schema) {
            Some(i) => i,
            None => {
                cat.schemas.push(SchemaInfo { name: schema, ..Default::default() });
                cat.schemas.len() - 1
            }
        };
        cat.schemas[si].objects.push(DbObject { name: cell(&r, 1), kind, detail: String::new(), ident: String::new(), table: None });
    }
}

/// Schemas TimescaleDB keeps its catalog, chunks and helper views in.
pub(crate) fn is_timescale_internal(schema: &str) -> bool {
    schema.starts_with("_timescaledb") || schema == "timescaledb_information" || schema == "timescaledb_experimental" || schema == "toolkit_experimental"
}

/// Hide TimescaleDB's internal schemas (chunks live there) and show
/// continuous aggregates as materialized views. Hypertables stay tables.
async fn timescale_adjust(c: &mut PgConn, cat: &mut Catalog) {
    cat.schemas.retain(|s| !is_timescale_internal(&s.name));
    let caggs = collect(c, "SELECT view_schema, view_name FROM timescaledb_information.continuous_aggregates").await.unwrap_or_default();
    for r in caggs {
        if let Some(t) = cat.schemas.iter_mut().find(|s| s.name == cell(&r, 0)).and_then(|s| s.tables.iter_mut().find(|t| t.name == cell(&r, 1))) {
            t.kind = ObjectKind::MaterializedView;
        }
    }
}

fn col_index(names: &[String], wanted: &[&str]) -> Option<usize> {
    names.iter().position(|n| wanted.iter().any(|w| n.eq_ignore_ascii_case(w)))
}

/// QuestDB has no schemas and no `information_schema`: list tables with
/// `SHOW TABLES` and columns with `SHOW COLUMNS`.
async fn questdb_introspect(c: &mut PgConn) -> anyhow::Result<Catalog> {
    let (names, rows) = collect_named(c, "SHOW TABLES").await?;
    let ti = col_index(&names, &["table_name", "name"]).unwrap_or(0);
    let mut tables = Vec::new();
    for r in rows.iter().take(1000) {
        let name = cell(r, ti);
        if name.is_empty() || name.starts_with("sys.") || name.starts_with("telemetry") {
            continue;
        }
        let mut t = TableInfo::new(name.clone(), ObjectKind::Table);
        if let Ok((cn, cols)) = collect_named(c, &format!("SHOW COLUMNS FROM {}", Dialect::Postgres.quote_ident(&name))).await {
            let (ci, ty, ix, des) = (col_index(&cn, &["column"]).unwrap_or(0), col_index(&cn, &["type"]).unwrap_or(1), col_index(&cn, &["indexed"]), col_index(&cn, &["designated"]));
            for col in cols {
                let cname = cell(&col, ci);
                if ix.map(|i| crate::truthy(col.get(i).and_then(|v| v.as_deref()))).unwrap_or(false) {
                    t.indexes.push(IndexInfo { name: format!("{cname}_idx"), columns: vec![cname.clone()], unique: false, primary: false });
                }
                let designated = des.map(|i| crate::truthy(col.get(i).and_then(|v| v.as_deref()))).unwrap_or(false);
                // the designated timestamp orders the table and can't be NULL
                t.columns.push(ColumnInfo { name: cname, data_type: cell(&col, ty).to_ascii_lowercase(), nullable: !designated, primary_key: false, default: None });
            }
        }
        tables.push(t);
    }
    tables.sort_by_cached_key(|t| t.name.to_lowercase());
    Ok(Catalog { default_schema: "public".into(), introspected_at: crate::now_secs(), schemas: vec![SchemaInfo { name: "public".into(), tables, objects: vec![] }] })
}

/// `EXPLAIN` for engines whose plans are text rather than Postgres JSON.
pub(crate) async fn explain(c: &mut PgConn, stmt: &str, analyze: bool) -> anyhow::Result<Plan> {
    let analyze = analyze && capabilities(c.kind).explain_analyze;
    let sql = match (c.kind, analyze) {
        (DbKind::CockroachDb, false) => format!("EXPLAIN (VERBOSE) {stmt}"),
        (_, true) => format!("EXPLAIN ANALYZE {stmt}"),
        (_, false) => format!("EXPLAIN {stmt}"),
    };
    // EXPLAIN ANALYZE runs the statement; keep writes from sticking.
    let wrap = analyze && databug_sql::classify(stmt, Dialect::Postgres).modifies();
    if wrap {
        c.client.simple_query("BEGIN").await.map_err(pg_error)?;
    }
    let res = c.client.simple_query(&sql).await.map_err(pg_error);
    if wrap {
        let _ = c.client.simple_query("ROLLBACK").await;
    }
    let mut lines = Vec::new();
    for m in res? {
        if let SimpleQueryMessage::Row(r) = m {
            // one line per row; CockroachDB's verbose plan has a single `info` column
            let parts: Vec<&str> = (0..r.len()).filter_map(|i| r.get(i)).collect();
            lines.push(parts.join("  "));
        }
    }
    Ok(Plan { root: None, raw: lines.join("\n"), format: "text" })
}

pub(crate) async fn object_source(c: &mut PgConn, schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
    let d = Dialect::Postgres;
    let q = format!("{}.{}", d.quote_ident(schema), d.quote_ident(&obj.name));
    match (c.kind, obj.kind) {
        (DbKind::TimescaleDb, ObjectKind::MaterializedView) => {
            let def = scalar(c, &format!("SELECT view_definition FROM timescaledb_information.continuous_aggregates WHERE view_schema = {} AND view_name = {}", d.quote_literal(schema), d.quote_literal(&obj.name))).await?;
            match def {
                Some(def) => Ok(format!(
                    "-- Continuous aggregates can't be replaced in place, so this drops and recreates it.\nDROP MATERIALIZED VIEW IF EXISTS {q};\n\nCREATE MATERIALIZED VIEW {q}\nWITH (timescaledb.continuous) AS\n{};\n",
                    def.trim_end().trim_end_matches(';')
                )),
                None => c.pg_object_source(schema, obj).await,
            }
        }
        (DbKind::TimescaleDb, _) => c.pg_object_source(schema, obj).await,
        (DbKind::CockroachDb, ObjectKind::View | ObjectKind::MaterializedView | ObjectKind::Sequence) => {
            let (names, rows) = collect_named(c, &format!("SHOW CREATE {q}")).await?;
            let i = col_index(&names, &["create_statement"]).unwrap_or(1);
            let def = rows.first().map(|r| cell(r, i)).filter(|s| !s.is_empty()).ok_or_else(|| anyhow::anyhow!("{q} not found"))?;
            Ok(match obj.kind {
                ObjectKind::View => format!("{};\n", crate::replace_create(def.trim_end_matches(';'), d, "CREATE OR REPLACE")),
                ObjectKind::MaterializedView => format!("-- Materialized views can't be replaced in place, so this drops and recreates it.\nDROP MATERIALIZED VIEW IF EXISTS {q};\n\n{};\n", def.trim_end_matches(';')),
                _ => format!("-- To change it, edit and run as ALTER SEQUENCE.\n{};\n", def.trim_end_matches(';')),
            })
        }
        (DbKind::H2, ObjectKind::View) => infoschema::view_source(c, d, schema, &obj.name).await,
        (DbKind::H2, ObjectKind::Sequence) => {
            let rows = collect(c, &format!("SELECT BASE_VALUE, INCREMENT, MINIMUM_VALUE, MAXIMUM_VALUE, CYCLE_OPTION FROM INFORMATION_SCHEMA.SEQUENCES WHERE SEQUENCE_SCHEMA = {} AND SEQUENCE_NAME = {}", d.quote_literal(schema), d.quote_literal(&obj.name))).await?;
            let r = rows.first().ok_or_else(|| anyhow::anyhow!("Sequence not found"))?;
            Ok(format!(
                "ALTER SEQUENCE {q}\n    RESTART WITH {}\n    INCREMENT BY {}\n    MINVALUE {}\n    MAXVALUE {}\n    {}CYCLE;\n",
                cell(r, 0),
                cell(r, 1),
                cell(r, 2),
                cell(r, 3),
                if cell(r, 4).eq_ignore_ascii_case("YES") { "" } else { "NO " }
            ))
        }
        (k, kind) => anyhow::bail!("Viewing the source of a {} isn't supported for {}", kind.label(), k.label()),
    }
}

pub(crate) async fn list_databases(c: &mut PgConn) -> anyhow::Result<(Vec<String>, String)> {
    match c.kind {
        DbKind::CockroachDb => {
            let names = collect(c, "SELECT database_name FROM [SHOW DATABASES] WHERE database_name NOT IN ('system') ORDER BY 1").await?;
            let current = scalar(c, "SELECT current_database()").await?.unwrap_or_default();
            Ok((names.into_iter().filter_map(|r| r.into_iter().next().flatten()).collect(), current))
        }
        DbKind::TimescaleDb => c.pg_list_databases().await,
        _ => Ok((Vec::new(), String::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timescale_internal_schemas() {
        assert!(is_timescale_internal("_timescaledb_internal"));
        assert!(is_timescale_internal("_timescaledb_catalog"));
        assert!(is_timescale_internal("timescaledb_information"));
        assert!(!is_timescale_internal("public"));
        assert!(!is_timescale_internal("metrics"));
    }

    #[test]
    fn honest_capabilities() {
        assert!(!capabilities(DbKind::QuestDb).transactions);
        assert!(!capabilities(DbKind::QuestDb).explain_analyze);
        assert!(capabilities(DbKind::CockroachDb).server_cancel);
        assert!(read_only_sql(DbKind::QuestDb).is_none());
        assert_eq!(read_only_sql(DbKind::CockroachDb), Some("SET default_transaction_read_only = on"));
    }

    #[test]
    fn finds_columns_by_name() {
        let names: Vec<String> = ["table_name", "designatedTimestamp"].iter().map(|s| s.to_string()).collect();
        assert_eq!(col_index(&names, &["name", "table_name"]), Some(0));
        assert_eq!(col_index(&names, &["DESIGNATEDTIMESTAMP"]), Some(1));
        assert_eq!(col_index(&names, &["x"]), None);
    }
}
