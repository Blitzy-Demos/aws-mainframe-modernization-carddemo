// Tests of the root route's element (routes/app-root.tsx). The document title
// is the matched routes' title followed by the application name on every
// route, a route that shows its errorElement included, and the application
// name alone where no route gives a title. A push or replace to another
// screen opens it at its top, Back and Forward return to each history entry's
// position, and a navigation flagged preventScrollReset, the update within a
// page, keeps the position.
//
// The routes carry handles as App.tsx gives them: a titled route, an index
// route that takes its parent's handle, a handle with a crumb only, and
// routes without a title. jsdom implements no scrolling, so window.scrollTo is
// stubbed to move window.scrollY, the position ScrollRestoration reads.
import { act, render, screen } from "@testing-library/react";
import { StrictMode, type JSX } from "react";
import {
  createMemoryRouter,
  Outlet,
  RouterProvider,
  type RouteObject,
} from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { PageHeader, type RouteHandle } from "@/components/page-header";
import { AppRoot } from "@/routes/app-root";

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

// The routes on a memory router, under StrictMode as main.tsx renders them,
// so the mount effects run twice as in development.
function renderRoot(initialPath: string): Router {
  const router = createMemoryRouter(routes, { initialEntries: [initialPath] });
  render(
    <StrictMode>
      <RouterProvider router={router} />
    </StrictMode>,
  );
  return router;
}

// Places the window at `y`, as the user's scrolling or a scrollTo call does.
function scrollWindow(y: number): void {
  Object.defineProperty(window, "scrollY", {
    configurable: true,
    writable: true,
    value: y,
  });
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
});

afterEach(() => {
  vi.restoreAllMocks();
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
});

describe("AppRoot scroll position", () => {
  it.each([
    {
      navigation: "a push to another screen",
      go: (router: Router) => router.navigate("/transactions/7"),
      where: "at the top",
      scrollY: 0,
    },
    {
      navigation: "a replace to another screen",
      go: (router: Router) =>
        router.navigate("/transactions/7", { replace: true }),
      where: "at the top",
      scrollY: 0,
    },
    {
      // CPVD's Back link: a push to the summary, not a history traversal.
      navigation: "a push back to a screen shown before",
      go: async (router: Router) => {
        await router.navigate("/transactions/7");
        scrollWindow(50);
        await router.navigate("/transactions?account_id=00000000001");
      },
      where: "at the top",
      scrollY: 0,
    },
    {
      // AuthorizationSummaryPage's account filter.
      navigation: "a search change flagged preventScrollReset",
      go: (router: Router) =>
        router.navigate("/transactions?account_id=00000000001", {
          preventScrollReset: true,
        }),
      where: "where it was",
      scrollY: 300,
    },
  ])("leaves the window $where after $navigation", async ({ go, scrollY }) => {
    const router = renderRoot("/transactions");
    scrollWindow(300);

    await act(() => go(router));

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
});
