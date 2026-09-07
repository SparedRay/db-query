// Virtualized result grid + one tab per statement result.
//
// Hand-rolled virtualization rather than a library: it is ~80 lines, it keeps
// the dependency surface (and the licence audit) small, and the only thing we
// need is "render the visible window of a flat row list".

import type { CellValue, ColumnMeta, ScriptResult, StatementResult } from "./api";
import { showValue } from "./dialog";
import { contextMenu } from "./menu";

const ROW_H = 22;
const OVERSCAN = 12;
const DEFAULT_COL_W = 150;

/**
 * What the view needs to render one script tab's results.
 *
 * The view owns no result state of its own: everything lives on the tab, so
 * switching away and back restores the grid, the selected statement, the
 * column widths and the scroll position exactly as they were left.
 */
/**
 * Rows and columns, intersected.
 *
 * Two sets rather than a list of cells, because it makes every case the user
 * actually wants fall out of one rule — **empty means "all"**:
 *
 *   * columns set, rows empty  → whole columns
 *   * rows set, columns empty  → whole rows
 *   * both set                 → the block where they cross
 *   * both empty               → the entire result
 *
 * It cannot express a ragged, non-rectangular selection. That is a feature: a
 * copy of one is not pasteable into anything, so there is nothing to lose.
 */
export interface GridSelection {
  rows: Set<number>;
  cols: Set<number>;
}

export const emptySelection = (): GridSelection => ({ rows: new Set(), cols: new Set() });

export interface ResultsFor {
  result: ScriptResult;
  activeIndex: number;
  widths: Map<string, number>;
  scrollTop: number;
  /**
   * What is selected, owned by the tab so it survives a tab switch exactly as
   * widths and scroll position already do.
   */
  selection: GridSelection;
  onSelect: (index: number) => void;
  onScrolled: (scrollTop: number) => void;
  onSelectionChanged: () => void;
}

export class ResultView {
  private tabsEl: HTMLElement;
  private gridEl: HTMLElement;
  private statusEl: HTMLElement;
  private result: ScriptResult | null = null;
  private active = 0;
  private widths = new Map<string, number>();
  private onSelect: (index: number) => void = () => {};
  private onScrolled: (scrollTop: number) => void = () => {};
  private selection: GridSelection = emptySelection();
  private onSelectionChanged: () => void = () => {};
  /** Anchors for shift-click ranges. */
  private lastCol: number | null = null;
  private lastRow: number | null = null;
  /** True while a drag across row numbers is in progress. */
  private dragging = false;
  private onScroll = () => {
    this.paintRows();
    this.onScrolled(this.gridEl.scrollTop);
  };

  /**
   * `onChanged` fires whenever what is displayed changes at all — a result
   * arriving, or a message replacing one. The per-call `onSelectionChanged`
   * cannot cover this: `setMessage` resets those hooks, so the one path that
   * turns exportable data into *no* exportable data was the one path that
   * reported nothing.
   */
  constructor(
    tabs: HTMLElement,
    grid: HTMLElement,
    status: HTMLElement,
    private onChanged: () => void = () => {},
  ) {
    this.tabsEl = tabs;
    this.gridEl = grid;
    this.statusEl = status;
    this.gridEl.addEventListener("scroll", this.onScroll, { passive: true });
  }

  setMessage(html: string) {
    this.result = null;
    this.onSelect = () => {};
    this.onScrolled = () => {};
    this.selection = emptySelection();
    this.tabsEl.replaceChildren();
    this.gridEl.replaceChildren(el("div", "empty", html));
    this.statusEl.replaceChildren();
    this.onChanged();
  }

  /** Render one tab's results, restoring exactly what it had before. */
  show(opts: ResultsFor) {
    this.onSelect = opts.onSelect;
    this.onScrolled = opts.onScrolled;
    this.onSelectionChanged = opts.onSelectionChanged;
    this.widths = opts.widths;
    this.selection = opts.selection;

    this.result = opts.result;
    this.active = Math.min(
      Math.max(0, opts.activeIndex),
      Math.max(0, opts.result.statements.length - 1),
    );
    this.renderTabs();
    this.renderActive();
    // Restore scroll after layout exists.
    this.gridEl.scrollTop = opts.scrollTop;
    this.paintRows();
    this.onChanged();
  }

