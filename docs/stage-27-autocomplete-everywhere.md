# Stage 27 — autocomplete that knows the whole server

**Status: done · built 2026-10-27**

## 1. The complaint

> *"Lets say for example I have selected a document but I'm writing a query
> with a fully qualified name, then it does not auto complete with tables for
> the query cursor is writing."*

Reported as "other databases don't complete". Measured, it was worse: **any**
qualified name completed nothing, including a qualifier naming the database
already selected.

## 2. What was measured

### 2.1 What the editor offered, before

Probed through the real editor, fixture database `poc` selected, cursor at `│`:

| Typed | Offered |
| --- | --- |
| `FROM │` | 4 tables, then keywords ✓ |
| `FROM poc.│` | **nothing** |
| `FROM other.│` | nothing |
| `FROM poc.orders.│` | **nothing** |
| `SELECT o.│ FROM poc.orders o` | **nothing** |
| `SELECT o.│ FROM orders o` | 3 columns ✓ |
| `SELECT │ FROM orders o` | columns above tables ✓ |

The cause, read from source (`node_modules/@codemirror/lang-sql/dist/index.js`,
`completeFromSchema`, lang-sql 6.10.0, 2026-10-27): the completer walks the
dotted path before the cursor through a tree of levels. The namespace it was
given was **flat** — `{table: [columns]}` — so no level named `poc` existed,
and with no `defaultSchema` configured there was no fallback either. An alias
resolves to the *full dotted path* of what it names, which is why
`FROM poc.orders o` killed `o.` as well.

### 2.2 What introspection costs

The question that decided the design was load, not privacy. A synthetic server
in the podman fixture — **12 databases, 1,920 tables, 26,880 columns**, MySQL 8
— best of 3, minus a 69ms process+connect baseline:

| Read | Rows | Cost |
| --- | --- | --- |
| Database names | 12 | ~9ms |
| Table names, one database | 160 | ~4ms |
| All columns, one database | 2,240 | ~22ms |
| Table names, whole server | 1,920 | ~60ms |
| All columns, whole server | 26,880 | ~140ms |

Linear and gentle: 1 db 37ms → 4 dbs 66ms → 12 dbs 125ms, about **3.5µs per
column** plus ~30ms fixed. Extrapolated, 100k columns ≈ 0.4s. Payload 1.22MB
of metadata, 776KB of names.

So two queries describe that whole server in ~200ms. The twenty-second figure
that shaped the old design was sixty **per-table** round trips; one bulk query
is a different animal, and that difference is now a capability
([`Capabilities::bulk_columns`]).

Caveats kept in mind: MySQL 8's data dictionary is what makes this fast (5.7 is
far slower); the queries select only names and types, because asking for
`table_rows` or `data_length` forces per-table statistics; and an idle fixture
flatters a loaded server. Hence per-database steps rather than one server-wide
query — see §3.

### 2.3 What a big namespace costs the browser

26,880 **distinct** column names pushed into the editor: the popup appeared in
170ms against 184ms for a 2,240-column namespace, and the JS heap was identical
at 22MB. No scaling cost — both figures are dominated by the test harness's own
keystroke latency. The namespace can hold a whole server without the editor
noticing.

## 3. Decisions

1. **The namespace is nested by database**, `{database: {table: [columns]}}`,
   with `defaultSchema` set to the tab's database. That pair is what makes
   `FROM orders` and `FROM poc.orders` both work from one namespace — lang-sql
   falls back to `defaultSchema` when the first segment is not a level.
2. **Every database is prefetched, in the background, one at a time.** The
   question is not what the server costs in total but how long a click can be
   made to wait for it: the meta connection is shared with the tree, the linter
   and the assistant, so each step holds it for one database (~26ms measured)
   and releases it before the next. A single server-wide query would be fewer
   round trips and would hold that connection for all of them at once.
3. **The engine decides what is prefetched, not whether.** With a bulk column
   query, a database's columns come too; without one, table *names* only, and
   columns wait until something refers to them. On Elasticsearch a mapping is
   fetched per index, so describing every catalog up front would be hundreds of
   requests nobody asked for.
4. **Nothing waits for it.** The current database is loaded first and awaited,
   because every unqualified name in the buffer refers to it; the rest is fired
   and forgotten. A connection change cancels it by generation, not by a flag.
5. **No cache on disk.** Measured, a live read of a big server is ~200ms, so a
   file would optimise something that is not slow — and it buys a new failure
   mode: a migration adds a column and autocomplete confidently offers last
   week's schema. It would also write table and column names to disk, which is
   a category of stored data this app does not keep. Reconnecting re-reads,
   which is the refresh the report asked for.
6. **The bare-column fallback stays in the current database.** The namespace
   holds the whole server now, and a statement naming no table is almost always
   about the database in front of you; every column on the fixture server would
   be 26,880 options.
7. **A catch-up path, for the window before the prefetch arrives.** Typing
   `name.` where the namespace has nothing under `name` reports the name; if it
   is a database on this connection, it is fetched on the spot. The keystroke
   that asked is too early — the next one has it. It is also the only path on an
   engine whose columns are not prefetched.
8. **`tableColumns` is keyed `database\0table`.** Two databases with a `users`
   table shared one column list before, and whichever loaded second won.
9. **Namespace keys are escaped.** lang-sql splits a key on unescaped dots, so
   a database named `sales.eu` would have become `sales` holding `eu`.

## 4. Milestones

- **A1** A qualified name completes: the selected database's own name, another
  database, their tables, and their columns.
- **A2** An alias of a qualified table completes its columns.
- **A3** Database names complete where a table name goes.
- **A4** Every database on the connection is described without anything in the
  app waiting for it, one database at a time.
- **A5** An engine with no bulk column query is prefetched by name only.
- **A6** A database referred to before the prefetch reaches it is fetched then.
- **A7** Unqualified completion, and the column fallback, are unchanged.

## 5. Task tracker

### Built 2026-10-27
- [x] `Capabilities::bulk_columns`, true for MySQL, false for Elasticsearch · A5
- [x] `setSchema(view, schema, defaultSchema)` and a namespace nested by
      database; `tableColumns` keyed per database; keys escaped · A1, A8, A9
- [x] `tableRef()` — an unqualified name means the tab's database, a qualified
      one means what it says · A1, A2
- [x] `pushSchema()`, `dbSlot()`, and the tree's two writes moved onto them
- [x] `loadCompletionSchema` keyed by **connection**, so moving between
      databases keeps what is already loaded
- [x] `prefetchSchemas()` — one database at a time, cancelled by generation · A4
- [x] `setSchemaLoader()` and the catch-up load · A6
- [x] 12 UI tests in `tests/ui/completion-qualified.spec.ts`, six falsified one
      at a time · A1–A7

**The test found a duplicate request.** "No database is asked for twice" failed
the first time it ran: on connect, the current-database load and the prefetch
both asked for the same database before either answered. `loaded()` asks what
has *arrived*, which is not the same question as what is in flight — so there
is a `loadsInFlight` set now. Without it the catch-up path is worse than
redundant: it fires on a keystroke, so every character typed after `other.`
would ask again while the first answer was still coming.

**A third falsification that passed, and the same lesson again.** The test for
decision 6 — that the fallback does not offer another database's columns —
passed with the scoping removed. The helper read each row's `innerText`, and a
row is its label and detail chip run together with no separator: `carrier` in
`shipments` reads as `carriershipments`, so `toContain("carrier")` could never
match either way. It reads `.cm-completionLabel` now, and the falsification
bites. All three of this project's passing falsifications have been the same
class: an assertion true for a reason other than the one claimed.
