// Light and dark, and the third option that is neither.
//
// `data-theme` on <html> is always set to a *resolved* value — "light" or
// "dark", never "system" — so the stylesheet needs one override block and no
// media query. Resolving here rather than in CSS also means the editor can be
// told which way it went, which it cannot work out for itself.

export type ThemePref = "system" | "light" | "dark";

const KEY = "db-query.theme";

/** Cycle order for the toggle: what the button does next. */
const NEXT: Record<ThemePref, ThemePref> = {
  system: "light",
  light: "dark",
  dark: "system",
};

const LABEL: Record<ThemePref, string> = {
  system: "Theme: follow system",
  light: "Theme: light",
  dark: "Theme: dark",
};

const ICON: Record<ThemePref, string> = {
  system: "◐", // half-filled circle
  light: "☀", // sun
  dark: "☽", // moon
};

/**
 * The stored preference.
 *
 * Reads defensively: `localStorage` throws outright in some embeddings, and a
 * value written by a future version must not break this one.
 */
export function preference(): ThemePref {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch {
    /* no storage: fall through to the default */
  }
  return "system";
}

/** What `system` currently means. */
export function systemIsDark(): boolean {
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? true;
}

/** Does this preference resolve to a dark palette right now? */
export function resolvesDark(pref: ThemePref): boolean {
  return pref === "dark" || (pref === "system" && systemIsDark());
}

export interface Theme {
  /** Apply the stored preference. Safe to call more than once. */
  apply: () => void;
  /** Advance system -> light -> dark -> system, persist, and apply. */
  cycle: () => void;
  current: () => ThemePref;
}

/**
 * @param onChange told whether the *resolved* theme is dark, for anything that
 * cannot follow CSS variables on its own — the editor, in practice.
 */
export function createTheme(button: HTMLButtonElement, onChange: (dark: boolean) => void): Theme {
  let pref = preference();

  const apply = () => {
    const dark = resolvesDark(pref);
    document.documentElement.dataset.theme = dark ? "dark" : "light";
    button.textContent = ICON[pref];
    button.title = `${LABEL[pref]} — click to change`;
    button.setAttribute("aria-label", LABEL[pref]);
    // Exposed for tests and for anyone reading the DOM: the *preference* is not
    // recoverable from `data-theme` alone, since "system" resolves to one of
    // the other two.
    button.dataset.pref = pref;
    onChange(dark);
  };

  const cycle = () => {
    pref = NEXT[pref];
    try {
      localStorage.setItem(KEY, pref);
    } catch {
      // Storage can be unavailable. The theme still changes for this session;
      // silently doing nothing at all would be the worse failure.
    }
    apply();
  };

  button.onclick = cycle;

  // Following the system means following it as it changes, not only at boot.
  window.matchMedia?.("(prefers-color-scheme: dark)").addEventListener("change", () => {
    if (pref === "system") apply();
  });

  return { apply, cycle, current: () => pref };
}
