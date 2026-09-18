import { expect, test, type Page } from "@playwright/test";
import { calls, commandNames, connect, installBackend, schemaBackend } from "./harness";

/**
 * The migrations pane.
 *
 * **Stage 17: projects stand on their own.** A project used to be attached to
 * a connection, and the pane followed whichever connection was active — which
 * implied that connection was the one being migrated, when Flyway always
 * connects with the project file's own settings. Now the pane lists projects
 * and their environments, and says which saved connection each environment
 * *is*, worked out from the file every time.
 *
 * A pane on the right rather than a section of the schema tree, so the database
 * structure stays visible while a migration is being read.
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

/** One project as `flyway_projects` returns it: `uat` matches a saved
 *  connection, `development` matches none. */
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

const MIGRATIONS = [
  {
    version: "1", description: "create widgets", state: "Success", category: "Versioned",
    kind: "SQL", filepath: "/p/V1__create_widgets.sql",
    installedOnUtc: "2026-09-10T23:39:43Z", installedBy: "root", executionTimeMs: 12, group: "done",
  },
  {
    version: "2", description: "add colour", state: "Pending", category: "Versioned",
    kind: "SQL", filepath: "/p/V2__add_colour.sql",
    installedOnUtc: null, installedBy: null, executionTimeMs: null, group: "pending",
  },
  {
    version: "3", description: "deliberately broken", state: "Failed", category: "Versioned",
    kind: "SQL", filepath: "/p/V3__broken.sql",
    installedOnUtc: "2026-09-10T23:39:43Z", installedBy: "root", executionTimeMs: 7, group: "failed",
  },
];

const PENDING_ONLY = [
  { version: "5", description: "add index", state: "Pending", category: "Versioned",
    kind: "SQL", filepath: "/p/V5.sql", installedOnUtc: null, installedBy: null,
    executionTimeMs: null, group: "pending" },
];

/**
 * A backend with a projects store that behaves like the real one: adding,
 * removing and listing all see the same list.
 */
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
    flyway_migrate: () => ({ executed: 1, target: "2" }),
    flyway_repair: () => ({
      actions: ["Removed failed migrations"],
      removed: [{ version: "3", description: "deliberately broken" }],
      deleted: [],
      aligned: [],
    }),
    refresh_schema: () => null,
    ...extra,
  };
}

async function openPane(page: Page) {
  await page.click("#btn-migrations");
  await expect(page.locator("#migrations-pane")).toBeVisible();
}

/** The app with one project added, **nothing connected**, on the list. */
async function withProject(page: Page, extra: Record<string, unknown> = {}) {
  await installBackend(page, flyway([project()], extra));
  await page.goto("/");
  await openPane(page);
  await page.locator(".mig").first().waitFor();
}

/** The last element, without `Array.prototype.at` (the tests target ES2020). */
const last = <T>(xs: T[]): T | undefined => xs[xs.length - 1];

const infoCalls = async (page: Page) =>
  (await calls(page)).filter((c) => c.cmd === "flyway_info").map((c) => c.args);

// ------------------------------------------------ standing on their own (F1)

/**
 * **The point of the stage.** Flyway connects with the project file's own
 * settings, so nothing about a project needs a connection open — and a pane
 * that followed the active connection implied that connection was the one
 * being migrated.
 */
test("the pane is offered, and works, with nothing connected", async ({ page }) => {
  await installBackend(page, flyway([project()]));
  await page.goto("/");
  await expect(page.locator("#btn-migrations")).toBeVisible();
  await openPane(page);

  await page.locator(".mig").first().waitFor();
  expect(await commandNames(page)).not.toContain("connect");
  expect(await infoCalls(page)).toEqual([
    { projectId: "fp1", environment: "uat", program: "", outOfOrder: false },
  ]);
});

