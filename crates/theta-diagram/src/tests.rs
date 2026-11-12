use crate::layout::{Layout, Metrics, layout};
use crate::model::{Graph, GraphColumn, GraphEdge, GraphNode, Scope, from_catalog};
use crate::{mermaid, svg};
use theta_core::{Catalog, ColumnInfo, ForeignKey, Schema, Table, TypeKind, UniqueConstraint};

fn col(name: &str) -> GraphColumn {
    GraphColumn {
        name: name.into(),
        data_type: "integer".into(),
        pk: name == "id",
        fk: false,
        referenced: false,
        nullable: false,
    }
}

/// Tables `(name, columns)` and foreign keys `(owner, owner column, referenced, referenced column)`.
fn graph(tables: &[(&str, &[&str])], fks: &[(&str, &str, &str, &str)]) -> Graph {
    let mut g = Graph::default();
    for (n, cols) in tables {
        g.nodes.push(GraphNode {
            schema: "public".into(),
            name: n.to_string(),
            columns: cols.iter().map(|c| col(c)).collect(),
        });
    }
    let idx = |n: &str| g.nodes.iter().position(|x| x.name == n).expect("table");
    let mut edges = Vec::new();
    for (f, fc, t, tc) in fks {
        edges.push(GraphEdge {
            name: format!("{f}_{fc}_fk"),
            from: idx(f),
            to: idx(t),
            from_cols: vec![fc.to_string()],
            to_cols: vec![tc.to_string()],
            optional: false,
            unique: false,
        });
    }
    for e in &edges {
        let c = g.nodes[e.from].columns.iter_mut().find(|c| c.name == e.from_cols[0]).expect("column");
        c.fk = true;
        let r = g.nodes[e.to].columns.iter_mut().find(|c| c.name == e.to_cols[0]).expect("column");
        r.referenced = true;
    }
    g.edges = edges;
    g
}

type Seg = ((f32, f32), (f32, f32));

