//! SQL intelligence for DataBug: tolerant lexing, statement splitting,
//! schema-aware completion, inspections, safety checks and formatting.
//!
//! This crate has no UI or driver dependencies so it can also back a
//! language server (LSP) for other editors.

pub mod analyze;
pub mod complete;
pub mod ddl;
pub mod diff;
pub mod format;
pub mod lexer;
pub mod mongo;
pub mod rename;
pub mod split;

pub use analyze::{classify, dangers, inspect, single_table_source, table_refs, Danger, Diagnostic, Severity, StatementKind, TableRef};
pub use complete::{complete, Completion, CompletionItem, ItemKind};
pub use format::format_sql;
pub use lexer::{tokenize, TokKind, Token};
pub use split::{split_statements, statement_at};

#[cfg(test)]
mod tests {
    #[test]
    fn keywords_sorted() {
        let mut k = crate::lexer::KEYWORDS.to_vec();
        k.sort();
        assert_eq!(k, crate::lexer::KEYWORDS);
    }
}
