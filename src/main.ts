import type { EditorView } from "@codemirror/view";
import type { SQLNamespace } from "@codemirror/lang-sql";

import {
  api,
  defaultCsvOptions,
  type ConnProfile,
  type TableRef,
  type RoutineRef,
  type ColumnInfo,
  type CsvOptions,
  type InsertOptions,
  type ExportFormat,
  type ExportOutcome,
  type SourceTable,
  type UpdateStatus,
} from "./api";
import { copyText } from "./clipboard";
import { choose } from "./dialog";
import { createTheme, type ThemePref } from "./theme";
import { FONTS, applyAppearance, load as loadSettings, save as saveSettings } from "./settings";
import { contextMenu } from "./menu";
import { ResultView } from "./grid";
import { TabManager, type ScriptTab } from "./tabs";
import {
  ConnectionManager, COLOURS, newConnectionId, type ConnectionEntry,
} from "./connections";
import { createFileUx, type FileUx } from "./files";
import { createSessionPersistence } from "./session";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  createEditor, cursorByteOffset, docText, insertAtCursor, refreshLint, selectedText,
  setEditorTheme, setLinting, setSchema,
} from "./editor";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const els = {
  sidebar: $("sidebar"), main: $("main"), tree: $("tree"),
  connLabel: $<HTMLSpanElement>("conn-label"),
  connDot: $<HTMLSpanElement>("conn-dot"),
  btnConnect: $<HTMLButtonElement>("btn-connect"),
  btnRun: $<HTMLButtonElement>("btn-run"),
  btnRunAll: $<HTMLButtonElement>("btn-run-all"),
  btnCancel: $<HTMLButtonElement>("btn-cancel"),
  autoLimit: $<HTMLInputElement>("chk-autolimit"),
  lintOn: $<HTMLInputElement>("chk-lint"),
  timeout: $<HTMLInputElement>("num-timeout"),
  fileNote: $<HTMLSpanElement>("file-note"),
  rail: $("rail"),
  connTitle: $<HTMLHeadingElement>("conn-title"),
  connColours: $("conn-colours"),
  connSave: $<HTMLInputElement>("conn-save"),
  connRemember: $<HTMLInputElement>("conn-remember"),
  resultNote: $<HTMLElement>("result-note"),
  btnCopy: $<HTMLButtonElement>("btn-copy"),
  btnCopyHead: $<HTMLButtonElement>("btn-copy-head"),
  btnExport: $<HTMLButtonElement>("btn-export"),
  btnUpdate: $<HTMLButtonElement>("btn-update"),
  btnSettings: $<HTMLButtonElement>("btn-settings"),
  settingsDialog: $<HTMLDialogElement>("settings-dialog"),
  setTheme: $<HTMLSelectElement>("set-theme"),
  setFont: $<HTMLSelectElement>("set-font"),
  setFontSize: $<HTMLInputElement>("set-font-size"),
  setAutoLimit: $<HTMLInputElement>("set-autolimit"),
  setLint: $<HTMLInputElement>("set-lint"),
  setTimeout: $<HTMLInputElement>("set-timeout"),
  setBrowse: $<HTMLInputElement>("set-browse"),
  setVersion: $<HTMLElement>("set-version"),
  setCheckUpdate: $<HTMLButtonElement>("set-check-update"),
  setUpdateNote: $<HTMLElement>("set-update-note"),
  setClose: $<HTMLButtonElement>("set-close"),
  exportDialog: $<HTMLDialogElement>("export-dialog"),
  exportForm: $<HTMLFormElement>("export-form"),
  exportFormat: $<HTMLSelectElement>("export-format"),
  exportScope: $<HTMLSelectElement>("export-scope"),
  exportScopeNote: $<HTMLParagraphElement>("export-scope-note"),
  exportCsvOpts: $<HTMLFieldSetElement>("export-csv-opts"),
  exportSqlOpts: $<HTMLFieldSetElement>("export-sql-opts"),
  exportError: $<HTMLParagraphElement>("export-error"),
  exportCancel: $<HTMLButtonElement>("export-cancel"),
  exportOk: $<HTMLButtonElement>("export-ok"),
  csvDelim: $<HTMLInputElement>("csv-delim"),
  csvHeaders: $<HTMLInputElement>("csv-headers"),
  csvNull: $<HTMLInputElement>("csv-null"),
  csvCrlf: $<HTMLInputElement>("csv-crlf"),
  csvBom: $<HTMLInputElement>("csv-bom"),
  csvGuard: $<HTMLInputElement>("csv-guard"),
  sqlTable: $<HTMLInputElement>("sql-table"),
  sqlCreate: $<HTMLInputElement>("sql-create"),
  sqlCreateNote: $<HTMLParagraphElement>("sql-create-note"),
  sqlBatch: $<HTMLInputElement>("sql-batch"),
  connRememberRow: $<HTMLLabelElement>("conn-remember-row"),
  dialog: $<HTMLDialogElement>("conn-dialog"),
  form: $<HTMLFormElement>("conn-form"),
  connError: $<HTMLParagraphElement>("conn-error"),
  connCancel: $<HTMLButtonElement>("conn-cancel"),
  vsplit: $("vsplit"), hsplit: $("hsplit"),
};

// `refreshExportBar` is a hoisted function declaration, so it can be handed
// over here even though it is defined further down.
const results = new ResultView(
  $("tabs"),
  $("grid"),
  $("status"),
  () => refreshExportBar(),
  // Passed to the constructor, not through `show()`: `setMessage` resets the
  // per-result hooks, and a copy that silently stopped working after a message
  // is exactly the class of bug this project has already shipped once.
  (headers) => void copySelection(headers),
);

let view!: EditorView;
let tabs!: TabManager;
let files!: FileUx;

let conns!: ConnectionManager;

/**
 * The open tabs, remembered across restarts. Restore happens per connection, on
 * connect — nothing connects at boot, so there is nowhere for a tab to be until
 * then.
 */
const session = createSessionPersistence({
  tabs: () => tabs,
  notify: (m) => results.setMessage(m),
});

/** Each connection keeps its own schema tree element, so expansion survives. */
const trees = new Map<string, HTMLElement>();

/** The tab every command in this module acts on. */
const activeTab = (): ScriptTab => {
  const t = tabs.active();
  if (!t) throw new Error("no active tab");
  return t;
};

let connected = false;
let activeDb: string | null = null;

/**
 * Rows a generated `SELECT` asks for.
 *
 * Seeded from the backend at boot rather than written down here as well — it is
 * a different number from the executor's safety ceiling, and two places that
 * both "know" it is how they drift apart.
 */
let browseLimit = 1000;
/** Accumulated schema for CodeMirror autocomplete, grown as the tree loads. */
const schemaMap: SQLNamespace = {};

// ---------------------------------------------------------------- utilities

/**
 * Rust reports diagnostics as BYTE offsets; CodeMirror positions are UTF-16
 * code-unit indices. They agree only while the buffer is pure ASCII, so the
 * conversion is mandatory — one accented character otherwise slides every
 * squiggle to the wrong place.
 */
function makeByteToChar(text: string): (byteOffset: number) => number {
  const bytes = new TextEncoder().encode(text);
  if (bytes.length === text.length) {
    return (b) => Math.min(Math.max(b, 0), text.length); // ASCII fast path
  }
  const dec = new TextDecoder();
  return (b) => dec.decode(bytes.slice(0, Math.min(Math.max(b, 0), bytes.length))).length;
}

