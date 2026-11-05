// Tests of the app shell's route-change handling (components/app-layout.tsx):
// a change of pathname other than the document load moves focus to the page
// content, unless the new page placed focus on one of its own controls, and
// the shell's status region announces the title PageHeader shows. The
// document load and a change of the search string alone do neither, and two
// consecutive destinations with one title are each announced. While the
// chunk of the page a navigation to another pathname goes to downloads, the
// page content shows the loading skeleton and is marked busy, until that page
// renders. A navigation to a page whose chunk has arrived, Back included,
// keeps the current page in place instead, and a change of the search string
// alone shows neither, even while a loader runs for it and a chunk downloads.
//
// The shell renders under the providers of main.tsx (without the Toaster),
// StrictMode included, so its mount effect runs twice as in development.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode, useEffect, useRef, type JSX } from "react";
import {
  createMemoryRouter,
  NavigationType,
  RouterProvider,
  useNavigate,
  useNavigation,
  type RouteObject,
} from "react-router";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { AppLayout } from "@/components/app-layout";
import { PageHeader, type RouteHandle } from "@/components/page-header";
import { ThemeProvider } from "@/components/theme-provider";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { TooltipProvider } from "@/components/ui/tooltip";
import { clearSession, setSession, type Session } from "@/lib/session";
import { menu, type MenuEntry } from "@/nav/menu";
import {
  lazyPage,
  pageDownloadsInFlight,
  pageRoute,
  type LazyPage,
  type PageModule,
} from "@/routes/lazy-page";

type Router = ReturnType<typeof createMemoryRouter>;
type User = ReturnType<typeof userEvent.setup>;

// The nav/menu.ts entry `id`, whose sidebar link and route the tests use.
function menuEntry(id: string): MenuEntry {
  const entry = menu.find((candidate) => candidate.id === id);
  if (entry === undefined) {
    throw new Error(`nav/menu.ts has no menu entry ${id}`);
  }
  return entry;
}

const TRANSACTION_LIST = menuEntry("COMEN02Y-06");
const CARD_LIST = menuEntry("COMEN02Y-03");
const BILL_PAYMENT = menuEntry("COMEN02Y-10");

// The shell's handle as App.tsx gives it, the heading of the index page.
const SHELL_HANDLE = {
  title: "Main Menu",
  crumb: "Home",
  to: "/",
} satisfies RouteHandle;

// One handle on two routes, as App.tsx gives the two TranTypeFormPage routes.
const TYPE_MAINTENANCE = {
  title: "Maintain Transaction Type",
  crumb: "Transaction Type Maintenance",
} satisfies RouteHandle;

// A page that places no focus: its header (crumb trail and h1) and its text.
function PlainPage(): JSX.Element {
  return (
    <>
      <PageHeader />
      <p>Page text</p>
    </>
  );
}

// A page whose filter takes focus through autoFocus, as CardListPage's does.
function AutoFocusPage(): JSX.Element {
  return (
    <>
      <PageHeader />
      <Input aria-label="Filter" autoFocus />
    </>
  );
}

// A page that focuses its first field from a mount effect, as the form pages
// do through form.setFocus. It focuses once, as a production build runs the
// effect: StrictMode's second run of a new component's effects comes after
// the shell's effect and would refocus the field whatever the shell did.
function EffectFocusPage(): JSX.Element {
  const field = useRef<HTMLInputElement>(null);
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current) {
      focused.current = true;
      field.current?.focus();
    }
  }, []);
  return (
    <>
      <PageHeader />
      <Input ref={field} aria-label="Account" />
    </>
  );
}

// The sign-on screen outside the shell: like LoginPage, it lands in the shell
// with a replace navigation, and its button leaves with it.
function SignOnPage(): JSX.Element {
  const navigate = useNavigate();
  return (
    <Button
      onClick={() => {
        void navigate("/", { replace: true });
      }}
    >
      Sign on
    </Button>
  );
}

