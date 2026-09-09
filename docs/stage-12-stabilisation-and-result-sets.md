# Stage 12 — Stabilisation: result sets, idle connections, and the carried bugs

**Status:** 📋 Planned 2026-09-09.

The first stage whose subject is the backlog rather than a feature. Two things
prompted it: results cannot be dismissed, and a connection left alone comes back
broken. Everything else here is a bug that has been carried, in writing, across
several freezes — this is where they get paid off rather than copied forward
again.

---

## 1. What this release actually is

Worth stating plainly, because the framing affects the tag.

**Corrected 2026-09-09.** An earlier draft of this section said `v0.2.0` was the
last release and proposed `v0.3.0`. That was read from a stale tag list: `v0.3.0`
and `v0.3.1` are both published, and `v0.3.1` is the version being used. Query
history, the assistant and Elasticsearch all shipped in them.

So this **is** a bugfix release by content as well as by name, and the next tag
is **`v0.3.2`**. Nothing here is a schema, config or API break — with one
deliberate exception, C5, which deletes three commands nothing calls.

The tree itself had drifted: `package.json` and `Cargo.toml` said `0.2.0` while
`v0.3.1` was in the wild, because CI stamps the version from the tag and nothing
wrote it back. Fixed by syncing the tree and by making `npm version` the way
releases are cut — it moves `package.json`, the lockfile and `Cargo.toml`
together in one commit and tags it. See the README's *Releasing* section.

---

## 2. Closing result sets

### What is there now

One run produces one `ScriptResult` holding a `StatementResult` per statement.
The grid shows **one statement at a time**; a tab strip (`#tabs`,
`ResultView.renderTabs`, `grid.ts:145`) selects which, and hides itself entirely
for a single-statement run.

The result is owned by the `ScriptTab` (`tabs.ts:39`) — `result`, `error`,
`activeResultIndex`, `colWidths`, `colSelection`, `scrollTop`, `sourceTable`.
The module-level `ResultView` holds a *reference* to the same object, which is
why a width drag or a selection mutates the tab in place.

**There is no way to dismiss a result.** No button, no menu item, no
keybinding. The only wipe primitive is `ResultView.setMessage()` (`grid.ts:100`),
and it is view-only: it nulls the view's `result` and leaves `tab.result`
untouched, so the rows come back on the next tab switch.

### Why it matters more than tidiness

Every statement's rows are retained in full — `CellValue[][]` for the whole
script, for every tab, for as long as the tab is open. The DOM is virtualised;
the arrays are not. A few large result sets across a few tabs is real memory
with no way to release it short of closing the tab, which also throws away the
SQL the user was writing.

### The shape

Two distinct actions, because they answer different questions:

1. **Close this result** — discards one statement's result from the strip. Only
   offered when there is more than one, since closing the only result is the
   next action, not this one.
2. **Close all results** — resets the tab to the state a new tab is in: the
   "No results yet." message, export and copy disabled, selections and column
   widths dropped.

Both reset the *tab's* fields, then repaint through the existing
`showResults(tab)` path, so there is exactly one way the pane gets drawn. The
fields to reset are the block `tabs.ts:234` already writes on create; a shared
`clearResult(tab)` keeps the two from drifting.

Placement: a button in `.result-actions` beside Copy and Export, an item in the
cell context menu (`grid.ts:352`), and a `✕` on each statement tab in the strip
for the per-statement close. A keybinding is deliberately **not** proposed —
Escape already clears the *selection* inside the grid, and overloading it to
sometimes destroy the result is the kind of thing that costs someone their work.

### The adjacent bug this exposes

**Disconnecting does not clear the result it invalidates.** `onDisconnected`
(`main.ts:1498`) calls `results.setMessage("Disconnected from …")`, which is
view-only — so switching tabs repaints rows fetched from a server we are no
longer connected to, with export and copy live. Fixed here because the fix is
`clearResult(tab)`, which this stage is adding anyway.

---

## 3. The idle disconnect

### What is actually broken

The exec connection **already reconnects**. `session::ensure_exec`
(`session.rs:548`) pings, reopens on failure, and re-applies the tab's `USE`.
Its doc comment names `wait_timeout` explicitly. That path is fine.

`ping()` appears **exactly once in the codebase** — in that function.

- `meta` — one shared connection, **seven call sites** (`schema.rs` ×5,
  `session.rs` ×2): the schema tree, column lists, routine lists, `SHOW CREATE`,
  and now the assistant's schema warm-up. No liveness check.
