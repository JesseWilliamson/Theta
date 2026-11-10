//! # quarry-grid
//!
//! The query-results data grid.
//!
//! The pure logic (comparison, filtering, selection geometry, parsing, staged
//! edits, layout arithmetic) lives in GPUI-free modules with unit tests; the
//! `ResultGrid` view only turns that state into pixels and input into state
//! changes.

mod actions;
pub mod compare;
pub mod copy;
pub mod filter;
pub mod format;
pub mod layout;
pub mod parse;
pub mod pending;
pub mod inspect;
pub mod selection;
pub mod summary;
pub mod view;
mod editing;
mod grid;
mod input;
mod inspector;
mod render;
mod scroll;
mod types;

pub use grid::ResultGrid;
pub use types::*;
