//! InfluxDB over its HTTP API. The console takes InfluxQL (`/query`, which
//! InfluxDB 1.x, 2.x and 3.x all serve), Flux on 2.x (statements that start
//! with `from(`, `import` or use `|>`), and `INSERT <line protocol>` to
//! write points. The bucket (or 1.x database) is the connection's database,
//! the API token goes in the password field.

use crate::http::check;
use crate::{Batcher, Canceller, DbConnection, ExecOptions, Sink, SourceRef};
use async_trait::async_trait;
use databug_core::{Capabilities, Catalog, ColumnInfo, ColumnMeta, ConnectionConfig, ObjectKind, Plan, QueryEvent, SchemaInfo, TableInfo};
use serde_json::Value;

pub struct InfluxConn {
    http: reqwest::Client,
    base: String,
    user: String,
    secret: String,
    org: String,
    bucket: String,
    /// Major version from the `X-Influxdb-Version` header (1, 2 or 3).
    major: u32,
}

/// Whether a statement is Flux rather than InfluxQL.
pub(crate) fn is_flux(stmt: &str) -> bool {
    let s = stmt.trim_start();
    s.starts_with("from(") || s.starts_with("from (") || s.starts_with("import ") || s.starts_with("buckets()") || s.contains("|>")
}

/// `INSERT cpu,host=a usage=1` → the line protocol after `INSERT`.
pub(crate) fn insert_lines(stmt: &str) -> Option<String> {
    let s = stmt.trim();
    let rest = s.get(..7).filter(|p| p.eq_ignore_ascii_case("insert ")).map(|_| &s[7..])?;
    Some(rest.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n"))
}

/// Parse `v2.7.10`, `2.7.10`, `1.8.10` or `3.0.1-…`.
pub(crate) fn major_version(header: &str) -> u32 {
    header.trim().trim_start_matches(['v', 'V']).split('.').next().and_then(|m| m.parse().ok()).unwrap_or(2)
}

/// Split one CSV line (RFC 4180 quoting).
pub(crate) fn csv_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Flux's annotated CSV → tables of (header, rows). A blank line or a new
/// header row starts a new table.
pub(crate) fn flux_tables(csv: &str) -> Vec<(Vec<String>, Vec<Vec<String>>)> {
    let mut out: Vec<(Vec<String>, Vec<Vec<String>>)> = Vec::new();
    let mut header: Option<Vec<String>> = None;
    for line in csv.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            header = None;
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let f = csv_fields(line);
        match &header {
            None => {
                // columns 0 and 1 are the annotation column and `result`
                header = Some(f.clone());
                if out.last().map(|t| t.0 != f).unwrap_or(true) {
                    out.push((f, Vec::new()));
                }
            }
            Some(_) => out.last_mut().unwrap().1.push(f),
        }
    }
    out
}

impl InfluxConn {
    pub async fn connect(cfg: &ConnectionConfig, password: &str) -> anyhow::Result<Self> {
        let http = crate::http::client()?;
        let base = crate::http::base_url(cfg);
        // /ping needs no sign-in and reports the version
        let ping = http.get(format!("{base}/ping")).send().await?;
        let major = ping.headers().get("X-Influxdb-Version").and_then(|v| v.to_str().ok()).map(major_version).unwrap_or(2);
        let conn = InfluxConn { http, base, user: cfg.user.clone(), secret: password.to_string(), org: cfg.org.trim().to_string(), bucket: cfg.database.trim().to_string(), major };
        // One signed-in request to check the token or password.
        conn.influxql("SHOW DATABASES", false).await?;
        Ok(conn)
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let r = self.http.request(method, format!("{}{}", self.base, path));
        if !self.user.is_empty() {
            r.basic_auth(&self.user, Some(&self.secret))
        } else if !self.secret.is_empty() {
            r.header("Authorization", format!("Token {}", self.secret))
        } else {
            r
        }
    }

