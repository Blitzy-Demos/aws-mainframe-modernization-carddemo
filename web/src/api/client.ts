// The single typed client of the CardDemo REST API (shared component "Web API
// access"). Query hooks and pages reach the API only through `api` and the
// result helpers below; this module is the only one that issues network
// requests.
//
// - Requests go to /api on the page's own origin. The Vite proxy strips the
//   prefix and forwards to CARDDEMO_API_PROXY_TARGET, so the paths in `paths`
//   are the API's own, unprefixed routes.
// - The acting user is the bearer token of the signed-on session
//   (lib/session.ts), the client-side counterpart of the COMMAREA's
//   CDEMO-USER-ID and CDEMO-USER-TYPE [app/cpy/COCOM01Y.cpy:L25-28]. No other
//   header, query parameter or body field names a user or a role. The token
//   goes only to the page's own origin under the /api path segment, the API
//   behind the proxy: a call whose `baseUrl` option points anywhere else is
//   sent without it.
// - Every non-2xx answer surfaces as an ApiError carrying the server's
//   `{code, message, fields}` body, the 3270 message line (COSGN00 ERRMSG
//   [app/cpy-bms/COSGN00.CPY:L84]) turned into data, which lib/api-errors.ts
//   shows.
import createClient, { type Middleware } from "openapi-fetch";

import type { components, paths } from "@/api/schema";
import { clearSession, getSession, type Session } from "@/lib/session";

export type ErrorBody = components["schemas"]["ErrorBody"];

export type ErrorFieldBody = components["schemas"]["ErrorFieldBody"];

/**
 * The shape every openapi-fetch call resolves to, so
 * `unwrap(await api.GET(…))` infers `D` as the operation's success body.
 */
export type ApiResult<D> = {
  data?: D;
  error?: unknown;
  response: Response;
};

// Frozen at both levels, because the constant is shared by every caller.
const NO_FIELDS: ErrorFieldBody[] = [];
Object.freeze(NO_FIELDS);

/**
 * The body used when the server's answer carries no usable error body: a
 * proxy or gateway page, an empty body, or JSON of another shape. The text is
 * the server's own `Unexpected error` (crates/domain/src/messages.rs).
 */
export const UNEXPECTED_ERROR_BODY: Readonly<ErrorBody> = Object.freeze({
  code: "UNEXPECTED",
  message: "Unexpected error",
  fields: NO_FIELDS,
});

// A fresh, mutable copy for each error, so no ApiError shares the constant.
function unexpectedErrorBody(): ErrorBody {
  return {
    code: UNEXPECTED_ERROR_BODY.code,
    message: UNEXPECTED_ERROR_BODY.message,
    fields: [],
  };
}

/** A non-2xx answer: the HTTP status and the server's error body. */
export class ApiError extends Error {
  readonly status: number;
  readonly body: ErrorBody;

  constructor(status: number, body: ErrorBody) {
    super(body.message);
    this.name = "ApiError";
    this.status = status;
    this.body = body;
  }
}

function isErrorFieldBody(value: unknown): value is ErrorFieldBody {
  return (
    typeof value === "object" &&
    value !== null &&
    "field" in value &&
    typeof value.field === "string" &&
    "message" in value &&
    typeof value.message === "string"
  );
}

// Only `field` and `message` are kept, in the order received: the server lists
// the fields in the order the COBOL program rejects them.
function toErrorFieldBodies(raw: unknown): ErrorFieldBody[] {
  if (!Array.isArray(raw)) {
    return [];
  }
  return raw
    .filter(isErrorFieldBody)
    .map(({ field, message }) => ({ field, message }));
}

/**
 * The error body of an answer, from what openapi-fetch hands back: the parsed
 * JSON, or the raw text when the body is not JSON. An object with string
 * `code` and `message` is the server's body; its malformed `fields` entries
 * are dropped, and an absent or non-array `fields` becomes `[]`, so the
 * message still reaches the user. Everything else becomes a copy of
 * UNEXPECTED_ERROR_BODY.
 */
