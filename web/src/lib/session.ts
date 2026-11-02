// The signed-on session: the application's single copy of what POST
// /auth/login returns, and the client-side replacement for the COMMAREA that
// COSGN00C hands to the menus on sign-on (CDEMO-USER-ID X(08) and
// CDEMO-USER-TYPE X(01) [app/cpy/COCOM01Y.cpy:L25-28]). The bearer token plays
// the COMMAREA's part on the wire; this module only keeps the client's copy.
//
// The session lives in memory and is mirrored to sessionStorage, the
// session-scoped store, so a reload in the same tab keeps the user signed on
// and a new browser session starts signed out. Nothing is written to any store
// that outlives the browser session.
//
// Consumers: api/client.ts reads the token for the Authorization header and
// clears the session on a 401; routes/guards.tsx and the app shell read it
// through useSession. usr_type and user_id serve navigation and display only.
// The server takes the acting user from the verified token, so neither value
// is ever sent as request data, and a 401 or 403 from the server overrides
// whatever this copy says.
import { useSyncExternalStore } from "react";

import type { components } from "@/api/schema";

type LoginResponse = components["schemas"]["LoginResponse"];

export type Session = Readonly<
  Pick<
    LoginResponse,
    "token" | "user_id" | "usr_type" | "expires_at" | "landing"
  >
>;

export const SESSION_STORAGE_KEY = "carddemo.session";

// CDEMO-USRTYP-ADMIN [app/cpy/COCOM01Y.cpy:L27]. Only this exact code is an
// administrator, as UserType::from_code reads it on the server
// (crates/api/src/auth/jwt.rs) and as COSGN00C's XCTL split treats it
// [app/cbl/COSGN00C.cbl:L230-240]; every other code is a regular user.
const ADMIN_USER_TYPE = "A";

// setTimeout's largest delay, 2^31 - 1 ms (about 24.8 days). A longer delay
// overflows and fires at once, so a later expiry is reached in several steps.
const MAX_TIMER_DELAY_MS = 2_147_483_647;

const listeners = new Set<() => void>();

let expiryTimer: ReturnType<typeof setTimeout> | undefined;

// Rehydrated when the module loads, so the first render after a reload
// already sees the session the tab signed on with.
let current: Session | null = readStoredSession();
if (current !== null) {
  scheduleExpiryCheck(current);
}

// The shape check for text read back from storage, which this module wrote
// but anything with access to the tab could have changed.
function isSession(value: unknown): value is Session {
  return (
    typeof value === "object" &&
    value !== null &&
    "token" in value &&
    typeof value.token === "string" &&
    "user_id" in value &&
    typeof value.user_id === "string" &&
    "usr_type" in value &&
    typeof value.usr_type === "string" &&
    "expires_at" in value &&
    typeof value.expires_at === "string" &&
    "landing" in value &&
    typeof value.landing === "string"
  );
}

// An unparsable expires_at parses to NaN, which compares false, so it counts
// as expired.
function isLive(session: Session): boolean {
  return Date.parse(session.expires_at) > Date.now();
}

// A frozen copy holding exactly the Session fields, so the other fields of a
// login response, or of tampered storage text, are neither kept nor mirrored.
function copyOf(session: Session): Session {
  return Object.freeze({
    token: session.token,
    user_id: session.user_id,
    usr_type: session.usr_type,
    expires_at: session.expires_at,
    landing: session.landing,
  });
}

function parseStoredSession(text: string): Session | null {
  try {
    const value: unknown = JSON.parse(text);
    return isSession(value) ? copyOf(value) : null;
  } catch {
    // Text that is not JSON holds no session.
    return null;
  }
}

