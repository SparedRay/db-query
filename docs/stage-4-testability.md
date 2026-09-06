# Stage 4 — Testability

> ## 🔒 FROZEN — do not modify
>
> This tracker covers **Stage 4**, whose purpose is met: the UI can be driven,
> and **192 UI tests run on both engines** (Chromium and WebKit) alongside 158
> unit + 3 fixture + 57 live + 5 keychain tests. Every Stage 3 milestone
> (E1-E8) is answered by test rather than by hand, and the frozen trackers' bug
> list is a regression suite that fails against the old behaviour.
>
> **Two milestones are carried to [Stage 5](stage-5-packaging.md), not
> abandoned:**
>
> - **T7 — CI.** There is no git remote on this repo, so no CI can run. The
>   workflow that would satisfy T7 is the same GitHub Actions matrix Stage 5
>   needs to build installers; writing it twice would be waste. It is now
>   Stage 5 Phase 2.
> - **T8 — E0/C3w (Windows).** Third stage carrying it, and for the same
>   reason: it needs a Windows machine or a Windows runner. Stage 5's matrix is
>   where it stops being carried.
>
> §6b (*What still needs a human*) is the standing manual list. It is not
> Stage 4 debt — those items are unreachable by automation by nature, and are
> re-stated in Stage 5 where the platforms they need actually exist.
>
> Corrections belong in the current stage's tracker with a link back here.

**Goal:** make every feature checkable by something that runs on demand, so no
stage has to be frozen on "someone should click this".
**Builds on:** [Stage 3 — Schema actions & result export](stage-3-explore-and-export.md) (frozen).
**Status:** 🔒 Frozen 2026-09-06. Phases 1-3 delivered, both engines green,
T7/T8 carried to Stage 5.

---

## 1. Why this stage exists

Stage 3 froze with its entire click-through unperformed. Not from haste — there
was **no way to drive the UI**. No `xdotool` or `ydotool` on the dev machine, a
Wayland session, and a clipboard write needs a trusted user gesture that cannot
be synthesised. So E1-E8 were written, built, and never checked.

That is the acute problem. The chronic one is older and worse:

> Of the bugs this project has shipped, **the majority were frontend and
> invisible to the Rust suite**, and several were found by reading the dev
> server's runtime log rather than by any test.

The roll call, from the frozen trackers:

| Bug | Stage | What would have caught it |
|---|---|---|
| Running tab showing the previous result set | 1 | a UI test |
| Failed connections left dead icons in the rail | 2 | a UI test |
| Context menu items silently unclickable (`mousedown` vs `click`) | 2 | a UI test |
| `window.confirm` intercepted — delete did nothing | 2 | a UI test |
| Tabs created with no connection, from three call sites | 2 | a UI test |
| Remembered passwords invisible after restart (`#[serde(skip)]`) | 2 | a **wire-format** test |
| Copy/Export enabled with nothing to export | 3 | a UI test — **and this is how it was found** |

Six of seven needed a browser. The seventh needed a test on the serialised form,
which is why those now exist in `sqlgen.rs` and `export.rs`.

---

## 2. What was tried, measured 2026-09-05

### The question: can Playwright help?

**Yes, decisively** — and the reason is one line of `@tauri-apps/api`:

```js
async function invoke(cmd, args = {}, options) {
    return window.__TAURI_INTERNALS__.invoke(cmd, args, options);
}
```

The entire Rust boundary is a single object on `window`. Replace it before the
page loads and **the real frontend runs in a real browser** — CodeMirror, the
schema tree, the virtualised grid, the dialogs — with **no changes to `src/`**.

Verified rather than assumed, in this order:

