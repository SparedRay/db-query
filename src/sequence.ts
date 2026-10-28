/**
 * Stage 29. Sequence: what actually ran, as a log.
 *
 * The grid answers "what came back". It cannot answer "what did this script
 * do" — a `Run all` over seven statements shows one of them at a time, and the
 * two that changed rows and the one that failed are a tab strip away each. So
 * the output pane gets a second view over the same runs: one line per
 * statement, grouped under the run that issued it, saying its kind, its SQL,
 * what came back and how long it took.
 *
 * **The log is not a view of `tab.result`.** It would have been — a
 * `ScriptResult` already carries every field drawn here — but `closeStatement`
 * splices that array to release a result's rows, so a view of it would lose
 * its own history the moment somebody closed a result, and would only ever
 * hold the newest run. These records hold no rows, which is what makes keeping
 * several runs affordable (§3 of the stage doc: ~190 bytes each, against the
 * `CellValue[][]` that `clearResult` exists to release).
 *
 * Flyway is deliberately absent: its account of an Apply or a Repair is the
 * migrations panel's "Last run" (Stage 28, decision 4), and one run recorded
 * in two places is the confusion Stage 18 §5.3 removed.
 */
import type { ScriptResult, StatementKind, StatementResult } from "./api";

/**
 * Row height, uniform for every row including a run's header.
 *
 * Uniform is the point: it lets the list be windowed exactly the way the
 * grid's rows have been since Stage 12, which §3 of the stage doc says is not
 * optional — a plain list costs 50ms at a thousand entries and 320ms at five
 * thousand, and a view switch that hitches is a view switch nobody uses.
 */
export const SEQ_ROW_H = 26;
const OVERSCAN = 10;

/**
 * How many statements the log keeps, across all of a tab's runs.
 *
 * Measured: ~190 bytes per entry, so 2,000 is ~380KB — less than one
 * thousand-row result set. **A run is never split**: half of a run would make
 * its own summary ("5 statements · stopped at statement 5") a lie about what
 * is on screen, so the cap drops whole runs from the far end and the footer
 * says how many went.
 */
export const SEQ_CAP = 2000;

/**
 * What a statement did, as a shape rather than only a colour.
 *
 * Four states that must be told apart at a glance, and colour alone does not
 * do that — so a circle, a square, a ring and a diamond, each also differing
 * in lightness.
 */
export type EntryShape = "data" | "change" | "ok" | "error";

export interface SequenceEntry {
  /** `SELECT`, `UPDATE`, … — read off the SQL, because `kind` is a category. */
  verb: string;
  shape: EntryShape;
  /** As written. One line: the log is a list, not a reader. */
  sql: string;
  /** The whole thing, for the row's title, when it was more than one line. */
  fullSql: string;
  /**
   * What was actually sent, when the row ceiling rewrote it.
   *
   * Worth keeping because it is the honest answer to "why did this return
   * exactly a thousand rows" — but not worth a column, so it goes on the
   * title beside the SQL it replaced.
   */
  effectiveSql: string | null;
  /** `3 rows`, `2 rows affected`, `OK`, `Error 1054: …`. */
  outcome: string;
  /** The whole message when `outcome` is a one-line summary of a longer one. */
  fullOutcome: string;
  /**
   * Rows returned, or rows changed, or null for a statement that has neither.
   *
   * The sentence in `outcome` is for reading; this is for adding up. The footer
   * says "3 changed 7 rows", which cannot be recovered from the sentences
   * without parsing text this module wrote — and a log that has to parse
   * itself is a log that will disagree with itself.
   */
  rows: number | null;
  elapsedMs: number;
  /** Byte offsets within the text this run was given, for revealing it. */
  start: number;
  end: number;
  /** It returned rows, so a result tab exists for it while the run is current. */
  madeRows: boolean;
}

export interface SequenceRun {
  /** `Run`, `Run all`, `Assistant` — which gesture issued it. */
  what: string;
  at: number;
  /** `· 5 statements · stopped at statement 5 · 64 ms`. */
  summary: string;
  entries: SequenceEntry[];
  /** How far into the buffer `entries[].start` is measured from. */
  base: number;
}

export interface SequenceLog {
  /** Newest first: the run you just did is the one you want to read. */
  runs: SequenceRun[];
  /** Dropped by the cap, so the footer can say so rather than quietly lie. */
  droppedRuns: number;
  droppedEntries: number;
}

export function emptyLog(): SequenceLog {
  return { runs: [], droppedRuns: 0, droppedEntries: 0 };
}

