import { expect, test, type Page } from "@playwright/test";
import { calls, connect } from "./harness";

/**
 * Stage 29. The output pane's two views.
 *
 * The grid answers "what came back" and cannot answer "what did this script
 * do": a `Run all` over four statements shows one of them at a time, and the
 * one that changed rows and the one that failed are each a tab away. Sequence
 * is the same runs as a log.
 *
 * What is only answerable here is what each view actually shows, which of them
 * is in front, and that the log outlives the result it was recorded beside.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

/**
 * No quotes and no brackets anywhere: CodeMirror closes both as you type, and
 * a fixture whose offsets disagreed with the buffer would be testing the
 * harness rather than the app.
 */
const SCRIPT = [
  "SET @cutoff = 1;",
  "SELECT id FROM orders;",
  "UPDATE orders SET status = 2;",
  "SELECT id FROM shipments;",
  "DELETE FROM cart WHERE ord_id = 101;",
].join("\n");

/** Offsets read off `SCRIPT`, so they are the ones the statement really has. */
function stmt(sql: string, kind: string, outcome: unknown, elapsedMs: number) {
  const start = SCRIPT.indexOf(sql);
  if (start === -1) throw new Error(`fixture statement not in SCRIPT: ${sql}`);
  return { sql, start, end: start + sql.length, effectiveSql: null, kind, outcome, elapsedMs };
}

function rows(n: number, name: string) {
  return {
    type: "rows",
    columns: [{ name, sqlType: "INT", typeHint: "numeric" }],
    rows: Array.from({ length: n }, (_, i) => [String(i + 1)]),
    truncated: false,
  };
}

/**
 * One of each: a session statement, rows, a change, more rows, and the error
 * that stopped it. Four shapes, which is the whole point of the view — and
 * **two result sets out of five statements**, which is the point of the strip
 * since Stage 29.1: statements 2 and 4 are Result 1 and Result 2, and nothing
 * else gets a tab at all.
 */
function mixedRun() {
  return {
    statements: [
      stmt("SET @cutoff = 1", "session", { type: "affected", rows: 0 }, 1),
      stmt("SELECT id FROM orders", "select", rows(3, "id"), 8),
      stmt("UPDATE orders SET status = 2", "modify", { type: "affected", rows: 2 }, 12),
      stmt("SELECT id FROM shipments", "select", rows(2, "id"), 5),
      stmt(
        "DELETE FROM cart WHERE ord_id = 101",
        "modify",
        { type: "error", message: "Error 1054: Unknown column ord_id in where clause" },
        30,
      ),
    ],
    totalElapsedMs: 64,
    abortedAt: 4,
    delimiterDetected: false,
    cancelled: false,
    timedOut: false,
    reconnected: false,
  };
}

async function open(page: Page, extra: Record<string, unknown> = {}) {
  await connect(page, {
    run_script: () => mixedRun(),
    // Ctrl+Enter with no selection asks Rust which statement the cursor is in.
    statement_at_cursor: (a: Record<string, unknown>) => {
      const sql = String(a.sql);
      const at = sql.lastIndexOf("DELETE");
      return at === -1 ? null : { sql: sql.slice(at).replace(/;\s*$/, ""), start: at };
    },
    ...extra,
  });
}

async function runAll(page: Page, text = SCRIPT) {
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+a");
  await page.keyboard.type(text);
  await page.keyboard.press("Control+Shift+Enter");
  // Two of the five statements returned rows, so the strip holds two tabs.
  await expect(page.locator("#tabs .tab")).toHaveCount(2);
}

const seq = (page: Page) => page.locator("#sequence");

async function toSequence(page: Page) {
  await page.click("#res-view-sequence");
  await expect(seq(page)).toBeVisible();
}

const ranSql = async (page: Page) =>
  (await calls(page))
    .filter((c) => c.cmd === "run_script")
    .map((c) => c.args.sql as string);

// ------------------------------------------------------------- the header

/**
 * Q1. **Both counts, always.** The view you are not reading has to say whether
 * it holds anything, or a run that returned no rows looks like a run that did
 * nothing.
 */
