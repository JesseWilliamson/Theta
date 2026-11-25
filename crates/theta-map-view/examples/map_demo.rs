//! Interactive demo of the map viewer.
//!
//!     cargo run -p theta-map-view --example map_demo -- [--theme "Theta Light"]
//!         [--cities | --planar | --utm | --mixed | --stress N | --points N | --polys N] [--select ROW]
//!         [--kind auto|points|heatmap|hexagons|grid|clusters] [--color-by COLUMN] [--tiles on|off]
//!         [--boundaries] [--bin-value sum|mean|count] [--pan] [--zoom]
//!
//! `THETA_WINDOW_SIZE=WxH` sets the window size, like the app. `THETA_TILE_URL_OVERRIDE`
//! replaces the tile URL template (point it at an unreachable host to see offline behaviour).

use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, WindowBounds, WindowOptions, div, px, size,
};
use std::time::Duration;
use theta_geo::{Feature, Geometry, lon_lat_to_utm};
use std::sync::Arc;
use theta_map_view::{
    ClassMethod, ColorBy, ColorScheme, ColorSpec, LayerKind, LayerStyle, MapEvent, MapFeature, MapLayer, MapPanel,
    MapValue, PaletteId, TilingMode,
};

struct Demo {
    map: Entity<MapPanel>,
    _sub: Subscription,
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.map.clone())
    }
}

fn feat(
    g: Geometry,
    srid: Option<i32>,
    row: usize,
    layer: usize,
    label: &str,
    props: &[(&str, String)],
) -> MapFeature {
    MapFeature {
        feature: Feature::new(g, srid),
        row,
        label: (!label.is_empty()).then(|| label.to_string()),
        properties: props.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        values: vec![],
        layer,
    }
}

type Dataset = (Vec<MapLayer>, Vec<MapFeature>);

fn layer(name: &str) -> MapLayer {
    MapLayer::new(name, vec![])
}

const CITIES: &[(&str, &str, f64, f64, f64)] = &[
    ("Tokyo", "Japan", 37.4, 139.6917, 35.6895),
    ("Delhi", "India", 32.9, 77.1025, 28.7041),
    ("Shanghai", "China", 29.2, 121.4737, 31.2304),
    ("S\u{00e3}o Paulo", "Brazil", 22.6, -46.6333, -23.5505),
    ("Mexico City", "Mexico", 22.1, -99.1332, 19.4326),
    ("Cairo", "Egypt", 21.3, 31.2357, 30.0444),
    ("Mumbai", "India", 21.0, 72.8777, 19.0760),
    ("Beijing", "China", 20.9, 116.4074, 39.9042),
    ("Dhaka", "Bangladesh", 22.5, 90.4125, 23.8103),
    ("Osaka", "Japan", 19.0, 135.5023, 34.6937),
    ("New York", "United States", 18.8, -74.0060, 40.7128),
    ("Karachi", "Pakistan", 16.8, 67.0011, 24.8607),
    ("Buenos Aires", "Argentina", 15.4, -58.3816, -34.6037),
    ("Istanbul", "Turkey", 15.6, 28.9784, 41.0082),
    ("Lagos", "Nigeria", 15.9, 3.3792, 6.5244),
    ("Manila", "Philippines", 14.4, 120.9842, 14.5995),
    ("Rio de Janeiro", "Brazil", 13.6, -43.1729, -22.9068),
    ("Los Angeles", "United States", 12.5, -118.2437, 34.0522),
    ("Moscow", "Russia", 12.6, 37.6173, 55.7558),
    ("Paris", "France", 11.1, 2.3522, 48.8566),
    ("Jakarta", "Indonesia", 11.0, 106.8456, -6.2088),
    ("London", "United Kingdom", 9.6, -0.1276, 51.5072),
    ("Lima", "Peru", 11.2, -77.0428, -12.0464),
    ("Bangkok", "Thailand", 10.7, 100.5018, 13.7563),
    ("Johannesburg", "South Africa", 6.1, 28.0473, -26.2041),
    ("Cape Town", "South Africa", 4.8, 18.4241, -33.9249),
    ("Nairobi", "Kenya", 5.0, 36.8219, -1.2921),
    ("Sydney", "Australia", 5.3, 151.2093, -33.8688),
    ("Melbourne", "Australia", 5.1, 144.9631, -37.8136),
    ("Perth", "Australia", 2.1, 115.8605, -31.9505),
    ("Auckland", "New Zealand", 1.7, 174.7633, -36.8485),
    ("Singapore", "Singapore", 5.9, 103.8198, 1.3521),
    ("Dubai", "United Arab Emirates", 3.6, 55.2708, 25.2048),
    ("Berlin", "Germany", 3.7, 13.4050, 52.5200),
    ("Madrid", "Spain", 6.7, -3.7038, 40.4168),
    ("Rome", "Italy", 4.3, 12.4964, 41.9028),
    ("Toronto", "Canada", 6.3, -79.3832, 43.6532),
    ("Chicago", "United States", 8.9, -87.6298, 41.8781),
    ("Seoul", "South Korea", 9.9, 126.9780, 37.5665),
    ("Reykjav\u{00ed}k", "Iceland", 0.24, -21.8174, 64.1265),
    ("Anchorage", "United States", 0.29, -149.9003, 61.2181),
];

