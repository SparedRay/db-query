import { expect, test, type Page } from "@playwright/test";
import { MYSQL_CAPS, commandNames, installBackend, schemaBackend, visit } from "./harness";

/**
 * Stage 35. Diagnose mode: the same keys, a different job.
 *
 * Two things here are not decoration. **The control that acts changes with the
 * mode** — Stage 34 shipped a disabled option whose reason lived in text that
 * only existed while a popup was open, and a toggle that silently repurposed
 * Run would be the same bug with a bigger blast radius. And **Auto-LIMIT is
 * suspended**, because diagnosing a statement the app quietly rewrote measures
 * a statement nobody runs: a `LIMIT 5000` can turn a sort of 118,944 rows into
 * a sort of 5,000 and hide the whole finding.
 *
 * The backend's own refusal is tested in Rust (`diagnose::tests`), where it
 * belongs: `EXPLAIN ANALYZE` executes what it is given, so the gate is decided
 * from the statement text before a connection exists. What is tested here is
 * that a refusal arrives in front of the user instead of vanishing.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

/** The worked example, as the server would answer it. */
const DIAGNOSIS = {
  statement: "SELECT s.SupplierId FROM Supplier s WHERE s.tenant_sub_id = 4821",
  normalised: "SELECT s.SupplierId FROM Supplier s WHERE s.tenant_sub_id = ?",
  source: "analyze",
  plan: [
    {
      depth: 0,
      op: "Sort",
      detail: "s.updated_at DESC",
      estRows: 50,
      cost: 1.2,
      actualRows: 118944,
      loops: 1,
      timeMs: 842,
      totalMs: 842,
      selfMs: 130,
      neverExecuted: false,
      unparsed: false,
    },
    {
      depth: 1,
      op: "Table scan on Supplier",
      detail: "",
      estRows: 119204,
      cost: 7121,
      actualRows: 1204117,
      loops: 1,
      timeMs: 196,
      totalMs: 196,
      selfMs: 196,
      neverExecuted: false,
      unparsed: false,
    },
  ],
  classic: [],
  classicColumns: [],
  counters: [
    { name: "Handler_read_rnd_next", delta: 1204118, notable: true },
    { name: "Select_scan", delta: 1, notable: true },
  ],
  findings: [
    {
      code: "full-scan",
      title: "A scan where a lookup would do",
      severity: "high",
      evidence: "Supplier was read end to end — 1,204,117 rows — to return 50.",
      suggestion: "CREATE INDEX ix_supplier_tenant_sub_id ON Supplier (tenant_sub_id) INVISIBLE;",
    },
  ],
  measuredMs: 842,
  rowsSent: 50,
  notes: [],
};

async function app(page: Page, extra: Record<string, unknown> = {}) {
  await installBackend(page, {
    ...schemaBackend,
    connect: () => ({
      id: "c1",
      serverVersion: "8.0.36",
      databases: ["analytics"],
      currentDatabase: "analytics",
      capabilities: MYSQL_CAPS,
    }),
    statement_at_cursor: (a: Record<string, unknown>) => ({
      sql: String(a.sql ?? "").trim() || "SELECT 1",
      start: 0,
    }),
    split_statements: (a: Record<string, unknown>) => [
      { sql: String(a.sql ?? "").trim(), start: 0 },
    ],
    diagnose_statement: () => DIAGNOSIS,
    ...extra,
  });
  await visit(page);
  await page.click("#rail-add, .rail-add, #btn-connect");
}

/** Connect through whatever the harness's dialog offers. */
async function connect(page: Page) {
  const ok = page.locator("#conn-ok");
  if (await ok.isVisible().catch(() => false)) await ok.click();
  await expect(page.locator("#btn-run")).toBeEnabled();
}

async function ready(page: Page, extra: Record<string, unknown> = {}) {
  await app(page, extra);
  await connect(page);
}

// ----------------------------------------------------------------- the mode

test("the mode is on the button that acts, not only on the checkbox", async ({ page }) => {
  await ready(page);

  await expect(page.locator("#btn-run")).toContainText("Run");
  await expect(page.locator("#btn-run-all")).toContainText("Run all");

  await page.check("#chk-diagnose");

  // The reason Stage 34 exists: a mode nobody can see from the control they
  // are about to press.
  await expect(page.locator("#btn-run")).toContainText("Diagnose");
  await expect(page.locator("#btn-run-all")).toContainText("Diagnose all");
  await expect(page.locator("#btn-run")).toHaveClass(/diagnosing/);

  await page.uncheck("#chk-diagnose");
  await expect(page.locator("#btn-run")).toContainText("Run");
  await expect(page.locator("#btn-run")).not.toHaveClass(/diagnosing/);
});

