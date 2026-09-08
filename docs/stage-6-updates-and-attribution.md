# Stage 6 — Updates & attribution

**Goal:** finish what shipping started. The app installs; it does not yet update
itself, and it does not yet carry the attribution that distributing it obliges.
**Builds on:** [Stage 5 — Packaging & distribution](stage-5-packaging.md) (frozen).
**Status:** 📋 Planned — **not started**.

---

## 1. Why this stage exists

Stage 5 delivered installers and a green CI matrix on both platforms, and closed
E0/C3w after three stages of carrying it. It froze with **the second half of its
own goal undone** — its goal line says *"and updates itself afterwards"*, and
nothing does.

Two of the items below are not polish:

- **Attribution is an obligation, not a nicety.** §2 of the Stage 5 tracker
  measured the dependency licences and they are clean. That was always a claim
  about a source tree. **Distribution turns it into a duty**, and installers are
  now being distributed: MIT, BSD and ISC all require their notices to travel
  with the binary, and `option-ext` (MPL-2.0) is linked into it.
- **The updater keypair has to exist before the release that will receive an
  update.** It is not retrofittable. A build that ships without a public key
  cannot be told to trust one later — those installs are simply not updatable,
  and every one of them has to be replaced by hand.

---

## 2. Milestones

- [ ] **U1 — The double-click install is confirmed.** Stage 5's P5b: `setup.exe` on a machine with no toolchain, no Administrator prompt, a Start Menu entry, then a connection and a password that survives a relaunch.
- [ ] **U2 — The release is confirmed to produce both installers.** Stage 5's P4, which could only be reported, not checked. Publish one release rather than leaving drafts.
- [ ] **U3 — It updates itself on Windows.** Release `x.y.z+1`; the installed copy offers it, the user accepts, and it comes back running the new version.
- [ ] **U4 — An update signed with the wrong key is refused**, and says so. The half of an updater that matters.
- [ ] **U5 — Attribution ships in the bundle** and is reachable from inside the app.
- [ ] **U6 — The audit covers the Windows tree**, not just Linux.
- [ ] **U7 — Nothing regressed.** Stages 0-5 suites pass on both platforms.

---

## 3. Task tracker

### Phase 1 — Confirm what Stage 5 could only report
- [ ] Publish a release rather than leaving it a draft; confirm `.deb` **and** `-setup.exe` are both attached
- [ ] Install from `setup.exe` on a Windows machine with no toolchain (U1)
- [ ] Remove the stray local `1.0.0` tag — it is not on the remote and means nothing
- [ ] Decide the versioning story: `npm version` currently leaves `Cargo.toml` and `tauri.conf.json` untouched, so three files can disagree about what a release is

### Phase 2 — Updater — built 2026-09-08
- [x] `tauri signer generate`; public key in `tauri.conf.json`. **The private key is at `~/.tauri/db-query-updater.key`, mode 600, outside the repository** — it was never written into the tree and never printed
- [ ] **`TAURI_SIGNING_PRIVATE_KEY` as a repo secret** ← *the one step that is not mine to do*
- [x] `tauri-plugin-updater`; endpoint at the release's `latest.json`; `includeUpdaterJson: true` in the release workflow
- [x] **Plugin licence verified from source**: `tauri-plugin-updater` is `Apache-2.0 OR MIT`. Adding it took the tree from 572 to 595 crates and introduced no copyleft — the only hit, `r-efi`, is `MIT OR Apache-2.0 OR LGPL-2.1-or-later` (disjunctive, so MIT) and `cargo tree -i` shows it reaching **neither** the Linux nor the Windows binary
- [x] In-app: **asks before installing**, through `dialog.ts`
- [x] **The `.deb` does not offer what it cannot apply** — `update::supported()` is false off Windows and the refusal names the alternative
- [x] Tests: 4 Rust, 11 UI on both engines — offered, declined, **dismissed with Escape**, accepted, a failed install, an unsupported build, and a failed check
- [ ] **A wrong-key manifest is rejected** — asserted at the UI layer (a failing install reports why); the real signature check is Tauri's and needs a live release to exercise. U4.

