# Stage 15 — Flyway migrations, on a connection that has them

**Status:** 🚧 Phase 1 built — 2026-09-10. F1-F4 met; apply and repair not started.

---

## 1. The use case, in the user's words

> "I have a TOML which I open with Flyway Desktop with the settings. This has
> the DB connection and the envs as well as the folder we are reading … with a
> click we can open the migration content so we can see what it has and
> run/execute the ones pending … we only apply migrations and repair when they
> go wrong, so those would be the main pieces."

The workflow today: switch to the branch for the environment you want, open
Flyway Desktop, point it at the right database, look at what is pending, apply.
The part this app can do better is the middle — seeing what is pending, reading
what each one contains, and knowing which server you are about to change.

## 2. Why this does not break the rule

**"We never run a statement the user did not ask us to run"** has held since
Stage 0. Applying a migration executes SQL — so it is worth being exact about
why this is allowed:

  * Nothing runs until a button is pressed, and the button names the versions
    it will apply.
  * Every migration's SQL can be opened and read first, from the pane that
    lists it.
  * The user's own framing, accepted here: *"Will not be a break of our rules
    as even when query is auto executed User has full access for review the
    content of each before."*

What would break the rule is applying on connect, on import, or on refresh.
None of those happen.

## 3. Design

### 3.1 Not an engine. A project attached to a connection.

The first proposal was a new `EngineKind` — Flyway as a connection kind whose
tree shows migrations instead of tables. Rejected, for three reasons:

  * **The `Engine` trait is the wrong shape.** It is `namespaces → tables →
    columns`, plus `table_ddl`, `routines`, `quote_ident`, `qualify`,
    `select_snippet`, `run_script`, `cancel`. A Flyway implementation stubs or
    lies about nearly all of it. Stage 11 built that trait so a *third
    database* is an addition; Flyway is not a database.
  * **A Flyway connection still has a real database underneath.** The TOML's
    environment *is* a MySQL connection. As a sibling engine you would need a
    second connection to the same server to look at the schema — and the
    migration you just applied would be invisible in the tree in front of you.
    `run_script` would have to open a MySQL connection anyway: a **wrapper
    around `MysqlEngine`, not a sibling of it**.
  * **Its capabilities would be almost all false.** When a new implementation
    has to answer "no" to nearly every question an interface asks, it is not
    that interface's implementation.

So: a connection on a compatible engine can have a **Flyway project** imported
into it. It keeps its schema tree, its editor, its history, its read-only flag,
and gains a migrations view.

### 3.2 The CLI does the work. We do not reimplement Flyway.

`flyway info -outputType=json` reports every migration with its `version`,
`description`, `state`, `installedOnUTC` **and the full path of the file** — so
"click to read the SQL" is answered by Flyway, not by us walking `locations`.
`migrate` and `repair` are Flyway's own. The exact field names are §9, measured
rather than read.

Reimplementing any of it means owning checksum rules, ordering and — worst —
`repair`, which rewrites the schema history. Being subtly wrong there is the
most destructive thing this app could do.

Flyway is Apache-2.0, and **we do not ship it**: the user installs it, we
invoke it. It enters neither the dependency graph nor the attribution manifest.

### 3.3 Store the path to the TOML, never a copy of its contents

The TOML lives in the repository and **changes with the branch** — that is how
the user targets an environment today. A snapshot of its values on our profile
would go stale the moment they switch, and claim the wrong folder or the wrong
environment with complete confidence.

So the profile stores the **path**, and every read re-parses. Switching branch
is then something we follow rather than something we have to be told about.

### 3.4 The working directory is the TOML's folder

    locations = [ "filesystem:../myproject/src/Flyway/MasterScripts" ]

Relative, and Flyway resolves it against the **process working directory**, not
the config file. It works for the user because they run Flyway from that
folder. So we run the CLI with its working directory set to the TOML's
directory — replicating what they already do, rather than inventing path
rewriting that would have to be right for every form a location can take.

### 3.5 One environment per connection, and the guard that makes it safe