test("the pane opens beside the schema tree, not instead of it", async ({ page }) => {
  await connect(page, flyway([project()]));
  await openPane(page);
  await expect(page.locator("#sidebar")).toBeVisible();
  await expect(page.locator("#editor")).toBeVisible();

  await page.click("#btn-migrations");
  await expect(page.locator("#migrations-pane")).toBeHidden();
});

test("with no projects the pane says what to do, and asks Flyway nothing", async ({ page }) => {
  await installBackend(page, flyway([]));
  await page.goto("/");
  await openPane(page);

  await expect(page.locator(".mig-empty")).toContainText(/No Flyway projects yet/);
  await expect(page.locator(".mig-empty")).toContainText(/No connection needs to be open/);
  await expect(page.locator(".mig-empty button")).toHaveText(/Add a project/);
  expect(await commandNames(page)).not.toContain("flyway_info");
});

test("adding a project stores its path and lands on its environment", async ({ page }) => {
  await installBackend(page, flyway([]));
  await page.goto("/");
  await openPane(page);
  await page.click(".mig-empty button");

  await page.locator(".mig").first().waitFor();
  const added = (await calls(page)).find((c) => c.cmd === "flyway_add_project")!;
  expect(added.args).toEqual({ path: PATH });
  // No environment question and no connection question: the file's own
  // default is selected, and which connection each environment is gets worked
  // out from the file.
  expect(await commandNames(page)).not.toContain("save_profile");
  expect(last(await infoCalls(page))).toMatchObject({ projectId: "fp1", environment: "uat" });
});

test("the header's Add button adds a project too", async ({ page }) => {
  await installBackend(page, flyway([]));
  await page.goto("/");
  await openPane(page);
  await page.click("#btn-mig-add");
  await expect(page.locator(".mig-envrow")).toHaveCount(2);
});

test("a project can be removed, and the file is not touched", async ({ page }) => {
  await withProject(page);
  await page.click(".mig-project-more");
  await expect(page.locator("dialog.ask")).toContainText(PATH);
  await page.locator('dialog.ask button:has-text("Remove from list")').click();

  await expect(page.locator(".mig-empty")).toContainText(/No Flyway projects yet/);
  const removed = (await calls(page)).find((c) => c.cmd === "flyway_remove_project")!;
  expect(removed.args).toEqual({ id: "fp1" });
  expect(await commandNames(page)).not.toContain("save_file");
});

/** A branch switch can take the file away; the project stays, with the reason. */
test("a project whose file cannot be read stays listed and says why", async ({ page }) => {
  await installBackend(
    page,
    flyway([
      project({
        environments: [],
        name: null,
        error: "Cannot read /p/flyway.toml: No such file or directory",
      }),
    ]),
  );
  await page.goto("/");
  await openPane(page);

  await expect(page.locator(".mig-project-error")).toContainText("No such file or directory");
  await expect(page.locator(".mig-project-name")).toHaveText("flyway.toml");
  expect(await commandNames(page)).not.toContain("flyway_info");
});

// ------------------------------------------------- which database it is (F2)

/**
 * Each environment says which of the user's saved connections it is — its
 * name and colour — or that it is none of them. That is what says "this is
 * prod" before anybody presses Apply.
 */
test("each environment names the saved connection it is, or says it is none", async ({ page }) => {
  await withProject(page);

  const uat = page.locator('.mig-envrow[data-env="uat"]');
  await expect(uat.locator(".mig-env-target")).toHaveText("uat.example.com:3306");
  await expect(uat.locator(".mig-match-name")).toHaveText("UAT");
  await expect(uat.locator(".conn-dot")).toHaveCSS("background-color", "rgb(239, 68, 68)");

  const dev = page.locator('.mig-envrow[data-env="development"]');
  await expect(dev.locator(".mig-match")).toHaveText("no saved connection");

  // The selected one is said in full above the list.
  await expect(page.locator("#mig-env")).toContainText("uat.example.com:3306 as uat_app");
  await expect(page.locator(".mig-proj-match")).toHaveText("This is your connection “UAT”.");
});

