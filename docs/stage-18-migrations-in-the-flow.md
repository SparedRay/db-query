# Stage 18 — Migrations in the app's own flow

**Status:** ✅ Built — 2026-09-29. The pane it replaced is deleted.

---

## 1. The use case, in the user's words

> *"the Flyway UI still feels like a foreign on our current flow. Lets spin up
> alternative UI we can test so we can have the job done but better aligned with
> general work"*

Stage 17 made projects independent of connections, which was right, and left
them living in a pane of their own that behaves like nothing else in the app.

## 2. What "foreign" is, concretely

The app has three habits, and the migrations pane keeps none of them:

| The app's habit | The pane |
|---|---|
| **The left sidebar is where you choose what you are looking at** — connections in the rail, databases and tables in the tree. | A second list, on the other side of the window, with its own idea of rows. |
| **The main area is where you read what you chose**, in tabs you can leave and come back to. | Reading happens in a 340px column, and the project list is squeezed above it. |
| **Actions belong to the thing on screen** — Run and Format sit over the editor. | Apply and Repair sit in a pane header, far from the list they act on. |

It also spends screen badly: a list of migrations is a *table* — version,
description, state, when it ran — shown in a narrow column while the wide part
of the window holds an editor nobody is looking at during a migration.

## 3. The design

### 3.1 Projects join the sidebar; migrations open as a tab

```
rail │ SCHEMA                │ [Untitled-1] [Migrations · uat ×]
  ●  │ ▾ maindatabase        │ Flyway Connections · uat · uat.example.com:3306
  ●  │   ▸ Tables            │ ● UAT   as uat_app        C:\repo\flyway.toml
     │   ▸ Views             │ [Apply 2…] [Repair…] [Refresh] [ ] Out of order
     │ ▾ MIGRATIONS          │ ┌────┬─────────────────┬─────────┬──────────┐
     │  ▾ Flyway Connections │ │ V  │ description     │ state   │ applied  │
     │      development      │ │ 1  │ create widgets  │ Success │ 10 Sep   │
     │    ▸ uat      ● UAT   │ │ 2  │ add colour      │ Pending │          │
     │  ▸ Orders project     │ │ 3  │ broken          │ Failed  │ 10 Sep   │
     │ + Add project         │ └────┴─────────────────┴─────────┴──────────┘
```

* **The sidebar** grows a `Migrations` section under the tree: projects, their
  environments, and which saved connection each one is — the same facts Stage 17
  computes, in the place where choosing happens.
* **The main area** shows the chosen environment in a tab: the target said in
  full, the actions over the list they act on, and the list itself given the
  width to be a table.
* **Groups stay.** Pending / Failed / Executed are headings within the table
  (Stage 15 §14.2): they answer "is anything waiting for me", which sorting
  by version does not.

### 3.2 A tab that belongs to no connection

Every tab so far belongs to a connection for life. A migrations tab cannot:
Stage 17's whole point is that a project needs no connection open.

So `connectionId: null` becomes legal and means **global** — visible in every
connection's strip, and present when there is no connection at all. `kind`
separates the two sorts of tab: `sql` (an editor buffer) and `migrations` (a
view onto a project environment). One migrations tab exists at a time; choosing
another environment re-points it, so the strip cannot fill up with them.

It is not remembered across restarts. A view derived entirely from a file on
disk and the saved projects is cheaper to reopen than to restore, and a restore
would have to guess whether the project still exists.

### 3.3 Both UIs, behind a setting

