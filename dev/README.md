# Dev fixtures

Three throwaway containers for the milestone checks — MySQL, Elasticsearch and
Ollama. All rootless podman, all bound to localhost, none of them something the
app depends on at runtime. No sudo after the one-time `apt install podman`.

| Fixture | Up | Down | Tests |
|---|---|---|---|
| MySQL 8 | `mise run db-up` | `mise run db-down` | `mise run test-live` |
| Elasticsearch 8.15 | `mise run es-up` | `mise run es-down` | `mise run test-es` |
| Ollama | `mise run ollama-up` | `mise run ollama-down` | `mise run test-ollama` |

Each `*-up` task starts the container, waits for it to answer, and seeds it, so
running one twice is harmless.

## MySQL

Connect from the app with host `127.0.0.1`, port `3306`, user `root`,
password `devpassword`, database `poc`.

**Tick "Allow invalid certificates".** The container generates a self-signed
server cert, so the default `VerifyIdentity` mode will (correctly) refuse it.
That refusal is itself worth seeing once — it proves the secure default works.

### What each object is for

| Object | Proves |
|---|---|
| `types_zoo` | Every `CellValue` decode arm; row 2 straddles the 2^53 BIGINT boundary; row 3 is all-NULL; `c_literal_null` holds the *string* `"NULL"` so it must render differently from a real null |
| `big` (7500 rows) | Auto-LIMIT truncates at 5000 and the chip appears |
| `users` / `orders` | Joins, aliases, autocomplete, and DECIMAL money that must not go through `f64` |
| `user_totals` | The tree's VIEW vs BASE TABLE distinction |

## Elasticsearch

Security is off, so the connection needs no credentials: choose
**Elasticsearch** in the connection dialog and enter `http://localhost:9200`.

The fixture seeds one index, `orders`, with three documents and a mapping
covering a keyword, a double and a date. It is enough to prove the second
engine end to end: `SELECT user, total FROM orders ORDER BY total DESC` returns
rows through the ordinary grid, and `DELETE FROM orders` is refused before a
request is sent — the capability model says this engine has no writes.

## Ollama

Only needed to exercise the assistant without an API key or a bill. The fixture
pulls **qwen2.5-coder:1.5b** (Apache-2.0), which runs on CPU in a couple of GB;
Ollama itself is MIT. Set `OLLAMA_MODEL` to use a different one.

In Settings, choose the **Ollama (local)** preset and enter the model name —
the base URL is already right and no key is asked for, because the app treats a
loopback endpoint as local and says so in the chat header.

The model is small enough that its SQL is often wrong. That is fine: this
fixture proves the OpenAI-compatible adapter streams and that nothing leaves
the machine, not that the answers are good.

```bash
mise run ollama-down            # stop; the pulled model survives in a volume
podman volume rm db-query-ollama  # and this reclaims its disk
```

### Not in CI

Unlike MySQL and Elasticsearch, the Ollama fixture is deliberately local-only.
CI would pay a ~3 GB image and a ~1 GB model on every run to assert something
non-deterministic — a 1.5B model's SQL varies run to run. The adapter's
*behaviour* is pinned by 240 offline unit tests; this fixture exists to prove
the wire format once, by hand, against something real.

## When the disk fills up

`cargo` never prunes `src-tauri/target/`: every dependency version and every
test binary ever built stays in it. On 2026-10-01 `target/debug` had reached
**97 GB** on the development machine, while a complete rebuild of it needs
**3.2 GB** and 82 seconds — so almost all of it was accumulation, not need.

```bash
mise run clean        # reclaims target/debug, keeps release
```

Worth knowing because of how a full disk announces itself: the **linker** fails,
and reports it as an opaque `linking with cc failed` with a wall of object
files. `df -h .` is the first thing to check when a build breaks that way.

The fixture containers are the other few gigabytes (`podman system df`), but
those are images you want to keep unless you are done with a fixture entirely.

## Debugging migrations end to end, by hand

Nothing needs installing: `mise run flyway-up` fetches the pinned 13.5.0 into
`dev/.flyway`, which is why `which flyway` finds nothing on a machine that can
already run the Flyway tests.

```bash
mise run db-up        # MySQL, in podman
mise run flyway-up    # the pinned CLI + flyway_dev, flyway_qa, flyway_probe
mise run dev          # the real app, real backend
```

Then, in the app:

1. **Settings → Migrations → Flyway command**: paste the absolute path printed
   by `realpath dev/.flyway/flyway-13.5.0/flyway`. The row above the field
   reports the version once it answers, which is the quickest check that the
   path is right. Leaving it empty means "whatever is on the PATH", and the
   pinned copy deliberately is not.
2. **Migrations sidebar → add project** → `dev/flyway/flyway.toml`.
3. Pick **`probe`**. It exists to be thrown away — the live tests clean it — so
   it is the one to experiment in. `development` ends at a deliberately broken
   V4, which is the interesting case: apply it and the output pane shows the
   three that ran, then why the fourth stopped.

This exercises everything the stubs cannot: the real spawn (including
`CREATE_NO_WINDOW` on Windows), the progress strip while a JVM starts, and the
output pane rendering a document Flyway actually produced.

**`FLYWAY_BIN`** points the *Rust live tests* at a different binary; the app
reads the Settings field instead, so the two can disagree deliberately — a
system Flyway in the app, the pinned one under test.