test("an environment matching several connections shows the first and how many more", async ({
  page,
}) => {
  await withProject(page, {
    flyway_projects: () => ({
      projects: [
        project({
          environments: [
            env("uat", "UAT", "uat.example.com", "uat_app", [
              UAT,
              { ...UAT, connectionId: "c-2", name: "UAT (browse)" },
            ]),
          ],
        }),
      ],
      warning: null,
    }),
  });
  await expect(page.locator(".mig-match-name")).toHaveText("UAT +1");
});

// --------------------------------------------------- choosing an environment

test("choosing another environment asks Flyway about it, and is remembered", async ({ page }) => {
  await withProject(page);
  await page.click('.mig-envrow[data-env="development"]');

  await expect(page.locator('.mig-envrow[data-env="development"]')).toHaveClass(/selected/);
  await expect(page.locator('.mig-envrow[data-env="uat"]')).not.toHaveClass(/selected/);
  await expect.poll(async () => last(await infoCalls(page))?.environment).toBe("development");

  const remembered = (await calls(page)).find((c) => c.cmd === "flyway_select_environment")!;
  expect(remembered.args).toEqual({ id: "fp1", environment: "development" });
  await expect(page.locator(".mig-proj-match")).toHaveText(
    "This is not one of your saved connections.",
  );
});

/**
 * **A late answer must not land under the wrong environment.** Flyway takes
 * ten seconds on a real server; clicking another environment in the meantime
 * and then getting the first one's list beside the second one's Apply button
 * would be an invitation to apply to a database whose list you were not shown.
 */
test("an answer for an environment you have left is dropped", async ({ page }) => {
  const DEV_ONLY = [{ ...MIGRATIONS[1], description: "dev's own migration" }];
  await installBackend(
    page,
    flyway([project()], {
      flyway_info: async (a: Record<string, unknown>) => {
        if (a.environment === "uat") {
          await new Promise((r) => setTimeout(r, 1500));
          return MIGRATIONS;
        }
        return DEV_ONLY;
      },
    }),
  );
  await page.goto("/");
  await openPane(page);
  await page.click('.mig-envrow[data-env="development"]');

  await expect(page.locator(".mig .d")).toHaveText("dev's own migration");
  // Long enough for the slow uat answer to have arrived and been ignored.
  await page.waitForTimeout(2000);
  await expect(page.locator(".mig .d")).toHaveText("dev's own migration");
  await expect(page.locator(".mig")).toHaveCount(1);
});

/**
 * Out-of-order decides what an apply will run, so it belongs to one
 * environment. A shared flag would carry the choice made on dev onto UAT the
 * moment you clicked across.
 */
test("out of order belongs to the environment, not to the window", async ({ page }) => {
  await withProject(page);
  await page.locator("#mig-out-of-order").check();
  await expect.poll(async () => last(await infoCalls(page))?.outOfOrder).toBe(true);

  await page.click('.mig-envrow[data-env="development"]');
  await expect(page.locator("#mig-out-of-order")).not.toBeChecked();
  await expect.poll(async () => last(await infoCalls(page))?.environment).toBe("development");
  expect(last(await infoCalls(page))?.outOfOrder).toBe(false);

  await page.click('.mig-envrow[data-env="uat"]');
  await expect(page.locator("#mig-out-of-order")).toBeChecked();
});

// ------------------------------------------------------------ the list

test("the list shows Flyway's own states, grouped", async ({ page }) => {
  await withProject(page);

  await expect(page.locator(".mig")).toHaveCount(3);
  await expect(page.locator('.mig[data-state="failed"] .d')).toHaveText("deliberately broken");
  await expect(page.locator('.mig[data-state="pending"] .d')).toHaveText("add colour");
  // Named for what has happened, not for what an apply would do: "Will not
  // run" over applied migrations read as a verdict.
  await expect(page.locator('.mig-group[data-group="pending"] .mig-group-title')).toHaveText(
    "Pending",
  );
  await expect(page.locator('.mig-group[data-group="done"] .mig-group-title')).toHaveText(
    "Executed",
  );
  await expect(page.locator('.mig-group[data-group="done"] .mig .d')).toHaveText(
    "create widgets",
  );
});

