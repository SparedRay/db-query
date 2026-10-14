-- A DELIMITER block. `split` returns the whole buffer as one span by design,
-- so this file is named and skipped rather than half-run.
DELIMITER //
CREATE PROCEDURE batch_probe_noop()
BEGIN
  SELECT 1;
END //
DELIMITER ;
