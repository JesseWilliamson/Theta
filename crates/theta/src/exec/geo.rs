//! Finding geometry columns in a result and turning rows into map features.

use theta_core::{Column, ResultSet, TypeKind, Value};
use std::sync::Arc;
use theta_map_view::{MapFeature, MapLayer, MapValue};

pub const MAX_PROPERTIES: usize = 12;
const MAX_PROP_CHARS: usize = 120;
/// Text columns are sniffed on this many non-null leading values.
const SNIFF: usize = 5;

pub struct GeoData {
    pub layers: Vec<MapLayer>,
    pub features: Arc<Vec<MapFeature>>,
    /// Result column index of each layer.
    pub columns: Vec<usize>,
}

fn decode(v: &Value) -> Option<theta_geo::Feature> {
    match v {
        Value::Geometry(b) => theta_geo::parse_wkb(b).ok(),
        Value::Text(s) | Value::Unknown(s) | Value::Json(s) => theta_geo::parse_any_text(s),
        _ => None,
    }
}

/// Result column indexes that hold geometry.
pub fn geometry_columns(result: &ResultSet) -> Vec<usize> {
    (0..result.columns.len())
        .filter(|&i| {
            if result.rows.iter().any(|r| matches!(r.get(i), Some(Value::Geometry(_)))) {
                return true;
            }
            let text_like =
                matches!(result.columns[i].kind, TypeKind::Text | TypeKind::Json | TypeKind::Other);
            if !text_like {
                return false;
            }
            let sample: Vec<&Value> =
                result.rows.iter().filter_map(|r| r.get(i)).filter(|v| !v.is_null()).take(SNIFF).collect();
            !sample.is_empty() && sample.iter().all(|v| decode(v).is_some())
        })
        .collect()
}

fn label_column(columns: &[Column], geo: &[usize]) -> Option<usize> {
    let ok = |i: &usize| !geo.contains(i);
    (0..columns.len())
        .filter(ok)
        .find(|&i| {
            columns[i].kind == TypeKind::Text
                && ["name", "title", "label"].contains(&columns[i].name.to_lowercase().as_str())
        })
        .or_else(|| (0..columns.len()).filter(ok).find(|&i| columns[i].kind == TypeKind::Text))
        .or_else(|| (0..columns.len()).find(ok))
}

/// A cell as a typed map value: numbers stay numbers (so layers can colour and size by
/// them), NULL stays NULL, everything else is its display text.
fn typed(v: &Value) -> MapValue {
    match v {
        Value::Null => MapValue::Null,
        Value::Int(i) => MapValue::Num(*i as f64),
        Value::UInt(u) => MapValue::Num(*u as f64),
        Value::Float(f) => MapValue::Num(*f),
        Value::Decimal(d) => d.parse::<f64>().map_or_else(|_| MapValue::Text(cap(d.clone()).into()), MapValue::Num),
        other => MapValue::Text(cap(other.display()).into()),
    }
}

fn cap(s: String) -> String {
    if s.chars().count() > MAX_PROP_CHARS {
        format!("{}…", s.chars().take(MAX_PROP_CHARS).collect::<String>())
    } else {
        s
    }
}

