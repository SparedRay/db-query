# Stage 29 — Results, and Sequence beside it

**Status:** ✅ Done — built and verified 2026-10-27

---

## 1. Where this came from

Asked for in the same request as Stage 28 and deferred until that one had been
used: *"I will start with option 2b for now. Lets work on that and see with
usage how it work for us"*. **The verbatim request for this half is not in the
repo** — it was made in the session that designed both, and only the reference
in Stage 28 §8 survives here. The design of record is the canvas, where it was
drawn and agreed:

<https://claude.ai/artifact/SxTpzgHeqwcMAixySnfBvd>

* `Main` — *"Option 1: … · output with Results / Sequence"*, interactive: the
  header, both views, the entry rows, the filters, the footers.
* `States` — *"Results / Sequence: edge cases"*: a run with no result sets, and
  the shut strip in three states.

Two of that canvas's four `States` columns are **superseded by Stage 28** and
are not built: "Migrations panel sizes" drew Option 1's sidebar, which lost to
2b; and "Flyway runs in Sequence" assumed a migrations tab with no result sets,
and there is no migrations tab any more.

## 2. What is there

| Piece | Where | What this stage does with it |
| --- | --- | --- |
| The pane header | `#res-toggle`, one `<button>` spanning the row (Stage 26) | Gains the view switch — and stops being one button, see §5.6 |
| Result sets | `#tabs` + `#grid` + the result bar, driven by `tab.result` and `tab.activeResultIndex` (Stage 12) | Unchanged. This is the Results view |
| `ScriptResult` | `tab.result`, the whole thing kept per tab | Already carries every field Sequence draws |
| `revealStatement` | `main.ts`, from a result back to its statement in the editor | Reused by a Sequence row |
| `closeStatement` | splices a statement out of `tab.result.statements` to free its rows | **The reason the log cannot be a view of `tab.result`** — see §5.1 |

`ScriptResult` is the find. Per statement it already has `sql`, `effectiveSql`,
`kind` (`select` / `rowReturning` / `modify` / `session` / `other`), `outcome`
(`rows` / `affected` / `error`), `elapsedMs`, and `start`/`end` byte offsets
into the buffer; the run has `totalElapsedMs`, `abortedAt`, `cancelled`,
`timedOut` and `reconnected`. **Nothing in the backend changes for this stage.**

## 3. What was measured

A Sequence entry without its rows, and a plain list of them. Chromium, the real
app page, a 900px-wide container, build plus forced layout:

| Entries | Records, as JSON | Build + layout | List height |
| --- | --- | --- | --- |
| 200 | 37 KB | 10 ms | 5,900 px |
| 1,000 | 186 KB | 50 ms | 29,500 px |
| 5,000 | 937 KB | 320 ms | 147,500 px |
| 20,000 | 3.7 MB | 1,277 ms | 590,000 px |

Linear both ways: **~190 bytes and ~64 µs per entry**, ~29.5px tall each.

Two things follow. **The records are cheap** — 1,000 statements cost 186 KB,
less than one 1,000-row result set, which is the memory `clearResult` exists to
release. So a log across runs is affordable in a way keeping their rows would
not be. And **a plain list has a real cliff**: 50 ms at a thousand entries is a
visible hitch on a view switch, 320 ms at five thousand is not acceptable. The
grid has been virtualised since Stage 12 for exactly this reason, and Sequence
rows are uniform height, so the same treatment applies.

## 4. The design

```
rail │ SCHEMA        │ [ship-orders.sql] [audit.sql]                 ┆ MIGRATIONS
  ●  │ ▾ maindb      │ Run  Run all  Format                         ┆
     │   ▸ Tables    │ SET @cutoff = '2026-10-01';                   ┆
     │               │ SELECT id, customer FROM orders …             ┆
     │               │ ── ▾ Output ─────── [Results 2│Sequence 6] ── Ctrl+J
     │               │  Run all  14:02:11  · 5 statements · stopped at 5 · 64 ms
     │               │   ● SELECT  SELECT id, customer, status …  3 rows → Result 1   8 ms
     │               │   ■ UPDATE  UPDATE orders SET status = …   2 rows affected    12 ms
     │               │   ◆ DELETE  DELETE FROM cart WHERE ord_…  Error 1054          30 ms
     │               │  6 statements · 3 changed 3 rows · 1 error   Copy as log  Clear
```

* **Two views of one pane**, chosen by a segmented control in the header that
  Stage 26 built. Results is what is there today, untouched. Sequence is what
  ran: one line per statement, grouped under the run that issued it.
