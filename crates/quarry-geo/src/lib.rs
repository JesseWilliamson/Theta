//! Pure-Rust geography toolkit for Quarry's map viewer, geometry inspector and exporters.
//!
//! No I/O, no async, no GUI. Modules:
//! * [`geometry`]: the `Geometry`/`Feature`/`BBox` model.
//! * [`wkb`], [`wkt`], [`geojson`], [`sniff`]: tolerant decoders and encoders.
//! * [`crs`]: SRID classification and projections (Web-Mercator, UTM).
//! * [`map`]: slippy-map camera and tile maths, plus a planar camera.
//! * [`ops`]: simplification and hit testing for rendering.
//! * [`measure`]: distances, areas, formatting and cell summaries.
//!
//! Decoders never panic on malformed input; they return [`GeoError`].

pub mod crs;
pub mod error;
pub mod geojson;
pub mod geometry;
pub mod map;
pub mod measure;
pub mod ops;
pub mod sniff;
pub mod wkb;
pub mod wkt;

pub use crs::{
    Crs, crs_for_srid, guess_crs, lon_lat_to_utm, lon_lat_to_web_mercator, to_lon_lat, utm_to_lon_lat,
    web_mercator_to_lon_lat,
};
pub use error::GeoError;
pub use geojson::{parse_geojson, to_geojson, to_geojson_feature_collection};
pub use geometry::{BBox, Coord, Feature, Geometry};
pub use map::{MapView, PlanarView, TileId, min_lon_arc, min_zoom_for_size, min_zoom_for_width, nice_scale_bar, wrap_lon};
pub use measure::{area_m2, describe, format_area, format_distance, haversine_m, length_m};
pub use ops::{hit_test, simplify};
pub use sniff::{parse_any_text, parse_any_text_result};
pub use wkb::{parse_hex_wkb, parse_wkb, to_ewkb};
pub use wkt::{parse_wkt, to_ewkt, to_wkt};
