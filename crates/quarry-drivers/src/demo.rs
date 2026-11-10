//! Deterministic demo dataset: a small commerce schema plus a geography layer.
//!
//! The data is generated in Rust from a fixed-seed PRNG, so every run (and every
//! database) gets exactly the same rows, and orders' totals really are the sum of
//! their items. [`seed`] drops and recreates the demo tables, which makes it
//! safe to run repeatedly.

use quarry_core::*;

/// Create (idempotently: drop & recreate) a realistic demo dataset in the
/// connected database.
///
/// Objects are created in the connection's *current* schema/database
/// (`current_schema()` / `DATABASE()` / `main`) and always referred to by their
/// qualified names, so seeding never touches same-named objects elsewhere.
pub async fn seed(conn: &dyn Connection) -> Result<()> {
    let dialect = Dialect::detect(conn).await?;
    let data = Data::generate();
    let mut run = Runner { conn };
    match dialect {
        Dialect::Sqlite => seed_sqlite(&mut run, &data).await,
        _ => seed_server(&mut run, &data, &dialect).await,
    }
}

// ---------------------------------------------------------------------------
// Deterministic data
// ---------------------------------------------------------------------------

/// SplitMix64: tiny, well distributed, and good enough for demo data.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1) as u64) as i64
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

const FIRST: &[&str] = &[
    "Ada", "Brian", "Chloe", "Dmitri", "Elena", "Farid", "Grace", "Hiro", "Ines", "Jamal", "Katya", "Liam",
    "Mei", "Noah", "Olga", "Priya", "Quentin", "Rosa", "Sven", "Tara", "Umar", "Vera", "Wen", "Ximena",
    "Yusuf", "Zoe", "Amara", "Bruno", "Carla", "Dev",
];
const LAST: &[&str] = &[
    "Almeida", "Bauer", "Chen", "Dubois", "Eriksen", "Fischer", "Garcia", "Haddad", "Ivanov", "Jensen",
    "Kim", "Lopez", "Meyer", "Nakamura", "Okafor", "Patel", "Quinn", "Rossi", "Silva", "Tanaka", "Usman",
    "Vargas", "Walker", "Xu", "Yilmaz", "Zhang", "Moreau", "Novak", "Larsen", "Costa",
];

/// (city, country, longitude, latitude)
const CITIES: &[(&str, &str, f64, f64)] = &[
    ("Amsterdam", "Netherlands", 4.9041, 52.3676),
    ("Berlin", "Germany", 13.4050, 52.5200),
    ("Lisbon", "Portugal", -9.1393, 38.7223),
    ("Paris", "France", 2.3522, 48.8566),
    ("Madrid", "Spain", -3.7038, 40.4168),
    ("Rome", "Italy", 12.4964, 41.9028),
    ("Vienna", "Austria", 16.3738, 48.2082),
    ("Stockholm", "Sweden", 18.0686, 59.3293),
    ("London", "United Kingdom", -0.1276, 51.5072),
    ("Dublin", "Ireland", -6.2603, 53.3498),
    ("New York", "United States", -74.0060, 40.7128),
    ("Chicago", "United States", -87.6298, 41.8781),
    ("San Francisco", "United States", -122.4194, 37.7749),
    ("Toronto", "Canada", -79.3832, 43.6532),
    ("Mexico City", "Mexico", -99.1332, 19.4326),
    ("Sao Paulo", "Brazil", -46.6333, -23.5505),
    ("Buenos Aires", "Argentina", -58.3816, -34.6037),
    ("Cape Town", "South Africa", 18.4241, -33.9249),
    ("Nairobi", "Kenya", 36.8219, -1.2921),
    ("Dubai", "United Arab Emirates", 55.2708, 25.2048),
    ("Mumbai", "India", 72.8777, 19.0760),
    ("Singapore", "Singapore", 103.8198, 1.3521),
    ("Tokyo", "Japan", 139.6917, 35.6895),
    ("Seoul", "South Korea", 126.9780, 37.5665),
    ("Sydney", "Australia", 151.2093, -33.8688),
];

const CATEGORIES: &[(&str, &[&str])] = &[
    (
        "Coffee",
        &[
            "Ethiopia Guji",
            "Colombia Huila",
            "Kenya AA",
            "Brazil Santos",
            "Sumatra Mandheling",
            "Guatemala Antigua",
            "Costa Rica Tarrazu",
            "Yemen Mocha",
        ],
    ),
    (
        "Tea",
        &[
            "Darjeeling First Flush",
            "Sencha Kirishima",
            "Oolong Tieguanyin",
            "Earl Grey Reserve",
            "Assam Breakfast",
            "Matcha Ceremonial",
            "Jasmine Pearls",
            "Pu-erh Aged",
        ],
    ),
    (
        "Equipment",
        &[
            "Burr Grinder",
            "Gooseneck Kettle",
            "Pour-over Dripper",
            "Espresso Tamper",
            "Digital Scale",
            "French Press",
            "Aeropress Kit",
            "Milk Frother",
        ],
    ),
    (
        "Accessories",
        &[
            "Ceramic Mug",
            "Travel Tumbler",
            "Paper Filters",
            "Tea Infuser",
            "Canister Set",
            "Knock Box",
            "Cleaning Brush",
            "Linen Apron",
        ],
    ),
    (
        "Snacks",
        &[
            "Almond Biscotti",
            "Dark Chocolate Bar",
            "Oat Cookies",
            "Salted Caramels",
            "Dried Mango",
            "Pistachio Baklava",
            "Shortbread Tin",
            "Granola Clusters",
        ],
    ),
];
const ORIGINS: &[&str] =
    &["Ethiopia", "Colombia", "Kenya", "Brazil", "India", "Japan", "China", "Italy", "Portugal", "Germany"];
