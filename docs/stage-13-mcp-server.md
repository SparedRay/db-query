# Stage 13 — Letting other tools in: an MCP server

**Status:** 🚧 In progress. Planned 2026-09-09; Phase 1 done the same day.

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
- [x] **M3 — `put_query` lands in the editor**, in a new tab, focused, **not
      run**, and visibly marked as external. Asserted in `tests/ui/mcp.spec.ts`,
      where the load-bearing assertion is the negative one: `run_script` never
      appears in the command log. Still to be seen by hand against a real
      client, which is M1's job.
- [x] **M4 — Off means off.** With the switch off, nothing is listening on the
      port: proved by connecting to it and failing, not by reading the code.
      `off_means_nothing_is_listening`, and `restarting_releases_the_previous_port`
      for the case where the port changes. Both were **confirmed to go red** by
      making `stop` a no-op.
- [x] **M5 — The door is locked.** A request with no token, a wrong token, or a
      foreign `Origin` is refused, and the refusal says which — asserted over
      the socket, not just over a `HeaderMap`.
- [x] **M6 — Nothing can execute.** Twice: over the router
      (`advertises_exactly_the_four_tools`) and over the wire
      (`a_client_handshake_lists_exactly_the_four_tools`).
- [ ] **M7 — Nothing regressed.** Every suite, both engines, both platforms.

---

## 6. Task tracker

### Phase 1 — The server ✅ 2026-09-09
- [x] `rmcp` dependency, MSRV bump, licence check recorded (§8, corrected)
- [x] `src-tauri/src/mcp.rs`: the four tools over `schema.rs`
- [x] Streamable HTTP on 127.0.0.1, started and stopped by a command
      (the *setting* that calls it is Phase 3)
- [x] Token in the keychain; `Origin` and bearer checks with their own tests
- [x] `tests/mcp_http.rs`: seven tests over a real socket — pulled forward from
      Phase 4 because a listening socket should not be committed unproven

### Phase 2 — Reaching the editor ✅ 2026-09-09
- [x] `put_query` emits to the frontend; a new tab, focused, never run
- [x] External tabs are marked, and the mark survives a session restore
- [x] `tests/ui/mcp.spec.ts`: 9 tests, both engines

### Phase 3 — Settings and discoverability ✅ 2026-09-09
- [x] Settings → *Integrations*: switch, port, token (show / copy / regenerate)
- [x] The exact client config snippet, copyable, with the URL and token in it —
      both forms, checked against Claude Code's own documentation
- [x] 10 more tests in `tests/ui/mcp.spec.ts`, and one more socket test for the
      property regeneration actually has to have

### Phase 4 — Proving it
- [x] Unit tests: tool list, auth, origin (11, in `mcp.rs`)
- [x] A live test that drives the server over HTTP as a client would
      (`tests/mcp_http.rs`, done in Phase 1)
- [ ] The schema shape each tool returns, against a live MySQL and Elasticsearch
- [ ] M1-M7 by hand

---

## 7. Risks

- **A listening socket is a new class of bug for this codebase.** Mitigated by
  §3.2 and by there being nothing to execute; not eliminated. This is the reason
  the stage exists as a stage rather than as a commit.
- ~~**`rmcp` 3.x requires Rust 1.88** and we declare 1.82.~~ Done, in the same
  commit as the dependency. Note the trap it hides: until the declaration moved,
  `cargo add` silently installed 2.2.0 instead — see §8.
- ~~**Two `schemars` versions** will be in the tree.~~ Three already were,
  before this stage. It did change the licence manifest, Stage 12 §17's guard
  did say so, and §8 records the fix.
- **The MCP spec revises.** Pin `rmcp` and record the protocol version the tests
  negotiate, so a client that speaks a newer one fails loudly rather than half-working.
- **An agent proposing a destructive query** is the residual risk, and it is the
  same one the assistant already carries: `DROP TABLE` in a tab is text until
  someone runs it. The provenance mark exists so it is obvious where it came from.

---

## 8. What it costs — measured on the way in

**The figure planned above was wrong, and the correction is instructive.** It
said 10 crates, with `schemars` and `ref-cast` among them. It was measured
against `rmcp` **2.2.0**, because `cargo add` had silently resolved down from
3.2.0 to respect the then-declared `rust-version = "1.82"` — it says so in a
warning that is easy to read past:

    warning: ignoring rmcp@3.2.0 (which requires rustc 1.88) to maintain
    db-query's rust-version of 1.82

