//! IBM Db2 and Spark SQL (Thrift server) through ODBC. Building needs the
//! ODBC driver manager (unixODBC on macOS/Linux; built into Windows), and
//! connecting needs the engine's ODBC driver: the IBM Data Server Driver for
//! ODBC and CLI for Db2, the Simba (or Databricks) Spark ODBC driver for Spark.

use crate::infoschema::{group_foreign_keys, group_indexes};
use crate::{build_catalog, cell, collect, collect_named, plan_text_rows, scalar, tx_command, Batcher, Canceller, DbConnection, ExecOptions, Extras, Sink, SourceRef, TxCommand};
use async_trait::async_trait;
use databug_core::{Capabilities, Catalog, ColumnInfo, ColumnMeta, ConnectionConfig, DbKind, DbObject, Dialect, ObjectKind, Plan, QueryEvent, SchemaInfo, SslMode, TableInfo};
use odbc_api::buffers::TextRowSet;
use odbc_api::{ConnectionOptions, Cursor, ResultSetMetadata};
use std::sync::{Arc, Mutex};

pub struct OdbcConn {
    conn: Arc<Mutex<odbc_api::Connection<'static>>>,
    kind: DbKind,
    in_tx: bool,
}

/// Wrap a connection-string value in braces when it needs it (`;`, `{`, `}`).
pub(crate) fn odbc_value(v: &str) -> String {
    if v.contains([';', '{', '}', '=']) || v.starts_with(' ') || v.ends_with(' ') {
        format!("{{{}}}", v.replace('}', "}}"))
    } else {
        v.to_string()
    }
}

/// The ODBC connection string. Built in memory for each connect; the
/// password is never saved with it.
pub(crate) fn connection_string(cfg: &ConnectionConfig, password: &str) -> String {
    let driver = if !cfg.odbc_driver.trim().is_empty() {
        cfg.odbc_driver.trim().to_string()
    } else if cfg.kind == DbKind::Db2 {
        "IBM DB2 ODBC DRIVER".into()
    } else {
        "Simba Spark ODBC Driver".into()
    };
    let mut parts = vec![format!("DRIVER={{{driver}}}")];
    if cfg.kind == DbKind::Db2 {
        parts.push(format!("HOSTNAME={}", odbc_value(cfg.host.trim())));
        parts.push(format!("PORT={}", cfg.port));
        parts.push("PROTOCOL=TCPIP".into());
        parts.push(format!("DATABASE={}", odbc_value(cfg.database.trim())));
        if cfg.ssl == SslMode::Require {
            parts.push("SECURITY=SSL".into());
        }
    } else {
        // Spark Thrift server (HiveServer2 protocol), SASL transport;
        // `odbc_options` can override any of these.
        parts.push(format!("HOST={}", odbc_value(cfg.host.trim())));
        parts.push(format!("PORT={}", cfg.port));
        parts.push("SparkServerType=3".into());
        parts.push("ThriftTransport=1".into());
        parts.push(format!("AuthMech={}", if password.is_empty() { 2 } else { 3 }));
        if !cfg.database.trim().is_empty() {
            parts.push(format!("Schema={}", odbc_value(cfg.database.trim())));
        }
        if cfg.ssl == SslMode::Require {
            parts.push("SSL=1".into());
        }
    }
    if !cfg.user.is_empty() {
        parts.push(format!("UID={}", odbc_value(&cfg.user)));
    }
    if !password.is_empty() {
        parts.push(format!("PWD={}", odbc_value(password)));
    }
    let extra = cfg.odbc_options.trim().trim_matches(';');
    if !extra.is_empty() {
        parts.push(extra.to_string());
    }
    parts.join(";") + ";"
}

