//! Coordinate reference systems we can put on a map, and the projections between them.

use crate::geometry::{BBox, Coord};

/// Web-Mercator sphere radius in metres (WGS 84 semi-major axis; EPSG:3857 uses a sphere).
const MERC_R: f64 = 6_378_137.0;
/// Latitude at which Web-Mercator's y equals its x range (the square-world cut-off).
pub const MAX_MERCATOR_LAT: f64 = 85.051_128_78;

/// How a geometry's coordinates relate to the globe, for display purposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crs {
    /// lon/lat degrees. Covers EPSG:4326 and datums that agree with WGS 84 to within a
    /// few metres (4269 NAD83, 4283 GDA94, 7844 GDA2020, 4258 ETRS89, ...), which is
    /// invisible on a slippy map.
    Geographic,
    /// EPSG:3857 / 900913 metres.
    WebMercator,
    /// Universal Transverse Mercator on a GRS80/WGS84 ellipsoid: EPSG:326xx (north),
    /// 327xx (south), and the GDA94/GDA2020 MGA, NAD83 and ETRS89 UTM zone families.
    Utm {
        /// Zone number 1..=60.
        zone: u8,
        /// Southern hemisphere (false northing 10 000 000 m).
        south: bool,
    },
    /// Unknown or missing SRID: plain planar coordinates, drawn without a basemap.
    Planar,
}

/// Map an EPSG code to a [`Crs`]. `None`, 0 and unrecognised codes are [`Crs::Planar`];
/// use [`guess_crs`] to rescue unlabelled lon/lat data.
pub fn crs_for_srid(srid: Option<i32>) -> Crs {
    let Some(s) = srid else { return Crs::Planar };
    let utm = |zone: i32, south: bool| Crs::Utm { zone: zone as u8, south };
    match s {
        4326 | 4269 | 4283 | 7844 | 4258 | 4617 | 4167 | 4171 => Crs::Geographic,
        3857 | 3785 | 900913 | 102100 | 102113 => Crs::WebMercator,
        32601..=32660 => utm(s - 32600, false),
        32701..=32760 => utm(s - 32700, true),
        // GDA94 / MGA zones 48..=58 (all in the southern hemisphere).
        28348..=28358 => utm(s - 28300, true),
        // GDA2020 / MGA zones 46..=59.
        7846..=7859 => utm(s - 7800, true),
        // NAD83 / UTM zones 1..=23 N and ETRS89 / UTM zones 28..=38 N.
        26901..=26923 => utm(s - 26900, false),
        25828..=25838 => utm(s - 25800, false),
        _ => Crs::Planar,
    }
}

/// Like [`crs_for_srid`], but a missing/zero SRID whose bounding box fits inside
/// `[-180,180] x [-90,90]` is assumed to be lon/lat.
pub fn guess_crs(srid: Option<i32>, bbox: Option<BBox>) -> Crs {
    if matches!(srid, None | Some(0)) {
        return match bbox {
            Some(b) if b.min[0] >= -180.0 && b.max[0] <= 180.0 && b.min[1] >= -90.0 && b.max[1] <= 90.0 => {
                Crs::Geographic
            }
            _ => Crs::Planar,
        };
    }
    crs_for_srid(srid)
}

/// Convert a coordinate in `crs` to lon/lat degrees; `None` for [`Crs::Planar`].
pub fn to_lon_lat(c: Coord, crs: Crs) -> Option<Coord> {
    match crs {
        Crs::Geographic => Some(c),
        Crs::WebMercator => Some(web_mercator_to_lon_lat(c)),
        Crs::Utm { zone, south } => Some(utm_to_lon_lat(c, zone, south)),
        Crs::Planar => None,
    }
}

/// Forward spherical Mercator. With R the sphere radius:
/// `x = R * lambda`, `y = R * ln(tan(pi/4 + phi/2))`.
/// Latitude is clamped to +-85.05112878 where the projected world becomes square.
pub fn lon_lat_to_web_mercator(c: Coord) -> Coord {
    let lat = c[1].clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT).to_radians();
    [MERC_R * c[0].to_radians(), MERC_R * (std::f64::consts::FRAC_PI_4 + lat / 2.0).tan().ln()]
}

