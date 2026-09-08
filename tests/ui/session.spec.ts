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
 * Tabs remembered across restarts.
 *
 * **This feature's only failure mode is losing work**, which is exactly what it
 * exists to prevent — so most of what is asserted here is what happens when
 * something is wrong: the file is gone, the file changed, the connection never
 * comes back. The happy path is one test; the rest are the ones that matter.
 *
 * Every test here connects through a **saved** profile, because that is what a
 * restart actually looks like: a rail with a remembered server on it, and one
 * click. A one-off connection mints a fresh id and by definition has nothing
 * stored under it.
 */

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

const FILE = {
  path: "/home/u/queries/report.sql",
  name: "report.sql",
  contents: "SELECT 1;\n",
  sizeBytes: 10,
  dialect: "mysql",
  encoding: "utf-8",
  lineEnding: "lf",
  mtimeMs: 1_700_000_000_000,
  large: false,
};

/** A stored tab with every field the frontend reads. */
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

/** Boot with a saved connection on the rail, and nothing connected. */
async function boot(page: Page, extra: Backend = {}) {
  await installBackend(page, {
    ...schemaBackend,
    list_profiles: () => ({ profiles: [SAVED], warning: null }),
    connect_saved: () => CONN,
    ...extra,
  });
  await page.goto("/");
  await expect(page.locator(".rail-item")).toHaveCount(1);
}

/** Boot, then click the saved connection — the restart, in two lines. */
async function reopen(page: Page, extra: Backend = {}) {
  await boot(page, extra);
  await page.locator(".rail-item").click();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
}

/** The most recent `save_session` payload the UI wrote. */
async function saved(page: Page) {
  const all = (await calls(page)).filter((c) => c.cmd === "save_session");
  return all.length
    ? (all[all.length - 1].args.session as {
        connections: Array<{
          connectionId: string;
          activeIndex: number;
          tabs: Array<Record<string, unknown>>;
        }>;
      })
    : null;
}

/** Writes are debounced, so waiting for one is the honest thing to do. */
async function waitForSave(page: Page) {
  await expect
    .poll(async () => (await commandNames(page)).filter((c) => c === "save_session").length, {
      timeout: 5000,
    })
    .toBeGreaterThan(0);
}

// ------------------------------------------------------------------ restoring

test("S1 — a remembered buffer comes back on connect, and brings no stray tab", async ({
  page,
}) => {
  await reopen(page, {
    load_session: () => session([storedTab({ title: "scratch" })]),
  });

  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
  await expect(page.locator("#script-tabs .stab.active .stab-label")).toHaveText("scratch");
  expect(await editorText(page)).toContain("remembered");
});

/**
 * The empty tab is the thing this most easily gets wrong: `setActiveConnection`
 * creates one for any workspace it finds empty, and it runs a moment after the
 * connection opens. Restoring has to finish first, which is why `onConnected`
 * is awaited.
 */
test("restoring wins the race against the automatic empty tab", async ({ page }) => {
  await reopen(page, {
    load_session: () =>
      session([storedTab({ title: "a", text: "-- a" }), storedTab({ title: "b", text: "-- b" })], 1),
  });

  await expect(page.locator("#script-tabs .stab")).toHaveCount(2);
  await expect(page.locator("#script-tabs .stab .stab-label")).toHaveText(["a", "b"]);
  // And the tab that was in front is in front again.
  await expect(page.locator("#script-tabs .stab.active .stab-label")).toHaveText("b");
  expect(await editorText(page)).toContain("-- b");
});

test("S5 — nothing connects at boot", async ({ page }) => {
  await boot(page, { load_session: () => session([storedTab()]) });

  // The workspace is not restored, and — the point — nothing was opened.
  await expect(page.locator("#script-tabs .stab")).toHaveCount(0);
  const names = await commandNames(page);
  expect(names).not.toContain("connect");
  expect(names).not.toContain("connect_saved");
});

// ------------------------------------------------------- files, and their absence

test("S2 — a clean file-backed tab is re-read from disk, not from the session", async ({
  page,
}) => {
  const onDisk = { ...FILE, contents: "SELECT 'from disk';\n", mtimeMs: 1_800_000_000_000 };
  await reopen(page, {
    load_session: () =>
      session([
        storedTab({
          title: "report.sql",
          filePath: FILE.path,
          mtimeMs: FILE.mtimeMs,
          // Clean: the session file holds no copy of the text.
          text: null,
          untitledNumber: null,
        }),
      ]),
    read_file: () => onDisk,
  });

  await expect(page.locator("#script-tabs .stab .stab-label")).toHaveText("report.sql");
  expect(await editorText(page)).toContain("from disk");
  // It *is* the file, so it is clean.
  await expect(page.locator("#script-tabs .stab.dirty")).toHaveCount(0);
});

