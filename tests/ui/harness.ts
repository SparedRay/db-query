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
  // Every boot checks for an update. Stubbed as "nothing new" so no other test
  // has to think about it, and overridden by the ones that do.
  update_check: () => ({ type: "upToDate", current: "0.0.0-test" }),
  list_profiles: () => ({ profiles: [], warning: null }),
  // Every boot reads the session file. Stubbed as "nothing remembered" so no
  // other test has to think about it, and overridden by the ones that do.
  load_session: () => ({ session: { version: 1, connections: [] }, warning: null }),
  save_session: () => null,
  open_tab: () => null,
  close_tab: () => null,
  lint_sql: () => [],
  tab_status: () => ({ connected: false, running: false, currentDatabase: null, connectionId: 0 }),
  list_connections: () => [],
  // Every run records history; every boot may open the dialog. Stubbed empty so
  // no other test has to think about it.
  history_search: () => [],
  assistant_status: () => ({ hasKey: false, ready: false, local: false }),
  remember_proposal: () => null,
  history_clear: () => null,
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

      // Every callback the app hands to Tauri, by id.
      //
      // The real runtime returns an integer from `transformCallback` and calls
      // back by id. Returning the function itself was simpler, but it made
      // streaming untestable: `Channel` is built entirely on that id, and a
      // function cannot travel through invoke args — Playwright cannot
      // serialise one.
      let callbackSeq = 0;
      const callbacks: Record<number, (arg: unknown) => unknown> = {};
      const listeners: Array<{ event: string; id: number }> = [];

      // Dispatch without awaiting the handler. A close-requested handler that
      // opens a dialog does not settle until someone answers it — awaiting here
      // would deadlock the very test that wants to see the dialog.
      (window as unknown as Record<string, unknown>).__FIRE__ = (
        event: string,
        payload: unknown,
      ) => {
        for (const l of listeners) {
          if (l.event === event) void callbacks[l.id]?.({ event, id: 0, payload });
        }
      };

      // Push one message into a `Channel`.
      //
      // The index is kept here, per channel, rather than asked of the caller.
      // `Channel` buffers out-of-order messages and silently holds anything
      // arriving before the index it is waiting for — so a test that sent a
      // second batch starting from zero would simply see nothing, with no
      // error. Counting here makes that impossible, and matches the runtime,
      // where Rust owns the counter.
      const channelIndex: Record<number, number> = {};
      (window as unknown as Record<string, unknown>).__CHANNEL_SEND__ = (
        id: number,
        message: unknown,
      ) => {
        const index = channelIndex[id] ?? 0;
        channelIndex[id] = index + 1;
        callbacks[id]?.({ index, message });
      };
      (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
        // The shape `@tauri-apps/api/mocks` installs. Without it,
        // `getCurrentWebview()` throws during boot and — because it is a
        // top-level call — silently aborts the rest of the startup script.
        metadata: {
          currentWindow: { label: "main" },
          currentWebview: { windowLabel: "main", label: "main" },
        },
        transformCallback: (cb: unknown) => {
          const id = ++callbackSeq;
          callbacks[id] = cb as (arg: unknown) => unknown;
          return id;
        },
        unregisterCallback: () => {},
        convertFileSrc: (p: string) => p,
        invoke: async (cmd: string, args: Record<string, unknown>) => {
          calls.push({ cmd, args });
          if (!names.includes(cmd)) {
            // `listen` is the one plugin channel worth modelling: window events
            // are how quitting reaches the app, and the close handler is where
            // unsaved work is decided. Everything else is inert.
            if (cmd === "plugin:event|listen") {
              listeners.push({ event: String(args.event), id: Number(args.handler) });
              return 0;
            }
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

/** MySQL's capabilities: everything this app has ever been able to do. */
export const MYSQL_CAPS = {
  engine: "mysql",
  writes: true,
  transactions: true,
  multiStatement: true,
  delimiterBlocks: true,
  routines: true,
  cancellation: true,
  streamingExport: true,
  rowCap: "clientLimit",
  namespaceLabel: "database",
};

/** A read-only engine, for testing what the UI must not offer. */
export const READ_ONLY_CAPS = {
  ...MYSQL_CAPS,
  engine: "elasticsearch",
  writes: false,
  transactions: false,
  delimiterBlocks: false,
  routines: false,
  streamingExport: false,
  rowCap: "serverPageSize",
  namespaceLabel: "catalog",
};

export const CONN_INFO = {
  id: "c1",
  serverVersion: "8.4.0",
  databases: ["poc"],
  currentDatabase: "poc",
  capabilities: MYSQL_CAPS,
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
  table_ddl: (a) =>
    `CREATE TABLE \`${a.table}\` (\n  \`id\` int NOT NULL\n);\n`,
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

/**
 * Fire a Tauri window event the way the runtime would.
 *
 * `tauri://close-requested` is the one that matters: the app's handler decides
 * whether quitting proceeds, and it is the last chance to write anything down.
 */
export async function fireEvent(page: Page, event: string, payload: unknown = null) {
  await page.evaluate(
    ([event, payload]) =>
      (window as unknown as { __FIRE__: (e: string, p: unknown) => void }).__FIRE__(event, payload),
    [event, payload] as [string, unknown],
  );
}

/**
 * Push messages into the `Channel` an invoke was given.
 *
 * `cmd` names the command whose most recent call carried the channel. The
 * channel arrives in the args as `__CHANNEL__:<id>` — the same shape the real
 * runtime sends — so this reads the id back out of what the app actually sent,
 * rather than out of a stub.
 */
export async function sendOnChannel(
  page: Page,
  cmd: string,
  argName: string,
  messages: unknown[],
) {
  const all = (await calls(page)).filter((c) => c.cmd === cmd);
  const last = all[all.length - 1];
  if (!last) throw new Error(`no call to ${cmd} to answer`);
  // Two shapes, because two things serialise it. The live runtime calls the
  // channel's `toJSON` and sends `__CHANNEL__:<id>`; reading `__CALLS__` back
  // out of the page structured-clones the object instead, which keeps its
  // public `id` and drops the rest. Accept either rather than depend on which.
  const arg = last.args[argName] as unknown;
  const id =
    typeof arg === "string"
      ? Number(arg.replace("__CHANNEL__:", ""))
      : Number((arg as { id?: number } | null)?.id);
  if (!id) {
    throw new Error(`${cmd}.${argName} was not a channel: ${JSON.stringify(arg)}`);
  }

  await page.evaluate(
    ([id, messages]) => {
      const send = (window as unknown as {
        __CHANNEL_SEND__: (i: number, m: unknown) => void;
      }).__CHANNEL_SEND__;
      for (const m of messages as unknown[]) send(id as number, m);
    },
    [id, messages] as [number, unknown[]],
  );
}
