-- Hand-written statements against the demo schema (customers, orders, order_items, products, stores).
-- One statement per line; every server judges each line for itself, so a line that one dialect rejects is
-- simply "broken" for that dialect. Expected to be valid in most of PostgreSQL, MySQL/MariaDB, SQLite, DuckDB.
select 1
select 1 as one
select 1, 2, 3
select 'a' as s, 1.5 as n, null as z
select * from customers
select c.* from customers c
select id, first_name from customers
select c.id, c.first_name from customers as c
select c.id, c.first_name, c.last_name from customers c where c.id = 1
select count(*) from customers
select count(*) as n from orders
select count(distinct customer_id) from orders
select count(*), sum(total), avg(total), min(total), max(total) from orders
select status, count(*) from orders group by status
select status, count(*) as n from orders group by status having count(*) > 10
select status, count(*) as n from orders group by status having count(*) > 10 order by n desc
select status, count(*) from orders group by 1 order by 2 desc
select customer_id, sum(total) as spent from orders group by customer_id order by spent desc limit 10
select customer_id, sum(total) from orders group by customer_id order by 2 desc limit 10 offset 5
select * from orders order by ordered_at desc limit 20
select * from orders order by total desc, id asc
select * from orders where total > 100
select * from orders where total >= 100 and total <= 500
select * from orders where total between 100 and 500
select * from orders where total not between 100 and 500
select * from orders where status in ('pending', 'shipped')
select * from orders where status not in ('pending', 'shipped')
select * from orders where status = 'pending' or status = 'shipped'
select * from orders where not (status = 'pending')
select * from orders where shipped_at is null
select * from orders where shipped_at is not null
select * from orders where note is not null and note <> ''
select * from orders where note like '%rush%'
select * from orders where note not like '%rush%'
select * from orders where total <> 0
select * from orders where total != 0
select * from orders where id in (select order_id from order_items)
select * from orders where id not in (select order_id from order_items where quantity > 5)
select * from orders where exists (select 1 from order_items i where i.order_id = orders.id)
select * from orders o where not exists (select 1 from order_items i where i.order_id = o.id)
select * from orders where total > (select avg(total) from orders)
select * from orders where total > all (select total from orders where status = 'pending')
select * from orders where total > any (select total from orders where status = 'pending')
select o.id, c.first_name from orders o join customers c on c.id = o.customer_id
select o.id, c.first_name from orders o inner join customers c on c.id = o.customer_id
select o.id, c.first_name from orders o left join customers c on c.id = o.customer_id
select o.id, c.first_name from orders o left outer join customers c on c.id = o.customer_id
select o.id, c.first_name from orders o right join customers c on c.id = o.customer_id
select o.id, c.first_name from orders o full join customers c on c.id = o.customer_id
select o.id, c.first_name from orders o full outer join customers c on c.id = o.customer_id
select o.id, c.first_name from orders o cross join customers c
select o.id, c.first_name from orders o, customers c where c.id = o.customer_id
select * from orders join order_items using (order_id)
select * from orders o join order_items i on i.order_id = o.id join products p on p.id = i.product_id
select * from orders o join order_items i on i.order_id = o.id join products p on p.id = i.product_id where p.category = 'tools'
select o.id, i.line_no, p.name from orders o join order_items i on i.order_id = o.id left join products p on p.id = i.product_id order by o.id, i.line_no
select * from customers natural join orders
select * from customers c join orders o on o.customer_id = c.id and o.total > 100
select * from customers c join orders o on (o.customer_id = c.id)
select * from (select id, total from orders) t
select t.id from (select id, total from orders) as t where t.total > 5
select t.id, u.n from (select id from orders) t join (select count(*) as n from orders) u on 1 = 1
select * from (select customer_id, sum(total) as spent from orders group by customer_id) s where s.spent > 1000
select * from (select * from (select id from orders) a) b
select a.id from customers a, (select id from orders) b where a.id = b.id
select (select count(*) from orders) as n
select id, (select count(*) from orders o where o.customer_id = c.id) as n from customers c
select id, (select max(total) from orders o where o.customer_id = c.id) from customers c where c.id < 10
with t as (select id from orders) select * from t
with t as (select id, total from orders) select t.id from t where t.total > 1
with t (a, b) as (select id, total from orders) select a, b from t
with a as (select 1 as x), b as (select 2 as y) select * from a, b
with a as (select id from orders), b as (select id from a) select * from b
with a as (select id from orders) select * from a join customers on customers.id = a.id
with recursive r(n) as (select 1 union all select n + 1 from r where n < 10) select * from r
with recursive r(n) as (select 1 union select n + 1 from r where n < 10) select n from r
select 1 union select 2
select 1 union all select 2
select 1 intersect select 1
select 1 except select 2
select id from orders union select id from customers
select id from orders union all select id from customers order by 1
select id from orders union all select id from customers order by id limit 5
(select id from orders) union (select id from customers)
(select id from orders order by id limit 3) union all (select id from customers order by id limit 3)
select id from orders union select id from customers union select id from products
select id from orders union select id from customers intersect select id from products
values (1, 'a'), (2, 'b')
values (1), (2), (3)
select * from (values (1, 'a'), (2, 'b')) as t(n, s)
select case when total > 100 then 'big' else 'small' end from orders
select case status when 'pending' then 1 when 'shipped' then 2 else 0 end from orders
select case when total > 100 then 'big' when total > 10 then 'mid' end as size from orders
select sum(case when status = 'pending' then 1 else 0 end) from orders
select coalesce(note, 'none') from orders
select nullif(total, 0) from orders
select cast(total as integer) from orders
select cast(id as varchar(10)) from orders
select cast('1' as integer)
select -total from orders
select +total from orders
select total * 2 + 1 from orders
select (total + 1) * 2 from orders
select total / 2, total % 2 from orders
select id + 1 as next_id from orders
select abs(-1), round(1.5), upper('a'), lower('A'), length('abc')
select upper(first_name) || ' ' || upper(last_name) from customers
select substr(first_name, 1, 3) from customers
select replace(first_name, 'a', 'b') from customers
select trim(first_name) from customers
select trim(both ' ' from first_name) from customers
select coalesce(city, country, 'x') from customers
select distinct country from customers
select distinct country, city from customers
select count(*) filter (where status = 'pending') from orders
select max(total) over () from orders
select id, row_number() over (order by total desc) from orders
select id, rank() over (partition by customer_id order by total desc) from orders
select id, sum(total) over (partition by customer_id order by ordered_at rows between unbounded preceding and current row) from orders
select id, sum(total) over (partition by customer_id order by ordered_at rows between 1 preceding and 1 following) from orders
select id, sum(total) over (order by ordered_at range between unbounded preceding and current row) from orders
select id, lag(total) over (order by id) as prev, lead(total) over (order by id) as next from orders
select id, sum(total) over w from orders window w as (partition by customer_id)
select id, sum(total) over w, avg(total) over w from orders window w as (partition by customer_id order by id)
select id, first_value(total) over (partition by customer_id order by id) from orders
select id, ntile(4) over (order by total) from orders
select id, count(*) over (partition by status) from orders
select * from orders limit 10
select * from orders limit 10 offset 20
select * from orders order by id limit 5 offset 5
select o.id from orders o where o.customer_id in (select c.id from customers c where c.country = 'FR')
select * from customers where country = 'FR' and (city = 'Paris' or city = 'Lyon')
select * from customers where first_name like 'A%' and last_name like '%son'
select * from customers where email = 'a@b.c'
select * from customers where created_at > '2020-01-01'
select * from customers where created_at >= '2020-01-01' and created_at < '2021-01-01'
select * from customers where marketing_opt_in
select * from customers where marketing_opt_in = true
select * from customers where not marketing_opt_in
select country, count(*) from customers group by country order by count(*) desc
select country, city, count(*) from customers group by country, city order by 1, 2
select c.country, count(distinct o.id) as orders, sum(o.total) as revenue from customers c join orders o on o.customer_id = c.id group by c.country order by revenue desc
select p.category, sum(i.quantity * i.unit_price) as revenue from order_items i join products p on p.id = i.product_id group by p.category
select p.name, sum(i.quantity) from products p left join order_items i on i.product_id = p.id group by p.id, p.name
select p.name, count(i.order_id) from products p left join order_items i on i.product_id = p.id group by p.name having count(i.order_id) = 0
select * from products where price > cost
select * from products where price between 10 and 20 order by price
select name, price - cost as margin from products order by margin desc
select name, round(price - cost, 2) as margin from products
select category, avg(price) from products group by category
select category, min(price), max(price) from products group by category having min(price) < 5
select * from products where category in (select category from products group by category having count(*) > 3)
select s.name, s.city from stores s order by s.name
select * from stores where opened_on > '2015-01-01'
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'a@example.com', '2020-01-01')
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'a@example.com', '2020-01-01'), ('C', 'D', 'c@example.com', '2020-01-02')
insert into customers (first_name, last_name, email, created_at) select first_name, last_name, email, created_at from customers
insert into customers (first_name, last_name, email, created_at) select 'A', 'B', 'x@y.z', current_timestamp
insert into orders (customer_id, status, ordered_at, total) values (1, 'pending', '2020-01-01', 10.5)
insert into order_items (order_id, line_no, product_id, quantity, unit_price) values (1, 1, 1, 2, 3.5)
insert into products (sku, name, category, price, cost) values ('x', 'y', 'z', 1, 1)
insert into customers values (default, 'A', 'B', 'a@b.c', null, null, current_timestamp, true)
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'a@b.c', current_timestamp)
update customers set city = 'Paris'
update customers set city = 'Paris' where id = 1
update customers set city = 'Paris', country = 'FR' where id = 1
update customers set city = upper(city) where city is not null
update customers set marketing_opt_in = false where country = 'FR'
update orders set total = total * 1.1 where status = 'pending'
update orders set status = 'shipped', shipped_at = current_timestamp where id = 1
update orders set note = (select first_name from customers where customers.id = orders.customer_id)
update orders set total = (select sum(quantity * unit_price) from order_items where order_id = orders.id) where id in (select order_id from order_items)
update products set price = price + 1 where category = 'tools'
update products set in_stock = in_stock - 1 where id = 1
delete from customers
delete from customers where id = 1
delete from customers where id in (select customer_id from orders)
delete from orders where status = 'cancelled' and ordered_at < '2020-01-01'
delete from order_items where order_id = 1 and line_no = 1
delete from products where not exists (select 1 from order_items i where i.product_id = products.id)
create table t1 (id integer primary key, name text)
create table t2 (id integer not null, name varchar(10) default 'x', n integer, primary key (id))
create table t3 (a integer, b integer, unique (a, b))
create table t4 (id integer primary key, customer_id integer references customers (id))
create table t5 (id integer, check (id > 0))
create table t6 as select * from customers
create table t7 as select id, first_name from customers where id < 10
create table if not exists t8 (id integer)
create temporary table t9 (id integer)
create index idx_t on customers (email)
create index idx_t2 on customers (country, city)
create unique index idx_t3 on customers (email)
create index if not exists idx_t4 on customers (city)
create view v1 as select id, first_name from customers
create view v2 as select c.id, count(o.id) as n from customers c left join orders o on o.customer_id = c.id group by c.id
create view v3 (a, b) as select id, first_name from customers
drop table customers
drop table if exists t1
drop table if exists t1, t2
drop index idx_orders_customer_id
drop view if exists v1
alter table customers add column notes text
alter table customers add column notes text default 'x'
alter table customers rename to clients
alter table customers rename column city to town
alter table customers drop column city
truncate table customers
begin
commit
rollback
select c.id, c.first_name, count(o.id) from customers c left join orders o on o.customer_id = c.id group by c.id
select c.id, c.first_name, count(o.id) from customers c left join orders o on o.customer_id = c.id group by c.id, c.first_name
select c.country, c.city, count(*) from customers c group by c.country, c.city
select country, count(*) from customers group by country having count(*) > 1
select o.status, sum(o.total) from orders o group by o.status
select o.status, sum(o.total) + 1 from orders o group by o.status
select o.status, avg(o.total) from orders o join customers c on c.id = o.customer_id group by o.status
select country, max(created_at), min(created_at) from customers group by country
select country from customers group by country order by country
select country, count(*) as n from customers group by country order by n desc
select count(*), max(total) from orders
select status, total from orders group by status, total
select p.category, p.name from products p group by p.category, p.name
select * from (select country, count(*) as n from customers group by country) t where n > 1
select a.id from customers a join customers b on a.id = b.id
select a.id, b.id from customers a, customers b where a.id < b.id
select x.id from customers x join orders y on y.customer_id = x.id join order_items z on z.order_id = y.id
select c1.id from customers c1 where c1.id in (select c2.id from customers c2 where c2.id > c1.id)
select (select max(o.total) from orders o where o.customer_id = c.id) as biggest from customers c
select id, first_name from customers where id = (select min(id) from customers)
insert into customers (first_name, last_name, email, created_at) select first_name, last_name, 'x' || id, created_at from customers
insert into order_items (order_id, line_no, product_id, quantity, unit_price) select id, 1, 1, 1, total from orders
update orders set total = (select sum(quantity * unit_price) from order_items i where i.order_id = orders.id)
update customers c set city = 'x' where exists (select 1 from orders o where o.customer_id = c.id)
delete from orders where customer_id in (select id from customers where country = 'FR')
select o.id from orders o where o.customer_id in (select c.id from customers c)
with a as (select id from orders), b as (select id from customers) select a.id, b.id from a join b on a.id = b.id
with a as (select id, total from orders) select a.id from a where a.total > (select avg(total) from a)
select t.n from (select 1 as n union all select 2) t
select t.a, u.b from (select 1 as a) t cross join (select 2 as b) u
select id from customers c join orders o on o.customer_id = c.id join order_items i on i.order_id = o.id where c.id = 1
