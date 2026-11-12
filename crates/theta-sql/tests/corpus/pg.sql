-- PostgreSQL 16 syntax (also meant for DuckDB where it overlaps; each server judges for itself).
select distinct on (customer_id) customer_id, id, total from orders order by customer_id, total desc
select * from customers where first_name ilike 'a%'
select * from customers where first_name not ilike 'a%'
select * from customers where first_name ~ '^A'
select * from customers where first_name ~* '^a'
select * from customers where first_name !~ '^A'
select * from customers where first_name similar to 'A%'
select id::text from orders
select id::bigint, total::numeric(10, 2), ordered_at::date from orders
select ordered_at::timestamp with time zone from orders
select ordered_at::timestamptz at time zone 'UTC' from orders
select ordered_at at time zone 'UTC' from orders
select '1'::int + 1
select '{1,2,3}'::int[]
select array[1, 2, 3]
select array[1, 2, 3][1]
select (array[1, 2, 3])[1]
select array_agg(id) from orders
select array_agg(id order by id desc) from orders
select string_agg(first_name, ', ' order by first_name) from customers
select string_agg(distinct country, ',') from customers
select unnest(array[1, 2, 3])
select * from unnest(array[1, 2, 3]) as t(n)
select * from unnest(array[1, 2, 3]) with ordinality as t(n, i)
select * from generate_series(1, 10) as g(n)
select * from generate_series(1, 10)
select g from generate_series(1, 10) g
select generate_series(1, 3)
select attributes->'a' from products
select attributes->>'a' from products
select attributes #> '{a,b}' from products
select attributes #>> '{a,b}' from products
select attributes @> '{"a": 1}' from products
select * from products where attributes @> '{"a": 1}'
select * from products where attributes ? 'a'
select * from products where attributes ?| array['a', 'b']
select * from products where attributes ?& array['a', 'b']
select * from products where attributes ->> 'color' = 'red'
select attributes -> 'tags' -> 0 from products
select jsonb_build_object('a', 1, 'b', 2)
select jsonb_agg(attributes) from products
select jsonb_each(attributes) from products
select * from jsonb_each(attributes)
select * from products p, jsonb_each(p.attributes) as e(k, v)
select * from products p, lateral jsonb_each(p.attributes) as e(k, v)
select * from customers c, lateral (select * from orders o where o.customer_id = c.id order by total desc limit 3) t
select * from customers c left join lateral (select * from orders o where o.customer_id = c.id limit 1) t on true
select * from customers c cross join lateral (select count(*) as n from orders o where o.customer_id = c.id) t
select count(*) filter (where status = 'pending') as pending, count(*) filter (where status = 'shipped') as shipped from orders
select percentile_cont(0.5) within group (order by total) from orders
select mode() within group (order by status) from orders
select rank(10) within group (order by total) from orders
select status, percentile_disc(array[0.25, 0.5]) within group (order by total) from orders group by status
select grouping(status), status, count(*) from orders group by rollup (status)
select status, customer_id, count(*) from orders group by cube (status, customer_id)
select status, customer_id, count(*) from orders group by grouping sets ((status), (customer_id), ())
select status, count(*) from orders group by status, ()
select * from orders order by total desc nulls last
select * from orders order by total asc nulls first
select * from orders order by total using >
select * from orders fetch first 5 rows only
select * from orders offset 5 rows fetch next 5 rows only
select * from orders order by id fetch first 5 rows with ties
select * from orders for update
select * from orders for update skip locked
select * from orders for share nowait
select * from orders o join customers c on c.id = o.customer_id for update of o
select * from only orders
select * from orders tablesample system (10)
select * from orders tablesample bernoulli (10) repeatable (1)
select * from rows from (generate_series(1, 3), generate_series(1, 2)) as t(a, b)
select 1 is distinct from 2
select 1 is not distinct from 2
select null is null, true is true, false is not true, null is unknown
select 1 between symmetric 3 and 1
select 'a' collate "C"
select first_name collate "C" from customers
select * from customers order by first_name collate "C"
select interval '1 day'
select interval '1 day' + now()
select now() - interval '30 days'
select now() - interval '1' day
select date '2020-01-01'
select timestamp '2020-01-01 00:00:00'
select timestamptz '2020-01-01 00:00:00+00'
select extract(year from now())
select extract(epoch from now())
select date_trunc('day', now())
select date_part('year', now())
select age(now(), ordered_at) from orders
select to_char(now(), 'YYYY-MM-DD')
select position('a' in 'abc')
select substring('abc' from 2 for 1)
select substring('abc' from 2)
select overlay('abc' placing 'x' from 2 for 1)
select trim(leading 'a' from 'aab')
select trim(trailing from 'a ')
select 5 % 2, 2 ^ 3, 7 / 2, |/ 25, @ -5, ||/ 27, 5 !
select 1 operator(pg_catalog.+) 2
select x from (select 1 as x) s where x operator(pg_catalog.=) 1
select current_date, current_time, current_timestamp, localtime, localtimestamp
select current_user, session_user, current_schema, current_catalog
select current_schema()
select version()
select pg_typeof(1)
select exists (select 1)
select row(1, 2)
select (1, 2)
select (1, 2) = (1, 2)
select (1, 2) < (1, 3)
select (1, 2) overlaps (3, 4)
select (c).first_name from (select c from customers c) t
select (t.c).first_name from (select c from customers c) t
select t.* from (select 1 as a) t
select * from customers c where (c.country, c.city) in (('FR', 'Paris'), ('DE', 'Berlin'))
select * from customers where id = any (array[1, 2, 3])
select * from customers where id = any ('{1,2,3}')
select * from customers where id <> all (array[1, 2, 3])
select * from customers where id in (select unnest(array[1, 2]))
select count(*) from customers where created_at > now() - interval '7 days'
select $1
select $1::int
select * from customers where id = $1
select * from customers where id = $1 and country = $2
select * from customers where id = $1 limit $2
select json_agg(c) from customers c
select to_jsonb(c) from customers c
select row_to_json(c) from customers c
select json_build_object('id', id, 'name', first_name) from customers
select json_object_agg(id, first_name) from customers
select first_name, count(*) over () from customers
select lag(total, 1, 0) over (order by id) from orders
select nth_value(total, 2) over (order by id) from orders
select sum(total) over (order by id rows between current row and unbounded following) from orders
select sum(total) over (order by id groups between 1 preceding and 1 following) from orders
select sum(total) over (order by id range between interval '1 day' preceding and current row) from orders
select sum(total) over (order by id rows unbounded preceding exclude current row) from orders
select sum(total) over (partition by status order by id) filter (where total > 0) from orders
with ins as (insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) returning id) select * from ins
with del as (delete from orders where status = 'cancelled' returning *) select count(*) from del
with upd as (update orders set status = 'shipped' where id = 1 returning id, status) select * from upd
with moved as (delete from orders where id = 1 returning *) insert into orders select * from moved
with recursive t(n) as (values (1) union all select n + 1 from t where n < 5) select * from t
with recursive t(n) as (select 1 union all select n + 1 from t where n < 5) cycle n set is_cycle using path select * from t
with recursive t(n) as (select 1 union all select n + 1 from t where n < 5) search depth first by n set ord select * from t
with a as materialized (select 1 as x) select * from a
with a as not materialized (select 1 as x) select * from a
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) returning id
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) returning *
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) returning id as new_id
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) on conflict do nothing
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) on conflict (email) do nothing
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) on conflict (email) do update set first_name = excluded.first_name
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) on conflict (email) do update set first_name = excluded.first_name, last_name = excluded.last_name where customers.id > 0
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) on conflict on constraint customers_email_key do nothing
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) on conflict (email) where email is not null do nothing
insert into customers as c (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) on conflict (email) do update set first_name = c.first_name || '!'
insert into customers (first_name, last_name, email, created_at) overriding user value values ('A', 'B', 'x@y.z', now())
insert into customers default values
insert into orders (customer_id, ordered_at, total) select id, now(), 0 from customers where country = 'FR'
update orders o set total = s.t from (select order_id, sum(quantity * unit_price) as t from order_items group by order_id) s where s.order_id = o.id
update orders set status = 'shipped' from customers c where c.id = orders.customer_id and c.country = 'FR'
update orders set status = 'shipped' where id = 1 returning id, status
update orders o set (status, note) = ('shipped', 'x') where o.id = 1
update orders set (status, note) = (select 'shipped', 'x') where id = 1
update customers set first_name = 'x' where current of c
update only customers set first_name = 'x'
update customers set first_name = first_name || '!' returning *
delete from orders using customers c where c.id = orders.customer_id and c.country = 'FR'
delete from orders o using customers c where c.id = o.customer_id returning o.id
delete from orders where id = 1 returning *
delete from only orders where id = 1
merge into customers c using (select 1 as id, 'x' as first_name) s on c.id = s.id when matched then update set first_name = s.first_name when not matched then insert (id, first_name, last_name, email, created_at) values (s.id, s.first_name, 'l', 'e', now())
merge into customers c using customers d on c.id = d.id when matched and c.id > 5 then delete
merge into customers c using customers d on c.id = d.id when matched then do nothing
merge into customers c using customers d on c.id = d.id when not matched by source then delete
create table t (id serial primary key, name text not null default '', created timestamptz default now())
create table t (id bigint generated always as identity primary key, name text)
create table t (id int generated by default as identity (start with 10 increment by 5), name text)
create table t (a int, b int generated always as (a * 2) stored)
create table t (a int, constraint pk primary key (a), constraint ck check (a > 0))
create table t (a int references customers (id) on delete cascade on update set null)
create table t (a int, b int, foreign key (a, b) references order_items (order_id, line_no))
create table t (a int, exclude using gist (a with =))
create table t (like customers including all)
create table t (like customers)
create table t (a int) partition by range (a)
create table t1 partition of t for values from (1) to (10)
create table t (a int, b text[], c jsonb, d numeric(10, 2), e timestamp without time zone, f double precision, g character varying(10))
create table t (a int) with (fillfactor = 70)
create unlogged table t (a int)
create table t as table customers
create table t as select 1 as a with no data
create temp table t on commit drop as select 1
create table s.t (a int)
create table "weird name" ("Col" int, "other col" text)
create index concurrently idx_x on customers (email)
create index idx_x on customers using btree (email)
create index idx_x on customers using gin (to_tsvector('english', first_name))
create index idx_x on customers (lower(email))
create index idx_x on customers (email) where email is not null
create index idx_x on customers (email) include (city)
create index idx_x on customers (email desc nulls last)
create unique index idx_x on customers (email, country)
create materialized view mv as select country, count(*) from customers group by country
create materialized view mv as select country, count(*) from customers group by country with no data
create view v as select * from customers
create or replace view v as select * from customers
create view v with (security_barrier = true) as select * from customers
create view v as with t as (select 1 as a) select * from t
create view v (a) as values (1)
create sequence seq
create sequence seq start with 1 increment by 1 minvalue 1 maxvalue 100 cache 10 cycle
create schema s
create schema if not exists s
create extension if not exists pgcrypto
create type mood as enum ('sad', 'ok', 'happy')
create type pt as (x int, y int)
create domain posint as integer check (value > 0)
create function f(a int) returns int language sql as $$ select a + 1 $$
create or replace function f(a int, b int default 1) returns int as $$ begin return a + b; end; $$ language plpgsql
create function f() returns trigger as $$ begin return new; end; $$ language plpgsql
create function f(a int) returns table (x int, y int) as $$ select a, a $$ language sql
create function f(a int) returns setof orders as $$ select * from orders where id = a $$ language sql stable
create procedure p(a int) language sql as $$ select a $$
create trigger tr before insert on orders for each row execute function f()
create trigger tr after update of total on orders for each row when (old.total is distinct from new.total) execute procedure f()
create rule r as on insert to orders do also select 1
create policy p on customers using (true)
create role app login password 'x'
alter table customers add column notes text not null default ''
alter table customers add constraint u unique (email)
alter table customers add constraint fk foreign key (id) references customers (id)
alter table customers drop constraint u
alter table customers alter column city set not null
alter table customers alter column city drop not null
alter table customers alter column city set default 'x'
alter table customers alter column city type varchar(100)
alter table customers alter column city type varchar(100) using city::varchar(100)
alter table customers rename column city to town
alter table customers rename to clients
alter table customers set schema s
alter table customers enable row level security
alter table customers owner to postgres
alter table if exists customers add column x int
alter table customers add column if not exists x int
alter table only customers add column x int
alter table customers add column a int, add column b int
alter index idx_orders_customer_id rename to idx_x
alter sequence seq restart with 1
alter table customers set (autovacuum_enabled = false)
alter table customers drop column city cascade
alter table customers cluster on customers_pkey
drop table customers cascade
drop table if exists customers cascade
drop table customers, orders
drop view if exists v cascade
drop materialized view mv
drop index concurrently idx_orders_customer_id
drop function f(int)
drop function if exists f
drop type mood
drop schema s cascade
drop sequence seq
drop trigger tr on orders
drop extension pgcrypto
truncate customers
truncate table customers, orders restart identity cascade
truncate only customers
comment on table customers is 'x'
comment on column customers.city is 'x'
grant select on customers to public
grant select, insert on customers to postgres
grant all privileges on all tables in schema public to postgres
revoke select on customers from public
set search_path to public
set search_path = public, pg_catalog
set local statement_timeout = '5s'
set time zone 'UTC'
set session characteristics as transaction isolation level serializable
reset all
reset search_path
show search_path
show all
begin isolation level serializable
begin read only
begin transaction
start transaction
commit and chain
savepoint sp
rollback to savepoint sp
release savepoint sp
rollback to sp
lock table customers in access exclusive mode
analyze customers
analyze verbose customers (city)
vacuum customers
vacuum (analyze, verbose) customers
reindex table customers
cluster customers using customers_pkey
refresh materialized view mv
refresh materialized view concurrently mv
explain select * from customers
explain (analyze, buffers, format json) select * from customers
explain analyze select * from customers
explain verbose select * from customers
explain (costs off) select * from customers
explain analyze delete from customers where id = 1
explain insert into customers (first_name) values ('x')
explain with t as (select 1) select * from t
prepare p as select 1
prepare p (int) as select * from customers where id = $1
execute p(1)
deallocate p
deallocate all
discard all
declare c cursor for select * from customers
declare c cursor with hold for select * from customers
fetch next from c
fetch 10 from c
close c
listen ch
notify ch
notify ch, 'payload'
unlisten *
do $$ begin perform 1; end $$
do language plpgsql $$ begin null; end $$
copy customers to stdout
copy customers (id, city) from stdin
copy (select * from customers) to stdout with (format csv, header)
select set_config('search_path', 'public', false)
select pg_advisory_lock(1)
select * from pg_catalog.pg_tables
select * from pg_tables where schemaname = 'public'
select * from information_schema.tables where table_schema = 'public'
select relname, reltuples from pg_class where relkind = 'r'
select n.nspname, c.relname from pg_class c join pg_namespace n on n.oid = c.relnamespace
select oid, typname from pg_type
select * from pg_stat_activity
select pg_size_pretty(pg_total_relation_size('customers'))
select ctid, xmin, * from customers
select tableoid::regclass, * from customers
select 'customers'::regclass
select 'customers'::regclass::oid
select * from customers where ctid = '(0,1)'
select st_astext(location) from stores
select st_distance(a.location, b.location) from stores a, stores b
select * from stores where st_dwithin(location, st_makepoint(0, 0)::geography, 1000)
select st_x(location::geometry) from stores
select 1 as "weird alias", 2 as "x""y"
select 'it''s'
select e'a\nb'
select e'it\'s'
select $$ text $$
select $tag$ text $tag$
select u&'d\0061t\+000061'
select b'101', x'1f'
select 1e3, 1.5e-3, .5, 5., 0x1f, 1_000
select ((1))
select (select 1)
select (select (select 1))
select ((select 1) union (select 2))
select * from ((select 1 as a) union (select 2)) t
select * from (select 1 as a union select 2) t
select a from (values (1), (2)) as t(a)
select a, b from (values (1, 2), (3, 4)) as t(a, b) where a > 1
select (values (1))
select * from customers c1 join customers c2 on c1.country = c2.country and c1.id < c2.id
select * from customers c1 join (customers c2 join orders o on o.customer_id = c2.id) on c1.id = c2.id
select * from (customers c join orders o on o.customer_id = c.id)
select * from (customers c join orders o on o.customer_id = c.id) as j
select * from customers c1 join customers c2 using (id)
select * from customers c1 join customers c2 using (id) as j
select * from customers c1 natural left join customers c2
select * from customers a join orders b on true join order_items c on true
select * from a join b on true
table customers
table customers order by id limit 1
(table customers) union (table customers)
select * from customers where id = (select max(id) from customers)
select * from customers where (select count(*) from orders where orders.customer_id = customers.id) > 5
select id, (select array_agg(total) from orders where customer_id = c.id) from customers c
select array(select id from orders)
select exists (select 1 from orders where customer_id = c.id) from customers c
select * from customers where exists (select from orders where customer_id = customers.id)
select from customers
select
select;
select 1 from customers where
