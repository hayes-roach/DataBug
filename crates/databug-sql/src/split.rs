//! Splitting scripts into statements and finding the statement under the cursor.

use crate::lexer::{tokenize, TokKind, Token};
use std::ops::Range;
use databug_core::Dialect;

/// Byte ranges of each non-empty statement, trimmed of surrounding trivia and
/// the terminating `;`.
///
/// On SQL Server a batch (the text between `GO` lines) runs as one statement
/// when it declares variables (`DECLARE @x …; SET @x = …; EXEC p @x`), since
/// variables only live for their batch, or when it starts with
/// `CREATE`/`ALTER` of a procedure, function, trigger or view, whose body is
/// the rest of the batch.
pub fn split_statements(src: &str, dialect: Dialect) -> Vec<Range<usize>> {
    if dialect == Dialect::Mongo {
        return split_mongo(src);
    }
    let parts = split_parts(src, dialect);
    if dialect != Dialect::MsSql {
        return parts.into_iter().map(|p| p.range).collect();
    }
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let batch = parts[i].batch;
        let end = parts[i..].iter().position(|p| p.batch != batch).map(|n| i + n).unwrap_or(parts.len());
        let group = &parts[i..end];
        let whole = group[0].starts == Start::Routine || group.iter().any(|p| p.starts == Start::Declare);
        if whole {
            out.push(group[0].range.start..group[group.len() - 1].range.end);
        } else {
            out.extend(group.iter().map(|p| p.range.clone()));
        }
        i = end;
    }
    out
}

/// What *Run* sends: on SQL Server, each batch between `GO` lines as one
/// request (as SQL Server Management Studio does), so variables, temp
/// tables and `SET` options apply to the whole batch; elsewhere, each statement.
pub fn split_batches(src: &str, dialect: Dialect) -> Vec<Range<usize>> {
    if dialect != Dialect::MsSql {
        return split_statements(src, dialect);
    }
    let parts = split_parts(src, dialect);
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut last_batch = usize::MAX;
    for p in parts {
        match out.last_mut() {
            Some(r) if p.batch == last_batch => r.end = p.range.end,
            _ => out.push(p.range.clone()),
        }
        last_batch = p.batch;
    }
    out
}

