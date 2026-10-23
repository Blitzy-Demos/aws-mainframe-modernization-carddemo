import { z } from "zod";

/**
 * Browser session of the signed-on user. It replaces the identity part of the COMMAREA
 * that COSGN00C fills on a matching password (app/cbl/COSGN00C.cbl:224-228):
 * `CDEMO-USER-ID PIC X(08)` and `CDEMO-USER-TYPE PIC X(01)` (app/cpy/COCOM01Y.cpy:25-28),
 * together with the JWT that sign-on returns.
 *
 * Nothing here authorizes anything: the API takes identity only from the verified JWT
 * (`SignedOnUser` and `AdminUser` extractors), and the user type kept here only chooses
 * navigation. Storage holds exactly the token, user ID and user type, never a password,
 * card number or other payload, and this module logs nothing. The JWT is never decoded:
 * an expired token surfaces as the API's 401, on which `api/client.ts` clears the session.
 * Every sign-on and every clear is announced to the listeners of {@link subscribeSession},
 * through which the route guard follows the session as it changes. So is every restore of the
 * page from the browser's back/forward cache: the restored page resumes as it was frozen, while a
 * later page of the same tab may have ended or replaced the session in the `sessionStorage` they
 * share, so the guard reads it again rather than keep showing a screen signed off elsewhere. The
 * module stays free of React: the subscription is a plain callback set, and the restore is heard
 * through the window's `pageshow` event.
 */

/** The single `sessionStorage` key that holds the session. */
export const SESSION_KEY = "carddemo.session";

const storedSessionSchema = z.object({
  token: z.string().min(1),
  userId: z.string().min(1),
  userType: z.string().min(1),
});

/** The stored session: the token, user ID and user type returned by sign-on. */
export type StoredSession = z.infer<typeof storedSessionSchema>;

/**
 * `sessionStorage`, or `null` where it does not exist (the Node test environment) or where
 * reaching it throws (`SecurityError` when the browser blocks storage). It is looked up on
 * every call and never at import time, so importing this module cannot throw.
 */
function storage(): Storage | null {
  try {
    return globalThis.sessionStorage ?? null;
  } catch {
    return null;
  }
}

/**
 * The functions {@link subscribeSession} registered, each told of every session change and of
 * every restore of the page from the back/forward cache.
 */
const listeners = new Set<() => void>();

/**
 * Tells every registered listener that the session may have changed. It runs whether or not
 * storage took the change, because a failed write or removal can still change what
 * {@link getSession} reads. Each listener runs on its own: one that throws neither stops the
 * others nor reaches the caller, so {@link setSession}, {@link clearSession} and the `pageshow`
 * handler never throw. The set is copied first, so a listener added or removed while the round
 * runs does not change that round.
 */
function notify(): void {
  for (const listener of [...listeners]) {
    try {
      listener();
    } catch {
      // The session change has already happened; a failing listener is the listener's concern.
    }
  }
}

/**
 * The window whose `pageshow` event reports a restore from the back/forward cache, or `null`
 * where there is no window or it cannot take event listeners (the Node test environment). It is
 * looked up on every call and never at import time, so importing this module cannot throw.
 */
function pageWindow(): Window | null {
  const page: Window | undefined = globalThis.window;
  return page !== undefined &&
    typeof page.addEventListener === "function" &&
    typeof page.removeEventListener === "function"
    ? page
    : null;
}

/**
 * Tells the listeners that the session may have changed when the page is restored from the
 * back/forward cache (`persisted`). No {@link setSession} or {@link clearSession} ran in the
 * restored page, yet another page of the tab may have changed the session meanwhile. Any other
 * `pageshow` is a fresh load, whose guard reads the session as it first renders.
 */
function onPageShow(event: PageTransitionEvent): void {
  if (event.persisted) {
    notify();
  }
}

/**
 * Registers `listener` to run after every {@link setSession} and {@link clearSession}, and after
 * every restore of the page from the back/forward cache, and returns the function that removes
 * it. This is the `subscribe` of React's `useSyncExternalStore`, through which `RequireSignon`
 * re-renders as soon as a 401 clears the session, and reads the session again when a restored
 * page resumes. A function registered twice is held, and called, once.
 *
 * One `pageshow` handler serves every listener: added to the window when the first listener is
 * registered and removed once the last is, so a duplicate registration adds none, and nothing
 * stays on the window once nobody listens. Without a window, only the session changes reach
 * the listeners.
 */
export function subscribeSession(listener: () => void): () => void {
  if (listeners.size === 0) {
    pageWindow()?.addEventListener("pageshow", onPageShow);
  }
  listeners.add(listener);
  return () => {
    if (listeners.delete(listener) && listeners.size === 0) {
      pageWindow()?.removeEventListener("pageshow", onPageShow);
    }
  };
}

/**
 * The stored session's raw text, or `null` when nothing is stored or storage is unreachable: the
 * `getSnapshot` of `useSyncExternalStore`. A string compares by value, so calls with no change in
 * between return identical snapshots, as React requires. The text is not parsed here; whether it
 * holds a session is {@link getSession}'s decision. Never throws.
 */
export function getSessionSnapshot(): string | null {
  try {
    return storage()?.getItem(SESSION_KEY) ?? null;
  } catch {
    // Refused storage access: the same snapshot as no session.
    return null;
  }
}

/** Stores the session returned by sign-on, then notifies the listeners. Never throws. */
export function setSession(token: string, userId: string, userType: string): void {
  const store = storage();
  try {
    // Removed first, so a failed write cannot leave an earlier sign-on's session in place.
    store?.removeItem(SESSION_KEY);
    store?.setItem(SESSION_KEY, JSON.stringify({ token, userId, userType }));
  } catch {
    // Quota or security failure: nothing is stored, the next getSession() returns null and
    // the route guard sends the user back to /signon.
  }
  notify();
}

/**
 * The stored session, read afresh on every call so a session cleared elsewhere takes effect
 * at once. Total: absent, unreadable, corrupt or wrongly shaped data all yield `null`.
 */
export function getSession(): StoredSession | null {
  try {
    const text = storage()?.getItem(SESSION_KEY) ?? null;
    if (text === null) {
      return null;
    }
    const raw: unknown = JSON.parse(text);
    const result = storedSessionSchema.safeParse(raw);
    return result.success ? result.data : null;
  } catch {
    // Corrupt JSON or refused storage access: treated the same way as no session.
    return null;
  }
}

/** The bearer token for `api/client.ts`, or `null` when nobody is signed on. */
export function getToken(): string | null {
  return getSession()?.token ?? null;
}

/**
 * Ends the session (sign-off, or a 401 from the API), then notifies the listeners, so a mounted
 * route guard sends the user to `/signon` at once. Never throws.
 */
export function clearSession(): void {
  try {
    storage()?.removeItem(SESSION_KEY);
  } catch {
    // Refused storage access: there is no stored session this module could reach.
  }
  notify();
}

/**
 * Mirrors the 88-level `CDEMO-USRTYP-ADMIN` (`'A'`, app/cpy/COCOM01Y.cpy:27); every other
 * user type, `U` included, is not admin. Chooses navigation only, never authority.
 */
export function isAdmin(session: StoredSession | null): boolean {
  return session !== null && session.userType === "A";
}
