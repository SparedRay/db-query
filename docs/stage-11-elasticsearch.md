# Stage 11 — A second engine: Elasticsearch SQL

**Goal:** run SQL against an Elasticsearch cluster from the same editor, with the
same tabs, grid, export and history.
**Builds on:** [Stage 10 — An assistant that writes SQL](stage-10-assistant.md).
**Status:** 🚧 Built 2026-09-08 — the seam, the engine, and both suites green
against a real cluster. Not yet clicked through by hand.

---

## 1. Why this is the right second engine

> *"the 2nd kind of query I constantly use is to elastic search index. they also
> support SQL like query and will prove our abstraction so we can query
> different kind of DB"*

Stage 0 drew the boundary at MySQL and every stage since has held it. This is
the first request to cross it, and Elasticsearch is a good place to cross:
**it is different enough to be a real test and similar enough to be affordable.**
It speaks SQL, returns rows and columns, and has no client library to adopt —
just HTTP, which we already do.

It is also honestly *narrow*, which the plan has to respect rather than paper
over. Verified against the API reference on 2026-09-08:

| | MySQL | Elasticsearch SQL |
|---|---|---|
| Transport | MySQL protocol (`sqlx`) | `POST /_sql?format=json` |
| Statements | anything | **`SELECT`, `SHOW`, `DESCRIBE` only** |
| Writes | yes | **none** — no DDL, no INSERT/UPDATE/DELETE |
| Transactions | yes | none |
| Multi-statement script | one connection, sequential | one query per request |
| Row limiting | `LIMIT`, our auto-LIMIT | `fetch_size` (default 1000) + `cursor` |
| Cancellation | second connection + `KILL` | abort the HTTP request |
| Introspection | `information_schema` | `SHOW TABLES`, `SHOW COLUMNS`, `DESCRIBE` |
| Namespaces | databases | **catalogs** (`SHOW CATALOGS`, `catalog` request field) |
| Per-tab session | a real connection | none — HTTP is stateless |

---

## 2. How much of this codebase is actually MySQL?

Measured rather than assumed — `grep -c 'MySql|mysql|sqlx'` per module:

| Module | Lines | Refs | Verdict |
|---|---|---|---|
| `session.rs` | 633 | 23 | **structural** |
| `decode.rs` | 268 | 13 | **structural** |
| `exec.rs` | 626 | 11 | **structural** |
| `schema.rs` | 441 | 8 | **structural** |
| `export.rs` | 895 | 8 | one streaming path only |
| `files.rs` | 463 | 9 | cosmetic — the string `"mysql"` |
| `sqlgen.rs`, `split.rs` | 1687 | 2 | comments |
| **`lib.rs`** | **1190** | **0** | **the whole command surface is already neutral** |
| everything else | ~3000 | 0 | neutral |

**Two findings decide the shape of this stage.**

1. **`lib.rs` has zero MySQL references.** Every Tauri command already speaks in
   connection ids, tab ids and `ScriptResult` — so the seam can go entirely
   behind the command layer, and the frontend needs no idea a second engine
   exists.
2. **The result types are already backend-neutral.** `CellValue` is
   `Null|Int|Float|Bool|Text` — a JSON shape, not a MySQL one — and `ColumnMeta`
   is a name, a type hint and a raw type string. Elasticsearch returns
   `columns[].name` and `columns[].type`, which maps **directly**. The grid,
   export, history, clipboard and assistant work unchanged.

So the work is concentrated in four files, and nothing above them has to move.

---

## 3. The seam

> *"even when no further engines are planned we could still need more
> integrations in future like Snowflake or other DBs so lets not block
> ourselves"*

**This reverses the first draft of this section**, which recommended an
`enum Backend` with two variants on the grounds that there was no third engine
planned. That reasoning was answered directly: a third *is* plausible, so the
design target is N, not 2.

### What actually blocks a third engine — and it is not the dispatch

The dispatch mechanism is the least important part of this. What decides whether
engine #3 is a weekend or a rewrite is:

