//! Theta: a keyboard-first database IDE.
//!
//! The binary (`main.rs`) is a thin wrapper; everything lives in this library so
//! the integration crates can use the shell's seams ([`workspace::Workspace`],
//! [`connections::ConnectionStore`]) and so dead-code analysis sees them as API.

pub mod actions;
pub mod admin;
pub mod app;
pub mod cli;
pub mod connections;
pub mod data_compare;
pub mod demo;
pub mod dialogs;
pub mod er;
pub mod exec;
pub mod explorer;
pub mod keymap;
pub mod palette;
pub mod panels;
pub mod panic;
pub mod runtime;
pub mod schema_compare;
pub mod sqlgen;
pub mod table_editor;
pub mod templates;
pub mod workspace;
