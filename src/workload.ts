// Where the server's time goes (Stage 31).
//
// # What it is for
//
// Reading MySQL's own workload counters and saying what kind of problem each
// query shape is. It **never runs anything** — the same rule as query history
// and every generated-SQL action in this app: the user reviews and executes.
// The most it will do is open a tab with an analysis scaffold in it.
//
// # Why a dock, and not a modal
//
// The first cut of this was a `<dialog>` opened with `showModal()`. Stage 21
// had already written down why that is wrong for a panel whose job is to hand
// you a statement, and the reasoning transfers word for word:
//
//   * **Everything behind a modal is inert.** The moment a proposal appears is
//     exactly when you want to look at the table it names, the indexes it
//     already has, or the rows in front of you — and a modal is what stops
//     you.
//   * **It owns the keyboard.** `main.ts` skips the global shortcuts while a
//     dialog is open, so Ctrl+T and Ctrl+W did nothing with the panel up.
//   * **Using a finding had to close it.** `openInTab` ended with
//     `dialog.close()`, which threw away the reading that produced the
//     statement — so comparing two shapes meant re-reading the server twice.
//
// A tab is not the answer either: `tabs.ts` says no tab is anything but an
// editor, and Stage 28 moved migrations *into* the dock for that reason. So
// both precedents point the same way, and this is the fourth dock.
//
// The one real cost is horizontal: four docks cannot all be open on a laptop.
// They are independent toggles, as `mig-shut` and `chat-closed` already are,
// so that is a choice rather than a layout failure.
//
// # Why there is no index proposal written out for you
//
// MySQL has no hypothetical index — no HypoPG, no missing-index DMV — so a
// `CREATE INDEX` this panel invented could not be priced by the server before
// you ran it. Worse, the columns would have to come from parsing a predicate
// this app does not parse. So the scaffold names the table, shows the evidence
// and the write cost, and leaves the columns to the person who can read the
// shape. Saying less is the honest option.

import {
  api,
  type IndexFault,
  type IndexInventory,
  type WorkloadPanel,
  type WorkloadShape,
  type WorkloadClass,
  type WorkloadVerdict,
} from "./api";

export interface WorkloadDeps {
  /** Elements, resolved by the caller so this module owns no lookups. */
  scope: HTMLElement;
  refresh: HTMLButtonElement;
  close: HTMLButtonElement;
  diagnosis: HTMLElement;
  headline: HTMLElement;
  detail: HTMLElement;
  body: HTMLElement;
  tiles: HTMLElement;
  loops: HTMLElement;
  classes: HTMLElement;
  sorts: HTMLElement;
  counts: Record<WorkloadClass, HTMLElement>;
  known: HTMLElement;
  list: HTMLElement;
  invRun: HTMLButtonElement;
  invNote: HTMLElement;
  inv: HTMLElement;
  windows: HTMLElement;
  baseline: HTMLButtonElement;
  /** The connection whose workspace is on screen, or null. */
  activeConnection: () => string | null;
  /** What to call it, for the scope line. */
  connectionLabel: () => string;
  /** Open this SQL as a new tab. Never runs it, and never shuts this panel. */
  openInTab: (sql: string) => void;
  /** Put a question to the assistant. Composed here, **not sent** — the
   *  person reads it and presses Send, as with every other proposal here. */
  ask: (question: string) => void;
  /** Show or hide the dock. Owned by the caller, which owns the grid. */
  setOpen: (open: boolean) => void;
  /** Whether the dock is showing right now. */
  isOpen: () => boolean;
}

type Sort = "time" | "examined" | "execs";

/** What each verdict is called, and whether it proposes anything. */
const VERDICTS: Record<WorkloadVerdict, string> = {
  missingIndex: "missing index",
  noIndexWillHelp: "no index will help",
  viewBlocksFilter: "view blocks the filter",
  rowByRowLoop: "row-by-row loop",
  waitingNotReading: "waiting, not reading",
  driverChatter: "driver chatter",
  platformInternal: "platform internal",
  unclassified: "unclassified",
};

