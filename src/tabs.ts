// Script tabs: several SQL buffers open at once, each with its own editor
// state, its own results, and its own backend connection.

import type { EditorState, Text } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

import type { ScriptResult, SourceTable } from "./api";
import { createEditorState, STARTER_DOC } from "./editor";
import { emptySelection, type GridSelection } from "./grid";

export interface ScriptTab {
  /** Stable for the tab's whole life; this is what the backend keys on. */
  id: string;
  /** The connection this tab belongs to, for life. Tabs never migrate. */
  connectionId: string;
  title: string;
  filePath: string | null;
  dialect: string;
  /** "utf-8" | "utf-8-lossy". Lossy tabs must not Save over their source. */
  encoding: string;
  /** "lf" | "crlf" — what to write back. */
  lineEnding: string;
  /** Modification time when opened or last saved; seeds the conflict check. */
  mtimeMs: number | null;

  /**
   * Text as last loaded or saved. Dirtiness is `doc !== baseline`, never a
   * "was edited" flag — typing a character and undoing it must leave the tab
   * clean, or people learn to dismiss the unsaved-changes prompt.
   *
   * Stored as CodeMirror `Text` so the comparison is a cheap tree compare
   * rather than an O(n) string build on every keystroke.
   */
  baseline: Text;

  /** Owns the doc, the cursor, and the undo history. */
  state: EditorState;

  // --- results, kept per tab so switching away and back restores them exactly
  result: ScriptResult | null;
  /** Last failure for this tab. Held as state so it survives a tab switch. */
  error: string | null;
  activeResultIndex: number;
  colWidths: Map<string, number>;
  /**
   * Selected rows and columns of the active statement's result. Lives on the
   * tab so a selection survives a tab switch, like widths and scroll position.
   */
  colSelection: GridSelection;
  /**
   * The table this tab's results came from, when they came from exactly one —
   * set by the schema tree's browse action, which is the only place we know it.
   * Per tab, not global: switching tabs must not carry another tab's source
   * into an export and claim a fidelity it does not have.
   */
  sourceTable: SourceTable | null;
  scrollTop: number;

  busy: boolean;
  activeDb: string | null;
  /** MySQL's own connection id for this tab, 0 until it first runs. */
  serverConnId: number;
  /** Set while the tab has never been saved, so the number can be reused. */
  untitledNumber: number | null;
}

/**
 * One tab as `restore` wants it: already resolved, with the file read and the
 * baseline decided. Everything ambiguous — a missing file, a changed file — is
 * settled by the caller, so this stays a plain description of a tab that is
 * about to exist.
 */
export interface RestoredTab {
  title: string;
  filePath: string | null;
  dialect: string;
  encoding: string;
  lineEnding: string;
  mtimeMs: number | null;
  /** The buffer. */
  contents: string;
  /** What the buffer is compared against; equal to `contents` when clean. */
  baseline: string;
  cursor: number;
  activeDb: string | null;
  untitledNumber: number | null;
}

export interface TabHooks {
  /** A different tab became active. */
  onActivate: (tab: ScriptTab) => void;
  /** About to close — return false to keep it open (unsaved-changes prompt). */
  canClose?: (tab: ScriptTab) => Promise<boolean>;
  /** Closed for good; release the backend session. */
  onClosed: (tab: ScriptTab) => void;
  /** A tab was created; register it with the backend. */
  onCreated: (tab: ScriptTab) => void;
  /**
   * The connection's colour, so the tab strip carries it too.
   *
   * Only one connection's tabs are visible at a time, so without this the tab
   * bar looks identical whichever server you are on — which is exactly the
   * moment you want to be sure.
   */
  colourFor?: (connectionId: string) => string;
}

let idSeq = 0;

export class TabManager {
  private tabs: ScriptTab[] = [];
  private activeId: string | null = null;
  /** Only this connection's tabs are shown; the rest stay live in memory. */
  private activeConnectionId: string | null = null;
  /** Where each connection was left, so switching back returns you there. */
  private lastActive = new Map<string, string>();

