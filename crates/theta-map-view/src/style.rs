//! The layer model: how one geometry layer is drawn. Plain data (serde-serialisable, so the
//! app can persist or export it) with no GPUI in it; the renderer and the Layers panel
//! both read and edit these structs.

use crate::classify::{MAX_CLASSES, MIN_CLASSES, Method};
use crate::ramp::PaletteId;
use serde::{Deserialize, Serialize};

/// Point layers with more features than this start as a heatmap: that many separate
/// markers are unreadable and slow, a density surface is not.
pub const HEATMAP_AUTO_THRESHOLD: usize = 50_000;

/// How a layer's geometry is turned into pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LayerKind {
    /// Points, lines and polygons as they are.
    #[default]
    Auto,
    /// A scatterplot: every feature is a circle (polygons and lines at their centre).
    Points,
    /// A Gaussian density surface of the features.
    Heatmap,
    /// Hexagonal bins of N pixels.
    Hexagons,
    /// Square bins of N pixels.
    Grid,
    /// Nearby points merged into counted circles; click to zoom in.
    Clusters,
}

impl LayerKind {
    /// Every kind, in menu order.
    pub const ALL: [LayerKind; 6] = [
        LayerKind::Auto,
        LayerKind::Points,
        LayerKind::Heatmap,
        LayerKind::Hexagons,
        LayerKind::Grid,
        LayerKind::Clusters,
    ];

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            LayerKind::Auto => "Auto",
            LayerKind::Points => "Points",
            LayerKind::Heatmap => "Heatmap",
            LayerKind::Hexagons => "Hexagons",
            LayerKind::Grid => "Grid",
            LayerKind::Clusters => "Clusters",
        }
    }

    /// Kinds that aggregate many features into one drawn thing.
    pub fn is_aggregate(self) -> bool {
        matches!(self, LayerKind::Heatmap | LayerKind::Hexagons | LayerKind::Grid | LayerKind::Clusters)
    }
}

/// How a colour-by-column is split up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorScheme {
    /// The `top_n` most frequent values each get a colour, the rest share "Other".
    Categorical {
        /// Distinct values coloured individually.
        top_n: usize,
    },
    /// Numbers cut into `classes` classes.
    Numeric {
        /// Classification method.
        method: Method,
        /// Number of classes, 5 to 9.
        classes: usize,
    },
}

/// Colour features by a column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorBy {
    /// The column.
    pub column: String,
    /// How its values map to classes.
    pub scheme: ColorScheme,
    /// The palette (a categorical one for categorical schemes).
    pub palette: PaletteId,
    /// Flip the palette.
    pub reversed: bool,
}

/// The fill colour of a layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ColorSpec {
    /// One colour; `None` takes the layer's default hue.
    Single(Option<[u8; 3]>),
    /// By column.
    ByColumn(ColorBy),
}

/// Size points by a numeric column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RadiusBy {
    /// The column.
    pub column: String,
    /// Radius of the smallest value, px.
    pub min_px: f32,
    /// Radius of the largest value, px.
    pub max_px: f32,
}

/// Options of the Points kind (and the marker size of Auto).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PointsOptions {
    /// Radius in px.
    pub radius_px: f32,
    /// Optional radius-by-column.
    pub radius_by: Option<RadiusBy>,
}

/// Options of the Heatmap kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeatmapOptions {
    /// Kernel radius in px.
    pub radius_px: f32,
    /// Multiplier on the density before it is mapped to colour.
    pub intensity: f32,
    /// Densities below this fraction of the maximum are transparent.
    pub threshold: f32,
    /// Weight column (each point counts as 1 when `None`).
    pub weight: Option<String>,
    /// Colour ramp.
    pub palette: PaletteId,
    /// Flip the ramp.
    pub reversed: bool,
}

/// What a hexagon or grid cell shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BinValue {
    /// Number of features.
    Count,
    /// Sum of a numeric column.
    Sum,
    /// Mean of a numeric column.
    Mean,
    /// Minimum of a numeric column.
    Min,
    /// Maximum of a numeric column.
    Max,
}

