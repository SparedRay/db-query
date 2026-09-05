# Trackers

One tracker per stage. A stage is frozen once its scope is delivered — its
tracker becomes the historical record of what was built and why, including
decisions that were corrected along the way.

| Stage | Tracker | Status |
|---|---|---|
| 0 | [POC — lightweight MySQL client](stage-0-poc.md) | 🔒 **Frozen** — complete and verified |
| 1 | [Script tabs & file open/save](stage-1-tabs-and-files.md) | 🔒 **Frozen** — complete and verified |

No stage is currently open. The next one starts by creating
`stage-2-<name>.md`, adding a row above, and marking Stage 1 as superseded by it
in its banner — the one edit a frozen tracker ever gets.

## Rules

- **Never edit a frozen tracker.** If something in it turns out to be wrong or
  gets superseded, record that in the *current* stage's tracker and link back.
  The value of a frozen tracker is that it says what we actually believed and
  decided at the time.
- **One stage, one file.** A stage is scoped to a coherent slice of work with
  its own milestones. When it closes, freeze it and open the next.
- **Milestones must be checkable by hand.** If you cannot demonstrate it, it is
  not a milestone.
- **Corrections are content.** When implementation proves a plan wrong, write
  down what was wrong and why — that is usually the most useful part.