  constructor(
    private bar: HTMLElement,
    private view: EditorView,
    private hooks: TabHooks,
  ) {
    this.bar.addEventListener("wheel", (e) => {
      // Horizontal scroll with a plain wheel, so an overflowing bar is usable
      // without a horizontal trackpad gesture.
      if (e.deltaY !== 0 && e.deltaX === 0) {
        this.bar.scrollLeft += e.deltaY;
        e.preventDefault();
      }
    });
  }

  /** Every tab across every connection. */
  all(): ScriptTab[] {
    return this.tabs;
  }

  /** Tabs belonging to the connection currently on screen. */
  visible(): ScriptTab[] {
    return this.tabs.filter((t) => t.connectionId === this.activeConnectionId);
  }

  forConnection(connectionId: string): ScriptTab[] {
    return this.tabs.filter((t) => t.connectionId === connectionId);
  }

  activeConnection(): string | null {
    return this.activeConnectionId;
  }

  /**
   * Show a different connection's workspace.
   *
   * Returns to wherever that connection was last left, or opens an empty tab if
   * it has never been used. The outgoing connection's tabs keep their editor
   * state, their results and their running queries — switching is a change of
   * view, not of session.
   */
  setActiveConnection(connectionId: string) {
    if (connectionId === this.activeConnectionId) return;
    this.stash();
    if (this.activeId) {
      const current = this.tabs.find((t) => t.id === this.activeId);
      if (current) this.lastActive.set(current.connectionId, current.id);
    }
    this.activeConnectionId = connectionId;

    const mine = this.visible();
    if (mine.length === 0) {
      this.activeId = null;
      this.create({ connectionId });
      return;
    }
    const remembered = this.lastActive.get(connectionId);
    const target = mine.find((t) => t.id === remembered) ?? mine[0];
    this.activeId = null; // force activate() past its early return
    this.activate(target.id);
  }

  active(): ScriptTab | null {
    return this.tabs.find((t) => t.id === this.activeId) ?? null;
  }

  /** Dirty = the buffer differs from what was last loaded or saved. */
  isDirty(tab: ScriptTab): boolean {
    const doc = tab.id === this.activeId ? this.view.state.doc : tab.state.doc;
    return !doc.eq(tab.baseline);
  }

  /** Numbering is per connection: each workspace counts from Untitled-1. */
  private nextUntitledNumber(): number {
    const taken = new Set(
      this.visible()
        .map((t) => t.untitledNumber)
        .filter((n): n is number => n !== null),
    );
    let n = 1;
    while (taken.has(n)) n++;
    return n;
  }

  create(opts?: {
    connectionId?: string;
    contents?: string;
    title?: string;
    sourceTable?: SourceTable | null;
    filePath?: string | null;
    dialect?: string;
    encoding?: string;
    lineEnding?: string;
    mtimeMs?: number | null;
  }): ScriptTab {
    const connectionId = opts?.connectionId ?? this.activeConnectionId;
    if (!connectionId) {
      throw new Error("a tab must belong to a connection");
    }
    const isUntitled = !opts?.filePath;
    const untitledNumber = isUntitled ? this.nextUntitledNumber() : null;
    const contents =
      opts?.contents ?? (this.forConnection(connectionId).length === 0 ? STARTER_DOC : "");
    const state = createEditorState(contents);

    const tab: ScriptTab = {
      id: `t${++idSeq}`,
      connectionId,
      title: opts?.title ?? `Untitled-${untitledNumber}`,
      filePath: opts?.filePath ?? null,
      dialect: opts?.dialect ?? "mysql",
      encoding: opts?.encoding ?? "utf-8",
      lineEnding: opts?.lineEnding ?? "lf",
      mtimeMs: opts?.mtimeMs ?? null,
      baseline: state.doc,
      state,
      result: null,
      error: null,
      activeResultIndex: 0,
      colWidths: new Map(),
      colSelection: emptySelection(),
      sourceTable: opts?.sourceTable ?? null,
      scrollTop: 0,
      busy: false,
      activeDb: null,
      serverConnId: 0,
      untitledNumber,
    };

    this.tabs.push(tab);
    this.hooks.onCreated(tab);
    this.activate(tab.id);
    return tab;
  }

