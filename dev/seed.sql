-- Fixture for the M1-M5 milestone checks. Every table here exists to make one
-- specific claim in the tracker falsifiable.

-- The client charset must be explicit. Without it the container's mysql client
-- announces latin1, the server transcodes on the way in, and every accented
-- character in this file is double-encoded before a test ever sees it.
SET NAMES utf8mb4;

CREATE DATABASE IF NOT EXISTS poc CHARACTER SET utf8mb4;
USE poc;

-- M2: type decoding. One column per arm of the CellValue match, plus an
-- all-NULL row so "NULL renders distinctly from the string 'NULL'" is testable.
DROP TABLE IF EXISTS types_zoo;
CREATE TABLE types_zoo (
  id            INT AUTO_INCREMENT PRIMARY KEY,
  c_tinyint     TINYINT,
  c_bool        BOOLEAN,
  c_smallint    SMALLINT,
  c_int         INT,
  c_int_u       INT UNSIGNED,
  c_bigint      BIGINT,
  c_bigint_u    BIGINT UNSIGNED,
  c_float       FLOAT,
  c_double      DOUBLE,
  c_decimal     DECIMAL(20,4),      -- must stay Text: f64 would corrupt money
  c_date        DATE,
  c_datetime    DATETIME(3),
  c_timestamp   TIMESTAMP NULL,
  c_time        TIME,
  c_year        YEAR,
  c_char        CHAR(8),
  c_varchar     VARCHAR(255),
  c_text        TEXT,
  c_json        JSON,
  c_enum        ENUM('a','b','c'),
  c_set         SET('x','y','z'),
  c_binary      BINARY(4),
  c_varbinary   VARBINARY(32),
  c_blob        BLOB,
  c_literal_null VARCHAR(16)        -- holds the STRING "NULL", not a null
);

INSERT INTO types_zoo (
  c_tinyint, c_bool, c_smallint, c_int, c_int_u, c_bigint, c_bigint_u,
  c_float, c_double, c_decimal, c_date, c_datetime, c_timestamp, c_time, c_year,
  c_char, c_varchar, c_text, c_json, c_enum, c_set,
  c_binary, c_varbinary, c_blob, c_literal_null
) VALUES
-- Ordinary row.
(42, TRUE, 1234, 2000000, 4000000000, 123456789, 18446744073709551615,
 1.5, 2.718281828459045, 12345678901234.5678, '2026-09-05', '2026-09-05 12:34:56.789',
 '2026-09-05 12:34:56', '13:45:00', 2026,
 'fixed', 'hello world', 'a longer piece of text', '{"k": [1, 2, 3]}', 'b', 'x,z',
 0x00FF00FF, 0xDEADBEEF, 0x1F8B0800, 'NULL'),

-- BIGINT hazard: both sides of the 2^53 boundary. 9007199254740993 must arrive
-- as Text, or it silently becomes ...992 in the grid.
(-128, FALSE, -32768, -2147483648, 0, 9007199254740993, 9007199254740993,
 -0.5, 1.7976931348623157e308, -0.0001, '1000-01-01', '1000-01-01 00:00:00.000',
 NULL, '-838:59:59', 1901,
 'edge', 'unicode: héllo → 世界 ；', 'tab\there', '[]', 'a', '',
 0x00000000, '', '', 'null'),

-- Every nullable column NULL.
(NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
 NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);

-- M5: auto-LIMIT truncation. Needs to exceed 5000 rows.
-- cte_max_recursion_depth defaults to 1000, so the generator needs headroom.
SET SESSION cte_max_recursion_depth = 10000;
DROP TABLE IF EXISTS big;
CREATE TABLE big (id INT PRIMARY KEY, label VARCHAR(64), n INT);
INSERT INTO big (id, label, n)
WITH RECURSIVE seq(i) AS (
  SELECT 1 UNION ALL SELECT i + 1 FROM seq WHERE i < 7500
)
SELECT i, CONCAT('row-', i), i * 3 FROM seq;

