import { expect, test } from "@playwright/test";
import { calls, commandNames, connect, editorText, openDatabase, openGroup } from "./harness";

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

/**
 * The rule the whole of Stage 3 was built on: **nothing generated is ever
 * executed.** Half of what these menus produce is `DROP`, so this is asserted
 * after every single action rather than once.
 */
async function assertNothingRan(page: import("@playwright/test").Page) {
  expect(await commandNames(page)).not.toContain("run_script");
}

// --------------------------------------------------------------------- E3

test("E3 — tables, views, procedures and functions are separate groups", async ({ page }) => {
  await connect(page);
  await openDatabase(page);

  // Counted, so the separation is legible without expanding anything.
  await expect(page.locator(".node.group")).toHaveText([
    /Tables \(3\)/,
    /Views \(1\)/,
    /Procedures \(2\)/,
    /Functions \(1\)/,
  ]);
});

test("E3 — tables open by default; the common case costs no extra click", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await expect(page.locator('.node.table:has-text("users")')).toBeVisible();
  // A view lives in its own group and is not shown until that group is opened.
  await expect(page.locator('.node.table:has-text("user_totals")')).toBeHidden();
});

test("E3 — a function shows its return type, a procedure does not", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await openGroup(page, "Functions");
  await expect(page.locator('.node.routine:has-text("order_count") .meta')).toHaveText("(uid) → int");

  await openGroup(page, "Procedures");
  await expect(page.locator('.node.routine:has-text("top_spenders") .meta')).toHaveText(
    "(min_total, max_rows)",
  );
  // A no-argument routine must render empty parentheses, not a stray comma.
  await expect(page.locator('.node.routine:has-text("ping_poc") .meta')).toHaveText("()");
});

test("an empty group is not rendered at all", async ({ page }) => {
  await connect(page, { list_routines: () => [] });
  await openDatabase(page);
  await expect(page.locator('.node.group:has-text("Procedures")')).toHaveCount(0);
  await expect(page.locator('.node.group:has-text("Functions")')).toHaveCount(0);
});

/** An old server or a restricted account must not take the tables down with it. */
test("a server that will not list routines still shows its tables", async ({ page }) => {
  await connect(page, {
    list_routines: () => {
      throw new Error("command list_routines not allowed");
    },
  });
  await openDatabase(page);
  await expect(page.locator('.node.group:has-text("Tables")')).toBeVisible();
  await expect(page.locator('.node.table:has-text("users")')).toBeVisible();
});

// --------------------------------------------------------------------- E1

test("E1 — double-clicking a table opens a new tab with SELECT, and runs nothing", async ({ page }) => {
  await connect(page);
  await openDatabase(page);

  const before = await page.locator("#script-tabs .stab").count();
  await page.locator('.node.table:has-text("users")').dblclick();

  await expect(page.locator("#script-tabs .stab")).toHaveCount(before + 1);
  await expect(page.locator("#script-tabs .stab.active")).toHaveText(/users/);
  expect(await editorText(page)).toContain("SELECT *");
  expect(await editorText(page)).toContain("`poc`.`users`");

  // The browse limit is the backend's number, not one written down twice.
  const gen = (await calls(page)).find((c) => c.cmd === "generate_select");
  expect(gen?.args).toMatchObject({ db: "poc", table: "users", limit: 1000 });
  await assertNothingRan(page);
});

test("E1 — double-click does not also expand the table", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await page.locator('.node.table:has-text("users")').dblclick();
  // The deferred single-click must have been cancelled: no columns appeared.
  await expect(page.locator('.node.column:has-text("email")')).toHaveCount(0);
  expect(await commandNames(page)).not.toContain("list_columns");
});

test("a single click still expands the table's columns", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await page.locator('.node.table:has-text("users")').click();
  await expect(page.locator('.node.column:has-text("email")')).toBeVisible();
  // The primary key is marked, which is the point of showing keys at all.
  await expect(page.locator('.node.column:has-text("id") .icon')).toHaveText("🔑");
});

// --------------------------------------------------------------------- E2

test("E2 — a table's menu generates a DROP and executes nothing", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await page.locator('.node.table:has-text("users")').click({ button: "right" });

  await expect(page.locator(".ctx-heading")).toHaveText("Table users");
  await page.locator('.ctx-menu button:has-text("Drop table")').click();

  const drop = (await calls(page)).find((c) => c.cmd === "generate_drop");
  expect(drop?.args.target).toEqual({ type: "table", db: "poc", name: "users" });
  expect(await editorText(page)).toContain("DROP");
  await assertNothingRan(page);
});

test("E2 — a view drops as a VIEW, not as a table", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await openGroup(page, "Views");
  await page.locator('.node.table:has-text("user_totals")').click({ button: "right" });

  await expect(page.locator(".ctx-heading")).toHaveText("View user_totals");
  await page.locator('.ctx-menu button:has-text("Drop view")').click();
  const drop = (await calls(page)).find((c) => c.cmd === "generate_drop");
  expect(drop?.args.target).toEqual({ type: "view", db: "poc", name: "user_totals" });
  await assertNothingRan(page);
});