/**
 * Linting budget, chosen from measurements (Stage 1 Phase 7), not guesswork.
 *
 * A lint pass costs roughly `IPC(serialise the whole doc) + Rust(scan it)`:
 *
 *     64 KB → ~3 ms      256 KB → ~9 ms
 *      1 MB → ~29 ms       5 MB → ~158 ms
 *
 * At 5 MB with a 300 ms debounce that is over half a core spent re-linting
 * while you type. So: full linting up to the cap, with the debounce widening
 * as the buffer grows, and off entirely beyond it.
 */
const LINT_MAX_BYTES = 2 * 1024 * 1024;

/**
 * Debounce for the current buffer size, quantised to 100 ms so it only
 * reconfigures every ~100 KB typed rather than on every keystroke.
 * `null` means "do not lint this buffer at all".
 */
function lintDelayFor(docLength: number): number | null {
  if (docLength > LINT_MAX_BYTES) return null;
  const raw = 300 + docLength / 1024;
  return Math.min(1500, Math.round(raw / 100) * 100);
}

/** Advisory diagnostics. Never gates Run — see the tracker's Phase 5b rule. */
async function lintSource(v: EditorView) {
  if (!connected) return [];
  // Second guard: the compartment may not have been reconfigured yet if the
  // buffer just grew past the cap.
  if (lintDelayFor(v.state.doc.length) === null) return [];
  const text = v.state.doc.toString();
  let diags;
  try {
    diags = await api.lintSql(activeTab().id, text);
  } catch {
    // A linter failure must never surface as an error the user has to dismiss.
    return [];
  }
  // The `catch` above only covers a rejection. A response that *resolves* to
  // something other than a list would reach `.map` below and throw an uncaught
  // page error — which is exactly the outcome the comment above rules out.
  if (!Array.isArray(diags)) return [];
  const toChar = makeByteToChar(text);
  const len = v.state.doc.length;
  return diags.map((d) => {
    const from = Math.min(toChar(d.start), len);
    return {
      from,
      to: Math.min(Math.max(toChar(d.end), from + 1), len),
      severity: d.severity,
      message: d.message,
    };
  });
}

/** Delay currently configured, so we only reconfigure when the bucket changes. */
let appliedLintDelay: number | null | undefined;

function applyLintSetting(force = false) {
  // doc.length counts UTF-16 units, not bytes. Close enough for a size bucket,
  // and far cheaper than encoding the whole buffer on every keystroke.
  const delay = els.lintOn.checked ? lintDelayFor(view.state.doc.length) : null;
  if (!force && delay === appliedLintDelay) return;
  appliedLintDelay = delay;
  setLinting(view, delay === null ? null : lintSource, delay ?? 300);
  refreshFileNote();
}

/** Reflect the ACTIVE tab's busy state. Other tabs keep running regardless. */
/**
 * Per-tab file indicator. Its whole job is to make "you cannot save this"
 * visible BEFORE the user tries, rather than only in the dialog afterwards.
 */
function refreshFileNote() {
  // Saving does not change the document, so `onDocChanged` never fires for it —
  // but the path, the title and the mtime all just moved.
  session.schedule();
  const tab = tabs?.active();
  if (!tab) {
    els.fileNote.hidden = true;
    return;
  }
  if (tab.encoding === "utf-8-lossy") {
    els.fileNote.hidden = false;
    els.fileNote.className = "chip warn";
    els.fileNote.textContent = "invalid UTF-8 · Save disabled";
    els.fileNote.title =
      "This file was not valid UTF-8, so it opened with replacement characters. " +
      "Saving over it would discard the original bytes. Use Save As to write a copy.";
    return;
  }
  if (els.lintOn.checked && lintDelayFor(view.state.doc.length) === null) {
    els.fileNote.hidden = false;
    els.fileNote.className = "chip warn";
    els.fileNote.textContent = "large buffer · linting off";
    els.fileNote.title =
      `Buffers over ${LINT_MAX_BYTES / 1048576} MB are not linted: a pass would cost ` +
      "more than the debounce window and make typing sluggish. Running is unaffected.";
    return;
  }
  if (tab.lineEnding === "crlf") {
    els.fileNote.hidden = false;
    els.fileNote.className = "chip";
    els.fileNote.textContent = "CRLF";
    els.fileNote.title = "Line endings are preserved on save.";
    return;
  }
  els.fileNote.hidden = true;
}

function syncBusy() {
  const tab = tabs?.active();
  const busy = tab?.busy ?? false;
  els.btnRun.disabled = busy || !connected;
  els.btnRunAll.disabled = busy || !connected;
  els.btnCancel.hidden = !busy;
  syncConnLabel();
}

/**
 * Connection label doubles as the cross-tab indicator: how many tabs are
 * running right now, and how many connections we hold.
 *
 * The count matters because tabs are not free on the server — the model is
 * one connection per tab that has run, plus a shared killer and meta.
 */
function syncConnLabel() {
  const active = conns?.active();
  // The button has always disconnected when there was something to disconnect;
  // it just never said so. A control that does the opposite of its label is
  // worse than one that is missing.
  const isLive = !!active?.connected;
  els.btnConnect.textContent = isLive ? "Disconnect" : "Connect";
  els.btnConnect.title = isLive
    ? `Disconnect from ${active.profile.name}`
    : "Open a connection";
  els.btnConnect.classList.toggle("danger", isLive);

  if (!active) {
    els.connLabel.textContent = "No connection";
    els.connLabel.title = "";
    els.connDot.hidden = true;
    return;
  }

  // The colour dot is the cheap half of environment safety: it answers "which
  // server am I on" without reading anything.
  els.connDot.hidden = false;
  els.connDot.style.background = active.profile.colour;

  if (!active.connected) {
    els.connLabel.textContent = `${active.profile.name} — not connected`;
    els.connLabel.title = `${active.profile.user}@${active.profile.host}:${active.profile.port}`;
    return;
  }

  const mine = tabs?.forConnection(active.profile.id) ?? [];
  const running = mine.filter((t) => t.busy).length;
  const db = tabs?.active()?.activeDb ?? null;

  const parts = [active.profile.name];
  if (db) parts.push(db);
  if (running > 0) parts.push(`${running} running`);
  els.connLabel.textContent = parts.join(" · ");

  // The arithmetic across every live connection, because tabs are not free on
  // the server and the total is what a DBA would ask about.
  const live = conns.all().filter((c) => c.connected);
  const perConn = live.map((c) => {
    const t = tabs?.forConnection(c.profile.id) ?? [];
    return { name: c.profile.name, tabs: t.filter((x) => x.serverConnId > 0).length };
  });
  const total = perConn.reduce((n, c) => n + c.tabs + 2, 0);
  els.connLabel.title =
    `${active.profile.user}@${active.profile.host}:${active.profile.port}\n` +
    `MySQL ${active.serverVersion ?? "?"}\n\n` +
    perConn.map((c) => `${c.name}: ${c.tabs} tab + 2 shared`).join("\n") +
    `\n= ${total} server connection${total === 1 ? "" : "s"} in total.\n` +
    "Tab connections open on a tab's first query and close with the tab.";
}

function setBusy(tab: ScriptTab, busy: boolean) {
  tab.busy = busy;
  tabs.render();
  syncBusy();
}

/** True when `tab` is the one currently on screen. */
function isFocused(tab: ScriptTab): boolean {
  return tabs.active()?.id === tab.id;
}


// --------------------------------------------------------------- connections

/** The connection the editor dialog is currently editing, if any. */
let editing: ConnectionEntry | null = null;
let chosenColour = COLOURS[0];

