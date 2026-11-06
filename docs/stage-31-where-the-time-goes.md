# Stage 31 — the server says where its time goes

**Status:** ✅ Built — 2026-11-04. Hands-on against the real server outstanding; §8's four questions still open.

Design mockup, six boards: https://claude.ai/artifact/Ho8ajLhBvkjd8nb5riUvMv

---

## 1. The ask

> *"A while back we considered the fact of having a sort of Perform dashboard. A
> way we could have insights or generate a report on how DB was performing so
> later we could use that data to suggest index and sort. Is it really possible
> knowing we will only have access to the DB connection? Or such requires
> different monitor tools?"*

And, once the probe came back:

> *"Lets plan this as our next stage. Lets start with the design, decisions and
> scope."*

## 2. What was actually true

**The idea was never written down.** It had been discussed and nothing survived
the conversation — no stage doc, no issue, no note. So there was nothing to
align against, and the first job was measurement rather than recall.

**`performance_schema` appears nowhere in the codebase.** The only thing this
app has ever asked a server about itself is `SELECT VERSION()`, at
`session.rs:661`. Everything else goes through `information_schema`.

**A read-only connection is no obstacle.** `engine.rs:351` refuses
`StatementKind::Modify | StatementKind::Other`. Reading
`performance_schema` is a `Select`, so it passes a connection the user has
ticked read-only, untouched and without an exception being carved for it. The
same gate refuses `CREATE INDEX`, which is the behaviour this stage wants.

**Three seams already exist that this stage was going to have to invent.**

* `TableRef.kind` is already `"BASE TABLE" | "VIEW"` (`schema.rs:28`). Telling a
  view from a table — which decides a whole verdict — costs **nothing**. The
  Stage 30 cache has it.
* `exec::StatementKind` already separates `Session` (`SET`, `USE`, `COMMIT`)
  and `RowReturning` (`SHOW`, `DESCRIBE`, `EXPLAIN`) from `Select` and
  `Modify`. That is most of the noise classifier, written three stages ago for
  another reason.
* `relevance::recognised` already answers "is this a word the schema has a name
  for", and it exists because naming a word the schema does not know was a
  secret-leak risk in the assistant log. The same guard is what keeps a crude
  table-name scan from reporting garbage here.

**Elasticsearch has no door.** `elastic.rs` speaks exactly one endpoint,
`POST /_sql`. There is no GET path and no `_cat`/`_stats` plumbing, so a
workload panel there is not a small addition — it is a transport change. Out of
scope, by capability rather than by engine name.

**Two databases, one server.** UAT reports shapes against both `appdata`
and `authdata`. The panel is per **connection**, not per database, and
the database is a column.

## 3. What was measured

The probe ran against the real UAT instance — Aurora MySQL on RDS, identified
by `mysql.rds_heartbeat2`, `information_schema.replica_host_status`,
`durable_lsn` and `oscar_local_only_replica_host_status` in its own digests.
Counters covered **54 days**, 11 Sep 08:32 → 4 Nov 23:59, with nothing lost
from the digest ring. QA, asked the same questions, returned **zero rows**.

Nothing was denied. The connection could read
`events_statements_summary_by_digest` and
`table_io_waits_summary_by_index_usage` with no grant.

| Shape | Execs | Total | Examined | Returned |
| --- | --- | --- | --- | --- |
| `SELECT … FROM Supplier WHERE tenant_sub_id = ? AND (NAME = ? OR …` | 40,668,477 | **79h 23m** | 108,410,167,913 | 41,708,854 |
| `SELECT c.id, c.ptype, c.v0 … FROM acl_rule c` | 3,621 | 8h 57m | 1,511,063,716 | 1,511,063,716 |
| `SELECT cc.partyId FROM partysettings cc WHERE cc.tenantId = ? …` | 78,685 | 6h 23m | **0** | 14,687 |
| `SELECT cc.partyId AS PartyId … FROM partysettings cc` | 39,318 | 1h 18m | 1,883,120,378 | 5,615 |
| `UPDATE Supplier SET NAME = ?, last_updated_by = ?` | 40,666,497 | 1h 57m | 40,903,221 | 0 |
| `UPDATE Supplier SET Line1 = ?, Line2 = ? …` | 40,666,507 | 1h 32m | 41,312,791 | 0 |
| `SELECT … FROM party_bouncing_view WHERE …` | 6,793 | 50m 33s | 1,640,137,221 | **15** |
| `SELECT cd.Id, cd.PartyId … FROM partydocuments cd` | 157,228 | 9m 51s | 918,137,315 | **18** |

Four things in that table decided the design.

**One shape is 6% of a core, continuously, for 54 days.** 108.4 billion rows
examined to return 41.7 million — 2,599 read per row. But 95.7% of its
executions *did* use an index, so one exists and is chosen; it narrows to about
2,666 rows and the rest is filtered row by row. The index is not wrong. It is
not selective enough.

**Three shapes on `Supplier` ran the same number of times.** 40,668,477 ·
40,666,497 · 40,666,507 — a spread of 1,980 across 40.7 million, 0.005%. First
seen within 140 ms of each other, last seen within 20 ms. That is one
read-then-write loop, 82h 52m of server time between the three. No single row
of the table says this; the pattern across rows does.

**`acl_rule` cannot be helped by an index.** 3,621 full scans, 417,306 rows
each, 8.89 seconds a call, examined-to-returned ratio exactly 1:1. It returns
everything it reads, which is the definition of a query an index cannot
shrink. An advisor that sorted by slowness would have proposed one here and
been confidently wrong about the second most expensive thing on the server.

**Eight of the top twenty by time are not application queries at all.** 7h 27m
of connection-pool handshakes and Aurora housekeeping. `SET NAMES` ran
**254,519,390** times in 54 days — 54 a second, a full handshake per pool
checkout. All of them have an empty `SCHEMA_NAME`, so the probe's
`SCHEMA_NAME NOT IN ('mysql', …)` filter never touched them. Letting `NULL`
through was right for correctness and useless for readability.

### 3.1 Three faults in the probe itself

Recorded because the stage must not inherit them.

* `LEFT(DIGEST_TEXT, 200)` **cut the `Supplier` predicate mid-`OR`** — the exact
  clause that decides which columns an index should carry. The panel reads the
  whole digest.
* `SUM_LOCK_TIME` and `SUM_ERRORS` were not selected, and they are what
  separates a lock wait from a timeout. Without them the
  `partysettings` shape — 6h 23m at 291 ms a call while examining
  **zero** rows — cannot be explained, only noticed.
* `LIMIT 20` makes any per-table accounting partial. The panel aggregates the
  whole ring.

## 4. Decisions

### 4.1 The digest table is the only source, and it is read whole

One query against `performance_schema.events_statements_summary_by_digest`, no
sampling, no second opinion, no agent. The table is in memory and bounded by
`performance_schema_digests_size` — 10,000 rows by default, 3,412 occupied on
UAT. Reading all of it is cheaper than the schema queries this app already runs
on connect.

*Rejected:* the slow query log. `mysql.slow_log` carries **real parameter
values** — customer data — and that is a different privacy posture needing a
different conversation. See §7.

### 4.2 A verdict, not a ranking

The panel's primary output per shape is a **verdict**: what kind of problem this
is. Sorting by slowness and offering `CREATE INDEX` is the obvious design and
the measurement above shows it is wrong.

Seven verdicts. **Four of them refuse to propose an index**, and on UAT those
four cover more total time than the three that do.

| Verdict | Rule over the digest row | Proposes |
| --- | --- | --- |
| `missing index` | `examined / sent > 100` and (`no_index > 0` or `scans > 0`) | an index |
| `no index will help` | `examined / sent <= 2` and `scans == execs` | nothing — the query wants the whole table |
| `view blocks the filter` | the target's `TableRef.kind == "VIEW"` | nothing — the view body is the problem |
| `row-by-row loop` | §4.5 | nothing — the caller holds the fix |
| `waiting, not reading` | `avg_time > 50ms` and `examined == 0` | nothing — look at `SUM_LOCK_TIME` |
| `driver chatter` | §4.4 | nothing — hidden by default |
| `platform internal` | §4.4 | nothing — always hidden |

Each rule is printed next to its verdict in the UI. A verdict that cannot be
argued with is a verdict nobody will trust, and these rules will be wrong
sometimes.

*The order matters:* `no index will help` is evaluated **before**
`missing index`, so a bulk load can never fall through into an index proposal.

### 4.3 The verdict rules are pure functions over a row struct

`workload.rs` holds a `DigestRow` and the rules as free functions taking
`&[DigestRow]`. No SQL, no `ServerConn`, no async. The whole taxonomy is unit
testable against the eight real UAT rows above, offline, in the Rust suite —
the same shape `relevance.rs` took in Stage 30 and for the same reason: the
part most likely to be wrong is the part that must be testable without a
server.

### 4.4 Noise is classified by statement kind, not by a list of names

`exec::StatementKind` already does most of it:

* `Session` (`SET NAMES`, `SET LOCAL`, `USE`) → `driver chatter`
* `RowReturning` whose verb is `SHOW` → `driver chatter`
* a `Select` whose entire select-list is `@@` variables → `driver chatter`
* anything touching `mysql.rds_*`, `information_schema.replica_host_status`,
  `rds_metrics_*` or `oscar_*` → `platform internal`

The first three are a general mechanism and will classify the next driver's
chatter without being taught it. The fourth is a list, and it is a list on
purpose — "Aurora's own housekeeping" has no structural signature, only names.
That list is the one place in this stage where a name is hardcoded, and it is
commented as such.

Default view is **Application**. Chatter and platform are one click away, never
deleted.

*But chatter is not silent.* 254.5 million `SET NAMES` is a real finding. The
Application view carries **one** line about it when the count is absurd,
because a pooling problem that costs 4h 53m deserves a sentence even on a
screen about indexes.

### 4.5 A loop is found across rows, not within one

Group the application shapes by referenced table. Within a group, any two or
more shapes where

