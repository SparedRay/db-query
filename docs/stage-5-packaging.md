# Stage 5 — Packaging & distribution

**Goal:** turn a `cargo run` into something that installs on Windows and Linux
by double-clicking it, and updates itself afterwards.
**Builds on:** [Stage 4 — Testability](stage-4-testability.md).
**Status:** 📋 Planned, scoped and decided (2026-09-06) — **not started**. Renumbered from Stage 4: testability took that slot, because shipping something we cannot test is the wrong order.
**Scope:** Linux + Windows. macOS is out. Built by GitHub Actions from a public repo; auto-update on Windows. See §4.

---

## 1. What this stage is really about

> **This section was written before the scope was decided. §4 supersedes its
> ordering** — dropping macOS removed the expensive item, and signing fell from
> first place to a documented one-time click. Kept because the reasoning is
> still what the stage is about.

Three things, in descending order of how badly they bite:

1. ~~**Signing.**~~ **Now the smallest of the three** (§5). It was first because
   of macOS, where an unsigned app refuses to open at all. With macOS out of
   scope, what is left is one SmartScreen click on Windows.
2. **Building on two operating systems.** Tauri cannot meaningfully
   cross-compile; each target has to be built on its own OS. We know this
   first-hand — Stage 2 tried to cross-check to Windows and found `ring` needs a
   Windows C toolchain, and this machine's `tauri build` offers only
   `deb, rpm, appimage`. **This is now the item everything else waits on**, and
   the answer is a CI matrix (§4.1).
3. **Licence compliance as a shipped artefact.** The audit below is good news,
   but "our licences are fine" is a claim, and distribution turns it into an
   obligation: attribution has to be *in the package*. Open-sourcing the project
   (§4.5) makes this easier to discharge, not optional.

---

## 2. Licence audit — measured 2026-09-05

Run against `Cargo.lock` (572 crates) and `node_modules`, resolving each crate's
`license` field from the vendored source rather than from memory.

### The headline

**No GPL, LGPL or AGPL anywhere in the Rust tree.** The Stage 0 decision to use
`sqlx` — a pure-Rust protocol implementation — rather than anything binding
`libmysqlclient` (GPL-2.0) is what bought that, and it has held all the way
through.

| Licence | Crates |
|---|---|
| MIT / Apache-2.0 (in every spelling) | ~400 |
| Unicode-3.0 (the ICU crates) | 18 |
| Unlicense OR MIT | 8 |
| MPL-2.0 | 5 |
| BSD-3-Clause | 3 + 2 combined |
| ISC, Zlib, 0BSD, CC0 | ~7 |

### The five MPL-2.0 crates, and why only one matters

MPL-2.0 is *file-level* copyleft: it can ship inside a closed product, but the
source of those files must be available. Four of the five never reach the
binary:

| Crate | Reaches us via | Ships? |
|---|---|---|
| `cssparser` | `dom_query` → `tauri-utils` → `tauri-codegen` → **`tauri-macros` (proc-macro)** | no — build-time |
| `selectors` | same | no — build-time |
| `dtoa-short` | via `cssparser` | no — build-time |
| `brotli` (BSD-3 AND MIT) | `tauri-codegen` → proc-macro | no — build-time |
| **`option-ext`** | `dirs-sys` → `dirs` → **`tauri`** | **yes — runtime** |

Proc-macro crates and their dependencies compile for the *host* and run during
the build; they are not linked into the shipped artefact. `cargo tree -i` says
so for each, which is why the table can be specific rather than cautious.

So the MPL obligation reduces to **one crate, `option-ext`**, discharged by
naming it and linking to its source in the licence manifest.

`lightningcss` (MPL-2.0) is the same story on the npm side: it arrives through
`vite`, a devDependency, and is a build-time CSS minifier. Nothing from
`node_modules` is distributed — only the bundle vite emits.

### What this audit does **not** cover

Two gaps, stated because an audit that overstates itself is worse than none:

