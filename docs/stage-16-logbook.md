# Stage 16 — A logbook

**Status:** 🚧 Built — 2026-09-16

## 1. Why, in the user's words

> "More than a log exclusive for flyway, wouldn't be better a general log of
> our app so we can debug this and future issues?"

Yes. And the finding behind the question is worse than the question implies:
**this app has no logging at all.** One `println!`, inside a test. No
`console.*` anywhere in `src/`. In a packaged build the webview console goes
nowhere, so every frontend error on a user's machine is simply lost.

The header of `tests/ui/regressions.spec.ts` has been saying so since Stage 5:

> Every bug in this file was shipped, found by hand, and fixed before a stage
> was frozen. None of them was reachable from the Rust suite; **three were
> spotted only by reading the dev server's runtime log.**

That log does not exist outside `mise dev`. Stage 15's Flyway report was a
keyhole cut because there was no window.

**What a log would and would not have caught.** Of the three defects the first
Windows pass found (§13 of Stage 15), a log answers exactly **one** — Flyway
not starting. The stretched colour dot and the vanishing editor were shape
bugs; nothing writes "that dot is an ellipse" to a file. A logbook makes the
*backend* half debuggable from a distance, which is the half a maintainer on
another machine cannot otherwise reach. It does not replace the hands-on pass.

## 2. The rule that shapes everything here

Three of the 58 commands take a password or a token as an argument. A general
log is open-ended by design, which is the exact opposite of how the rest of
this app treats secrets: the config file is secret-free **by construction**,
not by filtering.

The precedent is already in the tree, and it is the right one. From
`history.rs`:

> `CREATE USER … IDENTIFIED BY 'x'` puts a password in the SQL text, and this
> project's rule is that secrets live in the keychain and nowhere else. Those
> statements are **skipped entirely** rather than redacted: a redaction that is
> subtly wrong writes the secret down anyway, and failing closed is the only
> direction worth failing in. The history dialog says so, so an absence is
> explained rather than mysterious.

Both halves of that carry over: **fail closed**, and **say that you did**.

## 3. Design

### 3.1 The API takes a sentence, never a structure

```rust
logbook::note(Level::Warn, "flyway", format!("{shown} not found after {n} candidates"));
```

There is no `logbook::note_debug(&args)`, and nothing derives its way into the
log. A call site says what happened in words it chose. **This is the property
that keeps passwords out** — not the scrubber, which is only there to catch a
mistake. An API that accepts a struct and formats it with `{:?}` is an API
that writes down whatever is added to that struct next year.

### 3.2 Fail closed, and say so

A scrubber runs over every message and **drops the whole entry** when it
matches a credential shape — a URL with `user:secret@`, `IDENTIFIED BY`, a
`Bearer` token, `password=<value>`. Dropped entries are replaced by a line
saying an entry was withheld and why, so a gap in the log is explained rather
than mysterious.

`Access denied for user 'root'@'localhost' (using password: YES)` must
survive: it contains the word and not the value, and it is the single most
useful line MySQL ever prints.

### 3.3 One ring, a file behind it

2000 entries in memory; a capped file in the config dir beside
`history.jsonl`, rotated to one `.1` predecessor at 1 MiB. The memory ring is
what the diagnostics button copies; the file is what answers "it happened this
morning".

### 3.4 What is instrumented

* **Every command**, from one wrapper in `api.ts` — name and duration and, on
  failure, the error. Never arguments; the wrapper is handed them and does not
  look. 58 call sites instrumented by one function.
* **`window.onerror` and `unhandledrejection`**, which is the gap that matters:
  in a packaged app these currently vanish.
* **What only Rust knows** — how Flyway was resolved and what it exited with,
  connection attempts, the MCP server starting and stopping, the updater.

### 3.5 The report absorbs the Flyway one

Stage 15's `flyway::diagnose` does not go away; it becomes a section. The two
answer different questions and both are wanted: **the log is history, the
report is a live probe** — what does resolution see right now, and what does
`flyway -v` say today.

## 4. What this stage is not

* **Not telemetry.** Nothing leaves the machine. The user copies text and
  decides who sees it.
