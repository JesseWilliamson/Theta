SELECT p.category, c.country, count(DISTINCT o.id) AS orders, sum(oi.quantity * oi.unit_price) AS revenue
FROM customers c
JOIN orders o ON o.customer_id = c.id
JOIN order_items oi ON oi.order_id = o.id
JOIN products p ON p.id = oi.product_id
WHERE o.ordered_at >= now() - INTERVAL 5 YEAR AND c.marketing_opt_in
GROUP BY p.category, c.country
ORDER BY revenue DESC
LIMIT 20
