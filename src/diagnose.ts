// Stage 35. What a diagnosis looks like: the plan in execution order, what the
// statement cost the session, and a verdict on each of those facts.
//
// **Built from nodes, never from innerHTML.** The plan carries a predicate the
// server rendered from the user's SQL, and the findings quote table names; both
// are text from somewhere else, and a panel that concatenates them into markup
// is one quoted string away from executing it.
//
// Formats nothing it was given a number for: `diagnose.rs` decides what is a
// finding, this decides how a finding looks.

import type {
  DiagCounter,
  Finding,
  PlanNode,
  SkippedStatement,
  StatementDiagnosis,
} from "./api";

/** What the view needs from the app around it. */
export interface DiagnoseDeps {
  /** Put SQL in the editor — a suggestion is never run. */
  openInEditor: (sql: string) => void;
  /** Prefill the assistant with a question. */
  ask: (question: string) => void;
}

/** Longest bar in the self-time chart, in percent of the track. */
const BAR_MAX = 100;

/** Below this share of the measured time a node is not worth a bar. */
const BAR_FLOOR = 0.005;

/**
 * Draw every diagnosis in this tab into `host`.
 *
 * One statement is the usual case; Diagnose all produces several, and they
 * stack in the order they were measured.
 */
export function paintDiagnoses(
  host: HTMLElement,
  diagnoses: StatementDiagnosis[],
  skipped: SkippedStatement[],
  deps: DiagnoseDeps,
): void {
  host.replaceChildren();

  if (diagnoses.length === 0 && skipped.length === 0) {
    host.append(
      empty(
        "Nothing diagnosed yet.",
        "Turn on Diagnose and run a SELECT. The plan arrives first; the measured run follows it.",
      ),
    );
    return;
  }

  for (const d of diagnoses) host.append(card(d, deps));
  for (const s of skipped) host.append(skippedCard(s));
}

function empty(title: string, detail: string): HTMLElement {
  const box = div("diag-empty");
  box.append(el("p", "diag-empty-title", title), el("p", "diag-empty-detail", detail));
  return box;
}

/** One statement's whole diagnosis. */
function card(d: StatementDiagnosis, deps: DiagnoseDeps): HTMLElement {
  const box = div("diag-card");

  box.append(header(d));
  if (d.notes.length > 0) box.append(notes(d.notes));
  box.append(statement(d.statement));

  if (d.plan.length > 0) {
    box.append(planTable(d));
    const bars = selfTimeBars(d.plan);
    if (bars) box.append(bars);
  } else if (d.classic.length > 0) {
    box.append(classicTable(d));
  }

  if (d.counters.length > 0) box.append(counters(d.counters));
  box.append(findings(d.findings, deps));
  box.append(handoff(d, deps));

  return box;
}

/**
 * The headline chips.
 *
 * **The source is a chip, not a footnote.** "Measured" and "estimated" are
 * different claims, and a panel that shows timings without saying which it has
 * is inviting someone to act on a guess.
 */
function header(d: StatementDiagnosis): HTMLElement {
  const row = div("diag-head");

  const source = chip(
    d.source === "analyze"
      ? `Measured${d.measuredMs === null ? "" : ` · ${duration(d.measuredMs)}`}`
      : d.source === "tree"
        ? "Estimated — not executed"
        : "The server's own EXPLAIN table",
    d.source === "analyze" ? "measured" : "estimated",
  );
  row.append(source);

  const examined = d.counters.find((c) => c.name === "Handler_read_rnd_next");
  if (examined) row.append(chip(`${compact(examined.delta)} rows walked in table order`));
  if (d.rowsSent !== null) row.append(chip(`${compact(d.rowsSent)} rows out`));

  const worst = d.plan
    .map((n) => (n.estRows && n.actualRows ? n.actualRows / n.estRows : 1))
    .reduce((a, b) => Math.max(a, b), 1);
  if (worst >= 10) row.append(chip(`estimate off by ${Math.round(worst)}×`, "warn"));

  return row;
}

function notes(lines: string[]): HTMLElement {
  const box = div("diag-notes");
  for (const line of lines) box.append(el("p", "diag-note", line));
  return box;
}

