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
- [x] **F5 — Apply works, and is deliberate.** Pending migrations apply through
      `flyway migrate`; the confirmation names the versions; out-of-order is a
      choice made at apply time, defaulted from the file.
- [x] **F6 — Repair works on a genuinely failed migration**, and a failure
      shows Flyway's own words.
- [x] **F7 — Read-only refuses.** A connection marked read-only offers neither
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
- [x] `migrate`, behind a confirmation naming the versions
- [x] Out-of-order in the pane, defaulted from the file, re-asking Flyway
- [x] Read-only refuses

### Phase 3 — Repair
- [x] `repair`, behind a stronger confirmation
- [x] Flyway's own error text, verbatim

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

## 13. What the first real user found — 2026-09-16

Three reports, from the first hands-on pass against a real installation. Two
were shipped defects; the third is the feature that pass asked for.

### 13.1 A shared class, redefined

> "theres an odd deformed color after the connection name"

The colour dot beside the connection name was a **72px ellipse**, and the Run
button was a bar across the whole toolbar. Both since §12, from one line:

```css
.pane-head { … font-weight: 600; font-size: 12px; … }
.pane-head > :first-child { flex: 1; min-width: 0; }
```

The migrations pane's head wrote its wants against `.pane-head` — a class the
sidebar head and the editor toolbar have worn since Stage 1. `.conn-dot` sets
`flex: 0 0 auto`, but `.pane-head > :first-child` is the more specific
selector, so the dot grew. The styles are now scoped to `#migrations-pane` and
`#mig-title`.

**Why no test caught it.** Every test here asks about behaviour — is it
visible, does it say the right thing, does clicking it do the right thing —
and none asks about *shape*. A 72px dot is visible, it is in the right place,
and it says nothing. The regression test added measures the box: a round dot
is square, and Run is narrower than half its bar.

The general lesson is about **editing a class rather than a pane**. Nothing in
`#migrations-pane .pane-head { … }` could have leaked; `.pane-head { … }` in a
file section headed "the migrations pane" reads exactly the same and is not.

### 13.2 Windows cannot find `flyway.cmd`

> "the flyway from flyway desktop is not being recognized on windows (which
> uses flyway.cmd file)"

Not a configuration problem. `std::process::Command` on Windows searches the
PATH by appending `.exe` **and only `.exe`** — from `resolve_exe` in `std`:

```rust
let has_extension = exe_path.as_encoded_bytes().contains(&b'.');
let result = search_paths(parent_paths, child_paths, |mut path| {
    path.push(exe_path);
    if !has_extension {
        path.set_extension(EXE_EXTENSION);
    }
    program_exists(&path)
});
```

`PATHEXT` is never consulted. The Flyway distribution ships `flyway.cmd`, so
`Command::new("flyway")` reports *program not found* for a Flyway sitting on
the PATH in plain sight. The same rule is why `Command::new("npm")` fails on
Windows and has surprised people for a decade.

So `flywaycli::resolve` does the search: `.cmd`, `.bat`, `.exe`, then the bare
name, every spelling within one directory before moving to the next. Handing
the resolved `.cmd` back to `Command` is safe — `std` spots the extension and
runs it through `cmd.exe` with batch-specific quoting, which is the fix for
CVE-2024-24576 and the reason this does not build a command string of its own.

`EXTENSIONS` is empty everywhere else, and `resolve` returns early: `execvp`
already searches the PATH and there is nothing to guess, so Linux and macOS
keep exactly the behaviour they had.

**Tested on a machine that is not Windows.** `candidates` takes the PATH and
the extension list as arguments, so the Windows rules are five ordinary unit
tests here. The one that would have caught this in the first place:

```rust
let c = candidates("flyway", &dirs(&["C:\\tools"]), WIN);
assert_eq!(c, dirs(&[
    "C:\\tools/flyway.cmd", "C:\\tools/flyway.bat",
    "C:\\tools/flyway.exe", "C:\\tools/flyway",
]));
```

### 13.3 A report, because the message was unanswerable

