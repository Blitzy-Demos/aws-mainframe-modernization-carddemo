// Tests of the root route's element (routes/app-root.tsx). The document title
// is the matched routes' title followed by the application name on every
// route, a route that shows its errorElement included, and the application
// name alone where no route gives a title; routes from a guard the session
// does not pass down never title it. A push or replace to another pathname
// opens it at its top, a change of the search string alone, the update within
// a page, keeps the position, Back and Forward return to each history entry's
// position or the top, and the document load leaves the position alone. The
// position the window takes while the next page loads is not recorded, and
// nothing is written to sessionStorage.
//
// The routes carry handles as App.tsx gives them: a titled route, an index
// route that takes its parent's handle, a handle with a crumb only, routes
// without a title, and the guard routes guardRoute marks. jsdom implements no
// scrolling, so window.scrollTo is stubbed to move window.scrollY, and the
// tests fire the scroll event a browser fires when the position changes.
import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode, type JSX } from "react";
import {
  createMemoryRouter,
  Outlet,
  RouterProvider,
  type RouteObject,
} from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { PageHeader, type RouteHandle } from "@/components/page-header";
import {
  clearSession,
  SESSION_STORAGE_KEY,
  setSession,
  type Session,
} from "@/lib/session";
import { AppRoot } from "@/routes/app-root";
import { guardRoute } from "@/routes/guards";

type Router = ReturnType<typeof createMemoryRouter>;

// A page that heads itself with its route's title, as every page does.
function TitledPage(): JSX.Element {
  return <PageHeader />;
}

function UntitledPage(): JSX.Element {
  return <p>Untitled page</p>;
}

// A page whose render fails.
function FailingPage(): JSX.Element {
  throw new Error("The page failed to render");
}

// A route loader that fails, as the download of a page's chunk can.
function failingLoader(): Promise<null> {
  return Promise.reject(new Error("The page's chunk failed to load"));
}

// The runs of pendingLoad still waiting, oldest first.
const pendingLoads: (() => void)[] = [];

// A route loader that waits until the test settles it, as the download of a
// page's chunk keeps the router loading.
function pendingLoad(): Promise<null> {
  return new Promise((resolve) => {
    pendingLoads.push(() => {
      resolve(null);
    });
  });
}

function settlePendingLoad(): void {
  const settle = pendingLoads.shift();
  if (settle === undefined) {
    throw new Error("No pendingLoad run is waiting");
  }
  settle();
}

const ERROR_TEXT = "The page could not be shown";

const routes: RouteObject[] = [
  {
    element: <AppRoot />,
    children: [
      {
        path: "/login",
        element: <TitledPage />,
        handle: { title: "Sign on", crumb: "Sign on" } satisfies RouteHandle,
      },
      {
        // The signed-on routes, under RequireAuth as App.tsx places it.
        ...guardRoute("signed-on"),
        children: [
          {
            path: "/",
            element: <Outlet />,
            handle: {
              title: "Main Menu",
              crumb: "Home",
              to: "/",
            } satisfies RouteHandle,
            children: [
              { index: true, element: <TitledPage /> },
              {
                path: "transactions",
                handle: {
                  title: "List Transactions",
                  crumb: "Transaction List",
                } satisfies RouteHandle,
                children: [
                  { index: true, element: <TitledPage /> },
                  {
                    path: ":tranId",
                    element: <TitledPage />,
                    handle: {
                      title: "View Transaction",
                      crumb: "Transaction View",
                    } satisfies RouteHandle,
                  },
                ],
              },
              {
                path: "authorizations/:authId",
                element: <TitledPage />,
                handle: {
                  crumb: "View Authorization Details",
                } satisfies RouteHandle,
              },
              {
                path: "payments",
                element: <FailingPage />,
                errorElement: <p>{ERROR_TEXT}</p>,
                handle: {
                  title: "Bill Payment",
                  crumb: "Bill Payment",
                } satisfies RouteHandle,
              },
              {
                path: "reports/transactions",
                loader: failingLoader,
                hydrateFallbackElement: <p>Loading</p>,
                element: <TitledPage />,
                errorElement: <p>{ERROR_TEXT}</p>,
                handle: {
                  title: "Transaction Reports",
                  crumb: "Transaction Reports",
                } satisfies RouteHandle,
              },
              {
                path: "statements",
                loader: pendingLoad,
                element: <TitledPage />,
                handle: {
                  title: "Statements",
                  crumb: "Statements",
                } satisfies RouteHandle,
              },
              {
                // The admin routes, under RequireAdmin as App.tsx places it.
                path: "admin",
                ...guardRoute("admin", {
                  title: "Admin Menu",
                  crumb: "Administration",
                }),
                children: [
                  { index: true, element: <TitledPage /> },
                  {
                    path: "users",
                    element: <TitledPage />,
                    handle: {
                      title: "List Users",
                      crumb: "User List",
                    } satisfies RouteHandle,
                  },
                ],
              },
            ],
          },
        ],
      },
      { path: "/untitled", element: <UntitledPage /> },
      {
        path: "/blank",
        element: <UntitledPage />,
        handle: { crumb: "" } satisfies RouteHandle,
      },
    ],
  },
];