Bumping the declaration first and re-measuring gives the real number.

**12 packages, on a tree that goes from 596 to 608.** Every licence below read
from the crate's own vendored `Cargo.toml` on 2026-09-09, not from memory:

| Package | Licence | Comes from |
|---|---|---|
| `rmcp` 3.2.0, `rmcp-macros` 3.2.0 | Apache-2.0 | the SDK |
| `darling`, `darling_core`, `darling_macro` 0.24.1 | MIT | `rmcp-macros` |
| `schemars_derive` 1.2.2 | MIT | `rmcp`'s tool schemas |
| `serde_derive_internals` 0.30.0 | MIT OR Apache-2.0 | `schemars_derive` |
| `futures` 0.3.34, `pastey` 0.2.3, `sse-stream` 0.2.6, `base64` 0.23.1 | MIT OR Apache-2.0 | `rmcp` |
| `httpdate` 1.0.3 | MIT OR Apache-2.0 | hyper's **server** half |

Every one is in `about.toml`'s `accepted` list already.

Three things the plan expected that did not happen. `schemars` itself is **not
new** — 0.8.22, 0.9.0 and 1.2.2 were all in the lockfile before this stage, so
the "two versions will coexist" risk was already three versions of reality;
`rmcp` links the 1.2.2 that was there. `ref-cast` was likewise already present.
And the HTTP layer — hyper, hyper-util, http-body-util, bytes, tokio-util,
rand — cost **one** package between them, `httpdate`, because everything else
was already linked through `reqwest`.

### The licence manifest did change, and the guard caught it

Exactly as this plan predicted, and worth writing down because it is Stage 12
§17 doing its job rather than a surprise. `schemars` reaches the binary for the
first time in this stage, and cargo-about scanned it as **source code** — an
"MIT License" heading over a block of `pub fn`. `npm run attribution` refused to
write the file and named the defect.

Fixed with a `[schemars.clarify]` naming its real `LICENSE`. One entry covers
all three versions: a clarification is keyed by crate name, and all three ship a
byte-identical file, so the single checksum matches each of them.

---

## 9. What Phase 1 actually built, and what it decided

`src-tauri/src/mcp.rs` (~700 lines with its tests), `src-tauri/tests/mcp_http.rs`
(7 tests over a real socket), six commands in `lib.rs`, and `McpState` beside
`AppState`. Nothing is on: no frontend calls any of it yet, which is Phase 3.

**The four decisions worth arguing with later:**

**1. The active connection lives in `McpState`, not `AppState`.** `session.rs`
opens with a heading that says *there is no "active connection" here*, and the
reason is good: a command that resolves its own target can be raced onto the
wrong server by a UI switch mid-query. This stage needs exactly that ambient
fact, so it is held **where nothing that executes can reach it** — a `Focus`
inside the MCP server's own state, read by the three read-only tools and by
`put_query`, which writes text. Every existing command still names the
connection it acts on. The frontend pushes it with `mcp_set_focus`.

**2. `start_with_token` exists so the socket tests need no keychain.** `start`
reads the token and refuses to bind without one — a machine with no credential
store should fail closed rather than listen open. But a test of the lock that CI
cannot run is not a test of the lock, so the token is injectable and
`tests/mcp_http.rs` runs everywhere, unignored, needing no database, no keychain
and no network.

**3. `Tools` is generic over the Tauri runtime.** Only so the tests can drive it
against `tauri::test::mock_app()`. `Clone` is written out by hand rather than
derived, because `#[derive(Clone)]` would demand `R: Clone` and a runtime is not.

**4. `rmcp` validates `Origin` only if you ask it to.** Its
`StreamableHttpServerConfig::allowed_origins` defaults to an **empty list, which
means no validation at all** — so accepting the default would have silently
skipped the spec's one MUST for this transport. Both are set now: our own `gate`
refuses first, and rmcp refuses again. That was verified rather than assumed —
deleting our check leaves the 403 in place and changes only the message, which
is why `a_foreign_origin_is_refused_even_with_the_right_token` asserts the
wording and says in a comment why.

**Proving the tests can fail.** Two were confirmed red before being trusted:
making `stop` a no-op fails `off_means_nothing_is_listening` and
`restarting_releases_the_previous_port`; removing the `Origin` check fails the
origin test. The tool-list test over the wire caught its own first draft — it
searched the raw body for the word "execute", which appears in every
description, in the sentence *promising the query is not executed*. It parses
tool names now.