> "any way we could generate maybe a log with the error so we can share here
> and debug from there?"

Yes, and it should have existed from the start. "Flyway was not found" is a
sentence nobody can act on from anywhere but the machine it happened on, and
it is not even true in the case above.

**Settings → Integrations → Test Flyway** produces text:

```
db-query — Flyway diagnostics
(paths only, no passwords — redact if you like)

app        0.4.2
os         windows x86_64
setting    (empty — using the default, "flyway")
search     164 candidates, Windows appends only .exe so .cmd is tried here
found      C:\Users\…\flyway\flyway.cmd
running    "C:\Users\…\flyway\flyway.cmd" -v

exit       0

--- stdout ---
Flyway Community Edition 13.5.0 by Redgate
… and 31 more lines
```

Three decisions worth keeping:

* **It runs the same code path it reports on.** `run` and `diagnose` share one
  `spawn`, so a report saying the program was found cannot come from a
  resolver the real path does not use.
* **It carries no secret.** The project file, the JDBC URL and the connection
  are not in it. It does carry file paths and, on Windows, effectively the
  PATH — which is why the second line says so.
* **It is capped.** `flyway -v` prints a forty-line plugin table for databases
  nobody here uses. A report too long to paste answers nothing.

The misses are only listed when nothing was found, which is the only time they
are the answer.

**And a test that proved nothing.** The first version of the UI test filled the
path box and clicked the button, then asserted the backend was asked about the
typed path. It passed with the handler's `commit()` deleted: Playwright's
`fill` fires a `change` event, which is already wired to `commit`, so the
setting was saved on the way past no matter what the button did. It now sets
the value without dispatching `change`, and fails — `""` instead of the path —
when the handler stops committing. Proving a new test can fail is the step
that catches this, and it is worth doing every time.

### 13.4 The schema pane collapses

> "let's make the schema list collapsible as today we can setup the width but
> maybe a button to collapse on a second vertical row similar to connections"

A rail toggle beside the migrations one, and **Ctrl+B**. The width the
splitter was dragged to is untouched: collapsing overrides the grid *track*,
so reopening restores the width rather than resetting it.

Not remembered across launches — nor are the splitter positions or the
migrations pane. Starting with the schema hidden and no memory of having
hidden it is a worse first second than reopening it.

**The bug this grew.** The first version hid the pane with `hidden`, and every
panel on the shell grid was auto-placed. Auto-placement counts only the items
that are *displayed*, so removing two grid items slid `#main` two columns left
— into the zero-width track the schema had just vacated. The pane collapsed
and took the editor with it. Every panel now names its column.

That is what the test asserts, and why it asserts the wrong-looking thing: not
that the schema is hidden, but that **the editor got the width**.

```ts
const after = (await page.locator("#main").boundingBox())!.width;
expect(after).toBeGreaterThan(before + 200);   // was 0
```

Also fixed in passing: `.rail-cog.on` was set by the migrations toggle from
§12 onwards and styled by nothing, so a pane toggle looked identical whether
its pane was open or shut. Both now carry the accent bar the rail already uses
for the active connection.

## 14. Phase 2, and three things the second hands-on pass found — 2026-09-16

The Windows fix held: Flyway resolution found the user's `flyway.cmd`. What
came back with that confirmation was three pieces of feedback, and each one is
the kind that only a real project produces.

### 14.1 A tab that lied about where it came from

> "Icon on tab when clicking a migration file says its a file from MCP which is
> wrong (also icon is sort of weird looking is it an arrow?)"

Both true, and the second explains the first. Provenance was a boolean:

```ts
/** This tab arrived through the MCP server rather than being opened here. */
external: boolean;
```

MCP was the only thing that could set it, so the flag and the reason were the
same fact and the mark could hard-code *"Added by an MCP client"*. Stage 15
opened migrations through the same door, and every migration inherited a
sentence about a protocol it had never touched. **A fact with two possible
answers does not fit in a flag.**

