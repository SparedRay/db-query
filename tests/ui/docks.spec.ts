import { expect, test, type Page } from "@playwright/test";
import { connect } from "./harness";

/**
 * Every dock, held to the same head (Stage 31 §6.16).
 *
 * Four panes dock on the right — migrations, the assistant, the performance
 * panel, folder runs — and they arrived one stage at a time. Tested one stage
 * at a time too, which is how they ended up with three heads between them: two
 * closed on a `×` and two on the word "Close", and the two newest could take
 * the whole window while the two oldest could not. Nobody reading one panel's
 * tests could have noticed; it took opening them side by side.
 *
 * So the assertions here are deliberately *about the set*. They loop over the
 * four docks rather than naming one, because the defect was never in a dock —
 * it was in the gaps between them, and a test that visits one dock cannot see
 * a gap.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

/** The four docks: the pane, its rail button, and what its head calls it. */
const DOCKS = [
  { name: "mig", pane: "#mig-pane", rail: "#btn-migrations", title: "Migrations" },
  { name: "chat", pane: "#chat-pane", rail: "#btn-assistant", title: "Assistant" },
  { name: "wl", pane: "#wl-pane", rail: "#btn-workload", title: "Where the time goes" },
  { name: "batch", pane: "#batch-pane", rail: "#btn-sqlfiles", title: "Folder runs" },
  { name: "hist", pane: "#hist-pane", rail: "#btn-history", title: "Query history" },
] as const;

/**
 * The docks that can be opened with nothing stubbed but a connection.
 *
 * The performance panel needs a server with digest counters and folder runs
 * needs files; both are exercised where their fixtures live. What is *not*
 * tested there, and is here, is that the mechanism reaches the docks that had
 * been left out of it.
 */
const PLAIN = DOCKS.filter((d) => d.name !== "wl" && d.name !== "batch");

const ASSISTANT_READY = { hasKey: true, ready: true, local: false };

async function app(page: Page) {
  await connect(page, {
    assistant_status: () => ASSISTANT_READY,
    history_search: () => [],
  });
}

test("every dock's head ends with the same two controls, in the same order", async ({
  page,
}) => {
  await app(page);

  for (const dock of DOCKS) {
    const head = page.locator(`${dock.pane} > .dock-head`);
    // One head class across all four, so the padding, the background and the
    // gap between controls cannot drift again.
    await expect(head).toHaveCount(1);
    await expect(head.locator(".dock-title")).toHaveText(dock.title);

    // The last two children, in this order: the window, then the door. A dock
    // may put its own actions before them — Refresh, Clear — and none may put
    // anything after.
    const tail = await head.evaluate((el) =>
      [...el.children].slice(-2).map((c) => ({
        id: c.id,
        // Which drawing, not which path data: `icon()` names it for exactly
        // this, so a test pins the meaning and not the curve.
        icon: c.querySelector("svg")?.getAttribute("data-icon") ?? "",
        text: (c.textContent ?? "").trim(),
        label: c.getAttribute("aria-label") ?? "",
        classes: c.className,
      })),
    );
    expect(tail.map((c) => c.id)).toEqual([`${dock.name}-expand`, `${dock.name}-close`]);
    // The width, then the door — the same two drawings in every dock.
    expect(tail.map((c) => c.icon)).toEqual(["chevrons", "close"]);
    // Drawn, never typed: a text glyph renders as a hollow box wherever the
    // font lacks it, which is the reason `icons.ts` exists.
    expect(tail.map((c) => c.text)).toEqual(["", ""]);
    expect(tail[0].label).toBe("Expand");
    // The close control's name says which panel it shuts — four buttons that
    // all read "Close" is the same defect one step further on.
    expect(tail[1].label).toMatch(/^Close /);
    for (const c of tail) expect(c.classes).toContain("dock-icon");
  }
});

