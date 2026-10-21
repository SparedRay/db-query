import { expect, test, type Page } from "@playwright/test";
import { calls, connect, editorText, installBackend, openDatabase, relaunch, rowsResult, schemaBackend, sendOnChannel, settingsSection, visit } from "./harness";

/**
 * The assistant chat.
 *
 * **The rule under test is that it cannot run anything.** It has no tools and
 * no connection; every statement it writes goes into the editor behind an
 * explicit click, and is run the way anything typed by hand is run. Half of
 * what is asserted here is that `run_script` was never called.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

const READY = { hasKey: true, ready: true, local: false };

/**
 * `assistant_send` must not resolve until the stream is over.
 *
 * That is the real contract — Rust returns when the last chunk has been read —
 * and it matters: the UI decides what to do with a failure *after* the call
 * resolves. A stub that resolved immediately would let that run before any
 * event arrived, and the test would be checking a sequence that cannot happen.
 */
function gate() {
  let release!: () => void;
  const promise = new Promise<void>((r) => {
    release = r;
  });
  return { promise, release };
}

let stream = gate();

/** Push events down the channel, then let the call resolve. */
async function emit(page: Page, events: unknown[]) {
  await sendOnChannel(page, "assistant_send", "onEvent", events);
  stream.release();
}

/** The common case: one whole answer. */
async function reply(page: Page, text: string) {
  await emit(page, [
    { type: "text", delta: text },
    { type: "done", stopReason: "end_turn" },
  ]);
}

async function openChat(page: Page, extra: Record<string, unknown> = {}) {
  stream = gate();
  await connect(page, {
    assistant_status: () => READY,
    assistant_send: () => stream.promise,
    ...extra,
  });
  await page.click("#btn-assistant");
  await expect(page.locator("#chat-pane")).toBeVisible();
}

async function ask(page: Page, question: string) {
  await page.fill("#chat-input", question);
  await page.click("#chat-send");
}

async function assertNothingRan(page: Page) {
  expect((await calls(page)).map((c) => c.cmd)).not.toContain("run_script");
}

// --------------------------------------------------------------------- basics

test("a question is sent with the schema context, and streams back", async ({ page }) => {
  await openChat(page);
  await ask(page, "how many users?");

  await expect(page.locator(".chat-msg.from-user")).toContainText("how many users?");
  await reply(page, "Count them like this:\n\n```sql\nSELECT COUNT(*) FROM users;\n```");

  await expect(page.locator(".chat-sql pre")).toContainText("SELECT COUNT(*) FROM users;");
  await expect(page.locator(".chat-prose")).toContainText("Count them like this:");
  await assertNothingRan(page);
});

test("the request carries the active connection and database", async ({ page }) => {
  await openChat(page);
  await ask(page, "hi");

  const sent = (await calls(page)).find((c) => c.cmd === "assistant_send");
  expect(sent?.args.messages).toEqual([{ role: "user", content: "hi" }]);
  expect(sent?.args.connectionId).toBeTruthy();
});

test("streamed text appears as it arrives, not only at the end", async ({ page }) => {
  await openChat(page);
  await ask(page, "explain");

  await sendOnChannel(page, "assistant_send", "onEvent", [{ type: "text", delta: "Partly " }]);
  await expect(page.locator(".chat-msg.from-assistant")).toContainText("Partly");

  await emit(page, [{ type: "text", delta: "written." }]);
  await expect(page.locator(".chat-msg.from-assistant")).toContainText("Partly written.");
});

/** A thinking model is silent for a while; that has to look like progress. */
test("thinking is shown separately from the answer", async ({ page }) => {
  await openChat(page);
  await ask(page, "something hard");

  await emit(page, [
    { type: "thinking", delta: "considering the join order" },
    { type: "text", delta: "Here you go." },
  ]);

  await expect(page.locator(".chat-thinking")).toContainText("considering the join order");
  await expect(page.locator(".chat-prose")).toContainText("Here you go.");
});

// ------------------------------------------------------------- it never runs

test("SQL only reaches the editor when you click, and still runs nothing", async ({ page }) => {
  await openChat(page);
  await ask(page, "give me a query");
  await reply(page, "```sql\nSELECT 42;\n```");

  // Nothing is in the editor until the click.
  expect(await editorText(page)).not.toContain("SELECT 42");

  await page.locator('.chat-sql-actions button:has-text("Insert at cursor")').click();
  expect(await editorText(page)).toContain("SELECT 42");
  await assertNothingRan(page);
});

