# Stage 20 — The assistant through a CLI

**Status:** 📋 Planned — 2026-10-01. Nothing built yet. Two facts must be
measured before any code is written (§6).

---

## 1. The use case, in the user's words

> *"Wouldnt it be possible that our assistant connects to cli? That way we could
> use copilot cli for example and the app can communicate with it wouldn't that
> be a beeter integration than trying to use tokens?"*

And, on why this is not the MCP server we already have:

> *"the goal is be able to explain or interact giving the correct context of our
> connection. So mcp is basically when we are working through another tool like
> a code IDE where we might want it to check a query. While the assistant is to
> work directly on DB that's why it will be useful a way to interact with the
> CLI directly from there."*

Two different jobs, and the app should do both:

| | Who drives | Where the schema comes from | Where the SQL lands |
|---|---|---|---|
| **MCP server** (Stage 13) | an outside tool — an IDE, a CLI | our four read-only tools | `put_query` opens a tab here |
| **The assistant** (Stage 10) | the person, in this app | the cached schema in the system prompt | the chat, for them to copy or run |

Today the assistant needs an API key and bills per token. A CLI the person is
already signed into needs neither.

## 2. What the assistant is, so that a CLI can stand in for it

`assistant.rs` is already a provider adapter, and a narrow one — which is what
makes a third provider cheap:

* **One streamed completion per message.** No tool-use loop, because the model
  gets no tools: *"You CANNOT run anything. You have no tools and no
  connection."* The app's oldest rule is that the person at the keyboard
  presses run, and the way that rule is kept is that there is nothing to erode.
* **Two providers** behind one `Provider` enum: Anthropic `/v1/messages`, and
  anything speaking `/chat/completions` — which is how the Ollama fixture works
  with no key at all.
* **The schema goes in the system prompt**, rendered compactly by
  `system_prompt(...)`, so the model is not guessing table names.

A CLI provider has to deliver the same four things: a system prompt we wrote,
the conversation, a stream of text back, and **no tools**.

We already spawn a CLI and parse its output — `flywaycli.rs` does exactly that
for `flyway … -json`. This is not a new class of capability for the app.

## 3. What the two CLIs actually offer

Read from the vendors' own documentation on 2026-10-01, not from memory.
`claude` 2.1.283 is installed on this machine; `copilot` is not.

| | **Claude Code** | **Copilot CLI** |
|---|---|---|
| Non-interactive | `-p "query"` | `-p PROMPT` |
| Machine-readable out | `--output-format stream-json` | `--output-format=json` (JSONL) |
| Token-level stream | `--include-partial-messages` | not documented — **§6.1** |
| Our own system prompt | `--system-prompt-file` | not documented — **§6.2** |
| Close the tool set | `--tools ""`, plus `--disallowedTools "mcp__*"` | `--available-tools=TOOL` (allowlist), `--excluded-tools`, `--deny-tool` |
| Ignore the user's own config | `--bare` (no CLAUDE.md, hooks, skills, MCP) | not documented — **§6.3** |
| Never ask the user anything | `--permission-prompts none` | `--no-ask-user` |
| Bound one answer | `--max-turns 1`, `--max-budget-usd` | — |
| Quiet, answer only | — | `-s` |
| Pick a model | `--model` | `--model=MODEL`, `COPILOT_MODEL` |
| Installed / signed in? | `claude auth status` — JSON, exit 0/1 | **§6.4** |
| Redact secrets from logs | — | `--secret-env-vars=VAR` |

Claude Code documents `--restricted` for exactly this shape of caller — *"an
evaluation harness drives `claude` on a shared machine"* — and
`claude setup-token` for scripts on a subscription. Driving it headless is a
supported path, not a tolerated one.

**A correction worth recording.** My first pass concluded Copilot CLI had no
machine-readable output and that turning its tools off would mean maintaining a
blacklist. Both were wrong: I had read its README and its how-to page, neither
of which covers programmatic use, and stopped there. It has a *programmatic
reference* of its own, with `--output-format=json` and `--available-tools` —
an allowlist, which is the right shape. Not finding a thing is not the same as
a thing not existing, and a vendor's docs have more than one index.

