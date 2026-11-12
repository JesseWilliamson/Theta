//! Resolution tests against the `shop` fixture catalog (customers, orders, order_items, products, stores,
//! reviews, categories, the view active_customers in `public`; events and daily_sales in `analytics`).

use super::*;
use crate::fixtures::shop;
use theta_core::DatabaseKind as K;

fn run(sql: &str, kind: K) -> Analysis {
    analyze(sql, kind, Some(&shop()))
}

fn pg(sql: &str) -> Analysis {
    run(sql, K::Postgres)
}

fn msgs(a: &Analysis) -> Vec<String> {
    a.diagnostics.iter().map(|d| d.message.clone()).collect()
}

fn clean(sql: &str, kind: K) {
    let a = run(sql, kind);
    assert!(a.diagnostics.is_empty(), "unexpected diagnostics for {sql:?}: {:?}", msgs(&a));
}

/// The nth reference whose text is `name`.
fn nth_ref<'a>(a: &'a Analysis, sql: &str, name: &str, n: usize) -> &'a Ref {
    a.refs.iter().filter(|r| &sql[r.span.clone()] == name).nth(n).unwrap_or_else(|| {
        panic!(
            "no reference #{n} to {name:?} in {sql:?}: {:?}",
            a.refs.iter().map(|r| &sql[r.span.clone()]).collect::<Vec<_>>()
        )
    })
}

fn column_of(r: &Ref) -> (&str, &str) {
    match &r.resolution {
        Resolution::Column { table, column } => (table.name.as_str(), column.as_str()),
        other => panic!("expected a catalog column, got {other:?}"),
    }
}

#[test]
fn tables_and_columns_resolve_against_the_catalog() {
    let sql = "select c.id, name from customers c";
    let a = pg(sql);
    assert!(a.diagnostics.is_empty());
    assert_eq!(column_of(nth_ref(&a, sql, "id", 0)), ("customers", "id"));
    assert_eq!(column_of(nth_ref(&a, sql, "name", 0)), ("customers", "name"));
    assert!(
        matches!(&nth_ref(&a, sql, "customers", 0).resolution, Resolution::Table(t) if t.name == "customers" && t.schema.as_deref() == Some("public"))
    );
    // the alias resolves to its symbol, which names the alias token
    let r = nth_ref(&a, sql, "c", 0);
    assert!(matches!(r.resolution, Resolution::Symbol(_)));
}

#[test]
fn unknown_table_and_column_are_reported_with_context() {
    let a = pg("select id from custmers");
    assert_eq!(msgs(&a), vec!["Unknown table 'custmers'"]);
    assert!(
        matches!(&a.diagnostics[0].kind, DiagKind::UnknownTable { candidates, .. } if candidates.contains(&"customers".to_string()))
    );
    let a = pg("select emial from customers");
    assert_eq!(msgs(&a), vec!["Unknown column 'emial' (looked in customers)"]);
    let a = pg("select o.totl from orders o");
    assert_eq!(msgs(&a), vec!["Unknown column 'totl' in 'orders'"]);
    // the range is exactly the identifier
    let sql = "select emial from customers";
    let a = pg(sql);
    assert_eq!(&sql[a.diagnostics[0].range.clone()], "emial");
}

#[test]
fn an_alias_that_is_not_defined() {
    let a = pg("select x.id from customers c");
    assert_eq!(msgs(&a), vec!["'x' is not a table or alias in this query"]);
    // the unaliased table name keeps working, the aliased one does not
    clean("select customers.id from customers", K::Postgres);
    let a = pg("select customers.id from customers c");
    assert_eq!(msgs(&a), vec!["'customers' is not a table or alias in this query"]);
}

#[test]
fn ambiguous_columns_name_the_candidates() {
    let a = pg("select id from customers c join orders o on o.customer_id = c.id");
    assert_eq!(msgs(&a), vec!["Column 'id' is ambiguous (found in c, o)"]);
    clean("select c.id from customers c join orders o on o.customer_id = c.id", K::Postgres);
}

#[test]
fn using_and_natural_merge_columns() {
    clean("select customer_id from orders join reviews using (customer_id)", K::Postgres);
    // without USING it would be ambiguous
    assert_eq!(msgs(&pg("select customer_id from orders join reviews on true")).len(), 1);
    clean("select id from customers natural join active_customers", K::Postgres);
    // USING names a column the right table lacks
    let a = pg("select 1 from customers join stores using (email)");
    assert_eq!(a.diagnostics.len(), 1, "{:?}", msgs(&a));
}

