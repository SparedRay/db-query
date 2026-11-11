# Stage 32 — the colour of a connection, and the page when there is none

**Status:** ✅ Built — 2026-11-06. Both halves reported from live use; hands-on
confirmation outstanding.

---

## 1. The ask

> *"When we disconnect color on open tab remain from last connection which
> cause a slight confusion. Can't we have a default no tab page when theres no
> connection yet? Also result set and other details keep on blue, can't we have
> them same accent color as current connection?"*

Three things, and the first two are the same bug seen from opposite ends: the
accent on the workspace said something about a connection that was not true,
and the window had nothing to show when there was no connection at all.

## 2. What was actually true

**The colour came from the profile, which cannot answer the question.**
`TabHooks.colourFor` read `conns.get(id)?.profile.colour`. A profile records
what colour a connection *is* — chosen in the editor, saved to disk, there
whether or not anything is open. Nothing on that path knows whether we are
talking to the server, so a disconnect left the last server's colour painted
across a workspace with no server behind it. The tab strip went on saying
"prod-eu, red" about a connection we had hung up on.

**It reached exactly one element.** `tabs.ts:render()` set `--conn-colour` on
the strip itself, so the only things that could read it were the three rules
under `.stab`. Everything else that marks a selection in the workspace — the
result tabs, the Results/Sequence switch, the log's All/Changes/Errors, the
performance panel's two lists — was on `var(--accent)` and stayed blue on every
connection. That is the second half of the report, and it was not a separate
defect: a hook that is handed one element can only ever paint one element.

**The rule already existed, in the rail.** `.rail-item.offline` has drained an
icon to a dashed outline since Stage 7, with the comment *"not connected reads
as drained rather than absent — it is still yours"*. And
`connections.spec.ts` already carries *"the selection bar is for a live
connection"*, from a report in the same words as this one: *"a server you had
just disconnected from still looked like the one you were on."* The rail had
learned this twice. The rest of the window had not learned it once.

**Nothing was shown when there was nothing open.** At launch with no saved
connection there is no active connection, so no tab, so an empty strip — and
below it an editor nobody could usefully type into. Typing there was worse than
useless: the buffer belongs to no tab, so it is never written to the session,
and the first connection replaces the document wholesale. Text typed into the
launch screen was guaranteed to be lost.

## 3. The rule: colour means live, and marks identity

Two decisions, written down because both have an obvious wrong answer.

**Colour is a claim that we are connected.** Not "which connection is
selected" — selected and connected are different states and the window has
words for the first (`prod-eu — not connected`) and only colour for the second.
So the token is present while a connection is live and absent otherwise, and
absent means the app's own accent.

**It marks identity, not emphasis.** Everything in the app that is currently
blue falls into one of three groups, and only one of them should move:

| | takes the connection's colour | why |
|---|---|---|
| Identity — which of several am I looking at | ✅ | the active script tab's edge, the active result tab's edge, a pressed segmented button, the performance panel's selected list |
| Semantics — what kind of thing is this | ❌ | a verdict (`.wl-verdict`), a run in flight, the sequence dots, whose four shapes *and lightnesses* are a fixed vocabulary — a slate connection would make "data" and "ok" the same grey |
| Chrome — the app itself | ❌ | dialogs, primary buttons, focus rings, splitters, the rail's own pane toggles |

Provenance is the interesting case and it is in the second group.
`.stab.external` marks a tab that arrived through the MCP server rather than
because somebody opened it. Giving it the connection's colour would make it
read as *this is your connection* — the one thing it exists to deny. It keeps
the app's accent, and a test says so.

## 4. The mechanism

One custom property, declared in CSS and overridden in script:

```css
:root { --conn-colour: var(--accent); }
```

```ts
function paintConnectionAccent() {
  const active = conns?.active();
  if (active?.connected) els.app.style.setProperty("--conn-colour", active.profile.colour);
  else els.app.style.removeProperty("--conn-colour");
}
```

Three things follow from writing it this way.

