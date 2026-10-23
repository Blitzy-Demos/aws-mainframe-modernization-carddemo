import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createMemoryRouter,
  MemoryRouter,
  Route,
  RouterProvider,
  Routes,
  useLocation,
} from "react-router";
import type { components } from "@/api/schema";
import {
  SESSION_KEY,
  type StoredSession,
  clearSession,
  getSession,
  getSessionSnapshot,
  getToken,
  setSession,
  subscribeSession,
} from "@/auth/session";
import { PATHS, routes } from "@/routes";
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

/**
 * COSGN00C-005: `CCDA-MSG-INVALID-KEY PIC X(50)` (`app/cpy/CSMSG01Y.cpy:20-21`), its 40-character
 * `VALUE` padded with spaces to the 50 bytes of the field.
 */
const INVALID_KEY = `Invalid key pressed. Please see below...${" ".repeat(10)}`;

/**
 * COSGN00C-004: the 43-character `CCDA-MSG-THANK-YOU` value (`app/cpy/CSMSG01Y.cpy:18-19`) moved
 * into `WS-MESSAGE PIC X(80)` and sent as all 80 bytes (`app/cbl/COSGN00C.cbl:38,89,162-172`).
 */
const SESSION_END = `Thank you for using CardDemo application...${" ".repeat(37)}`;

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

/**
 * The header field `name` (`data-field`): the one `Input` of that name in the `CardHeader` title
 * rows of `ScreenShell`, failing the test when there is not exactly one.
 */
function headerInput(container: HTMLElement, name: string): HTMLInputElement {
  const fields = container.querySelectorAll(
    `[data-slot="card-header"] input[data-field="${name}"]`,
  );
  expect(fields).toHaveLength(1);
  const [field] = fields;
  if (!(field instanceof HTMLInputElement)) {
    throw new Error(`Expected the header input ${name}`);
  }
  return field;
}

/** The value of the header field `name`. */
function headerField(container: HTMLElement, name: string): string {
  return headerInput(container, name).value;
}

/** The text of every map-grid cell whose `POS` is (`row`, `col`), as `GridCell` places it. */
function cellsStartingAt(container: HTMLElement, row: number, col: number): (string | null)[] {
  const selector = `[class~="row-start-${row}"][class~="col-start-${col}"]`;
  return Array.from(container.querySelectorAll(selector), (cell) => cell.textContent);
}

function typeCredentials(userId: string, password: string): void {
  fireEvent.change(userIdInput(), { target: { value: userId } });
  fireEvent.change(passwordInput(), { target: { value: password } });
}

/** The texts of the `FormMessage`s that `input` names in its `aria-describedby`. */
function fieldMessages(input: HTMLInputElement): (string | null)[] {
  const ids = input.getAttribute("aria-describedby")?.split(" ") ?? [];
  return ids.flatMap((id) => {
    const note = document.getElementById(id);
    return note?.getAttribute("data-slot") === "form-message" ? [note.textContent] : [];
  });
}

/**
 * Asserts the form's error state of the two inputs (COSGN00C-011): only `marked`, the input the
 * last `ScreenError` named, is `aria-invalid` and described by a `FormMessage` holding `message`;
 * with `marked` null neither input is. Every `FormMessage` is visually hidden, because the error
 * row is the screen's one visible message.
 */
function expectMarked(
  container: HTMLElement,
  marked: HTMLInputElement | null,
  message: string,
): void {
  for (const input of [userIdInput(), passwordInput()]) {
    const named = input === marked;
    expect(input.getAttribute("aria-invalid")).toBe(String(named));
    expect(fieldMessages(input)).toEqual(named ? [message] : []);
  }
  const notes = Array.from(container.querySelectorAll('[data-slot="form-message"]'));
  expect(notes).toHaveLength(marked === null ? 0 : 1);
  expect(notes.filter((note) => !note.classList.contains("sr-only"))).toEqual([]);
}

/** The `ENTER=Sign-on` button (COSGN00-046). */
function enterButton(): HTMLElement {
  return screen.getByRole("button", { name: "ENTER=Sign-on" });
}

/** The `F3=Exit` button (COSGN00-047). */
function exitButton(): HTMLElement {
  return screen.getByRole("button", { name: "F3=Exit" });
}

/** The sign-on `<form>`, which holds both inputs. */
function signonForm(): HTMLFormElement {
  const form = userIdInput().form;
  if (form === null) {
    throw new Error("The user ID input is not in a form");
  }
  return form;
}

/**
 * Whether the screen shows the locked keyboard of a sign-on in flight: both keys disabled, the key
 * row and the form busy, both inputs read-only; or, with `locked` false, all of them free.
 */
function expectLocked(locked: boolean): void {
  expect(enterButton().getAttribute("aria-disabled")).toBe(locked ? "true" : null);
  expect(exitButton().getAttribute("aria-disabled")).toBe(locked ? "true" : null);
  expect(screen.getByRole("group", { name: "Function keys" }).getAttribute("aria-busy")).toBe(
    String(locked),
  );
  expect(signonForm().getAttribute("aria-busy")).toBe(String(locked));
  expect(userIdInput().readOnly).toBe(locked);
  expect(passwordInput().readOnly).toBe(locked);
}

/**
 * Gives a refused key the time a sign-on it had started would take to reach `fetch`. The wait runs
 * inside `act`, as the form's own state updates after an ENTER do.
 */
async function lingers(): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => {
      setTimeout(resolve, 50);
    });
  });
}

