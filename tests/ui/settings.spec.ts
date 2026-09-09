import { expect, test, type Page } from "@playwright/test";
import { calls, connect, installBackend, schemaBackend, settingsSection } from "./harness";

/**
 * The settings dialog: appearance, session defaults, and the manual update
 * check the boot check deliberately does not provide.
 *
 * Everything applies immediately. A settings dialog with an OK button makes you
 * guess what a font looks like before you are allowed to see it.
 */

const codeStyle = (page: Page) =>
  page.evaluate(() => {
    const cs = getComputedStyle(document.documentElement);
    const grid = document.querySelector("table.rs, .cm-scroller");
    return {
      size: cs.getPropertyValue("--code-size").trim(),
      family: cs.getPropertyValue("--mono").trim(),
      painted: grid ? getComputedStyle(grid).fontSize : null,
    };
  });

async function open(page: Page) {
  await page.click("#btn-settings");
  await expect(page.locator("#settings-dialog")).toBeVisible();
}

test("the cog opens settings and Close closes them", async ({ page }) => {
  await connect(page, schemaBackend);
  await open(page);
  await page.click("#set-close");
  await expect(page.locator("#settings-dialog")).toBeHidden();
});

test("changing the code size repaints the editor immediately", async ({ page }) => {
  await connect(page, schemaBackend);
  const before = await codeStyle(page);
  expect(before.size).toBe("12px");

  await open(page);
  await page.fill("#set-font-size", "17");
  await page.locator("#set-font-size").dispatchEvent("change");

  await expect.poll(async () => (await codeStyle(page)).size).toBe("17px");
  // Not just the token — what the editor actually paints with.
  expect((await codeStyle(page)).painted).toBe("17px");
});

test("the code font reaches the grid as well as the editor", async ({ page }) => {
  await connect(page, schemaBackend);
  await open(page);
  await page.selectOption("#set-font", { label: "Courier New" });
  await page.click("#set-close");

  const family = (await codeStyle(page)).family;
  expect(family).toContain("Courier New");
  // The chosen family is a *stack*: an uninstalled font must still fall back.
  expect(family).toContain("monospace");
});

test("settings survive a reload", async ({ page }) => {
  await connect(page, schemaBackend);
  await open(page);
  await page.fill("#set-font-size", "15");
  await page.locator("#set-font-size").dispatchEvent("change");
  await settingsSection(page, "editor");
  await page.uncheck("#set-autolimit");
  await page.click("#set-close");

  await page.reload();
  await expect.poll(async () => (await codeStyle(page)).size).toBe("15px");
  await expect(page.locator("#chk-autolimit")).not.toBeChecked();
});

/** The defaults exist to spare you resetting the same controls every launch. */
test("session defaults are applied to the controls at boot", async ({ page }) => {
  await connect(page, schemaBackend);
  await open(page);
  await settingsSection(page, "editor");
  await page.uncheck("#set-lint");
  await page.fill("#set-timeout", "30");
  await page.locator("#set-timeout").dispatchEvent("change");
  await page.click("#set-close");
  await page.reload();

  await expect(page.locator("#chk-lint")).not.toBeChecked();
  await expect(page.locator("#num-timeout")).toHaveValue("30");
});

/** The browse limit is what "Select first N rows" actually asks for. */
test("the browse limit reaches the generated SELECT", async ({ page }) => {
  await connect(page, { ...schemaBackend, generate_select: () => "SELECT 1" });
  await open(page);
  await settingsSection(page, "editor");
  await page.fill("#set-browse", "25");
  await page.locator("#set-browse").dispatchEvent("change");
  await page.click("#set-close");

  await page.locator('.node.db:has-text("poc")').click();
  await page.locator('.node.table:has-text("users")').dblclick();

  await expect
    .poll(async () => (await calls(page)).find((c) => c.cmd === "generate_select")?.args.limit)
    .toBe(25);
});

/**
 * A stored value must never be able to lock the user out of the UI that would
 * fix it — so every field is validated on the way in, not merged blindly.
 */
test("a nonsense stored setting falls back instead of breaking the app", async ({ page }) => {
  await installBackend(page, schemaBackend);
  await page.addInitScript(() => {
    localStorage.setItem(
      "db-query.settings",
      JSON.stringify({ fontSize: 900, timeoutSecs: -5, browseLimit: 0, fontFamily: "Comic Sans" }),
    );
  });
  await page.goto("/");

  const s = await codeStyle(page);
  expect(s.size).toBe("12px");
  expect(s.family).not.toContain("Comic Sans");
  await expect(page.locator("#num-timeout")).toHaveValue("0");
});

