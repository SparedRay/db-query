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
import {
  type BarRow,
  type PairRow,
  decades,
  dumbbells,
  pairLegend,
  rankedBars,
  segmentedBar,
} from "./charts";

export interface WorkloadDeps {
  /** Elements, resolved by the caller so this module owns no lookups. */
  scope: HTMLElement;
  refresh: HTMLButtonElement;
  close: HTMLButtonElement;
  diagnosis: HTMLElement;
  headline: HTMLElement;
  detail: HTMLElement;
  body: HTMLElement;
  /** The finding, in a sentence, and the line under it. */
  lead: HTMLElement;
  leadSub: HTMLElement;
  /** The Workload / Indexes tabs, and the counts on them. */
  views: HTMLElement;
  viewCounts: { workload: HTMLElement; indexes: HTMLElement };
  /** Carries `data-view`; CSS decides which list is on screen. */
  panes: HTMLElement;
  classes: HTMLElement;
  sort: HTMLSelectElement;
  counts: Record<WorkloadClass, HTMLElement>;
  list: HTMLElement;
  /** Where the server's time went, as a proportion (Stage 33 §4.1). */
  split: HTMLElement;
  /** The chart card: one switch, three drawings of the rows below it. */
  chart: HTMLElement;
  chartViews: HTMLElement;
  chartBody: HTMLElement;
  chartTitle: HTMLElement;
  /** Says "log scale" where the axis is one. A reader who takes a log axis
   *  for a linear one has read the chart backwards, so it is never implied. */
  chartScale: HTMLElement;
  invRun: HTMLButtonElement;
  invNote: HTMLElement;
  /** How the indexes divide, above the list of the ones worth questioning. */
  invSplit: HTMLElement;
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

/** Which of the three drawings the chart card is showing (Stage 33). */
type ChartView = "ranked" | "ratio" | "latency";

/**
 * How many rows a chart draws.
 *
 * Five, in a 420px column, with everything below them folded into one bar on
 * the ranked chart. Not a scroller: a chart you have to scroll has stopped
 * being a glance and the list underneath is already the long form.
 */
const CHART_ROWS = 5;

/** What each verdict is called, and whether it proposes anything. */
const VERDICTS: Record<WorkloadVerdict, string> = {
  missingIndex: "missing index",
  predicateNotIndexable: "predicate cannot be indexed",
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
  predicateNotIndexable:
    "The filter wraps a column in a function, so no b-tree index applies to it — and across " +
    "an OR, MySQL cannot merge an unindexable branch with an indexable one either. " +
    "Rewriting the predicate comes before any index.",
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
  /**
   * Which list is on screen in the dock.
   *
   * Meaningless when the panel is expanded, where both are side by side — so
   * it is written to `data-view` and a single CSS rule stops honouring it,
   * rather than this module having to know which mode it is in.
   */
  let view: "workload" | "indexes" = "workload";
  let only: WorkloadClass = "application";
  let sort: Sort = "time";
  let chartView: ChartView = "ranked";
  let expanded: string | null = null;
  let inventory: IndexInventory | null = null;
  /** Which window the person asked for — not which one they got. */
  let wanted: "start" | "baseline" = "start";
  /** Bumped per request so a slow read cannot overwrite a newer one. */
  let generation = 0;
  /**
   * Whose counters are on screen.
   *
   * Every number in this panel belongs to one server, and nothing about the
   * reading says which. Held so `connectionChanged` can tell a switch from the
   * dozens of other reasons the connection label repaints.
   */
  let readFor: string | null = null;

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

  /** The period the counters cover, as a phrase that fits mid-sentence. */
  function windowPhrase(p: WorkloadPanel): string {
    if (p.windowed && p.baselineAt != null) {
      const since = Math.max(0, Date.now() / 1000 - p.baselineAt);
      return `the ${duration(since)} since your baseline`;
    }
    const uptime = p.diagnosis.readings.uptimeSecs;
    if (!uptime) return "the window these counters cover";
    const days = Math.floor(uptime / 86400);
    return days >= 1 ? `the last ${days} days` : `the last ${duration(uptime)}`;
  }

  /** How the lead sentence ends, which is the verdict said as a consequence. */
  const CONSEQUENCE: Record<WorkloadVerdict, string> = {
    missingIndex: "and an index would help it",
    predicateNotIndexable: "and no index can help it",
    noIndexWillHelp: "and no index can shrink it",
    viewBlocksFilter: "and the index it wants cannot exist on that name",
    rowByRowLoop: "and the caller runs it once per row",
    waitingNotReading: "and it spends that time blocked, not reading",
    driverChatter: "and none of it is application work",
    platformInternal: "and none of it is yours to change",
    unclassified: "and none of the rules explain it",
  };

  /**
   * The finding, in a sentence, before any control.
   *
   * What this replaces is four tiles — worst shape, rows examined, wants an
   * index, filtered as chatter — which named every number in the sentence and
   * left the reader to write it, with the verdict that explains them three
   * bands further down and smaller than the SQL beside it.
   */
  function paintLead() {
    const report = panel?.report;
    deps.lead.replaceChildren();
    deps.leadSub.textContent = "";
    if (!report || !panel) return;

    const app = report.shapes.filter((s) => s.class === "application");
    const worst = app.reduce<WorkloadShape | null>(
      (best, s) => (!best || s.totalSecs > best.totalSecs ? s : best),
      null,
    );

    if (!worst) {
      deps.lead.textContent =
        "Nothing in this reading is application work — every shape the server " +
        "counted is driver or platform traffic.";
      deps.leadSub.textContent =
        `${duration(report.chatterSecs + report.platformSecs)} across ` +
        `${report.chatterShapes + report.platformShapes} shapes. Switch the filter to see them.`;
      return;
    }

    // Built from nodes, not innerHTML: `table` is a server-supplied name.
    const hot = document.createElement("span");
    hot.className = "wl-hot";
    hot.textContent = duration(worst.totalSecs);

    const where = document.createElement("span");
    where.className = "wl-obj";
    where.textContent = worst.tables[0] ?? "an unnamed target";

    deps.lead.append(
      hot,
      document.createTextNode(` of ${windowPhrase(panel)} went into one shape on `),
      where,
      document.createTextNode(`, ${CONSEQUENCE[worst.verdict]}.`),
    );

    const wantsIndex = app.filter((s) => s.verdict === "missingIndex").length;
    const others = Math.max(0, report.applicationShapes - 1);
    const sub = document.createElement("span");
    const n = document.createElement("b");
    n.textContent = String(wantsIndex);
    sub.append(n, document.createTextNode(` of the other ${others} application shapes do want one.`));

    const chatterSecs = report.chatterSecs + report.platformSecs;
    const chatterShapes = report.chatterShapes + report.platformShapes;
    deps.leadSub.replaceChildren(sub);
    if (chatterShapes > 0) {
      const t = document.createElement("b");
      t.textContent = duration(chatterSecs);
      deps.leadSub.append(
        document.createTextNode(" "),
        t,
        document.createTextNode(
          ` was driver and platform chatter across ${chatterShapes} shapes, kept out of the list.`,
        ),
      );
    }
  }

  // ------------------------------------------------------------ the charts
  //
  // Stage 33. The panel measured everything and drew none of it: the two
  // facts that decide a verdict -- 2,599 : 1 and 1.0 : 1 -- sat in identical
  // grey chips at the bottom of two rows, three hundred-fold apart and the
  // same size on screen. Every claim this panel makes is a claim about a
  // magnitude, and until now not one of them was drawn to scale.

  /**
   * Where the server's time went, before a single shape is read.
   *
   * The honest first fact on a shared instance is what fraction of the
   * counters are yours at all — the panel said it in a subordinate clause of
   * the lead's second line, after the finding it qualifies.
   *
   * **Seconds, where the class switch under it carries counts.** The two
   * disagreeing about what they measure would be worse than no bar, so the
   * legend prints its own figures and names them. The switch was going to
   * take the seconds instead, which is what the Stage 33 mockup proposed; at
   * 420px three buttons reading `App 96h 40m` wrap onto two lines, and the
   * row already shares its width with the sort control.
   */
  function paintSplit() {
    deps.split.replaceChildren();
    const report = panel?.report;
    if (!report) return;

    deps.split.append(
      segmentedBar([
        {
          label: "App",
          value: report.applicationSecs,
          tone: "mark",
          text: duration(report.applicationSecs),
        },
        {
          label: "Driver",
          value: report.chatterSecs,
          tone: "quiet",
          text: duration(report.chatterSecs),
        },
        {
          label: "Platform",
          value: report.platformSecs,
          tone: "faint",
          text: duration(report.platformSecs),
        },
      ]),
    );
  }

  /** What the ranked chart draws, which is whatever the sort is ranking by. */
  const METRIC: Record<
    Sort,
    { title: string; of: (s: WorkloadShape) => number; text: (n: number) => string }
  > = {
    time: { title: "Shapes by server time", of: (s) => s.totalSecs, text: duration },
    examined: { title: "Shapes by rows examined", of: (s) => s.rowsExamined, text: compact },
    execs: { title: "Shapes by executions", of: (s) => s.execs, text: compact },
  };

  /** Milliseconds, on an axis whose decades cross from ms into seconds. */
  function millis(ms: number): string {
    return ms < 1000 ? `${Math.round(ms).toLocaleString()} ms` : `${+(ms / 1000).toFixed(2)} s`;
  }

  function paintChart() {
    const report = panel?.report;
    const rows = shapes();
    deps.chartBody.replaceChildren();
    // Nothing to rank is not an empty chart, it is no chart. The list below
    // already says why the list is empty.
    deps.chart.hidden = !report || rows.length === 0;
    if (!report || rows.length === 0) return;

    // `QUANTILE_95` arrived in 8.0, so on an older server the tab is
    // **absent** rather than present and empty — the same claim
    // `usageAvailable` makes about never-read indexes.
    if (!report.hasQuantiles && chartView === "latency") chartView = "ranked";
    for (const btn of deps.chartViews.querySelectorAll<HTMLButtonElement>(".seg-btn")) {
      if (btn.dataset.chart === "latency") btn.hidden = !report.hasQuantiles;
      btn.setAttribute("aria-pressed", String(btn.dataset.chart === chartView));
    }

    if (chartView === "ranked") drawRanked(rows);
    else if (chartView === "ratio") drawRatio(rows);
    else drawLatency(rows);
  }

  /**
   * One shape against the rest, on a linear scale.
   *
   * **Linear deliberately.** A log axis would make these bars comparable and
   * that is precisely the wrong favour: on UAT one shape is twenty-three
   * times the next one, that disparity *is* the finding, and a scale that
   * evened it out would have edited the result before the reader saw it. The
   * small bars are supposed to look small.
   *
   * One tone for every bar with the accent on the first — emphasis, not a
   * ramp. Shading each bar darker-where-bigger would encode the bar's length
   * twice and spend the only free channel on what the reader can already see.
   */
  function drawRanked(rows: WorkloadShape[]) {
    const metric = METRIC[sort];
    deps.chartTitle.textContent = metric.title;
    deps.chartScale.textContent =
      rows.length > CHART_ROWS ? `top ${CHART_ROWS} of ${rows.length}` : "";

    const head = rows.slice(0, CHART_ROWS);
    const tail = rows.slice(CHART_ROWS);

    const bars: BarRow[] = head.map((s, i) => {
      const value = metric.of(s);
      return {
        label: s.text,
        value,
        text: metric.text(value),
        title: `${s.text}\n${metric.text(value)} · ${VERDICTS[s.verdict]}`,
        // The first row is the one the lead sentence already names, because
        // the sort and the metric are the same choice.
        lead: i === 0,
      };
    });

    if (tail.length > 0) {
      const rest = tail.reduce((n, s) => n + metric.of(s), 0);
      bars.push({
        label: `${tail.length} more shape${tail.length === 1 ? "" : "s"}`,
        value: rest,
        text: metric.text(rest),
        title: `The ${tail.length} shapes below the top ${CHART_ROWS}, added together.`,
      });
    }

    deps.chartBody.append(
      rankedBars(bars, {
        note:
          "Linear, so the gap between the first bar and the rest is the size it really is.",
      }),
    );
  }

  /**
   * The evidence the verdict is already based on.
   *
   * This panel classifies a shape by asking how much it reads against how
   * much it returns, and then printed that ratio as a chip of grey text.
   * Drawn against a 1 : 1 rule the verdicts stop being assertions: a shape
   * far to the right is reading its way through a table, and a shape *on* the
   * line returns everything it reads, which is the one case where no index
   * can help.
   *
   * **Log here, for the opposite reason to the ranked chart.** On a linear
   * axis every bar but the first is a sliver and the range the verdict
   * actually turns in — 1 : 1 to perhaps 50 : 1 — is the first two percent of
   * the track.
   */
  function drawRatio(rows: WorkloadShape[]) {
    deps.chartTitle.textContent = "Rows read per row returned";
    deps.chartScale.textContent = "log scale";

    // A write returns nothing, so it has no ratio — and a shape with no ratio
    // is left out rather than drawn at zero, which would put it on the 1 : 1
    // rule and claim it reads exactly what it returns.
    const rated = rows
      .flatMap((s) => (s.ratio === null ? [] : [{ shape: s, ratio: s.ratio }]))
      .sort((a, b) => b.ratio - a.ratio)
      .slice(0, CHART_ROWS);

    if (rated.length === 0) {
      const empty = document.createElement("div");
      empty.className = "wl-empty";
      empty.textContent =
        "Nothing here returned a row, so there is no read-to-returned ratio to draw.";
      deps.chartBody.append(empty);
      return;
    }

    const axis = {
      ...decades(rated[0].ratio),
      text: (n: number) =>
        n === 1 ? "1 : 1" : n >= 1e6 ? compact(n) : n.toLocaleString(),
    };

    deps.chartBody.append(
      rankedBars(
        rated.map(({ shape, ratio }, i) => ({
          label: shape.text,
          value: ratio,
          text: ratioText(ratio),
          title:
            `${shape.text}\n${shape.rowsExamined.toLocaleString()} rows examined to return ` +
            `${shape.rowsSent.toLocaleString()} · ${VERDICTS[shape.verdict]}`,
          lead: i === 0,
        })),
        {
          axis,
          note:
            "The rule at 1 : 1 is a query that returns everything it reads. A bar with no " +
            "length is sitting on it, and that is the one case no index can help.",
        },
      ),
    );
  }

  /**
   * The gap between the mean and the tail, which is the one thing this panel
   * measures and has never been able to show.
   *
   * The list is ranked by total time, so a shape that runs rarely and is
   * catastrophic when it does never reaches the top of it. `workload.rs`
   * already says why it takes the column — *tail latency matters more than
   * the mean for a shape that is usually fine and occasionally terrible* —
   * and nothing on screen had ever shown the difference.
   *
   * **Ranked by the gap, not by time.** Ranked by time this draws the same
   * five rows as the ranked chart and finds nothing.
   */
  function drawLatency(rows: WorkloadShape[]) {
    deps.chartTitle.textContent = "Average → 95th percentile";
    deps.chartScale.textContent = "log scale";

    const pairs = rows
      .flatMap((s) =>
        s.p95Millis === null || s.avgMillis <= 0
          ? []
          : [{ shape: s, avg: s.avgMillis, p95: s.p95Millis, gap: s.p95Millis / s.avgMillis }],
      )
      .sort((a, b) => b.gap - a.gap)
      .slice(0, CHART_ROWS);

    if (pairs.length === 0) {
      const empty = document.createElement("div");
      empty.className = "wl-empty";
      empty.textContent = "No shape here has both an average and a 95th percentile to compare.";
      deps.chartBody.append(empty);
      return;
    }

    const axis = {
      ...decades(Math.max(...pairs.map((p) => p.p95))),
      text: millis,
    };

    const drawn: PairRow[] = pairs.map(({ shape, avg, p95, gap }, i) => ({
      label: shape.text,
      from: avg,
      to: p95,
      text: `${millis(avg)} → ${millis(p95)}`,
      title:
        `${shape.text}\nThe slowest 5% take ${gap >= 10 ? Math.round(gap) : gap.toFixed(1)}× ` +
        `the average · ${shape.execs.toLocaleString()} executions`,
      flag: i === 0,
    }));

    deps.chartBody.append(
      dumbbells(drawn, {
        axis,
        note:
          "Ranked by the gap rather than by total time: a shape that runs rarely and is " +
          "terrible when it does never reaches the top of the list.",
      }),
      pairLegend("average", "95th percentile"),
    );
  }

  /**
   * The loop this shape is part of, if any.
   *
   * A loop is a finding *across* rows, which is why it used to sit in a strip
   * of its own above the list. But it is still a finding *about* particular
   * rows, and up there it sat above whichever shape happened to sort first —
   * so the one row it was actually about got no mark at all. Attached to its
   * own row it says the same thing in the place it applies.
   */
  function loopFor(shape: WorkloadShape): HTMLElement | null {
    const loop = (panel?.report?.loops ?? []).find((l) => l.digests.includes(shape.digest));
    if (!loop) return null;

    const box = document.createElement("div");
    box.className = "wl-loop";

    const lead = document.createElement("b");
    lead.textContent = "In a loop.";

    const body = document.createElement("div");
    const spread = (loop.spread * 100).toFixed(3);
    const ms = Math.round(loop.firstSeenSpreadUs / 1000);
    body.textContent =
      `${loop.digests.length} shapes on ${loop.table} ran` +
      ` ${loop.execsHigh.toLocaleString()} down to ${loop.execsLow.toLocaleString()} times` +
      ` — a spread of ${spread}%, first seen ${ms} ms apart.` +
      ` ${duration(loop.totalSecs)} of server time between them.` +
      " An index helps the read; only the caller can stop the loop.";

    box.append(lead, body);
    return box;
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

    // No reason paragraph here any more: it is in the row, visible without a
    // click, which is the point of the row leading with its verdict.

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

    box.append(facts, text);

    const loop = loopFor(shape);
    if (loop) box.append(loop);

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

      // Verdict, then what it cost, then why, then the evidence. The shipped
      // order was the other way up — statement first, verdict last and
      // smallest — which made every row look alike until it was opened.
      const top = document.createElement("div");
      top.className = "wl-row-top";

      const verdict = document.createElement("span");
      verdict.className = `wl-verdict ${shape.verdict}`;
      verdict.textContent = VERDICTS[shape.verdict];

      const total = document.createElement("b");
      total.className = "wl-total";
      total.textContent = duration(shape.totalSecs);

      top.append(verdict, total);
      row.append(top);

      row.append(cell(WHY[shape.verdict], "wl-reason"));
      row.append(cell(shape.text, "wl-shape"));

      const facts = document.createElement("div");
      facts.className = "wl-facts-line";

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

  /**
   * How the indexes divide, above the list of the ones worth questioning
   * (Stage 33 §4.3).
   *
   * The inventory reports how many findings it has and never what that is a
   * fraction of. On UAT it is 245 of 1,025 — a quarter of every index on the
   * instance — and a panel proposing to drop a quarter of an instance's
   * indexes should say so before the first `DROP INDEX` is read.
   *
   * `PRIMARY` is a part of its own rather than folded into "read", because
   * those are excluded by **the rule** and not by the data: a third category
   * the list makes invisible by simply not showing it.
   *
   * Drawn only when the usage read answered. With no usage there is no
   * never-read segment, and a bar showing zero never-read would be a claim
   * nobody made — the same absent-rather-than-empty distinction the note
   * above it already draws.
   */
  function paintInvSplit() {
    deps.invSplit.replaceChildren();
    const usage = inventory?.usage;
    if (!usage || usage.seen === 0) return;

    deps.invSplit.append(
      segmentedBar([
        {
          label: "Read",
          value: usage.read,
          tone: "mark",
          text: usage.read.toLocaleString(),
        },
        {
          label: "Never read",
          value: usage.unread,
          tone: "quiet",
          text: usage.unread.toLocaleString(),
        },
        {
          label: "PRIMARY",
          value: usage.primaryKeys,
          tone: "faint",
          text: usage.primaryKeys.toLocaleString(),
        },
      ]),
    );
  }

  function paintInventory() {
    deps.inv.replaceChildren();
    deps.inv.hidden = inventory === null;
    paintInvSplit();
    if (!inventory) return;

    const findings = inventory.findings;
    // "Absent" and "none" are different claims and must not read the same.
    // And neither does "the server would not let us" read the same as "we
    // sent it something it could not parse" — the reason comes from the
    // server now rather than being assumed to be a permission.
    const missing = inventory.usageAvailable
      ? ""
      : ` · never-read indexes are not listed: ${inventory.usageRefusal ?? "the usage read did not answer"}`;
    deps.invNote.textContent =
      `${inventory.indexesSeen.toLocaleString()} indexes across ` +
      `${inventory.databases.length} database${inventory.databases.length === 1 ? "" : "s"}` +
      ` · ${findings.length} worth questioning${missing}`;

    if (findings.length === 0) {
      const empty = document.createElement("div");
      empty.className = "wl-empty";
      empty.textContent = inventory.usageAvailable
        ? "Nothing to question: no unread index, no redundant one, no foreign key without support."
        : "No redundant index and no unsupported foreign key. Whether any index goes unread could not be read — see the reason above.";
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
    deps.sort.value = sort;

    // The counts on the tabs. The index one is the inventory's, which is read
    // separately — so it is blank until it has been, rather than claiming a
    // zero that would read as "this server has no indexes".
    deps.viewCounts.workload.textContent = report ? String(report.shapes.length) : "";
    deps.viewCounts.indexes.textContent = inventory
      ? String(inventory.indexesSeen)
      : "";
  }

  /** Which list the dock shows. Does nothing when both are on screen. */
  function paintViews() {
    deps.panes.dataset.view = view;
    for (const tab of deps.views.querySelectorAll<HTMLButtonElement>(".wl-view-tab")) {
      tab.setAttribute("aria-selected", String(tab.dataset.view === view));
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

    paintViews();

    if (panel?.report) {
      paintLead();
      paintSplit();
      paintCounts();
      paintWindows();
      paintChart();
      paintList();
      paintInventory();
    } else {
      deps.split.replaceChildren();
      deps.invSplit.replaceChildren();
      deps.chart.hidden = true;
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
    // This had a line of its own under the controls. It is the same kind of
    // fact as the rest of this line — what the reading covers — and it is the
    // caveat behind two verdicts, so it belongs with them and not above the
    // list it qualifies.
    bits.push(
      p.knownTables > 0
        ? `${p.knownTables.toLocaleString()} table names known`
        : "no schema warmed, so views and loops cannot be identified",
    );
    return bits.join(" · ");
  }

  async function load() {
    const connectionId = deps.activeConnection();
    readFor = connectionId;
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

  /** Shutting the panel leaves the expanded view first — otherwise the editor
   *  stays hidden with nothing on screen that could bring it back. Which
   *  `setOpen` does, for every dock, since §6.16: expanding is the app's
   *  mechanism and not this panel's. */
  function shut() {
    deps.setOpen(false);
  }

  deps.close.onclick = shut;
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

  deps.sort.addEventListener("change", () => {
    const next = deps.sort.value as Sort;
    if (next === sort) return;
    sort = next;
    expanded = null;
    paint();
  });

  // Only the chart is repainted: the three views are three drawings of the
  // same rows, so nothing below the card changes and `paint()` would rebuild
  // the list and lose whichever row was expanded.
  deps.chartViews.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLButtonElement>(".seg-btn");
    const next = btn?.dataset.chart as ChartView | undefined;
    if (!next || next === chartView) return;
    chartView = next;
    paintChart();
  });

  deps.views.addEventListener("click", (e) => {
    const tab = (e.target as HTMLElement).closest<HTMLButtonElement>(".wl-view-tab");
    const next = tab?.dataset.view as "workload" | "indexes" | undefined;
    if (!next || next === view) return;
    view = next;
    paintViews();
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
      shut();
    },
    /**
     * Re-read when the active connection changes under an open panel.
     *
     * Called from the same place every other per-connection control is
     * synced, so it fires on far more than a switch — hence the identity
     * check, which makes it free on every call that changed nothing. Without
     * it the panel kept showing the server you had left, with a scope line
     * naming the one you had arrived at.
     *
     * A shut panel is left alone: `open` reads unconditionally, so there is
     * nothing to keep fresh and no reason to spend the queries.
     */
    connectionChanged() {
      if (deps.activeConnection() === readFor) return;
      if (!deps.isOpen()) return;
      // The baseline is stored per connection, so "since the baseline" meant
      // the server being left. Asking for it again here would silently fall
      // back to the lifetime; asking for the lifetime says so.
      wanted = "start";
      void load();
    },
    /** Toggle, for the rail button — which is how every other dock behaves. */
    async toggle() {
      if (deps.isOpen()) {
        shut();
        return;
      }
      deps.setOpen(true);
      await load();
    },
    isOpen: () => deps.isOpen(),
  };
}
