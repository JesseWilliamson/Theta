-- DuckDB flavour of schema_sqlite.sql.
-- what the corpus needs. Used by the SQLite and DuckDB legs of the differential test.
CREATE TABLE customers (id INTEGER PRIMARY KEY, first_name TEXT NOT NULL, last_name TEXT NOT NULL, email TEXT NOT NULL UNIQUE, city TEXT, country TEXT, created_at TIMESTAMP NOT NULL, marketing_opt_in BOOLEAN NOT NULL DEFAULT true);
CREATE TABLE products (id INTEGER PRIMARY KEY, sku TEXT NOT NULL UNIQUE, name TEXT NOT NULL, category TEXT NOT NULL, price DECIMAL(12,2) NOT NULL, cost DECIMAL(12,2) NOT NULL, attributes JSON, in_stock INTEGER NOT NULL DEFAULT 0);
CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER NOT NULL REFERENCES customers(id), status TEXT NOT NULL DEFAULT 'pending', ordered_at TIMESTAMP NOT NULL, shipped_at TIMESTAMP, total DECIMAL(12,2) NOT NULL, note TEXT);
CREATE TABLE order_items (order_id INTEGER NOT NULL REFERENCES orders(id), line_no INTEGER NOT NULL, product_id INTEGER NOT NULL REFERENCES products(id), quantity INTEGER NOT NULL, unit_price DECIMAL(12,2) NOT NULL, PRIMARY KEY (order_id, line_no));
CREATE TABLE stores (id INTEGER PRIMARY KEY, name TEXT NOT NULL, city TEXT NOT NULL, country TEXT NOT NULL, opened_on DATE NOT NULL);
CREATE INDEX idx_orders_customer_id ON orders (customer_id);
CREATE INDEX idx_t ON customers (email);
CREATE TABLE t1 (id INTEGER PRIMARY KEY, name TEXT);
CREATE TABLE t2 (id INTEGER NOT NULL, name TEXT, n INTEGER);
CREATE VIEW customer_order_summary AS SELECT c.id AS customer_id, c.first_name AS customer_name, c.country AS country, count(o.id) AS order_count, sum(o.total) AS total_spent, max(o.ordered_at) AS last_order_at FROM customers c LEFT JOIN orders o ON o.customer_id = c.id GROUP BY c.id, c.first_name, c.country;
