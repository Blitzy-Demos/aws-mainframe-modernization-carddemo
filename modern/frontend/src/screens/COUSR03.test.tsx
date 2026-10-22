import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
import { clearSession, getSession, setSession } from "@/auth/session";
import { PATHS, routes } from "@/routes";
import { COUSR03 } from "./COUSR03";

/*
 * Render test of the user delete screen (map COUSR3A, transaction CU03, program COUSR03C, route
 * `/users/delete`) in jsdom, with `fetch` stubbed for endpoint 18, `GET /api/v1/users/{user_id}`
 * (the read COUSR02C shares), and endpoint 20, `DELETE /api/v1/users/{user_id}`. Expected values
 * are the audited results of `modern/audit/screens.md` (`COUSR03-nnn`) and
 * `modern/audit/online/COUSR03C.md` (`COUSR03C-nnn`). No response carries a password.
 */

const ENDPOINT = "/api/v1/users/";

/** The user a COUSR00 `D` selection hands over in route state (FIXUSR-006). */
const SELECTED_ID = "USER0001";
/** A user typed into the user ID input and fetched by ENTER (FIXUSR-001). */
const TYPED_ID = "ADMIN001";
/** A user typed and deleted by F5 with no fetch first (FIXUSR-007). */
const UNFETCHED_ID = "USER0002";
/** The selected ID in lower case: COUSR03C upper-cases nothing, so endpoint 18 finds no row. */
const UNKNOWN_ID = "user0001";
/** A user ID whose read and delete both fail on the server. */
const FAILING_ID = "LOCK0001";
/** A user ID whose delete finds no row. */
const VANISHED_ID = "GONE0001";
/** A user ID whose delete the admin gate refuses. */
const REFUSED_ID = "DENY0001";

/** The route state of a COUSR00 `D` selection of {@link SELECTED_ID}, as COUSR00 sends it. */
const FROM_LIST = { from: PATHS.COUSR00, user_id: SELECTED_ID };

/** COUSR03C's read prompt (COUSR03C-012). */
const DELETE_PROMPT = "Press PF5 key to delete this user ...";
/** COUSR02C's read prompt, which the delete screen never shows (COUSR02C-014). */
const UPDATE_PROMPT = "Press PF5 key to save your updates ...";

const selectedView: components["schemas"]["UserView"] = {
  user_id: SELECTED_ID,
  first_name: "LAWRENCE".padEnd(20, " "),
  last_name: "THOMAS".padEnd(20, " "),
  user_type: "U",
  delete_prompt: DELETE_PROMPT,
  update_prompt: UPDATE_PROMPT,
};

const typedView: components["schemas"]["UserView"] = {
  user_id: TYPED_ID,
  first_name: "MARGARET".padEnd(20, " "),
  last_name: "GOLD".padEnd(20, " "),
  user_type: "A",
  delete_prompt: DELETE_PROMPT,
  update_prompt: UPDATE_PROMPT,
};

/** COUSR03C-013 (COUSR02C-015): the read finds no row. */
const readNotFound: components["schemas"]["ScreenError"] = {
  message: "User ID NOT found...",
  field: "user_id",
};

/** COUSR03C-014 (COUSR02C-016): the read fails. */
const readFailed: components["schemas"]["ScreenError"] = {
  message: "Unable to lookup User...",
  field: "first_name",
};

/** COUSR03C-019: the delete of {@link SELECTED_ID} succeeds. */
const deletedSelected: components["schemas"]["ScreenResult"] = {
  message: "User USER0001 has been deleted ...",
};

/** COUSR03C-019: the delete of {@link UNFETCHED_ID} succeeds. */
const deletedUnfetched: components["schemas"]["ScreenResult"] = {
  message: "User USER0002 has been deleted ...",
};

/** COUSR03C-021: the delete affects no row. */
const deleteNotFound: components["schemas"]["ScreenError"] = {
  message: "User ID NOT found...",
  field: "user_id",
};

/** COUSR03C-022: the delete fails, in the program's own words (X-COUSR03C-1). */
const deleteFailed: components["schemas"]["ScreenError"] = {
  message: "Unable to Update User...",
  field: "first_name",
};

/** COUSR03C-002: the `AdminUser` extractor's 403, which names no field. */
const adminRequired: components["schemas"]["ScreenError"] = {
  message: "No access - Admin Only function ...",
};

/** Endpoint 18 and 20 replies by method and user ID; any other request gets a 500. */
const REPLIES: ReadonlyMap<string, { status: number; body: unknown }> = new Map([
  [`GET ${SELECTED_ID}`, { status: 200, body: selectedView }],
  [`GET ${TYPED_ID}`, { status: 200, body: typedView }],
  [`GET ${UNKNOWN_ID}`, { status: 404, body: readNotFound }],
  [`GET ${FAILING_ID}`, { status: 500, body: readFailed }],
  [`DELETE ${SELECTED_ID}`, { status: 200, body: deletedSelected }],
  [`DELETE ${UNFETCHED_ID}`, { status: 200, body: deletedUnfetched }],
  [`DELETE ${VANISHED_ID}`, { status: 404, body: deleteNotFound }],
  [`DELETE ${FAILING_ID}`, { status: 500, body: deleteFailed }],
  [`DELETE ${REFUSED_ID}`, { status: 403, body: adminRequired }],
]);

/** The fields {@link selectedView} shows. */
const SHOWN_SELECTED = {
  firstName: "LAWRENCE".padEnd(20, " "),
  lastName: "THOMAS".padEnd(20, " "),
  userType: "U",
};

/** The fields {@link typedView} shows. */
const SHOWN_TYPED = {
  firstName: "MARGARET".padEnd(20, " "),
  lastName: "GOLD".padEnd(20, " "),
  userType: "A",
};

/** Every output field blank: before a read, after a failed read and after a delete. */
const BLANK = { firstName: "", lastName: "", userType: "" };

/** `CCDA-MSG-INVALID-KEY`, the message of an F-key the screen does not serve (COUSR03C-009). */
const INVALID_KEY_TEXT = "Invalid key pressed. Please see below...";

