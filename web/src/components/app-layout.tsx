// The authenticated app shell, the element of the route under RequireAuth:
// the sidebar with the CM00 and CA00 menus (AppSidebar), a header with the
// sidebar trigger, the theme switch and the user menu, and the page content,
// which the matched child route renders through <Outlet />.
//
// - Breakpoints (AAP 0.5.4.4): from 1024px (Tailwind `lg`) the sidebar is
//   expanded, from 768px to 1023px it is the icon rail. The open state follows
//   that media query and stays in React state only; the trigger and Ctrl/Cmd+B
//   (the generated SidebarProvider's shortcut) still toggle it in between.
//   Below 768px the generated sidebar switches to its Sheet on its own.
// - Keyboard path, in DOM order: the "Skip to content" link, the sidebar
//   links, the header controls, then the page content. A route change other
//   than the document load moves focus to the page content, unless the new
//   page has already focused one of its own controls (an autofocused filter,
//   a form's first field, the field an error names), and a visually hidden
//   status region announces the new page's title. A change of the search
//   string alone does neither.
// - While the router downloads the next page's chunk (routes/lazy-page.tsx),
//   the page content shows the loading skeleton and is marked busy, so a
//   navigation never leaves the previous page standing without a cue. The
//   skeleton shows once a page's chunk is downloading while the router loads
//   another pathname, and stays until the router commits the next page or
//   stops loading another pathname. A navigation to a page whose chunk has
//   arrived, a Back to a visited page among them, shows none and keeps the
//   previous page until the next one renders. A change of the search string
//   alone, an update within the page, shows none either.
// - The user menu replaces PF3 on COMEN01 and COADM01 ('F3=Exit'
//   [app/bms/COMEN01.bms:L162] [app/bms/COADM01.bms:L162]), which returns to
//   the sign-on screen [app/cbl/COMEN01C.cbl:L96-98], and the sign-on screen's
//   own PF3, which ends with CCDA-MSG-THANK-YOU [app/cbl/COSGN00C.cbl:L88-90]
//   [app/cpy/CSMSG01Y.cpy:L18-19]. Its item is the function map's menuitem
//   "Log out" (F-COMEN01-012, F-COADM01-007, F-COSGN00-002), and its trigger
//   is named by the signed-on user ID. The server keeps no session, so the
//   client discards its own whether or not the logout call succeeds.
//
// The shell decides no access: AppSidebar picks the menu sections, and the
// server's 401 and 403 are authoritative.
import {
  useEffect,
  useRef,
  useState,
  type JSX,
  type MouseEvent,
  type RefObject,
} from "react";
import { CircleUserRound, LogOut } from "lucide-react";
import {
  NavigationType,
  Outlet,
  useLocation,
  useNavigate,
  useNavigation,
  useNavigationType,
} from "react-router";

import { useLogoutMutation } from "@/api/queries/auth";
import { AppSidebar } from "@/components/app-sidebar";
import { ModeToggle } from "@/components/mode-toggle";
import { usePageTitle } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Separator } from "@/components/ui/separator";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar";
import { showApiError, showSuccess } from "@/lib/api-errors";
import { clearSession, isAdmin, useSession, type Session } from "@/lib/session";
import { useDownloadingPage } from "@/routes/lazy-page";
import { PageFallback } from "@/routes/page-fallbacks";

// The id of the page content, the target of the skip link.
const MAIN_CONTENT_ID = "main-content";

// Tailwind 4's default `lg` breakpoint (64rem at the 16px root size): the
// width from which the sidebar starts expanded.
const EXPANDED_SIDEBAR_QUERY = "(min-width: 1024px)";

// The sidebar's open state: expanded at `lg` and wider, the icon rail below,
// re-applied whenever the viewport crosses the breakpoint. A toggle in
// between holds until the next crossing.
function useExpandedSidebarState(): [boolean, (open: boolean) => void] {
  const [open, setOpen] = useState(
    () => window.matchMedia(EXPANDED_SIDEBAR_QUERY).matches,
  );

  useEffect(() => {
    const query = window.matchMedia(EXPANDED_SIDEBAR_QUERY);
    const follow = (event: MediaQueryListEvent): void => {
      setOpen(event.matches);
    };
    query.addEventListener("change", follow);
    return () => {
      query.removeEventListener("change", follow);
    };
  }, []);

  return [open, setOpen];
}

