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

### Phase 2 — Updater
- [ ] `tauri signer generate`; public key into `tauri.conf.json`
- [ ] `TAURI_SIGNING_PRIVATE_KEY` (+ password) as repo secrets, never in the tree
- [ ] `tauri-plugin-updater`; endpoint served from GitHub Releases; `includeUpdaterJson` in the release workflow
- [ ] **Verify the plugin's licence from its own source and add it to the audit** — the standing rule since Stage 0
- [ ] In-app: **ask before updating**, through `dialog.ts`. `window.confirm` is unavailable, and "the user decides when something runs" is this project's rule
- [ ] **The `.deb` must not offer updates it cannot apply** — the Linux updater supports AppImage only, and we ship a `.deb` deliberately (Stage 5 §4.4). Detect and disable, rather than failing at download time
- [ ] Tests: offered, declined, accepted; **and a manifest signed with the wrong key is rejected**

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
