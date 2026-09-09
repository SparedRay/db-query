import { expect, test, type Page } from "@playwright/test";
import { commandNames, connect, editorText, schemaBackend } from "./harness";

/**
 * The editor's Format action.
 *
 * **What the layout looks like is decided and tested in Rust** — `sqlfmt.rs`
 * owns the rules and the guarantee that a rewrite never changes a token. These
 * tests are about the wiring: that the right text is sent, that the answer
 * lands in the right place, that a refusal is visible, and — as everywhere in
 * this app — that nothing is executed.
 */

test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => {
    throw new Error(`uncaught page error: ${e.message}`);
  });
});

const MESSY = "select a,b from t where a=1;";
const LAID_OUT = "select a,\n  b\nfrom t\nwhere a = 1;\n";

/** A backend whose formatter is a fixed answer, so the wiring is what is under test. */
function formatting(answer: string | (() => never) = LAID_OUT) {
  return {
    ...schemaBackend,
    format_sql: typeof answer === "function" ? answer : () => answer,
  };
}

/** Put `sql` in the active tab, replacing whatever the starter document was. */
async function typeSql(page: Page, sql: string) {
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+a");
  await page.keyboard.type(sql);
}

test("the Format button lays the tab out", async ({ page }) => {
  await connect(page, formatting());
  await typeSql(page, MESSY);

  await page.click("#btn-format");

  await expect.poll(() => editorText(page)).toContain("from t");
  expect(await editorText(page)).toContain("where a = 1");
});

test("Ctrl+Shift+F does the same thing", async ({ page }) => {
  await connect(page, formatting());
  await typeSql(page, MESSY);

  await page.keyboard.press("Control+Shift+F");

  await expect.poll(() => editorText(page)).toContain("where a = 1");
});

/** The action is a text edit and nothing else. */
test("formatting runs nothing", async ({ page }) => {
  await connect(page, formatting());
  await typeSql(page, MESSY);
  await page.click("#btn-format");
  await expect.poll(() => editorText(page)).toContain("from t");

  expect(await commandNames(page)).not.toContain("run_script");
});

test("one undo puts the buffer back", async ({ page }) => {
  await connect(page, formatting());
  await typeSql(page, MESSY);
  await page.click("#btn-format");
  await expect.poll(() => editorText(page)).toContain("from t");

  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+z");

  await expect.poll(() => editorText(page)).toContain(MESSY);
});

/**
 * A refusal has to be visible. An explicit action that silently does nothing is
 * worse than one that fails, because there is no way to tell it from a no-op.
 */
test("a refusal is reported and the buffer is untouched", async ({ page }) => {
  await connect(
    page,
    formatting(() => {
      throw new Error("This could not be laid out safely, so nothing was changed.");
    }),
  );
  await typeSql(page, "select 'unterminated");

  await page.click("#btn-format");

  await expect(page.locator("#grid .empty")).toContainText("could not be laid out");
  expect(await editorText(page)).toContain("select 'unterminated");
});

test("text that is already laid out says so rather than looking broken", async ({ page }) => {
  // The backend hands back exactly what it was given.
  await connect(page, { ...schemaBackend, format_sql: (a) => a.sql as string });
  await typeSql(page, "select 1;");

  await page.click("#btn-format");

  await expect(page.locator("#grid .empty")).toContainText("Already laid out");
});

/** With a selection, only the selection is sent and only it is replaced. */
test("a selection is formatted on its own", async ({ page }) => {
  await connect(page, { ...schemaBackend, format_sql: () => "SELECT 2;\n" });
  await typeSql(page, "select 1;\nselect 2;");

  // Select the second line.
  await page.keyboard.press("Home");
  await page.keyboard.press("Shift+End");
  await page.click("#btn-format");

  await expect.poll(() => editorText(page)).toContain("SELECT 2;");
  // The first statement was never sent, and never touched.
  expect(await editorText(page)).toContain("select 1;");
  const call = (await commandNames(page)).filter((c) => c === "format_sql");
  expect(call).toHaveLength(1);
});

/** An empty tab has nothing to lay out, and must not ask. */
test("an empty buffer asks for nothing", async ({ page }) => {
  await connect(page, formatting());
  await page.locator("#editor .cm-content").click();
  await page.keyboard.press("Control+a");
  await page.keyboard.press("Delete");

  await page.click("#btn-format");

  expect(await commandNames(page)).not.toContain("format_sql");
});

/**
 * The cursor stays where it was, which is what makes this usable mid-edit.
 *
 * It is exact rather than approximate: the position is found by counting the
 * non-whitespace characters in front of it, and formatting only moves
 * whitespace — so the count is invariant. Asserted by typing a character and
 * seeing where it lands, since that is what the user would notice.
 */
test("the cursor keeps its place in the text", async ({ page }) => {
  await connect(page, formatting());
  await typeSql(page, MESSY);

  // Put it immediately after "select a," — nine characters in.
  await page.keyboard.press("Home");
  for (let i = 0; i < 9; i++) await page.keyboard.press("ArrowRight");

  await page.click("#btn-format");
  await expect.poll(() => editorText(page)).toContain("from t");

  await page.keyboard.type("X");
  // Still just after the comma, even though "b" moved to its own line.
  expect(await editorText(page)).toContain("select a,X");
});