const TAGS: &[&str] = &["gift", "bestseller", "limited", "fair-trade", "new", "seasonal", "bulk"];
const STATUSES: &[(&str, u64)] = &[
    ("delivered", 55),
    ("shipped", 15),
    ("processing", 10),
    ("pending", 8),
    ("cancelled", 8),
    ("refunded", 4),
];
const NOTES: &[&str] = &[
    "Please leave at the door",
    "Gift wrap, no invoice",
    "Call on arrival",
    "Fragile: ceramics inside",
    "Deliver after 5pm",
    "It's a birthday present",
];

struct Customer {
    first: String,
    last: String,
    email: String,
    city: &'static str,
    country: &'static str,
    created_at: String,
    marketing: bool,
}
struct Product {
    sku: String,
    name: String,
    category: &'static str,
    price_cents: i64,
    cost_cents: i64,
    attributes: String,
    stock: i64,
}
struct Order {
    customer_id: i64,
    status: &'static str,
    ordered_at: String,
    shipped_at: Option<String>,
    total_cents: i64,
    note: Option<&'static str>,
}
struct Item {
    order_id: i64,
    line_no: i64,
    product_id: i64,
    quantity: i64,
    unit_cents: i64,
}
struct Store {
    name: String,
    city: &'static str,
    country: &'static str,
    opened_on: String,
    lon: f64,
    lat: f64,
}
struct Zone {
    store_id: i64,
    name: String,
    ring: Vec<(f64, f64)>,
}
struct Route {
    store_id: i64,
    name: String,
    path: Vec<(f64, f64)>,
    distance_km: f64,
}

struct Data {
    customers: Vec<Customer>,
    products: Vec<Product>,
    orders: Vec<Order>,
    items: Vec<Item>,
    stores: Vec<Store>,
    zones: Vec<Zone>,
    routes: Vec<Route>,
}

/// Days since 1970-01-01 to `YYYY-MM-DD`.
fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 2022-01-01 as days since the Unix epoch.
const EPOCH_2022: i64 = 18_993;

fn timestamp(rng: &mut Rng, day: i64) -> String {
    format!("{} {:02}:{:02}:{:02}", civil(day), rng.range(7, 22), rng.range(0, 59), rng.range(0, 59))
}

fn haversine_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (lat1, lat2) = (a.1.to_radians(), b.1.to_radians());
    let dlat = lat2 - lat1;
    let dlon = (b.0 - a.0).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * 6371.0 * h.sqrt().asin()
}

