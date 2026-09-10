// Typed mirror of the Rust command surface. Keep in sync with src-tauri/src.
import { Channel, invoke } from "@tauri-apps/api/core";

/**
 * Everything about a connection except its secret. **This is exactly what is
 * written to the config file.**
 *
 * Deliberately holds no password, so keeping it secret-free by construction
 * means the config file cannot leak one even by accident. It also holds nothing
 * derived — see `ProfileView` for why that separation matters.
 */
/**
 * What an update check found. Mirrors Rust's `UpdateStatus`.
 *
 * `unsupported` is not an error: the Linux build ships as a `.deb`, which the
 * updater cannot replace in place, and saying so up front beats failing after
 * a download.
 */
export type UpdateStatus =
  | { type: "unsupported"; reason: string }
  | { type: "upToDate"; current: string }
  | {
      type: "available";
      current: string;
      version: string;
      notes: string | null;
      date: string | null;
    };

export interface ConnProfile {
  id: string;
  name: string;
  /** Identification, not decoration — answers "which server am I on". */
  colour: string;
  host: string;
  port: number;
  user: string;
  database: string | null;
  allowInvalidCerts: boolean;

  /**
   * Which engine this connects to. Optional in the type as it is in the file:
   * a profile written before Stage 11 has none and means MySQL.
   */
  kind?: EngineKind;
  /** Base URL, for engines addressed by one. MySQL uses host and port. */
  url?: string;
  auth?: HttpAuth;
  /** This account authenticates with no secret at all — a MySQL user whose
   *  password is empty. Distinct from "no password is stored", which cannot
   *  tell "there isn't one" from "we don't know it". */
  noPassword?: boolean;
  /** Refuse statements that change data or schema on this connection.
   *
   *  A guard rail, not a boundary: whoever can open the connection can clear
   *  it. Applied in Rust by narrowing the connection's capabilities at
   *  connect, so the whole app keeps asking `capabilities.writes`. */
  readOnly?: boolean;
}

export type EngineKind = "mysql" | "elasticsearch";

/** How to authenticate an HTTP engine. A local cluster usually wants nothing. */
export type HttpAuth =
  | { type: "none" }
  | { type: "basic"; user: string }
  | { type: "apiKey" };

/**
 * A profile as it comes *back* from the backend: the stored fields plus facts
 * derived at runtime.
 *
 * Mirrors Rust's `ProfileView`. The two shapes are kept apart because they had
 * opposite needs and once shared a type: `rememberPassword` had to stay out of
 * the config file and had to reach the UI, and the attribute that achieved the
 * first silently defeated the second. Every remembered password then looked
 * forgotten on the next launch.
 */
export type ProfileView = ConnProfile & {
  /** Derived from the keychain, never persisted — true when a password is stored. */
  rememberPassword: boolean;
  /** True when connecting needs a secret we do not already hold — i.e. when the
   *  UI has to ask. A cluster with no authentication never does; nor does an
   *  account whose password is empty and known to be. Derived in Rust so the
   *  two sides cannot disagree about the rule. */
  needsSecret: boolean;
};

/** Procedure or function. Mirrors Rust's `RoutineKind`. */
export type RoutineKind = "procedure" | "function";

export interface RoutineParam {
  name: string;
  /** "IN" | "OUT" | "INOUT". Functions report no mode; they are all IN. */
  mode: string;
  dataType: string;
}

export interface RoutineRef {
  name: string;
  kind: RoutineKind;
  /** Return type — set for functions, null for procedures. */
  returns: string | null;
  params: RoutineParam[];
}

/**
 * What a generated `DROP` targets.
 *
 * A discriminated union rather than a name plus a kind string: a half-specified
 * target — a column with no table, a routine with no kind — cannot be built.
 */
export type DropTarget =
  | { type: "schema"; db: string }
  | { type: "table"; db: string; name: string }
  | { type: "view"; db: string; name: string }
  | { type: "column"; db: string; table: string; name: string }
  | { type: "routine"; db: string; name: string; kind: RoutineKind };

