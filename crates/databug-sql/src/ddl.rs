//! DDL generation per dialect: CREATE TABLE, ALTER TABLE column changes,
//! indexes, foreign keys, renames, and bulk INSERTs. Used by the table
//! editor, schema compare, rename refactoring and CSV import.

use databug_core::{ColumnInfo, Dialect, ForeignKey, IndexInfo, ObjectKind, TableInfo};

/// One generated statement. `note` explains a limitation (shown as a comment).
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub sql: String,
    pub note: Option<String>,
}

impl Stmt {
    pub fn new(sql: impl Into<String>) -> Self {
        Stmt { sql: sql.into(), note: None }
    }

    pub fn comment(note: impl Into<String>) -> Self {
        Stmt { sql: String::new(), note: Some(note.into()) }
    }
}

/// Render statements as a script with batch separators for SQL Server.
pub fn script(d: Dialect, stmts: &[Stmt]) -> String {
    let mut out = String::new();
    for s in stmts {
        if let Some(n) = &s.note {
            for line in n.lines() {
                out.push_str("-- ");
                out.push_str(line);
                out.push('\n');
            }
        }
        if !s.sql.is_empty() {
            out.push_str(s.sql.trim_end_matches(';'));
            out.push_str(";\n");
            if d == Dialect::MsSql && needs_batch(&s.sql) {
                out.push_str("GO\n");
            }
        }
        out.push('\n');
    }
    out
}

fn needs_batch(sql: &str) -> bool {
    let up = sql.trim_start().to_ascii_uppercase();
    ["CREATE VIEW", "CREATE OR ALTER", "CREATE PROCEDURE", "CREATE FUNCTION", "CREATE TRIGGER", "ALTER VIEW", "ALTER PROCEDURE", "ALTER FUNCTION"].iter().any(|p| up.starts_with(p))
}

pub fn qualified(d: Dialect, schema: &str, name: &str) -> String {
    if schema.is_empty() || (d == Dialect::Sqlite && schema == "main") {
        d.quote_ident(name)
    } else {
        format!("{}.{}", d.quote_ident(schema), d.quote_ident(name))
    }
}

fn idents(d: Dialect, cols: &[String]) -> String {
    cols.iter().map(|c| d.quote_ident(c)).collect::<Vec<_>>().join(", ")
}

fn column_def(d: Dialect, c: &ColumnInfo, inline_pk: bool) -> String {
    let ty = if c.data_type.trim().is_empty() { "TEXT" } else { c.data_type.trim() };
    let mut s = format!("{} {}", d.quote_ident(&c.name), ty);
    if inline_pk {
        s.push_str(" PRIMARY KEY");
    } else if !c.nullable {
        s.push_str(" NOT NULL");
    }
    if let Some(def) = c.default.as_deref().filter(|v| !v.trim().is_empty() && !v.trim().eq_ignore_ascii_case("null")) {
        s.push_str(" DEFAULT ");
        s.push_str(def.trim());
    }
    s
}

fn fk_clause(d: Dialect, fk: &ForeignKey, table_schema: &str) -> String {
    let rs = if fk.ref_schema.is_empty() { table_schema } else { &fk.ref_schema };
    let mut s = format!("FOREIGN KEY ({}) REFERENCES {} ({})", idents(d, &fk.columns), qualified(d, rs, &fk.ref_table), idents(d, &fk.ref_columns));
    if !fk.on_delete.is_empty() {
        s.push_str(" ON DELETE ");
        s.push_str(&fk.on_delete);
    }
    s
}

