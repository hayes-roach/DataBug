//! Database drivers behind one async trait. Each engine is a cargo feature,
//! so builds can leave out drivers they don't need.

use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use databug_core::{Capabilities, Catalog, ColumnInfo, ColumnMeta, ConnectionConfig, DbKind, DbObject, ForeignKey, IndexInfo, ObjectKind, Plan, QueryEvent, ResultSet, SchemaInfo, TableInfo};

pub mod plan;

#[cfg(feature = "mongodb")]
mod mongo;
#[cfg(feature = "mssql")]
mod mssql;
#[cfg(feature = "mysql")]
mod mysql;
#[cfg(feature = "postgres")]
mod pg;
#[cfg(feature = "sqlite")]
mod sqlite;

/// Receives streamed events from a running statement.
pub type Sink = Arc<dyn Fn(QueryEvent) + Send + Sync>;

/// Asks the server to cancel the statement currently running on a connection.
pub type Canceller = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, Copy)]
pub struct ExecOptions {
    /// Stop fetching after this many rows per result set.
    pub row_limit: usize,
}

impl Default for ExecOptions {
    fn default() -> Self {
        ExecOptions { row_limit: 10_000 }
    }
}

#[async_trait]
pub trait DbConnection: Send {
    fn capabilities(&self) -> Capabilities;

    /// A handle that cancels server-side, if the engine supports it. When this
    /// is `None`, the caller aborts the request and reconnects instead.
    fn canceller(&self) -> Option<Canceller>;

    /// Run one statement (or one batch), streaming results into `sink`.
    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()>;

    async fn introspect(&mut self) -> anyhow::Result<Catalog>;

    async fn explain(&mut self, stmt: &str, analyze: bool) -> anyhow::Result<Plan>;

    /// Editable DDL for a view, routine, trigger, sequence or other object:
    /// `CREATE OR REPLACE …` (or the engine's equivalent), ready to run.
    async fn object_source(&mut self, schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        let _ = schema;
        anyhow::bail!("Viewing the source of a {} isn't supported for this database", obj.kind.label())
    }
}

/// Identifies an object whose definition should be fetched.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceRef {
    pub name: String,
    pub kind: ObjectKind,
    pub ident: String,
    pub table: Option<String>,
    /// Argument list for overloaded routines (matches `DbObject::detail`).
    pub detail: String,
}

impl SourceRef {
    pub fn from_object(o: &DbObject) -> Self {
        SourceRef { name: o.name.clone(), kind: o.kind, ident: o.ident.clone(), table: o.table.clone(), detail: o.detail.clone() }
    }

    pub fn relation(name: &str, kind: ObjectKind) -> Self {
        SourceRef { name: name.to_string(), kind, ident: String::new(), table: None, detail: String::new() }
    }
}

/// Extra structure gathered during introspection, attached by `build_catalog`.
#[derive(Default)]
pub(crate) struct Extras {
    pub indexes: Vec<(String, String, IndexInfo)>,
    pub foreign_keys: Vec<(String, String, ForeignKey)>,
    pub objects: Vec<(String, DbObject)>,
}

pub(crate) fn split_list(v: Option<&String>) -> Vec<String> {
    v.map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default()
}

pub(crate) fn cell(row: &[Option<String>], i: usize) -> String {
    row.get(i).cloned().flatten().unwrap_or_default()
}

/// Whether Windows authentication for SQL Server works in this build: on
/// Windows always (SSPI); on macOS/Linux only with the `kerberos` feature.
pub fn windows_auth_available() -> bool {
    #[cfg(feature = "mssql")]
    {
        mssql::integrated_auth_available()
    }
    #[cfg(not(feature = "mssql"))]
    {
        false
    }
}

#[cfg(feature = "mssql")]
pub use mssql::kerberos_principal;

/// Drivers compiled into this build.
pub fn available() -> Vec<DbKind> {
    let mut v = Vec::new();
    #[cfg(feature = "postgres")]
    v.push(DbKind::Postgres);
    #[cfg(feature = "mysql")]
    v.push(DbKind::MySql);
    #[cfg(feature = "mssql")]
    v.push(DbKind::MsSql);
    #[cfg(feature = "sqlite")]
    v.push(DbKind::Sqlite);
    #[cfg(feature = "mongodb")]
    v.push(DbKind::MongoDb);
    v
}

