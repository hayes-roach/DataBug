//! Schema compare: the differences between a source schema and a target
//! schema, each with the DDL that makes the target match the source.

use crate::ddl::{self, script, ColumnEdit, Stmt};
use databug_core::{Catalog, DbObject, Dialect, ObjectKind, SchemaInfo, TableInfo};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiffKind {
    /// In the source, missing from the target.
    Added,
    /// Only in the target.
    Removed,
    /// In both, but different.
    Changed,
}

#[derive(Debug, Clone)]
pub struct DiffItem {
    pub kind: DiffKind,
    pub object: ObjectKind,
    pub name: String,
    /// One line per difference ("column qty: integer → bigint").
    pub details: Vec<String>,
    pub stmts: Vec<Stmt>,
    /// Drops something or changes a type: may lose data.
    pub destructive: bool,
}

/// Definitions fetched for views and routines, keyed by `kind:name:detail`.
pub type Sources = HashMap<String, String>;

pub fn source_key(kind: ObjectKind, name: &str, detail: &str) -> String {
    format!("{kind:?}:{}:{}", name.to_lowercase(), detail.to_lowercase())
}

fn norm_source(s: &str) -> String {
    // compare definitions ignoring whitespace, case of keywords and the CREATE/ALTER preamble
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let body = collapsed.find(" as ").map(|i| collapsed[i..].to_string()).unwrap_or(collapsed);
    let mut b = body.trim_end().to_string();
    loop {
        let t = b.trim_end_matches(';').trim_end();
        match t.strip_suffix(" go") {
            Some(rest) => b = rest.to_string(),
            None => return t.to_string(),
        }
    }
}

/// Compare `source.schema(src_schema)` to `target.schema(dst_schema)` and
/// produce DDL in the target's dialect. Table names match case-insensitively.
pub fn diff(source: &Catalog, src_schema: &str, target: &Catalog, dst_schema: &str, d: Dialect, src_sources: &Sources, dst_sources: &Sources) -> Vec<DiffItem> {
    let empty = SchemaInfo::default();
    let s = source.schema(src_schema).unwrap_or(&empty);
    let t = target.schema(dst_schema).unwrap_or(&empty);
    let mut out = Vec::new();

    // ---- relations ----
    for st in &s.tables {
        match t.table(&st.name) {
            None => {
                let stmts = match st.kind {
                    ObjectKind::Table => ddl::create_table(d, dst_schema, &retarget(st, src_schema, dst_schema)),
                    ObjectKind::View | ObjectKind::MaterializedView => match src_sources.get(&source_key(st.kind, &st.name, "")) {
                        Some(src) => vec![Stmt::new(src.trim().trim_end_matches(';').replace(&format!("{src_schema}."), &format!("{dst_schema}.")))],
                        None => vec![Stmt::comment(format!("Turn on \"Compare definitions\" to generate CREATE for {} {}", st.kind.label(), st.name))],
                    },
                    _ => vec![],
                };
                out.push(DiffItem { kind: DiffKind::Added, object: st.kind, name: st.name.clone(), details: vec![format!("{} columns", st.columns.len())], stmts, destructive: false });
            }
            Some(tt) if st.kind == ObjectKind::Table && tt.kind == ObjectKind::Table => {
                if let Some(item) = diff_table(d, dst_schema, st, tt, src_schema) {
                    out.push(item);
                }
            }
            Some(tt) if st.kind == tt.kind && st.kind != ObjectKind::Collection => {
                let k = source_key(st.kind, &st.name, "");
                if let (Some(a), Some(b)) = (src_sources.get(&k), dst_sources.get(&k)) {
                    if norm_source(a) != norm_source(b) {
                        out.push(DiffItem { kind: DiffKind::Changed, object: st.kind, name: st.name.clone(), details: vec!["definition differs".into()], stmts: vec![Stmt::new(a.trim().trim_end_matches(';'))], destructive: false });
                    }
                }
            }
            Some(tt) if st.kind != tt.kind => {
                out.push(DiffItem {
                    kind: DiffKind::Changed,
                    object: st.kind,
                    name: st.name.clone(),
                    details: vec![format!("is a {} in the source but a {} in the target", st.kind.label(), tt.kind.label())],
                    stmts: vec![Stmt::comment("Recreate this object by hand: its type differs")],
                    destructive: true,
                });
            }
            _ => {}
        }
    }
    for tt in &t.tables {
        if s.table(&tt.name).is_none() {
            out.push(DiffItem {
                kind: DiffKind::Removed,
                object: tt.kind,
                name: tt.name.clone(),
                details: vec![format!("{} columns", tt.columns.len())],
                stmts: if tt.kind == ObjectKind::Collection { vec![] } else { vec![ddl::drop_relation(d, dst_schema, &tt.name, tt.kind)] },
                destructive: true,
            });
        }
    }

    // ---- routines and other objects ----
    let find = |list: &[DbObject], o: &DbObject| list.iter().find(|x| x.kind == o.kind && x.name.eq_ignore_ascii_case(&o.name) && x.detail.eq_ignore_ascii_case(&o.detail)).cloned();
    for so in &s.objects {
        let k = source_key(so.kind, &so.name, &so.detail);
        match find(&t.objects, so) {
            None => {
                let stmts = match src_sources.get(&k) {
                    Some(src) => vec![Stmt::new(src.trim().trim_end_matches(';'))],
                    None => vec![Stmt::comment(format!("Turn on \"Compare definitions\" to generate CREATE for {} {}", so.kind.label(), so.name))],
                };
                out.push(DiffItem { kind: DiffKind::Added, object: so.kind, name: display(so), details: vec![], stmts, destructive: false });
            }
            Some(_) => {
                if let (Some(a), Some(b)) = (src_sources.get(&k), dst_sources.get(&k)) {
                    if norm_source(a) != norm_source(b) {
                        out.push(DiffItem { kind: DiffKind::Changed, object: so.kind, name: display(so), details: vec!["definition differs".into()], stmts: vec![Stmt::new(a.trim().trim_end_matches(';'))], destructive: false });
                    }
                }
            }
        }
    }
    for to in &t.objects {
        if find(&s.objects, to).is_none() {
            out.push(DiffItem { kind: DiffKind::Removed, object: to.kind, name: display(to), details: vec![], stmts: vec![drop_object(d, dst_schema, to)], destructive: true });
        }
    }
    out.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.object.cmp(&b.object)).then(a.name.cmp(&b.name)));
    out
}

