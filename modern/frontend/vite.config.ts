import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

/**
 * Vite dev server, build and Vitest configuration.
 *
 * Configuration comes from the environment only: export `modern/.env` first
 * (`set -a; . ./.env; set +a`); nothing here loads it. No variable is exposed to the client.
 */

/** Address the API binds; an unset or empty value falls back to the API's own default. */
const apiBind = process.env.CARDDEMO_API_BIND || "127.0.0.1:8080";

const DEFAULT_WEB_PORT = 5173;

/** A TCP port given as decimal digits in 1..65535, or the default for anything else. */
function webPort(value: string | undefined): number {
  if (value === undefined || !/^[0-9]{1,5}$/.test(value)) {
    return DEFAULT_WEB_PORT;
  }
  const port = Number.parseInt(value, 10);
  return port >= 1 && port <= 65535 ? port : DEFAULT_WEB_PORT;
}

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: {
    host: "127.0.0.1",
    // From CARDDEMO_WEB_PORT so concurrent local stacks keep their own port; defaults to 5173.
    port: webPort(process.env.CARDDEMO_WEB_PORT),
    strictPort: true,
    proxy: {
      // Matches the /api path segment only, so /api-docs and other /api* paths stay with Vite.
      "^/api(?:[/?]|$)": {
        target: `http://${apiBind}`,
      },
    },
  },
  test: {
    globals: true,
    projects: [
      {
        extends: true,
        test: {
          name: "unit",
          environment: "node",
          include: ["src/**/*.test.ts"],
        },
      },
      {
        extends: true,
        test: {
          name: "dom",
          environment: "jsdom",
          include: ["src/**/*.test.tsx"],
        },
      },
    ],
    coverage: {
      provider: "v8",
      include: ["src/**/*.{ts,tsx}"],
      exclude: [
        "src/components/ui/**",
        "src/api/schema.d.ts",
        "src/**/*.test.ts",
        "src/**/*.test.tsx",
      ],
      thresholds: {
        lines: 70,
      },
      reporter: ["text", "json-summary"],
      reportsDirectory: "coverage",
    },
  },
});
