//! Quick fixes, definitions, usages and rename: text before and after, and the refusals.

use super::*;
use crate::analysis::analyze;
use crate::fixtures::shop;
use theta_core::DatabaseKind as K;

fn an(sql: &str) -> crate::analysis::Analysis {
    analyze(sql, K::Postgres, Some(&shop()))
}

/// Offer actions at the `|` in `sql` (removed), apply the one whose title contains `title`, return the text.
fn apply(marked: &str, title: &str) -> String {
    let at = marked.find('|').expect("a cursor marker");
    let sql = marked.replace('|', "");
    let a = an(&sql);
    let fixes = code_actions(&sql, K::Postgres, Some(&shop()), &a, at);
    let fix = fixes.iter().find(|f| f.title.contains(title)).unwrap_or_else(|| {
        panic!(
            "no action {title:?} at the cursor of {sql:?}; offered: {:?}",
            fixes.iter().map(|f| &f.title).collect::<Vec<_>>()
        )
    });
    let out = apply_edits(&sql, &fix.edits);
    // an action never makes the statement worse: it must still parse without errors
    let again = an(&out);
    assert!(
        again.parse.errors.is_empty(),
        "{title:?} produced a syntax error: {out:?}: {:?}",
        again.parse.errors
    );
    out
}

fn titles(marked: &str) -> Vec<String> {
    let at = marked.find('|').unwrap();
    let sql = marked.replace('|', "");
    let a = an(&sql);
    code_actions(&sql, K::Postgres, Some(&shop()), &a, at).into_iter().map(|f| f.title).collect()
}

#[test]
fn did_you_mean_for_columns_tables_and_aliases() {
    assert_eq!(
        apply("select emi|al from customers", "Change 'emial' to 'email'"),
        "select email from customers"
    );
    assert_eq!(
        apply("select * from custo|mers2", "Change 'customers2' to 'customers'"),
        "select * from customers"
    );
    assert_eq!(apply("select * from custmers|", "'customers'"), "select * from customers");
    assert_eq!(
        apply("select c.emial| from customers c", "Change 'emial' to 'email'"),
        "select c.email from customers c"
    );
    assert_eq!(apply("select c|c.id from customers c", "Change 'cc' to 'c'"), "select c.id from customers c");
    // the fix is offered only at the problem, and only when something is close
    assert!(
        titles("select |id from customers").is_empty()
            || !titles("select |id from customers").iter().any(|t| t.starts_with("Change"))
    );
    assert!(!titles("select zzzzzz| from customers").iter().any(|t| t.starts_with("Change")));
    // a name that needs quoting is quoted
    let a = an("select nme from customers");
    assert!(a.diagnostics.len() == 1);
}

#[test]
fn fixes_for_the_statement_are_offered_from_anywhere_in_it() {
    let marked = "select emial from customers where id > 1|";
    assert!(titles(marked).iter().any(|t| t == "Change 'emial' to 'email'"), "{:?}", titles(marked));
    // but when the cursor is on a problem, only that problem's fixes come first
    let two = "select emial, naem| from customers";
    let t = titles(two);
    assert_eq!(t[0], "Change 'naem' to 'name'", "{t:?}");
    assert!(!t.iter().any(|x| x.contains("'emial'")), "{t:?}");
}

#[test]
fn qualify_an_ambiguous_column() {
    let sql = "select i|d from customers c join orders o on o.customer_id = c.id";
    assert_eq!(
        apply(sql, "Qualify as 'c.id'"),
        "select c.id from customers c join orders o on o.customer_id = c.id"
    );
    assert_eq!(
        apply(sql, "Qualify as 'o.id'"),
        "select o.id from customers c join orders o on o.customer_id = c.id"
    );
}

#[test]
fn expand_star() {
    assert_eq!(apply("select |* from stores", "Expand *"), "select id, name, location from stores");
    assert_eq!(
        apply("select |* from customers c join stores s on s.id = c.id", "Expand *"),
        "select c.id, c.email, c.name, c.tier, c.created_at, s.id, s.name, s.location from customers c join stores s on s.id = c.id"
    );
    assert_eq!(
        apply("select s.|* , c.id from customers c join stores s on s.id = c.id", "Expand *"),
        "select s.id, s.name, s.location , c.id from customers c join stores s on s.id = c.id"
    );
    // not offered over an unknown source
    assert!(!titles("select |* from generate_series(1, 3)").iter().any(|t| t.contains("Expand")));
    // a name that needs quoting is quoted
    let sql = "with t as (select 1 as \"Weird Name\") select |* from t";
    assert_eq!(apply(sql, "Expand *"), "with t as (select 1 as \"Weird Name\") select \"Weird Name\" from t");
}

