//! Schema-aware completion. Works on broken, half-typed SQL: it looks at the
//! tokens before the cursor to decide the context, and at the whole statement
//! (including text after the cursor) to resolve tables and aliases.

use crate::analyze::{significant, table_refs, unquote, TableRef};
use crate::lexer::{tokenize, TokKind, Token};
use crate::mongo;
use crate::split::statement_at;
use databug_core::{Catalog, Dialect, ObjectKind, TableInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ItemKind {
    Join,
    Column,
    Alias,
    Table,
    View,
    Schema,
    Function,
    Keyword,
    Method,
    Operator,
}

#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub label: String,
    pub insert: String,
    pub detail: String,
    pub kind: ItemKind,
    score: i32,
}

#[derive(Debug, Clone, Default)]
pub struct Completion {
    pub items: Vec<CompletionItem>,
    /// Byte range in the source replaced when an item is accepted.
    pub replace: std::ops::Range<usize>,
    pub prefix: String,
}

const STATEMENT_STARTERS: &[&str] = &[
    "SELECT", "INSERT INTO", "UPDATE", "DELETE FROM", "WITH", "CREATE TABLE", "CREATE INDEX",
    "CREATE VIEW", "ALTER TABLE", "DROP TABLE", "TRUNCATE TABLE", "EXPLAIN", "BEGIN", "COMMIT",
    "ROLLBACK",
];

const CLAUSE_KEYWORDS: &[&str] = &[
    "FROM", "WHERE", "JOIN", "LEFT JOIN", "INNER JOIN", "ON", "AND", "OR", "NOT", "IN", "IS NULL",
    "IS NOT NULL", "LIKE", "BETWEEN", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", "OFFSET", "AS",
    "DISTINCT", "CASE", "WHEN", "THEN", "ELSE", "END", "UNION ALL", "EXISTS", "ASC", "DESC",
    "VALUES", "SET", "RETURNING", "NULL", "TRUE", "FALSE",
];

const FUNCTIONS: &[&str] = &[
    "COUNT", "SUM", "AVG", "MIN", "MAX", "COALESCE", "NULLIF", "CAST", "LOWER", "UPPER", "LENGTH",
    "SUBSTRING", "TRIM", "ROUND", "ABS", "CONCAT", "REPLACE", "EXTRACT", "ROW_NUMBER", "RANK",
    "DENSE_RANK", "LAG", "LEAD", "NOW", "CURRENT_TIMESTAMP",
];

fn dialect_functions(d: Dialect) -> &'static [&'static str] {
    match d {
        Dialect::Postgres => &["DATE_TRUNC", "STRING_AGG", "ARRAY_AGG", "JSON_AGG", "JSONB_BUILD_OBJECT", "GENERATE_SERIES", "TO_CHAR", "AGE"],
        Dialect::MySql => &["IFNULL", "GROUP_CONCAT", "DATE_FORMAT", "DATEDIFF", "JSON_EXTRACT", "CURDATE"],
        Dialect::MsSql => &["GETDATE", "ISNULL", "DATEADD", "DATEDIFF", "FORMAT", "STRING_AGG", "TRY_CAST", "SYSDATETIME"],
        Dialect::Sqlite => &["IFNULL", "GROUP_CONCAT", "DATE", "DATETIME", "STRFTIME", "JSON_EXTRACT"],
        Dialect::Mongo => &[],
    }
}

/// Score `label` against the typed prefix: prefix > word-start > subsequence.
fn score(label: &str, prefix: &str) -> Option<i32> {
    if prefix.is_empty() {
        return Some(0);
    }
    let l = label.to_ascii_lowercase();
    let p = prefix.to_ascii_lowercase();
    if l == p {
        return Some(1000);
    }
    if l.starts_with(&p) {
        return Some(900 - (l.len() as i32 - p.len() as i32).min(100));
    }
    if l.split(['_', '.', ' ']).any(|w| w.starts_with(&p)) {
        return Some(600);
    }
    // subsequence (fuzzy): "cid" matches "customer_id"
    let mut it = l.chars();
    if p.chars().all(|c| it.any(|x| x == c)) && l.starts_with(p.chars().next()?) {
        return Some(300);
    }
    None
}

struct Builder<'a> {
    prefix: &'a str,
    items: Vec<CompletionItem>,
}

impl Builder<'_> {
    fn add(&mut self, label: impl Into<String>, insert: impl Into<String>, detail: impl Into<String>, kind: ItemKind, bonus: i32) {
        let label = label.into();
        if let Some(s) = score(&label, self.prefix) {
            if self.items.iter().any(|i| i.label == label && i.kind == kind) {
                return;
            }
            self.items.push(CompletionItem { label, insert: insert.into(), detail: detail.into(), kind, score: s + bonus });
        }
    }
}

