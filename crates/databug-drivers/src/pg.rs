use crate::{build_catalog, cell, collect, plan, scalar, split_list, truthy, Batcher, Canceller, DbConnection, ExecOptions, Extras, Sink, SourceRef};
use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt};
use std::sync::{Arc, Mutex};
use databug_core::{Capabilities, Catalog, ColumnMeta, ConnectionConfig, DbKind, DbObject, Dialect, ForeignKey, IndexInfo, ObjectKind, Plan, QueryEvent, SslMode};
use tokio_postgres::{AsyncMessage, Client, SimpleQueryMessage};
use tokio_postgres_rustls::MakeRustlsConnect;

/// Also used for engines that speak the PostgreSQL protocol (see `pgwire`).
pub struct PgConn {
    pub(crate) client: Client,
    tls: MakeRustlsConnect,
    notices: Arc<Mutex<Option<Sink>>>,
    pub(crate) kind: DbKind,
}

fn tls_config() -> MakeRustlsConnect {
    // Like libpq's `sslmode=prefer/require`: encrypt, but don't verify the
    // server certificate (most self-hosted servers use self-signed certs).
    MakeRustlsConnect::new(crate::tls::no_verify_config())
}

pub(crate) fn pg_error(e: tokio_postgres::Error) -> anyhow::Error {
    if let Some(db) = e.as_db_error() {
        let mut msg = format!("{}: {}", db.severity(), db.message());
        if let Some(d) = db.detail() {
            msg.push_str(&format!("\nDetail: {d}"));
        }
        if let Some(h) = db.hint() {
            msg.push_str(&format!("\nHint: {h}"));
        }
        if let Some(tokio_postgres::error::ErrorPosition::Original(p)) = db.position() {
            msg.push_str(&format!("\nPosition: {p}"));
        }
        anyhow::anyhow!(msg)
    } else {
        // include the cause ("connection refused", "password authentication failed", ...)
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(&e);
        while let Some(s) = src {
            msg.push_str(": ");
            msg.push_str(&s.to_string());
            src = s.source();
        }
        anyhow::anyhow!(msg)
    }
}

