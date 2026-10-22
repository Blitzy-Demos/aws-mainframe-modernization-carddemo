import { type ReactNode, useSyncExternalStore } from "react";
import { Navigate, Outlet, useLocation } from "react-router";
import { getSession, getSessionSnapshot, isAdmin, subscribeSession } from "./session";

/** Props of {@link RequireSignon}. */
export type RequireSignonProps = {
  /** Admin screens only: `/admin` (COADM01) and the screens reached from its menu. */
  admin?: boolean;
  /** Content to guard when used as a wrapper; a layout route renders its `<Outlet />`. */
  children?: ReactNode;
};

/**
 * Route guard for the signed-on and admin screens. It reproduces the navigation COSGN00C
 * starts at sign-on, where a `CDEMO-USRTYP-ADMIN` (`'A'`) user is transferred to COADM01C
 * and every other user to COMEN01C (app/cbl/COSGN00C.cbl:230-240,
 * app/cpy/COCOM01Y.cpy:26-28):
 * - nobody signed on: every gated path goes to `/signon`;
 * - a non-admin user on an admin path goes to the main menu, `/menu`.
 *
 * This guard only hides screens. The API is the control: it takes identity only from the
 * verified JWT, and its `AdminUser` extractor answers 403 on every admin endpoint for a
 * token whose user type is not `A`.
 *
 * The guard subscribes to the session store, so a session that `api/client.ts` clears on a
 * 401 re-renders it at once: the mounted screen gives way to `/signon` without waiting for a
 * navigation, as a CICS program started without a COMMAREA transfers to COSGN00C. The session
 * itself is read afresh on every render, with no state or memoisation. The redirect carries no
 * return-to location, because COSGN00C always starts the user at a menu. The target paths
 * are literals rather than imports from the route table, which imports this module.
 */
export function RequireSignon({ admin = false, children }: RequireSignonProps): ReactNode {
  // Subscribes the guard to navigation. A layout route's `element` keeps one reference, so
  // without this React skips re-rendering the guard when navigation stays inside its layout
  // (`/admin` to `/users`), and a session removed from storage other than through
  // `session.ts`, which announces nothing, would not be read again. This is a router
  // subscription only, not a session cache.
  useLocation();
  // Subscribes the guard to every setSession and clearSession. The snapshot only tells React
  // when to render the guard again; the decision below still comes from getSession().
  useSyncExternalStore(subscribeSession, getSessionSnapshot);
  const session = getSession();
  if (session === null) {
    return <Navigate to="/signon" replace />;
  }
  if (admin && !isAdmin(session)) {
    return <Navigate to="/menu" replace />;
  }
  return children ?? <Outlet />;
}