fn table_kind(t: &TableInfo) -> ItemKind {
    if t.kind == ObjectKind::View {
        ItemKind::View
    } else {
        ItemKind::Table
    }
}

pub fn complete(src: &str, cursor: usize, catalog: Option<&Catalog>, dialect: Dialect) -> Completion {
    let cursor = cursor.min(src.len());
    if dialect == Dialect::Mongo {
        return complete_mongo(src, cursor, catalog);
    }
    let range = statement_at(src, cursor, dialect).filter(|r| r.start <= cursor).unwrap_or(cursor..cursor);
    let stmt_start = range.start.min(cursor);
    let stmt_end = range.end.max(cursor);
    let stmt = &src[stmt_start..stmt_end];
    let rel = cursor - stmt_start;

    // word being typed
    let word_start = stmt[..rel]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == '$' || *c == '"' || *c == '`' || *c == '[')
        .last()
        .map(|(i, _)| i)
        .unwrap_or(rel);
    let word_end = rel + stmt[rel..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').map(|c| c.len_utf8()).sum::<usize>();
    let prefix_raw = &stmt[word_start..rel];
    let prefix = prefix_raw.trim_start_matches(['"', '`', '[']);

    let empty = Completion { items: vec![], replace: cursor..cursor, prefix: prefix.to_string() };
    // inside a string or comment: no completion
    let toks = tokenize(stmt, dialect);
    for t in &toks {
        if matches!(t.kind, TokKind::String | TokKind::Comment) && t.start < rel && rel <= t.end {
            let txt = t.text(stmt);
            let closed = if t.kind == TokKind::Comment {
                txt.starts_with("/*") && txt.len() >= 4 && txt.ends_with("*/")
            } else {
                txt.len() >= 2 && txt.ends_with(['\'', '"', '$'])
            };
            if rel < t.end || !closed {
                return empty;
            }
        }
    }

    let before: Vec<Token> = significant(&toks).into_iter().filter(|t| t.end <= word_start).collect();
    let refs = table_refs(stmt, dialect);
    let mut b = Builder { prefix, items: Vec::new() };

    let last = before.last().copied();
    let quote = |name: &str| dialect.quote_ident(name);

    // qualifier.<word>
    if let Some(dot) = last.filter(|t| t.is_punct(stmt, ".")) {
        let q_tok = before.len().checked_sub(2).and_then(|i| before.get(i)).filter(|t| t.is_name() || t.kind == TokKind::Keyword);
        if let (Some(q_tok), Some(cat)) = (q_tok, catalog) {
            let q = unquote(q_tok.text(stmt));
            // schema qualifier before the qualifier? (schema.table.col)
            let q2 = before.len().checked_sub(4).and_then(|i| {
                let d = before.get(i + 1)?;
                let s = before.get(i)?;
                (d.is_punct(stmt, ".") && s.is_name()).then(|| unquote(s.text(stmt)))
            });
            let _ = dot;
            if let Some(r) = refs.iter().find(|r| r.matches_qualifier(&q)) {
                if let Some(t) = cat.find_table(r.schema.as_deref(), &r.name) {
                    add_columns(&mut b, t, None, 50, &quote);
                }
            } else if let Some(t) = cat.find_table(q2.as_deref(), &q) {
                add_columns(&mut b, t, None, 50, &quote);
            }
            if let Some(s) = cat.schema(&q) {
                for t in &s.tables {
                    b.add(&t.name, quote(&t.name), format!("{} · {} cols", s.name, t.columns.len()), table_kind(t), 40);
                }
            }
        }
        return finish(b, stmt_start + word_start..stmt_start + word_end, prefix);
    }

    // nearest clause keyword before the cursor (at paren depth 0 relative to cursor)
    let mut depth = 0;
    let mut clause: Option<String> = None;
    for t in before.iter().rev() {
        let s = t.text(stmt);
        if s == ")" {
            depth += 1;
        } else if s == "(" {
            if depth == 0 {
                // inside parens: e.g. IN (, VALUES (, function args
                if clause.is_none() {
                    clause = Some("(".into());
                }
                break;
            }
            depth -= 1;
        } else if depth == 0 && t.kind == TokKind::Keyword {
            clause = Some(s.to_ascii_uppercase());
            break;
        }
    }
    let prev_is_clause_kw = last.map(|t| t.kind == TokKind::Keyword).unwrap_or(false);
    let prev_is_comma = last.map(|t| t.is_punct(stmt, ",")).unwrap_or(false);

    if before.is_empty() {
        for (i, kw) in STATEMENT_STARTERS.iter().enumerate() {
            b.add(*kw, *kw, "statement", ItemKind::Keyword, 20 - i as i32);
        }
        return finish(b, stmt_start + word_start..stmt_start + word_end, prefix);
    }

    let table_context = matches!(clause.as_deref(), Some("FROM" | "JOIN" | "UPDATE" | "INTO" | "TABLE" | "DESCRIBE"))
        && (prev_is_clause_kw || (prev_is_comma && clause.as_deref() == Some("FROM")));

    if table_context {
        if let Some(cat) = catalog {
            let join_ctx = clause.as_deref() == Some("JOIN");
            for (s, t) in cat.tables() {
                let in_default = s.name.eq_ignore_ascii_case(&cat.default_schema) || cat.schemas.len() == 1;
                let insert = if in_default { quote(&t.name) } else { format!("{}.{}", quote(&s.name), quote(&t.name)) };
                let mut bonus = if in_default { 30 } else { 0 };
                if join_ctx && related_to_refs(t, &refs, cat) {
                    bonus += 80;
                }
                b.add(&t.name, insert, format!("{} · {} cols", s.name, t.columns.len()), table_kind(t), bonus);
            }
            for s in &cat.schemas {
                b.add(&s.name, quote(&s.name), "schema", ItemKind::Schema, 0);
            }
        }
        return finish(b, stmt_start + word_start..stmt_start + word_end, prefix);
    }

    // after ON: suggest join conditions
    if last.map(|t| t.is_kw(stmt, "ON")).unwrap_or(false) {
        if let Some(cat) = catalog {
            for cond in join_conditions(&refs, cat, dialect) {
                b.add(cond.clone(), cond, "join condition", ItemKind::Join, 200);
            }
        }
    }

    // general expression context: columns of referenced tables
    if let Some(cat) = catalog {
        let multi = refs.iter().filter(|r| !r.is_cte).count() > 1;
        for r in refs.iter().filter(|r| !r.is_cte) {
            if let Some(t) = cat.find_table(r.schema.as_deref(), &r.name) {
                let qual = if multi { Some(r.alias.clone().unwrap_or_else(|| r.name.clone())) } else { None };
                add_columns(&mut b, t, qual.as_deref(), 60, &quote);
            }
            if let Some(a) = &r.alias {
                b.add(a, a.clone(), format!("alias for {}", r.name), ItemKind::Alias, 40);
            }
        }
        if refs.is_empty() && clause.as_deref() == Some("SELECT") {
            // SELECT before FROM: nothing to resolve yet, offer `*` and tables for `t.`
        }
    }
    if clause.as_deref() == Some("SELECT") && prefix.is_empty() {
        b.add("*", "*", "all columns", ItemKind::Operator, 10);
    }
    for f in FUNCTIONS.iter().chain(dialect_functions(dialect)) {
        b.add(*f, format!("{f}("), "function", ItemKind::Function, 0);
    }
    for kw in CLAUSE_KEYWORDS {
        b.add(*kw, *kw, "keyword", ItemKind::Keyword, 0);
    }
    finish(b, stmt_start + word_start..stmt_start + word_end, prefix)
}

