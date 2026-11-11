//! GeoJSON (RFC 7946) reading and writing on top of `serde_json::Value`.

use crate::error::GeoError;
use crate::geometry::{Coord, Feature, Geometry};
use serde_json::{Map, Value, json};

const MAX_DEPTH: usize = 32;

fn bad<T>(msg: impl Into<String>) -> Result<T, GeoError> {
    Err(GeoError::InvalidGeoJson(msg.into()))
}

fn position(v: &Value) -> Result<Coord, GeoError> {
    let a = v.as_array().map_or(&[][..], Vec::as_slice);
    match (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_f64)) {
        (Some(x), Some(y)) => Ok([x, y]),
        _ => bad("a position must be an array of at least two numbers"),
    }
}

fn array(v: &Value) -> Result<&[Value], GeoError> {
    match v.as_array() {
        Some(a) => Ok(a),
        None => bad("expected an array of coordinates"),
    }
}

fn line(v: &Value) -> Result<Vec<Coord>, GeoError> {
    array(v)?.iter().map(position).collect()
}

fn rings(v: &Value) -> Result<Vec<Vec<Coord>>, GeoError> {
    array(v)?.iter().map(line).collect()
}

fn geometry(v: &Value, depth: usize) -> Result<Geometry, GeoError> {
    if depth > MAX_DEPTH {
        return Err(GeoError::TooDeep);
    }
    let Some(kind) = v.get("type").and_then(Value::as_str) else {
        return bad("geometry has no \"type\"");
    };
    let coords = || v.get("coordinates").map_or_else(|| bad("geometry has no \"coordinates\""), Ok);
    Ok(match kind {
        "Point" => {
            let c = coords()?;
            if array(c)?.is_empty() { Geometry::Point(None) } else { Geometry::Point(Some(position(c)?)) }
        }
        "LineString" => Geometry::LineString(line(coords()?)?),
        "Polygon" => Geometry::Polygon(rings(coords()?)?),
        "MultiPoint" => Geometry::MultiPoint(line(coords()?)?),
        "MultiLineString" => Geometry::MultiLineString(rings(coords()?)?),
        "MultiPolygon" => {
            Geometry::MultiPolygon(array(coords()?)?.iter().map(rings).collect::<Result<Vec<_>, _>>()?)
        }
        "GeometryCollection" => {
            let Some(items) = v.get("geometries").and_then(Value::as_array) else {
                return bad("GeometryCollection has no \"geometries\" array");
            };
            Geometry::GeometryCollection(
                items.iter().map(|g| geometry(g, depth + 1)).collect::<Result<Vec<_>, _>>()?,
            )
        }
        other => return bad(format!("unsupported geometry type \"{other}\"")),
    })
}

/// Legacy (RFC 7946 removed it, but PostGIS still emits it) named CRS such as
/// `urn:ogc:def:crs:EPSG::3857` or `EPSG:3857`.
fn crs_srid(v: &Value) -> Option<i32> {
    let name = v.get("crs")?.get("properties")?.get("name")?.as_str()?;
    if !name.to_ascii_uppercase().contains("EPSG") {
        return None;
    }
    name.rsplit(':').next()?.parse().ok()
}

/// Parse a GeoJSON geometry, Feature or FeatureCollection into features.
///
/// GeoJSON is WGS 84 by definition, so features get `srid = Some(4326)` unless a legacy
/// `crs` member names another EPSG code. Features whose `geometry` is `null` are skipped.
pub fn parse_geojson(text: &str) -> Result<Vec<Feature>, GeoError> {
    let root: Value = serde_json::from_str(text).map_err(|e| GeoError::InvalidGeoJson(e.to_string()))?;
    let srid = Some(crs_srid(&root).unwrap_or(4326));
    let one = |g: &Value| geometry(g, 0).map(|geometry| Feature { geometry, srid });
    match root.get("type").and_then(Value::as_str) {
        Some("FeatureCollection") => {
            let Some(items) = root.get("features").and_then(Value::as_array) else {
                return bad("FeatureCollection has no \"features\" array");
            };
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item.get("geometry") {
                    None | Some(Value::Null) => {}
                    Some(g) => out.push(one(g)?),
                }
            }
            Ok(out)
        }
        Some("Feature") => match root.get("geometry") {
            None | Some(Value::Null) => Ok(vec![]),
            Some(g) => Ok(vec![one(g)?]),
        },
        Some(_) => Ok(vec![one(&root)?]),
        None => bad("missing \"type\""),
    }
}