fn cities() -> Dataset {
    let mut features = Vec::new();
    let mut row = 0;
    for (name, country, pop, lon, lat) in CITIES {
        features.push(feat(
            Geometry::Point(Some([*lon, *lat])),
            Some(4326),
            row,
            0,
            name,
            &[
                ("name", name.to_string()),
                ("country", country.to_string()),
                ("population", format!("{:.1} M", pop)),
                ("founded", "unknown".into()),
                ("elevation_m", "12".into()),
                ("timezone", "UTC+0".into()),
                ("notes", "A deliberately long note to prove that very long values are truncated in the hover card instead of growing it".into()),
                ("source", "demo".into()),
                ("extra_1", "x".into()),
                ("extra_2", "y".into()),
            ],
        ));
        row += 1;
    }
    let ring = |c: &[(f64, f64)]| c.iter().map(|(x, y)| [*x, *y]).collect::<Vec<_>>();
    let south_africa = ring(&[
        (16.5, -28.6),
        (19.9, -28.4),
        (20.0, -24.8),
        (25.2, -25.7),
        (27.0, -23.6),
        (29.4, -22.1),
        (31.5, -22.4),
        (32.0, -26.8),
        (32.9, -26.9),
        (31.2, -29.3),
        (30.0, -31.4),
        (27.0, -33.5),
        (25.6, -34.0),
        (22.0, -34.2),
        (20.0, -34.8),
        (18.4, -34.3),
        (17.9, -32.8),
        (16.5, -28.6),
    ]);
    let lesotho =
        ring(&[(27.0, -29.5), (28.2, -28.6), (29.4, -29.0), (29.2, -30.2), (28.0, -30.6), (27.0, -29.5)]);
    features.push(feat(
        Geometry::Polygon(vec![south_africa, lesotho]),
        Some(4326),
        row,
        1,
        "South Africa (with Lesotho hole)",
        &[("name", "South Africa".into()), ("note", "polygon with a hole".into())],
    ));
    row += 1;
    let honshu = ring(&[
        (139.9, 35.0),
        (141.0, 38.0),
        (141.5, 41.4),
        (140.0, 40.5),
        (139.5, 38.0),
        (137.0, 37.0),
        (136.0, 35.7),
        (133.0, 35.5),
        (131.0, 34.2),
        (132.5, 34.0),
        (135.0, 33.5),
        (136.9, 34.6),
        (138.5, 34.6),
        (139.9, 35.0),
    ]);
    let hokkaido = ring(&[(140.0, 42.0), (141.5, 45.4), (145.3, 43.4), (143.0, 42.0), (140.0, 42.0)]);
    let kyushu =
        ring(&[(130.0, 33.5), (131.8, 33.7), (131.2, 31.3), (130.2, 31.2), (129.8, 32.8), (130.0, 33.5)]);
    features.push(feat(
        Geometry::MultiPolygon(vec![vec![honshu], vec![hokkaido], vec![kyushu]]),
        Some(4326),
        row,
        1,
        "Japan (main islands)",
        &[("name", "Japan".into()), ("parts", "3".into())],
    ));
    row += 1;
    features.push(feat(
        Geometry::Polygon(vec![ring(&[
            (-109.05, 37.0),
            (-102.05, 37.0),
            (-102.05, 41.0),
            (-109.05, 41.0),
            (-109.05, 37.0),
        ])]),
        Some(4326),
        row,
        1,
        "Colorado",
        &[("name", "Colorado".into())],
    ));
    row += 1;
    let route = |pts: &[(f64, f64)]| Geometry::LineString(pts.iter().map(|(x, y)| [*x, *y]).collect());
    features.push(feat(
        route(&[(-0.1276, 51.5072), (-30.0, 55.0), (-55.0, 50.0), (-74.006, 40.7128)]),
        Some(4326),
        row,
        2,
        "London \u{2013} New York",
        &[("carrier", "demo air".into())],
    ));
    row += 1;
    features.push(feat(
        route(&[(151.2093, -33.8688), (125.0, -20.0), (103.8198, 1.3521), (80.0, 15.0), (55.2708, 25.2048)]),
        Some(4326),
        row,
        2,
        "Sydney \u{2013} Dubai",
        &[("carrier", "demo air".into())],
    ));
    row += 1;
    features.push(feat(
        route(&[
            (18.4241, -33.9249),
            (24.0, -20.0),
            (28.0473, -26.2041),
            (36.8219, -1.2921),
            (31.2357, 30.0444),
        ]),
        Some(4326),
        row,
        2,
        "Cape Town \u{2013} Cairo",
        &[("carrier", "demo air".into())],
    ));
    (vec![layer("cities"), layer("regions"), layer("routes")], features)
}

