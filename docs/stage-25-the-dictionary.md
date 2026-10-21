# Stage 25 — A dictionary the assistant can read

Reported as a want, 2026-10-20:

> In order to make this more useful we need to be able to feed the agent with
> information. Initially I was looking to fill comments on table but then that
> feels limited to each driver. What if we evaluate the posibility to attach
> profiles to connections, this profile can be used to define tables and
> columns usage so we feed this as the basic info assistant need when building
> query for a db.

The assistant and the MCP server both know a database's **shape** — names,
types, nullability, keys — and nothing about what any of it *means*. A column
called `status` holding `P`, `S` and `X` is a guess waiting to happen, and
`legacy_customer_ref` looks exactly like a column you should join on.

---

## 1. Why not the database's own comments

The first idea, and the reason it was dropped is in the report: *"feels limited
to each driver"*. Four more reasons found while checking:

| | |
|---|---|
| **Per engine** | MySQL has `COMMENT`; Elasticsearch has nothing to put one on. |
| **A write** | Setting a comment is DDL. The account from Stage 11 §12 holds index privileges and could not. |
| **Does not version with the code** | A comment lives in the server. The query that depends on it lives in a repository, on a branch. |
| **Cannot say the useful things** | *"join orders to customers on `customer_id`, never on `legacy_customer_ref`"* is not a property of one column. |

They remain a good **one-way seed** — "import the comments to start the file
off" — and the app reads none today, so that is new engine work and comes last
if at all.

## 2. Measured before designing — 2026-10-20

### What notes cost the prompt

Rendered through the real `render_schema`, characters, with ~4 chars/token for
scale:

| Schema | Today | + a sentence per table | + a phrase on **every** column |
|---|---|---|---|
| 20 tables × 10 cols | ~2.1k tokens | +21% | **+131%** |
| 60 × 10 | ~6.2k tokens | +21% | **+131%** |
| 60 × 30 | ~17.7k tokens | +7% | **+122%** |
| 385 × 10 | ~8.8k tokens | +15% | +92% |

So: table notes are nearly free; annotating *every* column doubles the schema
block in every request. Real annotation is sparse — people write about `status`
and `flags`, not `created_at` — so the rule is **only annotated items cost
anything**, and there is a character budget beside the existing 60-table detail
limit.

### What the file costs to read

A 385-table file with sparse column notes is ~240KB and **reads and parses in
~18ms**. Free per prompt; not free per keystroke, so it is cached against the
file's size and mtime — Stage 24's lesson, applied from the start.

## 3. Decisions

### 3.1 The file never reaches a model

The thing that changed the design. The assistant sees what `render_schema`
emits; an MCP client sees JSON from `describe_table`. **Nothing hands the file
over**, so TOML vs YAML vs Markdown is a question about editing and diffing,
not about parsing quality.

TOML, then, for the reasons it was proposed: `toml` is already a dependency, it
takes comments, and it has exact keys to look up — which the tree marks and the
stale-note check both need. Markdown is nicer to write and worse to look up:
"which column is this note about" turns fuzzy, and that is the part the rest of
the feature stands on.

### 3.2 A note is rendered **beside the thing it describes**

Not as a glossary section after the schema:

```
TABLE `orders`  -- One row per order.
  -- join: orders.customer_id = customers.id
  (id bigint NOT NULL PK,
   status varchar(16) NOT NULL  -- P=pending, S=shipped, X=cancelled.,
   total_cents int NOT NULL)
```

A note printed next to its own column cannot be misattributed. A separate list
of definitions asks the model to join two lists by name, which is where this
kind of context goes wrong, and it costs the same tokens either way.

**This is judgement, not a measurement.** The Ollama fixture could compare both
renderings on the same questions; it has not been done, and this decision is
recorded as reasoning so that it can be overturned by evidence rather than by
preference.

### 3.3 A path, not a copy

The file lives where the schema it describes lives — in the repository, on the
branch, reviewed like anything else — and the connection holds its **path**.
Stage 17's rule for Flyway projects, for the same reasons: paths never
contents, re-read every time it is used.

That also makes reuse free rather than a copy: several connections point at one
file, so dev, UAT and production share one dictionary and an edit lands
everywhere at once.

### 3.4 Keyed by table, with a per-database override

Entries are keyed by table name alone, so one file serves environments whose
*database* names differ — which is the reported setup: `flyway_dev` and
`flyway_qa` hold the same schema. `[databases.<name>.tables.<table>]` overrides
`[tables.<table>]` for the one database that has genuinely diverged.

### 3.5 Case-insensitive lookup

`editor.ts` already learned this: *"`FROM Users` found nothing when the server
had said `users`, which is most of what 'it does not suggest columns' turned
out to mean"*. MySQL folds table names to lower case on Windows and macOS.
Keys are folded on both sides, from the start, rather than after a report.

### 3.6 Unknown keys are ignored

`flyway.rs`'s rule: *"Validating the file would reject a config that works"*. A
dictionary written for a later version of this app must still work in this one.
`version` is read and a *newer* one is warned about, never refused.

### 3.7 "Dictionary", not "profile"

The report says "profile", and the word is already taken: `ConnProfile`,
`profiles.rs`, `ProfileView`, `PROFILE_KEYS`. A second meaning would be
genuinely confusing in the code. It is a data dictionary, so: `dictionary` in
the code, **"AI dictionary"** in the UI.

### 3.8 What this changes about what leaves the machine

`assistant.rs` promises, in the module docs and in the UI, *"No row data,
ever"*. A dictionary is prose somebody wrote, and prose can carry example
values. The claim becomes "the shape of your schema, plus the dictionary you
attached", the chat header says so, and the same applies to MCP — whatever
agent is connected now gets these notes.

## 4. The UI, as agreed

