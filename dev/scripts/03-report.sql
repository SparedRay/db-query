-- Three result sets in one file, so the names need the -2 and -3 suffixes.
SELECT COUNT(*) AS rows_total FROM batch_probe;
SELECT label FROM batch_probe WHERE amount > 5 ORDER BY label;
SHOW TABLES LIKE 'batch_probe';
