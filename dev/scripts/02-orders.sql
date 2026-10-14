-- The common case: one file, one SELECT, one CSV with a clean name.
SELECT id, label, amount FROM batch_probe ORDER BY id;
