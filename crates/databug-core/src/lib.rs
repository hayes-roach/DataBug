//! Core types shared by every DataBug crate: connection settings, the schema
//! model (catalog), compact result storage and query plans.

pub mod catalog;
pub mod config;
pub mod plan;
pub mod result;

pub use catalog::{Catalog, ColumnInfo, DbObject, ForeignKey, IndexInfo, ObjectKind, SchemaInfo, TableInfo};
pub use config::{Capabilities, ConnectionConfig, DbCategory, DbKind, Dialect, MsAuth, SslMode};
pub use plan::{Plan, PlanNode};
pub use result::{ColumnMeta, ResultSet};

/// Events streamed from a running statement back to the UI.
#[derive(Debug)]
pub enum QueryEvent {
    /// A new result set begins with these columns.
    Columns(Vec<ColumnMeta>),
    /// A chunk of rows for the current result set (same column layout).
    Rows(ResultSet),
    /// The result set hit the fetch limit and was cut short.
    Truncated,
    /// A statement that returns no rows finished.
    Affected(u64),
    /// Informational server message (NOTICE, PRINT, warnings).
    Message(String),
}
