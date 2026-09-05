# db-query

A lightweight MySQL client that never freezes. Tauri v2 + vanilla TS + CodeMirror 6.

Plan, milestones and task tracker: [sql-client-tracker.md](sql-client-tracker.md).
Licensing constraints for this POC: [§9 of the tracker](sql-client-tracker.md#9-licensing-poc-constraint).

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
npm install
npm run tauri dev
```

## Test

The statement splitter is the one component with real unit tests — it gates
auto-LIMIT, result tabs and statement-under-cursor, so a mis-split silently
runs the wrong SQL.

```bash
cd src-tauri && cargo test    # 40 offline unit tests
npx tsc --noEmit              # frontend type-check
```

Against a live server (see `dev/README.md` to bring one up) — these are
`#[ignore]`d so the offline suite stays runnable anywhere:

```bash
cd src-tauri && cargo test --test live_mysql -- --ignored --test-threads=1
```

Single-threaded on purpose: the tests share one exec connection, and MySQL
session state (`USE`, temp tables) is per-connection.

## Targets

MySQL **8.0+**. TLS verifies the CA chain and hostname by default; the
connection form has an "allow invalid certificates" toggle for internal servers
with self-signed certs, which drops to encrypted-but-unverified — never to
plaintext.
