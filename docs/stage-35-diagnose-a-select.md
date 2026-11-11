# Stage 35 — diagnose a SELECT

**Status:** ✅ Built — 2026-11-11. Designed on a canvas first, reviewed, then
built. Hands-on confirmation against a real slow statement outstanding.

The canvas: https://claude.ai/artifact/TRj1fHAY4Sgmr5sn1hTEXv

---

## 1. The ask

> *"Now I need a way to properly measure and analyze a SELECT statement. The
> goal here is to have a simple UI toggle way to Run/Run all but when set this
> will do a performance diagnosis instead of only the execution. With this my
> plan is to have a proper explain where visually we can see what happens and
> hopefully will help understand what kind of change is needed."*
>
> *"This will be used mainly on SELECT statements where we know they are slow
> compared with others and we are trying to see either the effects of an index
> or if there's a way around. Ideally this result we will have a quick way to
> send to the assistant so [it] can review and suggest."*

Stage 31 built a panel that answers **which statements cost the server its
time**. It cannot answer the next question, which is always **why this one and
what would fix it**. The dock ranks a workload; this stage measures one
statement.

## 2. What the server will actually tell us

Checked against the MySQL 8.0/8.4 manual and the Aurora user guide on
2026-11-11, not from memory. UAT is Aurora MySQL 3, which is 8.0-compatible.

| # | Source | Gives | Costs | Since |
|---|---|---|---|---|
| 1 | `EXPLAIN FORMAT=JSON` | The intended plan with the cost model's own numbers | Nothing — no execution | 5.6 |
| 2 | `EXPLAIN FORMAT=TREE` | The same plan as an iterator tree, in execution order; the only format that shows hash joins | Nothing — no execution | 8.0.16 |
| 3 | `EXPLAIN ANALYZE` | The tree with **actual** time, rows and loops per iterator | **Executes the statement** | 8.0.18 |
| 4 | Session counters | Rows read, temp tables, sort passes, `NO_INDEX_USED` | Needs the run; no privilege | any |
| 5 | Optimizer trace | *Why* an index was not chosen | Session setting, verbose | 5.6 |

Three facts shaped everything below.

**`EXPLAIN ANALYZE` executes.** The manual is explicit, and it accepts
multi-table `UPDATE` and `DELETE` as well as `SELECT`. A diagnose toggle that
runs a `DELETE` because the user was "only diagnosing" is the worst bug this
app could ship, so the mode refuses anything that is not row-returning and
offers the free plan instead.

**Its times are per loop, and cumulative.** `actual time=0.05..268` is the
average first-row and last-row time *of one iteration*, and it includes the
node's children. A node's total is `last × loops`; its own cost is that minus
its children's totals. Reading the reported numbers as self time makes the root
look like the problem every single time.

**The plan tree carries no counters.** Rows examined, temp tables on disk and
sort merge passes are not in it. They come from a `SHOW SESSION STATUS` delta
either side of the run — which is attributable here because every tab holds its
own connection.

## 3. Decisions

Five questions went to the user on the canvas. All five were answered with the
recommendation.

**D1 — Plan *and* measurement.** The plan is free, so it is fetched first and
fills the view while the measured run happens. It is also the whole answer on a
server older than 8.0.18, with no special case: the measurement is simply
absent and the view says so.

**D2 — A third output view**, beside Results and Sequence, not a new dock. A
diagnosis belongs to the statement you just ran, and that is where the
statement's answer already appears. The dock is for the server; this is for one
query.

**D3 — The assistant gets the statement normalised.** The performance dock is
safe by construction: a digest holds no literal values. A statement being
diagnosed is the opposite — it is real SQL with real values, and the assistant
is an external API. Literals become `?`, comments are dropped, the evidence
survives. *Send as written* is offered on the spot, never defaulted, because
sometimes the value is the question.

**D4 — The index what-if is in, as generated SQL only.** `CREATE INDEX …
INVISIBLE` goes into the editor and the person runs it, as with every other
statement this app generates. What the feature contributes is the measurement
either side of it.

**D5 — Optimizer trace is deferred.** Sources 1–4 answer *what happened*
completely. The trace answers *why the optimizer chose it*, which is a smaller
question and a much larger pile of JSON.

## 4. Milestones

- [x] **D1 — The toggle changes the button.** With Diagnose on, Run reads
      *Diagnose* and Run all reads *Diagnose all*, in the warning colour. The
      mode is never invisible on the control that acts.
- [x] **D2 — Auto-LIMIT is suspended, visibly.** The checkbox is disabled with
      the reason on hover while the mode is on, and the rewrite does not
      happen. Diagnosing a query the app silently rewrote measures a query
      nobody runs.
- [x] **D3 — A SELECT is diagnosed.** Plan, measured plan, counters and
      verdicts, in the Diagnosis view.
- [x] **D4 — A write is refused.** `UPDATE`, `DELETE`, DDL: refused by
      capability, with the plan still offered. Nothing executes.