const V2_FILE = {
  read_file: () => ({
    name: "V2__add_colour.sql", path: "/p/V2__add_colour.sql",
    contents: "ALTER TABLE widgets ADD colour VARCHAR(16);",
    sizeBytes: 41, encoding: "utf-8", lineEnding: "lf", mtimeMs: 1, readOnly: false,
  }),
};

test("clicking a migration opens its SQL in a tab, without binding the file", async ({ page }) => {
  await connect(page, flyway([project()], V2_FILE));
  await openPane(page);
  await page.locator('.mig[data-state="pending"]').click();

  await expect(page.locator("#editor .cm-content")).toContainText("ADD colour");
  const names = await commandNames(page);
  expect(names).not.toContain("run_script");
  expect(names).not.toContain("save_file");

  // And it says it is a migration, not something an MCP client sent.
  const mark = page.locator("#script-tabs .stab.active .stab-origin");
  await expect(mark).toHaveAttribute("data-origin", "migration");
  await expect(mark).toHaveAttribute("title", /Flyway migration/);
  await expect(mark.locator("svg")).toHaveCount(1);
});

/** A tab belongs to a connection; with none open there is still SQL to read. */
test("with nothing connected a migration opens in the viewer", async ({ page }) => {
  await withProject(page, V2_FILE);
  await page.locator('.mig[data-state="pending"]').click();

  const viewer = page.locator("dialog.viewer");
  await expect(viewer.locator("h2")).toHaveText("V2 add colour");
  await expect(viewer.locator(".viewer-body")).toContainText("ADD colour");
  expect(await commandNames(page)).not.toContain("run_script");
});

/** Flyway explains itself well; a paraphrase would replace an instruction. */
test("Flyway's own refusal is what the pane shows", async ({ page }) => {
  await installBackend(
    page,
    flyway([project()], {
      flyway_info: () => {
        throw new Error(
          "Validate failed: Detected failed migration to version 4. Please remove any " +
            "half-completed changes then run repair to fix the schema history.",
        );
      },
    }),
  );
  await page.goto("/");
  await openPane(page);
  await expect(page.locator(".mig-empty")).toContainText(/run repair to fix the schema history/);
});

test("groups collapse and expand without asking Flyway again", async ({ page }) => {
  await withProject(page);
  const done = page.locator('.mig-group[data-group="done"]');

  await expect(done).toHaveClass(/collapsed/);
  await expect(done.locator(".mig-group-count")).toHaveText("1");

  const before = (await infoCalls(page)).length;
  await done.locator(".mig-group-head").click();
  await expect(done).not.toHaveClass(/collapsed/);
  await expect(done.locator(".mig-group-head")).toHaveAttribute("aria-expanded", "true");
  expect((await infoCalls(page)).length).toBe(before);

  await done.locator(".mig-group-head").click();
  await expect(done).toHaveClass(/collapsed/);
});

/** `Ignored` becomes `Pending` under out-of-order, so the question is asked
 *  again with the flag rather than reasoned about here. */
test("out of order re-asks Flyway with the flag", async ({ page }) => {
  await withProject(page);
  expect((await infoCalls(page)).map((a) => a.outOfOrder)).toEqual([false]);

  await page.locator("#mig-out-of-order").check();
  await expect.poll(async () => (await infoCalls(page)).length).toBe(2);
  expect((await infoCalls(page))[1].outOfOrder).toBe(true);
});

// ---------------------------------------------------- where it is reading

