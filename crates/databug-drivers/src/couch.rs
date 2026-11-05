//! Apache CouchDB over its HTTP API. Each CouchDB database is shown as a
//! collection, and the console takes Mango queries written the mongo-shell
//! way: `db.orders.find({status: "open"}).sort({placed_at: -1}).limit(20)`.
//! `GET /path` lines send a raw read-only request.

use crate::http::{check, send_objects};
use crate::{Canceller, DbConnection, ExecOptions, Sink, SourceRef};
use async_trait::async_trait;
use databug_core::{Capabilities, Catalog, ColumnInfo, ConnectionConfig, IndexInfo, ObjectKind, Plan, QueryEvent, SchemaInfo, TableInfo};
use databug_sql::mongo::{parse, split_args, MongoCall, MongoStatement};
use serde_json::{json, Map, Value};

pub struct CouchConn {
    http: reqwest::Client,
    base: String,
    user: String,
    password: String,
    database: String,
}

/// The schema name all databases are listed under.
const SCHEMA: &str = "couchdb";

fn relaxed_json(s: &str) -> anyhow::Result<Value> {
    if s.trim().is_empty() {
        return Ok(json!({}));
    }
    json5::from_str(s).map_err(|e| anyhow::anyhow!("Couldn't parse `{s}`: {e}"))
}

/// A Mongo-style sort document (`{a: 1, b: -1}`) as a Mango sort list.
pub(crate) fn mango_sort(v: &Value) -> Value {
    match v {
        Value::Object(o) => Value::Array(o.iter().map(|(k, d)| json!({ k: if d.as_i64() == Some(-1) || d.as_str() == Some("desc") { "desc" } else { "asc" } })).collect()),
        other => other.clone(),
    }
}

/// A Mongo-style projection (`{name: 1, email: 1}`) as a Mango field list.
pub(crate) fn mango_fields(v: &Value) -> Option<Value> {
    match v {
        Value::Object(o) if !o.is_empty() => Some(Value::Array(o.iter().filter(|(_, on)| on.as_i64() != Some(0) && on.as_bool() != Some(false)).map(|(k, _)| Value::String(k.clone())).collect())),
        Value::Array(a) => Some(Value::Array(a.clone())),
        _ => None,
    }
}

/// Build the body of a `_find` request from `find(filter, projection)` and its cursor calls.
pub(crate) fn find_body(calls: &[MongoCall], max: usize) -> anyhow::Result<(Value, usize)> {
    let first = &calls[0];
    let args = split_args(&first.args);
    let mut body = Map::new();
    body.insert("selector".into(), relaxed_json(args.first().copied().unwrap_or(""))?);
    if let Some(f) = args.get(1).map(|a| relaxed_json(a)).transpose()?.as_ref().and_then(mango_fields) {
        body.insert("fields".into(), f);
    }
    let mut limit = if first.name == "findOne" { 1 } else { usize::MAX };
    for c in &calls[1..] {
        match c.name.as_str() {
            "sort" => {
                body.insert("sort".into(), mango_sort(&relaxed_json(&c.args)?));
            }
            "limit" => limit = limit.min(c.args.trim().parse().map_err(|_| anyhow::anyhow!("limit() takes a number"))?),
            "skip" => {
                body.insert("skip".into(), json!(c.args.trim().parse::<u64>().map_err(|_| anyhow::anyhow!("skip() takes a number"))?));
            }
            "projection" => {
                if let Some(f) = mango_fields(&relaxed_json(&c.args)?) {
                    body.insert("fields".into(), f);
                }
            }
            other => anyhow::bail!("`{other}()` isn't supported for CouchDB; use sort, limit, skip or projection"),
        }
    }
    // Mango returns 25 documents unless told otherwise; ask for one more
    // than the row limit so the grid can say the result was cut off.
    let want = limit.min(max.saturating_add(1));
    body.insert("limit".into(), json!(want));
    Ok((Value::Object(body), limit))
}