// The routes on a memory router at `initialPath`, after the history entries
// `earlier`, under StrictMode as main.tsx renders them, so the mount effects
// run twice as in development.
function renderRoot(
  initialPath: string,
  earlier: readonly string[] = [],
): Router {
  const router = createMemoryRouter(routes, {
    initialEntries: [...earlier, initialPath],
    initialIndex: earlier.length,
  });
  render(
    <StrictMode>
      <RouterProvider router={router} />
    </StrictMode>,
  );
  return router;
}

// Places the window at `y`, as the user's scrolling or a scrollTo call does,
// and fires the scroll event the browser fires for it.
function scrollWindow(y: number): void {
  Object.defineProperty(window, "scrollY", {
    configurable: true,
    writable: true,
    value: y,
  });
  window.dispatchEvent(new Event("scroll"));
}

// The keys sessionStorage holds, sorted.
function storedKeys(): string[] {
  return Array.from({ length: sessionStorage.length }, (_, index) =>
    sessionStorage.key(index),
  )
    .filter((key): key is string => key !== null)
    .sort();
}

// A session of user type `usrType`, as the login response gives it.
function signOn(usrType: string): void {
  const session: Session = {
    token: "test-token",
    user_id: usrType === "A" ? "ADMIN001" : "USER0001",
    usr_type: usrType,
    landing: usrType === "A" ? "/admin" : "/",
    expires_at: new Date(Date.now() + 3_600_000).toISOString(),
  };
  setSession(session);
}

// Records every title the document is given from now on. The returned
// function lists them in order, a repeat of the title just given (StrictMode's
// second effect run) folded into one.
function recordTitles(): () => string[] {
  const setter = vi.spyOn(document, "title", "set");
  return () =>
    setter.mock.calls
      .map(([title]) => title)
      .filter((title, index, titles) => title !== titles[index - 1]);
}

beforeEach(() => {
  // A title no route sets, so every assertion sees one the root wrote.
  document.title = "Stale title";
  scrollWindow(0);
  vi.spyOn(window, "scrollTo").mockImplementation(
    (_x: number, y: number) => {
      scrollWindow(y);
    },
  );
  // A regular user, whom RequireAuth lets through to every route but /admin.
  signOn("U");
});

// setup.ts clears sessionStorage; session.ts also keeps the session in a
// module variable with an expiry timer, which only clearSession resets.
afterEach(() => {
  vi.restoreAllMocks();
  clearSession();
});