const routes: RouteObject[] = [
  { path: "/login", element: <SignOnPage /> },
  {
    path: "/",
    element: <AppLayout />,
    handle: SHELL_HANDLE,
    children: [
      { index: true, element: <PlainPage /> },
      {
        path: TRANSACTION_LIST.route,
        element: <PlainPage />,
        handle: {
          title: "List Transactions",
          crumb: TRANSACTION_LIST.label,
        } satisfies RouteHandle,
      },
      {
        path: CARD_LIST.route,
        element: <AutoFocusPage />,
        handle: {
          title: "List Credit Cards",
          crumb: CARD_LIST.label,
        } satisfies RouteHandle,
      },
      {
        path: BILL_PAYMENT.route,
        element: <EffectFocusPage />,
        handle: {
          title: "Bill Payment",
          crumb: BILL_PAYMENT.label,
        } satisfies RouteHandle,
      },
      {
        path: "accounts/:accountId",
        handle: {
          title: "View Account",
          crumb: "Account View",
        } satisfies RouteHandle,
        children: [
          { index: true, element: <PlainPage /> },
          {
            path: "edit",
            element: <PlainPage />,
            handle: {
              title: "Update Account",
              crumb: "Account Update",
            } satisfies RouteHandle,
          },
        ],
      },
      {
        path: "transaction-types/new",
        element: <PlainPage />,
        handle: TYPE_MAINTENANCE,
      },
      {
        path: "transaction-types/:code/edit",
        element: <PlainPage />,
        handle: TYPE_MAINTENANCE,
      },
    ],
  },
];

function renderShell(
  initialPath: string,
  routeTable: RouteObject[] = routes,
): { router: Router; user: User } {
  const router = createMemoryRouter(routeTable, {
    initialEntries: [initialPath],
  });
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  render(
    <StrictMode>
      <ThemeProvider defaultTheme="light">
        <QueryClientProvider client={queryClient}>
          <TooltipProvider>
            <RouterProvider router={router} />
          </TooltipProvider>
        </QueryClientProvider>
      </ThemeProvider>
    </StrictMode>,
  );
  return { router, user: userEvent.setup() };
}

// The shell's route-change status region, the only status role rendered here.
function statusRegion(): HTMLElement {
  return screen.getByRole("status");
}

// The page content, the shell's single main landmark.
function pageContent(): HTMLElement {
  return screen.getByRole("main");
}

// The sidebar link `name`. The scope keeps out the breadcrumb's current page,
// which the generated BreadcrumbPage gives the link role under the same name.
function sidebarLink(name: string): HTMLElement {
  return within(screen.getByRole("navigation", { name: "Main" })).getByRole(
    "link",
    { name },
  );
}

async function expectPage(title: string): Promise<void> {
  await screen.findByRole("heading", { level: 1, name: title });
}

// The page `title` rendered and was announced: its h1 and the status region
// read the same text.
async function expectAnnounced(title: string): Promise<void> {
  await expectPage(title);
  await waitFor(() => {
    expect(statusRegion().textContent).toBe(title);
  });
}

// A regular user, so the user menu renders and the sidebar holds the user
// section only.
beforeEach(() => {
  const session: Session = {
    token: "test-token",
    user_id: "USER0001",
    usr_type: "U",
    landing: "/",
    expires_at: new Date(Date.now() + 3_600_000).toISOString(),
  };
  setSession(session);
});

// setup.ts clears sessionStorage; session.ts also keeps the session in a
// module variable with an expiry timer, which only clearSession resets.
afterEach(() => {
  clearSession();
  document.documentElement.className = "";
});

