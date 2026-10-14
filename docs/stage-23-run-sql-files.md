# Stage 23 — Run a folder of SQL files, and keep every row

**Status:** ✅ Built — 2026-10-14. Seven UX decisions (§3.5), eight facts
measured first (§9) with three of them changing the design, and a hands-on pass
that found one bug (§9b.5). Proved by unit tests on the scan, five live tests
on the runner against `dev/scripts`, and 20 UI tests on the dialog and report.

The first draft of this plan described the architecture — a job, its own
connection — and called that the UX. It was not: *how do you reach it, what
does it look like while it runs, and how do the results arrive* were all
answered by pointing at Stage 22. §3 is the answer to those three questions.

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

## 3. The surfaces

Underneath, it is a **job, not a tab** — the same argument Stage 22 §6.1 made
and this inherits. A folder of scripts over a million-row table runs for
minutes; holding somebody's editor hostage for that is wrong, and the CSV
streaming wants one connection for its whole run anyway.

That is the architecture. What follows is the part a user sees.

### 3.1 Reaching it: a rail button

The rail has four tool buttons — assistant, history, schema, settings. This
becomes the fifth, because it is a **task you come back to**, not a property of
a connection you happen to be pointing at:

```
┌──┬──────────────┬──────────────────────┐
│🔵│ poc          │  editor              │
│🔵│  ▸ tables    │                      │
│  │  ▸ views     │                      │
│──┤              │                      │
│🤖│              │  assistant   Ctrl+K  │
│🕐│              │  history     Ctrl+H  │
│🗂│              │  schema              │
│▶▤│              │  run SQL files  ←    │
│⚙ │              │  settings            │
└──┴──────────────┴──────────────────────┘
```

**`Ctrl+Shift+B`** opens it. *Corrected while building:* this section said
`Ctrl+B`, which has toggled the schema pane since Stage 12 — so it takes the
modifier rather than the letter. Not `Ctrl+Shift+R`, the other obvious
mnemonic: WebKitGTK claims it as reload, and a shortcut that reloads the app
under `mise run dev` and works when packaged is the worst kind of difference.

**The button is never disabled, however bad the news.** Stage 20 §11.1 settled
that for the assistant: opening the panel is how you read *why* it is not
ready, so taking the click away hides the explanation behind the thing needing
explanation. With no connection open the dialog opens and says so.

Because the rail supplies no connection, the dialog asks for one — a connection
and a database select at the top, defaulted to the active connection and its
current database. That is the one thing the rail entry costs, and §3.4 is why
it is worth stating rather than inheriting invisibly.

It is also offered on the **database node's context menu**, where it arrives
with both already answered.

### 3.2 While it runs: the strip, and nothing else

The dialog **closes** when Run is pressed. A job does not live in a modal —
that was Stage 21's whole lesson about the assistant, and a dump of forty
tables behind a modal is the same mistake with a different noun.

What is left on screen is the job strip, in the band above the result tabs
where Flyway's has been since Stage 20:

```
┌────────────────────────────────────────────────────┐
│ ◐  Running 3 of 7 · 02-orders.sql          [Stop] │
│    statement 2 of 4 · 1,284,000 rows · 42.1 MB    │
│    Running for 48s.                               │
└────────────────────────────────────────────────────┘
│ [query.sql] [report.sql]                          │
│  …your grid, your tabs, your editor, all live     │
```

**"Without interrupting other things" is a guarantee, not a hope**, and it has
a mechanism: the job holds **its own connection**, so a tab's queries keep
running while it works. Three things follow, and each is a test rather than a
claim (§8):

* the strip lives outside the tab system, so switching tabs does not disturb
  it and it does not disturb the tabs;
* the editor takes typing, and **a query runs and returns, while the job is
  mid-stream** — that is the own-connection claim, and only a live test can
  prove it;
* Stop lands inside a result rather than after it, because cancellation is
  polled between rows.

**One job at a time**, across kinds: there is one strip, and two jobs reporting
into it would make both illegible. A second Run — or a schema export started
while this is running — finds the button disabled with the reason on it, rather
than a dialog that opens and then refuses.