fn run(conn: &odbc_api::Connection<'static>, stmt: &str, limit: usize, sink: &Sink) -> anyhow::Result<()> {
    let mut pre = conn.preallocate()?;
    let sql = stmt.trim().trim_end_matches(';');
    let had_rows = match pre.execute(sql, ())? {
        Some(mut cursor) => {
            let names: Vec<String> = cursor.column_names()?.collect::<Result<_, _>>()?;
            let mut types = Vec::with_capacity(names.len());
            for i in 1..=names.len() as u16 {
                types.push(cursor.col_data_type(i).map(|t| format!("{t:?}").to_ascii_lowercase()).unwrap_or_default());
            }
            let width = names.len();
            let cols = names.into_iter().zip(types).map(|(n, t)| ColumnMeta::new(n, t)).collect();
            let mut b = Batcher::new(sink, cols, limit);
            let buffer = TextRowSet::for_cursor(500, &mut cursor, Some(64 * 1024))?;
            let mut rc = cursor.bind_buffer(buffer)?;
            'fetch: while let Some(batch) = rc.fetch()? {
                for r in 0..batch.num_rows() {
                    for c in 0..width {
                        b.cells().push(batch.at_as_str(c, r).ok().flatten());
                    }
                    if !b.row_done() {
                        break 'fetch;
                    }
                }
            }
            true
        }
        None => false,
    };
    if !had_rows {
        let n = pre.row_count()?.unwrap_or(0);
        sink(QueryEvent::Affected(n as u64));
    }
    Ok(())
}

impl OdbcConn {
    pub async fn connect(cfg: &ConnectionConfig, password: &str) -> anyhow::Result<Self> {
        let cs = connection_string(cfg, password);
        let kind = cfg.kind;
        let conn = tokio::task::spawn_blocking(move || -> anyhow::Result<odbc_api::Connection<'static>> {
            let env = odbc_api::environment()?;
            Ok(env.connect_with_connection_string(&cs, ConnectionOptions::default())?)
        })
        .await?
        .map_err(|e| anyhow::anyhow!("{e:#}\n\nCheck that the {} ODBC driver is installed and registered with the ODBC driver manager (see the README).", if kind == DbKind::Db2 { "IBM Db2" } else { "Spark" }))?;
        Ok(OdbcConn { conn: Arc::new(Mutex::new(conn)), kind, in_tx: false })
    }

    async fn blocking<T: Send + 'static>(&self, f: impl FnOnce(&odbc_api::Connection<'static>) -> anyhow::Result<T> + Send + 'static) -> anyhow::Result<T> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().map_err(|_| anyhow::anyhow!("connection poisoned"))?;
            f(&c)
        })
        .await?
    }
}

/// Db2's catalog keeps index columns as `+COL1-COL2` (sign = sort order).
pub(crate) fn db2_colnames(s: &str) -> Vec<String> {
    s.split(['+', '-']).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect()
}

const DB2_SYS: &str = "NOT (TABSCHEMA LIKE 'SYS%' OR TABSCHEMA IN ('NULLID', 'SQLJ'))";

const DB2_COLUMNS: &str = "SELECT RTRIM(c.TABSCHEMA), c.TABNAME, CASE t.TYPE WHEN 'V' THEN 'VIEW' WHEN 'S' THEN 'MATVIEW' ELSE 'TABLE' END, c.COLNAME, \
LOWER(c.TYPENAME) || CASE WHEN c.TYPENAME IN ('VARCHAR', 'CHARACTER', 'VARGRAPHIC', 'GRAPHIC') THEN '(' || RTRIM(CHAR(c.LENGTH)) || ')' WHEN c.TYPENAME = 'DECIMAL' THEN '(' || RTRIM(CHAR(c.LENGTH)) || ',' || RTRIM(CHAR(c.SCALE)) || ')' ELSE '' END, \
CASE c.NULLS WHEN 'Y' THEN 'true' ELSE 'false' END, c.DEFAULT, CASE WHEN c.KEYSEQ IS NOT NULL AND c.KEYSEQ > 0 THEN 'true' ELSE 'false' END \
FROM SYSCAT.COLUMNS c JOIN SYSCAT.TABLES t ON t.TABSCHEMA = c.TABSCHEMA AND t.TABNAME = c.TABNAME \
WHERE t.TYPE IN ('T', 'V', 'S') AND NOT (c.TABSCHEMA LIKE 'SYS%' OR c.TABSCHEMA IN ('NULLID', 'SQLJ')) ORDER BY 1, 2, c.COLNO";

