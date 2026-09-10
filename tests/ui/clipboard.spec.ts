import { expect, test, type Page } from "@playwright/test";
import { installBackend, rowsResult } from "./harness";

/**
 * The question Stage 3 could not answer.
 *
 * `navigator.clipboard.writeText` needs a **trusted user gesture**, which is
 * exactly what could not be produced on the dev machine — no `xdotool`, Wayland
 * session, and a synthetic event does not count. Playwright's `click()` is a
 * real gesture, and the browser will hand the clipboard back for inspection, so
 * the whole round trip is checkable here and nowhere else.
 */

/**
 * Read back what was copied — where the engine allows it.
 *
 * **Only Chromium grants `clipboard-read`.** WebKit rejects the permission
 * outright, and `navigator.clipboard.readText()` there raises `NotAllowedError`
 * regardless. So the round trip can only be closed in Chromium.
 *
 * WebKit still runs every one of these tests and still performs a real copy —
 * and the app's own report of what happened is asserted in **both** engines.
 * That is the question that actually matters for the shipped app: WebKitGTK is
 * what Tauri uses on Linux, so "did the copy succeed there" is worth more than
 * "can the test harness read it back".
 */
/**
 * Windows stores clipboard text as CRLF and hands it back that way, so on that
 * platform the bytes read here are **not** the bytes the app wrote — the OS
 * rewrote them in transit. That is Windows' convention and the right behaviour
 * (pasting into Excel or Notepad wants CRLF), so the app should not fight it
 * and neither should these tests.
 *
 * Gated rather than unconditional on purpose: everywhere else the clipboard is
 * byte-transparent, so the exact-LF assertions below stay meaningful there. If
 * our own code ever started emitting CRLF, Linux CI would still catch it.
 */
const CLIPBOARD_REWRITES_NEWLINES = process.platform === "win32";

async function copied(page: Page, browserName: string): Promise<string | null> {
  // Asserted everywhere: the copy reported success rather than an error.
  await expect(page.locator("#result-note")).toContainText(/^Copied /);
  if (browserName !== "chromium") return null;
  const text = await page.evaluate(() => navigator.clipboard.readText());
  return CLIPBOARD_REWRITES_NEWLINES ? text.replace(/\r\n/g, "\n") : text;
}

const CONNECTED = {
  id: "c1",
  serverVersion: "8.4.0",
  databases: ["poc"],
  currentDatabase: "poc",
};

/** Connect through the real dialog, then run a query that returns known rows. */
async function connectAndRun(page: import("@playwright/test").Page, result: unknown) {
  await installBackend(page, {
    connect: () => CONNECTED,
    save_profile: (a) => ({
      profile: { ...(a.profile as object), rememberPassword: false },
      passwordWarning: null,
      passwordStored: false,
    }),
    list_tables: () => [],
    list_routines: () => [],
    run_script: () => result,
    use_database: () => null,
  });
  await page.goto("/");

  await page.click("#btn-connect");
  await expect(page.locator("#conn-dialog")).toBeVisible();
  await page.click("#conn-ok");
  await expect(page.locator("#conn-dialog")).toBeHidden();

  await page.click("#btn-run-all");
  await expect(page.locator("table.rs")).toBeVisible();
}

test("Copy with headers includes the header row", async ({ page, browserName }) => {
  await connectAndRun(
    page,
    rowsResult(
      [{ name: "id", sqlType: "INT" }, { name: "email" }],
      [
        [1, "ada@example.com"],
        [2, "grace@example.com"],
      ],
    ),
  );

  await page.click("#btn-copy-head");
  const text = await copied(page, browserName);
  if (text === null) return;

  expect(text).toContain("id\temail");
  expect(text).toContain("1\tada@example.com");
  expect(text).toContain("2\tgrace@example.com");
});

test("plain Copy carries no headers \u2014 the default", async ({ page, browserName }) => {
  await connectAndRun(
    page,
    rowsResult([{ name: "id", sqlType: "INT" }], [[1], [2]]),
  );

  await page.click("#btn-copy");
  const text = await copied(page, browserName);
  if (text === null) return;
  expect(text).not.toContain("id");
  expect(text.trim().split("\n")).toEqual(["1", "2"]);
});

/**
 * **The gap that column-only selection left.** Selecting a set of rows had no
 * gesture at all until the row-number gutter existed.
 */