fn add_columns(b: &mut Builder, t: &TableInfo, qualifier: Option<&str>, bonus: i32, quote: &dyn Fn(&str) -> String) {
    for c in &t.columns {
        let detail = format!("{}{} · {}", c.data_type, if c.primary_key { " PK" } else { "" }, t.name);
        let insert = match qualifier {
            Some(q) => format!("{}.{}", quote(q), quote(&c.name)),
            None => quote(&c.name),
        };
        b.add(&c.name, insert, detail, ItemKind::Column, bonus);
    }
}

fn singular(name: &str) -> String {
    let l = name.to_ascii_lowercase();
    if let Some(s) = l.strip_suffix("ies") {
        format!("{s}y")
    } else if let Some(s) = l.strip_suffix("ses") {
        format!("{s}s")
    } else if let Some(s) = l.strip_suffix('s') {
        s.to_string()
    } else {
        l
    }
}

fn related_to_refs(t: &TableInfo, refs: &[TableRef], cat: &Catalog) -> bool {
    refs.iter().filter_map(|r| cat.find_table(r.schema.as_deref(), &r.name)).any(|o| {
        fk_pair(o, t).is_some()
            || fk_pair(t, o).is_some()
            || o.foreign_keys.iter().any(|f| f.ref_table.eq_ignore_ascii_case(&t.name))
            || t.foreign_keys.iter().any(|f| f.ref_table.eq_ignore_ascii_case(&o.name))
    })
}