/// Inverse of [`lon_lat_to_web_mercator`]: `lambda = x / R`,
/// `phi = 2 * atan(exp(y / R)) - pi/2` (the Gudermannian function).
pub fn web_mercator_to_lon_lat(c: Coord) -> Coord {
    let lat = 2.0 * (c[1] / MERC_R).exp().atan() - std::f64::consts::FRAC_PI_2;
    [(c[0] / MERC_R).to_degrees(), lat.to_degrees()]
}

// --- Transverse Mercator (UTM) via the Kruger series ------------------------------------
//
// Transverse Mercator on an ellipsoid has no closed form in elementary functions, but
// it does have a rapidly-converging series in the third flattening n = f / (2 - f)
// (Kruger 1912, in the form given by Karney 2011). Working with the *conformal*
// latitude chi, the projection is a complex-analytic map, so after scaling by the
// rectifying radius A the northing/easting (xi, eta) relate to the conformal "sphere"
// coordinates (xi', eta') by simple sin/cos/sinh/cosh sums. Truncated at n^4 the error
// is far below a millimetre inside a UTM zone.

const WGS84_A: f64 = 6_378_137.0;
const WGS84_F: f64 = 1.0 / 298.257_223_563;
const UTM_K0: f64 = 0.9996;
const UTM_E0: f64 = 500_000.0;
const UTM_N0_SOUTH: f64 = 10_000_000.0;

struct Kruger {
    /// Rectifying radius times k0.
    a: f64,
    alpha: [f64; 4],
    beta: [f64; 4],
    delta: [f64; 4],
    n: f64,
}

fn kruger() -> Kruger {
    let n = WGS84_F / (2.0 - WGS84_F);
    let (n2, n3, n4) = (n * n, n * n * n, n * n * n * n);
    let a = WGS84_A / (1.0 + n) * (1.0 + n2 / 4.0 + n4 / 64.0) * UTM_K0;
    Kruger {
        a,
        n,
        alpha: [
            n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0 + 41.0 * n4 / 180.0,
            13.0 * n2 / 48.0 - 3.0 * n3 / 5.0 + 557.0 * n4 / 1440.0,
            61.0 * n3 / 240.0 - 103.0 * n4 / 140.0,
            49561.0 * n4 / 161280.0,
        ],
        beta: [
            n / 2.0 - 2.0 * n2 / 3.0 + 37.0 * n3 / 96.0 - n4 / 360.0,
            n2 / 48.0 + n3 / 15.0 - 437.0 * n4 / 1440.0,
            17.0 * n3 / 480.0 - 37.0 * n4 / 840.0,
            4397.0 * n4 / 161280.0,
        ],
        delta: [
            2.0 * n - 2.0 * n2 / 3.0 - 2.0 * n3 + 116.0 * n4 / 45.0,
            7.0 * n2 / 3.0 - 8.0 * n3 / 5.0 - 227.0 * n4 / 45.0,
            56.0 * n3 / 15.0 - 136.0 * n4 / 35.0,
            4279.0 * n4 / 630.0,
        ],
    }
}

/// Longitude of a zone's central meridian in degrees.
fn central_meridian(zone: u8) -> f64 {
    (f64::from(zone) - 1.0) * 6.0 - 180.0 + 3.0
}

/// Inverse UTM: easting/northing metres to lon/lat degrees.
///
/// 1. Remove the false origin and scale: `xi = N / A`, `eta = E / A`.
/// 2. Undo the Kruger series: `xi' = xi - sum beta_j sin(2j xi) cosh(2j eta)`, and
///    `eta' = eta - sum beta_j cos(2j xi) sinh(2j eta)`.
/// 3. Conformal latitude `chi = asin(sin xi' / cosh eta')`.
/// 4. Geodetic latitude `phi = chi + sum delta_j sin(2j chi)`.
/// 5. `lambda = lambda0 + atan2(sinh eta', cos xi')`.
pub fn utm_to_lon_lat(c: Coord, zone: u8, south: bool) -> Coord {
    let k = kruger();
    let n0 = if south { UTM_N0_SOUTH } else { 0.0 };
    let xi = (c[1] - n0) / k.a;
    let eta = (c[0] - UTM_E0) / k.a;
    let (mut xi_p, mut eta_p) = (xi, eta);
    for (j, b) in k.beta.iter().enumerate() {
        let m = 2.0 * (j as f64 + 1.0);
        xi_p -= b * (m * xi).sin() * (m * eta).cosh();
        eta_p -= b * (m * xi).cos() * (m * eta).sinh();
    }
    let chi = (xi_p.sin() / eta_p.cosh()).asin();
    let mut phi = chi;
    for (j, d) in k.delta.iter().enumerate() {
        phi += d * (2.0 * (j as f64 + 1.0) * chi).sin();
    }
    let lambda = central_meridian(zone).to_radians() + eta_p.sinh().atan2(xi_p.cos());
    [lambda.to_degrees(), phi.to_degrees()]
}