/** The schema's blank-user-ID message (COUSR03C-010, COUSR03C-016). */
const USER_ID_EMPTY = "User ID can NOT be empty...";

/** The twelve F-keys a browser sends. */
const ALL_F_KEYS = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];

/** The F-keys COUSR03C serves under no branch but `WHEN OTHER`: all but F3, F4, F5 and F12. */
const UNSERVED_F_KEYS = ALL_F_KEYS.filter((key) => !["F3", "F4", "F5", "F12"].includes(key));

/** A JSON response, as the API sends every success and every `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces `fetch` with {@link REPLIES}. With `hold`, every reply waits until `release` is
 * called, so a test can act while a request is outstanding. Returns every request in the order
 * the screen sent it, and `release`, which lets every waiting reply go.
 */
function stubApi(hold = false): { requests: Request[]; release: () => void } {
  const requests: Request[] = [];
  const waiting: Array<() => void> = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      requests.push(req);
      if (hold) {
        await new Promise<void>((resolve) => {
          waiting.push(resolve);
        });
      }
      const { pathname } = new URL(req.url);
      const userId = pathname.startsWith(ENDPOINT)
        ? decodeURIComponent(pathname.slice(ENDPOINT.length))
        : undefined;
      const reply = userId === undefined ? undefined : REPLIES.get(`${req.method} ${userId}`);
      if (reply !== undefined) {
        return json(reply.status, reply.body);
      }
      const unexpected: components["schemas"]["ScreenError"] = {
        message: `Unexpected request ${req.method} ${pathname}`,
      };
      return json(500, unexpected);
    }),
  );
  return {
    requests,
    release: () => {
      for (const resolve of waiting.splice(0)) {
        resolve();
      }
    },
  };
}

/** Each request as `METHOD path`, in order. */
function sent(requests: readonly Request[]): string[] {
  return requests.map((request) => `${request.method} ${new URL(request.url).pathname}`);
}

/** A navigation target: shows the path it was reached at. */
function RouteStub() {
  const location = useLocation();
  return (
    <section aria-label="Route stub">
      <p data-testid="stub-path">{location.pathname}</p>
    </section>
  );
}

/** COUSR03 at `/users/delete` with `state`, and stubs at the routes it transfers to. */
function renderScreen(state?: unknown) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COUSR03, state }]}>
        <Routes>
          <Route path={PATHS.COUSR03} element={<COUSR03 />} />
          <Route path={PATHS.COUSR00} element={<RouteStub />} />
          <Route path={PATHS.COADM01} element={<RouteStub />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The path of the route stub a transfer reached. */
async function reachedPath(): Promise<string> {
  const stub = await screen.findByRole("region", { name: "Route stub" });
  return within(stub).getByTestId("stub-path").textContent ?? "";
}

/** The `<input>` the label `text` names, failing the test when it names anything else. */
function field(text: string): HTMLInputElement {
  const element = screen.getByLabelText(text);
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`"${text}" labels <${element.tagName.toLowerCase()}>, not an input`);
  }
  return element;
}

/** `USRIDIN`, the user ID input (`COUSR03-012`, `COUSR03-013`). */
function userIdInput(): HTMLInputElement {
  return field("Enter User ID:");
}

/** `FNAME`, `LNAME` and `USRTYPE`, the protected output fields. */
function outputs(): {
  firstName: HTMLInputElement;
  lastName: HTMLInputElement;
  userType: HTMLInputElement;
} {
  return {
    firstName: field("First Name:"),
    lastName: field("Last Name:"),
    userType: field("User Type:"),
  };
}

/** The values the three output fields show. */
function shown(): { firstName: string; lastName: string; userType: string } {
  const { firstName, lastName, userType } = outputs();
  return { firstName: firstName.value, lastName: lastName.value, userType: userType.value };
}

/** The screen's one message row: its `MessageLine` kind, its text and whether it is red. */
function messageRow(): { kind: string | undefined; text: string; red: boolean } {
  const rows = screen.getAllByRole("alert");
  expect(rows).toHaveLength(1);
  const [row] = rows;
  return {
    kind: row?.dataset.messageKind,
    text: row?.textContent ?? "",
    red: row?.classList.contains("text-destructive") ?? false,
  };
}

/** The message row as an `ERRMSG` text in the map's red (`destructive`). */
function errorRow(text: string): { kind: string; text: string; red: boolean } {
  return { kind: "error", text, red: true };
}

/** The message row as a neutral or green text (`default`). */
function infoRow(text: string): { kind: string; text: string; red: boolean } {
  return { kind: "info", text, red: false };
}

/** The empty message row, which stays an `ERRMSG` row. */
const EMPTY_ROW = { kind: "error", text: "", red: true };

/**
 * The error text `element` is marked with, or `null` when it is not marked. A marked control is
 * `aria-invalid` and its `aria-describedby` names one rendered `sr-only` element holding the text,
 * which is no message row: the message row stays the screen's one visible message.
 */
function markOf(element: HTMLElement): string | null {
  const hidden = (element.getAttribute("aria-describedby") ?? "")
    .split(/\s+/)
    .filter((id) => id !== "")
    .map((id) => document.getElementById(id))
    .filter((described) => described?.classList.contains("sr-only") === true);
  if (element.getAttribute("aria-invalid") !== "true") {
    expect(hidden).toEqual([]);
    return null;
  }
  expect(hidden).toHaveLength(1);
  const [message] = hidden;
  expect(message?.closest('[role="alert"]')).toBeNull();
  return message?.textContent ?? null;
}

/** The marks of the user ID input and the three output fields, `null` for each one unmarked. */
function marks(): {
  userId: string | null;
  firstName: string | null;
  lastName: string | null;
  userType: string | null;
} {
  const { firstName, lastName, userType } = outputs();
  return {
    userId: markOf(userIdInput()),
    firstName: markOf(firstName),
    lastName: markOf(lastName),
    userType: markOf(userType),
  };
}

/** No control marked. */
const UNMARKED = { userId: null, firstName: null, lastName: null, userType: null };

/** The text of the header field `name` (`data-field` of `ScreenShell`). */
function headerField(container: HTMLElement, name: string): string | null {
  return container.querySelector(`[data-field="${name}"]`)?.textContent ?? null;
}

