import { expect, test, type Page } from "@playwright/test";
import { calls, commandNames, installBackend, openDatabase, schemaBackend, visit } from "./harness";

/**
 * Stage 25 slice 4: the AI dictionary as a tab, and the tree's dots.
 *
 * The dictionary itself is Rust's (`dictionary.rs`, 22 unit tests) and the
 * prompt is `assistant.rs`'s. What is only answerable here is the shape:
 * that a dictionary tab is an editor with the SQL controls **off**, that the
 * dots say what the file describes, and that describing something from the
 * tree writes into the buffer rather than into the file.
 */

const DICT_PATH = "/work/warehouse/db/dictionary.toml";

const FILE = `version = 1

[tables.orders]
note = "One row per order."
columns.total = "Cents."
`;

/** A saved, connected MySQL profile with a dictionary attached. */
const BOUND = {
  id: "p1",
  name: "prod-eu",
  colour: "#3b82f6",
  host: "db.example.com",
  port: 3306,
  user: "reporting",
  database: "poc",
  allowInvalidCerts: false,
  kind: "mysql",
  url: "",
  auth: { type: "none" },
  noPassword: false,
  rememberPassword: true,
  needsSecret: false,
  dictionary: DICT_PATH,
};

const OPENED = {
  path: DICT_PATH,
  name: "dictionary.toml",
  contents: FILE,
  dialect: "mysql",
  encoding: "utf-8",
  lineEnding: "lf",
  bom: false,
  mtimeMs: 1_700_000_000_000,
  sizeBytes: FILE.length,
  large: false,
};

/** What the backend says about `poc`: `orders` described, its `total` too. */
const MARKS = { tables: { orders: ["total"] }, unmatched: [] };

const CHECK = {
  diagnostics: [],
  counts: { tables: 1, columns: 1 },
  unmatched: [],
  warning: null,
};

function backend(extra: Record<string, unknown> = {}) {
  return {
    ...schemaBackend,
    list_profiles: () => ({ profiles: [BOUND], warning: null }),
    connect_saved: () => ({
      id: "p1",
      serverVersion: "8.4.0",
      databases: ["poc"],
      currentDatabase: "poc",
      capabilities: { engine: "mysql", namespaceLabel: "database", writes: true, schemaExport: true },
    }),
    dictionary_open: () => OPENED,
    dictionary_marks: () => MARKS,
    dictionary_check: () => CHECK,
    dictionary_pick: () => DICT_PATH,
    dictionary_create: () => OPENED,
    ...extra,
  };
}

/** Connected, with the tree's `poc` open so the dots are on screen. */
async function live(page: Page, extra: Record<string, unknown> = {}) {
  await installBackend(page, backend(extra));
  await visit(page);
  await page.locator(".rail-item").click();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
}

/** Open the dictionary through the rail's context menu, as a person would. */
async function openDict(page: Page) {
  await page.locator(".rail-item").click({ button: "right" });
  await page.click('.ctx-menu button:has-text("AI dictionary")');
  await expect(page.locator("#dict-band")).toBeVisible();
}

// ------------------------------------------------------------ the tab

test("the dictionary opens as a tab holding the file", async ({ page }) => {
  await live(page);
  await openDict(page);

  // The file's text is in the editor, not in a dialog.
  await expect(page.locator(".cm-content")).toContainText("[tables.orders]");
  await expect(page.locator(".stab.active")).toContainText("dictionary.toml");
  await expect(page.locator("#file-note")).toContainText("AI dictionary");
});

/**
 * **The point of the shape.** An editor tab that runs nothing: Run, Run all,
 * Format and the three SQL toggles come off, because a Run button over a TOML
 * file is a control with nothing to act on.
 */
test("a dictionary tab has no SQL controls", async ({ page }) => {
  await live(page);
  await openDict(page);

  for (const sel of ["#btn-run", "#btn-run-all", "#btn-format", "#chk-autolimit", "#chk-lint", "#num-timeout"]) {
    await expect(page.locator(sel)).toBeHidden();
  }
  // And the editor itself is still there — this is a buffer, not a view.
  await expect(page.locator(".cm-content")).toBeVisible();
});

/**
 * The keys reach the editor directly, so hiding the buttons is not the guard.
 *
 * **Format is the one that distinguishes.** `run()` has asked "is this a SQL
 * tab" since the migrations view arrived, so Ctrl+Enter was already safe —
 * but `formatBuffer` asked whether the *migrations view* was up, which a
 * dictionary tab is not, so Ctrl+Shift+F would have sent this TOML to the SQL
 * formatter and replaced the buffer with the answer. Found by a falsification
 * that passed.
 */
