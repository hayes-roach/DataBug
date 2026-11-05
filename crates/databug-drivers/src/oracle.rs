//! Oracle Database through the `oracle` crate (ODPI-C). Building needs
//! nothing extra; running needs Oracle Instant Client (or a full client)
//! where the system can find it: on the library path, or next to DataBug.

use crate::infoschema::{group_foreign_keys, group_indexes};
use crate::{build_catalog, cell, collect, plan_text_rows, scalar, tx_command, Batcher, Canceller, DbConnection, ExecOptions, Extras, Sink, SourceRef, TxCommand};
use async_trait::async_trait;
use databug_core::{Capabilities, Catalog, ColumnMeta, ConnectionConfig, DbObject, Dialect, ObjectKind, Plan, QueryEvent, SslMode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Oracle is synchronous; calls run on Tokio's blocking pool. The connection
/// is shared with the canceller, which calls `break_execution` from another thread.
pub struct OraConn {
    conn: Arc<oracle::Connection>,
    /// Inside an explicit transaction: don't commit after each statement.
    in_tx: Arc<AtomicBool>,
}

/// The connect descriptor: the URI field (a TNS alias or full descriptor)
/// when set, otherwise one built from host, port and service name or SID.
pub(crate) fn connect_string(cfg: &ConnectionConfig) -> String {
    if !cfg.uri.trim().is_empty() {
        return cfg.uri.trim().to_string();
    }
    let protocol = if cfg.ssl == SslMode::Require { "TCPS" } else { "TCP" };
    let target = if cfg.oracle_sid { format!("(SID={})", cfg.database.trim()) } else { format!("(SERVICE_NAME={})", cfg.database.trim()) };
    format!("(DESCRIPTION=(ADDRESS=(PROTOCOL={protocol})(HOST={})(PORT={}))(CONNECT_DATA={target}))", cfg.host.trim(), cfg.port)
}

/// Oracle has no `LIMIT`; turn a trailing `LIMIT n` (what DataBug writes
/// for "open table") into `FETCH FIRST n ROWS ONLY` (Oracle 12c and later).
pub(crate) fn limit_to_fetch_first(sql: &str) -> String {
    let toks: Vec<_> = databug_sql::tokenize(sql, Dialect::Postgres).into_iter().filter(|t| !t.is_trivia()).collect();
    let n = toks.len();
    if n >= 3 && toks[n - 2].text(sql).eq_ignore_ascii_case("LIMIT") && toks[n - 1].text(sql).chars().all(|c| c.is_ascii_digit()) {
        return format!("{}FETCH FIRST {} ROWS ONLY", &sql[..toks[n - 2].start], toks[n - 1].text(sql));
    }
    sql.to_string()
}

/// SQL statements are sent without a trailing `;`, PL/SQL blocks with one.
pub(crate) fn oci_text(stmt: &str) -> String {
    let s = stmt.trim();
    let upper = s.trim_start_matches('(').to_ascii_uppercase();
    let words: Vec<&str> = upper.split_whitespace().take(6).collect();
    let plsql = matches!(words.first(), Some(&"BEGIN") | Some(&"DECLARE"))
        || (words.first() == Some(&"CREATE") && words.iter().any(|w| matches!(*w, "PROCEDURE" | "FUNCTION" | "PACKAGE" | "TRIGGER" | "TYPE")));
    if plsql {
        if s.ends_with(';') { s.to_string() } else { format!("{s};") }
    } else {
        limit_to_fetch_first(s.trim_end_matches(';').trim_end())
    }
}

fn ora_error(e: oracle::Error) -> anyhow::Error {
    let msg = e.to_string();
    if msg.contains("DPI-1047") {
        anyhow::anyhow!("{msg}\n\nDataBug needs Oracle Instant Client to talk to Oracle. Install it and put it on the library path (or next to DataBug); see the README.")
    } else {
        anyhow::anyhow!(msg)
    }
}

fn run(conn: &oracle::Connection, stmt: &str, limit: usize, sink: &Sink, in_tx: &AtomicBool) -> anyhow::Result<()> {
    match tx_command(stmt) {
        Some(TxCommand::Begin) => {
            in_tx.store(true, Ordering::SeqCst);
            sink(QueryEvent::Message("Transaction started".into()));
            return Ok(());
        }
        Some(TxCommand::Commit) => {
            conn.commit().map_err(ora_error)?;
            in_tx.store(false, Ordering::SeqCst);
            sink(QueryEvent::Message("Committed".into()));
            return Ok(());
        }
        Some(TxCommand::Rollback) => {
            conn.rollback().map_err(ora_error)?;
            in_tx.store(false, Ordering::SeqCst);
            sink(QueryEvent::Message("Rolled back".into()));
            return Ok(());
        }
        None => {}
    }
    let sql = oci_text(stmt);
    let mut st = conn.statement(&sql).fetch_array_size(500).build().map_err(ora_error)?;
    if st.is_query() {
        let rows = st.query(&[]).map_err(ora_error)?;
        let cols: Vec<ColumnMeta> = rows.column_info().iter().map(|c| ColumnMeta::new(c.name(), c.oracle_type().to_string().to_ascii_lowercase())).collect();
        let width = cols.len();
        let mut b = Batcher::new(sink, cols, limit);
        for row in rows {
            let row = row.map_err(ora_error)?;
            for i in 0..width {
                let v: Option<String> = match row.get::<usize, Option<String>>(i) {
                    Ok(v) => v,
                    Err(_) => row.sql_values().get(i).map(|v| v.to_string()),
                };
                b.cells().push(v.as_deref());
            }
            if !b.row_done() {
                break;
            }
        }
    } else {
        st.execute(&[]).map_err(ora_error)?;
        let n = if st.is_dml() { st.row_count().map_err(ora_error)? } else { 0 };
        // Oracle runs DDL in its own transaction; commit DML unless the user started one
        if !in_tx.load(Ordering::SeqCst) {
            conn.commit().map_err(ora_error)?;
        }
        sink(QueryEvent::Affected(n));
    }
    Ok(())
}

impl OraConn {
    pub async fn connect(cfg: &ConnectionConfig, password: &str) -> anyhow::Result<Self> {
        let (user, pw, cs) = (cfg.user.clone(), password.to_string(), connect_string(cfg));
        let conn = tokio::task::spawn_blocking(move || -> anyhow::Result<oracle::Connection> {
            let c = oracle::Connector::new(user, pw, cs).driver_name("DataBug").connect().map_err(ora_error)?;
            Ok(c)
        })
        .await??;
        Ok(OraConn { conn: Arc::new(conn), in_tx: Arc::default() })
    }

    async fn blocking<T: Send + 'static>(&self, f: impl FnOnce(&oracle::Connection) -> anyhow::Result<T> + Send + 'static) -> anyhow::Result<T> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || f(&conn)).await?
    }
}

