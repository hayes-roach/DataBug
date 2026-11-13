use crate::{build_catalog, plan, Batcher, Canceller, DbConnection, ExecOptions, Extras, Sink, SourceRef};
use async_trait::async_trait;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use std::fmt::Write;
use std::sync::{Arc, Mutex};
use databug_core::{Capabilities, Catalog, ColumnMeta, ConnectionConfig, DbObject, ForeignKey, IndexInfo, ObjectKind, Plan, QueryEvent};

/// SQLite is synchronous, so every call runs on Tokio's blocking pool.
pub struct LiteConn {
    conn: Arc<Mutex<Connection>>,
    interrupt: Arc<rusqlite::InterruptHandle>,
}

impl LiteConn {
    pub async fn connect(cfg: &ConnectionConfig) -> anyhow::Result<Self> {
        let path = cfg.path.clone();
        let read_only = cfg.read_only;
        let conn = tokio::task::spawn_blocking(move || -> anyhow::Result<Connection> {
            if path.trim().is_empty() {
                anyhow::bail!("Choose a database file");
            }
            let flags = if read_only {
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI
            } else {
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_URI
            };
            let c = Connection::open_with_flags(&path, flags)?;
            c.busy_timeout(std::time::Duration::from_secs(5))?;
            Ok(c)
        })
        .await??;
        let interrupt = Arc::new(conn.get_interrupt_handle());
        Ok(LiteConn { conn: Arc::new(Mutex::new(conn)), interrupt })
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

fn value_text(v: ValueRef<'_>, out: &mut String) -> bool {
    match v {
        ValueRef::Null => false,
        ValueRef::Integer(i) => {
            let _ = write!(out, "{i}");
            true
        }
        ValueRef::Real(f) => {
            let _ = write!(out, "{f}");
            true
        }
        ValueRef::Text(t) => {
            out.push_str(&String::from_utf8_lossy(t));
            true
        }
        ValueRef::Blob(b) => {
            out.push_str("0x");
            for byte in b.iter().take(4096) {
                let _ = write!(out, "{byte:02X}");
            }
            true
        }
    }
}

pub(crate) fn run_statement(c: &Connection, stmt: &str, limit: usize, sink: &Sink) -> anyhow::Result<()> {
    let mut st = c.prepare(stmt)?;
    let width = st.column_count();
    if width == 0 {
        let n = st.execute([])?;
        sink(QueryEvent::Affected(n as u64));
        return Ok(());
    }
    let cols: Vec<ColumnMeta> = st.columns().map(|c| ColumnMeta::new(c.name(), c.decl_type().unwrap_or("").to_ascii_lowercase())).collect();
    let mut b = Batcher::new(sink, cols, limit);
    let mut rows = st.raw_query();
    while let Some(row) = rows.next()? {
        for i in 0..width {
            let v = row.get_ref(i)?;
            let rs = b.cells();
            let mut has = false;
            rs.push_with(|s| has = value_text(v, s));
            if !has {
                rs.mark_last_null();
            }
        }
        if !b.row_done() {
            break;
        }
    }
    Ok(())
}

#[async_trait]
impl DbConnection for LiteConn {
    fn capabilities(&self) -> Capabilities {
        Capabilities { schemas: true, transactions: true, explain: true, explain_analyze: false, server_cancel: true, sql: true }
    }

    fn canceller(&self) -> Option<Canceller> {
        let h = self.interrupt.clone();
        Some(Arc::new(move || h.interrupt()))
    }

    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        let stmt = stmt.to_string();
        let limit = opts.row_limit;
        let sink = sink.clone();
        self.blocking(move |c| run_statement(c, &stmt, limit, &sink)).await.map_err(|e| match e.downcast_ref::<rusqlite::Error>() {
            // "no such table: x" reads better than rusqlite's full error chain
            Some(rusqlite::Error::SqliteFailure(_, Some(msg))) => anyhow::anyhow!("{msg}"),
            _ => e,
        })
    }

    async fn introspect(&mut self) -> anyhow::Result<Catalog> {
        self.blocking(|c| {
            let mut rows: Vec<Vec<Option<String>>> = Vec::new();
            let mut extras = Extras::default();
            let mut dbs = Vec::new();
            {
                let mut st = c.prepare("PRAGMA database_list")?;
                let mut r = st.query([])?;
                while let Some(row) = r.next()? {
                    let name: String = row.get(1)?;
                    if name != "temp" {
                        dbs.push(name);
                    }
                }
            }
            let qi = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
            for db in &dbs {
                let q = format!("SELECT name, type, tbl_name FROM {}.sqlite_master WHERE type IN ('table','view','trigger') AND name NOT LIKE 'sqlite_%' ORDER BY name", qi(db));
                let mut st = c.prepare(&q)?;
                let objs: Vec<(String, String, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
                for (name, kind, tbl) in objs {
                    if kind == "trigger" {
                        extras.objects.push((db.clone(), DbObject { name, kind: ObjectKind::Trigger, detail: format!("on {tbl}"), ident: String::new(), table: Some(tbl) }));
                        continue;
                    }
                    let mut st = c.prepare(&format!("PRAGMA {}.table_info({})", qi(db), qi(&name)))?;
                    let mut r = st.query([])?;
                    let mut any = false;
                    let mut pk_cols: Vec<(i64, String)> = Vec::new();
                    while let Some(row) = r.next()? {
                        any = true;
                        let col: String = row.get(1)?;
                        let ty: String = row.get::<_, Option<String>>(2)?.unwrap_or_default();
                        let notnull: i64 = row.get(3)?;
                        let dflt: Option<String> = row.get(4)?;
                        let pk: i64 = row.get(5)?;
                        if pk > 0 {
                            pk_cols.push((pk, col.clone()));
                        }
                        rows.push(vec![
                            Some(db.clone()),
                            Some(name.clone()),
                            Some(kind.to_ascii_uppercase()),
                            Some(col),
                            Some(ty.to_ascii_lowercase()),
                            Some(if notnull == 0 && pk == 0 { "true".into() } else { "false".into() }),
                            dflt,
                            Some(if pk > 0 { "true".into() } else { "false".into() }),
                        ]);
                    }
                    if !any {
                        rows.push(vec![Some(db.clone()), Some(name.clone()), Some(kind.to_ascii_uppercase()), None, None, None, None, None]);
                    }
                    if kind != "table" {
                        continue;
                    }
                    if !pk_cols.is_empty() {
                        pk_cols.sort();
                        extras.indexes.push((db.clone(), name.clone(), IndexInfo { name: format!("{name}_pkey"), columns: pk_cols.into_iter().map(|p| p.1).collect(), unique: true, primary: true }));
                    }
                    // indexes
                    let mut st = c.prepare(&format!("PRAGMA {}.index_list({})", qi(db), qi(&name)))?;
                    let list: Vec<(String, i64, String)> = st.query_map([], |r| Ok((r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
                    for (iname, unique, origin) in list {
                        if origin == "pk" {
                            continue;
                        }
                        let mut st = c.prepare(&format!("PRAGMA {}.index_info({})", qi(db), qi(&iname)))?;
                        let cols: Vec<String> = st.query_map([], |r| r.get::<_, Option<String>>(2))?.filter_map(|r| r.ok().flatten()).collect();
                        extras.indexes.push((db.clone(), name.clone(), IndexInfo { name: iname, columns: cols, unique: unique == 1, primary: false }));
                    }
                    // foreign keys: (id, seq, table, from, to, on_update, on_delete, match)
                    let mut st = c.prepare(&format!("PRAGMA {}.foreign_key_list({})", qi(db), qi(&name)))?;
                    let fks: Vec<(i64, String, String, Option<String>, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(6)?)))?.collect::<Result<_, _>>()?;
                    let mut by_id: Vec<(i64, ForeignKey)> = Vec::new();
                    for (id, rt, from, to, on_delete) in fks {
                        let fk = match by_id.iter_mut().find(|(i, _)| *i == id) {
                            Some((_, f)) => f,
                            None => {
                                by_id.push((
                                    id,
                                    ForeignKey {
                                        name: format!("{name}_{rt}_fk{id}"),
                                        columns: vec![],
                                        ref_schema: db.clone(),
                                        ref_table: rt.clone(),
                                        ref_columns: vec![],
                                        on_delete: if on_delete == "NO ACTION" { String::new() } else { on_delete },
                                    },
                                ));
                                &mut by_id.last_mut().unwrap().1
                            }
                        };
                        fk.columns.push(from);
                        fk.ref_columns.push(to.unwrap_or_else(|| "rowid".into()));
                    }
                    for (_, fk) in by_id {
                        extras.foreign_keys.push((db.clone(), name.clone(), fk));
                    }
                }
            }
            Ok(build_catalog(rows, "main".into(), dbs, extras))
        })
        .await
    }

    async fn object_source(&mut self, schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        let schema = schema.to_string();
        let obj = obj.clone();
        self.blocking(move |c| {
            let qs = format!("\"{}\"", schema.replace('"', "\"\""));
            let sql: Option<String> = c
                .query_row(&format!("SELECT sql FROM {qs}.sqlite_master WHERE name = ?1"), [&obj.name], |r| r.get(0))
                .map_err(|_| anyhow::anyhow!("{} not found", obj.name))?;
            let sql = sql.unwrap_or_default();
            let q = format!("{qs}.\"{}\"", obj.name.replace('"', "\"\""));
            let verb = match obj.kind {
                ObjectKind::View => "VIEW",
                ObjectKind::Trigger => "TRIGGER",
                k => anyhow::bail!("SQLite has no {} objects", k.label()),
            };
            Ok(format!("-- SQLite can't alter a {} in place, so this drops and recreates it.\nDROP {verb} IF EXISTS {q};\n\n{};\n", obj.kind.label(), sql.trim_end_matches(';')))
        })
        .await
    }

    async fn explain(&mut self, stmt: &str, _analyze: bool) -> anyhow::Result<Plan> {
        let sql = format!("EXPLAIN QUERY PLAN {stmt}");
        self.blocking(move |c| {
            let mut st = c.prepare(&sql)?;
            let rows: Vec<(i64, i64, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
            let raw = rows.iter().map(|r| format!("{}|{}|{}", r.0, r.1, r.2)).collect::<Vec<_>>().join("\n");
            Ok(Plan { root: Some(plan::from_sqlite_rows(&rows)), raw, format: "text" })
        })
        .await
    }
}