### 3.1 Licences

* **Copilot CLI** — `LICENSE.md`, read 2026-10-01: a bespoke *GitHub Copilot
  CLI License*. Distributed **only in unmodified form**, never *"on a standalone
  basis or as a primary product"*, and redistributable only as part of an
  application with *"material functionality beyond the Software itself"*, with
  the licence and notices retained. We invoke the person's own installation, so
  this is use, not redistribution — **and it means we must never bundle it**.
  The licence governs the software, not the GitHub service behind it, which has
  its own terms.
* **Claude Code** — not vendored either, same reasoning.

Neither becomes a dependency of this repository. Both stay something the person
installs, like Flyway.

## 4. The design

### 4.1 A provider, not a command box

A third `Provider::LocalCli`, chosen in Settings beside Anthropic and
OpenAI-compatible. No key, no base URL.

**Recipes, not a free-text command.** A "run this command" setting in a
database client is an arbitrary-code-execution feature wearing a different
label. Instead the app ships one recipe per supported CLI — the binary name,
the argument list, how to read its output — and Settings offers that closed
list. Adding a CLI is a commit, which is the point.

### 4.2 Every recipe runs the CLI with nothing in reach

The same four rules, per recipe:

1. **No tools.** Claude: `--tools ""` and `--disallowedTools "mcp__*"`.
   Copilot: `--available-tools` holding nothing but what we need, which is
   nothing (**§6.2**).
2. **None of the person's own agent configuration.** Claude: `--bare`. This
   matters more than it looks: *this app's own MCP server may be configured in
   their Claude Code*, so without `--bare` the assistant could reach
   `put_query` and the rest — the app talking to itself through a side door.
3. **An empty working directory.** Both are coding agents that read their cwd
   by default. We spawn them in a scratch directory, never in the person's
   project.
4. **A clean environment.** No inherited secrets. The database password is in
   the OS keychain and never in our environment anyway, which is what makes
   this cheap.

### 4.3 Streaming, or honestly not streaming

The chat pane renders deltas. Where a CLI streams, we render as it arrives.
Where one does not, the recipe is marked non-streaming and the chat shows the
answer arriving whole rather than faking a cursor. A pretend stream is a lie
about what the app knows.

### 4.4 Multi-turn: rebuild now, resume later

> *"For now as a POC we can rebuild on each invokation but we might have to
> target the holding of a session-id as ideally that will reduce token usage
> isn't?"*

Agreed, and that is the order. **Phase 1 rebuilds** the whole conversation into
each invocation: stateless, identical to what the HTTP providers already do,
and nothing new can go wrong with it.

**On "state we can't see" — that was too strong.** With a session id the
transcript is perfectly inspectable: Claude Code stores it as JSONL on disk and
`--resume` accepts either the id or the transcript path. Three real things stay
true, and they are what make resume a Phase 2 rather than a Phase 1:

* **The CLI owns compaction.** It decides when to compact and what survives, so
  after a long chat the model's context no longer matches the message list our
  pane is showing. Our history and its history drift, silently.
* **Resume needs persistence on.** `--no-session-persistence` exists precisely
  because a persisted session writes the conversation — our schema dump and the
  person's questions — to a store outside this app, under `~/.claude`. This app
  keeps its own history on purpose; a second copy somewhere else is a decision,
  not a detail.
* **Resume can fail** — session pruned, CLI updated, different directory. So
  the rebuild path has to exist anyway, as the fallback. Building it first is
  therefore free.

And the token saving is real but worth naming precisely: on a *subscription*
the person is not billed per token, so what resume saves is their usage
allowance rather than money.

One cheap win available in Phase 1, without any session state: Claude Code
splits a custom system prompt at a line containing only
`__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__`, keeping everything above it
prompt-cached while what is below changes. Our system prompt is exactly that
shape — fixed rules above, this connection's schema below.

## 5. Milestones

