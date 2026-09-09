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

- [x] **B1 — A result can be closed**, one statement or all, and the pane
      returns to the state a fresh tab is in.
- [x] **B2 — Disconnecting clears the results it invalidated.** No stale grid
      survives a tab switch, and export cannot act on one.
- [x] **B3 — An idle connection heals itself.** Leave a connection past the
      server's `wait_timeout`, then expand the tree: it works, with no error and
      no reconnect ceremony.
- [x] **B4 — A replaced exec connection says so**, once, where the user will see
      it, because session state was lost.
- [x] **B5 — Binary and text agree.** A binary-collation `VARCHAR` that the grid
      shows as text exports as text; a real BLOB is still refused.
- [x] **B6 — A cluster is not offered an export it cannot perform.**
- [x] **B7 — Nothing regressed.** Every suite, both engines, both live backends.

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
- [x] `ensure_exec` returns whether it **replaced** a dead connection (rather
      than opening a first one); `ScriptResult.reconnected` carries it, and a
      "reconnected" chip names what the new session cost (B4)
- [x] Live test: `KILL` the connection server-side, then introspect — asserts
      both that it works and that the id came back *different*
- [x] Live test: the same for `killer`

### Phase 3 — The carried bugs
- [x] `CellValue::Binary { bytes: Option<usize> }`; `literal()` refuses on the
      **value**, and text under a binary-hinted column now exports as text.
      Rendered in the grid and the viewer, with a UI test against
      `[object Object]`
- [x] Export dialog reads `streamingExport`, with its own reason string (C2/B6)
- [x] New tabs take their dialect from the connection — **and the editor now
      reads the field at all**: it had been written to the session file since
      Stage 1 and consulted by nothing, with `MySQL` hardcoded in both places.
      Non-MySQL engines get `StandardSQL`, so `"quoted"` identifiers highlight
      correctly (C3)
- [x] Lint audit (C4). **One finding, recorded not fixed:** `mask_impl` treats
      `"` as a string delimiter — MySQL's default — so a cluster's quoted
      identifiers are masked away and schema lint is silently inert for them.
      It fails *safe* (silence, not false errors), and the same mask decides
      statement boundaries and auto-LIMIT on the execution path, so making it
      dialect-aware does not belong in a bugfix release. Pinned by a test in
      `split.rs` that says so
- [x] Deleted the `split_sql`, `disconnect_all` and `has_stored_password`
      commands, their `api.ts` wrappers, and the `SplitOutput`/`StatementSpan`
      types orphaned with them. `session::disconnect_all` stays: an internal
      `pub fn` is not the frozen surface — the IPC command was (C5)
- [x] C6, and **two reversals the tests forced** — see §12

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


---

## 12. Two things Phase 3 got wrong, and the tests that said so

Both were written, run, and reverted. Recorded because the reasoning that
produced them was plausible, which is the kind worth leaving a marker against.

### D4 — pruning remembered workspaces was the wrong fix

The plan called for dropping session entries whose connection no longer exists.
Implemented at boot, then moved to save time when the first version broke
restore. Both failed the same test:

> *"connecting to one server does not forget another's tabs"*

The invariant that test defends is already written at the top of `session.ts`:
remembered workspaces are **written back untouched**. And the reasoning is
better than mine was — **an id we do not recognise is not an id that is gone.**
It is a profile this build has not read yet, or one whose connection is about to
come up. Pruning on "not a saved profile" is precisely the bug that comment
exists to prevent, and no signal in the session file distinguishes the two.

**D4 is closed as won't-fix.** A profile removed through the UI is handled by
`forget`, where the fact is actually known. The residual case — a file edited by
hand, or a removal that crashed halfway — costs one unused object in a JSON
file, which is a much smaller price than deleting someone's remembered tabs.

### C6 — the "Connected to …" line *is* visible

It was deleted on the grounds that the first tab activation overwrites it, so
nobody had ever seen it. A test disagreed: **reconnecting to a connection whose
tabs already exist activates nothing**, so the line stands — and it is the only
confirmation that the reconnect worked.

Restored, with the defect that was actually there fixed: it said "MySQL" on
every connection, including a cluster. It now names the engine.

Then it threw on the connect path, because the test fixture's `ConnInfo` has no
`capabilities` and the new code read one. Optional-chained, like `engineLabel`
in `connections.ts`, which had already learned this: **a cosmetic status line
must not be able to take down connecting.**


