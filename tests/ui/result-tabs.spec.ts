import { expect, test, type Page } from "@playwright/test";
import { calls, connect, editorText, rowsResult } from "./harness";

/**
 * The strip above a multi-statement result.
 *
 * Two claims, both about the same question — *which statement produced this?*
 * The tab says which result it is, and double-clicking it selects the SQL that
 * made it, in the editor, where the answer is.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

const SCRIPT = "SELECT 1;\nSELECT 2;\nSELECT 3;";

/** Three statements, with the offsets they really have in `SCRIPT`. */
function threeStatements() {
  const one = rowsResult([{ name: "a" }], [["1"]], { sql: "SELECT 1", start: 0 });
  const two = rowsResult([{ name: "b" }], [["2"]], { sql: "SELECT 2", start: 10 });
  const three = rowsResult([{ name: "c" }], [["3"]], { sql: "SELECT 3", start: 20 });
  return {
    ...one,
    statements: [one.statements[0], two.statements[0], three.statements[0]],
  };
}

/** Type a script and run the whole buffer. */
async function runScript(page: Page, text = SCRIPT) {
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+a");
  await page.keyboard.type(text);
  await page.keyboard.press("Control+Shift+Enter");
  await expect(page.locator("#tabs .tab")).toHaveCount(3);
}

const ranSql = async (page: Page) =>
  (await calls(page))
    .filter((c) => c.cmd === "run_script")
    .map((c) => c.args.sql as string);

async function open(page: Page, extra: Record<string, unknown> = {}) {
  await connect(page, {
    run_script: () => threeStatements(),
    // Run with no selection asks Rust which statement the cursor is in. The
    // real splitter decides it; here the last statement is the one the tests
    // leave the cursor in.
    statement_at_cursor: (a: Record<string, unknown>) => {
      const sql = String(a.sql);
      const at = sql.lastIndexOf("SELECT");
      return at === -1 ? null : { sql: sql.slice(at).replace(/;\s*$/, ""), start: at };
    },
    ...extra,
  });
}

test("each result says which result it is, and what made it", async ({ page }) => {
  await open(page);
  await runScript(page);

  const tabs = page.locator("#tabs .tab");
  await expect(tabs.nth(0)).toContainText("Result 1");
  await expect(tabs.nth(1)).toContainText("Result 2");
  await expect(tabs.nth(2)).toContainText("Result 3");
  // The SQL is still one hover away — it was the label, and every result of a
  // script that selects from one table wore the same forty characters.
  await expect(tabs.nth(1)).toHaveAttribute("title", /SELECT 2/);
  await expect(tabs.nth(1)).toHaveAttribute("title", /Double-click/);
  // The row count stays on the tab: it is what tells them apart at a glance.
  await expect(tabs.nth(1).locator(".badge")).toHaveText("1");
});

/**
 * **Double-click selects the statement.** Asserted through what the editor
 * then does with that selection: Run runs the selection, so if the right text
 * is selected, the right statement runs.
 */
test("double-clicking a result selects the statement that produced it", async ({ page }) => {
  await open(page);
  await runScript(page);
  expect(await ranSql(page)).toEqual([SCRIPT]);

  await page.locator("#tabs .tab").nth(1).dblclick();
  await page.keyboard.press("Control+Enter");

  await expect.poll(async () => (await ranSql(page)).length).toBe(2);
  expect((await ranSql(page))[1]).toBe("SELECT 2");
});

test("a single click still just switches results", async ({ page }) => {
  await open(page);
  await runScript(page);

  await page.locator("#tabs .tab").nth(2).click();
  await expect(page.locator("#tabs .tab").nth(2)).toHaveClass(/active/);
  // Nothing was selected, so Run still takes the statement under the cursor —
  // which is where typing left it, at the end of the script. Through the
  // button, because a shortcut would need the editor to have focus, and the
  // point here is that clicking a tab did not touch the editor at all.
  await page.click("#btn-run");
  await expect.poll(async () => (await ranSql(page)).length).toBe(2);
  expect((await ranSql(page))[1]).toBe("SELECT 3");
});

/**
 * **Where, not what.** Two identical statements are told apart only by their
 * offsets — searching for the text would find the first one for both results.
 * Asserted by typing over the selection: the character lands in the copy that
 * was revealed.
 */
test("two identical statements are told apart by where they are", async ({ page }) => {
  const first = rowsResult([{ name: "a" }], [["1"]], { sql: "SELECT 1", start: 0 });
  const second = rowsResult([{ name: "a" }], [["1"]], { sql: "SELECT 1", start: 10 });
  await open(page, {
    run_script: () => ({ ...first, statements: [first.statements[0], second.statements[0]] }),
  });
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+a");
  await page.keyboard.type("SELECT 1;\nSELECT 1;");
  await page.keyboard.press("Control+Shift+Enter");
  await expect(page.locator("#tabs .tab")).toHaveCount(2);

  await page.locator("#tabs .tab").nth(1).dblclick();
  await page.keyboard.type("X");
  expect(await editorText(page)).toBe("SELECT 1;\nX;");
});

/**
 * The offsets were true when the script ran, and the buffer has been editable
 * ever since. When the text has moved, the statement is searched for instead.
 */
test("a statement that has moved is still found", async ({ page }) => {
  await open(page);
  await runScript(page);

  // Push everything down: the recorded offsets now point at the wrong text.
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+Home");
  await page.keyboard.type("-- a note\n-- and another\n");

  await page.locator("#tabs .tab").nth(1).dblclick();
  await page.keyboard.press("Control+Enter");
  await expect.poll(async () => (await ranSql(page)).length).toBe(2);
  expect((await ranSql(page))[1]).toBe("SELECT 2");
});

/** Selecting the wrong lines and calling them the source would be worse. */
test("a statement that is gone selects nothing and says so", async ({ page }) => {
  await open(page);
  await runScript(page);

  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+a");
  await page.keyboard.type("SELECT 9;");

  await page.locator("#tabs .tab").nth(1).dblclick();
  await expect(page.locator("#grid .empty")).toContainText("no longer in this tab");
  await expect(page.locator("#grid .empty")).toContainText("SELECT 2");
});

/**
 * A statement that appears twice, once the recorded offsets no longer fit,
 * gives no reason to prefer either — so neither is chosen. (While the offsets
 * *do* fit, they decide it: that is the copy this result came from.)
 */
test("an ambiguous statement is not guessed at", async ({ page }) => {
  await open(page);
  await runScript(page);

  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+a");
  await page.keyboard.type("-- x\nSELECT 2;\nSELECT 2;");

  await page.locator("#tabs .tab").nth(1).dblclick();
  await expect(page.locator("#grid .empty")).toContainText("no longer in this tab");
});