impl Data {
    fn generate() -> Data {
        let mut rng = Rng(0x5155_4152_5259); // "QUARRY"
        let customers = (0..200)
            .map(|i| {
                let first = rng.pick(FIRST).to_string();
                let last = rng.pick(LAST).to_string();
                let &(city, country, ..) = rng.pick(CITIES);
                Customer {
                    email: format!("{}.{}{}@example.com", first.to_lowercase(), last.to_lowercase(), i + 1),
                    first,
                    last,
                    city,
                    country,
                    created_at: {
                        let day = EPOCH_2022 + rng.range(0, 700);
                        timestamp(&mut rng, day)
                    },
                    marketing: rng.below(100) < 70,
                }
            })
            .collect::<Vec<_>>();

        let mut products = Vec::new();
        for (ci, (category, names)) in CATEGORIES.iter().enumerate() {
            for (ni, name) in names.iter().enumerate() {
                let base = match *category {
                    "Equipment" => rng.range(1800, 24900),
                    "Accessories" => rng.range(600, 4500),
                    "Snacks" => rng.range(350, 1800),
                    _ => rng.range(850, 3200),
                };
                let tags: Vec<String> =
                    (0..rng.range(1, 3)).map(|_| format!("\"{}\"", rng.pick(TAGS))).collect::<Vec<_>>();
                products.push(Product {
                    sku: format!("{}-{:03}", category[..3].to_uppercase(), ci * 8 + ni + 1),
                    name: name.to_string(),
                    category,
                    price_cents: base,
                    cost_cents: base * rng.range(35, 65) / 100,
                    attributes: format!(
                        "{{\"weight_g\": {}, \"origin\": \"{}\", \"organic\": {}, \"tags\": [{}]}}",
                        rng.range(80, 2500),
                        rng.pick(ORIGINS),
                        rng.below(2) == 1,
                        tags.join(", ")
                    ),
                    stock: rng.range(0, 400),
                });
            }
        }

        let weight_total: u64 = STATUSES.iter().map(|s| s.1).sum();
        let mut orders = Vec::new();
        let mut items = Vec::new();
        for oid in 1..=600i64 {
            // Square the uniform draw so a few customers order much more often.
            let customer_id = 1 + (rng.unit().powi(2) * 200.0) as i64;
            let mut roll = rng.below(weight_total);
            let status = STATUSES
                .iter()
                .find(|(_, w)| {
                    if roll < *w {
                        true
                    } else {
                        roll -= *w;
                        false
                    }
                })
                .unwrap()
                .0;
            let day = EPOCH_2022 + 365 + rng.range(0, 700);
            let ordered_at = timestamp(&mut rng, day);
            let shipped_at = if matches!(status, "shipped" | "delivered" | "refunded") {
                let lag = rng.range(1, 4);
                Some(timestamp(&mut rng, day + lag))
            } else {
                None
            };
            let mut total = 0;
            let mut chosen: Vec<i64> = Vec::new();
            for line_no in 1..=rng.range(1, 5) {
                let pid = loop {
                    let p = rng.range(1, products.len() as i64);
                    if !chosen.contains(&p) {
                        break p;
                    }
                };
                chosen.push(pid);
                let quantity = rng.range(1, 4);
                let unit_cents = products[(pid - 1) as usize].price_cents;
                total += quantity * unit_cents;
                items.push(Item { order_id: oid, line_no, product_id: pid, quantity, unit_cents });
            }
            orders.push(Order {
                customer_id,
                status,
                ordered_at,
                shipped_at,
                total_cents: total,
                note: (rng.below(4) == 0).then(|| *rng.pick(NOTES)),
            });
        }

        let mut stores = Vec::new();
        let mut zones = Vec::new();
        let mut routes = Vec::new();
        for (i, &(city, country, lon, lat)) in CITIES.iter().enumerate() {
            let id = i as i64 + 1;
            stores.push(Store {
                name: format!("Quarry Coffee {city}"),
                city,
                country,
                opened_on: civil(EPOCH_2022 - 900 + rng.range(0, 1200)),
                lon,
                lat,
            });
            let radius = 0.03 + rng.unit() * 0.09;
            let sides = 12;
            let mut ring: Vec<(f64, f64)> = (0..sides)
                .map(|k| {
                    let a = std::f64::consts::TAU * k as f64 / sides as f64;
                    // Jitter the radius so zones are not perfect circles.
                    let r = radius * (0.75 + 0.5 * rng.unit());
                    (lon + r * a.cos() / lat.to_radians().cos(), lat + r * a.sin())
                })
                .collect();
            ring.push(ring[0]);
            zones.push(Zone { store_id: id, name: format!("{city} central zone"), ring });
            let mut path = vec![(lon, lat)];
            for _ in 0..4 {
                let last = *path.last().unwrap();
                path.push((last.0 + (rng.unit() - 0.4) * 0.05, last.1 + (rng.unit() - 0.4) * 0.04));
            }
            let distance_km = path.windows(2).map(|w| haversine_km(w[0], w[1])).sum::<f64>();
            routes.push(Route { store_id: id, name: format!("{city} morning loop"), path, distance_km });
        }
        Data { customers, products, orders, items, stores, zones, routes }
    }
}

// ---------------------------------------------------------------------------
// SQL helpers
// ---------------------------------------------------------------------------

enum Dialect {
    Postgres { schema: String, postgis: bool },
    MySql { database: String, mariadb: bool },
    Sqlite,
}

impl Dialect {
    async fn detect(conn: &dyn Connection) -> Result<Dialect> {
        match conn.kind() {
            DatabaseKind::Sqlite => Ok(Dialect::Sqlite),
            DatabaseKind::Postgres => {
                let schema = scalar(conn, "SELECT current_schema()").await?.ok_or_else(|| {
                    Error::other("the connection has no current schema (empty search_path)")
                })?;
                let mut postgis =
                    scalar(conn, "SELECT 1 FROM pg_extension WHERE extname = 'postgis'").await?.is_some();
                if !postgis {
                    // Needs a privileged role; without it the spatial tables are skipped.
                    if conn.execute("CREATE EXTENSION IF NOT EXISTS postgis", &opts()).await.is_ok() {
                        postgis = scalar(conn, "SELECT 1 FROM pg_extension WHERE extname = 'postgis'")
                            .await?
                            .is_some();
                    }
                }
                Ok(Dialect::Postgres { schema, postgis })
            }
            DatabaseKind::MySql => {
                let database = scalar(conn, "SELECT DATABASE()").await?.ok_or_else(|| {
                    Error::other("no database selected: connect with a database name (or run USE)")
                })?;
                let mariadb = conn.server_info().await?.version.starts_with("MariaDB");
                Ok(Dialect::MySql { database, mariadb })
            }
        }
    }

    fn q(&self, ident: &str) -> String {
        match self {
            Dialect::MySql { .. } => DatabaseKind::MySql.quote_ident(ident),
            _ => DatabaseKind::Postgres.quote_ident(ident),
        }
    }

    /// Fully qualified, quoted table name.
    fn t(&self, name: &str) -> String {
        match self {
            Dialect::Postgres { schema, .. } => format!("{}.{}", self.q(schema), self.q(name)),
            Dialect::MySql { database, .. } => format!("{}.{}", self.q(database), self.q(name)),
            Dialect::Sqlite => self.q(name),
        }
    }