describe("AppLayout route changes", () => {
  it("leaves focus on the body and the status region empty on the document load", async () => {
    const { user } = renderShell("/");
    await expectPage(SHELL_HANDLE.title);

    expect(document.activeElement).toBe(document.body);
    expect(statusRegion().textContent).toBe("");
    expect(statusRegion().childElementCount).toBe(0);
    // The region sits outside the page content and takes no Tab stop, so the
    // first Tab still reaches the skip link.
    expect(pageContent().contains(statusRegion())).toBe(false);
    await user.tab();
    expect(document.activeElement).toBe(
      screen.getByRole("link", { name: "Skip to content" }),
    );
  });

  it.each([
    {
      trigger: "a sidebar link",
      go: (user: User): Promise<void> =>
        user.click(sidebarLink(TRANSACTION_LIST.label)),
    },
    {
      trigger: "a programmatic navigation",
      go: (_user: User, router: Router): Promise<void> =>
        act(() => router.navigate(TRANSACTION_LIST.route)),
    },
  ])(
    "moves focus to the page content and announces the title after $trigger",
    async ({ go }) => {
      const { router, user } = renderShell("/");
      await expectPage(SHELL_HANDLE.title);

      await go(user, router);

      await expectAnnounced("List Transactions");
      expect(document.activeElement).toBe(pageContent());
    },
  );

  it.each([
    { crumb: "Account View", title: "View Account" },
    { crumb: SHELL_HANDLE.crumb, title: SHELL_HANDLE.title },
  ])(
    "moves focus to the page content when the crumb $crumb leaves with its page",
    async ({ crumb, title }) => {
      const { user } = renderShell("/accounts/00000000011/edit");
      await expectPage("Update Account");
      const link = within(pageContent()).getByRole("link", { name: crumb });

      await user.click(link);

      await expectAnnounced(title);
      expect(link.isConnected).toBe(false);
      expect(document.activeElement).toBe(pageContent());
    },
  );

  it.each([
    {
      placement: "autoFocus",
      entry: CARD_LIST,
      title: "List Credit Cards",
      field: "Filter",
    },
    {
      placement: "a mount effect",
      entry: BILL_PAYMENT,
      title: "Bill Payment",
      field: "Account",
    },
  ])(
    "keeps the focus the new page placed through $placement and still announces its title",
    async ({ entry, title, field }) => {
      const { user } = renderShell("/");
      await expectPage(SHELL_HANDLE.title);

      await user.click(sidebarLink(entry.label));

      await expectAnnounced(title);
      expect(document.activeElement).toBe(
        screen.getByRole("textbox", { name: field }),
      );
    },
  );

  it("neither moves focus nor announces when only the search string changes", async () => {
    const { router } = renderShell("/");
    await expectPage(SHELL_HANDLE.title);
    await act(() => router.navigate(TRANSACTION_LIST.route));
    await expectAnnounced("List Transactions");
    const announcement = statusRegion().firstElementChild;
    expect(announcement).not.toBeNull();
    const link = sidebarLink(TRANSACTION_LIST.label);
    act(() => {
      link.focus();
    });

    await act(() =>
      router.navigate(`${TRANSACTION_LIST.route}?account_id=00000000011`),
    );

    expect(router.state.location.search).toBe("?account_id=00000000011");
    expect(document.activeElement).toBe(link);
    expect(statusRegion().firstElementChild).toBe(announcement);
    expect(statusRegion().textContent).toBe("List Transactions");
  });

  it("announces each of two consecutive destinations that share a title", async () => {
    const { router } = renderShell("/");
    await expectPage(SHELL_HANDLE.title);
    await act(() => router.navigate("/transaction-types/new"));
    await expectAnnounced(TYPE_MAINTENANCE.title);
    const first = statusRegion().firstElementChild;
    expect(first).not.toBeNull();

    await act(() => router.navigate("/transaction-types/01/edit"));

    await waitFor(() => {
      expect(statusRegion().firstElementChild).not.toBe(first);
    });
    expect(first?.isConnected).toBe(false);
    expect(statusRegion().childElementCount).toBe(1);
    expect(statusRegion().textContent).toBe(TYPE_MAINTENANCE.title);
    expect(document.activeElement).toBe(pageContent());
  });

  it("treats the shell's first render after an in-app navigation, the sign-on landing, as a route change", async () => {
    const { user } = renderShell("/login");

    await user.click(screen.getByRole("button", { name: "Sign on" }));

    await expectAnnounced(SHELL_HANDLE.title);
    expect(statusRegion().childElementCount).toBe(1);
    expect(document.activeElement).toBe(pageContent());
  });
});

// A page module import the test settles, as the browser settles the download
// of a page's chunk.
type PendingImport = {
  promise: Promise<PageModule>;
  resolve: (module: PageModule) => void;
};