test("New tab opens the SQL in its own tab, and runs nothing", async ({ page }) => {
  await openChat(page);
  const before = await page.locator("#script-tabs .stab").count();
  await ask(page, "give me a query");
  await reply(page, "```sql\nSELECT 7;\n```");

  await page.locator('.chat-sql-actions button:has-text("New tab")').click();
  await expect(page.locator("#script-tabs .stab")).toHaveCount(before + 1);
  expect(await editorText(page)).toContain("SELECT 7");
  await assertNothingRan(page);
});

/** So history can say where a statement came from, as asked for. */
test("accepting a suggestion records it as the assistant's", async ({ page }) => {
  await openChat(page);
  await ask(page, "q");
  await reply(page, "```sql\nSELECT 1;\n```");

  expect((await calls(page)).map((c) => c.cmd)).not.toContain("remember_proposal");
  await page.locator('.chat-sql-actions button:has-text("Insert at cursor")').click();

  await expect
    .poll(async () => (await calls(page)).find((c) => c.cmd === "remember_proposal")?.args.sql)
    .toContain("SELECT 1;");
});

// ------------------------------------------------------------------ failures

/** A partial answer is worth more than a tidy error, so it is kept. */
test("a mid-stream failure keeps what was already written", async ({ page }) => {
  await openChat(page);
  await ask(page, "q");

  await emit(page, [
    { type: "text", delta: "Half an answer" },
    { type: "failed", message: "Overloaded" },
    { type: "done", stopReason: null },
  ]);

  await expect(page.locator(".chat-msg.from-assistant")).toContainText("Half an answer");
  await expect(page.locator(".chat-error")).toContainText("Overloaded");
});

test("a request that never starts reports why", async ({ page }) => {
  await openChat(page, {
    assistant_send: () => {
      throw new Error("The API key was rejected. Check it in Settings.");
    },
  });
  await ask(page, "q");
  await expect(page.locator(".chat-error")).toContainText("Check it in Settings");
});

// ----------------------------------------------------------------- unset key

test("with no key the chat says so and cannot be used", async ({ page }) => {
  await installBackend(page, {
    ...schemaBackend,
    assistant_status: () => ({ hasKey: false, ready: false, local: false }),
  });
  await visit(page);
  await page.click("#btn-assistant");

  await expect(page.locator("#chat-note")).toContainText("No API key set");
  await expect(page.locator("#chat-send")).toBeDisabled();
  await expect(page.locator("#chat-input")).toBeDisabled();
});

/** The promise the UI makes on the feature's behalf, so it is pinned. */
test("the chat states what is sent and what is not", async ({ page }) => {
  await openChat(page);
  await expect(page.locator("#chat-note")).toContainText("row data never is");
  // No dictionary on this connection, so the sentence does not claim one.
  await expect(page.locator("#chat-note")).not.toContainText("dictionary");
});

/**
 * Stage 25 §3.8. A dictionary is prose the user wrote and can carry an example
 * value, so "table and column names are sent, row data never is" stopped being
 * the whole truth the moment one could be attached. The header says it.
 */
test("the chat says when a dictionary is sent too", async ({ page }) => {
  await installBackend(page, {
    ...schemaBackend,
    assistant_status: () => ({ hasKey: true, ready: true, local: false }),
    connect_saved: () => ({
      id: "p1",
      serverVersion: "8.4.0",
      databases: ["poc"],
      currentDatabase: "poc",
      capabilities: { engine: "mysql", namespaceLabel: "database", writes: true },
    }),
    list_profiles: () => ({
      profiles: [
        {
          id: "p1",
          name: "prod-eu",
          colour: "#3b82f6",
          host: "db.example.com",
          port: 3306,
          user: "reporting",
          database: "poc",
          allowInvalidCerts: false,
          kind: "mysql",
          url: "",
          auth: { type: "none" },
          noPassword: false,
          rememberPassword: true,
          needsSecret: false,
          dictionary: "/work/warehouse/db/dictionary.toml",
        },
      ],
      warning: null,
    }),
  });
  await visit(page);
  await page.locator(".rail-item").click();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);

  await page.click("#btn-assistant");
  await expect(page.locator("#chat-pane")).toBeVisible();

  const note = page.locator("#chat-note");
  await expect(note).toContainText("the dictionary you attached");
  // And the rest of the promise is still there, not replaced by it.
  await expect(note).toContainText("row data never is");
});

