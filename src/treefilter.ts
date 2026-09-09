// The schema tree's quick filter.
//
// # It filters what is on screen, and nothing else
//
// Only nodes that have already been loaded are searched, because those are the
// only ones that exist: the tree is lazy, and a database whose tables have not
// been fetched has no table nodes to match. That is a deliberate limit rather
// than a shortcoming — searching the server would be a different feature, with
// a round trip and a spinner, and this one is for "I know the name, get me
// there" on a schema already open.
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

/** How a filter pass went, for the count beside the box. */
export interface FilterResult {
  matches: number;
  /** Nodes searched — everything currently loaded. */
  searched: number;
}

/**
 * One wrapper element: a `.node` and, optionally, its `.children`.
 *
 * Returns whether anything at or below it matched, which is what decides
 * whether it stays on screen — a group is worth showing when it *contains* a
 * match even though its own label does not.
 */
function walk(wrap: Element, needle: string, out: FilterResult): boolean {
  const node = wrap.firstElementChild;
  if (!(node instanceof HTMLElement) || !node.classList.contains("node")) {
    // Not a wrapper but a plain container — each connection's tree is held in
    // one, and `showTree` puts that inside `#tree`. Walk through it rather than
    // stopping: returning false here searched nothing at all, and the count
    // read "none of 0" beside a full tree.
    let any = false;
    for (const child of Array.from(wrap.children)) {
      if (walk(child, needle, out)) any = true;
    }
    return any;
  }

  const label = node.querySelector(".label")?.textContent ?? "";
  const self = label.toLowerCase().includes(needle);
  out.searched += 1;

  let below = false;
  const children = wrap.querySelector(":scope > .children");
  if (children) {
    for (const child of Array.from(children.children)) {
      // No short-circuit: every descendant must be marked, not just enough of
      // them to prove the parent stays.
      if (walk(child, needle, out)) below = true;
    }
  }

  // A database is never hidden, even when nothing under it matches.
  //
  // It is the only thing you can click to load more, so hiding it strands the
  // user: type a filter before expanding anything and the whole tree vanishes,
  // including the one node that would have brought the tables in. A database
  // showing nothing beneath it is a useful answer; an empty panel is not.
  const isRoot = node.classList.contains("db");
  const visible = self || below || isRoot;
  wrap.classList.toggle("filtered-out", !visible);
  node.classList.toggle("filter-hit", self);
  if (self) out.matches += 1;
  return visible;
}

/**
 * Apply `query` to a tree. An empty query clears the filter entirely.
 */
export function applyTreeFilter(root: HTMLElement, query: string): FilterResult {
  const needle = query.trim().toLowerCase();
  const out: FilterResult = { matches: 0, searched: 0 };

  if (!needle) {
    root.classList.remove("filtering");
    for (const el of root.querySelectorAll(".filtered-out")) {
      el.classList.remove("filtered-out");
    }
    for (const el of root.querySelectorAll(".filter-hit")) {
      el.classList.remove("filter-hit");
    }
    return out;
  }

  root.classList.add("filtering");
  for (const child of Array.from(root.children)) walk(child, needle, out);
  return out;
}