* **Not a second query history.** `history.jsonl` records SQL, with credential
  statements skipped; the logbook records *that* a statement ran, its shape and
  its timing, and never its text.
* **Not a crash reporter.** A panic that takes the process down may lose the
  tail of the ring; the file is flushed per entry, which is the cheap 90%.

## 5. Milestones

- [x] **L1 — Entries land.** A capped ring with levels and areas, unit tested
      including the eviction boundary.
- [x] **L2 — It survives a restart.** The file, and rotation at a size the
      test actually crosses.
- [x] **L3 — Both sides write.** Every command logged with name and duration;
      `onerror` and `unhandledrejection` reach the same ring.
- [x] **L4 — A secret cannot get in.** The scrubber, and a test that runs a
      real connect with a known password and proves the string never appears
      in the ring or the file.
- [x] **L5 — One button.** Settings → Copy diagnostics: header, Flyway probe,
      log tail, in the viewer that already has Copy.

## 6. Task tracker

- [x] `chrono` as a direct dependency — already in the tree three ways via
      `rmcp`, `schemars` and `sqlx-core`, so no new crate. `MIT OR Apache-2.0`,
      read from the vendored `Cargo.toml` and `LICENSE.txt` on 2026-09-16.
- [x] `logbook.rs`: `Entry`, `Level`, the ring, the scrubber
- [x] The file, and rotation
- [x] `log_notes` command (a batch), and the `api.ts` wrapper
- [x] Frontend error handlers
- [x] Rust call sites: boot, and Flyway resolution and exit
- [x] `logbook::report()` — header + Flyway probe + tail
- [x] Settings → About → Diagnostics

## 7. What was built, and the three things it taught — 2026-09-16

### 7.1 The shape, as it landed

`logbook.rs` is a `VecDeque` of 2000 behind a `Mutex` in a `OnceLock`, spilling
to `db-query.log` in the config dir and rotating to one `.1` predecessor at
1 MiB. `note` takes `(Level, &str, impl AsRef<str>)` — words, never a
structure — and there is deliberately no entry point that takes arguments.

The frontend side is **one function**. `api.ts` now imports Tauri's `invoke` as
`sendToRust` and defines a local `invoke` that shadows it, so all 58 call sites
were instrumented without touching one of them:

```ts
async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const started = performance.now();
  try {
    const out = await sendToRust<T>(cmd, args);
    if (!ROUTINE.has(cmd)) note("debug", "ui", `${cmd} ok in ${…}ms`);
    return out;
  } catch (err) {
    note("error", "ui", `${cmd} failed after ${…}ms: ${String(err)}`, true);
    throw err;
  }
}
```

It is handed `args` and does not read them. That is the entire secret story on
this side, and the test that defends it drives the real connection dialog with
a password, proves the password *did* reach the backend, and then proves it is
nowhere in the log. Re-running it with `JSON.stringify(args)` added to that
line fails exactly as it should:

```
Received string: …"connect ok in 3ms {\"profile\":{…},\"password\":\"hunter2-do-not-log-me\"}"…
```

### 7.2 A scrubber that eats the useful lines protects nothing

The first version of `looks_like_a_secret` dropped this:

```
Access denied for user 'root'@'localhost' (using password: YES)
```

— the single most useful line MySQL ever prints, and the reason somebody opens
a log at all. It contains the word `password`; what it does not contain is a
*value*. The rules are now shape-based, matching a credential **next to its
value**, and `YES)` taught the rule that trailing punctuation belongs to the
sentence and not to the value.

So there are two tests, and the second matters as much as the first:
`a_credential_is_withheld_rather_than_edited` and `the_useful_lines_survive`.
A scrubber that cries wolf gets switched off, and then it guards nothing.

### 7.3 A threshold would have made the log lie differently on every machine

`save_session` is debounced at one a second while somebody types. Logging it
would push a session's worth of interesting lines out of a 2000-entry ring
during one long editing spell.

The first attempt suppressed anything that completed in under 5ms. That is
wrong in a way worth naming: it makes the log **different on a fast machine
than a slow one**, and different again under a stubbed backend, so what the
report contains depends on hardware rather than on what happened. It is now a
named list of three commands whose *success* is not news — and a failure of any
of them is always recorded, because "nothing happened, repeatedly" is what is
being suppressed, not "something went wrong".

