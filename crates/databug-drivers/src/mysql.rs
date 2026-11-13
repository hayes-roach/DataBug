use crate::{build_catalog, cell, collect, collect_named, plan, replace_create, split_list, Batcher, Canceller, DbConnection, ExecOptions, Extras, Sink, SourceRef};
use async_trait::async_trait;
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, SslOpts, Value};
use std::sync::Arc;
use databug_core::{Capabilities, Catalog, ColumnMeta, ConnectionConfig, DbObject, Dialect, ForeignKey, IndexInfo, ObjectKind, Plan, QueryEvent, SslMode};

pub struct MyConn {
    conn: Conn,
    opts: Opts,
}

fn my_error(e: mysql_async::Error) -> anyhow::Error {
    match e {
        mysql_async::Error::Server(s) => anyhow::anyhow!("ERROR {} ({}): {}", s.code, s.state, s.message),
        other => anyhow::anyhow!("{other}"),
    }
}

fn type_name(t: mysql_async::consts::ColumnType) -> String {
    let s = format!("{t:?}");
    s.trim_start_matches("MYSQL_TYPE_").to_ascii_lowercase()
}

pub(crate) fn value_text(v: &Value, out: &mut String) {
    use std::fmt::Write;
    match v {
        Value::NULL => {}
        Value::Bytes(b) => match std::str::from_utf8(b) {
            Ok(s) => out.push_str(s),
            Err(_) => {
                out.push_str("0x");
                for byte in b.iter().take(4096) {
                    let _ = write!(out, "{byte:02X}");
                }
            }
        },
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::UInt(u) => {
            let _ = write!(out, "{u}");
        }
        Value::Float(f) => {
            let _ = write!(out, "{f}");
        }
        Value::Double(f) => {
            let _ = write!(out, "{f}");
        }
        Value::Date(y, m, d, h, mi, s, us) => {
            let _ = write!(out, "{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}");
            if *us > 0 {
                let _ = write!(out, ".{us:06}");
            }
        }
        Value::Time(neg, d, h, m, s, _us) => {
            let _ = write!(out, "{}{:02}:{m:02}:{s:02}", if *neg { "-" } else { "" }, *d * 24 + u32::from(*h));
        }
    }
}

impl MyConn {
    pub async fn connect(cfg: &ConnectionConfig, password: &str) -> anyhow::Result<Self> {
        let mut b = OptsBuilder::default()
            .ip_or_hostname(cfg.host.clone())
            .tcp_port(cfg.port)
            .user(Some(cfg.user.clone()))
            .prefer_socket(false);
        if !password.is_empty() {
            b = b.pass(Some(password.to_string()));
        }
        if !cfg.database.is_empty() {
            b = b.db_name(Some(cfg.database.clone()));
        }
        if cfg.ssl == SslMode::Require {
            b = b.ssl_opts(Some(SslOpts::default().with_danger_accept_invalid_certs(true)));
        }
        let opts: Opts = b.into();
        let mut conn = Conn::new(opts.clone()).await.map_err(my_error)?;
        if cfg.read_only {
            conn.query_drop("SET SESSION TRANSACTION READ ONLY").await.map_err(my_error)?;
        }
        Ok(MyConn { conn, opts })
    }
}

#[async_trait]
impl DbConnection for MyConn {
    fn capabilities(&self) -> Capabilities {
        Capabilities { schemas: true, transactions: true, explain: true, explain_analyze: true, server_cancel: true, sql: true }
    }