* **Each view carries its count** on the control — result sets, and statements.
* **A statement's line** says its kind as a shape and a verb, the SQL, what came
  back, and how long it took. `3 rows → Result 1` is a link to that result set;
  clicking the SQL reveals the statement in the editor, as a result set's tab
  already does.
* **Filters**: All, Changes, Errors — the two questions worth asking a log of
  statements, and neither is answerable by reading the grid.
* **The footer counts** what the run did, and offers the log as text.

## 5. Decisions

1. **Sequence reads its own log, not `tab.result`.** `closeStatement` splices
   the array to free a result's rows, so a view of `tab.result.statements`
   would lose its own history whenever somebody closed a result — and would
   only ever hold the newest run, where the canvas draws two and says "this
   tab, since it opened". The log holds records without rows (§3: ~190 bytes
   each); the rows stay in `tab.result`, where Results already has them.
2. **The log is per tab, and it is not persisted.** Same policy as every pane
   and splitter here (Stages 12, 21, 26, 28), and the session has never written
   a result down (Stage 7, S6) — a log of statements is the same category of
   thing.
3. **Sequence is SQL only.** Flyway's Apply and Repair stay in the migrations
   panel's "Last run", which is where Stage 28 decision 4 put them. The canvas
   drew them as Sequence entries, but it was drawn before that panel existed,
   and one run recorded in two places is the confusion Stage 18 §5.3 and Stage
   28 decision 1 both removed. *The user's call, asked and answered.*
4. **The view is chosen per tab**, like the result-set index and unlike Ctrl+J's
   shut state. A tab whose run returned no rows is worth leaving in Sequence
   while the tab beside it shows its grid, and switching tabs should not change
   what you were reading. *The user's call, asked and answered.*
5. **Nothing switches view on its own.** A run with no result sets leaves
   Results in front, saying so and offering a button to Sequence — the canvas's
   `States` column 1. A view that moved under you would make the count on the
   control unreadable as a fact.
6. **The header stops being one button.** Stage 26 made the whole strip the
   toggle, because "the whole strip is the target, which is what makes it a
   habit" — and a `<button>` cannot contain the two buttons of a segmented
   control. The toggle keeps the caret, the title and the shut summary and
   takes the row's spare width; the control, the summary and `Ctrl+J` sit to
   its right. The target shrinks; Ctrl+J and the seam (Stage 26) are the other
   two ways, and both already work.
7. **Sequence is virtualised, and the log is capped.** §3's cliff is real and
   the rows are uniform, so it is painted the way the grid has been since Stage
   12. The cap is on the records, so that a pasted thousand-statement script
   cannot grow without bound; the oldest run goes first, and the footer says
   when something was dropped rather than quietly lying about the count.

Decisions 8–12 were settled while slice 3 was built, and are recorded with it
in §9.1 rather than back-filled here.

## 6. Milestones

| # | Claim | Evidence |
|---|---|---|
| Q1 | The header offers both views, with a count on each, and switches between them | UI test |
| Q2 | The chosen view is remembered per tab, across a tab switch | UI test |
| Q3 | Results is unchanged: its tabs, grid, result bar and every Stage 12 behaviour | Stage 12's tests, untouched |
| Q4 | Sequence lists every statement of a run, grouped by run, with kind, SQL, outcome and elapsed | UI test |
| Q5 | A statement that made a result set links to it; its SQL reveals it in the editor | UI test |
| Q6 | Closing a result set does not remove its Sequence entry | UI test |
| Q7 | All / Changes / Errors filter the list, and the footer counts the run | UI test |
| Q8 | A run with no result sets leaves Results in front and says so | UI test |
| Q9 | Shut, the strip's summary follows the view in front | UI test |
| Q10 | A log at the cap paints without the §3 cliff, and says what it dropped | UI test, and a measurement |
| Q11 | Flyway is not in Sequence, and the migrations panel still has its Last run | UI test |
| Q12 | Hands-on, in use | The user |

## 7. Task tracker

### Built 2026-10-27
- [x] Slice 1 — the header: the segmented control, the per-tab view, the
      counts, the shut summary (Q1, Q2, Q3, Q9)
- [x] Slice 2 — the log: recording a run, the entry rows, the result link, the
      reveal, surviving `closeStatement` (Q4, Q5, Q6)

- [x] Slice 3 — the filters, the footer, Copy as log, Clear (Q7)

- [x] Slice 4 — the edge cases: no result sets, the cap, the windowed painting
      (Q8, Q10)
- [x] Flyway stays where it is (Q11)

- [x] Hands-on (Q12) — *"All working as expected"*

## 8. Built — 2026-10-27 (slices 1 and 2)

