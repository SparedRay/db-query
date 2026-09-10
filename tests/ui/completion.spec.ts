import { expect, test, type Page } from "@playwright/test";
import { connect, editorText, schemaBackend } from "./harness";

/**
 * Autocomplete: **Tab accepts, Enter does not.**
 *
 * CodeMirror's default is the other way round — Enter accepts and Tab is
 * unbound. In a SQL buffer the popup opens while you type, so Enter, the key
 * you press to start the next line, silently becomes "take whatever is
 * highlighted". You get an identifier you never chose instead of a newline.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

const POPUP = ".cm-tooltip-autocomplete";

/** An empty buffer, and the popup open on a partly-typed keyword. */
async function offering(page: Page, prefix: string) {
  await connect(page, schemaBackend);
  const cm = page.locator("#editor .cm-content");
  await cm.click();
  await page.keyboard.press("Control+a");
  await page.keyboard.press("Delete");
  await cm.pressSequentially(prefix);
  // Explicit, so the test does not depend on when the popup decides to appear
  // by itself. Ctrl+Space is CodeMirror's own binding and we kept it.
  await page.keyboard.press("Control+Space");
  await expect(page.locator(POPUP)).toBeVisible();
  // **CodeMirror ignores an accept within `interactionDelay` of the popup
  // opening — 75ms by default** — so that a keystroke already in flight cannot
  // take an option nobody has seen yet. A test that skips this wait is faster
  // than any human and measures the guard instead of the binding: the Enter
  // case in particular would pass whether or not Enter still accepted.
  await page.waitForTimeout(200);
}

test("Tab accepts the highlighted completion", async ({ page }) => {
  await offering(page, "sel");

  await page.keyboard.press("Tab");

  await expect.poll(() => editorText(page)).toContain("SELECT");
  await expect(page.locator(POPUP)).toBeHidden();
  // A completion, not an indent: the word was replaced, not pushed right.
  expect(await editorText(page)).not.toContain("sel ");
  expect(await editorText(page)).not.toMatch(/\tsel/);
});

/** The whole point of the change. */
test("Enter starts a new line instead of accepting", async ({ page }) => {
  await offering(page, "sel");

  await page.keyboard.press("Enter");

  const text = await editorText(page);
  expect(text, "Enter must not have accepted the completion").not.toContain("SELECT");
  expect(text).toContain("sel");
  expect(text.split("\n").length, "and it must have made a line").toBeGreaterThan(1);
});

/**
 * Tab still indents when there is no popup — `acceptCompletion` returns false
 * with nothing open, so the binding falls through to `indentWithTab`. Without
 * that, adding this would have taken indentation away.
 */
test("Tab still indents when nothing is being offered", async ({ page }) => {
  await connect(page, schemaBackend);
  const cm = page.locator("#editor .cm-content");
  await cm.click();
  await page.keyboard.press("Control+a");
  await page.keyboard.press("Delete");
  await cm.pressSequentially("x");
  await expect(page.locator(POPUP)).toBeHidden();

  await page.keyboard.press("Home");
  await page.keyboard.press("Tab");

  await expect.poll(() => editorText(page)).toMatch(/^\s+x/);
});

/** Escape still dismisses it, and then Enter is an ordinary Enter again. */
test("Escape closes the popup and leaves the word alone", async ({ page }) => {
  await offering(page, "sel");

  await page.keyboard.press("Escape");

  await expect(page.locator(POPUP)).toBeHidden();
  expect(await editorText(page)).toContain("sel");
  expect(await editorText(page)).not.toContain("SELECT");
});
