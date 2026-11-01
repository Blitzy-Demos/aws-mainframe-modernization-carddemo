import { defineConfig, devices } from "@playwright/test";

// Host mode and the Compose app profile both serve the SPA on port 5173; a
// run against another address overrides it through E2E_BASE_URL.
const baseURL = process.env.E2E_BASE_URL ?? "http://localhost:5173";

export default defineConfig({
  testDir: "./e2e",
  // e2e/fixtures.ts is a helper module, not a test file.
  testMatch: "**/*.spec.ts",
  // The specs share one seeded database: e2e/fixtures.ts sets reward balances
  // through psql and forms.spec.ts creates users and transactions, so the
  // specs run one at a time.
  fullyParallel: false,
  workers: 1,
  forbidOnly: true,
  // A retry would let a flaky pass hide a parity failure.
  retries: 0,
  outputDir: "test-results",
  reporter: [["list"], ["html", { outputFolder: "playwright-report", open: "never" }]],
  use: {
    baseURL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  // Every spec runs at both supported breakpoints, in Chromium only.
  projects: [
    {
      // md: the sidebar collapses to an icon rail with tooltips.
      name: "md-768x1024",
      use: { ...devices["Desktop Chrome"], viewport: { width: 768, height: 1024 } },
    },
    {
      // lg: the sidebar is expanded and forms use two columns.
      name: "lg-1280x800",
      use: { ...devices["Desktop Chrome"], viewport: { width: 1280, height: 800 } },
    },
  ],
  // No webServer: the stack is started by hand in host and Compose modes, and
  // the @smoke run must expose a failed startup instead of masking it.
});