// Reading sessionStorage throws where the browser blocks storage (sandboxed
// frames, some privacy settings); the tab then starts signed out. Stored text
// that is malformed or has expired is removed, so it is not read again.
function readStoredSession(): Session | null {
  let text: string | null;
  try {
    text = window.sessionStorage.getItem(SESSION_STORAGE_KEY);
  } catch {
    return null;
  }
  if (text === null) {
    return null;
  }
  const stored = parseStoredSession(text);
  if (stored !== null && isLive(stored)) {
    return stored;
  }
  removeStoredSession();
  return null;
}

// A blocked or full sessionStorage only loses the session on the next reload;
// the in-memory copy keeps the tab signed on. Any earlier stored session is
// removed so that a reload cannot restore it in place of this one.
function writeStoredSession(session: Session): void {
  try {
    window.sessionStorage.setItem(SESSION_STORAGE_KEY, JSON.stringify(session));
  } catch (error) {
    console.warn(
      `The session applies to this page only: sessionStorage rejected ${SESSION_STORAGE_KEY}.`,
      error,
    );
    removeStoredSession();
  }
}

function removeStoredSession(): void {
  try {
    window.sessionStorage.removeItem(SESSION_STORAGE_KEY);
  } catch (error) {
    console.warn(
      `sessionStorage rejected removing ${SESSION_STORAGE_KEY}; a reload may restore the cleared session until it expires.`,
      error,
    );
  }
}

function notifyListeners(): void {
  // A copy, so a listener that subscribes or unsubscribes while being
  // notified does not change this round.
  for (const listener of [...listeners]) {
    listener();
  }
}

function cancelExpiryTimer(): void {
  if (expiryTimer !== undefined) {
    clearTimeout(expiryTimer);
    expiryTimer = undefined;
  }
}

// One timer at a time: scheduling replaces the pending one.
function scheduleExpiryCheck(session: Session): void {
  cancelExpiryTimer();
  const remainingMs = Date.parse(session.expires_at) - Date.now();
  expiryTimer = setTimeout(
    checkExpiry,
    Math.min(remainingMs, MAX_TIMER_DELAY_MS),
  );
}

// Clears a lapsed session, so guards and the sidebar react when the token
// expires rather than at the next request. A timer that fires early, or an
// expiry beyond the longest delay, waits again.
function checkExpiry(): void {
  expiryTimer = undefined;
  if (current === null) {
    return;
  }
  if (isLive(current)) {
    scheduleExpiryCheck(current);
  } else {
    clearSession();
  }
}

/** Starts the session from a login response, replacing any earlier one. */
export function setSession(session: Session): void {
  const next = copyOf(session);
  current = next;
  writeStoredSession(next);
  scheduleExpiryCheck(next);
  notifyListeners();
}

/**
 * Ends the session in memory and in storage. Listeners are notified only when
 * a session was present.
 */
export function clearSession(): void {
  cancelExpiryTimer();
  removeStoredSession();
  if (current === null) {
    return;
  }
  current = null;
  notifyListeners();
}

/**
 * The live session, or null. An expired session is cleared on the way, which
 * notifies the listeners. For code outside render: the API client
 * middleware, guards' redirects and event handlers.
 */
export function getSession(): Session | null {
  if (current !== null && !isLive(current)) {
    clearSession();
  }
  return current;
}

/** Registers a change listener and returns the function that removes it. */
export function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * Whether the session is an administrator's. It selects navigation only (the
 * Administration sidebar group and the admin route guard); the server's 403
 * decides access.
 */
export function isAdmin(session: Session | null): boolean {
  return session?.usr_type === ADMIN_USER_TYPE;
}

// Called by React during render, so it must be pure and return the same value
// until the store changes: the stored object itself, or null once it has
// expired. Clearing is left to the expiry timer and getSession, because
// notifying from here would update state during render.
function getSnapshot(): Session | null {
  return current !== null && isLive(current) ? current : null;
}

// The application renders on the client only; a server render has no session.
function getServerSnapshot(): Session | null {
  return null;
}

/** The live session for rendering; re-renders on sign-on, sign-off and expiry. */
export function useSession(): Session | null {
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);
}