async fn db2_introspect(c: &mut OdbcConn) -> anyhow::Result<Catalog> {
    let rows = collect(c, DB2_COLUMNS).await?;
    let schemas = collect(c, "SELECT RTRIM(SCHEMANAME) FROM SYSCAT.SCHEMATA WHERE NOT (SCHEMANAME LIKE 'SYS%' OR SCHEMANAME IN ('NULLID', 'SQLJ')) ORDER BY 1").await.unwrap_or_default();
    let mut extras = Extras::default();
    if let Ok(ix) = collect(c, &format!("SELECT RTRIM(TABSCHEMA), TABNAME, INDNAME, CASE WHEN UNIQUERULE IN ('P', 'U') THEN 'true' ELSE 'false' END, CASE UNIQUERULE WHEN 'P' THEN 'true' ELSE 'false' END, COLNAMES FROM SYSCAT.INDEXES WHERE {DB2_SYS}")).await {
        // one row per index: expand COLNAMES into one row per column
        let mut expanded = Vec::new();
        for r in ix {
            for col in db2_colnames(&cell(&r, 5)) {
                let mut row = r[..5].to_vec();
                row.push(Some(col));
                expanded.push(row);
            }
        }
        extras.indexes = group_indexes(expanded);
    }
    if let Ok(fks) = collect(c, &format!("SELECT RTRIM(TABSCHEMA), TABNAME, CONSTNAME, FK_COLNAMES, RTRIM(REFTABSCHEMA), REFTABNAME, PK_COLNAMES, DELETERULE FROM SYSCAT.REFERENCES WHERE {DB2_SYS}")).await {
        let mut expanded = Vec::new();
        for r in fks {
            let rule = match cell(&r, 7).as_str() {
                "C" => "CASCADE",
                "N" => "SET NULL",
                "R" => "RESTRICT",
                _ => "NO ACTION",
            };
            // FK_COLNAMES / PK_COLNAMES are space-separated
            for (fc, pc) in cell(&r, 3).split_whitespace().zip(cell(&r, 6).split_whitespace()) {
                expanded.push(vec![r[0].clone(), r[1].clone(), r[2].clone(), Some(fc.into()), r[4].clone(), r[5].clone(), Some(pc.into()), Some(rule.into())]);
            }
        }
        extras.foreign_keys = group_foreign_keys(expanded);
    }
    let objects = "SELECT RTRIM(ROUTINESCHEMA), ROUTINENAME, CASE WHEN ROUTINETYPE = 'P' THEN 'P' WHEN FUNCTIONTYPE = 'T' THEN 'T' ELSE 'F' END, SPECIFICNAME FROM SYSCAT.ROUTINES WHERE NOT (ROUTINESCHEMA LIKE 'SYS%' OR ROUTINESCHEMA IN ('NULLID', 'SQLJ')) AND ORIGIN IN ('Q', 'E') \
UNION ALL SELECT RTRIM(TRIGSCHEMA), TRIGNAME, 'TR', TABNAME FROM SYSCAT.TRIGGERS WHERE NOT (TRIGSCHEMA LIKE 'SYS%') \
UNION ALL SELECT RTRIM(SEQSCHEMA), SEQNAME, 'S', '' FROM SYSCAT.SEQUENCES WHERE SEQTYPE = 'S' AND NOT (SEQSCHEMA LIKE 'SYS%' OR SEQSCHEMA IN ('NULLID', 'SQLJ'))";
    for r in collect(c, objects).await.unwrap_or_default() {
        let (kind, ident, table) = match cell(&r, 2).as_str() {
            "P" => (ObjectKind::Procedure, cell(&r, 3), None),
            "T" => (ObjectKind::TableFunction, cell(&r, 3), None),
            "F" => (ObjectKind::Function, cell(&r, 3), None),
            "TR" => (ObjectKind::Trigger, String::new(), Some(cell(&r, 3))),
            _ => (ObjectKind::Sequence, String::new(), None),
        };
        let detail = table.as_ref().map(|t| format!("on {t}")).unwrap_or_default();
        extras.objects.push((cell(&r, 0), DbObject { name: cell(&r, 1), kind, detail, ident, table }));
    }
    let default = scalar(c, "VALUES RTRIM(CURRENT SCHEMA)").await?.unwrap_or_default();
    Ok(build_catalog(rows, default, schemas.into_iter().filter_map(|r| r.into_iter().next().flatten()).collect(), extras))
}

