//! Lightweight semantic analysis over tokens: table references and aliases,
//! statement classification, safety checks and catalog-backed inspections.

use crate::lexer::{tokenize, TokKind, Token};
use crate::split::split_statements;
use std::ops::Range;
use databug_core::{Catalog, Dialect, ObjectKind};

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

/// The possibly qualified object name the byte offset `at` is on, as
/// `(schema, name)` without quotes: `dbo.usp_load`, `[etl].[load]`,
/// `public."Orders"`. A database prefix (`db.schema.name`) is dropped.
pub fn name_at(src: &str, at: usize, dialect: Dialect) -> Option<(Option<String>, String)> {
    let toks: Vec<Token> = tokenize(src, dialect).into_iter().filter(|t| t.kind != TokKind::Whitespace && t.kind != TokKind::Comment).collect();
    let is_name = |t: &Token| matches!(t.kind, TokKind::Ident | TokKind::QuotedIdent | TokKind::Keyword);
    let is_dot = |t: &Token| t.kind == TokKind::Punct && t.text(src) == ".";
    // the name token under the cursor, or just before it (cursor at its end)
    let i = toks.iter().position(|t| is_name(t) && t.start <= at && at < t.end).or_else(|| toks.iter().position(|t| is_name(t) && t.end == at))?;
    // extend to the whole dotted chain
    let mut a = i;
    while a >= 2 && is_dot(&toks[a - 1]) && is_name(&toks[a - 2]) && toks[a - 1].start == toks[a - 2].end && toks[a].start == toks[a - 1].end {
        a -= 2;
    }
    let mut b = i;
    while b + 2 < toks.len() && is_dot(&toks[b + 1]) && is_name(&toks[b + 2]) && toks[b + 1].start == toks[b].end && toks[b + 2].start == toks[b + 1].end {
        b += 2;
    }
    let parts: Vec<String> = (a..=b).step_by(2).map(|k| unquote(toks[k].text(src))).collect();
    let n = parts.len();
    let name = parts[n - 1].clone();
    let schema = (n >= 2).then(|| parts[n - 2].clone());
    Some((schema, name))
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
    if dialect == Dialect::Redis {
        return crate::redis::classify(stmt);
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
        // T-SQL variables: declaring, assigning and printing them changes nothing
        "DECLARE" | "PRINT" if dialect == Dialect::MsSql => StatementKind::Read,
        "SET" if dialect == Dialect::MsSql && toks.get(1).map(|t| t.kind == TokKind::Param).unwrap_or(false) => {
            let writes = toks.iter().any(|t| t.kind == TokKind::Keyword && ["INSERT", "UPDATE", "DELETE", "MERGE"].iter().any(|k| t.text(stmt).eq_ignore_ascii_case(k)));
            if writes {
                StatementKind::Write
            } else {
                StatementKind::Read
            }
        }
        "PRAGMA" => {
            if stmt.contains('=') {
                StatementKind::Other
            } else {
                StatementKind::Read
            }
        }
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "REPLACE" | "UPSERT" | "COPY" | "CALL" | "EXEC" | "EXECUTE" => StatementKind::Write,
        "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "GRANT" | "REVOKE" | "RENAME" | "COMMENT" => StatementKind::Ddl,
        // `BEGIN … END` blocks (PL/SQL, T-SQL `BEGIN TRY`) aren't transactions
        "BEGIN" => match toks.get(1).map(|t| t.text(stmt).to_ascii_uppercase()) {
            None => StatementKind::Transaction,
            Some(w) if matches!(w.as_str(), ";" | "TRAN" | "TRANSACTION" | "WORK" | "DEFERRED" | "IMMEDIATE" | "EXCLUSIVE" | "ISOLATION" | "READ" | "DEFERRABLE" | "NOT" | "DISTRIBUTED") => StatementKind::Transaction,
            Some(_) => StatementKind::Other,
        },
        "COMMIT" | "ROLLBACK" | "START" | "SAVEPOINT" | "RELEASE" | "END" => StatementKind::Transaction,
        _ => StatementKind::Other,
    }
}

