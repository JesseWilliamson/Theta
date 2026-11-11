//! # theta-map-view
//!
//! The geography viewer: draws the geometry values of a query result on an interactive
//! slippy map (or, for data with an unknown CRS, on a plain planar grid).
//!
//! Layering, bottom to top:
//! * pure logic with unit tests: [`world`] (projection + precompute), [`index`] (grid
//!   index), [`lod`], [`clip`], [`tiles`] (cache keys, LRU, request planning),
//!   [`measure_tool`], [`palette`], [`props`], [`grid`], [`layout`];
//! * [`loader`] / [`decode`]: blocking tile fetch on worker threads;
//! * [`core`] / [`scene`]: state, camera animation, hit testing and painting;
//! * [`panel`]: the GPUI entity, [`MapPanel`].

pub mod basemap;
mod camera;
mod clip;
mod core;
mod decode;
mod export;
mod grid;
mod index;
mod layout;
mod loader;
mod lod;
mod measure_tool;
mod model;
mod palette;
mod panel;
mod props;
mod scene;
mod tiles;
mod world;

pub use model::{MapEvent, MapFeature, MapLayer};
pub use panel::MapPanel;
