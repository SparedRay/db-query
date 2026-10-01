# Stage 19 — A result knows which statement made it

**Status:** ✅ Built — 2026-09-29.

---

## 1. The use case, in the user's words

> *"Maybe work on result tabs so the title says Resultset 1/2/3 and if you
> double click it it highlight the part of the code that returned that?"*

## 2. What was wrong

A multi-statement run drew one tab per statement, labelled with the
statement's first line. For a script that selects from the same table a dozen
times — which is what a real script looks like — that is the same forty
characters twelve times, truncated. The number was there but buried in front of
the text, and the only way to find out which statement a result came from was
to read the script and count.

## 3. The design

* **The label is the number**: `Result 1`, `Result 2`. That is what a tab strip
  is for — telling one of several apart — and the row count badge already says
  which is which at a glance.
* **The SQL moves to the tooltip**, with the gesture that reveals it.
* **Double-click selects the statement in the editor.** Its own gesture, not a
  second control: single click already means "show me this result", and the two
  would be a few pixels apart with nothing to tell them apart.

### 3.1 Knowing where a statement was

The splitter has computed each statement's byte span since Stage 0 and
`run_script` was throwing it away. `StatementResult` now carries `start` and
`end`, and `statement_at_cursor` answers `{ sql, start }` rather than the text
alone.

Those offsets are relative to the **script that was submitted**, which is not
always the buffer: Run all sends everything, a selection sends itself, and Run
sends one statement. So the tab records the base offset of what it ran, and the
editor position is `base + statement.start`. Bytes throughout, because that is
what every offset crossing the Rust boundary already is; the renderer converts
once, with the same helper diagnostics use.

### 3.2 The buffer moves on

The offsets were true when the script ran and the text has been editable ever
since. They are checked rather than trusted:

1. If the text at the recorded range is still that statement, that is it. **This
   is what tells two identical statements apart** — searching would find the
   first one for both.
2. Otherwise the statement's text is searched for, and used if it appears
   **exactly once**. Twice is as good as missing: there would be no reason to
   prefer either, and highlighting the wrong one is a confident lie.
3. Otherwise nothing is selected and the status line says the statement is no
   longer in this tab, quoting what ran.

## 4. Proof

Two Rust tests on the offsets `statement_at_cursor` now reports — including
that the span stops before the `;`, which is what the editor will select — and
seven UI tests: the labels and tooltip, double-click selecting the statement,
single click leaving the editor alone, two identical statements told apart by
position, a statement that moved being found, one that is gone saying so, and an
ambiguous one refusing to guess.

Falsified: with the offsets ignored the identical-statements test fails, and
with the "exactly once" guard removed the ambiguous test fails.

**390 Rust unit tests, 794 UI tests on both engines.** One UI test failed once
during a full run and did not reproduce on a re-run; it is a flake, unidentified
rather than fixed.

## 5. Found in use — 2026-09-29

Two bugs from the same report, neither about result sets.

### 5.1 Cancel did nothing when the server could not be reached

> *"cancel a query seems to be failing as it looks stuck (If for example I need
> a VPN but tried the query before connecting clicking on cancel does nothing
> until we reach a timeout)"*

Cancel did two things: set a flag the script reads **between** statements, and
`KILL QUERY` from the connection's killer. Both assume the server answers. With
the VPN down there is nothing to kill and nothing between statements — the wait
is inside the socket, either in the statement or in opening the connection —
so Cancel did nothing at all until the operating system gave up.

It now also **wakes everything waiting on that tab**, through a per-tab
`Notify`:

* `session::cancellable` wraps the connect and the `CONNECTION_ID()` round trip
  in `ensure_exec`, which is where a VPN-down query actually sits.
* `exec::await_query` races the statement against it.

**Dropping a query future abandons its connection mid-protocol**, so it stays
the last resort. On a cancel — and on the existing timeout — the server is asked
to stop and the future is given `CANCEL_GRACE` (two seconds, a round trip plus
room) to unwind on its own. That is the good path: the query stops server-side,
the connection is still good, and the tab keeps its session. Only when nothing
comes back is the future dropped, and then the connection is discarded and the
next run opens another.

`Cancel` itself is now bounded too: `KILL` needs the killer connection, and
opening one to an unreachable server is exactly as slow as the query being
cancelled, so the attempt gets two seconds and its failure is not reported. The
waiters are already awake by then, which is what stops the wait.

