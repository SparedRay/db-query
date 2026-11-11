import { expect, test, type Page } from "@playwright/test";
import {
  MYSQL_CAPS,
  editorText,
  installBackend,
  rowsResult,
  schemaBackend,
  visit,
} from "./harness";

/**
 * Which server the window says you are on, and what it says when you are on
 * none.
 *
 * Two reports from live use, one cause. The accent that marks the active
 * script tab came from the connection's *profile*, which records what colour a
 * connection is and says nothing about whether we are talking to it — so a
 * disconnect left the last server's colour painted across a workspace with no
 * server behind it. And it was set on the tab strip alone, so every other
 * surface that marks a selection — the result tabs, the output and log
 * switches, the performance panel's two lists — stayed the app's own blue
 * whichever connection you were on.
 *
 * One token on `#app` fixes both: present while a connection is **live**,
 * absent otherwise, and inherited by everything that reads it.
 * `connections.spec.ts` already asserts the rail's half of the same rule — a
 * disconnected connection must not look like the one you are on.
 *
 * **Every colour here is asserted against `--accent` rather than a literal.**
 * The first swatch in the palette *is* the accent blue, so a test that
 * hard-codes `rgb(59, 130, 246)` passes whether the mechanism works or not;
 * and the accent itself differs between the two themes.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

/** Red: the obvious one for production, and deliberately not the accent. */
const RED = "#ef4444";

const SAVED = [
  {
    id: "prod",
    name: "prod-eu",
    colour: RED,
    host: "db.example.com",
    port: 3306,
    user: "reporting",
    database: "analytics",
    allowInvalidCerts: false,
    rememberPassword: true,
  },
];

/**
 * The app with one saved connection, which a click connects.
 *
 * Not the harness's `connect()`: that goes through the dialog and takes the
 * default swatch, which is the accent — the one colour that cannot tell this
 * mechanism's two states apart.
 */
async function withSaved(page: Page, extra: Record<string, unknown> = {}) {
  await installBackend(page, {
    ...schemaBackend,
    list_profiles: () => ({ profiles: SAVED, warning: null }),
    connect_saved: () => ({
      id: "prod",
      serverVersion: "8.4.0",
      databases: ["analytics"],
      currentDatabase: "analytics",
      capabilities: MYSQL_CAPS,
    }),
    disconnect: () => null,
    ...extra,
  });
  await visit(page);
}

async function connectSaved(page: Page) {
  await page.click(".rail-item");
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
}

/** The two tokens as `#app` resolves them, so the comparison is theme-proof. */
function accents(page: Page) {
  return page.locator("#app").evaluate((el) => {
    const css = getComputedStyle(el);
    return {
      conn: css.getPropertyValue("--conn-colour").trim(),
      accent: css.getPropertyValue("--accent").trim(),
    };
  });
}

/** What is actually painted, which is what was reported. */
function painted(page: Page, selector: string, prop: keyof CSSStyleDeclaration) {
  return page
    .locator(selector)
    .evaluate((el, p) => String(getComputedStyle(el)[p as never]), prop);
}

// ---------------------------------------------------- the colour, and its absence

test("the workspace wears the live connection's colour", async ({ page }) => {
  await withSaved(page);
  await connectSaved(page);

  const { conn, accent } = await accents(page);
  expect(conn).toBe(RED);
  expect(conn, "a red connection must not resolve to the app's blue").not.toBe(accent);
});

test("disconnecting drains the colour back to the app's accent", async ({ page }) => {
  await withSaved(page);
  await connectSaved(page);
  const live = await painted(page, "#script-tabs .stab.active", "borderTopColor");

  // The same button; it says Disconnect once there is something to disconnect.
  await page.click("#btn-connect");
  await expect(page.locator(".rail-item.offline")).toHaveCount(1);

  const { conn, accent } = await accents(page);
  expect(conn, "the token falls back to :root rather than keeping the profile").toBe(accent);

  // Asserted on the paint, not only the token: the tab is the surface the
  // report was about, and a token nothing reads would pass the check above.
  const after = await painted(page, "#script-tabs .stab.active", "borderTopColor");
  expect(after).not.toBe(live);
});

test("the status dot is a ring, not a disc, once the connection is down", async ({ page }) => {
  await withSaved(page);
  await connectSaved(page);
  await expect(page.locator("#conn-dot")).not.toHaveClass(/offline/);
  expect(await painted(page, "#conn-dot", "backgroundColor")).toBe("rgb(239, 68, 68)");

  await page.click("#btn-connect");
  await expect(page.locator("#conn-dot")).toHaveClass(/offline/);
  // Not "some other colour": nothing filled at all, and an outline in its
  // place — the class alone would pass with no rule behind it.
  expect(await painted(page, "#conn-dot", "backgroundColor")).toBe("rgba(0, 0, 0, 0)");
  expect(await painted(page, "#conn-dot", "borderStyle")).toBe("solid");
});

// ------------------------------------------------- everything that reads the token

