import { expect, test, type Page } from "@playwright/test";
import { connect, schemaBackend } from "./harness";

/**
 * That every syntax colour is readable on the ground it is drawn on.
 *
 * This exists because none of them were. The editor used CodeMirror's
 * `defaultHighlightStyle`, whose colours are fixed and written for a light
 * background, on **both** themes: on the dark theme keywords measured 1.81:1
 * against `--bg`, comments 2.69 and numbers 2.54. Nothing reported it, because
 * the app's own chrome followed the theme correctly and the text merely looked
 * dim rather than broken.
 *
 * So the assertion is a measured contrast ratio for every token in every
 * preset on both grounds — 48 of them. A colour check would say nothing about
 * whether anyone can read the result.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

/** Presets, and the bar each is held to. High contrast has to earn its name. */
const PRESETS = [
  { value: "default", label: "Default", bar: 4.5 },
  { value: "muted", label: "Muted", bar: 4.5 },
  { value: "contrast", label: "High contrast", bar: 7 },
] as const;

const TOKENS = [
  "--syn-keyword",
  "--syn-string",
  "--syn-number",
  "--syn-comment",
  "--syn-name",
  "--syn-type",
  "--syn-function",
  "--syn-punct",
] as const;

function contrast(a: [number, number, number], b: [number, number, number]): number {
  const lum = (c: [number, number, number]) => {
    const [r, g, bl] = c.map((v) => {
      const x = v / 255;
      return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [x, y] = [lum(a), lum(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

/** `#rrggbb` or `rgb(...)`, whichever the browser hands back. */
function rgb(css: string): [number, number, number] {
  const v = css.trim();
  if (v.startsWith("#")) {
    const h = v.slice(1);
    return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)) as [number, number, number];
  }
  const m = v.match(/(\d+(?:\.\d+)?)/g);
  if (!m || m.length < 3) throw new Error(`not a colour: ${css}`);
  return [Number(m[0]), Number(m[1]), Number(m[2])];
}

async function choose(page: Page, theme: "dark" | "light", preset: string) {
  await page.click("#btn-settings");
  await page.selectOption("#set-theme", theme);
  await page.selectOption("#set-editor-colours", preset);
  await page.click("#set-close");
}

/** Every `--syn-*` token, and the editor's own background. */
async function palette(page: Page) {
  return page.evaluate((tokens: readonly string[]) => {
    const root = getComputedStyle(document.documentElement);
    const out: Record<string, string> = {};
    for (const t of tokens) out[t] = root.getPropertyValue(t).trim();
    return { tokens: out, background: root.getPropertyValue("--bg").trim() };
  }, TOKENS);
}

for (const preset of PRESETS) {
  for (const theme of ["dark", "light"] as const) {
    test(`${preset.label} is readable on the ${theme} theme`, async ({ page }) => {
      await connect(page, schemaBackend);
      await choose(page, theme, preset.value);

      const { tokens, background } = await palette(page);
      const ground = rgb(background);

      for (const [name, colour] of Object.entries(tokens)) {
        expect(colour, `${name} is not defined for ${preset.value}/${theme}`).toBeTruthy();
        const ratio = contrast(rgb(colour), ground);
        expect(
          ratio,
          `${name} ${colour} on ${background} is ${ratio.toFixed(2)}:1 (want ${preset.bar})`,
        ).toBeGreaterThanOrEqual(preset.bar);
      }
    });
  }
}

/** The colours have to reach the editor, not merely exist on :root. */
test("the preset reaches the text in the editor", async ({ page }) => {
  await connect(page, schemaBackend);

  await choose(page, "dark", "default");
  const before = await keywordColour(page);
  await choose(page, "dark", "contrast");
  const after = await keywordColour(page);

  expect(after).not.toBe(before);
  // And it is the token's value, not something CodeMirror chose.
  const { tokens } = await palette(page);
  expect(rgb(after)).toEqual(rgb(tokens["--syn-keyword"]));
});

/** The starter document opens with `SELECT`, which is the keyword to sample. */
async function keywordColour(page: Page): Promise<string> {
  return page.evaluate(() => {
    for (const el of document.querySelectorAll("#editor .cm-line span")) {
      if (el.textContent?.toUpperCase() === "SELECT") return getComputedStyle(el).color;
    }
    throw new Error("no SELECT keyword in the editor to sample");
  });
}

/** A preset is a preference like any other: it has to survive a reload. */
test("the chosen preset comes back after a restart", async ({ page }) => {
  await connect(page, schemaBackend);
  await choose(page, "dark", "muted");
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-editor-theme", "muted");
});

/** Default sets no attribute, so its palette is the plain `:root` block. */
test("Default leaves no attribute behind", async ({ page }) => {
  await connect(page, schemaBackend);
  await choose(page, "dark", "muted");
  await expect(page.locator("html")).toHaveAttribute("data-editor-theme", "muted");

  await choose(page, "dark", "default");
  expect(await page.locator("html").getAttribute("data-editor-theme")).toBeNull();
});
