# Stage 33 — charts in the performance dock

**Status:** ✅ Built — 2026-11-07. Designed on a canvas first at the dock's
real width, reviewed, then built. Hands-on confirmation outstanding.

The canvas: https://claude.ai/artifact/7o9vHiaCzTcdejwATtwRZ3

---

## 1. The ask

> *"Noticed our performance is mostly suggestion, wouldn't it worth a chart
> about usage? Or could we build charts about performance as well to make it
> clearer?"*

And, before any code:

> *"If it is possible lets make it on the artifact first so we can review
> before building code"*

So this stage has two halves and they happened in that order: a canvas of six
boards drawn at the dock's real width with every figure marked *measured* or
*illustrative*, and then the five of them that survived.

## 2. What was actually true

**The panel measured everything and drew none of it.** Stage 31 built a dock
that reads `performance_schema`, classifies each shape by how much it reads
against how much it returns, and then says what kind of problem it is. Every
number in it is real. Not one of them was drawn to scale.

The clearest statement of the defect is two rows of the list side by side:

| shape | per row | verdict |
|---|---|---|
| `SELECT … FROM Supplier WHERE …` | **2,599 : 1** | predicate cannot be indexed |
| `SELECT … FROM acl_rule` | **1.0 : 1** | no index will help |

Those two figures are what decide the two verdicts. They are three
hundred-fold apart, and they sat in identical grey 10px chips at the bottom of
their rows, the same size on screen. The panel had already done the
arithmetic and then asked the reader to do it again.

**So the ask is right and the cause is narrower than "no charts".** The panel
was not short of decoration. It was short of *one place where a magnitude is
drawn to scale*, because every claim it makes is a claim about a magnitude:
this shape against the others, rows read against rows returned, the tail
against the mean, writes against reads.

**And one question it cannot answer.** "A chart about usage" read naturally
means usage over time. `events_statements_summary_by_digest` holds cumulative
counters — one running total per shape since the server started or the table
was last truncated. One read is one number per shape, not a series.
`FIRST_SEEN` and `LAST_SEEN` bound a span and say nothing about its shape. §6
is what that costs and what it would take.

## 3. The rule: the form comes from the data's job, and colour comes last

**Three tones, not a hue per series.** Everything drawn here is one thing
against the rest, so the marks are the app's accent plus two neutrals — the
*emphasis* form. A palette of five hues would have spent the only free channel
restating what the bar lengths already say, and would have made the one row
that matters just another colour.

**Colour is semantics here, so it keeps the app's accent.** Stage 32's rule is
that `--conn-colour` marks **identity** — which server am I looking at. The
length of a bar is a measurement *about* that server, which is semantics, and
semantics does not follow the connection. A chart that recoloured on connect
would read as though the measurement belonged to the colour. A test says so.

The card makes the distinction visible in one place: the **switch** above the
plot is a `.seg`, so its pressed tab wears the connection's colour — *which of
three am I looking at* is exactly the identity question Stage 32 gave that
token — while the **marks** below it never do. Two colours from two rules,
six pixels apart, and both of them right.

**Every mark was measured against the surface it sits on**, in both themes,
with the `dataviz` validator rather than by eye:

| tone | dark | light | role |
|---|---|---|---|
| `--chart-mark` | 4.43:1 | 4.78:1 | the row the lead sentence names |
| `--chart-quiet` | 4.83:1 | 6.0:1 | every other row |
| `--chart-faint` | 2.16:1 | 2.40:1 | the third segment of a split |

The two neutrals are far enough apart to be told apart with any colour vision
(ΔE 16.8 dark, 19.8 light, OKLab ×100). `--chart-faint` is under 3:1 by
choice: it is only ever one segment of a bar whose legend names it and prints
its figure, which is the condition that relaxes the floor. The first attempt
put the two neutrals at ΔE 10.5, which the validator rejected and the eye
would not have.

**The axis is chosen per chart and never implied.** Linear where the
*disparity* is the finding; log where the range the verdict turns in would
otherwise be the first two percent of the track. The card prints `log scale`
whenever the axis is one, because a reader who takes a log axis for a linear
one has read the chart backwards.

## 4. What was built

### 4.1 Where the server's time went

A thin segmented bar above the class switch: application, driver, platform.
The honest first fact on a shared instance is what fraction of the counters
are yours at all, and the panel said it in a *subordinate clause of the lead's
second line* — after the finding it qualifies.