/**
 * Asserts the header value fields of rows 1 and 2 against their rows of `modern/audit/screens.md`
 * (COUSR03-002, -003, -005, -007, -008 and -010). Each entry of `fields` is the BMS name, the `POS`
 * row and column, the `LENGTH`, the value shown, and whether the map colours the field YELLOW.
 *
 * The rows describe each value as a read-only `Input` of the field's width in the `CardHeader`
 * title rows of `ScreenShell`. `ScreenShell` realises it as read-only text in those title rows,
 * its recorded decision (the header rows belong to the frame's title rows, not to the map's input
 * or output fields). The assertions prove what the rows require of that text: it is read-only and
 * out of the tab order, as an `ASKIP` field is skipped; its grid cell is the field box at `POS`
 * with `LENGTH` as its width, and it shows exactly `LENGTH` characters; it stands in the
 * `CardHeader`, not in the body or the footer; and it carries the tokens of AAP 0.4.3: the
 * foreground with `font-semibold` for a YELLOW title, the foreground alone for a `NORM` value.
 */
function expectHeaderValues(
  root: ParentNode,
  fields: readonly {
    readonly name: string;
    readonly row: number;
    readonly col: number;
    readonly length: number;
    readonly value: string;
    readonly title: boolean;
  }[],
): void {
  const header = root.querySelector('[data-slot="card-header"]');
  if (!(header instanceof HTMLElement)) {
    throw new Error("the screen renders no CardHeader");
  }
  // The header exposes no textbox that takes typing.
  expect(
    within(header)
      .queryAllByRole("textbox")
      .filter((box) =>
        box instanceof HTMLInputElement || box instanceof HTMLTextAreaElement
          ? !box.readOnly
          : box.getAttribute("aria-readonly") !== "true",
      ),
  ).toEqual([]);
  for (const { name, row, col, length, value, title } of fields) {
    const found = root.querySelectorAll(`[data-field="${name}"]`);
    expect(found).toHaveLength(1);
    const element = found[0];
    if (!(element instanceof HTMLElement)) {
      throw new Error(`the screen renders no header field ${name}`);
    }
    // Placement: in the CardHeader title rows, not in the body or the footer.
    expect(element.closest('[data-slot="card-header"]')).toBe(header);
    expect(element.closest('[data-slot="card-content"], [data-slot="card-footer"]')).toBeNull();
    // The field box: the grid cell at POS, LENGTH columns wide, holding exactly LENGTH characters.
    expect(element.parentElement?.className).toBe(
      `row-start-${row} col-start-${col} col-span-${length}`,
    );
    const shown = element instanceof HTMLInputElement ? element.value : element.textContent;
    expect(shown).toBe(value);
    expect(shown).toHaveLength(length);
    // Read-only and skipped by Tab: neither the field nor anything inside it takes typing or is a
    // tab stop.
    const nodes = [element, ...Array.from(element.querySelectorAll("*"))];
    expect(
      nodes.filter(
        (node) =>
          node instanceof HTMLSelectElement ||
          ((node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement) &&
            !node.readOnly) ||
          node.closest('[contenteditable]:not([contenteditable="false"])') !== null,
      ),
    ).toEqual([]);
    expect(element.tabIndex).toBeLessThan(0);
    expect(
      nodes.filter(
        (node) => (node instanceof HTMLElement || node instanceof SVGElement) && node.tabIndex >= 0,
      ),
    ).toEqual([]);
    // Tokens: the foreground colour, with emphasis only on a YELLOW title.
    expect(element.classList.contains("text-foreground")).toBe(true);
    expect(element.classList.contains("font-semibold")).toBe(title);
  }
}

/** The grid cell, `row-start-R col-start-C col-span-S`, that holds `element`. */
function cellOf(element: HTMLElement): string {
  return element.parentElement?.className ?? "";
}

/** The row-24 button labelled `label`. */
function key(label: string): HTMLElement {
  return screen.getByRole("button", { name: label });
}

/** A keydown of `name` at the focused element, as the browser sends it; it bubbles to `window`. */
function pressKey(name: string, shiftKey = false): void {
  fireEvent.keyDown(document.activeElement ?? document.body, { key: name, shiftKey });
}

/**
 * Moves the cursor to `element`, so that its move to another field is observable. The move runs
 * inside `waitFor`, which lets React render the blur it causes.
 */
async function moveFocus(element: HTMLElement): Promise<void> {
  await waitFor(() => {
    element.focus();
    expect(document.activeElement).toBe(element);
  });
}

/** Types `value` into the user ID input. */
function typeUserId(value: string): void {
  fireEvent.change(userIdInput(), { target: { value } });
}

/**
 * Lets pending effects, mutations and stubbed responses run, so that a request the screen would
 * send has been sent before a test counts the requests. The pause runs inside `waitFor`, which
 * lets React render the updates those responses cause.
 */
async function settle(): Promise<void> {
  await waitFor(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, 20);
    });
  });
}

/**
 * Renders the screen after a COUSR00 `D` selection, waits for the entry fetch to show, and
 * returns every request sent so far and after.
 */
async function renderSelected(): Promise<Request[]> {
  const { requests } = stubApi();
  renderScreen(FROM_LIST);
  await waitFor(() => {
    expect(shown()).toEqual(SHOWN_SELECTED);
  });
  return requests;
}