// Whether the page content shows the skeleton for another screen the router
// is loading. A navigation to a different pathname waits for its page's
// chunk before it commits, and until then the location is still the current
// page's. The skeleton shows once a page's chunk is downloading
// (useDownloadingPage) while the router loads another pathname, and stays
// until the router no longer does: the download settles a moment before the
// router commits the next page, and the current page must not mount again in
// between. A navigation to a page whose chunk has arrived, a Back to a
// visited page among them, downloads nothing, so the current page stays until
// the next one renders. A navigation that changes the search string alone
// runs no page loader, and its pathname is the current one.
function useLoadingAnotherPage(): boolean {
  const navigation = useNavigation();
  const { pathname } = useLocation();
  const downloading = useDownloadingPage();
  const [shown, setShown] = useState(false);
  const loadingAnotherPathname =
    navigation.state === "loading" && navigation.location.pathname !== pathname;
  const show = loadingAnotherPathname && (downloading || shown);
  // `shown` remembers whether the skeleton showed in the last render, so it
  // holds once shown; React renders the component again at once on a change.
  if (show !== shown) {
    setShown(show);
  }
  return show;
}

// The page content: the matched page, or the loading skeleton, marked busy,
// while the router loads another page whose chunk downloads. Only this part
// of the shell reads the router's navigation state and the page downloads, so
// AppLayout, and the sidebar with it, does not re-render when a download
// starts, and the next page renders as soon as its chunk has arrived.
function PageContent({
  mainRef,
}: {
  mainRef: RefObject<HTMLElement | null>;
}): JSX.Element {
  const loadingAnotherPage = useLoadingAnotherPage();
  return (
    <SidebarInset
      ref={mainRef}
      id={MAIN_CONTENT_ID}
      tabIndex={-1}
      aria-busy={loadingAnotherPage || undefined}
      className="gap-4 p-4 outline-none lg:p-6"
    >
      {loadingAnotherPage ? <PageFallback /> : <Outlet />}
    </SidebarInset>
  );
}

// The user type as the COUSR01 screen spells its codes, 'User Type: ' with
// '(A=Admin, U=User)' [app/bms/COUSR01.bms:L140] [app/bms/COUSR01.bms:L150].
// Every code other than A is a regular user, as isAdmin reads it.
function userTypeText(session: Session): string {
  return `User Type: ${isAdmin(session) ? "Admin" : "User"}`;
}

