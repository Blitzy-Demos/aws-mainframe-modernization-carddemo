// The element of the root route, the pathless parent of every route App.tsx
// declares, /login included. It keeps the two parts of the document that
// follow each route but that no page owns.
//
// - The document title (WCAG 2.4.2 Page Titled): the title the matched routes
//   give the page, the heading PageHeader shows and the app shell announces
//   (usePageTitle), then the application name, as in "List Transactions –
//   CardDemo". Without a titled route the title is the application name
//   alone, index.html's own. A route that shows its errorElement stays matched
//   and keeps its title.
// - The scroll position, through react-router's ScrollRestoration: a push or
//   replace to another screen opens it at its top, and Back and Forward return
//   to the position the history entry was left at. The positions are keyed by
//   history entry and kept in sessionStorage, so they last for the tab's
//   session only. A navigation flagged preventScrollReset keeps the position:
//   AuthorizationSummaryPage writes its account filter to the search string
//   that way, the update within the page that the shell's route-change
//   handling also leaves alone. The shell's focus move to the page content
//   (components/app-layout.tsx) passes preventScroll, so it keeps the
//   position this sets.
import { useEffect, type JSX } from "react";
import { Outlet, ScrollRestoration } from "react-router";

import { usePageTitle } from "@/components/page-header";

// The application name: index.html's title and the suffix of every page's.
const APP_NAME = "CardDemo";

// The document title of a page whose matched routes title it `pageTitle`.
function documentTitle(pageTitle: string | undefined): string {
  return pageTitle === undefined || pageTitle === ""
    ? APP_NAME
    : `${pageTitle} – ${APP_NAME}`;
}

/**
 * The root route's element: sets the document title from the matched routes,
 * restores or resets the window's scroll position on each navigation, and
 * renders the matched child route.
 */
export function AppRoot(): JSX.Element {
  const title = documentTitle(usePageTitle());

  // A side effect on the document, so it runs after the commit. It is not a
  // rendered <title> element: React would add that to the head after
  // index.html's own, and the document's title is the first one.
  useEffect(() => {
    document.title = title;
  }, [title]);

  return (
    <>
      <ScrollRestoration />
      <Outlet />
    </>
  );
}
