/**
 * Running the real frontend in a real browser, against a fake backend.
 *
 * # Why this works at all
 *
 * `@tauri-apps/api`'s `invoke` is one line: `window.__TAURI_INTERNALS__.invoke(
 * cmd, args, options)`. So the entire Rust boundary is a single object on
 * `window`. Replace it before the page loads and the whole app — CodeMirror,
 * the schema tree, the grid, the dialogs — runs in Chromium or WebKit with no
 * changes to `src/` at all.
 *
 * # Why not drive the real app
 *
 * `tauri-driver` exists and is the official route, but it needs
 * `WebKitWebDriver` (an apt package) on Linux, has **no macOS support at all**,
 * and boots a native window per test. It is the right tool for a handful of
 * "does the shell work" smoke tests and the wrong one for the fifty
 * interaction tests this app actually needs.
 *
 * Nearly every frontend bug this project has shipped — the running-tab result,
 * the dead context menu, the silent delete, the inert Copy button — was a DOM
 * and event-ordering bug. Those need a real browser and real events, not a real
 * webview.
 */
import type { Page } from "@playwright/test";

/** A command handler: return the value the Rust command would have returned. */
export type Backend = Record<string, (args: Record<string, unknown>) => unknown>;

/**
 * A stand-in for the Rust `clipboard_text` command.
 *
 * **The seam this reveals is worth naming.** The delimited text is built in
 * Rust, so a browser test can never exercise the real formatting — quoting,
 * NULL policy, the lot. That is correct layering, not a gap: `export.rs` owns
 * the format and has unit tests for it, and these tests own the question Rust
 * cannot answer — *did the right rows and columns get that far, and did the
 * clipboard actually receive the answer?*
 *
 * So this mirrors the shape faithfully enough for that, and deliberately no
 * more. If it and `export::to_csv` ever disagree about something a test
 * asserts, the test is asking the wrong layer.
 */
function tsv(args: Record<string, unknown>): string {
  const result = args.result as {
    columns: Array<{ name: string }>;
    rows: Array<Array<unknown>>;
  };
  const opts = args.options as { headers: boolean; nullAs: string };
  const cell = (v: unknown) =>
    v === null ? opts.nullAs : typeof v === "boolean" ? (v ? "1" : "0") : String(v);
  const lines: string[] = [];
  if (opts.headers) lines.push(result.columns.map((c) => c.name).join("\t"));
  for (const r of result.rows) lines.push(r.map(cell).join("\t"));
  return lines.join("\n") + "\n";
}

/** Commands every boot needs, so a test only declares what it cares about. */
export const baseBackend: Backend = {
  clipboard_text: (args) => tsv(args),
  app_defaults: () => ({ browseLimit: 1000, maxRows: 5000 }),
  list_profiles: () => ({ profiles: [], warning: null }),
  open_tab: () => null,
  close_tab: () => null,
  lint_sql: () => [],
  tab_status: () => ({ connected: false, running: false, currentDatabase: null, connectionId: 0 }),
  list_connections: () => [],
};

/**
 * Install the fake bridge. Must run before any app code, which is what
 * `addInitScript` guarantees.
 *
 * Every call is recorded on `window.__CALLS__` so a test can assert what the UI
 * *asked the backend to do* — which is the only way to check that a menu item
 * generated the right SQL without also running it.
 */
/**
 * Handlers per page. `exposeFunction` can only register a name once, so a test
 * that installs a backend twice — swapping one command for another mid-test —
 * would otherwise fail on the second call rather than on anything real.
 */
const handlers = new WeakMap<Page, Backend>();

export async function installBackend(page: Page, backend: Backend = {}) {
  const merged = { ...baseBackend, ...backend };
  const names = Object.keys(merged);

  // Second and later calls just swap the handlers behind the already-exposed
  // function. The init scripts have run; re-adding them would do nothing.
  if (handlers.has(page)) {
    handlers.set(page, merged);
    return;
  }
  handlers.set(page, merged);

  await page.addInitScript(
    ([names]: [string[]]) => {
      const calls: Array<{ cmd: string; args: unknown }> = [];
      (window as unknown as Record<string, unknown>).__CALLS__ = calls;
      (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
        // The shape `@tauri-apps/api/mocks` installs. Without it,
        // `getCurrentWebview()` throws during boot and — because it is a
        // top-level call — silently aborts the rest of the startup script.
        metadata: {
          currentWindow: { label: "main" },
          currentWebview: { windowLabel: "main", label: "main" },
        },
        transformCallback: (cb: unknown) => cb,
        unregisterCallback: () => {},
        convertFileSrc: (p: string) => p,
        invoke: async (cmd: string, args: Record<string, unknown>) => {
          calls.push({ cmd, args });
          if (!names.includes(cmd)) {
            // Plugin channels (`plugin:event|listen`) are expected and inert.
            if (cmd.startsWith("plugin:")) return 0;
            throw new Error(`unstubbed command: ${cmd}`);
          }
          const fn = (window as unknown as Record<string, Record<string, unknown>>)
            .__BACKEND__[cmd] as (a: unknown) => unknown;
          return fn(args);
        },
      };
    },
    [names] as [string[]],
  );

  // The handlers themselves are exposed separately: they are real functions and
  // cannot be serialised into an init script.
  await page.exposeFunction("__dispatch__", (cmd: string, args: Record<string, unknown>) => {
    const current = handlers.get(page)!;
    const fn = current[cmd];
    if (!fn) throw new Error(`unstubbed command: ${cmd}`);
    return fn(args);
  });
  await page.addInitScript(
    ([names]: [string[]]) => {
      const backend: Record<string, unknown> = {};
      for (const n of names) {
        backend[n] = (args: Record<string, unknown>) =>
          (window as unknown as Record<string, (c: string, a: unknown) => unknown>).__dispatch__(
            n,
            args,
          );
      }
      (window as unknown as Record<string, unknown>).__BACKEND__ = backend;
    },
    [names] as [string[]],
  );
}

