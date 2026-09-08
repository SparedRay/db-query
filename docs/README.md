# Trackers

One tracker per stage. A stage is frozen once its scope is delivered — its
tracker becomes the historical record of what was built and why, including
decisions that were corrected along the way.

| Stage | Tracker | Status |
|---|---|---|
| 0 | [POC — lightweight MySQL client](stage-0-poc.md) | 🔒 **Frozen** — complete and verified |
| 1 | [Script tabs & file open/save](stage-1-tabs-and-files.md) | 🔒 **Frozen** — complete and verified |
| 2 | [Connections](stage-2-connections.md) | 🔒 **Frozen** — complete; C3w (Windows) carried forward |
| 3 | [Schema actions & result export](stage-3-explore-and-export.md) | 🔒 **Frozen** — built; E1-E8 click-through never run, carried to Stage 4 |
| 4 | [Testability](stage-4-testability.md) | 🔒 **Frozen** — 192 UI tests on both engines; E1-E8 answered; CI + Windows carried to Stage 5 |
| 5 | [Packaging & distribution](stage-5-packaging.md) | 🔒 **Frozen** — public repo, MIT, CI green on both platforms, installers built; updater and attribution carried to Stage 6 |
| 6 | [Updates & attribution](stage-6-updates-and-attribution.md) | 📋 **Planned** — self-update on Windows, and the licence manifest distribution now obliges |

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
