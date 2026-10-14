import { expect, test, type Page } from "@playwright/test";
import { CONN_INFO, READ_ONLY_CAPS, calls, connect } from "./harness";

/**
 * The schema export dialog.
 *
 * What it owns is the **selection**, and the one claim worth pinning is what
 * a tick means: a database ticked whole sends `null` per kind — "everything of
 * that kind, resolved when the export runs" — while a partial tick sends
 * names. That is not an optimisation. The tree lazy-loads, so recording a tick
 * must not fetch every table in a warehouse, and a table created between the
 * tick and the Export belongs to "the whole database" in any reading somebody
 * would expect.
 *
 * The script itself is proved where it can be: `dump.rs`'s unit tests, and a
 * live round trip that loads the file into an empty database.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

const CATALOGUE = {
  tables: [
    { name: "orders", estimatedRows: 2_400_000 },
    { name: "users", estimatedRows: 12 },
  ],
  views: ["user_totals"],
  routines: [{ name: "do_thing", kind: "procedure", returns: null, params: [] }],
  triggers: [
    { name: "t_ins", table: "users", timing: "BEFORE", event: "INSERT", actionOrder: 1 },
  ],
  events: [{ name: "nightly" }],
};

const OUTCOME = {
  files: ["/tmp/poc-2026-10-14.sql"],
  bytesWritten: 4_194_304,
  tables: 2,
  rows: 2_400_012,
  cancelled: false,
  warnings: [] as string[],
};

async function withExport(page: Page, extra: Record<string, unknown> = {}) {
  await connect(page, {
    connect: () => ({ ...CONN_INFO, databases: ["poc", "warehouse"] }),
    export_catalogue: () => CATALOGUE,
    // A destination already chosen, with nothing in the way. The overwrite
    // tests below override it.
    export_destination: () => ({ path: "/tmp/dumps", existing: [] as string[] }),
    export_schema: () => OUTCOME,
    ...extra,
  });
}

/** Open it from the database node's context menu. */
async function openFromTree(page: Page) {
  await page.locator('.node.db:has-text("poc")').click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Export schema")').click();
  await expect(page.locator("#export-schema-dialog")).toBeVisible();
}

const lastExport = async (page: Page) =>
  (await calls(page)).filter((c) => c.cmd === "export_schema").pop()?.args as
    | {
        picks: Array<Record<string, unknown>>;
        options: Record<string, unknown>;
        layout: string;
        target: string;
      }
    | undefined;

// ------------------------------------------------------------ getting there

/**
 * Database nodes had no context menu at all before this — tables, columns and
 * routines all did, so it was the one gap in a pattern.
 */
test("a database offers its export, preselected", async ({ page }) => {
  await withExport(page);
  await openFromTree(page);

  // The database it was opened from is ticked, and opened, so the common case
  // is one right-click and one Export.
  const poc = page.locator('.exp-node:has-text("poc")').first();
  await expect(poc.locator('input[type="checkbox"]')).toBeChecked();
  await expect(page.locator("#exp-summary")).toContainText("1 database");
});

/** And the engine decides whether it is offered at all. */
test("an engine with no CREATE statements is not offered an export", async ({ page }) => {
  await connect(page, {
    connect: () => ({ ...CONN_INFO, capabilities: READ_ONLY_CAPS }),
    export_catalogue: () => CATALOGUE,
  });
  await page.locator('.node.db:has-text("poc")').click({ button: "right" });
  await expect(page.locator('.ctx-menu button:has-text("Export schema")')).toBeDisabled();
});

// ------------------------------------------------------- what a tick means

/**
 * **The claim.** A whole database is `null` per kind, not a list of names —
 * so the backend enumerates inside its snapshot, and a table added since the
 * dialog opened is included.
 */
test("a database ticked whole is sent as everything, not as a list", async ({ page }) => {
  await withExport(page);
  await openFromTree(page);
  await page.click("#exp-ok");

  const args = await lastExport(page);
  expect(args?.picks).toHaveLength(1);
  expect(args?.picks[0]).toMatchObject({
    db: "poc",
    tables: null,
    views: null,
    routines: null,
    triggers: null,
    events: null,
  });
});

/** Untick one table and it becomes a list — of the rest, by name. */
test("unticking one object turns the whole database into names", async ({ page }) => {
  await withExport(page);
  await openFromTree(page);
  // Wait for the catalogue to arrive before touching its children.
  await expect(page.locator('.exp-node:has-text("orders")')).toBeVisible();

  await page
    .locator('.exp-node:has-text("orders")')
    .locator('input[type="checkbox"]')
    .uncheck();
  await page.click("#exp-ok");

  const args = await lastExport(page);
  expect(args?.picks[0]).toMatchObject({
    tables: ["users"],
    views: ["user_totals"],
    routines: ["do_thing"],
    triggers: ["t_ins"],
    events: ["nightly"],
  });
});

