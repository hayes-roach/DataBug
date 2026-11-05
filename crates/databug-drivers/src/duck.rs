use crate::{infoschema, Batcher, Canceller, DbConnection, ExecOptions, Sink, SourceRef};
use async_trait::async_trait;
use databug_core::{Capabilities, Catalog, ColumnMeta, ConnectionConfig, DbObject, Dialect, ObjectKind, Plan, QueryEvent};
use duckdb::arrow::util::display::{ArrayFormatter, FormatOptions};
use duckdb::{AccessMode, Config, Connection};
use std::sync::{Arc, Mutex};

/// DuckDB runs in-process and is synchronous, so every call runs on Tokio's
/// blocking pool, like SQLite.
pub struct DuckConn {
    conn: Arc<Mutex<Connection>>,
    interrupt: Arc<duckdb::InterruptHandle>,
}

impl DuckConn {
    pub async fn connect(cfg: &ConnectionConfig) -> anyhow::Result<Self> {
        let path = cfg.path.trim().to_string();
        let read_only = cfg.read_only;
        let conn = tokio::task::spawn_blocking(move || -> anyhow::Result<Connection> {
            if path.is_empty() {
                anyhow::bail!("Choose a database file, or enter :memory: for a scratch database");
            }
            let config = Config::default().access_mode(if read_only { AccessMode::ReadOnly } else { AccessMode::Automatic })?;
            if path == ":memory:" {
                return Ok(Connection::open_in_memory_with_flags(config)?);
            }
            Ok(Connection::open_with_flags(&path, config)?)
        })
        .await??;
        let interrupt = conn.interrupt_handle();
        Ok(DuckConn { conn: Arc::new(Mutex::new(conn)), interrupt })
    }

    async fn blocking<T: Send + 'static>(&self, f: impl FnOnce(&Connection) -> anyhow::Result<T> + Send + 'static) -> anyhow::Result<T> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().map_err(|_| anyhow::anyhow!("connection poisoned"))?;
            f(&c)
        })
        .await?
    }
}

/// DuckDB answers INSERT/UPDATE/DELETE with a one-row `Count` result.
fn is_count_result(fields: &[String], modifies: bool) -> bool {
    modifies && fields.len() == 1 && fields[0].eq_ignore_ascii_case("count")
}

fn run_statement(c: &Connection, stmt: &str, limit: usize, sink: &Sink) -> anyhow::Result<()> {
    let modifies = databug_sql::classify(stmt, Dialect::Postgres).modifies();
    let mut st = c.prepare(stmt)?;
    let stream = st.stream_arrow([])?;
    let schema = stream.get_schema();
    let names: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
    if names.is_empty() {
        for b in stream {
            b?;
        }
        sink(QueryEvent::Affected(0));
        return Ok(());
    }
    if is_count_result(&names, modifies) {
        let mut n = 0u64;
        for b in stream {
            let b = b?;
            if b.num_rows() > 0 {
                let f = ArrayFormatter::try_new(b.column(0).as_ref(), &FormatOptions::default())?;
                n += f.value(0).to_string().parse::<u64>().unwrap_or(0);
            }
        }
        sink(QueryEvent::Affected(n));
        return Ok(());
    }
    let cols: Vec<ColumnMeta> = schema.fields().iter().map(|f| ColumnMeta::new(f.name().clone(), type_name(f.data_type()))).collect();
    let mut b = Batcher::new(sink, cols, limit);
    let opts = FormatOptions::default();
    'batches: for batch in stream {
        let batch = batch?;
        let fmts: Vec<ArrayFormatter> = batch.columns().iter().map(|a| ArrayFormatter::try_new(a.as_ref(), &opts)).collect::<Result<_, _>>()?;
        // logical nulls: a `NULL`-typed column has no null bitmap but is all nulls
        let nulls: Vec<_> = batch.columns().iter().map(|a| a.logical_nulls()).collect();
        for row in 0..batch.num_rows() {
            for (ci, f) in fmts.iter().enumerate() {
                if nulls[ci].as_ref().map(|n| n.is_null(row)).unwrap_or(false) {
                    b.cells().push(None);
                } else {
                    b.cells().push_with(|s| {
                        use std::fmt::Write;
                        let _ = write!(s, "{}", f.value(row));
                    });
                }
            }
            if !b.row_done() {
                break 'batches;
            }
        }
    }
    Ok(())
}

