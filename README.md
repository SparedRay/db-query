# db-query

A lightweight MySQL client that never freezes. Tauri v2 + vanilla TS + CodeMirror 6.

Plans, milestones and task trackers live in [docs/](docs/) — one per stage,
indexed in [docs/README.md](docs/README.md).
Licensing constraints: [§9 of the Stage 0 tracker](docs/stage-0-poc.md#9-licensing-poc-constraint).

## Prerequisites

Two separate steps, because they come from different places.

**1. Language toolchains — mise.** Versions are pinned in `mise.toml`; mise
drives rustup under the hood, so `rustfmt` and `clippy` come with it.

```bash
mise trust && mise install
```

**2. System libraries — apt.** mise does not manage C dev packages, and Tauri
links these natively via pkg-config, so this step needs sudo:

```bash
sudo apt update && sudo apt install -y \
  build-essential curl wget file pkg-config \
  libwebkit2gtk-4.1-dev libsoup-3.0-dev librsvg2-dev \
  libayatana-appindicator3-dev libssl-dev
```

Verify both:

```bash
cargo --version && pkg-config --modversion webkit2gtk-4.1
```

> **Both steps are required before `cargo test` will run.** The crate depends on
> `tauri`, which links webkit2gtk at build time — so even the pure-Rust splitter
> tests cannot compile until the apt packages are present.

## Develop

```bash
mise run dev
```

`mise run` applies mise's environment itself, so it works whether or not mise is
activated in your shell. Plain `npm run tauri dev` needs `cargo` on `PATH` —
see below.

### Getting `cargo` on your PATH

mise manages the Rust toolchain, but installing it does not put it on `PATH`;
that needs activation. Without it you get:

```
failed to run 'cargo metadata' ... No such file or directory (os error 2)
```

For an interactive shell (bash):

```bash
echo 'eval "$(mise activate bash)"' >> ~/.bashrc && exec bash
```

IDEs and GUI-launched processes usually do not read `~/.bashrc`, so also put
mise's shims on `PATH` for those:

```bash
echo 'export PATH="$HOME/.local/share/mise/shims:$PATH"' >> ~/.profile
```

## Tasks

```bash
mise run dev        # run the app with hot reload
mise run check      # build, clippy, rustfmt --check, tsc
mise run test       # offline tests: no database or keychain needed
mise run db-up      # start and seed the MySQL 8 fixture container
mise run test-live  # tests needing the fixture and a keychain
mise run db-down    # tear the fixture down
mise run es-up      # start and seed the Elasticsearch fixture
mise run test-es    # tests needing that fixture
mise run es-down    # tear it down
```

## Third-party licences

The app ships the notices its dependencies require — `THIRD-PARTY-LICENSES.txt`,
generated **per target** and readable from *Settings → About*.

Generating it needs one tool, once:

```bash
cargo install cargo-about --locked --features cli
npm run attribution
```

It runs automatically as part of `tauri build`, so a bundle cannot be produced
without it. The file is not committed: it is ~390KB and target-specific, and a
Linux-generated copy inside a Windows installer would be wrong rather than
merely short.

## Releasing

```bash
git tag v0.2.0 && git push origin v0.2.0
```

That is the whole flow. **The version comes from the tag**: CI stamps it into
`package.json` — which `src-tauri/tauri.conf.json` reads, via
`"version": "../package.json"` — and into `Cargo.toml`, so the installers and
the updater manifest all call themselves what the tag says.

The build produces a **draft** release so the notes can be edited. **Publish it**
before expecting anyone to receive it: the updater endpoint is
`releases/latest/download/latest.json`, and a draft is not `latest`.

To move the version in the working tree — so a checked-out tag builds the same
number — `mise run set-version 0.2.0`.

## Targets

MySQL **8.0+**, and **Elasticsearch SQL** (`POST /_sql`) — read-only, since that
is all the engine's SQL surface offers.

MySQL TLS verifies the CA chain and hostname by default; the
connection form has an "allow invalid certificates" toggle for internal servers
with self-signed certs, which drops to encrypted-but-unverified — never to
plaintext.

## Licence

[MIT](LICENSE) — use it, change it, ship it, sell it. Keep the copyright notice.

### What has actually been checked

Dependency licensing was **measured, not assumed** — resolved from each crate's
vendored source rather than from memory. The full audit, with method and
counts, is in
[§2 of the Stage 5 tracker](docs/stage-5-packaging.md#2-licence-audit--measured-2026-09-05).
The short version:

- **No GPL, LGPL or AGPL anywhere in the Rust tree.** This is a consequence of
  one early decision: `sqlx` speaks the MySQL protocol in pure Rust, so nothing
  here binds `libmysqlclient`, which is GPL-2.0.
- **One MPL-2.0 crate reaches the binary** (`option-ext`, via `dirs`). The other
  four are proc-macro dependencies that run at build time and are not linked.
- **The Linux webview (WebKitGTK, LGPL-2.1) is dynamically linked** as a system
  library — the `.deb` declares it as a dependency and ships none of it.

Two limits, stated because an audit that overstates itself is worse than none:

- **It is Linux-only.** ~108 crates could not be read on the audit machine
  because they belong to other targets. Re-running it per platform in CI is a
  tracked task.
- **`Cargo.lock` is the union of every platform's dependencies**, so counting it
  overstates what ships. Only `cargo tree --target … -e normal` distinguishes
  "in the lockfile" from "in the binary".

### On AI-assisted code

This project was written with heavy AI assistance, and that is a different
question from the one above — worth keeping separate rather than letting a
dependency audit imply it was answered.

What the audit establishes is that no dependency imposes terms we are not
honouring. What it cannot establish is the copyright status of AI-generated
output, which is genuinely unsettled and varies by jurisdiction. The code here
is original to this project rather than copied from another codebase, and it is
offered under MIT on that basis. If you need stronger provenance guarantees than
that, this is a POC and you should assume nothing beyond what this paragraph
says.
