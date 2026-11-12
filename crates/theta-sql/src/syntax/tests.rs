//! Parser unit tests: tree shape, spans, the no-false-errors rule and the errors that are certain.
//! The differential test (`crates/theta-drivers/tests/sql_differential.rs`) is the broad evidence; these pin
//! the behaviour a reader of the grammar would want to see written down.

use super::*;
use crate::lexer::TokenKind;
use theta_core::DatabaseKind as K;

/// Compact s-expression of the node tree without tokens, e.g. `(Select (SelectList (SelectItem (ColumnRef))))`.
pub(crate) fn shape(n: &Node) -> String {
    if n.kind.is_token() {
        return String::new();
    }
    let kids: Vec<String> = n.children.iter().map(shape).filter(|s| !s.is_empty()).collect();
    if kids.is_empty() { format!("({:?})", n.kind) } else { format!("({:?} {})", n.kind, kids.join(" ")) }
}

fn parse_ok(sql: &str, kind: K) -> Parse {
    let p = parse(sql, kind);
    p.root.check_tiling(sql).unwrap_or_else(|e| panic!("tiling broken for {sql:?}: {e}\n{}", shape(&p.root)));
    p
}

fn no_errors(sql: &str, kind: K) {
    let p = parse_ok(sql, kind);
    assert!(p.errors.is_empty(), "false error on {sql:?}: {:?}\n{}", p.errors, shape(&p.root));
}

fn errors(sql: &str, kind: K) -> Vec<String> {
    parse_ok(sql, kind).errors.iter().map(|e| e.message.clone()).collect()
}

fn has_error(sql: &str, kind: K, needle: &str) {
    let e = errors(sql, kind);
    assert!(
        e.iter().any(|m| m.contains(needle)),
        "{sql:?} ({kind:?}): wanted an error containing {needle:?}, got {e:?}"
    );
}

fn count(p: &Parse, kind: Kind) -> usize {
    let mut n = 0;
    p.root.walk(&mut |x| n += (x.kind == kind) as usize);
    n
}

fn first(p: &Parse, kind: Kind) -> Node {
    let mut found = None;
    p.root.walk(&mut |x| {
        if found.is_none() && x.kind == kind {
            found = Some(x.clone());
        }
    });
    found.unwrap_or_else(|| panic!("no {kind:?} in {}", shape(&p.root)))
}

#[test]
fn shapes_of_the_main_statements() {
    let p = parse_ok(
        "select a, b as c from t x join u on x.id = u.id where a > 1 order by 1 limit 5",
        K::Postgres,
    );
    assert_eq!(
        shape(&p.root),
        "(Root (Statement (Query (Select (SelectList (SelectItem (ColumnRef)) (SelectItem (ColumnRef) (Alias))) \
         (FromClause (Join (TableName (Name) (Alias)) (TableName (Name)) (JoinConstraint (Binary (ColumnRef) (ColumnRef))))) \
         (WhereClause (Binary (ColumnRef) (Literal)))) (OrderBy (OrderItem (Literal))) (Limit (Literal)))))"
    );
    let p = parse_ok("with r as (select 1) select * from r", K::Postgres);
    assert_eq!(
        shape(&p.root),
        "(Root (Statement (Query (WithClause (Cte (Query (Select (SelectList (SelectItem (Literal))))))) \
         (Select (SelectList (SelectItem (Star))) (FromClause (TableName (Name)))))))"
    );
    let p = parse_ok(
        "insert into t (a) values (1) on conflict (a) do update set a = excluded.a returning a",
        K::Postgres,
    );
    assert_eq!(
        shape(&p.root),
        "(Root (Statement (Insert (Name) (ColumnList) (Values (RowCtor (Literal))) (OnConflict (ColumnList) \
         (SetClause (Assignment (ColumnRef) (ColumnRef)))) (Returning (SelectList (SelectItem (ColumnRef)))))))"
    );
}

