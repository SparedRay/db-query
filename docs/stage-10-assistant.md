# Stage 10 — An assistant that writes SQL

**Goal:** ask a question in plain language and get SQL you can read, in the
editor, ready to run yourself.
**Builds on:** [Stage 9 — Query history](stage-9-query-history.md).
**Status:** 🚧 Experimental — built 2026-09-08, multi-provider the same day;
never yet run against a real endpoint from this machine.

---

## 1. The constraint that shaped everything

> *"the idea is that the chat can put the queries but not execute them"*

Since Stage 0 this project has never run a statement the user did not ask it to
run, and an assistant is exactly the feature that erodes that quietly. The
answer settles the design rather than compromising it:

**The model is given no tools at all.** It cannot query, cannot connect, cannot
act. It answers in prose and writes SQL into the editor, where the same Run
button and the same review step apply as to anything typed by hand. Every
statement it produces is `run_script`-ed by a human or not at all, and the UI
tests assert `run_script` was never called.

That also removes the largest piece of machinery this feature might have needed:
without execution there is **no tool-use loop**, so a turn is one streamed
completion.

### Explicitly out of scope

- Executing anything. See above.
- Sending row data. The schema's *shape* goes; its contents never do.
- ~~Choosing a provider.~~ **Reversed the same day** — see §8. The reasoning
  ("an abstraction with one implementation is a guess about a second one") was
  right to wait and right to be abandoned the moment a real second and third
  arrived.
- Conversation persistence. Chats are per-session; history already records the
  statements that mattered.

---

## 2. Feasibility, measured

The question was "how feasible", so these were checked rather than assumed.

| Question | Answer |
|---|---|
| Does it need a new HTTP client? | **No.** `reqwest 0.13.4` is already linked, via `tauri-plugin-updater`. |
| What does adding it cost? | **375 crates before, 375 after.** Zero new dependencies. |
| Licence? | `MIT OR Apache-2.0`, read from the vendored `Cargo.toml`. |
| TLS? | Already rustls, with the `ring` provider the updater plugin installs. |

The one trap: reqwest's `rustls` feature pulls **`aws-lc-rs`**, which is both a
new crate and a build that needs cmake and NASM — landing on Windows CI.
`rustls-no-provider` is what the updater already uses and what this uses, which
is why the crate count is unchanged.

---

## 3. Decisions

- **`claude-opus-5` is the default**, not the only option — see §8. Writing
  correct SQL against an unfamiliar schema is the kind of task where model
  quality shows, so that is where the default sits.
- **Streaming, with `thinking.display: "summarized"` set explicitly.** On this
  model thinking is on by default but its display defaults to omitted, which in
  a chat window is indistinguishable from the app having hung. The summary is
  rendered dimmed, above the answer.
- **`budget_tokens` is not sent.** It is rejected outright on this model; the
  request test asserts its absence so it cannot creep back in.
- **The key lives in the OS keychain**, under a namespaced account
  (`assistant:anthropic`) that cannot collide with a connection UUID. Same store
  as database passwords, and the settings field is cleared the moment it is
  handed over — it exists to pass the key on, not to hold it.
- **Schema goes, data never does.** `render_schema` emits table names, column
  names, types, nullability and keys. It has no access to rows by construction,
  and a test asserts no email address from the fixture reaches the prompt.
- **A table whose columns are not cached says so.** Columns load lazily when you
  expand the tree; rendering such a table as having *no* columns would invite
  invented column names.