fn utm() -> Dataset {
    let p = |lon: f64, lat: f64| lon_lat_to_utm([lon, lat], 33, false);
    let mut f = Vec::new();
    let landmarks = [
        ("Stephansdom", "Vienna", 16.3731, 48.2082),
        ("Sch\u{00f6}nbrunn Palace", "Vienna", 16.3122, 48.1845),
        ("Wiener Riesenrad", "Vienna", 16.3959, 48.2166),
        ("Prague Castle", "Prague", 14.4005, 50.0910),
        ("Old Town Square", "Prague", 14.4208, 50.0875),
        ("Petrin Tower", "Prague", 14.3955, 50.0834),
    ];
    for (i, (name, city, lon, lat)) in landmarks.iter().enumerate() {
        f.push(feat(
            Geometry::Point(Some(p(*lon, *lat))),
            Some(32633),
            i,
            0,
            name,
            &[("city", city.to_string()), ("srid", "32633".into())],
        ));
    }
    f.push(feat(
        Geometry::LineString(vec![p(16.3731, 48.2082), p(15.6, 49.0), p(14.9, 49.6), p(14.4208, 50.0875)]),
        Some(32633),
        6,
        1,
        "Vienna \u{2013} Prague (straight)",
        &[("kind", "demo line".into())],
    ));
    let garden = vec![
        p(16.3040, 48.1790),
        p(16.3190, 48.1790),
        p(16.3190, 48.1870),
        p(16.3040, 48.1870),
        p(16.3040, 48.1790),
    ];
    f.push(feat(
        Geometry::Polygon(vec![garden]),
        Some(32633),
        7,
        1,
        "Sch\u{00f6}nbrunn gardens",
        &[("kind", "park".into())],
    ));
    (vec![layer("landmarks"), layer("shapes")], f)
}

fn planar() -> Dataset {
    let rect = |x0: f64, y0: f64, x1: f64, y1: f64| vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]];
    let mut f = Vec::new();
    f.push(feat(
        Geometry::Polygon(vec![rect(0.0, 0.0, 1200.0, 800.0), rect(500.0, 300.0, 700.0, 500.0)]),
        None,
        0,
        0,
        "Hall (with pillar court)",
        &[("kind", "room".into())],
    ));
    f.push(feat(
        Geometry::Polygon(vec![rect(1250.0, 0.0, 1650.0, 400.0)]),
        None,
        1,
        0,
        "Annex",
        &[("kind", "room".into())],
    ));
    f.push(feat(
        Geometry::LineString(vec![[100.0, 100.0], [600.0, 150.0], [900.0, 650.0], [1450.0, 200.0]]),
        None,
        2,
        1,
        "Cable run",
        &[("kind", "cable".into())],
    ));
    for (i, (x, y)) in
        [(150.0, 650.0), (400.0, 120.0), (1000.0, 400.0), (1450.0, 320.0), (600.0, 400.0)].iter().enumerate()
    {
        f.push(feat(
            Geometry::Point(Some([*x, *y])),
            None,
            3 + i,
            2,
            &format!("Sensor {}", i + 1),
            &[("unit", "\u{00b0}C".into())],
        ));
    }
    (vec![layer("rooms"), layer("cables"), layer("sensors")], f)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
    fn normal(&mut self) -> f64 {
        let (a, b) = (self.next().max(1e-12), self.next());
        (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }
}

