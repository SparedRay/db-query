// The schema tree's quick filter.
//
// The invariant worth defending is the last test here: filtering is
// presentation only, so clearing the box must give back exactly the tree that
// was there before — same expansion, same collapsed groups. The obvious
// implementation (setting `hidden` along the path to each match) fails that the
// moment anything else touches the tree.

import { expect, test } from "@playwright/test";
import { READ_ONLY_CAPS, connect, openDatabase } from "./harness";

/** The Tables group opens by itself — clicking it would *collapse* it. */
async function treeWithTables(page: any) {
  await connect(page);
  await openDatabase(page);
  await expect(page.locator('.node.table:has-text("users")')).toBeVisible();
}

test("filtering hides what does not match and keeps the path to what does", async ({ page }) => {
  await treeWithTables(page);
  // **`total` and not `user`**: since Stage 29.2 a table is kept by its
  // columns too, and `orders.user_id` would keep `orders` — correctly, which
  // is a different test's job. This needle matches one table's name and
  // nothing else's columns.
  await page.fill("#tree-filter", "user");

  await expect(page.locator('.node.table[data-table="users"]')).toBeVisible();
  // `big` matches nothing — not its name, and not a column of it.
  await expect(page.locator('.node.table[data-table="big"]')).toBeHidden();
  // The ancestors of a match stay, or the match has nowhere to sit.
  await expect(page.locator(".node.db")).toBeVisible();
  await expect(page.locator('.node.group:has-text("Tables")')).toBeVisible();
  // A group with nothing under it goes.
  await expect(page.locator('.node.group:has-text("Procedures")')).toBeHidden();
});

