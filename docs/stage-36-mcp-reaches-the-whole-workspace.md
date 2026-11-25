# Stage 36 — MCP reaches the whole workspace

**Status:** 🔒 Frozen — opened and closed 2026-11-25. Built, tested on both
engines, and confirmed by use.

---

## 1. The ask

> *"Currently looks like agent can only open tabs for currently active session
> and is unable to read the result after we execute it. We need to give it the
> ability of first read a tab (we need to have a way to say which one as is not
> limited to the tabs it opens); second we need that the agent can open a tab on
> a connection that is active but is not the current one — like we open 2
> connections, currently we are seeing something on A but agent needs to create
> a tab on B, that should be possible."*
>
> *"Lastly as a small UI fix, Send button on assistant looks not rendered
> correctly."*

## 2. What is true today

`put_query` and `put_notes` both call one helper:

```rust
/// Which connection the user is looking at, or a refusal that says how to
/// fix it. An MCP client cannot choose: it sees the open window.
async fn connection(&self) -> Result<String, ErrorData>
```

That sentence is Stage 13's design, and half of it is now wrong. **The frontend
was never the limitation** — `put_query`'s handler in `main.ts` already creates
the tab on whichever connection the event names, and says so when that is not
the workspace on screen. The limitation is that the tool has no way to be
*told* which connection, because the server only ever reports the focused one.

And there is no way to read anything back. The Stage 13 posture was "nothing
that executes, nothing that reads data", so an agent that hands over a query is
blind to what it did: the person runs it, sees the answer, and has to paste it
back by hand.

## 3. What this changes, and what it does not

**Data now leaves the app.** This is the real decision in the stage, and it
should be stated rather than discovered. Until today the MCP surface carried
schema and nothing else: table names, column types, the dictionary's prose. A
tab's result is rows the server returned — real values, from a real database.
Reading one hands those to whatever MCP client is attached.

What keeps that acceptable is unchanged from Stage 13 and worth restating:
the server binds loopback only, it is off unless the person turns it on, and it
answers only the tools listed here. What changes is that "nothing reads data"
is no longer one of them, and the tool descriptions say so plainly rather than
implying a safety that is gone.

**Nothing runs unattended, still.** The agent may open a tab and read a result;
it may not run anything. The loop is: agent writes the query, *the person*
presses Run, agent reads the answer. Giving an agent the Run button is a
different stage and a different argument, and nobody has asked for it.

## 4. The tools

| Tool | New? | What it does |
|---|---|---|
| `list_connections` | new | every open connection: id, name, engine, current database, and which one is on screen |
| `list_tabs` | new | every open tab: id, title, which connection it belongs to, whether it has a result and how many rows |
| `read_tab` | new | one tab's result: the statement, its columns and its rows, capped |
| `put_query` | changed | takes an optional `connection`, defaulting to the one on screen |
| `list_databases`, `list_tables`, `describe_table` | changed | take an optional `connection`, same default |

**Addressing.** A tab or a connection is named by its id, which the list tools
return. `put_query` also accepts a connection's *name*, because that is what a
person says to an agent ("put it on UAT") and an agent that has to ask for a
list first to translate one word is a worse tool.

**Caps.** `read_tab` returns at most 200 rows by default and never more than
1000, and says how many it left. A grid holds 5,000; an agent's context does
not, and a tool that quietly fills it is worse than one that refuses.

## 5. How it is wired

Connections and their names live in Rust (`AppState.connections`), so
`list_connections` is answered there with no round trip.

Tab titles and results live only in the frontend, so `list_tabs` and `read_tab`
ask it. That round trip already exists for `put_notes` — an event out carrying
a request id, a `oneshot` held in a pending map, and a command the frontend
calls to answer — and **one channel is extended rather than a second one
invented**: `mcp://ask-workspace` carries what is being asked, and
`mcp_workspace_answered` carries the reply. A third question later adds a
variant, not another pair of endpoints.

## 6. Milestones