export function toErrorBody(raw: unknown): ErrorBody {
  if (
    typeof raw !== "object" ||
    raw === null ||
    !("code" in raw) ||
    typeof raw.code !== "string" ||
    !("message" in raw) ||
    typeof raw.message !== "string"
  ) {
    return unexpectedErrorBody();
  }
  return {
    code: raw.code,
    message: raw.message,
    fields: toErrorFieldBodies("fields" in raw ? raw.fields : undefined),
  };
}

// COSGN00C answers a failed sign-on with 401 and its own message ('Wrong
// Password. Try again ...', 'User not found. Try again ...'
// [app/cbl/COSGN00C.cbl:L241-251]), which the sign-on page must show, so a
// 401 from this path does not end the session.
const LOGIN_PATH = "/auth/login" satisfies keyof paths;

// The session each request was sent by. Every sign-on stores a new session
// object, which getSession returns until the session is cleared or replaced,
// so the object itself identifies the sign-on. openapi-fetch hands onResponse
// the Request object onRequest returned, so the answer is matched to its own
// request, and an entry goes away with its request. A request sent with no
// session, or to anywhere but the API, has no entry.
const sentBySession = new WeakMap<Request, Session>();

// The path segment the Vite proxy forwards to the API, under which every
// request of the default base URL lies.
const API_PREFIX = "/api";

// Whether `url` addresses the API: the page's own origin, with a path that is
// API_PREFIX or lies under it. A segment that merely begins with the prefix
// (/apiary) is another path, and path matching is case-sensitive. An opaque
// origin ("null", as for a file: or sandboxed document) matches nothing, and
// neither does a URL that does not parse. The page's origin is read on every
// request rather than once at load, so a test that stubs `location` after
// this module loads is honoured.
function isApiRequest(url: string): boolean {
  const pageOrigin = globalThis.location.origin;
  if (pageOrigin === "null") {
    return false;
  }
  let target: URL;
  try {
    target = new URL(url);
  } catch {
    return false;
  }
  return (
    target.origin === pageOrigin &&
    (target.pathname === API_PREFIX ||
      target.pathname.startsWith(`${API_PREFIX}/`))
  );
}

// The session's token is attached only to a request for the API
// (isApiRequest). openapi-fetch accepts a `baseUrl` option on every call and
// builds the Request from it before this middleware runs, so the client's own
// base URL does not bound where a request goes. A request elsewhere is sent
// untouched, with no Authorization header and no sentBySession entry, so a 401
// it receives ends no session.
const authMiddleware: Middleware = {
  onRequest({ request }) {
    if (!isApiRequest(request.url)) {
      return request;
    }
    const session = getSession();
    if (session !== null) {
      request.headers.set("Authorization", `Bearer ${session.token}`);
      sentBySession.set(request, session);
    }
    return request;
  },
  // A 401 elsewhere is the server's verdict on the token the request carried:
  // missing, expired or rejected. It ends the session only while the session
  // that sent the request is still the current one. A session signed on while
  // the request was in flight therefore stays, even when the server issued it
  // the same token again: the server's tokens are deterministic, derived from
  // the user and the second of issue, so a user signing on twice within one
  // second receives the same token. A request sent with no session ends none.
  //
  // Ending the session notifies useSession, and RequireAuth then redirects to
  // the sign-on page with the requested location as `from`; a document
  // navigation from here would lose that location. The answer reaches the
  // caller unchanged, whose unwrap raises the server's
  // 'Authentication required'. A 403 ('Admin access required') is left to the
  // caller, which shows it.
  onResponse({ request, response, schemaPath }) {
    if (response.status === 401 && schemaPath !== LOGIN_PATH) {
      const sentBy = sentBySession.get(request);
      if (sentBy !== undefined && getSession() === sentBy) {
        clearSession();
      }
    }
    return response;
  },
};

/**
 * The deadline of every request, from sending it to the last byte of its
 * answer. No API operation streams or runs long, so a live server answers well
 * within it; it ends a request whose connection has stalled, which would
 * otherwise leave the call, and the screen waiting on it, pending forever.
 */
export const REQUEST_TIMEOUT_MS = 30_000;

