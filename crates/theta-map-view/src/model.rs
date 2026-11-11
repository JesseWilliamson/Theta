//! The public data model the application hands to [`crate::MapPanel`].

use theta_geo::Feature;

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
    /// Index into the layer list given to `set_features`.
    pub layer: usize,
}

/// A geometry column of the result.
#[derive(Clone, Debug)]
pub struct MapLayer {
    /// Column name, e.g. `location`.
    pub name: String,
    /// Whether the layer is drawn.
    pub visible: bool,
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
