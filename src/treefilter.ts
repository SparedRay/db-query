// The schema tree's quick filter.
//
// # What it searches
//
// Every name the app has: the nodes on screen, and — since Stage 29.2 — the
// columns Stage 27 prefetched for the whole server, which are in memory whether
// or not anybody has expanded the table holding them.
//
// This used to say that searching only loaded nodes was "a deliberate limit
// rather than a shortcoming, because searching the server would be a different
// feature, with a round trip and a spinner". Stage 27 removed that cost: on an
// engine that can describe a database in one query, every column name is
// already here. What is left of the limit is honest and narrow — an engine
// without a bulk column query (Elasticsearch) has only the columns somebody
// opened, and the count says the answer covers what was loaded.
//
// Columns were also never searched *at all*, loaded or not: a column node is
// returned bare, with no wrapper, and the walk read every child of a
// `.children` as a wrapper. See `walk`.
//
// Views, procedures and functions are searched too, from the lists the tree
// already holds. Opening a database fetches `list_tables` and `list_routines`
// together — the counts in the group labels come from exactly that — so a
// collapsed group's names cost nothing to search. The first version of this
// left them out on the grounds that "the prefetch does not record which kind a
// name is", which was true of the column prefetch and beside the point: the
// tree's own group closures record it.
//
// What is left unfetched is a database nobody has opened, whose routines were
// never listed. The count says `loaded only` there, which is the one thing it
// can honestly say.
//
// # Why nothing is expanded or collapsed
//
// The obvious implementation walks the tree setting `hidden = false` on the
// path to each match, and puts it back afterwards. That makes the filter own
// the expansion state, and the moment anything else touches it — a refresh, a
// lazy load finishing, the user clicking mid-filter — the two disagree and the
// tree ends up in a state neither of them chose.
//
// So filtering is **presentation only**. A class on the root reveals collapsed
// containers for as long as the filter is active, matches are marked, and
// clearing the box restores exactly the tree that was there before, because
// nothing about it was ever changed.
//
// A column match in a table nobody has expanded is marked on the table, with
// how many matched, rather than by drawing the columns: drawing them would be
// the filter deciding what is loaded. Expanding it yourself brings them in, and
// the tree's mutation observer re-runs this, so they arrive already marked.
//
// **The mark is an attribute, not an element.** That observer watches
// `childList`, so a filter that appended a badge would retrigger itself for
// ever; `data-filter-note` is read back out by CSS.

/** How a filter pass went, for the count beside the box. */
export interface FilterResult {
  matches: number;
  /** Names searched — nodes on screen, plus prefetched columns. */
  searched: number;
  /**
   * Something on screen could not be searched, so the answer covers what was
   * loaded rather than the server.
   *
   * An engine with no bulk column query keeps its mappings until asked (Stage
   * 27, decision 3), and a prefetch still in flight has not arrived yet.
   * Either way the honest count says so instead of implying completeness.
   */
  partial: boolean;
}

/**
 * The names the tree cannot see, because nothing has drawn them.
 *
 * `null` means *never fetched*, which is a different answer from an empty list
 * and the only reason `partial` can be told from "nothing matched".
 */
export interface SchemaLookup {
  columns(db: string, table: string): string[] | null;
  tables(db: string): string[] | null;
  /** Names a group holds, or null when the tree never fetched them. */
  group(db: string, label: string): string[] | null;
  /**
   * Has the tree read what this database contains, routines included?
   *
   * False is what turns the count into "loaded only" for a database nobody has
   * opened: its tables and columns are prefetched, but no `list_routines` has
   * run, so claiming a complete search would be a claim about names never read.
   */
  opened(db: string): boolean;
}

function hits(names: string[], needle: string): number {
  let n = 0;
  for (const name of names) if (name.toLowerCase().includes(needle)) n += 1;
  return n;
}

/**
 * Count what is real but not drawn, under this node.
 *
 * Only when its children are **not** drawn: a table already showing its
 * columns has them walked like any other node, and counting them here as well
 * would report every match twice.
 */