// ------------------------------------------------------------- the key itself

test("saving a key hands it over and does not keep it in the field", async ({ page }) => {
  await connect(page, {
    assistant_status: () => ({ hasKey: false, ready: false, local: false }),
    assistant_set_key: () => true,
  });
  await page.click("#btn-settings");
  await settingsSection(page, "assistant");
  await page.fill("#set-ai-key", "sk-ant-secret");
  await page.click("#set-ai-save");

  await expect
    .poll(async () => (await calls(page)).find((c) => c.cmd === "assistant_set_key")?.args.key)
    .toBe("sk-ant-secret");
  // The field exists to hand the key over, not to hold it.
  await expect(page.locator("#set-ai-key")).toHaveValue("");
});

test("forgetting the key sends null", async ({ page }) => {
  await connect(page, { assistant_status: () => READY, assistant_set_key: () => false });
  await page.click("#btn-settings");
  await settingsSection(page, "assistant");
  await page.click("#set-ai-forget");

  await expect
    .poll(async () => (await calls(page)).find((c) => c.cmd === "assistant_set_key")?.args.key)
    .toBe(null);
});

// ------------------------------------------------------------------ providers

/**
 * The point of the abstraction: the same chat, a different server.
 *
 * Two adapters cover Anthropic and everything speaking OpenAI's shape — which
 * is Ollama, LM Studio, llama.cpp, vLLM, OpenRouter, Groq and Azure. What each
 * test here checks is that the *choice* reaches the backend, since the wire
 * formats themselves are pinned by Rust unit tests.
 */
async function choosePreset(page: Page, label: string) {
  await page.click("#btn-settings");
  await settingsSection(page, "assistant");
  await page.selectOption("#set-ai-preset", { label });
}

test("switching to a local model sends that provider and base URL", async ({ page }) => {
  await connect(page, {
    assistant_status: () => ({ hasKey: false, ready: true, local: true }),
    assistant_send: () => new Promise(() => {}),
  });

  await choosePreset(page, "Ollama (local)");
  await page.fill("#set-ai-model", "llama3.1");
  await page.locator("#set-ai-model").blur();
  await page.click("#set-close");

  await page.click("#btn-assistant");
  await page.fill("#chat-input", "hi");
  await page.click("#chat-send");

  const sent = (await calls(page)).find((c) => c.cmd === "assistant_send");
  expect(sent?.args).toMatchObject({
    provider: "openAiCompatible",
    baseUrl: "http://localhost:11434/v1",
    model: "llama3.1",
  });
});

/** A local model is the one configuration where nothing leaves the machine. */
test("a local model says so instead of warning about what is sent", async ({ page }) => {
  await connect(page, {
    assistant_status: () => ({ hasKey: false, ready: true, local: true }),
  });
  await choosePreset(page, "Ollama (local)");
  await page.fill("#set-ai-model", "llama3.1");
  await page.locator("#set-ai-model").blur();
  await expect(page.locator("#set-ai-note")).toContainText("nothing leaves it");
  await page.click("#set-close");

  await page.click("#btn-assistant");
  await expect(page.locator("#chat-note")).toContainText("runs on your machine");
});

/** Keys are per provider, so configuring one must not touch another's. */
test("the key is saved against the selected provider", async ({ page }) => {
  await connect(page, { assistant_status: () => READY, assistant_set_key: () => true });
  await choosePreset(page, "OpenAI");
  await page.fill("#set-ai-key", "sk-openai");
  await page.click("#set-ai-save");

  await expect
    .poll(async () => (await calls(page)).find((c) => c.cmd === "assistant_set_key")?.args)
    .toMatchObject({ provider: "openAiCompatible", key: "sk-openai" });
});

/**
 * A local server's model list is a property of that machine, so the preset
 * clears the field rather than guessing — a guess is a confident 404.
 */