/// Whether running `stmt` may add, change or remove something the explorer
/// shows, so the schema should be reloaded afterwards: DDL, `SELECT … INTO`
/// (SQL Server), procedures that rename or create objects, and on Redis and
/// MongoDB any write (it can create or remove a key or collection).
pub fn changes_schema(stmt: &str, dialect: Dialect) -> bool {
    let kind = classify(stmt, dialect);
    match dialect {
        Dialect::Mongo | Dialect::Redis => matches!(kind, StatementKind::Ddl | StatementKind::Write),
        _ => match kind {
            StatementKind::Ddl => true,
            StatementKind::Write => {
                let toks = significant(&tokenize(stmt, dialect));
                let first = toks.first().map(|t| t.text(stmt).to_ascii_uppercase()).unwrap_or_default();
                match first.as_str() {
                    // SELECT … INTO creates a table
                    "SELECT" | "WITH" => toks.iter().any(|t| t.is_kw(stmt, "INTO")),
                    // stored procedures may create, rename or drop objects
                    // (sp_rename, sp_addextendedproperty, a migration procedure…)
                    "EXEC" | "EXECUTE" | "CALL" => true,
                    _ => false,
                }
            }
            // anonymous blocks (`BEGIN … END`, `DO $$ … $$`) may run DDL
            StatementKind::Other => true,
            StatementKind::Read | StatementKind::Transaction => false,
        },
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
    if dialect == Dialect::Redis {
        return crate::redis::dangers(stmt);
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
    if classify(stmt, dialect) != StatementKind::Read || matches!(dialect, Dialect::Mongo | Dialect::Redis) {
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
    if matches!(dialect, Dialect::Mongo | Dialect::Redis) {
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
        // procedures and functions that don't exist
        let toks = significant(&tokenize(stmt, dialect));
        unknown_routines(stmt, base, &toks, &refs, cat, dialect, &mut out);
        // qualified column references: alias.column
        // only queries and DML qualify columns with aliases (not EXEC schema.proc)
        let first = toks.first().map(|t| t.text(stmt).to_ascii_uppercase()).unwrap_or_default();
        let dml = matches!(classify(stmt, dialect), StatementKind::Read | StatementKind::Write) && matches!(first.as_str(), "SELECT" | "WITH" | "INSERT" | "UPDATE" | "DELETE" | "MERGE");
        for (k, w) in toks.windows(3).enumerate() {
            if !(w[0].is_name() && w[1].is_punct(stmt, ".") && w[2].is_name()) {
                continue;
            }
            let q = unquote(w[0].text(stmt));
            let col = unquote(w[2].text(stmt));
            let Some(r) = refs.iter().find(|r| r.matches_qualifier(&q) && !r.is_cte) else {
                if dml && unknown_qualifier(stmt, &toks, k, &q, &refs, cat) {
                    out.push(Diagnostic { range: base + w[0].start..base + w[0].end, message: format!("Unknown table or alias '{q}'"), severity: Severity::Error });
                }
                continue;
            };
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

/// Whether the qualifier of `q.column` (at token `k`) names nothing this
/// statement defines. Conservative: anything that could be an alias (the
/// name appears in the statement other than before a dot), a schema, a
/// table, a CTE, a pseudo-table (`inserted`, `excluded`, `NEW`…) or a method
/// call (`geo.STDistance(…)`) counts as defined.
fn unknown_qualifier(stmt: &str, toks: &[Token], k: usize, q: &str, refs: &[TableRef], cat: &Catalog) -> bool {
    const PSEUDO: &[&str] = &["inserted", "deleted", "excluded", "new", "old", "sys", "information_schema", "pg_catalog", "dual", "mysql", "performance_schema", "dbo"];
    if PSEUDO.iter().any(|p| p.eq_ignore_ascii_case(q)) {
        return false;
    }
    // part of a longer name (db.schema.table, a.b.c) or a call (x.f(…))
    if k > 0 && toks[k - 1].is_punct(stmt, ".") {
        return false;
    }
    if toks.get(k + 3).map(|t| t.is_punct(stmt, ".") || t.is_punct(stmt, "(")).unwrap_or(false) {
        return false;
    }
    if cat.schema(q).is_some() || refs.iter().any(|r| r.name.eq_ignore_ascii_case(q) || r.schema.as_deref().map(|s| s.eq_ignore_ascii_case(q)).unwrap_or(false)) {
        return false;
    }
    // defined somewhere in the statement: `FROM t AS q`, `(SELECT …) q`,
    // `CROSS APPLY f(…) q`, `JOIN t q`, a CTE `WITH q AS (…)`
    let defined = toks.iter().enumerate().any(|(i, t)| t.is_name() && unquote(t.text(stmt)).eq_ignore_ascii_case(q) && !toks.get(i + 1).map(|n| n.is_punct(stmt, ".")).unwrap_or(false));
    !defined
}

fn is_routine(k: ObjectKind) -> bool {
    matches!(
        k,
        ObjectKind::Procedure | ObjectKind::Function | ObjectKind::TableFunction | ObjectKind::ClrProcedure | ObjectKind::ClrFunction | ObjectKind::ClrTableFunction | ObjectKind::ClrAggregate
    )
}

/// Flag `EXEC name` / `CALL name` of a procedure, and `schema.name(…)` calls
/// of a function, that the catalog doesn't have. Only when the catalog lists
/// routines at all (some engines' introspection doesn't), never for names in
/// another database, and never for unqualified function calls (built-ins
/// like `COUNT(…)` aren't in the catalog) or system procedures (`sp_…`).
fn unknown_routines(stmt: &str, base: usize, toks: &[Token], refs: &[TableRef], cat: &Catalog, dialect: Dialect, out: &mut Vec<Diagnostic>) {
    let has_routines = cat.schemas.iter().any(|s| s.objects.iter().any(|o| is_routine(o.kind)));
    if !has_routines {
        return;
    }
    let exists = |schema: Option<&str>, name: &str| -> bool {
        let found = |s: &databug_core::SchemaInfo| s.objects.iter().any(|o| is_routine(o.kind) && o.name.eq_ignore_ascii_case(name)) || s.table(name).is_some();
        match schema {
            Some(sc) => cat.schema(sc).map(found).unwrap_or(true), // unknown schema: don't guess
            None => cat.schemas.iter().any(found),
        }
    };
    let mut flag = |start: usize, end: usize, what: &str, name: &str| {
        if !out.iter().any(|d| d.range.start <= base + start && base + end <= d.range.end) {
            out.push(Diagnostic { range: base + start..base + end, message: format!("Unknown {what} '{name}'"), severity: Severity::Error });
        }
    };
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        let verb = t.text(stmt).to_ascii_uppercase();
        let calls = match dialect {
            Dialect::MsSql => verb == "EXEC" || verb == "EXECUTE",
            _ => verb == "CALL",
        };
        if calls && t.kind == TokKind::Keyword {
            let mut j = i + 1;
            // EXEC @status = dbo.proc …
            if toks.get(j).map(|x| x.kind == TokKind::Param).unwrap_or(false) && toks.get(j + 1).map(|x| x.text(stmt) == "=").unwrap_or(false) {
                j += 2;
            }
            // a qualified name: up to three parts
            let mut parts: Vec<&Token> = Vec::new();
            while let Some(n) = toks.get(j).filter(|n| n.is_name()) {
                parts.push(n);
                if toks.get(j + 1).map(|x| x.is_punct(stmt, ".")).unwrap_or(false) {
                    j += 2;
                } else {
                    j += 1;
                    break;
                }
            }
            if let (Some(first), Some(last)) = (parts.first(), parts.last()) {
                let name = unquote(last.text(stmt));
                let schema = (parts.len() == 2).then(|| unquote(first.text(stmt)));
                let system = name.to_ascii_lowercase().starts_with("sp_") || name.to_ascii_lowercase().starts_with("xp_");
                if parts.len() <= 2 && !system && !exists(schema.as_deref(), &name) {
                    flag(first.start, last.end, "procedure", &name);
                }
            }
            i = j.max(i + 1);
            continue;
        }
        // schema.function( … ): only qualified calls, in a schema we know
        if t.is_name()
            && toks.get(i + 1).map(|x| x.is_punct(stmt, ".")).unwrap_or(false)
            && toks.get(i + 2).map(|x| x.is_name()).unwrap_or(false)
            && toks.get(i + 3).map(|x| x.is_punct(stmt, "(")).unwrap_or(false)
            && !(i > 0 && toks[i - 1].is_punct(stmt, "."))
        {
            let schema = unquote(t.text(stmt));
            let name = unquote(toks[i + 2].text(stmt));
            let is_ref = refs.iter().any(|r| r.span.start <= t.start && toks[i + 2].end <= r.span.end);
            if cat.schema(&schema).is_some() && !is_ref && !exists(Some(&schema), &name) {
                flag(t.start, toks[i + 2].end, "function", &name);
            }
            i += 3;
            continue;
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_aliases() {
        use databug_core::{ColumnInfo, SchemaInfo, TableInfo};
        let mut orders = TableInfo::new("orders", ObjectKind::Table);
        for c in ["id", "total", "customer_id"] {
            orders.columns.push(ColumnInfo { name: c.into(), data_type: "int".into(), nullable: true, primary_key: c == "id", default: None });
        }
        let mut customers = TableInfo::new("customers", ObjectKind::Table);
        for c in ["id", "name"] {
            customers.columns.push(ColumnInfo { name: c.into(), data_type: "int".into(), nullable: true, primary_key: c == "id", default: None });
        }
        let cat = Catalog { default_schema: "dbo".into(), introspected_at: 1, schemas: vec![SchemaInfo { name: "dbo".into(), tables: vec![orders, customers], objects: vec![] }] };
        let errs = |sql: &str| inspect(sql, Some(&cat), Dialect::MsSql).into_iter().filter(|d| d.severity == Severity::Error).map(|d| (sql[d.range].to_string(), d.message)).collect::<Vec<_>>();
        assert_eq!(errs("SELECT x.name FROM customers c"), vec![("x".to_string(), "Unknown table or alias 'x'".to_string())]);
        assert_eq!(errs("SELECT o.total, z.id FROM orders o JOIN customers c ON c.id = o.customer_id"), vec![("z".to_string(), "Unknown table or alias 'z'".to_string())]);
        for ok in [
            "SELECT c.name FROM customers c",
            "SELECT customers.name FROM customers",
            "SELECT d.total FROM (SELECT total FROM orders) d",
            "SELECT d.total FROM (SELECT total FROM orders) AS d",
            "WITH big AS (SELECT * FROM orders) SELECT big.total FROM big",
            "SELECT j.value FROM orders o CROSS APPLY OPENJSON(o.total) j",
            "SELECT o.id FROM orders o WHERE EXISTS (SELECT 1 FROM customers c WHERE c.id = o.customer_id)",
            "SELECT inserted.id FROM orders",
            "SELECT dbo.orders.id FROM dbo.orders",
            "SELECT o.total.ToString() FROM orders o",
            "SELECT * FROM sys.objects",
            "CREATE VIEW v AS SELECT q.id FROM orders",
        ] {
            assert!(errs(ok).is_empty(), "{ok}: {:?}", errs(ok));
        }
    }

    #[test]
    fn missing_tables_and_routines() {
        use databug_core::{DbObject, SchemaInfo, TableInfo};
        let obj = |name: &str, kind: ObjectKind| DbObject { name: name.into(), kind, detail: String::new(), ident: String::new(), table: None };
        let cat = Catalog {
            default_schema: "dbo".into(),
            introspected_at: 1,
            schemas: vec![
                SchemaInfo { name: "dbo".into(), tables: vec![TableInfo::new(String::from("orders"), ObjectKind::Table)], objects: vec![obj("add_order", ObjectKind::Procedure), obj("fn_total", ObjectKind::Function)] },
                SchemaInfo { name: "sales".into(), tables: vec![], objects: vec![obj("close_month", ObjectKind::Procedure)] },
            ],
        };
        let errs = |sql: &str| inspect(sql, Some(&cat), Dialect::MsSql).into_iter().filter(|d| d.severity == Severity::Error).map(|d| (sql[d.range].to_string(), d.message)).collect::<Vec<_>>();
        assert_eq!(errs("SELECT * FROM nope"), vec![("nope".to_string(), "Unknown table 'nope'".to_string())]);
        assert_eq!(errs("EXEC dbo.missing_proc 1"), vec![("dbo.missing_proc".to_string(), "Unknown procedure 'missing_proc'".to_string())]);
        assert_eq!(errs("EXEC @rc = gone"), vec![("gone".to_string(), "Unknown procedure 'gone'".to_string())]);
        assert_eq!(errs("SELECT dbo.fn_nope(1) FROM orders"), vec![("dbo.fn_nope".to_string(), "Unknown function 'fn_nope'".to_string())]);
        // all of these exist, are built in, or can't be checked
        for ok in [
            "EXEC dbo.add_order 1",
            "EXEC add_order",
            "EXECUTE sales.close_month",
            "EXEC sp_rename 'a', 'b'",
            "EXEC other_db.dbo.anything",
            "EXEC nosuchschema.p",
            "EXEC ('SELECT 1')",
            "SELECT dbo.fn_total(1), COUNT(*), UPPER('a') FROM orders",
        ] {
            assert!(errs(ok).is_empty(), "{ok}: {:?}", errs(ok));
        }
        // an engine whose catalog lists no routines: no procedure checks
        let mut bare = cat.clone();
        for s in &mut bare.schemas {
            s.objects.clear();
        }
        assert!(inspect("CALL whatever()", Some(&bare), Dialect::Postgres).is_empty());
    }

    #[test]
    fn what_changes_the_schema() {
        let yes = |sql: &str, d: Dialect| assert!(changes_schema(sql, d), "{sql}");
        let no = |sql: &str, d: Dialect| assert!(!changes_schema(sql, d), "{sql}");
        yes("CREATE OR ALTER PROCEDURE dbo.p AS SELECT 1", Dialect::MsSql);
        yes("DROP TABLE t", Dialect::Postgres);
        yes("ALTER TABLE t ADD c int", Dialect::MySql);
        yes("SELECT * INTO dbo.copy FROM dbo.t", Dialect::MsSql);
        yes("EXEC sp_rename 'dbo.a', 'b'", Dialect::MsSql);
        yes("DO $$ BEGIN EXECUTE 'CREATE TABLE x (a int)'; END $$", Dialect::Postgres);
        yes("SET k v", Dialect::Redis);
        yes("DEL k", Dialect::Redis);
        yes("db.orders.insertOne({a: 1})", Dialect::Mongo);
        yes("db.orders.drop()", Dialect::Mongo);
        no("SELECT * FROM t", Dialect::MsSql);
        no("INSERT INTO t VALUES (1)", Dialect::Postgres);
        no("UPDATE t SET a = 1", Dialect::MsSql);
        no("DELETE FROM t", Dialect::MySql);
        no("BEGIN TRANSACTION", Dialect::MsSql);
        no("GET k", Dialect::Redis);
        no("db.orders.find({})", Dialect::Mongo);
    }

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
        assert_eq!(classify("BEGIN", Dialect::Postgres), StatementKind::Transaction);
        assert_eq!(classify("BEGIN TRANSACTION", Dialect::MsSql), StatementKind::Transaction);
        assert_eq!(classify("BEGIN\n  UPDATE t SET a = 1;\nEND", Dialect::Postgres), StatementKind::Other);
        assert_eq!(classify("BEGIN TRY SELECT 1; END TRY BEGIN CATCH END CATCH", Dialect::MsSql), StatementKind::Other);
    }

    #[test]
    fn editable_source() {
        assert!(single_table_source("select * from t where a = 1", Dialect::Postgres).is_some());
        assert!(single_table_source("select * from t join u on true", Dialect::Postgres).is_none());
        assert!(single_table_source("select count(*) from t group by a", Dialect::Postgres).is_none());
    }

    #[test]
    fn names_under_the_cursor() {
        let src = "EXEC [etl].[load] @id = 1; SELECT dbo.fn_total(3), x FROM db.public.\"Orders\"";
        assert_eq!(name_at(src, 8, Dialect::MsSql), Some((Some("etl".into()), "load".into())));
        assert_eq!(name_at(src, src.find("fn_total").unwrap() + 2, Dialect::MsSql), Some((Some("dbo".into()), "fn_total".into())));
        assert_eq!(name_at(src, src.find("dbo").unwrap(), Dialect::MsSql), Some((Some("dbo".into()), "fn_total".into())));
        assert_eq!(name_at(src, src.len() - 2, Dialect::Postgres), Some((Some("public".into()), "Orders".into())));
        assert_eq!(name_at(src, src.find(", x").unwrap() + 2, Dialect::MsSql), Some((None, "x".into())));
        assert_eq!(name_at(src, src.find("= 1").unwrap() + 2, Dialect::MsSql), None);
    }
}
