# Dev fixtures

Four throwaway containers for the milestone checks — MySQL, Elasticsearch
(twice) and Ollama. All rootless podman, all bound to localhost, none of them
something the app depends on at runtime. No sudo after the one-time
`apt install podman`.

| Fixture | Up | Down | Tests |
|---|---|---|---|
| MySQL 8 | `mise run db-up` | `mise run db-down` | `mise run test-live` |
| Elasticsearch 8.15 | `mise run es-up` | `mise run es-down` | `mise run test-es` |
| Elasticsearch, secured | `mise run es-secure-up` | `mise run es-secure-down` | `mise run test-es-secure` |
| Ollama | `mise run ollama-up` | `mise run ollama-down` | `mise run test-ollama` |

Each `*-up` task starts the container, waits for it to answer, and seeds it, so
running one twice is harmless.

**`db-wait` is why they wait over TCP.** On a container's *first* boot the
official MySQL image runs initialisation against a temporary server started
`--skip-networking` on the unix socket, so a socket ping answers about four
seconds before the real server exists — and whatever runs next is talking to a
server that is about to be shut down. That is a failure you only ever see on a
fresh container, which is the worst kind. Every task that needs MySQL depends
on `db-wait`; none of them has its own copy of the check.

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

### The secured one, on 9201

Security off is what makes the fixture above one command, and it is also what
made it blind: a cluster that cannot tell one account from another can never
answer **403**, and a 403 was the whole of [Stage 11
§12](../docs/stage-11-elasticsearch.md) — an account with index privileges and
no cluster privileges could not connect at all.

`es-secure-up` is a second cluster, on **9201**, with three accounts:

| Account | Password | Holds |
|---|---|---|
| `elastic` | `measure-me-9201` | superuser: reads the root, reports a version |
| `restricted` | `restricted-9201` | `index_admin` — no cluster privileges, everything on `orders*` |
| `nobody` | `nobody-9201` | nothing at all: cannot run even `SELECT 1` |

Connect as `restricted` with **Basic** auth and `http://localhost:9201`. The
connection comes up, the tree fills, queries run — and the status shows the
version as `?`, because `GET /` needs the `monitor` *cluster* privilege and
nothing else this engine does needs any cluster privilege at all.

TLS is off and the passwords are in the task, deliberately: the cluster holds
two documents and exists to be thrown away.

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
test binary ever built stays in it.

```bash
mise run clean        # reclaims target/debug, keeps release
```

Worth knowing because of how a full disk announces itself: the **linker** fails,
and reports it as an opaque `linking with cc failed` with a wall of object
files. `df -h .` is the first thing to check when a build breaks that way.

### What the space actually is

Measured 2026-10-03, after one day of ordinary work (edit, `cargo test`,
`cargo clippy --all-targets`, a few live suites):

| | |
|---|---|
| `target/debug/deps` | 12 G — every build's rlibs and test binaries, old hashes kept |
| `target/debug/incremental` | 7 G — per-edit recompilation state |
| `target/debug/build` | 1.4 G — build script output |
| `target/release` | 2.7 G |
| `dev/.flyway` | 902 M — the pinned Flyway CLI, with its own JRE |
| **a fresh `cargo test --lib`** | **3.2 G, 79 s** |

So **21 G of the 24 G was accumulation, not need** — the fresh number is the one
to compare against. Everything above is in `.gitignore`; the tracked repository
is about 18 MB, and its largest file is 172 KB.

### The one knob, measured

`debug = "line-tables-only"` on the dev profile builds **2.4 G in 69 s** instead
of 3.2 G in 79 s, and panic backtraces keep full `file:line:column` — verified
by panicking on purpose and reading the frames. What it costs is variable
inspection in a debugger.

**It is deliberately not the default**, because it treats the wrong problem: it
would have made 21 G into 16 G, while `mise run clean` makes it 0. Set it per
run when disk is tight and a debugger is not:

```bash
CARGO_PROFILE_DEV_DEBUG=line-tables-only cargo test --manifest-path src-tauri/Cargo.toml
```

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

## A folder of scripts: `dev/scripts`

Stage 23's batch runner reads a folder, so the fixture *is* a folder.
`dev/scripts/README.md` lists what each file is there to prove — a `DELIMITER`
block, a file of only comments, a missing trailing semicolon, a UTF-8 BOM, three
result sets in one file, and one in a subfolder so the output path has to mirror
rather than flatten.

`01-setup.sql` creates `batch_probe` and `99-teardown.sql` drops it, so the
folder can be run twice and leaves `poc` as it was found. Nothing needs starting
first beyond `mise run db-up`.

`cargo test --test sqlfiles_scan` reads these files from disk and asserts the
numbers the dialog's preview shows. **If you edit one of them, that test is
where it fails** — deliberately, because a fixture that drifts from what the
tests expect is a fixture that proves nothing.

## Driving the schema export by hand

The Playwright tests stub `invoke`, so nothing automated has ever had the
dialog talk to a real server: the selection is proved against a stub, and the
script is proved by `live_export_probe.rs` without the dialog. This recipe is
the join between them.

```bash
mise run db-up
mise run export-probe-up   # views on views, trigger order, 1,000,000 rows
mise run dev
```

Connect as above, then **right-click a database → Export schema** (the rail's
connection menu opens the same dialog with nothing preselected).