impl BinValue {
    /// Every value, in menu order.
    pub const ALL: [BinValue; 5] = [BinValue::Count, BinValue::Sum, BinValue::Mean, BinValue::Min, BinValue::Max];

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            BinValue::Count => "Count",
            BinValue::Sum => "Sum",
            BinValue::Mean => "Mean",
            BinValue::Min => "Min",
            BinValue::Max => "Max",
        }
    }
}

/// Options of the Hexagons and Grid kinds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BinOptions {
    /// Cell size in px (hexagon width across flats, or square side).
    pub cell_px: f32,
    /// What a cell shows.
    pub value: BinValue,
    /// The column for Sum, Mean, Min and Max.
    pub column: Option<String>,
    /// Classification of the cell values.
    pub method: Method,
    /// Number of colour classes.
    pub classes: usize,
    /// Colour ramp.
    pub palette: PaletteId,
    /// Flip the ramp.
    pub reversed: bool,
}

/// Options of the Clusters kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClusterOptions {
    /// Points closer than this many px merge.
    pub radius_px: f32,
}

/// Which simplification the tiler applies to lines and polygons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Simplify {
    /// Keep every vertex.
    Off,
    /// Douglas-Peucker: keeps the shape within the tolerance of the original.
    #[default]
    DouglasPeucker,
    /// Visvalingam-Whyatt: removes the least significant corners first; smoother.
    Visvalingam,
}

impl Simplify {
    /// Every choice, in menu order.
    pub const ALL: [Simplify; 3] = [Simplify::DouglasPeucker, Simplify::Visvalingam, Simplify::Off];

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            Simplify::Off => "Off",
            Simplify::DouglasPeucker => "Douglas-Peucker",
            Simplify::Visvalingam => "Visvalingam-Whyatt",
        }
    }
}

/// Whether a layer is drawn from a tile pyramid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TilingMode {
    /// Tiles when the layer is large (see [`LARGE_FEATURES`] and [`LARGE_VERTICES`]).
    #[default]
    Auto,
    /// Always tiles.
    On,
    /// Never tiles: every visible feature is drawn from the full-detail data.
    Off,
}

/// Auto tiling starts above this many features in the layer...
pub const LARGE_FEATURES: usize = 20_000;
/// ...or this many vertices.
pub const LARGE_VERTICES: usize = 200_000;

/// Tippecanoe-style tiling options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TilingOptions {
    /// Whether to tile.
    pub mode: TilingMode,
    /// Simplification tolerance in px of the tile (0.6 keeps lines within a pixel).
    pub detail_px: f32,
    /// Simplification algorithm.
    pub simplify: Simplify,
    /// Most points kept in one tile.
    pub point_budget: usize,
    /// Drop polygons under a pixel, keeping representative ones.
    pub drop_tiny_polygons: bool,
    /// Never drop or thin anything.
    pub keep_every_feature: bool,
    /// Deepest zoom with its own tiles; deeper views reuse it.
    pub max_zoom: u8,
    /// Debug overlay: tile edges with kept and dropped counts.
    pub show_boundaries: bool,
}

impl Default for TilingOptions {
    fn default() -> Self {
        TilingOptions {
            mode: TilingMode::Auto,
            detail_px: 0.6,
            simplify: Simplify::DouglasPeucker,
            point_budget: 20_000,
            drop_tiny_polygons: true,
            keep_every_feature: false,
            max_zoom: 14,
            show_boundaries: false,
        }
    }
}

