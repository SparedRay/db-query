A folder of scripts for Stage 23's batch runner, holding the cases on purpose.
Not SQL, so the runner ignores it — which is itself one of the cases.

| File | The case it is |
|---|---|
| `01-setup.sql` | Writes. Creates and fills a table, so the confirmation has something to name and the later files have something to read |
| `02-orders.sql` | One plain `SELECT`, the common case: one file, one CSV, clean name |
| `03-report.sql` | Three result sets in one file → `03-report.csv`, `-2`, `-3` |
| `04-proc.sql` | A `DELIMITER` block, which cannot be split per statement |
| `05-notes.sql` | Only comments: nothing to run, and still listed |
| `06-no-semicolon.sql` | No trailing semicolon, which a file has and an editor buffer usually does not |
| `07-bom.sql` | Saved with a UTF-8 BOM, the way Notepad does. Fails with MySQL error 1064 unless the reader strips it |
| `daily/08-nested.sql` | In a subfolder, so the output path mirrors rather than flattens |
| `zz-teardown.sql` | Drops what `01` made, so the fixture is re-runnable |

**Why `zz-` and not `99-`.** Files run in order of their **path**, so
`daily/08-nested.sql` sorts after every top-level file — `d` is greater than
`9`. With the teardown called `99-` it dropped the table before the nested
script read from it, and the nested script failed. That is the runner behaving
correctly and a numbering scheme spanning folders being wrong, which is worth
knowing before you number your own.
