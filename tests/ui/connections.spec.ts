import { expect, test, type Page } from "@playwright/test";
import {
  CONN_INFO,
  MYSQL_CAPS,
  calls,
  connect,
  installBackend,
  openDatabase,
  schemaBackend,
} from "./harness";

/**
 * The rail of saved connections: what each button says it is, and what order
 * they sit in.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

const MYSQL = {
  id: "p-mysql",
  name: "prod-eu",
  colour: "#3b82f6",
  host: "db.example.com",
  port: 3306,
  user: "reporting",
  database: "analytics",
  allowInvalidCerts: false,
  kind: "mysql",
  url: "",
  auth: { type: "none" },
  noPassword: false,
  rememberPassword: true,
  needsSecret: false,
};

/**
 * The shape that produced the bug: the editor hides the MySQL fields for a
 * cluster but does not blank them, so a saved Elasticsearch profile really
 * does carry `localhost:3306` — values it has never used for anything.
 */
const ELASTIC = {
  ...MYSQL,
  id: "p-es",
  name: "Elastic",
  colour: "#ef4444",
  host: "localhost",
  user: "",
  database: null,
  kind: "elasticsearch",
  url: "https://cluster.example.com:9243",
  auth: { type: "basic", user: "elastic" },
};

async function railOf(page: Page, profiles: unknown[]) {
  await installBackend(page, { list_profiles: () => ({ profiles, warning: null }) });
  await page.goto("/");
  await expect(page.locator(".rail-item")).toHaveCount(profiles.length);
}

const titles = (page: Page) =>
  page.locator(".rail-item").evaluateAll((els) => els.map((e) => e.getAttribute("title") ?? ""));

test("an Elasticsearch connection's tooltip names its URL, not a host and port", async ({
  page,
}) => {
  await railOf(page, [ELASTIC]);

  const title = (await titles(page))[0];
  expect(title).toContain("https://cluster.example.com:9243");
  // The fields it does not use must not be reported as if it did. This said
  // `@localhost:3306` for a cluster that has never been at either.
  expect(title).not.toContain("localhost");
  expect(title).not.toContain("3306");
  // Basic auth has a user, and it belongs in front of the URL.
  expect(title).toContain("elastic@https://");
});

test("a MySQL connection still reads user@host:port", async ({ page }) => {
  await railOf(page, [MYSQL]);
  expect((await titles(page))[0]).toContain("reporting@db.example.com:3306");
});

test("Move down reorders the rail and writes the new order", async ({ page }) => {
  await railOf(page, [MYSQL, ELASTIC]);
  expect(await titles(page)).toEqual([
    expect.stringContaining("prod-eu"),
    expect.stringContaining("Elastic"),
  ]);

  await page.locator(".rail-item").first().click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Move down")').click();

  await expect
    .poll(async () => (await titles(page)).map((t) => t.split("\n")[0]))
    .toEqual(["Elastic", "prod-eu"]);

  // And it is remembered, or it is a chore rather than a feature.
  const sent = (await calls(page)).filter((c) => c.cmd === "reorder_profiles");
  expect(sent).toHaveLength(1);
  expect(sent[0].args.ids).toEqual(["p-es", "p-mysql"]);
});

test("Move up does the reverse", async ({ page }) => {
  await railOf(page, [MYSQL, ELASTIC]);

  await page.locator(".rail-item").last().click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Move up")').click();

  await expect
    .poll(async () => (await titles(page)).map((t) => t.split("\n")[0]))
    .toEqual(["Elastic", "prod-eu"]);
});

/** Present but unusable at the ends, rather than appearing and disappearing. */
test("the ends offer the move that cannot be made, disabled", async ({ page }) => {
  await railOf(page, [MYSQL, ELASTIC]);

  await page.locator(".rail-item").first().click({ button: "right" });
  await expect(page.locator('.ctx-menu button:has-text("Move up")')).toBeDisabled();
  await expect(page.locator('.ctx-menu button:has-text("Move down")')).toBeEnabled();
  await page.keyboard.press("Escape");

  await page.locator(".rail-item").last().click({ button: "right" });
  await expect(page.locator('.ctx-menu button:has-text("Move up")')).toBeEnabled();
  await expect(page.locator('.ctx-menu button:has-text("Move down")')).toBeDisabled();
});

// ------------------------------------------- Stage 14: read-only connections

/**
 * A connection can refuse writes. The flag lives on the profile; Rust narrows
 * the connection's capabilities at connect, and the whole UI keeps asking the
 * one question it already asked — `capabilities.writes`.
 */

/** MySQL, on a connection someone marked read-only. Writes, but not here. */
const READ_ONLY_MYSQL = { ...MYSQL_CAPS, writes: false, readOnly: true };

