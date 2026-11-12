-- Every SQL statement found in string literals of this repository (tests, demo data, examples) on
-- 0.1.0-alpha.7, extracted by script; fragments the servers reject are fine, they count as broken input.
-- ---
ALTER ROLE "app" PASSWORD '***'
-- ---
ALTER ROLE "app" PASSWORD 'hunter2'
-- ---
ALTER ROLE "app" WITH NOLOGIN CREATEDB CONNECTION LIMIT -1
-- ---
ALTER ROUTINE
-- ---
ALTER SEQUENCE things_id_seq OWNED BY
-- ---
ALTER TABLE a ADD COLUMN b int
-- ---
ALTER TABLE a DROP COLUMN c
-- ---
ALTER TABLE a RENAME COLUMN x TO y
-- ---
ALTER TABLE a RENAME COLUMN y TO z
-- ---
ALTER TABLE big SET (autovacuum_enabled = false)
-- ---
ALTER TABLE customers ADD COLUMN notes TEXT DEFAULT ''
-- ---
ALTER TABLE customers ADD COLUMN score int NULL
-- ---
ALTER TABLE customers ADD CONSTRAINT fk_score FOREIGN KEY (score) REFERENCES scores (id) ON DELETE SET NULL
-- ---
ALTER TABLE customers DROP COLUMN created
-- ---
ALTER TABLE customers DROP CONSTRAINT chk
-- ---
ALTER TABLE customers DROP FOREIGN KEY fk_a
-- ---
ALTER TABLE customers DROP INDEX customers_email_key
-- ---
ALTER TABLE customers DROP PRIMARY KEY
-- ---
ALTER TABLE customers MODIFY COLUMN name varchar(40) NOT NULL COMMENT 'name'
-- ---
ALTER TABLE customers RENAME COLUMN email TO mail
-- ---
ALTER TABLE customers RENAME TO clients
-- ---
ALTER TABLE customers__new RENAME
-- ---
ALTER TABLE customers__new RENAME TO customers
-- ---
ALTER TABLE old_name RENAME TO new_name
-- ---
ALTER TABLE orders
    ADD CONSTRAINT orders_store_id_fkey FOREIGN KEY (store_id) REFERENCES stores (id)
-- ---
alter table orders add column note text
-- ---
alter table orders alter column |
-- ---
alter table orders drop column |
-- ---
ALTER TABLE people ADD COLUMN phone TEXT
-- ---
ALTER TABLE people DROP COLUMN city
-- ---
ALTER TABLE people RENAME COLUMN email TO mail
-- ---
ALTER TABLE people RENAME COLUMN name TO full_name
-- ---
ALTER TABLE public.customers ADD COLUMN phone text
-- ---
ALTER TABLE public.customers ADD COLUMN score integer DEFAULT 0 NOT NULL
-- ---
ALTER TABLE public.customers ADD COLUMN total numeric GENERATED ALWAYS AS (id * 3) STORED
-- ---
ALTER TABLE public.customers ADD CONSTRAINT customers_region_fk FOREIGN KEY (email) REFERENCES public.regions (code)
-- ---
ALTER TABLE public.customers ADD CONSTRAINT name_ok CHECK (length(name) > 0)
-- ---
ALTER TABLE public.customers ADD FOREIGN KEY (email) REFERENCES public.regions (code) ON DELETE CASCADE
-- ---
ALTER TABLE public.customers ADD PRIMARY KEY (id, name)
-- ---
ALTER TABLE public.customers ALTER COLUMN created DROP DEFAULT
-- ---
ALTER TABLE public.customers ALTER COLUMN email SET NOT NULL
-- ---
ALTER TABLE public.customers ALTER COLUMN id SET GENERATED ALWAYS
-- ---
ALTER TABLE public.customers ALTER COLUMN name TYPE varchar(10) USING name::varchar(10)
-- ---
ALTER TABLE public.customers ALTER COLUMN name TYPE varchar(50)
-- ---
ALTER TABLE public.customers DROP COLUMN email
-- ---
ALTER TABLE public.customers DROP COLUMN name
-- ---
ALTER TABLE public.customers DROP COLUMN total
-- ---
ALTER TABLE public.customers DROP CONSTRAINT customers_email_key
-- ---
ALTER TABLE public.customers DROP CONSTRAINT customers_pkey
-- ---
ALTER TABLE public.customers RENAME COLUMN email TO email_address
-- ---
ALTER TABLE public.customers RENAME COLUMN email TO mail
-- ---
ALTER TABLE public.customers RENAME TO clients
-- ---
ALTER TABLE public.orders RENAME COLUMN id TO order_id
-- ---
ALTER TABLE public.things RENAME COLUMN id TO key
-- ---
alter table t add column a int
-- ---
alter table t add column x int default 0, drop column y
-- ---
alter table |
-- ---
ALTER USER 'ann'@'%' ACCOUNT UNLOCK WITH MAX_USER_CONNECTIONS 3
-- ---
ALTER USER 'ann'@'%' IDENTIFIED BY '***'
-- ---
ALTER USER 'ann'@'%' IDENTIFIED BY 'pw'
-- ---
ANALYZE child
-- ---
ANALYZE inner_t
-- ---
ANALYZE orders
-- ---
analyze orders
-- ---
ANALYZE outer_t
-- ---
ANALYZE TABLE
-- ---
ANALYZE TABLE big
-- ---
ANALYZE TABLE child
-- ---
ANALYZE TABLE orders
-- ---
ANALYZE TABLE t
-- ---
BEGIN DEFERRED
-- ---
BEGIN EXCLUSIVE
-- ---
BEGIN IMMEDIATE
-- ---
BEGIN ISOLATION LEVEL REPEATABLE READ
-- ---
BEGIN ISOLATION LEVEL SERIALIZABLE READ ONLY
-- ---
BEGIN READ ONLY
-- ---
BEGIN TRANSACTION
-- ---
Begin Transaction
-- ---
BEGIN; select 1; COMMIT
-- ---
Call on arrival
-- ---
COMMENT ON COLUMN parent.label IS 'A label'
-- ---
COMMENT ON COLUMN public.customers.name IS 'Full name'
-- ---
COMMENT ON TABLE parent IS 'The parent table'
-- ---
COMMENT ON TABLE public.customers IS 'People'
-- ---
COMMENT ON TABLE public.customers IS 'Who buys'
-- ---
COMMENT ON TABLE public.products IS 'Catalog''s products'
-- ---
COMMENT ON TABLE t IS 'héllo wörld
-- ---
Commit and Close
-- ---
Commit and Quit
-- ---
Commit or roll back the open transaction before switching to auto-commit
-- ---
Commit Transaction
-- ---
Copy ALTER TABLE … ADD FOREIGN KEY
-- ---
Copy as INSERT
-- ---
Copy as JSON
-- ---
Copy as WKT point
-- ---
Copy Column Name
-- ---
Copy coordinates
-- ---
Copy every row into the new table
-- ---
Copy feature WKT
-- ---
Copy plan as text
-- ---
COPY public.t (a, b) FROM stdin
-- ---
Copy Qualified Name
-- ---
Copy raw output
-- ---
copy selection to next line
-- ---
copy selection to previous line
-- ---
copy t from stdin
-- ---
COPY t TO '/tmp/x'
-- ---
Copy the highlighted value (Ctrl+C)
-- ---
Copy the script (alt-c)
-- ---
Copy the whole row as name and value lines (Shift+Y)
-- ---
Copy this statement
-- ---
Copy with Headers
-- ---
Create databases
-- ---
CREATE DOMAIN posint AS integer CHECK (VALUE > 0)
-- ---
CREATE EXTENSION IF NOT EXISTS postgis
-- ---
CREATE FULLTEXT INDEX ix ON stores (location)
-- ---
CREATE FUNCTION add_one(x int) RETURNS int DETERMINISTIC RETURN x + 1
-- ---
CREATE FUNCTION add_one(x int) RETURNS int LANGUAGE sql AS 'SELECT x + 1'
-- ---
CREATE FUNCTION f() RETURNS int AS $$
-- ---
create function f() returns int as $$
begin
    select   1;
end;
$$ language plpgsql
-- ---
create function f() returns int as $$ begin delete from t; end $$ language plpgsql
-- ---
create function f() returns int as $$ begin select 1; return 2; end; $$ language plpgsql; select 3
-- ---
create function f() returns int as $$ begin update foo set a = 1; return 1; end; $$ language plpgsql
-- ---
create function f() returns int as $$ select $1 + :x $$ language sql
-- ---
CREATE FUNCTION f() RETURNS int LANGUAGE sql BEGIN ATOMIC SELECT 1; SELECT 2; END; SELECT 3
-- ---
create function f(a int) returns int as $$ begin return a + 1; end; $$ language plpgsql
-- ---
create function f(a int) returns int as $fn$ begin update t set a = 1; return a; end; $fn$ language plpgsql;
update orders o set status = 'paid' from customers c where o.customer_id = c.id;
delete from orders where id in (select order_id from order_items where quantity = 0);
select data->>'k', arr[1], x::int, E'a\'b' from t where a is distinct from null
-- ---
CREATE FUNCTION f(x INT) RETURNS INT BEGIN RETURN CASE WHEN x > 1 THEN 1 ELSE 0 END; END; SELECT 1
-- ---
Create if missing
-- ---
CREATE INDEX
-- ---
CREATE INDEX a_x_idx ON a (x)
-- ---
CREATE INDEX child_amount_idx ON child USING hash (amount)
-- ---
CREATE INDEX child_expr ON child (lower(note))
-- ---
CREATE INDEX child_multi_idx ON child (amount, note)
-- ---
CREATE INDEX customers_lower_idx ON public.customers USING btree (lower(email))
-- ---
CREATE INDEX customers_mail_idx ON customers (mail) WHERE mail IS NOT NULL
-- ---
CREATE INDEX customers_name_idx ON customers (name)
-- ---
CREATE INDEX customers_name_idx ON public.customers (name)
-- ---
create index i on orders (|)
-- ---
CREATE INDEX i ON t (a) WHERE a IS NOT NULL AND b > 1
-- ---
create index i on |
-- ---
create index idx on orders (customer_id)
-- ---
create index idx on t using gin (a, b) where c > 1
-- ---
CREATE INDEX idx_a ON a (b)
-- ---
CREATE INDEX idx_big_id ON big (id)
-- ---
CREATE INDEX idx_big_k ON big (k)
-- ---
CREATE INDEX idx_inner_t_k ON inner_t (k)
-- ---
CREATE INDEX idx_order_items_product ON order_items (product_id)
-- ---
CREATE INDEX idx_orders_customer_id ON orders (customer_id)
-- ---
CREATE INDEX idx_orders_status ON orders (status)
-- ---
CREATE INDEX idx_orders_status_ordered_at ON orders (status, ordered_at)
-- ---
CREATE INDEX idx_orders_status_store_id_placed_at ON orders (status, store_id, placed_at)
-- ---
CREATE INDEX idx_products_category ON products (category)
-- ---
CREATE INDEX ix ON stores (location)
-- ---
CREATE INDEX ix ON stores (location) USING HASH
-- ---
CREATE INDEX mine ON public.customers (name)
-- ---
CREATE INDEX orders_customer_id_idx ON public.orders (customer_id)
-- ---
CREATE INDEX orders_customer_idx ON public.orders (customer_id)
-- ---
CREATE INDEX products_sku ON products (sku)
-- ---
CREATE INDEX t_active ON t (lower(email)) WHERE qty > 0
-- ---
CREATE INDEX t_part ON t (qty) WHERE qty > 0
-- ---
CREATE INDEX tv ON t (v)
-- ---
CREATE MATERIALIZED VIEW child_mv AS SELECT id FROM child
-- ---
CREATE PROCEDURE do_nothing() LANGUAGE sql AS ''
-- ---
CREATE PROCEDURE do_nothing(IN a int, OUT b text) BEGIN SELECT 1; END
-- ---
CREATE PROCEDURE p AS BEGIN BEGIN TRANSACTION; UPDATE t SET a = 1; COMMIT; END
-- ---
create procedure p() begin select 1
-- ---
CREATE PROCEDURE p() BEGIN SELECT 1; IF x THEN SELECT 2; END IF; CASE WHEN a THEN SELECT 3; END CASE; END; SELECT 9
-- ---
create procedure p() begin select 1; update t set a = 1; end
-- ---
create procedure p() begin update t set a = 1; end
-- ---
create procedure p() begin update t set a = 1; select 1; end
-- ---
CREATE ROUTINE
-- ---
CREATE SEQUENCE extra_seq
-- ---
CREATE SEQUENCE IF NOT EXISTS things_id_seq
-- ---
CREATE SPATIAL INDEX ix ON stores (location)
-- ---
CREATE TABLE
-- ---
Create Table
-- ---
CREATE TABLE "select" (a text DEFAULT 'drop table')
-- ---
CREATE TABLE `a;b` (x int);
INSERT INTO `a;b` VALUES (1)
-- ---
CREATE TABLE a (id int)
-- ---
CREATE TABLE a (id int)
GO
INSERT INTO a VALUES (1)
GO 3
SELECT 1 AS go_col
-- ---
CREATE TABLE a (id INTEGER PRIMARY KEY, x TEXT)
-- ---
CREATE TABLE a (id INTEGER PRIMARY KEY, x TEXT, y TEXT)
-- ---
CREATE TABLE a AS SELECT g AS id FROM generate_series(1, 100) g
-- ---
CREATE TABLE b AS SELECT g AS id FROM generate_series(1, 100) g
-- ---
CREATE TABLE big (id int NOT NULL, k int NOT NULL, grp int NOT NULL, pad varchar(40) NOT NULL) ENGINE=InnoDB
-- ---
CREATE TABLE big (id INTEGER PRIMARY KEY, k INTEGER, pad TEXT)
-- ---
CREATE TABLE big AS SELECT g AS id, g % 5000 AS k, md5(g::text) AS pad FROM generate_series(1, 200000) g
-- ---
CREATE TABLE child (id int AUTO_INCREMENT PRIMARY KEY, pa int, pb int, note varchar(40), amount decimal(10,2), doc json, flag bit(1),
           CONSTRAINT fk_child_parent FOREIGN KEY (pa, pb) REFERENCES parent (a, b) ON DELETE CASCADE ON UPDATE SET NULL)
