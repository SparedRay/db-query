import { expect, test, type Page } from "@playwright/test";
import {
  type Backend,
  calls,
  connect,
  CONN_INFO,
  installBackend,
  MYSQL_CAPS,
  READ_ONLY_CAPS,
  visit,
} from "./harness";

/**
 * Where the server's time goes (Stage 31).
 *
 * A **dock**, not a modal. The first cut was a `<dialog>`, and Stage 21 §2 had
 * already recorded what that does to a panel whose job is to hand you a
 * statement — so the test that once proved the dialog shut itself before
 * making a tab now proves the opposite, and that inversion is the point.
 *
 * The rule it inherits from query history and every other generated-SQL path
 * here: it **never runs anything**. A good half of what is asserted below is
 * that nothing executed and no DDL was ever sent.
 *
 * The fixtures are the real digest rows measured against the UAT instance on
 * 2026-11-04, so a verdict that changes shows up as a test about a query
 * somebody can go and look at.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

function shape(over: Record<string, unknown> = {}) {
  return {
    digest: "d1",
    text: "SELECT `x` FROM `orders` WHERE `y` = ?",
    database: "appdata",
    verdict: "unclassified",
    class: "application",
    tables: ["orders"],
    execs: 10,
    totalSecs: 1,
    avgMillis: 1,
    p95Millis: null,
    rowsSent: 10,
    rowsExamined: 10,
    ratio: 1,
    noIndex: 0,
    scans: 0,
    sortScans: 0,
    tmpDisk: 0,
    errors: 0,
    lockSecs: 0,
    firstSeen: "2026-09-11 08:35:01",
    lastSeen: "2026-11-04 23:56:00",
    ...over,
  };
}

/** The three shapes that made this feature what it is. */
const CREDITOR_SELECT = shape({
  digest: "supplier-select",
  text:
    "SELECT `SupplierId` , `default_rate` AS `DefaultApr` FROM `Supplier` " +
    "WHERE `tenant_sub_id` = ? AND ( NAME = ? OR `supplier_master_id` = ? )",
  verdict: "missingIndex",
  tables: ["supplier"],
  execs: 40668477,
  totalSecs: 285820.11,
  avgMillis: 7.03,
  rowsSent: 41708854,
  rowsExamined: 108410167913,
  ratio: 2599.2,
  noIndex: 1766585,
  scans: 1766585,
});

const CASBIN = shape({
  digest: "casbin",
  text: "SELECT `c` . `id` , `c` . `ptype` FROM `acl_rule` AS `c`",
  verdict: "noIndexWillHelp",
  tables: ["acl_rule"],
  execs: 3621,
  totalSecs: 32200.7,
  avgMillis: 8892.76,
  rowsSent: 1511063716,
  rowsExamined: 1511063716,
  ratio: 1,
  noIndex: 3621,
  scans: 3621,
});

const SET_NAMES = shape({
  digest: "set-names",
  text: "SET NAMES `utf8mb4`",
  verdict: "driverChatter",
  class: "chatter",
  tables: [],
  execs: 128121716,
  totalSecs: 2320.33,
  avgMillis: 0.02,
  rowsSent: 0,
  rowsExamined: 0,
  ratio: null,
});

const HEARTBEAT = shape({
  digest: "heartbeat",
  text: "INSERT INTO `mysql` . `rds_heartbeat2` ( `id` , VALUE ) VALUES (...)",
  verdict: "platformInternal",
  class: "platform",
  database: null,
  tables: [],
  execs: 2360541,
  totalSecs: 3598.1,
  rowsSent: 0,
  rowsExamined: 0,
  ratio: null,
});

const READINGS = {
  enabled: true,
  digestConsumer: true,
  statementInstruments: 192,
  uptimeSecs: 4721760,
  digestsLost: 0,
  digestsPresent: 3412,
};

function panel(over: Record<string, unknown> = {}) {
  return {
    report: {
      shapes: [CREDITOR_SELECT, CASBIN, SET_NAMES, HEARTBEAT],
      loops: [],
      writes: { supplier: 81333004 },
      totalShapes: 4,
      applicationShapes: 2,
      chatterShapes: 1,
      platformShapes: 1,
      applicationSecs: 318020.81,
      chatterSecs: 2320.33,
      platformSecs: 3598.1,
      hasQuantiles: false,
    },
    diagnosis: {
      usable: true,
      headline: "Reading the server's own counters.",
      detail: "performance_schema is on, readable, and nothing has been lost from the ring.",
      readings: READINGS,
    },
    knownTables: 412,
    databases: ["appdata", "authdata"],
    baselineAt: null,
    windowed: false,
    newShapes: [],
    baselineVoid: false,
    ...over,
  };
}

async function open(page: Page, result: unknown = panel()) {
  await connect(page, { workload_read: () => result });
  await page.click("#btn-workload");
  await expect(page.locator("#wl-pane")).toBeVisible();
}

/** Nothing ran unless the test made it run. */
async function assertNothingRan(page: Page) {
  const names = (await calls(page)).map((c) => c.cmd);
  expect(names).not.toContain("run_script");
}

// --------------------------------------------------------------- the list