It is now `origin: "own" | "mcp" | "migration"`, a closed set, with each value
owing the strip a mark and a sentence — so a value nobody wrote those for
cannot be added silently. The `\u2197` is gone: MCP gets an arrow into a tray,
a migration gets the layers glyph the rail already uses, both from the app's
own icon set at the app's own stroke width.

The session file keeps `external` as a **read-only legacy key**: the frontend
maps `external: true` onto `origin: "mcp"` so upgrading does not erase the mark
from tabs that were already open. An `origin` this build does not recognise is
carried through a load and save rather than dropped, so running an older
version once cannot quietly erase one.

### 14.2 Grouped by what an apply would do

> "a way to quickly collapse the migrated vs pending (Skipped or missing can be
> considered as success ones in the sense that they will not be executted if we
> go to an apply)"

The parenthesis is the design. Flyway has a dozen states and most of them —
`Ignored`, `Superseded`, `Above Baseline`, `Missing` — differ in *why* they
will not run rather than in whether they will. Before an apply, the only
question is **will this run**, so that is the axis:

* **Failed** — blocks everything until repaired. Open.
* **Will run** — what `migrate` will execute, in order. Open.
* **Will not run** — applied, skipped, superseded, baselined. Collapsed, because
  it is the long one and the least urgent.

`Group` is computed in Rust from the state and travels with each migration, so
the renderer never learns Flyway's vocabulary. **`Done` is the default for a
state this build has never seen**, and the asymmetry is deliberate: claiming
something will not run and being wrong shows up as an extra line in what Flyway
reports it executed. Claiming something *will* run and being wrong is a
confirmation dialog that lied about what it was asking permission for.

`Ignored` lands in "will not run" — correctly, *while out-of-order is off*.
Turn it on and Flyway reports the same migration as `Pending`. Which is why:

### 14.3 Out of order belongs in the pane, not in the dialog

The obvious design is a checkbox in the confirmation. It is wrong, because
toggling it changes the list the confirmation is naming — a dialog that has to
re-query itself while open.

So it sits above the list, defaulted from the project file's `outOfOrder`, and
toggling it **re-runs `info` with the same flag the apply will use**. The "will
run" group visibly changes. Nothing is guessed: what the pane calls pending is
what Flyway called pending under exactly these flags.

### 14.4 No way to see or change what was attached

> "there seems to be no way to change the settings we have imported into
> current connection so if I ever want to se which folder this is targetting or
> just change the toml I cannot."

A connection could be pointed at a `flyway.toml` on some branch and the only
evidence was the migrations it listed. The pane now shows the project's name,
the file and its folder, and the environment — with **Change…** offering the
three things that can be done: change environment, change project file, detach.

Changing the environment goes through the **same guard** as an import, because
it is the same risk. And the project file is now **re-read on every render**
rather than remembered: it lives in a repository and changes with the branch,
so its name, its environments and its out-of-order default are only true as of
now. A file that has gone says so and offers a way out, instead of showing an
empty list.

### 14.5 Apply

The one place this app runs SQL nobody typed. The user's own framing of why
that is not a broken rule: *"even when query is auto executed User has full
access for review the content of each before"* — and every migration named in
the confirmation can be opened and read from this pane first.

