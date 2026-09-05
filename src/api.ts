// Typed mirror of the Rust command surface. Keep in sync with src-tauri/src.
import { invoke } from "@tauri-apps/api/core";

/**
 * Everything about a connection except its secret. **This is exactly what is
 * written to the config file.**
 *
 * Deliberately holds no password, so keeping it secret-free by construction
 * means the config file cannot leak one even by accident. It also holds nothing
 * derived — see `ProfileView` for why that separation matters.
 */
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
}

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
};

export interface ConnInfo {
  id: string;
  serverVersion: string;
  databases: string[];
  currentDatabase: string | null;
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
export type CellValue = null | number | boolean | string;

export type StatementKind = "select" | "rowReturning" | "modify" | "other";

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
}

/** One entry per engine we can open files for. The dialog filters and the
 *  editor's syntax mode both derive from this, so adding an engine later means
 *  editing one list in Rust. */
export interface FileTypeSpec {
  label: string;
  extensions: string[];
  dialect: string;
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

export interface StatementSpan { start: number; end: number; }
export interface SplitOutput { statements: StatementSpan[]; delimiterDetected: boolean; }

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
  hasStoredPassword: (id: string) => invoke<boolean>("has_stored_password", { id }),

  // --- connections. Several can be live at once; every call names one.
  connect: (profile: ConnProfile, password: string) =>
    invoke<ConnInfo>("connect", { profile, password }),
  /** Connect using a profile's remembered password; errors if none is stored. */
  connectSaved: (id: string) => invoke<ConnInfo>("connect_saved", { id }),
  disconnect: (connectionId: string) => invoke<void>("disconnect", { connectionId }),
  disconnectAll: () => invoke<void>("disconnect_all"),
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
  splitSql: (sql: string) => invoke<SplitOutput>("split_sql", { sql }),
  statementAtCursor: (sql: string, cursor: number) =>
    invoke<string | null>("statement_at_cursor", { sql, cursor }),
};