describe("AppRoot document title", () => {
  it.each([
    { path: "/login", title: "Sign on – CardDemo" },
    { path: "/", title: "Main Menu – CardDemo" },
    { path: "/transactions", title: "List Transactions – CardDemo" },
    { path: "/transactions/7", title: "View Transaction – CardDemo" },
    {
      path: "/authorizations/7",
      title: "View Authorization Details – CardDemo",
    },
    { path: "/untitled", title: "CardDemo" },
    { path: "/blank", title: "CardDemo" },
  ])("is $title on $path", ({ path, title }) => {
    renderRoot(path);

    expect(document.title).toBe(title);
  });

  it("follows every navigation, Back included", async () => {
    const router = renderRoot("/transactions");
    expect(document.title).toBe("List Transactions – CardDemo");

    await act(() => router.navigate("/transactions/7"));
    expect(document.title).toBe("View Transaction – CardDemo");

    await act(() => router.navigate("/untitled"));
    expect(document.title).toBe("CardDemo");

    await act(() => router.navigate(-2));
    expect(document.title).toBe("List Transactions – CardDemo");
  });

  it.each([
    { failure: "its page's render", path: "/payments", title: "Bill Payment" },
    {
      failure: "its loader",
      path: "/reports/transactions",
      title: "Transaction Reports",
    },
  ])(
    "keeps the route's title while $failure fails and its errorElement shows",
    async ({ path, title }) => {
      // React and React Router report a caught render error on the console.
      vi.spyOn(console, "error").mockImplementation(() => undefined);

      renderRoot(path);

      expect((await screen.findByText(ERROR_TEXT)).tagName).toBe("P");
      expect(document.title).toBe(`${title} – CardDemo`);
    },
  );

  it.each([
    {
      visitor: "a regular user",
      usrType: "U",
      path: "/admin/users",
      landing: "/",
      titles: ["Main Menu – CardDemo"],
    },
    {
      visitor: "a regular user",
      usrType: "U",
      path: "/admin",
      landing: "/",
      titles: ["Main Menu – CardDemo"],
    },
    {
      visitor: "a visitor without a session",
      usrType: null,
      path: "/transactions",
      landing: "/login",
      titles: ["CardDemo", "Sign on – CardDemo"],
    },
    {
      visitor: "a visitor without a session",
      usrType: null,
      path: "/admin/users",
      landing: "/login",
      titles: ["CardDemo", "Sign on – CardDemo"],
    },
    {
      visitor: "an administrator",
      usrType: "A",
      path: "/admin/users",
      landing: "/admin/users",
      titles: ["List Users – CardDemo"],
    },
  ])(
    "titles $path for $visitor from the routes the guards admit, until it lands on $landing",
    async ({ usrType, path, landing, titles }) => {
      clearSession();
      if (usrType !== null) {
        signOn(usrType);
      }
      const titled = recordTitles();

      const router = renderRoot(path);

      await waitFor(() => {
        expect(router.state.location.pathname).toBe(landing);
      });
      await waitFor(() => {
        expect(document.title).toBe(titles.at(-1));
      });
      expect(titled()).toEqual(titles);
    },
  );
});