const OWNERS: &str = "SELECT USERNAME FROM ALL_USERS WHERE ORACLE_MAINTAINED = 'N'";

fn introspect_sql() -> String {
    format!(
        "SELECT c.OWNER, c.TABLE_NAME, CASE WHEN v.VIEW_NAME IS NOT NULL THEN 'VIEW' WHEN m.MVIEW_NAME IS NOT NULL THEN 'MATVIEW' ELSE 'TABLE' END, c.COLUMN_NAME, \
LOWER(c.DATA_TYPE) || CASE WHEN c.DATA_TYPE IN ('VARCHAR2', 'NVARCHAR2', 'CHAR', 'NCHAR') THEN '(' || c.CHAR_LENGTH || ')' WHEN c.DATA_TYPE = 'RAW' THEN '(' || c.DATA_LENGTH || ')' \
WHEN c.DATA_TYPE = 'NUMBER' AND c.DATA_PRECISION IS NOT NULL THEN '(' || c.DATA_PRECISION || ',' || NVL(c.DATA_SCALE, 0) || ')' ELSE '' END, \
CASE c.NULLABLE WHEN 'Y' THEN 'true' ELSE 'false' END, NULL, \
CASE WHEN EXISTS (SELECT 1 FROM ALL_CONSTRAINTS k JOIN ALL_CONS_COLUMNS kc ON kc.OWNER = k.OWNER AND kc.CONSTRAINT_NAME = k.CONSTRAINT_NAME \
WHERE k.CONSTRAINT_TYPE = 'P' AND k.OWNER = c.OWNER AND k.TABLE_NAME = c.TABLE_NAME AND kc.COLUMN_NAME = c.COLUMN_NAME) THEN 'true' ELSE 'false' END \
FROM ALL_TAB_COLUMNS c LEFT JOIN ALL_VIEWS v ON v.OWNER = c.OWNER AND v.VIEW_NAME = c.TABLE_NAME LEFT JOIN ALL_MVIEWS m ON m.OWNER = c.OWNER AND m.MVIEW_NAME = c.TABLE_NAME \
WHERE c.OWNER IN ({OWNERS}) AND c.TABLE_NAME NOT LIKE 'BIN$%' ORDER BY c.OWNER, c.TABLE_NAME, c.COLUMN_ID"
    )
}

