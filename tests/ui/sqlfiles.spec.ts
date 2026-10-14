import { expect, test, type Page } from "@playwright/test";
import {
  CONN_INFO,
  READ_ONLY_CAPS,
  calls,
  connect,
  installBackend,
  rowsResult,
  visit,
} from "./harness";

/**
 * Running a folder of SQL files.
 *
 * Four of §3's claims had been asserted nowhere before, and each is a thing the
 * first draft of the plan got wrong by pointing at Stage 22 instead of
 * designing it:
 *
 *   * the **report survives** a tab switch and a query — the schema export's
 *     does not, because it goes into `#grid` and `onActivate` repaints that;
 *   * the report lists **not run** files after a stop, which is the row a
 *     report by omission drops;
 *   * a second job finds Run **disabled with the reason on it**, rather than a
 *     dialog that opens and then refuses;
 *   * the rail button **opens with no connection** and says so, rather than
 *     being disabled (Stage 20 §11.1).
 *
 * The run itself is proved where it can be: `sqlfilesrun`'s unit tests and five
 * live tests against `dev/scripts`.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

const SCAN = {
  files: [
    {
      path: "01-setup.sql",
      verdict: { type: "ready" },
      statements: [
        {
          start: 0,
          end: 30,
          line: 2,
          kind: "other",
          returnsRows: false,
          writes: true,
          preview: "DROP TABLE IF EXISTS batch_probe",
          output: null,
        },
      ],
    },
    {
      path: "02-orders.sql",
      verdict: { type: "ready" },
      statements: [
        {
          start: 0,
          end: 40,
          line: 2,
          kind: "select",
          returnsRows: true,
          writes: false,
          preview: "SELECT id, label FROM batch_probe",
          output: "02-orders.csv",
        },
      ],
    },
    { path: "04-proc.sql", verdict: { type: "cannotSplit" }, statements: [] },
    { path: "05-notes.sql", verdict: { type: "nothingToRun" }, statements: [] },
  ],
  runnableFiles: 2,
  statements: 2,
  outputs: 1,
  writes: 1,
};

const OUTCOME = {
  files: [
    {
      path: "01-setup.sql",
      database: "poc",
      status: { type: "ran" },
      csvs: [],
      statementsRun: 1,
      rowsAffected: 3,
    },
    {
      path: "02-orders.sql",
      database: "poc",
      status: { type: "ran" },
      csvs: [{ name: "02-orders.csv", rows: 1284, bytes: 40_000, partial: false }],
      statementsRun: 1,
      rowsAffected: 0,
    },
    {
      path: "04-proc.sql",
      database: "poc",
      status: { type: "skipped", reason: "DELIMITER block" },
      csvs: [],
      statementsRun: 0,
      rowsAffected: 0,
    },
  ],
  dest: "/home/u/out",
  csvs: 1,
  rows: 1284,
  bytes: 40_000,
  cancelled: false,
  stoppedAt: null as string | null,
};

async function withFiles(page: Page, extra: Record<string, unknown> = {}) {
  await connect(page, {
    connect: () => ({ ...CONN_INFO, databases: ["poc", "warehouse"] }),
    // Two different folders, in the order the dialog asks for them — the
    // source first, then the destination. One stub returning one path would
    // have the dialog writing CSVs among the scripts, which is a thing it
    // warns about and not what these tests are exercising.
    sqlfiles_pick_folder: (() => {
      let n = 0;
      return () => (n++ === 0 ? "/home/u/scripts" : "/home/u/out");
    })(),
    sqlfiles_scan: () => SCAN,
    sqlfiles_run: () => OUTCOME,
    ...extra,
  });
}

/** Open it, choose both folders, so Run is live. */
async function ready(page: Page) {
  await page.click("#btn-sqlfiles");
  await expect(page.locator("#sqlfiles-dialog")).toBeVisible();
  await page.click("#sf-pick-source");
  await page.click("#sf-pick-dest");
  await expect(page.locator("#sf-ok")).toBeEnabled();
}

