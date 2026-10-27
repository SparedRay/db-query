# Stage 28 — Migrations docked on the right

**Status:** ✅ Done — built and reviewed 2026-10-27

---

## 1. The use case, in the user's words

> *"our migration panel, let's align with rest of our UI so its its own
> resizable panel. Same way as results (Just that default value is smaller we
> dont need half screen for migration)"*

> *"Why migration is not just another panel as Results is with editor?"*

Three layouts were drawn on a canvas and compared — the sidebar section made
resizable, a panel docked under the output, a panel docked on the right — and
the right-hand panel was chosen to try in use: *"I will start with option 2b for
now. Lets work on that and see with usage how it work for us"*.

## 2. What is there (Stage 18)

| Piece | Where |
| --- | --- |
| Projects and environments | `#mig-side`, a section under the schema tree, capped at 45% of the sidebar, no seam |
| The list, Apply, Repair, Refresh | `#mig-view`, shown **in place of the editor** while a tab of kind `migrations` is in front |
| Flyway's report | `#flyway-pane`, shown **in place of the result grid** while that tab is in front |

So reading migrations hides the editor, and the one place migrations live is
split across three surfaces that each appear and disappear with a tab.

## 3. The design

```
rail │ SCHEMA          │ [query.sql] [V8__backfill.sql]   ┆ MIGRATIONS        Ctrl+Shift+M
  ●  │ ▾ maindatabase  │ Run  Run all  Format             ┆ [Orders project · uat ▾]
     │   ▸ Tables      │ SELECT …                         ┆ ● UAT  as uat_app · host:3306
     │   ▸ Views       │                                  ┆ Apply 2…  Repair…  Refresh
     │                 │ ── Output ────────────── Ctrl+J  ┆ ▾ Pending 2
     │                 │ Result 1 · Result 2              ┆   7  add shipping zone
     │                 │                                  ┆ ▾ Last run · migrate
```

* **One panel, docked on the right**, built the way the assistant's is
  (Stage 21): two grid columns, a seam that drags its width, a rail button.
  Shut, it is gone; the rail button carries how many are pending — a migration
  waiting is the one thing worth seeing without opening it.
* **The editor never goes away.** Checking a migration with a query is two
  things on screen at once rather than a round trip through the tab strip.
* **The environment is chosen in the panel's header**, from a picker that lists
  the same project and environment rows the sidebar drew, with the same
  matches, errors, Add and Remove. The sidebar goes back to the schema alone.
* **Flyway's report is the panel's own**, a "Last run" section under the list.
  The result pane goes back to being about SQL only.
* **Ctrl+Shift+M** toggles it. Free in every handler, measured.

## 4. Decisions

1. **The migrations tab kind is removed**, not kept beside the panel. Two places
   showing one environment's list is the confusion Stage 18 §5.3 removed.
2. **Shut is nothing, and the rail holds the button** — as the assistant's
   (see §7.1 for the strip this replaced). The button carries the pending
   count once one is known. Nothing runs Flyway to fill it: a count appears
   after the first reading.
3. **Starts shut, and neither its width nor its state is remembered across
   launches** — the policy every splitter and panel in this app follows
   (Stage 12, 21, 26). The chosen environment *is* remembered, as it is today.
4. **The report is kept per environment** (it was per tab). Switching
   environments shows that environment's last run, which is what the tab
   showed when it was re-pointed.
5. **Both right-hand panels can be open at once**; migrations sits inside the
   assistant, nearer the editor whose SQL it is about.

## 5. Milestones

| # | Claim | Evidence |
|---|---|---|
| D1 | The panel opens and shuts from its rail button, its close button and Ctrl+Shift+M, and gives its width back | UI test |
| D2 | Its seam drags its width, measured from the panel rather than the window | UI test, with the assistant open |
| D3 | The editor and the result grid stay on screen while it is open | UI test |
| D4 | The picker lists projects and environments, with matches, errors, Add and Remove | Stage 18's tests, re-pointed |
| D5 | Apply, Repair, Out of order, Check first and Apply up to here work from the panel | Stage 15–24's tests, re-pointed |
| D6 | Flyway's report shows in the panel and the result pane never shows it | UI tests |
| D7 | The rail button says how many are pending, or what has failed, once a reading exists | UI test |
| D8 | No tab of kind `migrations` exists anywhere | Code, and the session test |
| D9 | Hands-on, in use | The user |

## 6. Task tracker

- [x] `#mig-pane` and `#migsplit` in the grid; rail button, Ctrl+Shift+M (D1, D2)
- [x] The view's markup moved into the panel; the picker popover (D3, D4)
- [x] `#mig-side` and its CSS removed; dead CSS removed (`#mig-title`,
      `.mig-proj-top`, `.mig-proj-name`, `.mig-proj-swap`, `.mig-projects`,
      `.stab-view`), none of it referenced since Stage 18
