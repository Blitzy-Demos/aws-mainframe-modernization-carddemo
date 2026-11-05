// Tests of the lazy page routes (routes/lazy-page.tsx) and what they show in
// place of the page (routes/page-fallbacks.tsx):
//
// - lazyPage imports a page once, and again after a failed import, and counts
//   each import as a download in flight until it settles; useDownloadingPage
//   re-renders its reader when a download starts and when it settles.
// - A page route's loader downloads the page only when the guards it sits
//   under will render it, and a change of the search string runs no loader.
// - The page renders once its chunk has arrived without committing its
//   Suspense fallback; the document load shows the skeleton in the shell.
// - A failed download shows PageLoadError inside the shell, the sidebar and
//   header in place, and "Try again" or a new navigation to the route loads
//   the page; outside the shell the panel is a main landmark of its own.
// - A guard's redirect shows the skeleton until the sign-on page has arrived.
//
// The routes render under the providers of main.tsx (without the Toaster),
// StrictMode included, as components/app-layout.test.tsx renders the shell.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode, type JSX } from "react";
import {
  createMemoryRouter,
  RouterProvider,
  type RouteObject,
} from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AppLayout } from "@/components/app-layout";
import { PageHeader, type RouteHandle } from "@/components/page-header";
import { ThemeProvider } from "@/components/theme-provider";
import { TooltipProvider } from "@/components/ui/tooltip";
import { clearSession, setSession, type Session } from "@/lib/session";
import { RequireAuth } from "@/routes/guards";
import {
  inShellErrorBoundary,
  lazyPage,
  pageDownloadsInFlight,
  pageRoute,
  subscribeToPageDownloads,
  useDownloadingPage,
  type PageAccess,
  type PageModule,
} from "@/routes/lazy-page";
import { PageLoadError } from "@/routes/page-fallbacks";

type Router = ReturnType<typeof createMemoryRouter>;
type User = ReturnType<typeof userEvent.setup>;

const ALERT_TEXT =
  "This page could not be loaded. Check your network connection, then try again.";

// The rejection of a chunk that fails to download, as the browser words it.
function chunkFailure(): TypeError {
  return new TypeError(
    "Failed to fetch dynamically imported module: /src/pages/admin/users/UserAddPage.tsx",
  );
}

function UserAddPage(): JSX.Element {
  return (
    <>
      <PageHeader />
      <p>User add form</p>
    </>
  );
}

function SignOnPage(): JSX.Element {
  return (
    <main>
      <PageHeader />
      <p>Sign-on form</p>
    </main>
  );
}

function HomePage(): JSX.Element {
  return <PageHeader />;
}

// Whether a page's chunk is downloading, as useDownloadingPage reports it.
function DownloadProbe(): JSX.Element {
  return <p>{useDownloadingPage() ? "Downloading" : "No download"}</p>;
}

const USER_ADD_MODULE: PageModule = { default: UserAddPage };
const SIGN_ON_MODULE: PageModule = { default: SignOnPage };

const SHELL_HANDLE = {
  title: "Main Menu",
  crumb: "Home",
  to: "/",
} satisfies RouteHandle;

const USER_ADD_HANDLE = {
  title: "Add User",
  crumb: "User Add (Security)",
} satisfies RouteHandle;

const SIGN_ON_HANDLE = {
  title: "Sign on",
  crumb: "Sign on",
} satisfies RouteHandle;

// A module import the test settles, as the browser settles a chunk download.
type PendingImport = {
  promise: Promise<PageModule>;
  resolve: (module: PageModule) => void;
  reject: (error: unknown) => void;
};

function pendingImport(): PendingImport {
  let resolve: (module: PageModule) => void = () => undefined;
  let reject: (error: unknown) => void = () => undefined;
  const promise = new Promise<PageModule>((onResolve, onReject) => {
    resolve = onResolve;
    reject = onReject;
  });
  return { promise, resolve, reject };
}

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