test("the shortcut hints survive the mode change", async ({ page }) => {
  await ready(page);
  await page.check("#chk-diagnose");
  // Rebuilding a button's label is the easy way to drop the <kbd> inside it.
  await expect(page.locator("#btn-run kbd")).toHaveText("Ctrl+Enter");
  await expect(page.locator("#btn-run-all kbd")).toHaveText("Ctrl+Shift+Enter");
});

test("Auto-LIMIT is suspended while diagnosing, with its reason on it", async ({ page }) => {
  await ready(page);
  await expect(page.locator("#chk-autolimit")).toBeEnabled();

  await page.check("#chk-diagnose");

  // Disabled rather than ignored: a rewrite the user cannot see would measure
  // a different statement than the one they are asking about.
  await expect(page.locator("#chk-autolimit")).toBeDisabled();
  await expect(page.locator("#chk-autolimit")).toBeChecked();
  const why = await page
    .locator("#chk-autolimit")
    .evaluate((el) => el.closest("label")?.title ?? "");
  expect(why.toLowerCase()).toContain("suspended");

  await page.uncheck("#chk-diagnose");
  await expect(page.locator("#chk-autolimit")).toBeEnabled();
});

// ----------------------------------------------------------------- the view

test("diagnosing shows the plan, the counters and the verdict", async ({ page }) => {
  await ready(page);
  await page.check("#chk-diagnose");
  await page.click("#btn-run");

  const pane = page.locator("#diagnosis-pane");
  await expect(pane).toBeVisible();

  // The source is a chip and not a footnote: "measured" and "estimated" are
  // different claims.
  await expect(pane.locator(".diag-chip.measured")).toContainText("Measured");

  await expect(pane.locator(".diag-node")).toHaveCount(2);
  await expect(pane.locator(".diag-node").nth(1)).toContainText("Table scan on Supplier");
  await expect(pane.locator(".diag-counter")).toHaveCount(2);
  await expect(pane.locator(".diag-finding-title")).toHaveText("A scan where a lookup would do");

  // The grid is not showing: this is a swap, like Sequence.
  await expect(page.locator("#grid")).toBeHidden();
  await expect(page.locator("#res-view-diagnosis")).toHaveAttribute("aria-pressed", "true");
});

test("the estimate and the actual sit together, and a bad estimate is marked", async ({ page }) => {
  await ready(page);
  await page.check("#chk-diagnose");
  await page.click("#btn-run");

  // 50 estimated, 118,944 actual: 2,379× out, which is the finding itself.
  const rows = page.locator("#diagnosis-pane .diag-node").first().locator(".diag-rows");
  await expect(rows).toContainText("50");
  await expect(rows).toContainText("119k");
  await expect(rows.locator(".diag-actual.bad")).toHaveCount(1);
});

test("self time is drawn to scale, largest first", async ({ page }) => {
  await ready(page);
  await page.check("#chk-diagnose");
  await page.click("#btn-run");

  const bars = page.locator("#diagnosis-pane .diag-bar-row");
  await expect(bars).toHaveCount(2);
  // 196 ms before 130 ms — ranked by cost, not by position in the plan.
  await expect(bars.first()).toContainText("Table scan on Supplier");
  await expect(bars.first()).toContainText("196 ms");

  const widths = await page
    .locator("#diagnosis-pane .diag-fill")
    .evaluateAll((els) => els.map((e) => (e as HTMLElement).style.width));
  // Compared as numbers: the browser normalises "100.0%" to "100%".
  expect(parseFloat(widths[0])).toBe(100);
  // 130/196 = 66.3%. A bar that is not to scale is a drawing of nothing.
  expect(parseFloat(widths[1])).toBeGreaterThan(65);
  expect(parseFloat(widths[1])).toBeLessThan(68);
});