/// CREATE TABLE with primary key, foreign keys and secondary indexes.
pub fn create_table(d: Dialect, schema: &str, t: &TableInfo) -> Vec<Stmt> {
    let pk: Vec<String> = t.columns.iter().filter(|c| c.primary_key).map(|c| c.name.clone()).collect();
    // SQLite's rowid alias needs `INTEGER PRIMARY KEY` inline to autoincrement
    let inline = d == Dialect::Sqlite && pk.len() == 1 && t.column(&pk[0]).map(|c| c.data_type.eq_ignore_ascii_case("integer")).unwrap_or(false);
    let mut lines: Vec<String> = t.columns.iter().map(|c| format!("    {}", column_def(d, c, inline && c.primary_key))).collect();
    if !pk.is_empty() && !inline {
        let name = t.primary_index().map(|i| i.name.clone()).filter(|n| d != Dialect::MySql && d != Dialect::Sqlite && !n.is_empty());
        match name {
            Some(n) => lines.push(format!("    CONSTRAINT {} PRIMARY KEY ({})", d.quote_ident(&n), idents(d, &pk))),
            None => lines.push(format!("    PRIMARY KEY ({})", idents(d, &pk))),
        }
    }
    for fk in &t.foreign_keys {
        if fk.name.is_empty() || d == Dialect::Sqlite {
            lines.push(format!("    {}", fk_clause(d, fk, schema)));
        } else {
            lines.push(format!("    CONSTRAINT {} {}", d.quote_ident(&fk.name), fk_clause(d, fk, schema)));
        }
    }
    let mut out = vec![Stmt::new(format!("CREATE TABLE {} (\n{}\n)", qualified(d, schema, &t.name), lines.join(",\n")))];
    for idx in t.indexes.iter().filter(|i| !i.primary) {
        out.push(create_index(d, schema, &t.name, idx));
    }
    out
}

pub fn drop_relation(d: Dialect, schema: &str, name: &str, kind: ObjectKind) -> Stmt {
    let what = match kind {
        ObjectKind::View => "VIEW",
        ObjectKind::MaterializedView => "MATERIALIZED VIEW",
        _ => "TABLE",
    };
    Stmt::new(format!("DROP {what} {}", qualified(d, schema, name)))
}

pub fn create_index(d: Dialect, schema: &str, table: &str, idx: &IndexInfo) -> Stmt {
    let unique = if idx.unique { "UNIQUE " } else { "" };
    let name = match d {
        // Postgres and SQLite put the index in the table's schema
        Dialect::Sqlite if schema != "main" && !schema.is_empty() => qualified(d, schema, &idx.name),
        _ => d.quote_ident(&idx.name),
    };
    let target = if d == Dialect::Sqlite { d.quote_ident(table) } else { qualified(d, schema, table) };
    Stmt::new(format!("CREATE {unique}INDEX {name} ON {target} ({})", idents(d, &idx.columns)))
}

pub fn drop_index(d: Dialect, schema: &str, table: &str, idx: &IndexInfo) -> Stmt {
    match d {
        Dialect::MySql => Stmt::new(format!("DROP INDEX {} ON {}", d.quote_ident(&idx.name), qualified(d, schema, table))),
        Dialect::MsSql => Stmt::new(format!("DROP INDEX {} ON {}", d.quote_ident(&idx.name), qualified(d, schema, table))),
        _ => Stmt::new(format!("DROP INDEX {}", qualified(d, schema, &idx.name))),
    }
}

pub fn add_foreign_key(d: Dialect, schema: &str, table: &str, fk: &ForeignKey) -> Stmt {
    if d == Dialect::Sqlite {
        return Stmt::comment(format!("SQLite can't add a foreign key to an existing table ({table}: {}); rebuild the table to add it.", fk.columns.join(", ")));
    }
    let name = if fk.name.is_empty() { format!("fk_{}_{}", table, fk.columns.join("_")) } else { fk.name.clone() };
    Stmt::new(format!("ALTER TABLE {} ADD CONSTRAINT {} {}", qualified(d, schema, table), d.quote_ident(&name), fk_clause(d, fk, schema)))
}