1. **Chromium launches headless here.** It needs no extra system packages.
2. **A real click drives the real clipboard.** A Playwright `click()` is a
   trusted gesture, `navigator.clipboard.writeText` succeeds, and
   `readText()` hands it back for assertion. This is precisely what could not be
   done on the desktop.
   *One catch found the hard way:* `page.setContent` gives an `about:blank`
   origin, which is **not a secure context**, so `writeText` fails there. Serving
   over `http://127.0.0.1` — which is what the app does anyway — fixes it.
3. **The app boots under it.** `app_defaults` and `list_profiles` were called,
   the rail and editor mounted, the export bar rendered.
4. **It found a bug on the first run.** See §4.

### Why not `tauri-driver`

It exists (`tauri-driver` 2.0.6) and is the official route, but:

- it needs `WebKitWebDriver` — an apt package, **not installed here**;
- it has **no macOS support at all**, because WKWebView exposes no WebDriver;
- it boots a native window per test.

That makes it the right tool for a handful of "does the native shell work"
smoke tests and the wrong one for the fifty interaction tests this app needs.
It is on the backlog, not the critical path.

### Licences — checked, per the standing rule

| Tool | Licence |
|---|---|
| `@playwright/test` 1.63.0 | Apache-2.0 |
| `@types/node` | MIT |
| (considered) `vitest` | MIT |
| (considered) `webdriverio` | MIT |

All dev dependencies; none ships.

---

## 3. The shape

Three layers, each answering what the ones below it cannot.

| Layer | Runs | Answers |
|---|---|---|
| **Rust unit + live** (exists, 223 tests) | `mise run test` / `test-live` | SQL generation, escaping, protocol, file I/O |
| **Wire-format tests** (exists, in `sqlgen.rs`/`export.rs`) | `mise run test` | that Rust and `api.ts` agree on the JSON |
| **UI tests** (new) | `mise run test-ui` | DOM, events, ordering — everything above the IPC line |

**Where the seam falls is a design fact, not a gap.** `clipboard_text` builds its
text in Rust, so a browser test can never exercise the real formatting. That is
correct layering: `export.rs` owns the format and unit-tests it, and the UI tests
own the question Rust cannot answer — *did the right rows and columns get that
far, and did the clipboard actually receive the answer?* The harness's `tsv()`
stub mirrors the shape faithfully enough for that and deliberately no further.

### Running it

```
mise run test-ui              # chromium, no system packages needed
npm run test:ui:webkit        # closer to what ships; needs two apt packages
```

**WebKit is the one that matters** — Tauri uses WebKitGTK on Linux and WKWebView
on macOS. It is opt-in only because it needs `libevent-2.1-7t64` and `libavif16`,
which need root. Chromium is the default so the suite runs anywhere, including
an agent's sandbox with no sudo.

---

## 4. Bug found by the first UI test ever run here

**Copy and Export were enabled at boot with nothing to export.** Clicking either
did nothing: `selectedData()` returns `null` and the handler returns early. A
silent no-op — the same failure mode as the Stage 2 delete button.

The cause is worth keeping. `refreshExportBar` was only reachable from
`showResults`, which needs a tab; at boot there is no connection, so no tab, so
it never ran and the buttons kept their markup-default enabled state. The
per-call `onSelectionChanged` hook could not cover it either, because
`setMessage` *resets those hooks* — so the one path that turns exportable data
into no exportable data was the one path that reported nothing.

Fixed with a persistent `onChanged` on `ResultView`, fired by both `show()` and
`setMessage()`.

---

### Phase 2-3 — what shipped (2026-09-05)

**88 UI tests, 15 seconds**, across six files. Every one of Stage 3's E1-E8
milestones is now answered by something a machine runs, and the frozen trackers'
bug list is a regression suite.

