// Preferences that outlive a session.
//
// Kept in localStorage rather than in the Rust config file: none of it is a
// secret, none of it is needed before the window exists, and the config file's
// job is connections. Reads are defensive — storage throws outright in some
// embeddings, and a value written by a future version must not break this one.

import type { Provider } from "./api";
import type { ThemePref } from "./theme";

export interface Settings {
  theme: ThemePref;
  /** A font stack, not a single family: an uninstalled font must fall back. */
  fontFamily: string;
  fontSize: number;
  /** Defaults applied at boot to the controls in the editor header. */
  autoLimit: boolean;
  lint: boolean;
  timeoutSecs: number;
  /** Rows for the generated "SELECT ... LIMIT n" browse query. */
  browseLimit: number;

  // --- assistant. Where questions go; the key lives in the keychain.
  aiProvider: Provider;
  /** Base URL, so any OpenAI-compatible server — including a local one — works. */
  aiBaseUrl: string;
  aiModel: string;
}

export const DEFAULT_MONO =
  'ui-monospace, "SF Mono", "JetBrains Mono", Menlo, Consolas, monospace';

/** Offered families, each ending in the default stack so a miss falls back. */
export const FONTS: Array<{ label: string; stack: string }> = [
  { label: "System monospace", stack: DEFAULT_MONO },
  { label: "JetBrains Mono", stack: `"JetBrains Mono", ${DEFAULT_MONO}` },
  { label: "Fira Code", stack: `"Fira Code", ${DEFAULT_MONO}` },
  { label: "Cascadia Code", stack: `"Cascadia Code", "Cascadia Mono", ${DEFAULT_MONO}` },
  { label: "Consolas", stack: `Consolas, ${DEFAULT_MONO}` },
  { label: "Courier New", stack: `"Courier New", ${DEFAULT_MONO}` },
];

export const DEFAULTS: Settings = {
  theme: "system",
  fontFamily: DEFAULT_MONO,
  fontSize: 12,
  autoLimit: true,
  lint: true,
  timeoutSecs: 0,
  browseLimit: 1000,
  aiProvider: "anthropic",
  aiBaseUrl: "https://api.anthropic.com",
  aiModel: "claude-opus-5",
};

/**
 * One-click starting points. Every entry but the first speaks the OpenAI
 * shape — which is why a second provider was cheap and a fifth is free.
 *
 * The model is left to the user wherever we cannot know it: which models a
 * local server has pulled is a property of that machine, and guessing produces
 * a confident 404.
 */
export const AI_PRESETS: Array<{
  label: string;
  provider: Provider;
  baseUrl: string;
  model: string;
  /** Placeholder when `model` is blank, so the field is not a mystery. */
  modelHint: string;
}> = [
  {
    label: "Anthropic",
    provider: "anthropic",
    baseUrl: "https://api.anthropic.com",
    model: "claude-opus-5",
    modelHint: "claude-opus-5",
  },
  {
    label: "OpenAI",
    provider: "openAiCompatible",
    baseUrl: "https://api.openai.com/v1",
    model: "",
    modelHint: "the model name from your account",
  },
  {
    // Google's own OpenAI-compatibility layer, so this costs a preset rather
    // than an adapter: the key travels as `Authorization: Bearer`, which is
    // what the OpenAI path already sends. Google calls it beta and silently
    // ignores parameters it does not support — which for us is none, since we
    // send a model, messages and `stream`.
    label: "Google Gemini",
    provider: "openAiCompatible",
    baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai/",
    model: "",
    modelHint: "a model from your account, e.g. gemini-2.5-pro",
  },
  {
    label: "Ollama (local)",
    provider: "openAiCompatible",
    baseUrl: "http://localhost:11434/v1",
    model: "",
    modelHint: "a model you have pulled, e.g. llama3.1",
  },
  {
    label: "LM Studio (local)",
    provider: "openAiCompatible",
    baseUrl: "http://localhost:1234/v1",
    model: "",
    modelHint: "the model loaded in LM Studio",
  },
  {
    label: "Other OpenAI-compatible",
    provider: "openAiCompatible",
    baseUrl: "",
    model: "",
    modelHint: "the model name your server expects",
  },
];

const KEY = "db-query.settings";

/**
 * A number inside the range the UI can actually produce, or the default.
 *
 * Deliberately not a clamp. These values only ever come from our own controls
 * or from corruption, and 900 pinned to 22 is a size nobody chose — a reset is
 * more honest than an approximation of a value that was never meant.
 */
/** A string field, taken only when it really is a string. */
const text = (v: unknown, fallback: string): string =>
  typeof v === "string" ? v : fallback;

const provider = (v: unknown): Provider =>
  v === "openAiCompatible" || v === "anthropic" ? v : DEFAULTS.aiProvider;

const bounded = (n: number, lo: number, hi: number, fallback: number) =>
  Number.isFinite(n) && n >= lo && n <= hi ? Math.round(n) : fallback;

/**
 * Read what is stored, field by field.
 *
 * Deliberately not `{...DEFAULTS, ...parsed}`: that trusts every value in
 * storage, and one bad number — a font size of 400, a negative timeout — would
 * make the app unusable with no way back through the UI it broke.
 */
export function load(): Settings {
  let raw: unknown;
  try {
    raw = JSON.parse(localStorage.getItem(KEY) ?? "null");
  } catch {
    return { ...DEFAULTS };
  }
  if (!raw || typeof raw !== "object") return { ...DEFAULTS };
  const o = raw as Record<string, unknown>;
  const known = FONTS.some((f) => f.stack === o.fontFamily);
  return {
    // The theme has its own key and its own module; this field is only here so
    // the settings dialog can show it in one place.
    theme:
      o.theme === "light" || o.theme === "dark" || o.theme === "system"
        ? o.theme
        : DEFAULTS.theme,
    fontFamily: known ? (o.fontFamily as string) : DEFAULTS.fontFamily,
    fontSize: bounded(Number(o.fontSize), 9, 22, DEFAULTS.fontSize),
    autoLimit: typeof o.autoLimit === "boolean" ? o.autoLimit : DEFAULTS.autoLimit,
    lint: typeof o.lint === "boolean" ? o.lint : DEFAULTS.lint,
    timeoutSecs: bounded(Number(o.timeoutSecs), 0, 3600, DEFAULTS.timeoutSecs),
    browseLimit: bounded(Number(o.browseLimit), 1, 1_000_000, DEFAULTS.browseLimit),
    aiProvider: provider(o.aiProvider),
    aiBaseUrl: text(o.aiBaseUrl, DEFAULTS.aiBaseUrl),
    aiModel: text(o.aiModel, DEFAULTS.aiModel),
  };
}

export function save(s: Settings) {
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    // Unavailable storage means the change applies to this session only, which
    // is better than refusing to apply it at all.
  }
}

/**
 * Push the visual settings into CSS, where the editor and the grid both read
 * them. Everything else is applied by whoever owns that control.
 */
export function applyAppearance(s: Settings) {
  const root = document.documentElement;
  root.style.setProperty("--mono", s.fontFamily);
  root.style.setProperty("--code-size", `${s.fontSize}px`);
}