`src/sequence.ts` is new: the records, the mapping off a `ScriptResult`, the
cap, and the painted list. `main.ts` routes the two surfaces and writes the log
in the one place a result lands. The backend did not change, as §2 said it
would not.

### 8.1 What changed shape against the canvas

* **The pane is "Output", not "Results".** Results is one of two views inside
  it now, so a header that called the whole thing Results would be naming the
  part after the whole. Two migrations tests asserted that title and say
  "Output" — which is still what they were there to prove: the pane is not
  Flyway's (Stage 28, D6).
* **Result numbers are the app's, not the canvas's.** The mock-up numbered
  result *sets* among themselves — the first and third statements as "Result 1"
  and "Result 2" — but `renderTabs` has numbered them by statement since Stage
  12, so the third is `Result 3`. A Sequence row saying "Result 2" while the
  strip above it said "Result 3" would be a confident lie, so the app wins and
  the test pins it.
* **Four run labels, not two.** The canvas drew `Run` and `Run all`; the log
  also has to tell a selection from a statement and the assistant from a
  person, so `run()` takes what it was called by: `Run`, `Run selection`,
  `Run all`, `Assistant`.
* **The link is resolved at render, never stored.** `closeStatement` splices
  the statements, which renumbers every result tab after the one that went — a
  stored index would start pointing at somebody else's rows. `resultIndexOf`
  looks the statement up by its offsets (not its text: the row's SQL is
  collapsed to one line, so it would never match), and **only for the newest
  run**, because the run before it had its result replaced. Both halves are
  falsified by their own test; without the second one, two runs of the same
  script give the older run links into the newer run's results, which would
  look right.
* **The reveal failure goes to a different place in each view.** A result tab
  keeps Stage 12's behaviour exactly — the whole statement printed into the
  grid, which is the only surface here with room for it — and a Sequence row
  uses the note under both views, because its grid is not on screen. Found by
  the suite: moving both to the note broke two Stage 12 tests, which is Q3
  doing its job.
* **The verb is read off the SQL**, past any leading comment, because `kind` is
  a category: `modify` covers the UPDATE and the DELETE, and those are two
  different things to go looking for in a log. The category is the fallback
  when the text will not give a word.

### 8.2 Tests

Ten in `tests/ui/sequence.spec.ts`, **each claim falsified on its own** and
each failure landing on its own assertion: the verb taken from the category
instead of the SQL; any run allowed to link, not only the newest; results
numbered among themselves as the canvas drew them; the log row spliced out with
its result; the shut summary always reading the status chips; a view switch
reopening a shut pane; one view for the whole window instead of per tab; and
the grid left on screen under Sequence.

**`tsc` caught what the suite could not.** `.at(-1)` in a test passed every
Playwright run — the runner transpiles without typechecking — and failed
`mise run check`, which typechecks the tests against this project's ES2021
target. Recorded because it is a whole class of mistake the green suite says
nothing about.

**570 UI tests on Chromium and 569 on WebKit, none failing** (one skipped on
WebKit); `mise run check` exit 0.

## 9. Built — 2026-10-27 (slice 3)

The filter above the list, the footer under it, and the two buttons the canvas
drew on it. `#sequence` became three rows — filter, list, footer — of which only
the middle one scrolls: a filter that scrolled away would be unreachable from
the bottom of a long log, which is exactly where you would be when you wanted
it.

### 9.1 Decisions this slice had to make

8. **The chips count the whole log; the filter says what it hid.** They are the
   log's summary the way the result bar is a result set's, and a filter changes
   what you look *through*, not what the tab did. Counting the filtered set
   instead would make "1 error" under Errors the same figure twice, with
   nothing left saying what actually ran. How much is hidden is said once,
   beside the control that hid it: `Showing 1 of 4`.
9. **A run the filter empties loses its header.** A heading over a gap says
   nothing, and the chips below still count the run it belonged to.
10. **Copy follows the filter.** Copying what is not on screen is a surprise,
    and the header already says how much that is. Tab-separated, like the
    grid's own copy: it reads in a terminal and pastes into a spreadsheet.
11. **Clear is about the log.** The rows in the grid were asked for separately
    and "Close results" is how you say you are done with those. It also puts
    the filter back, because there is nothing left to be filtering.
12. **One segmented control, two uses.** The view switch and the filter are the
    same shape of choice, so `.res-views`/`.res-view` became `.seg`/`.seg-btn`
    and both wear it — a second look would be a second thing to learn.

### 9.2 What the tests found

