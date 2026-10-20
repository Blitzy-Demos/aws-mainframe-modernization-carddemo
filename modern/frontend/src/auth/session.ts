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

/** Stores the session returned by sign-on. Never throws. */
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

/** Ends the session (sign-off, or a 401 from the API). Never throws. */
export function clearSession(): void {
  try {
    storage()?.removeItem(SESSION_KEY);
  } catch {
    // Refused storage access: there is no stored session this module could reach.
  }
}

/**
 * Mirrors the 88-level `CDEMO-USRTYP-ADMIN` (`'A'`, app/cpy/COCOM01Y.cpy:27); every other
 * user type, `U` included, is not admin. Chooses navigation only, never authority.
 */
export function isAdmin(session: StoredSession | null): boolean {
  return session !== null && session.userType === "A";
}