```
|execs_a - execs_b| / max(execs) < 0.001
AND first_seen within 1s
AND at least one is a write
```

are reported as one `row-by-row loop` finding, with their combined total time.
On UAT this finds the `Supplier` triple and nothing else.

*The falsification that matters:* two unrelated shapes on one table that happen
to sit within 0.1% must **not** fire. A test asserts that, because a detector
with one real positive and no negative case has not been tested.

### 4.6 A table name is only reported when the schema has a name for it

The panel needs the referenced table to classify, to look up `TableRef.kind`,
to group for loop detection and to total write pressure. A SQL parser is out of
scope and would be the largest thing in this stage.

Instead: scan for identifiers following `FROM`, `JOIN`, `INTO` and `UPDATE`,
then **keep only the ones the Stage 30 schema cache already has a name for**,
exactly as `relevance::recognised` does. A crude scan plus a hard filter cannot
report a table that does not exist, and the cost of a scan miss is a shape
classified `unknown` rather than a fabricated finding.

A shape whose table cannot be recognised gets no verdict beyond the
ratio-based ones, and says so.

### 4.7 A proposal is priced before it is made

The same digests that find a missing index count the writes against that table.
`Supplier` takes **81,333,004** writes over the window, so every index on it is
maintained 81 million times — a two-column index is affordable there and a
six-column covering index is not. `partysettings` takes 950 and can be
indexed freely.

Every index proposal carries that number. This falls out of data already
fetched and is the half an index advisor usually skips.

### 4.8 Nothing on the panel runs DDL

Every finding ends in a statement the person copies into a tab and runs
themselves — the contract `assistant.rs` has had since Stage 10, restated
rather than re-invented. On a connection marked read-only, `engine.rs:351`
refuses the statement even then, so the proposal is still worth reading and
still cannot be acted on by accident.

MySQL has no hypothetical index — no HypoPG, no missing-index DMV — so a
proposal **cannot** be verified by this app. The panel says so, in those words,
next to every `CREATE INDEX` it offers.

### 4.9 A window is two readings subtracted, and the baseline is a file

The counters are cumulative with no timestamps, so one reading is 54 days of
everything. `Set baseline` writes a snapshot; the panel can then show the
delta. The baseline is a **snapshot, not a log**, so it follows the
`profiles.rs` / `prefs.rs` pattern — `FILE_NAME`, `path_in(dir)`, whole-file
rewrite, corrupt file moved aside — and not `history.rs`'s append-only JSONL.

Keyed by `ConnectionId`, so two servers do not share one baseline.

*The limitation goes in the UI, not in a footnote.* The digest counters
aggregate **every connection on the server**. On UAT a recording captures 54
`SET NAMES` a second and a 40-million-iteration loop whether you caused them or
not. The delta view is clean on a database you have to yourself and noisy on a
shared one, so it leads with the signal that survives noise: **shapes whose
`FIRST_SEEN` falls inside the window**, which were never executed on that
server before you started.

### 4.10 Version-gated columns, the way `VIEW_TABLE_USAGE` already is

`QUANTILE_95`, `QUANTILE_99` and `QUERY_SAMPLE_TEXT` are MySQL 8.0 only.
`schema.rs:990` already has the precedent for this — a column list chosen from
`server_version`, which `session.rs:661` already captures. Tail latency is far
more useful than the average for finding a query that is usually fine and
occasionally terrible, so it is taken where it exists and absent where it does
not, without a second round trip to find out.

### 4.11 A capability, never an engine name

`Capabilities.workload_digests: bool` — true in `Capabilities::mysql()`, false
in Elasticsearch's. The rail button and the tab kind are gated on it. The house
rule from `engine.rs:39` holds: *"Anything that branches on this belongs in a
capability instead."*

### 4.12 Not a tab, and not a modal: the fourth dock. **Corrected twice.**

This decision was wrong twice and is worth both corrections.

**The plan said a tab kind.** `TabKind` would become
`"sql" | "dictionary" | "performance"`, and M12 would prove the third kind did
not break the second the way Stage 30 §8 had.

That was reversing a decision somebody had already made. `tabs.ts`, on the
type itself:

> *"There was a third, `migrations`: a view onto one Flyway project environment
> shown in place of the editor (Stage 18). Since Stage 28 that view is a panel
> docked on the right, so **no tab is anything but an editor**."*

Both existing kinds are CodeMirror buffers and `onActivate` switches language,
lint and buttons and nothing else. A workload table is not a buffer.

**So the first build made it a modal dialog** — `#history-dialog` is wide,
read-mostly and opened from the rail, and that looked like the match. The user
disagreed, and was right:

> *"If this is now a modal how does it play with the design? Wouldn't it be
> better as another side panel? That way optimization queries can be opened on
> regular tabs opposite from a modal where it will be lost. Is a similar
> reasoning why we moved assistant from modal to panel"*

It is the same reasoning, and Stage 21 §2 had already written it down about a
panel with the same job — handing you a statement:

> *"**Everything behind it is inert.** The schema tree, the editor and the
> result grid could not be clicked. So the moment a proposed query appears —
> which is exactly when you want to look at the table it reads, or the rows you
> already have — the panel is what stops you."*
>
> *"**Using a suggestion had to close it.** Both actions ended with
> `assistantDialog.close()` … Closing threw away the conversation that produced
> the statement."*

`openInTab` ended with `workloadDialog.close()`, and the code comment
*approved* of it: a tab made behind a modal is a tab nobody can see. Both
halves of Stage 21's complaint, reintroduced three stages later, with a test
asserting the defect was working correctly.

Query history gets away with being modal because its output is a statement you
insert and then stop thinking about. This panel's output is a statement you run
and then **compare against the reading that proposed it** — which is the one
thing a modal cannot let you do.

**So: the fourth dock.** `#wlsplit` at grid column 9, `#wl-pane` at 10,
`wl-shut` on `#app`, width in `--wl-w-set`, seam dragged like the other three.
520px by default rather than 380px, because the content is a statement with its
counters rather than a list of short lines — a default, not a cap.

Three consequences worth recording.

**M12 is withdrawn, not met.** No third `TabKind`, so the five `sql`-only
guards were never touched and Stage 30 §8's hazard was designed out rather than
tested for.

**The row layout changed with the container.** Seven columns were right for a
1,180px dialog and wrong for a dock somebody may drag to 400px — and a
`@media` query cannot help, because it asks about the *window* while the
constraint is the container. Two lines per shape need no breakpoint: the
statement on one, the verdict chip and four labelled numbers on the next,
wrapping.

**It shares the right-hand side.** §6.5 — which was itself got wrong once.

## 5. Milestones

- [x] **M1 — The panel reads a real workload.** One query, no `LIMIT`, the
      whole ring. The eight shapes of §3 are the Rust suite's fixture, so the
      verdicts are asserted against rows somebody can go and look at.
- [x] **M2 — Chatter is classified without being named.** `SET NAMES` by
      statement kind, `SHOW STATUS` by leading keyword, the `@@`-only select by
      having no `FROM`. The Aurora list is separate and commented as a list,
      because platform housekeeping has names and no shape.
- [x] **M3 — No table is named that the schema has no name for.** Asserted on
      the classifier's output over every fixture row, not trusted of the scan.
- [x] **M4 — `acl_rule` gets no index proposal.** `NoIndexWillHelp`,
      asserted against its real row — 1.51 B examined, 1.51 B returned.
- [x] **M5 — A view is named as a view with no round trip.** `TableRef.kind`
      was already `"BASE TABLE" | "VIEW"`; the Stage 30 cache had it all along.
- [x] **M6 — The loop detector finds the `Supplier` triple and nothing else.**
      Both directions: it fires on the real three (spread 0.005%, first seen
      140 ms apart, 298,374.95 s between them) and refuses two reads at one
      frequency, and a write ten seconds adrift.
- [x] **M7 — Every index proposal carries the table's write volume**, totalled
      over the whole ring. `Supplier` → 81,333,004.
- [x] **M8 — A baseline survives the app closing**, and the delta is the
      subtraction. A corrupt file is moved aside, not deleted.
- [x] **M9 — Nothing runs DDL.** There is no Run button in the panel; the
      analysis tab is `SHOW INDEX` and comments. A test asserts no
      `CREATE INDEX` is ever generated.
- [x] **M10 — An empty digest table is diagnosed, not shrugged at.** Four
      rungs, each a different sentence, and a test that no rung can say "no
      data".
- [x] **M11 — Elasticsearch gets no panel**, by `workload_digests` and not by
      engine name. The rail button is hidden rather than disabled.
- [ ] ~~**M12 — The third tab kind did not break the second.**~~
      **Withdrawn**, not met — see §4.12. There is no third tab kind: the panel
      is a dialog, so the five `sql`-only guards were never touched and the
      Stage 30 §8 hazard was designed out instead of tested for. Claiming a
      pass here would be claiming a test for a risk that does not exist.
- [x] **M13 — Nothing regressed.** 643 Chromium, 642 WebKit (1 skipped), 651
      Rust unit tests (1 ignored), `check` exit 0, no clippy warning from
      either new module.

## 6. Task tracker

### Phase 1 — reading the digests (M1, M11)
- [x] `workload.rs`: `DigestRow`, and `mysql_workload` beside it
- [x] `Engine::workload`, defaulting to empty like `all_columns` already does
- [x] `Capabilities.workload_digests`, false for Elasticsearch
- [x] Column list chosen from `server_version` — `QUANTILE_95` on 8.0 only
- [x] `SUM_LOCK_TIME` and `SUM_ERRORS` included; `DIGEST_TEXT` untruncated
- [x] No `LIMIT`; `SHAPES_SENT` bounds the shipping, after the aggregation

### Phase 2 — classification and verdicts, all pure (M2-M7)
- [x] The noise classifier over `StatementKind` plus the commented Aurora list
- [x] Table extraction, filtered through the schema cache's own names
- [x] The seven verdicts, with the three that must run first running first
- [x] The loop detector, grouped by table, requiring a write and a shared start
- [x] Write-pressure totals per table, over the whole ring
- [x] 49 unit tests on the real UAT rows; 6 falsifications