/// Everything about how one layer looks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerStyle {
    /// Rendering kind.
    pub kind: LayerKind,
    /// Colour.
    pub color: ColorSpec,
    /// Opacity of fills and markers, `0..=1`.
    pub opacity: f32,
    /// Stroke width of outlines and lines, px.
    pub stroke_width: f32,
    /// Fill polygons.
    pub fill: bool,
    /// Point options.
    pub points: PointsOptions,
    /// Heatmap options.
    pub heatmap: HeatmapOptions,
    /// Hexagon and grid options.
    pub bins: BinOptions,
    /// Cluster options.
    pub clusters: ClusterOptions,
    /// Tiling options.
    pub tiling: TilingOptions,
}

impl Default for LayerStyle {
    fn default() -> Self {
        LayerStyle {
            kind: LayerKind::Auto,
            color: ColorSpec::Single(None),
            opacity: 1.0,
            stroke_width: 1.5,
            fill: true,
            points: PointsOptions { radius_px: 5.0, radius_by: None },
            heatmap: HeatmapOptions {
                radius_px: 20.0,
                intensity: 1.0,
                threshold: 0.03,
                weight: None,
                palette: PaletteId::Magma,
                reversed: false,
            },
            bins: BinOptions {
                cell_px: 28.0,
                value: BinValue::Count,
                column: None,
                method: Method::Quantile,
                classes: 7,
                palette: PaletteId::Viridis,
                reversed: false,
            },
            clusters: ClusterOptions { radius_px: 56.0 },
            tiling: TilingOptions::default(),
        }
    }
}

impl LayerStyle {
    /// The style a layer starts with: a heatmap for a large layer made only of points,
    /// otherwise Auto.
    pub fn automatic(features: usize, only_points: bool) -> LayerStyle {
        let kind = if only_points && features > HEATMAP_AUTO_THRESHOLD { LayerKind::Heatmap } else { LayerKind::Auto };
        LayerStyle { kind, ..LayerStyle::default() }
    }

    /// Clamp every number into its supported range and drop column references that are not
    /// in `columns`, so an edited or deserialised style can never break the renderer.
    pub fn sanitize(&mut self, columns: &[String]) {
        let known = |c: &String| columns.contains(c);
        self.opacity = self.opacity.clamp(0.05, 1.0);
        self.stroke_width = self.stroke_width.clamp(0.0, 8.0);
        self.points.radius_px = self.points.radius_px.clamp(0.5, 40.0);
        if let Some(r) = &mut self.points.radius_by {
            r.min_px = r.min_px.clamp(0.5, 40.0);
            r.max_px = r.max_px.clamp(r.min_px, 60.0);
        }
        if self.points.radius_by.as_ref().is_some_and(|r| !known(&r.column)) {
            self.points.radius_by = None;
        }
        if let ColorSpec::ByColumn(c) = &mut self.color {
            match &mut c.scheme {
                ColorScheme::Categorical { top_n } => *top_n = (*top_n).clamp(2, 10),
                ColorScheme::Numeric { classes, .. } => *classes = (*classes).clamp(MIN_CLASSES, MAX_CLASSES),
            }
        }
        if matches!(&self.color, ColorSpec::ByColumn(c) if !known(&c.column)) {
            self.color = ColorSpec::Single(None);
        }
        let h = &mut self.heatmap;
        h.radius_px = h.radius_px.clamp(2.0, 120.0);
        h.intensity = h.intensity.clamp(0.1, 10.0);
        h.threshold = h.threshold.clamp(0.0, 0.9);
        if h.weight.as_ref().is_some_and(|c| !known(c)) {
            h.weight = None;
        }
        let b = &mut self.bins;
        b.cell_px = b.cell_px.clamp(8.0, 200.0);
        b.classes = b.classes.clamp(MIN_CLASSES, MAX_CLASSES);
        if b.column.as_ref().is_some_and(|c| !known(c)) {
            b.column = None;
        }
        if b.value != BinValue::Count && b.column.is_none() {
            b.value = BinValue::Count;
        }
        self.clusters.radius_px = self.clusters.radius_px.clamp(16.0, 160.0);
        let t = &mut self.tiling;
        t.detail_px = t.detail_px.clamp(0.0, 8.0);
        t.point_budget = t.point_budget.clamp(500, 500_000);
        t.max_zoom = t.max_zoom.clamp(2, 22);
    }
}

