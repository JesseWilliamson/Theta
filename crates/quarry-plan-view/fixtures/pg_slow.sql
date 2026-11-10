SELECT o.id, o.ordered_at, o.total, c.last_name, c.country
FROM orders o
JOIN customers c ON c.id = o.customer_id
WHERE o.status = 'shipped' AND o.total > 500 AND o.id % 2 = 0
ORDER BY o.ordered_at DESC, o.total
