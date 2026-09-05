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
```

## Targets

MySQL **8.0+**. TLS verifies the CA chain and hostname by default; the
connection form has an "allow invalid certificates" toggle for internal servers
with self-signed certs, which drops to encrypted-but-unverified — never to
plaintext.
