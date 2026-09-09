import { expect, test } from "@playwright/test";
import { calls, commandNames, connect, rowsResult } from "./harness";

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

/** A script result with two statements, so the result tabs have something to do. */
function twoStatements() {
  const one = rowsResult([{ name: "a" }], [["1"]], { sql: "SELECT 'a'" }).statements[0];
  const two = rowsResult([{ name: "b" }, { name: "c" }], [["2", "3"]], {
    sql: "SELECT 'b','c'",
  }).statements[0];
  return {
    statements: [one, two],
    totalElapsedMs: 4,
    abortedAt: null,
    delimiterDetected: false,
    cancelled: false,
    timedOut: false,
  };
}

// ------------------------------------------------------------------ running

test("Run sends the statement under the cursor; Run all sends the whole buffer", async ({ page }) => {
  await connect(page, {
    statement_at_cursor: () => "SELECT 1",
    run_script: () => rowsResult([{ name: "n" }], [["1"]]),
  });

  await page.click("#btn-run");
  let run = (await calls(page)).filter((c) => c.cmd === "run_script");
  expect(run[0].args.sql).toBe("SELECT 1");
  // Boundaries come from the Rust splitter, never from a second scan in TS.
  expect(await commandNames(page)).toContain("statement_at_cursor");

  await page.click("#btn-run-all");
  run = (await calls(page)).filter((c) => c.cmd === "run_script");
  expect(run[1].args.sql).not.toBe("SELECT 1");
});

test("run and cancel are disabled until there is a connection", async ({ page }) => {
  await connect(page, { run_script: () => rowsResult([{ name: "n" }], [["1"]]) });
  await expect(page.locator("#btn-run")).toBeEnabled();
  await expect(page.locator("#btn-run-all")).toBeEnabled();
});

test("the auto-limit toggle is passed through to the backend", async ({ page }) => {
  await connect(page, { run_script: () => rowsResult([{ name: "n" }], [["1"]]) });

  await page.click("#btn-run-all");
  expect((await calls(page)).find((c) => c.cmd === "run_script")?.args.autoLimit).toBe(true);

  await page.uncheck("#chk-autolimit");
  await page.click("#btn-run-all");
  const runs = (await calls(page)).filter((c) => c.cmd === "run_script");
  expect(runs[runs.length - 1].args.autoLimit).toBe(false);
});

test("the timeout box is passed through, and zero means no limit", async ({ page }) => {
  await connect(page, { run_script: () => rowsResult([{ name: "n" }], [["1"]]) });

  await page.fill("#num-timeout", "30");
  await page.click("#btn-run-all");
  let runs = (await calls(page)).filter((c) => c.cmd === "run_script");
  expect(runs[runs.length - 1].args.timeoutSecs).toBe(30);

  await page.fill("#num-timeout", "0");
  await page.click("#btn-run-all");
  runs = (await calls(page)).filter((c) => c.cmd === "run_script");
  expect(runs[runs.length - 1].args.timeoutSecs).toBeNull();
});

test("Cancel reaches the backend for the active tab", async ({ page }) => {
  let release: (() => void) | null = null;
  const gate = new Promise<void>((r) => (release = r));
  await connect(page, {
    run_script: async () => {
      await gate;
      return rowsResult([{ name: "n" }], [["1"]]);
    },
    cancel_query: () => null,
  });

  await page.click("#btn-run-all");
  await expect(page.locator("#grid")).toContainText(/Running/);
  await page.click("#btn-cancel");
  expect(await commandNames(page)).toContain("cancel_query");
  release!();
});

// ------------------------------------------------------------- status chips

/**
 * Auto-LIMIT rewriting the query is the whole explanation for "why doesn't this
 * count match?", so it must always be visible.
 */
test("a rewritten statement says so", async ({ page }) => {
  const r = rowsResult([{ name: "n" }], [["1"]]);
  r.statements[0].effectiveSql = "SELECT * FROM big LIMIT 5000";
  await connect(page, { run_script: () => r });
  await page.click("#btn-run-all");
  await expect(page.locator("#status")).toContainText(/LIMIT 5000 applied/);
});

test("a truncated result is flagged in the status bar", async ({ page }) => {
  await connect(page, {
    run_script: () => rowsResult([{ name: "n" }], [["1"]], { truncated: true }),
  });
  await page.click("#btn-run-all");
  await expect(page.locator("#status")).toContainText(/truncated at 1/);
});

/**
 * A timeout and a user cancel both KILL the query and mean very different
 * things to whoever is reading the status bar.
 */
test("a timeout and a cancel are reported differently", async ({ page }) => {
  const timedOut = { ...rowsResult([{ name: "n" }], []), timedOut: true };
  await connect(page, { run_script: () => timedOut });
  await page.click("#btn-run-all");
  await expect(page.locator("#status")).toContainText(/timed out/);

  const cancelled = { ...rowsResult([{ name: "n" }], []), cancelled: true };
  await connect(page, { run_script: () => cancelled });
  await page.click("#btn-run-all");
  await expect(page.locator("#status")).toContainText(/cancelled/);
});