function statement(sql: string): HTMLElement {
  const pre = el("pre", "diag-sql", sql);
  pre.tabIndex = 0;
  return pre;
}

// ------------------------------------------------------------- the plan

/**
 * The plan, one row per iterator, indented by depth.
 *
 * `est → actual` share a column because **the gap between them is the
 * finding**; two columns make the reader do the division.
 */
function planTable(d: StatementDiagnosis): HTMLElement {
  const box = div("diag-plan");
  box.append(
    planHead(
      d.source === "analyze"
        ? ["Plan, in execution order", "Self time", "Rows: est → actual", "Loops"]
        : ["Plan, in execution order", "", "Rows (estimate)", ""],
    ),
  );

  for (const n of d.plan) {
    const row = div("diag-node");
    row.style.setProperty("--depth", String(Math.min(n.depth, 12)));
    if (isHot(n, d)) row.classList.add("hot");

    const label = div("diag-op");
    label.append(el("span", "diag-op-name", n.op));
    if (n.detail) label.append(el("span", "diag-op-detail", n.detail));
    if (n.neverExecuted) label.append(el("span", "diag-never", "never executed"));
    row.append(label);

    row.append(figure(n.selfMs === null ? "" : duration(n.selfMs), n.selfMs !== null && n.selfMs >= 50));
    row.append(rowsCell(n));
    row.append(figure(n.loops === null ? "" : compact(n.loops), (n.loops ?? 0) >= 1000));

    box.append(row);
  }
  return box;
}

function planHead(labels: string[]): HTMLElement {
  const head = div("diag-plan-head");
  for (const l of labels) head.append(el("span", "", l));
  return head;
}

/** `11,920 → 118,944`, or just the estimate when nothing ran. */
function rowsCell(n: PlanNode): HTMLElement {
  const cell = div("diag-figure diag-rows");
  if (n.estRows === null && n.actualRows === null) return cell;
  if (n.actualRows === null) {
    cell.append(el("span", "", compact(n.estRows ?? 0)));
    return cell;
  }
  cell.append(el("span", "diag-est", compact(n.estRows ?? 0)));
  cell.append(el("span", "diag-arrow", "→"));
  const off = n.estRows && n.estRows > 0 ? n.actualRows / n.estRows : 1;
  cell.append(el("span", off >= 10 ? "diag-actual bad" : "diag-actual", compact(n.actualRows)));
  return cell;
}

function figure(text: string, bad: boolean): HTMLElement {
  return el("span", bad ? "diag-figure bad" : "diag-figure", text);
}

/** A node worth the eye: it cost real time, or its estimate was far out. */
function isHot(n: PlanNode, d: StatementDiagnosis): boolean {
  const total = d.measuredMs ?? 0;
  if (n.selfMs !== null && total > 0 && n.selfMs / total >= 0.2) return true;
  return !!(n.estRows && n.actualRows && n.actualRows / n.estRows >= 10);
}

/**
 * Self time, ranked.
 *
 * The same form the performance dock uses for the same job: one accent, the
 * rest drained, bars to scale so the comparison is the drawing rather than the
 * arithmetic.
 */
function selfTimeBars(plan: PlanNode[]): HTMLElement | null {
  const timed = plan
    .filter((n) => n.selfMs !== null && n.selfMs > 0)
    .sort((a, b) => (b.selfMs ?? 0) - (a.selfMs ?? 0))
    .slice(0, 5);
  if (timed.length === 0) return null;

  const top = timed[0].selfMs ?? 1;
  const total = plan.reduce((sum, n) => sum + (n.selfMs ?? 0), 0);

  const box = div("diag-bars");
  box.append(el("p", "diag-cap", "Self time, children subtracted"));
  for (const [i, n] of timed.entries()) {
    const share = total > 0 ? (n.selfMs ?? 0) / total : 0;
    if (share < BAR_FLOOR && i > 0) continue;
    const row = div("diag-bar-row");
    row.append(el("span", "diag-bar-label", barLabel(n)));
    const track = div("diag-track");
    const fill = div(i === 0 ? "diag-fill" : "diag-fill quiet");
    fill.style.width = `${(((n.selfMs ?? 0) / top) * BAR_MAX).toFixed(1)}%`;
    track.append(fill);
    row.append(track, el("span", "diag-bar-value", duration(n.selfMs ?? 0)));
    box.append(row);
  }
  return box;
}