#[test]
fn every_node_has_its_exact_span() {
    let sql = "SELECT  c.id , count(*)  FROM customers AS c -- note\n WHERE c.id > 1;";
    let p = parse_ok(sql, K::Postgres);
    let from = first(&p, Kind::FromClause);
    assert_eq!(from.text(sql), "FROM customers AS c");
    let name = first(&p, Kind::TableName).child(Kind::Name).cloned().unwrap();
    assert_eq!(name.text(sql), "customers");
    let alias = first(&p, Kind::Alias);
    assert_eq!(alias.text(sql), "AS c");
    let call = first(&p, Kind::FuncCall);
    assert_eq!(call.text(sql), "count(*)");
    let w = first(&p, Kind::WhereClause);
    assert_eq!(w.text(sql), "WHERE c.id > 1");
    // A statement includes its `;`, the comment between clauses belongs to the parent.
    let stmt = first(&p, Kind::Statement);
    assert_eq!(stmt.text(sql), &sql[..sql.len()]);
    // path_at maps a cursor to the innermost node.
    let at = sql.find("c.id >").unwrap() + 1;
    let path = p.root.path_at(at);
    let node = path.iter().rev().find(|n| !n.kind.is_token()).unwrap();
    assert_eq!(node.kind, Kind::ColumnRef);
    assert_eq!(node.text(sql), "c.id");
}

#[test]
fn chains_are_flat_not_towers() {
    let sql = format!("select {}", (0..2000).map(|i| format!("a{i}")).collect::<Vec<_>>().join(" + "));
    let p = parse_ok(&sql, K::Postgres);
    assert_eq!(count(&p, Kind::Binary), 1, "a + b + c ... must be one flat Binary");
    let sql = format!(
        "select 1 from t where {}",
        (0..500).map(|i| format!("x = {i}")).collect::<Vec<_>>().join(" and ")
    );
    assert_eq!(count(&parse_ok(&sql, K::Postgres), Kind::Binary), 501, "500 comparisons under one flat AND");
    let sql = format!("select 1 {}", "union select 1 ".repeat(300));
    assert_eq!(count(&parse_ok(&sql, K::Postgres), Kind::SetOp), 1);
    // Different precedence still nests.
    let p = parse_ok("select a + b * c + d", K::Postgres);
    assert_eq!(count(&p, Kind::Binary), 2, "a + (b * c) + d: the product nests, the two sums are one chain");
}

#[test]
fn precedence_and_or_not() {
    let p = parse_ok("select * from t where a = 1 or b = 2 and not c = 3", K::Postgres);
    let w = first(&p, Kind::WhereClause);
    // OR at the top, with AND (flat, binding tighter) as its right operand
    let or = w.nodes().next().unwrap();
    assert_eq!(or.kind, Kind::Binary);
    let kids: Vec<_> = or.nodes().collect();
    assert_eq!(kids.len(), 2);
    assert_eq!(kids[0].kind, Kind::Binary); // a = 1
    assert_eq!(kids[1].kind, Kind::Binary); // b = 2 and not c = 3
    assert_eq!(count(&p, Kind::Prefix), 1);
}

