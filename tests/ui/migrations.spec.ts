import { expect, test, type Page } from "@playwright/test";
import { calls, commandNames, connect, installBackend, rowsResult, schemaBackend, visit } from "./harness";

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
 * **Stage 18: and they live in the app's own flow.** Projects are a section of
 * the schema sidebar; choosing an environment opens the list as a tab in the
 * main area, where it has the width to be a table. The pane both of these
 * stages started from is gone, and every claim it carried is here.
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

/** Two pending, nothing failed — the state in which Apply is offered. */
const TWO_PENDING = [
  MIGRATIONS[0],
  { ...MIGRATIONS[1], version: "4", description: "add colour" },
  { ...MIGRATIONS[1], version: "5", description: "index on orders" },
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
function flywayCommands(initial: unknown[], extra: Record<string, unknown> = {}) {
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

/**
 * The folders a stubbed `info` pretends its migrations came from.
 *
 * Stage 24 made `flyway_info` answer with a *reading* — the list, the folders
 * it came from and a fingerprint of them — so the app can ask the disk whether
 * anything moved instead of starting another JVM. Every stub below still
 * returns a plain list, which is the thing each test is about, and this wraps
 * it into the shape the app now receives.
 *
 * **`flyway_fingerprint` is deliberately left unstubbed** here: the call fails,
 * the app cannot prove the files are unchanged, and it asks Flyway again — so
 * every test written before this stage still exercises what it was written to
 * exercise. The tests that are about reuse stub it on purpose.
 */
const STUB_FOLDERS = ["/srv/app/migrations"];

function asReading(value: unknown) {
  return Array.isArray(value)
    ? { migrations: value, folders: STUB_FOLDERS, fingerprint: "stub-fingerprint" }
    : value;
}

/** The backend every test here installs, with `flyway_info` answering a reading. */
function flyway(initial: unknown[], extra: Record<string, unknown> = {}) {
  const merged = flywayCommands(initial, extra) as Record<
    string,
    (a: Record<string, unknown>) => unknown
  >;
  const info = merged.flyway_info;
  merged.flyway_info = async (a: Record<string, unknown>) => asReading(await info(a));
  return merged;
}

/**
 * Open the migrations panel from its rail button (Stage 28). Opening it asks Flyway
 * about the remembered environment, as opening the old tab did.
 */
async function openPanel(page: Page) {
  if (await page.locator("#mig-pane").isHidden()) await page.click("#btn-migrations");
  await expect(page.locator("#mig-pane")).toBeVisible();
}

/** The environment picker, open: the project and environment rows. */
async function picker(page: Page) {
  await openPanel(page);
  if (await page.locator("#mig-picker-pop").isHidden()) await page.click("#mig-picker");
  const pop = page.locator("#mig-picker-pop");
  await expect(pop).toBeVisible();
  return pop;
}

/** Choose an environment from the picker. */
async function chooseEnv(page: Page, env: string) {
  await (await picker(page)).locator(`.mig-envrow[data-env="${env}"]`).click();
}

/** The panel open, with nothing connected. */
async function openApp(page: Page, extra: Record<string, unknown> = {}) {
  await installBackend(page, flyway([project()], extra));
  await visit(page);
  await openPanel(page);
}

/** …and `uat` chosen, with its list on screen. */
async function withProject(page: Page, extra: Record<string, unknown> = {}) {
  await openApp(page, extra);
  await chooseEnv(page, "uat");
  await page.locator(".mig").first().waitFor();
}

/** The last element, without `Array.prototype.at` (the tests target ES2020). */
const last = <T>(xs: T[]): T | undefined => xs[xs.length - 1];

const migrateCalls = async (page: Page) =>
  (await calls(page)).filter((c) => c.cmd === "flyway_migrate").map((c) => c.args);

const infoCalls = async (page: Page) =>
  (await calls(page)).filter((c) => c.cmd === "flyway_info").map((c) => c.args);

// ------------------------------------------------ standing on their own (F1)



test("with no projects the panel says what to do, and asks Flyway nothing", async ({ page }) => {
  await installBackend(page, flyway([]));
  await visit(page);
  await openPanel(page);

  await expect(page.locator(".mig-empty")).toContainText(/No Flyway projects yet/);
  await expect(page.locator(".mig-empty")).toContainText(/No connection needs to be open/);

  // Nothing to choose from and nothing that can have run: the picker's popover
  // and the Last run section would both be empty boxes with a border on them.
  await page.click("#mig-picker");
  await expect(page.locator("#mig-picker-label")).toHaveText("No projects yet");
  await expect(page.locator("#mig-picker-pop")).toBeHidden();
  await expect(page.locator("#mig-report")).toBeHidden();
  await expect(page.locator(".mig-empty button")).toHaveText(/Add a project/);
  expect(await commandNames(page)).not.toContain("flyway_info");
});

test("adding a project stores its path and lands on its environment", async ({ page }) => {
  await installBackend(page, flyway([]));
  await visit(page);
  await openPanel(page);
  await page.click("#mig-pane-body .mig-empty button");

  await page.locator(".mig").first().waitFor();
  const added = (await calls(page)).find((c) => c.cmd === "flyway_add_project")!;
  expect(added.args).toEqual({ path: PATH });
  // No environment question and no connection question: the file's own
  // default is selected, and which connection each environment is gets worked
  // out from the file.
  expect(await commandNames(page)).not.toContain("save_profile");
  expect(last(await infoCalls(page))).toMatchObject({ projectId: "fp1", environment: "uat" });
});

test("the panel's Add button adds a project too", async ({ page }) => {
  await installBackend(page, flyway([]));
  await visit(page);
  await openPanel(page);
  await page.click("#btn-mig-add");
  await expect((await picker(page)).locator(".mig-envrow")).toHaveCount(2);
});

test("a project can be removed, and the file is not touched", async ({ page }) => {
  await withProject(page);
  await (await picker(page)).locator(".mig-project-more").click();
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
  await visit(page);
  await openPanel(page);

  await expect(page.locator(".mig-project-error")).toContainText("No such file or directory");
  await expect((await picker(page)).locator(".mig-project-name")).toHaveText("flyway.toml");
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

  const uat = (await picker(page)).locator('.mig-envrow[data-env="uat"]');
  await expect(uat.locator(".mig-env-target")).toHaveText("uat.example.com:3306");
  await expect(uat.locator(".mig-match-name")).toHaveText("UAT");
  await expect(uat.locator(".conn-dot")).toHaveCSS("background-color", "rgb(239, 68, 68)");

  const dev = (await picker(page)).locator('.mig-envrow[data-env="development"]');
  await expect(dev.locator(".mig-match")).toHaveText("no saved connection");

  // The selected one is said in full above the list.
  await expect(page.locator("#mig-view-env")).toContainText("uat.example.com:3306 as uat_app");
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
  await expect((await picker(page)).locator(".mig-match-name")).toHaveText("UAT +1");
});

// --------------------------------------------------- choosing an environment

test("choosing another environment asks Flyway about it, and is remembered", async ({ page }) => {
  await withProject(page);
  await chooseEnv(page, "development");

  await expect((await picker(page)).locator('.mig-envrow[data-env="development"]')).toHaveClass(/selected/);
  await expect((await picker(page)).locator('.mig-envrow[data-env="uat"]')).not.toHaveClass(/selected/);
  await expect.poll(async () => last(await infoCalls(page))?.environment).toBe("development");

  const chosen = (await calls(page)).filter((c) => c.cmd === "flyway_select_environment");
  expect(last(chosen)?.args).toEqual({ id: "fp1", environment: "development" });
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
  await visit(page);
  await openPanel(page);
  await chooseEnv(page, "development");

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

  await chooseEnv(page, "development");
  await expect(page.locator("#mig-out-of-order")).not.toBeChecked();
  await expect.poll(async () => last(await infoCalls(page))?.environment).toBe("development");
  expect(last(await infoCalls(page))?.outOfOrder).toBe(false);

  await chooseEnv(page, "uat");
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
  await chooseEnv(page, "uat");
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
test("Flyway's own refusal is what the list shows", async ({ page }) => {
  await openApp(page, {
    flyway_info: () => {
      throw new Error(
        "Validate failed: Detected failed migration to version 4. Please remove any " +
          "half-completed changes then run repair to fix the schema history.",
      );
    },
  });
  await chooseEnv(page, "uat");
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

test("the view says which project file and folder it is using", async ({ page }) => {
  await withProject(page);
  await expect((await picker(page)).locator(".mig-project-name")).toHaveText("Flyway Connections");
  await expect((await picker(page)).locator(".mig-project-name")).toHaveAttribute("title", PATH);
  await expect(page.locator(".mig-proj-where")).toContainText("flyway.toml");
  await expect(page.locator(".mig-proj-where")).toContainText("/p");
});

/**
 * Where the migrations come from — taken from the files Flyway reported, never
 * from `locations`, which is relative, can be a list and can be overridden per
 * environment.
 */
test("the view says which folder the migrations were actually read from", async ({ page }) => {
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

  await expect(page.locator("#btn-mig-view-apply")).toBeEnabled();
  await expect(page.locator("#btn-mig-view-apply")).toHaveText("Apply 1 pending");
  await page.click("#btn-mig-view-apply");

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
  await chooseEnv(page, "development");
  await expect(page.locator("#btn-mig-view-apply")).toBeEnabled();
  await page.click("#btn-mig-view-apply");

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

  await page.click("#btn-mig-view-apply");
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
    // No row asked for a stopping point, so Flyway runs every pending one.
    target: null,
  });
});

test("confirming applies, and says what Flyway did", async ({ page }) => {
  await withProject(page, {
    flyway_info: () => PENDING_ONLY,
    flyway_migrate: () => ({ executed: 1, target: "5" }),
  });

  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply to uat")').click();

  await expect(page.locator("#flyway-body")).toContainText("applied 1 migration to uat");
  await expect(page.locator("#flyway-body")).toContainText("now at 5");
  // UAT is not open here, so there is nothing to refresh and it says so.
  await expect(page.locator("#flyway-body")).toContainText("may need a refresh");
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

  await chooseEnv(page, "uat");
  await page.locator(".mig").first().waitFor();
  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply to uat")').click();

  await expect(page.locator("#flyway-body")).toContainText("refreshed the schema of UAT");
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

  await expect((await picker(page)).locator('.mig-envrow[data-env="uat"] .chip')).toHaveText("read-only");
  await expect(page.locator("#btn-mig-view-apply")).toBeDisabled();
  await expect(page.locator("#btn-mig-view-apply")).toHaveAttribute("title", /“UAT”.*read-only/);
  await expect(page.locator("#btn-mig-view-repair")).toBeDisabled();
  await expect(page.locator("#btn-mig-view-repair")).toHaveAttribute("title", /“UAT”.*read-only/);
  // Not urged either: something is failed, but nothing may be done about it.
  await expect(page.locator("#btn-mig-view-repair")).not.toHaveClass(/urge/);
});

test("a failed migration blocks apply, and the button explains why", async ({ page }) => {
  await withProject(page);
  await expect(page.locator("#btn-mig-view-apply")).toBeDisabled();
  await expect(page.locator("#btn-mig-view-apply")).toHaveAttribute("title", /failed/);
  await expect(page.locator("#btn-mig-view-apply")).toHaveAttribute("title", /repaired/);
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

  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply to uat")').click();

  await expect(page.locator("#flyway-body")).toContainText("Can't DROP 'weight'");
  await expect(page.locator("#flyway-body")).toContainText("V4__deliberately_broken.sql");
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
  await expect(page.locator("#btn-mig-view-repair")).toBeVisible();
  await expect(page.locator("#btn-mig-view-repair")).toHaveClass(/urge/);

  await page.click("#btn-mig-view-refresh");
  await expect(page.locator('.mig[data-state="failed"]')).toHaveCount(0);
  await expect(page.locator("#btn-mig-view-repair")).toBeVisible();
  await expect(page.locator("#btn-mig-view-repair")).not.toHaveClass(/urge/);
  await expect(page.locator("#btn-mig-view-apply")).toBeEnabled();
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

  await page.click("#btn-mig-view-repair");
  const ask = page.locator("dialog.ask");
  await expect(ask).not.toContainText("remove the failed entry");
  await expect(ask).toContainText("realign the checksum");
  await expect(ask).toContainText("does not run, re-run or undo anything");
  await expect(ask).toContainText("as it was first executed");

  await ask.locator('button:has-text("Repair uat")').click();
  await expect(page.locator("#flyway-body")).toContainText("Aligned applied migration checksums");
  await expect(page.locator("#flyway-body")).toContainText("V1");
});

/** "Repair" sounds like it fixes the database, and it does not. */
test("the repair confirmation says where, and what it does not undo", async ({ page }) => {
  await withProject(page);
  await page.click("#btn-mig-view-repair");

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

/**
 * The stubbed `flyway_repair` answers without the `info` that the real one now
 * carries back from the same invocation (Stage 24 §2.1), so this is also the
 * **fallback** path: no refreshed list came with the run, so the app asks for
 * one. The batched path has its own test.
 */
test("confirming repairs, says what Flyway did, and re-reads the list", async ({ page }) => {
  await withProject(page);
  const before = (await infoCalls(page)).length;

  await page.click("#btn-mig-view-repair");
  await page.locator('dialog.ask button:has-text("Repair uat")').click();

  await expect.poll(async () => commandNames(page)).toContain("flyway_repair");
  const call = (await calls(page)).find((c) => c.cmd === "flyway_repair")!;
  expect(call.args).toEqual({
    projectId: "fp1",
    environment: "uat",
    confirmed: { url: "jdbc:mysql://uat.example.com:3306/flyway", user: "uat_app" },
    program: "",
    // Not repair's own business: it steers the `info` that runs beside it, so
    // the refreshed list matches what the pane was showing.
    outOfOrder: false,
  });
  await expect(page.locator("#flyway-body")).toContainText("Removed failed migrations");
  await expect(page.locator("#flyway-body")).toContainText("V3");
  await expect(page.locator("#flyway-body")).toContainText("database itself is unchanged");
  await expect.poll(async () => (await infoCalls(page)).length).toBeGreaterThan(before);
});

test("a repair that found nothing to do says so, rather than claiming success", async ({
  page,
}) => {
  await withProject(page, {
    flyway_repair: () => ({ actions: [], removed: [], deleted: [], aligned: [] }),
  });

  await page.click("#btn-mig-view-repair");
  await page.locator('dialog.ask button:has-text("Repair uat")').click();

  await expect(page.locator("#flyway-body")).toContainText("found nothing to repair");
  await expect(page.locator("#flyway-body")).not.toContainText("Removed");
});

test("a refused repair shows Flyway's own message", async ({ page }) => {
  await withProject(page, {
    flyway_repair: () => {
      throw new Error("Unable to connect to the database. Check the connection details.");
    },
  });

  await page.click("#btn-mig-view-repair");
  await page.locator('dialog.ask button:has-text("Repair uat")').click();

  await expect(page.locator("#flyway-body")).toContainText("Unable to connect to the database");
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

  await expect(page.locator("#btn-mig-view-repair")).not.toHaveClass(/urge/);
  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply to uat")').click();

  await expect(page.locator("#flyway-body")).toContainText("checksum mismatch");
  await expect(page.locator("#btn-mig-view-repair")).toBeEnabled();
  await expect(page.locator("#btn-mig-view-repair")).toHaveClass(/urge/);
  await expect(page.locator("#btn-mig-view-repair")).toHaveAttribute("title", /asked for a repair/);

  // Asked for *this* environment. Moving to another must not carry it along.
  await chooseEnv(page, "development");
  await expect(page.locator("#btn-mig-view-repair")).not.toHaveClass(/urge/);
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

  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply to uat")').click();

  await expect(page.locator("#flyway-body")).toContainText("Unable to connect");
  await expect(page.locator("#btn-mig-view-repair")).not.toHaveClass(/urge/);
});

// ------------------------------------------- the view, and what it is not

/** D3/D8. One place, and the editor never leaves: no tab is opened for it. */
test("choosing an environment shows it in the panel, and the editor stays", async ({ page }) => {
  await connect(page, flyway([project()]));
  await chooseEnv(page, "uat");
  await page.locator(".mig").first().waitFor();

  await expect(page.locator("#mig-picker-label")).toHaveText("Flyway Connections \u00b7 uat");
  await expect(page.locator("#mig-picker-pop")).toBeHidden();
  await expect(page.locator("#editor")).toBeVisible();
  await expect(page.locator("#editor-head")).toBeVisible();
  await expect(page.locator("#grid")).toBeVisible();
  await expect(page.locator("#script-tabs .stab").filter({ hasText: "Migrations" })).toHaveCount(0);

  await chooseEnv(page, "development");
  await expect(page.locator("#mig-picker-label")).toHaveText("Flyway Connections \u00b7 development");
  await expect(page.locator("#script-tabs .stab").filter({ hasText: "Migrations" })).toHaveCount(0);
});

/** It belongs to no connection, so switching connection leaves it alone. */
test("the panel stays put when the connection changes", async ({ page }) => {
  await connect(page, flyway([project()]));
  await chooseEnv(page, "uat");
  await page.locator(".mig").first().waitFor();

  // A second connection, with its own workspace.
  await page.click(".rail-add");
  await page.locator("#conn-dialog").waitFor({ state: "visible" });
  await page.fill("#conn-dialog input[name=name]", "second");
  await page.click("#conn-ok");
  await page.locator("#conn-dialog").waitFor({ state: "hidden" });

  await expect(page.locator("#mig-pane-body")).toBeVisible();
  await expect(page.locator("#mig-picker-label")).toHaveText("Flyway Connections \u00b7 uat");
  await expect(page.locator(".mig").first()).toBeVisible();
  await expect(page.locator("#editor")).toBeVisible();
});

/**
 * D1. Shut from its rail button, its close button and the key, and it gives
 * all of its width back to the editor — nothing is left on the window edge.
 */
test("the panel shuts from the rail, its close button and the key, and gives its width back", async ({ page }) => {
  await withProject(page);
  const pane = page.locator("#mig-pane");
  const button = page.locator("#btn-migrations");
  const editorOpen = (await page.locator("#editor-pane").boundingBox())!.width;
  expect((await pane.boundingBox())!.width).toBeGreaterThan(280);
  await expect(button).toHaveAttribute("aria-pressed", "true");
  await expect(button).toHaveClass(/\bon\b/);
  await expect(page.locator("#migsplit")).toBeVisible();

  await button.click();
  await expect(pane).toBeHidden();
  await expect(button).toHaveAttribute("aria-pressed", "false");
  await expect(page.locator("#migsplit")).toBeHidden();
  // Both tracks go to zero: the editor takes the panel and its seam.
  const editorShut = (await page.locator("#editor-pane").boundingBox())!.width;
  expect(editorShut).toBeGreaterThan(editorOpen + 280);

  await page.keyboard.press("Control+Shift+M");
  await expect(pane).toBeVisible();
  await page.click("#mig-close");
  await expect(pane).toBeHidden();
  await page.keyboard.press("Control+Shift+M");
  await expect(pane).toBeVisible();
  await page.keyboard.press("Control+Shift+M");
  await expect(pane).toBeHidden();
  // Plain Ctrl+M is the editor's, not the panel's.
  await page.keyboard.press("Control+m");
  await expect(pane).toBeHidden();
});

/** D2. The seam drags the width, measured from the panel's own edge. */
test("the panel's seam drags its width", async ({ page }) => {
  await withProject(page);
  const pane = page.locator("#mig-pane");
  const before = (await pane.boundingBox())!.width;
  const seam = (await page.locator("#migsplit").boundingBox())!;
  await page.mouse.move(seam.x + seam.width / 2, seam.y + seam.height / 2);
  await page.mouse.down();
  await page.mouse.move(seam.x - 80, seam.y + seam.height / 2, { steps: 4 });
  await page.mouse.up();
  const after = (await pane.boundingBox())!.width;
  expect(after).toBeGreaterThan(before + 60);

  // Shut and open again: the dragged width survives.
  await page.click("#btn-migrations");
  await page.click("#btn-migrations");
  expect(Math.abs((await pane.boundingBox())!.width - after)).toBeLessThan(2);
});

/** …and choosing one brings it back, with its own account in it. */
test("an environment restores the Last run section", async ({ page }) => {
  await installBackend(page, flyway([]));
  await visit(page);
  await openPanel(page);
  await expect(page.locator("#mig-report")).toBeHidden();

  await page.click("#mig-pane-body .mig-empty button");
  await page.locator(".mig").first().waitFor();
  await expect(page.locator("#mig-report")).toBeVisible();
  await expect(page.locator("#flyway-body")).toContainText("Nothing has run here yet");
});

/**
 * Decision 5, and the seam's own measurement.
 *
 * Both right-hand panels can be open at once, migrations the inner one — it is
 * about the SQL in the editor, so it sits nearer it. And the seam measures from
 * **the panel's own right edge**, not the window's, which only matters once
 * something is open beyond it: measured from the window, the assistant's 380px
 * offsets every drag and the panel jumps to the seam's ceiling instead of
 * following the pointer. §7.1 records that bug and its fix; nothing caught it,
 * because every other drag here happens with the assistant shut.
 */
test("both right-hand panels open at once, and the seam still measures from the panel", async ({
  page,
}) => {
  await withProject(page);
  await page.click("#btn-assistant");
  await expect(page.locator("#chat-pane")).toBeVisible();
  await expect(page.locator("#mig-pane")).toBeVisible();

  const mig = (await page.locator("#mig-pane").boundingBox())!;
  const chat = (await page.locator("#chat-pane").boundingBox())!;
  expect(mig.x + mig.width).toBeLessThanOrEqual(chat.x);

  // Rightwards, which narrows it: the floor is 300px and nothing clamps this.
  const seam = (await page.locator("#migsplit").boundingBox())!;
  await page.mouse.move(seam.x + seam.width / 2, seam.y + seam.height / 2);
  await page.mouse.down();
  await page.mouse.move(seam.x + 60, seam.y + seam.height / 2, { steps: 4 });
  await page.mouse.up();

  // 60px of pointer is 60px of panel. From the window it would have grown
  // instead, to the ceiling, because the measurement would be 387px too wide.
  const after = (await page.locator("#mig-pane").boundingBox())!.width;
  expect(after).toBeLessThan(mig.width - 40);
  expect(Math.abs(after - (mig.width - 60))).toBeLessThan(10);
});

/** The picker shuts by choosing, by Escape wherever focus is, and by a click away. */
test("the environment picker shuts on Escape and on a click elsewhere", async ({ page }) => {
  await withProject(page);
  await picker(page);
  // Focus is on the picker's button after the click, not inside the popover.
  await page.keyboard.press("Escape");
  await expect(page.locator("#mig-picker-pop")).toBeHidden();
  await expect(page.locator("#mig-picker")).toHaveAttribute("aria-expanded", "false");

  await picker(page);
  await page.locator("#editor .cm-content").click();
  await expect(page.locator("#mig-picker-pop")).toBeHidden();
});

/** D8. Nothing about migrations is a tab, so nothing of it reaches the session. */
test("migrations are never written to the session", async ({ page }) => {
  await connect(page, flyway([project()]));
  await chooseEnv(page, "uat");
  await page.locator(".mig").first().waitFor();
  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("select 2");

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

/** The labels are spelled out: "…" was read as a label cut short. */
test("the buttons say what they do, in full", async ({ page }) => {
  await withProject(page, { flyway_info: () => TWO_PENDING });
  await expect(page.locator("#btn-mig-view-apply")).toHaveText("Apply 2 pending");
  await expect(page.locator("#btn-mig-view-repair")).toHaveText("Repair schema history");
});

/**
 * **Apply up to here.** Measured against Flyway 13.5.0 Community on
 * 2026-09-28: `-cherryPick` is refused by this edition, `-target=N` is not. So
 * a row offers a stopping point, and the confirmation names only what will run.
 */
test("a pending row applies up to itself, and says only what will run", async ({ page }) => {
  await withProject(page, { flyway_info: () => TWO_PENDING });

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
  await withProject(page, { flyway_info: () => TWO_PENDING });
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
  await withProject(page, {
    flyway_info: () => [
      { ...TWO_PENDING[1], version: null, description: "repeatable view", category: "Repeatable" },
      TWO_PENDING[2],
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
  await withProject(page, {
    flyway_info: () => TWO_PENDING,
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

/** D7. Shut, the rail button says what is waiting — from a reading, never by asking. */
test("shut, the rail button says how many are pending once a reading exists", async ({ page }) => {
  await installBackend(page, flyway([project()], { flyway_info: () => TWO_PENDING }));
  await visit(page);
  // Nothing read yet: the button says nothing, and nothing asked Flyway.
  await expect(page.locator("#mig-pending")).toBeHidden();
  expect(await commandNames(page)).not.toContain("flyway_info");

  await openPanel(page);
  await page.locator(".mig").first().waitFor();
  await page.click("#btn-migrations");
  await expect(page.locator("#mig-pane")).toBeHidden();
  await expect(page.locator("#mig-pending")).toBeVisible();
  await expect(page.locator("#mig-pending")).toHaveText("2");
  await expect(page.locator("#mig-pending")).not.toHaveClass(/\bbad\b/);
  await expect(page.locator("#btn-migrations")).toHaveAttribute("title", /2 pending/);
});

/**
 * D7's other half, and why the badge is on the rail button rather than in the
 * panel: it is **left showing while the panel is open**, because a badge that
 * vanished on opening would read as "nothing pending". A failure is said in its
 * own words and in red, since a failed migration blocks every apply until it
 * is repaired.
 */
test("the badge says what failed, in red, and stays while the panel is open", async ({ page }) => {
  await withProject(page);
  const badge = page.locator("#mig-pending");

  await expect(page.locator("#mig-pane")).toBeVisible();
  await expect(badge).toBeVisible();
  await expect(badge).toHaveText("1");
  await expect(badge).toHaveClass(/\bbad\b/);
  await expect(page.locator("#btn-migrations")).toHaveAttribute("title", /1 failed/);
});

/**
 * And cleared the moment the environment changes, so a count never describes
 * the environment you have left. `uat` has a failure; `development`'s answer is
 * held, and in between the button has to say nothing rather than "1 failed".
 */
test("the badge is cleared while another environment is being read", async ({ page }) => {
  const gate = held();
  await withProject(page, {
    flyway_info: (a: Record<string, unknown>) =>
      a.environment === "development" ? gate.promise : MIGRATIONS,
  });
  const badge = page.locator("#mig-pending");
  await expect(badge).toHaveText("1");

  await chooseEnv(page, "development");
  await expect(badge).toBeHidden();

  gate.release(TWO_PENDING);
  await expect(badge).toBeVisible();
  await expect(badge).toHaveText("2");
  await expect(badge).not.toHaveClass(/\bbad\b/);
});

// ------------------------------------------------------- waiting for Flyway

/**
 * **The button that started it is where the waiting shows.**
 *
 * Apply and Repair disabled themselves and wrote "Flyway is applying…" beside
 * the list, and on Windows neither was visible: spawning Flyway opened a
 * console window over the app, so the click looked like it had done nothing.
 * The window is gone (`proc.rs` passes `CREATE_NO_WINDOW`), and the wait is now
 * on the control somebody is already looking at.
 */

/**
 * Answer the first `flyway_info` normally and gate every call after it.
 *
 * The migrations view loads its list on arrival, so gating the command outright
 * leaves nothing on screen and the click under test never becomes possible. The
 * pre-check a click makes is the *second* call.
 */
function gateAfterFirst<T>(first: T) {
  let seen = 0;
  const gate = held();
  return {
    stub: () => (seen++ === 0 ? (first as unknown) : gate.promise),
    release: gate.release,
  };
}

/** A gate, so "while Flyway is running" is a state rather than a race. */
function held() {
  let release!: (v: unknown) => void;
  const promise = new Promise<unknown>((r) => {
    release = r;
  });
  return { promise, release };
}

test("Repair shows a spinner on its own button until Flyway answers", async ({ page }) => {
  const gate = held();
  await withProject(page, { flyway_repair: () => gate.promise });

  await page.click("#btn-mig-view-repair");
  await page.locator('dialog.ask button:has-text("Repair uat")').click();

  const btn = page.locator("#btn-mig-view-repair");
  await expect(btn.locator(".spinner")).toBeVisible();
  await expect(btn).toContainText("Repairing");
  await expect(btn).toBeDisabled();
  // Said out loud, for anyone who cannot see it turn.
  await expect(btn).toHaveAttribute("aria-busy", "true");

  gate.release({ actions: ["Removed failed migrations"], removed: [], deleted: [], aligned: [] });
  await expect(btn.locator(".spinner")).toHaveCount(0);
  await expect(btn).not.toHaveAttribute("aria-busy", "true");
  await expect(btn).toContainText("Repair");
});

test("Apply shows a spinner on its own button until Flyway answers", async ({ page }) => {
  const gate = held();
  // `PENDING_ONLY`, because the default fixture has a failed migration and
  // Flyway will not apply anything until that is repaired — the Apply button is
  // correctly disabled there.
  await withProject(page, {
    flyway_info: () => PENDING_ONLY,
    flyway_migrate: () => gate.promise,
  });

  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply")').last().click();

  const btn = page.locator("#btn-mig-view-apply");
  await expect(btn.locator(".spinner")).toBeVisible();
  await expect(btn).toContainText("Applying");
  await expect(btn).toBeDisabled();

  gate.release({ executed: 1, target: "2" });
  await expect(btn.locator(".spinner")).toHaveCount(0);
});

/**
 * A failure must not leave a button spinning: the undo runs in a `finally`, so
 * the control comes back however the call ended.
 */
test("a Flyway failure gives the button back", async ({ page }) => {
  await withProject(page, {
    flyway_repair: () => {
      throw new Error("Flyway was not found at \"flyway\".");
    },
  });

  await page.click("#btn-mig-view-repair");
  await page.locator('dialog.ask button:has-text("Repair uat")').click();

  await expect(page.locator("#flyway-body")).toContainText("was not found");
  const btn = page.locator("#btn-mig-view-repair");
  await expect(btn.locator(".spinner")).toHaveCount(0);
  await expect(btn).toContainText("Repair");
});

/**
 * **The check before the confirmation is a Flyway run too.**
 *
 * Apply asks Flyway what is pending so the dialog can name it — a second JVM
 * start, seconds long, which reported nothing. Reported as "it does not show
 * any loader until we confirm".
 */
test("Apply shows the wait while it works out what is pending", async ({ page }) => {
  const gate = gateAfterFirst(PENDING_ONLY);
  await withProject(page, { flyway_info: gate.stub });
  const btn = page.locator("#btn-mig-view-apply");

  await page.click("#btn-mig-view-apply");
  await expect(btn.locator(".spinner")).toBeVisible();
  await expect(btn).toContainText("Checking");
  // And no dialog yet — this is the part that used to look like nothing.
  await expect(page.locator("dialog.ask")).toHaveCount(0);

  gate.release(PENDING_ONLY);
  await expect(page.locator("dialog.ask")).toBeVisible();
  // Given back for the dialog: nothing is running while somebody reads it.
  await expect(btn.locator(".spinner")).toHaveCount(0);
});

test("Repair shows the wait while it checks, before asking", async ({ page }) => {
  const gate = gateAfterFirst(MIGRATIONS);
  await withProject(page, { flyway_info: gate.stub });
  const btn = page.locator("#btn-mig-view-repair");

  await page.click("#btn-mig-view-repair");
  await expect(btn.locator(".spinner")).toBeVisible();
  await expect(btn).toContainText("Checking");
  await expect(page.locator("dialog.ask")).toHaveCount(0);

  gate.release(MIGRATIONS);
  await expect(page.locator("dialog.ask")).toBeVisible();
});

/**
 * **A migrations tab must not show the last query's rows.** The grid is shared
 * with the editor, and activating a migrations tab returned before anything
 * repainted it — so somebody else's result set sat there looking like Flyway's.
 */
test("opening migrations leaves the editor's result set where it is", async ({ page }) => {
  await connect(
    page,
    flyway([project()], {
      statement_at_cursor: () => ({ sql: "SELECT 1", start: 0 }),
      run_script: () => rowsResult([{ name: "n" }], [[1]]),
    }),
  );
  // A real result in the grid first.
  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("select 1");
  await page.keyboard.press("Control+Enter");
  await expect(page.locator("#grid table")).toBeVisible();

  await chooseEnv(page, "uat");
  await expect(page.locator(".mig").first()).toBeVisible();
  // Since Stage 28 the two sit side by side: the query's rows stay in the grid,
  // and Flyway's account is the panel's own.
  await expect(page.locator("#grid table")).toBeVisible();
  await expect(page.locator("#res-title")).toHaveText("Results");
  await expect(page.locator("#mig-report #flyway-body")).toContainText("Nothing has run here yet");
});

/** And an outcome survives a trip to another tab and back. */
test("what Flyway did stays with its environment", async ({ page }) => {
  // Connected, so there is a SQL tab to switch away to: opening the
  // environment on its own creates the only tab there is.
  await connect(page, flyway([project()]));
  await chooseEnv(page, "uat");
  await page.locator(".mig").first().waitFor();

  await page.click("#btn-mig-view-repair");
  await page.locator('dialog.ask button:has-text("Repair uat")').click();
  await expect(page.locator("#flyway-body")).toContainText("Removed failed migrations");

  // Another environment has its own (empty) account, and uat keeps its own.
  await chooseEnv(page, "development");
  await expect(page.locator("#flyway-body")).toContainText("Nothing has run here yet");
  await chooseEnv(page, "uat");
  await expect(page.locator("#flyway-body")).toContainText("Removed failed migrations");
});

/**
 * **A version is the migration's identity, so it gets the width it needs.**
 *
 * The cell was a fixed 56px, which fits "V1" and spills a timestamp version
 * over the description: measured, `20260214093000` needs 93px in this font and
 * overflowed by 37, at every window width.
 */
test("a long version does not run over the description", async ({ page }) => {
  await withProject(page, {
    // Pending, so the group is open and the cells have geometry to measure.
    flyway_info: () => [
      { ...PENDING_ONLY[0], version: "20260214093000", description: "add a column" },
      { ...PENDING_ONLY[0], version: "1", description: "create widgets" },
    ],
  });

  const cells = await page.locator(".mig .v").evaluateAll((els) =>
    els.map((e) => ({
      text: e.textContent,
      overflowsBy: e.scrollWidth - e.clientWidth,
    })),
  );
  expect(cells.length).toBeGreaterThan(0);
  for (const c of cells) {
    expect(c.overflowsBy, `version ${c.text} spills out of its cell`).toBeLessThanOrEqual(0);
  }
  // Short versions still line up with each other rather than each hugging text.
  const widths = await page.locator(".mig .v").evaluateAll((els) =>
    els.map((e) => Math.round(e.getBoundingClientRect().width)),
  );
  expect(Math.min(...widths)).toBeGreaterThanOrEqual(56);
});

// ------------------------------------------- L2 and L3: waiting, and the output

/**
 * **The first open of the tab waits the way everything else does.**
 *
 * `flyway_info` starts a JVM and talks to the server, so it is the same
 * seconds of waiting as an apply — but it used to announce itself with one
 * grey line of text where the list goes, which read as a list that had failed
 * to load rather than as work in progress. Reported from use.
 */
test("the first open of the tab waits with the loader, not a line of text", async ({ page }) => {
  const gate = held();
  await openApp(page, { flyway_info: () => gate.promise });
  await chooseEnv(page, "uat");

  const strip = page.locator("#mig-progress");
  await expect(strip).toBeVisible();
  await expect(strip.locator(".spinner")).toBeVisible();
  await expect(strip).toContainText("Asking Flyway about uat");
  await expect(strip.locator(".mig-progress-what")).toHaveText("flyway info");
  // And no grey sentence pretending to be the list.
  await expect(page.locator("#mig-view-list .mig-empty")).toHaveCount(0);

  gate.release(MIGRATIONS);
  await expect(page.locator(".mig").first()).toBeVisible();
  await expect(strip).toBeHidden();
});

/**
 * Re-asking the environment already on screen keeps its rows under the
 * spinner. Blanking them and painting them again is a flash that says
 * something changed when nothing has.
 */
test("refreshing keeps the rows on screen while it re-asks", async ({ page }) => {
  const gate = gateAfterFirst(MIGRATIONS);
  await withProject(page, { flyway_info: gate.stub });

  await page.click("#btn-mig-view-refresh");
  await expect(page.locator("#mig-progress")).toBeVisible();
  await expect(page.locator(".mig").first()).toBeVisible();

  gate.release(MIGRATIONS);
  await expect(page.locator("#mig-progress")).toBeHidden();
});

/**
 * **L2. The strip says what is known and admits what is not.**
 *
 * Flyway runs with `-outputType=json`, which emits one document when it
 * finishes — so the app knows the operation, the environment and which
 * migrations it asked for, and does *not* know which one is in flight. A
 * progress bar would claim the second, so there is a spinner, an elapsed count
 * and a sentence saying Flyway reports at the end.
 *
 * There is no Stop, deliberately: killing a JVM part-way through a migration is
 * how a schema history ends up locked.
 */
test("while applying, the strip names the migrations and admits what it cannot know", async ({
  page,
}) => {
  const gate = gateAfterFirst(PENDING_ONLY);
  await withProject(page, { flyway_info: gate.stub, flyway_migrate: () => held().promise });

  await page.click("#btn-mig-view-apply");
  gate.release(PENDING_ONLY);
  await page.locator('dialog.ask button:has-text("Apply")').last().click();

  const strip = page.locator("#mig-progress");
  await expect(strip).toBeVisible();
  await expect(strip.locator(".spinner")).toBeVisible();
  await expect(strip).toContainText("Applying 1 migration to uat");
  // Which ones, by version.
  await expect(strip.locator(".mig-progress-what")).toContainText("V5");
  await expect(strip).toContainText("Flyway reports when it finishes, not as it goes");
  await expect(strip).toHaveAttribute("role", "status");
  // Nothing that offers to kill a running migration.
  await expect(strip.locator("button")).toHaveCount(0);
});

test("the strip goes when the run ends", async ({ page }) => {
  await withProject(page, { flyway_info: () => PENDING_ONLY });
  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply")').last().click();
  await expect(page.locator("#flyway-body")).toContainText("applied 1 migration");
  await expect(page.locator("#mig-progress")).toBeHidden();
});

/**
 * **L3. Flyway's report is rendered, not printed.**
 *
 * `stdout` is a JSON document, so printing it raw would put a wall of braces in
 * a pane somebody opened to read a report. The fields come from the captured
 * fixtures in `flywaycli.rs`.
 */
/**
 * **Real output**, captured from Flyway 13.5.0 against the `dev/flyway`
 * fixture on 2026-10-02 (`mise run db-up && mise run flyway-up`).
 *
 * Not invented, and the first version of this fixture was: it gave each
 * migration a `state` of "Success", which a migrate report does not have at
 * all. The test passed and proved nothing about real output.
 *
 * The `error` object's `cause` chain and its three `stackTrace` strings are cut
 * out here — the whole document was 18,480 bytes, of which 527 was the message.
 * The renderer must never show them, so carrying them in a test file would
 * prove nothing either.
 */
const MIGRATE_OK = "{\n  \"database\": \"flyway_qa\",\n  \"databaseType\": \"MySQL\",\n  \"flywayVersion\": \"13.5.0\",\n  \"initialSchemaVersion\": null,\n  \"licenseFailed\": false,\n  \"migrations\": [\n    {\n      \"category\": \"Versioned\",\n      \"description\": \"create widgets\",\n      \"executionTime\": 27,\n      \"filepath\": \"/home/lenovo/Documents/Projects/db-query/dev/flyway/migrations/V1__create_widgets.sql\",\n      \"type\": \"SQL\",\n      \"version\": \"1\"\n    },\n    {\n      \"category\": \"Versioned\",\n      \"description\": \"seed widgets\",\n      \"executionTime\": 4,\n      \"filepath\": \"/home/lenovo/Documents/Projects/db-query/dev/flyway/migrations/V2__seed_widgets.sql\",\n      \"type\": \"SQL\",\n      \"version\": \"2\"\n    },\n    {\n      \"category\": \"Versioned\",\n      \"description\": \"add colour\",\n      \"executionTime\": 13,\n      \"filepath\": \"/home/lenovo/Documents/Projects/db-query/dev/flyway/migrations/V3__add_colour.sql\",\n      \"type\": \"SQL\",\n      \"version\": \"3\"\n    }\n  ],\n  \"migrationsExecuted\": 3,\n  \"operation\": \"migrate\",\n  \"schemaName\": \"\",\n  \"success\": true,\n  \"targetSchemaVersion\": \"3\",\n  \"timestamp\": \"2026-10-02T19:16:06.00892756-03:00\",\n  \"totalMigrationTime\": 44,\n  \"warnings\": []\n}";

/**
 * The same fixture, stopped by its deliberately broken V4 — **with its real
 * `cause` and a real `stackTrace`**, shortened to six frames.
 *
 * It has to carry them. The first version of this fixture had the traces cut
 * out for size, which made the test's "and none of the Java" assertions
 * vacuous: falsifying the renderer by printing the whole error object still
 * passed, because there was no Java in the fixture to leak.
 */
const MIGRATE_FAILED = "{\n  \"database\": \"flyway_dev\",\n  \"databaseType\": \"MySQL\",\n  \"error\": {\n    \"errorCode\": \"FAILED_VERSIONED_MIGRATION\",\n    \"message\": \"Failed to execute script V4__deliberately_broken.sql against development environment\\n------------------------------------------------------------------------------------\\nSQL State  : 42000\\nError Code : 1091\\nMessage    : (conn=12) Can't DROP 'weight'; check that column/key exists\\nLocation   : migrations/V4__deliberately_broken.sql (/home/lenovo/Documents/Projects/db-query/dev/flyway/migrations/V4__deliberately_broken.sql)\\nLine       : 4\\nStatement  : Run Flyway with -X option to see the actual statement causing the problem\\n\",\n    \"lineNumber\": 4,\n    \"path\": \"/home/lenovo/Documents/Projects/db-query/dev/flyway/migrations/V4__deliberately_broken.sql\",\n    \"cause\": {\n      \"message\": \"Failed to execute script V4__deliberately_broken.sql against development environment\\n------------------------------------------------------------------------------------\\nSQL State  : 42000\\nError Code : 1091\\nMessage    : (conn=12) Can't DROP 'weight'; check that column/key exists\\nLocation   : migrations/V4__deliberately_broken.sql (/home/lenovo/Documents/Projects/db-query/dev/flyway/migrations/V4__deliberately_broken.sql)\\nLine       : 4\\nStatement  : Run Flyway with -X option to see the actual statement causing the problem\\n\",\n      \"stackTrace\": \"org.flywaydb.core.internal.sqlscript.FlywaySqlScriptException: Failed to execute script V4__deliberately_broken.sql against development environment\\n------------------------------------------------------------------------------------\\nSQL State  : 42000\\nError Code : 1091\\nMessage    : (conn=12) Can't DROP 'weight'; check that column/key exists\\nLocation   : migrations/V4__deliberately_broken.sql (/home/lenovo/Documents/Projects/db-query/dev/flyway/migrations/V4__deliberately_broken.sql)\\n\\t\\u2026 23 more\"\n    }\n  },\n  \"flywayVersion\": \"13.5.0\",\n  \"initialSchemaVersion\": null,\n  \"licenseFailed\": false,\n  \"migrations\": [\n    {\n      \"category\": \"Versioned\",\n      \"description\": \"create widgets\",\n      \"executionTime\": 24,\n      \"filepath\": \"/home/lenovo/Documents/Projects/db-query/dev/flyway/migrations/V1__create_widgets.sql\",\n      \"type\": \"SQL\",\n      \"version\": \"1\"\n    },\n    {\n      \"category\": \"Versioned\",\n      \"description\": \"seed widgets\",\n      \"executionTime\": 3,\n      \"filepath\": \"/home/lenovo/Documents/Projects/db-query/dev/flyway/migrations/V2__seed_widgets.sql\",\n      \"type\": \"SQL\",\n      \"version\": \"2\"\n    },\n    {\n      \"category\": \"Versioned\",\n      \"description\": \"add colour\",\n      \"executionTime\": 11,\n      \"filepath\": \"/home/lenovo/Documents/Projects/db-query/dev/flyway/migrations/V3__add_colour.sql\",\n      \"type\": \"SQL\",\n      \"version\": \"3\"\n    }\n  ],\n  \"migrationsExecuted\": 3,\n  \"operation\": \"migrate\",\n  \"schemaName\": \"\",\n  \"success\": false,\n  \"targetSchemaVersion\": null,\n  \"timestamp\": \"2026-10-02T19:15:42.170597838-03:00\",\n  \"totalMigrationTime\": 38,\n  \"warnings\": []\n}";

const REPORT = { op: "migrate", code: 0, stdout: MIGRATE_OK, stderr: "" };

test("the output pane renders Flyway's report rather than its JSON", async ({ page }) => {
  await withProject(page, {
    flyway_info: () => PENDING_ONLY,
    flyway_last_run: () => REPORT,
  });
  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply")').last().click();

  const body = page.locator("#flyway-body");
  await expect(body).toContainText("Flyway 13.5.0");
  await expect(body).toContainText("database flyway_qa");
  await expect(body).toContainText("3 executed");
  await expect(body).toContainText("now at 3");
  // A migrate report carries no `state`, so a listed migration is one that ran.
  await expect(body.locator(".fw-mig").first()).toContainText("create widgets");
  await expect(body.locator(".fw-mig").first()).toContainText("Applied");
  await expect(body.locator(".fw-mig").first()).toContainText("27 ms");
  // Rendered: no braces from the document it came from.
  await expect(body).not.toContainText("flywayVersion");
  await expect(body).not.toContainText("{");
});

/** Anything that will not parse is shown verbatim — that is when you need it. */
test("output that is not a report is shown as it came", async ({ page }) => {
  await withProject(page, {
    flyway_info: () => PENDING_ONLY,
    flyway_last_run: () => ({
      op: "migrate",
      code: 1,
      stdout: "Error: Unable to obtain connection from database",
      stderr: "Picked up JAVA_TOOL_OPTIONS: -Xmx512m",
    }),
  });
  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply")').last().click();

  const body = page.locator("#flyway-body");
  await expect(body).toContainText("Unable to obtain connection");
  // Including the JVM's own noise, which is never inside the report.
  await expect(body).toContainText("JAVA_TOOL_OPTIONS");
});

/**
 * Stage 26 moved this: the output has no Hide button of its own, because the
 * pane it lives in grew a header that collapses either kind of output. What is
 * still asserted is the same property — it can be shut, and it says which
 * output it is while shut.
 */
test("the last run collapses from its own header, and the result pane never shows it", async ({ page }) => {
  await withProject(page);
  // D6: the result pane is about SQL only.
  await expect(page.locator("#res-title")).toHaveText("Results");
  await expect(page.locator("#results-pane #flyway-body")).toHaveCount(0);

  const head = page.locator("#mig-report-toggle");
  await expect(page.locator("#flyway-body")).toBeVisible();
  await expect(head).toHaveAttribute("aria-expanded", "true");
  await head.click();
  await expect(page.locator("#flyway-body")).toBeHidden();
  await expect(head).toHaveAttribute("aria-expanded", "false");
  await head.click();
  await expect(page.locator("#flyway-body")).toBeVisible();
});

/** Before anything has run, it says so rather than sitting blank. */
test("an untouched environment says nothing has run", async ({ page }) => {
  await withProject(page);
  await expect(page.locator("#flyway-body")).toContainText("Nothing has run here yet");
});

/**
 * **A failed migrate still lists what succeeded.** Measured: Flyway applied
 * V1, V2 and V3, then V4 failed — and the report carries all three plus the
 * error. That ordering is the whole value of the pane after a failure.
 *
 * And the stack trace stays out. The real document was 18,480 bytes, of which
 * the message was 527; the rest was a `cause` chain three deep, each with its
 * own `stackTrace`. Printing the document raw would bury the one line that
 * says which file and which SQL.
 */
test("a failed run shows what ran, then why it stopped, without the Java", async ({ page }) => {
  await withProject(page, {
    flyway_info: () => PENDING_ONLY,
    flyway_migrate: () => {
      throw new Error("Flyway refused");
    },
    flyway_last_run: () => ({
      op: "migrate",
      code: 1,
      stdout: MIGRATE_FAILED,
      stderr: "",
    }),
  });

  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply")').last().click();

  const body = page.locator("#flyway-body");
  // The three that worked, in order.
  await expect(body.locator(".fw-mig")).toHaveCount(3);
  await expect(body.locator(".fw-mig").nth(0)).toContainText("create widgets");
  await expect(body.locator(".fw-mig").nth(2)).toContainText("add colour");
  // Then the reason, naming the file and the SQL error.
  await expect(body.locator(".fw-err")).toContainText("V4__deliberately_broken.sql");
  await expect(body.locator(".fw-err")).toContainText("Can't DROP 'weight'");

  // None of the Java.
  await expect(body).not.toContainText("org.flywaydb");
  await expect(body).not.toContainText("stackTrace");
  await expect(body).not.toContainText("MariaDbStatement");
});

/**
 * **Flyway's third shape: a refusal.** `{"error": {…}}` and nothing else —
 * no `operation`, no `migrations`. Captured from the fixture on 2026-10-02 by
 * running the broken environment twice: the second run is stopped by validation
 * before it starts.
 *
 * Found by a live test that asserted `operation` was "migrate" and got `null`.
 * The pane has to render this too, falling back to the operation it asked for.
 */
const REFUSAL = JSON.stringify({
  error: {
    errorCode: "VALIDATE_ERROR",
    message:
      "Validate failed: Migrations have failed validation\n" +
      "Detected failed migration to version 4 (deliberately broken).\n" +
      "Please remove any half-completed changes then run repair to fix the schema history.",
  },
});

test("a refusal, which carries nothing but a message, still reads", async ({ page }) => {
  await withProject(page, {
    flyway_info: () => PENDING_ONLY,
    flyway_migrate: () => {
      throw new Error("Validate failed");
    },
    flyway_last_run: () => ({ op: "migrate", code: 1, stdout: REFUSAL, stderr: "" }),
  });

  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply")').last().click();

  const body = page.locator("#flyway-body");
  await expect(body.locator(".fw-err")).toContainText("Detected failed migration to version 4");
  await expect(body.locator(".fw-err")).toContainText("run repair");
  // No rows and no summary, because the document has neither.
  await expect(body.locator(".fw-mig")).toHaveCount(0);
  // And the header still says which operation it was.
  await expect(body.locator(".fw-head")).toContainText("migrate");
});

// ------------------------------------------------ Stage 24: fewer JVMs

/**
 * Why any of this exists, measured 2026-10-19: `flyway info` costs ~4.8s
 * whatever it finds, because almost all of it is JVM start — four migrations
 * cost the same as a thousand, and `flyway --version` costs the same again.
 * The reported workflow spent ten of those on three operations.
 *
 * `fingerprinted` stubs the cheap half: the app asks the disk whether the
 * migration folders have moved, and `flyway_fingerprint` answers with whatever
 * the reading claimed, so "nothing changed" is the default and a test that
 * wants a change says so.
 */
function fingerprinted(extra: Record<string, unknown> = {}, fingerprint = "stub-fingerprint") {
  return { flyway_fingerprint: () => fingerprint, ...extra };
}

/**
 * How many times Flyway has been asked for `info`, **once it has stopped going
 * up**.
 *
 * `expect.poll(…).toBe(n)` returns the moment it matches, so it cannot say
 * "and nothing further happened" — it would pass on the sample taken before
 * the call it is meant to catch. Two equal readings in a row can. This is the
 * same settling the tab-strip scroll test uses, and for the same reason.
 */
async function settledInfoCalls(page: Page): Promise<number> {
  let last = -1;
  await expect
    .poll(async () => {
      const now = (await infoCalls(page)).length;
      const same = now === last;
      last = now;
      return same;
    })
    .toBe(true);
  return last;
}

test("a write dialog after a look asks the disk, not Flyway again", async ({ page }) => {
  await withProject(page, fingerprinted());
  const before = (await infoCalls(page)).length;

  await page.click("#btn-mig-view-repair");
  await expect(page.locator("dialog.ask")).toBeVisible();

  expect(await commandNames(page)).toContain("flyway_fingerprint");
  expect((await infoCalls(page)).length).toBe(before);
  // And it says where its description came from, rather than implying it just
  // looked — the one thing a fingerprint cannot prove is what somebody else
  // did to that database.
  await expect(page.locator("dialog.ask")).toContainText("From the reading taken");
  await expect(page.locator("dialog.ask")).toContainText("have not changed since");
});

test("an edited migration file makes the next dialog ask Flyway", async ({ page }) => {
  // The reported workflow: scripts commented out, run by hand, uncommented.
  // The fingerprint answers something else, so the reading cannot be reused.
  await withProject(page, fingerprinted({}, "the-files-moved"));
  const before = (await infoCalls(page)).length;

  await page.click("#btn-mig-view-repair");
  await expect(page.locator("dialog.ask")).toBeVisible();

  expect((await infoCalls(page)).length).toBeGreaterThan(before);
  await expect(page.locator("dialog.ask")).toContainText("Checked just now");
});

test("Check first asks Flyway however untouched the files are", async ({ page }) => {
  await withProject(page, fingerprinted());

  await page.check("#mig-check-first");
  const before = (await infoCalls(page)).length;

  await page.click("#btn-mig-view-repair");
  await expect(page.locator("dialog.ask")).toBeVisible();

  expect((await infoCalls(page)).length).toBeGreaterThan(before);
  await expect(page.locator("dialog.ask")).toContainText("Checked just now");
  // Ticking it is not itself a reason to re-read the pane: that would spend
  // the very Flyway start the box exists to control.
  expect(await commandNames(page)).toContain("flyway_info");
});

test("Check first belongs to the environment, not to the window", async ({ page }) => {
  await withProject(page, fingerprinted());

  await page.check("#mig-check-first");
  await chooseEnv(page, "development");
  await expect(page.locator("#mig-check-first")).not.toBeChecked();

  await chooseEnv(page, "uat");
  await expect(page.locator("#mig-check-first")).toBeChecked();
});

test("a repair that brings the refreshed list back does not ask for it again", async ({ page }) => {
  await withProject(
    page,
    fingerprinted({
      flyway_repair: () => ({
        actions: ["Removed failed migrations"],
        removed: [{ version: "3", description: "deliberately broken" }],
        deleted: [],
        aligned: [],
        // What the real command now returns: the `info` that ran in the same
        // JVM, which is both free and fresher than a second process.
        info: {
          migrations: PENDING_ONLY,
          folders: ["/srv/app/migrations"],
          fingerprint: "after-the-repair",
        },
      }),
    }),
  );

  const before = await settledInfoCalls(page);

  await page.click("#btn-mig-view-repair");
  await page.locator('dialog.ask button:has-text("Repair uat")').click();
  await expect.poll(async () => commandNames(page)).toContain("flyway_repair");

  await expect(page.locator("#flyway-body")).toContainText("Removed failed migrations");
  // One JVM for the whole click: the dialog reused the reading and the list
  // came back with the repair, so Flyway was never asked for `info`…
  expect(await settledInfoCalls(page)).toBe(before);
  // …and it is the list on screen: the repaired migration is pending again.
  const pending = page.locator('.mig-group[data-group="pending"]');
  await expect(pending.locator(".mig-group-count")).toHaveText(String(PENDING_ONLY.length));
  await expect(pending.locator(".mig .d")).toHaveText([PENDING_ONLY[0].description]);
});

test("an apply that brings the refreshed list back does not ask for it again", async ({ page }) => {
  await withProject(
    page,
    fingerprinted({
      flyway_info: () => PENDING_ONLY,
      flyway_migrate: () => ({
        executed: 1,
        target: "2",
        info: {
          migrations: [MIGRATIONS[0]],
          folders: ["/srv/app/migrations"],
          fingerprint: "after-the-apply",
        },
      }),
    }),
  );

  const before = await settledInfoCalls(page);

  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply")').last().click();
  await expect.poll(async () => commandNames(page)).toContain("flyway_migrate");

  await expect(page.locator("#flyway-body")).toContainText("applied 1 migration");
  expect(await settledInfoCalls(page)).toBe(before);
});

test("a failed apply still ends with a list on screen", async ({ page }) => {
  // No `info` comes back from a failed run — Flyway stops the batch at the
  // operation that failed — so the app asks for one itself, as it always did.
  await withProject(
    page,
    fingerprinted({
      flyway_info: () => PENDING_ONLY,
      flyway_migrate: () => {
        throw { message: "Validate failed", suggestsRepair: true };
      },
    }),
  );
  const before = (await infoCalls(page)).length;

  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply")').last().click();

  await expect.poll(async () => (await infoCalls(page)).length).toBeGreaterThan(before);
  await expect(page.locator("#btn-mig-view-repair")).toHaveClass(/urge/);
});

/**
 * Shape, not behaviour — and the narrow pane, because that is where it breaks.
 *
 * `Check first` is a fourth item in a flex row that held three. Measured while
 * writing this: at 640px the row runs out of width and, without wrapping,
 * simply cuts the last item off. No behavioural test can see that; every
 * button still works perfectly while being invisible.
 */
test("the actions row fits its toggles and buttons in the narrowest panel", async ({ page }) => {
  await withProject(page, fingerprinted());

  const row = page.locator("#mig-view-actions");
  const items = [
    "#mig-out-of-order",
    "#mig-check-first",
    "#btn-mig-view-apply",
    "#btn-mig-view-repair",
    "#btn-mig-view-refresh",
  ];
  for (const sel of items) await expect(row.locator(sel)).toBeVisible();

  // The narrowest the seam allows.
  await page.evaluate(() => document.documentElement.style.setProperty("--mig-w-set", "300px"));
  for (const sel of items) await expect(row.locator(sel)).toBeVisible();
  const clipped = await row.evaluate((el) => el.scrollWidth > el.clientWidth + 1);
  expect(clipped, "the actions row is overflowing, so something is cut off").toBe(false);
});

/**
 * Reported in use: *"When we apply and it finish, after loading Apply button
 * still shows as if there's a pending migration (Until I manually refresh)"*.
 *
 * The list was right and only the button was wrong, which is the clue:
 * `buttonBusy` snapshots the button's children and `disabled` when the wait
 * starts and puts them back when it ends. Stage 24 moved the refresh *into*
 * the wait — the batched `info` arrives with the apply — so the undo in the
 * `finally` restored the pre-apply label over a button that had already been
 * re-synced correctly.
 */
test("after an apply the Apply button stops offering what it just applied", async ({ page }) => {
  await withProject(
    page,
    fingerprinted({
      flyway_info: () => PENDING_ONLY,
      flyway_migrate: () => ({
        executed: 1,
        target: "5",
        // Nothing pending any more: it ran.
        info: {
          migrations: [MIGRATIONS[0]],
          folders: ["/srv/app/migrations"],
          fingerprint: "after-the-apply",
        },
      }),
    }),
  );

  const apply = page.locator("#btn-mig-view-apply");
  await expect(apply).toHaveText("Apply 1 pending");

  await page.click("#btn-mig-view-apply");
  await page.locator('dialog.ask button:has-text("Apply")').last().click();
  await expect(page.locator("#flyway-body")).toContainText("applied 1 migration");

  // The list came back with the run and says nothing is pending, so the button
  // has to say so too — without a manual Refresh.
  await expect(apply).toHaveText("Apply");
  await expect(apply).toBeDisabled();
  await expect(apply).toHaveAttribute("title", "Nothing is pending.");
  await expect(apply).not.toHaveAttribute("aria-busy", "true");
});

/**
 * The other half of the same fix: when nothing *has* repainted the button, the
 * old label must still come back.
 *
 * Reusing a reading is the case with no repaint at all — the check is a
 * fingerprint, the list does not change, and nothing calls `syncApplyButton`.
 * So the spinner and "Checking…" are this function's to take away, and a
 * cancelled dialog must not leave the button spinning.
 */
test("a reused reading leaves the button as it found it", async ({ page }) => {
  await withProject(page, fingerprinted());
  const repair = page.locator("#btn-mig-view-repair");
  const before = await repair.textContent();

  await page.click("#btn-mig-view-repair");
  await expect(page.locator("dialog.ask")).toBeVisible();
  await page.locator('dialog.ask button:has-text("Cancel")').click();

  await expect(repair).toHaveText(before ?? "");
  await expect(repair).toBeEnabled();
  await expect(repair).not.toHaveAttribute("aria-busy", "true");
  await expect(repair.locator(".spinner")).toHaveCount(0);
});