/** `Index lookup ×118,944` — the multiplier is why the node is on this chart. */
function barLabel(n: PlanNode): string {
  const loops = n.loops ?? 1;
  return loops > 1 ? `${n.op} ×${compact(loops)}` : n.op;
}

/** The classic tabular EXPLAIN, for a server with no tree. */
function classicTable(d: StatementDiagnosis): HTMLElement {
  const box = div("diag-classic");
  const table = document.createElement("table");
  const head = document.createElement("tr");
  for (const c of d.classicColumns) head.append(el("th", "", c));
  table.append(head);
  for (const row of d.classic) {
    const tr = document.createElement("tr");
    for (const cell of row) tr.append(el("td", "", cell ?? "—"));
    table.append(tr);
  }
  box.append(table);
  return box;
}

// ------------------------------------------------------------- counters

function counters(list: DiagCounter[]): HTMLElement {
  const box = div("diag-counters");
  for (const c of list) {
    const cell = div(c.notable ? "diag-counter notable" : "diag-counter");
    cell.append(el("span", "diag-counter-k", c.name));
    cell.append(el("span", "diag-counter-v", compact(c.delta)));
    const why = COUNTER_NOTES[c.name];
    if (why) cell.append(el("span", "diag-counter-why", why));
    box.append(cell);
  }
  return box;
}

/**
 * What each counter means, in one line.
 *
 * Here rather than in Rust: this is the only place it is read, and a sentence
 * about a counter is presentation the same way a column header is.
 */
const COUNTER_NOTES: Record<string, string> = {
  Handler_read_rnd_next: "Rows walked in table order — a scan's fingerprint.",
  Handler_read_next: "Rows walked along an index.",
  Handler_read_key: "Lookups by key.",
  Created_tmp_tables: "Intermediate results the server had to materialise.",
  Created_tmp_disk_tables: "One of those did not fit in memory.",
  Sort_merge_passes: "The sort was larger than sort_buffer_size.",
  Sort_rows: "Rows sorted.",
  Sort_scan: "Sorts fed by a scan rather than an index.",
  Select_scan: "Full scans the optimizer chose.",
  Select_full_join: "Joins with no usable index.",
  Select_range: "Index ranges used.",
};

// ------------------------------------------------------------- verdicts

function findings(list: Finding[], deps: DiagnoseDeps): HTMLElement {
  const box = div("diag-findings");
  for (const f of list) {
    const card = div(`diag-finding sev-${f.severity}`);
    card.append(el("h4", "diag-finding-title", f.title));
    card.append(el("p", "diag-finding-ev", f.evidence));
    if (f.suggestion) {
      card.append(el("pre", "diag-suggestion", f.suggestion));
      const open = document.createElement("button");
      open.type = "button";
      open.className = "mini";
      open.textContent = "Put it in the editor";
      // **Never run.** Every statement this app generates goes into the
      // editor and waits for the person, and an index on a large table is the
      // last place to break that habit.
      open.onclick = () => deps.openInEditor(f.suggestion ?? "");
      card.append(open);
    }
    box.append(card);
  }
  return box;
}

// ------------------------------------------------------------- handoff

/**
 * Hand it to the assistant.
 *
 * **Normalised by default.** The performance dock can send a digest because a
 * digest never held a value. This is the real statement, with the real values
 * in it, going to an external API — so what travels is the shape, and sending
 * it as written is a second, deliberate click that says what it will send.
 */
