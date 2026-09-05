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
  connectionId: number;
}

export interface StatusInfo {
  connected: boolean;
  hostLabel: string | null;
  currentDatabase: string | null;
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
  connect: (config: ConnConfig) => invoke<ConnInfo>("connect", { config }),
  disconnect: () => invoke<void>("disconnect"),
  useDatabase: (db: string) => invoke<void>("use_database", { db }),
  status: () => invoke<StatusInfo>("status"),
  listTables: (db: string) => invoke<TableRef[]>("list_tables", { db }),
  listColumns: (db: string, table: string) => invoke<ColumnInfo[]>("list_columns", { db, table }),
  refreshSchema: (db: string) => invoke<void>("refresh_schema", { db }),
  splitSql: (sql: string) => invoke<SplitOutput>("split_sql", { sql }),
  statementAtCursor: (sql: string, cursor: number) =>
    invoke<string | null>("statement_at_cursor", { sql, cursor }),
  runScript: (sql: string, autoLimit: boolean, timeoutSecs: number | null) =>
    invoke<ScriptResult>("run_script", { sql, autoLimit, timeoutSecs }),
  lintSql: (sql: string) => invoke<Diagnostic[]>("lint_sql", { sql }),
  cancelQuery: () => invoke<void>("cancel_query"),
};