- **108 crates could not be read locally**, because their sources are not
  vendored on this machine: 24 `windows-*`, 25 Apple/`objc2`, 7 Android, and
  ~51 that belong to other targets (`redox`, `haiku`, `wasm`, `jni`). Most will
  never compile into *our* binary; the Windows and macOS ones will, on those
  builds. **The audit is therefore Linux-only and must be re-run per target.**
- **`Cargo.lock` is the union of every platform's dependencies**, so counting it
  overstates what ships. Only `cargo tree --target … -e normal` distinguishes
  "in the lockfile" from "in the binary", which is how the MPL table above was
  produced.

The fix for both is the same and belongs in this stage: generate the manifest
**per target, in CI, from the real build**.

### Correction this audit produced

`Cargo.toml` claimed `tauri-plugin-fs` "is deliberately NOT used". Half true, and
the half that was wrong matters here: it is a **transitive dependency of
`tauri-plugin-dialog`** and is compiled in. What we actually control is that we
never call it and never grant it — the capabilities file lists no `fs:`
permission, so JavaScript cannot reach the filesystem through it. The comment
now says that instead. A licence manifest lists what is linked, not what we
chose to call.

---

## 3. The webview is the distribution story

Tauri does not ship a browser; it uses the platform's. That is the whole reason
the binary is small, and the source of every platform-specific packaging
problem.

| Platform | Webview | Licence | Consequence |
|---|---|---|---|
| Linux | WebKitGTK 4.1 | LGPL-2.1 | **Dynamically linked** — a system library, so the LGPL is satisfied without shipping source. Must be a hard package dependency (`libwebkit2gtk-4.1-0`). |
| Windows | WebView2 (Edge) | Microsoft redistributable terms | Present on Windows 11; on older machines the installer must bootstrap it. |
| macOS | WKWebView | System framework | Always present. **Out of scope** — listed for completeness. |

**The LGPL point is worth being precise about**: dynamic linking against a
system WebKitGTK is fine and imposes no source obligation on us. Statically
linking it — or bundling a modified copy inside an AppImage — is a different
question, and is the reason AppImage needs care rather than being the obvious
choice.

---

## 4. Decisions taken — 2026-09-06

Answered by the user, and they narrow this stage sharply.

| Question | Answer | What it costs, or saves |
|---|---|---|
| Which machines | **Linux + Windows.** macOS out of scope. | Saves the whole Apple long pole: $99/yr enrolment, a Developer ID certificate, and notarization on every release. **Signing is no longer this stage's critical path.** |
| What builds it | **GitHub Actions → Releases**, tag-triggered | Requires pushing to GitHub — there is no remote today. Also closes Stage 4's carried **T7** and **T8** in the same workflow. |
| Repo visibility | **Public** | Unlimited free CI minutes, and release assets are plain URLs. History was scanned for secrets before this was agreed — see §4.6. |
| Auto-update | **Yes, in the first release** | Forces a format decision on Linux. See §4.4. |
| Licensing | **Fully open source is fine**; the care is that everything is properly attributed | The repo needs its own `LICENSE`, which it does not have. See §4.5. |
| Authorship | **`SparedRay`, via GitHub's private no-reply address** | Done. All five commits rewritten before any push, so the real address never reaches a public repo. |

### 4.1 Why Windows currently needs a command line, and what fixes it

This machine **cannot build a Windows installer**, and that is not a
misconfiguration. `tauri build --help` here offers exactly `deb, rpm, appimage`
as bundle targets, because installer formats are produced by the OS they target.
Tauri's own documentation calls cross-compiling to Windows a *"last resort if
local VMs or CI solutions like GitHub Actions don't work for you"* — and it
needs NSIS, LLVM, LLD and `cargo-xwin` to get there.

So the fix is a **Windows runner**, and the artefact is an **NSIS
`db-query_x.y.z_x64-setup.exe`**:

- **`currentUser` is the default install mode** — into `%LOCALAPPDATA%`, with
  **no Administrator prompt**. Start Menu shortcut, Add/Remove Programs entry,
  double-click to launch. Never a terminal again.
- **WebView2** defaults to `downloadBootstrapper`: the smallest installer, which
  fetches the runtime at install time if it is missing. Windows 11 ships it
  already. The alternative, `offlineInstaller`, embeds ~127MB.

