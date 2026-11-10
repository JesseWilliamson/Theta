//! # quarry-sql
//!
//! Pure SQL intelligence for Quarry: statement splitting, formatting, completion,
//! foreign-key join suggestions, plan analysis, linting, SQL generation and export.
//! No I/O and no async: everything is a function of its arguments so it can run on
//! every keystroke and be tested without a database.

pub mod keywords;
pub mod lexer;
pub mod script;

pub use script::{StatementClass, StatementSpan, classify, split_statements, statement_at};
