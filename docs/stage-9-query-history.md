# Stage 9 — Query history

**Goal:** find a statement you ran before, and put it back in the editor.
**Builds on:** [Stage 7 — Session persistence](stage-7-session-persistence.md)
(frozen), whose storage pattern this reuses and departs from in one place.
**Status:** 🚧 In progress — built 2026-09-08; hands-on confirmation pending.

---

## 1. Why this one

Query history has been on the backlog since **Stage 0**, and appears in Stage 1's
and Stage 2's backlogs too — it is the single most-deferred item in the project.
It is also the one that got cheaper while it waited: Stage 7 built the pattern
for a versioned, owner-only config file, and Stage 6 built the settings dialog
this borrows its shape from.

### Explicitly out of scope

- Re-running from the history list. It inserts; **you** run it. Same rule as
  every generated-SQL action since Stage 3.
- Saved/named queries, or favourites. A different feature that happens to look
  similar — history is automatic, a snippet library is deliberate.
- Syncing history anywhere, or exporting it.
- Result snapshots. History records that a statement ran and what happened, not
  what came back.

---

## 2. Decisions

### JSON Lines, not a rewritten document

`profiles.rs` and `workspace.rs` rewrite their whole file on every change, which
is right for a list of servers and **wrong for a log**. History only appends and
grows without limit; rewriting a few thousand entries after every query would
make running SQL slower the longer you used the app.

One JSON object per line gives an O(1) append and fails *partially*: a line torn
by a crash costs that line, not the file. A single JSON array truncated
mid-write costs everything. There is a test that appends a deliberately broken
line and asserts the entries on either side of it survive.

Compaction keeps the newest 5,000 entries, triggered by a **file-size check**
rather than a line count, so the common path stays a metadata call.

### Credentials are never recorded

`CREATE USER … IDENTIFIED BY 'x'` puts a password in the statement text, and this
project's rule since Stage 2 is that secrets live in the keychain and nowhere
else. Those statements are **skipped entirely rather than redacted**: a
redaction that is subtly wrong writes the secret down anyway, and failing closed
is the only direction worth failing in.

The match is deliberately narrow — `identified by`, `identified with`,
`set password`, `password(`. Matching the bare word "password" would drop every
query against a `password_hash` column, and **a rule that silently eats ordinary
history is worse than one that occasionally misses**. Both halves have tests,
including one proving `SELECT password_hash FROM users` is still recorded.

The dialog says so in a footnote, because an unexplained absence reads as a bug.

### What is stored is what the user wrote

Auto-LIMIT rewrites a statement before sending it. History keeps `sql`, never
`effective_sql`: the `LIMIT` is ours, and handing it back later as though they
had typed it is a small lie that compounds every time the statement is re-run.
Pinned by a live test that first asserts the rewrite actually happened, so the
test cannot pass vacuously.

### Recorded in Rust, at the one point everything passes through

`run_script` is the single funnel for every execution, so recording there cannot
miss a path the way a frontend hook could. It is **best-effort and silent**:
failing to remember a query must never fail the query.

It lives in the *command* rather than in `exec::run_script`, so execution keeps
no opinion about config directories — and `remember` takes a directory rather
than an `AppHandle`, which is what lets the live suite drive it against a real
server.

### Repeats collapse

Twenty runs of one statement is one row that says twenty. The list exists to
find a statement again, and repetition is exactly what pushes the interesting
ones off the bottom. The row keeps the **newest** occurrence, so "1m ago" means
what it says.

### Failures are kept, with their message

A failed statement is often precisely the one you are trying to find again. The
server's own error is on the row.

---

## 3. Milestones

- [x] **H1 — Statements are recorded as they run**, with database, outcome, row count and timing. Proved against a real MySQL, not only against synthetic entries.
- [x] **H2 — A failed statement is recorded with its error.**
- [x] **H3 — A credential never reaches the disk.** Asserted on the file's bytes, live, not on what the reader filters out.
- [x] **H4 — What is recorded is what was typed**, not our auto-LIMIT rewrite.
- [x] **H5 — The list is searchable**, scoped to the active connection by default, newest first, repeats collapsed.
- [x] **H6 — Choosing a statement puts it in the editor and runs nothing.** Asserted in every picking test.
- [x] **H7 — History can be cleared**, after asking, scoped the way the list is scoped.
- [x] **H8 — Confirmed by hand** against a real database on both platforms. Confirmed 2026-09-09, on the installed app.

---

## 4. Task tracker

### Phase 1 — Store — built 2026-09-08
- [x] `src-tauri/src/history.rs`: JSONL, 0600, append-only, size-triggered compaction, corrupt-line tolerance. **12 unit tests**
- [x] `carries_credential` and its two-sided test: credentials skipped, `password_hash` kept
- [x] `search` — newest first, deduplicated with a run count, optional substring and connection filters

### Phase 2 — Recording — built 2026-09-08
- [x] `remember()` in the `run_script` command; silent and best-effort
- [x] **4 live tests**: both statements of a script recorded; a failure carries its message; the user's SQL survives auto-LIMIT; a password never reaches the file

### Phase 3 — The dialog — built 2026-09-08
- [x] `src/history.ts`, `#history-dialog`, a rail button and **Ctrl+H**
- [x] Search-as-you-type with a generation counter, so a slow request cannot overwrite a newer one
- [x] Arrow keys move, Enter takes; right-click offers a new tab or a copy
- [x] Clear, behind `dialog.ts` — scoped to whatever the list is scoped to
- [x] **14 UI tests** on both engines

---

## 5. Two bugs this found

### The context menu was unreachable inside a modal

Right-clicking a history row drew the menu and then **swallowed every click**.
`showModal()` puts a dialog in the browser's *top layer*, above the entire
normal stacking order — and `contextMenu` appended to `document.body`. No
`z-index` can lift an element out of the normal stacking order; being a
descendant of the dialog is the only way in. `contextMenu` now appends to
`dialog[open]` when there is one.

This is **the fourth time in this project that an element was painted but not
usable** — after the `[hidden]` rule in Stage 2, and the two `display: flex`
dialogs in Stage 6. Found by a test rather than by eye, which is new.

### `Array.prototype.at` is newer than our TS target

`npm run build` typechecks the tests as well as `src/`, and rejected `.at(-1)`.
Worth recording because it is the second time that typecheck has caught
something Playwright ran happily.

---

## 6. Backlog

- Re-running a statement from the list directly, behind a confirmation — decided
  against for now; inserting is undoable and running is not
- Pinning or naming a statement, which is the saved-queries feature wearing this
  one's clothes
- Filtering by outcome (errors only) or by date
- A per-tab history rail, rather than a modal