test("no control in a dock head is a typed glyph, and all of them say what they do", async ({
  page,
}) => {
  await app(page);

  for (const dock of DOCKS) {
    const controls = await page
      .locator(`${dock.pane} > .dock-head button`)
      .evaluateAll((els) =>
        els.map((el) => ({
          id: el.id,
          text: (el.textContent ?? "").trim(),
          icon: el.querySelector("svg")?.getAttribute("data-icon") ?? "",
          label: el.getAttribute("aria-label") ?? "",
          title: el.getAttribute("title") ?? "",
        })),
      );
    expect(controls.length).toBeGreaterThan(0);

    for (const c of controls) {
      // Either a drawing from `icons.ts` with no text, or a word — never a
      // character standing in for a drawing. `×` and `⟳` are one
      // hollow box each on a system whose font lacks them, which is the whole
      // reason that file exists; `icons.ts`' own header records it.
      if (c.icon) expect(c.text).toBe("");
      else expect(c.text.length).toBeGreaterThan(1);

      // Every one of them has a tooltip, including the ones with words on
      // them: "Clear" is a word, not a sentence, and what it clears and what
      // that costs is the part worth hovering for.
      expect(c.title.length, `${c.id} has no tooltip`).toBeGreaterThan(8);

      // And an icon with no accessible name is a button a screen reader reads
      // as "button".
      if (c.icon) expect(c.label.length, `${c.id} has no name`).toBeGreaterThan(0);
    }
  }
});

test("every drawing in an open dock's head is actually on screen", async ({ page }) => {
  await app(page);

  /**
   * The test this file was missing.
   *
   * Every assertion above this one is about the DOM -- which icon, which
   * name, which order -- and all of them passed while the drawings were
   * invisible in the running app. `.dock-icon`'s `padding: 0` lost to
   * `button.mini`'s `padding: 3px 9px` on specificity, `border-box` left a
   * 22px button with a 2px content box, and **WebKit shrank the 12px SVG into
   * it** while Chromium left it alone. A node that exists is not a picture
   * anybody can see, so this asserts the box.
   *
   * It also only fails on one engine, which is the second half of the lesson:
   * Tauri ships WebKitGTK on Linux, so `--project=webkit` is the run that
   * speaks for the app. With the old stylesheet this test reports
   * `mig-expand is 2px wide` there and passes on Chromium.
   */
  for (const dock of PLAIN) {
    await page.click(dock.rail);
    await expect(page.locator(dock.pane)).toBeVisible();

    const svgs = page.locator(`${dock.pane} > .dock-head button svg`);
    const n = await svgs.count();
    expect(n).toBeGreaterThan(0);

    for (let i = 0; i < n; i += 1) {
      const svg = svgs.nth(i);
      const owner = await svg.evaluate((el) => el.parentElement?.id ?? "?");
      const box = await svg.boundingBox();
      expect(box, `${owner} has no box`).not.toBeNull();
      // 12px by the stylesheet. The floor is deliberately well under that and
      // well over zero: this is asking "can it be seen", not "is it 12px".
      expect(box!.width, `${owner} is ${box!.width}px wide`).toBeGreaterThan(8);
      expect(box!.height, `${owner} is ${box!.height}px tall`).toBeGreaterThan(8);
    }

    await page.click(`#${dock.name}-close`);
  }
});

test("the width control points the way the pane is about to move", async ({ page }) => {
  await app(page);
  await page.click("#btn-migrations");

  // Collapsed: left, across the editor it is about to cover. The drawing
  // points right and CSS turns it, so this asserts the turn rather than a
  // second drawing — if the rule stopped matching, both states would look
  // identical and `aria-pressed` alone would not notice.
  const transform = () =>
    page.locator("#mig-expand svg").evaluate((el) => getComputedStyle(el).transform);
  await expect.poll(transform).toBe("matrix(-1, 0, 0, -1, 0, 0)");

  await page.click("#mig-expand");
  // Expanded: right, giving the editor back.
  await expect.poll(transform).toBe("none");
});

test("every dock says what the expanded view hid", async ({ page }) => {
  await app(page);
  for (const dock of DOCKS) {
    await expect(page.locator(`${dock.pane} .dock-full-note`)).toHaveText(
      /editor and results hidden/,
    );
  }
});