    async fn influxql(&self, q: &str, with_db: bool) -> anyhow::Result<Value> {
        let mut params: Vec<(&str, &str)> = vec![("q", q)];
        if with_db && !self.bucket.is_empty() {
            params.push(("db", &self.bucket));
        }
        let resp = check(self.req(reqwest::Method::POST, "/query").header("Accept", "application/json").form(&params).send().await?).await?;
        let v: Value = resp.json().await?;
        for r in v.get("results").and_then(|r| r.as_array()).into_iter().flatten() {
            if let Some(e) = r.get("error").and_then(|e| e.as_str()) {
                anyhow::bail!("{e}");
            }
        }
        if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
            anyhow::bail!("{e}");
        }
        Ok(v)
    }

    /// All series of all results, each as (name, columns, rows).
    fn series(v: &Value) -> Vec<(String, Vec<String>, Vec<Vec<Value>>)> {
        let mut out = Vec::new();
        for r in v.get("results").and_then(|r| r.as_array()).into_iter().flatten() {
            for s in r.get("series").and_then(|s| s.as_array()).into_iter().flatten() {
                let mut cols: Vec<String> = s.get("columns").and_then(|c| c.as_array()).into_iter().flatten().filter_map(|c| c.as_str().map(str::to_string)).collect();
                let tags: Vec<(String, Value)> = s.get("tags").and_then(|t| t.as_object()).map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
                let mut rows: Vec<Vec<Value>> = s.get("values").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|r| r.as_array().cloned()).collect();
                if !tags.is_empty() {
                    // GROUP BY tags: put them in front of every row
                    cols = tags.iter().map(|(k, _)| k.clone()).chain(cols).collect();
                    for r in &mut rows {
                        let mut full: Vec<Value> = tags.iter().map(|(_, v)| v.clone()).collect();
                        full.append(r);
                        *r = full;
                    }
                }
                out.push((s.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(), cols, rows));
            }
        }
        out
    }

    async fn flux(&self, q: &str, limit: usize, sink: &Sink) -> anyhow::Result<()> {
        if self.major < 2 {
            anyhow::bail!("Flux needs InfluxDB 2.x; use InfluxQL here");
        }
        if self.org.is_empty() {
            anyhow::bail!("Flux queries need the organization; set it in the connection settings");
        }
        let body = serde_json::json!({ "query": q, "type": "flux", "dialect": { "header": true, "annotations": [], "delimiter": "," } });
        let resp = check(self.req(reqwest::Method::POST, "/api/v2/query").query(&[("org", &self.org)]).header("Accept", "application/csv").json(&body).send().await?).await?;
        let text = resp.text().await?;
        let tables = flux_tables(&text);
        if tables.is_empty() {
            sink(QueryEvent::Message("No rows".into()));
        }
        for (header, rows) in tables {
            // drop the empty annotation column and `result`
            let skip = header.iter().take(2).filter(|h| h.is_empty() || *h == "result").count();
            let cols = header[skip..].iter().map(|h| ColumnMeta::new(h.clone(), "")).collect();
            let mut b = Batcher::new(sink, cols, limit);
            for r in rows {
                for i in skip..header.len() {
                    let v = r.get(i).map(String::as_str).unwrap_or("");
                    b.cells().push(if v.is_empty() { None } else { Some(v) });
                }
                if !b.row_done() {
                    break;
                }
            }
        }
        Ok(())
    }

    async fn write(&self, lines: &str, sink: &Sink) -> anyhow::Result<()> {
        if self.bucket.is_empty() {
            anyhow::bail!("Set the bucket (database) in the connection settings to write points");
        }
        let req = if self.major >= 2 && !self.org.is_empty() {
            self.req(reqwest::Method::POST, "/api/v2/write").query(&[("org", self.org.as_str()), ("bucket", self.bucket.as_str())])
        } else {
            self.req(reqwest::Method::POST, "/write").query(&[("db", self.bucket.as_str())])
        };
        check(req.body(lines.to_string()).send().await?).await?;
        sink(QueryEvent::Affected(lines.lines().count() as u64));
        Ok(())
    }
}

#[async_trait]
impl DbConnection for InfluxConn {
    fn capabilities(&self) -> Capabilities {
        Capabilities { schemas: false, transactions: false, explain: false, explain_analyze: false, server_cancel: false, sql: true }
    }