/// Each statement on its own, split at every `;` even inside a SQL Server
/// batch that runs as a whole. Used to check what a batch does.
pub fn statement_parts(src: &str, dialect: Dialect) -> Vec<Range<usize>> {
    if dialect == Dialect::Mongo {
        return split_mongo(src);
    }
    split_parts(src, dialect).into_iter().map(|p| p.range).collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Start {
    Declare,
    Routine,
    Other,
}

struct Part {
    range: Range<usize>,
    batch: usize,
    starts: Start,
}

fn split_parts(src: &str, dialect: Dialect) -> Vec<Part> {
    let toks = tokenize(src, dialect);
    let mut out: Vec<Part> = Vec::new();
    let mut batch = 0usize;
    let mut starts = Start::Other;
    let mut first: Option<usize> = None;
    let mut last_end = 0usize;
    let mut block_depth = 0i32;
    let mut prev_sig: Option<Token> = None;
    // PL/SQL and SQL PL (Oracle, Db2): semicolons inside a block's
    // declarations don't end the statement
    let mut hold = Hold::No;
    let plsql = dialect == Dialect::Postgres;

    let flush = |out: &mut Vec<Part>, first: &mut Option<usize>, last_end: usize, batch: usize, starts: Start| {
        if let Some(s) = first.take() {
            out.push(Part { range: s..last_end, batch, starts });
        }
    };

    for (idx, t) in toks.iter().enumerate() {
        if t.is_trivia() {
            continue;
        }
        let text = t.text(src);
        // `GO` on its own line ends a batch in SQL Server.
        if dialect == Dialect::MsSql && t.kind == TokKind::Keyword && text.eq_ignore_ascii_case("GO") && alone_on_line(src, t) {
            flush(&mut out, &mut first, last_end, batch, starts);
            batch += 1;
            block_depth = 0;
            prev_sig = None;
            continue;
        }
        // `/` on its own line ends a PL/SQL block or package (as in SQL*Plus)
        if plsql && text == "/" && alone_on_line(src, t) {
            flush(&mut out, &mut first, last_end, batch, starts);
            block_depth = 0;
            hold = Hold::No;
            prev_sig = None;
            continue;
        }
        if t.kind == TokKind::Punct && text == ";" && block_depth <= 0 && hold == Hold::No {
            flush(&mut out, &mut first, last_end, batch, starts);
            prev_sig = None;
            continue;
        }
        if plsql && first.is_none() {
            hold = plsql_hold(src, &toks[idx..]);
        } else if plsql && hold == Hold::No && block_depth == 0 && text.eq_ignore_ascii_case("DECLARE") {
            // a trigger's DECLARE section (`… FOR EACH ROW DECLARE v NUMBER; BEGIN …`)
            hold = Hold::UntilBegin;
        }
        if t.kind == TokKind::Keyword {
            if text.eq_ignore_ascii_case("CASE") {
                block_depth += 1;
            } else if text.eq_ignore_ascii_case("BEGIN") {
                // BEGIN starts a block unless it starts a transaction.
                let next = toks[idx + 1..].iter().find(|t| !t.is_trivia());
                let is_tx = match next {
                    None => true,
                    Some(n) => {
                        let nt = n.text(src).to_ascii_uppercase();
                        n.is_punct(src, ";") || matches!(nt.as_str(), "TRAN" | "TRANSACTION" | "WORK" | "DEFERRED" | "IMMEDIATE" | "EXCLUSIVE" | "ISOLATION" | "READ" | "DEFERRABLE" | "NOT")
                    }
                };
                let at_start = prev_sig.is_none();
                if !(is_tx && at_start) {
                    block_depth += 1;
                    if hold == Hold::UntilBegin {
                        hold = Hold::No;
                    }
                }
            } else if text.eq_ignore_ascii_case("END") {
                // `END IF`, `END LOOP`, `END WHILE`, `END REPEAT` close blocks we never
                // counted (IF/LOOP don't open one), so they leave the depth alone.
                let next = toks[idx + 1..].iter().find(|t| !t.is_trivia()).map(|n| n.text(src).to_ascii_uppercase());
                if !matches!(next.as_deref(), Some("IF" | "LOOP" | "WHILE" | "REPEAT")) && block_depth > 0 {
                    block_depth -= 1;
                }
            }
        }
        if first.is_none() {
            first = Some(t.start);
            starts = if dialect == Dialect::MsSql { statement_start(src, &toks[idx..]) } else { Start::Other };
        }
        last_end = t.end;
        prev_sig = Some(*t);
    }
    flush(&mut out, &mut first, last_end, batch, starts);
    out
}

/// How long a PL/SQL statement ignores `;` at its top level.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hold {
    No,
    /// declarations before the block's BEGIN (`DECLARE v NUMBER; BEGIN …`,
    /// `CREATE PROCEDURE p AS v NUMBER; BEGIN …`); the BEGIN … END depth takes over
    UntilBegin,
    /// packages and type bodies hold several routines: up to a `/` line
    UntilSlash,
}

