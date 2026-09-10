// CodeMirror 6 setup: MySQL highlighting, schema-aware autocomplete, and the
// run keybindings. Statement boundaries are never computed here — the editor
// asks the Rust splitter, so execution and cursor detection cannot disagree.

import { EditorState, type Extension } from "@codemirror/state";
import {
  EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection, dropCursor,
  rectangularSelection, crosshairCursor, highlightSpecialChars,
} from "@codemirror/view";
import {
  defaultKeymap, history, historyKeymap, indentWithTab, isolateHistory, redo,
} from "@codemirror/commands";
import {
  autocompletion, acceptCompletion, completionKeymap, closeBrackets,
} from "@codemirror/autocomplete";
import { search, searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import {
  bracketMatching, syntaxHighlighting, HighlightStyle, indentOnInput,
} from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { sql, MySQL, StandardSQL, type SQLDialect, type SQLNamespace } from "@codemirror/lang-sql";
import {
  forceLinting, linter, lintGutter, type Diagnostic as CmDiagnostic,
} from "@codemirror/lint";
import { Compartment } from "@codemirror/state";

const schemaCompartment = new Compartment();

/**
 * The SQL configuration currently in force, remembered because the two things
 * that change it arrive separately: the schema loads when the tree is expanded,
 * and the dialect changes when the active tab does. Reconfiguring for one must
 * not throw away the other.
 */
let sqlConfig: { dialect: SQLDialect; schema?: SQLNamespace; defaultTable?: string } = {
  dialect: MySQL,
};

/**
 * Which CodeMirror dialect an engine's name maps to.
 *
 * `StandardSQL` for anything that is not MySQL, because the difference that
 * actually shows is quoting: MySQL highlights `` `backticks` `` as identifiers
 * and Elasticsearch uses `"double quotes"`, which MySQL's dialect reads as a
 * string. The tab has carried a `dialect` field since Stage 1 — it was written
 * to the session file and restored, and then never read by anything.
 */
export function dialectFor(name: string | null | undefined): SQLDialect {
  return name === "mysql" || name === undefined || name === null ? MySQL : StandardSQL;
}
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
/**
 * Syntax colours, as CSS variables.
 *
 * This replaces CodeMirror's `defaultHighlightStyle`, whose colours are fixed
 * and **written for a light background**. They were being used on the dark
 * theme too, where — measured, not guessed — keywords came out at 1.81:1
 * against `--bg`, comments at 2.69 and numbers at 2.54. The dark theme's editor
 * had never actually been legible; nothing said so because the app's own chrome
 * followed the theme correctly and the text merely looked dim.
 *
 * Every colour is a token, for the same reason every colour in `styles.css` is:
 * a literal is a colour that cannot follow the theme. The pleasant consequence
 * is that an editor theme becomes a block of custom properties and no code at
 * all — see `--syn-*` and `data-editor-theme` in `styles.css`.
 *
 * The tags are the ones `@codemirror/lang-sql` actually emits, read from the
 * package rather than guessed: anything it never produces would be a rule that
 * looks like it works, which is a mistake this file has already made once.
 */
const appHighlightStyle = HighlightStyle.define([
  { tag: t.keyword, color: "var(--syn-keyword)" },
  { tag: [t.string, t.special(t.string)], color: "var(--syn-string)" },
  { tag: [t.number, t.bool, t.null], color: "var(--syn-number)" },
  { tag: [t.lineComment, t.blockComment], color: "var(--syn-comment)", fontStyle: "italic" },
  { tag: [t.name, t.special(t.name)], color: "var(--syn-name)" },
  { tag: t.typeName, color: "var(--syn-type)" },
  // Standard function names — `COUNT`, `NOW` — which the SQL grammar tags as a
  // "standard" name rather than as a function.
  { tag: t.standard(t.name), color: "var(--syn-function)" },
  {
    tag: [t.operator, t.punctuation, t.brace, t.paren, t.squareBracket],
    color: "var(--syn-punct)",
  },
]);

const themeRules = {
  "&": { height: "100%", backgroundColor: "var(--bg)", color: "var(--fg)" },
  // Currently inert, and kept deliberately: `drawSelection` forces
  // `caret-color: transparent !important` on `.cm-content` and paints the
  // caret itself as `.cm-cursor`, whose colour comes from CodeMirror's base
  // theme (black / #ddd, both of which are right here — measured). This is the
  // fallback if `drawSelection` is ever removed.
  ".cm-content": { caretColor: "var(--fg)" },
  ".cm-gutters": {
    backgroundColor: "var(--bg-raised)",
    color: "var(--fg-dim)",
    border: "none",
  },
  // Translucent, and it must stay that way: the selection layer is painted
  // *behind* the lines, so an opaque active line hides the selection on the one
  // line a short selection is always on. See --editor-active-line.
  ".cm-activeLine": { backgroundColor: "var(--editor-active-line)" },
  ".cm-activeLineGutter": { backgroundColor: "var(--chip-bg)" },
  // Selection, at the specificity CodeMirror's own base theme uses.
  //
  // This was written as `&.cm-focused .cm-selectionBackground` and **never
  // applied**: the base theme's rule is
  // `&dark.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground`
  // — five classes to our three — so it won every time, and a focused selection
  // was drawn in its default `#233`. On the light theme that is a visible (if
  // wrong) lavender; on the dark theme it is #233 on a #16181d background,
  // which is to say invisible. Select-all looked like it did nothing.
  //
  // Matching the selector shape exactly is what fixes it, and the reason the
  // long form is written out rather than tidied: it is not decoration, it is
  // the specificity. `selection_is_visible_in_both_themes` measures the result.
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground": {
    backgroundColor: "var(--sel-editor-bg)",
  },
  // The editor is not focused — clicking into the grid, say. Two classes,
  // because the base theme's blurred rule (`&dark .cm-selectionBackground`)
  // has two. Dimmer, so it reads as "this was selected" rather than "this is".
  "& .cm-selectionBackground": { backgroundColor: "var(--sel-editor-bg-blur)" },
} as const;