function renderColourSwatches() {
  els.connColours.replaceChildren(
    ...COLOURS.map((c) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "swatch" + (c === chosenColour ? " selected" : "");
      b.style.background = c;
      b.title = c;
      b.onclick = () => {
        chosenColour = c;
        renderColourSwatches();
      };
      return b;
    }),
  );
}

/**
 * Open the connection editor.
 *
 * With an entry: editing it, or supplying a password it has not remembered.
 * Without: a new connection, saved or one-off depending on the checkbox.
 */
function openConnectionEditor(existing?: ConnectionEntry) {
  editing = existing ?? null;
  const p = existing?.profile;
  els.connError.hidden = true;
  els.connTitle.textContent = existing ? `Edit ${p!.name}` : "New connection";
  chosenColour = p?.colour ?? COLOURS[0];
  renderColourSwatches();

  const f = els.form;
  const set = (name: string, value: string) => {
    (f.elements.namedItem(name) as HTMLInputElement).value = value;
  };
  set("name", p?.name ?? "");
  set("host", p?.host ?? "127.0.0.1");
  set("port", String(p?.port ?? 3306));
  set("user", p?.user ?? "root");
  set("password", "");
  set("database", p?.database ?? "");
  (f.elements.namedItem("allowInvalidCerts") as HTMLInputElement).checked =
    p?.allowInvalidCerts ?? false;
  els.connSave.checked = existing ? existing.saved : true;
  els.connRemember.checked = p?.rememberPassword ?? false;
  // A password can only be remembered against something that is saved.
  els.connRememberRow.hidden = !els.connSave.checked;

  const pw = f.elements.namedItem("password") as HTMLInputElement;
  pw.placeholder = p?.rememberPassword ? "(unchanged — stored in the keychain)" : "";

  els.dialog.showModal();
}

els.connSave.onchange = () => {
  els.connRememberRow.hidden = !els.connSave.checked;
  if (!els.connSave.checked) els.connRemember.checked = false;
};

els.btnConnect.onclick = () => {
  const active = conns.active();
  if (active?.connected) void conns.disconnect(active);
  else openConnectionEditor(active ?? undefined);
};

els.connCancel.onclick = () => els.dialog.close();

els.form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const fd = new FormData(els.form);
  const host = String(fd.get("host") ?? "").trim();
  const wantSave = els.connSave.checked;

  const profile: ConnProfile = {
    id: editing?.profile.id ?? newConnectionId(),
    name: String(fd.get("name") ?? "").trim() || host,
    colour: chosenColour,
    host,
    port: Number(fd.get("port") ?? 3306),
    user: String(fd.get("user") ?? "").trim(),
    database: String(fd.get("database") ?? "").trim() || null,
    allowInvalidCerts: fd.get("allowInvalidCerts") === "on",
  };
  // Kept out of the profile on purpose — see ConnProfile's doc comment.
  const typed = String(fd.get("password") ?? "");

  const ok = $<HTMLButtonElement>("conn-ok");
  ok.disabled = true;
  ok.textContent = "Connecting…";
  try {
    const entry: ConnectionEntry = {
      // The best answer available before saving: what the keychain said when
      // this profile was loaded. Replaced below with what the save actually
      // achieved, which is the only claim worth keeping.
      profile: { ...profile, rememberPassword: editing?.profile.rememberPassword ?? false },
      saved: wantSave,
      connected: false,
      serverVersion: null,
      databases: [],
    };

    // Connect FIRST, and add nothing anywhere until it succeeds. A failed
    // attempt used to leave a dead icon in the rail for every typo, and put its
    // error in the results pane rather than next to the fields being corrected.
    //
    // An empty password box on a profile that already remembers one means "use
    // the stored one", not "connect with a blank password".
    const useStored = !typed && (editing?.profile.rememberPassword ?? false);
    const res = useStored
      ? await conns.connectStored(entry)
      : await conns.connectWith(entry, typed);

    if (!res.ok) {
      els.connError.textContent = res.error ?? "Could not connect.";
      els.connError.hidden = false;
      return;
    }

    // Only a connection that actually works is worth writing down — and this
    // way the password is validated before it reaches the keychain.
    if (wantSave) {
      // Three-way: a string remembers it, "" forgets it, null leaves it alone.
      // Leaving it alone is what lets someone edit a port without retyping.
      const password = els.connRemember.checked
        ? typed || null
        : editing?.profile.rememberPassword
          ? ""
          : null;
      const outcome = await api.saveProfile(profile, password);
      entry.profile.rememberPassword = outcome.passwordStored;
      entry.saved = true;
      conns.upsert(entry);
      // Editing the active connection's colour must repaint the tab strip;
      // setActiveConnection early-returns when the id has not changed.
      tabs.render();
      if (outcome.passwordWarning) results.setMessage(outcome.passwordWarning);
    }

    els.dialog.close();
  } catch (err) {
    // Saving failures are shown here too; the dialog keeps its values so the
    // user can correct something rather than retyping everything.
    els.connError.textContent = String(err);
    els.connError.hidden = false;
  } finally {
    ok.disabled = false;
    ok.textContent = "Connect";
  }
});

// -------------------------------------------------------------- schema tree

function node(cls: string, label: string, twisty: string, icon: string) {
  const n = document.createElement("div");
  n.className = `node ${cls}`;
  const t = document.createElement("span");
  t.className = "twisty";
  t.textContent = twisty;
  const ic = document.createElement("span");
  ic.className = "icon";
  ic.textContent = icon;
  const lb = document.createElement("span");
  lb.className = "label";
  lb.textContent = label;
  n.append(t, ic, lb);
  return { n, twisty: t, label: lb };
}

/**
 * Single click and double click on the same element, without them fighting.
 *
 * The single action is deferred until the double-click window has passed and
 * cancelled if a second click arrives. Nothing here is instant anyway — expanding
 * a table fetches its columns — so the delay is invisible, whereas expanding on
 * the first click of a double-click makes the tree jump under the cursor.
 */
function clickOrDouble(el: HTMLElement, single: () => void, double: () => void) {
  let timer: number | undefined;
  el.onclick = (e) => {
    e.stopPropagation();
    window.clearTimeout(timer);
    timer = window.setTimeout(single, 220);
  };
  el.ondblclick = (e) => {
    e.stopPropagation();
    e.preventDefault();
    window.clearTimeout(timer);
    double();
  };
}

/**
 * Put generated SQL in front of the user.
 *
 * A **new tab**, always — generating into the script someone is working on
 * would edit their work to show them something. The one exception is the
 * routine "append", which is what was asked for and what a CALL is usually for.
 *
 * Nothing is ever run. That is the rule this whole stage obeys.
 */
async function showGenerated(
  title: string,
  produce: () => Promise<string>,
  sourceTable?: SourceTable,
) {
  try {
    tabs.create({ contents: await produce(), title, sourceTable: sourceTable ?? null });
    view.focus();
  } catch (err) {
    results.setMessage(String(err));
  }
}

/** Append generated SQL to the end of the current tab, leaving the cursor after it. */
async function appendGenerated(produce: () => Promise<string>) {
  try {
    const text = await produce();
    const end = view.state.doc.length;
    const needsBlankLine = end > 0 && !view.state.sliceDoc(Math.max(0, end - 2), end).endsWith("\n\n");
    const insert = (end > 0 ? (needsBlankLine ? "\n\n" : "") : "") + text;
    view.dispatch({
      changes: { from: end, insert },
      selection: { anchor: end + insert.length },
      scrollIntoView: true,
    });
    view.focus();
  } catch (err) {
    results.setMessage(String(err));
  }
}

