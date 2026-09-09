# Stage 7 — Session persistence

> ## 🔒 FROZEN — do not modify
>
> This tracker covers **Stage 7**, and its scope is delivered. Tabs, their
> contents, their order and the one in front survive a quit; a workspace waits
> in memory and materialises when its connection opens. **S1-S8 are answered by
> test** — 17 UI tests on both engines and 6 Rust tests — and two of the
> load-bearing ones were confirmed by removing the fix and watching them fail:
> the `onConnected` await (nine failures) and the pending-workspace merge.
>
> **Two things carry to
> [Stage 8](stage-8-attribution-and-the-first-release.md):**
>
> - **S9 — nobody has quit the real app and reopened it.** Everything here is
>   proven in a browser against a stubbed backend, which is the right place for
>   the logic and cannot answer whether the file lands where `app_config_dir()`
>   says on Windows.
> - **D4 — tabs stored under a profile that no longer exists.** `onRemoved`
>   forgets them while the app is running; a session file that was hand-edited,
>   or crashed mid-removal, is not covered.
>
> One pre-existing bug was **found and deliberately not fixed** here: the
> "Connected to …" status line has never been visible, because the first tab
> activation overwrites it. Recorded in §5 rather than fixed, since it is not
> this stage's scope.
>
> Corrections belong in the current stage's tracker with a link back here.

**Goal:** the app comes back the way you left it. Tabs, their contents and their
place in the workspace survive a quit, a crash and an update.
**Builds on:** [Stage 6 — Updates & attribution](stage-6-updates-and-attribution.md).
**Status:** 🔒 Frozen 2026-09-08 — restore-on-connect delivered and tested;
hands-on confirmation (S9) and D4 carried to Stage 8.

---

## 1. The question

> *"now we must work on how can we keep the tab sessions after we close the app.
> So lets analyze this and plan if is possible"*

**Yes, and most of the machinery is already here.** `ScriptTab` is a plain data
record, `profiles.rs` is a working, tested pattern for a versioned, atomic,
0600, corrupt-file-recoverable config file, and the backend already treats a tab
as *registered on connect* rather than *created by connecting* — which is the
property that makes a tab able to exist before its server does.

There is exactly **one thing the current design cannot do**, and it is a UI
problem rather than a storage one. §4 is about that.

---

## 2. What is restored, and what is deliberately dropped

`ScriptTab` has 22 fields. They fall into three groups, and the split is the
substance of the design.

### Restored — this is the feature

| Field | Note |
|---|---|
| `connectionId` | Which workspace the tab belongs to. Tabs never migrate. |
| `title`, `filePath`, `untitledNumber` | The tab strip, unchanged. |
| the document text | The whole point. |
| cursor / selection offset | Coming back to the line you left. |
| `dialect`, `encoding`, `lineEnding`, `mtimeMs` | Already the inputs to Save; losing them would make the first save after a restart write the wrong bytes. |
| `activeDb` | Per tab today, and re-issued as a `USE` when the connection comes up. |
| tab order, active tab per connection, last active connection | Otherwise everything reopens in creation order. |

### Dropped on purpose — restoring these would be worse than losing them

- **`result`.** Result sets are unbounded (up to `maxRows`, 5000 by default,
  arbitrarily wide) and **stale by definition**. Painting yesterday's rows as
  though they were a result is the kind of wrong that costs someone a decision.
  A restored tab shows its script and an empty results pane.
- **`error`** — a failure against a connection that no longer exists.
- **`sourceTable`, `colWidths`, `colSelection`, `activeResultIndex`,
  `scrollTop`** — every one of them describes a result that will not be there.
- **`serverConnId`, `busy`** — runtime facts about a socket that is closed.
- **Undo history.** `EditorState.toJSON` can carry it, but the format is
  CodeMirror's and it would become a compatibility surface across app updates
  for a feature nobody has asked for. The restored buffer starts with a clean
  history and a baseline (see D2).

### Not stored at all

Anything already covered: settings and theme stay in `localStorage`, profiles
stay in `connections.json`, passwords stay in the keychain.

---

## 3. Where it lives

**A Rust-owned `session.json` in `app_config_dir()`**, written through
`files::write_atomic`, `version`-stamped, restricted to 0600, and moved aside
with a timestamped name if it will not parse — the same five properties
`profiles.rs` already has, for the same five reasons.

**Not `localStorage`,** even though settings use it:

- It is the *webview's* storage. WebKitGTK and WebView2 put it in different
  places, an update or a "clear app data" wipes it, and it is subject to a quota
  we would be putting whole SQL files into.
- No atomic write. A half-written session is a lost session, and unlike a
  half-written settings blob it is not something you can shrug off.
- No recovery path for a corrupt file.

Settings are a handful of scalars that can be recreated in ten seconds by
retyping them. A day's unsaved SQL is not.

### Size