1. **Whether the interface is defined by capabilities or by MySQL's habits.**
2. **Whether engine differences live in data or in scattered conditionals.**

The second is the one that bites. Write `if backend.is_elastic() { refuse writes }`
in `exec.rs` and Snowflake needs a new conditional *there*, and in the tree, and
in the assistant, and in export. Write `if !caps.writes { refuse }` and Snowflake
declares `writes: true` and none of those files are touched.

So the plan carries **a capability descriptor** as a first-class thing:

```
Capabilities {
    writes, transactions, multi_statement, delimiter_blocks,
    routines, cancellation, streaming_export,
    row_cap: RowCap::{ClientLimit, ServerFetchSize},
    namespace_label: "database" | "catalog" | "schema",
}
```

It is returned with `ConnInfo`, so **the frontend consults it too** — an index
should not offer "Drop table…", and a cluster should not offer `BEGIN`. Today
those menus are unconditional because there has only ever been one engine.

### A trait, with capabilities as data

With N ≥ 3 expected, each engine should be **one file implementing one
interface**, so that adding one is an addition rather than a hunt for every
match arm.

```
trait Engine {
    fn capabilities(&self) -> &Capabilities;
    async fn connect(...) -> Result<ConnInfo>;
    async fn namespaces(...) -> Result<Vec<String>>;
    async fn tables(...) / columns(...) / routines(...)
    async fn run(&self, stmt: &Statement, budget: RowBudget, cancel: &CancelToken)
        -> Result<Outcome>;
    async fn cancel(...);
}
```

`Outcome`, `CellValue` and `ColumnMeta` are already engine-neutral (§2), so the
trait returns the types the app already speaks.

**Cost of dynamic dispatch, measured rather than assumed:** `dyn Trait` with
async methods is not object-safe natively, so it needs `async_trait`.
`async-trait 0.1.92` is **already in the Linux tree** via
`keyring -> secret-service -> zbus`, and **absent from the Windows tree** — so
the true cost is one crate on Windows. It is `MIT OR Apache-2.0`, it is a
**proc macro**, so it runs at build time and contributes **zero bytes to the
binary**; it appears in the attribution manifest as one more build-time entry
the file already documents as an over-listed superset. That is a small enough
price that it should not have been the deciding argument in the first place.

### Two families, not N unrelated engines

The engines worth anticipating fall into two shapes, and noticing this changes
how Phase 2 is written:

| Family | Transport | Examples |
|---|---|---|
| Wire protocol | `sqlx` | MySQL, PostgreSQL, MariaDB |
| HTTP + JSON SQL API | `reqwest` | **Elasticsearch**, **Snowflake** (`POST /api/v2/statements`), Databricks, BigQuery |

Snowflake's REST API has the same shape as Elasticsearch's: post a statement,
receive columns and rows and a pagination handle. So `elastic.rs` should be
**a thin engine over a shared "HTTP SQL API" helper** — request, poll or page,
map columns, map errors — rather than a bespoke module. That helper is the part
Snowflake would reuse, and building it now costs almost nothing while
retrofitting it later costs the whole file.

### The order that keeps this honest

`ServerConn` becomes `{ profile, engine: Box<dyn Engine>, .. }`, and
`TabSession.exec: Mutex<Option<MySqlConnection>>` moves behind the engine —
Elasticsearch needs no per-tab connection at all, which *deletes* the lazy
connect and the `KILL` machinery for that side rather than reimplementing it.

**MySQL is ported to the trait first, with no second engine present.** If the
existing suite still passes against a `MysqlEngine` behind `dyn Engine`, the
seam is real; if it needed changes to accommodate Elasticsearch, the seam was
shaped by the second engine and would be shaped again by the third.

---

## 4. Decisions to make before writing code

- **D1 — Refuse what the engine cannot do, before sending it — from
  capabilities, never from the engine's name.** `classify()` already sorts
  statements into Select / RowReturning / Modify / Session / Other; the refusal
  reads `caps.writes` and `caps.transactions`, so a future engine that allows
  writes needs no change here. The message names the engine and what it does
  support. Better than forwarding an `UPDATE` and relaying a parser error, and
  read-only is a fact from the API reference rather than a guess.
