import { expect, test, type Page } from "@playwright/test";
import { connect, schemaBackend } from "./harness";

/**
 * That a selection is actually visible.
 *
 * This exists because it was not. The theme rule meant to colour the editor's
 * selection was written as `&.cm-focused .cm-selectionBackground` — three
 * classes — and CodeMirror's own base theme has
 * `&dark.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground`,
 * which is five. Ours never applied, and the default it lost to is `#233`: on
 * the dark theme's `#16181d` background that is a contrast ratio of 1.35, so
 * Ctrl+A appeared to do nothing at all. On the light theme the same bug was
 * invisible, because the default it fell back to there happens to be a
 * perfectly reasonable lavender.
 *
 * So the assertion is a **measured contrast ratio**, not a colour. A colour
 * check would pass again the moment someone renamed a token; this fails
 * whenever the selection stops being something a person can see, however that
 * happens.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

/** WCAG relative luminance, and the ratio between two of them. */
function contrast(a: [number, number, number], b: [number, number, number]): number {
  const lum = (rgb: [number, number, number]) => {
    const [r, g, bl] = rgb.map((v) => {
      const x = v / 255;
      return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [x, y] = [lum(a), lum(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

function rgb(css: string): [number, number, number] {
  const m = css.match(/(\d+(?:\.\d+)?)/g);
  if (!m || m.length < 3) throw new Error(`not a colour: ${css}`);
  return [Number(m[0]), Number(m[1]), Number(m[2])];
}

async function setTheme(page: Page, theme: "dark" | "light") {
  await page.click("#btn-settings");
  await page.selectOption("#set-theme", theme);
  await page.click("#set-close");
}

/** The painted selection band, the editor's ground, and the text on top. */
async function selectionColours(page: Page) {
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+a");
  await page.locator(".cm-selectionBackground").first().waitFor();
  return page.evaluate(() => {
    const band = document.querySelector(".cm-selectionBackground")!;
    const line = document.querySelector("#editor .cm-line")!;
    return {
      selection: getComputedStyle(band).backgroundColor,
      background: getComputedStyle(document.querySelector("#editor .cm-content")!).backgroundColor,
      // The editor paints its ground on `.cm-editor`; the content is
      // transparent over it.
      editor: getComputedStyle(document.querySelector(".cm-editor")!).backgroundColor,
      text: getComputedStyle(line).color,
    };
  });
}

for (const theme of ["dark", "light"] as const) {
  test(`Ctrl+A paints a selection you can see — ${theme}`, async ({ page }) => {
    await connect(page, schemaBackend);
    await setTheme(page, theme);

    const c = await selectionColours(page);
    const ground = rgb(c.background === "rgba(0, 0, 0, 0)" ? c.editor : c.background);
    const band = rgb(c.selection);

    // Not a colour, a visible difference. CodeMirror's own default scores 1.35
    // here on the dark theme, which is the bug this guards.
    const seen = contrast(band, ground);
    expect(seen, `selection ${c.selection} on ${ground.join(",")} is ${seen.toFixed(2)}:1`)
      .toBeGreaterThan(1.6);

    // And the text has to stay readable on top of it, which is the constraint
    // that stops "make it brighter" from being the whole answer.
    const readable = contrast(rgb(c.text), band);
    expect(readable, `text ${c.text} on selection ${c.selection} is ${readable.toFixed(2)}:1`)
      .toBeGreaterThan(4.5);
  });
}

/** The selection is drawn by CodeMirror, so it must survive a theme switch. */
test("switching theme repaints the selection", async ({ page }) => {
  await connect(page, schemaBackend);
  await setTheme(page, "dark");
  const dark = (await selectionColours(page)).selection;
  await setTheme(page, "light");
  const light = (await selectionColours(page)).selection;
  expect(light).not.toBe(dark);
});
