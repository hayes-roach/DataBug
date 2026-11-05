//! Apache Cassandra (and ScyllaDB) over the CQL native protocol, through the
//! `scylla` driver. Keyspaces are shown as schemas.

use crate::{Batcher, Canceller, DbConnection, ExecOptions, Sink, SourceRef};
use async_trait::async_trait;
use databug_core::{Capabilities, Catalog, ColumnInfo, ColumnMeta, ConnectionConfig, DbObject, Dialect, IndexInfo, ObjectKind, Plan, QueryEvent, SchemaInfo, SslMode, TableInfo};
use futures_util::StreamExt;
use scylla::client::session::Session;
use scylla::client::session_builder::SessionBuilder;
use scylla::cluster::metadata::{ColumnType, NativeType};
use scylla::errors::TranslationError;
use scylla::policies::address_translator::{AddressTranslator, UntranslatedPeer};
use scylla::value::{CqlValue, Row};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

pub struct CassConn {
    session: Session,
    keyspace: String,
}

/// Sends every connection to the address the user entered. Nodes advertise
/// their own (often internal) addresses, which aren't reachable through
/// Docker, NAT, port-forwards or load balancers; any node can coordinate a
/// request, so one entry point works for a client like DataBug.
struct ThroughEntryPoint(SocketAddr);

#[async_trait]
impl AddressTranslator for ThroughEntryPoint {
    async fn translate_address(&self, _peer: &UntranslatedPeer) -> Result<SocketAddr, TranslationError> {
        Ok(self.0)
    }
}

/// Keyspaces Cassandra and ScyllaDB keep for themselves.
pub(crate) fn is_system_keyspace(k: &str) -> bool {
    k.starts_with("system") || k == "dse_system" || k == "dse_security" || k == "dse_auth" || k == "solr_admin"
}

impl CassConn {
    pub async fn connect(cfg: &ConnectionConfig, password: &str) -> anyhow::Result<Self> {
        // prefer IPv4 when a name has both (`localhost`, Docker port mappings)
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((cfg.host.trim(), cfg.port)).await?.collect();
        let entry = addrs.iter().find(|a| a.is_ipv4()).or(addrs.first()).copied().ok_or_else(|| anyhow::anyhow!("Couldn't resolve {}", cfg.host))?;
        let mut b = SessionBuilder::new().known_node_addr(entry).address_translator(Arc::new(ThroughEntryPoint(entry))).connection_timeout(Duration::from_secs(10));
        if !cfg.user.is_empty() {
            b = b.user(cfg.user.clone(), password.to_string());
        }
        // A node listens for either TLS or plain text, and trying one after the
        // other would sign in twice, so only `Require` turns TLS on. Like the
        // Postgres driver it encrypts but accepts self-signed certificates.
        if cfg.ssl == SslMode::Require {
            b = b.tls_context(Some(Arc::new(crate::tls::no_verify_config())));
        }
        let keyspace = cfg.database.trim().to_string();
        if !keyspace.is_empty() {
            b = b.use_keyspace(keyspace.clone(), keyspace.chars().any(|c| c.is_ascii_uppercase()));
        }
        let session = b.build().await?;
        Ok(CassConn { session, keyspace })
    }
}

pub(crate) fn type_name(t: &ColumnType) -> String {
    match t {
        ColumnType::Native(n) => match n {
            NativeType::BigInt => "bigint".into(),
            other => format!("{other:?}").to_ascii_lowercase(),
        },
        ColumnType::Collection { .. } => "collection".into(),
        ColumnType::Vector { dimensions, .. } => format!("vector[{dimensions}]"),
        ColumnType::UserDefinedType { .. } => "udt".into(),
        ColumnType::Tuple(_) => "tuple".into(),
        _ => "unknown".into(),
    }
}