#[test]
fn add_alias_and_qualify() {
    assert_eq!(
        apply("select id, email from custom|ers where tier = 'x'", "Add alias 'c'"),
        "select c.id, c.email from customers c where c.tier = 'x'"
    );
    assert_eq!(apply("select customers.id from custom|ers", "Add alias 'c'"), "select c.id from customers c");
    // names already in use are avoided; qualifiers inside subqueries that reach the table follow
    assert_eq!(
        apply(
            "select 1 from customers c join orders| on orders.customer_id = c.id where exists (select 1 from reviews r where r.customer_id = c.id and orders.id = r.id)",
            "Add alias 'o'"
        ),
        "select 1 from customers c join orders o on o.customer_id = c.id where exists (select 1 from reviews r where r.customer_id = c.id and o.id = r.id)"
    );
    // an inner table of the same name hides the outer one: its references stay
    assert_eq!(
        apply("select id from custom|ers where id in (select id from customers)", "Add alias 'c'"),
        "select c.id from customers c where c.id in (select id from customers)"
    );
    // multi-word names make initials
    assert!(titles("select order_id from order_|items").iter().any(|t| t.contains("alias 'oi'")));
    // already aliased: nothing to add
    assert!(!titles("select 1 from custom|ers c").iter().any(|t| t.starts_with("Add alias")));
}

#[test]
fn group_by_columns_are_added() {
    assert_eq!(
        apply("select status, to|tal from orders group by status", "to GROUP BY"),
        "select status, total from orders group by status, total"
    );
    assert_eq!(
        apply("select status, to|tal, count(*) from orders", "to GROUP BY"),
        "select status, total, count(*) from orders GROUP BY status, total"
    );
}

#[test]
fn comma_joins_become_explicit() {
    assert_eq!(
        apply("select * from customers c, or|ders o where o.customer_id = c.id", "Convert the comma join"),
        "select * from customers c\nJOIN orders o ON o.customer_id = c.id"
    );
    assert_eq!(
        apply(
            "select * from customers c, orders| o where o.customer_id = c.id and c.tier = 'gold' and o.total > 5",
            "Convert the comma join"
        )
        .replace('\n', " "),
        "select * from customers c JOIN orders o ON o.customer_id = c.id WHERE c.tier = 'gold' AND o.total > 5"
    );
    // three tables: each condition goes to the join where its last table appears
    assert_eq!(
        apply(
            "select * from customers c, orders o, order_items| i where o.customer_id = c.id and i.order_id = o.id",
            "Convert the comma join"
        )
        .replace('\n', " "),
        "select * from customers c JOIN orders o ON o.customer_id = c.id JOIN order_items i ON i.order_id = o.id"
    );
    // no predicate: a cross join, said out loud
    assert_eq!(
        apply("select * from customers c, st|ores s", "Convert the comma join").replace('\n', " "),
        "select * from customers c CROSS JOIN stores s"
    );
}

#[test]
fn join_sides_are_flipped() {
    assert_eq!(
        apply("select * from customers c left jo|in orders o on o.customer_id = c.id", "Flip the sides"),
        "select * from orders o right join customers c on o.customer_id = c.id"
    );
    assert_eq!(
        apply("select * from customers c RIGHT J|OIN orders o ON o.customer_id = c.id", "Flip the sides"),
        "select * from orders o LEFT JOIN customers c ON o.customer_id = c.id"
    );
    assert_eq!(
        apply("select * from customers c j|oin orders o on true", "Flip"),
        "select * from orders o join customers c on true"
    );
    // three tables: not offered
    assert!(!titles("select * from a j|oin b on true join c on true").iter().any(|t| t.contains("Flip")));
}

#[test]
fn ctes_are_introduced_and_inlined() {
    assert_eq!(
        apply("select s.n from (select count(*) as n from orders) |s", "Extract the subquery 's'"),
        "WITH s AS (select count(*) as n from orders)\nselect s.n from s"
    );
    assert_eq!(
        apply("with a as (select 1 as x) select * from (select x from a) |b", "Extract the subquery 'b'"),
        "with a as (select 1 as x),\nb AS (select x from a) select * from b"
    );
    // correlated: not offered
    assert!(
        !titles("select * from customers c where exists (select 1 from (select 1 where c.id = 1) |s)")
            .iter()
            .any(|t| t.starts_with("Extract"))
    );
    assert_eq!(
        apply(
            "with big as (select id from orders where total > 5) select * from |big b",
            "Inline the CTE 'big'"
        ),
        "select * from (select id from orders where total > 5) b"
    );
    assert_eq!(
        apply("with big as (select id from orders) select * from |big", "Inline the CTE 'big'"),
        "select * from (select id from orders) big"
    );
    assert_eq!(
        apply("with a as (select 1 as x), |b as (select 2 as y) select * from a, b", "Inline the CTE 'b'"),
        "with a as (select 1 as x) select * from a, (select 2 as y) b"
    );
    // referenced twice: not offered
    assert!(
        !titles("with big as (select id from orders) select * from |big b1, big b2")
            .iter()
            .any(|t| t.starts_with("Inline"))
    );
}

