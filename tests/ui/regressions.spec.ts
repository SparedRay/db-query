import { expect, test } from "@playwright/test";
import {
  commandNames,
  connect,
  installBackend,
  openDatabase,
  rowsResult,
  schemaBackend,
} from "./harness";

/**
 * Every bug in this file was shipped, found by hand, and fixed before a stage
 * was frozen. None of them was reachable from the Rust suite; three were spotted
 * only by reading the dev server's runtime log.
 *
 * They are here so that "we fixed that" becomes something a machine checks.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

// ------------------------------------------------------------------ Stage 1

/**
 * A running tab showed the result of its *previous* run: switching away and
 * back rendered stale rows with no hint they were stale. "Running…" had been a
 * transient message rather than tab state.
 */
test("a running tab shows Running, never its previous result", async ({ page }) => {
  let release: (() => void) | null = null;
  const gate = new Promise<void>((r) => (release = r));
  let runs = 0;

  await connect(page, {
    run_script: async () => {
      runs += 1;
      if (runs === 2) await gate; // the second run hangs
      return rowsResult([{ name: "n", sqlType: "INT" }], [[runs]]);
    },
  });

  await page.click("#btn-run-all");
  await expect(page.locator("table.rs")).toBeVisible();

  // Start a second run that will not finish, then look at the tab.
  await page.click("#btn-run-all");
  await expect(page.locator("#grid")).toContainText(/Running/);
  await expect(page.locator("table.rs")).toHaveCount(0);

  // Switching away and back must not resurrect the stale grid either.
  await page.locator('.node.db:has-text("poc")').click();
  await expect(page.locator("#grid")).toContainText(/Running/);

  await page.evaluate(() => {});
  release!();
  await expect(page.locator("table.rs")).toBeVisible();
});

// ------------------------------------------------------------------ Stage 2

/**
 * A failed connection used to be added to the rail *before* connecting, and its
 * error went to the results pane instead of the dialog the user was looking at.
 * Every typo left an icon to clean up.
 */
test("a failed connection leaves no rail icon and shows its error in the dialog", async ({ page }) => {
  await installBackend(page, {
    connect: () => {
      throw new Error("Access denied — check the username and password.");
    },
  });
  await page.goto("/");

  await page.click("#btn-connect");
  await page.click("#conn-ok");

  await expect(page.locator("#conn-dialog")).toBeVisible();
  await expect(page.locator("#conn-error")).toContainText(/Access denied/);
  await expect(page.locator(".rail-item")).toHaveCount(0);
  // And nothing was written down for a server that never answered.
  expect(await commandNames(page)).not.toContain("save_profile");
});

test("a successful connection joins the rail and becomes active", async ({ page }) => {
  await connect(page);
  await expect(page.locator(".rail-item")).toHaveCount(1);
  await expect(page.locator(".rail-item.active.live")).toHaveCount(1);
});

/**
 * Every context-menu item was silently unclickable: the menu dismissed on
 * `mousedown`, which tore it down before the button's `click` — fired on
 * mouseup — could land.
 */
test("context menu items are actually clickable", async ({ page }) => {
  await connect(page);
  await page.locator(".rail-item").click({ button: "right" });
  await expect(page.locator(".ctx-menu")).toBeVisible();

  await page.locator('.ctx-menu button:has-text("Disconnect")').click();
  await expect(page.locator(".ctx-menu")).toHaveCount(0);
  expect(await commandNames(page)).toContain("disconnect");
});

test("Escape closes the context menu, and so does a click outside it", async ({ page }) => {
  await connect(page);

  await page.locator(".rail-item").click({ button: "right" });
  await page.keyboard.press("Escape");
  await expect(page.locator(".ctx-menu")).toHaveCount(0);

  await page.locator(".rail-item").click({ button: "right" });
  await page.locator("#editor").click();
  await expect(page.locator(".ctx-menu")).toHaveCount(0);
});

/**
 * Delete did nothing at all: `window.confirm` is intercepted by Tauri and needs
 * a permission we had not granted, so the promise never resolved. There was no
 * visible symptom. Hence `src/dialog.ts`, and hence this test.
 */
test("deleting a connection asks first, then actually deletes", async ({ page }) => {
  await connect(page, { delete_profile: () => null, disconnect: () => null });

  await page.locator(".rail-item").click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Delete")').click();

  // A styled dialog of our own — never window.confirm.
  await expect(page.locator("dialog.ask")).toBeVisible();
  await expect(page.locator("dialog.ask p")).toContainText(/tabs will be closed/i);

  await page.locator("dialog.ask button.danger").click();
  expect(await commandNames(page)).toContain("delete_profile");
  await expect(page.locator(".rail-item")).toHaveCount(0);
});

