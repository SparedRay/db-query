-- Writes, so the confirmation has something to name.
DROP TABLE IF EXISTS batch_probe;
CREATE TABLE batch_probe (id INT PRIMARY KEY, label VARCHAR(40), amount DECIMAL(10,2));
INSERT INTO batch_probe VALUES (1, 'first', 10.50), (2, 'second', 20.25), (3, 'héllo ★', 0.01);