/** One sentence per verdict, for the expanded row. */
const WHY: Record<WorkloadVerdict, string> = {
  missingIndex:
    "It reads far more than it returns, and some executions found no index to use.",
  noIndexWillHelp:
    "It returns everything it reads, so a scan is the right plan and no index can shrink it. " +
    "The fix is to ask for less, in the caller.",
  viewBlocksFilter:
    "The target is a view, so the predicate cannot reach the base tables and there is no " +
    "index on this name to add. The view body is where to look.",
  rowByRowLoop:
    "Several shapes on this table ran the same number of times and started together. " +
    "That is a loop in the caller; no index changes how often it runs.",
  waitingNotReading:
    "It spends its time blocked rather than reading — it examined no rows at all. " +
    "Look at locks, not indexes.",
  driverChatter:
    "Session setup, or the server being asked about itself. Not application work.",
  platformInternal:
    "The managed platform's own housekeeping, running inside your instance and charged " +
    "to your counters. Nothing here is yours to change.",
  unclassified:
    "None of the rules fired. Said out loud rather than guessed at — the numbers are still real.",
};

/** Seconds as something a person can hold in their head. */
export function duration(secs: number): string {
  if (secs >= 3600) {
    const h = Math.floor(secs / 3600);
    const m = Math.round((secs - h * 3600) / 60);
    return m > 0 ? `${h}h ${m}m` : `${h}h`;
  }
  if (secs >= 60) {
    const m = Math.floor(secs / 60);
    const s = Math.round(secs - m * 60);
    return s > 0 ? `${m}m ${s}s` : `${m}m`;
  }
  if (secs >= 1) return `${secs.toFixed(1)} s`;
  return `${(secs * 1000).toFixed(0)} ms`;
}

/** Big counts, short. 108,410,167,913 is unreadable in a 104px column. */
export function compact(n: number): string {
  if (n >= 1e12) return `${(n / 1e12).toFixed(2)} T`;
  if (n >= 1e9) return `${(n / 1e9).toFixed(2)} B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)} M`;
  if (n >= 1e4) return `${(n / 1e3).toFixed(1)} K`;
  return n.toLocaleString();
}

/** Rows read per row returned. `null` means nothing was returned. */
export function ratioText(ratio: number | null): string {
  if (ratio === null) return "—";
  if (ratio >= 10) return `${Math.round(ratio).toLocaleString()} : 1`;
  return `${ratio.toFixed(1)} : 1`;
}

