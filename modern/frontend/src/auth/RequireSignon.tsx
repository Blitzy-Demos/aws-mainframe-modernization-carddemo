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
 * The path a gated screen sends its visitor to instead of showing, or `null` when the screen may
 * show. It reproduces the navigation COSGN00C starts at sign-on, where a `CDEMO-USRTYP-ADMIN`
 * (`'A'`) user is transferred to COADM01C and every other user to COMEN01C
 * (app/cbl/COSGN00C.cbl:230-240, app/cpy/COCOM01Y.cpy:26-28):
 * - nobody signed on: every gated path goes to `/signon`;
 * - a non-admin user on an admin path (`admin`) goes to the main menu, `/menu`.
 *
 * This is the one gate decision. {@link RequireSignon} applies it at every render, and each gated
 * route's loader applies it before the screen's module is imported, so a refused visitor never
 * downloads the screen. The session is read afresh on every call, with no state or memoisation.
 * The target paths are literals rather than imports from the route table, which imports this
 * module.
 */
export function refusedTarget(admin: boolean): "/signon" | "/menu" | null {
  const session = getSession();
  if (session === null) {
    return "/signon";
  }
  if (admin && !isAdmin(session)) {
    return "/menu";
  }
  return null;
}

/**
 * Route guard for the signed-on and admin screens: it renders the screen unless
 * {@link refusedTarget} refuses it, and then replaces the location with that target.
 *
 * This guard only hides screens. The API is the control: it takes identity only from the
 * verified JWT, and its `AdminUser` extractor answers 403 on every admin endpoint for a
 * token whose user type is not `A`.
 *
 * The route loaders decide the gate once per navigation; this guard decides it again whenever the
 * session changes under a mounted screen. It subscribes to the session store, so a session that
 * `api/client.ts` clears on a 401 re-renders it at once: the mounted screen gives way to `/signon`
 * without waiting for a navigation, as a CICS program started without a COMMAREA transfers to
 * COSGN00C. A page restored from the back/forward cache, whose session a later page of the tab may
 * have ended or replaced, has the guard read the session again as it resumes. The redirect carries
 * no return-to location, because COSGN00C always starts the user at a menu.
 */
export function RequireSignon({ admin = false, children }: RequireSignonProps): ReactNode {
  // Subscribes the guard to navigation. A layout route's `element` keeps one reference, so
  // without this React skips re-rendering the guard when navigation stays inside its layout
  // (`/admin` to `/users`), and a session removed from storage other than through
  // `session.ts`, which announces nothing, would not be read again. This is a router
  // subscription only, not a session cache.
  useLocation();
  // Subscribes the guard to every setSession and clearSession, and to every restore of the page
  // from the back/forward cache. The snapshot only tells React when to render the guard again;
  // the decision below still comes from getSession().
  useSyncExternalStore(subscribeSession, getSessionSnapshot);
  const target = refusedTarget(admin);
  if (target !== null) {
    return <Navigate to={target} replace />;
  }
  return children ?? <Outlet />;
}