fn indexes_sql() -> String {
    format!(
        "SELECT i.TABLE_OWNER, i.TABLE_NAME, i.INDEX_NAME, CASE i.UNIQUENESS WHEN 'UNIQUE' THEN 'true' ELSE 'false' END, \
CASE WHEN EXISTS (SELECT 1 FROM ALL_CONSTRAINTS k WHERE k.CONSTRAINT_TYPE = 'P' AND k.OWNER = i.TABLE_OWNER AND k.INDEX_NAME = i.INDEX_NAME) THEN 'true' ELSE 'false' END, ic.COLUMN_NAME \
FROM ALL_INDEXES i JOIN ALL_IND_COLUMNS ic ON ic.INDEX_OWNER = i.OWNER AND ic.INDEX_NAME = i.INDEX_NAME \
WHERE i.TABLE_OWNER IN ({OWNERS}) ORDER BY 1, 2, 3, ic.COLUMN_POSITION"
    )
}

fn foreign_keys_sql() -> String {
    format!(
        "SELECT c.OWNER, c.TABLE_NAME, c.CONSTRAINT_NAME, cc.COLUMN_NAME, r.OWNER, r.TABLE_NAME, rc.COLUMN_NAME, c.DELETE_RULE \
FROM ALL_CONSTRAINTS c JOIN ALL_CONS_COLUMNS cc ON cc.OWNER = c.OWNER AND cc.CONSTRAINT_NAME = c.CONSTRAINT_NAME \
JOIN ALL_CONSTRAINTS r ON r.OWNER = c.R_OWNER AND r.CONSTRAINT_NAME = c.R_CONSTRAINT_NAME \
JOIN ALL_CONS_COLUMNS rc ON rc.OWNER = r.OWNER AND rc.CONSTRAINT_NAME = r.CONSTRAINT_NAME AND rc.POSITION = cc.POSITION \
WHERE c.CONSTRAINT_TYPE = 'R' AND c.OWNER IN ({OWNERS}) ORDER BY 1, 2, 3, cc.POSITION"
    )
}

#[async_trait]
impl DbConnection for OraConn {
    fn capabilities(&self) -> Capabilities {
        Capabilities { schemas: true, transactions: true, explain: true, explain_analyze: false, server_cancel: true, sql: true }
    }