### 4.2 The bundle identifier still says "poc"

`com.dbquery.poc`. Not cosmetic — it is the config directory
(`~/.config/com.dbquery.poc/connections.json`), and on Windows it is part of how
the OS identifies the app. Changing it after a release **orphans every saved
connection**.

The keychain is separate and safer: `secrets.rs` uses the service name
`db-query`, which contains no identifier. So a rename would strand the profile
list while leaving the passwords behind — the worst of both. **Decide before
anything is distributed**, and if it changes, ship the migration in the same
release.

### 4.3 Icons — resolved 2026-09-06

*Was: a hard blocker. Windows bundling needs an `icon.ico`, and the repo had
only four PNGs, all Tauri's defaults.*

**Done.** The mark is a result grid with one column selected — the app's own
signature gesture, and the option that stayed legible when four candidates were
rendered at 512, 128, 48 and 32 px on both dark and light grounds. The source is
[`design/logo-c-column.svg`](../design/logo-c-column.svg); three rejected
candidates sit beside it for the record.

`tauri icon` generated the set from it. `icon.ico` carries six frames — 16, 24,
32, 48, 64 and 256 px — each checked by rendering it back out. At 16 px the grid
compresses to a blue stripe, which still reads as a selected column; from 24 px
up it is crisp. `bundle.icon` now lists `.ico` and `.icns` alongside the PNGs,
and a rebuilt `.deb` installs them into `usr/share/icons/hicolor/`, so the
launcher entry gets the mark.

The Android and iOS assets `tauri icon` also emits were deleted: those platforms
are not in scope, and unused binaries in a public repo are noise.

### 4.4 Auto-update forces a Linux format decision — and it is measured

Verified against Tauri's updater documentation, not remembered:

- **Linux: AppImage only.** A `.deb` **cannot** self-update.
- **Windows: NSIS and MSI both work.** The app is force-exited during the
  install step — a Windows installer limitation, not a Tauri choice.
- The update signing keypair (`tauri signer generate`) is **separate from any
  code-signing certificate**, costs nothing, and the public half goes in
  `tauri.conf.json` while the private half is a CI secret.

**A trial build here measured what that choice actually costs:**

| Format | Size | What it contains |
|---|---|---|
| `.deb` | **4.0 MB** | The binary. Declares `Depends: libwebkit2gtk-4.1-0, libgtk-3-0` — the system's webview, not ours. |
| `.AppImage` | **76 MB** | **170 bundled libraries, including `libwebkit2gtk-4.1.so.0` and `libgtk-3.so.0`.** |

That 19× difference is the LGPL question the risk list predicted, now answered
with evidence. The `.deb` **links** the system WebKitGTK — no source obligation
of any kind. The AppImage **distributes** it, which under LGPL-2.1 obliges us to
carry the licence text and to offer the source of the libraries we ship.

**Recommendation: `.deb` on Linux, and wire the updater for Windows only.**
Auto-update is what removes friction on Windows; on Linux the target machine is
the development machine, where the repo already is. Open-sourcing the project
makes the AppImage obligation cheap to discharge rather than free — but 76 MB
and a copyleft obligation are still a real price for updating an app on a box
that has the source tree on it. **Reversible later**: adding the AppImage is a
config line plus a licence file, so this is not a door that closes.

### 4.5 The project has no licence of its own

A public repo with no `LICENSE` is **"all rights reserved"** by default, which
is the opposite of open-sourcing it. This must land before or with the push.

**Chosen: MIT** (2026-09-06), over AGPL-3.0. The reasoning is worth recording,
because the question asked was narrower than the answer:

- **Attribution was never the gap.** Every candidate — MIT included — requires
  anyone redistributing the code to keep the copyright notice. Nobody may strip
  the authorship off it under any of them.
- **No open source licence prevents commercialisation.** GPL, AGPL and MIT all
  explicitly permit selling. What copyleft controls is whether a fork may be
  *closed*, not whether it may be *sold*.
