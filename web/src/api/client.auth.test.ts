// Tests of the API client's 401 handling (api/client.ts authMiddleware): which
// session a 401 ends, that the caller still receives the server's error, and
// that the way back to the sign-on page is RequireAuth's router redirect, which
// keeps the requested location as `from`, never a document navigation, and
// shows the server's message in a single toast. Also
// which requests carry the session's token at all: only those for the page's
// own origin under /api, whatever base URL a call gives.
//
// The network is the stubbed global the client looks up on every request. The
// test releases each answer itself, so a sign-on can happen while a request is
// in flight.
import { render, screen } from "@testing-library/react";
import { createElement, Fragment, useEffect, type ReactNode } from "react";
import {
  MemoryRouter,
  Route,
  Routes,
  useLocation,
  type Location as RouterLocation,
} from "react-router";
import { toast, Toaster } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiError, api, unwrap } from "@/api/client";
import { showApiError } from "@/lib/api-errors";
import {
  clearSession,
  getSession,
  setSession,
  takeSessionEndCause,
  type Session,
} from "@/lib/session";
import { RequireAdmin, RequireAuth } from "@/routes/guards";

const ONE_HOUR_MS = 60 * 60 * 1000;

function signedOn(token: string, user_id: string, usr_type: string): Session {
  return {
    token,
    user_id,
    usr_type,
    expires_at: new Date(Date.now() + ONE_HOUR_MS).toISOString(),
    landing: usr_type === "A" ? "/admin" : "/",
  };
}

const EARLIER = signedOn("earlier.session.token", "USER0001", "U");

const LATER = signedOn("later.session.token", "ADMIN001", "A");

// EARLIER's user signing on again within the same second: the server's tokens
// are deterministic, so this later sign-on holds EARLIER's token.
const REISSUED: Session = { ...EARLIER };

// The server's answer to a missing, expired or rejected bearer token
// (crates/api/src/error.rs).
function authenticationRequired(): Response {
  return Response.json(
    { code: "UNAUTHENTICATED", message: "Authentication required" },
    { status: 401 },
  );
}

type InFlight = {
  request: Request;
  answer: (response: Response) => void;
};

const inFlight: InFlight[] = [];

function network(request: Request): Promise<Response> {
  return new Promise((resolve) => {
    inFlight.push({ request, answer: resolve });
  });
}

// The one request the client has handed to the network and the test has not
// answered yet.
async function nextRequest(): Promise<InFlight> {
  await vi.waitFor(() => {
    expect(inFlight).toHaveLength(1);
  });
  const sent = inFlight.shift();
  if (sent === undefined) {
    throw new Error("no request reached the network");
  }
  return sent;
}

function viewAccount(): Promise<unknown> {
  return api
    .GET("/accounts/{id}", { params: { path: { id: "00000000001" } } })
    .then(unwrap);
}

// What a call rejected with; a call that resolves fails the test.
async function rejectionOf(call: Promise<unknown>): Promise<unknown> {
  const outcome = await call.then(
    (data: unknown) => ({ rejected: false, value: data }),
    (reason: unknown) => ({ rejected: true, value: reason }),
  );
  expect(outcome.rejected).toBe(true);
  return outcome.value;
}

function expectApiError(
  error: unknown,
  status: number,
  message: string,
  endedSession = false,
): void {
  expect(error).toBeInstanceOf(ApiError);
  expect(error).toMatchObject({
    status,
    message,
    body: { message },
    endedSession,
  });
}

// The toast id lib/api-errors.ts gives the error of a 401 that ended the
// session.
const SESSION_ENDED_TOAST = { id: "session-ended" };

// jsdom performs no document navigation, so the document's location is
// replaced by one that records every way of starting one.
const documentNavigation = vi.fn<(how: string, url?: string | URL) => void>();