Six tests. Three unit tests on `cancellable` (a cancelled wait stops, a cancel
that arrived first is not lost, an ordinary wait still returns), three on
`await_query` (returns, unwinds in time, never returns), and a live one:
**a temporary table created before a cancelled query is still there afterwards**
— the session survived, so the connection was not thrown away needlessly. With
`CANCEL_GRACE` set to zero that live test fails.

### 5.2 A new tab forgot which schema you were in

> *"On new tab opening we are loosing the selected schema. We can mantain either
> the default of the connection or the same as last opened tab"*

A new tab took the connection's own database — where its session starts — and
nothing else. So opening a tab while working in another schema dropped you back
on the default, and the first unqualified name you wrote meant something else.

It now inherits the database of the tab you were on (`create` calls `onCreated`
before it activates the new tab, so that is still the previous one), falling
back to the connection's. When the inherited one differs, the tab's session is
moved with a `USE` — one round trip, and only then; the case where they agree
costs nothing, which the existing test still pins.

**396 Rust unit tests, 78 live MySQL, 798 UI on both engines.** The tab-scroll
test that flaked in the last two runs was the strip's smooth scrolling being
measured mid-animation; it now waits for the scroll to settle and passed three
runs in a row.

## 6. The connection loader is the connection's own shape — 2026-10-01

> *"On UI the loader for the connection today is a round loader. While the
> connection is a dotted square. Can't we make it so its an animation of the
> square loading?"*

A connecting rail item is `rail-item offline connecting`: a transparent
**rounded square**, 32px, radius 9, dashed border — with the shared `.spinner`
laid over it at `inset: -4px`, which is a **40px circle**. The right instinct
(the thing you clicked is the thing that should look busy) drawn in the wrong
shape, larger than the item it reported on, and the only circle on a rail of
rounded squares.

### 6.1 Choosing between five

Five loaders were drawn and animated at real size before any of them was
built — marching ants, a tracing stroke, corner brackets, a rising fill and a
conic sweep — on a design canvas, each in the rail at 32px in both themes,
because 32px in peripheral vision is the only size that matters. What the
mockups settled:

* **A rising fill** is a determinate shape. A connect has no percentage, so a
  level that climbs would be inventing one.
* **A conic sweep** has the right shape and the wrong speed: rotation is
  uniform in angle, not in distance, so the leading edge crawls past a corner
  and races along an edge.
* **Corner brackets** read as a selection marquee more than as waiting.

Chosen: **a tracing stroke** — one accent segment travelling the item's own
perimeter. *"Indeed is closer to today's UI"*: it keeps the current meaning
exactly and corrects only the shape, so there is nothing to re-learn.

### 6.2 What it cost

`busyOutline(radius)` in `icons.ts` — an inline `<svg>` with one `<rect>`.
Three details are load-bearing:

* **`pathLength="100"`** normalises the perimeter, so `stroke-dasharray:
  26 74` is *a quarter of the outline lit* at any size, and one element serves
  32px and anything later.
* **`inset: -1px`, not `0`.** An absolutely positioned child is laid out
  against the *padding* box, so `0` draws the outline a pixel inside the border
  it is meant to run along. Every state of a rail item carries a 1px border —
  dashed offline, transparent live — so -1px lands on the border box exactly.
* **`rx` is passed in, not normalised.** A corner radius is a length, not a
  fraction of a perimeter. The caller states the item's own 9px, so the CSS and
  the SVG agree by construction rather than by coincidence.

Also: `.rail-item.connecting:hover` no longer grows to an 11px radius. The item
is not clickable while connecting — the handler returns early, so a second
click cannot open a second connection — and growing it would both promise a
click that is refused and move the border 2px away from the stroke tracing it.

Reduced motion slows it to 3.4s rather than the ring's 2.4s, deliberately: a
travelling dash covers the whole perimeter in one period, so matching the
ring's duration would move the segment at about the speed it does now.

### 6.3 Proof

The three existing rail tests follow the element. The new one measures rather
than asserting a class name — **whatever draws the busy state has to be the
item's own box**, so it compares the two bounding boxes, and separately that
the rect is animating and drawn at the item's radius. Both halves falsify:
`inset: -5px` (a ring around the item again) fails it, and removing the
animation fails it.

**800 UI tests on both engines**, `mise run check` clean.

Noted in passing, not fixed here: `cargo clippy` warns `while_let_loop` in
`lint.rs`, from `e91e804` (the CTE work), and has since that commit.
