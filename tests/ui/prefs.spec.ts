import { expect, test, type Page } from "@playwright/test";
import { calls, connect, installBackend, ready, schemaBackend, visit, withSettings } from "./harness";

/**
 * Preferences, and the store they are really kept in.
 *
 * # What was wrong
 *
 * Reported from use: enabling an agent CLI never stuck — every launch needed
 * Settings opening, the integration enabling and the provider choosing again.
 *
 * The frontend was saving and reloading it correctly. What it was saving into
 * is `localStorage`, which belongs to the **webview's origin**, and this app
 * has two: `http://localhost:1420` under `mise run dev` and `tauri://localhost`
 * installed. Measured on 2026-10-03 under
 * `~/.local/share/com.dbquery.poc/localstorage/`: two separate files, the
 * packaged build's holding nothing but a theme. Connections, tabs and history
 * never had the problem, because all three live in the app config directory.
 *
 * So the record moved there too (`src-tauri/src/prefs.rs`), `localStorage`
 * stayed as the cache that can be read synchronously in the first frame, and
 * `src/boot.ts` copies one into the other before the app is loaded.
 *
 * These tests own the frontend half of that: which store wins, that a change
 * reaches the file, and that an existing installation's choices are carried
 * into it rather than reset by the change itself.
 */

const SIGNED_IN = { installed: true, signedIn: true, detail: "" };

async function boot(page: Page, extra: Record<string, unknown> = {}) {
  await connect(page, {
    assistant_status: () => ({ hasKey: true, ready: true, local: false }),
    assistant_cli_probe: () => SIGNED_IN,
    ...extra,
  });
}

const saves = async (page: Page) =>
  (await calls(page))
    .filter((c) => c.cmd === "prefs_save")
    .map((c) => (c.args as { settings: Record<string, unknown> }).settings);

/**
 * **The file is the record.** Nothing is in this origin's storage, and the
 * preferences still arrive — which is the whole point: a build that has never
 * been run before now starts on the choices made in the other one.
 */
test("preferences come back from the file with nothing in this origin's storage", async ({
  page,
}) => {
  await boot(page, {
    prefs_load: () => ({
      settings: { fontSize: 18, cliEnabled: ["copilot"], aiProvider: "localCli", aiRecipe: "copilot" },
      warning: null,
    }),
  });

  // Applied before the first frame, not after: the font size is a CSS variable
  // the editor is drawn with.
  await expect(page.locator("html")).toHaveAttribute("style", /--code-size:\s*18px/);
  // And the integration is on, so the CLI is where questions go.
  await expect(page.locator("#btn-assistant")).toHaveAttribute("title", /Ask Copilot CLI/);

  await page.click("#btn-settings");
  await page.click("#set-tab-assistant");
  await expect(
    page.locator('.integration[data-recipe="copilot"] input[type="checkbox"]'),
  ).toBeChecked();
});

/** Changing one writes it through, so the next launch of any build has it. */
test("changing a preference reaches the file", async ({ page }) => {
  await boot(page);
  await page.click("#btn-settings");
  await page.click("#set-tab-assistant");
  await page.click('.integration[data-recipe="copilot"] input[type="checkbox"]');

  await expect.poll(async () => (await saves(page)).length).toBeGreaterThan(0);
  const last = (await saves(page)).pop()!;
  expect(last.cliEnabled).toEqual(["copilot"]);
});

/**
 * **The change does not cost anybody their settings.**
 *
 * On the first launch after this, the file does not exist and the cache holds
 * choices somebody made. Those go up into the file rather than being
 * overwritten by the defaults.
 */
test("choices already in the webview's storage are carried into the file", async ({ page }) => {
  await withSettings(page, {
    cliEnabled: ["claude"],
    aiProvider: "localCli",
    aiRecipe: "claude",
    fontSize: 15,
  });
  await boot(page);

  await expect.poll(async () => (await saves(page)).length).toBeGreaterThan(0);
  const first = (await saves(page))[0]!;
  expect(first.cliEnabled).toEqual(["claude"]);
  expect(first.aiRecipe).toBe("claude");
  expect(first.fontSize).toBe(15);
});

/**
 * A file that could not be read is said out loud, in the place the session's
 * own warning is said. Starting silently on the defaults is how somebody
 * spends an afternoon wondering where their settings went.
 */
test("an unreadable preferences file explains itself", async ({ page }) => {
  // Deliberately not connected: the warning goes where a result would, and
  // connecting opens a tab whose own empty state paints over it. That is the
  // existing behaviour of that one slot, which the session's warning shares.
  await installBackend(page, {
    ...schemaBackend,
    prefs_load: () => ({
      settings: null,
      warning:
        "Your preferences could not be read (expected `,`). " +
        "The file was kept as /tmp/p.corrupt-1.",
    }),
  });
  await visit(page);
  await ready(page);
  await expect(page.locator("#grid")).toContainText("preferences could not be read");
});

/**
 * **A backend that never answers must not cost a window.** The file is a small
 * local read, so the wait is a guarantee rather than a budget — and losing the
 * race leaves the app on the cache, which is what it had before the file
 * existed.
 */
test("the app still starts when the preferences never arrive", async ({ page }) => {
  await withSettings(page, { fontSize: 17 });
  await boot(page, { prefs_load: () => new Promise(() => {}) });
  await expect(page.locator("#editor")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator("html")).toHaveAttribute("style", /--code-size:\s*17px/);
});