// --------------------------------------------------------------- updates

test("the settings dialog reports the running version", async ({ page }) => {
  await connect(page, {
    ...schemaBackend,
    update_check: () => ({ type: "upToDate", current: "0.4.2" }),
  });
  await open(page);
  await settingsSection(page, "updates");
  await expect(page.locator("#set-version")).toContainText("0.4.2");
});

test("checking manually says so when there is nothing new", async ({ page }) => {
  await connect(page, {
    ...schemaBackend,
    update_check: () => ({ type: "upToDate", current: "0.4.2" }),
  });
  await open(page);
  await settingsSection(page, "updates");
  await page.click("#set-check-update");
  await expect(page.locator("#set-update-note")).toContainText("latest version");
});

/** The boot check is silent by design, which leaves no way to ask again. */
test("checking manually offers an update when there is one", async ({ page }) => {
  let n = 0;
  await connect(page, {
    ...schemaBackend,
    // Nothing at boot, something when asked — exactly the case the manual
    // check exists for: you were offline when the app started.
    update_check: () =>
      ++n === 1
        ? { type: "upToDate", current: "0.1.0" }
        : { type: "available", current: "0.1.0", version: "0.2.0", notes: null, date: null },
  });
  await expect(page.locator("#btn-update")).toBeHidden();

  await open(page);
  await settingsSection(page, "updates");
  await page.click("#set-check-update");

  await expect(page.locator("dialog.ask")).toBeVisible();
  await expect(page.locator("dialog.ask h2")).toHaveText("Update to 0.2.0?");
  // Settings gets out of the way rather than stacking two modals.
  await expect(page.locator("#settings-dialog")).toBeHidden();
});

test("a build that cannot update itself explains why, here too", async ({ page }) => {
  await connect(page, {
    ...schemaBackend,
    update_check: () => ({ type: "unsupported", reason: "install the newer .deb" }),
  });
  await open(page);
  await settingsSection(page, "updates");
  await page.click("#set-check-update");
  await expect(page.locator("#set-update-note")).toContainText(".deb");
});

// ------------------------------------------------------------------ sections

/**
 * Settings are several unrelated subjects, so they are several panes rather
 * than one scrolling column. What is worth pinning is that exactly one is on
 * screen — a pane that failed to hide is the only way this arrangement can be
 * worse than the stack it replaced. The count is asserted against the tab
 * strip rather than written down, so adding a section is one edit.
 */
test("one section is shown at a time", async ({ page }) => {
  await connect(page, schemaBackend);
  await open(page);

  const panes = page.locator("#settings-dialog .pane");
  await expect(panes).toHaveCount(
    await page.locator('#settings-nav [role="tab"]').count(),
  );
  await expect(page.locator("#settings-dialog .pane:visible")).toHaveCount(1);
  await expect(page.locator("#set-pane-appearance")).toBeVisible();

  await settingsSection(page, "assistant");
  await expect(page.locator("#settings-dialog .pane:visible")).toHaveCount(1);
  await expect(page.locator("#set-pane-appearance")).toBeHidden();
  // The controls keep their values across a switch: the panes are hidden, not
  // rebuilt, so nothing half-typed is thrown away by looking at another one.
  await expect(page.locator("#set-ai-base")).toHaveValue(/https/);
});

test("arrow keys move between sections, and the highlight follows", async ({ page }) => {
  await connect(page, schemaBackend);
  await open(page);
  await page.locator("#set-tab-appearance").focus();

  await page.keyboard.press("ArrowDown");
  await expect(page.locator("#set-pane-editor")).toBeVisible();
  await expect(page.locator("#set-tab-editor")).toBeFocused();
  await expect(page.locator("#set-tab-editor")).toHaveAttribute("aria-selected", "true");
  await expect(page.locator("#set-tab-appearance")).toHaveAttribute("aria-selected", "false");

  await page.keyboard.press("End");
  await expect(page.locator("#set-pane-about")).toBeVisible();
  // Past the end is not a wrap: the last section stays put.
  await page.keyboard.press("ArrowDown");
  await expect(page.locator("#set-pane-about")).toBeVisible();
});

/** Reopening to change the same thing again is the common case. */
test("settings reopen on the section last used", async ({ page }) => {
  await connect(page, schemaBackend);
  await open(page);
  await settingsSection(page, "updates");
  await page.click("#set-close");

  await open(page);
  await expect(page.locator("#set-pane-updates")).toBeVisible();
  await expect(page.locator("#set-pane-appearance")).toBeHidden();
});
