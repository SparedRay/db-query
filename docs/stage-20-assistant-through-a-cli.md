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

> **Superseded by §9.3** — a warmed, long-lived process turns out to give
> continuity *without* either of the options weighed below. The reasoning here
> is kept because it is what rules `--resume` out, and §9.3 depends on it.

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

## 9. Enabled per integration, warmed at startup — 2026-10-01

> *"what if we select on integrations which one we want to enable (That way
> user has control over which one they need) and then we condition the
> assitant button wit a loader. when enabled on start up it will warm the
> tool. If user needs it is already there, if not (Or disable) we don't add
> anything."*

Right on both counts, and it matches what the app already does: the MCP server
is off until switched on, and anything slower than an eyeblink says so. Nothing
is spawned, probed or shown for an integration that is off.

### 9.1 What "ready" can honestly mean, per CLI

Warming is only worth having if the light it turns on is true, and the two CLIs
differ in what can be learned for free:

| | **Claude Code** | **Copilot CLI** |
|---|---|---|
| Installed | binary on PATH | binary on PATH |
| Signed in | `claude auth status` — JSON, exit 0/1, **free** | **not knowable for free** |

Copilot has no `auth status`. `copilot login` is an interactive OAuth flow; the
token lives in the system credential store, or falls back to plaintext under
`~/.copilot/`; and the subcommands that exit 0 do so while signed out too
(measured: `mcp list`, `instruction list`, `skill list` all exit 0 with an empty
`HOME`). Sign-in is learned only by sending a real prompt, which costs a premium
request.

Reading Copilot's own credential-store entry is **not** the answer. This app's
rule is that it stores its secrets in the OS keychain and reads only its own;
going through another application's entry to infer a boolean trades that for a
convenience.

So the states the button can show are:

* **off** — not enabled. Nothing drawn, nothing spawned.
* **starting** — the probe is in flight. This is the loader.
* **ready** — Claude: installed *and* signed in. Copilot: installed, sign-in
  unverified, and the tooltip says so rather than implying otherwise.
* **not installed** — names the binary it looked for.
* **not signed in** — Claude at startup; Copilot only after a question has
  been asked. Says how to fix it.

And in Settings, a **Test** button that spends one premium request and
**says so in its own label** before it is pressed. The app does not spend the
person's quota to light a lamp they did not ask for.

### 9.2 What warming can and cannot buy

Measured in §6.4: Copilot's first run took 11.7 s and its warm runs 5.3 s. So
roughly **6 s is one-time** — token refresh, update check — and **~5 s is per
message**, and stays however warm the process is. Warming makes the first
question as fast as the fifth. It does not make either fast, and the UI must
not promise that it does: the *starting* state of §6.4 is still needed on every
message.

Two rules, from the same lesson the connection spinner taught:

* The probe is **asynchronous and never blocks startup**. An assistant that is
  not ready is not an app that cannot open.
* A failed probe is **quiet but visible** — the button says what it found, and
  nothing is thrown in the person's face, exactly as the update check behaves.

### 9.3 Warming and multi-turn are the same mechanism

This is the part worth taking from the suggestion. "Warm the tool" implies a
process that *stays*, and a process that stays is also the answer to §4.4:

`claude -p --input-format stream-json` holds one process open and reads turns
from stdin. Spawned at startup it makes no API call until a message arrives, so
warming is **free** — and because every turn goes down the same pipe, the
conversation continues without `--resume`. That sidesteps all three objections
in §4.4 at once: we still own the message list, nothing is persisted outside
this app, and there is no session id to go stale. Compaction is still theirs,
but a chat tab is short-lived and we can restart the process rather than let it
drift.

**For Copilot this is unknown.** Its `-p` exits after completion, so a
long-lived process needs `--acp`, the Agent Client Protocol server mode
(§6.6) — which is JSON-RPC and designed for exactly this. Until that is
probed, Copilot's "warm" means nothing more than the binary check, and its
recipe is one process per message.