---

## 13. Tree icons and a quick filter — 2026-09-09

Added after Stage 12's tracker was already complete, from hands-on use.

### The icons were half emoji

The tree mixed **emoji** — `🗄` database, `👁` view, `🔑` key — with **text
glyphs** — `▦` `ƒ` `⚙` `·`. Three consequences, none fixable by choosing
different characters:

- Emoji render in colour from the system emoji font, so they ignored
  `color: var(--fg-dim)` and half the tree was grey while half was not.
- Each character has its own advance width, and `.node .icon` had no fixed
  size — so **the labels beside them started at different x positions**. The
  Tables / Views / Procedures column visibly failed to line up, which is what
  reads as "weird" before you can name it.
- Availability and text-vs-emoji presentation differ by platform. `⚙` in
  particular flips to a colour emoji on some systems and not others.

**No icon pack.** The shapes did not justify a dependency and a licence
obligation, so `src/icons.ts` draws them: one 16-unit grid, one stroke width,
`currentColor` throughout, in a fixed box. They inherit the theme, scale with
the font, and every label now starts at the same x.

### Then the rest of the chrome, for the same reason

The tree was where it was most visible, but not where it stopped. A sweep for
non-ASCII characters in `src/` and `index.html` found the rail's own buttons
(`✦` assistant, `↺` history, `⚙` settings), the tree's refresh (`⟳`), the tab
strip's busy marker (`◴`) and its failed-statement badge (`✕`), and the tree's
open/closed twisties (`▸` `▾`).

`✦`, `↺` and `◴` are the risky ones — thin font coverage, and a missing glyph
renders as a hollow box. `⚙` is worse than missing: it flips to a **colour
emoji** on some platforms, so the button that is meant to be quiet chrome
becomes the loudest thing on screen.

All now come from the same set. Three details worth keeping:

- **The twisty is one chevron, rotated by CSS**, not two characters. Two glyphs
  can disagree about size and baseline; one drawing cannot.
- **The busy marker reuses the app's existing `.spinner`.** A glyph that spins
  is a glyph that might not exist.
- **The rail's icons are injected from `icons.ts`**, not written into
  `index.html`. The same paths in two files is the same drawing in two files,
  and they drift.

**The SVG swap broke clicking a result tab**, and only a click test could have
seen it. A `display: block` SVG inside a plain `inline` span makes that span's
box stretch to the whole line — so the statement tab's ✕ became a 159px hit
area covering the tab, and clicking a tab to *select* it closed it instead. The
containers are `inline-flex` now. A screenshot showed nothing wrong: the icon
was drawn in the right place and the right size; it was the invisible box
around it that had swallowed the tab.

The settings icon went through two drafts: a cog drawn as a circle with eight
radiating lines reads as a **sun** at 16px — indistinguishable from a
brightness control. It is sliders now. A cog's teeth need more pixels than this
to be teeth.

### A quick filter over what is loaded

`src/treefilter.ts`, an input above the tree, `Ctrl+P` to focus, Escape to
clear, and a count beside it. Scope is deliberately **already-loaded nodes**:
the tree is lazy, so searching the server would be a different feature with a
round trip and a spinner. This one is for "I know the name, get me there".

**Filtering is presentation only.** The obvious implementation walks the tree
setting `hidden = false` along the path to each match and puts it back
afterwards — which makes the filter an owner of the expansion state, and the
moment a lazy load finishes or the user clicks mid-filter, the two disagree.
Instead a class on the root reveals collapsed containers for as long as the
filter is on, and clearing it gives back exactly the tree that was there,
because nothing about it was changed. A test pins that: collapse a group,
filter to reveal something inside it, clear — the group is collapsed again.

Two things the tests found:

- **The filter searched nothing.** Each connection's tree lives in a container
  inside `#tree`, so the walk hit a non-wrapper element first and stopped. The
  count read "none of 0" beside a full tree.
- **A database must never be filtered out.** Typing before expanding anything
  emptied the panel completely — including the one node that could have loaded
  the tables the filter was looking for. A filter that hides the only control
  you can click has not focused the tree, it has broken it.

---

## 14. Three things that were hard to read — 2026-09-09

All from the same session as §13, and all the same complaint from a different
angle: the app was showing the right thing in a shape nobody could take in.