- **D2 — `fetch_size`, not auto-LIMIT.** Auto-LIMIT rewrites the user's SQL,
  which Stage 9 established we must not record as theirs. Elasticsearch has a
  first-class row cap, so use it: `fetch_size` = our row budget, take the first
  page, and if a `cursor` comes back set the **existing** `truncated` flag the
  grid already renders. Then **close the cursor** (`POST /_sql/close`) rather
  than leaking server-side state — the one piece of cleanup this engine needs
  that MySQL does not.
- **D3 — Catalogs are the databases, and the label is data.** `namespace_label`
  comes from the capabilities so the UI can say "catalog" where that is the true
  word — Snowflake would say "schema" — instead of every engine borrowing
  MySQL's vocabulary.
  Elasticsearch has no schemas, but it has catalogs, a `SHOW CATALOGS` command
  and a `catalog` request field. Mapping them onto the existing namespace concept
  is honest, and `use_database` becomes "set this tab's catalog". The
  alternative — inventing a fake database name — would put a lie in the tree.
- **D4 — Indices are tables; there are no routines.** `SHOW TABLES` gives name
  and type, `DESCRIBE` gives columns. The tree already **omits empty groups**,
  so Procedures and Functions will not render — but the *context menus* are
  unconditional today and must start reading capabilities, or an index will
  offer "Drop table…".
- **D5 — Cancellation is dropping the request.** No second connection, no
  `KILL`. The tab's existing `cancel_requested` flag gates it; the work is a
  cancellation token the HTTP call selects on.
- **D6 — A multi-statement script is N requests.** Elasticsearch takes one query
  per call, so `Run all` over three SELECTs is three round trips, executed
  sequentially and reported per statement exactly as now. `split.rs` needs no
  change; `DELIMITER` is meaningless here and should be refused rather than
  ignored.
- **D7 — The profile grows a `kind`, and the file stays versioned.**
  `ConnProfile` gains `kind: "mysql" | "elasticsearch"` plus a `url` used only by
  the latter, both `#[serde(default)]` so existing `connections.json` files load
  untouched — the same backward-compatible move `workspace.rs` relies on.
  Elasticsearch auth is **basic, API key, or none**, which is a third shape the
  connection dialog has to grow; the secret still goes to the keychain.
- **D8 — The assistant needs a dialect.** Its system prompt says "a MySQL
  client" and would confidently write `UPDATE`. It has to be told which engine
  it is writing for and what that engine refuses.
- **D9 — Streaming export stays MySQL-only at first.** `export.rs` has a
  row-at-a-time CSV path typed on `MySqlRow`; Elasticsearch results are already
  in memory, so they go through the existing in-memory path. Worth saying out
  loud rather than discovering at the Export button.
- **D10 — No new dependency, and no licence question.** We speak HTTP with
  `reqwest`, which the updater already linked and the assistant already uses.
  **We link no Elastic code**, so Elasticsearch's SSPL/Elastic Licence does not
  reach this binary. (OpenSearch's SQL plugin is Apache-2.0 but lives at
  `_plugins/_sql` with a different response shape — a sibling of this work, not
  part of it.)

---

## 5. Milestones

- [x] **E1 — A cluster can be saved and connected to**, with basic auth, an API key, or neither.
- [x] **E2 — Indices appear in the tree**, with their columns, under the right catalog.
- [x] **E3 — A `SELECT` runs and renders** in the ordinary grid, with the ordinary types.
- [x] **E4 — More rows than the cap sets `truncated`**, and the cursor is closed rather than left open.
- [x] **E5 — A write is refused before it is sent**, naming what the engine supports.
- [ ] **E6 — A running query can be cancelled.**
- [x] **E7 — Everything above it works untouched**: export, copy, history (including provenance), the value viewer, tab persistence.
- [x] **E8 — The assistant writes Elasticsearch SQL** when the tab is on a cluster, and does not offer writes.
- [x] **E9 — Nothing regressed for MySQL.** 240 unit, 63 live and 412 UI tests, unchanged.

