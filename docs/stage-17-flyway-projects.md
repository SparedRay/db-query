# Stage 17 — Flyway projects stand on their own

**Status:** 🚧 Phases 1–2 built — 2026-09-18. Live Flyway run and hands-on (F10) outstanding.

---

## 1. The use case, in the user's words

> *"if Flyway is independent from my current connection why it needs us to be
> connected to a DB in order to execute? that creates the false sensation that
> will be applied to current connection. If is independent then will be better
> if it shows each project as we list schemas maybe so we can switch between
> them"*

And, agreeing to the plan: *"Lets plan as its own stage and start working on
it."*

## 2. What was wrong with Stage 15's shape

Stage 15 §3.1 attached a project to a connection: one environment per
connection, the guard comparing the two, the pane following the active
connection. It was argued for, and the arguments were reasonable. The first real
use showed where they fell short:

| Stage 15 said | What is actually true |
|---|---|
| "A Flyway environment *is* a connection" | It is a **URL and a user in a file we do not own**. `flyway migrate -environment=qa` connects with the file's settings. Our connection is never used, and never could be (§5.4). |
| The pane follows the active connection | So the UI implies that the connection you are looking at is the one being migrated. Nothing in the backend needs that connection open: apply reads the *saved* profile, not the session. |
| The guard compares environment and connection | It compared the wrong field (`database`, which is only the default schema — Stage 16 §17.1), its override was forgotten (§17.2), and editing the connection detached the project (§17.3). Three bugs in the one mechanism that exists only because of the coupling. |
| Seeing the result in the tree needs the connection | Still true, and kept (§4, F6) — but as a *consequence* of a match, not a precondition for applying. |

The question the user actually has before applying is not "does this file agree
with the connection I happen to have open?" It is **"which of my databases is
this environment, and is that one I am allowed to change?"**

## 3. The design

### 3.1 A project is a thing in its own right

The Migrations pane lists **projects**, independently of any connection, the
way the schema pane lists databases. Each project expands into its
environments. You choose an environment, and the migrations list shows what
Flyway says about it.

```
Migrations                                    [+ Add project]
▾ Flyway Connections            C:\repo\flyway.toml        ⋯
    development   dev.example.com    ● Dev
    qa            qa.example.com     ● QA  read-only
  ▸ uat           uat.example.com    no saved connection
──────────────────────────────────────────────────────────────
uat · uat.example.com:3306 as cftconn_uat_app
Config · flyway.toml in C:\repo
Migrations · C:\repo\myproject\src\Flyway\MasterScripts
[ ] Out of order                  [Repair…] [Apply 2…] [Refresh]
Failed / Pending / Executed …
```

The rail button is always there. No connection has to be open, and the pane no
longer changes when you click a different connection.

### 3.2 Environments are matched to connections, not attached to them

For each environment the backend finds the **saved** connections whose host,
port and user agree with its URL — `flyway::disagreements` is empty. Only MySQL
profiles are candidates, because only MySQL projects are supported.

* The match is **computed every time**, from the file as it reads now. Nothing
  is stored, so a branch switch that changes a URL changes the match, and there
  is no record of a match to go stale.
* A match shows the connection's name and colour. That is what says "this is
  prod" before anybody presses Apply.
* **More than one match** — the same server and account saved twice, say with
  different default databases — shows the first and "+N". If *any* of them is
  read-only, the environment is treated as read-only. Failing closed is the only
  safe answer when two profiles disagree about whether writes are allowed.
* **No match** is allowed. The environment is labelled "no saved connection",
  and the apply and repair confirmations say plainly that the target is not one
  of your connections, naming its host and user.

### 3.3 The guard, rebuilt around the match

`flyway_migrate` and `flyway_repair` take a **project id and an environment**,
not a connection id. Before running Flyway they re-read the file and refuse if:

1. **The environment no longer exists** in the file.
2. **Any matching connection is read-only.** The refusal names it. Stage 14's
   flag keeps its meaning: marking your prod connection read-only stops this app
   from writing to prod, migrations included.
3. **The target moved since you confirmed.** The confirmation dialog is built
   from the environment's URL and user. The command receives those back and
   refuses if the file now says something else — for example, a branch switched
   between the dialog opening and the button being pressed. The confirmation
   has to describe what actually runs.

