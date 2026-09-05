// Script tabs: several SQL buffers open at once, each with its own editor
// state, its own results, and its own backend connection.

import type { EditorState, Text } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

import type { ScriptResult } from "./api";
import { createEditorState, STARTER_DOC } from "./editor";

export interface ScriptTab {
  /** Stable for the tab's whole life; this is what the backend keys on. */
  id: string;
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
  scrollTop: number;

  busy: boolean;
  activeDb: string | null;
  /** Server connection id, 0 until this tab first runs. */
  connectionId: number;
  /** Set while the tab has never been saved, so the number can be reused. */
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
}

let idSeq = 0;

export class TabManager {
  private tabs: ScriptTab[] = [];
  private activeId: string | null = null;

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

  all(): ScriptTab[] {
    return this.tabs;
  }

  active(): ScriptTab | null {
    return this.tabs.find((t) => t.id === this.activeId) ?? null;
  }

  /** Dirty = the buffer differs from what was last loaded or saved. */
  isDirty(tab: ScriptTab): boolean {
    const doc = tab.id === this.activeId ? this.view.state.doc : tab.state.doc;
    return !doc.eq(tab.baseline);
  }

  private nextUntitledNumber(): number {
    const taken = new Set(
      this.tabs.map((t) => t.untitledNumber).filter((n): n is number => n !== null),
    );
    let n = 1;
    while (taken.has(n)) n++;
    return n;
  }

  create(opts?: {
    contents?: string;
    title?: string;
    filePath?: string | null;
    dialect?: string;
    encoding?: string;
    lineEnding?: string;
    mtimeMs?: number | null;
  }): ScriptTab {
    const isUntitled = !opts?.filePath;
    const untitledNumber = isUntitled ? this.nextUntitledNumber() : null;
    const contents = opts?.contents ?? (this.tabs.length === 0 ? STARTER_DOC : "");
    const state = createEditorState(contents);

    const tab: ScriptTab = {
      id: `t${++idSeq}`,
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
      scrollTop: 0,
      busy: false,
      activeDb: null,
      connectionId: 0,
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

    if (this.tabs.length === 0) {
      // Never zero tabs — an empty window has nowhere to type.
      this.activeId = null;
      this.create();
      return;
    }
    if (this.activeId === id) {
      const neighbour = this.tabs[Math.min(idx, this.tabs.length - 1)];
      this.activeId = null; // force activate() past its early return
      this.activate(neighbour.id);
    } else {
      this.render();
    }
  }

  cycle(delta: number) {
    if (this.tabs.length < 2) return;
    const i = this.tabs.findIndex((t) => t.id === this.activeId);
    const next = (i + delta + this.tabs.length) % this.tabs.length;
    this.activate(this.tabs[next].id);
  }

  activateIndex(n: number) {
    const tab = this.tabs[n];
    if (tab) this.activate(tab.id);
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
    const nodes = this.tabs.map((tab) => {
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

    const add = document.createElement("button");
    add.className = "stab-add";
    add.textContent = "+";
    add.title = "New tab (Ctrl+T)";
    add.onclick = () => this.create();

    this.bar.replaceChildren(...nodes, add);
  }
}
