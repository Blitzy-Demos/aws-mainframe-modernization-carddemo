// Route-level code splitting (AAP 0.10.3): every page is a React.lazy
// component of a chunk of its own, and its route's loader downloads that
// chunk before the router commits the navigation.
//
// The router applies each navigation in a transition, so until the chunk has
// arrived the screen that is being left stays in place. lazyPage counts the
// chunk downloads in flight (useDownloadingPage), and the app shell shows its
// skeleton in that screen's place while one is (components/app-layout.tsx). A
// navigation to a page whose chunk has already arrived, a Back to a visited
// page among them, downloads nothing and shows none: its loader settles at
// once. The page then renders as soon as the chunk is evaluated: React.lazy
// reads the import that has already settled, and no Suspense fallback is
// committed, so React does not hold the page back behind one, and the page's
// data requests start when it mounts. The loaders fetch no data; the pages'
// TanStack Query hooks do.
//
// A failed download rejects the route's loader, and the route's errorElement
// (routes/page-fallbacks.tsx) shows in place of the page. The failed import
// is forgotten, so "Try again", or the next navigation to the route, imports
// the chunk again.
import {
  lazy,
  Suspense,
  useSyncExternalStore,
  type ComponentType,
  type LazyExoticComponent,
} from "react";
import type { RouteObject } from "react-router";

import { getSession, isAdmin } from "@/lib/session";
import { PageFallback, PageLoadError } from "@/routes/page-fallbacks";

/** A page module: its default export is the page component. */
export type PageModule = { default: ComponentType };

/** A page loaded on demand: its lazy component and its chunk's download. */
export type LazyPage = {
  /** The React.lazy component that renders the page. */
  readonly Component: LazyExoticComponent<ComponentType>;
  /**
   * Downloads the page's chunk, once: every call returns the same import
   * until it fails, and the call after a failure imports again.
   */
  readonly preload: () => Promise<PageModule>;
};

// The page chunks downloading now: the imports every lazyPage has started
// that have not settled yet, and the listeners told of each change of their
// number.
let downloadsInFlight = 0;
const downloadListeners = new Set<() => void>();

// Counts an import as started (+1) or settled (-1). The listeners hear of the
// change in a microtask, for React.lazy starts an import while it renders,
// and a listener that schedules a render must not run then.
function countDownload(change: 1 | -1): void {
  downloadsInFlight += change;
  queueMicrotask(() => {
    for (const listener of downloadListeners) {
      listener();
    }
  });
}

/**
 * Subscribes `listener` to the page-chunk downloads: it runs after every
 * change of their number, once per import started and once per import
 * settled. Returns the function that unsubscribes it.
 */
export function subscribeToPageDownloads(listener: () => void): () => void {
  downloadListeners.add(listener);
  return () => {
    downloadListeners.delete(listener);
  };
}

/** The number of page-chunk imports started and not yet settled. */
export function pageDownloadsInFlight(): number {
  return downloadsInFlight;
}

function isDownloadingPage(): boolean {
  return downloadsInFlight > 0;
}

/**
 * Whether a page's chunk is downloading: an import some lazyPage started has
 * not settled. The component re-renders when a download starts while none
 * was in flight, and when the last one in flight settles.
 */
export function useDownloadingPage(): boolean {
  return useSyncExternalStore(subscribeToPageDownloads, isDownloadingPage);
}

/**
 * The page module `load` imports. `load` is a literal `import("@/pages/…")`,
 * so Vite emits the page as a chunk of its own; the lazy component and the
 * route loader share its one memoized import, which counts as a download in
 * flight from its start until it settles, whether it succeeds or fails.
 */
export function lazyPage(load: () => Promise<PageModule>): LazyPage {
  let pending: Promise<PageModule> | null = null;
  const preload = (): Promise<PageModule> => {
    if (pending === null) {
      const download = load();
      countDownload(1);
      pending = download.then(
        (module) => {
          countDownload(-1);
          return module;
        },
        (error: unknown) => {
          pending = null;
          countDownload(-1);
          throw error;
        },
      );
    }
    return pending;
  };
  return { Component: lazy(preload), preload };
}

/**
 * The guard a page route sits under in App.tsx: none for "public" (/login),
 * RequireAuth for "signed-on", and RequireAuth with RequireAdmin for "admin".
 */
export type PageAccess = "public" | "signed-on" | "admin";

// Whether the guards above a page route will render its page for the session
// the tab holds now, the checks RequireAuth and RequireAdmin make: a live
// session for every guarded page, an administrator's for the admin pages.
function guardsAdmit(access: PageAccess): boolean {
  switch (access) {
    case "public":
      return true;
    case "signed-on":
      return getSession() !== null;
    case "admin":
      return isAdmin(getSession());
  }
}

/** The properties a page route takes from pageRoute. */
export type PageRoute = Pick<
  RouteObject,
  "loader" | "shouldRevalidate" | "hydrateFallbackElement" | "element"
>;

/**
 * The properties of the route that renders `page`, spread into its route
 * object; `access` names the guards the route sits under.
 *
 * - loader: downloads the page's chunk and resolves to null. Loaders run
 *   before any route element renders, the guards included, so a page its
 *   guards will redirect away from is not downloaded: none of the signed-on
 *   pages without a live session (RequireAuth sends the visitor to /login),
 *   and no admin page without an administrator's session (RequireAdmin sends
 *   the user to the main menu). /login is always downloaded.
 * - shouldRevalidate: never, because the loader loads code and no data. A
 *   change of the search string or the parameters, and a revalidation, leave
 *   the downloaded page as it is. A loader that failed holds no data, so the
 *   router runs it again on a retry and on the next navigation to the route,
 *   whatever this returns.
 * - hydrateFallbackElement: the skeleton the document load shows in the
 *   page's place, the app shell around it, until the loader has run.
 * - element: the lazy page. Its Suspense fallback shows only if the page
 *   renders before its chunk has arrived, which happens only when a session
 *   starts between the loader and the render.
 */
export function pageRoute(
  { Component, preload }: LazyPage,
  access: PageAccess,
): PageRoute {
  return {
    loader: async (): Promise<null> => {
      if (guardsAdmit(access)) {
        await preload();
      }
      return null;
    },
    shouldRevalidate: () => false,
    hydrateFallbackElement: <PageFallback />,
    element: (
      <Suspense fallback={<PageFallback />}>
        <Component />
      </Suspense>
    ),
  };
}

/**
 * The pathless route that wraps the app shell's child routes, the index route
 * included, and catches what they throw: a page whose chunk fails to download,
 * or one that fails to render, shows PageLoadError in the shell's page content,
 * with the sidebar and header in place.
 */
export function inShellErrorBoundary(children: RouteObject[]): RouteObject[] {
  return [{ errorElement: <PageLoadError />, children }];
}