/** Commands the UI invoked, in order. */
export async function calls(page: Page): Promise<Array<{ cmd: string; args: Record<string, unknown> }>> {
  return page.evaluate(
    () => (window as unknown as { __CALLS__: Array<{ cmd: string; args: Record<string, unknown> }> }).__CALLS__,
  );
}

/** A result set shaped the way `run_script` returns one. */
export function rowsResult(
  columns: Array<{ name: string; sqlType?: string }>,
  rows: unknown[][],
  opts?: { truncated?: boolean; sql?: string },
) {
  return {
    statements: [
      {
        sql: opts?.sql ?? "SELECT 1",
        // `string | null`: a test that pins the auto-LIMIT chip sets this.
        effectiveSql: null as string | null,
        kind: "select",
        outcome: {
          type: "rows",
          columns: columns.map((c) => ({
            name: c.name,
            sqlType: c.sqlType ?? "VARCHAR",
            typeHint: (c.sqlType ?? "VARCHAR").match(/INT|DECIMAL|DOUBLE|FLOAT/i)
              ? "numeric"
              : "text",
          })),
          rows,
          truncated: opts?.truncated ?? false,
        },
        elapsedMs: 1,
      },
    ],
    totalElapsedMs: 1,
    abortedAt: null,
    delimiterDetected: false,
    cancelled: false,
    timedOut: false,
  };
}

// ---------------------------------------------------------------- fixtures

/** The seeded fixture, mirrored so tree tests read like the real thing. */
export const SCHEMA = {
  tables: [
    { name: "users", kind: "BASE TABLE" },
    { name: "orders", kind: "BASE TABLE" },
    { name: "big", kind: "BASE TABLE" },
    { name: "user_totals", kind: "VIEW" },
  ],
  columns: {
    users: [
      { name: "id", dataType: "int", nullable: false, key: "PRI" },
      { name: "email", dataType: "varchar(190)", nullable: false, key: "UNI" },
      { name: "display_name", dataType: "varchar(80)", nullable: true, key: null },
    ],
    orders: [
      { name: "id", dataType: "int", nullable: false, key: "PRI" },
      { name: "user_id", dataType: "int", nullable: false, key: "MUL" },
      { name: "total", dataType: "decimal(12,2)", nullable: false, key: null },
    ],
  } as Record<string, unknown[]>,
  routines: [
    {
      name: "top_spenders",
      kind: "procedure",
      returns: null,
      params: [
        { name: "min_total", mode: "IN", dataType: "decimal(12,2)" },
        { name: "max_rows", mode: "IN", dataType: "int" },
      ],
    },
    { name: "ping_poc", kind: "procedure", returns: null, params: [] },
    {
      name: "order_count",
      kind: "function",
      returns: "int",
      params: [{ name: "uid", mode: "IN", dataType: "int" }],
    },
  ],
};

export const CONN_INFO = {
  id: "c1",
  serverVersion: "8.4.0",
  databases: ["poc"],
  currentDatabase: "poc",
};

/** The schema-side commands, so a tree test declares only what it changes. */
export const schemaBackend: Backend = {
  connect: () => CONN_INFO,
  save_profile: (a) => ({
    profile: { ...(a.profile as object), rememberPassword: false },
    passwordWarning: null,
    passwordStored: false,
  }),
  list_tables: () => SCHEMA.tables,
  list_columns: (a) => SCHEMA.columns[a.table as string] ?? [],
  list_routines: () => SCHEMA.routines,
  refresh_schema: () => null,
  use_database: () => null,
  // Text generation lives in Rust and is tested there. What these stubs make
  // checkable is the *arguments* — that the tree asked for the right thing.
  generate_select: (a) =>
    `SELECT *\nFROM \`${a.db}\`.\`${a.table}\`\nLIMIT ${a.limit};\n`,
  generate_drop: (a) => `-- generated\nDROP <${JSON.stringify(a.target)}>;\n`,
  generate_call: (a) => `-- PROCEDURE\nCALL \`${a.db}\`.\`${a.name}\`();\n`,
  routine_ddl: (a) =>
    `USE \`${a.db}\`;\nDROP ${String(a.kind).toUpperCase()} IF EXISTS \`${a.name}\`;\nDELIMITER $$\nCREATE ...END$$\nDELIMITER ;\n`,
};

/** Boot, connect through the real dialog, and land with a schema tree. */
export async function connect(page: Page, extra: Backend = {}) {
  await installBackend(page, { ...schemaBackend, ...extra });
  await page.goto("/");
  await page.click("#btn-connect");
  await page.locator("#conn-dialog").waitFor({ state: "visible" });
  await page.click("#conn-ok");
  await page.locator("#conn-dialog").waitFor({ state: "hidden" });
}

/** Expand the database node so its groups render. */
export async function openDatabase(page: Page, db = "poc") {
  await page.locator(`.node.db:has-text("${db}")`).click();
  await page.locator('.node.group:has-text("Tables")').waitFor();
}

/** Open a named group and wait for its children. */
export async function openGroup(page: Page, label: string) {
  await page.locator(`.node.group:has-text("${label}")`).click();
}

/** The text of the active editor tab. */
export async function editorText(page: Page): Promise<string> {
  return page.locator("#editor .cm-content").innerText();
}

/** Every command name the UI has invoked so far. */
export async function commandNames(page: Page): Promise<string[]> {
  return (await calls(page)).map((c) => c.cmd);
}
