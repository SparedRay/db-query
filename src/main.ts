import type { EditorView } from "@codemirror/view";
import type { SQLNamespace } from "@codemirror/lang-sql";

import { api, type ConnConfig, type TableRef } from "./api";
import { ResultView } from "./grid";
import { TabManager, type ScriptTab } from "./tabs";
import { createFileUx, type FileUx } from "./files";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  createEditor, cursorByteOffset, docText, insertAtCursor, refreshLint, selectedText,
  setLinting, setSchema,
} from "./editor";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const els = {
  sidebar: $("sidebar"), main: $("main"), tree: $("tree"),
  connLabel: $<HTMLSpanElement>("conn-label"),
  btnConnect: $<HTMLButtonElement>("btn-connect"),
  btnRun: $<HTMLButtonElement>("btn-run"),
  btnRunAll: $<HTMLButtonElement>("btn-run-all"),
  btnCancel: $<HTMLButtonElement>("btn-cancel"),
  autoLimit: $<HTMLInputElement>("chk-autolimit"),
  lintOn: $<HTMLInputElement>("chk-lint"),
  timeout: $<HTMLInputElement>("num-timeout"),
  fileNote: $<HTMLSpanElement>("file-note"),
  dialog: $<HTMLDialogElement>("conn-dialog"),
  form: $<HTMLFormElement>("conn-form"),
  connError: $<HTMLParagraphElement>("conn-error"),
  connCancel: $<HTMLButtonElement>("conn-cancel"),
  vsplit: $("vsplit"), hsplit: $("hsplit"),
};

const results = new ResultView($("tabs"), $("grid"), $("status"));

let view!: EditorView;
let tabs!: TabManager;
let files!: FileUx;

/** The tab every command in this module acts on. */
const activeTab = (): ScriptTab => {
  const t = tabs.active();
  if (!t) throw new Error("no active tab");
  return t;
};

let connected = false;
let activeDb: string | null = null;
let hostLabel: string | null = null;
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
  if (!connected) {
    els.connLabel.textContent = "Not connected";
    els.connLabel.title = "";
    return;
  }
  const all = tabs?.all() ?? [];
  const running = all.filter((t) => t.busy).length;
  const withConn = all.filter((t) => t.connectionId > 0).length;

  const db = tabs?.active()?.activeDb ?? activeDb;
  const parts = [hostLabel ?? ""];
  if (db) parts.push(db);
  if (running > 0) parts.push(`${running} running`);
  els.connLabel.textContent = parts.join(" · ");
  els.connLabel.title =
    `${withConn} tab connection${withConn === 1 ? "" : "s"} + 2 shared ` +
    `(killer, meta) = ${withConn + 2} total.\n` +
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

function setConnected(label: string | null, db: string | null) {
  connected = label !== null;
  activeDb = db;
  hostLabel = label;
  if (!connected) for (const t of tabs?.all() ?? []) t.connectionId = 0;
  els.btnConnect.textContent = connected ? "Disconnect" : "Connect";
  syncBusy();
}

// --------------------------------------------------------------- connection

els.btnConnect.onclick = async () => {
  if (connected) {
    await api.disconnect();
    hostLabel = null;
    els.tree.replaceChildren();
    results.setMessage("Disconnected.");
    setConnected(null, null);
    return;
  }
  els.connError.hidden = true;
  els.dialog.showModal();
};

els.connCancel.onclick = () => els.dialog.close();

