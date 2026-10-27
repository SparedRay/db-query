import { expect, test, type Page } from "@playwright/test";
import { MYSQL_CAPS, READ_ONLY_CAPS, calls, connect, schemaBackend } from "./harness";

/**
 * Stage 27: a namespace that holds the whole server.
 *
 * It used to hold one database, flat — `{table: [columns]}` — so every
 * qualified name completed nothing, including a qualifier naming the database
 * already selected. Measured in the real editor before the change, with `poc`
 * selected: `FROM poc.` nothing, `FROM poc.orders.` nothing, and `o.` after
 * `FROM poc.orders o` nothing.
 *
 * What is only answerable here is what the popup actually offers, and what the
 * app asked the backend for to be able to offer it.
 */

const POPUP = ".cm-tooltip-autocomplete";

/** Two databases, so "the other one" is a real place. */
const OTHER = {
  archive_2025: ["shipments", "shipment_lines"],
};

function backend(over: Record<string, unknown> = {}) {
  return {
    ...schemaBackend,
    connect: () => ({
      id: "c1",
      serverVersion: "8.4.0",
      databases: ["poc", "archive_2025"],
      currentDatabase: "poc",
      capabilities: MYSQL_CAPS,
    }),
    // `poc` comes from the shared fixture; the second database is this file's.
    schema_names: (a: Record<string, unknown>) => {
      const db = a.db as string;
      if (db === "archive_2025") {
        return {
          shipments: ["id", "order_id", "carrier", "left_at"],
          shipment_lines: ["id", "shipment_id", "qty"],
        };
      }
      return (schemaBackend.schema_names as (x: unknown) => unknown)(a);
    },
    list_tables: (a: Record<string, unknown>) =>
      (a.db as string) === "archive_2025"
        ? OTHER.archive_2025.map((name) => ({ name, kind: "BASE TABLE" }))
        : (schemaBackend.list_tables as (x: unknown) => unknown)(a),
    ...over,
  };
}

/**
 * Type `text` with the cursor left where `|` was, then ask for completions.
 *
 * Explicit rather than waiting for the popup to decide to appear: what is
 * under test is what the namespace can answer, not when CodeMirror opens.
 */
async function offer(page: Page, text: string): Promise<string[]> {
  const at = text.indexOf("|");
  const typed = text.replace("|", "");
  const cm = page.locator("#editor .cm-content");
  await cm.click();
  await page.keyboard.press("Control+a");
  await page.keyboard.press("Delete");
  await cm.pressSequentially(typed);
  for (let i = typed.length; i > at; i--) await page.keyboard.press("ArrowLeft");
  await page.keyboard.press("Control+Space");
  await page.waitForTimeout(250);
  if (!(await page.locator(POPUP).isVisible())) return [];
  // **The label element, not the row.** A row's text is the label and its
  // detail chip run together with no separator — `carrier` in the table
  // `shipments` reads as `carriershipments` — so reading rows made every
  // "contains this name" assertion unable to match. It passed, which is worse
  // than failing: the test for the fallback's scope proved nothing until this
  // was fixed.
  return (await page.locator(`${POPUP} li .cm-completionLabel`).allInnerTexts()).map((t) =>
    t.trim(),
  );
}

/** Connected on `poc`, with the prefetch given time to reach the second database. */
async function live(page: Page, over: Record<string, unknown> = {}) {
  await connect(page, backend(over));
  await expect
    .poll(async () => (await calls(page)).filter((c) => c.cmd === "schema_names").length)
    .toBeGreaterThan(1);
}

// -------------------------------------------------- the database you are on

/**
 * **The reported bug.** A fully-qualified name, with that database already
 * selected, offered nothing at all.
 */
test("the selected database's own name completes its tables", async ({ page }) => {
  await live(page);
  const offered = await offer(page, "SELECT * FROM poc.|");
  expect(offered).toContain("orders");
  expect(offered).toContain("users");
});

test("a qualified table completes its columns", async ({ page }) => {
  await live(page);
  const offered = await offer(page, "SELECT * FROM poc.orders.|");
  expect(offered).toContain("total");
  expect(offered).toContain("user_id");
});

/**
 * An alias resolves to the **full dotted path** of what it names
 * (`@codemirror/lang-sql`'s `sourceContext`), so a qualified table's alias was
 * the one thing a flat namespace could never answer — and `FROM db.table x` is
 * how most people write a cross-database join.
 */
test("an alias of a qualified table completes its columns", async ({ page }) => {
  await live(page);
  const offered = await offer(page, "SELECT o.| FROM poc.orders o");
  expect(offered).toContain("total");
  expect(offered).not.toContain("orders");
});

/** And the unqualified path, which worked before, still does. */
test("an unqualified name still completes from the selected database", async ({ page }) => {
  await live(page);
  expect(await offer(page, "SELECT o.| FROM orders o")).toContain("total");
  expect(await offer(page, "SELECT * FROM |")).toContain("orders");
});

// ------------------------------------------------------- the other database

test("another database completes its tables and their columns", async ({ page }) => {
  await live(page);

  const tables = await offer(page, "SELECT * FROM archive_2025.|");
  expect(tables).toContain("shipments");
  expect(tables).toContain("shipment_lines");

  const columns = await offer(page, "SELECT * FROM archive_2025.shipments.|");
  expect(columns).toContain("carrier");
  expect(columns).toContain("left_at");
});

