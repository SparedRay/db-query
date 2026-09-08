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