| File | Tests | Covers |
|---|---|---|
| `smoke.spec.ts` | 3 | boot, editor mounts, no tab without a connection |
| `clipboard.spec.ts` | 6 | E7 — headers, no headers, row subset, range, block, NULL |
| `tree.spec.ts` | 17 | E1-E4 — browse, every context menu, routine groups, Examine |
| `export.spec.ts` | 12 | E5, E6, E8 — options, source table, truncation, cancel, refusal |
| `execution.spec.ts` | 17 | run/run-all/cancel, status chips, result tabs, grid rendering |
| `regressions.spec.ts` | 18 | the shipped-bug list, per-connection workspaces, selection |
| `tabs-files.spec.ts` | 16 | tabs, dirty state, open/save/conflict, connection dialog rules |

**Every tree test asserts `run_script` was never called.** The rule the whole of
Stage 3 was built on is checked after each action rather than once, because half
of what those menus produce is `DROP`.

#### Two real bugs found

**1. `hidden` did not hide.** `.toggle { display: flex }` is an author rule, and
an author `display` beats the UA sheet's `[hidden] { display: none }` regardless
of specificity. So `els.connRememberRow.hidden = true` did nothing: the
*"Remember password in the system keychain"* row stayed visible after its
*"Save this connection"* checkbox was unticked — letting a password be marked
for the keychain against a connection that was never going to be saved.

Fixed with a global `[hidden] { display: none !important; }`, which is the rule
every design system carries and this one was missing.

**2. A malformed lint response threw an uncaught page error.** `lintSource`
wraps the call in `try/catch` under a comment saying *"a linter failure must
never surface as an error the user has to dismiss"* — but the `catch` only
covers a rejection. A response that *resolved* to anything but a list reached
`.map` and threw, straight past the guard. Now checked with `Array.isArray`.

Neither was reachable from the Rust suite. Neither is exotic.

#### Five test bugs, and what they were worth

Roughly a third of the tests failed first for reasons in the *test*, not the
app. Worth recording, because each one was a specific misunderstanding that a
green-first-time suite would have hidden:

- **`.trim()` ate the empty field** that the NULL test existed to assert.
- **Stub shapes were guessed.** `open_file_dialog` returns `OpenedFile | null`,
  not an array; `[]` is truthy, so "cancel" silently opened a garbage tab. The
  real shapes are in `api.ts` and are not optional reading.
- **Ambiguous selectors.** `#status, .empty` matched two elements; `th.sel`
  matched row numbers as well as column headers.
- **`toBeDisabled()` does not report a disabled `<option>`** — the attribute has
  to be asserted directly.
- **Two assertions could not fail.** A single-column result cannot distinguish a
  selection from no selection, because selecting the only column *is* selecting
  everything.

Two "failures" were the app being right and the test being wrong: the delete
prompt mentions the keychain **only when a password is actually stored**, and
`connectionId` in an export source is the browser-minted profile id, not the
server's.

#### The harness earned one change

`page.exposeFunction` can only register a name once, so a test that swapped a
command mid-run failed on the *second* `installBackend` rather than on anything
real. Handlers now live in a `WeakMap` behind a single exposed function.

### WebKit, and the bug it uncovered (2026-09-05)

**89 tests, both engines.** WebKit is what Tauri actually uses on Linux and
macOS, so a green Chromium run was never proof of anything shipped.

#### Every WebKit test failed for one reason, and it was mine

`browserContext.newPage: Unknown permission: clipboard-write`. The clipboard
permissions were set **globally** in the Playwright config, and WebKit rejects
them outright — so all 88 tests failed before a line of app code ran. Scoped to
the Chromium project, 81 passed immediately.

The remaining 7 were the clipboard *readback* tests: only Chromium grants
`clipboard-read`, and `navigator.clipboard.readText()` raises `NotAllowedError`
in WebKit regardless.

**So the round trip closes in Chromium, and WebKit asserts the app's own report
instead** — which is the more valuable half. The question that matters is not
"can the harness read it back" but *"does a copy succeed in the engine that
ships"*, and the answer is now checked on every run.

