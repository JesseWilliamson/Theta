-- ClickHouse examples written from the official documentation (SELECT, JOIN, ARRAY JOIN, WITH, SAMPLE, FINAL
-- ...). Only the server in 18.16 is available locally, which rejects much of modern syntax; these statements are
-- checked against that server only for "accepted by the server means no false error".
SELECT 1
SELECT 1 AS one, 2 AS two
SELECT * FROM system.numbers LIMIT 10
SELECT number FROM system.numbers LIMIT 5
SELECT number, number * 2 FROM system.numbers LIMIT 5
SELECT number FROM numbers(10)
SELECT number FROM numbers(10) WHERE number > 5
SELECT number FROM numbers(10) ORDER BY number DESC LIMIT 3
SELECT count() FROM numbers(10)
SELECT count(*) FROM numbers(10)
SELECT sum(number), avg(number), min(number), max(number) FROM numbers(10)
SELECT number % 3 AS g, count() FROM numbers(10) GROUP BY g ORDER BY g
SELECT number % 3 AS g, count() FROM numbers(10) GROUP BY g WITH TOTALS
SELECT number % 3 AS g, count() FROM numbers(10) GROUP BY g HAVING count() > 3
SELECT number FROM numbers(10) LIMIT 2, 3
SELECT number FROM numbers(10) LIMIT 3 OFFSET 2
SELECT number FROM numbers(10) LIMIT 2 BY number
SELECT DISTINCT number % 3 FROM numbers(10)
SELECT number FROM numbers(10) WHERE number IN (1, 2, 3)
SELECT number FROM numbers(10) WHERE number NOT IN (1, 2, 3)
SELECT number FROM numbers(10) WHERE number BETWEEN 2 AND 4
SELECT number FROM numbers(10) WHERE number > 2 AND number < 8 OR number = 0
SELECT number FROM numbers(10) PREWHERE number > 5
SELECT number FROM numbers(10) PREWHERE number > 5 WHERE number < 8
SELECT number FROM numbers(10) FORMAT JSON
SELECT number FROM numbers(10) FORMAT TabSeparated
SELECT number FROM numbers(10) SETTINGS max_threads = 1
SELECT toDate('2020-01-01'), toDateTime('2020-01-01 00:00:00'), toString(1), toInt32('1')
SELECT now(), today(), yesterday()
SELECT toStartOfMonth(today()), toYear(today()), formatDateTime(now(), '%Y-%m-%d')
SELECT arrayJoin([1, 2, 3])
SELECT arrayJoin([1, 2, 3]) AS x
SELECT [1, 2, 3]
SELECT [1, 2, 3][1]
SELECT (1, 2)
SELECT (1, 2).1
SELECT tuple(1, 2)
SELECT map('a', 1)
SELECT range(5)
SELECT arrayMap(x -> x * 2, [1, 2, 3])
SELECT arrayFilter(x -> x > 1, [1, 2, 3])
SELECT arraySum([1, 2, 3])
SELECT if(1 > 0, 'a', 'b')
SELECT multiIf(1 > 2, 'a', 1 > 0, 'b', 'c')
SELECT 1 > 0 ? 'a' : 'b'
SELECT CASE WHEN 1 > 0 THEN 'a' ELSE 'b' END
SELECT CAST(1 AS String)
SELECT CAST('1', 'Int32')
SELECT 1::Int32
SELECT toDecimal32(1.5, 2)
SELECT 'a' || 'b'
SELECT concat('a', 'b')
SELECT 'abc' LIKE 'a%'
SELECT 'abc' ILIKE 'A%'
SELECT match('abc', '^a')
SELECT x FROM (SELECT 1 AS x)
SELECT x FROM (SELECT 1 AS x) WHERE x = 1
SELECT a.x, b.y FROM (SELECT 1 AS x) a JOIN (SELECT 1 AS y) b ON a.x = b.y
SELECT a.x, b.y FROM (SELECT 1 AS x) AS a ANY LEFT JOIN (SELECT 1 AS y) AS b ON a.x = b.y
SELECT a.x, b.y FROM (SELECT 1 AS x) a ALL INNER JOIN (SELECT 1 AS y) b USING (x)
SELECT a.x, b.x FROM (SELECT 1 AS x) a LEFT JOIN (SELECT 1 AS x) b USING x
SELECT a.x FROM (SELECT 1 AS x) a GLOBAL ANY LEFT JOIN (SELECT 1 AS x) b USING x
SELECT a.x FROM (SELECT 1 AS x) a CROSS JOIN (SELECT 1 AS y) b
SELECT a.x FROM (SELECT 1 AS x) a, (SELECT 1 AS y) b
SELECT x FROM (SELECT [1, 2] AS x) ARRAY JOIN x
SELECT x, y FROM (SELECT [1, 2] AS x) ARRAY JOIN x AS y
SELECT x, y FROM (SELECT [1, 2] AS x) LEFT ARRAY JOIN x AS y
SELECT x FROM (SELECT 1 AS x) UNION ALL SELECT 2
SELECT 1 UNION ALL SELECT 2
SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3
WITH 1 AS one SELECT one
WITH (SELECT 1) AS one SELECT one
WITH x AS (SELECT 1) SELECT * FROM x
SELECT number FROM numbers(10) SAMPLE 0.1
SELECT * FROM system.tables WHERE database = 'system'
SELECT name, type FROM system.columns WHERE table = 'numbers'
SELECT database, name FROM system.tables LIMIT 5
SELECT * FROM system.settings LIMIT 5
SELECT version()
SELECT currentDatabase()
SELECT DISTINCT database FROM system.tables
SELECT count() FROM system.tables GROUP BY database
SELECT 1 FROM system.one
SELECT dummy FROM system.one
SELECT * FROM system.one
SELECT * EXCEPT (dummy) FROM system.one
SELECT COLUMNS('d.*') FROM system.one
SELECT * FROM system.one FINAL
SELECT quantile(0.5)(number) FROM numbers(10)
SELECT quantiles(0.5, 0.9)(number) FROM numbers(10)
SELECT groupArray(number) FROM numbers(10)
SELECT groupArray(number) FROM numbers(10) GROUP BY number % 2
SELECT uniq(number) FROM numbers(10)
SELECT topK(3)(number) FROM numbers(10)
SELECT sumIf(number, number > 5) FROM numbers(10)
SELECT countIf(number > 5) FROM numbers(10)
SELECT number, row_number() OVER (ORDER BY number) FROM numbers(10)
EXISTS TABLE system.numbers
SHOW TABLES
SHOW DATABASES
SHOW CREATE TABLE system.numbers
DESCRIBE TABLE system.numbers
DESC system.numbers
USE system
SET max_threads = 1
OPTIMIZE TABLE t
CREATE TABLE t (a UInt32, b String) ENGINE = MergeTree ORDER BY a
CREATE TABLE t (a UInt32, b String, c DateTime DEFAULT now()) ENGINE = MergeTree() PARTITION BY toYYYYMM(c) ORDER BY (a, b)
CREATE TABLE IF NOT EXISTS t (a UInt32) ENGINE = Memory
CREATE TABLE t (a Nullable(String), b Array(UInt8), c LowCardinality(String), d Map(String, UInt8), e Tuple(a UInt8, b String)) ENGINE = Memory
CREATE TABLE t AS SELECT 1 AS a
CREATE TABLE t ENGINE = Memory AS SELECT 1 AS a
CREATE VIEW v AS SELECT 1 AS a
CREATE MATERIALIZED VIEW mv ENGINE = MergeTree ORDER BY a AS SELECT 1 AS a
CREATE DATABASE d
CREATE DATABASE IF NOT EXISTS d ENGINE = Atomic
ALTER TABLE t ADD COLUMN c UInt8
ALTER TABLE t DROP COLUMN c
ALTER TABLE t MODIFY COLUMN c UInt16
ALTER TABLE t UPDATE c = 1 WHERE a = 1
ALTER TABLE t DELETE WHERE a = 1
DROP TABLE t
DROP TABLE IF EXISTS t
DROP DATABASE d
TRUNCATE TABLE t
RENAME TABLE t TO u
INSERT INTO t VALUES (1, 'a')
INSERT INTO t (a, b) VALUES (1, 'a'), (2, 'b')
INSERT INTO t SELECT 1, 'a'
INSERT INTO t FORMAT CSV
INSERT INTO TABLE t VALUES (1)
INSERT INTO FUNCTION file('x.csv') SELECT 1
SYSTEM FLUSH LOGS
KILL QUERY WHERE query_id = 'x'
EXPLAIN SELECT 1
EXPLAIN SYNTAX SELECT 1
EXPLAIN PIPELINE SELECT 1
WATCH lv
