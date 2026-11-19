//! Redis over its own protocol (RESP2). The console runs one command per
//! line as typed in redis-cli; replies are shown as tables shaped for the
//! command (`HGETALL` as field / value, `ZRANGE … WITHSCORES` as member /
//! score, `XRANGE` as one row per entry, `INFO` as section / key / value).
//! The explorer lists keys (sampled with `SCAN`) with their type and expiry.

use crate::{Batcher, Canceller, DbConnection, ExecOptions, Sink, SourceRef};
use async_trait::async_trait;
use databug_core::{Capabilities, Catalog, ColumnInfo, ColumnMeta, ConnectionConfig, ObjectKind, Plan, QueryEvent, SchemaInfo, SslMode, TableInfo};
use databug_sql::redis::split_args;
use redis::aio::MultiplexedConnection;
use redis::{RedisConnectionInfo, Value};
use std::time::Duration;

/// Keys loaded for the explorer and completion.
const MAX_KEYS: usize = 5000;
/// Hashes whose fields are sampled for completion.
const MAX_HASHES_SAMPLED: usize = 200;

pub struct RedisConn {
    conn: MultiplexedConnection,
    db: i64,
}

/// The logical database from the connection's Database field (`0`–`15` by
/// default; empty means 0).
pub(crate) fn database_index(s: &str) -> anyhow::Result<i64> {
    let s = s.trim().trim_start_matches("db");
    if s.is_empty() {
        return Ok(0);
    }
    s.parse::<i64>().ok().filter(|n| *n >= 0).ok_or_else(|| anyhow::anyhow!("The database should be a number such as 0 (Redis databases are numbered)"))
}

async fn handshake<S>(info: &RedisConnectionInfo, stream: S) -> anyhow::Result<MultiplexedConnection>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (conn, driver) = MultiplexedConnection::new_with_config(info, stream, redis::AsyncConnectionConfig::default()).await.map_err(friendly)?;
    tokio::spawn(driver);
    Ok(conn)
}

fn friendly(e: redis::RedisError) -> anyhow::Error {
    let msg = e.to_string();
    if msg.contains("NOAUTH") || msg.contains("WRONGPASS") || msg.contains("invalid password") || msg.contains("AuthenticationFailed") {
        anyhow::anyhow!("Redis didn't accept the user name and password ({msg})")
    } else {
        anyhow::anyhow!(msg)
    }
}

