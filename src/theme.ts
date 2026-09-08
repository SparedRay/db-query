// Light and dark, and the third option that is neither.
//
// `data-theme` on <html> is always set to a *resolved* value — "light" or
// "dark", never "system" — so the stylesheet needs one override block and no
// media query. Resolving here rather than in CSS also means the editor can be
// told which way it went, which it cannot work out for itself.

export type ThemePref = "system" | "light" | "dark";

const KEY = "db-query.theme";

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
  /** Apply the current preference. Safe to call more than once. */
  apply: () => void;
  /** Choose explicitly, persist, and apply. */
  set: (pref: ThemePref) => void;
  current: () => ThemePref;
}

/**
 * @param onChange told whether the *resolved* theme is dark, for anything that
 * cannot follow CSS variables on its own — the editor, in practice.
 */
export function createTheme(onChange: (dark: boolean) => void): Theme {
  let pref = preference();

  const apply = () => {
    const dark = resolvesDark(pref);
    document.documentElement.dataset.theme = dark ? "dark" : "light";
    // The *preference* is not recoverable from `data-theme` alone, since
    // "system" resolves to one of the other two. Exposed for tests and for
    // anyone reading the DOM.
    document.documentElement.dataset.themePref = pref;
    onChange(dark);
  };

  const set = (next: ThemePref) => {
    pref = next;
    try {
      localStorage.setItem(KEY, pref);
    } catch {
      // Storage can be unavailable. The theme still changes for this session;
      // silently doing nothing at all would be the worse failure.
    }
    apply();
  };

  // Following the system means following it as it changes, not only at boot.
  window.matchMedia?.("(prefers-color-scheme: dark)").addEventListener("change", () => {
    if (pref === "system") apply();
  });

  return { apply, set, current: () => pref };
}