-- M3/M4: a second table so joins, aliases and autocomplete have something real.
--
-- `orders` is dropped first because it holds the foreign key into `users`.
-- Without this the seed only works on a fresh container and fails on every
-- re-run — which is exactly what `mise run db-up` does against an existing one.
DROP TABLE IF EXISTS orders;
DROP TABLE IF EXISTS users;
CREATE TABLE users (
  id INT AUTO_INCREMENT PRIMARY KEY,
  email VARCHAR(190) NOT NULL UNIQUE,
  display_name VARCHAR(80),
  balance DECIMAL(12,2) NOT NULL DEFAULT 0.00,
  created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
INSERT INTO users (email, display_name, balance) VALUES
  ('ada@example.com',   'Ada',   10.00),
  ('grace@example.com', 'Grace', 0.10),
  ('alan@example.com',  NULL,    -5.25);

DROP TABLE IF EXISTS orders;
CREATE TABLE orders (
  id INT AUTO_INCREMENT PRIMARY KEY,
  user_id INT NOT NULL,
  total DECIMAL(12,2) NOT NULL,
  CONSTRAINT fk_orders_user FOREIGN KEY (user_id) REFERENCES users(id)
);
INSERT INTO orders (user_id, total) VALUES (1, 99.99), (1, 1.00), (2, 250.00);

-- M3: a VIEW, so the tree's BASE TABLE vs VIEW distinction is exercised.
CREATE OR REPLACE VIEW user_totals AS
  SELECT u.id, u.email, COALESCE(SUM(o.total), 0) AS spent
  FROM users u LEFT JOIN orders o ON o.user_id = u.id
  GROUP BY u.id, u.email;

-- Stage 3 (E3, E4): a procedure and a function, so routine introspection and
-- the examine round trip have something real to work against.
--
-- `DELIMITER` is a client directive, not SQL — the mysql CLI understands it
-- when this file is piped in, which is how db-up seeds.
DROP PROCEDURE IF EXISTS top_spenders;
DELIMITER $$
CREATE PROCEDURE top_spenders(IN min_total DECIMAL(12,2), IN max_rows INT)
BEGIN
  SELECT u.id, u.email, SUM(o.total) AS spent
  FROM users u JOIN orders o ON o.user_id = u.id
  GROUP BY u.id, u.email
  HAVING spent >= min_total
  ORDER BY spent DESC
  LIMIT max_rows;
END$$
DELIMITER ;

DROP FUNCTION IF EXISTS order_count;
DELIMITER $$
CREATE FUNCTION order_count(uid INT) RETURNS INT
DETERMINISTIC READS SQL DATA
BEGIN
  DECLARE n INT;
  SELECT COUNT(*) INTO n FROM orders WHERE user_id = uid;
  RETURN n;
END$$
DELIMITER ;

-- A no-argument procedure: the CALL snippet generator must not emit a stray
-- placeholder, and an empty parameter list is a distinct code path.
DROP PROCEDURE IF EXISTS ping_poc;
DELIMITER $$
CREATE PROCEDURE ping_poc()
BEGIN
  SELECT 'pong' AS reply;
END$$
DELIMITER ;

-- Columns whose *type name* lies about what they hold. Every one of these was
-- decoded wrongly by the first build to meet a real database:
--   * a string column with a binary collation is reported as VARBINARY, and
--     came out as "<binary, 16 bytes>" over perfectly readable words;
--   * JSON and BIT came out as "<undecodable>", because sqlx's typed accessors
--     reject an incompatible type before decoding anything at all.
-- The two genuinely binary columns are here to keep the fix honest: they must
-- still be reported as binary rather than as mojibake.
DROP TABLE IF EXISTS awkward_types;
CREATE TABLE awkward_types (
  id            INT PRIMARY KEY,
  v_utf8mb4     VARCHAR(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci,
  v_latin1      VARCHAR(64) CHARACTER SET latin1  COLLATE latin1_swedish_ci,
  v_binary_coll VARCHAR(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin,
  c_char        CHAR(8)     CHARACTER SET latin1,
  t_text        TEXT        CHARACTER SET latin1,
  e_enum        ENUM('alpha','beta'),
  s_set         SET('x','y'),
  j_json        JSON,
  b_bit         BIT(8),
  vb_varbinary  VARBINARY(32),
  bl_blob       BLOB
) ENGINE=InnoDB;

INSERT INTO awkward_types VALUES (
  1, 'héllo wörld', 'héllo latin1', 'héllo bincoll', 'ábc', 'latin1 text é',
  'alpha', 'x,y', '{"k": "v"}', b'10101010', 0x00FF10, 0xDEADBEEF
);
