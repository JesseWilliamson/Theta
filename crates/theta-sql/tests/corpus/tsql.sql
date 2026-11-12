-- T-SQL examples written from the Microsoft documentation (Transact-SQL reference: SELECT, JOIN, APPLY, CTE,
-- TOP, OUTPUT, MERGE, PIVOT, OFFSET FETCH, temporary tables, variables, control flow). No SQL Server is
-- available locally; these are assumed valid as documented and are only checked for "no false error".
SELECT 1
SELECT 1 AS one, 2 AS two
SELECT TOP 10 * FROM customers
SELECT TOP (10) * FROM customers
SELECT TOP 10 PERCENT * FROM customers
SELECT TOP (10) WITH TIES * FROM customers ORDER BY id
SELECT DISTINCT TOP 5 country FROM customers
SELECT c.id, c.first_name FROM dbo.customers AS c
SELECT [id], [first name] FROM [dbo].[customers]
SELECT * FROM mydb.dbo.customers
SELECT * FROM mydb..customers
SELECT * FROM customers WITH (NOLOCK)
SELECT * FROM customers AS c WITH (NOLOCK, INDEX(1))
SELECT * FROM customers c WITH (NOLOCK) JOIN orders o WITH (NOLOCK) ON o.customer_id = c.id
SELECT * FROM customers WHERE id = @id
SELECT * FROM customers WHERE id = @id AND country = @country
SELECT @@VERSION, @@ROWCOUNT, @@IDENTITY, @@SERVERNAME
SELECT @x = 1
SELECT @x = id FROM customers
SELECT alias = first_name FROM customers
SELECT 'x' AS [a b], N'unicode' AS n
SELECT first_name + ' ' + last_name FROM customers
SELECT ISNULL(city, 'x'), COALESCE(city, country, 'x') FROM customers
SELECT IIF(id > 1, 'a', 'b') FROM customers
SELECT CASE WHEN id > 1 THEN 'a' ELSE 'b' END FROM customers
SELECT CAST(id AS VARCHAR(10)), CONVERT(VARCHAR(10), id), CONVERT(VARCHAR(10), created_at, 120) FROM customers
SELECT TRY_CAST('1' AS INT), TRY_CONVERT(INT, '1'), TRY_PARSE('1' AS INT)
SELECT GETDATE(), SYSDATETIME(), GETUTCDATE(), CURRENT_TIMESTAMP
SELECT DATEADD(day, 1, GETDATE()), DATEDIFF(day, '2020-01-01', GETDATE()), DATEPART(year, GETDATE()), DATENAME(month, GETDATE())
SELECT YEAR(GETDATE()), MONTH(GETDATE()), DAY(GETDATE()), EOMONTH(GETDATE())
SELECT LEN('abc'), SUBSTRING('abc', 1, 2), CHARINDEX('a', 'abc'), REPLACE('abc', 'a', 'b'), LTRIM(' a'), RTRIM('a ')
SELECT STRING_AGG(first_name, ', ') FROM customers
SELECT STRING_AGG(first_name, ', ') WITHIN GROUP (ORDER BY first_name) FROM customers
SELECT COUNT(*), COUNT_BIG(*), SUM(id), AVG(id), MIN(id), MAX(id) FROM customers
SELECT country, COUNT(*) FROM customers GROUP BY country HAVING COUNT(*) > 1 ORDER BY COUNT(*) DESC
SELECT country, city, COUNT(*) FROM customers GROUP BY ROLLUP (country, city)
SELECT country, city, COUNT(*) FROM customers GROUP BY CUBE (country, city)
SELECT country, city, COUNT(*) FROM customers GROUP BY GROUPING SETS ((country), (city), ())
SELECT * FROM customers ORDER BY id OFFSET 10 ROWS FETCH NEXT 5 ROWS ONLY
SELECT * FROM customers ORDER BY id OFFSET 10 ROWS
SELECT * FROM customers ORDER BY id OFFSET @n ROWS FETCH NEXT @m ROWS ONLY
SELECT * FROM customers c JOIN orders o ON o.customer_id = c.id
SELECT * FROM customers c INNER JOIN orders o ON o.customer_id = c.id
SELECT * FROM customers c LEFT JOIN orders o ON o.customer_id = c.id
SELECT * FROM customers c LEFT OUTER JOIN orders o ON o.customer_id = c.id
SELECT * FROM customers c RIGHT JOIN orders o ON o.customer_id = c.id
SELECT * FROM customers c FULL OUTER JOIN orders o ON o.customer_id = c.id
SELECT * FROM customers c CROSS JOIN orders o
SELECT * FROM customers c CROSS APPLY (SELECT TOP 1 * FROM orders o WHERE o.customer_id = c.id ORDER BY o.id DESC) t
SELECT * FROM customers c OUTER APPLY (SELECT TOP 1 * FROM orders o WHERE o.customer_id = c.id) t
SELECT * FROM customers c CROSS APPLY dbo.fn(c.id) f
SELECT * FROM customers c INNER HASH JOIN orders o ON o.customer_id = c.id
SELECT * FROM customers, orders WHERE customers.id = orders.customer_id
SELECT * FROM (SELECT id FROM customers) AS t
SELECT * FROM (VALUES (1, 'a'), (2, 'b')) AS t (n, s)
SELECT * FROM customers WHERE id IN (SELECT customer_id FROM orders)
SELECT * FROM customers WHERE EXISTS (SELECT 1 FROM orders o WHERE o.customer_id = customers.id)
SELECT * FROM customers WHERE id BETWEEN 1 AND 5
SELECT * FROM customers WHERE first_name LIKE 'A%' ESCAPE '\'
SELECT * FROM customers WHERE first_name LIKE '[A-C]%'
SELECT * FROM customers WHERE city IS NULL OR city IS NOT NULL
SELECT * FROM customers WHERE id = ALL (SELECT 1)
SELECT * FROM customers WHERE id > ANY (SELECT 1)
SELECT * FROM customers WHERE id > SOME (SELECT 1)
SELECT * INTO #tmp FROM customers
SELECT * INTO ##gtmp FROM customers
SELECT * INTO dbo.copy FROM customers
SELECT * FROM #tmp
SELECT * FROM ##gtmp
SELECT t.* FROM #tmp t JOIN customers c ON c.id = t.id
SELECT * FROM @tbl
SELECT * FROM @tbl t
SELECT id FROM customers FOR XML PATH('')
SELECT id FROM customers FOR XML AUTO
SELECT id FROM customers FOR JSON AUTO
SELECT id FROM customers FOR JSON PATH, ROOT('x')
SELECT * FROM customers OPTION (RECOMPILE)
SELECT * FROM customers OPTION (MAXDOP 1, RECOMPILE)
SELECT * FROM OPENJSON('[1,2]')
SELECT * FROM OPENJSON(@json) WITH (a int '$.a', b nvarchar(10) '$.b')
SELECT * FROM STRING_SPLIT('a,b', ',')
SELECT value FROM STRING_SPLIT('a,b', ',')
SELECT * FROM sys.tables
SELECT name FROM sys.objects WHERE type = 'U'
SELECT * FROM INFORMATION_SCHEMA.TABLES
SELECT OBJECT_ID('customers'), DB_NAME(), SCHEMA_NAME(), USER_NAME(), SUSER_SNAME()
SELECT ROW_NUMBER() OVER (ORDER BY id) FROM customers
SELECT ROW_NUMBER() OVER (PARTITION BY country ORDER BY id) FROM customers
SELECT SUM(id) OVER (PARTITION BY country ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) FROM customers
SELECT LAG(id) OVER (ORDER BY id), LEAD(id, 1, 0) OVER (ORDER BY id) FROM customers
SELECT NTILE(4) OVER (ORDER BY id), RANK() OVER (ORDER BY id), DENSE_RANK() OVER (ORDER BY id) FROM customers
SELECT PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY id) OVER (PARTITION BY country) FROM customers
SELECT * FROM customers UNION SELECT * FROM customers
SELECT * FROM customers UNION ALL SELECT * FROM customers
SELECT id FROM customers INTERSECT SELECT customer_id FROM orders
SELECT id FROM customers EXCEPT SELECT customer_id FROM orders
SELECT * FROM (SELECT country, id FROM customers) src PIVOT (COUNT(id) FOR country IN ([FR], [DE])) AS p
SELECT * FROM customers UNPIVOT (v FOR n IN (city, country)) AS u
SELECT NEXT VALUE FOR seq
SELECT 1 WHERE 1 = 1
WITH t AS (SELECT 1 AS a) SELECT * FROM t
WITH t (a) AS (SELECT 1) SELECT * FROM t
WITH t AS (SELECT 1 AS a), u AS (SELECT 2 AS b) SELECT * FROM t, u
WITH t AS (SELECT 1 AS n UNION ALL SELECT n + 1 FROM t WHERE n < 5) SELECT * FROM t
WITH t AS (SELECT id FROM customers) DELETE FROM orders WHERE customer_id IN (SELECT id FROM t)
WITH t AS (SELECT id FROM customers) UPDATE orders SET note = 'x' WHERE customer_id IN (SELECT id FROM t)
INSERT INTO customers (first_name, last_name, email, created_at) VALUES ('A', 'B', 'x@y.z', GETDATE())
INSERT INTO customers VALUES (1, 'A', 'B', 'x@y.z', NULL, NULL, GETDATE(), 1)
INSERT INTO customers (first_name) VALUES ('A'), ('B')
INSERT INTO customers (first_name) SELECT first_name FROM customers
INSERT customers (first_name) VALUES ('A')
INSERT INTO customers (first_name) OUTPUT INSERTED.id VALUES ('A')
INSERT INTO customers (first_name) OUTPUT INSERTED.id, INSERTED.first_name INTO @t (id, name) VALUES ('A')
INSERT INTO customers DEFAULT VALUES
INSERT INTO customers WITH (TABLOCK) (first_name) VALUES ('A')
INSERT INTO #tmp EXEC sp_who
INSERT INTO @tbl (a) VALUES (1)
UPDATE customers SET city = 'x'
UPDATE customers SET city = 'x' WHERE id = 1
UPDATE customers SET city = 'x', country = 'y' WHERE id = 1
UPDATE TOP (10) customers SET city = 'x'
UPDATE customers SET city = o.note FROM customers c JOIN orders o ON o.customer_id = c.id
UPDATE c SET c.city = 'x' FROM customers c JOIN orders o ON o.customer_id = c.id WHERE o.id = 1
UPDATE customers SET city = 'x' OUTPUT INSERTED.id, DELETED.city WHERE id = 1
UPDATE customers SET @x = city
UPDATE customers SET city += 'x'
UPDATE customers SET city = DEFAULT
DELETE FROM customers
DELETE FROM customers WHERE id = 1
DELETE customers WHERE id = 1
DELETE TOP (10) FROM customers
DELETE c FROM customers c JOIN orders o ON o.customer_id = c.id
DELETE FROM customers OUTPUT DELETED.id WHERE id = 1
DELETE FROM c FROM customers c WHERE id = 1
MERGE INTO customers AS t USING (SELECT 1 AS id) AS s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.first_name = 'x' WHEN NOT MATCHED THEN INSERT (id) VALUES (s.id)
MERGE customers AS t USING customers AS s ON t.id = s.id WHEN MATCHED AND t.id > 1 THEN DELETE
MERGE customers AS t USING customers AS s ON t.id = s.id WHEN NOT MATCHED BY SOURCE THEN DELETE
MERGE customers AS t USING customers AS s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.city = s.city OUTPUT $action, INSERTED.id
CREATE TABLE t (id INT IDENTITY(1, 1) PRIMARY KEY, name NVARCHAR(50) NOT NULL DEFAULT '')
CREATE TABLE dbo.t (a INT, b VARCHAR(MAX), c DATETIME2(3), d DECIMAL(10, 2), e BIT, f UNIQUEIDENTIFIER DEFAULT NEWID())
CREATE TABLE t (a INT, CONSTRAINT pk PRIMARY KEY CLUSTERED (a), CONSTRAINT fk FOREIGN KEY (a) REFERENCES customers (id))
CREATE TABLE #t (a INT)
CREATE TABLE ##t (a INT)
CREATE TABLE t (a INT, b AS (a + 1))
CREATE TABLE t (a INT, b AS (a + 1) PERSISTED)
CREATE INDEX idx ON customers (email)
CREATE UNIQUE NONCLUSTERED INDEX idx ON customers (email) INCLUDE (city) WHERE email IS NOT NULL
CREATE CLUSTERED INDEX idx ON t (a)
CREATE VIEW v AS SELECT * FROM customers
CREATE OR ALTER VIEW v AS SELECT * FROM customers
CREATE VIEW v WITH SCHEMABINDING AS SELECT id FROM dbo.customers
CREATE PROCEDURE p @a INT AS SELECT @a
CREATE PROCEDURE p @a INT, @b INT = 1 AS BEGIN SELECT @a + @b END
CREATE FUNCTION f (@a INT) RETURNS INT AS BEGIN RETURN @a + 1 END
CREATE FUNCTION f (@a INT) RETURNS TABLE AS RETURN (SELECT * FROM customers WHERE id = @a)
CREATE TRIGGER tr ON customers AFTER INSERT AS BEGIN SELECT 1 END
CREATE SEQUENCE seq START WITH 1 INCREMENT BY 1
CREATE SCHEMA s
CREATE DATABASE d
ALTER TABLE customers ADD notes NVARCHAR(100)
ALTER TABLE customers ADD COLUMN notes NVARCHAR(100)
ALTER TABLE customers DROP COLUMN city
ALTER TABLE customers ALTER COLUMN city NVARCHAR(100) NOT NULL
ALTER TABLE customers ADD CONSTRAINT u UNIQUE (email)
ALTER TABLE customers DROP CONSTRAINT u
ALTER TABLE customers NOCHECK CONSTRAINT ALL
ALTER INDEX idx ON customers REBUILD
DROP TABLE customers
DROP TABLE IF EXISTS customers
DROP TABLE #tmp
DROP INDEX idx ON customers
DROP VIEW v
DROP PROCEDURE p
TRUNCATE TABLE customers
EXEC sp_who
EXEC sp_who2 'active'
EXEC dbo.p @a = 1, @b = 2
EXECUTE p 1, 2
EXEC('SELECT 1')
EXEC sp_executesql N'SELECT 1'
EXEC sp_rename 'customers.city', 'town', 'COLUMN'
DECLARE @x INT
DECLARE @x INT = 1
DECLARE @x INT = 1, @y NVARCHAR(10) = N'a'
DECLARE @t TABLE (a INT, b VARCHAR(10))
DECLARE @x INT = (SELECT COUNT(*) FROM customers)
DECLARE c CURSOR FOR SELECT id FROM customers
SET @x = 1
SET @x = (SELECT COUNT(*) FROM customers)
SET @x += 1
SET NOCOUNT ON
SET XACT_ABORT ON
SET TRANSACTION ISOLATION LEVEL READ COMMITTED
SET IDENTITY_INSERT customers ON
SET DATEFORMAT ymd
SET ANSI_NULLS ON
USE mydb
PRINT 'x'
PRINT @x
RAISERROR('x', 16, 1)
RAISERROR('x %d', 16, 1, @x) WITH NOWAIT
THROW 50000, 'x', 1
RETURN
RETURN 1
BEGIN TRANSACTION
BEGIN TRAN
BEGIN TRAN t1
COMMIT
COMMIT TRANSACTION
COMMIT TRAN
ROLLBACK
ROLLBACK TRANSACTION
ROLLBACK TRAN t1
SAVE TRANSACTION sp
IF @x = 1 SELECT 1 ELSE SELECT 2
IF EXISTS (SELECT 1 FROM customers) SELECT 1
IF NOT EXISTS (SELECT 1 FROM customers) BEGIN SELECT 1 END
WHILE @x < 5 BEGIN SET @x += 1 END
BEGIN TRY SELECT 1 END TRY BEGIN CATCH SELECT ERROR_MESSAGE() END CATCH
BEGIN SELECT 1 END
GOTO lbl
WAITFOR DELAY '00:00:01'
OPEN c
FETCH NEXT FROM c INTO @x
CLOSE c
DEALLOCATE c
GRANT SELECT ON customers TO public
REVOKE SELECT ON customers FROM public
DENY SELECT ON customers TO public
BACKUP DATABASE d TO DISK = 'x.bak'
DBCC CHECKDB
UPDATE STATISTICS customers
CHECKPOINT
BULK INSERT customers FROM 'x.csv' WITH (FIELDTERMINATOR = ',')
