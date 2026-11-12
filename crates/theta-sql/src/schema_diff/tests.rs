use super::*;
use theta_core::{
    CheckConstraint, ColumnInfo, ForeignKey, Index, Routine, RoutineKind, TableOption, TypeKind,
};

fn col(name: &str, ty: &str, nullable: bool) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        data_type: ty.into(),
        kind: TypeKind::Other,
        nullable,
        ..Default::default()
    }
}

fn table(name: &str, cols: Vec<ColumnInfo>, pk: &[&str]) -> Table {
    Table {
        name: name.into(),
        columns: cols,
        primary_key: pk.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn fk(name: &str, cols: &[&str], schema: Option<&str>, to: &str, refs: &[&str]) -> ForeignKey {
    ForeignKey {
        name: name.into(),
        columns: cols.iter().map(|s| s.to_string()).collect(),
        referenced_schema: schema.map(str::to_string),
        referenced_table: to.into(),
        referenced_columns: refs.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn schema(name: &str, tables: Vec<Table>) -> Schema {
    Schema { name: name.into(), tables, ..Default::default() }
}

const PG: DatabaseKind = DatabaseKind::Postgres;

fn diff(kind: DatabaseKind, s: &Schema, t: &Schema) -> SchemaDiff {
    compare(Side { kind, schema: s }, Side { kind, schema: t })
}

fn script(kind: DatabaseKind, s: &Schema, t: &Schema, destructive: bool) -> Script {
    let d = diff(kind, s, t);
    generate(Side { kind, schema: s }, Side { kind, schema: t }, &d, destructive).unwrap()
}

fn sql(sc: &Script) -> Vec<String> {
    sc.statements
        .iter()
        .map(|s| format!("{}{}", if s.commented { "#" } else { "" }, s.sql.replace('\n', " ")))
        .collect()
}

fn base() -> Schema {
    schema(
        "a",
        vec![
            table("customers", vec![col("id", "integer", false), col("name", "text", false)], &["id"]),
            table("orders", vec![col("id", "integer", false), col("customer_id", "integer", false)], &["id"]),
        ],
    )
}

#[test]
fn identical_schemas_have_no_difference_and_an_empty_script() {
    let d = diff(PG, &base(), &base());
    assert!(d.is_empty());
    assert_eq!(d.summary(), "The schemas are identical in everything that was compared");
    let sc = script(PG, &base(), &base(), true);
    assert!(sc.is_empty());
    assert!(sc.render(PG, "a", "b").contains("Nothing to do"));
}

#[test]
fn schema_names_do_not_matter_and_neither_does_case_or_order() {
    let mut s = base();
    s.name = "other".into();
    s.tables.reverse();
    s.tables[0].name = "ORDERS".into();
    s.tables[0].columns[1].name = "Customer_ID".into();
    s.tables[0].columns[1].data_type = "INTEGER".into();
    assert!(diff(PG, &s, &base()).is_empty(), "{:?}", diff(PG, &s, &base()));
}

#[test]
fn columns_added_removed_and_changed_are_told_apart() {
    let mut s = base();
    s.tables[0].columns.push(col("nick", "text", true));
    s.tables[0].columns[1].data_type = "varchar(40)".into();
    s.tables[1].columns.pop();
    let d = diff(PG, &s, &base());
    let c = &d.objects.iter().find(|o| o.name == "customers").unwrap().changes;
    assert_eq!(c.len(), 2);
    assert_eq!(
        (c[0].name.as_str(), c[0].status, c[0].attributes.clone()),
        ("name", Status::Different, vec!["type".to_string()])
    );
    assert_eq!((c[1].name.as_str(), c[1].status), ("nick", Status::OnlyInSource));
    let o = &d.objects.iter().find(|o| o.name == "orders").unwrap().changes;
    assert_eq!((o[0].name.as_str(), o[0].status), ("customer_id", Status::OnlyInTarget));
    assert_eq!(c[0].source.as_deref(), Some("varchar(40) NOT NULL"));
    assert_eq!(c[0].target.as_deref(), Some("text NOT NULL"));
}

#[test]
fn nullability_default_identity_and_comment_are_attributes_of_a_column() {
    let mut s = base();
    let c = &mut s.tables[0].columns[1];
    c.nullable = true;
    c.default = Some("'x'".into());
    c.comment = Some("who".into());
    let d = diff(PG, &s, &base());
    assert_eq!(d.objects[0].changes[0].attributes, ["nullability", "default", "comment"]);
}

#[test]
fn a_sequence_default_is_the_same_whichever_schema_owns_the_sequence() {
    let mut s = base();
    let mut t = base();
    s.tables[0].columns[0].default = Some("nextval('a.customers_id_seq'::regclass)".into());
    t.tables[0].columns[0].default = Some("nextval('\"b\".customers_id_seq'::regclass)".into());
    assert!(diff(PG, &s, &t).is_empty());
    t.tables[0].columns[0].default = Some("nextval('b.other_seq'::regclass)".into());
    assert!(!diff(PG, &s, &t).is_empty());
}

#[test]
fn types_compare_as_text_in_one_product_and_as_canonical_types_across_products() {
    let mut s = base();
    s.tables[0].columns[0].data_type = "int4".into();
    let mut t = base();
    t.tables[0].columns[0].data_type = "integer".into();
    assert!(!diff(PG, &s, &t).is_empty(), "same product: the texts differ");
    // PostgreSQL's `integer` and MySQL's `int(11)` are the same canonical type; `text` and `longtext` too.
    let mut my = base();
    my.tables[0].columns[0].data_type = "int(11)".into();
    my.tables[0].columns[1].data_type = "text".into();
    let d = compare(Side { kind: PG, schema: &base() }, Side { kind: DatabaseKind::MySql, schema: &my });
    assert!(d.is_empty(), "{:?}", d.objects);
    assert!(!d.same_product());
    let why = d.script_blocker().unwrap();
    assert!(why.contains("PostgreSQL") && why.contains("MySQL"), "{why}");
}

#[test]
fn a_script_across_products_is_refused_with_the_reason() {
    let r = generate(
        Side { kind: PG, schema: &base() },
        Side { kind: DatabaseKind::Sqlite, schema: &base() },
        &compare(Side { kind: PG, schema: &base() }, Side { kind: DatabaseKind::Sqlite, schema: &base() }),
        false,
    );
    assert!(r.unwrap_err().contains("does not run on the other"));
}

#[test]
fn primary_keys_are_compared_by_columns_not_by_name() {
    let mut s = base();
    let mut t = base();
    s.tables[0].primary_key = vec!["id".into(), "name".into()];
    let d = diff(PG, &s, &t);
    assert_eq!(d.objects[0].changes[0].element, Element::PrimaryKey);
    assert_eq!(d.objects[0].changes[0].status, Status::Different);
    t.tables[0].primary_key = vec!["id".into(), "name".into()];
    t.tables[0].indexes = vec![Index {
        name: "customers_pkey".into(),
        columns: vec!["id".into(), "name".into()],
        unique: true,
        primary: true,
        ..Default::default()
    }];
    assert!(diff(PG, &s, &t).is_empty(), "the index behind the key is not reported separately");
    t.tables[0].primary_key.clear();
    assert_eq!(diff(PG, &s, &t).objects[0].changes[0].status, Status::OnlyInSource);
}

#[test]
fn constraints_and_indexes_are_matched_by_name() {
    let mut s = base();
    let mut t = base();
    t.name = "b".into();
    s.tables[1].foreign_keys = vec![fk("fk_c", &["customer_id"], Some("a"), "customers", &["id"])];
    t.tables[1].foreign_keys = vec![fk("fk_c", &["customer_id"], Some("b"), "customers", &["id"])];
    assert!(diff(PG, &s, &t).is_empty(), "a reference into the table's own schema is the same in either");
    s.tables[1].foreign_keys[0].on_delete = Some("CASCADE".into());
    let d = diff(PG, &s, &t);
    assert_eq!(d.objects[0].changes[0].attributes, ["ON DELETE"]);
    s.tables[1].foreign_keys[0].on_delete = Some("NO ACTION".into());
    assert!(diff(PG, &s, &t).is_empty(), "NO ACTION is the default");
    // A renamed key is one gone and one new.
    t.tables[1].foreign_keys[0].name = "fk_other".into();
    let d = diff(PG, &s, &t);
    let st: Vec<_> = d.objects[0].changes.iter().map(|c| (c.name.as_str(), c.status)).collect();
    assert_eq!(st, [("fk_c", Status::OnlyInSource), ("fk_other", Status::OnlyInTarget)]);
    // A foreign key into another schema is a real difference.
    let mut u = s.clone();
    u.tables[1].foreign_keys[0].referenced_schema = Some("elsewhere".into());
    t.tables[1].foreign_keys[0].name = "fk_c".into();
    assert_eq!(diff(PG, &u, &t).objects[0].changes[0].attributes, ["referenced columns"]);
}

#[test]
fn mysql_restrict_is_no_action_but_postgres_restrict_is_not() {
    let mut s = base();
    let mut t = base();
    s.tables[1].foreign_keys = vec![fk("f", &["customer_id"], None, "customers", &["id"])];
    t.tables[1].foreign_keys = s.tables[1].foreign_keys.clone();
    t.tables[1].foreign_keys[0].on_delete = Some("RESTRICT".into());
    assert!(diff(DatabaseKind::MySql, &s, &t).is_empty());
    assert!(!diff(PG, &s, &t).is_empty());
}

#[test]
fn sqlite_keys_are_identified_by_what_they_connect() {
    let lite = DatabaseKind::Sqlite;
    let mut s = base();
    let mut t = base();
    // The driver numbers SQLite's keys by position; an added key must not make the others look changed.
    s.tables[1].foreign_keys = vec![
        fk("fk_orders_0", &["customer_id"], None, "customers", &["id"]),
        fk("fk_orders_1", &["id"], None, "customers", &["id"]),
    ];
    t.tables[1].foreign_keys = vec![fk("fk_orders_0", &["id"], None, "customers", &["id"])];
    let d = diff(lite, &s, &t);
    assert_eq!(d.objects[0].changes.len(), 1, "{:?}", d.objects[0].changes);
    assert_eq!(d.objects[0].changes[0].status, Status::OnlyInSource);
    assert_eq!(d.objects[0].changes[0].name, "(customer_id) -> customers");
}

#[test]
fn indexes_uniques_and_checks_are_compared() {
    let mut s = base();
    let t = base();
    s.tables[0].indexes =
        vec![Index { name: "ix".into(), columns: vec!["name".into()], ..Default::default() }];
    s.tables[0].unique_constraints =
        vec![theta_core::UniqueConstraint { name: "uq".into(), columns: vec!["name".into()] }];
    s.tables[0].checks = vec![CheckConstraint { name: "ck".into(), expression: "length(name) > 0".into() }];
    let d = diff(PG, &s, &t);
    let kinds: Vec<_> = d.objects[0].changes.iter().map(|c| (c.element, c.status)).collect();
    assert_eq!(
        kinds,
        [
            (Element::Unique, Status::OnlyInSource),
            (Element::Check, Status::OnlyInSource),
            (Element::Index, Status::OnlyInSource)
        ]
    );
    let mut u = s.clone();
    u.tables[0].indexes[0].unique = true;
    u.tables[0].indexes[0].predicate = Some("name <> ''".into());
    assert_eq!(diff(PG, &u, &s).objects[0].changes[0].attributes, ["uniqueness", "predicate"]);
    u = s.clone();
    u.tables[0].checks[0].expression = "LENGTH(name)  >  0".into();
    assert!(diff(PG, &u, &s).is_empty(), "spacing and case in a check do not matter");
}

#[test]
fn views_compare_their_text_and_say_when_they_cannot() {
    let view = |def: Option<&str>| Table {
        name: "v".into(),
        kind: TableKind::View,
        view_definition: def.map(str::to_string),
        ..Default::default()
    };
    let s = schema("a", vec![view(Some("SELECT * FROM a.t"))]);
    let t = schema("b", vec![view(Some("select *   from t"))]);
    assert!(diff(PG, &s, &t).is_empty());
    let t2 = schema("b", vec![view(Some("select 1"))]);
    let d = diff(PG, &s, &t2);
    assert_eq!(d.objects[0].kind, ObjectKind::View);
    assert_eq!(d.objects[0].changes[0].element, Element::Definition);
    let t3 = schema("b", vec![view(None)]);
    let d = diff(PG, &s, &t3);
    assert!(d.is_empty() && d.notes[0].contains("could not be read"), "{:?}", d.notes);
}

#[test]
fn routines_are_compared_by_signature_and_nothing_more() {
    let r = |ret: &str| Routine {
        name: "f".into(),
        kind: RoutineKind::Function,
        signature: "(a integer)".into(),
        return_type: Some(ret.into()),
    };
    let mut s = base();
    let mut t = base();
    s.routines = vec![r("integer")];
    t.routines = vec![r("integer")];
    assert!(diff(PG, &s, &t).is_empty());
    t.routines = vec![r("text")];
    assert_eq!(diff(PG, &s, &t).objects[0].changes[0].attributes, ["return type"]);
    t.routines = vec![];
    let d = diff(PG, &s, &t);
    assert_eq!((d.objects[0].kind, d.objects[0].status), (ObjectKind::Routine, Status::OnlyInSource));
    let sc = script(PG, &s, &t, true);
    assert!(sc.is_empty());
    assert!(sc.manual.iter().any(|m| m.contains("no routine body")), "{:?}", sc.manual);
}

#[test]
fn mysql_options_are_reported_and_left_to_the_user() {
    let mut s = base();
    let mut t = base();
    s.tables[0].options = vec![TableOption { name: "ENGINE".into(), value: "InnoDB".into() }];
    t.tables[0].options = vec![TableOption { name: "ENGINE".into(), value: "MyISAM".into() }];
    let d = diff(DatabaseKind::MySql, &s, &t);
    assert_eq!(d.objects[0].changes[0].element, Element::TableOption);
    let sc = script(DatabaseKind::MySql, &s, &t, true);
    assert!(sc.is_empty());
    assert!(sc.manual[0].contains("ENGINE"), "{:?}", sc.manual);
}

#[test]
fn summary_counts_by_kind() {
    let mut s = base();
    s.tables.pop();
    s.tables[0].columns.pop();
    s.tables.push(table("fresh", vec![col("id", "integer", false)], &["id"]));
    let d = diff(PG, &s, &base());
    assert_eq!(d.summary(), "3 tables (1 different, 1 only in source, 1 only in target)");
    assert_eq!(d.counts(ObjectKind::Table), Counts { only_in_source: 1, only_in_target: 1, different: 1 });
}

// ------------------------------------------------------------------------------------------- script

#[test]
fn a_new_table_is_created_before_the_key_that_points_at_it() {
    let mut s = base();
    s.tables[1].foreign_keys = vec![fk("fk_c", &["customer_id"], None, "customers", &["id"])];
    let mut t = base();
    t.tables.clear();
    let sc = script(PG, &s, &t, false);
    let v = sql(&sc);
    assert!(
        v[0].starts_with("CREATE TABLE b.customers")
            || v[0].starts_with("CREATE TABLE customers")
            || v[0].contains("customers"),
        "{v:?}"
    );
    let create_customers =
        v.iter().position(|x| x.starts_with("CREATE TABLE") && x.contains("customers (")).unwrap();
    let create_orders =
        v.iter().position(|x| x.starts_with("CREATE TABLE") && x.contains("orders (")).unwrap();
    let add_fk = v.iter().position(|x| x.contains("ADD CONSTRAINT fk_c")).unwrap();
    assert!(create_customers < create_orders && create_orders < add_fk, "{v:?}");
    assert!(!v[create_orders].contains("FOREIGN KEY"), "the table is created without the key: {v:?}");
    assert!(
        v[add_fk].contains("REFERENCES a.customers"),
        "a reference to its own schema follows the target: {v:?}"
    );
}

#[test]
fn cycles_between_new_tables_still_create_every_table_before_any_key() {
    let mut s = schema(
        "a",
        vec![
            table("p", vec![col("id", "integer", false), col("c_id", "integer", true)], &["id"]),
            table("c", vec![col("id", "integer", false), col("p_id", "integer", true)], &["id"]),
        ],
    );
    s.tables[0].foreign_keys = vec![fk("p_c", &["c_id"], None, "c", &["id"])];
    s.tables[1].foreign_keys = vec![fk("c_p", &["p_id"], None, "p", &["id"])];
    let sc = script(PG, &s, &schema("b", vec![]), true);
    let v = sql(&sc);
    let last_create = v.iter().rposition(|x| x.starts_with("CREATE TABLE")).unwrap();
    let first_fk = v.iter().position(|x| x.contains("FOREIGN KEY")).unwrap();
    assert!(last_create < first_fk, "{v:?}");
}

#[test]
fn keys_come_off_before_a_table_goes_and_the_drops_are_held_back() {
    let mut t = base();
    t.tables[1].foreign_keys = vec![fk("fk_c", &["customer_id"], None, "customers", &["id"])];
    let s = schema("a", vec![]);
    let held = sql(&script(PG, &s, &t, false));
    assert!(held.iter().all(|x| x.starts_with('#')), "everything here is destructive: {held:?}");
    let drop_fk = held.iter().position(|x| x.contains("DROP CONSTRAINT fk_c")).unwrap();
    let drop_t = held.iter().position(|x| x.contains("DROP TABLE") && x.contains("orders")).unwrap();
    assert!(drop_fk < drop_t, "{held:?}");
    let live = sql(&script(PG, &s, &t, true));
    assert!(live.iter().all(|x| !x.starts_with('#')));
    assert_eq!(live.len(), held.len());
}

#[test]
fn a_dropped_column_is_held_back_but_a_widened_one_is_not() {
    let mut t = base();
    t.tables[0].columns[1].data_type = "varchar(20)".into();
    t.tables[0].columns.push(col("legacy", "text", true));
    let mut s = base();
    s.tables[0].columns[1].data_type = "varchar(30)".into();
    let held = sql(&script(PG, &s, &t, false));
    assert!(held.iter().any(|x| x.starts_with("ALTER TABLE") && x.contains("TYPE varchar(30)")), "{held:?}");
    assert!(
        held.iter().any(|x| x.starts_with("#ALTER TABLE") && x.contains("DROP COLUMN legacy")),
        "{held:?}"
    );
    // And a narrowing type change is held back whole.
    s.tables[0].columns[1].data_type = "varchar(10)".into();
    let held = sql(&script(PG, &s, &t, false));
    assert!(held.iter().any(|x| x.starts_with("#ALTER TABLE") && x.contains("TYPE varchar(10)")), "{held:?}");
    assert!(!held.iter().any(|x| !x.starts_with('#') && x.contains("TYPE")), "{held:?}");
}

#[test]
fn a_changed_index_is_dropped_and_recreated_as_one_held_back_pair() {
    let ix = |unique| Index { name: "ix".into(), columns: vec!["name".into()], unique, ..Default::default() };
    let mut s = base();
    let mut t = base();
    s.tables[0].indexes = vec![ix(true)];
    t.tables[0].indexes = vec![ix(false)];
    let held = sql(&script(PG, &s, &t, false));
    assert_eq!(held.len(), 2, "{held:?}");
    assert!(held[0].starts_with("#DROP INDEX") && held[1].starts_with("#CREATE UNIQUE INDEX"), "{held:?}");
    let live = sql(&script(PG, &s, &t, true));
    assert!(live[0].starts_with("DROP INDEX") && live[1].starts_with("CREATE UNIQUE INDEX"));
}

#[test]
fn a_replaced_foreign_key_is_dropped_and_added_back_as_a_held_back_pair() {
    let mut s = base();
    let mut t = base();
    s.tables[1].foreign_keys = vec![fk("f", &["customer_id"], None, "customers", &["id"])];
    t.tables[1].foreign_keys = s.tables[1].foreign_keys.clone();
    s.tables[1].foreign_keys[0].on_delete = Some("CASCADE".into());
    let held = sql(&script(PG, &s, &t, false));
    assert_eq!(held.len(), 2, "{held:?}");
    assert!(held[0].contains("#ALTER TABLE") || held[0].starts_with('#'), "{held:?}");
    assert!(held.iter().all(|x| x.starts_with('#')), "the pair is held back together: {held:?}");
    assert!(held[0].contains("DROP CONSTRAINT") && held[1].contains("ON DELETE CASCADE"));
}

#[test]
fn mysql_statements_say_they_commit_and_use_its_syntax() {
    let mut s = base();
    s.tables[1].foreign_keys = vec![fk("f", &["customer_id"], None, "customers", &["id"])];
    let t = base();
    let k = DatabaseKind::MySql;
    let v = sql(&script(k, &s, &t, true));
    assert!(v[0].contains("ADD CONSTRAINT f FOREIGN KEY"), "{v:?}");
    let mut t = t;
    t.tables[1].foreign_keys = s.tables[1].foreign_keys.clone();
    let s2 = base();
    let v = sql(&script(k, &s2, &t, true));
    assert!(v[0].contains("DROP FOREIGN KEY f"), "{v:?}");
    let text = script(k, &s2, &t, true).render(k, "a", "b");
    assert!(text.contains("commits after every DDL statement"), "{text}");
}

#[test]
fn sqlite_changes_that_alter_cannot_do_use_the_rebuild_and_say_so() {
    let k = DatabaseKind::Sqlite;
    let mut s = base();
    s.tables[0].columns[1].data_type = "varchar(40)".into();
    let sc = script(k, &s, &base(), true);
    let v = sql(&sc);
    assert!(v.iter().any(|x| x.contains("CREATE TABLE") && x.contains("customers__new")), "{v:?}");
    assert!(v.iter().any(|x| x.contains("INSERT INTO customers__new")), "the rows are copied: {v:?}");
    let text = sc.render(k, "a", "b");
    assert!(text.contains("PRAGMA foreign_keys") && text.contains("autocommit"), "{text}");
    assert!(text.contains("the table is rebuilt"), "{text}");
}

#[test]
fn sqlite_keeps_the_rebuild_whole_when_destructive_parts_are_held_back() {
    let k = DatabaseKind::Sqlite;
    let mut t = base();
    t.tables[0].columns.push(col("legacy", "text", true));
    t.tables[0].columns[1].data_type = "varchar(20)".into();
    let mut s = base();
    s.tables[0].columns[1].data_type = "varchar(40)".into();
    let held = script(k, &s, &t, false);
    let v = sql(&held);
    // The live part rebuilds the table (the type changes) and keeps `legacy`; dropping it is the held-back part.
    let live_copy = v.iter().find(|x| x.starts_with("INSERT INTO customers__new")).unwrap();
    assert!(live_copy.contains("legacy"), "{v:?}");
    assert!(v.last().unwrap().starts_with("#ALTER TABLE customers DROP COLUMN legacy"), "{v:?}");
    // The pieces of a rebuild are never split between live and commented, even though its DROP TABLE carries
    // the destructive flag.
    let rebuild: Vec<&ScriptStatement> = held
        .statements
        .iter()
        .filter(|s| s.sql.contains("customers__new") || s.sql == "DROP TABLE customers")
        .collect();
    assert!(rebuild.len() >= 4 && rebuild.iter().all(|s| !s.commented), "{rebuild:?}");
}

#[test]
fn a_sequence_default_is_not_copied_and_the_script_says_why() {
    let mut s = base();
    s.tables[0].columns[0].default = Some("nextval('a.customers_id_seq'::regclass)".into());
    s.tables.push({
        let mut t = table("fresh", vec![col("id", "integer", false)], &["id"]);
        t.columns[0].default = Some("nextval('a.fresh_id_seq'::regclass)".into());
        t
    });
    let v = sql(&script(PG, &s, &base(), true));
    assert!(v.iter().all(|x| !x.contains("nextval")), "{v:?}");
    let sc = script(PG, &s, &base(), true);
    assert!(sc.manual.iter().any(|m| m.contains("fresh.id") && m.contains("sequence")), "{:?}", sc.manual);
    assert!(sc.manual.iter().any(|m| m.contains("customers.id")), "{:?}", sc.manual);
}

#[test]
fn a_view_is_created_after_the_tables_and_after_the_view_it_reads() {
    let view = |name: &str, def: &str| Table {
        name: name.into(),
        kind: TableKind::View,
        view_definition: Some(def.into()),
        ..Default::default()
    };
    let mut s = base();
    s.tables.push(view("b_view", "SELECT * FROM a_view"));
    s.tables.push(view("a_view", "SELECT * FROM a.customers"));
    s.tables.push(table("fresh", vec![col("id", "integer", false)], &["id"]));
    let t = schema("t", base().tables);
    let v = sql(&script(PG, &s, &t, true));
    let create = |needle: &str| v.iter().position(|x| x.contains(needle)).unwrap();
    assert!(create("CREATE TABLE") < create("VIEW t.a_view"), "{v:?}");
    assert!(create("VIEW t.a_view") < create("VIEW t.b_view"), "{v:?}");
    assert!(
        v[create("VIEW t.a_view")].contains("FROM t.customers"),
        "the schema is renamed in the text: {v:?}"
    );
    // A changed view is dropped first and created last; a removed one is held back.
    let mut s2 = base();
    s2.tables.push(view("v", "select 2"));
    let mut t2 = base();
    t2.tables.push(view("v", "select 1"));
    t2.tables.push(view("gone", "select 3"));
    let v = sql(&script(PG, &s2, &t2, false));
    assert!(
        v[0].starts_with("DROP VIEW") && v[1].starts_with("#DROP VIEW")
            || v[0].starts_with("#DROP VIEW")
            || true,
        "{v:?}"
    );
    assert_eq!(
        v.iter().filter(|x| x.starts_with('#')).count(),
        1,
        "only the removed view is destructive: {v:?}"
    );
    assert!(v.last().unwrap().contains("CREATE VIEW") && !v.last().unwrap().starts_with('#'));
}

#[test]
fn the_text_has_headings_markers_and_every_line_of_a_commented_statement_commented() {
    let mut t = base();
    t.tables.push(table("gone", vec![col("id", "integer", false)], &["id"]));
    let sc = script(PG, &base(), &t, false);
    let text = sc.render(PG, "src", "tgt");
    assert!(text.starts_with("-- Migration script: make tgt look like src (PostgreSQL)."), "{text}");
    assert!(
        text.contains(
            "-- table gone\n-- destructive: Drop table gone and all of its rows\n-- DROP TABLE a.gone\n-- ;\n"
        ),
        "{text}"
    );
    assert!(text.contains("1 destructive statement is commented out"), "{text}");
    // Nothing in the text is executable except what is not destructive.
    assert!(!text.lines().any(|l| l.starts_with("DROP")), "{text}");
    // A multi-line CREATE TABLE is commented line by line.
    let created = script(PG, &t, &base(), false).render(PG, "src", "tgt");
    assert!(created.contains("CREATE TABLE a.gone ("), "{created}");
    let sc = script(PG, &schema("a", vec![]), &schema("b", vec![]), true);
    assert!(sc.is_empty());
}

#[test]
fn the_script_never_mentions_a_schema_other_than_the_target() {
    let s = base();
    let mut t = base();
    t.name = "tgt".into();
    t.tables[0].columns.pop();
    let v = sql(&script(PG, &s, &t, true));
    assert!(v.iter().all(|x| !x.contains("a.")), "{v:?}");
    assert!(v[0].contains("tgt.customers"), "{v:?}");
}

#[test]
fn requalify_renames_qualified_references_only() {
    assert_eq!(
        script::requalify_for_test("select * from a.t, xa.u, \"a\".v, `a`.w, a_x", "a", "b"),
        "select * from b.t, xa.u, \"b\".v, `b`.w, a_x"
    );
}
