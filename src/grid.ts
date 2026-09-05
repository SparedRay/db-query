// Virtualized result grid + one tab per statement result.
//
// Hand-rolled virtualization rather than a library: it is ~80 lines, it keeps
// the dependency surface (and the licence audit) small, and the only thing we
// need is "render the visible window of a flat row list".

import type { CellValue, ColumnMeta, ScriptResult, StatementResult } from "./api";

const ROW_H = 22;
const OVERSCAN = 12;
const DEFAULT_COL_W = 150;

export class ResultView {
  private tabsEl: HTMLElement;
  private gridEl: HTMLElement;
  private statusEl: HTMLElement;
  private result: ScriptResult | null = null;
  private active = 0;
  private widths = new Map<string, number>();
  private onScroll = () => this.paintRows();

  constructor(tabs: HTMLElement, grid: HTMLElement, status: HTMLElement) {
    this.tabsEl = tabs;
    this.gridEl = grid;
    this.statusEl = status;
    this.gridEl.addEventListener("scroll", this.onScroll, { passive: true });
  }

  setMessage(html: string) {
    this.result = null;
    this.tabsEl.replaceChildren();
    this.gridEl.replaceChildren(el("div", "empty", html));
    this.statusEl.replaceChildren();
  }

  show(result: ScriptResult) {
    this.result = result;
    this.widths.clear();
    // Focus the statement that failed if there was one, else the last result.
    this.active = result.abortedAt ?? Math.max(0, result.statements.length - 1);
    this.renderTabs();
    this.renderActive();
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
      t.onclick = () => { this.active = i; this.renderTabs(); this.renderActive(); };
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
    cols.forEach((c) => {
      const cg = document.createElement("col");
      cg.style.width = `${this.widths.get(c.name) ?? DEFAULT_COL_W}px`;
      colgroup.append(cg);
    });
    table.append(colgroup);

    const thead = document.createElement("thead");
    const tr = document.createElement("tr");
    cols.forEach((c, i) => {
      const th = document.createElement("th");
      th.textContent = c.name;
      th.title = `${c.name} — ${c.sqlType}`;
      th.style.position = "sticky";
      const handle = el("span", "resize");
      handle.onmousedown = (e) => this.startResize(e, colgroup, i, c.name);
      th.append(handle);
      tr.append(th);
    });
    thead.append(tr);
    table.append(thead);

    const tbody = document.createElement("tbody");
    table.append(tbody);
    this.bodyEl = tbody;

    // Spacer rows above and below the window hold the full scroll height,
    // so only the visible slice of <tr>s ever exists in the DOM.
    this.gridEl.replaceChildren(table);
    this.gridEl.scrollTop = 0;
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
      const row = this.rows[i];
      for (let c = 0; c < this.cols.length; c++) {
        const td = document.createElement("td");
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
  td.colSpan = cols;
  td.style.padding = "0";
  td.style.border = "none";
  tr.append(td);
  return tr;
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