/// Forward UTM (lon/lat degrees to easting/northing metres) in the given zone.
///
/// The mirror image of [`utm_to_lon_lat`]: geodetic to conformal latitude
/// via `t = sinh(atanh(sin phi) - (2 sqrt(n) / (1 + n)) atanh((2 sqrt(n) / (1 + n)) sin phi))`,
/// then `xi' = atan(t / cos dl)`, `eta' = atanh(sin dl / sqrt(1 + t^2))` and the alpha series.
pub fn lon_lat_to_utm(c: Coord, zone: u8, south: bool) -> Coord {
    let k = kruger();
    let phi = c[1].to_radians();
    let dl = (c[0] - central_meridian(zone)).to_radians();
    let q = 2.0 * k.n.sqrt() / (1.0 + k.n);
    let t = (phi.sin().atanh() - q * (q * phi.sin()).atanh()).sinh();
    let xi_p = t.atan2(dl.cos());
    let eta_p = (dl.sin() / (1.0 + t * t).sqrt()).atanh();
    let (mut xi, mut eta) = (xi_p, eta_p);
    for (j, a) in k.alpha.iter().enumerate() {
        let m = 2.0 * (j as f64 + 1.0);
        xi += a * (m * xi_p).sin() * (m * eta_p).cosh();
        eta += a * (m * xi_p).cos() * (m * eta_p).sinh();
    }
    let n0 = if south { UTM_N0_SOUTH } else { 0.0 };
    [UTM_E0 + k.a * eta, n0 + k.a * xi]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::haversine_m;

    #[test]
    fn srid_mapping() {
        assert_eq!(crs_for_srid(Some(4326)), Crs::Geographic);
        for s in [4269, 4283, 7844, 4258] {
            assert_eq!(crs_for_srid(Some(s)), Crs::Geographic);
        }
        assert_eq!(crs_for_srid(Some(3857)), Crs::WebMercator);
        assert_eq!(crs_for_srid(Some(900913)), Crs::WebMercator);
        assert_eq!(crs_for_srid(Some(32633)), Crs::Utm { zone: 33, south: false });
        assert_eq!(crs_for_srid(Some(32756)), Crs::Utm { zone: 56, south: true });
        assert_eq!(crs_for_srid(Some(32601)), Crs::Utm { zone: 1, south: false });
        assert_eq!(crs_for_srid(Some(32760)), Crs::Utm { zone: 60, south: true });
        assert_eq!(crs_for_srid(Some(28356)), Crs::Utm { zone: 56, south: true });
        assert_eq!(crs_for_srid(Some(7856)), Crs::Utm { zone: 56, south: true });
        assert_eq!(crs_for_srid(Some(7846)), Crs::Utm { zone: 46, south: true });
        assert_eq!(crs_for_srid(Some(26910)), Crs::Utm { zone: 10, south: false });
        assert_eq!(crs_for_srid(Some(25832)), Crs::Utm { zone: 32, south: false });
    }

    #[test]
    fn unknown_srid_is_planar() {
        assert_eq!(crs_for_srid(None), Crs::Planar);
        assert_eq!(crs_for_srid(Some(0)), Crs::Planar);
        assert_eq!(crs_for_srid(Some(27700)), Crs::Planar);
        assert_eq!(crs_for_srid(Some(32600)), Crs::Planar);
        assert_eq!(crs_for_srid(Some(32661)), Crs::Planar);
        assert_eq!(crs_for_srid(Some(-1)), Crs::Planar);
    }

    #[test]
    fn guessing() {
        let inside = BBox { min: [-10.0, -5.0], max: [170.0, 80.0] };
        let outside = BBox { min: [0.0, 0.0], max: [500_000.0, 6_000_000.0] };
        assert_eq!(guess_crs(None, Some(inside)), Crs::Geographic);
        assert_eq!(guess_crs(Some(0), Some(inside)), Crs::Geographic);
        assert_eq!(guess_crs(None, Some(outside)), Crs::Planar);
        assert_eq!(guess_crs(None, None), Crs::Planar);
        assert_eq!(guess_crs(Some(3857), Some(inside)), Crs::WebMercator);
        assert_eq!(guess_crs(Some(27700), Some(inside)), Crs::Planar);
    }

    #[test]
    fn web_mercator_known_values() {
        let m = lon_lat_to_web_mercator([0.0, 0.0]);
        assert!(m[0].abs() < 1e-9 && m[1].abs() < 1e-9);
        let m = lon_lat_to_web_mercator([180.0, 0.0]);
        assert!((m[0] - 20_037_508.342_789_244).abs() < 1e-6);
        let m = lon_lat_to_web_mercator([0.0, MAX_MERCATOR_LAT]);
        assert!((m[1] - 20_037_508.34).abs() < 1.0, "{}", m[1]);
    }

    #[test]
    fn web_mercator_clamps_latitude() {
        let a = lon_lat_to_web_mercator([0.0, 89.9]);
        let b = lon_lat_to_web_mercator([0.0, 85.05112878]);
        assert_eq!(a, b);
        assert!(lon_lat_to_web_mercator([0.0, 90.0])[1].is_finite());
        assert!(lon_lat_to_web_mercator([0.0, -90.0])[1].is_finite());
    }

    #[test]
    fn web_mercator_roundtrip() {
        for (lon, lat) in [(0.0, 0.0), (151.2093, -33.8688), (-122.4, 37.8), (179.9, 84.0), (-179.9, -84.0)] {
            let back = web_mercator_to_lon_lat(lon_lat_to_web_mercator([lon, lat]));
            assert!((back[0] - lon).abs() < 1e-9 && (back[1] - lat).abs() < 1e-9, "{lon} {lat}");
        }
    }

    #[test]
    fn utm_central_meridian_on_equator() {
        let e = lon_lat_to_utm([3.0, 0.0], 31, false);
        assert!((e[0] - 500_000.0).abs() < 1e-6 && e[1].abs() < 1e-6);
        assert_eq!(central_meridian(31), 3.0);
        assert_eq!(central_meridian(1), -177.0);
        assert_eq!(central_meridian(60), 177.0);
    }

    #[test]
    fn utm_known_value_sydney() {
        // Published GDA/WGS84 value for the Sydney Opera House area, EPSG:32756.
        let p = lon_lat_to_utm([151.2153, -33.8568], 56, true);
        assert!((p[0] - 334_900.0).abs() < 200.0 && (p[1] - 6_252_300.0).abs() < 200.0, "{p:?}");
    }

    #[test]
    fn utm_roundtrip_over_grid() {
        for zone in [1u8, 17, 31, 56, 60] {
            for south in [false, true] {
                for dlon in [-3.0, -1.0, 0.0, 2.5, 3.0] {
                    for lat in [-80.0, -45.0, -0.01, 0.0, 30.0, 60.0, 84.0] {
                        let lon = central_meridian(zone) + dlon;
                        let en = lon_lat_to_utm([lon, lat], zone, south);
                        let back = utm_to_lon_lat(en, zone, south);
                        let d = haversine_m([lon, lat], back);
                        assert!(d < 0.001, "zone {zone} {lon},{lat}: {d} m");
                    }
                }
            }
        }
    }

    #[test]
    fn to_lon_lat_dispatch() {
        assert_eq!(to_lon_lat([1.0, 2.0], Crs::Geographic), Some([1.0, 2.0]));
        assert_eq!(to_lon_lat([1.0, 2.0], Crs::Planar), None);
        let ll = to_lon_lat([500_000.0, 0.0], Crs::Utm { zone: 31, south: false }).unwrap();
        assert!((ll[0] - 3.0).abs() < 1e-9 && ll[1].abs() < 1e-9);
        let ll = to_lon_lat([20_037_508.342_789_244, 0.0], Crs::WebMercator).unwrap();
        assert!((ll[0] - 180.0).abs() < 1e-9);
    }

    #[test]
    fn utm_hemisphere_offsets() {
        let n = utm_to_lon_lat([500_000.0, 0.0], 31, false);
        let s = utm_to_lon_lat([500_000.0, 10_000_000.0], 31, true);
        assert!(n[1].abs() < 1e-9 && s[1].abs() < 1e-9);
    }
}