Drawn before building: <https://claude.ai/artifact/QiqnQdoPMUBe3qN2UvY6gp>

- **The tree gains one dot per table and per column.** Filled when the
  dictionary says something about it; an empty slot when it does not, so a
  glance down the column of dots says what the assistant knows. A note that
  matches no table or column gets an amber warning instead — the one state that
  has to be findable.
- **The file opens as a tab**, in CodeMirror, with Ctrl+S, the mtime guard, the
  line endings and the BOM that `files.rs` already handles. The SQL controls —
  Run, Run all, Format, Auto-LIMIT, Lint, Timeout — are absent, exactly as on a
  migrations tab.
- **That tab carries the status**: which file, which connections share it, how
  many notes, how many match nothing. Next to the thing it describes rather
  than in the sidebar, so the schema tree keeps its full height.
- **The tree's context menu writes into the buffer**, never into the file
  behind the editor's back: "Describe for the assistant…" opens the tab and
  inserts the stub. No dirty-buffer clash to handle.
- **No session persistence** for dictionary tabs (asked and answered): they
  open when wanted.
- Rejected, drawn, and kept for later: a `Schema / Dictionary` switch in the
  sidebar. Better for *status*, and it puts the app's main navigation behind a
  tab for a panel nobody looks at twice an hour. Additive if the tab header
  leaves something missing.

## 5. Milestones

- **D1** A dictionary file is parsed, and a note is found for a table and for a
  column — whatever the case, in any database, with a per-database override
  winning.
- **D2** A file with keys this build does not know still loads.
- **D3** A note that matches no table or column is reported, by name.
- **D4** The assistant's prompt carries the notes, beside their own columns,
  inside the existing budget.
- **D5** `describe_table` and `list_tables` carry them, with no fifth tool.
- **D6** The tree marks tables and columns that have a note.
- **D7** The file opens in a tab, saves with Ctrl+S, and offers no Run.
- **D8** "Describe for the assistant…" inserts a stub in that tab.
- **D9** The chat header says the dictionary is being sent.

## 6. Task tracker

### Slice 1 — the file and the binding · built 2026-10-21
- [x] `dictionary.rs`: parse, resolve, case-folding, per-database override
- [x] unmatched notes, for the warning
- [x] a cache keyed by path, invalidated by size and mtime
- [x] `ConnProfile.dictionary`, and `PROFILE_KEYS` → 14
- [x] 18 unit tests · D1, D2, D3 met

Two things it decided that the plan had not:

**A blank stub is not a note.** `starter()` generates one commented-out section
per real table, so the work is filling blanks rather than typing 385 headers
and a typo cannot be the first thing that happens. But an entry somebody
uncomments and leaves empty must not count as described, or the tree fills with
dots that mean nothing — so `is_empty` is asked of the *content*, not of the
key, and `counts()` and the dots both go through it.

**"Not loaded" is not "not there".** `unmatched` takes a closure that answers
which columns a table has, and `None` means nobody has fetched them yet — in
which case that table's column notes are not reported. Reporting them would put
an amber warning on a perfectly good dictionary for every table the user had
not expanded.

Also found while wiring: the connection dialog **builds its profile from its
own form fields**, so any field without a control is dropped on save. A
connection whose dictionary vanished because somebody changed its port would be
a loss nobody could explain, so the path is carried from the profile being
edited and there is a test that fails if that line goes.

### Slice 2 — the assistant · built 2026-10-21
- [x] notes inline in `render_schema`, with their own budget
- [x] the prompt's own sentence about where they came from
- [x] the chat header (D9)
- [x] 7 unit tests, 2 UI tests · D4 and D9 met

**The real before and after**, against the fixture's `poc` (6 tables, 4 of them
described, one note deliberately typo'd):

```
TABLE `orders` -- One row per order. There is no order_lines table in this schema.
  -- joins: orders.user_id = users.id
  (id int NOT NULL PK,
   user_id int NOT NULL INDEX,
   total decimal(12,2) NOT NULL) -- Cents. Divide by 100 only when showing it to a person.
```

1,208 characters before, 1,664 after: **+37%** for four annotated tables out of
six, and the two nobody described are byte-for-byte what they were. The stale
check reported exactly one note, `oders`.

Three things the build decided:

**The budget decides the shape, not just the text.** A table renders on one
line unless something was *afforded* for it. The first version asked whether
the dictionary *had* anything — so a table whose note the budget dropped still
paid for the multi-line form and the indentation, which is the opposite of
proportional. Its own test caught it.

**Order of spending: what the table is, then how it joins, then its columns.**
A dictionary that runs out of budget therefore loses detail and keeps the
sentence that explains the table.

**`unmatched` asks one question at a time.** It took a list of column names
first — and the app's cache holds `ColumnInfo`, not `String`, so every caller
had to build a throwaway `Vec`. The first caller written (a probe, minutes
later) passed an empty slice, and every column note in a good dictionary came
back stale. A signature whose easiest wrong answer is "none of them" is the
wrong signature; it is `Fn(&str, &str) -> Option<bool>` now, where the wrong
answer has to be typed on purpose.

And one slip worth recording because it cost twenty minutes: the chat header's
promise was written out **twice**, once per provider branch, and the edit that
added "and the dictionary" was applied to one of them. It is one `whatIsSent()`
now — two copies of a promise is one copy that gets updated.

### Slice 3 — MCP
- [ ] `note` on the `Tables` and `Columns` results

### Slice 4 — the tab and the tree
- [ ] a third `TabKind`, its header band, no SQL controls
- [ ] TOML highlighting (`@codemirror/legacy-modes`) and attribution
- [ ] TOML parse errors as this tab's lint
- [ ] dots in the tree, the amber warning, the context-menu stub