  /** Index the UI should select for a freshly-arrived result. */
  static initialIndex(result: ScriptResult): number {
    // The statement that failed if there was one, else the last result.
    return result.abortedAt ?? Math.max(0, result.statements.length - 1);
  }

  private renderTabs() {
    const r = this.result;
    if (!r) return;
    // A single statement is the common case; tabs would just be noise.
    if (r.statements.length <= 1) {
      this.tabsEl.replaceChildren();
      return;
    }
    const nodes = r.statements.map((s, i) => {
      const t = el("button", "tab");
      t.textContent = `${i + 1} · ${firstLine(s.sql)}`;
      const badge = el("span", "badge");
      if (s.outcome.type === "rows") {
        badge.textContent = `${s.outcome.rows.length}`;
        if (s.outcome.truncated) t.classList.add("truncated");
      } else if (s.outcome.type === "affected") {
        badge.textContent = `${s.outcome.rows} affected`;
      } else {
        badge.textContent = "✕";
        t.classList.add("errored");
      }
      t.append(badge);
      if (i === this.active) t.classList.add("active");
      t.onclick = () => {
        this.active = i;
        // A different statement has different rows and columns, so indices from
        // the old one would highlight the wrong things.
        this.selection.rows.clear();
        this.selection.cols.clear();
        this.lastRow = null;
        this.lastCol = null;
        // Render *before* telling anyone, or the listeners read a grid that
        // still belongs to the statement we just left.
        this.renderTabs();
        this.renderActive();
        this.onSelect(i);
        this.onSelectionChanged();
      };
      return t;
    });
    this.tabsEl.replaceChildren(...nodes);
  }

  private renderActive() {
    const r = this.result;
    const s = r?.statements[this.active];
    if (!r || !s) { this.setMessage("No results."); return; }

    if (s.outcome.type === "error") {
      this.gridEl.replaceChildren(el("div", "err-box", s.outcome.message));
    } else if (s.outcome.type === "affected") {
      this.gridEl.replaceChildren(el("div", "empty", `${s.outcome.rows} row(s) affected.`));
    } else {
      this.buildTable(s.outcome.columns, s.outcome.rows);
    }
    this.renderStatus(r, s);
  }

  private cols: ColumnMeta[] = [];
  private rows: CellValue[][] = [];
  private bodyEl: HTMLElement | null = null;
  private headEl: HTMLElement | null = null;

  private buildTable(cols: ColumnMeta[], rows: CellValue[][]) {
    this.cols = cols;
    this.rows = rows;
    if (!cols.length) {
      this.gridEl.replaceChildren(el("div", "empty", "Query returned no columns."));
      return;
    }

    const table = document.createElement("table");
    table.className = "rs";

    const colgroup = document.createElement("colgroup");
    // The row-number gutter. Its width scales with the digits it has to hold,
    // so a 7500-row result does not clip its own numbers.
    const gutter = document.createElement("col");
    gutter.style.width = `${28 + String(rows.length).length * 7}px`;
    colgroup.append(gutter);
    cols.forEach((c) => {
      const cg = document.createElement("col");
      // Widths come from the tab, so a drag survives a tab switch.
      cg.style.width = `${this.widths.get(c.name) ?? DEFAULT_COL_W}px`;
      colgroup.append(cg);
    });
    table.append(colgroup);

    const thead = document.createElement("thead");
    const tr = document.createElement("tr");

    // The corner cell selects everything, the way a spreadsheet's does.
    const corner = document.createElement("th");
    corner.className = "rownum-h";
    corner.title = "Select every row and column";
    corner.onclick = () => this.selectAll();
    tr.append(corner);

    cols.forEach((c, i) => {
      const th = document.createElement("th");
      th.textContent = c.name;
      th.title = `${c.name} — ${c.sqlType}\nClick to select the column; shift-click for a range, ctrl-click to toggle.`;
      th.style.position = "sticky";
      th.dataset.col = String(i);
      th.onclick = (e) => this.clickColumn(e, i);
      const handle = el("span", "resize");
      // The resize grip lives inside the header, so without this a drag also
      // selects the column.
      handle.onclick = (e) => e.stopPropagation();
      handle.onmousedown = (e) => this.startResize(e, colgroup, i, c.name);
      th.append(handle);
      tr.append(th);
    });
    thead.append(tr);
    table.append(thead);
    this.headEl = tr;
    this.paintSelection();

    const tbody = document.createElement("tbody");
    table.append(tbody);
    this.bodyEl = tbody;

    // Spacer rows above and below the window hold the full scroll height,
    // so only the visible slice of <tr>s ever exists in the DOM.
    this.gridEl.replaceChildren(table);
    this.paintRows();
  }

