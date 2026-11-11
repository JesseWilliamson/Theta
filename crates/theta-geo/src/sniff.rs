//! Cheap detection of geometry in arbitrary text cells.

use crate::error::GeoError;
use crate::geojson::parse_geojson;
use crate::geometry::{Feature, Geometry};
use crate::wkb::parse_hex_wkb;
use crate::wkt::parse_wkt;

const KEYWORDS: [&str; 7] =
    ["POINT", "LINESTRING", "POLYGON", "MULTIPOINT", "MULTILINESTRING", "MULTIPOLYGON", "GEOMETRYCOLLECTION"];

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.as_bytes().get(..prefix.len()).is_some_and(|b| b.eq_ignore_ascii_case(prefix.as_bytes()))
}

/// Decide whether a text cell holds a geometry and decode it.
///
/// Recognises EWKT/WKT, hex (E)WKB and GeoJSON. Result grids call this on every text
/// cell of a column, so each format is gated on a prefix check (`SRID=` or a WKT keyword,
/// a `00`/`01` byte-order pair, a `{`) and ordinary strings are rejected without parsing.
/// A GeoJSON FeatureCollection with several features becomes one `GeometryCollection`.
pub fn parse_any_text(text: &str) -> Option<Feature> {
    let t = text.trim();
    let first = *t.as_bytes().first()?;
    match first {
        b'{' => {
            if !t.contains("\"type\"") {
                return None;
            }
            let mut features = parse_geojson(t).ok()?;
            match features.len() {
                0 => None,
                1 => features.pop(),
                _ => {
                    let srid = features[0].srid;
                    let parts = features.into_iter().map(|f| f.geometry).collect();
                    Some(Feature { geometry: Geometry::GeometryCollection(parts), srid })
                }
            }
        }
        b'0' | b'\\' => {
            let digits = t.strip_prefix("\\x").or_else(|| t.strip_prefix("0x")).unwrap_or(t);
            let plausible = digits.len() >= 18 && (digits.starts_with("00") || digits.starts_with("01"));
            if plausible { parse_hex_wkb(t).ok() } else { None }
        }
        _ => {
            if starts_with_ci(t, "SRID=") || KEYWORDS.iter().any(|k| starts_with_ci(t, k)) {
                parse_wkt(t).ok()
            } else {
                None
            }
        }
    }
}

/// Like [`parse_any_text`] but explains the failure; used by the inspector.
pub fn parse_any_text_result(text: &str) -> Result<Feature, GeoError> {
    parse_any_text(text)
        .ok_or_else(|| GeoError::InvalidWkt { offset: 0, message: "not a recognised geometry".into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_each_format() {
        assert!(parse_any_text("POINT(1 2)").is_some());
        assert!(parse_any_text("  srid=4326;point(1 2)").is_some());
        assert!(parse_any_text("0101000020E6100000000000000000F03F0000000000000040").is_some());
        assert!(parse_any_text("\\x0101000000000000000000F03F0000000000000040").is_some());
        assert!(parse_any_text(r#"{"type":"Point","coordinates":[1,2]}"#).is_some());
        assert!(
            parse_any_text(r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[1,2]}}"#).is_some()
        );
    }

    #[test]
    fn rejects_ordinary_text() {
        for s in [
            "",
            "   ",
            "hello",
            "Point of sale",
            "polygonal",
            "0123456789",
            "0",
            "00",
            "01abc",
            "{}",
            "{\"a\":1}",
            "{\"type\":\"Nope\"}",
            "123",
            "SRID=abc",
            "POINT",
            "é",
            "0101zz",
            "\\x",
        ] {
            assert!(parse_any_text(s).is_none(), "{s:?}");
        }
    }

    #[test]
    fn feature_collection_merges() {
        let s = r#"{"type":"FeatureCollection","features":[{"type":"Feature","geometry":{"type":"Point","coordinates":[1,2]}},{"type":"Feature","geometry":{"type":"Point","coordinates":[3,4]}}]}"#;
        let f = parse_any_text(s).unwrap();
        assert!(matches!(f.geometry, Geometry::GeometryCollection(g) if g.len() == 2));
        assert!(parse_any_text(r#"{"type":"FeatureCollection","features":[]}"#).is_none());
    }

    #[test]
    fn result_variant() {
        assert!(parse_any_text_result("POINT(1 2)").is_ok());
        assert!(parse_any_text_result("nope").is_err());
    }
}