  /** Capture live editor state back into the tab before we swap away from it. */
  private stash() {
    const current = this.active();
    if (current) current.state = this.view.state;
  }

  activate(id: string) {
    if (id === this.activeId) return;
    const next = this.tabs.find((t) => t.id === id);
    if (!next) return;

    this.stash();
    this.activeId = id;
    this.lastActive.set(next.connectionId, next.id);
    this.view.setState(next.state);
    this.render();
    this.hooks.onActivate(next);
  }

  async close(id: string) {
    const idx = this.tabs.findIndex((t) => t.id === id);
    if (idx === -1) return;
    const tab = this.tabs[idx];

    if (this.hooks.canClose && !(await this.hooks.canClose(tab))) return;

    // Stash first: if we are closing a background tab, the active tab's live
    // state must not be lost when we re-render.
    this.stash();
    this.tabs.splice(idx, 1);
    this.hooks.onClosed(tab);

    const siblings = this.forConnection(tab.connectionId);
    if (siblings.length === 0) {
      // Never zero tabs on a connection — its workspace would have nowhere to
      // type, and switching to it would show an empty pane.
      this.activeId = null;
      this.create({ connectionId: tab.connectionId });
      return;
    }
    if (this.activeId === id) {
      const neighbour = siblings[Math.min(idx, siblings.length - 1)];
      this.activeId = null; // force activate() past its early return
      this.activate(neighbour.id);
    } else {
      this.render();
    }
  }

  /**
   * Remove a tab without prompting.
   *
   * Used when its connection has gone away: the unsaved-changes prompt already
   * ran once at the connection level, and asking again per tab after the user
   * confirmed would be nagging.
   */
  discard(id: string) {
    const idx = this.tabs.findIndex((t) => t.id === id);
    if (idx === -1) return;
    this.stash();
    this.tabs.splice(idx, 1);
    if (this.activeId === id) this.activeId = null;
    this.render();
  }

  cycle(delta: number) {
    const mine = this.visible();
    if (mine.length < 2) return;
    const i = mine.findIndex((t) => t.id === this.activeId);
    const next = (i + delta + mine.length) % mine.length;
    this.activate(mine[next].id);
  }

  activateIndex(n: number) {
    const tab = this.visible()[n];
    if (tab) this.activate(tab.id);
  }

  /**
   * Rebuild a connection's tabs from a stored session.
   *
   * **Deliberately does not activate anything.** This runs from `onConnected`,
   * which fires before the connection becomes the visible workspace — calling
   * `activate` here would swap the editor to a tab that `visible()` does not
   * yet include, and render the wrong strip. Instead it records which tab
   * should come to the front, and `setActiveConnection` picks it up a moment
   * later through the same path it uses for any other workspace switch.
   *
   * Ids are minted fresh. Nothing outside the session file refers to the old
   * ones, the backend session is new regardless, and reusing them would collide
   * with `idSeq`, which counts from zero every launch.
   */
  restore(connectionId: string, specs: RestoredTab[], activeIndex: number): ScriptTab[] {
    const made: ScriptTab[] = [];
    for (const spec of specs) {
      const state = createEditorState(spec.contents, spec.cursor);
      const tab: ScriptTab = {
        id: `t${++idSeq}`,
        connectionId,
        title: spec.title,
        filePath: spec.filePath,
        dialect: spec.dialect,
        encoding: spec.encoding,
        lineEnding: spec.lineEnding,
        mtimeMs: spec.mtimeMs,
        // The whole point of restoring a dirty tab: the baseline is what was on
        // disk, so `isDirty` still answers the question it always answered.
        baseline: createEditorState(spec.baseline).doc,
        state,
        result: null,
        error: null,
        activeResultIndex: 0,
        colWidths: new Map(),
        colSelection: emptySelection(),
        sourceTable: null,
        scrollTop: 0,
        busy: false,
        activeDb: spec.activeDb,
        serverConnId: 0,
        untitledNumber: spec.untitledNumber,
      };
      this.tabs.push(tab);
      this.hooks.onCreated(tab);
      made.push(tab);
    }

    const front = made[activeIndex] ?? made[0];
    if (front) this.lastActive.set(connectionId, front.id);
    return made;
  }