**It does, and natively.** Neither engine falls back: the note says plainly
`Copied all 1 row(s) × 1 column(s) with headers.` with no *"via the legacy
clipboard path"* suffix, so `navigator.clipboard.writeText` works in WebKitGTK.
The `execCommand` fallback in `src/clipboard.ts` has still never been needed —
and is still worth keeping, because that is now a measurement rather than a
hope.

#### Copying deleted the results it had just copied

The worst bug this stage has found, and it was hiding behind six passing tests.

Copy reported success through `results.setMessage`, which **replaces the grid**.
So a copy wiped the table off the screen, disabled the Copy button (there was
nothing left to export), and made copying twice mean re-running the query. Every
clipboard test passed throughout: they asserted what reached the clipboard and
never once looked at what was left behind.

Fixed with `showNote()` and a dedicated `#result-note` element: feedback about a
result must not destroy the result. `setMessage` keeps its job — "no results
yet", an error — where replacing the grid is the correct behaviour.

**The lesson is about the tests, not the code.** Six tests covered this feature
and all six were green while it was broken, because each asserted an *effect*
and none asserted that nothing else changed. The regression test now checks the
table is still there, the count is unchanged, the button still works, and a
second copy succeeds.

### Reported from the app: result tabs only switched one way (2026-09-06)

*A Stage 3 defect. That tracker is frozen, so the correction is recorded here.*

> "When we have multiple resultsets, we can change to the first resultset but
> then we cannot change back."

Exactly right, and the asymmetry is the whole clue: the trip that fails is the
one **into a smaller result**.

`ResultView` keeps `this.cols` / `this.rows` as a render cache, filled by
`buildTable`. The result-tab click handler cleared the selection and then ran,
in this order:

```
onSelect(i) → onSelectionChanged() → renderTabs() → renderActive()
```

`onSelectionChanged` reaches `refreshExportBar` → `selectedData()`, and with an
empty selection `selectedColumns()` / `selectedRows()` mean *everything* —
sized from the render cache, which at that moment still described **the
statement being left**. Switching to a shorter result indexed `o.rows[r][c]`
past the end, threw `Cannot read properties of undefined`, and the exception
aborted the handler *before* `renderTabs`/`renderActive` ever ran. So the click
did nothing at all, silently: no error surface, no repaint, a dead tab.

Going the other way — into a *larger* result — the stale indices are in range,
so that direction worked. Hence "we can change to the first but not back".

Fixed in two places, either of which would close it:

- The accessors now read the **active statement's outcome** (`activeGrid()`),
  not the render cache, and clamp out-of-range indices. `isEverything()` and
  `selectAll()` use the same source.
- The handler renders before it notifies, so no listener ever observes a grid
  belonging to the previous statement.

**Why 94 passing UI tests missed it.** There *was* a test for switching result
tabs — it clicked the first tab and asserted the selection cleared. It never
clicked back, and both its statements had one row. The bug needs three things
at once: two result sets, a size difference, and a **return** trip. Fixtures
built for convenience are usually uniform, and uniform fixtures cannot express
a difference. The two new tests use deliberately uneven statements (40 rows ×
2 columns against 1 × 1), bounce 1→2→1→2, and assert no page error was raised
at any point.

## 5. Milestones

- [x] **T1 — UI tests run at all**, headless, with no system packages and no sudo.
- [x] **T2 — E7 answered.** Copy with and without headers, selected rows, a shift-click range, a row×column block, and NULL versus the string "NULL" — all verified through a real clipboard.
- [x] **T3 — E1-E4 answered.** Double-click browses into a new unrun tab; every context menu item generates the right SQL and executes nothing; routines appear grouped; Examine produces its DROP+DELIMITER script.
- [x] **T4 — E5, E6, E8 answered.** The export dialog's scope and warnings, and that a truncated result never exports silently.
- [x] **T5 — The frozen-tracker bug list is a regression suite.** Every bug in the §1 table has a test that fails against the old behaviour.
- [x] **T6 — WebKit passes too**, not just Chromium.
- [ ] **T7 — It runs in CI**, on every push, with traces kept on failure. → **carried to Stage 5 Phase 2**: there is no git remote, and the release matrix is the same workflow.
- [ ] **T8 — E0/C3w.** Still unanswered, now on its third stage. → **carried to Stage 5 Phase 2**, where a Windows runner exists.

