# Stage 21 — The assistant docked, and a window that remembers

**Status:** ✅ Built — 2026-10-13.

---

## 1. The use case, in the user's words

> *"First lets design some alternative UX for the assistant. A modal that
> covers all navigation is not ideal and it creates issues interacting. Better
> to have a proper chat panel probably on right side so we can with one click
> either execute proposed queries or copy to active tab/new tab"*
>
> *"2nd lets check a small issue as on windows at least it does not remember
> last size/place where it was opened."*

Two changes, unrelated to each other, both from using the thing.

---

## 2. What was wrong with the assistant

It was a `<dialog>` opened with `showModal()`. That is one call, and it decides
three things at once:

* **Everything behind it is inert.** The schema tree, the editor and the result
  grid could not be clicked. So the moment a proposed query appears — which is
  exactly when you want to look at the table it reads, or the rows you already
  have — the panel is what stops you.
* **It owns the keyboard.** `main.ts` skipped every global shortcut while a
  dialog was open, so Ctrl+B, Ctrl+T and Ctrl+W did nothing while the chat was
  up. That was right for a modal and wrong for a conversation.
* **Using a suggestion had to close it.** Both actions ended with
  `assistantDialog.close()`, because leaving a modal over the editor you had
  just pasted into would be worse. Closing threw away the conversation that
  produced the statement.

And the actions themselves were *Insert at cursor* and *New tab* — both of
which leave you with a buffer and a Run still to press.

## 3. The design

Three layouts were drawn before anything was built: a full-height dock on the
right, a tab inside the results pane, and a floating non-modal panel. The dock
was chosen.

The alternatives are worth recording, because both are cheaper:

* **A tab in the results pane** reuses `showResultsSurface` and costs no
  horizontal width. It is also short, and showing it hides the rows — and the
  rows are half of what you are asking about.
* **Floating and non-modal** is `show()` instead of `showModal()` plus a drag
  handle. It unblocks the app immediately, and then sits on top of the grid,
  needing re-parking at every window size.

### 3.1 A fourth surface

`#app` is a grid of named columns. The schema is columns 1–3, the main area is
4, and column 5 was already spelled `#msplit` in the stylesheet — left over
from Stage 17's migrations pane. The dock takes 5 and 6:

```css
grid-template-columns:
  48px var(--tree-w, …) var(--tree-gap, 7px) minmax(0, 1fr)
  var(--chat-gap, 7px) var(--chat-w, 380px);
```

Closing it is the mirror of `toggleSchema`: the class zeroes the two tracks,
the width the splitter was dragged to stays in `--chat-w-set` so reopening
restores it, and the pane and its splitter are `hidden` as well as zero-wide —
a collapsed pane holding a textarea would still take Tab.

`#app` carries `chat-closed` **in the markup**, not from script. Adding it on
boot would paint one frame with a 380px gap in it.

### 3.2 The splitter measures the other way

The schema's splitter sets its width from `ev.clientX`. The dock grows from the
right edge, so its width is the pointer's distance *from* that edge; measuring
it the same way would make the panel shrink as you dragged it wider. That is
the `x-end` axis, and the comment Stage 17 left behind — "the migrations pane
grows from the right, so its splitter measures the other way" — is now a
branch rather than a note.

### 3.3 The chat does not know it is a panel

`AssistantDeps.dialog: HTMLDialogElement` became `panel: AssistantPanel`, three
methods wide. The old type is how `showModal()` ended up inside the chat's own
logic; opening a docked pane is a class and a `hidden` flag, and the chat has
no business knowing which.

### 3.4 Not remembered across launches

Deliberately, and consistently with the schema pane and both splitters. The
reasoning is already recorded at `toggleSchema`: starting in a state you cannot
remember choosing is a worse first second than reopening something. A 380px
column missing from the window on launch, with no memory of having asked for
it, is exactly that.

## 4. One-click Run

Four actions on a proposed statement: **Run**, **Insert at cursor**, **New
tab**, **Copy**.

**The model still executes nothing.** That is the rule the whole feature is
built on — no tools, no connection, `assistant.rs` — and Run does not touch it:
Run is a person pressing a button. What it replaces is the *second* button they
had to press afterwards.

### 4.1 Where the statement goes

> the active tab if it is spare, otherwise a new one

Spare means a buffer nobody has touched: no file behind it, and not dirty — so
it still holds exactly the starter text `tabs.create` put there, or nothing.
Anything typed, and anything read from a file, gets a new tab instead. The rule
errs towards a spare tab rather than towards overwriting, because one of those
costs a tab and the other costs somebody's work.

Either way **the SQL is in a buffer before it runs**. That is not a detour:

* the editor shows exactly what ran, beside its rows;
* `replaceRange` isolates the edit, so one Ctrl+Z puts the buffer back;
* `remember_proposal` records it, so history says where it came from;
* the statement strip and the result tabs work as they do for anything typed.

Running straight through without a buffer was offered and rejected for the
first of those: rows with nothing on screen saying what produced them.

### 4.2 Run with no connection

Disabled, with the reason in its title, rather than absent. A disabled control
that explains itself is how somebody finds out what the thing they wanted
needs; a missing one is a feature they never knew existed.

### 4.3 Copy says what happened

