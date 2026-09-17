import { expect, test, type Page } from "@playwright/test";
import {
  MYSQL_CAPS, calls, commandNames, connect, editorText, schemaBackend,
} from "./harness";

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

// ----------------------------------------------- what there is to complete

/**
 * **The schema, without having to go and find it.**
 *
 * Autocomplete used to be fed only by clicks in the tree: table names when a
 * database was expanded, column names when a *table* was. So a freshly
 * connected editor offered nothing but the dialect's own keywords — pages of
 * `HOUR_MICROSECOND` and `IGNORE_SERVER_IDS` — and after some browsing it
 * offered whichever subset had been opened, which is worse: a list that is
 * silently partial reads as a complete one.
 */

/** Type into an empty buffer once the schema has been loaded, and open the popup. */
async function offeringAgainst(page: Page, prefix: string) {
  await expect.poll(async () => commandNames(page)).toContain("schema_names");
  const cm = page.locator("#editor .cm-content");
  await cm.click();
  await page.keyboard.press("Control+a");
  await page.keyboard.press("Delete");
  await cm.pressSequentially(prefix);
  await page.keyboard.press("Control+Space");
  await expect(page.locator(POPUP)).toBeVisible();
}

/** What the popup is showing, most relevant first. */
async function offered(page: Page): Promise<string[]> {
  return page.locator(`${POPUP} li`).allInnerTexts();
}

test("tables complete without opening anything in the tree", async ({ page }) => {
  await connect(page, schemaBackend);
  await offeringAgainst(page, "SELECT * FROM ord");

  // Ahead of every keyword that also matches, because it is what was asked for.
  expect((await offered(page))[0]).toContain("orders");
});

/** And the app asked for the database it is actually connected to. */
test("the schema is fetched for the connection's own database", async ({ page }) => {
  await connect(page, schemaBackend);
  await expect.poll(async () => commandNames(page)).toContain("schema_names");
  const asked = (await calls(page)).filter((c) => c.cmd === "schema_names");
  expect(asked[0].args.db).toBe("poc");
  // Nothing was clicked: `connect` has always reported `currentDatabase`, and
  // this is the first thing to read it.
  expect(await page.locator(".node.group").count()).toBe(0);
});

/**
 * **The gap `@codemirror/lang-sql` leaves.** Read from its source on
 * 2026-09-17 (`completeFromSchema`): it completes columns after `table.` or an
 * alias, and bare columns only for one configured `defaultTable`. So the
 * position people actually type a column in — a `WHERE` clause — offered
 * table names and keywords and never the columns of the table being selected
 * from.
 */
test("columns complete where columns are typed", async ({ page }) => {
  await connect(page, schemaBackend);
  await offeringAgainst(page, "SELECT * FROM orders WHERE us");

  const list = await offered(page);
  // `user_id` belongs to `orders`; `users` and `user_totals` are tables that
  // merely match the same prefix.
  expect(list[0]).toContain("user_id");
  // And it says which table it came from, because two tables in a join can
  // both have an `id`.
  expect(list[0]).toContain("orders");
});

test("a qualified, back-quoted, aliased table still resolves", async ({ page }) => {
  await connect(page, schemaBackend);
  await offeringAgainst(
    page,
    "SELECT * FROM `poc`.`orders` o JOIN users u ON u.id = o.user_id WHERE dis",
  );

  expect((await offered(page))[0]).toContain("display_name");
});

/**
 * Scoped to the statement under the cursor. A script that touches twenty
 * tables must not offer every column in the database on every word.
 */
test("columns come from this statement, not the one before it", async ({ page }) => {
  await connect(page, schemaBackend);
  await offeringAgainst(page, "SELECT * FROM orders; SELECT * FROM users WHERE em");

  const list = await offered(page);
  expect(list[0]).toContain("email");
  // `total` is a column of `orders`, which this statement does not mention.
  expect(list.join(" ")).not.toContain("total");
});

/** Dotted completion is lang-sql's and must stay exactly as it was. */
test("a dotted table still completes only that table's columns", async ({ page }) => {
  await connect(page, schemaBackend);
  await offeringAgainst(page, "SELECT orders.");

  expect(await offered(page)).toEqual(["id", "total", "user_id"]);
});

/**
 * **A stale map is worse than an empty one.** One map serves the whole app and
 * nothing used to clear it, so connecting to staging after production left
 * production's tables completing against a database that had never heard of
 * them — a confidently wrong answer rather than a missing one.
 */
