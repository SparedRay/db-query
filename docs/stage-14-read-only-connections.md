# Stage 14 — Read-only connections

**Status:** 🚧 Built — 2026-09-10. R1-R7 met by test; hands-on pass outstanding.

Deferred once already: Stage 2's backlog records
*"Confirm-on-destructive-SQL for connections marked production (deferred
2026-09-05; colour and label shipped instead)"*. The colour tells you which
server you are on. It does not stop you.

---

## 1. Goal

A saved connection can be marked **read-only**. Statements that change data or
schema are refused **before they are sent**, naming the connection as the
reason. Everything that reads still works.

## 2. Why this, and why now

This app has one accident it cannot undo. It runs whatever is in the editor,
against whichever server the tab is bound to, and an `UPDATE` without a `WHERE`
on the production connection is indistinguishable from the same statement on a
local fixture until it has already happened.

Everything needed to prevent it is already built and already tested:

* `Capabilities` describes what a connection can do, and travels to the
  frontend with `ConnInfo`.
* `engine::refusal` turns `writes: false` into a refusal, before the statement
  reaches a driver.
* Elasticsearch has been running through that path since Stage 11, so the
  read-only case is not a new code path — it is an existing one, reached by a
  second route.

The feature is therefore mostly a **decision about vocabulary**, not new
machinery: a read-only connection is a connection whose capabilities say so.

## 3. Design

### 3.1 The flag is on the profile; the clamp is on the capabilities

`ConnProfile.read_only` is what the user sets and what `connections.json`
remembers. It is applied **once, at connect**, by narrowing the engine's
declared capabilities into the connection's:

    Capabilities::for_profile(engine.capabilities(), &profile)

Everything downstream keeps asking the only question it already asks — *does
this connection do writes?* A parallel notion of "may I write", checked
somewhere else, would be a second thing to forget.

### 3.2 The refusal has to say which reason it is

`refusal` currently answers with the engine's name:

> mysql is read-only through this interface — it accepts SELECT, SHOW and
> DESCRIBE.

For a MySQL connection the user marked read-only, that sentence is **false**.
MySQL writes; this connection refuses to. So `Capabilities` carries
`read_only`, set only by the clamp, and the message names the real reason and
where to change it.

### 3.3 One gate, which today is two

`exec.rs` refuses from `tab.server.capabilities` — the connection's. `elastic.rs`
refuses from `self.capabilities` — the *engine's declaration*, which the clamp
never touches. Two sources for one question, and the Elasticsearch one would
silently ignore the flag.

The Elasticsearch gate moves to `tab.server.capabilities`. That is a
correctness fix, not tidying.

### 3.4 What it is **not**

**Not a security boundary, and the UI must not imply it is.** Anyone who can
open the connection can untick the box. It stops an accident; it does not stop
a person. The control that actually enforces this is a database account without
write grants, and the app should say so where the choice is made rather than
letting a tick box imply a guarantee it cannot keep.

Also not: a confirmation prompt on destructive SQL (a different feature, still
in the backlog), per-statement overrides, or read-only *tabs*.

## 4. Milestones

- [x] **R1 — A write is refused.** `INSERT`, `UPDATE`, `DELETE`, `DROP`,
      `TRUNCATE` and `CALL` are refused on a read-only connection, before
      anything reaches the server, and the message names the connection.
- [x] **R2 — Reads are untouched.** `SELECT`, `SHOW`, `DESCRIBE`, `EXPLAIN`
      and switching database all still work.
- [x] **R3 — The menus follow.** The schema tree offers no `DROP` or
      `TRUNCATE` on that connection — the frontend reads the same flag.
- [x] **R4 — It is visible and it persists.** Marked in the rail without
      opening a dialog, and still marked after a restart.
- [x] **R5 — The assistant is told.** It does not propose writes for a
      read-only connection.
- [x] **R6 — Changing it is honest.** *(§8 — the premise was wrong and it found a bug.)* The flag applies to the connection as it
      was opened; changing it takes effect on the next connect, and the app
      says so rather than appearing to do nothing.
- [x] **R7 — Nothing regressed.** Both engines, both suites.

## 5. Task tracker

### Phase 1 — Rust
- [x] `ConnProfile.read_only`, `#[serde(default)]`, added to `PROFILE_KEYS`
- [x] `Capabilities.read_only` + `for_profile`, applied on both connect paths
- [x] `refusal` names the right reason
- [ ] The Elasticsearch gate reads the connection's capabilities
- [x] Live: a read-only connection refuses a write and the table is unchanged

### Phase 2 — The app
- [x] The checkbox, with the sentence that says what it is not
- [x] Visible on the rail without opening anything
- [x] R6 — became the ordering fix in §8 instead

### Phase 3 — Proving it
- [x] UI tests: refusal shown, menus narrowed, round trip
- [ ] R1-R7 by hand

## 6. Risks

- **A tick box that reads as a guarantee.** The mitigation is wording, and it
  is the main design risk in the stage.