test("a provider with no known model leaves the field blank, and blocks the chat", async ({
  page,
}) => {
  await connect(page, {
    assistant_status: () => ({ hasKey: false, ready: true, local: true }),
  });
  await choosePreset(page, "Ollama (local)");
  await expect(page.locator("#set-ai-model")).toHaveValue("");
  await expect(page.locator("#set-ai-model")).toHaveAttribute("placeholder", /llama3\.1/);
  await page.click("#set-close");

  await page.click("#btn-assistant");
  await expect(page.locator("#chat-note")).toContainText("No model set");
  await expect(page.locator("#chat-send")).toBeDisabled();
});

test("the choice survives a reload", async ({ page }) => {
  await connect(page, { assistant_status: () => READY });
  await choosePreset(page, "OpenAI");
  await page.fill("#set-ai-model", "some-model");
  await page.locator("#set-ai-model").blur();

  await relaunch(page);
  await page.click("#btn-settings");
  await settingsSection(page, "assistant");
  await expect(page.locator("#set-ai-base")).toHaveValue("https://api.openai.com/v1");
  await expect(page.locator("#set-ai-model")).toHaveValue("some-model");
});

/**
 * A hosted provider that speaks OpenAI's shape costs a preset, not an adapter.
 * What is worth pinning is that choosing it sends the *provider* and *base URL*
 * the compatibility layer expects — a preset with the wrong base URL is a 404
 * an hour later, in someone else's account.
 */
test("Gemini is reached through the OpenAI-compatible path", async ({ page }) => {
  await connect(page, {
    assistant_status: () => READY,
    assistant_send: () => new Promise(() => {}),
  });
  await choosePreset(page, "Google Gemini");
  await page.fill("#set-ai-model", "gemini-2.5-pro");
  await page.locator("#set-ai-model").blur();
  await page.click("#set-close");

  await page.click("#btn-assistant");
  await page.fill("#chat-input", "which tables are there?");
  await page.click("#chat-send");

  await expect
    .poll(async () => (await calls(page)).find((c) => c.cmd === "assistant_send")?.args)
    .toMatchObject({
      provider: "openAiCompatible",
      baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai/",
      model: "gemini-2.5-pro",
    });
});

// ------------------------------------------------- a dock, not a modal

/**
 * **The point of the change.** It was a `showModal()` dialog, so while the
 * assistant was open the tree, the editor and the grid were all inert — and a
 * proposed query is exactly when you want to look at the schema it came from.
 *
 * `inert` is not something to assert directly; what matters is that clicking
 * through still *does* something. So this drives the app with the panel open.
 */
test("the app stays usable while the assistant is open", async ({ page }) => {
  await openChat(page, { use_database: () => null });

  // The schema still answers a click: expanding a database is two round trips
  // and a repaint, none of which a modal would have allowed.
  await openDatabase(page);
  await expect(page.locator('.node.group:has-text("Tables")')).toBeVisible();

  // The editor still takes typing.
  await page.locator(".cm-content").click();
  await page.keyboard.type("SELECT 1");
  expect(await editorText(page)).toContain("SELECT 1");

  // And the global shortcuts still work, which they did not while a modal
  // owned the keyboard.
  const before = await page.locator("#script-tabs .stab").count();
  await page.keyboard.press("Control+t");
  await expect(page.locator("#script-tabs .stab")).toHaveCount(before + 1);

  // Through all of which the panel stayed open.
  await expect(page.locator("#chat-pane")).toBeVisible();
});

/** The rail button and Ctrl+K are the same toggle, and closed means gone. */
test("the assistant toggles, and gives its width back", async ({ page }) => {
  await openChat(page);
  const width = async () =>
    (await page.locator("#chat-pane").boundingBox())?.width ?? 0;
  expect(await width()).toBeGreaterThan(200);

  // The rail button says which way it is, the way the schema button does.
  await expect(page.locator("#btn-assistant")).toHaveAttribute("aria-pressed", "true");

  await page.click("#btn-assistant");
  await expect(page.locator("#chat-pane")).toBeHidden();
  await expect(page.locator("#btn-assistant")).toHaveAttribute("aria-pressed", "false");
  // Not merely zero-width: a collapsed pane holding a textarea would still
  // take Tab.
  await expect(page.locator("#msplit")).toBeHidden();

  await page.keyboard.press("Control+k");
  await expect(page.locator("#chat-pane")).toBeVisible();
  await page.keyboard.press("Control+k");
  await expect(page.locator("#chat-pane")).toBeHidden();
});