// ------------------------------------------------------------ getting there

test("the rail button opens it, and Ctrl+Shift+B does too", async ({ page }) => {
  await withFiles(page);
  await page.click("#btn-sqlfiles");
  await expect(page.locator("#sqlfiles-dialog")).toBeVisible();
  await page.click("#sf-cancel");
  await expect(page.locator("#sqlfiles-dialog")).toBeHidden();

  await page.keyboard.press("Control+Shift+B");
  await expect(page.locator("#sqlfiles-dialog")).toBeVisible();
});

/**
 * **`Ctrl+B` still toggles the schema.** It has since Stage 12, so Stage 23
 * took the modifier rather than the letter — and the branch matches both cases
 * of "b", so without a `shiftKey` test it would have answered both.
 */
test("Ctrl+B still belongs to the schema pane", async ({ page }) => {
  await withFiles(page);
  await expect(page.locator("#sidebar")).toBeVisible();
  await page.keyboard.press("Control+b");
  await expect(page.locator("#sidebar")).toBeHidden();
  await expect(page.locator("#sqlfiles-dialog")).toBeHidden();
});

/**
 * **Never disabled, however bad the news.** Stage 20 §11.1 settled this for
 * the assistant: opening is how you read *why* it is not ready, so taking the
 * click away hides the explanation behind the thing needing explanation.
 */
test("with no connection the dialog opens and says so", async ({ page }) => {
  // Through the harness rather than `goto` directly: the stubbed `invoke` is
  // installed by `installBackend`, and without it the app cannot boot at all.
  await installBackend(page);
  await visit(page);
  await page.click("#btn-sqlfiles");
  await expect(page.locator("#sqlfiles-dialog")).toBeVisible();
  await expect(page.locator("#sf-error")).toContainText("No connection is open");
  await expect(page.locator("#sf-ok")).toBeDisabled();
});

/** And an engine with no statement splitter is offered nothing to run. */
test("an engine that cannot run scripts says which", async ({ page }) => {
  await connect(page, { connect: () => ({ ...CONN_INFO, capabilities: READ_ONLY_CAPS }) });
  await page.click("#btn-sqlfiles");
  await expect(page.locator("#sf-error")).toContainText("can run a folder");
  await expect(page.locator("#sf-ok")).toBeDisabled();
});

// --------------------------------------------------------------- the preview

test("the preview lists every file, including the ones that will not run", async ({ page }) => {
  await withFiles(page);
  await page.click("#btn-sqlfiles");
  await page.click("#sf-pick-source");

  const rows = page.locator("#sf-preview .sf-file");
  await expect(rows).toHaveCount(4);
  // **A file that will not run is a line, not an omission.** Somebody put it in
  // the folder; a preview silent about it leaves them wondering.
  await expect(rows.filter({ hasText: "04-proc.sql" })).toContainText("DELIMITER");
  await expect(rows.filter({ hasText: "05-notes.sql" })).toContainText("Nothing to run");
  // And the one that writes is marked as such.
  await expect(page.locator('.sf-file[data-path="01-setup.sql"]')).toHaveClass(/writes/);

  await expect(page.locator("#sf-summary")).toContainText("2 of 4 files");
  await expect(page.locator("#sf-summary")).toContainText("1 statement will write");
});

/**
 * **Reported in use: "it just does nothing when clicking on the button."**
 *
 * A folder with no `.sql` files in it disabled Run and said so only in the grey
 * hint line, because that branch returned early and never reached the `say()`
 * its sibling case used — so a folder of unrunnable scripts shouted while an
 * empty folder whispered. The press then did nothing, with nothing to read.
 *
 * Every reason Run is unavailable now comes from one place, and appears both in
 * the error line and on the button, so a new reason cannot be added that
 * forgets to explain itself.
 */
