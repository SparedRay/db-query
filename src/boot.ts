/**
 * The entry point, whose only job is to put the preferences in place before
 * the app reads them.
 *
 * # Why the app is not the entry point any more
 *
 * `main.ts` reads its preferences at module evaluation — `const settings =
 * loadSettings()` — because the font, the theme and the editor palette have to
 * be right in the first frame rather than applied over a window that has
 * already been painted wrong. That read is synchronous, so `localStorage` is
 * the only store it can come from.
 *
 * The record, though, is a file the Rust side owns (`src-tauri/src/prefs.rs`),
 * because a webview's storage belongs to its origin and this app has two of
 * them — one for `mise run dev`, one for the installed build. So the file is
 * copied into `localStorage` here, and only then is `main.ts` imported, which
 * is what makes its synchronous read see the right values.
 *
 * A dynamic import rather than top-level `await`: the latter is ES2022 and the
 * bundle targets ES2021, and the ordering is clearer written down than implied
 * by the module graph.
 */
import { api } from "./api";
import { KEY } from "./settings";

/** Surfaced by `main.ts` once there is somewhere to put a message. */
declare global {
  interface Window {
    __PREFS_WARNING__?: string;
  }
}

/**
 * How long the window will wait for the file.
 *
 * It is a small local read, so this is not a budget — it is a guarantee that a
 * backend which never answers cannot leave a blank window. Losing the race
 * means starting on whatever `localStorage` holds, which is the behaviour the
 * app had before the file existed.
 */
const PATIENCE_MS = 2000;

function timeout<T>(work: Promise<T>, ms: number): Promise<T | null> {
  return Promise.race([
    work,
    new Promise<null>((resolve) => window.setTimeout(() => resolve(null), ms)),
  ]);
}

async function seed(): Promise<void> {
  const stored = await timeout(api.prefsLoad(), PATIENCE_MS);
  if (!stored) return;
  if (stored.warning) window.__PREFS_WARNING__ = stored.warning;

  if (stored.settings) {
    try {
      localStorage.setItem(KEY, JSON.stringify(stored.settings));
    } catch {
      // Unwritable storage is not fatal: `load` falls back to the defaults,
      // which is what this build would have done anyway.
    }
    return;
  }

  // **Nothing stored yet.** Either this is a first launch, or it is the first
  // launch since preferences moved into the file — and in that second case
  // `localStorage` holds choices somebody made and should not have to make
  // again. Hand them up, once.
  let existing: string | null = null;
  try {
    existing = localStorage.getItem(KEY);
  } catch {
    return;
  }
  if (!existing) return;
  try {
    await api.prefsSave(JSON.parse(existing) as Record<string, unknown>);
  } catch {
    // A preference file we could not write is a preference file the next save
    // will write. Nothing is lost: the cache still holds it.
  }
}

// Whatever happens, the app starts. `finally` rather than `then`, so a failure
// here cannot be the reason somebody sees an empty window.
void seed().finally(() => {
  void import("./main");
});