test("the header offers both views, each with its own count", async ({ page }) => {
  await open(page);
  // The pane is "Output" now: Results is one of two views inside it.
  await expect(page.locator("#res-title")).toHaveText("Output");
  await expect(page.locator("#res-view-results")).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("#res-count-results")).toBeHidden();
  await expect(page.locator("#res-count-sequence")).toBeHidden();

  await runAll(page);
  // **Two against five.** Results counts result sets, Sequence counts
  // statements, and the gap between them is the reason both are shown.
  await expect(page.locator("#res-count-results")).toHaveText("2");
  await expect(page.locator("#res-count-sequence")).toHaveText("5");

  await toSequence(page);
  await expect(page.locator("#res-view-sequence")).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("#res-view-results")).toHaveAttribute("aria-pressed", "false");
  // A swap, not two regions: the grid, its tab strip and the result bar's
  // chips and buttons all belong to Results.
  await expect(page.locator("#grid")).toBeHidden();
  await expect(page.locator("#tabs")).toBeHidden();
  await expect(page.locator("#result-bar-row")).toBeHidden();

  await page.click("#res-view-results");
  await expect(page.locator("#grid")).toBeVisible();
  await expect(seq(page)).toBeHidden();
});

/**
 * Q2. Per tab, like the result-set index and unlike Ctrl+J: switching tabs must
 * not change which view you were reading.
 */
test("the chosen view belongs to the tab, not the window", async ({ page }) => {
  await open(page);
  await runAll(page);
  await toSequence(page);

  await page.keyboard.press("Control+t");
  await expect(page.locator("#res-view-results")).toHaveAttribute("aria-pressed", "true");
  await expect(seq(page)).toBeHidden();
  await expect(page.locator("#grid")).toBeVisible();

  await page.locator("#script-tabs .stab").first().click();
  await expect(page.locator("#res-view-sequence")).toHaveAttribute("aria-pressed", "true");
  await expect(seq(page)).toBeVisible();
});

/** Q9. Shut, the strip is all there is, so its summary follows the view. */
test("shut, the summary follows the view in front", async ({ page }) => {
  await open(page);
  await runAll(page);
  await page.click("#res-toggle");
  await expect(page.locator("#results-body")).toBeHidden();

  const summary = page.locator("#res-summary");
  await expect(summary).toBeVisible();
  await expect(summary).not.toBeEmpty();
  // Results reads the status chips, so it is not saying what Sequence says.
  await expect(summary).not.toHaveText(/^DELETE · /);

  await page.click("#res-view-sequence");
  // The newest run's last statement: where the script actually stopped.
  await expect(summary).toHaveText(/^DELETE · Error 1054/);
  await expect(summary).toContainText("30 ms");
  // And choosing a view did not undo Ctrl+J.
  await expect(page.locator("#results-body")).toBeHidden();
});

// --------------------------------------------------------------- the log

/** Q4. One line per statement, under the run that issued it. */
test("Sequence lists what ran, grouped under its run", async ({ page }) => {
  await open(page);
  await runAll(page);
  await toSequence(page);

  await expect(seq(page).locator(".seq-run")).toHaveCount(1);
  await expect(seq(page).locator(".seq-run .seq-what")).toHaveText("Run all");
  const summary = seq(page).locator(".seq-run-summary");
  await expect(summary).toContainText("5 statements");
  await expect(summary).toContainText("stopped at statement 5");
  await expect(summary).toContainText("64 ms");

  const rows = seq(page).locator(".seq-row");
  await expect(rows).toHaveCount(5);

  // **The verb is read off the SQL**, because `kind` is a category: `modify`
  // covers the UPDATE and the DELETE, which are two different things to find.
  await expect(rows.nth(0).locator(".seq-verb")).toHaveText("SET");
  await expect(rows.nth(1).locator(".seq-verb")).toHaveText("SELECT");
  await expect(rows.nth(2).locator(".seq-verb")).toHaveText("UPDATE");
  await expect(rows.nth(3).locator(".seq-verb")).toHaveText("SELECT");
  await expect(rows.nth(4).locator(".seq-verb")).toHaveText("DELETE");

  // What each one did, in its own words.
  await expect(rows.nth(0)).toContainText("OK");
  await expect(rows.nth(1)).toContainText("3 rows");
  await expect(rows.nth(2)).toContainText("2 rows affected");
  await expect(rows.nth(3)).toContainText("2 rows");
  await expect(rows.nth(4)).toContainText("Unknown column");
  await expect(rows.nth(4).locator(".seq-ms")).toHaveText("30 ms");

  // Shape as well as colour: four states told apart at a glance.
  await expect(rows.nth(0).locator(".seq-dot")).toHaveClass(/seq-ok/);
  await expect(rows.nth(1).locator(".seq-dot")).toHaveClass(/seq-data/);
  await expect(rows.nth(2).locator(".seq-dot")).toHaveClass(/seq-change/);
  await expect(rows.nth(3).locator(".seq-dot")).toHaveClass(/seq-data/);
  await expect(rows.nth(4).locator(".seq-dot")).toHaveClass(/seq-error/);
});

