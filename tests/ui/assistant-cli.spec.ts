import { expect, test, type Page } from "@playwright/test";
import { calls, commandNames, connect, sendOnChannel, withSettings } from "./harness";

/**
 * The assistant, answered by a CLI the person is already signed into.
 *
 * Nothing here needs a vendor installed or a subscription: the probe and the
 * reply are both backend commands, so the stub covers them exactly as it
 * covers the HTTP provider. The real CLIs are exercised by
 * `src-tauri/tests/agentcli_live.rs` (`mise run test-cli`), which is where
 * "does Copilot still emit this shape" belongs.
 */

const SIGNED_IN = { installed: true, signedIn: true, detail: "" };
const NOT_INSTALLED = {
  installed: false,
  signedIn: false,
  detail: "`copilot` was not found on your PATH. Install Copilot CLI to use it here.",
};
const SIGNED_OUT = {
  installed: true,
  signedIn: false,
  detail: "Authentication required. Run `copilot login` in a terminal.",
};

/** `assistant_cli_send` must not resolve until the stream is over. */
function gate() {
  let release!: () => void;
  const promise = new Promise<void>((r) => {
    release = r;
  });
  return { promise, release };
}

let stream = gate();

async function boot(
  page: Page,
  opts: { enabled?: string[]; provider?: string; recipe?: string; probe?: unknown } = {},
) {
  stream = gate();
  await withSettings(page, {
    cliEnabled: opts.enabled ?? [],
    aiProvider: opts.provider ?? "anthropic",
    aiRecipe: opts.recipe ?? null,
  });
  await connect(page, {
    assistant_status: () => ({ hasKey: true, ready: true, local: false }),
    assistant_cli_probe: () => opts.probe ?? SIGNED_IN,
    assistant_cli_send: () => stream.promise,
    assistant_cli_cancel: () => null,
    assistant_send: () => stream.promise,
  });
}

async function openSettings(page: Page) {
  await page.click("#btn-settings");
  await page.click("#set-tab-assistant");
}

// --------------------------------------------------------------- off by default

/**
 * **An integration that is off costs nothing.** Not probed, not offered, and
 * no process spawned — which is the whole reason it is a switch rather than
 * something the app works out for itself.
 */
test("nothing is probed when no integration is enabled", async ({ page }) => {
  await boot(page);
  await openSettings(page);
  await expect(page.locator(".integration")).toHaveCount(2);
  await expect(page.locator('.integration[data-recipe="copilot"] .tool-detail')).toHaveText(
    "Off.",
  );
  expect(await commandNames(page)).not.toContain("assistant_cli_probe");
});

/** And a CLI you have not enabled is not somewhere questions can go. */
test("a disabled integration is not offered as a provider", async ({ page }) => {
  await boot(page);
  await openSettings(page);
  const labels = await page.locator("#set-ai-preset option").allTextContents();
  expect(labels).not.toContain("Copilot CLI");
  expect(labels).toContain("Anthropic");
});

// ------------------------------------------------------------------- warming

/**
 * **Enabled means warmed.** The probe runs at startup so the answer is already
 * there when the person asks — and it costs nothing, which is what makes that
 * honest rather than a way to spend their quota.
 */
test("an enabled integration is probed at startup, without being asked", async ({ page }) => {
  await boot(page, { enabled: ["copilot"] });
  await expect
    .poll(async () => (await calls(page)).filter((c) => c.cmd === "assistant_cli_probe").length)
    .toBeGreaterThan(0);

  const probes = (await calls(page)).filter((c) => c.cmd === "assistant_cli_probe");
  expect(probes[0]!.args.recipe).toBe("copilot");
  // Nothing was asked of a model to find that out.
  expect(await commandNames(page)).not.toContain("assistant_cli_send");
});

test("turning an integration on probes it and offers it", async ({ page }) => {
  await boot(page);
  await openSettings(page);
  await page.click('.integration[data-recipe="copilot"] input[type="checkbox"]');

  await expect(
    page.locator('.integration[data-recipe="copilot"] .tool-detail'),
  ).toHaveText(/installed and signed in/);
  const labels = await page.locator("#set-ai-preset option").allTextContents();
  expect(labels).toContain("Copilot CLI");
});

/** A CLI that is not installed says which binary was missing. */
test("a missing CLI names the binary rather than just failing", async ({ page }) => {
  await boot(page, { enabled: ["copilot"], probe: NOT_INSTALLED });
  await openSettings(page);
  await expect(
    page.locator('.integration[data-recipe="copilot"] .tool-detail'),
  ).toContainText("`copilot` was not found on your PATH");
});

/**
 * **The button says what the probe found, and stays clickable.**
 *
 * Opening the panel is how you read *why* it is not ready, so disabling the
 * button would hide the explanation behind the thing that needs explaining.
 */