test("the pane says which project file and folder it is using", async ({ page }) => {
  await withProject(page);
  await expect(page.locator(".mig-project-name")).toHaveText("Flyway Connections");
  await expect(page.locator(".mig-project-name")).toHaveAttribute("title", PATH);
  await expect(page.locator(".mig-proj-where")).toContainText("flyway.toml");
  await expect(page.locator(".mig-proj-where")).toContainText("/p");
});

/**
 * Where the migrations come from — taken from the files Flyway reported, never
 * from `locations`, which is relative, can be a list and can be overridden per
 * environment.
 */
test("the pane says which folder the migrations were actually read from", async ({ page }) => {
  await withProject(page);
  await expect(page.locator(".mig-proj-from")).toContainText("/p");
  await expect(page.locator(".mig-proj-from")).toHaveAttribute("title", /reading migrations from/);
});

test("several folders are all named, and none is invented", async ({ page }) => {
  await withProject(page, {
    flyway_info: () => [
      { ...MIGRATIONS[0], filepath: "/repo/sql/common/V1__create_widgets.sql" },
      { ...MIGRATIONS[1], filepath: "/repo/sql/uat/V2__add_colour.sql" },
    ],
  });
  const from = page.locator(".mig-proj-from");
  await expect(from).toContainText("/repo/sql/common");
  await expect(from).toContainText("/repo/sql/uat");
});

test("a list with no file paths leaves the folder line out", async ({ page }) => {
  await withProject(page, { flyway_info: () => [{ ...MIGRATIONS[1], filepath: null }] });
  await expect(page.locator(".mig-proj-from")).toBeHidden();
});

// ------------------------------------------------------------- applying

test("Apply names the versions and the target, and runs nothing until confirmed", async ({
  page,
}) => {
  await withProject(page, { flyway_info: () => [MIGRATIONS[0], ...PENDING_ONLY] });

  await expect(page.locator("#btn-mig-apply")).toBeEnabled();
  await expect(page.locator("#btn-mig-apply")).toHaveText("Apply 1…");
  await page.click("#btn-mig-apply");

  const ask = page.locator("dialog.ask");
  await expect(ask).toContainText("V5");
  await expect(ask).toContainText("add index");
  // Which database, as whom, and which of your connections that is.
  await expect(ask).toContainText("uat.example.com:3306 as uat_app");
  await expect(ask).toContainText("This is your connection “UAT”.");

  await ask.locator('button:has-text("Cancel")').click();
  expect(await commandNames(page)).not.toContain("flyway_migrate");
});

/** Unmatched is allowed — often the migration account — but said plainly. */
test("applying to an environment that is none of your connections says so", async ({ page }) => {
  await withProject(page, { flyway_info: () => PENDING_ONLY });
  await page.click('.mig-envrow[data-env="development"]');
  await expect(page.locator("#btn-mig-apply")).toBeEnabled();
  await page.click("#btn-mig-apply");

  const ask = page.locator("dialog.ask");
  await expect(ask).toContainText("dev.example.com:3306 as dev_app");
  await expect(ask).toContainText("This is not one of your saved connections.");
});

/**
 * F5's UI half. The dialog is built from the file as it reads **when the
 * dialog opens**, and the command is handed that URL and user so Rust can
 * refuse if the file moved again before the button was pressed.
 */