### An icon per column type

The tree drew one dot for every column, so a table was a list of names with no
shape to it. Now the icon slot says what the column *holds* — `#` for a number,
`A` for text, `{}` for a document, a calendar, a switch, `10` for bytes — and
an unrecognised type keeps the plain dot.

`src/coltype.ts` is a lookup over **type names**, not over one engine's types:
`varchar` and `keyword` are both text, `long` and `decimal` are both numbers.
The vocabulary is deliberately Rust's `TypeHint` (`decode.rs`, which classifies
*result* columns for alignment) plus `object`, which is the one shape the grid
never needed to tell apart and the tree does.

Two decisions worth writing down:

- **The key moved out of the icon slot.** A primary key used to be drawn as a
  key *instead of* a column dot, which was fine when the alternative was a dot
  and wrong the moment the slot started carrying the type — a primary key is
  also a column of some type. The key is now a small mark beside the type name,
  and both facts are on screen at once.
- **`bit` is filed with the blobs, not the booleans.** MySQL hands a `BIT`
  column over as bytes and the grid shows it as bytes. `bit(1)` tempts you the
  other way, and `bit(8)` is what that costs.
- **`tinyint(1)` stays a number.** It is MySQL's boolean by convention, and
  nothing in `information_schema` says which convention this column follows.
  Claiming otherwise would put a switch beside every small integer in the
  database. A test pins that, next to the one pinning `boolean`.

The icon is a guess made from a type's name; the type itself is written beside
it. That is the whole reason it is allowed to guess.

### Settings became five sections instead of one column

Everything was stacked in one scrolling `fieldset` column, so reaching the
assistant's base URL meant scrolling past fonts and update checks, and every
section was as tall as the tallest.

Vertical tabs now, one pane at a time: Appearance, Editor, Assistant, Updates,
About. The panes are **hidden, not rebuilt** — `commit()` reads every control on
every change, and a pane that had to be reconstructed would drop whatever was
half-typed into it. A roving tabindex makes the strip one Tab stop with arrows
moving inside it, which is both what a tablist is supposed to do and what stops
Tab from walking through five buttons to reach a field.

The dialog's height is **fixed** rather than fitted. Sized to its content, a
five-line pane and a fifteen-line one move the Close button every time you
switch sections.

The section is remembered for the session but not persisted: reopening settings
to adjust the same thing again is the common case, and a new launch is a new
task.

This broke fifteen tests, all in the same way and all correctly — a control in
a hidden pane is not clickable, which is true of the user too. They go through
`settingsSection()` now rather than reaching past the tabs with `evaluate`.

### Definitions arrive as one line, and now do not

MySQL stores a **view** as a normalised one-liner: whatever you wrote, `SHOW
CREATE VIEW` answers with a single line several hundred characters wide.
"Examine definition…" put exactly that in a tab — correct, and for the question
being asked, close to no answer at all.

`src-tauri/src/sqlfmt.rs` lays it out. Two rules keep it from being a liability:

1. **It only acts on dense text.** If every line is already under 120
   characters, the input is returned untouched. That is what separates the
   one-lined view from a routine someone laid out by hand — `SHOW CREATE
   PROCEDURE` returns the body *as written*, newlines and all, and reformatting
   that would be replacing an author's work with a machine's.
2. **It only moves whitespace.** The output is re-lexed and compared with the
   input token for token, and the original is returned if they differ at all.
   A formatter that changes what a definition means ends with someone running
   the result, so it fails closed rather than trusting its own rules to be
   complete.

The rules themselves are ordinary — clause keywords start lines, commas hang
under their clause, a parenthesis gets lines of its own when it holds a query or
is simply too wide to read. Three details were not ordinary, and each was found
by a test rather than by reading the code:

- **Keywords are re-emitted as written, never as matched.** The first draft
  pushed the literal `"CASE"` and `"END"` it had matched on, which uppercased
  half of a lowercase view — and rule 2 caught it, because `case` and `CASE` are
  not the same token.
- **`@` is spaced by what the author wrote.** It joins the halves of a definer
  (`` `root`@`localhost` ``, where MySQL rejects the spaces) and introduces a
  session variable (`SET @x = 0`, where it needs one). Nothing in the tokens
  distinguishes them; the source's own adjacency does, so the lexer records it.
  The same fact fixes `count(*)` versus ``CREATE TABLE `t` (…)``, which a
  keyword list had been getting wrong.