test("the assistant button marks a CLI that is not ready, and still opens", async ({ page }) => {
  await boot(page, {
    enabled: ["copilot"],
    provider: "localCli",
    recipe: "copilot",
    probe: SIGNED_OUT,
  });
  await expect(page.locator("#btn-assistant.not-ready")).toHaveCount(1);
  await expect(page.locator("#btn-assistant")).toHaveAttribute("title", /Run `copilot login`/);
  await expect(page.locator("#btn-assistant")).toBeEnabled();

  await page.click("#btn-assistant");
  await expect(page.locator("#assistant-dialog")).toBeVisible();
  await expect(page.locator("#chat-note")).toContainText("Authentication required");
  // And it will not let a question be sent into a CLI that cannot answer.
  await expect(page.locator("#chat-send")).toBeDisabled();
});

test("a ready CLI leaves the button unmarked", async ({ page }) => {
  await boot(page, { enabled: ["copilot"], provider: "localCli", recipe: "copilot" });
  await expect(page.locator("#btn-assistant")).toHaveAttribute("title", /Ask Copilot CLI/);
  await expect(page.locator("#btn-assistant.not-ready")).toHaveCount(0);
});

// ------------------------------------------------------------------- settings

/**
 * A CLI has no key and no URL. The fields go away rather than sitting there
 * looking optional.
 */
test("choosing a CLI hides the key and base URL", async ({ page }) => {
  await boot(page, { enabled: ["copilot"] });
  await openSettings(page);
  await expect(page.locator("#ai-http-fields")).toBeVisible();

  await page.selectOption("#set-ai-preset", { label: "Copilot CLI" });
  await expect(page.locator("#ai-http-fields")).toBeHidden();
  await expect(page.locator("#set-ai-note")).toContainText(/installed and signed in/);
});

/**
 * Turning off the integration you are currently using must not leave the
 * provider pointing at it — that is a setting that contradicts itself.
 */
test("disabling the CLI in use moves the provider off it", async ({ page }) => {
  await boot(page, { enabled: ["copilot"], provider: "localCli", recipe: "copilot" });
  await openSettings(page);
  await page.click('.integration[data-recipe="copilot"] input[type="checkbox"]');

  await expect(page.locator("#ai-http-fields")).toBeVisible();
  const labels = await page.locator("#set-ai-preset option").allTextContents();
  expect(labels).not.toContain("Copilot CLI");
  await expect(page.locator("#btn-assistant.not-ready")).toHaveCount(0);
});

// ---------------------------------------------------------------- asking it

async function ask(page: Page, question: string) {
  await page.click("#btn-assistant");
  await expect(page.locator("#chat-send")).toBeEnabled();
  await page.fill("#chat-input", question);
  await page.click("#chat-send");
}

/** The question goes to the CLI command, not the HTTP one. */
test("a question with a CLI selected goes to the CLI", async ({ page }) => {
  await boot(page, { enabled: ["copilot"], provider: "localCli", recipe: "copilot" });
  await ask(page, "which tables hold orders?");

  await expect
    .poll(async () => (await calls(page)).filter((c) => c.cmd === "assistant_cli_send").length)
    .toBe(1);
  const sent = (await calls(page)).filter((c) => c.cmd === "assistant_cli_send")[0]!;
  expect(sent.args.recipe).toBe("copilot");
  expect(sent.args.requestId).toBeTruthy();
  expect(await commandNames(page)).not.toContain("assistant_send");

  await sendOnChannel(page, "assistant_cli_send", "onEvent", [
    { type: "started" },
    { type: "text", delta: "SELECT * FROM orders;" },
    { type: "done", stopReason: null },
  ]);
  stream.release();
  await expect(page.locator(".chat-msg.from-assistant")).toContainText("SELECT * FROM orders;");
});

/**
 * **A CLI reply can be stopped, and stopping it kills the process.**
 *
 * Milestone C8. A reply that is merely abandoned leaves the CLI talking to a
 * server and spending the person's quota, so Send becomes Stop for the whole
 * time a CLI reply is in flight — including the seconds before its first word,
 * which is exactly when someone would want it back.
 */
test("Send becomes Stop while a CLI answers, and cancels the request it started", async ({
  page,
}) => {
  await boot(page, { enabled: ["copilot"], provider: "localCli", recipe: "copilot" });
  await ask(page, "write me an essay");

  await expect(page.locator("#chat-send")).toHaveText("Stop");
  await expect(page.locator("#chat-send")).toBeEnabled();

  await page.click("#chat-send");
  await expect
    .poll(async () => (await calls(page)).filter((c) => c.cmd === "assistant_cli_cancel").length)
    .toBe(1);

  const started = (await calls(page)).filter((c) => c.cmd === "assistant_cli_send")[0]!;
  const stopped = (await calls(page)).filter((c) => c.cmd === "assistant_cli_cancel")[0]!;
  // The same request, not merely *a* cancel.
  expect(stopped.args.requestId).toBe(started.args.requestId);

  stream.release();
});

/** The HTTP path has nothing to stop, so it must not pretend to. */
test("an HTTP reply shows no Stop button", async ({ page }) => {
  await boot(page);
  await ask(page, "hello");
  await expect(page.locator("#chat-send")).toHaveText("Answering…");
  await expect(page.locator("#chat-send")).toBeDisabled();
  stream.release();
});

