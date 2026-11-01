import { fileURLToPath, URL } from "node:url";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Upstream of the dev server's /api proxy, the browser's only route to the API.
// The default equals CARDDEMO_API_PROXY_TARGET in .env.example; the Compose
// `app` profile sets http://api:8080.
const apiProxyTarget =
  process.env.CARDDEMO_API_PROXY_TARGET ?? "http://127.0.0.1:8080";

// Vendor chunks, so no chunk reaches Vite's 500 kB warning; pages are split
// further by route-level React.lazy. Raising the warning limit instead counts
// as a lint suppression. Rollup also places every unassigned dependency of a
// group's modules into the first group (by name) that reaches it, so only the
// packages the app imports are named here.
const vendorChunks: Record<string, readonly string[]> = {
  react: ["react", "react-dom", "react-router"],
  tanstack: ["@tanstack/react-query", "@tanstack/react-table"],
  forms: ["react-hook-form", "zod", "@hookform/resolvers"],
  radix: ["radix-ui"],
  openapi: ["openapi-fetch"],
};

const chunkByPackage = new Map(
  Object.entries(vendorChunks).flatMap(([chunk, packages]) =>
    packages.map((name) => [name, chunk] as const),
  ),
);

// The package owning a module id, taken from its last node_modules segment so
// pnpm's .pnpm store paths and the commonjs plugin's `?commonjs-*` ids of the
// same file resolve alike.
const packageOf = (id: string): string | undefined =>
  /.*\/node_modules\/((?:@[^/]+\/)?[^/]+)/.exec(id)?.[1];

// Assigned by package, not through the object form's entry list: that form
// names only the commonjs entry proxy of React's CommonJS builds, so another
// group claims the real modules and the two chunks import each other. The
// commonjs interop helpers serve those builds and stay with React, which then
// imports no other vendor chunk.
const manualChunks = (id: string): string | undefined => {
  if (id === "\0commonjsHelpers.js") {
    return "react";
  }
  const name = packageOf(id);
  return name === undefined ? undefined : chunkByPackage.get(name);
};

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    // Must agree with the "@/*" paths of tsconfig.app.json and the aliases of
    // components.json.
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: {
    // No `host` here: Compose passes --host 0.0.0.0 on the command line, and
    // processes started on the host stay on localhost.
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": {
        target: apiProxyTarget,
        changeOrigin: true,
        // The API serves its routes unprefixed (/auth/login, /accounts/{id});
        // the typed client calls them under baseUrl "/api".
        rewrite: (path) => path.replace(/^\/api/, ""),
      },
    },
  },
  build: {
    rollupOptions: {
      output: {
        manualChunks,
      },
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    // Limited to src so the Playwright specs under e2e/ stay out of Vitest.
    include: ["src/**/*.test.{ts,tsx}"],
    globals: false,
    css: true,
  },
});