/** Types the credentials, presses ENTER and waits for the main menu `userSignon` routes to. */
async function signOnAsUser(): Promise<void> {
  typeCredentials(TYPED_USER_ID, TYPED_PASSWORD);
  fireEvent.click(screen.getByRole("button", { name: "ENTER=Sign-on" }));
  expect(await screen.findByText(`Stub at ${PATHS.COMEN01}`)).toBeTruthy();
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
  // audit: COSGN00-028
  // audit: COSGN00-029
  // audit: COSGN00-030
  // audit: COSGN00-032
  // audit: COSGN00-033
  // audit: COSGN00-034
  // audit: COSGN00-035
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
    expect(cellsStartingAt(container, 20, 61)).toEqual([]);
    // The zero-length stoppers add no element. Those at (19,52) and (20,52) share their position
    // with the `(8 Char)` literal declared after them, whose cell is the only one there
    // (X-SCREENS-3); nothing starts at the stopper's position (20,63).
    expect(cellsStartingAt(container, 19, 52)).toEqual(["(8 Char)"]);
    expect(cellsStartingAt(container, 20, 52)).toEqual(["(8 Char)"]);
    expect(cellsStartingAt(container, 20, 63)).toEqual([]);

    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(keys.map((key) => key.textContent)).toEqual(["ENTER=Sign-on", "F3=Exit"]);
    expect(keys.map((key) => key.getAttribute("type"))).toEqual(["submit", "button"]);
    expect(errorRow().textContent).toBe("");
  });

  it.each([
    // audit: COSGN00-002
    {
      id: "COSGN00-002",
      name: "TRNNAME",
      row: 1,
      col: 8,
      length: 4,
      label: "Tran :",
      shows: "CC00",
    },
    // audit: COSGN00-003
    {
      id: "COSGN00-003",
      name: "TITLE01",
      row: 1,
      col: 21,
      length: 40,
      label: null,
      shows: "      AWS Mainframe Modernization       ",
    },
    // audit: COSGN00-005
    {
      id: "COSGN00-005",
      name: "CURDATE",
      row: 1,
      col: 71,
      length: 8,
      label: "Date :",
      shows: expect.stringMatching(/^\d{2}\/\d{2}\/\d{2}$/),
    },
    // audit: COSGN00-007
    {
      id: "COSGN00-007",
      name: "PGMNAME",
      row: 2,
      col: 8,
      length: 8,
      label: "Prog :",
      shows: "COSGN00C",
    },
    // audit: COSGN00-008
    {
      id: "COSGN00-008",
      name: "TITLE02",
      row: 2,
      col: 21,
      length: 40,
      label: null,
      shows: "              CardDemo                  ",
    },
    // The eight-character time leaves the ninth position of `CURTIME` blank (X-SCREENS-11).
    // audit: COSGN00-010
    {
      id: "COSGN00-010",
      name: "CURTIME",
      row: 2,
      col: 71,
      length: 9,
      label: "Time :",
      shows: expect.stringMatching(/^\d{2}:\d{2}:\d{2}$/),
    },
  ])(
    "$id shows $name in a read-only Input at ($row,$col), $length wide, in the card header",
    ({ name, row, col, length, label, shows }) => {
      // The header value is an output field of the map: a read-only `Input` that holds `LENGTH`
      // characters (`size`), in the cell of its `POS` and `LENGTH` in the frame's title rows. The
      // map's literal before it is its bound `Label`; a title, which no literal labels, is named
      // by `aria-label`.
      const { fetchStub } = stubFetch();
      const { container } = renderScreen();

      const field = headerInput(container, name);
      expect(field.dataset.slot).toBe("input");
      expect(field.readOnly).toBe(true);
      expect(field.size).toBe(length);
      expect(field.parentElement?.className).toBe(
        `row-start-${row} col-start-${col} col-span-${length}`,
      );
      expect(field.value).toEqual(shows);
      const labels = Array.from(field.labels ?? []);
      expect(labels.map((bound) => bound.textContent)).toEqual(label === null ? [] : [label]);
      expect(labels.every((bound) => bound.dataset.slot === "label")).toBe(true);
      expect(screen.getByRole("textbox", { name: label ?? `Title row ${row}` })).toBe(field);
      expect(fetchStub).not.toHaveBeenCalled();
    },
  );

  it.each([
    // audit: COSGN00-011
    // audit: COSGN00-012
    { label: "AppID:", labelCol: 1, name: "APPLID", fieldCol: 8 },
    // audit: COSGN00-013
    // audit: COSGN00-014
    { label: "SysID:", labelCol: 64, name: "SYSID", fieldCol: 71 },
  ])(
    "$label in BLUE at (3,$labelCol) labels the empty read-only $name at (3,$fieldCol) in the header",
    ({ label, labelCol, name, fieldCol }) => {
      // Map line 3 is the third of COSGN00's title rows in the frame's `CardHeader`: each `Label`
      // is bound to a read-only `Input` eight characters wide that stays empty, as no CICS region
      // assigns the IDs (X-SCREENS-1).
      const { fetchStub } = stubFetch();
      const { container } = renderScreen();

      const text = screen.getByText(label, exactText);
      if (!(text instanceof HTMLLabelElement)) {
        throw new Error(`${label} is not a <label>`);
      }
      expect(text.dataset.slot).toBe("label");
      expect(text.classList.contains("text-muted-foreground")).toBe(true);
      expect(text.parentElement?.className).toBe(`row-start-3 col-start-${labelCol} col-span-6`);
      expect(text.closest('[data-slot="card-header"]')).not.toBeNull();

      const field = headerInput(container, name);
      expect(field.id).not.toBe("");
      expect(text.htmlFor).toBe(field.id);
      expect(asInput(screen.getByLabelText(label, exactText))).toBe(field);
      expect(field.dataset.slot).toBe("input");
      expect(field.readOnly).toBe(true);
      expect(field.size).toBe(8);
      expect(field.value).toBe("");
      expect(field.parentElement?.className).toBe(`row-start-3 col-start-${fieldCol} col-span-8`);
      expect(fetchStub).not.toHaveBeenCalled();
    },
  );

  it("frames the map in a positioned main that holds every control at text-sm, with a skip link shown over it", () => {
    // jsdom applies no Tailwind CSS, so the frame's token rules are proved by the classes that
    // produce them: `main` is the containing block of its positioned descendants, has no side
    // padding below `lg`, and gives every `<input>` `text-sm` and every `<label>` the
    // `--text-sm--line-height` line height (AAP 0.4.3), whatever the generated defaults are. The
    // rule matches the element: inside `FormControl` an Input's `data-slot` is `form-control`.
    const { fetchStub } = stubFetch();
    const { container } = renderScreen();

    const main = screen.getByRole("main");
    for (const token of [
      "relative",
      "overflow-x-auto",
      "py-4",
      "lg:px-4",
      "[&_input]:text-sm",
      "[&_label]:leading-(--text-sm--line-height)",
    ]) {
      expect(main.classList.contains(token)).toBe(true);
    }
    const inputs = Array.from(container.querySelectorAll("input"));
    const labels = Array.from(container.querySelectorAll("label"));
    expect(new Set(inputs.map((input) => input.dataset.slot))).toEqual(
      new Set(["input", "form-control"]),
    );
    expect(labels.length).toBeGreaterThan(0);
    expect([...inputs, ...labels].filter((control) => !main.contains(control))).toEqual([]);

    // The skip link leads `main`, is visually hidden until keyboard focus, and is then placed
    // absolutely over the frame's top left in the frame's font, with the system focus ring and no
    // layer token.
    const skip = screen.getByRole("link", { name: "Skip to main content" });
    expect(main.firstElementChild).toBe(skip);
    const body = container.querySelector('[data-slot="card-content"]');
    expect(skip.getAttribute("href")).toBe(`#${body?.id ?? ""}`);
    for (const token of [
      "sr-only",
      "font-mono",
      "text-sm",
      "bg-background",
      "focus-visible:not-sr-only",
      "focus-visible:absolute",
      "focus-visible:top-2",
      "focus-visible:left-2",
      "focus-visible:ring-3",
      "focus-visible:ring-ring/50",
      "focus-visible:outline-hidden",
    ]) {
      expect(skip.classList.contains(token)).toBe(true);
    }
    expect(Array.from(skip.classList).filter((token) => /(^|:)-?z-/.test(token))).toEqual([]);
    expect(fetchStub).not.toHaveBeenCalled();
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
  it.each([
    {
      outcome: "a blank user ID (COSGN00C-007)",
      typedUserId: "",
      typedPassword: TYPED_PASSWORD,
      status: 400,
      failure: { message: "Please enter User ID ...", field: "user_id" },
      marked: userIdInput,
    },
    {
      outcome: "a blank password (COSGN00C-008)",
      typedUserId: TYPED_USER_ID,
      typedPassword: "",
      status: 400,
      failure: { message: "Please enter Password ...", field: "password" },
      marked: passwordInput,
    },
    {
      outcome: "a wrong password (COSGN00C-018)",
      typedUserId: TYPED_USER_ID,
      typedPassword: TYPED_PASSWORD,
      status: 401,
      failure: { message: "Wrong Password. Try again ...", field: "password" },
      marked: passwordInput,
    },
    {
      outcome: "an unknown user (COSGN00C-019)",
      typedUserId: TYPED_USER_ID,
      typedPassword: TYPED_PASSWORD,
      status: 401,
      failure: { message: "User not found. Try again ...", field: "user_id" },
      marked: userIdInput,
    },
    {
      outcome: "a failed user read (COSGN00C-020)",
      typedUserId: TYPED_USER_ID,
      typedPassword: TYPED_PASSWORD,
      status: 500,
      failure: { message: "Unable to verify the User ...", field: "user_id" },
      marked: userIdInput,
    },
  ])(
    "shows $outcome in the error row, marks the input it names and stores no session",
    async ({ typedUserId, typedPassword, status, failure, marked }) => {
      const { sent } = stubFetch({ status, body: failure });
      const { container } = renderScreen();
      expect(errorRow().textContent).toBe("");
      expectMarked(container, null, "");

      typeCredentials(typedUserId, typedPassword);
      fireEvent.click(screen.getByRole("button", { name: "ENTER=Sign-on" }));

      await waitFor(() => {
        expect(errorRow().textContent).toBe(failure.message);
      });
      // ERRMSG is RED, the `--destructive` token whole: the description takes `text-destructive`
      // in place of the destructive variant's 90 percent `text-destructive/90`.
      const row = errorRow();
      expect(row.classList.contains("*:data-[slot=alert-description]:text-destructive")).toBe(true);
      expect(row.className).not.toContain("text-destructive/90");
      // ERRMSG is `BRT`: its text is semibold.
      const description = row.querySelector('[data-slot="alert-description"]');
      expect(description?.classList.contains("font-semibold")).toBe(true);
      expect(sent).toHaveLength(1);
      expect(getSession()).toBeNull();
      // The map is sent again from the fields as typed, with the cursor on the field the error
      // names, which is marked invalid with the server message for assistive technology.
      expect(userIdInput().value).toBe(typedUserId);
      expect(passwordInput().value).toBe(typedPassword);
      await waitFor(() => {
        expect(document.activeElement).toBe(marked());
      });
      expectMarked(container, marked(), failure.message);
      expect(headerField(container, "TRNNAME")).toBe("CC00");
      expect(screen.queryByText(/^Stub at /)).toBeNull();
    },
  );

  // audit: COSGN00C-011
  it("moves the mark to the input each answer names and clears it on the next ENTER", async () => {
    const wrongPassword: components["schemas"]["ScreenError"] = {
      message: "Wrong Password. Try again ...",
      field: "password",
    };
    stubFetch({ status: 401, body: wrongPassword });
    const { container } = renderScreen();
    typeCredentials(TYPED_USER_ID, TYPED_PASSWORD);
    const enter = screen.getByRole("button", { name: "ENTER=Sign-on" });

    fireEvent.click(enter);
    await waitFor(() => {
      expect(errorRow().textContent).toBe("Wrong Password. Try again ...");
    });
    expectMarked(container, passwordInput(), "Wrong Password. Try again ...");

    // The next answer names the user ID: the mark moves there, and the kept dark password is sent
    // again as typed (FSET).
    const unverified: components["schemas"]["ScreenError"] = {
      message: "Unable to verify the User ...",
      field: "user_id",
    };
    const retry = stubFetch({ status: 500, body: unverified });
    fireEvent.click(enter);
    await waitFor(() => {
      expect(errorRow().textContent).toBe("Unable to verify the User ...");
    });
    const typed: components["schemas"]["SignonRequest"] = {
      user_id: TYPED_USER_ID,
      password: TYPED_PASSWORD,
    };
    expect(retry.sent).toEqual([{ authorization: null, body: typed }]);
    await waitFor(() => {
      expect(document.activeElement).toBe(userIdInput());
    });
    expectMarked(container, userIdInput(), "Unable to verify the User ...");

    // A sign-on sent clears every mark before its answer arrives; the success then leaves.
    const releases: (() => void)[] = [];
    const gate = new Promise<void>((resolve) => {
      releases.push(resolve);
    });
    const success = stubFetch({ status: 200, body: userSignon, gate });
    fireEvent.click(enter);
    await waitFor(() => {
      expect(success.sent).toHaveLength(1);
      expect(errorRow().textContent).toBe("");
    });
    expectMarked(container, null, "");
    for (const release of releases) {
      release();
    }
    expect(await screen.findByText(`Stub at ${PATHS.COMEN01}`)).toBeTruthy();
  });

  // audit: COSGN00C-011
  it("marks no input for a failure that names none, clearing the earlier mark", async () => {
    const notFound: components["schemas"]["ScreenError"] = {
      message: "User not found. Try again ...",
      field: "user_id",
    };
    stubFetch({ status: 401, body: notFound });
    const { container } = renderScreen();
    typeCredentials(TYPED_USER_ID, TYPED_PASSWORD);
    const enter = screen.getByRole("button", { name: "ENTER=Sign-on" });

    fireEvent.click(enter);
    await waitFor(() => {
      expect(errorRow().textContent).toBe("User not found. Try again ...");
    });
    expectMarked(container, userIdInput(), "User not found. Try again ...");

    // A gateway's empty 502 carries no `ScreenError`, so no field: only the row shows it.
    const badGateway = vi.fn(
      async () => new Response(null, { status: 502, statusText: "Bad Gateway" }),
    );
    vi.stubGlobal("fetch", badGateway);
    fireEvent.click(enter);
    await waitFor(() => {
      expect(errorRow().textContent).toBe("HTTP 502 Bad Gateway");
    });
    expect(badGateway).toHaveBeenCalledTimes(1);
    expectMarked(container, null, "");
    expect(userIdInput().value).toBe(TYPED_USER_ID);
    expect(passwordInput().value).toBe(TYPED_PASSWORD);
    expect(getSession()).toBeNull();
  });

  it("keeps the label and the text colour of the input a failure names; only its mark changes", async () => {
    const wrongPassword: components["schemas"]["ScreenError"] = {
      message: "Wrong Password. Try again ...",
      field: "password",
    };
    stubFetch({ status: 401, body: wrongPassword });
    renderScreen();
    typeCredentials(TYPED_USER_ID, TYPED_PASSWORD);

    fireEvent.click(enterButton());
    await waitFor(() => {
      expect(passwordInput().getAttribute("aria-invalid")).toBe("true");
    });
    const label = screen.getByText("Password    :", exactText);
    expect(label.getAttribute("data-error")).toBe("true");
    expect(label.classList.contains("data-[error=true]:text-foreground")).toBe(true);
    expect(label.classList.contains("data-[error=true]:text-destructive")).toBe(false);
    expect(passwordInput().className).not.toContain("text-destructive");
  });

  it.each([
    { name: "User ID", box: userIdInput },
    { name: "Password", box: passwordInput },
  ])("gives the $name input the shared text-sm field box of its 8 characters", ({ box }) => {
    stubFetch();
    renderScreen();
    expect(box().classList.contains("field-length-8")).toBe(true);
    expect(box().classList.contains("text-sm")).toBe(true);
    expect(box().classList.contains("text-base")).toBe(false);
  });

  it("gives every header value the shared text-sm box of its LENGTH, each title exactly that wide", () => {
    stubFetch();
    const { container } = renderScreen();
    const boxes = Array.from(
      container.querySelectorAll<HTMLInputElement>('[data-slot="card-header"] input'),
    );
    expect(
      boxes.map((box) => ({
        field: box.dataset.field,
        sized: box.classList.contains(`field-length-${box.size}`),
        small: box.classList.contains("text-sm"),
        width: ["w-full", "w-(--field-width)"].filter((width) => box.classList.contains(width)),
      })),
    ).toEqual(
      ["TRNNAME", "TITLE01", "CURDATE", "PGMNAME", "TITLE02", "CURTIME", "APPLID", "SYSID"].map(
        (field) => ({
          field,
          sized: true,
          small: true,
          width: [field.startsWith("TITLE") ? "w-(--field-width)" : "w-full"],
        }),
      ),
    );
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
    expect(errorRow().textContent).toHaveLength(50);
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

  // audit: COSGN00-036
  it.each([
    { what: "all spaces", text: " ".repeat(78), blank: true },
    { what: "a program message", text: "User not found. Try again ...", blank: false },
  ])(
    "marks the error row empty, with no visible box, exactly while it shows no text: $what",
    async ({ text, blank }) => {
      const failure: components["schemas"]["ScreenError"] = { message: text };
      stubFetch({ status: 401, body: failure });
      renderScreen();

      // First entry: no message yet, so the live region exists with no box.
      expect(errorRow().textContent).toBe("");
      expect(errorRow().getAttribute("data-message-empty")).toBe("true");
      expect(errorRow().className).toContain("border-transparent");

      typeCredentials(TYPED_USER_ID, TYPED_PASSWORD);
      fireEvent.click(enterButton());
      await waitFor(() => {
        expect(errorRow().textContent).toBe(text);
      });

      // The marker `src/index.css` selects in forced-colors mode agrees with the transparent
      // border the row has outside that mode.
      expect(errorRow().getAttribute("data-message-empty")).toBe(blank ? "true" : null);
      expect(errorRow().className.includes("border-transparent")).toBe(blank);
    },
  );

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
    expect(text.textContent).toBe(SESSION_END);
    expect(text.textContent).toHaveLength(80);
    expect(getSession()).toBeNull();
    expect(fetchStub).not.toHaveBeenCalled();
    // Plain text alone: no map, header or keys.
    expect(screen.queryByRole("heading")).toBeNull();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
    expect(screen.queryAllByRole("textbox")).toHaveLength(0);
    expect(screen.queryByText("CC00")).toBeNull();
  });

  // audit: COSGN00-046
  // audit: COSGN00-047
  it.each([
    {
      how: "a click on ENTER=Sign-on",
      press: () => {
        fireEvent.click(enterButton());
      },
    },
    {
      how: "the Enter key",
      press: () => {
        fireEvent.keyDown(window, { key: "Enter" });
      },
    },
    {
      how: "a submit of the form",
      press: () => {
        fireEvent.submit(signonForm());
      },
    },
    {
      how: "a click on F3=Exit",
      press: () => {
        fireEvent.click(exitButton());
      },
    },
    {
      how: "the F3 key",
      press: () => {
        fireEvent.keyDown(window, { key: "F3" });
      },
    },
    {
      how: "the unserved F5 key",
      press: () => {
        fireEvent.keyDown(userIdInput(), { key: "F5" });
      },
    },
  ])(
    "while a sign-on is out, $how is refused and the keys unlock when its answer arrives",
    async ({ press }) => {
      let release = (): void => undefined;
      const gate = new Promise<void>((resolve) => {
        release = resolve;
      });
      const wrongPassword: components["schemas"]["ScreenError"] = {
        message: "Wrong Password. Try again ...",
        field: "password",
      };
      const { fetchStub, sent } = stubFetch({ status: 401, body: wrongPassword, gate });
      renderScreen();
      expectLocked(false);

      typeCredentials(TYPED_USER_ID, TYPED_PASSWORD);
      fireEvent.click(enterButton());
      await waitFor(() => {
        expect(sent).toHaveLength(1);
        expectLocked(true);
      });

      press();
      press();
      await lingers();
      // Nothing was sent, ended or shown: the screen is as the first ENTER left it.
      expect(fetchStub).toHaveBeenCalledTimes(1);
      expect(sent).toHaveLength(1);
      expect(errorRow().textContent).toBe("");
      expect(screen.queryByText("Thank you for using CardDemo application...")).toBeNull();
      expectLocked(true);

      release();
      await waitFor(() => {
        expect(errorRow().textContent).toBe("Wrong Password. Try again ...");
        expectLocked(false);
      });
      expect(userIdInput().value).toBe(TYPED_USER_ID);
      expect(passwordInput().value).toBe(TYPED_PASSWORD);

      // The next ENTER is taken: a new sign-on goes out with the same input.
      fireEvent.click(enterButton());
      await waitFor(() => {
        expect(sent).toHaveLength(2);
      });
      expect(sent[1]).toEqual(sent[0]);
      await waitFor(() => {
        expectLocked(false);
      });
    },
  );

  // audit: COSGN00-046
  it("takes ENTER once when it is pressed again before the first sign-on is accepted", async () => {
    const { fetchStub, sent } = stubFetch({ status: 200, body: userSignon });
    renderScreen();
    typeCredentials(TYPED_USER_ID, TYPED_PASSWORD);

    // All three arrive before the schema's check of the first has finished.
    fireEvent.click(enterButton());
    fireEvent.click(enterButton());
    fireEvent.keyDown(window, { key: "Enter" });

    expect(await screen.findByText(`Stub at ${PATHS.COMEN01}`)).toBeTruthy();
    await lingers();
    expect(fetchStub).toHaveBeenCalledTimes(1);
    expect(sent).toHaveLength(1);
  });

  // audit: COSGN00-027
  it("sends nothing for a value the schema refuses and takes the next ENTER", async () => {
    const { fetchStub, sent } = stubFetch({ status: 200, body: userSignon });
    renderScreen();

    // Nine characters, one past `LENGTH=8`; `maxLength` stops typing, not a value set at once.
    typeCredentials(`${TYPED_USER_ID}x`, TYPED_PASSWORD);
    fireEvent.click(enterButton());
    await waitFor(() => {
      expect(errorRow().textContent).not.toBe("");
    });
    await lingers();
    expect(fetchStub).not.toHaveBeenCalled();
    expectLocked(false);

    typeCredentials(TYPED_USER_ID, TYPED_PASSWORD);
    fireEvent.click(enterButton());
    expect(await screen.findByText(`Stub at ${PATHS.COMEN01}`)).toBeTruthy();
    expect(sent).toHaveLength(1);
  });
});

