import { expect, test } from "@playwright/test";
import { calls, installBackend, visit } from "./harness";

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

/**
 * **Mounted, not on screen.** With no connection there is no tab, so what the
 * window shows is the no-connection page (Stage 32 §2) and the editor is
 * hidden behind it. It is still built, and still has the box it will be shown
 * at: the two share one grid cell precisely so that CodeMirror never measures
 * itself against nothing, and an editor that came back with the wrong line
 * heights after its first reveal is the failure this asserts against.
 */
test("boots, mounts the editor behind the blank page, and asks for its defaults", async ({
  page,
}) => {
  await installBackend(page);
  await visit(page);

  await expect(page.locator("#rail")).toHaveCount(1);
  await expect(page.locator("#blank-page")).toBeVisible();

  const box = await page.locator("#editor .cm-editor").evaluate((el) => {
    const r = el.getBoundingClientRect();
    return { w: Math.round(r.width), h: Math.round(r.height) };
  });
  expect(box.w).toBeGreaterThan(100);
  expect(box.h).toBeGreaterThan(50);

  const cmds = (await calls(page)).map((c) => c.cmd);
  // The browse limit and the row ceiling are owned by Rust; the UI must ask
  // rather than repeat them.
  expect(cmds).toContain("app_defaults");
  expect(cmds).toContain("list_profiles");
});

/**
 * Found by the first UI test this project was ever able to run.
 *
 * `refreshExportBar` was only reachable from `showResults`, which needs a tab —
 * and at boot there is no connection, so no tab. Copy and Export sat enabled
 * with nothing to act on: clicking either did nothing at all, which is the same
 * silent no-op that cost Stage 2 its delete button.
 */
test("copy and export are disabled when there is nothing to export", async ({ page }) => {
  await installBackend(page);
  await visit(page);

  await expect(page.locator("#btn-copy")).toBeDisabled();
  await expect(page.locator("#btn-copy-head")).toBeDisabled();
  await expect(page.locator("#btn-export")).toBeDisabled();
});

/**
 * A tab cannot exist without a connection — Stage 2 established that, after the
 * rule was broken from three separate call sites. With no connection there must
 * be no tab bar entries and no thrown error.
 */
test("no connection means no tabs, and no error", async ({ page }) => {
  await installBackend(page);
  await visit(page);
  await expect(page.locator("#script-tabs .stab")).toHaveCount(0);
});
