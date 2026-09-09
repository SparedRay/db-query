import { expect, test, type Page } from "@playwright/test";
import {
  type Backend,
  calls,
  commandNames,
  editorText,
  fireEvent,
  installBackend,
  schemaBackend,
} from "./harness";

/**
 * Queries arriving from an MCP client.
 *
 * **The assertion that matters is a negative one**: `run_script` is never
 * called. Everything else here — the tab, the mark, the message — exists to
 * make it obvious *where* a query came from, and none of it would matter if
 * something outside the app could run one.
 *
 * The event is fired directly rather than through a real server, because what
 * is under test is the frontend's half. The server, its refusals and its tool
 * list are proved over a real socket in `src-tauri/tests/mcp_http.rs`.
 */

const EVENT = "mcp://put-query";
const SQL = "SELECT id, email FROM users WHERE id = 1;";

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

const SAVED = {
  id: "c1",
  name: "local",
  colour: "#3b82f6",
  host: "127.0.0.1",
  port: 3306,
  user: "root",
  database: null,
  allowInvalidCerts: false,
  rememberPassword: true,
};

const CONN = {
  id: SAVED.id,
  serverVersion: "8.4.0",
  databases: ["poc"],
  currentDatabase: "poc",
};

function storedTab(over: Record<string, unknown> = {}) {
  return {
    title: "Untitled-1",
    filePath: null,
    dialect: "mysql",
    encoding: "utf-8",
    lineEnding: "lf",
    mtimeMs: null,
    text: "SELECT 'remembered';",
    cursor: 0,
    activeDb: null,
    untitledNumber: 1,
    ...over,
  };
}

function session(tabs: unknown[], activeIndex = 0) {
  return {
    session: {
      version: 1,
      connections: [{ connectionId: SAVED.id, tabs, activeIndex }],
    },
    warning: null,
  };
}

/** Boot with a saved connection and click it — a workspace, in two lines. */
async function connected(page: Page, extra: Backend = {}) {
  await installBackend(page, {
    ...schemaBackend,
    list_profiles: () => ({ profiles: [SAVED], warning: null }),
    connect_saved: () => CONN,
    ...extra,
  });
  await page.goto("/");
  await expect(page.locator(".rail-item")).toHaveCount(1);
  await page.locator(".rail-item").click();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
}

async function saved(page: Page) {
  const all = (await calls(page)).filter((c) => c.cmd === "save_session");
  return all.length
    ? (all[all.length - 1].args.session as {
        connections: Array<{ tabs: Array<Record<string, unknown>> }>;
      })
    : null;
}

// --------------------------------------------------------------------- M3

test("M3 — a query from a client lands in a new tab, focused, and is not run", async ({
  page,
}) => {
  await connected(page);
  const before = await page.locator("#script-tabs .stab").count();

  await fireEvent(page, EVENT, { sql: SQL, connectionId: SAVED.id });

  await expect(page.locator("#script-tabs .stab")).toHaveCount(before + 1);
  await expect(page.locator("#script-tabs .stab.active")).toHaveCount(1);
  expect(await editorText(page)).toContain("FROM users");

  // The whole point. Nothing ran, and nothing was even asked to.
  expect(await commandNames(page)).not.toContain("run_script");
  await expect(page.locator("#grid .empty")).not.toContainText("row");
});

test("the tab that arrives is marked as external, and says so on hover", async ({ page }) => {
  await connected(page);
  await fireEvent(page, EVENT, { sql: SQL, connectionId: SAVED.id });

  const mark = page.locator("#script-tabs .stab.active .stab-external");
  await expect(mark).toHaveCount(1);
  await expect(mark).toHaveAttribute("title", /MCP client/);
  await expect(mark).toHaveAttribute("title", /not been run/);
});

test("the tabs you opened yourself are not marked", async ({ page }) => {
  await connected(page);
  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
  await expect(page.locator("#script-tabs .stab-external")).toHaveCount(0);

  await fireEvent(page, EVENT, { sql: SQL, connectionId: SAVED.id });
  // Exactly one of the two, not both.
  await expect(page.locator("#script-tabs .stab")).toHaveCount(2);
  await expect(page.locator("#script-tabs .stab-external")).toHaveCount(1);
});

test("the message names the tab and says nothing has run", async ({ page }) => {
  await connected(page);
  await fireEvent(page, EVENT, { sql: SQL, connectionId: SAVED.id });

  await expect(page.locator("#grid .empty")).toContainText("MCP client");
  await expect(page.locator("#grid .empty")).toContainText("Nothing has been run");
});

// ------------------------------------------------------------- provenance lasts

test("the mark is written to the session file", async ({ page }) => {
  await connected(page);
  await fireEvent(page, EVENT, { sql: SQL, connectionId: SAVED.id });

  await expect
    .poll(async () => {
      const s = await saved(page);
      return s?.connections[0]?.tabs.some((t) => t.external === true) ?? false;
    }, { timeout: 5000 })
    .toBe(true);

  // And the tab the user opened is written down as theirs.
  const s = await saved(page);
  expect(s!.connections[0].tabs.filter((t) => t.external === true)).toHaveLength(1);
});

