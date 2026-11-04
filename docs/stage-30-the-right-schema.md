# Stage 30 — the assistant reads the right schema

**Status:** built, hands-on outstanding
**Opened:** 2026-11-04
**Built:** 2026-11-04

## 1. The ask

From Alfredo Cabello's first trial, 2026-11-04: db-query on Windows against an
AWS QA MySQL server, with Claude Code in WSL2 driving the MCP server while the
built-in assistant ran in the app.

The server (`appdata`, ~400 tables and views) was asked one question two
ways. Through MCP, Claude Code wrote a
correct query whose joins matched the production report code. In the built-in
assistant it declined, saying:

> `enrolled_item`, `enrolled_party`, `supplier_status`, `party_status` and the
> other tables involved are among the 340 whose columns weren't shared, and I'd
> be guessing at names like the reserve balance, the original supplier and the
> servicing status. The schema also has no `--` notes to go by.

Separately: attaching a dictionary did not take effect until the app was
restarted — `put_notes` kept answering *"AWS QA" has no AI dictionary
attached*, and the connection menu's **AI dictionary…** appeared to do nothing
at all.

The question was: *for one member, list every enrolled item with the client
name, servicing status, supplier status, current and original supplier,
balances and reserve balance* — over `enrolled_item`, `enrolled_party`,
`parties`, `party_status`, `supplier_status`, `Supplier` and
`external_holding_account`. Through MCP the client called `list_tables`, then
`describe_table` per table, and read the dictionary notes; one note decided the
answer, that `enrolled_party.status_id` points at `party_status` and not at
the similarly named `statuses`.

Worth recording because it is why the `join` field earns its keep: they are
filling the dictionary **from their application code** — what each table is,
`join` expressions taken from the SQL the app actually runs, and notes on
confusing columns, starting with the core domains. That works through MCP
today, and this stage is what makes it reach the built-in assistant.

The report is in the trial's own words; this stage is the review of it against
the code, and what we do about it. Their five questions are answered in §2:
how tables are chosen (§2.1), whether the dictionary is read from the file or
the tab and whether notes reach tables sent without columns (§2.2), whether
per-question lookup is coming (§4.1), and whether the MCP token is stable
(§2.4).

## 2. What was actually true

Every claim checked out, and three of the five issues share one cause.

### 2.1 The prompt gets the alphabetically first 60 tables

Nothing is missing from the cache. `schema::warm` calls MySQL's bulk
`all_columns`, so **every column of all 400 tables is in memory** before the
first word of an answer. The loss is entirely in the rendering:
`assistant_system` (`lib.rs:518`) passes `schema::TABLE_DETAIL_BUDGET` — 60 — as
`render_schema`'s `detail_limit`, and `mysql_list_tables` is `ORDER BY
table_name` (`schema.rs:382`). So the prompt carries the alphabetically first
60 tables with columns and 340 names alone.

*Their question 1 — how does the assistant choose which tables get columns?*
Alphabetically, to a fixed count of 60. Not by size, not by editor context, not
by anything about the question.

**And 60 was never a number about prompts.** `TABLE_DETAIL_BUDGET`'s own doc
comment says it bounds *round trips* on an engine that describes one table at a
time, and that the assistant's limit "is about prompt size, not round trips,
and lives in `render_schema`'s caller". The caller then passes the same
constant. MySQL stopped making those round trips in Stage 27; the prompt limit
was left standing on a reason that no longer exists.

### 2.2 Notes and joins are lost for exactly the tables that needed them

Both causes the report guessed at are real, and they compound.

**The saved file only.** `assistant_system` reads the dictionary with
`dictionary::load_cached(path)` (`lib.rs:498`). An unsaved buffer never reaches
it. *Their question 2:* the saved file, and `dictionary_marks` is deliberately
the same — Stage 25's stance is that the tree's dots say what the assistant
will actually be told, so a dot must not promise an unsaved edit.

**The column cap hides notes too.** In `render_schema`, a table past
`detail_limit` takes the elided branch (`assistant.rs:234`), which prints
`TABLE \`name\`` and **never calls `afford`** — so it loses its note *and* its
`join`. The "columns not loaded" branch keeps the note but has never rendered
the `join` at all. *Their question 3:* no. A note survives only for a table
whose columns were not cached; a `join` survives nowhere.