pub fn drop_foreign_key(d: Dialect, schema: &str, table: &str, fk: &ForeignKey) -> Stmt {
    match d {
        Dialect::Sqlite => Stmt::comment(format!("SQLite can't drop a foreign key ({}) without rebuilding the table", fk.name)),
        Dialect::MySql => Stmt::new(format!("ALTER TABLE {} DROP FOREIGN KEY {}", qualified(d, schema, table), d.quote_ident(&fk.name))),
        _ => Stmt::new(format!("ALTER TABLE {} DROP CONSTRAINT {}", qualified(d, schema, table), d.quote_ident(&fk.name))),
    }
}

pub fn rename_table(d: Dialect, schema: &str, from: &str, to: &str) -> Stmt {
    match d {
        Dialect::MsSql => Stmt::new(format!("EXEC sp_rename {}, {}", d.quote_literal(&format!("{schema}.{from}")), d.quote_literal(to))),
        Dialect::MySql => Stmt::new(format!("RENAME TABLE {} TO {}", qualified(d, schema, from), qualified(d, schema, to))),
        _ => Stmt::new(format!("ALTER TABLE {} RENAME TO {}", qualified(d, schema, from), d.quote_ident(to))),
    }
}

pub fn rename_column(d: Dialect, schema: &str, table: &str, from: &str, to: &str, def: Option<&ColumnInfo>) -> Stmt {
    match d {
        Dialect::MsSql => Stmt::new(format!("EXEC sp_rename {}, {}, 'COLUMN'", d.quote_literal(&format!("{schema}.{table}.{from}")), d.quote_literal(to))),
        // CHANGE COLUMN works on MySQL 5.7 and MariaDB as well as 8.0
        Dialect::MySql => match def {
            Some(c) => {
                let renamed = ColumnInfo { name: to.to_string(), ..c.clone() };
                Stmt::new(format!("ALTER TABLE {} CHANGE COLUMN {} {}", qualified(d, schema, table), d.quote_ident(from), column_def(d, &renamed, false)))
            }
            None => Stmt::new(format!("ALTER TABLE {} RENAME COLUMN {} TO {}", qualified(d, schema, table), d.quote_ident(from), d.quote_ident(to))),
        },
        _ => Stmt::new(format!("ALTER TABLE {} RENAME COLUMN {} TO {}", qualified(d, schema, table), d.quote_ident(from), d.quote_ident(to))),
    }
}

fn norm_type(t: &str) -> String {
    let t = t.trim().to_ascii_lowercase().replace(' ', "");
    match t.as_str() {
        "int4" | "integer" | "int" => "int".into(),
        "int8" | "bigint" => "bigint".into(),
        "int2" | "smallint" => "smallint".into(),
        "bool" | "boolean" => "boolean".into(),
        "float8" | "doubleprecision" | "double" => "double".into(),
        "float4" | "real" => "real".into(),
        "timestamp" | "timestampwithouttimezone" => "timestamp".into(),
        "timestamptz" | "timestampwithtimezone" => "timestamptz".into(),
        _ => t.replace("charactervarying", "varchar").replace("character", "char").replace("numeric", "decimal"),
    }
}

pub fn same_type(a: &str, b: &str) -> bool {
    norm_type(a) == norm_type(b)
}

fn norm_default(v: &Option<String>) -> Option<String> {
    v.as_ref().map(|s| s.trim().trim_matches(|c| c == '(' || c == ')').to_string()).filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null"))
}

/// The edits a table goes through. Column identity is tracked by the
/// original name so renames don't look like drop + add.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnEdit {
    /// Original column name, or `None` for a new column.
    pub original: Option<String>,
    pub column: ColumnInfo,
    pub dropped: bool,
}

