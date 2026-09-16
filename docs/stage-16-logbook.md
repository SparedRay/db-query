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
