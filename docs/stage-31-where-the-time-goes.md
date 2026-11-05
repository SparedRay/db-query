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

## 8. Open questions

1. **Why does QA return nothing?** Phase 5's ladder is the answer, but running
   it on QA before the stage starts would tell us whether the panel is testable
   on the box development actually happens against. One paste.
2. **The full `Supplier` digest.** The probe cut it mid-`OR`, so the correct
   index columns are still unknown. `SELECT DIGEST_TEXT FROM
   events_statements_summary_by_digest WHERE DIGEST = '…'` settles it, and it
   is the single highest-value piece of information on the server.
3. **Are `rds_metrics_gauge` and `rds_metrics_meta` readable by our user?**
   Decides whether a host-metrics strip is a later stage or nothing.
4. **Is `Performance_schema_digest_lost` still zero on UAT?** It was at the
   time of the probe. If it moves, the ring is overflowing and the whole list
   becomes a sample rather than a census — which would change §4.1.
5. **How much of this is one stage?** Phases 1-3 are a complete, useful feature
   on their own: read the workload, classify it, propose indexes. Phases 4-5
   are the parts that make it trustworthy rather than merely informative.
   Splitting is available; the tracker is ordered so that the split is clean.