test("an empty folder says so, in the error line and on the button", async ({ page }) => {
  await withFiles(page, {
    sqlfiles_scan: () => ({ files: [], runnableFiles: 0, statements: 0, outputs: 0, writes: 0 }),
  });
  await page.click("#btn-sqlfiles");
  await page.click("#sf-pick-source");

  await expect(page.locator("#sf-error")).toBeVisible();
  await expect(page.locator("#sf-error")).toContainText("No .sql files");
  await expect(page.locator("#sf-ok")).toBeDisabled();
  // Hovering a disabled button has to answer too.
  await expect(page.locator("#sf-ok")).toHaveAttribute("title", /No \.sql files/);
});

/** And a folder whose files all turn out to be unrunnable reads the same way. */
test("a folder of nothing-to-run says so too", async ({ page }) => {
  await withFiles(page, {
    sqlfiles_scan: () => ({
      files: [{ path: "notes.sql", verdict: { type: "nothingToRun" }, statements: [] }],
      runnableFiles: 0,
      statements: 0,
      outputs: 0,
      writes: 0,
    }),
  });
  await page.click("#btn-sqlfiles");
  await page.click("#sf-pick-source");

  await expect(page.locator("#sf-error")).toContainText("Nothing in that folder can be run");
  await expect(page.locator("#sf-ok")).toBeDisabled();
  await expect(page.locator("#sf-ok")).toHaveAttribute("title", /Nothing in that folder/);
});

test("Run waits for a destination", async ({ page }) => {
  await withFiles(page);
  await page.click("#btn-sqlfiles");
  await page.click("#sf-pick-source");
  await expect(page.locator("#sf-ok")).toBeDisabled();
  await expect(page.locator("#sf-summary")).toContainText("choose a destination");
  await page.click("#sf-pick-dest");
  await expect(page.locator("#sf-ok")).toBeEnabled();
});

/**
 * **Both folders can be the same, and the dialog says what that means.** §3.2
 * chose two so inputs never mix with outputs; somebody who undoes that should
 * hear about it now rather than find CSVs among their scripts later. Not
 * refused — it is their folder.
 */
test("pointing the destination at the source is allowed and noted", async ({ page }) => {
  await withFiles(page, { sqlfiles_pick_folder: () => "/home/u/scripts" });
  await page.click("#btn-sqlfiles");
  await page.click("#sf-pick-source");
  await page.click("#sf-pick-dest");

  await expect(page.locator("#sf-error")).toContainText("among the scripts");
  // Allowed, not blocked.
  await expect(page.locator("#sf-ok")).toBeEnabled();
});

// ------------------------------------------------------- the write confirmation

/**
 * **It names the statements, not the count.** "1 statement will write" is not
 * checkable; `01-setup.sql:2 DROP TABLE …` is. Same rule as the Flyway
 * confirmation and Stage 22's overwrite one.
 */
test("writes are named before they run, and Cancel runs nothing", async ({ page }) => {
  await withFiles(page);
  await ready(page);
  await page.click("#sf-ok");

  const ask = page.locator("dialog.ask");
  await expect(ask).toContainText("Run 1 statement that writes?");
  await expect(ask).toContainText("01-setup.sql:2");
  await expect(ask).toContainText("DROP TABLE IF EXISTS batch_probe");

  await ask.locator('button:has-text("Cancel")').click();
  expect((await calls(page)).some((c) => c.cmd === "sqlfiles_run")).toBe(false);
  await expect(page.locator("#job-strip")).toBeHidden();
});

test("the plan the dialog showed is the plan that is sent", async ({ page }) => {
  await withFiles(page);
  await ready(page);
  await page.click("#sf-ok");
  await page.locator('dialog.ask button:has-text("Run")').click();

  const sent = (await calls(page)).find((c) => c.cmd === "sqlfiles_run")?.args as
    | { scan: typeof SCAN; source: string; dest: string; database: string | null }
    | undefined;
  // Byte for byte the same object, which is what makes the confirmation
  // honest: it cannot have named statements from a different plan.
  expect(sent?.scan).toEqual(SCAN);
  expect(sent?.source).toBe("/home/u/scripts");
  expect(sent?.dest).toBe("/home/u/out");
  expect(sent?.database).toBe("poc");
});

