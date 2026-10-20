import { defineConfig, devices } from "@playwright/test";
import viteConfig from "./vite.config";

/**
 * Playwright configuration for NAV (`e2e/nav.spec.ts`), which walks the screen navigation
 * graph through on-screen controls only.
 *
 * `scripts/e2e.sh` prepares the database, starts `carddemo-api` and the Vite dev server, runs
 * this suite and stops them again, so no `webServer` is configured here: one would start a
 * second dev server or race with that sequence. Export `modern/.env` before running it by hand.
 */

/**
 * Origin of the Vite dev server, read from `vite.config.ts` so that NAV always targets the host
 * and port the dev server binds (127.0.0.1 and `CARDDEMO_WEB_PORT`, default 5173, strict).
 */
function devServerOrigin(): string {
  const { host, port } = viteConfig.server ?? {};
  if (typeof host !== "string" || port === undefined) {
    throw new Error("vite.config.ts must set server.host and server.port for NAV to reach it");
  }
  return `http://${host}:${port}`;
}

export default defineConfig({
  testDir: "e2e",
  // helpers.ts is imported by the spec, never run on its own.
  testMatch: "**/*.spec.ts",
  // The USER0001 and ADMIN001 branches run one after the other against one shared database.
  fullyParallel: false,
  workers: 1,
  retries: 0,
  forbidOnly: true,
  reporter: [["list"], ["html", { open: "never", outputFolder: "playwright-report" }]],
  outputDir: "test-results",
  use: {
    baseURL: devServerOrigin(),
    // Traces and video would persist API payloads, card data included, to disk.
    trace: "off",
    screenshot: "only-on-failure",
    video: "off",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