So §6.6 stops being curiosity and becomes a dependency: **`--acp` is probed
before Phase 1 is written**, because it decides whether Copilot gets one
mechanism or two.

### 9.4 Tracker

- [ ] Settings → Integrations: a row per CLI, off by default
- [ ] An async readiness probe per enabled integration, run at startup
- [ ] The five button states of §9.1, with honest tooltips
- [ ] A **Test** button whose label says it spends a request
- [ ] Probe `--acp` (§9.3) — decides Copilot's process model
- [ ] A long-lived process per chat where the CLI supports it, with an idle
      timeout and a respawn when it dies
- [ ] UI tests: a fake CLI on PATH covers ready, missing and signed-out without
      a vendor or a subscription


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

## 10. Phase 1, the backend — built 2026-10-01

`agentcli.rs`: the recipes, the parser, the process runner and the readiness
probes. 18 unit tests, 4 live tests (`mise run test-cli`), 414 Rust tests in
all. The UI is not built yet.

Three things the plan got wrong, found by building it:

### 10.1 `--bare` breaks authentication

It looked exactly right — it skips hooks, skills, CLAUDE.md and MCP discovery
in one flag, which is §4.2's whole wish list. With it, every run returns
`Not logged in · Please run /login` and `duration_api_ms: 0`, while
`claude auth status` says the account is signed in. Without it, the same call
works.

So the person's configuration **does** load, and the tool flags carry the whole
weight. Which turned out to matter: with `--disallowedTools "mcp__*"` the tool
list came back empty but the *server* list did not — four claude.ai connectors
had loaded anyway. No tools were exposed, so the rule held, but it held for a
different reason than the flag implied. `--strict-mcp-config` with an empty
config is now passed as well, and the `init` readback is what will say whether
it worked.

### 10.2 A clean environment would break both CLIs

§4.2 promised `env_clear()`. It reads as the careful choice and it is the wrong
one: both CLIs authenticate out of the person's own session — Claude from under
`HOME`, Copilot from the OS credential store, which on Linux needs the session
D-Bus address. Clearing the environment does not harden the call, it stops it
working.

Giving that up is cheap because there is nothing of ours to leak: a database
password lives in the keychain and is read at the moment it is used, never
parked in our environment. The empty working directory, which is the part that
keeps a coding agent out of the person's project, stays.

### 10.3 The invariant, proven against the real CLI

The live test asks a real question and asserts the answer did not fail —
because a non-empty tool set is *how* a reply fails. Falsified on 2026-10-01 by
switching the recipe to the documented `--available-tools=` form: the real CLI
offered 21 tools, the readback named every one of them in the refusal, and the
test failed. That is the central safety claim of this stage, demonstrated
failing when broken rather than asserted.

Cancel is proven the same way — a cancelled reply returns in ~2.5s with no
`Done` event, because the process was killed rather than waited out — and the
`Started` event earns its place with numbers: it arrived at 2.3s against a
first token at 3.8s, so it covers 1.4s that would otherwise be an empty bubble.

### 10.4 Still to build

- [ ] Settings → Integrations, and the assistant button's five states (§9.1)
- [ ] UI tests with a fake CLI on PATH
- [ ] A long-lived process per chat (§9.3) — now known to be possible for both,
      since Copilot's ACP mode is real and answers a handshake

## 11. Phase 1, the UI — built 2026-10-01

Settings → Assistant → **Integrations**: one row per CLI, off by default, each
saying what its probe found. Enabling one warms it at startup and adds it to the
provider list; disabling one removes the option and, if questions were going
there, moves the provider back. The assistant button carries the result.

### 11.1 Decisions the build settled

* **`localCli` is not a Rust `Provider`.** It has its own command and needs
  neither key nor URL, so widening the Rust enum would have added a variant
  whose `key_id` and `endpoint` mean nothing. It is a TypeScript-side
  `AiProvider` instead, and the branch happens where the config is read.
