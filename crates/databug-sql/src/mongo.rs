//! A small parser for mongo-shell style statements such as
//! `db.orders.find({status: "open"}).sort({total: -1}).limit(20)`.

use crate::analyze::{Danger, StatementKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MongoCall {
    pub name: String,
    /// Raw text between the parentheses (relaxed JSON / JSON5).
    pub args: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MongoStatement {
    /// `show collections`, `show dbs`
    Show(String),
    /// `db.<collection>.<calls>` or `db.<calls>` when `collection` is None.
    Command { collection: Option<String>, calls: Vec<MongoCall> },
}

pub const COLLECTION_METHODS: &[(&str, &str)] = &[
    ("find", "find(filter, projection)"),
    ("findOne", "findOne(filter)"),
    ("aggregate", "aggregate([pipeline])"),
    ("countDocuments", "countDocuments(filter)"),
    ("estimatedDocumentCount", "estimatedDocumentCount()"),
    ("distinct", "distinct(field, filter)"),
    ("insertOne", "insertOne(document)"),
    ("insertMany", "insertMany([documents])"),
    ("updateOne", "updateOne(filter, update)"),
    ("updateMany", "updateMany(filter, update)"),
    ("replaceOne", "replaceOne(filter, document)"),
    ("deleteOne", "deleteOne(filter)"),
    ("deleteMany", "deleteMany(filter)"),
    ("createIndex", "createIndex(keys)"),
    ("getIndexes", "getIndexes()"),
    ("drop", "drop()"),
];

pub const CURSOR_METHODS: &[&str] = &["sort", "limit", "skip", "projection"];

pub const OPERATORS: &[&str] = &[
    "$eq", "$ne", "$gt", "$gte", "$lt", "$lte", "$in", "$nin", "$and", "$or", "$not", "$nor",
    "$exists", "$type", "$regex", "$elemMatch", "$size", "$all", "$set", "$unset", "$inc", "$push",
    "$pull", "$addToSet", "$match", "$group", "$project", "$sort", "$limit", "$skip", "$lookup",
    "$unwind", "$count", "$sum", "$avg", "$min", "$max", "$first", "$last",
];

const READ_METHODS: &[&str] = &[
    "find", "findOne", "aggregate", "countDocuments", "estimatedDocumentCount", "distinct",
    "getIndexes", "explain", "stats", "listCollections", "getCollectionNames",
];

pub fn parse(stmt: &str) -> Result<MongoStatement, String> {
    let s = stmt.trim().trim_end_matches(';').trim();
    if let Some(rest) = s.strip_prefix("show ").or_else(|| s.strip_prefix("SHOW ")) {
        return Ok(MongoStatement::Show(rest.trim().to_string()));
    }
    let b = s.as_bytes();
    let mut i = 0;
    let ident = |i: &mut usize| -> String {
        let st = *i;
        while *i < b.len() && (b[*i].is_ascii_alphanumeric() || b[*i] == b'_' || b[*i] == b'$' || b[*i] == b'-') {
            *i += 1;
        }
        s[st..*i].to_string()
    };
    let skip_ws = |i: &mut usize| {
        while *i < b.len() && (b[*i] as char).is_whitespace() {
            *i += 1;
        }
    };
    if ident(&mut i) != "db" {
        return Err("Expected a statement starting with `db.` or `show`".into());
    }
    // segments: names, possibly ending with a call
    let mut names: Vec<String> = Vec::new();
    let mut calls: Vec<MongoCall> = Vec::new();
    let mut collection: Option<String> = None;
    loop {
        skip_ws(&mut i);
        if i >= b.len() {
            break;
        }
        if b[i] != b'.' {
            return Err(format!("Unexpected `{}` at position {}", &s[i..i + 1], i));
        }
        i += 1;
        skip_ws(&mut i);
        let name = ident(&mut i);
        if name.is_empty() {
            return Err("Expected a name after `.`".into());
        }
        skip_ws(&mut i);
        if i < b.len() && b[i] == b'(' {
            let close = matching_paren(s, i).ok_or("Unclosed `(`")?;
            let args = s[i + 1..close].trim().to_string();
            i = close + 1;
            if calls.is_empty() && name == "getCollection" {
                let n = args.trim_matches(|c| c == '"' || c == '\'' || c == ' ').to_string();
                collection = Some(n);
                continue;
            }
            if calls.is_empty() && collection.is_none() && !names.is_empty() {
                collection = Some(names.join("."));
            }
            calls.push(MongoCall { name, args });
        } else if calls.is_empty() {
            names.push(name);
        } else {
            return Err(format!("Expected `(` after `{name}`"));
        }
    }
    if calls.is_empty() {
        return Err("Nothing to run: add a method call such as `.find({})`".into());
    }
    Ok(MongoStatement::Command { collection, calls })
}

fn matching_paren(s: &str, open: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 0;
    let mut i = open;
    let mut quote: Option<u8> = None;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
        } else {
            match c {
                b'"' | b'\'' => quote = Some(c),
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Split top-level comma-separated arguments.
pub fn split_args(args: &str) -> Vec<&str> {
    let b = args.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0;
    let mut quote: Option<u8> = None;
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
        } else {
            match c {
                b'"' | b'\'' => quote = Some(c),
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b',' if depth == 0 => {
                    out.push(args[start..i].trim());
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    let last = args[start..].trim();
    if !last.is_empty() {
        out.push(last);
    }
    out
}

pub fn classify(stmt: &str) -> StatementKind {
    match parse(stmt) {
        Ok(MongoStatement::Show(_)) => StatementKind::Read,
        Ok(MongoStatement::Command { calls, .. }) => {
            let m = calls[0].name.as_str();
            if READ_METHODS.contains(&m) {
                StatementKind::Read
            } else if matches!(m, "drop" | "dropDatabase" | "createIndex" | "dropIndex" | "createCollection" | "renameCollection") {
                StatementKind::Ddl
            } else if m == "runCommand" {
                StatementKind::Other
            } else {
                StatementKind::Write
            }
        }
        Err(_) => StatementKind::Other,
    }
}

pub fn dangers(stmt: &str) -> Vec<Danger> {
    let Ok(MongoStatement::Command { calls, .. }) = parse(stmt) else { return vec![] };
    let c = &calls[0];
    let first_arg = split_args(&c.args).first().map(|a| a.replace(char::is_whitespace, "")).unwrap_or_default();
    let empty = first_arg.is_empty() || first_arg == "{}";
    match c.name.as_str() {
        "deleteMany" if empty => vec![Danger::DeleteWithoutWhere],
        "updateMany" if empty => vec![Danger::UpdateWithoutWhere],
        "drop" | "dropDatabase" => vec![Danger::Drop],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chain() {
        let p = parse("db.orders.find({status: 'open', n: {$gt: 1}}).sort({total: -1}).limit(20)").unwrap();
        let MongoStatement::Command { collection, calls } = p else { panic!() };
        assert_eq!(collection.as_deref(), Some("orders"));
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].args, "{status: 'open', n: {$gt: 1}}");
        assert_eq!(calls[2].args, "20");
    }

    #[test]
    fn parses_get_collection_and_dotted() {
        let MongoStatement::Command { collection, .. } = parse("db.getCollection('a.b').countDocuments({})").unwrap() else { panic!() };
        assert_eq!(collection.as_deref(), Some("a.b"));
        let MongoStatement::Command { collection, .. } = parse("db.system.users.find()").unwrap() else { panic!() };
        assert_eq!(collection.as_deref(), Some("system.users"));
        let MongoStatement::Command { collection, calls } = parse("db.runCommand({ping: 1})").unwrap() else { panic!() };
        assert_eq!(collection, None);
        assert_eq!(calls[0].name, "runCommand");
    }

    #[test]
    fn args_and_safety() {
        assert_eq!(split_args("{a: [1,2]}, {b: 'x,y'}"), vec!["{a: [1,2]}", "{b: 'x,y'}"]);
        assert_eq!(dangers("db.t.deleteMany({})"), vec![Danger::DeleteWithoutWhere]);
        assert!(dangers("db.t.deleteMany({a: 1})").is_empty());
        assert_eq!(classify("db.t.find()"), StatementKind::Read);
        assert_eq!(classify("db.t.insertOne({})"), StatementKind::Write);
    }
}