### Phase 3 — the panel (M1, M9, M12→§4.12)
- [x] The fourth dock — §4.12, corrected twice: not a tab, then not a modal
- [x] `workload.ts`: scope line, tiles, shape table, verdict column
      — the tiles became the lead sentence and the verdict column the row's
      first line in §6.11; the shape of the work was right, the layout was not
- [x] The expanded row: evidence, the priced write cost, the no-hypothetical
      -index sentence, the analysis tab, and the question to the assistant
- [x] The index inventory behind its own button and its own command, because
      it costs three more queries
- [x] Rail button gated on the capability, hidden rather than disabled

### Phase 4 — baseline and delta (M8)
- [x] `baseline.rs` on the `prefs.rs` pattern, keyed by connection id
- [x] `subtract`, with the window's own average and the quantile dropped
- [x] Counter-reset detection, checked across every row before any is
      subtracted
- [x] Absence from the baseline as the new-shape signal
- [x] The shared-server caveat in the dialog, not in a comment
- [x] 13 unit tests; 6 falsifications

### Phase 5 — the degraded states (M10)
- [x] The four-rung ladder, pure and tested: consumer, instruments, uptime,
      digests lost
- [x] Each rung a different sentence with the fix named
- [x] The no-privilege path, with the `GRANT` to ask for
- [x] A `usable` ring that has overflowed still warns

### Phase 6 — docs
- [x] §4.12 corrected on the record rather than quietly
- [x] M12 withdrawn rather than ticked
- [x] §6.1-§6.7 below

### Phase 7 — what running it turned up
- [x] §6.8-§6.13: the connection switch, the two hangs, the wrong verdict,
      420px, the `NULL` column name, the asserted cause
- [x] §6.14 expanded as a mechanism, §6.15 the batch dock
- [x] §6.16 one head for all four docks, and expand wired to every one
- [x] §6.17 the rail button named the vendor, not the feature
- [x] §6.18 query history as the fifth dock, and one rule for five seams
- [x] `stage-31-probe.sql` deleted; §8's preamble points at `workload.rs`

## 6.1 The wrong mask, and `lint.rs` had already hit it

Table extraction found **nothing**: nine tests failed at once, all of them the
ones that needed a name. `split::mask_noncode` blanks quoted identifiers along
with string literals, so `` FROM `Supplier` `` arrives as `FROM` and a bare
space.

The fix was already in the file. `split.rs` has four masks sharing one
scanner, and the second one says:

> *"Like `mask_noncode`, but keeps the contents of backtick-quoted identifiers
> (the backticks themselves become spaces). **The linter needs this:
> `` FROM `users` `` must still yield the table name.**"*

Same question, same mask. Worth recording because the first instinct was to
write a backtick-stripping pass — a fifth mask, in a different file, for a
question this project had already answered.

The second half of the bug was mine: a digest writes a qualified name as
`` `appdata` . `partysettings` ``, and once the mask turns the
backticks into spaces the dot is a token of its own. "The token after `FROM`"
is therefore the **database**, which the known-names filter then dropped —
so the symptom was an empty list rather than a wrong one, which is at least
the right direction to fail in.

## 6.2 A test that asserted an ordering which could not matter

`the_refusal_is_checked_before_the_proposal` claimed that `NoIndexWillHelp`
sits above `MissingIndex` so a bulk load cannot fall through into an index
proposal. Writing the falsification for it showed the claim is false: one rule
needs `examined / sent > 100` and the other `<= 2`, so **no ratio satisfies
both** and their order is not load-bearing at all. The test passed with the
rules in either order, which means it was testing nothing.

Two changes rather than one. The test now uses `party_bouncing_view`, where
the ordering genuinely decides the answer — ratio 109,342,481 with `no_index`
on every execution, so it satisfies the index rule outright and only the view
check standing earlier stops the proposal. And the disjointness is now a
`const _: () = assert!(...)` beside the two constants, so widening a band
until they overlap is a build error rather than something to discover later.

The module header was overclaiming in the same way and now names the three
checks whose position actually matters: platform, chatter, and view.

## 6.3 A control that agreed with the request instead of the answer

Found the same way. The window segmented control was painted from `wanted` —
what the person clicked — and the test for a void baseline passed either way,
because that test never clicked anything: it opened on the lifetime, where
"asked for" and "shown" agree.

They disagree in exactly one case, which is the case the feature exists to
handle: you ask for the window, the baseline turns out to be older than the
server's counters, and you are shown the lifetime instead. The control now
reads `panel.windowed`, so it reflects the numbers underneath it, and the test
clicks the button first so the two sources can be told apart.

## 6.4 A dock whose track was not zeroed at boot

Adding grid columns 9 and 10 without adding `wl-shut` to `#app`'s markup took
520px plus a 7px gap out of the window **from the first paint**, which pushed
the migrations seam past the right edge. Twelve migrations tests failed on
`locator.click: Test timeout` — nothing to do with migrations, everything to do
with the panel being unreachable.

`index.html` had the warning written on the line above:

> *"`chat-closed` from the first byte: the class is what zeroes the two
> right-hand grid tracks, and adding it from script would paint one frame with
> a 380px gap in it. `mig-shut` likewise."*

The comment explains it as a one-frame flash. It is worse than that: the class
is the *only* thing that zeroes the track, so leaving it off is not a flicker
but a permanent 527px theft. The comment now says so, and a test names it
rather than leaving it to twelve unrelated failures to find.

## 6.5 Three docks do not fit, and the first fix broke the point of the panel

Written as a test on a hunch — "three docks open at once leave the editor on
screen" — which then failed, with `#main` at **zero width**. 48 rail + 260
schema + three docks is more than a 1,280px window has, and `minmax(0, 1fr)` on
the main column is `1fr` that is *allowed to be smaller than its content*. That
property is load-bearing elsewhere — it is what stops a long schema pushing the
result grid below the fold — and here it means the editor silently vanishes.

**The first fix was mutual exclusion:** opening the performance dock shut the
assistant and migrations. It held the layout, and the user took one look:

> *"Mmm, if we cannot have the assistant and performance at the same time how
> could we use assistant to explain performance queries?"*

Of every pairing that could have been forbidden, that was the worst one. The
panel can say precisely what is wrong with a shape and nothing at all about
what to do with it; the assistant is the half that answers that. Forbidding the
pair removed the reason to have either open.

And the premise was wrong anyway. **The app already permits the editor to reach
zero width:** the assistant and migrations together are 760px of docks plus 308
of rail and schema, so on anything under about 1,100px the main column is
already zero today, and that is accepted, tested behaviour. The test asserted a
guarantee this app has never made anywhere.

So: no exclusion. Three docks may be open at once, the width is the user's
choice, and `Ctrl+B` buys back 260px of it in one keystroke. The default width
came down from 520px to 420px so that the pair that matters fits more often —
still a default and not a cap.

The test was replaced with the two honest ones: that the assistant and the
panel can be open together **in either order**, and that all three leave the
editor on screen on a window wide enough to hold them.

### 6.5.1 And the coexistence test only covered one order

The falsification found it. Putting the exclusion back in `setWorkloadOpen`
left the test passing, because the test opened the panel *first* and then the
assistant — which runs `toggleChat`, the other function. Half the exclusion
restored, the test green.

Opening either panel runs different code, so a rule about the pair has two
sides and needs asserting from both. The test now opens them in both orders,
and each direction falsifies separately.

## 6.6 The assistant hook the mockup had and the first build dropped

The design canvas gave the shape detail an **Ask the assistant** button. The
implementation shipped `Open an analysis tab` and nothing else, and nobody
noticed until the user asked how the two panels were supposed to work together
— which is a question about a button that was not there.

It is wired now, and the shape of it is worth recording: `assistant.prefill`
puts the composed question in the box and opens the panel **without sending
it**. A model call nobody pressed Send on is a cost nobody chose, and the
question is worth reading before it goes — this app proposes and the person
executes, which is as true of a question as of a statement.

The question carries the **evidence rather than the conclusion**: the counters,
the normalised statement, and the table's write volume, so the model argues
from what the server said instead of from what `workload.rs` decided. The
verdict is named too, because disagreeing with it is a useful answer — and the
closing line says "if an index is the answer, say which columns; if it is not,
say that instead", so a refusal stays askable.

Safe to send by construction: a digest holds no literal values, so there is
nothing in it a parameter could have smuggled out. The same property that makes
this panel safe to log.

## 6.7 `///` on a function parameter, again

> *error: allow, cfg, cfg_attr, deny, expect, forbid, and warn are the only
> allowed built-in attributes in function parameters*

The same mistake as Stage 30. A doc comment on a parameter is an attribute
position; `//` is fine and `///` is not. Noted a second time because noting it
once evidently did not take.

## 6.8 The panel did not follow the connection

Found by use, the day after the stage shipped: the dock stayed open across a
connection switch and kept showing the server that had been left — under a
scope line naming the one that had been arrived at. Not stale, which the
scope line would at least have hinted at. Wrong, and specific about it.

The cause is that the panel had exactly two ways in, `open` and `toggle`, and
both read. Nothing else did, because nothing else in the panel has any reason
to care which connection is active — the connection id arrives as a dependency
(`activeConnection()`) and is read at the moment a request is made. A switch
never makes a request.

**The reading is identified by nothing.** That is the part worth keeping. Every
number in the panel belongs to one server and no field in a `WorkloadPanel`
says which, so there is no way to notice the mismatch by looking at what is on
screen — not in a test, and not as a user. The fix is `connectionChanged()`,
which holds the id it last read for and compares:

```ts
connectionChanged() {
  if (deps.activeConnection() === readFor) return;
  if (!deps.isOpen()) return;
  wanted = "start";
  void load();
}
```

Three decisions in five lines.

**It is called from `syncWorkloadButton`, not from a connection hook.** That
function already exists for exactly this — the panel is offered per connection
and follows the same signal as every other per-connection control, which is
§4.11's capability rule and not a new wire. The cost is that it fires on far
more than a switch: `syncConnLabel` repaints on every tab change, every run,
every database change. Hence the identity check, which makes all of those free.
Putting the check inside the panel rather than at the call site is what makes
the call site safe to use carelessly.

