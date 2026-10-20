# Stage 24 — Fewer JVMs between a click and an answer

Reported in use, 2026-10-19:

> Currently when we do a Repair for example it does a checking step first, then
> it asks for a confirmation and only then starts the process. On day to day
> where we have a migration folder with over 1k migrations this takes some
> time. Is the checking step really necessary? Couldn't the result from prev
> step be cached? For example common use case is do a repair with commented
> scripts that were run manually, then uncomment and do the repair, so we have
> Checking before apply, then confirm, then apply, then checking before repair
> then confirm then repair which seems like time wasted as each checking takes
> some time

---

## 1. What is actually slow — measured 2026-10-19

Flyway 13.5.0 Community, the pinned fixture CLI, against a local MySQL 8 and a
generated 1,000-migration project.

| What was timed | Time |
|---|---|
| `flyway --version` — no config file, no database | **4.3s** |
| `info`, 4 migrations (the repo fixture) | **4.3s** |
| `info`, 1,000 pending | 3.9 – 5.3s |
| `info`, 1,000 applied (1,000 history rows, 1,000 checksums) | **4.8s** |
| `migrate`, 1,000 pending | 37.2s |

**The folder size is not the cost.** Four migrations cost the same as a
thousand; `--version`, which reads no config and opens no connection, costs the
same again. Nearly all of it is JVM start and Flyway's own initialisation. A
thousand migrations add about half a second.

So the thing to reduce is not the work per invocation. It is **the number of
invocations**, and the reported workflow starts ten:

| | Why |
|---|---|
| 1 | select the environment — `showSelected` asks `info` |
| 2 | click Repair — the "Checking…" `info` |
| 3 | `repair` |
| 4 | `showSelected` again after it — `info` |
| 5 | click Apply — the "Checking…" `info` |
| 6 | `migrate` |
| 7 | `showSelected` — `info` |
| 8 | click Repair — the "Checking…" `info` |
| 9 | `repair` |
| 10 | `showSelected` — `info` |

Ten JVM starts, ~48s, of which three invocations do what was asked.

### Flyway takes several commands in one invocation

Measured the same day:

```
$ flyway -configFiles=flyway.toml -environment=development -outputType=json \
         repair migrate info
exit 0 in 4.83s
```

```json
{ "individualResults": [
    { "operation": "repair",  "repairActions": ["Removed failed migrations"] },
    { "operation": "migrate", "migrationsExecuted": 1, "targetSchemaVersion": "2" },
    { "operation": "info",    "migrations": [ … ] } ] }
```

Three operations, **one JVM, 4.83s**. So the refresh after a write — today its
own 4.8s process — is free if it rides along, and it is *fresher* than today's,
coming from the same JVM moments after the write rather than from a new process
later.

Three more facts that shape the implementation, all measured 2026-10-19:

- **A failing first command aborts the batch.** `migrate info` where `migrate`
  fails returns the ordinary single-operation shape — `operation: migrate`, an
  `error`, no `individualResults` — and the trailing `info` never runs. So the
  existing `complaint()` parser keeps working untouched, and the failure path
  still needs a refresh of its own.
- **`-outOfOrder=true` is harmless on a batched `repair info`.** Exit 0, both
  results present.
- **`-target=2` changes what the trailing `info` reports.** V3 and V4 came back
  as `Above Target` rather than `Pending` — which `Migration::group()` maps to
  `Done`, so a batched refresh after "apply up to here" would show the
  remaining migrations as though they had run. **A targeted apply is therefore
  not batched.**

### Asking the disk is a thousand times cheaper than asking Flyway

| | |
|---|---|
| `flyway info`, 1,000 applied | **4,800ms** |
| `stat` of 1,000 migration files, hashed | **4.3ms** |

---

## 2. The design

### 2.1 One JVM per action

`migrate` and `repair` run their refresh in the same invocation —
`migrate info`, `repair info` — and return the refreshed list with their own
result. The frontend uses it instead of calling `info` again.