#[test]
fn queries_in_every_shape_parse_without_errors() {
    let cases: &[(&str, K)] = &[
        ("select 1 union all select 2 intersect select 3 except select 4 order by 1 limit 2", K::Postgres),
        ("(select 1) union (select 2)", K::Postgres),
        ("((select 1)) union ((select 2) limit 1)", K::Postgres),
        (
            "with recursive r(n) as (select 1 union all select n + 1 from r where n < 5) select * from r",
            K::Postgres,
        ),
        (
            "with a as materialized (select 1), b as not materialized (select 2) select * from a, b",
            K::Postgres,
        ),
        ("with d as (delete from t returning *) select * from d", K::Postgres),
        (
            "select * from a left join lateral (select * from b where b.x = a.x limit 1) s on true",
            K::Postgres,
        ),
        (
            "select * from a cross join b natural join c join d using (x) full outer join e on a.x = e.x",
            K::Postgres,
        ),
        ("select * from a join b join c on b.x = c.x on a.x = b.x", K::MySql),
        ("select * from (a join b on a.x = b.x) join c on c.x = a.x", K::Postgres),
        (
            "select count(*) filter (where x > 1) over (partition by y order by z rows between 1 preceding and current row) from t",
            K::Postgres,
        ),
        ("select sum(x) over w from t window w as (partition by y)", K::Postgres),
        ("select percentile_cont(0.5) within group (order by x) from t", K::Postgres),
        ("select string_agg(x, ',' order by y desc) from t", K::Postgres),
        ("select x::int, y::numeric(10, 2), z::int[], '{1}'::text[] from t", K::Postgres),
        ("select a[1], b[1:2], (c).d, (e).* from t", K::Postgres),
        ("select case when a then 1 else 2 end, case x when 1 then 2 end from t", K::Postgres),
        (
            "select a is distinct from b, a is not null, a between 1 and 2 and b, a not like 'x' escape '!' from t",
            K::Postgres,
        ),
        ("select x in (1, 2), x in (select 1), x not in (select 1), x = any (array[1]) from t", K::Postgres),
        ("select exists (select 1), not exists (select 1)", K::Postgres),
        ("select interval '1 day', date '2020-01-01', timestamp '2020-01-01 00:00:00'", K::Postgres),
        (
            "select extract(year from now()), substring('abc' from 2 for 1), trim(both 'x' from 'xax')",
            K::Postgres,
        ),
        ("select a operator(pg_catalog.+) b, a #> b, a ->> 'k', a @> b, a ?| b from t", K::Postgres),
        ("select $1, $2::int, :name, ? from t", K::Postgres),
        (
            "select * from t order by a nulls last, b desc nulls first fetch first 5 rows with ties",
            K::Postgres,
        ),
        ("select * from t for update of t skip locked", K::Postgres),
        (
            "select top (5) percent with ties * from t order by a offset 2 rows fetch next 3 rows only",
            K::SqlServer,
        ),
        ("select * from t group by rollup (a, b), cube (c), grouping sets ((d), ())", K::Postgres),
        ("select * from t tablesample bernoulli (10) repeatable (1)", K::Postgres),
        ("select * from unnest(array[1, 2]) with ordinality as u(n, i)", K::Postgres),
        ("select distinct on (a) a, b from t order by a, b", K::Postgres),
        ("select * from only t, only (u)", K::Postgres),
        ("values (1, 'a'), (2, 'b')", K::Postgres),
        ("table t", K::Postgres),
        ("from t select a where a > 1", K::DuckDb),
        ("select * exclude (a) replace (b + 1 as b) from t", K::DuckDb),
        ("select [x + 1 for x in l], {'a': 1}.a, map {'k': 1}['k'] from t", K::DuckDb),
        ("select * from t qualify row_number() over (order by a) = 1", K::DuckDb),
        ("select `a`, group_concat(b order by c separator ',') from `t` limit 1, 2", K::MySql),
        ("select * from t use index (i) where a = 1 lock in share mode", K::MySql),
        ("select @a := 1, @@version", K::MySql),
        ("select a from t where match (a) against ('x' in boolean mode)", K::MySql),
        ("select [col], @v from #tmp t with (nolock) cross apply f(t.id) x", K::SqlServer),
        ("select alias = col from dbo.t for xml path('')", K::SqlServer),
        ("select x from t array join arr as x final sample 0.1", K::ClickHouse),
    ];
    for (sql, k) in cases {
        no_errors(sql, *k);
    }
}

