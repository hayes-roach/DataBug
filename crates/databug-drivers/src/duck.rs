//! DuckDB through its C API, from a library loaded at run time (see
//! `duck_lib`). DuckDB runs in-process and is synchronous, so every call
//! runs on Tokio's blocking pool, like SQLite.

use crate::duck_lib::{api, cstr, Api, Handle, RawResult};
use crate::{infoschema, Batcher, Canceller, DbConnection, ExecOptions, Sink, SourceRef};
use async_trait::async_trait;
use databug_core::{Capabilities, Catalog, ColumnMeta, ConnectionConfig, DbObject, Dialect, ObjectKind, Plan, QueryEvent};
use std::ffi::CString;
use std::sync::{Arc, Mutex};

/// An open database file, shared by every DataBug session on it. DuckDB
/// allows one instance per file per process (on Windows its file lock is
/// exclusive even within a process), so sessions share the instance and each
/// opens its own connection to it. Closed when the last session ends.
struct Database {
    api: Arc<Api>,
    db: Handle,
}

// SAFETY: a `duckdb_database` may be used from any thread; DuckDB serializes
// access internally (`duckdb_connect` is thread-safe).
unsafe impl Send for Database {}
// SAFETY: see above; shared use is only `duckdb_connect`.
unsafe impl Sync for Database {}

impl Drop for Database {
    fn drop(&mut self) {
        // SAFETY: `db` was opened by `open_database` and is closed exactly once,
        // after every connection to it (each holds an `Arc`) has been closed.
        unsafe { (self.api.close)(&mut self.db) }
    }
}

/// Open databases by file (and access mode), so a second session reuses the
/// first one's instance instead of failing on the file lock.
static DATABASES: Mutex<Vec<(String, std::sync::Weak<Database>)>> = Mutex::new(Vec::new());

/// The registry key: the file's canonical path, or the connection for an
/// in-memory database (so its sessions see the same tables).
fn database_key(cfg_id: &str, path: &str, read_only: bool) -> String {
    let file = if path == ":memory:" {
        format!(":memory:{}", ConnectionConfig::base_id(cfg_id))
    } else {
        // resolve the folder, not the file: the file may not exist until the
        // first session creates it, and the key mustn't change when it does
        let p = std::path::Path::new(path);
        let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
        match (std::fs::canonicalize(dir), p.file_name()) {
            (Ok(d), Some(name)) => d.join(name).to_string_lossy().into_owned(),
            _ => path.to_string(),
        }
    };
    format!("{}|{file}", if read_only { "ro" } else { "rw" })
}

/// The shared instance for this file, opening it if no session has it open.
fn open_database(api: &Arc<Api>, key: &str, path: &str, read_only: bool) -> anyhow::Result<Arc<Database>> {
    let mut open = DATABASES.lock().map_err(|_| anyhow::anyhow!("DuckDB registry poisoned"))?;
    open.retain(|(_, w)| w.strong_count() > 0);
    if let Some(db) = open.iter().find(|(k, _)| k == key).and_then(|(_, w)| w.upgrade()) {
        return Ok(db);
    }
    // SAFETY: every handle passed to DuckDB is either null (to be filled) or
    // one it just returned; `cpath` and the config strings are NUL-terminated
    // and outlive the calls; the config is destroyed after opening.
    let db = unsafe {
        let mut config: Handle = std::ptr::null_mut();
        if (api.create_config)(&mut config) != 0 {
            anyhow::bail!("DuckDB couldn't create a configuration");
        }
        if read_only {
            (api.set_config)(config, c"access_mode".as_ptr(), c"READ_ONLY".as_ptr());
        }
        let cpath = CString::new(if path == ":memory:" { "" } else { path })?;
        let mut db: Handle = std::ptr::null_mut();
        let mut err: *mut std::ffi::c_char = std::ptr::null_mut();
        let state = (api.open_ext)(cpath.as_ptr(), &mut db, config, &mut err);
        (api.destroy_config)(&mut config);
        if state != 0 {
            let msg = cstr(err);
            if !err.is_null() {
                (api.free)(err.cast());
            }
            anyhow::bail!("{}", if msg.is_empty() { "DuckDB couldn't open the database".into() } else { msg });
        }
        Arc::new(Database { api: api.clone(), db })
    };
    open.push((key.to_string(), Arc::downgrade(&db)));
    Ok(db)
}

/// One session's connection to a shared database. Shared with the
/// canceller, which calls `duckdb_interrupt` (thread-safe) while a query
/// holds `lock`.
struct Inner {
    api: Arc<Api>,
    _db: Arc<Database>,
    conn: Handle,
    lock: Mutex<()>,
}

// SAFETY: DuckDB connections may be used from any thread when calls are
// serialized (`lock`); `duckdb_interrupt` may be called concurrently.
unsafe impl Send for Inner {}
// SAFETY: shared access only issues DuckDB calls under `lock`, except
// `duckdb_interrupt`, which DuckDB allows concurrently.
unsafe impl Sync for Inner {}