test("the confirmation describes the file as it reads now, and sends that target back", async ({
  page,
}) => {
  let reads = 0;
  const moved = project({
    environments: [
      env("development", "Development database", "dev.example.com", "dev_app"),
      env("uat", "UAT database", "uat2.example.com", "uat_app"),
    ],
  });
  await withProject(page, {
    flyway_projects: () => ({ projects: [reads++ === 0 ? project() : moved], warning: null }),
    flyway_info: () => PENDING_ONLY,
  });

  await page.click("#btn-mig-apply");
  const ask = page.locator("dialog.ask");
  await expect(ask).toContainText("uat2.example.com:3306");
  await expect(ask).toContainText("This is not one of your saved connections.");
  await ask.locator('button:has-text("Apply to uat")').click();

  await expect.poll(async () => commandNames(page)).toContain("flyway_migrate");
  const call = (await calls(page)).find((c) => c.cmd === "flyway_migrate")!;
  expect(call.args).toEqual({
    projectId: "fp1",
    environment: "uat",
    confirmed: { url: "jdbc:mysql://uat2.example.com:3306/flyway", user: "uat_app" },
    program: "",
    outOfOrder: false,
  });
});

test("confirming applies, and says what Flyway did", async ({ page }) => {
  await withProject(page, {
    flyway_info: () => PENDING_ONLY,
    flyway_migrate: () => ({ executed: 1, target: "5" }),
  });

  await page.click("#btn-mig-apply");
  await page.locator('dialog.ask button:has-text("Apply to uat")').click();

  await expect(page.locator("#grid .empty")).toContainText("applied 1 migration to uat");
  await expect(page.locator("#grid .empty")).toContainText("now at 5");
  // UAT is not open here, so there is nothing to refresh and it says so.
  await expect(page.locator("#grid .empty")).toContainText("may need a refresh");
});

/**
 * F6. The tree beside the pane should show what the apply just made. Every
 * open connection this environment *is* has its schema cache dropped.
 */
test("after an apply, an open connection that is this environment is refreshed", async ({
  page,
}) => {
  let connId = "";
  await connect(
    page,
    flyway([], {
      flyway_projects: () => ({
        projects: [
          project({
            environments: [
              env("uat", "UAT", "uat.example.com", "uat_app", [{ ...UAT, connectionId: connId }]),
            ],
          }),
        ],
        warning: null,
      }),
      flyway_info: () => PENDING_ONLY,
    }),
  );
  const connected = (await calls(page)).find((c) => c.cmd === "connect")!;
  connId = (connected.args.profile as { id: string }).id;

  await openPane(page);
  await page.click("#btn-mig-apply");
  await page.locator('dialog.ask button:has-text("Apply to uat")').click();

  await expect(page.locator("#grid .empty")).toContainText("refreshed the schema of UAT");
  const refreshed = (await calls(page)).filter((c) => c.cmd === "refresh_schema");
  expect(refreshed.map((c) => c.args)).toEqual([{ connectionId: connId, db: "poc" }]);
});

/** F3. Refused in Rust too; this is the courtesy, not the guard. */
test("a read-only match is not offered an apply or a repair, and says which", async ({
  page,
}) => {
  await withProject(page, {
    flyway_projects: () => ({
      projects: [
        project({
          environments: [
            env("uat", "UAT", "uat.example.com", "uat_app", [{ ...UAT, readOnly: true }]),
          ],
        }),
      ],
      warning: null,
    }),
  });

  await expect(page.locator('.mig-envrow[data-env="uat"] .chip')).toHaveText("read-only");
  await expect(page.locator("#btn-mig-apply")).toBeDisabled();
  await expect(page.locator("#btn-mig-apply")).toHaveAttribute("title", /“UAT”.*read-only/);
  await expect(page.locator("#btn-mig-repair")).toBeDisabled();
  await expect(page.locator("#btn-mig-repair")).toHaveAttribute("title", /“UAT”.*read-only/);
  // Not urged either: something is failed, but nothing may be done about it.
  await expect(page.locator("#btn-mig-repair")).not.toHaveClass(/urge/);
});

test("a failed migration blocks apply, and the button explains why", async ({ page }) => {
  await withProject(page);
  await expect(page.locator("#btn-mig-apply")).toBeDisabled();
  await expect(page.locator("#btn-mig-apply")).toHaveAttribute("title", /failed/);
  await expect(page.locator("#btn-mig-apply")).toHaveAttribute("title", /repaired/);
});