#[test]
fn ctes_define_tables_and_columns() {
    let sql = "with t as (select id, total as amount from orders) select t.amount, id from t";
    let a = pg(sql);
    assert!(a.diagnostics.is_empty(), "{:?}", msgs(&a));
    let r = nth_ref(&a, sql, "amount", 1);
    let Resolution::Symbol(s) = r.resolution else { panic!("{:?}", r.resolution) };
    assert_eq!(a.symbols[s].kind, SymbolKind::OutputColumn);
    assert_eq!(&sql[a.symbols[s].span.clone()], "amount");
    assert!(a.symbols[s].renamable);
    // the CTE reference resolves to the CTE symbol
    let cte_ref = nth_ref(&a, sql, "t", 1);
    let Resolution::Symbol(c) = cte_ref.resolution else { panic!() };
    assert_eq!(a.symbols[c].kind, SymbolKind::Cte);
    // a column the CTE does not expose
    let a = pg("with t as (select id from orders) select t.total from t");
    assert_eq!(msgs(&a), vec!["Unknown column 'total' in 't'"]);
    // explicit column list
    clean("with t (a, b) as (select id, total from orders) select a, b from t", K::Postgres);
    assert_eq!(msgs(&pg("with t (a, b) as (select id, total from orders) select id from t")).len(), 1);
}

#[test]
fn recursive_ctes_see_themselves_and_plain_ones_do_not() {
    clean(
        "with recursive r(n) as (select 1 union all select n + 1 from r where n < 5) select n from r",
        K::Postgres,
    );
    // a non-recursive CTE named like a table: the inner reference is the catalog table
    let sql = "with orders as (select id from orders) select id from orders";
    let a = pg(sql);
    assert!(a.diagnostics.is_empty());
    let first = nth_ref(&a, sql, "orders", 1);
    assert!(
        matches!(first.resolution, Resolution::Table(_)),
        "inner `orders` is the table: {:?}",
        first.resolution
    );
    let second = nth_ref(&a, sql, "orders", 2);
    assert!(matches!(second.resolution, Resolution::Symbol(_)), "outer `orders` is the CTE");
}

#[test]
fn derived_tables_expose_their_select_items() {
    clean("select s.n from (select count(*) as n from orders) s", K::Postgres);
    clean("select s.id from (select id from orders) s", K::Postgres);
    let a = pg("select s.total from (select id from orders) s");
    assert_eq!(msgs(&a), vec!["Unknown column 'total' in 's'"]);
    // an unnamed expression column: a name that is not listed may still exist, so no complaint
    clean("select count from (select count(*) from orders) s", K::Postgres);
    // alias column list
    clean("select a from (select id from orders) s (a)", K::Postgres);
}

#[test]
fn star_expands_to_exactly_the_columns_and_unknown_sources_stay_unknown() {
    let a = pg("select * from customers c join stores s on s.id = c.id");
    let star = &a.stars[0];
    assert!(star.complete);
    assert_eq!(star.columns.len(), 5 + 3);
    assert_eq!(star.columns[0], ("c".to_string(), "id".to_string()));
    let a = pg("select s.* from customers c join stores s on s.id = c.id");
    assert_eq!(a.stars[0].columns.len(), 3);
    assert!(a.stars[0].qualified);
    // a star over a table function is unknown: nothing after it may be reported
    clean("select x.a from (select * from generate_series(1, 3)) x", K::Postgres);
    let a = pg("select * from generate_series(1, 3)");
    assert!(!a.stars[0].complete);
    // USING hides the duplicate
    let a = pg("select * from orders join reviews using (customer_id)");
    assert_eq!(a.stars[0].columns.iter().filter(|(_, c)| c == "customer_id").count(), 1);
}

#[test]
fn correlated_subqueries_see_the_outer_query() {
    clean(
        "select c.id, (select count(*) from orders o where o.customer_id = c.id) from customers c",
        K::Postgres,
    );
    clean(
        "select * from customers c where exists (select 1 from orders o where o.customer_id = c.id)",
        K::Postgres,
    );
    // an inner name wins over an outer one
    clean("select * from customers where id in (select id from orders where total > 1)", K::Postgres);
    // but the outer query does not see the inner one
    let a = pg("select o.id from customers c where exists (select 1 from orders o)");
    assert_eq!(msgs(&a), vec!["'o' is not a table or alias in this query"]);
}

