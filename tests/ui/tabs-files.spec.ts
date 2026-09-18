import { expect, test } from "@playwright/test";
import { commandNames, connect, editorText, installBackend } from "./harness";

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

/** Matches `OpenedFile` exactly — a wrong shape here fails as a UI mystery. */
const FILE = {
  path: "/home/u/queries/report.sql",
  name: "report.sql",
  contents: "SELECT 1;\n",
  sizeBytes: 10,
  dialect: "mysql",
  encoding: "utf-8",
  lineEnding: "lf",
  mtimeMs: 1_700_000_000_000,
  large: false,
};

// ------------------------------------------------------------------- tabs

test("the + button adds a tab; each is numbered per connection", async ({ page }) => {
  await connect(page);
  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
  await expect(page.locator("#script-tabs .stab.active .stab-label")).toHaveText("Untitled-1");

  await page.click(".stab-add");
  await expect(page.locator("#script-tabs .stab")).toHaveCount(2);
  await expect(page.locator("#script-tabs .stab.active .stab-label")).toHaveText("Untitled-2");
});

test("closing a clean tab needs no confirmation and releases it in the backend", async ({ page }) => {
  await connect(page);
  await page.click(".stab-add");
  await expect(page.locator("#script-tabs .stab")).toHaveCount(2);

  await page.locator("#script-tabs .stab.active .stab-mark").click();
  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
  expect(await commandNames(page)).toContain("close_tab");
});

test("an edited tab is marked dirty", async ({ page }) => {
  await connect(page);
  await expect(page.locator("#script-tabs .stab.dirty")).toHaveCount(0);

  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("SELECT 1");
  await expect(page.locator("#script-tabs .stab.dirty")).toHaveCount(1);
  await expect(page.locator("#script-tabs .stab .stab-mark")).toHaveAttribute(
    "title",
    /Unsaved changes/,
  );
});

/**
 * Closing a dirty tab must ask — and through our own dialog, never
 * `window.confirm`, which Tauri intercepts.
 */
test("closing a dirty tab asks first, and Cancel keeps it", async ({ page }) => {
  await connect(page);
  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("SELECT 1");

  await page.locator("#script-tabs .stab.active .stab-mark").click();
  await expect(page.locator("dialog.ask")).toBeVisible();

  await page.locator('dialog.ask button:has-text("Cancel")').click();
  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
});

test("discarding a dirty tab closes it", async ({ page }) => {
  await connect(page);
  await page.click(".stab-add");
  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("SELECT 1");

  await page.locator("#script-tabs .stab.active .stab-mark").click();
  await page.locator('dialog.ask button:has-text("Discard")').click();
  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
});

test("switching tabs restores each one's own text", async ({ page }) => {
  await connect(page);
  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("FIRST");

  await page.click(".stab-add");
  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("SECOND");
  expect(await editorText(page)).toContain("SECOND");

  await page.locator("#script-tabs .stab").first().click();
  expect(await editorText(page)).toContain("FIRST");
  expect(await editorText(page)).not.toContain("SECOND");
});

