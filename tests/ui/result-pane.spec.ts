import { expect, test, type Page } from "@playwright/test";
import { calls, connect, rowsResult } from "./harness";

/**
 * Stage 26: the result pane's header, and collapsing from it.
 *
 * The pane took 55% of the window whether or not anything was in it, and
 * nothing gave the space back. One header now serves the SQL grid and Flyway's
 * output, and shutting it hands the row to the editor.
 *
 * What is only answerable here is the geometry: that the height survives a
 * collapse, that the grid paints the rows its new viewport can show, and that
 * a running job keeps its line while everything else goes.
 */

const ROWS = Array.from({ length: 400 }, (_, i) => [String(i + 1), `name-${i}`]);

async function live(page: Page) {
  await connect(page, {
    statement_at_cursor: () => ({ sql: "SELECT 1", start: 0 }),
    run_script: () => rowsResult([{ name: "id", sqlType: "INT" }, { name: "name" }], ROWS),
  });
}

async function runQuery(page: Page) {
  await page.locator(".cm-content").click();
  await page.keyboard.type("SELECT 1");
  await page.keyboard.press("Control+Enter");
  await expect(page.locator("#grid tbody tr")).not.toHaveCount(0);
}

/** The editor's share of the window, in px, as the layout actually resolves. */
async function editorHeight(page: Page) {
  return page.evaluate(() => document.getElementById("editor-pane")!.getBoundingClientRect().height);
}

async function paneHeight(page: Page) {
  return page.evaluate(() =>
    document.getElementById("results-pane")!.getBoundingClientRect().height,
  );
}

// ------------------------------------------------------------- the header

test("the header collapses the pane and gives the row to the editor", async ({ page }) => {
  await live(page);
  await runQuery(page);

  const editorBefore = await editorHeight(page);
  const paneBefore = await paneHeight(page);
  expect(paneBefore).toBeGreaterThan(80);

  await page.click("#res-toggle");

  await expect(page.locator("#results-body")).toBeHidden();
  await expect(page.locator("#res-toggle")).toHaveAttribute("aria-expanded", "false");
  expect(await editorHeight(page)).toBeGreaterThan(editorBefore);
  // Shut is the header's own height, not a sliver of grid.
  const shut = await paneHeight(page);
  expect(shut).toBeLessThan(44);
  expect(shut).toBeGreaterThan(20);

  await page.click("#res-toggle");
  await expect(page.locator("#results-body")).toBeVisible();
});

/**
 * **The property the whole shape rests on.** Collapsing must not overwrite the
 * height somebody dragged to — `--editor-h` is left alone and the row template
 * changes instead, so reopening lands where they left it.
 */
test("the size you dragged to comes back when you reopen", async ({ page }) => {
  await live(page);
  await runQuery(page);

  // Drag the seam well away from the default, as a person sizing the pane does.
  const seam = page.locator("#hsplit");
  const box = (await seam.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2, box.y - 120, { steps: 8 });
  await page.mouse.up();

  const sized = await paneHeight(page);
  expect(sized).toBeGreaterThan(80);

  await page.click("#res-toggle");
  await page.click("#res-toggle");

  // The same height, not the 45% default it started at.
  expect(Math.abs((await paneHeight(page)) - sized)).toBeLessThan(2);
});

/**
 * The grid paints only the rows its viewport can show, and `paintRows` ran on
 * scroll alone — so a pane that GREW painted nothing new, and the bottom of a
 * tall one was blank until you touched the wheel. Measured before the fix:
 * 347px and 585px viewports both painted 42 rows.
 *
 * **Asserted by growing the pane, not by collapsing it.** A collapse ends on
 * the viewport it started from, so the painted count is identical with the
 * observer or without it — a test built on that would prove nothing.
 */
