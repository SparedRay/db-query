// The schema tree's icons.
//
// # Why these are drawn here rather than taken from a pack
//
// The tree used to mix **emoji** (🗄 database, 👁 view, 🔑 key) with **text
// glyphs** (▦ ƒ ⚙ ·). Three things went wrong with that, none of which a
// different set of characters would have fixed:
//
//   * Emoji are rendered by the system's emoji font, in colour. They ignore
//     `color: var(--fg-dim)`, so half the tree was grey and half was not.
//   * Every character has its own advance width, so the labels beside them
//     started at different x positions — the "Tables / Views / Procedures"
//     column visibly failed to line up.
//   * Which glyphs are available, and whether one renders as text or as emoji,
//     differs across Linux, Windows and macOS. `⚙` in particular flips to a
//     colour emoji on some platforms and not others.
//
// An icon *pack* would fix all three, at the cost of a dependency and a licence
// obligation for seven shapes. These are simple enough to draw: one 16-unit
// grid, one stroke width, `currentColor` throughout — so they inherit the
// theme, scale with the font, and occupy exactly the same box as each other.

/** Every icon the tree can draw. */
export type IconName =
  | "database"
  | "table"
  | "view"
  | "procedure"
  | "function"
  | "key"
  | "column"
  // Chrome, for the same reason as the tree: `✦` and `↺` and `◴` are glyphs
  // with thin font coverage, and a missing one renders as a hollow box.
  | "chevron"
  | "refresh"
  | "settings"
  | "history"
  | "assistant"
  | "close";

/**
 * Path markup per icon, on a 16×16 grid.
 *
 * Stroked rather than filled, apart from the column dot, which is too small for
 * a stroke to read at 14px.
 */
const PATHS: Record<IconName, string> = {
  // A cylinder: the shape every database has used since before we were here.
  database:
    '<ellipse cx="8" cy="4.2" rx="5.3" ry="2.2"/>' +
    '<path d="M2.7 4.2v7.6c0 1.2 2.4 2.2 5.3 2.2s5.3-1 5.3-2.2V4.2"/>' +
    '<path d="M2.7 8c0 1.2 2.4 2.2 5.3 2.2s5.3-1 5.3-2.2"/>',
  // A grid with a header row, which is what a table looks like here.
  table:
    '<rect x="2.5" y="3" width="11" height="10" rx="1.4"/>' +
    '<path d="M2.5 6.6h11"/><path d="M6.6 6.6V13"/>',
  view: '<path d="M1.7 8S4 4.3 8 4.3 14.3 8 14.3 8 12 11.7 8 11.7 1.7 8 1.7 8z"/><circle cx="8" cy="8" r="1.7"/>',
  // Runnable, and distinct from a function at a glance.
  procedure:
    '<rect x="2.5" y="3" width="11" height="10" rx="1.4"/>' +
    '<path d="M6.6 6.4l3.4 2.6-3.4 2.6z"/>',
  // An italic f with its crossbar — the ƒ this replaces, drawn rather than typed.
  function: '<path d="M5.9 13V5.9c0-1.7 1-2.7 2.5-2.7.5 0 .9.1 1.3.3"/><path d="M4.2 7.6h4.9"/>',
  key: '<circle cx="6" cy="6.1" r="2.7"/><path d="M7.9 8l4.6 4.6"/><path d="M10.6 12.2l1.3-1.3"/>',
  column: '<circle cx="8" cy="8" r="1.5" fill="currentColor" stroke="none"/>',

  // Points right; CSS rotates it 90° when its node is open, so the open and
  // closed states cannot drift apart the way two separate glyphs can.
  chevron: '<path d="M6.2 3.8L10.4 8l-4.2 4.2"/>',
  refresh:
    '<path d="M13.2 8a5.2 5.2 0 1 1-1.6-3.7"/><path d="M13.4 2.9v3.3h-3.3"/>',
  // Sliders, not a cog. A cog's teeth need more pixels than 16 to read as
  // teeth: drawn as a circle with eight radiating lines it comes out looking
  // like a sun, which is what a brightness control looks like.
  settings:
    '<path d="M3 4.6h10M3 8h10M3 11.4h10"/>' +
    '<circle cx="6" cy="4.6" r="1.5"/><circle cx="10.4" cy="8" r="1.5"/>' +
    '<circle cx="5.2" cy="11.4" r="1.5"/>',
  // A clock with its hand back: history, without borrowing an arrow that means
  // "undo" everywhere else in an editor.
  history: '<circle cx="8" cy="8" r="5.6"/><path d="M8 4.6V8l2.4 1.6"/>',
  assistant:
    '<path d="M8 2.2l1.5 3.6 3.6 1.5-3.6 1.5L8 12.4 6.5 8.8 2.9 7.3l3.6-1.5z"/>',
  close: '<path d="M4.4 4.4l7.2 7.2M11.6 4.4l-7.2 7.2"/>',
};

/**
 * One icon, as an inline `<svg>`.
 *
 * `aria-hidden` because every icon here sits beside its own text label — a
 * screen reader announcing "database poc" would be reading the decoration
 * twice.
 */
export function icon(name: IconName): SVGElement {
  const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  el.setAttribute("viewBox", "0 0 16 16");
  el.setAttribute("fill", "none");
  el.setAttribute("stroke", "currentColor");
  el.setAttribute("stroke-width", "1.3");
  el.setAttribute("stroke-linecap", "round");
  el.setAttribute("stroke-linejoin", "round");
  el.setAttribute("aria-hidden", "true");
  // Names the icon for tests and for anyone reading the DOM. The alternative is
  // asserting on path data, which pins the drawing rather than the meaning.
  el.dataset.icon = name;
  // Static markup from the table above, never anything a server sent.
  el.innerHTML = PATHS[name];
  return el;
}