impl CouchConn {
    pub async fn connect(cfg: &ConnectionConfig, password: &str) -> anyhow::Result<Self> {
        let conn = CouchConn { http: crate::http::client()?, base: crate::http::base_url(cfg), user: cfg.user.clone(), password: password.to_string(), database: cfg.database.trim().to_string() };
        // One request that checks both that this is CouchDB and the sign-in.
        let info: Value = check(conn.req(reqwest::Method::GET, "/_session").send().await?).await?.json().await?;
        if !conn.user.is_empty() && info.pointer("/userCtx/name").map(|n| n.is_null()).unwrap_or(true) {
            anyhow::bail!("CouchDB didn't accept the user name and password");
        }
        Ok(conn)
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let r = self.http.request(method, format!("{}{}", self.base, path)).header("Accept", "application/json");
        if self.user.is_empty() {
            r
        } else {
            r.basic_auth(&self.user, Some(&self.password))
        }
    }

    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        Ok(check(self.req(reqwest::Method::GET, path).send().await?).await?.json().await?)
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: &Value) -> anyhow::Result<Value> {
        Ok(check(self.req(method, path).json(body).send().await?).await?.json().await?)
    }

    async fn databases(&self) -> anyhow::Result<Vec<String>> {
        match self.get("/_all_dbs").await {
            Ok(v) => Ok(v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).filter(|n| !n.starts_with('_')).collect()).unwrap_or_default()),
            // only admins may list databases; fall back to the configured one
            Err(_) if !self.database.is_empty() => Ok(vec![self.database.clone()]),
            Err(e) => Err(e),
        }
    }

    fn db_path(name: &str) -> String {
        format!("/{}", urlencode(name))
    }

    async fn run(&self, db: &str, calls: &[MongoCall], limit: usize, sink: &Sink) -> anyhow::Result<()> {
        let path = Self::db_path(db);
        let c = &calls[0];
        match c.name.as_str() {
            "find" | "findOne" => {
                let (body, _) = find_body(calls, limit)?;
                let res = self.send(reqwest::Method::POST, &format!("{path}/_find"), &body).await?;
                if let Some(w) = res.get("warning").and_then(|w| w.as_str()) {
                    sink(QueryEvent::Message(format!("CouchDB: {w}")));
                }
                let docs = res.get("docs").and_then(|d| d.as_array()).cloned().unwrap_or_default();
                send_objects(&docs, limit, sink);
            }
            "countDocuments" | "estimatedDocumentCount" => {
                let filter = relaxed_json(split_args(&c.args).first().copied().unwrap_or(""))?;
                let n = if filter.as_object().map(|o| o.is_empty()).unwrap_or(false) {
                    self.get(&path).await?.get("doc_count").and_then(|n| n.as_u64()).unwrap_or(0)
                } else {
                    // page through matching ids with bookmarks
                    let mut n = 0u64;
                    let mut bookmark: Option<String> = None;
                    loop {
                        let mut body = json!({ "selector": filter, "fields": ["_id"], "limit": 5000 });
                        if let Some(b) = &bookmark {
                            body["bookmark"] = json!(b);
                        }
                        let res = self.send(reqwest::Method::POST, &format!("{path}/_find"), &body).await?;
                        let got = res.get("docs").and_then(|d| d.as_array()).map(|a| a.len()).unwrap_or(0) as u64;
                        n += got;
                        bookmark = res.get("bookmark").and_then(|b| b.as_str()).map(str::to_string);
                        if got < 5000 || bookmark.is_none() {
                            break;
                        }
                    }
                    n
                };
                send_objects(&[json!({ "count": n })], limit, sink);
            }
            "insertOne" => {
                let doc = relaxed_json(&c.args)?;
                let res = self.send(reqwest::Method::POST, &path, &doc).await?;
                sink(QueryEvent::Message(format!("Inserted {} (rev {})", res.get("id").and_then(|v| v.as_str()).unwrap_or("?"), res.get("rev").and_then(|v| v.as_str()).unwrap_or("?"))));
                sink(QueryEvent::Affected(1));
            }
            "insertMany" => {
                let docs = relaxed_json(&c.args)?;
                if !docs.is_array() {
                    anyhow::bail!("insertMany() takes an array of documents");
                }
                let res = self.send(reqwest::Method::POST, &format!("{path}/_bulk_docs"), &json!({ "docs": docs })).await?;
                let results = res.as_array().cloned().unwrap_or_default();
                let failed: Vec<String> = results.iter().filter_map(|r| r.get("error").map(|e| format!("{}: {}", r.get("id").and_then(|v| v.as_str()).unwrap_or("?"), e))).collect();
                for f in &failed {
                    sink(QueryEvent::Message(format!("Not inserted: {f}")));
                }
                sink(QueryEvent::Affected((results.len() - failed.len()) as u64));
            }
            "getIndexes" => {
                let res = self.get(&format!("{path}/_index")).await?;
                send_objects(res.get("indexes").and_then(|i| i.as_array()).map(|a| a.as_slice()).unwrap_or(&[]), limit, sink);
            }
            "createIndex" => {
                let keys = relaxed_json(split_args(&c.args).first().copied().unwrap_or(""))?;
                let fields = mango_sort(&keys);
                let res = self.send(reqwest::Method::POST, &format!("{path}/_index"), &json!({ "index": { "fields": fields }, "type": "json" })).await?;
                sink(QueryEvent::Message(format!("Index {} {}", res.get("name").and_then(|v| v.as_str()).unwrap_or("?"), res.get("result").and_then(|v| v.as_str()).unwrap_or("created"))));
            }
            "drop" => {
                check(self.req(reqwest::Method::DELETE, &path).send().await?).await?;
                sink(QueryEvent::Message(format!("Dropped database {db}")));
            }
            other => anyhow::bail!(
                "`{other}()` isn't supported for CouchDB. Supported: find, findOne, countDocuments, estimatedDocumentCount, insertOne, insertMany, getIndexes, createIndex, drop. Updates and deletes need the document's _rev; send them with Fauxton or curl for now."
            ),
        }
        Ok(())
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'$' | b'(' | b')' | b'+' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[async_trait]
impl DbConnection for CouchConn {
    fn capabilities(&self) -> Capabilities {
        // Mango has `_explain` (which index a query would use) but no
        // transactions or cancel; queries aren't SQL.
        Capabilities { schemas: false, transactions: false, explain: true, explain_analyze: false, server_cancel: false, sql: false }
    }

    fn canceller(&self) -> Option<Canceller> {
        None
    }

    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        let s = stmt.trim().trim_end_matches(';').trim();
        // raw read-only request: `GET /_up`, `GET /orders/_design_docs`
        if let Some(path) = s.strip_prefix("GET ") {
            let path = path.trim();
            let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
            let v = self.get(&path).await?;
            let rows = match &v {
                Value::Array(a) => a.clone(),
                Value::Object(o) if o.get("rows").map(|r| r.is_array()).unwrap_or(false) => o["rows"].as_array().cloned().unwrap_or_default(),
                other => vec![other.clone()],
            };
            send_objects(&rows, opts.row_limit, sink);
            return Ok(());
        }
        match parse(s).map_err(|e| anyhow::anyhow!("{e} (or `GET /path` for a raw request)"))? {
            MongoStatement::Show(what) => match what.as_str() {
                "dbs" | "databases" | "collections" | "tables" => {
                    let docs: Vec<Value> = self.databases().await?.into_iter().map(|n| json!({ "name": n })).collect();
                    send_objects(&docs, opts.row_limit, sink);
                    Ok(())
                }
                other => anyhow::bail!("Unknown `show {other}`; try `show dbs`"),
            },
            MongoStatement::Command { collection: Some(db), calls } => self.run(&db, &calls, opts.row_limit, sink).await,
            MongoStatement::Command { collection: None, calls } if calls[0].name == "createCollection" => {
                let name = calls[0].args.trim().trim_matches(|c| c == '"' || c == '\'' || c == ' ').to_string();
                if name.is_empty() {
                    anyhow::bail!("createCollection() takes the new database's name");
                }
                check(self.req(reqwest::Method::PUT, &Self::db_path(&name)).send().await?).await?;
                sink(QueryEvent::Message(format!("Created database {name}")));
                Ok(())
            }
            MongoStatement::Command { collection: None, .. } => anyhow::bail!("Name the database: db.<database>.find({{}}), or db.createCollection('name') to create one"),
        }
    }

    async fn introspect(&mut self) -> anyhow::Result<Catalog> {
        let mut tables = Vec::new();
        for (i, name) in self.databases().await?.into_iter().take(500).enumerate() {
            let mut t = TableInfo::new(name.clone(), ObjectKind::Collection);
            t.columns.push(ColumnInfo { name: "_id".into(), data_type: "string".into(), nullable: false, primary_key: true, default: None });
            if i < 200 {
                // sample a few documents to learn field names
                if let Ok(v) = self.get(&format!("{}/_all_docs?include_docs=true&limit=20", Self::db_path(&name))).await {
                    for row in v.get("rows").and_then(|r| r.as_array()).into_iter().flatten() {
                        if row.get("id").and_then(|x| x.as_str()).map(|id| id.starts_with("_design/")).unwrap_or(false) {
                            continue;
                        }
                        for (k, val) in row.get("doc").and_then(|d| d.as_object()).into_iter().flatten() {
                            if !t.columns.iter().any(|c| &c.name == k) {
                                t.columns.push(ColumnInfo { name: k.clone(), data_type: crate::http::json_type(val).into(), nullable: true, primary_key: false, default: None });
                            }
                        }
                    }
                }
                if let Ok(v) = self.get(&format!("{}/_index", Self::db_path(&name))).await {
                    for ix in v.get("indexes").and_then(|r| r.as_array()).into_iter().flatten() {
                        let fields: Vec<String> = ix.pointer("/def/fields").and_then(|f| f.as_array()).into_iter().flatten().filter_map(|f| f.as_object().and_then(|o| o.keys().next().cloned())).collect();
                        let special = ix.get("type").and_then(|t| t.as_str()) == Some("special");
                        t.indexes.push(IndexInfo { name: ix.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(), columns: if special { vec!["_id".into()] } else { fields }, unique: special, primary: special });
                    }
                }
            }
            tables.push(t);
        }
        Ok(Catalog { default_schema: SCHEMA.into(), introspected_at: crate::now_secs(), schemas: vec![SchemaInfo { name: SCHEMA.into(), tables, objects: vec![] }] })
    }

    async fn object_source(&mut self, _schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        anyhow::bail!("CouchDB {}s have no SQL definition", obj.kind.label())
    }

    async fn explain(&mut self, stmt: &str, _analyze: bool) -> anyhow::Result<Plan> {
        match parse(stmt).map_err(|e| anyhow::anyhow!(e))? {
            MongoStatement::Command { collection: Some(db), calls } if matches!(calls[0].name.as_str(), "find" | "findOne") => {
                let (body, _) = find_body(&calls, 10_000)?;
                let v = self.send(reqwest::Method::POST, &format!("{}/_explain", Self::db_path(&db)), &body).await?;
                Ok(Plan { root: None, raw: serde_json::to_string_pretty(&v).unwrap_or_default(), format: "json" })
            }
            _ => anyhow::bail!("CouchDB can explain find() queries only"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calls(s: &str) -> Vec<MongoCall> {
        match parse(s).unwrap() {
            MongoStatement::Command { calls, .. } => calls,
            _ => unreachable!(),
        }
    }

    #[test]
    fn mongo_shell_to_mango() {
        let (b, limit) = find_body(&calls("db.orders.find({status: 'shipped', total: {$gt: 100}}, {total: 1, status: 1}).sort({total: -1}).limit(5).skip(10)"), 1000).unwrap();
        assert_eq!(b["selector"], json!({"status": "shipped", "total": {"$gt": 100}}));
        assert_eq!(b["fields"], json!(["total", "status"]));
        assert_eq!(b["sort"], json!([{"total": "desc"}]));
        assert_eq!(b["limit"], json!(5));
        assert_eq!(b["skip"], json!(10));
        assert_eq!(limit, 5);
    }

    #[test]
    fn default_limit_is_one_past_the_row_limit() {
        let (b, _) = find_body(&calls("db.orders.find({})"), 1000).unwrap();
        assert_eq!(b["limit"], json!(1001));
        let (b, _) = find_body(&calls("db.orders.findOne({_id: 'a'})"), 1000).unwrap();
        assert_eq!(b["limit"], json!(1));
        assert!(find_body(&calls("db.orders.find({}).hint('x')"), 10).is_err());
    }

    #[test]
    fn database_names_are_escaped() {
        assert_eq!(CouchConn::db_path("store/orders"), "/store%2Forders");
        assert_eq!(CouchConn::db_path("orders-2026"), "/orders-2026");
    }
}