    fn canceller(&self) -> Option<Canceller> {
        let id = self.conn.id();
        let opts = self.opts.clone();
        let rt = tokio::runtime::Handle::current();
        Some(Arc::new(move || {
            let opts = opts.clone();
            rt.spawn(async move {
                if let Ok(mut c) = Conn::new(opts).await {
                    let _ = c.query_drop(format!("KILL QUERY {id}")).await;
                    let _ = c.disconnect().await;
                }
            });
        }))
    }

    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        let mut res = self.conn.query_iter(stmt).await.map_err(my_error)?;
        let mut guard = 0;
        loop {
            guard += 1;
            let cols: Vec<ColumnMeta> = res.columns_ref().iter().map(|c| ColumnMeta::new(c.name_str(), type_name(c.column_type()))).collect();
            if cols.is_empty() {
                sink(QueryEvent::Affected(res.affected_rows()));
                // advances past this (row-less) result set
                let _ = res.next().await.map_err(my_error)?;
            } else {
                let width = cols.len();
                let mut b = Batcher::new(sink, cols, opts.row_limit);
                let mut keep = true;
                while let Some(row) = res.next().await.map_err(my_error)? {
                    if !keep {
                        continue;
                    }
                    for i in 0..width {
                        match row.as_ref(i) {
                            Some(Value::NULL) | None => b.cells().push(None),
                            Some(v) => b.cells().push_with(|s| value_text(v, s)),
                        }
                    }
                    keep = b.row_done();
                }
            }
            if res.is_empty() || guard > 10_000 {
                break;
            }
        }
        let has_warnings = res.warnings() > 0;
        drop(res);
        if !has_warnings {
            return Ok(());
        }
        if let Ok(w) = self.conn.query_iter("SHOW WARNINGS").await {
            let rows: Vec<mysql_async::Row> = w.collect_and_drop().await.unwrap_or_default();
            for r in rows {
                let level: String = r.get(0).unwrap_or_default();
                let msg: String = r.get(2).unwrap_or_default();
                sink(QueryEvent::Message(format!("{level}: {msg}")));
            }
        }
        Ok(())
    }

    async fn introspect(&mut self) -> anyhow::Result<Catalog> {
        let rows = collect(self, "SELECT c.TABLE_SCHEMA, c.TABLE_NAME, t.TABLE_TYPE, c.COLUMN_NAME, c.COLUMN_TYPE, c.IS_NULLABLE, c.COLUMN_DEFAULT, c.COLUMN_KEY \
            FROM information_schema.COLUMNS c JOIN information_schema.TABLES t ON t.TABLE_SCHEMA = c.TABLE_SCHEMA AND t.TABLE_NAME = c.TABLE_NAME \
            WHERE c.TABLE_SCHEMA NOT IN ('mysql','information_schema','performance_schema','sys') \
            ORDER BY c.TABLE_SCHEMA, c.TABLE_NAME, c.ORDINAL_POSITION").await?;
        let schemas = collect(self, "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME NOT IN ('mysql','information_schema','performance_schema','sys')").await?;
        const SYS: &str = "('mysql','information_schema','performance_schema','sys')";
        let mut extras = Extras::default();
        for r in collect(self, &format!("SELECT TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, NON_UNIQUE, GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX) FROM information_schema.STATISTICS WHERE TABLE_SCHEMA NOT IN {SYS} GROUP BY TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, NON_UNIQUE")).await.unwrap_or_default() {
            let name = cell(&r, 2);
            extras.indexes.push((cell(&r, 0), cell(&r, 1), IndexInfo { primary: name == "PRIMARY", unique: cell(&r, 3) == "0", columns: split_list(r[4].as_ref()), name }));
        }
        for r in collect(
            self,
            &format!(
                "SELECT k.TABLE_SCHEMA, k.TABLE_NAME, k.CONSTRAINT_NAME, GROUP_CONCAT(k.COLUMN_NAME ORDER BY k.ORDINAL_POSITION), k.REFERENCED_TABLE_SCHEMA, k.REFERENCED_TABLE_NAME, \
                 GROUP_CONCAT(k.REFERENCED_COLUMN_NAME ORDER BY k.ORDINAL_POSITION), MAX(r.DELETE_RULE) \
                 FROM information_schema.KEY_COLUMN_USAGE k JOIN information_schema.REFERENTIAL_CONSTRAINTS r ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME AND r.TABLE_NAME = k.TABLE_NAME \
                 WHERE k.REFERENCED_TABLE_NAME IS NOT NULL AND k.TABLE_SCHEMA NOT IN {SYS} \
                 GROUP BY k.TABLE_SCHEMA, k.TABLE_NAME, k.CONSTRAINT_NAME, k.REFERENCED_TABLE_SCHEMA, k.REFERENCED_TABLE_NAME"
            ),
        )
        .await
        .unwrap_or_default()
        {
            let rule = cell(&r, 7);
            extras.foreign_keys.push((
                cell(&r, 0),
                cell(&r, 1),
                ForeignKey {
                    name: cell(&r, 2),
                    columns: split_list(r[3].as_ref()),
                    ref_schema: cell(&r, 4),
                    ref_table: cell(&r, 5),
                    ref_columns: split_list(r[6].as_ref()),
                    on_delete: if rule == "NO ACTION" || rule == "RESTRICT" { String::new() } else { rule },
                },
            ));
        }
        for r in collect(self, &format!("SELECT ROUTINE_SCHEMA, ROUTINE_NAME, ROUTINE_TYPE FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA NOT IN {SYS}")).await.unwrap_or_default() {
            let kind = if cell(&r, 2) == "PROCEDURE" { ObjectKind::Procedure } else { ObjectKind::Function };
            extras.objects.push((cell(&r, 0), DbObject { name: cell(&r, 1), kind, detail: String::new(), ident: String::new(), table: None }));
        }
        for r in collect(self, &format!("SELECT TRIGGER_SCHEMA, TRIGGER_NAME, EVENT_OBJECT_TABLE, ACTION_TIMING, EVENT_MANIPULATION FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA NOT IN {SYS}")).await.unwrap_or_default() {
            extras.objects.push((
                cell(&r, 0),
                DbObject { name: cell(&r, 1), kind: ObjectKind::Trigger, detail: format!("{} {} on {}", cell(&r, 3), cell(&r, 4), cell(&r, 2)).to_lowercase(), ident: String::new(), table: Some(cell(&r, 2)) },
            ));
        }
        for r in collect(self, &format!("SELECT EVENT_SCHEMA, EVENT_NAME, STATUS FROM information_schema.EVENTS WHERE EVENT_SCHEMA NOT IN {SYS}")).await.unwrap_or_default() {
            extras.objects.push((cell(&r, 0), DbObject { name: cell(&r, 1), kind: ObjectKind::Event, detail: cell(&r, 2).to_lowercase(), ident: String::new(), table: None }));
        }
        let db = collect(self, "SELECT DATABASE()").await?;
        let default = db.first().and_then(|r| r[0].clone()).unwrap_or_default();
        // COLUMN_KEY is 'PRI' for primary key columns; MariaDB reports "no default" as the text NULL
        let rows = rows
            .into_iter()
            .map(|mut r| {
                if r.get(6).map(|v| v.as_deref() == Some("NULL")).unwrap_or(false) {
                    r[6] = None;
                }
                if let Some(k) = r.get_mut(7) {
                    *k = Some(if k.as_deref() == Some("PRI") { "true".into() } else { "false".into() });
                }
                r
            })
            .collect();
        Ok(build_catalog(rows, default, schemas.into_iter().filter_map(|r| r[0].clone()).collect(), extras))
    }

    async fn object_source(&mut self, schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        let d = Dialect::MySql;
        let q = format!("{}.{}", d.quote_ident(schema), d.quote_ident(&obj.name));
        let (verb, col) = match obj.kind {
            ObjectKind::View => ("VIEW", "Create View"),
            ObjectKind::Procedure => ("PROCEDURE", "Create Procedure"),
            ObjectKind::Function => ("FUNCTION", "Create Function"),
            ObjectKind::Trigger => ("TRIGGER", "SQL Original Statement"),
            ObjectKind::Event => ("EVENT", "Create Event"),
            k => anyhow::bail!("MySQL has no {} objects", k.label()),
        };
        let (cols, rows) = collect_named(self, &format!("SHOW CREATE {verb} {q}")).await?;
        let i = cols.iter().position(|c| c == col).ok_or_else(|| anyhow::anyhow!("Unexpected SHOW CREATE output"))?;
        let def = rows.first().and_then(|r| r.get(i).cloned().flatten()).ok_or_else(|| anyhow::anyhow!("You don't have permission to see this definition"))?;
        Ok(match obj.kind {
            ObjectKind::View => replace_create(&def, d, "CREATE OR REPLACE") + ";\n",
            _ => format!("DROP {verb} IF EXISTS {q};\n\n{};\n", def.trim_end().trim_end_matches(';')),
        })
    }

    async fn explain(&mut self, stmt: &str, analyze: bool) -> anyhow::Result<Plan> {
        if analyze {
            let modifies = databug_sql::classify(stmt, Dialect::MySql).modifies();
            if modifies {
                self.conn.query_drop("START TRANSACTION").await.map_err(my_error)?;
            }
            let res: Result<Vec<String>, _> = self.conn.query(format!("EXPLAIN ANALYZE {stmt}")).await;
            if modifies {
                let _ = self.conn.query_drop("ROLLBACK").await;
            }
            let raw = res.map_err(my_error)?.join("\n");
            return Ok(Plan { root: None, raw, format: "text" });
        }
        let rows: Vec<String> = self.conn.query(format!("EXPLAIN FORMAT=JSON {stmt}")).await.map_err(my_error)?;
        let raw = rows.join("\n");
        let root = plan::from_generic_json(&raw);
        let pretty = serde_json::from_str::<serde_json::Value>(&raw).ok().and_then(|v| serde_json::to_string_pretty(&v).ok()).unwrap_or(raw);
        Ok(Plan { root, raw: pretty, format: "json" })
    }
}