* **The key and base-URL fields are hidden as a group** when a CLI is selected.
  Left on screen they read as optional rather than irrelevant.
* **The assistant button is never disabled**, however bad the probe's news.
  Opening the panel is how you read *why* it is not ready, so taking the click
  away would hide the explanation behind the thing needing explanation. It gets
  a mark and a tooltip; the panel's own Send stays disabled, which is where
  refusing to send actually belongs.
* **Send becomes Stop for a CLI reply**, for the whole time one is in flight —
  including the seconds before its first word, which is exactly when someone
  wants it back. The HTTP path shows no Stop, because it has nothing to kill.

### 11.2 A correction to §9.4

The plan called for "a fake CLI on PATH" in the UI suite. It is not needed and
would have been worse: the probe and the reply are both backend commands, so the
existing stub covers them the same way it covers the HTTP provider — no vendor,
no subscription, no PATH manipulation, and the same tests on both engines. What
a fake binary would have tested is whether the *vendor's* output still parses,
and that belongs in `agentcli_live.rs`, where it now is.

### 11.3 Proof

13 UI tests, on both engines. Two falsified:

* Warming every CLI rather than only the enabled ones fails *"nothing is probed
  when no integration is enabled"* — which is the claim that an integration
  switched off costs nothing.
* Leaving Send disabled during a CLI reply fails *"Send becomes Stop…"*.

One app bug found by its own test: enabling an integration updated its note but
not the provider list, because the list was only redrawn on the disable path.

**826 UI tests on both engines, 414 Rust, `mise run check` clean.**

### 11.4 Left for Phase 3

- [ ] A long-lived process per chat (§9.3), now known to be possible for both

## 12. Phase 3 measured first — 2026-10-01

ACP's prompt flow, against the real Copilot CLI. Three findings, one of which is
a bug Phase 3 would have shipped.

### 12.1 The premise holds: a session is worth 3–9 seconds a message

| | one-shot (Phase 1) | ACP session (Phase 3) |
|---|---|---|
| start | 5.3–11.7 s **every message** | 2.4 s **once per chat** |
| first turn | — | 1.85 s |
| second turn | — | 2.66 s |

So the cost moves from per-message to per-chat, which is the whole point. The
flow is `session/prompt` → `session/update` notifications carrying
`agent_message_chunk` → a response with `stopReason` and a `usage` object. The
chunks are finer than the JSONL path's: single tokens, `"1"`, `"\n"`, `"2"`.

### 12.2 The CLI talks to the user inside the answer

The first two chunks of a turn were not the answer:

```
Info: Disabled tools: bash, create, dynamic_workflows_manage, edit, glob, grep,
      list_agents, list_bash, read_agent, read_bash, run_dynamic_workflow, sql,
      stop_bash, task, view, web_fetch, write_agent
Info: Unknown tool name in the tool allowlist: "__db_query_no_tools__"
```

They arrive as `agent_message_chunk` — the same channel as the reply — so
rendering the stream naively would open every answer with two lines of our own
plumbing. **Found before it shipped only because the flow was measured rather
than assumed.** The ACP path filters the known `Info:` forms, and a live test
pins their shape so that a vendor rewording breaks a test rather than quietly
leaking into the chat.

### 12.3 The invariant is weaker over ACP, and the design has to say so

The JSONL path reports its tool set as **data** — `tool_count` and the names, in
a usage checkpoint — which is what §10.3 falsified. ACP has no equivalent:
`usage_update` carries tokens only. What ACP gives instead is:

* that `Info: Disabled tools: …` line, which **names what was removed** — prose,
  but a positive statement rather than an absence, and it lists `bash`, `edit`,
  `create`, `sql` by name;
* `session/update` with `tool_call`, if the model ever calls one.