export interface AppDefaults {
  /** Rows a generated SELECT asks for. */
  browseLimit: number;
  /** The executor's per-statement safety ceiling. A different thing entirely. */
  maxRows: number;
  /** Where the MCP server listens when nobody has chosen. From Rust, so the
   *  number Settings shows is the number that gets bound. */
  mcpPort: number;
}

/**
 * How a CSV should be written.
 *
 * Every field here is a decision the format forces and cannot make for us —
 * most of all `nullAs`, because CSV has no way to tell NULL from an empty
 * string and they are different values.
 */
export interface CsvOptions {
  delimiter: string;
  /** RFC 4180 and Excel want CRLF; Unix tools do not. */
  crlf: boolean;
  /** Without it Excel mangles non-ASCII; with it some Unix tools show a stray BOM. */
  bom: boolean;
  headers: boolean;
  /** What a NULL becomes. Empty by default — and warned about. */
  nullAs: string;
  /**
   * Prefix `'` to fields starting `=`, `+`, `-`, `@` so spreadsheets do not
   * execute them. Off by default: it changes the exported value.
   */
  formulaGuard: boolean;
}

export const defaultCsvOptions = (): CsvOptions => ({
  delimiter: ",",
  crlf: true,
  bom: true,
  headers: true,
  nullAs: "",
  formulaGuard: false,
});

export interface InsertOptions {
  table: string;
  db: string | null;
  createTable: boolean;
  /** Rows per statement. Multi-row inserts replay far faster than one each. */
  batchSize: number;
}

/** The table a result set came from, when it came from exactly one. */
export interface SourceTable {
  connectionId: string;
  db: string;
  table: string;
}

export type ExportFormat = "csv" | "inserts";

/**
 * A result set as the grid holds it.
 *
 * One object rather than three arguments because they are one thing: rows
 * without `truncated` is how a partial export gets presented as a whole one.
 */
export interface ResultSet {
  columns: ColumnMeta[];
  rows: CellValue[][];
  /** The row ceiling had already cut this short before it reached the grid. */
  truncated: boolean;
}

export interface ExportOptions {
  csv: CsvOptions;
  inserts: InsertOptions;
}

export interface ExportOutcome {
  path: string;
  rowsWritten: number;
  /**
   * The rows exported were already cut short by the row ceiling. The file is
   * complete for what was on screen and incomplete for the query — never
   * present it as the latter.
   */
  truncatedSource: boolean;
  bytesWritten: number;
  /** True and unwelcome: a binary column, a NULL CSV cannot express. */
  warnings: string[];
}

/**
 * What one engine supports.
 *
 * Read by the UI so a menu cannot offer what the engine will refuse. Branch on
 * these, **never** on `engine` — that field is for display and for telling the
 * assistant which dialect to write; anything behavioural belongs in a flag, or
 * a fourth engine has to be remembered in every place that guessed from a name.
 */
export interface Capabilities {
  engine: string;
  writes: boolean;
  transactions: boolean;
  multiStatement: boolean;
  delimiterBlocks: boolean;
  routines: boolean;
  cancellation: boolean;
  /** `writes` is false because the *connection* was marked read-only, not
   *  because the engine cannot write. Only the refusal wording depends on the
   *  difference — everything else asks `writes`. */
  readOnly: boolean;
  streamingExport: boolean;
  rowCap: "clientLimit" | "serverPageSize";
  /** What this engine calls the thing `USE` switches between. */
  namespaceLabel: string;
}

export interface ConnInfo {
  id: string;
  serverVersion: string;
  databases: string[];
  currentDatabase: string | null;
  capabilities: Capabilities;
}

export interface ProfileList {
  profiles: ProfileView[];
  /** Set when the config file could not be read and was moved aside. */
  warning: string | null;
}

export interface SaveProfileOutcome {
  profile: ProfileView;
  /** Profile saved but the password did not — show it and fall back to prompting. */
  passwordWarning: string | null;
  passwordStored: boolean;
}

export interface ConnectionStatus {
  id: string;
  name: string;
  colour: string;
  hostLabel: string;
  connected: boolean;
  serverVersion: string | null;
  openTabs: number;
  runningTabs: number;
}

/** Per-tab status. Each tab has its own connection and active database. */
export interface TabStatus {
  connected: boolean;
  running: boolean;
  currentDatabase: string | null;
  connectionId: number;
}

