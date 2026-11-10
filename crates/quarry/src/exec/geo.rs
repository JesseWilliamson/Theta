//! Finding geometry columns in a result and turning rows into map features.

use quarry_core::{Column, ResultSet, TypeKind, Value};
use quarry_map_view::{MapFeature, MapLayer};

pub const MAX_PROPERTIES: usize = 12;
const MAX_PROP_CHARS: usize = 120;
/// Text columns are sniffed on this many non-null leading values.
const SNIFF: usize = 5;

pub struct GeoData {
    pub layers: Vec<MapLayer>,
    pub features: Vec<MapFeature>,
    /// Result column index of each layer.
    pub columns: Vec<usize>,
}

fn decode(v: &Value) -> Option<quarry_geo::Feature> {
    match v {
        Value::Geometry(b) => quarry_geo::parse_wkb(b).ok(),
        Value::Text(s) | Value::Unknown(s) | Value::Json(s) => quarry_geo::parse_any_text(s),
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
            let text_like = matches!(result.columns[i].kind, TypeKind::Text | TypeKind::Json | TypeKind::Other);
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
        .find(|&i| columns[i].kind == TypeKind::Text && ["name", "title", "label"].contains(&columns[i].name.to_lowercase().as_str()))
        .or_else(|| (0..columns.len()).filter(ok).find(|&i| columns[i].kind == TypeKind::Text))
        .or_else(|| (0..columns.len()).filter(ok).next())
}

fn cap(s: String) -> String {
    if s.chars().count() > MAX_PROP_CHARS { format!("{}…", s.chars().take(MAX_PROP_CHARS).collect::<String>()) } else { s }
}

/// Build layers and features; `None` when the result has no geometry.
pub fn extract(result: &ResultSet) -> Option<GeoData> {
    let geo = geometry_columns(result);
    if geo.is_empty() {
        return None;
    }
    let label = label_column(&result.columns, &geo);
    let layers: Vec<MapLayer> =
        geo.iter().map(|&i| MapLayer { name: result.columns[i].name.clone(), visible: true }).collect();
    let mut features = Vec::new();
    for (row_idx, row) in result.rows.iter().enumerate() {
        for (layer, &col) in geo.iter().enumerate() {
            let Some(feature) = row.get(col).and_then(decode) else { continue };
            let label = label.and_then(|i| row.get(i)).filter(|v| !v.is_null()).map(|v| cap(v.display()));
            let properties = (0..result.columns.len())
                .filter(|i| !geo.contains(i) && Some(*i) != label_column(&result.columns, &geo))
                .filter_map(|i| {
                    let v = row.get(i)?;
                    (!v.is_null()).then(|| (result.columns[i].name.clone(), cap(v.display())))
                })
                .take(MAX_PROPERTIES)
                .collect();
            features.push(MapFeature { feature, row: row_idx, label, properties, layer });
        }
    }
    (!features.is_empty()).then_some(GeoData { layers, features, columns: geo })
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
            columns: vec![col("id", TypeKind::Integer), col("name", TypeKind::Text), col("geom", TypeKind::Other)],
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
        let r = ResultSet { columns: vec![col("a", TypeKind::Integer)], rows: vec![vec![Value::Int(1)]], truncated: false };
        assert!(extract(&r).is_none());
    }

    #[test]
    fn long_properties_are_capped() {
        let long = "x".repeat(500);
        assert!(cap(long).chars().count() <= MAX_PROP_CHARS + 1);
    }
}
