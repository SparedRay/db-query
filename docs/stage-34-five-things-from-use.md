# Stage 34 — five things from use

**Status:** 🚧 In progress — 2026-11-10. Five reports in one message, three of
them bugs and two of them design. Hands-on confirmation outstanding.

The indicator canvas: https://claude.ai/artifact/ApZe7M9FqFfYBMh6YtMZie

---

## 1. The ask

> *"First, select dropdowns. Does not seem to be working. Tested Export and now
> does not allow to select a second option (When I click does not select it)"*

> *"Second let do a change on the connection panel. Right now it shows the
> Color, the name of the connection and then there a dot with the default
> database. But since this database is not neccesarily the default there no
> reason to show it there (Is also highlighted on bold so is redundant). With
> this in mind let do a similar change so same way we have the AUto limit
> checkbox let have a checkbox to wrap or not the text."*

> *"This will include reviewing the responsiveness of this panel. today if we
> shrink enough buttons seem to be taking all height."*

> *"we have some UX mismatch on assistant button, migration and others … so this
> we probably want to unify into a single UI language."*

> *"Also when disconecting it shows no loader so sometimes it feels stuck. We
> need to make sure any action we do that might have delay shows a proper loader
> always."*

## 2. The dropdown, which was not a dropdown

**The report was right and both of my first two theories were wrong.** The
first was that the control was broken; the second, after finding the one
disabled `<option>` in the app, was that it was greyed and the person had not
noticed. The answer to *"which one, and did it look greyed?"* settled it:

> *"i ONLY TRIED THE ROWS ONE, LOOKED NORMAL AND DIDN ALLOWED TO CHANGE"*

**Clicking anything in the grid disabled it.** `rerunOk` read

```ts
sql !== null && results.isEverything() && canStream
```

and `isEverything()` is false the moment a single cell, row or column is
selected. Reproduced in all three cases. So the sequence that provokes it is
the ordinary one: click a row to look at it, then reach for Export.

Two things kept it invisible. WebKitGTK greys a disabled `<option>` so faintly
that it reads as ordinary — which is exactly what was reported. And the
explanation was written **into that option's own label**, so it existed only
while the popup was open; once it closed, nothing on screen accounted for the
click that did nothing.

**And the refusal was guarding a path that does not consult it.** Choosing
"every row" calls `exportRerun(tabId, sql, …)` and never reads `data`. The
selection is irrelevant on that branch, so the check protected nothing and cost
the one option a person is most likely to want. It is gone. The genuine
refusals — an engine that cannot stream, a result that came from no re-runnable
statement — remain, and now say so in the dialog where they can be read without
opening anything.

**E8 is inverted rather than deleted.** It asserted *"the re-run is disabled,
with a reason, while a subset is selected"* — which is the reported bug,
written down as a guarantee. Its second half now proves the other half of the
argument: choose "every row" with a selection active and `export_rerun` is what
runs.

**A trap found on the way.** `toBeDisabled()` does not report a disabled
`<option>`; Playwright calls every option enabled. My first version of the new
test used it and passed whether or not the fix worked. The assertions are
`toHaveJSProperty("disabled", …)`, which the two older tests in that file had
already learned.

## 3. The connection head

`syncConnLabel` built `name · database · N running`, where the database was
`tabs.active()?.activeDb` — a fact about **one tab**, printed on the header of
the connection as though it were a property of the server. And the schema tree
immediately below already marks it `.node.db-active`, bold and in
`--fg-strong`.

Two copies of one fact, and this was both the weaker and the less true one. It
is gone.

## 4. Wrap

A checkbox beside Auto-LIMIT. `EditorView.lineWrapping` was hard-coded into the
shared extension array; it is a compartment now, with `setLineWrapping`.

**Not `sql-only`.** A dictionary tab is TOML in the same CodeMirror with the
same long-line problem, so wrapping applies to every tab kind — which caught a
bug while it was being written. `onActivate` branches for dictionary tabs and
**returns early**, so applying the setting below the branch left those tabs
wrapped whatever the box said. It is applied above the branch.

**Persisted but not mirrored in Settings.** Auto-LIMIT has a toolbar control
*and* a default in the Settings dialog; this deliberately does not. It is one
click away in the header, and a second control is a second thing that can
disagree — which is the defect §3 exists to remove.

## 5. A toolbar that wrapped into columns

`.pane-head` was `display: flex` with **no `flex-wrap`**. So narrowing a pane
never wrapped the row: every control kept `flex-shrink: 1` and was squeezed
until its own label broke across lines, turning a 34px bar into a stack of tall
thin buttons. That is the "buttons seem to be taking all height" in the report,
and it is why two docks open at once were unusable.

Three changes, in order of how much work each does:

**It wraps**, with `flex: 0 0 auto` and `white-space: nowrap` on the controls.
The invariant is the button's *height*: whatever the width, a control keeps its
own size and moves to another row. Measured on the editor head:

| pane width | head height | Run button |
|---|---|---|
| 951px | 35px — one row | 22px |
| 671px | 60px — two rows | 22px |
| 144px | 138px | 22px |

