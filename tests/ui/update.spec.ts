import { expect, test, type Page } from "@playwright/test";
import { calls, installBackend, schemaBackend, type Backend } from "./harness";

/**
 * Self-update.
 *
 * Two rules, and every test here defends one of them:
 *   * **Checking is automatic; installing never is.** The user is asked, and a
 *     no means nothing happens.
 *   * **Never offer what cannot be delivered.** The Linux build ships as a
 *     `.deb`, which the updater cannot replace in place.
 */

const AVAILABLE = {
  type: "available",
  current: "0.1.0",
  version: "0.1.1",
  notes: "Fixes the result-tab switch.",
  date: "2026-09-08T10:00:00Z",
};

async function boot(page: Page, extra: Backend = {}) {
  await installBackend(page, { ...schemaBackend, ...extra });
  await page.goto("/");
}

const btn = "#btn-update";

test("nothing is offered when the build is already current", async ({ page }) => {
  await boot(page);
  await expect(page.locator("#conn-label")).toBeVisible();
  await expect(page.locator(btn)).toBeHidden();
});

/** A `.deb` cannot update itself. Saying so beats failing after a download. */
test("a build that cannot update itself offers nothing", async ({ page }) => {
  await boot(page, {
    update_check: () => ({ type: "unsupported", reason: "install the newer .deb" }),
  });
  await expect(page.locator("#conn-label")).toBeVisible();
  await expect(page.locator(btn)).toBeHidden();
});

/** Being offline is the ordinary case, not news. */
test("a check that fails is silent", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await boot(page, {
    update_check: () => {
      throw new Error("network unreachable");
    },
  });
  await expect(page.locator("#conn-label")).toBeVisible();
  await expect(page.locator(btn)).toBeHidden();
  expect(errors).toEqual([]);
});

test("an available update appears as a button naming the version", async ({ page }) => {
  await boot(page, { update_check: () => AVAILABLE });
  await expect(page.locator(btn)).toBeVisible();
  await expect(page.locator(btn)).toHaveText("Update to 0.1.1");
  await expect(page.locator(btn)).toHaveAttribute("title", /0\.1\.0/);
});

/** Checking must never install. The whole point of the button. */
test("finding an update installs nothing on its own", async ({ page }) => {
  await boot(page, { update_check: () => AVAILABLE });
  await expect(page.locator(btn)).toBeVisible();
  expect((await calls(page)).map((c) => c.cmd)).not.toContain("update_install");
});

test("the offer shows the version, the notes and what will happen", async ({ page }) => {
  await boot(page, { update_check: () => AVAILABLE });
  await page.click(btn);
  const dlg = page.locator("dialog.ask");
  await expect(dlg).toBeVisible();
  await expect(dlg.locator("h2")).toHaveText("Update to 0.1.1?");
  const body = dlg.locator(".ask-body");
  await expect(body).toContainText("Fixes the result-tab switch.");
  await expect(body).toContainText("0.1.0");
  // The consequence has to be stated: the app closes, and nothing is saved.
  await expect(body).toContainText(/close/i);
  await expect(body).toContainText(/[Uu]nsaved/);
});

/**
 * The notes and the app's own sentences are separate blocks and must render
 * that way, or the whole thing reads as one wall of text.
 */
test("the offer's paragraphs are not run together", async ({ page }) => {
  await boot(page, { update_check: () => AVAILABLE });
  await page.click(btn);
  const text = await page.locator("dialog.ask .ask-body").innerText();
  expect(text.split("\n").filter((l) => l.trim()).length).toBeGreaterThanOrEqual(3);
});

// ------------------------------------------------------- the notes as a document

/** Shaped like a real release body: this is what our own workflow writes. */
const MARKDOWN_NOTES = [
  "## Install",
  "",
  "**Windows** — download `db-query_x64-setup.exe` and run it.",
  "Installs for the current user, so there is no Administrator prompt.",
  "",
  "```",
  "sudo apt install ./db-query_amd64.deb",
  "```",
  "",
  "### Updates",
  "",
  "- Windows installs check on launch",
  "- Linux `.deb` installs do not self-update",
].join("\n");

const RICH = { ...AVAILABLE, notes: MARKDOWN_NOTES };

/**
 * **The notes are a document, so they are drawn as one.** They are a GitHub
 * release body, and were being shown as the characters they are typed with
 * — `## Install`, `**Windows**`, stray backticks — which reads as the app
 * failing to draw something rather than as a document.
 */