Gone: `Disagreement` as a refusal reason, `flywayAccepted`, "Attach anyway",
and re-attaching.

### 3.4 Storage

`flyway_projects.json` in the config directory:

```json
{ "version": 1, "projects": [ { "id": "p1-…", "path": "C:\\repo\\flyway.toml", "environment": "uat" } ] }
```

* **Paths only.** Stage 15 §3.3 still holds: the TOML changes with the branch,
  so it is re-read every time and never copied. `environment` is the last one
  selected — a UI preference, not a safety fact.
* **No secrets, by construction.** There is no field that could hold one, and
  it is asserted with an exact key set, the same way as `PROFILE_KEYS`.
* Atomic writes, and a corrupt file is moved aside rather than discarded — the
  same rules as `connections.json`, and the same code where it can be shared.

`ConnProfile` loses `flywayProject`, `flywayEnvironment` and `flywayAccepted`,
and `PROFILE_KEYS` goes from sixteen keys to thirteen.

### 3.5 Nothing set up is lost

The first time the projects file is missing, it is seeded from whatever
`connections.json` has attached: each distinct `flywayProject` becomes a
project, and its `flywayEnvironment` becomes the selected environment. The old
fields are read from the raw JSON for this purpose only. `ConnProfile` no
longer has them, so the next save of a profile drops them.

### 3.6 What stays exactly as it is

The CLI runner and its working directory (Stage 15 §3.2, §3.4), the
Pending / Failed / Executed groups, both repair dialogs, out-of-order, opening a
migration's SQL in an untitled tab, and the "Migrations ·" source line. The
only changes are that these are now keyed by project and environment instead
of by connection, and each confirmation names the environment's host, user and
matched connection.

## 4. Milestones

| # | Claim | Evidence |
|---|---|---|
| F1 | Projects can be added, listed and removed with no connection open | UI test starting from a fresh app, nothing connected |
| F2 | An environment shows the saved connection it matches, or says it matches none | Rust unit tests on matching; UI test on the labels |
| F3 | A read-only match refuses apply and repair, in Rust | Unit test on the guard; UI test that the buttons are disabled with a reason |
| F4 | Several matches, one of them read-only, are treated as read-only | Unit test |
| F5 | An apply confirmed against one target refuses if the file now names another | Unit test on the guard |
| F6 | After an apply, a matched connection that is open has its schema cache for the migrated database dropped | UI test on the call sequence |
| F7 | Existing attachments become projects on first launch | Rust test over a legacy `connections.json` |
| F8 | The projects file holds no secret: exact key set | Rust test, same shape as `PROFILE_KEYS` |
| F9 | Unchanged behaviour survives: groups, repair dialogs, out-of-order, open SQL | The Stage 15 UI tests, ported rather than deleted |
| F10 | Hands-on against the user's real project | The user |

## 5. Decisions, and the alternatives turned down

### 5.1 Why not keep attaching, and just fix the guard?

Stage 16 §17 did fix it. That made the coupling *work*. It did not stop the
coupling from *implying* something false, which is the user's point. A UI that
has to explain that the connection you are looking at is not the one being
changed is a UI with the wrong shape.

### 5.2 Why match rather than let the user pick the connection?

Picking would be a stored claim ("uat is my UAT connection") that goes stale on
a branch switch. Matching is recomputed from the file every time, so it cannot
be stale, and it needs nobody to remember to update it.

### 5.3 Why allow an environment that matches nothing?

Because the user's migration account is often not the account they browse with.
Refusing would recreate the "Attach anyway" dead end in a new place. What
matters is that the confirmation says the target is not one of their
connections. Unmatched means *unlabelled*, and that is exactly what the
confirmation spells out.

### 5.4 Why not run Flyway with our connection's settings instead?

Asked for in Stage 16 §17 and turned down there, for three reasons:

* it would pass a keychain password to a child process;
* it would drop the file's JDBC options;
* it would change the default schema that unqualified objects land in.

Nothing here changes that answer.

### 5.5 `Capabilities.migrations`

The pane no longer looks at the active connection, so the frontend stops
reading this capability. Keeping it costs nothing, and if a second engine gains
Flyway support, this is where that will be declared.

## 6. Task tracker

