//! Turning each engine's native plan format into a `PlanNode` tree.

use serde_json::Value;
use databug_core::PlanNode;

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
}

/// PostgreSQL `EXPLAIN (FORMAT JSON)`.
pub fn from_pg_json(text: &str) -> Option<PlanNode> {
    let v: Value = serde_json::from_str(text).ok()?;
    let root = v.get(0)?.get("Plan")?;
    Some(pg_node(root))
}

fn pg_node(p: &Value) -> PlanNode {
    let s = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let mut label = s("Node Type").to_string();
    if !s("Join Type").is_empty() {
        label = format!("{} {}", s("Join Type"), label);
    }
    if !s("Relation Name").is_empty() {
        label.push_str(" on ");
        label.push_str(s("Relation Name"));
        if !s("Alias").is_empty() && s("Alias") != s("Relation Name") {
            label.push(' ');
            label.push_str(s("Alias"));
        }
    }
    if !s("Index Name").is_empty() {
        label.push_str(" using ");
        label.push_str(s("Index Name"));
    }
    let keys = ["Index Cond", "Filter", "Hash Cond", "Merge Cond", "Join Filter", "Recheck Cond", "Sort Key", "Group Key", "Strategy"];
    let detail: Vec<String> = keys
        .iter()
        .filter_map(|k| {
            let v = p.get(*k)?;
            let txt = match v {
                Value::String(s) => s.clone(),
                Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "),
                other => other.to_string(),
            };
            Some(format!("{k}: {txt}"))
        })
        .collect();
    let loops = num(p.get("Actual Loops")).unwrap_or(1.0);
    PlanNode {
        label,
        detail: detail.join("  ·  "),
        total_cost: num(p.get("Total Cost")),
        startup_cost: num(p.get("Startup Cost")),
        rows: num(p.get("Actual Rows")).or_else(|| num(p.get("Plan Rows"))),
        actual_ms: num(p.get("Actual Total Time")).map(|t| t * loops),
        children: p.get("Plans").and_then(|v| v.as_array()).map(|a| a.iter().map(pg_node).collect()).unwrap_or_default(),
    }
}

/// Any JSON plan (MySQL `FORMAT=JSON`, MongoDB explain): objects become nodes,
/// nested objects and arrays of objects become children.
pub fn from_generic_json(text: &str) -> Option<PlanNode> {
    let v: Value = serde_json::from_str(text).ok()?;
    Some(generic_node("plan", &v))
}

fn generic_node(key: &str, v: &Value) -> PlanNode {
    let mut node = PlanNode { label: key.to_string(), ..Default::default() };
    let Value::Object(map) = v else {
        if let Value::Array(a) = v {
            node.children = a.iter().enumerate().map(|(i, x)| generic_node(&format!("{key}[{i}]"), x)).collect();
        }
        return node;
    };
    let pick = |k: &str| map.get(k).and_then(|v| v.as_str());
    if let Some(stage) = pick("stage").or(pick("table_name")).or(pick("operation")) {
        node.label = match pick("access_type") {
            Some(a) => format!("{stage} ({a})"),
            None => stage.to_string(),
        };
        if let Some(idx) = pick("indexName").or(pick("key")) {
            node.label.push_str(&format!(" using {idx}"));
        }
    }
    let mut detail = Vec::new();
    for (k, val) in map {
        match val {
            Value::Object(_) if k == "cost_info" => {
                node.total_cost = num(val.get("query_cost")).or_else(|| num(val.get("prefix_cost"))).or_else(|| num(val.get("read_cost")));
            }
            Value::Object(_) => node.children.push(generic_node(k, val)),
            Value::Array(a) if a.iter().any(|x| x.is_object()) => {
                for (i, x) in a.iter().enumerate() {
                    node.children.push(generic_node(&format!("{k}[{i}]"), x));
                }
            }
            Value::String(s) if !matches!(k.as_str(), "stage" | "table_name") => detail.push(format!("{k}: {s}")),
            Value::Number(n) => {
                if matches!(k.as_str(), "rows_examined_per_scan" | "nReturned" | "docsExamined" | "rows") {
                    node.rows = n.as_f64();
                }
                if k == "executionTimeMillisEstimate" || k == "executionTimeMillis" {
                    node.actual_ms = n.as_f64();
                }
                detail.push(format!("{k}: {n}"));
            }
            Value::Bool(b) => detail.push(format!("{k}: {b}")),
            _ => {}
        }
    }
    detail.truncate(8);
    node.detail = detail.join("  ·  ");
    node
}

/// SQLite `EXPLAIN QUERY PLAN` rows: (id, parent, detail).
pub fn from_sqlite_rows(rows: &[(i64, i64, String)]) -> PlanNode {
    fn build(parent: i64, rows: &[(i64, i64, String)]) -> Vec<PlanNode> {
        rows.iter()
            .filter(|r| r.1 == parent)
            .map(|r| PlanNode { label: r.2.clone(), children: build(r.0, rows), ..Default::default() })
            .collect()
    }
    PlanNode { label: "QUERY PLAN".into(), children: build(0, rows), ..Default::default() }
}

