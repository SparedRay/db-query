import { expect, test, type Page } from "@playwright/test";
import { calls, connect, editorText, installBackend, schemaBackend, sendOnChannel } from "./harness";

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

const CONFIGURED = { configured: true, model: "claude-opus-5" };

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
    assistant_status: () => CONFIGURED,
    assistant_send: () => stream.promise,
    ...extra,
  });
  await page.click("#btn-assistant");
  await expect(page.locator("#assistant-dialog")).toBeVisible();
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
    assistant_status: () => ({ configured: false, model: "claude-opus-5" }),
  });
  await page.goto("/");
  await page.click("#btn-assistant");

  await expect(page.locator("#chat-note")).toContainText("No API key set");
  await expect(page.locator("#chat-send")).toBeDisabled();
  await expect(page.locator("#chat-input")).toBeDisabled();
});

/** The promise the UI makes on the feature's behalf, so it is pinned. */
test("the chat states what is sent and what is not", async ({ page }) => {
  await openChat(page);
  await expect(page.locator("#chat-note")).toContainText("row data never is");
});

// ------------------------------------------------------------- the key itself

test("saving a key hands it over and does not keep it in the field", async ({ page }) => {
  await connect(page, {
    assistant_status: () => ({ configured: false, model: "claude-opus-5" }),
    assistant_set_key: () => true,
  });
  await page.click("#btn-settings");
  await page.fill("#set-ai-key", "sk-ant-secret");
  await page.click("#set-ai-save");

  await expect
    .poll(async () => (await calls(page)).find((c) => c.cmd === "assistant_set_key")?.args.key)
    .toBe("sk-ant-secret");
  // The field exists to hand the key over, not to hold it.
  await expect(page.locator("#set-ai-key")).toHaveValue("");
});

test("forgetting the key sends null", async ({ page }) => {
  await connect(page, { assistant_status: () => CONFIGURED, assistant_set_key: () => false });
  await page.click("#btn-settings");
  await page.click("#set-ai-forget");

  await expect
    .poll(async () => (await calls(page)).find((c) => c.cmd === "assistant_set_key")?.args.key)
    .toBe(null);
});