test("copies only the selected rows", async ({ page, browserName }) => {
  await connectAndRun(
    page,
    rowsResult(
      [{ name: "id", sqlType: "INT" }, { name: "label" }],
      [[1, "one"], [2, "two"], [3, "three"], [4, "four"]],
    ),
  );

  // Rows 2 and 4, picked individually — the case that needs ctrl-click.
  await page.click('tr[data-row="1"] .rownum');
  await page.click('tr[data-row="3"] .rownum', { modifiers: ["Control"] });
  await expect(page.locator("#btn-copy")).toHaveText(/2 rows/);

  await page.click("#btn-copy-head");
  const text = await copied(page, browserName);
  if (text === null) return;
  const lines = text.trim().split("\n");
  expect(lines[0]).toBe("id\tlabel");
  expect(lines.slice(1)).toEqual(["2\ttwo", "4\tfour"]);
});

test("shift-click selects a contiguous range of rows", async ({ page, browserName }) => {
  await connectAndRun(
    page,
    rowsResult([{ name: "id", sqlType: "INT" }], [[1], [2], [3], [4], [5]]),
  );

  await page.click('tr[data-row="1"] .rownum');
  await page.click('tr[data-row="3"] .rownum', { modifiers: ["Shift"] });
  await expect(page.locator("#btn-copy")).toHaveText(/3 rows/);

  await page.click("#btn-copy");
  const text = await copied(page, browserName);
  if (text === null) return;
  expect(text.trim().split("\n")).toEqual(["2", "3", "4"]);
});

test("copies the block where selected rows and columns cross", async ({ page, browserName }) => {
  await connectAndRun(
    page,
    rowsResult(
      [{ name: "a" }, { name: "b" }, { name: "c" }],
      [
        ["a1", "b1", "c1"],
        ["a2", "b2", "c2"],
        ["a3", "b3", "c3"],
      ],
    ),
  );

  // A cell, then shift-click another: the intersection rule in one gesture.
  await page.click('tr[data-row="0"] td[data-col="1"]');
  await page.click('tr[data-row="1"] td[data-col="2"]', { modifiers: ["Shift"] });
  await expect(page.locator("#btn-copy")).toHaveText(/2 rows × 2 columns/);

  await page.click("#btn-copy");
  const text = await copied(page, browserName);
  if (text === null) return;
  expect(text.trim().split("\n")).toEqual(["b1\tc1", "b2\tc2"]);
});

/** A NULL must not become the string "NULL" or an empty cell by accident. */
test("a NULL copies as an empty field, not the word NULL", async ({ page, browserName }) => {
  await connectAndRun(
    page,
    rowsResult([{ name: "v" }], [[null], ["NULL"]]),
  );

  await page.click("#btn-copy");
  const text = await copied(page, browserName);
  if (text === null) return;
  // Row 1 is a real NULL, row 2 is the four-character string. Only the trailing
  // newline is stripped — `.trim()` would eat the empty first field, which is
  // the entire thing this test is about.
  expect(text.replace(/\n$/, "").split("\n")).toEqual(["", "NULL"]);
});

/**
 * **Copying used to delete the results it had just copied.**
 *
 * The confirmation went through `setMessage`, which replaces the grid — so the
 * table vanished, Copy went disabled, and copying twice meant re-running the
 * query. Every clipboard test above passed throughout, because they asserted
 * what reached the clipboard and never looked at what was left on screen.
 */
test("copying does not destroy the result it copied", async ({ page }) => {
  await connectAndRun(
    page,
    rowsResult([{ name: "a" }, { name: "b" }], [["1", "2"], ["3", "4"]]),
  );

  await page.click("#btn-copy");
  await expect(page.locator("#result-note")).toContainText(/^Copied /);

  await expect(page.locator("table.rs")).toBeVisible();
  await expect(page.locator("tr[data-row]")).toHaveCount(2);
  await expect(page.locator("#btn-copy")).toBeEnabled();

  // And it can be done again, which was the practical symptom.
  await page.click("#btn-copy");
  await expect(page.locator("table.rs")).toBeVisible();
});

// ------------------------------------------------- headers are opt-in

/**
 * Plain copy carries no headers.
 *
 * Pasting into another query, a spreadsheet column or a chat message is the
 * common case, and a stray header row there is something you have to notice
 * and delete. Headers are the deliberate act, so they get the modifier and the
 * second button.
 */
