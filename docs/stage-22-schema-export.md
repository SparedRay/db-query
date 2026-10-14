# Stage 22 — Export a schema, with or without its data

**Status:** 🚧 In progress — 2026-10-14. UI agreed; the six facts in §9 are measured, and two of them changed the design.

---

## 1. The use case, in the user's words

> *"We are looking for an export feature so we can generate a script to
> recreate DB schema with and without data. This should allow us to select
> either specific dbs, tables, view or all and select if we want a single file
> or a file per schema."*

## 2. What already exists

This is not a green field, and knowing which parts are already built is what
decides where the work actually is.

| Piece | Where | Reused as-is? |
|---|---|---|
| Row-at-a-time streaming to a file, cancellable, `.part` then rename | `exec::stream_to_file` | **No** — see §6.1 |
| Batched `INSERT` from a live row | `export::insert_header`, `insert_row` | Yes |
| `CREATE TABLE` from the server | `schema::table_ddl` (`SHOW CREATE TABLE`) | Yes |
| `CREATE VIEW` from the server | the same command — MySQL answers a view with it | Yes |
| Procedures and functions, with `DELIMITER` blocks | `sqlgen::routine_script` | Yes |
| Identifier quoting, literals | `sqlgen::quote_ident`, `literal` | Yes |
| Save dialog | `pick_export_path` | Yes |
| Folder picker | `tauri-plugin-dialog`'s `pick_folder` | **Present, never called** |
| Progress over a channel | the assistant's `tauri::ipc::Channel` | Pattern |
| A strip that says what a long job is doing | `#mig-progress` (L2, Stage 20 §19) | Generalised — §6.3 |

The genuinely new work is: a selection UI, a writer that appends many objects
to one file, dependency ordering, triggers and events, and a job that is not
tied to a tab.

## 3. Where it lives

**Its own dialog**, with its own tree. Chosen over putting checkboxes in the
schema sidebar (which would put a mode on the app's main navigation, in a
260px column) and over a tab (a workspace for something configured once).

```
┌─ Export schema ─────────────────────────────────────┐
│ Filter…                              Contents       │
│ ┌────────────────────────┐   ( ) Schema only        │
│ │ ▾ ■ poc          (12)  │   (•) Schema + data      │
│ │   ▾ ■ Tables (9)       │   ( ) Data only          │
│ │     ☑ users            │                          │
│ │     ☑ orders           │   Output                 │
│ │     ☐ audit_log        │   (•) One file           │
│ │   ▸ ▣ Views (1)        │   ( ) One file per database
│ │   ▸ ☐ Routines (2)     │                          │
│ │ ▸ ☐ warehouse    (31)  │   ☑ DROP IF EXISTS first │
│ └────────────────────────┘   ☑ CREATE DATABASE      │
│ All  None  Tables only       ☑ Disable FK checks    │
│                              Rows per INSERT [1000] │
│ 11 tables, 1 view · ~2.4M rows      [Cancel][Export]│
└─────────────────────────────────────────────────────┘
```

**Opened from two places**, both of which already have the context the dialog
needs:

* a **database node** in the schema tree — right-click, *Export…*, with that
  database preselected. Database nodes have no context menu today; tables,
  columns and routines all do, so this is the one gap in a pattern rather
  than a new idea.
* a **connection** in the rail — its menu already exists; *Export schema…*
  opens with nothing preselected.

There is no new rail button. A fifth icon for something done once a month
would cost every session a permanent pixel for a rare action.

### 3.1 Selection means "resolved when it runs"

The tree lazy-loads, as the sidebar's does, and ticking a database you have
never expanded must not force a fetch of every table in it.

So a parent in the **all** state means *everything under it, enumerated at
export time*; a parent in the **some** state means *exactly these children*.
That is also the meaning people expect: tick a database, get the database.

