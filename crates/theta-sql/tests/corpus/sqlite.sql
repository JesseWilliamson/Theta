-- SQLite syntax.
select [id], [first_name] from [customers]
select "id" from "customers"
select `id` from `customers`
select 'it''s', "col", x'ff', 0xff, 1e3, .5
select * from customers where first_name glob 'A*'
select * from customers where first_name not glob 'A*'
select * from customers where first_name like 'a%' escape '\'
select * from customers where first_name regexp '^A'
select * from customers where first_name match 'a'
select * from customers where id is 1
select * from customers where id is not 1
select * from customers where id isnull
select * from customers where id notnull
select * from customers where id not null
select id from customers where id between 1 and 5 collate nocase
select first_name collate nocase from customers
select * from customers order by first_name collate nocase
select * from customers order by id limit 5, 10
select * from customers order by id limit 10 offset 5
select * from customers limit -1
select * from customers indexed by idx_t
select * from customers not indexed
select * from customers c indexed by idx_t where c.id = 1
select * from main.customers
select main.customers.id from main.customers
select * from sqlite_master
select name from sqlite_master where type = 'table'
select * from sqlite_schema
select * from pragma_table_info('customers')
select * from pragma_table_info('customers') where pk = 1
select name, type from pragma_table_info('customers')
select ?1, ?2, ?
select :a, @b, $c
select * from customers where id = ?1 and country = ?2
select * from customers where id = :id
select * from customers where id = @id
select * from customers where id = $id
select typeof(1), typeof('a'), typeof(null)
select group_concat(first_name) from customers
select group_concat(first_name, ', ') from customers
select group_concat(distinct country) from customers
select ifnull(note, 'x'), coalesce(note, 'x'), nullif(1, 2) from orders
select iif(total > 1, 'a', 'b') from orders
select date('now'), time('now'), datetime('now'), strftime('%Y', 'now'), julianday('now')
select datetime('now', '-1 day'), date('now', 'start of month', '+1 month', '-1 day')
select substr('abc', 2), substr('abc', 2, 1), length('abc'), instr('abc', 'b'), replace('abc', 'a', 'x')
select cast(id as text), cast('1' as integer), cast(1 as real) from orders
select abs(-1), round(1.5), round(1.55, 1), max(1, 2), min(1, 2), random(), hex(x'ff'), quote('a')
select json_extract(attributes, '$.a') from products
select attributes -> '$.a', attributes ->> '$.a' from products
select * from json_each('[1,2,3]')
select value from json_each('[1,2,3]')
select j.value from products p, json_each(p.attributes) j
select json_group_array(id), json_group_object(id, first_name) from customers
select json_object('a', 1), json_array(1, 2), json_valid('{}')
select count(*) filter (where status = 'pending') from orders
select id, row_number() over (order by id) from orders
select id, sum(total) over (order by id rows between unbounded preceding and current row) from orders
select id, sum(total) over (partition by customer_id order by id range between unbounded preceding and current row) from orders
select id, sum(total) over w from orders window w as (order by id)
select id, lag(total) over (order by id), lead(total) over (order by id) from orders
select id, first_value(total) over (order by id) from orders
select id, sum(total) over (order by id rows between 1 preceding and 1 following exclude no others) from orders
select * from orders order by total desc nulls last
select * from orders order by total nulls first
select * from customers c join orders o on o.customer_id = c.id
select * from customers c left join orders o on o.customer_id = c.id
select * from customers c left outer join orders o using (id)
select * from customers c cross join orders o
select * from customers c natural join orders o
select * from customers c natural left join orders o
select * from customers c join orders o
select * from customers, orders
select * from customers c1 join customers c2 join customers c3 on c1.id = c3.id on c2.id = c3.id
select * from (customers c join orders o on o.customer_id = c.id)
select * from (select 1 as a) t
select * from (values (1, 'a'), (2, 'b'))
select * from (values (1, 'a'), (2, 'b')) as t(a, b)
select * from (select * from customers) where id = 1
values (1), (2)
values (1, 2), (3, 4)
with t as (select 1 as a) select * from t
with t(a) as (select 1) select * from t
with t as materialized (select 1 as a) select * from t
with t as not materialized (select 1 as a) select * from t
with recursive t(n) as (select 1 union all select n + 1 from t where n < 5) select * from t
with t as (select id from customers) delete from orders where customer_id in (select id from t)
with t as (select id from customers) update orders set note = 'x' where customer_id in (select id from t)
with t as (select id from customers) insert into orders (customer_id, ordered_at, total) select id, 'x', 0 from t
select 1 union select 2
select 1 union all select 2
select 1 intersect select 1
select 1 except select 2
select id from orders union select id from customers order by 1
select id from orders union select id from customers order by id limit 5
select case when 1 then 2 end, case 1 when 1 then 2 else 3 end
select (select 1), (select count(*) from orders)
select exists (select 1), not exists (select 1)
select * from customers where id in (select customer_id from orders)
select * from customers where (id, country) in (select customer_id, 'FR' from orders)
select * from customers where (id, country) = (1, 'FR')
select (1, 2) = (1, 2)
select row(1, 2) = row(1, 2)
select id from customers where id in ()
select id from customers where id in (1)
select id from customers where id in (1, 2, 3)
select id from customers where id not in ()
select 1 as a, 2 as b order by a
select 'a' || 'b', 1 + 2, 5 % 2, 5 / 2, -5, +5, ~5, 1 << 2, 1 >> 2, 1 & 2, 1 | 2
select 1 = 1, 1 == 1, 1 <> 2, 1 != 2, 1 < 2, 1 <= 2, 1 > 2, 1 >= 2
select 1 is null, 1 is not null, null is null
select true, false, null, current_time, current_date, current_timestamp
select * from customers where marketing_opt_in
select raise(abort, 'x')
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now')
insert or replace into customers (id, first_name, last_name, email, created_at) values (1, 'A', 'B', 'x@y.z', 'now')
insert or ignore into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now')
insert or abort into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now')
insert or fail into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now')
insert or rollback into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now')
replace into customers (id, first_name, last_name, email, created_at) values (1, 'A', 'B', 'x@y.z', 'now')
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now') on conflict do nothing
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now') on conflict (email) do nothing
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now') on conflict (email) do update set first_name = excluded.first_name
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now') on conflict (email) do update set first_name = excluded.first_name where excluded.first_name <> customers.first_name
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now') on conflict (email) where email is not null do nothing
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now') returning id
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now') returning *
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now') returning id as new_id, email
insert into customers (first_name, last_name, email, created_at) select first_name, last_name, 'x' || id, created_at from customers
insert into customers (first_name, last_name, email, created_at) select first_name, last_name, 'x' || id, created_at from customers where true on conflict do nothing
insert into customers default values
insert into main.customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now')
insert into customers as c (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', 'now')
update customers set city = 'x'
update or replace customers set city = 'x'
update or ignore customers set city = 'x' where id = 1
update customers set city = 'x' where id = 1 returning id
update customers set (city, country) = ('x', 'y') where id = 1
update customers set city = o.note from orders o where o.customer_id = customers.id
update customers set city = o.note from orders o where o.customer_id = customers.id returning customers.id
update customers set city = 'x' order by id limit 1
update customers as c set city = 'x' where c.id = 1
update customers indexed by idx_t set city = 'x'
delete from customers
delete from customers where id = 1
delete from customers where id = 1 returning *
delete from customers order by id limit 1
delete from main.customers where id = 1
delete from customers as c where c.id = 1
delete from customers indexed by idx_t where id = 1
create table t (id integer primary key autoincrement, name text not null default '')
create table t (id integer primary key, name text unique, n integer check (n > 0))
create table t (a, b, c)
create table t (a int, b text) without rowid
create table t (a integer primary key, b) strict
create table t (a integer, b integer, primary key (a, b)) without rowid, strict
create table t (a integer references customers (id) on delete cascade deferrable initially deferred)
create table t (a integer, foreign key (a) references customers (id))
create table t (a integer as (b + 1), b integer)
create table t (a integer generated always as (b + 1) stored, b integer)
create table t (a text collate nocase, b integer not null on conflict replace)
create table if not exists t (a integer)
create temp table t (a integer)
create temporary table t (a integer)
create table t as select * from customers
create table main.t (a integer)
create virtual table t using fts5 (a, b)
create virtual table t using rtree (id, minx, maxx)
create index idx on customers (email)
create unique index idx on customers (email collate nocase desc)
create index idx on customers (lower(email))
create index idx on customers (email) where email is not null
create index if not exists idx on customers (email)
create view v as select * from customers
create view v (a, b) as select id, first_name from customers
create view if not exists v as select * from customers
create temp view v as select * from customers
create trigger tr after insert on customers begin insert into orders (customer_id, ordered_at, total) values (new.id, 'x', 0); end
create trigger tr before update on customers for each row when new.id <> old.id begin select raise(abort, 'no'); end
create trigger tr after delete on customers begin delete from orders where customer_id = old.id; update products set price = price + 1; end
create trigger if not exists tr instead of insert on v begin insert into customers (first_name) values (new.a); end
create trigger tr after update of city on customers begin select 1; end
alter table customers rename to clients
alter table customers rename column city to town
alter table customers add column notes text
alter table customers add column notes text default 'x' not null
alter table customers drop column city
drop table customers
drop table if exists customers
drop index idx_t
drop index if exists idx_t
drop view v
drop view if exists v
drop trigger tr
drop trigger if exists tr
pragma foreign_keys = on
pragma foreign_keys
pragma journal_mode = wal
pragma table_info(customers)
pragma table_info('customers')
pragma main.table_info(customers)
pragma index_list(customers)
pragma index_info(idx_t)
pragma foreign_key_list(customers)
pragma database_list
pragma user_version = 3
pragma table_xinfo(customers)
pragma integrity_check
pragma synchronous = normal
pragma cache_size = -2000
attach database ':memory:' as other
attach ':memory:' as other2
detach database other
detach other2
vacuum
vacuum main
vacuum into '/tmp/x.db'
analyze
analyze customers
reindex
reindex customers
begin
begin transaction
begin deferred transaction
begin immediate
begin exclusive transaction
commit
commit transaction
end
end transaction
rollback
rollback transaction
savepoint sp
release sp
release savepoint sp
rollback to sp
rollback transaction to savepoint sp
explain select * from customers
explain query plan select * from customers
explain query plan select * from customers where id = 1
explain insert into customers (first_name) values ('x')
