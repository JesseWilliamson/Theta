-- DuckDB syntax.
from customers
from customers select first_name
from customers select first_name, last_name where id < 10
from customers where id < 10
from customers limit 5
select * from customers
select * exclude (email) from customers
select * exclude email from customers
select * replace (upper(first_name) as first_name) from customers
select * rename (first_name as given) from customers
select * exclude (email) replace (lower(first_name) as first_name) from customers
select c.* exclude (email) from customers c
select columns(*) from customers
select columns('.*name') from customers
select min(columns(*)) from customers
select columns(c -> c like '%name') from customers
select * like '%name' from customers
select 1 as a, 2 as b, from customers
select first_name, from customers
select id, first_name from customers where id = 1
select #1, #2 from customers
select a from (select 1 as a)
select * from (select 1 as a) t
select * from range(10)
select * from range(1, 10)
select * from range(1, 10, 2) t(n)
select range from range(5)
select * from generate_series(1, 10) as g(n)
select unnest([1, 2, 3])
select unnest([1, 2, 3]) as n
select * from unnest([1, 2, 3]) as t(n)
select [1, 2, 3]
select [1, 2, 3][1]
select [1, 2, 3][1:2]
select [1, 2, 3][2:]
select list_value(1, 2, 3)
select list_transform([1, 2, 3], x -> x + 1)
select list_filter([1, 2, 3], x -> x > 1)
select list_reduce([1, 2, 3], (a, b) -> a + b)
select [x + 1 for x in [1, 2, 3]]
select [x for x in [1, 2, 3] if x > 1]
select {'a': 1, 'b': 2}
select {'a': 1, 'b': 2}.a
select {'a': {'b': 2}}.a.b
select struct_pack(a := 1, b := 2)
select row(1, 2)
select (1, 2)
select map {'a': 1, 'b': 2}
select map([1, 2], ['a', 'b'])
select map {'a': 1}['a']
select s['a'] from (select {'a': 1} as s) t
select 1::int, '1'::varchar, 1::double, 1::decimal(10, 2), '2020-01-01'::date, 1::bigint[]
select [1, 2]::int[]
select '[1, 2]'::json
select '{"a": 1}'::json -> 'a'
select '{"a": 1}'::json ->> 'a'
select json_extract('{"a": 1}', '$.a')
select json_extract_string('{"a": 1}', '$.a')
select attributes -> 'a' from products
select attributes ->> 'a' from products
select cast(1 as integer), try_cast('x' as integer), cast('2020-01-01' as date)
select 5 // 2, 5 % 2, 5 / 2, 2 ** 3, 2 ^ 3
select 5 // 2
select 'abc' similar to 'a.*'
select 'abc' ~ 'a'
select 'abc' ilike 'A%', 'abc' not ilike 'A%'
select 'abc' glob 'a*'
select 'a' || 'b'
select 'a' collate nocase
select interval 1 day
select interval '1 day'
select interval (1) day
select now() - interval 1 day
select now() - interval '1' hour
select date '2020-01-01', timestamp '2020-01-01 00:00:00', time '00:00:00'
select date_trunc('month', now()), date_part('year', now()), extract(year from now())
select strftime(now(), '%Y'), strptime('2020', '%Y')
select epoch(now()), epoch_ms(0), to_timestamp(0)
select age(now(), timestamp '2020-01-01')
select datediff('day', timestamp '2020-01-01', now()), date_diff('day', date '2020-01-01', now()), datesub('day', date '2020-01-01', now())
select current_date, current_time, current_timestamp, now(), today()
select current_schema(), current_database(), version()
select typeof(1), pg_typeof(1)
select count(*) filter (where status = 'pending') from orders
select list(id) from orders
select list(id order by id desc) from orders
select string_agg(first_name, ', ' order by first_name) from customers
select array_agg(id) from orders
select arg_max(id, total), arg_min(id, total) from orders
select first(id), last(id), any_value(id) from orders
select histogram(status) from orders
select approx_count_distinct(customer_id) from orders
select quantile_cont(total, 0.5), median(total), mode(status) from orders
select percentile_cont(0.5) within group (order by total) from orders
select count(*), sum(total) from orders group by all
select status, count(*) from orders group by all
select status, count(*) from orders group by 1
select status, count(*) from orders group by status order by all
select * from orders order by all desc
select * from orders order by total desc nulls last
select * from orders limit 10 offset 5
select * from orders limit 10%
select * from orders limit 5 offset 2
select * from orders using sample 10%
select * from orders using sample 10 rows
select * from orders using sample reservoir(10 rows) repeatable (1)
select * from orders tablesample 10%
select * from orders tablesample bernoulli (10)
select id, row_number() over (order by id) from orders
select id, sum(total) over (order by id rows between unbounded preceding and current row) from orders
select id, sum(total) over (order by id range between interval 1 day preceding and current row) from orders
select id, sum(total) over w from orders window w as (order by id)
select id, lag(total, 1, 0) over (order by id) from orders
select id, first_value(total ignore nulls) over (order by id) from orders
select id, total from orders qualify row_number() over (partition by customer_id order by total desc) = 1
select id, total from orders qualify rank() over (order by total) <= 3
select distinct on (customer_id) customer_id, id from orders order by customer_id, id
select distinct country from customers
select * from customers pivot (count(*) for country in ('FR', 'DE'))
pivot orders on status using sum(total)
pivot orders on status using sum(total) group by customer_id
unpivot orders on total, id into name n value v
select * from orders unpivot (v for n in (total, id))
summarize customers
summarize select * from customers
describe customers
describe select * from customers
show tables
show all tables
show customers
select * from duckdb_tables()
select * from duckdb_columns() where table_name = 'customers'
select * from information_schema.tables
select * from information_schema.columns where table_name = 'customers'
select * from pragma_table_info('customers')
select * from read_csv('x.csv')
select * from read_csv_auto('x.csv', header = true)
select * from read_parquet('x.parquet')
select * from 'x.parquet'
select * from 'x.csv'
select * from "x.csv"
select * from read_json('x.json')
select * from glob('*.csv')
select * from customers c join orders o on o.customer_id = c.id
select * from customers c join orders o using (id)
select * from customers c asof join orders o on c.created_at >= o.ordered_at
select * from customers c asof left join orders o on c.created_at >= o.ordered_at
select * from customers c positional join orders o
select * from customers c semi join orders o on o.customer_id = c.id
select * from customers c anti join orders o on o.customer_id = c.id
select * from customers c left join orders o on o.customer_id = c.id
select * from customers c full outer join orders o on o.customer_id = c.id
select * from customers c cross join orders o
select * from customers c natural join orders o
select * from customers c join lateral (select * from orders o where o.customer_id = c.id limit 1) t on true
select * from customers c, lateral (select * from orders o where o.customer_id = c.id) t
select * from customers c join orders o on true join order_items i on true
select * from customers c join orders o
select * from customers, orders
select (select count(*) from orders)
select exists (select 1)
select * from customers where id in (select customer_id from orders)
select * from customers where id in [1, 2, 3]
select * from customers where id in (1, 2)
select * from customers where id in ()
select * from customers where id = any (select customer_id from orders)
select * from customers where id = any ([1, 2, 3])
select * from customers where list_contains([1, 2], id)
select * from customers where id = $1
select * from customers where id = ?
select $1, $2
select ? + ?
with t as (select 1 as a) select * from t
with t(a) as (select 1) select * from t
with recursive t(n) as (select 1 union all select n + 1 from t where n < 5) select * from t
with t as materialized (select 1 as a) select * from t
with t as not materialized (select 1 as a) select * from t
with t as (select 1 as a), u as (select 2 as b) select * from t, u
select 1 union select 2
select 1 union all select 2
select 1 union by name select 2
select 1 union all by name select 2
select 1 intersect select 1
select 1 except select 2
select 1 intersect all select 1
select 1 except all select 2
values (1), (2)
values (1, 'a'), (2, 'b')
select * from (values (1, 'a')) t(a, b)
select * from (values (1, 'a')) as t
select * from customers where id between 1 and 5
select * from customers where id not between 1 and 5
select case when 1 then 2 end
select coalesce(null, 1), nullif(1, 2), ifnull(null, 1)
select 1 is distinct from 2, 1 is not distinct from 2
select 4 !
select factorial(4)
select 1 as "weird alias"
select 'it''s', e'a\nb', $$x$$, $tag$ y $tag$
select x'ff', 0xff, 1e3, 1_000, .5
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now())
insert into customers by name select 'A' as first_name, 'B' as last_name, 'x' as email, now() as created_at
insert into customers by position select 1, 'A', 'B', 'x', null, null, now(), true
insert or replace into customers by name select 1 as id, 'A' as first_name, 'B' as last_name, 'x' as email, now() as created_at
insert or ignore into customers (id, first_name, last_name, email, created_at) values (1, 'A', 'B', 'x@y.z', now())
insert into customers (id, first_name, last_name, email, created_at) values (1, 'A', 'B', 'x@y.z', now()) on conflict do nothing
insert into customers (id, first_name, last_name, email, created_at) values (1, 'A', 'B', 'x@y.z', now()) on conflict (id) do nothing
insert into customers (id, first_name, last_name, email, created_at) values (1, 'A', 'B', 'x@y.z', now()) on conflict (id) do update set first_name = excluded.first_name
insert into customers (id, first_name, last_name, email, created_at) values (1, 'A', 'B', 'x@y.z', now()) on conflict (id) do update set first_name = excluded.first_name where excluded.first_name <> customers.first_name
insert into customers (id, first_name, last_name, email, created_at) values (1, 'A', 'B', 'x@y.z', now()) returning id
insert into customers (id, first_name, last_name, email, created_at) values (1, 'A', 'B', 'x@y.z', now()) returning *
insert into customers (id, first_name, last_name, email, created_at) values (1, 'A', 'B', 'x@y.z', now()) returning id as new_id
insert into customers (first_name, last_name, email, created_at) select first_name, last_name, 'x' || id, created_at from customers
insert into customers from customers
insert into customers default values
insert into customers values (1, 'A', 'B', 'x', null, null, now(), true)
update customers set city = 'x'
update customers set city = 'x' where id = 1
update customers set city = 'x' where id = 1 returning id
update customers set (city, country) = ('x', 'y')
update customers set city = o.note from orders o where o.customer_id = customers.id
update customers set city = 'x' from orders o where o.customer_id = customers.id returning customers.id
update customers as c set city = 'x' where c.id = 1
update customers c set city = 'x' where c.id = 1
delete from customers
delete from customers where id = 1
delete from customers where id = 1 returning *
delete from customers using orders where orders.customer_id = customers.id
delete from customers c using orders o where o.customer_id = c.id
delete from customers as c where c.id = 1
truncate customers
truncate table customers
merge into customers using (select 1 as id) s on customers.id = s.id when matched then update set first_name = 'x'
create table t (id integer primary key, name varchar default 'x')
create table t (a int, b text not null, c double, d date, e timestamp, f decimal(10, 2), g boolean)
create table t (a int[], b struct(x int, y text), c map(text, int), d int[3], e union(a int, b text))
create table t (a int, unique (a), check (a > 0))
create table t (a integer references customers (id))
create table t (a integer, foreign key (a) references customers (id))
create table t (a int, b int generated always as (a + 1) virtual)
create table t (a int, b int as (a + 1))
create table t as select * from customers
create table t as from customers
create table t as select * from customers with no data
create or replace table t as select 1 as a
create table if not exists t (a int)
create temp table t (a int)
create temporary table t (a int)
create table s.t (a int)
create sequence seq
create sequence seq start 1 increment 1
create type mood as enum ('a', 'b')
create index idx on customers (email)
create unique index idx on customers (email)
create index idx on customers (lower(email))
create view v as select * from customers
create or replace view v as select * from customers
create view v (a, b) as select id, first_name from customers
create temp view v as select * from customers
create schema s
create schema if not exists s
create macro add1(a) as a + 1
create macro tbl() as table select 1
create function add2(a) as a + 2
create or replace macro m(a, b := 1) as a + b
alter table customers add column notes text
alter table customers add column if not exists notes text
alter table customers drop column city
alter table customers rename to clients
alter table customers rename column city to town
alter table customers alter column city set data type varchar
alter table customers alter city type varchar
alter table customers alter column city set default 'x'
alter table customers alter column city drop default
alter table customers alter column city drop not null
alter view v rename to w
drop table customers
drop table if exists customers
drop table customers cascade
drop view v
drop view if exists v
drop index idx
drop sequence seq
drop type mood
drop schema s
drop macro m
drop function f
set threads = 4
set memory_limit = '1GB'
set search_path = 'main'
set schema = 'main'
set global threads = 4
set variable x = 1
reset threads
pragma version
pragma database_size
pragma table_info('customers')
pragma show_tables
pragma enable_progress_bar
pragma threads = 4
call pragma_version()
call duckdb_tables()
attach 'x.db' as other
attach 'x.db' as other (read_only)
attach database 'x.db'
detach other
detach database other
use other
use memory.main
install httpfs
load httpfs
force install httpfs
checkpoint
force checkpoint
vacuum
analyze
begin
begin transaction
commit
rollback
abort
start transaction
explain select * from customers
explain analyze select * from customers
explain (format json) select * from customers
copy customers to 'x.csv'
copy customers to 'x.csv' (header, delimiter ',')
copy customers to 'x.parquet' (format parquet)
copy customers from 'x.csv'
copy (select * from customers) to 'x.csv'
copy (select * from customers) to 'x.csv' with (header 1)
export database 'dir'
import database 'dir'
create secret s (type s3)
prepare p as select $1
prepare p as select * from customers where id = ?
execute p(1)
deallocate p
comment on table customers is 'x'
comment on column customers.city is 'x'
