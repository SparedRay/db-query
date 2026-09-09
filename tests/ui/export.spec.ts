import { expect, test, type Page } from "@playwright/test";
import { CONN_INFO, READ_ONLY_CAPS, calls, connect, openDatabase, rowsResult } from "./harness";

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

const OUTCOME = {
  path: "/tmp/out.csv",
  rowsWritten: 2,
  truncatedSource: false,
  bytesWritten: 2048,
  warnings: [] as string[],
};

/** Connect, run something, and land with a grid full of rows. */
async function withRows(page: Page, result: unknown, extra = {}) {
  await connect(page, {
    run_script: () => result,
    export_csv: () => OUTCOME,
    export_inserts: () => OUTCOME,
    export_rerun: () => ({ ...OUTCOME, rowsWritten: 7500 }),
    ...extra,
  });
  await page.click("#btn-run-all");
  await page.locator("table.rs").waitFor();
}

const TWO_ROWS = rowsResult(
  [{ name: "id", sqlType: "INT" }, { name: "email" }],
  [[1, "ada@example.com"], [2, "grace@example.com"]],
);

// --------------------------------------------------------------------- E5

test("E5 — the CSV options reach the backend exactly as set", async ({ page }) => {
  await withRows(page, TWO_ROWS);
  await page.click("#btn-export");
  await expect(page.locator("#export-dialog")).toBeVisible();

  await page.fill("#csv-delim", ";");
  await page.fill("#csv-null", "\\N");
  await page.uncheck("#csv-crlf");
  await page.uncheck("#csv-bom");
  await page.check("#csv-guard");
  await page.click("#export-ok");

  const call = (await calls(page)).find((c) => c.cmd === "export_csv");
  expect(call?.args.options).toEqual({
    delimiter: ";",
    crlf: false,
    bom: false,
    headers: true,
    nullAs: "\\N",
    formulaGuard: true,
  });
  // And the rows that were on screen went with them.
  expect((call?.args.result as { rows: unknown[] }).rows).toHaveLength(2);
  await expect(page.locator("#export-dialog")).toBeHidden();
});

test("E5 — the format switch shows the right options and only those", async ({ page }) => {
  await withRows(page, TWO_ROWS);
  await page.click("#btn-export");

  await expect(page.locator("#export-csv-opts")).toBeVisible();
  await expect(page.locator("#export-sql-opts")).toBeHidden();

  await page.selectOption("#export-format", "inserts");
  await expect(page.locator("#export-csv-opts")).toBeHidden();
  await expect(page.locator("#export-sql-opts")).toBeVisible();
});

// --------------------------------------------------------------------- E6

test("E6 — SQL export sends the table, batch size and CREATE TABLE flag", async ({ page }) => {
  await withRows(page, TWO_ROWS);
  await page.click("#btn-export");
  await page.selectOption("#export-format", "inserts");
  await page.fill("#sql-table", "copy_of_users");
  await page.check("#sql-create");
  await page.fill("#sql-batch", "25");
  await page.click("#export-ok");

  const call = (await calls(page)).find((c) => c.cmd === "export_inserts");
  expect(call?.args.options).toMatchObject({
    table: "copy_of_users",
    createTable: true,
    batchSize: 25,
  });
});

/**
 * The source table decides whether `CREATE TABLE` is the server's exact DDL or a
 * widened guess, so it must only be set where it is actually known.
 */