- **AGPL-3.0 was the alternative** and was verified compatible with this
  dependency tree (permissive crates fold into copyleft; MPL-2.0 permits it;
  the LGPL webview is dynamically linked — with the constraint that Apache-2.0
  is GPL **v3**-compatible and not v2). It was declined in favour of reach.

Two consequences accepted deliberately: someone may fork this, close it and
sell it, owing only a copyright line; and accepting outside contributions later
makes relicensing hard without a CLA, since contributors hold copyright in
their patches. Copyright in existing work is retained either way, so *future*
versions can be licensed differently — never retroactively.

`license = "MIT"` is declared in `src-tauri/Cargo.toml` and `package.json`, so
licence tooling reads it rather than guessing.

On the AI-assisted point specifically, keeping the two questions apart is what
keeps this honest:

- **Dependencies** — measured, and the answer is good: no GPL/LGPL/AGPL in the
  Rust tree at all (§2), one MPL-2.0 crate that actually ships, and the LGPL
  webview linked dynamically as a system library. That is the part §2 and §3
  cover, and it is the part that could have gone wrong through a careless
  library choice.
- **The code in this repo** — original to this project rather than copied from
  a codebase with its own terms. What no licence audit can settle is the
  unsettled legal status of AI-assisted output generally; the honest position
  is to say that plainly in the README rather than to imply a licence audit
  answered it.

### 4.6 History was scanned before agreeing to publish

Every blob in `git log --all` was searched for credentials, keys and tokens.
**Nothing found.** The only hits were source identifiers (`password: Secret`,
`remember_password`), the fixture password `devpassword` in `dev/seed.sql` —
which is the throwaway password of a local podman container documented as such —
and `hunter2` in a unit test. Passwords have never been in the config file by
construction, and the keychain is not in the repo.

Worth repeating before the push, since history grows.

---

## 5. Signing — much smaller than the draft assumed

Dropping macOS removes the item that had money and weeks of lead time attached.
What remains:

| Platform | Unsigned experience | Cost to fix |
|---|---|---|
| **Linux** | Nothing blocks anything. `.deb` installs normally. | n/a |
| **Windows** | SmartScreen: *"Windows protected your PC"* → **More info** → **Run anyway**. Once, on first install. | An OV/EV code-signing certificate, ~$200-500/yr, now requiring a hardware token or cloud HSM. |

**Recommendation: ship unsigned, and say so in the release notes.** For an
open-source tool installed by the person who built it, one click on first
install is the correct trade against several hundred dollars a year and an
identity-verification process.

One consequence to be deliberate about: **the updater downloads and runs an
unsigned installer.** Tauri's own minisign signature still protects the update
channel — an update that is not signed with our private key is rejected — so the
risk is not "anyone can push an update to us". It is that the OS cannot
independently vouch for what we push. That distinction belongs in the release
notes rather than being glossed.

---

## 6. Milestones

- [ ] **P1 — It is on GitHub, public, with a licence.** History scanned again at push time; `LICENSE` present; README states what is and is not audited.
- [ ] **P2 — CI is green on Linux and Windows.** `check`, the Rust suites, and the UI suite on every push. **This closes Stage 4's T7.**
- [ ] **P3 — Windows is no longer a blank spot.** The Windows job runs `cargo test` and the keychain round trip against Credential Manager. **This closes T8 / E0 / C3w, carried since Stage 2.**
- [ ] **P4 — A tag produces installers.** `.deb` and `-setup.exe` attached to a GitHub Release, downloadable from a plain URL.
- [ ] **P5 — It installs on a Windows machine that has never seen the toolchain**, from a double-click, with no Administrator prompt, and appears in the Start Menu. Then it connects to a database and remembers a password across a relaunch.
- [ ] **P6 — It updates itself on Windows.** Release `x.y.z+1`, and the installed copy offers it, applies it, and comes back running the new version.
- [ ] **P7 — Attribution ships.** `THIRD-PARTY-LICENSES` generated per target by `cargo about` in CI, in the bundle, and reachable from inside the app.
- [ ] **P8 — Nothing regressed.** Stages 0-4 suites pass by name on both platforms in the matrix.

---

## 7. Task tracker