test("the mark comes back after a restart", async ({ page }) => {
  await connected(page, {
    load_session: () =>
      session([
        storedTab({ title: "mine" }),
        storedTab({ title: "from-a-client", untitledNumber: 2, external: true }),
      ]),
  });

  await expect(page.locator("#script-tabs .stab")).toHaveCount(2);
  await expect(page.locator("#script-tabs .stab-external")).toHaveCount(1);
  await expect(
    page.locator("#script-tabs .stab:has(.stab-external) .stab-label"),
  ).toHaveText("from-a-client");
});

/**
 * A session file written before Stage 13 has no `external` key. Every tab in it
 * was opened by the user, and marking them all would be a lie told to everyone
 * at once on the first launch of a new build.
 */
test("tabs stored before the mark existed are not marked", async ({ page }) => {
  await connected(page, {
    load_session: () => session([storedTab({ title: "old" })]),
  });

  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
  await expect(page.locator("#script-tabs .stab-external")).toHaveCount(0);
});

// ------------------------------------------------------------- the awkward cases

/**
 * The client was told the query was placed, so dropping it silently would be
 * the one outcome nobody can see. The connection id travels with the payload
 * precisely so this case can be told apart from the ordinary one.
 */
test("a query for a connection that is gone is reported, not swallowed", async ({ page }) => {
  await connected(page);
  await fireEvent(page, EVENT, { sql: SQL, connectionId: "a-connection-that-closed" });

  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
  await expect(page.locator("#grid .empty")).toContainText("no longer open");
});

test("saving the tab clears the mark — it is your file now", async ({ page }) => {
  await connected(page, {
    save_file_dialog: () => ({
      path: "/home/u/queries/from-a-client.sql",
      name: "from-a-client.sql",
      mtimeMs: 1_700_000_000_000,
    }),
  });
  await fireEvent(page, EVENT, { sql: SQL, connectionId: SAVED.id });
  await expect(page.locator("#script-tabs .stab-external")).toHaveCount(1);

  await page.keyboard.press("Control+Shift+S");

  await expect(page.locator("#script-tabs .stab.active .stab-label")).toHaveText(
    "from-a-client.sql",
  );
  await expect(page.locator("#script-tabs .stab-external")).toHaveCount(0);
});

// ------------------------------------------------------- Settings > Integrations

const TOKEN = "a-token-from-the-keychain";
const URL_49731 = "http://127.0.0.1:49731/mcp";

/** Boot connected, with the MCP commands answering as a working server would. */
async function withServer(page: Page, extra: Backend = {}) {
  let running = false;
  let port = 0;
  await connected(page, {
    mcp_status: () => ({
      running,
      port: running ? port : null,
      url: running ? `http://127.0.0.1:${port}/mcp` : null,
    }),
    mcp_start: (a) => {
      running = true;
      port = Number(a.port);
      return { running: true, port, url: `http://127.0.0.1:${port}/mcp` };
    },
    mcp_stop: () => {
      running = false;
      return { running: false, port: null, url: null };
    },
    mcp_token: () => TOKEN,
    mcp_regenerate_token: () => TOKEN + "-2",
    ...extra,
  });
}

async function openIntegrations(page: Page) {
  await page.click("#btn-settings");
  await page.locator("#settings-dialog").waitFor({ state: "visible" });
  await page.click("#set-tab-integrations");
  await expect(page.locator("#set-pane-integrations")).toBeVisible();
}

test("the server is off until it is turned on", async ({ page }) => {
  await withServer(page);
  await openIntegrations(page);

  await expect(page.locator("#set-mcp-on")).not.toBeChecked();
  await expect(page.locator("#set-mcp-status")).toContainText("Nothing is listening");
  // No token, no snippets, nothing to copy — there is nothing to connect to.
  await expect(page.locator("#set-mcp-live")).toBeHidden();
  expect(await commandNames(page)).not.toContain("mcp_start");
  // And no credential was minted just because someone looked at the pane.
  expect(await commandNames(page)).not.toContain("mcp_token");
});

test("turning it on binds the default port and shows how to connect", async ({ page }) => {
  await withServer(page);
  await openIntegrations(page);
  await page.locator("#set-mcp-on").check();

  await expect(page.locator("#set-mcp-status")).toContainText(URL_49731);
  await expect(page.locator("#set-mcp-live")).toBeVisible();

  // The port came from the backend rather than being written down twice.
  const start = (await calls(page)).find((c) => c.cmd === "mcp_start");
  expect(start!.args.port).toBe(49731);
});

test("the snippets are the ones a client actually takes", async ({ page }) => {
  await withServer(page);
  await openIntegrations(page);
  await page.locator("#set-mcp-on").check();

  const cli = await page.locator("#set-mcp-cli").textContent();
  expect(cli).toContain("claude mcp add --transport http");
  expect(cli).toContain(URL_49731);
  expect(cli).toContain(`Authorization: Bearer ${TOKEN}`);

  const json = JSON.parse((await page.locator("#set-mcp-json").textContent())!);
  expect(json.mcpServers["db-query"]).toEqual({
    type: "http",
    url: URL_49731,
    headers: { Authorization: `Bearer ${TOKEN}` },
  });
});