test("the panel lists shapes with their verdicts, worst first", async ({ page }) => {
  await open(page);

  const rows = page.locator("#wl-list .wl-row");
  await expect(rows).toHaveCount(2, { timeout: 5000 });

  // Application only, by default: the chatter and the platform row are real
  // and are not what this screen is for.
  await expect(rows.nth(0).locator(".wl-shape")).toContainText("Supplier");
  await expect(rows.nth(0).locator(".wl-verdict")).toHaveText("missing index");
  await expect(rows.nth(1).locator(".wl-verdict")).toHaveText("no index will help");

  await assertNothingRan(page);
});

test("79 hours reads as hours, and 108 billion rows as a short number", async ({ page }) => {
  await open(page);
  const first = page.locator("#wl-list .wl-row").nth(0);
  // 285,820.11 seconds. Printed in full it is unreadable in an 86px column.
  // 79h 23m 40s, and the minutes round rather than truncate — so 24.
  await expect(first).toContainText("79h 24m");
  await expect(first).toContainText("108.41 B");
  await expect(first).toContainText("2,599 : 1");
});

test("chatter and platform traffic are one click away, not deleted", async ({ page }) => {
  await open(page);

  await expect(page.locator("#wl-count-application")).toHaveText("2");
  await expect(page.locator("#wl-count-chatter")).toHaveText("1");
  await expect(page.locator("#wl-count-platform")).toHaveText("1");

  await page.click('#wl-classes .seg-btn[data-class="chatter"]');
  const rows = page.locator("#wl-list .wl-row");
  await expect(rows).toHaveCount(1);
  await expect(rows.nth(0).locator(".wl-shape")).toContainText("SET NAMES");

  await page.click('#wl-classes .seg-btn[data-class="platform"]');
  await expect(rows.nth(0).locator(".wl-shape")).toContainText("rds_heartbeat2");
});

test("sorting by rows examined reorders without asking the server again", async ({ page }) => {
  await open(page);
  const before = (await calls(page)).filter((c) => c.cmd === "workload_read").length;

  await page.selectOption("#wl-sort", "execs");
  const rows = page.locator("#wl-list .wl-row");
  // 40.7 M executions beats 3,621, which is the other way round from time.
  await expect(rows.nth(0).locator(".wl-shape")).toContainText("Supplier");

  const after = (await calls(page)).filter((c) => c.cmd === "workload_read").length;
  expect(after).toBe(before);
});

// ------------------------------------------------------------ the verdicts

test("a query that returns what it reads is never offered an index", async ({ page }) => {
  await open(page);

  // The reason is **in the row**, before any click. The shipped panel put the
  // verdict last and smallest on each row and its reason one click inside, so
  // every row looked alike until it was opened.
  const row = page.locator("#wl-list .wl-row").nth(1);
  await expect(row.locator(".wl-verdict")).toHaveText("no index will help");
  await expect(row.locator(".wl-reason")).toContainText("returns everything it reads");

  await row.click();
  const detail = page.locator(".wl-detail");
  await expect(detail).toBeVisible();
  await expect(detail).not.toContainText("CREATE INDEX");
});

test("the expanded row prices an index from the same digests", async ({ page }) => {
  await open(page);
  await page.click("#wl-list .wl-row >> nth=0");

  // The write count comes from `report.writes`, which the backend totalled
  // over the whole ring — this is the half an advisor usually skips.
  await expect(page.locator(".wl-cost")).toContainText("81,333,004 writes");
  await expect(page.locator(".wl-caveat")).toContainText("no hypothetical index");
});

test("a loop is reported across shapes, not as one row's verdict", async ({ page }) => {
  await open(
    page,
    panel({
      report: {
        ...panel().report,
        loops: [
          {
            table: "supplier",
            digests: ["supplier-select", "u1", "u2"],
            execsLow: 40666497,
            execsHigh: 40668477,
            spread: 0.0000487,
            firstSeenSpreadUs: 140323,
            totalSecs: 298374.95,
          },
        ],
      },
    }),
  );

  // A loop is a finding across rows, but it is *about* particular rows. In a
  // strip of its own above the list it sat over whichever shape happened to
  // sort first, and the one row it was actually about carried no mark.
  const row = page.locator("#wl-list .wl-row").nth(0);
  await expect(row.locator(".wl-shape")).toContainText("Supplier");
  await row.click();

  const loop = page.locator(".wl-detail .wl-loop");
  await expect(loop).toBeVisible();
  await expect(loop).toContainText("3 shapes on supplier ran 40,668,477 down to 40,666,497 times");
  await expect(loop).toContainText("140 ms apart");
  await expect(loop).toContainText("82h 53m");
  await expect(loop).toContainText("only the caller can stop the loop");

  // And it is not on the shape it is not about.
  await row.click();
  await page.locator("#wl-list .wl-row").nth(1).click();
  await expect(page.locator(".wl-detail")).toBeVisible();
  await expect(page.locator(".wl-detail .wl-loop")).toHaveCount(0);
});

// ------------------------------------------------- the lead, and the tiers

test("the finding is a sentence, above every control", async ({ page }) => {
  await open(page);

  // What this replaces: four tiles naming a worst shape, a row count, an
  // index count and a chatter total, with the verdict that explains them
  // three bands further down. Every number was on screen and the reader had
  // to write the sentence themselves.
  const lead = page.locator("#wl-lead");
  await expect(lead).toContainText("79h 24m");
  await expect(lead).toContainText("the last 54 days");
  // The table, from `tables[0]`, which is a server-supplied name — appended as
  // a text node, never interpolated into markup.
  await expect(lead).toContainText("one shape on supplier");
  await expect(lead).toContainText("an index would help it");

  await expect(page.locator("#wl-lead-sub")).toContainText("application shapes do want one");

  // Above the controls, not merely present: a panel that offers you a filter
  // before it tells you anything has answered the wrong question first.
  const leadY = (await lead.boundingBox())!.y;
  const controlY = (await page.locator("#wl-classes").boundingBox())!.y;
  expect(leadY).toBeLessThan(controlY);
});