| | Milestone | How it is checked by hand |
|---|---|---|
| C1 | The assistant answers with no API key configured | Settings → Assistant → a CLI recipe; ask a question; an answer streams in |
| C2 | A CLI that is not installed says so | Rename the binary on PATH; the chat says it is not installed, not "failed" |
| C3 | A CLI that is not signed in says so | Log the CLI out; the chat says to sign in, and how |
| C4 | The schema reaches it | Ask "what columns does X have"; the answer names real columns without a tool call |
| C5 | **It cannot run anything** | Ask it directly to run a query; it refuses. Check the logbook: no statement was executed |
| C6 | **It cannot reach our MCP server** | Turn the MCP server on, configure it in the person's own CLI, then ask the assistant to use it; it has no such tool |
| C7 | Copilot and Claude both work | The same question on both recipes |
| C8 | Cancelling stops it | Ask something long, press Cancel; the process is gone (`ps`), the chat says cancelled |

## 6. What must be measured first

Each of these decides a design detail, and none of them is in the
documentation. Measuring them is Phase 0 and needs `copilot` installed and
signed in — which is the person's call, since it is their machine and their
subscription.

* **6.1 Is `--output-format=json` incremental?** JSONL one-object-per-line
  suggests events as they happen, but if the lines arrive only at the end the
  Copilot recipe is non-streaming (§4.3) and the UI has to say so.
* **6.2 Can `--available-tools` be empty, and can the system prompt be
  replaced?** If neither, Copilot's recipe cannot be given our prompt or
  stripped of its tools, and we would have to carry our rules in the user
  prompt instead — weaker, and worth knowing before it is designed around.
* **6.3 Is there a `--bare` equivalent?** Without one, the person's own
  `copilot-instructions.md`, agents and MCP servers load into our assistant.
* **6.4 How is "not signed in" reported** — exit code, stderr, a JSON line?
  C3 depends on telling that apart from a real failure.
* **6.5 Cold-start cost.** A process per message, measured, both CLIs. If it is
  seconds, the chat needs to say "starting" rather than look hung — the same
  lesson as the connection spinner.

## 7. Task tracker

### Phase 0 — Measure (§6)

- [ ] Install and sign in to `copilot` (needs the person's go-ahead)
- [ ] Record §6.1–6.5 for both CLIs, with the command and its raw output

### Phase 1 — One provider, rebuilt each turn

- [ ] `Provider::LocalCli` + a `Recipe` table (binary, args, parser, streams?)
- [ ] Spawn with an empty cwd and a clean environment; stream stdout
- [ ] Claude recipe
- [ ] Copilot recipe, shaped by Phase 0
- [ ] Settings: the closed recipe list, no key field, no base URL
- [ ] Not-installed and not-signed-in states (C2, C3)
- [ ] Cancel kills the process (C8)

### Phase 2 — Proof

- [ ] Unit tests on argument construction per recipe — the tool-stripping flags
      are the ones that must never regress
- [ ] Unit tests on each output parser, including a malformed line mid-stream
- [ ] A fake CLI on PATH for the UI suite: a script that emits a known stream,
      so the tests need neither vendor nor a subscription
- [ ] Live, by hand: C1–C8

### Phase 3 — Resume (optional, measured)

- [ ] A session id per chat, `--resume` with rebuild as the fallback
- [ ] Decide the second-copy question in §4.4 before turning persistence on
- [ ] Measure what it actually saves before keeping it

## 8. Decisions

### 8.1 Why not just use the MCP server for this?

Because the person is in this app, looking at this connection, and wants to ask
about it here. MCP answers the other direction — an outside tool asking us. The
two overlap in subject and not in use.

### 8.2 Why recipes rather than a configurable command?

Because the difference between "a setting that holds a command line" and
"arbitrary code execution, configured" is only who typed it. A closed list of
recipes is a feature; a text field is a vulnerability with a label.

### 8.3 Why keep the HTTP providers?

They are what the Ollama fixture tests, they are what works with no CLI
installed, and they are what works on a machine where neither vendor's CLI
exists. A CLI recipe is another way in, not a replacement.