fn segments(pts: &[(f32, f32)]) -> Vec<Seg> {
    pts.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Whether an axis-aligned segment passes through the inside of a rectangle (touching the border is fine:
/// edges start and end on it).
fn crosses_interior(s: Seg, x: f32, y: f32, w: f32, h: f32) -> bool {
    let (x0, x1) = (s.0.0.min(s.1.0), s.0.0.max(s.1.0));
    let (y0, y1) = (s.0.1.min(s.1.1), s.0.1.max(s.1.1));
    let e = 0.5;
    x1 > x + e && x0 < x + w - e && y1 > y + e && y0 < y + h - e
}

/// The geometric promises of the layout, checked on any graph.
fn check(g: &Graph, l: &Layout, m: &Metrics) {
    assert_eq!(l.boxes.len(), g.nodes.len());
    assert_eq!(l.edges.len(), g.edges.len());
    for (i, a) in l.boxes.iter().enumerate() {
        assert!(
            a.x >= 0.0 && a.y >= 0.0 && a.right() <= l.width && a.bottom() <= l.height,
            "box {i} outside the picture"
        );
        for b in &l.boxes[i + 1..] {
            let apart = a.right() <= b.x || b.right() <= a.x || a.bottom() <= b.y || b.bottom() <= a.y;
            assert!(apart, "boxes {} and {} overlap", a.node, b.node);
        }
    }
    for (ei, e) in l.edges.iter().enumerate() {
        assert_eq!(e.edge, ei);
        assert!(e.points.len() >= 2, "edge {ei} has no line");
        for w in e.points.windows(2) {
            assert!(w[0].0 == w[1].0 || w[0].1 == w[1].1, "edge {ei} has a diagonal segment {w:?}");
        }
        for s in segments(&e.points) {
            for b in &l.boxes {
                assert!(!crosses_interior(s, b.x, b.y, b.w, b.h), "edge {ei} runs through table {}", b.node);
            }
        }
        // The ends sit on the boxes of the edge's own tables, at the row of the first key column.
        let ge = &g.edges[ei];
        let (owner_pt, ref_pt) = if e.owner_at_start {
            (e.points[0], *e.points.last().unwrap())
        } else {
            (*e.points.last().unwrap(), e.points[0])
        };
        for (pt, node, cols) in [(owner_pt, ge.from, &ge.from_cols), (ref_pt, ge.to, &ge.to_cols)] {
            let b = &l.boxes[node];
            assert!(
                (pt.0 - b.x).abs() < 0.01 || (pt.0 - b.right()).abs() < 0.01,
                "edge {ei} end is not on a side of its table"
            );
            let rows: Vec<f32> = cols
                .iter()
                .filter_map(|c| {
                    b.rows
                        .iter()
                        .find(|r| g.nodes[node].columns[r.column].name == *c)
                        .map(|r| r.y + m.row_h / 2.0)
                })
                .collect();
            let (lo, hi) = (
                rows.iter().cloned().fold(f32::MAX, f32::min),
                rows.iter().cloned().fold(f32::MIN, f32::max),
            );
            assert!(
                (pt.1 - (lo + hi) / 2.0).abs() < 0.01,
                "edge {ei} ends at y={} but its columns are at {lo}..{hi}",
                pt.1
            );
        }
    }
}

fn crossings(l: &Layout) -> usize {
    // Proper crossings between segments of different edges.
    let mut n = 0;
    for (i, a) in l.edges.iter().enumerate() {
        for b in &l.edges[i + 1..] {
            for s in segments(&a.points) {
                for t in segments(&b.points) {
                    let (sv, tv) = (s.0.0 == s.1.0, t.0.0 == t.1.0);
                    if sv == tv {
                        continue;
                    }
                    let (v, h) = if sv { (s, t) } else { (t, s) };
                    let x = v.0.0;
                    let y = h.0.1;
                    let (hx0, hx1) = (h.0.0.min(h.1.0), h.0.0.max(h.1.0));
                    let (vy0, vy1) = (v.0.1.min(v.1.1), v.0.1.max(v.1.1));
                    if x > hx0 + 0.01 && x < hx1 - 0.01 && y > vy0 + 0.01 && y < vy1 - 0.01 {
                        n += 1;
                    }
                }
            }
        }
    }
    n
}

#[test]
fn a_chain_goes_left_to_right_from_the_referenced_table() {
    let g = graph(
        &[("customers", &["id"]), ("orders", &["id", "customer_id"]), ("items", &["id", "order_id"])],
        &[("orders", "customer_id", "customers", "id"), ("items", "order_id", "orders", "id")],
    );
    let m = Metrics::default();
    let l = layout(&g, &m);
    check(&g, &l, &m);
    let (c, o, i) = (&l.boxes[0], &l.boxes[1], &l.boxes[2]);
    assert!(c.right() < o.x && o.right() < i.x, "referenced tables are to the left of their referrers");
    assert_eq!(crossings(&l), 0);
}

#[test]
fn edges_end_at_the_right_columns_and_are_straight_when_they_can_be() {
    let g = graph(&[("a", &["id", "x"]), ("b", &["id", "y", "a_id"])], &[("b", "a_id", "a", "id")]);
    let m = Metrics::default();
    let l = layout(&g, &m);
    check(&g, &l, &m);
    let e = &l.edges[0];
    // Two ends, aligned: one horizontal line.
    assert_eq!(e.points.len(), 2, "{:?}", e.points);
    assert_eq!(e.points[0].1, e.points[1].1);
}

#[test]
fn self_reference_loops_on_the_right_of_its_own_table() {
    let g = graph(&[("emp", &["id", "boss_id", "name"])], &[("emp", "boss_id", "emp", "id")]);
    let m = Metrics::default();
    let l = layout(&g, &m);
    check(&g, &l, &m);
    let b = &l.boxes[0];
    let e = &l.edges[0];
    assert_eq!(e.points.len(), 4);
    assert!(e.points.iter().skip(1).take(2).all(|p| p.0 > b.right()), "the loop leaves the box");
    assert!(l.width >= e.points[1].0, "the picture is wide enough for the loop");
}

#[test]
fn several_self_references_get_separate_loops() {
    let g = graph(&[("t", &["id", "a", "b"])], &[("t", "a", "t", "id"), ("t", "b", "t", "id")]);
    let m = Metrics::default();
    let l = layout(&g, &m);
    check(&g, &l, &m);
    assert_ne!(l.edges[0].points[1].0, l.edges[1].points[1].0);
}

#[test]
fn cycles_are_laid_out_and_every_edge_is_drawn() {
    let g = graph(
        &[("a", &["id", "b_id"]), ("b", &["id", "c_id"]), ("c", &["id", "a_id"])],
        &[("a", "b_id", "b", "id"), ("b", "c_id", "c", "id"), ("c", "a_id", "a", "id")],
    );
    let m = Metrics::default();
    let l = layout(&g, &m);
    check(&g, &l, &m);
    assert_eq!(l.edges.iter().filter(|e| e.points.len() >= 2).count(), 3);
}

#[test]
fn mutual_references_between_two_tables() {
    let g = graph(
        &[("a", &["id", "b_id"]), ("b", &["id", "a_id"])],
        &[("a", "b_id", "b", "id"), ("b", "a_id", "a", "id")],
    );
    let m = Metrics::default();
    check(&g, &layout(&g, &m), &m);
}

#[test]
fn composite_foreign_keys_end_in_the_middle_of_a_bracket() {
    let mut g = graph(&[("parent", &["a", "b", "z"]), ("child", &["id", "pa", "pb"])], &[]);
    for n in &mut g.nodes[0].columns {
        n.referenced = n.name != "z";
    }
    for n in &mut g.nodes[1].columns {
        n.fk = n.name.starts_with('p');
    }
    g.edges.push(GraphEdge {
        name: "child_fk".into(),
        from: 1,
        to: 0,
        from_cols: vec!["pa".into(), "pb".into()],
        to_cols: vec!["a".into(), "b".into()],
        optional: false,
        unique: false,
    });
    let m = Metrics::default();
    let l = layout(&g, &m);
    check(&g, &l, &m);
    let e = &l.edges[0];
    let (sb, eb) = (e.start_bracket.as_ref(), e.end_bracket.as_ref());
    assert_eq!(sb.map(|b| b.ticks.len()), Some(2));
    assert_eq!(eb.map(|b| b.ticks.len()), Some(2));
}

#[test]
fn tables_without_foreign_keys_are_packed_in_a_grid() {
    let names: Vec<String> = (0..30).map(|i| format!("t{i:02}")).collect();
    let tables: Vec<(&str, &[&str])> = names.iter().map(|n| (n.as_str(), &["id", "name"][..])).collect();
    let g = graph(&tables, &[]);
    let m = Metrics::default();
    let l = layout(&g, &m);
    check(&g, &l, &m);
    assert!(l.width > l.boxes[0].w * 3.0, "wider than three boxes: {}", l.width);
    assert!(l.height < l.width * 1.5, "roughly square, not a ribbon: {}x{}", l.width, l.height);
}

#[test]
fn unconnected_groups_do_not_share_a_layering() {
    let g = graph(
        &[
            ("a", &["id"]),
            ("b", &["id", "a_id"]),
            ("x", &["id"]),
            ("y", &["id", "x_id"]),
            ("lonely", &["id"]),
        ],
        &[("b", "a_id", "a", "id"), ("y", "x_id", "x", "id")],
    );
    let m = Metrics::default();
    let l = layout(&g, &m);
    check(&g, &l, &m);
}

#[test]
fn a_long_edge_routes_around_the_tables_it_passes() {
    // c references a, but b sits between them (b references a, c references b as well): the edge c -> a
    // crosses b's layer and must go through the slot reserved for it.
    let g = graph(
        &[("a", &["id"]), ("b", &["id", "a_id"]), ("c", &["id", "a_id", "b_id"])],
        &[("b", "a_id", "a", "id"), ("c", "b_id", "b", "id"), ("c", "a_id", "a", "id")],
    );
    let m = Metrics::default();
    let l = layout(&g, &m);
    check(&g, &l, &m);
    assert!(l.edges[2].points.len() >= 4, "the long edge bends around b: {:?}", l.edges[2].points);
}

#[test]
fn crossings_are_removed_where_the_order_allows_it() {
    // Two parents and two children each referencing "its" parent, input order crosses them.
    let g = graph(
        &[("p1", &["id"]), ("p2", &["id"]), ("c1", &["id", "p"]), ("c2", &["id", "p"])],
        &[("c1", "p", "p2", "id"), ("c2", "p", "p1", "id")],
    );
    let m = Metrics::default();
    let l = layout(&g, &m);
    check(&g, &l, &m);
    assert_eq!(crossings(&l), 0);
}

#[test]
fn multiple_edges_between_the_same_pair_all_appear() {
    let g = graph(
        &[("user", &["id"]), ("msg", &["id", "sender", "receiver"])],
        &[("msg", "sender", "user", "id"), ("msg", "receiver", "user", "id")],
    );
    let m = Metrics::default();
    let l = layout(&g, &m);
    check(&g, &l, &m);
    assert_ne!(l.edges[0].points, l.edges[1].points);
}

#[test]
fn long_column_lists_keep_their_key_columns() {
    let cols: Vec<String> = (0..80).map(|i| format!("c{i}")).collect();
    let refs: Vec<&str> = std::iter::once("id")
        .chain(cols.iter().map(|s| s.as_str()))
        .chain(std::iter::once("late_fk"))
        .collect();
    let g = graph(&[("wide", &refs), ("p", &["id"])], &[("wide", "late_fk", "p", "id")]);
    let m = Metrics::default();
    let l = layout(&g, &m);
    check(&g, &l, &m);
    let b = &l.boxes[g.node_index("public", "wide").unwrap()];
    assert_eq!(b.rows.len(), m.max_rows);
    assert_eq!(b.hidden, 82 - m.max_rows);
    assert!(b.rows.iter().any(|r| g.nodes[b.node].columns[r.column].name == "late_fk"));
}

#[test]
fn layout_is_deterministic() {
    let g = random_graph(60, 90, 7);
    let m = Metrics::default();
    assert_eq!(layout(&g, &m), layout(&g, &m));
}

/// A reproducible pseudo-random schema: `n` tables and `e` foreign keys (cycles and self-references
/// included).
fn random_graph(n: usize, e: usize, seed: u64) -> Graph {
    let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let mut next = move |m: usize| {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((s >> 33) as usize) % m
    };
    let mut g = Graph::default();
    for i in 0..n {
        let mut cols = vec![col("id")];
        for j in 0..(2 + next(6)) {
            cols.push(col(&format!("c{j}")));
        }
        g.nodes.push(GraphNode { schema: "public".into(), name: format!("t{i:03}"), columns: cols });
    }
    for k in 0..e {
        let (f, t) = (next(n), next(n));
        let owner_cols = g.nodes[f].columns.len();
        let ci = 1 + next(owner_cols - 1);
        let name = g.nodes[f].columns[ci].name.clone();
        g.nodes[f].columns[ci].fk = true;
        g.nodes[t].columns[0].referenced = true;
        g.edges.push(GraphEdge {
            name: format!("fk{k}"),
            from: f,
            to: t,
            from_cols: vec![name],
            to_cols: vec!["id".into()],
            optional: next(2) == 0,
            unique: false,
        });
    }
    g
}

#[test]
fn a_hundred_and_fifty_tables_are_fast_and_correct() {
    let g = random_graph(150, 220, 42);
    let m = Metrics::default();
    let t = std::time::Instant::now();
    let l = layout(&g, &m);
    let took = t.elapsed();
    check(&g, &l, &m);
    assert!(took.as_secs_f32() < 5.0, "150 tables took {took:?}");
}

#[test]
fn four_hundred_tables_do_not_blow_up() {
    let g = random_graph(400, 700, 3);
    let m = Metrics::default();
    let t = std::time::Instant::now();
    let l = layout(&g, &m);
    let took = t.elapsed();
    check(&g, &l, &m);
    assert!(took.as_secs_f32() < 15.0, "400 tables took {took:?}");
}

#[test]
fn several_random_graphs_keep_the_invariants() {
    for seed in 1..=12u64 {
        let g = random_graph(12 + seed as usize * 3, 10 + seed as usize * 5, seed);
        let m = Metrics::default();
        check(&g, &layout(&g, &m), &m);
    }
}

#[test]
fn the_empty_graph_has_no_size() {
    let l = layout(&Graph::default(), &Metrics::default());
    assert_eq!((l.width, l.height, l.boxes.len()), (0.0, 0.0, 0));
}

#[test]
fn reading_order_is_left_to_right_then_top_to_bottom() {
    let g = graph(
        &[("a", &["id"]), ("b", &["id", "a_id"]), ("c", &["id", "b_id"])],
        &[("b", "a_id", "a", "id"), ("c", "b_id", "b", "id")],
    );
    let l = layout(&g, &Metrics::default());
    assert_eq!(l.reading_order(), vec![0, 1, 2]);
}

// ------------------------------------------------------------------------------------------ catalog

fn table(name: &str, cols: &[(&str, bool)], pk: &[&str], fks: Vec<ForeignKey>) -> Table {
    Table {
        name: name.into(),
        columns: cols
            .iter()
            .map(|(c, nullable)| ColumnInfo {
                name: c.to_string(),
                data_type: "integer".into(),
                kind: TypeKind::Integer,
                nullable: *nullable,
                ..Default::default()
            })
            .collect(),
        primary_key: pk.iter().map(|s| s.to_string()).collect(),
        foreign_keys: fks,
        ..Default::default()
    }
}

fn fk(name: &str, cols: &[&str], schema: Option<&str>, table: &str, refs: &[&str]) -> ForeignKey {
    ForeignKey {
        name: name.into(),
        columns: cols.iter().map(|s| s.to_string()).collect(),
        referenced_schema: schema.map(str::to_string),
        referenced_table: table.into(),
        referenced_columns: refs.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn catalog() -> Catalog {
    let public = Schema {
        name: "public".into(),
        tables: vec![
            table(
                "orders",
                &[("id", false), ("customer_id", true), ("region", false)],
                &["id"],
                vec![
                    fk("orders_customer", &["customer_id"], None, "customers", &["id"]),
                    fk("orders_audit", &["region"], Some("audit"), "regions", &["code"]),
                ],
            ),
            table("customers", &[("id", false)], &["id"], vec![]),
            {
                let mut t = table(
                    "profile",
                    &[("customer_id", false)],
                    &["customer_id"],
                    vec![fk("profile_c", &["customer_id"], None, "customers", &["id"])],
                );
                t.unique_constraints =
                    vec![UniqueConstraint { name: "u".into(), columns: vec!["customer_id".into()] }];
                t
            },
            {
                let mut t = table("v", &[("x", true)], &[], vec![]);
                t.kind = theta_core::TableKind::View;
                t
            },
        ],
        ..Default::default()
    };
    let audit = Schema {
        name: "audit".into(),
        tables: vec![table("regions", &[("code", false)], &["code"], vec![])],
        ..Default::default()
    };
    Catalog { schemas: vec![public, audit], default_schema: Some("public".into()) }
}

#[test]
fn a_schema_scope_has_its_tables_sorted_and_no_views() {
    let g = from_catalog(&catalog(), &Scope::Schema("public".into()));
    let names: Vec<&str> = g.nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["customers", "orders", "profile"]);
    assert_eq!(g.edges.len(), 2, "the key into another schema is left out");
    assert!(g.notes.iter().any(|n| n.starts_with("1 foreign key to tables outside")), "{:?}", g.notes);
    let o = &g.nodes[1];
    assert!(o.columns[1].fk && o.columns[0].pk);
    assert!(g.nodes[0].columns[0].referenced);
    let to_customer = g.edges.iter().find(|e| e.name == "orders_customer").unwrap();
    assert!(to_customer.optional, "the column allows NULL");
    let profile = g.edges.iter().find(|e| e.name == "profile_c").unwrap();
    assert!(profile.unique && !profile.optional, "primary key and unique constraint make it one-to-one");
}

#[test]
fn catalog_order_does_not_change_the_graph() {
    let mut c = catalog();
    let a = from_catalog(&c, &Scope::Schema("public".into()));
    c.schemas[0].tables.reverse();
    let b = from_catalog(&c, &Scope::Schema("public".into()));
    assert_eq!(a, b);
}

#[test]
fn a_table_scope_is_the_table_and_its_neighbours_in_both_directions() {
    let g = from_catalog(&catalog(), &Scope::Table { schema: "public".into(), table: "customers".into() });
    let names: Vec<String> = (0..g.nodes.len()).map(|i| g.label(i)).collect();
    assert_eq!(names, ["customers", "orders", "profile"]);
    let g = from_catalog(&catalog(), &Scope::Table { schema: "public".into(), table: "orders".into() });
    assert!(g.multi_schema);
    let names: Vec<String> = (0..g.nodes.len()).map(|i| g.label(i)).collect();
    assert_eq!(names, ["audit.regions", "public.customers", "public.orders"]);
    assert_eq!(g.edges.len(), 2);
}

#[test]
fn an_unknown_scope_is_empty() {
    assert!(from_catalog(&catalog(), &Scope::Schema("nope".into())).nodes.is_empty());
    assert!(
        from_catalog(&catalog(), &Scope::Table { schema: "public".into(), table: "nope".into() })
            .nodes
            .is_empty()
    );
}

#[test]
fn neighbours_and_edges_of() {
    let g = from_catalog(&catalog(), &Scope::Schema("public".into()));
    assert_eq!(g.neighbours(0), vec![1, 2]);
    assert_eq!(g.edges_of(1).count(), 1);
}

// --------------------------------------------------------------------------------------- exporters

#[test]
fn mermaid_text_is_exact() {
    let g = from_catalog(&catalog(), &Scope::Schema("public".into()));
    let want = "erDiagram
    customers |o--o{ orders : \"orders_customer\"
    customers ||--o| profile : \"profile_c\"
    customers {
        integer id PK
    }
    orders {
        integer id PK
        integer customer_id FK
        integer region FK
    }
    profile {
        integer customer_id PK, FK
    }
";
    assert_eq!(mermaid::render(&g), want);
}

#[test]
fn mermaid_makes_names_and_types_safe() {
    let mut g = graph(&[("my table", &["id", "odd col!"])], &[]);
    g.nodes[0].columns[1].data_type = "numeric(10,2)".into();
    g.nodes[0].columns[0].data_type = "double precision".into();
    g.multi_schema = true;
    let t = mermaid::render(&g);
    assert!(t.contains("\"public.my table\" {"), "{t}");
    assert!(t.contains("numeric(10_2) odd_col_"), "{t}");
    assert!(t.contains("double_precision id PK"), "{t}");
}

#[test]
fn svg_is_well_formed_escaped_and_complete() {
    let mut g = graph(&[("a", &["id"]), ("b<&>", &["id", "a_id"])], &[("b<&>", "a_id", "a", "id")]);
    g.nodes[1].columns[1].data_type = "varchar(\"x\")".into();
    let m = Metrics::default();
    let l = layout(&g, &m);
    let s = svg::render(&g, &l, &m, &svg::Palette::light(), "Schema \"public\" & more");
    assert!(s.starts_with("<svg "));
    assert!(s.trim_end().ends_with("</svg>"));
    assert!(s.contains("b&lt;&amp;&gt;"), "names are escaped");
    assert!(s.contains("&quot;x&quot;"));
    assert!(s.contains("<title>Schema &quot;public&quot; &amp; more</title>"));
    assert_eq!(s.matches("<polyline").count(), 1, "one edge, no bracket");
    assert_eq!(s.matches("<path ").count(), 2, "one header per table");
    // Every opened element is closed or self-closed: a cheap well-formedness check.
    let opens = s.matches('<').count();
    let closes = s.matches('>').count();
    assert_eq!(opens, closes);
    assert_eq!(
        svg::render(&g, &l, &m, &svg::Palette::light(), "x"),
        svg::render(&g, &l, &m, &svg::Palette::light(), "x")
    );
}

#[test]
fn optional_keys_are_dashed_in_the_svg() {
    let mut g = graph(&[("a", &["id"]), ("b", &["id", "a_id"])], &[("b", "a_id", "a", "id")]);
    let m = Metrics::default();
    let l = layout(&g, &m);
    assert!(!svg::render(&g, &l, &m, &svg::Palette::light(), "t").contains("stroke-dasharray"));
    g.edges[0].optional = true;
    assert!(svg::render(&g, &l, &m, &svg::Palette::light(), "t").contains("stroke-dasharray"));
}

#[test]
fn end_marks_put_a_crow_s_foot_on_the_owner_and_a_bar_on_the_parent() {
    let g = graph(&[("a", &["id"]), ("b", &["id", "a_id"])], &[("b", "a_id", "a", "id")]);
    let l = layout(&g, &Metrics::default());
    let marks = crate::layout::end_marks(&l.edges[0], false);
    assert_eq!(marks.len(), 4, "three prongs and one bar");
    assert_eq!(crate::layout::end_marks(&l.edges[0], true).len(), 2, "unique: a bar at each end");
}
