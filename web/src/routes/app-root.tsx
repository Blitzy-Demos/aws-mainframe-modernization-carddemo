// The element of the root route, the pathless parent of every route App.tsx
// declares, /login included. It keeps the two parts of the document that
// follow each route but that no page owns.
//
// - The document title (WCAG 2.4.2 Page Titled): the title the matched routes
//   give the page, the heading PageHeader shows and the app shell announces
//   (usePageTitle), then the application name, as in "List Transactions –
//   CardDemo". Only the routes the guards let the session reach count, so a
//   screen a guard redirects away from never titles the document: a regular
//   user on /admin/** sees the main menu's title, and a visitor without a
//   session the application name until /login. Without a titled route the
//   title is the application name alone, index.html's own. A route that
//   shows its errorElement stays matched and keeps its title.
// - The window's scroll position, kept per history entry (location.key) in
//   this document's memory only, so no Web Storage holds it: sessionStorage
//   keeps the session and the theme alone (AAP 0.4.1).
//   - A push or replace to another pathname opens that screen at its top, a
//     push back to a screen shown before included.
//   - A change of the search string alone keeps the position: it is an update
//     within the page, the rule the shell's route-change handling follows
//     (components/app-layout.tsx). AuthorizationSummaryPage writes its
//     account filter there.
//   - Back and Forward return to the position the history entry was left at,
//     or to the top for an entry this document has not shown.
//   - The document load leaves the position as the browser placed it.
//   The position is recorded from the window's scroll events, only while the
//   router's navigation is idle. While the router loads another page, the
//   shell shows its skeleton in the page content, which shortens the document
//   and clamps the window's position; recording nothing then leaves the
//   entry being left with the position its page had. The recording flag and
//   the entry follow each commit in layout effects, before the browser
//   dispatches the scroll events of that commit's layout changes.
//   history.scrollRestoration is "manual" while the root is mounted, so the
//   browser restores no same-document entry over these positions, and "auto"
//   from pagehide to the next pageshow, so a reload, or a page the
//   back/forward cache restores, gets the browser's own restoration. The
//   shell's focus move to the page content passes preventScroll, so it keeps
//   the position this sets.
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type JSX,
} from "react";
import {
  NavigationType,
  Outlet,
  useLocation,
  useNavigation,
  useNavigationType,
} from "react-router";

import { usePageTitle } from "@/components/page-header";

// The application name: index.html's title and the suffix of every page's.
const APP_NAME = "CardDemo";

// The document title of a page whose matched routes title it `pageTitle`.
function documentTitle(pageTitle: string | undefined): string {
  return pageTitle === undefined || pageTitle === ""
    ? APP_NAME
    : `${pageTitle} – ${APP_NAME}`;
}

// The history entry the window's position belongs to.
type ShownEntry = { key: string; pathname: string };

// Keeps history.scrollRestoration "manual" while mounted and "auto" from a
// pagehide to the next pageshow, and puts back the value it found on unmount.
function useManualScrollRestoration(): void {
  useEffect(() => {
    const { history } = window;
    const found = history.scrollRestoration;
    const manual = (): void => {
      history.scrollRestoration = "manual";
    };
    const auto = (): void => {
      history.scrollRestoration = "auto";
    };
    manual();
    window.addEventListener("pagehide", auto);
    window.addEventListener("pageshow", manual);
    return () => {
      window.removeEventListener("pagehide", auto);
      window.removeEventListener("pageshow", manual);
      history.scrollRestoration = found;
    };
  }, []);
}

// Records the window's position for the shown history entry, and sets it on
// each navigation, as the header describes. It renders nothing, so only it
// re-renders when the router's navigation state changes.
function ScrollPositions(): null {
  const location = useLocation();
  const navigationType = useNavigationType();
  const idle = useNavigation().state === "idle";
  // The window's position per location key, for as long as the document.
  const [positions] = useState(() => new Map<string, number>());
  const shown = useRef<ShownEntry | null>(null);
  const recording = useRef(false);
  useManualScrollRestoration();

  useEffect(() => {
    const record = (): void => {
      if (recording.current && shown.current !== null) {
        positions.set(shown.current.key, window.scrollY);
      }
    };
    window.addEventListener("scroll", record, { passive: true });
    return () => {
      window.removeEventListener("scroll", record);
    };
  }, [positions]);

  // Declared before the navigation's effect, so a commit that ends the
  // navigation records the position that effect sets.
  useLayoutEffect(() => {
    recording.current = idle;
  }, [idle]);

  useLayoutEffect(() => {
    const left = shown.current;
    // StrictMode's second run of the effect, or a render of the same entry.
    if (left?.key === location.key) {
      return;
    }
    shown.current = { key: location.key, pathname: location.pathname };
    // left is null on the document load, which leaves the position alone.
    if (left !== null) {
      if (navigationType === NavigationType.Pop) {
        window.scrollTo(0, positions.get(location.key) ?? 0);
      } else if (location.pathname !== left.pathname) {
        window.scrollTo(0, 0);
      }
    }
    // A kept or unchanged position fires no scroll event, so the entry
    // records it here.
    if (recording.current) {
      positions.set(location.key, window.scrollY);
    }
  }, [location.key, location.pathname, navigationType, positions]);

  return null;
}

/**
 * The root route's element: sets the document title from the matched routes
 * the guards admit, restores or resets the window's scroll position on each
 * navigation, and renders the matched child route.
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
      <ScrollPositions />
      <Outlet />
    </>
  );
}