### 7.4 The first run is the one that would have been silent

`config_dir` locates the config directory; it does not create it. That is made
by whichever module writes first — and on a brand new install, that is nobody
until a profile is saved. So the log would have failed silently for exactly
the person most likely to need it: somebody whose very first attempt at
something did not work. `open_in` creates the directory.

### 7.5 Two tests that only ran on Linux — caught by Windows CI

Both Windows failures were tests, not product. Both are worth recording,
because one of them was passing here for a reason that had nothing to do with
what it claimed.

**The easy one.** `directories_are_exhausted_one_at_a_time` compared
`p.display().to_string()` against a hand-typed `"a/flyway.cmd"`. A candidate is
a *path*, and its separator belongs to the platform — so the assertion tested
this file's spelling rather than the rule, and Windows correctly produced
`a\flyway.cmd`. Candidates are now built with `join` on both sides.

**The one worth thinking about.** `an_unwritable_file_is_counted_not_raised`
pointed `open_in` at `/definitely/not/a/directory/that/exists`. That premise
stopped being true the moment §7.4 taught `open_in` to create its directory —
and the test kept passing here anyway, because nobody may `mkdir` in `/`. It
was asserting a fact about root permissions on Linux, not about this code. On
Windows the same string is drive-relative, the directory was created happily,
the write succeeded, and the test failed — after littering `C:\`.

So the obstacle is now **a regular file where a directory has to go**, which
cannot be a directory on any platform and stays inside the temporary
directory. Verified the way every test here is: by removing the counting in
`append` and watching it fail.

The general lesson is not "be careful with paths". It is that **a change can
invalidate a test's premise while leaving it green**, and that the platform a
test is written on decides which of those it gets away with. Nothing here
tests the Windows rules on Windows except CI, which is exactly what CI caught.

### 7.6 What it does not cover

The batch flush is 200ms, so a hard crash can lose up to that much. Errors
flush immediately, which is the cheap mitigation and not a complete one.

And the honest limit from §1 stands: of the three defects the first Windows
pass found, this answers **one**. A log is for the half of the app that has no
pixels.

**654 UI tests on both engines, 367 Rust.**

## 8. The first thing the log could not answer — 2026-09-17

> *"we have something odd as I have deployed new version and make the release
> tag yet the updater is not showing the update is available"*

The release was fine. `v0.4.5` was published, not a draft, and the endpoint
served the right document with every signature in place:

```
GET https://github.com/SparedRay/db-query/releases/latest/download/latest.json
200 — {"version":"0.4.5", … 4 platform entries}
GitHub API: tag v0.4.5, draft false, prerelease false, published 15:04:35Z
```

Published five minutes before it was reported missing. The boot check runs
**once, at launch** (§5 of stage 5: nothing interrupts), so an app started
before 15:04 would never have seen it, and Settings → Check for updates was the
whole answer.

### 8.1 But nothing anywhere said so

That is the part worth fixing. Four different situations were producing the
same observable — *no update button* — and none of them wrote a line anywhere:

| what happened | what the user sees |
|---|---|
| this build cannot self-update at all (a `.deb`) | no button |
| the endpoint has nothing newer | no button |
| the check never completed (offline, proxy, DNS) | no button |
| the check found something | a button |

The frontend swallows a failed check on purpose — being offline is ordinary and
is not news — and `api.ts`'s funnel recorded `update_check ok in 412ms`, which
says the command ran and nothing about what it found.

**Silent to the user and silent in the log are different promises, and only the
first one was ever intended.** This is the logbook's own §2 rule applied to the
one command where the *answer* is the diagnostic rather than the failure:

```
update  running 0.4.4; the endpoint offers 0.4.5
update  running 0.4.5; the endpoint has nothing newer
update  running 0.4.5; this build does not self-update
update  the check did not complete: <Flyway-style verbatim error>
```

The wording lives in `update.rs` rather than inline in the command, for two
reasons. Most of it sits behind `#[cfg(target_os = "windows")]`, which no
machine here compiles — only CI does — so inline text would be unreachable to
every local test. And the value of these lines is precisely that they are
*distinguishable*; that is a property of the words, and nothing was checking
it. `a_check_says_which_of_the_outcomes_it_was` now does, and it fails when two
of them are made to read the same.

