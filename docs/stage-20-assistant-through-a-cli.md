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

## 16. Settings, reorganised — 2026-10-02

> *"Integrations hold Flyway and MCP while Assistant holds a setting on Dropdown
> for local modals and api keys and checks for CLI integration. that's sort of
> incoherent"*

Right, and the first round of options — regroup the tabs, put cards in a pane —
was reshuffling boxes. Sorting these things by **what they are** shows today's
tabs cut across the grain:

| | What it is |
|---|---|
| MCP server | something **we host**, so other tools reach in |
| Flyway | a **program on this machine** that we run |
| Claude Code, Copilot CLI | **programs on this machine** that we run |
| Anthropic, OpenAI, Ollama | **services we call** over HTTP |

So Flyway and the CLI agents are the same kind of thing — each answers *is it
installed, where is it, is it ready* — and sat in different tabs under different
headings. Meanwhile the Assistant's provider list spans two of those categories
at once, which is why it read wrong wherever it was put.

### 16.1 Named after the task

"Integrations" was **our** category, not anything a person sets out to do;
nobody opens Settings meaning to configure an integration. The word is gone:

```
Appearance · Editor · Assistant · Migrations · MCP server · Updates · About
```

Flyway is under **Migrations**, which is where you look when migrations cannot
find it. The server has its own section because it is a server and is like
nothing else here. The agents are under **Assistant**, the only place they are
used, under a heading that says what they are rather than what we filed them as.

The tab switcher needed no change at all: it reads `[role="tab"]` and
`aria-controls`, so this was markup.

### 16.2 One row for a program on this machine

A dot, a name, what it is for, its control — the same row for the agents and for
Flyway, because the question is the same. The state is in the **text** as well
as the dot: a colour alone asks the person to remember which was which, and
fails outright for anyone who cannot tell two of them apart.

That row needed Flyway to say something true, and nothing could.
`flywaycli::resolve` cannot answer it — off Windows it deliberately leaves the
search to the operating system, so `found` is `None` either way — so
`flywaycli::version` runs `flyway -v`. It costs a JVM start, so **only the pane
that wonders pays for it**: opening Settings on Appearance runs nothing, and the
answer is re-asked when the command is edited.

### 16.3 A bug the new tests found

`flywayAsked` started as `""` as its "not asked yet" sentinel — and `""` is a
**real setting**, meaning "whatever is on the PATH". So the row never drew for
anyone who had not typed a custom path, which is nearly everyone. It is `null`
now. The test that opens the pane on a default install is what caught it; by
hand it would have looked like a feature that simply did not exist.

### 16.4 Proof

Six UI tests on the organisation itself rather than on what it replaced: the tab
list, Flyway being under Migrations with the server not beside it, the row
reporting a version, the row reporting a failure, the probe not running until
its pane is opened, and the gutter. Two falsified — probing on every open fails
the laziness test, and 14px back to 2px fails the gutter test.

The gutter was confirmed from the mockup before it was built: `padding-right`
2px → 14px, plus `scrollbar-gutter: stable` so the text does not shift sideways
when a pane is short enough not to scroll.

**846 UI tests on both engines, 422 Rust, `mise run check` clean.**

## 17. No console window, and a loader on the button — 2026-10-02

> *"When we click on Repair on migration (And probably on apply) it does not
> show a loader. Just the cmd opening externally on windows … any call to
> external tools like flyway opens a external cmd"*

Two reports, and the second explains the first. Apply and Repair **did** disable
themselves and write "Flyway is applying…" beside the list — but on Windows,
spawning a console program from a GUI app opens a console window in front of
it, so neither was visible. The click looked inert because the feedback was
behind `cmd.exe`.

### 17.1 One helper, six spawn sites

`proc.rs` builds every `Command` with `CREATE_NO_WINDOW` on Windows, and is the
only place that flag appears. It is a module rather than a flag added where the
problem was noticed because **Flyway was not the only one**: the app spawns
external programs in six places, and three of them are the agent CLIs, which
would have flashed a console window **on every question asked of the
assistant**. The next spawn site should be born with it.

