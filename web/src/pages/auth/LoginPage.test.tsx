// FN tests of the CC00 sign-on page (LoginPage; COSGN00C, map COSGN00): Enter
// sends the values as typed and lands on the response's route, an empty user
// ID is rejected before anything is sent, and a failed sign-on shows the
// server's message on the field it names, moves the cursor there from the
// Sign on button, keeps both values and lets the next attempt be sent. A
// second press while a sign-on is out sends nothing, and the page stays busy
// until the landing route replaces it (or a navigation keeps the screen at
// another location).
//
// The network is the global the API client looks up on every request, stubbed
// per test; openapi-fetch hands it the Request it built. No Toaster is
// rendered, so the field error is the only element showing a message, and the
// toast is observed through a spy on the `sonner` object lib/api-errors.ts
// calls.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
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
// With `holdUserHome`, `/` is a lazy route that loads only once it settles,
// and with `userHomeLoader` it has that loader, as App.tsx's page loaders
// download a landing page's chunk; either way the sign-on page stays on
// screen meanwhile.
function renderLogin(
  options: {
    holdUserHome?: Promise<void>;
    userHomeLoader?: () => Promise<null>;
  } = {},
) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const { holdUserHome, userHomeLoader } = options;
  const userHome = <p>User home</p>;
  const routes: RouteObject[] = [
    {
      path: "/login",
      element: <LoginPage />,
      handle: { title: "Sign on", crumb: "Sign on" } satisfies RouteHandle,
    },
    holdUserHome === undefined
      ? { path: "/", loader: userHomeLoader, element: userHome }
      : {
          path: "/",
          lazy: async () => {
            await holdUserHome;
            return { element: userHome };
          },
        },
    { path: "/admin", element: <p>Admin home</p> },
  ];
  const router = createMemoryRouter(routes, { initialEntries: ["/login"] });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router, queryClient };
}

// A promise and the function that settles it, for an answer a test holds.
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

// Answers every request with `status` and the JSON of `body`, a new Response
// per call, once `held` has settled, so a test can see the page while a
// sign-on is out.
function stubFetch(
  status: number,
  body: unknown,
  held: Promise<void> = Promise.resolve(),
) {
  const fetchMock = vi.fn(
    async (_input: RequestInfo | URL, _init?: RequestInit) => {
      await held;
      return new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json" },
      });
    },
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

