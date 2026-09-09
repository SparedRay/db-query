# Stage 13 — Letting other tools in: an MCP server

**Status:** 📋 Planned 2026-09-09.

The first stage where db-query **listens**. Everything until now dialled out —
to a database, to a model, to a release endpoint. This opens a door, and the
whole stage is about opening it narrowly enough to be worth opening.

---

## 1. It is not a POC any more

Stated first because it changes what "good enough" means. It is in daily use,
by someone who did not write it, on two platforms, with real data. So:

- **"It's only a POC" stops being an argument.** Not for a dependency's licence,
  not for a listening socket, not for a placeholder in a legal notice. It was
  never much of an argument here — the licence review in Stage 12 §17 is what a
  tool gets, not what a prototype gets — but it can no longer be reached for.
- **`major` remains unused for now.** The README's reasoning stands on its own:
  below 1.0, `minor` already means "new capability". That is a versioning
  choice, not a claim about maturity.
- **The two rules that made this app safe do not relax.** The assistant
  proposes and never executes; secrets live in the OS keychain and nowhere
  else. This stage leans on both harder than any before it.

---

## 2. Why an MCP server, and why this app can host one safely

The ask: let Claude — or any MCP client — see the schema you are connected to
and drop a query into your editor, without the round trip of copying text
between two windows.

The reason it is safe here is a rule that predates it: **nothing runs without
the user pressing run.** A database MCP server that executes SQL is a
genuinely dangerous thing to leave running on a laptop — an agent that
misreads a request can drop a table. One that can only *propose* has the blast
radius of a stranger showing you some text. That is the whole design.

So this server has **no execute tool**, and that omission is the feature rather
than a limitation to be fixed later. It is asserted by a test over the
advertised tool list, so it cannot be added by accident.

---

## 3. Design

### 3.1 Transport: Streamable HTTP on loopback

The specification defines two transports and says clients **SHOULD** prefer
stdio. We cannot use stdio, and the reason is worth writing down: in stdio the
**client launches the server as a subprocess**. A subprocess of db-query is a
second copy of the app — no window, no connections, no editor. The thing an MCP
client needs to talk to is the app that is *already running*, holding the
connection the user opened and the tabs they are typing in.

So: **Streamable HTTP**, one endpoint, `http://127.0.0.1:<port>/mcp`.

### 3.2 The four things that make a local listener acceptable

The spec's own security section for this transport is short and we follow all
of it, plus one more:

1. **Off by default.** A switch in Settings → *Integrations*. An app that starts
   listening because it was installed is not something to ship.
2. **Loopback only.** Bind `127.0.0.1`, never `0.0.0.0` (spec: SHOULD).
3. **`Origin` validation.** Reject any request whose `Origin` is not absent or
   loopback (spec: MUST — without it a web page you visit can drive the server
   through DNS rebinding).
4. **A bearer token** (spec: SHOULD). 32 random bytes, base64. It lives in the
   **OS keychain**, like every other secret this app holds, and never in the
   config file. Settings shows it with *Copy* and *Regenerate*.

And the fifth, which is ours rather than the spec's: **there is nothing to
execute.** Even a complete failure of the four above yields an attacker who can
put text in a tab.

### 3.3 Tools

| Tool | Does | Reads |
|---|---|---|
| `list_databases` | The namespaces on the active connection | `schema.rs` cache |
| `list_tables` | Tables and views, optionally in one database | `schema.rs` cache |
| `describe_table` | Columns, types, nullability, keys | `schema.rs` cache |
| `put_query` | Opens a **new tab** with the SQL, focused, **not run** | — |

Four, and no more. In particular there is no `run_query`, no `read_results`,
and no write of any kind.

**Everything is scoped to the active connection.** An MCP client cannot choose
a server: it sees what the user is looking at. That is deliberate on two
counts — it keeps the mental model honest ("it sees my screen"), and it means
an agent cannot reach a production connection the user forgot was open in
another tab. The consequence, which must be documented rather than hidden: if
the user switches connection, the agent's view changes under it.

**`read_results` is deliberately out.** Schema names are one privacy question;
row data is a different and larger one. If it is ever added it gets its own
switch, not a share of this one.

### 3.4 Provenance