`migrationsLayout`: `tree` (new, the default) or `pane` (Stage 17's, unchanged).
Settings → Migrations switches it without a restart. Both are tested. When the
new one has been used against a real project the loser is deleted — the toggle
is scaffolding for this comparison, not a permanent choice to maintain.

### 3.4 What does not change

Everything Stage 17 decided about *behaviour*: matching environments to saved
connections, the read-only refusal, the confirmed-target check, dropping a stale
answer, refreshing a matched connection's schema after an apply, and both repair
dialogs. This stage moves where things are, not what they do. The backend does
not change at all.

## 4. Milestones

| # | Claim | Evidence |
|---|---|---|
| G1 | The sidebar lists projects and environments, with no connection open | UI test from a fresh app |
| G2 | Choosing an environment opens one tab; choosing another re-points it rather than opening a second | UI test on the strip |
| G3 | The migrations tab is visible whichever connection is active, and with none | UI test switching connections |
| G4 | Apply, repair, out-of-order and opening a migration's SQL work from the new UI | UI tests, the Stage 17 set re-pointed |
| G5 | Run, Format and Save do nothing while a migrations tab is active | UI test |
| G6 | A migrations tab is never written to the session file | UI test on `save_session` |
| G7 | The setting switches between the two, live, and the pane still works | UI tests on both layouts |
| G8 | Hands-on against the real project | The user |

## 5. Decisions

### 5.1 Why not keep it all in the sidebar?

Because the list is a table and the sidebar is a column. The pane's rows had to
drop the applied-on date and Flyway's own state word to fit; both are things
people read when deciding whether to apply.

### 5.2 Why a tab rather than a mode of the editor pane?

A tab is leavable. During a migration people move between the migration's SQL,
a query that checks what it did, and the list; a mode that replaces the editor
makes each of those a round trip through a toggle.

### 5.3 Why one migrations tab rather than one per environment?

Because the sidebar already holds the choosing. Two tabs on two environments of
one project would put two Apply buttons on screen for two different databases,
which is the exact confusion Stage 17 removed.

## 6. Task tracker

### Phase 1 — The setting and the sidebar

- [x] `migrationsLayout` setting, with the Settings control
- [x] Sidebar `Migrations` section: projects, environments, matches, add/remove (G1)

### Phase 2 — The tab

- [x] Global (`connectionId: null`) tabs of kind `migrations` (G2, G3)
- [x] The view: target line, toolbar, grouped table (G4)
- [x] Guards: Run/Format/Save inert; not persisted (G5, G6)

### Phase 3 — Proof

- [x] UI tests for both layouts (G7)
- [x] Hands-on (G8), then delete the loser

## 7. The design pass, and what it changed — 2026-09-28

> *"Before we push anything we should properly plan on canvas the design
> concept"*

Drawn first, on a canvas, in the app's own colours: today's pane beside the
concept, the five states it has to answer, and the apply flow step by step.
Three things came out of reviewing it that the code would otherwise have
shipped wrong.

### 7.1 The buttons were not cut; the convention was wrong

> *"even Apply is cutted, Repair as well. Cannot be show with full text?"*

`Apply 2…` and `Repair…` end in an ellipsis because that is the desktop
convention for "this opens a dialog". Read as a label cut short — which, in a
340px pane, is what a label usually is. They now say `Apply 2 pending` and
`Repair schema history`. The width to spell them out is one of the things the
new layout buys.

### 7.2 Picking migrations: what Flyway actually allows

> *"is it a feature of Flyway where we can use checkbox to select which
> migrations to apply (All if none selected)"*

Measured against the pinned Flyway 13.5.0 Community on 2026-09-28, against the
fixture, rather than recalled:

* `migrate -cherryPick=4` → **`ERROR: Upgrade required: Cherry pick is not
  supported by Community.`** Arbitrary selection is Teams/Enterprise.
* `migrate -target=2` → applied V1 and V2 and left V3 and V4 `Pending`.
  Community, no licence.

So checkboxes would offer something this edition refuses. What is honest is a
**stopping point**: every pending row carries `Apply up to here`, which runs
everything pending up to and including it. Apply without one runs them all,
which is Flyway's own default.

`target_flag` refuses anything that is not shaped like a version. The value
comes from Flyway's own output and travels as its own argv entry, so there is
no shell to escape — but a value starting `-` would be read by Flyway as
another flag, and a flag we did not mean to send is not something to pass on
trust. `latest` and `current` are refused too: this button means "up to this
row", and accepting a keyword would make it a different promise.

The row is a `<button>` that opens the migration's SQL, so the action could not
sit inside it — invalid markup, and unreachable by keyboard. The row is wrapped
instead, and the action lives beside it, shown on hover or focus.

### 7.3 The confirmation names only what will run

> *"Confirmation we should only mention the ones that will be applied. no need
> to mention the missing ones."*

An earlier draft named what would be left behind as well. The list underneath
the dialog already says that, and naming migrations that will *not* be touched
invites reading them as part of what is about to happen.

### 7.4 Rationale does not belong inside a mockup

> *"I must assume the Why not checboxes text is not final rendering right?"*

It was not, and it should never have been drawn inside the frame. A mockup
shows what the product shows; reasons go in the notes beside it. Removed from
all four boards; the tooltips and result messages that *are* app copy stayed,
drawn as the tooltips and results pane they really are.

### 7.5 Two bugs the new tests found

* **Switching connection could land on the migrations view.**
  `setActiveConnection` chose from `visible()`, which now includes global tabs
  — and worse, a connection with no tabs of its own would never get one,
  because the global tab made the list look non-empty. It asks
  `forConnection` now.
* **Closing the view with nothing connected crashed.** There was no neighbour
  to activate, and `siblings[-1].id` threw.

### 7.6 Measured, not assumed

**388 Rust unit tests, 788 UI tests on both engines** (13 new, in
`migrations-tree.spec.ts`; the pane's 38 kept and re-pointed at
`migrationsLayout: "pane"`). Three claims were falsified deliberately: the
confirmation naming only what runs, the connection-switch guard, and the
session guard — each fails its test when its fix is removed. Screenshots of the
built layout and of the confirmation match the canvas.

## 8. The pane is gone — 2026-09-29

> *"our new panel needs a caret. Otherwise it looks like is not toggleable.
> Also we will have to remove the side button and the old code if not in use
> anymore."*

The comparison §3.3 set up is over: the sidebar shape is the one, so the other
was deleted rather than left to rot behind a setting.

* **A caret.** The section header had a `twisty` span and nothing in it — the
  tree's chevron is scoped to `.node`, which the section is not. It now carries
  the app's own chevron, rotating rather than swapping glyph, so open and shut
  cannot drift apart.
* **Deleted**: the rail button, `#migrations-pane` and its splitter, the grid
  tracks they needed, `migrationsLayout` and its Settings control, and every
  branch that asked which layout was on. `migUi()` survives as one place to
  name the elements rather than four call sites naming them.
* **The section is always shown**, and filled at boot: it reads the project
  files and the saved connections and asks Flyway nothing.
* **The empty state moved into the sidebar.** With no projects there is no view
  open, so "No Flyway projects yet…" was being drawn in a list nobody could
  see.
* **Adding a project lands on it.** Adding one is asking to look at it, and the
  row you would click next is the one the file already names.

`migrations-tree.spec.ts` is folded into `migrations.spec.ts`: **47 tests, one
file, one layout**. Nothing was dropped in the merge — the pane's claims were
about behaviour both shapes shared, and they are all still asserted.

**388 Rust unit tests, 780 UI tests on both engines.**