/// Open a connection. Must be called inside a Tokio runtime.
pub async fn connect(cfg: &ConnectionConfig, password: Option<&str>) -> anyhow::Result<Box<dyn DbConnection>> {
    init_crypto();
    let pw = password.unwrap_or("");
    let fut = async {
        match cfg.kind {
            #[cfg(feature = "postgres")]
            DbKind::Postgres => Ok(Box::new(pg::PgConn::connect(cfg, pw).await?) as Box<dyn DbConnection>),
            #[cfg(feature = "mysql")]
            DbKind::MySql => Ok(Box::new(mysql::MyConn::connect(cfg, pw).await?) as Box<dyn DbConnection>),
            #[cfg(feature = "mssql")]
            DbKind::MsSql => Ok(Box::new(mssql::MsConn::connect(cfg, pw).await?) as Box<dyn DbConnection>),
            #[cfg(feature = "sqlite")]
            DbKind::Sqlite => Ok(Box::new(sqlite::LiteConn::connect(cfg).await?) as Box<dyn DbConnection>),
            #[cfg(feature = "mongodb")]
            DbKind::MongoDb => Ok(Box::new(mongo::MongoConn::connect(cfg, pw).await?) as Box<dyn DbConnection>),
            #[allow(unreachable_patterns)]
            other => anyhow::bail!("{} support isn't included in this build", other.label()),
        }
    };
    match tokio::time::timeout(Duration::from_secs(20), fut).await {
        Ok(r) => r,
        Err(_) => anyhow::bail!("Connection timed out after 20 seconds"),
    }
}

/// Several dependencies enable different rustls crypto backends; pick one
/// process-wide so none of them panic.
fn init_crypto() {
    #[cfg(any(feature = "postgres", feature = "mysql"))]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }
}

/// Accumulates rows and flushes them to the sink in batches, so the UI sees
/// the first rows quickly without a channel message per row.
pub(crate) struct Batcher<'a> {
    sink: &'a Sink,
    width: usize,
    buf: ResultSet,
    total: usize,
    limit: usize,
    last_flush: Instant,
}

impl<'a> Batcher<'a> {
    pub fn new(sink: &'a Sink, columns: Vec<ColumnMeta>, limit: usize) -> Self {
        let width = columns.len();
        sink(QueryEvent::Columns(columns));
        Batcher { sink, width, buf: ResultSet::with_capacity(width, 64), total: 0, limit, last_flush: Instant::now() }
    }

    pub fn cells(&mut self) -> &mut ResultSet {
        &mut self.buf
    }

    /// Call after pushing one full row. Returns false when the row limit is reached.
    pub fn row_done(&mut self) -> bool {
        self.total += 1;
        if self.total > self.limit {
            // one row past the limit: drop it and report the cut
            self.buf.pop_row();
            self.flush();
            (self.sink)(QueryEvent::Truncated);
            return false;
        }
        let n = self.buf.rows();
        if n >= 2000 || (n >= 50 && self.last_flush.elapsed() > Duration::from_millis(80)) {
            self.flush();
        }
        true
    }

    pub fn flush(&mut self) {
        if !self.buf.is_empty() {
            let mut chunk = std::mem::replace(&mut self.buf, ResultSet::with_capacity(self.width, 256));
            chunk.shrink_to_fit();
            (self.sink)(QueryEvent::Rows(chunk));
        }
        self.last_flush = Instant::now();
    }
}

impl Drop for Batcher<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Run a query and collect every row (used for introspection).
pub(crate) async fn collect<C: DbConnection + ?Sized>(conn: &mut C, sql: &str) -> anyhow::Result<Vec<Vec<Option<String>>>> {
    let rows: Arc<Mutex<Vec<Vec<Option<String>>>>> = Arc::default();
    let r2 = rows.clone();
    let sink: Sink = Arc::new(move |ev| {
        if let QueryEvent::Rows(rs) = ev {
            let mut out = r2.lock().unwrap();
            for r in 0..rs.rows() {
                out.push((0..rs.width()).map(|c| rs.cell(r, c).map(str::to_string)).collect());
            }
        }
    });
    conn.execute(sql, &ExecOptions { row_limit: usize::MAX }, &sink).await?;
    drop(sink);
    let out = std::mem::take(&mut *rows.lock().unwrap());
    Ok(out)
}

pub(crate) fn truthy(v: Option<&str>) -> bool {
    matches!(v.map(|s| s.to_ascii_lowercase()).as_deref(), Some("1" | "t" | "true" | "yes" | "pri"))
}

