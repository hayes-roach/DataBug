//! Error-tolerant SQL tokenizer. Never fails: unterminated strings and
//! comments run to end of input, unknown characters become `Unknown` tokens.

use databug_core::Dialect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokKind {
    Keyword,
    Ident,
    QuotedIdent,
    String,
    Number,
    Comment,
    Operator,
    Punct,
    Param,
    Whitespace,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokKind,
    pub start: usize,
    pub end: usize,
}

impl Token {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }

    pub fn is_trivia(&self) -> bool {
        matches!(self.kind, TokKind::Whitespace | TokKind::Comment)
    }

    pub fn is_name(&self) -> bool {
        matches!(self.kind, TokKind::Ident | TokKind::QuotedIdent)
    }

    pub fn is_kw(&self, src: &str, kw: &str) -> bool {
        self.kind == TokKind::Keyword && self.text(src).eq_ignore_ascii_case(kw)
    }

    pub fn is_punct(&self, src: &str, p: &str) -> bool {
        self.kind == TokKind::Punct && self.text(src) == p
    }
}

/// Reserved and common keywords, uppercase and sorted for binary search.
pub const KEYWORDS: &[&str] = &[
    "ADD", "ALL", "ALTER", "AND", "ANY", "AS", "ASC", "BEGIN", "BETWEEN", "BY", "CASCADE", "CASE",
    "CAST", "CHECK", "COLUMN", "COMMIT", "CONSTRAINT", "CREATE", "CROSS", "CURRENT_DATE",
    "CURRENT_TIMESTAMP", "DATABASE", "DECLARE", "DEFAULT", "DELETE", "DESC", "DESCRIBE", "DISTINCT",
    "DROP", "ELSE", "END", "EXCEPT", "EXEC", "EXECUTE", "EXISTS", "EXPLAIN", "FALSE", "FETCH",
    "FILTER", "FIRST", "FOR", "FOREIGN", "FROM", "FULL", "FUNCTION", "GO", "GRANT", "GROUP",
    "HAVING", "IF", "ILIKE", "IN", "INDEX", "INNER", "INSERT", "INTERSECT", "INTO", "IS", "JOIN",
    "KEY", "LATERAL", "LEFT", "LIKE", "LIMIT", "MERGE", "NATURAL", "NEXT", "NOT", "NULL", "NULLS",
    "OFFSET", "ON", "ONLY", "OR", "ORDER", "OUTER", "OVER", "PARTITION", "PRAGMA", "PRIMARY",
    "PROCEDURE", "RECURSIVE", "REFERENCES", "RENAME", "REPLACE", "RETURNING", "REVOKE", "RIGHT",
    "ROLLBACK", "ROW", "ROWS", "SAVEPOINT", "SCHEMA", "SELECT", "SET", "SHOW", "START", "TABLE",
    "TEMP", "TEMPORARY", "THEN", "TOP", "TRAN", "TRANSACTION", "TRIGGER", "TRUE", "TRUNCATE",
    "UNION", "UNIQUE", "UPDATE", "USE", "USING", "VALUES", "VIEW", "WHEN", "WHERE", "WINDOW",
    "WITH",
];

