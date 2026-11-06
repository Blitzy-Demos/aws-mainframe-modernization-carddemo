import { fileURLToPath, URL } from "node:url";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Upstream of the dev server's /api proxy, the browser's only route to the API.
// The default equals CARDDEMO_API_PROXY_TARGET in .env.example; the Compose
// `app` profile sets http://api:8080. A set value must be an absolute http or
// https URL, or loading this config fails naming the variable, as the Rust
// binaries refuse an unusable setting at start; the proxy would otherwise
// crash or hang on the first /api request. The errors never echo the value,
// since a URL can carry credentials. The proxy receives the URL standard's
// canonical form of the value (http:127.0.0.1:8080 becomes
// http://127.0.0.1:8080/), not the value as written: the proxy parses its
// target with Node's legacy URL parser, which reads such a value without its
// host and so would send every /api request to port 80 of localhost.
const apiProxyTargetOf = (value: string | undefined): string => {
  if (value === undefined) {
    return "http://127.0.0.1:8080";
  }
  const target = value.trim();
  if (target === "") {
    throw new Error("CARDDEMO_API_PROXY_TARGET is set but blank");
  }
  const url = URL.canParse(target) ? new URL(target) : undefined;
  if (
    url === undefined ||
    (url.protocol !== "http:" && url.protocol !== "https:") ||
    url.hostname === ""
  ) {
    throw new Error(
      "invalid value for CARDDEMO_API_PROXY_TARGET: expected an absolute " +
        "http:// or https:// URL, for example http://127.0.0.1:8080",
    );
  }
  return url.href;
};

const apiProxyTarget = apiProxyTargetOf(process.env.CARDDEMO_API_PROXY_TARGET);

// Sent with every document, module and asset the server delivers, so that no
// other page can frame the signed-on SPA and steer a click onto a payment or
// an admin action (clickjacking). Both headers, because older engines honour
// only X-Frame-Options. The policy holds frame-ancestors alone: a script or
// style directive would block the inline code of Vite's dev client and HMR.
// `vite preview` sends server.headers when preview.headers is unset, so the
// built SPA it serves carries them too.
const documentHeaders: Record<string, string> = {
  "Content-Security-Policy": "frame-ancestors 'none'",
  "X-Frame-Options": "DENY",
};

// Vendor chunks, which aim to keep each chunk under Vite's 500 kB warning;
// pages are split further by route-level React.lazy. The warning-free
// `pnpm build` verifies the result: a chunk it still reports is split further,
// and raising the warning limit instead counts as a suppression. Rollup takes
// the modules assigned here in execution order and adds to each one's group
// every ungrouped module its static imports reach, directly or through other
// ungrouped modules, so a dependency left unassigned joins the group of the
// earliest-executing assigned module that reaches it.
//
// The groups keep what the entry needs (react, query, openapi and the Radix
// primitives of the app shell: sidebar sheet, menus, tooltips, separators,
// Slot) apart from what only lazy routes need (table, forms), so the latter
// load with the first route that uses them. The `radix-ui` barrel the
// components import stays unassigned: it imports every primitive it
// re-exports, so assigning it would pull them all into its group. The shell
// primitives are named by their own packages instead, and the route-only
// primitives stay unassigned, so they go to the lazy chunks Rollup creates
// for the routes that import them.
const vendorChunks: Record<string, readonly string[]> = {
  react: ["react", "react-dom", "react-router"],
  query: ["@tanstack/react-query"],
  table: ["@tanstack/react-table"],
  forms: ["react-hook-form", "zod", "@hookform/resolvers"],
  radix: [
    "@radix-ui/react-dialog",
    "@radix-ui/react-dropdown-menu",
    "@radix-ui/react-tooltip",
    "@radix-ui/react-separator",
    "@radix-ui/react-slot",
  ],
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
    headers: documentHeaders,
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