test("S3 — a file-backed tab with unsaved edits comes back dirty, edits intact", async ({
  page,
}) => {
  await reopen(page, {
    load_session: () =>
      session([
        storedTab({
          title: "report.sql",
          filePath: FILE.path,
          mtimeMs: FILE.mtimeMs,
          text: "SELECT 'edited but never saved';\n",
          untitledNumber: null,
        }),
      ]),
    read_file: () => FILE,
  });

  expect(await editorText(page)).toContain("never saved");
  await expect(page.locator("#script-tabs .stab.dirty")).toHaveCount(1);
});

/**
 * The file moved on while the app was closed. The tab keeps the mtime it was
 * *based on*, so the first Save runs into the existing conflict check rather
 * than silently overwriting whatever someone else wrote.
 */
test("S4 — a restored dirty tab still saves against the version it knew", async ({ page }) => {
  await reopen(page, {
    load_session: () =>
      session([
        storedTab({
          title: "report.sql",
          filePath: FILE.path,
          mtimeMs: FILE.mtimeMs,
          text: "SELECT 'mine';\n",
          untitledNumber: null,
        }),
      ]),
    // Disk has moved on since the tab was stored.
    read_file: () => ({ ...FILE, contents: "SELECT 'theirs';\n", mtimeMs: 1_900_000_000_000 }),
    save_file: () => ({ path: FILE.path, name: FILE.name, mtimeMs: 2_000_000_000_000 }),
  });

  await page.keyboard.press("Control+s");
  await expect
    .poll(async () => (await calls(page)).find((c) => c.cmd === "save_file")?.args.expectMtime)
    .toBe(FILE.mtimeMs);
});

test("a vanished file takes a clean tab with it, and says so", async ({ page }) => {
  await reopen(page, {
    load_session: () =>
      session([
        storedTab({ title: "gone.sql", filePath: "/gone.sql", text: null, untitledNumber: null }),
        storedTab({ title: "scratch", text: "-- kept" }),
      ]),
    read_file: () => {
      throw new Error("No such file");
    },
  });

  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
  await expect(page.locator("#script-tabs .stab .stab-label")).toHaveText("scratch");
  await expect(page.locator("dialog.ask")).toBeVisible();
  await expect(page.locator("dialog.ask")).toContainText("gone.sql");
});

/**
 * The one case where dropping the tab would destroy something: the buffer is
 * the only copy. It keeps its text and lets go of the path, so Save asks where
 * to put it instead of failing.
 */
test("a vanished file does NOT take unsaved edits with it", async ({ page }) => {
  await reopen(page, {
    load_session: () =>
      session([
        storedTab({
          title: "gone.sql",
          filePath: "/gone.sql",
          mtimeMs: FILE.mtimeMs,
          text: "SELECT 'the only copy';\n",
          untitledNumber: null,
        }),
      ]),
    read_file: () => {
      throw new Error("No such file");
    },
  });

  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
  expect(await editorText(page)).toContain("the only copy");

  // The path is gone, so Save must ask rather than write to nowhere.
  await page.keyboard.press("Control+s");
  await expect.poll(async () => await commandNames(page)).toContain("save_file_dialog");
});

// ------------------------------------------------------------------ what is written

test("a clean file-backed tab stores its path, never its text", async ({ page }) => {
  await reopen(page, { open_file_dialog: () => FILE, read_file: () => FILE });
  await page.keyboard.press("Control+o");
  await expect(page.locator("#script-tabs .stab.active .stab-label")).toHaveText("report.sql");
  await waitForSave(page);

  await expect
    .poll(async () => (await saved(page))?.connections[0]?.tabs.find((t) => t.filePath === FILE.path))
    .toMatchObject({ filePath: FILE.path, text: null, mtimeMs: FILE.mtimeMs });
});

/** The moment it is edited, the buffer becomes the only copy and is written. */
test("editing a file-backed tab starts storing its text", async ({ page }) => {
  await reopen(page, { open_file_dialog: () => FILE, read_file: () => FILE });
  await page.keyboard.press("Control+o");
  await expect(page.locator("#script-tabs .stab.active .stab-label")).toHaveText("report.sql");

  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("-- unsaved");

  await expect
    .poll(
      async () =>
        (await saved(page))?.connections[0]?.tabs.find((t) => t.filePath === FILE.path)?.text,
    )
    .toContain("-- unsaved");
});