#### Decisions worth recording

- **Checking is automatic; installing never is.** The boot check downloads
  nothing and interrupts nothing — it only reveals a button. An app that greets
  you with a modal before you have opened it teaches you to dismiss modals
  without reading them.
- **A failed check is silent.** Being offline is the ordinary case, not news.
- **`update_install` re-checks rather than holding the handle** from
  `update_check` across the dialog. One extra request, against no cross-command
  state to keep in sync and no chance of applying a stale result — and if a
  newer release appeared while the dialog was open, installing *that* is right.
- **The dialog states the consequence**: the app closes, and unsaved scripts are
  not saved for anyone. Tested, because that is the sentence people skip.
- **`installMode: passive`** — a progress bar, no wizard. The user already said
  yes in our dialog; the installer asking again is the same question twice.
- **A presentation bug this caught:** the messages are built with `\n\n`
  between paragraphs and set as `textContent` (never `innerHTML` — release notes
  come from the server). Without `white-space: pre-line` the breaks collapsed
  and the notes ran into the warning. Found by screenshotting it rather than by
  reading the assertions, which all passed.

#### Two operational facts, neither obvious

1. **The Windows release job will fail until the secret exists.** A Linux build
   succeeds without it — verified — because `.deb` is not an updater target, so
   nothing is signed. NSIS *is* one. Add the secret, then use the workflow's
   **dry run** (Actions → Release → empty tag) to prove signing works on both
   platforms before cutting a tag.
2. **The updater only sees *published* releases.** The endpoint is
   `releases/latest/download/latest.json`, and a draft is not `latest`. The flow
   is draft → review the notes → publish → installs can see it.

### Phase 5 — Quality of life — built 2026-09-08

Asked for after living with the app on Windows: a light theme, and honest
feedback while waiting.

#### Light mode

- [x] **Every colour in `styles.css` is now a token.** The refactor *was* the
  feature: 57 hardcoded literals against 12 variables, and a single literal is
  a colour that cannot follow the theme. The palette is now 29 tokens per
  theme, with the literals confined to the two definition blocks.
- [x] Two roles that were both `#fff` had to be separated. `--on-accent` is
  text on an accent fill and stays white in both themes; `--fg-strong` is
  emphasis, and **inverts** — mapping it to white in light mode would have
  erased the text it was emphasising.
- [x] **`data-theme` is always a resolved value**, "light" or "dark", never
  "system". `theme.ts` resolves the system preference itself and watches for
  changes, so the stylesheet needs one override block instead of a media query
  plus a duplicated palette. Dark lives on bare `:root`, so the app is still
  styled if no script runs.
- [x] **The editor follows the tokens**, not its own colours. The exception is
  CodeMirror's `dark` *facet*, which is not CSS — extensions read it, and lint
  tooltips pick their styling from it — so the theme sits in a compartment and
  the flag is swapped on change.
- [x] Preference cycles system → light → dark, persists in `localStorage`
  (read and written defensively; storage throws outright in some embeddings).
- [x] The toggle lives **beside** the rail, not in it: the rail is rebuilt with
  `replaceChildren` on every connection change, which would delete it.

#### Loaders

> *"If we click on a saved connection it seems like nothing is happening."*

- [x] **Connecting spins on the disc you clicked**, and a second click while one
  is in flight is ignored — otherwise a dead-looking button opens two
  connections to the same server.
- [x] **Expanding a database or a table spins.** The `loading` class was
  *already being set*; it was styled `opacity: .5` on a caret, which nobody can
  see. The state existed and the feedback did not.
- [x] **The database spinner covers the `USE` round trip too.** Clicking a
  database is two requests, and the first was invisible work the user was still
  waiting through.
- [x] One `.spinner`, honouring `prefers-reduced-motion`.

#### A test that would have proved nothing

The first version of the tree tests asserted the `loading` class — which was
already set before the fix, so they passed against the bug. They now assert
what is actually **painted**: the caret's `::after`, its `animationName` and
its width. Both fail against the old stylesheet and pass against the new, and
the `USE`-coverage test was checked the same way against the old `main.ts`.