function fromSchema(
  node: HTMLElement,
  children: Element | null,
  needle: string,
  out: FilterResult,
  look: SchemaLookup,
): string {
  if (children?.querySelector(".node")) return "";
  const db = node.dataset.db ?? "";

  if (node.classList.contains("table")) {
    const cols = look.columns(db, node.dataset.table ?? "");
    if (cols === null) {
      out.partial = true;
      return "";
    }
    out.searched += cols.length;
    const n = hits(cols, needle);
    if (n === 0) return "";
    out.matches += n;
    return `${n} col${n === 1 ? "" : "s"}`;
  }

  if (node.classList.contains("group")) {
    const names = look.group(db, node.dataset.group ?? "");
    // Never fetched: the database this group belongs to has not been opened,
    // and its own node is what says so.
    if (names === null) return "";
    // A routine has no columns here — its parameters are not in the prefetch —
    // so asking would answer null and wrongly report a partial search.
    const relations = node.dataset.groupKind === "relation";
    let n = 0;
    for (const name of names) {
      out.searched += 1;
      if (name.toLowerCase().includes(needle)) n += 1;
      if (!relations) continue;
      const cols = look.columns(db, name);
      if (cols === null) {
        out.partial = true;
        continue;
      }
      out.searched += cols.length;
      n += hits(cols, needle);
    }
    if (n === 0) return "";
    out.matches += n;
    return `${n} inside`;
  }

  if (node.classList.contains("db")) {
    // Its routines were never listed, whatever its columns say.
    if (!look.opened(db)) out.partial = true;
    const tables = look.tables(db);
    if (tables === null) {
      out.partial = true;
      return "";
    }
    let n = 0;
    for (const table of tables) {
      out.searched += 1;
      if (table.toLowerCase().includes(needle)) n += 1;
      const cols = look.columns(db, table);
      if (cols === null) {
        out.partial = true;
        continue;
      }
      out.searched += cols.length;
      n += hits(cols, needle);
    }
    if (n === 0) return "";
    out.matches += n;
    return `${n} inside`;
  }

  return "";
}

/**
 * One element of a tree level, and whether anything at or below it matched.
 *
 * That answer is what decides whether it stays on screen — a group is worth
 * showing when it *contains* a match even though its own label does not.
 *
 * The element handed in is **either a wrapper or a node**. A `.node` plus its
 * `.children` come wrapped in a div; a leaf is returned bare, because a column
 * has no children to hold. Reading every child of a `.children` as a wrapper is
 * why columns were never searched: the walk took a column's `.twisty` for its
 * node, found none, recursed into spans and reported nothing — so a filter
 * typed over an expanded table said "none of 8" with the match on screen.
 */
function walk(
  el: Element,
  needle: string,
  out: FilterResult,
  look: SchemaLookup | null,
): boolean {
  const node = el.classList.contains("node") ? el : el.firstElementChild;
  if (!(node instanceof HTMLElement) || !node.classList.contains("node")) {
    // Not a wrapper but a plain container — each connection's tree is held in
    // one, and `showTree` puts that inside `#tree`. Walk through it rather than
    // stopping: returning false here searched nothing at all, and the count
    // read "none of 0" beside a full tree.
    let any = false;
    for (const child of Array.from(el.children)) {
      if (walk(child, needle, out, look)) any = true;
    }
    return any;
  }

  const label = node.querySelector(".label")?.textContent ?? "";
  const self = label.toLowerCase().includes(needle);
  out.searched += 1;

  // A leaf is its own element, so it has no `:scope > .children` and this is
  // null — which is also what stops `fromSchema` double-counting it.
  const children = el === node ? null : el.querySelector(":scope > .children");
  let below = false;
  if (children) {
    for (const child of Array.from(children.children)) {
      // No short-circuit: every descendant must be marked, not just enough of
      // them to prove the parent stays.
      if (walk(child, needle, out, look)) below = true;
    }
  }

  const note = look ? fromSchema(node, children, needle, out, look) : "";
  if (note) node.dataset.filterNote = note;
  else delete node.dataset.filterNote;

  // A database is never hidden, even when nothing under it matches.
  //
  // It is the only thing you can click to load more, so hiding it strands the
  // user: type a filter before expanding anything and the whole tree vanishes,
  // including the one node that would have brought the tables in. A database
  // showing nothing beneath it is a useful answer; an empty panel is not.
  const isRoot = node.classList.contains("db");
  const visible = self || below || isRoot || note !== "";
  el.classList.toggle("filtered-out", !visible);
  node.classList.toggle("filter-hit", self);
  if (self) out.matches += 1;
  return visible;
}

/**
 * Apply `query` to a tree. An empty query clears the filter entirely.
 *
 * `look` is optional, and omitting it searches the tree alone — which is what
 * the tests of the walk itself do, and what a connection whose prefetch
 * belongs to a different server must do.
 */
export function applyTreeFilter(
  root: HTMLElement,
  query: string,
  look: SchemaLookup | null = null,
): FilterResult {
  const needle = query.trim().toLowerCase();
  const out: FilterResult = { matches: 0, searched: 0, partial: false };

  if (!needle) {
    root.classList.remove("filtering");
    for (const el of root.querySelectorAll(".filtered-out")) {
      el.classList.remove("filtered-out");
    }
    for (const el of root.querySelectorAll(".filter-hit")) {
      el.classList.remove("filter-hit");
    }
    for (const el of root.querySelectorAll<HTMLElement>("[data-filter-note]")) {
      delete el.dataset.filterNote;
    }
    return out;
  }

  root.classList.add("filtering");
  for (const child of Array.from(root.children)) walk(child, needle, out, look);
  return out;
}
