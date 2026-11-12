use super::*;
use std::collections::VecDeque;
use theta_core::TypeKind;

const SAME: Rules = Rules { cross_product: false };
const CROSS: Rules = Rules { cross_product: true };

fn eq(a: Value, b: Value, r: Rules) -> bool {
    values_equal(&a, &b, r) && values_equal(&b, &a, r)
}

fn t(s: &str) -> Value {
    Value::Text(s.into())
}

fn dec(s: &str) -> Value {
    Value::Decimal(s.into())
}

#[test]
fn null_equals_null_and_nothing_else() {
    for r in [SAME, CROSS] {
        assert!(eq(Value::Null, Value::Null, r));
        assert!(!values_equal(&Value::Null, &Value::Int(0), r));
        assert!(!values_equal(&t(""), &Value::Null, r));
        assert!(!values_equal(&Value::Bool(false), &Value::Null, r));
    }
}

#[test]
fn numbers_compare_by_value_not_by_scale() {
    for r in [SAME, CROSS] {
        assert!(eq(dec("1.50"), dec("1.5"), r));
        assert!(eq(dec("10"), Value::Int(10), r));
        assert!(eq(dec("-0.00"), Value::Int(0), r));
        assert!(eq(dec("007.100"), dec("7.1"), r));
        assert!(!eq(dec("1.51"), dec("1.5"), r));
        assert!(!eq(dec("-1"), dec("1"), r));
        assert!(eq(Value::Int(i64::MAX), dec("9223372036854775807"), r), "no float rounding of big integers");
        assert!(!eq(Value::Int(i64::MAX), dec("9223372036854775806"), r));
        assert!(eq(Value::UInt(u64::MAX), dec("18446744073709551615"), r));
    }
}

#[test]
fn only_across_products_a_boolean_is_one_and_text_can_be_a_number() {
    assert!(!values_equal(&Value::Bool(true), &Value::Int(1), SAME));
    assert!(eq(Value::Bool(true), Value::Int(1), CROSS));
    assert!(eq(Value::Bool(false), Value::Int(0), CROSS));
    assert!(!eq(Value::Bool(true), Value::Int(0), CROSS));
    assert!(!eq(Value::Bool(true), Value::Bool(false), CROSS));
    assert!(!values_equal(&t("10"), &Value::Int(10), SAME));
    assert!(eq(t("10"), Value::Int(10), CROSS));
    assert!(eq(t("10.0"), dec("10"), CROSS));
    assert!(!eq(t("ten"), Value::Int(10), CROSS));
    assert!(!eq(t("1"), t("1.0"), CROSS), "two texts are compared as texts");
}

#[test]
fn floats_are_exact_within_a_product_and_tolerant_between_products() {
    assert!(eq(Value::Float(0.1), Value::Float(0.1), SAME));
    assert!(!values_equal(&Value::Float(0.1), &Value::Float(0.100000001490116), SAME));
    assert!(eq(Value::Float(0.1), Value::Float(0.100000001490116), CROSS), "a real is not a double");
    assert!(!eq(Value::Float(0.1), Value::Float(0.11), CROSS));
    assert!(eq(Value::Float(f64::NAN), Value::Float(f64::NAN), SAME));
    assert!(eq(Value::Float(2.5), dec("2.50"), CROSS));
}

