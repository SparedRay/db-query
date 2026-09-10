import { expect, test, type Page } from "@playwright/test";
import { CONN_INFO, MYSQL_CAPS, calls, connect, installBackend, schemaBackend } from "./harness";

/**
 * The migrations pane.
 *
 * A pane on the right rather than a section of the schema tree, so the database
 * structure stays visible while a migration is being read — which is the only
 * reason to want both at once.
 *
 * Everything here **reads**. Applying and repairing are later phases, and the
 * pane offers neither rather than offering a button that explains itself.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

/** MySQL can host a Flyway project; Elasticsearch cannot, and says so. */
const NO_MIGRATIONS = { ...MYSQL_CAPS, engine: "elasticsearch", migrations: false };

const PROJECT = {
  name: "Flyway Connections",
  databaseType: "MySql",
  defaultEnvironment: "uat",
  outOfOrder: false,
  environments: [
    { id: "development", displayName: "Development database", url: "jdbc:mysql://dev:3306/f", user: "dev_app" },
    { id: "uat", displayName: "UAT database", url: "jdbc:mysql://uat:3306/f", user: "uat_app" },
  ],
};

const MIGRATIONS = [
  {
    version: "1", description: "create widgets", state: "Success", category: "Versioned",
    kind: "SQL", filepath: "/p/V1__create_widgets.sql",
    installedOnUtc: "2026-09-10T23:39:43Z", installedBy: "root", executionTimeMs: 12,
  },
  {
    version: "2", description: "add colour", state: "Pending", category: "Versioned",
    kind: "SQL", filepath: "/p/V2__add_colour.sql",
    installedOnUtc: null, installedBy: null, executionTimeMs: null,
  },
  {
    version: "3", description: "deliberately broken", state: "Failed", category: "Versioned",
    kind: "SQL", filepath: "/p/V3__broken.sql",
    installedOnUtc: "2026-09-10T23:39:43Z", installedBy: "root", executionTimeMs: 7,
  },
];

/** A connection with a project already attached. */
function attached(extra: Record<string, unknown> = {}) {
  return {
    ...schemaBackend,
    save_profile: (a: Record<string, unknown>) => ({
      profile: { ...(a.profile as object), rememberPassword: false, needsSecret: false },
      passwordWarning: null,
      passwordStored: false,
    }),
    flyway_info: () => MIGRATIONS,
    flyway_read_project: () => PROJECT,
    flyway_pick_project: () => "/p/flyway.toml",
    flyway_check: () => [],
    ...extra,
  };
}

/** Every `save_profile` that actually attached a project. */
async function attachments(page: Page) {
  return (await calls(page))
    .filter((c) => c.cmd === "save_profile")
    .map((c) => c.args.profile as Record<string, unknown>)
    .filter((p) => p.flywayProject)
    .map((p) => ({ project: p.flywayProject, env: p.flywayEnvironment }));
}

async function openPane(page: Page) {
  await page.click("#btn-migrations");
  await expect(page.locator("#migrations-pane")).toBeVisible();
}

test("the toggle is offered on an engine Flyway can drive, and not otherwise", async ({ page }) => {
  await connect(page, attached());
  await expect(page.locator("#btn-migrations")).toBeVisible();

  // A capability, never the engine's name — an engine Flyway cannot drive has
  // nothing to attach, so the button is not there to press.
  await page.reload();
  await installBackend(page, {
    ...attached(),
    connect: () => ({ ...CONN_INFO, capabilities: NO_MIGRATIONS }),
  });
  await page.goto("/");
  await page.click("#btn-connect");
  await page.click("#conn-ok");
  await expect(page.locator("#btn-migrations")).toBeHidden();
});

test("the pane opens beside the schema tree, not instead of it", async ({ page }) => {
  await connect(page, attached());
  await openPane(page);
  // The whole reason it is a pane: both are on screen at once.
  await expect(page.locator("#sidebar")).toBeVisible();
  await expect(page.locator("#editor")).toBeVisible();

  await page.click("#btn-migrations");
  await expect(page.locator("#migrations-pane")).toBeHidden();
});

test("with no project attached the pane says what to do", async ({ page }) => {
  await connect(page, attached());
  await openPane(page);

  await expect(page.locator(".mig-empty")).toContainText(/No Flyway project/);
  await expect(page.locator(".mig-empty button")).toHaveText(/Import a project/);
  // And it has not gone asking Flyway about a project that is not there.
  expect(await calls(page)).not.toContainEqual(expect.objectContaining({ cmd: "flyway_info" }));
});

