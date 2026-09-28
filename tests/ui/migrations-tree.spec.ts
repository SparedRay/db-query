import { expect, test, type Page } from "@playwright/test";
import { calls, commandNames, connect, installBackend, schemaBackend } from "./harness";

/**
 * **The sidebar layout** (Stage 18), which is the default: projects live under
 * the schema tree, and the environment you choose opens as a tab in the main
 * area.
 *
 * `migrations.spec.ts` covers the same behaviour in the pane layout it
 * replaces. What is tested *here* is what only this shape has: the section, the
 * tab, the guards that keep a view from behaving like a buffer, and the row
 * action that Flyway's `-target` makes possible.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

const PATH = "/p/flyway.toml";

interface Match {
  connectionId: string;
  name: string;
  colour: string;
  readOnly: boolean;
}

const UAT: Match = { connectionId: "c-uat", name: "UAT", colour: "#ef4444", readOnly: false };

function env(id: string, displayName: string, host: string, user: string, matches: Match[] = []) {
  return {
    id,
    displayName,
    url: `jdbc:mysql://${host}:3306/flyway`,
    user,
    target: `${host}:3306`,
    matches,
  };
}

function project(over: Record<string, unknown> = {}) {
  return {
    id: "fp1",
    path: PATH,
    selected: "uat",
    name: "Flyway Connections",
    outOfOrder: false,
    environments: [
      env("development", "Development database", "dev.example.com", "dev_app"),
      env("uat", "UAT database", "uat.example.com", "uat_app", [UAT]),
    ],
    error: null,
    ...over,
  };
}

/** Two pending, one executed, nothing failed — so Apply is offered. */
const MIGRATIONS = [
  {
    version: "1", description: "create widgets", state: "Success", category: "Versioned",
    kind: "SQL", filepath: "/p/V1__create_widgets.sql",
    installedOnUtc: "2026-09-10T23:39:43Z", installedBy: "root", executionTimeMs: 12, group: "done",
  },
  {
    version: "4", description: "add colour", state: "Pending", category: "Versioned",
    kind: "SQL", filepath: "/p/V4__add_colour.sql",
    installedOnUtc: null, installedBy: null, executionTimeMs: null, group: "pending",
  },
  {
    version: "5", description: "index on orders", state: "Pending", category: "Versioned",
    kind: "SQL", filepath: "/p/V5__index.sql",
    installedOnUtc: null, installedBy: null, executionTimeMs: null, group: "pending",
  },
];

function flyway(initial: unknown[], extra: Record<string, unknown> = {}) {
  let store = [...initial] as Array<Record<string, unknown>>;
  return {
    ...schemaBackend,
    flyway_projects: () => ({ projects: store, warning: null }),
    flyway_pick_project: () => PATH,
    flyway_add_project: (a: Record<string, unknown>) => {
      const added = project({ path: a.path });
      store.push(added);
      return added;
    },
    flyway_remove_project: (a: Record<string, unknown>) => {
      store = store.filter((p) => p.id !== a.id);
    },
    flyway_select_environment: () => null,
    flyway_info: () => MIGRATIONS,
    flyway_migrate: () => ({ executed: 1, target: "4" }),
    flyway_repair: () => ({ actions: [], removed: [], deleted: [], aligned: [] }),
    refresh_schema: () => null,
    ...extra,
  };
}

/** The app with one project, nothing connected, the sidebar section drawn. */
async function withProject(page: Page, extra: Record<string, unknown> = {}) {
  await installBackend(page, flyway([project()], extra));
  await page.goto("/");
  await expect(page.locator("#mig-side")).toBeVisible();
}

/** …and the uat environment open as a tab. */
async function openUat(page: Page, extra: Record<string, unknown> = {}) {
  await withProject(page, extra);
  await page.click('#mig-side .mig-envrow[data-env="uat"]');
  await page.locator(".mig").first().waitFor();
}

const migrateCalls = async (page: Page) =>
  (await calls(page)).filter((c) => c.cmd === "flyway_migrate").map((c) => c.args);

// ----------------------------------------------------------------- the section

/** G1. No pane, no connection, and the projects are where databases are. */
test("projects live in the sidebar, with nothing connected", async ({ page }) => {
  await withProject(page);

  await expect(page.locator("#migrations-pane")).toBeHidden();
  await expect(page.locator("#mig-side .mig-project-name")).toHaveText("Flyway Connections");
  await expect(page.locator("#mig-side .mig-envrow")).toHaveCount(2);
  await expect(page.locator('#mig-side .mig-envrow[data-env="uat"] .mig-match-name')).toHaveText("UAT");
  expect(await commandNames(page)).not.toContain("connect");
  // Nothing has been asked of Flyway: no environment has been opened.
  expect(await commandNames(page)).not.toContain("flyway_info");
});