test("a shape whose predicate cannot be indexed says so, and proposes nothing", async ({
  page,
}) => {
  await open(
    page,
    panel({
      report: {
        ...panel().report,
        shapes: [
          {
            ...CREDITOR_SELECT,
            verdict: "predicateNotIndexable",
            text:
              "SELECT `SupplierId` FROM `Supplier` WHERE `tenant_sub_id` = ? AND ( NAME = ? " +
              "OR `JSON_CONTAINS` ( `LOWER` ( COALESCE ( ALIAS , ? ) ) , ? ) )",
          },
        ],
        applicationShapes: 1,
      },
    }),
  );

  const row = page.locator("#wl-list .wl-row").nth(0);
  await expect(row.locator(".wl-verdict")).toHaveText("predicate cannot be indexed");
  await expect(row.locator(".wl-reason")).toContainText("wraps a column in a function");
  // The sentence has to carry the consequence, not just the label.
  await expect(page.locator("#wl-lead")).toContainText("no index can help it");

  // And the scaffold must not go looking for columns to index.
  await row.click();
  await page.click(".wl-open");
  const sql = await page.locator("#editor .cm-content").innerText();
  expect(sql).toContain("predicate cannot be indexed");
  expect(sql).not.toContain("CREATE INDEX");
});

test("expanding hides the editor, and Escape gives it back", async ({ page }) => {
  await open(page);
  await expect(page.locator("#main")).toBeVisible();

  await page.click("#wl-expand");
  await expect(page.locator("#app")).toHaveAttribute("data-full", "wl");
  await expect(page.locator("#main")).not.toBeVisible();
  await expect(page.locator("#sidebar")).not.toBeVisible();
  // The seam has nothing left to drag against.
  await expect(page.locator("#wlsplit")).not.toBeVisible();
  // The control stays on screen and says what it will do now. Its drawing is
  // one icon turned by `aria-pressed`, so the state is what the test asserts;
  // the words are the accessible name, which the tooltip reads from too.
  await expect(page.locator("#wl-expand")).toHaveAttribute("aria-label", "Collapse");
  await expect(page.locator("#wl-expand")).toHaveAttribute("aria-pressed", "true");

  await page.keyboard.press("Escape");
  await expect(page.locator("#app")).not.toHaveAttribute("data-full", /.*/);
  await expect(page.locator("#main")).toBeVisible();
  await expect(page.locator("#wl-expand")).toHaveAttribute("aria-label", "Expand");
  await expect(page.locator("#wl-expand")).toHaveAttribute("aria-pressed", "false");
});

test("the editor is hidden, not closed — its text survives the round trip", async ({ page }) => {
  await open(page);

  await page.click("#editor .cm-content");
  await page.keyboard.type("SELECT 1 FROM dual");
  await expect(page.locator("#editor .cm-content")).toContainText("SELECT 1 FROM dual");

  await page.click("#wl-expand");
  await expect(page.locator("#main")).not.toBeVisible();
  await page.keyboard.press("Escape");

  // The head says "with your tabs and text intact", which is a promise about
  // `display: none` rather than unmounting. CodeMirror re-measures itself on
  // the way back in — checked against @codemirror/view 6.43.11.
  await expect(page.locator("#main")).toBeVisible();
  await expect(page.locator("#editor .cm-content")).toContainText("SELECT 1 FROM dual");
});

test("expanded, the two lists are siblings and the tabs go away", async ({ page }) => {
  await connect(page, {
    workload_read: () => panel(),
    workload_indexes: () => inventory(),
  });
  await page.click("#btn-workload");
  await expect(page.locator("#wl-pane")).toBeVisible();

  // In the dock one of the two is on screen at a time.
  await expect(page.locator("#wl-views")).toBeVisible();
  await expect(page.locator("#wl-workload")).toBeVisible();
  await expect(page.locator("#wl-indexes")).not.toBeVisible();

  await page.click("#wl-expand");
  // Nothing is competing for the width, so there is nothing left to switch.
  await expect(page.locator("#wl-views")).not.toBeVisible();
  await expect(page.locator("#wl-workload")).toBeVisible();
  await expect(page.locator("#wl-indexes")).toBeVisible();

  // And the inventory can be read from where it is, without a tab to find.
  await page.click("#wl-inv-run");
  await expect(page.locator("#wl-inv .wl-inv-row").first()).toBeVisible();
  await expect(page.locator("#wl-list .wl-row").first()).toBeVisible();
});

test("shutting the panel while expanded does not leave the editor hidden", async ({ page }) => {
  await open(page);
  await page.click("#wl-expand");
  await expect(page.locator("#main")).not.toBeVisible();

  // The state this guards against is a shut panel that is also expanded: the
  // editor hidden behind a pane that is no longer on screen to bring it back.
  await page.click("#wl-close");
  await expect(page.locator("#wl-pane")).not.toBeVisible();
  await expect(page.locator("#app")).not.toHaveAttribute("data-full", /.*/);
  await expect(page.locator("#main")).toBeVisible();
});