So the ACP path enforces the rule with three things rather than one: the same
flags, a refusal on any `tool_call`, and a check that the `Disabled tools:` line
is present and names the write-capable tools. That is **not as strong** as
reading a count back, and this is where it is written down rather than glossed:
the strong check lives on the one-shot path.

It also confirms the trick is visible to the vendor — *"Unknown tool name in the
tool allowlist"* means they already notice the name that empties the list, and
could validate it away. Which is the argument for the readback, not against it.

### 12.4 Scope

Phase 3 is the ACP transport for Copilot first, since that is the main driver.
Claude keeps the one-shot path until its `--input-format stream-json` transport
is built, and keeps the stronger check while it does.

## 13. Phase 3 — the kept session, built 2026-10-01

`acp.rs` holds a CLI open as an ACP session: JSON-RPC over its stdin and
stdout, `initialize` advertising **no filesystem capability**, `session/new`,
then one `session/prompt` per question. Copilot routes through it; Claude still
spends a process per message until its `--input-format stream-json` transport is
built, and keeps the stronger tool check while it does.

Measured, on this machine:

| | before | after |
|---|---|---|
| first question | 5.3–11.7 s | 3.7 s |
| every question after | 5.3–11.7 s | **1.4–1.8 s** |
| after a cancel | a cold start | 1.5 s — the session lived |

### 13.1 Three things the tests found that the design had wrong

**The `Disabled tools:` line is sent once per session, not once per turn.** The
guard demanded it every turn, so every turn after the first was refused —
caught by the live test, whose second question came back with its text and no
`Done`. The fact belongs to the session, so it is remembered there; what still
runs per turn is the refusal on any `tool_call`, which is the part that could
change mid-session.

**A cancel could be lost entirely.** `notify_waiters` wakes whoever is waiting
at that moment and is otherwise dropped, and starting a session takes
seconds — so Stop pressed during those seconds, which is exactly when someone
means it, vanished. The live test cancelled on the first event and then watched
a 15-second answer arrive in full. `CliCancel` now carries a flag beside the
`Notify`, the same two-part shape `session::cancellable` uses for a query, and
both CLI paths check it before spending anything. Falsified: removing the flag
check makes that test fail again.

**A scratch directory cannot be a local guard.** The one-shot path removes its
directory when the run ends, which is right. A session outlives the call that
started it, so the same guard would delete the working directory out from under
a running CLI. It is deliberately leaked, with a comment saying so.

### 13.2 Cancelling is now polite

Over ACP a cancel is a `session/cancel` notification: the turn ends with
`stopReason: "cancelled"` and **the session survives**. The one-shot path has to
kill the process, which costs the next question a cold start. The live test
asks again afterwards and expects a real answer, which is what makes this a
claim rather than a hope.

A session is dropped in only two cases, both deliberate: the system prompt
changed — which is how a different connection, a different schema, or a newly
expanded table is noticed, without enumerating them — or the turn failed, since
a broken session should not be the thing the next question starts from.

### 13.3 Proof

8 unit tests on the protocol's pure parts, including that an answer which merely
*mentions* disabled tools is still an answer, and two live tests:
**the second question skips the startup** (3.7 s then 1.8 s) and **a cancelled
turn leaves the session usable** (answered again in 1.5 s). Six live tests in
all, `mise run test-cli`.

**422 Rust tests, 826 UI on both engines, `mise run check` clean.**

## 14. Found in use — the prose guard refused a good answer, 2026-10-01

> *"For copilot I got this after the first question: Copilot CLI left tools
> available to the model (edit, create). … No response received"*

§12.3 said the ACP path's tool check was the weaker one and §13 shipped it as a
**hard refusal** anyway. On the first real question anybody asked, it stopped a
perfectly good answer: the `Disabled tools:` line that CLI produced did not name
`edit` and `create`, so a difference in the agent's *wording* became a refusal
with nothing to show for it.