fn stress(n: usize) -> Dataset {
    let mut rng = Rng(42);
    let mut f = Vec::with_capacity(n + n / 10);
    let centres: Vec<(f64, f64)> = CITIES.iter().map(|c| (c.3, c.4)).collect();
    for i in 0..n {
        let (cx, cy) = centres[i % centres.len()];
        let (x, y) = (cx + rng.normal() * 3.0, (cy + rng.normal() * 2.0).clamp(-80.0, 80.0));
        f.push(feat(
            Geometry::Point(Some([x, y])),
            Some(4326),
            i,
            0,
            &format!("pt {i}"),
            &[("i", i.to_string())],
        ));
    }
    // ~100 vertices each: 5000 polygons are 500k vertices at N = 50 000.
    for j in 0..n / 10 {
        let (cx, cy) = centres[j % centres.len()];
        let (x, y) = (cx + rng.normal() * 6.0, (cy + rng.normal() * 4.0).clamp(-75.0, 75.0));
        let r = 0.2 + rng.next() * 1.2;
        let mut ring: Vec<[f64; 2]> = (0..100)
            .map(|k| {
                let a = std::f64::consts::TAU * k as f64 / 100.0;
                let rr = r * (1.0 + 0.25 * (a * 5.0).sin() + 0.08 * rng.normal());
                [x + rr * a.cos(), y + rr * a.sin() * 0.8]
            })
            .collect();
        ring.push(ring[0]);
        f.push(feat(Geometry::Polygon(vec![ring]), Some(4326), n + j, 1, &format!("poly {j}"), &[]));
    }
    (vec![layer("points"), layer("polygons")], f)
}

/// `n` polygons of 10 000 vertices each (500k vertices at n = 50).
fn bigpoly(n: usize) -> Dataset {
    let mut rng = Rng(7);
    let mut f = Vec::new();
    for j in 0..n {
        let (x, y) = (-150.0 + rng.next() * 300.0, -50.0 + rng.next() * 100.0);
        let r = 2.0 + rng.next() * 6.0;
        let ph = rng.next() * 6.0;
        let mut ring: Vec<[f64; 2]> = (0..10_000)
            .map(|k| {
                let a = std::f64::consts::TAU * k as f64 / 10_000.0;
                let rr = r
                    * (1.0
                        + 0.2 * (a * 7.0 + ph).sin()
                        + 0.03 * (a * 97.0).sin()
                        + 0.004 * (a * 1013.0).sin());
                [x + rr * a.cos(), y + rr * a.sin() * 0.8]
            })
            .collect();
        ring.push(ring[0]);
        f.push(feat(Geometry::Polygon(vec![ring]), Some(4326), j, 0, &format!("big {j}"), &[]));
    }
    (vec![layer("polygons")], f)
}

fn val_feat(g: Geometry, row: usize, layer: usize, label: &str, values: Vec<MapValue>, cols: &[&str]) -> MapFeature {
    let props: Vec<(&str, String)> =
        cols.iter().zip(&values).map(|(c, v)| (*c, v.text().unwrap_or_default())).collect();
    let mut f = feat(g, Some(4326), row, layer, label, &props);
    f.values = values;
    f
}