#[test]
fn lateral_sees_earlier_items_and_plain_derived_tables_do_not() {
    clean(
        "select * from customers c, lateral (select * from orders o where o.customer_id = c.id) t",
        K::Postgres,
    );
    clean(
        "select * from customers c join lateral (select o.id from orders o where o.customer_id = c.id) t on true",
        K::Postgres,
    );
    let a = pg("select * from customers c, (select * from orders o where o.customer_id = c.id) t");
    assert_eq!(msgs(&a), vec!["'c' is not a table or alias in this query"]);
    clean(
        "select * from customers c cross apply (select * from orders o where o.customer_id = c.id) t",
        K::SqlServer,
    );
}

#[test]
fn dml_targets_and_returning() {
    clean("insert into customers (id, email) values (1, 'a') returning id, email", K::Postgres);
    let a = pg("insert into customers (id, emial) values (1, 'a')");
    assert_eq!(msgs(&a), vec!["Unknown column 'emial' in 'customers'"]);
    let a = pg("insert into custmers (id) values (1)");
    assert_eq!(msgs(&a), vec!["Unknown table 'custmers'"]);
    clean("update customers set email = 'x' where id = 1 returning id", K::Postgres);
    let a = pg("update customers set emial = 'x'");
    assert_eq!(msgs(&a), vec!["Unknown column 'emial' in 'customers'"]);
    clean("update orders o set status = c.tier from customers c where c.id = o.customer_id", K::Postgres);
    clean("delete from orders using customers c where c.id = orders.customer_id", K::Postgres);
    clean("delete from orders o where o.id = 1 returning o.id", K::Postgres);
    let a = pg("delete from orders where idd = 1");
    assert_eq!(msgs(&a), vec!["Unknown column 'idd' (looked in orders)"]);
}

#[test]
fn on_conflict_has_excluded() {
    clean(
        "insert into customers (id, email) values (1, 'a') on conflict (id) do update set email = excluded.email, name = customers.name",
        K::Postgres,
    );
    clean(
        "insert into customers (id, email) values (1, 'a') on conflict (id) do update set email = excluded.email where customers.id > 0",
        K::Postgres,
    );
    let a = pg("insert into customers (id, email) values (1, 'a') on conflict (idd) do nothing");
    assert_eq!(msgs(&a), vec!["Unknown column 'idd' in 'customers'"]);
    clean(
        "insert into customers (id, email) values (1, 'a') on duplicate key update email = values(email)",
        K::MySql,
    );
}

#[test]
fn t_sql_update_with_an_alias_target() {
    clean(
        "update o set status = c.tier from orders o join customers c on c.id = o.customer_id where c.id = 1",
        K::SqlServer,
    );
    clean("delete o from orders o join customers c on c.id = o.customer_id", K::SqlServer);
    clean("delete from c using customers c join orders o on o.customer_id = c.id", K::MySql);
    clean("insert into customers (id) output inserted.id values (1)", K::SqlServer);
}

#[test]
fn merge_sees_target_and_source() {
    clean(
        "merge into customers t using (select 1 as id, 'x' as email) s on t.id = s.id \
         when matched then update set email = s.email when not matched then insert (id, email) values (s.id, s.email)",
        K::Postgres,
    );
    let a = pg("merge into customers t using orders s on t.id = s.idd when matched then delete");
    assert_eq!(msgs(&a), vec!["Unknown column 'idd' in 'orders'"]);
}

#[test]
fn quoting_and_case_follow_the_dialect() {
    // PostgreSQL folds unquoted names to lower case, quoted ones are exact
    clean("select ID from CUSTOMERS", K::Postgres);
    clean("select \"id\" from customers", K::Postgres);
    // "ID" does not exist exactly, but a name that differs only in case is never reported
    clean("select \"ID\" from customers", K::Postgres);
    clean("SELECT ID FROM Customers", K::Sqlite);
    clean("select `id` from customers", K::MySql);
    clean("select [id] from [customers]", K::SqlServer);
    // a quoted alias keeps its case: `"A"` is not `a`
    clean("select \"A\".id from customers \"A\"", K::Postgres);
}

