// The editing shortcuts a text editor is expected to have.
//
// Written because Ctrl+F did nothing at all — `@codemirror/search` was never
// installed, so there was no find in a SQL editor — and because Ctrl+Shift+Z
// undid instead of redoing: `historyKeymap` binds it only on platforms it
// recognises as Linux, and otherwise it falls through to plain undo.
//
// None of this is visible until someone reaches for the key, which is why it
// went unnoticed through eleven stages.

import { expect, test } from "@playwright/test";
import { connect, editorText } from "./harness";

async function type(page: any, text: string) {
  const cm = page.locator("#editor .cm-content");
  await cm.click();
  await page.keyboard.press("Control+End");
  await cm.pressSequentially(text);
}

test("ctrl+z undoes and ctrl+shift+z redoes", async ({ page }) => {
  await connect(page);
  await type(page, "ZZTOP");
  expect(await editorText(page)).toContain("ZZTOP");
  await page.keyboard.press("Control+z");
  expect(await editorText(page)).not.toContain("ZZTOP");
  await page.keyboard.press("Control+Shift+z");
  expect(await editorText(page)).toContain("ZZTOP");
});

test("ctrl+y also redoes", async ({ page }) => {
  await connect(page);
  await type(page, "ZZTOP");
  await page.keyboard.press("Control+z");
  await page.keyboard.press("Control+y");
  expect(await editorText(page)).toContain("ZZTOP");
});

test("ctrl+f opens the find panel", async ({ page }) => {
  await connect(page);
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+f");
  await expect(page.locator("#editor .cm-search")).toBeVisible();
  await expect(page.locator("#editor .cm-search input[name=search]")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(page.locator("#editor .cm-search")).toHaveCount(0);
});

test("undo survives a tab switch, and each tab has its own history", async ({ page }) => {
  await connect(page);
  await type(page, "ZZTOP");
  await page.keyboard.press("Control+t");
  await type(page, "SECOND");
  await page.keyboard.press("Control+1");
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+z");
  const back = await editorText(page);
  expect(back).not.toContain("ZZTOP");
  expect(back).not.toContain("SECOND");
});