/** Colour is identification, not decoration: it answers "which server am I on". */
test("tabs carry the active connection's colour", async ({ page }) => {
  await connect(page);
  // The colour is set once on the bar and inherited, rather than per tab.
  const colour = await page
    .locator("#script-tabs")
    .evaluate((el) => getComputedStyle(el).getPropertyValue("--conn-colour").trim());
  expect(colour).toMatch(/^#|rgb/);
});

// ------------------------------------------------------------------ files

test("opening a file creates a tab titled after it", async ({ page }) => {
  await connect(page, {
    open_file_dialog: () => FILE,
    supported_file_types: () => [{ label: "SQL", extensions: ["sql"] }],
  });

  await page.keyboard.press("Control+o");
  await expect(page.locator('#script-tabs .stab .stab-label:has-text("report.sql")')).toBeVisible();
  expect(await editorText(page)).toContain("SELECT 1");
});

test("cancelling the open dialog changes nothing", async ({ page }) => {
  await connect(page, { open_file_dialog: () => null });
  const before = await page.locator("#script-tabs .stab").count();
  await page.keyboard.press("Control+o");
  await expect(page.locator("#script-tabs .stab")).toHaveCount(before);
});

test("saving a file-backed tab writes to its own path and clears the dirty mark", async ({ page }) => {
  await connect(page, {
    open_file_dialog: () => FILE,
    supported_file_types: () => [{ label: "SQL", extensions: ["sql"] }],
    save_file: () => ({ type: "saved", mtimeMs: 1_700_000_001_000 }),
  });

  await page.keyboard.press("Control+o");
  await page.locator("#editor .cm-content").click();
  await page.keyboard.type(" -- edited");
  await expect(page.locator("#script-tabs .stab.dirty")).toHaveCount(1);

  await page.keyboard.press("Control+s");
  await expect(page.locator("#script-tabs .stab.dirty")).toHaveCount(0);
  expect(await commandNames(page)).toContain("save_file");
});

/** An untitled tab has no path, so Save must become Save As. */
test("saving an untitled tab goes through the save dialog", async ({ page }) => {
  await connect(page, {
    save_file_dialog: () => ({
      path: "/home/u/new.sql",
      name: "new.sql",
      mtimeMs: 1,
      lineEnding: "lf",
      encoding: "utf-8",
      dialect: "mysql",
    }),
  });

  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("SELECT 1");
  await page.keyboard.press("Control+s");

  expect(await commandNames(page)).toContain("save_file_dialog");
  await expect(page.locator('#script-tabs .stab .stab-label:has-text("new.sql")')).toBeVisible();
});

/**
 * A file that changed on disk since it was opened must not be clobbered
 * silently — the whole reason `save_file` carries an mtime.
 */
test("a conflicting save asks rather than overwriting", async ({ page }) => {
  await connect(page, {
    open_file_dialog: () => FILE,
    supported_file_types: () => [{ label: "SQL", extensions: ["sql"] }],
    save_file: () => ({ type: "conflict" }),
  });

  await page.keyboard.press("Control+o");
  await page.locator("#editor .cm-content").click();
  await page.keyboard.type(" -- edited");
  await page.keyboard.press("Control+s");

  await expect(page.locator("dialog.ask")).toBeVisible();
  await expect(page.locator("dialog.ask h2")).toHaveText(/changed on disk/i);
  await expect(page.locator("dialog.ask p")).toContainText(/modified since you opened it/i);
  // Overwrite must not be the easy default when it discards someone's work.
  await expect(page.locator("dialog.ask button.primary")).toHaveText("Cancel");
});

// -------------------------------------------------- connection dialog rules

test("remember-password is only offered for a connection being saved", async ({ page }) => {
  await installBackend(page);
  await page.goto("/");
  await page.click("#btn-connect");

  await expect(page.locator("#conn-remember-row")).toBeVisible();
  await page.uncheck("#conn-save");
  await expect(page.locator("#conn-remember-row")).toBeHidden();
  // And unticking Save must not leave a stale "remember" behind it.
  await expect(page.locator("#conn-remember")).not.toBeChecked();
});

test("an ad-hoc connection is not written down", async ({ page }) => {
  await installBackend(page, { connect: () => ({ id: "c1", serverVersion: "8.4.0", databases: ["poc"], currentDatabase: null }) });
  await page.goto("/");
  await page.click("#btn-connect");
  await page.uncheck("#conn-save");
  await page.click("#conn-ok");

  await expect(page.locator(".rail-item.adhoc")).toHaveCount(1);
  expect(await commandNames(page)).not.toContain("save_profile");
});

test("Cancel closes the connection dialog without connecting", async ({ page }) => {
  await installBackend(page);
  await page.goto("/");
  await page.click("#btn-connect");
  await page.click("#conn-cancel");
  await expect(page.locator("#conn-dialog")).toBeHidden();
  expect(await commandNames(page)).not.toContain("connect");
});

test("duplicating a connection opens the editor with a new name and no password", async ({ page }) => {
  await connect(page);
  await page.locator(".rail-item").click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Duplicate")').click();

  await expect(page.locator("#conn-dialog")).toBeVisible();
  await expect(page.locator('#conn-form input[name="name"]')).toHaveValue(/copy/);
  await expect(page.locator('#conn-form input[name="password"]')).toHaveValue("");
});

// ---------------------------------------------------- many tabs

/**
 * **"When opening too many tabs there's no way to scroll."** The strip hides
 * its own scrollbar to stay one line tall, so past the edge there was nothing
 * to click: a wheel scrolled it, but nothing said so, the newest tab opened
 * out of sight, and `+` scrolled away with the tabs.
 */
async function openMany(page: import("@playwright/test").Page, n: number) {
  for (let i = 0; i < n; i++) await page.click(".stab-add");
  await expect(page.locator("#script-tabs .stab")).toHaveCount(n + 1);
}

/** Is `el` wholly inside the strip's visible area? */
async function inView(page: import("@playwright/test").Page, selector: string) {
  return page.evaluate((sel) => {
    const bar = document.getElementById("script-tabs")!.getBoundingClientRect();
    const el = document.querySelector(sel)!.getBoundingClientRect();
    return el.left >= bar.left - 1 && el.right <= bar.right + 1;
  }, selector);
}

test("the scroll arrows appear only when the tabs overflow", async ({ page }) => {
  await connect(page);
  await expect(page.locator("#stab-left")).toBeHidden();
  await expect(page.locator("#stab-right")).toBeHidden();

  await openMany(page, 25);
  await expect(page.locator("#stab-left")).toBeVisible();
  await expect(page.locator("#stab-right")).toBeVisible();
});

test("a new tab is scrolled into view, and + stays reachable", async ({ page }) => {
  await connect(page);
  await openMany(page, 25);

  await expect(page.locator("#script-tabs .stab.active .stab-label")).toHaveText("Untitled-26");
  expect(await inView(page, "#script-tabs .stab.active"), "the tab just opened is off-screen").toBe(true);
  expect(await inView(page, ".stab-add"), "+ scrolled away with the tabs").toBe(true);
  // At the far end, so only the way back is offered.
  await expect(page.locator("#stab-right")).toBeDisabled();
  await expect(page.locator("#stab-left")).toBeEnabled();
});

test("the arrows walk the strip back to the first tab and forward again", async ({ page }) => {
  await connect(page);
  await openMany(page, 25);
  const first = "#script-tabs .stab:first-child";
  expect(await inView(page, first)).toBe(false);

  for (let i = 0; i < 20 && !(await inView(page, first)); i++) {
    await page.click("#stab-left");
    await page.waitForTimeout(250);
  }
  expect(await inView(page, first), "the arrows never reached the first tab").toBe(true);
  await expect(page.locator("#stab-left")).toBeDisabled();

  await page.click("#stab-right");
  await expect
    .poll(() => page.evaluate(() => document.getElementById("script-tabs")!.scrollLeft))
    .toBeGreaterThan(0);
});

/** Scrolling to look must not be undone by the strip re-rendering. */
test("scrolling away is not snapped back by a re-render", async ({ page }) => {
  await connect(page);
  await openMany(page, 25);
  await page.evaluate(() => {
    document.getElementById("script-tabs")!.scrollLeft = 0;
  });
  // Typing marks the active tab dirty, which re-renders the strip.
  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("x");
  await expect(page.locator("#script-tabs .stab.active")).toHaveClass(/dirty/);
  expect(await page.evaluate(() => document.getElementById("script-tabs")!.scrollLeft)).toBe(0);
});