/**
 * The second report: the result strip and the switches under it.
 *
 * Asserted together, in one test, because the defect was that they disagreed —
 * the claim is that every surface marking a selection in this workspace paints
 * the same colour, and a test per surface would not say that.
 */
test("the result tabs and the switches wear the same colour as the tab strip", async ({
  page,
}) => {
  await withSaved(page, {
    run_script: () => {
      const one = rowsResult([{ name: "a" }], [["1"]], { sql: "SELECT 1", start: 0 });
      const two = rowsResult([{ name: "b" }], [["2"]], { sql: "SELECT 2", start: 10 });
      return { ...one, statements: [one.statements[0], two.statements[0]] };
    },
  });
  await connectSaved(page);

  await page.locator("#editor .cm-content").click();
  await page.keyboard.type("SELECT 1;\nSELECT 2;");
  await page.keyboard.press("Control+Shift+Enter");
  await expect(page.locator("#tabs .tab")).toHaveCount(2);

  const red = "rgb(239, 68, 68)";
  expect(await painted(page, "#script-tabs .stab.active", "borderTopColor")).toBe(red);
  expect(await painted(page, "#tabs .tab.active", "borderTopColor")).toBe(red);
  expect(
    await painted(page, '#res-head .seg-btn[aria-pressed="true"]', "backgroundColor"),
  ).toBe(red);
});

/**
 * And what is deliberately left alone.
 *
 * `.stab.external` marks a tab that arrived through the MCP server rather than
 * because somebody opened it. That is provenance, and if it took the
 * connection's colour it would read as "this is your connection" — the one
 * thing it exists to deny. The sequence log's dots are a fixed four-state
 * vocabulary for the same reason: a slate connection would make "data" and
 * "ok" the same grey.
 */
test("provenance and semantic colour do not follow the connection", async ({ page }) => {
  await withSaved(page);
  await connectSaved(page);

  const same = await page.evaluate(() => {
    const probe = document.createElement("span");
    probe.className = "stab-origin";
    document.getElementById("script-tabs")!.append(probe);
    const origin = getComputedStyle(probe).color;
    probe.remove();
    const app = getComputedStyle(document.getElementById("app")!);
    return { origin, conn: app.getPropertyValue("--conn-colour").trim() };
  });
  expect(same.conn).toBe(RED);
  expect(same.origin, "provenance keeps the app's accent").not.toBe("rgb(239, 68, 68)");
});

// ------------------------------------------ the page for when nothing is connected

test("there is a page to look at before anything is connected", async ({ page }) => {
  await installBackend(page, schemaBackend);
  await visit(page);

  await expect(page.locator("#script-tabs .stab")).toHaveCount(0);
  await expect(page.locator("#blank-page")).toBeVisible();
  await expect(page.locator("#blank-page")).toContainText("No connection");
  // The editor keeps its box — CodeMirror measures against it — but is not on
  // screen and cannot be typed into.
  await expect(page.locator("#editor")).toBeHidden();
  // And the toolbar goes with it: Run, Format, Auto-LIMIT and the timeout all
  // act on a buffer, and there is no buffer. Run was already disabled here,
  // which is exactly why Format being live was easy to miss.
  await expect(page.locator("#editor-head")).toBeHidden();
  await expect(page.locator(".script-tabs-row")).toBeHidden();
});

test("the page's button opens the connection dialog", async ({ page }) => {
  await installBackend(page, schemaBackend);
  await visit(page);

  await page.click("#blank-connect");
  await expect(page.locator("#conn-dialog")).toBeVisible();
});

test("connecting takes the page away and gives the editor back", async ({ page }) => {
  await withSaved(page);
  await expect(page.locator("#blank-page")).toBeVisible();

  await connectSaved(page);
  await expect(page.locator("#blank-page")).toBeHidden();
  await expect(page.locator("#editor")).toBeVisible();
});

/**
 * **Disconnecting shows it too, and this test is the inverse of the one that
 * stood here first.**
 *
 * That one pinned the opposite — the tabs stay on screen, only the colour
 * drains — on the grounds that blanking the workspace would throw away SQL
 * somebody wrote. The report that followed was unambiguous: *"if we
 * disconnect, leaving no active connection, we should show that connection
 * screen as well."* Which is right, and the premise was the thing that was
 * wrong: hiding the editor pane discards nothing. The tabs stay in memory and
 * in the session file, `session.restoreInto` treats live tabs as the truth on
 * reconnect, and the second half of this test is the proof — the same tab,
 * with the same text, after a round trip.
 *
 * What is given up is editing SQL while disconnected, which is written down in
 * the stage doc rather than argued about here.
 */