/**
 * A database nobody opened is still exportable in full — which is the whole
 * reason "all" is resolved later. Ticking it must not have fetched it.
 */
test("a database can be exported whole without ever being opened", async ({ page }) => {
  await withExport(page);
  await openFromTree(page);

  await page
    .locator('.exp-node:has-text("warehouse")')
    .first()
    .locator('input[type="checkbox"]')
    .check();
  await page.click("#exp-ok");

  const fetched = (await calls(page))
    .filter((c) => c.cmd === "export_catalogue")
    .map((c) => c.args.db);
  expect(fetched).not.toContain("warehouse");

  const args = await lastExport(page);
  expect(args?.picks.map((p) => p.db)).toEqual(["poc", "warehouse"]);
});

// ------------------------------------------------------------- the options

test("the options reach the backend as chosen", async ({ page }) => {
  await withExport(page);
  await openFromTree(page);

  await page.locator('input[name="exp-contents"][value="schemaOnly"]').check();
  await page.locator('input[name="exp-layout"][value="filePerDatabase"]').check();
  await page.uncheck("#exp-drop");
  await page.uncheck("#exp-omit-definer");
  await page.fill("#exp-batch", "2000");
  await page.click("#exp-ok");

  const args = await lastExport(page);
  expect(args?.layout).toBe("filePerDatabase");
  expect(args?.options).toMatchObject({
    contents: "schemaOnly",
    dropExisting: false,
    omitDefiner: false,
    // Left ticked, so it has to still arrive true — a test that only pinned
    // the boxes it changed would pass with the others wired to nothing.
    createDatabase: true,
    batchRows: 2000,
  });
});

/** The dialog opens on the measured default from Settings. */
test("rows per INSERT starts at the setting", async ({ page }) => {
  await withExport(page);
  await openFromTree(page);
  await expect(page.locator("#exp-batch")).toHaveValue("1000");
});

/**
 * The two options that change what the script *is*, said in words. Turning
 * CREATE DATABASE off is how a copy gets restored under another name, and
 * that is not guessable from the label.
 */
test("the CREATE DATABASE note says what each choice means", async ({ page }) => {
  await withExport(page);
  await openFromTree(page);

  await expect(page.locator("#exp-create-db-note")).toContainText("recreates each database");
  await page.uncheck("#exp-create-db");
  await expect(page.locator("#exp-create-db-note")).toContainText(
    "loads into whichever one you point it at",
  );
});

// ------------------------------------------------------------- the summary

test("the summary counts what was chosen, and marks the estimate", async ({ page }) => {
  await withExport(page);
  await openFromTree(page);
  await expect(page.locator('.exp-node:has-text("orders")')).toBeVisible();

  // `~` because it is information_schema's estimate, about 1% out on a
  // million rows and never used for anything but this line.
  await expect(page.locator("#exp-summary")).toContainText("~2,400,012 rows");
  await expect(page.locator("#exp-summary")).toContainText("2 tables");

  // Schema only has no rows to count, so it does not claim any.
  await page.locator('input[name="exp-contents"][value="schemaOnly"]').check();
  await expect(page.locator("#exp-summary")).not.toContainText("rows");
});

test("with nothing selected the export cannot be started", async ({ page }) => {
  await withExport(page);
  await openFromTree(page);
  await page.click("#exp-none");
  await expect(page.locator("#exp-ok")).toBeDisabled();
  await expect(page.locator("#exp-summary")).toHaveText("Nothing selected.");
});

// --------------------------------------------------------- while it runs

/** A dump runs for minutes, so it reports outside the dialog — the app stays
 *  usable, which is the whole reason it is not a bar inside a modal. */
test("the run reports in the job strip, not in a modal", async ({ page }) => {
  let release!: (v: unknown) => void;
  const held = new Promise((r) => {
    release = r;
  });
  await withExport(page, { export_schema: () => held });
  await openFromTree(page);
  await page.click("#exp-ok");

  // The dialog is gone and the strip is up.
  await expect(page.locator("#export-schema-dialog")).toBeHidden();
  await expect(page.locator("#job-strip")).toBeVisible();
  await expect(page.locator("#job-strip")).toContainText("Exporting");
  // And the editor still takes typing while it runs.
  await page.locator(".cm-content").click();
  await page.keyboard.type("SELECT 1");

  release(OUTCOME);
  await expect(page.locator("#job-strip")).toBeHidden();
});