/** A second run is its own group, and the one you just did is on top. */
test("each run is its own group, newest first", async ({ page }) => {
  await open(page);
  await runAll(page);
  await toSequence(page);
  await expect(seq(page).locator(".seq-run")).toHaveCount(1);

  // Ctrl+Enter runs the statement under the cursor, which is a different
  // gesture and says so.
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+Enter");
  await expect(seq(page).locator(".seq-run")).toHaveCount(2);

  const what = seq(page).locator(".seq-what");
  await expect(what.nth(0)).toHaveText("Run");
  await expect(what.nth(1)).toHaveText("Run all");
  // Eight statements across the two runs, and the count says so.
  await expect(page.locator("#res-count-sequence")).toHaveText("10");
  // **Only the newest run can link.** The run before it had its result
  // replaced, and offering a link to one would send you to another run's rows
  // — which, since both ran the same script, would look right.
  await expect(seq(page).locator(".seq-link")).toHaveCount(2);
});

/**
 * Q5. The link names the result tab the strip actually shows.
 *
 * Numbered **among the result sets**, which is all the strip holds since Stage
 * 29.1: statements 2 and 4 of five are Result 1 and Result 2. Stage 29 had to
 * say 2 and 4 here, because the strip then numbered every statement — the
 * canvas drew it this way from the start and only needed the strip to agree.
 */
test("a statement that returned rows links to its own result tab", async ({ page }) => {
  await open(page);
  await runAll(page);
  await toSequence(page);

  const links = seq(page).locator(".seq-link");
  await expect(links).toHaveCount(2);
  await expect(links.nth(0)).toHaveText("3 rows → Result 1");
  await expect(links.nth(1)).toHaveText("2 rows → Result 2");

  await links.nth(1).click();
  // It takes you there: Results, with that result selected.
  await expect(page.locator("#grid")).toBeVisible();
  await expect(page.locator("#res-view-results")).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("#tabs .tab").nth(1)).toHaveClass(/active/);
  await expect(page.locator("#grid tbody tr[data-row]")).toHaveCount(2);
});

/** Q5. And the SQL goes the other way, to the statement in the buffer. */
test("a row's SQL selects that statement in the editor", async ({ page }) => {
  await open(page);
  await runAll(page);
  await toSequence(page);

  await seq(page).locator(".seq-row").nth(2).locator(".seq-sql").click();
  // Asserted through what the editor does with the selection: Ctrl+Enter runs
  // the selection, so the right text selected means the right statement runs.
  await page.keyboard.press("Control+Enter");
  await expect
    .poll(async () => {
      const ran = await ranSql(page);
      return ran[ran.length - 1];
    })
    .toBe("UPDATE orders SET status = 2");
});

/**
 * Q6. **The log is not a view of `tab.result`.** `closeStatement` splices that
 * array to release the rows, which is exactly why the log keeps its own
 * records — but the link has to go with the result it pointed at.
 */