- `killer` — the connection cancellation uses. No liveness check.

So after the server reaps an idle connection: running a query works, and
expanding the tree answers `Cannot reach the server: …` (`friendly()`,
`session.rs:275`). That asymmetry is the whole bug.

### The fix

Generalise the pattern that already works. `ServerConn` gains an accessor per
shared connection that pings, reopens on failure, and hands back the guard —
`mysql_meta()` and `mysql_killer()` become the checked versions rather than
gaining checked twins, so a new call site cannot accidentally get the unchecked
one. There is no third way to reach these fields.

Three details that are easy to get wrong:

- **`meta` must not re-apply a `USE`.** Every introspection query binds its
  schema explicitly; a reconnect that inherited a database would be a behaviour
  change smuggled in as a fix.
- **`killer` reconnecting is still correct.** `KILL` needs *a* live connection,
  not the original one. The id it kills is stored on the tab.
- **Elasticsearch needs none of this.** HTTP is stateless and `meta` is `None`
  for it — the accessors already return a `Result` saying a MySQL-only path was
  reached. The fix stays behind the engine seam.

### What the user is told

**Nothing, for `meta`.** Introspection carries no session state, so a silent
reconnect loses nothing and a message would be noise.

**For `exec`, we should start saying something.** It already reconnects
silently, and that is the half that *does* lose state — temporary tables,
session variables, and an open transaction. Someone mid-transaction currently
has it rolled back and is told nothing. `ensure_exec` knows whether it replaced
a dead connection or opened the first one; propagating that one bit lets the
result bar carry a "reconnected — session state was reset" chip beside the
existing ones. Cheap, honest, and it does not interrupt anything.

### Considered and rejected: a keepalive

Pinging on a timer to stop the server reaping us. Rejected: it holds a
connection open on someone's production server for as long as the app is
running, it does not help when the loss is the network rather than
`wait_timeout`, and reconnect-on-use is strictly simpler. Reconnecting when the
user next asks for something is the behaviour with no cost when idle.

**The standing rule is untouched.** Reconnecting happens on a user-initiated
action and re-runs nothing. Nothing is ever executed on the user's behalf.

---

## 4. The carried bugs

Each verified against the code on 2026-09-09 rather than taken from its tracker.

### C1 — Export and the grid disagree about binary

`sqlgen::literal()` (`sqlgen.rs:683`) refuses on the column's `TypeHint::Binary`
**before looking at the value**. `decode.rs:147` decodes the same BLOB-family
type names *text-first*, because a `VARCHAR` with a binary collation is reported
under exactly those names. So such a column renders as readable text in the grid
and is then refused by the SQL-INSERT export.

The doc has said "make one of them right and both agree" since Stage 5. The
value is the half that knows the truth, so:

Add `CellValue::Binary { bytes: usize }`. The decoder already distinguishes the
cases — where it now writes `CellValue::Text("<binary, N bytes>")` it writes the
new variant. `literal()` refuses on the *value* being `Binary`, not on the
column type. A binary-collation `VARCHAR` that decoded to real text exports
normally; a genuine BLOB is still refused, for the same reason as before.

Blast radius, all covered by existing tests: `decode.rs`, `sqlgen::literal`,
`export.rs` (CSV, INSERTs, `binary_column_warning`), the `CellValue` type in
`api.ts`, the grid renderer and the cell viewer. The placeholder string stops
being load-bearing, which is the point — a real value of `"<binary, 12 bytes>"`
is currently indistinguishable from the real thing.

### C2 — `streamingExport` is declared and never read

The capability exists on `Capabilities` (`engine.rs:56`), is `false` for
Elasticsearch (`elastic.rs:43`), is plumbed into the TS type (`api.ts:209`) and
into both test fixtures — and **no UI code reads it**. `refreshExportBar`
decides with `rerunOk = sql !== null && results.isEverything()`
(`main.ts:1131`).

So on a cluster the export dialog offers "Re-run the query and export every
row", enabled, and taking it reaches a MySQL-only streaming path. The dialog
already knows how to disable that option with a reason — it does so for a
subset selection and for a non-rerunnable statement. This is a third reason,
and the sentence it needs is the honest one: this engine cannot stream.

### C3 — A new tab's dialect is hardcoded

`tabs.ts:228`: `dialect: opts?.dialect ?? "mysql"`, whatever the connection is.
Highlighting only, not correctness. The tab knows its connection; the connection
knows its engine; `capabilities.engine` is the value.

### C4 — Lint is MySQL-flavoured