- **`END` closes two different things.** `CASE … END` in a select list ends an
  expression; `IF … END IF` in a routine ends a block. Treating them alike
  dedented the rest of the statement. A `CASE` that starts a line is a
  statement, one that appears mid-line is a value, and the block stack
  remembers which.

Verified against the live server, not only against a fixture: the shape of that
one line is MySQL's choice, so the test asks the real `user_totals` view for its
definition and checks the clauses start lines — and its sibling checks a
**table**'s definition comes back byte-identical to what the server wrote, which
is the half that would have been easy to break quietly.

The first draft of that live test asserted a `WHERE` clause the view does not
have. The server said so.

---

## 15. The linter learns which driver it is talking to — 2026-09-09

Three reports, two of them the same underlying mistake: the linter assumed
MySQL, and the results pane assumed it would be painted by something else.

### A fully-qualified name was reported as an unknown table

`SELECT * FROM `poc`.`users`` drew a warning saying `poc` was an unknown table
— a squiggle under correct SQL, which is precisely the noise this linter's
opening comment says it must never produce.

The cause is a seam between two pieces that were each right. Masking replaces
every quote character with a **space** so byte offsets survive, so
`` `poc`.`users` `` reaches the tokenizer as ` poc . users `. The tokenizer read
a dotted identifier as an unbroken run of word characters and dots, so it saw
`poc`, then a stray dot, then `users` — and `collect_tables` took the first
token after `FROM` as the table name. The database was reported as the table.

The unquoted form `poc.users` was handled correctly all along, which is why this
survived: the check that skips qualified tables was working, and simply never
ran. The tokenizer now continues an identifier across a dot **with whitespace on
either side** — which is also legal SQL written by hand — and `t.*` still ends
the identifier before the star, as it always did.

### It only spoke MySQL

Masking keeps the contents of backtick-quoted identifiers and blanks
double-quoted ones, because in MySQL a double quote is a string. Elasticsearch —
and standard SQL — is the other way round. So on a cluster connection **every
identifier in the statement was masked away**, and the schema checks quietly ran
against what looked like an empty query.

That is the worst kind of failure this codebase has a rule about: nothing was
wrong on screen, so nothing prompted anyone to look.

`lint::Dialect` now carries the two facts the checks actually need — the
identifier quote, and whether `DELIMITER` blocks exist — and `lint_sql` builds
it from **the engine, not its name**, per the standing rule that behaviour
branches on a capability. The quote comes from a new `Engine::ident_quote`,
which sits beside the `quote_ident` the trait already had; `delimiter_blocks` is
an existing capability. A tab with no connection gets MySQL, which is what the
editor already highlights it as — the two fall back together rather than each
guessing separately.

The `DELIMITER` rule earned its own test. Finding that word blanks the entire
lint, because the statement boundaries stop being ours to trust — but only on an
engine that has such blocks. On one that does not, it is an ordinary word, and
giving up the whole lint over it is giving up for nothing.

### The results pane was never painted

Reported as "the table background uses the same colour, so it looks weird", and
measuring it said why: `#results-pane`, the statement tab strip, the grid host,
the table and its cells were **all transparent**. Every row sat directly on the
window's own ground — the same colour as the gap between the panes. Only the
sticky header and the bottom bar had a surface, which is what made it look
*nearly* right rather than obviously broken.

It uses the two tokens the script tab strip above it already uses: `--bg-tabs`
behind the tabs, `--bg` on the surface they open onto, so an active tab and its
grid are visibly the same sheet. A test asserts the separation exists in both
themes — as "differs from the page ground", not as a hex value, because the
point is the contrast and pinning the colour would fail on the next palette
change for no reason.

---

## 16. Confirmed by hand — 2026-09-09

**B1-B7 all verified** against the installed app, and the §15 changes with
them. Everything this stage set out to fix has now been used, not only tested:
results close, disconnecting takes its stale grid with it, an idle connection
heals itself without ceremony, the replacement says so once, binary and text
agree, and a cluster is not offered an export it cannot perform.

B5 is also Stage 8's C1, stated twice in two trackers — carried since Stage 5
and closed here.