**Quitting mid-run is a gap this stage closes.** `files.confirmQuit` asks about
unsaved buffers today and says nothing about a running job, so a dump or a
batch run dies silently on quit. The confirmation gains a line naming it. That
fixes it for the schema export too, which has the same hole.

### 3.3 When it finishes: a report that outlives the tab

**The report is the deliverable**, which is exactly why today's export report
is the wrong model: it goes through `results.setMessage` into `#grid`, and
`onActivate` repaints the grid per tab — so switching tabs or running one query
destroys it. Stage 20 §19 had already learned this for Flyway (*"an outcome
left only there is lost the moment another tab paints over it"*) and solved it
with a pane; this does the same.

When the run ends the strip is replaced in place by `#job-report`:

```
┌─────────────────────────────────────────────────────┐
│ SQL files · 7 files · 14 CSVs · 2,412,118 rows      │
│ stopped at 05-bad.sql:12                            │
│ /home/me/out     [Copy path] [Copy report] [Hide]   │
├─────────────────────────────────────────────────────┤
│ ✓ 01-setup.sql     2 statements · wrote 3 rows      │
│ ✓ 02-orders.sql    orders.csv                       │
│                    1,284,000 rows · 42.1 MB         │
│ ✓ 03-report.sql    report.csv                       │
│                    report-2.csv                     │
│                    report-3.csv                     │
│ ⊘ 04-proc.sql      DELIMITER — skipped              │
│ ✗ 05-bad.sql:12    Unknown column 'x' in 'field …'  │
│ – 06-later.sql     not run (stopped)                │
│ – 07-last.sql      not run (stopped)                │
└─────────────────────────────────────────────────────┘
```

Five properties, each deliberate:

1. **It is a band, not a replacement.** The grid keeps its place and is pushed
   down, so a report does not hide the query you ran next. `#flyway-pane`
   replaces the grid because a migrations tab has no result set; this is not
   that case. The band scrolls at a max height, so forty files do not eat the
   pane.
2. **It survives**, because it is outside the tab system — the whole point.
   Switch tabs, run queries, come back; it is still there.
3. **One line per file, in the order they ran** (§4.2), with a glyph for each
   of the four outcomes: ran, skipped-with-reason, failed, and **not run**. That
   last one is the honest consequence of stop-on-error (§4.3) and the easiest
   to leave out: a report that lists five successes and one failure, saying
   nothing of the two files it never reached, is a report that lies by omission.
4. **Dismissed with Hide**, or replaced by the next run. Not kept across
   restarts: it describes files on disk, and the files are the durable part.
5. **Both Copy buttons go through `wireCopyButton`**, so each says what
   happened — Stage 21's rule, earned by a Copy that failed silently.

`Copy path` is there because **the folder is the real result and the app cannot
open it.** Only `tauri-plugin-dialog` and `tauri-plugin-updater` are in the
tree; `tauri-plugin-opener` would be a new dependency, a licence to read from
source and a capability entry, and it was declined for now on purpose rather
than smuggled in for one button. If it is ever added, `Open folder` belongs
here and `Copy path` can stay.

**Nothing pops up, and no OS notification is sent.** A run that finishes while
the window is in the background leaves the report waiting; nothing is lost by
not looking. `tauri-plugin-notification` is the same trade as the opener — a
dependency and a first-run permission prompt — and was declined for the same
reason.

### 3.4 Which database it runs against is part of the job

A script full of unqualified names means a different thing in two schemas, and
this feature runs scripts somebody may not have read. So the database is
**stated in the dialog** (§3.1), not inherited invisibly, and the job issues one
`USE` before the first file.

A `USE` *inside* a script changes it from there on — that is MySQL, not
something to fight — so the report names the database each file actually ran
against. §9.5 measures that before the report is written to claim it.

### 3.5 The seven decisions

Settled with the user on 2026-10-14, before any code. The first four are about
what it does, the last three about how it is seen — and the second set took a
second pass, because the first draft of this plan answered them by pointing at
Stage 22 instead of designing them.

1. **Writes run, and are confirmed first.** Taking "execute all scripts"
   literally, with the dialog counting and naming the writing statements before
   Run. See §4.1.
2. **The destination is a second folder, picked separately.** Not a subfolder of
   the source: the scripts may live somewhere shared or read-only, and inputs
   never mix with outputs.
3. **A failure stops the run.** Nothing after it is attempted. See §4.3.
4. **One CSV per result set** — `name.csv`, `name-2.csv`, `name-3.csv`. See
   §4.4.
5. **A rail button and `Ctrl+Shift+B`**, not only a context menu. See §3.1.
6. **A report pane in the results band**, not text in the grid, because the
   report is what you ran the job for. See §3.3.
7. **In-app only** — no OS notification, and no new plugin for it. See §3.3.

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

**And the statement it shows starts at the SQL, not at the comment above it.**
§9.1 measured that a span owns its leading comment, so a confirmation built from
`span.start` would list `-- rebuild the totals` where it means to list the
`DELETE` underneath. §6.3.

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
2. One `USE` for the database the dialog named (§3.1). The job **tracks** which
   database it is in rather than asking the server: §9.5 measured a cached
   prepared `SELECT DATABASE()` replaying a stale answer with no error, which is
   exactly how a report would confidently name the wrong database.
3. Per file, in order: read through `files::read_file`, split, then per
   statement:
   * `engine::refusal` **first**, so a read-only connection refuses a write
     before it is sent rather than relaying the server's complaint;
   * row-returning → stream to its CSV, unbounded;
   * anything else → execute, and record the affected-row count for the report.
4. `Progress` carries file *i* of *n*, statement *j* of *k*, and rows and bytes
   for the statement in flight — real progress, because we are the ones doing
   it, exactly as Stage 22 §6.3 argued for the strip. Reported **every 25,000
   rows**, which §9.4 measured as about four a second, and the CSV is buffered
   1,000 rows at a time because past that there is nothing left to win.
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

### 6.3 A span starts at the comment, so three things need the first code byte

`split::StatementSpan` carries byte offsets, and §9.1 found that a span **owns
the comment above it** — `-- header` then `SELECT 1` is one span starting at the
dash. Three things the user reads are wrong if they use `span.start`:

* the write confirmation (§4.1), which would show `-- header` where it means to
  show the `INSERT` it is asking permission for;
* the line in `report.sql:42`, which would name the comment's line;
* the preview's "nothing to run", for a span that is *only* comment.

So `split` gains two small functions: the 1-based line of a byte offset, and the
first code byte of a span — the latter built on `mask_noncode`, which already
knows where code is. A second opinion about MySQL comment syntax would be a
second thing to get wrong. §9.6 measures both.

### 6.4 Stripping a BOM, in the one place both readers share

§9.7 measured MySQL refusing a statement with a leading BOM, and
`files::read_file` does not strip one. That makes it a **latent bug in the
editor** as well: open a BOM'd `.sql` today and running its first statement
fails with a syntax error pointing at an invisible character.

`files::read_file` is where it is fixed, because the batch runner and the editor
both read through it. The `OpenedFile` it returns already records `encoding` and
`lineEnding` honestly, so "this file had a BOM" belongs there too rather than
being silently undone — a Save that dropped a BOM the file came with would be
editing bytes nobody asked us to edit.

### 6.5 A band that outlives the tabs

`#flyway-pane` is the closest thing and the wrong shape: it *replaces* the grid,
which is right for a migrations tab that has no result set and wrong for a
report you want to read beside the query you ran next. `#job-report` is a band
in the strip's slot — one that pushes the grid down, scrolls at a max height,
and is owned by nothing in the tab system.

The honest cost: it is a third thing that can occupy that band, after
`#mig-progress` and `#job-strip`. Two of those three are already one element
reused (Stage 22), and this is the first that must coexist with the strip
rather than replace it, because a run ends by turning one into the other.

### 6.6 A quit that knows a job is running

`files.confirmQuit` asks about unsaved buffers. Nothing asks about a job, so a
schema export or a batch run dies silently on quit today. One predicate — *is a
job running, and which* — fixes both, and it belongs next to the one-job-at-a-
time rule in §3.2 rather than being invented twice.

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
  statements, the overwrite confirmation, Stop mid-run, and the report. Four of
  these are §3's claims and have been asserted nowhere before:
  * the report **survives**: finish a run, switch tabs, run a query, and it is
    still on screen. The export's report fails this today, which is why §3.3
    exists;
  * the report lists **not run** files after a stop, which is the row a
    report-by-omission would leave out;
  * a second job finds Run **disabled with the reason on it**, rather than a
    dialog that opens and then refuses;
  * the rail button **opens with no connection** and says so, rather than being
    disabled (Stage 20 §11.1).
* **Live, the non-interruption guarantee** — the one claim in §3.2 that a
  stubbed `invoke` cannot touch. Start a run streaming a large table, and on
  another tab run a query and get its rows back while the job is still mid-
  stream. Without its own connection this blocks, and the test says so.

## 9. The facts, measured — 2026-10-14

Stage 22 measured six and two of them moved the design, which is why this came
before the code. Eight here, and **three change something**. All of them live in
`src-tauri/tests/measure_sqlfiles.rs`, three offline and five `#[ignore]`d:

```bash
cargo test --test measure_sqlfiles                       # §9.1, 9.2, 9.6
mise run db-up && mise run export-probe-up
cargo test --test measure_sqlfiles -- --ignored --nocapture --test-threads=1
```

### 9.1 `split` on real files — two findings ⚠️ design changed

A file is not an editor buffer. Measured:

| Shape | Statements |
|---|---|
| two plain statements | 2 ✅ |
| no trailing semicolon | 2 ✅ |
| CRLF | 2 ✅ |
| `;;` between statements | 2 ✅ (no empty span) |
| trailing comment after the last statement | 2 ✅ |
| **UTF-8 BOM** | 2, and the BOM is **inside the first statement** ⚠️ |
| **leading comment** | 1, and the span is `-- header` + newline + `SELECT 1` ⚠️ |
| block comment between two statements | 2, attached to the **following** one |

**A span owns the comment above it.** That is right for execution and wrong for
everything the user reads: the write confirmation (§4.1) would show
`-- header` where it means to show the `INSERT`, and the line number would point
at the comment. So the preview and every message use the **first code byte** of
a span, not its start — see §9.6.

**The BOM is a bug, not a wrinkle** — see §9.7.

### 9.2 Nothing to run is cleanly nothing ✅

Six shapes — a line comment, a block comment, empty, whitespace, a lone `;`, a
comment then `;` — all give **zero** statements, never an empty span. So the
preview's "nothing to run" row is a count of zero rather than a special case,
and the runner can never send an empty string to the server.

### 9.3 A connection survives an abandoned stream ✅

The fact that decides one connection or many. On one connection:

* a stream read to the end, then another statement — works;
* a stream **dropped after 5 of 7500 rows**, then another statement — works;
* and again after that — works.

So §5 keeps one connection for the whole job, and **Stop can land mid-result**
without poisoning what follows. If this had failed, the job would have needed a
connection per statement.

### 9.4 CSV throughput: 64k rows/s, and the flush size barely matters ⚠️ design changed

Streaming `export_probe.wide` (1,000,000 rows, 121 MB of CSV) through the real
path — stream, `export::csv_row`, write:

| Write every | Time | Rows/s | MB/s |
|---|---|---|---|
| 1 row | 18.43s | 54,272 | 6.6 |
| 1,000 rows | 15.53s | 64,410 | 7.8 |
| 100,000 rows | 15.57s | 64,214 | 7.8 |

Two things follow. **Buffer 1,000 rows and stop thinking about it** — past that
there is nothing left to win, and the gap from 1 to 1,000 is 19%, not the 250×
Stage 22 §9.5 found for `INSERT` batching. Writing every row is *fine*, which
was not obvious.

**And the renderer is the bottleneck, not the server or the disk.** Stage 22
measured the same table reading at ~317k rows/s with no rendering; adding CSV
takes it to 64k. That is a 5× cost in `csv_row`, which allocates per field. Not
fixed here — this feature does not need it — but it is where any future speed
comes from, and the progress line should not pretend otherwise.

**The progress tick follows from it.** At 64k rows/s a 250ms tick is one per
16,000 rows, so progress reports every 25,000 rows rather than per row or per
batch.

### 9.5 `USE` persists — and the first measurement of it lied ⚠️ design changed

The fact §3.4 rests on: the job runs every file on one connection, so a `USE`
inside one script changes where later files run.

**It does persist.** Both `Executor::execute("USE …")` and
`sqlx::raw_sql("USE …")` change the session database, and it survives into the
next statement.

**The first attempt said the opposite**, and the measurement was wrong, not the
server. It asked `SELECT DATABASE()` through `sqlx::query`, which prepares; sqlx
caches prepared statements per connection by SQL text; MySQL resolves
`DATABASE()` at **prepare** time. Prepared before the `USE`, the cached
statement kept replaying the old answer:

```
prepared before USE:        Some("poc")
same prepared after USE:    Some("poc")      <- stale, and no error
text protocol after USE:    Some("export_probe")
```

Then a second wrong theory — that `execute` could not change the database and
`raw_sql` could — survived until both were measured on connections that had
never prepared the question. Neither theory was right. The instrument was the
problem, twice.

**What this changes:** §3.4 promises the report names the database each file ran
against, and it **must not ask the server through the ordinary path** to find
out. The job tracks it — which is what `session.rs` already does with
`tab.current_db` — or reads it over the text protocol. A stale answer here would
mean a report that confidently names the wrong database.

The measurement now tests the **consequence** instead: `t` exists in
`export_probe` and not in `poc`, so an unqualified `FROM t` is a question only
the session's real database can answer.

### 9.6 Byte offsets, line numbers, and the first code byte ✅

Counting newline **bytes** in the prefix is the right arithmetic — a multi-byte
character contains no `0x0A` — and spans always land on character boundaries, so
slicing to one cannot panic on an accented comment.

The useful part is §9.1's consequence. `split::mask_noncode` already knows where
code is, so the first code byte is *the first non-whitespace byte of the masked
span* — no second opinion about comment syntax, which would be a second thing to
get wrong:

```
span at   0 (line 1) · code at  65 (line 2) · shows "SELECT 1"
span at  75 (line 3) · code at  90 (line 4) · shows "SELECT 2"
```

It returns `None` for a span that is all comment, which is the same answer §9.2
needs.

### 9.7 A BOM is refused by the server ⚠️ design changed

§9.1 left the BOM inside the first statement and `files::read_file` does not
strip one. MySQL's verdict:

```
plain                  -> ok
leading line comment   -> ok
leading block comment  -> ok
leading BOM            -> REFUSED: 1064 … You have an error in your SQL syntax
BOM then comment       -> REFUSED: 1064 … near the invisible character
```

So **the reader strips a leading BOM**, and it is not optional. A `.sql` file
saved by Notepad is exactly where a folder of hand-written scripts comes from,
and without this the first file fails with a syntax error pointing at a
character nobody can see.

This is **a latent bug in the editor too**: open a BOM'd `.sql` file today and
running its first statement fails the same way. Stripping it in
`files::read_file` fixes both, and is the right place precisely because both
paths read through it.

## 9b. Built so far

### 9b.1 The foundation: a BOM off the text, and two ways to read a span — 2026-10-14

The three findings from §9 that could be acted on before any new surface
exists, each of which fixes something that was already wrong.

**The BOM comes off the text and stays on the file.** `files::read_file` strips
a leading U+FEFF and records `bom` on `OpenedFile`; `save_file` takes it and
puts it back. Symmetrical with `line_ending`, which has worked this way since
Stage 1, and symmetrical for the same reason: the editor wants normalised text
and the file's owner wants their bytes. Writing the file back without its BOM
would be a second wrong — a diff with a line in it that nobody changed.

It is **recorded, not inferred**, and it rides on the tab through the session
file, so a restored tab still saves correctly. On restore it is taken from the
file as it is now rather than from the session file, because whether a file has
a BOM is a fact about the bytes on disk.

This fixes a **latent editor bug that predates this stage**: open a BOM'd `.sql`
today, press Ctrl+Enter on the first statement, and MySQL answers 1064 pointing
at a character nobody can see. Two UI tests pin the marker's round trip through
the tab, and the Rust tests cover the file: a BOM survives, a plain file never
gains one, a BOM *and* CRLF both survive together, and only a **leading** U+FEFF
counts — one in the middle of a file is a zero-width no-break space, which is
content, and a reader that strips every one of them is corrupting data to tidy
an encoding marker.

**`split::line_of`** turns a byte offset into a 1-based line by counting newline
bytes, which is right because UTF-8 encodes nothing but `\n` as `0x0A`. An
offset past the end answers with the last line rather than panicking: a line
number is for a person to read, and refusing to produce one is never the more
useful failure.

**`split::first_code_byte`** answers §9.1's finding — a span owns the comment
above it — and took a correction worth keeping. Built on `mask_noncode` it
failed its own test: that mask blanks a string literal's *quotes* as well as its
contents, so `'hi'` looked like no code at all. "No code" means "nothing to
run", so the consequence would have been a **silently skipped file**, which is
the failure this project least wants to ship.

So the masker gained a fourth question — `mask_comments`, comments blanked and
everything else verbatim — from the same scanner that already answers the other
three. That is the module's own pattern (*"two different questions, so two
different masks — sharing one scanner"*) and it keeps `first_code_byte` from
holding a second opinion about where a comment ends, which would eventually
disagree with the first about something like `a--b`.

Both falsified: not stripping the BOM fails two file tests, and putting
`first_code_byte` back on `mask_noncode` fails the literal case.

**478 Rust unit tests, 483 UI on Chromium, 482 + 1 engine-skip on WebKit.**

### 9b.2 The scan: one plan, used by the confirmation and the run — 2026-10-14

`sqlfiles::scan` is pure — file contents in, a plan out — so every number the
preview shows is a unit test. The impure part is `collect` (the directory walk)
and `read_and_scan` (reading through `files::read_file`, which is what makes the
BOM come off and the size limit apply).

**The claim worth stating is that there is only one plan.** The preview, the
write confirmation and the run all read the same `Scan`, so a confirmation
cannot describe a run other than the one that happens. That is not a comment —
the offsets in it are `split`'s own, and a test slices the real fixture files
with them and gets the statements back.

**A write is exactly what a read-only connection refuses**, asserted as a
property over every `StatementKind` against `engine::refusal` rather than as a
hand-counted number. If the two ever drift, one of two bad things has happened:
the dialog promises a run that will be refused, or — far worse — it says nothing
about a write it is about to perform. `Session` is deliberately not a write:
`SET`, `USE` and `START TRANSACTION` change this connection and nothing in
anybody's data, and listing them in a confirmation about writes would teach
people to skim it.

**Output names come from a set, not from a counter.** Mirroring subfolders makes
a collision nearly impossible, and "nearly" there means silently overwriting
somebody's results, so each name is claimed and the next free one taken.
`report.sql` gives `report.csv`, `report-2.csv`, `report-3.csv` — no `-1` on the
first, because a folder of one-`SELECT` files is the common case.

Four falsifications, each failing exactly the test that claims it: counting
`Session` as a write, using `span.start` for the line, putting `-1` on the first
output, and flattening subfolders instead of mirroring.

#### The fixture is a folder

`dev/scripts` holds each case on purpose and its own README says which is which:
a `DELIMITER` block, a file of only comments, a missing trailing semicolon, a
real UTF-8 BOM, three result sets in one file, one file in a subfolder, and a
`README.md` that is not SQL and so is not picked up. `01-setup.sql` creates the
table and `99-teardown.sql` drops it, so the folder runs twice and leaves `poc`
as it was found.

`cargo test --test sqlfiles_scan` reads those files **from disk** and asserts
what the preview will show. It needs no server, which is precisely why it exists
now rather than later: Stage 22 §9b.7 shipped a dialog whose only backend call
had never run against anything real, because the UI suite stubs `invoke` and the
live suite went through another path. This is that gap, closed on purpose.

It also pins the fixture against itself — the BOM test asserts the three bytes
are really there, so a reformat cannot quietly remove the case it exists to
prove.

**514 Rust tests (496 unit, 9 scan, 9 measurement), check clean.**

### 9b.3 The runner, and the two things its live test caught — 2026-10-14

**One row loop, two surroundings.** `exec::stream_to_file` was the right loop in
the wrong place (§6.1), so the loop came out as
`exec::stream_rows_to_file(conn, …, cancelled, progress)` and
`stream_to_file` became its tab-flavoured caller — borrowing the editor's
connection and reading cancellation from the tab's atomics. What the two must
not differ about is the easy-to-get-subtly-wrong part: when the buffer flushes,
that a cancelled stream still writes what it had, and that the `.part` file
never outlives the attempt.

`sqlfilesrun::run` is shaped like `dumprun`: its own connection, `Progress`
over a channel, cancel polled between files, between statements and between
rows. Non-row statements go through **`raw_sql`, not `query`** — a script holds
`CREATE PROCEDURE`, `USE`, `SET`, `FLUSH`, and sqlx's ordinary path prepares,
which MySQL refuses for some of those.

Three unit tests carry what is pure (every skip reason is a sentence, `USE` is
recognised behind the comment its span owns, report paths are relative); five
live tests carry the rest. Two of them found things.

#### The CSV said `poc` while the report said `information_schema`

The `USE`-moves-the-next-file test failed with the report right and the **data
wrong** — which is the worse way round. §9.5's trap, biting the feature this
time: `sqlx::query` sets `persistent: true`, sqlx caches prepared statements per
connection by SQL text, and MySQL resolves `DATABASE()` when a statement is
*prepared*. Two files holding the same `SELECT DATABASE()` with a `USE` between
them got the first file's answer, with no error anywhere.

Fixed with `.persistent(false)` on the streaming path, read from the vendored
source rather than guessed: `arguments: Some(_)` with `persistent` false takes
`prepare_statement` instead of `get_or_prepare_statement`, so prepare time is
execute time, and it stays on the binary protocol so nothing about decoding
changes (`sqlx-mysql-0.9.0/src/connection/executor.rs:124-150` and
`sqlx-core-0.9.0/src/query.rs:685-689`, read 2026-10-14). The cost is one
prepare per statement, against a result set big enough to be worth streaming.

It falsifies: with the cache back on, that test fails and the other four pass —
which is exactly what makes it dangerous. **Still open, deliberately:** the
editor's own `run_script` has the same staleness in principle. Fixing it there
means a prepare per statement for every query anybody runs, which is a
measurement this stage has not made.

#### Files run in path order, so a subfolder lands last

The folder run failed at `daily/08-nested.sql` because `99-teardown.sql` had
already dropped its table: paths sort as paths, and `d` is greater than `9`. The
runner was right and the fixture's numbering was wrong — a scheme spanning
folders does not mean what it looks like.

The teardown is now `zz-teardown.sql`, the fixture README says why, and the live
test **asserts the order** rather than only depending on it, so a reader sees
what the name is for. Worth knowing before anybody numbers their own folder.

**Also built:** stop-on-error leaves the files after it reported as `NotRun` —
the row a report by omission drops; a cancelled CSV is kept and marked
**partial**, because a silently truncated CSV is one somebody opens in a
spreadsheet and believes; and a read-only connection's refusal names the
connection rather than claiming MySQL cannot write.

**503 Rust unit tests, 9 scan, 9 measurement, 5 live runner.**

### 9b.4 The UI: a rail button, a preview, a strip and a band — 2026-10-14

**The shortcut in §3.1 was wrong.** `Ctrl+B` has toggled the schema pane since
Stage 12, which the plan said was free. It is `Ctrl+Shift+B` instead — the
modifier rather than the letter — and the branch that handles it has to read
`shiftKey`, because it matches both cases of "b" and would otherwise answer
both shortcuts. A test holds `Ctrl+B` to the schema pane so this cannot drift
back.

The rail button is the fifth, with a new `run-files` icon: a page with a play
mark, not a bare play triangle, which would read as "run the editor's query"
when the button is about *files*.

**The preview lists every file, including the ones that will not run** — four
shapes, from the scan's verdicts — and marks the ones that write. The summary
ends on the number that matters and only when there is one: "0 write" reads as
reassurance nobody asked for.

**The write confirmation names statements**, `01-setup.sql:2  DROP TABLE …`, up
to a dozen. It is built from the same `Scan` that is handed back to the runner,
so a test asserts the sent plan equals the shown plan **object for object** —
the confirmation cannot have named statements from a different plan.

**The report is a band outside the tab system**, and a test switches tabs *and
runs a query* to prove it survives both. That is the claim §3.3 exists for: the
schema export's report goes through `results.setMessage` into `#grid`, and
`onActivate` repaints the grid per tab, so switching tabs destroys it. Breaking
this one back to `results.setMessage` fails six tests.

**One job at a time, and Run says which.** `jobRunning` holds a *name* rather
than a boolean, so a second attempt reads "A schema export is already running"
rather than "a job is". The schema export takes the same lock, so the two
cannot contend for the one strip.

**Quitting mid-job now asks** (§6.6), after the unsaved-work question rather
than before — losing edits is the worse of the two and should be read first.

#### Three things the tests caught

* **"Run 1 statement that write?"** The count agreed and the verb did not. Its
  own test found it, and the title is now written out per branch rather than
  pluralised by suffix.
* **Both folder pickers returning one path.** A fixture problem that exposed a
  real gap: nothing said anything when the destination *was* the source, which
  silently undoes §3.2's reason for having two. It is allowed — it is their
  folder — and now noted: *"The CSVs will be written among the scripts."*
* **`sqlfiles_run_cancel` unstubbed**, which is the harness's job and is now
  stubbed centrally so no later spec has to think about it.

Three falsifications: the report in the grid (6 tests fail), ignoring the write
confirmation's answer, and letting `Ctrl+Shift+B` fall through to the schema
toggle.

**18 UI tests for this feature; 501 on Chromium, 500 + 1 engine-skip on
WebKit, 499 Rust unit.**

### 9b.5 Found in use: a folder with no SQL in it did nothing, quietly — 2026-10-14

> *"If i select a folder with no SQL it does not run. Does not show an error it
> just does nothing when clicking on the button. Apart from that it worked
> fine."*

The hands-on pass found one bug, and its shape is the interesting part.

`drawPreview` had five branches that each decided whether Run was possible, and
each was separately responsible for saying so. The empty-folder branch
**returned early**, set the grey hint line, and never reached the `say()` its
sibling case used — so a folder of unrunnable scripts shouted into the red error
line while an empty folder whispered in grey. Pressing a disabled button then
did nothing, with nothing to read.

Writing the test found a second half nobody had reported: **the disabled button
had no `title` in any case**, so hovering it explained nothing either. Only the
"another job is running" reason ever reached it.

**The fix is one `blocker()`**, which returns the reason Run is unavailable and
whether it is news or just the next step:

* `error` — *we looked, and there is nothing here.* No `.sql` files, nothing
  runnable, another job in the way, no connection, no connection that can.
* `hint` — *this is the next step.* No folder yet, no destination yet. Not a
  failure, so not red.

Every branch now gets the error line **and** the button's title from the same
answer, so a reason cannot be added that forgets to explain itself. The two
connection-level messages moved in from `open()`, where they were said once and
then painted over by the first redraw — a quieter instance of the same bug,
found by moving the first one.

Falsified both halves: the empty folder back to a hint fails one test, and
dropping the button's title fails two.

**503 UI tests on Chromium, 502 + 1 engine-skip on WebKit.**

## 10. Engines

Gated by a capability, not an engine name — the rule `engine.rs` states for
everything else and Stage 22 §10 followed. The gate is real here rather than
cosmetic: `split` is a **MySQL** statement splitter and `DELIMITER` is a MySQL
*client* directive, so a folder of scripts means nothing to the Elasticsearch
engine. MySQL sets the capability; Elasticsearch does not, and the menu item is
disabled there rather than behaving differently per row.