For `appdata` that is the whole of it. The notes were on `parties`,
`partyinformation`, `enrolled_party` and `enrolled_item`; the `enrolled_*`
pair is past the alphabetical 60 in a 400-table database, so the one note that
mattered — `enrolled_party.status_id` points at `party_status`, not at the
similarly named `statuses` — was dropped by a limit about prompt size.

### 2.3 A saved profile never reaches the live connection

This is the sharpest bug in the report, and it explains the attach failure, the
restart, the dead menu item, and possibly 2.2 as well.

`ServerConn.profile` is a **clone taken at connect time** (`session.rs:618`),
held inside an `Arc` and never reassigned — `grep '\.profile = '` over
`src-tauri/src` finds nothing. `save_profile` (`lib.rs:143`) writes
`connections.json` and the keychain and does not touch the live session. So
every backend reader of `server.profile.dictionary` keeps seeing `None` until a
reconnect: `assistant_system`, `dictionary_open`, `dictionary_marks`, and MCP's
`Tools::dictionary` and `put_notes`.

Every path that saves a profile reconnects **except one.** The connection dialog
reconnects on every save — that is why Stage 14's read-only flag works, and §8
of that tracker is where the order of write-then-connect was settled.
`attachDictionary` (`main.ts:5626`) is the only caller of `saveProfile` that
does not, because attaching a text file to a connection has nothing to do with
the socket.

Three consequences, all reported:

- `put_notes` answers *has no AI dictionary attached* until a restart.
- The assistant sees no dictionary at all, so `has_dictionary` is false and the
  prompt does not even carry the paragraph that says whose the `--` comments
  are. "The schema also has no `--` notes to go by" is then literally correct.
- **AI dictionary… appears to do nothing.** `openDictionary` checks the
  *frontend's* freshly-upserted profile, so it skips attaching and calls
  `dictionaryOpen`, which throws from the stale backend profile. The catch
  reports through `results.setMessage` — which paints into `#grid`, and
  `showSurface` sets `#grid` `hidden` whenever the tab's output view is
  Sequence (`main.ts:2070`). With `[hidden] { display: none !important }` the
  error is not merely easy to miss, it is not rendered. `showNote` would have
  survived, because `#result-note` sits outside `#result-bar-row`.

And the worst of it: `hasDictionary()` (`main.ts:3594`) also reads the frontend
profile, so the chat header says *"along with the dictionary you attached to
this connection"* while the backend sends none. That sentence is built by
`whatIsSent`, whose doc comment says it exists in one place **because it is a
promise**. It was false in exactly the window this bug opens.

### 2.4 WSL2, and the token

Both of their remaining points are documentation, not code.

The bind is `Ipv4Addr::LOCALHOST` (`mcp.rs:1088`) with rmcp's allowed hosts
pinned to `localhost`, `127.0.0.1` and `::1` (`mcp.rs:1104`). That is Stage 13's
deliberate "loopback only, never `0.0.0.0`", and it should not change —
`networkingMode=mirrored` is the right fix and it belongs in the setup notes.
Nothing in the repository mentions WSL at all. Worth recording *why* their fix
worked without touching `allowed_hosts`: mirrored networking leaves the `Host`
header as `127.0.0.1`.

*Their question 5 — is the bearer token stable across restarts?* **Yes.**
`mcp::token()` reads it from the OS keychain and mints one only when there is
none; the only thing that changes it is Settings → Regenerate, which breaks
configured clients on purpose (`mcp.rs:123`). The `mcp_token` in their log is
the **Tauri command name** — `api.ts:94` logs `${cmd} ok in ${ms}ms` for every
call, and Settings calls it to display the token. The value is never logged.

## 3. What was measured

`src-tauri/tests/measure_prompt.rs`, a 400-table × 12-column database shaped
like theirs. First, what a table count buys:

