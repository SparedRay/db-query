-- The fixture Stage 22's measurements and round-trip tests run against.
--
-- Separate from `seed.sql` deliberately: that one is tuned for decoding and
-- the grid, with tiny tables. This one holds the cases a *dump* has to get
-- right — a view on a view, two triggers on one table whose order matters, an
-- event, and a table big enough that row estimates and batch sizes mean
-- something.
--
--   mise run export-probe-up

DROP DATABASE IF EXISTS export_probe;
CREATE DATABASE export_probe;
USE export_probe;

CREATE TABLE t (id INT PRIMARY KEY, n INT);
CREATE TABLE audit (id INT AUTO_INCREMENT PRIMARY KEY, what VARCHAR(50));

-- A chain: v3 -> v2 -> v1 -> t. Re-created in the wrong order, v2 fails.
CREATE VIEW v1 AS SELECT id, n FROM t WHERE n > 0;
CREATE VIEW v2 AS SELECT id FROM v1 WHERE id < 100;
CREATE VIEW v3 AS SELECT a.id FROM v2 a JOIN t b ON b.id = a.id;

-- Two BEFORE INSERT triggers on one table. `SHOW CREATE TRIGGER` emits no
-- FOLLOWS/PRECEDES, so the only thing preserving their order is the order
-- they are created in — which is what ACTION_ORDER reports.
CREATE TRIGGER t_before_ins BEFORE INSERT ON t FOR EACH ROW SET NEW.n = IFNULL(NEW.n, 0);
CREATE TRIGGER t_before_ins2 BEFORE INSERT ON t FOR EACH ROW SET NEW.n = NEW.n + 1;
-- And one whose body names another table *unqualified*, which is why the
-- script must `USE` before creating triggers.
CREATE TRIGGER t_after_ins AFTER INSERT ON t FOR EACH ROW
  INSERT INTO audit (what) VALUES (CONCAT('ins ', NEW.id));

CREATE EVENT e_daily ON SCHEDULE EVERY 1 DAY DO DELETE FROM audit WHERE id < 0;

-- A million rows, wide enough that a batch of 10,000 produces a statement
-- worth measuring against max_allowed_packet.
CREATE TABLE seq (n INT PRIMARY KEY);
INSERT INTO seq (n)
  WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM s WHERE n < 1000)
  SELECT n FROM s;

CREATE TABLE wide (
  id BIGINT AUTO_INCREMENT PRIMARY KEY,
  name VARCHAR(80) NOT NULL,
  email VARCHAR(120),
  amount DECIMAL(12,2),
  note TEXT,
  flag TINYINT(1),
  made_at DATETIME,
  blob_col VARBINARY(32),
  KEY k_name (name)
) ENGINE=InnoDB;

INSERT INTO wide (name, email, amount, note, flag, made_at, blob_col)
SELECT CONCAT('name_', a.n, '_', b.n),
       CONCAT('u', a.n * 1000 + b.n, '@example.com'),
       ROUND(RAND() * 10000, 2),
       REPEAT(CONCAT('note ', b.n, ' '), 4),
       b.n % 2,
       NOW() - INTERVAL (a.n * b.n % 10000) MINUTE,
       UNHEX(MD5(CONCAT(a.n, b.n)))
FROM seq a CROSS JOIN seq b;
