// Session-only preference gate (AAP 0.4.1, 0.4.4): nothing the SPA keeps, the
// theme, the sidebar's open state or the signed-on session, outlives the browser
// session. sessionStorage is the one store allowed; cookies, localStorage and
// IndexedDB are not, because preference persistence beyond the session is out of
// scope. The generated sidebar's seven-day sidebar_state cookie is the leak this
// gate was written against.
//
// Two checks:
// - A static scan of every application and generated TypeScript source under
//   src, read through Vite's ?raw glob and scanned whole, comments included: a
//   source has no reason to name a persistent store at all. Cookie attributes
//   are case-insensitive, so every token is matched case-insensitively. Test
//   sources are left out, because they name the stores to assert on them, and
//   this file holds the tokens. index.css is not scanned: a style sheet cannot
//   reach a browser store.
// - A runtime render of the authenticated shell (AppLayout under the root
//   route's AppRoot, as App.tsx nests them, and the providers of main.tsx)
//   that toggles the sidebar and the theme through their own controls and
//   fires pagehide, when a page makes its last writes, then finds no cookie,
//   no localStorage entry, and in sessionStorage exactly the session and the
//   theme.
//
// The file is .ts, so elements are built with createElement rather than JSX.
import { createElement } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AppLayout } from "@/components/app-layout";
import { THEME_STORAGE_KEY, ThemeProvider } from "@/components/theme-provider";
import { TooltipProvider } from "@/components/ui/tooltip";
import {
  clearSession,
  SESSION_STORAGE_KEY,
  setSession,
  type Session,
} from "@/lib/session";
import { AppRoot } from "@/routes/app-root";

// The persistent-store tokens of AAP 0.4.1: the cookie API and its lifetime
// attributes, and the two persistent Web Storage and database APIs.
const STORE_TOKENS = [
  "document.cookie",
  "localStorage",
  "indexedDB",
  "max-age",
  "expires=",
] as const;

type StoreToken = (typeof STORE_TOKENS)[number];

// Each token as a literal, case-insensitive pattern.
const TOKEN_PATTERNS: readonly (readonly [StoreToken, RegExp])[] =
  STORE_TOKENS.map(
    (token) =>
      [
        token,
        new RegExp(token.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi"),
      ] as const,
  );

// One token found in a source: its 1-based line, the token and the trimmed line.
interface TokenHit {
  readonly line: number;
  readonly token: StoreToken;
  readonly text: string;
}

function tokenHits(source: string): TokenHit[] {
  const lines = source.split("\n");
  return TOKEN_PATTERNS.flatMap(([token, pattern]) =>
    Array.from(source.matchAll(pattern), (match) => ({
      at: match.index,
      token,
    })),
  )
    .sort((a, b) => a.at - b.at)
    .map(({ at, token }) => {
      const line = source.slice(0, at).split("\n").length;
      return { line, token, text: lines[line - 1].trim() };
    });
}

function isTestSource(file: string): boolean {
  return /\.test\.tsx?$/.test(file) || file.startsWith("/src/test/");
}

// Keys are project-absolute paths such as /src/components/ui/sidebar.tsx. The
// pattern also takes declaration files such as the generated api/schema.d.ts.
const sources = import.meta.glob<string>("/src/**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
});

const byFile = (
  [a]: readonly [string, string],
  [b]: readonly [string, string],
): number => (a < b ? -1 : a > b ? 1 : 0);

const scanned: readonly (readonly [file: string, source: string])[] =
  Object.entries(sources)
    .filter(([file]) => !isTestSource(file))
    .sort(byFile);

// The sources that hold, or once held, a store write in the registry or the
// shadcn guides (sidebar cookie, theme provider, the next-themes import of
// sonner), the session and API modules that keep the token, and the entry
// modules. A glob that misses any of them would let the scan pass vacuously.
const REQUIRED_SOURCES: readonly string[] = [
  "/src/App.tsx",
  "/src/api/client.ts",
  "/src/api/schema.d.ts",
  "/src/components/theme-provider.tsx",
  "/src/components/ui/sidebar.tsx",
  "/src/components/ui/sonner.tsx",
  "/src/hooks/use-mobile.ts",
  "/src/lib/session.ts",
  "/src/lib/utils.ts",
  "/src/main.tsx",
];

