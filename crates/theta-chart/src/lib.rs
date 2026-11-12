//! # theta-chart
//!
//! Everything a chart needs to know that is not painting: which columns to plot by default, how rows become
//! series (grouping, aggregation, bucketing), where the axis ticks go, how data maps to pixels, and which point
//! is under the pointer. Pure functions over [`theta_core::ResultSet`], so every rule has a unit test and none
//! needs a window. The application paints what this crate computes (`crates/theta/src/panels/chart.rs`).

pub mod hit;
pub mod layout;
pub mod scale;
pub mod shape;
pub mod spec;
pub mod ticks;
pub mod time;

pub use layout::{Layout, Readout, Target};
pub use scale::{Band, Linear};
pub use shape::{ChartData, Point, Series, XKind, build};
pub use spec::{Aggregate, ChartKind, ChartSpec, suggest};