test("closing a result keeps its Sequence row and drops only the link", async ({ page }) => {
  await open(page);
  await runAll(page);
  await toSequence(page);
  await expect(seq(page).locator(".seq-link")).toHaveCount(2);

  await page.click("#res-view-results");
  await page.locator("#tabs .tab").nth(1).locator(".tab-close").click();
  await expect(page.locator("#res-count-results")).toHaveText("1");

  await toSequence(page);
  await expect(seq(page).locator(".seq-row")).toHaveCount(5);
  await expect(seq(page).locator(".seq-row").nth(3)).toContainText("2 rows");
  await expect(page.locator("#res-count-sequence")).toHaveText("5");
  // The closed one stops offering a link; the one still open keeps its own.
  await expect(seq(page).locator(".seq-link")).toHaveCount(1);
  await expect(seq(page).locator(".seq-link")).toHaveText("3 rows → Result 1");
});

/** Closing every result leaves the log alone too. */
test("discarding the results leaves the log standing", async ({ page }) => {
  await open(page);
  await runAll(page);
  await page.click("#btn-close-results");
  await expect(page.locator("#res-count-results")).toBeHidden();

  await toSequence(page);
  await expect(seq(page).locator(".seq-row")).toHaveCount(5);
  await expect(page.locator("#res-count-sequence")).toHaveText("5");
});

/** One statement, one failure — a run with nothing in it to call a change. */
function justAnError() {
  return {
    statements: [
      stmt(
        "DELETE FROM cart WHERE ord_id = 101",
        "modify",
        { type: "error", message: "Error 1054: Unknown column ord_id in where clause" },
        4,
      ),
    ],
    totalElapsedMs: 4,
    abortedAt: 0,
    delimiterDetected: false,
    cancelled: false,
    timedOut: false,
    reconnected: false,
  };
}

/** One statement that only returned rows: no change, no error. */
function rowsOnly() {
  return {
    statements: [
      stmt(
        "SELECT id FROM orders",
        "select",
        {
          type: "rows",
          columns: [{ name: "id", sqlType: "INT", typeHint: "numeric" }],
          rows: [["1"]],
          truncated: false,
        },
        3,
      ),
    ],
    totalElapsedMs: 3,
    abortedAt: null,
    delimiterDetected: false,
    cancelled: false,
    timedOut: false,
    reconnected: false,
  };
}

const chips = (page: Page) => page.locator("#seq-chips .chip");

/** Capture what reaches the clipboard, both paths, as `history.spec` does. */
async function watchClipboard(page: Page) {
  await page.evaluate(() => {
    const w = window as unknown as { copied?: string };
    w.copied = undefined;
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: (t: string) => {
          w.copied = t;
          return Promise.resolve();
        },
      },
    });
  });
}

const copied = (page: Page) =>
  page.evaluate(() => (window as unknown as { copied?: string }).copied ?? "");

// ------------------------------------------------- the filter and the footer

/**
 * Q7. **The chips count the whole log.** They are its summary the way the
 * result bar is a result set's, so a filter must not change them — otherwise
 * "1 error" under Errors is the same figure said twice and nothing says what
 * the tab actually did.
 */
test("the footer counts what the log holds", async ({ page }) => {
  await open(page);
  await runAll(page);
  await toSequence(page);

  await expect(chips(page).nth(0)).toHaveText("5 statements");
  await expect(chips(page).nth(1)).toHaveText("1 changed 2 rows");
  await expect(chips(page).nth(2)).toHaveText("1 error");
  await expect(page.locator("#seq-scope")).toHaveText("This tab, since it opened");
});

test("Changes and Errors filter the list, and the chips keep counting the log", async ({
  page,
}) => {
  await open(page);
  await runAll(page);
  await toSequence(page);
  await expect(seq(page).locator(".seq-row")).toHaveCount(5);

  await page.click("#seq-changes");
  await expect(page.locator("#seq-changes")).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("#seq-all")).toHaveAttribute("aria-pressed", "false");
  await expect(seq(page).locator(".seq-row")).toHaveCount(1);
  await expect(seq(page).locator(".seq-verb")).toHaveText("UPDATE");
  // Unmoved by the filter, and the one place that says what it hid.
  await expect(chips(page).nth(0)).toHaveText("5 statements");
  await expect(page.locator("#seq-scope")).toHaveText("Showing 1 of 5");

  await page.click("#seq-errors");
  await expect(seq(page).locator(".seq-row")).toHaveCount(1);
  await expect(seq(page).locator(".seq-verb")).toHaveText("DELETE");

  await page.click("#seq-all");
  await expect(seq(page).locator(".seq-row")).toHaveCount(5);
  await expect(page.locator("#seq-scope")).toHaveText("This tab, since it opened");
});