/// Build layers and features; `None` when the result has no geometry.
pub fn extract(result: &ResultSet) -> Option<GeoData> {
    let geo = geometry_columns(result);
    if geo.is_empty() {
        return None;
    }
    let label = label_column(&result.columns, &geo);
    // Typed values cover every non-geometry column (the label column too); the hover card keeps
    // showing the first few as display strings.
    let value_cols: Vec<usize> = (0..result.columns.len()).filter(|i| !geo.contains(i)).collect();
    let names: Vec<String> = value_cols.iter().map(|&i| result.columns[i].name.clone()).collect();
    let layers: Vec<MapLayer> =
        geo.iter().map(|&i| MapLayer::new(result.columns[i].name.clone(), names.clone())).collect();
    let prop_cols: Vec<usize> = value_cols.iter().copied().filter(|i| Some(*i) != label).collect();
    let mut features = Vec::new();
    for (row_idx, row) in result.rows.iter().enumerate() {
        let mut values: Option<Vec<MapValue>> = None;
        for (layer, &col) in geo.iter().enumerate() {
            let Some(feature) = row.get(col).and_then(decode) else { continue };
            let label = label.and_then(|i| row.get(i)).filter(|v| !v.is_null()).map(|v| cap(v.display()));
            let properties = prop_cols
                .iter()
                .filter_map(|&i| {
                    let v = row.get(i)?;
                    (!v.is_null()).then(|| (result.columns[i].name.clone(), cap(v.display())))
                })
                .take(MAX_PROPERTIES)
                .collect();
            // One row can feed several layers; its typed values are built once.
            let values = values
                .get_or_insert_with(|| value_cols.iter().map(|&i| row.get(i).map_or(MapValue::Null, typed)).collect())
                .clone();
            features.push(MapFeature { feature, row: row_idx, label, properties, values, layer });
        }
    }
    (!features.is_empty()).then_some(GeoData { layers, features: Arc::new(features), columns: geo })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, kind: TypeKind) -> Column {
        Column { name: name.into(), type_name: "t".into(), kind, origin: None }
    }

    fn point_wkb(x: f64, y: f64) -> Vec<u8> {
        let mut b = vec![1u8, 1, 0, 0, 0];
        b.extend(x.to_le_bytes());
        b.extend(y.to_le_bytes());
        b
    }

    #[test]
    fn wkb_column_becomes_layer() {
        let r = ResultSet {
            columns: vec![
                col("id", TypeKind::Integer),
                col("name", TypeKind::Text),
                col("geom", TypeKind::Other),
            ],
            rows: vec![
                vec![Value::Int(1), Value::Text("A".into()), Value::Geometry(point_wkb(4.9, 52.37))],
                vec![Value::Int(2), Value::Text("B".into()), Value::Null],
            ],
            truncated: false,
        };
        let g = extract(&r).unwrap();
        assert_eq!(g.layers.len(), 1);
        assert_eq!(g.features.len(), 1);
        assert_eq!(g.features[0].label.as_deref(), Some("A"));
        assert_eq!(g.features[0].properties, vec![("id".to_string(), "1".to_string())]);
    }

    #[test]
    fn values_are_typed_and_aligned_with_the_layer_columns() {
        let r = ResultSet {
            columns: vec![
                col("id", TypeKind::Integer),
                col("name", TypeKind::Text),
                col("w", TypeKind::Decimal),
                col("geom", TypeKind::Other),
            ],
            rows: vec![vec![
                Value::Int(7),
                Value::Text("A".into()),
                Value::Decimal("2.5".into()),
                Value::Geometry(point_wkb(4.9, 52.37)),
            ]],
            truncated: false,
        };
        let g = extract(&r).unwrap();
        assert_eq!(g.layers[0].columns, vec!["id", "name", "w"]);
        assert_eq!(g.features[0].values, vec![MapValue::Num(7.0), MapValue::Text("A".into()), MapValue::Num(2.5)]);
    }

    #[test]
    fn wkt_text_is_sniffed_but_plain_text_is_not() {
        let r = ResultSet {
            columns: vec![col("w", TypeKind::Text), col("t", TypeKind::Text)],
            rows: vec![vec![Value::Text("POINT(1 2)".into()), Value::Text("hello".into())]],
            truncated: false,
        };
        assert_eq!(geometry_columns(&r), vec![0]);
    }

    #[test]
    fn no_geometry_gives_none() {
        let r = ResultSet {
            columns: vec![col("a", TypeKind::Integer)],
            rows: vec![vec![Value::Int(1)]],
            truncated: false,
        };
        assert!(extract(&r).is_none());
    }

    #[test]
    fn long_properties_are_capped() {
        let long = "x".repeat(500);
        assert!(cap(long).chars().count() <= MAX_PROP_CHARS + 1);
    }
}
