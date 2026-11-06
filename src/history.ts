// The query history dock.
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
//
// # Why it is not a dialog any more (§6.18)
//
// It was one from Stage 13, and using a row had to **close** it first:
// `deps.dialog.close()` on the line before `deps.insert(...)`, because a modal
// owns the screen and cannot put text into the editor behind it. So the list
// you were working through was destroyed by the act of using one row of it —
// exactly what Stage 21 §2 recorded about the performance panel's first cut,
// and the reason that one is a dock. Those two `close()` calls are gone: the
// panel stays open, and the second statement you want is still on screen.
//
// The grid is the caller's, as it is for every other dock, so this module asks
// for `setOpen` and never touches a class or a column.

import { api, type HistoryHit } from "./api";
import { choose } from "./dialog";
import { contextMenu } from "./menu";

export interface HistoryDeps {
  /** Elements, resolved by the caller so this module owns no lookups. */
  pane: HTMLElement;
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
  /** Show or hide the dock. Owned by the caller, which owns the grid. */
  setOpen: (open: boolean) => void;
  /** Whether the dock is showing right now. */
  isOpen: () => boolean;
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
    // Where it came from. Shown only for the assistant: "user" is the default
    // and labelling every ordinary statement would be noise.
    if (hit.source === "assistant") bits.push({ text: "from the assistant" });
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
        // **The dock stays open.** It closed itself here and in `accept()`,
        // which is the defect this file's header records.
        { label: "Open in a new tab", run: () => deps.openInTab(hit.sql) },
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

  /** Take the selected statement into the editor, and stay open. */
  function accept() {
    const hit = hits[selected];
    if (!hit) return;
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
  deps.close.onclick = () => deps.setOpen(false);

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

  // Arrow keys move, Enter takes. Handled on the **pane**, so they work
  // wherever focus is inside it — typing to filter and pressing Down to pick
  // is the whole interaction. On the dialog this listener saw every key in the
  // app, because a modal has the keyboard; on a dock it sees the keys meant
  // for it, which is the behaviour that was wanted both times.
  deps.pane.addEventListener("keydown", (e) => {
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

  /** The scope control, against the connection that is actually on screen. */
  function syncScope() {
    const active = deps.activeConnection();
    deps.thisConnectionOnly.checked = active !== null;
    deps.thisConnectionOnly.disabled = active === null;
  }

  return {
    /** Open the dock on a fresh search. */
    async open() {
      // Already open: put the cursor back in the search box, which is what
      // asking for history you are already looking at means. (As a dialog this
      // branch existed because `showModal()` on an open dialog throws; it
      // survives because the behaviour was right for its own sake.)
      if (deps.isOpen()) {
        deps.search.focus();
        return;
      }
      deps.search.value = "";
      syncScope();
      deps.setOpen(true);
      deps.search.focus();
      await refresh();
    },
    /** The rail button and `Ctrl+H`, which toggle as every other dock's do. */
    async toggle() {
      if (deps.isOpen()) {
        deps.setOpen(false);
        return;
      }
      await this.open();
    },
    close() {
      deps.setOpen(false);
    },
    /**
     * Follow the active connection (§6.8's rule, now that this can stay open
     * across a switch).
     *
     * A dialog was opened, read and dismissed inside one connection, so its
     * scope could be decided once at `open()`. A dock outlives the switch: the
     * box says "This connection only" and would have gone on showing the old
     * server's statements, with nothing on screen saying which server they
     * came from. Shut, it costs nothing — the search is not sent.
     */
    connectionChanged() {
      if (!deps.isOpen()) return;
      const active = deps.activeConnection();
      const scoped = deps.thisConnectionOnly.checked;
      // **Only the availability follows the connection, not the choice.** The
      // first cut called `syncScope()` here, which re-checked the box on
      // every switch and so overrode a deliberate "every connection" with a
      // list of one -- caught by `an unscoped list is not re-read on a
      // switch`, which is the test that exists because `open()` and a switch
      // want different things from the same two lines.
      deps.thisConnectionOnly.disabled = active === null;
      if (scoped && active === null) {
        // There is no connection left to be scoped to, so widen rather than
        // keep showing a list filtered by a connection that has gone.
        deps.thisConnectionOnly.checked = false;
        void refresh();
        return;
      }
      // Re-read only when the scope is the connection: an all-connections
      // list is the same list whichever one is active.
      if (scoped) void refresh();
    },
  };
}
