//! The dialect entries added for the newer products: lexing, quoting, statement splitting, keywords and
//! the formatter must not mangle their syntax.

use theta_core::DatabaseKind::{self, *};
use theta_sql::lexer::{TokenKind, tokenize};
use theta_sql::*;

fn kinds_of(sql: &str, k: DatabaseKind) -> Vec<(TokenKind, String)> {
    tokenize(sql, k)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .map(|t| (t.kind, sql[t.start..t.end].to_string()))
        .collect()
}

#[test]
fn bracket_identifiers_and_national_strings_in_tsql() {
    let t = kinds_of("SELECT [Order Id], N'héllo' FROM [dbo].[My Table] WHERE x = @p1", SqlServer);
    assert!(t.contains(&(TokenKind::QuotedIdent, "[Order Id]".into())));
    assert!(t.contains(&(TokenKind::String, "N'héllo'".into())));
    assert!(t.contains(&(TokenKind::QuotedIdent, "[My Table]".into())));
    assert!(t.contains(&(TokenKind::Param, "@p1".into())));
}

#[test]
fn go_separates_batches_but_only_on_its_own_line() {
    let sql = "CREATE TABLE a (id int)\nGO\nINSERT INTO a VALUES (1)\nGO 3\nSELECT 1 AS go_col";
    let s = split_statements(sql, SqlServer);
    let texts: Vec<&str> = s.iter().map(|x| &sql[x.range.clone()]).collect();
    assert_eq!(texts, ["CREATE TABLE a (id int)", "INSERT INTO a VALUES (1)", "SELECT 1 AS go_col"]);
    // In another dialect GO is just a word.
    assert_eq!(split_statements("SELECT 1\nGO\nSELECT 2", Postgres).len(), 1);
    // A column that happens to be called go is not a separator.
    assert_eq!(split_statements("SELECT go FROM t", SqlServer).len(), 1);
}

#[test]
fn begin_transaction_is_not_a_block_in_a_procedure() {
    let sql = "CREATE PROCEDURE p AS BEGIN BEGIN TRANSACTION; UPDATE t SET a = 1; COMMIT; END";
    assert_eq!(split_statements(sql, SqlServer).len(), 1);
}

#[test]
fn clickhouse_comments_and_escapes() {
    let t = kinds_of("SELECT 'it\\'s', `a b` # trailing\nFROM t", ClickHouse);
    assert!(t.contains(&(TokenKind::String, "'it\\'s'".into())));
    assert!(t.contains(&(TokenKind::QuotedIdent, "`a b`".into())));
    // `"x"` is an identifier in ClickHouse, a string in MySQL.
    assert_eq!(kinds_of("\"x\"", ClickHouse)[0].0, TokenKind::QuotedIdent);
}

#[test]
fn duckdb_keeps_postgres_lexing() {
    let t = kinds_of("SELECT $$a;b$$, '{}'::JSON, x::INT[]", DuckDb);
    assert!(t.contains(&(TokenKind::DollarString, "$$a;b$$".into())), "{t:?}");
    assert_eq!(split_statements("SELECT $$a;b$$; SELECT 2", DuckDb).len(), 2);
}

#[test]
fn quoting_per_product() {
    assert_eq!(SqlServer.quote_ident("a]b"), "[a]]b]");
    assert_eq!(ClickHouse.quote_ident("a`b"), "`a``b`");
    assert_eq!(DuckDb.quote_ident("a\"b"), "\"a\"\"b\"");
    assert_eq!(TiDb.quote_ident("t"), "`t`");
    assert_eq!(CockroachDb.quote_ident("t"), "\"t\"");
    assert!(keywords::needs_quote("order", SqlServer));
    assert!(keywords::needs_quote("User Name", ClickHouse));
}

#[test]
fn literals_follow_each_dialect() {
    use theta_core::Value;
    assert_eq!(sql_literal(&Value::Text("it's".into()), SqlServer), "N'it''s'");
    assert_eq!(sql_literal(&Value::Bool(true), SqlServer), "1");
    assert_eq!(sql_literal(&Value::Bytes(vec![0xab, 0x01]), SqlServer), "0xab01");
    assert_eq!(sql_literal(&Value::Text("a\\b".into()), ClickHouse), "'a\\\\b'");
    assert_eq!(sql_literal(&Value::Bytes(vec![0xab]), DuckDb), "unhex('ab')");
    assert_eq!(sql_literal(&Value::Float(f64::INFINITY), DuckDb), "'Infinity'::DOUBLE");
}

#[test]
fn formatter_leaves_dialect_syntax_alone() {
    let o = FormatOptions::default();
    let f = |sql: &str, k| format(sql, k, &o);
    let t = f("select top (10) [a b], isnull(x,0) from [dbo].[t] where [c]=@p1", SqlServer);
    assert!(t.contains("[a b]") && t.contains("[dbo].[t]") && t.contains("@p1"), "{t}");
    assert!(t.to_uppercase().contains("TOP (10)"), "{t}");
    let c = f(
        "select toStartOfHour(ts) as h, countIf(x > 1) from db.t final where a in (1,2) settings max_threads=2",
        ClickHouse,
    );
    assert!(c.contains("toStartOfHour(ts)") && c.contains("countIf(x > 1)"), "{c}");
    let d = f("select * from read_parquet('a.parquet') where l[1] = 2 and s.a = {'k': 1}['k']", DuckDb);
    assert!(d.contains("read_parquet('a.parquet')") && d.contains("l[1]"), "{d}");
    // GO survives formatting, on its own line
    let g = f("select 1\nGO\nselect 2", SqlServer);
    assert!(g.lines().any(|l| l == "GO"), "{g}");
    // idempotent: formatting formatted text changes nothing
    for (sql, k) in [(t.clone(), SqlServer), (c.clone(), ClickHouse), (d.clone(), DuckDb)] {
        assert_eq!(f(&sql, k), sql);
    }
}

#[test]
fn classification_and_keywords() {
    assert_eq!(classify("SELECT a INTO #t FROM b", SqlServer), StatementClass::Ddl);
    assert_eq!(classify("OPTIMIZE TABLE t FINAL", ClickHouse), StatementClass::Ddl);
    assert_eq!(classify("EXPLAIN SELECT 1", DuckDb), StatementClass::Query);
    let f = keywords::builtin_functions(SqlServer, false);
    assert!(f.iter().any(|x| x.name == "dateadd") && !f.iter().any(|x| x.name == "date_trunc"));
    let f = keywords::builtin_functions(ClickHouse, false);
    assert!(f.iter().any(|x| x.name == "toStartOfInterval"));
    let f = keywords::builtin_functions(DuckDb, false);
    assert!(f.iter().any(|x| x.name == "read_parquet"));
    assert!(ddl::type_suggestions(DuckDb).contains(&"HUGEINT"));
    assert!(ddl::type_suggestions(SqlServer).contains(&"datetime2"));
    assert!(ddl::type_suggestions(ClickHouse).contains(&"LowCardinality(String)"));
    assert!(!ddl::type_suggestions(Redshift).contains(&"jsonb"));
}