/**
 * The filter belongs to the tab, like the view above it (decision 4).
 *
 * **Two tabs filtered differently at the same time** is the only thing that
 * tells per-tab from one setting for the window. The first version of this
 * test set a filter, opened a second tab and came back — and passed with the
 * filter made global, because both tabs had ended up holding the same value.
 */
test("two tabs hold their own filters at once", async ({ page }) => {
  await open(page);
  await runAll(page);
  await toSequence(page);
  await page.click("#seq-errors");
  await expect(seq(page).locator(".seq-row")).toHaveCount(1);

  // A second tab, with a log of its own, left on All.
  await page.keyboard.press("Control+t");
  await expect(page.locator("#seq-filters")).toBeHidden();
  await runAll(page);
  await page.click("#res-view-sequence");
  await expect(page.locator("#seq-all")).toHaveAttribute("aria-pressed", "true");
  await expect(seq(page).locator(".seq-row")).toHaveCount(5);

  // And the first one is still where it was left.
  await page.locator("#script-tabs .stab").first().click();
  await expect(page.locator("#seq-errors")).toHaveAttribute("aria-pressed", "true");
  await expect(seq(page).locator(".seq-row")).toHaveCount(1);
});

/** A run the filter empties loses its header: a heading over a gap says nothing. */
test("a run with nothing left in it loses its header", async ({ page }) => {
  await open(page, {
    run_script: (a: Record<string, unknown>) =>
      String(a.sql).startsWith("DELETE") ? justAnError() : mixedRun(),
  });
  await runAll(page);
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+Enter");
  await toSequence(page);
  await expect(seq(page).locator(".seq-run")).toHaveCount(2);

  // Only the first run changed anything, so only its header survives Changes.
  await page.click("#seq-changes");
  await expect(seq(page).locator(".seq-run")).toHaveCount(1);
  await expect(seq(page).locator(".seq-what")).toHaveText("Run all");
  await expect(seq(page).locator(".seq-row")).toHaveCount(1);
  // Both runs are still counted: a filter changes the view, not the history.
  await expect(chips(page).nth(0)).toHaveText("6 statements");
  await expect(page.locator("#seq-scope")).toHaveText("Showing 1 of 6");
});

/** And a filter that matches nothing says which nothing it is. */
test("a filter that matches nothing says so in its own words", async ({ page }) => {
  await open(page, { run_script: () => rowsOnly() });
  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("SELECT id FROM orders;");
  await page.keyboard.press("Control+Shift+Enter");
  await expect(page.locator("#res-count-sequence")).toHaveText("1");
  await toSequence(page);
  await expect(seq(page).locator(".seq-row")).toHaveCount(1);

  await page.click("#seq-changes");
  await expect(seq(page).locator(".seq-empty")).toContainText("has changed any rows");
  await page.click("#seq-errors");
  await expect(seq(page).locator(".seq-empty")).toContainText("Nothing has failed");
});

// --------------------------------------------------------- copying, clearing

/** Q7. The log as text, and the button says what happened to it. */
test("Copy as log copies what is shown, and reports on itself", async ({ page }) => {
  await open(page);
  await runAll(page);
  await toSequence(page);
  await watchClipboard(page);

  await page.click("#seq-copy");
  await expect(page.locator("#seq-copy")).toHaveText("Copied");

  const text = await copied(page);
  expect(text).toContain("Run all");
  expect(text).toContain("5 statements");
  // One line per statement, tab-separated — the grid's own copy format.
  expect(text).toContain("\tSELECT\t8 ms\t3 rows\tSELECT id FROM orders");
  expect(text).toContain("DELETE");
  expect(text).toContain("Unknown column");

  // It gives its label back rather than saying "Copied" forever.
  await expect(page.locator("#seq-copy")).toHaveText("Copy as log", { timeout: 6000 });
});