/// For engines on the PostgreSQL dialect (PostgreSQL, Oracle, Db2, …): does
/// the statement starting at `toks[0]` open a PL/SQL-style block?
/// PostgreSQL's own `DECLARE c CURSOR FOR …` and `$$` / quoted function
/// bodies don't.
fn plsql_hold(src: &str, toks: &[Token]) -> Hold {
    let sig: Vec<&Token> = toks.iter().filter(|t| !t.is_trivia()).take(400).collect();
    let word = |i: usize| sig.get(i).map(|t| t.text(src).to_ascii_uppercase()).unwrap_or_default();
    match word(0).as_str() {
        "DECLARE" => {
            if word(1) == "CURSOR" {
                return Hold::UntilBegin;
            }
            // PostgreSQL: DECLARE name [options] CURSOR … FOR query
            let head: Vec<String> = (1..sig.len()).map(&word).take_while(|w| w != ";").collect();
            let pg_cursor = head.iter().any(|w| w == "CURSOR") && head.iter().any(|w| w == "FOR") && !head.iter().any(|w| w == ":=");
            if pg_cursor {
                Hold::No
            } else {
                Hold::UntilBegin
            }
        }
        "CREATE" => {
            let mut i = 1;
            if word(i) == "OR" {
                i += 2; // OR REPLACE
            }
            if matches!(word(i).as_str(), "EDITIONABLE" | "NONEDITIONABLE") {
                i += 1;
            }
            match word(i).as_str() {
                "PACKAGE" => Hold::UntilSlash,
                "TYPE" if word(i + 1) == "BODY" => Hold::UntilSlash,
                "PROCEDURE" | "FUNCTION" => {
                    // PL/SQL bodies follow IS/AS directly; PostgreSQL's are strings
                    let mut depth = 0;
                    for (k, t) in sig.iter().enumerate().skip(i) {
                        let w = t.text(src);
                        match w {
                            "(" => depth += 1,
                            ")" => depth -= 1,
                            ";" if depth == 0 => return Hold::No,
                            _ => {}
                        }
                        if depth == 0 && (w.eq_ignore_ascii_case("AS") || w.eq_ignore_ascii_case("IS")) {
                            let body_is_string = sig.get(k + 1).map(|n| n.kind == TokKind::String).unwrap_or(true);
                            return if body_is_string { Hold::No } else { Hold::UntilBegin };
                        }
                    }
                    Hold::No
                }
                _ => Hold::No,
            }
        }
        _ => Hold::No,
    }
}