| tables with columns | characters | ~tokens | render |
|---|---|---|---|
| 0 (names only) | 13,767 | 3,400 | 0.28ms |
| 60 (what shipped) | 32,608 | 8,200 | 0.45ms |
| 120 | 51,449 | 12,900 | 0.61ms |
| 200 | 76,569 | 19,100 | 0.89ms |
| 300 | 107,969 | 27,000 | 1.25ms |
| 400 (all) | 139,184 | 34,800 | 1.54ms |

A table with its columns costs ~315 characters; named alone, ~34. Rendering is
free at every size, so the budget is about what a request costs and nothing
else.

Two things follow. **The whole schema fits** — 35k tokens is affordable in a
200k context, just not in every request for ever. And **60 was far below where
the trade-off bites**: four times the columns for four times nothing.

Then the same database against the budget that shipped in this stage, which is
in characters (§4.2):

| budget | characters | ~tokens | described | by name |
|---|---|---|---|---|
| 0 | 13,783 | 3,400 | 0 | 400 |
| 23,000 | 34,508 | 8,600 | 66 | 334 |
| 45,000 | 54,291 | 13,600 | 129 | 271 |
| **60,000** | **67,793** | **16,900** | **172** | **228** |
| 115,000 | 117,404 | 29,400 | 330 | 70 |
| 200,000 | 139,184 | 34,800 | 400 | 0 |

So `SCHEMA_BUDGET_CHARS = 60_000` describes **172 of 400 tables** where the old
limit described 60 — 2.9× as many, for ~17k tokens a request — and the ranking
decides which 172. `SCHEMA_BUDGET_SESSION_CHARS = 240_000` covers a schema this
size whole.

And what runs on **every** question rather than once, same fixture, debug build:

| | cost |
|---|---|
| `words` on the question | 0.06ms |
| `relevance::order` over 400 tables | 21.9ms |
| `relevance::recognised` for the log line | 17.6ms |
| `assistant::reckoning` (diagnostics only, three renders) | 7.6ms |

`order` is 22ms rather than the renderer's 2ms because it calls `words` on
every column of every table — 4,800 of them — so it is O(tables × columns) with
an allocation each. Left alone: this is a debug build, and 40ms of it sits in
front of an API call that takes seconds. Worth knowing before somebody points
it at a 5,000-table warehouse, where the same shape is ~250ms.

The log line it produces is ~690 characters at its worst, which is the price of
naming twelve tables and five more at the cut. It exists to be pasted.

## 4. Decisions

### 4.1 Rank the tables, do not fetch them per question

The report's first suggestion is per-question schema lookup, the way an MCP
client does it. We are not doing that, and the reason is in `assistant.rs`'s
header: **the model is given no tools at all.** That is the feature's shape, not
an oversight.

It is also unnecessary here. The columns are already in memory; the prompt was
choosing the wrong 60 of them. So: order the tables by relevance to the
question, and spend the budget down that order.

Recorded as an open question rather than settled for ever — an *internal*
read-only introspection tool would not breach "it proposes; you execute", since
`warm` already introspects unasked. It would breach "no tools at all", and that
sentence is load-bearing enough to need its own stage.

### 4.2 The budget is in characters, not tables

A table count prices a 3-column lookup table the same as a 60-column fact
table. §3 measures characters, the constant should be the thing measured, and
`DICTIONARY_BUDGET_CHARS` is already in characters for the same reason.

`TABLE_DETAIL_BUDGET` stays what its doc comment always said it was — a bound on
round trips — and stops doing a second job it was never chosen for.

### 4.3 Two budgets, because a kept CLI session pays once

`agentcli::run_session` keeps a CLI process per recipe and reuses it only while
`agent.primed_with(system)` holds — the system prompt must be *identical*. A
prompt that moves with every question would restart the process every turn, at
a measured 5.3–11.7s against 1.85–2.66s for a kept one.

So the schema is ranked without the question on that path, and gets a much
larger budget: it travels once per session, so there is no per-request cost to
protect. Which also makes it the most stable prompt of the three — it changes
only when the schema does.

