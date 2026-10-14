# Stage 23 — Run a folder of SQL files, and keep every row

**Status:** 📋 Planned — 2026-10-14. UX settled with four decisions (§3.2); six
facts to measure before building (§9).

---

## 1. The use case, in the user's words

> *"I have a few SQL on files. I need a way to export from file. This feature
> works as we can select a folder and we will execute all scripts that they
> have, then the results will be automatically exported as a csv with all
> rows."*

Three things in that sentence do the work. **A folder**, not a file — the unit
is the batch. **Execute all scripts** — including whatever is in them, which is
why §4.1 exists. **All rows** — not what fits on screen, so this is the
unbounded path, not the grid's.

## 2. What already exists

More than for any stage so far. Worth listing precisely, because the shape of
this feature is mostly *which* of these it assembles.

| Piece | What it gives |
|---|---|
| `split::split` | Statement boundaries, the single source of truth. Knows strings, identifiers, comments, and reports `delimiter_detected` |
| `exec::classify` / `returns_rows` | What a statement *is* — `Select`, `RowReturning`, `Modify`, `Session`, `Other` |
| `engine::refusal` | Why this engine or this connection will not run a statement, **checked before it is sent** |
| `exec::stream_to_file` | One row-returning statement, streamed unbounded to a file, `.part`-then-rename. Not capped by `MAX_ROWS` |
| `export::csv_header` / `csv_row` / `CsvOptions` | The CSV renderer, already used by the unbounded re-run |
| `dumprun` | The job shape: its own connection, `Progress` over a channel, cancel by request id, a `Sink` with a `Drop` guard |
| `jobShow` / `jobHide` | The job strip above the results pane, with **Stop**, generalised out of the migrations view in Stage 22 |
| `export_destination` | Stage 22's pick-then-confirm-then-run split, so the overwrite question is ours and happens before any work |
| `files::read_file` | Encoding and line-ending detection, and an honest `encoding: utf-8-lossy` when the bytes are not text |
| Settings → Export | Where the CSV defaults belong, so they are not asked on every run |

**The one piece that does not fit as it stands** is `stream_to_file`: it takes a
`tab_id`, locks that tab's connection and flips its `running` flag. See §6.1.

## 3. Where it lives

A **job**, not a tab — the same argument Stage 22 §6.1 made and this inherits.
A folder of scripts over a million-row table runs for minutes; holding
somebody's editor hostage for that is wrong, and the CSV streaming wants one
connection for its whole run anyway.

So: its own connection from the pool, the job strip with Stop, and the report
in the results surface. The app stays usable while it runs.

**Entry points.** The rail's connection menu, and the database node's context
menu — the second one because of §3.1.

### 3.1 Which database it runs against is part of the job

A script full of unqualified names means a different thing in two schemas, and
this feature runs scripts somebody may not have read. So the database is
**stated in the dialog**, not inherited invisibly: opened from a database node
it is that node, from the rail it is the connection's current database, and the
job issues one `USE` before the first file.

A `USE` *inside* a script changes it from there on — that is MySQL, not
something to fight — so the report names the database each file actually ran
against. §9.5 measures that it behaves as expected before the report is written
to claim it.

### 3.2 The four decisions

Settled with the user on 2026-10-14, before any code:

1. **Writes run, and are confirmed first.** Taking "execute all scripts"
   literally, with the dialog counting and naming the writing statements before
   Run. See §4.1.
2. **The destination is a second folder, picked separately.** Not a subfolder of
   the source: the scripts may live somewhere shared or read-only, and inputs
   never mix with outputs.
3. **A failure stops the run.** Nothing after it is attempted. See §4.3.
4. **One CSV per result set** — `name.csv`, `name-2.csv`, `name-3.csv`. See
   §4.4.

## 4. The dialog

Four things in order, each describing what the one before it produced.

1. **The source folder.** Picked first, because everything below is a statement
   about what is in it. *Include subfolders* is a checkbox, off by default.
2. **What was found.** The preview, and it is the feature — see §4.1.
3. **The destination folder.** Picked separately (§3.2).
4. **CSV options** — `CsvOptions` unchanged, defaulted from Settings → Export.
   Delimiter, header row, NULL spelling, CRLF, BOM, formula defusing.

Then **Run**, which goes through the write confirmation (§4.1) and the
overwrite confirmation (§4.5) before anything is read or written.

### 4.1 The preview is the feature, and it names the writes