/// Whole numbers are written as JSON integers (`1`, not `1.0`) like PostGIS does;
/// non-finite values have no JSON representation and become `null`.
fn num(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return Value::from(v as i64);
    }
    serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number)
}

fn pos(c: Coord) -> Value {
    json!([num(c[0]), num(c[1])])
}

fn line_v(l: &[Coord]) -> Value {
    Value::Array(l.iter().map(|c| pos(*c)).collect())
}

fn rings_v(r: &[Vec<Coord>]) -> Value {
    Value::Array(r.iter().map(|l| line_v(l)).collect())
}

/// The GeoJSON geometry object for a feature. The SRID is not written: RFC 7946
/// mandates WGS 84, so callers should reproject to lon/lat first.
/// `POINT EMPTY` becomes `{"type":"Point","coordinates":[]}`, as PostGIS does.
pub fn to_geojson(feature: &Feature) -> Value {
    geometry_v(&feature.geometry)
}

fn geometry_v(g: &Geometry) -> Value {
    match g {
        Geometry::Point(p) => {
            json!({"type": "Point", "coordinates": p.map_or(json!([]), pos)})
        }
        Geometry::LineString(l) => json!({"type": "LineString", "coordinates": line_v(l)}),
        Geometry::Polygon(r) => json!({"type": "Polygon", "coordinates": rings_v(r)}),
        Geometry::MultiPoint(l) => json!({"type": "MultiPoint", "coordinates": line_v(l)}),
        Geometry::MultiLineString(r) => json!({"type": "MultiLineString", "coordinates": rings_v(r)}),
        Geometry::MultiPolygon(p) => json!({
            "type": "MultiPolygon",
            "coordinates": Value::Array(p.iter().map(|r| rings_v(r)).collect())
        }),
        Geometry::GeometryCollection(gs) => json!({
            "type": "GeometryCollection",
            "geometries": Value::Array(gs.iter().map(geometry_v).collect())
        }),
    }
}