### Phase 1 — Backend

- [x] `flyway_projects.json` store: load, save, corrupt-file handling, key-set test (F8)
- [x] Seed from legacy profile fields (F7)
- [x] `flyway::matches(env, profiles)` (F2, F4)
- [x] Guard: missing environment, read-only match, confirmed target (F3, F4, F5)
- [x] Commands keyed by project id + environment; `flyway_projects`, `flyway_add_project`, `flyway_remove_project`, `flyway_select_environment`
- [x] Remove the three `ConnProfile` fields and `flyway_check`

### Phase 2 — The pane

- [x] Always-available rail button; project and environment list with match labels
- [x] Info, apply, repair and open, keyed by project and environment (F9)
- [x] Confirmations name the host, the user and the matched connection
- [x] Drop the matched open connection's schema cache after an apply (F6)
- [x] Port `tests/ui/migrations.spec.ts`

### Phase 3 — Proof

- [ ] Live Flyway suite through the new commands
- [ ] Hands-on (F10)

## 7. Building it — 2026-09-18

Phases 1 and 2 in one pass. Four things worth keeping.

### 7.1 A late answer under the wrong environment

Flyway takes about ten seconds per `info` on the user's server (Stage 16's log:
`flyway_info ok in 9509ms`). Once environments are rows you can click, clicking
a second one while the first is still being asked about is the normal case —
and the first answer, arriving late, would be drawn under the second
environment, beside **its** Apply button. `showSelected` now drops any answer
for an environment that is no longer selected. The test delays `uat` by 1.5 s,
selects `development`, and checks that the list is still `development`'s
afterwards. With the check removed, it fails.

This could not happen in Stage 15's shape: one connection had one environment.
It is the one new risk that listing environments side by side introduces.

### 7.2 Nothing connected, and a migration to read

Opening a migration's SQL made an untitled tab, and a tab belongs to a
connection — so with nothing connected, the new normal, clicking a migration
did nothing but put `a tab must belong to a connection` in the grid. It now
opens in the read-only value viewer, which can neither run nor save anything
either. With a connection open, it still opens in a tab.

### 7.3 The confirmation is built from a fresh read

`freshTarget` re-reads the projects before every apply or repair dialog, and
the command receives back the URL and user the dialog showed (§3.3, F5). The UI
test changes the host between the list and the dialog and checks that the
dialog shows the new host, and that the new host is what gets sent.

### 7.4 Corrections to the plan

* `PROFILE_KEYS` went from sixteen keys to thirteen, not back to fifteen as
  §3.4 first said. Fifteen already included the two Stage 15 fields.
* `refreshMigrationsButton` survives, because `syncConnLabel` calls it, but it
  no longer reads the connection: the button is always shown and the pane is
  not redrawn when the active connection changes.

**374 Rust unit tests; 754 UI tests on both engines**, 38 of them the migrations
spec (was 37). The new tests for stale answers and the post-apply refresh both
fail when their fix is removed. The live Flyway suite does not exercise these
commands, since it drives `flywaycli` directly, so it was not re-run. That is
Phase 3.

## 8. Found in use, alongside this stage — 2026-09-18

Corrections that are not about Flyway, recorded here because this is the
current tracker.

### 8.1 A CTE was an "Unknown table"

> *"When using a CTE. is not recognized as a table if used on same script"*

The linter knew only the server's tables, so every name a `WITH` clause defines
was reported as missing. `cte_names` now collects them —
`WITH [RECURSIVE] a [(cols)] AS (…), b AS (…)` — and they count as tables that
exist for the rest of their statement. Their columns are whatever their
`SELECT` produces, which the linter does not work out, so nothing is said about
them. That includes a CTE that shadows a real table: its columns are not the
table's.

A name only counts once its `AS (` has been seen, so `WITH ROLLUP`, `WITH CHECK
OPTION` and `WITH GRANT OPTION` define nothing. A CTE does not carry over into
the next statement.

**Found on the way:** `ROLLUP` was not in the keyword list, so
`GROUP BY id WITH ROLLUP` on a single table reported "`users` has no column
`ROLLUP`". Added, with `RECURSIVE`.

Six tests. The report's own case and the "does not leak into the next
statement" case fail when CTE names are not treated as tables.