#[test]
fn schema_qualified_names() {
    clean("select e.id from analytics.events e", K::Postgres);
    clean("select analytics.events.id from analytics.events", K::Postgres);
    let a = pg("select * from analytics.evnts");
    assert_eq!(msgs(&a), vec!["Unknown table 'analytics.evnts'"]);
    // a table of another schema is found even without the qualifier (a search path with several entries)
    clean("select day from daily_sales", K::Postgres);
    // schemas the server keeps to itself are never reported
    clean("select * from pg_catalog.pg_tables", K::Postgres);
    clean("select * from information_schema.tables", K::Postgres);
    clean("select * from pg_stat_activity", K::Postgres);
    // but an unknown schema is
    assert_eq!(msgs(&pg("select * from nosuch.t")).len(), 1);
}

#[test]
fn opaque_constructs_answer_unknown_never_wrong() {
    // a table function: its columns are unknown
    clean("select g from generate_series(1, 3) g", K::Postgres);
    clean("select t.whatever from jsonb_each('{}') t", K::Postgres);
    // an unknown statement part silences the statement
    clean("select nosuchcolumn from customers weird words here", K::DuckDb);
    // without a catalog nothing can be reported
    let a = analyze("select x from nowhere", K::Postgres, None);
    assert!(a.diagnostics.is_empty());
    let a = analyze("select x from nowhere", K::Postgres, Some(&theta_core::Catalog::default()));
    assert!(a.diagnostics.is_empty(), "an empty catalog means 'not loaded yet'");
    // tables the script creates are known
    clean(
        "create table scratch (a int, b text); select a, b from scratch; insert into scratch (a) values (1)",
        K::Postgres,
    );
    assert_eq!(msgs(&pg("create table scratch (a int); select zz from scratch")).len(), 1);
    clean("create temp table x as select * from customers; select whatever from x", K::Postgres);
    clean("select * into #tmp from customers; select anything from #tmp", K::SqlServer);
    // niladic functions and system columns are not columns
    clean("select current_date, ctid, current_user from customers", K::Postgres);
    // DuckDB lambdas and comprehensions bind their own names
    clean("select list_transform([1, 2], x -> x + 1)", K::DuckDb);
    clean("select [x + 1 for x in [1, 2]]", K::DuckDb);
    // SQLite reads a double-quoted unknown name as a string
    clean("select \"hello\" from customers", K::Sqlite);
}

#[test]
fn select_aliases_are_visible_to_order_by_and_group_by() {
    let sql = "select total * 2 as dbl from orders order by dbl";
    let a = pg(sql);
    assert!(a.diagnostics.is_empty());
    let r = nth_ref(&a, sql, "dbl", 1);
    assert!(matches!(r.resolution, Resolution::Symbol(_)));
    clean("select status, count(*) as n from orders group by status order by n desc", K::Postgres);
    // ORDER BY over a UNION names output columns
    clean("select id as a from orders union select id from customers order by a", K::Postgres);
    // an alias wins over a same-named column in ORDER BY, the column wins elsewhere
    let sql = "select total as id from orders where id > 1 order by id";
    let a = pg(sql);
    assert!(matches!(nth_ref(&a, sql, "id", 1).resolution, Resolution::Column { .. }));
    assert!(matches!(nth_ref(&a, sql, "id", 2).resolution, Resolution::Symbol(_)));
}

#[test]
fn duplicate_aliases_are_reported_where_the_dialect_rejects_them() {
    let a = pg("select 1 from customers c join orders c on true");
    assert_eq!(a.diagnostics.len(), 1);
    assert!(matches!(a.diagnostics[0].kind, DiagKind::DuplicateAlias { .. }));
    let a = pg("select 1 from customers join customers on true");
    assert_eq!(a.diagnostics.len(), 1);
    clean("select 1 from customers a join customers b on true", K::Postgres);
}

#[test]
fn column_counts_are_checked_when_certain() {
    let a = pg("insert into customers (id, email) values (1)");
    assert!(msgs(&a)[0].contains("2 target columns but this row has 1"), "{:?}", msgs(&a));
    let a = pg("insert into customers (id, email) select id from customers");
    assert_eq!(a.diagnostics.len(), 1);
    // more values than the table has columns, without a list
    let a = pg("insert into stores values (1, 'a', null, 4)");
    assert_eq!(a.diagnostics.len(), 1);
    // fewer values without a list is legal in PostgreSQL
    clean("insert into customers values (1, 'a')", K::Postgres);
    let a = pg("select id, email from customers union select id from customers");
    assert_eq!(a.diagnostics.len(), 1);
    assert!(matches!(a.diagnostics[0].kind, DiagKind::ColumnCount { expected: 2, found: 1 }));
    // a star over a known table counts; over an unknown one it does not
    let a = pg("select * from stores union select 1");
    assert_eq!(a.diagnostics.len(), 1);
    clean("select * from generate_series(1, 2) union select 1, 2", K::Postgres);
    clean("values (1, 2), (3, 4)", K::Postgres);
}