**A shut dock reads nothing.** `open` reads unconditionally, so there is
nothing to keep fresh and no reason to spend the queries. This half has its own
test, because a fix that re-read on every switch regardless would have passed
the other two.

**The baseline window resets to the lifetime.** A baseline is a file per
connection (§4.9), so "since the baseline" after a switch is a question about a
server you are no longer on. The command would have fallen back to the lifetime
and `paintWindows` would have drawn it correctly — §6.3 already made the
control agree with the answer rather than the request — so this is not a bug
being fixed, it is a request not worth making.

`workload` had to be declared before `ConnectionManager` is constructed and
assigned where it was built, the same `let x!: T` shape `tabs` and `conns`
already use: the first connection-label sync happens inside that constructor,
long before the panel exists, and a `const` in its temporal dead zone throws
rather than reading as undefined.

Each of the three decisions was falsified separately. Dropping the call breaks
the switch test and the baseline test; dropping the identity check breaks the
switch test alone, on the exact read sequence it asserts; dropping the window
reset breaks only the baseline test. The shut-dock test is the one that passes
in every one of those, which is what it is for.

## 6.9 Running the probe file broke the app twice

The three `performance_schema` reads had never touched a server when the stage
shipped; §8 called that the one honest gap. Running them on 2026-11-05 closed
it — the SQL is valid, every column exists, the 8.0 branch of the gate is the
one taken — and turned up two defects that no test could have found, because
both need a server that does not answer.

### 6.9.1 A read on the shared connection could hang the app, with no way out

`SELECT * FROM information_schema.rds_metrics_gauge LIMIT 5` never returned on
the UAT instance. Neither did `information_schema.STATISTICS` over two schemas
-- which is this panel's own index inventory, the query behind **Read the index
inventory**.

That is worse than a slow panel. Every read in `workload.rs` runs on
`ServerConn::mysql_meta`: **one connection, behind one mutex, shared with the
schema tree and autocomplete.** A statement that never returns holds that mutex
for the rest of the session, and there is no cancel path for it at all --
`KILL QUERY` targets a *tab's* connection id and this connection has none. One
click would have stopped schema browsing and completion working, silently,
until the connection was closed and reopened.

The fix is a deadline, and it has to be the **server's**:

```sql
SELECT /*+ MAX_EXECUTION_TIME(15000) */ TABLE_SCHEMA AS db, ...
```

A client-side timeout would have been worse than nothing. It abandons the
connection mid-protocol — that is exactly what `exec.rs` does for a tab, and
why a cancelled statement costs a reconnect — and this is the connection that
cannot be abandoned. Handing the deadline to the server means the server aborts
the statement, returns `ER_QUERY_TIMEOUT`, and leaves the connection clean.

Three decisions worth keeping:

**The hint has to be the first thing after `SELECT`.** Anywhere else MySQL
reads it as a comment and ignores it, silently. That is what
`every_shared_read_carries_its_deadline_in_the_only_place_it_works` asserts,
and moving the hint one column to the right is what falsifies it.

**The SQL is now built by pure functions.** `digest_sql`, `indexes_sql`,
`index_use_sql`, `foreign_keys_sql`. The version gate, the deadline and the
placeholder count are testable without a server — which is what the untested
half of this module should have been from the start. One of the three new tests
pins the `QUANTILE_95` gate that §4.10 argued for and never checked.

**The refusal says which half still holds.** A timeout on the inventory gets
its own sentence, because the workload above it is still true and a shared
error message would read as a broken panel. `refusal_text` takes the error
*code*, not the `sqlx::Error`, for an unglamorous reason: a
`sqlx::Error::Database` cannot be constructed outside the driver, so a test
written against the error type would only ever reach the fallback — while
claiming to have checked both branches.

**Still unanswered:** whether that instance can serve `information_schema.STATISTICS`
at all. A deadline stops it hurting; it is not a measurement. §8.2.

### 6.9.2 A cancel outlived the run it cancelled, and wedged the tab

Not this stage's code — the defect is in the cancel machinery — but this
stage's probe file is what found it, so it is recorded here.

Cancelling the hung statement worked exactly as designed: the wait stopped, and
the mid-protocol connection was dropped, because it cannot be reused. From then
on the tab answered `Cancelled.` to **everything** — every statement, every
export, every change of database — and disconnecting and reconnecting was the
only way out. That is not a cancel, it is a restart.

Both run paths cleared `cancel_requested` *after* `session::ensure_exec`, and
`ensure_exec` opens through `cancellable`, which refuses outright while that
flag is set. So the flag that stopped one run went on to refuse every
replacement connection, for the life of the tab. It only ever showed up after a
cancel that *abandoned* the connection: when the statement came back in time,
`ensure_exec` reused it and never reached the refusal. A hung query was the
missing ingredient, which is why nothing caught it.

The fix is one rule with one name: `session::begin_run` clears the flag
**before** opening anything, and is now the only way the three callers — run,
export, `USE` — take the editor connection. It is safe there because it holds
the `exec` lock, and a run holds that lock for its whole life: holding it means
no run is in flight, so there is nothing left for the flag to describe.

`running` is deliberately left outside it. Setting that before the connect
would leak a busy tab forever if the connect failed, which is the same class of
bug one layer along.

The test asserts *which* error comes back from a tab pointed at a refused port:
a connection error means it reached the socket, and `Cancelled.` means it never
tried. Swapping the two lines back makes it fail with literally the string the
report described.

## 6.10 The panel's verdict on the worst shape on the server was wrong

§8.1 recorded this and left it: the 5 Nov read returned the whole `Supplier`
statement for the first time, and its predicate is

```sql
WHERE tenant_sub_id = ?
  AND ( NAME = ?
     OR JSON_CONTAINS(LOWER(COALESCE(ALIAS, ?)), JSON_QUOTE(LOWER(?))) )
```

`verdict_of` called that **`missingIndex`** — 41 M executions, 79h 24m,
2,599 rows read per row returned, 1,766,585 of them finding no index — and
every one of those triggers is real. The conclusion is not. No b-tree index
applies to `LOWER(COALESCE(ALIAS, ?))`, and MySQL cannot index-merge an `OR`
whose other branch is unindexable, so it scans. The panel's single most
prominent finding was advice to build an index that could not be used.

The fix is a rung, `Verdict::PredicateNotIndexable`, and **where** it sits is
most of the design:

```rust
if ratio > RATIO_WANTS_INDEX && (row.no_index > 0 || row.scans > 0) {
    if wraps_a_column(&row.text) {
        return Verdict::PredicateNotIndexable;
    }
    return Verdict::MissingIndex;
}
```

Inside step 5, not before it. §6.2 is the record of how easily verdict
ordering goes wrong, and a rung placed earlier could have intercepted
`ViewBlocksFilter`, `NoIndexWillHelp` or `WaitingNotReading` — four verdicts
that were already right. From here it can only ever change the one it was
added to change, and `the_new_rung_cannot_reach_any_other_verdict` appends a
wrapped column to four other shapes' text and asserts every one of them keeps
the verdict it had.

### What the detector can and cannot see

`wraps_a_column` is a list of functions and a scanner, not a parser — this
app does not parse SQL. It reads the text after `WHERE`, finds a call to one
of the functions that turn a column into a computed value, and asks whether
the argument list mentions a column at all. That last question is the whole
rule: `LOWER(COALESCE(ALIAS, ?))` hides a column, `LOWER(?)` hides nothing
and is perfectly indexable on the other side of the comparison. Both guards
were falsified by removing them, and each one fails a test of its own.

It is wrong in one direction on purpose. It does not know which side of a
comparison it is on, so `WHERE col = ? AND other = LOWER(col2)` answers yes
even though an index on `col` would help. That is tolerable **because of
where it is called from**: a shape only reaches this branch if the server is
already reading a hundred rows for every one it returns and has found no
index on some executions. A sargable branch that was keeping the shape
healthy would not leave the counters looking like that. A wrapped column in a
`JOIN ... ON` or a `HAVING` is missed entirely, and falls back to
`MissingIndex` — which is what it said before any of this existed.

### The fixture was the reason this survived review

`uat()` carried the statement as the 4 Nov probe printed it, and that probe
truncated mid-predicate. The stand-in written in its place read
`OR supplier_master_id = ?` — sargable, so `missingIndex` was the right
answer to the question the fixture was asking. Two tests asserted it. Both
were correct about a statement that does not exist.

Correcting the fixture broke exactly those two tests and nothing else, which
is the most useful thing that happened here: the test suite had been
measuring the half of the predicate that had been cut off, and said so the
moment it was given the whole thing.

## 6.11 420px was the design, and it was the wrong one to only have

Everything awkward about the shipped panel comes from one number. In a 420px
column there were six bands of chrome above the first row of data — title,
window control, scope line, diagnosis, four tiles, loop strip, two segmented
controls, a table-names line — the tiles wrapped two by two, the verdict
chip was the smallest thing on a row and sat under the SQL, and the index
inventory was a second unrelated list in the same scroller, below the fold.

Two changes, and the second is why the first could be made.

**The dock says less.** The four tiles become one sentence — *"79h 24m of
the last 54 days went into one shape on `Supplier`, and no index can help
it"* — because the tiles named every number in that sentence and left the
reader to write it, with the verdict that explains them three bands further
down. `Workload` and `Indexes` become sibling tabs instead of a stack, so
neither is below the other's fold. Each row leads with its verdict and its
reason, and the statement drops to dimmed evidence; it had been the other way
up, which made every row look alike until it was opened. The loop attaches to
the shape it is about, where before it sat in a strip above whichever shape
happened to sort first. The window control, the baseline and the scope line
go to a footer, because they only mean something once you have read a
reading. Three bands before data instead of six.

