// The query history dialog.
//
// # What it is for
//
// Finding a statement you ran before, and putting it back in the editor. It
// **never runs anything** — same rule as every generated-SQL action in this
// app: the user reviews and executes. Choosing a row inserts it at the cursor,
// which is undoable; the context menu offers a new tab and a copy.
//
// Recording happens in Rust, at the one point every execution passes through.
// This module only reads.

import { api, type HistoryHit } from "./api";
import { choose } from "./dialog";
import { contextMenu } from "./menu";

export interface HistoryDeps {
  /** Elements, resolved by the caller so this module owns no lookups. */
  dialog: HTMLDialogElement;
  search: HTMLInputElement;
  thisConnectionOnly: HTMLInputElement;
  list: HTMLElement;
  clear: HTMLButtonElement;
  close: HTMLButtonElement;
  /** The connection whose workspace is on screen, or null. */
  activeConnection: () => string | null;
  /** Put this SQL where the cursor is. */
  insert: (sql: string) => void;
  /** Open it as a new tab instead. */
  openInTab: (sql: string) => void;
  copy: (sql: string) => void;
  notify: (message: string) => void;
}

/** A short, local "when" — the exact timestamp is on the row's tooltip. */
function when(at: number): string {
  const delta = Date.now() - at;
  const mins = Math.floor(delta / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d ago`;
  return new Date(at).toLocaleDateString();
}

export function createHistory(deps: HistoryDeps) {
  let hits: HistoryHit[] = [];
  let selected = 0;
  /** Bumped per request so a slow search cannot overwrite a newer one. */
  let generation = 0;

  function rowFor(hit: HistoryHit, index: number): HTMLElement {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "hist-row" + (index === selected ? " selected" : "");
    row.dataset.index = String(index);
    row.title = new Date(hit.at).toLocaleString();

    const sql = document.createElement("div");
    sql.className = "hist-sql";
    // textContent, never innerHTML: this is SQL somebody typed.
    sql.textContent = hit.sql;

    const meta = document.createElement("div");
    meta.className = "hist-meta";
    const bits: Array<{ text: string; failed?: boolean }> = [{ text: when(hit.at) }];
    if (hit.database) bits.push({ text: hit.database });
    if (hit.status === "error") {
      // The error is why you are looking for the statement, so it is on the row.
      bits.push({ text: hit.error ?? "failed", failed: true });
    } else if (hit.rows !== null) {
      bits.push({ text: `${hit.rows} row${hit.rows === 1 ? "" : "s"}` });
    }
    bits.push({ text: `${hit.elapsedMs} ms` });
    if (hit.runs > 1) bits.push({ text: `run ${hit.runs}×` });
    for (const b of bits) {
      const span = document.createElement("span");
      if (b.failed) span.className = "failed";
      span.textContent = b.text;
      meta.append(span);
    }

    row.append(sql, meta);
    row.onclick = () => {
      selected = index;
      accept();
    };
    row.oncontextmenu = (e) =>
      contextMenu(e, [
        { label: "Insert at cursor", run: () => (selected = index, accept()) },
        {
          label: "Open in a new tab",
          run: () => {
            deps.openInTab(hit.sql);
            deps.dialog.close();
          },
        },
        { label: "Copy", run: () => deps.copy(hit.sql) },
      ]);
    return row;
  }

  function render() {
    if (hits.length === 0) {
      const empty = document.createElement("div");
      empty.className = "hist-empty";
      empty.textContent = deps.search.value.trim()
        ? "Nothing matches that."
        : "Nothing here yet — statements you run are recorded as you go.";
      deps.list.replaceChildren(empty);
      return;
    }
    deps.list.replaceChildren(...hits.map(rowFor));
  }

  function moveSelection(delta: number) {
    if (hits.length === 0) return;
    selected = Math.max(0, Math.min(hits.length - 1, selected + delta));
    render();
    deps.list
      .querySelector(".hist-row.selected")
      ?.scrollIntoView({ block: "nearest" });
  }

  /** Take the selected statement into the editor. */
  function accept() {
    const hit = hits[selected];
    if (!hit) return;
    deps.dialog.close();
    deps.insert(hit.sql);
  }

  async function refresh() {
    const mine = ++generation;
    const connectionId = deps.thisConnectionOnly.checked ? deps.activeConnection() : null;
    try {
      const found = await api.historySearch(deps.search.value, connectionId);
      // A search typed quickly fires several requests; only the newest may win.
      if (mine !== generation) return;
      hits = found;
      selected = 0;
      render();
    } catch (err) {
      if (mine !== generation) return;
      hits = [];
      render();
      deps.notify(String(err));
    }
  }

  deps.search.oninput = () => void refresh();
  deps.thisConnectionOnly.onchange = () => void refresh();
  deps.close.onclick = () => deps.dialog.close();

  deps.clear.onclick = async () => {
    // Destructive and unrecoverable, so it asks — through dialog.ts, never
    // window.confirm, which Tauri does not provide.
    const scope = deps.thisConnectionOnly.checked ? deps.activeConnection() : null;
    const answer = await choose(
      "Clear query history?",
      scope
        ? "The recorded statements for this connection will be deleted. This cannot be undone."
        : "Every recorded statement, for every connection, will be deleted. This cannot be undone.",
      [
        { value: "cancel", label: "Cancel", primary: true },
        { value: "clear", label: "Clear", danger: true },
      ],
    );
    if (answer !== "clear") return;
    try {
      await api.historyClear(scope);
      await refresh();
    } catch (err) {
      deps.notify(String(err));
    }
  };

  // Arrow keys move, Enter takes. Handled on the dialog so they work whether
  // focus is in the search box or the list — typing to filter and pressing Down
  // to pick is the whole interaction.
  deps.dialog.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      moveSelection(1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      moveSelection(-1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      accept();
    }
  });

  return {
    async open() {
      // `showModal()` on an open dialog throws; re-asking for history you are
      // already looking at should just put the cursor back in the search box.
      if (deps.dialog.open) {
        deps.search.focus();
        return;
      }
      deps.search.value = "";
      // Default to the connection you are looking at, when there is one.
      const active = deps.activeConnection();
      deps.thisConnectionOnly.checked = active !== null;
      deps.thisConnectionOnly.disabled = active === null;
      deps.dialog.showModal();
      deps.search.focus();
      await refresh();
    },
  };
}