function pendingImport(): PendingImport {
  let resolve: (module: PageModule) => void = () => undefined;
  const promise = new Promise<PageModule>((onResolve) => {
    resolve = onResolve;
  });
  return { promise, resolve };
}

const BILL_PAYMENT_MODULE: PageModule = { default: PlainPage };

// Route loaders the test settles, the way a loader waits on data. Each run of
// the loader waits on its own.
type PendingLoads = {
  loader: () => Promise<null>;
  /** Settles the latest run, inside act so that its render commits. */
  finishLatest: () => Promise<void>;
  /** The number of runs the router has started. */
  count: () => number;
};

function pendingLoads(): PendingLoads {
  const settlers: (() => void)[] = [];
  return {
    loader: () =>
      new Promise<null>((resolve) => {
        settlers.push(() => {
          resolve(null);
        });
      }),
    finishLatest: () =>
      act(async () => {
        const settle = settlers.at(-1);
        if (settle === undefined) {
          throw new Error("No route loader has run");
        }
        settle();
      }),
    count: () => settlers.length,
  };
}

// The router's navigation state as the rendered page last saw it, so a test
// knows the loading state has been rendered.
function NavigationStateProbe(): JSX.Element {
  return <p>Navigation {useNavigation().state}</p>;
}

// The shell with two pages whose loading the test settles: Bill Payment, a
// lazy page route and a direct child of the shell as it is in App.tsx, whose
// loader downloads `billPayment`, and the transaction list, whose loader is
// one of `loads` and also runs on a change of its search string
// (react-router's default shouldRevalidate).
function loadingRoutes(
  billPayment: LazyPage,
  loads: PendingLoads,
): RouteObject[] {
  return [
    {
      path: "/",
      element: <AppLayout />,
      handle: SHELL_HANDLE,
      children: [
        { index: true, element: <PlainPage /> },
        {
          path: BILL_PAYMENT.route,
          ...pageRoute(billPayment, "signed-on"),
          handle: {
            title: "Bill Payment",
            crumb: BILL_PAYMENT.label,
          } satisfies RouteHandle,
        },
        {
          path: TRANSACTION_LIST.route,
          loader: loads.loader,
          element: (
            <>
              <PlainPage />
              <NavigationStateProbe />
            </>
          ),
          handle: {
            title: "List Transactions",
            crumb: TRANSACTION_LIST.label,
          } satisfies RouteHandle,
        },
      ],
    },
  ];
}

// The page skeleton inside the page content, or null.
function contentSkeleton(): HTMLElement | null {
  return within(pageContent()).queryByRole("status", { name: "Loading" });
}

// The text of every h1 added to the document from now until `stop`, which
// returns them in the order they were added.
function observeAddedHeadings(): { stop: () => string[] } {
  const added: string[] = [];
  const collect = (records: MutationRecord[]): void => {
    for (const record of records) {
      for (const node of record.addedNodes) {
        if (node instanceof Element) {
          const headings = node.matches("h1")
            ? [node]
            : Array.from(node.querySelectorAll("h1"));
          added.push(...headings.map((heading) => heading.textContent));
        }
      }
    }
  };
  const observer = new MutationObserver(collect);
  observer.observe(document.body, { childList: true, subtree: true });
  return {
    stop: () => {
      collect(observer.takeRecords());
      observer.disconnect();
      return added;
    },
  };
}

