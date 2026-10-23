import createClient, { type Middleware } from "openapi-fetch";
import { clearSession, getToken } from "@/auth/session";
import type { paths } from "./schema";

/** Endpoint 1, sign-on: the one operation that takes no bearer token. */
const SIGNON_PATH: keyof paths = "/api/v1/signon";

/**
 * Bearer-token handling for every API call.
 *
 * Identity reaches the API only as the JWT that sign-on returned, as COSGN00C sets the user ID
 * and user type in the COMMAREA at sign-on and never takes them from screen input
 * (app/cbl/COSGN00C.cbl:224-228, app/cpy/COCOM01Y.cpy:25-28). No other header claims who the
 * caller is: the API takes identity only from the verified token.
 *
 * Every operation but sign-on carries `Authorization: Bearer <token>` while a session is stored.
 * Sign-on obtains the token, so it goes out with no `Authorization` header at all, whatever
 * session is stored, rather than relying on the sign-on screen having ended it first.
 *
 * The session a 401 ends is cleared by `answered`, below, as soon as the 401's status arrives,
 * not by response middleware: the client's `fetch` resolves, and openapi-fetch runs response
 * middleware, only once the whole body has arrived.
 *
 * The callback reads, copies and logs no request body: the client is a conduit, so no card
 * number, SSN or password can reach any other sink through it.
 */
const sessionMiddleware: Middleware = {
  onRequest({ request, schemaPath }) {
    if (schemaPath === SIGNON_PATH) {
      request.headers.delete("Authorization");
      return request;
    }
    const token = getToken();
    if (token !== null) {
      request.headers.set("Authorization", `Bearer ${token}`);
    }
    return request;
  },
};

/**
 * The longest any API call may take, from sending the request to the last byte of its answer:
 * 30 seconds, shared by every call, the menu reads and every key-driven read and write alike. A
 * call still unanswered then is aborted and fails, so a request that the API or the dev proxy
 * accepts but never completes cannot leave a screen's keys locked.
 */
export const REQUEST_DEADLINE_MS = 30_000;

/** The `message` of the deadline's `TimeoutError`, which the screen's error row shows. */
const DEADLINE_MESSAGE = `No answer from the server within ${REQUEST_DEADLINE_MS / 1000} seconds`;

/**
 * The global `fetch`'s answer to `request`, once its body has arrived in full.
 *
 * A 401 means either a failed sign-on (endpoint 1) or a missing, expired or invalid token. Either
 * way the stored session is cleared, which is harmless after a failed sign-on, and it is cleared
 * here, once per response, as soon as the status arrives and before the body is awaited. A 401
 * whose body then stalls past the deadline or fails part-way still ends the session, although
 * openapi-fetch never sees that response. `RequireSignon` is subscribed to the session, so a 401
 * on a guarded screen takes the user to `/signon` at once, without waiting for a navigation;
 * nothing navigates from here.
 */
async function answered(request: Request): Promise<Response> {
  const response = await globalThis.fetch(request);
  if (response.status === 401) {
    clearSession();
  }
  await response.clone().arrayBuffer();
  return response;
}

/**
 * Sends `request` through the global `fetch` and resolves with its response once the whole body
 * has arrived, or rejects when `REQUEST_DEADLINE_MS` passes first.
 *
 * The request goes out with a signal of its own. The caller's signal, which TanStack Query
 * aborts to cancel a menu read, aborts it with the caller's reason, so a cancellation stays a
 * cancellation. The deadline aborts it with a `TimeoutError` `DOMException` and rejects with
 * that same error, which openapi-fetch rethrows and the screen shows in its message row as any
 * other failure. The race against the deadline settles even when a `fetch` ignores its signal.
 * On every outcome the timer is cleared and the listener on the caller's signal removed.
 *
 * Nothing is retried: a write the deadline cut off may have committed, so only the operator's
 * next key sends anything again. The body is read from a clone only to wait for its end, then
 * dropped; the original response goes back to openapi-fetch untouched, and nothing is kept or
 * logged, so the client stays a conduit.
 */
async function fetchWithinDeadline(request: Request): Promise<Response> {
  const controller = new AbortController();
  const { signal } = request;
  const forwardAbort = (): void => {
    controller.abort(signal.reason);
  };
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => {
      const timeout = new DOMException(DEADLINE_MESSAGE, "TimeoutError");
      controller.abort(timeout);
      reject(timeout);
    }, REQUEST_DEADLINE_MS);
  });
  if (signal.aborted) {
    controller.abort(signal.reason);
  } else {
    signal.addEventListener("abort", forwardAbort, { once: true });
  }
  try {
    return await Promise.race([
      deadline,
      answered(new Request(request, { signal: controller.signal })),
    ]);
  } finally {
    clearTimeout(timer);
    signal.removeEventListener("abort", forwardAbort);
  }
}

/**
 * The frontend's only path to the API, every call typed by the generated `paths`.
 *
 * `baseUrl` is the page's own origin, never a configured host. It is absolute because the
 * `Request` of the jsdom test environment rejects relative URLs, and it carries no path prefix
 * because every generated path already starts with `/api/v1`. In the browser the origin is the
 * Vite dev server, whose proxy forwards `/api` to the API's bind address.
 *
 * Every call goes through `fetchWithinDeadline`, which hands the global `fetch` one `Request`
 * and looks it up on every call rather than when the client is created, so a test can replace
 * the global `fetch` after importing this module.
 */
export const api = createClient<paths>({
  baseUrl: globalThis.location.origin,
  fetch: fetchWithinDeadline,
});

api.use(sessionMiddleware);