**A clean file-backed tab stores no text** — only its path and `mtimeMs`, and
the text is re-read on restore. Text is written only for **untitled** tabs and
**dirty** file-backed ones, which is where it is the only copy in existence.
That keeps the common case to a few hundred bytes a tab.

---

## 4. The decision: restore on connect

The analysis offered two shapes — restore lazily at connect (**A**), or restore
eagerly into an offline workspace (**B**). The recommendation was B.

> *"the request will need the tabs to be restored when we connect (As each
> connection can have its tab) so restoring them only when we connect is good
> enough"*

**A**, and the reasoning is better than the recommendation was: tabs belong to a
connection, so a workspace you cannot use is a workspace whose tabs cannot run
anything anyway. It also leaves the connection model — Stage 2's, stable since —
completely untouched.

So a workspace waits, fully loaded, in memory until its connection comes up.
Nothing connects at boot; that is asserted, not assumed.

### The one ordering that matters

`setActiveConnection` **creates an empty tab for any workspace it finds empty**,
and it runs a moment after a connection opens. Restoring has to finish first or
every restored workspace gets a stray `Untitled-1` beside it — and, worse, a
restore that lost the race would leave the tabs on disk and an empty window on
screen.

`onConnected` is therefore **awaited**: its hook type is now
`void | Promise<void>`, and `attempt()` waits for it before calling `activate()`.
Removing that one `await` fails **nine** of the seventeen tests, which is the
right amount for something that load-bearing.

`restoreInto` also waits on the file read, because connecting can easily beat it
at boot — and it takes its workspace out of `pending` *before* its first `await`,
so two connects in flight cannot restore the same tabs twice.

---

## 5. Decisions, and what implementation changed about them

- **D1 — When does it write?** Debounced on document change (~1s idle), plus
  immediately on tab create / close / reorder / activate and on
  `onCloseRequested`. The debounce is what bounds how much a crash costs. Every
  write is whole-file and atomic; there is no incremental format.
- **D2 — Dirtiness must survive.** `isDirty` is `doc !== baseline`, never a
  flag. A dirty file-backed tab therefore has to restore **both** texts, or it
  comes back looking clean and the user never learns they had edits. Store the
  buffer, and re-read the file for the baseline when `mtimeMs` still matches; if
  it does not, the file changed underneath us and the tab restores straight into
  the existing conflict path rather than inventing a second one.
- **D3 — A missing file.** Keep the buffer, keep the tab, mark it detached from
  its path so Save asks where to put it. Losing a buffer because a path went
  stale would be the worst possible failure for a feature whose entire job is
  not losing buffers.
- **D4 — A profile that no longer exists.** Drop its tabs, and say so once in
  the same warning channel `profiles.rs` uses. Removing a connection already
  prompts per tab (`canDrop` → `confirmClose`), so a stored tab with no profile
  means a crash mid-removal or a hand-edited file, not lost consent.
- **D5 — `idSeq` collides.** It is a module-level counter starting at zero, so
  restored `t1..tn` would be minted again within the session. Restored tabs get
  **fresh** ids; nothing outside the session file refers to the old ones, and
  the backend session is new regardless.
- **D6 — The quit prompt changes meaning.** Persistence keeps the *buffer*, not
  the *file*. Quitting with unsaved edits to `report.sql` still leaves stale
  bytes on disk that something else may read, so the prompt stays for
  file-backed tabs — but an **untitled** buffer is no longer at risk and should
  stop being counted. Prompting about work that is provably safe is how people
  learn to click through prompts.
- **D7 — Buffers can contain secrets.** `CREATE USER … IDENTIFIED BY '…'` in an
  untitled tab is today a thing that only ever exists in RAM; this feature
  writes it to disk. 0600 and `app_config_dir` are the same protection
  `connections.json` gets, and worth stating out loud rather than discovering.

---

### What implementation corrected

- **D2 got simpler.** The plan worried about how to restore the baseline of a
  dirty tab. The answer is to **re-read the file** — the baseline is whatever is
  on disk, which is what it always meant. What is stored instead is the mtime
  the tab was *based on*, so the first Save after a restart runs into the
  **existing** conflict check rather than a second one invented for restore.
  A clean tab takes the file's current mtime, because it **is** the file.
- **D3 split in two, and the split is the whole point.** A vanished file takes a
  **clean** tab with it — that tab held nothing the file did not, so dropping it
  loses no work. A **dirty** tab keeps its buffer and lets go of its path, so
  Save asks where to put it. Both are tested; the second is the one that would
  destroy something if it were wrong.
- **Warnings had to become a dialog.** They were going to `results.setMessage`,
  which the first tab activation overwrites a moment later — so the message
  saying a tab had been lost was invisible. Restored tabs are their own evidence,
  on screen; a tab that did *not* come back is the one thing nothing on screen
  can tell you, so it gets a dialog. **This also revealed that the existing
  "Connected to …" line has never been visible either**, for the same reason.
  Left alone, and recorded here.
