//! # theta-sql
//!
//! Pure SQL intelligence for Theta: statement splitting, formatting, completion,
//! foreign-key join suggestions, plan analysis, linting, SQL generation and export.
//! No I/O and no async: everything is a function of its arguments so it can run on
//! every keystroke and be tested without a database.

pub mod admin;
pub mod analysis;
pub mod complete;
pub mod ddl;
pub mod export;
pub mod fk;
pub mod format;
pub mod generate;
pub mod keywords;
pub mod lexer;
pub mod lint;
pub mod params;
pub mod plan;
pub(crate) mod scope;
pub mod script;
pub mod syntax;
pub mod templates;

/// Realistic catalogs used by this crate's tests; exposed so other crates can reuse them.
#[doc(hidden)]
pub mod fixtures;

pub use complete::{CompletionItem, CompletionKind, CompletionList, CompletionRequest, complete};
pub use export::{ExportEncoder, ExportFormat, export};
pub use fk::{
    JoinSuggestion, TableRef, VirtualForeignKey, draft_as_table, infer_foreign_keys, infer_foreign_keys_for,
    suggest_joins, tables_in_scope,
};
pub use format::{FormatOptions, format};
pub use generate::{build_delete, build_insert, build_update, select_top, sql_literal, table_ddl};
pub use lint::{Diagnostic, Severity, is_incomplete_while_typing, lint};
pub use plan::{Insight, PlanSummary, analyze_plan};
pub use script::{StatementClass, StatementSpan, classify, split_statements, statement_at};