/// Build a catalog from rows of
/// `schema, table, kind, column, type, nullable, default, is_pk`
/// where kind is `TABLE`, `VIEW` or `MATVIEW`.
pub(crate) fn build_catalog(rows: Vec<Vec<Option<String>>>, default_schema: String, extra_schemas: Vec<String>, extras: Extras) -> Catalog {
    let mut cat = Catalog { default_schema, introspected_at: now_secs(), schemas: Vec::new() };
    fn schema_idx(cat: &mut Catalog, name: &str) -> usize {
        match cat.schemas.iter().position(|s| s.name == name) {
            Some(i) => i,
            None => {
                cat.schemas.push(SchemaInfo { name: name.to_string(), ..Default::default() });
                cat.schemas.len() - 1
            }
        }
    }
    for s in extra_schemas {
        schema_idx(&mut cat, &s);
    }
    for row in rows {
        let (schema, table, kind) = (cell(&row, 0), cell(&row, 1), cell(&row, 2).to_ascii_uppercase());
        let si = schema_idx(&mut cat, &schema);
        let s = &mut cat.schemas[si];
        if s.tables.last().map(|t| t.name != table).unwrap_or(true) {
            let kind = if kind.contains("MAT") {
                ObjectKind::MaterializedView
            } else if kind.contains("VIEW") {
                ObjectKind::View
            } else {
                ObjectKind::Table
            };
            s.tables.push(TableInfo::new(table, kind));
        }
        let t = s.tables.last_mut().unwrap();
        let col = cell(&row, 3);
        if !col.is_empty() {
            t.columns.push(ColumnInfo {
                name: col,
                data_type: cell(&row, 4),
                nullable: truthy(row.get(5).and_then(|v| v.as_deref())),
                primary_key: truthy(row.get(7).and_then(|v| v.as_deref())),
                default: row.get(6).cloned().flatten(),
            });
        }
    }
    for (schema, table, idx) in extras.indexes {
        if let Some(t) = cat.schemas.iter_mut().find(|s| s.name == schema).and_then(|s| s.tables.iter_mut().find(|t| t.name == table)) {
            if !t.indexes.iter().any(|i| i.name == idx.name) {
                t.indexes.push(idx);
            }
        }
    }
    for (schema, table, fk) in extras.foreign_keys {
        if let Some(t) = cat.schemas.iter_mut().find(|s| s.name == schema).and_then(|s| s.tables.iter_mut().find(|t| t.name == table)) {
            t.foreign_keys.push(fk);
        }
    }
    for (schema, obj) in extras.objects {
        let si = schema_idx(&mut cat, &schema);
        cat.schemas[si].objects.push(obj);
    }
    let default = cat.default_schema.clone();
    cat.schemas.sort_by(|a, b| (a.name != default).cmp(&(b.name != default)).then(a.name.cmp(&b.name)));
    for s in &mut cat.schemas {
        s.tables.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        s.objects.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())).then(a.detail.cmp(&b.detail)));
        s.objects.dedup_by(|a, b| a.kind == b.kind && a.name == b.name && a.detail == b.detail && a.table == b.table);
        for t in &mut s.tables {
            t.indexes.sort_by(|a, b| b.primary.cmp(&a.primary).then(a.name.cmp(&b.name)));
        }
    }
    cat
}

/// Run a query and collect every row together with the column names.
pub(crate) async fn collect_named<C: DbConnection + ?Sized>(conn: &mut C, sql: &str) -> anyhow::Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
    let cols: Arc<Mutex<Vec<String>>> = Arc::default();
    let c2 = cols.clone();
    let rows: Arc<Mutex<Vec<Vec<Option<String>>>>> = Arc::default();
    let r2 = rows.clone();
    let sink: Sink = Arc::new(move |ev| match ev {
        QueryEvent::Columns(c) => *c2.lock().unwrap() = c.into_iter().map(|c| c.name).collect(),
        QueryEvent::Rows(rs) => {
            let mut out = r2.lock().unwrap();
            for r in 0..rs.rows() {
                out.push((0..rs.width()).map(|c| rs.cell(r, c).map(str::to_string)).collect());
            }
        }
        _ => {}
    });
    conn.execute(sql, &ExecOptions { row_limit: usize::MAX }, &sink).await?;
    drop(sink);
    let c = std::mem::take(&mut *cols.lock().unwrap());
    let r = std::mem::take(&mut *rows.lock().unwrap());
    Ok((c, r))
}

/// First cell of the first row.
pub(crate) async fn scalar<C: DbConnection + ?Sized>(conn: &mut C, sql: &str) -> anyhow::Result<Option<String>> {
    Ok(collect(conn, sql).await?.into_iter().next().and_then(|r| r.into_iter().next()).flatten())
}

/// Replace the first `CREATE` keyword in a definition with `replacement`
/// (e.g. `CREATE OR ALTER`), leaving comments before it alone.
pub fn replace_create(def: &str, dialect: databug_core::Dialect, replacement: &str) -> String {
    for t in databug_sql::tokenize(def, dialect) {
        if t.is_trivia() {
            continue;
        }
        if t.text(def).eq_ignore_ascii_case("CREATE") {
            let rest = &def[t.end..];
            // already `CREATE OR ...`
            if rest.trim_start().to_ascii_uppercase().starts_with("OR ") {
                return def.to_string();
            }
            return format!("{}{}{}", &def[..t.start], replacement, rest);
        }
        break;
    }
    def.to_string()
}

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
