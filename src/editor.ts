// CodeMirror 6 setup: MySQL highlighting, schema-aware autocomplete, and the
// run keybindings. Statement boundaries are never computed here — the editor
// asks the Rust splitter, so execution and cursor detection cannot disagree.

import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, highlightActiveLine } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { autocompletion, completionKeymap, closeBrackets } from "@codemirror/autocomplete";
import { bracketMatching, syntaxHighlighting, defaultHighlightStyle } from "@codemirror/language";
import { sql, MySQL, type SQLNamespace } from "@codemirror/lang-sql";
import {
  forceLinting, linter, lintGutter, type Diagnostic as CmDiagnostic,
} from "@codemirror/lint";
import { Compartment } from "@codemirror/state";

const schemaCompartment = new Compartment();
const lintCompartment = new Compartment();

const theme = EditorView.theme(
  {
    "&": { height: "100%", backgroundColor: "#16181d", color: "#d7dae0" },
    ".cm-content": { caretColor: "#d7dae0" },
    ".cm-gutters": { backgroundColor: "#1d2027", color: "#5b6270", border: "none" },
    ".cm-activeLine": { backgroundColor: "#1b1e25" },
    ".cm-activeLineGutter": { backgroundColor: "#22262e" },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
      backgroundColor: "#2c4a7c",
    },
  },
  { dark: true },
);

export interface EditorHooks {
  onRunStatement: () => void;
  onRunAll: () => void;
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
    // Run keys come first so they win over the default keymap.
    runKeys,
    keymap.of([...defaultKeymap, ...historyKeymap, ...completionKeymap, indentWithTab]),
    schemaCompartment.of(sql({ dialect: MySQL, upperCaseKeywords: true })),
    lintCompartment.of([]),
    theme,
    EditorView.lineWrapping,
  ];

  return new EditorView({
    parent,
    state: EditorState.create({
      doc: "-- Ctrl+Enter runs the statement under the cursor (or the selection).\n-- Ctrl+Shift+Enter runs the whole buffer.\n\nSELECT 1;\n",
      extensions,
    }),
  });
}

export type LintSource = (view: EditorView) => Promise<CmDiagnostic[]>;

/**
 * Turn advisory linting on or off.
 *
 * The linter has no veto: it contributes squiggles and gutter marks and
 * nothing else. Run is never disabled by a diagnostic.
 */
export function setLinting(view: EditorView, source: LintSource | null) {
  view.dispatch({
    effects: lintCompartment.reconfigure(
      source ? [linter(source, { delay: 300 }), lintGutter()] : [],
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