| path | question used | budget |
|---|---|---|
| HTTP (Anthropic, OpenAI-compatible) | yes | per request |
| CLI, no session support | yes | per request |
| CLI, kept session | **no** | per session |

### 4.4 The order, and no invented weights

A lexicographic sort key, not a weighted sum: weights would need a corpus of
real questions to fit, we have one question, and a number nobody measured
reading as though somebody had is worse than an ordering that admits what it is.

Per table, in order — **as planned here; §6.2 added two rungs when two tests
proved spelling a name out is a different act from sharing its words:**

1. **The question names it.** The strongest signal there is.
2. **How many of the question's words its columns match**, most first. This is
   the one that answers their actual question — "reserve balance", "original
   supplier" and "servicing status" name no table at all, they name columns.
3. **The editor names it.** You are writing a query about these tables.
4. **The dictionary describes it**, or another table's `join` points at it. Both
   mean somebody cared enough to write it down. Following joins is the report's
   own suggestion and it is free: the expressions are already parsed.
5. **Recent history on this connection names it.**
6. **It does not look like a copy.** `*_backup`, `*_archive`, `*_old`,
   `*_tmp`, a date suffix. Demoted, never dropped, and only below the signals
   above — a backup you asked about by name is a backup you meant.
7. **Alphabetically**, so the same question twice is the same prompt.

### 4.5 The dictionary is still read from the file — and now says so

Keeping Stage 25's stance: the file, not the buffer. The dots in the tree mean
"this is what the assistant will be told", and a dot that promised an unsaved
edit would be a worse lie than the one we are fixing.

What was missing is that nobody is *told*. `whatIsSent` gains a clause when the
dictionary tab is dirty, in the same sentence that already promises what leaves
the machine. `put_notes` already ends with "It is NOT saved".

### 4.6 Say what was left out to the person, not only to the model

`render_schema` already tells the model that columns were listed for the first
*n* tables. The person is told nothing, and they are the one who can fix it.

### 4.7 A saved profile reaches the live connection

The general shape, not a special case for `dictionary`. A live connection's
profile splits in two:

- **Connection parameters** — `host`, `port`, `user`, `database`, `url`, `auth`,
  `kind`, `allowInvalidCerts`, `readOnly`, `noPassword`. Frozen for the
  connection's life. They describe the socket that is already open, and
  `capabilities` was derived from them once at connect. Changing them takes a
  reconnect, which the dialog already does — Stage 14 §8.
- **Labels** — `name`, `colour`, `dictionary`. These describe the connection,
  not the socket. They follow a save at once.

So `save_profile` pushes the saved profile at any live session with that id, and
the session takes only the labels. One source of truth, and the invariant is
enforced in one function rather than hoped for at 21 call sites.

`id` comes out of the lock entirely: it is the identity, and nothing can edit
it.

### 4.8 Two ways to measure it against a real schema

Everything in §4 was fitted to a **synthetic** 400-table schema, because that
is what there was. The budget and the ordering are the two things most likely
to be wrong against a real one, and neither can be checked from here. So the
app says what it did, in the place that already exists for that —
`logbook`, whose header is *"what the app did, kept where somebody can copy it
out"*, and whose *Diagnostics…* button copies the ring and names the file.

Two surfaces, because they answer different questions:

- **The cost of a schema** goes in the diagnostics report's `=== schema ===`
  section, beside the cache counts that are already there. `assistant::reckoning`
  reads the cache and nothing else — the standing rule for that dialog is that
  opening it must not introspect a production server — so this is answerable
  **without asking a question and paying for it**, which matters when the thing
  being measured is the cost of asking.
- **Whether the ranking was right** can only come from a real question, so it
  is one `info` line per question in the `assistant` area.

#### What the log line may carry

`logbook`'s rule is that `note` takes a sentence and never a structure, so a
call site says what happened in words it chose — that is what keeps a password
out *by construction* rather than by filtering, and the scrubber behind it
drops a whole entry rather than editing it.

