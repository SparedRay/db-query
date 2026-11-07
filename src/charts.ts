// Chart marks for the performance dock (Stage 33).
//
// # Why a module, and not a painter per chart
//
// Five boards came out of the Stage 33 review and every one of them draws the
// same two things: a proportion of a whole, or a magnitude against a shared
// scale. Written as five painters inside `workload.ts` they would have been
// five copies of the same arithmetic with five chances to get the denominator
// wrong — and a sixth chart, whenever it comes, would have been a sixth copy.
// So the shapes live here and `workload.ts` supplies the numbers.
//
// # What this module does not do
//
// **It does not format.** Every figure arrives as a string that the caller has
// already run through `duration`, `compact` or `ratioText`. A chart that
// formatted its own numbers would print them differently from the row beside
// it, and in this panel the row beside it is the evidence.
//
// **It does not name colours.** A mark asks for a *tone* — `mark`, `quiet`,
// `faint` — and `styles.css` decides what that is in each theme. The tones are
// the app's accent plus two neutrals, which is the **emphasis** form: one
// thing in colour and the rest drained, rather than a hue per series. That is
// deliberate, and §3 of the stage doc is the argument.
//
// **It never takes the connection's colour.** Stage 32's rule: `--conn-colour`
// marks *identity* — which server am I looking at. A bar's length is a
// measurement about that server, which is semantics, and semantics keeps the
// app's own accent. A chart that changed colour with the connection would be
// saying the measurement belonged to the colour.
//
// # Everything is built from nodes
//
// Never `innerHTML`. Half of what is drawn here is labelled with a table name
// or a digest's text, and both come from the server.

/** Which of the three marks a fill uses. `styles.css` owns the colours. */
export type Tone = "mark" | "quiet" | "faint";

export interface Segment {
  label: string;
  /** What this part is worth. Shares are computed here, never passed in. */
  value: number;
  tone: Tone;
  /** The figure, already formatted. This module does no formatting. */
  text: string;
}

export interface BarRow {
  label: string;
  value: number;
  /** The figure printed at the end of the row. */
  text: string;
  /** The whole row's hover. On the row, not the fill: a fill of zero width
   *  cannot be hovered, and zero width is a meaningful value on a log axis. */
  title: string;
  /** The row the lead sentence already names: it takes the accent and every
   *  other row goes quiet. Emphasis, rather than a colour per row. */
  lead?: boolean;
}

export interface PairRow {
  label: string;
  /** The near end — the mean. */
  from: number;
  /** The far end — the tail. */
  to: number;
  text: string;
  title: string;
  /** The row worth looking at: the widest gap, not the biggest number. */
  flag?: boolean;
}

export interface Axis {
  /** The top of the scale: the next power of ten at or above the data. */
  hi: number;
  ticks: number[];
  /** What to print at a tick. */
  text: (n: number) => string;
}

/**
 * A decade axis from 1 to the next power of ten at or above `max`.
 *
 * Both log charts in this panel start at 1 rather than at the smallest value,
 * because in both of them **1 means something**: one row read per row
 * returned, and one millisecond. A scale that started at the data's floor
 * would move the only fixed point either chart has.
 */
export function decades(max: number): Omit<Axis, "text"> {
  const top = Math.max(10, 10 ** Math.ceil(Math.log10(Math.max(1, max))));
  const ticks: number[] = [];
  for (let t = 1; t <= top; t *= 10) ticks.push(t);
  return { hi: top, ticks };
}

/** Where a value sits on a decade axis, as a fraction of its width. */
export function logPos(v: number, hi: number): number {
  if (!(v > 1) || !(hi > 1)) return 0;
  return Math.min(1, Math.log10(v) / Math.log10(hi));
}

function pct(share: number): string {
  return `${(Math.max(0, Math.min(1, share)) * 100).toFixed(2)}%`;
}