### 8.2 The report now rules out the package first

Diagnostics gained one line above the log:

```
updates    NOT this build — install the newer .deb by hand
```

A `.deb` never shows an update button, and from outside the machine that is
indistinguishable from a release that never went out. It is a property of how
the binary was packaged, knowable without a network call — so it is stated at
the top of the report rather than inferred from an absence at the bottom. The
test asserts the report cannot disagree with the build it came from.

### 8.3 What this still does not cover

The endpoint URL itself is not in the report. It is a build constant, CI has a
packaging test pinning it, and adding it would mean threading the Tauri config
through a module that deliberately has no handle on the app. If an endpoint
ever turns out to be wrong in the field, that is the line to add.

**708 UI tests on both engines, 382 Rust.**

## 9. A Stage 3 bug, found by using it — 2026-09-17

Recorded here because [Stage 3](stage-3-explore-and-export.md) is frozen.

> *"Exporting seems to be failing. After we do first export seems like stuck on
> Exporting..."*

Reproduced on the first try, and the failing test named it exactly:

```
locator resolved to <button disabled type="submit" id="export-ok">Exporting…</button>
```

The export itself was fine. The **button** was disabled before the dialog had
even been reopened, so the second export was unpressable and looked like the
first one had never finished.

### 9.1 The shape of it

Three paths leave an export — it worked, the save dialog was cancelled, the
backend refused — and the button was restored by hand at each one. Two of the
three did it. The successful path did not:

```ts
if (outcome) {
  showNote(describeExport(outcome, all));
  els.exportDialog.close();          // ← and nothing put the button back
} else {
  els.exportOk.disabled = false;     // cancelled: restored
  ...
} catch {
  els.exportOk.disabled = false;     // refused: restored
```

**Invisible at the time**, because the dialog closes over it. It waits for the
next export and presents as a stuck one. And the exit that was forgotten is the
one where nothing went wrong, which is the usual place to forget.

The fix is a `finally` and one `exportBusy(busy: boolean)` holding the two
states — which is how the connect dialog has done it since Stage 2, in the same
file, fourteen hundred lines up. This was not a missing idea; it was a hand-
rolled instance of an idea already present.

Every other busy button was checked: connect and the two update buttons use
`finally`, and `updateInstall` deliberately leaves *"Downloading…"* standing
because on Windows the installer takes over and the process may never return.
Export was the only one.

### 9.2 What the two tests had in common

Both existing exit-path tests were about *something going wrong*:
`cancelling the save dialog leaves the export dialog usable`, and
`a backend refusal is shown in the dialog, which stays open`. The third exit —
success — was tested for what it *wrote*, never for what it left behind.

The new test asserts the button is enabled and reads "Export", and then
presses it and asserts a second `export_csv` reached the backend. A button that
looks right and does nothing is the same bug wearing a different face.

### 9.3 And the logbook did not help

Worth saying plainly, in this tracker of all places. Nothing failed: no command
errored, no exception was thrown, and `export_csv ok in 31ms` is exactly what
the log said — twice, correctly, both times. §7.6 claims a log is for the half
of the app that has no pixels; this was the other half, and it took a
screenshot-shaped bug report and a test to find.

**710 UI tests on both engines, 382 Rust.**

## 10. The update dialog was showing its notes as source — 2026-09-17

Recorded here because [Stage 6](stage-6-updates-and-attribution.md) is frozen.

> *"the changelog modal that we show on an update does not render the markdown.
> is it intentional?"*

Half intentional, which is the interesting part. The notes were never *meant*
to be Markdown-shaped; they were meant to be safe. `choose()` sets its message
with `textContent` and says why, at the line that does it:

```ts
// textContent, not innerHTML: these messages carry file names, connection
// names and server errors, none of which we control.
```

That rule is right and is not being relaxed. But release notes are a GitHub
release body — the one the workflow itself writes, full of `##`, `**` and
fenced code — and a correct refusal to parse HTML had turned into showing a
document as its own punctuation.