/// `child.<parent>_id` pointing at `parent.id`, by naming convention.
fn fk_pair<'a>(child: &'a TableInfo, parent: &'a TableInfo) -> Option<(&'a str, &'a str)> {
    let pk = parent.columns.iter().find(|c| c.primary_key).or_else(|| parent.column("id"))?;
    let want = format!("{}_{}", singular(&parent.name), pk.name.to_ascii_lowercase());
    let want2 = format!("{}{}", singular(&parent.name), pk.name.to_ascii_lowercase());
    child
        .columns
        .iter()
        .find(|c| {
            let n = c.name.to_ascii_lowercase();
            n == want || n == want2
        })
        .map(|c| (c.name.as_str(), pk.name.as_str()))
}

fn join_conditions(refs: &[TableRef], cat: &Catalog, dialect: Dialect) -> Vec<String> {
    let real: Vec<&TableRef> = refs.iter().filter(|r| !r.is_cte).collect();
    let Some(new) = real.last() else { return vec![] };
    let Some(nt) = cat.find_table(new.schema.as_deref(), &new.name) else { return vec![] };
    let q = |r: &TableRef| dialect.quote_ident(r.alias.as_deref().unwrap_or(&r.name));
    let mut out = Vec::new();
    for other in &real[..real.len() - 1] {
        let Some(ot) = cat.find_table(other.schema.as_deref(), &other.name) else { continue };
        // declared foreign keys, in both directions
        for (child, child_ref, parent, parent_ref) in [(ot, other, nt, new), (nt, new, ot, other)] {
            for fk in child.foreign_keys.iter().filter(|f| f.ref_table.eq_ignore_ascii_case(&parent.name)) {
                let conds: Vec<String> = fk
                    .columns
                    .iter()
                    .zip(&fk.ref_columns)
                    .map(|(c, r)| {
                        let (l, rr) = if std::ptr::eq(*parent_ref, *new) { ((parent_ref, r), (child_ref, c)) } else { ((child_ref, c), (parent_ref, r)) };
                        format!("{}.{} = {}.{}", q(l.0), dialect.quote_ident(l.1), q(rr.0), dialect.quote_ident(rr.1))
                    })
                    .collect();
                if !conds.is_empty() {
                    out.push(conds.join(" AND "));
                }
            }
        }
        if !out.is_empty() {
            continue;
        }
        if let Some((fk, pk)) = fk_pair(ot, nt) {
            out.push(format!("{}.{} = {}.{}", q(new), dialect.quote_ident(pk), q(other), dialect.quote_ident(fk)));
        }
        if let Some((fk, pk)) = fk_pair(nt, ot) {
            out.push(format!("{}.{} = {}.{}", q(new), dialect.quote_ident(fk), q(other), dialect.quote_ident(pk)));
        }
    }
    out
}

fn finish(mut b: Builder, replace: std::ops::Range<usize>, prefix: &str) -> Completion {
    b.items.sort_by(|a, c| c.score.cmp(&a.score).then(a.kind.cmp(&c.kind)).then(a.label.len().cmp(&c.label.len())).then(a.label.cmp(&c.label)));
    b.items.truncate(200);
    // nothing useful to add if the only item is exactly what's typed
    if b.items.len() == 1 && b.items[0].insert.eq_ignore_ascii_case(prefix) {
        b.items.clear();
    }
    Completion { items: b.items, replace, prefix: prefix.to_string() }
}