**The fallback lives in one place.** Removing the property rather than setting
it to `var(--accent)` means the default is `:root`'s, which also applies before
the first paint and in any test that mounts a fragment of the app.

**The painter has one call site.** It hangs off `syncConnLabel()`, which every
path that changes which connection is on screen, or whether it is up, already
ends in — connect, disconnect, activate, remove. Adding a call to each of them
is how one of them gets missed.

**`TabHooks.colourFor` is deleted**, along with the `setProperty` in
`tabs.ts:render()`. The strip inherits the token like everything else.

## 5. The page when nothing is connected

**The first cut keyed it off the tab strip, and that was wrong.** `#blank-page`
showed on `#editor-pane:has(.script-tabs:empty)` — "no tab on screen", which is
true at launch and only at launch. Disconnecting leaves the tabs in place, so
the strip was not empty and the window went on showing an editor for a server
that had gone. Reported in one line:

> *"Almost got it, but new screen only shows on first opening. If we disconect
> (Leaving no active connection we should show that conenction screen as
> well."*

**The condition is the connection**: `data-offline` on `#app`, from
`conns.active()?.connected`, set by the same painter that drains
`--conn-colour`. The two were briefly computed in different places from
different facts, and they promptly disagreed — one function now, because
"is there a live connection behind this workspace" is one question and
answering it twice is what let it be answered differently.

**The attribute is in the HTML**, and script only ever takes it away: nothing
is connected at launch, so the first paint is already the page. Same reason the
docks are shut by an attribute rather than by a line of `main.ts` (Stage 31
§6.9).

**The whole editor pane goes, tab strip included.** A row of tabs above a page
saying nothing is open is the incoherence the page exists to remove, and a tab
you can click but cannot open is worse than one you cannot see.

**The page's text is written from script**, because the two states want
different words. Nothing connected yet is an invitation. A connection you have
just left is a thing to pick back up — so the heading names it, the body says
the tabs are kept, and the button is `Connect to prod-eu`, doing exactly what
clicking the rail icon does rather than opening the new-connection dialog
nobody asked for.

**The editor is hidden, not removed.** `#editor` and `#blank-page` share one
cell of a one-cell grid, so the editor keeps a real box of the right size at
all times and is taken off screen with `visibility`. CodeMirror measures itself
against its host: an editor built inside a `display: none` host comes back with
the wrong line heights until something provokes a re-measure, and the smoke
test now asserts the box rather than the visibility it used to assert.

**The toolbar goes with it**, and that was found in a screenshot rather than by
a test. Run and Run all were correctly disabled, which is precisely what made
Format sitting there enabled — above a page saying there is nothing open —
easy to miss. Every control in that row acts on a buffer.

## 6. What was deliberately not done

**~~Disconnecting does not take the tabs away.~~** This section first argued
the opposite of §5, with a test named for it, on the grounds that blanking the
workspace on a disconnect would throw away SQL somebody wrote. The report
quoted in §5 settled it, and the premise is what was wrong: **hiding the editor
pane discards nothing.** The tabs stay in memory and in the session file, and
`session.restoreInto` returns early when `tabs.forConnection(id).length > 0` —
*"reconnecting inside one session — the live tabs are the truth"* — so a
reconnect finds them exactly as they were. The test is inverted rather than
deleted, and its second half now proves the round trip: same tab, same text.

What is actually given up is **editing SQL while disconnected**, which the
first version did allow. Nobody has asked for it; it is in §8 so that the next
report about it is met with a decision rather than a surprise.

**The schema tree's icons keep the app's accent.** A database icon is a
*kind*, like the sequence dots.

**The status dot did change**, by the rule in §3: filled while live, a ring
when not. It sits beside the words "not connected", so it was never ambiguous —
but a colour that is right only because of the text beside it is one layout
change away from being wrong.

## 7. What the tests pin, and what they survived

`tests/ui/connection-accent.spec.ts`, ten tests. Every colour is asserted
against `--accent` as `#app` resolves it rather than against a literal: the
first swatch in the palette **is** the accent blue, so a test that hard-codes
`rgb(59, 130, 246)` passes whether the mechanism works or not. For the same
reason the fixture connects a saved red profile by clicking the rail, instead
of using the harness's `connect()`, which takes the default swatch.

Six falsifications, each reverting one decision, each failing the test that
claims it:

| broken | fails |
|---|---|
| colour from the profile regardless of `connected` | *disconnecting drains the colour back to the app's accent* |
| result tabs and switches back on `--accent` | *the result tabs and the switches wear the same colour as the tab strip* |
| the blank page never shows | the three page tests |
| `data-offline` from "nothing selected" rather than "nothing live" | *disconnecting shows the connection screen* and *the screen offers to reconnect* |
| the tab strip left on screen over the page | the launch test and the disconnect test |
| no `.conn-dot.offline` rule | *the status dot is a ring, not a disc* |

The fourth reinstates the exact bug that was reported against the first cut,
and it passes every other test in the file — including the one that checks the
page appears at launch, which is what made the first version look finished.

One existing test changed its claim rather than its assertion.
`migrations.spec.ts`'s *"shuts on Escape and on a click elsewhere"* clicked the
editor to click away — and that panel needs no connection, because it reads a
repository rather than a server, so with nothing connected the editor pane is
now §5's page. The click-away target is the page. The panel being usable with
no connection is correct and was not noticed until this change made it visible.

## 8. Still open

**~~No editing while disconnected.~~ Settled, 2026-11-06.** The connection
screen covers the editor pane, so a buffer cannot be read or written until
something is connected. Raised as a cost of §5 and answered:

> *"Our app is not a SQL editor but directly for interaction so thats ok."*

Which is the sharper statement of what this app is, and worth keeping for the
next decision of this shape: a buffer here exists to be run against a server.
Offline editing is a text editor's job, and the tabs are intact either way.

**~~White on amber.~~ Closed 2026-11-11.** A pressed segmented button filled
with the connection's colour and kept `--on-accent` white. On `#f59e0b` that
measured **2.15:1** — the weakest pair in the palette. This note said fixing it
properly meant a foreground computed from the swatch's luminance, and that
doing so would flip other chrome to dark text — a visible change nobody had
asked for. Asked for now, and done.

Three things the note had slightly wrong, all found by drawing it rather than
by reading the CSS:

- **It is not only the pressed button.** `.rail-item` carries the same pair, so
  an amber connection's rail disc wore white initials the whole time it was
  connected, not only while something was pressed. That is the more visible
  instance, and it is the one a screenshot shows first.
- **The rail's `.chip` elements were never part of it.** They are coloured text
  on a neutral background — a different pair, already legible. The note's worry
  about "flipping the rail's green and amber chips" was about a cost that did
  not exist.
- **"Whichever wins" is the wrong rule.** Taking the better of white and dark
  flips five of the seven swatches, including blue (3.68 → 3.99) and red
  (3.76 → 3.90), where it buys almost nothing and restyles the two most-used
  connections. The rule is a **threshold**: below 3:1 white has stopped being
  readable rather than merely imperfect. Amber (2.15), green (2.28) and teal
  (2.49) take dark ink; blue, red, violet and slate keep the white they have
  always had.

**The shape of it.** A second token, `--on-conn`, beside `--conn-colour` — not
a change to `--on-accent`, which answers for fills of `--accent` and is always
white on blue. `inkOn()` in `connections.ts` picks between two palette tokens
and is set at the same two places the colour is, `#app` and each rail item, so
a colour can never arrive without its ink. Exactly four rules read the new
token: the rail item, the busy outline's stroke, the pressed segment and its
count. Amber now measures **6.83:1**.

Three tests in `connection-accent.spec.ts`, all measuring the contrast of what
is *painted* rather than comparing a token to a literal — a literal would pass
on either theme and either ink. Both halves falsified: pointing the rail rule
back at `--on-accent` fails the amber test, and moving the threshold to 4.5
fails the red one.
