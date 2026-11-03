//! Splitting scripts into statements and finding the statement under the cursor.

use crate::lexer::{tokenize, TokKind, Token};
use std::ops::Range;
use databug_core::Dialect;

/// Byte ranges of each non-empty statement, trimmed of surrounding trivia and
/// the terminating `;`.
pub fn split_statements(src: &str, dialect: Dialect) -> Vec<Range<usize>> {
    if dialect == Dialect::Mongo {
        return split_mongo(src);
    }
    let toks = tokenize(src, dialect);
    let mut out = Vec::new();
    let mut first: Option<usize> = None;
    let mut last_end = 0usize;
    let mut block_depth = 0i32;
    let mut prev_sig: Option<Token> = None;

    let flush = |out: &mut Vec<Range<usize>>, first: &mut Option<usize>, last_end: usize| {
        if let Some(s) = first.take() {
            out.push(s..last_end);
        }
    };

    for (idx, t) in toks.iter().enumerate() {
        if t.is_trivia() {
            continue;
        }
        let text = t.text(src);
        // `GO` on its own line ends a batch in SQL Server.
        if dialect == Dialect::MsSql && t.kind == TokKind::Keyword && text.eq_ignore_ascii_case("GO") && alone_on_line(src, t) {
            flush(&mut out, &mut first, last_end);
            block_depth = 0;
            prev_sig = None;
            continue;
        }
        if t.kind == TokKind::Punct && text == ";" && block_depth <= 0 {
            flush(&mut out, &mut first, last_end);
            prev_sig = None;
            continue;
        }
        if t.kind == TokKind::Keyword {
            if text.eq_ignore_ascii_case("CASE") {
                block_depth += 1;
            } else if text.eq_ignore_ascii_case("BEGIN") && dialect != Dialect::Postgres {
                // BEGIN starts a block unless it starts a transaction.
                let next = toks[idx + 1..].iter().find(|t| !t.is_trivia());
                let is_tx = match next {
                    None => true,
                    Some(n) => {
                        let nt = n.text(src);
                        n.is_punct(src, ";")
                            || nt.eq_ignore_ascii_case("TRAN")
                            || nt.eq_ignore_ascii_case("TRANSACTION")
                            || nt.eq_ignore_ascii_case("WORK")
                            || nt.eq_ignore_ascii_case("DEFERRED")
                            || nt.eq_ignore_ascii_case("IMMEDIATE")
                            || nt.eq_ignore_ascii_case("EXCLUSIVE")
                    }
                };
                let at_start = prev_sig.is_none();
                if !(is_tx && at_start) {
                    block_depth += 1;
                }
            } else if text.eq_ignore_ascii_case("END") {
                // `END IF`, `END LOOP`, `END WHILE`, `END REPEAT` close blocks we never
                // counted (IF/LOOP don't open one), so they leave the depth alone.
                let next = toks[idx + 1..].iter().find(|t| !t.is_trivia()).map(|n| n.text(src).to_ascii_uppercase());
                if !matches!(next.as_deref(), Some("IF" | "LOOP" | "WHILE" | "REPEAT")) {
                    block_depth -= 1;
                }
            } else if text.eq_ignore_ascii_case("CASE") {
                // handled above
            }
        }
        if first.is_none() {
            first = Some(t.start);
        }
        last_end = t.end;
        prev_sig = Some(*t);
    }
    flush(&mut out, &mut first, last_end);
    out
}

fn alone_on_line(src: &str, t: &Token) -> bool {
    let line_start = src[..t.start].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line_end = src[t.end..].find('\n').map(|p| t.end + p).unwrap_or(src.len());
    src[line_start..t.start].trim().is_empty() && src[t.end..line_end].trim().trim_end_matches(';').trim().is_empty()
}