Asserting the state a feature sets, rather than the effect a user sees, is the
same mistake the clipboard tests made in Stage 4 — six green tests while copying
destroyed the results.

### Phase 6 — Settings, and a mislabelled button — 2026-09-08

- [x] **A settings dialog behind a cog** at the foot of the rail: theme, code
  font and size, session defaults (Auto-LIMIT, Lint, timeout, browse limit),
  and the running version with a manual **Check for updates**.
- [x] **The connect button says Disconnect** when a connection is live.

#### The button was already right; only its label was wrong

`#btn-connect` has disconnected the active connection since Stage 2 — the
handler branches on `active?.connected`. Nothing was broken except the word on
it, which is the worse half: a control that does the opposite of what it says
is more dangerous than one that is missing.

#### Decisions

- **Everything applies immediately.** A settings dialog with an OK button makes
  you guess what a font looks like before you are allowed to see it.
- **Updates sits second**, above the longer "defaults" block, because it is the
  section people open settings to find and the dialog scrolls at 720px.
- **The manual check is the counterpart to the silent boot check.** Being
  offline at launch left no way to ask again; now there is one, and finding an
  update from here closes settings rather than stacking two modals.
- **Font choices are stacks, not families**, each ending in the system default,
  so an uninstalled font degrades instead of disappearing.
- **Out-of-range stored values reset rather than clamp.** These numbers can only
  come from our own controls or from corruption, and a font size of 900 pinned
  to 22 is a size nobody chose. Every field is validated individually —
  `{...DEFAULTS, ...parsed}` would trust whatever is in storage, and one bad
  value could lock someone out of the UI that would fix it.

#### Three bugs the tests found, two of them pre-existing

1. **A closed `<dialog>` stayed painted and swallowed clicks.** `display: flex`
   on the dialog **beats the UA sheet's `dialog:not([open]) { display: none }`**,
   so the settings dialog never really went away. Scoped to `[open]`. The value
   viewer had the same declaration and got the same fix — it was invisible there
   only because that dialog removes itself from the DOM. **This is the third
   time an author `display` rule has silently beaten a UA one in this project**;
   the first cost a whole stage (`[hidden]`, Stage 2).
2. **The editor font size could not be changed**, because `styles.css` already
   carried `#editor .cm-scroller { font-size: 13px }`. The new token rule sat
   directly above it and lost.
3. **My audit missed it** — the grep that was supposed to find every font and
   colour rule ended in `| head`, and the offending line was the eleventh
   result. The colour audit was re-run without truncation: **zero literals
   outside `:root`**.

Fonts are now set in the stylesheet rather than in the CodeMirror theme, where a
plain rule beats the generated theme class and what is written is what is
painted.

### Copy without headers by default — 2026-09-08

Plain copy now carries **no** headers. Headers are the deliberate act:

| Gesture | Result |
|---|---|
| `Ctrl+C`, the **Copy** button, right-click → **Copy** | data only |
| `Ctrl+Shift+C`, **Copy with headers**, right-click → **Copy with headers** | header row first |

Pasting into another query, a spreadsheet column or a chat message is the common
case, and a stray header row there is something you have to notice and delete.
The reverse — needing headers and not getting them — is visible immediately and
one click away.

The copy actions were also added to the cell context menu. They act on the
**selection**, like the toolbar buttons, not on the cell under the pointer —
which is why right-click still leaves the selection alone.

`onCopy` is a **constructor** argument of `ResultView`, not a `show()` option:
`setMessage` resets the per-result hooks, and a copy that silently stopped
working after a message is precisely the bug this project shipped once already.

### Settings showed nothing when run from source — 2026-09-08

Reported after trying the built app. Not reproducible from here: loaded with no
backend stub at all — the page exactly as `tauri dev` serves it — the dialog has
its 3 fieldsets, 9 controls and 6 font options, and the cog's handler is bound.

So the markup and the script are not at fault, which leaves the engine or a
stale process. **The engine is the half worth acting on**: the dialog used
`display: flex` on the `<dialog>` element itself, and *"Playwright's WebKit is
not WebKitGTK"* is a lesson this project has already paid for once.

