// Closing result sets.
//
// Before this there was no way to dismiss a result at all — no button, no menu
// item, no key. The only wipe primitive, `ResultView.setMessage`, is view-only:
// it nulls the *view's* copy and leaves `tab.result`, so anything that used it
// to "clear" the pane was undone by the next tab switch. Disconnecting did
// exactly that, which is why the last test here exists.

import { expect, test } from "@playwright/test";
import { connect, editorText, rowsResult } from "./harness";

/** A run of three statements, so the statement strip is rendered. */
function threeStatements() {
  const one = rowsResult([{ name: "a" }], [["1"]], { sql: "SELECT 1" });
  const two = rowsResult([{ name: "b" }], [["2"]], { sql: "SELECT 2" });
  const three = rowsResult([{ name: "c" }], [["3"]], { sql: "SELECT 3" });
  return {
    ...one,
    statements: [one.statements[0], two.statements[0], three.statements[0]],
  };
}

async function runScript(page: any) {
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+Shift+Enter");
  await page.waitForTimeout(150);
}

test("the button discards the result and disables copy and export", async ({ page }) => {
  await connect(page, { run_script: () => rowsResult([{ name: "a" }], [["1"]]) });
  await runScript(page);

  await expect(page.locator("#grid table.rs")).toBeVisible();
  await expect(page.locator("#btn-close-results")).toBeEnabled();

  await page.click("#btn-close-results");
  await expect(page.locator("#grid table.rs")).toHaveCount(0);
  await expect(page.locator("#grid .empty")).toContainText("No results yet");
  await expect(page.locator("#btn-copy")).toBeDisabled();
  await expect(page.locator("#btn-export")).toBeDisabled();
  await expect(page.locator("#btn-close-results")).toBeDisabled();

  // The script is untouched. Closing a result must never cost someone their
  // SQL — that is the whole reason this is not "close the tab".
  expect(await editorText(page)).toContain("SELECT 1;");
});

test("a closed result stays closed across a tab switch", async ({ page }) => {
  await connect(page, { run_script: () => rowsResult([{ name: "a" }], [["1"]]) });
  await runScript(page);
  await page.click("#btn-close-results");

  await page.keyboard.press("Control+t");
  await page.keyboard.press("Control+1");
  // The whole point: `setMessage` alone would have repainted the old rows here.
  await expect(page.locator("#grid table.rs")).toHaveCount(0);
  await expect(page.locator("#grid .empty")).toContainText("No results yet");
});

test("one statement can be closed, leaving the others", async ({ page }) => {
  await connect(page, { run_script: () => threeStatements() });
  await runScript(page);

  await expect(page.locator("#tabs .tab")).toHaveCount(3);
  await page.locator("#tabs .tab").nth(1).locator(".tab-close").click();

  await expect(page.locator("#tabs .tab")).toHaveCount(2);
  const labels = await page.locator("#tabs .tab").allInnerTexts();
  expect(labels.join(" ")).toContain("SELECT 1");
  expect(labels.join(" ")).toContain("SELECT 3");
  expect(labels.join(" ")).not.toContain("SELECT 2");
});

test("the strip disappears once one statement is left, and the button finishes it", async ({
  page,
}) => {
  await connect(page, { run_script: () => threeStatements() });
  await runScript(page);

  await page.locator("#tabs .tab").first().locator(".tab-close").click();
  await page.locator("#tabs .tab").first().locator(".tab-close").click();

  // A strip for one statement would be noise, so it hides — the last result is
  // closed with the button, which is always there.
  await expect(page.locator("#tabs .tab")).toHaveCount(0);
  await expect(page.locator("#grid table.rs")).toBeVisible();

  await page.click("#btn-close-results");
  // Not an empty grid with a live export bar — actually cleared.
  await expect(page.locator("#grid .empty")).toContainText("No results yet");
  await expect(page.locator("#btn-export")).toBeDisabled();
});

test("closing a statement does not select the one being closed", async ({ page }) => {
  await connect(page, { run_script: () => threeStatements() });
  await runScript(page);

  // Close the first while a later one is active: the active statement must
  // still be the same statement, not whatever slid into its index.
  await page.locator("#tabs .tab").nth(2).click();
  await page.locator("#tabs .tab").first().locator(".tab-close").click();
  await expect(page.locator("#tabs .tab.active")).toContainText("SELECT 3");
});

test("the cell menu closes the result", async ({ page }) => {
  await connect(page, { run_script: () => rowsResult([{ name: "a" }], [["1"]]) });
  await runScript(page);

  await page.locator('#grid td[data-col="0"]').first().click({ button: "right" });
  await page.locator(".ctx-menu button", { hasText: "Close this result" }).click();
  await expect(page.locator("#grid .empty")).toContainText("No results yet");
});

test("disconnecting discards the results it invalidated", async ({ page }) => {
  await connect(page, { run_script: () => rowsResult([{ name: "a" }], [["1"]]) });
  await runScript(page);
  await expect(page.locator("#grid table.rs")).toBeVisible();

  await page.locator(".rail-item").first().click({ button: "right" });
  await page.locator(".ctx-menu button", { hasText: "Disconnect" }).click();
  await expect(page.locator("#grid .empty")).toContainText("Disconnected");

  // The bug: this switch used to repaint rows fetched from a server we left.
  await page.keyboard.press("Control+t").catch(() => {});
  await expect(page.locator("#grid table.rs")).toHaveCount(0);
  await expect(page.locator("#btn-copy")).toBeDisabled();
  await expect(page.locator("#btn-export")).toBeDisabled();
});