The first instinct was to log the question's words. **That is not safe, and the
scrubber would not have caught it:** somebody typing "connect with password
hunter2" yields the bare word `hunter2`, and `looks_like_a_secret` looks for
`password=` and `password:`, not for a word in a list.

So the line names only the words **the schema itself has a name for**
(`relevance::recognised`), which cannot be anything but schema vocabulary — the
same argument that keeps the editor's text out of the prompt. The rest are a
count.

It is also the better diagnostic, which is the part worth keeping. The question
to ask of a bad answer is "did my words land at all?" An empty vocabulary says
the folding is wrong — `status` arriving as `statu`, which really happened and
is §6.1 — where a full one says the ordering is.

## 5. Milestones

- [x] **M1 — Attaching a dictionary works without a restart.** The live
      connection follows the save, so `put_notes`, the assistant and the tree's
      dots all see it at once.
- [x] **M2 — AI dictionary… never silently fails.** Its four failure paths
      report through `showNote`, which is outside the row Sequence view hides.
- [x] **M3 — Connection parameters still need a reconnect.** `follow_saved`
      takes the labels and refuses the rest, asserted field by field.
- [x] **M4 — A note survives the budget.** A table with a note and a `join`,
      ranked past the budget, carries both.
- [x] **M5 — The question picks the tables.** Their question against a
      127-table fixture puts all seven tables it needs in the first seven.
- [x] **M6 — A column name is enough.** "reserve balance" ranks
      `external_holding_account` first without naming it.
- [x] **M7 — The editor's text never reaches the prompt.** Asserted on the
      ranking's output rather than trusted of the call site.
- [x] **M8 — A kept CLI session is not restarted per question.** Structural:
      one `Option` turns off every signal that moves — see §6.3, where leaving
      history in would have done exactly this.
- [x] **M9 — The person is told what was left out**, in the chat, and only when
      something was.
- [x] **M10 — The unsaved dictionary is admitted** in the chat's standing
      sentence, and as you type rather than when the panel is next opened.
- [x] **M11 — WSL2 is in the README**, with why mirrored networking is the fix
      and why the server's host checks do not have to be relaxed.
- [x] **M13 — It can be measured against a real schema.** Diagnostics says
      what a cached database costs a prompt with no query and no API call, and
      the log carries one line per question with the counts, the ranking's
      first twelve, where it cut, and which of the question's words the schema
      recognised.
- [x] **M12 — Nothing regressed.** 602 Chromium, 601 WebKit (1 skipped), the
      offline Rust suite green, `check` exit 0.

## 6. Task tracker

### Phase 1 — the live profile (M1-M3)
- [x] `ServerConn.id` hoisted out; `profile` behind an `RwLock` with an accessor
- [x] `ServerConn::parts`, so three construction sites cannot disagree about `id`
- [x] `ServerConn::follow_saved`, taking labels and refusing parameters
- [x] `save_profile` pushes at the live session, after the write succeeds
- [x] `openDictionary` and `attachDictionary` report through `showNote`
- [x] 5 unit tests; 3 falsifications

### Phase 2 — notes survive the budget (M4)
- [x] A table named alone pays for its note and its `join`, whichever branch
      put it there
- [x] "Named alone" and "columns not loaded" are told apart, in the text and in
      `Rendered`
- [x] 6 unit tests

### Phase 3 — relevance (M5-M8)
- [x] `relevance.rs`: word extraction, folding, copy detection, the sort key
- [x] `render_schema` takes an order and a budget in characters, and returns
      what it could not carry
- [x] `assistant_system` takes `Ask`, with one `Option` over the moving signals
- [x] Both commands plumbed; the session path passes `None` and the big budget
- [x] 13 unit tests, including the editor-text leak

### Phase 4 — what the person is told (M9, M10)
- [x] `StreamEvent::Context`, rendered as one dim line and only when it says
      something
- [x] `whatIsSent` admits an unsaved dictionary; the note is repaintable
- [x] `paintDictChip` so "· unsaved" appears as you type
- [x] 4 UI tests; 4 falsifications