---

## 6. Task tracker

### Phase 1 — The seam, proved on MySQL alone
- [x] **`Capabilities`, returned with `ConnInfo`** — `src-tauri/src/engine.rs`. MySQL declares everything it already does, written out in full rather than derived from a `Default`, so adding a capability forces a decision
- [x] **`engine::refusal`** — statements are refused from `caps.writes` / `caps.transactions`, before being sent, never from an engine name
- [x] **The tree menus read capabilities.** `DROP` on a table, a column and a routine is offered only where the engine accepts writes; the reads beside them stay
- [x] **The rail names the engine the server reported**, rather than the hardcoded "MySQL" it had said since Stage 2
- [x] **Gate passed:** the whole existing suite ran unchanged before the new tests were added — 203 UI, 217 unit, 63 live
- [x] **`trait Engine`; `MysqlEngine` implements it; `ServerConn` holds `Box<dyn Engine>`.** Deliberately thin: the MySQL queries and caching did **not** move, they were renamed to `mysql_*` and the trait points at them. The seam was added; nothing behind it was rewritten
- [x] **`ConnProfile.kind` + `url` + `auth`, all `#[serde(default)]`** — every `connections.json` written before Stage 11 loads unchanged and means MySQL
- [x] Connection dialog grows an engine picker, a URL field and three auth shapes; host/port/user and the TLS toggle hide for a cluster
- [x] **Gate passed twice**: 63 live MySQL tests and the whole UI suite ran unchanged after the trait refactor, before any Elasticsearch code was reachable

#### Decisions taken while building

- **Unknown capabilities mean "allowed".** A connection that has never connected
  has no capabilities, and that is the one state where no menu is reachable
  anyway — hiding items there would make a reconnect look like a lost feature.
- **Capabilities come from the server, not the profile.** They are set on
  connect and cleared on disconnect, so a `connections.json` copied between
  machines cannot claim capabilities the server does not have.
- **`engine` is a display field.** It names the engine for the rail and, later,
  for the assistant's dialect. Every behavioural branch reads a flag instead —
  a name check has to be revisited for each new engine, and one will be missed.

### Phase 2 — The engine — built 2026-09-08
- [x] **`src-tauri/src/httpsql.rs`** — the shared half: auth shapes, JSON scalars into `CellValue`, `{name,type}` into `ColumnMeta`, error extraction. **10 unit tests.** This is the part Snowflake reuses
- [x] **`src-tauri/src/elastic.rs`** — `POST /_sql?format=json`, `fetch_size`, cursor detection, `/_sql/close`, `SHOW CATALOGS` / `SHOW TABLES` / `DESCRIBE`. **13 unit tests** against recorded payloads
- [x] Read-only refusal via capabilities (D1); cancellation between statements (D5)
- [x] **10 live tests against a real 8.15 cluster**, including three that go through the app's own `connect` / `open_tab` / `run_script`

### Phase 3 — Fitting in — built 2026-09-08
- [x] **Assistant dialect (D8).** `system_prompt` takes the capabilities: a read-only engine is told so explicitly, told it has no routines, and told what its namespaces are called. MySQL gains no restrictions it did not have
- [x] `mise run es-up` / `es-down` / `test-es` — a container fixture and a seeded index, mirroring `db-up`
- [x] **CI runs both engines on every push** — an Elasticsearch service container beside the MySQL one, because the risk of this stage is breaking MySQL quietly
- [x] Lint rules audited for MySQL-only assumptions — done in Stage 12 §15: `lint::Dialect` is built from the engine, so an identifier quoted the standard way is read rather than masked away
- [x] `dialect` on new tabs follows the connection (Stage 12)
- [ ] Export path check (D9) — still no Elasticsearch export test

---

## 7. What this stage is *not*

- Not every engine at once. The seam is built for N and the *second* one is
  built now; Snowflake and PostgreSQL are the cases it is shaped to accept, not
  cases it delivers.