function locationRecordingNavigation(real: Location): object {
  return {
    get origin() {
      return real.origin;
    },
    get href() {
      return real.href;
    },
    set href(url: string) {
      documentNavigation("href", url);
    },
    get pathname() {
      return real.pathname;
    },
    set pathname(path: string) {
      documentNavigation("pathname", path);
    },
    get search() {
      return real.search;
    },
    get hash() {
      return real.hash;
    },
    assign(url: string | URL) {
      documentNavigation("assign", url);
    },
    replace(url: string | URL) {
      documentNavigation("replace", url);
    },
    reload() {
      documentNavigation("reload");
    },
  };
}

let documentLocation: object;

beforeEach(() => {
  clearSession();
  inFlight.length = 0;
  documentNavigation.mockClear();
  documentLocation = locationRecordingNavigation(globalThis.location);
  vi.stubGlobal("location", documentLocation);
  vi.stubGlobal("fetch", vi.fn(network));
});

// Checked after every test: no outcome of a 401 navigates the document.
afterEach(() => {
  try {
    expect(documentNavigation).not.toHaveBeenCalled();
    expect(globalThis.location).toBe(documentLocation);
  } finally {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    clearSession();
    takeSessionEndCause();
  }
});

describe("a 401 to a protected request", () => {
  it.each([
    {
      sender: "the current session",
      outcome: "ends it",
      sentWith: EARLIER,
      signedOnMeanwhile: null,
      after: null,
      endedSession: true,
    },
    {
      sender: "an earlier session",
      outcome: "keeps the session signed on meanwhile",
      sentWith: EARLIER,
      signedOnMeanwhile: LATER,
      after: LATER,
      endedSession: false,
    },
    {
      sender: "no session",
      outcome: "keeps the session signed on meanwhile",
      sentWith: null,
      signedOnMeanwhile: LATER,
      after: LATER,
      endedSession: false,
    },
    {
      sender: "an earlier session, its token reissued",
      outcome: "keeps the session signed on meanwhile",
      sentWith: EARLIER,
      signedOnMeanwhile: REISSUED,
      after: REISSUED,
      endedSession: false,
    },
  ])(
    "sent with $sender $outcome, and the call rejects with the server's error",
    async ({ sentWith, signedOnMeanwhile, after, endedSession }) => {
      if (sentWith !== null) {
        setSession(sentWith);
      }
      const call = viewAccount();
      const { request, answer } = await nextRequest();
      expect(request.headers.get("Authorization")).toBe(
        sentWith === null ? null : `Bearer ${sentWith.token}`,
      );
      if (signedOnMeanwhile !== null) {
        setSession(signedOnMeanwhile);
      }

      answer(authenticationRequired());

      expectApiError(
        await rejectionOf(call),
        401,
        "Authentication required",
        endedSession,
      );
      expect(getSession()).toEqual(after);
      // The cause a guard shows as it redirects: only an ended session has one.
      const cause = takeSessionEndCause();
      if (endedSession) {
        expectApiError(cause, 401, "Authentication required", true);
      } else {
        expect(cause).toBeUndefined();
      }
    },
  );

  it.each<{ body: string; respond: () => Response }>([
    {
      body: "text that is not JSON",
      respond: () => new Response("Unauthorized", { status: 401 }),
    },
    {
      body: "no body",
      respond: () => new Response(null, { status: 401 }),
    },
    {
      body: "JSON of another shape",
      respond: () => Response.json({ error: "denied" }, { status: 401 }),
    },
  ])(
    "with $body ends the session with the unexpected error the caller receives",
    async ({ respond }) => {
      setSession(EARLIER);
      const call = viewAccount();
      const { answer } = await nextRequest();

      answer(respond());

      expectApiError(await rejectionOf(call), 401, "Unexpected error", true);
      expect(getSession()).toBeNull();
      expectApiError(takeSessionEndCause(), 401, "Unexpected error", true);
    },
  );

  it("sent by a session a 401 has already ended is marked as that session's end", async () => {
    setSession(EARLIER);
    const ending = viewAccount();
    const first = await nextRequest();
    const late = viewAccount();
    const second = await nextRequest();

    first.answer(authenticationRequired());

    expectApiError(
      await rejectionOf(ending),
      401,
      "Authentication required",
      true,
    );
    expect(getSession()).toBeNull();

    second.answer(authenticationRequired());

    expectApiError(
      await rejectionOf(late),
      401,
      "Authentication required",
      true,
    );
    expectApiError(
      takeSessionEndCause(),
      401,
      "Authentication required",
      true,
    );
    expect(takeSessionEndCause()).toBeUndefined();
  });

  it("whose body cannot be read ends the session with the unexpected error, and the call rejects with the read failure", async () => {
    setSession(EARLIER);
    const call = viewAccount();
    const { answer } = await nextRequest();
    const failure = new TypeError("connection reset");

    answer(
      new Response(
        new ReadableStream({
          start(controller) {
            controller.error(failure);
          },
        }),
        { status: 401 },
      ),
    );

    expect(await rejectionOf(call)).toBe(failure);
    expect(getSession()).toBeNull();
    expectApiError(takeSessionEndCause(), 401, "Unexpected error", true);
  });

  it("sent with the current session ends it after that session's other requests were answered", async () => {
    setSession(EARLIER);
    const answered = viewAccount();
    const first = await nextRequest();
    const call = viewAccount();
    const second = await nextRequest();
    const account = { acct_id: "00000000001" };

    first.answer(Response.json(account));

    await expect(answered).resolves.toEqual(account);
    expect(getSession()).toEqual(EARLIER);

    second.answer(authenticationRequired());

    expectApiError(
      await rejectionOf(call),
      401,
      "Authentication required",
      true,
    );
    expect(getSession()).toBeNull();
  });
});