pub fn is_keyword(word: &str) -> bool {
    if word.len() > 17 {
        return false;
    }
    let mut buf = [0u8; 17];
    for (i, b) in word.bytes().enumerate() {
        buf[i] = b.to_ascii_uppercase();
    }
    let up = std::str::from_utf8(&buf[..word.len()]).unwrap_or("");
    KEYWORDS.binary_search(&up).is_ok()
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

pub fn tokenize(src: &str, dialect: Dialect) -> Vec<Token> {
    let bytes = src.as_bytes();
    let mut out = Vec::with_capacity(src.len() / 3);
    let mut i = 0;
    let n = bytes.len();

    let char_at = |i: usize| src[i..].chars().next().unwrap_or('\0');

    while i < n {
        let start = i;
        let c = char_at(i);
        let next = if i + c.len_utf8() < n { char_at(i + c.len_utf8()) } else { '\0' };
        let kind;

        if c.is_whitespace() {
            while i < n && char_at(i).is_whitespace() {
                i += char_at(i).len_utf8();
            }
            kind = TokKind::Whitespace;
        } else if (c == '-' && next == '-') || (c == '#' && dialect == Dialect::MySql) || (c == '/' && next == '/' && dialect == Dialect::Mongo) {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            kind = TokKind::Comment;
        } else if c == '/' && next == '*' {
            i += 2;
            while i < n && !(bytes[i] == b'*' && i + 1 < n && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(n);
            kind = TokKind::Comment;
        } else if c == '\'' || (c == '"' && matches!(dialect, Dialect::MySql | Dialect::Mongo)) {
            i = scan_quoted(bytes, i, c as u8, dialect == Dialect::MySql || dialect == Dialect::Mongo);
            kind = TokKind::String;
        } else if (c == 'E' || c == 'e' || c == 'N' || c == 'n') && next == '\'' {
            i = scan_quoted(bytes, i + 1, b'\'', c == 'E' || c == 'e');
            kind = TokKind::String;
        } else if c == '"' {
            i = scan_quoted(bytes, i, b'"', false);
            kind = TokKind::QuotedIdent;
        } else if c == '`' {
            i = scan_quoted(bytes, i, b'`', false);
            kind = TokKind::QuotedIdent;
        } else if c == '[' && dialect == Dialect::MsSql {
            i += 1;
            while i < n && bytes[i] != b']' {
                i += 1;
            }
            i = (i + 1).min(n);
            kind = TokKind::QuotedIdent;
        } else if c == '$' && dialect == Dialect::Postgres && (next == '$' || is_ident_start(next)) && dollar_tag(src, i).is_some() {
            let tag_len = dollar_tag(src, i).unwrap();
            let tag = &src[i..i + tag_len];
            i += tag_len;
            match src[i..].find(tag) {
                Some(p) => i += p + tag_len,
                None => i = n,
            }
            kind = TokKind::String;
        } else if c.is_ascii_digit() || (c == '.' && next.is_ascii_digit()) {
            while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_') {
                // exponent sign
                if (bytes[i] == b'e' || bytes[i] == b'E') && i + 1 < n && (bytes[i + 1] == b'+' || bytes[i + 1] == b'-') {
                    i += 1;
                }
                i += 1;
            }
            kind = TokKind::Number;
        } else if (c == '$' && next.is_ascii_digit())
            || (c == ':' && is_ident_start(next) && dialect != Dialect::Mongo)
            || c == '@'
            || (c == '?' && dialect != Dialect::Postgres)
        {
            i += c.len_utf8();
            while i < n && (is_ident_char(char_at(i)) || char_at(i) == '@') {
                i += char_at(i).len_utf8();
            }
            kind = TokKind::Param;
        } else if is_ident_start(c) || (c == '$' && dialect == Dialect::Mongo) {
            while i < n && is_ident_char(char_at(i)) {
                i += char_at(i).len_utf8();
            }
            kind = if dialect != Dialect::Mongo && is_keyword(&src[start..i]) { TokKind::Keyword } else { TokKind::Ident };
        } else if "(),;.[]{}".contains(c) {
            i += 1;
            kind = TokKind::Punct;
        } else if "+-*/%<>=!|&^~:?".contains(c) {
            i += 1;
            while i < n && "<>=!|&:".contains(bytes[i] as char) && !(bytes[i] == b':' && dialect == Dialect::Mongo) {
                i += 1;
            }
            kind = TokKind::Operator;
        } else {
            i += c.len_utf8();
            kind = TokKind::Unknown;
        }
        out.push(Token { kind, start, end: i });
    }
    out
}

fn scan_quoted(bytes: &[u8], mut i: usize, q: u8, backslash: bool) -> usize {
    let n = bytes.len();
    i += 1;
    while i < n {
        let b = bytes[i];
        if backslash && b == b'\\' {
            i += 2;
            continue;
        }
        if b == q {
            if i + 1 < n && bytes[i + 1] == q {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    n
}

/// Length of a `$tag$` opener at `i`, if present.
fn dollar_tag(src: &str, i: usize) -> Option<usize> {
    let rest = &src[i + 1..];
    let end = rest.find('$')?;
    let tag = &rest[..end];
    if tag.chars().all(|c| c.is_alphanumeric() || c == '_') {
        Some(end + 2)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str, d: Dialect) -> Vec<(TokKind, String)> {
        tokenize(src, d)
            .into_iter()
            .filter(|t| t.kind != TokKind::Whitespace)
            .map(|t| (t.kind, t.text(src).to_string()))
            .collect()
    }

    #[test]
    fn basic_select() {
        let k = kinds("SELECT a.\"Id\", 'it''s' FROM t -- hi", Dialect::Postgres);
        assert_eq!(k[0].0, TokKind::Keyword);
        assert_eq!(k[3], (TokKind::QuotedIdent, "\"Id\"".into()));
        assert_eq!(k[5], (TokKind::String, "'it''s'".into()));
        assert_eq!(k.last().unwrap().0, TokKind::Comment);
    }

    #[test]
    fn unterminated_is_tolerated() {
        let k = kinds("select 'abc", Dialect::Postgres);
        assert_eq!(k[1].0, TokKind::String);
        let k = kinds("select /* x", Dialect::Postgres);
        assert_eq!(k[1].0, TokKind::Comment);
    }

    #[test]
    fn dollar_quotes_and_brackets() {
        let k = kinds("do $fn$ select 1; $fn$;", Dialect::Postgres);
        assert_eq!(k[1].0, TokKind::String);
        let k = kinds("select [order id] from [dbo].[t]", Dialect::MsSql);
        assert_eq!(k[1], (TokKind::QuotedIdent, "[order id]".into()));
    }

    #[test]
    fn unicode_does_not_panic() {
        let k = kinds("select 'héllo', naïve from tablé", Dialect::Sqlite);
        assert!(k.iter().any(|(_, t)| t == "naïve"));
    }
}