// The session sign-on keeps in `sessionStorage`: one key holding exactly the token, user ID and
// user type; a later sign-on replaces it, a refused write leaves none, and entry and F3 remove
// only it. No storage failure stops the screen.
describe("COSGN00 session storage", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  /** Every request `fetch` received: method, path, query, `Authorization` header and JSON body. */
  async function sentRequests() {
    return Promise.all(
      vi.mocked(fetch).mock.calls.map(async ([input, init]) => {
        const request = input instanceof Request ? input : new Request(input, init);
        const url = new URL(request.url);
        const text = await request.clone().text();
        const body: unknown = text === "" ? undefined : JSON.parse(text);
        return {
          method: request.method,
          path: url.pathname,
          query: Object.fromEntries(url.searchParams),
          authorization: request.headers.get("Authorization"),
          body,
        };
      }),
    );
  }

  it("ENTER stores exactly the token, user ID and user type as JSON under carddemo.session, after one POST with no bearer token and no query", async () => {
    stubFetch({ status: 200, body: userSignon });
    renderScreen();

    await signOnAsUser();

    expect(SESSION_KEY).toBe("carddemo.session");
    expect(sessionStorage).toHaveLength(1);
    expect(JSON.parse(sessionStorage.getItem(SESSION_KEY) ?? "null")).toStrictEqual({
      token: "test-token",
      userId: "USER0001",
      userType: "U",
    });
    const typed: components["schemas"]["SignonRequest"] = {
      user_id: TYPED_USER_ID,
      password: TYPED_PASSWORD,
    };
    expect(await sentRequests()).toEqual([
      { method: "POST", path: "/api/v1/signon", query: {}, authorization: null, body: typed },
    ]);
  });

  it("a later sign-on replaces the session an earlier one stored", async () => {
    stubFetch({ status: 200, body: userSignon });
    renderScreen();
    // Stored after entry, which itself ends any session.
    setSession("test-token-earlier", "ADMIN001", "A");

    await signOnAsUser();

    expect(getSession()).toEqual({ token: "test-token", userId: "USER0001", userType: "U" });
    expect(sessionStorage).toHaveLength(1);
  });

  // Endpoint 1 is the one operation that takes no bearer token, so the client sends none on it
  // whatever session is stored, without relying on entry having ended that session.
  it("ENTER sends no bearer token while an earlier session is stored, after one POST with no query", async () => {
    stubFetch({ status: 200, body: userSignon });
    renderScreen();
    // Stored after entry, which itself ends any session.
    setSession("test-token-earlier", "ADMIN001", "A");
    expect(getToken()).toBe("test-token-earlier");

    await signOnAsUser();

    const typed: components["schemas"]["SignonRequest"] = {
      user_id: TYPED_USER_ID,
      password: TYPED_PASSWORD,
    };
    expect(await sentRequests()).toEqual([
      { method: "POST", path: "/api/v1/signon", query: {}, authorization: null, body: typed },
    ]);
  });

  it("a refused write leaves no session, earlier or new, and sign-on still completes", async () => {
    stubFetch({ status: 200, body: userSignon });
    renderScreen();
    setSession("test-token-earlier", "ADMIN001", "A");
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("quota exceeded", "QuotaExceededError");
    });

    await signOnAsUser();

    expect(getSession()).toBeNull();
    expect(sessionStorage.getItem(SESSION_KEY)).toBeNull();
  });

  it("first entry and F3 each end the session and leave every other key in place", () => {
    stubFetch();
    sessionStorage.setItem("unrelated", "kept");
    setSession("test-token", "USER0001", "U");

    renderScreen();
    expect(getSession()).toBeNull();
    expect(sessionStorage.getItem(SESSION_KEY)).toBeNull();
    expect(sessionStorage.getItem("unrelated")).toBe("kept");

    setSession("test-token", "USER0001", "U");
    fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));
    expect(screen.getByText("Thank you for using CardDemo application...")).toBeTruthy();
    expect(getSession()).toBeNull();
    expect(sessionStorage.getItem("unrelated")).toBe("kept");
    expect(sessionStorage).toHaveLength(1);
  });

  it("entry and F3 still show their screens when removing the session throws", () => {
    stubFetch();
    const stored = { token: "test-token", userId: "USER0001", userType: "U" };
    setSession(stored.token, stored.userId, stored.userType);
    vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => {
      throw new DOMException("storage blocked", "SecurityError");
    });

    renderScreen();
    expect(screen.getByRole("button", { name: "ENTER=Sign-on" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));
    expect(screen.getByText("Thank you for using CardDemo application...")).toBeTruthy();
    // Storage refused both removals, so the session it holds is unchanged.
    expect(getSession()).toEqual(stored);
  });
});