/// A `FeatureCollection` with one `Feature` per entry, carrying its properties
/// (used by "Export as GeoJSON": the other columns of the result row).
pub fn to_geojson_feature_collection(features: &[(Feature, Map<String, Value>)]) -> Value {
    let items: Vec<Value> = features
        .iter()
        .map(|(f, props)| json!({"type": "Feature", "geometry": to_geojson(f), "properties": props}))
        .collect();
    json!({"type": "FeatureCollection", "features": items})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_geometry() {
        let f = parse_geojson(r#"{"type":"Point","coordinates":[1,2]}"#).unwrap();
        assert_eq!(f, vec![Feature::new(Geometry::Point(Some([1.0, 2.0])), Some(4326))]);
    }

    #[test]
    fn point_with_z_and_empty() {
        let f = parse_geojson(r#"{"type":"Point","coordinates":[1,2,3]}"#).unwrap();
        assert_eq!(f[0].geometry, Geometry::Point(Some([1.0, 2.0])));
        let f = parse_geojson(r#"{"type":"Point","coordinates":[]}"#).unwrap();
        assert_eq!(f[0].geometry, Geometry::Point(None));
    }

    #[test]
    fn feature_and_collection() {
        let f = parse_geojson(r#"{"type":"Feature","properties":{"a":1},"geometry":{"type":"LineString","coordinates":[[0,0],[1,1]]}}"#).unwrap();
        assert_eq!(f.len(), 1);
        let fc = r#"{"type":"FeatureCollection","features":[
            {"type":"Feature","geometry":{"type":"Point","coordinates":[1,2]}},
            {"type":"Feature","geometry":null},
            {"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}}]}"#;
        let f = parse_geojson(fc).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[1].geometry.type_name(), "Polygon");
        assert_eq!(parse_geojson(r#"{"type":"Feature","geometry":null}"#).unwrap(), vec![]);
    }

    #[test]
    fn geometry_collection_stays_one_feature() {
        let s = r#"{"type":"GeometryCollection","geometries":[{"type":"Point","coordinates":[1,2]},{"type":"MultiPoint","coordinates":[[1,1],[2,2]]}]}"#;
        let f = parse_geojson(s).unwrap();
        assert_eq!(f.len(), 1);
        assert!(matches!(&f[0].geometry, Geometry::GeometryCollection(g) if g.len() == 2));
    }

    #[test]
    fn legacy_crs() {
        let s = r#"{"type":"Point","coordinates":[1,2],"crs":{"type":"name","properties":{"name":"urn:ogc:def:crs:EPSG::3857"}}}"#;
        assert_eq!(parse_geojson(s).unwrap()[0].srid, Some(3857));
        let s = r#"{"type":"Point","coordinates":[1,2],"crs":{"type":"name","properties":{"name":"EPSG:27700"}}}"#;
        assert_eq!(parse_geojson(s).unwrap()[0].srid, Some(27700));
    }

    #[test]
    fn errors() {
        assert!(parse_geojson("nope").is_err());
        assert!(parse_geojson("{}").is_err());
        assert!(parse_geojson(r#"{"type":"Point"}"#).is_err());
        assert!(parse_geojson(r#"{"type":"Point","coordinates":["a","b"]}"#).is_err());
        assert!(parse_geojson(r#"{"type":"Circle","coordinates":[1,2]}"#).is_err());
        assert!(parse_geojson(r#"{"type":"LineString","coordinates":5}"#).is_err());
        assert!(parse_geojson(r#"{"type":"FeatureCollection"}"#).is_err());
        assert!(parse_geojson(r#"{"type":"GeometryCollection"}"#).is_err());
    }

    #[test]
    fn deep_collection_is_capped() {
        let mut s = String::from(r#"{"type":"Point","coordinates":[1,2]}"#);
        for _ in 0..60 {
            s = format!(r#"{{"type":"GeometryCollection","geometries":[{s}]}}"#);
        }
        assert_eq!(parse_geojson(&s), Err(GeoError::TooDeep));
    }

    #[test]
    fn write_all_types_roundtrip() {
        let sq = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 0.0]];
        let geoms = [
            Geometry::Point(Some([1.5, 2.5])),
            Geometry::Point(None),
            Geometry::LineString(vec![[0.0, 0.0], [1.0, 1.0]]),
            Geometry::Polygon(vec![sq.clone()]),
            Geometry::MultiPoint(vec![[1.0, 1.0], [2.0, 2.0]]),
            Geometry::MultiLineString(vec![vec![[0.0, 0.0], [1.0, 1.0]]]),
            Geometry::MultiPolygon(vec![vec![sq.clone()], vec![sq]]),
            Geometry::GeometryCollection(vec![Geometry::Point(Some([1.0, 2.0]))]),
        ];
        for g in geoms {
            let f = Feature::new(g, Some(4326));
            let text = to_geojson(&f).to_string();
            assert_eq!(parse_geojson(&text).unwrap(), vec![f], "{text}");
        }
    }

    #[test]
    fn nan_becomes_null_not_panic() {
        let f = Feature::new(Geometry::Point(Some([f64::NAN, 1.0])), None);
        assert_eq!(to_geojson(&f)["coordinates"][0], Value::Null);
    }

    #[test]
    fn feature_collection_export() {
        let mut props = Map::new();
        props.insert("name".into(), json!("Sydney"));
        let f = Feature::new(Geometry::Point(Some([151.2, -33.9])), Some(4326));
        let v = to_geojson_feature_collection(&[(f, props)]);
        assert_eq!(v["type"], "FeatureCollection");
        assert_eq!(v["features"][0]["properties"]["name"], "Sydney");
        assert_eq!(v["features"][0]["geometry"]["coordinates"][1], -33.9);
        assert_eq!(to_geojson_feature_collection(&[])["features"], json!([]));
    }
}