/// A short SQL-ish name for an Arrow type, for the grid header.
fn type_name(t: &duckdb::arrow::datatypes::DataType) -> String {
    use duckdb::arrow::datatypes::DataType as T;
    match t {
        T::Boolean => "boolean".into(),
        T::Int8 => "tinyint".into(),
        T::Int16 => "smallint".into(),
        T::Int32 => "integer".into(),
        T::Int64 => "bigint".into(),
        T::UInt8 => "utinyint".into(),
        T::UInt16 => "usmallint".into(),
        T::UInt32 => "uinteger".into(),
        T::UInt64 => "ubigint".into(),
        T::Float16 | T::Float32 => "float".into(),
        T::Float64 => "double".into(),
        T::Decimal128(p, s) | T::Decimal256(p, s) => format!("decimal({p},{s})"),
        T::Utf8 | T::LargeUtf8 | T::Utf8View => "varchar".into(),
        T::Binary | T::LargeBinary | T::BinaryView | T::FixedSizeBinary(_) => "blob".into(),
        T::Date32 | T::Date64 => "date".into(),
        T::Time32(_) | T::Time64(_) => "time".into(),
        T::Timestamp(_, Some(_)) => "timestamptz".into(),
        T::Timestamp(_, None) => "timestamp".into(),
        T::Interval(_) | T::Duration(_) => "interval".into(),
        T::List(_) | T::LargeList(_) | T::FixedSizeList(_, _) => "list".into(),
        T::Struct(_) => "struct".into(),
        T::Map(_, _) => "map".into(),
        T::Dictionary(_, v) => type_name(v),
        other => format!("{other}").to_lowercase(),
    }
}

const SYSTEM: &[&str] = &["information_schema", "pg_catalog"];

/// One row per index column for PRIMARY KEY / UNIQUE constraints and
/// `CREATE INDEX` indexes (whose columns DuckDB reports as expressions).
const INDEXES: &str = "SELECT schema_name, table_name, table_name || CASE constraint_type WHEN 'PRIMARY KEY' THEN '_pkey' ELSE '_key' || constraint_index END, 'true', (constraint_type = 'PRIMARY KEY')::VARCHAR, unnest(constraint_column_names) \
FROM duckdb_constraints() WHERE constraint_type IN ('PRIMARY KEY', 'UNIQUE') AND database_name = current_database() \
UNION ALL \
SELECT schema_name, table_name, index_name, is_unique::VARCHAR, is_primary::VARCHAR, trim(unnest(string_split(trim(CAST(expressions AS VARCHAR), '[]'), ',')), ' \"''') \
FROM duckdb_indexes() WHERE database_name = current_database() ORDER BY 1, 2, 3";

#[async_trait]
impl DbConnection for DuckConn {
    fn capabilities(&self) -> Capabilities {
        Capabilities { schemas: true, transactions: true, explain: true, explain_analyze: true, server_cancel: true, sql: true }
    }