// The header's user menu. Its trigger's accessible name is the user ID, the
// text it shows beside the decorative icon.
function UserMenu({ session }: { session: Session }): JSX.Element {
  const logout = useLogoutMutation();
  const navigate = useNavigate();

  // mutateAsync rather than mutate with per-call callbacks: the hook's own
  // onSettled clears the session, RequireAuth then unmounts the shell, and
  // TanStack Query drops per-call callbacks of an unmounted observer, which
  // would lose the toast. The awaited result outlives the unmount, and the
  // Toaster sits outside the router, so the toast survives the navigation.
  // The message is shown exactly as the server returns it.
  const logOut = async (): Promise<void> => {
    try {
      const response = await logout.mutateAsync();
      showSuccess(response.message);
    } catch (error) {
      showApiError(error);
    } finally {
      clearSession();
      await navigate("/login", { replace: true });
    }
  };

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost">
          <CircleUserRound aria-hidden="true" />
          <span>{session.user_id}</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel className="flex flex-col gap-1">
          <span>{session.user_id}</span>
          <span className="text-xs font-normal text-muted-foreground">
            {userTypeText(session)}
          </span>
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          disabled={logout.isPending}
          onSelect={() => {
            void logOut();
          }}
        >
          <LogOut aria-hidden="true" />
          Log out
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// The latest route-change announcement: the destination's title, and the
// count of route changes so far, which keys the node that holds the title.
type RouteAnnouncement = { id: number; title: string };

// The shell's response to a route change, a change of the location's
// pathname. A change of the search string alone is an update within the page
// (AuthorizationSummaryPage writes its account filter there) and gets none.
//
// - Focus moves to the page content, so the keyboard continues from the top
//   of the new page instead of the sidebar link that was activated, or the
//   body when the activated link left with the old page (a crumb, a row
//   link). Focus the new page has placed inside the content stands: React
//   applies autoFocus while it commits, and runs the effects of the earlier
//   siblings' trees, the page's mount-time setFocus among them, before this
//   component's, which is rendered after the page content. preventScroll
//   keeps the scroll position as it was.
// - The status region announces the destination's title, the heading
//   PageHeader shows (usePageTitle), also when the new page keeps its own
//   focus. Each announcement is a new node, so two destinations with the same
//   title (the two TranTypeFormPage routes, consecutive
//   AuthorizationDetailPage records opened with Next) are each announced.
//
// The document load is no route change: focus stays on the body, so the
// first Tab reaches the skip link, and the region stays empty. A shell first
// rendered by an in-app navigation, the landing after sign-on, is one. The
// handled pathname also makes StrictMode's second run of the effect a no-op.
function RouteChangeAnnouncer({
  mainRef,
}: {
  mainRef: RefObject<HTMLElement | null>;
}): JSX.Element {
  const { pathname } = useLocation();
  const navigationType = useNavigationType();
  const title = usePageTitle();
  const handledPathname = useRef<string | null>(null);
  const [announcement, setAnnouncement] = useState<RouteAnnouncement | null>(
    null,
  );

  useEffect(() => {
    const previous = handledPathname.current;
    if (previous === pathname) {
      return;
    }
    handledPathname.current = pathname;
    if (previous === null && navigationType === NavigationType.Pop) {
      return;
    }
    const main = mainRef.current;
    const active = document.activeElement;
    if (
      main !== null &&
      (active === null || active === main || !main.contains(active))
    ) {
      main.focus({ preventScroll: true });
    }
    setAnnouncement((current) => ({
      id: (current?.id ?? 0) + 1,
      title: title ?? "",
    }));
  }, [mainRef, navigationType, pathname, title]);

  // Always rendered, outside the page content and never focusable, so
  // assistive technology tracks the region from the shell's first render and
  // the Tab order and the single main landmark are unchanged.
  return (
    <p role="status" className="sr-only">
      {announcement !== null && announcement.title !== "" ? (
        <span key={announcement.id}>{announcement.title}</span>
      ) : null}
    </p>
  );
}

export function AppLayout(): JSX.Element {
  const [open, setOpen] = useExpandedSidebarState();
  const session = useSession();
  const mainRef = useRef<HTMLElement>(null);

  // Focus moves to the content without changing the URL hash, which the
  // router would otherwise take as a navigation.
  const skipToContent = (event: MouseEvent<HTMLAnchorElement>): void => {
    event.preventDefault();
    mainRef.current?.focus();
  };

  return (
    <>
      <a
        href={`#${MAIN_CONTENT_ID}`}
        onClick={skipToContent}
        className="sr-only focus:not-sr-only focus:fixed focus:inset-s-4 focus:inset-bs-4 focus:z-50 focus:rounded-md focus:bg-background focus:px-4 focus:py-2 focus:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
      >
        Skip to content
      </a>
      <SidebarProvider open={open} onOpenChange={setOpen}>
        <AppSidebar />
        {/* The generated SidebarInset is a <main>, so it is the page content
            itself and the header sits beside it in this column rather than
            inside it: a second <main> nested in SidebarInset would break the
            one-main-landmark rule of semantic HTML, and the generated
            sidebar.tsx keeps only its two sanctioned edits. The header is
            then the banner landmark and stays outside #main-content, ahead
            of it in DOM order. min-w-0 lets wide tables scroll inside their
            card instead of widening the column. */}
        <div className="flex min-w-0 flex-1 flex-col">
          <header className="flex h-14 shrink-0 items-center gap-2 border-be px-4">
            <SidebarTrigger />
            <Separator
              orientation="vertical"
              className="data-[orientation=vertical]:h-4"
            />
            <div className="ms-auto flex items-center gap-2">
              <ModeToggle />
              {session !== null && <UserMenu session={session} />}
            </div>
          </header>
          <PageContent mainRef={mainRef} />
        </div>
      </SidebarProvider>
      <RouteChangeAnnouncer mainRef={mainRef} />
    </>
  );
}