    fn lit(&self, s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('\'');
        for c in s.chars() {
            match c {
                '\'' => out.push_str("''"),
                // MySQL treats backslash as an escape character inside strings.
                '\\' if matches!(self, Dialect::MySql { .. }) => out.push_str("\\\\"),
                c => out.push(c),
            }
        }
        out.push('\'');
        out
    }

    fn json(&self, s: &str) -> String {
        match self {
            Dialect::Postgres { .. } => format!("{}::jsonb", self.lit(s)),
            _ => self.lit(s),
        }
    }

    fn has_spatial(&self) -> bool {
        match self {
            Dialect::Postgres { postgis, .. } => *postgis,
            _ => true,
        }
    }
}

fn opts() -> ExecOptions {
    ExecOptions { row_limit: None }
}

async fn scalar(conn: &dyn Connection, sql: &str) -> Result<Option<String>> {
    let out = conn.execute(sql, &ExecOptions { row_limit: Some(1) }).await?;
    Ok(out
        .result
        .and_then(|r| r.rows.into_iter().next())
        .and_then(|mut r| (!r.is_empty()).then(|| r.remove(0)))
        .and_then(|v| match v {
            Value::Null => None,
            other => Some(other.display()),
        }))
}

struct Runner<'a> {
    conn: &'a dyn Connection,
}

impl Runner<'_> {
    async fn run(&mut self, sql: impl AsRef<str>) -> Result<()> {
        self.conn.execute(sql.as_ref(), &opts()).await.map(|_| ())
    }

    /// Multi-row insert in chunks of 100 rows.
    async fn insert(&mut self, head: &str, rows: Vec<String>) -> Result<()> {
        for chunk in rows.chunks(100) {
            self.run(format!("{head} VALUES\n{}", chunk.join(",\n"))).await?;
        }
        Ok(())
    }
}

