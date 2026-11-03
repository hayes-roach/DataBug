-- DataBug sample database: a small store with customers, products and orders.
CREATE TABLE customers (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    email TEXT,
    city TEXT,
    created_at TEXT
);

CREATE TABLE products (
    id INTEGER PRIMARY KEY,
    sku TEXT NOT NULL,
    name TEXT NOT NULL,
    category TEXT,
    price NUMERIC(10,2),
    stock INTEGER
);

CREATE TABLE orders (
    id INTEGER PRIMARY KEY,
    customer_id INTEGER REFERENCES customers(id),
    status TEXT,
    total NUMERIC(12,2),
    ordered_at TEXT
);

CREATE TABLE order_items (
    id INTEGER PRIMARY KEY,
    order_id INTEGER REFERENCES orders(id),
    product_id INTEGER REFERENCES products(id),
    qty INTEGER,
    unit_price NUMERIC(10,2)
);

INSERT INTO customers (id, name, email, city, created_at)
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 500)
SELECT i,
    CASE i % 8 WHEN 0 THEN 'Avery' WHEN 1 THEN 'Jordan' WHEN 2 THEN 'Riley' WHEN 3 THEN 'Morgan' WHEN 4 THEN 'Casey' WHEN 5 THEN 'Quinn' WHEN 6 THEN 'Harper' ELSE 'Rowan' END
      || ' ' ||
    CASE (i / 8) % 8 WHEN 0 THEN 'Brooks' WHEN 1 THEN 'Ellis' WHEN 2 THEN 'Hayes' WHEN 3 THEN 'Porter' WHEN 4 THEN 'Reed' WHEN 5 THEN 'Sutton' WHEN 6 THEN 'Wells' ELSE 'Vance' END,
    'customer' || i || '@example.com',
    CASE i % 5 WHEN 0 THEN 'Atlanta' WHEN 1 THEN 'Savannah' WHEN 2 THEN 'Macon' WHEN 3 THEN 'Augusta' ELSE 'Athens' END,
    '2025-' || CASE WHEN 1 + i % 12 < 10 THEN '0' ELSE '' END || (1 + i % 12) || '-' || CASE WHEN 1 + i % 28 < 10 THEN '0' ELSE '' END || (1 + i % 28)
FROM n;

INSERT INTO products (id, sku, name, category, price, stock)
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 60)
SELECT i,
    'SKU-' || (1000 + i),
    CASE i % 6 WHEN 0 THEN 'Copper wire' WHEN 1 THEN 'Cable reel' WHEN 2 THEN 'Conduit' WHEN 3 THEN 'Junction box' WHEN 4 THEN 'Breaker' ELSE 'Connector kit' END || ' ' || i,
    CASE i % 4 WHEN 0 THEN 'Wire' WHEN 1 THEN 'Cable' WHEN 2 THEN 'Hardware' ELSE 'Electrical' END,
    (i * 137 % 20000) / 100.0 + 5,
    (i * 53) % 400
FROM n;

INSERT INTO orders (id, customer_id, status, total, ordered_at)
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000)
SELECT i,
    1 + (i * 7) % 500,
    CASE i % 10 WHEN 0 THEN 'cancelled' WHEN 1 THEN 'pending' WHEN 2 THEN 'pending' ELSE 'shipped' END,
    (i * 3731 % 250000) / 100.0 + 10,
    '2026-' || CASE WHEN 1 + i % 10 < 10 THEN '0' ELSE '' END || (1 + i % 10) || '-' || CASE WHEN 1 + i % 28 < 10 THEN '0' ELSE '' END || (1 + i % 28)
FROM n;

INSERT INTO order_items (id, order_id, product_id, qty, unit_price)
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 15000)
SELECT i, 1 + (i - 1) / 3, 1 + (i * 11) % 60, 1 + i % 9, (i * 97 % 15000) / 100.0 + 2
FROM n;

CREATE INDEX idx_orders_customer ON orders (customer_id);

CREATE VIEW customer_totals AS
SELECT c.id, c.name, c.city, COUNT(o.id) AS order_count, SUM(o.total) AS lifetime_total
FROM customers c
LEFT JOIN orders o ON o.customer_id = c.id
GROUP BY c.id, c.name, c.city;
