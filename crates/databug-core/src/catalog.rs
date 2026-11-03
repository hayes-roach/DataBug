use serde::{Deserialize, Serialize};

/// Every kind of object DataBug shows. Relations (tables, views, materialized
/// views, collections) live in `SchemaInfo::tables`; everything else lives in
/// `SchemaInfo::objects`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ObjectKind {
    Table,
    View,
    MaterializedView,
    Collection,
    Procedure,
    Function,
    TableFunction,
    Aggregate,
    Trigger,
    Sequence,
    Synonym,
    Event,
    ClrProcedure,
    ClrFunction,
    ClrTableFunction,
    ClrAggregate,
    ClrTrigger,
    Assembly,
}

impl ObjectKind {
    pub const DISPLAY_ORDER: [ObjectKind; 18] = [
        ObjectKind::Table,
        ObjectKind::View,
        ObjectKind::MaterializedView,
        ObjectKind::Collection,
        ObjectKind::Procedure,
        ObjectKind::Function,
        ObjectKind::TableFunction,
        ObjectKind::Aggregate,
        ObjectKind::Trigger,
        ObjectKind::Sequence,
        ObjectKind::Synonym,
        ObjectKind::Event,
        ObjectKind::ClrProcedure,
        ObjectKind::ClrFunction,
        ObjectKind::ClrTableFunction,
        ObjectKind::ClrAggregate,
        ObjectKind::ClrTrigger,
        ObjectKind::Assembly,
    ];

    pub fn is_relation(self) -> bool {
        matches!(self, ObjectKind::Table | ObjectKind::View | ObjectKind::MaterializedView | ObjectKind::Collection)
    }

    pub fn is_clr(self) -> bool {
        matches!(self, ObjectKind::ClrProcedure | ObjectKind::ClrFunction | ObjectKind::ClrTableFunction | ObjectKind::ClrAggregate | ObjectKind::ClrTrigger | ObjectKind::Assembly)
    }

    /// Objects whose definition is SQL text that can be fetched and edited.
    pub fn has_source(self) -> bool {
        !matches!(self, ObjectKind::Table | ObjectKind::Collection)
    }

    pub fn group_label(self) -> &'static str {
        match self {
            ObjectKind::Table => "tables",
            ObjectKind::View => "views",
            ObjectKind::MaterializedView => "materialized views",
            ObjectKind::Collection => "collections",
            ObjectKind::Procedure => "procedures",
            ObjectKind::Function => "functions",
            ObjectKind::TableFunction => "table functions",
            ObjectKind::Aggregate => "aggregates",
            ObjectKind::Trigger => "triggers",
            ObjectKind::Sequence => "sequences",
            ObjectKind::Synonym => "synonyms",
            ObjectKind::Event => "events",
            ObjectKind::ClrProcedure => "CLR procedures",
            ObjectKind::ClrFunction => "CLR functions",
            ObjectKind::ClrTableFunction => "CLR table functions",
            ObjectKind::ClrAggregate => "CLR aggregates",
            ObjectKind::ClrTrigger => "CLR triggers",
            ObjectKind::Assembly => "assemblies",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ObjectKind::Table => "table",
            ObjectKind::View => "view",
            ObjectKind::MaterializedView => "materialized view",
            ObjectKind::Collection => "collection",
            ObjectKind::Procedure => "procedure",
            ObjectKind::Function => "function",
            ObjectKind::TableFunction => "table function",
            ObjectKind::Aggregate => "aggregate",
            ObjectKind::Trigger => "trigger",
            ObjectKind::Sequence => "sequence",
            ObjectKind::Synonym => "synonym",
            ObjectKind::Event => "event",
            ObjectKind::ClrProcedure => "CLR procedure",
            ObjectKind::ClrFunction => "CLR function",
            ObjectKind::ClrTableFunction => "CLR table function",
            ObjectKind::ClrAggregate => "CLR aggregate",
            ObjectKind::ClrTrigger => "CLR trigger",
            ObjectKind::Assembly => "assembly",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    /// `CASCADE`, `SET NULL`, `RESTRICT`, ... or empty for the default (NO ACTION).
    #[serde(default)]
    pub on_delete: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    pub kind: ObjectKind,
    pub columns: Vec<ColumnInfo>,
    #[serde(default)]
    pub indexes: Vec<IndexInfo>,
    #[serde(default)]
    pub foreign_keys: Vec<ForeignKey>,
}

impl TableInfo {
    pub fn new(name: impl Into<String>, kind: ObjectKind) -> Self {
        TableInfo { name: name.into(), kind, columns: Vec::new(), indexes: Vec::new(), foreign_keys: Vec::new() }
    }