Not batched when the apply carries a `-target`, for the measured reason above:
the refresh would misreport everything above the target. That path keeps today's
two invocations.

When the operation fails there is no trailing result, so the frontend falls
back to asking `info` itself — which is exactly what it does today.

### 2.2 The check before the confirmation, when the files have not moved

The "Checking…" `info` exists so the confirmation describes the database it is
about to write to, and that is worth keeping: the reported workflow *edits
migrations between operations*, and a blindly cached list would describe the
commented-out version of the scripts that were just uncommented.

But finding out whether anything changed does not need a JVM. Every `info`
answer now carries:

- the **folders** Flyway read migrations from — taken from the `filepath` of
  each migration it reported, never from `locations`, which stays Flyway's
  business (`flyway.rs` §module docs);
- a **fingerprint** of those folders: each folder's own mtime, plus every
  file's name, length and mtime.

Before a write dialog the app recomputes the fingerprint (4ms for 1,000 files)
and reuses the stored answer when it matches. A folder mtime changes when a
file is added or removed, and a file's length and mtime change when it is
edited — so the case the report is about, commenting and uncommenting scripts,
invalidates the reading and the check runs.

**What a fingerprint cannot see** is somebody else writing to that database,
and a brand-new `locations` folder that holds no migration the last `info`
reported. Two things answer that, below.

### 2.3 "Check first", a checkbox beside "Out of order"

> just lets add a check so same way we add the Out of order we could force the
> DB check (That way if i know no one else is working I can work from cache if
> in doubts i can force the check before running)

Off by default: the reading is reused when the fingerprint says it is still
true. Ticked: every write dialog asks Flyway first, whatever the fingerprint
says.

Per environment, in memory, exactly like `Out of order` — and beside it, for
the same reason: both change what happens when the button next to them is
pressed.

### 2.4 The dialog says how old its reading is

The honest half of §2.2. A confirmation built from a stored reading says so —
*"Read from disk 12 seconds ago."* — and one that just asked says that instead.
Nobody has to guess which they are looking at, and the one hole a fingerprint
cannot close is visible rather than silent.

---

## 3. Decisions

| | |
|---|---|
| **The cache is keyed by evidence, not by time** | A TTL would be a guess about somebody else's workday. A fingerprint is a fact about the files. The age is shown rather than enforced. |
| **`Done` stays the safe default for an unknown state** | Stage 11's rule, and `Above Target` is why it earns its keep: the batched-refresh trap it caused was a wrong *group*, not a crash. |
| **The out-of-order flag is part of the cache key** | `Ignored` becomes `Pending` under out-of-order, so the two answers are different answers. |
| **No persistence across restarts** | "Check first" matches `Out of order`, which is in memory and seeded from the project file. A flag that silently survived a restart would decide a write path nobody had looked at this session. |
| **The fingerprint is computed in Rust, over folders the frontend passes back** | The folders come from Flyway's own answer. Re-deriving them from `locations` is the thing `flyway.rs` deliberately does not do. |

---

## 4. Milestones

- **F1** Selecting an environment, then Repair, asks Flyway **once**, not twice.
- **F2** A repair reports *and* refreshes the list in one invocation.
- **F3** An apply of everything pending does the same.
- **F4** "Apply up to here" still refreshes correctly — the migrations above the
  target are still shown as pending.
- **F5** Editing a migration file between two operations makes the next dialog
  check again, with no checkbox touched.
- **F6** With "Check first" ticked, every write dialog asks Flyway.
- **F7** A failed apply still ends with a correct list on screen.
- **F8** The reported workflow — repair, edit, apply, repair — costs five JVM
  starts instead of ten.

## 5. Task tracker — built 2026-10-20

- [x] `flywaycli::run` takes several operations; `arguments` appends them in order
- [x] `result_for(stdout, op)` — the batched shape and the single shape
- [x] `fingerprint(folders)` and `folders_of(migrations)`
- [x] `flyway_info` answers a `Reading`: migrations + folders + fingerprint
- [x] `flyway_fingerprint` command
- [x] `flyway_migrate` / `flyway_repair` batch the refresh and return it
- [x] The reading cache, `Check first`, and the age line in both dialogs
- [x] 8 Rust unit tests, 3 live tests, 6 UI tests