- Not OpenSearch. Similar, different endpoint and response shape.
- Not writes to Elasticsearch through some other API. The engine's SQL surface
  is read-only and this follows it.
- Not Elasticsearch's own query DSL. That is a different editor and a different
  language; this stage is about the SQL we already have a home for.

---

## 8. Risks

- **The abstraction is the deliverable, and it is invisible.** Every milestone
  except E9 is about the new engine, but the way this goes wrong is by breaking
  MySQL quietly. The existing suite runs at every step, not at the end.
- **`session.rs` is the oldest and most load-bearing file here**, and this
  reaches straight into it. It has 23 MySQL references and every connection
  behaviour the app has.
- **Elasticsearch SQL is narrower than it looks.** Its `SELECT` support has real
  gaps (notably around joins), so a query that reads like ordinary SQL can be
  rejected by the server. Error mapping matters more here than for MySQL — the
  parser's message is the only guidance the user gets.
- **A second engine doubles the hands-on surface** for every future stage, and
  the release loop (Stage 8) has still never been exercised end to end.
- **A seam designed for three engines and tested against two can still be
  wrong.** The mitigation is the Phase 1 gate — port MySQL first and require the
  existing suite to pass untouched — plus writing the HTTP helper as if
  Snowflake were next, because on this plan it is.

---

## 9. What implementation changed about the plan — 2026-09-08

### A latent crash the plan never anticipated

A unit test in `httpsql` — not the app — found that **`reqwest::Client::new()`
panics**. `reqwest` is built with `rustls-no-provider` (chosen so `aws-lc-rs`,
which needs cmake and NASM, stays out of Windows CI), and that feature makes the
caller responsible for installing a crypto provider. **Nothing did.**

So the assistant shipped in Stage 10 would have panicked on its first request on
Linux, where `update_check` returns early and never builds a client to install
one incidentally. `install_tls()` now runs before anything else in `run()`.

Two lessons worth keeping: a feature flag chosen for the *build* had a runtime
obligation attached to it, and the bug was found by a test for a different
module in a different stage.

### The trait was cheaper than expected, because nothing moved

`MysqlEngine` is a dispatch vtable. The `information_schema` queries, the
caching, the exec loop — none of it moved; the functions were renamed `mysql_*`
and the trait points at them. That is why 63 live tests passed through the
refactor without an edit, which was the gate the plan set.

`ServerConn.meta` and `.killer` became `Option`, since an HTTP engine holds
nothing open. Reaching them returns a `Result` rather than panicking: it means a
MySQL-only path was called for another engine, which is a bug worth a message
rather than a crash in someone's session.

### Decisions taken while building

- **`SHOW CATALOGS` failing is not a failed connection.** Cross-cluster search
  may simply be off, so it yields an empty namespace list and the tree shows
  indices without one.
- **There is no `SHOW CREATE` for an index**, so "Examine definition" renders
  `DESCRIBE` as a comment block plus the statement itself. Inventing a
  `CREATE TABLE` this engine could never run would be worse than saying what is
  actually known.
- **`SHOW TABLES` column names differ between versions** ("name"/"table",
  "type"/"kind"), so both are accepted. Pinning one is the kind of failure that
  only appears on someone else's cluster.
- **An alias reads as a view.** Readable, not a base table — and the tree
  already groups on that word.
- **Nested documents keep their JSON** rather than becoming "[object]". The cell
  viewer from Stage 6 can already show them.
- **Cancellation stops the script between statements.** There is no server-side
  kill, so a single long query cannot be interrupted — stated here because the
  capability says `cancellation: true` and that is the honest extent of it.

---

## 10. What is still open

- ~~**Nobody has clicked through it.**~~ Done 2026-09-09 — see §11, which is
  what that review found.
- **Lint is still MySQL-flavoured** (§6 Phase 3). Schema-aware checks work
  because they read the tree; dialect-specific rules have not been audited.
- **A new tab's `dialect` still says "mysql"** regardless of its connection,
  which affects syntax highlighting rather than correctness.