/** Every entry in the log, newest run first, flattened. */
export function entryCount(log: SequenceLog): number {
  let n = 0;
  for (const r of log.runs) n += r.entries.length;
  return n;
}

/**
 * Add a run to the log, dropping the oldest ones past the cap.
 *
 * The newest run is kept whole however long it is — see `SEQ_CAP`. So the cap
 * bounds history, not the thing you are looking at.
 */
export function pushRun(log: SequenceLog, run: SequenceRun) {
  log.runs.unshift(run);
  while (log.runs.length > 1 && entryCount(log) > SEQ_CAP) {
    const gone = log.runs.pop();
    if (!gone) break;
    log.droppedRuns += 1;
    log.droppedEntries += gone.entries.length;
  }
}

/** Turn a finished run into a log entry. Nothing here reads the rows. */
export function runFrom(what: string, base: number, result: ScriptResult): SequenceRun {
  return {
    what,
    at: Date.now(),
    summary: summarise(result),
    base,
    entries: result.statements.map(entryFrom),
  };
}

function entryFrom(s: StatementResult): SequenceEntry {
  const o = s.outcome;
  const shape: EntryShape =
    o.type === "error"
      ? "error"
      : o.type === "rows"
        ? "data"
        : s.kind === "session"
          ? "ok"
          : "change";

  let outcome: string;
  let fullOutcome: string;
  if (o.type === "error") {
    fullOutcome = o.message;
    outcome = firstLine(o.message);
  } else if (o.type === "rows") {
    // The ceiling cut it short, and a bare count would read as the whole
    // answer — which is the one thing a log must not say.
    outcome = `${plural(o.rows.length, "row")}${o.truncated ? " (truncated)" : ""}`;
    fullOutcome = outcome;
  } else if (s.kind === "session") {
    // "0 rows affected" is true of a SET and says nothing, the same reason the
    // grid prints "Statement executed." for one.
    outcome = "OK";
    fullOutcome = outcome;
  } else {
    outcome = `${plural(o.rows, "row")} affected`;
    fullOutcome = outcome;
  }

  return {
    verb: verbOf(s.sql, s.kind),
    shape,
    sql: oneLine(s.sql),
    fullSql: s.sql,
    effectiveSql: s.effectiveSql,
    outcome,
    fullOutcome,
    rows: o.type === "rows" ? o.rows.length : o.type === "affected" ? o.rows : null,
    elapsedMs: s.elapsedMs,
    start: s.start,
    end: s.end,
    madeRows: o.type === "rows",
  };
}

function summarise(r: ScriptResult): string {
  const bits = [plural(r.statements.length, "statement")];
  if (r.cancelled) bits.push("cancelled");
  if (r.timedOut) bits.push("timed out");
  if (r.abortedAt !== null) bits.push(`stopped at statement ${r.abortedAt + 1}`);
  // Stage 12's warning, and it belongs in a log more than anywhere: a new
  // connection is a new session, so temporary tables and open transactions
  // from the statements above are gone.
  if (r.reconnected) bits.push("reconnected");
  bits.push(`${r.totalElapsedMs} ms`);
  return `· ${bits.join(" · ")}`;
}

function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

/** What the grid's own labels call each kind, when the SQL will not say. */
const KIND_LABEL: Record<StatementKind, string> = {
  select: "SELECT",
  rowReturning: "ROWS",
  modify: "MODIFY",
  session: "SESSION",
  other: "SQL",
};

/**
 * The statement's verb, off the front of the SQL.
 *
 * `kind` is a category — `modify` covers INSERT, UPDATE and DELETE, which are
 * three different things to find in a log — so the verb is read from the text,
 * past any leading comment or bracket. When the text will not give one (it
 * begins with a quoted identifier, say) the category is the honest fallback
 * rather than a guess.
 */