Thin and wordless rather than tall with the shares printed on it, because the
legend under it already names each part and gives its figure. The bar's job is
the proportion and nothing else.

A part worth nothing is drawn as nothing, not as the 3px sliver a `min-width`
would give it. "None of this" and "a little of this" must not look the same.

### 4.2 One card, three charts, one switch

Three cards stacked would have been most of a 420px column before the list
began, and the three answer the same question about the same rows — which is
what a switch is for. The card follows the class filter and the sort, so it
always describes the list underneath it.

**Ranked** — the current sort metric, linear. On UAT one shape is twenty-three
times the next one; that disparity *is* the finding, and a log axis would have
evened it out before the reader saw it. The small bars are supposed to look
small. The top five are drawn and everything below them folds into one bar —
never a sixth colour.

**Selectivity** — rows read per row returned, log, against a rule at 1 : 1.
This is the board that answers the complaint. A shape far to the right is
reading its way through a table; a shape *on* the line returns everything it
reads, which is the one case where no index can help, and the two verdicts
stop being assertions. A shape with no ratio — every write — is **left out**
rather than drawn at zero, because zero on this chart is a position that
means something.

**Latency** — average → 95th percentile, log, as a dumbbell. The only chart
here that shows something the dock could not say at all: the list is ranked by
total time, so a shape that runs rarely and is catastrophic when it does never
reaches the top of it. `workload.rs` has always said why it takes the column —
*tail latency matters more than the mean for a shape that is usually fine and
occasionally terrible* — and nothing on screen had ever shown the gap. Ranked
by the gap and not by time, or it draws the same five rows as the first chart
and finds nothing. Absent, not empty, on a server without `QUANTILE_95`.

A dumbbell rather than two bars because the reader's job is the **distance**
between two values of one measure. Two bars of different colours invite a
comparison across rows that nobody needs.

### 4.3 What the indexes cost

A segmented bar above the inventory: read, never read, `PRIMARY`. The list
reports how many findings it has and never what that is a fraction of — on UAT
245 of 1,025, a quarter of every index on the instance, and a panel proposing
to drop a quarter of an instance's indexes should say so before the first
`DROP INDEX` is read.

**`PRIMARY` is a part of its own** rather than folded into "read", because
those are excluded by *the rule* and not by the data: a third category the
list made invisible by not showing it.

**Counted from the usage rows, not the catalogue.** The two reads agreed
exactly on UAT — 1,025 both ways, §8.2.1 — and that is a measurement rather
than a guarantee, so the split is drawn from the rows the never-read rule was
actually applied to. One new nullable struct on `IndexInventory` carries it,
and `None` whenever the usage read did not answer: absent, rather than three
zeroes that would read as *every index on this server has been read*.

### 4.4 The primitives are a module

`charts.ts`. Five boards all draw the same two things — a proportion of a
whole, or a magnitude against a shared scale — and written as five painters
inside `workload.ts` they would have been five copies of the same arithmetic
with five chances to get the denominator wrong.

It formats nothing: every figure arrives already through `duration`, `compact`
or `ratioText`, so a chart cannot print a number differently from the row
beside it. It names no colours: a mark asks for a *tone* and `styles.css`
decides what that is per theme. And it builds from nodes, never `innerHTML` —
half of what is drawn here is labelled with a digest's text.

## 5. What changed between the mockup and the build

**The class switch kept its counts.** The mockup proposed moving it to
seconds so the bar and the figures beside it could not disagree about what
they measure. At 420px three buttons reading `App 96h 40m` wrap onto two
lines, and that row already shares its width with the sort control. The
legend prints its own figures and names them instead, which answers the same
requirement without breaking the control row.

**The three charts became one card with a switch.** They were drawn as
separate boards because that is how a review reads them, not as a layout
proposal. Stacked in the dock they are most of a 420px column.

**Selectivity moved off the row and onto its own chart.** A bar per row on a
shared scale was tempting and is what the mockup's board looked like; it would
have put a chart inside a list item that is already a verdict, a sentence, a
statement and three figures.

## 6. Usage over time, and why there is no chart of it

Three routes, and they are not equivalent.