/** Inserting used to shut the panel. A dock is not in the way, and closing it
 *  after one insert throws away the conversation that produced the statement. */
test("using a suggestion leaves the conversation on screen", async ({ page }) => {
  await openChat(page);
  await ask(page, "give me a query");
  await reply(page, "```sql\nSELECT 42;\n```");

  await page.locator('.chat-sql-actions button:has-text("Insert at cursor")').click();
  await expect(page.locator("#chat-pane")).toBeVisible();
  await expect(page.locator(".chat-sql")).toBeVisible();
});

// --------------------------------------------------------------- one-click Run

/**
 * **Run is one click standing for two, and still a click.** The model executes
 * nothing: this is the person pressing a button, and the statement goes into a
 * buffer on the way so the editor shows what ran and undo puts it back.
 *
 * The active tab is used when it is spare — nothing typed, no file behind it.
 */
test("Run puts the statement in the spare active tab and runs it", async ({ page }) => {
  await openChat(page, { run_script: () => rowsResult([{ name: "n" }], [[42]]) });
  const before = await page.locator("#script-tabs .stab").count();
  await ask(page, "give me a query");
  await reply(page, "```sql\nSELECT 42;\n```");
  await assertNothingRan(page);

  await page.locator('.chat-sql-actions button:has-text("Run")').click();

  // Ran, in the tab that was already there.
  await expect
    .poll(async () => (await calls(page)).find((c) => c.cmd === "run_script")?.args.sql)
    .toContain("SELECT 42;");
  await expect(page.locator("#script-tabs .stab")).toHaveCount(before);
  // The buffer shows what ran, and the rows are on screen.
  expect(await editorText(page)).toContain("SELECT 42;");
  await expect(page.locator(".grid-host")).toContainText("42");
  // And history knows where it came from.
  await expect
    .poll(async () => (await calls(page)).map((c) => c.cmd))
    .toContain("remember_proposal");
});

/** Work in progress is never overwritten — that tab is not spare. */
test("Run opens a new tab rather than overwriting what you were writing", async ({ page }) => {
  await openChat(page, { run_script: () => rowsResult([{ name: "n" }], [[7]]) });
  await page.locator(".cm-content").click();
  await page.keyboard.type("-- mine, do not touch");
  const before = await page.locator("#script-tabs .stab").count();

  await ask(page, "give me a query");
  await reply(page, "```sql\nSELECT 7;\n```");
  await page.locator('.chat-sql-actions button:has-text("Run")').click();

  await expect(page.locator("#script-tabs .stab")).toHaveCount(before + 1);
  expect(await editorText(page)).toContain("SELECT 7;");
  expect(await editorText(page)).not.toContain("do not touch");

  // The typing is still there, in the tab it was typed in.
  await page.locator("#script-tabs .stab").first().click();
  expect(await editorText(page)).toContain("-- mine, do not touch");
});

/** One undo, not one per character: the replacement is a single history step. */
test("undo puts back what Run replaced", async ({ page }) => {
  await openChat(page, { run_script: () => rowsResult([{ name: "n" }], [[1]]) });
  const starter = await editorText(page);
  await ask(page, "q");
  await reply(page, "```sql\nSELECT 1;\n```");

  await page.locator('.chat-sql-actions button:has-text("Run")').click();
  expect(await editorText(page)).toContain("SELECT 1;");

  await page.locator(".cm-content").click();
  await page.keyboard.press("Control+z");
  expect(await editorText(page)).toBe(starter);
});

/**
 * With no connection there is nothing to run against, so the button says so
 * rather than disappearing: a disabled control with a reason is how somebody
 * finds out what the thing they wanted needs.
 */
test("Run is offered but disabled with no connection, and explains why", async ({ page }) => {
  stream = gate();
  await installBackend(page, {
    ...schemaBackend,
    assistant_status: () => READY,
    assistant_send: () => stream.promise,
  });
  await visit(page);
  await page.click("#btn-assistant");
  await ask(page, "q");
  await reply(page, "```sql\nSELECT 1;\n```");

  const run = page.locator('.chat-sql-actions button:has-text("Run")');
  await expect(run).toBeDisabled();
  await expect(run).toHaveAttribute("title", /Open a connection first/);
  await assertNothingRan(page);
});
