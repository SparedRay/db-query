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
async function copied(page: Page, browserName: string): Promise<string | null> {
  // Asserted everywhere: the copy reported success rather than an error.
  await expect(page.locator("#result-note")).toContainText(/^Copied /);
  if (browserName !== "chromium") return null;
  return page.evaluate(() => navigator.clipboard.readText());
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

test("copies the whole result with headers", async ({ page, browserName }) => {
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

  await page.click("#btn-copy");
  const text = await copied(page, browserName);
  if (text === null) return;

  expect(text).toContain("id\temail");
  expect(text).toContain("1\tada@example.com");
  expect(text).toContain("2\tgrace@example.com");
});

test("copies without headers when asked", async ({ page, browserName }) => {
  await connectAndRun(
    page,
    rowsResult([{ name: "id", sqlType: "INT" }], [[1], [2]]),
  );

  await page.click("#btn-copy-nohead");
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

  await page.click("#btn-copy");
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

  await page.click("#btn-copy-nohead");
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

  await page.click("#btn-copy-nohead");
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

  await page.click("#btn-copy-nohead");
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
  await page.click("#btn-copy-nohead");
  await expect(page.locator("table.rs")).toBeVisible();
});