impl Drop for Inner {
    fn drop(&mut self) {
        // SAFETY: `conn` was opened by `connect` and is closed exactly once,
        // here, before `_db` (declared earlier but released after this body
        // runs) can close the database.
        unsafe { (self.api.disconnect)(&mut self.conn) }
    }
}

/// A materialized result, destroyed on drop.
struct QueryResult<'a> {
    api: &'a Api,
    raw: RawResult,
}

impl Drop for QueryResult<'_> {
    fn drop(&mut self) {
        // SAFETY: `raw` was filled by `duckdb_query` (or zeroed, which DuckDB
        // accepts) and is destroyed once, when the result is dropped.
        unsafe { (self.api.destroy_result)(&mut self.raw) }
    }
}

impl QueryResult<'_> {
    fn columns(&mut self) -> usize {
        // SAFETY: `raw` is a live result owned by `self`.
        unsafe { (self.api.column_count)(&mut self.raw) as usize }
    }
    fn name(&mut self, c: usize) -> String {
        // SAFETY: `raw` is live; DuckDB returns NULL for an out-of-range column,
        // which `cstr` handles, and the name stays valid while `raw` lives.
        cstr(unsafe { (self.api.column_name)(&mut self.raw, c as u64) })
    }
    fn rows(&mut self) -> usize {
        // SAFETY: `raw` is a live result owned by `self`.
        unsafe { (self.api.row_count)(&mut self.raw) as usize }
    }
    fn changed(&mut self) -> u64 {
        // SAFETY: `raw` is a live result owned by `self`.
        unsafe { (self.api.rows_changed)(&mut self.raw) }
    }
    /// The value cast to text, or `None` for NULL.
    fn value(&mut self, c: usize, r: usize) -> Option<String> {
        // SAFETY: `raw` is live; `value_varchar` returns a newly allocated C
        // string (or NULL) that we copy and then release with `duckdb_free`.
        unsafe {
            if (self.api.value_is_null)(&mut self.raw, c as u64, r as u64) {
                return None;
            }
            let p = (self.api.value_varchar)(&mut self.raw, c as u64, r as u64);
            if p.is_null() {
                return None;
            }
            let s = cstr(p);
            (self.api.free)(p.cast());
            Some(s)
        }
    }
}

impl Inner {
    fn query(&self, sql: &str) -> anyhow::Result<QueryResult<'_>> {
        let c = CString::new(sql).map_err(|_| anyhow::anyhow!("The statement contains a NUL character"))?;
        let mut res = QueryResult { api: &self.api, raw: RawResult::zeroed() };
        // SAFETY: `conn` is open for the life of `Inner`, `c` is NUL-terminated and
        // outlives the call, and `res.raw` is a zeroed result for DuckDB to fill.
        // Callers hold `lock` (see `DuckConn::blocking`), so calls don't overlap.
        let state = unsafe { (self.api.query)(self.conn, c.as_ptr(), &mut res.raw) };
        if state != 0 {
            // SAFETY: `res.raw` was just filled by `duckdb_query`; the message is
            // owned by the result and copied before it's destroyed.
            let msg = cstr(unsafe { (self.api.result_error)(&mut res.raw) });
            anyhow::bail!("{}", if msg.is_empty() { "DuckDB reported an error".to_string() } else { msg });
        }
        Ok(res)
    }
}

pub struct DuckConn {
    inner: Arc<Inner>,
}

impl DuckConn {
    pub async fn connect(cfg: &ConnectionConfig) -> anyhow::Result<Self> {
        let path = cfg.path.trim().to_string();
        let read_only = cfg.read_only;
        let key = database_key(&cfg.id, &path, read_only);
        let inner = tokio::task::spawn_blocking(move || -> anyhow::Result<Inner> {
            if path.is_empty() {
                anyhow::bail!("Choose a database file, or enter :memory: for a scratch database");
            }
            let api = api()?;
            let db = open_database(&api, &key, &path, read_only)?;
            let mut conn: Handle = std::ptr::null_mut();
            // SAFETY: `db.db` is open while `db` lives; `conn` is filled by DuckDB.
            if unsafe { (api.connect)(db.db, &mut conn) } != 0 {
                anyhow::bail!("DuckDB couldn't open a connection");
            }
            Ok(Inner { api, _db: db, conn, lock: Mutex::new(()) })
        })
        .await??;
        Ok(DuckConn { inner: Arc::new(inner) })
    }

    async fn blocking<T: Send + 'static>(&self, f: impl FnOnce(&Inner) -> anyhow::Result<T> + Send + 'static) -> anyhow::Result<T> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _g = inner.lock.lock().map_err(|_| anyhow::anyhow!("connection poisoned"))?;
            f(&inner)
        })
        .await?
    }
}

/// DuckDB answers INSERT/UPDATE/DELETE with a one-row `Count` result.
fn is_count_result(fields: &[String], modifies: bool) -> bool {
    modifies && fields.len() == 1 && fields[0].eq_ignore_ascii_case("count")
}