Confirmed in the same pass, and recorded in their own trackers: **R1, R3, R4 and
S9** (Stage 8 §6 — the updater has now actually run, against real published
releases) and **H8** (Stage 9).

Still open, and honestly so:

- **R2** — the first double-click install on a clean Windows machine. Not
  failed; not reported. An install exists, since R3 updated one.
- **A2** — the Windows licence file is readable at Settings → About →
  "Third-party licences" on the Windows install; nobody has read it yet. See
  Stage 8 §6 for where it lives and why it is not a CI artefact.
- **N1** — CI runs every suite on both platforms and is green, which is not the
  same statement as someone having used the built app on both.

---

## 17. The licence file was reviewed, and it was wrong — 2026-09-09

A2 asked whether the audit covered the Windows tree. It did. Nobody had read
what it *said*, and reading it found twelve defects — none of which had ever
failed anything. `cargo about` exited 0 every time, `--fail` never fired, CI was
green, and the file looked plausible. That is the whole lesson: **an
obligation you generate is not an obligation you have met.**

### The one that mattered

Roughly fifty crates — the whole Tauri family, all of windows-rs, chrono, sqlx,
libm, minisign-verify, the unic crates — carried this:

```
MIT License
Copyright (c) <year> <copyright holders>
```

That is cargo-about's fallback **template**, used when it finds no licence file.
MIT's one substantive condition is that the copyright notice be retained, and a
literal `<copyright holders>` retains nothing. Fifty crates with no attribution
at all, in the file whose entire purpose is attribution.

Reading cargo-about's own trace found two causes, neither of them "the crate
ships nothing":

- **The filename never reaches the scanner.** It walks with the `ignore`
  crate's default file types, and `LICENSE_MIT` — an underscore, which is how
  the whole Tauri family spells it — matches none of them. Tauri's
  `LICENSE_APACHE-2.0` *is* scanned, and only because it ends in `.0` and so
  looks like a man page. windows-rs spells it `license-mit`, lowercase, and
  loses the same way.
- **Confidence.** `minisign-verify`'s LICENSE is detected as MIT at 0.64 against
  a threshold of 0.8, and discarded. Lowering the threshold is not the answer:
  at 0.3 the file grows by 2500 lines of source whose headers now match.

Thirty-six crates are now named in `about.toml` and carry their real notices.
Eleven remain without one, and each was checked by hand: the unic crates,
dlopen2, siphasher and webview2-com publish **no licence file at all**, and the
sqlx crates publish `LICENSE-MIT` as a symlink to `../LICENSE-MIT`, which does
not resolve inside the package — its content is that string. For those, the file
now says so plainly instead of printing a template.

### The rest

- **`ring` was shipping source code.** It puts four licences in one file, so the
  scanner fell back to hunting headers and extracted whole `.rs` and `.h` files
  that merely open with the ISC header — `eddsa_digest()`, a set of C typedefs,
  and a stub reading `mod bits_tests;`. Eighteen "ISC License" entries, for
  three crates that are actually ISC. cargo-about *has* a built-in workaround
  for exactly this, and it **fails silently**: ring 0.17.14 restructured its
  licensing, the clarification no longer matches, and the reason is logged at
  `debug`. Ours is written against the files ring ships today. `schemars_derive`
  (carrying a copy of regex-syntax's `escape()`) and `libdbus-sys` (vendored
  dbus C headers) had the same problem and are named too.
- **`tracing-core` was attributed to the wrong people** — it appeared twice,
  once correctly as "Tokio Contributors" and once under the `zip` crate's
  copyright. The only outright false statement in the file.
- **`atomic-waker`** was headed "MIT License" over a body that opened with the
  full Apache-2.0 grant.
- **The counts counted blocks, not crates**, which is why "ISC License: 18" read
  as eighteen ISC crates. They are counted by distinct package now, and the npm
  half is included.
- **db-query listed itself**, under a placeholder copyright, in its own
  third-party licence file. `cargo-about`'s `private.ignore` keys off `publish`
  in Cargo.toml, which was never set.
- Duplicate blocks (ring's Apache text twice, differing by indentation;
  `miniz_oxide` twice, differing by a blank line) are merged, compared on
  collapsed whitespace so a reflow cannot hide one.
- The npm half has a banner and the same block layout as the crates, instead of
  reading as a second document stapled on.
- The build-time-only note claimed four crates when the real superset is several
  dozen.