- [x] **D5 — Self time is self time.** Children subtracted, loops multiplied.
      A worked example is pinned as a test.
- [x] **D6 — The verdicts refuse what they cannot support.** No index is
      proposed for a statement that returns everything it reads.
- [x] **D7 — The assistant gets evidence, normalised.** Literals replaced,
      verdict named, counters attached; *as written* available and never
      default.
- [x] **D8 — An old server degrades.** Below 8.0.18 the plan arrives and the
      measurement is absent with a reason. Below 8.0.16 the classic tabular
      `EXPLAIN` is shown instead of a tree.
- [x] **D9 — Elasticsearch never offers it.** A capability flag, like
      `workload_digests`.
- [x] **D10 — Nothing regressed.** Both engines, both suites, `check` clean.

## 5. Task tracker

### Phase 1 — Rust
- [x] `diagnose.rs`: the tree parser (`EXPLAIN FORMAT=TREE` and `EXPLAIN
      ANALYZE` share one grammar)
- [x] Self time: `last × loops`, children subtracted
- [x] Counters: `SHOW SESSION STATUS` delta over a whitelist
- [x] Verdicts: a pure function over (plan, counters)
- [x] Index candidate: equality columns from the filter, order column from the
      sort — conservative, omitted when it cannot be read
- [x] `normalise`: literals to `?`, comments dropped, offsets from
      `split::mask_noncode`
- [x] `Capabilities.diagnose`, false for Elasticsearch
- [x] `diagnose_statement` command

### Phase 2 — The app
- [x] The toggle, the button labels, Auto-LIMIT suspended
- [x] The Diagnosis view: plan tree, self-time bars, counters, verdicts
- [x] Ask the assistant, normalised, with *as written* beside it

### Phase 3 — Proving it
- [x] Rust: parser, self time, verdicts, normalise
- [x] UI: the toggle, the refusal, the view, the handoff

## 6. Risks

**The parser is reading a human-readable format.** `EXPLAIN ANALYZE` output is
documentation, not an interface, and it changes between versions. The mitigation
is that a line it cannot read becomes a node with its text and no figures,
rather than an error — a diagnosis with one unparsed line is still useful, and
a diagnosis that refuses to open because of one is not.

**The measured run costs what the query costs.** There is no measuring a
40-second query in under 40 seconds. The plan arrives first so there is
something to read, and the existing cancel applies.

**An index suggestion can be wrong.** It is generated from the predicate text,
which is the optimizer's rendering rather than the user's SQL. It goes into the
editor as a proposal, never runs, and the verdict says what evidence it rests
on.

## 7. What the tests pin

**Rust, 26 of them, all pure.** The gate (`may_diagnose`) is asserted against
every write shape the manual says `EXPLAIN ANALYZE` accepts, because that is the
one failure that would be unforgivable. The parser is fed the server's own
output, including a line it cannot read, which must become a node rather than an
error. Self time is pinned on the worked example: a lookup printed at 0.003 ms
is the largest single cost in the statement once its 118,944 loops are counted,
and a test that did not multiply would have let the view blame the root.

**Normalisation has four tests** rather than one, because it is what keeps data
off the network: a literal goes, a backticked identifier stays, a digit inside
`t1` is not a literal, and a quote inside a comment does not open one.

**UI, 11 tests.** The mode is on the button that acts; the shortcut hints
survive rebuilding that button's label; Auto-LIMIT is disabled with its reason
on it; the view shows plan, counters and verdict and hides the grid; the bars
are to scale; a refusal is shown and counted rather than swallowed; a suggestion
reaches the editor and nothing runs; the assistant gets `?` and not `4821`; and
an engine without the capability is never offered the mode.

**Falsified rather than assumed.** Four of them, against copies of the files:
dropping the label change fails the mode test; sending `statement` instead of
`normalised` fails the handoff test; and removing either half of the capability
gate fails both capability tests.

## 8. Still open

**No hands-on run against a real slow statement.** Everything here is measured
against fixtures and the parser against the manual's own output. The first real
`EXPLAIN ANALYZE` on UAT is where the parser's tolerance gets tested properly,
and where the index suggestion either reads the predicate correctly or declines
to — both are acceptable, and which one happens is worth knowing.

**Diagnose all runs the statements one at a time**, each measured on its own
connection round trip, with no cancel between them beyond the existing
per-statement one. A script of twenty slow SELECTs would be twenty slow runs.
Nothing stops it; it is simply not a shape anyone has wanted yet.

**The index what-if is one-sided.** The app generates `CREATE INDEX …
INVISIBLE` and measures whatever you run next, but nothing ties the two
readings together — you compare them by eye. A before/after pair held in the
view is the obvious next step, and it needs somewhere to keep the first
reading.

**Optimizer trace is deferred** (§3, D5). The only question left unanswered is
*why the optimizer chose this*, and the trace is the only thing that answers it.