function handoff(d: StatementDiagnosis, deps: DiagnoseDeps): HTMLElement {
  const row = div("diag-actions");

  const ask = document.createElement("button");
  ask.type = "button";
  ask.className = "mini diag-ask";
  ask.textContent = "Ask the assistant";
  ask.title = "Sends the plan, the counters and the statement with its values replaced by ?";
  ask.onclick = () => deps.ask(question(d, false));
  row.append(ask);

  const raw = document.createElement("button");
  raw.type = "button";
  raw.className = "mini diag-ask-raw";
  raw.textContent = "…with the real values";
  raw.title = "Sends the statement exactly as written, values included";
  raw.onclick = () => deps.ask(question(d, true));
  row.append(raw);

  row.append(
    el(
      "span",
      "diag-actions-note",
      "The default replaces every literal with ?.",
    ),
  );
  return row;
}

/**
 * The question.
 *
 * Carries **evidence, not the verdict's conclusion**: the plan, the counters
 * and the measured times, so the model argues from what the server said. The
 * verdicts are named too, because disagreeing with one is a useful answer.
 */
export function question(d: StatementDiagnosis, asWritten: boolean): string {
  const lines: string[] = [];
  lines.push(
    asWritten
      ? "Here is a slow statement and what the server said about it."
      : "Here is a slow statement, with its literal values replaced by ?, and what the server said about it.",
  );
  lines.push("", asWritten ? d.statement : d.normalised, "");

  lines.push(d.source === "analyze" ? "Measured plan (EXPLAIN ANALYZE):" : "Plan (estimates only):");
  for (const n of d.plan) {
    const parts: string[] = [`${"  ".repeat(n.depth)}-> ${n.op}${n.detail ? `: ${n.detail}` : ""}`];
    const figs: string[] = [];
    if (n.estRows !== null) figs.push(`est rows=${Math.round(n.estRows)}`);
    if (n.actualRows !== null) figs.push(`actual rows=${Math.round(n.actualRows)}`);
    if (n.loops !== null) figs.push(`loops=${Math.round(n.loops)}`);
    if (n.selfMs !== null) figs.push(`self=${n.selfMs.toFixed(1)}ms`);
    if (figs.length > 0) parts.push(`  (${figs.join(", ")})`);
    lines.push(parts.join(""));
  }

  if (d.counters.length > 0) {
    lines.push("", "Session counters for that run:");
    for (const c of d.counters) lines.push(`  ${c.name} +${c.delta}`);
  }

  if (d.findings.length > 0) {
    lines.push("", "What this app concluded, which you are welcome to disagree with:");
    for (const f of d.findings) lines.push(`  - ${f.title}: ${f.evidence}`);
  }

  lines.push(
    "",
    "What would you change, and what evidence here supports it? If an index would help, say which columns and in which order, and say what it would cost on writes.",
  );
  return lines.join("\n");
}

// ------------------------------------------------------------- skipped

function skippedCard(s: SkippedStatement): HTMLElement {
  const box = div("diag-card diag-skipped");
  box.append(chipRow(chip("Not diagnosed", "warn")));
  box.append(statement(s.sql));
  box.append(el("p", "diag-finding-ev", s.why));
  return box;
}

function chipRow(...nodes: HTMLElement[]): HTMLElement {
  const row = div("diag-head");
  row.append(...nodes);
  return row;
}

// ------------------------------------------------------------- formatting

/** `842` → `842 ms`, `1842` → `1.8 s`. Read, not computed with. */
export function duration(ms: number): string {
  if (ms < 1) return "<1 ms";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)} s`;
}

/** `1204117` → `1.2M`. Thousands separators below ten thousand. */
export function compact(n: number): string {
  const v = Math.round(n);
  if (v < 10_000) return v.toLocaleString("en-US");
  if (v < 1_000_000) return `${(v / 1000).toFixed(v < 100_000 ? 1 : 0)}k`;
  if (v < 1_000_000_000) return `${(v / 1_000_000).toFixed(v < 100_000_000 ? 1 : 0)}M`;
  return `${(v / 1_000_000_000).toFixed(1)}B`;
}

function chip(text: string, tone = ""): HTMLElement {
  return el("span", tone ? `diag-chip ${tone}` : "diag-chip", text);
}

function div(cls: string): HTMLElement {
  const d = document.createElement("div");
  d.className = cls;
  return d;
}

function el(tag: string, cls: string, text = ""): HTMLElement {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text) node.textContent = text;
  return node;
}
