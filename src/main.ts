import type { EditorView } from "@codemirror/view";
import type { SQLNamespace } from "@codemirror/lang-sql";

import { api, type ConnConfig, type TableRef } from "./api";
import { ResultView } from "./grid";
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
  dialog: $<HTMLDialogElement>("conn-dialog"),
  form: $<HTMLFormElement>("conn-form"),
  connError: $<HTMLParagraphElement>("conn-error"),
  connCancel: $<HTMLButtonElement>("conn-cancel"),
  vsplit: $("vsplit"), hsplit: $("hsplit"),
};

const results = new ResultView($("tabs"), $("grid"), $("status"));

let view: EditorView;
let connected = false;
let activeDb: string | null = null;
let inFlight = false;
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

/** Advisory diagnostics. Never gates Run — see the tracker's Phase 5b rule. */
async function lintSource(v: EditorView) {
  if (!connected) return [];
  const text = v.state.doc.toString();
  let diags;
  try {
    diags = await api.lintSql(text);
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

function applyLintSetting() {
  setLinting(view, els.lintOn.checked ? lintSource : null);
}

function setBusy(busy: boolean) {
  inFlight = busy;
  els.btnRun.disabled = busy || !connected;
  els.btnRunAll.disabled = busy || !connected;
  els.btnCancel.hidden = !busy;
}

function setConnected(label: string | null, db: string | null) {
  connected = label !== null;
  activeDb = db;
  els.connLabel.textContent = label
    ? `${label}${db ? ` · ${db}` : ""}`
    : "Not connected";
  els.btnConnect.textContent = connected ? "Disconnect" : "Connect";
  setBusy(false);
}

// --------------------------------------------------------------- connection

els.btnConnect.onclick = async () => {
  if (connected) {
    await api.disconnect();
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
    setConnected(`${config.user}@${config.host}:${config.port}`, info.currentDatabase);
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
    if (activeDb !== db) {
      try {
        await api.useDatabase(db);
        activeDb = db;
        els.tree.querySelectorAll(".node.db-active").forEach((x) => x.classList.remove("db-active"));
        n.classList.add("db-active");
        const label = els.connLabel.textContent?.split(" · ")[0] ?? "";
        els.connLabel.textContent = `${label} · ${db}`;
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

async function run(sql: string) {
  if (!connected || inFlight || !sql.trim()) return;
  setBusy(true);
  results.setMessage("Running…");
  try {
    const secs = Number(els.timeout.value) || 0;
    const res = await api.runScript(sql, els.autoLimit.checked, secs > 0 ? secs : null);
    results.show(res);
    // A `USE db` inside the script may have moved the active database.
    const st = await api.status();
    if (st.currentDatabase && st.currentDatabase !== activeDb) {
      setConnected(st.hostLabel, st.currentDatabase);
      markActiveDb(st.currentDatabase);
    }
  } catch (err) {
    results.setMessage(String(err));
  } finally {
    setBusy(false);
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
    await api.cancelQuery();
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
});

els.lintOn.onchange = applyLintSetting;
applyLintSetting();

results.setMessage("Not connected. Press Connect to get started.");
setConnected(null, null);