impl PgConn {
    pub async fn connect(cfg: &ConnectionConfig, password: &str) -> anyhow::Result<Self> {
        let mut pc = tokio_postgres::Config::new();
        pc.host(&cfg.host).port(cfg.port).user(&cfg.user).application_name("DataBug");
        if !password.is_empty() {
            pc.password(password);
        }
        // no database given: use the default `postgres` database rather than
        // libpq's fallback of a database named after the user
        pc.dbname(&cfg.effective_database());
        pc.ssl_mode(match cfg.ssl {
            SslMode::Disable => tokio_postgres::config::SslMode::Disable,
            SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
            SslMode::Require => tokio_postgres::config::SslMode::Require,
        });
        let tls = tls_config();
        let (client, mut connection) = pc.connect(tls.clone()).await.map_err(pg_error)?;
        let notices: Arc<Mutex<Option<Sink>>> = Arc::default();
        let n2 = notices.clone();
        tokio::spawn(async move {
            let mut stream = futures_util::stream::poll_fn(move |cx| connection.poll_message(cx));
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(AsyncMessage::Notice(n)) => {
                        if let Some(s) = n2.lock().unwrap().as_ref() {
                            s(QueryEvent::Message(format!("{}: {}", n.severity(), n.message())));
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
        if cfg.read_only {
            if let Some(sql) = crate::pgwire::read_only_sql(cfg.kind) {
                client.simple_query(sql).await.map_err(pg_error)?;
            }
        }
        Ok(PgConn { client, tls, notices, kind: cfg.kind })
    }
}

#[async_trait]
impl DbConnection for PgConn {
    fn capabilities(&self) -> Capabilities {
        crate::pgwire::capabilities(self.kind)
    }

    fn is_closed(&self) -> bool {
        self.client.is_closed()
    }

    fn canceller(&self) -> Option<Canceller> {
        let token = self.client.cancel_token();
        let tls = self.tls.clone();
        let rt = tokio::runtime::Handle::current();
        Some(Arc::new(move || {
            let token = token.clone();
            let tls = tls.clone();
            rt.spawn(async move {
                let _ = token.cancel_query(tls).await;
            });
        }))
    }

    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        *self.notices.lock().unwrap() = Some(sink.clone());
        let res = self.run(stmt, opts, sink).await;
        *self.notices.lock().unwrap() = None;
        res
    }

    async fn list_databases(&mut self) -> anyhow::Result<(Vec<String>, String)> {
        if self.kind != DbKind::Postgres {
            return crate::pgwire::list_databases(self).await;
        }
        self.pg_list_databases().await
    }

    async fn introspect(&mut self) -> anyhow::Result<Catalog> {
        if self.kind != DbKind::Postgres {
            return crate::pgwire::introspect(self).await;
        }
        self.introspect_pg().await
    }

    async fn object_source(&mut self, schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        if self.kind != DbKind::Postgres {
            return crate::pgwire::object_source(self, schema, obj).await;
        }
        self.pg_object_source(schema, obj).await
    }

    async fn explain(&mut self, stmt: &str, analyze: bool) -> anyhow::Result<Plan> {
        if !matches!(self.kind, DbKind::Postgres | DbKind::TimescaleDb) {
            return crate::pgwire::explain(self, stmt, analyze).await;
        }
        self.pg_explain(stmt, analyze).await
    }
}

/// The PostgreSQL catalog queries, shared with TimescaleDB.
impl PgConn {
    pub(crate) async fn pg_list_databases(&mut self) -> anyhow::Result<(Vec<String>, String)> {
        let names = collect(self, "SELECT datname FROM pg_database WHERE datallowconn AND NOT datistemplate ORDER BY 1").await?;
        let current = scalar(self, "SELECT current_database()").await?.unwrap_or_default();
        Ok((names.into_iter().filter_map(|r| r.into_iter().next().flatten()).collect(), current))
    }

    pub(crate) async fn introspect_pg(&mut self) -> anyhow::Result<Catalog> {
        let rows = collect(self, INTROSPECT).await?;
        let schemas = collect(self, &format!("SELECT nspname FROM pg_namespace n WHERE {SYS} ORDER BY 1")).await?;
        let mut extras = Extras::default();
        for r in collect(self, INDEXES).await? {
            extras.indexes.push((cell(&r, 0), cell(&r, 1), IndexInfo { name: cell(&r, 2), unique: truthy(r[3].as_deref()), primary: truthy(r[4].as_deref()), columns: split_list(r[5].as_ref()) }));
        }
        for r in collect(self, FOREIGN_KEYS).await? {
            let on_delete = match cell(&r, 7).as_str() {
                "c" => "CASCADE",
                "n" => "SET NULL",
                "d" => "SET DEFAULT",
                "r" => "RESTRICT",
                _ => "",
            };
            extras.foreign_keys.push((
                cell(&r, 0),
                cell(&r, 1),
                ForeignKey { name: cell(&r, 2), columns: split_list(r[3].as_ref()), ref_schema: cell(&r, 4), ref_table: cell(&r, 5), ref_columns: split_list(r[6].as_ref()), on_delete: on_delete.into() },
            ));
        }
        for r in collect(self, ROUTINES).await? {
            let kind = match cell(&r, 2).as_str() {
                "PROCEDURE" => ObjectKind::Procedure,
                "AGGREGATE" => ObjectKind::Aggregate,
                "TABLEFUNCTION" => ObjectKind::TableFunction,
                _ => ObjectKind::Function,
            };
            extras.objects.push((cell(&r, 0), DbObject { name: cell(&r, 1), kind, detail: cell(&r, 3), ident: cell(&r, 4), table: None }));
        }
        for r in collect(self, &format!("SELECT n.nspname, t.tgname, c.relname FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace WHERE NOT t.tgisinternal AND {SYS}")).await? {
            extras.objects.push((cell(&r, 0), DbObject { name: cell(&r, 1), kind: ObjectKind::Trigger, detail: format!("on {}", cell(&r, 2)), ident: String::new(), table: Some(cell(&r, 2)) }));
        }
        for r in collect(self, &format!("SELECT n.nspname, c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind = 'S' AND {SYS}")).await? {
            extras.objects.push((cell(&r, 0), DbObject { name: cell(&r, 1), kind: ObjectKind::Sequence, detail: String::new(), ident: String::new(), table: None }));
        }
        let default = scalar(self, "SELECT current_schema()").await?.unwrap_or_else(|| "public".into());
        Ok(build_catalog(rows, default, schemas.into_iter().filter_map(|r| r[0].clone()).collect(), extras))
    }

    pub(crate) async fn pg_object_source(&mut self, schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        let d = Dialect::Postgres;
        let q = format!("{}.{}", d.quote_ident(schema), d.quote_ident(&obj.name));
        let lit = |s: &str| d.quote_literal(s);
        let rel = format!("to_regclass({})", lit(&q));
        match obj.kind {
            ObjectKind::View => {
                let def = scalar(self, &format!("SELECT pg_get_viewdef({rel}, true)")).await?.ok_or_else(|| anyhow::anyhow!("View {q} not found"))?;
                Ok(format!("CREATE OR REPLACE VIEW {q} AS\n{}", def.trim_end().trim_end_matches(';')) + ";\n")
            }
            ObjectKind::MaterializedView => {
                let def = scalar(self, &format!("SELECT pg_get_viewdef({rel}, true)")).await?.ok_or_else(|| anyhow::anyhow!("Materialized view {q} not found"))?;
                Ok(format!(
                    "-- Materialized views can't be replaced in place, so this drops and recreates it.\nDROP MATERIALIZED VIEW IF EXISTS {q};\n\nCREATE MATERIALIZED VIEW {q} AS\n{};\n",
                    def.trim_end().trim_end_matches(';')
                ))
            }
            ObjectKind::Function | ObjectKind::Procedure | ObjectKind::TableFunction => {
                let ident = if obj.ident.is_empty() { q.clone() } else { obj.ident.clone() };
                let def = scalar(self, &format!("SELECT pg_get_functiondef({}::regprocedure)", lit(&ident))).await?.unwrap_or_default();
                Ok(def.trim_end().to_string() + ";\n")
            }
            ObjectKind::Aggregate => Ok(format!("-- Aggregate {}({}) — Postgres can't print aggregate definitions; use CREATE OR REPLACE AGGREGATE to change it.\n", q, obj.ident)),
            ObjectKind::Trigger => {
                let table = obj.table.clone().unwrap_or_default();
                let def = scalar(
                    self,
                    &format!(
                        "SELECT pg_get_triggerdef(t.oid, true) FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace WHERE t.tgname = {} AND c.relname = {} AND n.nspname = {}",
                        lit(&obj.name),
                        lit(&table),
                        lit(schema)
                    ),
                )
                .await?
                .ok_or_else(|| anyhow::anyhow!("Trigger not found"))?;
                Ok(format!("DROP TRIGGER IF EXISTS {} ON {}.{};\n\n{};\n", d.quote_ident(&obj.name), d.quote_ident(schema), d.quote_ident(&table), def))
            }
            ObjectKind::Sequence => {
                let rows = collect(self, &format!("SELECT start_value, min_value, max_value, increment_by, cycle::text, cache_size, last_value FROM pg_sequences WHERE schemaname = {} AND sequencename = {}", lit(schema), lit(&obj.name))).await?;
                let r = rows.first().ok_or_else(|| anyhow::anyhow!("Sequence not found"))?;
                Ok(format!(
                    "-- current value: {}\nALTER SEQUENCE {q}\n    INCREMENT BY {}\n    MINVALUE {}\n    MAXVALUE {}\n    START WITH {}\n    CACHE {}\n    {}CYCLE;\n",
                    r[6].clone().unwrap_or_else(|| "(not used yet)".into()),
                    cell(r, 3),
                    cell(r, 1),
                    cell(r, 2),
                    cell(r, 0),
                    cell(r, 5),
                    if truthy(r[4].as_deref()) { "" } else { "NO " }
                ))
            }
            k => anyhow::bail!("Postgres has no {} objects", k.label()),
        }
    }

    pub(crate) async fn pg_explain(&mut self, stmt: &str, analyze: bool) -> anyhow::Result<Plan> {
        let modifies = databug_sql::classify(stmt, databug_core::Dialect::Postgres).modifies();
        let sql = format!("EXPLAIN (FORMAT JSON{}) {}", if analyze { ", ANALYZE, BUFFERS" } else { "" }, stmt);
        // EXPLAIN ANALYZE runs the statement; keep writes from sticking.
        let wrap = analyze && modifies;
        if wrap {
            self.client.simple_query("BEGIN").await.map_err(pg_error)?;
        }
        let res = self.client.simple_query(&sql).await.map_err(pg_error);
        if wrap {
            let _ = self.client.simple_query("ROLLBACK").await;
        }
        let mut raw = String::new();
        for m in res? {
            if let SimpleQueryMessage::Row(r) = m {
                raw.push_str(r.get(0).unwrap_or(""));
            }
        }
        let root = plan::from_pg_json(&raw);
        let pretty = serde_json::from_str::<serde_json::Value>(&raw).ok().and_then(|v| serde_json::to_string_pretty(&v).ok()).unwrap_or(raw);
        Ok(Plan { root, raw: pretty, format: "json" })
    }
}

impl PgConn {
    async fn run(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        // Column types come from a cheap prepare (parse + describe, no execution).
        let mut types: Vec<String> = Vec::new();
        if databug_sql::classify(stmt, databug_core::Dialect::Postgres) == databug_sql::StatementKind::Read && databug_sql::split_statements(stmt, databug_core::Dialect::Postgres).len() <= 1 {
            if let Ok(prep) = self.client.prepare(stmt).await {
                types = prep.columns().iter().map(|c| c.type_().name().to_string()).collect();
            }
        }
        let stream = self.client.simple_query_raw(stmt).await.map_err(pg_error)?;
        futures_util::pin_mut!(stream);
        let mut batch: Option<Batcher> = None;
        let mut stopped = false;
        loop {
            let msg = match stream.try_next().await {
                Ok(Some(m)) => m,
                Ok(None) => break,
                // We cancelled the rest of an over-limit result ourselves.
                Err(_) if stopped => break,
                Err(e) => return Err(pg_error(e)),
            };
            match msg {
                SimpleQueryMessage::RowDescription(cols) => {
                    let metas = cols.iter().enumerate().map(|(i, c)| ColumnMeta::new(c.name(), types.get(i).cloned().unwrap_or_default())).collect();
                    batch = Some(Batcher::new(sink, metas, opts.row_limit));
                    stopped = false;
                    types.clear();
                }
                SimpleQueryMessage::Row(row) => {
                    if stopped {
                        continue;
                    }
                    let Some(b) = batch.as_mut() else { continue };
                    for i in 0..row.len() {
                        b.cells().push(row.get(i));
                    }
                    if !b.row_done() {
                        // Stop the server from streaming rows we won't keep (engines
                        // without cancel just stream the rest, which is skipped).
                        stopped = true;
                        if self.capabilities().server_cancel {
                            let token = self.client.cancel_token();
                            let cancel = token.cancel_query(self.tls.clone());
                            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), cancel).await;
                        }
                    }
                }
                SimpleQueryMessage::CommandComplete(n) => {
                    if let Some(b) = batch.take() {
                        drop(b);
                    } else {
                        sink(QueryEvent::Affected(n));
                    }
                }
                _ => {}
            }
        }
        if stopped && self.capabilities().server_cancel {
            // The cancel reaches the server process as a signal; if the query
            // finished first, it would cancel whatever runs next. A throwaway
            // query takes it instead.
            let _ = self.client.simple_query("SELECT 1").await;
        }
        Ok(())
    }
}

const INTROSPECT: &str = "SELECT n.nspname, c.relname, CASE c.relkind WHEN 'v' THEN 'VIEW' WHEN 'm' THEN 'MATVIEW' ELSE 'TABLE' END, a.attname, format_type(a.atttypid, a.atttypmod), (NOT a.attnotnull)::text, pg_get_expr(d.adbin, d.adrelid), COALESCE((SELECT true FROM pg_index i WHERE i.indrelid = c.oid AND i.indisprimary AND a.attnum = ANY(i.indkey)), false)::text \
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
LEFT JOIN pg_attrdef d ON d.adrelid = c.oid AND d.adnum = a.attnum \
WHERE c.relkind IN ('r','v','m','p','f') AND n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%' AND n.nspname NOT LIKE 'pg_temp%' \
ORDER BY 1, 2, a.attnum";

const SYS: &str = "n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%' AND n.nspname NOT LIKE 'pg_temp%'";

const INDEXES: &str = "SELECT n.nspname, t.relname, i.relname, ix.indisunique::text, ix.indisprimary::text, \
array_to_string(ARRAY(SELECT a.attname FROM unnest(ix.indkey) WITH ORDINALITY k(attnum, ord) JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum ORDER BY k.ord), ',') \
FROM pg_index ix JOIN pg_class t ON t.oid = ix.indrelid JOIN pg_class i ON i.oid = ix.indexrelid JOIN pg_namespace n ON n.oid = t.relnamespace \
WHERE n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%'";

const FOREIGN_KEYS: &str = "SELECT n.nspname, t.relname, c.conname, \
array_to_string(ARRAY(SELECT a.attname FROM unnest(c.conkey) WITH ORDINALITY k(n, o) JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.n ORDER BY k.o), ','), \
rn.nspname, rt.relname, \
array_to_string(ARRAY(SELECT a.attname FROM unnest(c.confkey) WITH ORDINALITY k(n, o) JOIN pg_attribute a ON a.attrelid = c.confrelid AND a.attnum = k.n ORDER BY k.o), ','), \
c.confdeltype::text \
FROM pg_constraint c JOIN pg_class t ON t.oid = c.conrelid JOIN pg_namespace n ON n.oid = t.relnamespace \
JOIN pg_class rt ON rt.oid = c.confrelid JOIN pg_namespace rn ON rn.oid = rt.relnamespace \
WHERE c.contype = 'f' AND n.nspname NOT IN ('pg_catalog','information_schema')";

const ROUTINES: &str = "SELECT n.nspname, p.proname, \
CASE p.prokind WHEN 'p' THEN 'PROCEDURE' WHEN 'a' THEN 'AGGREGATE' ELSE CASE WHEN p.proretset THEN 'TABLEFUNCTION' ELSE 'FUNCTION' END END, \
pg_get_function_identity_arguments(p.oid), p.oid::regprocedure::text \
FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
WHERE n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%' \
AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.objid = p.oid AND d.deptype = 'e')";