export interface TableRef { name: string; kind: string; }
export interface ColumnInfo { name: string; dataType: string; nullable: boolean; key: string | null; }

export type TypeHint = "numeric" | "text" | "temporal" | "binary" | "bool";
export interface ColumnMeta { name: string; typeHint: TypeHint; sqlType: string; }

/** Untagged on the Rust side: null stays null, numbers stay numbers. */
/**
 * Mirrors Rust's `CellValue`, which is `#[serde(untagged)]`.
 *
 * `BinaryCell` is the odd one: the bytes of a BLOB never cross the IPC boundary
 * — only the fact that they were binary, and how many. It used to arrive as the
 * *string* `"<binary, 12 bytes>"`, which made a real string of those characters
 * indistinguishable from real bytes and forced the SQL export to guess from the
 * column type instead. `bytes` is null when even the length is unknown.
 */
export interface BinaryCell {
  bytes: number | null;
}

export type CellValue = null | number | boolean | string | BinaryCell;

/** Narrow a cell to the binary case. */
export function isBinaryCell(v: CellValue): v is BinaryCell {
  return typeof v === "object" && v !== null && "bytes" in v;
}

/** How a binary cell reads. Mirrors `CellValue::binary_placeholder` in Rust. */
export function binaryPlaceholder(v: BinaryCell): string {
  return v.bytes === null ? "<binary>" : `<binary, ${v.bytes} bytes>`;
}

/**
 * Mirrors Rust's `StatementKind`. `session` covers SET / USE / COMMIT and
 * friends: statements for which a row count was never the point.
 */
export type StatementKind = "select" | "rowReturning" | "modify" | "session" | "other";

export type Outcome =
  | { type: "rows"; columns: ColumnMeta[]; rows: CellValue[][]; truncated: boolean }
  | { type: "affected"; rows: number }
  | { type: "error"; message: string };

export interface StatementResult {
  sql: string;
  effectiveSql: string | null;
  kind: StatementKind;
  outcome: Outcome;
  elapsedMs: number;
}

export interface ScriptResult {
  statements: StatementResult[];
  totalElapsedMs: number;
  abortedAt: number | null;
  delimiterDetected: boolean;
  cancelled: boolean;
  timedOut: boolean;
  /**
   * The tab's connection had died and was replaced before this ran.
   *
   * A reconnect is the right behaviour — a connection the server reaped while
   * you were away should heal, not break. But a new connection is a new
   * session, so temporary tables, session variables and any open transaction
   * are gone, and that is not something to discover from a later error.
   */
  reconnected: boolean;
}

/** One entry per engine we can open files for. The dialog filters and the
 *  editor's syntax mode both derive from this, so adding an engine later means
 *  editing one list in Rust. */
export interface FileTypeSpec {
  label: string;
  extensions: string[];
  dialect: string;
}

// ---------------------------------------------------------------- assistant

/**
 * Which wire format to speak. Two, not five: Ollama, LM Studio, llama.cpp,
 * vLLM, OpenRouter, Groq and Azure all expose the OpenAI Chat Completions
 * shape, so "openAiCompatible plus a base URL" reaches every one of them.
 */
export type Provider = "anthropic" | "openAiCompatible";

/** Where to send questions. Not a secret — the key is separate. */
export interface AssistantConfig {
  provider: Provider;
  baseUrl: string;
  model: string;
}

export interface AssistantStatus {
  /** A key is stored for this provider. It never comes back out. */
  hasKey: boolean;
  /** Usable — a local model needs no key. */
  ready: boolean;
  /** Nothing leaves the machine. Worth saying out loud. */
  local: boolean;
}

export interface ChatMessage {
  role: "user" | "assistant";
  content: string;
}

/** What arrives while a reply is being written. */
export type AssistantEvent =
  | { type: "thinking"; delta: string }
  | { type: "text"; delta: string }
  | { type: "done"; stopReason: string | null }
  | { type: "failed"; message: string };

// ------------------------------------------------------------------ history

/**
 * One statement in the history list — the newest run of it, plus how often it
 * has been run. `sql` is always what the user wrote, never a rewritten form.
 */
