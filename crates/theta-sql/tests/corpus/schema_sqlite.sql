-- Schema for the embedded engines: the demo tables of crates/theta-drivers/src/demo.rs, reduced to
-- what the corpus needs. Used by the SQLite and DuckDB legs of the differential test.
CREATE TABLE customers (id INTEGER PRIMARY KEY, first_name TEXT NOT NULL, last_name TEXT NOT NULL, email TEXT NOT NULL UNIQUE, city TEXT, country TEXT, created_at TEXT NOT NULL, marketing_opt_in INTEGER NOT NULL DEFAULT 1);
CREATE TABLE products (id INTEGER PRIMARY KEY, sku TEXT NOT NULL UNIQUE, name TEXT NOT NULL, category TEXT NOT NULL, price NUMERIC NOT NULL, cost NUMERIC NOT NULL, attributes TEXT, in_stock INTEGER NOT NULL DEFAULT 0);
CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER NOT NULL REFERENCES customers(id), status TEXT NOT NULL DEFAULT 'pending', ordered_at TEXT NOT NULL, shipped_at TEXT, total NUMERIC NOT NULL, note TEXT);
CREATE TABLE order_items (order_id INTEGER NOT NULL REFERENCES orders(id), line_no INTEGER NOT NULL, product_id INTEGER NOT NULL REFERENCES products(id), quantity INTEGER NOT NULL, unit_price NUMERIC NOT NULL, PRIMARY KEY (order_id, line_no));
CREATE TABLE stores (id INTEGER PRIMARY KEY, name TEXT NOT NULL, city TEXT NOT NULL, country TEXT NOT NULL, opened_on TEXT NOT NULL);
CREATE INDEX idx_orders_customer_id ON orders (customer_id);
CREATE INDEX idx_t ON customers (email);
CREATE TABLE t1 (id INTEGER PRIMARY KEY, name TEXT);
CREATE TABLE t2 (id INTEGER NOT NULL, name TEXT, n INTEGER);
CREATE VIEW customer_order_summary AS SELECT c.id AS customer_id, c.first_name AS customer_name, c.country AS country, count(o.id) AS order_count, sum(o.total) AS total_spent, max(o.ordered_at) AS last_order_at FROM customers c LEFT JOIN orders o ON o.customer_id = c.id GROUP BY c.id;