test("the run and format keys do nothing on a dictionary tab", async ({ page }) => {
  await live(page);
  await openDict(page);
  const before = await page.locator(".cm-content").textContent();

  await page.locator(".cm-content").click();
  await page.keyboard.press("Control+Enter");
  await page.keyboard.press("Control+Shift+Enter");
  await page.keyboard.press("Control+Shift+F");

  const names = await commandNames(page);
  expect(names).not.toContain("run_script");
  expect(names).not.toContain("format_sql");
  // The buffer is the user's TOML, byte for byte.
  expect(await page.locator(".cm-content").textContent()).toBe(before);
});

/**
 * Who else reads this file is the first thing somebody about to rewrite a
 * sentence should know — it is three environments' description, not one's.
 */
test("the band names the connections that share the file and what it says", async ({ page }) => {
  await live(page);
  await openDict(page);

  const band = page.locator("#dict-band");
  await expect(band).toContainText("Used by");
  await expect(band).toContainText("prod-eu");
  await expect(band).toContainText("1 table, 1 column");
});

test("notes that match nothing are counted in the band", async ({ page }) => {
  await live(page, {
    dictionary_check: () => ({
      ...CHECK,
      unmatched: [
        { table: "oders", column: null },
        { table: "orders", column: "stock_lvl" },
      ],
    }),
  });
  await openDict(page);

  const stale = page.locator("#dict-band .stale");
  await expect(stale).toContainText("2 match nothing");
  // The names are in the tooltip rather than the band: the band is a glance.
  await expect(stale).toHaveAttribute("title", /oders/);
  await expect(stale).toHaveAttribute("title", /orders\.stock_lvl/);
});

// ------------------------------------------------------------ the tree

test("the tree marks what the dictionary describes, and only that", async ({ page }) => {
  await live(page);
  await openDatabase(page);

  const described = page.locator('.node.table:has(.label:text-is("orders")) .dict-dot');
  await expect(described).toHaveClass(/on/);
  await expect(described).toHaveAttribute("title", /describes this table/);

  // `users` is a table nobody wrote about: the slot is there, unfilled, so the
  // names stay in one column.
  const plain = page.locator('.node.table:has(.label:text-is("users")) .dict-dot');
  await expect(plain).toHaveCount(1);
  await expect(plain).not.toHaveClass(/on/);
});

test("a described column is marked too", async ({ page }) => {
  await live(page);
  await openDatabase(page);
  await page.locator('.node.table:has(.label:text-is("orders"))').click();
  await expect(page.locator('.node.column:has(.label:text-is("total"))')).toBeVisible();

  await expect(
    page.locator('.node.column:has(.label:text-is("total")) .dict-dot'),
  ).toHaveClass(/on/);
  await expect(
    page.locator('.node.column:has(.label:text-is("user_id")) .dict-dot'),
  ).not.toHaveClass(/on/);
});

/**
 * **Into the buffer, never into the file.** A tree action that saved behind an
 * open editor would fight whatever was being typed; one that refused while the
 * tab was dirty would be a dead menu item.
 */
test("describing a table from the tree inserts a stub rather than saving", async ({ page }) => {
  await live(page);
  await openDatabase(page);

  await page.locator('.node.table:has(.label:text-is("users"))').click({ button: "right" });
  await page.click('.ctx-menu button:has-text("Describe for the assistant")');

  await expect(page.locator("#dict-band")).toBeVisible();
  await expect(page.locator(".cm-content")).toContainText("[tables.users]");
  // Nothing was written: the file is saved by the person, with Ctrl+S.
  expect(await commandNames(page)).not.toContain("save_file");
  // And the tab now differs from what was loaded.
  await expect(page.locator(".stab.active")).toContainText("dictionary.toml");
});

/** A table that already has a section is not given a second one. */
test("describing a described table moves the cursor instead of duplicating", async ({ page }) => {
  await live(page);
  await openDatabase(page);

  await page.locator('.node.table:has(.label:text-is("orders"))').click({ button: "right" });
  await page.click('.ctx-menu button:has-text("Edit the dictionary note")');

  const text = await page.locator(".cm-content").textContent();
  expect((text ?? "").match(/\[tables\.orders\]/g) ?? []).toHaveLength(1);
});

/**
 * With nothing attached, the menu offers to attach one — and the file the
 * person picks is remembered on the profile, which is what makes it survive a
 * restart.
 */
test("a connection with no dictionary is offered one, and remembers it", async ({ page }) => {
  await live(page, {
    list_profiles: () => ({ profiles: [{ ...BOUND, dictionary: null }], warning: null }),
  });

  await page.locator(".rail-item").click({ button: "right" });
  await page.click('.ctx-menu button:has-text("Add an AI dictionary")');
  await expect(page.locator("dialog.ask")).toBeVisible();
  await page.click('dialog.ask button:has-text("Choose an existing file")');

  await expect.poll(async () => commandNames(page)).toContain("save_profile");
  const saved = (await calls(page)).find((c) => c.cmd === "save_profile");
  expect((saved?.args.profile as { dictionary?: string })?.dictionary).toBe(DICT_PATH);
});
