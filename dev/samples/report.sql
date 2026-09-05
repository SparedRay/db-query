-- Sample script for testing tabs and file open/save.
USE poc;
SELECT id, email, display_name FROM users ORDER BY id;

SELECT u.email, COUNT(o.id) AS orders, SUM(o.total) AS spent
FROM users u
LEFT JOIN orders o ON o.user_id = u.id
GROUP BY u.email;

SHOW TABLES;
