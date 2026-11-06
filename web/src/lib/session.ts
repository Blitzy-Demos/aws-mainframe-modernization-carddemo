// The signed-on session: the application's single copy of what POST
// /auth/login returns, and the client-side replacement for the COMMAREA that
// COSGN00C hands to the menus on sign-on (CDEMO-USER-ID X(08) and
// CDEMO-USER-TYPE X(01) [app/cpy/COCOM01Y.cpy:L25-28]). The bearer token plays
// the COMMAREA's part on the wire; this module only keeps the client's copy.
//
// The session lives in memory and is mirrored to sessionStorage, the
// session-scoped store, so a reload in the same tab keeps the user signed on
// and a new browser session starts signed out. Nothing is written to any store
// that outlives the browser session. Each change this module makes to the
// stored record is announced on a BroadcastChannel, and Chrome evicts a page
// held in its back/forward cache whose channel receives a message, so Back
// after a sign-off loads that page afresh instead of showing the signed-on
// view it was frozen with. A page restored from the cache all the same, or
// told of a change to the store, checks the store again, so a sign-off in a
// later page of the tab ends its session too.
//
// Consumers: api/client.ts reads the token for the Authorization header and
// clears the session on a 401 to a request that session sent, naming that
// answer as the cause; routes/guards.tsx and the app shell read it through
// useSession, and the guards show the cause (takeSessionEndCause). usr_type and
// user_id serve navigation and display only. The server takes the acting user
// from the verified token, so neither value is ever sent as request data, and
// a 401 or 403 from the server overrides whatever this copy says.
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

// What readStoredSession returns when the browser refuses the read, which
// tells nothing about whether the tab holds a session.
const UNREADABLE = Symbol("sessionStorage unreadable");

// The channel on which every change to the stored record is announced (see
// resyncWithStore). It is opened before the rehydration below, which removes
// an expired or malformed record and so announces it. Where the browser has no
// BroadcastChannel, or refuses one as some refuse storage, nothing is
// announced and the resync alone applies.
const SESSION_CHANNEL_NAME = "carddemo.session";

function openSessionChannel(): BroadcastChannel | null {
  if (typeof BroadcastChannel !== "function") {
    return null;
  }
  try {
    return new BroadcastChannel(SESSION_CHANNEL_NAME);
  } catch (error) {
    console.warn(
      `BroadcastChannel ${SESSION_CHANNEL_NAME} could not be opened; Back may briefly show a page signed on with an earlier session.`,
      error,
    );
    return null;
  }
}

const sessionChannel = openSessionChannel();

// The one message the channel carries. It names no session, so neither the
// token nor the user ever leaves this page through the channel.
const RECORD_CHANGED = "record-changed";

// Rehydrated when the module loads, so the first render after a reload
// already sees the session the tab signed on with. A store the browser blocks
// (sandboxed frames, some privacy settings) starts the tab signed out.
const rehydrated = readStoredSession();
let current: Session | null = rehydrated === UNREADABLE ? null : rehydrated;

// Whether sessionStorage holds the copy of `current`: true for a rehydrated
// session and for one whose write succeeded. A session whose write the
// browser rejected lives in this page only, so no stored value speaks for it.
let mirrored = current !== null;

// What ended the last session, when the code ending it named a cause: the API
// client's 401 answer. Every other end (sign-off, expiry, a resync) and every
// sign-on leave it undefined. takeSessionEndCause hands it out once.
let endCause: unknown;

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

