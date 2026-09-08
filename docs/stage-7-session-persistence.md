# Stage 7 — Session persistence

**Goal:** the app comes back the way you left it. Tabs, their contents and their
place in the workspace survive a quit, a crash and an update.
**Builds on:** [Stage 6 — Updates & attribution](stage-6-updates-and-attribution.md).
**Status:** 📋 Planned — analysis only, nothing built.

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

## 4. The one thing that does not fit today

**A disconnected connection cannot be the visible workspace.**
`connections.ts` line ~252 reads:

```ts
if (entry.connected) this.activate(entry.profile.id);
else void this.connect(entry);
```

So there is no way to be *looking at* a connection that is offline, and at boot
nothing is connected. Restored tabs would have nowhere to appear.

We will not solve that by connecting at boot. Connecting is a network action
against someone else's server, it prompts for passwords that are not in the
keychain, and it is squarely the thing this project has refused to do
unattended since Stage 0.

Two shapes, and they are genuinely different products:

| | **A — restore lazily, at connect** | **B — restore into an offline workspace** |
|---|---|---|
| Boot looks like | exactly as today | your tabs, as you left them |
| Tabs appear | when you connect that server | immediately |
| Connection model | untouched | rail click activates *and* connects |
| Editing offline | impossible | works; Run is disabled and says why |
| Cost | small | one new UI state, honestly done |

**Recommendation: B**, because A does not actually answer the request — quitting
and reopening would still show an empty window, and the scripts would be a
password prompt away. The change B needs is small and is an improvement on its
own terms:

- Rail click **always activates**, and additionally starts connecting if the
  entry is offline. Nothing is taken away — one click still connects — but the
  workspace and its spinner are visible while it happens, which is the Phase 5
  loader argument applied one level up.
- At boot, the last active connection is **activated, not connected**.
- `syncBusy` learns to say *"Not connected"* rather than leaving Run enabled
  against nothing.

---

## 5. Decisions to make before writing code

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

## 6. Milestones

- [ ] **S1 — Quit and reopen restores an untitled buffer**, its text, its cursor and its tab position.
- [ ] **S2 — A clean file-backed tab restores from its path**, with no copy of its text in the session file.
- [ ] **S3 — A dirty file-backed tab restores dirty**, with the unsaved edits intact and the dot showing.
- [ ] **S4 — A file changed on disk while the app was closed** lands in the conflict path, not silently either way.
- [ ] **S5 — Nothing connects at boot.** Asserted, not assumed: no `connect` command is invoked before the user acts.
- [ ] **S6 — No result set is ever restored**, and the results pane says so rather than looking broken.
- [ ] **S7 — A corrupt `session.json` is moved aside** and the app starts with no tabs and one warning.
- [ ] **S8 — A kill -9 loses at most the debounce window**, demonstrated rather than reasoned about.

---

## 7. Task tracker

### Phase 1 — Storage
- [ ] `src-tauri/src/workspace.rs`: `SessionStore { version, connections: [{ id, tabs: [...], activeTabId }], activeConnectionId }`, `load` / `save`, atomic + 0600 + move-aside, unit tests mirroring `profiles.rs`
- [ ] `load_session` / `save_session` commands; `api.ts` wrappers

### Phase 2 — Capture
- [ ] Serialise `ScriptTab` → stored shape; omit text for clean file-backed tabs
- [ ] Debounced writer; flush on create / close / reorder / activate / `onCloseRequested`

### Phase 3 — Restore
- [ ] The offline-workspace state (§4 B) and the rail click change
- [ ] Materialise tabs per connection; re-read files; conflict path on mtime mismatch (D2, D3)
- [ ] Drop tabs with no profile, with a warning (D4); fresh ids (D5)

### Phase 4 — Consequences
- [ ] Quit prompt counts only file-backed dirty tabs (D6)
- [ ] Results pane copy for a restored tab (S6)
- [ ] UI tests for S1-S8; Rust tests for the store

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
