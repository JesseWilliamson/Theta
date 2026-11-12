//! # theta-grid
//!
//! The query-results data grid.
//!
//! The pure logic (comparison, filtering, selection geometry, parsing, staged
//! edits, layout arithmetic) lives in GPUI-free modules with unit tests; the
//! `ResultGrid` view only turns that state into pixels and input into state
//! changes.

mod actions;
mod agg_ui;
pub mod aggregate;
pub mod compare;
pub mod copy;
mod editing;
pub mod filter;
pub mod fold;
pub mod format;
mod grid;
pub mod imagekind;
mod input;
pub mod inspect;
mod inspector;
pub mod layout;
pub mod parse;
pub mod pending;
mod record;
mod render;
mod scroll;
pub mod selection;
pub mod summary;
mod types;
pub mod view;

pub use grid::ResultGrid;
pub use input::owns_chord;
pub use types::*;