function el(tag: string, cls: string, text?: string): HTMLElement {
  const node = document.createElement(tag);
  node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

/**
 * One whole split into parts: a thin bar, and a legend that carries the
 * figures.
 *
 * **Thin and wordless, rather than tall with the shares printed on it.** The
 * legend under the bar already names every part and gives its figure, so
 * printing them on the bar as well would be the same number twice, in a place
 * where a short segment cannot hold it. The bar's job here is the proportion
 * and nothing else.
 *
 * A part worth nothing is drawn as nothing — not as the 3px sliver a
 * `min-width` would give it — and still appears in the legend with its zero.
 * "None of this" and "a little of this" must not look the same.
 */
export function segmentedBar(segments: Segment[]): HTMLElement {
  const total = segments.reduce((n, s) => n + Math.max(0, s.value), 0);

  const box = el("div", "chart-split");
  const bar = el("div", "chart-seg");
  bar.setAttribute("role", "img");
  bar.setAttribute(
    "aria-label",
    segments.map((s) => `${s.label} ${s.text}`).join(", "),
  );

  const legend = el("div", "chart-legend");

  for (const s of segments) {
    const value = Math.max(0, s.value);

    const part = el("i", `chart-part tone-${s.tone}`);
    part.style.flexGrow = String(total > 0 ? value / total : 0);
    part.title = `${s.label} — ${s.text}`;
    part.hidden = value <= 0;
    bar.append(part);

    const item = el("span", "chart-key");
    item.append(
      el("i", `chart-swatch tone-${s.tone}`),
      el("span", "chart-key-name", s.label),
      el("b", "chart-key-val", s.text),
    );
    legend.append(item);
  }

  box.append(bar, legend);
  return box;
}

/**
 * Magnitude down a list of rows, on one shared scale.
 *
 * `linear` is for a quantity whose *disparity* is the finding — one shape
 * against the rest of the server. A log axis there would make six bars
 * comparable, which is exactly the wrong favour: the small bars are supposed
 * to look small.
 *
 * `log` is for a quantity read in orders of magnitude, where a linear axis
 * would crush the range the verdict actually turns in. It draws an axis and
 * gridlines, so nobody reads it as linear by accident.
 */
export function rankedBars(
  rows: BarRow[],
  opts: { axis?: Axis; note?: string },
): HTMLElement {
  const box = el("div", "chart-plot");
  const axis = opts.axis;
  const max = rows.reduce((n, r) => Math.max(n, r.value), 0);

  if (axis) box.append(gridlines(axis));

  const list = el("div", "chart-rows");
  for (const r of rows) {
    const row = el("div", `chart-row${r.lead ? " lead" : ""}`);
    row.title = r.title;

    const name = el("span", "chart-name", r.label);
    const val = el("span", "chart-val", r.text);

    const track = el("div", `chart-track${axis ? " on-axis" : ""}`);
    const fill = el("div", `chart-fill tone-${r.lead ? "mark" : "quiet"}`);
    fill.style.width = pct(axis ? logPos(r.value, axis.hi) : max > 0 ? r.value / max : 0);
    // A linear bar of zero is absent, not a sliver. On a log axis zero width
    // *is* the value — a row sitting on the origin — so it keeps its fill.
    if (!axis && r.value <= 0) fill.hidden = true;
    track.append(fill);

    row.append(name, val, track);
    list.append(row);
  }
  box.append(list);

  if (axis) box.append(axisStrip(axis));
  if (opts.note) box.append(el("p", "chart-note", opts.note));
  return box;
}

/**
 * Two values of the same measure, per row, and the distance between them.
 *
 * A dumbbell rather than two bars, because the reader's job is the **gap**.
 * Two bars of different colours invite a comparison across rows that nobody
 * needs, and bury the one thing this chart exists to show.
 */
export function dumbbells(
  rows: PairRow[],
  opts: { axis: Axis; note?: string },
): HTMLElement {
  const box = el("div", "chart-plot");
  box.append(gridlines(opts.axis));

  const list = el("div", "chart-rows");
  for (const r of rows) {
    const row = el("div", `chart-row${r.flag ? " lead" : ""}`);
    row.title = r.title;

    const name = el("span", "chart-name", r.label);
    const val = el("span", "chart-val", r.text);

    // Ordered, so a tail that somehow came back below the mean still draws a
    // bar rather than a negative-width one that renders as nothing.
    const lo = logPos(Math.min(r.from, r.to), opts.axis.hi);
    const hi = logPos(Math.max(r.from, r.to), opts.axis.hi);

    const track = el("div", "chart-pair");
    const line = el("i", "chart-pair-line");
    line.style.left = pct(lo);
    line.style.right = pct(1 - hi);
    const a = el("i", "chart-dot tone-from");
    a.style.left = pct(lo);
    const b = el("i", "chart-dot tone-to");
    b.style.left = pct(hi);
    track.append(line, a, b);

    row.append(name, val, track);
    list.append(row);
  }
  box.append(list);

  box.append(axisStrip(opts.axis));
  if (opts.note) box.append(el("p", "chart-note", opts.note));
  return box;
}

/** The two ends of a dumbbell, named. Identity is never colour alone. */
export function pairLegend(from: string, to: string): HTMLElement {
  const legend = el("div", "chart-legend");
  for (const [tone, label] of [
    ["from", from],
    ["to", to],
  ] as const) {
    const item = el("span", "chart-key");
    item.append(el("i", `chart-swatch tone-${tone}`), el("span", "chart-key-name", label));
    legend.append(item);
  }
  return legend;
}

function gridlines(axis: Axis): HTMLElement {
  const grid = el("div", "chart-grid");
  grid.setAttribute("aria-hidden", "true");
  for (const t of axis.ticks) {
    if (t === 1) continue; // the origin has its own rule, drawn by the track
    const line = el("i", "chart-gridline");
    line.style.left = pct(logPos(t, axis.hi));
    grid.append(line);
  }
  return grid;
}

function axisStrip(axis: Axis): HTMLElement {
  const strip = el("div", "chart-axis");
  for (const t of axis.ticks) {
    const tick = el("span", "chart-tick", axis.text(t));
    tick.style.left = pct(logPos(t, axis.hi));
    if (t === 1) tick.classList.add("first");
    if (t === axis.hi) tick.classList.add("last");
    strip.append(tick);
  }
  return strip;
}
