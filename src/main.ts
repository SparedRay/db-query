import type { EditorView } from "@codemirror/view";
import type { SQLNamespace } from "@codemirror/lang-sql";

import { api, type ConnProfile, type TableRef } from "./api";
import { ResultView } from "./grid";
import { TabManager, type ScriptTab } from "./tabs";
import {
  ConnectionManager, COLOURS, newConnectionId, type ConnectionEntry,
} from "./connections";
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
  connRememberRow: $<HTMLLabelElement>("conn-remember-row"),
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

let conns!: ConnectionManager;

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
  const active = conns?.active();
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
    if (!children.hidden) await loadTables(connId, db, children, n);
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
      await loadTables(connId, db, children, n);
    }
  };

  wrap.append(n, children);
  return wrap;
}

async function loadTables(connId: string, db: string, host: HTMLElement, dbNode: HTMLElement) {
  dbNode.classList.add("loading");
  try {
    const tables = await api.listTables(connId, db);
    host.replaceChildren(...tables.map((t) => buildTableNode(connId, db, t)));
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

function buildTableNode(connId: string, db: string, t: TableRef): HTMLElement {
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
        const cols = await api.listColumns(connId, db, t.name);
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
  onConnected: (entry, info) => {
    renderDatabases(entry.profile.id, info.databases);
    connected = true;
    // Register this connection's tabs with the backend; each is bound for life.
    for (const t of tabs.forConnection(entry.profile.id)) {
      void api.openTab(entry.profile.id, t.id);
    }
    results.setMessage(`Connected to ${entry.profile.name} — MySQL ${info.serverVersion}.`);
    syncBusy();
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
  },
  onRemoved: (entry) => {
    trees.delete(entry.profile.id);
    for (const t of tabs.forConnection(entry.profile.id)) tabs.discard(t.id);
  },
  notify: (m) => results.setMessage(m),
}, openConnectionEditor);

tabs = new TabManager($("script-tabs"), view, {
  colourFor: (connectionId) => conns.get(connectionId)?.profile.colour ?? "var(--accent)",
  onCreated: (tab) => {
    // Register with the backend so it can hold a session for this tab. Harmless
    // before a connection exists; connect() registers everything again.
    void api.openTab(tab.connectionId, tab.id);
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
    void api.closeTab(tab.id);
  },
});

files = createFileUx({
  tabs,
  view,
  notify: (m) => results.setMessage(m),
  refreshNote: refreshFileNote,
});

// No tab is created here: a tab must belong to a connection, and at boot there
// is none. The first tab appears when a connection becomes active.

// Saved connections appear in the rail immediately, disconnected. Nothing is
// contacted until the user clicks one.
void conns.loadSaved();

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

results.setMessage("No connection. Use + in the left rail to add one.");
syncBusy();