"Execute all scripts in this folder" on a folder somebody else assembled is a
loaded gun. So before Run is pressed the dialog already says, per file and in
total:

```
7 files · 23 statements · 14 CSVs · 9 write
```

and lists every file with its counts, including the ones that will do nothing:

* **Nothing to run** — a file of only comments. Zero statements is a fact worth
  showing, not a file to drop silently (§9.2).
* **Cannot be split** — a `DELIMITER` block. §7 says what happens to those.
* **Cannot be read as text** — `files::read_file` reports `utf-8-lossy`, and a
  script we would have to guess at is not one to run.

**The confirmation names the statements, not the count.** `9 statements will
write` is not something anybody can check — the same objection Stage 20 §18.1
raised about Flyway's confirmation and Stage 22 §9b.4 about the overwrite one.
So it lists them: file, line, and the statement's first line of text, up to a
dozen, then *and N more*. That is what makes it a confirmation rather than a
speed bump.

This is why the scan happens **in Rust, at dialog time** (§6.2): the counts and
the statement list come from the same `split` and `classify` the run will use,
so the confirmation cannot describe a different plan from the one that executes.

### 4.2 Order, stated rather than assumed

Files **alphabetically by path**, which is why people name them `01-setup.sql`,
`02-report.sql`. Statements in file order. Both said in the dialog, because a
batch runner whose order you have to guess is one you cannot use for anything
that depends on order.

### 4.3 No wrapping transaction, and the dialog says so

The tempting promise is one transaction around the whole folder, so a failure
rolls everything back. It cannot be kept: MySQL commits implicitly on DDL, so a
folder containing one `CREATE TABLE` is not atomic and never can be. A promise
that breaks on the first `CREATE` is worse than no promise.

What is promised instead is exactly what stop-on-error gives: **nothing after
the failure runs**, and the report names the file, the statement number, the
line and the server's own message. What the failed script already did is the
script's business, and the report says which file it was so somebody can look.

### 4.4 Naming, and the collision nobody thinks about

`<stem>.csv` for a file's first result, then `<stem>-2.csv`, `<stem>-3.csv`.
Deliberately no `-1` on the first: a folder of one-SELECT files — the common
case — should produce clean names.

With *include subfolders* on, the destination **mirrors the source's folder
structure** rather than flattening it. Two files called `report.sql` in
different directories are a normal thing in a repository of scripts, and
flattening them into one destination is a silent overwrite.

### 4.5 Existing files are named before anything is written

The promise Stage 22 §5 made and §9b.4 kept, applied here: the destination is
checked for the names this run would write, and they are **named** in a
confirmation. `dumprun::existing` is the right shape and the wrong signature —
it takes `&[DbPick]`. It gets generalised to take planned names, so there is one
implementation of "which of the files we are about to write are already there"
rather than two that can disagree.

## 5. The run

1. Its own connection — `ServerConn::open_private`, built in Stage 22.
2. One `USE` for the database the dialog named (§3.1).
3. Per file, in order: read through `files::read_file`, split, then per
   statement:
   * `engine::refusal` **first**, so a read-only connection refuses a write
     before it is sent rather than relaying the server's complaint;
   * row-returning → stream to its CSV, unbounded;
   * anything else → execute, and record the affected-row count for the report.
4. `Progress` carries file *i* of *n*, statement *j* of *k*, and rows and bytes
   for the statement in flight — real progress, because we are the ones doing
   it, exactly as Stage 22 §6.3 argued for the strip.
5. Cancel is polled between statements and between rows, so Stop lands inside a
   big result rather than after it.
6. Every CSV is written `.part` and renamed, under a `Drop` guard. A cancelled
   run must not leave a half-written CSV wearing the real name — somebody would
   open it in a spreadsheet and believe it.

## 6. The pieces that are not reuse

### 6.1 A streamer that is not a tab's

`exec::stream_to_file` is the right loop in the wrong place: it locks a tab's
connection, flips that tab's `running` flag, and takes cancellation from the
tab's atomics. A job has its own connection and expresses cancellation as a
closure, the way `dumprun::run` does.

So the row loop comes **out** rather than being copied — one function taking a
connection, a sink and a `cancelled: impl Fn() -> bool`, with `stream_to_file`
becoming the tab-flavoured caller of it. Stage 22 §9b.6 made exactly this move
with `export::row_literal`, for the same reason: two copies of a row loop will
disagree about cancellation or about the `.part` file, and the disagreement will
be found by a user.