Both dialogs now cap and scroll their **body** instead, with no layout mode on
the `<dialog>` at all — simpler, and it cannot depend on how one engine treats
flex on a dialog. Kept regardless of whether it turns out to be the cause: the
construct bought nothing that a `max-height` on the body does not.

### Panels, and SET is not a result — 2026-09-08

#### Panels

The rail+sidebar, the editor and the results are now bordered, rounded surfaces
on a recessed canvas (`--bg-app`, new token, one value per theme) rather than
regions of one flat screen — the same move the settings fieldsets make.

- `overflow: hidden` on each panel is what keeps a sticky table header or a long
  tree from painting over a rounded corner.
- **The splitters are now invisible until aimed at.** They used to be drawn in
  `--border`, which put a seam back between surfaces that are now separated by
  space; they keep their hover and drag colours.
- The rail and the sidebar share one panel — one seam between them, not two.

#### `SET` reported "0 rows affected", which is true and useless

> *"Using a SET seems to be returning a resultset, which we could skip — that's
> not really a result is it?"*

Right, and the fix is to stop calling it one rather than to hide it. `SET`,
`USE`, `BEGIN`, `COMMIT`, `ROLLBACK`, `SAVEPOINT`, `RELEASE`, `LOCK`, `UNLOCK`
and `FLUSH` are now a distinct `StatementKind::Session`, and the UI reports
**"Statement executed."** with an `executed` chip.

- **The count still matters for an UPDATE.** An `UPDATE` that matched nothing
  reports zero, because there zero *is* the answer. That distinction is the
  whole reason this is a classification and not a check for `rows == 0`.
- **The statement is still shown, not skipped.** Hiding it would mean a script
  of nothing but `SET` appeared to do nothing at all, and would hide its errors
  and its timing.
- **What changed instead is which tab opens.** `initialIndex` now picks the last
  statement that returned *rows*, falling back to the last statement; a failure
  still wins outright. A script ending in `COMMIT;` used to open on the commit
  and hide the query above it.
- `PREPARE` / `EXECUTE` were deliberately left out: an executed prepared
  `SELECT` does return rows, and misclassifying it would silently discard them.

An existing test pinned `USE` as `Other`. That expectation was the old
behaviour, so it was updated rather than worked around — and a test now asserts
the serialised name `"session"`, because the frontend matches on that string and
renaming it silently would put every `SET` back to "0 rows affected".

### Phase 3 — Attribution
- [ ] `cargo about` config; `THIRD-PARTY-LICENSES` generated **per target** in CI
- [ ] Re-run the audit on the Windows tree — the ~24 `windows-*` crates Stage 5 §2 could not read
- [ ] Name `option-ext` (MPL-2.0) with a link to its source, per that licence
- [ ] npm attribution for the **shipped bundle only** — CodeMirror, `@tauri-apps/api` — not `node_modules`
- [ ] Surface it in the app: an About dialog, or a menu item that opens the file

### Phase 4 — Consistency debts
- [ ] **Export disagrees with the grid about binary.** Stage 5 fixed decoding so a binary-collation `VARCHAR` renders as text; `export.rs` still decides from the column *type*, so the same value is refused by the SQL-INSERT export. Make one of them right and both agree
- [ ] **Dead API surface.** `split_sql`, `disconnect_all` and `has_stored_password` are registered in Rust and wrapped in `api.ts` but called from nowhere. Found in Stage 4; a public release freezes an API, so decide: use them or delete them
- [ ] Stage 4 §6b's remaining hands-on list — **A1-A5 are now reachable**, because there is an installed app on two platforms to click

---

## 4. Risks

- **The keypair is a one-way door.** Every install shipped before the updater
  exists is an install that can never update itself. The longer this waits, the
  more copies have to be replaced by hand.
- **Nobody has watched an update apply.** The failure mode of a broken updater
  is an app that will not start, on someone else's machine, with no console —
  which is why U4 (a wrong key is *refused*) matters as much as U3.
- **`cargo about` needs every licence file to resolve**, and crates with unusual
  or missing `license-file` entries will stall it. Budget for a handful of
  manual clarifications rather than assuming it runs clean first time.