- [x] **W1 — The agent can see what is open.** `list_connections` names every
      open connection and marks the one on screen; `list_tabs` names every tab
      with its connection and whether it has a result.
- [x] **W2 — The agent can read a tab it did not open.** `read_tab` returns the
      statement, columns and rows of any open tab.
- [x] **W3 — Caps hold.** More rows than the cap returns the cap and says how
      many were left behind.
- [x] **W4 — The agent can put a query on another connection.** `put_query`
      with a connection that is open but not on screen creates the tab there,
      and the app says so without switching the user away.
- [x] **W5 — A bad address is refused usefully.** An unknown tab, an unknown
      connection or an ambiguous name is an error that names what is open.
- [x] **W6 — Nothing runs.** No tool executes SQL, and the test asserts it.
- [x] **W7 — The Send button looks like the app.** Every button in the
      assistant pane shares the app's chrome.
- [x] **W8 — Nothing regressed.** Both engines, both suites, `check` clean.

## 7. Task tracker

### Phase 1 — Rust
- [x] `WorkspaceAsk` / `WorkspaceAnswer`, the `mcp://ask-workspace` event and
      `mcp_workspace_answered`
- [x] `list_connections`, answered from `AppState`
- [x] `list_tabs`, `read_tab`, answered by the frontend
- [x] `connection` resolves an argument, by id then by name, before falling
      back to focus
- [x] Caps and truncation

### Phase 2 — The app
- [x] Answer `ask-workspace`: tab list, and one tab's result
- [x] The Send button, and every other button in that pane

### Phase 3 — Proving it
- [x] Rust: resolution, caps, refusals
- [x] UI: the answers, and that nothing ran
- [x] `mcp_http` integration: the new tools over the real transport

## 8. Risks

**A result is bigger than it looks.** One tab can hold several result sets, and
a cell can hold a megabyte of JSON. The caps are per request *and* per cell, so
a single wide row cannot defeat them.

**The answer can arrive after the asker gave up.** The round trip has a
timeout; a late answer resolves nothing and must not panic. `put_notes` already
has this shape and the pending map drops the entry on timeout.

**A tab can close between listing and reading.** `read_tab` says so rather than
returning an empty result that reads like an empty table.

## 9. What the tests pin

**Rust, 11 new.** Addressing is a pure function (`resolve_connection`) so every
case is a unit test: by id, by name whatever the case, an id preferred over a
name that collides with it, and — the one that matters — **two connections with
the same name is a refusal that lists their ids, never a guess**. The cost of
guessing here is a query opened against production.

The caps are pinned the same way: the default, the ceiling on the argument, a
zero that becomes one, a long cell cut on a character boundary so a multi-byte
value does not come back broken, and the cell cap reaching every row of every
set — because one row of JSON blobs would otherwise defeat the row cap on its
own. NULL survives as NULL through all of it.

`TOOL_NAMES` went from five to eight, which failed the guard test on purpose:
the advertised surface of a server other people's agents talk to does not grow
by accident, and the test exists to make somebody type the new name.

**UI, 7 new.** The window's half: listing tabs with their connections, reading
a tab the client never opened, the cap applied where the rows are, a tab that
is not open, a tab nothing has run in, nothing executing, and a query placed on
a connection that is open but not on screen — which **does not switch the
user**, because a client that could move the window out from under somebody
mid-sentence is worse than one that cannot reach the other connection at all.

**Falsified.** Removing the row cap fails W3; dropping the connection name from
the tab list fails W1.

## 10. Still open

**`read_tab` has no live test.** The round trip is proved against the window
and the tools are proved over a real socket, but nothing exercises an agent
reading a real result from a real server end to end. The first real use is the
test.

**One tab, one call.** An agent that wants three tabs makes three calls. Fine
today; a `read_tabs` taking several ids is the obvious shape if it is not.

**Nothing runs, still.** The loop is: the client writes the query, the person
presses Run, the client reads the answer. Giving an agent the Run button is a
different stage and a different argument, and nobody has asked for it.