/// Turn an edited table into ALTER statements. SQLite changes it can't do
/// in place are done with a table rebuild.
pub fn alter_table(d: Dialect, schema: &str, old: &TableInfo, new_name: &str, edits: &[ColumnEdit], new_indexes: &[IndexInfo], new_fks: &[ForeignKey]) -> Vec<Stmt> {
    let q = qualified(d, schema, &old.name);
    let mut out = Vec::new();
    let mut needs_rebuild = false;

    // dropped foreign keys and indexes first (they may depend on columns we change)
    for fk in &old.foreign_keys {
        if !new_fks.iter().any(|n| n == fk) {
            if d == Dialect::Sqlite {
                needs_rebuild = true;
            } else {
                out.push(drop_foreign_key(d, schema, &old.name, fk));
            }
        }
    }
    for idx in old.indexes.iter().filter(|i| !i.primary) {
        if !new_indexes.iter().any(|n| n == idx) {
            out.push(drop_index(d, schema, &old.name, idx));
        }
    }

    let old_pk: Vec<String> = old.columns.iter().filter(|c| c.primary_key).map(|c| c.name.clone()).collect();
    let new_pk: Vec<String> = edits.iter().filter(|e| !e.dropped && e.column.primary_key).map(|e| e.column.name.clone()).collect();
    let pk_renamed_only = old_pk.len() == new_pk.len()
        && old_pk.iter().all(|o| edits.iter().any(|e| !e.dropped && e.original.as_deref() == Some(o.as_str()) && e.column.primary_key));
    let pk_changed = !pk_renamed_only;
    if pk_changed && !old_pk.is_empty() {
        match d {
            Dialect::Postgres | Dialect::MsSql => {
                let name = old.primary_index().map(|i| i.name.clone()).unwrap_or_else(|| format!("{}_pkey", old.name));
                out.push(Stmt::new(format!("ALTER TABLE {q} DROP CONSTRAINT {}", d.quote_ident(&name))));
            }
            Dialect::MySql => out.push(Stmt::new(format!("ALTER TABLE {q} DROP PRIMARY KEY"))),
            _ => needs_rebuild = true,
        }
    }

    for e in edits {
        let c = &e.column;
        match (&e.original, e.dropped) {
            (None, true) => {}
            (Some(orig), true) => match d {
                Dialect::Sqlite if old_pk.contains(orig) => needs_rebuild = true,
                _ => out.push(Stmt::new(format!("ALTER TABLE {q} DROP COLUMN {}", d.quote_ident(orig)))),
            },
            (None, false) => {
                let def = column_def(d, &ColumnInfo { primary_key: false, ..c.clone() }, false);
                if d == Dialect::Sqlite && (!c.nullable && c.default.is_none() || c.primary_key) {
                    needs_rebuild = true;
                } else {
                    out.push(Stmt::new(format!("ALTER TABLE {q} ADD {}{def}", if d == Dialect::MsSql { "" } else { "COLUMN " })));
                }
            }
            (Some(orig), false) => {
                let Some(before) = old.column(orig) else { continue };
                let renamed = !orig.eq(&c.name);
                let type_changed = !same_type(&before.data_type, &c.data_type);
                let null_changed = before.nullable != c.nullable && !c.primary_key;
                let default_changed = norm_default(&before.default) != norm_default(&c.default);
                if d == Dialect::Sqlite {
                    if type_changed || null_changed || default_changed {
                        needs_rebuild = true;
                    } else if renamed {
                        out.push(rename_column(d, schema, &old.name, orig, &c.name, None));
                    }
                    continue;
                }
                if d == Dialect::MySql {
                    if renamed || type_changed || null_changed || default_changed {
                        let def = column_def(d, &ColumnInfo { primary_key: false, nullable: c.nullable && !c.primary_key, ..c.clone() }, false);
                        if renamed {
                            out.push(Stmt::new(format!("ALTER TABLE {q} CHANGE COLUMN {} {def}", d.quote_ident(orig))));
                        } else {
                            out.push(Stmt::new(format!("ALTER TABLE {q} MODIFY COLUMN {def}")));
                        }
                    }
                    continue;
                }
                let name = d.quote_ident(&c.name);
                if renamed {
                    out.push(rename_column(d, schema, &old.name, orig, &c.name, None));
                }
                if d == Dialect::Postgres {
                    if type_changed {
                        out.push(Stmt::new(format!("ALTER TABLE {q} ALTER COLUMN {name} TYPE {} USING {name}::{}", c.data_type, c.data_type)));
                    }
                    if null_changed {
                        out.push(Stmt::new(format!("ALTER TABLE {q} ALTER COLUMN {name} {} NOT NULL", if c.nullable { "DROP" } else { "SET" })));
                    }
                    if default_changed {
                        match c.default.as_deref().filter(|v| !v.trim().is_empty()) {
                            Some(v) => out.push(Stmt::new(format!("ALTER TABLE {q} ALTER COLUMN {name} SET DEFAULT {v}"))),
                            None => out.push(Stmt::new(format!("ALTER TABLE {q} ALTER COLUMN {name} DROP DEFAULT"))),
                        }
                    }
                } else {
                    // SQL Server
                    if type_changed || null_changed {
                        out.push(Stmt::new(format!("ALTER TABLE {q} ALTER COLUMN {name} {} {}NULL", c.data_type, if c.nullable && !c.primary_key { "" } else { "NOT " })));
                    }
                    if default_changed {
                        let mut s = Stmt::new(match c.default.as_deref().filter(|v| !v.trim().is_empty()) {
                            Some(v) => format!("ALTER TABLE {q} ADD DEFAULT {v} FOR {name}"),
                            None => String::new(),
                        });
                        if before.default.is_some() {
                            s.note = Some(format!("Drop the existing default constraint on {} first (see sys.default_constraints)", c.name));
                        }
                        out.push(s);
                    }
                }
            }
        }
    }

    if pk_changed && !new_pk.is_empty() && !needs_rebuild {
        match d {
            Dialect::Sqlite => needs_rebuild = true,
            Dialect::MySql => out.push(Stmt::new(format!("ALTER TABLE {q} ADD PRIMARY KEY ({})", idents(d, &new_pk)))),
            _ => out.push(Stmt::new(format!("ALTER TABLE {q} ADD CONSTRAINT {} PRIMARY KEY ({})", d.quote_ident(&format!("{}_pkey", new_name)), idents(d, &new_pk)))),
        }
    }

    for idx in new_indexes.iter().filter(|i| !i.primary) {
        if !old.indexes.iter().any(|o| o == idx) {
            out.push(create_index(d, schema, &old.name, idx));
        }
    }
    for fk in new_fks {
        if !old.foreign_keys.iter().any(|o| o == fk) {
            if d == Dialect::Sqlite {
                needs_rebuild = true;
            } else {
                out.push(add_foreign_key(d, schema, &old.name, fk));
            }
        }
    }

    if needs_rebuild {
        return sqlite_rebuild(schema, old, new_name, edits, new_indexes, new_fks);
    }
    if new_name != old.name {
        out.push(rename_table(d, schema, &old.name, new_name));
    }
    out
}

