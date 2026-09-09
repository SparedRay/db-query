import { expect, test, type Page } from "@playwright/test";
import { calls, connect, editorText, installBackend, schemaBackend } from "./harness";

/**
 * Query history — deferred since Stage 0, and on every backlog since.
 *
 * The rule it inherits from every other generated-SQL path in this app: it
 * **never runs anything**. Choosing a statement puts it in the editor and stops
 * there. Half of what is asserted here is that nothing executed.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

const NOW = Date.now();

function hit(over: Record<string, unknown> = {}) {
  return {
    at: NOW - 60_000,
    connectionId: "c1",
    database: "poc",
    sql: "SELECT * FROM orders",
    kind: "select",
    status: "ok",
    rows: 12,
    elapsedMs: 4,
    error: null,
    runs: 1,
    ...over,
  };
}

/** Nothing ran unless the test made it run. */
async function assertNothingRan(page: Page) {
  const names = (await calls(page)).map((c) => c.cmd);
  expect(names).not.toContain("run_script");
}

/** The most recent history_search call. `.at(-1)` is newer than our TS lib. */
async function lastSearch(page: Page) {
  const all = (await calls(page)).filter((c) => c.cmd === "history_search");
  return all[all.length - 1];
}

async function openHistory(page: Page, hits: unknown[]) {
  await connect(page, { history_search: () => hits });
  await page.click("#btn-history");
  await expect(page.locator("#history-dialog")).toBeVisible();
}

// --------------------------------------------------------------------- list

test("history lists what was run, newest first, with its outcome", async ({ page }) => {
  await openHistory(page, [
    hit({ sql: "SELECT * FROM orders", rows: 12 }),
    hit({ sql: "DROP TABLE nope", status: "error", rows: null, error: "Unknown table 'nope'" }),
  ]);

  const rows = page.locator(".hist-row");
  await expect(rows).toHaveCount(2);
  await expect(rows.first()).toContainText("SELECT * FROM orders");
  await expect(rows.first()).toContainText("12 rows");
  // A failed statement is often the one you are looking for, so it says why.
  await expect(rows.nth(1)).toContainText("Unknown table 'nope'");
  await expect(rows.nth(1).locator(".failed")).toBeVisible();
});

test("a statement run many times is one row that says so", async ({ page }) => {
  await openHistory(page, [hit({ runs: 7 })]);
  await expect(page.locator(".hist-row")).toHaveCount(1);
  await expect(page.locator(".hist-row").first()).toContainText("run 7×");
});

test("an empty history explains itself rather than looking broken", async ({ page }) => {
  await openHistory(page, []);
  await expect(page.locator(".hist-empty")).toContainText("statements you run are recorded");
});

/** The absence has to be explained, or it reads as a bug. */
test("the dialog says credentials are never recorded", async ({ page }) => {
  await openHistory(page, [hit()]);
  await expect(page.locator("#hist-note")).toContainText("credential");
});

// ------------------------------------------------------------------ picking

test("choosing a statement puts it in the editor and runs nothing", async ({ page }) => {
  await openHistory(page, [hit({ sql: "SELECT 42" })]);

  await page.locator(".hist-row").first().click();
  await expect(page.locator("#history-dialog")).toBeHidden();
  expect(await editorText(page)).toContain("SELECT 42");
  await assertNothingRan(page);
});

test("arrow keys move and Enter takes the highlighted one", async ({ page }) => {
  await openHistory(page, [hit({ sql: "SELECT 1" }), hit({ sql: "SELECT 2" })]);

  await page.keyboard.press("ArrowDown");
  await expect(page.locator(".hist-row").nth(1)).toHaveClass(/selected/);
  await page.keyboard.press("Enter");

  await expect(page.locator("#history-dialog")).toBeHidden();
  expect(await editorText(page)).toContain("SELECT 2");
  await assertNothingRan(page);
});

