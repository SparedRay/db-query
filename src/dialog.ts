// Modal questions, asked with our own <dialog> rather than window.confirm.
//
// Tauri routes window.confirm/alert/prompt through the dialog plugin, which
// needs its own permission — and without it the call rejects and the caller
// silently does nothing. Ours needs no permission, matches the rest of the UI,
// and can offer more than two answers.

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
