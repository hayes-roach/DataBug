use crate::{now_secs, plan, Batcher, Canceller, DbConnection, ExecOptions, Sink, SourceRef};
use async_trait::async_trait;
use futures_util::TryStreamExt;
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::ClientOptions;
use mongodb::{Client, Database};
use databug_core::{Capabilities, Catalog, ColumnInfo, ColumnMeta, ConnectionConfig, IndexInfo, ObjectKind, Plan, QueryEvent, SchemaInfo, SslMode, TableInfo};
use databug_sql::mongo::{parse, split_args, MongoCall, MongoStatement};

pub struct MongoConn {
    client: Client,
    db_name: String,
}

/// Parse relaxed JSON (unquoted keys, single quotes) into BSON.
fn to_bson(text: &str) -> anyhow::Result<Bson> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(Bson::Document(Document::new()));
    }
    let v: serde_json::Value = json5::from_str(&shell_helpers(t)).map_err(|e| anyhow::anyhow!("Couldn't parse `{t}`: {e}"))?;
    Bson::try_from(v).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Rewrite common shell helpers into extended JSON: ObjectId("..") and ISODate("..").
fn shell_helpers(s: &str) -> String {
    let mut out = s.to_string();
    for (f, key) in [("ObjectId(", "$oid"), ("ISODate(", "$date"), ("new Date(", "$date")] {
        while let Some(p) = out.find(f) {
            let rest = &out[p + f.len()..];
            let Some(close) = rest.find(')') else { break };
            let arg = rest[..close].trim().trim_matches(|c| c == '"' || c == '\'');
            let repl = format!("{{\"{key}\": \"{arg}\"}}");
            out.replace_range(p..p + f.len() + close + 1, &repl);
        }
    }
    out
}

fn to_doc(text: &str) -> anyhow::Result<Document> {
    match to_bson(text)? {
        Bson::Document(d) => Ok(d),
        other => anyhow::bail!("Expected a document, got {other}"),
    }
}

fn to_docs(text: &str) -> anyhow::Result<Vec<Document>> {
    match to_bson(text)? {
        Bson::Array(a) => a.into_iter().map(|b| b.as_document().cloned().ok_or_else(|| anyhow::anyhow!("Expected documents in the array"))).collect(),
        Bson::Document(d) => Ok(vec![d]),
        other => anyhow::bail!("Expected an array of documents, got {other}"),
    }
}

fn bson_type(b: &Bson) -> &'static str {
    match b {
        Bson::Double(_) => "double",
        Bson::String(_) => "string",
        Bson::Document(_) => "object",
        Bson::Array(_) => "array",
        Bson::Boolean(_) => "bool",
        Bson::Null => "null",
        Bson::Int32(_) => "int",
        Bson::Int64(_) => "long",
        Bson::ObjectId(_) => "objectId",
        Bson::DateTime(_) => "date",
        Bson::Decimal128(_) => "decimal",
        Bson::Binary(_) => "binData",
        _ => "mixed",
    }
}

fn bson_text(b: &Bson, out: &mut String) {
    match b {
        Bson::String(s) => out.push_str(s),
        Bson::ObjectId(o) => out.push_str(&o.to_hex()),
        Bson::DateTime(d) => out.push_str(&d.try_to_rfc3339_string().unwrap_or_else(|_| d.to_string())),
        Bson::Int32(i) => out.push_str(&i.to_string()),
        Bson::Int64(i) => out.push_str(&i.to_string()),
        Bson::Double(f) => out.push_str(&f.to_string()),
        Bson::Boolean(v) => out.push_str(if *v { "true" } else { "false" }),
        other => out.push_str(&other.clone().into_relaxed_extjson().to_string()),
    }
}