test("the grid paints the rows a bigger pane can show", async ({ page }) => {
  await live(page);
  await runQuery(page);

  const painted = () => page.locator("#grid tbody tr").count();
  const seam = page.locator("#hsplit");
  const drag = async (dy: number) => {
    const box = (await seam.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2, box.y + dy, { steps: 8 });
    await page.mouse.up();
  };

  // A small pane first, so the painted set is small.
  await drag(200);
  const small = await painted();

  // Then a big one. Nothing scrolls; only the viewport changes.
  await drag(-320);
  await expect.poll(painted).toBeGreaterThan(small);
});

/** A seam that looks draggable and does nothing is the worst of the states. */
test("dragging the seam while it is shut opens it", async ({ page }) => {
  await live(page);
  await runQuery(page);
  await page.click("#res-toggle");
  await expect(page.locator("#results-body")).toBeHidden();

  const box = (await page.locator("#hsplit").boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2, box.y - 150, { steps: 8 });
  await page.mouse.up();

  await expect(page.locator("#results-body")).toBeVisible();
  expect(await paneHeight(page)).toBeGreaterThan(80);
});

test("Ctrl+J toggles the pane", async ({ page }) => {
  await live(page);
  await runQuery(page);

  await page.keyboard.press("Control+j");
  await expect(page.locator("#results-body")).toBeHidden();
  await page.keyboard.press("Control+j");
  await expect(page.locator("#results-body")).toBeVisible();
});

// ------------------------------------------------------- what it says shut

/**
 * The summary is read back off the status chips rather than composed twice, so
 * it cannot disagree with the bar underneath it — and it only appears while the
 * pane is shut, where nothing else is saying it.
 */
test("shut, the header says what the status bar would have", async ({ page }) => {
  await live(page);
  await runQuery(page);

  const summary = page.locator("#res-summary");
  await expect(summary).toBeHidden();

  await page.click("#res-toggle");
  await expect(summary).toBeVisible();
  await expect(summary).toContainText("400 rows");
  await expect(summary).toContainText("ms");
});

/** With no result at all, the sentence the grid would show is the summary. */
test("shut with nothing run, the header carries the sentence", async ({ page }) => {
  await live(page);
  await page.click("#res-toggle");
  await expect(page.locator("#res-summary")).toContainText("No results yet");
});

/**
 * **Results arriving do not reopen it.** Collapsing was an explicit act, and a
 * run that undid it would make the gesture useless to the person who wants it
 * most — somebody iterating on a long script. The header says what landed.
 */
test("a run while it is shut leaves it shut, and says what landed", async ({ page }) => {
  await live(page);
  await runQuery(page);
  await page.click("#res-toggle");

  await page.locator(".cm-content").click();
  await page.keyboard.press("Control+Enter");

  // It ran — and the pane is still shut.
  await expect.poll(async () => (await calls(page)).filter((c) => c.cmd === "run_script").length)
    .toBeGreaterThan(1);
  await expect(page.locator("#results-body")).toBeHidden();
  await expect(page.locator("#res-summary")).toContainText("400 rows");
  // Marked for a moment, so "did that do anything?" has an answer on screen.
  await expect(page.locator("#res-toggle")).toHaveClass(/landed/);
});

/**
 * A job's progress is the one thing worth keeping while the pane is shut: it is
 * why somebody collapses it in the first place — to carry on working while an
 * export or a migrate runs.
 */
test("a running job keeps its line while the pane is shut", async ({ page }) => {
  await live(page);
  await runQuery(page);

  // Raise the strip the way a long job does, then shut the pane around it.
  await page.evaluate(() => {
    const strip = document.getElementById("job-strip")!;
    strip.hidden = false;
    strip.textContent = "Exporting poc · 2 of 6 tables";
  });
  await page.click("#res-toggle");

  await expect(page.locator("#results-body")).toBeHidden();
  await expect(page.locator("#job-strip")).toBeVisible();
  await expect(page.locator("#job-strip")).toContainText("2 of 6");
});