A tab that appears unbidden must not look like one you opened. Tabs created
through `put_query` are marked as external in the tab strip, and the query is
never run. The chat has the same property today; the difference is that a chat
answer was asked for and this may not have been.

---

## 4. What this stage is not

- **Not an execute tool.** See §2. Not now, not behind a flag.
- **Not a remote server.** Loopback only. No LAN, no tunnel, no auth beyond the
  local token — anything else is a different threat model and a different stage.
- **Not multi-connection addressing.** The active connection, and only that.
- **Not row data.** Schema only.
- **Not an MCP *client*.** db-query does not consume other servers' tools. That
  is a plausible future stage and shares nothing with this one but a spec.

---

## 5. Milestones

- [ ] **M1 — A real client connects.** Claude Desktop or Claude Code, configured
      with the URL and token, lists the four tools.
- [ ] **M2 — The schema it reports is the real one.** `list_tables` and
      `describe_table` against a live MySQL and a live Elasticsearch match what
      the tree shows.
- [ ] **M3 — `put_query` lands in the editor**, in a new tab, focused, **not
      run**, and visibly marked as external.
- [ ] **M4 — Off means off.** With the switch off, nothing is listening on the
      port: proved by connecting to it and failing, not by reading the code.
- [ ] **M5 — The door is locked.** A request with no token, a wrong token, or a
      foreign `Origin` is refused, and the refusal says which.
- [ ] **M6 — Nothing can execute.** A test asserts the advertised tool list is
      exactly the four in §3.3.
- [ ] **M7 — Nothing regressed.** Every suite, both engines, both platforms.

---

## 6. Task tracker

### Phase 1 — The server
- [ ] `rmcp` dependency, MSRV bump, licence check recorded (§8)
- [ ] `src-tauri/src/mcp.rs`: the four tools over `schema.rs`
- [ ] Streamable HTTP on 127.0.0.1, started and stopped by a setting
- [ ] Token in the keychain; `Origin` and bearer checks with their own tests

### Phase 2 — Reaching the editor
- [ ] `put_query` emits to the frontend; a new tab, focused, never run
- [ ] External tabs are marked, and the mark survives a session restore

### Phase 3 — Settings and discoverability
- [ ] Settings → *Integrations*: switch, port, token (copy / regenerate)
- [ ] The exact client config snippet, copyable, with the URL and token in it

### Phase 4 — Proving it
- [ ] Unit tests: tool list, auth, origin, and the schema shape each tool returns
- [ ] A live test that drives the server over HTTP as a client would
- [ ] M1-M7 by hand

---

## 7. Risks

- **A listening socket is a new class of bug for this codebase.** Mitigated by
  §3.2 and by there being nothing to execute; not eliminated. This is the reason
  the stage exists as a stage rather than as a commit.
- **`rmcp` 3.x requires Rust 1.88** and we declare 1.82. CI builds with stable,
  so this is a declaration change, not a toolchain one — but it is a real bump
  and belongs in the same commit as the dependency.
- **Two `schemars` versions** will be in the tree (0.8 via tauri, 1.x via rmcp).
  Harmless to the build; it will change the licence manifest, and Stage 12 §17's
  guard will say so rather than letting it slip.
- **The MCP spec revises.** Pin `rmcp` and record the protocol version the tests
  negotiate, so a client that speaks a newer one fails loudly rather than half-working.
- **An agent proposing a destructive query** is the residual risk, and it is the
  same one the assistant already carries: `DROP TABLE` in a tab is text until
  someone runs it. The provenance mark exists so it is obvious where it came from.

---

## 8. What it costs

Measured, not estimated: `cargo add rmcp --features server,macros,
transport-streamable-http-server` adds **10 crates** to a tree of 524, because
tokio, serde, futures, hyper and chrono are already there.

| Crate | Licence |
|---|---|
| `rmcp`, `rmcp-macros` | Apache-2.0 |
| `schemars`, `schemars_derive` | MIT |
| `futures`, `pastey`, `ref-cast`, `ref-cast-impl`, `serde_derive_internals`, `sse-stream` | MIT OR Apache-2.0 |

Every one is already in `about.toml`'s `accepted` list. Checked against
crates.io on 2026-09-09, not from memory.