test("the count says how many of how many", async ({ page }) => {
  await treeWithTables(page);
  // `users` by name, `orders` by its `user_id` column, and the `user_totals`
  // view through a group nobody has expanded.
  await page.fill("#tree-filter", "user");
  await expect(page.locator("#tree-filter-count")).toHaveText(/^3 of \d+$/);

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
  await expect(page.locator('.node.table[data-table="big"]')).toBeHidden();

  await page.locator("#tree-filter").press("Escape");
  await expect(page.locator("#tree-filter")).toHaveValue("");
  await expect(page.locator('.node.table[data-table="big"]')).toBeVisible();
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
  await expect(page.locator('.node.table[data-table="users"]')).toBeVisible();
  await expect(page.locator('.node.table[data-table="big"]')).toBeHidden();
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

// ------------------------------------------------- Stage 29.2: the columns

/**
 * **Columns were never searched at all.** Not "only the loaded ones" — none of
 * them, loaded or not: `buildColumnNode` returns a bare `.node`, and the walk
 * read every child of a `.children` as a wrapper, so it took a column's
 * `.twisty` for its node and recursed into spans. Typing a column name over an
 * expanded table said "none of 8" with the match on screen.
 */
test("a column of an expanded table is a match in its own right", async ({ page }) => {
  await treeWithTables(page);
  await page.locator('.node.table[data-table="users"]').click();
  await expect(page.locator('.node.column:has-text("display_name")')).toBeVisible();

  await page.fill("#tree-filter", "display_name");
  await expect(page.locator(".node.column.filter-hit")).toHaveCount(1);
  await expect(page.locator(".node.column.filter-hit")).toContainText("display_name");
  // The other columns of the same table go, as any non-matching node does.
  await expect(page.locator('.node.column:has-text("email")')).toBeHidden();
  await expect(page.locator("#tree-filter-count")).toHaveText(/^1 of \d+$/);
});

/**
 * And a column of a table nobody has expanded, which is the ask: Stage 27 holds
 * the whole server's columns in memory, so this costs a substring scan rather
 * than the round trip the old doc said it would.
 *
 * The table is marked rather than opened — drawing its columns would make the
 * filter decide what is loaded, which is the one thing this module refuses.
 */
test("a column of an unexpanded table marks its table, with a count", async ({ page }) => {
  await treeWithTables(page);
  const orders = page.locator('.node.table[data-table="orders"]');
  await expect(page.locator('.node.column:has-text("user_id")')).toHaveCount(0);

  await page.fill("#tree-filter", "user_id");
  await expect(orders).toBeVisible();
  await expect(orders).toHaveAttribute("data-filter-note", "1 col");
  // Not a hit itself: `orders` does not contain `user_id`, its column does.
  await expect(orders).not.toHaveClass(/filter-hit/);
  await expect(page.locator('.node.table[data-table="users"]')).toBeHidden();

  // Opening it brings the columns in already marked, because the tree's
  // observer re-runs the filter for whatever arrives.
  await orders.click();
  await expect(page.locator(".node.column.filter-hit")).toContainText("user_id");
  await expect(page.locator('.node.column:has-text("total")')).toBeHidden();
});

/** A database nobody has opened says how many are inside it. */
test("an unopened database says how many matches are inside", async ({ page }) => {
  await connect(page);
  const db = page.locator(".node.db");
  await expect(page.locator(".node.table")).toHaveCount(0);

  await page.fill("#tree-filter", "user");
  // `users` and `user_totals` by name, `orders.user_id` by column.
  await expect(db).toHaveAttribute("data-filter-note", "3 inside");

  // Opening it replaces the count with the real thing, and does not double it:
  // the same three, now attributed to the table and the group that hold them.
  await openDatabase(page);
  await expect(db).not.toHaveAttribute("data-filter-note", /.*/);
  await expect(page.locator("#tree-filter-count")).toHaveText(/^3 of \d+/);
});

/**
 * The invariant this module exists to protect: clearing gives back exactly the
 * tree that was there. The marks are the filter's own, so they go with it.
 */
test("clearing takes the counts away with it", async ({ page }) => {
  await treeWithTables(page);
  await page.fill("#tree-filter", "user_id");
  await expect(page.locator("[data-filter-note]")).toHaveCount(1);

  await page.fill("#tree-filter", "");
  await expect(page.locator("[data-filter-note]")).toHaveCount(0);
  await expect(page.locator('.node.table[data-table="big"]')).toBeVisible();
});

/**
 * A view, and a routine, in a group nobody has expanded.
 *
 * Both cost nothing: opening a database fetches `list_tables` and
 * `list_routines` together — the counts in the group labels come from exactly
 * that — so the names are in memory whether or not the group has ever been
 * opened. The first version of this left them out, reasoning that the *column*
 * prefetch does not record which kind a name is; the tree's own group closures
 * do.
 */
test("a collapsed group is searched from the names the tree already has", async ({ page }) => {
  await treeWithTables(page);
  // Neither group has ever been opened, so neither has a node in it.
  await expect(page.locator('.node.table[data-table="user_totals"]')).toHaveCount(0);
  await expect(page.locator(".node.routine")).toHaveCount(0);

  await page.fill("#tree-filter", "user_totals");
  const views = page.locator('.node.group[data-group="Views"]');
  await expect(views).toBeVisible();
  await expect(views).toHaveAttribute("data-filter-note", "1 inside");

  await page.fill("#tree-filter", "top_spenders");
  const procs = page.locator('.node.group[data-group="Procedures"]');
  await expect(procs).toBeVisible();
  await expect(procs).toHaveAttribute("data-filter-note", "1 inside");
  // And the groups that hold nothing matching go, as any other node does.
  await expect(views).toBeHidden();
  await expect(page.locator('.node.group[data-group="Tables"]')).toBeHidden();
});

/** Expanding it replaces the count with the real node, and does not double it. */
test("expanding a counted group replaces its count", async ({ page }) => {
  await treeWithTables(page);
  await page.fill("#tree-filter", "user_totals");
  const views = page.locator('.node.group[data-group="Views"]');
  await expect(views).toHaveAttribute("data-filter-note", "1 inside");
  await expect(page.locator("#tree-filter-count")).toHaveText(/^1 of \d+$/);

  await views.click();
  await expect(page.locator('.node.table[data-table="user_totals"]')).toBeVisible();
  await expect(views).not.toHaveAttribute("data-filter-note", /.*/);
  // Still one match, counted once: the group defers to its own nodes.
  await expect(page.locator("#tree-filter-count")).toHaveText(/^1 of \d+$/);
});

/**
 * A routine group is searched by name only. Its parameters are not in the
 * column prefetch, and asking for them would answer "never fetched" and report
 * a partial search that is nothing of the kind.
 */
test("a routine group reports no partial search of its own", async ({ page }) => {
  await treeWithTables(page);
  await page.fill("#tree-filter", "ping_poc");
  await expect(page.locator('.node.group[data-group="Procedures"]')).toHaveAttribute(
    "data-filter-note",
    "1 inside",
  );
  await expect(page.locator("#tree-filter-count")).toHaveText(/^1 of \d+$/);
});

/**
 * A database nobody has opened has had no `list_routines` run against it, so
 * its tables and columns are searchable and its routines were never read —
 * which the count says rather than implying a whole server was covered.
 */
test("an unopened database says the search covered what was loaded", async ({ page }) => {
  await connect(page);
  await page.fill("#tree-filter", "user");
  await expect(page.locator("#tree-filter-count")).toHaveText(/loaded only$/);
  // Opening it reads the routines, and the count stops hedging.
  await openDatabase(page);
  await expect(page.locator("#tree-filter-count")).not.toHaveText(/loaded only$/);
});

/**
 * An engine with no bulk column query keeps its mappings until asked (Stage 27,
 * decision 3), so the prefetch holds names and empty lists standing for
 * "not fetched". Reporting that as a complete search would be the lie; the
 * count says what it covered.
 */
test("without prefetched columns the count says it searched what was loaded", async ({ page }) => {
  await connect(page, {
    connect: () => ({
      id: "c1",
      serverVersion: null,
      databases: ["poc"],
      currentDatabase: "poc",
      capabilities: READ_ONLY_CAPS,
    }),
  });
  await openDatabase(page);
  await expect(page.locator('.node.table[data-table="users"]')).toBeVisible();

  await page.fill("#tree-filter", "user");
  await expect(page.locator("#tree-filter-count")).toHaveText(/loaded only$/);
  // The name still matches; the unexpanded columns are simply not claimed.
  await expect(page.locator('.node.table[data-table="users"]')).toBeVisible();
  await expect(page.locator('.node.table[data-table="orders"]')).toBeHidden();
});