// Sign-on is the one screen no guard stands in front of in the route table. A failed sign-on is a
// 401 too, and the session the client clears on it is the one entry already removed: the screen
// stays at `/signon` with the audited message and the input as typed, and nothing navigates.
describe("COSGN00 in the route table: a failed sign-on stays on sign-on", () => {
  it("a sign-on answered 401 keeps the screen, its message and the typed input, with no navigation", async () => {
    const failure: components["schemas"]["ScreenError"] = {
      message: "Wrong Password. Try again ...",
      field: "password",
    };
    const { sent } = stubFetch({ status: 401, body: failure });
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, { initialEntries: [PATHS.COSGN00] });
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
    const enter = await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 });
    const entry = router.state.location;

    typeCredentials(TYPED_USER_ID, TYPED_PASSWORD);
    fireEvent.click(enter);

    await waitFor(() => {
      expect(errorRow().textContent).toBe(failure.message);
    });
    expect(router.state.location).toBe(entry);
    expect(router.state.location.pathname).toBe(PATHS.COSGN00);
    expect(screen.getByRole("button", { name: "ENTER=Sign-on" })).toBe(enter);
    expect(userIdInput().value).toBe(TYPED_USER_ID);
    expect(passwordInput().value).toBe(TYPED_PASSWORD);
    expect(getSession()).toBeNull();
    const typed: components["schemas"]["SignonRequest"] = {
      user_id: TYPED_USER_ID,
      password: TYPED_PASSWORD,
    };
    expect(sent).toEqual([{ authorization: null, body: typed }]);
    router.dispose();
  });
});