/**
 * Build (or reveal) the schema tree for one connection.
 *
 * Each connection keeps its own tree element, so switching away and back does
 * not collapse everything the user had expanded.
 */
function renderDatabases(connId: string, dbs: string[]) {
  const host = document.createElement("div");
  host.replaceChildren(...dbs.map((db) => buildDbNode(connId, db)));
  trees.set(connId, host);
  showTree(connId);
}

function showTree(connId: string) {
  const host = trees.get(connId);
  els.tree.replaceChildren(...(host ? [host] : []));
}

function buildDbNode(connId: string, db: string): HTMLElement {
  const wrap = document.createElement("div");
  const { n, twisty } = node("db", db, "▸", "🗄");
  const children = document.createElement("div");
  children.className = "children";
  children.hidden = true;

  const refresh = document.createElement("span");
  refresh.className = "meta";
  refresh.textContent = "⟳";
  refresh.title = "Refresh this database";
  refresh.onclick = async (e) => {
    e.stopPropagation();
    await api.refreshSchema(connId, db);
    children.replaceChildren();
    children.dataset.loaded = "";
    if (!children.hidden) await loadDbChildren(connId, db, children, n);
  };
  n.append(refresh);

  n.onclick = async () => {
    // Clicking a database both expands it and makes it the active schema, so
    // unqualified table names in the editor resolve against it.
    // The spinner covers the whole gesture, not just the child fetch: clicking
    // a database is *two* round trips — USE first, then the listing — and the
    // first one is invisible work the user is still waiting through.
    n.classList.add("loading");
    try {
    const tab = activeTab();
    if (tab.activeDb !== db) {
      try {
        await api.useDatabase(tab.id, db);
        tab.activeDb = db;
        activeDb = db;
        els.tree.querySelectorAll(".node.db-active").forEach((x) => x.classList.remove("db-active"));
        n.classList.add("db-active");
        activeDb = db;
        syncConnLabel();
      } catch (err) {
        results.setMessage(String(err));
      }
    }
    children.hidden = !children.hidden;
    twisty.textContent = children.hidden ? "▸" : "▾";
    if (!children.hidden && !children.dataset.loaded) {
      await loadDbChildren(connId, db, children, n);
    }
    } finally {
      n.classList.remove("loading");
    }
  };

  wrap.append(n, children);
  return wrap;
}

/**
 * A database's children, grouped by kind.
 *
 * Tables and views were previously one flat list distinguished only by icon,
 * and routines were not shown at all. Grouping makes the separation structural
 * rather than a matter of noticing an emoji — and it is where procedures and
 * functions can live without being mistaken for tables.
 *
 * Tables open automatically: it is the common case and should not cost a click.
 */
async function loadDbChildren(
  connId: string,
  db: string,
  host: HTMLElement,
  dbNode: HTMLElement,
) {
  dbNode.classList.add("loading");
  try {
    const [tables, routines] = await Promise.all([
      api.listTables(connId, db),
      // A server that will not report routines (no privilege, an old version)
      // must not stop the tables from appearing.
      api.listRoutines(connId, db).catch(() => [] as RoutineRef[]),
    ]);

    const baseTables = tables.filter((t) => !t.kind.toUpperCase().includes("VIEW"));
    const views = tables.filter((t) => t.kind.toUpperCase().includes("VIEW"));
    const procs = routines.filter((r) => r.kind === "procedure");
    const funcs = routines.filter((r) => r.kind === "function");

    host.replaceChildren(
      buildGroup("Tables", "\u25a6", baseTables.length, () =>
        baseTables.map((t) => buildTableNode(connId, db, t)),
      { open: true }),
      buildGroup("Views", "\u{1f441}", views.length, () =>
        views.map((t) => buildTableNode(connId, db, t))),
      buildGroup("Procedures", "\u2699", procs.length, () =>
        procs.map((r) => buildRoutineNode(connId, db, r))),
      buildGroup("Functions", "\u0192", funcs.length, () =>
        funcs.map((r) => buildRoutineNode(connId, db, r))),
    );
    host.dataset.loaded = "1";

    // Seed autocomplete with table names immediately; columns fill in lazily.
    for (const t of tables) {
      if (!(t.name in schemaMap)) (schemaMap as Record<string, string[]>)[t.name] = [];
    }
    setSchema(view, schemaMap);
    refreshLint(view);
  } catch (err) {
    host.replaceChildren(Object.assign(document.createElement("div"), {
      className: "empty", textContent: String(err),
    }));
  } finally {
    dbNode.classList.remove("loading");
  }
}

/** A collapsible "Tables (12)" style grouping. Empty groups are not rendered. */
function buildGroup(
  label: string,
  icon: string,
  count: number,
  build: () => HTMLElement[],
  opts?: { open?: boolean },
): HTMLElement {
  const wrap = document.createElement("div");
  if (count === 0) return wrap;

  const { n, twisty } = node("group", `${label} (${count})`, "\u25b8", icon);
  const children = document.createElement("div");
  children.className = "children";
  children.hidden = true;

  const toggle = () => {
    children.hidden = !children.hidden;
    twisty.textContent = children.hidden ? "\u25b8" : "\u25be";
    if (!children.hidden && !children.dataset.loaded) {
      children.replaceChildren(...build());
      children.dataset.loaded = "1";
    }
  };
  n.onclick = (e) => {
    e.stopPropagation();
    toggle();
  };

  wrap.append(n, children);
  if (opts?.open) toggle();
  return wrap;
}

function buildTableNode(connId: string, db: string, t: TableRef): HTMLElement {
  const wrap = document.createElement("div");
  const isView = t.kind.toUpperCase().includes("VIEW");
  const { n, twisty, label } = node("table", t.name, "\u25b8", isView ? "\u{1f441}" : "\u25a6");
  const children = document.createElement("div");
  children.className = "children";
  children.hidden = true;

  const expand = async () => {
    children.hidden = !children.hidden;
    twisty.textContent = children.hidden ? "\u25b8" : "\u25be";
    if (!children.hidden && !children.dataset.loaded) {
      n.classList.add("loading");
      try {
        const cols = await api.listColumns(connId, db, t.name);
        children.replaceChildren(...cols.map((c) => buildColumnNode(db, t.name, c)));
        children.dataset.loaded = "1";
        (schemaMap as Record<string, string[]>)[t.name] = cols.map((c) => c.name);
        setSchema(view, schemaMap);
        refreshLint(view);
      } catch (err) {
        children.replaceChildren(Object.assign(document.createElement("div"), {
          className: "empty", textContent: String(err),
        }));
      } finally {
        n.classList.remove("loading");
      }
    }
  };

  const browse = () =>
    void showGenerated(
      t.name,
      () => api.generateSelect(db, t.name, browseLimit),
      // The one place the source table is known for certain, which is what lets
      // an exported CREATE TABLE be the server's own rather than a guess.
      { connectionId: connId, db, table: t.name },
    );

  // Single click expands, double click browses. The label used to insert the
  // table name on a plain click, which cannot coexist with a double-click
  // action — that action now lives in the context menu, where it is also
  // easier to find than "click the text but not the row".
  clickOrDouble(n, () => void expand(), browse);
  clickOrDouble(label, () => void expand(), browse);

  n.oncontextmenu = (e) =>
    contextMenu(e, [
      { label: `${isView ? "View" : "Table"} ${t.name}`, run: () => {}, heading: true },
      { label: `Select first ${browseLimit} rows`, run: browse },
      { label: "Insert name at cursor", run: () => insertAtCursor(view, t.name) },
      {
        // A view's definition is the only place its query lives, so this is
        // what "what does this actually select?" costs. Tables get the same
        // entry: it is the same command, and `SHOW CREATE TABLE` is the exact
        // answer to keys, defaults and collation that the tree cannot show.
        label: "Examine definition\u2026",
        run: () => void showGenerated(t.name, () => api.tableDdl(connId, db, t.name)),
      },
      {
        label: isView ? "Drop view…" : "Drop table…",
        danger: true,
        run: () =>
          void showGenerated(`drop ${t.name}`, () =>
            api.generateDrop({ type: isView ? "view" : "table", db, name: t.name })),
      },
    ]);

  wrap.append(n, children);
  return wrap;
}