Reproduced first, with the recipe's exact arguments, and the line came back
complete — `bash, create, dynamic_workflows_manage, edit, …` — so the cause is
not the flags. Something about that run produced a shorter list, and the honest
summary is that **we do not know what the line will say**, which is the whole
problem with having depended on it.

### 14.1 Where the guarantee actually lives

It was never the prose. A model cannot act without a `tool_call` reaching us,
and four things stand between it and that:

1. the allowlist flag, which empties the tool set;
2. no `--allow-all-tools`, so nothing is approved without being asked;
3. every request the agent makes of *us* is answered with an error — we
   advertised no capability, a permission request included;
4. any `tool_call` at all cancels the turn and refuses the answer.

So the `Disabled tools:` line is now **recorded, not enforced**: when it fails
to name a write-capable tool the logbook says so, with the list it did give, and
the answer continues. The refusal that matters stays exactly where it was.

### 14.2 What this cost, and the lesson

A user's first question, and their confidence that the feature works. The
mistake was not the check — corroborating a vendor's claim is reasonable — it
was making a *refusal* out of a signal already documented, two sections
earlier, as the weak one. **A guard built on prose should warn; only a guard
built on behaviour should refuse.**

Tests follow the change in meaning rather than being deleted:
`a_missing_write_tool_is_named_rather_than_fatal` pins that a short list is
reported by name and is not fatal, and `silence_reports_every_write_tool` pins
that saying nothing reports everything and still does not refuse.

Not covered by a test, and said plainly: the `tool_call` abort and the refusal
of agent-side requests are exercised only by the code path, because a CLI with
no tools cannot be made to call one. They are three lines each, and both would
be worth a fake ACP agent on stdio if this path grows.

**422 Rust tests, `mise run check` clean. The session path re-verified live:
3.8 s then 1.8 s.**

## 15. Two silences, filled — 2026-10-01

> *"we need a loader while is getting a response as right now is just … which is
> not very intuitive … Lastly, we also need a loader when we open asistant first
> time otherwise tha 1s seems like it did nothing"*

Both are the rule the rail and the tree already follow — anything slower than an
eyeblink says so — applied to the two places in the chat that did not.

### 15.1 Opening the panel waited for the probe

`open()` awaited the readiness check **before** `showModal()`, so clicking the
button did nothing visible until it answered: 2.5 s for Copilot's ACP
handshake. The panel now appears at once, says it is checking, and fills in —
with Send disabled until it knows, because appearing usable is a different lie
from appearing dead.

### 15.2 `…` is not a loader

A literal ellipsis is indistinguishable from a one-character answer and from a
panel that has died, and a CLI shows it for seconds. It is now a spinner and a
word that changes every 2.6 s.

The words are **vague on purpose** — *Thinking, Mulling, Sifting, Percolating*.
A specific one ("reading your schema", "checking the columns") would be a claim
about what the model is doing, and we do not know: all we know is that a process
has not answered yet. Three details that are not decoration:

* The next word is never the one before it. A list that repeats itself looks
  stuck, which is the one thing it exists to disprove.
* `role="status"` with a fixed label and the word `aria-hidden`: a screen reader
  should hear "waiting for a reply" once, not a new verb every few seconds.
* Under `prefers-reduced-motion` it shows one word and does not cycle. A word
  that changes is motion too.

It is a `<span>`, not a `<p>`: it goes inside `#chat-note`, which is already a
paragraph, and a paragraph inside a paragraph is not markup.

### 15.3 Proof

Four UI tests on both engines, two falsified: restoring the old open order fails
*"the panel opens before it knows whether the CLI is ready"*, and restoring the
bare `…` fails *"waiting for a reply shows a spinner and a word"*. The others
pin that `started` does **not** clear the line — the wait is not over, only its
owner has changed — and that a turn which fails before saying anything leaves no
spinner behind.

**834 UI tests on both engines, 422 Rust, `mise run check` clean.**
