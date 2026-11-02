// The route guards: the application's one sign-on check and one role check for
// routing. App.tsx wraps the signed-on routes in RequireAuth and the /admin
// routes in RequireAdmin; no page or layout repeats either check.
//
// Both guards are advisory. They keep the screens consistent with the session
// the tab holds, while the server decides access: a missing or expired token
// is answered 401 'Authentication required' and a regular user's token on
// /admin/** 403 'Admin access required', and api/client.ts clears the session
// on a 401, which brings RequireAuth back to the sign-on screen.
//
// The landing after sign-on is not chosen here: LoginPage navigates to the
// login response's `landing`, the counterpart of COSGN00C's XCTL split.
import { useEffect, type ReactNode } from "react";
import { Navigate, Outlet, useLocation } from "react-router";
import { toast } from "sonner";

import { isAdmin, useSession, type Session } from "@/lib/session";

// Byte-identical to ADMIN_ACCESS_REQUIRED in crates/domain/src/messages.rs, the
// `message` of the server's 403. It takes the place of COMEN01C's menu text
// 'No access - Admin Only option... ' [app/cbl/COMEN01C.cbl:L136-143].
const ADMIN_ACCESS_REQUIRED = "Admin access required";

// One id for every admin-gate toast: sonner updates a toast whose id is still
// shown instead of adding another, so StrictMode's second effect run and
// repeated redirects show a single notification.
const ADMIN_TOAST_ID = "admin-access-required";

// useSession already withholds an expired session; the guard checks expiry
// itself as well, so a redirect never depends on when the store last looked.
// An unparsable expires_at counts as expired.
function isLive(session: Session | null): session is Session {
  if (session === null) {
    return false;
  }
  const expiresAtMs = Date.parse(session.expires_at);
  return Number.isFinite(expiresAtMs) && expiresAtMs > Date.now();
}

type GuardProps = {
  /** The guarded content; omitted when the guard is a layout route. */
  children?: ReactNode;
};

/**
 * Requires a live session, as every CardDemo transaction is reached only
 * through COSGN00C sign-on. Without one it redirects to /login, carrying the
 * requested location in the navigation state as `from`.
 *
 * Renders `children` when given, otherwise the matched child route. The
 * server's 401 'Authentication required' is authoritative.
 */
export function RequireAuth({ children }: GuardProps): ReactNode {
  const session = useSession();
  const location = useLocation();

  if (!isLive(session)) {
    return <Navigate to="/login" replace state={{ from: location }} />;
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
 * Without a live session it redirects to /login and shows no toast, the
 * server's order as well (no token is a 401, not a 403), so the guard also
 * holds outside RequireAuth. Renders `children` when given, otherwise the
 * matched child route. The server's 401 and 403 are authoritative.
 */
export function RequireAdmin({ children }: GuardProps): ReactNode {
  const session = useSession();
  const location = useLocation();
  const live = isLive(session);
  const admin = live && isAdmin(session);

  // Toasting is a side effect, so it runs after the commit, never in render.
  useEffect(() => {
    if (live && !admin) {
      toast.error(ADMIN_ACCESS_REQUIRED, { id: ADMIN_TOAST_ID });
    }
  }, [live, admin]);

  if (!live) {
    return <Navigate to="/login" replace state={{ from: location }} />;
  }
  if (!admin) {
    return <Navigate to="/" replace />;
  }
  return children ?? <Outlet />;
}
