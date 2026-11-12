-- Typos every checked engine rejects as syntax errors. The differential test asserts that the parser flags
-- each statement that an engine rejects here (the first 18 are the regression list reported from real use).
select id email city from customers;
select id, from customers;
select * form customers;
select * from customers where;
select * from customers c join orders o c.id = o.customer_id;
select count(* from customers;
select * from customers where id = 1 and;
select * from customers order id;
insert into customers (id, email values (1, 'a');
update customers set email 'a' where id = 1;
selct * from customers;
select * from customers group country;
delete customers where id = 1;
select c.country count(*) from customers c;
select * from customers where id in (1, 2;
select * from customers limit;
select case when id = 1 then 'a' from customers;
select * from customers where email like;
-- ORDER without BY, in other positions
select * from customers order
select * from customers where id = 1 order id desc
select id from customers order id limit 5
select id from customers union select id from customers order id
select * from customers c order c.id
select * from customers order by id, order
-- SET target without =
update customers set email 1 where id = 1
update customers set email where id = 1
update customers set email, id = 1
update customers set email id where id = 1
update customers set id = 1, email 'a' where id = 1
update customers c set c.email 'a' where c.id = 1
-- a select item followed by a call: the comma is missing
select c.country count(*) from customers c
select id lower(email) from customers
select 1 x(1)
select id x (y) from customers
select count(*) sum(id) from customers
select 1 as x(1)
-- CASE that is not closed
select case when id = 1 then 'a' else 'b' from customers
select case when id = 1 then 'a', id from customers
select case id from customers
select (case when id = 1 then 'a') from customers
select case when id = 1 then 'a' where id > 1
select case id when 1 then 'a' from customers
select id, case when id = 1 then 'a' from customers order by id
select * from customers where case when id = 1 then true group by id
select id, (case when id > 1 then 'x' else 'y') as k from customers