  private paintRows() {
    const tbody = this.bodyEl;
    if (!tbody || !this.cols.length) return;
    const total = this.rows.length;
    const viewport = this.gridEl.clientHeight;
    const first = Math.max(0, Math.floor(this.gridEl.scrollTop / ROW_H) - OVERSCAN);
    const visible = Math.ceil(viewport / ROW_H) + OVERSCAN * 2;
    const last = Math.min(total, first + visible);

    const frag = document.createDocumentFragment();
    frag.append(spacer(first * ROW_H, this.cols.length));
    for (let i = first; i < last; i++) {
      const tr = document.createElement("tr");
      tr.style.height = `${ROW_H}px`;
      tr.dataset.row = String(i);

      // The row-number gutter is the affordance for selecting rows at all —
      // without something to aim at, "select these five rows" has no gesture.
      const num = document.createElement("th");
      num.className = "rownum";
      num.textContent = String(i + 1);
      num.title = "Click to select the row; shift-click for a range, ctrl-click to toggle.";
      num.onmousedown = (e) => this.startRowDrag(e, i);
      num.onmouseenter = () => this.dragOverRow(i);
      tr.append(num);

      const row = this.rows[i];
      for (let c = 0; c < this.cols.length; c++) {
        const td = document.createElement("td");
        td.dataset.col = String(c);
        td.onclick = (e) => this.clickCell(e, i, c);
        td.oncontextmenu = (e) => this.cellMenu(e, i, c);
        const v = row[c];
        if (v === null) {
          td.className = "null";
          td.textContent = "NULL";
        } else if (typeof v === "number") {
          td.className = "numeric";
          td.textContent = String(v);
        } else if (typeof v === "boolean") {
          td.className = "numeric";
          td.textContent = v ? "1" : "0";
        } else {
          // Numeric-hinted columns arrive as strings when precision matters
          // (DECIMAL, BIGINT beyond 2^53). Align them like numbers anyway.
          if (this.cols[c].typeHint === "numeric") td.className = "numeric";
          td.textContent = v;
          td.title = v.length > 40 ? v : "";
        }
        tr.append(td);
      }
      frag.append(tr);
    }
    frag.append(spacer((total - last) * ROW_H, this.cols.length));
    tbody.replaceChildren(frag);
    // Rows are recreated on every scroll, so the highlight has to be reapplied
    // rather than set once when the column was clicked.
    this.paintSelection();
  }

  /**
   * Right-click a cell.
   *
   * Acts on the cell under the pointer and deliberately leaves the selection
   * alone: right-clicking to inspect one value should not throw away a
   * selection someone just built up in order to copy it.
   */
  private cellMenu(e: MouseEvent, row: number, col: number) {
    const v = this.rows[row]?.[col];
    if (v === undefined) return;
    const name = this.cols[col]?.name ?? `column ${col + 1}`;
    contextMenu(e, [
      {
        label: "Open in full view",
        run: () => showValue(`${name} \u00b7 row ${row + 1}`, cellText(v)),
      },
    ]);
  }

