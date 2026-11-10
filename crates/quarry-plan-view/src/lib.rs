//! # quarry-plan-view
//!
//! The query-performance panel: a tree table, a flame graph and the raw server
//! output of an `EXPLAIN`, with the analysis insights always visible beside them.
//!
//! Everything that can be decided without a window (number formatting, tree
//! flattening, flame layout, property grouping) is in plain modules with unit
//! tests; `PlanPanel` only draws it.

mod details;
pub mod flame;
mod flame_view;
pub mod format;
mod insights;
pub mod model;
mod panel;
pub mod sqlcolor;
mod tree_view;
mod ui;

pub use panel::{PlanEvent, PlanInput, PlanPanel, ViewMode};