/// Whether tiling applies to a layer of `features` features and `vertices` vertices.
pub fn tiling_enabled(t: &TilingOptions, features: usize, vertices: usize) -> bool {
    match t.mode {
        TilingMode::On => true,
        TilingMode::Off => false,
        TilingMode::Auto => features > LARGE_FEATURES || vertices > LARGE_VERTICES || t.show_boundaries,
    }
}

/// A viewport-sized world rectangle used by the timing tests.
#[cfg(test)]
pub fn tests_rect() -> theta_geo::BBox {
    theta_geo::BBox { min: [0.4, 0.3], max: [0.62, 0.42] }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn big_point_layers_default_to_heatmap_everything_else_to_auto() {
        assert_eq!(LayerStyle::automatic(50_001, true).kind, LayerKind::Heatmap);
        assert_eq!(LayerStyle::automatic(50_000, true).kind, LayerKind::Auto);
        assert_eq!(LayerStyle::automatic(1_000_000, false).kind, LayerKind::Auto);
        assert_eq!(LayerStyle::automatic(10, true).kind, LayerKind::Auto);
    }

    #[test]
    fn defaults_match_the_brief() {
        let s = LayerStyle::default();
        assert_eq!(s.tiling.detail_px, 0.6);
        assert_eq!(s.tiling.point_budget, 20_000);
        assert!(s.tiling.drop_tiny_polygons && !s.tiling.keep_every_feature);
        assert_eq!(s.tiling.simplify, Simplify::DouglasPeucker);
        assert!((5..=9).contains(&s.bins.classes));
    }

    #[test]
    fn serde_round_trip() {
        let mut s = LayerStyle::default();
        s.kind = LayerKind::Hexagons;
        s.color = ColorSpec::ByColumn(ColorBy {
            column: "pop".into(),
            scheme: ColorScheme::Numeric { method: Method::Logarithmic, classes: 6 },
            palette: PaletteId::Cividis,
            reversed: true,
        });
        s.points.radius_by = Some(RadiusBy { column: "pop".into(), min_px: 2.0, max_px: 20.0 });
        let json = serde_json::to_string(&s).unwrap();
        let back: LayerStyle = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn sanitize_clamps_and_drops_unknown_columns() {
        let mut s = LayerStyle::default();
        s.opacity = 7.0;
        s.heatmap.radius_px = 0.0;
        s.heatmap.weight = Some("gone".into());
        s.bins.value = BinValue::Sum;
        s.bins.column = Some("gone".into());
        s.bins.classes = 50;
        s.color = ColorSpec::ByColumn(ColorBy {
            column: "gone".into(),
            scheme: ColorScheme::Categorical { top_n: 99 },
            palette: PaletteId::Tableau10,
            reversed: false,
        });
        s.sanitize(&["pop".to_string()]);
        assert_eq!(s.opacity, 1.0);
        assert_eq!(s.heatmap.radius_px, 2.0);
        assert_eq!(s.heatmap.weight, None);
        assert_eq!(s.bins.value, BinValue::Count);
        assert_eq!(s.bins.classes, 9);
        assert_eq!(s.color, ColorSpec::Single(None));
    }

    #[test]
    fn tiling_switches_on_for_large_layers_only() {
        let t = TilingOptions::default();
        assert!(!tiling_enabled(&t, 100, 5_000));
        assert!(tiling_enabled(&t, LARGE_FEATURES + 1, 0));
        assert!(tiling_enabled(&t, 10, LARGE_VERTICES + 1));
        let off = TilingOptions { mode: TilingMode::Off, ..t.clone() };
        assert!(!tiling_enabled(&off, 1_000_000, 0));
        let on = TilingOptions { mode: TilingMode::On, ..t };
        assert!(tiling_enabled(&on, 1, 1));
    }
}