test("the two docks that could not take the window now can", async ({ page }) => {
  await app(page);

  for (const dock of PLAIN) {
    await page.click(dock.rail);
    await expect(page.locator(dock.pane)).toBeVisible();
    await expect(page.locator("#main")).toBeVisible();

    await page.click(`#${dock.name}-expand`);
    // The same one-dock-at-a-time mechanism the performance panel introduced:
    // an attribute naming which dock has the window, not a flag per dock.
    await expect(page.locator("#app")).toHaveAttribute("data-full", dock.name);
    await expect(page.locator("#main")).not.toBeVisible();
    await expect(page.locator("#sidebar")).not.toBeVisible();
    await expect(page.locator(dock.pane)).toBeVisible();
    await expect(page.locator(`#${dock.name}-expand`)).toHaveAttribute(
      "aria-label",
      "Collapse",
    );
    await expect(page.locator(`#${dock.name}-expand`)).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    // The key out, which the control's own tooltip names.
    await page.keyboard.press("Escape");
    await expect(page.locator("#app")).not.toHaveAttribute("data-full", /.*/);
    await expect(page.locator("#main")).toBeVisible();
    await expect(page.locator(`#${dock.name}-expand`)).toHaveAttribute(
      "aria-pressed",
      "false",
    );

    await page.click(`#${dock.name}-close`);
    await expect(page.locator(dock.pane)).toBeHidden();
  }
});

test("shutting a dock while it has the window gives the editor back", async ({ page }) => {
  await app(page);

  for (const dock of PLAIN) {
    await page.click(dock.rail);
    await page.click(`#${dock.name}-expand`);
    await expect(page.locator("#main")).not.toBeVisible();

    // The invariant that cost the editor: there is no shut-and-expanded state.
    // Reaching it would hide the editor behind a pane that is not on screen to
    // bring it back, and the only way out would be the rail.
    await page.click(`#${dock.name}-close`);
    await expect(page.locator(dock.pane)).toBeHidden();
    await expect(page.locator("#main")).toBeVisible();
    await expect(page.locator("#app")).not.toHaveAttribute("data-full", /.*/);
  }
});

/**
 * The bug the fifth dock found in the other four (§6.18).
 *
 * Three of the five seams measured the pointer's distance from **the window's
 * right edge**, which was true of each one on the day it was written, because
 * each was the outermost dock at the time. With a dock open beyond it, a seam
 * measuring that way computes a width that includes its neighbour -- so
 * dragging it *narrower* makes it jump wider instead. `migrations.spec.ts`
 * has carried this test for one seam since Stage 28, naming the error as
 * "387px too wide"; this is the same test for the rule rather than the seam.
 */
test("a seam with another dock beyond it sizes its own pane", async ({ page }) => {
  await app(page);
  await page.click("#btn-migrations");
  await page.click("#btn-history");
  const mig = page.locator("#mig-pane");
  await expect(mig).toBeVisible();
  await expect(page.locator("#hist-pane")).toBeVisible();

  const before = (await mig.boundingBox())!.width;
  const seam = (await page.locator("#migsplit").boundingBox())!;
  await page.mouse.move(seam.x + seam.width / 2, seam.y + seam.height / 2);
  await page.mouse.down();
  // Rightwards, which narrows it. From a window measurement it would widen.
  await page.mouse.move(seam.x + 60, seam.y + seam.height / 2, { steps: 4 });
  await page.mouse.up();

  const after = (await mig.boundingBox())!.width;
  expect(after).toBeLessThan(before - 40);
  // 60px of pointer is 60px of panel, give or take the seam's own width.
  expect(Math.abs(after - (before - 60))).toBeLessThan(10);
});

test("a second dock takes the window from the first", async ({ page }) => {
  await app(page);

  // Both open at once, which §6.5 settled is allowed.
  await page.click("#btn-migrations");
  await page.click("#btn-assistant");
  await expect(page.locator("#mig-pane")).toBeVisible();
  await expect(page.locator("#chat-pane")).toBeVisible();

  await page.click("#mig-expand");
  await expect(page.locator("#app")).toHaveAttribute("data-full", "mig");
  // Expanded means *the* window, so the other dock is hidden with the editor.
  await expect(page.locator("#chat-pane")).not.toBeVisible();
  await expect(page.locator("#chat-expand")).toHaveAttribute("aria-pressed", "false");

  // And the control of the dock that lost it goes back to saying "Expand",
  // rather than two docks both claiming to be the expanded one. One painter
  // for all four controls is what makes that true by construction.
  await page.keyboard.press("Escape");
  await page.click("#chat-expand");
  await expect(page.locator("#app")).toHaveAttribute("data-full", "chat");
  await expect(page.locator("#mig-pane")).not.toBeVisible();
  await expect(page.locator("#mig-expand")).toHaveAttribute("aria-pressed", "false");
  await expect(page.locator("#chat-expand")).toHaveAttribute("aria-pressed", "true");
});