describe("a 401 to POST /auth/login", () => {
  it("rejects with the sign-on message and keeps the current session", async () => {
    setSession(EARLIER);
    const call = api
      .POST("/auth/login", {
        body: { user_id: "USER0001", password: "WRONG" },
      })
      .then(unwrap);
    const { answer } = await nextRequest();

    answer(
      Response.json(
        {
          code: "SIGNON_FAILED",
          message: "Wrong Password. Try again ...",
          fields: [
            { field: "PASSWD", message: "Wrong Password. Try again ..." },
          ],
        },
        { status: 401 },
      ),
    );

    expectApiError(
      await rejectionOf(call),
      401,
      "Wrong Password. Try again ...",
    );
    expect(getSession()).toEqual(EARLIER);
    expect(takeSessionEndCause()).toBeUndefined();
  });
});

describe("the sign-on redirect after a 401", () => {
  type AccountViewProps = { onFailure: (error: unknown) => void };

  // A guarded screen that loads its account when it mounts.
  function AccountView({ onFailure }: AccountViewProps): ReactNode {
    useEffect(() => {
      viewAccount().catch(onFailure);
    }, [onFailure]);
    return createElement("h1", null, "Account 00000000001");
  }

  type SignOnScreenProps = { onLocation: (location: RouterLocation) => void };

  function SignOnScreen({ onLocation }: SignOnScreenProps): ReactNode {
    const location = useLocation();
    useEffect(() => {
      onLocation(location);
    }, [location, onLocation]);
    return createElement("h1", null, "Sign on");
  }

  it("is RequireAuth's, carrying the requested path, query and fragment as `from`, and shows the server's message once", async () => {
    const toastError = vi.spyOn(toast, "error");
    setSession(EARLIER);
    const onFailure = vi.fn<(error: unknown) => void>();
    const onLocation = vi.fn<(location: RouterLocation) => void>();
    render(
      createElement(
        MemoryRouter,
        { initialEntries: ["/accounts/view?acct=00000000001#details"] },
        createElement(
          Routes,
          null,
          createElement(
            Route,
            { element: createElement(RequireAuth) },
            createElement(Route, {
              path: "/accounts/view",
              element: createElement(AccountView, { onFailure }),
            }),
          ),
          createElement(Route, {
            path: "/login",
            element: createElement(SignOnScreen, { onLocation }),
          }),
        ),
      ),
    );
    await screen.findByRole("heading", { name: "Account 00000000001" });
    const { answer } = await nextRequest();

    answer(authenticationRequired());

    await screen.findByRole("heading", { name: "Sign on" });
    expect(getSession()).toBeNull();
    // The heading is in the document once its render commits, but the screens
    // report through effects, which React may run after that, and the call's
    // rejection settles on its own schedule; so both reports are awaited.
    await vi.waitFor(() => {
      expect(onLocation).toHaveBeenLastCalledWith(
        expect.objectContaining({
          pathname: "/login",
          state: {
            from: expect.objectContaining({
              pathname: "/accounts/view",
              search: "?acct=00000000001",
              hash: "#details",
            }),
          },
        }),
      );
    });
    await vi.waitFor(() => {
      expect(onFailure).toHaveBeenCalledOnce();
    });
    expectApiError(
      onFailure.mock.calls[0]?.[0],
      401,
      "Authentication required",
      true,
    );
    // The page's caller shows nothing here; the guard shows the message.
    expect(toastError).toHaveBeenCalledOnce();
    expect(toastError).toHaveBeenCalledWith(
      "Authentication required",
      SESSION_ENDED_TOAST,
    );
  });

  it("is RequireAdmin's as well when it guards the page alone, showing the server's message once", async () => {
    const toastError = vi.spyOn(toast, "error");
    setSession(LATER);
    render(
      createElement(
        MemoryRouter,
        { initialEntries: ["/admin/users"] },
        createElement(
          Routes,
          null,
          createElement(
            Route,
            { element: createElement(RequireAdmin) },
            createElement(Route, {
              path: "/admin/users",
              element: createElement(AccountView, { onFailure: vi.fn() }),
            }),
          ),
          createElement(Route, {
            path: "/login",
            element: createElement(SignOnScreen, { onLocation: vi.fn() }),
          }),
        ),
      ),
    );
    await screen.findByRole("heading", { name: "Account 00000000001" });
    const { answer } = await nextRequest();

    answer(authenticationRequired());

    await screen.findByRole("heading", { name: "Sign on" });
    await vi.waitFor(() => {
      expect(toastError).toHaveBeenCalledOnce();
    });
    expect(toastError).toHaveBeenCalledWith(
      "Authentication required",
      SESSION_ENDED_TOAST,
    );
  });

  it("shows a single toast when the caller awaiting the answer shows its error as well", async () => {
    const toastError = vi.spyOn(toast, "error");
    setSession(EARLIER);
    const onFailure = vi.fn((error: unknown) => {
      showApiError(error);
    });
    render(
      createElement(
        Fragment,
        null,
        createElement(Toaster),
        createElement(
          MemoryRouter,
          { initialEntries: ["/accounts/view"] },
          createElement(
            Routes,
            null,
            createElement(
              Route,
              { element: createElement(RequireAuth) },
              createElement(Route, {
                path: "/accounts/view",
                element: createElement(AccountView, { onFailure }),
              }),
            ),
            createElement(Route, {
              path: "/login",
              element: createElement(SignOnScreen, { onLocation: vi.fn() }),
            }),
          ),
        ),
      ),
    );
    await screen.findByRole("heading", { name: "Account 00000000001" });
    const { answer } = await nextRequest();

    answer(authenticationRequired());

    await screen.findByRole("heading", { name: "Sign on" });
    await vi.waitFor(() => {
      expect(onFailure).toHaveBeenCalledOnce();
    });
    expect(toastError.mock.calls).toEqual([
      ["Authentication required", SESSION_ENDED_TOAST],
      ["Authentication required", SESSION_ENDED_TOAST],
    ]);
    // Sonner renders each toast call on a timer task of its own; one more
    // task, queued after both, lets them land before the toasts are counted.
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
    expect(screen.getAllByText("Authentication required")).toHaveLength(1);
  });
});