/// What a SQL Server statement starting at `toks[0]` is, for batch grouping.
fn statement_start(src: &str, toks: &[Token]) -> Start {
    let mut words = toks.iter().filter(|t| !t.is_trivia()).map(|t| t.text(src).to_ascii_uppercase());
    match words.next().as_deref() {
        Some("DECLARE") => Start::Declare,
        Some("CREATE" | "ALTER") => {
            let mut w = words.next();
            if w.as_deref() == Some("OR") {
                words.next(); // ALTER
                w = words.next();
            }
            match w.as_deref() {
                Some("PROC" | "PROCEDURE" | "FUNCTION" | "TRIGGER" | "VIEW") => Start::Routine,
                _ => Start::Other,
            }
        }
        _ => Start::Other,
    }
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
    fn mssql_batch_with_variables_runs_whole() {
        let src = "DECLARE @id int;\nSET @id = 5;\nEXEC dbo.p @id = @id;\nSELECT @id;\nGO\nSELECT 1; SELECT 2";
        let p = parts(src, Dialect::MsSql);
        assert_eq!(p, vec!["DECLARE @id int;\nSET @id = 5;\nEXEC dbo.p @id = @id;\nSELECT @id", "SELECT 1", "SELECT 2"]);
        assert_eq!(statement_parts(src, Dialect::MsSql).len(), 6);
        // the statement under the cursor is the whole batch
        let r = statement_at(src, src.find("EXEC").unwrap(), Dialect::MsSql).unwrap();
        assert!(src[r].starts_with("DECLARE"));
        // other dialects still split
        assert_eq!(parts("SET a = 1; SELECT 2", Dialect::MySql).len(), 2);
    }

    #[test]
    fn mssql_routine_body_is_the_rest_of_the_batch() {
        let src = "CREATE OR ALTER PROCEDURE dbo.p AS\nSET NOCOUNT ON;\nSELECT 1;\nGO\nEXEC dbo.p;";
        let p = parts(src, Dialect::MsSql);
        assert_eq!(p, vec!["CREATE OR ALTER PROCEDURE dbo.p AS\nSET NOCOUNT ON;\nSELECT 1", "EXEC dbo.p"]);
        // a CREATE TABLE batch still splits
        assert_eq!(parts("CREATE TABLE t (a int); INSERT INTO t VALUES (1)", Dialect::MsSql).len(), 2);
    }

    #[test]
    fn mssql_run_sends_whole_batches() {
        let src = "\u{feff}SELECT 1;\r\nSELECT 2;\r\nGO\r\nSELECT 3;";
        let b: Vec<&str> = split_batches(src, Dialect::MsSql).into_iter().map(|r| &src[r]).collect();
        assert_eq!(b, vec!["SELECT 1;\r\nSELECT 2", "SELECT 3"]);
        // other engines still run statement by statement
        assert_eq!(split_batches("SELECT 1; SELECT 2", Dialect::Postgres).len(), 2);
        // a byte-order mark doesn't hide a leading DECLARE
        let bom = "\u{feff}DECLARE @x int = 1;\nSELECT @x;";
        assert_eq!(split_statements(bom, Dialect::MsSql).len(), 1);
    }

    #[test]
    fn plsql_blocks_stay_whole() {
        let ora = Dialect::Postgres;
        let src = "DECLARE\n  v NUMBER := 5;\nBEGIN\n  UPDATE t SET a = v;\n  COMMIT;\nEND;\n/\nSELECT 1 FROM dual;";
        assert_eq!(parts(src, ora), vec!["DECLARE\n  v NUMBER := 5;\nBEGIN\n  UPDATE t SET a = v;\n  COMMIT;\nEND", "SELECT 1 FROM dual"]);
        assert_eq!(parts("BEGIN\n  DBMS_OUTPUT.PUT_LINE('hi');\nEND;\nSELECT 2;", ora).len(), 2);
        let proc = "CREATE OR REPLACE PROCEDURE p AS\n  n NUMBER;\nBEGIN\n  SELECT 1 INTO n FROM dual;\nEND;\nSELECT 3 FROM dual;";
        assert_eq!(parts(proc, ora).len(), 2, "{:?}", parts(proc, ora));
        let pkg = "CREATE OR REPLACE PACKAGE BODY k AS\n  PROCEDURE a IS BEGIN NULL; END a;\n  PROCEDURE b IS BEGIN NULL; END b;\nEND k;\n/\nSELECT 4 FROM dual;";
        assert_eq!(parts(pkg, ora).len(), 2, "{:?}", parts(pkg, ora));
        let trg = "CREATE TRIGGER trg BEFORE INSERT ON t FOR EACH ROW\nDECLARE n NUMBER;\nBEGIN\n  n := 1;\nEND;\nSELECT 5 FROM dual;";
        assert_eq!(parts(trg, ora).len(), 2, "{:?}", parts(trg, ora));
    }

    #[test]
    fn postgres_transactions_cursors_and_functions_still_split() {
        let pg = Dialect::Postgres;
        assert_eq!(parts("BEGIN;\nUPDATE t SET a = 1;\nCOMMIT;", pg).len(), 3);
        assert_eq!(parts("BEGIN ISOLATION LEVEL SERIALIZABLE;\nSELECT 1;\nEND;", pg).len(), 3);
        assert_eq!(parts("DECLARE c CURSOR FOR SELECT * FROM t;\nFETCH 10 FROM c;\nCLOSE c;", pg).len(), 3);
        let f = "CREATE FUNCTION f() RETURNS int AS $$ BEGIN RETURN 1; END; $$ LANGUAGE plpgsql;\nSELECT f();";
        assert_eq!(parts(f, pg).len(), 2, "{:?}", parts(f, pg));
        let atomic = "CREATE FUNCTION g() RETURNS int LANGUAGE sql BEGIN ATOMIC SELECT 1; END;\nSELECT g();";
        assert_eq!(parts(atomic, pg).len(), 2, "{:?}", parts(atomic, pg));
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