/** An untitled buffer is the only copy from the first character. */
test("an untitled buffer is always stored with its text", async ({ page }) => {
  await reopen(page);
  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("SELECT 'scratch'");

  await expect
    .poll(async () => (await saved(page))?.connections[0]?.tabs[0])
    .toMatchObject({ filePath: null });
  await expect
    .poll(async () => (await saved(page))?.connections[0]?.tabs[0]?.text)
    .toContain("scratch");
});

test("S6 — a result is never written down", async ({ page }) => {
  await reopen(page, {
    run_script: () => ({
      statements: [
        {
          sql: "SELECT 1",
          effectiveSql: null,
          kind: "select",
          outcome: {
            type: "rows",
            columns: [{ name: "id", sqlType: "INT", typeHint: "numeric" }],
            rows: [[1]],
            truncated: false,
          },
          elapsedMs: 1,
        },
      ],
      totalElapsedMs: 1,
      abortedAt: null,
      delimiterDetected: false,
      cancelled: false,
      timedOut: false,
    }),
  });
  await page.click("#btn-run-all");
  await expect(page.locator("table.rs")).toBeVisible();
  await waitForSave(page);

  const s = await saved(page);
  const written = JSON.stringify(s);
  expect(written).not.toContain("rows");
  expect(written).not.toContain("columns");
});

/**
 * The mistake that would quietly delete everything: connecting to one server
 * and writing a session file that only mentions that server.
 */
test("connecting to one server does not forget another's tabs", async ({ page }) => {
  await reopen(page, {
    load_session: () => ({
      session: {
        version: 1,
        connections: [
          { connectionId: SAVED.id, tabs: [storedTab({ title: "here" })], activeIndex: 0 },
          { connectionId: "other", tabs: [storedTab({ title: "elsewhere" })], activeIndex: 0 },
        ],
      },
      warning: null,
    }),
  });
  await expect(page.locator("#script-tabs .stab .stab-label")).toHaveText("here");
  await waitForSave(page);

  await expect
    .poll(async () => (await saved(page))?.connections.map((c) => c.connectionId).sort())
    .toEqual([SAVED.id, "other"]);

  const other = (await saved(page))!.connections.find((c) => c.connectionId === "other")!;
  expect(other.tabs[0].title).toBe("elsewhere");
});

test("reconnecting inside one session does not restore the tabs twice", async ({ page }) => {
  await reopen(page, {
    load_session: () => session([storedTab({ title: "once" })]),
    disconnect: () => null,
  });
  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);

  await page.click("#btn-connect"); // now labelled Disconnect
  await expect(page.locator("#grid .empty")).toContainText("Disconnected");
  await page.locator(".rail-item").click();

  await expect(page.locator("#grid .empty")).toContainText("Connected");
  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
});

// ------------------------------------------------------------------- quitting

/**
 * Quitting asks about files, not buffers. An untitled buffer is remembered now,
 * so prompting about it is prompting about work that is provably safe — which
 * is how people learn to click through prompts.
 */
test("an untitled buffer no longer blocks quitting", async ({ page }) => {
  await reopen(page);
  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("SELECT 1");
  await expect(page.locator("#script-tabs .stab.dirty")).toHaveCount(1);

  await fireEvent(page, "tauri://close-requested");
  await expect(page.locator("dialog.ask")).toBeHidden();
});

/** A file with unsaved edits still does: what persists is the buffer, not it. */
test("a file with unsaved edits still blocks quitting", async ({ page }) => {
  await reopen(page, {
    load_session: () =>
      session([
        storedTab({
          title: "report.sql",
          filePath: FILE.path,
          mtimeMs: FILE.mtimeMs,
          text: "SELECT 'edited';\n",
          untitledNumber: null,
        }),
      ]),
    read_file: () => FILE,
  });
  await expect(page.locator("#script-tabs .stab.dirty")).toHaveCount(1);

  await fireEvent(page, "tauri://close-requested");
  await expect(page.locator("dialog.ask")).toBeVisible();
  await expect(page.locator("dialog.ask")).toContainText("report.sql");
});

/**
 * S8 — the debounce is what bounds how much a crash costs, so quitting has to
 * beat it. Type, then quit immediately: the keystrokes must be on disk before
 * the window is allowed to go.
 */
test("quitting flushes what the debounce is still holding", async ({ page }) => {
  await reopen(page);
  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("SELECT 'typed just now'");

  await fireEvent(page, "tauri://close-requested");

  await expect
    .poll(async () => JSON.stringify(await saved(page)))
    .toContain("typed just now");
});
