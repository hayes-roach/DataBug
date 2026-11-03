//! Rename refactoring: work out what the identifier under the cursor refers
//! to (an alias, a table or a column) and rewrite every reference to it.

use crate::analyze::{significant, table_refs, unquote, TableRef};
use crate::lexer::{tokenize, TokKind, Token};
use crate::split::{split_statements, statement_at};
use databug_core::{Catalog, Dialect};
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameTarget {
    /// A table alias, local to one statement (byte range of that statement).
    Alias { statement: Range<usize>, alias: String },
    Table { schema: Option<String>, name: String },
    Column { schema: Option<String>, table: String, column: String },
}

impl RenameTarget {
    pub fn current_name(&self) -> &str {
        match self {
            RenameTarget::Alias { alias, .. } => alias,
            RenameTarget::Table { name, .. } => name,
            RenameTarget::Column { column, .. } => column,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            RenameTarget::Alias { alias, .. } => format!("alias {alias}"),
            RenameTarget::Table { name, .. } => format!("table {name}"),
            RenameTarget::Column { table, column, .. } => format!("column {table}.{column}"),
        }
    }
}

fn ref_for<'a>(refs: &'a [TableRef], q: &str) -> Option<&'a TableRef> {
    refs.iter().find(|r| r.matches_qualifier(q))
}

/// What the identifier at `cursor` refers to.
pub fn target_at(src: &str, cursor: usize, catalog: Option<&Catalog>, d: Dialect) -> Option<RenameTarget> {
    let range = statement_at(src, cursor, d)?;
    let stmt = &src[range.clone()];
    let rel = cursor.checked_sub(range.start)?;
    let toks = significant(&tokenize(stmt, d));
    let i = toks.iter().position(|t| t.start <= rel && rel <= t.end && t.is_name())?;
    let tok = toks[i];
    let word = unquote(tok.text(stmt));
    let refs = table_refs(stmt, d);
    let prev_dot = i >= 2 && toks[i - 1].is_punct(stmt, ".");
    let next_dot = toks.get(i + 1).map(|t| t.is_punct(stmt, ".")).unwrap_or(false);

    // the table name itself in FROM/JOIN/UPDATE/INTO
    if let Some(r) = refs.iter().find(|r| r.span.start == tok.start && !r.is_cte) {
        return Some(RenameTarget::Table { schema: r.schema.clone(), name: r.name.clone() });
    }
    // alias definition or alias used as a qualifier
    if let Some(r) = refs.iter().find(|r| r.alias.as_deref().map(|a| a.eq_ignore_ascii_case(&word)).unwrap_or(false)) {
        if !prev_dot {
            let _ = r;
            return Some(RenameTarget::Alias { statement: range, alias: word });
        }
    }
    if prev_dot {
        let q = unquote(toks[i - 2].text(stmt));
        if let Some(r) = ref_for(&refs, &q) {
            return Some(RenameTarget::Column { schema: r.schema.clone(), table: r.name.clone(), column: word });
        }
        // schema.table outside a FROM clause
        if let Some(t) = catalog.and_then(|c| c.find_table(Some(&q), &word)) {
            return Some(RenameTarget::Table { schema: Some(q), name: t.name.clone() });
        }
        return None;
    }
    if next_dot {
        if let Some(r) = refs.iter().find(|r| r.alias.is_none() && r.name.eq_ignore_ascii_case(&word)) {
            return Some(RenameTarget::Table { schema: r.schema.clone(), name: r.name.clone() });
        }
    }
    // unqualified column: the one referenced table that has it
    if let Some(cat) = catalog {
        let owners: Vec<&TableRef> = refs.iter().filter(|r| !r.is_cte && cat.find_table(r.schema.as_deref(), &r.name).map(|t| t.column(&word).is_some()).unwrap_or(false)).collect();
        if owners.len() == 1 {
            return Some(RenameTarget::Column { schema: owners[0].schema.clone(), table: owners[0].name.clone(), column: word });
        }
        if let Some(t) = cat.find_table(None, &word) {
            return Some(RenameTarget::Table { schema: None, name: t.name.clone() });
        }
    } else if refs.len() == 1 && tok.kind == TokKind::Ident {
        return Some(RenameTarget::Column { schema: refs[0].schema.clone(), table: refs[0].name.clone(), column: word });
    }
    None
}

