-- Deliberately broken statements. The servers decide per dialect whether each is a syntax error (some are
-- valid in one dialect: `select from t` is legal in PostgreSQL). The share the parser also flags is reported;
-- it is a measure of coverage, never an assertion.
select
select from
select from customers
select * from
select * from where
select * from customers where
select * from customers where id =
select * from customers where id = and name = 'x'
select * from customers where and id = 1
select * from customers where id = 1 and
select * from customers where id = 1 or
select * from customers where not
select * from customers where id in
select * from customers where id in ()
select * from customers where id in (1, 2
select * from customers where id in 1, 2)
select * from customers where id between 1
select * from customers where id between 1 and
select * from customers where id is
select * from customers where id like
select * from customers where (id = 1
select * from customers where id = 1)
select * from customers where ((id = 1)
select * from customers where id = (1
select * from customers where id = ((1)
select * from customers where id > > 1
select * from customers where id = = 1
select * from customers where id + 1 +
select * from customers where id * 
select * from customers where id || 
select * from customers where id =* 1
select * from customers where id == 
select * from customers order by
select * from customers order by id,
select * from customers order by , id
select * from customers group by
select * from customers group by id,
select * from customers group by , id
select * from customers having
select * from customers limit
select * from customers limit offset 5
select * from customers offset
select * from customers fetch
select * from customers join
select * from customers join orders
select * from customers join orders on
select * from customers join orders using
select * from customers join orders using (
select * from customers join orders using (id
select * from customers left join
select * from customers inner join orders on customers.id =
select * from customers c join orders o on c.id = o.customer_id join
select * from customers, 
select * from customers,
select * from , customers
select * from (select 1
select * from (select 1) 
select * from (select
select * from ()
select * from (
select * from customers c join (select 1
select a, from customers
select a,, b from customers
select , a from customers
select a b c from customers
select a as from customers
select a as as b from customers
select a from customers c d
select 1 +
select 1 + + 
select 1 -
select 1 *
select 1 /
select 1 =
select 1 <
select 1 <=
select 1 ||
select 1 and
select 1 or
select 1 is
select (1
select (1 +
select ((1)
select 1)
select 1))
select (1))
select (select
select (select 1
select (select 1) +
select count(
select count(*
select count(*) +
select count(* from customers
select coalesce(1,
select coalesce(1, 
select coalesce(,1)
select case
select case when
select case when 1
select case when 1 then
select case when 1 then 2
select case when 1 then 2 else
select case when 1 then 2 else 3
select case end
select case 1 end
select cast(1 as
select cast(1 as integer
select cast(1)
select 'abc
select "abc
select `abc
select 'abc from customers
select * from customers where first_name = 'abc
select /* abc
select 1 /* abc
select 1 -- comment ok
select $$ abc
select $1 +
select ? +
select * from customers where id = ? and
select * from customers where id = :id and
select * from customers where id = @id and
select * from customers where
select * from customers where id = 1;;
select * from customers where id = 1;
select * from customers;;
select * form customers
select * frm customers
select * fro customers
select * from customer where id = 1
select * from customers wher id = 1
select * from customers where id = 1 grup by id
select * from customers orderby id
select * from customers order id
select * from customers group id
select * from customers join orders customers.id = orders.customer_id
select * from customers join orders o ON
select * from customers c where c.id = 1 and c.
select * from customers c where c. = 1
select * from customers c where . = 1
select c. from customers c
select .c from customers c
select c.. from customers c
select * from customers. 
select * from .customers
select * from customers..
select * from a. b
select count(*) from customers group by
select distinct
select distinct from customers
select all from customers
select top
select * from customers union
select * from customers union select
select * from customers union all
select * from customers union all select
select * from customers intersect
select * from customers except
select union select 1
select 1 union union select 2
(select 1
(select 1) union
(select 1) union (select
select 1 union (select 1
with
with t
with t as
with t as (
with t as (select 1
with t as (select 1)
with t as (select 1),
with t as (select 1), select * from t
with t as select 1 select * from t
with recursive
with recursive t as (select 1 union all select * from t) 
values
values (
values (1
values (1,
values (1, 2), (
values (1), (2
values ()
values 1
insert
insert into
insert into customers
insert into customers (
insert into customers (id
insert into customers (id)
insert into customers (id) values
insert into customers (id) values (
insert into customers (id) values (1
insert into customers (id) values (1,
insert into customers (id) values (1), (
insert into customers (id) values (1) (2)
insert into customers (id) values 1
insert into customers (id) select
insert into customers (id) select from
insert into customers values
insert into customers set
insert customers
insert into (id) values (1)
insert into customers (id, ) values (1)
insert into customers (, id) values (1)
insert into customers (id,, name) values (1, 2)
insert into customers (id) values (1,)
insert into customers (id) values (,1)
insert into customers (id) values (1) on
insert into customers (id) values (1) on conflict
insert into customers (id) values (1) on conflict do
insert into customers (id) values (1) on conflict (id)
insert into customers (id) values (1) on conflict (id) do
insert into customers (id) values (1) on conflict (id) do update
insert into customers (id) values (1) on conflict (id) do update set
insert into customers (id) values (1) on conflict (id) do update set id =
insert into customers (id) values (1) on duplicate
insert into customers (id) values (1) on duplicate key
insert into customers (id) values (1) on duplicate key update
insert into customers (id) values (1) on duplicate key update id =
insert into customers (id) values (1) returning
insert into customers (id) values (1) returning ,
update
update customers
update customers set
update customers set id
update customers set id =
update customers set id = 1,
update customers set id = 1, where id = 1
update customers set = 1
update customers set id = 1 where
update customers set id = 1 where id =
update set id = 1
update customers id = 1
update customers set id == 1
update customers set id = 1 from
update customers set id = 1 from orders where
update customers set id = 1 returning
delete
delete from
delete from customers where
delete from customers where id =
delete from customers where id in (
delete customers where id = 1
delete from where id = 1
delete from customers using
delete from customers returning
merge
merge into
merge into customers
merge into customers using
merge into customers using customers
merge into customers using customers on
merge into customers using customers on true
merge into customers using customers on true when
merge into customers using customers on true when matched
merge into customers using customers on true when matched then
create
create table
create table t
create table t (
create table t (a
create table t (a int
create table t (a int,
create table t (a int,)
create table t (,a int)
create table t (a int b)
create table t (a int) (b int)
create table (a int)
create table t as
create table t as select
create view
create view v
create view v as
create view v as select
create view as select 1
create index
create index idx
create index idx on
create index idx on customers
create index idx on customers (
create index idx on customers (a
create index on
create unique
create or replace
alter
alter table
alter table customers
alter table customers add
alter table customers add column
alter table customers drop
alter table customers drop column
alter table customers rename
alter table customers rename to
alter table customers rename column
alter table customers rename column a
alter table customers rename column a to
alter table add column x int
drop
drop table
drop table if
drop table if exists
drop table customers,
drop table ,customers
drop index
drop view
truncate
truncate table
grant
grant select
grant select on
grant select on customers
grant select on customers to
set
set x
set x =
set x = 
show
begin;begin
explain
explain select
explain select * from
explain analyze
explain (
explain (analyze select 1
update customers set (a, b) = (1
update customers set (a, b = (1, 2)
select * from customers c join orders o on c.id = o.customer_id and
select * from customers c join orders o on (c.id = o.customer_id
select * from customers c join orders o on c.id = o.customer_id)
select * from customers where id in (select
select * from customers where id in (select 1
select * from customers where exists (
select * from customers where exists (select
select * from customers where exists select 1
select * from customers where id = any (
select * from customers where id = all (select
select count(*) over (
select count(*) over (partition by
select count(*) over (order by
select count(*) over (partition by id order by
select count(*) over w from customers window w as (
select count(*) over w from customers window
select sum(x) filter (where
select sum(x) filter (
select string_agg(x, ',' order by
select x from t where x::
select x::
select x:: from t
select [1, 2
select {'a': 1
select array[1, 2
select array[
select 1 from customers limit 1 2
select * from customers where id = 1 2
select * from customers where id = 1 and 2 3
select * from customers where 1 = 1 = 
select 1 from
select 1 from customers where
select 1 where
select 1 group by
select 1 order by
select 1 having
select 1 limit
select 1 offset
select -
select +
select not
select ~
select !
select @
select #
select *,
select *, from customers
select *, * from
select c.*, from customers c
select count(*), from customers
select count(*),, sum(1) from customers
select (a, b from customers
select a from customers where (a = 1 and (b = 2
select a from customers where a = 1 and (b = 2 or c = 3
select a from customers where a = 1 and b = 2) or (c = 3
select a from (select b from t where c = 1
select a from (select b from t) where
select a from (select b from t) x where
select a from (select b from t) x join
select a from t1 join t2 on t1.a = t2.a join t3 on
select a from t1 left join t2 on t1.a = t2.a left join t3
select a from t1 where a = 1 union select b from t2 where
select a from t1 where a in (select b from t2 where
select a from t1 where a in (select b from t2 where c = 1
select a from t1 where exists (select 1 from t2 where t2.a = t1.a
select a, b, c, from t
select a, b, c,
select a, b, c, from
select a from t where b = 1 and c = 2 and
select a from t where b = 1 and c = 2 or
select a from t where b = 1 and (c = 2 or)
select a from t where b = 1 and (or c = 2)
select a from t where (and b = 1)
select a from t where b = 1 and ()
select a from t where b in (1, 2,)
select a from t where b in (,1, 2)
select a from t where b in (1,, 2)
select a from t where b not
select a from t where b not 1
select a from t where b is not
select a from t where b is not distinct
select a from t where b is distinct from
select a from t where b like 'x' escape
select a from t order by b desc asc, 
select a from t order by b nulls
select a from t order by b nulls first,
select a from t limit 1, 
select a from t limit , 1
select a from t limit 1 offset
select a from t fetch first
select a from t fetch first 5
select a from t for
select a from t for update of
select 1 in
select 1 like
select 1 not
select 1 between
select 1 isnull
select 1 collate
select a and from customers
select a or from customers
select a is from customers
select a not from customers