The count in the footer is therefore an estimate for unexpanded nodes, and
says so. Row counts come from `information_schema.TABLES.TABLE_ROWS`, which
for InnoDB is an estimate from the optimiser and can be out by a wide margin —
it is labelled `~` and is never used for anything but the sentence.

## 4. What the script has to be

A dump that cannot be re-run is a text file, not a backup. So the generated
script is **re-runnable against an empty server, top to bottom, in one go**,
and everything below follows from that.

### 4.1 Order

1. The header (§4.2).
2. Per database: `CREATE DATABASE IF NOT EXISTS` … `USE`.
3. **Tables**, structure.
4. **Data**, as batched `INSERT`s.
5. **Views**, after the tables they read, and ordered among themselves.
6. **Routines**.
7. **Triggers** — after the data, deliberately. A trigger installed before the
   load fires once per inserted row and writes rows nobody asked for.
8. **Events**.
9. The footer, restoring what the header changed.

### 4.2 The header, and why each line is there

```sql
SET @OLD_FOREIGN_KEY_CHECKS = @@FOREIGN_KEY_CHECKS;
SET FOREIGN_KEY_CHECKS = 0;
SET @OLD_SQL_MODE = @@SQL_MODE;
SET SQL_MODE = 'NO_AUTO_VALUE_ON_ZERO';
SET NAMES utf8mb4;
```

* **Foreign keys off** is what lets tables be written in any order. With them
  on, a correct dump would need a topological sort of the whole schema, and a
  cycle — which MySQL permits — would make it impossible.
* **`NO_AUTO_VALUE_ON_ZERO`** is not decoration. Without it, a literal `0` in
  an `AUTO_INCREMENT` column is replaced by the next sequence value on load,
  so a row silently changes its primary key and every foreign key pointing at
  it becomes wrong.
* **`SET NAMES utf8mb4`** so the file's own bytes are interpreted as written.

Each is restored in the footer, because a script that leaves a session with
foreign-key checks off is a trap for whatever runs next in it.

### 4.3 Views need an order MySQL will not hand over

A view that selects from another view must be created second. `SHOW CREATE
VIEW` does not say what a view depends on.

A topological sort over `information_schema.VIEW_TABLE_USAGE`, which
**§9.1 confirms reports view→view edges**, not merely view→table. Joining it
back to `information_schema.TABLES` says which of the two a dependency is:

```
v1 → t   BASE TABLE
v2 → v1  VIEW
v3 → t   BASE TABLE
v3 → v2  VIEW
```

That table arrived in MySQL 8.0.13. Below that version the fallback is to emit
views in name order *and say so in the file*, because a wrong order that is
announced is recoverable and one that is not is a mystery.

### 4.3b Triggers carry an order that nothing in their text records

