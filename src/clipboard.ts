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

/**
 * Copy, say what happened, then get out of the way.
 *
 * `copyText` throws rather than returning false precisely so a caller cannot
 * ignore a failure by accident — but four call sites did ignore it, by
 * writing `void copyText(…)`. The outcome has to land *somewhere*, and where
 * differs: a button can say it on itself, a context-menu item has no element
 * left to say it on by the time the copy finishes. So this owns the part that
 * is always the same — await, report, restore — and the caller owns the
 * surface.
 *
 * `say(null)` is the restore, and it is called whatever happened. A Copy that
 * is still saying "Copied" a minute later is a button that has stopped
 * meaning anything.
 */
export type CopyOutcome = { ok: true } | { ok: false; error: string };

export function copyAndReport(
  text: string,
  say: (outcome: CopyOutcome | null) => void,
  ms = 2000,
): void {
  void copyText(text)
    .then(() => say({ ok: true }))
    .catch((err: unknown) => say({ ok: false, error: String(err) }))
    .finally(() => {
      window.setTimeout(() => say(null), ms);
    });
}

/**
 * The common case: a button that reports on itself.
 *
 * The label and tooltip it was built with are captured once and restored
 * after, so the button goes back to being what it was. An empty string is not
 * copied at all: there is nothing to put on the clipboard, and "Copied" would
 * be a lie about it.
 */
export function wireCopyButton(button: HTMLButtonElement, text: () => string): void {
  const label = button.textContent ?? "Copy";
  const title = button.title;
  // A second press takes the button over. Without this the first press's
  // restore — already scheduled, and two seconds behind — would fire during
  // the second one and put the label back while it was still saying
  // something.
  let generation = 0;
  button.onclick = () => {
    const payload = text();
    if (!payload) return;
    const mine = ++generation;
    copyAndReport(payload, (outcome) => {
      if (mine !== generation) return;
      if (outcome === null) {
        button.textContent = label;
        button.title = title;
        return;
      }
      button.textContent = outcome.ok ? "Copied" : "Copy failed";
      button.title = outcome.ok ? title : outcome.error;
    });
  };
}