test("a refusal is shown rather than swallowed", async ({ page }) => {
  await ready(page, {
    diagnose_statement: () => {
      throw new Error(
        "Only a row-returning statement can be diagnosed. EXPLAIN ANALYZE runs what it is given.",
      );
    },
  });
  await page.check("#chk-diagnose");
  await page.click("#btn-run");

  const pane = page.locator("#diagnosis-pane");
  await expect(pane.locator(".diag-skipped")).toHaveCount(1);
  await expect(pane).toContainText("Only a row-returning statement can be diagnosed");
  // The count on the switch includes it: "one refused" is something this view
  // holds, and a count that hid it would be the quiet failure again.
  await expect(page.locator("#res-count-diagnosis")).toHaveText("1");
});

test("an index suggestion goes to the editor and is never run", async ({ page }) => {
  await ready(page);
  await page.check("#chk-diagnose");
  await page.click("#btn-run");

  await expect(page.locator("#diagnosis-pane .diag-suggestion")).toContainText("CREATE INDEX");

  const before = await page.locator("#script-tabs .stab").count();
  await page.click("#diagnosis-pane .diag-finding button");
  await expect(page.locator("#script-tabs .stab")).toHaveCount(before + 1);

  // Nothing executed: the app generates SQL and waits for the person. Read
  // from the harness's own recorder — an invented global would have made this
  // assertion pass whether the rule held or not.
  expect(await commandNames(page)).not.toContain("run_script");
});

// --------------------------------------------------------------- the handoff

test("the assistant gets the statement normalised, with the values offered separately", async ({
  page,
}) => {
  await ready(page);
  await page.check("#chk-diagnose");
  await page.click("#btn-run");

  await page.click("#diagnosis-pane .diag-ask");
  const asked = page.locator("#assistant-input, #chat-input, textarea.assistant-input").first();
  const normalised = await asked.inputValue();

  // **The decision that keeps data off the network.** The shape travels; the
  // value does not.
  expect(normalised).toContain("tenant_sub_id = ?");
  expect(normalised).not.toContain("4821");
  // Evidence, not just the conclusion.
  expect(normalised).toContain("Table scan on Supplier");
  expect(normalised).toContain("Handler_read_rnd_next");

  await page.click("#diagnosis-pane .diag-ask-raw");
  const asWritten = await asked.inputValue();
  expect(asWritten).toContain("4821");
});

// ------------------------------------------------------------- the capability

test("an engine that cannot diagnose is never offered the mode", async ({ page }) => {
  await ready(page, {
    connect: () => ({
      id: "c1",
      serverVersion: "8.13.0",
      databases: ["catalog"],
      currentDatabase: "catalog",
      // Elasticsearch SQL: no EXPLAIN, no session counters through POST /_sql.
      capabilities: { ...MYSQL_CAPS, engine: "elasticsearch", diagnose: false },
    }),
  });

  // Hidden, not disabled: there is nothing to explain on a connection that was
  // never going to have this.
  await expect(page.locator("#lbl-diagnose")).toBeHidden();
  await expect(page.locator("#btn-run")).toContainText("Run");
  await expect(page.locator("#btn-run")).not.toHaveClass(/diagnosing/);
});

test("the mode does not survive a connection that cannot diagnose", async ({ page }) => {
  // The second connection is Elasticsearch. The first is not, so the mode can
  // be turned on before the switch — which is the case a toggle kept in the
  // DOM gets wrong: Run would go on saying Diagnose over a connection that
  // has nothing to diagnose with.
  let calls = 0;
  await ready(page, {
    connect: () => {
      calls += 1;
      return {
        id: `c${calls}`,
        serverVersion: "8.0.36",
        databases: ["analytics"],
        currentDatabase: "analytics",
        capabilities:
          calls === 1
            ? MYSQL_CAPS
            : { ...MYSQL_CAPS, engine: "elasticsearch", diagnose: false },
      };
    },
  });

  await page.check("#chk-diagnose");
  await expect(page.locator("#btn-run")).toContainText("Diagnose");

  await page.click("#rail-add, .rail-add, #btn-connect");
  const ok = page.locator("#conn-ok");
  if (await ok.isVisible().catch(() => false)) await ok.click();

  await expect(page.locator("#lbl-diagnose")).toBeHidden();
  await expect(page.locator("#chk-diagnose")).not.toBeChecked();
  await expect(page.locator("#btn-run")).toContainText("Run");
  await expect(page.locator("#btn-run")).not.toHaveClass(/diagnosing/);
});
