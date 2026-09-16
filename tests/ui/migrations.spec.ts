import { expect, test, type Page } from "@playwright/test";
import {
  CONN_INFO,
  MYSQL_CAPS,
  calls,
  commandNames,
  connect,
  installBackend,
  schemaBackend,
} from "./harness";

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
    flyway_migrate: () => ({ executed: 1, target: "2" }),
    flyway_repair: () => ({
      actions: ["Removed failed migrations"],
      removed: [{ version: "3", description: "deliberately broken" }],
      deleted: [],
      aligned: [],
    }),
    ...extra,
  };
}

/** Attach the project through the real dialogs, and land on the list. */
async function attach(page: Page, extra: Record<string, unknown> = {}) {
  await connect(page, attached(extra));
  await openPane(page);
  await page.click(".mig-empty button");
  await page.locator('dialog.ask button:has-text("UAT database")').click();
  await page.locator(".mig").first().waitFor();
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
  // Grouped by what an apply would do, not by what has happened.
  await expect(page.locator('.mig-group[data-group="pending"] .mig-group-title')).toHaveText(
    "Will run",
  );
  await expect(page.locator('.mig-group[data-group="done"] .mig-group-title')).toHaveText(
    "Will not run",
  );
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

// ------------------------------------------------- what a tab says it is

/**
 * A migration tab claimed to have been "Added by an MCP client", because the
 * provenance mark was a boolean called `external` and MCP was the only thing
 * that could set it. A fact with two possible answers does not fit in a flag.
 */
test("a migration opens as a migration, not as something an MCP client sent", async ({
  page,
}) => {
  await attach(page, {
    read_file: () => ({
      name: "V2__add_colour.sql", path: "/p/V2__add_colour.sql",
      contents: "ALTER TABLE widgets ADD colour VARCHAR(16);",
      sizeBytes: 41, encoding: "utf-8", lineEnding: "lf", mtimeMs: 1, readOnly: false,
    }),
  });
  await page.locator('.mig[data-state="pending"]').click();

  const mark = page.locator("#script-tabs .stab.active .stab-origin");
  await expect(mark).toHaveAttribute("data-origin", "migration");
  await expect(mark).toHaveAttribute("title", /Flyway migration/);
  await expect(mark).not.toHaveAttribute("title", /MCP/);
  // It is an icon from the app's own set, not a stray arrow character.
  await expect(mark.locator("svg")).toHaveCount(1);
});

// ------------------------------------------------------------- grouping

test("groups collapse and expand without asking Flyway again", async ({ page }) => {
  await attach(page);
  const done = page.locator('.mig-group[data-group="done"]');
  const runs = async () => (await calls(page)).filter((c) => c.cmd === "flyway_info").length;

  // The long pile starts shut: a project with two hundred applied migrations
  // should not need scrolling to find the one that is pending.
  await expect(done).toHaveClass(/collapsed/);
  await expect(done.locator(".mig-group-count")).toHaveText("1");

  const before = await runs();
  await done.locator(".mig-group-head").click();
  await expect(done).not.toHaveClass(/collapsed/);
  await expect(done.locator(".mig-group-head")).toHaveAttribute("aria-expanded", "true");
  // Redrawn from what is already on screen: collapsing must not start a JVM.
  expect(await runs()).toBe(before);

  await done.locator(".mig-group-head").click();
  await expect(done).toHaveClass(/collapsed/);
});

/**
 * Out-of-order changes *which* migrations Flyway will run, so the question is
 * asked again with the flag rather than reasoned about here. `Ignored` becomes
 * `Pending` under it, and a confirmation built on a guess would name the wrong
 * versions.
 */
test("out of order re-asks Flyway with the flag", async ({ page }) => {
  await attach(page);
  const flags = async () =>
    (await calls(page)).filter((c) => c.cmd === "flyway_info").map((c) => c.args.outOfOrder);

  expect(await flags()).toEqual([false]);

  await page.locator("#mig-out-of-order").check();
  await expect.poll(async () => (await flags()).length).toBe(2);
  expect((await flags())[1]).toBe(true);
});

// -------------------------------------------- seeing and changing the project

/** There was no way to see which file a connection was pointed at. */
test("the pane says which project file and folder it is using", async ({ page }) => {
  await attach(page);
  await expect(page.locator(".mig-proj-name")).toHaveText("Flyway Connections");
  await expect(page.locator(".mig-proj-where")).toContainText("flyway.toml");
  await expect(page.locator(".mig-proj-where")).toContainText("/p");
  await expect(page.locator(".mig-proj-name")).toHaveAttribute("title", "/p/flyway.toml");
});

test("the environment can be changed, and goes through the same guard", async ({ page }) => {
  // Agrees at import, disagrees afterwards: the file moved on, which is what
  // a branch switch does.
  let asked = 0;
  await attach(page, { flyway_check: () => (asked++ === 0 ? [] : ["host"]) });

  await page.click(".mig-proj-top button");
  await page.locator('dialog.ask button:has-text("Change environment")').click();
  await page.locator('dialog.ask button:has-text("Development database")').click();

  // The guard fires for a change exactly as it does for an import.
  await expect(page.locator("dialog.ask")).toContainText("points somewhere else");
  await page.locator('dialog.ask button:has-text("Cancel")').click();
  expect(await attachments(page)).toHaveLength(1);
  expect((await attachments(page))[0].env).toBe("uat");
});

test("detaching clears the project and offers the import again", async ({ page }) => {
  await attach(page);
  await page.click(".mig-proj-top button");
  await page.locator('dialog.ask button:has-text("Detach")').click();

  await expect(page.locator(".mig-empty")).toContainText("No Flyway project is attached");
  const saved = (await calls(page))
    .filter((c) => c.cmd === "save_profile")
    .map((c) => c.args.profile as Record<string, unknown>);
  const last = saved[saved.length - 1];
  expect(last.flywayProject).toBeNull();
  expect(last.flywayEnvironment).toBeNull();
});

/** A branch switch can take the file away while the connection still names it. */
test("a project file that has gone says so and offers a way out", async ({ page }) => {
  // Readable when it is attached, gone by the time the pane re-reads it —
  // which is exactly what switching branch does to it.
  let reads = 0;
  await attach(page, {
    flyway_read_project: () => {
      if (reads++ < 2) return PROJECT;
      throw new Error("Cannot read /p/flyway.toml: No such file or directory");
    },
  });
  await page.click("#btn-mig-refresh");

  await expect(page.locator(".mig-empty")).toContainText("could not be read");
  await expect(page.locator(".mig-empty button")).toHaveText(/Change project/);
});

// ------------------------------------------------------------- applying

test("Apply names the versions and runs nothing until it is confirmed", async ({ page }) => {
  await attach(page, {
    flyway_info: () => [
      { version: "1", description: "create widgets", state: "Success", category: "Versioned",
        kind: "SQL", filepath: "/p/V1.sql", installedOnUtc: "2026-09-10T23:39:43Z",
        installedBy: "root", executionTimeMs: 12, group: "done" },
      { version: "5", description: "add index", state: "Pending", category: "Versioned",
        kind: "SQL", filepath: "/p/V5.sql", installedOnUtc: null, installedBy: null,
        executionTimeMs: null, group: "pending" },
    ],
  });

  await expect(page.locator("#btn-mig-apply")).toBeEnabled();
  await expect(page.locator("#btn-mig-apply")).toHaveText("Apply 1…");
  await page.click("#btn-mig-apply");

  // Versions, not a count: "Apply 3 migrations?" is a question about
  // arithmetic; naming them is a question about which changes.
  const ask = page.locator("dialog.ask");
  await expect(ask).toContainText("V5");
  await expect(ask).toContainText("add index");
  await expect(ask).toContainText("uat");

  await ask.locator('button:has-text("Cancel")').click();
  expect(await commandNames(page)).not.toContain("flyway_migrate");
});

test("confirming applies, and says what Flyway did", async ({ page }) => {
  await attach(page, {
    flyway_info: () => [
      { version: "5", description: "add index", state: "Pending", category: "Versioned",
        kind: "SQL", filepath: "/p/V5.sql", installedOnUtc: null, installedBy: null,
        executionTimeMs: null, group: "pending" },
    ],
    flyway_migrate: () => ({ executed: 1, target: "5" }),
  });

  await page.click("#btn-mig-apply");
  await page.locator('dialog.ask button:has-text("Apply to uat")').click();

  await expect.poll(async () => commandNames(page)).toContain("flyway_migrate");
  const call = (await calls(page)).find((c) => c.cmd === "flyway_migrate")!;
  expect(call.args.outOfOrder).toBe(false);
  await expect(page.locator("#grid .empty")).toContainText("applied 1 migration");
  await expect(page.locator("#grid .empty")).toContainText("now at 5");
});

/** F7. Refused in Rust too; this is the courtesy, not the guard. */
test("a read-only connection is not offered an apply", async ({ page }) => {
  await attach(page, {
    save_profile: (a: Record<string, unknown>) => ({
      profile: { ...(a.profile as object), readOnly: true, rememberPassword: false, needsSecret: false },
      passwordWarning: null, passwordStored: false,
    }),
  });

  await expect(page.locator("#btn-mig-apply")).toBeDisabled();
  await expect(page.locator("#btn-mig-apply")).toHaveAttribute("title", /read-only/);
});

/** Flyway refuses while a failure sits in the history, so the button says so
 *  rather than spending a confirmation to find out. */
test("a failed migration blocks apply, and the button explains why", async ({ page }) => {
  await attach(page);
  await expect(page.locator("#btn-mig-apply")).toBeDisabled();
  await expect(page.locator("#btn-mig-apply")).toHaveAttribute("title", /failed/);
  await expect(page.locator("#btn-mig-apply")).toHaveAttribute("title", /repaired/);
});

/** Flyway's own words: it names the file, the line and the SQL error. */
test("a failed apply shows Flyway's message verbatim", async ({ page }) => {
  const FLYWAY_SAID =
    "Failed to execute script V4__deliberately_broken.sql against development environment\n" +
    "Message    : (conn=13) Can't DROP 'weight'; check that column/key exists";
  await attach(page, {
    flyway_info: () => [
      { version: "5", description: "add index", state: "Pending", category: "Versioned",
        kind: "SQL", filepath: "/p/V5.sql", installedOnUtc: null, installedBy: null,
        executionTimeMs: null, group: "pending" },
    ],
    flyway_migrate: () => {
      throw new Error(FLYWAY_SAID);
    },
  });

  await page.click("#btn-mig-apply");
  await page.locator('dialog.ask button:has-text("Apply to uat")').click();

  await expect(page.locator("#grid .empty")).toContainText("Can't DROP 'weight'");
  await expect(page.locator("#grid .empty")).toContainText("V4__deliberately_broken.sql");
});

/**
 * Out-of-order decides what an apply will run, so it belongs to a connection
 * rather than to the app. One shared flag would carry the choice made on a dev
 * connection onto a UAT one the moment you clicked across — silently changing
 * which migrations the next confirmation would name.
 */
test("out of order belongs to the connection, not to the window", async ({ page }) => {
  await attach(page);
  await page.locator("#mig-out-of-order").check();
  await expect(page.locator("#mig-out-of-order")).toBeChecked();

  // A second connection, its own project, its own default.
  await page.click(".rail-add");
  await page.locator("#conn-dialog").waitFor({ state: "visible" });
  await page.fill("#conn-dialog input[name=name]", "second");
  await page.click("#conn-ok");
  await page.locator("#conn-dialog").waitFor({ state: "hidden" });
  await page.click(".mig-empty button");
  await page.locator('dialog.ask button:has-text("UAT database")').click();
  await page.locator(".mig").first().waitFor();

  await expect(page.locator("#mig-out-of-order")).not.toBeChecked();
  const flags = (await calls(page))
    .filter((c) => c.cmd === "flyway_info")
    .map((c) => c.args.outOfOrder);
  expect(flags[flags.length - 1]).toBe(false);
});

// ------------------------------------------------------------- repairing

/**
 * Repair rewrites the schema history. It is not a maintenance button somebody
 * might press to see what it does, so it is not there unless there is
 * something to repair.
 */
test("Repair is offered only while something has failed", async ({ page }) => {
  // Failed to begin with, healthy once the list is asked again — which is what
  // a successful repair looks like from here, and proves the button tracks the
  // list rather than being decided once.
  let asked = 0;
  await attach(page, {
    flyway_info: () => (asked++ === 0 ? MIGRATIONS : [MIGRATIONS[0], MIGRATIONS[1]]),
  });
  await expect(page.locator("#btn-mig-repair")).toBeVisible();

  await page.click("#btn-mig-refresh");
  await expect(page.locator('.mig[data-state="failed"]')).toHaveCount(0);
  await expect(page.locator("#btn-mig-repair")).toBeHidden();
  // And Apply becomes possible again, which is the point of having repaired.
  await expect(page.locator("#btn-mig-apply")).toBeEnabled();
});

/**
 * **The confirmation is the feature.** "Repair" sounds like it fixes the
 * database, and it does not — it removes a row from the schema history and
 * leaves every change that migration made in place. Somebody who misreads that
 * will apply again on top of a half-applied migration.
 */
test("the repair confirmation says what it does not undo", async ({ page }) => {
  await attach(page);
  await page.click("#btn-mig-repair");

  const ask = page.locator("dialog.ask");
  await expect(ask).toContainText("uat");
  // Which entry, by version and description.
  await expect(ask).toContainText("V3");
  await expect(ask).toContainText("deliberately broken");
  // What it does not do.
  await expect(ask).toContainText("does not undo");
  await expect(ask).toContainText("still in the database");
  // And what happens next, so "pending again" is not a surprise.
  await expect(ask).toContainText("pending again");

  await ask.locator('button:has-text("Cancel")').click();
  expect(await commandNames(page)).not.toContain("flyway_repair");
});

test("confirming repairs, says what Flyway did, and re-reads the list", async ({ page }) => {
  await attach(page);
  const before = (await calls(page)).filter((c) => c.cmd === "flyway_info").length;

  await page.click("#btn-mig-repair");
  await page.locator('dialog.ask button:has-text("Repair uat")').click();

  await expect.poll(async () => commandNames(page)).toContain("flyway_repair");
  await expect(page.locator("#grid .empty")).toContainText("Removed failed migrations");
  await expect(page.locator("#grid .empty")).toContainText("V3");
  // The claim the confirmation made, repeated where it is acted on.
  await expect(page.locator("#grid .empty")).toContainText("database itself is unchanged");

  // The list is asked again: after a repair the failed row is gone and the
  // migration is pending, and a stale pane would still show it as blocking.
  await expect
    .poll(async () => (await calls(page)).filter((c) => c.cmd === "flyway_info").length)
    .toBeGreaterThan(before);
});

/**
 * Success and "nothing needed doing" are different answers. Flyway reports the
 * second as a success with every list empty, and calling that "repaired" tells
 * somebody their problem is fixed when nothing was touched.
 */
test("a repair that found nothing to do says so, rather than claiming success", async ({
  page,
}) => {
  await attach(page, {
    flyway_repair: () => ({ actions: [], removed: [], deleted: [], aligned: [] }),
  });

  await page.click("#btn-mig-repair");
  await page.locator('dialog.ask button:has-text("Repair uat")').click();

  await expect(page.locator("#grid .empty")).toContainText("found nothing to repair");
  await expect(page.locator("#grid .empty")).not.toContainText("Removed");
});

/** F7's other half. Refused in Rust too; this is the courtesy. */
test("a read-only connection cannot repair either", async ({ page }) => {
  await attach(page, {
    save_profile: (a: Record<string, unknown>) => ({
      profile: { ...(a.profile as object), readOnly: true, rememberPassword: false, needsSecret: false },
      passwordWarning: null, passwordStored: false,
    }),
  });

  await expect(page.locator("#btn-mig-repair")).toBeVisible();
  await expect(page.locator("#btn-mig-repair")).toBeDisabled();
  await expect(page.locator("#btn-mig-repair")).toHaveAttribute("title", /read-only/);
});

test("a refused repair shows Flyway's own message", async ({ page }) => {
  await attach(page, {
    flyway_repair: () => {
      throw new Error("Unable to connect to the database. Check the connection details.");
    },
  });

  await page.click("#btn-mig-repair");
  await page.locator('dialog.ask button:has-text("Repair uat")').click();

  await expect(page.locator("#grid .empty")).toContainText("Unable to connect to the database");
});

/**
 * **The dead end this closes.** A migration edited after it ran is still
 * reported by `info` as `Success`, so the list looks entirely healthy and
 * nothing is failed. Apply is offered, Flyway refuses it with a checksum
 * mismatch and says to run repair — and until this, there was no Repair button
 * anywhere to follow that instruction with.
 *
 * Measured against Flyway 13.5.0 on 2026-09-16; the message below is its own.
 */
test("Repair appears when Flyway asks for one, even with nothing failed", async ({ page }) => {
  const HEALTHY = [MIGRATIONS[0], MIGRATIONS[1]]; // done + pending, nothing failed
  await attach(page, {
    flyway_info: () => HEALTHY,
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

  await expect(page.locator("#btn-mig-repair")).toBeHidden();
  await page.click("#btn-mig-apply");
  await page.locator('dialog.ask button:has-text("Apply to uat")').click();

  // Flyway's own words, and then a way to act on them.
  await expect(page.locator("#grid .empty")).toContainText("checksum mismatch");
  await expect(page.locator("#btn-mig-repair")).toBeVisible();
  await expect(page.locator("#btn-mig-repair")).toBeEnabled();
});

/**
 * A refusal that has nothing to do with repair must not offer one. Rewriting a
 * schema history because the database was unreachable would be a real change
 * made in answer to an imaginary problem.
 */
test("an unrelated refusal leaves Repair where it was", async ({ page }) => {
  await attach(page, {
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
  await expect(page.locator("#btn-mig-repair")).toBeHidden();
});