test("E2 — a column's menu drops the column, naming its table", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await page.locator('.node.table:has-text("users")').click();
  await page.locator('.node.column:has-text("email")').click({ button: "right" });

  await expect(page.locator(".ctx-heading")).toHaveText("Column users.email");
  await page.locator('.ctx-menu button:has-text("Drop column")').click();
  const drop = (await calls(page)).find((c) => c.cmd === "generate_drop");
  expect(drop?.args.target).toEqual({
    type: "column",
    db: "poc",
    table: "users",
    name: "email",
  });
  await assertNothingRan(page);
});

test("E2 — Select first N rows from the menu matches the double-click", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await page.locator('.node.table:has-text("orders")').click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Select first")').click();

  const gen = (await calls(page)).find((c) => c.cmd === "generate_select");
  expect(gen?.args).toMatchObject({ db: "poc", table: "orders", limit: 1000 });
  await assertNothingRan(page);
});

test("Insert name at cursor puts the name in the editor, not a new tab", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  const before = await page.locator("#script-tabs .stab").count();

  await page.locator('.node.table:has-text("orders")').click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Insert name")').click();

  await expect(page.locator("#script-tabs .stab")).toHaveCount(before);
  expect(await editorText(page)).toContain("orders");
});

// --------------------------------------------------------------------- E4

test("E4 — Examine generates the DROP + DELIMITER re-creation script", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await openGroup(page, "Procedures");
  await page.locator('.node.routine:has-text("top_spenders")').click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Examine")').click();

  const ddl = (await calls(page)).find((c) => c.cmd === "routine_ddl");
  expect(ddl?.args).toMatchObject({ db: "poc", name: "top_spenders", kind: "procedure" });

  const text = await editorText(page);
  // Order is the substance: creating before dropping deletes what was made.
  expect(text.indexOf("DROP PROCEDURE")).toBeLessThan(text.indexOf("CREATE"));
  expect(text).toContain("DELIMITER $$");
  await assertNothingRan(page);
});

test("E4 — a function is examined as a FUNCTION", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await openGroup(page, "Functions");
  await page.locator('.node.routine:has-text("order_count")').click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Examine")').click();

  const ddl = (await calls(page)).find((c) => c.cmd === "routine_ddl");
  expect(ddl?.args.kind).toBe("function");
  await assertNothingRan(page);
});

/**
 * Views had no way to show their own query at all — the one thing a view *is*
 * lived only on the server. The definition entry now sits on tables too: it is
 * the same `SHOW CREATE TABLE` either way, and MySQL simply answers a view with
 * a "Create View" column instead.
 */
test("E4 — a view's definition is examinable, like a routine's", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await openGroup(page, "Views");
  await page.locator('.node.table:has-text("user_totals")').click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Examine")').click();

  const ddl = (await calls(page)).find((c) => c.cmd === "table_ddl");
  expect(ddl?.args).toMatchObject({ db: "poc", table: "user_totals" });
  expect(await editorText(page)).toContain("user_totals");
  await assertNothingRan(page);
});

test("E4 — a table's definition is examinable too", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await page.locator('.node.table:has-text("orders")').click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Examine")').click();

  const ddl = (await calls(page)).find((c) => c.cmd === "table_ddl");
  expect(ddl?.args).toMatchObject({ db: "poc", table: "orders" });
  await assertNothingRan(page);
});

/**
 * Routines append to the tab in progress rather than opening a new one —
 * calling a procedure is usually a step inside a script. Recorded in Stage 3 as
 * a deliberate inconsistency with tables, so it is pinned here.
 */
test("double-clicking a routine appends to the current tab", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await page.locator('.node.table:has-text("users")').dblclick();
  const tabs = await page.locator("#script-tabs .stab").count();

  await openGroup(page, "Procedures");
  await page.locator('.node.routine:has-text("ping_poc")').dblclick();

  await expect(page.locator("#script-tabs .stab")).toHaveCount(tabs);
  const text = await editorText(page);
  expect(text).toContain("SELECT *");  // the browse query is still there
  expect(text).toContain("CALL");      // and the call was added after it
  await assertNothingRan(page);
});

test("a routine's menu drops it with the right keyword", async ({ page }) => {
  await connect(page);
  await openDatabase(page);
  await openGroup(page, "Functions");
  await page.locator('.node.routine:has-text("order_count")').click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Drop function")').click();

  const drop = (await calls(page)).find((c) => c.cmd === "generate_drop");
  expect(drop?.args.target).toEqual({
    type: "routine",
    db: "poc",
    name: "order_count",
    kind: "function",
  });
  await assertNothingRan(page);
});