#[test]
fn keyword_case_follows_the_tree_not_the_spelling() {
    // `name` and `status` are columns here, not keywords, and keep their case
    assert_eq!(
        apply("sel|ect Name, status from customers c where c.id = 1 order by 1", "Uppercase the keywords"),
        "SELECT Name, status FROM customers c WHERE c.id = 1 ORDER BY 1"
    );
    assert_eq!(
        apply("SEL|ECT Name FROM customers AS C WHERE C.id = 1", "Lowercase the keywords"),
        "select Name from customers as C where C.id = 1"
    );
    // already uppercase: no uppercase action
    assert!(!titles("SEL|ECT 1 FROM customers").iter().any(|t| t.starts_with("Uppercase")));
    // an alias that is a keyword-looking word is an identifier
    assert_eq!(
        apply("select 1 as |level from customers", "Uppercase the keywords"),
        "SELECT 1 AS level FROM customers"
    );
}

// ---- navigation

fn def(sql: &str, needle: &str, nth: usize) -> Option<Definition> {
    let a = an(sql);
    // the cursor goes on the last identifier of the needle (`b.amount` -> `amount`)
    let start = sql.match_indices(needle).nth(nth).unwrap().0;
    let at = start + needle.rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + 1) + 1;
    definition_at(&a, at)
}

#[test]
fn definition_of_aliases_ctes_and_derived_columns() {
    let sql =
        "with big as (select id, total as amount from orders) select b.amount from big b where b.id > 1";
    // alias use -> where the alias is introduced
    let a = an(sql);
    let Some(Definition::Here(r)) = definition_at(&a, sql.find("b.amount").unwrap()) else { panic!() };
    assert_eq!(&sql[r.clone()], "b");
    assert_eq!(r.start, sql.rfind("big b").unwrap() + 4, "the alias in the FROM clause");
    // CTE reference -> CTE name
    let Some(Definition::Here(r)) = def(sql, "big", 1) else { panic!() };
    assert_eq!(&sql[r.clone()], "big");
    assert_eq!(r.start, sql.find("big").unwrap());
    // column of a CTE -> the select item alias
    let Some(Definition::Here(r)) = def(sql, "b.amount", 0) else { panic!() };
    assert_eq!(&sql[r.clone()], "amount");
    assert_eq!(r.start, sql.find("as amount").unwrap() + 3);
    // passthrough column -> the select item
    let Some(Definition::Here(r)) = def(sql, "b.id", 0) else { panic!() };
    assert_eq!(&sql[r], "id");
    // catalog table and column
    let sql = "select o.total from orders o";
    assert!(matches!(def(sql, "orders", 0), Some(Definition::Table(t)) if t.name == "orders"));
    assert!(
        matches!(def(sql, "o.total", 0), Some(Definition::Column { table, column }) if table.name == "orders" && column == "total")
    );
    // an unresolved name has no definition
    assert_eq!(def("select nope from orders", "nope", 0), None);
}

#[test]
fn usages_and_highlights() {
    let sql = "select o.id, o.total from orders o join customers c on c.id = o.customer_id where o.total > 1";
    let a = an(sql);
    let at = sql.find("o.id").unwrap();
    let h = highlights_at(&a, at);
    // the alias: declaration + 4 qualifier uses
    assert_eq!(h.len(), 5, "{:?}", h.iter().map(|r| &sql[r.clone()]).collect::<Vec<_>>());
    // the column orders.total: twice
    let at = sql.find("total").unwrap() + 1;
    assert_eq!(highlights_at(&a, at).len(), 2);
    // a catalog table across the statement
    let (target, _) = target_at(&a, sql.find("orders").unwrap()).unwrap();
    assert_eq!(usages_of(&a, &target).len(), 1);
    // nothing under the cursor on a keyword
    assert!(highlights_at(&a, 1).is_empty());
}

