# Stage 29.2 — the filter searches the whole schema

**Status:** ✅ Built — 2026-10-30. Hands-on outstanding.

---

## 1. The ask

> *"one additional change we will need to do is on our Search currently is only
> searching tables and we should allow to search columns as well"*

## 2. What was actually true

Worse than reported, and better.

**Columns were never searched at all** — not "only the loaded ones", none of
them, expanded or not. `buildColumnNode` returns a bare `.node`, with no
wrapper, because a column has no children to hold; every other level is a
wrapper div holding a `.node` and a `.children`. The walk read every child of a
`.children` as a wrapper, so for a column it took the `.twisty` span for the
node, found none, recursed into spans and reported nothing. Probed: a tree with
`users` expanded and `display_name` on screen, filtered for `display_name`,
said **"none of 8"**.

And `treefilter.ts`'s own header said the limit was deliberate:

> *"searching the server would be a different feature, with a round trip and a
> spinner, and this one is for 'I know the name, get me there' on a schema
> already open."*

**Stage 27 removed that cost.** Every column name on the server is already in
`schemaMap` for any engine that can describe a database in one query. The
reason for the limit had been gone for three days.

## 3. What was measured

A substring scan of the Stage 27 fixture's whole server — 12 databases, 1,920
tables, **26,880 columns** — in the real page:

| | |
| --- | --- |
| Best of 5 | **1.7 ms** |
| Worst of 5 | **6.2 ms** |

Per keystroke, against a tree whose own walk is already O(nodes). No debounce,
no cache, nothing to schedule.

## 4. Decisions

1. **The tree searches the names it has plus the columns Stage 27 prefetched.**
   No round trip, so the old limit's reason does not survive restating.
2. **A match in something undrawn is marked on the nearest thing that is
   drawn**, with how many matched: `1 col` on a table, `3 inside` on a database
   nobody has opened. *The user's call, asked and answered — the alternative
   was a flat list of matches beside the tree.*
3. **Nothing is expanded to show a match.** Drawing a table's columns would be
   the filter deciding what is loaded, which is the one thing this module has
   always refused; and the tree's mutation observer already re-runs the filter
   on whatever arrives, so expanding it yourself brings the columns in
   *already marked*.
4. **The mark is an attribute, not an element.** That observer watches
   `childList`, so a filter that appended a badge would retrigger itself for
   ever. `data-filter-note` is read back by CSS through `attr()`.
5. **An engine with no bulk column query searches what was loaded, and the
   count says so.** `loadDatabase` fills such a connection with
   `slot[name] ??= []` — names, with an empty list standing for *not fetched*.
   Returning that as an answer would report a complete search of a server whose
   fields were never read, so the lookup says `null` and the count reads
   `· loaded only`. *The user's call: the alternative was fetching every
   mapping on first search, which is the round trip Stage 27 refused to make on
   anybody's behalf.*
6. **The schema is consulted only while the tree and the prefetch agree about
   the connection.** `schemaMapFor` follows the active *tab*; the tree follows
   the active *connection*. Normally the same, but Stage 27 §3 records
   production's tables once completing against staging, and this is the same
   hazard one surface over.

## 5. Milestones

| # | Claim | Evidence |
|---|---|---|
| S1 | A column of an expanded table is a match in its own right | UI test |
| S2 | A column of an unexpanded table marks its table, with a count | UI test |
| S3 | Expanding that table shows the column already marked | UI test |
| S4 | A database nobody has opened says how many are inside | UI test |
| S5 | Opening it replaces the count with the real thing, without doubling it | UI test |
| S6 | Clearing the box takes the counts with it | UI test |
| S7 | Without prefetched columns the count says it searched what was loaded | UI test |
| S9 | A collapsed group — views, procedures, functions — is searched from the lists the tree already holds | UI test |
| S10 | Expanding a counted group replaces its count without doubling it | UI test |
| S11 | A database nobody has opened says the search covered what was loaded | UI test |
| S8 | Hands-on, in use | The user |

