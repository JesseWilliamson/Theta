-- MySQL / MariaDB 10.11 syntax.
select `id`, `first_name` from `customers`
select c.`id` from `customers` as `c`
select `customers`.`id` from `quarry_demo`.`customers`
select quarry_demo.customers.id from quarry_demo.customers
select * from customers limit 5, 10
select * from customers limit 10 offset 5
select * from customers order by id limit 1
select sql_calc_found_rows * from customers limit 5
select distinct country from customers
select distinctrow country from customers
select high_priority * from customers
select straight_join * from customers c join orders o on o.customer_id = c.id
select * from customers c straight_join orders o on o.customer_id = c.id
select * from customers c inner join orders o on o.customer_id = c.id
select * from customers c left join orders o using (id)
select * from customers use index (primary)
select * from customers use index (primary) where id = 1
select * from customers force index (primary) order by id
select * from customers ignore index for order by (primary)
select * from customers c use key (primary) join orders o on o.customer_id = c.id
select * from customers partition (p0)
select group_concat(first_name) from customers
select group_concat(first_name order by first_name separator ', ') from customers
select group_concat(distinct country order by country desc separator '|') from customers
select country, group_concat(city) from customers group by country
select if(total > 100, 'big', 'small') from orders
select ifnull(note, 'none') from orders
select isnull(note) from orders
select date_add(now(), interval 1 day)
select date_sub(now(), interval 1 month)
select now() + interval 1 day
select now() - interval 30 minute
select date_format(now(), '%Y-%m-%d')
select str_to_date('2020-01-01', '%Y-%m-%d')
select timestampdiff(second, '2020-01-01', now())
select timestampadd(minute, 1, now())
select datediff(now(), '2020-01-01')
select extract(year from now())
select extract(day_second from now())
select cast(id as signed) from orders
select cast(id as unsigned) from orders
select cast(id as char) from orders
select cast(id as char(10)) from orders
select cast(total as decimal(10, 2)) from orders
select cast(ordered_at as date) from orders
select cast('1' as json)
select cast(id as char character set utf8mb4) from orders
select convert(id, char) from orders
select convert(id, signed) from orders
select convert(first_name using utf8mb4) from customers
select binary first_name from customers
select _utf8mb4'abc'
select _binary'abc'
select 'a' collate utf8mb4_bin
select first_name collate utf8mb4_bin from customers
select * from customers where first_name = 'a' collate utf8mb4_bin
select * from customers where first_name regexp '^A'
select * from customers where first_name not regexp '^A'
select * from customers where first_name rlike '^A'
select * from customers where first_name like 'A%' escape '|'
select * from customers where first_name sounds like 'Ada'
select * from customers where id <=> null
select * from customers where id div 2 = 1
select * from customers where id mod 2 = 1
select 5 div 2, 5 mod 2, 5 % 2
select 1 xor 0
select 1 && 0, 1 || 0, !1
select ~1, 1 | 2, 1 & 2, 1 ^ 2, 1 << 2, 1 >> 2
select * from customers where match (first_name, last_name) against ('ada')
select * from customers where match (first_name) against ('ada' in boolean mode)
select * from customers where match (first_name) against ('ada' with query expansion)
select match (first_name) against ('ada') as score from customers
select attributes->'$.a' from products
select attributes->>'$.a' from products
select json_extract(attributes, '$.a') from products
select json_unquote(json_extract(attributes, '$.a')) from products
select * from products where attributes->>'$.color' = 'red'
select * from products where json_contains(attributes, '1', '$.a')
select * from products where 1 member of (attributes->'$.a')
select json_object('a', 1, 'b', 2)
select json_arrayagg(id) from customers
select json_objectagg(id, first_name) from customers
select * from json_table('[1,2]', '$[*]' columns (v int path '$')) as jt
select @a
select @a := 1
select @a := @a + 1
select @@version, @@global.max_connections, @@session.sql_mode
select @a, @b := 2 from dual
select 1 from dual
select 1 from dual where 1 = 1
select * from customers into outfile '/tmp/x'
select id into @a from customers limit 1
select id, first_name into @a, @b from customers limit 1
select * from customers for update
select * from customers lock in share mode
select * from customers for share
select * from customers for update nowait
select * from customers for update skip locked
select * from customers where id = ? limit ?
select * from customers where id = ?
select ?, ?
select * from customers order by field(country, 'FR', 'DE')
select * from customers order by rand() limit 1
select * from customers order by 1, 2
select * from customers group by country with rollup
select country, count(*) from customers group by country with rollup
select country, count(*) from customers group by country order by null
select * from customers where country in ('FR') and not city is null
select 'a' 'b'
select "a"
select 'it''s', 'it\'s', "say ""hi""", "it's"
select 0x1f, x'1f', b'101', 0b101
select 1.5e3, .5, 5.
select now(3), curdate(), curtime(), current_timestamp(6), sysdate()
select utc_timestamp(), unix_timestamp(), from_unixtime(0)
select database(), schema(), user(), version(), connection_id(), last_insert_id(), row_count(), found_rows()
select concat('a', 'b'), concat_ws(',', 'a', 'b'), left('abc', 1), right('abc', 1)
select substring('abc', 2), substring('abc' from 2 for 1), substring_index('a.b', '.', 1)
select trim(both 'x' from 'xax'), ltrim(' a'), rtrim('a ')
select locate('a', 'abc'), position('a' in 'abc'), instr('abc', 'a')
select lpad('a', 3, 'x'), rpad('a', 3, 'x'), reverse('abc'), repeat('a', 3)
select round(1.234, 2), truncate(1.234, 2), floor(1.5), ceil(1.5), mod(5, 2)
select count(*) over (partition by country) from customers
select id, row_number() over (order by id) from customers
select id, sum(total) over (order by id rows between unbounded preceding and current row) from orders
select id, sum(total) over w from orders window w as (order by id)
select id, lag(total) over (order by id), lead(total, 2, 0) over (order by id) from orders
select id, first_value(total) over (order by id), nth_value(total, 2) over (order by id) from orders
select id, percent_rank() over (order by total), cume_dist() over (order by total) from orders
with t as (select 1 as a) select * from t
with recursive t(n) as (select 1 union all select n + 1 from t where n < 5) select * from t
with t as (select id from customers) update orders set note = 'x' where customer_id in (select id from t)
with t as (select id from customers) delete from orders where customer_id in (select id from t)
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) on duplicate key update first_name = values(first_name)
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) on duplicate key update first_name = 'x', last_name = 'y'
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) as new on duplicate key update first_name = new.first_name
insert ignore into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now())
insert low_priority into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now())
insert into customers set first_name = 'A', last_name = 'B', email = 'x@y.z', created_at = now()
insert into customers (first_name, last_name, email, created_at) value ('A', 'B', 'x@y.z', now())
insert into customers (first_name, last_name, email, created_at) values row('A', 'B', 'x@y.z', now())
insert into customers (first_name, last_name, email, created_at) select first_name, last_name, concat(id, email), created_at from customers
insert into customers (first_name, last_name, email, created_at) (select first_name, last_name, concat(id, email), created_at from customers)
insert into customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now()) returning id
insert into customers values (default, 'A', 'B', 'a@b.c', null, null, now(), true)
insert into customers () values ()
insert customers (first_name, last_name, email, created_at) values ('A', 'B', 'x@y.z', now())
replace into customers (id, first_name, last_name, email, created_at) values (1, 'A', 'B', 'x@y.z', now())
replace customers set id = 1, first_name = 'A', last_name = 'B', email = 'x@y.z', created_at = now()
update customers set city = 'Paris' limit 1
update customers set city = 'Paris' order by id limit 1
update ignore customers set city = 'Paris'
update low_priority customers set city = 'Paris'
update customers c join orders o on o.customer_id = c.id set c.city = 'x' where o.id = 1
update customers c, orders o set c.city = 'x', o.note = 'y' where o.customer_id = c.id
update customers c inner join orders o on o.customer_id = c.id set c.city = 'x'
update customers c left join orders o on o.customer_id = c.id set c.city = 'x' where o.id is null
update customers set city = 'x' where id in (select customer_id from orders)
update customers set first_name = concat(first_name, '!') where id = 1
delete from customers limit 1
delete from customers order by id limit 1
delete quick from customers
delete ignore from customers
delete c from customers c join orders o on o.customer_id = c.id where o.id = 1
delete c, o from customers c join orders o on o.customer_id = c.id
delete from c using customers c join orders o on o.customer_id = c.id
delete from customers using customers join orders on orders.customer_id = customers.id
delete from customers where id = 1 returning *
create table t (id int unsigned not null auto_increment primary key, name varchar(10) character set utf8mb4 collate utf8mb4_bin) engine=innodb default charset=utf8mb4
create table t (id int auto_increment, primary key (id)) engine = innodb auto_increment = 10
create table t (a int, b int, key (a), unique key (b), index idx (a, b))
create table t (a int, fulltext key (b))
create table t (a enum('x', 'y') not null default 'x', b set('a', 'b'))
create table t (a tinyint(1), b smallint unsigned, c mediumint, d bigint, e float, f double, g decimal(10, 2) zerofill)
create table t (a datetime default current_timestamp on update current_timestamp, b timestamp(6) null)
create table t (a json, b blob, c text, d mediumtext, e longblob, f binary(16), g varbinary(10))
create table t (a int as (b + 1), b int)
create table t (a int generated always as (b + 1) virtual, b int)
create table t (a int, b int generated always as (a * 2) stored)
create table t (a int, check (a > 0))
create table t (a int, constraint chk check (a > 0))
create table t (a int, constraint fk foreign key (a) references customers (id) on delete cascade)
create table t (a int comment 'x') comment = 'y'
create table t like customers
create table t (like customers)
create table t as select * from customers
create table t select * from customers
create table t (id int) select id from customers
create table if not exists t (a int)
create temporary table t (a int)
create table t (a int) partition by hash (a) partitions 4
create table t (a int) row_format = dynamic
create index idx on customers (email)
create index idx on customers (email(10))
create index idx on customers (email desc)
create unique index idx on customers (email) using btree
create fulltext index idx on customers (first_name)
create index idx on customers ((lower(email)))
create view v as select * from customers
create or replace view v as select * from customers
create algorithm = merge view v as select * from customers
create definer = current_user sql security invoker view v as select * from customers
create view v (a, b) as select id, first_name from customers
create view v as select * from customers with check option
create database d
create database if not exists d character set utf8mb4
create schema d
create user 'u'@'%' identified by 'x'
create function f(a int) returns int deterministic return a + 1
create function f(a int) returns int deterministic begin return a + 1; end
create procedure p(in a int, out b int) begin set b = a + 1; end
create procedure p() begin select 1; end
create procedure p() begin declare x int default 0; while x < 5 do set x = x + 1; end while; end
create procedure p() begin declare done int default 0; declare cur cursor for select id from customers; declare continue handler for not found set done = 1; open cur; read_loop: loop fetch cur into @a; if done then leave read_loop; end if; end loop; close cur; end
create trigger tr before insert on customers for each row set new.created_at = now()
create trigger tr after insert on customers for each row begin insert into orders (customer_id) values (new.id); end
create event ev on schedule every 1 day do delete from orders where status = 'cancelled'
alter table customers add column notes text
alter table customers add column notes text after city
alter table customers add column notes text first
alter table customers add column a int, add column b int
alter table customers add index idx (city)
alter table customers add unique key uk (email)
alter table customers add primary key (id)
alter table customers add constraint fk foreign key (id) references customers (id)
alter table customers drop column city
alter table customers drop index idx
alter table customers drop primary key
alter table customers drop foreign key fk
alter table customers modify column city varchar(100)
alter table customers modify city varchar(100) not null
alter table customers change column city town varchar(100)
alter table customers change city town varchar(100)
alter table customers rename column city to town
alter table customers rename to clients
alter table customers rename index a to b
alter table customers alter column city set default 'x'
alter table customers alter column city drop default
alter table customers engine = innodb
alter table customers auto_increment = 100
alter table customers convert to character set utf8mb4
alter table customers algorithm = inplace, lock = none
alter table customers add column x int, algorithm = copy
alter table customers disable keys
alter table customers add partition (partition p1 values less than (10))
drop table customers
drop table if exists customers, orders
drop temporary table if exists t
drop index idx on customers
drop view if exists v
drop database d
drop database if exists d
drop function f
drop procedure if exists p
drop trigger if exists tr
drop user 'u'@'%'
truncate table customers
truncate customers
rename table customers to clients
rename table customers to clients, orders to ords
show tables
show full tables
show tables from quarry_demo
show tables like 'c%'
show databases
show columns from customers
show full columns from customers from quarry_demo
show create table customers
show create view customer_order_summary
show index from customers
show indexes in customers
show table status
show table status like 'customers'
show variables like 'max%'
show global variables
show session status
show status like 'Threads%'
show processlist
show full processlist
show warnings
show errors
show grants
show grants for 'u'@'%'
show engines
show collation
show character set
show triggers
show events
show function status
show procedure status
show master status
show binary logs
show slave status
show open tables
show privileges
describe customers
describe customers id
desc customers
explain customers
explain select * from customers
explain format=json select * from customers
explain extended select * from customers
explain analyze select * from customers
explain delete from customers
explain update customers set city = 'x'
use quarry_demo
set @a = 1
set @a := 1, @b := 2
set @@session.sql_mode = ''
set session sql_mode = ''
set global max_connections = 100
set names utf8mb4
set names utf8mb4 collate utf8mb4_bin
set character set utf8
set autocommit = 0
set transaction isolation level read committed
set session transaction read only
set foreign_key_checks = 0
set sql_safe_updates = 1
set @a = (select count(*) from customers)
set @a = (select max(id) from customers), @b = 2
set time_zone = '+00:00'
set password = 'x'
begin
begin work
start transaction
start transaction read only
start transaction with consistent snapshot
commit
commit work
rollback
rollback work
savepoint sp
rollback to savepoint sp
rollback to sp
release savepoint sp
lock tables customers read
lock tables customers write, orders read
lock tables customers as c write
unlock tables
analyze table customers
analyze table customers, orders
optimize table customers
check table customers
repair table customers
checksum table customers
flush tables
flush privileges
flush tables with read lock
kill 1
kill query 1
kill connection 1
call p()
call p(1, @a)
call db.p(1)
do 1
do sleep(0)
prepare s from 'select 1'
prepare s from @sql
execute s
execute s using @a
deallocate prepare s
drop prepare s
grant select on customers to 'u'@'%'
grant all privileges on *.* to 'u'@'%' with grant option
grant select on quarry_demo.* to 'u'@'localhost'
revoke select on customers from 'u'@'%'
load data infile '/tmp/x' into table customers
load data local infile '/tmp/x' into table customers fields terminated by ',' lines terminated by '\n' ignore 1 lines
handler customers open
signal sqlstate '45000' set message_text = 'x'
select * from customers c1 join customers c2 on c1.id = c2.id join customers c3 on c2.id = c3.id
select * from customers c1 join (customers c2 join orders o on o.customer_id = c2.id) on c1.id = c2.id
select * from (customers c1 join orders o on o.customer_id = c1.id)
select * from customers c1 join customers c2 join customers c3 on c1.id = c3.id on c2.id = c3.id
select * from customers join orders
select * from customers c1 join customers c2
select * from customers, orders
select * from customers c1 left join (customers c2, orders o) on c1.id = c2.id
select * from t1 natural join t2
select * from customers c1 natural left join customers c2
select * from customers where id in (select customer_id from orders limit 5)
select * from customers where (country, city) in (('FR', 'Paris'))
select * from customers where (id, country) = (1, 'FR')
select (1, 2) = (1, 2)
select row(1, 2) = row(1, 2)
select id from customers where id between 1 and 5 and city is not null or country is null
select -1, - 1, --1, 2 - -1, 2--1
select 1 -- comment
select /* c */ 1
select 1 # comment
select /*! straight_join */ 1
select /*!50000 1 */
select * from customers /* hint */ where id = 1
select /*+ max_execution_time(1000) */ * from customers
select * from `customers` `c` where `c`.`id` = 1
select `a b`, `a``b` from (select 1 as `a b`, 2 as `a``b`) t