- **SQL is parsed out of ```sql fences**, not returned as a structured field.
  That keeps the prose the model wrote *around* the SQL — usually the part that
  explains the join — instead of discarding it to get at a schema.
- **Errors after the stream starts are events, not failures.** By then there may
  be half an answer on screen, and replacing it with an error loses the more
  useful half.
- **An unanswered question is removed from the transcript.** If nothing came
  back, leaving the question in place would build the next turn on a lie.
- **Provenance is exact-match.** Accepting a block records the SQL; when a
  statement with that exact text is later run, history marks it *from the
  assistant*. Edit it first and it is yours — the honest answer, and one that
  needs no fuzzy matching. Recording happens **on accept, not on generation**: a
  suggestion you never used is not part of your history.

---

## 4. Milestones

- [x] **A1 — A question streams back an answer**, with SQL in its own block.
- [x] **A2 — Nothing executes.** Asserted in every picking test.
- [x] **A3 — SQL reaches the editor only on an explicit click**, to the cursor or a new tab.
- [x] **A4 — History says what came from the assistant.**
- [x] **A5 — The key is in the keychain**, never in a config file, never read back out.
- [x] **A6 — A mid-stream failure keeps the partial answer.**
- [x] **A7 — With no key the chat says so and cannot be used.**
- [x] **A8 — Confirmed against a real endpoint** (local half, 2026-09-09; Anthropic half still needs a key). Three ignored tests in `assistant_live.rs`: Anthropic with a key, a rejected key, and a local Ollama needing no key. The Ollama one now passes against `mise run ollama-up` — see §*Run, 2026-09-09*, including the two bugs it found.

---

## 5. Task tracker

### Phase 1 — Core — built 2026-09-08
- [x] `src-tauri/src/assistant.rs`: prompt, schema rendering, request shape, incremental SSE decoder, error messages, proposal tracking. **16 unit tests**
- [x] Commands: `assistant_status`, `assistant_set_key`, `assistant_send` (streaming over `tauri::ipc::Channel`), `remember_proposal`
- [x] `history::Entry` gained `source`, defaulted to `"user"` so older history still reads — and so the *absence* of provenance never reads as the assistant's

### Phase 2 — The chat — built 2026-09-08
- [x] `src/assistant.ts`, `#assistant-dialog`, a rail button and **Ctrl+K**
- [x] Fence splitting that tolerates an unterminated block, which is what a cut-off reply looks like
- [x] Settings: key field, save/forget, and a plain statement of what is sent
- [x] **18 UI tests** on both engines, five of them covering provider choice

### Phase 3 — Verification
- [x] `src-tauri/tests/assistant_live.rs` — two ignored tests: the API accepts our body and streams fenced SQL back; a bad key produces the message settings promises
- [ ] Run them with a real key (A8)

---

## 6. What this cost the test harness

Streaming was **untestable** as the harness stood, and fixing that was the
largest single piece of work outside the feature.

- `transformCallback` returned the callback itself. That was fine for events and
  impossible for channels: `Channel` is built entirely on the *id* that function
  returns, and a function cannot travel through invoke args — Playwright cannot
  serialise one. It now registers callbacks and returns an integer, as the real
  runtime does.
- `Channel` **buffers out-of-order messages and silently holds** anything
  arriving before the index it expects. A test sending a second batch from index
  zero would simply see nothing, with no error. The harness now owns the counter
  per channel, which is where the runtime keeps it too.
- The channel arrives in two shapes: the live runtime sends `__CHANNEL__:<id>`
  via `toJSON`, while reading `__CALLS__` back out of the page structured-clones
  the object and keeps its public `id`. `sendOnChannel` accepts either.

### Two bugs the tests caught

1. **The request was passing `history` by reference**, so the recorded call
   mutated after the fact — and the array really was mutated mid-turn, when an
   unanswered question is popped. The request now sends a snapshot, which is
   what a request should be.
2. **The stub resolved `assistant_send` immediately**, but the real command
   returns only once the stream has ended. The UI decides what to do with a
   failure *after* the call resolves, so the original test was checking a
   sequence that cannot occur. The stub is now gated on the events being
   delivered.

---

## 7. Risks

- **Untried against the real API.** Every part of the wire format is asserted
  offline, but a wrong field name returns a 400, not a compile error. A8 is the
  only thing that closes this.
- **Cost is real and per-question**, and nothing in the UI shows it yet.
- **Prompt injection through schema.** Table and column names come from the
  server, and a hostile name is text the model reads. It cannot act on anything
  — no tools — so the worst case is a misleading answer rather than an action.
- **The model can be confidently wrong about SQL.** That is precisely why it
  does not run it, and why the statement lands somewhere you read it first.

---

## 8. Any provider, including local — 2026-09-08

> *"Could it accept local implementations? like Ollama? OpenAI? Or any other?
> probably worth to make it configurable on settings isnt?"*

Yes, and it cost less than the first provider did.

### Two adapters, not five

