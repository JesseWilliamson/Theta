//! # theta-diagram
//!
//! Everything an entity-relationship diagram needs that is not painting: the graph built from a catalog
//! ([`model`]), a deterministic layered layout with orthogonal edge routing ([`layout`]), a standalone SVG
//! writer ([`svg`]) and Mermaid `erDiagram` text ([`mermaid`]). Pure Rust, no UI; the application paints the
//! same [`Layout`] on screen (`crates/theta/src/er/`), so the picture, the SVG and the tests agree.

pub mod layout;
pub mod mermaid;
pub mod model;
pub mod svg;

#[cfg(test)]
mod tests;

pub use layout::{BoxLayout, Bracket, EdgeLayout, Layout, Metrics, RowLayout, clip, layout};
pub use model::{Graph, GraphColumn, GraphEdge, GraphNode, Scope, from_catalog};