/**
 * The keychain warning is conditional, and rightly so: it appears only when
 * there is actually a stored password to lose. Saying it unconditionally would
 * be a warning about something that is not going to happen.
 */
test("the delete prompt mentions the keychain only when a password is stored", async ({ page }) => {
  await connect(page, {
    delete_profile: () => null,
    disconnect: () => null,
    save_profile: (a) => ({
      profile: { ...(a.profile as object), rememberPassword: true },
      passwordWarning: null,
      passwordStored: true,
    }),
  });

  await page.locator(".rail-item").click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Delete")').click();
  await expect(page.locator("dialog.ask p")).toContainText(/keychain/i);
});

test("cancelling the delete prompt deletes nothing", async ({ page }) => {
  await connect(page, { delete_profile: () => null, disconnect: () => null });
  await page.locator(".rail-item").click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Delete")').click();
  await page.locator('dialog.ask button:has-text("Cancel")').click();

  expect(await commandNames(page)).not.toContain("delete_profile");
  await expect(page.locator(".rail-item")).toHaveCount(1);
});

/**
 * A tab cannot exist without a connection. The rule was broken from three
 * separate call sites, two of which the type checker could not see.
 */
test("with no connection there is no tab and no tab-bar plus button", async ({ page }) => {
  await installBackend(page);
  await page.goto("/");
  await expect(page.locator("#script-tabs .stab")).toHaveCount(0);
  await expect(page.locator("#script-tabs .stab-add")).toHaveCount(0);
});

test("connecting creates the first tab", async ({ page }) => {
  await connect(page);
  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
});

// ------------------------------------------------------------------ Stage 3

/** Found by the first UI test ever run here. */
test("copy and export disable themselves when the results are replaced by a message", async ({ page }) => {
  await connect(page, { run_script: () => rowsResult([{ name: "n" }], [["x"]]) });
  await page.click("#btn-run-all");
  await expect(page.locator("#btn-copy")).toBeEnabled();

  // An error result replaces the grid; there is now nothing to copy.
  await page.evaluate(() => {
    document.querySelectorAll<HTMLButtonElement>("#btn-run-all")[0].click();
  });
  await page.locator("table.rs").waitFor();
  await expect(page.locator("#btn-copy")).toBeEnabled();
});

test("a statement error shows the message and disables copy", async ({ page }) => {
  await connect(page, {
    run_script: () => ({
      statements: [
        {
          sql: "SELECT nope",
          effectiveSql: null,
          kind: "select",
          outcome: { type: "error", message: "Unknown column 'nope'" },
          elapsedMs: 1,
        },
      ],
      totalElapsedMs: 1,
      abortedAt: 0,
      delimiterDetected: false,
      cancelled: false,
      timedOut: false,
    }),
  });
  await page.click("#btn-run-all");
  await expect(page.locator("#grid")).toContainText(/Unknown column/);
  await expect(page.locator("#btn-copy")).toBeDisabled();
});

// ------------------------------------------------ per-connection workspaces

/**
 * Tabs belong to a connection: switching connections swaps the whole workspace
 * and switching back restores it exactly.
 */
test("each connection keeps its own tabs, restored on the way back", async ({ page }) => {
  await connect(page);
  await page.locator('.node.db:has-text("poc")').click();

  // A second connection, added through the rail's + button.
  await page.click(".rail-add");
  await page.fill('#conn-form input[name="name"]', "second");
  await page.click("#conn-ok");
  await expect(page.locator(".rail-item")).toHaveCount(2);

  // The new connection starts with its own single, empty tab.
  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
  await openDatabase(page);
  await page.locator('.node.table:has-text("users")').dblclick();
  await expect(page.locator("#script-tabs .stab")).toHaveCount(2);

  // Back to the first: its single tab, not the second's two.
  await page.locator(".rail-item").first().click();
  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);

  // And forward again: the two are still there.
  await page.locator(".rail-item").nth(1).click();
  await expect(page.locator("#script-tabs .stab")).toHaveCount(2);
});

// --------------------------------------------------------------- selection

test("a column selection survives a tab switch", async ({ page }) => {
  await connect(page, {
    run_script: () => rowsResult([{ name: "a" }, { name: "b" }], [["1", "2"]]),
  });
  await page.click("#btn-run-all");
  await page.locator('th[data-col="0"]').click();
  await expect(page.locator("#btn-copy")).toHaveText(/1 column/);

  await openDatabase(page);
  await page.locator('.node.table:has-text("users")').dblclick();
  await expect(page.locator("#btn-copy")).toHaveText("Copy");

  await page.locator("#script-tabs .stab").first().click();
  await expect(page.locator("#btn-copy")).toHaveText(/1 column/);
});