- [x] The `migrations` tab kind removed from `tabs.ts` and `main.ts` (D8)
- [x] The report moved from `#flyway-pane` to the panel, kept per environment (D6)
- [x] The pending count on the rail button (D7)
- [x] Tests re-pointed; new panel tests; `mise run check`; both engines
- [x] Reviewed against this document; four claims it made were untested — §7.3
- [x] Hands-on (D9) — *"Tested locally and seems fine"*

## 7. Built — 2026-10-27

### 7.1 What changed shape on the way

* **The default width is 380px, not the 420 drawn.** A screenshot at 1440×860
  with the panel open showed the editor's toolbar folding each button onto two
  lines (`Run` over `Ctrl+Enter`). 380 is the assistant's default, and Apply,
  Repair and Refresh still sit on one line at the seam's 300px floor — which
  the narrow-row test now checks by setting that width rather than shrinking
  the window.
* **Escape shuts the picker wherever focus is.** The first version listened on
  the popover, and after a click focus is on the picker's own button, so Escape
  did nothing. Found by the screenshot run, not by a test; the test came after.
* **The seam measures from the panel's own right edge**, not the window's, so
  dragging works with the assistant open beyond it.
* **The strip became a rail button**, after the first hands-on look. The
  shut strip's head was turned with `writing-mode: vertical-rl`, and in the
  app it still read horizontally, squeezed into 32px — no test measured which
  way text ran. Rather than fight that, the panel closes to zero like the
  assistant and its toggle sits in the rail beside the assistant's, the count a
  badge on it ("2", the full "2 pending" in its title). That raised whether the
  right-hand panels' buttons belong on a right-hand rail; one rail it stays,
  until use says otherwise.
* **A close button in the head**, which the §3 drawing does not have — it put
  `Ctrl+Shift+M` there instead. The key is in the button's title, where every
  other pane in this app keeps its shortcut, and the head reads as the
  assistant's does rather than as a label with a keystroke stuck to it.
* **The cheat sheet** in a new buffer grew a line for the panels:
  `Ctrl+J results · Ctrl+Shift+M migrations · Ctrl+K assistant`. On the line
  it was first added to, it wrapped beside an open panel.

### 7.2 Tests

`openApp` and `withProject` — which nearly every migrations test goes
through — now open the panel and choose from the picker, so Stage 15–24's
behaviour tests run unchanged against the new surface. The tab's own tests
(one tab re-pointed, survives switching connection, closing puts the editor
back, Run/Format inert, never in the session) became the panel's: one place and
the editor stays; it stays put across connections; it shuts from the rail,
its close button and the key, and gives all its width back; the seam drags and the width survives a shut; the picker
shuts on Escape and a click away; the session still never holds it. The
section-collapse test became the rail button's pending count (D7), and the two output
tests moved to the panel's Last run, which is now kept per environment.

**Falsified, one at a time**, each failing its test: Escape only inside the
popover; the strip never showing a count; Ctrl+M without Shift toggling the
panel; the report read from one global slot instead of per environment.

**A WebKit run that overlapped editing failed 47 tests**, MCP ones among them:
Vite reloads the page when a source file changes under it. Re-run with nothing
changing, the same files passed. Recorded because the number looked like a
regression and was not.

After the rail change the badge was falsified once more (never shown), and
its test failed.

**556 UI tests on Chromium and 555 on WebKit, none failing**, run back to back
with nothing changing; `mise run check` clean. The backend did not change.
(§7.3 added two more: 557 and 556.)

### 7.3 Reviewed against this document — 2026-10-27

Read side by side with §3–§6. The shape is what was designed: the panel is
grid columns 5–6 and the assistant 7–8, so migrations does sit *inside* it
(decision 5); `#app` is born `mig-shut` and the width lives only in
`--mig-w-set` (decision 3); no `TabKind` but `sql` and `dictionary` survives
(decision 1, D8); `#flyway-body` is the panel's and `#res-title` is always
"Results" (decision 4, D6); the count is painted from a reading and from
nothing else (decision 2, D7). What the review changed:

**Four claims this document makes that no test held.** Each was written into
§7.1 or into a code comment as settled, and each could be reversed with the
whole suite still green:

* **§7.1's seam fix.** "The seam measures from the panel's own right edge, not
  the window's, so dragging works with the assistant open beyond it." Replace
  `els.migPane.getBoundingClientRect().right` with `window.innerWidth` and all
  77 migrations tests still passed — because every drag in the file happens
  with the assistant shut, and there the two edges are the same place. The bug
  it fixed only appears with something open beyond the panel. There is a test
  with the assistant open now, and it also holds decision 5: both panels on
  screen at once, migrations the inner one. Measured from the window, a 60px
  pull does not narrow the panel at all — the assistant's 380px offsets the
  measurement and the panel jumps to the seam's ceiling instead.