#[test]
fn dml_in_every_shape_parses_without_errors() {
    let cases: &[(&str, K)] = &[
        (
            "insert into t (a, b) values (1, 2), (3, 4) on conflict (a) where a > 0 do update set b = excluded.b where t.b < 9 returning *",
            K::Postgres,
        ),
        ("insert into t select * from u on conflict on constraint c do nothing", K::Postgres),
        ("insert into t default values returning id", K::Postgres),
        ("insert into t as x (a) overriding system value values (1)", K::Postgres),
        ("insert into t (a) values (1) on duplicate key update a = values(a), b = b + 1", K::MySql),
        ("insert ignore into t set a = 1, b = 2", K::MySql),
        ("replace into t values (1)", K::MySql),
        ("insert or replace into t (a) select 1", K::Sqlite),
        ("insert into t (a) output inserted.id values (1)", K::SqlServer),
        ("update t set a = 1, (b, c) = (2, 3), d[1] = 4 from u where u.x = t.x returning t.a", K::Postgres),
        ("update t set a = a + 1 where current of cur", K::Postgres),
        ("update t1 join t2 on t1.x = t2.x set t1.a = t2.a where t2.b > 1 order by t1.id limit 3", K::MySql),
        (
            "update top (10) t set a += 1 output deleted.a, inserted.a from t join u on u.x = t.x",
            K::SqlServer,
        ),
        ("delete from t using u where t.x = u.x returning t.*", K::Postgres),
        ("delete t1, t2 from t1 join t2 on t1.x = t2.x where t1.a = 1", K::MySql),
        ("delete top (5) from t output deleted.* where a = 1", K::SqlServer),
        (
            "merge into t using (select 1 as id) s on t.id = s.id when matched and t.x > 1 then update set x = s.id when not matched then insert (id) values (s.id) when matched then delete",
            K::Postgres,
        ),
        (
            "merge t as x using u on x.id = u.id when not matched by source then delete output $action",
            K::SqlServer,
        ),
        ("with u as (select 1) update t set a = 1 from u", K::Postgres),
        ("explain (analyze, format json) select 1", K::Postgres),
        ("explain query plan select 1", K::Sqlite),
        (
            "create table if not exists s.t (id serial primary key, n text not null default '', c int references o (id), check (n <> ''))",
            K::Postgres,
        ),
        ("create table t as select * from u with no data", K::Postgres),
        ("create view v (a) as with x as (select 1) select * from x", K::Postgres),
        (
            "create unique index concurrently if not exists i on t using btree (a desc nulls last, lower(b)) include (c) where d > 0",
            K::Postgres,
        ),
        ("create function f() returns int as $$ select 1 $$ language sql", K::Postgres),
        ("create trigger tr after insert on t begin update u set a = new.a; end", K::Sqlite),
        (
            "alter table t add column c int, drop column d, alter column e type text using e::text",
            K::Postgres,
        ),
        ("drop table if exists a, b cascade", K::Postgres),
        ("truncate table a, b restart identity", K::Postgres),
        ("set search_path to public, pg_catalog", K::Postgres),
        ("copy (select * from t) to stdout", K::Postgres),
        ("prepare p (int) as select * from t where id = $1", K::Postgres),
        ("declare c cursor for select 1", K::Postgres),
    ];
    for (sql, k) in cases {
        no_errors(sql, *k);
    }
}

#[test]
fn embedded_queries_in_other_statements_are_parsed() {
    let p = parse_ok("create view v as select a from t where b = 1", K::Postgres);
    assert_eq!(count(&p, Kind::Query), 1);
    assert_eq!(count(&p, Kind::WhereClause), 1);
    let p = parse_ok("prepare q as select * from t", K::Postgres);
    assert_eq!(count(&p, Kind::Query), 1);
    let p = parse_ok("copy (select * from t) to stdout", K::Postgres);
    assert_eq!(count(&p, Kind::Subquery), 1);
    // Routine bodies stay opaque: `new` / `old` are not columns of any table.
    let p = parse_ok("create trigger tr after insert on t begin select new.a; end", K::Sqlite);
    assert_eq!(count(&p, Kind::Query), 0);
    let p = parse_ok("create procedure p() begin select x from y; end", K::MySql);
    assert_eq!(count(&p, Kind::Query), 0);
}