test("the rail button also leaves the expanded view", async ({ page }) => {
  await open(page);
  await page.click("#wl-expand");
  await expect(page.locator("#main")).not.toBeVisible();

  await page.click("#btn-workload");
  await expect(page.locator("#wl-pane")).not.toBeVisible();
  await expect(page.locator("#main")).toBeVisible();
});

test("Escape with nothing expanded is left for everything else", async ({ page }) => {
  await open(page);
  // The handler answers only while the panel is expanded, so it cannot
  // swallow an Escape meant for a pane behind it.
  await page.keyboard.press("Escape");
  await expect(page.locator("#wl-pane")).toBeVisible();
  await expect(page.locator("#main")).toBeVisible();
  await expect(page.locator("#app")).not.toHaveAttribute("data-full", /.*/);
});

// ---------------------------------------------------------- never runs DDL

test("the analysis tab is a scaffold, and no DDL is generated", async ({ page }) => {
  await open(page);
  await page.click("#wl-list .wl-row >> nth=0");
  await page.click(".wl-open");

  // **Stays open.** This assertion was the other way round when the panel was
  // a modal, and inverting it is the whole reason it stopped being one: the
  // reading that produced the statement is what you read the tab against.
  await expect(page.locator("#wl-pane")).toBeVisible();
  await expect(page.locator("#wl-list .wl-row")).toHaveCount(2);

  const sql = await page.locator("#editor .cm-content").innerText();
  expect(sql).toContain("SHOW INDEX FROM `supplier`");
  expect(sql).toContain("81,333,004 writes");
  expect(sql).toContain("Where the time goes: missing index");
  // The one thing it must not do: invent an index and present it as an answer.
  expect(sql).not.toMatch(/^\s*CREATE\s+INDEX/im);

  await assertNothingRan(page);
});

// ----------------------------------------------------- the degraded states

test("an unusable server explains itself instead of showing an empty table", async ({ page }) => {
  await open(
    page,
    panel({
      report: null,
      diagnosis: {
        usable: false,
        headline: "The statements_digest consumer is off, so no shape is ever recorded.",
        detail: "performance_schema can be ON with this one consumer disabled.",
        readings: { ...READINGS, digestConsumer: false, digestsPresent: 0 },
      },
    }),
  );

  await expect(page.locator("#wl-diagnosis")).toBeVisible();
  await expect(page.locator("#wl-headline")).toContainText("statements_digest consumer is off");
  await expect(page.locator("#wl-diagnosis")).toHaveClass(/wl-blocked/);
  await expect(page.locator("#wl-body")).toBeHidden();
});

test("a healthy server does not nag, but an overflowed ring says so", async ({ page }) => {
  await open(page);
  // Nothing to warn about: the banner stays out of the way.
  await expect(page.locator("#wl-diagnosis")).toBeHidden();

  await page.click("#wl-close");
  await open(
    page,
    panel({
      diagnosis: {
        usable: true,
        headline: "The digest ring has overflowed, so this list is a sample.",
        detail: "Shapes were dropped rather than counted.",
        readings: { ...READINGS, digestsLost: 14220 },
      },
    }),
  );
  // Usable *and* worth a warning — so the body is shown and so is the banner.
  await expect(page.locator("#wl-diagnosis")).toBeVisible();
  await expect(page.locator("#wl-body")).toBeVisible();
  await expect(page.locator("#wl-diagnosis")).not.toHaveClass(/wl-blocked/);
});

test("a cold schema cache is admitted rather than quietly degrading", async ({ page }) => {
  await open(page, panel({ knownTables: 0 }));
  // It had a line of its own under the controls; it is the same kind of fact
  // as the rest of the scope line, and it is the caveat behind two verdicts.
  await expect(page.locator("#wl-scope")).toContainText("no schema warmed");
});

test("the scope line says how much history the counters cover", async ({ page }) => {
  await open(page);
  // 4,721,760 seconds is 54 days. Without this the top shape's 79 hours has
  // no denominator and means nothing.
  await expect(page.locator("#wl-scope")).toContainText("counters cover 54 days");
  await expect(page.locator("#wl-scope")).toContainText("4 shapes in the ring");
});

test("a list that is only the top of the ring says so", async ({ page }) => {
  // The backend ships the 200 most expensive shapes and totals over all of
  // them, so a scope line that said "3,412 shapes" beside 200 rows would be
  // quietly wrong about what is on screen.
  await open(page, panel({ report: { ...panel().report, totalShapes: 3412 } }));
  await expect(page.locator("#wl-scope")).toContainText("showing 4 of 3,412 shapes");
});

// ------------------------------------------------------------- capability

test("an engine without workload counters is not offered the panel", async ({ page }) => {
  await connect(page, {
    connect: () => ({ ...CONN_INFO, capabilities: READ_ONLY_CAPS }),
  });
  await expect(page.locator("#conn-label")).not.toHaveText("No connection");

  // Hidden, not disabled: there is nothing to explain on a connection that
  // was never going to have this.
  await expect(page.locator("#btn-workload")).toBeHidden();
});

test("with no connection the panel says so rather than failing", async ({ page }) => {
  await installBackend(page, {});
  await visit(page);
  // The button is hidden without a connection, so the panel is reached the
  // way a keyboard user would reach it — directly.
  await page.evaluate(() => {
    document.getElementById("btn-workload")?.removeAttribute("hidden");
  });
  await page.click("#btn-workload");
  await expect(page.locator("#wl-headline")).toContainText("No connection is open");
  await expect(page.locator("#wl-body")).toBeHidden();
});