function signOnButton(): HTMLButtonElement {
  const element = screen.getByRole("button", { name: "Sign on" });
  if (!(element instanceof HTMLButtonElement)) {
    throw new Error('"Sign on" is not a button element');
  }
  return element;
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

    // The card title is a level-2 heading, and it names the form.
    expect(
      screen.getByRole("heading", { level: 2, name: "Sign on to CardDemo" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("form", { name: "Sign on to CardDemo" }),
    ).toBeTruthy();

    // Mixed case, so a case change on the way would show in the body; the
    // stubbed answer carries the user ID as the server upper-cases it.
    await user.type(inputByLabel("User ID"), "User0001");
    await user.type(inputByLabel("Password"), "passWord{Enter}");

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
      user_id: "User0001",
      password: "passWord",
    });
  });

  it("keeps the form busy until the router has loaded the landing route", async () => {
    stubFetch(200, USER_SIGN_ON);
    let finishLanding: () => void = () => undefined;
    const user = userEvent.setup();
    const { router, queryClient } = renderLogin({
      userHomeLoader: () =>
        new Promise<null>((resolve) => {
          finishLanding = () => {
            resolve(null);
          };
        }),
    });

    await user.type(inputByLabel("User ID"), "USER0001");
    await user.type(inputByLabel("Password"), "PASSWORD");
    // Every change of the button's disabled state from the submit on, so a
    // render that enables it between the answer and the landing shows.
    const signOn = screen.getByRole("button", { name: "Sign on" });
    const disabledStates: boolean[] = [];
    const observer = new MutationObserver(() => {
      disabledStates.push(signOn.hasAttribute("disabled"));
    });
    observer.observe(signOn, { attributeFilter: ["disabled"] });

    await user.keyboard("{Enter}");

    // The sign-on has succeeded and its render has committed; the router is
    // still loading the landing route.
    await vi.waitFor(() => {
      expect(queryClient.getMutationCache().getAll()[0]?.state.status).toBe(
        "success",
      );
    });
    await act(async () => undefined);
    observer.disconnect();
    expect(router.state.navigation.state).toBe("loading");
    expect(signOn.hasAttribute("disabled")).toBe(true);
    expect(disabledStates).toEqual([true]);
    expect(
      screen
        .getByRole("form", { name: "Sign on to CardDemo" })
        .getAttribute("aria-busy"),
    ).toBe("true");

    await act(async () => {
      finishLanding();
    });

    await screen.findByText("User home");
    expect(router.state.location.pathname).toBe("/");
  });

  it("ends the busy state when a navigation keeps the sign-on screen at another location", async () => {
    stubFetch(200, USER_SIGN_ON);
    const user = userEvent.setup();
    // A landing route that never finishes loading, so only the second
    // navigation decides what the screen shows.
    const { router, queryClient } = renderLogin({
      userHomeLoader: () => new Promise<null>(() => undefined),
    });
    await user.type(inputByLabel("User ID"), "USER0001");
    await user.type(inputByLabel("Password"), "PASSWORD{Enter}");
    await vi.waitFor(() => {
      expect(queryClient.getMutationCache().getAll()[0]?.state.status).toBe(
        "success",
      );
    });
    await act(async () => undefined);
    const signOn = screen.getByRole("button", { name: "Sign on" });
    expect(signOn.hasAttribute("disabled")).toBe(true);

    await act(() => router.navigate("/login"));

    expect(router.state.location.pathname).toBe("/login");
    expect(signOn.hasAttribute("disabled")).toBe(false);
    expect(
      screen
        .getByRole("form", { name: "Sign on to CardDemo" })
        .getAttribute("aria-busy"),
    ).toBe("false");
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
    // client leaves a 401 from the sign-on path to the page. The answer is
    // held, so the focus before it arrives can be checked.
    const answer = deferred<void>();
    const fetchMock = stubFetch(
      401,
      {
        code: "SIGNON_FAILED",
        message: WRONG_PASSWORD,
        fields: [{ field: "PASSWD", message: WRONG_PASSWORD }],
      } satisfies ErrorBody,
      answer.promise,
    );
    const user = userEvent.setup();
    const { router } = renderLogin();

    await user.type(inputByLabel("User ID"), "USER0001");
    await user.type(inputByLabel("Password"), "WRONGPWD");
    // Submitted from the button, so the cursor is off the password until the
    // answer puts it there.
    await user.tab();
    const signOn = signOnButton();
    expect(document.activeElement).toBe(signOn);
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledTimes(1);
    });
    const password = inputByLabel("Password");
    expect(document.activeElement).not.toBe(password);
    answer.resolve();

    const fieldError = await screen.findByText(WRONG_PASSWORD);
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

    // The failure releases the form, so Enter sends the next attempt.
    expect(signOn.disabled).toBe(false);
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(toastError).toHaveBeenCalledTimes(2);
    });
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("sends one sign-on when Sign on is pressed twice before the answer", async () => {
    const answer = deferred<void>();
    const fetchMock = stubFetch(200, USER_SIGN_ON, answer.promise);
    // No delay between the two clicks, so the second lands before the page
    // re-renders as busy.
    const user = userEvent.setup({ delay: null });
    renderLogin();

    await user.type(inputByLabel("User ID"), "USER0001");
    await user.type(inputByLabel("Password"), "PASSWORD");
    await user.dblClick(signOnButton());
    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalled();
    });
    answer.resolve();

    await screen.findByText("User home");
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("stays busy after a sign-on until the landing route replaces the page", async () => {
    stubFetch(200, USER_SIGN_ON);
    const userHome = deferred<void>();
    const user = userEvent.setup();
    const { router } = renderLogin({ holdUserHome: userHome.promise });

    await user.type(inputByLabel("User ID"), "USER0001");
    await user.type(inputByLabel("Password"), "PASSWORD{Enter}");
    // The sign-on has succeeded: the router is loading the landing route.
    await waitFor(() => {
      expect(router.state.navigation.location?.pathname).toBe("/");
    });
    // TanStack Query hands the settled mutation to the page in a timer, so
    // one timer turn lets the page render it before the checks.
    await act(async () => {
      await new Promise<void>((resolve) => {
        setTimeout(resolve, 0);
      });
    });

    expect(router.state.location.pathname).toBe("/login");
    const signOn = signOnButton();
    expect(signOn.disabled).toBe(true);
    expect(signOn.querySelector("svg")).not.toBeNull();
    expect(
      screen
        .getByRole("form", { name: "Sign on to CardDemo" })
        .getAttribute("aria-busy"),
    ).toBe("true");

    userHome.resolve();
    await screen.findByText("User home");
    expect(router.state.location.pathname).toBe("/");
  });
});
