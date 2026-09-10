-- **Fails on purpose.** F6 needs a genuinely failed migration to repair, and a
-- schema history with a failed row in it is the only honest way to test that.
-- `widgets` has no `weight` column and never will.
ALTER TABLE widgets DROP COLUMN weight;
