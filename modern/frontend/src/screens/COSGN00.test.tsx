import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import type { components } from "@/api/schema";
import { SESSION_KEY, getSession, setSession } from "@/auth/session";
import { INVALID_KEY, THANK_YOU } from "@/lib/header";
import { PATHS } from "@/routes";
import { COSGN00 } from "./COSGN00";

/*
 * Render test of the sign-on screen (map COSGN0A, transaction CC00, program COSGN00C) in jsdom,
 * with `fetch` stubbed for endpoint 1. Expected texts are the audited literals of
 * `modern/audit/screens.md` (`COSGN00-nnn`) and `modern/audit/online/COSGN00C.md`
 * (`COSGN00C-nnn`). The typed password is synthetic request input; no response fixture carries a
 * password.
 *
 * Sign-on needs no session, so no test seeds one before rendering except the first-entry test,
 * which proves that the screen clears it.
 */

/** The user ID as the user types it: lower case, which the server alone upper-cases (E26). */
const TYPED_USER_ID = "user0001";
/** A synthetic typed password, sent as typed and never upper-cased by the client (E26). */
const TYPED_PASSWORD = "password";

/** COSGN00-016 to COSGN00-024: the note on rows 7 to 15, each `LENGTH=42`, spacing included. */
const NOTE_ROWS = [
  "+========================================+",
  "|%%%%%%%  NATIONAL RESERVE NOTE  %%%%%%%%|",
  "|%(1)  THE UNITED STATES OF KICSLAND (1)%|",
  "|%$$              ___       ********  $$%|",
  "|%$    {x}       (o o)                 $%|",
  "|%$     ******  (  V  )      O N E     $%|",
  "|%(1)          ---m-m---             (1)%|",
  "|%%~~~~~~~~~~~ ONE DOLLAR ~~~~~~~~~~~~~%%|",
  "+========================================+",
];

const userSignon: components["schemas"]["SignonResponse"] = {
  token: "test-token",
  user_id: "USER0001",
  user_type: "U",
  next_route: "/menu",
};

const adminSignon: components["schemas"]["SignonResponse"] = {
  token: "test-token",
  user_id: "ADMIN001",
  user_type: "A",
  next_route: "/admin",
};

/** Text matching that keeps every space, as the map literals do. */
const exactText = { normalizer: (text: string) => text };

/** A JSON response, as the API sends it. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces `fetch`: `POST /api/v1/signon` is recorded with its `Authorization` header and JSON
 * body, then gets `reply` once `reply.gate` (when given) settles; any other request, and sign-on
 * without a `reply`, gets a 500.
 */
function stubFetch(reply?: {
  status: number;
  body: components["schemas"]["SignonResponse"] | components["schemas"]["ScreenError"];
  gate?: Promise<void>;
}) {
  const sent: { authorization: string | null; body: unknown }[] = [];
  const fetchStub = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const req = input instanceof Request ? input : new Request(input, init);
    if (
      reply !== undefined &&
      req.method === "POST" &&
      new URL(req.url).pathname === "/api/v1/signon"
    ) {
      sent.push({
        authorization: req.headers.get("Authorization"),
        body: await req.clone().json(),
      });
      await reply.gate;
      return json(reply.status, reply.body);
    }
    const unexpected: components["schemas"]["ScreenError"] = {
      message: `Unexpected request ${req.method} ${new URL(req.url).pathname}`,
    };
    return json(500, unexpected);
  });
  vi.stubGlobal("fetch", fetchStub);
  return { fetchStub, sent };
}

/** The screen a navigation reached, by its path. */
function RouteStub() {
  const location = useLocation();
  return <p>{`Stub at ${location.pathname}`}</p>;
}

/** COSGN00 at `/signon`, with stubs at the two `next_route` targets. */
function renderScreen() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[PATHS.COSGN00]}>
        <Routes>
          <Route path={PATHS.COSGN00} element={<COSGN00 />} />
          <Route path={PATHS.COMEN01} element={<RouteStub />} />
          <Route path={PATHS.COADM01} element={<RouteStub />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** `element` as an `<input>`, failing the test when it is anything else. */
function asInput(element: HTMLElement): HTMLInputElement {
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`Expected an input, found <${element.tagName.toLowerCase()}>`);
  }
  return element;
}

function userIdInput(): HTMLInputElement {
  return asInput(screen.getByLabelText("User ID     :", exactText));
}

function passwordInput(): HTMLInputElement {
  return asInput(screen.getByLabelText("Password    :", exactText));
}