const darkTheme = EditorView.theme(themeRules, { dark: true });
const lightTheme = EditorView.theme(themeRules, { dark: false });
const themeCompartment = new Compartment();

export interface EditorHooks {
  onRunStatement: () => void;
  onRunAll: () => void;
  /** Lay the buffer — or the selection — out. Never runs anything. */
  onFormat: () => void;
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
    // What every other editor calls Format Document. `Mod-Shift-f` is free:
    // CodeMirror's search keymap binds Mod-f, Mod-g, Mod-d and Mod-Shift-l,
    // and nothing here binds this.
    { key: "Mod-Shift-f", preventDefault: true, run: () => (hooks.onFormat(), true) },
  ]);

  /**
   * **Tab accepts a completion. Enter never does.**
   *
   * CodeMirror's `completionKeymap` puts `acceptCompletion` on Enter and binds
   * nothing to Tab. In a SQL buffer that is the wrong way round: the popup
   * opens on its own while you type, so Enter — the key you press to start the
   * next line — silently becomes "accept whatever is highlighted", and you get
   * an identifier you never chose instead of a newline. Tab is the key every
   * other editor uses for this, and it is not the key anyone presses to mean
   * something else here.
   *
   * `acceptCompletion` **returns false when no completion is open**, so Tab
   * falls through to `indentWithTab` in the keymap below and still indents.
   * That is also why this sits before that one: earlier is higher precedence.
   *
   * `autocompletion({ defaultKeymap: false })` is what stops CodeMirror adding
   * Enter back — its own binding is registered at `Prec.highest`, so filtering
   * the array alone would not have been enough.
   */
  const completionKeys = keymap.of([
    { key: "Tab", run: acceptCompletion },
    // Everything else the popup needs — Escape to dismiss, arrows to move —
    // kept exactly as CodeMirror ships it, minus the one binding above.
    ...completionKeymap.filter((b) => b.key !== "Enter"),
  ]);

  const extensions: Extension[] = [
    lineNumbers(),
    history(),
    bracketMatching(),
    closeBrackets(),
    highlightActiveLine(),
    autocompletion({ defaultKeymap: false }),
    syntaxHighlighting(appHighlightStyle, { fallback: true }),

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
    completionKeys,
    // Redo, spelled the way every editor spells it. `historyKeymap` binds
    // Ctrl+Shift+Z only on the platforms it recognises as Linux, and Ctrl+Y
    // only where it does not — so on any platform one of the two habits fails.
    // Binding both, unconditionally, costs nothing and surprises nobody.
    keymap.of([
      { key: "Mod-Shift-z", preventDefault: true, run: redo },
      { key: "Mod-y", preventDefault: true, run: redo },
    ]),
    keymap.of([
      ...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab,
    ]),
    schemaCompartment.of(sql({ ...sqlConfig, upperCaseKeywords: true })),
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
  "-- Ctrl+F find · Ctrl+Shift+F format · Ctrl+Z undo · Ctrl+Shift+Z redo\n" +
  "-- Tab accepts a suggestion · Esc dismisses it\n\n" +
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
  sqlConfig = { ...sqlConfig, schema, defaultTable };
  applySqlConfig(view);
}

/**
 * Highlight this tab's SQL the way its engine writes it.
 *
 * Called on tab activation, so switching between a MySQL tab and a cluster tab
 * changes the quoting rules with it.
 */
export function setDialect(view: EditorView, name: string | null | undefined) {
  const dialect = dialectFor(name);
  if (dialect === sqlConfig.dialect) return;
  sqlConfig = { ...sqlConfig, dialect };
  applySqlConfig(view);
}

function applySqlConfig(view: EditorView) {
  view.dispatch({
    effects: schemaCompartment.reconfigure(sql({ ...sqlConfig, upperCaseKeywords: true })),
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

/**
 * Replace a range as **one undo step of its own**.
 *
 * `isolateHistory` is the whole point. CodeMirror groups nearby edits by time,
 * so a reformat arriving within half a second of the last keystroke merges into
 * that typing — and Ctrl+Z then throws away both, which for an action people
 * reach for speculatively is the wrong answer twice over. Isolating it makes
 * "one undo puts it back" true, and a test says so.
 */
export function replaceRange(
  view: EditorView,
  range: { from: number; to: number; insert: string; cursor: number },
) {
  view.dispatch({
    changes: { from: range.from, to: range.to, insert: range.insert },
    selection: { anchor: range.cursor },
    annotations: isolateHistory.of("full"),
  });
  view.focus();
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