/// Spark SQL has no information schema: list namespaces, tables and views
/// with SHOW, and columns with DESCRIBE (stopping at the partition section).
async fn spark_introspect(c: &mut OdbcConn) -> anyhow::Result<Catalog> {
    let d = Dialect::MySql;
    let default = scalar(c, "SELECT current_database()").await?.unwrap_or_else(|| "default".into());
    let schema_rows = collect(c, "SHOW SCHEMAS").await?;
    let mut schemas: Vec<SchemaInfo> = schema_rows.into_iter().filter_map(|r| r.into_iter().next().flatten()).map(|name| SchemaInfo { name, ..Default::default() }).collect();
    let mut described = 0;
    for s in schemas.iter_mut().take(50) {
        let views: Vec<String> = collect(c, &format!("SHOW VIEWS IN {}", d.quote_ident(&s.name))).await.unwrap_or_default().into_iter().map(|r| cell(&r, 1)).collect();
        let (names, rows) = collect_named(c, &format!("SHOW TABLES IN {}", d.quote_ident(&s.name))).await.unwrap_or_default();
        let ti = names.iter().position(|n| n.eq_ignore_ascii_case("tableName")).unwrap_or(1);
        for r in rows {
            let name = cell(&r, ti);
            let mut t = TableInfo::new(name.clone(), if views.contains(&name) { ObjectKind::View } else { ObjectKind::Table });
            if described < 300 {
                described += 1;
                for col in collect(c, &format!("DESCRIBE TABLE {}.{}", d.quote_ident(&s.name), d.quote_ident(&name))).await.unwrap_or_default() {
                    let cname = cell(&col, 0);
                    if cname.is_empty() || cname.starts_with('#') {
                        break;
                    }
                    t.columns.push(ColumnInfo { name: cname, data_type: cell(&col, 1), nullable: true, primary_key: false, default: None });
                }
            }
            s.tables.push(t);
        }
    }
    schemas.sort_by(|a, b| (a.name != default).cmp(&(b.name != default)).then(a.name.cmp(&b.name)));
    Ok(Catalog { default_schema: default, introspected_at: crate::now_secs(), schemas })
}

#[async_trait]
impl DbConnection for OdbcConn {
    fn capabilities(&self) -> Capabilities {
        match self.kind {
            // Db2 needs explain tables created first, so EXPLAIN isn't offered
            DbKind::Db2 => Capabilities { schemas: true, transactions: true, explain: false, explain_analyze: false, server_cancel: false, sql: true },
            _ => Capabilities { schemas: true, transactions: false, explain: true, explain_analyze: false, server_cancel: false, sql: true },
        }
    }