/** Filtered, because copying what is not on screen is a surprise. */
test("Copy as log follows the filter", async ({ page }) => {
  await open(page);
  await runAll(page);
  await toSequence(page);
  await page.click("#seq-errors");
  await watchClipboard(page);

  await page.click("#seq-copy");
  await expect(page.locator("#seq-copy")).toHaveText("Copied");
  const text = await copied(page);
  expect(text).toContain("DELETE");
  expect(text).not.toContain("SELECT id FROM orders");
  expect(text).not.toContain("UPDATE");
});

/**
 * Q7. **Clear is about the log.** The rows in the grid were asked for
 * separately, and "Close results" is how you say you are done with those.
 */
test("Clear empties the log and leaves the results standing", async ({ page }) => {
  await open(page);
  await runAll(page);
  await toSequence(page);

  await page.click("#seq-clear");
  await expect(seq(page).locator(".seq-empty")).toContainText("Nothing has run in this tab yet");
  await expect(page.locator("#seq-filters")).toBeHidden();
  await expect(page.locator("#seq-foot")).toBeHidden();
  await expect(page.locator("#res-count-sequence")).toBeHidden();

  // The results are untouched.
  await expect(page.locator("#res-count-results")).toHaveText("2");
  await page.click("#res-view-results");
  await expect(page.locator("#tabs .tab")).toHaveCount(2);
  await page.locator("#tabs .tab").nth(0).click();
  await expect(page.locator("#grid table")).toBeVisible();
  await expect(page.locator("#grid tbody tr[data-row]")).toHaveCount(3);
});

/** Clearing puts the filter back: there is nothing left to be filtering. */
test("Clear puts the filter back to All", async ({ page }) => {
  await open(page);
  await runAll(page);
  await toSequence(page);
  await page.click("#seq-errors");
  await page.click("#seq-clear");
  await expect(page.locator("#seq-all")).toHaveAttribute("aria-pressed", "true");

  // The buffer is untouched by a Clear, so running it again is one keystroke —
  // from the editor, which is where that keystroke lives.
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+Shift+Enter");
  await expect(seq(page).locator(".seq-row")).toHaveCount(5);
});

/**
 * A run of `n` statements that all changed one row, for the cap and for the
 * windowing. Offsets are synthetic and unique, which is all either needs.
 */
function bigRun(n: number, tag: string) {
  return {
    statements: Array.from({ length: n }, (_, i) => ({
      sql: `UPDATE t SET n = ${i} /* ${tag} */`,
      start: i * 64,
      end: i * 64 + 40,
      effectiveSql: null as string | null,
      kind: "modify",
      outcome: { type: "affected", rows: 1 },
      elapsedMs: 1,
    })),
    totalElapsedMs: n,
    abortedAt: null,
    delimiterDetected: false,
    cancelled: false,
    timedOut: false,
    reconnected: false,
  };
}

/**
 * Switch to Sequence *before* running, so the run never paints Results.
 *
 * Not tidiness: the result strip draws one tab per statement, so a 1,100
 * statement fixture would build 1,100 buttons this test has no interest in.
 */
async function runInSequence(page: Page, text: string) {
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+a");
  await page.keyboard.type(text);
  await page.keyboard.press("Control+Shift+Enter");
}

// ----------------------------------------------------- a run in flight

/**
 * A run in flight, in the log.
 *
 * The backend answers once, at the end, so a run that has started has no
 * entries yet — it is a header with nothing under it, at the top where its
 * statements will appear. It names **which** run, which is the part the grid's
 * "Running…" cannot: a log of five runs needs to know whether the sixth is a
 * Run all or one statement.
 */
test("a run in flight is a line at the top of the log", async ({ page }) => {
  const gate = held();
  await open(page, { run_script: () => gate.promise });
  await toSequence(page);
  // Nothing has run yet, so this is also the first-run case: the sentence has
  // to give way to the line rather than claim nothing has happened.
  await expect(seq(page).locator(".seq-empty")).toBeVisible();

  await runInSequence(page, SCRIPT);
  const running = seq(page).locator(".seq-run.seq-running");
  await expect(running).toBeVisible();
  await expect(running).toContainText("Executing");
  await expect(running).toContainText("Run all");
  await expect(seq(page).locator(".seq-empty")).toHaveCount(0);

  gate.release(mixedRun());
  await expect(running).toHaveCount(0);
  await expect(seq(page).locator(".seq-row")).toHaveCount(5);
});

