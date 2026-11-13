//! Lightweight semantic analysis over tokens: table references and aliases,
//! statement classification, safety checks and catalog-backed inspections.

use crate::lexer::{tokenize, TokKind, Token};
use crate::split::split_statements;
use std::ops::Range;
use databug_core::{Catalog, Dialect};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
    /// Byte range of the name (relative to the analyzed text).
    pub span: Range<usize>,
    /// Defined by a CTE (`WITH name AS (...)`) rather than the catalog.
    pub is_cte: bool,
}

impl TableRef {
    pub fn matches_qualifier(&self, q: &str) -> bool {
        match &self.alias {
            Some(a) => a.eq_ignore_ascii_case(q),
            None => self.name.eq_ignore_ascii_case(q),
        }
    }
}

pub fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() >= 2 {
        let (f, l) = (b[0], b[b.len() - 1]);
        if (f == b'"' && l == b'"') || (f == b'`' && l == b'`') || (f == b'[' && l == b']') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

pub(crate) fn significant(toks: &[Token]) -> Vec<Token> {
    toks.iter().copied().filter(|t| !t.is_trivia()).collect()
}

const REF_INTRO: &[&str] = &["FROM", "JOIN", "UPDATE", "INTO", "TABLE"];
const ALIAS_STOP: &[&str] = &[
    "WHERE", "ON", "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "CROSS", "GROUP", "ORDER", "LIMIT",
    "SET", "VALUES", "USING", "HAVING", "UNION", "EXCEPT", "INTERSECT", "WINDOW", "OFFSET",
    "FETCH", "FOR", "NATURAL", "LATERAL", "RETURNING", "SELECT", "OUTER", "WITH",
];

/// All table references in one statement, plus CTE names.
pub fn table_refs(src: &str, dialect: Dialect) -> Vec<TableRef> {
    let toks = significant(&tokenize(src, dialect));
    let mut refs = Vec::new();

    // CTE names: WITH [RECURSIVE] name [(cols)] AS (
    let mut i = 0;
    if toks.first().map(|t| t.is_kw(src, "WITH")).unwrap_or(false) {
        i = 1;
        if toks.get(i).map(|t| t.is_kw(src, "RECURSIVE")).unwrap_or(false) {
            i += 1;
        }
        while let Some(name) = toks.get(i).filter(|t| t.is_name()) {
            refs.push(TableRef {
                schema: None,
                name: unquote(name.text(src)),
                alias: None,
                span: name.start..name.end,
                is_cte: true,
            });
            i += 1;
            if toks.get(i).map(|t| t.is_punct(src, "(")).unwrap_or(false) {
                i = skip_parens(&toks, src, i);
            }
            if !toks.get(i).map(|t| t.is_kw(src, "AS")).unwrap_or(false) {
                break;
            }
            i += 1;
            while toks.get(i).map(|t| t.is_kw(src, "NOT") || t.text(src).eq_ignore_ascii_case("MATERIALIZED")).unwrap_or(false) {
                i += 1;
            }
            if !toks.get(i).map(|t| t.is_punct(src, "(")).unwrap_or(false) {
                break;
            }
            i = skip_parens(&toks, src, i);
            if toks.get(i).map(|t| t.is_punct(src, ",")).unwrap_or(false) {
                i += 1;
                continue;
            }
            break;
        }
    }
    let _ = i;

    let mut k = 0;
    while k < toks.len() {
        let t = toks[k];
        let intro = t.kind == TokKind::Keyword && REF_INTRO.iter().any(|kw| t.text(src).eq_ignore_ascii_case(kw));
        if !intro {
            k += 1;
            continue;
        }
        let is_from = t.is_kw(src, "FROM");
        k += 1;
        loop {
            // skip `ONLY`, `IF EXISTS`, `LATERAL`
            while toks.get(k).map(|t| t.is_kw(src, "ONLY") || t.is_kw(src, "LATERAL") || t.is_kw(src, "IF") || t.is_kw(src, "NOT") || t.is_kw(src, "EXISTS")).unwrap_or(false) {
                k += 1;
            }
            let Some(first) = toks.get(k).copied() else { break };
            if first.is_punct(src, "(") {
                // subquery: skip it, then an optional alias
                k = skip_parens(&toks, src, k);
                if toks.get(k).map(|t| t.is_kw(src, "AS")).unwrap_or(false) {
                    k += 1;
                }
                if toks.get(k).map(|t| t.is_name()).unwrap_or(false) {
                    k += 1;
                }
            } else if first.is_name() {
                let mut parts = vec![first];
                k += 1;
                while toks.get(k).map(|t| t.is_punct(src, ".")).unwrap_or(false) {
                    match toks.get(k + 1) {
                        Some(n) if n.is_name() || n.kind == TokKind::Keyword => {
                            parts.push(*n);
                            k += 2;
                        }
                        _ => {
                            k += 1;
                            break;
                        }
                    }
                }
                // table-valued function call: name(...)
                if toks.get(k).map(|t| t.is_punct(src, "(")).unwrap_or(false) && !is_from_insert_cols(&toks, src, k) {
                    k = skip_parens(&toks, src, k);
                    parts.clear();
                }
                let mut alias = None;
                if toks.get(k).map(|t| t.is_kw(src, "AS")).unwrap_or(false) {
                    k += 1;
                }
                if let Some(a) = toks.get(k) {
                    if a.is_name() && !ALIAS_STOP.iter().any(|s| a.text(src).eq_ignore_ascii_case(s)) {
                        alias = Some(unquote(a.text(src)));
                        k += 1;
                    }
                }
                if let Some(last) = parts.last() {
                    let name = unquote(last.text(src));
                    let schema = if parts.len() >= 2 { Some(unquote(parts[parts.len() - 2].text(src))) } else { None };
                    let is_cte = schema.is_none() && refs.iter().any(|r: &TableRef| r.is_cte && r.name.eq_ignore_ascii_case(&name));
                    refs.push(TableRef { schema, name, alias, span: last.start..last.end, is_cte });
                }
            } else {
                break;
            }
            if is_from && toks.get(k).map(|t| t.is_punct(src, ",")).unwrap_or(false) {
                k += 1;
                continue;
            }
            break;
        }
    }
    // CTE definitions are listed first; drop references that merely name a CTE.
    let mut seen_defs = Vec::new();
    refs.retain(|r| {
        if r.is_cte && !seen_defs.iter().any(|n: &String| n.eq_ignore_ascii_case(&r.name)) {
            seen_defs.push(r.name.clone());
            return true;
        }
        !r.is_cte
    });
    refs
}

/// Index just past the paren group opening at `i`.
pub(crate) fn skip_parens(toks: &[Token], src: &str, mut i: usize) -> usize {
    let mut depth = 0;
    while i < toks.len() {
        match toks[i].text(src) {
            "(" => depth += 1,
            ")" => {
                depth -= 1;
                if depth <= 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}

fn is_from_insert_cols(toks: &[Token], src: &str, k: usize) -> bool {
    // INSERT INTO t (a, b) — parens list columns, not a function call.
    toks[..k].iter().rev().find(|t| t.kind == TokKind::Keyword).map(|t| t.is_kw(src, "INTO") || t.is_kw(src, "TABLE")).unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    Read,
    Write,
    Ddl,
    Transaction,
    Other,
}

impl StatementKind {
    /// Whether the statement may change data or schema.
    pub fn modifies(self) -> bool {
        matches!(self, StatementKind::Write | StatementKind::Ddl | StatementKind::Other)
    }
}

pub fn classify(stmt: &str, dialect: Dialect) -> StatementKind {
    if dialect == Dialect::Mongo {
        return crate::mongo::classify(stmt);
    }
    let toks = significant(&tokenize(stmt, dialect));
    let Some(first) = toks.first() else { return StatementKind::Other };
    let w = first.text(stmt).to_ascii_uppercase();
    match w.as_str() {
        "SELECT" | "WITH" | "VALUES" | "TABLE" => {
            let writes = toks.iter().any(|t| {
                t.kind == TokKind::Keyword && ["INSERT", "UPDATE", "DELETE", "MERGE"].iter().any(|k| t.text(stmt).eq_ignore_ascii_case(k))
            });
            let into = w == "SELECT" && toks.iter().any(|t| t.is_kw(stmt, "INTO")) && dialect == Dialect::MsSql;
            if writes || into {
                StatementKind::Write
            } else {
                StatementKind::Read
            }
        }
        "SHOW" | "EXPLAIN" | "DESCRIBE" | "DESC" => StatementKind::Read,
        "PRAGMA" => {
            if stmt.contains('=') {
                StatementKind::Other
            } else {
                StatementKind::Read
            }
        }
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "REPLACE" | "UPSERT" | "COPY" | "CALL" | "EXEC" | "EXECUTE" => StatementKind::Write,
        "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "GRANT" | "REVOKE" | "RENAME" | "COMMENT" => StatementKind::Ddl,
        "BEGIN" | "COMMIT" | "ROLLBACK" | "START" | "SAVEPOINT" | "RELEASE" | "END" => StatementKind::Transaction,
        _ => StatementKind::Other,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Danger {
    DeleteWithoutWhere,
    UpdateWithoutWhere,
    Drop,
    Truncate,
}

impl Danger {
    pub fn message(&self) -> &'static str {
        match self {
            Danger::DeleteWithoutWhere => "DELETE without WHERE removes every row in the table",
            Danger::UpdateWithoutWhere => "UPDATE without WHERE changes every row in the table",
            Danger::Drop => "DROP permanently removes the object",
            Danger::Truncate => "TRUNCATE removes every row in the table",
        }
    }
}

pub fn dangers(stmt: &str, dialect: Dialect) -> Vec<Danger> {
    if dialect == Dialect::Mongo {
        return crate::mongo::dangers(stmt);
    }
    let toks = significant(&tokenize(stmt, dialect));
    let mut out = Vec::new();
    let first = toks.iter().find(|t| t.kind == TokKind::Keyword).map(|t| t.text(stmt).to_ascii_uppercase());
    // find the main verb (skipping a leading CTE)
    let mut depth = 0;
    let mut verb: Option<String> = None;
    let mut has_where = false;
    for t in &toks {
        let s = t.text(stmt);
        if s == "(" {
            depth += 1;
        } else if s == ")" {
            depth -= 1;
        } else if depth == 0 && t.kind == TokKind::Keyword {
            let up = s.to_ascii_uppercase();
            if verb.is_none() && matches!(up.as_str(), "DELETE" | "UPDATE" | "SELECT" | "INSERT") {
                verb = Some(up);
            } else if up == "WHERE" {
                has_where = true;
            }
        }
    }
    match verb.as_deref() {
        Some("DELETE") if !has_where => out.push(Danger::DeleteWithoutWhere),
        Some("UPDATE") if !has_where => out.push(Danger::UpdateWithoutWhere),
        _ => {}
    }
    match first.as_deref() {
        Some("DROP") => out.push(Danger::Drop),
        Some("TRUNCATE") => out.push(Danger::Truncate),
        _ => {}
    }
    out
}

/// If the statement is a plain single-table SELECT (so its rows can be edited
/// in the grid), return that table.
pub fn single_table_source(stmt: &str, dialect: Dialect) -> Option<TableRef> {
    if classify(stmt, dialect) != StatementKind::Read || dialect == Dialect::Mongo {
        return None;
    }
    let toks = significant(&tokenize(stmt, dialect));
    if !toks.first()?.is_kw(stmt, "SELECT") {
        return None;
    }
    let blocked = ["JOIN", "GROUP", "DISTINCT", "UNION", "EXCEPT", "INTERSECT", "HAVING"];
    if toks.iter().any(|t| t.kind == TokKind::Keyword && blocked.iter().any(|b| t.text(stmt).eq_ignore_ascii_case(b))) {
        return None;
    }
    let refs = table_refs(stmt, dialect);
    if refs.len() == 1 && !refs[0].is_cte {
        refs.into_iter().next()
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub range: Range<usize>,
    pub message: String,
    pub severity: Severity,
}

/// Inspections over a whole script: unknown tables and columns (when a
/// catalog is loaded) and risky statements.
pub fn inspect(src: &str, catalog: Option<&Catalog>, dialect: Dialect) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if dialect == Dialect::Mongo {
        return out;
    }
    for range in split_statements(src, dialect) {
        let stmt = &src[range.clone()];
        let base = range.start;
        for d in dangers(stmt, dialect) {
            let verb_end = stmt.find(char::is_whitespace).unwrap_or(stmt.len());
            out.push(Diagnostic { range: base..base + verb_end, message: d.message().into(), severity: Severity::Warning });
        }
        let Some(cat) = catalog else { continue };
        if cat.schemas.is_empty() {
            continue;
        }
        let refs = table_refs(stmt, dialect);
        let creating = classify(stmt, dialect) == StatementKind::Ddl;
        for r in &refs {
            if r.is_cte || creating {
                continue;
            }
            if r.schema.as_deref().map(|s| cat.schema(s).is_none()).unwrap_or(false) {
                continue; // other database / unknown schema: don't guess
            }
            if cat.find_table(r.schema.as_deref(), &r.name).is_none() {
                out.push(Diagnostic {
                    range: base + r.span.start..base + r.span.end,
                    message: format!("Unknown table '{}'", r.name),
                    severity: Severity::Error,
                });
            }
        }
        // qualified column references: alias.column
        let toks = significant(&tokenize(stmt, dialect));
        for w in toks.windows(3) {
            if !(w[0].is_name() && w[1].is_punct(stmt, ".") && w[2].is_name()) {
                continue;
            }
            let q = unquote(w[0].text(stmt));
            let col = unquote(w[2].text(stmt));
            let Some(r) = refs.iter().find(|r| r.matches_qualifier(&q) && !r.is_cte) else { continue };
            if r.span.start == w[0].start || r.span.start == w[2].start {
                continue; // schema.table itself
            }
            if let Some(t) = cat.find_table(r.schema.as_deref(), &r.name) {
                if t.kind != databug_core::ObjectKind::Collection && t.column(&col).is_none() {
                    out.push(Diagnostic {
                        range: base + w[2].start..base + w[2].end,
                        message: format!("Unknown column '{}' in '{}'", col, t.name),
                        severity: Severity::Error,
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_with_aliases() {
        let s = "SELECT * FROM sales.orders o, items i JOIN customers AS c ON c.id = o.customer_id";
        let r = table_refs(s, Dialect::Postgres);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].schema.as_deref(), Some("sales"));
        assert_eq!(r[0].alias.as_deref(), Some("o"));
        assert_eq!(r[2].alias.as_deref(), Some("c"));
    }

    #[test]
    fn refs_skip_where() {
        let r = table_refs("select * from t where x = 1", Dialect::Postgres);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].alias, None);
    }

    #[test]
    fn cte_refs() {
        let r = table_refs("WITH x AS (SELECT * FROM a) SELECT * FROM x JOIN b ON true", Dialect::Postgres);
        let names: Vec<_> = r.iter().map(|r| (r.name.as_str(), r.is_cte)).collect();
        assert!(names.contains(&("x", true)));
        assert!(names.contains(&("a", false)));
        assert!(names.contains(&("b", false)));
        assert_eq!(names.iter().filter(|n| n.0 == "x").count(), 1);
    }

    #[test]
    fn safety() {
        assert_eq!(dangers("DELETE FROM t", Dialect::Postgres), vec![Danger::DeleteWithoutWhere]);
        assert!(dangers("DELETE FROM t WHERE id = 1", Dialect::Postgres).is_empty());
        assert_eq!(dangers("update t set a = (select 1 where true)", Dialect::Postgres), vec![Danger::UpdateWithoutWhere]);
        assert_eq!(classify("with x as (select 1) delete from t", Dialect::Postgres), StatementKind::Write);
        assert_eq!(classify("select 1", Dialect::Postgres), StatementKind::Read);
    }

    #[test]
    fn editable_source() {
        assert!(single_table_source("select * from t where a = 1", Dialect::Postgres).is_some());
        assert!(single_table_source("select * from t join u on true", Dialect::Postgres).is_none());
        assert!(single_table_source("select count(*) from t group by a", Dialect::Postgres).is_none());
    }
}
