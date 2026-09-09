import { expect, test } from "@playwright/test";
import { installBackend } from "./harness";

/**
 * The licence notices, reachable from inside the app.
 *
 * Shipping the file is the obligation; being able to read it without unpacking
 * the installer is what makes shipping it mean anything. Attribution was
 * deferred through two whole stages precisely because nothing ever failed
 * without it — so it gets tests like anything else.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

const NOTICES = "db-query — third-party licences\n\nCRATES (MIT License: 359)";

async function openSettings(page: import("@playwright/test").Page) {
  await page.click("#btn-settings");
  await expect(page.locator("#settings-dialog")).toBeVisible();
}

test("the licences are readable from settings", async ({ page }) => {
  await installBackend(page, { third_party_licenses: () => NOTICES });
  await page.goto("/");
  await openSettings(page);

  await page.click("#set-licences");
  await expect(page.locator("dialog.viewer")).toBeVisible();
  await expect(page.locator("dialog.viewer")).toContainText("third-party licences");
  await expect(page.locator("dialog.viewer")).toContainText("MIT License: 359");
});

/**
 * A source build has no generated file. That is not a failure to hide — the
 * message names the command that makes one.
 */
test("a build with no generated file says how to generate one", async ({ page }) => {
  await installBackend(page, {
    third_party_licenses: () => {
      throw new Error("This build carries no licence file. Run:\n\n    npm run attribution");
    },
  });
  await page.goto("/");
  await openSettings(page);

  await page.click("#set-licences");
  await expect(page.locator("dialog.viewer")).toBeVisible();
  await expect(page.locator("dialog.viewer")).toContainText("npm run attribution");
});

/** Nothing is fetched until it is asked for — settings open on every launch. */
test("opening settings does not read the licence file", async ({ page }) => {
  await installBackend(page, { third_party_licenses: () => NOTICES });
  await page.goto("/");
  await openSettings(page);

  const names = await page.evaluate(
    () => (window as unknown as { __CALLS__: Array<{ cmd: string }> }).__CALLS__.map((c) => c.cmd),
  );
  expect(names).not.toContain("third_party_licenses");
});