**The confirmation names the versions rather than counting them.** "Apply 3
migrations?" is a question about arithmetic; `V5 add index` / `V6 add audit
table` is a question about which changes.

Three guards, in this order, in Rust rather than only in the UI:

1. A read-only connection refuses (F7), in the connection's own terms — it is a
   choice the user made and can unmake — and *first*, so somebody is not sent
   to fix the wrong thing.
2. **The environment guard runs again.** It ran at import, but the TOML lives in
   a repository and changes with the branch, which is this feature's whole
   workflow. The file that was checked is not necessarily the file about to be
   used.
3. The caller has confirmed, in front of the list.

They are `flyway::apply_refusal`, extracted from the command so they are five
ordinary unit tests rather than something reachable only through a Tauri
handle.

The button is hidden with no project, and **disabled with a reason** otherwise:
nothing pending, connection read-only, or a failure blocking everything. A
button that is simply missing makes people wonder whether the feature exists;
one that is enabled and then refuses wastes a confirmation.

### 14.6 Measured against a real database

`live_flyway.rs` applies to the fixture for real. One run covers both halves,
because the fixture's fourth migration is broken on purpose: three apply, the
fourth fails, and the assertions are on Flyway's own words.

```rust
assert_eq!(c.code.as_deref(), Some("FAILED_VERSIONED_MIGRATION"));
assert!(c.message.contains("Can't DROP 'weight'"));
assert_eq!(v["migrationsExecuted"], 3);
```

A second apply is then refused outright with `VALIDATE_ERROR` and *"run repair
to fix the schema history"* — and that output **has no `migrations` key at
all**, which is why `complaint()` is read before the success shape. Phase 3 is
what that instruction is asking for.

### 14.7 Two bugs the new tests found before the user did

**The pane kept showing the previous connection's migrations.** Switching
connection ran `refreshMigrationsButton`, which hid or showed the rail toggle
and nothing else — so the list stayed, under the new connection's name, beside
the new connection's schema. Every row in it was an invitation to apply
something to a database it did not belong to. Found by the test for §14.3,
which switches connections to check that out-of-order does not travel with the
window; it could not get past the stale pane.

**Out-of-order was one variable for the whole app.** It is now keyed by
connection. A single flag would have carried the choice made on a dev
connection onto a UAT one the moment somebody clicked across, silently changing
which migrations the next confirmation would name.

**And a temporal dead zone, for the second time.** `migrationsShowing` went in
beside the migrations code at the foot of `main.ts`, exactly where
`migrationsOpen` went in Stage 15 (§12), and broke every boot the same way:
`refreshMigrationsButton` runs from `syncConnLabel` while the module is still
initialising. Twice is a pattern rather than a slip — anything
`refreshMigrationsButton` or `syncConnLabel` touches must be declared at the
top of the file with `connected`, and both declarations now say so.

### 14.8 A flake that cost fifteen other tests

The first full run after Phase 2 came back with **sixteen WebKit failures**, and
fifteen of them had no error message at all — only a trace artifact that could
not be written. That shape is the tell: one test hung, its worker died, and
Playwright reported everything queued behind it as failed.

The one that hung was mine. Three of the new logbook tests clicked `#conn-ok`
without waiting for the dialog, where every other test in the suite goes
through a helper that waits both ways. It passed alone and hung under a full
parallel run.

Worth recording because the failure list was actively misleading: the
`tree.spec` and `regressions.spec` names in it had nothing wrong with them, and
chasing any of those would have been an afternoon. **Sixteen failures with one
error message between them is one failure.**

**And then it happened again, with a different cause.** The next full run came
back with eight WebKit failures in a different file cluster, all passing in
isolation, and this time with *no* error message anywhere in the log — a
crashed worker rather than a hung one. The variable was the fixtures: the live
Flyway tests need MySQL and a JVM running in podman, and eight WebKit workers
on a 13 GB machine do not have room alongside them. Stopping the containers and
re-running: **680 passed, none failed.**

Worth knowing before a run is believed. `mise run test-ui-all` and
`mise run test-flyway` are cheap separately and expensive at the same time, and
the failure they produce together looks like a product defect in whichever
files the dead worker happened to be holding.

### 14.9 Left undone, deliberately

**The schema tree is not refreshed after an apply.** A migration usually
changes the schema, so the tree beside it goes stale, and the success message
says so rather than pretending otherwise.

Refreshing it automatically needs to know *which* database was changed. The
only honest source for that is the environment's JDBC URL, which this side does
not have — and the refresh itself is bound to a per-database node in the tree
rather than existing as a function anything can call. Inferring the database
here would be a guess, and a refresh of the wrong one is worse than no refresh:
it looks like it worked. Left as a note rather than half-built.

### 14.10 Found on the way

`setMessage(html: string)` took a parameter called `html` and set
`textContent`. Safe, and a standing invitation to somebody "fixing" it with
`innerHTML` — on a path that now carries Flyway's error text. Renamed, and the
message is `pre-wrap` so Flyway's layout survives: it puts the file on one
line, the SQL state on the next and the database error after that, and
collapsing that into a paragraph loses the shape that makes it readable.