/** It sits above the run before it, and does not disturb what is already there. */
test("a run in flight sits above the log it is adding to", async ({ page }) => {
  const gate = held();
  let first = true;
  await open(page, {
    run_script: () => {
      if (!first) return gate.promise;
      first = false;
      return mixedRun();
    },
  });
  await toSequence(page);
  await runInSequence(page, SCRIPT);
  await expect(seq(page).locator(".seq-row")).toHaveCount(5);

  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+Shift+Enter");
  // The line is first; the finished run is still under it, entries and all.
  await expect(seq(page).locator(".seq-run").first()).toHaveClass(/seq-running/);
  await expect(seq(page).locator(".seq-row")).toHaveCount(5);

  gate.release(mixedRun());
  await expect(seq(page).locator(".seq-run.seq-running")).toHaveCount(0);
  await expect(seq(page).locator(".seq-row")).toHaveCount(10);
});

/** Shut, the strip says it too, and says which run. */
test("shut, the strip says a run is executing", async ({ page }) => {
  const gate = held();
  await open(page, { run_script: () => gate.promise });
  await toSequence(page);
  await page.click("#res-toggle");
  await expect(page.locator("#results-body")).toBeHidden();

  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+a");
  await page.keyboard.type(SCRIPT);
  await page.keyboard.press("Control+Shift+Enter");
  await expect(page.locator("#res-summary")).toHaveText("Executing \u00b7 Run all");

  gate.release(mixedRun());
  await expect(page.locator("#res-summary")).toHaveText(/^DELETE \u00b7 Error 1054/);
});

/** A Node-side promise the test opens when it chooses, as migrations.spec does. */
function held() {
  let release!: (v: unknown) => void;
  const promise = new Promise<unknown>((r) => {
    release = r;
  });
  return { promise, release };
}

// ---------------------------------------------------- a run with no rows

/**
 * Q8. **Results stays in front.** A view that switched itself would make the
 * count on the header unreadable as a fact, and would take the pane away from
 * somebody who ran a migration script on purpose. So it says what the run did
 * instead, and offers the move.
 */
test("a run that returned no rows says so, and offers Sequence", async ({ page }) => {
  await open(page, { run_script: () => bigRun(3, "norows") });
  await runInSequence(page, "UPDATE t SET n = 1;");
  await page.click("#res-view-results");

  const hint = page.locator("#grid .empty-hint");
  await expect(hint).toBeVisible();
  await expect(hint).toContainText("No result sets in this run");
  await expect(hint).toContainText("3 statements");
  await expect(hint).toContainText("3 changed 3 rows");

  // It did not switch on its own — and the header says the other view has
  // something, which is the whole reason this is not a dead end.
  await expect(page.locator("#res-view-results")).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("#res-count-sequence")).toHaveText("3");

  await hint.locator("button").click();
  await expect(page.locator("#res-view-sequence")).toHaveAttribute("aria-pressed", "true");
  await expect(seq(page).locator(".seq-row")).toHaveCount(3);
});

/**
 * And discarding the rows does not turn it into a run that never had any.
 *
 * The hint asks the **log**, not the live result array: `closeStatement`
 * splices that one, so a view asking it would tell you there were no result
 * sets in a run whose result set you had just closed yourself.
 */
test("closing every result does not make it a run with no result sets", async ({ page }) => {
  await open(page);
  await runAll(page);

  // Both of them, so the live array has no row-returning statement left while
  // three statements remain — which is the state that tells the two gates
  // apart.
  await page.locator("#tabs .tab").nth(1).locator(".tab-close").click();
  await expect(page.locator("#tabs .tab")).toHaveCount(0);
  await page.click("#btn-close-results");
  await expect(page.locator("#res-count-results")).toBeHidden();

  // No hint: the run returned rows, and you closed them. The log remembers.
  await expect(page.locator("#grid .empty-hint")).toHaveCount(0);
});

/** And a run that *did* return rows has no business saying it returned none. */
test("a run with a result set shows no such hint", async ({ page }) => {
  await open(page);
  await runAll(page);
  await expect(page.locator("#grid .empty-hint")).toHaveCount(0);
  await page.locator("#tabs .tab").nth(0).click();
  await expect(page.locator("#grid table")).toBeVisible();
  await expect(page.locator("#grid .empty-hint")).toHaveCount(0);
});