    fn canceller(&self) -> Option<Canceller> {
        let c = self.conn.clone();
        Some(Arc::new(move || {
            let _ = c.break_execution();
        }))
    }

    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        let (stmt, limit, sink, in_tx) = (stmt.to_string(), opts.row_limit, sink.clone(), self.in_tx.clone());
        self.blocking(move |c| run(c, &stmt, limit, &sink, &in_tx)).await
    }

    async fn introspect(&mut self) -> anyhow::Result<Catalog> {
        let rows = collect(self, &introspect_sql()).await?;
        let schemas = collect(self, &format!("{OWNERS} ORDER BY 1")).await?;
        let mut extras = Extras::default();
        if let Ok(ix) = collect(self, &indexes_sql()).await {
            extras.indexes = group_indexes(ix);
        }
        if let Ok(fk) = collect(self, &foreign_keys_sql()).await {
            extras.foreign_keys = group_foreign_keys(fk);
        }
        let objects = format!(
            "SELECT OWNER, OBJECT_NAME, OBJECT_TYPE, NULL FROM ALL_OBJECTS WHERE OBJECT_TYPE IN ('PROCEDURE', 'FUNCTION', 'SEQUENCE') AND OWNER IN ({OWNERS}) \
UNION ALL SELECT OWNER, TRIGGER_NAME, 'TRIGGER', TABLE_NAME FROM ALL_TRIGGERS WHERE OWNER IN ({OWNERS}) \
UNION ALL SELECT OWNER, SYNONYM_NAME, 'SYNONYM', TABLE_OWNER || '.' || TABLE_NAME FROM ALL_SYNONYMS WHERE OWNER IN ({OWNERS})"
        );
        for r in collect(self, &objects).await.unwrap_or_default() {
            let kind = match cell(&r, 2).as_str() {
                "PROCEDURE" => ObjectKind::Procedure,
                "FUNCTION" => ObjectKind::Function,
                "SEQUENCE" => ObjectKind::Sequence,
                "TRIGGER" => ObjectKind::Trigger,
                _ => ObjectKind::Synonym,
            };
            let extra = cell(&r, 3);
            let (detail, table) = match kind {
                ObjectKind::Trigger => (format!("on {extra}"), Some(extra)),
                _ => (extra, None),
            };
            extras.objects.push((cell(&r, 0), DbObject { name: cell(&r, 1), kind, detail, ident: String::new(), table }));
        }
        let default = scalar(self, "SELECT SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA') FROM DUAL").await?.unwrap_or_default();
        Ok(build_catalog(rows, default, schemas.into_iter().filter_map(|r| r.into_iter().next().flatten()).collect(), extras))
    }

    async fn object_source(&mut self, schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        let d = Dialect::Postgres;
        let ty = match obj.kind {
            ObjectKind::View => "VIEW",
            ObjectKind::MaterializedView => "MATERIALIZED_VIEW",
            ObjectKind::Procedure => "PROCEDURE",
            ObjectKind::Function => "FUNCTION",
            ObjectKind::Trigger => "TRIGGER",
            ObjectKind::Sequence => "SEQUENCE",
            ObjectKind::Synonym => "SYNONYM",
            k => anyhow::bail!("Viewing the source of a {} isn't supported for Oracle", k.label()),
        };
        let def = scalar(self, &format!("SELECT DBMS_METADATA.GET_DDL('{ty}', {}, {}) FROM DUAL", d.quote_literal(&obj.name), d.quote_literal(schema))).await?.ok_or_else(|| anyhow::anyhow!("{} not found", obj.name))?;
        let def = def.trim();
        Ok(match obj.kind {
            ObjectKind::View | ObjectKind::Procedure | ObjectKind::Function | ObjectKind::Trigger => format!("{}\n", crate::replace_create(def, d, "CREATE OR REPLACE")),
            _ => format!("{def}\n"),
        })
    }

    async fn explain(&mut self, stmt: &str, _analyze: bool) -> anyhow::Result<Plan> {
        let sql = format!("EXPLAIN PLAN SET STATEMENT_ID = 'DATABUG' FOR {}", oci_text(stmt));
        let lines = self
            .blocking(move |c| {
                c.execute(&sql, &[]).map_err(ora_error)?;
                let rows = c.query("SELECT PLAN_TABLE_OUTPUT FROM TABLE(DBMS_XPLAN.DISPLAY(NULL, 'DATABUG', 'TYPICAL'))", &[]).map_err(ora_error)?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r.map_err(ora_error)?.get::<usize, Option<String>>(0).map_err(ora_error)?.unwrap_or_default());
                }
                Ok(out)
            })
            .await?;
        Ok(plan_text_rows(lines))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use databug_core::DbKind;

    #[test]
    fn descriptors() {
        let mut c = ConnectionConfig::new(DbKind::Oracle);
        c.host = "ora.example.com".into();
        c.database = "FREEPDB1".into();
        assert_eq!(connect_string(&c), "(DESCRIPTION=(ADDRESS=(PROTOCOL=TCP)(HOST=ora.example.com)(PORT=1521))(CONNECT_DATA=(SERVICE_NAME=FREEPDB1)))");
        c.oracle_sid = true;
        c.ssl = SslMode::Require;
        assert!(connect_string(&c).contains("(PROTOCOL=TCPS)") && connect_string(&c).contains("(SID=FREEPDB1)"));
        c.uri = "store_high".into();
        assert_eq!(connect_string(&c), "store_high");
    }

    #[test]
    fn limit_becomes_fetch_first() {
        assert_eq!(oci_text("SELECT * FROM \"CUSTOMERS\" LIMIT 100;"), "SELECT * FROM \"CUSTOMERS\" FETCH FIRST 100 ROWS ONLY");
        assert_eq!(oci_text("select 'LIMIT 5' from dual"), "select 'LIMIT 5' from dual");
        assert_eq!(oci_text("BEGIN NULL; END"), "BEGIN NULL; END;");
        assert_eq!(oci_text("CREATE OR REPLACE PROCEDURE p AS BEGIN NULL; END;"), "CREATE OR REPLACE PROCEDURE p AS BEGIN NULL; END;");
    }
}