beforeEach(() => {
  setSession("test-token", "ADMIN001", "A");
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COUSR03 user delete screen", () => {
  // audit: COUSR03-011
  // audit: COUSR03-025
  // audit: COUSR03-026
  // audit: COUSR03-037
  it("renders the map: title, user ID input, read-only outputs, message row, key row", () => {
    stubApi();
    renderScreen({ from: PATHS.COADM01 });

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Delete User");

    const userId = userIdInput();
    expect(userId.maxLength).toBe(8);
    expect(userId.size).toBe(8);
    expect(userId.readOnly).toBe(false);
    expect(userId.hasAttribute("readonly")).toBe(false);

    const { firstName, lastName, userType } = outputs();
    for (const [output, width] of [
      [firstName, 20],
      [lastName, 20],
      [userType, 1],
    ] as const) {
      expect(output.readOnly).toBe(true);
      expect(output.hasAttribute("readonly")).toBe(true);
      expect(output.maxLength).toBe(width);
      expect(output.size).toBe(width);
      expect(output.value).toBe("");
    }
    // The map has no password field, and the screen shows or asks for none.
    expect(document.querySelector('input[type="password"]')).toBeNull();

    // The map's four labels in map order, then F12, which the map does not label.
    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(
      keys.map((button) => [
        button.textContent,
        button.getAttribute("type"),
        button.dataset.variant,
        button.dataset.size,
        button.hidden,
      ]),
    ).toEqual([
      ["ENTER=Fetch", "submit", "default", "sm", false],
      ["F3=Back", "button", "outline", "sm", false],
      ["F4=Clear", "button", "outline", "sm", false],
      ["F5=Delete", "button", "outline", "sm", false],
      ["F12", "button", "outline", "sm", false],
    ]);

    // ERRMSG is the map's only message row: a red, polite live region, empty at first display.
    const rows = screen.getAllByRole("alert");
    expect(rows).toHaveLength(1);
    expect(rows[0]?.getAttribute("aria-live")).toBe("polite");
    expect(messageRow()).toEqual(EMPTY_ROW);
  });

  // audit: COUSR03-012
  // audit: COUSR03-014
  // audit: COUSR03-015
  // audit: COUSR03-016
  // audit: COUSR03-018
  // audit: COUSR03-019
  // audit: COUSR03-021
  // audit: COUSR03-022
  // audit: COUSR03-024
  it("places each literal and field at its map POS and LENGTH and binds each label", () => {
    stubApi();
    renderScreen();

    // Each label names its field (`getByLabelText`), and both sit at their declared cells.
    expect(cellOf(screen.getByText("Enter User ID:"))).toBe("row-start-6 col-start-6 col-span-14");
    expect(cellOf(userIdInput())).toBe("row-start-6 col-start-21 col-span-8");
    expect(cellOf(screen.getByText("First Name:"))).toBe("row-start-11 col-start-6 col-span-11");
    expect(cellOf(field("First Name:"))).toBe("row-start-11 col-start-18 col-span-20");
    expect(cellOf(screen.getByText("Last Name:"))).toBe("row-start-13 col-start-6 col-span-10");
    expect(cellOf(field("Last Name:"))).toBe("row-start-13 col-start-18 col-span-20");
    // `User Type: ` keeps its trailing space; USRTYPE starts at its own column (X-SCREENS-3).
    const userTypeLabel = screen.getByText("User Type:");
    expect(userTypeLabel.textContent).toBe("User Type: ");
    expect(cellOf(userTypeLabel)).toBe("row-start-15 col-start-6 col-span-11");
    expect(cellOf(field("User Type:"))).toBe("row-start-15 col-start-17 col-span-1");

    const hint = screen.getByText("(A=Admin, U=User)");
    expect(hint.textContent).toBe("(A=Admin, U=User)");
    expect(cellOf(hint)).toBe("row-start-15 col-start-19 col-span-17");

    // The YELLOW rule of row 8: seventy asterisks, decorative and so hidden from assistive tech.
    const rule = screen.getByText("*".repeat(70));
    expect(rule.getAttribute("aria-hidden")).toBe("true");
    expect(cellOf(rule)).toBe("row-start-8 col-start-6 col-span-70");

    // The zero-length stoppers that end USRIDIN, FNAME and LNAME render nothing.
    expect(document.querySelector(".row-start-6.col-start-30")).toBeNull();
    expect(document.querySelector(".row-start-11.col-start-39")).toBeNull();
    expect(document.querySelector(".row-start-13.col-start-39")).toBeNull();
  });

  // audit: COUSR03C-026
  // audit: COUSR03-027
  // audit: COUSR03-028
  // audit: COUSR03-029
  // audit: COUSR03-030
  // audit: COUSR03-031
  // audit: COUSR03-032
  it("fills header rows 1 and 2 at every send: CU03, COUSR03C, titles, date, time", async () => {
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
    stubApi();
    const { container } = renderScreen({ from: PATHS.COADM01 });

    expect(headerField(container, "TRNNAME")).toBe("CU03");
    expect(headerField(container, "PGMNAME")).toBe("COUSR03C");
    expect(headerField(container, "TITLE01")).toBe("      AWS Mainframe Modernization       ");
    expect(headerField(container, "TITLE02")).toBe("              CardDemo                  ");
    expect(headerField(container, "CURDATE")).toBe("01/02/24");
    expect(headerField(container, "CURTIME")).toBe("03:04:05");

    vi.setSystemTime(new Date(2025, 11, 31, 23, 59, 58));
    typeUserId(TYPED_ID);
    fireEvent.click(key("ENTER=Fetch"));
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_TYPED);
    });
    expect(headerField(container, "CURDATE")).toBe("12/31/25");
    expect(headerField(container, "CURTIME")).toBe("23:59:58");
    expect(headerField(container, "TRNNAME")).toBe("CU03");
    expect(headerField(container, "PGMNAME")).toBe("COUSR03C");
  });

  // audit: COUSR03-002
  // audit: COUSR03-003
  // audit: COUSR03-005
  // audit: COUSR03-007
  // audit: COUSR03-008
  // audit: COUSR03-010
  it("shows the six header values as read-only text in the CardHeader title rows, each at its POS and LENGTH", async () => {
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
    const { requests } = stubApi();
    const { container } = renderScreen({ from: PATHS.COADM01 });

    expectHeaderValues(container, [
      { name: "TRNNAME", row: 1, col: 7, length: 4, value: "CU03", title: false },
      {
        name: "TITLE01",
        row: 1,
        col: 21,
        length: 40,
        value: "      AWS Mainframe Modernization       ",
        title: true,
      },
      { name: "CURDATE", row: 1, col: 71, length: 8, value: "01/02/24", title: false },
      { name: "PGMNAME", row: 2, col: 7, length: 8, value: "COUSR03C", title: false },
      {
        name: "TITLE02",
        row: 2,
        col: 21,
        length: 40,
        value: "              CardDemo                  ",
        title: true,
      },
      { name: "CURTIME", row: 2, col: 71, length: 8, value: "03:04:05", title: false },
    ]);
    await settle();
    expect(requests).toEqual([]);
  });

  // audit: COUSR03C-005
  it.each([
    { title: "from the admin menu", state: { from: PATHS.COADM01 } },
    { title: "with no route state", state: undefined },
    { title: "with route state that is not an object", state: "USER0001" },
    { title: "with an all-space selected user ID", state: { from: PATHS.COUSR00, user_id: "  " } },
    { title: "with a selected user ID that is not a string", state: { user_id: 1 } },
  ])("first display $title: everything empty, cursor in user ID, no request", async (row) => {
    const { requests } = stubApi();
    renderScreen(row.state);

    await waitFor(() => {
      expect(document.activeElement).toBe(userIdInput());
    });
    expect(userIdInput().value).toBe("");
    expect(shown()).toEqual(BLANK);
    expect(messageRow()).toEqual(EMPTY_ROW);
    await settle();
    expect(requests).toHaveLength(0);
  });

  // audit: COUSR03-017
  // audit: COUSR03-020
  // audit: COUSR03-023
  // audit: COUSR03C-004
  // audit: COUSR03C-011
  // audit: COUSR03C-012
  it("after a COUSR00 selection fetches the user at once, shows it and the prompt", async () => {
    const { requests } = stubApi();
    renderScreen(FROM_LIST);

    expect(userIdInput().value).toBe(SELECTED_ID);
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_SELECTED);
    });
    // COUSR03C's own prompt, never COUSR02C's, in the neutral colour.
    expect(messageRow()).toEqual(infoRow(DELETE_PROMPT));
    expect(screen.queryByText(UPDATE_PROMPT)).toBeNull();
    expect(userIdInput().value).toBe(SELECTED_ID);
    // The values sit in protected fields of the map's widths.
    const { firstName, lastName, userType } = outputs();
    const protectedWidths = [firstName, lastName, userType].map((output) => [
      output.readOnly,
      output.size,
    ]);
    expect(protectedWidths).toEqual([
      [true, 20],
      [true, 20],
      [true, 1],
    ]);
    await waitFor(() => {
      expect(document.activeElement).toBe(userIdInput());
    });

    // One read, and no delete: the entry sends endpoint 18 exactly one request.
    await settle();
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`]);
    expect(requests[0]?.headers.get("Authorization")).toBe("Bearer test-token");
  });

  // audit: COUSR03-033
  // audit: COUSR03C-011
  it.each([
    { title: "a click on ENTER=Fetch", submit: () => fireEvent.click(key("ENTER=Fetch")) },
    {
      title: "submitting the form",
      submit: () => {
        const form = userIdInput().closest("form");
        if (form === null) {
          throw new Error("the user ID input is outside a form");
        }
        fireEvent.submit(form);
      },
    },
  ])("ENTER by $title fetches the typed user ID with no route state", async (row) => {
    const { requests } = stubApi();
    renderScreen();
    await settle();
    expect(requests).toHaveLength(0);

    typeUserId(TYPED_ID);
    await moveFocus(key("F4=Clear"));
    row.submit();

    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_TYPED);
    });
    expect(messageRow()).toEqual(infoRow(DELETE_PROMPT));
    expect(userIdInput().value).toBe(TYPED_ID);
    await waitFor(() => {
      expect(document.activeElement).toBe(userIdInput());
    });
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${TYPED_ID}`]);
  });

  // audit: COUSR03C-015
  it.each([
    { title: "the 404 of COUSR03C-013", userId: UNKNOWN_ID, error: readNotFound, cursor: "user" },
    { title: "the 500 of COUSR03C-014", userId: FAILING_ID, error: readFailed, cursor: "first" },
  ])("ENTER answered by $title blanks the outputs and shows it in red", async (row) => {
    const requests = await renderSelected();

    typeUserId(row.userId);
    await moveFocus(key("F4=Clear"));
    fireEvent.click(key("ENTER=Fetch"));

    await waitFor(() => {
      expect(messageRow()).toEqual(errorRow(row.error.message));
    });
    expect(shown()).toEqual(BLANK);
    expect(userIdInput().value).toBe(row.userId);
    const cursor = row.cursor === "user" ? userIdInput() : field("First Name:");
    await waitFor(() => {
      expect(document.activeElement).toBe(cursor);
    });
    // The red error marks the control it names, the user ID input or the protected first name,
    // with its message for assistive technology only.
    expect(marks()).toEqual({
      ...UNMARKED,
      [row.cursor === "user" ? "userId" : "firstName"]: row.error.message,
    });
    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `GET ${ENDPOINT}${row.userId}`,
    ]);
  });

  // audit: COUSR03C-015
  // COUSR03C-010 is a `both` row: the schema's half is proved in src/schemas/COUSR03.test.ts and
  // the server's in the endpoint tests. This shows what the screen does with the schema's refusal.
  it("ENTER with a blank user ID sends nothing, keeps the outputs, shows it in red", async () => {
    const requests = await renderSelected();

    typeUserId("        ");
    await moveFocus(key("F4=Clear"));
    fireEvent.click(key("ENTER=Fetch"));

    await waitFor(() => {
      expect(messageRow()).toEqual(errorRow(USER_ID_EMPTY));
    });
    expect(shown()).toEqual(SHOWN_SELECTED);
    expect(userIdInput().value).toBe("        ");
    await waitFor(() => {
      expect(document.activeElement).toBe(userIdInput());
    });
    await settle();
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`]);
  });

  // audit: COUSR03-036
  // audit: COUSR03C-023
  // audit: COUSR03C-025
  it.each([
    { title: "a click on F5=Delete", byKey: false },
    { title: "the F5 key", byKey: true },
  ])("$title deletes the fetched user, empties every field, shows the result", async (row) => {
    const requests = await renderSelected();

    await moveFocus(key("F3=Back"));
    if (row.byKey) {
      pressKey("F5");
    } else {
      fireEvent.click(key("F5=Delete"));
    }

    await waitFor(() => {
      expect(messageRow()).toEqual(infoRow(deletedSelected.message));
    });
    expect(userIdInput().value).toBe("");
    expect(shown()).toEqual(BLANK);
    await waitFor(() => {
      expect(document.activeElement).toBe(userIdInput());
    });
    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `DELETE ${ENDPOINT}${SELECTED_ID}`,
    ]);
    // The path names the user; the delete carries no body.
    expect(await requests[1]?.clone().text()).toBe("");
    expect(requests[1]?.headers.get("Authorization")).toBe("Bearer test-token");
  });

  // audit: COUSR03C-023
  it("F5 deletes the typed user ID at once, with no fetch and no confirmation first", async () => {
    const { requests } = stubApi();
    renderScreen({ from: PATHS.COADM01 });

    typeUserId(UNFETCHED_ID);
    fireEvent.click(key("F5=Delete"));

    await waitFor(() => {
      expect(messageRow()).toEqual(infoRow(deletedUnfetched.message));
    });
    expect(userIdInput().value).toBe("");
    expect(shown()).toEqual(BLANK);
    expect(sent(requests)).toEqual([`DELETE ${ENDPOINT}${UNFETCHED_ID}`]);
  });

  // audit: COUSR03C-023
  // audit: COUSR03C-025
  // The delete's `User ID NOT found...` and `Unable to Update User...` keep the neutral colour the
  // read before the delete set (X-COUSR03C-2), so they show in the `default` (info) variant, not
  // in the red error row the scenario list names; every other refusal shows in red.
  it.each([
    {
      title: "the 500 Unable to Update User... of COUSR03C-022 in the neutral colour",
      userId: FAILING_ID,
      expected: infoRow(deleteFailed.message),
      cursor: "first",
    },
    {
      title: "the 404 User ID NOT found... of COUSR03C-021 in the neutral colour",
      userId: VANISHED_ID,
      expected: infoRow(deleteNotFound.message),
      cursor: "user",
    },
    {
      title: "the admin gate's 403 in red",
      userId: REFUSED_ID,
      expected: errorRow(adminRequired.message),
      cursor: "user",
    },
  ])("F5 answered by $title keeps every field", async (row) => {
    const requests = await renderSelected();

    // F5 deletes the user ID as it stands, whatever the outputs still show.
    typeUserId(row.userId);
    await moveFocus(key("F3=Back"));
    fireEvent.click(key("F5=Delete"));

    await waitFor(() => {
      expect(messageRow()).toEqual(row.expected);
    });
    expect(shown()).toEqual(SHOWN_SELECTED);
    expect(userIdInput().value).toBe(row.userId);
    const cursor = row.cursor === "user" ? userIdInput() : field("First Name:");
    await waitFor(() => {
      expect(document.activeElement).toBe(cursor);
    });
    // A neutral failure only moves the cursor, and the 403 names no field: nothing is marked.
    expect(marks()).toEqual(UNMARKED);
    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `DELETE ${ENDPOINT}${row.userId}`,
    ]);
  });

  // audit: COUSR03C-025
  // COUSR03C-016 is a `both` row: the schema's half is proved in src/schemas/COUSR03.test.ts.
  it("F5 with a blank user ID sends nothing, keeps every field, shows it in red", async () => {
    const requests = await renderSelected();

    typeUserId("");
    await moveFocus(key("F3=Back"));
    fireEvent.click(key("F5=Delete"));

    await waitFor(() => {
      expect(messageRow()).toEqual(errorRow(USER_ID_EMPTY));
    });
    expect(shown()).toEqual(SHOWN_SELECTED);
    expect(userIdInput().value).toBe("");
    await waitFor(() => {
      expect(document.activeElement).toBe(userIdInput());
    });
    await settle();
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`]);
  });

  // The screen's keyboard lock: while a request is outstanding ENTER, F4 and F5 do nothing, as the
  // 3270 keyboard stays locked until the program answers.
  it("ignores ENTER, F4 and F5 while the fetch or the delete is outstanding", async () => {
    const { requests, release } = stubApi(true);
    renderScreen(FROM_LIST);
    const form = userIdInput().closest("form");
    await waitFor(() => {
      expect(form?.getAttribute("aria-busy")).toBe("true");
    });

    fireEvent.click(key("F5=Delete"));
    fireEvent.click(key("F4=Clear"));
    fireEvent.click(key("ENTER=Fetch"));
    await settle();
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`]);
    expect(userIdInput().value).toBe(SELECTED_ID);

    release();
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_SELECTED);
    });
    await waitFor(() => {
      expect(form?.getAttribute("aria-busy")).toBe("false");
    });

    fireEvent.click(key("F5=Delete"));
    await waitFor(() => {
      expect(form?.getAttribute("aria-busy")).toBe("true");
    });
    pressKey("F5");
    fireEvent.click(key("F4=Clear"));
    fireEvent.click(key("ENTER=Fetch"));
    await settle();
    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `DELETE ${ENDPOINT}${SELECTED_ID}`,
    ]);
    expect(shown()).toEqual(SHOWN_SELECTED);
    expect(messageRow()).toEqual(infoRow(DELETE_PROMPT));

    release();
    await waitFor(() => {
      expect(messageRow()).toEqual(infoRow(deletedSelected.message));
    });
    expect(shown()).toEqual(BLANK);
    await settle();
    expect(requests).toHaveLength(2);
  });

  // audit: COUSR03-035
  // audit: COUSR03C-007
  it("F4 empties the user ID, outputs and message row, refocuses, sends nothing", async () => {
    const requests = await renderSelected();
    expect(messageRow()).toEqual(infoRow(DELETE_PROMPT));

    // F4=Clear by its button, over a shown user, with the cursor away from the input.
    const clear = key("F4=Clear");
    await moveFocus(clear);
    fireEvent.click(clear);
    await waitFor(() => {
      expect(shown()).toEqual(BLANK);
    });
    expect(userIdInput().value).toBe("");
    expect(messageRow()).toEqual(EMPTY_ROW);
    expect(document.activeElement).toBe(userIdInput());

    // The F4 key, over an error message.
    typeUserId(UNKNOWN_ID);
    fireEvent.click(key("ENTER=Fetch"));
    await waitFor(() => {
      expect(messageRow()).toEqual(errorRow(readNotFound.message));
    });
    pressKey("F4");
    await waitFor(() => {
      expect(messageRow()).toEqual(EMPTY_ROW);
    });
    expect(userIdInput().value).toBe("");
    expect(shown()).toEqual(BLANK);
    expect(document.activeElement).toBe(userIdInput());

    await settle();
    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `GET ${ENDPOINT}${UNKNOWN_ID}`,
    ]);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Delete User");
  });

  // audit: COUSR03-034
  // audit: COUSR03C-006
  it.each([
    {
      title: "F3=Back after a COUSR00 selection returns to /users",
      state: FROM_LIST,
      byKey: false,
      target: "/users",
    },
    {
      title: "the F3 key after a COUSR00 selection returns to /users",
      state: FROM_LIST,
      byKey: true,
      target: "/users",
    },
    {
      title: "F3=Back after entry from the admin menu returns to /admin",
      state: { from: PATHS.COADM01 },
      byKey: false,
      target: "/admin",
    },
    {
      title: "F3=Back with no recorded origin returns to /admin",
      state: undefined,
      byKey: false,
      target: "/admin",
    },
    {
      title: "F3=Back with an origin that is no route of the application returns to /admin",
      state: { from: "https://example.invalid/users", user_id: SELECTED_ID },
      byKey: false,
      target: "/admin",
    },
  ])("$title, deleting nothing and sending no request", async (row) => {
    const { requests } = stubApi();
    renderScreen(row.state);
    // The entry's outcome first, so that only the key's effect remains to observe.
    await settle();
    const before = requests.length;

    if (row.byKey) {
      pressKey("F3");
    } else {
      fireEvent.click(key("F3=Back"));
    }

    expect(await reachedPath()).toBe(row.target);
    await settle();
    expect(requests).toHaveLength(before);
    expect(sent(requests).some((request) => request.startsWith("DELETE"))).toBe(false);
  });

  // audit: COUSR03-037
  // audit: COUSR03C-008
  it.each([
    { title: "the F12 button after a COUSR00 selection", state: FROM_LIST, byKey: false },
    { title: "the F12 key after a COUSR00 selection", state: FROM_LIST, byKey: true },
    { title: "the F12 button with no recorded origin", state: undefined, byKey: false },
  ])("$title goes to /admin, deleting nothing and sending no request", async (row) => {
    const { requests } = stubApi();
    renderScreen(row.state);
    await settle();
    const before = requests.length;

    if (row.byKey) {
      pressKey("F12");
    } else {
      fireEvent.click(key("F12"));
    }

    expect(await reachedPath()).toBe("/admin");
    await settle();
    expect(requests).toHaveLength(before);
    expect(sent(requests).some((request) => request.startsWith("DELETE"))).toBe(false);
  });

  // audit: COUSR03C-009
  it.each([
    ...UNSERVED_F_KEYS.map((name) => ({ title: name, name, shiftKey: false })),
    // PF13 to PF24: COUSR03C does not fold them onto PF1 to PF12, so Shift+F5 is no F5.
    ...ALL_F_KEYS.map((name) => ({ title: `Shift+${name}`, name, shiftKey: true })),
  ])("$title shows the invalid-key message in red and keeps every field", async (row) => {
    const requests = await renderSelected();
    typeUserId(TYPED_ID);

    pressKey(row.name, row.shiftKey);

    await waitFor(() => {
      expect(messageRow()).toEqual(errorRow(INVALID_KEY_TEXT));
    });
    expect(userIdInput().value).toBe(TYPED_ID);
    expect(shown()).toEqual(SHOWN_SELECTED);
    expect(screen.queryByRole("region", { name: "Route stub" })).toBeNull();
    await settle();
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`]);
  });

  it.each([
    {
      title: "an invalid key",
      act: () => {
        pressKey("F2");
      },
      row: errorRow(INVALID_KEY_TEXT),
      marked: UNMARKED,
    },
    {
      title: "F4",
      act: () => {
        fireEvent.click(key("F4=Clear"));
      },
      row: EMPTY_ROW,
      marked: UNMARKED,
    },
    {
      title: "an ENTER that finds the user",
      act: () => {
        typeUserId(TYPED_ID);
        fireEvent.click(key("ENTER=Fetch"));
      },
      row: infoRow(DELETE_PROMPT),
      marked: UNMARKED,
    },
    {
      title: "an F5 the admin gate refuses with no field",
      act: () => {
        typeUserId(REFUSED_ID);
        fireEvent.click(key("F5=Delete"));
      },
      row: errorRow(adminRequired.message),
      marked: UNMARKED,
    },
    {
      title: "an ENTER the blank-user-ID edit refuses",
      act: () => {
        typeUserId("");
        fireEvent.click(key("ENTER=Fetch"));
      },
      row: errorRow(USER_ID_EMPTY),
      marked: { ...UNMARKED, userId: USER_ID_EMPTY },
    },
  ])(
    "the next action, $title, takes the mark off the output a red error named",
    async ({ act, row, marked }) => {
      await renderSelected();
      typeUserId(FAILING_ID);
      fireEvent.click(key("ENTER=Fetch"));
      await waitFor(() => {
        expect(messageRow()).toEqual(errorRow(readFailed.message));
      });
      expect(marks()).toEqual({ ...UNMARKED, firstName: readFailed.message });

      act();

      await waitFor(() => {
        expect(messageRow()).toEqual(row);
      });
      expect(marks()).toEqual(marked);
    },
  );

  // audit: COUSR03C-006
  it("keeps its origin across ENTER, F4, F5 and invalid keys: F3 returns to COUSR00", async () => {
    const requests = await renderSelected();

    typeUserId(TYPED_ID);
    fireEvent.click(key("ENTER=Fetch"));
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_TYPED);
    });
    fireEvent.click(key("F4=Clear"));
    await waitFor(() => {
      expect(shown()).toEqual(BLANK);
    });
    typeUserId(UNFETCHED_ID);
    fireEvent.click(key("F5=Delete"));
    await waitFor(() => {
      expect(messageRow()).toEqual(infoRow(deletedUnfetched.message));
    });
    pressKey("F7");
    await waitFor(() => {
      expect(messageRow()).toEqual(errorRow(INVALID_KEY_TEXT));
    });

    fireEvent.click(key("F3=Back"));

    expect(await reachedPath()).toBe("/users");
    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `GET ${ENDPOINT}${TYPED_ID}`,
      `DELETE ${ENDPOINT}${UNFETCHED_ID}`,
    ]);
  });
});