A Flyway environment *is* a connection: `[environments.uat]` carries the `url`
and credentials. So "which environment" and "which database am I pointed at"
are the same question, and a connection answers it once. The chosen environment
is stored on the profile; the same TOML imported into three connections gives
dev, qa and uat on the rail, each its own colour.

**The danger this creates.** `flyway migrate -environment=uat` connects to the
URL *in the TOML*, not to our connection. Attach the wrong environment and you
would be watching one database while changing another — the exact thing this
app's colours, labels and read-only flag exist to prevent.

So at import the environment's JDBC URL is parsed and compared against the
connection's host, port, database **and user**, and a mismatch refuses to
attach unless it is deliberately overridden. The user field is not optional
here: environments commonly share a server and differ only by account.

Belt and braces, because a guard can only compare what differs: the chosen
environment's `displayName` is shown in the migrations pane and on every apply
confirmation. Visibility is what protects the cases matching cannot.

### 3.6 We never read a password

Flyway TOML environments carry `password` inline. We do not read it and do not
copy it anywhere: Flyway reads its own config for its own connection, and our
SQL connection keeps using the keychain as every connection does. **Phase 1
touches no secret at all.**

### 3.7 Parse what we use; tolerate everything else

The real file carries `[flywayDesktop]`, `[redgateCompare]`,
`callbackLocations`, `schemaModelLocation`, and a `shadowEnvironment` naming an
environment the file does not define. Validating it would reject a config that
works. We read the environment table, `[flyway] environment`, and
`[flyway] outOfOrder`, and ignore the rest.

## 4. What this stage is not

- **Not a Flyway Desktop replacement.** No schema model, no compare, no
  generate, no undo.
- **Not a migration editor.** Files are read, not written.
- **Not Flyway bundled.** The user's own CLI, at a path they can set.
- **Not a second execution path.** Applying goes through Flyway, because
  running the SQL ourselves would leave `flyway_schema_history` wrong —
  ranks, checksums, execution times — and we would have quietly become a
  broken Flyway.

## 5. Milestones

- [x] **F1 — A project imports.** Point at a real `flyway.toml`, pick an
      environment, and the connection remembers it across a restart.
- [x] **F2 — The list is real.** Applied and pending migrations, with state and
      installed-on, from `flyway info` against the fixture.
- [x] **F3 — A migration opens.** Click one, read its SQL in a tab. Nothing runs.
- [x] **F4 — The guard bites.** Attaching an environment whose URL does not
      match the connection is refused, and says which field disagreed.
- [ ] **F5 — Apply works, and is deliberate.** Pending migrations apply through
      `flyway migrate`; the confirmation names the versions; out-of-order is a
      choice made at apply time, defaulted from the file.
- [ ] **F6 — Repair works on a genuinely failed migration**, and a failure
      shows Flyway's own words.
- [ ] **F7 — Read-only refuses.** A connection marked read-only offers neither
      apply nor repair.

## 6. Task tracker

### Phase 1 — Import, list, read (nothing writes)
- [x] `toml` crate, licence read from the vendored source before it is added
- [x] Parse: environments (`url`, `user`, `displayName`), `[flyway] environment`,
      `outOfOrder`
- [x] JDBC URL parsing (`jdbc:mysql://host:port/db?params`), params discarded
- [x] `flywayProject` + `flywayEnvironment` on `ConnProfile`, and the
      `PROFILE_KEYS` decision that comes with them
- [x] A `migrations` capability — true where Flyway can drive the engine
- [x] Invoke the CLI: arguments as a vector, never a shell string; working
      directory the TOML's folder; a settable path; an honest error when it is
      not installed
- [x] `info -outputType=json`, parsed defensively
- [x] The migrations pane, and opening a migration's file in a tab
- [x] The import guard (§3.5)

### Phase 2 — Apply
- [ ] `migrate`, behind a confirmation naming the versions
- [ ] Out-of-order at apply time, defaulted from the file
- [ ] Read-only refuses

### Phase 3 — Repair
- [ ] `repair`, behind a stronger confirmation
- [ ] Flyway's own error text, verbatim