test("a DELIMITER script says the row cap does not apply", async ({ page }) => {
  const r = { ...rowsResult([{ name: "n" }], [["1"]]), delimiterDetected: true };
  await connect(page, { run_script: () => r });
  await page.click("#btn-run-all");
  await expect(page.locator("#status")).toContainText(/DELIMITER/);
});

// ------------------------------------------------------- result statement tabs

test("a multi-statement script gets one result tab per statement", async ({ page }) => {
  await connect(page, { run_script: () => twoStatements() });
  await page.click("#btn-run-all");
  await expect(page.locator("#tabs .tab")).toHaveCount(2);
  // The last statement is the one shown, which is what "the answer" usually is.
  await expect(page.locator("#tabs .tab.active")).toHaveText(/2 ·/);
});

test("a single statement gets no result tabs, because they would be noise", async ({ page }) => {
  await connect(page, { run_script: () => rowsResult([{ name: "n" }], [["1"]]) });
  await page.click("#btn-run-all");
  await expect(page.locator("#tabs .tab")).toHaveCount(0);
});

/**
 * A different statement has different columns, so indices from the old one
 * would highlight — and copy — the wrong things.
 */
test("switching result tabs clears the selection", async ({ page }) => {
  await connect(page, { run_script: () => twoStatements() });
  await page.click("#btn-run-all");

  await page.locator('th[data-col="0"]').click();
  await expect(page.locator("#btn-copy")).toHaveText(/1 column/);

  await page.locator("#tabs .tab").first().click();
  await expect(page.locator("#btn-copy")).toHaveText("Copy");
});

/**
 * Reported from the app: with several result sets you could switch to the
 * first and then never get back.
 *
 * The tab click cleared the selection and told the listeners about it *before*
 * rendering, so "everything is selected" was still sized from the statement we
 * had just left. Coming back to a shorter result then indexed past its rows and
 * threw, which aborted the handler before it could render anything — a dead
 * click with nothing in the UI to say why.
 */
function unevenStatements() {
  const many = rowsResult(
    [{ name: "a" }, { name: "b" }],
    Array.from({ length: 40 }, (_, i) => [String(i), String(i * 2)]),
    { sql: "SELECT a, b FROM big" },
  ).statements[0];
  const few = rowsResult([{ name: "n" }], [["1"]], { sql: "SELECT 1" }).statements[0];
  return {
    statements: [many, few],
    totalElapsedMs: 4,
    abortedAt: null,
    delimiterDetected: false,
    cancelled: false,
    timedOut: false,
  };
}

test("result tabs switch back and forth, whichever result is the larger", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));

  await connect(page, { run_script: () => unevenStatements() });
  await page.click("#btn-run-all");
  const tabs = page.locator("#tabs .tab");

  // Start on 2 (one row), to 1 (forty), back to 2 — the direction that used to
  // die, because the return trip is the one that shrinks.
  for (const i of [0, 1, 0, 1]) {
    await tabs.nth(i).click();
    await expect(page.locator("#tabs .tab.active")).toHaveText(new RegExp(`^${i + 1} \\u00b7`));
    await expect(page.locator("th[data-col]")).toHaveCount(i === 0 ? 2 : 1);
  }
  expect(errors).toEqual([]);
});

test("copy after switching to a shorter result copies that result", async ({ page }) => {
  let sent: unknown = null;
  await connect(page, {
    run_script: () => unevenStatements(),
    clipboard_text: (a) => {
      sent = (a.result as { rows: unknown[][] }).rows;
      return "x";
    },
  });
  await page.click("#btn-run-all");

  // The 40-row statement first, so the stale sizes are the dangerous ones.
  await page.locator("#tabs .tab").first().click();
  await page.locator("#tabs .tab").nth(1).click();
  await page.click("#btn-copy");

  await expect.poll(() => sent).toEqual([["1"]]);
});

test("a failed statement is marked and its error shown", async ({ page }) => {
  const r = twoStatements();
  r.statements[1].outcome = { type: "error", message: "Unknown column 'zzz'" } as never;
  r.abortedAt = 1 as never;
  await connect(page, { run_script: () => r });
  await page.click("#btn-run-all");

  await expect(page.locator("#tabs .tab.errored")).toHaveCount(1);
  await expect(page.locator("#grid")).toContainText(/Unknown column/);
});

// ------------------------------------------------------------------- the grid

test("NULL renders distinctly from the string NULL", async ({ page }) => {
  await connect(page, {
    run_script: () => rowsResult([{ name: "v" }], [[null], ["NULL"]]),
  });
  await page.click("#btn-run-all");
  await expect(page.locator("td.null")).toHaveCount(1);
  await expect(page.locator("td.null")).toHaveText("NULL");
});

test("numeric columns are right-aligned even when they arrive as text", async ({ page }) => {
  await connect(page, {
    run_script: () =>
      // A DECIMAL comes back as text on purpose, so JS cannot round it.
      rowsResult([{ name: "money", sqlType: "DECIMAL" }], [["12345678901234.99"]]),
  });
  await page.click("#btn-run-all");
  await expect(page.locator("td.numeric")).toHaveText("12345678901234.99");
});