/// SQLite can't change column types, constraints or keys in place: build a
/// new table, copy the rows, swap it in.
fn sqlite_rebuild(schema: &str, old: &TableInfo, new_name: &str, edits: &[ColumnEdit], indexes: &[IndexInfo], fks: &[ForeignKey]) -> Vec<Stmt> {
    let d = Dialect::Sqlite;
    let tmp = format!("{}__databug_new", new_name);
    let mut t = TableInfo::new(tmp.clone(), ObjectKind::Table);
    t.columns = edits.iter().filter(|e| !e.dropped).map(|e| e.column.clone()).collect();
    t.foreign_keys = fks.to_vec();
    let copy: Vec<(&str, &str)> = edits.iter().filter(|e| !e.dropped).filter_map(|e| e.original.as_deref().map(|o| (o, e.column.name.as_str()))).collect();
    let mut out = vec![Stmt { sql: "PRAGMA foreign_keys = OFF".into(), note: Some(format!("SQLite can't make these changes in place, so {} is rebuilt: create a new table, copy the rows, swap it in.", old.name)) }];
    out.push(Stmt::new("BEGIN"));
    out.extend(create_table(d, schema, &TableInfo { indexes: vec![], ..t }));
    out.push(Stmt::new(format!(
        "INSERT INTO {} ({}) SELECT {} FROM {}",
        qualified(d, schema, &tmp),
        copy.iter().map(|c| d.quote_ident(c.1)).collect::<Vec<_>>().join(", "),
        copy.iter().map(|c| d.quote_ident(c.0)).collect::<Vec<_>>().join(", "),
        qualified(d, schema, &old.name)
    )));
    out.push(Stmt::new(format!("DROP TABLE {}", qualified(d, schema, &old.name))));
    out.push(Stmt::new(format!("ALTER TABLE {} RENAME TO {}", qualified(d, schema, &tmp), d.quote_ident(new_name))));
    for idx in indexes.iter().filter(|i| !i.primary) {
        out.push(create_index(d, schema, new_name, idx));
    }
    out.push(Stmt::new("COMMIT"));
    out.push(Stmt::new("PRAGMA foreign_keys = ON"));
    out
}