// ------------------------------------------------------- index inventory

function inventory(over: Record<string, unknown> = {}) {
  return {
    usageRefusal: null,
    findings: [
      {
        fault: "neverRead",
        database: "appdata",
        table: "Supplier",
        name: "idx_altcontact",
        columns: ["AltContact"],
        coveredBy: null,
        writes: 81333004,
        statement: "DROP INDEX `idx_altcontact` ON `appdata`.`Supplier`;",
      },
      {
        fault: "redundant",
        database: "appdata",
        table: "partysettings",
        name: "idx_cfg",
        columns: ["tenantId"],
        coveredBy: "idx_cfg_status",
        writes: 950,
        statement: "DROP INDEX `idx_cfg` ON `appdata`.`partysettings`;",
      },
      {
        fault: "unindexedForeignKey",
        database: "appdata",
        table: "partydocuments",
        name: "fk_client",
        columns: ["PartyId"],
        coveredBy: null,
        writes: null,
        statement:
          "CREATE INDEX `idx_partydocuments_PartyId` ON `appdata`.`partydocuments` (`PartyId`);",
      },
    ],
    usageAvailable: true,
    indexesSeen: 1284,
    databases: ["appdata", "authdata"],
    ...over,
  };
}

async function readInventory(page: Page, result: unknown = inventory()) {
  await connect(page, {
    workload_read: () => panel(),
    workload_indexes: () => result,
  });
  await page.click("#btn-workload");
  await expect(page.locator("#wl-pane")).toBeVisible();
  // The inventory is a sibling list now, not a second scroller stacked under
  // the first one where a 420px column kept it below the fold.
  await page.click('#wl-views .wl-view-tab[data-view="indexes"]');
  await page.click("#wl-inv-run");
}

test("the inventory is asked for, not taken with every read", async ({ page }) => {
  await open(page);
  // Three more queries than the workload needs, so nothing fetches them until
  // somebody wants them.
  const before = (await calls(page)).filter((c) => c.cmd === "workload_indexes");
  expect(before).toHaveLength(0);
  await expect(page.locator("#wl-inv")).toBeHidden();
});

test("the inventory ranks by what an index costs in writes", async ({ page }) => {
  await readInventory(page);

  const rows = page.locator("#wl-inv .wl-inv-row");
  await expect(rows).toHaveCount(3);
  // 81 M writes first: that is what makes an unread index expensive rather
  // than merely untidy.
  await expect(rows.nth(0)).toContainText("never read");
  await expect(rows.nth(0)).toContainText("81.3 M writes");
  await expect(rows.nth(0)).toContainText("DROP INDEX `idx_altcontact`");
  await expect(rows.nth(1)).toContainText("redundant");
  await expect(rows.nth(1)).toContainText("leading prefix of idx_cfg_status");
});

test("an unindexed foreign key proposes a CREATE, and is left unpriced", async ({ page }) => {
  await readInventory(page);
  const fk = page.locator("#wl-inv .wl-inv-row", { hasText: "foreign key, no index" });
  await expect(fk).toContainText("CREATE INDEX");
  // The workload never mentioned the table, so there is nothing to price it
  // with — and an em dash says that better than a zero would.
  await expect(fk.locator(".wl-num")).toHaveText("—");
});

test("a refused usage read leaves never-read absent, not empty", async ({ page }) => {
  await readInventory(
    page,
    inventory({
      usageAvailable: false,
      usageRefusal: "SELECT command denied to user 'app'@'%' for table 'table_io_waits_summary_by_index_usage'",
      findings: [inventory().findings[1]],
    }),
  );
  const note = page.locator("#wl-inv-note");
  await expect(note).toContainText("never-read indexes are not listed");
  // **What the server said, not what we assumed.** This printed
  // "performance_schema refused" for every cause, including a malformed query
  // of our own — which is how a bug in this module gets filed against
  // somebody's database permissions.
  await expect(note).toContainText("SELECT command denied");
});

test("a usage read that failed for our own reasons says so, not 'refused'", async ({ page }) => {
  await readInventory(
    page,
    inventory({
      usageAvailable: false,
      usageRefusal: "You have an error in your SQL syntax near 'reads'",
      findings: [],
    }),
  );
  const note = page.locator("#wl-inv-note");
  await expect(note).toContainText("error in your SQL syntax");
  await expect(note).not.toContainText("performance_schema refused");
});

test("a clean inventory says which of the two claims it is making", async ({ page }) => {
  await readInventory(page, inventory({ findings: [] }));
  await expect(page.locator("#wl-inv .wl-empty")).toContainText("Nothing to question");

  await page.click("#wl-close");
  await readInventory(
    page,
    inventory({ findings: [], usageAvailable: false, usageRefusal: "Deadline exceeded" }),
  );
  await expect(page.locator("#wl-inv .wl-empty")).toContainText(
    "could not be read — see the reason above",
  );
});

test("refreshing the workload drops an inventory taken with the old reading", async ({ page }) => {
  await readInventory(page);
  await expect(page.locator("#wl-inv .wl-inv-row")).toHaveCount(3);

  await page.click("#wl-refresh");
  // Stale write counts beside fresh shape counts would be the worst of both.
  await expect(page.locator("#wl-inv")).toBeHidden();
});