`SHOW CREATE TRIGGER` emits no `FOLLOWS` or `PRECEDES` clause. Two triggers
with the same timing and event on one table therefore have an order that
exists **only in the order they were created** — and
`information_schema.TRIGGERS.ACTION_ORDER` is the only place it is written
down (measured: 1 and 2 for the probe's two `BEFORE INSERT` triggers).

So triggers are emitted sorted by table, timing, event, then `ACTION_ORDER`.
Get that wrong and the dump loads perfectly while computing different values.

Two more things the measurement turned up:

* a trigger body names other tables **unqualified** (`INSERT INTO audit …`),
  so the script must `USE` the database before creating them — which §4.1
  already does, now for a reason rather than for tidiness;
* `SHOW CREATE TRIGGER` returns the `sql_mode` the trigger was created under,
  and a trigger behaves differently under a different one. The script sets it
  around each trigger and restores it, as `mysqldump` does.

### 4.4 DEFINER

`SHOW CREATE VIEW`, `SHOW CREATE TRIGGER` and the routine DDL all carry
`DEFINER=\`someone\`@\`somewhere\``, and loading that into a server where that
account does not exist fails. `sqlgen.rs` already keeps the clause as the
server reports it and says so in a comment.

An **"Omit DEFINER clauses"** option, defaulting to **on**, because the common
case is moving a schema to a different server. Off keeps the dump faithful.

### 4.5 Consistency

The export runs inside:

```sql
SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ;
START TRANSACTION WITH CONSISTENT SNAPSHOT;
```

This is what `mysqldump --single-transaction` does, and without it a dump
taken over several minutes — while the app's own tabs are running queries —
can contain a child row whose parent it already passed.

**§9.4 measured that it holds**, including across the things a dump does
between tables: `SHOW CREATE TABLE`, `SHOW CREATE VIEW`, `SHOW TRIGGERS` and
`USE` all leave the snapshot intact. `USE` was the one worth checking — an
implicit commit there would have ended the snapshot silently, half way
through.

Two honest limits, to be written into the report rather than implied away:
it is consistent only for transactional tables, and any DDL run during the
dump breaks it. `mysqldump` has the same two.

## 5. Output

* **One file** — the save dialog, defaulting to `<connection>-<date>.sql`.
* **One file per database** — the folder picker, then `<database>.sql` each.
  Existing files are counted and confirmed before anything is written; the
  save dialog does that itself in the single-file case, a folder does not.

Every file is written to `.part` and renamed at the end, which is what
`stream_to_file` already does: an export that was cancelled or crashed must
never leave a half-written file wearing the real name.

## 6. The three pieces that are not reuse

### 6.1 A job, not a tab

`stream_to_file` takes a `tab_id`, locks that tab's connection and sets its
`running` flag. That is right for "export this result set" and wrong here: a
dump of forty tables would hold somebody's editor hostage for minutes, and
§4.5 needs one connection held open across every table anyway.

So the export takes **its own connection** from the pool for its whole run.
That is the single biggest structural change, and it is also what makes the
snapshot and the non-blocking progress possible.

### 6.2 A writer that appends

Today's streamer owns a file end to end. The export needs one writer that many
producers append to — DDL, then rows, then more DDL — with the byte count
carried along for the progress line, and `.part`-then-rename kept.

### 6.3 A job strip, generalised

Stage 20 built `#mig-progress`: a spinner, what is being done, how long it has
been, and a sentence admitting what is *not* known. It lives inside the
migrations view.

Export needs the same thing, so it moves out: a strip above the results pane
that any long job can raise, with **Stop**. The export's report then renders
in the results surface, next to where Flyway's output already goes
(`showResultsSurface`).

Unlike Flyway, this job knows real progress — objects done out of objects
total, rows and bytes so far — because we are the ones doing it. The strip
says exactly that, and nothing it cannot know.

## 7. What is out of scope

Users and grants (not schema), binary-log coordinates and `--master-data`,
compression, and `LOCK TABLES` for non-transactional engines. Each is a
decision somebody should make deliberately, not get as a side effect.

## 8. How it will be proved

* **Unit, Rust** — the assembler: order, header, footer, `DROP` placement,
  DEFINER stripping, the view sort, per-database file split. Pure input to
  pure output.
* **Live, against the fixture** — the one that matters: **dump `poc`, load the
  script into a scratch database, and compare.** Same tables, same
  `SHOW CREATE TABLE` text, same row counts, same checksums. A dump that
  re-runs is the entire claim, and nothing short of running it proves it.
* **UI, stubbed backend** — tri-state, lazy children, the filter, "all"
  meaning "resolved later", the confirmation before overwriting files, Stop
  mid-run.

## 9. The six facts, measured — 2026-10-14

Written down before building, in the style of Stage 20 §2, because each one
changes the design and none was a safe guess. Measured against MySQL 8.4.11 in
the podman fixture, with `mise run export-probe-up` (`dev/export-probe.sql`)
for the cases `seed.sql` does not carry. The sqlx halves are
`src-tauri/tests/live_export_probe.rs`.

**Two of the six changed the design.**

### 9.1 `VIEW_TABLE_USAGE` — yes, and it gives view→view ✅

Present on 8.4.11 and it reports `v2 → v1` and `v3 → v2`, not only edges to
base tables; joining to `information_schema.TABLES` says which kind each
dependency is. A topological sort is straightforward and needs no fallback
above MySQL 8.0.13. §4.3 rewritten.

### 9.2 Triggers — round-trip, **and carry a hidden ordering** ⚠️ design changed

`SHOW TRIGGERS FROM db` enumerates; `SHOW CREATE TRIGGER` returns the whole
`CREATE DEFINER=… TRIGGER …` text. But it emits **no `FOLLOWS`/`PRECEDES`**,
so two triggers sharing a timing and event have an order recorded nowhere
except `information_schema.TRIGGERS.ACTION_ORDER`. New §4.3b.

### 9.3 Events — round-trip ✅

`SHOW EVENTS FROM db` enumerates, `SHOW CREATE EVENT` gives the full text with
`DEFINER` and an absolute `STARTS '2026-10-14 16:59:15'`. The scheduler is on
in the fixture. Noted: a one-shot event whose `STARTS` is in the past, with
`ON COMPLETION NOT PRESERVE`, can be dropped the moment it loads — which is
the server behaving correctly on a stale dump, and belongs in the report
rather than in a workaround.

### 9.4 The consistent snapshot — holds ✅

Another connection inserted five rows and committed; the dump connection saw
its original count across several statements and 300 ms, then saw the new one
after `COMMIT`. `SHOW CREATE TABLE`, `SHOW CREATE VIEW`, `SHOW TRIGGERS` and
`USE` all left it intact — `USE` being the one that could plausibly have
ended it with an implicit commit.

### 9.5 Batch size — **the default is 1,000, not 100** ⚠️ design changed

Reading is the same speed at every batch, ~317,000 rows/s, because the fetch
dominates. That is not the number that matters. What a batch buys is how long
the script takes to **load**, which is what somebody restoring a dump waits
for:

| rows per `INSERT` | file | longest statement | **load time** |
|---|---|---|---|
| 1 | 9.9 MB | — | **274 s** |
| 100 | 4.4 MB | — | **4.7 s** |
| 1,000 | 4.4 MB | 0.04 MB | **1.3 s** |
| 10,000 | 4.3 MB | 0.44 MB | **1.1 s** |

100,000 rows of `export_probe.wide`, loaded through the `mysql` client.

One to a hundred is **59×**. A hundred to a thousand is another **3.6×**. A
thousand to ten thousand is 1.17× and multiplies the longest statement by ten.

So: **1,000 by default**, and — because the table above is four narrow columns
and a real one has `TEXT` and `BLOB` — the writer caps a statement by **bytes
as well as rows**, flushing at 1 MB, whichever comes first. `max_allowed_packet`
is 64 MB here but belongs to the *target* server, which the dump cannot see;
1 MB is `mysqldump`'s own `--net-buffer-length` default, and it makes wide rows
safe without a setting.

> **Found on the way past:** the result-set export dialog (Stage 3) defaults to
> **100** rows per `INSERT`. On this measurement that is 3.6× slower to load
> than it needs to be, for no benefit. Not changed here — a different
> feature's default, and its own one-line change.

### 9.6 `TABLE_ROWS` — about 1% low, good enough ✅

On a million rows: estimate **990,296** against an actual **1,000,000**, and
991,953 after `ANALYZE TABLE`. The fixture's own tables are too small to be
sampled at all and report exactly.

`COUNT(*)` on the same million rows took **0.2 s**, which is tempting — but it
is a full index scan, so it is 0.2 s here and minutes on a hundred million.
The dialog keeps the estimate, one query for a whole database, and labels it
`~`.

## 10. Engines

Gated by a new `Capabilities::schema_export`, not by the engine's name — the
rule `engine.rs` already states for everything else. MySQL sets it; the
Elasticsearch engine does not, and the menu items simply do not appear there.