#[test]
fn ddl_names_are_recorded() {
    let p = parse_ok("create table if not exists s.t (id int, name text)", K::Postgres);
    let d = first(&p, Kind::Ddl);
    let sql = "create table if not exists s.t (id int, name text)";
    assert_eq!(d.children_of(Kind::Name).next().unwrap().text(sql), "s.t");
    assert_eq!(count(&p, Kind::ColumnDef), 2);
    let p = parse_ok("drop table a, b.c", K::Postgres);
    assert_eq!(first(&p, Kind::Ddl).children_of(Kind::Name).count(), 2);
}

#[test]
fn scripts_split_into_statements_with_separators() {
    let sql = "select 1;\n\nselect 2; -- tail\ngo\nselect 3";
    let p = parse_ok(sql, K::SqlServer);
    assert_eq!(p.statements().count(), 3);
    assert_eq!(count(&p, Kind::BatchSeparator), 1);
    let p = parse_ok(";;select 1;", K::Postgres);
    assert_eq!(p.statements().count(), 3);
    let p = parse_ok("   \n-- only a comment\n", K::Postgres);
    assert_eq!(p.statements().count(), 0);
    assert!(p.errors.is_empty());
}

#[test]
fn unknown_syntax_is_opaque_and_silent() {
    for (sql, k) in [
        ("select * from t foo bar baz", K::DuckDb),
        ("select x from t sample 10 percent weird words here", K::ClickHouse),
        ("alter table t frobnicate (a, b) with options", K::Postgres),
        ("select * from t asof join u match_condition (a > b)", K::SqlServer),
        ("select a from t where b ~~~ c", K::Postgres),
        ("some completely unknown statement (with parens) and 'strings'", K::Postgres),
    ] {
        no_errors(sql, k);
    }
    let p = parse_ok("select * from t foo bar baz", K::DuckDb);
    assert_eq!(count(&p, Kind::Unknown), 1, "the stray words are kept in one Unknown node");
}

// ---- errors that are certain

#[test]
fn certain_errors_unbalanced_parentheses() {
    has_error("select (1", K::Postgres, "Unclosed '('");
    has_error("select 1)", K::Postgres, "Unmatched ')'");
    has_error("select * from (select 1", K::MySql, "Unclosed '('");
    has_error("select f(a, (b)", K::Sqlite, "Unclosed '('");
    has_error("create table t (a int", K::Postgres, "Unclosed '('");
    has_error("grant select on t to (x", K::Postgres, "Unclosed '('");
    // The span points at the parenthesis itself.
    let sql = "select (1 + 2";
    let p = parse_ok(sql, K::Postgres);
    assert_eq!(&sql[p.errors[0].span.clone()], "(");
    assert!(p.errors[0].incomplete);
}

#[test]
fn certain_errors_missing_operands() {
    has_error("select * from t where a =", K::Postgres, "after '='");
    has_error("select * from t where a = 1 and", K::MySql, "after AND");
    has_error("select * from t where a +", K::Sqlite, "after '+'");
    has_error("select * from t where not", K::Postgres, "after NOT");
    has_error("select * from t where a is", K::Postgres, "after IS");
    has_error("select * from t where a between 1", K::Postgres, "BETWEEN");
    has_error("select * from t where a in", K::Postgres, "after IN");
    has_error("select * from t where", K::Postgres, "after WHERE");
    has_error("select * from t order by", K::Postgres, "after ORDER BY");
    has_error("select * from t group by", K::MySql, "after GROUP BY");
    has_error("select * from t limit", K::Postgres, "LIMIT");
    has_error("select * from", K::Postgres, "table after FROM");
    has_error("select * from where", K::Postgres, "table after FROM");
    has_error("select * from t join", K::Postgres, "table after JOIN");
    has_error("select * from t join u on", K::Postgres, "after ON");
    has_error("select case when a then 1", K::Postgres, "CASE is missing END");
    has_error("select 1 union", K::Postgres, "after the set operator");
    has_error("with t as (select 1)", K::Postgres, "after the WITH");
    has_error("update t where a = 1", K::Postgres, "SET");
    has_error("update t set a =", K::Postgres, "after '='");
    has_error("insert into t (a) values (1,)", K::Postgres, "comma");
    has_error("select a,, b from t", K::Postgres, "comma");
    has_error("select f(,1)", K::Postgres, "comma");
    has_error("select cast(1)", K::Postgres, "CAST");
}