test("nothing in the inventory runs anything", async ({ page }) => {
  await readInventory(page);
  await assertNothingRan(page);
  const names = (await calls(page)).map((c) => c.cmd);
  expect(names).not.toContain("run_sql");
});

// ------------------------------------------------------ baseline and delta

test("without a baseline the window control is offered but not armed", async ({ page }) => {
  await open(page);
  const since = page.locator('#wl-windows .seg-btn[data-window="baseline"]');
  await expect(since).toBeDisabled();
  await expect(page.locator('#wl-windows .seg-btn[data-window="start"]')).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.locator("#wl-baseline")).toHaveText("Set baseline");
});

test("taking a baseline switches straight to the window", async ({ page }) => {
  const TAKEN = Math.floor(Date.now() / 1000) - 600;
  let armed = false;
  await connect(page, {
    workload_baseline_set: () => {
      armed = true;
      return TAKEN;
    },
    workload_read: (a) => {
      if (!armed) return panel();
      return panel({
        baselineAt: TAKEN,
        windowed: a.sinceBaseline === true,
        newShapes: ["supplier-select"],
      });
    },
  });
  await page.click("#btn-workload");
  await page.click("#wl-baseline");

  // The read that followed asked for the window, not the lifetime.
  const reads = (await calls(page)).filter((c) => c.cmd === "workload_read");
  expect(reads[reads.length - 1].args.sinceBaseline).toBe(true);

  await expect(page.locator('#wl-windows .seg-btn[data-window="baseline"]')).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.locator("#wl-baseline")).toHaveText("Re-baseline");
  await expect(page.locator("#wl-scope")).toContainText(
    /window of 10m( \d+s)? since the baseline/,
  );
  await expect(page.locator("#wl-scope")).toContainText("1 shape never seen before it");
});

test("a baseline older than the counters says so and falls back", async ({ page }) => {
  // The window has to be *asked for* here, or the control's two possible
  // sources — what was requested and what came back — happen to agree and the
  // test proves nothing. This is the one case where they differ.
  await open(
    page,
    panel({
      baselineAt: Math.floor(Date.now() / 1000) - 86400,
      // The backend could not compute a window, so it returned the lifetime.
      windowed: false,
      baselineVoid: true,
    }),
  );
  await page.click('#wl-windows .seg-btn[data-window="baseline"]');

  await expect(page.locator("#wl-headline")).toContainText(
    "baseline is older than the server's counters",
  );
  await expect(page.locator("#wl-diagnosis")).toHaveClass(/wl-blocked/);
  // Falling back is the point: the body still shows the lifetime rather than
  // hiding a working server behind a stale file.
  await expect(page.locator("#wl-body")).toBeVisible();
  // The person asked for "baseline" and got the lifetime, so the control must
  // say "start" — it reflects the numbers, not the request.
  await expect(page.locator('#wl-windows .seg-btn[data-window="start"]')).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.locator('#wl-windows .seg-btn[data-window="baseline"]')).toHaveAttribute(
    "aria-pressed",
    "false",
  );
});

test("switching back to the lifetime asks for the lifetime", async ({ page }) => {
  const TAKEN = Math.floor(Date.now() / 1000) - 120;
  await connect(page, {
    workload_read: (a) =>
      panel({ baselineAt: TAKEN, windowed: a.sinceBaseline === true }),
  });
  await page.click("#btn-workload");

  await page.click('#wl-windows .seg-btn[data-window="baseline"]');
  let reads = (await calls(page)).filter((c) => c.cmd === "workload_read");
  expect(reads[reads.length - 1].args.sinceBaseline).toBe(true);

  await page.click('#wl-windows .seg-btn[data-window="start"]');
  reads = (await calls(page)).filter((c) => c.cmd === "workload_read");
  expect(reads[reads.length - 1].args.sinceBaseline).toBe(false);
  await expect(page.locator("#wl-scope")).toContainText("counters cover 54 days");
});

test("a failed baseline is reported rather than silently doing nothing", async ({ page }) => {
  await connect(page, {
    workload_read: () => panel(),
    workload_baseline_set: () => {
      throw new Error("SELECT command denied to user 'app'@'%'");
    },
  });
  await page.click("#btn-workload");
  await page.click("#wl-baseline");

  await expect(page.locator("#wl-headline")).toContainText("baseline was not taken");
  await expect(page.locator("#wl-detail")).toContainText("command denied");
});

// ------------------------------------------------------------ it is a dock

test("the rail button toggles the dock rather than opening a modal", async ({ page }) => {
  await open(page);
  await expect(page.locator("#btn-workload")).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("#app")).not.toHaveClass(/wl-shut/);

  await page.click("#btn-workload");
  await expect(page.locator("#wl-pane")).toBeHidden();
  await expect(page.locator("#btn-workload")).toHaveAttribute("aria-pressed", "false");
  // Zero-width is not gone: a shut pane that still held its buttons would
  // still take Tab.
  await expect(page.locator("#app")).toHaveClass(/wl-shut/);
});

test("the app behind the panel stays live", async ({ page }) => {
  await open(page);
  // The exact thing a modal prevented: looking at the schema while a proposal
  // is on screen.
  await page.click('.node.db:has-text("poc")');
  await expect(page.locator('.node.group:has-text("Tables")')).toBeVisible();
  await expect(page.locator("#wl-pane")).toBeVisible();
});