fn money(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

fn wkt_point((lon, lat): (f64, f64)) -> String {
    format!("POINT({lon:.6} {lat:.6})")
}
fn wkt_ring(ring: &[(f64, f64)]) -> String {
    format!(
        "POLYGON(({}))",
        ring.iter().map(|(x, y)| format!("{x:.6} {y:.6}")).collect::<Vec<_>>().join(", ")
    )
}
fn wkt_line(path: &[(f64, f64)]) -> String {
    format!(
        "LINESTRING({})",
        path.iter().map(|(x, y)| format!("{x:.6} {y:.6}")).collect::<Vec<_>>().join(", ")
    )
}

// ---------------------------------------------------------------------------
// PostgreSQL + MySQL/MariaDB
// ---------------------------------------------------------------------------

async fn seed_server(run: &mut Runner<'_>, data: &Data, d: &Dialect) -> Result<()> {
    let pg = matches!(d, Dialect::Postgres { .. });
    let spatial = d.has_spatial();
    let (customers, products, orders, items) =
        (d.t("customers"), d.t("products"), d.t("orders"), d.t("order_items"));
    let (stores, zones, routes) = (d.t("stores"), d.t("delivery_zones"), d.t("routes"));
    let summary = d.t("customer_order_summary");

    // Tear down in dependency order.
    run.run(format!("DROP VIEW IF EXISTS {summary}")).await?;
    for t in [&routes, &zones, &stores, &items, &orders, &products, &customers] {
        run.run(format!("DROP TABLE IF EXISTS {t}")).await?;
    }
    if let Dialect::Postgres { schema, .. } = d {
        run.run(format!("DROP FUNCTION IF EXISTS {}.order_total(integer)", d.q(schema))).await?;
        run.run(format!("DROP TYPE IF EXISTS {}.order_status", d.q(schema))).await?;
        run.run(format!(
            "CREATE TYPE {}.order_status AS ENUM ('pending','processing','shipped','delivered','cancelled','refunded')",
            d.q(schema)
        ))
        .await?;
    }
    if let Dialect::MySql { database, .. } = d {
        run.run(format!("DROP FUNCTION IF EXISTS {}.order_total", d.q(database))).await?;
    }

    let (serial, ts, tstz, status_ty, json_ty) = if pg {
        let schema = if let Dialect::Postgres { schema, .. } = d { schema.clone() } else { unreachable!() };
        (
            "integer GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY".to_string(),
            "timestamp",
            "timestamptz",
            format!("{}.order_status", d.q(&schema)),
            "jsonb",
        )
    } else {
        (
            "INT AUTO_INCREMENT PRIMARY KEY".to_string(),
            "DATETIME",
            "DATETIME",
            "ENUM('pending','processing','shipped','delivered','cancelled','refunded')".to_string(),
            "JSON",
        )
    };
    let _ = ts;
    let engine = if pg { "" } else { " ENGINE=InnoDB DEFAULT CHARSET=utf8mb4" };
    // MySQL takes comments inline; PostgreSQL via COMMENT ON afterwards.
    let c = |text: &str| if pg { String::new() } else { format!(" COMMENT {}", d.lit(text)) };
    let tc = |text: &str| if pg { String::new() } else { format!(" COMMENT={}", d.lit(text)) };

    run.run(format!(
        "CREATE TABLE {customers} (
  id {serial}{},
  first_name varchar(60) NOT NULL{},
  last_name varchar(60) NOT NULL,
  email varchar(160) NOT NULL UNIQUE,
  city varchar(80),
  country varchar(80),
  created_at {tstz} NOT NULL,
  marketing_opt_in boolean NOT NULL DEFAULT {}
){engine}{}",
        c("Surrogate key"),
        c("Given name"),
        if pg { "true" } else { "1" },
        tc("People who have placed (or could place) orders"),
    ))
    .await?;
    run.run(format!(
        "CREATE TABLE {products} (
  id {serial},
  sku varchar(20) NOT NULL UNIQUE,
  name varchar(120) NOT NULL,
  category varchar(40) NOT NULL,
  price numeric(10,2) NOT NULL{},
  cost numeric(10,2) NOT NULL,
  attributes {json_ty},
  in_stock integer NOT NULL DEFAULT 0
){engine}{}",
        c("Shelf price in EUR"),
        tc("Sellable catalogue items"),
    ))
    .await?;
    run.run(format!(
        "CREATE TABLE {orders} (
  id {serial},
  customer_id integer NOT NULL REFERENCES {customers}(id),
  status {status_ty} NOT NULL DEFAULT 'pending',
  ordered_at {tstz} NOT NULL,
  shipped_at {tstz},
  total numeric(12,2) NOT NULL{},
  note text
){engine}{}",
        c("Sum of quantity * unit_price over the order's items"),
        tc("Customer orders"),
    ))
    .await?;
    run.run(format!(
        "CREATE TABLE {items} (
  order_id integer NOT NULL REFERENCES {orders}(id) ON DELETE CASCADE,
  line_no smallint NOT NULL,
  product_id integer NOT NULL REFERENCES {products}(id),
  quantity integer NOT NULL,
  unit_price numeric(10,2) NOT NULL,
  PRIMARY KEY (order_id, line_no)
){engine}{}",
        tc("Order lines; the primary key is composite"),
    ))
    .await?;
    run.run(format!("CREATE INDEX idx_orders_customer_id ON {orders} (customer_id)")).await?;
    run.run(format!("CREATE INDEX idx_orders_status_ordered_at ON {orders} (status, ordered_at)")).await?;
    run.run(format!("CREATE INDEX idx_products_category ON {products} (category)")).await?;
    run.run(format!("CREATE INDEX idx_order_items_product ON {items} (product_id)")).await?;

    if spatial {
        let (point, polygon, line) = if pg {
            ("geometry(Point,4326)", "geometry(Polygon,4326)", "geometry(LineString,4326)")
        } else {
            ("POINT NOT NULL SRID 4326", "POLYGON NOT NULL SRID 4326", "LINESTRING NOT NULL SRID 4326")
        };
        // MariaDB does not know the `SRID n` column attribute (MySQL 8 does).
        let mariadb = matches!(d, Dialect::MySql { mariadb: true, .. });
        let strip = |s: &str| if mariadb { s.replace(" SRID 4326", "") } else { s.to_string() };
        let (point, polygon, line) = (strip(point), strip(polygon), strip(line));
        run.run(format!(
            "CREATE TABLE {stores} (
  id {serial},
  name varchar(120) NOT NULL,
  city varchar(80) NOT NULL,
  country varchar(80) NOT NULL,
  opened_on date NOT NULL,
  location {point}{}
){engine}{}",
            c("WGS 84 position (lon/lat)"),
            tc("Shops, with a POINT location"),
        ))
        .await?;
        run.run(format!(
            "CREATE TABLE {zones} (
  id {serial},
  store_id integer NOT NULL REFERENCES {stores}(id),
  name varchar(120) NOT NULL,
  area {polygon}
){engine}{}",
            tc("Delivery area per store (POLYGON)"),
        ))
        .await?;
        run.run(format!(
            "CREATE TABLE {routes} (
  id {serial},
  store_id integer NOT NULL REFERENCES {stores}(id),
  name varchar(120) NOT NULL,
  distance_km numeric(8,2) NOT NULL,
  path {line}
){engine}{}",
            tc("Courier routes (LINESTRING)"),
        ))
        .await?;
        if pg {
            run.run(format!("CREATE INDEX idx_stores_location ON {stores} USING gist (location)")).await?;
            run.run(format!("CREATE INDEX idx_zones_area ON {zones} USING gist (area)")).await?;
        } else {
            run.run(format!("CREATE SPATIAL INDEX idx_stores_location ON {stores} (location)")).await?;
        }
    }

    // ---- data ----
    let geom = |wkt: String| -> String {
        match d {
            Dialect::Postgres { .. } => format!("ST_GeomFromText('{wkt}', 4326)"),
            // MySQL 8 follows the EPSG axis order (lat, lon) for SRID 4326 unless
            // told otherwise; MariaDB always reads x y as written.
            Dialect::MySql { mariadb: false, .. } => {
                format!("ST_GeomFromText('{wkt}', 4326, 'axis-order=long-lat')")
            }
            _ => format!("ST_GeomFromText('{wkt}', 4326)"),
        }
    };
    let bool_lit = |b: bool| if pg { b.to_string() } else { (b as i32).to_string() };
    run.insert(
        &format!("INSERT INTO {customers} (first_name, last_name, email, city, country, created_at, marketing_opt_in)"),
        data.customers
            .iter()
            .map(|c| {
                format!(
                    "({}, {}, {}, {}, {}, {}, {})",
                    d.lit(&c.first),
                    d.lit(&c.last),
                    d.lit(&c.email),
                    d.lit(c.city),
                    d.lit(c.country),
                    d.lit(&c.created_at),
                    bool_lit(c.marketing)
                )
            })
            .collect(),
    )
    .await?;
    run.insert(
        &format!("INSERT INTO {products} (sku, name, category, price, cost, attributes, in_stock)"),
        data.products
            .iter()
            .map(|p| {
                format!(
                    "({}, {}, {}, {}, {}, {}, {})",
                    d.lit(&p.sku),
                    d.lit(&p.name),
                    d.lit(p.category),
                    money(p.price_cents),
                    money(p.cost_cents),
                    d.json(&p.attributes),
                    p.stock
                )
            })
            .collect(),
    )
    .await?;
    run.insert(
        &format!("INSERT INTO {orders} (customer_id, status, ordered_at, shipped_at, total, note)"),
        data.orders
            .iter()
            .map(|o| {
                format!(
                    "({}, {}, {}, {}, {}, {})",
                    o.customer_id,
                    d.lit(o.status),
                    d.lit(&o.ordered_at),
                    o.shipped_at.as_deref().map(|s| d.lit(s)).unwrap_or_else(|| "NULL".into()),
                    money(o.total_cents),
                    o.note.map(|s| d.lit(s)).unwrap_or_else(|| "NULL".into()),
                )
            })
            .collect(),
    )
    .await?;
    run.insert(
        &format!("INSERT INTO {items} (order_id, line_no, product_id, quantity, unit_price)"),
        data.items
            .iter()
            .map(|i| {
                format!(
                    "({}, {}, {}, {}, {})",
                    i.order_id,
                    i.line_no,
                    i.product_id,
                    i.quantity,
                    money(i.unit_cents)
                )
            })
            .collect(),
    )
    .await?;
    if spatial {
        run.insert(
            &format!("INSERT INTO {stores} (name, city, country, opened_on, location)"),
            data.stores
                .iter()
                .map(|s| {
                    format!(
                        "({}, {}, {}, {}, {})",
                        d.lit(&s.name),
                        d.lit(s.city),
                        d.lit(s.country),
                        d.lit(&s.opened_on),
                        geom(wkt_point((s.lon, s.lat)))
                    )
                })
                .collect(),
        )
        .await?;
        run.insert(
            &format!("INSERT INTO {zones} (store_id, name, area)"),
            data.zones
                .iter()
                .map(|z| format!("({}, {}, {})", z.store_id, d.lit(&z.name), geom(wkt_ring(&z.ring))))
                .collect(),
        )
        .await?;
        run.insert(
            &format!("INSERT INTO {routes} (store_id, name, distance_km, path)"),
            data.routes
                .iter()
                .map(|r| {
                    format!(
                        "({}, {}, {:.2}, {})",
                        r.store_id,
                        d.lit(&r.name),
                        r.distance_km,
                        geom(wkt_line(&r.path))
                    )
                })
                .collect(),
        )
        .await?;
    }

    // ---- view, function, comments ----
    run.run(format!(
        "CREATE VIEW {summary} AS
SELECT c.id AS customer_id,
       {} AS customer_name,
       c.country,
       COUNT(o.id) AS order_count,
       COALESCE(SUM(o.total), 0) AS total_spent,
       MAX(o.ordered_at) AS last_order_at
FROM {customers} c
LEFT JOIN {orders} o ON o.customer_id = c.id AND o.status NOT IN ('cancelled', 'refunded')
GROUP BY c.id, c.first_name, c.last_name, c.country",
        if pg { "c.first_name || ' ' || c.last_name" } else { "CONCAT(c.first_name, ' ', c.last_name)" }
    ))
    .await?;
    match d {
        Dialect::Postgres { schema, .. } => {
            run.run(format!(
                "CREATE FUNCTION {s}.order_total(p_order_id integer) RETURNS numeric
LANGUAGE sql STABLE AS $$ SELECT COALESCE(SUM(quantity * unit_price), 0) FROM {items} WHERE order_id = p_order_id $$",
                s = d.q(schema)
            ))
            .await?;
            let comments = [
                format!("COMMENT ON TABLE {customers} IS 'People who have placed (or could place) orders'"),
                format!("COMMENT ON COLUMN {customers}.id IS 'Surrogate key'"),
                format!("COMMENT ON COLUMN {customers}.first_name IS 'Given name'"),
                format!("COMMENT ON TABLE {products} IS 'Sellable catalogue items'"),
                format!("COMMENT ON COLUMN {products}.price IS 'Shelf price in EUR'"),
                format!("COMMENT ON TABLE {orders} IS 'Customer orders'"),
                format!(
                    "COMMENT ON COLUMN {orders}.total IS 'Sum of quantity * unit_price over the order''s items'"
                ),
                format!("COMMENT ON TABLE {items} IS 'Order lines; the primary key is composite'"),
                format!(
                    "COMMENT ON VIEW {summary} IS 'Order count and spend per customer, ignoring cancelled and refunded orders'"
                ),
                format!(
                    "COMMENT ON FUNCTION {}.order_total(integer) IS 'Recomputes an order total from its items'",
                    d.q(schema)
                ),
            ];
            for sql in comments {
                run.run(sql).await?;
            }
            if spatial {
                for sql in [
                    format!("COMMENT ON TABLE {stores} IS 'Shops, with a POINT location'"),
                    format!("COMMENT ON COLUMN {stores}.location IS 'WGS 84 position (lon/lat)'"),
                    format!("COMMENT ON TABLE {zones} IS 'Delivery area per store (POLYGON)'"),
                    format!("COMMENT ON TABLE {routes} IS 'Courier routes (LINESTRING)'"),
                ] {
                    run.run(sql).await?;
                }
            }
            // Fresh statistics make row estimates in the explorer meaningful.
            for t in [&customers, &products, &orders, &items] {
                run.run(format!("ANALYZE {t}")).await?;
            }
        }
        Dialect::MySql { database, .. } => {
            // Creating a stored function can be refused (binary logging without
            // log_bin_trust_function_creators); the dataset is complete without it.
            let _ = run
                .run(format!(
                    "CREATE FUNCTION {}.order_total(p_order_id INT) RETURNS DECIMAL(12,2) READS SQL DATA \
                     RETURN (SELECT COALESCE(SUM(quantity * unit_price), 0) FROM {items} WHERE order_id = p_order_id)",
                    d.q(database)
                ))
                .await;
            for t in [&customers, &products, &orders, &items] {
                run.run(format!("ANALYZE TABLE {t}")).await?;
            }
        }
        Dialect::Sqlite => unreachable!(),
    }
    let _ = (tstz, &stores);
    Ok(())
}