export interface HistoryHit {
  /** Unix milliseconds. */
  at: number;
  connectionId: string;
  database: string | null;
  sql: string;
  kind: StatementKind;
  /** "ok" | "error" — a failed statement is often the one worth finding. */
  status: string;
  rows: number | null;
  elapsedMs: number;
  error: string | null;
  /** "user" or "assistant" — where the statement came from. */
  source: string;
  runs: number;
}

// ------------------------------------------------------------------ session

/**
 * One tab as it is written down. Nothing describing a *result* is here: results
 * are unbounded and stale by definition, so a restored tab shows its script and
 * an empty results pane.
 */
export interface StoredTab {
  title: string;
  filePath: string | null;
  dialect: string;
  encoding: string;
  lineEnding: string;
  /** The mtime the baseline came from, so the save-time conflict check
   *  still has something to compare against after a restart. */
  mtimeMs: number | null;
  /** Present only when this is the only copy: an untitled tab, or a
   *  file-backed tab with unsaved edits. */
  text: string | null;
  cursor: number;
  activeDb: string | null;
  untitledNumber: number | null;
  /** Arrived through the MCP server rather than being opened by the user.
   *  Stored, because provenance that lasts only until you quit is provenance
   *  you cannot rely on. Absent in files written before Stage 13. */
  external?: boolean;
}

export interface StoredWorkspace {
  connectionId: string;
  tabs: StoredTab[];
  /** An index, not an id: restored tabs are minted fresh ids. */
  activeIndex: number;
}

export interface SessionStore {
  version: number;
  connections: StoredWorkspace[];
}

export interface McpStatus {
  running: boolean;
  port: number | null;
  /** The full URL a client is configured with, when running. */
  url: string | null;
}

/**
 * What `put_query` sends. Mirrors `mcp::PutQuery` in Rust.
 *
 * The connection id travels with it so a tab that arrives after the user has
 * switched connection can say so, rather than quietly attaching to whichever
 * workspace happens to be in front.
 */
export interface McpPutQuery {
  sql: string;
  connectionId: string;
}

/** Mirrors `mcp::PUT_QUERY_EVENT`. */
export const MCP_PUT_QUERY_EVENT = "mcp://put-query";

export interface SessionLoad {
  session: SessionStore;
  /** Set when the file was unreadable and moved aside. */
  warning: string | null;
}

export interface OpenedFile {
  path: string;
  name: string;
  /** Always LF-normalised; `lineEnding` records what to write back. */
  contents: string;
  sizeBytes: number;
  dialect: string;
  /** "utf-8" | "utf-8-lossy". Lossy means Save must be disabled — writing back
   *  would replace the original bytes with U+FFFD. */
  encoding: string;
  /** "lf" | "crlf" */
  lineEnding: string;
  mtimeMs: number;
  /** Large enough that the UI should warn and ease off linting. */
  large: boolean;
}

export interface SavedFile {
  path: string;
  name: string;
  mtimeMs: number;
}

export type SaveOutcome =
  | { type: "saved"; mtimeMs: number }
  | { type: "conflict"; diskMtimeMs: number };

export type Severity = "error" | "warning" | "info";

/** Byte offsets into the buffer — convert before handing to CodeMirror. */
export interface Diagnostic {
  start: number;
  end: number;
  severity: Severity;
  message: string;
}