// COUSR03C entered with no COMMAREA (`EIBCALEN = 0`) transfers to COSGN00C. The replacement is
// the route guard that only the application's route table mounts in front of `/users/delete`, so
// this group renders that table rather than the screen alone, with nobody signed on. The file's
// `afterEach` removes the `fetch` stub and the session.
describe("COUSR03 /users/delete opened with no session, through the route table", () => {
  beforeEach(() => {
    clearSession();
  });

  // audit: COUSR03C-003
  it("the route guard sends a COUSR00 selection to /signon before the entry fetch, with no request", async () => {
    // This case covers the row's no-session condition through the route table: the real routes
    // and their `RequireSignon` guard, not a stub of either. The row's 401 condition is covered
    // by the group "with a request answered 401" below. Entered with a COUSR00 `D` selection, the
    // screen itself fetches the selected user at once, so the empty request list shows that the
    // guard stops that fetch. The guard only hides the screen; the API's admin gate is the
    // control.
    const { requests } = stubApi();
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, {
      initialEntries: [{ pathname: PATHS.COUSR03, state: FROM_LIST }],
    });
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );

    // Both screens load lazily, through the route tree, before the sign-on screen renders.
    await waitFor(
      () => {
        expect(router.state.location.pathname).toBe(PATHS.COSGN00);
        expect(screen.getByRole("button", { name: "ENTER=Sign-on" })).toBeTruthy();
      },
      { timeout: 4000 },
    );
    expect(screen.queryByRole("heading", { level: 1, name: "Delete User" })).toBeNull();
    for (const label of ["Enter User ID:", "First Name:", "Last Name:", "User Type:"]) {
      expect(screen.queryByLabelText(label)).toBeNull();
    }
    expect(screen.queryByRole("button", { name: "F5=Delete" })).toBeNull();

    await settle();
    expect(sent(requests)).toEqual([]);
    router.dispose();
  });
});