/// Flatten documents into a grid: top-level fields become columns in order
/// of first appearance; nested values show as JSON.
fn send_docs(docs: &[Document], limit: usize, sink: &Sink) {
    let mut cols: Vec<(String, &'static str)> = Vec::new();
    for d in docs {
        for (k, v) in d {
            if !cols.iter().any(|(c, _)| c == k) {
                cols.push((k.clone(), bson_type(v)));
            }
        }
    }
    if cols.is_empty() {
        cols.push(("result".into(), "object"));
    }
    let metas = cols.iter().map(|(n, t)| ColumnMeta::new(n.clone(), *t)).collect();
    let mut b = Batcher::new(sink, metas, limit);
    for d in docs {
        for (k, _) in &cols {
            match d.get(k) {
                None | Some(Bson::Null) => b.cells().push(None),
                Some(v) => b.cells().push_with(|s| bson_text(v, s)),
            }
        }
        if !b.row_done() {
            break;
        }
    }
}

impl MongoConn {
    pub async fn connect(cfg: &ConnectionConfig, password: &str) -> anyhow::Result<Self> {
        let uri = if !cfg.uri.trim().is_empty() {
            cfg.uri.trim().to_string()
        } else {
            format!("mongodb://{}:{}/", cfg.host, cfg.port)
        };
        let mut opts = ClientOptions::parse(&uri).await?;
        opts.app_name = Some("DataBug".into());
        if !cfg.user.is_empty() {
            let mut cred = mongodb::options::Credential::default();
            cred.username = Some(cfg.user.clone());
            cred.password = Some(password.to_string());
            if let Some(existing) = &opts.credential {
                cred.source = existing.source.clone();
                cred.mechanism = existing.mechanism.clone();
            }
            opts.credential = Some(cred);
        }
        if cfg.ssl == SslMode::Require && opts.tls.is_none() {
            opts.tls = Some(mongodb::options::Tls::Enabled(mongodb::options::TlsOptions::default()));
        }
        opts.server_selection_timeout = Some(std::time::Duration::from_secs(10));
        let default_db = opts.default_database.clone();
        let client = Client::with_options(opts)?;
        let db_name = if !cfg.database.is_empty() { cfg.database.clone() } else { default_db.unwrap_or_else(|| "test".into()) };
        client.database(&db_name).run_command(doc! { "ping": 1 }).await?;
        Ok(MongoConn { client, db_name })
    }

    fn db(&self) -> Database {
        self.client.database(&self.db_name)
    }

    async fn run(&mut self, collection: Option<String>, calls: Vec<MongoCall>, limit: usize, sink: &Sink) -> anyhow::Result<()> {
        let db = self.db();
        let main = &calls[0];
        let args = split_args(&main.args);
        let arg = |i: usize| args.get(i).copied().unwrap_or("");
        let Some(coll_name) = collection else {
            match main.name.as_str() {
                "runCommand" | "adminCommand" => {
                    let target = if main.name == "adminCommand" { self.client.database("admin") } else { db };
                    let r = target.run_command(to_doc(arg(0))?).await?;
                    send_docs(&[r], limit, sink);
                }
                "getCollectionNames" | "listCollections" => {
                    let names = db.list_collection_names().await?;
                    let docs: Vec<Document> = names.into_iter().map(|n| doc! { "name": n }).collect();
                    send_docs(&docs, limit, sink);
                }
                "stats" => {
                    let r = db.run_command(doc! { "dbStats": 1 }).await?;
                    send_docs(&[r], limit, sink);
                }
                "dropDatabase" => {
                    db.drop().await?;
                    sink(QueryEvent::Message(format!("Dropped database {}", self.db_name)));
                }
                other => anyhow::bail!("Unsupported database method `{other}`"),
            }
            return Ok(());
        };
        let coll = db.collection::<Document>(&coll_name);
        match main.name.as_str() {
            "find" | "findOne" => {
                let mut f = coll.find(to_doc(arg(0))?);
                if !arg(1).is_empty() {
                    f = f.projection(to_doc(arg(1))?);
                }
                let mut user_limit: Option<i64> = (main.name == "findOne").then_some(1);
                for c in &calls[1..] {
                    match c.name.as_str() {
                        "sort" => f = f.sort(to_doc(&c.args)?),
                        "limit" => user_limit = c.args.trim().parse::<i64>().ok().filter(|n| *n > 0),
                        "skip" => f = f.skip(c.args.trim().parse::<u64>().unwrap_or(0)),
                        "projection" => f = f.projection(to_doc(&c.args)?),
                        "pretty" | "toArray" => {}
                        other => anyhow::bail!("Unsupported cursor method `{other}`"),
                    }
                }
                // fetch one extra row so the grid knows whether the row limit cut it short
                let fetch = user_limit.map(|u| u.min(limit as i64 + 1)).unwrap_or(limit as i64 + 1);
                let cursor = f.limit(fetch).await?;
                let docs: Vec<Document> = cursor.try_collect().await?;
                send_docs(&docs, limit, sink);
            }
            "aggregate" => {
                let cursor = coll.aggregate(to_docs(arg(0))?).await?;
                let mut docs = Vec::new();
                futures_util::pin_mut!(cursor);
                while let Some(d) = cursor.try_next().await? {
                    docs.push(d);
                    if docs.len() > limit {
                        break;
                    }
                }
                send_docs(&docs, limit, sink);
            }
            "countDocuments" | "count" => {
                let n = coll.count_documents(to_doc(arg(0))?).await?;
                send_docs(&[doc! { "count": n as i64 }], 1, sink);
            }
            "estimatedDocumentCount" => {
                let n = coll.estimated_document_count().await?;
                send_docs(&[doc! { "count": n as i64 }], 1, sink);
            }
            "distinct" => {
                let field = arg(0).trim_matches(|c| c == '"' || c == '\'');
                let values = coll.distinct(field, to_doc(arg(1))?).await?;
                let docs: Vec<Document> = values.into_iter().map(|v| doc! { field: v }).collect();
                send_docs(&docs, limit, sink);
            }
            "insertOne" => {
                let r = coll.insert_one(to_doc(arg(0))?).await?;
                sink(QueryEvent::Message(format!("Inserted _id {}", r.inserted_id)));
                sink(QueryEvent::Affected(1));
            }
            "insertMany" => {
                let r = coll.insert_many(to_docs(arg(0))?).await?;
                sink(QueryEvent::Affected(r.inserted_ids.len() as u64));
            }
            "updateOne" | "updateMany" => {
                let filter = to_doc(arg(0))?;
                let update = to_bson(arg(1))?;
                let r = match (main.name.as_str(), update) {
                    ("updateOne", Bson::Document(u)) => coll.update_one(filter, u).await?,
                    ("updateMany", Bson::Document(u)) => coll.update_many(filter, u).await?,
                    ("updateOne", Bson::Array(p)) => coll.update_one(filter, p.into_iter().filter_map(|b| b.as_document().cloned()).collect::<Vec<_>>()).await?,
                    (_, Bson::Array(p)) => coll.update_many(filter, p.into_iter().filter_map(|b| b.as_document().cloned()).collect::<Vec<_>>()).await?,
                    _ => anyhow::bail!("The update must be a document or a pipeline"),
                };
                sink(QueryEvent::Message(format!("Matched {}, modified {}", r.matched_count, r.modified_count)));
                sink(QueryEvent::Affected(r.modified_count));
            }
            "replaceOne" => {
                let r = coll.replace_one(to_doc(arg(0))?, to_doc(arg(1))?).await?;
                sink(QueryEvent::Affected(r.modified_count));
            }
            "deleteOne" => {
                let r = coll.delete_one(to_doc(arg(0))?).await?;
                sink(QueryEvent::Affected(r.deleted_count));
            }
            "deleteMany" => {
                let r = coll.delete_many(to_doc(arg(0))?).await?;
                sink(QueryEvent::Affected(r.deleted_count));
            }
            "createIndex" => {
                let model = mongodb::IndexModel::builder().keys(to_doc(arg(0))?).build();
                let r = coll.create_index(model).await?;
                sink(QueryEvent::Message(format!("Created index {}", r.index_name)));
            }
            "getIndexes" => {
                let cursor = coll.list_indexes().await?;
                let idx: Vec<mongodb::IndexModel> = cursor.try_collect().await?;
                let docs: Vec<Document> = idx
                    .into_iter()
                    .map(|m| {
                        let name = m.options.as_ref().and_then(|o| o.name.clone()).unwrap_or_default();
                        doc! { "name": name, "key": m.keys }
                    })
                    .collect();
                send_docs(&docs, limit, sink);
            }
            "drop" => {
                coll.drop().await?;
                sink(QueryEvent::Message(format!("Dropped collection {coll_name}")));
            }
            other => anyhow::bail!("Unsupported collection method `{other}`"),
        }
        Ok(())
    }
}

#[async_trait]
impl DbConnection for MongoConn {
    fn capabilities(&self) -> Capabilities {
        Capabilities { schemas: false, transactions: false, explain: true, explain_analyze: true, server_cancel: false, sql: false }
    }

    fn canceller(&self) -> Option<Canceller> {
        None
    }

    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        match parse(stmt).map_err(|e| anyhow::anyhow!(e))? {
            MongoStatement::Show(what) => {
                let docs: Vec<Document> = match what.as_str() {
                    "collections" | "tables" => self.db().list_collection_names().await?.into_iter().map(|n| doc! { "name": n }).collect(),
                    "dbs" | "databases" => self.client.list_database_names().await?.into_iter().map(|n| doc! { "name": n }).collect(),
                    other => anyhow::bail!("Unknown `show {other}`; try `show collections` or `show dbs`"),
                };
                send_docs(&docs, opts.row_limit, sink);
                Ok(())
            }
            MongoStatement::Command { collection, calls } => self.run(collection, calls, opts.row_limit, sink).await,
        }
    }

    async fn list_databases(&mut self) -> anyhow::Result<(Vec<String>, String)> {
        let mut names = self.client.list_database_names().await?;
        names.sort();
        Ok((names, self.db_name.clone()))
    }

    async fn introspect(&mut self) -> anyhow::Result<Catalog> {
        let db = self.db();
        let mut specs: Vec<mongodb::results::CollectionSpecification> = db.list_collections().await?.try_collect().await?;
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        let mut tables = Vec::new();
        for (i, spec) in specs.into_iter().take(500).enumerate() {
            let name = spec.name.clone();
            let is_view = matches!(spec.collection_type, mongodb::results::CollectionType::View);
            // sample a few documents to learn field names
            let coll = db.collection::<Document>(&name);
            let docs: Vec<Document> = match coll.find(doc! {}).limit(20).await {
                Ok(c) => c.try_collect().await.unwrap_or_default(),
                Err(_) => vec![],
            };
            let mut t = TableInfo::new(name, if is_view { ObjectKind::View } else { ObjectKind::Collection });
            for d in &docs {
                for (k, v) in d {
                    if !t.columns.iter().any(|c| &c.name == k) {
                        t.columns.push(ColumnInfo { name: k.clone(), data_type: bson_type(v).into(), nullable: true, primary_key: k == "_id", default: None });
                    }
                }
            }
            if !is_view && i < 200 {
                if let Ok(c) = coll.list_indexes().await {
                    let idx: Vec<mongodb::IndexModel> = c.try_collect().await.unwrap_or_default();
                    for m in idx {
                        let name = m.options.as_ref().and_then(|o| o.name.clone()).unwrap_or_default();
                        let unique = m.options.as_ref().and_then(|o| o.unique).unwrap_or(false);
                        t.indexes.push(IndexInfo { primary: name == "_id_", unique: unique || name == "_id_", columns: m.keys.keys().cloned().collect(), name });
                    }
                }
            }
            tables.push(t);
        }
        Ok(Catalog { default_schema: self.db_name.clone(), introspected_at: now_secs(), schemas: vec![SchemaInfo { name: self.db_name.clone(), tables, objects: vec![] }] })
    }

    async fn object_source(&mut self, _schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        if obj.kind != ObjectKind::View {
            anyhow::bail!("Only views have a definition in MongoDB");
        }
        let specs: Vec<mongodb::results::CollectionSpecification> = self.db().list_collections().filter(doc! { "name": &obj.name }).await?.try_collect().await?;
        let spec = specs.into_iter().next().ok_or_else(|| anyhow::anyhow!("View not found"))?;
        let on = spec.options.view_on.unwrap_or_default();
        let pipeline: Vec<serde_json::Value> = spec.options.pipeline.unwrap_or_default().into_iter().map(|d| Bson::Document(d).into_relaxed_extjson()).collect();
        Ok(format!(
            "// View {} is defined over {on}. Run its pipeline directly:\ndb.{on}.aggregate({})\n",
            obj.name,
            serde_json::to_string_pretty(&pipeline).unwrap_or_default()
        ))
    }

    async fn explain(&mut self, stmt: &str, analyze: bool) -> anyhow::Result<Plan> {
        let MongoStatement::Command { collection: Some(coll), calls } = parse(stmt).map_err(|e| anyhow::anyhow!(e))? else {
            anyhow::bail!("Explain works on find, aggregate and count statements");
        };
        let main = &calls[0];
        let args = split_args(&main.args);
        let arg = |i: usize| args.get(i).copied().unwrap_or("");
        let mut cmd = match main.name.as_str() {
            "find" | "findOne" => {
                let mut c = doc! { "find": &coll, "filter": to_doc(arg(0))? };
                for k in &calls[1..] {
                    match k.name.as_str() {
                        "sort" => {
                            c.insert("sort", to_doc(&k.args)?);
                        }
                        "limit" => {
                            c.insert("limit", k.args.trim().parse::<i64>().unwrap_or(0));
                        }
                        "skip" => {
                            c.insert("skip", k.args.trim().parse::<i64>().unwrap_or(0));
                        }
                        _ => {}
                    }
                }
                c
            }
            "aggregate" => doc! { "aggregate": &coll, "pipeline": to_docs(arg(0))?, "cursor": {} },
            "countDocuments" | "count" => doc! { "count": &coll, "query": to_doc(arg(0))? },
            other => anyhow::bail!("Explain isn't available for `{other}`"),
        };
        cmd = doc! { "explain": cmd, "verbosity": if analyze { "executionStats" } else { "queryPlanner" } };
        let r = self.db().run_command(cmd).await?;
        let json = Bson::Document(r).into_relaxed_extjson();
        // the interesting part is the winning plan
        let focus = json.pointer("/queryPlanner/winningPlan").cloned();
        let raw = serde_json::to_string_pretty(&json).unwrap_or_default();
        let root = match focus {
            Some(w) => plan::from_generic_json(&w.to_string()),
            None => plan::from_generic_json(&raw),
        };
        Ok(Plan { root, raw, format: "json" })
    }
}
