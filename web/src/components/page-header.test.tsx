// Tests of the breadcrumb trail PageHeader builds from the matched routes
// (components/page-header.tsx), on the shell and account routes as App.tsx
// gives them. The shell's "Home" crumb links to "/". The keyed crumb above the
// account update page links to the account's own URL as the current URL
// spells it, so a key holding a space, "#", "?", "%" or "/" leads back to the
// same account, under a memory router entered at an encoded URL and under the
// browser router App.tsx uses after a navigation to a raw key. The page's own
// crumb is the current page, not a link.
import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { JSX } from "react";
import {
  createBrowserRouter,
  createMemoryRouter,
  Outlet,
  RouterProvider,
  type RouteObject,
} from "react-router";
import { afterEach, describe, expect, it } from "vitest";

import { PageHeader, type RouteHandle } from "@/components/page-header";
import { menu } from "@/nav/menu";

type Router = ReturnType<typeof createMemoryRouter>;

// The label of the nav/menu.ts entry `id`, as App.tsx's crumbs take it.
function menuLabel(id: string): string {
  const entry = menu.find((candidate) => candidate.id === id);
  if (entry === undefined) {
    throw new Error(`nav/menu.ts has no menu entry ${id}`);
  }
  return entry.label;
}

const ACCOUNT_VIEW = menuLabel("COMEN02Y-01");
const ACCOUNT_UPDATE = menuLabel("COMEN02Y-02");

// A page that renders only its header: the crumb trail and the h1.
function HeaderPage(): JSX.Element {
  return <PageHeader />;
}

// The App.tsx route shape: the pathless shell carrying the "Home" crumb, the
// pathless in-shell error boundary, the dashboard index, and the account view
// with the account update nested under it.
const routes: RouteObject[] = [
  {
    element: <Outlet />,
    handle: {
      title: "Main Menu",
      crumb: "Home",
      to: "/",
    } satisfies RouteHandle,
    children: [
      {
        children: [
          { index: true, element: <HeaderPage /> },
          {
            path: "accounts/:accountId",
            handle: {
              title: "View Account",
              crumb: ACCOUNT_VIEW,
            } satisfies RouteHandle,
            children: [
              { index: true, element: <HeaderPage /> },
              {
                path: "edit",
                element: <HeaderPage />,
                handle: {
                  title: "Update Account",
                  crumb: ACCOUNT_UPDATE,
                } satisfies RouteHandle,
              },
            ],
          },
        ],
      },
    ],
  },
];

let active: Router | undefined;

afterEach(() => {
  active?.dispose();
  active = undefined;
  // The browser router reads and writes jsdom's URL, which outlives a test.
  window.history.replaceState(null, "", "/");
});

function renderRouter(router: Router): ReturnType<typeof userEvent.setup> {
  active = router;
  const user = userEvent.setup();
  render(<RouterProvider router={router} />);
  return user;
}

function trail(): HTMLElement {
  return screen.getByRole("navigation", { name: "breadcrumb" });
}

// The crumb link `name`: an anchor, unlike the current page's crumb.
function crumbLink(name: string): HTMLAnchorElement {
  const link = within(trail()).getByRole("link", { name });
  if (!(link instanceof HTMLAnchorElement)) {
    throw new Error(`the crumb ${name} is not a link`);
  }
  return link;
}

// Follows the "Account View" crumb and checks that it lands on the view of
// the account `accountId`, at `href`, with that crumb now the current page.
async function followAccountCrumb(
  user: ReturnType<typeof userEvent.setup>,
  router: Router,
  href: string,
  accountId: string,
): Promise<void> {
  await user.click(crumbLink(ACCOUNT_VIEW));
  expect(router.state.location.pathname).toBe(href);
  // The view is the index page of the account route.
  const matches = router.state.matches;
  expect(matches.at(-2)?.route.path).toBe("accounts/:accountId");
  expect(matches.at(-1)?.route.index).toBe(true);
  expect(matches.at(-1)?.params).toEqual({ accountId });
  expect(
    screen.getByRole("heading", { level: 1, name: "View Account" }),
  ).toBeDefined();
  const current = within(trail()).getByText(ACCOUNT_VIEW);
  expect(current.tagName).toBe("SPAN");
  expect(current.getAttribute("aria-current")).toBe("page");
}

describe("PageHeader breadcrumb trail", () => {
  it.each([
    ["/accounts/00000000001/edit", "/accounts/00000000001", "00000000001"],
    ["/accounts/%20%20/edit", "/accounts/%20%20", "  "],
    ["/accounts/1%23/edit", "/accounts/1%23", "1#"],
    ["/accounts/1%3F/edit", "/accounts/1%3F", "1?"],
    ["/accounts/1%25/edit", "/accounts/1%25", "1%"],
    ["/accounts/a%2Fb/edit", "/accounts/a%2Fb", "a/b"],
    ["/accounts/%20%20/edit/", "/accounts/%20%20", "  "],
  ])(
    "at %s links Account View to %s, the view of account %j",
    async (entry, href, accountId) => {
      const router = createMemoryRouter(routes, { initialEntries: [entry] });
      const user = renderRouter(router);

      // The memory router keeps the entry as it was written, encoded.
      expect(router.state.location.pathname).toBe(entry);
      expect(router.state.matches.at(-1)?.params).toEqual({ accountId });
      expect(crumbLink("Home").getAttribute("href")).toBe("/");
      expect(crumbLink(ACCOUNT_VIEW).getAttribute("href")).toBe(href);
      const page = within(trail()).getByText(ACCOUNT_UPDATE);
      expect(page.tagName).toBe("SPAN");
      expect(page.getAttribute("aria-current")).toBe("page");
      expect(
        screen.getByRole("heading", { level: 1, name: "Update Account" }),
      ).toBeDefined();

      await followAccountCrumb(user, router, href, accountId);
    },
  );

  it("links Account View to the URL the browser router encodes a raw key to", async () => {
    window.history.replaceState(null, "", "/");
    const router = createBrowserRouter(routes);
    const user = renderRouter(router);

    await act(() => router.navigate("/accounts/a b/edit"));

    // The browser history spells the navigation's pathname as the URL does.
    expect(router.state.location.pathname).toBe("/accounts/a%20b/edit");
    expect(window.location.pathname).toBe("/accounts/a%20b/edit");
    expect(crumbLink(ACCOUNT_VIEW).getAttribute("href")).toBe(
      "/accounts/a%20b",
    );

    await followAccountCrumb(user, router, "/accounts/a%20b", "a b");
    expect(window.location.pathname).toBe("/accounts/a%20b");
  });
});