Nothing is lost by hiding the window. Every call site already captures stdout
and stderr — `output()`, or a piped reader — so the window never held anything
the app did not have. It only showed it sooner, and over the top.

### 17.2 Verified without a Windows machine

The branch that matters cannot run here, so it was checked two other ways
rather than assumed:

* `cargo check --target x86_64-pc-windows-msvc` fails in this environment — a
  dependency's build script wants MSVC's `lib.exe` — so the module's Windows
  path was compiled on its own with `rustc --target x86_64-pc-windows-msvc`,
  which passes. That proves `std::os::windows::process::CommandExt` and
  `creation_flags` are what the code thinks they are.
* `creation_flags` on the **async** command was read from the vendored source:
  `~/.cargo/registry/.../tokio-1.53.1/src/process/mod.rs:675`, inside
  `cfg_windows!`, delegating to std's. Read 2026-10-02. That it is behind
  `cfg_windows!` is also why the call needs the `#[cfg(windows)]` block: off
  Windows the method does not exist.

`mut` is used only by that block, so every other platform warned about an
unused `mut`. Allowed narrowly with `#[cfg_attr(not(windows), allow(unused_mut))]`
rather than dropped, which would have broken the Windows build.

### 17.3 The wait is on the button

`buttonBusy` puts a spinner in a button, returns the undo, and the callers run
it in a `finally` so a thrown error cannot leave a control spinning for good. It
also sets `aria-busy`, because a spinner nobody can see is not feedback.

Three UI tests, two falsified: reverting to "disable, no spinner" fails the
spinner test, and dropping the `finally` fails *"a Flyway failure gives the
button back"*.

The first attempt at that first falsification **passed**, which was not
evidence of a weak test: the patch searched for a real `…` while the source
holds the six-character `…` escape, so nothing was edited and the feature
was still intact. Re-run against the actual text, it fails as it should. A
falsification that passes is a claim about the patch until the patch is proven
to have landed.

### 17.4 Also fixed

`flywaycli`'s not-found error still said *"set the path to it in Settings →
Integrations"* — a tab renamed in §16. It says **Migrations** now.

### 17.5 Still open: showing Flyway's output

The second half of the request — *"maybe we can still show the console output on
a debug window"* — is **not built**. Today the logbook records only
`"<op> exited <code>"`, and the output survives solely inside a failure's
message, so a successful run's console text is discarded. Doing it properly
means keeping the last run's output and giving the migrations view a way to open
it, which is a feature rather than a fix, and is written down here rather than
half-done.

**852 UI tests on both engines, 422 Rust, `mise run check` exits 0.**

## 18. Three more, from using it — 2026-10-02

### 18.1 The check before the confirmation is a Flyway run too

> *"when we click on apply. First time it will check on Flyway if theres
> something to apply but it does not show any loader until we confirm"*

§17 put the spinner on the run and missed the **pre-check**: Apply and Repair
each ask Flyway what is pending first, so the dialog can name it. That is a
second JVM start, seconds long, and it reported nothing — so the click sat
there, and the loader only appeared after the confirmation.

`whileBusy(btn, label, work)` wraps a call with the spinner and puts the undo in
a `finally` once rather than in each caller. Both pre-checks say "Checking…",
and the button is given back for the dialog, because nothing is running while
somebody reads it.

### 18.2 A migrations tab showed the last query's rows

> *"when we open the migration tab it still shows the last result set which
> creates some confusion"*

The grid is shared with the editor, and `onActivate`'s migrations branch
**returned before anything repainted it** — so another tab's result set sat
there looking like Flyway's output. It calls `showResults(tab)` now, which
already knew what a migrations tab should say.

That exposed the other half: a migrations tab has no `result`, because its
outcomes are sentences rather than rows, so painting it would have *erased* the
answer to the repair you just ran. Outcomes are recorded on the tab
(`MigrationsRef.lastOutcome`) by one function every caller goes through, and
come back when you do.

### 18.3 The version ran over the description

> *"the migration name is sort of overlapping the migration version"*