// `setSession` and `clearSession` never throw, even when a listener of `subscribeSession` does,
// and a listener that throws stops none after it. The throwing listener here is subscribed before
// the route table mounts, so on every change it runs, and throws, before the sign-on guard's own
// listener. The sign-on is an admin's: the admin menu has no redirect of its own on a refused
// token, so only the guard's listener can replace it with sign-on after a 401.
describe("COSGN00 in the route table: a session listener that throws", () => {
  it("sign-on still stores the session and reaches the server's next route, a 401 there still sends the guard to sign-on, and F3 still ends the session", async () => {
    const seen: (StoredSession | null)[] = [];
    const failing = vi.fn((): void => {
      seen.push(getSession());
      throw new Error("session listener failure");
    });
    onTestFinished(subscribeSession(failing));

    const refused: components["schemas"]["ScreenError"] = {
      message: "The sign-on token is missing, invalid or expired",
    };
    let refuse = (): void => undefined;
    const refusal = new Promise<void>((resolve) => {
      refuse = resolve;
    });
    const sent: { method: string; path: string; authorization: string | null }[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const req = input instanceof Request ? input : new Request(input, init);
        const path = new URL(req.url).pathname;
        sent.push({ method: req.method, path, authorization: req.headers.get("Authorization") });
        if (req.method === "POST" && path === "/api/v1/signon") {
          return json(200, adminSignon);
        }
        if (req.method === "GET" && path === "/api/v1/menus/admin") {
          await refusal;
          return json(401, refused);
        }
        const unexpected: components["schemas"]["ScreenError"] = {
          message: `Unexpected request ${req.method} ${path}`,
        };
        return json(500, unexpected);
      }),
    );
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, { initialEntries: [PATHS.COSGN00] });
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
    const enter = await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 });
    // Entry ended the session: the listener was told, threw, and the screen rendered regardless.
    expect(seen).toEqual([null]);

    typeCredentials("admin001", TYPED_PASSWORD);
    fireEvent.click(enter);

    expect(
      await screen.findByRole("heading", { level: 1, name: "Admin Menu" }, { timeout: 5_000 }),
    ).toBeTruthy();
    expect(router.state.location.pathname).toBe(adminSignon.next_route);
    const stored: StoredSession = { token: "test-token", userId: "ADMIN001", userType: "A" };
    expect(getSession()).toEqual(stored);
    expect(seen).toEqual([null, stored]);
    await waitFor(() => {
      expect(sent).toHaveLength(2);
    });
    expect(sent).toEqual([
      { method: "POST", path: "/api/v1/signon", authorization: null },
      { method: "GET", path: "/api/v1/menus/admin", authorization: "Bearer test-token" },
    ]);

    // The admin menu's load is refused: the client clears the session, the throwing listener is
    // told first, and the guard's listener after it still replaces the screen with sign-on, whose
    // entry ends the session once more.
    refuse();
    expect(
      await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
    ).toBeTruthy();
    expect(router.state.location.pathname).toBe(PATHS.COSGN00);
    expect(screen.queryByRole("heading", { level: 1, name: "Admin Menu" })).toBeNull();
    expect(getSession()).toBeNull();
    expect(errorRow().textContent).toBe("");
    expect(seen).toEqual([null, stored, null, null]);

    fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));
    expect(screen.getByText("Thank you for using CardDemo application...")).toBeTruthy();
    expect(getSession()).toBeNull();
    expect(seen).toEqual([null, stored, null, null, null]);
    expect(failing.mock.results.map((result) => result.type)).toEqual(Array(5).fill("throw"));
    expect(sent).toHaveLength(2);
    router.dispose();
  });
});