test("the database names are offered where a table name goes", async ({ page }) => {
  await live(page);
  const offered = await offer(page, "SELECT * FROM |");
  expect(offered).toContain("archive_2025");
  // And the selected database's tables are still there unqualified.
  expect(offered).toContain("orders");
});

/**
 * **The bare-column fallback stays in the database you are on.** The namespace
 * holds the whole server, and a statement that names no table is almost always
 * about the current database — offering every column on the server would be
 * tens of thousands of options.
 */
test("a statement naming no table does not offer another database's columns", async ({ page }) => {
  await live(page);
  const offered = await offer(page, "SELECT car|");
  // `carrier` lives in archive_2025, which is loaded but is not this database.
  expect(offered).not.toContain("carrier");
  // Where it does belong, it is offered.
  expect(await offer(page, "SELECT * FROM archive_2025.shipments.car|")).toContain("carrier");
});

// ------------------------------------------------------------- the prefetch

/**
 * The prefetch is **one database at a time**: the meta connection is shared
 * with the tree and the linter, so what matters is not what the server costs
 * in total but how long a click can be made to wait — one database's query,
 * never the whole server's.
 */
test("the prefetch describes every database, one call each", async ({ page }) => {
  await live(page);
  const asked = (await calls(page))
    .filter((c) => c.cmd === "schema_names")
    .map((c) => c.args.db as string);
  expect(asked).toContain("poc");
  expect(asked).toContain("archive_2025");
  // The database in front of the user is described first: every unqualified
  // name in the buffer refers to it.
  expect(asked[0]).toBe("poc");
  // And no database is asked for twice.
  expect(new Set(asked).size).toBe(asked.length);
});

/**
 * An engine that cannot describe a database in one query gets **names only**.
 * Prefetching columns there would be a round trip per table — sixty of them
 * took twenty seconds on a server 330ms away.
 */
test("an engine with no bulk query is prefetched by name only", async ({ page }) => {
  await connect(page, {
    ...backend(),
    connect: () => ({
      id: "c1",
      serverVersion: null,
      databases: ["poc", "archive_2025"],
      currentDatabase: "poc",
      capabilities: READ_ONLY_CAPS,
    }),
  });

  await expect
    .poll(async () => (await calls(page)).filter((c) => c.cmd === "list_tables").length)
    .toBeGreaterThan(0);
  const made = await calls(page);
  const listed = made.filter((c) => c.cmd === "list_tables").map((c) => c.args.db as string);
  expect(listed).toContain("archive_2025");
  // The current database still gets the full description; the others do not.
  const described = made.filter((c) => c.cmd === "schema_names").map((c) => c.args.db as string);
  expect(described).not.toContain("archive_2025");

  // Names are enough to complete a table.
  expect(await offer(page, "SELECT * FROM archive_2025.|")).toContain("shipments");
});

/**
 * The catch-up path: a database referred to before the prefetch reached it is
 * fetched on the spot. The keystroke that asked is too early to answer — the
 * one after it has the answer.
 */
test("a database referred to before it is loaded gets fetched", async ({ page }) => {
  // Nothing but the current database is described up front: the second one
  // refuses until it is asked for by name.
  let allow = false;
  await connect(page, {
    ...backend({
      schema_names: (a: Record<string, unknown>) => {
        const db = a.db as string;
        if (db === "archive_2025") {
          if (!allow) throw new Error("not yet");
          return { shipments: ["id", "carrier"] };
        }
        return (schemaBackend.schema_names as (x: unknown) => unknown)(a);
      },
    }),
  });
  await expect
    .poll(async () => (await calls(page)).filter((c) => c.cmd === "schema_names").length)
    .toBeGreaterThan(1);

  expect(await offer(page, "SELECT * FROM archive_2025.|")).toEqual([]);

  allow = true;
  // Typing the qualifier asks for it; the next request has it.
  await offer(page, "SELECT * FROM archive_2025.|");
  await expect.poll(async () => offer(page, "SELECT * FROM archive_2025.|")).toContain("shipments");
});

/**
 * A database with a dot in its name. Legal, and the reason the namespace keys
 * are escaped: `lang-sql` splits an unescaped key into levels, so `sales.eu`
 * would become a database `sales` holding a database `eu` and the real one
 * would be unreachable.
 */
test("a database whose name contains a dot still completes", async ({ page }) => {
  await connect(page, {
    ...backend(),
    connect: () => ({
      id: "c1",
      serverVersion: "8.4.0",
      databases: ["poc", "sales.eu"],
      currentDatabase: "poc",
      capabilities: MYSQL_CAPS,
    }),
    schema_names: (a: Record<string, unknown>) =>
      (a.db as string) === "sales.eu"
        ? { invoices: ["id", "gross_cents"] }
        : (schemaBackend.schema_names as (x: unknown) => unknown)(a),
  });
  await expect
    .poll(async () => (await calls(page)).filter((c) => c.cmd === "schema_names").length)
    .toBeGreaterThan(1);

  expect(await offer(page, "SELECT * FROM `sales.eu`.|")).toContain("invoices");
});

/** A name that is not a database on this connection must not become a query. */
test("a qualifier that names nothing asks for nothing", async ({ page }) => {
  await live(page);
  const before = (await calls(page)).filter((c) => c.cmd === "schema_names").length;

  await offer(page, "SELECT * FROM nonsense_db.|");
  await page.waitForTimeout(200);

  const after = (await calls(page)).filter((c) => c.cmd === "schema_names").length;
  expect(after).toBe(before);
});