export const api = {
  // --- saved profiles. No password ever comes back out of these.
  listProfiles: () => invoke<ProfileList>("list_profiles"),
  /**
   * `password` is a three-way instruction, not just a value:
   *   a string     — remember this password
   *   ""           — forget any stored password
   *   null         — leave whatever is stored alone
   */
  saveProfile: (profile: ConnProfile, password: string | null) =>
    invoke<SaveProfileOutcome>("save_profile", { profile, password }),
  /** Returns a warning if the keychain entry could not be removed. */
  deleteProfile: (id: string) => invoke<string | null>("delete_profile", { id }),
  /**
   * Persist the rail's order. Ids only — the order is the only thing this may
   * change — and a profile the list does not mention keeps its place rather
   * than being dropped.
   */
  reorderProfiles: (ids: string[]) => invoke<void>("reorder_profiles", { ids }),

  // --- connections. Several can be live at once; every call names one.
  listRoutines: (connectionId: string, db: string) =>
    invoke<RoutineRef[]>("list_routines", { connectionId, db }),
  /** The re-runnable creation script for a routine. Text only — never executed. */
  routineDdl: (connectionId: string, db: string, name: string, kind: RoutineKind) =>
    invoke<string>("routine_ddl", { connectionId, db, name, kind }),
  /**
   * The server's own `CREATE` for a table or a view. Text only — never executed.
   *
   * Views go through the same command as tables: `SHOW CREATE TABLE` is what
   * MySQL answers a view with, and Rust unwraps whichever column comes back.
   */
  tableDdl: (connectionId: string, db: string, table: string) =>
    invoke<string>("table_ddl", { connectionId, db, table }),

  // --- the remembered session. Rust owns the file; the shape is ours.
  loadSession: () => invoke<SessionLoad>("load_session"),
  saveSession: (session: SessionStore) => invoke<void>("save_session", { session }),

  // --- the assistant. It has no tools and no connection: it writes SQL into
  // the editor and the user runs it, like every other generated-SQL path here.
  assistantStatus: (provider: Provider, baseUrl: string) =>
    invoke<AssistantStatus>("assistant_status", { provider, baseUrl }),
  /** `null` or "" forgets the key. Keys are stored per provider. */
  assistantSetKey: (provider: Provider, key: string | null) =>
    invoke<boolean>("assistant_set_key", { provider, key }),
  /**
   * Stream one reply. Rejects only if the request never started; anything that
   * goes wrong afterwards arrives as a `failed` event, so a partial answer
   * already on screen is kept.
   */
  assistantSend: (
    config: AssistantConfig,
    connectionId: string | null,
    db: string | null,
    messages: ChatMessage[],
    onEvent: (e: AssistantEvent) => void,
  ) => {
    const channel = new Channel<AssistantEvent>();
    channel.onmessage = onEvent;
    return invoke<void>("assistant_send", {
      provider: config.provider,
      baseUrl: config.baseUrl,
      model: config.model,
      connectionId,
      db,
      messages,
      onEvent: channel,
    });
  },

  /** Record that the assistant proposed this SQL, for history provenance. */
  rememberProposal: (sql: string) => invoke<void>("remember_proposal", { sql }),

  // --- query history. Recorded in Rust at the one point every execution
  // passes through; never run from here, only inserted for the user to run.
  historySearch: (query: string, connectionId: string | null, limit = 200) =>
    invoke<HistoryHit[]>("history_search", { query, connectionId, limit }),
  historyClear: (connectionId: string | null) =>
    invoke<void>("history_clear", { connectionId }),

  /**
   * The licence notices this build ships. Generated per target at package time;
   * a source build gets an error explaining how to generate one.
   */
  thirdPartyLicenses: () => invoke<string>("third_party_licenses"),

  // --- self-update. `updateCheck` is safe to call unattended; `updateInstall`
  // must only ever follow an explicit yes from the user.
  updateCheck: () => invoke<UpdateStatus>("update_check"),
  updateInstall: () => invoke<void>("update_install"),

  // --- generated SQL. Every one of these returns text for the user to read
  // and run themselves; nothing here executes anything.
  appDefaults: () => invoke<AppDefaults>("app_defaults"),
  generateSelect: (connectionId: string, db: string, table: string, limit: number) =>
    invoke<string>("generate_select", { connectionId, db, table, limit }),
  generateDrop: (target: DropTarget) => invoke<string>("generate_drop", { target }),
  generateCall: (connectionId: string, db: string, name: string) =>
    invoke<string>("generate_call", { connectionId, db, name }),

  // --- export. The save dialog and the file write both live in Rust; these
  // return null when the user cancels the dialog.
  exportCsv: (result: ResultSet, options: CsvOptions, suggestedName: string) =>
    invoke<ExportOutcome | null>("export_csv", { result, options, suggestedName }),
  exportInserts: (
    result: ResultSet,
    options: InsertOptions,
    source: SourceTable | null,
    suggestedName: string,
  ) =>
    invoke<ExportOutcome | null>("export_inserts", {
      result, options, source, suggestedName,
    }),
  /** Re-run the statement with no row ceiling and stream every row to a file. */
  exportRerun: (
    tabId: string,
    sql: string,
    format: ExportFormat,
    options: ExportOptions,
    suggestedName: string,
  ) =>
    invoke<ExportOutcome | null>("export_rerun", {
      tabId, sql, format, options, suggestedName,
    }),
  /** Delimited text for the clipboard. The frontend owns the clipboard call. */
  clipboardText: (result: ResultSet, options: CsvOptions) =>
    invoke<string>("clipboard_text", { result, options }),

  connect: (profile: ConnProfile, password: string) =>
    invoke<ConnInfo>("connect", { profile, password }),
  /** Connect using a profile's remembered password; errors if none is stored. */
  connectSaved: (id: string) => invoke<ConnInfo>("connect_saved", { id }),
  disconnect: (connectionId: string) => invoke<void>("disconnect", { connectionId }),
  listConnections: () => invoke<ConnectionStatus[]>("list_connections"),

  // --- tab lifecycle. A tab is bound to one connection for life.
  openTab: (connectionId: string, tabId: string) =>
    invoke<void>("open_tab", { connectionId, tabId }),
  closeTab: (tabId: string) => invoke<void>("close_tab", { tabId }),

  // --- per tab: each has its own connection, active database and cancel
  useDatabase: (tabId: string, db: string) => invoke<void>("use_database", { tabId, db }),
  tabStatus: (tabId: string) => invoke<TabStatus>("tab_status", { tabId }),
  runScript: (tabId: string, sql: string, autoLimit: boolean, timeoutSecs: number | null) =>
    invoke<ScriptResult>("run_script", { tabId, sql, autoLimit, timeoutSecs }),
  cancelQuery: (tabId: string) => invoke<void>("cancel_query", { tabId }),
  lintSql: (tabId: string, sql: string) => invoke<Diagnostic[]>("lint_sql", { tabId, sql }),

  // --- schema cache, per connection, served over that connection's meta link
  listTables: (connectionId: string, db: string) =>
    invoke<TableRef[]>("list_tables", { connectionId, db }),
  listColumns: (connectionId: string, db: string, table: string) =>
    invoke<ColumnInfo[]>("list_columns", { connectionId, db, table }),
  refreshSchema: (connectionId: string, db: string) =>
    invoke<void>("refresh_schema", { connectionId, db }),

  // --- files
  supportedFileTypes: () => invoke<FileTypeSpec[]>("supported_file_types"),
  openFileDialog: () => invoke<OpenedFile | null>("open_file_dialog"),
  readFile: (path: string) => invoke<OpenedFile>("read_file", { path }),
  saveFile: (path: string, contents: string, expectMtime: number | null, lineEnding: string) =>
    invoke<SaveOutcome>("save_file", { path, contents, expectMtime, lineEnding }),
  saveFileDialog: (suggestedName: string, contents: string, lineEnding: string) =>
    invoke<SavedFile | null>("save_file_dialog", { suggestedName, contents, lineEnding }),

  // --- stateless
  statementAtCursor: (sql: string, cursor: number) =>
    invoke<string | null>("statement_at_cursor", { sql, cursor }),
  /** Lay SQL out. Rejects rather than returning something that means
   *  something else, so a failure is worth showing. */
  formatSql: (sql: string) => invoke<string>("format_sql", { sql }),

  // --- the MCP server. Off unless started; see src-tauri/src/mcp.rs for why a
  // listening socket is acceptable in an app where nothing runs unattended.
  mcpStart: (port: number) => invoke<McpStatus>("mcp_start", { port }),
  mcpStop: () => invoke<McpStatus>("mcp_stop"),
  mcpStatus: () => invoke<McpStatus>("mcp_status"),
  /** Which connection an MCP client sees. Only the UI knows this. */
  mcpSetFocus: (connectionId: string | null) =>
    invoke<void>("mcp_set_focus", { connectionId }),
  /** Minted on first read and kept in the OS keychain. */
  mcpToken: () => invoke<string>("mcp_token"),
  mcpRegenerateToken: () => invoke<string>("mcp_regenerate_token"),
};
