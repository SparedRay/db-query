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
  await expect(page.locator("#btn-settings")).toBeVisible();
}

/** The theme now lives in Settings, so choosing one means opening it. */
async function chooseTheme(page: Page, value: "system" | "light" | "dark") {
  await page.click("#btn-settings");
  await page.selectOption("#set-theme", value);
  await page.click("#set-close");
}

test("follows the system by default, in both directions", async ({ page }) => {
  await boot(page, "light");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.locator("html")).toHaveAttribute("data-theme-pref", "system");

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

test("each theme can be chosen explicitly", async ({ page }) => {
  await boot(page, "dark");
  const html = page.locator("html");
  await expect(html).toHaveAttribute("data-theme-pref", "system");

  await chooseTheme(page, "light");
  await expect(html).toHaveAttribute("data-theme", "light");
  await expect(html).toHaveAttribute("data-theme-pref", "light");

  await chooseTheme(page, "dark");
  await expect(html).toHaveAttribute("data-theme", "dark");

  // Back to following the system, which here is dark.
  await chooseTheme(page, "system");
  await expect(html).toHaveAttribute("data-theme-pref", "system");
  await expect(html).toHaveAttribute("data-theme", "dark");
});

/** An explicit choice must beat the system, or it is not a choice. */
test("an explicit choice overrides the system and survives a reload", async ({ page }) => {
  await boot(page, "dark");
  await chooseTheme(page, "light"); // against a dark system
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");

  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.locator("html")).toHaveAttribute("data-theme-pref", "light");
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

  await chooseTheme(page, "light");
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
test("the settings cog survives the rail being rebuilt", async ({ page }) => {
  await boot(page, "dark");
  await page.click("#btn-connect");
  await page.click("#conn-ok");
  await expect(page.locator(".rail-item")).toHaveCount(1);
  await expect(page.locator("#btn-settings")).toBeVisible();
});