/// A two's-complement big-endian integer as decimal text.
fn varint_text(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "0".into();
    }
    if bytes.len() <= 16 {
        let neg = bytes[0] & 0x80 != 0;
        let mut v: i128 = if neg { -1 } else { 0 };
        for b in bytes {
            v = (v << 8) | *b as i128;
        }
        return v.to_string();
    }
    format!("0x{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// Unscaled integer text plus a scale → `123.45`.
pub(crate) fn decimal_text(bytes: &[u8], scale: i32) -> String {
    let digits = varint_text(bytes);
    if scale <= 0 || digits.starts_with("0x") {
        return if scale < 0 && !digits.starts_with("0x") { format!("{digits}{}", "0".repeat((-scale) as usize)) } else { digits };
    }
    let (sign, digits) = match digits.strip_prefix('-') {
        Some(d) => ("-", d.to_string()),
        None => ("", digits),
    };
    let scale = scale as usize;
    let digits = if digits.len() <= scale { format!("{}{digits}", "0".repeat(scale - digits.len() + 1)) } else { digits };
    let (int, frac) = digits.split_at(digits.len() - scale);
    format!("{sign}{int}.{frac}")
}

/// Grid text for a CQL value: scalars without CQL quoting, compound values
/// in CQL literal form.
pub(crate) fn value_text(v: &CqlValue) -> String {
    match v {
        CqlValue::Ascii(s) | CqlValue::Text(s) => s.clone(),
        CqlValue::Decimal(d) => {
            let (bytes, scale) = d.as_signed_be_bytes_slice_and_exponent();
            decimal_text(bytes, scale)
        }
        CqlValue::Varint(v) => varint_text(v.as_signed_bytes_be_slice()),
        CqlValue::Date(_) | CqlValue::Time(_) | CqlValue::Timestamp(_) | CqlValue::Inet(_) => v.to_string().trim_matches('\'').to_string(),
        other => other.to_string(),
    }
}

#[async_trait]
impl DbConnection for CassConn {
    fn capabilities(&self) -> Capabilities {
        // CQL has no multi-statement transactions, no EXPLAIN and no cancel request
        Capabilities { schemas: true, transactions: false, explain: false, explain_analyze: false, server_cancel: false, sql: true }
    }

    fn canceller(&self) -> Option<Canceller> {
        None
    }

    async fn execute(&mut self, stmt: &str, opts: &ExecOptions, sink: &Sink) -> anyhow::Result<()> {
        let stmt = stmt.trim().trim_end_matches(';').trim();
        // `USE ks` changes the session's keyspace
        if let Some(ks) = stmt.strip_prefix("USE ").or_else(|| stmt.strip_prefix("use ")) {
            let ks = ks.trim();
            let quoted = ks.starts_with('"');
            let name = ks.trim_matches('"').to_string();
            self.session.use_keyspace(name.clone(), quoted).await?;
            self.keyspace = name;
            sink(QueryEvent::Message(format!("Using keyspace {}", self.keyspace)));
            return Ok(());
        }
        let pager = self.session.query_iter(stmt, &[]).await?;
        let specs: Vec<ColumnMeta> = pager.column_specs().iter().map(|c| ColumnMeta::new(c.name(), type_name(c.typ()))).collect();
        if specs.is_empty() {
            // writes and schema changes return no rows; CQL doesn't report a count
            sink(QueryEvent::Message("Done".into()));
            return Ok(());
        }
        let width = specs.len();
        let mut b = Batcher::new(sink, specs, opts.row_limit);
        let mut rows = pager.rows_stream::<Row>()?;
        while let Some(row) = rows.next().await {
            let row = row?;
            for i in 0..width {
                match row.columns.get(i).and_then(|c| c.as_ref()) {
                    None => b.cells().push(None),
                    Some(v) => b.cells().push(Some(value_text(v).as_str())),
                }
            }
            if !b.row_done() {
                break;
            }
        }
        Ok(())
    }

    async fn list_databases(&mut self) -> anyhow::Result<(Vec<String>, String)> {
        Ok((Vec::new(), String::new()))
    }

    async fn introspect(&mut self) -> anyhow::Result<Catalog> {
        let q = |s: &str| s.to_string();
        let mut schemas: Vec<SchemaInfo> = Vec::new();
        let rows = self.text_rows(&q("SELECT keyspace_name FROM system_schema.keyspaces")).await?;
        for r in rows {
            if !is_system_keyspace(&r[0]) {
                schemas.push(SchemaInfo { name: r[0].clone(), ..Default::default() });
            }
        }
        schemas.sort_by(|a, b| a.name.cmp(&b.name));
        let mut cols = self.text_rows("SELECT keyspace_name, table_name, column_name, type, kind, position FROM system_schema.columns").await?;
        // partition key, then clustering columns, then the rest; each by position
        let rank = |k: &str| match k {
            "partition_key" => 0,
            "clustering" => 1,
            "static" => 2,
            _ => 3,
        };
        cols.sort_by(|a, b| (a[0].as_str(), a[1].as_str(), rank(&a[4]), a[5].parse::<i32>().unwrap_or(0), a[2].as_str()).cmp(&(b[0].as_str(), b[1].as_str(), rank(&b[4]), b[5].parse::<i32>().unwrap_or(0), b[2].as_str())));
        let views: Vec<(String, String)> = self.text_rows("SELECT keyspace_name, view_name FROM system_schema.views").await.unwrap_or_default().into_iter().map(|r| (r[0].clone(), r[1].clone())).collect();
        for r in cols {
            let Some(s) = schemas.iter_mut().find(|s| s.name == r[0]) else { continue };
            if s.tables.last().map(|t| t.name != r[1]).unwrap_or(true) {
                let kind = if views.iter().any(|(k, v)| *k == r[0] && *v == r[1]) { ObjectKind::MaterializedView } else { ObjectKind::Table };
                s.tables.push(TableInfo::new(r[1].clone(), kind));
            }
            let t = s.tables.last_mut().unwrap();
            let key = rank(&r[4]) <= 1;
            t.columns.push(ColumnInfo { name: r[2].clone(), data_type: r[3].clone(), nullable: !key, primary_key: key, default: None });
        }
        for s in &mut schemas {
            for t in &mut s.tables {
                let pk: Vec<String> = t.columns.iter().filter(|c| c.primary_key).map(|c| c.name.clone()).collect();
                if !pk.is_empty() && t.kind == ObjectKind::Table {
                    t.indexes.push(IndexInfo { name: format!("{}_pkey", t.name), columns: pk, unique: true, primary: true });
                }
            }
        }
        // secondary indexes: options['target'] names the column
        if let Ok(ix) = self.text_rows("SELECT keyspace_name, table_name, index_name, options FROM system_schema.indexes").await {
            for r in ix {
                if let Some(t) = schemas.iter_mut().find(|s| s.name == r[0]).and_then(|s| s.tables.iter_mut().find(|t| t.name == r[1])) {
                    let target = cql_map_get(&r[3], "target").unwrap_or_default();
                    t.indexes.push(IndexInfo { name: r[2].clone(), columns: if target.is_empty() { vec![] } else { vec![target] }, unique: false, primary: false });
                }
            }
        }
        for (sql, kind) in [("SELECT keyspace_name, function_name, argument_types FROM system_schema.functions", ObjectKind::Function), ("SELECT keyspace_name, aggregate_name, argument_types FROM system_schema.aggregates", ObjectKind::Aggregate)] {
            if let Ok(rows) = self.text_rows(sql).await {
                for r in rows {
                    if let Some(s) = schemas.iter_mut().find(|s| s.name == r[0]) {
                        s.objects.push(DbObject { name: r[1].clone(), kind, detail: r[2].trim_start_matches('[').trim_end_matches(']').replace('\'', ""), ident: String::new(), table: None });
                    }
                }
            }
        }
        let default = if self.keyspace.is_empty() { schemas.first().map(|s| s.name.clone()).unwrap_or_default() } else { self.keyspace.clone() };
        // the default keyspace first, like the other drivers
        schemas.sort_by(|a, b| (a.name != default).cmp(&(b.name != default)).then(a.name.cmp(&b.name)));
        Ok(Catalog { default_schema: default, introspected_at: crate::now_secs(), schemas })
    }

    async fn object_source(&mut self, schema: &str, obj: &SourceRef) -> anyhow::Result<String> {
        let d = Dialect::Postgres;
        let lit = |s: &str| d.quote_literal(s);
        let q = format!("{}.{}", d.quote_ident(schema), d.quote_ident(&obj.name));
        match obj.kind {
            ObjectKind::MaterializedView => {
                let r = self.text_rows(&format!("SELECT base_table_name, where_clause, include_all_columns FROM system_schema.views WHERE keyspace_name = {} AND view_name = {}", lit(schema), lit(&obj.name))).await?;
                let r = r.first().ok_or_else(|| anyhow::anyhow!("View {q} not found"))?;
                let cols = self.text_rows(&format!("SELECT column_name, kind, position FROM system_schema.columns WHERE keyspace_name = {} AND table_name = {}", lit(schema), lit(&obj.name))).await?;
                let mut pk: Vec<(i32, i32, String)> = cols.iter().filter(|c| c[1] == "partition_key" || c[1] == "clustering").map(|c| (if c[1] == "partition_key" { 0 } else { 1 }, c[2].parse().unwrap_or(0), d.quote_ident(&c[0]))).collect();
                pk.sort();
                let parts: Vec<String> = pk.iter().filter(|p| p.0 == 0).map(|p| p.2.clone()).collect();
                let clus: Vec<String> = pk.iter().filter(|p| p.0 == 1).map(|p| p.2.clone()).collect();
                let key = if parts.len() > 1 { format!("({})", parts.join(", ")) } else { parts.join("") };
                let key = if clus.is_empty() { key } else { format!("{key}, {}", clus.join(", ")) };
                let all: Vec<String> = cols.iter().map(|c| d.quote_ident(&c[0])).collect();
                let select = if r[2] == "true" { "*".to_string() } else { all.join(", ") };
                Ok(format!(
                    "-- Materialized views can't be altered in place, so this drops and recreates it.\nDROP MATERIALIZED VIEW IF EXISTS {q};\n\nCREATE MATERIALIZED VIEW {q} AS\n    SELECT {select} FROM {}.{}\n    WHERE {}\n    PRIMARY KEY ({key});\n",
                    d.quote_ident(schema),
                    d.quote_ident(&r[0]),
                    r[1]
                ))
            }
            ObjectKind::Function => {
                let r = self.text_rows(&format!("SELECT argument_names, argument_types, return_type, language, body, called_on_null_input FROM system_schema.functions WHERE keyspace_name = {} AND function_name = {}", lit(schema), lit(&obj.name))).await?;
                let r = r.first().ok_or_else(|| anyhow::anyhow!("Function {q} not found"))?;
                let names = list_items(&r[0]);
                let types = list_items(&r[1]);
                let args: Vec<String> = names.iter().zip(types.iter()).map(|(n, t)| format!("{n} {t}")).collect();
                Ok(format!(
                    "CREATE OR REPLACE FUNCTION {q}({})\n    {} ON NULL INPUT\n    RETURNS {}\n    LANGUAGE {}\n    AS $${}$$;\n",
                    args.join(", "),
                    if r[5] == "true" { "CALLED" } else { "RETURNS NULL" },
                    r[2],
                    r[3],
                    r[4]
                ))
            }
            k => anyhow::bail!("Viewing the source of a {} isn't supported for Cassandra", k.label()),
        }
    }

    async fn explain(&mut self, _stmt: &str, _analyze: bool) -> anyhow::Result<Plan> {
        anyhow::bail!("Cassandra has no EXPLAIN; use TRACING in cqlsh to see how a query runs")
    }
}

/// A value from a CQL map printed as text: `{'target':'status'}` → `status`.
pub(crate) fn cql_map_get(s: &str, key: &str) -> Option<String> {
    let rest = s.split(&format!("'{key}':")).nth(1)?.trim_start();
    Some(rest.strip_prefix('\'')?.split('\'').next()?.to_string())
}

/// `['a', 'b']` (a CQL list printed as text) → `a`, `b`.
fn list_items(s: &str) -> Vec<String> {
    s.trim_start_matches('[').trim_end_matches(']').split(',').map(|x| x.trim().trim_matches('\'').to_string()).filter(|x| !x.is_empty()).collect()
}

impl CassConn {
    /// Every row of a query with each column as text (system tables only).
    async fn text_rows(&self, sql: &str) -> anyhow::Result<Vec<Vec<String>>> {
        let pager = self.session.query_iter(sql, &[]).await?;
        let mut rows = pager.rows_stream::<Row>()?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await {
            let row = row?;
            out.push(row.columns.iter().map(|c| c.as_ref().map(value_text).unwrap_or_default()).collect());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_and_varints() {
        // 12345 with scale 2
        assert_eq!(decimal_text(&[0x30, 0x39], 2), "123.45");
        assert_eq!(decimal_text(&[0xCF, 0xC7], 2), "-123.45");
        assert_eq!(decimal_text(&[0x05], 3), "0.005");
        assert_eq!(decimal_text(&[0x07], 0), "7");
        assert_eq!(decimal_text(&[0x07], -2), "700");
        assert_eq!(varint_text(&[0xFF]), "-1");
        assert_eq!(varint_text(&[0x01, 0x00]), "256");
    }

    #[test]
    fn system_keyspaces_are_hidden() {
        assert!(is_system_keyspace("system"));
        assert!(is_system_keyspace("system_schema"));
        assert!(is_system_keyspace("system_auth"));
        assert!(!is_system_keyspace("store"));
    }

    #[test]
    fn plain_text_for_scalars() {
        assert_eq!(value_text(&CqlValue::Text("it's".into())), "it's");
        assert_eq!(value_text(&CqlValue::Int(42)), "42");
        assert_eq!(list_items("['id', 'qty']"), vec!["id", "qty"]);
        assert_eq!(list_items("['id','qty']"), vec!["id", "qty"]);
        assert_eq!(cql_map_get("{'class_name':'x','target':'status'}", "target").as_deref(), Some("status"));
        assert_eq!(cql_map_get("{'target': 'values(tags)'}", "target").as_deref(), Some("values(tags)"));
        assert_eq!(cql_map_get("{}", "target"), None);
    }
}