/** A folder of reads asks nothing — a confirmation about nothing teaches
 *  people to skim the ones that matter. */
test("a folder with no writes is not confirmed", async ({ page }) => {
  const reads = { ...SCAN, writes: 0, files: SCAN.files.slice(1) };
  await withFiles(page, { sqlfiles_scan: () => reads });
  await ready(page);
  await page.click("#sf-ok");

  await expect(page.locator("dialog.ask")).toHaveCount(0);
  await expect(page.locator("#job-report")).toBeVisible();
});

// ------------------------------------------------------------- while it runs

test("the run reports in the strip, and the app stays usable", async ({ page }) => {
  let release!: (v: unknown) => void;
  const held = new Promise((r) => {
    release = r;
  });
  await withFiles(page, { sqlfiles_run: () => held, sqlfiles_scan: () => ({ ...SCAN, writes: 0 }) });
  await ready(page);
  await page.click("#sf-ok");

  await expect(page.locator("#sqlfiles-dialog")).toBeHidden();
  await expect(page.locator("#job-strip")).toBeVisible();
  await expect(page.locator("#job-strip")).toContainText("Running");
  // The editor still takes typing — the job holds its own connection.
  await page.locator(".cm-content").click();
  await page.keyboard.type("SELECT 1");

  release(OUTCOME);
  await expect(page.locator("#job-strip")).toBeHidden();
});

/**
 * **One job at a time, and Run says which.** There is one strip, and two jobs
 * reporting into it would make both illegible.
 */
test("a second job finds Run disabled with the reason on it", async ({ page }) => {
  let release!: (v: unknown) => void;
  const held = new Promise((r) => {
    release = r;
  });
  await withFiles(page, { sqlfiles_run: () => held, sqlfiles_scan: () => ({ ...SCAN, writes: 0 }) });
  await ready(page);
  await page.click("#sf-ok");
  await expect(page.locator("#job-strip")).toBeVisible();

  // Open it again while that one runs.
  await page.click("#btn-sqlfiles");
  await page.click("#sf-pick-source");
  await page.click("#sf-pick-dest");
  await expect(page.locator("#sf-ok")).toBeDisabled();
  await expect(page.locator("#sf-error")).toContainText("already running");

  await page.click("#sf-cancel");
  release(OUTCOME);
});

test("Stop asks the backend to stop, by the id the run was started with", async ({ page }) => {
  let release!: (v: unknown) => void;
  const held = new Promise((r) => {
    release = r;
  });
  await withFiles(page, { sqlfiles_run: () => held, sqlfiles_scan: () => ({ ...SCAN, writes: 0 }) });
  await ready(page);
  await page.click("#sf-ok");

  await page.locator('#job-strip button:has-text("Stop")').click();
  const started = (await calls(page)).find((c) => c.cmd === "sqlfiles_run")?.args.requestId;
  const stopped = (await calls(page)).find((c) => c.cmd === "sqlfiles_run_cancel")?.args.requestId;
  expect(stopped).toBe(started);
  release({ ...OUTCOME, cancelled: true });
});

// ----------------------------------------------------------------- the report

test("the report names each file and where the CSVs went", async ({ page }) => {
  await withFiles(page, { sqlfiles_scan: () => ({ ...SCAN, writes: 0 }) });
  await ready(page);
  await page.click("#sf-ok");

  const report = page.locator("#job-report");
  await expect(report).toBeVisible();
  await expect(report).toContainText("1 CSV");
  await expect(report).toContainText("1,284 rows");
  await expect(report).toContainText("/home/u/out");
  await expect(report.locator('.job-report-file[data-path="02-orders.sql"]')).toContainText(
    "02-orders.csv",
  );
  // A skipped file keeps its reason here too.
  await expect(report.locator('.job-report-file[data-path="04-proc.sql"]')).toContainText(
    "DELIMITER",
  );
});