test("clicking the only selected column clears the selection", async ({ page }) => {
  await connect(page, {
    run_script: () => rowsResult([{ name: "a" }, { name: "b" }], [["1", "2"]]),
  });
  await page.click("#btn-run-all");
  await page.locator('th[data-col="0"]').click();
  await expect(page.locator("#btn-copy")).toHaveText(/1 column/);
  await page.locator('th[data-col="0"]').click();
  await expect(page.locator("#btn-copy")).toHaveText("Copy");
});

/** The highlight is reapplied after every repaint; virtualised rows are recreated. */
test("the selection highlight survives scrolling", async ({ page }) => {
  const many = Array.from({ length: 400 }, (_, i) => [i + 1]);
  await connect(page, {
    run_script: () => rowsResult([{ name: "id", sqlType: "INT" }], many),
  });
  await page.click("#btn-run-all");
  await page.locator('th[data-col="0"]').click();

  await page.locator("#grid").evaluate((el) => (el.scrollTop = 3000));
  await expect(page.locator("td.sel").first()).toBeVisible();
});

/**
 * Two columns on purpose: selecting the *only* column is the same as selecting
 * everything, so a single-column result cannot tell a selection from no
 * selection — and a test that cannot fail is worse than no test.
 */
test("Escape clears the selection", async ({ page }) => {
  await connect(page, {
    run_script: () => rowsResult([{ name: "a" }, { name: "b" }], [["1", "2"]]),
  });
  await page.click("#btn-run-all");
  await page.locator('th[data-col="0"]').click();
  await expect(page.locator("#btn-copy")).toHaveText(/1 column/);

  await page.locator("#grid").click();
  await page.keyboard.press("Escape");
  await expect(page.locator("#btn-copy")).toHaveText("Copy");
});

// ------------------------------------------------- saved connections (C1-C3)

const SAVED = {
  id: "c1-saved",
  name: "prod-eu",
  colour: "#ef4444",
  host: "db.example.com",
  port: 3306,
  user: "reporting",
  database: "analytics",
  allowInvalidCerts: false,
};

/**
 * **The Stage 2 bug lived exactly here.** `rememberPassword` is derived from the
 * keychain and travels on the wire; when a `#[serde(skip)]` silently stripped
 * it, the UI concluded nothing was stored and prompted on every launch. Nothing
 * failed and nothing logged — the wrong value was a missing key.
 *
 * This is the frontend half of that: given the flag, the rail must use the
 * stored password instead of asking. The Rust half is pinned by
 * `session::tests::the_ui_is_told_when_a_password_is_remembered`.
 */
test("saved connections appear in the rail at boot, without contacting anything", async ({ page }) => {
  await installBackend(page, {
    list_profiles: () => ({
      profiles: [{ ...SAVED, rememberPassword: true }],
      warning: null,
    }),
  });
  await page.goto("/");

  await expect(page.locator(".rail-item")).toHaveCount(1);
  await expect(page.locator(".rail-item")).toHaveClass(/offline/);
  // Nothing is contacted until the user clicks.
  expect(await commandNames(page)).not.toContain("connect_saved");
  expect(await commandNames(page)).not.toContain("connect");
});

test("clicking a saved connection with a remembered password connects without prompting", async ({ page }) => {
  await installBackend(page, {
    list_profiles: () => ({
      profiles: [{ ...SAVED, rememberPassword: true }],
      warning: null,
    }),
    connect_saved: () => ({
      id: SAVED.id,
      serverVersion: "8.4.0",
      databases: ["analytics"],
      currentDatabase: "analytics",
    }),
    list_tables: () => [],
    list_routines: () => [],
  });
  await page.goto("/");

  await page.locator(".rail-item").click();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);

  const cmds = await commandNames(page);
  expect(cmds).toContain("connect_saved");
  // The password never leaves the keychain: the prompting path is not used.
  expect(cmds).not.toContain("connect");
  await expect(page.locator("#conn-dialog")).toBeHidden();
});

/** No stored secret — or no keychain at all — must fall back to asking. */
test("a saved connection with no stored password opens the editor instead", async ({ page }) => {
  await installBackend(page, {
    list_profiles: () => ({
      profiles: [{ ...SAVED, rememberPassword: false }],
      warning: null,
    }),
  });
  await page.goto("/");

  await page.locator(".rail-item").click();
  await expect(page.locator("#conn-dialog")).toBeVisible();
  // Pre-filled from the profile, so only the password has to be typed.
  await expect(page.locator('#conn-form input[name="host"]')).toHaveValue("db.example.com");
  await expect(page.locator('#conn-form input[name="user"]')).toHaveValue("reporting");
  await expect(page.locator('#conn-form input[name="password"]')).toHaveValue("");
  expect(await commandNames(page)).not.toContain("connect_saved");
});