/// Rewrite references to `target` in `src`. Returns the new text and how
/// many identifiers changed. Statements that don't involve the table are
/// left alone.
pub fn rename_references(src: &str, d: Dialect, target: &RenameTarget, new_name: &str, catalog: Option<&Catalog>) -> (String, usize) {
    let quoted = d.quote_ident(new_name);
    let mut edits: Vec<Range<usize>> = Vec::new();
    let ranges = match target {
        RenameTarget::Alias { statement, .. } => vec![statement.clone()],
        _ => split_statements(src, d),
    };
    for range in ranges {
        let stmt = &src[range.clone()];
        let toks = significant(&tokenize(stmt, d));
        let refs = table_refs(stmt, d);
        let is = |t: &Token, w: &str| t.is_name() && unquote(t.text(stmt)).eq_ignore_ascii_case(w);
        let dot_before = |i: usize| i >= 1 && toks[i - 1].is_punct(stmt, ".");
        let dot_after = |i: usize| toks.get(i + 1).map(|t| t.is_punct(stmt, ".")).unwrap_or(false);
        let schema_matches = |r: &TableRef, s: &Option<String>| match (s, &r.schema) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => true,
        };
        match target {
            RenameTarget::Alias { alias, .. } => {
                for (i, t) in toks.iter().enumerate() {
                    if is(t, alias) && !dot_before(i) {
                        edits.push(range.start + t.start..range.start + t.end);
                    }
                }
            }
            RenameTarget::Table { schema, name } => {
                let hits: Vec<&TableRef> = refs.iter().filter(|r| !r.is_cte && r.name.eq_ignore_ascii_case(name) && schema_matches(r, schema)).collect();
                for r in &hits {
                    edits.push(range.start + r.span.start..range.start + r.span.end);
                }
                // qualifiers `name.column` where the table has no alias
                if hits.iter().any(|r| r.alias.is_none()) {
                    for (i, t) in toks.iter().enumerate() {
                        let is_ref_span = hits.iter().any(|r| r.span.start == t.start);
                        // `schema.name.col` → the name is the middle part; `name.col` → first part
                        if is(t, name) && dot_after(i) && !is_ref_span && toks.get(i + 2).map(|n| n.is_name()).unwrap_or(false) {
                            edits.push(range.start + t.start..range.start + t.end);
                        }
                    }
                }
            }
            RenameTarget::Column { schema, table, column } => {
                let mine: Vec<&TableRef> = refs.iter().filter(|r| !r.is_cte && r.name.eq_ignore_ascii_case(table) && schema_matches(r, schema)).collect();
                if mine.is_empty() {
                    continue;
                }
                let ref_spans: Vec<usize> = refs.iter().map(|r| r.span.start).collect();
                for (i, t) in toks.iter().enumerate() {
                    if !is(t, column) || ref_spans.contains(&t.start) {
                        continue;
                    }
                    if dot_before(i) {
                        let q = unquote(toks[i - 2].text(stmt));
                        if mine.iter().any(|r| r.matches_qualifier(&q)) {
                            edits.push(range.start + t.start..range.start + t.end);
                        }
                        continue;
                    }
                    if dot_after(i) || toks.get(i + 1).map(|n| n.is_punct(stmt, "(")).unwrap_or(false) {
                        continue;
                    }
                    // unqualified: only when no other table in the statement has this column
                    let others_have = refs.iter().filter(|r| !mine.iter().any(|m| std::ptr::eq(*m, *r))).any(|r| {
                        r.is_cte || catalog.and_then(|c| c.find_table(r.schema.as_deref(), &r.name)).map(|t| t.column(column).is_some()).unwrap_or(catalog.is_none())
                    });
                    // skip the alias in `AS column` of the SELECT list
                    let is_output_alias = i >= 1 && toks[i - 1].is_kw(stmt, "AS");
                    if !others_have && !is_output_alias {
                        edits.push(range.start + t.start..range.start + t.end);
                    }
                }
            }
        }
    }
    edits.sort_by_key(|r| r.start);
    edits.dedup();
    let mut out = src.to_string();
    for r in edits.iter().rev() {
        out.replace_range(r.clone(), &quoted);
    }
    (out, edits.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use databug_core::{ColumnInfo, ObjectKind, SchemaInfo, TableInfo};

    fn cat() -> Catalog {
        let t = |n: &str, cols: &[&str]| {
            let mut t = TableInfo::new(n, ObjectKind::Table);
            t.columns = cols.iter().map(|c| ColumnInfo { name: c.to_string(), data_type: "int".into(), nullable: true, primary_key: *c == "id", default: None }).collect();
            t
        };
        Catalog {
            default_schema: "public".into(),
            introspected_at: 0,
            schemas: vec![SchemaInfo { name: "public".into(), tables: vec![t("orders", &["id", "customer_id", "total"]), t("customers", &["id", "name"])], objects: vec![] }],
        }
    }

    fn at(src: &str) -> (String, usize) {
        let c = src.find('|').unwrap();
        (src.replace('|', ""), c)
    }

    #[test]
    fn alias() {
        let (s, c) = at("SELECT o|.total FROM orders o WHERE o.id = 1; SELECT o FROM x");
        let t = target_at(&s, c, Some(&cat()), Dialect::Postgres).unwrap();
        assert!(matches!(t, RenameTarget::Alias { .. }));
        let (out, n) = rename_references(&s, Dialect::Postgres, &t, "ord", None);
        assert_eq!(n, 3);
        assert_eq!(out, "SELECT ord.total FROM orders ord WHERE ord.id = 1; SELECT o FROM x");
    }

    #[test]
    fn column_through_aliases_and_unqualified() {
        let src = "SELECT o.total, c.name FROM orders o JOIN customers c ON c.id = o.customer_id;\nSELECT total FROM orders WHERE total > 5;\nSELECT total FROM other;";
        let (s, c) = at(&src.replacen("o.total", "o.to|tal", 1));
        let t = target_at(&s, c, Some(&cat()), Dialect::Postgres).unwrap();
        assert_eq!(t, RenameTarget::Column { schema: None, table: "orders".into(), column: "total".into() });
        let (out, n) = rename_references(&s, Dialect::Postgres, &t, "amount", Some(&cat()));
        assert_eq!(n, 3, "{out}");
        assert!(out.contains("SELECT o.amount, c.name"));
        assert!(out.contains("SELECT amount FROM orders WHERE amount > 5"));
        assert!(out.contains("SELECT total FROM other"), "other tables untouched");
    }

    #[test]
    fn table_with_qualifiers() {
        let (s, c) = at("SELECT orders.id FROM ord|ers WHERE orders.total > 1; SELECT * FROM customers");
        let t = target_at(&s, c, Some(&cat()), Dialect::Postgres).unwrap();
        assert_eq!(t, RenameTarget::Table { schema: None, name: "orders".into() });
        let (out, n) = rename_references(&s, Dialect::Postgres, &t, "Sales Orders", None);
        assert_eq!(n, 3);
        assert_eq!(out, "SELECT \"Sales Orders\".id FROM \"Sales Orders\" WHERE \"Sales Orders\".total > 1; SELECT * FROM customers");
    }

    #[test]
    fn rewrites_a_view_definition() {
        let view = "CREATE OR REPLACE VIEW v AS SELECT c.name, sum(o.total) AS total FROM customers c JOIN orders o ON o.customer_id = c.id GROUP BY c.name";
        let t = RenameTarget::Column { schema: None, table: "orders".into(), column: "total".into() };
        let (out, n) = rename_references(view, Dialect::Postgres, &t, "amount", Some(&cat()));
        assert_eq!(n, 1);
        assert!(out.contains("sum(o.amount) AS total"));
    }
}
