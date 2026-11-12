//! A lossless, error-tolerant SQL syntax tree.
//!
//! Three properties are the contract; everything else in this module serves them.
//!
//! 1. **Every node has a byte span, and the leaves tile the input.** `parse(sql)` returns a root whose span is
//!    `0..sql.len()`, whose leaves (tokens, whitespace and comments included) concatenate back to `sql`, and in
//!    which every node's children tile the node's own span. Cursor to node and node to text are therefore
//!    plain interval arithmetic.
//! 2. **No false errors.** What the parser does not understand becomes an [`Kind::Unknown`] node and no
//!    diagnostic. A [`ParseError`] is only produced where the text cannot be valid in the dialect (see
//!    `errors.rs`'s catalogue in the module docs of [`parser`]).
//! 3. **Recovery keeps the intact parts.** A statement is parsed on its own (the script splitter decides
//!    where it ends), unknown or broken regions are skipped up to the next place where the grammar can
//!    resume (`,`, `)`, a clause keyword), and a missing piece never prevents the siblings from being parsed.
//!
//! The grammar is a hand-written recursive descent over [`crate::lexer`] tokens. Queries and DML are parsed in
//! depth; DDL and everything else only far enough to know the statement kind and the objects named in it.

mod dml;
mod expr;
mod node;
mod parser;
mod query;
mod words;

pub use node::{Kind, Node, ParseError};
pub use parser::{Parse, parse};

#[cfg(test)]
mod tests;
