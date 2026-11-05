// The route guards: the application's one sign-on check and one role check for
// routing. App.tsx wraps the signed-on routes in RequireAuth and the /admin
// routes in RequireAdmin; no page or layout repeats either check.
//
// Both guards are advisory. They keep the screens consistent with the session
// the tab holds, while the server decides access: a missing or expired token
// is answered 401 'Authentication required' and a regular user's token on
// /admin/** 403 'Admin access required', and api/client.ts clears the session
// on a 401 to a request that session sent, which brings RequireAuth back to
// the sign-on screen. That redirect unmounts the page and cancels its
// queries, so the guard shows the server's 401 message, once, as it sends the
// user to sign on.
//
// The landing after sign-on is not chosen here: LoginPage navigates to the
// login response's `landing`, the counterpart of COSGN00C's XCTL split.
//
// Each redirect is a Redirect, which shows the page skeleton until the router
// has downloaded the destination's page and commits it.
//
// Both checks are lib/session.ts's sessionAdmits, the rule the page loaders
// and the page title apply as well. App.tsx places each guard through
// guardRoute, which also marks the route's handle with the access the guard
// requires, so the page title (usePageTitle) never names a screen the guard
// sends the user away from.
import { useEffect, type ReactNode } from "react";
import { Outlet, useLocation, type RouteObject } from "react-router";
import { toast } from "sonner";

import type { AccessHandle, RouteHandle } from "@/components/page-header";
import { showApiError } from "@/lib/api-errors";
import {
  sessionAdmits,
  takeSessionEndCause,
  useSession,
  type RouteAccess,
} from "@/lib/session";
import { Redirect } from "@/routes/page-fallbacks";

// Byte-identical to ADMIN_ACCESS_REQUIRED in crates/domain/src/messages.rs, the
// `message` of the server's 403. It takes the place of COMEN01C's menu text
// 'No access - Admin Only option... ' [app/cbl/COMEN01C.cbl:L136-143].
const ADMIN_ACCESS_REQUIRED = "Admin access required";

// One id for every admin-gate toast: sonner updates a toast whose id is still
// shown instead of adding another, so StrictMode's second effect run and
// repeated redirects show a single notification.
const ADMIN_TOAST_ID = "admin-access-required";

// Once the session has ended, shows what ended it, when that was a server
// answer (the API client's 401), through the application's error display.
// The cause is taken, so it is shown once: by whichever guard sees the end
// first, and not again on StrictMode's second effect run. A sign-off or an
// expiry records no cause and shows nothing.
function useSessionEndNotice(live: boolean): void {
  useEffect(() => {
    if (!live) {
      const cause = takeSessionEndCause();
      if (cause !== undefined) {
        showApiError(cause);
      }
    }
  }, [live]);
}

type GuardProps = {
  // The guarded content; omitted when the guard is a layout route.
  children?: ReactNode;
};

/**
 * Requires a live session, as every CardDemo transaction is reached only
 * through COSGN00C sign-on. Without one it redirects to /login, carrying the
 * requested location in the navigation state as `from`. When the server's 401
 * ended the session, its message ('Authentication required') is shown once.
 *
 * Renders `children` when given, otherwise the matched child route. The
 * server's 401 'Authentication required' is authoritative.
 */
export function RequireAuth({ children }: GuardProps): ReactNode {
  const session = useSession();
  const location = useLocation();
  // useSession already withholds an expired session; sessionAdmits checks the
  // expiry as well, so a redirect never depends on when the store last looked.
  const live = sessionAdmits(session, "signed-on");
  useSessionEndNotice(live);

  if (!live) {
    return <Redirect to="/login" replace state={{ from: location }} />;
  }
  return children ?? <Outlet />;
}

/**
 * Requires an administrator's session: CDEMO-USER-TYPE 'A', the 88-level
 * CDEMO-USRTYP-ADMIN [app/cpy/COCOM01Y.cpy:L26-28]. It replaces COMEN01C's
 * refusal of an admin-only option to a regular user
 * [app/cbl/COMEN01C.cbl:L136-143]: a regular user is sent to the main menu at
 * / with an 'Admin access required' toast.
 *
 * Without a live session it redirects to /login and shows no admin toast,
 * the server's order as well (no token is a 401, not a 403), so the guard also
 * holds outside RequireAuth; as there, the message of a 401 that ended the
 * session is shown once. Renders `children` when given, otherwise the matched
 * child route. The server's 401 and 403 are authoritative.
 */
export function RequireAdmin({ children }: GuardProps): ReactNode {
  const session = useSession();
  const location = useLocation();
  const live = sessionAdmits(session, "signed-on");
  const admin = sessionAdmits(session, "admin");
  useSessionEndNotice(live);

  // Toasting is a side effect, so it runs after the commit, never in render.
  useEffect(() => {
    if (live && !admin) {
      toast.error(ADMIN_ACCESS_REQUIRED, { id: ADMIN_TOAST_ID });
    }
  }, [live, admin]);

  if (!live) {
    return <Redirect to="/login" replace state={{ from: location }} />;
  }
  if (!admin) {
    return <Redirect to="/" replace />;
  }
  return children ?? <Outlet />;
}

/** The access a guard requires: RequireAuth's, or RequireAdmin's. */
export type GuardedAccess = Exclude<RouteAccess, "public">;

/** The properties a guard route takes from guardRoute. */
export type GuardRoute = Pick<RouteObject, "element" | "handle">;

/**
 * The properties of the layout route that `access` guards, spread into its
 * route object, so the guard and the record of what it requires are declared
 * once and together:
 *
 * - element: RequireAuth for "signed-on", RequireAdmin for "admin".
 * - handle: `handle`, the route's own crumb and title when it has them, marked
 *   with `access` (AccessHandle). usePageTitle stops at the first matched
 *   route whose access the session lacks, so the document title and the
 *   shell's announcement never name the screen the guard redirects away from.
 */
export function guardRoute(
  access: GuardedAccess,
  handle?: RouteHandle,
): GuardRoute {
  return {
    element: access === "admin" ? <RequireAdmin /> : <RequireAuth />,
    handle: { ...handle, access } satisfies AccessHandle,
  };
}