### 10.1 Why a renderer and not a library

`src/markdown.ts`, about 150 lines, no dependency. Two reasons, and the second
is the one that decided it.

A Markdown library would be far more capable, and would bring a licence to
audit, a supply chain to trust, and — every one of them — an **HTML pipeline**:
they emit a string of markup, which then has to be sanitised before it can be
shown. That is the wrong shape for this input.

**The notes come off the network, and nothing verifies them.** They are read
out of `latest.json`, and the updater's signature covers the *installer bytes*,
not the manifest's prose. So the renderer never produces a string of markup at
all: every node is `createElement`, every piece of source text lands in a
`Text` node. There is no sanitiser because there is nothing to sanitise, which
is a much easier property to keep true.

The test that matters sends `<img src=x onerror="document.title='pwned'">` and
asserts no `<img>` exists, the characters are on screen, and the title did not
change. It fails the moment any paragraph is built with `innerHTML`.

### 10.2 What it draws, and what it refuses to

Headings (`h3`/`h4` — never outranking the dialog's own `h2`), bullet and
numbered lists, fenced code, code spans, bold, italic. Anything unrecognised
comes out verbatim: an unclosed `**`, a table, a blockquote. Showing a
construct as text is a far smaller failure than swallowing it.

**Links render as text**, `label (url)`. A real anchor inside a Tauri webview
navigates the app window away from the app — there is no tab to land in — and
the address is still readable and copyable this way.

Wrapped lines are joined rather than kept as breaks, which differs from GitHub
deliberately: the wrapping in a release body is an artefact of the file it was
typed into, at a width this dialog does not have. Structure is preserved
exactly; line endings inside a paragraph are not.

### 10.3 The notes are theirs, and look it

The app's own sentences stay outside the rendered block — the running version
above, the warning about unsaved work below — and the notes sit in their own
panel. A test asserts the warning is *not* inside it. Text from the endpoint
must not be able to occupy the place where the app speaks; that is the same
separation the logbook keeps between what the app did and what a server said.

`choose()` now takes `string | Node`, and a `Node` message is wrapped in a
`div` rather than the usual `p`: a paragraph cannot legally contain a heading
or a `<pre>`, and the `white-space: pre-line` that makes multi-line messages
readable would fight the renderer's own line breaking.

**718 UI tests on both engines.** Both new properties were falsified first:
bypassing the renderer fails the rendering test, and building one paragraph
with `innerHTML` fails the injection test.

## 11. The export note had nowhere to go — 2026-09-17

Also a [Stage 3](stage-3-explore-and-export.md) correction, recorded here.

> *"the exported with how many lines shows broken on the bottom area. We should
> render this on its own row"*

Screenshotted before it was touched, which is what showed the size of it. The
note shared the result bar with the status chips and four buttons, and lost:

```
[ 2   ]
[ rows]   Exported 5000 row(s), 3.2 MB, to /home/…/widgets_2026_09_17.csv. Thes…   [Copy] [Copy with headers] [Export…] [Close results]
[ 1  ]
[ ms  ]
```

**Three failures from one cause.** The chips wrapped into a vertical stack to
make room; the bar grew to three times its height; and the message was
ellipsised after its first clause. The chips even wrapped *inside themselves* —
"2" above "rows" — because nothing said a chip breaks between chips and never
within one.

The third is the one that matters. `describeExport` is not a receipt: after a
truncated result it says *"the result had already been cut off — the query has
more"*, and after a re-run it says *"this is every row"*. **Exactly the
sentence that distinguishes a partial export from a whole one was the part
being cut off**, which is the silent-wrong-answer failure the note was written
to prevent. Stage 3's own rule, defeated by a flex row.

So: its own row, wrapping rather than ellipsising, `white-space: nowrap` on
`.chip`, and the `title` attribute dropped — it existed only because the text
was unreadable, and a tooltip repeating what is on screen is noise.

The test asserts all three sentences are present *and* that
`scrollWidth/scrollHeight` do not exceed the client box — nothing is clipped —
*and* that the status chips share one line. Restoring the old layout fails it,
and so does putting `text-overflow: ellipsis` back on its own.

**720 UI tests on both engines.**

### 11.1 A note on where this is written down

This tracker has now taken three corrections that have nothing to do with
logbooks: an export button (§9), a changelog dialog (§10) and this. The rule
says corrections go in the current stage's tracker, and stage 16 is it — but
its scope closed at §7, and what is accumulating here is post-release polish
rather than a coherent slice. A Stage 17 for it would be the honest shape.

## 12. Autocomplete knew nothing — 2026-09-17

> *"something we can do to improve intellisense. Today seems not to be picking
> anything for autocomplete beside MySQL things which are not useful when
> requiring tables and columns"*

Three separate causes, each of which alone was enough to produce that.

### 12.1 The schema was fed by clicking

`schemaMap` was filled from the tree and nowhere else: table names when a
database was expanded, column names when a *table* was. So a freshly connected
editor offered nothing but the dialect's keywords — pages of
`HOUR_MICROSECOND` and `IGNORE_SERVER_IDS` — and after some browsing it offered
whichever subset had been opened. The second state is worse than the first: a
list that is silently partial reads as a complete one, so a missing table looks
like a table that does not exist.

**The mechanism already existed.** `warm_for_assistant` was built in Stage 10
for the identical problem one layer over — the model was handed "columns not
loaded" for every table nobody had clicked, and invented the rest. Completion
needed the same thing and never asked for it. So it is now `schema::warm`, with
`TABLE_DETAIL_BUDGET` in place of `ASSISTANT_TABLE_BUDGET`, and a `names`
reader on top of it.

A table whose columns are not cached comes back with an **empty list rather
than being left out** — omitting it would make autocomplete deny the existence
of a table the tree is showing.

### 12.2 Nothing read the database the connection was already on

Even in bulk, there was nothing to load: `tab.activeDb` was `null` until
somebody clicked a database, so there was no database to describe.

`connect` has returned `currentDatabase` since Stage 2 and **nothing ever read
it**. A tab's exec connection is opened with `profile.database` on it — read
from `session.rs`'s `options()` on 2026-09-17 — so a new tab is genuinely on
that schema from its first statement. Saying it had none was not caution; it
was a wrong answer about something already decided.

Fixed in both halves, because the frontend fix alone would have been undone by
the backend:

* `ConnectionEntry.currentDatabase`, adopted by every tab as it is created
  (`??=`, so a restored tab keeps the database it was left on).
* `TabSession::new` starts `current_db` from the profile rather than at `None`.
  Without this, `tab_status` reported `null` after the first run and the
  frontend dutifully set the tab's database back to nothing — completion would
  have worked until you ran a query, and then stopped.

Two consequences worth stating. `markActiveDb` is now the single funnel for
"the database changed", and the tree's click handler goes through it instead of
repainting the highlight itself — it was the most common way of changing
database and would have been the one place that missed this. And clicking the
database you are already on no longer issues a `USE`, because you are already
there; the spinner test now clicks a *different* database, which is the only
case that still costs two round trips.

### 12.3 `lang-sql` does not complete bare columns

With the schema loaded, `FROM ord` completed `orders` and `orders.` completed
its columns — but `SELECT * FROM orders WHERE us` offered the *tables* `users`
and `user_totals` and never `orders`' own `user_id`, which is the position
people actually type a column in.

Read from the source rather than guessed at
(`node_modules/@codemirror/lang-sql/dist/index.js`, `completeFromSchema`,
2026-09-17): it resolves `table.` and aliases, and offers bare columns only for
a single configured `defaultTableName`. There is no "columns of the tables this
query mentions".

So `columnsInScope`, registered **beside** the dialect's source rather than as
an `override` — keywords, tables and dotted columns all still come from
`lang-sql`. It reads the statement around the cursor, pulls the names after
`FROM`/`JOIN`/`UPDATE`/`INTO`, and offers those tables' columns, each labelled
with the table it came from.

A regex over text, not a walk of the syntax tree, and the comment says why: it
only ever *adds* suggestions, so being wrong costs an irrelevant row in a list
rather than a wrong answer or a missing one. The Rust splitter remains the only
thing that decides where a statement really begins.

Measured against the fixture, with nothing expanded:

```
SELECT * FROM ord                                  → orders, ORDER, ORDINALITY…
SELECT * FROM orders WHERE us                      → user_id (orders), user_totals, users…
SELECT * FROM `poc`.`orders` o JOIN users u … dis  → display_name (users)
UPDATE orders SET tot                              → total (orders)
SELECT * FROM orders; SELECT * FROM users WHERE em → email (users), and no `total`
```

### 12.4 Counts

**738 UI tests on both engines, 382 Rust, 73 live MySQL.** The two new live
tests prove the budget's edge: every table is named even when nothing is
detailed. Both halves of the frontend change were falsified — disabling
`columnsInScope` fails three tests, and dropping the database adoption fails
eight.

## 13. Two releases shipped with no Linux download — 2026-09-17

> *"linux build step keeps failing to upload the .deb"*

**The cause is still unknown**, and this section does not pretend otherwise.
What is known, all of it read from the public API rather than from the log,
which needs admin rights this machine does not have (`403 Must have admin
rights to Repository`):

```
v0.4.6  success   15:33   deb + deb.sig + exe + exe.sig + latest.json (3462 B)
v0.4.7  FAILURE   20:40   exe + exe.sig + latest.json (2324 B)   ← published
v0.4.8  FAILURE   22:12   exe + exe.sig + latest.json (2324 B)   ← published
```

In both failures the Linux `tauri-action` step ran for 6m15s and 6m16s, of
which the build itself is 5m27s — so it spends about 45 seconds uploading and
then fails. Windows succeeded both times. Nothing packaging-related changed
between v0.4.6 and v0.4.7: the diff is `docs/`, `src/main.ts`, a test and the
version bump.

One hypothesis was checked and is **dead**: `tauri-apps/tauri-action@v0` did not
move under us. Its `v0` tag was last written on 2026-03-14, six months ago.

### 13.1 The part that did not need a diagnosis

Both failed releases were **published anyway**, and neither offers a Linux
download. The smaller `latest.json` is the tell: 2324 bytes against 3462,
because it names only the Windows platforms.

That is the failure worth fixing first, and it is fixable without knowing why
the upload dies. The release workflow already refuses to build without a
signing key, on the stated grounds that *"the Linux job succeeds and the draft
looks almost right"* — the mirror image of this. It had no check on the way
out.

* **`rescued-*` artefacts.** A failed publish threw the installers away with the
  runner: the `.deb` was built, signed, and then existed nowhere. They are now
  kept for 14 days on failure, signatures included, so the release can be
  completed by hand.
* **A `verify` job.** After both builds, whatever they did, it lists the
  release's assets and fails naming any that are missing. The red X on a build
  job says *a job failed*; this says *the release has no `.deb`*, which is the
  sentence somebody needs before pressing Publish.

Its patterns are anchored, so `_amd64.deb` is not satisfied by `_amd64.deb.sig`
alone. Checked against the real releases before committing: v0.4.6 passes,
v0.4.7 reports `_amd64.deb _amd64.deb.sig`, and a synthetic sig-only release
reports just `_amd64.deb`.

### 13.2 The log, and the two words in it

It arrived. The whole of it:

```
2026-09-17T22:18:54.7859887Z Uploading db-query_0.4.8_amd64.deb...
2026-09-17T22:19:01.1806755Z ##[error]Error uploading
```

Six and a half seconds, and **no status code, no body, no cause**. `index.ts`
ends in `core.setFailed(error.message)`, so that string is the entire thrown
error. It is not even tauri-action's own wording — it appears nowhere in its
`src/`, so it comes from a dependency, and the bundle is too large to fetch
here to find out which.

So the diagnosis everybody wanted is not available, and this section will not
invent one. What the source does say, read at commit `84b9d35` — the one `v0`
has pointed at since 2026-03-14:

```ts
const retryAttempts = parseInt(core.getInput('retryAttempts') || '0', 10)
...
await retry(() => github.rest.repos.uploadReleaseAsset({ ... }), retryAttempts + 1)
```

**Unset means one attempt.** `retry` logs *"Attempt N failed, retrying..."* and
no such line is in the log, which confirms it from the other end. So every
release so far has uploaded its `.deb` exactly once, and two of them lost the
coin toss.

`retryAttempts: 3` now. It does not diagnose anything — nothing can, while the
message is swallowed — but a transient upload stops costing a release, and if
the failure is *not* transient the log will now say so three times over, which
is more than it has ever said.

The cost, stated because it is real: `retryAttempts` also re-tries the
*build*, so a genuinely broken one compiles four times. CI has already built
and tested the same commit before a tag is cut, so that is the rare case.

### 13.3 What was ruled out, and how

* **The action changing under us.** `tauri-action`'s `v0` tag was last written
  2026-03-14. Not it.
* **Size.** The `.deb` is 5.68 MB, within a few kilobytes of v0.4.5's and
  v0.4.6's.
* **Which job creates the release.** Linux finished first in all three runs,
  the successful one included, so "the job that creates the release is the one
  that fails" does not separate them.
* **Anything in the repository.** The diff between the release that worked and
  the first that did not is `docs/`, `src/main.ts`, one test and the version.

## 14. Autocomplete, second pass — 2026-09-17

> *"Still failing, is not suggesting columns properly and priorizing MySQL
> native methods"*

§12 loaded the schema and every test passed. The tests were the problem: they
asked questions the fixture answered well and nobody had asked what the popup
does when SQL is typed the way SQL is typed. So this pass began by printing the
list for eleven realistic inputs rather than by writing another test:

```
"SELECT e"                       → EACH, EDIT, EGO, ELSE, ELSEIF, ENABLE
"SELECT * FROM Users WHERE em"   → REMOVE, SCHEMA, SYSTEM, SCHEMAS, TEMPORARY
"SELECT * FROM users WHERE e"    → email (users), EACH, EDIT, EGO
```

Three separate bugs, and the first two produce exactly the reported symptom:
no columns at all, so the list is nothing but keywords.

### 14.1 SQL is typed in the wrong order for this

`SELECT` comes before `FROM`. While writing a fresh query there is no table in
scope, and §12's source required one — so it offered nothing in the position
where columns are most often typed.

With no table named, every column in the database is offered instead, once
something has been typed or Ctrl+Space has been pressed. What stays excluded is
the popup that opens *by itself* on an empty word, where nine hundred column
names would be noise nobody asked for.

### 14.2 `FROM Users` is `users`

The lookup was case-sensitive. MySQL folds table names to lower case on Windows
and macOS, and people capitalise however they like everywhere; this alone made
the feature look broken for anyone who types `Users`. The map is keyed by
lower-case name now and carries the server's own spelling for display.

### 14.3 Ranking, in both directions

`lang-sql` ranks its keywords at `boost: -1` — read from its source — and
nothing ranked the schema above them, so a column and a keyword that matched
equally well were sorted by name. Columns now carry `boost: 2` when the
statement names their table and `1` when they are merely somewhere in the
database, which puts them above tables (0) and keywords (-1).

**And the other direction.** Right after `FROM`, `JOIN`, `INTO` or `UPDATE` the
thing being typed is a *table*, so the source says nothing there — otherwise a
boosted `user_id` would sit above `users` in `FROM us`. A suggestion ranked
above the thing you are actually typing is worse than no suggestion.

A column that exists in several tables appears **once**, and says `2 tables`
rather than naming whichever came first: `id — big` is a confident answer to a
question nobody asked.

### 14.4 After

```
"SELECT e"                       → email (users), EACH, EDIT, EGO
"SELECT * FROM Users WHERE em"   → email (users), REMOVE, SCHEMA, SYSTEM
"SELECT * FROM users WHERE "     → display_name, email, id, then the tables
"SELECT * FROM us"               → users, user_totals   (no user_id)
"SELECT "                        → display_name, email, id (2 tables), total…
```

**748 UI tests on both engines.** Each of the five behaviours was reverted on
its own and fails its own test: the schema-wide fallback, the case-insensitive
lookup, the table-position guard, the boost, and the shared-column rollup.

### 14.5 The lesson, which is about the tests

§12's tests were green while the feature was unusable, because they only ever
asked what the author already believed. Eleven lines of printed output found in
one run what a dozen assertions had not. **When the complaint is "it feels
wrong", print what it does before writing another test about what it should
do.**