function buildColumnNode(
  db: string,
  table: string,
  c: ColumnInfo,
): HTMLElement {
  const { n: cn } = node("column", c.name, "", c.key === "PRI" ? "\u{1f511}" : "\u00b7");
  const meta = document.createElement("span");
  meta.className = "meta";
  meta.textContent = c.dataType + (c.nullable ? "" : " \u00b7");
  cn.append(meta);
  // No double-click action on a column, so a plain click can still insert the
  // name — the fastest way to build a select list while writing.
  cn.onclick = (e) => {
    e.stopPropagation();
    insertAtCursor(view, c.name);
  };
  cn.oncontextmenu = (e) =>
    contextMenu(e, [
      { label: `Column ${table}.${c.name}`, run: () => {}, heading: true },
      { label: "Insert name at cursor", run: () => insertAtCursor(view, c.name) },
      {
        label: "Drop column…",
        danger: true,
        run: () =>
          void showGenerated(`drop ${c.name}`, () =>
            api.generateDrop({ type: "column", db, table, name: c.name })),
      },
    ]);
  return cn;
}

function buildRoutineNode(connId: string, db: string, r: RoutineRef): HTMLElement {
  const isFn = r.kind === "function";
  const { n } = node("routine", r.name, "", isFn ? "\u0192" : "\u2699");

  const meta = document.createElement("span");
  meta.className = "meta";
  // The signature at a glance: a routine you cannot call without opening
  // something else is barely listed at all.
  const args = r.params.map((p) => p.name).join(", ");
  meta.textContent = isFn ? `(${args}) \u2192 ${r.returns ?? "?"}` : `(${args})`;
  n.append(meta);

  const append = () => void appendGenerated(() => api.generateCall(connId, db, r.name));

  // Double click appends the call to the tab in progress, as asked: calling a
  // routine is usually a step inside a script, not a fresh task the way
  // browsing a table is.
  n.ondblclick = (e) => {
    e.stopPropagation();
    e.preventDefault();
    append();
  };

  n.oncontextmenu = (e) =>
    contextMenu(e, [
      { label: `${isFn ? "Function" : "Procedure"} ${r.name}`, run: () => {}, heading: true },
      { label: "Append call to this tab", run: append },
      {
        label: "Examine definition\u2026",
        run: () =>
          void showGenerated(r.name, () => api.routineDdl(connId, db, r.name, r.kind)),
      },
      {
        label: `Drop ${isFn ? "function" : "procedure"}\u2026`,
        danger: true,
        run: () =>
          void showGenerated(`drop ${r.name}`, () =>
            api.generateDrop({ type: "routine", db, name: r.name, kind: r.kind })),
      },
    ]);

  return n;
}

// ------------------------------------------------------------------- export

/**
 * The table the active tab's results came from, when they came from one.
 *
 * Set only by the schema tree's browse action, which is the one place we *know*
 * the answer. Guessing it by parsing the SQL would be wrong for a join and would
 * make an exported `CREATE TABLE` claim a fidelity it does not have.
 */
const exportSourceOf = (): SourceTable | null => tabs.active()?.sourceTable ?? null;

/** "3 rows", "2 columns", "3 rows × 2 columns" — or nothing when all is selected. */
function describeSelection(): string {
  const { rows, cols } = results.selectionCounts();
  const parts: string[] = [];
  if (rows) parts.push(`${rows} row${rows === 1 ? "" : "s"}`);
  if (cols) parts.push(`${cols} column${cols === 1 ? "" : "s"}`);
  return parts.join(" \u00d7 ");
}

/**
 * Report what a copy or export did **without destroying the result**.
 *
 * `results.setMessage` replaces the grid — right for "no results yet" and an
 * error, catastrophic for "Copied 2 rows": copying your results used to delete
 * them from the screen and disable the button, so you could not copy twice
 * without re-running the query. Found by a UI test that had been passing,
 * because it asserted the clipboard and never looked at what was left behind.
 */
function showNote(text: string) {
  els.resultNote.textContent = text;
  els.resultNote.title = text;
  els.resultNote.hidden = !text;
}

function refreshExportBar() {
  const data = results.selectedData();
  const has = data !== null;
  els.btnCopy.disabled = !has;
  els.btnCopyHead.disabled = !has;
  els.btnExport.disabled = !has;
  const what = describeSelection();
  els.btnCopy.textContent = what ? `Copy ${what}` : "Copy";
  els.btnCopy.title = what
    ? `Copy the selected ${what}, without headers (Ctrl+C)`
    : "Copy every row and column, without headers (Ctrl+C)";
  els.btnCopyHead.title = what
    ? `Copy the selected ${what} with headers (Ctrl+Shift+C)`
    : "Copy every row and column with headers (Ctrl+Shift+C)";
}

async function copySelection(headers: boolean) {
  const data = results.selectedData();
  if (!data) return;
  try {
    const text = await api.clipboardText(
      { columns: data.columns, rows: data.rows, truncated: data.truncated },
      { ...defaultCsvOptions(), delimiter: "\t", headers, bom: false, crlf: false },
    );
    const how = await copyText(text);
    const what = describeSelection();
    const scope = what
      ? `the selected ${what}`
      : `all ${data.rows.length} row(s) \u00d7 ${data.columns.length} column(s)`;
    showNote(
      `Copied ${scope}${headers ? " with headers" : ""}.` +
        (data.truncated ? " The result was truncated, so this is not the whole query." : "") +
        (how === "exec-command" ? " (via the legacy clipboard path)" : ""),
    );
  } catch (err) {
    // Never a silent no-op — that is the whole lesson of the delete button.
    showNote(String(err));
  }
}

// Plain copy carries no headers. Pasting into another query, a spreadsheet
// column or a chat message is the common case, and a stray header row there is
// something you have to notice and delete. Headers are the deliberate act.
els.btnCopy.onclick = () => void copySelection(false);
els.btnCopyHead.onclick = () => void copySelection(true);
els.btnExport.onclick = () => openExportDialog();

function openExportDialog() {
  const data = results.selectedData();
  if (!data) return;

  els.exportError.hidden = true;
  const sql = results.activeStatementSql();
  const what = describeSelection();
  els.exportScopeNote.textContent =
    (what ? `Selected: ${what}. ` : "") +
    `${data.rows.length} row(s), ${data.columns.length} column(s) will be written.` +
    (data.truncated
      ? ` This result was cut off at ${data.rows.length} rows — "re-run the query" fetches the rest.`
      : "");

  // The unbounded path re-executes the statement, so it is only offered when
  // that can be done safely. Disabling it with a reason beats offering it and
  // being wrong once.
  const rerunOk = sql !== null && results.isEverything();
  const allOption = els.exportScope.options[1];
  allOption.disabled = !rerunOk;
  allOption.textContent = rerunOk
    ? "Re-run the query and export every row"
    : !results.isEverything()
      ? "Re-run the query \u2014 not available while a subset is selected"
      : "Re-run the query \u2014 not available for this statement";
  if (!rerunOk) els.exportScope.value = "shown";
  else if (data.truncated) els.exportScope.value = "all";

  const source = exportSourceOf();
  els.sqlTable.value = source?.table ?? "exported_rows";
  els.sqlCreateNote.textContent = source
    ? "Uses the server's own CREATE TABLE for this table, so it is exact."
    : "These rows are not from a single table, so the schema is derived from the result columns: types are widened, and there are no keys or defaults.";

  syncExportFormat();
  els.exportDialog.showModal();
}