    fn canceller(&self) -> Option<Canceller> {
        None
    }

    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        if let Some(lines) = insert_lines(stmt) {
            return self.write(&lines, sink).await;
        }
        if is_flux(stmt) {
            return self.flux(stmt, opts.row_limit, sink).await;
        }
        let v = self.influxql(stmt, true).await?;
        let series = Self::series(&v);
        if series.is_empty() {
            sink(QueryEvent::Message("No rows".into()));
        }
        for (name, cols, rows) in series {
            if !name.is_empty() {
                sink(QueryEvent::Message(format!("Series {name}")));
            }
            let types: Vec<&str> = (0..cols.len()).map(|i| rows.iter().find_map(|r| r.get(i).filter(|v| !v.is_null())).map(crate::http::json_type).unwrap_or("")).collect();
            let metas = cols.iter().zip(types).map(|(c, t)| ColumnMeta::new(c.clone(), if c == "time" { "timestamp" } else { t })).collect();
            let mut b = Batcher::new(sink, metas, opts.row_limit);
            for r in rows {
                for i in 0..cols.len() {
                    b.cells().push(r.get(i).and_then(crate::http::json_text).as_deref());
                }
                if !b.row_done() {
                    break;
                }
            }
        }
        Ok(())
    }

    async fn introspect(&mut self) -> anyhow::Result<Catalog> {
        let schema = if self.bucket.is_empty() { "influxdb".to_string() } else { self.bucket.clone() };
        let mut tables: Vec<TableInfo> = Vec::new();
        if !self.bucket.is_empty() {
            for (_, _, rows) in Self::series(&self.influxql("SHOW MEASUREMENTS LIMIT 1000", true).await?) {
                for r in rows {
                    if let Some(n) = r.first().and_then(|v| v.as_str()) {
                        let mut t = TableInfo::new(n, ObjectKind::Table);
                        t.columns.push(ColumnInfo { name: "time".into(), data_type: "timestamp".into(), nullable: false, primary_key: true, default: None });
                        tables.push(t);
                    }
                }
            }
            // one call each lists tag and field keys for every measurement
            if let Ok(v) = self.influxql("SHOW TAG KEYS", true).await {
                for (m, _, rows) in Self::series(&v) {
                    if let Some(t) = tables.iter_mut().find(|t| t.name == m) {
                        for r in rows {
                            if let Some(k) = r.first().and_then(|v| v.as_str()) {
                                t.columns.push(ColumnInfo { name: k.into(), data_type: "tag".into(), nullable: true, primary_key: false, default: None });
                            }
                        }
                    }
                }
            }
            if let Ok(v) = self.influxql("SHOW FIELD KEYS", true).await {
                for (m, _, rows) in Self::series(&v) {
                    if let Some(t) = tables.iter_mut().find(|t| t.name == m) {
                        for r in rows {
                            if let Some(k) = r.first().and_then(|v| v.as_str()) {
                                if !t.columns.iter().any(|c| c.name == k) {
                                    t.columns.push(ColumnInfo { name: k.into(), data_type: r.get(1).and_then(|v| v.as_str()).unwrap_or("field").into(), nullable: true, primary_key: false, default: None });
                                }
                            }
                        }
                    }
                }
            }
        }
        tables.sort_by_cached_key(|t| t.name.to_lowercase());
        Ok(Catalog { default_schema: schema.clone(), introspected_at: crate::now_secs(), schemas: vec![SchemaInfo { name: schema, tables, objects: vec![] }] })
    }

    async fn object_source(&mut self, _schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        anyhow::bail!("InfluxDB has no {} definitions", obj.kind.label())
    }

    async fn explain(&mut self, _stmt: &str, _analyze: bool) -> anyhow::Result<Plan> {
        anyhow::bail!("EXPLAIN isn't supported for InfluxDB in DataBug yet")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_flux_from_influxql() {
        assert!(is_flux("from(bucket: \"store\") |> range(start: -1h)"));
        assert!(is_flux("import \"array\"\narray.from(rows: [])"));
        assert!(!is_flux("SELECT mean(\"usage\") FROM \"cpu\" WHERE time > now() - 1h GROUP BY time(5m)"));
    }

    #[test]
    fn insert_is_line_protocol() {
        assert_eq!(insert_lines("INSERT orders,region=east total=42.5"), Some("orders,region=east total=42.5".into()));
        assert_eq!(insert_lines("insert a v=1\n  b v=2\n"), Some("a v=1\nb v=2".into()));
        assert_eq!(insert_lines("SELECT 1"), None);
    }

    #[test]
    fn versions() {
        assert_eq!(major_version("v2.7.10"), 2);
        assert_eq!(major_version("1.8.10"), 1);
        assert_eq!(major_version("3.0.1-abc"), 3);
    }

    #[test]
    fn parses_annotated_csv() {
        let csv = ",result,table,_time,_value,region\r\n,_result,0,2026-01-01T00:00:00Z,1,\"east, main\"\r\n,_result,0,2026-01-01T00:01:00Z,2,east\r\n\r\n,result,table,_time,count\r\n,_result,1,2026-01-01T00:00:00Z,7\r\n";
        let t = flux_tables(csv);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].0, vec!["", "result", "table", "_time", "_value", "region"]);
        assert_eq!(t[0].1.len(), 2);
        assert_eq!(t[0].1[0][5], "east, main");
        assert_eq!(t[1].1[0][4], "7");
    }

    #[test]
    fn group_by_tags_become_columns() {
        let v = serde_json::json!({"results": [{"statement_id": 0, "series": [{"name": "cpu", "tags": {"host": "a"}, "columns": ["time", "mean"], "values": [["2026-01-01T00:00:00Z", 1.5]]}]}]});
        let s = InfluxConn::series(&v);
        assert_eq!(s[0].1, vec!["host", "time", "mean"]);
        assert_eq!(s[0].2[0][0], serde_json::json!("a"));
    }
}