#[test]
fn group_by_requires_grouped_or_aggregated_columns() {
    let a = pg("select status, total from orders group by status");
    assert_eq!(
        msgs(&a),
        vec!["Column 'total' must appear in the GROUP BY clause or be used in an aggregate function"]
    );
    clean("select status, count(*), max(total) from orders group by status", K::Postgres);
    clean("select status, total + 1 from orders group by status, total", K::Postgres);
    // grouping by the primary key determines every column of the table
    clean("select c.id, c.email from customers c group by c.id", K::Postgres);
    let a = pg("select status, count(*) from orders");
    assert_eq!(a.diagnostics.len(), 1);
    // window functions do not make an aggregate query
    clean("select id, sum(total) over () from orders", K::Postgres);
    // positions and expressions are not analysed; neither are dialects that allow it
    clean("select status, total from orders group by 1", K::Postgres);
    clean("select status, total from orders group by status", K::Sqlite);
    clean("select status, total from orders group by status", K::MySql);
}

#[test]
fn refs_are_sorted_and_findable_by_offset() {
    let sql = "select o.id, c.name from orders o join customers c on c.id = o.customer_id";
    let a = pg(sql);
    assert!(a.refs.windows(2).all(|w| w[0].span.start <= w[1].span.start));
    let off = sql.find("customer_id").unwrap() + 3;
    let r = a.ref_at(off).unwrap();
    assert_eq!(&sql[r.span.clone()], "customer_id");
    assert_eq!(column_of(r), ("orders", "customer_id"));
    // the end of an identifier still belongs to it
    let r = a.ref_at(sql.find("name").unwrap() + 4).unwrap();
    assert_eq!(&sql[r.span.clone()], "name");
}

#[test]
fn symbols_know_their_references() {
    let sql = "with big as (select id, total from orders where total > 100) select big.id, total from big where big.total > 5";
    let a = pg(sql);
    let cte = a.symbols.iter().position(|s| s.kind == SymbolKind::Cte).unwrap();
    // the declaration, plus `from big`, plus the two `big.` qualifiers
    assert_eq!(a.refs_to_symbol(cte).count(), 4);
    let alias_sym =
        a.symbols.iter().position(|s| s.kind == SymbolKind::OutputColumn && s.name == "total").unwrap();
    // `big.total` in the WHERE and the unqualified `total` in the select list both reach the CTE column
    assert!(a.refs_to_symbol(alias_sym).count() >= 2);
    assert!(!a.symbols[alias_sym].renamable, "a passthrough column cannot be renamed");
}

#[test]
fn scopes_list_sources_with_their_columns() {
    let a = pg("select * from customers c join (select id from orders) o on true");
    let scope = a.scopes.iter().find(|s| s.sources.len() == 2).unwrap();
    assert_eq!(scope.sources[0].name, "c");
    assert_eq!(scope.sources[0].cols.as_ref().unwrap().len(), 5);
    assert_eq!(scope.sources[1].kind, SourceKind::Derived);
    assert_eq!(scope.sources[1].cols.as_ref().unwrap()[0].name, "id");
}

#[test]
fn a_script_is_analysed_statement_by_statement() {
    let a = pg("select emial from customers; select id from orders; select nope from stores");
    assert_eq!(a.diagnostics.len(), 2);
    assert!(a.diagnostics[0].range.start < a.diagnostics[1].range.start);
}

#[test]
fn analysis_of_a_large_script_is_fast_enough() {
    let unit = "select c.id, c.name, count(o.id) from customers c left join orders o on o.customer_id = c.id where c.tier = 'gold' group by c.id, c.name order by 3 desc;\n";
    let sql = unit.repeat(5000);
    let t = std::time::Instant::now();
    let a = analyze(&sql, K::Postgres, Some(&shop()));
    let took = t.elapsed();
    assert!(a.diagnostics.is_empty());
    eprintln!("analysed {} statements ({} lines) in {took:?}", 5000, sql.lines().count());
    assert!(took.as_secs_f64() < 5.0, "{took:?}");
}