Measured rather than eyeballed, and the first measurement was **wrong**: the
gaps between the three cells were a clean 8px at every window width, so the
first probe said there was nothing to fix. The gap is between *boxes*. The
version cell in the migrations view was a fixed `56px`, and `20260214093000`
needs **93px** in that font — it overflowed its cell by 37 and ran straight over
the name, at every width, for every timestamp-style version.

`minmax(56px, max-content)`: short versions still line up at 56px, long ones
take the room they need, and the description — which already ends in an
ellipsis — is the one that yields. A version is the migration's identity, so it
is not the thing to truncate.

The lesson worth keeping: **a layout measurement has to measure the thing being
complained about.** Box geometry looked perfect while text was spilling out of
it; `scrollWidth > clientWidth` was what found it.

### 18.4 Proof

Five UI tests, all three falsified: a fixed `56px` fails the overflow test,
dropping the pre-check wrapper fails *"Apply shows the wait while it works out
what is pending"*, and restoring the early `return` fails *"opening migrations
clears the editor's result set"*.

**862 UI tests on both engines, 422 Rust, `mise run check` exits 0.**

## 19. L3 + L2 — Flyway's output where the grid was, 2026-10-02

Chosen from the canvas. Two changes that answer one complaint each, and a
discovery that changed what L3 could honestly be.

### 19.1 The pane holds Flyway's output, not the SQL grid (L3)

A migrations tab has no result set — its answers are a sentence and a report —
so the pane that holds the grid now holds Flyway's output, and the grid and the
result-tab strip are hidden while a migrations tab is in front. A **swap**, not
a second region: a tab with two scrolling panes, one of them always empty, is
worse than either.

Outcomes go there too. They used to go through `results.setMessage`, which
painted a surface the tab no longer shows.

### 19.2 What the output actually is

The mockup drew Flyway's human log. **It does not arrive.** Every operation is
run with `-outputType=json`, so `stdout` is one JSON document, and printing it
raw would put a wall of braces in a pane someone opened to read a report.

So the pane **renders** it: a header line (`Flyway 13.5.0 · migrate · database
flyway_dev`), one line per migration with its state and execution time, the
error message when Flyway refused, and `stderr` verbatim underneath — which is
where a JVM warning or a driver complaint turns up, never inside the report.
Every field used is one in the captured fixtures in `flywaycli.rs`; none was
invented. Output that will not parse is shown exactly as it came, because a
Flyway that printed something unexpected is precisely when you want it unedited.

This is also the "debug window" §17.5 recorded as not built. The output was
being parsed and discarded: a successful run left a sentence, and the only way
to see what Flyway said was to make it fail. `flywaycli::LAST_RUN` keeps the
newest one; the report is copied **onto the tab** the moment a run ends, because
the backend keeps only one and `showSelected` immediately asks for `info`, which
would otherwise overwrite a migrate's report with that.

### 19.3 The strip says what is known (L2)

The operation, the environment, the migrations by version, and seconds elapsed —
plus one line admitting *"Flyway reports when it finishes, not as it goes"*.
No progress bar, because a bar claims to know which migration is in flight and
we do not.

**No Stop button**, though the mockup had one: killing a JVM part-way through a
migration is how a schema history ends up locked, and offering it would make
that the person's problem. Dropping it is the decision; drawing it was a mistake
in the mockup.

L1 — per-migration progress in the list — stays unbuilt and depends on the
measurement in §19.5.

### 19.4 Proof

Six new UI tests, three falsified: printing the JSON raw fails *"renders
Flyway's report rather than its JSON"*, leaving the grid visible fails *"opening
migrations clears the editor's result set"*, and dropping the "not as it goes"
sentence fails *"admits what it cannot know"*. Twenty-one existing assertions
moved from `#grid .empty` to `#flyway-body`: the design moved, so the
assertions followed it rather than the reverse.

**874 UI tests on both engines, 422 Rust, `mise run check` exits 0.**

### 19.5 Still unmeasured