test("Stop asks the backend to stop, by the id the run was started with", async ({ page }) => {
  let release!: (v: unknown) => void;
  const held = new Promise((r) => {
    release = r;
  });
  await withExport(page, { export_schema: () => held });
  await openFromTree(page);
  await page.click("#exp-ok");

  await page.locator('#job-strip button:has-text("Stop")').click();
  const started = (await calls(page)).find((c) => c.cmd === "export_schema")?.args.requestId;
  const stopped = (await calls(page)).find((c) => c.cmd === "export_schema_cancel")?.args.requestId;
  expect(stopped).toBe(started);

  release({ ...OUTCOME, cancelled: true, rows: 1200, warnings: ["Cancelled after 1200 rows."] });
  await expect(page.locator("#grid")).toContainText("Export stopped after 1,200 rows");
});

test("the report says what was written and where", async ({ page }) => {
  await withExport(page);
  await openFromTree(page);
  await page.click("#exp-ok");

  await expect(page.locator("#grid")).toContainText("Exported 2 tables");
  await expect(page.locator("#grid")).toContainText("4.0 MB in 1 file");
  await expect(page.locator("#grid")).toContainText("/tmp/poc-2026-10-14.sql");
});

/** Dismissing the file dialog is not an outcome to report. */
test("cancelling the file dialog reports nothing", async ({ page }) => {
  await withExport(page, { export_destination: () => null });
  await openFromTree(page);
  await page.click("#exp-ok");

  await expect(page.locator("#job-strip")).toBeHidden();
  await expect(page.locator("#grid")).not.toContainText("Exported");
  // And nothing was read: the destination comes before any work.
  expect((await calls(page)).some((c) => c.cmd === "export_schema")).toBe(false);
});

/**
 * **Nothing claims to be happening until something is.**
 *
 * The destination used to be chosen inside `export_schema`, so the strip went
 * up first and sat over an unanswered file dialog saying "Exporting schema",
 * with a Stop that stopped nothing.
 */
test("the strip stays down while the destination is being chosen", async ({ page }) => {
  let release!: (v: unknown) => void;
  const held = new Promise((r) => {
    release = r;
  });
  await withExport(page, { export_destination: () => held });
  await openFromTree(page);
  await page.click("#exp-ok");

  // The dialog has closed, the picker is open, and the strip says nothing.
  await expect(page.locator("#export-schema-dialog")).toBeHidden();
  await expect(page.locator("#job-strip")).toBeHidden();

  release({ path: "/tmp/dumps", existing: [] });
  await expect(page.locator("#grid")).toContainText("Exported 2 tables");
});

// ------------------------------------------------- landing on existing files

/**
 * **The save dialog asks about one file; a folder picker cannot.**
 *
 * It does not know what we are about to put in the folder, so the
 * per-database layout asks here instead (§5). The confirmation **names** the
 * files rather than counting them, for the same reason the Flyway one names
 * versions: "3 files will be overwritten" in a folder of forty is not
 * something anybody can check.
 */
async function perDatabase(page: Page) {
  await openFromTree(page);
  await page.locator('#export-schema-dialog input[value="filePerDatabase"]').check();
}

test("files already in the folder are named, and Cancel writes nothing", async ({ page }) => {
  await withExport(page, {
    export_destination: () => ({ path: "/tmp/dumps", existing: ["poc.sql", "warehouse.sql"] }),
  });
  await perDatabase(page);
  await page.click("#exp-ok");

  const ask = page.locator("dialog.ask");
  await expect(ask).toContainText("Overwrite 2 files?");
  await expect(ask).toContainText("poc.sql");
  await expect(ask).toContainText("warehouse.sql");
  await expect(ask).toContainText("/tmp/dumps");

  await ask.locator('button:has-text("Cancel")').click();
  expect((await calls(page)).some((c) => c.cmd === "export_schema")).toBe(false);
  // No strip either: the export never started.
  await expect(page.locator("#job-strip")).toBeHidden();
});

test("Overwrite goes ahead with the destination already chosen", async ({ page }) => {
  await withExport(page, {
    export_destination: () => ({ path: "/tmp/dumps", existing: ["poc.sql"] }),
  });
  await perDatabase(page);
  await page.click("#exp-ok");

  await page.locator('dialog.ask button:has-text("Overwrite")').click();
  const sent = await lastExport(page);
  expect(sent?.target).toBe("/tmp/dumps");
  expect(sent?.layout).toBe("filePerDatabase");
  // Asked once. The second call would re-open the folder picker.
  expect((await calls(page)).filter((c) => c.cmd === "export_destination")).toHaveLength(1);
});

/** An empty folder is not a question. */
test("an empty folder asks nothing", async ({ page }) => {
  await withExport(page);
  await perDatabase(page);
  await page.click("#exp-ok");

  await expect(page.locator("dialog.ask")).toHaveCount(0);
  await expect(page.locator("#grid")).toContainText("Exported 2 tables");
});