#[test]
fn certain_errors_depend_on_the_dialect() {
    // Valid in PostgreSQL and DuckDB, an error elsewhere.
    no_errors("select from t", K::Postgres);
    no_errors("select from t", K::DuckDb);
    has_error("select from t", K::MySql, "select list");
    has_error("select from t", K::Sqlite, "select list");
    has_error("select from t", K::SqlServer, "select list");
    // DuckDB accepts trailing commas.
    no_errors("select a, from t", K::DuckDb);
    no_errors("select * from t order by a,", K::DuckDb);
    has_error("select a, from t", K::Postgres, "Trailing comma");
    has_error("select a, from t", K::MySql, "Trailing comma");
    // `x IN ()` is legal in SQLite.
    no_errors("select * from t where x in ()", K::Sqlite);
    has_error("select * from t where x in ()", K::Postgres, "at least one value");
    has_error("select * from t where x in ()", K::MySql, "at least one value");
    // JOIN without ON: an error in PostgreSQL and T-SQL, a cross join in MySQL and SQLite.
    has_error("select * from a join b", K::Postgres, "ON or USING");
    has_error("select * from a join b", K::SqlServer, "ON or USING");
    no_errors("select * from a join b", K::MySql);
    no_errors("select * from a join b", K::Sqlite);
    no_errors("select * from a cross join b", K::Postgres);
    no_errors("select * from a natural join b", K::Postgres);
    // In PostgreSQL almost any word is a bare column label: `select 1 and` is an operand with an alias.
    no_errors("select 1 and", K::Postgres);
    has_error("select 1 and", K::MySql, "after AND");
    has_error("select * from t where x = 1 and", K::Postgres, "after AND");
    // PostgreSQL's `format` is a column, T-SQL's `output` is not.
    no_errors("select format, option, settings from t", K::Postgres);
    no_errors("select offset from t", K::MySql);
}

#[test]
fn certain_errors_unterminated_literals() {
    has_error("select 'abc", K::Postgres, "Unterminated string");
    has_error("select \"abc", K::Postgres, "Unterminated quoted identifier");
    has_error("select 1 /* abc", K::Postgres, "Unterminated comment");
    has_error("select $$ abc", K::Postgres, "dollar");
    // SQLite runs an unterminated block comment to the end without complaint.
    no_errors("select 1 /* abc", K::Sqlite);
    // PostgreSQL has no backtick quoting: the character is an operator.
    no_errors("select `abc", K::Postgres);
}

#[test]
fn one_error_does_not_hide_the_rest() {
    // The WHERE is broken; the FROM clause and the following statement are intact.
    let sql = "select a from t where ;\nselect b from u where b > 1";
    let p = parse_ok(sql, K::Postgres);
    assert_eq!(p.errors.len(), 1);
    assert_eq!(p.statements().count(), 2);
    assert_eq!(count(&p, Kind::TableName), 2);
    // A broken select item still leaves the FROM clause parsed.
    let p = parse_ok("select a, , b from t where x = 1", K::Postgres);
    assert_eq!(p.errors.len(), 1);
    assert_eq!(count(&p, Kind::SelectItem), 2);
    assert_eq!(count(&p, Kind::WhereClause), 1);
    // An unclosed parenthesis in one statement does not swallow the next one.
    let p = parse_ok("select (1;\nselect 2", K::Postgres);
    assert_eq!(p.statements().count(), 2);
    assert_eq!(p.errors.len(), 1);
}