- **A clamp that a third engine forgets.** Two connect paths apply it today.
  The live tests assert the *connection's* capabilities rather than the
  engine's, so a third engine that forgets is a failing test rather than a
  silent hole.
- **Capabilities are captured at connect.** R6 exists because the alternative —
  appearing to apply immediately and not doing so — is the worse failure.

---

## 7. What it cost

No new dependencies. The feature is one profile field, one capability, one
clamp and two sentences — because `engine::refusal` and `capabilities.writes`
were already the single gate, and Elasticsearch had been proving that path
worked since Stage 11.

    299 Rust unit  (+7)
     71 live MySQL (+3)
     12 live Elasticsearch
    600 UI, both engines (+10)

## 8. R6 was wrong, and finding out found a bug

The plan said the flag would take effect "on the next connect", and the refusal
was written to say so. Writing the test that asserts it turned up something
worse.

**`connect_saved` connects from the file.** It has to: the point of it is that
the stored password never reaches JavaScript, and the id is what looks the
secret up. But the profile was written **after** connecting — "only a
connection that actually works is worth writing down" — so an edit that reused
the stored password connected with the values it was replacing.

For this feature that is the dangerous direction. Clearing Read-only looked
like it did nothing; **ticking it also did nothing, while looking exactly like
protection.** Somebody would have believed a connection was guarded and run a
`DELETE` on it.

And it was never specific to this flag. Editing the port of a connection with a
remembered password had the same shape, and had since Stage 2 — connect to the
old port, then write the new one down.

### The fix, and the one it is not

On that one path — editing, with a stored password — the profile is written
**before** connecting. The order that exists everywhere else protects a *new*
password from reaching the keychain before it is known to work, and on this
path there is no new password: the box is empty and `password` resolves to
`null`, meaning "leave what is stored alone". So nothing that order protects is
at stake.

The rejected alternative is worth writing down: let `connect_saved` take the
profile from the dialog and use the keychain only for the secret. It is a
smaller change and it is **wrong** — it would let anything running in the
renderer point a profile at its own host and have the stored password sent
there. Today the renderer cannot read that password at all, and that property
is worth more than the tidier signature.

The cost accepted instead: an edit that then fails to connect stays written
down. That is the better failure — the connection was already saved, the change
was deliberate, and the edit surviving a bad port is what lets it be fixed.

The test asserts the **order** (`connect_saved`, `save_profile`,
`connect_saved`) rather than counting connects, because a count passes with the
bug still in.

## 9. Two reasons that must not be told as one

`writes: false` now has two causes, and three places were phrasing them as one:

  * **The refusal.** "mysql is read-only through this interface" is false about
    MySQL and silent about the cause. It now names the connection and says how
    to change it.
  * **The assistant's system prompt.** Same sentence, same problem — and it
    also told the model the connection had **no transactions**, which was a
    rider on the engine case that a read-only MySQL connection does not share.
    Transactions are now their own clause, keyed on `caps.transactions`, the
    way `routines` already was.
  * **A test of the live suite.** `a_read_only_connection_refuses_ddl_too`
    first asserted only that `DROP TABLE users` produced an error — and it
    **passed with the clamp deliberately removed**, because MySQL refuses that
    `DROP` on its own (a foreign key points at `users`). A test that cannot
    tell our refusal from the server's is not testing this feature, and would
    have gone on passing if the statement started reaching the server. It now
    asserts whose refusal it is.

## 10. Still to do

- **The hands-on pass.** Everything above is proved by test, including against
  a live MySQL and a live cluster, but nobody has yet ticked the box in the
  real app and tried to break their own database with it.
- Stage 13's M4 and M7 remain outstanding and are not affected by this.

---

## 11. Tab completes; Enter does not — 2026-09-10

Asked for directly, and the right way round.

CodeMirror's `completionKeymap` puts `acceptCompletion` on **Enter** and binds
nothing to Tab. In a SQL buffer the popup opens by itself as you type, so
Enter — the key you press to start the next line — silently means "accept
whatever is highlighted", and you get an identifier you never chose instead of
a newline.

Tab now accepts and Enter never does. Two details make it safe:

  * `acceptCompletion` **returns false when nothing is open**, so Tab falls
    through to `indentWithTab` and still indents. Tested, because adding this
    could otherwise have quietly removed indentation.
  * `autocompletion({ defaultKeymap: false })` is required. CodeMirror
    registers its own bindings at `Prec.highest`, so filtering Enter out of the
    exported array would not have removed it.

### The test was faster than a person

`acceptCompletion` ignores an accept within `interactionDelay` — **75ms** by
default — of the popup opening, so a keystroke already in flight cannot take an
option nobody has seen. The first draft of the Tab test pressed the key
immediately and got an indent, which looked like the binding had not worked.

The Enter test is the reason this is written down rather than just fixed: it
**passed without the wait**, and would have passed with Enter still bound —
measuring CodeMirror's guard instead of our keymap. Both tests now wait past
it, and both fail when the default keymap is restored while "Tab indents" and
"Escape dismisses" keep passing.

The starter document lists the two keys, since a binding nobody knows about is
not a feature.