test("the section collapses, and the rail button brings it back", async ({ page }) => {
  await withProject(page);
  await page.click("#mig-side-toggle");
  await expect(page.locator("#mig-side")).toHaveClass(/collapsed/);
  await expect(page.locator("#mig-side-body")).toBeHidden();

  await page.click("#btn-migrations");
  await expect(page.locator("#mig-side")).not.toHaveClass(/collapsed/);
  await expect(page.locator("#mig-side-body")).toBeVisible();
});

// --------------------------------------------------------------------- the tab

/** G2. One tab, re-pointed — never one per environment. */
test("choosing an environment opens one tab, and another re-points it", async ({ page }) => {
  await openUat(page);

  const strip = page.locator("#script-tabs .stab");
  await expect(strip.filter({ hasText: "Migrations" })).toHaveCount(1);
  await expect(strip.filter({ hasText: "Migrations · uat" })).toBeVisible();
  await expect(page.locator("#mig-view")).toBeVisible();
  await expect(page.locator("#editor")).toBeHidden();

  await page.click('#mig-side .mig-envrow[data-env="development"]');
  await expect(strip.filter({ hasText: "Migrations" })).toHaveCount(1);
  await expect(strip.filter({ hasText: "Migrations · development" })).toBeVisible();
});

/** G3. It belongs to no connection, so every connection's strip has it. */
test("the migrations tab survives switching connection", async ({ page }) => {
  await connect(page, flyway([project()]));
  await page.click('#mig-side .mig-envrow[data-env="uat"]');
  await page.locator(".mig").first().waitFor();

  // A second connection, with its own workspace.
  await page.click(".rail-add");
  await page.locator("#conn-dialog").waitFor({ state: "visible" });
  await page.fill("#conn-dialog input[name=name]", "second");
  await page.click("#conn-ok");
  await page.locator("#conn-dialog").waitFor({ state: "hidden" });

  await expect(page.locator("#script-tabs .stab").filter({ hasText: "Migrations · uat" })).toBeVisible();
  // Its own SQL tab is in front, not the view.
  await expect(page.locator("#editor")).toBeVisible();
  await expect(page.locator("#mig-view")).toBeHidden();
});

test("closing the tab puts the editor back", async ({ page }) => {
  await openUat(page);
  await page.locator("#script-tabs .stab").filter({ hasText: "Migrations" }).locator(".stab-mark").click();

  await expect(page.locator("#mig-view")).toBeHidden();
  await expect(page.locator("#editor")).toBeVisible();
  await expect(page.locator("#script-tabs .stab").filter({ hasText: "Migrations" })).toHaveCount(0);
});

/** G5. A view has no buffer: the buffer's controls must not act on it. */
test("Run, Run all and Format do nothing while the view is in front", async ({ page }) => {
  await connect(page, flyway([project()]));
  await page.click('#mig-side .mig-envrow[data-env="uat"]');
  await page.locator(".mig").first().waitFor();

  // The buffer's toolbar goes with the buffer: Run, Run all and Format belong
  // to an editor, and there is none in front.
  await expect(page.locator("#editor-head")).toBeHidden();
  await page.keyboard.press("Control+Shift+F");
  await page.keyboard.press("Control+s");
  await page.keyboard.press("Control+Enter");

  const names = await commandNames(page);
  expect(names).not.toContain("run_script");
  expect(names).not.toContain("format_sql");
  expect(names).not.toContain("save_file");
});

/** G6. A view is reopened from the sidebar, never restored from the session. */
test("the migrations tab is never written to the session", async ({ page }) => {
  await connect(page, flyway([project()]));
  await page.click('#mig-side .mig-envrow[data-env="uat"]');
  await page.locator(".mig").first().waitFor();

  await expect
    .poll(async () => (await calls(page)).filter((c) => c.cmd === "save_session").length)
    .toBeGreaterThan(0);
  const saved = (await calls(page)).filter((c) => c.cmd === "save_session");
  const titles = saved.flatMap((c) => {
    const session = c.args.session as { connections: Array<{ tabs: Array<{ title: string }> }> };
    return session.connections.flatMap((w) => w.tabs.map((t) => t.title));
  });
  expect(titles.join(" ")).not.toContain("Migrations");
});

// ------------------------------------------------------------------- applying

