/**
 * The one context menu.
 *
 * Extracted from `connections.ts`, where it learned three things the hard way
 * and where the schema tree could not reach it:
 *
 *   * **Dismiss must ignore events inside the menu.** Tearing it down on any
 *     `mousedown` kills the button's `click`, which fires on mouseup — every
 *     item was silently unclickable for a whole stage.
 *   * **Escape closes it.** A menu with no keyboard exit is a trap.
 *   * **It must be clamped to the viewport**, which can only be measured once
 *     it is attached.
 *   * **A modal dialog owns the screen.** `showModal()` puts the dialog in the
 *     browser's top layer, above everything in the normal stacking order — so a
 *     menu appended to `document.body` is *painted* but not reachable: the
 *     dialog swallows the click. It has to be appended inside the dialog.
 *
 * One implementation, so the next fix lands everywhere — the same move
 * `choose()` made into `dialog.ts`.
 */

export interface MenuItem {
  label: string;
  run: () => void;
  /** Styled as destructive. Reserved for things that delete data. */
  danger?: boolean;
  /** A non-interactive separator label. `run` is ignored. */
  heading?: boolean;
  disabled?: boolean;
}

/** Open a context menu at the event's position. */
export function contextMenu(e: MouseEvent, items: MenuItem[]) {
  e.preventDefault();
  document.querySelector(".ctx-menu")?.remove();

  const menu = document.createElement("div");
  menu.className = "ctx-menu";
  menu.style.left = `${e.clientX}px`;
  menu.style.top = `${e.clientY}px`;

  for (const item of items) {
    if (item.heading) {
      const h = document.createElement("div");
      h.className = "ctx-heading";
      h.textContent = item.label;
      menu.append(h);
      continue;
    }
    const b = document.createElement("button");
    b.textContent = item.label;
    if (item.danger) b.className = "danger";
    if (item.disabled) b.disabled = true;
    b.onclick = () => {
      close();
      item.run();
    };
    menu.append(b);
  }

  // Inside the open modal, when there is one. No z-index can lift an element
  // out of the normal stacking order and into the top layer; being a descendant
  // of the dialog is the only way in.
  const host = document.querySelector("dialog[open]") ?? document.body;
  host.append(menu);

  // Measurable only once attached, which is why this runs after the append.
  const box = menu.getBoundingClientRect();
  if (box.bottom > window.innerHeight) {
    menu.style.top = `${Math.max(4, window.innerHeight - box.height - 4)}px`;
  }
  if (box.right > window.innerWidth) {
    menu.style.left = `${Math.max(4, window.innerWidth - box.width - 4)}px`;
  }

  // The containment check is the whole reason this works: without it the menu
  // is gone before the button it was clicked on ever sees the click.
  const dismiss = (ev: MouseEvent) => {
    if (menu.contains(ev.target as Node)) return;
    close();
  };
  const onKey = (ev: KeyboardEvent) => {
    if (ev.key === "Escape") close();
  };
  const close = () => {
    menu.remove();
    document.removeEventListener("mousedown", dismiss, true);
    document.removeEventListener("keydown", onKey);
  };
  document.addEventListener("mousedown", dismiss, true);
  document.addEventListener("keydown", onKey);
}