test("ticking Read-only saves it with the profile", async ({ page }) => {
  await installBackend(page, schemaBackend);
  await page.goto("/");

  await page.click("#btn-connect");
  await page.click("#conn-read-only");
  await page.click("#conn-ok");
  await expect(page.locator("#conn-dialog")).toBeHidden();

  const saved = (await calls(page)).find((c) => c.cmd === "save_profile");
  expect((saved?.args.profile as { readOnly?: boolean })?.readOnly).toBe(true);
});

test("a saved read-only connection is marked before you connect to it", async ({ page }) => {
  await railOf(page, [{ ...MYSQL, readOnly: true }]);

  // The mark is on the button, not only in the tooltip: the point is knowing
  // before running something, and a tooltip is read after reaching for it.
  await expect(page.locator('.rail-item [data-icon="lock"]')).toHaveCount(1);
  expect((await titles(page))[0]).toContain("Read-only");
});

test("an ordinary connection carries no such mark", async ({ page }) => {
  await railOf(page, [MYSQL]);
  await expect(page.locator('.rail-item [data-icon="lock"]')).toHaveCount(0);
  expect((await titles(page))[0]).not.toContain("Read-only");
});

/**
 * The menus follow the clamp, on an engine that *does* write. The read-only
 * *engine* case has been covered since Stage 11; what is new is that a
 * connection can now reach the same state, and the tree must not tell the
 * difference — it reads a capability, not an engine name.
 */
test("a read-only MySQL connection is offered no DROP", async ({ page }) => {
  await connect(page, { connect: () => ({ ...CONN_INFO, capabilities: READ_ONLY_MYSQL }) });
  await openDatabase(page);

  await page.locator('.node.table:has-text("orders")').click({ button: "right" });
  await expect(page.locator(".ctx-menu")).toBeVisible();
  await expect(page.locator('.ctx-menu button:has-text("Drop")')).toHaveCount(0);
  // And everything that reads is still there.
  await expect(page.locator('.ctx-menu button:has-text("Select first")')).toHaveCount(1);
});

/**
 * **R6, and the bug the test found on its way to being written.**
 *
 * The plan said the flag would take effect "on the next connect". Trying to
 * assert that turned up something worse: `connect_saved` connects from the
 * **file**, and the profile was written *after* connecting — so an edit that
 * reused the stored password connected with the values it was replacing.
 * Clearing Read-only looked like it did nothing, and ticking it did nothing
 * while looking exactly like protection.
 *
 * Not specific to this flag: editing a port on a connection with a remembered
 * password had the same shape, and had since Stage 2.
 *
 * So on that one path the profile is written first, and the order is what this
 * asserts — a count of connects would pass with the bug still in.
 */
test("saving an edit reconnects, so a change to the flag applies at once", async ({ page }) => {
  // A *saved* read-only profile, connected. The first draft of this stubbed
  // read-only capabilities onto a profile that was not marked, which the app
  // cannot produce — Rust derives the one from the other — and the test
  // failed on its own fiction rather than on the code.
  // A *saved* read-only profile with a remembered password — the path where
  // `connect_saved` is used. The first draft of this test stubbed read-only
  // capabilities onto a profile that was not marked, which the app cannot
  // produce (Rust derives one from the other), and it failed on its own
  // fiction rather than on the code.
  await installBackend(page, {
    ...schemaBackend,
    connect_saved: () => ({ ...CONN_INFO, capabilities: READ_ONLY_MYSQL }),
    list_profiles: () => ({ profiles: [{ ...MYSQL, readOnly: true }], warning: null }),
  });
  await page.goto("/");
  await page.locator(".rail-item").click();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);

  await page.locator(".rail-item").click({ button: "right" });
  await page.locator('.ctx-menu button:has-text("Edit")').click();
  await expect(page.locator("#conn-dialog")).toBeVisible();
  // It shows what was saved, so clearing it is one click.
  await expect(page.locator("#conn-read-only")).toBeChecked();
  await page.click("#conn-read-only");
  await page.click("#conn-ok");
  await expect(page.locator("#conn-dialog")).toBeHidden();

  const order = (await calls(page))
    .map((c) => c.cmd)
    .filter((c) => c === "save_profile" || c === "connect_saved");
  // Connected once by the rail click, then the edit: written down, *then*
  // reconnected. The second connect_saved is what re-reads the profile, so
  // the write has to come first or it reads the old one.
  expect(order).toEqual(["connect_saved", "save_profile", "connect_saved"]);

  const saved = (await calls(page)).filter((c) => c.cmd === "save_profile");
  expect((saved[0].args.profile as { readOnly?: boolean })?.readOnly).toBe(false);
});