/** The labels are spelled out: "…" was read as a label cut short. */
test("the buttons say what they do, in full", async ({ page }) => {
  await openUat(page);
  await expect(page.locator("#btn-mig-view-apply")).toHaveText("Apply 2 pending");
  await expect(page.locator("#btn-mig-view-repair")).toHaveText("Repair schema history");
});

/**
 * **Apply up to here.** Measured against Flyway 13.5.0 Community on
 * 2026-09-28: `-cherryPick` is refused by this edition, `-target=N` is not. So
 * a row offers a stopping point, and the confirmation names only what will run.
 */
test("a pending row applies up to itself, and says only what will run", async ({ page }) => {
  await openUat(page);

  const rows = page.locator(".mig-row");
  await rows.filter({ hasText: "add colour" }).locator(".mig-uptohere").click();

  const ask = page.locator("dialog.ask");
  await expect(ask.locator("h2")).toHaveText("Apply 1 migration?");
  await expect(ask).toContainText("V4");
  await expect(ask).toContainText("add colour");
  // The one it stops before is not named: the list behind the dialog says it.
  await expect(ask).not.toContainText("V5");
  await expect(ask).not.toContainText("index on orders");

  await ask.locator('button:has-text("Apply to uat")').click();
  await expect.poll(async () => (await migrateCalls(page)).length).toBe(1);
  expect((await migrateCalls(page))[0].target).toBe("4");
});

test("Apply with no row chosen runs every pending migration, with no target", async ({ page }) => {
  await openUat(page);
  await page.click("#btn-mig-view-apply");

  const ask = page.locator("dialog.ask");
  await expect(ask.locator("h2")).toHaveText("Apply 2 migrations?");
  await expect(ask).toContainText("V4");
  await expect(ask).toContainText("V5");
  await ask.locator('button:has-text("Apply to uat")').click();

  await expect.poll(async () => (await migrateCalls(page)).length).toBe(1);
  expect((await migrateCalls(page))[0].target).toBe(null);
});

/** A repeatable migration has no version, so it cannot be a target. */
test("a repeatable migration is not offered as a stopping point", async ({ page }) => {
  await openUat(page, {
    flyway_info: () => [
      { ...MIGRATIONS[1], version: null, description: "repeatable view", category: "Repeatable" },
      MIGRATIONS[2],
    ],
  });

  const repeatable = page.locator(".mig-row").filter({ hasText: "repeatable view" });
  await expect(repeatable.locator(".mig-uptohere")).toHaveCount(0);
  await expect(
    page.locator(".mig-row").filter({ hasText: "index on orders" }).locator(".mig-uptohere"),
  ).toHaveCount(1);
});

/** The row still opens the migration's SQL; the action must not. */
test("the row action does not open the file", async ({ page }) => {
  await openUat(page, {
    read_file: () => ({
      name: "V4__add_colour.sql", path: "/p/V4__add_colour.sql",
      contents: "ALTER TABLE widgets ADD colour VARCHAR(16);",
      sizeBytes: 41, encoding: "utf-8", lineEnding: "lf", mtimeMs: 1, readOnly: false,
    }),
  });

  await page.locator(".mig-row").filter({ hasText: "add colour" }).locator(".mig-uptohere").click();
  await expect(page.locator("dialog.ask")).toBeVisible();
  expect(await commandNames(page)).not.toContain("read_file");
  await page.locator('dialog.ask button:has-text("Cancel")').click();

  // The row itself still does.
  await page.locator('.mig[data-state="pending"]').first().click();
  await expect.poll(async () => commandNames(page)).toContain("read_file");
});

// -------------------------------------------------------------- the two layouts

/** G7. The setting swaps them without a restart, and the pane still works. */
test("the layout setting switches between the two", async ({ page }) => {
  await openUat(page);

  await page.click("#btn-settings");
  await page.click("#set-tab-integrations");
  await page.selectOption("#set-mig-layout", "pane");
  await page.click("#set-close");

  // The sidebar section goes, the view's tab goes with it, and the pane opens.
  await expect(page.locator("#mig-side")).toBeHidden();
  await expect(page.locator("#script-tabs .stab").filter({ hasText: "Migrations" })).toHaveCount(0);
  await page.click("#btn-migrations");
  await expect(page.locator("#migrations-pane")).toBeVisible();
  await expect(page.locator("#migrations-pane .mig-envrow")).toHaveCount(2);

  // And back.
  await page.click("#btn-settings");
  await page.selectOption("#set-mig-layout", "tree");
  await page.click("#set-close");
  await expect(page.locator("#mig-side")).toBeVisible();
  await expect(page.locator("#migrations-pane")).toBeHidden();
});
