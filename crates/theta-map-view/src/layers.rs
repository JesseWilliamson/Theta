//! The runtime side of the layer model: column statistics, colour classes per feature and
//! the legends, computed from a `LayerStyle` and the typed values of the features.

use crate::agg::BinResult;
use crate::classify::{self, MAX_CLASSES, short_number};
use crate::model::{MapFeature, MapLayer};
use crate::palette::layer_color;
use crate::ramp::{OTHER_GREY, PaletteId};
use crate::style::{BinValue, ColorScheme, ColorSpec, HeatmapOptions, LayerStyle};
use crate::world::Prepared;

/// What the Layers panel needs to know about one value column.
#[derive(Clone, Debug, PartialEq)]
pub struct ColStat {
    /// Column name.
    pub name: String,
    /// Mostly numbers (so it can drive size, weight and numeric colour).
    pub numeric: bool,
    /// Non-NULL values.
    pub non_null: usize,
}

/// Column statistics of layer `layer` (one pass over its features).
pub fn column_stats(layer: &MapLayer, layer_idx: usize, prep: &Prepared, src: &[MapFeature]) -> Vec<ColStat> {
    let n = layer.columns.len();
    let (mut non_null, mut nums) = (vec![0usize; n], vec![0usize; n]);
    for f in prep.features.iter().filter(|f| f.layer == layer_idx) {
        for (i, v) in src[f.src].values.iter().enumerate().take(n) {
            match v {
                crate::model::MapValue::Null => {}
                crate::model::MapValue::Num(_) => {
                    non_null[i] += 1;
                    nums[i] += 1;
                }
                crate::model::MapValue::Text(_) => non_null[i] += 1,
            }
        }
    }
    layer
        .columns
        .iter()
        .enumerate()
        .map(|(i, name)| ColStat { name: name.clone(), numeric: non_null[i] > 0 && nums[i] * 10 >= non_null[i] * 9, non_null: non_null[i] })
        .collect()
}

/// A legend for the overlay.
#[derive(Clone, Debug, PartialEq)]
pub struct Legend {
    /// Heading, e.g. the column name.
    pub title: String,
    /// The entries.
    pub body: LegendBody,
}

/// How a legend lists its colours.
#[derive(Clone, Debug, PartialEq)]
pub enum LegendBody {
    /// Swatch and label per class or value.
    Classes(Vec<([u8; 3], String)>),
    /// A continuous ramp from `low` to `high`.
    Gradient {
        /// Colours from low to high.
        colors: Vec<[u8; 3]>,
        /// Label at the low end.
        low: String,
        /// Label at the high end.
        high: String,
    },
}

/// The result of colouring a layer.
#[derive(Clone, Debug, Default)]
pub struct Coloring {
    /// Class colours, the last being the one for "Other" and missing values. Empty means the
    /// layer's own hue (which depends on the theme and is resolved when painting).
    pub colors: Vec<[u8; 3]>,
    /// Legend for colour-by-column.
    pub legend: Option<Legend>,
}

/// Assign every feature of `layer_idx` a class in `class_of` (indexed like
/// `prep.features`) according to the style's colour spec.
pub fn color_layer(style: &LayerStyle, layer_idx: usize, columns: &[String], prep: &Prepared, src: &[MapFeature], class_of: &mut [u8]) -> Coloring {
    let own: Vec<usize> = prep.features.iter().enumerate().filter(|(_, f)| f.layer == layer_idx).map(|(i, _)| i).collect();
    let ColorSpec::ByColumn(by) = &style.color else {
        for &i in &own {
            class_of[i] = 0;
        }
        let colors = match &style.color {
            ColorSpec::Single(Some(c)) => vec![*c],
            _ => Vec::new(),
        };
        return Coloring { colors, legend: None };
    };
    let Some(col) = columns.iter().position(|c| *c == by.column) else {
        own.iter().for_each(|&i| class_of[i] = 0);
        return Coloring::default();
    };
    let value = |i: usize| src[prep.features[i].src].values.get(col);
    match by.scheme {
        ColorScheme::Numeric { method, classes } => {
            let nums: Vec<f64> = own.iter().filter_map(|&i| value(i).and_then(|v| v.num())).collect();
            let edges = classify::breaks(&nums, method, classes);
            if edges.is_empty() {
                own.iter().for_each(|&i| class_of[i] = 0);
                return Coloring { colors: vec![OTHER_GREY], legend: None };
            }
            let n = edges.len() - 1;
            let mut missing = false;
            for &i in &own {
                class_of[i] = match value(i).and_then(|v| v.num()) {
                    Some(x) => classify::class_of(&edges, x) as u8,
                    None => {
                        missing = true;
                        n as u8
                    }
                };
            }
            let mut colors = by.palette.classes(n, by.reversed);
            let mut entries: Vec<([u8; 3], String)> = colors
                .iter()
                .enumerate()
                .map(|(k, c)| (*c, format!("{} \u{2013} {}", short_number(edges[k]), short_number(edges[k + 1]))))
                .collect();
            colors.push(OTHER_GREY);
            if missing {
                entries.push((OTHER_GREY, "No value".into()));
            }
            Coloring { colors, legend: Some(Legend { title: by.column.clone(), body: LegendBody::Classes(entries) }) }
        }
        ColorScheme::Categorical { top_n } => {
            let texts: Vec<Option<String>> = own.iter().map(|&i| value(i).and_then(|v| v.text())).collect();
            let (top, other) = classify::top_values(texts.iter().flatten().map(String::as_str), top_n.min(MAX_CLASSES + 1));
            let n = top.len();
            let mut missing = false;
            for (k, &i) in own.iter().enumerate() {
                class_of[i] = match &texts[k] {
                    Some(t) => top.iter().position(|(v, _)| v == t).unwrap_or(n) as u8,
                    None => {
                        missing = true;
                        n as u8
                    }
                };
            }
            let mut colors = by.palette.classes(n, by.reversed);
            let mut entries: Vec<([u8; 3], String)> = top.iter().zip(&colors).map(|((v, _), c)| (*c, v.clone())).collect();
            colors.push(OTHER_GREY);
            if other > 0 || missing {
                entries.push((OTHER_GREY, if other > 0 { "Other".into() } else { "No value".into() }));
            }
            Coloring { colors, legend: Some(Legend { title: by.column.clone(), body: LegendBody::Classes(entries) }) }
        }
    }
}

