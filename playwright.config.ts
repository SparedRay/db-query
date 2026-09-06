import { defineConfig, devices } from "@playwright/test";

/**
 * UI tests run the real frontend in a real browser against a stubbed Tauri
 * bridge. See `tests/ui/harness.ts` for why that is the shape.
 *
 * **Chromium is the default and WebKit is the one that matters.** Tauri uses
 * WebKitGTK on Linux and WKWebView on macOS, so WebKit is much closer to what
 * ships; Chromium is here because it needs no extra system packages and so runs
 * anywhere, including an agent's sandbox. WebKit needs `libevent-2.1-7t64` and
 * `libavif16`, which is why it is opt-in rather than the default.
 */
export default defineConfig({
  testDir: "./tests/ui",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "list" : [["list"]],
  use: {
    baseURL: "http://127.0.0.1:1420",
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        // Chromium is the only engine that grants these, and therefore the only
        // one where a copy can be read back and asserted. Setting them globally
        // is what broke every WebKit test: `newPage` rejects the unknown
        // permission outright, so all 88 failed before running a line of app code.
        permissions: ["clipboard-read", "clipboard-write"],
      },
    },
    // WebKit is what actually ships on Linux and macOS, so it runs the same
    // suite — minus the clipboard *readback*, which the engine does not expose.
    // See `copied()` in clipboard.spec.ts for what is asserted there instead.
    { name: "webkit", use: { ...devices["Desktop Safari"] } },
  ],
  webServer: {
    command: "npx vite --host 127.0.0.1",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
});
