// CodeMirror 6 setup: MySQL highlighting, schema-aware autocomplete, and the
// run keybindings. Statement boundaries are never computed here — the editor
// asks the Rust splitter, so execution and cursor detection cannot disagree.

import { EditorState, type Extension } from "@codemirror/state";
import {
  EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection, dropCursor,
  rectangularSelection, crosshairCursor, highlightSpecialChars,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab, redo } from "@codemirror/commands";
import { autocompletion, completionKeymap, closeBrackets } from "@codemirror/autocomplete";
import { search, searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import {
  bracketMatching, syntaxHighlighting, defaultHighlightStyle, indentOnInput,
} from "@codemirror/language";
import { sql, MySQL, type SQLNamespace } from "@codemirror/lang-sql";
import {
  forceLinting, linter, lintGutter, type Diagnostic as CmDiagnostic,
} from "@codemirror/lint";
import { Compartment } from "@codemirror/state";

const schemaCompartment = new Compartment();
const lintCompartment = new Compartment();

/**
 * The editor follows the app's theme tokens rather than carrying its own
 * colours, so switching themes needs no work here.
 *
 * The `dark` flag is the exception: it is a CodeMirror *facet*, not CSS, and
 * extensions consult it — lint tooltips in particular pick their own styling
 * from it. So the theme lives in a compartment and the flag is swapped when the
 * theme changes; the rules themselves are identical.
 */
const themeRules = {
  "&": { height: "100%", backgroundColor: "var(--bg)", color: "var(--fg)" },
  ".cm-content": { caretColor: "var(--fg)" },
  ".cm-gutters": {
    backgroundColor: "var(--bg-raised)",
    color: "var(--fg-dim)",
    border: "none",
  },
  ".cm-activeLine": { backgroundColor: "var(--bg-row-hover)" },
  ".cm-activeLineGutter": { backgroundColor: "var(--chip-bg)" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
    backgroundColor: "var(--sel-header-bg)",
  },
} as const;

const darkTheme = EditorView.theme(themeRules, { dark: true });
const lightTheme = EditorView.theme(themeRules, { dark: false });
const themeCompartment = new Compartment();

export interface EditorHooks {
  onRunStatement: () => void;
  onRunAll: () => void;
  /** Fired on every document change, so the tab bar can refresh its dirty dot. */
  onDocChanged?: () => void;
}

/**
 * Extensions are built once and shared by every tab's `EditorState`.
 *
 * Each tab owns a full `EditorState`, which is what carries its undo history —
 * that is why undo in one tab cannot reach into another. They must all be built
 * from the *same* extension array so the compartments below refer to the same
 * identities across tabs.
 */
let sharedExtensions: Extension[] | null = null;

/**
 * A fresh state for a new tab, wired with the same extensions as every other.
 *
 * Compartment contents (schema, linting) are per-state, so whoever swaps states
 * must re-apply them afterwards — see `setSchema` / `setLinting`.
 */
/**
 * `cursor` is clamped rather than trusted: it can come from a restored session
 * whose file changed on disk while the app was closed, and an out-of-range
 * offset makes `EditorState.create` throw — which at boot means no editor at
 * all.
 */
export function createEditorState(doc: string, cursor?: number): EditorState {
  if (!sharedExtensions) {
    throw new Error("createEditor() must run before createEditorState()");
  }
  const selection =
    cursor === undefined
      ? undefined
      : { anchor: Math.max(0, Math.min(cursor, doc.length)) };
  return EditorState.create({ doc, selection, extensions: sharedExtensions });
}

export function createEditor(parent: HTMLElement, hooks: EditorHooks): EditorView {
  const runKeys = keymap.of([
    { key: "Mod-Enter", preventDefault: true, run: () => (hooks.onRunStatement(), true) },
    { key: "Mod-Shift-Enter", preventDefault: true, run: () => (hooks.onRunAll(), true) },
  ]);

  const extensions: Extension[] = [
    lineNumbers(),
    history(),
    bracketMatching(),
    closeBrackets(),
    highlightActiveLine(),
    autocompletion(),
    syntaxHighlighting(defaultHighlightStyle, { fallback: true }),

    // The rest of what a text editor is expected to be. These were missing, and
    // their absence is invisible until someone reaches for one: without
    // `search` there is no Ctrl+F at all, and a SQL buffer long enough to need
    // scrolling is long enough to need finding.
    search({ top: true }),
    highlightSelectionMatches(),
    drawSelection(),
    dropCursor(),
    indentOnInput(),
    highlightSpecialChars(),
    rectangularSelection(),
    crosshairCursor(),
    EditorState.allowMultipleSelections.of(true),

    // Run keys come first so they win over the default keymap.
    runKeys,
    // Redo, spelled the way every editor spells it. `historyKeymap` binds
    // Ctrl+Shift+Z only on the platforms it recognises as Linux, and Ctrl+Y
    // only where it does not — so on any platform one of the two habits fails.
    // Binding both, unconditionally, costs nothing and surprises nobody.
    keymap.of([
      { key: "Mod-Shift-z", preventDefault: true, run: redo },
      { key: "Mod-y", preventDefault: true, run: redo },
    ]),
    keymap.of([
      ...defaultKeymap, ...historyKeymap, ...searchKeymap, ...completionKeymap, indentWithTab,
    ]),
    schemaCompartment.of(sql({ dialect: MySQL, upperCaseKeywords: true })),
    lintCompartment.of([]),
    themeCompartment.of(darkTheme),
    EditorView.lineWrapping,
    EditorView.updateListener.of((u) => {
      if (u.docChanged) hooks.onDocChanged?.();
    }),
  ];

  sharedExtensions = extensions;
  return new EditorView({ parent, state: createEditorState(STARTER_DOC) });
}

export const STARTER_DOC =
  "-- Ctrl+Enter runs the statement under the cursor (or the selection).\n" +
  "-- Ctrl+Shift+Enter runs the whole buffer.\n" +
  "-- Ctrl+T new tab · Ctrl+W close · Ctrl+Tab next\n" +
  "-- Ctrl+F find · Ctrl+Z undo · Ctrl+Shift+Z redo\n\n" +
  "SELECT 1;\n";

export type LintSource = (view: EditorView) => Promise<CmDiagnostic[]>;

/**
 * Turn advisory linting on or off.
 *
 * The linter has no veto: it contributes squiggles and gutter marks and
 * nothing else. Run is never disabled by a diagnostic.
 */
export function setLinting(view: EditorView, source: LintSource | null, delay = 300) {
  view.dispatch({
    effects: lintCompartment.reconfigure(
      source ? [linter(source, { delay }), lintGutter()] : [],
    ),
  });
}

/**
 * Re-run the linter now, without waiting for the next edit.
 *
 * Needed because linting is schema-aware: when the sidebar loads a table's
 * columns, diagnostics computed against the previously-empty cache are stale
 * and would keep showing "unknown table" until the user typed something.
 */
export function refreshLint(view: EditorView) {
  forceLinting(view);
}

/** Feed the schema cache into autocomplete as tables load in the sidebar. */
export function setSchema(view: EditorView, schema: SQLNamespace, defaultTable?: string) {
  view.dispatch({
    effects: schemaCompartment.reconfigure(
      sql({ dialect: MySQL, upperCaseKeywords: true, schema, defaultTable }),
    ),
  });
}

/** Selected text, or null when the selection is empty/whitespace. */
export function selectedText(view: EditorView): string | null {
  const { from, to } = view.state.selection.main;
  if (from === to) return null;
  const text = view.state.sliceDoc(from, to);
  return text.trim() ? text : null;
}

export function docText(view: EditorView): string {
  return view.state.doc.toString();
}

/**
 * Cursor position as a BYTE offset. The Rust splitter reports byte offsets,
 * and CodeMirror counts UTF-16 code units — they diverge the moment a non-ASCII
 * character appears in the buffer, so this conversion is mandatory, not
 * cosmetic.
 */
export function cursorByteOffset(view: EditorView): number {
  const head = view.state.selection.main.head;
  const prefix = view.state.sliceDoc(0, head);
  return new TextEncoder().encode(prefix).length;
}

export function insertAtCursor(view: EditorView, text: string) {
  const { from, to } = view.state.selection.main;
  view.dispatch({
    changes: { from, to, insert: text },
    selection: { anchor: from + text.length },
  });
  view.focus();
}

/**
 * Tell CodeMirror which way the theme went.
 *
 * Only the `dark` facet actually changes — the colours are tokens and follow on
 * their own — but that facet decides how tooltips and panels are drawn, and a
 * light-mode lint tooltip drawn dark is the sort of thing nobody reports and
 * everybody notices.
 */
export function setEditorTheme(view: EditorView, dark: boolean) {
  view.dispatch({
    effects: themeCompartment.reconfigure(dark ? darkTheme : lightTheme),
  });
}
