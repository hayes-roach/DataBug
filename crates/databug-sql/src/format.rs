//! A simple, predictable SQL formatter: uppercase keywords, one clause per
//! line, one select-list item per line, AND/OR on their own lines.

use crate::lexer::{tokenize, TokKind};
use crate::split::split_statements;
use databug_core::Dialect;

const BREAK_BEFORE: &[&str] = &[
    "SELECT", "FROM", "WHERE", "GROUP", "ORDER", "HAVING", "LIMIT", "OFFSET", "UNION", "EXCEPT",
    "INTERSECT", "VALUES", "SET", "RETURNING", "JOIN", "LEFT", "RIGHT", "INNER", "FULL", "CROSS",
    "WINDOW",
];

pub fn format_sql(src: &str, dialect: Dialect) -> String {
    if dialect == Dialect::Mongo {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len() + 64);
    for (n, r) in split_statements(src, dialect).into_iter().enumerate() {
        if n > 0 {
            out.push_str(";\n\n");
        }
        format_one(&src[r], dialect, &mut out);
    }
    if !out.is_empty() {
        out.push(';');
        out.push('\n');
    }
    out
}

fn format_one(stmt: &str, dialect: Dialect, out: &mut String) {
    let toks: Vec<_> = tokenize(stmt, dialect).into_iter().filter(|t| t.kind != TokKind::Whitespace).collect();
    let mut depth: usize = 0;
    let mut clause = String::new();
    let mut prev_text = "";
    let mut prev_kind = TokKind::Whitespace;
    let mut line_start = true;
    let indent = |out: &mut String, d: usize| {
        out.push('\n');
        for _ in 0..d {
            out.push_str("    ");
        }
    };

    for (i, t) in toks.iter().enumerate() {
        let raw = t.text(stmt);
        let up = raw.to_ascii_uppercase();
        let is_kw = t.kind == TokKind::Keyword;
        let word = if is_kw { up.as_str() } else { raw };

        let joins_prev = matches!(prev_text.to_ascii_uppercase().as_str(), "LEFT" | "RIGHT" | "INNER" | "FULL" | "CROSS" | "OUTER" | "NATURAL" | "UNION" | "GROUP" | "ORDER" | "PARTITION");
        let breaks = is_kw && BREAK_BEFORE.contains(&word) && !joins_prev && !(word == "SET" && clause != "UPDATE");
        let base = depth;

        if breaks && out.len() > 0 && !line_start {
            indent(out, base);
        } else if is_kw && (word == "AND" || word == "OR") && matches!(clause.as_str(), "WHERE" | "HAVING" | "ON") {
            indent(out, base + 1);
        } else if is_kw && word == "ON" {
            indent(out, base + 1);
        } else if !line_start {
            let no_space_before = matches!(raw, "," | ")" | "." | ";") || prev_text == "." || prev_text == "(" || (raw == "(" && matches!(prev_kind, TokKind::Ident | TokKind::QuotedIdent));
            if !no_space_before {
                out.push(' ');
            }
        }
        out.push_str(word);
        line_start = false;

        if is_kw && matches!(word, "SELECT" | "FROM" | "WHERE" | "SET" | "HAVING" | "UPDATE" | "ON" | "BY" | "VALUES") {
            clause = word.to_string();
            if matches!(word, "SELECT" | "BY" | "SET") {
                let next_is_distinct = toks.get(i + 1).map(|n| n.text(stmt).eq_ignore_ascii_case("DISTINCT")).unwrap_or(false);
                if !next_is_distinct {
                    indent(out, base + 1);
                    line_start = true;
                }
            }
        }
        if raw == "(" {
            depth += 1;
        } else if raw == ")" {
            depth = depth.saturating_sub(1);
        } else if raw == "," && matches!(clause.as_str(), "SELECT" | "BY" | "SET") && depth == base_depth(&toks[..=i], stmt) {
            indent(out, depth + 1);
            line_start = true;
        }
        prev_text = raw;
        prev_kind = t.kind;
    }
}

/// Paren depth at the most recent SELECT/BY/SET before position `i`.
fn base_depth(toks: &[crate::lexer::Token], stmt: &str) -> usize {
    let mut depth: i32 = 0;
    let mut at_clause = 0;
    for t in toks {
        match t.text(stmt) {
            "(" => depth += 1,
            ")" => depth -= 1,
            s if t.kind == TokKind::Keyword && (s.eq_ignore_ascii_case("SELECT") || s.eq_ignore_ascii_case("BY") || s.eq_ignore_ascii_case("SET")) => at_clause = depth,
            _ => {}
        }
    }
    at_clause.max(0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_basic_query() {
        let f = format_sql("select a, count(b) from t join u on t.id = u.tid and u.x > 1 where a = 1 group by a", Dialect::Postgres);
        assert_eq!(
            f,
            "SELECT\n    a,\n    count(b)\nFROM t\nJOIN u\n    ON t.id = u.tid\n    AND u.x > 1\nWHERE a = 1\nGROUP BY\n    a;\n"
        );
    }
}