function syncExportFormat() {
  const csv = els.exportFormat.value === "csv";
  els.exportCsvOpts.hidden = !csv;
  els.exportSqlOpts.hidden = csv;
}
els.exportFormat.onchange = syncExportFormat;
els.exportCancel.onclick = () => els.exportDialog.close();

function csvOptionsFromForm(): CsvOptions {
  return {
    delimiter: els.csvDelim.value || ",",
    crlf: els.csvCrlf.checked,
    bom: els.csvBom.checked,
    headers: els.csvHeaders.checked,
    nullAs: els.csvNull.value,
    formulaGuard: els.csvGuard.checked,
  };
}

function insertOptionsFromForm(): InsertOptions {
  return {
    table: els.sqlTable.value.trim() || "exported_rows",
    db: exportSourceOf()?.db ?? null,
    createTable: els.sqlCreate.checked,
    batchSize: Math.max(1, Number(els.sqlBatch.value) || 100),
  };
}

els.exportForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const data = results.selectedData();
  if (!data) return;

  const format = els.exportFormat.value as ExportFormat;
  const all = els.exportScope.value === "all";
  const csv = csvOptionsFromForm();
  const inserts = insertOptionsFromForm();
  const name = `${inserts.table}.${format === "csv" ? "csv" : "sql"}`;

  els.exportOk.disabled = true;
  els.exportOk.textContent = "Exporting…";
  try {
    const outcome = all
      ? await api.exportRerun(
          activeTab().id,
          results.activeStatementSql() ?? "",
          format,
          { csv, inserts },
          name,
        )
      : format === "csv"
        ? await api.exportCsv(
            { columns: data.columns, rows: data.rows, truncated: data.truncated },
            csv,
            name,
          )
        : await api.exportInserts(
            { columns: data.columns, rows: data.rows, truncated: data.truncated },
            inserts,
            exportSourceOf(),
            name,
          );

    // null means the user cancelled the save dialog, which is not a failure
    // and must not be reported as one.
    if (outcome) {
      showNote(describeExport(outcome, all));
      els.exportDialog.close();
    } else {
      els.exportOk.disabled = false;
      els.exportOk.textContent = "Export";
    }
  } catch (err) {
    // Keeps the dialog open with its values, so a refusal can be corrected
    // rather than re-entered.
    els.exportError.textContent = String(err);
    els.exportError.hidden = false;
    els.exportOk.disabled = false;
    els.exportOk.textContent = "Export";
  }
});

/**
 * Say what actually landed in the file.
 *
 * A truncated source is stated outright: writing 5000 rows and letting it look
 * like the whole table is the same silent-wrong-answer failure as Stage 0's
 * empty schema tree.
 */
function describeExport(o: ExportOutcome, wasRerun: boolean): string {
  const size = o.bytesWritten > 1024 * 1024
    ? `${(o.bytesWritten / 1024 / 1024).toFixed(1)} MB`
    : `${Math.max(1, Math.round(o.bytesWritten / 1024))} KB`;
  const parts = [`Exported ${o.rowsWritten} row(s), ${size}, to ${o.path}.`];
  if (o.truncatedSource) {
    parts.push(
      "These were the rows on screen, and the result had already been cut off — " +
        "the query has more. Use \u201cre-run the query\u201d to export all of them.",
    );
  } else if (wasRerun) {
    parts.push("The query was re-run with no row limit, so this is every row.");
  }
  parts.push(...o.warnings);
  return parts.join(" ");
}

// Grid keyboard: Ctrl+C copies the selection, Ctrl+A selects every column.
// Scoped to the grid so neither shadows the editor's own Ctrl+C / Ctrl+A —
// which is why the grid carries a tabindex and can hold focus at all.
$("grid").addEventListener("keydown", (e) => {
  const ev = e as KeyboardEvent;
  if (!(ev.ctrlKey || ev.metaKey)) return;
  if (ev.key === "c" || ev.key === "C") {
    ev.preventDefault();
    // Ctrl+C plain, Ctrl+Shift+C with headers.
    void copySelection(ev.shiftKey);
  } else if (ev.key === "a" || ev.key === "A") {
    ev.preventDefault();
    results.selectAll();
  }
});
$("grid").addEventListener("keydown", (e) => {
  if ((e as KeyboardEvent).key === "Escape") results.clearSelection();
});

// ---------------------------------------------------------------- execution

/**
 * Render whatever this tab should currently be showing.
 *
 * Busy is checked FIRST and deliberately. A running tab must not display the
 * result of its previous run: switching away and back would otherwise show
 * stale rows with no hint they are stale, while the real query is still in
 * flight. The running state belongs to the tab, not to whichever tab happened
 * to be focused when the run started.
 */
function showResults(tab: ScriptTab) {
  // A note describes what was done to the *previous* result, so it goes when
  // that result does.
  showNote("");
  // Every early return below ends in "no exportable data", so the bar is
  // refreshed on the way out rather than at each one.
  try {
    showResultsInner(tab);
  } finally {
    refreshExportBar();
  }
}

function showResultsInner(tab: ScriptTab) {
  if (tab.busy) {
    results.setMessage(
      tab.result
        ? "Running… the previous result is superseded."
        : "Running…",
    );
    return;
  }
  if (tab.error) {
    results.setMessage(tab.error);
    return;
  }
  if (!tab.result) {
    results.setMessage(
      connected ? "No results yet. Ctrl+Enter to run." : "Not connected.",
    );
    return;
  }
  results.show({
    result: tab.result,
    activeIndex: tab.activeResultIndex,
    widths: tab.colWidths,
    scrollTop: tab.scrollTop,
    selection: tab.colSelection,
    onSelect: (i) => { tab.activeResultIndex = i; },
    onScrolled: (top) => { tab.scrollTop = top; },
    onSelectionChanged: () => refreshExportBar(),
  });
}

async function run(sql: string) {
  const tab = activeTab();
  if (!connected || tab.busy || !sql.trim()) return;
  tab.error = null;
  setBusy(tab, true);
  if (isFocused(tab)) showResults(tab);

  try {
    const secs = Number(els.timeout.value) || 0;
    const res = await api.runScript(tab.id, sql, els.autoLimit.checked, secs > 0 ? secs : null);
    // A `USE db` inside the script may have moved THIS tab's database.
    const st = await api.tabStatus(tab.id);

    tab.result = res;
    tab.error = null;
    tab.activeResultIndex = ResultView.initialIndex(res);
    tab.scrollTop = 0;
    tab.colWidths.clear();
    tab.activeDb = st.currentDatabase;
    tab.serverConnId = st.connectionId;

    if (st.currentDatabase && st.currentDatabase !== activeDb && isFocused(tab)) {
      activeDb = st.currentDatabase;
      markActiveDb(st.currentDatabase);
    }
  } catch (err) {
    tab.result = null;
    tab.error = String(err);
  } finally {
    // Clear busy BEFORE painting: showResults renders the running state while
    // the flag is set, so painting first would leave "Running…" over the rows.
    setBusy(tab, false);
    // The user may have switched tabs while this ran; only paint if still here.
    // Either way the outcome is on the tab, so switching back shows it.
    if (isFocused(tab)) {
      showResults(tab);
      syncConnLabel();
    }
  }
}

