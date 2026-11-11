//! Clipboard and export text produced from the panel's state.

use crate::core::Core;
use theta_geo::{Coord, Crs, Feature, guess_crs, to_geojson_feature_collection, to_lon_lat};

/// Pretty-printed FeatureCollection of the visible layers. Coordinates are reprojected to
/// lon/lat whenever the source CRS is known (GeoJSON mandates WGS 84); planar data is
/// exported as it is. Each feature carries the row's other columns as properties, plus its
/// label and, when there are several layers, the layer name.
pub fn geojson(core: &Core) -> String {
    let mut items: Vec<(Feature, serde_json::Map<String, serde_json::Value>)> = Vec::new();
    for f in &core.prepared.features {
        if !core.layer_visible(f) {
            continue;
        }
        let src = &core.source[f.src];
        let crs = guess_crs(src.feature.srid, src.feature.geometry.bbox());
        let geom = match crs {
            Crs::Planar => src.feature.geometry.clone(),
            crs => src.feature.geometry.map_coords(&|c| to_lon_lat(c, crs).unwrap_or(c)),
        };
        let mut props = serde_json::Map::new();
        if let Some(l) = &src.label {
            props.insert("label".into(), l.clone().into());
        }
        for (k, v) in &src.properties {
            props.insert(k.clone(), v.clone().into());
        }
        if core.layers.len() > 1
            && let Some(l) = core.layers.get(f.layer)
        {
            props.insert("layer".into(), l.name.clone().into());
        }
        items.push((Feature::new(geom, Some(4326)), props));
    }
    serde_json::to_string_pretty(&to_geojson_feature_collection(&items)).unwrap_or_default()
}

/// "Copy coordinates" text: `lon, lat` with 6 decimals (about 0.1 m), or the planar pair as is.
pub fn coord_text(c: Coord, geographic: bool) -> String {
    if geographic { format!("{:.6}, {:.6}", c[0], c[1]) } else { format!("{}, {}", c[0], c[1]) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MapFeature, MapLayer};
    use theta_geo::Geometry;

    fn feature(g: Geometry, srid: Option<i32>, layer: usize) -> MapFeature {
        MapFeature {
            feature: Feature::new(g, srid),
            row: 0,
            label: Some("Vienna".into()),
            properties: vec![("country".into(), "AT".into())],
            layer,
        }
    }

    fn core() -> Core {
        let mut c = Core::new("http://127.0.0.1:9/{z}/{x}/{y}.png".into());
        c.set_features(
            vec![MapLayer { name: "a".into(), visible: true }, MapLayer { name: "b".into(), visible: false }],
            vec![
                feature(Geometry::Point(Some([601_800.0, 5_340_000.0])), Some(32633), 0),
                feature(Geometry::Point(Some([1.0, 2.0])), Some(4326), 1),
            ],
        );
        c
    }

    #[test]
    fn exports_only_visible_layers_reprojected_with_properties() {
        let v: serde_json::Value = serde_json::from_str(&geojson(&core())).unwrap();
        let feats = v["features"].as_array().unwrap();
        assert_eq!(feats.len(), 1, "layer b is hidden");
        let coords = feats[0]["geometry"]["coordinates"].as_array().unwrap();
        assert!((coords[0].as_f64().unwrap() - 16.37).abs() < 0.05, "{coords:?}");
        assert!((coords[1].as_f64().unwrap() - 48.2).abs() < 0.05);
        assert_eq!(feats[0]["properties"]["label"], "Vienna");
        assert_eq!(feats[0]["properties"]["country"], "AT");
        assert_eq!(feats[0]["properties"]["layer"], "a");
        assert_eq!(v["type"], "FeatureCollection");
    }

    #[test]
    fn empty_export_is_a_valid_collection() {
        let c = Core::new("x".into());
        let v: serde_json::Value = serde_json::from_str(&geojson(&c)).unwrap();
        assert_eq!(v["features"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn coordinate_text() {
        assert_eq!(coord_text([16.373_123_456, 48.2], true), "16.373123, 48.200000");
        assert_eq!(coord_text([12.5, -3.0], false), "12.5, -3");
    }
}
