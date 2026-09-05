// Typed mirror of the Rust command surface. Keep in sync with src-tauri/src.
import { invoke } from "@tauri-apps/api/core";

export interface ConnConfig {
  host: string;
  port: number;
  user: string;
  password: string;
  database: string | null;
  allowInvalidCerts: boolean;
}

export interface ConnInfo {
  serverVersion: string;
  databases: string[];
  currentDatabase: string | null;
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
  // --- server-wide
  connect: (config: ConnConfig) => invoke<ConnInfo>("connect", { config }),
  disconnect: () => invoke<void>("disconnect"),

  // --- tab lifecycle
  openTab: (tabId: string) => invoke<void>("open_tab", { tabId }),
  closeTab: (tabId: string) => invoke<void>("close_tab", { tabId }),

  // --- per tab: each has its own connection, active database and cancel
  useDatabase: (tabId: string, db: string) => invoke<void>("use_database", { tabId, db }),
  tabStatus: (tabId: string) => invoke<TabStatus>("tab_status", { tabId }),
  runScript: (tabId: string, sql: string, autoLimit: boolean, timeoutSecs: number | null) =>
    invoke<ScriptResult>("run_script", { tabId, sql, autoLimit, timeoutSecs }),
  cancelQuery: (tabId: string) => invoke<void>("cancel_query", { tabId }),
  lintSql: (tabId: string, sql: string) => invoke<Diagnostic[]>("lint_sql", { tabId, sql }),

  // --- shared schema cache, served over the meta connection
  listTables: (db: string) => invoke<TableRef[]>("list_tables", { db }),
  listColumns: (db: string, table: string) => invoke<ColumnInfo[]>("list_columns", { db, table }),
  refreshSchema: (db: string) => invoke<void>("refresh_schema", { db }),

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