- **Export (D9)** goes through the in-memory path for a cluster, which is right,
  but the streaming path's absence has not been surfaced in the UI.

---

## 11. First hands-on review — 2026-09-09

The first time a human clicked through it. Three reports, all three real, and
one more found while fixing them. Corrections live here rather than in the
stages that introduced them, per the rule about frozen trackers — the editor
keymap belongs to Stage 1 and the assistant schema to
[Stage 10](stage-10-assistant.md).

### The browse snippet was MySQL-only

Double-clicking an index generated ``SELECT * FROM `catalog`.`orders` ``:
backtick quoting the cluster rejects, and a catalog prefix that is not a schema.

`generate_select` was a **pure function that never saw the connection** — it
could not have been right for two engines. It now goes through the engine, and
the trait gained the three dialect primitives that decision needs:
`quote_ident`, `qualify` (defaulted to `ns.table`) and `select_snippet`
(defaulted to `SELECT * … LIMIT n`). Elasticsearch overrides `qualify` to drop
the prefix entirely; MySQL delegates to the existing `sqlgen` generator so the
live tests keep covering the path the app actually calls.

This is the seam earning its keep: a Snowflake engine gets a correct browse
snippet by writing one method, and gets `LIMIT` wrong loudly rather than
quietly, because the default is the thing it would override.

**A unit test pins the string; a live test runs it against the cluster.** Only
the second one could have caught the original bug.

### The assistant was guessing at the schema

`assistant_send` read the schema **only from the cache**, which is populated by
expanding the tree. Ask a question on a fresh connection and the model was told
`(columns not loaded)` for every table — and a model told a table has no columns
invents some.

`schema::warm_for_assistant` now loads the shape before the prompt is built.

**This is not "running queries automatically".** It calls the engine's `tables`
and `columns` — the same introspection an expand does. No user SQL runs, nothing
is recorded in history, nothing reaches the grid, and the assistant still cannot
execute a statement. The standing rule is about *the user's* statements and the
model's inability to run them; both are intact.

Tradeoffs, taken deliberately:

| | |
|---|---|
| **Latency** | Columns are per table, so the first question on a database pays N round trips. Capped at `ASSISTANT_TABLE_BUDGET` (60) and cached, so it is paid once. |
| **Prompt size** | ~60 tables of columns is a few thousand tokens per question. Table *names* are never capped — one query for all of them, and knowing a table exists is most of the value. |
| **Truncation** | When the budget bites, the prompt says how many of how many were detailed rather than implying the rest are empty. |
| **Privacy** | Unchanged in kind, larger in degree: more table and column names leave the machine. Still names and types only — `render_schema` cannot see row data by construction — and the chat header already says so. A local model sends nothing anywhere. |

### The editor was missing most of a text editor

Ctrl+Z was reported as broken. It was not — undo, redo and per-tab history all
pass on both engines, and there are now tests saying so. What was missing was
everything around it: **`@codemirror/search` was never installed, so Ctrl+F did
nothing at all**, and Ctrl+Shift+Z *undid* rather than redoing, because
`historyKeymap` binds it only on platforms it recognises as Linux.

Added: `search`, `highlightSelectionMatches`, `drawSelection`, `dropCursor`,
`indentOnInput`, `highlightSpecialChars`, `rectangularSelection`,
`crosshairCursor`, multiple selections, and explicit unconditional bindings for
both redo spellings. The starter document now names the keys, because a
shortcut nobody can discover is a shortcut nobody has.

None of this is visible until someone reaches for the key, which is how it
survived eleven stages. `tests/ui/editor-keys.spec.ts` is the regression.

### The Elasticsearch fixture was not idempotent

Found by the live tests failing on `rows.len() == 3` after finding six. The
seed's `{"index":{}}` mints a fresh document id per run, so **every `es-up`
appended another copy of the fixture data**. `es-up` now deletes the index first
and gives each document an explicit `_id`; running it twice leaves three
documents. A fixture that is not idempotent lies the second time you use it.
