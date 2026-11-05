//! Shared HTTP plumbing for engines with REST APIs (CouchDB, InfluxDB).

use databug_core::{ConnectionConfig, SslMode};
use serde_json::Value;
use std::time::Duration;

/// An HTTP client that encrypts with rustls but accepts self-signed
/// certificates, like the other drivers.
pub(crate) fn client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder().use_preconfigured_tls(crate::tls::no_verify_config()).connect_timeout(Duration::from_secs(10)).user_agent(concat!("DataBug/", env!("CARGO_PKG_VERSION"))).build()?)
}

/// The server's base URL without a trailing slash: the URI field when set
/// (e.g. a cloud endpoint), otherwise `http(s)://host:port`. HTTPS is used
/// only for `Require`, since trying both would sign in twice.
pub(crate) fn base_url(cfg: &ConnectionConfig) -> String {
    let uri = cfg.uri.trim().trim_end_matches('/');
    if !uri.is_empty() {
        return uri.to_string();
    }
    let host = cfg.host.trim();
    if host.starts_with("http://") || host.starts_with("https://") {
        return format!("{}:{}", host.trim_end_matches('/'), cfg.port);
    }
    let scheme = if cfg.ssl == SslMode::Require { "https" } else { "http" };
    let host = if host.is_empty() { "localhost" } else { host };
    format!("{scheme}://{host}:{}", cfg.port)
}

/// Turn a non-2xx response into an error with the server's message.
pub(crate) async fn check(resp: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let msg = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| {
            let parts: Vec<String> = ["error", "reason", "message", "code"].iter().filter_map(|k| v.get(*k).and_then(|x| x.as_str()).map(str::to_string)).collect();
            if parts.is_empty() { None } else { Some(parts.join(": ")) }
        })
        .unwrap_or_else(|| body.trim().chars().take(500).collect());
    match status.as_u16() {
        401 => anyhow::bail!("Sign-in failed (401): {msg}"),
        403 => anyhow::bail!("Not allowed (403): {msg}"),
        _ => anyhow::bail!("HTTP {status}: {msg}"),
    }
}

/// Grid text for a JSON value: strings as-is, everything else as JSON.
pub(crate) fn json_text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

pub(crate) fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_f64() => "double",
        Value::Number(_) => "integer",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Stream JSON objects as a result set whose columns are the union of
/// their top-level keys, in first-seen order.
#[cfg(feature = "couchdb")]
pub(crate) fn send_objects(docs: &[Value], limit: usize, sink: &crate::Sink) {
    let mut cols: Vec<(String, &'static str)> = Vec::new();
    for d in docs {
        if let Some(o) = d.as_object() {
            for (k, v) in o {
                if !cols.iter().any(|(c, _)| c == k) {
                    cols.push((k.clone(), json_type(v)));
                }
            }
        }
    }
    let scalar = cols.is_empty();
    if scalar {
        cols.push(("value".into(), "json"));
    }
    let metas = cols.iter().map(|(n, t)| databug_core::ColumnMeta::new(n.clone(), *t)).collect();
    let mut b = crate::Batcher::new(sink, metas, limit);
    for d in docs {
        for (k, _) in &cols {
            let v = if scalar { Some(d) } else { d.get(k) };
            b.cells().push(v.and_then(json_text).as_deref());
        }
        if !b.row_done() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use databug_core::DbKind;

    #[test]
    fn base_urls() {
        let mut c = ConnectionConfig::new(DbKind::CouchDb);
        c.host = "couch.example.com".into();
        assert_eq!(base_url(&c), "http://couch.example.com:5984");
        c.ssl = SslMode::Require;
        assert_eq!(base_url(&c), "https://couch.example.com:5984");
        c.uri = "https://cloud.example.com/".into();
        assert_eq!(base_url(&c), "https://cloud.example.com");
    }

    #[test]
    fn json_cells() {
        assert_eq!(json_text(&serde_json::json!("a")), Some("a".into()));
        assert_eq!(json_text(&serde_json::json!({"x": 1})), Some("{\"x\":1}".into()));
        assert_eq!(json_text(&Value::Null), None);
    }
}