/**
 * **Only a table gets a tab.** The SET, the UPDATE and the failure were each a
 * tab holding one sentence; a list of things that happened is what Sequence is
 * for, and the strip is for tables you look at.
 */
test("only the statements that returned rows get a tab", async ({ page }) => {
  await open(page);
  await runAll(page);

  const tabs = page.locator("#tabs .tab");
  await expect(tabs).toHaveCount(2);
  await expect(tabs.nth(0)).toContainText("Result 1");
  await expect(tabs.nth(1)).toContainText("Result 2");
  // The two SELECTs, by their row counts, and nothing wearing "affected" or an
  // error mark.
  await expect(tabs.nth(0).locator(".badge")).toHaveText("3");
  await expect(tabs.nth(1).locator(".badge")).toHaveText("2");
  await expect(page.locator("#tabs .tab.errored")).toHaveCount(0);
  await expect(page.locator("#tabs")).not.toContainText("affected");

  // It opens on the last result set, not on the statement that stopped it.
  await expect(tabs.nth(1)).toHaveClass(/active/);
  // The failure is still marked, and its message is on the chip that marks it.
  const chip = page.locator("#status .chip", { hasText: "aborted at 5" });
  await expect(chip).toBeVisible();
  await expect(chip).toHaveAttribute("title", /Unknown column/);
});

// ------------------------------------------------- the cap, and the painting

/**
 * Q10. The cap bounds history, and **a run is never split** — half a run would
 * make its own summary a lie about what is on screen. So whole runs go, and
 * the footer says how many.
 */
test("a log past the cap drops whole runs, and the footer says so", async ({ page }) => {
  let nth = 0;
  await open(page, { run_script: () => bigRun(1100, `run${++nth}`) });
  await toSequence(page);

  await runInSequence(page, "UPDATE t SET n = 1;");
  await expect(page.locator("#res-count-sequence")).toHaveText("1100");
  await expect(chips(page)).toHaveCount(2);

  await runInSequence(page, "UPDATE t SET n = 2;");
  // 2,200 past a cap of 2,000: the older run goes entire, not trimmed.
  await expect(page.locator("#res-count-sequence")).toHaveText("1100");
  await expect(chips(page).last()).toHaveText("1 earlier run dropped (1100 statements)");
  await expect(seq(page).locator(".seq-run")).toHaveCount(1);
  // The one kept is the newest.
  await expect(seq(page).locator(".seq-sql").first()).toContainText("run2");
});

/**
 * Q10. Windowed, not built row by row. §3 measured a plain list at ~64µs a row,
 * so a thousand of them is a visible hitch on a view switch.
 */
test("the list paints a window, and the far end is still reachable", async ({ page }) => {
  await open(page, { run_script: () => bigRun(1100, "big") });
  await toSequence(page);
  await runInSequence(page, "UPDATE t SET n = 1;");
  await expect(page.locator("#res-count-sequence")).toHaveText("1100");

  const painted = await seq(page).locator(".seq-row").count();
  expect(painted).toBeGreaterThan(0);
  expect(painted).toBeLessThan(150);

  // The scrollbar still describes the whole log, not the window.
  const height = await seq(page).evaluate((el) => el.scrollHeight);
  expect(height).toBeGreaterThan(1100 * 26 * 0.9);

  // **Windowed, not truncated.** Without this a view that painted the first
  // forty rows and dropped the rest would pass everything above.
  await seq(page).evaluate((el) => {
    el.scrollTop = el.scrollHeight;
  });
  await expect(seq(page).locator(".seq-sql").last()).toContainText("n = 1099");
  await expect(seq(page).locator(".seq-row").count()).resolves.toBeLessThan(150);
});

/** Nothing has run: a sentence, not an empty box. */
test("an untouched tab says nothing has run in it", async ({ page }) => {
  await open(page);
  await toSequence(page);
  await expect(seq(page).locator(".seq-empty")).toContainText("Nothing has run in this tab yet");
  await expect(seq(page).locator(".seq-row")).toHaveCount(0);
});