    pub fn column(&self, name: &str) -> Option<&ColumnInfo> {
        self.columns.iter().find(|c| c.name.eq_ignore_ascii_case(name))
    }

    pub fn primary_key(&self) -> Vec<&ColumnInfo> {
        self.columns.iter().filter(|c| c.primary_key).collect()
    }

    pub fn primary_index(&self) -> Option<&IndexInfo> {
        self.indexes.iter().find(|i| i.primary)
    }

    pub fn is_fk_column(&self, col: &str) -> bool {
        self.foreign_keys.iter().any(|f| f.columns.iter().any(|c| c.eq_ignore_ascii_case(col)))
    }
}

/// A non-relation object: routine, trigger, sequence, synonym, event, CLR object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbObject {
    pub name: String,
    pub kind: ObjectKind,
    /// Argument list for routines, target table for triggers, base object for synonyms.
    #[serde(default)]
    pub detail: String,
    /// Engine identity used to fetch the definition (e.g. a Postgres `regprocedure`).
    #[serde(default)]
    pub ident: String,
    /// Table the object belongs to (triggers).
    #[serde(default)]
    pub table: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaInfo {
    pub name: String,
    pub tables: Vec<TableInfo>,
    #[serde(default)]
    pub objects: Vec<DbObject>,
}

impl SchemaInfo {
    pub fn table(&self, name: &str) -> Option<&TableInfo> {
        self.tables.iter().find(|t| t.name.eq_ignore_ascii_case(name))
    }

    pub fn count(&self, kind: ObjectKind) -> usize {
        if kind.is_relation() {
            self.tables.iter().filter(|t| t.kind == kind).count()
        } else {
            self.objects.iter().filter(|o| o.kind == kind).count()
        }
    }
}

/// Local model of a database's structure. Introspected once, cached on disk,
/// and used for completion, inspections, the explorer, ER diagrams and compare.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catalog {
    pub schemas: Vec<SchemaInfo>,
    /// Schema used to resolve unqualified names (`public`, `dbo`, `main`, the MySQL database).
    pub default_schema: String,
    /// Unix seconds of the last introspection.
    #[serde(default)]
    pub introspected_at: u64,
}

impl Catalog {
    pub fn schema(&self, name: &str) -> Option<&SchemaInfo> {
        self.schemas.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Resolve a possibly schema-qualified table name.
    pub fn find_table(&self, schema: Option<&str>, name: &str) -> Option<&TableInfo> {
        if let Some(s) = schema {
            return self.schema(s)?.table(name);
        }
        if let Some(t) = self.schema(&self.default_schema).and_then(|s| s.table(name)) {
            return Some(t);
        }
        self.schemas.iter().flat_map(|s| s.tables.iter()).find(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// Schema that holds a table found by `find_table`.
    pub fn schema_of(&self, table: &TableInfo) -> Option<&SchemaInfo> {
        self.schemas.iter().find(|s| s.tables.iter().any(|t| std::ptr::eq(t, table)))
    }

    pub fn tables(&self) -> impl Iterator<Item = (&SchemaInfo, &TableInfo)> {
        self.schemas.iter().flat_map(|s| s.tables.iter().map(move |t| (s, t)))
    }

    pub fn table_count(&self) -> usize {
        self.schemas.iter().map(|s| s.tables.len() + s.objects.len()).sum()
    }
}
