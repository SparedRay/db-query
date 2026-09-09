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

**An enum, not a trait.** There are exactly two backends and there is no plan
for a third; an `enum Backend { MySql(..), Elastic(..) }` with match dispatch
needs no `async_trait` dependency, no boxing, and no object-safety contortions
around async methods. If a third engine ever arrives and the matches get
tiresome, converting an enum to a trait is mechanical — converting a premature
trait back is not. (The same reasoning that made the assistant wait for its
second provider before abstracting; see Stage 10 §8.)

```
ServerConn                  ->  ServerConn { profile, backend: Backend, .. }
Backend::MySql(MySqlServer)     meta/killer connections, schema cache
Backend::Elastic(EsServer)      base URL, auth, http client, catalog
```

`TabSession.exec: Mutex<Option<MySqlConnection>>` becomes backend-specific too —
**Elasticsearch needs no per-tab connection at all**, which deletes the lazy
connect, the connection count and the `KILL` machinery for that side rather than
reimplementing them.

---

## 4. Decisions to make before writing code

- **D1 — Refuse what the engine cannot do, before sending it.** `classify()`
  already sorts statements into Select / RowReturning / Modify / Session / Other.
  For Elasticsearch, anything but Select and RowReturning is rejected with
  *"Elasticsearch SQL is read-only — SELECT, SHOW and DESCRIBE."* Better than
  forwarding an `UPDATE` and relaying a parser error, and it is a fact from the
  API reference rather than a guess.
- **D2 — `fetch_size`, not auto-LIMIT.** Auto-LIMIT rewrites the user's SQL,
  which Stage 9 established we must not record as theirs. Elasticsearch has a
  first-class row cap, so use it: `fetch_size` = our row budget, take the first
  page, and if a `cursor` comes back set the **existing** `truncated` flag the
  grid already renders. Then **close the cursor** (`POST /_sql/close`) rather
  than leaking server-side state — the one piece of cleanup this engine needs
  that MySQL does not.
- **D3 — Catalogs are the databases.** Elasticsearch has no schemas, but it has
  catalogs, a `SHOW CATALOGS` command and a `catalog` request field. Mapping
  them onto the existing database concept is honest, and `use_database` becomes
  "set this tab's catalog". The alternative — inventing a fake database name —
  would put a lie in the tree.
- **D4 — Indices are tables; there are no routines.** `SHOW TABLES` gives name
  and type, `DESCRIBE` gives columns. The tree already **omits empty groups**,
  so Procedures and Functions simply will not render. No frontend change.
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

### Phase 1 — The seam
- [ ] `Backend` enum; move MySQL specifics behind `Backend::MySql`
- [ ] `ConnProfile.kind` + `url`, defaulted; connection dialog grows the Elasticsearch shape
- [ ] Route `connect` / `disconnect` / `tab_status` / `use_database` through the enum, with the existing MySQL tests still passing at every step

### Phase 2 — The engine
- [ ] `src-tauri/src/elastic.rs`: `POST /_sql`, `fetch_size`, cursor detection, `/_sql/close`, auth shapes, error mapping
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

- Not a general "any database" abstraction. Two engines, two enum variants, and
  a third one is a decision to take when there is a third — not now.
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