### Phase 5 — docs (M11)
- [x] WSL2 and the token's stability in the README
- [x] Their five questions answered in §2

### Phase 6 — measuring it against a real server (M13)
- [x] `assistant::reckoning`, in the diagnostics report's schema section
- [x] `assistant::ranked_note`, one `info` line per question
- [x] `relevance::recognised`, so the line cannot name a word the schema has
      no name for
- [x] 9 unit tests, including one that the scrubber does not eat an ordinary
      line; 3 falsifications
- [x] `measure_prompt.rs` records the per-question cost

## 6.1 `fold` ate the most common word in a schema

`status` ends in `s` and is six letters, so the length guard let it through and
the singular rule returned `statu`. Every question mentioning a status stopped
matching every `*_status` table — and the doc comment claimed the guard
prevented exactly that.

A length guard cannot tell a Latin ending from a plural. `NOT_PLURAL` lists the
endings where the `s` belongs to the word — `us`, `is`, `os`, `ss` — and `as` is
deliberately absent, because `areas` and `schemas` are ordinary plurals. Found
by the test that asserts it, which is the only reason it was not shipped.

## 6.2 Naming a table had to outrank using its words

Two tests failed, and both were right to.

`names_it` asks whether the question used *every* word of a table's name, which
is what stops a schema full of `client_*` tables from all matching "client".
But it also means that asking to "select from `parties_backup`" names `parties`
— one word, and the question has it — so `parties` won on the alphabet while the
table actually spelled out came second. The same shape put `Supplier` ahead of
`supplier_status` when the editor named only the latter.

The fix is a stronger signal above the existing one: **the question spells the
name out.** `raw` keeps identifiers whole where `words` splits them, and two
keys became four. Spelling a table out is a different act from sharing its
vocabulary, and it deserved its own rung rather than a tweak to the rung below.

## 6.3 Gating the question was not enough to keep a CLI session

§4.3 said the kept-session path must not rank on the question, because
`primed_with` restarts the process when the prompt moves. The first
implementation gated the question and the editor and left recent history in —
and history moves every time anything is run. So running a query in the middle
of a conversation would have restarted the CLI, costing 5.3-11.7s for a reason
nobody could have inferred from the outside.

`Ask.moving` is one `Option` over all three signals now, so there is no way to
turn off the question and leave something else that moves. The rule lives in the
type rather than in three agreeing conditionals.

## 6.4 Two more places nothing was said

Neither was in the report, and both are the same bug as the report's dead menu
item: something true, nowhere visible.

**The dictionary chip never said "· unsaved" while you typed.**
`refreshFileNote` paints it, and it runs on tab activation only — because its
other work asks the backend for the tree's dots and the band, which must not
happen per keystroke. So the chip appeared only once the tab was next
activated, which is exactly not when you want it. `paintDictChip` is the text
alone, and `onDocChanged` can afford it.

**The chat's standing sentence was painted only when the panel opened.** The
panel is a dock; it stays open. Typing in the dictionary changed what would be
sent and the sentence kept the old promise. The note is rebuilt from a held
prefix now, and `onDocChanged` rebuilds it.

## 7. Open questions

- **43 more invisible messages.** `results.setMessage` is used 44 times in
  `main.ts` and paints into `#grid`, which Sequence view hides. For "no results
  yet" and a failed run that is exactly right — it *is* what the pane shows.
  For an incidental failure it is the bug §2.3 describes, and this stage fixed
  only the four dictionary paths. Sorting the rest is a reading of all 44, not a
  sweep, because the two uses look identical at the call site.
- **An internal schema tool.** §4.1. The honest version of the report's first
  suggestion, and a breach of a sentence in `assistant.rs`'s header that has
  held since Stage 0. Its own stage, if ever.
- **"Don't query" in the dictionary.** The report suggests honouring it. There
  is no such field — `TableNote` has `note`, `join` and `columns` — and reading
  it out of prose would be magic. A real field is a Stage 25 question.
