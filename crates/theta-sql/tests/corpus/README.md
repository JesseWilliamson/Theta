# Corpus for the differential and robustness tests

One statement per line in `common.sql`, `pg.sql`, `mysql.sql`, `sqlite.sql`, `duckdb.sql`, `clickhouse.sql`,
`tsql.sql`; `extracted.sql` holds multi-line statements separated by `-- ---`; `broken.sql` is deliberately
malformed. `schema_sqlite.sql` / `schema_duckdb.sql` create the tables the corpus refers to (PostgreSQL and MariaDB
use the local `quarry_demo` database).

How each engine is asked (`crates/theta-drivers/tests/sql_differential.rs`): PostgreSQL `PREPARE` for queries and
DML and `BEGIN` ... `ROLLBACK` for the rest (SQLSTATE 42601 / 42602 / 42000 is a syntax error); MariaDB
`PREPARE ... FROM` (1064 / 1149 syntax, 1295 parsed but not preparable); SQLite and DuckDB `prepare`. A test
skips when its server is unreachable. ClickHouse and T-SQL entries are documentation examples assumed valid.