fn complete_mongo(src: &str, cursor: usize, catalog: Option<&Catalog>) -> Completion {
    let before = &src[..cursor];
    let word_start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == '$')
        .last()
        .map(|(i, _)| i)
        .unwrap_or(cursor);
    let prefix = &src[word_start..cursor];
    let word_end = cursor + src[cursor..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').map(|c| c.len_utf8()).sum::<usize>();
    let mut b = Builder { prefix, items: Vec::new() };
    let head = before[..word_start].trim_end();
    let line_start = head.rfind(['\n', ';']).map(|p| p + 1).unwrap_or(0);
    let stmt = &head[line_start..];

    let collections: Vec<&TableInfo> = catalog.map(|c| c.tables().map(|(_, t)| t).collect()).unwrap_or_default();
    let open = stmt.matches('(').count() > stmt.matches(')').count();

    if !open && stmt.trim() == "db." {
        for t in &collections {
            b.add(&t.name, &t.name, "collection", ItemKind::Table, 50);
        }
        b.add("getCollection", "getCollection(\"", "db method", ItemKind::Method, 0);
        b.add("runCommand", "runCommand({", "db method", ItemKind::Method, 0);
    } else if !open && stmt.ends_with('.') {
        let chained = stmt.ends_with(").");
        if chained {
            for m in mongo::CURSOR_METHODS {
                b.add(*m, format!("{m}("), "cursor", ItemKind::Method, 20);
            }
        } else {
            for (m, sig) in mongo::COLLECTION_METHODS {
                b.add(*m, format!("{m}("), *sig, ItemKind::Method, 20);
            }
        }
    } else if open || stmt.is_empty() {
        if stmt.is_empty() {
            b.add("db", "db.", "database", ItemKind::Keyword, 0);
            b.add("show collections", "show collections", "list collections", ItemKind::Keyword, 0);
        }
        // fields of the collection in this statement
        let coll = stmt.trim_start().strip_prefix("db.").and_then(|r| r.split(['.', '(']).next()).unwrap_or("");
        if let Some(t) = collections.iter().find(|t| t.name == coll) {
            for c in &t.columns {
                b.add(&c.name, &c.name, c.data_type.clone(), ItemKind::Column, 40);
            }
        }
        if prefix.starts_with('$') || open {
            for op in mongo::OPERATORS {
                if prefix.starts_with('$') {
                    b.add(*op, *op, "operator", ItemKind::Operator, 0);
                }
            }
        }
    }
    finish(b, word_start..word_end, prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use databug_core::{ColumnInfo, SchemaInfo};

    fn col(n: &str, pk: bool) -> ColumnInfo {
        ColumnInfo { name: n.into(), data_type: "int".into(), nullable: !pk, primary_key: pk, default: None }
    }

    fn cat() -> Catalog {
        Catalog {
            default_schema: "public".into(),
            introspected_at: 0,
            schemas: vec![
                SchemaInfo {
                    name: "public".into(),
                    objects: vec![],
                    tables: vec![
                        TableInfo { name: "orders".into(), kind: ObjectKind::Table, columns: vec![col("id", true), col("customer_id", false), col("total", false), col("currency", false)], indexes: vec![], foreign_keys: vec![] },
                        TableInfo { name: "customers".into(), kind: ObjectKind::Table, columns: vec![col("id", true), col("name", false)], indexes: vec![], foreign_keys: vec![] },
                    ],
                },
                SchemaInfo { name: "audit".into(), objects: vec![], tables: vec![TableInfo { name: "log".into(), kind: ObjectKind::Table, columns: vec![col("id", true)], indexes: vec![], foreign_keys: vec![] }] },
            ],
        }
    }

    fn labels(src: &str) -> Vec<String> {
        let cursor = src.find('|').unwrap();
        let s = src.replace('|', "");
        complete(&s, cursor, Some(&cat()), Dialect::Postgres).items.into_iter().map(|i| i.label).collect()
    }

    #[test]
    fn alias_columns() {
        let l = labels("SELECT * FROM orders o WHERE o.cu|");
        assert_eq!(l[0], "currency");
        assert!(l.contains(&"customer_id".to_string()));
        assert!(!l.contains(&"name".to_string()));
    }

    #[test]
    fn alias_defined_after_cursor() {
        let l = labels("SELECT c.| FROM customers c");
        assert!(l.contains(&"name".to_string()));
    }

    #[test]
    fn tables_after_from() {
        let l = labels("SELECT * FROM or|");
        assert_eq!(l[0], "orders");
        let l = labels("SELECT * FROM audit.|");
        assert_eq!(l, vec!["log"]);
    }

    #[test]
    fn join_condition() {
        let s = "SELECT * FROM orders o JOIN customers c ON |";
        let cursor = s.find('|').unwrap();
        let s2 = s.replace('|', "");
        let c = complete(&s2, cursor, Some(&cat()), Dialect::Postgres);
        assert_eq!(c.items[0].label, "c.id = o.customer_id");
    }

    #[test]
    fn broken_sql_still_completes() {
        let l = labels("SELECT o.|, FROM orders o WHERE (");
        assert!(l.contains(&"total".to_string()));
    }

    #[test]
    fn fuzzy() {
        let l = labels("SELECT cid| FROM orders");
        assert_eq!(l[0], "customer_id");
    }

    #[test]
    fn mongo_methods() {
        let src = "db.orders.fi";
        let c = complete(src, src.len(), Some(&cat()), Dialect::Mongo);
        assert_eq!(c.items[0].label, "find");
    }
}