impl RedisConn {
    pub async fn connect(cfg: &ConnectionConfig, password: &str) -> anyhow::Result<Self> {
        let host = cfg.host.trim();
        let host = if host.is_empty() { "localhost" } else { host }.to_string();
        let port = if cfg.port == 0 { 6379 } else { cfg.port };
        let db = database_index(&cfg.database)?;
        let mut info = RedisConnectionInfo::default().set_db(db).set_skip_set_lib_name();
        if !cfg.user.trim().is_empty() {
            info = info.set_username(cfg.user.trim());
        }
        if !password.is_empty() {
            info = info.set_password(password);
        }
        let tcp = tokio::time::timeout(Duration::from_secs(15), tokio::net::TcpStream::connect((host.as_str(), port)))
            .await
            .map_err(|_| anyhow::anyhow!("Timed out connecting to {host}:{port}"))??;
        tcp.set_nodelay(true)?;
        let conn = if cfg.ssl == SslMode::Require {
            let tls = tokio_rustls::TlsConnector::from(std::sync::Arc::new(crate::tls::no_verify_config()));
            let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|_| anyhow::anyhow!("{host} isn't a valid host name for TLS"))?;
            let stream = tls.connect(name, tcp).await?;
            handshake(&info, stream).await?
        } else {
            handshake(&info, tcp).await?
        };
        let mut c = RedisConn { conn, db };
        // the handshake signs in and selects the database; PING proves it
        c.run(&["PING".to_string()]).await?;
        Ok(c)
    }

    async fn run(&mut self, args: &[String]) -> anyhow::Result<Value> {
        let mut cmd = redis::cmd(&args[0]);
        for a in &args[1..] {
            cmd.arg(a);
        }
        cmd.query_async::<Value>(&mut self.conn).await.map_err(friendly)
    }

    /// Keys of the current database with their type and expiry, and a sample
    /// of each hash's fields.
    async fn keys(&mut self) -> anyhow::Result<Vec<TableInfo>> {
        let mut names: Vec<String> = Vec::new();
        let mut cursor = "0".to_string();
        loop {
            let v = self.run(&["SCAN".into(), cursor.clone(), "COUNT".into(), "1000".into()]).await?;
            let Value::Array(parts) = v else { break };
            cursor = parts.first().map(text).unwrap_or_else(|| "0".into());
            if let Some(Value::Array(ks)) = parts.get(1) {
                names.extend(ks.iter().map(text));
            }
            if cursor == "0" || names.len() >= MAX_KEYS {
                break;
            }
        }
        names.sort();
        names.dedup();
        names.truncate(MAX_KEYS);
        let mut out = Vec::with_capacity(names.len());
        for chunk in names.chunks(500) {
            let mut p = redis::pipe();
            for k in chunk {
                p.cmd("TYPE").arg(k).cmd("PTTL").arg(k);
            }
            let replies: Vec<Value> = p.query_async(&mut self.conn).await.map_err(friendly)?;
            for (i, k) in chunk.iter().enumerate() {
                let ty = replies.get(i * 2).map(text).unwrap_or_default();
                let ttl = match replies.get(i * 2 + 1) {
                    Some(Value::Int(ms)) if *ms >= 0 => human_ttl(*ms),
                    _ => "no expiry".to_string(),
                };
                let mut t = TableInfo::new(k.clone(), ObjectKind::Key);
                let col = |name: &str, data_type: String| ColumnInfo { name: name.into(), data_type, nullable: false, primary_key: false, default: None };
                t.columns.push(col("type", ty));
                t.columns.push(col("ttl", ttl));
                out.push(t);
            }
        }
        // hash fields, for completing HGET key <field>
        let hashes: Vec<usize> = out.iter().enumerate().filter(|(_, t)| t.columns[0].data_type == "hash").map(|(i, _)| i).take(MAX_HASHES_SAMPLED).collect();
        for i in hashes {
            let v = self.run(&["HSCAN".into(), out[i].name.clone(), "0".into(), "COUNT".into(), "50".into()]).await?;
            if let Value::Array(parts) = v {
                if let Some(Value::Array(kv)) = parts.get(1) {
                    for f in kv.iter().step_by(2).take(50) {
                        out[i].columns.push(ColumnInfo { name: text(f), data_type: "field".into(), nullable: true, primary_key: false, default: None });
                    }
                }
            }
        }
        Ok(out)
    }
}

fn human_ttl(ms: i64) -> String {
    let s = ms / 1000;
    match s {
        s if s >= 86_400 => format!("expires in {}d {}h", s / 86_400, (s % 86_400) / 3600),
        s if s >= 3600 => format!("expires in {}h {}m", s / 3600, (s % 3600) / 60),
        s if s >= 60 => format!("expires in {}m {}s", s / 60, s % 60),
        s => format!("expires in {s}s"),
    }
}

/// A reply as text (`None` for nil); nested replies as JSON.
fn cell(v: &Value) -> Option<String> {
    match v {
        Value::Nil => None,
        Value::Int(i) => Some(i.to_string()),
        Value::BulkString(b) => Some(String::from_utf8_lossy(b).into_owned()),
        Value::SimpleString(s) => Some(s.clone()),
        Value::Okay => Some("OK".into()),
        Value::Double(d) => Some(d.to_string()),
        Value::Boolean(b) => Some(b.to_string()),
        Value::VerbatimString { text, .. } => Some(text.clone()),
        other => Some(json(other).to_string()),
    }
}

fn text(v: &Value) -> String {
    cell(v).unwrap_or_default()
}

fn json(v: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Nil => J::Null,
        Value::Int(i) => J::from(*i),
        Value::Double(d) => J::from(*d),
        Value::Boolean(b) => J::from(*b),
        Value::Array(a) | Value::Set(a) => J::Array(a.iter().map(json).collect()),
        Value::Map(m) => J::Object(m.iter().map(|(k, v)| (text(k), json(v))).collect()),
        Value::Attribute { data, .. } => json(data),
        other => cell(other).map(J::String).unwrap_or(J::Null),
    }
}

