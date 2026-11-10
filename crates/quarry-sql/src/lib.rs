//! # quarry-sql
//!
//! Pure SQL intelligence for Quarry: statement splitting, formatting, completion,
//! foreign-key join suggestions, plan analysis, linting, SQL generation and export.
//! No I/O and no async: everything is a function of its arguments so it can run on
//! every keystroke and be tested without a database.

pub mod complete;
pub mod export;
pub mod fk;
pub mod generate;
pub mod keywords;
pub mod lexer;
pub mod lint;
pub mod plan;
pub mod script;
pub(crate) mod scope;

#[cfg(test)]
pub(crate) mod fixtures;

pub use complete::{CompletionItem, CompletionKind, CompletionList, CompletionRequest, complete};
pub use export::{ExportFormat, export};
pub use fk::{JoinSuggestion, TableRef, VirtualForeignKey, infer_foreign_keys, suggest_joins, tables_in_scope};
pub use generate::{build_delete, build_insert, build_update, select_top, sql_literal, table_ddl};
pub use lint::{Diagnostic, Severity, lint};
pub use plan::{Insight, PlanSummary, analyze_plan};
pub use script::{StatementClass, StatementSpan, classify, split_statements, statement_at};