test("global shortcuts still work with the panel open", async ({ page }) => {
  await open(page);
  const before = await page.locator("#script-tabs .stab").count();
  // `main.ts` skips every global shortcut while a dialog is open, so this
  // did nothing when the panel was a modal.
  await page.keyboard.press("Control+t");
  await expect(page.locator("#script-tabs .stab")).toHaveCount(before + 1);
  await expect(page.locator("#wl-pane")).toBeVisible();
});

test("the seam resizes the dock", async ({ page }) => {
  await open(page);
  const seam = page.locator("#wlsplit");
  await expect(seam).toBeVisible();

  const box = await seam.boundingBox();
  if (!box) throw new Error("no seam");
  await page.mouse.move(box.x + box.width / 2, box.y + 60);
  await page.mouse.down();
  await page.mouse.move(box.x - 140, box.y + 60);
  await page.mouse.up();

  const width = await page.evaluate(() =>
    document.documentElement.style.getPropertyValue("--wl-w-set"),
  );
  expect(width).not.toBe("");
});

test("losing the capability shuts the dock instead of leaving it apologising", async ({ page }) => {
  await open(page);
  await expect(page.locator("#wl-pane")).toBeVisible();

  await page.click("#btn-connect");
  await expect(page.locator("#btn-workload")).toBeHidden();
  await expect(page.locator("#wl-pane")).toBeHidden();
});

test("the dock is shut from the first byte, not by script", async ({ page }) => {
  await connect(page, { workload_read: () => panel() });
  // Adding `wl-shut` from script would paint one frame with a 520px gap in it
  // — and, worse, leaving it off the markup entirely took 527px out of the
  // window from boot and pushed the migrations seam past the right edge.
  // Twelve migrations tests found that; this one names it.
  await expect(page.locator("#app")).toHaveClass(/wl-shut/);
  await expect(page.locator("#wl-pane")).toBeHidden();
  await expect(page.locator("#wlsplit")).toBeHidden();

  // The other two docks keep their own tracks, independently.
  await expect(page.locator("#app")).toHaveClass(/mig-shut/);
  await expect(page.locator("#app")).toHaveClass(/chat-closed/);
});

test("the assistant and the performance panel are open at the same time", async ({ page }) => {
  // The pairing an earlier cut forbade, which was exactly the wrong one to
  // forbid: explaining a shape is the assistant's subject.
  // **Both orders.** Opening either panel runs a different function, so one
  // order exercises one of them — the first version of this test only opened
  // the panel first, and passed with half the exclusion put back.
  await open(page);
  await page.click("#btn-assistant");
  await expect(page.locator("#wl-pane")).toBeVisible();
  await expect(page.locator("#chat-pane")).toBeVisible();
  await expect(page.locator("#btn-workload")).toHaveAttribute("aria-pressed", "true");

  // Shut both, then open them the other way round.
  await page.click("#btn-assistant");
  await page.click("#btn-workload");
  await expect(page.locator("#wl-pane")).toBeHidden();

  await page.click("#btn-assistant");
  await expect(page.locator("#chat-pane")).toBeVisible();
  await page.click("#btn-workload");
  await expect(page.locator("#chat-pane")).toBeVisible();
  await expect(page.locator("#wl-pane")).toBeVisible();

  // And migrations too, for whoever wants it — the width is a choice, not a
  // rule this app enforces.
  await page.click("#btn-migrations");
  await expect(page.locator("#wl-pane")).toBeVisible();
  await expect(page.locator("#chat-pane")).toBeVisible();
  await expect(page.locator("#mig-pane")).toBeVisible();
});

test("all three docks leave the editor on screen on a wide window", async ({ page }) => {
  // On a narrow one it does not, and that is pre-existing: the assistant and
  // migrations together already drive the main column to zero below about
  // 1,100px. The honest assertion is that the layout works where there is
  // room, not that it protects the user from their own toggles.
  await page.setViewportSize({ width: 1920, height: 900 });
  await open(page);
  await page.click("#btn-assistant");
  await page.click("#btn-migrations");

  const width = await page.locator("#main").evaluate((el) => el.getBoundingClientRect().width);
  expect(width).toBeGreaterThan(300);
});

test("asking the assistant carries the evidence and does not send it", async ({ page }) => {
  await open(page);
  await page.click("#wl-list .wl-row >> nth=0");
  await page.click(".wl-explain");

  // The panel opens with the question in the box, unsent.
  await expect(page.locator("#chat-pane")).toBeVisible();
  const asked = await page.locator("#chat-input").inputValue();

  expect(asked).toContain("missing index");
  expect(asked).toContain("40,668,477 times");
  expect(asked).toContain("108,410,167,913 rows");
  expect(asked).toContain("2,599 : 1");
  expect(asked).toContain("1,766,585 executions used no index");
  // The write cost, from the same digests.
  expect(asked).toContain("81,333,004 writes");
  // And the statement itself, which carries no literal values.
  expect(asked).toContain("FROM `Supplier`");
  expect(asked).not.toContain("hunter2");

  // Nothing was asked of a model: no bubble, and the send button is the
  // person's to press.
  await expect(page.locator("#chat-log .chat-msg")).toHaveCount(0);
  const names = (await calls(page)).map((c) => c.cmd);
  expect(names).not.toContain("assistant_send");
  expect(names).not.toContain("assistant_cli_send");

  // The performance panel is still there to read the answer against.
  await expect(page.locator("#wl-pane")).toBeVisible();
});