#[test]
fn timestamps_ignore_trailing_zeros_and_zones_become_utc() {
    let ts = |s: &str| Value::Timestamp(s.into());
    assert!(eq(ts("2024-03-05 10:00:00.100"), ts("2024-03-05 10:00:00.1"), SAME));
    assert!(eq(ts("2024-03-05 10:00:00.000000"), ts("2024-03-05 10:00:00"), SAME));
    assert!(
        !eq(ts("2024-03-05 10:00:00.123456"), ts("2024-03-05 10:00:00.123"), SAME),
        "precision matters within a product"
    );
    assert!(
        eq(ts("2024-03-05 10:00:00.123456"), ts("2024-03-05 10:00:00.123"), CROSS),
        "the coarser precision decides between products"
    );
    assert!(!eq(ts("2024-03-05 10:00:00.124456"), ts("2024-03-05 10:00:00.123"), CROSS));
    assert!(!eq(ts("2024-03-05 10:00:01"), ts("2024-03-05 10:00:00"), CROSS));
    // 12:00 at +02:00 is 10:00 UTC.
    assert!(eq(Value::TimestampTz("2024-03-05 12:00:00+02".into()), ts("2024-03-05 10:00:00"), SAME));
    assert!(eq(
        Value::TimestampTz("2024-03-05 12:00:00+02:00".into()),
        Value::TimestampTz("2024-03-05 10:00:00+00".into()),
        SAME
    ));
    assert!(!eq(Value::TimestampTz("2024-03-05 12:00:00+00".into()), ts("2024-03-05 10:00:00"), SAME));
    assert!(eq(Value::Date("2024-03-05".into()), Value::Date("2024-03-05".into()), SAME));
    assert!(!eq(Value::Date("2024-03-05".into()), Value::Date("2024-03-06".into()), SAME));
}

#[test]
fn json_uuid_arrays_and_bytes() {
    assert!(eq(
        Value::Json("{\"a\": 1, \"b\": [1, 2]}".into()),
        Value::Json("{\"b\":[1,2],\"a\":1}".into()),
        SAME
    ));
    assert!(!eq(Value::Json("{\"a\":1}".into()), Value::Json("{\"a\":2}".into()), SAME));
    assert!(
        eq(Value::Json("{\"a\":1}".into()), t("{ \"a\" : 1 }"), SAME),
        "json against text that is the same json"
    );
    assert!(eq(
        Value::Uuid("ABCDEF00-0000-0000-0000-000000000001".into()),
        t("abcdef00-0000-0000-0000-000000000001"),
        SAME
    ));
    assert!(eq(
        Value::Array(vec![Value::Int(1), Value::Null]),
        Value::Array(vec![Value::Int(1), Value::Null]),
        SAME
    ));
    assert!(!eq(Value::Array(vec![Value::Int(1)]), Value::Array(vec![Value::Int(1), Value::Int(2)]), SAME));
    assert!(eq(Value::Bytes(vec![1, 2]), Value::Bytes(vec![1, 2]), SAME));
    assert!(!eq(Value::Bytes(vec![1, 2]), Value::Bytes(vec![1, 3]), SAME));
    assert!(!eq(t("a"), t("A"), CROSS), "text is case sensitive");
    assert!(!eq(t("a "), t("a"), CROSS), "and trailing space counts");
}

#[test]
fn key_order_is_numeric_for_numbers_bytewise_for_text_and_chronological_for_times() {
    use std::cmp::Ordering::*;
    assert_eq!(
        cmp_value(&Value::Int(9), &Value::Int(10), KeyClass::Number),
        Less,
        "9 before 10, not '10' before '9'"
    );
    assert_eq!(cmp_value(&dec("2.5"), &Value::Int(2), KeyClass::Number), Greater);
    assert_eq!(cmp_value(&dec("-3"), &dec("-20"), KeyClass::Number), Greater);
    assert_eq!(cmp_value(&dec("1.50"), &dec("1.5"), KeyClass::Number), Equal);
    assert_eq!(
        cmp_value(&t("B"), &t("a"), KeyClass::Text),
        Less,
        "upper case sorts before lower case in bytes"
    );
    assert_eq!(cmp_value(&t("é"), &t("z"), KeyClass::Text), Greater);
    let ts = |s: &str| Value::Timestamp(s.into());
    assert_eq!(cmp_value(&ts("2024-01-01 10:00:00"), &ts("2024-01-01 10:00:00.5"), KeyClass::Time), Less);
    assert_eq!(cmp_value(&ts("2024-01-01 10:00:00.50"), &ts("2024-01-01 10:00:00.5"), KeyClass::Time), Equal);
    assert_eq!(cmp_value(&Value::Bytes(vec![1]), &Value::Bytes(vec![1, 0]), KeyClass::Bytes), Less);
}