## 7. The fixture, and why it is not a container

Testing needs a real CLI and a real failed migration. Flyway publishes a Docker
image, and **using it would prove the wrong thing**: in a container the paths
are mounts, the working directory is the container's, and the database host is
not the one the TOML names — every part of §3.4 that is most likely to be wrong
would be replaced by something the app never does.

So: the real command-line tool on the host — its tarball ships its own `jre/`,
so nothing else has to be installed — pointed at the **MySQL fixture we already
have**, with a sample project under `dev/flyway/` shaped like the real one: a
few environments, `locations` given as a **relative** path so §3.4 is exercised,
and a migration that **fails on purpose**, because F6 needs something genuinely
broken to repair.

## 8. Risks

- **A new class of surface: this app has never spawned a process.** An external
  binary at a user-supplied path. Arguments as a vector, no shell, and the
  failure when it is absent has to be plain rather than mysterious.
- **Version coupling.** TOML configuration is Flyway 10+, and the JSON shape
  can move. Parse defensively; show Flyway's own text when it disagrees with us.
- **The guard is only as good as what differs.** §3.5's second half exists
  because of that.
- **Scope.** This widens the app from "talk to a database over its protocol" to
  "drive a tool and show its state". Said out loud here so it is a decision
  rather than a drift.

---

## 9. What the CLI actually says — measured 2026-09-10, Flyway 13.5.0

Run against the fixture, not read from documentation, and it matters: **the
documentation I had been working from was wrong in three places.**

### `info -outputType=json`

    top level  allSchemasEmpty database exception flywayVersion licenseFailed
               migrations operation schemaName schemaVersion timestamp
    migration  category description executionTime filepath installedBy
               installedOnUTC rawVersion shouldExecuteExpression state type
               undoFilepath undoable version

  * The path is **`filepath`**, not `script` — and it is absolute, which is
    what makes §3.2's claim ("Flyway answers where the file is") true.
  * The timestamp is **`installedOnUTC`**, not `installedOn`.
  * **There is no `checksum`.** The tracker claimed one before this was run.
    Nothing needs it — but a plan that says a field exists is a plan somebody
    will later write code against.

`state` is `Pending`, `Success` or `Failed` in the cases the fixture produces.

### `migrate -outputType=json`

Two **different shapes**, and code that assumes one will panic on the other:

    succeeded   migrationsExecuted, migrations[], success, totalMigrationTime,
                targetSchemaVersion, warnings[]
    refused     { "error": { "errorCode": …, "message": … } }   — and nothing else

The second is what a run against a history with a failed migration returns:

> Validate failed: Migrations have failed validation. Detected failed migration
> to version 4 (deliberately broken). Please remove any half-completed changes
> then run repair to fix the schema history.

Which is exactly the text F6 should put in front of the user, verbatim. Note
also that a run which **partly** succeeds reports `"success": false` with
`migrationsExecuted: 3` — "did it work" is not a yes/no, and the UI has to say
how far it got.

### `repair -outputType=json`

    database flywayVersion migrationsAligned migrationsDeleted
    migrationsRemoved operation repairActions warnings

`migrationsRemoved` carried `{version: "4", description: "deliberately broken"}`
— so the confirmation after a repair can say what it did rather than "done".

### Exit codes and streams

**Exit 1 on failure, 0 on success. All output on stdout; stderr was empty even
for the failure.** So the JSON is the thing to parse and the exit code is the
thing to trust — and a first attempt at measuring this read `$?` after a pipe
and got `tail`'s status, which is the sort of mistake that turns into "why does
it think the migration worked".

## 10. Phase 1 groundwork — 2026-09-10

`src-tauri/src/flyway.rs`: the project file parser, the JDBC URL reader and the
import guard, with the user's own (redacted) file as the fixture in its tests —
including its two awkward properties, environments that differ only by user and
a `shadowEnvironment` naming an environment the file never defines.