**A · The baseline already gives two points.** `Set baseline` stores the
counters and every later read subtracts them: *then* and *now*. That is a
delta, and the honest drawing for it is the dumbbell from §4.2 with the two
ends being the two readings. It answers "what changed since I pressed the
button", which is a real question during a deploy. It does not answer "what
does Tuesday look like", and it must never be drawn as a line between two
points, because a line asserts the path between them. Not built; nobody has
asked for it yet.

**B · A distribution, not a series.**
`events_statements_histogram_by_digest` would give a real *shape* per
statement in a single read — bucketed latency counts, no sampling and no
daemon. It is the honest answer to "a chart about performance": not usage over
time, but where a shape's executions actually fell. **Unverified on Aurora**,
and every other claim in Stage 31 was probed before it was built. The probe is
two read-only statements and it belongs to the next stage, not this one.

**C · Sampling. No.** A genuine time series means polling the counters on a
schedule and storing the samples. Stage 31 §7 ruled on it — *"db-query is not
a daemon and this stage does not make it one. A baseline is taken when
asked."* Reversing that is not an increment: it needs a store, a schedule, a
retention policy, and a series that is only as continuous as the app was
open, which on a laptop is a series with nights missing. If usage over
wall-clock time is the real requirement, Stage 31 §7 already names where it
comes from: Aurora's own gauge metrics, also unverified, also a probe first.

## 7. What the tests pin, and what they survived

Eleven UI tests in `workload.spec.ts` and two unit tests in `workload.rs`.

**The marks are asserted, not the pixels.** A bar's `style.width` is the
number the chart was told to draw; its rendered width is that number through a
layout nobody is testing here. One assertion is deliberately the exception —
the bar that must render at zero — because that is the one case where a CSS
rule, not the arithmetic, is what makes the claim true.

**Every colour is resolved, never a literal.** The accent differs between the
two themes and the suite runs light, which is how the first draft of the
colour test passed for the wrong reason and then failed for the right one.

Nine falsifications, each reverting one decision, each failing the test that
claims it:

| broken | fails |
|---|---|
| the ranked chart given a log axis | *the ranked chart draws one shape against the rest, linear* |
| selectivity given a linear axis | *selectivity puts the shape that returns what it reads on the 1 : 1 rule* |
| `min-width: 0` off the on-axis fill, so the 1 : 1 bar gets a 3px stub | the same test's one rendered-width assertion |
| the index split's third part priced off `indexesSeen` | *the index split is counted from the usage rows, not the catalogue* |
| a missing usage read drawn as three zeroes | *no split is drawn when the usage read did not answer* |
| latency ranked by total time | *latency ranks by the gap, not by total time* |
| `.tone-mark` reading `--conn-colour` | *a chart does not take the connection's colour* |
| a write drawn at ratio zero | *a write has no ratio and is left out rather than drawn on the rule* |
| the chart switch calling `load()` | *switching chart keeps the open row and asks the server nothing* |

**The seventh had to be rewritten before it bit, and why is the useful
part.** The first attempt changed the token — `--chart-mark: var(--conn-colour)`
on `:root` — and every test still passed. That is not a weak test: a custom
property substitutes at computed-value time *on the element it is declared
on*, so a `var(--conn-colour)` written at `:root` resolves against `:root`'s
own value, which is `var(--accent)`, and inherits down already blue. `#app`'s
override never reaches it. Declaring the chart tones at `:root` makes them
**structurally unable** to follow the connection, which is the same mechanism
Stage 32 §4 leaned on to give `--conn-colour` a default, used here to
guarantee the opposite outcome. The falsification that bites is the one a
person would actually write: `background: var(--conn-colour)` on the rule
itself, inside `#app`, where inheritance does reach.

**Gates.** 1,397 UI tests pass on Chromium and WebKit with 1 skipped;
`mise run check` exits 0 and clippy reports nothing in the new code.

## 8. Still open

**The 1 : 1 counter-example can fall off its own chart.** Selectivity draws
the top five by ratio, so on a server with more than five application shapes
the row that sits *on* the rule — the one that makes the rule mean something —
is the first to be cut. The ranking is right (the worst offenders are what the
chart is for) and the verdict chip still says *no index will help* on the row
below. Worth watching in use.

**No chart on the chatter or platform lists.** They get the same card, drawn
from the same code, and nothing has been thought about whether ranking driver
chatter by selectivity says anything. It costs nothing to leave on and would
cost a special case to turn off.

**White on amber** is still open from Stage 32 §8, and the charts do not touch
it: nothing here uses `--conn-colour` or `--on-accent`.