export function verbOf(sql: string, kind: StatementKind): string {
  let s = sql;
  // Leading comments and whitespace, in any order and any number.
  for (;;) {
    const before = s;
    s = s.replace(/^\s+/, "");
    if (s.startsWith("--") || s.startsWith("#")) s = s.replace(/^[^\n]*\n?/, "");
    else if (s.startsWith("/*")) {
      const end = s.indexOf("*/");
      s = end === -1 ? "" : s.slice(end + 2);
    }
    if (s === before) break;
  }
  const m = /^\(*\s*([A-Za-z_][A-Za-z_]*)/.exec(s);
  return m ? m[1].toUpperCase() : KIND_LABEL[kind];
}

/** The SQL as one line, so every row is one row tall. */
function oneLine(sql: string): string {
  return sql.replace(/\s+/g, " ").trim();
}

function firstLine(text: string): string {
  const at = text.indexOf("\n");
  return (at === -1 ? text : text.slice(0, at)).trim();
}

/**
 * Which statements the list is showing.
 *
 * The two questions worth asking a log of statements, and neither is answerable
 * by reading the grid: *what did this change* and *what went wrong*. Anything
 * finer than that is a search box, which this is not.
 */
export type SeqFilter = "all" | "changes" | "errors";

function matches(entry: SequenceEntry, filter: SeqFilter): boolean {
  if (filter === "all") return true;
  if (filter === "errors") return entry.shape === "error";
  return entry.shape === "change";
}

export interface LogTotals {
  statements: number;
  changes: number;
  rowsChanged: number;
  errors: number;
  /** After the filter — so the header can say "showing 2 of 8". */
  shown: number;
}

/**
 * What the log holds, and how much of it is on screen.
 *
 * **The chips count the whole log, not the filtered view.** They are its
 * summary, the way the result bar summarises a result set; a filter changes
 * what you are looking *through*, not what the tab did. The count of what is
 * shown is said once, beside the filter that caused it.
 */
export function totals(log: SequenceLog, filter: SeqFilter): LogTotals {
  const t: LogTotals = { statements: 0, changes: 0, rowsChanged: 0, errors: 0, shown: 0 };
  for (const run of log.runs) add(t, run, filter);
  return t;
}

/** The same question of one run, which is what the no-result-sets hint asks. */
export function runTotals(run: SequenceRun): LogTotals {
  const t: LogTotals = { statements: 0, changes: 0, rowsChanged: 0, errors: 0, shown: 0 };
  add(t, run, "all");
  return t;
}

function add(t: LogTotals, run: SequenceRun, filter: SeqFilter) {
  for (const e of run.entries) {
    t.statements += 1;
    if (e.shape === "change") {
      t.changes += 1;
      t.rowsChanged += e.rows ?? 0;
    }
    if (e.shape === "error") t.errors += 1;
    if (matches(e, filter)) t.shown += 1;
  }
}

/** Did this run return any rows at all? The hint exists for when it did not. */
export function returnedRows(run: SequenceRun): boolean {
  return run.entries.some((e) => e.shape === "data");
}

/**
 * The log as text, as it is on screen.
 *
 * Filtered, because copying what is not shown is a surprise — and the header
 * beside the filter already says how much that is. Tab-separated, like the
 * grid's own copy: it reads in a terminal and pastes into a spreadsheet.
 */
export function logText(log: SequenceLog, filter: SeqFilter): string {
  const lines: string[] = [];
  for (const run of log.runs) {
    const kept = run.entries.filter((e) => matches(e, filter));
    if (!kept.length) continue;
    lines.push(`${run.what}	${clock(run.at)}	${run.summary}`);
    for (const e of kept) {
      lines.push(`	${e.verb}	${e.elapsedMs} ms	${e.fullOutcome}	${e.sql}`);
    }
  }
  if (log.droppedRuns > 0) {
    lines.push(
      `[${plural(log.droppedRuns, "earlier run")} dropped, ` +
        `${plural(log.droppedEntries, "statement")} not listed]`,
    );
  }
  return lines.length ? `${lines.join("\n")}\n` : "";
}

/** One row of the painted list: a run's header, or one of its statements. */
type Row =
  | { kind: "run"; run: SequenceRun }
  | { kind: "entry"; run: SequenceRun; runIndex: number; entry: SequenceEntry };

export interface SequenceFor {
  log: SequenceLog;
  filter: SeqFilter;
  /**
   * Which result tab this entry's statement has right now, or -1.
   *
   * Asked rather than stored, because `closeStatement` renumbers the tabs: a
   * row saying "Result 3" while the strip says "Result 2" would be a confident
   * lie, and an entry whose result has been closed should simply stop offering
   * the link.
   */
  resultIndexOf: (runIndex: number, entry: SequenceEntry) => number;
  onGoToResult: (index: number) => void;
  onReveal: (run: SequenceRun, entry: SequenceEntry) => void;
  scrollTop: number;
  onScrolled: (top: number) => void;
}

/**
 * The painted list.
 *
 * Windowed like `ResultView`'s rows, and watching its own size for the same
 * reason Stage 26 had to add a `ResizeObserver` there: a pane that grows
 * paints no more rows than the old viewport could show, and the empty band
 * below looks like the end of the log.
 */
export class SequenceView {
  private rows: Row[] = [];
  private opts: SequenceFor | null = null;
  private sizeWatch: ResizeObserver | null = null;

  constructor(private host: HTMLElement) {
    this.host.addEventListener("scroll", () => {
      this.opts?.onScrolled(this.host.scrollTop);
      this.paint();
    });
    if (typeof ResizeObserver !== "undefined") {
      this.sizeWatch = new ResizeObserver(() => this.paint());
      this.sizeWatch.observe(this.host);
    }
  }

  /** A sentence instead of a list: nothing has run, or nothing matches. */
  setMessage(text: string) {
    this.rows = [];
    this.opts = null;
    this.host.replaceChildren(div("seq-empty", text));
  }

  show(opts: SequenceFor) {
    this.opts = opts;
    this.rows = [];
    opts.log.runs.forEach((run, runIndex) => {
      const kept = run.entries.filter((entry) => matches(entry, opts.filter));
      // A run with nothing left in it is a header over a gap. The run still
      // happened, and the chips below still count it.
      if (!kept.length) return;
      this.rows.push({ kind: "run", run });
      for (const entry of kept) this.rows.push({ kind: "entry", run, runIndex, entry });
    });
    if (!this.rows.length) {
      this.setMessage(
        opts.filter === "errors"
          ? "Nothing has failed in this tab."
          : "Nothing in this tab has changed any rows.",
      );
      return;
    }
    this.paint();
    this.host.scrollTop = opts.scrollTop;
    this.paint();
  }

  /** Repaint for a viewport that changed size without scrolling. */
  relayout() {
    this.paint();
  }

  private paint() {
    const opts = this.opts;
    if (!opts || !this.rows.length) return;
    const total = this.rows.length;
    const viewport = this.host.clientHeight;
    const first = Math.max(0, Math.floor(this.host.scrollTop / SEQ_ROW_H) - OVERSCAN);
    const visible = Math.ceil(viewport / SEQ_ROW_H) + OVERSCAN * 2;
    const last = Math.min(total, first + visible);

    const frag = document.createDocumentFragment();
    frag.append(spacer(first * SEQ_ROW_H));
    for (let i = first; i < last; i++) frag.append(this.rowEl(this.rows[i], opts));
    frag.append(spacer((total - last) * SEQ_ROW_H));
    this.host.replaceChildren(frag);
  }

  private rowEl(row: Row, opts: SequenceFor): HTMLElement {
    if (row.kind === "run") {
      const head = div("seq-run");
      head.style.height = `${SEQ_ROW_H}px`;
      head.append(div("seq-what", row.run.what), div("seq-at", clock(row.run.at)));
      head.append(div("seq-run-summary", row.run.summary));
      return head;
    }

    const { entry, run, runIndex } = row;
    const el = div("seq-row");
    el.style.height = `${SEQ_ROW_H}px`;

    const dot = div(`seq-dot seq-${entry.shape}`);
    dot.setAttribute("aria-hidden", "true");
    el.append(dot);

    el.append(div(`seq-verb seq-verb-${entry.shape}`, entry.verb));

    // Its own button: the SQL is what you click to find the statement again,
    // the same gesture a result tab's double-click already is.
    const sql = document.createElement("button");
    sql.type = "button";
    sql.className = "seq-sql";
    sql.textContent = entry.sql;
    sql.title =
      (entry.effectiveSql && entry.effectiveSql !== entry.fullSql
        ? `${entry.fullSql}\n\nSent as:\n${entry.effectiveSql}\n\n`
        : `${entry.fullSql}\n\n`) + "Click to select this statement in the editor";
    sql.onclick = () => opts.onReveal(run, entry);
    el.append(sql);

    const at = entry.madeRows ? opts.resultIndexOf(runIndex, entry) : -1;
    if (at >= 0) {
      const link = document.createElement("button");
      link.type = "button";
      link.className = "seq-out seq-link";
      link.textContent = `${entry.outcome} → Result ${at + 1}`;
      link.title = "Show this result in Results";
      link.onclick = () => opts.onGoToResult(at);
      el.append(link);
    } else {
      const out = div(`seq-out seq-out-${entry.shape}`, entry.outcome);
      if (entry.fullOutcome !== entry.outcome) out.title = entry.fullOutcome;
      el.append(out);
    }

    el.append(div("seq-ms", `${entry.elapsedMs} ms`));
    return el;
  }
}

function div(cls: string, text?: string): HTMLElement {
  const e = document.createElement("div");
  e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function spacer(height: number): HTMLElement {
  const s = div("seq-spacer");
  s.style.height = `${height}px`;
  return s;
}

function clock(at: number): string {
  const d = new Date(at);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}