fn display(o: &DbObject) -> String {
    if o.detail.is_empty() || o.kind == ObjectKind::Trigger {
        o.name.clone()
    } else {
        format!("{}({})", o.name, o.detail)
    }
}

fn retarget(t: &TableInfo, from: &str, to: &str) -> TableInfo {
    let mut t = t.clone();
    for fk in &mut t.foreign_keys {
        if fk.ref_schema.eq_ignore_ascii_case(from) {
            fk.ref_schema = to.to_string();
        }
    }
    t
}

pub fn drop_object(d: Dialect, schema: &str, o: &DbObject) -> Stmt {
    let q = ddl::qualified(d, schema, &o.name);
    let sql = match o.kind {
        ObjectKind::Procedure | ObjectKind::ClrProcedure => format!("DROP PROCEDURE {q}{}", if d == Dialect::Postgres { format!("({})", o.detail) } else { String::new() }),
        ObjectKind::Function | ObjectKind::TableFunction | ObjectKind::ClrFunction | ObjectKind::ClrTableFunction => {
            format!("DROP FUNCTION {q}{}", if d == Dialect::Postgres { format!("({})", o.detail) } else { String::new() })
        }
        ObjectKind::Aggregate | ObjectKind::ClrAggregate => format!("DROP AGGREGATE {q}{}", if d == Dialect::Postgres { format!("({})", o.detail) } else { String::new() }),
        ObjectKind::Trigger | ObjectKind::ClrTrigger => match (d, &o.table) {
            (Dialect::Postgres, Some(t)) => format!("DROP TRIGGER {} ON {}", d.quote_ident(&o.name), ddl::qualified(d, schema, t)),
            _ => format!("DROP TRIGGER {q}"),
        },
        ObjectKind::Sequence => format!("DROP SEQUENCE {q}"),
        ObjectKind::Synonym => format!("DROP SYNONYM {q}"),
        ObjectKind::Event => format!("DROP EVENT {q}"),
        ObjectKind::Assembly => format!("DROP ASSEMBLY {}", d.quote_ident(&o.name)),
        k => return Stmt::comment(format!("Drop {} {} by hand", k.label(), o.name)),
    };
    Stmt::new(sql)
}