test("the context menu opens a statement in its own tab, and runs nothing", async ({ page }) => {
  await openHistory(page, [hit({ sql: "SELECT 99" })]);
  const before = await page.locator("#script-tabs .stab").count();

  await page.locator(".hist-row").first().click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Open in a new tab")').click();

  await expect(page.locator("#script-tabs .stab")).toHaveCount(before + 1);
  expect(await editorText(page)).toContain("SELECT 99");
  await assertNothingRan(page);
});

// ---------------------------------------------------------------- searching

test("typing searches, and the search reaches the backend", async ({ page }) => {
  await openHistory(page, [hit()]);
  await page.fill("#hist-search", "orders");

  await expect
    .poll(async () =>
      (await lastSearch(page))?.args.query,
    )
    .toBe("orders");
});

test("a search with no matches says so, distinctly from an empty history", async ({ page }) => {
  await connect(page, { history_search: (a) => (a.query ? [] : [hit()]) });
  await page.click("#btn-history");
  await expect(page.locator(".hist-row")).toHaveCount(1);

  await page.fill("#hist-search", "zzz");
  await expect(page.locator(".hist-empty")).toContainText("Nothing matches");
});

/**
 * The connection you are looking at is the one you almost always mean.
 *
 * The id is read back from the `connect` call rather than hardcoded: the
 * connection dialog mints its own profile id, so a literal here would be
 * asserting the harness rather than the app.
 */
test("the search is scoped to the active connection by default", async ({ page }) => {
  await openHistory(page, [hit()]);
  const profile = (await calls(page)).find((c) => c.cmd === "connect")?.args.profile as {
    id: string;
  };
  expect(profile.id).toBeTruthy();

  await expect(page.locator("#hist-this-conn")).toBeChecked();
  expect(
    (await lastSearch(page))?.args.connectionId,
  ).toBe(profile.id);

  await page.uncheck("#hist-this-conn");
  await expect
    .poll(async () =>
      (await lastSearch(page))?.args.connectionId,
    )
    .toBe(null);
});

// ----------------------------------------------------------------- clearing

test("clearing asks first, and Cancel clears nothing", async ({ page }) => {
  await openHistory(page, [hit()]);

  await page.click("#hist-clear");
  await expect(page.locator("dialog.ask")).toBeVisible();
  await page.locator('dialog.ask button:has-text("Cancel")').click();

  expect((await calls(page)).map((c) => c.cmd)).not.toContain("history_clear");
});

test("confirming clears, scoped the way the list is scoped", async ({ page }) => {
  await openHistory(page, [hit()]);

  const profile = (await calls(page)).find((c) => c.cmd === "connect")?.args.profile as {
    id: string;
  };

  await page.click("#hist-clear");
  await page.locator('dialog.ask button:has-text("Clear")').click();

  // Scoped, because the list was scoped — clearing more than you were looking
  // at is the kind of surprise that only shows up after the data is gone.
  await expect
    .poll(async () => (await calls(page)).find((c) => c.cmd === "history_clear")?.args)
    .toMatchObject({ connectionId: profile.id });
});

// ------------------------------------------------------------------ opening

test("Ctrl+H opens it, and pressing it again does not throw", async ({ page }) => {
  await connect(page, { history_search: () => [hit()] });
  await page.keyboard.press("Control+h");
  await expect(page.locator("#history-dialog")).toBeVisible();

  // `showModal()` on an open dialog throws — the pageerror hook would catch it.
  await page.keyboard.press("Control+h");
  await expect(page.locator("#history-dialog")).toBeVisible();
});

/** Without a connection there is nowhere to put a tab; it must say so. */
test("with no connection, history still opens and is not connection-scoped", async ({ page }) => {
  await installBackend(page, { ...schemaBackend, history_search: () => [hit()] });
  await page.goto("/");

  await page.click("#btn-history");
  await expect(page.locator("#history-dialog")).toBeVisible();
  await expect(page.locator("#hist-this-conn")).not.toBeChecked();
  await expect(page.locator("#hist-this-conn")).toBeDisabled();
});