/** MySQL identifier quoting: a backtick inside a name is doubled. */
function quoteIdent(name: string): string {
  return "`" + name.replace(/`/g, "``") + "`";
}

export function createWorkload(deps: WorkloadDeps) {
  let panel: WorkloadPanel | null = null;
  let only: WorkloadClass = "application";
  let sort: Sort = "time";
  let expanded: string | null = null;
  let inventory: IndexInventory | null = null;
  /** Which window the person asked for — not which one they got. */
  let wanted: "start" | "baseline" = "start";
  /** Bumped per request so a slow read cannot overwrite a newer one. */
  let generation = 0;

  function shapes(): WorkloadShape[] {
    const all = panel?.report?.shapes ?? [];
    const mine = all.filter((s) => s.class === only);
    const by: Record<Sort, (s: WorkloadShape) => number> = {
      time: (s) => s.totalSecs,
      examined: (s) => s.rowsExamined,
      execs: (s) => s.execs,
    };
    return [...mine].sort((a, b) => by[sort](b) - by[sort](a));
  }

  function tile(label: string, value: string, note: string, tone = ""): HTMLElement {
    const box = document.createElement("div");
    box.className = "wl-tile";
    const l = document.createElement("div");
    l.className = "wl-tile-label";
    l.textContent = label;
    const v = document.createElement("div");
    v.className = "wl-tile-value" + (tone ? ` ${tone}` : "");
    v.textContent = value;
    const n = document.createElement("div");
    n.className = "wl-tile-note";
    n.textContent = note;
    box.append(l, v, n);
    return box;
  }

  function paintTiles() {
    deps.tiles.replaceChildren();
    const report = panel?.report;
    if (!report) return;

    const app = report.shapes.filter((s) => s.class === "application");
    const worst = app.reduce<WorkloadShape | null>(
      (best, s) => (!best || s.totalSecs > best.totalSecs ? s : best),
      null,
    );
    const readiest = app.reduce<WorkloadShape | null>(
      (best, s) => (!best || s.rowsExamined > best.rowsExamined ? s : best),
      null,
    );
    const wantsIndex = app.filter((s) => s.verdict === "missingIndex").length;

    deps.tiles.append(
      tile(
        "Worst shape, total time",
        worst ? duration(worst.totalSecs) : "—",
        worst ? `${compact(worst.execs)} executions` : "nothing to rank",
        "bad",
      ),
      tile(
        "Rows examined, that shape",
        readiest ? compact(readiest.rowsExamined) : "—",
        readiest
          ? `to return ${compact(readiest.rowsSent)} · ${ratioText(readiest.ratio)}`
          : "nothing to rank",
        "warn",
      ),
      tile(
        "Wants an index",
        String(wantsIndex),
        `of ${report.applicationShapes} application shapes`,
        "num",
      ),
      tile(
        "Filtered as chatter",
        duration(report.chatterSecs + report.platformSecs),
        `${report.chatterShapes + report.platformShapes} shapes, hidden by default`,
      ),
    );
  }

  function paintLoops() {
    const found = panel?.report?.loops ?? [];
    deps.loops.replaceChildren();
    deps.loops.hidden = found.length === 0 || only !== "application";
    for (const loop of found) {
      const box = document.createElement("div");
      box.className = "wl-loop";

      const lead = document.createElement("b");
      lead.textContent = `${loop.digests.length} shapes on ${loop.table} ran the same number of times`;

      const body = document.createElement("div");
      const spread = (loop.spread * 100).toFixed(3);
      const ms = Math.round(loop.firstSeenSpreadUs / 1000);
      body.textContent =
        `${loop.execsHigh.toLocaleString()} down to ${loop.execsLow.toLocaleString()}` +
        ` — a spread of ${spread}%, first seen ${ms} ms apart.` +
        ` ${duration(loop.totalSecs)} of server time between them.` +
        " An index helps the read; only the caller can stop the loop.";

      box.append(lead, body);
      deps.loops.append(box);
    }
  }

  function cell(text: string, cls: string): HTMLElement {
    const el = document.createElement("span");
    el.className = cls;
    el.textContent = text;
    return el;
  }

  /** A small labelled number for the facts line. */
  function fact(label: string, value: string, cls = ""): HTMLElement {
    const box = document.createElement("span");
    box.className = "wl-fact";
    const v = document.createElement("span");
    v.className = `wl-num ${cls}`.trim();
    v.textContent = value;
    const l = document.createElement("span");
    l.textContent = ` ${label}`;
    box.append(v, l);
    return box;
  }

  function detailFor(shape: WorkloadShape): HTMLElement {
    const box = document.createElement("div");
    box.className = "wl-detail";

    const why = document.createElement("p");
    why.className = "wl-why";
    why.textContent = WHY[shape.verdict];

    // textContent, never innerHTML: this is a statement from a server.
    const text = document.createElement("pre");
    text.className = "wl-digest";
    text.textContent = shape.text;

    const facts = document.createElement("div");
    facts.className = "wl-facts";
    const bits: string[] = [
      `${shape.execs.toLocaleString()} executions`,
      `${duration(shape.totalSecs)} total`,
      `${shape.avgMillis.toFixed(2)} ms average`,
    ];
    if (shape.p95Millis !== null) bits.push(`${shape.p95Millis.toFixed(2)} ms at p95`);
    bits.push(`${shape.rowsExamined.toLocaleString()} rows examined`);
    bits.push(`${shape.rowsSent.toLocaleString()} returned`);
    if (shape.noIndex > 0) bits.push(`${shape.noIndex.toLocaleString()} used no index`);
    if (shape.scans > 0) bits.push(`${shape.scans.toLocaleString()} full scans`);
    if (shape.sortScans > 0) bits.push(`${shape.sortScans.toLocaleString()} filesorts`);
    if (shape.tmpDisk > 0) bits.push(`${shape.tmpDisk.toLocaleString()} temp tables on disk`);
    if (shape.errors > 0) bits.push(`${shape.errors.toLocaleString()} errors`);
    if (shape.lockSecs > 0.5) bits.push(`${duration(shape.lockSecs)} waiting on locks`);
    if (shape.firstSeen) bits.push(`first seen ${shape.firstSeen}`);
    facts.textContent = bits.join(" · ");

    box.append(why, facts, text);

    // What an index would cost, from the same digests. This is the half an
    // index advisor usually skips, and it is free here.
    const writes = panel?.report?.writes ?? {};
    for (const table of shape.tables) {
      const n = writes[table];
      if (!n) continue;
      const cost = document.createElement("p");
      cost.className = "wl-cost";
      cost.textContent =
        `${table} takes ${n.toLocaleString()} writes over this window, so every index on it ` +
        "is maintained that many times. A narrow index is affordable here; a wide covering " +
        "index is not.";
      box.append(cost);
    }

    const caveat = document.createElement("p");
    caveat.className = "wl-caveat";
    caveat.textContent =
      "MySQL has no hypothetical index, so nothing here can price a candidate before you " +
      "build it. Add it on a restored copy and compare.";
    box.append(caveat);

    const actions = document.createElement("div");
    actions.className = "wl-actions";

    if (shape.tables.length > 0) {
      const open = document.createElement("button");
      open.type = "button";
      open.className = "mini wl-open";
      open.textContent = "Open an analysis tab";
      open.onclick = (e) => {
        e.stopPropagation();
        deps.openInTab(scaffold(shape));
      };
      actions.append(open);
    }

    const explain = document.createElement("button");
    explain.type = "button";
    explain.className = "mini wl-explain";
    explain.textContent = "Ask the assistant";
    explain.onclick = (e) => {
      e.stopPropagation();
      deps.ask(question(shape));
    };
    actions.append(explain);

    box.append(actions);
    return box;
  }

  /**
   * The question to put to the assistant about one shape.
   *
   * It carries the **evidence, not the verdict's conclusion**: the counters,
   * the normalised statement and the write cost, so the model is arguing from
   * what the server said rather than from what this module decided. The
   * verdict is named too, because disagreeing with it is a useful answer.
   *
   * Safe to send by construction. A digest holds no literal values, so there
   * is nothing here that a parameter could have smuggled in — the same
   * property that makes this panel safe to log.
   */
  function question(shape: WorkloadShape): string {
    const writes = panel?.report?.writes ?? {};
    const lines: string[] = [
      `This query shape is the ${VERDICTS[shape.verdict]} case on my server. ` +
        "Over the window these counters cover it ran " +
        `${shape.execs.toLocaleString()} times for ${duration(shape.totalSecs)} of server time, ` +
        `${shape.avgMillis.toFixed(2)} ms on average.`,
      `It examined ${shape.rowsExamined.toLocaleString()} rows to return ` +
        `${shape.rowsSent.toLocaleString()} (${ratioText(shape.ratio)}).`,
    ];
    if (shape.noIndex > 0) {
      lines.push(`${shape.noIndex.toLocaleString()} executions used no index at all.`);
    }
    if (shape.scans > 0) {
      lines.push(`${shape.scans.toLocaleString()} scanned a table from the first row.`);
    }
    if (shape.sortScans > 0) {
      lines.push(`${shape.sortScans.toLocaleString()} needed a filesort.`);
    }
    for (const table of shape.tables) {
      const n = writes[table];
      if (n) {
        lines.push(
          `${table} takes ${n.toLocaleString()} writes over the same window, so any index ` +
            "on it is maintained that many times.",
        );
      }
    }
    lines.push(
      "",
      "The statement, as the server normalised it — the values are already `?`:",
      "",
      shape.text,
      "",
      "What would you change, and what would it cost? If an index is the answer, say which " +
        "columns and in what order. If it is not, say that instead.",
    );
    return lines.join("\n");
  }

  /**
   * A tab to start from: the evidence as comments, and the one statement that
   * is actually the next thing to run.
   *
   * No `CREATE INDEX` is written out — see this module's header. `SHOW INDEX`
   * is, because what is already indexed is the first thing anybody needs and
   * this panel does not know it.
   */
  function scaffold(shape: WorkloadShape): string {
    const writes = panel?.report?.writes ?? {};
    const lines: string[] = [
      `-- Where the time goes: ${VERDICTS[shape.verdict]}`,
      `-- ${WHY[shape.verdict]}`,
      "--",
      `-- ${shape.execs.toLocaleString()} executions, ${duration(shape.totalSecs)} of server time.`,
      `-- ${shape.rowsExamined.toLocaleString()} rows examined to return ` +
        `${shape.rowsSent.toLocaleString()} (${ratioText(shape.ratio)}).`,
    ];
    for (const table of shape.tables) {
      const n = writes[table];
      if (n) {
        lines.push(
          `-- ${table}: ${n.toLocaleString()} writes over the same window — every index here`,
          "--   is maintained that many times.",
        );
      }
    }
    lines.push("--", "-- The shape, as the server normalised it:");
    for (const line of shape.text.split("\n")) lines.push(`--   ${line}`);
    lines.push("");
    for (const table of shape.tables) {
      lines.push(`SHOW INDEX FROM ${quoteIdent(table)};`);
    }
    lines.push(
      "",
      "-- MySQL cannot price an index that does not exist. Build the candidate on a",
      "-- restored copy and compare, then bring the statement back here.",
    );
    return lines.join("\n");
  }

  function paintList() {
    deps.list.replaceChildren();
    const rows = shapes();

    if (rows.length === 0) {
      const empty = document.createElement("div");
      empty.className = "wl-empty";
      empty.textContent =
        only === "application"
          ? "No application shapes — everything counted is driver or platform traffic."
          : "Nothing in this list.";
      deps.list.append(empty);
      return;
    }

    for (const shape of rows) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "wl-row";
      row.dataset.digest = shape.digest;
      row.setAttribute("aria-expanded", String(expanded === shape.digest));
      row.title = shape.text;

      row.append(cell(shape.text, "wl-shape"));

      const facts = document.createElement("div");
      facts.className = "wl-facts-line";

      const verdict = document.createElement("span");
      verdict.className = `wl-verdict ${shape.verdict}`;
      verdict.textContent = VERDICTS[shape.verdict];
      facts.append(verdict);

      facts.append(fact("total", duration(shape.totalSecs), "strong"));
      facts.append(fact("execs", compact(shape.execs)));
      facts.append(fact("examined", compact(shape.rowsExamined), "num"));
      facts.append(
        fact(
          "per row",
          ratioText(shape.ratio),
          shape.ratio !== null && shape.ratio > 100 ? "bad" : shape.ratio === null ? "nil" : "",
        ),
      );

      row.append(facts);

      row.onclick = () => {
        expanded = expanded === shape.digest ? null : shape.digest;
        paintList();
      };

      deps.list.append(row);
      if (expanded === shape.digest) deps.list.append(detailFor(shape));
    }
  }

  const FAULTS: Record<IndexFault, string> = {
    neverRead: "never read",
    redundant: "redundant",
    unindexedForeignKey: "foreign key, no index",
  };

  function paintInventory() {
    deps.inv.replaceChildren();
    deps.inv.hidden = inventory === null;
    if (!inventory) return;

    const findings = inventory.findings;
    // "Absent" and "none" are different claims and must not read the same.
    const missing = inventory.usageAvailable
      ? ""
      : " · performance_schema refused, so never-read indexes are not listed";
    deps.invNote.textContent =
      `${inventory.indexesSeen.toLocaleString()} indexes across ` +
      `${inventory.databases.length} database${inventory.databases.length === 1 ? "" : "s"}` +
      ` · ${findings.length} worth questioning${missing}`;

    if (findings.length === 0) {
      const empty = document.createElement("div");
      empty.className = "wl-empty";
      empty.textContent = inventory.usageAvailable
        ? "Nothing to question: no unread index, no redundant one, no foreign key without support."
        : "No redundant index and no unsupported foreign key. Whether any index goes unread cannot be read on this connection.";
      deps.inv.append(empty);
      return;
    }

    // Order: the ones that cost the most writes first, since that is what
    // makes an unused index expensive rather than merely untidy.
    const sorted = [...findings].sort((a, b) => (b.writes ?? -1) - (a.writes ?? -1));

    for (const f of sorted) {
      const row = document.createElement("div");
      row.className = "wl-inv-row";

      const fault = document.createElement("span");
      fault.className = `wl-fault ${f.fault}`;
      fault.textContent = FAULTS[f.fault];

      const what = document.createElement("div");
      what.className = "wl-inv-what";
      const stmt = document.createElement("div");
      stmt.className = "wl-inv-stmt";
      stmt.textContent = f.statement;
      stmt.title = f.statement;
      const why = document.createElement("div");
      why.className = "wl-inv-why";
      const cols = f.columns.length > 0 ? ` (${f.columns.join(", ")})` : "";
      why.textContent =
        f.fault === "redundant"
          ? `${f.name}${cols} is a leading prefix of ${f.coveredBy ?? "another index"}.`
          : f.fault === "neverRead"
            ? `${f.name}${cols} has not been read once since the counters started.`
            : `${f.name}${cols} has a constraint but no index to support it.`;
      what.append(stmt, why);

      const cost = document.createElement("span");
      cost.className = "wl-num" + (f.writes && f.writes > 1e6 ? " bad" : "");
      cost.textContent = f.writes === null ? "—" : `${compact(f.writes)} writes`;
      cost.title =
        f.writes === null
          ? "The workload never mentioned this table, so there is nothing to price it with."
          : `${f.writes.toLocaleString()} writes on ${f.table} over this window.`;

      const line = document.createElement("div");
      line.className = "wl-facts-line";
      line.append(fault, cost);
      row.append(what, line);
      deps.inv.append(row);
    }
  }

  async function loadInventory() {
    const connectionId = deps.activeConnection();
    if (!connectionId) return;
    deps.invRun.disabled = true;
    deps.invNote.textContent = "Reading…";
    try {
      inventory = await api.workloadIndexes(connectionId, panel?.databases ?? []);
      paintInventory();
    } catch (e) {
      inventory = null;
      deps.inv.hidden = true;
      deps.invNote.textContent = String(e);
    } finally {
      deps.invRun.disabled = false;
    }
  }

  function paintCounts() {
    const report = panel?.report;
    const counts: Record<WorkloadClass, number> = {
      application: report?.applicationShapes ?? 0,
      chatter: report?.chatterShapes ?? 0,
      platform: report?.platformShapes ?? 0,
    };
    for (const key of ["application", "chatter", "platform"] as const) {
      deps.counts[key].textContent = String(counts[key]);
      deps.counts[key].hidden = !report;
    }
    for (const btn of deps.classes.querySelectorAll<HTMLButtonElement>(".seg-btn")) {
      btn.setAttribute("aria-pressed", String(btn.dataset.class === only));
    }
    for (const btn of deps.sorts.querySelectorAll<HTMLButtonElement>(".seg-btn")) {
      btn.setAttribute("aria-pressed", String(btn.dataset.sort === sort));
    }
  }

  function paintWindows() {
    const has = panel?.baselineAt != null;
    // Pressed reflects what is *shown*, not what was asked for: a void
    // baseline falls back to the lifetime, and the control has to agree with
    // the numbers under it.
    const showing = panel?.windowed ? "baseline" : "start";
    for (const btn of deps.windows.querySelectorAll<HTMLButtonElement>(".seg-btn")) {
      const which = btn.dataset.window;
      btn.setAttribute("aria-pressed", String(which === showing));
      if (which === "baseline") btn.disabled = !has;
    }
    deps.baseline.textContent = has ? "Re-baseline" : "Set baseline";
    deps.baseline.disabled = !panel?.report && !panel?.diagnosis.usable;
  }

  function paint() {
    const diagnosis = panel?.diagnosis ?? null;

    // Shown whenever it says something — including the usable case where the
    // ring has overflowed, which is a warning about the data rather than a
    // refusal to show it.
    if (panel?.baselineVoid) {
      // Not a server fault and not nothing: the baseline is older than the
      // counters it was taken from, so the window cannot be computed and the
      // lifetime is being shown instead.
      deps.headline.textContent = "Your baseline is older than the server's counters.";
      deps.detail.textContent =
        "A counter went backwards, so the server restarted or the digest table was truncated " +
        "since you took it. Showing the whole lifetime instead — take a new baseline to get a " +
        "window back.";
      deps.diagnosis.classList.add("wl-blocked");
      deps.diagnosis.hidden = false;
    } else if (diagnosis) {
      deps.headline.textContent = diagnosis.headline;
      deps.detail.textContent = diagnosis.detail;
      deps.diagnosis.classList.toggle("wl-blocked", !diagnosis.usable);
      deps.diagnosis.hidden = diagnosis.usable && !diagnosis.headline.includes("sample");
    } else {
      deps.diagnosis.hidden = true;
    }

    deps.body.hidden = !panel?.report;
    deps.scope.textContent = panel ? scopeLine(panel) : "";

    deps.invRun.hidden = !panel?.report;
    if (!panel?.report) {
      deps.inv.hidden = true;
      deps.invNote.textContent = "";
    }

    if (panel?.report) {
      deps.known.textContent =
        panel.knownTables > 0
          ? `${panel.knownTables.toLocaleString()} table names known`
          : "no schema warmed — views and loops cannot be identified";
      paintTiles();
      paintLoops();
      paintCounts();
      paintWindows();
      paintList();
      paintInventory();
    }
  }

  function scopeLine(p: WorkloadPanel): string {
    const bits: string[] = [deps.connectionLabel()];
    if (p.windowed && p.baselineAt != null) {
      const since = Math.max(0, Date.now() / 1000 - p.baselineAt);
      bits.push(`window of ${duration(since)} since the baseline`);
      if (p.newShapes.length > 0) {
        bits.push(
          `${p.newShapes.length} shape${p.newShapes.length === 1 ? "" : "s"} never seen before it`,
        );
      }
    } else {
      const uptime = p.diagnosis.readings.uptimeSecs;
      if (uptime) {
        const days = Math.floor(uptime / 86400);
        bits.push(days >= 1 ? `counters cover ${days} days` : `counters cover ${duration(uptime)}`);
      }
    }
    if (p.report) {
      // The backend reads every digest but ships only the most expensive
      // `SHAPES_SENT` of them, so on a busy server this list is a subset and
      // has to say so. Totals in the tiles are over everything read.
      const shown = p.report.shapes.length;
      bits.push(
        shown < p.report.totalShapes
          ? `showing ${shown.toLocaleString()} of ${p.report.totalShapes.toLocaleString()} shapes`
          : `${p.report.totalShapes.toLocaleString()} shapes in the ring`,
      );
    }
    if (p.databases.length > 0) bits.push(p.databases.join(", "));
    return bits.join(" · ");
  }

  async function load() {
    const connectionId = deps.activeConnection();
    if (!connectionId) {
      panel = null;
      deps.body.hidden = true;
      deps.diagnosis.hidden = false;
      deps.diagnosis.classList.add("wl-blocked");
      deps.headline.textContent = "No connection is open.";
      deps.detail.textContent =
        "This panel reads counters from a server, so there has to be one to read.";
      deps.scope.textContent = "";
      return;
    }

    const mine = ++generation;
    deps.refresh.disabled = true;
    try {
      const next = await api.workloadRead(connectionId, wanted === "baseline");
      if (mine !== generation) return;
      panel = next;
      expanded = null;
      // The inventory belongs to the reading it was taken with, so a refresh
      // drops it rather than leaving stale write counts beside fresh ones.
      inventory = null;
      paint();
    } catch (e) {
      if (mine !== generation) return;
      panel = null;
      deps.body.hidden = true;
      deps.diagnosis.hidden = false;
      deps.diagnosis.classList.add("wl-blocked");
      deps.headline.textContent = "The server would not say.";
      deps.detail.textContent = String(e);
    } finally {
      if (mine === generation) deps.refresh.disabled = false;
    }
  }

  deps.close.onclick = () => deps.setOpen(false);
  deps.refresh.onclick = () => void load();
  deps.invRun.onclick = () => void loadInventory();

  deps.windows.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLButtonElement>(".seg-btn");
    const next = btn?.dataset.window as "start" | "baseline" | undefined;
    if (!next || btn?.disabled || next === wanted) return;
    wanted = next;
    void load();
  });

  deps.baseline.onclick = async () => {
    const connectionId = deps.activeConnection();
    if (!connectionId) return;
    deps.baseline.disabled = true;
    try {
      await api.workloadBaselineSet(connectionId);
      // Switch to the window immediately. A baseline you have just taken
      // shows an empty window, which is correct and is also the clearest way
      // to see that it took.
      wanted = "baseline";
      await load();
    } catch (e) {
      deps.diagnosis.hidden = false;
      deps.diagnosis.classList.add("wl-blocked");
      deps.headline.textContent = "The baseline was not taken.";
      deps.detail.textContent = String(e);
    } finally {
      deps.baseline.disabled = false;
    }
  };

  deps.classes.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLButtonElement>(".seg-btn");
    const next = btn?.dataset.class as WorkloadClass | undefined;
    if (!next || next === only) return;
    only = next;
    expanded = null;
    paint();
  });

  deps.sorts.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLButtonElement>(".seg-btn");
    const next = btn?.dataset.sort as Sort | undefined;
    if (!next || next === sort) return;
    sort = next;
    expanded = null;
    paint();
  });

  return {
    /** Open the dock and read. Reading again on every open is deliberate:
     *  counters move, and a panel showing a reading from an hour ago with no
     *  sign of its age is worse than one that costs a query. */
    async open() {
      deps.setOpen(true);
      await load();
    },
    close() {
      deps.setOpen(false);
    },
    /** Toggle, for the rail button — which is how every other dock behaves. */
    async toggle() {
      if (deps.isOpen()) {
        deps.setOpen(false);
        return;
      }
      deps.setOpen(true);
      await load();
    },
    isOpen: () => deps.isOpen(),
  };
}