### Phase 0 — Publish
- [ ] Re-scan history for secrets immediately before the push
- [x] Choose the licence — **MIT**, chosen 2026-09-06 over AGPL-3.0; reach over control, and no open licence prevents commercialisation anyway. `LICENSE` added.
- [x] README: what the licence audit covers, its two stated limits, and the honest note on AI-assisted code
- [ ] Create the public repo; push `main`

### Phase 1 — Make the bundle releasable
- [ ] **Final bundle identifier**; migration if it changes (§4.2)
- [x] **A real icon; `tauri icon` to generate `icon.ico`** — done, §4.3
- [x] Explicit per-platform `bundle.targets`: `["deb"]` in `tauri.conf.json`, `["nsis"]` in `tauri.windows.conf.json` (auto-merged by the CLI per platform). A plain `tauri build` now emits the 4 MB `.deb` and nothing else.
- [ ] A display `productName` (`db-query` is a directory name, not a title)
- [ ] Version single-sourced between `Cargo.toml`, `package.json` and `tauri.conf.json`
- [ ] Fill in the `.deb` description and maintainer — currently `(none)` and `db-query`
- [ ] A `Categories=` value in the desktop entry, currently empty, so it files under Development

### Phase 2 — CI (closes T7, T8)

> **Written, not yet verified.** `.github/workflows/ci.yml` exists and parses,
> and every command in it passes locally. No workflow has ever *run*, because
> there is no remote. **T7 and T8 stay open until a green run exists** — a
> workflow file is a hypothesis, not evidence.

- [x] `ci.yml`: `ubuntu-latest` + `windows-latest`, on push to `main`, on tags, and on pull requests
- [x] Linux: apt deps, the four `check` commands, both Rust suites, **the live MySQL suite against a `mysql:8` service container** seeded from `dev/seed.sql`, then the UI suite on both engines
- [x] Windows: the same `check` and Rust suites, plus the UI suite on Chromium — the engine WebView2 actually is
- [x] **The Windows job settles C3w/E0**: `cargo test --test keychain -- --ignored` against Credential Manager. A failure there is a *result*, not an accident
- [x] Playwright report and traces as artefacts on failure, kept 14 days
- [x] `Swatinem/rust-cache` per platform, npm cache, and the Playwright browser cache
- [ ] **A green run on both platforms** ← the actual milestone

Toolchain versions are repeated in the workflow rather than driven by `mise`,
because mise on the Windows runner is the less-travelled path and a broken
toolchain step would hide real failures. **That is a drift risk**: bumping
`mise.toml` means bumping `ci.yml`. Noted in the workflow header.

### The first CI run failed on both platforms — 2026-09-07

Both jobs died at the same line, before a single test ran:

```
error: proc macro panicked
  --> src/lib.rs:780  .run(tauri::generate_context!())
  = help: message: The `frontendDist` configuration is set to `"../dist"`
                   but this path doesn't exist
```

**`generate_context!()` reads `tauri.conf.json` at compile time and hard-fails
if `frontendDist` is missing.** `dist/` is gitignored, so a fresh checkout has
none — and nothing in the cargo steps creates it. `npm run build` is wired as
Tauri's `beforeBuildCommand`, which `tauri build` runs and a plain `cargo build`
or `cargo test` does not.

Fixed by building the frontend before any cargo step in both jobs. It costs
nothing: `npm run build` is `tsc --noEmit && vite build`, so it replaces the
separate typecheck step rather than adding one.

**Why it was not caught, which is the part worth keeping.** Every command in the
workflow *was* run locally before pushing, and every one passed. They passed
because this working tree has had a `dist/` since the first `tauri build` days
earlier — a side effect of unrelated work, invisible because it is gitignored.
**Verifying a command in a dirty tree does not verify it in CI**; the only
faithful model of a runner is a clean checkout.

So there is now a dry run, and it is cheap:

```bash
git clone --no-hardlinks . /tmp/ci-dry && cd /tmp/ci-dry
npm ci && npm run build && cargo build --manifest-path src-tauri/Cargo.toml
```

Anything that only exists because of local history fails there, in two minutes,
instead of on a runner.