// The live session sessionStorage holds, null when it holds none, or
// UNREADABLE when reading throws, as it does where the browser blocks storage.
// Stored text that is malformed or has expired is removed, so it is not read
// again.
function readStoredSession(): Session | null | typeof UNREADABLE {
  let text: string | null;
  try {
    text = window.sessionStorage.getItem(SESSION_STORAGE_KEY);
  } catch {
    return UNREADABLE;
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
// removed so that a reload cannot restore it in place of this one. Returns
// whether the session was written.
function writeStoredSession(session: Session): boolean {
  try {
    window.sessionStorage.setItem(SESSION_STORAGE_KEY, JSON.stringify(session));
  } catch (error) {
    console.warn(
      `The session applies to this page only: sessionStorage rejected ${SESSION_STORAGE_KEY}.`,
      error,
    );
    removeStoredSession();
    return false;
  }
  announceRecordChange();
  return true;
}

// Announces the removal only when a record was there to remove: clearSession
// calls this with nothing stored as well, and an announcement then would
// evict cached pages that nothing has changed for.
function removeStoredSession(): void {
  let held: boolean;
  try {
    held = window.sessionStorage.getItem(SESSION_STORAGE_KEY) !== null;
    window.sessionStorage.removeItem(SESSION_STORAGE_KEY);
  } catch (error) {
    console.warn(
      `sessionStorage rejected removing ${SESSION_STORAGE_KEY}; a reload may restore the cleared session until it expires.`,
      error,
    );
    return;
  }
  if (held) {
    announceRecordChange();
  }
}

// Tells the origin's other pages that the stored record changed. The message
// matters only for the eviction described at resyncWithStore, so no page
// listens for it: the other pages of this tab, which share the record, are the
// cached ones it evicts, and other tabs keep their own record in their own
// sessionStorage, so a cached page of theirs that it evicts merely loads
// afresh on Back. A refused post leaves the cached pages to the resync.
function announceRecordChange(): void {
  if (sessionChannel === null) {
    return;
  }
  try {
    sessionChannel.postMessage(RECORD_CHANGED);
  } catch (error) {
    console.warn(
      `BroadcastChannel ${SESSION_CHANNEL_NAME} rejected the notice that ${SESSION_STORAGE_KEY} changed; Back may briefly show a page signed on with an earlier session.`,
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

// Ends the session in memory only, leaving sessionStorage as it is.
function endSessionInMemory(): void {
  cancelExpiryTimer();
  current = null;
  endCause = undefined;
  notifyListeners();
}

// sessionStorage is the one record of the session that every document of the
// tab shares. A document the browser restores from its back/forward cache
// resumes with the in-memory session it was frozen with, even when a later
// document of the tab has signed off since; the server keeps no session, so
// that token would still be accepted until it expires. A restored document,
// and one told that another document changed the record, therefore checks the
// record and ends its in-memory session unless the record still holds the
// same token.
//
// This check cannot keep the signed-on view off the screen. Chrome first
// presents a restored document's last painted frame and runs its script,
// pageshow included, only afterwards, so the view it was frozen with shows for
// a frame before this check sends it to sign on. Changes are therefore also
// announced on sessionChannel: Chrome evicts a cached document whose open
// BroadcastChannel receives a message, reporting the not-restored reason
// "broadcastchannel-message", and Back then loads that page afresh. A channel
// that receives nothing leaves the cache working as before. This check stays
// for browsers that restore such a document anyway, for a refused post, and
// for a document frozen before a change that no live document announced, such
// as its session expiring while the tab showed another site.
//
// The session stays when the record cannot be read, which proves nothing, and
// when it never reached the record (writeStoredSession's rejection), since
// this page holds its only copy. The record is left as found: an empty one
// needs no clearing, and a session it holds belongs to the document that
// signed it on.
function resyncWithStore(): void {
  if (current === null || !mirrored) {
    return;
  }
  const stored = readStoredSession();
  if (stored === UNREADABLE) {
    return;
  }
  if (stored !== null && stored.token === current.token) {
    return;
  }
  endSessionInMemory();
}

// Whether a storage event reports a change to the session in this tab's
// sessionStorage: its key, or a clear(), which reports a null key. Reading
// sessionStorage throws where the browser blocks storage, and then no event
// concerns it.
function isSessionStoreChange(event: StorageEvent): boolean {
  if (event.key !== null && event.key !== SESSION_STORAGE_KEY) {
    return false;
  }
  try {
    return event.storageArea === window.sessionStorage;
  } catch {
    return false;
  }
}

window.addEventListener("pageshow", (event) => {
  if (event.persisted) {
    resyncWithStore();
  }
});

window.addEventListener("storage", (event) => {
  if (isSessionStoreChange(event)) {
    resyncWithStore();
  }
});

/** Starts the session from a login response, replacing any earlier one. */
export function setSession(session: Session): void {
  const next = copyOf(session);
  current = next;
  mirrored = writeStoredSession(next);
  endCause = undefined;
  scheduleExpiryCheck(next);
  notifyListeners();
}

/**
 * Ends the session in memory and in storage. Listeners are notified only when
 * a session was present, and only then is `cause`, what ended it, recorded for
 * takeSessionEndCause; without a cause none is recorded.
 */
export function clearSession(cause?: unknown): void {
  cancelExpiryTimer();
  removeStoredSession();
  if (current === null) {
    return;
  }
  current = null;
  endCause = cause;
  notifyListeners();
}

/**
 * What ended the last session, as clearSession recorded it, or undefined; the
 * cause is forgotten once taken, so it is shown once. RequireAuth takes it to
 * show the server's 401 message as it sends the user to sign on.
 */
export function takeSessionEndCause(): unknown {
  const cause = endCause;
  endCause = undefined;
  return cause;
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

/**
 * The session a route requires: none ("public", /login), a live one
 * ("signed-on", the routes under RequireAuth) or a live administrator's
 * ("admin", the routes under RequireAdmin).
 */
export type RouteAccess = "public" | "signed-on" | "admin";

// Every RouteAccess, keyed so that the compiler rejects a missing or an extra
// member.
const ROUTE_ACCESS: Readonly<Record<RouteAccess, true>> = {
  public: true,
  "signed-on": true,
  admin: true,
};

/** Whether `value`, read from untyped data such as a route handle, is a RouteAccess. */
export function isRouteAccess(value: unknown): value is RouteAccess {
  return typeof value === "string" && Object.hasOwn(ROUTE_ACCESS, value);
}

/**
 * Whether `session` has the access `access` names: the one admission rule of
 * the route guards (routes/guards.tsx), which the page loaders apply before
 * they download a page (routes/lazy-page.tsx) and the page title follows
 * (components/page-header.tsx). A session counts only while it is live. Like
 * isAdmin, it selects navigation only; the server's 401 and 403 decide access.
 */
export function sessionAdmits(
  session: Session | null,
  access: RouteAccess,
): boolean {
  switch (access) {
    case "public":
      return true;
    case "signed-on":
      return session !== null && isLive(session);
    case "admin":
      return session !== null && isLive(session) && isAdmin(session);
  }
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