test("a failed apply shows Flyway's message verbatim", async ({ page }) => {
  const FLYWAY_SAID =
    "Failed to execute script V4__deliberately_broken.sql against development environment\n" +
    "Message    : (conn=13) Can't DROP 'weight'; check that column/key exists";
  await withProject(page, {
    flyway_info: () => PENDING_ONLY,
    flyway_migrate: () => {
      throw new Error(FLYWAY_SAID);
    },
  });

  await page.click("#btn-mig-apply");
  await page.locator('dialog.ask button:has-text("Apply to uat")').click();

  await expect(page.locator("#grid .empty")).toContainText("Can't DROP 'weight'");
  await expect(page.locator("#grid .empty")).toContainText("V4__deliberately_broken.sql");
});

// ------------------------------------------------------------- repairing

/**
 * Always offered: two of the three things repair fixes — a drifted checksum,
 * an entry whose file is gone — are reported by `info` as `Success`, so a
 * button shown only when the list looks wrong is missing when it is needed.
 * Urged only when something has asked for it.
 */
test("Repair is always offered, and urged while something has failed", async ({ page }) => {
  let asked = 0;
  await withProject(page, {
    flyway_info: () => (asked++ === 0 ? MIGRATIONS : [MIGRATIONS[0], MIGRATIONS[1]]),
  });
  await expect(page.locator("#btn-mig-repair")).toBeVisible();
  await expect(page.locator("#btn-mig-repair")).toHaveClass(/urge/);

  await page.click("#btn-mig-refresh");
  await expect(page.locator('.mig[data-state="failed"]')).toHaveCount(0);
  await expect(page.locator("#btn-mig-repair")).toBeVisible();
  await expect(page.locator("#btn-mig-repair")).not.toHaveClass(/urge/);
  await expect(page.locator("#btn-mig-apply")).toBeEnabled();
});

/**
 * With nothing failed, repair realigns checksums — which leaves the database
 * holding the *original* version while the history claims the edited one ran.
 * Measured against Flyway 13.5.0 on 2026-09-16.
 */
test("repairing with nothing failed says what realigning a checksum means", async ({ page }) => {
  await withProject(page, {
    flyway_info: () => [MIGRATIONS[0], MIGRATIONS[1]],
    flyway_repair: () => ({
      actions: ["Aligned applied migration checksums"],
      removed: [],
      deleted: [],
      aligned: [{ version: "1", description: "create widgets" }],
    }),
  });

  await page.click("#btn-mig-repair");
  const ask = page.locator("dialog.ask");
  await expect(ask).not.toContainText("remove the failed entry");
  await expect(ask).toContainText("realign the checksum");
  await expect(ask).toContainText("does not run, re-run or undo anything");
  await expect(ask).toContainText("as it was first executed");

  await ask.locator('button:has-text("Repair uat")').click();
  await expect(page.locator("#grid .empty")).toContainText("Aligned applied migration checksums");
  await expect(page.locator("#grid .empty")).toContainText("V1");
});

/** "Repair" sounds like it fixes the database, and it does not. */
test("the repair confirmation says where, and what it does not undo", async ({ page }) => {
  await withProject(page);
  await page.click("#btn-mig-repair");

  const ask = page.locator("dialog.ask");
  await expect(ask).toContainText("uat.example.com:3306 as uat_app");
  await expect(ask).toContainText("This is your connection “UAT”.");
  await expect(ask).toContainText("V3");
  await expect(ask).toContainText("deliberately broken");
  await expect(ask).toContainText("does not undo");
  await expect(ask).toContainText("still in the database");
  await expect(ask).toContainText("pending again");

  await ask.locator('button:has-text("Cancel")').click();
  expect(await commandNames(page)).not.toContain("flyway_repair");
});