- **Routines.** `render_schema` appends every procedure and function after the
  tables, unranked and unbudgeted. Nothing in the report touched it, and a
  server with hundreds of them would now be the biggest thing in the prompt.
- **The weights that are not there.** §4.4 is an ordering because there is no
  corpus to fit weights against. §4.8 is how one gets collected: the log line
  pairs each question's recognised vocabulary with what the ranking did about
  it. Worth revisiting once there are twenty real lines, rather than by taste.
- **A vocabulary cache.** `order` and `recognised` both call `words` over every
  column of every table, 4,800 times each on a 400-table schema and ~60,000 on
  a 5,000-table one. Caching the word sets per database would make both nearly
  free. Not built, because §3 says the pair costs 40ms in a debug build in front
  of an API call that takes seconds — but it is the first thing to do if anyone
  points this at a warehouse.
- **Hands-on.** Everything here is tested and none of it has been used against
  a real 400-table server. The ranking is the part where that matters: a test
  can say the right tables came first for a question somebody wrote down, not
  that it feels right on the twentieth question of a working afternoon.

## 8. Found in use: the dictionary could not be saved at all — 2026-11-04

Reported the moment this went in front of somebody: an MCP agent wrote notes
into the dictionary tab, and **Ctrl+S did nothing.**

Ctrl+S is the only save path in this app. There is no File menu and no Save
button, and the keymap read:

```ts
} else if (e.key === "s" || e.key === "S") {
  e.preventDefault();
  const t = tabs.active();
  if (!t || t.kind !== "sql") return;      // <- a dictionary is not "sql"
  void (e.shiftKey ? files.saveAs(t) : files.save(t));
}
```

So a dictionary tab could not be saved **by any means**. And since the
assistant and MCP both read the *file* — deliberately, §4.5 — every note
`put_notes` wrote was unreachable: the whole flow dead-ended one keystroke
before the end. `e.preventDefault()` ran first, so there was not even a browser
save to fall back on. It did nothing, silently, which is the third time in this
stage that the bug was *something true with nowhere to appear*.

### Stage 25 said it worked

Its §4 describes the dictionary as "an ordinary file-backed tab — same
`OpenedFile`, so Ctrl+S, the modification-time guard, the line endings and the
BOM are the ones `files.rs` already has", and milestone **D7** is "the file
opens in a tab, **saves with Ctrl+S**, and offers no Run". D7 is ticked.

Every word of that is true of `files.save`, which has never looked at a tab's
kind and needed no change here. None of it was true of the guard in front of
it. **Every Ctrl+S test in the suite is on a SQL tab**, so nothing held the
claim — the same shape as Stage 28 §8, where reviewing a stage against its own
document found four settled claims no test held. A milestone that says "saves
with Ctrl+S" and is checked by hand once is a milestone that stops being true
the next time somebody touches the keymap.

### Save As is refused rather than allowed

`markSaved` repoints the tab at whatever was written, while the connection's
profile still names the old file. So a copy made to be safe would quietly
become the file Ctrl+S writes from then on, and the dictionary the assistant
reads would stop receiving edits with nothing on screen saying so. Ctrl+Shift+S
on a dictionary says it is saved in place and does nothing else.

### Tests

Four, all of which should have existed since Stage 25, and four falsifications:

- Ctrl+S writes the buffer to the dictionary's own path, and the tab stops
  being dirty.
- A save re-reads what the file describes, so the tree's dots follow. (The
  first version of this test asserted on a tree with nothing expanded, where
  there are no dots to repaint and `dictionary_marks` is never called — the
  test was wrong, not the code.)
- A save withdraws the chat's "unsaved dictionary edits" clause.
- Ctrl+Shift+S explains itself and calls no save dialog.

### Left open

**A dictionary's path cannot be changed once set.** The rail's menu offers
"Add an AI dictionary…" only while there is none, the connection dialog carries
the field forward rather than editing it (Stage 25 §3.3, so a port change
cannot drop it), and Save As is now refused. Pointing a connection at a
different file means editing `connections.json` by hand. Nobody has asked for
it; it is written down here because this fix is the third door to close.