## 6. Built — 2026-10-30

`treefilter.ts` takes an optional `SchemaLookup`; `walk` handles a leaf that is
its own node; `fromSchema` counts what is real but undrawn. `main.ts` puts
`data-db`/`data-table` on the nodes, tracks which connection's tree is on
screen, and builds the lookup. One CSS rule.

### 6.1 A decorative pseudo-element made a tree node unclickable

The badge was `margin-left: auto`, to sit at the right of the row. That made
the `.node` as wide as its flex line and moved its centre to the far edge, past
the pane's horizontal clip — so **clicking a database stopped expanding it**,
and only for needles that produced a badge, which is why it looked like a
filter bug rather than a layout one. Two tests failed; the probe that settled it
was disabling the one rule. It sits beside the label now, where it also reads
better.

Worth the note because nothing about the rule looks behavioural. A badge is
decoration until it changes where the row's centre is.

### 6.2 Tests

Five rewritten and seven new, in `tests/ui/tree-filter.spec.ts`.

The rewrites are the feature, not patches: four tests used `user` as a needle
that "matches nothing but `users`", and `orders.user_id` now keeps `orders` —
correctly. They assert `big`, which matches no name and no column, and the one
about the count asserts `2 of …`, because two tables match now and one of them
by a column.

**Six falsifications, each biting its own assertion**: the walk back to
wrappers only, so columns are unreachable; no schema consulted; the schema
counted even where the DOM already has it, which double-counts; a names-only
prefetch reported as a complete search; clearing leaving the counts behind; and
the badge pushed right again.

## 7. The groups, and a checkbox that was not needed — 2026-10-30

> *"Couldt we add a sort of checkbox in search so if check does a full
> (Heavier) search including views and if niot it will do a lighter for only
> columns/tables?"*

**There was no heavier.** §6.3 had recorded views and routines as a gap left
open because "the prefetch does not record which kind a name is" — true of the
*column* prefetch, and beside the point. `loadDbChildren` fetches
`list_tables` **and** `list_routines` when a database is opened, and holds
`baseTables`, `views`, `procs` and `funcs` in the closures that build each
group; the counts in the group labels come from exactly those. And
`mysql_all_columns` reads `information_schema.columns`, which covers views, so
a view's columns were already in `schemaMap`. Searching a collapsed group costs
a substring scan over arrays already in memory.

So the toggle would have gated nothing, and the honest answer was to make the
search complete instead — *asked, and that is what the user chose.* What a
checkbox could have gated is one case only: **routines in a database nobody has
opened**, which have had no `list_routines` run against them. That stays
unsearched, and the count says `loaded only` there.

7. **A collapsed group is searched from the names the tree fetched**, and
   marked `N inside` like a database. Its `data-group` and `data-group-kind`
   say which list to ask for and whether those names have columns — a
   routine's parameters are not in the prefetch, and asking for them would
   answer "never fetched" and report a partial search that is nothing of the
   kind.
8. **A database nobody has opened reports a partial search**, because its
   routines were never listed however complete its columns are. Before this,
   such a database quietly claimed a whole-server answer.

### 7.1 Tests

Four new, and two counts corrected upward because the feature now finds more:
`user` matches three things, not two — `users` by name, `orders.user_id` by
column, and the `user_totals` view through a group nobody has expanded.

**Four falsifications, each biting its own assertion**: a collapsed group left
unsearched; a routine group asked for columns, so reporting a partial search;
an unopened database claiming it searched everything; and a group counted from
the schema even once its nodes are drawn, which double-counts.

**598 UI tests on Chromium and 597 on WebKit, none failing** (one skipped on
WebKit); `mise run check` exit 0. The backend did not change.

### 7.2 What is left unsearched

Routines in a database nobody has opened. One `list_routines` per database
would fix it, which is the round trip Stage 27 declined to make on anybody's
behalf — and the count says `loaded only` rather than implying otherwise.
