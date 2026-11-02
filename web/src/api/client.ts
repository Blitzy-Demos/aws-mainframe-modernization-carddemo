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
//   header, query parameter or body field names a user or a role.
// - Every non-2xx answer surfaces as an ApiError carrying the server's
//   `{code, message, fields}` body, the 3270 message line (COSGN00 ERRMSG
//   [app/cpy-bms/COSGN00.CPY:L84]) turned into data, which lib/api-errors.ts
//   shows.
import createClient, { type Middleware } from "openapi-fetch";

import type { components, paths } from "@/api/schema";
import { clearSession, getSession } from "@/lib/session";

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

// The SPA route of the sign-on page.
const LOGIN_ROUTE = "/login";

const authMiddleware: Middleware = {
  onRequest({ request }) {
    const session = getSession();
    if (session !== null) {
      request.headers.set("Authorization", `Bearer ${session.token}`);
    }
    return request;
  },
  // A 401 elsewhere means the token is missing, expired or rejected: the
  // session ends and the browser goes to the sign-on page. The answer still
  // reaches the caller, whose unwrap raises the server's
  // 'Authentication required'. A 403 ('Admin access required') is left to the
  // caller, which shows it.
  onResponse({ response, schemaPath }) {
    if (response.status === 401 && schemaPath !== LOGIN_PATH) {
      clearSession();
      if (globalThis.location.pathname !== LOGIN_ROUTE) {
        globalThis.location.assign(LOGIN_ROUTE);
      }
    }
    return response;
  },
};

/**
 * The API client. The origin-qualified base URL equals `/api` in the browser
 * and is what Node's Request (Vitest under jsdom) needs, since it rejects
 * relative URLs. The global is looked up on every request rather than once
 * here, so a test that stubs it after this module loads still intercepts the
 * calls.
 */
export const api = createClient<paths>({
  baseUrl: `${globalThis.location.origin}/api`,
  fetch: (request) => globalThis.fetch(request),
});

api.use(authMiddleware);

/**
 * The success body of an answer and its status, for operations whose 2xx
 * statuses mean different things: `POST /payments` answers 200 with the
 * confirm prompt and 201 once the payment is made, and `POST /transactions`
 * answers 200 for a message-only path and 201 for an add.
 *
 * @throws ApiError for a non-2xx answer, with the server's error body, and for
 * a 2xx answer without a body, with UNEXPECTED_ERROR_BODY. A request that
 * never got an answer rejects with the original error.
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
