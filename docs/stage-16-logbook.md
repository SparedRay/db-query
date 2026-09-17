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