// `subscribeSession` and `getSessionSnapshot` are the store `RequireSignon` reads through
// `useSyncExternalStore`. The screen makes the session changes: every entry and F3 end the session
// (`clearSession`) and a successful sign-on stores it (`setSession`); only a session left by an
// earlier sign-on is seeded directly. A restore from the back/forward cache is the window's
// `pageshow`, fired directly, and where no window can be reached the store is called directly,
// because no screen renders without one. That a listener reads the session the change made, and
// that a throwing listener stops neither the listener after it nor the screen, is proven by
// "COSGN00 in the route table: a session listener that throws".
describe("COSGN00 session changes: subscribeSession and getSessionSnapshot", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  /** Registers `listener` for the current test only, and returns its remover. */
  function subscribe(listener: () => void): () => void {
    const unsubscribe = subscribeSession(listener);
    onTestFinished(unsubscribe);
    return unsubscribe;
  }

  /** The error a browser throws when it blocks storage. */
  function storageBlocked(): DOMException {
    return new DOMException("storage blocked", "SecurityError");
  }

  /** Presses F3 and checks that the session-end text replaced the screen. */
  function pressF3(): void {
    fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));
    expect(screen.getByText("Thank you for using CardDemo application...")).toBeTruthy();
  }

  /** Fires the window's `pageshow`: a restore from the back/forward cache when `persisted`. */
  function showPage(persisted: boolean): void {
    globalThis.dispatchEvent(new PageTransitionEvent("pageshow", { persisted }));
  }

  /** The handlers of the `pageshow` calls among `calls` to add or remove a listener, in order. */
  function pageshowHandlers(calls: readonly (readonly unknown[])[]): unknown[] {
    return calls.filter((call) => call[0] === "pageshow").map((call) => call[1]);
  }

  // Each change calls every listener once, a function subscribed twice is held and called once,
  // and an unsubscribed listener hears no later change while the others still do.
  it("notifies every listener once per entry and per sign-on, holds a function subscribed twice once, and stops calling one that unsubscribed", async () => {
    stubFetch({ status: 200, body: userSignon });
    const first = vi.fn();
    const second = vi.fn();
    const unsubscribeFirst = subscribe(first);
    subscribe(second);
    // The same function registered again is held once, so it is still called once per change.
    subscribe(second);

    // Entry ends the session (one clearSession), then the sign-on stores it (one setSession).
    const { unmount } = renderScreen();
    expect(first).toHaveBeenCalledTimes(1);
    expect(second).toHaveBeenCalledTimes(1);

    await signOnAsUser();
    expect(first).toHaveBeenCalledTimes(2);
    expect(second).toHaveBeenCalledTimes(2);

    // Unsubscribed, `first` hears neither the next entry nor the next sign-on; `second` hears both.
    unsubscribeFirst();
    unmount();
    renderScreen();
    expect(second).toHaveBeenCalledTimes(3);
    await signOnAsUser();
    expect(first).toHaveBeenCalledTimes(2);
    expect(second).toHaveBeenCalledTimes(4);
  });

  // The snapshot is the stored text itself, so repeated reads with no change between them are
  // identical, as `useSyncExternalStore` requires, and a different session gives a different one.
  it("getSessionSnapshot returns the identical stored text until the session changes, and null once entry ends it", async () => {
    stubFetch({ status: 200, body: userSignon });
    expect(getSessionSnapshot()).toBeNull();
    expect(getSessionSnapshot()).toBe(getSessionSnapshot());

    const { unmount } = renderScreen();
    // Stored after entry, which itself ends any session.
    setSession("test-token-earlier", "ADMIN001", "A");
    const earlier = getSessionSnapshot();
    expect(earlier).not.toBeNull();
    expect(earlier).toBe(sessionStorage.getItem(SESSION_KEY));
    expect(getSessionSnapshot()).toBe(earlier);

    await signOnAsUser();
    const signedOn = getSessionSnapshot();
    expect(signedOn).not.toBeNull();
    expect(signedOn).not.toBe(earlier);
    expect(signedOn).toBe(sessionStorage.getItem(SESSION_KEY));
    expect(getSessionSnapshot()).toBe(signedOn);

    unmount();
    renderScreen();
    expect(getSessionSnapshot()).toBeNull();
  });

  // A refused read is the same snapshot as no session, and never reaches the caller.
  it("getSessionSnapshot returns null, without throwing, when reading storage throws", async () => {
    stubFetch({ status: 200, body: userSignon });
    renderScreen();
    await signOnAsUser();
    expect(getSessionSnapshot()).not.toBeNull();
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw storageBlocked();
    });

    expect(() => getSessionSnapshot()).not.toThrow();
    expect(getSessionSnapshot()).toBeNull();
  });

  // A storage operation that throws still announces the change, once, and the screen still
  // finishes the key that made it.
  it.each([
    { method: "setItem", change: "setSession at sign-on", press: signOnAsUser },
    { method: "removeItem", change: "setSession at sign-on", press: signOnAsUser },
    { method: "removeItem", change: "clearSession at F3", press: pressF3 },
  ] as const)(
    "still notifies once when $method throws during $change",
    async ({ method, press }) => {
      stubFetch({ status: 200, body: userSignon });
      renderScreen();
      const listener = vi.fn();
      subscribe(listener);
      vi.spyOn(Storage.prototype, method).mockImplementation(() => {
        throw storageBlocked();
      });

      await press();

      expect(listener).toHaveBeenCalledTimes(1);
    },
  );

  // With no storage to reach, entry and sign-on still complete, nothing is held, and both changes
  // are still announced.
  it.each([
    {
      why: "sessionStorage does not exist",
      install: () => {
        vi.stubGlobal("sessionStorage", undefined);
      },
    },
    {
      why: "reaching sessionStorage throws",
      install: () => {
        // Stubbed first, so `vi.unstubAllGlobals` restores the original property afterwards.
        vi.stubGlobal("sessionStorage", undefined);
        Object.defineProperty(globalThis, "sessionStorage", {
          configurable: true,
          get(): Storage {
            throw storageBlocked();
          },
        });
      },
    },
  ])(
    "entry and sign-on complete, hold no session and still notify both changes when $why",
    async ({ install }) => {
      stubFetch({ status: 200, body: userSignon });
      const listener = vi.fn();
      subscribe(listener);
      install();

      renderScreen();
      await signOnAsUser();

      expect(getSession()).toBeNull();
      expect(getToken()).toBeNull();
      expect(getSessionSnapshot()).toBeNull();
      expect(listener).toHaveBeenCalledTimes(2);
    },
  );

  // A restore from the back/forward cache (`pageshow` with `persisted`) is announced as a session
  // change is: once to each listener, a function subscribed twice included, through one window
  // handler that the first listener adds and the last one's removal takes away. Any other
  // `pageshow` is a fresh load and announces nothing.
  it("announces each back/forward-cache restore once per listener through one window handler, which leaves with the last listener", () => {
    const added = vi.spyOn(globalThis, "addEventListener");
    const removed = vi.spyOn(globalThis, "removeEventListener");
    const first = vi.fn();
    const second = vi.fn();
    const unsubscribeFirst = subscribe(first);
    const unsubscribeSecond = subscribe(second);
    subscribe(second);
    const handlers = pageshowHandlers(added.mock.calls);
    expect(handlers).toHaveLength(1);

    showPage(false);
    expect(first).not.toHaveBeenCalled();
    expect(second).not.toHaveBeenCalled();

    showPage(true);
    expect(first).toHaveBeenCalledTimes(1);
    expect(second).toHaveBeenCalledTimes(1);

    // Another listener still holds the handler on the window.
    unsubscribeFirst();
    showPage(true);
    expect(first).toHaveBeenCalledTimes(1);
    expect(second).toHaveBeenCalledTimes(2);
    expect(pageshowHandlers(removed.mock.calls)).toEqual([]);

    unsubscribeSecond();
    expect(pageshowHandlers(removed.mock.calls)).toEqual(handlers);
    showPage(true);
    expect(second).toHaveBeenCalledTimes(2);
    expect(pageshowHandlers(added.mock.calls)).toEqual(handlers);
  });

  // With no window, or one that takes no listeners, a subscription adds no `pageshow` handler and
  // cannot throw: session changes are still announced, a restore cannot be, and unsubscribing
  // still removes the listener.
  it.each([
    { why: "there is no window", page: undefined },
    { why: "the window takes no event listeners", page: {} },
  ])("holds, notifies and removes a listener, with no pageshow handler, when $why", ({ page }) => {
    const added = vi.spyOn(globalThis, "addEventListener");
    vi.stubGlobal("window", page);
    const listener = vi.fn();
    const unsubscribe = subscribe(listener);

    setSession("test-token", "USER0001", "U");
    clearSession();
    expect(listener).toHaveBeenCalledTimes(2);
    showPage(true);
    expect(listener).toHaveBeenCalledTimes(2);

    expect(() => {
      unsubscribe();
    }).not.toThrow();
    setSession("test-token", "USER0001", "U");
    expect(listener).toHaveBeenCalledTimes(2);
    expect(pageshowHandlers(added.mock.calls)).toEqual([]);
  });
});
