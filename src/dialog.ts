// Modal questions, asked with our own <dialog> rather than window.confirm.
//
// Tauri routes window.confirm/alert/prompt through the dialog plugin, which
// needs its own permission — and without it the call rejects and the caller
// silently does nothing. Ours needs no permission, matches the rest of the UI,
// and can offer more than two answers.

import { copyText } from "./clipboard";

export interface Choice<T extends string> {
  value: T;
  label: string;
  primary?: boolean;
  danger?: boolean;
}

/**
 * Ask a question. Resolves to the chosen value, or `null` if dismissed —
 * Escape and backdrop dismissal both mean "cancel", never "go ahead".
 */
export function choose<T extends string>(
  title: string,
  message: string,
  choices: Choice<T>[],
): Promise<T | null> {
  return new Promise((resolve) => {
    const dlg = document.createElement("dialog");
    dlg.className = "ask";

    const h = document.createElement("h2");
    h.textContent = title;
    const p = document.createElement("p");
    // textContent, not innerHTML: these messages carry file names, connection
    // names and server errors, none of which we control.
    p.textContent = message;
    const menu = document.createElement("menu");

    let settled = false;
    const done = (value: T | null) => {
      if (settled) return;
      settled = true;
      dlg.close();
      dlg.remove();
      resolve(value);
    };

    for (const c of choices) {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = c.label;
      if (c.primary) b.classList.add("primary");
      if (c.danger) b.classList.add("danger");
      b.onclick = () => done(c.value);
      menu.append(b);
    }

    dlg.append(h, p, menu);
    dlg.addEventListener("cancel", (e) => {
      e.preventDefault();
      done(null);
    });
    document.body.append(dlg);
    dlg.showModal();
    (menu.querySelector("button.primary") as HTMLButtonElement | null)?.focus();
  });
}

/**
 * Show one cell's whole value.
 *
 * The grid ellipsises every cell, so a JSON document or a paragraph of text is
 * unreadable in the table and there was no way to reach the rest of it. This is
 * a viewer and nothing more: it never writes anything back.
 */
export function showValue(title: string, value: string | null) {
  const dlg = document.createElement("dialog");
  dlg.className = "viewer";

  const h = document.createElement("h2");
  h.textContent = title;

  const meta = document.createElement("p");
  meta.className = "viewer-meta";

  const body = document.createElement("pre");
  body.className = "viewer-body";

  const menu = document.createElement("menu");
  const note = document.createElement("span");
  note.className = "viewer-note";

  const close = () => {
    dlg.close();
    dlg.remove();
  };

  if (value === null) {
    // A real NULL, which must not be confused with the four-character string.
    meta.textContent = "SQL NULL — not the text “NULL”.";
    body.classList.add("is-null");
    body.textContent = "NULL";
  } else {
    const parsed = asJsonDocument(value);
    // Formatted by default when it is JSON: that is the whole point of opening
    // it. `Raw` is one click away and shows the bytes exactly as stored.
    let formatted = parsed !== undefined;

    const render = () => {
      body.textContent =
        formatted && parsed !== undefined ? JSON.stringify(parsed, null, 2) : value;
      meta.textContent =
        `${value.length.toLocaleString()} characters` +
        (parsed !== undefined ? (formatted ? " · JSON, formatted" : " · JSON, raw") : "");
    };

    if (parsed !== undefined) {
      const toggle = document.createElement("button");
      toggle.type = "button";
      toggle.textContent = "Raw";
      toggle.onclick = () => {
        formatted = !formatted;
        toggle.textContent = formatted ? "Raw" : "Format";
        render();
      };
      menu.append(toggle);
    }

    const copy = document.createElement("button");
    copy.type = "button";
    copy.textContent = "Copy";
    copy.onclick = () => {
      // Copies what is on screen: formatting it and then copying something
      // else would be a small lie the user cannot see.
      void copyText(body.textContent ?? "")
        .then(() => {
          note.textContent = "Copied.";
        })
        .catch((err: unknown) => {
          note.textContent = String(err);
        });
    };
    menu.append(copy);
    render();
  }

  const closeBtn = document.createElement("button");
  closeBtn.type = "button";
  closeBtn.className = "primary";
  closeBtn.textContent = "Close";
  closeBtn.onclick = close;
  menu.append(note, closeBtn);

  dlg.append(h, meta, body, menu);
  dlg.addEventListener("cancel", (e) => {
    e.preventDefault();
    close();
  });
  document.body.append(dlg);
  dlg.showModal();
  closeBtn.focus();
}

/**
 * Parse `text` as a JSON **document**, or `undefined`.
 *
 * Deliberately requires an object or an array. `JSON.parse` happily accepts
 * `123` and `true`, and offering to "format" a bare number would be noise on
 * every integer column in the database.
 */
function asJsonDocument(text: string): unknown | undefined {
  const t = text.trim();
  if (!t.startsWith("{") && !t.startsWith("[")) return undefined;
  try {
    return JSON.parse(t) as unknown;
  } catch {
    return undefined;
  }
}