#[test]
fn mysql_executable_comments_silence_missing_piece_errors() {
    no_errors("select /*!50000 1 */", K::MySql);
    has_error("select", K::MySql, "select list");
}

#[test]
fn lexer_kinds_survive_in_the_tree() {
    let sql = "select 'a' /* c */ , \"b\" -- x\n from t";
    let p = parse_ok(sql, K::Postgres);
    let mut kinds = vec![];
    p.root.walk(&mut |n| {
        if let Kind::Token(k) = n.kind {
            kinds.push(k)
        }
    });
    assert!(kinds.contains(&TokenKind::BlockComment));
    assert!(kinds.contains(&TokenKind::LineComment));
    assert!(kinds.contains(&TokenKind::QuotedIdent));
}

// ---- typos found in real use: the general causes, each with its valid neighbours

const CHECKED: [K; 4] = [K::Postgres, K::MySql, K::Sqlite, K::DuckDb];

#[test]
fn a_keyword_that_must_be_followed_by_by() {
    for k in CHECKED {
        has_error("select * from t order id", k, "BY after ORDER");
        has_error("select * from t where a = 1 order", k, "BY after ORDER");
        has_error("select * from t group country", k, "BY after GROUP");
        no_errors("select * from t order by id", k);
        no_errors("select a, count(*) over (order by a) from t group by a", k);
    }
    // T-SQL and ClickHouse are not checked against a live server: silent.
    no_errors("select * from t order id", K::SqlServer);
    no_errors("select * from t order id", K::ClickHouse);
}

#[test]
fn an_assignment_target_without_an_equals_sign() {
    for k in CHECKED {
        has_error("update t set email 'a' where id = 1", k, "Expected '='");
        has_error("update t set email where id = 1", k, "Expected '='");
        has_error("update t set a, b = 1", k, "Expected '='");
        no_errors("update t set email = 'a', city = 'b' where id = 1", k);
        no_errors("update t set (a, b) = (1, 2)", k);
    }
    no_errors("update t set email 'a'", K::SqlServer);
}

#[test]
fn a_call_after_a_select_item_means_a_missing_comma() {
    for k in CHECKED {
        has_error("select c.country count(*) from c", k, "comma");
        has_error("select 1 x(1)", k, "comma");
        no_errors("select c.country, count(*) from c", k);
        no_errors("select count(*) cnt, sum(a) total from c", k);
        no_errors("select a x from c", k);
    }
    // A column list after a table alias is real (PostgreSQL, DuckDB; the others reject it, which is not claimed).
    no_errors("select * from f x(a, b)", K::Postgres);
    no_errors("select * from generate_series(1, 2) x(a)", K::DuckDb);
}

#[test]
fn a_case_that_is_never_closed() {
    for k in [K::Postgres, K::MySql, K::Sqlite, K::DuckDb, K::SqlServer, K::ClickHouse] {
        has_error("select case when a = 1 then 'x' from t", k, "missing END");
        has_error("select case when a = 1 then 'x', b from t", k, "missing END");
        has_error("select (case when a = 1 then 'x') from t", k, "missing END");
        has_error("select case a from t", k, "missing END");
        no_errors("select case when a = 1 then 'x' end from t", k);
        no_errors("select case when a = 1 then 'x' else 'y' end x from t", k);
        no_errors("select sum(case when a = 1 then 1 else 0 end) from t", k);
    }
}

#[test]
fn delete_without_from_in_mysql() {
    has_error("delete t where id = 1", K::MySql, "FROM after the DELETE target");
    no_errors("delete t1 from t1 join t2 on t1.id = t2.id", K::MySql);
    no_errors("delete t where id = 1", K::SqlServer);
}

#[test]
fn duckdb_allows_one_word_after_a_star_and_flags_the_second() {
    no_errors("select * fro", K::DuckDb);
    has_error("select * form customers", K::DuckDb, "Unexpected 'customers'");
    no_errors("from t select a", K::DuckDb);
    no_errors("select * like '%a' from t", K::DuckDb);
}