/// Mongo shell scripts: split on `;` and on new lines that start a new `db.` call
/// at bracket depth zero.
fn split_mongo(src: &str) -> Vec<Range<usize>> {
    let toks = tokenize(src, Dialect::Mongo);
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut first: Option<usize> = None;
    let mut last_end = 0;
    let mut saw_newline = false;
    for t in &toks {
        if t.kind == TokKind::Whitespace {
            if t.text(src).contains('\n') {
                saw_newline = true;
            }
            continue;
        }
        if t.kind == TokKind::Comment {
            continue;
        }
        let text = t.text(src);
        if depth == 0 && text == ";" {
            if let Some(s) = first.take() {
                out.push(s..last_end);
            }
            continue;
        }
        if depth == 0 && saw_newline && !text.starts_with('.') {
            if let Some(s) = first.take() {
                out.push(s..last_end);
            }
        }
        saw_newline = false;
        match text {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => depth -= 1,
            _ => {}
        }
        if first.is_none() {
            first = Some(t.start);
        }
        last_end = t.end;
    }
    if let Some(s) = first {
        out.push(s..last_end);
    }
    out
}

/// The statement the cursor is in. A cursor in the blank space after a
/// statement picks that statement, the way DataGrip does.
pub fn statement_at(src: &str, cursor: usize, dialect: Dialect) -> Option<Range<usize>> {
    let stmts = split_statements(src, dialect);
    if let Some(r) = stmts.iter().find(|r| r.start <= cursor && cursor <= r.end) {
        return Some(r.clone());
    }
    // Nearest statement ending before the cursor on the same or previous lines,
    // unless a blank line separates them.
    let before = stmts.iter().rev().find(|r| r.end <= cursor)?;
    let gap = &src[before.end..cursor];
    let gap = gap.trim_start_matches(';');
    if gap.matches('\n').count() <= 1 {
        Some(before.clone())
    } else {
        stmts.iter().find(|r| r.start >= cursor).filter(|r| src[cursor..r.start].matches('\n').count() <= 1).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(src: &str, d: Dialect) -> Vec<&str> {
        split_statements(src, d).into_iter().map(|r| &src[r]).collect()
    }

    #[test]
    fn splits_on_semicolons_not_in_strings() {
        let p = parts("select ';' ; select 2;\n\n -- c\n", Dialect::Postgres);
        assert_eq!(p, vec!["select ';'", "select 2"]);
    }

    #[test]
    fn mssql_go_and_blocks() {
        let src = "CREATE PROCEDURE p AS BEGIN SELECT 1; SELECT 2; END\nGO\nSELECT 3";
        let p = parts(src, Dialect::MsSql);
        assert_eq!(p.len(), 2);
        assert!(p[0].ends_with("END"));
        let p = parts("BEGIN TRAN; UPDATE t SET a=1; COMMIT", Dialect::MsSql);
        assert_eq!(p.len(), 3);
    }

    #[test]
    fn mysql_procedure_with_if_blocks() {
        let src = "DROP PROCEDURE IF EXISTS p;\nCREATE PROCEDURE p(IN x INT)\nBEGIN\n  IF x > 1 THEN\n    SELECT 1;\n  ELSE\n    SELECT 2;\n  END IF;\n  WHILE x > 0 DO SET x = x - 1; END WHILE;\nEND;\nSELECT 3";
        let p = parts(src, Dialect::MySql);
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(p[1].ends_with("END"));
    }

    #[test]
    fn case_end_does_not_break() {
        let p = parts("select case when a then 1 else 2 end from t; select 1", Dialect::MySql);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn cursor_after_statement() {
        let src = "select 1;\nselect 2;  ";
        let r = statement_at(src, src.len(), Dialect::Postgres).unwrap();
        assert_eq!(&src[r], "select 2");
        let r = statement_at(src, 3, Dialect::Postgres).unwrap();
        assert_eq!(&src[r], "select 1");
    }

    #[test]
    fn mongo_lines() {
        let src = "db.a.find({x: 1})\n  .limit(5)\ndb.b.countDocuments({})";
        let p = parts(src, Dialect::Mongo);
        assert_eq!(p.len(), 2);
        assert!(p[0].ends_with(".limit(5)"));
    }
}