#[test]
fn rename_changes_every_reference_at_once() {
    let sql = "select o.id, o.total from orders o join customers c on c.id = o.customer_id where o.total > 1";
    let a = an(sql);
    let edits = rename_at(&a, sql, K::Postgres, sql.find("o.id").unwrap(), "ord").unwrap();
    assert_eq!(
        apply_edits(sql, &edits),
        "select ord.id, ord.total from orders ord join customers c on c.id = ord.customer_id where ord.total > 1"
    );
    // a CTE and its columns
    let sql = "with big as (select id, total as amount from orders) select b.amount, amount from big b order by amount";
    let a = an(sql);
    let e = rename_at(&a, sql, K::Postgres, sql.find("big").unwrap(), "large").unwrap();
    assert_eq!(
        apply_edits(sql, &e),
        "with large as (select id, total as amount from orders) select b.amount, amount from large b order by amount"
    );
    let e = rename_at(&a, sql, K::Postgres, sql.find("as amount").unwrap() + 4, "sum_total").unwrap();
    assert_eq!(
        apply_edits(sql, &e),
        "with big as (select id, total as sum_total from orders) select b.sum_total, sum_total from big b order by sum_total"
    );
    // a select-list alias used by ORDER BY
    let sql = "select total * 2 as dbl from orders order by dbl";
    let a = an(sql);
    let e = rename_at(&a, sql, K::Postgres, sql.find("dbl").unwrap(), "double_total").unwrap();
    assert_eq!(apply_edits(sql, &e), "select total * 2 as double_total from orders order by double_total");
    // quoting: a name that needs it is quoted everywhere
    let e = rename_at(&a, sql, K::Postgres, sql.find("dbl").unwrap(), "\"Double Total\"").unwrap();
    assert_eq!(
        apply_edits(sql, &e),
        "select total * 2 as \"Double Total\" from orders order by \"Double Total\""
    );
    // a mixed-case name is quoted so that PostgreSQL does not fold it
    let e = rename_at(&a, sql, K::Postgres, sql.find("dbl").unwrap(), "DoubleTotal").unwrap();
    assert_eq!(
        apply_edits(sql, &e),
        "select total * 2 as \"DoubleTotal\" from orders order by \"DoubleTotal\""
    );
    let e = rename_at(&a, sql, K::Postgres, sql.find("dbl").unwrap(), "\"x y\"").unwrap();
    assert_eq!(apply_edits(sql, &e), "select total * 2 as \"x y\" from orders order by \"x y\"");
}

#[test]
fn rename_refuses_with_a_reason() {
    let sql = "select o.id, c.id from orders o join customers c on c.id = o.customer_id";
    let a = an(sql);
    let at = sql.find("o.id").unwrap();
    let err = |name: &str| rename_at(&a, sql, K::Postgres, at, name).unwrap_err();
    assert!(err("c").contains("already used"), "{}", err("c"));
    assert!(err("").contains("empty"));
    assert!(err("o").contains("unchanged"));
    assert!(err("a b").contains("not a plain identifier"));
    // catalog objects are out of scope and the message says where to go
    let e = rename_at(&a, sql, K::Postgres, sql.find("orders").unwrap(), "x").unwrap_err();
    assert!(e.contains("table editor"), "{e}");
    let e = rename_at(&a, sql, K::Postgres, sql.find("id").unwrap(), "x").unwrap_err();
    assert!(e.contains("table editor"), "{e}");
    // a CTE must not collide with another CTE or hide a table the statement uses
    let sql = "with a as (select 1 as x), b as (select 2 as y) select * from a, b, orders";
    let a = an(sql);
    let e = rename_at(&a, sql, K::Postgres, sql.find("a as").unwrap(), "b").unwrap_err();
    assert!(e.contains("already a CTE"), "{e}");
    let e = rename_at(&a, sql, K::Postgres, sql.find("a as").unwrap(), "orders").unwrap_err();
    assert!(e.contains("would hide"), "{e}");
    // an output column must not collide with a column that ORDER BY refers to unqualified
    let sql = "select upper(email) as e from customers order by id";
    let a = an(sql);
    let e = rename_at(&a, sql, K::Postgres, sql.find(" e ").unwrap() + 1, "id").unwrap_err();
    assert!(e.contains("ORDER BY"), "{e}");
    // but a harmless collision with a table column is fine
    assert!(rename_at(&a, sql, K::Postgres, sql.find(" e ").unwrap() + 1, "name").is_ok());
    // a passthrough column is not renamable
    let sql = "with t as (select id from orders) select id from t";
    let a = an(sql);
    let e = rename_at(&a, sql, K::Postgres, sql.rfind("id").unwrap(), "x").unwrap_err();
    assert!(e.contains("passes through"), "{e}");
    // an inner alias would capture a reference to the outer one
    let sql = "select o.id from orders o where exists (select 1 from customers c where c.id = o.customer_id)";
    let a = an(sql);
    let e = rename_at(&a, sql, K::Postgres, sql.find("o.id").unwrap(), "c").unwrap_err();
    assert!(e.contains("captured") || e.contains("already used"), "{e}");
}

#[test]
fn edits_never_overlap_and_apply_in_any_order() {
    let sql = "select a from t";
    let edits = vec![Edit::insert(7, "x."), Edit::replace(14..15, "u"), Edit::insert(0, "-- ")];
    assert_eq!(apply_edits(sql, &edits), "-- select x.a from u");
    let reversed: Vec<Edit> = edits.into_iter().rev().collect();
    assert_eq!(apply_edits(sql, &reversed), "-- select x.a from u");
}