function markActiveDb(db: string | null) {
  activeDb = db;
  els.tree.querySelectorAll(".node.db").forEach((n) => {
    n.classList.toggle("db-active", db !== null && n.querySelector(".label")?.textContent === db);
  });
}

/** Ctrl+Enter: the selection if there is one, else the statement under the cursor. */
async function runStatementUnderCursor() {
  const sel = selectedText(view);
  if (sel) return run(sel);

  // Boundary rules live in exactly one place — the Rust splitter. This asks
  // for the statement text rather than reimplementing the scan here, so the
  // statement we run is by construction the one execution would pick.
  const stmt = await api.statementAtCursor(docText(view), cursorByteOffset(view));
  if (stmt) await run(stmt);
}

els.btnRun.onclick = () => void runStatementUnderCursor();
els.btnRunAll.onclick = () => void run(docText(view));
els.btnCancel.onclick = async () => {
  try {
    await api.cancelQuery(activeTab().id);
  } catch (err) {
    results.setMessage(String(err));
  }
};

// ---------------------------------------------------------------- splitters

function draggable(handle: HTMLElement, axis: "x" | "y") {
  handle.addEventListener("mousedown", (e) => {
    e.preventDefault();
    handle.classList.add("dragging");
    const move = (ev: MouseEvent) => {
      if (axis === "x") {
        const w = Math.min(Math.max(140, ev.clientX), window.innerWidth - 260);
        document.documentElement.style.setProperty("--sidebar-w", `${w}px`);
      } else {
        const rect = els.main.getBoundingClientRect();
        const h = Math.min(Math.max(80, ev.clientY - rect.top), rect.height - 120);
        document.documentElement.style.setProperty("--editor-h", `${h}px`);
      }
    };
    const up = () => {
      handle.classList.remove("dragging");
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", up);
    };
    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", up);
  });
}

draggable(els.vsplit, "x");
draggable(els.hsplit, "y");

// ------------------------------------------------------------------- boot

view = createEditor($("editor"), {
  onRunStatement: () => void runStatementUnderCursor(),
  onRunAll: () => void run(docText(view)),
  // Keeps the dirty dot honest without polling.
  onDocChanged: () => {
    tabs?.render();
    applyLintSetting();
    session.schedule();
  },
});

conns = new ConnectionManager($("rail"), {
  onActivate: (entry) => {
    // Switching connection swaps the whole workspace: tabs, schema tree and
    // status. The connection being left keeps its sessions and running queries.
    showTree(entry.profile.id);
    tabs.setActiveConnection(entry.profile.id);
    connected = entry.connected;
    markActiveDb(tabs.active()?.activeDb ?? null);
    syncBusy();
  },
  onConnected: async (entry, info) => {
    renderDatabases(entry.profile.id, info.databases);
    connected = true;

    // Before anything else, and awaited: `setActiveConnection` runs a moment
    // from now and creates an empty tab for a workspace it finds empty, which
    // would leave a stray Untitled-1 beside everything we just restored.
    const { warnings } = await session.restoreInto(entry.profile.id);

    // Register this connection's tabs with the backend; each is bound for life.
    for (const t of tabs.forConnection(entry.profile.id)) {
      void api.openTab(entry.profile.id, t.id);
      // Put each tab back on the schema it was left on. `open_tab` is already
      // issued unattended here; this is the same class of session setup, and
      // without it a restored script silently runs against a different schema
      // than the one it was written for. A failure is harmless — the tab simply
      // stays on the connection's default.
      if (t.activeDb && info.databases.includes(t.activeDb)) {
        void api.useDatabase(t.id, t.activeDb).catch(() => {});
      }
    }

    results.setMessage(`Connected to ${entry.profile.name} — MySQL ${info.serverVersion}.`);

    // Restored tabs are their own evidence — they are on screen. A tab that did
    // *not* come back is the thing nothing on screen can tell you, so it gets a
    // dialog rather than a status line that the first tab activation overwrites.
    // Not awaited: the workspace should finish opening behind it.
    if (warnings.length) {
      void choose("Some tabs were not restored", warnings.join("\n\n"), [
        { value: "ok", label: "OK", primary: true },
      ]);
    }
    syncBusy();
    session.schedule();
  },
  canDrop: async (entry) => {
    // Closing a connection closes its tabs, so the Stage 1 unsaved-changes
    // prompt has to apply to each of them.
    for (const t of tabs.forConnection(entry.profile.id)) {
      if (!(await files.confirmClose(t))) return false;
    }
    return true;
  },
  onDisconnected: (entry) => {
    trees.delete(entry.profile.id);
    if (conns.active()?.profile.id === entry.profile.id) {
      showTree(entry.profile.id);
      connected = false;
      results.setMessage(`Disconnected from ${entry.profile.name}.`);
      syncBusy();
    }
    for (const t of tabs.forConnection(entry.profile.id)) t.serverConnId = 0;
    session.schedule();
  },
  onRemoved: (entry) => {
    trees.delete(entry.profile.id);
    for (const t of tabs.forConnection(entry.profile.id)) tabs.discard(t.id);
    session.forget(entry.profile.id);
  },
  notify: (m) => results.setMessage(m),
}, openConnectionEditor);

tabs = new TabManager($("script-tabs"), view, {
  colourFor: (connectionId) => conns.get(connectionId)?.profile.colour ?? "var(--accent)",
  onCreated: (tab) => {
    // Register with the backend so it can hold a session for this tab. Harmless
    // before a connection exists; connect() registers everything again.
    void api.openTab(tab.connectionId, tab.id);
    session.schedule();
  },
  onActivate: (tab) => {
    // Compartment contents live in the EditorState, so a swapped-in state has
    // whatever config it was born with. Re-apply both after every switch.
    setSchema(view, schemaMap);
    applyLintSetting(true);
    showResults(tab);
    syncBusy();
    refreshFileNote();
    if (tab.activeDb) markActiveDb(tab.activeDb);
    view.focus();
    session.schedule();
  },
  canClose: (tab) => files.confirmClose(tab),
  onClosed: (tab) => {
    // Releases that tab's MySQL connection; leaving it would leak until
    // disconnect.
    void api.closeTab(tab.id);
    session.schedule();
  },
});

files = createFileUx({
  tabs,
  view,
  notify: (m) => results.setMessage(m),
  refreshNote: refreshFileNote,
});

// No tab is created here: a tab must belong to a connection, and at boot there
// is none. The first tab appears when a connection becomes active — restored
// from the session file if that connection had tabs when the app last closed.
//
// Read before anything can be written, or the first keystroke would save an
// empty session over the remembered one.
void session.boot().then((warning) => {
  if (warning) results.setMessage(warning);
});

// One source of truth for the row numbers: the backend owns them, and the UI
// asks rather than repeating them.
void api.appDefaults().then((d) => {
  browseLimit = d.browseLimit;
});

// Theme before anything else is drawn, so there is no flash of the wrong one.
// CodeMirror is told separately: its `dark` facet is not a CSS variable and
// decides how lint tooltips are drawn.
const theme = createTheme((dark) => setEditorTheme(view, dark));

// --- settings ---------------------------------------------------------------

const settings = loadSettings();