**And the panel can take the window.** `#app.wl-full` is `wl-shut` inverted:
that rule zeroes this dock's own track, this one zeroes every other one. Two
tiers on purpose — the dock is the glance, expanded is the study — and
expanded is where the whole 288-byte statement is on screen instead of a
clipped line, and where the inventory sits beside the workload rather than
behind a tab. The tabs disappear there, because nothing is competing for the
width and there is nothing left to switch. That switch is a `data-view`
attribute read by two CSS rules, so one rule in the expanded block turns the
whole mechanism off rather than this module having to know which mode it is
in.

It is deliberately not reachable by dragging. `draggable`'s `x-wl` arm clamps
the dock to `innerWidth - 420` so the editor always keeps a column, which is
the right rule for a seam and the wrong one for a mode.

### Three things that had to be true before it was worth building

**Nothing slides into a vacated track.** §6.4's neighbour is the comment at
the grid definition: auto-placement counts only displayed items, so hiding
the schema once slid `#main` two columns left and took the editor with it.
Every pane is pinned to a named column now, which is what makes
`display: none` on the editor safe.

**CodeMirror survives being hidden.** Read from the vendored source rather
than assumed: in `@codemirror/view` 6.43.11, `dist/index.js` lines 6370-6376,
`inView` is computed from the pixel viewport having non-zero width and
height, `measure()` returns early while it is false, and the transition back
to true sets `measureContent` — so it records no geometry while hidden and
re-measures itself on the way in. There is a `DOMObserver`
`IntersectionObserver` on the editor DOM at line 7177 besides. No manual
`requestMeasure` is needed, and the test that types into the editor, expands,
collapses and finds the text still there is the one that would catch it if
that ever changed.

**There is no shut-and-expanded state.** That one hides the editor behind a
pane that is not on screen to bring it back. The invariant lives in
`setWorkloadOpen`, not at each call site, so the rail button, the Close
button and losing the capability all go through it. `Esc` leaves the expanded
view, in a listener of its own because the shortcut map returns immediately
unless Ctrl or Meta is held — and it answers only while the panel is
actually expanded, so it cannot swallow an `Esc` meant for anything behind
it.

## 6.12 A `NULL` column name dropped a key part, and five true findings with it

Found by running the inventory against both servers on **2026-11-06**, not by
reading the code. `mysql_indexes` decoded every column of its row as a
`String`:

```rust
let (Ok(db), Ok(tbl), Ok(idx), Ok(col)) = ( /* ... */ ) else { continue; };
```

`information_schema.STATISTICS` reports `COLUMN_NAME` as **`NULL`** for a
functional key part -- an index on an expression rather than a column, which
MySQL 8.0 has had since 8.0.13. `try_get::<String, _>` on a `NULL` is an
error, so the `else` arm fired and the whole row was dropped. Not the column:
the **key part**, silently, leaving the index narrower than it is.

This schema has six of them, on `external_client`,
`external_party_account`, `external_secondary`,
`external_holding_account`, `external_signup` and
`reconciliation_config`. Two consequences, both measured:

**Five true redundancy findings were lost.** `external_client` carries a
one-column non-unique index on `service_org_id` *and* a unique index on
`(service_org_id, <expression>)`. The narrow one is redundant -- the
unique one serves it. With the expression part dropped both indexes read as
one column wide, so neither was a strict prefix of the other and nothing was
reported. Fifty of fifty-five on UAT; the five that went missing are all this
shape.

**One index was described wrongly.** `uq_reconciliation_config_scope` is
`(<expression>, <expression>, entity_type)`. Dropping the first two left it
reading as an index on `(entity_type)` -- so `unindexed_foreign_keys`, which
judges support by leading prefix, would have counted it as support for a
foreign key on that column that it does not serve. Nothing in either schema
has that key today. The rule was wrong anyway.

### `None` is "a key part we cannot read", not "no column"

`IndexRow::columns` became `Vec<Option<String>>`, and the two rules that read
it now refuse to conclude anything that would need to know what an expression
is:

* **Redundancy.** Proposing that an index be dropped is a claim that another
  one covers it. `a` may therefore not contain an expression -- two opaque
  parts are not known to be equal. An expression in `b` *beyond* `a`'s length
  is harmless, because nothing is being compared to it, and that asymmetry is
  what recovers the five findings rather than merely silencing the bug.
* **Foreign-key support.** An expression supports no column, so a key part
  that cannot be read never matches one that can.

`IndexFinding::columns` stays `Vec<String>`, with an expression rendered
`(expression)` at the boundary by `shown`. A blank in the panel would read as
"this index has no columns", which is a different and wronger claim.

### The decode was not reachable from a test, which is why it survived

A `sqlx::Row` cannot be constructed outside the driver, so any test of
`mysql_indexes` would have exercised whichever branch its fixture happened to
take. That is the same trap `refusal_text` was split to escape in §6.9.1, and
the same answer: `fold_part` now takes the already-decoded parts, so
`a_null_column_name_is_a_key_part_and_not_a_lost_row` can hand it a `None` and
watch what happens to it. All three guards -- the decode, the opacity rule and
the asymmetry -- were falsified by breaking them one at a time, and each fails
a test of its own.

The general lesson is narrower than "test more". Every column this module
reads from `information_schema` is nullable in some schema somewhere, and
`try_get::<String, _>` turns that into a dropped row rather than a visible
gap. A `continue` in a decode loop is a silent data loss, and this one was
invisible for as long as the only schemas it ran against had no functional
indexes.

## 6.13 The panel asserted a cause it had not established

Trying to run the usage read by hand on 2026-11-06, the server rejected it.
Which exposed two things, and the second is worse than the first.

### The query

`index_use_sql` emitted `COUNT_READ AS reads`. Every other alias in the probe
folder had already been proved safe against these servers -- `db`, `tbl`,
`idx`, `col`, `non_unique` all came back fine from the index reads -- and
`reads` was the only new one. **Settled by re-running it**: with the alias
backticked the identical statement returned 1,025 rows, so the alias was the
whole problem. Which specific rule of Aurora 3.10.3's parser rejected it is
still not something this repo can check -- there is no MySQL keyword list in
`node_modules`, and the one in `@codemirror/lang-sql` is 220 words for
highlighting rather than MySQL's ~260 reserved words -- and it no longer
matters, because the fix does not depend on knowing.

It does not need to be established, because the fix does not depend on it.
Every alias in every generated query is backticked now, and
`every_generated_alias_is_quoted_and_no_cast_target_is` keeps it that way. A
quoted alias is valid whatever the server's keyword list holds, and this panel
generates SQL for every MySQL it is pointed at, including versions whose
reserved words it cannot know.

### The error handling, which is the real defect

```rust
let (uses, usage_available) = match mysql_index_use(server, dbs).await {
    Ok(u) => (u, true),
    Err(_) => (Vec::new(), false),
};
```

`Err(_)`. The reason thrown away, and the panel then printed

> performance_schema refused, so never-read indexes are not listed

for **every** cause. A privilege wall, a deadline, a dropped connection and a
malformed query of our own all arrived as the first one. §4.x of this document
is careful that "absent" and "none" must not read the same; this is the same
mistake one level up -- *"the server would not let us"* and *"we sent it
something it could not parse"* must not read the same either, and the second
one is a bug in this module being filed against somebody's database
permissions.

`IndexScan` and `IndexInventory` carry a `usage_refusal: Option<String>` now,
and the panel prints what the server said instead of what it assumed.

### And the first fix was worse than the bug

Quoting the aliases with one pass over the text turned
`CAST(... AS UNSIGNED)` into ``CAST(... AS `UNSIGNED`)`` -- invalid, in the
digest read, which is the one query the entire feature is built on. The test
written in the same breath passed, because it asked only whether a backtick
followed every ` AS `.

So the test now asserts both directions: every alias quoted, and **no cast
target quoted**. Both were falsified by breaking them one at a time. A rule
checked in one direction will wave through the mistake that breaks the other,
and a blanket find-and-replace over generated SQL is exactly the change that
makes that mistake.

## 6.14 Expanded was built for one dock and wanted to be a mechanism

§6.11 gave the performance panel a second tier: `#app.wl-full` zeroed every
other grid track and let the dock take the window. Then the batch report
became a dock of its own and wanted the same thing, which is the moment a
feature is supposed to turn into a mechanism rather than a second copy.

`data-full` on `#app` names **which** dock has the window, and `.dock-full`
marks that pane:

```css
#app[data-full] { grid-template-columns: 48px minmax(0, 1fr); }
#app[data-full] > * { display: none; }
#app[data-full] > .railbar { display: flex; grid-column: 1; }
#app[data-full] > .dock-full { display: flex; grid-column: 2; }
```

Four lines, once, instead of four lines per dock. The class on the pane is
there because **CSS cannot compare two attribute values** -- `[data-full="wl"]`
cannot select "the pane that names", so without it each dock would need a rule
of its own to say which pane the attribute meant.

### One value, not a flag per dock

§6.5 settled that two docks may be open at once -- an earlier cut shut the
others and that broke the point of the panel. So "expanded" cannot be
something each dock holds independently: exactly one can have the window, and
the others go away while it does. `setDockFull(dock, full)` is the only way in,
`EXPANDABLE` is the register a new dock adds itself to, and
`only_one_dock_can_have_the_window_at_a_time` is the test that expanding the
second takes it from the first.

### The column pinning had to learn about it

Every pane is pinned to a named column, which §6.4's neighbour explains: auto
placement counts only displayed items, so hiding the schema once slid `#main`
two columns left and took the editor with it. Those rules are two ids each, so
they beat the generic one -- and an expanded pane stayed in column 10 of a
two-column grid. They all carry `:not([data-full])` now. A pinned column is
right for the ordinary layout and wrong for a dock that needs the whole row.

## 6.15 The batch report was in the wrong place, twice

A folder run's report started as `results.setMessage`, which `onActivate`
repaints per tab -- so switching tabs threw away the report of a run that had
taken minutes. Stage 23 moved it to a band in the output pane, outside the tab
system, and that was right about tabs.