describe("persistent stores in sources", () => {
  it("scans every application and generated source and no test source", () => {
    const files = scanned.map(([file]) => file);
    expect(files).toEqual(expect.arrayContaining([...REQUIRED_SOURCES]));

    // Checked independently of isTestSource, so a broken filter fails here.
    const tests = files.filter(
      (file) =>
        file.startsWith("/src/test/") ||
        file.endsWith(".test.ts") ||
        file.endsWith(".test.tsx"),
    );
    expect(tests).toEqual([]);

    // The glob reaches the test sources, under src/test and beside the modules
    // they test, and the filter is what leaves them out. Vite omits the
    // importing module, this file, from its own glob.
    const excluded = Object.keys(sources).filter(isTestSource);
    expect(excluded).toEqual(
      expect.arrayContaining([
        "/src/test/setup.ts",
        "/src/test/no-raw-elements.test.ts",
        "/src/schemas/schemas.test.ts",
      ]),
    );

    const empty = scanned
      .filter(([, source]) => typeof source !== "string" || source.trim() === "")
      .map(([file]) => file);
    expect(empty).toEqual([]);
  });

  it("matches the store tokens in any case but not sessionStorage", () => {
    const flagged = [
      'document.cookie = "a=1"',
      'window.localStorage.setItem("k", "v")',
      'indexedDB.open("x")',
      '"sidebar_state=1; Max-Age=604800"',
      '"a=1; expires=Thu"',
    ];
    expect(
      flagged.map((sample) => tokenHits(sample).map((hit) => hit.token)),
    ).toEqual([
      ["document.cookie"],
      ["localStorage"],
      ["indexedDB"],
      ["max-age"],
      ["expires="],
    ]);

    const ignored = [
      'sessionStorage.setItem("carddemo.theme", "dark")',
      "expires_at",
    ];
    expect(ignored.filter((sample) => tokenHits(sample).length > 0)).toEqual(
      [],
    );

    expect(
      tokenHits(
        'const a = 1;\n\n  // keeps it in LocalStorage\n  document.cookie = "x; Expires=Thu";\n',
      ),
    ).toEqual([
      { line: 3, token: "localStorage", text: "// keeps it in LocalStorage" },
      {
        line: 4,
        token: "document.cookie",
        text: 'document.cookie = "x; Expires=Thu";',
      },
      {
        line: 4,
        token: "expires=",
        text: 'document.cookie = "x; Expires=Thu";',
      },
    ]);
  });

  it("finds no cookie, localStorage or IndexedDB use in any source", () => {
    const violations = scanned.flatMap(([file, source]) =>
      tokenHits(source).map((hit) => ({ file, ...hit })),
    );
    expect(violations).toEqual([]);
  });
});

// The desktop sidebar's open state. jsdom's 1024px window is not mobile
// (hooks/use-mobile.ts), so the generated Sidebar renders its desktop element,
// the only data-slot="sidebar" carrying data-state.
function sidebarState(): string | null {
  const sidebar = document.querySelector('[data-slot="sidebar"]');
  expect(sidebar).not.toBeNull();
  return sidebar?.getAttribute("data-state") ?? null;
}

describe("persistent stores at runtime", () => {
  // Any request the shell sends goes through the API client to this stub; none
  // is expected while the shell renders.
  beforeEach(() => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(JSON.stringify({ message: "ok" }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
      ),
    );
  });

  // setup.ts clears sessionStorage, but session.ts also holds the session in a
  // module variable and keeps an expiry timer, which only clearSession resets.
  afterEach(() => {
    vi.unstubAllGlobals();
    clearSession();
    document.documentElement.className = "";
  });

  it("keeps the sidebar and theme toggles and the session out of cookies and localStorage", async () => {
    // Measured before the shell mounts, so the final check sees the shell alone.
    expect(document.cookie).toBe("");
    expect(localStorage.length).toBe(0);

    // An administrator, so both sidebar groups render.
    const session: Session = {
      token: "test-token",
      user_id: "ADMIN001",
      usr_type: "A",
      landing: "/admin",
      expires_at: new Date(Date.now() + 3_600_000).toISOString(),
    };
    setSession(session);

    const queryClient = new QueryClient({
      defaultOptions: {
        queries: { retry: false },
        mutations: { retry: false },
      },
    });
    const router = createMemoryRouter(
      [
        {
          element: createElement(AppRoot),
          children: [
            {
              path: "/",
              element: createElement(AppLayout),
              children: [
                {
                  index: true,
                  element: createElement("p", null, "Shell content"),
                },
              ],
            },
          ],
        },
      ],
      { initialEntries: ["/"] },
    );

    // The provider order of main.tsx, without the Toaster.
    render(
      createElement(ThemeProvider, {
        defaultTheme: "light",
        children: createElement(QueryClientProvider, {
          client: queryClient,
          children: createElement(TooltipProvider, {
            children: createElement(RouterProvider, { router }),
          }),
        }),
      }),
    );
    await screen.findByText("Shell content");
    expect(screen.getByRole("list", { name: "Administration" })).toBeTruthy();

    const user = userEvent.setup();

    // The setup.ts matchMedia stub matches no query, so the shell starts as
    // the icon rail. The trigger opens it and the generated Ctrl+B shortcut
    // closes it again, the two paths through the provider's setOpen.
    expect(sidebarState()).toBe("collapsed");
    await user.click(screen.getByRole("button", { name: /toggle sidebar/i }));
    await waitFor(() => {
      expect(sidebarState()).toBe("expanded");
    });
    await user.keyboard("{Control>}b{/Control}");
    await waitFor(() => {
      expect(sidebarState()).toBe("collapsed");
    });

    // The theme changes through ModeToggle's menu, as a user changes it.
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    await user.click(screen.getByRole("button", { name: "Toggle theme" }));
    await user.click(await screen.findByRole("menuitem", { name: "Dark" }));
    await waitFor(() => {
      expect(document.documentElement.classList.contains("dark")).toBe(true);
    });

    // A page writes what it keeps for its next load at the latest on pagehide.
    window.dispatchEvent(new Event("pagehide"));

    expect(document.cookie).toBe("");
    expect(localStorage.length).toBe(0);
    // The session store is the one in use, and it holds the session and the
    // theme alone (AAP 0.4.1), so any third key fails.
    const sessionKeys = Array.from(
      { length: sessionStorage.length },
      (_, index) => sessionStorage.key(index),
    ).sort();
    expect(sessionKeys).toEqual(
      [SESSION_STORAGE_KEY, THEME_STORAGE_KEY].sort(),
    );
  });
});