/** Importing: pick, choose an environment, and it is remembered on the profile. */
test("importing a project stores the path and the chosen environment", async ({ page }) => {
  await connect(page, attached());
  await openPane(page);
  await page.click(".mig-empty button");

  // The environment the file itself names is offered first.
  await expect(page.locator("dialog.ask")).toBeVisible();
  await page.locator('dialog.ask button:has-text("UAT database")').click();

  await expect.poll(() => attachments(page)).toEqual([{ project: "/p/flyway.toml", env: "uat" }]);
});

/**
 * **The guard.** `flyway migrate -environment=x` connects using the project's
 * own settings, so attaching the wrong environment means watching one database
 * while changing another.
 */
test("a disagreeing environment is refused until it is deliberately overridden", async ({ page }) => {
  await connect(page, attached({ flyway_check: () => ["host", "user"] }));
  await openPane(page);
  await page.click(".mig-empty button");
  await page.locator('dialog.ask button:has-text("UAT database")').click();

  // It names the fields, because "does not match" is not actionable.
  await expect(page.locator("dialog.ask h2")).toContainText(/points somewhere else/);
  await expect(page.locator("dialog.ask p")).toContainText(/host, user/);

  // Cancelling attaches nothing. Connecting saves the profile on its own, so
  // the question is whether a project was written to it — not whether the
  // profile was ever saved.
  await page.locator('dialog.ask button:has-text("Cancel")').click();
  expect(await attachments(page), "cancelling must attach nothing").toEqual([]);

  // Overriding is a second, deliberate act.
  await page.click(".mig-empty button");
  await page.locator('dialog.ask button:has-text("UAT database")').click();
  await page.locator('dialog.ask button:has-text("Attach anyway")').click();
  await expect.poll(() => attachments(page)).toEqual([{ project: "/p/flyway.toml", env: "uat" }]);
});

/** Once attached, the pane lists what Flyway reported. */
test("the list shows Flyway's own states, and which environment it is", async ({ page }) => {
  await connect(page, attached());
  await openPane(page);
  await page.click(".mig-empty button");
  await page.locator('dialog.ask button:has-text("UAT database")').click();

  await expect(page.locator(".mig")).toHaveCount(3);
  await expect(page.locator('.mig[data-state="failed"] .d')).toHaveText("deliberately broken");
  await expect(page.locator('.mig[data-state="pending"] .d')).toHaveText("add colour");
  // §3.5's second half: the environment is never out of sight, because a guard
  // can only compare what differs.
  await expect(page.locator("#mig-env")).toContainText("uat");
});

test("clicking a migration opens its SQL without binding the file", async ({ page }) => {
  await connect(
    page,
    attached({ read_file: () => ({
      name: "V2__add_colour.sql", path: "/p/V2__add_colour.sql", contents: "ALTER TABLE widgets ADD colour VARCHAR(16);",
      sizeBytes: 41, encoding: "utf-8", lineEnding: "lf", mtimeMs: 1, readOnly: false,
    }) }),
  );
  await openPane(page);
  await page.click(".mig-empty button");
  await page.locator('dialog.ask button:has-text("UAT database")').click();

  await page.locator('.mig[data-state="pending"]').click();

  await expect.poll(async () => (await calls(page)).some((c) => c.cmd === "read_file")).toBe(true);
  await expect(page.locator("#editor .cm-content")).toContainText("ADD colour");
  // Nothing was run, and nothing was saved over.
  const names = (await calls(page)).map((c) => c.cmd);
  expect(names).not.toContain("run_script");
  expect(names).not.toContain("save_file");
});

/** Flyway explains itself well; a paraphrase would replace an instruction. */
test("Flyway's own refusal is what the pane shows", async ({ page }) => {
  await connect(
    page,
    attached({
      flyway_info: () => {
        throw new Error(
          "Validate failed: Detected failed migration to version 4. Please remove any " +
            "half-completed changes then run repair to fix the schema history.",
        );
      },
    }),
  );
  await openPane(page);
  await page.click(".mig-empty button");
  await page.locator('dialog.ask button:has-text("UAT database")').click();

  await expect(page.locator(".mig-empty")).toContainText(/run repair to fix the schema history/);
});