/// Per-feature radius in px for the Points kind: constant, or scaled linearly between the
/// column's smallest and largest value.
pub fn radii(style: &LayerStyle, layer_idx: usize, columns: &[String], prep: &Prepared, src: &[MapFeature], out: &mut [f32]) {
    let base = style.points.radius_px;
    let by = style.points.radius_by.as_ref().and_then(|r| columns.iter().position(|c| *c == r.column).map(|c| (c, r)));
    let own = prep.features.iter().enumerate().filter(|(_, f)| f.layer == layer_idx).map(|(i, _)| i);
    let Some((col, r)) = by else {
        own.for_each(|i| out[i] = base);
        return;
    };
    let val = |i: usize| src[prep.features[i].src].values.get(col).and_then(|v| v.num());
    let own: Vec<usize> = own.collect();
    let (lo, hi) = own.iter().filter_map(|&i| val(i)).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| (a.min(x), b.max(x)));
    for &i in &own {
        out[i] = match val(i) {
            Some(x) if hi > lo => r.min_px + (r.max_px - r.min_px) * ((x - lo) / (hi - lo)) as f32,
            Some(_) => (r.min_px + r.max_px) / 2.0,
            None => r.min_px,
        };
    }
}

/// Legend of a hexagon or grid result.
pub fn bins_legend(r: &BinResult, column: Option<&str>) -> Legend {
    let title = match (r.what, column) {
        (BinValue::Count, _) | (_, None) => "Count".to_string(),
        (w, Some(c)) => format!("{} of {c}", w.label()),
    };
    let entries = r
        .colors
        .iter()
        .enumerate()
        .filter(|(k, _)| k + 1 < r.edges.len())
        .map(|(k, c)| (*c, format!("{} \u{2013} {}", short_number(r.edges[k]), short_number(r.edges[k + 1]))))
        .collect();
    Legend { title, body: LegendBody::Classes(entries) }
}

/// Legend of a heatmap: its ramp from sparse to dense.
pub fn heat_legend(o: &HeatmapOptions, weight: Option<&str>) -> Legend {
    let colors = (0..24).map(|i| PaletteId::sample(o.palette, i as f32 / 23.0, o.reversed)).collect();
    Legend {
        title: weight.map_or("Density".to_string(), |w| format!("Density of {w}")),
        body: LegendBody::Gradient { colors, low: "Low".into(), high: "High".into() },
    }
}

/// Convert a stored colour to the panel's default-hue colour when the layer is `Single(None)`.
pub fn default_rgb(layer: usize, dark: bool) -> [u8; 3] {
    let h = layer_color(layer, dark);
    hsl_to_rgb(h.h, h.s, h.l)
}