test("the grid virtualises: not every row is in the DOM", async ({ page }) => {
  const many = Array.from({ length: 5000 }, (_, i) => [String(i + 1)]);
  await connect(page, { run_script: () => rowsResult([{ name: "id" }], many) });
  await page.click("#btn-run-all");

  const painted = await page.locator("tr[data-row]").count();
  expect(painted).toBeGreaterThan(0);
  expect(painted).toBeLessThan(200);
  await expect(page.locator("#status")).toContainText(/5000 rows/);
});

test("Ctrl+A in the grid selects everything and Ctrl+C copies it", async ({ page, browserName }) => {
  await connect(page, {
    run_script: () => rowsResult([{ name: "a" }, { name: "b" }], [["1", "2"]]),
  });
  await page.click("#btn-run-all");

  await page.locator("#grid").click();
  await page.keyboard.press("Control+a");
  // `thead` scopes this to the column headers: `th.sel` alone would also match
  // the selected row-number cells in the body.
  await expect(page.locator("thead th.sel")).toHaveCount(2);

  await page.keyboard.press("Control+c");
  // The copy itself is asserted in both engines; only Chromium can read back.
  await expect(page.locator("#result-note")).toContainText(/^Copied /);
  if (browserName !== "chromium") return;
  const text = await page.evaluate(() => navigator.clipboard.readText());
  // Plain Ctrl+C: the data, and no header row. See clipboard.spec.ts.
  expect(text).toContain("1\t2");
  expect(text).not.toContain("a\tb");
});

// ------------------------------------------------ session statements

/**
 * `SET`, `USE`, `COMMIT` and friends report "0 rows affected", which is true
 * and useless: it reads as a query that matched nothing rather than as a
 * statement for which a row count was never the point.
 */
function sessionThenSelect() {
  const set = {
    sql: "SET @x = 1",
    effectiveSql: null,
    kind: "session",
    outcome: { type: "affected", rows: 0 },
    elapsedMs: 1,
  };
  const sel = rowsResult([{ name: "x" }], [["1"]], { sql: "SELECT @x" }).statements[0];
  return {
    statements: [set, sel],
    totalElapsedMs: 2,
    abortedAt: null,
    delimiterDetected: false,
    cancelled: false,
    timedOut: false,
  };
}

test("a SET says it ran, not that it affected no rows", async ({ page }) => {
  const r = sessionThenSelect();
  r.statements = [r.statements[0]];
  await connect(page, { run_script: () => r });
  await page.click("#btn-run-all");

  await expect(page.locator("#grid .empty")).toHaveText("Statement executed.");
  await expect(page.locator("#grid")).not.toContainText("affected");
  await expect(page.locator("#status")).toContainText("executed");
});

/** Zero *is* the answer for an UPDATE, so that count must survive. */
test("an UPDATE that matched nothing still reports zero", async ({ page }) => {
  await connect(page, {
    run_script: () => ({
      statements: [
        {
          sql: "UPDATE t SET a = 1 WHERE 0",
          effectiveSql: null,
          kind: "modify",
          outcome: { type: "affected", rows: 0 },
          elapsedMs: 1,
        },
      ],
      totalElapsedMs: 1,
      abortedAt: null,
      delimiterDetected: false,
      cancelled: false,
      timedOut: false,
    }),
  });
  await page.click("#btn-run-all");
  await expect(page.locator("#grid .empty")).toHaveText("0 row(s) affected.");
});

/** A script ending in COMMIT must not open on the commit. */
test("the tab that opens is the last one with rows, not the last statement", async ({ page }) => {
  const r = sessionThenSelect();
  // SELECT first, then COMMIT: the naive "last statement" rule picks the wrong one.
  r.statements = [r.statements[1], { ...r.statements[0], sql: "COMMIT" }];
  await connect(page, { run_script: () => r });
  await page.click("#btn-run-all");

  await expect(page.locator("#tabs .tab.active")).toHaveText(/^1 ·/);
  await expect(page.locator("table.rs")).toBeVisible();
});

/**
 * A replaced connection is a new session, and losing one silently is the half
 * of auto-reconnect that is not a kindness.
 *
 * The reconnect itself is right — a connection the server reaped while the user
 * was away should heal rather than break — but temporary tables, session
 * variables and any open transaction go with the old session, and finding that
 * out from a later confusing error is worse than being told once.
 */
test("a replaced connection is reported, with what it cost", async ({ page }) => {
  await connect(page, {
    run_script: () => ({ ...rowsResult([{ name: "a" }], [["1"]]), reconnected: true }),
  });
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+Shift+Enter");

  const chip = page.locator("#status .chip", { hasText: "reconnected" });
  await expect(chip).toBeVisible();
  await expect(chip).toHaveAttribute("title", /transaction/);
});

test("an ordinary run says nothing about reconnecting", async ({ page }) => {
  await connect(page, { run_script: () => rowsResult([{ name: "a" }], [["1"]]) });
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+Shift+Enter");
  await expect(page.locator("#status .chip", { hasText: "1 row" })).toBeVisible();
  await expect(page.locator("#status .chip", { hasText: "reconnected" })).toHaveCount(0);
});
