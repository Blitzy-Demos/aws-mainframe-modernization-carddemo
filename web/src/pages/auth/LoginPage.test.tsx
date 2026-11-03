// FN tests of the CC00 sign-on page (LoginPage; COSGN00C, map COSGN00): Enter
// sends the values as typed and lands on the response's route, an empty user
// ID is rejected before anything is sent, and a failed sign-on shows the
// server's message on the field it names, with the cursor there and both
// values kept.
//
// The network is the global the API client looks up on every request, stubbed
// per test; openapi-fetch hands it the Request it built. No Toaster is
// rendered, so the field error is the only element showing a message, and the
// toast is observed through a spy on the `sonner` object lib/api-errors.ts
// calls.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  createMemoryRouter,
  RouterProvider,
  type RouteObject,
} from "react-router";
import { toast } from "sonner";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { components } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import { clearSession } from "@/lib/session";
import { LOGIN_MESSAGES } from "@/schemas/auth";

import LoginPage from "./LoginPage";

type LoginResponse = components["schemas"]["LoginResponse"];
type ErrorBody = components["schemas"]["ErrorBody"];

const USER_SIGN_ON = {
  token: "test-token",
  expires_at: "2030-01-01T00:00:00Z",
  user_id: "USER0001",
  usr_type: "U",
  landing: "/",
} satisfies LoginResponse;

// V-COSGN00C-005, byte-identical to COSGN00C [app/cbl/COSGN00C.cbl:L242].
const WRONG_PASSWORD = "Wrong Password. Try again ...";

// PageHeader reads the matched routes' handles through useMatches(), so the
// page needs a data router, as App.tsx gives it. The two landing routes stand
// in for the menus COSGN00C transfers to [app/cbl/COSGN00C.cbl:L230-240].
function renderLogin() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const routes: RouteObject[] = [
    {
      path: "/login",
      element: <LoginPage />,
      handle: { title: "Sign on", crumb: "Sign on" } satisfies RouteHandle,
    },
    { path: "/", element: <p>User home</p> },
    { path: "/admin", element: <p>Admin home</p> },
  ];
  const router = createMemoryRouter(routes, { initialEntries: ["/login"] });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router };
}

// Answers every request with `status` and the JSON of `body`, a new Response
// per call.
function stubFetch(status: number, body: unknown) {
  const fetchMock = vi.fn(
    async (_input: RequestInfo | URL, _init?: RequestInit) =>
      new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json" },
      }),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function inputByLabel(label: string): HTMLInputElement {
  const element = screen.getByLabelText(label);
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`"${label}" does not label an input element`);
  }
  return element;
}

function describedBy(element: HTMLElement): string[] {
  return element.getAttribute("aria-describedby")?.split(" ") ?? [];
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  // A successful sign-on stores the session in memory and in sessionStorage.
  clearSession();
  sessionStorage.clear();
});

describe("LoginPage (CC00 sign-on)", () => {
  it("sends the values as typed on Enter and lands on the response's route", async () => {
    const fetchMock = stubFetch(200, USER_SIGN_ON);
    const user = userEvent.setup();
    const { router } = renderLogin();

    await user.type(inputByLabel("User ID"), "USER0001");
    await user.type(inputByLabel("Password"), "PASSWORD{Enter}");

    // Awaited first, so every update the sign-on causes lands while Testing
    // Library is waiting.
    await screen.findByText("User home");
    expect(router.state.location.pathname).toBe("/");

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const request = fetchMock.mock.calls[0]?.[0];
    if (!(request instanceof Request)) {
      throw new Error("the API client did not send a Request");
    }
    expect(request.method).toBe("POST");
    expect(new URL(request.url).pathname).toBe("/api/auth/login");
    // No case change on the way: the server upper-cases (E-20).
    expect(await request.clone().json()).toEqual({
      user_id: "USER0001",
      password: "PASSWORD",
    });
  });

  it("rejects an empty user ID without sending anything", async () => {
    const fetchMock = stubFetch(200, USER_SIGN_ON);
    const user = userEvent.setup();
    const { router } = renderLogin();

    await user.type(inputByLabel("Password"), "PASSWORD{Enter}");

    const fieldError = await screen.findByRole("alert");
    expect(fieldError.textContent).toBe(LOGIN_MESSAGES.userIdEmpty);
    const userId = inputByLabel("User ID");
    expect(userId.getAttribute("aria-invalid")).toBe("true");
    expect(describedBy(userId)).toContain(fieldError.id);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(router.state.location.pathname).toBe("/login");
  });

  it("shows a failed sign-on on the field the server names and keeps both values", async () => {
    const toastError = vi.spyOn(toast, "error");
    // The audited status of a wrong password is 401 (V-COSGN00C-005); the
    // client leaves a 401 from the sign-on path to the page.
    const fetchMock = stubFetch(401, {
      code: "SIGNON_FAILED",
      message: WRONG_PASSWORD,
      fields: [{ field: "PASSWD", message: WRONG_PASSWORD }],
    } satisfies ErrorBody);
    const user = userEvent.setup();
    const { router } = renderLogin();

    await user.type(inputByLabel("User ID"), "USER0001");
    await user.type(inputByLabel("Password"), "WRONGPWD{Enter}");

    const fieldError = await screen.findByText(WRONG_PASSWORD);
    const password = inputByLabel("Password");
    expect(fieldError.getAttribute("role")).toBe("alert");
    expect(password.getAttribute("aria-invalid")).toBe("true");
    expect(describedBy(password)).toContain(fieldError.id);
    // The cursor goes to the password, COSGN00C's `MOVE -1 TO PASSWDL`.
    await waitFor(() => {
      expect(document.activeElement).toBe(password);
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(toastError).toHaveBeenCalledTimes(1);
    expect(toastError.mock.calls[0]?.[0]).toBe(WRONG_PASSWORD);
    expect(inputByLabel("User ID").getAttribute("aria-invalid")).toBe("false");
    expect(router.state.location.pathname).toBe("/login");
    expect(inputByLabel("User ID").value).toBe("USER0001");
    expect(password.value).toBe("WRONGPWD");
  });
});