It was silent about connections. A run picks its own connection **in its own
dialog** -- `plan.connectionId` is `#sf-connection`'s value, not the active
connection -- so a report from UAT sat on screen while QA was selected, with
nothing on it saying which server it came from.

### Why a dock and not a third view in the output pane

The first attempt was a `Batch` button beside `Results` and `Sequence`. It
does not work, and the reason is one line: `setOutputView(tab, next)` stores
the chosen view **on the tab**. Those two are views of *this tab's run*; a
batch report belongs to a connection. Putting per-connection content behind
per-tab state is the mismatch, and no amount of count badges fixes it.

A dock settles it, and the precedent is already in this repo --
`workload.ts`'s own header records that Stage 28 moved migrations *into* the
dock because a long job with its own connection is not a tab's output. So the
fifth dock, with the same four pieces as the other four: a `batch-shut` class
that zeroes its track, a `--batch-w-set` width that shutting leaves alone, a
seam, and a rail button.

### The rail button, and what Hide means now

`#btn-sqlfiles` opened the dialog directly. Every other rail button toggles a
pane, and the old behaviour meant a dismissed report had nowhere to go back
to. It toggles the dock now and `Run a folder...` is a button inside it, as
Migrations' actions are inside Migrations. `Ctrl+Shift+B` opens the dock and
then the dialog, because asking to run a folder is asking to see what it
produces -- the same move `Ctrl+P` makes for the schema.

`Hide`, inside the report, can only sensibly mean "discard this one" now that
the dock has a `Close` of its own. Two controls that looked like the same
thing would be worse than either.

### Swapped, not cleared

`batchReports` is a `Map` keyed by the connection the run was given. Switching
away hides a report; switching back brings it back. Clearing was the other
option and it is worse: it throws away the report of a finished run for a
reason that has nothing to do with the run, and the report is how somebody
finds the CSVs it wrote. "One batch per connection" is what the `Map` is.

Tab switches do not touch it, by construction -- it is not in the tab system
at all, which is the whole argument for the dock.

### And a browser notice this provoked

Expanding a dock on a **narrow** window with a second dock open makes WebKit
report `ResizeObserver loop completed with undelivered notifications`. Four
panes change in one frame, and both windowed lists here watch the size they
themselves change by painting into it -- `ResultView` and `SequenceView` each
hold a `ResizeObserver` whose callback repaints rows, which changes the
scroller's content size.

It is benign: the browser delivers the rest on the next frame and the UI
settles correctly. A wide window does not trip it at all. Deferring those
repaints by a frame was the obvious fix and **did not stop it**, so that change
was reverted rather than kept on the strength of sounding right -- something
else in the chain, most likely one of CodeMirror's own observers, is a
participant. The spec tolerates that exact message and nothing else, matched
in full so the exception cannot grow into a filter that hides real errors, and
§8.3 keeps it as a known rough edge rather than a fixed one.

## 6.16 Four docks, three heads, two of which could not expand

Found by running the app, which is the only way this one was ever going to be
found. Nothing was broken; the four docks simply did not agree with each other,
and no test could see it because each dock's tests only ever look at that dock.

What was on screen:

| dock | head | title | close | expand | actions |
| --- | --- | --- | --- | --- | --- |
| Migrations (Stage 28) | `.pane-head` | `.chat-title` | `×` | no | -- |
| Assistant (Stage 29) | `.pane-head` | `.chat-title` | `×` | no | `Clear`, no tooltip |
| Where the time goes (§6.11) | `.wl-head` | `.wl-title` | `Close` | `Expand` | `Refresh`, no tooltip |
| Folder runs (§6.15) | `.wl-head` | `.wl-title` | `Close` | `Expand` | -- |

Four defects in one table. The assistant's title class was being used by
migrations, which is how a class called `chat-title` ends up styling a word
that is not "Assistant". Two docks closed on a glyph and two on a word. And
§6.14 had just finished turning expanded into a mechanism -- then wired two of
the four docks to it, which is a different thing from being general.

### The head

One `.dock-head`, in one order: the name, the note the expanded view adds, the
dock's own actions, the two controls every dock has. The spacer is `.grow`
rather than `flex: 1` on the title, so the expanded view's note sits beside the
name instead of being pushed to the far side of the row.

`.pane-head` stays for the sidebar and the editor. Those are panes and not
docks: nothing shuts them and nothing expands them, so giving them the dock's
head would have meant a head with two controls missing.

### The vocabulary

Four drawings: **chevrons** for the width, **close** for the door, **refresh**
for a re-read, **trash** for "forget this conversation". A control in a dock
head is an icon if it is one concept, and a word only if it takes an argument
-- which leaves exactly one word in the four heads, `Run a folder…`, and it
opens a chooser.

**They are drawn, not typed.** The first cut of this used `«`, `»` and `×`
as characters, and `icons.ts`' own header had already recorded why that is
wrong: a text glyph has thin font coverage and a missing one renders as a
hollow box, while an emoji ignores `color` and arrives in the system's colour.
Those three characters happen to be safe ones -- but the rail beside these
heads is drawn SVG, so a head of characters would have been *two drawing
systems in one window*, which is the inconsistency this section exists to
remove. `close` is now literally the same drawing as the result grid's close
and `refresh` the same as the schema tree's.

The chevrons point the way the pane is about to move: left, across the editor
it is about to cover, while the dock is collapsed; right, giving the editor
back, while it has the window. That is **one** drawing turned by CSS reading
`aria-pressed`, not a drawing per state -- the same argument the tree's twisty
makes, and it means the picture cannot disagree with the state.

The words did not disappear; they moved to `aria-label`, which is the
accessible name *and* the subject of the tooltip. An unlabelled icon is a
guess, and an unlabelled `trash` is a guess with consequences. The close
control's name says which panel it shuts -- "Close migrations", not "Close" --
because four buttons that all read "Close" is the same defect one step further
on, in the place a screen reader reads from.

Every control in a head has a tooltip now, the words included: "Clear" is a
word, not a sentence, and *what* it clears and what that costs is the part
worth hovering for -- it drops the conversation **and** the context the model
had. A test asserts a tooltip on every one of them, because the two that were
missing one were missing it for a stage and a half.

`.dock-icon` is a 22px × 20px box rather than `.mini`'s pill: 9px of side
padding around a 12px drawing reads as a gap in the row, not a button. Every
head control is that size, so a row of them is a row rather than four sizes of
button.

### One painter for four controls

There were two `syncExpand` functions, near-identical, one inside
`workload.ts` and one beside the batch dock -- and `Esc` had to call both **by
name**:

```ts
if (collapseFullDock()) {
  workload?.syncExpand();
  syncBatchExpand();
  e.preventDefault();
}
```

That is the shape in which the third and fourth docks arrive with controls
that disagree with the key that undoes them. `syncExpandControls()` paints
every registered dock's control from `fullDock`, `setDockFull` calls it, and
`Esc` is one line again. The performance panel no longer owns an expand control
at all: `expand`, `setFull` and `isFull` are gone from its deps, because
expanding is the app's mechanism and the panel's business is digests.

`EXPANDABLE` also stopped asking each dock whether it is open and started
reading `pane.hidden`. Two of the four flags are declared *below* the line
that would read them, and a `let` read before its declaration runs throws --
while `hidden` is set by the same four functions that set the flags and cannot
disagree with them.

### What the invariant cost before it was shared

`setWorkloadOpen` and `setBatchOpen` each held one line -- shutting a dock
leaves the expanded view first -- and migrations and the assistant had never
needed it. They do now, and both have it, because the state it forbids is the
one where the editor is hidden behind a pane that is not on screen to bring it
back. `workload.ts`'s `shut()` used to do this itself; it is three lines
shorter and the rule lives in one place.

### And then none of them rendered

Reported immediately: the new buttons were empty. Not the wrong icon -- no
icon.

`.dock-icon` carried `padding: 0`, and it was written as a bare class beside
`.dock-head`, one specificity point weaker than `button.mini`'s
`padding: 3px 9px`. That rule won. With `box-sizing: border-box` a 22px button
then spent 18px on padding and 2px on its border and had a **2px content
box**.

What happened next depended on the engine, which is why every test passed:

| engine | the 12px drawing came out |
| --- | --- |
| Chromium | 12px -- the stated width kept |
| WebKit | **2px** -- shrunk into the content box as a flex item |

**Tauri ships WebKitGTK on Linux.** The engine that disagreed is the one the
app runs on, and the engine that agreed is the one the suite's default project
uses. Measured both ways afterwards rather than reasoned about: with the old
stylesheet the new test reports `mig-expand is 2px wide` on `--project=webkit`
and passes on `--project=chromium`.

Two rules were wrong, so both are fixed: `button.dock-icon`, below
`button.mini`, so the padding the design asks for actually applies; and
`flex: none` on the drawing, because a 12px icon is never the thing in a row
that should give way. **Either one alone is enough** -- falsified separately,
and that is the point of fixing both. A padding that only works because a
sibling rule stops the damage is a rule waiting to be moved.

The missing test was the whole lesson. Eight assertions about the DOM -- which
icon, which name, which order, which `transform` -- and not one of them asked
whether the thing had a size. `expect(box.width).toBeGreaterThan(8)` is now
the first thing that would have failed, on the engine that mattered. Two
earlier falsifications of mine *passed* while the bug was live, which is the
signal I already keep a rule about: a falsification that passes means the test
proves less than it claims. It took measuring the box to find out what.

### The order they appear in is fixed, and that is deliberate

Asked while reading this: the docks always sit left to right as Migrations,
Assistant, Where the time goes, Folder runs, whatever order they were opened
in. Each is pinned to its own grid column -- `#mig-pane` is column 6, the
assistant 8, performance 10, folder runs 12 -- so click order has no say.

Kept. A dock in the same place every time is muscle memory, and opening a
second one never shuffles the first out from under the pointer. The pinning is
also load-bearing for §6.14: grid auto-placement counts only *displayed*
items, which is the bug that once slid `#main` two columns left when the schema
was hidden, and it is what makes `display: none` on an expanded view's
neighbours safe. Click order would mean assigning columns from JavaScript
against an open-order list, and a dock that moves when you open another one.