---

## 6. Task tracker

### Phase 1 — Make UI tests possible → *T1, T2*
- [x] `@playwright/test`; licences recorded
- [x] `tests/ui/harness.ts` — the `__TAURI_INTERNALS__` stub, a declarative fake backend, and call recording
- [x] `playwright.config.ts`; chromium default, webkit opt-in, vite as `webServer`
- [x] `mise run test-ui`; `tsc` covers `tests/`
- [x] Smoke tests: boot, editor mounts, no tab without a connection
- [x] Clipboard tests: headers, no headers, row subset, shift range, cell block, NULL

### Phase 2 — Close the Stage 3 click-through → *T3, T4*
- [x] E1: double-click a table opens a new tab holding `SELECT … LIMIT n`, unrun — assert `run_script` was **never** called
- [x] E2: every context-menu item on table/view/column/schema/routine generates the right SQL and runs nothing
- [x] E3: routines grouped separately; a function shows its return type
- [x] E4: Examine produces `USE` + `DROP … IF EXISTS` + `DELIMITER` in that order
- [x] E5/E6: the export dialog's format and scope reach the backend as the right arguments
- [x] E8: a truncated result warns, and pre-selects the unbounded re-run

### Phase 3 — Regression suite from the frozen trackers → *T5*
- [x] Running tab must not show the previous result set
- [x] A failed connection leaves no rail icon and puts its error in the dialog
- [x] Context menu items are clickable; Escape closes; it stays on screen
- [x] Delete asks, and actually deletes
- [x] Tab-per-connection: switching restores tabs, results and cursor
- [x] Selection survives a tab switch and clears on a statement switch

### Phase 4 — Infrastructure → *T6, T7*
- [x] WebKit green — needs `libevent-2.1-7t64` and `libavif16`; `mise run test-ui-webkit`
- [ ] GitHub Actions: `mise run check`, `test`, `test-ui` on every push
- [ ] Traces and the HTML report as artefacts on failure
- [ ] Decide whether `tauri-driver` smoke tests earn their keep (Linux + Windows only)

### Phase 5 — Verification
- [ ] T1-T7 demonstrated
- [ ] Stages 0-3 suites still pass by name

---

## 6b. What still needs a human — the complete list

Derived from what the automated layers **structurally cannot reach**, not from
what has not been got round to. 411 tests now pass (158 unit + 3 fixture + 57
live + 5 keychain + 188 UI across two engines); everything below is outside all
of them, and the reason is given in each case.

### A. The Tauri shell — the UI tests stub it away

The browser suite replaces `window.__TAURI_INTERNALS__`, so **every native
interaction is faked by construction**. These have never run:

- [ ] **A1 — Open a real `.sql` file.** Ctrl+O opens the OS picker; the filter
      lists SQL; the file loads with its content, encoding and line endings
      intact. `files.rs` is well tested; the *dialog plumbing* is not.
- [ ] **A2 — Save, and Save As.** Both write real bytes to a real path. Check a
      CRLF file stays CRLF and a Latin-1 file refuses to save rather than
      writing U+FFFD over the original.
- [ ] **A3 — Drag a `.sql` file onto the window.** This is the one piece of
      Tauri-specific code with **no test at any layer**: Tauri intercepts
      drag-and-drop at the webview level so HTML5 drop events never fire, and
      `onDragDropEvent` cannot exist in a browser. Drop one file, then several.
- [ ] **A4 — Close the window with unsaved changes**, and with a query running.
- [ ] **A5 — Copy, in the real app.** Playwright's WebKit is **not** WebKitGTK —
      it is a separate build with no GTK. The clipboard works in both engines we
      test, which is strong evidence and not proof. Copy, then paste into a text
      editor *and* a spreadsheet.

