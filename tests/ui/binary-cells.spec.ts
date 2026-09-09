// Binary values, which used to arrive as the *string* `<binary, 12 bytes>`.
//
// That representation could not be told apart from a real string of those
// characters, so nothing downstream could rely on it and the SQL export decided
// from the column type instead — refusing a binary-collation VARCHAR that the
// grid was happily showing as text. The value carries the truth now, which
// means it also has to render correctly here: an object reaching `String(v)`
// shows up as [object Object], and nobody writes a test for that until it has
// already shipped.

import { expect, test } from "@playwright/test";
import { connect, rowsResult } from "./harness";

const binaryRows = () =>
  rowsResult(
    [{ name: "id", sqlType: "INT" }, { name: "data", sqlType: "BLOB" }],
    [
      [1, { bytes: 12 }],
      [2, { bytes: null }],
      [3, "readable text"],
    ],
  );

async function run(page: any) {
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+Shift+Enter");
  await page.waitForTimeout(150);
}

test("a binary cell renders as a placeholder, not [object Object]", async ({ page }) => {
  await connect(page, { run_script: () => binaryRows() });
  await run(page);

  const cells = page.locator('#grid td[data-col="1"]');
  await expect(cells.nth(0)).toHaveText("<binary, 12 bytes>");
  // Unknown length is still binary, and must not read as "0 bytes".
  await expect(cells.nth(1)).toHaveText("<binary>");
  // Text in the same BLOB-typed column stays text — the whole point.
  await expect(cells.nth(2)).toHaveText("readable text");

  const all = await page.locator("#grid table.rs").innerText();
  expect(all).not.toContain("object Object");
});

test("the full-value viewer shows the placeholder too", async ({ page }) => {
  await connect(page, { run_script: () => binaryRows() });
  await run(page);

  await page.locator('#grid td[data-col="1"]').first().click({ button: "right" });
  await page.locator(".ctx-menu button", { hasText: "Open in full view" }).click();
  await expect(page.locator("dialog.viewer")).toBeVisible();
  await expect(page.locator("dialog.viewer")).toContainText("<binary, 12 bytes>");
  await expect(page.locator("dialog.viewer")).not.toContainText("object Object");
});