### What stops this happening again

Not care. `scripts/attribution.mjs` now **refuses to write the file** if any of
these defects is present: a clarification that has gone stale, source code in a
licence block, or the app listing itself. Ten tests in
`scripts/attribution.test.mjs` cover each case, and both they and the generation
run on both platforms in CI.

The guard matters more than the fixes. Every clarification is keyed by a
SHA-256, so a dependency bump silently un-does one — which is precisely what had
already happened to cargo-about's own `ring` workaround, in a file that looked
fine. A build that fails is the only version of this that stays fixed.

### One mistake worth recording

I destroyed `about.toml` mid-edit with `open(p, "w").write(open(p).read() ...)`,
which truncates the file before the read runs. It was rebuilt from `git` plus
the generated checksums, and every edit after that read first and wrote once.

---

## 18. A Format action for the editor — 2026-09-09

`Ctrl+Shift+F`, and a **Format** button in the editor header. The selection if
there is one, otherwise the whole tab.

**The formatter already existed** — `sqlfmt::tidy`, written for the
one-lined view definitions `SHOW CREATE VIEW` hands back. What it needed was to
drop rule 1. `tidy` returns its input untouched when every line is already
narrower than 120 characters, which is exactly right when it is guessing whether
to act and exactly wrong when the user pressed a button. So the layout moved
into a new `format()` that always acts, and `tidy` is now `format()` guarded by
rule 1. Rule 2 — re-lex the output and compare token for token, returning the
original if anything differs — is untouched, and here it becomes the interface:
`format` returns `None`, the command returns an error, and the UI says so.

### `DELIMITER` would have corrupted scripts, silently

The first thing the new entry point met was the shape this app generates itself:

    DELIMITER $$
    CREATE PROCEDURE p() BEGIN SELECT 1; END$$
    DELIMITER ;

laid out as `END$$ DELIMITER;` — one line, **token for token identical to the
input**, so rule 2 could not see it, and no longer a script that runs.
`DELIMITER` is a *client* directive that owns its whole line; the server has
never understood it.

The fix is to split the text on those lines, lay out each run between them, and
copy the directives across verbatim. The rule for recognising one is
`split.rs`'s, quoted rather than reinvented: two modules disagreeing about what
a directive is would be a bug neither could show you. `chunks()` does not track
strings or comments the way `split.rs` does; the consequence is bounded and in
the safe direction, and is written down where it happens — a `DELIMITER` opening
a line *inside* a string would split a run, and rule 2 then refuses the whole
request. Refusing to format is a worse outcome than formatting. It is not a
wrong one.

A second, smaller thing fell out: `$` is a word byte, because MySQL identifiers
may contain one, so `END$$` lexes as a **single word** and the block closer
hides inside it — leaving every routine body indented one level too deep.
Trailing `$` is now stripped when matching keywords, and only when matching; the
token is still emitted exactly as it came.

### Two decisions in the UI

**The cursor keeps its place, exactly.** Its position is found by counting the
non-whitespace characters in front of it and finding the same count in the
result. That is exact rather than approximate — and it is exact *because* the
formatter only moves whitespace. The rule that makes the feature safe is the
same rule that makes the cursor land where it was.

**The edit is isolated in the undo history.** Found by a test, not by reasoning:
"one undo puts the buffer back" failed, because CodeMirror groups nearby edits
by time and a format arriving within half a second of the last keystroke merged
into that typing — so Ctrl+Z threw away both. For an action people reach for
speculatively, that is the wrong answer twice over. `isolateHistory` fixes it
and the test now means what it says.

Statements also gained a blank line between them at the top level, and only at
the top level: the `;`s inside a routine body belong to one statement and must
not be spread apart.

### Proved

9 UI tests (both engines), 8 new Rust tests. Three confirmed able to fail before
being trusted: dropping the `DELIMITER` chunking reddens two, and pinning the
cursor to 0 reddens the cursor test. 288 Rust unit tests and 534 UI tests pass.

---

## 19. The editor's selection was never visible on the dark theme — 2026-09-09

Reported as a Windows problem: Ctrl+A "seems weird", the text does not look
highlighted, and it is fine on Linux.