/**
 * The rejection of a request whose answer was still incomplete when
 * REQUEST_TIMEOUT_MS elapsed: its status line, or the end of its body, had
 * not arrived. The request is abandoned, never retried, so a write may or may
 * not have reached the server.
 */
export class RequestTimeoutError extends Error {
  constructor() {
    super(
      `No answer from the server within ${REQUEST_TIMEOUT_MS / 1000} seconds`,
    );
    this.name = "RequestTimeoutError";
  }
}

// Sends one request, bounded by REQUEST_TIMEOUT_MS and by the caller's
// `signal` option, which openapi-fetch puts on the Request. One controller
// carries both to the network: the caller's abort rejects with the caller's
// own reason (an AbortError unless the caller gave one), which query hooks
// treat as cancellation, and the deadline rejects with a RequestTimeoutError.
// - The signal goes in fetch's init argument, so the Request the middleware
//   saw and sees again in onResponse stays the same object. openapi-fetch's
//   second argument, requestInitExt, is never configured here.
// - The controller stays armed after the headers arrive, because openapi-fetch
//   reads the body afterwards and a stalled body is cut off the same way.
//   Aborting once the body has been read changes nothing, so the deadline is
//   disarmed early only when the call fails or the answer has no body.
// - Nothing is retried: a write that timed out may already have been applied,
//   so the caller sees the rejection and the request is sent exactly once.
function fetchWithDeadline(request: Request): Promise<Response> {
  const caller = request.signal;
  const controller = new AbortController();
  const onCallerAbort = () => {
    disarm();
    controller.abort(caller.reason);
  };
  const timer = setTimeout(() => {
    disarm();
    controller.abort(new RequestTimeoutError());
  }, REQUEST_TIMEOUT_MS);
  function disarm() {
    clearTimeout(timer);
    caller.removeEventListener("abort", onCallerAbort);
  }
  if (caller.aborted) {
    onCallerAbort();
  } else {
    caller.addEventListener("abort", onCallerAbort, { once: true });
  }
  // The global is looked up on every request rather than once at load, so a
  // test that stubs it after this module loads still intercepts the calls.
  return globalThis.fetch(request, { signal: controller.signal }).then(
    (response) => {
      if (response.body === null) {
        disarm();
      }
      return response;
    },
    (error: unknown) => {
      disarm();
      throw error;
    },
  );
}

/**
 * The API client. The origin-qualified base URL equals `/api` in the browser
 * and is what Node's Request (Vitest under jsdom) needs, since it rejects
 * relative URLs. Each call sends one request through fetchWithDeadline and is
 * never retried. A call with no complete answer within REQUEST_TIMEOUT_MS
 * rejects with a RequestTimeoutError; one whose `signal` option is aborted
 * first rejects with that signal's reason, an AbortError by default.
 */
export const api = createClient<paths>({
  baseUrl: `${globalThis.location.origin}${API_PREFIX}`,
  fetch: fetchWithDeadline,
});

api.use(authMiddleware);

/**
 * The success body of an answer and its status, for operations whose 2xx
 * statuses mean different things: `POST /payments` answers 200 with the
 * confirm prompt, writing nothing, and 201 once the payment is made.
 *
 * @throws ApiError for a non-2xx answer, with the server's error body, and for
 * a 2xx answer without a body, with UNEXPECTED_ERROR_BODY. A request without a
 * complete answer never gets here, because the call itself rejects: with a
 * RequestTimeoutError once REQUEST_TIMEOUT_MS has elapsed, with the abort
 * reason of its `signal` option (an AbortError by default) when the caller
 * cancels it, and otherwise with the network error itself.
 */
export function unwrapWithStatus<D>(result: ApiResult<D>): {
  data: D;
  status: number;
} {
  const { data, error, response } = result;
  if (error !== undefined || !response.ok) {
    throw new ApiError(response.status, toErrorBody(error));
  }
  if (data === undefined) {
    throw new ApiError(response.status, unexpectedErrorBody());
  }
  return { data, status: response.status };
}

/**
 * The success body of an answer.
 *
 * @throws ApiError as unwrapWithStatus does.
 */
export function unwrap<D>(result: ApiResult<D>): D {
  return unwrapWithStatus(result).data;
}
