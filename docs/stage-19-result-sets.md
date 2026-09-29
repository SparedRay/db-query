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