els.form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const fd = new FormData(els.form);
  const config: ConnConfig = {
    host: String(fd.get("host") ?? "").trim(),
    port: Number(fd.get("port") ?? 3306),
    user: String(fd.get("user") ?? "").trim(),
    password: String(fd.get("password") ?? ""),
    database: (String(fd.get("database") ?? "").trim() || null),
    allowInvalidCerts: fd.get("allowInvalidCerts") === "on",
  };

  const ok = $<HTMLButtonElement>("conn-ok");
  ok.disabled = true;
  ok.textContent = "Connecting…";
  try {
    const info = await api.connect(config);
    els.dialog.close();
    hostLabel = `${config.user}@${config.host}:${config.port}`;
    for (const t of tabs.all()) await api.openTab(t.id);
    setConnected(hostLabel, info.currentDatabase);
    renderDatabases(info.databases);
    results.setMessage(`Connected to MySQL ${info.serverVersion}.`);
  } catch (err) {
    // Connection failures are expected, not exceptional. Show the message and
    // leave the dialog open with the values still filled in.
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

function renderDatabases(dbs: string[]) {
  els.tree.replaceChildren(...dbs.map(buildDbNode));
}

function buildDbNode(db: string): HTMLElement {
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
    await api.refreshSchema(db);
    children.replaceChildren();
    children.dataset.loaded = "";
    if (!children.hidden) await loadTables(db, children, n);
  };
  n.append(refresh);

  n.onclick = async () => {
    // Clicking a database both expands it and makes it the active schema, so
    // unqualified table names in the editor resolve against it.
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
      await loadTables(db, children, n);
    }
  };

  wrap.append(n, children);
  return wrap;
}

async function loadTables(db: string, host: HTMLElement, dbNode: HTMLElement) {
  dbNode.classList.add("loading");
  try {
    const tables = await api.listTables(db);
    host.replaceChildren(...tables.map((t) => buildTableNode(db, t)));
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

function buildTableNode(db: string, t: TableRef): HTMLElement {
  const wrap = document.createElement("div");
  const isView = t.kind.toUpperCase().includes("VIEW");
  const { n, twisty, label } = node("table", t.name, "▸", isView ? "👁" : "▦");
  const children = document.createElement("div");
  children.className = "children";
  children.hidden = true;

  // Clicking the name inserts it; clicking the twisty expands columns.
  label.onclick = (e) => {
    e.stopPropagation();
    insertAtCursor(view, t.name);
  };

  n.onclick = async () => {
    children.hidden = !children.hidden;
    twisty.textContent = children.hidden ? "▸" : "▾";
    if (!children.hidden && !children.dataset.loaded) {
      n.classList.add("loading");
      try {
        const cols = await api.listColumns(db, t.name);
        children.replaceChildren(...cols.map((c) => {
          const { n: cn } = node("column", c.name, "", c.key === "PRI" ? "🔑" : "·");
          const meta = document.createElement("span");
          meta.className = "meta";
          meta.textContent = c.dataType + (c.nullable ? "" : " ·");
          cn.append(meta);
          cn.onclick = (e) => { e.stopPropagation(); insertAtCursor(view, c.name); };
          return cn;
        }));
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

  wrap.append(n, children);
  return wrap;
}

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
    onSelect: (i) => { tab.activeResultIndex = i; },
    onScrolled: (top) => { tab.scrollTop = top; },
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
    tab.connectionId = st.connectionId;

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

function markActiveDb(db: string) {
  activeDb = db;
  els.tree.querySelectorAll(".node.db").forEach((n) => {
    n.classList.toggle("db-active", n.querySelector(".label")?.textContent === db);
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
  },
});

tabs = new TabManager($("script-tabs"), view, {
  onCreated: (tab) => {
    // Register with the backend so it can hold a session for this tab. Harmless
    // before a connection exists; connect() registers everything again.
    if (connected) void api.openTab(tab.id);
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
  },
  canClose: (tab) => files.confirmClose(tab),
  onClosed: (tab) => {
    // Releases that tab's MySQL connection; leaving it would leak until
    // disconnect.
    if (connected) void api.closeTab(tab.id);
  },
});

files = createFileUx({
  tabs,
  view,
  notify: (m) => results.setMessage(m),
  refreshNote: refreshFileNote,
});

tabs.create();

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
  if (!(await files.confirmQuit())) event.preventDefault();
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

results.setMessage("Not connected. Press Connect to get started.");
setConnected(null, null);