test("E6 — a browsed table carries its source; an ad-hoc query does not", async ({ page }) => {
  await connect(page, {
    run_script: () => TWO_ROWS,
    export_inserts: () => OUTCOME,
  });

  // An ad-hoc run: no source table.
  await page.click("#btn-run-all");
  await page.locator("table.rs").waitFor();
  await page.click("#btn-export");
  await page.selectOption("#export-format", "inserts");
  await expect(page.locator("#sql-create-note")).toHaveText(/derived from the result columns/);
  await page.click("#export-cancel");

  // Browsed from the tree: the source is known, so the DDL can be exact.
  await openDatabase(page);
  await page.locator('.node.table:has-text("users")').dblclick();
  await page.click("#btn-run-all");
  await page.locator("table.rs").waitFor();
  await page.click("#btn-export");
  await page.selectOption("#export-format", "inserts");
  await expect(page.locator("#sql-create-note")).toHaveText(/server's own CREATE TABLE/);
  await expect(page.locator("#sql-table")).toHaveValue("users");

  await page.check("#sql-create");
  await page.click("#export-ok");
  const call = (await calls(page)).find((c) => c.cmd === "export_inserts");
  // `connectionId` is the profile id, minted in the browser, so only its shape
  // is knowable here. What matters is that db and table are the browsed ones.
  expect(call?.args.source).toMatchObject({ db: "poc", table: "users" });
  expect(typeof (call?.args.source as { connectionId: string }).connectionId).toBe("string");
});

// --------------------------------------------------------------------- E8

test("E8 — a truncated result says so and pre-selects the unbounded re-run", async ({ page }) => {
  await withRows(
    page,
    rowsResult([{ name: "id", sqlType: "INT" }], [[1], [2]], { truncated: true }),
  );
  await page.click("#btn-export");

  await expect(page.locator("#export-scope-note")).toHaveText(/cut off/);
  await expect(page.locator("#export-scope")).toHaveValue("all");
});

test("E8 — the unbounded re-run goes back to the server for every row", async ({ page }) => {
  await withRows(
    page,
    rowsResult([{ name: "id", sqlType: "INT" }], [[1], [2]], {
      truncated: true,
      sql: "SELECT id FROM big",
    }),
  );
  await page.click("#btn-export");
  await page.click("#export-ok");

  const call = (await calls(page)).find((c) => c.cmd === "export_rerun");
  expect(call?.args).toMatchObject({ sql: "SELECT id FROM big", format: "csv" });
  // The note, not the grid: an export must not cost the user their results.
  await expect(page.locator("#result-note")).toContainText(/7500/);
  await expect(page.locator("table.rs")).toBeVisible();
});

/**
 * The re-run returns the query's own columns, so it cannot honour a subset —
 * and it must say why rather than vanishing.
 */
test("E8 — the re-run is disabled, with a reason, while a subset is selected", async ({ page }) => {
  await withRows(page, TWO_ROWS);
  await page.click('tr[data-row="0"] .rownum');
  await page.click("#btn-export");

  const all = page.locator("#export-scope option").nth(1);
  // `toBeDisabled` does not report a disabled <option>, so assert the attribute.
  await expect(all).toHaveAttribute("disabled", "");
  await expect(all).toHaveText(/not available while a subset is selected/);
  await expect(page.locator("#export-scope")).toHaveValue("shown");
});

test("selecting everything is not a subset, so the re-run stays available", async ({ page }) => {
  await withRows(page, TWO_ROWS);
  await page.locator("#grid").click();
  await page.keyboard.press("Control+a");
  await page.click("#btn-export");
  await expect(page.locator("#export-scope option").nth(1)).not.toHaveAttribute("disabled", "");
});

test("only the selected rows are exported", async ({ page }) => {
  await withRows(
    page,
    rowsResult([{ name: "id", sqlType: "INT" }], [[1], [2], [3]]),
  );
  await page.click('tr[data-row="2"] .rownum');
  await page.click("#btn-export");
  await page.click("#export-ok");

  const call = (await calls(page)).find((c) => c.cmd === "export_csv");
  expect((call?.args.result as { rows: unknown[][] }).rows).toEqual([[3]]);
  // A hand-picked subset was never the whole query, so it is not "truncated".
  expect((call?.args.result as { truncated: boolean }).truncated).toBe(false);
});

// ----------------------------------------------------------- dialog behaviour

/** Cancelling the native save dialog returns null. That is not a failure. */
test("cancelling the save dialog leaves the export dialog usable", async ({ page }) => {
  await withRows(page, TWO_ROWS, { export_csv: () => null });
  await page.click("#btn-export");
  await page.click("#export-ok");

  await expect(page.locator("#export-dialog")).toBeVisible();
  await expect(page.locator("#export-error")).toBeHidden();
  await expect(page.locator("#export-ok")).toBeEnabled();
  await expect(page.locator("#export-ok")).toHaveText("Export");
});

/** A refusal keeps the values, so it can be corrected rather than re-entered. */
test("a backend refusal is shown in the dialog, which stays open", async ({ page }) => {
  await withRows(page, TWO_ROWS, {
    export_inserts: () => {
      throw new Error("Binary columns cannot be written as literals");
    },
  });
  await page.click("#btn-export");
  await page.selectOption("#export-format", "inserts");
  await page.fill("#sql-table", "keep_me");
  await page.click("#export-ok");

  await expect(page.locator("#export-dialog")).toBeVisible();
  await expect(page.locator("#export-error")).toContainText(/Binary columns/);
  await expect(page.locator("#sql-table")).toHaveValue("keep_me");
});

test("warnings from the backend are surfaced, not swallowed", async ({ page }) => {
  await withRows(page, TWO_ROWS, {
    export_csv: () => ({
      ...OUTCOME,
      warnings: ["NULLs were written as empty fields; CSV cannot tell the two apart."],
    }),
  });
  await page.click("#btn-export");
  await page.click("#export-ok");
  await expect(page.locator("#result-note")).toContainText(/CSV cannot tell the two apart/);
});

/**
 * A capability that nobody reads is decoration.
 *
 * `streamingExport` has been on `Capabilities` since Stage 11 and was consulted
 * by nothing, so an Elasticsearch connection — which has no streaming path at
 * all — was offered "re-run and export every row" and sent down a MySQL-only
 * road when it took it.
 */
test("an engine that cannot stream is not offered the unbounded re-run", async ({ page }) => {
  await connect(page, {
    connect: () => ({ ...CONN_INFO, capabilities: READ_ONLY_CAPS }),
    run_script: () => rowsResult([{ name: "a" }], [["1"]], { truncated: true }),
  });
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+Shift+Enter");
  await page.waitForTimeout(150);

  await page.click("#btn-export");
  // toBeDisabled() does not cover <option>; the property is the fact.
  const all = page.locator("#export-scope option").nth(1);
  await expect(all).toHaveJSProperty("disabled", true);
  await expect(all).toContainText("cannot stream");
  // And it must not have been *selected* by the truncation branch either.
  await expect(page.locator("#export-scope")).toHaveValue("shown");
});

test("an engine that can stream is still offered it", async ({ page }) => {
  await connect(page, {
    run_script: () => rowsResult([{ name: "a" }], [["1"]], { truncated: true }),
  });
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+Shift+Enter");
  await page.waitForTimeout(150);

  await page.click("#btn-export");
  const all = page.locator("#export-scope option").nth(1);
  await expect(all).toHaveJSProperty("disabled", false);
  await expect(page.locator("#export-scope")).toHaveValue("all");
});