test("confirming repairs, says what Flyway did, and re-reads the list", async ({ page }) => {
  await withProject(page);
  const before = (await infoCalls(page)).length;

  await page.click("#btn-mig-repair");
  await page.locator('dialog.ask button:has-text("Repair uat")').click();

  await expect.poll(async () => commandNames(page)).toContain("flyway_repair");
  const call = (await calls(page)).find((c) => c.cmd === "flyway_repair")!;
  expect(call.args).toEqual({
    projectId: "fp1",
    environment: "uat",
    confirmed: { url: "jdbc:mysql://uat.example.com:3306/flyway", user: "uat_app" },
    program: "",
  });
  await expect(page.locator("#grid .empty")).toContainText("Removed failed migrations");
  await expect(page.locator("#grid .empty")).toContainText("V3");
  await expect(page.locator("#grid .empty")).toContainText("database itself is unchanged");
  await expect.poll(async () => (await infoCalls(page)).length).toBeGreaterThan(before);
});

test("a repair that found nothing to do says so, rather than claiming success", async ({
  page,
}) => {
  await withProject(page, {
    flyway_repair: () => ({ actions: [], removed: [], deleted: [], aligned: [] }),
  });

  await page.click("#btn-mig-repair");
  await page.locator('dialog.ask button:has-text("Repair uat")').click();

  await expect(page.locator("#grid .empty")).toContainText("found nothing to repair");
  await expect(page.locator("#grid .empty")).not.toContainText("Removed");
});

test("a refused repair shows Flyway's own message", async ({ page }) => {
  await withProject(page, {
    flyway_repair: () => {
      throw new Error("Unable to connect to the database. Check the connection details.");
    },
  });

  await page.click("#btn-mig-repair");
  await page.locator('dialog.ask button:has-text("Repair uat")').click();

  await expect(page.locator("#grid .empty")).toContainText("Unable to connect to the database");
});

/**
 * A migration edited after it ran is still `Success` in `info`, so the list
 * looks healthy; Flyway refuses the apply with a checksum mismatch and says to
 * run repair. That is the one moment to push the button forward.
 */
test("Repair is urged when Flyway asks for one, even with nothing failed", async ({ page }) => {
  await withProject(page, {
    flyway_info: () => [MIGRATIONS[0], MIGRATIONS[1]],
    flyway_migrate: () => {
      throw {
        message:
          "Validate failed: Migrations have failed validation\n" +
          "Migration checksum mismatch for migration version 2\n" +
          "Either revert the changes to the migration, or run repair to update the schema history.",
        suggestsRepair: true,
      };
    },
  });

  await expect(page.locator("#btn-mig-repair")).not.toHaveClass(/urge/);
  await page.click("#btn-mig-apply");
  await page.locator('dialog.ask button:has-text("Apply to uat")').click();

  await expect(page.locator("#grid .empty")).toContainText("checksum mismatch");
  await expect(page.locator("#btn-mig-repair")).toBeEnabled();
  await expect(page.locator("#btn-mig-repair")).toHaveClass(/urge/);
  await expect(page.locator("#btn-mig-repair")).toHaveAttribute("title", /asked for a repair/);

  // Asked for *this* environment. Moving to another must not carry it along.
  await page.click('.mig-envrow[data-env="development"]');
  await expect(page.locator("#btn-mig-repair")).not.toHaveClass(/urge/);
});

test("an unrelated refusal leaves Repair where it was", async ({ page }) => {
  await withProject(page, {
    flyway_info: () => [MIGRATIONS[0], MIGRATIONS[1]],
    flyway_migrate: () => {
      throw {
        message: "Unable to connect to the database. Check the connection details.",
        suggestsRepair: false,
      };
    },
  });

  await page.click("#btn-mig-apply");
  await page.locator('dialog.ask button:has-text("Apply to uat")').click();

  await expect(page.locator("#grid .empty")).toContainText("Unable to connect");
  await expect(page.locator("#btn-mig-repair")).not.toHaveClass(/urge/);
});
