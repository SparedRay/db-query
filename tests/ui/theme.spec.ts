import { expect, test, type Page } from "@playwright/test";
import { installBackend, schemaBackend } from "./harness";

/**
 * Light and dark.
 *
 * `data-theme` is always a *resolved* value — "light" or "dark", never
 * "system" — so the stylesheet needs one override block and the editor can be
 * told which way it went. The preference is kept on the button's `data-pref`,
 * because it cannot be recovered from `data-theme` alone.
 */

const bg = (page: Page) =>
  page.evaluate(() => getComputedStyle(document.body).backgroundColor);
const editorBg = (page: Page) =>
  page.evaluate(() => getComputedStyle(document.querySelector(".cm-editor")!).backgroundColor);

async function boot(page: Page, scheme: "dark" | "light" = "dark") {
  await page.emulateMedia({ colorScheme: scheme });
  await installBackend(page, schemaBackend);
  await page.goto("/");
  await expect(page.locator("#btn-theme")).toBeVisible();
}

test("follows the system by default, in both directions", async ({ page }) => {
  await boot(page, "light");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.locator("#btn-theme")).toHaveAttribute("data-pref", "system");

  await page.emulateMedia({ colorScheme: "dark" });
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
});

/** Following the system means following it as it changes, not only at boot. */
test("a system change is picked up without a reload", async ({ page }) => {
  await boot(page, "dark");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.emulateMedia({ colorScheme: "light" });
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
});

test("the button cycles system, light, dark and back", async ({ page }) => {
  await boot(page, "dark");
  const btn = page.locator("#btn-theme");
  await expect(btn).toHaveAttribute("data-pref", "system");

  await btn.click();
  await expect(btn).toHaveAttribute("data-pref", "light");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");

  await btn.click();
  await expect(btn).toHaveAttribute("data-pref", "dark");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");

  await btn.click();
  await expect(btn).toHaveAttribute("data-pref", "system");
});

/** An explicit choice must beat the system, or it is not a choice. */
test("an explicit choice overrides the system and survives a reload", async ({ page }) => {
  await boot(page, "dark");
  await page.locator("#btn-theme").click(); // -> light, against a dark system
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");

  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.locator("#btn-theme")).toHaveAttribute("data-pref", "light");
});

/**
 * The point of the whole token refactor: a literal colour anywhere is one that
 * cannot follow the theme. Comparing the painted result catches that in a way
 * reading the stylesheet does not.
 */
test("the painted colours actually change, editor included", async ({ page }) => {
  await boot(page, "dark");
  const darkBody = await bg(page);
  const darkEditor = await editorBg(page);

  await page.locator("#btn-theme").click(); // light
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  const lightBody = await bg(page);
  const lightEditor = await editorBg(page);

  expect(lightBody).not.toBe(darkBody);
  expect(lightEditor).not.toBe(darkEditor);
  // Light really is lighter — a swap that merely differs would pass a
  // not-equal check while looking wrong.
  const lum = (c: string) => c.match(/\d+/g)!.slice(0, 3).reduce((a, v) => a + +v, 0);
  expect(lum(lightBody)).toBeGreaterThan(lum(darkBody));
  expect(lum(lightEditor)).toBeGreaterThan(lum(darkEditor));
});

/** The rail is rebuilt on every connection change; the button must survive. */
test("the theme button survives the rail being rebuilt", async ({ page }) => {
  await boot(page, "dark");
  await page.click("#btn-connect");
  await page.click("#conn-ok");
  await expect(page.locator(".rail-item")).toHaveCount(1);
  await expect(page.locator("#btn-theme")).toBeVisible();
});