test("disconnecting shows the connection screen, and the tabs survive it", async ({ page }) => {
  await withSaved(page);
  await connectSaved(page);
  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
  const before = await editorText(page);

  await page.click("#btn-connect");
  await expect(page.locator(".rail-item.offline")).toHaveCount(1);

  await expect(page.locator("#blank-page")).toBeVisible();
  await expect(page.locator("#editor")).toBeHidden();
  // The strip goes with the editor: a tab you can click but cannot open is
  // worse than one you cannot see.
  await expect(page.locator(".script-tabs-row")).toBeHidden();

  // ...and nothing was discarded.
  await page.click("#blank-connect");
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
  await expect(page.locator("#script-tabs .stab")).toHaveCount(1);
  expect(await editorText(page)).toBe(before);
});

/**
 * The screen names the connection you just left, and its button reconnects it.
 *
 * A page that only offers "New connection…" after a disconnect answers a
 * question nobody asked and hides the one action that is wanted. The button
 * does what the rail icon does — `conns.connect`, which uses the remembered
 * password and falls back to the editor when there is none — so no dialog
 * stands between the screen and the server.
 */
test("the screen offers to reconnect the connection you left", async ({ page }) => {
  await withSaved(page);
  await connectSaved(page);
  await page.click("#btn-connect");
  await expect(page.locator(".rail-item.offline")).toHaveCount(1);

  await expect(page.locator("#blank-title")).toHaveText("prod-eu is not connected");
  await expect(page.locator("#blank-connect")).toHaveText("Connect to prod-eu");

  await page.click("#blank-connect");
  // Straight back, with no dialog in the way.
  await expect(page.locator("#conn-dialog")).toBeHidden();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
  await expect(page.locator("#blank-page")).toBeHidden();
});

// ------------------------------------------------- text on the connection's colour

/**
 * Amber: 2.15:1 under white, the weakest pair in the palette and the one
 * Stage 32 §8 wrote down rather than fixed.
 */
const AMBER = "#f59e0b";

/** The same app, with the saved connection in whatever colour a test needs. */
async function withColour(page: Page, colour: string) {
  await withSaved(page, {
    list_profiles: () => ({ profiles: [{ ...SAVED[0], colour }], warning: null }),
  });
  await connectSaved(page);
}

/**
 * What the pixels actually contrast at, measured from the element rather than
 * from the token — the ink is only right if it lands on the fill it was
 * chosen for. A literal would pass on either theme and on either ink.
 */
function paintedContrast(page: Page, selector: string) {
  return page.locator(selector).evaluate((el) => {
    const cs = getComputedStyle(el);
    const lum = (v: string) => {
      const [r, g, b] = v.match(/\d+(\.\d+)?/g)!.slice(0, 3).map(Number);
      const ch = (c: number) => {
        c /= 255;
        return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
      };
      return 0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b);
    };
    const [hi, lo] = [lum(cs.color), lum(cs.backgroundColor)].sort((a, b) => b - a);
    return (hi + 0.05) / (lo + 0.05);
  });
}

/**
 * A token as the *painter* sees it. Reading the custom property gives the
 * authored `#ffffff`, which never equals a computed `rgb(255, 255, 255)` — so
 * the comparison has to go through an element that actually uses it.
 */
function tokenColour(page: Page, token: string) {
  return page.locator("#app").evaluate((el, t) => {
    const probe = document.createElement("span");
    probe.style.color = `var(${t})`;
    el.appendChild(probe);
    const seen = getComputedStyle(probe).color;
    probe.remove();
    return seen;
  }, token);
}

test("a light connection colour takes dark text, not white", async ({ page }) => {
  await withColour(page, AMBER);

  const ratio = await paintedContrast(page, ".rail-item.live");
  expect(ratio, "white on amber measured 2.15:1 before this").toBeGreaterThan(4.5);

  const ink = await painted(page, ".rail-item.live", "color");
  const white = await tokenColour(page, "--on-accent");
  expect(ink, "amber must not resolve to the accent's white").not.toBe(white);
});

test("a dark connection colour keeps the white it has always had", async ({ page }) => {
  await withColour(page, RED);

  // 3.76:1, and dark ink would buy 3.90 — not enough to be worth changing the
  // look of the two most-used swatches. The threshold is what is under test.
  const ratio = await paintedContrast(page, ".rail-item.live");
  expect(ratio).toBeGreaterThan(3);

  const ink = await painted(page, ".rail-item.live", "color");
  const white = await tokenColour(page, "--on-accent");
  expect(ink, "red was readable already").toBe(white);
});

test("a pressed segmented button reads the same ink as the rail", async ({ page }) => {
  await withColour(page, AMBER);

  // The control that prompted the note: it fills with `--conn-colour` too, and
  // inherits the ink from `#app` rather than deciding anything itself.
  // One of the five pressed segments, named rather than taken by position:
  // `painted` is strict, and a selector that matches five is not an assertion.
  const pressed = "#res-view-results[aria-pressed='true']";
  await expect(page.locator(pressed)).toBeVisible();
  expect(await paintedContrast(page, pressed)).toBeGreaterThan(4.5);
  expect(await painted(page, pressed, "color")).toBe(
    await painted(page, ".rail-item.live", "color"),
  );
});