#[test]
fn a_key_must_be_the_same_kind_of_value_on_both_sides() {
    assert_eq!(key_class(TypeKind::Integer, TypeKind::Decimal), Ok(KeyClass::Number));
    assert_eq!(key_class(TypeKind::Text, TypeKind::Uuid), Ok(KeyClass::Text));
    assert!(key_class(TypeKind::Integer, TypeKind::Text).is_err());
    assert!(key_class(TypeKind::Json, TypeKind::Json).is_err());
    assert!(key_class(TypeKind::Bool, TypeKind::Bool).is_err(), "a boolean is not a key");
}

// ------------------------------------------------------------------------------------------ mapping

fn col(name: &str, kind: TypeKind) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        data_type: format!("{kind:?}"),
        kind,
        nullable: true,
        ..Default::default()
    }
}

fn table(cols: &[(&str, TypeKind)], pk: &[&str]) -> Table {
    Table {
        name: "t".into(),
        columns: cols.iter().map(|(n, k)| col(n, *k)).collect(),
        primary_key: pk.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

use TypeKind::{Integer as I, Text as T};

#[test]
fn columns_are_matched_by_name_ignoring_case_and_the_rest_is_listed() {
    let l = table(&[("id", I), ("Name", T), ("only_l", T)], &["id"]);
    let r = table(&[("ID", I), ("name", T), ("only_r", T)], &["ID"]);
    let m = map_columns(&l, &r, &[]).unwrap();
    assert_eq!(m.left_names(), ["id", "Name"]);
    assert_eq!(m.right_names(), ["ID", "name"]);
    assert_eq!(m.left_only, ["only_l"]);
    assert_eq!(m.right_only, ["only_r"]);
    assert_eq!(m.key, [0]);
    assert_eq!(m.key_right(), ["ID"]);
}

#[test]
fn no_key_is_refused_with_a_sentence() {
    let l = table(&[("id", I)], &[]);
    let r = table(&[("id", I)], &["id"]);
    let e = map_columns(&l, &r, &[]).unwrap_err();
    assert!(e.contains("no primary key") && e.contains("Choose the key columns"), "{e}");
    assert!(map_columns(&l, &r, &["id".into()]).is_ok(), "the user can choose one");
    let e = map_columns(&l, &r, &["nope".into()]).unwrap_err();
    assert!(e.contains("not a column of both tables"), "{e}");
}

#[test]
fn a_key_of_different_kinds_is_refused_and_generated_columns_are_skipped() {
    let l = table(&[("id", I), ("g", T)], &["id"]);
    let mut r = table(&[("id", T), ("g", T)], &["id"]);
    assert!(map_columns(&l, &r, &[]).unwrap_err().contains("different kind of value"));
    r.columns[0].kind = I;
    r.columns[1].generated = Some(theta_core::GeneratedColumn { expression: "x".into(), stored: true });
    let m = map_columns(&l, &r, &[]).unwrap();
    assert_eq!(m.generated, ["g"]);
    assert_eq!(m.columns.len(), 1);
}

// ------------------------------------------------------------------------------------------ merging

fn mapping2() -> Mapping {
    let l = table(&[("id", I), ("v", T)], &["id"]);
    map_columns(&l, &l, &[]).unwrap()
}

fn row(id: i64, v: &str) -> Vec<Value> {
    vec![Value::Int(id), t(v)]
}

fn run_pages(
    m: Mapping,
    left: &[Vec<Value>],
    right: &[Vec<Value>],
    page: usize,
    cap: usize,
) -> Result<Comparer, MergeError> {
    let mut c = Comparer::new(m, SAME, cap);
    let (mut l, mut r): (VecDeque<Vec<Value>>, VecDeque<Vec<Value>>) = (VecDeque::new(), VecDeque::new());
    let (mut li, mut ri) = (0, 0);
    let (mut ld, mut rd) = (false, false);
    loop {
        match c.feed(&mut l, ld, &mut r, rd)? {
            Need::Done => return Ok(c),
            Need::Left => {
                let end = (li + page).min(left.len());
                l.extend(left[li..end].iter().cloned());
                ld = end == left.len();
                li = end;
            }
            Need::Right => {
                let end = (ri + page).min(right.len());
                r.extend(right[ri..end].iter().cloned());
                rd = end == right.len();
                ri = end;
            }
        }
    }
}

#[test]
fn rows_are_classified_as_only_left_only_right_different_and_same() {
    let left = vec![row(1, "a"), row(2, "b"), row(3, "c"), row(5, "e")];
    let right = vec![row(2, "b"), row(3, "X"), row(4, "d"), row(5, "e"), row(6, "f")];
    let c = run_pages(mapping2(), &left, &right, 100, 100).unwrap();
    assert_eq!(c.counts, Counts { only_left: 1, only_right: 2, different: 1, same: 2 });
    let kinds: Vec<(DiffKind, i64)> =
        c.rows.iter().map(|r| (r.kind, if let Value::Int(i) = r.key[0] { i } else { 0 })).collect();
    assert_eq!(
        kinds,
        [
            (DiffKind::OnlyLeft, 1),
            (DiffKind::Different, 3),
            (DiffKind::OnlyRight, 4),
            (DiffKind::OnlyRight, 6)
        ]
    );
    assert_eq!(c.rows[1].changed, [1], "only the column that differs");
    assert!(!c.capped);
}

#[test]
fn the_page_size_never_changes_the_answer() {
    let left: Vec<_> =
        (0..200).filter(|i| i % 7 != 0).map(|i| row(i, if i % 11 == 0 { "changed" } else { "v" })).collect();
    let right: Vec<_> = (0..200).filter(|i| i % 5 != 0).map(|i| row(i, "v")).collect();
    let whole = run_pages(mapping2(), &left, &right, 1000, 10_000).unwrap();
    for page in [1, 2, 3, 7, 64, 199] {
        let c = run_pages(mapping2(), &left, &right, page, 10_000).unwrap();
        assert_eq!(c.counts, whole.counts, "page {page}");
        assert_eq!(c.rows, whole.rows, "page {page}");
    }
    assert!(
        whole.counts.only_left > 0
            && whole.counts.only_right > 0
            && whole.counts.different > 0
            && whole.counts.same > 0
    );
}

#[test]
fn what_is_kept_is_capped_but_the_counts_stay_exact() {
    let left: Vec<_> = (0..50).map(|i| row(i, "a")).collect();
    let right: Vec<_> = (0..50).map(|i| row(i, "b")).collect();
    let c = run_pages(mapping2(), &left, &right, 8, 10).unwrap();
    assert_eq!(c.counts.different, 50);
    assert_eq!(c.rows.len(), 10);
    assert!(c.capped);
    assert_eq!(c.rows[9].key, vec![Value::Int(9)], "the first ones in key order are kept");
    let c = run_pages(mapping2(), &left, &right, 8, 50).unwrap();
    assert!(!c.capped, "exactly the cap is not capped");
}

#[test]
fn empty_tables() {
    let c = run_pages(mapping2(), &[], &[], 10, 10).unwrap();
    assert_eq!(c.counts, Counts::default());
    let c = run_pages(mapping2(), &[row(1, "a")], &[], 10, 10).unwrap();
    assert_eq!(c.counts.only_left, 1);
    let c = run_pages(mapping2(), &[], &[row(1, "a")], 10, 10).unwrap();
    assert_eq!(c.counts.only_right, 1);
}

#[test]
fn a_stream_that_is_out_of_order_is_detected_not_merged() {
    // Text keys the way a case-insensitive server sends them: a, B, c.
    let l = table(&[("k", T), ("v", T)], &["k"]);
    let m = map_columns(&l, &l, &[]).unwrap();
    let ci = vec![vec![t("a"), t("1")], vec![t("B"), t("2")], vec![t("c"), t("3")]];
    let err = run_pages(m.clone(), &ci, &ci, 10, 10).err().expect("an error");
    assert!(matches!(err, MergeError::Unsorted { side: Side::Left, .. }), "{err:?}");
    assert!(err.describe("source", "target").contains("sort this key differently"));
    // In byte order the same keys merge.
    let bytes = vec![vec![t("B"), t("2")], vec![t("a"), t("1")], vec![t("c"), t("3")]];
    let c = run_pages(m, &bytes, &bytes, 10, 10).unwrap();
    assert_eq!(c.counts.same, 3);
}

#[test]
fn without_the_check_two_orders_would_give_a_wrong_answer() {
    // The proof that the check is needed: a naive merge of case-insensitive order (left) against byte order
    // (right) pairs the wrong rows. With the check it is an error instead.
    let l = table(&[("k", T)], &["k"]);
    let m = map_columns(&l, &l, &[]).unwrap();
    let ci = vec![vec![t("a")], vec![t("B")], vec![t("c")]];
    let bytes = vec![vec![t("B")], vec![t("a")], vec![t("c")]];
    // Naive: compare heads, advance the smaller by bytes.
    let (mut i, mut j, mut same) = (0, 0, 0);
    while i < ci.len() && j < bytes.len() {
        match cmp_keys(&ci[i], &bytes[j], &m.classes) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                same += 1;
                i += 1;
                j += 1;
            }
        }
    }
    assert_eq!(same, 2, "the tables hold the same three keys, the naive merge finds two");
    assert!(run_pages(m, &ci, &bytes, 10, 10).is_err());
}

