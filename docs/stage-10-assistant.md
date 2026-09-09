# Stage 10 — An assistant that writes SQL

**Goal:** ask a question in plain language and get SQL you can read, in the
editor, ready to run yourself.
**Builds on:** [Stage 9 — Query history](stage-9-query-history.md).
**Status:** 🚧 Experimental — built 2026-09-08; never yet run against the real
API from this machine (no key here).

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
- Choosing a provider. This talks to one API; a provider abstraction with one
  implementation is a guess about a second one.
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

- **`claude-opus-5`.** Writing correct SQL against an unfamiliar schema is the
  kind of task where model quality shows.
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
- [ ] **A8 — Confirmed against the real API.** `assistant_live.rs` exists and is ignored by default; there is no key on this machine to run it with.

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
- [x] **13 UI tests** on both engines

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