  /**
   * Apply the three standard gestures to a set: plain click replaces,
   * ctrl/cmd-click toggles, shift-click extends from the anchor.
   *
   * Shared by rows and columns because they behave identically — and because
   * two copies of this is how the two drift apart.
   */
  private applyGesture(
    set: Set<number>,
    i: number,
    anchor: number | null,
    e: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean },
  ): number | null {
    if (e.shiftKey && anchor !== null) {
      const [a, b] = [anchor, i].sort((x, y) => x - y);
      for (let k = a; k <= b; k++) set.add(k);
      return anchor;
    }
    if (e.ctrlKey || e.metaKey) {
      if (set.has(i)) set.delete(i);
      else set.add(i);
      return i;
    }
    // Clicking the only selected entry clears it, so there is a way back to
    // "nothing selected" without hunting for one.
    const onlyThis = set.size === 1 && set.has(i);
    set.clear();
    if (!onlyThis) set.add(i);
    return i;
  }

  /** A column header: selects whole columns, across every row. */
  private clickColumn(e: MouseEvent, i: number) {
    this.selection.rows.clear();
    this.lastCol = this.applyGesture(this.selection.cols, i, this.lastCol, e);
    this.settle();
  }

  /** A row number: selects whole rows, across every column. */
  private clickRow(e: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }, i: number) {
    this.selection.cols.clear();
    this.lastRow = this.applyGesture(this.selection.rows, i, this.lastRow, e);
    this.settle();
  }

  /**
   * A cell: one row and one column, so the intersection is that cell.
   * Shift-click extends both, which is how a block gets selected.
   */
  private clickCell(e: MouseEvent, row: number, col: number) {
    e.stopPropagation();
    if (e.shiftKey && this.lastRow !== null && this.lastCol !== null) {
      const [r1, r2] = [this.lastRow, row].sort((a, b) => a - b);
      const [c1, c2] = [this.lastCol, col].sort((a, b) => a - b);
      this.selection.rows.clear();
      this.selection.cols.clear();
      for (let r = r1; r <= r2; r++) this.selection.rows.add(r);
      for (let c = c1; c <= c2; c++) this.selection.cols.add(c);
    } else if (e.ctrlKey || e.metaKey) {
      // Adding one more row to a growing set is far more common than adding one
      // more cell, so ctrl-click on a cell extends the row set.
      this.selection.cols.clear();
      if (this.selection.rows.has(row)) this.selection.rows.delete(row);
      else this.selection.rows.add(row);
      this.lastRow = row;
    } else {
      this.selection.rows.clear();
      this.selection.cols.clear();
      this.selection.rows.add(row);
      this.selection.cols.add(col);
      this.lastRow = row;
      this.lastCol = col;
    }
    this.settle();
  }

  /**
   * Dragging down the row numbers, which is how anyone selects a contiguous
   * block without counting to shift-click the end of it.
   */
  private startRowDrag(e: MouseEvent, i: number) {
    // Let ctrl and shift keep their meanings; a drag is the plain gesture.
    if (e.button !== 0) return;
    e.preventDefault();
    this.clickRow(e, i);
    if (e.shiftKey || e.ctrlKey || e.metaKey) return;
    this.dragging = true;
    const stop = () => {
      this.dragging = false;
      document.removeEventListener("mouseup", stop);
    };
    document.addEventListener("mouseup", stop);
  }

  private dragOverRow(i: number) {
    if (!this.dragging || this.lastRow === null) return;
    const [a, b] = [this.lastRow, i].sort((x, y) => x - y);
    this.selection.rows.clear();
    for (let k = a; k <= b; k++) this.selection.rows.add(k);
    this.settle();
  }

  private settle() {
    this.paintSelection();
    this.onSelectionChanged();
  }

  /**
   * Fills both sets rather than clearing them.
   *
   * "Empty means all" is right for the *data* but wrong as feedback: clearing
   * would answer Ctrl+A by highlighting nothing. Filling means select-all looks
   * like what it did, and `isEverything` keeps the two states equivalent
   * everywhere it matters.
   */
  selectAll() {
    const g = this.activeGrid();
    if (!g) return;
    g.rows.forEach((_, i) => this.selection.rows.add(i));
    g.columns.forEach((_, i) => this.selection.cols.add(i));
    this.settle();
  }

  clearSelection() {
    this.selection.rows.clear();
    this.selection.cols.clear();
    this.lastRow = null;
    this.lastCol = null;
    this.settle();
  }

  /**
   * The active statement's grid — the source of truth for what the selection
   * can refer to.
   *
   * `this.cols` / `this.rows` are a *render* cache, and they lag: switching
   * statements clears the selection and notifies before the new table is
   * built. Sizing "everything is selected" from the cache therefore produced
   * indices into the previous statement, which threw on the way back to a
   * shorter result and left the tab click dead.
   */
  private activeGrid(): { columns: ColumnMeta[]; rows: CellValue[][] } | null {
    const o = this.result?.statements[this.active]?.outcome;
    return o?.type === "rows" ? o : null;
  }

  /** Selection indices in order, clamped to what the statement actually has. */
  private indices(sel: Set<number>, count: number): number[] {
    if (!sel.size) return Array.from({ length: count }, (_, i) => i);
    return [...sel].filter((i) => i < count).sort((a, b) => a - b);
  }

  /** Selected column indices in display order, or every column when none are. */
  selectedColumns(): number[] {
    return this.indices(this.selection.cols, this.activeGrid()?.columns.length ?? 0);
  }

  /** Selected row indices in display order, or every row when none are. */
  selectedRows(): number[] {
    return this.indices(this.selection.rows, this.activeGrid()?.rows.length ?? 0);
  }

  hasSelection(): boolean {
    return this.selection.rows.size > 0 || this.selection.cols.size > 0;
  }

  /**
   * Is the selection equivalent to the whole result?
   *
   * True both when nothing is selected and when everything is, because those
   * mean the same thing — and the difference must not decide whether the
   * unbounded re-run is offered.
   */
  isEverything(): boolean {
    const g = this.activeGrid();
    const r = this.selection.rows;
    const c = this.selection.cols;
    return (
      (r.size === 0 || r.size === (g?.rows.length ?? 0)) &&
      (c.size === 0 || c.size === (g?.columns.length ?? 0))
    );
  }

  /** What is selected, for the status line: `null` where "all" applies. */
  selectionCounts(): { rows: number | null; cols: number | null } {
    if (this.isEverything()) return { rows: null, cols: null };
    return {
      rows: this.selection.rows.size || null,
      cols: this.selection.cols.size || null,
    };
  }

  /** The active statement's data, narrowed to the selection. */
  selectedData(): { columns: ColumnMeta[]; rows: CellValue[][]; truncated: boolean } | null {
    const s = this.result?.statements[this.active];
    if (!s) return null;
    const o = s.outcome;
    if (o.type !== "rows") return null;
    const cols = this.selectedColumns();
    const rows = this.selectedRows();
    return {
      columns: cols.map((i) => o.columns[i]),
      rows: rows.map((r) => cols.map((c) => o.rows[r][c])),
      // Only meaningful when every row is being exported. A hand-picked subset
      // was never going to be "the query" anyway, so calling it truncated would
      // be a warning about the wrong thing.
      truncated: o.truncated && rows.length === o.rows.length,
    };
  }

  /** The SQL behind the active statement, for the unbounded re-run. */
  activeStatementSql(): string | null {
    return this.result?.statements[this.active]?.sql ?? null;
  }

  /**
   * Repaint the highlight from the sets.
   *
   * Driven by `data-col` / `data-row` rather than DOM position, because the
   * row-number gutter makes those two disagree — and reapplied on every
   * repaint, because virtualised rows are recreated on each scroll.
   */
  private paintSelection() {
    const { rows, cols } = this.selection;
    const allRows = rows.size === 0;
    const allCols = cols.size === 0;

    this.headEl?.querySelectorAll<HTMLElement>("th[data-col]").forEach((th) => {
      th.classList.toggle("sel", cols.has(Number(th.dataset.col)));
    });
    this.bodyEl?.querySelectorAll<HTMLElement>("tr[data-row]").forEach((tr) => {
      const r = Number(tr.dataset.row);
      const rowSel = rows.has(r);
      tr.querySelector(".rownum")?.classList.toggle("sel", rowSel);
      tr.querySelectorAll<HTMLElement>("td[data-col]").forEach((td) => {
        const c = Number(td.dataset.col);
        // The intersection rule, applied one cell at a time.
        const on = (allRows || rowSel) && (allCols || cols.has(c));
        td.classList.toggle("sel", on && this.hasSelection());
      });
    });
  }

  private startResize(e: MouseEvent, colgroup: HTMLElement, idx: number, name: string) {
    e.preventDefault();
    e.stopPropagation();
    const col = colgroup.children[idx] as HTMLElement;
    const startX = e.clientX;
    const startW = col.getBoundingClientRect().width;
    const move = (ev: MouseEvent) => {
      const w = Math.max(48, startW + ev.clientX - startX);
      col.style.width = `${w}px`;
      this.widths.set(name, w);
    };
    const up = () => {
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", up);
    };
    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", up);
  }

  private renderStatus(r: ScriptResult, s: StatementResult) {
    const chips: HTMLElement[] = [];

    if (s.outcome.type === "rows") {
      const n = s.outcome.rows.length;
      chips.push(el("span", "chip", `${n} row${n === 1 ? "" : "s"}`));
      if (s.outcome.truncated) {
        chips.push(el("span", "chip warn", `truncated at ${n}`));
      }
    } else if (s.outcome.type === "affected") {
      chips.push(el("span", "chip", `${s.outcome.rows} affected`));
    } else {
      chips.push(el("span", "chip err", "error"));
    }

    chips.push(el("span", "chip", `${s.elapsedMs} ms`));

    // Auto-LIMIT rewrote the SQL. Saying so is the whole mitigation for
    // "why doesn't this count match?" — never render results without it.
    if (s.effectiveSql) {
      const chip = el("span", "chip warn", "LIMIT 5000 applied");
      chip.title = `Sent to the server:\n${s.effectiveSql}`;
      chips.push(chip);
    }

    if (r.delimiterDetected) {
      const chip = el("span", "chip warn", "DELIMITER — no row cap");
      chip.title = "The buffer was sent verbatim as one statement; auto-LIMIT does not apply.";
      chips.push(chip);
    }
    // A timeout and a user cancel both KILL the query, but they mean very
    // different things to whoever is reading the status bar.
    if (r.timedOut) {
      const chip = el("span", "chip err", "timed out");
      chip.title = "The safety-net timeout fired and the query was killed.";
      chips.push(chip);
    } else if (r.cancelled) {
      chips.push(el("span", "chip err", "cancelled"));
    }

    if (r.statements.length > 1) {
      const failed = r.abortedAt !== null ? `, aborted at ${r.abortedAt + 1}` : "";
      chips.push(
        el("span", "chip", `${r.statements.length} statements · ${r.totalElapsedMs} ms${failed}`),
      );
    }
    this.statusEl.replaceChildren(...chips);
  }
}

function spacer(height: number, cols: number): HTMLElement {
  const tr = document.createElement("tr");
  tr.style.height = `${Math.max(0, height)}px`;
  const td = document.createElement("td");
  // +1 for the row-number gutter, or the spacer pulls the columns out of line.
  td.colSpan = cols + 1;
  td.style.padding = "0";
  td.style.border = "none";
  tr.append(td);
  return tr;
}

/**
 * A cell as text, or `null` for a real SQL NULL.
 *
 * Matches what the grid paints, so the viewer never shows something the table
 * disagrees with.
 */
function cellText(v: CellValue): string | null {
  if (v === null) return null;
  if (typeof v === "boolean") return v ? "1" : "0";
  return String(v);
}

function el(tag: string, cls: string, text?: string): HTMLElement {
  const n = document.createElement(tag);
  n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
}

function firstLine(sql: string): string {
  const line = sql.split("\n").find((l) => l.trim().length) ?? sql;
  const t = line.trim();
  return t.length > 28 ? `${t.slice(0, 28)}…` : t;
}