Schema-aware checks work on any engine because they read the tree. The
dialect-specific rules have never been audited against a second engine. This is
an **audit with an unknown outcome**, not a known defect — it may produce no
change beyond a note. Scoped as such deliberately.

### C5 — Dead API surface

`split_sql`, `disconnect_all` and `has_stored_password` are registered in Rust
and wrapped in `api.ts`, and called from nowhere. Found in Stage 4. A published
release freezes an API, so this is the last comfortable moment to delete them.
Deleting is the recommendation; each is reconstructible from git if wanted.

### C6 — Two small ones

- **D4** (Stage 7): tabs stored under a profile that no longer exists.
  `onRemoved` forgets them live; a session file hand-edited or written across a
  crash mid-removal is not covered.
- **The "Connected to …" line has never been visible**, because the first tab
  activation overwrites it. Found in Stage 7 and left. Either give it somewhere
  durable or stop writing it — writing a message nobody can see is worse than
  not writing one.

---

## 5. Milestones

- [ ] **B1 — A result can be closed**, one statement or all, and the pane
      returns to the state a fresh tab is in.
- [ ] **B2 — Disconnecting clears the results it invalidated.** No stale grid
      survives a tab switch, and export cannot act on one.
- [ ] **B3 — An idle connection heals itself.** Leave a connection past the
      server's `wait_timeout`, then expand the tree: it works, with no error and
      no reconnect ceremony.
- [ ] **B4 — A replaced exec connection says so**, once, where the user will see
      it, because session state was lost.
- [ ] **B5 — Binary and text agree.** A binary-collation `VARCHAR` that the grid
      shows as text exports as text; a real BLOB is still refused.
- [ ] **B6 — A cluster is not offered an export it cannot perform.**
- [ ] **B7 — Nothing regressed.** Every suite, both engines, both live backends.

## 6. Task tracker

### Phase 1 — Result sets
- [x] `clearResult(tab)` + `emptyResultState()` in `tabs.ts`, spread by `create` and `restore` so the three cannot drift
- [x] "Close results" button, a `✕` per statement tab, and a `danger` cell-menu item
- [x] `onDisconnected` clears rather than merely repainting (B2)
- [x] **14 UI tests** on both engines: each route; survives a tab switch; export
      and copy go disabled; the strip hides at one statement; the active
      statement is preserved when an earlier one closes; a disconnect leaves
      nothing to repaint; the script is never touched

### Phase 2 — Connections
- [x] `mysql_meta()` / `mysql_killer()` hand back a **locked, checked** guard,
      and the fields are now private — so there is no unchecked way in
- [ ] `ensure_exec` reports whether it replaced a connection; a chip says so (B4)
- [ ] Live test: reap the connection server-side with `KILL`, then introspect —
      the honest reproduction, and it needs no eight-hour wait
- [ ] Live test: the same for `killer`, and for a tab's `USE` surviving

### Phase 3 — The carried bugs
- [ ] `CellValue::Binary`; `literal()` decides on the value (C1/B5)
- [ ] Export dialog reads `streamingExport` (C2/B6)
- [ ] New tabs take their dialect from the connection (C3)
- [ ] Audit lint rules against Elasticsearch; record the outcome even if nil (C4)
- [ ] Delete `split_sql`, `disconnect_all`, `has_stored_password` (C5)
- [ ] D4, and the invisible "Connected to …" line (C6)

## 7. What this stage is not

- **Not a new engine, and not new AI surface.** The seam is proven; leave it.
- **Not a keepalive.** §3 says why.
- **Not collapsible panes or persisted splitter positions.** The pane is
  resizable and cannot collapse to zero; that is a separate, cosmetic question.
- **Not the hands-on release checklist.** Stage 8's R1-R4 stay Stage 8's.

## 8. Risks

- **`CellValue::Binary` reaches the frontend.** It is a serialised type crossing
  the IPC boundary and rendered in three places. The risk is a variant that
  renders as `[object Object]` somewhere nobody looked — mitigated by the cell
  viewer and clipboard tests that already exist for the placeholder string.
- **Reconnect hides a real outage.** A server that is genuinely down now gets
  retried once per action rather than reported immediately. The error still
  surfaces — it is the reopen that fails — but the wording must not imply the
  connection was fine until this moment.
- **Closing a result is destructive and adjacent to Copy and Export.** It needs
  to be plainly labelled and not adjacent enough to be hit by accident. This
  project has already had one bug where feedback destroyed the results it was
  describing (`main.ts:1053`).