// The row's other condition: a request of the screen answered 401, the API's form of a token that
// no longer verifies. `api/client.ts` clears the session on it, and the `RequireSignon` guard the
// route table mounts, which follows the session, replaces the mounted screen with `/signon`, as
// the XCTL to COSGN00C does. The file's `beforeEach` signs ADMIN001 on, so every request carries
// a token; its `afterEach` removes the `fetch` stub and the session.
describe("COUSR03 /users/delete with a request answered 401, through the route table", () => {
  /** A 401 for a token that no longer verifies: the API's own text, naming no field. */
  const signonRequired: components["schemas"]["ScreenError"] = {
    message: "Sign-on required. Please sign on ...",
  };

  /**
   * Replaces `fetch`: every request is answered 401 with {@link signonRequired}, except, when
   * `entryServed`, the entry fetch of {@link SELECTED_ID}, which gets {@link selectedView}.
   * Returns every request in the order the screen sent it.
   */
  function stubSignonRequired(entryServed: boolean): Request[] {
    const requests: Request[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const req = input instanceof Request ? input : new Request(input, init);
        requests.push(req);
        const { pathname } = new URL(req.url);
        if (entryServed && req.method === "GET" && pathname === `${ENDPOINT}${SELECTED_ID}`) {
          return json(200, selectedView);
        }
        return json(401, signonRequired);
      }),
    );
    return requests;
  }

  /** The route table at `/users/delete`, entered with a COUSR00 `D` selection. */
  function renderRoutes(): ReturnType<typeof createMemoryRouter> {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, {
      initialEntries: [{ pathname: PATHS.COUSR03, state: FROM_LIST }],
    });
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
    return router;
  }

  /**
   * Waits until the browser is at `/signon` and the sign-on screen, loaded lazily through the
   * route tree, shows; then asserts that the delete screen is gone and the session ended.
   */
  async function signonReached(router: ReturnType<typeof createMemoryRouter>): Promise<void> {
    await waitFor(
      () => {
        expect(router.state.location.pathname).toBe(PATHS.COSGN00);
        expect(screen.getByRole("button", { name: "ENTER=Sign-on" })).toBeTruthy();
      },
      { timeout: 4000 },
    );
    expect(screen.queryByRole("heading", { level: 1, name: "Delete User" })).toBeNull();
    for (const label of ["Enter User ID:", "First Name:", "Last Name:", "User Type:"]) {
      expect(screen.queryByLabelText(label)).toBeNull();
    }
    expect(screen.queryByRole("button", { name: "F5=Delete" })).toBeNull();
    expect(getSession()).toBeNull();
  }

  // audit: COUSR03C-003
  it("the entry fetch answered 401 takes the screen to /signon and ends the session", async () => {
    const requests = stubSignonRequired(false);
    const router = renderRoutes();

    await signonReached(router);
    // The guard let the screen mount with the session, so the screen's own entry fetch went out
    // with the token; its 401 alone ended the session, and nothing was sent after it.
    await settle();
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`]);
    expect(requests[0]?.headers.get("Authorization")).toBe("Bearer test-token");
    router.dispose();
  });

  // audit: COUSR03C-003
  it("F5=Delete answered 401 takes the screen to /signon and ends the session", async () => {
    const requests = stubSignonRequired(true);
    const router = renderRoutes();
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_SELECTED);
    });
    expect(screen.getByRole("heading", { level: 1, name: "Delete User" })).toBeTruthy();

    fireEvent.click(key("F5=Delete"));

    await signonReached(router);
    await settle();
    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `DELETE ${ENDPOINT}${SELECTED_ID}`,
    ]);
    expect(requests[1]?.headers.get("Authorization")).toBe("Bearer test-token");
    router.dispose();
  });
});