**The first diagnosis was wrong and is left here rather than tidied away.** It
said: not a platform problem, a dark-theme one, and the platforms differ only in
which theme each machine is on. Both machines are on the **light** theme, so
that explanation is dead. What is left is simpler and less satisfying — the same
colour, at 1.44:1, on two different displays, visible on one and not on the
other. Contrast that low is not a colour choice, it is a coin flip.

### What was wrong

`themeRules` carried a rule meant to colour the selection:

    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": { … }

CodeMirror's own base theme carries this one:

    "&dark.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground": { background: "#233" }

Five classes to our three. **Ours never applied, in either theme**, and had not
since `drawSelection` was added — the rule was written at the same time and
looked like it worked. Measured in a browser rather than reasoned about: the
painted band was `rgb(34,51,51)` in both themes.

On the dark theme that is `#233` on a `#16181d` ground — a contrast ratio of
**1.34:1**, which is to say invisible. On the light theme it fell back to
CodeMirror's `#d7d4f0`, a perfectly reasonable lavender, which is exactly why
nobody saw the bug on one machine and did on the other.

### The fix, and why the selector is written out longhand

The selector now matches the shape the base theme uses, because that shape *is*
the specificity — it is load-bearing, not decoration, and there is a comment
next to it saying so. The blurred state got the same treatment: its base rule
has two classes and ours had one.

The colours moved to their own tokens, `--sel-editor-bg` and
`--sel-editor-bg-blur`, rather than borrowing `--sel-header-bg`. The two roles
sit on different grounds — a selected grid header is a chip on `--bg-raised`,
this is a band behind body text on `--bg` — and reusing the header colour scored
only 1.56:1. Chosen by measurement: **2.20:1** against the editor background,
with `--fg` still at 5.76:1 on top of it, which is the constraint that stops
"make it brighter" from being the whole answer.

**The light theme was raised a second time**, and that is the part worth
remembering. The first attempt took it from 1.44 to 1.65 — a measurable
improvement, chosen because the dark theme was assumed to be the real problem.
It was still the faint end of the scale, on the only display anyone had checked
it on. Both themes now sit at ~2.1:1 (`#8ab4f8` on white, `#35507f` on
`#16181d`), and the test's bar moved from 1.6 to **2.0** so that the earlier
"improvement" would not pass it either.

### The test asserts a contrast ratio, not a colour

`tests/ui/editor-selection.spec.ts` computes WCAG relative luminance in the page
and asserts the band is distinguishable from the ground **and** that the text
stays readable on it. A colour assertion would pass again the moment somebody
renamed a token; this fails whenever the selection stops being something a
person can see, however that happens. Restoring the old selector reddens it in
both themes, at 1.34 and 1.44 — checked, not assumed.

### Two things found on the way

The caret is fine, and fine **by accident**: `drawSelection` forces
`caret-color: transparent !important` on `.cm-content`, so our `caretColor` rule
there is inert too, and the visible caret is `.cm-cursor` in the base theme's
black / `#ddd`. Both happen to be right. The dead rule is kept as the fallback
if `drawSelection` ever goes, with a comment saying it is currently overridden —
a rule that looks like it works and does not is the whole subject of this
section.

`drawSelection` does hide the native selection, so there is no second highlight
underneath and no `::selection` rule of ours to write.

---

## 20. Editor colour presets, and the dark theme's syntax colours — 2026-09-09

Found while measuring §19: **the editor's syntax colours never followed the
theme at all.** The same `defaultHighlightStyle` was in force on both, and its
colours are fixed and written for a light background. Measured against `--bg`:

| token | colour | on light | on dark |
|---|---|---|---|
| keyword | `#770088` | 9.81:1 | **1.81:1** |
| comment | `#994400` | 6.60:1 | **2.69:1** |
| number | `#116644` | 6.98:1 | **2.54:1** |

Keywords at 1.81:1 are not dim, they are unreadable. Nobody reported it because
the app's own chrome followed the theme correctly, so the editor looked *washed
out* rather than broken — and this is the third rule in three days that appeared
to be wired up and was not.

### The fix is the feature

Replacing `defaultHighlightStyle` with a `HighlightStyle` over `var(--syn-*)`
fixes the dark theme **and** makes an editor theme a block of custom properties
with no code behind it. Those are the same change, which is why the answer to
"what is the quick path to custom themes" was "fix this bug".

Eight tokens — keyword, string, number, comment, name, type, function, punct —
mapped to the tags `@codemirror/lang-sql` actually emits, read from the package
rather than guessed. A rule for a tag the grammar never produces is another rule
that looks like it works.