test("changing database replaces what completes, rather than adding to it", async ({ page }) => {
  await connect(page, {
    connect: () => ({
      id: "c1",
      serverVersion: "8.4.0",
      databases: ["poc", "warehouse"],
      currentDatabase: "poc",
      capabilities: MYSQL_CAPS,
    }),
    schema_names: (a: Record<string, unknown>) =>
      a.db === "warehouse" ? { shipments: ["shipped_on"] } : { orders: ["total"] },
    list_tables: (a: Record<string, unknown>) =>
      a.db === "warehouse"
        ? [{ name: "shipments", kind: "BASE TABLE" }]
        : [{ name: "orders", kind: "BASE TABLE" }],
  });

  await offeringAgainst(page, "SELECT * FROM ord");
  expect((await offered(page))[0]).toContain("orders");

  await page.keyboard.press("Escape");
  await page.locator('.node.db:has-text("warehouse")').click();
  await expect
    .poll(async () => (await calls(page)).filter((c) => c.cmd === "schema_names").length)
    .toBeGreaterThan(1);

  await offeringAgainst(page, "SELECT * FROM ship");
  expect((await offered(page))[0]).toContain("shipments");

  // And `orders` is gone: it is not in this database.
  await page.keyboard.press("Escape");
  await offeringAgainst(page, "SELECT * FROM orders WHERE tot");
  expect((await offered(page)).join(" ")).not.toContain("total");
});

/** Failing to describe the schema must not break the editor. */
test("an unreadable schema leaves completion working on keywords", async ({ page }) => {
  await connect(page, {
    schema_names: () => {
      throw new Error("Access denied for user");
    },
  });
  await offeringAgainst(page, "sel");
  expect((await offered(page)).join(" ")).toContain("SELECT");
});

// ------------------------------------------- what the second pass found

/**
 * **SQL is typed in the wrong order for this.** `SELECT` comes before `FROM`,
 * so while writing a fresh query there is no table in scope at all — and the
 * first version of this offered nothing there. `SELECT e` returned `EACH`,
 * `EDIT`, `EGO`, `ELSE`, which is exactly the "only MySQL things" complaint.
 *
 * With no table named, every column in the database is offered instead, once
 * something has been typed.
 */
test("columns are offered before the table has been named", async ({ page }) => {
  await connect(page, schemaBackend);
  await offeringAgainst(page, "SELECT e");

  expect((await offered(page))[0]).toContain("email");
});

/**
 * **`FROM Users` found nothing when the server had said `users`.**
 *
 * MySQL folds table names to lower case on Windows and macOS, and people type
 * them however they like everywhere. A case-sensitive lookup made the feature
 * look broken for anyone who capitalises.
 */
test("a table typed in another case still resolves", async ({ page }) => {
  await connect(page, schemaBackend);
  await offeringAgainst(page, "SELECT * FROM Users WHERE em");

  expect((await offered(page))[0]).toContain("email");
});

/**
 * Right after `FROM`, the thing being typed is a table. Offering columns there
 * — boosted above the tables, as they are everywhere else — would put
 * `user_id` ahead of `users` in `FROM us`.
 */
test("no columns are offered where a table name belongs", async ({ page }) => {
  await connect(page, schemaBackend);
  await offeringAgainst(page, "SELECT * FROM us");

  const list = await offered(page);
  // Tables lead — which of the two matching ones comes first is lang-sql's
  // own ordering and not this test's business.
  expect(["users", "user_totals"]).toContain(list[0]);
  // And the column is not there at all, boosted or otherwise.
  expect(list.join(" ")).not.toContain("user_id");
});

/**
 * A name in several tables appears once. Naming whichever table came first
 * — "id — big" — would be a confident answer to a question nobody asked.
 */
test("a column shared by several tables is listed once, and says so", async ({ page }) => {
  await connect(page, schemaBackend);
  await offeringAgainst(page, "SELECT i");

  const ids = (await offered(page)).filter((l) => l.startsWith("id"));
  expect(ids).toHaveLength(1);
  // `id` is in both `users` and `orders`.
  expect(ids[0]).toContain("2 tables");
});

/**
 * And with a table in scope the columns lead, because `lang-sql` ranks its
 * keywords at -1 and nothing ranked the schema above them.
 */
test("columns outrank keywords and tables once a table is in scope", async ({ page }) => {
  await connect(page, schemaBackend);
  await offeringAgainst(page, "SELECT * FROM users WHERE ");

  const list = await offered(page);
  expect(list.slice(0, 3).join(" ")).toContain("email");
  expect(list.slice(0, 3).join(" ")).toContain("display_name");
});