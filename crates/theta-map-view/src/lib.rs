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

mod agg;
pub mod basemap;
mod camera;
mod classify;
mod clip;
mod core;
mod decode;
mod export;
mod grid;
mod index;
mod layers;
mod layers_ui;
mod layerstate;
mod layout;
mod loader;
mod lod;
mod measure_tool;
mod model;
mod palette;
mod panel;
#[cfg(test)]
mod perf_tests;
mod props;
mod ramp;
mod scene;
mod scene_layers;
mod style;
mod tiler;
mod tiles;
mod world;

pub use classify::Method as ClassMethod;
pub use model::{MapEvent, MapFeature, MapLayer, MapValue};
pub use ramp::PaletteId;
pub use style::{BinValue, ColorBy, ColorScheme, ColorSpec, LayerKind, LayerStyle, Simplify, TilingMode};
pub use panel::{LayerPerf, MapPanel};
