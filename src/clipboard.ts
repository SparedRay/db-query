/**
 * Putting text on the clipboard, without a silent failure.
 *
 * Stage 2 lost a whole delete feature to `window.confirm`, which Tauri
 * intercepts and routes to a plugin we had not permitted: the call rejected,
 * nothing happened, and there was no symptom at all. The clipboard is the same
 * shape of risk — `navigator.clipboard` needs a secure context and a user
 * gesture, and WebKitGTK has historically been the weakest of the three
 * platforms here.
 *
 * So this tries the modern API, falls back to the old one, and **throws** if
 * both fail. The caller shows the error. What must never happen is a Copy that
 * quietly does nothing.
 */

export type CopyMethod = "clipboard-api" | "exec-command";

/** Copy `text`, returning which mechanism worked. Throws if neither does. */
export async function copyText(text: string): Promise<CopyMethod> {
  const problems: string[] = [];

  // The modern path. Requires a secure context; our dev URL is localhost and
  // the bundled app is served from a scheme Tauri registers as secure, so this
  // should be the one that runs.
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return "clipboard-api";
    }
    problems.push("navigator.clipboard is not available");
  } catch (err) {
    problems.push(`navigator.clipboard failed: ${err}`);
  }

  // Deprecated, but it works in non-secure contexts and in older WebKit, which
  // is exactly the situation the modern path fails in.
  try {
    if (execCommandCopy(text)) return "exec-command";
    problems.push("document.execCommand('copy') returned false");
  } catch (err) {
    problems.push(`document.execCommand('copy') failed: ${err}`);
  }

  throw new Error(`Could not copy to the clipboard. ${problems.join("; ")}.`);
}

function execCommandCopy(text: string): boolean {
  const ta = document.createElement("textarea");
  ta.value = text;
  // Off-screen but focusable: `display:none` and `hidden` both make the
  // selection fail, which is the classic way this fallback silently breaks.
  ta.setAttribute("readonly", "");
  ta.style.position = "fixed";
  ta.style.top = "-1000px";
  ta.style.opacity = "0";
  document.body.append(ta);
  try {
    ta.select();
    ta.setSelectionRange(0, ta.value.length);
    return document.execCommand("copy");
  } finally {
    ta.remove();
  }
}
