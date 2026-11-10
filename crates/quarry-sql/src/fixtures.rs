//! Realistic catalogs shared by unit tests.

use quarry_core::*;

pub fn col(name: &str, ty: &str, kind: TypeKind) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        data_type: ty.into(),
        kind,
        nullable: true,
        default: None,
        comment: None,
        geometry: None,
    }
}

pub fn int(name: &str) -> ColumnInfo {
    col(name, "integer", TypeKind::Integer)
}

pub fn text(name: &str) -> ColumnInfo {
    col(name, "text", TypeKind::Text)
}

pub fn fk(name: &str, cols: &[&str], schema: Option<&str>, table: &str, refs: &[&str]) -> ForeignKey {
    ForeignKey {
        name: name.into(),
        columns: cols.iter().map(|s| s.to_string()).collect(),
        referenced_schema: schema.map(|s| s.to_string()),
        referenced_table: table.into(),
        referenced_columns: refs.iter().map(|s| s.to_string()).collect(),
        on_delete: None,
        on_update: None,
    }
}

pub fn index(name: &str, cols: &[&str], unique: bool, primary: bool) -> Index {
    Index {
        name: name.into(),
        columns: cols.iter().map(|s| s.to_string()).collect(),
        unique,
        primary,
        method: Some("btree".into()),
        definition: None,
    }
}

pub fn table(name: &str, columns: Vec<ColumnInfo>, pk: &[&str], fks: Vec<ForeignKey>, rows: Option<u64>) -> Table {
    let mut indexes = Vec::new();
    if !pk.is_empty() {
        indexes.push(index(&format!("{name}_pkey"), pk, true, true));
    }
    Table {
        name: name.into(),
        kind: TableKind::Table,
        columns,
        primary_key: pk.iter().map(|s| s.to_string()).collect(),
        foreign_keys: fks,
        indexes,
        row_estimate: rows,
        comment: None,
    }
}

/// customers / orders / order_items / products / stores in `public` (default) and
/// `analytics` as a second schema. `reviews` and `analytics.events` deliberately
/// lack declared foreign keys so inference has something to find.
pub fn shop() -> Catalog {
    let customers = table(
        "customers",
        vec![
            col("id", "integer", TypeKind::Integer),
            col("email", "text", TypeKind::Text),
            col("name", "text", TypeKind::Text),
            col("tier", "text", TypeKind::Text),
            col("created_at", "timestamp with time zone", TypeKind::Timestamp),
        ],
        &["id"],
        vec![],
        Some(12_400),
    );
    let mut orders = table(
        "orders",
        vec![
            int("id"),
            int("customer_id"),
            int("store_id"),
            text("status"),
            col("total", "numeric(10,2)", TypeKind::Decimal),
            col("placed_at", "timestamp with time zone", TypeKind::Timestamp),
        ],
        &["id"],
        vec![
            fk("orders_customer_id_fkey", &["customer_id"], None, "customers", &["id"]),
            fk("orders_store_id_fkey", &["store_id"], None, "stores", &["id"]),
        ],
        Some(1_250_000),
    );
    orders.indexes.push(index("orders_customer_id_idx", &["customer_id"], false, false));
    let order_items = table(
        "order_items",
        vec![
            int("order_id"),
            int("product_id"),
            int("quantity"),
            col("unit_price", "numeric(10,2)", TypeKind::Decimal),
        ],
        &["order_id", "product_id"],
        vec![
            fk("order_items_order_id_fkey", &["order_id"], None, "orders", &["id"]),
            fk("order_items_product_id_fkey", &["product_id"], None, "products", &["id"]),
        ],
        Some(3_400_000),
    );
    let products = table(
        "products",
        vec![int("id"), text("sku"), text("name"), col("price", "numeric(10,2)", TypeKind::Decimal), int("category_id")],
        &["id"],
        vec![],
        Some(850),
    );
    let mut location = col("location", "geometry(Point,4326)", TypeKind::Geometry);
    location.geometry = Some(GeometryColumn { geometry_type: "POINT".into(), srid: Some(4326), geography: false });
    let stores = table("stores", vec![int("id"), text("name"), location], &["id"], vec![], Some(42));
    let reviews = table(
        "reviews",
        vec![int("id"), int("product_id"), int("customer_id"), int("rating"), text("body")],
        &["id"],
        vec![],
        Some(90_000),
    );
    let categories = table("categories", vec![int("id"), text("name"), int("parent_id")], &["id"], vec![], Some(30));
    let mut active = table(
        "active_customers",
        vec![int("id"), text("email")],
        &[],
        vec![],
        None,
    );
    active.kind = TableKind::View;

    let events = table(
        "events",
        vec![int("id"), int("customer_id"), col("occurred_at", "timestamp with time zone", TypeKind::Timestamp), text("kind")],
        &["id"],
        vec![],
        Some(8_000_000),
    );
    let daily = table(
        "daily_sales",
        vec![col("day", "date", TypeKind::Date), col("revenue", "numeric", TypeKind::Decimal)],
        &["day"],
        vec![],
        Some(900),
    );

    Catalog {
        schemas: vec![
            Schema {
                name: "public".into(),
                tables: vec![customers, orders, order_items, products, stores, reviews, categories, active],
                routines: vec![Routine {
                    name: "order_total".into(),
                    kind: RoutineKind::Function,
                    signature: "(order_id integer)".into(),
                    return_type: Some("numeric".into()),
                }],
                sequences: vec!["orders_id_seq".into()],
            },
            Schema { name: "analytics".into(), tables: vec![events, daily], routines: vec![], sequences: vec![] },
        ],
        default_schema: Some("public".into()),
    }
}

/// `n` synthetic tables of ten columns each, for the performance test.
pub fn big(n: usize) -> Catalog {
    let tables = (0..n)
        .map(|i| {
            let mut cols = vec![int("id")];
            for c in 0..9 {
                cols.push(text(&format!("col_{c}_of_{i}")));
            }
            let mut t = table(&format!("table_{i:04}"), cols, &["id"], vec![], Some(1000 + i as u64));
            if i > 0 {
                t.columns.push(int("parent_id"));
                t.foreign_keys.push(fk(&format!("fk_{i}"), &["parent_id"], None, &format!("table_{:04}", i - 1), &["id"]));
            }
            t
        })
        .collect();
    Catalog { schemas: vec![Schema { name: "public".into(), tables, routines: vec![], sequences: vec![] }], default_schema: Some("public".into()) }
}
