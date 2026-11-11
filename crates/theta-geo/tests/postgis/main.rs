//! Ground-truth tests: every constant in `data.rs` was produced by a real PostGIS 3.4
//! (see the header there), so these check our decoders/encoders/projections against it.

mod data;

use data::*;
use theta_geo::*;

fn has_zm(input: &str) -> bool {
    let up = input.to_ascii_uppercase();
    up.contains(" Z") || up.contains(" M")
}

#[test]
fn hex_ewkb_both_byte_orders_and_iso_agree_with_wkt() {
    for fx in GEOMS {
        let expect = parse_wkt(fx.input).unwrap();
        let le = parse_hex_wkb(fx.ewkb_le).unwrap_or_else(|e| panic!("{}: {e}", fx.input));
        let be = parse_hex_wkb(fx.ewkb_be).unwrap_or_else(|e| panic!("{}: {e}", fx.input));
        assert_eq!(le, expect, "LE {}", fx.input);
        assert_eq!(be, expect, "BE {}", fx.input);
        let iso = parse_hex_wkb(fx.iso).unwrap_or_else(|e| panic!("{}: {e}", fx.input));
        assert_eq!(iso.geometry, expect.geometry, "ISO {}", fx.input);
        assert_eq!(iso.srid, None, "ISO WKB carries no SRID: {}", fx.input);
    }
}

#[test]
fn text_sniffer_accepts_postgis_hex_and_text() {
    for fx in GEOMS {
        let expect = parse_wkt(fx.input).unwrap();
        assert_eq!(parse_any_text(fx.ewkb_le).as_ref(), Some(&expect), "{}", fx.input);
        assert_eq!(
            parse_any_text(fx.ewkt).map(|f| f.geometry),
            Some(expect.geometry.clone()),
            "{}",
            fx.input
        );
    }
}

#[test]
fn our_ewkb_is_byte_identical_to_postgis_for_2d() {
    let mut checked = 0;
    for fx in GEOMS.iter().filter(|f| !has_zm(f.input)) {
        let hex: String = to_ewkb(&parse_wkt(fx.input).unwrap()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, fx.ewkb_le, "{}", fx.input);
        checked += 1;
    }
    assert!(checked >= 14);
}

#[test]
fn wkt_and_ewkt_output_match_postgis_text() {
    for fx in GEOMS {
        let f = parse_hex_wkb(fx.ewkb_le).unwrap();
        if fx.input.contains("MULTIPOINT") {
            // PostGIS 3.4 writes `MULTIPOINT(1 2,3 4)`; we write the OGC form `((1 2),(3 4))`
            // that every WKT reader accepts. Both must mean the same thing.
            assert_eq!(parse_wkt(&to_ewkt(&f, None)).unwrap(), parse_wkt(fx.ewkt).unwrap(), "{}", fx.input);
            continue;
        }
        assert_eq!(to_wkt(&f, None), fx.wkt, "{}", fx.input);
        assert_eq!(to_ewkt(&f, None), fx.ewkt, "{}", fx.input);
    }
}

#[test]
fn geojson_matches_postgis() {
    let mut checked = 0;
    for fx in GEOMS.iter().filter(|f| !f.geojson.is_empty()) {
        let f = parse_hex_wkb(fx.ewkb_le).unwrap();
        let mut theirs: serde_json::Value = serde_json::from_str(fx.geojson).unwrap();
        // PostGIS adds a legacy "crs" member for non-4326 SRIDs.
        let crs = theirs.as_object_mut().unwrap().remove("crs");
        assert_eq!(to_geojson(&f), theirs, "{}", fx.input);
        let back = parse_geojson(fx.geojson).unwrap();
        assert_eq!(back[0].geometry, f.geometry, "{}", fx.input);
        assert_eq!(back[0].srid, Some(if crs.is_some() { f.srid.unwrap() } else { 4326 }), "{}", fx.input);
        checked += 1;
    }
    assert!(checked >= 20, "{checked}");
}

#[test]
fn utm_matches_postgis_st_transform() {
    let mut worst: f64 = 0.0;
    for &(name, lon, lat, epsg, e, n, _, _) in PROJ {
        let crs = crs_for_srid(Some(epsg));
        let Crs::Utm { zone, south } = crs else { panic!("{epsg}") };
        // Forward.
        let fwd = lon_lat_to_utm([lon, lat], zone, south);
        let d_fwd = (fwd[0] - e).hypot(fwd[1] - n);
        // Inverse, measured on the ground.
        let inv = to_lon_lat([e, n], crs).unwrap();
        let d_inv = haversine_m([lon, lat], inv);
        worst = worst.max(d_fwd).max(d_inv);
        assert!(d_fwd < 0.01, "{name}: forward off by {d_fwd} m");
        assert!(d_inv < 0.01, "{name}: inverse off by {d_inv} m");
    }
    eprintln!("UTM worst error vs PostGIS: {worst:.6} m over {} points", PROJ.len());
}

#[test]
fn web_mercator_matches_postgis_st_transform() {
    let mut worst: f64 = 0.0;
    for &(name, lon, lat, _, _, _, mx, my) in PROJ {
        let fwd = lon_lat_to_web_mercator([lon, lat]);
        let d = (fwd[0] - mx).hypot(fwd[1] - my);
        let inv = web_mercator_to_lon_lat([mx, my]);
        let d_inv = haversine_m([lon, lat], inv);
        worst = worst.max(d).max(d_inv);
        assert!(d < 0.01 && d_inv < 0.01, "{name}: {d} / {d_inv}");
        assert_eq!(to_lon_lat([mx, my], Crs::WebMercator), Some(inv));
    }
    eprintln!("Web-Mercator worst error vs PostGIS: {worst:.6} m");
}

#[test]
fn haversine_vs_geography_distance() {
    let mut worst: f64 = 0.0;
    for &(x1, y1, x2, y2, d) in DIST {
        let ours = haversine_m([x1, y1], [x2, y2]);
        let rel = (ours - d).abs() / d;
        worst = worst.max(rel);
        assert!(rel < 0.005, "({x1},{y1})-({x2},{y2}): ours {ours} theirs {d} ({:.3}%)", rel * 100.0);
    }
    eprintln!("haversine worst relative error vs ST_Distance(geography): {:.4}%", worst * 100.0);
}

#[test]
fn area_and_perimeter_vs_geography() {
    let (mut worst_a, mut worst_p): (f64, f64) = (0.0, 0.0);
    for &(name, wkt, area, perimeter) in AREAS {
        let g = parse_wkt(wkt).unwrap().geometry;
        let (a, p) = (area_m2(&g), length_m(&g));
        let (ra, rp) = ((a - area).abs() / area, (p - perimeter).abs() / perimeter);
        worst_a = worst_a.max(ra);
        worst_p = worst_p.max(rp);
        assert!(ra < 0.01, "{name}: area ours {a} theirs {area} ({:.3}%)", ra * 100.0);
        assert!(rp < 0.005, "{name}: perimeter ours {p} theirs {perimeter} ({:.3}%)", rp * 100.0);
    }
    eprintln!("area worst: {:.4}%, perimeter worst: {:.4}%", worst_a * 100.0, worst_p * 100.0);
}

#[test]
fn line_length_vs_geography() {
    let mut worst: f64 = 0.0;
    for &(name, wkt, len) in LENGTHS {
        let g = parse_wkt(wkt).unwrap().geometry;
        let rel = (length_m(&g) - len).abs() / len;
        worst = worst.max(rel);
        assert!(rel < 0.005, "{name}: {rel}");
    }
    eprintln!("line length worst: {:.4}%", worst * 100.0);
}
