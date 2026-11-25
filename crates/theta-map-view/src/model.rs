//! The public data model the application hands to [`crate::MapPanel`].

use crate::style::LayerStyle;
use serde::{Deserialize, Serialize};
use theta_geo::Feature;

/// One cell of a row, typed so layers can colour and size by a column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MapValue {
    /// SQL NULL (or a value that is not shown).
    Null,
    /// Any number (integers, decimals, floats), as `f64`.
    Num(f64),
    /// Text, dates and everything else, in its display form.
    Text(Box<str>),
}

impl MapValue {
    /// The number, if this is one and it is finite.
    pub fn num(&self) -> Option<f64> {
        match self {
            MapValue::Num(x) if x.is_finite() => Some(*x),
            _ => None,
        }
    }

    /// The display text (numbers formatted, NULL is `None`).
    pub fn text(&self) -> Option<String> {
        match self {
            MapValue::Null => None,
            MapValue::Num(x) => Some(if x.fract() == 0.0 && x.abs() < 1e15 { format!("{x:.0}") } else { x.to_string() }),
            MapValue::Text(t) => Some(t.to_string()),
        }
    }
}

/// One drawable row. The app builds these from a result set's geometry column.
#[derive(Clone, Debug)]
pub struct MapFeature {
    /// The decoded geometry and its SRID.
    pub feature: Feature,
    /// Row index in the result, reported back in events.
    pub row: usize,
    /// Shown as the hover-card title, e.g. the row's name/id column.
    pub label: Option<String>,
    /// The row's other columns, for the hover card and GeoJSON export.
    pub properties: Vec<(String, String)>,
    /// The row's non-geometry columns, typed, in the order of the layer's `columns`.
    pub values: Vec<MapValue>,
    /// Index into the layer list given to `set_features`.
    pub layer: usize,
}

/// A geometry column of the result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MapLayer {
    /// Column name, e.g. `location`.
    pub name: String,
    /// Whether the layer is drawn.
    pub visible: bool,
    /// Names of the value columns that features' `values` follow.
    #[serde(default)]
    pub columns: Vec<String>,
    /// How the layer is drawn; `None` lets the panel choose (see `LayerStyle::automatic`).
    #[serde(default)]
    pub style: Option<LayerStyle>,
}

impl MapLayer {
    /// A visible layer with automatic styling.
    pub fn new(name: impl Into<String>, columns: Vec<String>) -> MapLayer {
        MapLayer { name: name.into(), visible: true, columns, style: None }
    }
}

/// What the panel tells its owner.
#[derive(Clone, Debug)]
pub enum MapEvent {
    /// The user clicked a feature; `row` is the result row it came from.
    FeatureClicked { row: usize },
    /// The user chose Export, GeoJSON: the pretty-printed FeatureCollection of the
    /// visible layers.
    ExportGeoJson(String),
    /// Text that was already placed on the clipboard (informational).
    CopyText(String),
}