**`toml` costs nothing.** `tauri` already links toml 1.1 through `tauri-utils`
at runtime, so cargo unifies it: `cargo tree -i` confirmed it before the line
was added, and the lockfile diff is **one line**. Licence read from the
vendored `Cargo.toml`: MIT OR Apache-2.0.

**No type in that module has a password field**, the same construction
`ConnProfile` uses — so a secret from the project file cannot be logged,
serialised or shown by accident, and a test asserts the parsed value's debug
output does not contain one.

The fixture is `mise run flyway-up`: the pinned CLI (13.5.0, matching the
version Flyway Desktop ships as 13.5.0-rc2720) plus `dev/flyway/`, whose
`locations` is deliberately **relative** and whose `V4` fails on purpose.

---

## 11. The runner, and the test that catches the mistake — 2026-09-10

`src-tauri/src/flywaycli.rs` runs the CLI and reads what comes back.

**The first process this app has ever started**, so three rules are written
into the module rather than left to habit: arguments are a **vector**, never a
shell string (a test passes `dev; rm -rf /` as an environment name and asserts
it stays one argument); the **working directory is the project's folder**; and
a missing binary is an ordinary answer that says where to fix it, because most
people will not have Flyway on their PATH the first time.

`std::process::Command` on the blocking pool, not `tokio::process` — that
feature pulls tokio's unix signal machinery for what is one short command run
when somebody clicks a button.

**A refusal is looked for before the success shape**, because a `migrate`
against a failed history returns `{"error": {…}}` and *nothing else*: code that
reads `migrations` first sees an empty run rather than a refusal.

### What the live suite is for

`tests/live_flyway.rs`, `#[ignore]`d beside the MySQL and Elasticsearch ones,
run by `mise run test-flyway`. Four things a JSON parser cannot tell you:
that the arguments we build are arguments **Flyway accepts**, that a relative
`locations` **resolves**, that `-environment=` really **picks the database**,
and that an unknown environment comes back as Flyway's own complaint.

The middle two are the ones that matter, and this is why:

> **A wrong working directory does not look like an error.** Flyway finds no
> migrations and reports a *successful* run of an empty project.

So the test was checked by breaking it on purpose — running from a temporary
directory with an absolute `-configFiles` so only the working directory
changed. The two tests that read migrations failed; the two that do not depend
on `locations` kept passing. That is the shape a real regression would have.

**324 Rust unit tests, 4 live Flyway.**

---

## 12. The pane — 2026-09-10

A fourth column on the shell grid, with its own splitter and a rail toggle.
Two extra tracks that are **zero wide when it is closed**, so the layout is the
same grid whether or not Flyway is in use rather than two layouts to keep in
step.

A pane rather than a section of the schema tree because of what it is for:
reading a migration while the database structure it changes is still on screen.

The toggle follows `capabilities.migrations`, so it is absent on Elasticsearch
— and would appear on the next engine Flyway supports without a line changing
here. It requires a live connection, since capabilities arrive with one; that
is the cost of not asking the engine's name.

**The environment is never out of sight.** §3.5's guard can only compare what
differs, so the cases it cannot catch are answered by saying which environment
this is, permanently, above the list.

Clicking a migration opens its SQL in an **untitled** tab, not one bound to the
file. This stage reads migrations; a tab carrying the path would make Ctrl+S
overwrite one, and editing an applied migration changes its checksum and breaks
the next validation.

### Two things the tests found

**A temporal dead zone.** `migrationsOpen` was declared beside the rest of the
migrations code at the foot of `main.ts`, and `syncConnLabel` runs while the
module is still initialising — so every boot threw *"Cannot access
'migrationsOpen' before initialization"* before a single test ran. The
declaration moved up beside `connected`.

**A fixture that had stopped mirroring Rust.** `MYSQL_CAPS` in the test harness
is a hand-written copy of what MySQL declares, and adding a capability in Rust
does not touch it — so the button was hidden in every test while being visible
in the app. Rust has a test listing the keys the frontend is promised; the
harness has nothing equivalent, and this is the second time that gap has cost a
confusing failure.

**626 UI tests on both engines, 324 Rust unit, 4 live Flyway.**