Milestones: **F1–F7 tested.** F8 is arithmetic over them, below.

---

## 6. What implementation changed about the plan — 2026-10-20

### Two labels wearing one id

`showEnvironmentDetail` cleared its toggle with
`querySelector(".mig-ooo")?.remove()` — **singular**, written when there was
one. Adding `Check first` beside it meant one of the two survived every redraw,
so clicking between environments stacked up checkboxes all called
`mig-out-of-order`.

Found by an existing test, *"out of order belongs to the environment, not to
the window"*, which failed with `locator('#mig-out-of-order') resolved to 2
elements` — a strict-mode violation reporting a duplicate id nobody had
asserted about. It is now `querySelectorAll`.

### The fingerprint had to sort its own folders

Written to digest the folders in the order given, which is fine while the only
caller is `folders_of`, which sorts. But the list makes a round trip — out to
the frontend with the reading, back again to be compared — and a caller that
reordered it would get a different digest for the same folders: **a cache that
never hits, whose only symptom is the slowness this stage exists to remove.**

Caught by its own test, written because the claim "a fingerprint of a set of
folders" was worth checking rather than assuming. `fingerprint` sorts.

### A fingerprint does not come back

Adding a file and then deleting it leaves the folder's mtime moved **twice**,
so the digest differs from both earlier values. The live test asserted it
returned to its original value; it does not.

That is the right behaviour and the test was wrong: the digest answers *has
anything happened here*, not *is the content equivalent to before*. The cost of
a folder changed and changed back is one extra `info`; the opposite error would
be a confirmation built on a stale list. Written down rather than tightened.

### A poll that could not prove "and nothing more happened"

Both batched-refresh tests ended with

```ts
await expect.poll(async () => (await infoCalls(page)).length).toBe(after);
```

which **returns the moment it matches** — including on the sample taken before
the call it was meant to catch. Deleting the whole `if (out.info)` branch
failed only one of the two tests, and the falsification found it: the apply
test had been passing for the wrong reason.

Replaced with `settledInfoCalls`, which polls until the count stops changing —
the same settling the tab-strip scroll test uses. With that in place, the same
falsification fails both.

### A layout test that proved nothing until it was narrowed

`Check first` is a fourth item in a flex row that held three, so the row got
`flex-wrap`. The test for it — nothing clipped, every item visible — **passed
with the wrap removed again**, which means it was not testing the claim.

Probed for the width where it actually bites: 1280 through 700 are all fine,
and at **640px** the row runs out of space and, without wrapping, cuts the last
item off. The test now measures there. Removing `flex-wrap` fails it.

Worth recording because the failure mode is invisible to every behavioural
test: a button that has been cut off still answers a click perfectly.

### Repair takes an out-of-order flag it does not use

Not repair's business at all — it steers the `info` that now runs beside it, so
the refreshed list is asked the same question the pane was showing. Measured
harmless on `repair info` before it was wired up.

### F8, as arithmetic

The reported workflow — look, repair, edit the migrations, apply, repair —
with `Check first` off:

| | Before | After |
|---|---|---|
| select the environment | `info` | `info` |
| click Repair | `info` | fingerprint (~4ms) |
| repair | `repair` | `repair info` |
| refresh after it | `info` | — |
| click Apply | `info` | `info` — the files changed, correctly |
| apply | `migrate` | `migrate info` |
| refresh after it | `info` | — |
| click Repair | `info` | fingerprint |
| repair | `repair` | `repair info` |
| refresh after it | `info` | — |
| **JVM starts** | **10** | **5** |

~48s of JVM start becomes ~24s, and the one check that was genuinely needed —
after the files were edited — still runs. With `Check first` ticked it is 8
instead of 10: the two batched refreshes are still free.
