//! Theta: a keyboard-first database IDE.
//!
//! The binary (`main.rs`) is a thin wrapper; everything lives in this library so
//! the integration crates can use the shell's seams ([`workspace::Workspace`],
//! [`connections::ConnectionStore`]) and so dead-code analysis sees them as API.

pub mod actions;
pub mod app;
pub mod cli;
pub mod connections;
pub mod demo;
pub mod dialogs;
pub mod exec;
pub mod explorer;
pub mod keymap;
pub mod palette;
pub mod panels;
pub mod panic;
pub mod runtime;
pub mod sqlgen;
pub mod templates;
pub mod workspace;