**A falsification that passed, and the fourth of its class here.** "The filter
belongs to the tab" set a filter, opened a second tab and came back — and
passed with the filter made one setting for the whole window, because both tabs
had ended up holding the same value. One tab cannot tell per-tab from global.
It now gives the second tab a log of its own and leaves it on All, so **two
tabs are filtered differently at the same time**, and the mutation fails it.
Same lesson as Stages 27 and 28: an assertion true for a reason other than the
one claimed.

**A real defect, found by its own test.** The filter buttons were painted only
inside `paintSequenceFoot`, which the empty-log path returns before reaching —
so a Clear left "Errors" pressed in a row that had just been hidden, and the
next run inherited a control disagreeing with the tab until something else
repainted it. The `aria-pressed` pass now runs whether or not the row is on
screen: a hidden control whose state is wrong is a control that is wrong the
moment it is shown.

**Two test mistakes worth the note.** `statement_at_cursor` was asked about an
empty buffer, because the test ran before typing anything; and `#grid tbody tr`
counted the grid's **spacer** rows as well as its data rows, which is what
windowing means — `tr[data-row]` is the real count.

**Nine more tests, each claim falsified on its own**: the chips counting the
filtered view; an emptied run keeping its header; Copy ignoring the filter;
Clear discarding the results too; one filter for the window; and a hidden
filter row left saying what it last said.

**579 UI tests on Chromium and 578 on WebKit, none failing** (one skipped on
WebKit); `mise run check` exit 0.

## 10. Built — 2026-10-27 (slice 4)

The canvas's last column, the cap, and the two things that had never been
tested.

### 10.1 Decisions

13. **A run that returned no rows says so, beside the grid's own message**, and
    offers a button through to Sequence. Results stays in front: a view that
    switched itself would make the count on the header unreadable as a fact,
    and would take the pane away from somebody who ran a migration script on
    purpose and wants to watch it finish.
14. **The question is asked of the log, not of `tab.result`.** The live array
    is what `closeStatement` splices, so a view asking it would announce "no
    result sets in this run" about a run whose result set you had just closed
    yourself. The log remembers what the run did.
15. **The dropped notice is a footer chip**, which is what decision 7 already
    said and slice 3 did not do: it was built as the last row of the list,
    where at the cap it sits two thousand rows below anyone who might want it.

### 10.2 What the tests found

**A falsification that passed — and it found dead code encoding the wrong
question.** Removing `grid.ts`'s "did this run return rows" guard changed
nothing, because `emptyRunHint` in `main.ts` had already answered the same
question before composing the text. Two gates, one effective. Worse, the
redundant one read `this.result` — the array `closeStatement` splices — so had
it ever been the deciding one it would have called a run resultless because its
rows had been closed. The guard is gone, the gate is the caller's, and a new
test closes the only result of a run and asserts no hint appears.

**Q3 caught it again.** The hint went *inside* the grid's message box, and two
Stage 12 tests failed on exact text: "Statement executed." had become
"Statement executed.No result sets in this run — 1 statement.Show in Sequence".
It is a sibling now. That is twice this stage that "Results is unchanged" has
stopped a change from leaking into it — the first was the reveal failure
message — which is the whole reason that milestone names Stage 12's tests as
its evidence rather than a promise.

**Five more falsifications, each biting its own assertion**: the hint judged
per statement rather than per run; a no-rows run switching view on its own; the
cap trimming the newest run instead of dropping an older one whole; nothing
saying what the cap took; and the list truncated rather than windowed — that
last one is why the windowing test scrolls to the end and looks for statement
1,099, since a view painting the first forty rows and discarding the rest would
pass every other assertion.

**The cap, measured in use.** Two runs of 1,100 statements against a cap of
2,000: the older run goes entire, the footer says "1 earlier run dropped (1100
statements)", and the list paints fewer than 150 rows for a 1,100-entry log
while its scrollbar still describes all of it.

**585 UI tests on Chromium and 584 on WebKit, none failing** (one skipped on
WebKit); `mise run check` exit 0. The backend never changed, as §2 said.

## 11. Where it stands

Committed on `main`, after the hands-on. The version is not bumped: that is a
release decision, and the README's rule puts this at `minor` — a feature
someone would notice — whenever it is cut.

Three things left to settle with use rather than by a test:

* **Where the view switch sits.** Keeping Stage 26's wide click target pushed
  it right of the title, where the canvas drew it tucked against it.
* **Whether `Run selection` and `Assistant` are labels worth having**, or
  whether every run should just read `Run` and `Run all` as the canvas had it.
* **Whether a cap of 2,000 is the right number.** It is a measured guess at
  "more history than anybody scrolls" and nothing has pushed on it yet.
