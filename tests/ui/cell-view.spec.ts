import { expect, test, type Page } from "@playwright/test";
import { calls, connect, rowsResult } from "./harness";

/**
 * Right-click a cell -> "Open in full view".
 *
 * The grid ellipsises every cell, so before this there was no way to read a
 * JSON document or a long paragraph at all — the value was on screen and
 * unreachable. JSON gets a formatter because that is the case that hurts most.
 */

const JSON_DOC = '{"id":7,"tags":["a","b"],"nested":{"deep":true}}';
const LONG_TEXT = `not json ${"x".repeat(4000)}`;

async function openViewer(page: Page, cell: string) {
  await page.locator(cell).click({ button: "right" });
  await expect(page.locator(".ctx-menu")).toBeVisible();
  await page.locator(".ctx-menu button", { hasText: "Open in full view" }).click();
  await expect(page.locator("dialog.viewer")).toBeVisible();
}

async function show(page: Page, rows: unknown[][], cols = [{ name: "v" }]) {
  await connect(page, { run_script: () => rowsResult(cols, rows) });
  await page.click("#btn-run-all");
  await expect(page.locator("table.rs")).toBeVisible();
}

test("a cell's whole value is reachable, formatted when it is JSON", async ({ page }) => {
  await show(page, [[JSON_DOC]]);
  await openViewer(page, 'td[data-col="0"]');

  const body = page.locator(".viewer-body");
  // Formatted by default: indented across several lines, not the one-liner.
  await expect(body).toContainText('"tags": [');
  expect((await body.innerText()).split("\n").length).toBeGreaterThan(5);
  await expect(page.locator(".viewer-meta")).toContainText("JSON, formatted");
});

test("Raw shows the bytes exactly as stored, and toggles back", async ({ page }) => {
  await show(page, [[JSON_DOC]]);
  await openViewer(page, 'td[data-col="0"]');

  await page.locator("dialog.viewer menu button", { hasText: "Raw" }).click();
  expect(await page.locator(".viewer-body").innerText()).toBe(JSON_DOC);
  await expect(page.locator(".viewer-meta")).toContainText("JSON, raw");

  await page.locator("dialog.viewer menu button", { hasText: "Format" }).click();
  await expect(page.locator(".viewer-body")).toContainText('"tags": [');
});

/** Offering to "format" a plain string would be noise on most columns. */
test("a value that is not a JSON document gets no format button", async ({ page }) => {
  await show(page, [["just a string"]]);
  await openViewer(page, 'td[data-col="0"]');
  await expect(page.locator("dialog.viewer menu button", { hasText: /Raw|Format/ })).toHaveCount(0);
  await expect(page.locator(".viewer-body")).toHaveText("just a string");
});

/** A bare number parses as JSON but formatting it is meaningless. */
test("a bare number is not treated as a JSON document", async ({ page }) => {
  await show(page, [["12345"]]);
  await openViewer(page, 'td[data-col="0"]');
  await expect(page.locator("dialog.viewer menu button", { hasText: /Raw|Format/ })).toHaveCount(0);
});

test("the long value scrolls inside the dialog, which stays on screen", async ({ page }) => {
  await show(page, [[LONG_TEXT]]);
  await openViewer(page, 'td[data-col="0"]');

  const m = await page.evaluate(() => {
    const dlg = document.querySelector("dialog.viewer") as HTMLElement;
    const body = document.querySelector(".viewer-body") as HTMLElement;
    return {
      vh: window.innerHeight,
      dialogH: dlg.getBoundingClientRect().height,
      scrollable: body.scrollHeight > body.clientHeight,
    };
  });
  expect(m.dialogH).toBeLessThanOrEqual(m.vh);
  expect(m.scrollable).toBe(true);
});

test("the whole value is present, not the truncated cell text", async ({ page }) => {
  await show(page, [[LONG_TEXT]]);
  await openViewer(page, 'td[data-col="0"]');
  expect((await page.locator(".viewer-body").innerText()).length).toBe(LONG_TEXT.length);
  await expect(page.locator(".viewer-meta")).toContainText("characters");
});

/** A real NULL must not be confused with the four-character string. */
test("a NULL says it is a NULL", async ({ page }) => {
  await show(page, [[null]]);
  await openViewer(page, 'td[data-col="0"]');
  await expect(page.locator(".viewer-meta")).toContainText("SQL NULL");
  await expect(page.locator(".viewer-body")).toHaveClass(/is-null/);
});

test("Escape closes it", async ({ page }) => {
  await show(page, [["v"]]);
  await openViewer(page, 'td[data-col="0"]');
  await page.keyboard.press("Escape");
  await expect(page.locator("dialog.viewer")).toHaveCount(0);
});

/**
 * Right-clicking to inspect one value must not throw away a selection someone
 * just built in order to copy it.
 */
test("right-click leaves the selection alone", async ({ page }) => {
  await show(page, [["a1", "b1"], ["a2", "b2"]], [{ name: "a" }, { name: "b" }]);
  await page.locator('th[data-col="0"]').click();
  await expect(page.locator("#btn-copy")).toHaveText(/1 column/);

  await page.locator('td[data-col="1"]').first().click({ button: "right" });
  await expect(page.locator(".ctx-menu")).toBeVisible();
  await expect(page.locator("#btn-copy")).toHaveText(/1 column/);
});

/** This project's standing rule: nothing executes unless the user asked it to. */
test("the viewer never runs anything", async ({ page }) => {
  await show(page, [[JSON_DOC]]);
  await openViewer(page, 'td[data-col="0"]');
  await page.locator("dialog.viewer menu button", { hasText: "Raw" }).click();
  const ran = (await calls(page)).filter((c) => c.cmd === "run_script");
  expect(ran).toHaveLength(1); // only the Run all we asked for
});