Three further landmines were fixed pre-emptively by reading the workflow as if
it were a clean machine, rather than waiting for three more red runs:

- **`mysql-client` added to the apt step.** The seed step shells out to `mysql`;
  whether a runner image happens to ship it is not something to depend on.
- **`--with-deps` dropped from the Windows browser install.** It installs Linux
  OS packages and has no job on Windows.
- **The Windows Playwright cache path** uses forward slashes, so no backslash
  reaches YAML.

### Phase 3 — Release workflow

> **Written, not yet verified**, for the same reason as Phase 2.

- [x] `release.yml`: tag-triggered (`v*`) plus manual dispatch, `tauri-apps/tauri-action`, **draft** release so notes can be edited before anyone sees them
- [x] Linux job → `.deb`; Windows job → NSIS `-setup.exe`
- [x] Release-notes template: install steps for both platforms, and a plain statement that the build is unsigned with the SmartScreen click-through spelled out
- [x] Updater signing env vars wired but unset — harmless until `plugins.updater` exists, required the moment it does
- [ ] **A tag that actually produces two downloadable installers**

The Linux release job pins **`ubuntu-22.04`, not `ubuntu-latest`** — glibc 2.35
rather than 2.39, so the `.deb` installs on 22.04-era machines as well as this
one. Building on 24.04 would silently narrow that. If the image is retired, the
fix is one line and the cost is compatibility.

**Release does not gate on CI.** A tag triggers both workflows in parallel; a
red CI run means pull the draft, it does not stop the build. Gating would mean
building everything twice.

### Phase 4 — Updater (Windows)
- [ ] `tauri signer generate`; public key into `tauri.conf.json`
- [ ] `TAURI_SIGNING_PRIVATE_KEY` (+ password) as repo secrets, never in the tree
- [ ] `tauri-plugin-updater`; endpoint served from GitHub Releases
- [ ] **Verify the plugin's licence and add it to the audit** — standing rule: read it from source, not from memory
- [ ] In-app: **ask before updating**, through `dialog.ts` — `window.confirm` is unavailable, and "the user decides when something runs" is this project's rule
- [ ] **The `.deb` build must not offer updates it cannot apply.** Detect and disable, rather than failing at download time
- [ ] Tests: an update offered, declined, accepted; a manifest signed with the wrong key is rejected

### Phase 5 — Licence compliance
- [ ] `cargo about` config; `THIRD-PARTY-LICENSES` generated per target in CI
- [ ] **Re-run the audit on the Windows tree** — the ~24 `windows-*` crates §2 could not read locally
- [ ] Name `option-ext` (MPL-2.0) with a source link
- [ ] npm attribution for the shipped bundle only — CodeMirror, `@tauri-apps/api` — not `node_modules`
- [ ] Surface it in the app: an About dialog or a menu item

### Phase 6 — Verification
- [ ] P5 and P6 on a real Windows machine, not the runner that built them
- [ ] The `.deb` on a Pop!_OS machine: launcher entry, icon, it starts
- [ ] Uninstall leaves no stray config or keychain entries — or documents what it leaves and why
- [ ] Work through Stage 4 §6b: **A1-A5 and B1-B3 are now reachable**, because there is an installed app on two platforms to click

---

## 8. Risks

- ~~**The icon blocks Windows.**~~ **Cleared** — `icon.ico` exists and the
  bundle config points at it (§4.3).
- **`com.dbquery.poc` escaping into a release.** The cheapest catastrophe on
  this list to prevent and the most annoying to undo.
- **Unsigned Windows builds carry an unsigned updater.** The update *channel* is
  protected by minisign; the *installer* is not vouched for by the OS. Say so
  rather than discover it.
- **The audit is still Linux-only.** Windows pulls in ~24 `windows-*` crates
  this machine has never downloaded. They are MIT/Apache-2.0 by reputation —
  which is an expectation, not a measurement, and Phase 5 turns it into one.
- **Public repo, permanent history.** Publishing is not reversible in practice;
  clones and forks outlive a deletion. §4.6 is why this was checked first.
- **Windows CI minutes count double.** Free on a public repo, so this is only a
  risk if the repo is ever made private.
