import { expect, test, type Page } from "@playwright/test";
import { calls, installBackend } from "./harness";

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