`clipboard.ts` exists because a Copy that quietly does nothing is this
project's worst bug class — `navigator.clipboard` needs a secure context and a
gesture, and WebKitGTK is the weakest of the three platforms at it. So it
throws, and the button shows the outcome on itself for two seconds: *Copied*,
or *Copy failed* with the error in its title.

Not through the result note, which belongs to a result set and would paint over
rows somebody is reading, and not through the chat's note, which carries the
statement about what is sent to the model.

### 4.4 And everywhere else a Copy was silent

Flagging the history dialog's Copy as having the same shape turned up the real
number. `void copyText(…)` — the promise dropped, the throw swallowed —
appeared at **four** call sites:

| | reported before | reports now |
|---|---|---|
| history dialog, context menu | nothing | the dialog's standing note |
| MCP · Copy token | nothing | the button |
| MCP · Copy command | nothing | the button |
| MCP · Copy config | nothing | the button |

The grid's Copy and the value viewer's were already fine, which is what made
the others easy to miss: the feature *looked* covered. The viewer moved onto
the shared mechanism anyway — it reported, but it never took its message back
down, so its note sat there saying "Copied." for as long as the dialog was
open.

Three of the four are the buttons where silence costs most. They are pasted
into another program's configuration, so a failed copy surfaces as that
program not connecting — a symptom arbitrarily far from its cause.

So the reporting moved into `clipboard.ts`, which is the module that already
owns "a copy must not fail quietly":

* **`copyAndReport(text, say)`** — await, hand the outcome to `say`, then hand
  it `null` so it can put things back. The *surface* is the caller's, because
  it genuinely differs: a button can say it on itself, while a context-menu
  item has no element left by the time the copy finishes.
* **`wireCopyButton(button, text)`** — the common case, capturing the label and
  tooltip it was built with so the button goes back to being what it was. A
  generation counter, so a second press takes the button over rather than
  having the first press's restore fire underneath it.

The history dialog reports into its standing note rather than through
`notify`. That is `results.setMessage`, which paints *behind* a modal: nothing
would be visible until the dialog closed, and what it then showed would be the
clipboard sentence sitting where somebody's result set used to be.

Falsified twice, against both surfaces: swallow the rejection and the two
"refuses" tests fail; drop the restore and the two "goes back" tests fail.

## 5. The window, remembered

### 5.1 Not a plugin

`tauri-plugin-window-state` does exactly this. It is not used because
everything it needs is in `tauri` itself — `outer_position`, `inner_size`,
`is_maximized`, `available_monitors`, `set_position`, `set_size`, `maximize`,
and the `Moved`/`Resized`/`CloseRequested` events. Read from the vendored
`tauri-2.11.5/src/window/mod.rs` and `tauri-runtime-2.11.3/src/window.rs` on
2026-10-13. A new crate here would be a new entry in the licence manifest for
arithmetic.

`window.json` joins `connections.json`, `session.json` and `preferences.json`
in the app config directory — the same rule as everything else the app
remembers.

### 5.2 What is stored, and when

The **last non-maximized** geometry, plus whether the window was maximized at
close. Tauri does not expose a maximized window's restored rect, so storing
the rect as-is would give a window that comes back maximized and then
un-maximizes to the size of the whole screen.

The file is written **once, when the window is asked to close**. `Moved` and
`Resized` only update a mutex. A crash or a `kill` therefore loses the last
move — which is the right way round, because the alternative is writing to
disk while somebody drags a window.

### 5.3 The part worth owning

Deciding whether a remembered position is still *reachable*. A window restored
onto a monitor that has since been unplugged is a window nobody can find, and
it is the classic way this feature goes wrong.

`placement()` is pure and has eight tests:

* a position on an unplugged monitor is dropped — the size is still honoured;
* a window hanging off an edge is kept, because dragging one nearly off screen
  is something people do on purpose;
* the 80px threshold, one pixel either side, in both axes;
* negative coordinates — a screen above or to the left of the primary one — are
  normal, not off-screen;
* a window remembered from a 4K screen is cut down to the laptop panel;
* a window in the gap of an L-shaped desktop is unreachable, which is why
  monitors are tested one at a time rather than as a bounding box;
* no monitors reported is not a reason to refuse to place a window;
* a monitor reported with zero size is ignored rather than believed.

Falsified by removing the reachability check: three of them fail.

### 5.4 Measured in the packaged build

The bug was reported on Windows, and nothing here is platform-specific — but
"it builds" is not "it works". So, against a scratch XDG root
(`XDG_CONFIG_HOME` / `XDG_DATA_HOME`), on 2026-10-13:

* a planted `window.json` of 1024×640 produced a **1024×640** window, not the
  configured 1280×800 default;
* closing it with a real `WM_DELETE_WINDOW` rewrote the file with the geometry
  the window actually had.

The position could not be confirmed this way: the window had to be run over
XWayland to be observable at all, and that compositor places windows itself.
The size is the half that proves `restore` ran; the position is covered by the
unit tests and by the write-back being correct.

## 6. What this cost the tests

`showModal()` was doing one more thing nobody had written down: **it was what
made the panel visible in a test**. Three references to `#assistant-dialog`
became `#chat-pane`, which was the easy part.

The real work was the new claims, which are about what is *not* blocked:
driving the tree, the editor and Ctrl+T with the panel open. That test fails if
the old keyboard guard is put back, which is how it was falsified.

Twelve new UI tests, twelve new Rust tests. 460 UI tests on both engines.
