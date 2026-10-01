# Stage 20 — The assistant through a CLI

**Status:** 📋 Planned — 2026-10-01. Nothing built yet. **Phase 0 is
measured** (§6) and changed the design: the tool-stripping flag Copilot
documents is silently ignored, so the app checks the tool set rather than
trusting it.

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

## 6. Phase 0 — measured, 2026-10-01

`copilot` 1.0.91 and `claude` 2.1.283, both signed in on this machine, every run
in an empty scratch directory. Four invocations of Copilot and one of Claude;
the raw JSONL is not committed, the numbers below are read from it.

| | **Claude Code** | **Copilot CLI** |
|---|---|---|
| Deltas arrive as they are produced | yes | **yes** — `assistant.message_delta` / `deltaContent` |
| Tool set can be emptied | `--tools ""` → `tools: []` | only by a trick (§6.2) |
| **The CLI states what it sent** | `system`/`init` carries `tools: []`, `mcp_servers: []` | `session.usage_checkpoint` carries `tool_count`, `tool_tokens` |
| Our own system prompt | `--system-prompt-file` | **no** — §6.3 |
| Ignore the person's config | `--bare` | `--no-custom-instructions`, `--disable-builtin-mcps` |
| Trivial prompt, wall clock | **1.5 s** | 5.3–11.7 s, first line at ~2.5 s |
| Not signed in | `claude auth status`, exit 0/1 | exit **1**, `Error: No authentication information found.` |

### 6.1 Copilot streams, and the event stream is good

`--output-format json` is JSONL emitted as things happen, not dumped at the
end: for a twenty-line answer the `assistant.message_delta` lines arrived
spread over ~220 ms, between `assistant.message_start` and `model.call_finished`.
`deltaContent` is the text. So **both recipes stream** and §4.3's
non-streaming fallback is not needed for either.

The stream also carries `session.mcp_servers_loaded`, `model.call_start`,
`session.usage_checkpoint` and a final `result` with `exitCode` and
`premiumRequests`. More than we need, and all of it useful for the logbook.

### 6.2 `--available-tools=` is silently ignored — and that is the headline

Asking for an empty tool set **does nothing**. The run went out with
`tool_count: 21` — `bash`, `create`, `edit`, `web_fetch`, `task` and the rest —
at `tool_tokens: 7289`. The flag is not rejected, it is disregarded, so the
mistake looks exactly like success.

What the flag *does* do, with a real value, is work exactly as documented, and
it is the **single lever that closes everything**: it filters MCP tools too, so
there is no need to chase `--disable-mcp-server` for each server the person has
configured — including, as §4.2 warns, possibly this app's own.

| `--available-tools=` | `tool_count` | `tool_tokens` | `prompt_tokens` |
|---|---|---|---|
| *(empty)* | 21 | 7289 | 11013 |
| `view` | 1 | 203 | 1836 |
| `__db_query_none__` (no such tool) | **0** | 0 | 1589 |

Zero is reachable — by naming a tool that does not exist. It still answers
normally. But **that is a trick, not a contract**: nothing documents it, and if
Copilot ever validates the name it breaks *open*, handing the model back its
tools, which is the wrong direction to fail in.

So the invariant is not trusted, it is **checked, every message**: both CLIs
report their own tool set, so the recipe reads it back —
`tool_count` from `session.usage_checkpoint`, `tools: []` from Claude's `init` —
and anything other than empty is a bug the app surfaces rather than a silence
it keeps. Milestone C5 becomes an assertion in the code, not only a hands-on
check.

### 6.3 Copilot's system prompt is not ours to replace

There is no equivalent of `--system-prompt-file`. With zero tools the request
still carried 1589 prompt tokens of Copilot's own instructions, in segments it
names itself — `customized_identity_preamble`, `code_change_instructions`,
`tone_and_style`. We can add our rules to the prompt text; we cannot stop it
being a coding assistant underneath.

That is acceptable — our rules are about what it must not claim to be able to
do, and nothing in its own prompt contradicts them — but it is a real
difference from Claude, where `--system-prompt-file` means `system_prompt(...)`
can be handed over verbatim. Untested and worth one probe later:
`--agent`, which may be able to carry our instructions as a custom agent.

### 6.4 Cold start is the UX problem

Claude answered a trivial prompt in **1.5 s** end to end. Copilot took 5.3 s on
a warm run and 11.7 s on the first, with nothing at all on stdout for the first
~2.5 s. A chat that sits blank for two and a half seconds reads as broken —
the same lesson as the connection spinner. The recipe therefore reports
*starting* as a state of its own, before the first delta, rather than letting
an empty bubble stand for it.

### 6.5 What a message costs on Copilot

One **premium request** per invocation, and `--max-ai-credits` can cap a
session. On these free credentials `availableModels` held only
`mai-code-1.1-flash`, chosen by auto-routing. Cutting the tool schemas out
takes a trivial message from 11,013 to 1,589 prompt tokens — so §6.2 is a cost
fix as much as a safety one.

Also worth passing: `--no-auto-update`, so the CLI does not silently download a
new version underneath a recipe that was measured against this one.

### 6.6 Still unanswered

* Whether `--agent` can carry our system prompt (§6.3).
* What `--acp` offers. Copilot CLI can run as an **Agent Client Protocol**
  server, which is a real JSON-RPC protocol rather than a stdout format. If it
  suits, it is a better integration than parsing lines — and it is how editors
  already talk to agents. Worth a look before Phase 1 is written.

## 7. Task tracker

### Phase 0 — Measure (§6) — ✅ done 2026-10-01

- [x] Install and sign in to `copilot` — 1.0.91, free credentials
- [x] Record both CLIs' streaming, tool stripping, auth failure and cold start
- [ ] `--agent` as a system-prompt carrier (§6.6)
- [ ] Assess `--acp` before Phase 1 is written (§6.6)

### Phase 1 — One provider, rebuilt each turn

- [ ] `Provider::LocalCli` + a `Recipe` table (binary, args, parser, streams?)
- [ ] Spawn with an empty cwd and a clean environment; stream stdout
- [ ] **Read the tool set back and assert it is empty, every message** (§6.2)
- [ ] A `starting` state before the first delta (§6.4)
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