/// `n` points in Gaussian blobs around the cities, with a numeric weight (log-normal) and a
/// category, so layers can colour, size and weight by columns.
fn points(n: usize) -> Dataset {
    let mut rng = Rng(11);
    let centres: Vec<(f64, f64)> = CITIES.iter().map(|c| (c.3, c.4)).collect();
    let cats = ["retail", "office", "depot", "kiosk", "locker"];
    let cols = ["weight", "category"];
    let f = (0..n)
        .map(|i| {
            let (cx, cy) = centres[i % centres.len()];
            let (x, y) = (cx + rng.normal() * 2.5, (cy + rng.normal() * 1.8).clamp(-80.0, 80.0));
            let w = (rng.normal() * 0.9 + 2.0).exp();
            val_feat(
                Geometry::Point(Some([x, y])),
                i,
                0,
                &format!("pt {i}"),
                vec![MapValue::Num(w.round()), MapValue::Text(cats[(rng.next() * 5.0) as usize % 5].into())],
                &cols,
            )
        })
        .collect();
    let mut l = MapLayer::new("points", cols.iter().map(|c| c.to_string()).collect());
    l.visible = true;
    (vec![l], f)
}

/// `n` wobbly polygons (about 200 vertices each) laid out on a jittered world grid, each with a
/// numeric `density` that varies smoothly with position and a `region`.
fn polys(n: usize) -> Dataset {
    let mut rng = Rng(23);
    let cols = ["density", "region"];
    let side = (n as f64).sqrt().ceil() as usize;
    let (dx, dy) = (340.0 / side as f64, 140.0 / side as f64);
    let regions = ["north", "south", "east", "west"];
    let f = (0..n)
        .map(|j| {
            let (gx, gy) = ((j % side) as f64, (j / side) as f64);
            let (x, y) = (-170.0 + (gx + 0.5) * dx, -70.0 + (gy + 0.5) * dy);
            let r = dx.min(dy) * (0.35 + 0.15 * rng.next());
            let ph = rng.next() * 6.0;
            let mut ring: Vec<[f64; 2]> = (0..200)
                .map(|k| {
                    let a = std::f64::consts::TAU * k as f64 / 200.0;
                    let rr = r * (1.0 + 0.18 * (a * 5.0 + ph).sin() + 0.04 * (a * 31.0).sin() + 0.01 * (a * 97.0).sin());
                    [x + rr * a.cos(), y + rr * a.sin() * 0.8]
                })
                .collect();
            ring.push(ring[0]);
            let density = 50.0 + 40.0 * (x / 40.0).sin() * (y / 25.0).cos() + rng.normal() * 6.0;
            val_feat(
                Geometry::Polygon(vec![ring]),
                j,
                0,
                &format!("zone {j}"),
                vec![MapValue::Num(density.round()), MapValue::Text(regions[(j % side) * 4 / side].into())],
                &cols,
            )
        })
        .collect();
    (vec![MapLayer::new("zones", cols.iter().map(|c| c.to_string()).collect())], f)
}

fn mixed() -> Dataset {
    let (layers, mut f) = cities();
    let base = f.len();
    for i in 0..12 {
        f.push(feat(Geometry::Point(Some([5000.0 + i as f64 * 10.0, 9000.0])), None, base + i, 0, "", &[]));
    }
    (layers, f)
}