**The shortcut hints go before it gets desperate.** `Run Ctrl+Enter` is three
times the width of `Run`, and the shortcut is still in the title and in the
menu — the least load-bearing thing in the bar.

**A container query, not a media query.** `#editor-pane` carries
`container-type: inline-size`, because with three docks open the *window* is
1280px wide while the editor has 144px. A media query answers a question nobody
asked.

**And below the width of one button it scrolls**, rather than bleeding past its
pane. Wrapping has nothing left to do at that point and an invisible overflow
is worse than a scroll.

## 6. Anything slow says so

`connect` set `entry.connecting` and re-rendered. **`disconnect` set nothing at
all** — so hanging up on a server that was not answering looked exactly like a
button that had stopped working.

A fifth point fix was available and was the wrong shape. What the class had was
a flag each slow path remembered to set, which is a flag some path forgets; and
one of three had forgotten. So:

**The state is `busy`, not `connecting`.** Naming it after one of the things
that can cause it is how the other causes ended up with no indicator.

**One wrapper.** Every slow path runs inside `inFlight`, which sets the flag,
renders, and clears it in `finally` — so the error path is covered by
construction rather than by remembering. Nothing sets the flag any more; the
wrapper does.

**One signal out.** `onBusyChanged` lets the Connect/Disconnect button in the
sidebar head — which is the control a person actually clicks, and which lives
outside this class — spin and disable without each path telling it.

**The confirmation stays outside the busy state.** `canDrop` opens a dialog and
waits for a person; marking that as work in flight would spin the rail for as
long as they take to read it.

### 6.1 A bug the canvas found, which no test would have

The traced outline strokes `--accent`. That was right while it only ever
appeared on a **connecting** item — still `.offline`, so transparent with a
dashed border, and the stroke read against the rail behind it. Disconnecting
draws the same outline on a **live** item, filled with the connection's own
colour: on a blue connection that is accent on accent, which is to say nothing.

It strokes `--on-accent` on a live item now — the colour already chosen to be
legible on that fill, whatever the connection is.

**The first test for it was worthless and passed with the fix reverted.** It
asserted the stroke differed from the fill, and the fixture connection is red,
so an accent-blue stroke differs from it with or without the rule. It asserts
the stroke *is* the item's own foreground now, which is the rule, and the
falsification bites.

## 7. One language for the rail

Four marks, four corners, three shapes, no shared rule — each added on its own,
none of them aware of the others:

| mark | corner | kind |
|---|---|---|
| migrations pending | top-right | a count |
| read-only | top-left | a property |
| not saved | bottom-right | a property |
| assistant not ready | bottom-right, other control | a warning |

The two warm dots were the same 8px circle in the same colour meaning two
unrelated things.

**The fix is not a corner, it is a vocabulary.** What can a rail button say?

* **A property of the thing** — read-only, not saved. True whether or not
  anything is happening, and asked *before* doing something. **Top-left.**
* **A count** — a number worth seeing without opening the panel. **Top-right**,
  where it has room to be two digits wide. Only one count exists per button, so
  it never collides.
* **Activity** — transient, and belonging to the *whole control* rather than to
  a corner of it. **No corner:** the button traces its own outline. This was
  the most obviously wrong one before — the assistant drew an 8px corner
  spinner while a rail connection drew a traced outline, for the same fact.

A new mark asks which kind it is, and the place follows.

### 7.1 Both at once

A connection can be read-only **and** unsaved, which is why those two sat in
opposite corners; the stylesheet said so. Two answers were drawn on the canvas
and the choice was the user's:

> *"Let indeed go with the stacked option for the property"*

So they stack along the top edge: 13 + 2 + 9 = **24px on a 32px button**. Two
fit and a third would not, and that limit is the point — it is where this
decision gets reopened, rather than being settled now by a priority nobody
wrote down.

The alternative was a priority order with the loser in the tooltip. It never
runs out of room and is simpler to hold in your head, but it hides a true
thing, and *"this connection will vanish when you close it"* is exactly the
sort of fact wanted before disconnecting — which is the reasoning the read-only
mark was given in the first place.

The two are deliberately **different shapes**: a glyph in a disc, and a bare
dot. Side by side they do not read as a pair of anything.

### 7.2 What was removed

**The assistant's warning dot, outright.** It meant "a CLI was found but not
verified" — a fact about a tool's installation rather than about anything you
are doing, permanent until somebody fixes their PATH, carrying no action, and
sitting in the corner the connection marks now use. It trained you to ignore
the one corner that was about to start mattering. The title says what is wrong
and the panel says it again with the command to run.

**The `not-ready` class went with it**, rather than being left to paint
nothing. Three tests asserted that class; they assert the title and the panel's
sentence now, which is what a person actually reads. A class that is the only
evidence of a state is the same defect as §2's refusal written into an option's
label.


## 8. Still open

**Native select popups are untestable here.** Playwright sets a `<select>`
through the DOM and never opens the platform popup, so nothing in this suite
can reach the behaviour that was reported — including the disabled-option
rendering that hid this bug for as long as it did. Worth remembering the next
time a control "does not work" and every test is green.