### The test is about the set, not the docks

`tests/ui/docks.spec.ts` loops the four. The defect was never *in* a dock --
it was in the gaps between them, and a test that visits one dock cannot see a
gap. It asserts the last two children of every head are that dock's expand and
close, in that order, with those glyphs, with a name on each; that every head
carries the note; that the two docks which could not take the window now can;
and that a second dock takes it from the first rather than two docks each
believing they have it.

Falsified seven ways: the painter's call removed (the loser's control kept
`aria-pressed="true"`), the shut-first invariant removed from migrations (the
editor stayed hidden), the two controls swapped in one head, one close
control's name shortened to "Close", a `×` typed back into a close button,
one tooltip shortened to its own button's word, and the rotation rule's
selector broken so both states drew the same picture.

### Expanded, for the two new ones

Migrations needed no layout of its own -- it is a list of versions with their
descriptions, a table, and a table takes every pixel it is given. The
assistant needed one thing: prose does not read better at 1,800px, so the log
and the composer keep an 820px column and centre it.

## 6.17 The rail button was named after the program we shell out to

Reported from the running app: the assistant's rail button said **"Ask Claude
Code"**.

Which was true -- `paintAssistantButton` interpolated `cliLabel(recipe)` --
and wrong in four directions at once. The button opens a dock whose head says
"Assistant". The same button on an HTTP provider said "Ask the assistant". The
shortcut list calls `Ctrl+K` the assistant. And the thing being named is a
feature of this app, not the program it happens to shell out to; a user who
switches provider has not switched feature.

One string, `ASSISTANT_TIP`, in `main.ts`, matching the markup's own copy:
"Ask the AI assistant (Ctrl+K)". **The provider did not disappear**, because
with two CLIs installed which one answers is worth knowing without opening
Settings -- it is the tooltip's second line, "Answered by Copilot CLI". The
not-ready branch is untouched: there the tooltip's whole job is to name the
program and what is wrong with it, "Run `copilot login`".

Four tests read the vendor's name off that button, one of them as its evidence
that a preference loaded from file had been applied before the first frame.
They assert the second line now, which is the same evidence -- nothing but the
loaded preference could have put it there -- and falsifying it by dropping the
line fails all four.

Not changed: the two in-panel labels that say "Ask the assistant" (the
performance panel's per-shape button, and the dock's own head). Those sit
beside the panel they refer to, and the defect here was a vendor's name
leaking onto the chrome, not the word "assistant".

## 6.18 The last dialog that was a reference, not a decision

Asked while reading §6.16: should query history be a side panel too? Yes --
and not as a matter of taste. `history.ts` called **`deps.dialog.close()` on
the line before `deps.insert(...)`**, and again before `openInTab`, because a
modal owns the screen and cannot put text into the editor behind it. Using one
row destroyed the list you were working through. That is, line for line, what
Stage 21 §2 recorded about the performance panel's own first cut as a
`<dialog>` -- the comment still at the top of `workload.ts`, "using a finding
had to close it" -- and the reason that panel is a dock.

### The rule, written down at the fourth time of asking

This stage hit it in §6.5, §6.15, §6.16 and here, and re-derived it each
time:

| | |
| --- | --- |
| **A dock** | a reference you work *from*: it stays open while you type, and using it hands something to the editor |
| **A modal** | a decision in flight: one choice, commit or cancel, nothing to say afterwards |

Against that rule the remaining dialogs sort themselves and all five stay:
new connection, Settings, Export, Export schema, and the folder chooser are
every one of them commit-or-cancel. Query history was the only one on the
wrong side of the line.

### What the dock is

The fifth, built on the same five lines as the other four: `hist-shut` zeroes
the track, the pane is `hidden` as well as zero-width so a shut panel cannot
take Tab, the dragged width lives in `--hist-w-set`, columns 13 and 14 are
pinned, and `setDockFull("hist", false)` runs on shutting so there is no
shut-and-expanded state. The rail button existed already and is a toggle now,
with `aria-pressed`; `Ctrl+H` toggles with it, as `Ctrl+K` and `Ctrl+Shift+M`
do for their docks.

Two smaller consequences, both of them improvements by accident:

* **The arrow keys moved from the dialog to the pane.** On a modal that
  listener saw every keystroke in the app, because a modal has the keyboard.
  On the pane it sees the keys meant for the panel -- which is the behaviour
  the comment above it claimed both times.
* **The list stopped capping itself.** `max-height: 56vh` existed to stop a
  dialog growing past the viewport; a dock is given a column and told to fill
  it, so it is `flex: 1` with `min-height: 0`.

And one rule inherited rather than invented: §6.8's. A dialog was opened,
read and dismissed inside one connection, so deciding its scope once at
`open()` was sound. A dock outlives the switch, and a list scoped to "this
connection" would have gone on showing the old server's statements with
nothing on screen saying so. `connectionChanged()` re-reads it -- and only
when the scope *is* the connection, because an all-connections list is the
same list whichever one is active. Shut, it costs nothing.

**The first cut of that reused `open()`'s own two lines**, and so re-checked
"This connection only" on every switch -- overriding a deliberate "every
connection" with a list of one. `open()` and a switch want different things
from the same control: one *decides* the default, the other may only change
what is *available*. The test that found it was written in the same pass and
had no idea it was about to: `an unscoped list is not re-read on a switch`.
What a switch does now is set `disabled`, and widen the scope only when the
connection it was scoped to has gone.

### The fifth seam found a bug in the other four

`draggable` had a branch per dock, and three of them measured the pointer's
distance from **the window's right edge** -- true of each one on the day it
was written, because each was the outermost dock at the time. The migrations
seam already had the right version, measuring from its own pane's right edge,
and `migrations.spec.ts` already carried the test that says why: dragged
rightwards from a window measurement the panel *grows* instead of shrinking,
"because the measurement would be 387px too wide".

A fifth dock makes that wrong for three seams at once, so there is one
`x-dock` branch now and a `DockSeam` per seam: the pane it sizes, the variable
it writes, and the floor its narrowest row needs. Five seams, one rule, and
the floors are the only numbers that differ -- a SQL block's four buttons,
Apply/Repair/Refresh, a verdict chip with its counters, a file name with three
numbers, two lines of clamped SQL.

**The ceiling turned out to belong to the editor, not to the dock.** Each seam
had carried its own version of it, `window.innerWidth - 420` for three of them
and `right - 560` for migrations, and both forms are the same wish -- leave
the editor usable -- written as arithmetic about whichever panes existed on
the day. My first generalisation kept the shape of the old one, `right -
keep`, and two seam tests immediately said no: with a second dock open the
pane's right edge is no longer near the window's, so the ceiling fell *below
the pane's current width* and a drag asking it to narrow by 60px clamped it
inwards by 80. Both tests reported the same "40px, not 60".

What it is now: one `EDITOR_KEEPS = 420`, measured off `#main`'s own width at
each mousemove, and floored at zero -- because the number is a licence to
**grow**, so an editor already narrower than 420px grants none and the pane
can still be dragged *narrower*. True wherever a dock sits, whatever is open
beyond it, and it is the first version of this that is a statement about the
editor rather than about a layout that happened to hold.

### Two tests inverted, which is the point

`history.spec.ts` asserted `#history-dialog` was **hidden** after a row was
used -- twice, once for a click and once for Enter. Both now assert the panel
is still visible with its rows still in it. The same inversion §6.5 describes
for the performance panel, and the reason to do this at all: the second
statement you want is usually two rows from the first.

Falsified five ways: `setOpen(false)` put back into `accept()` (both
inversions failed), `connectionChanged()` unhooked (the scoped switch failed),
and the seam measured from the window again with an unbounded ceiling (both
seam tests failed, the one in `docks.spec.ts` and the one `migrations.spec.ts`
has carried since Stage 28).

## 7. Out of scope, and why

**The slow query log.** `mysql.slow_log` holds statements with their real
literal values. Everything in this stage is safe to log and show *because the
digest carries no literals* — that is a property of the source, not a
redaction, and it is what makes the feature defensible. Reading the slow log
inverts that, and this project's rule is that secrets fail closed. A separate
stage, with its own argument.

**`processlist` and a live session view.** Needs `PROCESS`, shows in-flight
statements with values, and answers a different question — "what is happening
now" rather than "where has the time gone".

**Host metrics.** UAT's digests show something querying
`information_schema.rds_metrics_gauge` joined to `rds_metrics_meta`, 78,684
times, which means Aurora exposes gauge metrics *through the SQL connection*
and contradicts the earlier claim that CPU, memory and I/O always need
CloudWatch. **Unverified.** A probe first, then its own stage if it holds.

**Elasticsearch.** §2. A transport change, not a feature.

**Background sampling.** db-query is not a daemon and this stage does not make
it one. A baseline is taken when asked.

**Verifying a proposal.** §4.8. MySQL cannot price a plan for an index that
does not exist, so neither can we.

**`EXPLAIN` on a sample statement.** Tempting, and `QUERY_SAMPLE_TEXT` would
make it possible on 8.0 — but it runs a real statement against a real server
from a panel, which deserves more thought than a line in this stage. Candidate
for a follow-up.


## 8. What the server said, and what is still open

**Where the panel's own SQL lives: `workload.rs`, and now only there.**
`readings()`, `mysql_shapes()`, `mysql_index_scan()` and the inventory's three
reads. A transcription of all of them sat beside this file as
`stage-31-probe.sql` while the stage was open, so that a feature which queries
somebody's production server could be read as SQL by the person whose server it
is. It is deleted: a second copy of four queries is a second thing to keep
true, and §6.13 is this stage's own record of what happens when a copy and
the build disagree. The queries are 40 lines of `workload.rs` with the aliases
on them, which is the copy that ships.