/**
 * Apply everything at boot: appearance through CSS, and the session defaults
 * into the controls that own them. `browseLimit` is the one exception — the
 * backend still supplies the number, and this only overrides it.
 */
function applySettings() {
  applyAppearance(settings);
  theme.set(settings.theme);
  els.autoLimit.checked = settings.autoLimit;
  els.lintOn.checked = settings.lint;
  els.timeout.value = String(settings.timeoutSecs);
  browseLimit = settings.browseLimit;
  // Via the existing helper, which owns the debounce bucket — reconfiguring
  // the linter behind its back would leave `appliedLintDelay` lying.
  applyLintSetting(true);
}

for (const f of FONTS) {
  els.setFont.append(new Option(f.label, f.stack));
}

function openSettings() {
  els.setTheme.value = theme.current();
  els.setFont.value = settings.fontFamily;
  els.setFontSize.value = String(settings.fontSize);
  els.setAutoLimit.checked = settings.autoLimit;
  els.setLint.checked = settings.lint;
  els.setTimeout.value = String(settings.timeoutSecs);
  els.setBrowse.value = String(settings.browseLimit);
  els.setUpdateNote.hidden = true;
  els.settingsDialog.showModal();
  void showVersion();
}

/** Every change takes effect immediately — a settings dialog with an OK button
 *  makes you guess what a font looks like before you can see it. */
function commit() {
  settings.fontFamily = els.setFont.value;
  settings.fontSize = Number(els.setFontSize.value) || settings.fontSize;
  settings.autoLimit = els.setAutoLimit.checked;
  settings.lint = els.setLint.checked;
  settings.timeoutSecs = Math.max(0, Number(els.setTimeout.value) || 0);
  settings.browseLimit = Math.max(1, Number(els.setBrowse.value) || 1);
  settings.theme = els.setTheme.value as ThemePref;
  saveSettings(settings);
  applySettings();
}

for (const el of [
  els.setFont, els.setFontSize, els.setAutoLimit, els.setLint, els.setTimeout, els.setBrowse,
]) {
  el.onchange = commit;
}
els.setTheme.onchange = commit;
els.btnSettings.onclick = () => openSettings();
els.setClose.onclick = () => els.settingsDialog.close();

/** The running version, which every check reports whatever else it finds. */
async function showVersion() {
  try {
    const s = await api.updateCheck();
    els.setVersion.textContent =
      s.type === "unsupported"
        ? `Version unknown \u2014 ${s.reason}`
        : `Running version ${s.current}.`;
  } catch {
    els.setVersion.textContent = "Version unavailable.";
  }
}

/**
 * The manual check. The boot check is silent by design, which leaves no way to
 * ask again after being offline — this is that way.
 */
els.setCheckUpdate.onclick = async () => {
  els.setCheckUpdate.disabled = true;
  els.setUpdateNote.hidden = false;
  els.setUpdateNote.textContent = "Checking\u2026";
  try {
    const s = await api.updateCheck();
    if (s.type === "available") {
      els.setUpdateNote.textContent = `Version ${s.version} is available.`;
      els.settingsDialog.close();
      await offerUpdate(s);
    } else if (s.type === "unsupported") {
      els.setUpdateNote.textContent = s.reason;
    } else {
      els.setUpdateNote.textContent = `You are on the latest version (${s.current}).`;
    }
  } catch (err) {
    els.setUpdateNote.textContent = String(err);
  } finally {
    els.setCheckUpdate.disabled = false;
  }
};

applySettings();

// Saved connections appear in the rail immediately, disconnected. Nothing is
// contacted until the user clicks one.
void conns.loadSaved();

// --- self-update -----------------------------------------------------------

/**
 * Ask once at boot whether there is anything newer, and *only* show a button.
 *
 * Checking is automatic; installing is not, and the two are deliberately
 * different in kind. Nothing is downloaded, nothing is applied, and nothing
 * interrupts: the app that greets you with a modal before you have opened it is
 * the one everybody learns to dismiss without reading.
 *
 * A failed check stays quiet. Being offline is the ordinary case, and it is not
 * news — the button simply does not appear.
 */
async function checkForUpdate() {
  let status;
  try {
    status = await api.updateCheck();
  } catch {
    return;
  }
  if (status.type !== "available") return;

  els.btnUpdate.hidden = false;
  els.btnUpdate.textContent = `Update to ${status.version}`;
  els.btnUpdate.title = `You are running ${status.current}.`;
  els.btnUpdate.onclick = () => void offerUpdate(status);
}

/** Ask, then install only on a yes. Declining leaves the button where it was. */
async function offerUpdate(status: Extract<UpdateStatus, { type: "available" }>) {
  const notes = status.notes?.trim();
  const answer = await choose(
    `Update to ${status.version}?`,
    `You are running ${status.current}.` +
      (status.date ? ` ${status.version} was released ${status.date.slice(0, 10)}.` : "") +
      (notes ? `\n\n${notes}` : "") +
      "\n\nThe app will close while it installs, then reopen. " +
      "Unsaved scripts are not saved for you.",
    [
      { value: "install", label: "Download and install", primary: true },
      { value: "later", label: "Not now" },
    ],
  );
  if (answer !== "install") return;

  els.btnUpdate.disabled = true;
  els.btnUpdate.textContent = "Downloading\u2026";
  try {
    // On Windows this hands over to the installer and may never return.
    await api.updateInstall();
  } catch (err) {
    els.btnUpdate.disabled = false;
    els.btnUpdate.textContent = `Update to ${status.version}`;
    await choose("The update could not be installed", String(err), [
      { value: "ok", label: "Close", primary: true },
    ]);
  }
}

void checkForUpdate();

// Dropping a file onto the window opens it. Tauri intercepts drag-and-drop at
// the webview level, so the HTML5 drop events never fire — this is the only
// way to receive them.
void getCurrentWebview().onDragDropEvent((event) => {
  if (event.payload.type === "drop") {
    void files.openPaths(event.payload.paths);
  }
});

// Quitting with unsaved work must ask first. `preventDefault` keeps the window
// open while we do.
void getCurrentWindow().onCloseRequested(async (event) => {
  if (!(await files.confirmQuit())) {
    event.preventDefault();
    return;
  }
  // Last chance: the debounce may still be holding the most recent keystrokes.
  await session.flush();
});

// --- tab keybindings, at window level so they work even when the tab bar has
// focus. Ctrl+W in particular must be intercepted or the webview may act on it.
window.addEventListener("keydown", (e) => {
  if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
  const inDialog = els.dialog.open;
  if (inDialog) return;

  if (e.key === "t" || e.key === "T") {
    e.preventDefault();
    tabs.create();
  } else if (e.key === "w" || e.key === "W") {
    e.preventDefault();
    const t = tabs.active();
    if (t) void tabs.close(t.id);
  } else if (e.key === "Tab") {
    e.preventDefault();
    tabs.cycle(e.shiftKey ? -1 : 1);
  } else if (/^[1-9]$/.test(e.key)) {
    e.preventDefault();
    tabs.activateIndex(Number(e.key) - 1);
  } else if (e.key === "o" || e.key === "O") {
    e.preventDefault();
    void files.openViaDialog();
  } else if (e.key === "s" || e.key === "S") {
    e.preventDefault();
    const t = tabs.active();
    if (!t) return;
    void (e.shiftKey ? files.saveAs(t) : files.save(t));
  }
});

els.lintOn.onchange = () => applyLintSetting(true);
applyLintSetting(true);

results.setMessage("No connection. Use + in the left rail to add one.");
syncBusy();