fn main() {
    let mut theme: Option<String> = None;
    let mut which = "cities".to_string();
    let mut stress_n = 50_000usize;
    let mut select: Option<usize> = None;
    let (mut kind, mut color_by, mut tiles, mut boundaries) = (None::<String>, None::<String>, None::<String>, false);
    let mut bin_value: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--theme" => theme = args.next(),
            "--cities" | "--planar" | "--utm" | "--mixed" | "--empty" => {
                which = a.trim_start_matches("--").to_string()
            }
            "--stress" => {
                which = "stress".into();
                if let Some(n) = args.next().and_then(|v| v.parse().ok()) {
                    stress_n = n;
                }
            }
            "--bigpoly" => {
                which = "bigpoly".into();
                if let Some(n) = args.next().and_then(|v| v.parse().ok()) {
                    stress_n = n;
                }
            }
            "--points" | "--polys" => {
                which = a.trim_start_matches("--").to_string();
                if let Some(n) = args.next().and_then(|v| v.parse().ok()) {
                    stress_n = n;
                }
            }
            "--kind" => kind = args.next(),
            "--color-by" => color_by = args.next(),
            "--tiles" => tiles = args.next(),
            "--bin-value" => bin_value = args.next(),
            "--boundaries" => boundaries = true,
            "--select" => select = args.next().and_then(|v| v.parse().ok()),
            _ => {}
        }
    }
    theta_ui::application().run(move |cx| {
        theta_ui::init(cx);
        if let Some(name) = theme.clone() {
            theta_ui::themes::set_theme(&name, cx);
        }
        let (w, h) = theta_ui::window_size_from_env().unwrap_or((1200., 760.));
        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        let which = which.clone();
        let (_, view) = gpui_kit::open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            cx,
            move |window, cx| {
                let map = cx.new(|cx| MapPanel::new(window, cx));
                let (layers, features) = match which.as_str() {
                    "planar" => planar(),
                    "utm" => utm(),
                    "mixed" => mixed(),
                    "stress" => stress(stress_n),
                    "bigpoly" => bigpoly(stress_n),
                    "points" => points(stress_n),
                    "polys" => polys(stress_n),
                    "empty" => (vec![], vec![]),
                    _ => cities(),
                };
                let mut layers = layers;
                for l in &mut layers {
                    if kind.is_none() && color_by.is_none() && tiles.is_none() && !boundaries && bin_value.is_none() {
                        break;
                    }
                    let mut st = LayerStyle::default();
                    st.kind = match kind.as_deref() {
                        Some("points") => LayerKind::Points,
                        Some("heatmap") => LayerKind::Heatmap,
                        Some("hexagons") => LayerKind::Hexagons,
                        Some("grid") => LayerKind::Grid,
                        Some("clusters") => LayerKind::Clusters,
                        _ => LayerKind::Auto,
                    };
                    if let Some(c) = &color_by {
                        let numeric = c != "category" && c != "region";
                        st.color = ColorSpec::ByColumn(ColorBy {
                            column: c.clone(),
                            scheme: if numeric {
                                ColorScheme::Numeric { method: ClassMethod::Quantile, classes: 7 }
                            } else {
                                ColorScheme::Categorical { top_n: 8 }
                            },
                            palette: if numeric { PaletteId::Viridis } else { PaletteId::Tableau10 },
                            reversed: false,
                        });
                    }
                    if let Some(v) = &bin_value {
                        st.bins.value = match v.as_str() {
                            "sum" => theta_map_view::BinValue::Sum,
                            "mean" => theta_map_view::BinValue::Mean,
                            _ => theta_map_view::BinValue::Count,
                        };
                        st.bins.column = l.columns.first().cloned();
                        st.heatmap.weight = l.columns.first().cloned();
                    }
                    st.tiling.mode = match tiles.as_deref() {
                        Some("on") => TilingMode::On,
                        Some("off") => TilingMode::Off,
                        _ => TilingMode::Auto,
                    };
                    st.tiling.show_boundaries = boundaries;
                    l.style = Some(st);
                }
                map.update(cx, |m, cx| m.set_features(layers, Arc::new(features), window, cx));
                let handle = map.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
                if let Some(row) = select {
                    map.update(cx, |m, cx| m.select_row(Some(row), true, cx));
                }
                let sub = cx.subscribe(&map, |_, ev: &MapEvent, _| match ev {
                    MapEvent::FeatureClicked { row } => eprintln!("clicked row {row}"),
                    MapEvent::ExportGeoJson(text) => eprintln!("export: {} bytes", text.len()),
                    MapEvent::CopyText(text) => eprintln!("copied: {text}"),
                });
                cx.new(|_| Demo { map: map.clone(), _sub: sub })
            },
        )
        .expect("open window");
        if std::env::var_os("THETA_MAP_STATS").is_some() {
            let entity = view.read(cx).map.clone();
            // Frame timing for the stress runs: print paint statistics once a second.
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let (frames, avg, max) = entity.read_with(cx, |m, _| m.take_paint_stats());
                    let lp = entity.read_with(cx, |m, _| m.take_layer_perf());
                    if frames > 0 || lp.tiles_built > 0 {
                        eprintln!(
                            "paint: {frames} frames, avg {avg:.2} ms, max {max:.2} ms; tiles built {} cached {} ({} KB) missing {}; agg {:.0} ms",
                            lp.tiles_built,
                            lp.tiles_cached,
                            lp.tile_cache_bytes / 1024,
                            lp.tiles_missing,
                            lp.last_agg_ms
                        );
                    }
                }
            })
            .detach();
        }
    });
}