/**
 * A refusal is shown as the answer's own failure. The one that matters is the
 * tool-set refusal: if a vendor's flags stop working, the person is told rather
 * than quietly handed an agent with a shell.
 */
test("a refused reply says why, in the conversation", async ({ page }) => {
  await boot(page, { enabled: ["copilot"], provider: "localCli", recipe: "copilot" });
  await ask(page, "hello");
  await sendOnChannel(page, "assistant_cli_send", "onEvent", [
    { type: "started" },
    {
      type: "failed",
      message:
        "Copilot CLI offered the model 21 tools (bash, edit). The assistant is not allowed any, so this answer was stopped.",
    },
  ]);
  stream.release();

  await expect(page.locator(".chat-error")).toContainText("offered the model 21 tools");
});

// ------------------------------------------------------------- waiting for it

/**
 * **Anything slower than an eyeblink has to say so** — the rule the rail and the
 * tree already follow, applied to the two places in the chat that were silent.
 */

/** A gate the test opens, so "while it is in flight" is a state not a race. */
function heldProbe() {
  let open!: (v: unknown) => void;
  const p = new Promise<unknown>((r) => {
    open = r;
  });
  return { p, open };
}

/**
 * Opening the panel used to wait for the readiness check before showing
 * anything, and for a CLI that is a second or two of a click doing nothing.
 */
test("the panel opens before it knows whether the CLI is ready", async ({ page }) => {
  const held = heldProbe();
  stream = gate();
  await withSettings(page, {
    cliEnabled: ["copilot"],
    aiProvider: "localCli",
    aiRecipe: "copilot",
  });
  await connect(page, {
    assistant_status: () => ({ hasKey: true, ready: true, local: false }),
    assistant_cli_probe: () => held.p,
    assistant_cli_send: () => stream.promise,
  });

  await page.click("#btn-assistant");
  // Open, and saying so, while the probe is still out.
  await expect(page.locator("#assistant-dialog")).toBeVisible();
  await expect(page.locator("#chat-note .spinner")).toBeVisible();
  await expect(page.locator("#chat-note .chat-waiting")).toHaveAttribute(
    "aria-label",
    /Waiting/,
  );
  // And it does not pretend to be usable yet.
  await expect(page.locator("#chat-send")).toBeDisabled();

  held.open(SIGNED_IN);
  await expect(page.locator("#chat-note")).toContainText("signed in on this machine");
  await expect(page.locator("#chat-note .spinner")).toHaveCount(0);
  await expect(page.locator("#chat-send")).toBeEnabled();
});

/**
 * While a reply is awaited the chat showed a literal `…`, which is
 * indistinguishable from a one-character answer and from a panel that has died.
 */
test("waiting for a reply shows a spinner and a word, then the answer", async ({ page }) => {
  await boot(page, { enabled: ["copilot"], provider: "localCli", recipe: "copilot" });
  await ask(page, "which tables hold orders?");

  const waiting = page.locator(".chat-msg.from-assistant .chat-waiting");
  await expect(waiting).toBeVisible();
  await expect(waiting.locator(".spinner")).toBeVisible();

  // One of the words, with its ellipsis — not the bare "…" it used to be.
  const word = await waiting.locator(".chat-waiting-word").textContent();
  expect(word).toMatch(/^[A-Z][a-z]+…$/);
  const { WAITING_WORDS } = await import("../../src/assistant");
  expect(WAITING_WORDS).toContain(word!.replace("…", ""));

  // The rotating word is hidden from assistive tech; the line speaks once.
  await expect(waiting.locator(".chat-waiting-word")).toHaveAttribute("aria-hidden", "true");
  await expect(waiting).toHaveAttribute("role", "status");

  await sendOnChannel(page, "assistant_cli_send", "onEvent", [
    { type: "started" },
    { type: "text", delta: "SELECT 1;" },
    { type: "done", stopReason: null },
  ]);
  stream.release();

  await expect(page.locator(".chat-msg.from-assistant")).toContainText("SELECT 1;");
  await expect(waiting).toHaveCount(0);
});

/** The `started` event must not clear the line — the wait is not over yet. */
test("the line survives the CLI starting up", async ({ page }) => {
  await boot(page, { enabled: ["copilot"], provider: "localCli", recipe: "copilot" });
  await ask(page, "hello");
  await sendOnChannel(page, "assistant_cli_send", "onEvent", [{ type: "started" }]);

  await expect(page.locator(".chat-waiting .spinner")).toBeVisible();
  await expect(page.locator("#chat-send")).toHaveText("Stop");
  stream.release();
});

/** A turn that fails before saying anything still clears it. */
test("a failed reply leaves no spinner behind", async ({ page }) => {
  await boot(page, { enabled: ["copilot"], provider: "localCli", recipe: "copilot" });
  await ask(page, "hello");
  await sendOnChannel(page, "assistant_cli_send", "onEvent", [
    { type: "failed", message: "Copilot CLI is not signed in." },
  ]);
  stream.release();

  await expect(page.locator(".chat-error")).toContainText("not signed in");
  await expect(page.locator(".chat-waiting")).toHaveCount(0);
});
