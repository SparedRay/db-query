import { expect, test, type Page } from "@playwright/test";
import {
  calls,
  connect,
  editorText,
  installBackend,
  MYSQL_CAPS,
  schemaBackend,
  visit,
} from "./harness";

/**
 * Query history — deferred since Stage 0, and on every backlog since.
 *
 * The rule it inherits from every other generated-SQL path in this app: it
 * **never runs anything**. Choosing a statement puts it in the editor and stops
 * there. Half of what is asserted here is that nothing executed.
 *
 * **A dock since §6.18, and that inverted two tests below.** As a `<dialog>`
 * it had to close itself before handing a statement over, so the tests that
 * once proved it had closed now prove the opposite: the list survives being
 * used, which is the whole reason for the change. Same inversion the
 * performance panel's tests went through in §6.5, for the same reason —
 * Stage 21 §2's rule that a panel whose job is to hand you a statement must
 * not own the screen.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

const NOW = Date.now();

/** Two profiles the rail offers, as `list_profiles` returns them. */
const TWO_SAVED = [
  {
    id: "uat",
    name: "uat",
    colour: "#22c55e",
    host: "uat.example.com",
    port: 3306,
    user: "app",
    database: "appdata",
    allowInvalidCerts: false,
    rememberPassword: true,
  },
  {
    id: "qa",
    name: "qa",
    colour: "#f59e0b",
    host: "qa.example.com",
    port: 3306,
    user: "app",
    database: "appdata",
    allowInvalidCerts: false,
    rememberPassword: true,
  },
];

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
    source: "user",
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

/** Two saved connections, each with its own history, and neither connected. */
async function twoConnections(page: Page) {
  await installBackend(page, {
    ...schemaBackend,
    list_profiles: () => ({ profiles: TWO_SAVED, warning: null }),
    connect_saved: (a) => ({
      id: a.id as string,
      serverVersion: "8.4.0",
      databases: ["appdata"],
      currentDatabase: "appdata",
      capabilities: MYSQL_CAPS,
    }),
    // Keyed off the argument rather than a call counter, so an extra search
    // cannot make a scoping test pass.
    history_search: (a) =>
      a.connectionId === "qa" ? [hit({ sql: "SELECT 'qa'" })] : [hit({ sql: "SELECT 'uat'" })],
  });
  await visit(page);
}

