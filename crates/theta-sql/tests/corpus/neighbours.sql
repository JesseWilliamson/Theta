-- The valid side of the typos in typos.sql: forms that look alike and that the engines accept (or reject only
-- for a semantic reason). The parser must stay silent on all of them, in every dialect it is run for.
select * from customers order by id
select * from customers c order by c.id desc
select * from customers order by 1
select id from customers union select id from customers order by id
select id, count(*) over (order by id) from customers
select id, row_number() over (partition by country order by id) from customers
select string_agg(email, ',' order by email) from customers
select * from customers order by id limit 5
update customers set email = 'a' where id = 1
update customers set email = 'a', city = 'b' where id = 1
update customers c set email = 'a' where c.id = 1
update customers set (email, city) = ('a', 'b') where id = 1
update customers set email = case when id = 1 then 'a' else 'b' end
update customers set email = (select max(email) from customers) where id = 1
select * from customers x(a)
select * from (select 1, 2) x(a, b)
select c.country, count(*) from customers c
select c.country cnt from customers c
select c.country as cnt from customers c
select count(*) cnt, sum(id) total from customers
select lower(email) e from customers
select id, (select 1) x from customers
select (id) i from customers
select 1 x union select 2
select case when id = 1 then 'a' end from customers
select case when id = 1 then 'a' else 'b' end x from customers
select case id when 1 then 'a' end from customers
select (case when id = 1 then 'a' end) from customers
select case when id = 1 then case when id = 2 then 'a' end end from customers
select id, case when id = 1 then 'a' end as k from customers order by id
select * from customers where case when id = 1 then true else false end
select sum(case when id = 1 then 1 else 0 end) from customers
select 5 / 2, 5 % 2, 2 * (3 + 4)
select id in (1, 2) x from customers
select id not in (1, 2) x from customers
select id between 1 and (2) x from customers
select email like ('a%') x from customers
select email is null x from customers
select 1 union (select 2)
select (1, 2) = (1, 2) x
select id::text x, cast(id as text) y from customers
select 'a' || ('b') x