* **The badge while the panel is open.** D7 only ever looked at it shut. The
  comment this review wrote — that it is deliberately left showing, because a
  badge vanishing on open would read as "nothing pending" — now has a test;
  hiding it while open fails that test.
* **The badge for a failure.** `.bad` and "N failed" had no test at all, so the
  red that says an apply is blocked could be dropped silently.
* **"Cleared when the environment changes."** `if (switched) paintPending(null)`
  could be deleted and nothing noticed. The test holds `development`'s answer
  and asserts the button says nothing in between, rather than `uat`'s failure.

Not the same class as this project's three passing falsifications, which were
assertions true for the wrong reason. These were claims with no assertion at
all — recorded as done in §7.1 and in comments, which reads the same in a
document and is not the same thing.

**Two things the panel did with no projects in it**, both found by probing the
state no test covered, both now tested and each falsified on its own:

* **The picker opened an empty box.** With no projects the popover still
  opened — 362×10px, bordered, shadowed, nothing in it — under a button
  already saying "No projects yet". `.mig-picker-pop:empty` now hides it, the
  same idiom `.mig-projects:empty` used before this stage deleted it.
* **"Last run" sat over an empty body.** `renderMigrations`'s no-projects
  branch was the one path that never called `showFlywayOutput()`, so the
  section drew its header and nothing else — not even its own "Nothing has run
  here yet". `showFlywayOutput()` hides the whole section when no environment
  is chosen, and that branch calls it.

**Five things left behind by the move.** None of them changed behaviour, and
`mise run check` has nothing to say about any of them — this project has no
prettier, and dead CSS compiles:

* `--mig-w`'s fallback was still `32px`, the width of the shut strip §7.1
  abandoned. Unreachable, because both `.mig-shut` and `:not(.mig-shut)` set
  the variable — but it said the opposite of decision 2. It is `380px` now,
  the assistant's.
* `.mig-pane-body > .mig-empty` matched nothing: every `migrationsMessage`
  lands in `#mig-view-list`, which is a grandchild.
* `#editor[hidden] { display: none; }` is dead — nothing hides the editor
  since the view became a panel, and this file has `[hidden] { display: none
  !important }` anyway — and `.mig-projects`'s comment had been orphaned onto
  it when that rule was deleted. The same class of thing clippy caught in
  Stage 25 slice 3, with no clippy here to catch it.
* Stage 26's result header still said it was built from "the sidebar's
  `.mig-side-toggle`", and `onCreated` still said `openMigrations`
  deliberately skipped it. Both name something this stage removed.
* `paintPending`'s own comment said "what the strip says while the panel is
  shut". There is no strip, and the badge shows open or shut — which is
  deliberate, so the comment now says why.

And one slip: `tabs.ts` had `title: spec.title` indented two columns too far,
left over from deleting the `migrations: null` line above it.

**`docs/README.md` described the strip, not the rail button.** The index row
was written from §3 before §7.1 changed it, so the one-line summary of this
stage contradicted the stage.

**Not changed, noted.** `#mig-pane` repeats `#chat-pane`'s five layout
declarations and its head borrows `.chat-title`; the two panels are the same
shape by design, and a shared `.pane-title` would be a rename reaching into
the assistant. Left for whoever settles the §8 question about the rail.
`showSelected`'s "Choose an environment from the list above." is unreachable —
`validSelection` always falls back to a project's first environment, so a null
target means no projects, and that branch returns earlier. It was unreachable
before this stage too.

**Six new tests, each falsified on its own**, and each failure landing on its
own assertion: the empty popover shown; the Last run section left visible; its
`showFlywayOutput()` call dropped from the no-projects branch; the seam
measured from the window; the badge never red; the badge hidden while open; the
badge not cleared on a switch.

**560 UI tests on Chromium and 559 on WebKit, none failing** (one skipped on
WebKit), run back to back with nothing changing; `mise run check` exit 0.

## 8. Where it stands — 2026-10-27

* **Committed** on branch `stage-28-migrations-panel`, after the hands-on
  (D9) and the review in §7.3. The version is not bumped: `0.8.2` was cut
  above Stage 27, so this stage is the only unreleased one and the bump is a
  release decision, not part of the work.
* **The canvas** the layouts were compared on, with the Results/Sequence
  mock-up: https://claude.ai/artifact/SxTpzgHeqwcMAixySnfBvd (screens Today,
  Option 1, States, Dock bottom 2a, Dock right 2b — 2b is what was built).
* **Open, to settle with use:** whether the right-hand panels' buttons
  (assistant, migrations) move to a rail of their own on the right. Not a
  setting — one layout or the other.
* **Next, Stage 29: Results / Sequence**, two views of the output pane, asked
  for in the same request as this stage and deferred until this one had been
  used. Designed on the canvas, not built. Decisions still open there: no
  automatic switch to Sequence; whether the view is chosen per tab; how the
  header splits between the two.