#[test]
fn duplicate_and_null_keys_are_refused() {
    let dup = vec![row(1, "a"), row(1, "b")];
    let e = run_pages(mapping2(), &dup, &[], 10, 10).err().unwrap();
    assert!(matches!(e, MergeError::Duplicate { side: Side::Left, .. }), "{e:?}");
    let e = run_pages(mapping2(), &[], &dup, 1, 10).err().unwrap();
    assert!(matches!(e, MergeError::Duplicate { side: Side::Right, .. }), "{e:?}");
    let null = vec![vec![Value::Null, t("a")]];
    let e = run_pages(mapping2(), &null, &[], 10, 10).err().unwrap();
    assert_eq!(e, MergeError::NullKey { side: Side::Left });
    assert!(e.describe("source", "target").contains("NULL"));
}

#[test]
fn a_composite_key_orders_by_each_column_in_turn() {
    let l = table(&[("a", I), ("b", T), ("v", T)], &["a", "b"]);
    let m = map_columns(&l, &l, &[]).unwrap();
    let r = |a: i64, b: &str, v: &str| vec![Value::Int(a), t(b), t(v)];
    let left = vec![r(1, "x", "1"), r(1, "y", "1"), r(2, "x", "1")];
    let right = vec![r(1, "y", "2"), r(2, "x", "1"), r(2, "y", "1")];
    let c = run_pages(m, &left, &right, 2, 10).unwrap();
    assert_eq!(c.counts, Counts { only_left: 1, only_right: 1, different: 1, same: 1 });
}

