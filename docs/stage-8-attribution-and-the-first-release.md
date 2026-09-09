# Stage 8 — Attribution & the first real release

**Goal:** ship the attribution that distributing this obliges, and prove the
release loop end to end — a published release, installed by double-click, that
updates itself to the next one.
**Builds on:** [Stage 6 — Updates & attribution](stage-6-updates-and-attribution.md)
(frozen) and [Stage 7 — Session persistence](stage-7-session-persistence.md) (frozen).
**Status:** 📋 Planned — **not started**.

---

## 1. Why this stage exists

Two of its three parts are **carried, not new**, and one of them is carried for
the second time.

- **Attribution has now been deferred twice.** Stage 5 froze with P7 undone;
  Stage 6 restated it as an obligation in its own §1 and froze with it undone
  again. Nothing about the argument has changed: MIT, BSD and ISC all require
  their notices to travel with the binary, `option-ext` is MPL-2.0 and reaches
  it, and installers are being built for distribution. The audit exists; what
  does not exist is the file that ships.
- **The updater has never run.** It is fully built and fully tested against
  stubs, and on 2026-09-08 the repository was measured rather than reported:
  three tags, **zero published releases**, and a 404 from the endpoint the
  updater reads. Every one of U1-U4 is still a claim.
- **Two small debts** are worth closing while the code is still small enough
  that closing them is cheap.

The one step that unblocks everything else **is not in this repository**:
`TAURI_SIGNING_PRIVATE_KEY` has to exist as a repo secret. The release workflow
now refuses to start without it rather than producing a half-release, so this is
a hard gate, deliberately.

---

## 2. Milestones

- [ ] **R1 — A release is published**, with `.deb` *and* `-setup.exe` *and* `latest.json` attached. (Stage 6's U2, Stage 5's P4.)
- [ ] **R2 — The double-click install is confirmed.** `setup.exe` on a Windows machine with no toolchain: no Administrator prompt, a Start Menu entry, then a connection whose password survives a relaunch. (U1, and Stage 5's P5b before it.)
- [ ] **R3 — It updates itself.** Publish `x.y.z+1`; the installed copy offers it, the user accepts, and it comes back **reporting the new version**. That last clause is the part the version-from-tag fix was for. (U3.)
- [ ] **R4 — An update signed with the wrong key is refused**, and says so. The half of an updater that matters, and today asserted only at the UI layer. (U4.)
- [ ] **A1 — Attribution ships in the bundle** and is reachable from inside the app. (U5.)
- [ ] **A2 — The audit covers the Windows tree**, not just Linux. (U6.)
- [ ] **S9 — The real app is quit with tabs open and reopened**, on both platforms. Stage 7 proved this in a browser; nobody has proved the file lands where `app_config_dir()` says.
- [ ] **C1 — Export and the grid agree about binary values.**
- [ ] **N1 — Nothing regressed.** Stages 0-7 suites pass on both platforms.

---

## 3. Task tracker

### Phase 1 — Unblock the release
- [ ] **`TAURI_SIGNING_PRIVATE_KEY` as a repo secret** ← *the one step that is not mine to do*. The value is the private key generated in Stage 6, at `~/.tauri/db-query-updater.key`
- [ ] **Dry run first** (Actions → Release → empty tag). It proves signing works on both platforms and exercises the tag-stamping path, and spends no tag doing it
- [ ] Tag, then **publish** the draft — the updater endpoint is `releases/latest/download/latest.json`, and a draft is not `latest` (R1)
- [ ] Install from `setup.exe` on a clean Windows machine (R2)
- [ ] Remove the stray local `1.0.0` tag — it is not on the remote and means nothing

### Phase 2 — Prove the updater
- [ ] Publish a second release and watch an install take it (R3). Confirm the version it reports afterwards, which is what Stage 6's version-from-tag fix was for
- [ ] Serve a manifest signed with a different key and confirm the refusal, and that it says why (R4)
- [ ] Confirm S9 on the updated install: **an update must not lose the open tabs**. Stage 7 stores them outside the app directory, so it should not — untested across an actual install

### Phase 3 — Attribution
- [ ] `cargo about` config; `THIRD-PARTY-LICENSES` generated **per target** in CI
- [ ] Re-run the audit on the Windows tree — the ~24 `windows-*` crates Stage 5 §2 could not read (A2)
- [ ] Name `option-ext` (MPL-2.0) with a link to its source, per that licence
- [ ] npm attribution for the **shipped bundle only** — CodeMirror, `@tauri-apps/api` — not `node_modules`
- [ ] Surface it in the app: an About section in settings, or a menu item that opens the file (A1)

### Phase 4 — Debts carried from Stage 6 and 7
- [ ] **Export disagrees with the grid about binary.** Stage 5 fixed decoding so a binary-collation `VARCHAR` renders as text; `export.rs` still decides from the column *type*, so the same value is refused by the SQL-INSERT export. Make one of them right and both agree (C1)
- [ ] **Dead API surface.** `split_sql`, `disconnect_all` and `has_stored_password` are registered in Rust and wrapped in `api.ts` but called from nowhere. Found in Stage 4; a public release freezes an API, so decide: use them or delete them
- [ ] **D4** — tabs stored under a profile that no longer exists (Stage 7)
- [ ] **The "Connected to …" line has never been visible**, because the first tab activation overwrites it. Found in Stage 7 and deliberately left; either give it somewhere durable or stop writing it
- [ ] Stage 4 §6b's remaining hands-on list — A1-A5 are reachable once there is an installed app on two platforms to click

---

## 4. Risks

- **Attribution has been deferred twice.** A third time is not a schedule
  problem, it is a licence-compliance one — and unlike the rest of this stage it
  is blocked on nothing at all.
- **Nobody has watched an update apply.** The failure mode of a broken updater
  is an app that will not start, on someone else's machine, with no console —
  which is why R4 matters as much as R3.
- **The updater keypair is a one-way door.** Every install shipped before the
  updater existed can never update itself. There are no such installs yet only
  because nothing has been published; that stops being true with R1.
- **`cargo about` needs every licence file to resolve**, and crates with unusual
  or missing `license-file` entries will stall it. Budget for a handful of
  manual clarifications rather than assuming it runs clean first time.