/// HSL (each `0..=1`) to 8-bit RGB.
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h.rem_euclid(1.0)) * 6.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let f = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [f(r), f(g), f(b)]
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::model::MapValue;
    use crate::style::{ColorBy, RadiusBy};
    use crate::world::prepare;
    use theta_geo::{Feature, Geometry};

    fn feats(vals: &[(MapValue, MapValue)]) -> (MapLayer, Vec<MapFeature>, Prepared) {
        let layer = MapLayer::new("geom", vec!["n".into(), "t".into()]);
        let f: Vec<MapFeature> = vals
            .iter()
            .enumerate()
            .map(|(i, (n, t))| MapFeature {
                feature: Feature::new(Geometry::Point(Some([i as f64, 0.0])), Some(4326)),
                row: i,
                label: None,
                properties: vec![],
                values: vec![n.clone(), t.clone()],
                layer: 0,
            })
            .collect();
        let p = prepare(&f);
        (layer, f, p)
    }

    fn num(x: f64) -> MapValue {
        MapValue::Num(x)
    }

    fn text(s: &str) -> MapValue {
        MapValue::Text(s.into())
    }

    #[test]
    fn numeric_columns_are_detected() {
        let (l, f, p) = feats(&[(num(1.0), text("a")), (num(2.0), text("b")), (MapValue::Null, text("c"))]);
        let s = column_stats(&l, 0, &p, &f);
        assert!(s[0].numeric && !s[1].numeric);
        assert_eq!(s[0].non_null, 2);
    }

    #[test]
    fn numeric_colouring_assigns_classes_and_a_legend() {
        let vals: Vec<(MapValue, MapValue)> = (0..100).map(|i| (num(f64::from(i)), text("x"))).chain([(MapValue::Null, text("x"))]).collect();
        let (l, f, p) = feats(&vals);
        let mut style = LayerStyle::default();
        style.color = ColorSpec::ByColumn(ColorBy {
            column: "n".into(),
            scheme: ColorScheme::Numeric { method: classify::Method::EqualInterval, classes: 5 },
            palette: PaletteId::Viridis,
            reversed: false,
        });
        let mut cls = vec![0u8; p.features.len()];
        let c = color_layer(&style, 0, &l.columns, &p, &f, &mut cls);
        assert_eq!(c.colors.len(), 6, "five classes plus grey");
        assert_eq!(cls[0], 0);
        assert_eq!(cls[99], 4);
        assert_eq!(cls[100], 5, "NULL is grey");
        let Some(Legend { body: LegendBody::Classes(e), title }) = c.legend else { panic!() };
        assert_eq!(title, "n");
        assert_eq!(e.len(), 6);
        assert_eq!(e[5].1, "No value");
        assert!(e[0].1.starts_with("0 "), "{}", e[0].1);
    }

    #[test]
    fn categorical_colouring_is_top_n_plus_other() {
        let vals: Vec<(MapValue, MapValue)> =
            ["a", "b", "a", "c", "a", "b", "d"].iter().map(|t| (num(1.0), text(t))).collect();
        let (l, f, p) = feats(&vals);
        let mut style = LayerStyle::default();
        style.color = ColorSpec::ByColumn(ColorBy {
            column: "t".into(),
            scheme: ColorScheme::Categorical { top_n: 2 },
            palette: PaletteId::Tableau10,
            reversed: false,
        });
        let mut cls = vec![0u8; p.features.len()];
        let c = color_layer(&style, 0, &l.columns, &p, &f, &mut cls);
        assert_eq!(cls, vec![0, 1, 0, 2, 0, 1, 2]);
        let Some(Legend { body: LegendBody::Classes(e), .. }) = c.legend else { panic!() };
        assert_eq!(e.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), vec!["a", "b", "Other"]);
    }

    #[test]
    fn single_colour_and_unknown_column_fall_back() {
        let (l, f, p) = feats(&[(num(1.0), text("a"))]);
        let mut cls = vec![9u8; 1];
        let c = color_layer(&LayerStyle::default(), 0, &l.columns, &p, &f, &mut cls);
        assert!(c.colors.is_empty() && c.legend.is_none());
        assert_eq!(cls[0], 0);
        let mut st = LayerStyle::default();
        st.color = ColorSpec::Single(Some([1, 2, 3]));
        assert_eq!(color_layer(&st, 0, &l.columns, &p, &f, &mut cls).colors, vec![[1, 2, 3]]);
    }

    #[test]
    fn radius_scales_between_min_and_max() {
        let (l, f, p) = feats(&[(num(0.0), text("a")), (num(5.0), text("a")), (num(10.0), text("a")), (MapValue::Null, text("a"))]);
        let mut st = LayerStyle::default();
        st.points.radius_by = Some(RadiusBy { column: "n".into(), min_px: 2.0, max_px: 12.0 });
        let mut r = vec![0f32; 4];
        radii(&st, 0, &l.columns, &p, &f, &mut r);
        assert_eq!(r, vec![2.0, 7.0, 12.0, 2.0]);
        radii(&LayerStyle::default(), 0, &l.columns, &p, &f, &mut r);
        assert!(r.iter().all(|x| *x == 5.0));
    }

    #[test]
    fn hsl_conversion() {
        assert_eq!(hsl_to_rgb(0.0, 1.0, 0.5), [255, 0, 0]);
        assert_eq!(hsl_to_rgb(1.0 / 3.0, 1.0, 0.5), [0, 255, 0]);
        assert_eq!(hsl_to_rgb(0.0, 0.0, 1.0), [255, 255, 255]);
    }
}