async function openHistory(page: Page, hits: unknown[]) {
  await connect(page, { history_search: () => hits });
  await page.click("#btn-history");
  await expect(page.locator("#hist-pane")).toBeVisible();
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
test("the dock says credentials are never recorded", async ({ page }) => {
  await openHistory(page, [hit()]);
  await expect(page.locator("#hist-note")).toContainText("credential");
});

// ------------------------------------------------------------------ picking

test("choosing a statement puts it in the editor and leaves the list open", async ({
  page,
}) => {
  await openHistory(page, [hit({ sql: "SELECT 42" })]);

  await page.locator(".hist-row").first().click();
  expect(await editorText(page)).toContain("SELECT 42");
  // **The inversion.** This asserted `toBeHidden()` while it was a dialog,
  // because `accept()` called `close()` on the line before `insert()`. The
  // second statement you want is usually two rows from the first one.
  await expect(page.locator("#hist-pane")).toBeVisible();
  await expect(page.locator(".hist-row")).toHaveCount(1);
  await assertNothingRan(page);
});

test("arrow keys move and Enter takes the highlighted one", async ({ page }) => {
  await openHistory(page, [hit({ sql: "SELECT 1" }), hit({ sql: "SELECT 2" })]);

  await page.keyboard.press("ArrowDown");
  await expect(page.locator(".hist-row").nth(1)).toHaveClass(/selected/);
  await page.keyboard.press("Enter");

  expect(await editorText(page)).toContain("SELECT 2");
  await expect(page.locator("#hist-pane")).toBeVisible();
  await assertNothingRan(page);
});

test("the context menu opens a statement in its own tab, and runs nothing", async ({ page }) => {
  await openHistory(page, [hit({ sql: "SELECT 99" })]);
  const before = await page.locator("#script-tabs .stab").count();

  await page.locator(".hist-row").first().click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Open in a new tab")').click();

  await expect(page.locator("#script-tabs .stab")).toHaveCount(before + 1);
  expect(await editorText(page)).toContain("SELECT 99");
  // The other `close()` call that is gone: a new tab is a thing you compare
  // against the row that produced it.
  await expect(page.locator("#hist-pane")).toBeVisible();
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

test("Ctrl+H opens the dock and shuts it again", async ({ page }) => {
  await connect(page, { history_search: () => [hit()] });
  await page.keyboard.press("Control+h");
  await expect(page.locator("#hist-pane")).toBeVisible();
  await expect(page.locator("#histsplit")).toBeVisible();

  // A toggle, as `Ctrl+K` and `Ctrl+Shift+M` are: the dock takes 420px of
  // width while it is open, so the key that opens it gives the width back.
  // (As a dialog this test existed to prove a second press did not throw out
  // of `showModal()`; the `pageerror` hook above still covers that class.)
  await page.keyboard.press("Control+h");
  await expect(page.locator("#hist-pane")).toBeHidden();
  await expect(page.locator("#histsplit")).toBeHidden();

  await page.keyboard.press("Control+h");
  await expect(page.locator("#hist-pane")).toBeVisible();
});

test("the rail button toggles it, and says which state it is in", async ({ page }) => {
  await connect(page, { history_search: () => [hit()] });
  const rail = page.locator("#btn-history");
  await expect(rail).toHaveAttribute("aria-pressed", "false");

  await rail.click();
  await expect(page.locator("#hist-pane")).toBeVisible();
  await expect(rail).toHaveAttribute("aria-pressed", "true");

  await rail.click();
  await expect(page.locator("#hist-pane")).toBeHidden();
  await expect(rail).toHaveAttribute("aria-pressed", "false");
});

/**
 * §6.8's rule, inherited: a dock outlives a connection switch, so a list
 * scoped to "this connection" has to follow it. The dialog decided its scope
 * once, at `open()`, which was sound for something you dismissed before you
 * could ever switch.
 */
test("switching connection re-reads a list scoped to the connection", async ({ page }) => {
  await twoConnections(page);

  await page.locator('.rail-item[title^="uat"]').click();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
  await page.click("#btn-history");
  await expect(page.locator("#hist-pane")).toBeVisible();
  await expect.poll(async () => (await lastSearch(page))?.args.connectionId).toBe("uat");

  await page.locator('.rail-item[title^="qa"]').click();
  await expect(page.locator(".rail-item.live")).toHaveCount(2);

  // No refresh pressed, and the dock stays open: what is in it is now QA's.
  await expect(page.locator("#hist-pane")).toBeVisible();
  await expect.poll(async () => (await lastSearch(page))?.args.connectionId).toBe("qa");
  // Still the scoped list, not silently widened to every server.
  await expect(page.locator("#hist-this-conn")).toBeChecked();
});

test("an unscoped list is not re-read on a switch", async ({ page }) => {
  await twoConnections(page);
  await page.locator('.rail-item[title^="uat"]').click();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
  await page.click("#btn-history");
  await page.uncheck("#hist-this-conn");
  await expect.poll(async () => (await lastSearch(page))?.args.connectionId).toBe(null);
  const before = (await calls(page)).filter((c) => c.cmd === "history_search").length;

  await page.locator('.rail-item[title^="qa"]').click();
  await expect(page.locator(".rail-item.live")).toHaveCount(2);

  // Every connection's statements are the same list whichever one is active,
  // so there is nothing to re-read -- and the box stays unchecked rather than
  // the switch quietly re-scoping a list you asked to see widened.
  await expect(page.locator("#hist-this-conn")).not.toBeChecked();
  expect((await calls(page)).filter((c) => c.cmd === "history_search")).toHaveLength(before);
});

test("a shut dock spends nothing on a switch", async ({ page }) => {
  await twoConnections(page);

  await page.locator('.rail-item[title^="uat"]').click();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
  await page.locator('.rail-item[title^="qa"]').click();
  await expect(page.locator(".rail-item.live")).toHaveCount(2);

  // Opening searches unconditionally, so a shut panel has nothing to keep
  // fresh -- and the sync that notices the switch runs on far more than a
  // switch.
  expect((await calls(page)).map((c) => c.cmd)).not.toContain("history_search");
});

/** Without a connection there is nowhere to put a tab; it must say so. */
test("with no connection, history still opens and is not connection-scoped", async ({ page }) => {
  await installBackend(page, { ...schemaBackend, history_search: () => [hit()] });
  await visit(page);

  await page.click("#btn-history");
  await expect(page.locator("#hist-pane")).toBeVisible();
  await expect(page.locator("#hist-this-conn")).not.toBeChecked();
  await expect(page.locator("#hist-this-conn")).toBeDisabled();
});

// ------------------------------------------------------------------- copying

/**
 * **Copy says what happened.** It used to be `void copyText(sql)`: the promise
 * was dropped, so a copy that failed looked exactly like one that worked —
 * which is the bug class `clipboard.ts` was written to prevent, reintroduced
 * at the call site.
 *
 * The outcome goes in the panel's own standing note, not through `notify`.
 * That is `results.setMessage`, and when this was a modal it painted *behind*
 * it: the person saw nothing until they closed the dialog, and then found
 * their result set replaced by a sentence about the clipboard. As a dock the
 * note is still the right place — it is beside the row that was copied, and
 * the results pane belongs to the statement that ran.
 */
test("copying from history says so, in the panel", async ({ page }) => {
  await openHistory(page, [hit({ sql: "SELECT 99" })]);
  const note = page.locator("#hist-note");
  const standing = await note.textContent();

  await page.locator(".hist-row").first().click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Copy")').click();

  await expect(note).toHaveText("Copied.");
  // And it gives the note back, rather than sitting there saying "Copied"
  // long after it has stopped meaning anything.
  await expect(note).toHaveText(standing ?? "", { timeout: 6000 });
  await assertNothingRan(page);
});

/**
 * The case that was invisible. `navigator.clipboard` needs a secure context
 * and a gesture, and WebKitGTK is the weakest of the three platforms at it —
 * so a refusal is a real outcome, not a hypothetical, and it has to show.
 */
test("a clipboard that refuses is reported, not swallowed", async ({ page }) => {
  await openHistory(page, [hit({ sql: "SELECT 99" })]);
  // Both paths: the modern API and the `execCommand` fallback under it.
  await page.evaluate(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: () => Promise.reject(new Error("denied by the test")),
      },
    });
    document.execCommand = () => false;
  });

  await page.locator(".hist-row").first().click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Copy")').click();

  await expect(page.locator("#hist-note")).toContainText("Could not copy to the clipboard");
  await expect(page.locator("#hist-note")).toHaveClass(/copy-failed/);
});
