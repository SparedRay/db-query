import { expect, test } from "@playwright/test";
import { calls, connect, installBackend, logNotes, openDatabase, schemaBackend } from "./harness";

/**
 * The logbook, from the side that writes to it.
 *
 * Before Stage 16 this app logged nothing at all. The Rust half — the ring,
 * the file, the scrubber — is tested in `logbook.rs`. What these own is the
 * half Rust cannot see: that the frontend records what it did, that the errors
 * a packaged webview would otherwise swallow reach the log, and above all
 * **that a password never does**.
 */

test("every command the UI sends is recorded by name", async ({ page }) => {
  await connect(page);
  // The tree fetches lazily, so a database has to be opened before anything
  // beyond the connection itself has been asked for.
  await openDatabase(page);

  // Notes are batched and flushed on a short timer, so the last ones are still
  // in the queue the instant a click returns. Polling is what a reader does
  // too — nobody copies the report mid-command.
  await expect
    .poll(async () => (await logNotes(page)).some((n) => n.message.startsWith("list_tables ok in")))
    .toBe(true);

  const notes = await logNotes(page);
  const messages = notes.map((n) => n.message);
  expect(messages.some((m) => m.startsWith("connect ok in"))).toBe(true);
  // Every one carries a duration, which is half the point of recording it.
  for (const m of messages.filter((x) => x.includes(" ok in "))) {
    expect(m).toMatch(/ ok in \d+ms$/);
  }
});

/**
 * **The test this whole module is shaped around.**
 *
 * Three commands take a password. The wrapper is handed the arguments and does
 * not read them — so the way this fails is somebody "improving" it to log
 * `args` for context, and this is what would stop them.
 */
test("a password reaches the backend and never reaches the log", async ({ page }) => {
  await installBackend(page, schemaBackend);
  await page.goto("/");
  await page.click("#btn-connect");
  await page.locator("#conn-dialog").waitFor({ state: "visible" });
  await page.fill("#conn-dialog input[name=password]", "hunter2-do-not-log-me");
  await page.click("#conn-ok");
  await page.locator("#conn-dialog").waitFor({ state: "hidden" });

  // It really did go to the backend — otherwise this proves nothing at all.
  const sent = await calls(page);
  const connectCall = sent.find((c) => c.cmd === "connect");
  expect(JSON.stringify(connectCall?.args)).toContain("hunter2-do-not-log-me");

  // And it is nowhere in the log.
  const notes = await logNotes(page);
  expect(notes.length).toBeGreaterThan(0);
  expect(JSON.stringify(notes)).not.toContain("hunter2-do-not-log-me");
});

test("a failed command is recorded as an error, with what went wrong", async ({ page }) => {
  await installBackend(page, {
    ...schemaBackend,
    list_tables: () => {
      throw new Error("the server went away");
    },
  });
  await page.goto("/");
  await page.click("#btn-connect");
  await page.click("#conn-ok");
  // Opening the database is what asks for the tables, and what fails.
  await page.locator('.node.db:has-text("poc")').click();

  await expect
    .poll(async () => (await logNotes(page)).filter((n) => n.level === "error"))
    .not.toHaveLength(0);

  const failures = (await logNotes(page)).filter((n) => n.level === "error");
  expect(failures.some((n) => n.message.includes("list_tables failed"))).toBe(true);
  expect(failures.some((n) => n.message.includes("the server went away"))).toBe(true);
});

/**
 * The gap the logbook exists to close. In `mise dev` this lands in the dev
 * server's runtime log; in a packaged build it lands nowhere, and the user
 * reports "it did nothing".
 */
test("an uncaught error reaches the log instead of vanishing", async ({ page }) => {
  await installBackend(page, schemaBackend);
  await page.goto("/");

  await page.evaluate(() => {
    // Thrown from a timer, so it is genuinely uncaught rather than caught by
    // the evaluate call itself.
    setTimeout(() => {
      throw new Error("something nobody was expecting");
    }, 0);
  });

  await expect
    .poll(async () =>
      (await logNotes(page)).some((n) => n.message.includes("something nobody was expecting")),
    )
    .toBe(true);
});

test("an unhandled rejection reaches the log too", async ({ page }) => {
  await installBackend(page, schemaBackend);
  await page.goto("/");

  await page.evaluate(() => {
    void Promise.reject(new Error("a promise nobody caught"));
  });

  await expect
    .poll(async () => (await logNotes(page)).find((n) => n.message.includes("unhandled rejection")))
    .toBeTruthy();
  const note = (await logNotes(page)).find((n) => n.message.includes("unhandled rejection"))!;
  expect(note.level).toBe("error");
  expect(note.message).toContain("a promise nobody caught");
});

/**
 * Routine commands are suppressed on success and never on failure. Suppressing
 * a failure would hide exactly the thing the log is for.
 */
test("a routine command is quiet when it works and loud when it does not", async ({ page }) => {
  await connect(page);
  expect((await logNotes(page)).some((n) => n.message.startsWith("save_session ok"))).toBe(false);

  await installBackend(page, {
    ...schemaBackend,
    save_session: () => {
      throw new Error("the disk is full");
    },
  });
  await page.goto("/");
  await page.click("#btn-connect");
  await page.click("#conn-ok");
  await page.click("#script-tabs .new-tab, #script-tabs button:last-child");

  await expect
    .poll(async () =>
      (await logNotes(page)).some(
        (n) => n.level === "error" && n.message.includes("save_session failed"),
      ),
    )
    .toBe(true);
});

/** The report has to arrive somewhere it can be copied from. */
test("the diagnostics report opens where it can be copied", async ({ page }) => {
  await connect(page, {
    diagnostics: () => "db-query — diagnostics\n\n=== log (2 lines) ===\nsomething happened\n",
  });
  await page.click("#btn-settings");
  await page.click("#set-tab-about");
  await page.click("#btn-diagnostics");

  const viewer = page.locator("dialog.viewer");
  await expect(viewer).toBeVisible();
  await expect(viewer.locator(".viewer-body")).toContainText("something happened");
  await expect(viewer.locator("menu button", { hasText: "Copy" })).toBeVisible();
});
