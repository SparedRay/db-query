# Stage 11 — A second engine: Elasticsearch SQL

**Goal:** run SQL against an Elasticsearch cluster from the same editor, with the
same tabs, grid, export and history.
**Builds on:** [Stage 10 — An assistant that writes SQL](stage-10-assistant.md).
**Status:** 📋 Planned — analysis only, nothing built.

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

- [ ] **E1 — A cluster can be saved and connected to**, with basic auth, an API key, or neither.
- [ ] **E2 — Indices appear in the tree**, with their columns, under the right catalog.
- [ ] **E3 — A `SELECT` runs and renders** in the ordinary grid, with the ordinary types.
- [ ] **E4 — More rows than the cap sets `truncated`**, and the cursor is closed rather than left open.
- [ ] **E5 — A write is refused before it is sent**, naming what the engine supports.
- [ ] **E6 — A running query can be cancelled.**
- [ ] **E7 — Everything above it works untouched**: export, copy, history (including provenance), the value viewer, tab persistence.
- [ ] **E8 — The assistant writes Elasticsearch SQL** when the tab is on a cluster, and does not offer writes.
- [ ] **E9 — Nothing regressed for MySQL.** The whole existing suite, unchanged.

---

## 6. Task tracker

### Phase 1 — The seam, proved on MySQL alone
- [ ] `Capabilities`, returned with `ConnInfo`; MySQL declares everything it already does
- [ ] `trait Engine`; `MysqlEngine` implements it; `ServerConn` holds `Box<dyn Engine>`
- [ ] Replace the unconditional menus and refusals with capability checks — **with no second engine present**, so the seam cannot be shaped by one
- [ ] `ConnProfile.kind` + `url`, defaulted; connection dialog grows a second shape
- [ ] **Gate:** the whole existing suite passes unchanged. If it needed edits, the seam is wrong

### Phase 2 — The engine
- [ ] A shared **HTTP SQL API** helper — request, page, map columns, map errors — the part Snowflake would reuse
- [ ] `src-tauri/src/elastic.rs` on top of it: `POST /_sql`, `fetch_size`, cursor detection, `/_sql/close`, auth shapes
- [ ] Map `columns[].type` onto `TypeHint`; decode JSON scalars into `CellValue`
- [ ] `SHOW TABLES` / `DESCRIBE` into the existing `TableRef` / `ColumnInfo`
- [ ] Read-only refusal (D1), cancellation token (D5)

### Phase 3 — Fitting in
- [ ] Assistant dialect (D8); lint rules audited for MySQL-only assumptions
- [ ] Export path check (D9); `dialect` on new tabs follows the connection
- [ ] `mise run es-up` — a container fixture and a seeded index, mirroring `db-up`
- [ ] Live tests against it; UI tests against a stubbed backend

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