test("Ctrl+C copies without headers; Ctrl+Shift+C copies with them", async ({
  page,
  browserName,
}) => {
  await connectAndRun(
    page,
    rowsResult([{ name: "id", sqlType: "INT" }, { name: "label" }], [[1, "one"]]),
  );

  await page.locator("#grid").click();
  await page.keyboard.press("Control+c");
  const plain = await copied(page, browserName);
  if (plain !== null) {
    expect(plain).not.toContain("label");
    expect(plain.trim()).toBe("1\tone");
  }

  await page.keyboard.press("Control+Shift+c");
  const withHeaders = await copied(page, browserName);
  if (withHeaders !== null) {
    expect(withHeaders.trim().split("\n")).toEqual(["id\tlabel", "1\tone"]);
  }
});

/** The same pair, reachable without the keyboard. */
test("the cell menu offers both copies", async ({ page, browserName }) => {
  await connectAndRun(
    page,
    rowsResult([{ name: "id", sqlType: "INT" }, { name: "label" }], [[1, "one"]]),
  );

  await page.locator('td[data-col="0"]').click({ button: "right" });
  await page.locator(".ctx-menu button", { hasText: /^Copy$/ }).click();
  const plain = await copied(page, browserName);
  if (plain !== null) expect(plain.trim()).toBe("1\tone");

  await page.locator('td[data-col="0"]').click({ button: "right" });
  await page.locator(".ctx-menu button", { hasText: "Copy with headers" }).click();
  const withHeaders = await copied(page, browserName);
  if (withHeaders !== null) {
    expect(withHeaders.trim().split("\n")).toEqual(["id\tlabel", "1\tone"]);
  }
});

/**
 * What a click actually selects, and what Ctrl+C then copies.
 *
 * Reported from live use: "click on a column and Ctrl+C copies the whole row —
 * we just need the selected value". The cell gesture was already right; the
 * one next to it was not.
 *
 * `applyGesture` had a rule that clicking the only selected entry clears it, so
 * there is a way back to "nothing selected". It asked that question of one axis
 * while the other still held a selection. Click cell b2 — columns are now `{b}`
 * — then click b's header, and the rule read it as "you clicked the only
 * selected column, deselect it" and cleared everything. **Empty means all**, so
 * the copy silently widened from one cell to the entire result.
 *
 * The button label is asserted alongside the clipboard because it is the only
 * warning a user gets that the scope changed, and it runs on both engines.
 */

const THREE_BY_TWO = () =>
  rowsResult(
    [{ name: "a" }, { name: "b" }, { name: "c" }],
    [
      ["a1", "b1", "c1"],
      ["a2", "b2", "c2"],
    ],
  );

test("a cell copies that one value and nothing else", async ({ page, browserName }) => {
  await connectAndRun(page, THREE_BY_TWO());

  await page.click('tr[data-row="1"] td[data-col="1"]');
  await expect(page.locator("#btn-copy")).toHaveText(/1 row × 1 column/);

  await page.keyboard.press("Control+c");
  const text = await copied(page, browserName);
  if (text === null) return;
  expect(text.trim()).toBe("b2");
});

test("a column header after a cell in it selects the column, not everything", async ({
  page,
  browserName,
}) => {
  await connectAndRun(page, THREE_BY_TWO());

  await page.click('tr[data-row="1"] td[data-col="1"]');
  await page.click('th[data-col="1"]');
  await expect(page.locator("#btn-copy")).toHaveText(/1 column/);

  await page.keyboard.press("Control+c");
  const text = await copied(page, browserName);
  if (text === null) return;
  expect(text.trim().split("\n")).toEqual(["b1", "b2"]);
});

test("a row number after a cell in it selects the row, not everything", async ({
  page,
  browserName,
}) => {
  await connectAndRun(page, THREE_BY_TWO());

  await page.click('tr[data-row="1"] td[data-col="1"]');
  await page.click('tr[data-row="1"] th.rownum');
  await expect(page.locator("#btn-copy")).toHaveText(/1 row$/);

  await page.keyboard.press("Control+c");
  const text = await copied(page, browserName);
  if (text === null) return;
  expect(text.trim()).toBe("a2\tb2\tc2");
});

/**
 * And the way back is still there. The fix narrows when the deselect applies;
 * it must not remove it, or there is no gesture for "stop selecting".
 */
test("clicking a selected column header again clears the selection", async ({ page }) => {
  await connectAndRun(page, THREE_BY_TWO());

  await page.click('th[data-col="1"]');
  await expect(page.locator("#btn-copy")).toHaveText(/1 column/);
  await page.click('th[data-col="1"]');
  await expect(page.locator("#btn-copy")).toHaveText(/^Copy$/);
});