test("release notes render as Markdown, not as their own source", async ({ page }) => {
  await boot(page, { update_check: () => RICH });
  await page.click(btn);
  const notes = page.locator("dialog.ask .ask-notes");

  await expect(notes.locator("h3")).toHaveText("Install");
  await expect(notes.locator("h4")).toHaveText("Updates");
  await expect(notes.locator("strong").first()).toHaveText("Windows");
  await expect(notes.locator("pre code")).toHaveText("sudo apt install ./db-query_amd64.deb");
  await expect(notes.locator("ul li")).toHaveCount(2);
  await expect(notes.locator("li code").first()).toHaveText(".deb");

  // And none of the punctuation survives as punctuation.
  const text = await notes.innerText();
  expect(text).not.toContain("##");
  expect(text).not.toContain("**");
  expect(text).not.toContain("`");
});

/**
 * **The one that matters.** These notes come off the network: they are read
 * from `latest.json`, and only the *installer bytes* are signature-checked.
 * Nothing verifies this text. So it is rendered by building nodes — never by
 * handing a remote string to anything that can parse markup.
 */
test("HTML in release notes is shown, never interpreted", async ({ page }) => {
  await boot(page, {
    update_check: () => ({
      ...AVAILABLE,
      notes: 'Before <img src=x onerror="document.title=\'pwned\'"> after\n\n<b>not bold</b>',
    }),
  });
  await page.click(btn);
  const notes = page.locator("dialog.ask .ask-notes");

  await expect(notes.locator("img")).toHaveCount(0);
  await expect(notes.locator("b")).toHaveCount(0);
  // The characters arrive as characters.
  await expect(notes).toContainText("<img src=x");
  await expect(notes).toContainText("<b>not bold</b>");
  expect(await page.title()).not.toBe("pwned");
});

/**
 * The app's own sentences stay outside the notes box. The warning about
 * unsaved work has to be ours, and unmistakably so — a release body that
 * said the opposite must not be able to sit where it would be read as ours.
 */
test("the app's own warning is not inside the notes", async ({ page }) => {
  await boot(page, { update_check: () => RICH });
  await page.click(btn);
  await expect(page.locator("dialog.ask .ask-notes")).not.toContainText(/[Uu]nsaved/);
  await expect(page.locator("dialog.ask .ask-body > p").last()).toContainText(/[Uu]nsaved/);
});

/** No notes, no box — rather than an empty panel with a border round it. */
test("an offer with no notes shows no notes box", async ({ page }) => {
  await boot(page, { update_check: () => ({ ...AVAILABLE, notes: null }) });
  await page.click(btn);
  await expect(page.locator("dialog.ask")).toBeVisible();
  await expect(page.locator("dialog.ask .ask-notes")).toHaveCount(0);
  await expect(page.locator("dialog.ask .ask-body")).toContainText(/[Uu]nsaved/);
});

test("declining installs nothing and keeps the offer", async ({ page }) => {
  await boot(page, { update_check: () => AVAILABLE });
  await page.click(btn);
  await page.locator("dialog.ask menu button", { hasText: "Not now" }).click();
  await expect(page.locator("dialog.ask")).toHaveCount(0);
  expect((await calls(page)).map((c) => c.cmd)).not.toContain("update_install");
  await expect(page.locator(btn)).toBeVisible();
});

/** Escape means cancel, never "go ahead" — the rule dialog.ts was built for. */
test("dismissing the offer with Escape installs nothing", async ({ page }) => {
  await boot(page, { update_check: () => AVAILABLE });
  await page.click(btn);
  await expect(page.locator("dialog.ask")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator("dialog.ask")).toHaveCount(0);
  expect((await calls(page)).map((c) => c.cmd)).not.toContain("update_install");
});

test("accepting installs, and only then", async ({ page }) => {
  await boot(page, { update_check: () => AVAILABLE, update_install: () => null });
  await page.click(btn);
  await page.locator("dialog.ask menu button", { hasText: "Download and install" }).click();
  await expect.poll(async () => (await calls(page)).map((c) => c.cmd)).toContain(
    "update_install",
  );
});

/**
 * The failure that matters. An install that dies silently leaves someone
 * staring at a button that did nothing.
 */
test("a failed install says why and lets you try again", async ({ page }) => {
  await boot(page, {
    update_check: () => AVAILABLE,
    update_install: () => {
      throw new Error("signature did not match the public key");
    },
  });
  await page.click(btn);
  await page.locator("dialog.ask menu button", { hasText: "Download and install" }).click();

  const err = page.locator("dialog.ask", { hasText: "could not be installed" });
  await expect(err).toBeVisible();
  await expect(err.locator("p")).toContainText("signature did not match");
  await err.locator("menu button", { hasText: "Close" }).click();

  await expect(page.locator(btn)).toBeEnabled();
  await expect(page.locator(btn)).toHaveText("Update to 0.1.1");
});