describe("AppLayout while the next page loads", () => {
  it("shows the loading skeleton in the busy page content while the next page's chunk downloads, until that page renders", async () => {
    const chunk = pendingImport();
    const { user } = renderShell(
      "/",
      loadingRoutes(
        lazyPage(() => chunk.promise),
        pendingLoads(),
      ),
    );
    await expectPage(SHELL_HANDLE.title);
    expect(pageContent().hasAttribute("aria-busy")).toBe(false);

    await user.click(sidebarLink(BILL_PAYMENT.label));

    await waitFor(() => {
      expect(contentSkeleton()).not.toBeNull();
    });
    expect(pageContent().getAttribute("aria-busy")).toBe("true");
    expect(screen.queryByRole("heading", { level: 1 })).toBeNull();
    // The shell around the content stays in place.
    expect(sidebarLink(BILL_PAYMENT.label)).toBeDefined();
    expect(pageDownloadsInFlight()).toBe(1);
    const headings = observeAddedHeadings();

    // Settled outside act, so React schedules the renders as in the browser,
    // where the download settles before the router commits the page.
    chunk.resolve(BILL_PAYMENT_MODULE);

    await expectAnnounced("Bill Payment");
    // The skeleton held from the download's end until the page rendered: the
    // main menu did not come back in between.
    expect(headings.stop()).toEqual(["Bill Payment"]);
    expect(contentSkeleton()).toBeNull();
    expect(pageContent().hasAttribute("aria-busy")).toBe(false);
    expect(pageDownloadsInFlight()).toBe(0);
  });

  it.each([
    {
      trigger: "a link to it",
      action: NavigationType.Push,
      go: (router: Router): Promise<void> =>
        router.navigate(BILL_PAYMENT.route),
    },
    {
      trigger: "Back",
      action: NavigationType.Pop,
      go: (router: Router): Promise<void> => router.navigate(-1),
    },
  ])(
    "keeps the current page in place, with no skeleton and no busy state, on a navigation through $trigger to a page whose chunk has arrived",
    async ({ action, go }) => {
      const chunk = pendingImport();
      const loads = pendingLoads();
      const { router } = renderShell(
        "/",
        loadingRoutes(
          lazyPage(() => chunk.promise),
          loads,
        ),
      );
      await expectPage(SHELL_HANDLE.title);
      act(() => {
        void router.navigate(BILL_PAYMENT.route);
      });
      await act(async () => {
        chunk.resolve(BILL_PAYMENT_MODULE);
      });
      await expectPage("Bill Payment");
      act(() => {
        void router.navigate(TRANSACTION_LIST.route);
      });
      await loads.finishLatest();
      await expectPage("List Transactions");
      const headings = observeAddedHeadings();

      act(() => {
        void go(router);
      });

      // The router is loading Bill Payment, and the transaction list stands.
      expect(screen.getByText("Navigation loading")).toBeDefined();
      expect(contentSkeleton()).toBeNull();
      expect(pageContent().hasAttribute("aria-busy")).toBe(false);
      expect(
        screen.getByRole("heading", { level: 1, name: "List Transactions" }),
      ).toBeDefined();

      await expectAnnounced("Bill Payment");
      expect(router.state.historyAction).toBe(action);
      expect(headings.stop()).toEqual(["Bill Payment"]);
      expect(contentSkeleton()).toBeNull();
      expect(pageDownloadsInFlight()).toBe(0);
    },
  );

  it("shows no skeleton while a loader runs for a change of the search string alone, even while a page's chunk downloads", async () => {
    const chunk = pendingImport();
    const loads = pendingLoads();
    const { router } = renderShell(
      "/",
      loadingRoutes(
        lazyPage(() => chunk.promise),
        loads,
      ),
    );
    await expectPage(SHELL_HANDLE.title);
    act(() => {
      void router.navigate(TRANSACTION_LIST.route);
    });
    await loads.finishLatest();
    await expectPage("List Transactions");
    // A navigation to Bill Payment starts the download of its chunk, and the
    // change of the search string below interrupts it.
    act(() => {
      void router.navigate(BILL_PAYMENT.route);
    });
    await waitFor(() => {
      expect(contentSkeleton()).not.toBeNull();
    });

    act(() => {
      void router.navigate(`${TRANSACTION_LIST.route}?account_id=00000000011`);
    });

    await screen.findByText("Navigation loading");
    expect(pageDownloadsInFlight()).toBe(1);
    expect(loads.count()).toBe(2);
    expect(contentSkeleton()).toBeNull();
    expect(pageContent().hasAttribute("aria-busy")).toBe(false);
    expect(
      screen.getByRole("heading", { level: 1, name: "List Transactions" }),
    ).toBeDefined();

    await loads.finishLatest();

    await screen.findByText("Navigation idle");
    expect(router.state.location.search).toBe("?account_id=00000000011");
    await act(async () => {
      chunk.resolve(BILL_PAYMENT_MODULE);
    });
    expect(pageDownloadsInFlight()).toBe(0);
  });
});