test("the question names the refusal when there is nothing to index", async ({ page }) => {
  await open(page);
  await page.click("#wl-list .wl-row >> nth=1");
  await page.click(".wl-explain");

  const asked = await page.locator("#chat-input").inputValue();
  expect(asked).toContain("no index will help");
  expect(asked).toContain("1,511,063,716 rows to return 1,511,063,716");
  // The closing instruction has to leave room for "it is not an index".
  expect(asked).toContain("If it is not, say that instead");
});

// ------------------------------------------------- it follows the connection

/**
 * Every number in this panel belongs to one server, and nothing in a reading
 * says which — so a dock left open across a switch showed the server you had
 * just left, under a scope line naming the one you had arrived at. Worse than
 * stale: wrong, and confidently.
 *
 * Two saved connections, both connected, so the switch is the real path
 * (`onActivate` → `syncBusy` → `syncConnLabel`) rather than a poke at the
 * panel's own entry point.
 */
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

/**
 * The QA reading: the same server software, a ring nobody has filled.
 *
 * `report: null` because that is what the command returns when the diagnosis
 * is unusable — there is nothing to report on, and an empty report would be
 * the different claim that the server is idle.
 */
function emptyPanel() {
  return panel({
    report: null,
    knownTables: 0,
    databases: [],
    diagnosis: {
      usable: false,
      headline: "The digest table is empty.",
      detail:
        "performance_schema is on and the consumer is enabled, but nothing has been recorded.",
      readings: { ...READINGS, digestsPresent: 0 },
    },
  });
}

async function twoConnections(page: Page, extra: Backend = {}) {
  await installBackend(page, {
    list_profiles: () => ({ profiles: TWO_SAVED, warning: null }),
    connect_saved: (a) => ({
      id: a.id as string,
      serverVersion: "8.4.0",
      databases: ["appdata"],
      currentDatabase: "appdata",
      capabilities: MYSQL_CAPS,
    }),
    list_tables: () => [],
    list_columns: () => [],
    list_routines: () => [],
    schema_names: () => ({}),
    // The panel's whole subject, served per connection. Keyed off the argument
    // rather than a call counter, so an extra read cannot make this pass.
    workload_read: (a) => (a.connectionId === "qa" ? emptyPanel() : panel()),
    ...extra,
  });
  await visit(page);
}

test("switching connection re-reads the panel instead of showing the old server", async ({
  page,
}) => {
  await twoConnections(page);

  await page.locator('.rail-item[title^="uat"]').click();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
  await page.click("#btn-workload");
  await expect(page.locator("#wl-list .wl-row")).toHaveCount(2);
  await expect(page.locator("#wl-scope")).toContainText("appdata");

  await page.locator('.rail-item[title^="qa"]').click();
  await expect(page.locator(".rail-item.live")).toHaveCount(2);

  // No refresh pressed. The dock stays open — shutting it would be a second
  // way of being wrong — and what is in it is now QA's reading.
  await expect(page.locator("#wl-pane")).toBeVisible();
  await expect(page.locator("#wl-headline")).toContainText("The digest table is empty");
  await expect(page.locator("#wl-body")).toBeHidden();
  const reads = (await calls(page)).filter((c) => c.cmd === "workload_read");
  expect(reads.map((c) => (c.args as { connectionId: string }).connectionId)).toEqual([
    "uat",
    "qa",
  ]);
});

test("a shut dock spends nothing on a switch", async ({ page }) => {
  await twoConnections(page);

  await page.locator('.rail-item[title^="uat"]').click();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
  await page.locator('.rail-item[title^="qa"]').click();
  await expect(page.locator(".rail-item.live")).toHaveCount(2);

  // Opening reads unconditionally, so there is nothing for a shut panel to
  // keep fresh — and the sync that notices the switch runs on far more than
  // a switch.
  expect((await calls(page)).filter((c) => c.cmd === "workload_read")).toHaveLength(0);

  await page.click("#btn-workload");
  await expect(page.locator("#wl-headline")).toContainText("The digest table is empty");
});

test("switching connection forgets the other server's baseline window", async ({ page }) => {
  const TAKEN = Math.floor(Date.now() / 1000) - 600;
  await twoConnections(page, {
    workload_baseline_set: () => TAKEN,
    workload_read: (a) =>
      a.connectionId === "qa"
        ? emptyPanel()
        : panel({ baselineAt: TAKEN, windowed: a.sinceBaseline === true }),
  });

  await page.locator('.rail-item[title^="uat"]').click();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
  await page.click("#btn-workload");
  // Taking it is what selects the window, so this is the state a switch has
  // to clear rather than one the test sets by hand.
  await page.click("#wl-baseline");
  await expect(page.locator('#wl-windows .seg-btn[data-window="baseline"]')).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  // A baseline is stored per connection, so asking for "since the baseline"
  // after a switch asks about a server you are no longer on — and the answer
  // would quietly be the lifetime anyway. Asking for the lifetime says so.
  await page.locator('.rail-item[title^="qa"]').click();
  await expect(page.locator(".rail-item.live")).toHaveCount(2);

  const reads = (await calls(page)).filter((c) => c.cmd === "workload_read");
  const last = reads[reads.length - 1].args as { connectionId: string; sinceBaseline: boolean };
  expect(last.connectionId).toBe("qa");
  expect(last.sinceBaseline).toBe(false);
});