fn diff_table(d: Dialect, schema: &str, src: &TableInfo, dst: &TableInfo, src_schema: &str) -> Option<DiffItem> {
    let src = retarget(src, src_schema, schema);
    let mut details = Vec::new();
    let mut destructive = false;
    // build the edit list that turns `dst` into `src`
    let mut edits: Vec<ColumnEdit> = Vec::new();
    for dc in &dst.columns {
        match src.column(&dc.name) {
            Some(sc) => {
                if !ddl::same_type(&sc.data_type, &dc.data_type) {
                    details.push(format!("column {}: {} → {}", dc.name, dc.data_type, sc.data_type));
                    destructive = true;
                }
                if sc.nullable != dc.nullable {
                    details.push(format!("column {}: {} → {}", dc.name, if dc.nullable { "NULL" } else { "NOT NULL" }, if sc.nullable { "NULL" } else { "NOT NULL" }));
                }
                if sc.default.as_deref().map(str::trim) != dc.default.as_deref().map(str::trim) {
                    details.push(format!("column {}: default {} → {}", dc.name, dc.default.as_deref().unwrap_or("none"), sc.default.as_deref().unwrap_or("none")));
                }
                if sc.primary_key != dc.primary_key {
                    details.push(format!("column {}: primary key {}", dc.name, if sc.primary_key { "added" } else { "removed" }));
                }
                edits.push(ColumnEdit { original: Some(dc.name.clone()), column: ColumnInfo { name: dc.name.clone(), ..sc.clone() }, dropped: false });
            }
            None => {
                details.push(format!("column {} only in target", dc.name));
                destructive = true;
                edits.push(ColumnEdit { original: Some(dc.name.clone()), column: dc.clone(), dropped: true });
            }
        }
    }
    for sc in &src.columns {
        if dst.column(&sc.name).is_none() {
            details.push(format!("column {} missing in target", sc.name));
            edits.push(ColumnEdit { original: None, column: sc.clone(), dropped: false });
        }
    }
    // indexes and keys compare by columns, keeping the target's names where they match
    let mut indexes = Vec::new();
    for si in &src.indexes {
        match dst.indexes.iter().find(|di| di.primary == si.primary && cols_eq(&di.columns, &si.columns) && di.unique == si.unique) {
            Some(di) => indexes.push(di.clone()),
            None => {
                if !si.primary {
                    details.push(format!("index {} ({}) missing in target", si.name, si.columns.join(", ")));
                }
                indexes.push(si.clone());
            }
        }
    }
    for di in &dst.indexes {
        if !di.primary && !indexes.iter().any(|i| i == di) {
            details.push(format!("index {} only in target", di.name));
        }
    }
    let mut fks = Vec::new();
    for sf in &src.foreign_keys {
        match dst.foreign_keys.iter().find(|df| cols_eq(&df.columns, &sf.columns) && df.ref_table.eq_ignore_ascii_case(&sf.ref_table) && cols_eq(&df.ref_columns, &sf.ref_columns)) {
            Some(df) if df.on_delete == sf.on_delete => fks.push(df.clone()),
            _ => {
                details.push(format!("foreign key ({}) → {} missing or different in target", sf.columns.join(", "), sf.ref_table));
                fks.push(sf.clone());
            }
        }
    }
    for df in &dst.foreign_keys {
        if !fks.iter().any(|f| f == df) {
            details.push(format!("foreign key {} only in target", df.name));
        }
    }
    if details.is_empty() {
        return None;
    }
    let stmts = ddl::alter_table(d, schema, dst, &dst.name, &edits, &indexes, &fks);
    Some(DiffItem { kind: DiffKind::Changed, object: ObjectKind::Table, name: dst.name.clone(), details, stmts, destructive })
}

use databug_core::ColumnInfo;