test("the token is masked until you ask for it", async ({ page }) => {
  await withServer(page);
  await openIntegrations(page);
  await page.locator("#set-mcp-on").check();

  const field = page.locator("#set-mcp-token");
  await expect(field).toHaveAttribute("type", "password");
  await page.click("#set-mcp-reveal");
  await expect(field).toHaveAttribute("type", "text");
  await expect(field).toHaveValue(TOKEN);
});

/**
 * The failure that matters. A switch reading "on" while nothing is listening is
 * worse than no switch at all, so the checkbox is corrected from what the
 * backend reports rather than from what was clicked.
 */
test("a port that will not bind leaves the switch off and says why", async ({ page }) => {
  await withServer(page, {
    mcp_start: () => {
      throw new Error("Could not listen on 127.0.0.1:49731 (address in use). Choose another port.");
    },
  });
  await openIntegrations(page);
  // `click`, not `check`: Playwright's `check` asserts the box ends up ticked,
  // and the behaviour under test is precisely that it does not.
  await page.locator("#set-mcp-on").click();

  await expect(page.locator("#set-mcp-on")).not.toBeChecked();
  await expect(page.locator("#set-mcp-status")).toContainText("address in use");
  await expect(page.locator("#set-mcp-live")).toBeHidden();
});

test("turning it off stops the server and takes the token off screen", async ({ page }) => {
  await withServer(page);
  await openIntegrations(page);
  await page.locator("#set-mcp-on").check();
  await expect(page.locator("#set-mcp-live")).toBeVisible();

  await page.locator("#set-mcp-on").uncheck();
  await expect(page.locator("#set-mcp-live")).toBeHidden();
  await expect(page.locator("#set-mcp-status")).toContainText("Nothing is listening");
  expect(await commandNames(page)).toContain("mcp_stop");
});

test("regenerating asks first, and cancelling changes nothing", async ({ page }) => {
  await withServer(page);
  await openIntegrations(page);
  await page.locator("#set-mcp-on").check();

  await page.click("#set-mcp-regen");
  await expect(page.locator("dialog.ask")).toBeVisible();
  await expect(page.locator("dialog.ask p")).toContainText(/stops working/i);
  await page.locator('dialog.ask button:has-text("Cancel")').click();

  expect(await commandNames(page)).not.toContain("mcp_regenerate_token");
});

test("regenerating, when confirmed, issues a new token", async ({ page }) => {
  await withServer(page);
  await openIntegrations(page);
  await page.locator("#set-mcp-on").check();

  await page.click("#set-mcp-regen");
  await page.locator('dialog.ask button:has-text("Regenerate")').click();

  expect(await commandNames(page)).toContain("mcp_regenerate_token");
  await expect(page.locator("#set-mcp-status")).toContainText("new token issued");
});

test("the server comes back on at boot when it was left on", async ({ page }) => {
  await withServer(page);
  await openIntegrations(page);
  await page.locator("#set-mcp-on").check();
  await expect(page.locator("#set-mcp-live")).toBeVisible();

  await page.reload();
  await expect
    .poll(async () => (await commandNames(page)).filter((c) => c === "mcp_start").length, {
      timeout: 5000,
    })
    .toBeGreaterThan(0);
});

/**
 * A busy port this morning must not turn the feature off forever. The
 * preference is the user's decision, made earlier; a boot that could not act on
 * it says so and tries again next launch.
 */
test("a boot that cannot bind reports it and keeps the preference", async ({ page }) => {
  await withServer(page);
  await openIntegrations(page);
  await page.locator("#set-mcp-on").check();
  await page.click("#set-close");

  // Reload with the port taken by something else.
  await installBackend(page, {
    ...schemaBackend,
    list_profiles: () => ({ profiles: [SAVED], warning: null }),
    connect_saved: () => CONN,
    mcp_status: () => ({ running: false, port: null, url: null }),
    mcp_start: () => {
      throw new Error("Could not listen on 127.0.0.1:49731 (address in use).");
    },
    mcp_stop: () => ({ running: false, port: null, url: null }),
  });
  await page.reload();

  await expect(page.locator("#grid .empty")).toContainText("did not start");
  await expect(page.locator("#grid .empty")).toContainText("address in use");

  // And it still tried, which is the half that would be lost by writing the
  // preference off on a failure nobody was watching.
  expect(await commandNames(page)).toContain("mcp_start");
});

/** A fresh install listens on nothing, which is the only acceptable default. */
test("a fresh install starts nothing", async ({ page }) => {
  await withServer(page);
  await expect
    .poll(async () => (await commandNames(page)).includes("app_defaults"), { timeout: 5000 })
    .toBe(true);
  expect(await commandNames(page)).not.toContain("mcp_start");
});