**Ollama, LM Studio, llama.cpp's server, vLLM, OpenRouter, Groq and Azure all
expose the OpenAI Chat Completions shape.** So the axis is not "which vendor"
but "which of two wire formats, and at which URL":

| | Anthropic | OpenAI-compatible |
|---|---|---|
| Endpoint | `{base}/v1/messages` | `{base}/chat/completions` |
| Auth | `x-api-key` + `anthropic-version` | `Authorization: Bearer` |
| System prompt | a top-level field | the first message |
| Text delta | `content_block_delta` → `delta.text` | `choices[0].delta.content` |
| Thinking | `thinking_delta` | `delta.reasoning_content` (a convention) |
| End of turn | `message_delta.stop_reason` | `choices[0].finish_reason` |
| Terminator | — | `data: [DONE]` |

**The SSE framing is identical**, so only payload decoding forks — which is the
single fact that made a second provider cheap. A third adapter would need a
provider speaking neither shape.

Settings offers Anthropic, OpenAI, Ollama, LM Studio and "Other
OpenAI-compatible"; the last is not an escape hatch but the general case, since
a base URL is all that separates Ollama from Groq.

### Decisions

- **One keychain account per provider** (`assistant:anthropic`,
  `assistant:openai`). Switching to a local model and back must not lose a cloud
  key, and a keyless local setup must not shadow a stored one.
- **A local server is not asked for a key.** `requires_key` is false for a local
  base URL, and the request omits the auth header entirely when there is no key
  — a local model given `Authorization: Bearer` is at best ignored.
- **"Nothing leaves this machine" is said out loud** when the base URL is local.
  That is the genuinely better property of running locally and the main reason
  to support it; the schema-is-shared warning is replaced rather than repeated.
- **Nothing Anthropic-specific is sent to an OpenAI-compatible server.** An
  unknown parameter is a 400 on some servers and silently ignored on others, and
  neither is worth risking for a field a local model would not honour.
- **The model field is cleared, not guessed, when a preset cannot know it.**
  Which models a machine has pulled is a property of that machine; a guess
  produces a confident 404. The chat stays disabled with "No model set" until
  one is chosen, and the field carries a per-preset placeholder.
- **Provider, base URL and model live in `localStorage`**, with the settings.
  None of it is secret — the key is the secret, and it is elsewhere.

### A bug the tests caught

`is_local` split the host on `:` to drop the port, which turns
`http://[::1]:8080` into `[`. A perfectly ordinary local address would have been
treated as remote: we would have demanded an API key it does not need, and
warned that the schema was leaving a machine it never left. Bracketed IPv6 is
now stripped before the port. There is also a test pinning the *other*
direction — `https://localhost.example.com` and `http://evil.com/?x=localhost`
must never be read as local, because that mistake suppresses a true warning.

### Still unverified

`assistant_live.rs` now has a third ignored test that runs the whole flow
against a local Ollama:

```
mise run ollama-up     # podman container + qwen2.5-coder:1.5b, ~2 GB
mise run test-ollama
```

It needs nothing but a running server — no key, no account, no spend — which
makes it the cheapest way to prove the second adapter is real rather than
plausible. A8 now covers both.

### Run, 2026-09-09 — and two bugs it found

The local half of A8 passed: `qwen2.5-coder:1.5b`, on CPU, streamed
```` ```sql\nSELECT COUNT(*) FROM users;\n``` ```` back through our SSE decoder
in 3.1 s. The second adapter is real.

Getting there cost two fixes, both of which had been sitting in a green repo:

**The command documented above ran nothing.** `-- --ignored ollama` matched no
test name — the test is `a_local_openai_compatible_server_streams_back` — and
`cargo test` exits **0** when a filter matches nothing. So the documented
command reported success while contacting no server at all. `mise run
test-ollama` now filters `--exact` and fails if the pass count is not 1, because
a test runner's silence is not the same as a test passing.

**Every live test panicked before sending a byte.** `reqwest` is built with
`rustls-no-provider`, so `Client::new()` panics unless a provider is installed.
`run()` installs one — but integration tests never call `run()`. This is the
same latent bug found during Stage 11, one layer further out: the fix had been
applied to the app and not to anything that bypasses it. `install_tls` is now
`pub`, `ElasticEngine::new` calls it itself so no caller has to remember, and
the six scattered copies of the incantation are one function.