  /**
   * Which tab is in front for a connection, on screen or not.
   *
   * The session file has to record where each workspace was left, not just the
   * one currently visible — every other connection's tabs are still live in
   * memory and are still going to be written down.
   */
  frontOf(connectionId: string): ScriptTab | null {
    const mine = this.forConnection(connectionId);
    if (mine.length === 0) return null;
    const id =
      this.activeConnectionId === connectionId
        ? this.activeId
        : this.lastActive.get(connectionId);
    return mine.find((t) => t.id === id) ?? mine[0];
  }

  /** Where the cursor is, whether or not this is the active tab. */
  cursorOf(tab: ScriptTab): number {
    const state = tab.id === this.activeId ? this.view.state : tab.state;
    return state.selection.main.head;
  }

  /** The tab's current text, whether or not it is the active one. */
  textOf(tab: ScriptTab): string {
    return this.docOf(tab).toString();
  }

  docOf(tab: ScriptTab): Text {
    return tab.id === this.activeId ? this.view.state.doc : tab.state.doc;
  }

  /** Replace a tab's content wholesale — used by Reload-from-disk. */
  replaceDoc(tab: ScriptTab, text: string) {
    if (tab.id === this.activeId) {
      this.view.dispatch({
        changes: { from: 0, to: this.view.state.doc.length, insert: text },
      });
      tab.baseline = this.view.state.doc;
    } else {
      tab.state = createEditorState(text);
      tab.baseline = tab.state.doc;
    }
    this.render();
  }

  /**
   * After a save, the buffer becomes the new clean baseline.
   *
   * `baseline` is the document as it was when the save STARTED, not now:
   * anything typed while the write was in flight is a genuine unsaved change
   * and must keep the tab dirty.
   */
  markSaved(
    tab: ScriptTab,
    opts: { path: string; name: string; mtimeMs: number; baseline: Text },
  ) {
    tab.filePath = opts.path;
    tab.title = opts.name;
    tab.untitledNumber = null;
    tab.mtimeMs = opts.mtimeMs;
    tab.baseline = opts.baseline;
    this.render();
  }

  render() {
    // Carry the active connection's colour into the strip, so the accent on the
    // selected tab matches the rail icon you clicked to get here.
    const colour = this.activeConnectionId
      ? this.hooks.colourFor?.(this.activeConnectionId)
      : undefined;
    this.bar.style.setProperty("--conn-colour", colour ?? "var(--accent)");

    const nodes = this.visible().map((tab) => {
      const el = document.createElement("div");
      el.className =
        "stab" +
        (tab.id === this.activeId ? " active" : "") +
        (this.isDirty(tab) ? " dirty" : "");
      el.title = tab.filePath ?? tab.title;

      if (tab.busy) {
        const spin = document.createElement("span");
        spin.className = "stab-busy";
        spin.textContent = "◴";
        spin.title = "A query is running in this tab";
        el.append(spin);
      }

      const label = document.createElement("span");
      label.className = "stab-label";
      label.textContent = tab.title;
      el.append(label);

      const close = document.createElement("span");
      close.className = "stab-mark";
      // Glyph comes from CSS: × normally, • when dirty, × again on hover — so
      // the unsaved marker never hides the close affordance.
      close.title = this.isDirty(tab) ? "Unsaved changes — click to close" : "Close";
      close.onclick = (e) => {
        e.stopPropagation();
        void this.close(tab.id);
      };
      el.append(close);

      el.onclick = () => this.activate(tab.id);
      el.onauxclick = (e) => {
        if (e.button === 1) {
          e.preventDefault();
          void this.close(tab.id);
        }
      };
      return el;
    });

    // No active connection means no workspace to add a tab to, so the button
    // is absent rather than present-and-throwing.
    const extras: HTMLElement[] = [];
    if (this.activeConnectionId) {
      const add = document.createElement("button");
      add.className = "stab-add";
      add.textContent = "+";
      add.title = "New tab (Ctrl+T)";
      add.onclick = () => this.create();
      extras.push(add);
    }

    this.bar.replaceChildren(...nodes, ...extras);
  }
}