What is worth doing once, because no test covers it:

1. **`export_probe`, schema and data, one file.** 1M rows in `wide`, so the
   strip has time to show real progress — objects done of total, then rows and
   megabytes. Press **Stop** in the middle: the `.part` file must be gone
   afterwards, and nothing wearing the real name.
   ```bash
   ls -l /tmp/*.sql /tmp/*.part 2>/dev/null
   ```
2. **Both databases, one file per database.** Then export again into the same
   folder: this is the overwrite confirmation, and it should **name**
   `export_probe.sql` and `poc.sql` rather than counting them.
3. **Schema only, then data only**, into two files — the pair should load in
   that order and produce what one combined file does.

### The round trip, by hand

The claim the whole stage rests on is that the script rebuilds the database.
`live_export_probe.rs` checks this for `export_probe`; here it is for whatever
you exported, against a scratch name:

```bash
F=/tmp/dumps/export_probe.sql               # whatever you saved
M="podman exec -i db-query-mysql mysql -uroot -pdevpassword"

# The script carries its own CREATE DATABASE + USE, so load it into an empty
# server and it lands where it says. To load it somewhere *else*, strip both.
sed -e '/^CREATE DATABASE/d' -e '/^USE /d' "$F" > /tmp/into-scratch.sql
$M -e "DROP DATABASE IF EXISTS scratch; CREATE DATABASE scratch"
$M scratch < /tmp/into-scratch.sql

# Same tables, same row counts, same checksums.
for db in export_probe scratch; do
  $M -N -e "SELECT TABLE_NAME FROM information_schema.TABLES
            WHERE TABLE_SCHEMA='$db' AND TABLE_TYPE='BASE TABLE' ORDER BY 1" > /tmp/$db.tables
  $M -N -e "$(sed "s/.*/CHECKSUM TABLE $db.&;/" /tmp/$db.tables)" | sed "s/^$db\./scratch./" > /tmp/$db.sums
done
diff /tmp/export_probe.tables /tmp/scratch.tables && echo "same tables"
diff /tmp/export_probe.sums /tmp/scratch.sums && echo "same checksums"
```

**Checking that `USE` travels with `CREATE DATABASE` is the point of the
`sed`.** An early build wrote `USE \`export_probe\`` unconditionally, so
loading the script into an empty database left it empty and quietly overwrote
the original instead — a restore that was really an overwrite. If you skip the
`sed` and the script still touches `export_probe`, that bug is back. The only
`export_probe` left in a stripped script should be in its two header comments;
`grep -an export_probe` says.

Run against a `mysqldump --databases export_probe` on 2026-10-14 as a control:
same five tables, same five checksums. Note that `grep` without `-a` says
*nothing* about that file — mysqldump writes the fixture's blob bytes raw, so
it is "binary" and grep goes quiet rather than reporting zero matches. Our own
scripts stay text, because `sqlgen::binary_literal` writes hex.

## Checking that preferences survive being run a different way

`localStorage` belongs to the **webview's origin**, and this app has two of
them — `http://localhost:1420` under `mise run dev`, `tauri://localhost`
installed. That is why preferences live in `preferences.json` in the app config
directory now, with `localStorage` as the cache; `src-tauri/src/prefs.rs` has
the measurement. No automated test can cover it, because it needs two
different webviews, so here is the check by hand:

```bash
ls ~/.config/com.dbquery.poc/                      # preferences.json, shared
ls ~/.local/share/com.dbquery.poc/localstorage/    # one file per origin
```

To read what an origin's cache actually holds (it is SQLite, and the app holds
a write-ahead log, so copy all three files):

```bash
cd "$(mktemp -d)"
for f in ~/.local/share/com.dbquery.poc/localstorage/tauri_localhost_0.localstorage*; do
  cp "$f" "ls.sqlite${f##*.localstorage}"
done
python3 -c "
import sqlite3
for k, v in sqlite3.connect('ls.sqlite').execute('select key, value from ItemTable'):
    print(k, '=', v if isinstance(v, str) else bytes(v).decode('utf-16-le'))
"
```

The property to check: change a setting in one build, start the other, and it
is there. `preferences.json` should hold the same object, and the other
origin's cache should have been overwritten with it at boot.

### Without touching anything real

Both paths resolve from XDG, so the packaged build can be run against a
scratch root — `app_config_dir` is `$XDG_CONFIG_HOME/com.dbquery.poc` and the
webview's storage is `$XDG_DATA_HOME/com.dbquery.poc`:

```bash
ISO=$(mktemp -d); mkdir -p "$ISO/config/com.dbquery.poc" "$ISO/data"
printf '{"version":1,"settings":{"fontSize":19,"theme":"dark"}}'   > "$ISO/config/com.dbquery.poc/preferences.json"
XDG_CONFIG_HOME="$ISO/config" XDG_DATA_HOME="$ISO/data"   ./src-tauri/target/release/db-query
```

Close it and read `$ISO/data/com.dbquery.poc/localstorage/` with the snippet
above: `db-query.settings` should hold what was planted, and `db-query.theme`
should say `dark` — that second key is written by `applySettings`, so it is
also the proof that `main.ts` was reached at all, which is what the dev server
cannot tell you about the custom protocol and the CSP.

Delete `preferences.json`, launch again, and it comes back `0600` with the same
settings: that is the one-time hand-up from the cache.