describe("AppRoot scroll position", () => {
  it.each([
    {
      navigation: "a push to another screen",
      go: (router: Router) => act(() => router.navigate("/transactions/7")),
      where: "at the top",
      scrollY: 0,
    },
    {
      navigation: "a replace to another screen",
      go: (router: Router) =>
        act(() => router.navigate("/transactions/7", { replace: true })),
      where: "at the top",
      scrollY: 0,
    },
    {
      // CPVD's Back link: a push to the summary, not a history traversal.
      // The detail page renders, and is scrolled, before the user leaves it.
      navigation: "a push back to a screen shown before",
      go: async (router: Router) => {
        await act(() => router.navigate("/transactions/7"));
        scrollWindow(50);
        await act(() =>
          router.navigate("/transactions?account_id=00000000001"),
        );
      },
      where: "at the top",
      scrollY: 0,
    },
    {
      // AuthorizationSummaryPage's account filter, flagged as it flags it.
      navigation: "a change of the search string alone, flagged",
      go: (router: Router) =>
        act(() =>
          router.navigate("/transactions?account_id=00000000001", {
            preventScrollReset: true,
          }),
        ),
      where: "where it was",
      scrollY: 300,
    },
    {
      navigation: "a change of the search string alone, unflagged",
      go: (router: Router) =>
        act(() => router.navigate("/transactions?account_id=00000000001")),
      where: "where it was",
      scrollY: 300,
    },
  ])("leaves the window $where after $navigation", async ({ go, scrollY }) => {
    const router = renderRoot("/transactions");
    scrollWindow(300);

    await go(router);

    expect(window.scrollY).toBe(scrollY);
  });

  it("returns to each history entry's position on Back and Forward", async () => {
    const router = renderRoot("/transactions");
    scrollWindow(300);

    await act(() => router.navigate("/transactions/7"));
    expect(window.scrollY).toBe(0);
    scrollWindow(40);

    await act(() => router.navigate(-1));
    expect(window.scrollY).toBe(300);

    await act(() => router.navigate(1));
    expect(window.scrollY).toBe(40);
  });

  it("returns on Back to the position a change of the search string kept", async () => {
    const router = renderRoot("/transactions");
    scrollWindow(300);
    await act(() => router.navigate("/transactions?account_id=00000000001"));
    await act(() => router.navigate("/transactions/7"));
    expect(window.scrollY).toBe(0);

    await act(() => router.navigate(-1));

    expect(router.state.location.search).toBe("?account_id=00000000001");
    expect(window.scrollY).toBe(300);
  });

  it("opens at the top on Back to an entry this document has not shown", async () => {
    const router = renderRoot("/transactions/7", ["/transactions"]);
    scrollWindow(200);

    await act(() => router.navigate(-1));

    expect(router.state.location.pathname).toBe("/transactions");
    expect(window.scrollY).toBe(0);
  });

  it("leaves the position alone on the document load", () => {
    scrollWindow(120);

    renderRoot("/transactions");

    expect(window.scrollY).toBe(120);
    expect(window.scrollTo).not.toHaveBeenCalled();
  });

  it("keeps the position an entry was left at when the window moves while the next page loads", async () => {
    const router = renderRoot("/transactions");
    scrollWindow(300);
    await act(async () => {
      void router.navigate("/statements");
    });
    expect(router.state.navigation.state).toBe("loading");
    // The shell's skeleton replaces the page and clamps the window.
    scrollWindow(0);

    await act(async () => {
      settlePendingLoad();
    });
    expect(router.state.location.pathname).toBe("/statements");
    await act(() => router.navigate(-1));

    expect(window.scrollY).toBe(300);
  });

  it("keeps history.scrollRestoration manual while mounted and auto from pagehide to pageshow", () => {
    window.history.scrollRestoration = "auto";

    renderRoot("/transactions");
    expect(window.history.scrollRestoration).toBe("manual");

    window.dispatchEvent(new Event("pagehide"));
    expect(window.history.scrollRestoration).toBe("auto");
    window.dispatchEvent(new Event("pageshow"));
    expect(window.history.scrollRestoration).toBe("manual");

    cleanup();
    expect(window.history.scrollRestoration).toBe("auto");
  });

  it("writes nothing to sessionStorage across navigations and a pagehide", async () => {
    const router = renderRoot("/transactions");
    scrollWindow(300);
    await act(() => router.navigate("/transactions/7"));
    scrollWindow(40);
    await act(() => router.navigate("/transactions?account_id=00000000001"));
    await act(() => router.navigate(-2));

    window.dispatchEvent(new Event("pagehide"));

    // The signed-on session alone, which beforeEach stored.
    expect(storedKeys()).toEqual([SESSION_STORAGE_KEY]);
  });
});