/// Multi-row INSERTs, `batch` rows per statement.
pub fn insert_batches(d: Dialect, target: &str, columns: &[String], numeric: &[bool], rows: &[Vec<Option<String>>], batch: usize) -> Vec<String> {
    let cols = idents(d, columns);
    let lit = |v: &Option<String>, num: bool| match v {
        None => "NULL".to_string(),
        Some(s) if num && s.trim().parse::<f64>().is_ok() => s.trim().to_string(),
        Some(s) => d.quote_literal(s),
    };
    rows.chunks(batch.clamp(1, 1000))
        .map(|chunk| {
            let values: Vec<String> = chunk.iter().map(|r| format!("({})", r.iter().enumerate().map(|(i, v)| lit(v, numeric.get(i).copied().unwrap_or(false))).collect::<Vec<_>>().join(", "))).collect();
            format!("INSERT INTO {target} ({cols}) VALUES\n{}", values.join(",\n"))
        })
        .collect()
}

/// Split a parameter list on commas outside parentheses
/// (`a numeric(10,2), b text` → two parameters).
pub fn split_params(list: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in list.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// A ready-to-edit statement that runs a procedure or function: `EXEC` on
/// SQL Server, `CALL` for PostgreSQL and MySQL procedures, and `SELECT` for
/// functions. Each parameter gets a `NULL` placeholder with its declaration
/// as a comment. `params` is the routine's parameter list as introspected.
pub fn exec_template(d: Dialect, schema: &str, name: &str, kind: ObjectKind, params: &str) -> Option<String> {
    let target = qualified(d, schema, name);
    let mut params = split_params(params);
    let is_proc = matches!(kind, ObjectKind::Procedure | ObjectKind::ClrProcedure);
    let is_table_fn = matches!(kind, ObjectKind::TableFunction | ObjectKind::ClrTableFunction);
    let is_fn = matches!(kind, ObjectKind::Function | ObjectKind::ClrFunction | ObjectKind::Aggregate) || is_table_fn;
    if !is_proc && !is_fn {
        return None;
    }
    if matches!(d, Dialect::Sqlite | Dialect::Mongo) {
        return None;
    }
    if is_fn {
        // OUT parameters are results, not arguments, when calling a function
        params.retain(|p| {
            let u = p.to_ascii_uppercase();
            !(u.starts_with("OUT ") || u.ends_with(" OUTPUT"))
        });
    }
    let args = |named: bool| -> String {
        if params.is_empty() {
            return String::new();
        }
        let n = params.len();
        let lines: Vec<String> = params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let sep = if i + 1 < n { "," } else { "" };
                let value = if named {
                    let pname = p.split_whitespace().next().unwrap_or("");
                    if pname.starts_with('@') {
                        format!("{pname} = NULL")
                    } else {
                        "NULL".to_string()
                    }
                } else {
                    "NULL".to_string()
                };
                format!("    {value}{sep}  -- {p}")
            })
            .collect();
        format!("\n{}\n", lines.join("\n"))
    };
    let sql = match (d, is_proc) {
        (Dialect::MsSql, true) => {
            if params.is_empty() {
                format!("EXEC {target};")
            } else {
                // named arguments, each on its own line; the last comment sits after `;`
                let a = args(true);
                let a = a.trim_end_matches('\n');
                match a.rfind("  -- ") {
                    Some(i) => format!("EXEC {target}{};{}", &a[..i], &a[i..]),
                    None => format!("EXEC {target}{a};"),
                }
            }
        }
        (_, true) => format!("CALL {target}({});", args(false)),
        (_, false) if is_table_fn => format!("SELECT * FROM {target}({});", args(false)),
        (_, false) => format!("SELECT {target}({});", args(false)),
    };
    Some(sql)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(n: &str, t: &str, nullable: bool, pk: bool) -> ColumnInfo {
        ColumnInfo { name: n.into(), data_type: t.into(), nullable, primary_key: pk, default: None }
    }

    fn table() -> TableInfo {
        let mut t = TableInfo::new("items", ObjectKind::Table);
        t.columns = vec![col("id", "integer", false, true), col("name", "text", true, false), col("qty", "integer", true, false)];
        t.indexes = vec![IndexInfo { name: "items_pkey".into(), columns: vec!["id".into()], unique: true, primary: true }];
        t
    }

    fn edits(t: &TableInfo) -> Vec<ColumnEdit> {
        t.columns.iter().map(|c| ColumnEdit { original: Some(c.name.clone()), column: c.clone(), dropped: false }).collect()
    }

    #[test]
    fn create_table_postgres() {
        let mut t = table();
        t.foreign_keys.push(ForeignKey { name: "fk_x".into(), columns: vec!["qty".into()], ref_schema: "public".into(), ref_table: "q".into(), ref_columns: vec!["id".into()], on_delete: "CASCADE".into() });
        let s = script(Dialect::Postgres, &create_table(Dialect::Postgres, "public", &t));
        assert!(s.contains("CREATE TABLE public.items (\n    id integer NOT NULL,"));
        assert!(s.contains("CONSTRAINT items_pkey PRIMARY KEY (id)"));
        assert!(s.contains("CONSTRAINT fk_x FOREIGN KEY (qty) REFERENCES public.q (id) ON DELETE CASCADE"));
    }

    #[test]
    fn alter_postgres_rename_type_null() {
        let t = table();
        let mut e = edits(&t);
        e[1].column.name = "title".into();
        e[2].column.data_type = "bigint".into();
        e[2].column.nullable = false;
        e.push(ColumnEdit { original: None, column: col("note", "text", true, false), dropped: false });
        let s = alter_table(Dialect::Postgres, "public", &t, "items", &e, &t.indexes, &[]);
        let sql: Vec<&str> = s.iter().map(|s| s.sql.as_str()).collect();
        assert_eq!(
            sql,
            vec![
                "ALTER TABLE public.items RENAME COLUMN name TO title",
                "ALTER TABLE public.items ALTER COLUMN qty TYPE bigint USING qty::bigint",
                "ALTER TABLE public.items ALTER COLUMN qty SET NOT NULL",
                "ALTER TABLE public.items ADD COLUMN note text",
            ]
        );
    }

    #[test]
    fn alter_mysql_uses_change_and_modify() {
        let t = table();
        let mut e = edits(&t);
        e[1].column.name = "title".into();
        e[2].column.nullable = false;
        let s = alter_table(Dialect::MySql, "shop", &t, "items", &e, &t.indexes, &[]);
        assert_eq!(s[0].sql, "ALTER TABLE shop.items CHANGE COLUMN name title text");
        assert_eq!(s[1].sql, "ALTER TABLE shop.items MODIFY COLUMN qty integer NOT NULL");
    }

    #[test]
    fn alter_mssql_rename_and_table_rename() {
        let t = table();
        let mut e = edits(&t);
        e[1].column.name = "title".into();
        let s = alter_table(Dialect::MsSql, "dbo", &t, "products", &e, &t.indexes, &[]);
        assert_eq!(s[0].sql, "EXEC sp_rename 'dbo.items.name', 'title', 'COLUMN'");
        assert_eq!(s[1].sql, "EXEC sp_rename 'dbo.items', 'products'");
    }

    #[test]
    fn sqlite_type_change_rebuilds() {
        let t = table();
        let mut e = edits(&t);
        e[2].column.data_type = "real".into();
        e[1].dropped = true;
        let s = alter_table(Dialect::Sqlite, "main", &t, "items", &e, &t.indexes, &[]);
        let all = script(Dialect::Sqlite, &s);
        assert!(all.contains("CREATE TABLE items__databug_new ("), "{all}");
        assert!(all.contains("INSERT INTO items__databug_new (id, qty) SELECT id, qty FROM items"));
        assert!(all.contains("ALTER TABLE items__databug_new RENAME TO items"));
    }

    #[test]
    fn sqlite_simple_changes_stay_in_place() {
        let t = table();
        let mut e = edits(&t);
        e[1].column.name = "title".into();
        e.push(ColumnEdit { original: None, column: col("note", "text", true, false), dropped: false });
        let s = alter_table(Dialect::Sqlite, "main", &t, "items", &e, &t.indexes, &[]);
        assert_eq!(s[0].sql, "ALTER TABLE items RENAME COLUMN name TO title");
        assert_eq!(s[1].sql, "ALTER TABLE items ADD COLUMN note text");
    }

    #[test]
    fn index_changes() {
        let t = table();
        let mut idx = t.indexes.clone();
        idx.push(IndexInfo { name: "items_name".into(), columns: vec!["name".into()], unique: false, primary: false });
        let s = alter_table(Dialect::MsSql, "dbo", &t, "items", &edits(&t), &idx, &[]);
        assert_eq!(s[0].sql, "CREATE INDEX items_name ON dbo.items (name)");
    }

    #[test]
    fn inserts() {
        let rows = vec![vec![Some("1".into()), Some("it's".into())], vec![Some("2".into()), None]];
        let s = insert_batches(Dialect::Postgres, "t", &["id".into(), "name".into()], &[true, false], &rows, 500);
        assert_eq!(s, vec!["INSERT INTO t (id, name) VALUES\n(1, 'it''s'),\n(2, NULL)"]);
    }

    #[test]
    fn exec_templates() {
        assert_eq!(exec_template(Dialect::MsSql, "dbo", "usp_refresh", ObjectKind::Procedure, "").unwrap(), "EXEC dbo.usp_refresh;");
        let t = exec_template(Dialect::MsSql, "etl", "load", ObjectKind::Procedure, "@id int, @amount decimal(10,2), @out nvarchar OUTPUT").unwrap();
        assert_eq!(t, "EXEC etl.load\n    @id = NULL,  -- @id int\n    @amount = NULL,  -- @amount decimal(10,2)\n    @out = NULL;  -- @out nvarchar OUTPUT");
        assert_eq!(exec_template(Dialect::Postgres, "public", "archive", ObjectKind::Procedure, "days integer").unwrap(), "CALL public.archive(\n    NULL  -- days integer\n);");
        assert_eq!(exec_template(Dialect::Postgres, "public", "f", ObjectKind::TableFunction, "").unwrap(), "SELECT * FROM public.f();");
        assert_eq!(exec_template(Dialect::MySql, "shop", "total", ObjectKind::Function, "a INT, OUT b INT").unwrap(), "SELECT shop.total(\n    NULL  -- a INT\n);");
        assert!(exec_template(Dialect::Postgres, "public", "t", ObjectKind::Table, "").is_none());
    }
}