/**
 * **The claim §3.3 exists for.** Today's export report goes through
 * `results.setMessage` into `#grid`, and `onActivate` repaints the grid per
 * tab — so switching tabs destroys it. For a batch run the report *is* the
 * deliverable.
 */
test("the report outlives a tab switch and a query", async ({ page }) => {
  await withFiles(page, {
    sqlfiles_scan: () => ({ ...SCAN, writes: 0 }),
    statement_at_cursor: () => ({ sql: "SELECT 1", start: 0 }),
    run_script: () => rowsResult([{ name: "n" }], [["1"]]),
  });
  await ready(page);
  await page.click("#sf-ok");
  await expect(page.locator("#job-report")).toBeVisible();

  // A second tab, then back.
  await page.click(".stab-add");
  await expect(page.locator("#job-report")).toBeVisible();
  await page.locator("#script-tabs .stab").first().click();
  await expect(page.locator("#job-report")).toBeVisible();

  // And a query of our own, which repaints the grid.
  await page.locator(".cm-content").click();
  await page.keyboard.type("SELECT 1");
  await page.keyboard.press("Control+Enter");
  await expect(page.locator("#job-report")).toBeVisible();
  await expect(page.locator("#job-report")).toContainText("02-orders.csv");
});

/**
 * **The row a report by omission drops.** Five successes and one failure, with
 * nothing said about the files never attempted, is a report that misleads.
 */
test("a stopped run lists what failed and what was never reached", async ({ page }) => {
  const stopped = {
    ...OUTCOME,
    stoppedAt: "02-bad.sql:12",
    csvs: 0,
    rows: 0,
    files: [
      { ...OUTCOME.files[0], path: "01-ok.sql" },
      {
        path: "02-bad.sql",
        database: "poc",
        status: { type: "failed", statement: 1, line: 12, message: "Unknown column 'x'" },
        csvs: [],
        statementsRun: 0,
        rowsAffected: 0,
      },
      {
        path: "03-never.sql",
        database: "poc",
        status: { type: "notRun" },
        csvs: [],
        statementsRun: 0,
        rowsAffected: 0,
      },
    ],
  };
  await withFiles(page, {
    sqlfiles_scan: () => ({ ...SCAN, writes: 0 }),
    sqlfiles_run: () => stopped,
  });
  await ready(page);
  await page.click("#sf-ok");

  const report = page.locator("#job-report");
  await expect(report).toContainText("stopped at 02-bad.sql:12");
  const failed = report.locator('.job-report-file[data-path="02-bad.sql"]');
  await expect(failed).toContainText("02-bad.sql:12");
  await expect(failed).toContainText("Unknown column 'x'");
  const never = report.locator('.job-report-file[data-path="03-never.sql"]');
  await expect(never).toContainText("not run");
  await expect(never).toHaveClass(/not-run/);
});

/** A CSV cut short says so, because one that does not is opened and believed. */
test("a partial CSV is labelled", async ({ page }) => {
  const cut = {
    ...OUTCOME,
    cancelled: true,
    files: [
      {
        ...OUTCOME.files[1],
        csvs: [{ name: "02-orders.csv", rows: 500, bytes: 9_000, partial: true }],
      },
    ],
  };
  await withFiles(page, {
    sqlfiles_scan: () => ({ ...SCAN, writes: 0 }),
    sqlfiles_run: () => cut,
  });
  await ready(page);
  await page.click("#sf-ok");

  await expect(page.locator("#job-report")).toContainText("(partial)");
  await expect(page.locator("#job-report")).toContainText("stopped on request");
});

test("Hide dismisses the report", async ({ page }) => {
  await withFiles(page, { sqlfiles_scan: () => ({ ...SCAN, writes: 0 }) });
  await ready(page);
  await page.click("#sf-ok");
  await expect(page.locator("#job-report")).toBeVisible();
  await page.locator('#job-report button:has-text("Hide")').click();
  await expect(page.locator("#job-report")).toBeHidden();
});