    fn canceller(&self) -> Option<Canceller> {
        None
    }

    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        if let Some(cmd) = tx_command(stmt) {
            if self.kind != DbKind::Db2 {
                anyhow::bail!("Spark SQL has no transactions");
            }
            let begin = cmd == TxCommand::Begin;
            self.blocking(move |c| {
                match cmd {
                    TxCommand::Begin => c.set_autocommit(false)?,
                    TxCommand::Commit => {
                        c.commit()?;
                        c.set_autocommit(true)?;
                    }
                    TxCommand::Rollback => {
                        c.rollback()?;
                        c.set_autocommit(true)?;
                    }
                }
                Ok(())
            })
            .await?;
            self.in_tx = begin;
            sink(QueryEvent::Message(if begin { "Transaction started" } else { "Transaction ended" }.into()));
            return Ok(());
        }
        let (stmt, limit, sink) = (stmt.to_string(), opts.row_limit, sink.clone());
        self.blocking(move |c| run(c, &stmt, limit, &sink)).await
    }

    async fn introspect(&mut self) -> anyhow::Result<Catalog> {
        match self.kind {
            DbKind::Db2 => db2_introspect(self).await,
            _ => spark_introspect(self).await,
        }
    }

    async fn object_source(&mut self, schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        if self.kind == DbKind::Spark {
            if obj.kind != ObjectKind::View {
                anyhow::bail!("Spark SQL shows the source of views only");
            }
            let d = Dialect::MySql;
            let def = scalar(self, &format!("SHOW CREATE TABLE {}.{}", d.quote_ident(schema), d.quote_ident(&obj.name))).await?.ok_or_else(|| anyhow::anyhow!("View not found"))?;
            return Ok(format!("{};\n", crate::replace_create(def.trim().trim_end_matches(';'), d, "CREATE OR REPLACE")));
        }
        let d = Dialect::Postgres;
        let (s, n) = (d.quote_literal(schema), d.quote_literal(&obj.name));
        let sql = match obj.kind {
            ObjectKind::View | ObjectKind::MaterializedView => format!("SELECT TEXT FROM SYSCAT.VIEWS WHERE VIEWSCHEMA = {s} AND VIEWNAME = {n}"),
            ObjectKind::Trigger => format!("SELECT TEXT FROM SYSCAT.TRIGGERS WHERE TRIGSCHEMA = {s} AND TRIGNAME = {n}"),
            ObjectKind::Procedure | ObjectKind::Function | ObjectKind::TableFunction => format!("SELECT TEXT FROM SYSCAT.ROUTINES WHERE ROUTINESCHEMA = {s} AND SPECIFICNAME = {}", d.quote_literal(&obj.ident)),
            k => anyhow::bail!("Viewing the source of a {} isn't supported for Db2", k.label()),
        };
        let def = scalar(self, &sql).await?.ok_or_else(|| anyhow::anyhow!("{} not found", obj.name))?;
        let def = def.trim().trim_end_matches(';');
        Ok(match obj.kind {
            ObjectKind::MaterializedView => format!("{def};\n"),
            _ => format!("{};\n", crate::replace_create(def, d, "CREATE OR REPLACE")),
        })
    }

    async fn explain(&mut self, stmt: &str, _analyze: bool) -> anyhow::Result<Plan> {
        if self.kind != DbKind::Spark {
            anyhow::bail!("EXPLAIN for Db2 needs the explain tables (SYSPROC.SYSINSTALLOBJECTS); run db2exfmt instead");
        }
        let lines = collect(self, &format!("EXPLAIN FORMATTED {}", stmt.trim().trim_end_matches(';'))).await?;
        Ok(plan_text_rows(lines.into_iter().map(|r| cell(&r, 0)).collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db2_connection_string() {
        let mut c = ConnectionConfig::new(DbKind::Db2);
        c.host = "db2.example.com".into();
        c.database = "STORE".into();
        c.user = "db2inst1".into();
        let s = connection_string(&c, "p;w}d");
        assert_eq!(s, "DRIVER={IBM DB2 ODBC DRIVER};HOSTNAME=db2.example.com;PORT=50000;PROTOCOL=TCPIP;DATABASE=STORE;UID=db2inst1;PWD={p;w}}d};");
    }

    #[test]
    fn spark_connection_string() {
        let mut c = ConnectionConfig::new(DbKind::Spark);
        c.host = "spark.example.com".into();
        c.odbc_options = "ThriftTransport=2;HTTPPath=cliservice;".into();
        let s = connection_string(&c, "");
        assert!(s.starts_with("DRIVER={Simba Spark ODBC Driver};HOST=spark.example.com;PORT=10000;"));
        assert!(s.contains("AuthMech=2") && s.ends_with("ThriftTransport=2;HTTPPath=cliservice;"));
    }

    #[test]
    fn db2_index_columns() {
        assert_eq!(db2_colnames("+CUSTOMER_ID-PLACED_AT"), vec!["CUSTOMER_ID", "PLACED_AT"]);
    }
}
