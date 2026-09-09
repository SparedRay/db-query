// The schema tree's quick filter.
//
// The invariant worth defending is the last test here: filtering is
// presentation only, so clearing the box must give back exactly the tree that
// was there before — same expansion, same collapsed groups. The obvious
// implementation (setting `hidden` along the path to each match) fails that the
// moment anything else touches the tree.

import { expect, test } from "@playwright/test";
import { connect, openDatabase } from "./harness";

/** The Tables group opens by itself — clicking it would *collapse* it. */
async function treeWithTables(page: any) {
  await connect(page);
  await openDatabase(page);
  await expect(page.locator('.node.table:has-text("users")')).toBeVisible();
}

test("filtering hides what does not match and keeps the path to what does", async ({ page }) => {
  await treeWithTables(page);
  await page.fill("#tree-filter", "user");

  await expect(page.locator('.node.table:has-text("users")')).toBeVisible();
  await expect(page.locator('.node.table:has-text("orders")')).toBeHidden();
  // The ancestors of a match stay, or the match has nowhere to sit.
  await expect(page.locator(".node.db")).toBeVisible();
  await expect(page.locator('.node.group:has-text("Tables")')).toBeVisible();
  // A group with nothing under it goes.
  await expect(page.locator('.node.group:has-text("Procedures")')).toBeHidden();
});

test("the count says how many of how many", async ({ page }) => {
  await treeWithTables(page);
  await page.fill("#tree-filter", "user");
  await expect(page.locator("#tree-filter-count")).toHaveText(/^1 of \d+$/);

  await page.fill("#tree-filter", "zzz-nothing");
  await expect(page.locator("#tree-filter-count")).toHaveText(/^none of \d+$/);
  await expect(page.locator("#tree-filter-count")).toHaveClass(/none/);
});

test("a match inside a collapsed group is revealed", async ({ page }) => {
  await treeWithTables(page);
  // Collapse it: the node still exists, but is not on screen.
  await page.locator('.node.group:has-text("Tables")').click();
  await expect(page.locator('.node.table:has-text("users")')).toBeHidden();

  await page.fill("#tree-filter", "users");
  await expect(page.locator('.node.table:has-text("users")')).toBeVisible();
});

test("clearing restores the tree exactly, collapsed groups included", async ({ page }) => {
  await treeWithTables(page);
  await page.locator('.node.group:has-text("Tables")').click();
  await expect(page.locator('.node.table:has-text("users")')).toBeHidden();

  await page.fill("#tree-filter", "users");
  await expect(page.locator('.node.table:has-text("users")')).toBeVisible();

  // The whole point: the group was collapsed before, and is collapsed after.
  await page.fill("#tree-filter", "");
  await expect(page.locator('.node.group:has-text("Tables")')).toBeVisible();
  await expect(page.locator('.node.table:has-text("users")')).toBeHidden();
});

test("Escape clears the filter", async ({ page }) => {
  await treeWithTables(page);
  await page.fill("#tree-filter", "user");
  await expect(page.locator('.node.table:has-text("orders")')).toBeHidden();

  await page.locator("#tree-filter").press("Escape");
  await expect(page.locator("#tree-filter")).toHaveValue("");
  await expect(page.locator('.node.table:has-text("orders")')).toBeVisible();
});

test("Ctrl+P puts the cursor in the filter", async ({ page }) => {
  await treeWithTables(page);
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+p");
  await expect(page.locator("#tree-filter")).toBeFocused();
});

test("children loaded while filtering are filtered too", async ({ page }) => {
  await connect(page);
  // Filter typed before the database is expanded, so its children arrive after.
  // Without the observer they would appear unfiltered inside a filtered tree.
  await page.fill("#tree-filter", "user");
  await openDatabase(page);
  await expect(page.locator('.node.table:has-text("users")')).toBeVisible();
  await expect(page.locator('.node.table:has-text("orders")')).toBeHidden();
});

/**
 * A filter that hides the only thing you can click has not focused the tree,
 * it has broken it. Typing before anything is expanded used to empty the panel
 * completely — including the database node that would have loaded the tables
 * the filter was looking for.
 */
test("a database stays visible even when nothing under it matches", async ({ page }) => {
  await connect(page);
  await page.fill("#tree-filter", "zzz-nothing-matches-this");
  await expect(page.locator(".node.db")).toBeVisible();

  // And it is still the working control it was: clicking it loads the tables,
  // which stay filtered out because they do not match — and are there the
  // instant the filter is cleared. (Clicked directly rather than through
  // `openDatabase`, which waits for a group the filter is hiding on purpose.)
  await page.locator(".node.db").click();
  await expect(page.locator('.node.group:has-text("Tables")')).toBeHidden();
  await page.fill("#tree-filter", "");
  await expect(page.locator('.node.table:has-text("users")')).toBeVisible();
});
