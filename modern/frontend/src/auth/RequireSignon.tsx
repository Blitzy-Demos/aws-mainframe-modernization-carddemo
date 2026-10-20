import type { ReactNode } from "react";
import { Navigate, Outlet, useLocation } from "react-router";
import { getSession, isAdmin } from "./session";

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
 * The session is read on every render, with no state or memoisation, so a session cleared
 * after a 401 takes effect at the next render or navigation. The redirect carries no
 * return-to location, because COSGN00C always starts the user at a menu. The target paths
 * are literals rather than imports from the route table, which imports this module.
 */
export function RequireSignon({ admin = false, children }: RequireSignonProps): ReactNode {
  // Subscribes the guard to navigation. A layout route's `element` keeps one reference, so
  // without this React skips re-rendering the guard when navigation stays inside its layout
  // (`/admin` to `/users`), and a session cleared in between would not be read again. This
  // is a router subscription only, not a session cache.
  useLocation();
  const session = getSession();
  if (session === null) {
    return <Navigate to="/signon" replace />;
  }
  if (admin && !isAdmin(session)) {
    return <Navigate to="/menu" replace />;
  }
  return children ?? <Outlet />;
}