### 6.2 A scan, which is a plan over files

`dumprun::Plan` resolves a selection against a server inside a snapshot. Here
the plan is *these files, these statements, these output names*, it needs no
server at all, and it must exist **before** the confirmation so the
confirmation can name things. That makes it a separate command —
`sqlfiles_scan` — and a pure function underneath it.

Pure matters: every claim in §4.1 is then a unit test, and the one thing that
cannot be tested that way (does the folder enumeration see what is actually on
disk) is small enough to see.

### 6.3 Byte offsets are not line numbers

`split::StatementSpan` carries byte offsets. The confirmation wants
`report.sql:42` and so does every error message. A helper in `split` turning an
offset into a 1-based line is new, and §9.6 measures it against multi-byte
characters, because "count the newlines" is right and "divide by something" is
the bug.

## 7. What is out of scope

* **`DELIMITER` files are listed and skipped.** `split` returns the whole buffer
  as one span once it sees a `DELIMITER` directive, by design — the terminator
  is user-defined and per-statement handling is meaningless. Such a file is
  almost always a routine definition, which returns no rows and so has no CSV
  to write. It is named in the preview with that reason rather than half-run.
* **Variable substitution** — no `${env}` or `:param` in scripts. A batch runner
  that rewrites SQL is a different, larger feature.
* **Watching a folder, or scheduling.** This runs when somebody presses Run.
* **Non-`.sql` files**, which are not read at all.
* **Rolling anything back.** See §4.3.

## 8. How it will be proved

* **Unit, Rust** — the scan: statement counts, which statements write, the
  output names including the `-2` suffix and the subfolder mirroring, a file of
  only comments, a `DELIMITER` file, and the planned-overwrite check. Pure input
  to pure output.
* **Live, against the fixture** — a folder in `dev/` holding the cases on
  purpose: one plain `SELECT`, one with three `SELECT`s, one that writes, one
  `DELIMITER` block, one with no trailing semicolon, one that is only comments.
  Run end to end against `poc`, and the CSVs compared against what the same
  queries return. **And the stop-on-error promise**: a folder whose second file
  fails must leave the third one's CSV absent.
* **Live, the dialog's own command** — `sqlfiles_scan` gets a live test even
  though it needs no server, and `engine::refusal`'s path gets one that does.
  Stage 22 §9b.7 is the reason: the export dialog's catalogue query had never
  run against MySQL, because the UI suite stubs `invoke` and the live suite went
  through a different path. The dialog's backend calls are the gap in the
  middle, and they are covered deliberately now rather than by a user.
* **UI, stubbed backend** — the preview table, the write confirmation naming
  statements, the overwrite confirmation, Stop mid-run, and the report.

## 9. The six facts to measure first

Stage 22 measured six and two of them changed the design, which is the reason
this section exists before any code.

1. **`split` on real files.** No trailing semicolon, a UTF-8 BOM, CRLF line
   endings, a trailing comment after the last statement. Each of these is
   ordinary in a file and absent from an editor buffer, and each could make the
   statement count in the preview wrong.
2. **A file of only comments.** Zero statements, or one empty span? The preview
   has to say "nothing to run" rather than produce a zero-byte CSV.
3. **Streaming, then the next statement, on one connection.** sqlx holds the
   connection for the life of a `fetch` stream. Proving that the next statement
   runs cleanly after a stream is dropped — and after a stream is *abandoned*
   mid-way by a cancel — decides whether the job needs one connection or one per
   statement.
4. **CSV throughput** on the million-row `wide` table, so the progress line's
   update rate and any buffering are chosen rather than guessed. Stage 22 §9.5
   found batch size mattered 250× on load; the equivalent here is unknown.
5. **A `USE` inside a script.** Does it persist to the next file on the same
   connection? If it does, the report must name the database per file, or a
   later script silently runs somewhere else.
6. **Byte offset to line number** with multi-byte characters above the
   statement, which is where an offset-based line number goes wrong.

## 10. Engines

Gated by a capability, not an engine name — the rule `engine.rs` states for
everything else and Stage 22 §10 followed. The gate is real here rather than
cosmetic: `split` is a **MySQL** statement splitter and `DELIMITER` is a MySQL
*client* directive, so a folder of scripts means nothing to the Elasticsearch
engine. MySQL sets the capability; Elasticsearch does not, and the menu item is
disabled there rather than behaving differently per row.