// ------------------------------------------------------------------------------------------ the script

fn diffs() -> (Mapping, Vec<DiffRow>) {
    let m = mapping2();
    let left = vec![row(1, "new"), row(2, "changed"), row(3, "same")];
    let right = vec![row(2, "old"), row(3, "same"), row(9, "extra")];
    let c = run_pages(m.clone(), &left, &right, 10, 10).unwrap();
    (m, c.rows)
}

#[test]
fn the_script_deletes_updates_then_inserts_and_comments_the_deletes_out() {
    let (m, rows) = diffs();
    let kind = DatabaseKind::Postgres;
    let s = sync_script(&m, &rows, kind, "public", "t", false);
    let sql: Vec<String> =
        s.statements.iter().map(|x| format!("{}{}", if x.commented { "#" } else { "" }, x.sql)).collect();
    assert_eq!(sql[0], "#DELETE FROM public.t WHERE id = 9");
    assert_eq!(sql[1], "UPDATE public.t SET v = 'changed' WHERE id = 2");
    assert_eq!(sql[2], "INSERT INTO public.t (id, v) VALUES\n    (1, 'new')");
    let text = s.render("a.t", "b.t", false);
    assert!(text.contains("1 DELETE is commented out"), "{text}");
    assert!(text.contains("-- destructive: delete a row that exists only in the target\n-- DELETE FROM public.t WHERE id = 9\n-- ;\n"), "{text}");
    let live = sync_script(&m, &rows, kind, "public", "t", true);
    assert!(live.statements.iter().all(|x| !x.commented));
    assert!(live.render("a", "b", true).contains("covers those"));
}

