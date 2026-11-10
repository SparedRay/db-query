import { expect, test, type Page } from "@playwright/test";
import {
  connect,
  installBackend,
  openDatabase,
  relaunch,
  schemaBackend,
  visit,
  withSettings,
} from "./harness";

/**
 * The editor toolbar and the connection head (Stage 34).
 *
 * Three reports from use, all about the same two bars:
 *
 *   * the connection head printed the active **tab's** database beside the
 *     connection's name, as though it were a property of the server, while
 *     the tree directly underneath marked the same database in bold;
 *   * there was no way to stop long lines wrapping;
 *   * narrowing a pane turned the toolbar into a column of tall thin buttons,
 *     because the row shrank its controls instead of wrapping.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

// ------------------------------------------------- what the head says

test("the connection head names the connection, not the tab's database", async ({ page }) => {
  await connect(page);
  await openDatabase(page);

  // The database is still on screen — in the tree, in bold, which is both the
  // stronger statement and the true one. It is `activeDb` that was being
  // printed up here, which belongs to one tab and not to the server.
  await expect(page.locator(".node.db-active > .label")).toHaveCount(1);
  await expect(page.locator("#conn-label")).not.toContainText("·");
  await expect(page.locator("#conn-label")).toHaveText("127.0.0.1");
});

// ------------------------------------------------------------ wrapping

/** What CodeMirror is actually doing, which is the only thing that counts. */
function wraps(page: Page) {
  return page
    .locator("#editor .cm-content")
    .evaluate((el) => getComputedStyle(el).whiteSpace);
}

test("Wrap is on by default and turns long lines off wrapping", async ({ page }) => {
  await connect(page);
  await expect(page.locator("#chk-wrap")).toBeChecked();
  expect(await wraps(page)).toBe("break-spaces");

  await page.uncheck("#chk-wrap");
  expect(await wraps(page)).toBe("pre");

  await page.check("#chk-wrap");
  expect(await wraps(page)).toBe("break-spaces");
});

test("Wrap survives a new tab, which is where a compartment usually loses it", async ({ page }) => {
  await connect(page);
  await page.uncheck("#chk-wrap");
  expect(await wraps(page)).toBe("pre");

  // Compartment contents live in the EditorState, so a new tab is born with
  // whatever the compartment was *initialised* with — wrapping on. Without
  // re-applying on activate, the setting silently reverts here.
  await page.keyboard.press("Control+t");
  await expect(page.locator("#script-tabs .stab")).toHaveCount(2);
  expect(await wraps(page)).toBe("pre");
  await expect(page.locator("#chk-wrap")).not.toBeChecked();
});

test("Wrap is remembered across a restart", async ({ page }) => {
  await connect(page);
  await page.uncheck("#chk-wrap");
  await relaunch(page);
  await expect(page.locator("#chk-wrap")).not.toBeChecked();
  expect(await wraps(page)).toBe("pre");
});

test("a setting of false is honoured at boot", async ({ page }) => {
  await withSettings(page, { wrapText: false });
  await installBackend(page, schemaBackend);
  await visit(page);
  await expect(page.locator("#chk-wrap")).not.toBeChecked();
  expect(await wraps(page)).toBe("pre");
});

// ------------------------------------------------------ the narrow case

/**
 * **Rows, not columns.**
 *
 * `.pane-head` was a flex row with no `flex-wrap`, so narrowing it left every
 * control at `flex-shrink: 1` and squeezed each button until its own label
 * broke across lines. A 34px bar became a stack of tall thin buttons, which
 * is what made two docks open at once unusable.
 *
 * The invariant is the button's **height**: whatever the width, a control
 * keeps its own size and moves to another row. Asserted on geometry rather
 * than on `flex-wrap`, because the property can be set and still fail the
 * person — a control that wraps and also shrinks passes a property check.
 */
async function headShape(page: Page) {
  return page.evaluate(() => {
    const head = document.getElementById("editor-head") as HTMLElement;
    const run = document.getElementById("btn-run") as HTMLElement;
    const kbd = head.querySelector("kbd") as HTMLElement;
    return {
      pane: Math.round((document.getElementById("editor-pane") as HTMLElement).getBoundingClientRect().width),
      head: Math.round(head.getBoundingClientRect().height),
      button: Math.round(run.getBoundingClientRect().height),
      kbd: getComputedStyle(kbd).display,
      bleeds: head.getBoundingClientRect().width >
        (document.getElementById("editor-pane") as HTMLElement).getBoundingClientRect().width + 1,
    };
  });
}

test("a narrow toolbar wraps into rows and never grows a tall button", async ({ page }) => {
  await connect(page);

  await page.setViewportSize({ width: 1280, height: 760 });
  const wide = await headShape(page);
  expect(wide.head).toBeLessThan(45); // one row
  expect(wide.kbd).toBe("inline"); // room for the shortcut hints

  // Narrow enough to wrap, with nothing else open.
  await page.setViewportSize({ width: 1000, height: 760 });
  const mid = await headShape(page);
  expect(mid.head).toBeGreaterThan(wide.head); // it grew in rows...
  expect(mid.button).toBe(wide.button); // ...not in the controls

  // And the real case: a dock open beside a narrow window.
  await page.setViewportSize({ width: 900, height: 760 });
  await page.click("#btn-workload");
  await expect(page.locator("#wl-pane")).toBeVisible();
  const tight = await headShape(page);
  expect(tight.button).toBe(wide.button);
  // The shortcut hints go first: `Run Ctrl+Enter` is three times the width of
  // `Run`, and the shortcut is still in the title and the menu.
  expect(tight.kbd).toBe("none");
  // Whatever happens, the bar stays inside its pane rather than pushing it.
  expect(tight.bleeds).toBe(false);
});