**680 UI tests on both engines, 376 Rust, 8 live Flyway.**

## 15. Phase 3: repair — 2026-09-16

### 15.1 What repair actually does, measured

Everything below was read off Flyway 13.5.0 against the fixture on 2026-09-16,
not from the documentation. Starting from a history with V1–V3 applied and V4
failed:

```json
{ "migrationsRemoved": [ { "version": "4", "description": "deliberately broken" } ],
  "migrationsDeleted": [], "migrationsAligned": [],
  "repairActions": [ "Removed failed migrations" ] }
```

Exit 0, no `error` key. And afterwards:

```
history:  1 ✓   2 ✓   3 ✓          (the failed row is gone)
widgets:  id, name, made_on, colour (unchanged)
info:     4 | Pending
```

**Three lists, not a count.** `removed`, `deleted` and `aligned` are three
different things happening to a schema history, and `aligned` in particular is
a different problem wearing the same button: it rewrites checksums to match
migrations that were *edited after they ran*.

A repair with nothing to repair is a success with every list empty and
`repairActions: []`. That is why `Repaired::is_empty` exists — reporting it as
"repaired" would tell somebody their problem was fixed when nothing was
touched. The UI says *"Flyway found nothing to repair"* instead, and the test
for it fails with the message that would otherwise have shipped: `Repaired: .
The database itself is unchanged.`

### 15.2 The confirmation is the feature

`repair` sounds like it fixes the database. **It does not**, and the gap between
those two is the entire risk in this phase. Flyway's own refusal says so —
*"Please remove any half-completed changes then run repair to fix the schema
history"* — and that instruction is worthless to somebody who thinks repair is
what removes them.

MySQL has no transactional DDL, so a migration that ran three of its five
statements before failing really did run three of them. Clear the history row,
apply again, and Flyway runs that migration from the first statement, on a
database where the first three have already happened.

So the dialog says three things in this order: **what it removes**, **what it
does not undo**, and **what happens next**.

> Flyway will remove the failed entry from this environment's schema history:
>
> **V4 deliberately broken**
>
> It does not undo anything the migration already did. If it ran some of its
> statements before it failed, those changes are still in the database — check
> them yourself first.
>
> Afterwards the migration counts as pending again, and the next apply will run
> it from the start.

Cancel is the default button. "Repair uat" names the environment, like Apply
does.

### 15.3 Proved where it counts

The claim the dialog stakes itself on is *"it does not undo anything"*, so the
live test proves that with `SHOW COLUMNS` rather than with Flyway's own report
of itself:

```rust
assert_eq!(
    sql("flyway_qa", "SHOW COLUMNS FROM widgets;"),
    before,
    "repair must not have altered the table"
);
```

— and that the failed row is gone, the successful ones are not, and V4 is back
in the `Pending` group where the next apply will find it.

### 15.4 The button, and F7's other half

Repair is **hidden unless something has failed**. It is not a maintenance
button somebody might press to see what it does; offering it against a healthy
history invites exactly that. The test proves it tracks the list rather than
being decided once: failed to begin with, hidden after the list comes back
clean — and Apply enabled at the same moment, which is the point of having
repaired.

**F7 was marked done a phase early.** The milestone says a read-only connection
offers *neither* apply nor repair, and Phase 2 only did the first half. Both
now go through one guard, `flyway::write_refusal`, which takes a `Write::Apply`
or `Write::Repair` and says which was refused — a connection that will not
repair should not be told migrations will not be applied, which is not what it
was asked to do.

The environment re-check applies to repair too. It writes less than an apply
does — one table rather than a schema — but it writes to the *wrong* database
just as easily.

### 15.5 One shared preamble

`flyway_target` loads the profile, its project path and environment, and parses
the TOML **as it reads now**. Apply and repair both start with it, so the
"re-read rather than remember" rule cannot hold in one command and lapse in the
other.

**692 UI tests on both engines, 380 Rust, 10 live Flyway.**