**Where the numbers came from.** Two rounds, both run through the batch runner
as folders of one-`SELECT` scripts, against UAT and QA:
**2026-11-05** closed the stage's one honest gap -- the three
`performance_schema` reads execute, every column exists, and the 8.0 branch of
the version gate is the one taken (§8.1) -- and **2026-11-06** answered what
was left, and found three defects while it was there (§8.2, §8.2.1). Those
scripts and their CSVs are gone now that they have been read. Every figure they
produced is written out below, because a CSV of somebody's schema and query
shapes is not a thing to keep in git.

### 8.1 Answered

1. **Why does QA return nothing?** Because `@@performance_schema` is **0** --
   the engine is off, not misconfigured. The whole ladder reads zero on QA:
   no statement instruments, no `statements_digest` consumer row at all, no
   digests, on an instance up for 178 days. On Aurora that parameter is not
   dynamic, so switching it on is a parameter-group change **and an instance
   restart**. QA therefore cannot host this feature as it stands, and the
   panel now says so on rung 1 instead of showing an empty list. The box
   development happens against is not the box this can be developed against.

2. **The full `Supplier` digest, and it changes §4.2's verdict.** Nothing was
   truncated: the text is 288 bytes, well inside
   `performance_schema_max_digest_length`. What the earlier probe cut off is
   the part that matters:

   ```sql
   SELECT SupplierId, default_rate, include_in_plan, import_on_sync
   FROM Supplier
   WHERE tenant_sub_id = ?
     AND ( NAME = ?
        OR JSON_CONTAINS(LOWER(COALESCE(ALIAS, ?)), JSON_QUOTE(LOWER(?))) )
   ```

   **No conventional index can serve this**, so `missingIndex` — the verdict
   this panel puts on the most expensive shape on the server — would have sent
   somebody to build an index that does not help. The `OR` pairs an indexable
   predicate with `JSON_CONTAINS` over a `LOWER(COALESCE(...))`-wrapped JSON
   column. MySQL cannot index that expression, and it cannot index-merge an
   `OR` whose other branch is not sargable, so it scans. The counters were
   saying exactly that all along: 2,599 rows read per row returned and
   1,766,585 executions that found no index.

   The same read showed three `UPDATE Supplier` shapes at ~41M executions
   each, one for one with the 41M `SELECT`: a read-modify-write over every
   supplier, and the origin of the 81,333,004 writes the panel prices indexes
   against.

   **This was a real gap in `verdict_of`**, and §6.10 is how it was closed: a
   `PredicateNotIndexable` rung placed inside the `MissingIndex` branch, so it
   can only ever change the verdict it was added to change. The fixture
   carried the truncated statement too, which is why two passing tests had
   been asserting the right answer about a predicate that does not exist.

4. **Is `Performance_schema_digest_lost` still zero on UAT?** Yes — zero on
   both UAT and QA. §4.1 holds: the list is a census of the ring, not a
   sample of it, and the scope line is telling the truth.

### 8.2 Answered by the batch probe, 2026-11-06

Seven one-`SELECT` scripts, run against **both** servers through the batch
runner. All seven produced output on both, which alone settles question 6. The
scripts and the CSVs have since been deleted; the numbers are here because
this is where they belonged all along.

8. **Can `information_schema.STATISTICS` ever answer here? Yes, easily.**
   1,319 index key parts over 368 tables on UAT, 1,361 over 375 on QA, in a
   run nobody had to wait for. And there was never a structural reason for it
   to be slow: `information_schema_stats_expiry` is **86400**, not 0, and
   `innodb_stats_on_metadata` is **OFF**, so the read uses cached index
   statistics and never reopens a table to refresh cardinality.

   So the 5 Nov hang was **not** a property of the query or the schema. It was
   transient -- a lock, or the shared `meta` connection being in the middle of
   something -- which is exactly the case §6.9.1's deadline exists for. The
   inventory is viable on this instance; the deadline stays as the guard that
   stops a bad minute taking the connection with it.

9. **Are the RDS metric views readable? They are not there.** `information_
   schema` holds no table whose name begins with `rds` on either server: zero
   rows from the catalogue read, on both. Not a privilege wall, not a
   reader-versus-writer difference, not slow -- absent. A host-metrics strip
   is out of scope for good rather than parked, and that is a better outcome
   than the two hangs suggested.

10. **The `Supplier` verdict is confirmed, and for a sharper reason than
    §6.10 gave.** UAT carries
    `idx_Supplier_tenant_master (tenant_sub_id, supplier_master_id)`.
    The indexable half of that predicate is **already indexed**, and 39.3 M of
    its 41.1 M executions did find an index to use. So `missingIndex` was
    wrong in the worst available way -- the index it implied already exists --
    and `predicateNotIndexable` is right: what remains is entirely the
    unindexable `OR` branch, and only the caller can change that.

11. **The redundancy rule found something real, on its first look at a
    production schema.** QA carries `idx_supplier_owner_sub_id
    (tenant_sub_id)`, a strict prefix of the two-column index above.
    UAT does not. Across the whole schema: 55 leading-prefix redundant
    indexes on UAT, 62 on QA. The two environments having drifted apart on
    `Supplier` is its own finding, and not this stage's to fix.

12. **The two index sources agree exactly.** `STATISTICS` and the InnoDB
    dictionary returned the same row count on both servers and the same set of
    key parts, with two encoding differences and no disagreements: the
    dictionary names a functional key part `!hidden!<index>!<n>!<m>` where
    `STATISTICS` reports `NULL`, and it encodes a dot in a table name as
    `@002e` -- which this schema has, in a table called
    `appdata.ComplianceLog` inside `appdata`. So the dictionary is a
    usable fallback source if one is ever needed, at the cost of decoding
    those two things.

### 8.2.1 And the two reads that had never run, 2026-11-06

Round 1 covered the digest read, the diagnosis ladder and the index
catalogue. These are the remaining two of the panel's five queries, so after
them every statement the feature issues has run against a real instance.

13. **`COUNT_READ AS reads` was the cause**, established rather than
    probable. The same statement with the alias backticked returned
    **1,025 rows on UAT** -- exactly the 1,025 indexes round 1 had counted --
    where unquoted it was rejected. So the "never read" finding had never
    worked on any server: §6.13 is not a hardening, it is a repair.

14. **Index usage agrees with the catalogue exactly.** 1,025 rows, and the
    set of `(db, table, index)` matches the catalogue in both directions with
    nothing missing or extra. No empty `db`, `tbl` or `idx` in any row, and
    every `reads` a plain integer -- so `mysql_index_use`'s `filter_map` with
    `.ok()?`, which drops a whole row on any `NULL` in those three, has
    nothing to drop here. That is a measurement, not a guarantee: the shape of
    §6.12's bug is still in that function.

15. **377 indexes have never been read**, of which 132 are `PRIMARY` and
    skipped by the rule, leaving **245 that the panel would propose dropping**
    -- roughly a quarter of every index on the instance. The counter is
    cumulative since the server started, so the claim is "not read in the 54
    days these counters cover", which is what the panel's own wording says and
    is the only claim the data supports.

    QA returned nothing at all, because `performance_schema` is off there.
    That is the "this connection cannot tell" case rather than "no index goes
    unread", and the two must not read the same -- which is what
    `usage_available` and now `usage_refusal` are for.

16. **No foreign key on either server lacks a supporting index.** 428
    constraints on UAT, 434 on QA, and zero unsupported on both --
    which is exactly what `unindexed_foreign_keys`' own doc comment predicts,
    because InnoDB creates an index for a key it defines itself. An empty list
    is a result: the finding exists for the case a later `ALTER` leaves
    behind, and this schema does not have one. No empty `COLUMN_NAME` either,
    so that function's assumption holds -- the same assumption §6.12
    disproved for `STATISTICS`.

### 8.3 Still open

Questions 3 and 6 are answered above. What is left is either recorded for its
own sake or belongs to another part of the app.

5. **How much of this is one stage?** Kept for the record: phases 1-3 were a
   complete feature on their own, and phases 4-5 were what made it
   trustworthy rather than merely informative. It shipped as one.

7. **A statement's label in the results list is its leading trivia.** Running
   a heavily commented file reported its eighth statement as
   `-- 2b. The pre-8.0 form, for when the column above is rejected...`,
   because two kilobytes of comment sat between the previous `;` and the
   `SELECT`. The splitter is right -- `has_code` drops a comment-only span and
   the boundaries were all correct -- but the list shows the span, so a
   commented file is unreadable while it runs. The fix belongs to the results
   list, not here. It is why every probe script kept its prose in one block
   at the top rather than above each statement.

13. **An empty result set exports as a zero-byte CSV with no header.**
    `04-rds-catalog.csv` is 0 bytes on both servers, because
    `stream_rows_to_file` derives the header from the first row and there was
    no first row. A file with nothing in it is then indistinguishable from one
    that was never written or whose statement failed -- and here the empty
    result *was* the finding. The batch runner's own report does say the
    statement ran, so nothing is lost, but the CSV cannot be read on its own.
    Belongs to `sqlfilesrun`, not here.

14. **The two environments have drifted on `Supplier`.** QA carries a
    redundant single-column index that UAT does not (§8.2.11), and UAT has
    ten fewer tables and one more view than QA (368/23 against 378/22). That
    is a schema-management question, and the panel's job here was only to
    notice.

16. **A `ResizeObserver` loop notice on a narrow window.** Expanding a dock
    with a second dock open makes WebKit report
    `ResizeObserver loop completed with undelivered notifications`, because
    both windowed lists repaint inside a callback watching the size that
    repaint changes. Benign -- the browser finishes on the next frame -- and
    absent on a wide window. §6.15 records what was tried and reverted. The
    real fix is for whichever observer is actually the participant, and
    finding that out costs more than the symptom does.

15. **Whether a prelude belongs in the batch runner.** The probe folder
    worked by putting `SET SESSION max_execution_time` and `lock_wait_timeout`
    in every file, which is honest but repetitive -- and a prelude file that
    only works when it sorts first is a dependency nobody can see. A
    `_prelude.sql` run once before the first file, named in the plan so the
    confirmation shows it and refused if it returns rows, would serve `USE`,
    `sql_mode` and `SET ROLE` as well. Proposed, not built.