// ---------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------

async fn seed_sqlite(run: &mut Runner<'_>, data: &Data) -> Result<()> {
    let d = Dialect::Sqlite;
    run.run("DROP VIEW IF EXISTS customer_order_summary").await?;
    for t in ["routes", "delivery_zones", "stores", "order_items", "orders", "products", "customers"] {
        run.run(format!("DROP TABLE IF EXISTS {t}")).await?;
    }
    let ddl = [
        "CREATE TABLE customers (
  id INTEGER PRIMARY KEY,
  first_name VARCHAR(60) NOT NULL,
  last_name VARCHAR(60) NOT NULL,
  email VARCHAR(160) NOT NULL UNIQUE,
  city VARCHAR(80),
  country VARCHAR(80),
  created_at TIMESTAMP NOT NULL,
  marketing_opt_in BOOLEAN NOT NULL DEFAULT 1
)",
        "CREATE TABLE products (
  id INTEGER PRIMARY KEY,
  sku VARCHAR(20) NOT NULL UNIQUE,
  name VARCHAR(120) NOT NULL,
  category VARCHAR(40) NOT NULL,
  price NUMERIC NOT NULL,
  cost NUMERIC NOT NULL,
  attributes JSON,
  in_stock INTEGER NOT NULL DEFAULT 0
)",
        "CREATE TABLE orders (
  id INTEGER PRIMARY KEY,
  customer_id INTEGER NOT NULL REFERENCES customers(id),
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','processing','shipped','delivered','cancelled','refunded')),
  ordered_at TIMESTAMP NOT NULL,
  shipped_at TIMESTAMP,
  total NUMERIC NOT NULL,
  note TEXT
)",
        "CREATE TABLE order_items (
  order_id INTEGER NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
  line_no INTEGER NOT NULL,
  product_id INTEGER NOT NULL REFERENCES products(id),
  quantity INTEGER NOT NULL,
  unit_price NUMERIC NOT NULL,
  PRIMARY KEY (order_id, line_no)
)",
        "CREATE TABLE stores (
  id INTEGER PRIMARY KEY,
  name VARCHAR(120) NOT NULL,
  city VARCHAR(80) NOT NULL,
  country VARCHAR(80) NOT NULL,
  opened_on DATE NOT NULL,
  longitude REAL NOT NULL,
  latitude REAL NOT NULL,
  location_wkt TEXT NOT NULL
)",
        "CREATE TABLE delivery_zones (
  id INTEGER PRIMARY KEY,
  store_id INTEGER NOT NULL REFERENCES stores(id),
  name VARCHAR(120) NOT NULL,
  area_wkt TEXT NOT NULL
)",
        "CREATE TABLE routes (
  id INTEGER PRIMARY KEY,
  store_id INTEGER NOT NULL REFERENCES stores(id),
  name VARCHAR(120) NOT NULL,
  distance_km NUMERIC NOT NULL,
  path_wkt TEXT NOT NULL
)",
        "CREATE INDEX idx_orders_customer_id ON orders (customer_id)",
        "CREATE INDEX idx_orders_status_ordered_at ON orders (status, ordered_at)",
        "CREATE INDEX idx_products_category ON products (category)",
        "CREATE INDEX idx_order_items_product ON order_items (product_id)",
    ];
    for sql in ddl {
        run.run(sql).await?;
    }
    run.insert(
        "INSERT INTO customers (first_name, last_name, email, city, country, created_at, marketing_opt_in)",
        data.customers
            .iter()
            .map(|c| {
                format!(
                    "({}, {}, {}, {}, {}, {}, {})",
                    d.lit(&c.first),
                    d.lit(&c.last),
                    d.lit(&c.email),
                    d.lit(c.city),
                    d.lit(c.country),
                    d.lit(&c.created_at),
                    c.marketing as i32
                )
            })
            .collect(),
    )
    .await?;
    run.insert(
        "INSERT INTO products (sku, name, category, price, cost, attributes, in_stock)",
        data.products
            .iter()
            .map(|p| {
                format!(
                    "({}, {}, {}, {}, {}, {}, {})",
                    d.lit(&p.sku),
                    d.lit(&p.name),
                    d.lit(p.category),
                    money(p.price_cents),
                    money(p.cost_cents),
                    d.lit(&p.attributes),
                    p.stock
                )
            })
            .collect(),
    )
    .await?;
    run.insert(
        "INSERT INTO orders (customer_id, status, ordered_at, shipped_at, total, note)",
        data.orders
            .iter()
            .map(|o| {
                format!(
                    "({}, {}, {}, {}, {}, {})",
                    o.customer_id,
                    d.lit(o.status),
                    d.lit(&o.ordered_at),
                    o.shipped_at.as_deref().map(|s| d.lit(s)).unwrap_or_else(|| "NULL".into()),
                    money(o.total_cents),
                    o.note.map(|s| d.lit(s)).unwrap_or_else(|| "NULL".into()),
                )
            })
            .collect(),
    )
    .await?;
    run.insert(
        "INSERT INTO order_items (order_id, line_no, product_id, quantity, unit_price)",
        data.items
            .iter()
            .map(|i| {
                format!(
                    "({}, {}, {}, {}, {})",
                    i.order_id,
                    i.line_no,
                    i.product_id,
                    i.quantity,
                    money(i.unit_cents)
                )
            })
            .collect(),
    )
    .await?;
    run.insert(
        "INSERT INTO stores (name, city, country, opened_on, longitude, latitude, location_wkt)",
        data.stores
            .iter()
            .map(|s| {
                format!(
                    "({}, {}, {}, {}, {:.6}, {:.6}, {})",
                    d.lit(&s.name),
                    d.lit(s.city),
                    d.lit(s.country),
                    d.lit(&s.opened_on),
                    s.lon,
                    s.lat,
                    d.lit(&wkt_point((s.lon, s.lat)))
                )
            })
            .collect(),
    )
    .await?;
    run.insert(
        "INSERT INTO delivery_zones (store_id, name, area_wkt)",
        data.zones
            .iter()
            .map(|z| format!("({}, {}, {})", z.store_id, d.lit(&z.name), d.lit(&wkt_ring(&z.ring))))
            .collect(),
    )
    .await?;
    run.insert(
        "INSERT INTO routes (store_id, name, distance_km, path_wkt)",
        data.routes
            .iter()
            .map(|r| {
                format!(
                    "({}, {}, {:.2}, {})",
                    r.store_id,
                    d.lit(&r.name),
                    r.distance_km,
                    d.lit(&wkt_line(&r.path))
                )
            })
            .collect(),
    )
    .await?;
    run.run(
        "CREATE VIEW customer_order_summary AS
SELECT c.id AS customer_id,
       c.first_name || ' ' || c.last_name AS customer_name,
       c.country,
       COUNT(o.id) AS order_count,
       COALESCE(SUM(o.total), 0) AS total_spent,
       MAX(o.ordered_at) AS last_order_at
FROM customers c
LEFT JOIN orders o ON o.customer_id = c.id AND o.status NOT IN ('cancelled', 'refunded')
GROUP BY c.id, c.first_name, c.last_name, c.country",
    )
    .await?;
    run.run("ANALYZE").await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_is_deterministic_and_consistent() {
        let a = Data::generate();
        let b = Data::generate();
        assert_eq!(a.customers.len(), 200);
        assert_eq!(a.products.len(), 40);
        assert_eq!(a.orders.len(), 600);
        assert_eq!(a.stores.len(), 25);
        assert_eq!(a.orders[17].ordered_at, b.orders[17].ordered_at);
        assert_eq!(a.customers[5].email, b.customers[5].email);
        // Every order total equals the sum of its items.
        for (i, o) in a.orders.iter().enumerate() {
            let sum: i64 = a
                .items
                .iter()
                .filter(|it| it.order_id == i as i64 + 1)
                .map(|it| it.quantity * it.unit_cents)
                .sum();
            assert_eq!(sum, o.total_cents);
        }
        // Emails are unique.
        let mut emails: Vec<_> = a.customers.iter().map(|c| c.email.clone()).collect();
        emails.sort();
        emails.dedup();
        assert_eq!(emails.len(), 200);
        // Rings are closed.
        assert!(a.zones.iter().all(|z| z.ring.first() == z.ring.last()));
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), "1970-01-01");
        assert_eq!(civil(EPOCH_2022), "2022-01-01");
    }
}