-- ---
CREATE TABLE child (id INTEGER PRIMARY KEY AUTOINCREMENT, pa INTEGER, pb INTEGER, note VARCHAR(40), FOREIGN KEY (pa, pb) REFERENCES parent (a, b) ON DELETE CASCADE ON UPDATE SET NULL)
-- ---
CREATE TABLE child (id serial PRIMARY KEY, pa int, pb int, note varchar(40), amount numeric(10,2), FOREIGN KEY (pa, pb) REFERENCES parent (a, b) ON DELETE CASCADE ON UPDATE SET NULL)
-- ---
CREATE TABLE customers (
    id int NOT NULL AUTO_INCREMENT,
    name varchar(100) NOT NULL COMMENT 'it''s the name',
    email varchar(100) NULL,
    created datetime NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (id),
    CONSTRAINT customers_email_key UNIQUE (email)
) COMMENT = 'People'
-- ---
CREATE TABLE customers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    email TEXT,
    created TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT customers_email_key UNIQUE (email)
)
-- ---
CREATE TABLE customers (
  id INTEGER PRIMARY KEY,
  first_name VARCHAR(60) NOT NULL,
  last_name VARCHAR(60) NOT NULL,
  email VARCHAR(160) NOT NULL UNIQUE,
  city VARCHAR(80),
  country VARCHAR(80),
  created_at TIMESTAMP NOT NULL,
  marketing_opt_in BOOLEAN NOT NULL DEFAULT 1
)
-- ---
CREATE TABLE customers__new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name VARCHAR(40) NOT NULL,
    mail TEXT,
    created TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT customers_email_key UNIQUE (mail)
)
-- ---
create table ddl_refresh_t(a int)
-- ---
CREATE TABLE delivery_zones (
  id INTEGER PRIMARY KEY,
  store_id INTEGER NOT NULL REFERENCES stores(id),
  name VARCHAR(120) NOT NULL,
  area_wkt TEXT NOT NULL
)
-- ---
CREATE TABLE dst (id int, g geometry)
-- ---
CREATE TABLE dyn (i INTEGER)
-- ---
CREATE TABLE ended the transaction
-- ---
CREATE TABLE fresh (
    id integer NOT NULL,
    name text
)
-- ---
CREATE TABLE half_done (id INTEGER)
-- ---
CREATE TABLE inner_t AS SELECT g AS id, g % 100 AS k FROM generate_series(1, 5000) g
-- ---
CREATE TABLE k (id INTEGER PRIMARY KEY, a TEXT, b TEXT)
-- ---
CREATE TABLE keep (a int)
-- ---
CREATE TABLE log (msg TEXT)
-- ---
CREATE TABLE m (a INTEGER)
-- ---
CREATE TABLE nokey (x integer, y text)
-- ---
CREATE TABLE nope (a int)
-- ---
CREATE TABLE old_name (id INTEGER PRIMARY KEY)
-- ---
CREATE TABLE order_items (
  order_id INTEGER NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
  line_no INTEGER NOT NULL,
  product_id INTEGER NOT NULL REFERENCES products(id),
  quantity INTEGER NOT NULL,
  unit_price NUMERIC NOT NULL,
  PRIMARY KEY (order_id, line_no)
)
-- ---
CREATE TABLE orders (
  id INTEGER PRIMARY KEY,
  customer_id INTEGER NOT NULL REFERENCES customers(id),
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','processing','shipped','delivered','cancelled','refunded')),
  ordered_at TIMESTAMP NOT NULL,
  shipped_at TIMESTAMP,
  total NUMERIC NOT NULL,
  note TEXT
)
-- ---
CREATE TABLE other.o (a INTEGER)
-- ---
CREATE TABLE outer_t AS SELECT g AS id, g AS k FROM generate_series(1, 300) g
-- ---
CREATE TABLE parent (a int, b int, label text NOT NULL DEFAULT 'x', PRIMARY KEY (a, b))
-- ---
CREATE TABLE parent (a int, b int, label varchar(20) NOT NULL DEFAULT 'x' COMMENT 'A label', PRIMARY KEY (a, b)) COMMENT='The parent table'
-- ---
CREATE TABLE parent (a INTEGER, b INTEGER, label TEXT NOT NULL DEFAULT 'x', PRIMARY KEY (a, b))
-- ---
CREATE TABLE parent (id integer PRIMARY KEY)
-- ---
CREATE TABLE parted (id int, d date) PARTITION BY RANGE (d)
-- ---
CREATE TABLE people (id int PRIMARY KEY, name text)
-- ---
CREATE TABLE people (id int PRIMARY KEY, name varchar(20))
-- ---
CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT)
-- ---
CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT, city TEXT)
-- ---
CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT, email TEXT, city TEXT)
-- ---
CREATE TABLE pets (id int PRIMARY KEY, owner int, name text)
-- ---
CREATE TABLE pets (id int PRIMARY KEY, owner int, name varchar(20))
-- ---
CREATE TABLE places (id int, g polygon NOT NULL, any_g geometry, SPATIAL INDEX (g))
-- ---
CREATE TABLE plain (a INTEGER PRIMARY KEY, b TEXT)
-- ---
CREATE TABLE products (
  id INTEGER PRIMARY KEY,
  sku VARCHAR(20) NOT NULL UNIQUE,
  name VARCHAR(120) NOT NULL,
  category VARCHAR(40) NOT NULL,
  price NUMERIC NOT NULL,
  cost NUMERIC NOT NULL,
  attributes JSON,
  in_stock INTEGER NOT NULL DEFAULT 0
)
-- ---
CREATE TABLE products (id INTEGER PRIMARY KEY AUTOINCREMENT, sku TEXT NOT NULL, price REAL DEFAULT 9.5, photo BLOB, note TEXT)
-- ---
CREATE TABLE products (id INTEGER PRIMARY KEY, sku VARCHAR NOT NULL, price DOUBLE DEFAULT 9.5, photo BLOB, note VARCHAR)
-- ---
CREATE TABLE public."Order Items" (
-- ---
CREATE TABLE public.customers (
    id integer GENERATED BY DEFAULT AS IDENTITY NOT NULL,
    name text NOT NULL,
    email text,
    created timestamp DEFAULT now(),
    PRIMARY KEY (id),
    CONSTRAINT customers_email_key UNIQUE (email),
    CONSTRAINT name_nonempty CHECK (length(name) > 0),
    CONSTRAINT customers_region_fk FOREIGN KEY (email) REFERENCES public.regions (code) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
)
-- ---
CREATE TABLE public.orders (
-- ---
CREATE TABLE public.orders (
    id integer NOT NULL,
-- ---
CREATE TABLE public.t (
    id integer NOT NULL,
    "order" text
)
-- ---
CREATE TABLE public.things (
    id integer GENERATED BY DEFAULT AS IDENTITY NOT NULL,
    PRIMARY KEY (id)
)
-- ---
CREATE TABLE ro_probe (a int)
-- ---
CREATE TABLE routes (
  id INTEGER PRIMARY KEY,
  store_id INTEGER NOT NULL REFERENCES stores(id),
  name VARCHAR(120) NOT NULL,
  distance_km NUMERIC NOT NULL,
  path_wkt TEXT NOT NULL
)
-- ---
CREATE TABLE shop.customers
-- ---
CREATE TABLE spatial (id int, g geometry(Polygon, 4326), anyg geometry, geo geography(Point, 4326))
-- ---
CREATE TABLE src (id int, g geometry)
-- ---
CREATE TABLE stock (store_id INTEGER NOT NULL REFERENCES stores(id) ON DELETE CASCADE, product_id INTEGER NOT NULL REFERENCES products(id), qty INTEGER NOT NULL DEFAULT 0 CHECK (qty >= 0), PRIMARY KEY (store_id, product_id))
-- ---
CREATE TABLE stock (store_id INTEGER NOT NULL REFERENCES stores(id), product_id INTEGER NOT NULL REFERENCES products(id), qty INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (store_id, product_id))
-- ---
CREATE TABLE stores (
  id INTEGER PRIMARY KEY,
  name VARCHAR(120) NOT NULL,
  city VARCHAR(80) NOT NULL,
  country VARCHAR(80) NOT NULL,
  opened_on DATE NOT NULL,
  longitude REAL NOT NULL,
  latitude REAL NOT NULL,
  location_wkt TEXT NOT NULL
)
-- ---
CREATE TABLE stores (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, opened DATE)
-- ---
create table stores (id integer primary key, name text)
-- ---
CREATE TABLE stores (id INTEGER PRIMARY KEY, name VARCHAR NOT NULL UNIQUE, opened DATE)
-- ---
CREATE TABLE t (
           ti tinyint, si smallint, mi mediumint, i int, bi bigint,
           uti tinyint unsigned, ubi bigint unsigned,
           d decimal(10,2), f float, db double,
           dt date, tm time, dtm datetime(6), ts timestamp NULL, yr year,
           c char(5), vc varchar(20), tx text, tt tinytext, bl blob, vb varbinary(8), bn binary(4),
           j json, b1 bit(1), b8 bit(8), en enum('small','large'), st set('a','b','c'),
           g point NOT NULL, bo boolean, nul int NULL)
-- ---
CREATE TABLE t (
    id serial PRIMARY KEY,
    name text NOT NULL,
    CONSTRAINT fk FOREIGN KEY (id) REFERENCES u (id)
)
-- ---
create table t (a int default null)
-- ---
CREATE TABLE t (a INT PRIMARY KEY) WITHOUT ROWID
-- ---
create table t (a int)
-- ---
CREATE TABLE t (a int) ENGINE=InnoDB
-- ---
create table t (a int); insert into t values (1)
-- ---
CREATE TABLE t (a INTEGER PRIMARY KEY);
INSERT INTO t VALUES (1);
INSERT INTO nope VALUES (1);
INSERT INTO t VALUES (2);
INSERT INTO t VALUES (1);
INSERT INTO t VALUES (3)
-- ---
CREATE TABLE t (a INTEGER)
-- ---
CREATE TABLE t (a)
-- ---
CREATE TABLE t (i INTEGER, r REAL, s TEXT, b BLOB, n NUMERIC, v VARCHAR(10), flag BOOLEAN, j JSON, d DATE, ts DATETIME, x)
-- ---
CREATE TABLE t (id int AUTO_INCREMENT PRIMARY KEY, v int, UNIQUE KEY uq (v))
-- ---
CREATE TABLE t (id int NOT NULL AUTO_INCREMENT PRIMARY KEY, price decimal(10,2) NOT NULL, qty int NOT NULL DEFAULT 1, total decimal(12,2) GENERATED ALWAYS AS (price * qty) STORED, label varchar(20) GENERATED ALWAYS AS (concat('#', id)) VIRTUAL, email varchar(100), CONSTRAINT email_unique UNIQUE (email), CONSTRAINT price_pos CHECK (price > 0)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4
-- ---
create table t (id int primary key, n varchar(10)); select x::text, y::my_type
-- ---
CREATE TABLE t (id int PRIMARY KEY, v int)
-- ---
CREATE TABLE t (id int PRIMARY KEY, v int, KEY tv (v)) ENGINE=InnoDB
-- ---
CREATE TABLE t (id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY, alt integer GENERATED BY DEFAULT AS IDENTITY, price numeric(10,2) NOT NULL CHECK (price > 0), qty integer NOT NULL DEFAULT 1, total numeric GENERATED ALWAYS AS (price * qty) STORED, email text, parent_id integer CONSTRAINT t_parent_fk REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED, CONSTRAINT email_unique UNIQUE (email), CONSTRAINT qty_nonneg CHECK (qty >= 0))
-- ---
CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, price REAL CHECK (price > 0), qty INT, total REAL GENERATED ALWAYS AS (price * qty) STORED, name TEXT CONSTRAINT name_len CHECK (length(name) < 'a,b'), CONSTRAINT qty_ok CHECK (qty >= 0), UNIQUE (name, qty))
-- ---
CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, price REAL NOT NULL CHECK (price > 0), qty INT DEFAULT 1, total REAL GENERATED ALWAYS AS (price * qty) STORED, double_it INT AS (qty * 2), email TEXT, CONSTRAINT qty_ok CHECK (qty >= 0), UNIQUE (email))
-- ---
CREATE TABLE t (id INTEGER PRIMARY KEY, v INTEGER)
-- ---
CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)
-- ---
create table t (id integer primary key, v text); insert into t values (1, 'one'), (2, 'two')
-- ---
CREATE TABLE t (id INTEGER PRIMARY KEY, v)
-- ---
CREATE TABLE t (id integer, name text)
-- ---
create table t (id serial primary key, name text not null default '', c timestamptz default now(), constraint fk foreign key (id) references u(id) on delete cascade)
-- ---
create table t (id serial primary key, name text not null, constraint fk foreign key (id) references u(id))
-- ---
CREATE TABLE t (id serial PRIMARY KEY, v int)
-- ---
CREATE TABLE t (name text)
-- ---
CREATE TABLE t (x INTEGER)
-- ---
CREATE TABLE t(a int)
-- ---
create table t(a int)
-- ---
CREATE TABLE things (
-- ---
CREATE TABLE this is not sql
-- ---
CREATE TABLE u (a UNIQUE)
-- ---
CREATE TABLE with an id and a timestamp
-- ---
create table |
-- ---
create temp table scratch (a int); select * from scratch
-- ---
CREATE TEMPORARY TABLES
-- ---
CREATE TRIGGER
-- ---
CREATE TRIGGER a_log
-- ---
CREATE TRIGGER a_log AFTER INSERT ON a BEGIN INSERT INTO log VALUES (new.x); END
-- ---
CREATE TRIGGER t AFTER INSERT ON a BEGIN UPDATE b SET x = 1; INSERT INTO c VALUES (1); END; SELECT 1
-- ---
CREATE TRIGGER t1 AFTER INSERT ON customers BEGIN SELECT 1; END
-- ---
create trigger tr after insert on t begin update u set a = 1; end
-- ---
CREATE TRIGGER trg
-- ---
CREATE TRIGGER trg AFTER INSERT ON customers BEGIN SELECT 1; END
-- ---
CREATE TYPE mood AS ENUM ('sad','ok')
-- ---
CREATE TYPE mood AS ENUM ('sad','ok','happy')
-- ---
CREATE TYPE pair AS (a integer, b text)
-- ---
CREATE UNIQUE INDEX
-- ---
CREATE UNIQUE INDEX `child_note_idx`
-- ---
CREATE UNIQUE INDEX child_note
-- ---
CREATE UNIQUE INDEX child_note ON child (note)
-- ---
CREATE UNIQUE INDEX child_note_idx
-- ---
CREATE UNIQUE INDEX child_note_idx ON child (lower(note))
-- ---
CREATE UNIQUE INDEX child_note_idx ON child (note)
-- ---
CREATE UNIQUE INDEX customers_email_lower ON public.customers (email) WHERE email IS NOT NULL
-- ---
CREATE UNIQUE INDEX products_sku_key ON public.products USING btree (sku)
-- ---
CREATE VIEW child_v AS SELECT id, note FROM child
-- ---
CREATE VIEW child_view AS SELECT id, note FROM child
-- ---
CREATE VIEW customer_order_summary AS
SELECT c.id AS customer_id,
       c.first_name || ' ' || c.last_name AS customer_name,
       c.country,
       COUNT(o.id) AS order_count,
       COALESCE(SUM(o.total), 0) AS total_spent,
       MAX(o.ordered_at) AS last_order_at
FROM customers c
LEFT JOIN orders o ON o.customer_id = c.id AND o.status NOT IN ('cancelled', 'refunded')
GROUP BY c.id, c.first_name, c.last_name, c.country
-- ---
CREATE VIEW low_stock AS SELECT store_id, product_id FROM stock WHERE qty < 5
-- ---
CREATE VIEW v AS
SELECT a
FROM t
-- ---
CREATE VIEW v AS SELECT 1 AS x
-- ---
create view v as select a from t
-- ---
create view v as select a, b from t where c > 1
-- ---
CREATE VIEW v AS SELECT id, price FROM t
-- ---
CREATE VIEW v1 AS SELECT id FROM customers
-- ---
create-missing
-- ---
Delete every row
-- ---
Delete Folder
-- ---
Delete Folder…
-- ---
DELETE FROM "Order Items" WHERE "Group" = 1
-- ---
delete from customers where |
-- ---
delete from orders
-- ---
delete from orders o using customers c where true
-- ---
delete from orders where id = 1
-- ---
delete from orders where id in (select order_id from order_items where quantity = 0)
-- ---
DELETE FROM public.customers WHERE id = 9
-- ---
DELETE FROM public.orders
WHERE id = NULL
-- ---
DELETE FROM s.t
-- ---
DELETE FROM shop.`order` WHERE id = 1
-- ---
DELETE FROM t
-- ---
delete from t
-- ---
DELETE FROM t
WHERE id = 1
-- ---
DELETE FROM t WHERE id = 5000
-- ---
delete from t where id=1
-- ---
DELETE FROM t WHERE v = 1
-- ---
delete from |
-- ---
DELETE FROM … WHERE …
-- ---
delete history
-- ---
Delete Row(s)
-- ---
Delete selected rows (Delete)
-- ---
delete selection
-- ---
delete t from t join u on t.a = u.a where u.b = 1
-- ---
delete t from t join u on t.id = u.id where u.x = 1
-- ---
delete t where a = 1
-- ---
DELETE without WHERE
-- ---
delete without yanking
-- ---
DROP COLUMN email
-- ---
Drop Column…
-- ---
DROP DEFAULT
-- ---
Drop Foreign Key
-- ---
Drop Foreign Key…
-- ---
DROP IDENTITY
-- ---
DROP INDEX a_x_idx
-- ---
DROP INDEX customers_name_idx ON customers
-- ---
DROP INDEX i
-- ---
DROP INDEX public.customers_name_idx
-- ---
DROP NOT NULL
-- ---
DROP TABLE a
-- ---
DROP TABLE customers
-- ---
drop table ddl_refresh_t
-- ---
DROP TABLE IF EXISTS accounts__new
-- ---
drop table if exists ddl_refresh_t; create table ddl_refresh_t(a int)
-- ---
drop table if exists public.ddl_refresh_t; create table public.ddl_refresh_t(a int)
-- ---
drop table public.ddl_refresh_t
-- ---
drop table t
-- ---
drop table |
-- ---
Drop the primary key
-- ---
DROP VIEW IF EXISTS customer_order_summary
-- ---
Drop which table…
-- ---
EXPLAIN (ANALYZE, BUFFERS) $0
-- ---
explain (analyze, buffers) select * from t where x = 1
-- ---
explain (analyze, buffers) update t set a = 1
-- ---
EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
-- ---
EXPLAIN (ANALYZE, DIST) $0
-- ---
EXPLAIN (ANALYZE, FORMAT JSON)
-- ---
EXPLAIN (FORMAT JSON)
-- ---
EXPLAIN a statement
-- ---
EXPLAIN ANALYZE
-- ---
Explain Analyze
-- ---
explain analyze
-- ---
EXPLAIN ANALYZE
SELECT *
FROM t
-- ---
EXPLAIN ANALYZE $0
-- ---
explain analyze delete from t
-- ---
EXPLAIN ANALYZE of a write inside an open transaction would keep its changes; commit or roll back first
-- ---
EXPLAIN ANALYZE returned no plan
-- ---
Explain Analyze runs the statement
-- ---
explain analyze select * from orders where id = 1
-- ---
explain analyze select * from t
-- ---
explain analyze sel|
-- ---
explain delete from t
-- ---
EXPLAIN output has no Plan
-- ---
EXPLAIN output has no query_block
-- ---
EXPLAIN output has no top-level object
-- ---
EXPLAIN output is not a JSON object
-- ---
EXPLAIN PLAN actions = 1 $0
-- ---
EXPLAIN QUERY PLAN $0
-- ---
EXPLAIN QUERY PLAN returned no rows
-- ---
EXPLAIN QUERY PLAN select 1
-- ---
EXPLAIN returned no plan nodes
-- ---
EXPLAIN returned no rows
-- ---
EXPLAIN SELECT 1
-- ---
explain select 1
-- ---
Explain the statement under the cursor to see how the database runs it.
-- ---
GRANT "readers" TO "app"
-- ---
GRANT `staff` TO 'ann'@'%'
-- ---
GRANT `staff` TO `ann`@`%`
-- ---
GRANT ALL PRIVILEGES ON *.* TO 'ann'@'%'
-- ---
GRANT ALL PRIVILEGES ON *.* TO `quarry`@`localhost` IDENTIFIED BY PASSWORD '*13'
-- ---
Grant Privileges
-- ---
GRANT SELECT (id, name), UPDATE (name) ON `shop`.`stores` TO `ann`@`%` WITH GRANT OPTION
-- ---
GRANT SELECT ON `shop`.* TO 'ann'@'%'
-- ---
GRANT SELECT, INSERT ON `shop`.* TO `ann`@`%`
-- ---
GRANT SELECT, UPDATE ON TABLE "public"."orders" TO "app" WITH GRANT OPTION
-- ---
insert at column 0
-- ---
insert at last insert
-- ---
insert at line start
-- ---
insert before selection
-- ---
INSERT INTO `order` (id, Name) VALUES (1, 'O''Neil')
-- ---
INSERT INTO a (x) VALUES ('hi')
-- ---
INSERT INTO a VALUES (1)
-- ---
insert into analytics.events (|
-- ---
INSERT INTO big WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < 200000) SELECT n, n % 5000, n % 50, md5(n) FROM s
-- ---
INSERT INTO child (pa, pb) SELECT 1, 1 FROM generate_series(1, 3)
-- ---
INSERT INTO child (pa, pb) VALUES (1, 1)
-- ---
INSERT INTO child (pa, pb) VALUES (1,1)
-- ---
Insert into Console
-- ---
insert into customers (a) values (1) on |
-- ---
INSERT INTO customers (first_name, last_name, email, city, country, created_at, marketing_opt_in)
-- ---
INSERT INTO customers (first_name, last_name, email, created_at) VALUES ('a', 'b', 'roundtrip@example.test', now())
-- ---
insert into customers (id) values (1) on conflict (id) do update set name = excluded.|
-- ---
insert into customers (id) values (1) on conflict (id) do update set |
-- ---
insert into customers (id) values (1) on conflict (|) do nothing
-- ---
INSERT INTO customers (id, email)
VALUES
    (1, 'a'),
    (2, 'b')
ON CONFLICT (id) DO UPDATE
SET email = excluded.email
RETURNING id
-- ---
insert into customers (id, email) values (1, 'a'), (2, 'b') on conflict (id) do update set email = excluded.email returning id
-- ---
insert into customers (id, email) values (1, 'a@b.c') on conflict (id) do update set email = excluded.email returning id
-- ---
insert into customers (id, email) values (1, 'x')
-- ---
insert into customers (id, emial) values (1, 'x')
-- ---
insert into customers (id, |
-- ---
insert into customers (name) select | from products
-- ---
insert into customers (|) values (1)
-- ---
INSERT INTO customers__new (id, name, mail, created) SELECT id, name, email, created FROM customers
-- ---
INSERT INTO delivery_zones (store_id, name, area_wkt)
-- ---
INSERT INTO dyn VALUES ('not a number')
-- ---
INSERT INTO nokey (x, y) VALUES (0, 'v0')
-- ---
INSERT INTO nope VALUES (1)
-- ---
INSERT INTO order_items (order_id, line_no, product_id, quantity, unit_price)
-- ---
INSERT INTO orders (customer_id, status, ordered_at, shipped_at, total, note)
-- ---
insert into orders (id) select id from customers
-- ---
INSERT INTO parent VALUES (1, 1)
-- ---
INSERT INTO parent VALUES (1, 1, 'a')
-- ---
INSERT INTO parent VALUES (1,1,'a')
-- ---
INSERT INTO people VALUES (1, 'ann')
-- ---
INSERT INTO people VALUES (1, 'ann', 'a@x', 'Oslo')
-- ---
INSERT INTO people VALUES (1,'ann')
-- ---
INSERT INTO pets VALUES (1,1,'rex')
-- ---
INSERT INTO products (sku, name, category, price, cost, attributes, in_stock)
-- ---
INSERT INTO products (sku, price, photo, note) VALUES ('a-1', 1.25, x'00ff10', 'line1
line2 -- not a comment'), ('b-2', NULL, NULL, 'ünïcode ✓ 漢字'), ('c-3', 3, x'', '')
-- ---
INSERT INTO products VALUES (1, 'a-1', 1.25, '\x00\xff\x10'::BLOB, 'line1
line2 -- not a comment'), (2, 'b-2', NULL, NULL, 'ünïcode ✓ 漢字'), (3, 'c-3', 3, ''::BLOB, '')
-- ---
INSERT INTO public.customers (name) VALUES ('new')
-- ---
INSERT INTO public.orders (id, customer_id, "Total Price")
VALUES (
-- ---
INSERT INTO public.t (id, "Name") VALUES (1, 'O''Neil')
-- ---
INSERT INTO routes (store_id, name, distance_km, path_wkt)
-- ---
INSERT INTO src VALUES (1, ST_GeomFromText('POINT(1 2)', 4326)), (2, ST_GeomFromText('POLYGON((0 0,4 0,4 4,0 0))', 3857)), (3, ST_GeomFromText('POINT(5 6)'))
-- ---
INSERT INTO stock VALUES (1, 1, 10), (1, 2, 3), (2, 3, 0)
-- ---
INSERT INTO stores (name, city, country, opened_on, longitude, latitude, location_wkt)
-- ---
INSERT INTO stores VALUES (1, 'Main St', '2020-01-02'), (2, 'O''Brien''s; "quoted"', NULL)
-- ---
INSERT INTO stores VALUES (1, 'Main St', DATE '2020-01-02'), (2, 'O''Brien''s; "quoted"', NULL)
-- ---
INSERT INTO t () VALUES ()
-- ---
INSERT INTO t (a)
VALUES (1)
-- ---
insert into t (a) values (1)
-- ---
insert into t (a) values (1) on conflict (a) do update set a = 2
-- ---
insert into t (a) values (1) on conflict do nothing
-- ---
INSERT INTO t (a) VALUES (1),
       (2)
-- ---
insert into t (a) values (null)
-- ---
INSERT INTO t (a, b)
VALUES (1, 2)
ON DUPLICATE KEY UPDATE
    b = VALUES (b),
    a = a + 1
-- ---
insert into t (a, b) values (1, 2) on duplicate key update b = values(b)
-- ---
insert into t (a, b) values (1, 2) on duplicate key update b = values(b), a = a + 1
-- ---
INSERT INTO t (a, b) VALUES (1, NULL)
-- ---
insert into t (a, b,) values (1, 2)
-- ---
INSERT INTO t (g) VALUES (ST_GeomFromText('POINT(0 0)'))
-- ---
INSERT INTO t (i) VALUES (NULL)
-- ---
INSERT INTO t (id, name) OVERRIDING SYSTEM VALUE VALUES
(0, 'n0'),
(1, 'n1');
INSERT INTO t (id, name) OVERRIDING SYSTEM VALUE VALUES
(2, 'n2'),
(3, 'n3');
INSERT INTO t (id, name) OVERRIDING SYSTEM VALUE VALUES
(4, 'n4')
-- ---
INSERT INTO t (v) VALUES (1),(2),(3)
-- ---
INSERT INTO t (v) VALUES (10),(11) RETURNING id, v
-- ---
INSERT INTO t (v) VALUES (7)
-- ---
INSERT INTO t (v) VALUES (9) RETURNING id
-- ---
INSERT INTO t DEFAULT VALUES
-- ---
INSERT INTO t SELECT * FROM range(5)
-- ---
insert into t select * from u where x = 1
-- ---
INSERT INTO t SELECT g, g % 10 FROM generate_series(1, 1000) g
-- ---
INSERT INTO t SELECT seq, seq % 10 FROM seq_1_to_1000
-- ---
INSERT INTO t VALUES (1)
-- ---
insert into t values (1)
-- ---
insert into t values (1) on conflict do update set a = 1
-- ---
insert into t values (1) on conflict do update set a = 2
-- ---
INSERT INTO t VALUES (1, 'a'), (2, NULL)
-- ---
INSERT INTO t VALUES (2)
-- ---
INSERT INTO t VALUES (42)
-- ---
INSERT INTO t VALUES (5000, 1)
-- ---
INSERT INTO t VALUES (6000, 1)
-- ---
INSERT INTO t VALUES (6001, 1)
-- ---
INSERT INTO t VALUES (7000, 1)
-- ---
INSERT INTO t VALUES (7001, 1)
-- ---
INSERT INTO t VALUES (9999, 1)
-- ---
INSERT INTO t__new
-- ---
INSERT INTO u VALUES (1)
-- ---
insert into |
-- ---
INSERT INTO … VALUES …
-- ---
Insert mode / back to normal
-- ---
Insert mode may stand after it
-- ---
Insert mode shows nothing
-- ---
INSERT OR REPLACE INTO t (a)
VALUES (1)
-- ---
insert or replace into t (a) values (1)
-- ---
Insert register
-- ---
merge consecutive selections
-- ---
merge into t using s on true when matched then delete
-- ---
merge selections
-- ---
PRAGMA foreign_key_check
-- ---
pragma foreign_key_check
-- ---
PRAGMA foreign_keys
-- ---
PRAGMA foreign_keys = OFF
-- ---
PRAGMA foreign_keys = ON
-- ---
pragma foreign_keys = on
-- ---
PRAGMA settings such as user_version and the journal mode
-- ---
pragma table_info(t)
-- ---
pragma user_version
-- ---
REPLACE INTO
-- ---
replace into t values (1)
-- ---
replace with character
-- ---
replace with yanked text
-- ---
REVOKE "readers" FROM "app"
-- ---
REVOKE ALL ON ALL TABLES IN SCHEMA "public" FROM "app"
-- ---
Revoke Privileges
-- ---
rollback must have removed the table
-- ---
ROLLBACK TO SAVEPOINT a
-- ---
ROLLBACK TO SAVEPOINT theta_explain
-- ---
ROLLBACK TO SAVEPOINT theta_explain; RELEASE SAVEPOINT theta_explain
-- ---
ROLLBACK TO theta_explain; RELEASE theta_explain
-- ---
ROLLBACK TRANSACTION
-- ---
ROLLBACK TRANSACTION theta_explain
-- ---
SAVEPOINT theta_explain
-- ---
SELECT
    -1,
    3 - -4,
    a - b,
    -2
-- ---
SELECT
    a,
    b
FROM t
WHERE x = 1
-- ---
SELECT
    c.id,
    c.name,
    count(o.id) AS n
FROM customers c
LEFT JOIN orders o ON o.customer_id = c.id AND o.status = 'paid'
WHERE c.tier IN ('gold', 'silver')
GROUP BY c.id, c.name
HAVING count(o.id) > 2
ORDER BY n DESC
LIMIT 10
-- ---
SELECT
    CASE
        WHEN a > 1 THEN 'x'
        WHEN a > 0 THEN 'y'
        ELSE 'z'
    END AS c
FROM t
-- ---
SELECT
    id,
    customer_id,
    "Total Price"
FROM public.orders
LIMIT 100
-- ---
SELECT
  a,
  b
FROM t
-- ---
SELECT 
  FROM t
-- ---
select  1  from a
-- ---
select  1  from a;
select  2  from b;
select  3  from c
-- ---
select  1 from   t;
#[|]#select 2
-- ---
select  1 from   t;
select 2
-- ---
select  1;
select  2
-- ---
select  1;
select  2;
select  3
-- ---
select  3  from c
-- ---
SELECT  FROM tbl WHERE
-- ---
select "?" from t
-- ---
select "Odd Name", t.order, `x` from public.users
-- ---
SELECT "schema"::text, "table"::text, tbl_rows::float8, diststyle::text, sortkey1::text FROM svv_table_info
-- ---
select "x;y" from t
-- ---
select $$ cus| $$
-- ---
select $$ multi
select not keyword
$$ || 'x';
select $tag$ a $$ b $tag$
-- ---
SELECT $$a;b$$; SELECT 2
-- ---
select $1, $1
-- ---
select $1, $2, $1
-- ---
select '$1' from t
-- ---
select ';' , "a;b", `c;d` /* ; */
-- ---
select '?', ':id', '$1', '@x'
-- ---
select '\$1' from t
-- ---
select 'a'| from customers
-- ---
select 'a;b'
-- ---
select 'a;b'; select "x;y" from t; -- c; d
select /* ; */ 3
-- ---
select 'a\'; select 2
-- ---
select 'a\\b''c'
-- ---
select 'abc
select still string
' from t
-- ---
select 'abc from t
-- ---
select 'abc; select 2
-- ---
SELECT 'copy from stdin'
-- ---
select 'cust|' from t
-- ---
select 'from b' as who
-- ---
SELECT 'héllo wörld ✓'
-- ---
select 'héllo', 日本 from t
-- ---
select 'it''s; fine'
-- ---
SELECT 'it\'s', `a b` # trailing
FROM t
-- ---
select 'it\'s;ok'
-- ---
select 'it\'s;ok'; # note; here
select 2
-- ---
select 'kept'
-- ---
select 'plain' as who
-- ---
SELECT 'POINT(1 2)'::GEOMETRY
-- ---
select 'Select From Where', "Select" from "Table" where "Order" = 'And'
-- ---
select 'unterminated cus|
-- ---
SELECT 'via tunnel'
-- ---
select 'x''; drop table t; --'
-- ---
select 'x', 'y'
-- ---
select 'x', 2
-- ---
select 'é' -- hi
-- ---
SELECT 'é', bogus
-- ---
SELECT 'é', FROM t
-- ---
SELECT 'é', nmae FROM t
-- ---
select 'é', 日本 from t
-- ---
select 'é';
select 2
-- ---
select 'é日本' as 名前, c.na from customers c where c.id = 1
-- ---
select 'é日本' as 日本語, 😀
-- ---
select ( from
-- ---
select (1 + 2)) from t
-- ---
SELECT (current_schemas(false))[1]::text
-- ---
SELECT (id, name)
-- ---
SELECT (SELECT count(*) FROM customers), (SELECT count(*) FROM products), (SELECT count(*) FROM orders), (SELECT count(*) FROM order_items)
-- ---
select (select max(|) from orders o) from customers c
-- ---
SELECT *
FROM (
    SELECT
        a,
        b
    FROM t
) s
-- ---
SELECT *
FROM orders o
JOIN customers c ON o.customer_id = c.id
-- ---
SELECT *
FROM t
WHERE
    a = 1
    AND b = 2
-- ---
SELECT *
FROM t
WHERE
    aaaaaaaaaaaaaaaaaaaa = 1
    AND bbbbbbbbbbbbbbbbbbbbbbbb = 2
    AND cccccccccccccccccccccccc = 3
    AND dddddddddddd = 4
-- ---
SELECT *
FROM t
WHERE a = 1
-- ---
SELECT *
FROM t
WHERE id IN (SELECT id FROM u WHERE x = 1)
-- ---
select * fro
-- ---
select * from
-- ---
select * from "cust|
-- ---
SELECT * FROM 'https://example.invalid/x.parquet'
-- ---
select * from (select 1,
-- ---
select * from (select a from
-- ---
select * from (select a, b from t where c = 1) s where s.a in (1,2,3) and exists (select 1 from u where u.a = s.a)
-- ---
select * from (select a, b from t) s
-- ---
select * from (values (1, 2, 3)) v
-- ---
SELECT * FROM (…) AS t
-- ---
select * from :t
-- ---
select * from a
-- ---
select * from a cross join b
-- ---
select * from a JOIN
-- ---
select * from a join b
-- ---
select * from a join b ON
-- ---
SELECT * FROM a JOIN b ON a.id = b.id
-- ---
select * from a join b on a.x = b.x,
-- ---
select * from a join b where a.x = 1
-- ---
select * from a left join
-- ---
select * from a left join b on a.x = b.x
-- ---
select * from a left join b on a.x = b.x join c using (y)
-- ---
select * from a natural join b
-- ---
SELECT * from a table you pick
-- ---
select * from a union all select * from b union select * from c order by 1
-- ---
select * from a where
-- ---
select * from a where id in (select x from b union select y from c where z)
-- ---
select * from a,
-- ---
select * from a, (select 1) s
-- ---
select * from a, (select 1) s where true
-- ---
SELECT * FROM a, b
-- ---
select * from a, |
-- ---
select * from analytics.events
-- ---
select * from analytics.eventz
-- ---
select * from analytics.|
-- ---
SELECT * FROM big ORDER BY pad
-- ---
SELECT * FROM big ORDER BY pad LIMIT 20000
-- ---
SELECT * FROM big WHERE (id % 1) = 0
-- ---
SELECT * FROM big WHERE id = 5
-- ---
SELECT * FROM big WHERE id = 777
-- ---
SELECT * FROM big WHERE k = 123
-- ---
SELECT * FROM big WHERE k = 5
-- ---
select * from cu
-- ---
select * from custmers
-- ---
select * from custmers c
-- ---
select * from customers
-- ---
select * from customers c
where 1 = 1
-- ---
select * from customers c join orders o on |
-- ---
select * from customers c join reviews r on |
-- ---
SELECT * FROM customers c JOIN |
-- ---
select * from customers c join |
-- ---
select * from customers c where c.id in (select o.| from orders o)
-- ---
select * from customers c where exists (select 1 from orders o where o.customer_id = c.id)
-- ---
select * from customers c where exists (select 1 from orders o where o.customer_id = c.idd)
-- ---
select * from customers c where exists (select 1 from orders o where o.customer_id = |)
-- ---
select * from customers c, orders o where c.id = o.customer_id
-- ---
select * from customers c<ret>where 1 = 1
-- ---
select * from customers group by |
-- ---
select * from customers join orders on true, stores
-- ---
select * from customers join |
-- ---
select * from customers left join orders on true
-- ---
select * from customers order by |
-- ---
select * from customers where emial = 'x'
-- ---
select * from customers where id = 1
-- ---
select * from customers where id = 1 and |
-- ---
select * from customers where id = 1 |
-- ---
select * from customers where |
-- ---
select * from customers WHE|
-- ---
select * from customers whe|
-- ---
SELECT * FROM customers |
-- ---
select * from customers |
-- ---
select * from customers, orders
-- ---
select * from cust|
-- ---
SELECT * FROM db.t LIMIT 5
-- ---
select * from generate_series(1, 3) as g(n)
-- ---
select * from generate_series(1, 3) g, orders
-- ---
select * from information_schema.tables
-- ---
SELECT * FROM keep
-- ---
SELECT * FROM low_stock ORDER BY 1, 2
-- ---
SELECT * FROM nope
-- ---
select * from o
-- ---
select * from oi|
-- ---
select * from ord
-- ---
select * from order
-- ---
select * from orders
-- ---
select * from orders for update
-- ---
select * from orders join customers
-- ---
select * from orders o join <C-space>
-- ---
select * from orders o join customers c on o.customer_id = c.id
-- ---
select * from orders o join customers c on o.customer_id = c.id
where 1 = 1
-- ---
select * from orders o join customers c using (id)
-- ---
select * from orders o join cu|
-- ---
select * from orders o, lateral (select 1) x
-- ---
select * from orders where false
-- ---
select * from orders where placed_at between '2024-01-01' and '2024-02-01'
-- ---
select * from ord|
-- ---
select * from other_schema.t
-- ---
SELECT * FROM outer_t o JOIN inner_t i ON i.k = o.k
-- ---
select * from pg_catalog.pg_class
-- ---
select * from products;
select | from customers;
select * from orders
-- ---
SELECT * FROM public."order" LIMIT 500 OFFSET 1500
-- ---
SELECT * FROM public."order" ORDER BY id LIMIT 500
-- ---
SELECT * FROM public."order" WHERE (a, b) > (1, 'x''y') ORDER BY a, b LIMIT 500
-- ---
SELECT * FROM public."order" WHERE id > 7 ORDER BY id LIMIT 500
-- ---
SELECT * FROM public.customer_order_summary ORDER BY 1
-- ---
select * from public.customers as c join orders o on o.customer_id = c.id, stores
-- ---
select * from public.customers c join orders on true, stores s
-- ---
select * from public.items
-- ---
SELECT * FROM public.orders LIMIT 100
-- ---
select * from public.|
-- ---
select * from stores s join
-- ---
SELECT * FROM t
-- ---
select * from t -- trailing
where /* inline */ a = 1 -- more
-- ---
SELECT * FROM t a JOIN t b ON b.id = a.v ORDER BY a.id DESC
-- ---
select * from t for update
-- ---
select * from t group |
-- ---
select * from t join
-- ---
select * from t join u on t.a = u.a
-- ---
select * from t left outer join u on t.a = u.a straight_join v on v.a = u.a
-- ---
select * from t left |
-- ---
SELECT * FROM t LIMIT 5
-- ---
select * from t limit @n
-- ---
select * from t order by
-- ---
SELECT * FROM t ORDER BY i DESC
-- ---
select * from t order |
-- ---
select * from t union |
-- ---
select * from t where
-- ---
select * from t where a != NULL
-- ---
select * from t where a <> null
-- ---
select * from t where a =
-- ---
select * from t where a = "x" # comment
-- ---
select * from t where a = $1 and b = :name and c = ? and d = @v and e @> f
-- ---
select * from t where a = 'unterminated
-- ---
select * from t where a = 1 and
-- ---
select * from t where a = 1 and b = 2
-- ---
select * from t where a = ? and b = ?
-- ---
select * from t where a = ?1 and b = :name
-- ---
select * from t where a = null
-- ---
select * from t where a >=@lo
-- ---
select * from t where a @@ b
-- ---
select * from t where a between 1 and 10 and b not like '%x' or c is not null and d is distinct from e
-- ---
select * from t where a in ()
-- ---
select * from t where a is null
-- ---
select * from t where a is |
-- ---
select * from t where a=1
-- ---
select * from t where aaaaaaaaaaaaaaaaaaaa = 1 and bbbbbbbbbbbbbbbbbbbbbbbb = 2 and cccccccccccccccccccccccc = 3 and dddddddddddd = 4
-- ---
select * from t where aaaaaaaaaaaaaaaaaaaaaaaaa between 1 and 100000000 and bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb = 2 and c = 3
-- ---
select * from t where doc ? 'k'
-- ---
select * from t where doc ?& array['a','b']
-- ---
select * from t where doc ?| array['a','b']
-- ---
SELECT * FROM t WHERE id < 100
-- ---
select * from t where id in (select id from u where x = 1)
-- ---
select * from t where n = 'O''Brien' and id = 42
-- ---
select * from t where n = :n and id = :id
-- ---
SELECT * FROM t WHERE v < 5
-- ---
SELECT * FROM t WHERE v = 3
-- ---
SELECT * FROM t WHERE v = 3 ORDER BY id
-- ---
select * from t where x in (select y from u where z = null)
-- ---
select * from t, unnest(t.arr) x
-- ---
SELECT * FROM t__new
-- ---
select * from users where id = 1
-- ---
select * from users where name = 'unterminated
-- ---
SELECT * FROM v_base
-- ---
select * from where a = 1
-- ---
select * from where and or
-- ---
select * from |
-- ---
select * into newt from t
-- ---
select -1, - 2, 3 - -4, a - b, a-b, +5, ~6 from t
-- ---
select -1, 3 - -4, a-b, - 2
-- ---
select /* cus| */ 1
-- ---
select /* open
-- ---
select 0;
select 1; select 2
-- ---
select 1
    from t
-- ---
select 1
  from t
-- ---
select 1
from t
-- ---
SELECT 1
FROM t WHERE ??? = 1
-- ---
SELECT 1
GO
SELECT 2
-- ---
select 1
GO
select 2
-- ---
SELECT 1
GO
SELECT 2
GO
-- ---
select 1
select 2
-- ---
select 1   


;select 2
-- ---
select 1 -- :id ? $1
-- ---
SELECT 1 -- c
-- ---
select 1 -- c
-- ---
select 1 -- cus|
-- ---
select 1 -- trailing
/* a /* b */ still */ select 2
/* open
still comment
*/ select 3
-- ---
SELECT 1 -- ü
-- ---
select 1 /* :id ? @x */
-- ---
select 1 /* open
-- ---
select 1 /* x */ | from customers
-- ---
select 1 as a, 'x' as b union all select 2, 'y'
-- ---
SELECT 1 AS a, 2.5 AS b, 'x' AS c, x'00' AS d, NULL AS e
-- ---
SELECT 1 AS go_col
-- ---
select 1 as one, 'x' as two
-- ---
select 1 as r
-- ---
SELECT 1 AS x
-- ---
select 1 format CSV
-- ---
SELECT 1 FORMAT JSON
-- ---
SELECT 1 FROM customers WHERE 1 = 0
-- ---
SELECT 1 FROM dual WHERE 0
-- ---
SELECT 1 FROM pg_extension WHERE extname = 'postgis'
-- ---
select 1 from t for update
-- ---
select 1 from t;
select 2
-- ---
SELECT 1 UNION ALL SELECT 2
-- ---
SELECT 1 WHERE false
-- ---
select 11 as v;
select 22 as v
-- ---
SELECT 1::BIGINT AS i, 2.50::DECIMAL(10,2) AS n, 'héllo' AS t, true AS b, DATE '2024-02-29' AS d, TIMESTAMP '2024-02-29 13:14:15' AS ts, '123e4567-e89b-12d3-a456-426614174000'::UUID AS u
-- ---
SELECT 1::bigint AS i, 2.50::decimal(10,2) AS n, 'héllo'::varchar(20) AS t, true AS b, DATE '2024-02-29' AS d, TIMESTAMP '2024-02-29 13:14:15' AS ts
-- ---
SELECT 1::int8 AS i, 2.50::numeric(10,2) AS n, 'héllo'::text AS t, true AS b, DATE '2024-02-29' AS d, TIMESTAMP '2024-02-29 13:14:15' AS ts, '123e4567-e89b-12d3-a456-426614174000'::uuid AS u
-- ---
select 1;


-- end
-- ---
SELECT 1;

-- end
-- ---
SELECT 1;

SELECT 2
-- ---
SELECT 1;

SELECT 2;
SELECT
3
-- ---
select 1;
   select 2
-- ---
select 1;
delete from t
-- ---
select 1;
select 2
-- ---
select 1;
select 2;

select 3
-- ---
select 1;
select 2;
select 3
-- ---
select 1;
select nme from t
-- ---
select 1;
select nope from t
-- ---
select 1;
sel|
-- ---
select 1;  -- done
-- ---
select 1; /* open
-- ---
select 1; /* x */ drop table t
-- ---
SELECT 1; DROP TABLE t
-- ---
SELECT 1; SELECT 2
-- ---
select 1; select 2
-- ---
select 1; select 2,
-- ---
SELECT 1; SELECT 2; SEL
-- ---
select 1; select 2; select nope from nowhere; select 4
-- ---
select 1; select 2;;

-- end comment
-- ---
select 1; update t set a=1 where b=2; create table x(a int)
-- ---
select 1;; ;
 select 2
-- ---
select 1;select 2
-- ---
select 1select 1
-- ---
select 1select 1X
-- ---
select 2 as b
-- ---
select 3 as three
-- ---
SELECT 40 + 2
-- ---
select 41 + 1 as r, '?' as lit
-- ---
select 5 as five
-- ---
SELECT 6 * 7
-- ---
select 7 as seven
-- ---
select :a, $1;
select :a, :b
-- ---
select :a, :b
-- ---
select :a::int
-- ---
select :id, @name
-- ---
select :id, @name, :id
-- ---
select :n + 1 as r, '?' as lit
-- ---
select ?, :a
-- ---
select ?, :a, :b
-- ---
select ?1, ?2, ?1
-- ---
select @ -5, a @ b
-- ---
SELECT @@have_ssl
-- ---
SELECT @@TRANCOUNT
-- ---
SELECT @@tx_isolation
-- ---
select @@version, @@global.max_connections
-- ---
SELECT @@VERSION, DB_NAME(), SUSER_SNAME(), CAST(@@SPID AS int), CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(50)), CAST(SERVERPROPERTY('Edition') AS nvarchar(100))
-- ---
select @a:=1
-- ---
SELECT @x := 'two'
-- ---
select @x := 1
-- ---
select @x := @x + 1
-- ---
SELECT [id], [shape].STAsBinary() AS [shape], [shape].STSrid AS [theta_srid_1] FROM (SELECT id, shape FROM dbo.t) AS [theta_q]
-- ---
select [my col] from t
-- ---
select [my col], ?1 from t where x glob 'a*'
-- ---
select [my col], ?1, :name from t where x glob 'a*'
-- ---
select [my col], ?1, :name, @v from t where x glob 'a*';
create trigger tr after insert on t begin update u set a = 1; insert into log values (1); end;
insert or replace into t (a) values (x'ff');
select * from t /* open
-- ---
SELECT [Order Id], N'héllo' FROM [dbo].[My Table] WHERE x = @p1
-- ---
select `:a` from t
-- ---
select `order`.id from `order` # note
where x = 1
-- ---
select `order`.id from `order` where name = 'x'
-- ---
select `order`.id, ifnull(a, 0) from `order` where name = "x" # note
limit 5, 10
-- ---
select `order`.id, ifnull(a, 0) from `order` where name = "x\"y" # note; here
limit 5, 10;
insert into t (a, b) values (1, 2) on duplicate key update b = values(b);
create procedure p() begin select 1; if x then update t set a = 1; end if; end;
select 1
-- ---
SELECT a
FROM t
-- keep rows
WHERE a = 1
-- ---
SELECT a
FROM t
UNION ALL
SELECT b
FROM u
-- ---
SELECT a
FROM t
WHERE b = 1
-- ---
Select a cell to inspect its value.
-- ---
SELECT a FROM b WHERE x
-- ---
select a from customers union select | from products
-- ---
SELECT a FROM t
-- ---
select a from t
-- keep rows
where a = 1
-- ---
select a from t union all select b from u
-- ---
SELECT a FROM t WHERE b = 1
-- ---
select a from t where b = 1
-- before order
order by a
-- ---
select a from t where x = 1 group by a order by a limit 3
-- ---
SELECT a INTO #t FROM b
-- ---
Select a keymap…
-- ---
Select a theme…
-- ---
select a, b from t
-- ---
SELECT a, b FROM t WHERE (x = 1 AND y = 'z') -- c
ORDER BY a
-- ---
SELECT a, b, count(*) FROM some_table WHERE id = 42 AND name LIKE 'x%' -- c
-- ---
select a, b, from t
-- ---
select a, from t where
-- ---
select a, row_number() over (partition by a order by b desc) as rn from t
-- ---
select a, row_number() over (partition by a order by b desc) as rn, sum(x) over w from t window w as (partition by a)
-- ---
select a, sum(b) over (partition by a order by c rows between unbounded preceding and current row) from t
-- ---
select a,, b from t
-- ---
select a,, b from t where x = null; update t set a = 1
-- ---
select a,b from t where x=1
-- ---
SELECT a.b, count(*) FILTER (WHERE x > 1e3) AS "q" /* c */ FROM t -- end
$$x$$ 'y' ::int
-- ---
select a.from
-- ---
select a::int, b::text, c :: date from t
-- ---
select analytics.events.| from analytics.events
-- ---
select around object
-- ---
select arr[1:n], arr[i:j] from t
-- ---
SELECT ARRAY[1,2]::int4[] AS a
-- ---
select c. from customers c
-- ---
select c.* , c.id from customers c
-- ---
select c.emial from customers c
-- ---
select c.id, (select max(o.total) from orders o where o.customer_id = c.id) from customers c join orders o on o.customer_id = c.id where c.tier = 'x' group by c.id order by 1
-- ---
select c.id, c.name, count(o.id) as n from customers c left join orders o on o.customer_id = c.id and o.status = 'paid' where c.tier in ('gold','silver') group by c.id, c.name having count(o.id) > 2 order by n desc limit 10
-- ---
select c.id, c.name, count(o.id) as n from customers c left join orders o on o.customer_id = c.id where c.tier = 'gold' group by c.id, c.name having count(o.id) > 2 order by n desc limit 10
-- ---
select c.id, count(*) as n from customers c left join orders o on o.customer_id = c.id where c.tier = 'gold' group by c.id having count(*) > 1 order by n desc limit 10
-- ---
select c.na| from customers c
-- ---
SELECT c.object_id, c.name, TYPE_NAME(c.user_type_id), TYPE_NAME(c.system_type_id), c.max_length, c.precision, c.scale, c.is_nullable, c.is_identity, OBJECT_DEFINITION(c.default_object_id), cc.definition, cc.is_persisted, CAST(ep.value AS nvarchar(4000)) FROM sys.columns c JOIN sys.objects o ON o.object_id = c.object_id AND o.type IN ('U', 'V') AND o.is_ms_shipped = 0 LEFT JOIN sys.computed_columns cc ON cc.object_id = c.object_id AND cc.column_id = c.column_id LEFT JOIN sys.extended_properties ep ON ep.major_id = c.object_id AND ep.minor_id = c.column_id AND ep.class = 1 AND ep.name = 'MS_Description' ORDER BY c.object_id, c.column_id
-- ---
SELECT c.oid, n.nspname::text, c.relname::text, a.attnum, a.attname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped WHERE c.oid = ANY($1::oid[])
-- ---
select c.| from customers c
-- ---
select case when a > 1 then 'x' when a > 0 then 'y' else 'z' end as c from t
-- ---
select case when a > 1 then 'x' when a > 0 then 'y' else 'z' end as c, case b when 1 then 'one' end from t
-- ---
select case when a > 1 then 'x' |
-- ---
SELECT CASE WHEN a THEN 1 ELSE 0 END
-- ---
select case when a then 1 else 0 end from t
-- ---
select case when total > 10 then 'big' else 'small' end from orders
-- ---
select case | from customers
-- ---
SELECT CAST('abc' AS SIGNED) AS x
-- ---
SELECT CAST(1 AS bigint) AS i, CAST(2.50 AS decimal(10,2)) AS n, N'héllo' AS t, CAST(1 AS bit) AS b, CAST('2024-02-29' AS date) AS d, CAST('2024-02-29 13:14:15' AS datetime2(0)) AS ts, CAST('123E4567-E89B-12D3-A456-426614174000' AS uniqueidentifier) AS u
-- ---
SELECT CAST(1 AS SIGNED) AS i, CAST(2.5 AS DECIMAL(10,2)) AS n, 'héllo' AS t, CAST('2024-02-29' AS DATE) AS d, CAST('2024-02-29 13:14:15' AS DATETIME) AS ts
-- ---
SELECT CAST(@@SPID AS int)
-- ---
SELECT CAST(@@SPID AS varchar(20))
-- ---
SELECT CAST(CONNECTION_ID() AS CHAR)
-- ---
select cast(id as text), id::bigint, current_date, now() from customers
-- ---
SELECT CAST(p.ID AS CHAR) AS sid,
       p.USER AS usr,
       COALESCE(p.DB, '') AS db,
       p.HOST AS client,
       '' AS app,
       CASE WHEN p.COMMAND = 'Sleep' AND t.trx_id IS NOT NULL THEN 'idle in transaction'
            WHEN p.COMMAND = 'Sleep' THEN 'idle' ELSE 'active' END AS state,
       COALESCE(p.STATE, '') AS wait,
       TIMESTAMPDIFF(SECOND, t.trx_started, NOW()) AS txn_age,
       p.TIME AS query_age,
       COALESCE(p.INFO, '') AS query,
       '' AS blocked_by,
       (p.ID = CONNECTION_ID()) AS is_self
FROM information_schema.PROCESSLIST p
LEFT JOIN information_schema.INNODB_TRX t ON t.trx_mysql_thread_id = p.ID
ORDER BY p.ID
-- ---
SELECT CAST(r.trx_mysql_thread_id AS CHAR) AS waiting, CAST(b.trx_mysql_thread_id AS CHAR) AS blocking
FROM information_schema.INNODB_LOCK_WAITS w
JOIN information_schema.INNODB_TRX r ON r.trx_id = w.requesting_trx_id
JOIN information_schema.INNODB_TRX b ON b.trx_id = w.blocking_trx_id
-- ---
SELECT CAST(r.trx_mysql_thread_id AS CHAR) AS waiting, CAST(b.trx_mysql_thread_id AS CHAR) AS blocking
FROM performance_schema.data_lock_waits w
JOIN information_schema.innodb_trx r ON r.trx_id = w.REQUESTING_ENGINE_TRANSACTION_ID
JOIN information_schema.innodb_trx b ON b.trx_id = w.BLOCKING_ENGINE_TRANSACTION_ID
-- ---
SELECT CAST(s.session_id AS varchar(20)) AS sid,
       COALESCE(s.login_name, '') AS usr,
       COALESCE(DB_NAME(r.database_id), DB_NAME(s.database_id), '') AS db,
       COALESCE(c.client_net_address, '') AS client,
       COALESCE(s.program_name, '') AS app,
       CASE WHEN r.session_id IS NOT NULL THEN 'active'
            WHEN s.open_transaction_count > 0 THEN 'idle in transaction' ELSE 'idle' END AS state,
       COALESCE(r.wait_type, '') AS wait,
       DATEDIFF(SECOND, at.transaction_begin_time, GETDATE()) AS txn_age,
       DATEDIFF(SECOND, COALESCE(r.start_time, s.last_request_start_time), GETDATE()) AS query_age,
       COALESCE(t.text, '') AS query,
       CASE WHEN COALESCE(r.blocking_session_id, 0) > 0
            THEN CAST(r.blocking_session_id AS varchar(20)) ELSE '' END AS blocked_by,
       CASE WHEN s.session_id = @@SPID THEN 1 ELSE 0 END AS is_self
FROM sys.dm_exec_sessions s
LEFT JOIN sys.dm_exec_connections c ON c.session_id = s.session_id
LEFT JOIN sys.dm_exec_requests r ON r.session_id = s.session_id
LEFT JOIN sys.dm_tran_session_transactions st ON st.session_id = s.session_id
LEFT JOIN sys.dm_tran_active_transactions at ON at.transaction_id = st.transaction_id
OUTER APPLY sys.dm_exec_sql_text(COALESCE(r.sql_handle, c.most_recent_sql_handle)) t
WHERE s.is_user_process = 1
ORDER BY s.session_id
-- ---
select coalesce(aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb, cccccccccccccccccccccccccccc, dddddddddddddddddddd) as x from t
-- ---
select connection_id()
-- ---
select count(*) filter (where status = 'x') from orders
-- ---
select count(*) filter (where x > 1), string_agg(distinct a, ',' order by a) from t
-- ---
SELECT count(*) FROM (…) t
-- ---
SELECT count(*) FROM big a JOIN big b ON a.pad = b.pad WHERE a.id < 150000
-- ---
SELECT count(*) FROM customers
-- ---
SELECT count(*) FROM delivery_zones WHERE ST_IsValid(area)
-- ---
SELECT count(*) FROM half_done
-- ---
SELECT count(*) FROM log
-- ---
SELECT count(*) FROM new_name
-- ---
SELECT count(*) FROM old_name
-- ---
SELECT count(*) FROM orders
-- ---
SELECT count(*) FROM orders o LEFT JOIN customers c ON c.id = o.customer_id WHERE c.id IS NULL
-- ---
SELECT count(*) FROM orders o WHERE ABS(o.total - (SELECT SUM(quantity * unit_price) FROM order_items i WHERE i.order_id = o.id)) > 0.001
-- ---
SELECT count(*) FROM orders o WHERE o.total <> order_total(o.id)
-- ---
SELECT count(*) FROM pg_tables WHERE schemaname = 'public' AND tablename = 'customers_never'
-- ---
SELECT count(*) FROM products
-- ---
SELECT count(*) FROM sqlite_master
-- ---
SELECT count(*) FROM sqlite_master WHERE name = 't'
-- ---
SELECT count(*) FROM stores
-- ---
SELECT count(*) FROM stores s JOIN delivery_zones z ON z.store_id = s.id WHERE ST_Contains(z.area, s.location)
-- ---
SELECT count(*) FROM stores WHERE location_wkt LIKE 'POINT(%'
-- ---
SELECT count(*) FROM stores WHERE ST_SRID(location) = 4326
-- ---
SELECT count(*) FROM stores WHERE ST_SRID(location) = 4326 AND ST_IsValid(location)
-- ---
SELECT count(*) FROM t
-- ---
select count(*) from t
-- ---
SELECT count(*) FROM t WHERE id = 5000
-- ---
SELECT count(*) FROM t WHERE id >= 6000
-- ---
SELECT count(*) FROM t WHERE id >= 7000
-- ---
SELECT count(*) FROM t WHERE id IN (6000, 6001)
-- ---
SELECT count(*) FROM t WHERE v = 99
-- ---
SELECT count(*), 'a''b', 42, 3.14e-2 FROM t WHERE x IS NULL
-- ---
select count(a from t
-- ---
select count(a, b) from t
-- ---
select cou| from customers
-- ---
select cou|(*) from customers
-- ---
select ctid, * from orders
-- ---
SELECT current_database()
-- ---
SELECT current_database()::text, current_user::text
-- ---
SELECT current_schema()
-- ---
SELECT current_schema()::text
-- ---
SELECT current_setting('server_version_num')::int4
-- ---
SELECT current_setting('theta.x')
-- ---
SELECT currentDatabase() AS db
-- ---
select customers.| from customers
-- ---
select d.| from (select email as mail, id from customers) d
-- ---
select data->>'k', x::int, y::numeric(10,2), (a+b)::text, arr[1] from t
-- ---
SELECT DATABASE()
-- ---
select distinct on (a) a, b from t
-- ---
select distinct on (a) a, b from t order by a, b desc
-- ---
select distinct on (customer_id) customer_id from orders order by customer_id, placed_at desc
-- ---
select E'a\';b'
-- ---
select E'a\';b'; /* a /* ; */ ; */ select 2
-- ---
select E'a\'b', 'it''s', "Quoted Ident", $1, $2::text from t
-- ---
select E'it\'s :id'
-- ---
select e.* from (values (1, 2)) as e(a, b)
-- ---
select email as e from customers order by e
-- ---
select email from customers group by email having |
-- ---
select email, bogus from customers c join orders o on o.customer_id = c.id
-- ---
select email, total from customers c join orders o on o.customer_id = c.id
-- ---
select email, upper(name) n from customers where id > 1 order by n
-- ---
SELECT every column of a table you pick
-- ---
select extract(year from created_at) from customers
-- ---
SELECT extversion::text FROM pg_extension WHERE extname = 'timescaledb'
-- ---
SELECT F_TABLE_SCHEMA, F_TABLE_NAME, F_GEOMETRY_COLUMN, SRID, GEOMETRY_TYPE FROM information_schema.GEOMETRY_COLUMNS
-- ---
select FALSE
-- ---
SELECT fk.parent_object_id, fk.name, ps.name, pt.name, rs.name, rt.name, (SELECT STRING_AGG(CAST(pc.name AS nvarchar(max)), ',') WITHIN GROUP (ORDER BY fkc.constraint_column_id) FROM sys.foreign_key_columns fkc JOIN sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id WHERE fkc.constraint_object_id = fk.object_id), (SELECT STRING_AGG(CAST(rc.name AS nvarchar(max)), ',') WITHIN GROUP (ORDER BY fkc.constraint_column_id) FROM sys.foreign_key_columns fkc JOIN sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id WHERE fkc.constraint_object_id = fk.object_id), fk.delete_referential_action_desc, fk.update_referential_action_desc FROM sys.foreign_keys fk JOIN sys.tables pt ON pt.object_id = fk.parent_object_id JOIN sys.schemas ps ON ps.schema_id = pt.schema_id JOIN sys.tables rt ON rt.object_id = fk.referenced_object_id JOIN sys.schemas rs ON rs.schema_id = rt.schema_id ORDER BY fk.parent_object_id, fk.name
-- ---
select foo bar
-- ---
SELECT format FROM t
-- ---
SELECT FROM_USER AS role_name, FROM_HOST AS role_host, TO_USER AS member_name, TO_HOST AS member_host
FROM mysql.role_edges
-- ---
SELECT full_name FROM people
-- ---
SELECT g FROM (SELECT generate_series(1, 2000000000) AS g) s
-- ---
SELECT g FROM generate_series(1, 20000) g
-- ---
SELECT g FROM generate_series(1, 500) g
-- ---
SELECT g FROM generate_series(1, 5000) g
-- ---
SELECT go FROM t
-- ---
select group_concat(a separator ',') from t
-- ---
SELECT hypertable_schema::text, hypertable_name::text, num_dimensions::int4, compression_enabled::bool FROM timescaledb_information.hypertables
-- ---
SELECT i FROM dyn
-- ---
SELECT i.object_id, i.name, i.type_desc, i.is_unique, i.is_primary_key, i.is_unique_constraint, i.filter_definition, (SELECT STRING_AGG(CAST(col.name AS nvarchar(max)), ',') WITHIN GROUP (ORDER BY ic.key_ordinal) FROM sys.index_columns ic JOIN sys.columns col ON col.object_id = ic.object_id AND col.column_id = ic.column_id WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id AND ic.is_included_column = 0 AND ic.key_ordinal > 0) FROM sys.indexes i JOIN sys.objects o ON o.object_id = i.object_id AND o.type IN ('U', 'V') AND o.is_ms_shipped = 0 WHERE i.index_id > 0 AND i.name IS NOT NULL ORDER BY i.object_id, i.name
-- ---
SELECT id FROM customers
-- ---
SELECT id FROM orders
-- ---
SELECT id FROM t
-- ---
select id, 1 n from orders
-- ---
SELECT id, g FROM dst ORDER BY id
-- ---
SELECT id, g FROM src ORDER BY id
-- ---
SELECT id, seq, "table", "from", "to", on_update, on_delete FROM pragma_foreign_key_list(?1, ?2) ORDER BY id, seq
-- ---
SELECT id, shape FROM dbo.t
-- ---
SELECT id, total FROM public.orders -- 2026-11-11
-- ---
select id, v from t
-- ---
select ifnull(a, 0), date_format(now(), '%Y') from t limit 5, 10
-- ---
select inside object
-- ---
SELECT k, count(*) FROM big GROUP BY k
-- ---
Select Keymap…
-- ---
select limit 5, 10 from t
-- ---
SELECT location FROM stores ORDER BY id LIMIT 1
-- ---
SELECT max(a) FROM t
-- ---
select mode keeps the anchor
-- ---
select name
from users
-- ---
SELECT name FROM pets UNION ALL SELECT full_name FROM people
-- ---
SELECT name FROM pragma_database_list ORDER BY seq
-- ---
SELECT name FROM pragma_index_info(?1, ?2) ORDER BY seqno
-- ---
SELECT name FROM sys.schemas WHERE name NOT IN ('sys', 'INFORMATION_SCHEMA', 'guest') AND schema_id NOT BETWEEN 16384 AND 16399 ORDER BY name
-- ---
select name from t
-- ---
select name from users
-- ---
select name from users where id = 1
-- ---
SELECT name, "unique", origin, partial FROM pragma_index_list(?1, ?2) ORDER BY seq
-- ---
SELECT name, mail FROM people
-- ---
SELECT name, type, "notnull", dflt_value, pk, hidden FROM pragma_table_xinfo(?1, ?2) WHERE hidden IN (0, 2, 3) ORDER BY cid
-- ---
select nme from t
-- ---
SELECT nope FROM t
-- ---
select not keyword
-- ---
SELECT now() = statement_timestamp()
-- ---
select now| from customers
-- ---
SELECT number FROM numbers(10) WHERE number > 3
-- ---
SELECT number FROM numbers(100000)
-- ---
SELECT number FROM numbers(5)
-- ---
select o.*, c.name, c.email, c.location from orders o join customers c on c.id = o.customer_id
-- ---
select o.cu| from orders o join customers c on c.id = o.customer_id
-- ---
SELECT o.object_id, s.name, o.name, o.type, (SELECT SUM(CAST(p.rows AS bigint)) FROM sys.partitions p WHERE p.object_id = o.object_id AND p.index_id IN (0, 1)), CAST(ep.value AS nvarchar(4000)), OBJECT_DEFINITION(o.object_id) FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id LEFT JOIN sys.extended_properties ep ON ep.major_id = o.object_id AND ep.minor_id = 0 AND ep.class = 1 AND ep.name = 'MS_Description' WHERE o.type IN ('U', 'V') AND o.is_ms_shipped = 0 ORDER BY s.name, o.name
-- ---
select o.| from orders o
-- ---
select order_t| from orders
-- ---
SELECT p.id AS pid, name AS n, id + 1, 'x' FROM people p
-- ---
SELECT p.id AS pid, p.name AS owner_name, t.name, p.id + 1 AS computed, 'x' AS lit FROM people p JOIN pets t ON t.owner = p.id
-- ---
SELECT p.name AS name,
       CASE WHEN p.is_disabled = 1 THEN 0 ELSE 1 END AS login,
       CASE WHEN EXISTS (SELECT 1 FROM sys.server_role_members m
                         JOIN sys.server_principals r ON r.principal_id = m.role_principal_id
                         WHERE m.member_principal_id = p.principal_id AND r.name = 'sysadmin')
            THEN 1 ELSE 0 END AS superuser,
       CASE WHEN p.type = 'R' THEN 1 ELSE 0 END AS is_role,
       COALESCE((SELECT STRING_AGG(r.name, ', ') FROM sys.server_role_members m
                 JOIN sys.server_principals r ON r.principal_id = m.role_principal_id
                 WHERE m.member_principal_id = p.principal_id), '') AS member_of,
       COALESCE((SELECT STRING_AGG(u.name, ', ') FROM sys.server_role_members m
                 JOIN sys.server_principals u ON u.principal_id = m.member_principal_id
                 WHERE m.role_principal_id = p.principal_id), '') AS members
FROM sys.server_principals p
WHERE p.type IN ('S', 'U', 'G', 'R', 'E', 'X') AND p.name NOT LIKE '##%'
ORDER BY p.name
-- ---
SELECT pad, count(*) FROM big GROUP BY pad
-- ---
SELECT parent_object_id, name, definition FROM sys.check_constraints ORDER BY parent_object_id, name
-- ---
select pg_backend_pid()::int8
-- ---
SELECT pg_backend_pid()::text
-- ---
SELECT pid::text AS sid,
       coalesce(usename, '') AS usr,
       coalesce(datname, '') AS db,
       CASE WHEN client_addr IS NULL THEN 'local' ELSE client_addr::text || ':' || client_port END AS client,
       coalesce(application_name, '') AS app,
       coalesce(state, '') AS state,
       CASE WHEN wait_event_type IS NULL THEN '' ELSE wait_event_type || ': ' || coalesce(wait_event, '') END AS wait,
       extract(epoch FROM now() - xact_start)::float8 AS txn_age,
       extract(epoch FROM now() - query_start)::float8 AS query_age,
       coalesce(query, '') AS query,
       array_to_string(pg_blocking_pids(pid), ',') AS blocked_by,
       (pid = pg_backend_pid()) AS is_self
FROM pg_stat_activity
WHERE backend_type = 'client backend'
ORDER BY pid
-- ---
SELECT query_id AS sid, user AS usr, '' AS db, toString(address) AS client, '' AS app,
       'active' AS state, '' AS wait, elapsed AS query_age, query AS query,
       '' AS blocked_by, position(query, '/* theta-sessions */') > 0 AS is_self
FROM system.processes
ORDER BY elapsed DESC
-- ---
SELECT r.rolname AS name, r.rolcanlogin AS login, r.rolsuper AS superuser,
       r.rolcreatedb AS createdb, r.rolcreaterole AS createrole, r.rolreplication AS replication,
       r.rolbypassrls AS bypassrls, r.rolinherit AS inherit, r.rolconnlimit AS conn_limit,
       r.rolvaliduntil::text AS valid_until,
       coalesce((SELECT string_agg(g.rolname, ', ' ORDER BY g.rolname) FROM pg_auth_members m
                 JOIN pg_roles g ON g.oid = m.roleid WHERE m.member = r.oid), '') AS member_of,
       coalesce((SELECT string_agg(u.rolname, ', ' ORDER BY u.rolname) FROM pg_auth_members m
                 JOIN pg_roles u ON u.oid = m.member WHERE m.roleid = r.oid), '') AS members,
       shobj_description(r.oid, 'pg_authid') AS comment
FROM pg_roles r
-- ---
select regex matches
-- ---
select register
-- ---
SELECT Role AS role_name, '' AS role_host, User AS member_name, Host AS member_host FROM mysql.roles_mapping
-- ---
SELECT routine_schema::text, routine_name::text, routine_type::text, data_type::text FROM information_schema.routines WHERE routine_schema NOT IN ('pg_catalog', 'information_schema', 'crdb_internal', 'pg_extension') ORDER BY 1, 2
-- ---
SELECT s.name, o.name, o.type, (SELECT STRING_AGG(CONCAT(p.name, ' ', TYPE_NAME(p.user_type_id)), ', ') WITHIN GROUP (ORDER BY p.parameter_id) FROM sys.parameters p WHERE p.object_id = o.object_id AND p.parameter_id > 0), (SELECT TYPE_NAME(p.user_type_id) FROM sys.parameters p WHERE p.object_id = o.object_id AND p.parameter_id = 0) FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id WHERE o.type IN ('P', 'FN', 'IF', 'TF', 'AF', 'PC', 'FS', 'FT') AND o.is_ms_shipped = 0 ORDER BY s.name, o.name
-- ---
SELECT s.name, q.name FROM sys.sequences q JOIN sys.schemas s ON s.schema_id = q.schema_id ORDER BY 1, 2
-- ---
SELECT SCHEMA_NAME()
-- ---
SELECT seq FROM seq_1_to_100000
-- ---
SELECT seq FROM seq_1_to_2000000000
-- ---
SELECT seq FROM seq_1_to_5000
-- ---
SELECT set_config('theta.x', 'two', false)
-- ---
SELECT setval
-- ---
SELECT setval('things_id_seq'
-- ---
Select some SQL to surround first
-- ---
SELECT sqlite_version()
-- ---
SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()
-- ---
SELECT ST_SetSRID(ST_MakePoint(1, 2), 4326) AS g, ST_GeogFromText('POINT(3 4)') AS geo,
                ST_GeomFromText('LINESTRING(0 0, 1 1)') AS plain
-- ---
SELECT ST_SRID(g), ST_AsText(g) FROM dst ORDER BY id
-- ---
select status->>'k', total::int, extract(year from placed_at) from orders
-- ---
select still string
-- ---
select strftime('%Y', d) from t where x glob 'a*'
-- ---
SELECT sum(cityHash64(number)) FROM numbers(100000000000)
-- ---
SELECT sum(i) FROM range(100000000000) t(i)
-- ---
select sum(x) over (order by d rows between unbounded preceding and current row) from t
-- ---
SELECT sysdate = getdate()
-- ---
select t.order, t.group, status, name, key, value from t
-- ---
SELECT TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, SRS_ID, GEOMETRY_TYPE_NAME FROM information_schema.ST_GEOMETRY_COLUMNS
-- ---
select theme
-- ---
Select Theme…
-- ---
SELECT to_regclass('timescaledb_information.hypertables') IS NOT NULL
-- ---
select top (10) [a b], isnull(x,0) from [dbo].[t] where [c]=@p1
-- ---
SELECT TOP 5 * FROM dbo.t
-- ---
select toStartOfHour(ts) as h, countIf(x > 1) from db.t final where a in (1,2) settings max_threads=2
-- ---
SELECT trx_isolation_level FROM information_schema.innodb_trx WHERE trx_mysql_thread_id = CONNECTION_ID()
-- ---
SELECT u.User AS name, u.Host AS host, u.plugin AS plugin, (u.is_role = 'Y') AS is_role,
       (u.Super_priv = 'Y') AS superuser,
       (COALESCE(JSON_VALUE(g.Priv, '$.account_locked'), 'false') = 'true') AS locked,
       (u.password_expired = 'Y') AS expired,
       JSON_VALUE(g.Priv, '$.password_lifetime') AS lifetime,
       u.max_user_connections AS conn_limit
FROM mysql.user u JOIN mysql.global_priv g ON g.User = u.User AND g.Host = u.Host
ORDER BY u.User, u.Host
-- ---
SELECT User AS name, Host AS host, plugin AS plugin, 0 AS is_role, (Super_priv = 'Y') AS superuser,
       (account_locked = 'Y') AS locked, (password_expired = 'Y') AS expired,
       password_lifetime AS lifetime, max_user_connections AS conn_limit
FROM mysql.user ORDER BY User, Host
-- ---
SELECT v, count(*) FROM t GROUP BY v ORDER BY v
-- ---
SELECT value FROM system.settings WHERE name = 'max_threads'
-- ---
SELECT version()
-- ---
SELECT version(), currentDatabase()
-- ---
SELECT VERSION(), DATABASE(), CURRENT_USER()
-- ---
SELECT version()::text
-- ---
SELECT with a JOIN
-- ---
select x as end, left(a, 2), right(b, 1) from t
-- ---
select x into @out from t where a = @n
-- ---
select x.| from "Order Items" x
-- ---
select x::numeric(10,2)
-- ---
select zz.a from customers c
-- ---
select zzz from customers, generate_series(1,2) g
-- ---
select | from customers
-- ---
select | from customers c
-- ---
select | from customers c join orders o on o.customer_id = c.id
-- ---
select | from customers c join orders o on true
-- ---
select | from stores
-- ---
SELECT … FROM …
-- ---
SELECT … FROM … WHERE …
-- ---
SELECT, INSERT, ALL PRIVILEGES
-- ---
SELECT, INSERT, UPDATE, DELETE
-- ---
SELECT, INSERT, USAGE, ALL
-- ---
select, then act; multiple selections
-- ---
SELECT; DROP TABLE x
-- ---
set @a = 1, @b = @a
-- ---
SET autocommit = 0
-- ---
SET client_encoding = 'UTF8'
-- ---
SET enable_mergejoin = off
-- ---
SET enable_nestloop = off
-- ---
SET FOREIGN_KEY_CHECKS = 0
-- ---
SET FOREIGN_KEY_CHECKS = 1
-- ---
SET GENERATED ALWAYS
-- ---
SET GENERATED BY DEFAULT
-- ---
SET max_parallel_workers_per_gather = 0
-- ---
SET max_threads = 3
-- ---
SET NAMES utf8mb4
-- ---
SET NOT NULL
-- ---
set operations have no single source column
-- ---
Set Password…
-- ---
SET ROWCOUNT 0
-- ---
set search_path = a
-- ---
SET SESSION max_recursive_iterations = 1000000
-- ---
SET SESSION max_recursive_iterations = 4294967295
-- ---
SET SHOWPLAN_XML OFF
-- ---
SET SHOWPLAN_XML ON
-- ---
SET statement_timeout = 0
-- ---
SET statement_timeout = 100
-- ---
SET STATISTICS TIME ON;
$0
-- ---
SET STATISTICS XML OFF
-- ---
SET STATISTICS XML ON
-- ---
Set the table comment
-- ---
SET TRANSACTION ISOLATION LEVEL READ COMMITTED
-- ---
SET TRANSACTION ISOLATION LEVEL READ UNCOMMITTED
-- ---
SET work_mem
-- ---
SET work_mem = '
-- ---
SET work_mem = '32MB'
-- ---
SET work_mem = '64kB'
-- ---
SET work_mem = '64MB'
-- ---
Show Column on Map
-- ---
show me its rows
-- ---
Show system roles
-- ---
show tables; use x; pragma x; analyze; vacuum; call p()
-- ---
SHOW transaction_isolation
-- ---
SHOW transaction_read_only
-- ---
SHOW transaction_status
-- ---
SHOW WARNINGS
-- ---
Table column
-- ---
Table comment
-- ---
Table for the template…
-- ---
TABLE IF EXISTS
-- ---
table listed
-- ---
table rewrite
-- ---
Table scan on t
-- ---
table · 12k rows
-- ---
table-editor
-- ---
table.active.background
-- ---
table.active.border
-- ---
table.background
-- ---
table.even.background
-- ---
table.foot.background
-- ---
table.foot.foreground
-- ---
table.head.background
-- ---
table.head.foreground
-- ---
table.hover.background
-- ---
table.row.border
-- ---
Truncate Table
-- ---
TRUNCATE TABLE public.people
-- ---
TRUNCATE TABLE s.t
-- ---
Truncate Table…
-- ---
Truncate which table…
-- ---
UPDATE (name)
-- ---
UPDATE (total)
-- ---
update customers set email = 'x', |
-- ---
update customers set email = | where id = 1
-- ---
update customers set emial = 'x' where id = 1
-- ---
update customers set name = 'x' returning |
-- ---
update customers set |
-- ---
UPDATE orders
SET
    status = 'paid',
    total = total * 2
WHERE id = 7
-- ---
update orders o set status = 'paid', total = total * 2 from customers c where o.customer_id = c.id and c.tier = 'x'
-- ---
update orders o set status = 'x' from customers c where c.id = o.customer_id
-- ---
update orders set status = 'paid' where id = 7
-- ---
update orders set status = 'paid', total = total * 2 where id = 7
-- ---
update orders set status = 'x'
-- ---
UPDATE public.customers SET name = 'O''Neil' WHERE id = 7
-- ---
UPDATE public.orders
SET
    customer_id = NULL,
    "Total Price" = NULL
WHERE id = NULL
-- ---
UPDATE public.orders SET status = 'paid', total = NULL WHERE id = 7 AND note IS NULL
-- ---
UPDATE statement has no SET
-- ---
UPDATE statement has no SET clause
-- ---
UPDATE STATISTICS
-- ---
update t join u on t.id = u.id set t.a = u.a where u.b = 1
-- ---
update t set a = 1
-- ---
update t set a = 1 where b in (select b from u)
-- ---
update t set a = 1 where id = 3
-- ---
UPDATE t SET a = a + 1 WHERE a < 2
-- ---
update t set a = null where b = 1
-- ---
UPDATE t SET a=1
-- ---
update t set a=1
-- ---
UPDATE t SET v = 99
-- ---
UPDATE t SET v = v + 1 WHERE v > 1
-- ---
UPDATE t SET v = v + 1 WHERE v >= 2
-- ---
UPDATE t SET v = v + 10 WHERE v >= 2
-- ---
update t where a = 1
-- ---
UPDATE — 3 rows affected
-- ---
UPDATE … SET … WHERE …
-- ---
use --import on its own, without --run or --script
-- ---
use --modify / --new-table on their own, without --import, --run or --script
-- ---
use either --modify or --new-table, not both
-- ---
use either --run or --script, not both
-- ---
Use Helix Keymap
-- ---
Use Standard Keymap
-- ---
USE TEMP B-TREE
-- ---
USE TEMP B-TREE FOR DISTINCT
-- ---
USE TEMP B-TREE FOR GROUP BY
-- ---
USE TEMP B-TREE FOR ORDER BY
-- ---
use the original
-- ---
Use Vim Keymap
-- ---
Values are inserted into the SQL as literals before it runs.
-- ---
WITH a AS (
    SELECT 1 AS x
),
b AS (
    SELECT x
    FROM a
)
SELECT *
FROM b
-- ---
with a as (select 1
-- ---
with a as (select 1 as x), b as (select x from a) select * from b
-- ---
with a as (select 1) insert into t select * from a
-- ---
with a as (select 1) select * from a
-- ---
with a as (select 1) update t set x = 1
-- ---
with c(a, b) as (select 1, 2), d as (select * from customers) select 1
-- ---
WITH cte AS (…) SELECT * FROM cte
-- ---
WITH d AS (
    DELETE FROM t
    WHERE a = 1
    RETURNING *
)
INSERT INTO log
SELECT *
FROM d
-- ---
with d as (delete from orders where id = 1 returning *) select * from d
-- ---
with d as (delete from t returning *) select * from d
-- ---
with d as (delete from t where a = 1 returning *) insert into log select * from d
-- ---
WITH GRANT OPTION
-- ---
With grant option
-- ---
with recent as (select id, total as amt from orders) select | from recent
-- ---
with recent as (select id, total from orders where placed_at > now() - interval '7 days') select r.id from recent r where r.total > 10
-- ---
with recent as (select id, total from orders where placed_at > now() - interval '7 days'), big as (select * from recent where total > 100) select r.id from recent r join big b on b.id = r.id
-- ---
with recent as (select id, total from orders where placed_at > now()), big as (select * from recent where total > 100)
select r.id, case when r.total > 1 then 'é' else '😀' end from recent r join big b on b.id = r.id;
insert into customers (id, email) values (1, 'a@b.c'), (2, $$x;y$$) on conflict (id) do update set email = excluded.email returning id
-- ---
with recent as (select id, total from orders) select * from |
-- ---
with recent as (select id, total from orders) select r.| from recent r
-- ---
with recursive c(x) as (select 1 union all select x + 1 from c where x < 4000000000) select count(*) from c
-- ---
WITH RECURSIVE g(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM g WHERE x < 500) INSERT INTO t SELECT x, x % 10 FROM g
-- ---
WITH RECURSIVE g(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM g WHERE x < 5000) SELECT x FROM g
-- ---
WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r) SELECT count(*) FROM r
-- ---
WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < 200000) INSERT INTO big SELECT n, n % 5000, hex(randomblob(8)) FROM s
-- ---
with recursive t(n) as (select 1 union all select n + 1 from t where n < 5) select * from t
-- ---
with u as (update t set a = 1 where b = 2 returning id) select * from u
-- ---
with x as (delete from orders returning *) select * from x
-- ---
with x as (select 1 as a) select a from x
-- ---
with x as (select 1) select * from x, (select 2) d
-- ---
with x as (select id, name as n, count(*) from t) select * from x join (select a, b from y) d on true
-- ---
with x as (select | from customers) select 1
-- ---
WITH … AS (…) SELECT …