- **D5 held.** Fresh ids, and the active tab is stored as an **index** rather
  than an id, since a stored id would refer to nothing after a restart.
- **D6 was implemented.** Quitting now asks about **files**, not buffers: an
  untitled buffer is remembered, so prompting about it is prompting about work
  that is provably safe. A file-backed tab with unsaved edits still asks — what
  persists is the buffer, not the file, and the stale bytes on disk are what
  something else will read.
- **`activeDb` is restored, and it issues a `USE`.** This is the one place
  restore talks to the server beyond registering a tab. It is the same class of
  session setup as `open_tab`, which is already issued unattended at connect, and
  without it a restored script silently runs against a different schema than the
  one it was written for. A failure is harmless: the tab stays on the default.
- **The harness can fire Tauri events now.** `plugin:event|listen` is recorded
  and `fireEvent(page, "tauri://close-requested")` dispatches it, which is what
  made the quit path — the prompt *and* the final flush — testable at all.
  Dispatch deliberately does not await the handler: a close handler that opens a
  dialog never settles until someone answers, which would deadlock the test that
  wants to see the dialog.

---

## 6. Milestones

- [x] **S1 — Quit and reopen restores an untitled buffer**, its text, its cursor and its tab position.
- [x] **S2 — A clean file-backed tab restores from its path**, with no copy of its text in the session file.
- [x] **S3 — A dirty file-backed tab restores dirty**, with the unsaved edits intact and the dot showing.
- [x] **S4 — A file changed on disk while the app was closed** lands in the conflict path: the restored tab keeps the mtime it was based on, so the first Save asks.
- [x] **S5 — Nothing connects at boot.** Asserted, not assumed: no `connect` or `connect_saved` command is invoked before the user acts.
- [x] **S6 — No result set is ever restored** — asserted against the written payload, not just the screen.
- [x] **S7 — A corrupt `session.json` is moved aside** and the app starts with no tabs and one warning (Rust).
- [x] **S8 — Quitting flushes what the debounce is still holding**, so a clean quit loses nothing; a `kill -9` still costs at most the debounce window.
- [ ] **S9 — Confirmed by hand on both platforms**: quit the real app with three tabs open and reopen it.

---

## 7. Task tracker

### Phase 1 — Storage — built 2026-09-08
- [x] `src-tauri/src/workspace.rs`: `SessionStore { version, connections: [{ connectionId, tabs, activeIndex }] }`, `load` / `save`, atomic + 0600 + move-aside. **6 unit tests**, mirroring `profiles.rs` — including that `save` stamps the version itself, so a frontend that forgets to cannot write an unversioned file, and that an unknown field from a future version does not discard the tabs it *does* understand
- [x] `load_session` / `save_session` commands; `api.ts` wrappers and types

### Phase 2 — Capture — built 2026-09-08
- [x] `src/session.ts`: snapshot, debounce (1s), flush
- [x] Text stored only when it is the only copy — untitled, or file-backed and dirty
- [x] **Workspaces never restored this session are written back untouched.** Without this, connecting to one server silently deletes every other server's remembered tabs. It has its own test, and that test fails without the merge
- [x] Scheduled on doc change, tab create / close / activate, save, connect / disconnect / remove; flushed on `tauri://close-requested`

### Phase 3 — Restore — built 2026-09-08
- [x] Materialise per connection, inside an **awaited** `onConnected` (§4)
- [x] Re-read files; stored mtime for dirty tabs so the existing conflict check still fires (D2)
- [x] A vanished file drops a clean tab and spares a dirty one (D3)
- [x] Fresh ids; the front tab stored as an index (D5)
- [x] `activeDb` re-issued as a `USE` per restored tab

### Phase 4 — Consequences — built 2026-09-08
- [x] Quit prompt counts only file-backed dirty tabs (D6)
- [x] **17 UI tests** on both engines, **6 Rust**. The `onConnected` await was removed to confirm nine of them fail without it, and the pending-merge was removed to confirm its test fails without it
- [ ] D4 — tabs stored under a profile that no longer exists. `onRemoved` already forgets them live; a session file hand-edited or crashed mid-removal is not yet covered

---

## 8. Risks

- **This feature's only failure mode is losing work**, which is exactly what it
  exists to prevent. Every path that discards a buffer (D3, D4, a corrupt file)
  needs a test before it needs a nicety.
- **The offline workspace touches the connection model**, which is Stage 2's and
  has been stable since. Worth doing carefully rather than quickly.
- **Whole-file atomic writes on every debounce** are fine at ten tabs and worth
  re-measuring at a hundred, or with a multi-megabyte dirty buffer.
- **A format written now will be read by a future version.** `version` is
  cheap; a migration for a file we shipped without one is not.