/// A stream entry's field and value.
type Field = (String, Option<String>);

/// Rows for a result grid.
struct Table {
    columns: Vec<(String, &'static str)>,
    rows: Vec<Vec<Option<String>>>,
}

fn send(t: Table, limit: usize, sink: &Sink) {
    let metas = t.columns.iter().map(|(n, ty)| ColumnMeta::new(n.clone(), *ty)).collect();
    let mut b = Batcher::new(sink, metas, limit);
    for row in t.rows {
        for c in row {
            b.cells().push(c.as_deref());
        }
        if !b.row_done() {
            break;
        }
    }
}

fn pairs(items: &[Value], a: &'static str, b: &'static str, b_type: &'static str) -> Table {
    Table { columns: vec![(a.into(), "string"), (b.into(), b_type)], rows: items.chunks(2).map(|p| vec![p.first().and_then(cell), p.get(1).and_then(cell)]).collect() }
}

/// Shape a reply into a table for the command that produced it.
fn shape(args: &[String], v: &Value) -> Table {
    let name = args[0].to_ascii_uppercase();
    let has = |w: &str| args.iter().skip(1).any(|a| a.eq_ignore_ascii_case(w));
    let scalar = |v: &Value| Table { columns: vec![("value".into(), "string")], rows: vec![vec![cell(v)]] };
    match v {
        Value::Map(m) => Table { columns: vec![("field".into(), "string"), ("value".into(), "string")], rows: m.iter().map(|(k, v)| vec![cell(k), cell(v)]).collect() },
        Value::Array(items) | Value::Set(items) => match name.as_str() {
            "HGETALL" | "CONFIG" => pairs(items, "field", "value", "string"),
            "ZRANGE" | "ZREVRANGE" | "ZRANGEBYSCORE" | "ZREVRANGEBYSCORE" | "ZRANDMEMBER" if has("WITHSCORES") => pairs(items, "member", "score", "double"),
            "ZPOPMIN" | "ZPOPMAX" => pairs(items, "member", "score", "double"),
            "HRANDFIELD" if has("WITHVALUES") => pairs(items, "field", "value", "string"),
            "SCAN" | "HSCAN" | "SSCAN" | "ZSCAN" if items.len() == 2 => {
                let next = text(&items[0]);
                let list = match &items[1] {
                    Value::Array(a) => a.as_slice(),
                    _ => &[],
                };
                let mut t = match name.as_str() {
                    "HSCAN" => pairs(list, "field", "value", "string"),
                    "ZSCAN" => pairs(list, "member", "score", "double"),
                    "SSCAN" => Table { columns: vec![("member".into(), "string")], rows: list.iter().map(|m| vec![cell(m)]).collect() },
                    _ => Table { columns: vec![("key".into(), "string")], rows: list.iter().map(|m| vec![cell(m)]).collect() },
                };
                t.columns.push(("next cursor".into(), "string"));
                for r in &mut t.rows {
                    r.push(Some(next.clone()));
                }
                t
            }
            "XRANGE" | "XREVRANGE" => {
                // one row per entry: id, then each field as a column
                let mut fields: Vec<String> = Vec::new();
                let mut entries: Vec<(String, Vec<Field>)> = Vec::new();
                for e in items {
                    let Value::Array(parts) = e else { continue };
                    let id = parts.first().map(text).unwrap_or_default();
                    let mut kv = Vec::new();
                    if let Some(Value::Array(f)) = parts.get(1) {
                        for p in f.chunks(2) {
                            let k = p.first().map(text).unwrap_or_default();
                            if !fields.contains(&k) {
                                fields.push(k.clone());
                            }
                            kv.push((k, p.get(1).and_then(cell)));
                        }
                    }
                    entries.push((id, kv));
                }
                let mut columns: Vec<(String, &'static str)> = vec![("id".into(), "string")];
                columns.extend(fields.iter().map(|f| (f.clone(), "string")));
                let rows = entries
                    .into_iter()
                    .map(|(id, kv)| {
                        let mut r = vec![Some(id)];
                        r.extend(fields.iter().map(|f| kv.iter().find(|(k, _)| k == f).and_then(|(_, v)| v.clone())));
                        r
                    })
                    .collect();
                Table { columns, rows }
            }
            "LRANGE" => Table { columns: vec![("index".into(), "integer"), ("value".into(), "string")], rows: items.iter().enumerate().map(|(i, v)| vec![Some(i.to_string()), cell(v)]).collect() },
            "MGET" | "HMGET" | "SMISMEMBER" | "ZMSCORE" => {
                let names: Vec<&String> = if name == "MGET" { args.iter().skip(1).collect() } else { args.iter().skip(2).collect() };
                let label = if name == "MGET" { "key" } else if name == "ZMSCORE" || name == "SMISMEMBER" { "member" } else { "field" };
                Table { columns: vec![(label.into(), "string"), ("value".into(), "string")], rows: items.iter().enumerate().map(|(i, v)| vec![names.get(i).map(|s| s.to_string()), cell(v)]).collect() }
            }
            "KEYS" => Table { columns: vec![("key".into(), "string")], rows: items.iter().map(|v| vec![cell(v)]).collect() },
            "SMEMBERS" | "SINTER" | "SUNION" | "SDIFF" | "SRANDMEMBER" | "SPOP" => Table { columns: vec![("member".into(), "string")], rows: items.iter().map(|v| vec![cell(v)]).collect() },
            "HKEYS" => Table { columns: vec![("field".into(), "string")], rows: items.iter().map(|v| vec![cell(v)]).collect() },
            _ => Table { columns: vec![("#".into(), "integer"), ("value".into(), "string")], rows: items.iter().enumerate().map(|(i, v)| vec![Some((i + 1).to_string()), cell(v)]).collect() },
        },
        Value::BulkString(b) if name == "INFO" => {
            let s = String::from_utf8_lossy(b);
            let mut section = String::new();
            let mut rows = Vec::new();
            for line in s.lines() {
                if let Some(h) = line.strip_prefix("# ") {
                    section = h.trim().to_string();
                } else if let Some((k, v)) = line.split_once(':') {
                    rows.push(vec![Some(section.clone()), Some(k.to_string()), Some(v.trim().to_string())]);
                }
            }
            Table { columns: vec![("section".into(), "string"), ("key".into(), "string"), ("value".into(), "string")], rows }
        }
        Value::BulkString(b) if b.contains(&b'\n') => {
            let s = String::from_utf8_lossy(b);
            Table { columns: vec![("line".into(), "string")], rows: s.lines().map(|l| vec![Some(l.to_string())]).collect() }
        }
        Value::Int(_) => Table { columns: vec![("value".into(), "integer")], rows: vec![vec![cell(v)]] },
        other => scalar(other),
    }
}

/// Commands that never finish in a console (they stream until the
/// connection closes).
fn streams_forever(name: &str) -> bool {
    matches!(name, "SUBSCRIBE" | "PSUBSCRIBE" | "SSUBSCRIBE" | "MONITOR" | "SYNC" | "PSYNC")
}

#[async_trait]
impl DbConnection for RedisConn {
    fn capabilities(&self) -> Capabilities {
        // MULTI/EXEC is Redis's own batching, not a transaction you can roll back
        Capabilities { schemas: false, transactions: false, explain: false, explain_analyze: false, server_cancel: false, sql: false }
    }

    fn canceller(&self) -> Option<Canceller> {
        None
    }

    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        let args = split_args(stmt.trim()).map_err(|e| anyhow::anyhow!(e))?;
        let Some(first) = args.first() else { return Ok(()) };
        let name = first.to_ascii_uppercase();
        if streams_forever(&name) {
            anyhow::bail!("{name} streams until the connection closes, so it can't run in a console. Use redis-cli for it");
        }
        let v = self.run(&args).await?;
        if name == "SELECT" {
            if let Some(n) = args.get(1).and_then(|n| n.parse().ok()) {
                self.db = n;
                sink(QueryEvent::Message(format!("Now using database {n}")));
            }
        }
        match &v {
            Value::Okay => sink(QueryEvent::Message("OK".into())),
            Value::SimpleString(s) => sink(QueryEvent::Message(s.clone())),
            Value::Nil => {
                sink(QueryEvent::Message("(nil)".into()));
                send(shape(&args, &v), opts.row_limit, sink);
            }
            Value::Int(n) if databug_sql::redis::command(&name).is_some_and(|c| c.access != databug_sql::redis::Access::Read) => {
                sink(QueryEvent::Message(format!("(integer) {n}")));
            }
            _ => send(shape(&args, &v), opts.row_limit, sink),
        }
        Ok(())
    }

    async fn introspect(&mut self) -> anyhow::Result<Catalog> {
        let tables = self.keys().await?;
        let schema = format!("db{}", self.db);
        Ok(Catalog { default_schema: schema.clone(), introspected_at: crate::now_secs(), schemas: vec![SchemaInfo { name: schema, tables, objects: vec![] }] })
    }

    async fn object_source(&mut self, _schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        anyhow::bail!("Redis {}s have no definition to show", obj.kind.label())
    }

    async fn explain(&mut self, _stmt: &str, _analyze: bool) -> anyhow::Result<Plan> {
        anyhow::bail!("Redis has no query plans")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bulk(s: &str) -> Value {
        Value::BulkString(s.as_bytes().to_vec())
    }
    fn args(s: &str) -> Vec<String> {
        split_args(s).unwrap()
    }
    fn names(t: &Table) -> Vec<&str> {
        t.columns.iter().map(|c| c.0.as_str()).collect()
    }

    #[test]
    fn databases_are_numbers() {
        assert_eq!(database_index("").unwrap(), 0);
        assert_eq!(database_index("3").unwrap(), 3);
        assert_eq!(database_index("db2").unwrap(), 2);
        assert!(database_index("cache").is_err());
    }

    #[test]
    fn replies_take_the_commands_shape() {
        let t = shape(&args("HGETALL user:1"), &Value::Array(vec![bulk("name"), bulk("Avery"), bulk("plan"), bulk("pro")]));
        assert_eq!(names(&t), vec!["field", "value"]);
        assert_eq!(t.rows[1], vec![Some("plan".into()), Some("pro".into())]);

        let t = shape(&args("ZRANGE lb 0 -1 WITHSCORES"), &Value::Array(vec![bulk("p1"), bulk("10"), bulk("p2"), bulk("20")]));
        assert_eq!(t.columns[1], ("score".to_string(), "double"));
        assert_eq!(t.rows.len(), 2);

        let t = shape(&args("SCAN 0 MATCH user:*"), &Value::Array(vec![bulk("17"), Value::Array(vec![bulk("user:1"), bulk("user:2")])]));
        assert_eq!(names(&t), vec!["key", "next cursor"]);
        assert_eq!(t.rows[0][1].as_deref(), Some("17"));

        let entry = |id: &str, kv: &[&str]| Value::Array(vec![bulk(id), Value::Array(kv.iter().map(|s| bulk(s)).collect())]);
        let t = shape(&args("XRANGE events - +"), &Value::Array(vec![entry("1-0", &["order", "o1", "status", "paid"]), entry("2-0", &["order", "o2", "total", "9.5"])]));
        assert_eq!(names(&t), vec!["id", "order", "status", "total"]);
        assert_eq!(t.rows[1], vec![Some("2-0".into()), Some("o2".into()), None, Some("9.5".into())]);

        let t = shape(&args("INFO server"), &bulk("# Server\r\nredis_version:8.0.0\r\nos:Linux\r\n"));
        assert_eq!(t.rows[0], vec![Some("Server".into()), Some("redis_version".into()), Some("8.0.0".into())]);

        let t = shape(&args("MGET a b"), &Value::Array(vec![bulk("1"), Value::Nil]));
        assert_eq!(t.rows[1], vec![Some("b".into()), None]);

        let t = shape(&args("LRANGE q 0 -1"), &Value::Array(vec![bulk("x"), bulk("y")]));
        assert_eq!(t.rows[1], vec![Some("1".into()), Some("y".into())]);
    }

    #[test]
    fn ttls_read_naturally() {
        assert_eq!(human_ttl(5_000), "expires in 5s");
        assert_eq!(human_ttl(3_700_000), "expires in 1h 1m");
        assert_eq!(human_ttl(90_000_000), "expires in 1d 1h");
    }
}