#[test]
fn the_where_clause_uses_the_targets_key_and_is_null_for_null_parts() {
    let l = table(&[("a", I), ("b", T), ("v", T)], &["a", "b"]);
    let m = map_columns(&l, &l, &[]).unwrap();
    let rows = vec![DiffRow {
        kind: DiffKind::Different,
        key: vec![Value::Int(1), t("it's")],
        left: Some(vec![Value::Int(1), t("it's"), t("new")]),
        right: Some(vec![Value::Int(1), t("it's"), Value::Null]),
        changed: vec![2],
    }];
    let s = sync_script(&m, &rows, DatabaseKind::Sqlite, "main", "t", false);
    assert_eq!(s.statements[0].sql, "UPDATE main.t SET v = 'new' WHERE a = 1 AND b = 'it''s'");
}

#[test]
fn inserts_are_batched_and_use_the_targets_column_names() {
    let l = table(&[("id", I), ("Name", T)], &["id"]);
    let r = table(&[("ID", I), ("name", T)], &["ID"]);
    let m = map_columns(&l, &r, &[]).unwrap();
    let rows: Vec<DiffRow> = (0..250)
        .map(|i| DiffRow {
            kind: DiffKind::OnlyLeft,
            key: vec![Value::Int(i)],
            left: Some(vec![Value::Int(i), t("x")]),
            right: None,
            changed: vec![],
        })
        .collect();
    let s = sync_script(&m, &rows, DatabaseKind::MySql, "db", "t", false);
    assert_eq!(s.statements.iter().map(|x| x.rows).collect::<Vec<_>>(), [100, 100, 50]);
    assert!(
        s.statements[0].sql.starts_with("INSERT INTO db.t (ID, name) VALUES\n    (0, 'x'),"),
        "{}",
        s.statements[0].sql
    );
}

#[test]
fn values_are_converted_for_the_target_column() {
    let mut l = table(&[("id", I), ("flag", TypeKind::Integer), ("at", TypeKind::Timestamp)], &["id"]);
    l.columns[1].data_type = "tinyint(1)".into();
    let mut r = l.clone();
    r.columns[1] = ColumnInfo {
        name: "flag".into(),
        data_type: "boolean".into(),
        kind: TypeKind::Bool,
        ..Default::default()
    };
    let m = map_columns(&l, &r, &[]).unwrap();
    let rows = vec![DiffRow {
        kind: DiffKind::OnlyLeft,
        key: vec![Value::Int(1)],
        left: Some(vec![Value::Int(1), Value::Int(1), Value::TimestampTz("2024-03-05 12:00:00+02".into())]),
        right: None,
        changed: vec![],
    }];
    let pg = sync_script(&m, &rows, DatabaseKind::Postgres, "s", "t", false);
    assert!(pg.statements[0].sql.contains("(1, TRUE,"), "{}", pg.statements[0].sql);
    let my = sync_script(&m, &rows, DatabaseKind::MySql, "s", "t", false);
    assert!(
        my.statements[0].sql.contains("'2024-03-05 10:00:00'"),
        "an offset becomes UTC wall time for MySQL: {}",
        my.statements[0].sql
    );
}

#[test]
fn an_always_identity_target_column_gets_overriding_system_value() {
    let l = table(&[("id", I), ("v", T)], &["id"]);
    let mut r = l.clone();
    r.columns[0].identity = Some(Identity::Always);
    let m = map_columns(&l, &r, &[]).unwrap();
    let rows = vec![DiffRow {
        kind: DiffKind::OnlyLeft,
        key: vec![Value::Int(1)],
        left: Some(row(1, "a")),
        right: None,
        changed: vec![],
    }];
    let s = sync_script(&m, &rows, DatabaseKind::Postgres, "s", "t", false);
    assert!(s.statements[0].sql.contains(") OVERRIDING SYSTEM VALUE VALUES"), "{}", s.statements[0].sql);
    let s = sync_script(&m, &rows, DatabaseKind::MySql, "s", "t", false);
    assert!(!s.statements[0].sql.contains("OVERRIDING"));
}

#[test]
fn nothing_to_do_says_so() {
    let (m, _) = diffs();
    let s = sync_script(&m, &[], DatabaseKind::Postgres, "s", "t", false);
    assert!(s.statements.is_empty());
    assert!(s.render("a", "b", false).contains("Nothing to do"));
}