fn cols_eq(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// The full migration script for the selected items, in a safe order:
/// drops of dependent objects first, then tables, then everything else.
pub fn migration_script(d: Dialect, items: &[&DiffItem]) -> String {
    let rank = |i: &DiffItem| -> u8 {
        match (i.kind, i.object) {
            (DiffKind::Removed, k) if !k.is_relation() => 0,
            (DiffKind::Removed, ObjectKind::View | ObjectKind::MaterializedView) => 1,
            (DiffKind::Removed, _) => 2,
            (DiffKind::Added, ObjectKind::Table) => 3,
            (DiffKind::Changed, ObjectKind::Table) => 4,
            (_, ObjectKind::View | ObjectKind::MaterializedView) => 5,
            _ => 6,
        }
    };
    let mut sorted: Vec<&&DiffItem> = items.iter().collect();
    sorted.sort_by_key(|i| rank(i));
    let mut out = String::from("-- Migration generated by DataBug schema compare. Review before running.\n\n");
    for i in sorted {
        out.push_str(&format!("-- {} {} {}\n", match i.kind { DiffKind::Added => "create", DiffKind::Removed => "drop", DiffKind::Changed => "alter" }, i.object.label(), i.name));
        out.push_str(&script(d, &i.stmts));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use databug_core::{ForeignKey, IndexInfo};

    fn col(n: &str, t: &str) -> ColumnInfo {
        ColumnInfo { name: n.into(), data_type: t.into(), nullable: true, primary_key: n == "id", default: None }
    }

    fn cat(tables: Vec<TableInfo>, objects: Vec<DbObject>) -> Catalog {
        Catalog { schemas: vec![SchemaInfo { name: "public".into(), tables, objects }], default_schema: "public".into(), introspected_at: 0 }
    }

    #[test]
    fn finds_added_removed_changed() {
        let mut a = TableInfo::new("orders", ObjectKind::Table);
        a.columns = vec![col("id", "integer"), col("total", "numeric(12,2)"), col("note", "text")];
        a.indexes = vec![IndexInfo { name: "o_total".into(), columns: vec!["total".into()], unique: false, primary: false }];
        a.foreign_keys = vec![ForeignKey { name: "fk".into(), columns: vec!["id".into()], ref_schema: "public".into(), ref_table: "x".into(), ref_columns: vec!["id".into()], on_delete: String::new() }];
        let mut b = TableInfo::new("orders", ObjectKind::Table);
        b.columns = vec![col("id", "int4"), col("total", "integer"), col("legacy", "text")];
        let mut only_src = TableInfo::new("customers", ObjectKind::Table);
        only_src.columns = vec![col("id", "integer")];
        let mut only_dst = TableInfo::new("old_stuff", ObjectKind::Table);
        only_dst.columns = vec![col("id", "integer")];
        let f = DbObject { name: "f".into(), kind: ObjectKind::Function, detail: "x integer".into(), ident: String::new(), table: None };
        let src = cat(vec![a, only_src], vec![f.clone()]);
        let dst = cat(vec![b, only_dst], vec![]);
        let mut srcs = Sources::new();
        srcs.insert(source_key(ObjectKind::Function, "f", "x integer"), "CREATE OR REPLACE FUNCTION public.f(x integer) RETURNS int AS $$ select x $$ LANGUAGE sql".into());
        let items = diff(&src, "public", &dst, "public", Dialect::Postgres, &srcs, &Sources::new());
        let names: Vec<(DiffKind, &str)> = items.iter().map(|i| (i.kind, i.name.as_str())).collect();
        assert!(names.contains(&(DiffKind::Added, "customers")));
        assert!(names.contains(&(DiffKind::Removed, "old_stuff")));
        assert!(names.contains(&(DiffKind::Changed, "orders")));
        assert!(names.contains(&(DiffKind::Added, "f(x integer)")));
        let orders = items.iter().find(|i| i.name == "orders").unwrap();
        let sql = script(Dialect::Postgres, &orders.stmts);
        assert!(sql.contains("DROP COLUMN legacy"), "{sql}");
        assert!(sql.contains("ADD COLUMN note text"));
        assert!(sql.contains("ALTER COLUMN total TYPE numeric(12,2)"));
        assert!(sql.contains("CREATE INDEX o_total"));
        assert!(sql.contains("ADD CONSTRAINT fk FOREIGN KEY (id)"));
        assert!(!sql.contains("ALTER COLUMN id TYPE"), "int4 and integer are the same type");
        let all: Vec<&DiffItem> = items.iter().collect();
        let m = migration_script(Dialect::Postgres, &all);
        assert!(m.find("DROP TABLE public.old_stuff").unwrap() < m.find("CREATE TABLE public.customers").unwrap());
    }

    #[test]
    fn identical_schemas_have_no_diff() {
        let mut a = TableInfo::new("t", ObjectKind::Table);
        a.columns = vec![col("id", "integer")];
        let c = cat(vec![a], vec![]);
        assert!(diff(&c, "public", &c, "public", Dialect::Postgres, &Sources::new(), &Sources::new()).is_empty());
    }

    #[test]
    fn definitions_compare_ignoring_whitespace() {
        assert_eq!(norm_source("CREATE OR REPLACE VIEW v AS\n  SELECT 1;"), norm_source("create view v as select 1"));
    }
}