    fn canceller(&self) -> Option<Canceller> {
        let h = self.interrupt.clone();
        Some(Arc::new(move || h.interrupt()))
    }

    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        let stmt = stmt.to_string();
        let limit = opts.row_limit;
        let sink = sink.clone();
        self.blocking(move |c| run_statement(c, &stmt, limit, &sink)).await
    }

    async fn introspect(&mut self) -> anyhow::Result<Catalog> {
        let default = crate::scalar(self, "SELECT current_schema()").await?.unwrap_or_else(|| "main".into());
        let mut cat = infoschema::introspect(self, infoschema::Opts { system_schemas: SYSTEM, catalog: Some("current_database()"), indexes: Some(INDEXES), default_schema: default }).await?;
        // sequences and macros (DuckDB's user-defined functions)
        let seqs = crate::collect(self, "SELECT schema_name, sequence_name FROM duckdb_sequences() WHERE database_name = current_database() AND NOT temporary").await.unwrap_or_default();
        let macros = crate::collect(self, "SELECT DISTINCT schema_name, function_name, function_type, array_to_string(parameters, ', ') FROM duckdb_functions() WHERE NOT internal AND database_name = current_database() AND function_type IN ('macro', 'table_macro')").await.unwrap_or_default();
        for r in seqs {
            if let Some(s) = cat.schemas.iter_mut().find(|s| s.name == crate::cell(&r, 0)) {
                s.objects.push(DbObject { name: crate::cell(&r, 1), kind: ObjectKind::Sequence, detail: String::new(), ident: String::new(), table: None });
            }
        }
        for r in macros {
            if let Some(s) = cat.schemas.iter_mut().find(|s| s.name == crate::cell(&r, 0)) {
                let kind = if crate::cell(&r, 2) == "table_macro" { ObjectKind::TableFunction } else { ObjectKind::Function };
                s.objects.push(DbObject { name: crate::cell(&r, 1), kind, detail: crate::cell(&r, 3), ident: String::new(), table: None });
            }
        }
        Ok(cat)
    }

    async fn object_source(&mut self, schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        let d = Dialect::Postgres;
        let (s, n) = (d.quote_literal(schema), d.quote_literal(&obj.name));
        let sql = match obj.kind {
            ObjectKind::View => format!("SELECT sql FROM duckdb_views() WHERE schema_name = {s} AND view_name = {n} AND database_name = current_database()"),
            ObjectKind::Sequence => format!("SELECT sql FROM duckdb_sequences() WHERE schema_name = {s} AND sequence_name = {n} AND database_name = current_database()"),
            ObjectKind::Function | ObjectKind::TableFunction => format!("SELECT macro_definition FROM duckdb_functions() WHERE schema_name = {s} AND function_name = {n} AND database_name = current_database() LIMIT 1"),
            k => anyhow::bail!("DuckDB has no {} objects", k.label()),
        };
        let def = crate::scalar(self, &sql).await?.ok_or_else(|| anyhow::anyhow!("{} not found", obj.name))?;
        let q = format!("{}.{}", d.quote_ident(schema), d.quote_ident(&obj.name));
        Ok(match obj.kind {
            ObjectKind::View => format!("{};\n", crate::replace_create(def.trim().trim_end_matches(';'), d, "CREATE OR REPLACE")),
            ObjectKind::Sequence => format!("-- DuckDB can't alter a sequence in place, so this drops and recreates it.\nDROP SEQUENCE IF EXISTS {q};\n\n{};\n", def.trim().trim_end_matches(';')),
            ObjectKind::TableFunction => format!("CREATE OR REPLACE MACRO {q}({}) AS TABLE\n{};\n", obj.detail, def.trim()),
            _ => format!("CREATE OR REPLACE MACRO {q}({}) AS\n{};\n", obj.detail, def.trim()),
        })
    }

    async fn explain(&mut self, stmt: &str, analyze: bool) -> anyhow::Result<Plan> {
        let modifies = databug_sql::classify(stmt, Dialect::Postgres).modifies();
        let sql = format!("EXPLAIN {}{stmt}", if analyze { "ANALYZE " } else { "" });
        // EXPLAIN ANALYZE runs the statement; keep writes from sticking.
        let wrap = analyze && modifies;
        self.blocking(move |c| {
            if wrap {
                c.execute_batch("BEGIN")?;
            }
            let res = (|| -> anyhow::Result<String> {
                let mut st = c.prepare(&sql)?;
                let mut rows = st.query([])?;
                let mut out = Vec::new();
                while let Some(r) = rows.next()? {
                    // (explain_key, explain_value): the value holds the drawn plan
                    let v: String = r.get(1)?;
                    out.push(v);
                }
                Ok(out.join("\n"))
            })();
            if wrap {
                let _ = c.execute_batch("ROLLBACK");
            }
            Ok(Plan { root: None, raw: res?, format: "text" })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_results_only_for_writes() {
        assert!(is_count_result(&["Count".into()], true));
        assert!(!is_count_result(&["count".into()], false));
        assert!(!is_count_result(&["Count".into(), "x".into()], true));
    }

    #[test]
    fn arrow_type_names() {
        use duckdb::arrow::datatypes::{DataType, TimeUnit};
        assert_eq!(type_name(&DataType::Int64), "bigint");
        assert_eq!(type_name(&DataType::Decimal128(12, 2)), "decimal(12,2)");
        assert_eq!(type_name(&DataType::Timestamp(TimeUnit::Microsecond, None)), "timestamp");
        assert_eq!(type_name(&DataType::Utf8), "varchar");
    }
}