// The shell as App.tsx builds it, its children in the pathless route whose
// errorElement is PageLoadError: the main menu and the user add page.
function shellRoutes(load: () => Promise<PageModule>): RouteObject[] {
  return [
    {
      path: "/",
      element: <AppLayout />,
      handle: SHELL_HANDLE,
      children: inShellErrorBoundary([
        { index: true, element: <HomePage /> },
        {
          path: "admin/users/new",
          ...pageRoute(lazyPage(load), "signed-on"),
          handle: USER_ADD_HANDLE,
        },
      ]),
    },
  ];
}

function renderRoutes(
  routes: RouteObject[],
  initialPath: string,
): { router: Router; user: User } {
  const router = createMemoryRouter(routes, { initialEntries: [initialPath] });
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

// The shell's page content, its single main landmark.
function pageContent(): HTMLElement {
  return screen.getByRole("main");
}

function skeletonIn(container: HTMLElement): HTMLElement | null {
  return within(container).queryByRole("status", { name: "Loading" });
}

afterEach(() => {
  clearSession();
  document.documentElement.className = "";
  vi.restoreAllMocks();
});

describe("lazyPage", () => {
  it("imports the page once, however often it is preloaded", async () => {
    const load = vi.fn(() => Promise.resolve(USER_ADD_MODULE));
    const page = lazyPage(load);

    const first = page.preload();
    expect(page.preload()).toBe(first);
    await expect(first).resolves.toBe(USER_ADD_MODULE);
    expect(page.preload()).toBe(first);

    expect(load).toHaveBeenCalledOnce();
  });

  it("imports the page again after a failed import", async () => {
    const failure = chunkFailure();
    const load = vi
      .fn<() => Promise<PageModule>>()
      .mockRejectedValueOnce(failure)
      .mockResolvedValueOnce(USER_ADD_MODULE);
    const page = lazyPage(load);

    await expect(page.preload()).rejects.toBe(failure);
    const retry = page.preload();
    await expect(retry).resolves.toBe(USER_ADD_MODULE);

    expect(page.preload()).toBe(retry);
    expect(load).toHaveBeenCalledTimes(2);
  });

  it("counts each import it starts as a download in flight until the import succeeds or fails", async () => {
    const failure = chunkFailure();
    const imports = [pendingImport(), pendingImport(), pendingImport()];
    const load = vi
      .fn<() => Promise<PageModule>>()
      .mockImplementationOnce(() => imports[0].promise)
      .mockImplementationOnce(() => imports[1].promise);
    const page = lazyPage(load);
    const otherPage = lazyPage(() => imports[2].promise);
    const listener = vi.fn();
    const unsubscribe = subscribeToPageDownloads(listener);
    expect(pageDownloadsInFlight()).toBe(0);

    // A repeat preload returns the pending import and counts nothing more.
    const first = page.preload();
    page.preload();
    expect(pageDownloadsInFlight()).toBe(1);
    const other = otherPage.preload();
    expect(pageDownloadsInFlight()).toBe(2);

    imports[0].reject(failure);
    await expect(first).rejects.toBe(failure);
    expect(pageDownloadsInFlight()).toBe(1);

    // The import after the failure is a download of its own.
    const retry = page.preload();
    expect(pageDownloadsInFlight()).toBe(2);
    imports[1].resolve(USER_ADD_MODULE);
    await expect(retry).resolves.toBe(USER_ADD_MODULE);
    expect(pageDownloadsInFlight()).toBe(1);
    // A preload of the import that has settled counts nothing.
    expect(page.preload()).toBe(retry);
    expect(pageDownloadsInFlight()).toBe(1);

    imports[2].resolve(USER_ADD_MODULE);
    await expect(other).resolves.toBe(USER_ADD_MODULE);
    expect(pageDownloadsInFlight()).toBe(0);
    expect(load).toHaveBeenCalledTimes(2);
    // One notification per change: three imports started, three settled.
    await vi.waitFor(() => {
      expect(listener).toHaveBeenCalledTimes(6);
    });
    unsubscribe();
  });

  it("re-renders a reader of useDownloadingPage when a download starts and when it settles", async () => {
    const chunk = pendingImport();
    const page = lazyPage(() => chunk.promise);
    render(<DownloadProbe />);
    expect(screen.getByText("No download")).toBeDefined();

    void page.preload();

    await screen.findByText("Downloading");

    chunk.resolve(USER_ADD_MODULE);

    await screen.findByText("No download");
    expect(pageDownloadsInFlight()).toBe(0);
  });
});

describe("pageRoute's loader", () => {
  it.each<{ access: PageAccess; usrType: string | null; downloads: boolean }>([
    { access: "public", usrType: null, downloads: true },
    { access: "public", usrType: "U", downloads: true },
    { access: "signed-on", usrType: null, downloads: false },
    { access: "signed-on", usrType: "U", downloads: true },
    { access: "signed-on", usrType: "A", downloads: true },
    { access: "admin", usrType: null, downloads: false },
    { access: "admin", usrType: "U", downloads: false },
    { access: "admin", usrType: "A", downloads: true },
  ])(
    "downloads a $access page for user type $usrType: $downloads",
    async ({ access, usrType, downloads }) => {
      if (usrType !== null) {
        signOn(usrType);
      }
      const load = vi.fn(() => Promise.resolve(USER_ADD_MODULE));
      const router = createMemoryRouter(
        [{ id: "page", path: "/page", ...pageRoute(lazyPage(load), access) }],
        { initialEntries: ["/page"] },
      );

      await vi.waitFor(() => {
        expect(router.state.initialized).toBe(true);
      });

      expect(router.state.errors).toBeNull();
      expect(router.state.loaderData).toEqual({ page: null });
      expect(load).toHaveBeenCalledTimes(downloads ? 1 : 0);
      router.dispose();
    },
  );

  it("downloads no page for a session that has expired", async () => {
    setSession({
      token: "test-token",
      user_id: "USER0001",
      usr_type: "U",
      landing: "/",
      expires_at: new Date(Date.now() - 1_000).toISOString(),
    });
    const load = vi.fn(() => Promise.resolve(USER_ADD_MODULE));
    const router = createMemoryRouter(
      [{ path: "/page", ...pageRoute(lazyPage(load), "signed-on") }],
      { initialEntries: ["/page"] },
    );

    await vi.waitFor(() => {
      expect(router.state.initialized).toBe(true);
    });

    expect(load).not.toHaveBeenCalled();
    router.dispose();
  });

  it("runs again for neither a change of the search string nor a revalidation", async () => {
    signOn("U");
    const load = vi.fn(() => Promise.resolve(USER_ADD_MODULE));
    const router = createMemoryRouter(
      [{ path: "/page", ...pageRoute(lazyPage(load), "signed-on") }],
      { initialEntries: ["/page"] },
    );
    await vi.waitFor(() => {
      expect(router.state.initialized).toBe(true);
    });
    const states: string[] = [];
    const unsubscribe = router.subscribe((state) => {
      states.push(state.navigation.state);
    });

    await router.navigate("/page?account_id=00000000011");
    await router.revalidate();

    unsubscribe();
    expect(router.state.location.search).toBe("?account_id=00000000011");
    expect(states).not.toContain("loading");
    expect(router.state.revalidation).toBe("idle");
    router.dispose();
  });
});

describe("a lazy page route", () => {
  it("shows the skeleton in the shell on the document load until the page has arrived", async () => {
    signOn("U");
    const chunk = pendingImport();
    renderRoutes(
      shellRoutes(() => chunk.promise),
      "/admin/users/new",
    );

    await waitFor(() => {
      expect(skeletonIn(pageContent())).not.toBeNull();
    });
    expect(screen.getByRole("navigation", { name: "Main" })).toBeDefined();

    await act(async () => {
      chunk.resolve(USER_ADD_MODULE);
    });

    await screen.findByText("User add form");
    expect(skeletonIn(pageContent())).toBeNull();
  });

  it("renders the page without committing its Suspense fallback once the chunk has arrived", async () => {
    signOn("U");
    const chunk = pendingImport();
    const { router } = renderRoutes(
      shellRoutes(() => chunk.promise),
      "/",
    );
    await screen.findByRole("heading", { level: 1, name: SHELL_HANDLE.title });
    act(() => {
      void router.navigate("/admin/users/new");
    });
    await waitFor(() => {
      expect(skeletonIn(pageContent())).not.toBeNull();
    });
    // The skeleton shown is the shell's, for the router waits for the chunk,
    // so any skeleton added from here on is the page's Suspense fallback.
    expect(pageContent().getAttribute("aria-busy")).toBe("true");
    const added: Node[] = [];
    const observer = new MutationObserver((records) => {
      for (const record of records) {
        added.push(...record.addedNodes);
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });

    // Settled outside act, so React schedules the commit as in the browser,
    // where a task boundary lies between the chunk's arrival and the render.
    chunk.resolve(USER_ADD_MODULE);
    await screen.findByText("User add form");
    observer.disconnect();

    const skeletons = added.filter(
      (node) =>
        node instanceof Element &&
        (node.matches('[aria-label="Loading"]') ||
          node.querySelector('[aria-label="Loading"]') !== null),
    );
    expect(skeletons).toEqual([]);
  });
});

describe("PageLoadError", () => {
  it.each([
    {
      recovery: "Try again",
      recover: async (user: User): Promise<void> => {
        await user.click(screen.getByRole("button", { name: "Try again" }));
      },
    },
    {
      recovery: "Back and a new navigation to the route",
      recover: async (_user: User, router: Router): Promise<void> => {
        await act(() => router.navigate(-1));
        await screen.findByRole("heading", {
          level: 1,
          name: SHELL_HANDLE.title,
        });
        await act(() => router.navigate("/admin/users/new"));
      },
    },
  ])(
    "shows in the shell when the page's chunk fails, and $recovery loads the page",
    async ({ recover }) => {
      const consoleError = vi
        .spyOn(console, "error")
        .mockImplementation(() => undefined);
      signOn("U");
      const failure = chunkFailure();
      const load = vi
        .fn<() => Promise<PageModule>>()
        .mockRejectedValueOnce(failure)
        .mockResolvedValueOnce(USER_ADD_MODULE);
      const { router, user } = renderRoutes(shellRoutes(load), "/");
      await screen.findByRole("heading", {
        level: 1,
        name: SHELL_HANDLE.title,
      });

      await act(() => router.navigate("/admin/users/new"));

      const alert = await screen.findByRole("alert");
      expect(alert.textContent).toBe(ALERT_TEXT);
      // The panel is in the shell's page content, the sidebar and header in
      // place, and it is headed by the page that failed, crumbs included.
      expect(pageContent().contains(alert)).toBe(true);
      expect(screen.getByRole("navigation", { name: "Main" })).toBeDefined();
      expect(
        screen.getByRole("button", { name: "Toggle Sidebar" }),
      ).toBeDefined();
      expect(
        within(pageContent()).getByRole("heading", {
          level: 1,
          name: USER_ADD_HANDLE.title,
        }),
      ).toBeDefined();
      expect(
        within(pageContent()).getByRole("link", { name: SHELL_HANDLE.crumb }),
      ).toBeDefined();
      expect(screen.getByRole("button", { name: "Reload page" })).toBeDefined();
      expect(consoleError).toHaveBeenCalledWith(
        "The page could not be loaded.",
        failure,
      );

      await recover(user, router);

      await screen.findByText("User add form");
      expect(screen.queryByRole("alert")).toBeNull();
      expect(load).toHaveBeenCalledTimes(2);
    },
  );

  it("keeps Try again disabled while it retries, announces a failed retry again and moves focus to the page content on success", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    signOn("U");
    const retries = [pendingImport(), pendingImport()];
    const load = vi
      .fn<() => Promise<PageModule>>()
      .mockRejectedValueOnce(chunkFailure())
      .mockImplementationOnce(() => retries[0].promise)
      .mockImplementationOnce(() => retries[1].promise);
    const { router, user } = renderRoutes(shellRoutes(load), "/");
    await screen.findByRole("heading", { level: 1, name: SHELL_HANDLE.title });
    await act(() => router.navigate("/admin/users/new"));
    const firstAlert = await screen.findByRole("alert");
    const tryAgain = screen.getByRole("button", { name: "Try again" });

    await user.click(tryAgain);

    await waitFor(() => {
      expect(tryAgain.hasAttribute("disabled")).toBe(true);
    });
    await act(async () => {
      retries[0].reject(chunkFailure());
    });
    await waitFor(() => {
      expect(tryAgain.hasAttribute("disabled")).toBe(false);
    });
    const secondAlert = screen.getByRole("alert");
    expect(secondAlert).not.toBe(firstAlert);
    expect(secondAlert.textContent).toBe(ALERT_TEXT);
    expect(document.activeElement).toBe(tryAgain);

    await user.click(tryAgain);
    await act(async () => {
      retries[1].resolve(USER_ADD_MODULE);
    });

    await screen.findByText("User add form");
    expect(document.activeElement).toBe(pageContent());
    expect(load).toHaveBeenCalledTimes(3);
  });

  it("is a page with its own main landmark outside the shell, and Try again loads the page", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const load = vi
      .fn<() => Promise<PageModule>>()
      .mockRejectedValueOnce(chunkFailure())
      .mockResolvedValueOnce(SIGN_ON_MODULE);
    const { user } = renderRoutes(
      [
        {
          path: "/login",
          ...pageRoute(lazyPage(load), "public"),
          errorElement: <PageLoadError standalone />,
          handle: SIGN_ON_HANDLE,
        },
      ],
      "/login",
    );

    const alert = await screen.findByRole("alert");
    const main = screen.getByRole("main");
    expect(main.contains(alert)).toBe(true);
    expect(
      within(main).getByRole("heading", {
        level: 1,
        name: SIGN_ON_HANDLE.title,
      }),
    ).toBeDefined();

    await user.click(screen.getByRole("button", { name: "Try again" }));

    await screen.findByText("Sign-on form");
    expect(screen.getAllByRole("main")).toHaveLength(1);
    expect(load).toHaveBeenCalledTimes(2);
  });
});

describe("a guard's redirect", () => {
  it("shows the skeleton until the sign-on page has arrived, and downloads no guarded page", async () => {
    const signOnChunk = pendingImport();
    const guardedLoad = vi.fn(() => Promise.resolve(USER_ADD_MODULE));
    const { router } = renderRoutes(
      [
        {
          path: "/login",
          ...pageRoute(
            lazyPage(() => signOnChunk.promise),
            "public",
          ),
          handle: SIGN_ON_HANDLE,
        },
        {
          element: <RequireAuth />,
          children: [
            {
              path: "/transactions/new",
              ...pageRoute(lazyPage(guardedLoad), "signed-on"),
            },
          ],
        },
      ],
      "/transactions/new",
    );

    await waitFor(() => {
      expect(router.state.navigation.location?.pathname).toBe("/login");
    });
    expect(skeletonIn(document.body)).not.toBeNull();

    await act(async () => {
      signOnChunk.resolve(SIGN_ON_MODULE);
    });

    await screen.findByText("Sign-on form");
    expect(skeletonIn(document.body)).toBeNull();
    expect(router.state.location.pathname).toBe("/login");
    expect(guardedLoad).not.toHaveBeenCalled();
  });
});