/// SQL Server showplan XML: nested `<RelOp>` elements.
pub fn from_mssql_xml(xml: &str) -> Option<PlanNode> {
    let mut stack: Vec<PlanNode> = vec![PlanNode { label: "Statement".into(), ..Default::default() }];
    let mut i = 0;
    let b = xml.as_bytes();
    while let Some(p) = xml[i..].find('<') {
        let start = i + p;
        let end = start + xml[start..].find('>')?;
        let tag = &xml[start + 1..end];
        i = end + 1;
        if let Some(rest) = tag.strip_prefix("RelOp") {
            let attr = |name: &str| -> Option<String> {
                let key = format!(" {name}=\"");
                let s = rest.find(&key)? + key.len();
                let e = s + rest[s..].find('"')?;
                Some(rest[s..e].replace("&gt;", ">").replace("&lt;", "<").replace("&amp;", "&"))
            };
            let phys = attr("PhysicalOp").unwrap_or_default();
            let logical = attr("LogicalOp").unwrap_or_default();
            let node = PlanNode {
                label: if logical.is_empty() || logical == phys { phys } else { format!("{phys} ({logical})") },
                detail: String::new(),
                total_cost: attr("EstimatedTotalSubtreeCost").and_then(|v| v.parse().ok()),
                startup_cost: None,
                rows: attr("EstimateRows").and_then(|v| v.parse().ok()),
                actual_ms: None,
                children: vec![],
            };
            if tag.ends_with('/') {
                stack.last_mut()?.children.push(node);
            } else {
                stack.push(node);
            }
        } else if tag.starts_with("/RelOp") {
            let n = stack.pop()?;
            stack.last_mut()?.children.push(n);
        } else if let Some(obj) = tag.strip_prefix("Object ") {
            // attach table/index names to the innermost operator
            let get = |name: &str| {
                let key = format!("{name}=\"");
                obj.find(&key).map(|s| {
                    let s = s + key.len();
                    obj[s..s + obj[s..].find('"').unwrap_or(0)].to_string()
                })
            };
            if let Some(top) = stack.last_mut() {
                let t = get("Table").unwrap_or_default();
                let ix = get("Index").unwrap_or_default();
                if top.detail.is_empty() && !t.is_empty() {
                    top.detail = if ix.is_empty() { t } else { format!("{t} {ix}") };
                }
            }
        } else if let Some(stats) = tag.strip_prefix("RunTimeCountersPerThread ") {
            if let Some(top) = stack.last_mut() {
                let get = |name: &str| {
                    let key = format!("{name}=\"");
                    stats.find(&key).and_then(|s| {
                        let s = s + key.len();
                        stats[s..s + stats[s..].find('"')?].parse::<f64>().ok()
                    })
                };
                if let Some(r) = get("ActualRows") {
                    top.rows = Some(top.rows.unwrap_or(0.0).max(r));
                }
                if let Some(ms) = get("ActualElapsedms") {
                    top.actual_ms = Some(top.actual_ms.unwrap_or(0.0).max(ms));
                }
            }
        }
        let _ = b;
    }
    while stack.len() > 1 {
        let n = stack.pop()?;
        stack.last_mut()?.children.push(n);
    }
    let mut root = stack.pop()?;
    if root.children.len() == 1 {
        root = root.children.pop()?;
    }
    Some(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_plan() {
        let j = r#"[{"Plan": {"Node Type": "Hash Join", "Join Type": "Inner", "Total Cost": 10.5, "Startup Cost": 1.0, "Plan Rows": 100, "Hash Cond": "(o.cid = c.id)",
          "Plans": [{"Node Type": "Seq Scan", "Relation Name": "orders", "Alias": "o", "Total Cost": 5.0, "Plan Rows": 100}]}}]"#;
        let n = from_pg_json(j).unwrap();
        assert_eq!(n.label, "Inner Hash Join");
        assert_eq!(n.children[0].label, "Seq Scan on orders o");
        assert!(n.detail.contains("Hash Cond"));
    }

    #[test]
    fn mssql_plan() {
        let x = r#"<ShowPlanXML><RelOp NodeId="0" PhysicalOp="Hash Match" LogicalOp="Inner Join" EstimateRows="10" EstimatedTotalSubtreeCost="0.5"><Hash><RelOp NodeId="1" PhysicalOp="Clustered Index Scan" LogicalOp="Clustered Index Scan" EstimateRows="10" EstimatedTotalSubtreeCost="0.1"><Object Table="[orders]" Index="[PK]"/></RelOp></Hash></RelOp></ShowPlanXML>"#;
        let n = from_mssql_xml(x).unwrap();
        assert_eq!(n.label, "Hash Match (Inner Join)");
        assert_eq!(n.children[0].detail, "[orders] [PK]");
    }

    #[test]
    fn sqlite_plan() {
        let n = from_sqlite_rows(&[(2, 0, "SCAN t".into()), (5, 0, "SEARCH u USING INDEX".into())]);
        assert_eq!(n.children.len(), 2);
    }
}
