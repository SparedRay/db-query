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
    // here on the dark theme, which is the bug this guards. The bar is 2.0
    // rather than "better than that": a first fix at 1.65 measured as an
    // improvement and was still reported as invisible on another display.
    const seen = contrast(band, ground);
    expect(seen, `selection ${c.selection} on ${ground.join(",")} is ${seen.toFixed(2)}:1`)
      .toBeGreaterThan(2);

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

/**
 * A **one-word** selection, measured in painted pixels.
 *
 * Every test above reads `getComputedStyle(band).backgroundColor` — the
 * selection element's own colour. That is a precise measurement of the wrong
 * thing: it says what the band was told to be, not what ended up on the screen.
 * Something painted on top of it is invisible to that check, and something was.
 *
 * CodeMirror draws the selection in a layer *behind* the lines, and
 * `highlightActiveLine` marks the line under the cursor head whether or not
 * the selection is empty. The active line's background was opaque
 * (`--bg-row-hover`), so it covered the band on the one line a short selection
 * is always on. Selecting a word looked like nothing happened; selecting
 * several lines worked, because only the head line was covered — which is
 * exactly how it was reported.
 *
 * So this one screenshots the band and reads the pixels back, and compares
 * them against the pixels immediately to its right on the same line. That is
 * the comparison a person's eye makes, and no amount of stacking can fool it.
 */
async function paintedRgb(
  page: Page,
  clip: { x: number; y: number; width: number; height: number },
): Promise<[number, number, number]> {
  const png = (await page.screenshot({ clip })).toString("base64");
  return page.evaluate(async (b64) => {
    const img = new Image();
    img.src = `data:image/png;base64,${b64}`;
    await img.decode();
    const canvas = document.createElement("canvas");
    canvas.width = img.width;
    canvas.height = img.height;
    const ctx = canvas.getContext("2d")!;
    ctx.drawImage(img, 0, 0);
    const data = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
    // The commonest pixel is the ground: glyphs and antialiasing are a
    // minority of any strip wide enough to hold a word.
    const seen = new Map<string, number>();
    for (let i = 0; i < data.length; i += 4) {
      const key = `${data[i]},${data[i + 1]},${data[i + 2]}`;
      seen.set(key, (seen.get(key) ?? 0) + 1);
    }
    let best = "";
    let most = -1;
    for (const [key, n] of seen) if (n > most) [best, most] = [key, n];
    return best.split(",").map(Number) as [number, number, number];
  }, png);
}

for (const theme of ["dark", "light"] as const) {
  test(`selecting a single word is visible — ${theme}`, async ({ page }) => {
    await connect(page, schemaBackend);
    await setTheme(page, theme);

    await page.locator("#editor .cm-content").click();
    await page.keyboard.press("Control+a");
    await page.keyboard.type("select alpha from beta;");
    // Double-click takes the word, and leaves the cursor on that line — so the
    // selection and the active-line highlight are on the same line, which is
    // the whole point.
    await page.dblclick("#editor .cm-content span:has-text('alpha')").catch(async () => {
      await page.dblclick("#editor .cm-line");
    });
    const band = page.locator(".cm-selectionBackground").first();
    await band.waitFor();
    const box = (await band.boundingBox())!;
    expect(box.width, "the word's selection band has a width").toBeGreaterThan(4);

    const selected = await paintedRgb(page, box);
    // The same line, just past the selection: active-line ground, no band.
    const beside = await paintedRgb(page, {
      x: box.x + box.width + 4,
      y: box.y,
      width: 24,
      height: box.height,
    });

    const seen = contrast(selected, beside);
    expect(
      seen,
      `painted selection ${selected.join(",")} against the line beside it ` +
        `${beside.join(",")} is ${seen.toFixed(2)}:1`,
    ).toBeGreaterThan(2);
  });
}
