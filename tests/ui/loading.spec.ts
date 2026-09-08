import { expect, test, type Page } from "@playwright/test";
import { connect, installBackend, openDatabase, schemaBackend } from "./harness";

/**
 * Anything slower than an eyeblink has to say so.
 *
 * Reported after the first real database: clicking a saved connection looked
 * like nothing happened until it eventually opened, and expanding a schema was
 * silent too. The tree already *set* a `loading` class — it was styled with
 * `opacity: .5` on a caret, which nobody can see.
 *
 * Every test here gates the backend on a promise the test resolves itself, so
 * "while it is in flight" is a real state rather than a race against a timer.
 */

const SAVED = {
  id: "c1-saved",
  name: "prod-eu",
  colour: "#ef4444",
  host: "db.example.com",
  port: 3306,
  user: "reporting",
  database: "analytics",
  allowInvalidCerts: false,
  rememberPassword: true,
};

const CONN = {
  id: SAVED.id,
  serverVersion: "8.4.0",
  databases: ["analytics"],
  currentDatabase: "analytics",
};

/**
 * Is a spinner actually **painted** on this node?
 *
 * Asserting the `loading` class would prove nothing: the class was already set
 * before this change and styled with `opacity: .5` on a caret. The bug was that
 * nothing visible happened, so the test has to look at what is drawn — the
 * caret's `::after`, which is the spinner.
 */
async function spinnerPaintedOn(page: Page, selector: string) {
  return page.evaluate((sel) => {
    const el = document.querySelector(`${sel} .twisty`);
    if (!el) return { animation: "none", width: "0px" };
    const cs = getComputedStyle(el, "::after");
    return { animation: cs.animationName, width: cs.width };
  }, selector);
}

/** A promise the test decides when to settle. */
function gate() {
  let open!: () => void;
  let fail!: (e: unknown) => void;
  const p = new Promise<void>((res, rej) => {
    open = res;
    fail = rej;
  });
  return { p, open, fail };
}

async function bootSaved(page: Page, connectSaved: () => Promise<unknown>) {
  await installBackend(page, {
    ...schemaBackend,
    list_profiles: () => ({ profiles: [SAVED], warning: null }),
    connect_saved: connectSaved,
  });
  await page.goto("/");
  await expect(page.locator(".rail-item")).toHaveCount(1);
}

// ------------------------------------------------------------ connecting

test("connecting shows a spinner on the connection being opened", async ({ page }) => {
  const g = gate();
  await bootSaved(page, async () => {
    await g.p;
    return CONN;
  });

  await page.locator(".rail-item").click();
  await expect(page.locator(".rail-item.connecting")).toHaveCount(1);
  await expect(page.locator(".rail-item .spinner")).toBeVisible();
  await expect(page.locator(".rail-item")).toHaveAttribute("aria-busy", "true");

  g.open();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
  await expect(page.locator(".rail-item .spinner")).toHaveCount(0);
});

/** A dead-looking button gets clicked again; that must not open two connections. */
test("clicking again while connecting does not start a second attempt", async ({ page }) => {
  const g = gate();
  let attempts = 0;
  await bootSaved(page, async () => {
    attempts++;
    await g.p;
    return CONN;
  });

  await page.locator(".rail-item").click();
  await expect(page.locator(".rail-item.connecting")).toHaveCount(1);
  await page.locator(".rail-item").click();
  await page.locator(".rail-item").click();

  g.open();
  await expect(page.locator(".rail-item.live")).toHaveCount(1);
  expect(attempts).toBe(1);
});

/** A spinner that outlives its failure is worse than no spinner. */
test("a failed connection clears the spinner", async ({ page }) => {
  const g = gate();
  await bootSaved(page, async () => {
    await g.p;
    return CONN;
  });

  await page.locator(".rail-item").click();
  await expect(page.locator(".rail-item .spinner")).toBeVisible();

  g.fail(new Error("Access denied for user 'reporting'"));
  await expect(page.locator(".rail-item .spinner")).toHaveCount(0);
  await expect(page.locator(".rail-item.connecting")).toHaveCount(0);
  await expect(page.locator(".rail-item.offline")).toHaveCount(1);
});

// ------------------------------------------------------------ the tree

test("expanding a database spins until its contents arrive", async ({ page }) => {
  const g = gate();
  await connect(page, {
    ...schemaBackend,
    list_tables: async () => {
      await g.p;
      return [{ name: "users", kind: "BASE TABLE" }];
    },
  });

  await page.locator('.node.db:has-text("poc")').click();
  await expect(page.locator(".node.db.loading")).toHaveCount(1);
  const spun = await spinnerPaintedOn(page, ".node.db.loading");
  expect(spun.animation).toBe("spin");
  expect(spun.width).not.toBe("0px");

  g.open();
  await expect(page.locator(".node.group")).not.toHaveCount(0);
  await expect(page.locator(".node.db.loading")).toHaveCount(0);
  expect((await spinnerPaintedOn(page, ".node.db")).animation).toBe("none");
});

/**
 * Clicking a database is *two* round trips — USE, then the listing. The first
 * used to be invisible work the user was still waiting through.
 */
test("the database spinner covers the USE round trip too", async ({ page }) => {
  const g = gate();
  await connect(page, {
    ...schemaBackend,
    use_database: async () => {
      await g.p;
      return null;
    },
  });

  await page.locator('.node.db:has-text("poc")').click();
  await expect(page.locator(".node.db.loading")).toHaveCount(1);

  g.open();
  await expect(page.locator(".node.db.loading")).toHaveCount(0);
});

test("expanding a table spins until its columns arrive", async ({ page }) => {
  const g = gate();
  await connect(page, {
    ...schemaBackend,
    list_columns: async () => {
      await g.p;
      return [{ name: "id", dataType: "int", nullable: false, key: "PRI" }];
    },
  });
  await openDatabase(page);

  await page.locator('.node.table:has-text("users")').click();
  await expect(page.locator(".node.table.loading")).toHaveCount(1);
  expect((await spinnerPaintedOn(page, ".node.table.loading")).animation).toBe("spin");

  g.open();
  await expect(page.locator(".node.column")).not.toHaveCount(0);
  await expect(page.locator(".node.table.loading")).toHaveCount(0);
});

test("a failed expansion clears the spinner and shows why", async ({ page }) => {
  const g = gate();
  await connect(page, {
    ...schemaBackend,
    list_columns: async () => {
      await g.p;
      return [];
    },
  });
  await openDatabase(page);

  await page.locator('.node.table:has-text("users")').click();
  await expect(page.locator(".node.table.loading")).toHaveCount(1);

  g.fail(new Error("SELECT command denied"));
  await expect(page.locator(".node.table.loading")).toHaveCount(0);
  await expect(page.locator(".children .empty")).toContainText("SELECT command denied");
});