Whether Flyway's human log is observable while the JSON is being collected.
Flyway is not installed on the development machine, so one command on a machine
that has it decides whether L1 is buildable:

```bash
flyway -outputType=json info > out.json 2> err.txt
```

Progress lines in `err.txt` mean a live log, and L1, are possible; nothing until
the end means L2 is already the truthful version of it.

## 20. §19.5 answered — L1 is not buildable, 2026-10-02

> *"Why we dont create Pod on podman so we can execute that … That way we don't
> need to install on the machine"*

Right instinct, and it turned out nothing needed installing **or** containering:
`mise run flyway-up` already unpacks a pinned Flyway 13.5.0 into `dev/.flyway`,
which is why `which flyway` found nothing. The measurement ran against the real
`dev/flyway` fixture with MySQL in podman, as the project already does.

### 20.1 The answer

```
flyway -configFiles=flyway.toml -environment=development -outputType=json migrate
  → stdout 18,831 bytes, stderr 0 bytes
```

**stderr is empty, on every run.** With `-outputType=json` Flyway writes one
document to stdout when it finishes and nothing anywhere as it goes. So there
is no live progress to stream, and **L1 — per-migration progress in the list —
cannot be built honestly**. L2's "Flyway reports when it finishes, not as it
goes" is not a hedge; it is the measured truth.

Streaming would mean giving up `-outputType=json`, and with it every structured
result this app depends on. Not a trade worth making for an animation.

### 20.2 What the measurement changed

**A migrate report has no `state` field.** Measured: its `migrations[]` entries
carry `category`, `description`, `executionTime`, `filepath`, `type`, `version`
and nothing else — `state` belongs to an `info` report. The renderer said
`m.state ?? ""` and so printed an empty column for every row. A listed
migration in a migrate is one that *ran*, and the row now says `Applied`.

**A failed migrate still lists what succeeded.** V1, V2 and V3 applied, then V4
failed, and the document carries all four facts. That ordering is the whole
value of the pane after a failure, and it is now what the pane shows.

**The error object is 18 KB of Java.** The useful message was 527 bytes; the
rest was a `cause` chain three deep, each with its own `stackTrace`. Rendering
`error.message` alone was already the design — now it is a verified necessity
rather than a guess.

Also added from the real reports: `migrationsExecuted`, `targetSchemaVersion`
and `totalMigrationTime` as a summary line, and `warnings[]`.

### 20.3 A test that proved nothing

The fixture for all this started out **invented**: each migration had
`state: "Success"`, which a migrate report never contains. It passed, and
validated fiction.

Replacing it with the real capture exposed a second, worse problem. The real
document was trimmed of its stack traces for file size — which removed the very
thing the test's *"and none of the Java"* assertions guard. Falsifying the
renderer by printing the whole error object **still passed**, because there was
no Java left in the fixture to leak.

The fixture now carries a real `cause` and a real `stackTrace`, cut to six
frames, and the falsification fails as it should. **A guard assertion is only
worth what the fixture can violate.**

### 20.4 On running Flyway in a container

Worth separating two uses. For *measuring Flyway's behaviour*, a container
would have worked and so did the already-unpacked binary. For *testing this
app*, it cannot: the app's whole design is that it spawns **your own Flyway**,
so the fixture has to be a binary this process can `Command::new` — one inside
a container is reachable by `podman exec`, which is not the code path under
test.

Where it would earn its place is CI, so a run does not fetch 584 MB. That is
not built, and is noted here rather than assumed.

**876 UI tests on both engines, 422 Rust, `mise run check` exits 0.**

## 21. Testing L2 and L3 against the fixture — 2026-10-02

> *"Ok and L2 + L3 can me test those with the fixtures?"*

Partly, and the split is worth naming, because the honest answer is not "yes".

**L2 cannot be.** The strip is UI timing — a spinner, an elapsed counter, a
sentence. The fixture adds nothing a stub does not already give, and the claims
are already pinned by the Playwright tests.

**L3's rendering cannot be either.** The pane turns a document into lines, which
is a function of its input. What the fixture *can* do — and now does — is pin
the **contract**: that what the backend hands the pane is what the pane expects.
A UI test can only check that against output pasted into it. These check it
against Flyway.

