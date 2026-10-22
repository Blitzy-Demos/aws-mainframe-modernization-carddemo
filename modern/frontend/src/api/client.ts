import createClient, { type Middleware } from "openapi-fetch";
import { clearSession, getToken } from "@/auth/session";
import type { paths } from "./schema";

/**
 * Bearer-token handling for every API call.
 *
 * Identity reaches the API only as the JWT that sign-on returned, as COSGN00C sets the user ID
 * and user type in the COMMAREA at sign-on and never takes them from screen input
 * (app/cbl/COSGN00C.cbl:224-228, app/cpy/COCOM01Y.cpy:25-28). No other header claims who the
 * caller is: the API takes identity only from the verified token.
 *
 * A 401 means either a failed sign-on (endpoint 1) or a missing, expired or invalid token. Either
 * way the stored session is cleared, which is harmless after a failed sign-on. `RequireSignon`
 * is subscribed to the session, so a 401 on a guarded screen takes the user to `/signon` at once,
 * without waiting for a navigation; nothing navigates from here.
 *
 * Neither callback reads, copies or logs a request or response body: the client is a conduit,
 * so no card number, SSN or password can reach any other sink through it.
 */
const sessionMiddleware: Middleware = {
  onRequest({ request }) {
    const token = getToken();
    if (token !== null) {
      request.headers.set("Authorization", `Bearer ${token}`);
    }
    return request;
  },
  onResponse({ response }) {
    if (response.status === 401) {
      clearSession();
    }
    return response;
  },
};

/**
 * The frontend's only path to the API, every call typed by the generated `paths`.
 *
 * `baseUrl` is the page's own origin, never a configured host. It is absolute because the
 * `Request` of the jsdom test environment rejects relative URLs, and it carries no path prefix
 * because every generated path already starts with `/api/v1`. In the browser the origin is the
 * Vite dev server, whose proxy forwards `/api` to the API's bind address.
 *
 * `fetch` is looked up on every call rather than captured when the client is created, so a
 * test can replace the global `fetch` after importing this module.
 */
export const api = createClient<paths>({
  baseUrl: globalThis.location.origin,
  fetch: (request) => globalThis.fetch(request),
});

api.use(sessionMiddleware);
