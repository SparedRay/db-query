// A very small Markdown renderer, for text we did not write.
//
// # Why this exists
//
// Release notes are authored as Markdown — they are a GitHub release body —
// and the update dialog was showing them raw: `## Install`, `**Windows**` and
// stray backticks, which looks like a bug in the app rather than a document it
// failed to draw.
//
// # Why it builds nodes instead of setting innerHTML
//
// **The notes arrive from the network.** They come out of `latest.json` at the
// updater endpoint, and only the *installer bytes* are signature-checked —
// nothing verifies this text. So the one thing this file must never do is hand
// a remote string to a parser that can create elements: no `innerHTML`, no
// `insertAdjacentHTML`, no `DOMParser`. Every node here is made by
// `createElement` and every piece of source text lands in a `Text` node, which
// cannot become markup however it is spelled.
//
// That is also why there is no dependency. A Markdown library would be far more
// capable, and would bring a licence to audit, a supply chain to trust and an
// HTML pipeline to sanitise — to draw four constructs in one dialog.
//
// # What it deliberately does not do
//
// Links are rendered as **text**, never as `<a href>`. A click on a real anchor
// inside a Tauri webview navigates the app window away from the app; there is
// no tab to land in. The address is shown in brackets so it can be read and
// copied, which is all the dialog needs.
//
// Anything unrecognised is left exactly as written. An unclosed `**`, a table,
// a blockquote: they come out as their own characters rather than disappearing.
// Showing a construct verbatim is a much smaller failure than swallowing it.

/**
 * Render `source` as a fragment of ordinary DOM.
 *
 * Block level: fenced code, ATX headings, bullet and numbered lists,
 * paragraphs. Inline: code spans, bold, italic, links-as-text.
 */
export function renderMarkdown(source: string): DocumentFragment {
  const out = document.createDocumentFragment();
  const lines = source.replace(/\r\n?/g, "\n").split("\n");
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];

    if (!line.trim()) {
      i++;
      continue;
    }

    // Fenced code. Everything until the closing fence is literal — no inline
    // pass over it, which is the whole point of a code block.
    if (/^\s*```/.test(line)) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !/^\s*```/.test(lines[i])) body.push(lines[i++]);
      // A missing closing fence runs to the end rather than throwing the rest
      // of the notes away.
      if (i < lines.length) i++;
      const pre = document.createElement("pre");
      const code = document.createElement("code");
      code.textContent = body.join("\n");
      pre.append(code);
      out.append(pre);
      continue;
    }

    // ATX headings. `h3`/`h4` and no higher: the dialog's own title is an
    // `h2`, and a document heading must not outrank the question being asked.
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      const el = document.createElement(heading[1].length <= 2 ? "h3" : "h4");
      inline(heading[2].trim(), el);
      out.append(el);
      i++;
      continue;
    }

    const bullet = /^\s*[-*]\s+(.*)$/;
    const numbered = /^\s*\d+[.)]\s+(.*)$/;
    const pattern = bullet.test(line) ? bullet : numbered.test(line) ? numbered : null;
    if (pattern) {
      const list = document.createElement(pattern === bullet ? "ul" : "ol");
      while (i < lines.length) {
        const item = pattern.exec(lines[i]);
        if (!item) break;
        const li = document.createElement("li");
        inline(item[1], li);
        list.append(li);
        i++;
      }
      out.append(list);
      continue;
    }

    // A paragraph: every line until a blank one or the start of another block.
    //
    // **Wrapped lines are joined with a space** rather than kept as breaks.
    // GitHub renders a single newline as `<br>`, so this does differ from the
    // release page — deliberately. The wrapping in a release body is an
    // artefact of the file it was typed into, at a width this dialog does not
    // have; preserving it here produces a ragged column that then wraps again.
    // Structure — headings, lists, code — is where the meaning is, and that is
    // kept exactly.
    const para: string[] = [];
    while (i < lines.length && lines[i].trim() && !startsABlock(lines[i])) {
      para.push(lines[i].trim());
      i++;
    }
    const p = document.createElement("p");
    inline(para.join(" "), p);
    out.append(p);
  }

  return out;
}

/** Would this line begin a block of its own, and so end the paragraph above? */
function startsABlock(line: string): boolean {
  return (
    /^\s*```/.test(line) ||
    /^#{1,6}\s+/.test(line) ||
    /^\s*[-*]\s+/.test(line) ||
    /^\s*\d+[.)]\s+/.test(line)
  );
}

/**
 * Inline constructs, in the order they are recognised.
 *
 * Code first and terminal: the contents of a code span are never looked at
 * again, so `` `a_b_c` `` keeps its underscores.
 *
 * **A source, not a shared instance.** `inline` recurses — bold can contain a
 * code span — and a `g` regex carries `lastIndex` with it, so one object shared
 * between the outer scan and the inner one would have the inner call rewind the
 * outer. Each call compiles its own.
 */
const INLINE_SOURCE =
  /`([^`]+)`|\*\*([\s\S]+?)\*\*|__([\s\S]+?)__|\*([^*\n]+)\*|_([^_\n]+)_|\[([^\]]*)\]\(([^()\s]*)\)/
    .source;

/**
 * Append `text` to `parent`, turning inline markers into elements.
 *
 * Everything that is not a recognised marker becomes a `Text` node, so a
 * string like `<img onerror=...>` arrives on screen as those characters and
 * never as an element.
 */
function inline(text: string, parent: Element) {
  let at = 0;
  const scan = new RegExp(INLINE_SOURCE, "g");
  let m: RegExpExecArray | null;

  while ((m = scan.exec(text)) !== null) {
    if (m.index > at) parent.append(text.slice(at, m.index));
    const [, code, strongStar, strongUnder, emStar, emUnder, linkText, href] = m;

    if (code !== undefined) {
      const el = document.createElement("code");
      el.textContent = code;
      parent.append(el);
    } else if (strongStar !== undefined || strongUnder !== undefined) {
      const el = document.createElement("strong");
      inline(strongStar ?? strongUnder, el);
      parent.append(el);
    } else if (emStar !== undefined || emUnder !== undefined) {
      const el = document.createElement("em");
      inline(emStar ?? emUnder, el);
      parent.append(el);
    } else {
      // A link, as text. See the note at the top: an anchor in a Tauri webview
      // navigates the app away from itself. The address is shown so it can be
      // read and copied.
      const label = linkText.trim();
      parent.append(label && href ? `${label} (${href})` : label || href);
    }
    at = m.index + m[0].length;
  }

  if (at < text.length) parent.append(text.slice(at));
}