### B. The OS keychain — nothing automated touches the app's own flow

`tests/keychain.rs` exercises the Rust functions against the real Secret
Service. What it does not exercise is the app using them.

- [ ] **B1 — C3, end to end.** Save a connection with *Remember password*, quit,
      relaunch: it is in the rail, and **one click connects with no prompt**.
      This is the exact path the Stage 2 bug broke, and the launch-to-launch part
      is what no test can span.
- [ ] **B2 — Revocation.** Delete that connection, then confirm the entry is gone
      from the OS credential store (`seahorse` on GNOME, Keychain Access on
      macOS, Credential Manager on Windows).
- [ ] **B3 — No credential store.** On a machine without one, saving still
      succeeds, the warning explains, and connecting prompts.

### C. Generated output meeting real software

We verify our own reading of what we produce. What we cannot verify is what
*other* programs do with it.

- [ ] **C1 — E5: open an exported CSV in a spreadsheet.** Excel or LibreOffice.
      The BOM makes accents render; CRLF keeps rows intact; a field beginning
      `=` is *not* executed when the formula guard is on, and the guard is
      visibly absent when it is off. Our RFC 4180 tests use an independently
      written reader — a spreadsheet is a different question.
- [ ] **C2 — E6: replay an exported script with the `mysql` CLI**, not through
      our own executor. The round trip is proven against our code path; a `.sql`
      file is usually run by something else.
- [ ] **C3 — E2: actually run one generated `DROP`** against the fixture and
      confirm it drops exactly what it named. The tests prove we never run one;
      nothing proves the SQL is right when a human does.

### D. Platforms we have never built on

- [ ] **D1 — E0/C3w: Windows.** `cargo test`, the keychain round trip, and the
      entry appearing in Credential Manager. **Third stage carrying this.**
- [ ] **D2 — macOS.** Never built, never run. WKWebView, Keychain Services and
      the file dialogs are all different code paths.

Both are Stage 5's CI matrix, which is where they stop being carried by hand.

### E. Judgement — no assertion can settle these

- [ ] **E1 — Does the 220 ms double-click deferral feel laggy?** It is
      imperceptible in theory because expanding a table is a round trip anyway.
      Theory is not the test.
- [ ] **E2 — Is the selection legible?** Selected columns, selected rows, and
      their intersection, against the dark theme.
- [ ] **E3 — Do the tree groups read clearly** at a realistic size — a schema
      with a hundred tables, not four.
- [ ] **E4 — Is the export dialog understandable** without having written it?
      Particularly the NULL policy and the formula guard, both of which describe
      trade-offs rather than settings.

### Not on this list, deliberately

**E1-E4, E7, E8 and the whole Stage 2 regression set are done** — by the UI
suite, in both engines. They needed a human when Stage 3 froze; they do not now.

### A cleanup this audit turned up

`split_sql`, `disconnect_all` and `has_stored_password` are registered in Rust
and wrapped in `api.ts` but **called from nowhere**. Dead surface: either the
frontend should use them or they should go. Not a verification item — a tidying
one, and cheap before packaging freezes an API.

## 7. Risks

- **A stubbed backend can drift from the real one.** The wire-format tests in
  `sqlgen.rs` and `export.rs` are the guard, and they only work if new commands
  get one. Any command the UI calls needs its JSON pinned on the Rust side.
- **Chromium is not WebKitGTK.** A green Chromium run is not proof the shipped
  app works; that is what T6 is for, and why the config carries a webkit project
  from day one rather than adding it later.
- **UI tests can encode the bug they were written against.** Two of the nine
  written so far failed first for reasons in the *test* — an unstubbed command
  and a `.trim()` that ate the empty field being asserted. Worth watching that a
  test fails for the right reason before trusting it.