Five tests in `live_flyway.rs` (`mise run test-flyway`, 15 total now):

* **stderr stays empty**, which is the measurement L1 was cancelled over,
  pinned so that the day Flyway narrates to stderr a test says so.
* **A migrate answers in one of three shapes**, which the live run discovered by
  failing: a report that worked, a report that broke part-way, and a **refusal**
  carrying `{"error": …}` and nothing else — no `operation`, no `migrations`.
  The third now has a UI test of its own, because it is the common one: a failed
  or edited migration refuses before it starts.
* **Migrate rows carry no `state`, info rows do** — the asymmetry that made the
  renderer print an empty column.
* **A failure lists what ran before naming what broke**, and the document is
  still mostly stack trace.

### 21.1 Two tests that proved nothing, for the same reason

Inverting *"a migrate row has no state"* **passed**: an earlier test had already
migrated `qa`, so `migrations[]` came back empty and the loop body never ran.
Inverting the renderer's error handling **passed** too, because the UI fixture
had been trimmed of the stack traces its assertions forbid.

Same lesson twice in one sitting: **an assertion is worth only what its input
can violate.** A loop over nothing passes every claim about its contents.

Both are fixed by the test owning its state — `clean` then migrate, so the rows
are guaranteed — and the UI fixture now carries a real `cause` and a real
`stackTrace` cut to six frames.

### 21.2 The fixture gained a third database

`flyway_probe`, because a test that must clean cannot share a database with
tests that expect what `flyway-up` left. Cleaning `development` broke whichever
test ran next; the suite now passes **twice in a row with no reset**, which is
the property that was missing.

### 21.3 On the container question

For *measuring Flyway*, a container would have worked. For *testing this app*,
it cannot: the design is that the app spawns **your own Flyway**, so the fixture
must be a binary this process can `Command::new`. One inside a container is
reachable by `podman exec`, which is not the code path under test. Where it
would earn its place is CI, so a run does not fetch 584 MB — still not built.

**878 UI tests on both engines, 422 Rust, 15 live Flyway tests, `mise run
check` exits 0.**

## 22. The capture path, tested against real Flyway — 2026-10-03

> *"if I install flyway locally we can create a fixture so we can debug the
> behavior end to end right?"*

**Nothing needs installing**, which is worth saying before anything else:
`mise run flyway-up` already fetches the pinned 13.5.0 into `dev/.flyway`. That
is why `which flyway` finds nothing on a machine that can run the whole Flyway
suite. Hand-debugging works today, and `dev/README.md` now has the recipe —
including the one detail that trips it up, that the app's **Flyway command**
setting must be given the absolute path of the pinned binary, because empty
means "whatever is on the PATH" and the pinned copy deliberately is not.

The question did find something, though.

### 22.1 A gap in what §21 claimed

§21 said the live tests pin "the contract between the backend and the pane".
They did not. They asserted the `Finished` that `run` returns, and the UI tests
asserted a document pasted into them — while the thing **in between**, the store
that `flyway_last_run` reads, had no live coverage at all. That store is what
actually feeds the pane.

Two tests now cover it:

* **the stored run is the run that just happened** — same stdout, same exit
  code, same operation, and still parseable. Falsified by removing the
  `remember` call.
* **a later run replaces it** — a migrate, then an `info`, and the store holds
  the `info`. This is why the UI copies the report onto the tab *before*
  `showSelected` re-reads the list, which until now was asserted only by a
  comment. Falsified by making `info` not overwrite.

17 live Flyway tests.

### 22.2 What still cannot be automated

The UI and the real backend together. The Playwright harness stubs `invoke`, so
a test either drives the real UI against stubs or the real backend without a
UI. Closing that would mean driving a built Tauri binary — a different kind of
test, not a fixture — and it is not built.

So "end to end" splits in three, and all three now have something: the UI
against stubs (878 tests), the backend against real Flyway (17), and the whole
app by hand (the recipe in `dev/README.md`).