describe("the session token", () => {
  type Target = {
    target: string;
    outcome: string;
    // The call's `baseUrl` option, from the page's origin; null sends the call
    // with the client's own base URL.
    override: (origin: string) => string | null;
    carried: boolean;
  };

  const ACCOUNT = { acct_id: "00000000001" };

  // The page's host on another port, which is another origin.
  function otherPort(origin: string): string {
    const url = new URL(origin);
    url.port = url.port === "1" ? "2" : "1";
    return url.origin;
  }

  // GET /accounts/{id} with the per-call `baseUrl` option openapi-fetch
  // accepts, or with the client's own base URL when `baseUrl` is null.
  function viewAccountAt(baseUrl: string | null): Promise<unknown> {
    if (baseUrl === null) {
      return viewAccount();
    }
    return api
      .GET("/accounts/{id}", {
        baseUrl,
        params: { path: { id: "00000000001" } },
      })
      .then(unwrap);
  }

  // `recording` as a document of opaque origin (a file: or sandboxed page)
  // reports itself: its navigation is still recorded, and its origin is
  // "null".
  function withOpaqueOrigin(recording: object): object {
    const opaque: object = Object.create(recording, {
      origin: { get: () => "null" },
    });
    return opaque;
  }

  it.each<Target>([
    {
      target: "the client's own base URL",
      outcome: "is sent",
      override: () => null,
      carried: true,
    },
    {
      target: "the page's origin under /api",
      outcome: "is sent",
      override: (origin) => `${origin}/api`,
      carried: true,
    },
    {
      target: "another origin",
      outcome: "is withheld",
      override: () => "https://other.example",
      carried: false,
    },
    {
      target: "the page's host on another port",
      outcome: "is withheld",
      override: otherPort,
      carried: false,
    },
    {
      target: "the page's origin under /apiary",
      outcome: "is withheld",
      override: (origin) => `${origin}/apiary`,
      carried: false,
    },
    {
      target: "the page's origin under /API",
      outcome: "is withheld",
      override: (origin) => `${origin}/API`,
      carried: false,
    },
    {
      target: "the page's origin outside /api",
      outcome: "is withheld",
      override: (origin) => `${origin}/other`,
      carried: false,
    },
  ])("$outcome with a call to $target", async ({ override, carried }) => {
    setSession(EARLIER);
    const origin = globalThis.location.origin;
    const baseUrl = override(origin);
    const call = viewAccountAt(baseUrl);
    const { request, answer } = await nextRequest();
    const base = baseUrl ?? `${origin}/api`;
    expect(request.url).toBe(`${base}/accounts/00000000001`);
    expect(request.headers.get("Authorization")).toBe(
      carried ? `Bearer ${EARLIER.token}` : null,
    );

    answer(Response.json(ACCOUNT));

    await expect(call).resolves.toEqual(ACCOUNT);
    expect(getSession()).toEqual(EARLIER);
  });

  it("is withheld from another origin, whose 401 keeps the current session", async () => {
    setSession(EARLIER);
    const call = viewAccountAt("https://other.example");
    const { request, answer } = await nextRequest();
    expect(request.headers.get("Authorization")).toBeNull();

    answer(authenticationRequired());

    expectApiError(await rejectionOf(call), 401, "Authentication required");
    expect(getSession()).toEqual(EARLIER);
  });

  it("is withheld on a page of opaque origin", async () => {
    documentLocation = withOpaqueOrigin(documentLocation);
    vi.stubGlobal("location", documentLocation);
    setSession(EARLIER);
    const call = viewAccount();
    const { request, answer } = await nextRequest();
    expect(request.headers.get("Authorization")).toBeNull();

    answer(Response.json(ACCOUNT));

    await expect(call).resolves.toEqual(ACCOUNT);
    expect(getSession()).toEqual(EARLIER);
  });
});