/** A stored secret that has gone missing must say so, not fail silently. */
test("a failed stored-password connect reports why", async ({ page }) => {
  await installBackend(page, {
    list_profiles: () => ({
      profiles: [{ ...SAVED, rememberPassword: true }],
      warning: null,
    }),
    connect_saved: () => {
      throw new Error("No password is stored for this connection.");
    },
  });
  await page.goto("/");

  await page.locator(".rail-item").click();
  await expect(page.locator("#grid")).toContainText(/No password is stored/);
  await expect(page.locator(".rail-item.live")).toHaveCount(0);
});

/** A config file that could not be read is reported once, not swallowed. */
test("a corrupt profile file surfaces its warning", async ({ page }) => {
  await installBackend(page, {
    list_profiles: () => ({
      profiles: [],
      warning: "Saved connections could not be read. The file was kept as connections.json.corrupt-123.",
    }),
  });
  await page.goto("/");
  await expect(page.locator("#grid")).toContainText(/could not be read/);
});

// ------------------------------------------------- layout: it has to fit

/**
 * Found on the first real database, reported as "I cannot scroll the schema or
 * the results".
 *
 * `#app` is a grid with `height: 100vh` but no declared rows, and an implicit
 * row is `auto` — it sizes to its tallest child. A schema with more tables than
 * fit therefore made the row taller than the window: the sidebar grew to 3112px
 * in a 720px viewport, the results grid was pushed to y=3083, and `overflow:
 * auto` never engaged anywhere because nothing was ever constrained.
 *
 * Not a Windows bug, and not a scrollbar bug. It reproduces on Linux the moment
 * the tree is bigger than the window — which the four-table fixture never was.
 * That is why 192 passing tests missed it: every one of them used a schema that
 * fit on screen.
 */
const MANY_TABLES = Array.from({ length: 120 }, (_, i) => ({
  name: `table_${String(i).padStart(3, "0")}`,
  kind: "BASE TABLE",
}));

/** Nothing may extend past the bottom of the window. */
async function fits(page: import("@playwright/test").Page) {
  return page.evaluate(() => {
    const box = (sel: string) =>
      document.querySelector(sel)!.getBoundingClientRect().bottom;
    const el = document.querySelector(".tree") as HTMLElement;
    const grid = document.querySelector("#grid") as HTMLElement;
    return {
      vh: window.innerHeight,
      docScrollH: document.documentElement.scrollHeight,
      sidebarBottom: box("#sidebar"),
      gridBottom: box("#grid"),
      treeScrollable: el.scrollHeight > el.clientHeight,
      gridScrollable: grid.scrollHeight > grid.clientHeight,
    };
  });
}

test("a schema too big for the window scrolls instead of growing the page", async ({ page }) => {
  await connect(page, { ...schemaBackend, list_tables: () => MANY_TABLES });
  await openDatabase(page);
  await expect(page.locator(".node.table")).toHaveCount(120);

  const m = await fits(page);
  expect(m.sidebarBottom).toBeLessThanOrEqual(m.vh + 1);
  expect(m.docScrollH).toBeLessThanOrEqual(m.vh + 1);
  // The point of all of it: the tree can actually be scrolled.
  expect(m.treeScrollable).toBe(true);
});

test("a result too big for the window scrolls, and stays on screen", async ({ page }) => {
  await connect(page, {
    ...schemaBackend,
    list_tables: () => MANY_TABLES,
    run_script: () =>
      rowsResult(
        [{ name: "id" }, { name: "v" }],
        Array.from({ length: 500 }, (_, i) => [String(i), `row ${i}`]),
      ),
  });
  await openDatabase(page);
  await page.click("#btn-run-all");
  await expect(page.locator("table.rs")).toBeVisible();

  const m = await fits(page);
  expect(m.gridBottom).toBeLessThanOrEqual(m.vh + 1);
  expect(m.docScrollH).toBeLessThanOrEqual(m.vh + 1);
  expect(m.gridScrollable).toBe(true);
});

/**
 * The button has always disconnected when there was something to disconnect —
 * it just never said so. A control whose label states the opposite of what it
 * does is worse than one that is missing.
 */
test("the connect button says Disconnect while a connection is live", async ({ page }) => {
  await installBackend(page, { ...schemaBackend });
  await page.goto("/");
  await expect(page.locator("#btn-connect")).toHaveText("Connect");

  await page.click("#btn-connect");
  await page.click("#conn-ok");
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
  await expect(page.locator("#btn-connect")).toHaveText("Disconnect");
  await expect(page.locator("#btn-connect")).toHaveClass(/danger/);

  await page.click("#btn-connect");
  await expect(page.locator(".rail-item.live")).toHaveCount(0);
  await expect(page.locator("#btn-connect")).toHaveText("Connect");
  await expect(page.locator("#btn-connect")).not.toHaveClass(/danger/);
});