### Three presets, not a colour picker

*Default*, *Muted*, *High contrast*, chosen in Settings → Appearance and stored
like any other preference. Each is written for **both** grounds, so the editor
palette and the light/dark theme stay separate choices and the editor never
disagrees with the window around it. `data-editor-theme` on `<html>`; Default
sets no attribute, so its palette is the plain `:root` block.

A colour picker was considered and rejected for now: every custom palette is one
the contrast test below cannot vet in advance, and the value of these presets is
precisely that they *have* been vetted.

### 48 measurements, and one test that matters more

`tests/ui/editor-theme.spec.ts` asserts every token in every preset on both
grounds — 3 × 2 × 8 — at 4.5:1, and at **7:1** for High contrast, which has to
earn its name.

But the test that actually catches this bug is the small one: *the preset
reaches the text in the editor*. Reinstating `defaultHighlightStyle` leaves all
48 palette assertions green — the tokens are still defined and still readable,
they are simply not being used — and reddens that one alone. Checked by doing
it. A guard on the values would have missed the entire defect it was written
for.

---

## 21. A connection with nothing to remember — 2026-09-09

Reported from use: an Elasticsearch cluster with no authentication opened the
connect dialog on **every** click, forever, with no way to make it stop.

The gate asked the wrong question:

    if (entry.saved && entry.profile.rememberPassword) { connect } else { ask }

`rememberPassword` means *a secret is in the keychain*. A cluster with no
authentication has nothing to remember, so the answer was permanently no. The
question that should have been asked is **"do we have what it takes"**, and
there are two independent ways for that to be yes: a secret is stored, or none
is needed.

### `needs_secret`, decided in Rust

`ConnProfile::needs_secret()` — false when the profile declares an auth scheme
that carries no secret, and false when the account is known to have no password.
It travels on `ProfileView` beside `remember_password`, so the frontend never
reimplements the rule and then disagrees with the backend about it.

It does match on the engine, which the capability rule normally forbids. The
rule is about what an engine can *do*; this is the shape of the profile itself.
MySQL authenticates with a user and a password, always. The HTTP engines declare
a scheme, and one of the schemes is "none".

### Telling "there isn't one" from "we don't know"

MySQL has no declaration to read, and an account with an empty password is a
real thing. "Nothing stored" could not be told apart from "never asked", and
that ambiguity *is* the bug in its second form. So `ConnProfile.no_password`
records the fact, set when the connect dialog is confirmed with **Remember
ticked, the box empty, and nothing already stored** — the only reading of which
is "there is no password". With something already stored, an empty box keeps its
older meaning: leave it alone, so a port can be edited without retyping.

### Two tests pushed back, and both were right

`=== false`, not `!`: an absent `needsSecret` means "we do not know", and not
knowing falls back to asking. A MySQL profile with no stored password still
opens the dialog, which is correct — we do not know the password.

And `rememberPassword` stays as an independent reason. Gating on `needsSecret`
alone made **every remembered password prompt again** the moment the field was
missing — which is exactly the Stage 2 bug that two tests in
`regressions.spec.ts` exist to catch. They caught it. The gate is now either
reason, so a missing field degrades to the old behaviour instead of regressing
the oldest one.

### And a guard that fired for the right reason on the wrong thing

`profiles::the_file_contains_no_secret` hunted the config file for the substrings
"password" and "secret". `noPassword` — a boolean saying an account *has* none —
tripped it. A word filter cannot tell that from a field that holds one.

Loosening the filter would have been the wrong repair. It now asserts **the
exact set of keys written**, so any new field on `ConnProfile` fails until
someone adds it to the list. That is a decision made on purpose, rather than a
word nobody happened to choose.

---

## 22. The active result tab was white on white — 2026-09-09

`.tab.active` said `background: var(--bg)` and nothing else. On the dark theme
that reads; on the light theme `--bg` is `#ffffff` against `--bg-raised`
`#f4f6f9`, which is one hairline of difference — no signal at all when a script
ran four statements and you need to know which result you are looking at.

It now carries an accent top border, which is the idiom the script-tab strip
already uses for exactly the same question, so the two strips answer it the same
way. The border is reserved as transparent on every tab, so selecting one cannot
shift the row by a pixel.