**Still open, and deliberately:** the port default (49731, from IANA's
dynamic/private range) is a guess about what is free on someone's machine, which
is why it is configurable. `put_query` emits its event today and nothing
listens — a tool that reports success into the void until Phase 2 lands. And
`rust-version` moved 1.82 → 1.88: a declaration catching up with what mise and
CI have always built with (`stable`), not a toolchain change.

---

## 10. What Phase 2 built

`put_query` now reaches the editor. One Tauri event, `mcp://put-query` — the
app's **first**, which is why `listen` had to be imported at all — carrying the
SQL and the connection it was aimed at.

**The connection id travels with the payload rather than being assumed.** A
query aimed at one server must not land in another's workspace because the user
switched while the agent was thinking, and if that connection has closed the
query is *reported* rather than dropped: the client has already been told it was
placed, so silence is the one outcome nobody can see.

**Provenance is stored, not just displayed.** `StoredTab.external` in
`workspace.rs`, `#[serde(default)]` so every session file written before this
stage loads unchanged and means "the user opened this" — a compat detail with
teeth, since getting it wrong would put a provenance mark on everybody's
existing tabs on the first launch of a new build. Two Rust tests and one UI test
hold that line.

**The mark clears on Save.** Once the buffer is a file you chose a name and a
place for, "where did this come from" has an answer, and keeping the mark would
make it decoration rather than information. This is a judgement call and the one
most likely to be revisited.

**Rejected: inserting into the current tab.** Even when it is empty. A new tab
is the only shape where the arrival is visible, and visibility is the whole
mechanism — §3.4 exists because a tab that appears unbidden must not look like
one you opened.

**Six tests were confirmed able to fail.** Breaking the session round-trip fails
"the mark comes back after a restart"; breaking the clear-on-save fails its own
test. Both were checked rather than assumed.

Still unreachable by a user: nothing starts the server. That is Phase 3.

---

## 11. What Phase 3 built

Settings → **Integrations**. A switch, a port, the token, and the two snippets
that connect a client — which is what turns three phases of plumbing into a
feature someone can use.

**The switch is driven by the backend, never by the checkbox.** Every attempt
repaints from the `McpStatus` that came back, so a port that will not bind
leaves the box *unticked* and says why. A switch reading "on" while nothing is
listening is worse than no switch, and it is the failure this arrangement is
shaped around. `a_port_that_will_not_bind_leaves_the_switch_off` is the test;
writing it also caught that Playwright's `check()` is the wrong tool for it,
since `check()` asserts the box ends up ticked.

**The token is minted when the server starts, not when the pane opens.**
Creating a credential because somebody looked at a settings tab is a surprise
nobody asked for, so the whole token block is hidden until something is
listening. Masked by default, with *Show*.

**Regenerating restarts the listener, in Rust.** `start` reads the token once
and holds it for the listener's life, so minting a new one without a restart
would leave the *old* token in force while Settings displayed the new one — a
security control that reports success and changes nothing. It is a correctness
property rather than a UI nicety, so it lives in `mcp_regenerate_token` rather
than in the click handler, and
`a_restart_with_a_new_token_stops_accepting_the_old_one` drives a real socket
to prove it. Confirmed red by dropping the restart.

**The snippets were verified, not remembered.** Fetched from Claude Code's own
documentation: `--transport http` on the command line, `"type": "http"` in JSON,
the token as an ordinary `Authorization` header. A test parses the rendered JSON
and compares the object rather than matching a substring, so a change in shape
fails rather than a change in whitespace.

**The default port comes from Rust**, through `app_defaults`, joining
`browseLimit` and `maxRows`. The stored preference is a `0` sentinel meaning
"ask", so the number Settings shows and the number `mcp.rs` binds cannot drift.
A stored port outside 1024–65535 falls back to the sentinel and self-heals.

**A fresh install starts nothing**, and the server only comes back at boot if it
was explicitly left on — both asserted.

**A failure at boot is not the same as a failure you watched.** Turning the
switch on and seeing it fail records "off": you saw it, and next launch should
not surprise you. A *boot* that cannot bind reports it in the results pane and
**keeps the preference**, because a port that happens to be busy this morning
must not quietly turn a feature off forever. One flag, two behaviours, and a
test for each.