/// Queries that can be wrapped as a subquery, so DuckDB casts every value
/// to text and stops at the row limit instead of materializing everything.
pub(crate) fn wrappable(stmt: &str) -> bool {
    if databug_sql::split_statements(stmt, Dialect::Postgres).len() > 1 {
        return false;
    }
    let first = databug_sql::tokenize(stmt, Dialect::Postgres).into_iter().find(|t| !t.is_trivia()).map(|t| t.text(stmt).to_ascii_uppercase());
    match first.as_deref() {
        // DuckDB's FROM-first shorthand (`FROM orders`) is always a query
        Some("FROM") => true,
        Some("SELECT" | "WITH" | "VALUES" | "TABLE" | "(") => databug_sql::classify(stmt, Dialect::Postgres) == databug_sql::StatementKind::Read,
        _ => false,
    }
}

pub(crate) fn wrap(stmt: &str, limit: usize) -> String {
    let limit = if limit == usize::MAX { String::new() } else { format!(" LIMIT {}", limit + 1) };
    // the newline keeps a trailing `-- comment` from swallowing the `)`
    format!("SELECT CAST(COLUMNS(*) AS VARCHAR) FROM (\n{stmt}\n){limit}")
}

fn send_rows(res: &mut QueryResult<'_>, names: Vec<String>, types: Vec<String>, limit: usize, sink: &Sink) {
    let width = names.len();
    let cols = names.into_iter().enumerate().map(|(i, n)| ColumnMeta::new(n, types.get(i).cloned().unwrap_or_default())).collect();
    let mut b = Batcher::new(sink, cols, limit);
    for r in 0..res.rows() {
        for c in 0..width {
            let v = res.value(c, r);
            b.cells().push(v.as_deref());
        }
        if !b.row_done() {
            break;
        }
    }
}

fn run_statement(c: &Inner, stmt: &str, limit: usize, sink: &Sink) -> anyhow::Result<()> {
    let stmt = stmt.trim().trim_end_matches(';').trim_end();
    if wrappable(stmt) {
        // column names and types of the original query
        let described: Vec<(String, String)> = match c.query(&format!("DESCRIBE {stmt}")) {
            Ok(mut d) => (0..d.rows()).map(|r| (d.value(0, r).unwrap_or_default(), d.value(1, r).unwrap_or_default().to_ascii_lowercase())).collect(),
            Err(_) => Vec::new(),
        };
        match c.query(&wrap(stmt, limit)) {
            Ok(mut res) => {
                let width = res.columns();
                let names = if described.len() == width { described.iter().map(|d| d.0.clone()).collect() } else { (0..width).map(|i| res.name(i)).collect() };
                let types = described.into_iter().map(|d| d.1).collect();
                send_rows(&mut res, names, types, limit, sink);
                return Ok(());
            }
            // cancelled: don't run it again
            Err(e) if e.to_string().to_ascii_lowercase().contains("interrupt") => return Err(e),
            // otherwise run it as written, for DuckDB's own error (or result)
            Err(_) => {}
        }
    }
    let mut res = c.query(stmt)?;
    let width = res.columns();
    let names: Vec<String> = (0..width).map(|i| res.name(i)).collect();
    let modifies = databug_sql::classify(stmt, Dialect::Postgres).modifies();
    if width == 0 {
        sink(QueryEvent::Affected(res.changed()));
    } else if is_count_result(&names, modifies) {
        let n = res.value(0, 0).and_then(|v| v.parse().ok()).unwrap_or_else(|| res.changed());
        sink(QueryEvent::Affected(n));
    } else {
        send_rows(&mut res, names, Vec::new(), limit, sink);
    }
    Ok(())
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
        let inner = self.inner.clone();
        // SAFETY: `duckdb_interrupt` is documented as safe to call from another
        // thread while a query runs; the `Arc` keeps `conn` open meanwhile.
        Some(Arc::new(move || unsafe { (inner.api.interrupt)(inner.conn) }))
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
        let sql = format!("EXPLAIN {}{}", if analyze { "ANALYZE " } else { "" }, stmt.trim().trim_end_matches(';'));
        // EXPLAIN ANALYZE runs the statement; keep writes from sticking.
        let wrap = analyze && modifies;
        self.blocking(move |c| {
            if wrap {
                c.query("BEGIN")?;
            }
            let res = (|| -> anyhow::Result<String> {
                let mut r = c.query(&sql)?;
                // (explain_key, explain_value): the value holds the drawn plan
                Ok((0..r.rows()).filter_map(|i| r.value(1, i)).collect::<Vec<_>>().join("\n"))
            })();
            if wrap {
                let _ = c.query("ROLLBACK");
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
    fn wraps_only_plain_queries() {
        assert!(wrappable("SELECT * FROM orders"));
        assert!(wrappable("with t as (select 1) select * from t"));
        assert!(wrappable("FROM orders"));
        assert!(!wrappable("DESCRIBE orders"));
        assert!(!wrappable("SHOW TABLES"));
        assert!(!wrappable("UPDATE orders SET total = 1"));
        assert!(!wrappable("SELECT 1; SELECT 2"));
        assert_eq!(wrap("SELECT 1 -- one", 100), "SELECT CAST(COLUMNS(*) AS VARCHAR) FROM (\nSELECT 1 -- one\n) LIMIT 101");
        assert!(!wrap("SELECT 1", usize::MAX).contains("LIMIT"));
    }
}