/** The `ERRMSG` row (COSGN00-036): the one error-kind message row. */
function errorRow(): HTMLElement {
  const rows = screen
    .getAllByRole("alert")
    .filter((row) => row.getAttribute("data-message-kind") === "error");
  expect(rows).toHaveLength(1);
  return rows[0];
}

/** The text of the header field `name` (`data-field` of `ScreenShell`). */
function headerField(container: HTMLElement, name: string): string | null {
  return container.querySelector(`[data-field="${name}"]`)?.textContent ?? null;
}

function typeCredentials(userId: string, password: string): void {
  fireEvent.change(userIdInput(), { target: { value: userId } });
  fireEvent.change(passwordInput(), { target: { value: password } });
}

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COSGN00 sign-on screen", () => {
  // audit: COSGN00C-002
  // audit: COSGN00C-012
  // audit: COSGN00-015
  // audit: COSGN00-016
  // audit: COSGN00-017
  // audit: COSGN00-018
  // audit: COSGN00-019
  // audit: COSGN00-020
  // audit: COSGN00-021
  // audit: COSGN00-022
  // audit: COSGN00-023
  // audit: COSGN00-024
  // audit: COSGN00-025
  // audit: COSGN00-026
  // audit: COSGN00-029
  // audit: COSGN00-030
  // audit: COSGN00-033
  // audit: COSGN00-034
  // audit: COSGN00-037
  // audit: COSGN00-038
  // audit: COSGN00-040
  // audit: COSGN00-041
  // audit: COSGN00-043
  // audit: COSGN00-044
  // audit: COSGN00-045
  it("renders the cleared map on first entry and ends any stored session", () => {
    // A session left by an earlier sign-on: no COMMAREA reaches COSGN00C, so entry clears it.
    setSession("test-token", "USER0001", "U");
    const { fetchStub } = stubFetch();
    const { container } = renderScreen();

    expect(getSession()).toBeNull();
    expect(fetchStub).not.toHaveBeenCalled();

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "This is a Credit Card Demo Application for Mainframe Modernization",
    );
    expect(headerField(container, "TRNNAME")).toBe("CC00");
    expect(headerField(container, "PGMNAME")).toBe("COSGN00C");
    expect(headerField(container, "TITLE01")).toBe("      AWS Mainframe Modernization       ");
    expect(headerField(container, "TITLE02")).toBe("              CardDemo                  ");
    expect(headerField(container, "CURDATE")).toMatch(/^\d{2}\/\d{2}\/\d{2}$/);
    expect(headerField(container, "CURTIME")).toMatch(/^\d{2}:\d{2}:\d{2}$/);
    for (const label of ["AppID:", "SysID:"]) {
      const field = asInput(screen.getByLabelText(label));
      expect(field.readOnly).toBe(true);
      expect(field.value).toBe("");
    }

    // The note's first and last rows are the same border, so each is found twice.
    expect(NOTE_ROWS.map((row) => screen.getAllByText(row, exactText).length)).toEqual([
      2, 1, 1, 1, 1, 1, 1, 1, 2,
    ]);
    expect(
      screen.getByText("Type your User ID and Password, then press ENTER:", exactText),
    ).toBeTruthy();
    expect(screen.getAllByText("(8 Char)", exactText)).toHaveLength(2);

    const userId = userIdInput();
    expect(userId.maxLength).toBe(8);
    expect(userId.value).toBe("");
    expect(document.activeElement).toBe(userId);
    const password = passwordInput();
    expect(password.type).toBe("password");
    expect(password.maxLength).toBe(8);
    expect(password.value).toBe("");
    // Only the two named inputs are editable: the unnamed dark field at (20,61) is not rendered.
    expect(container.querySelectorAll("input:not([readonly])")).toHaveLength(2);

    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(keys.map((key) => key.textContent)).toEqual(["ENTER=Sign-on", "F3=Exit"]);
    expect(keys.map((key) => key.getAttribute("type"))).toEqual(["submit", "button"]);
    expect(errorRow().textContent).toBe("");
  });

  // audit: COSGN00C-017
  // audit: COSGN00-046
  it.each([
    {
      title: "as a user goes to the main menu",
      typedUserId: TYPED_USER_ID,
      reply: userSignon,
      route: PATHS.COMEN01,
      session: { token: "test-token", userId: "USER0001", userType: "U" },
    },
    {
      title: "as an admin goes to the admin menu",
      typedUserId: "admin001",
      reply: adminSignon,
      route: PATHS.COADM01,
      session: { token: "test-token", userId: "ADMIN001", userType: "A" },
    },
  ])(
    "ENTER $title, sending the input as typed and storing the session",
    async ({ typedUserId, reply, route, session }) => {
      const { sent } = stubFetch({ status: 200, body: reply });
      renderScreen();

      typeCredentials(typedUserId, TYPED_PASSWORD);
      fireEvent.click(screen.getByRole("button", { name: "ENTER=Sign-on" }));

      expect(await screen.findByText(`Stub at ${route}`)).toBeTruthy();
      // The client never upper-cases (E26), and sign-on sends no token: entry cleared the session.
      const typed: components["schemas"]["SignonRequest"] = {
        user_id: typedUserId,
        password: TYPED_PASSWORD,
      };
      expect(sent).toEqual([{ authorization: null, body: typed }]);
      expect(getSession()).toEqual(session);
      expect(sessionStorage).toHaveLength(1);
      expect(sessionStorage.getItem(SESSION_KEY)).not.toContain(TYPED_PASSWORD);
    },
  );

  // audit: COSGN00C-011
  // audit: COSGN00-036
  it("shows a failed sign-on in the error row, keeps the input and stores no session", async () => {
    const failure: components["schemas"]["ScreenError"] = {
      message: "Wrong Password. Try again ...",
      field: "password",
    };
    const { sent } = stubFetch({ status: 401, body: failure });
    const { container } = renderScreen();
    expect(errorRow().textContent).toBe("");

    typeCredentials(TYPED_USER_ID, TYPED_PASSWORD);
    fireEvent.click(screen.getByRole("button", { name: "ENTER=Sign-on" }));

    await waitFor(() => {
      expect(errorRow().textContent).toBe("Wrong Password. Try again ...");
    });
    expect(sent).toHaveLength(1);
    expect(getSession()).toBeNull();
    // The map is sent again from the fields as typed, with the cursor on the field the error names.
    expect(userIdInput().value).toBe(TYPED_USER_ID);
    expect(passwordInput().value).toBe(TYPED_PASSWORD);
    await waitFor(() => {
      expect(document.activeElement).toBe(passwordInput());
    });
    expect(headerField(container, "TRNNAME")).toBe("CC00");
    expect(screen.queryByText(/^Stub at /)).toBeNull();
  });

  // audit: COSGN00C-001
  // audit: COSGN00C-005
  it("shows the invalid-key message for an unserved F-key and clears it on ENTER", async () => {
    const releases: (() => void)[] = [];
    const gate = new Promise<void>((resolve) => {
      releases.push(resolve);
    });
    const notFound: components["schemas"]["ScreenError"] = {
      message: "User not found. Try again ...",
      field: "user_id",
    };
    const { sent } = stubFetch({ status: 401, body: notFound, gate });
    renderScreen();

    // COSGN0A labels no F5, so there is no button: the key is pressed in the focused input.
    fireEvent.keyDown(userIdInput(), { key: "F5" });
    expect(errorRow().textContent).toBe(INVALID_KEY);
    expect(errorRow().textContent?.trimEnd()).toBe("Invalid key pressed. Please see below...");
    expect(sent).toHaveLength(0);

    typeCredentials(TYPED_USER_ID, TYPED_PASSWORD);
    fireEvent.click(screen.getByRole("button", { name: "ENTER=Sign-on" }));

    // Cleared before the outcome: the request is out, its answer held back, and the row is empty.
    await waitFor(() => {
      expect(sent).toHaveLength(1);
      expect(errorRow().textContent).toBe("");
    });
    for (const release of releases) {
      release();
    }
    await waitFor(() => {
      expect(errorRow().textContent).toBe("User not found. Try again ...");
    });
    await waitFor(() => {
      expect(document.activeElement).toBe(userIdInput());
    });
  });

  // audit: COSGN00C-004
  // audit: COSGN00-047
  it("F3 replaces the screen with the session-end text and ends the session", () => {
    const { fetchStub } = stubFetch();
    renderScreen();
    // Seeded after the first render: entry itself clears a session (COSGN00C-002), so only a
    // session stored after entry shows that F3 ends it.
    setSession("test-token", "USER0001", "U");
    expect(getSession()).not.toBeNull();
    typeCredentials(TYPED_USER_ID, TYPED_PASSWORD);

    fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));

    const text = screen.getByText("Thank you for using CardDemo application...");
    expect(text.textContent).toBe(THANK_YOU);
    expect(text.textContent?.trimEnd()).toBe("Thank you for using CardDemo application...");
    expect(getSession()).toBeNull();
    expect(fetchStub).not.toHaveBeenCalled();
    // Plain text alone: no map, header or keys.
    expect(screen.queryByRole("heading")).toBeNull();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
    expect(screen.queryAllByRole("textbox")).toHaveLength(0);
    expect(screen.queryByText("CC00")).toBeNull();
  });
});
