import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ComponentType } from "react";
import {
  createMemoryRouter,
  MemoryRouter,
  Route,
  RouterProvider,
  Routes,
  useLocation,
  type RouteObject,
} from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { components } from "@/api/schema";
import { clearSession, getSession, setSession } from "@/auth/session";
import { PATHS, routes, ScreenOutlet } from "@/routes";
import { COUSR00 } from "./COUSR00";

/**
 * Render test of map `COUSR0A` (transaction CU00, program COUSR00C) at `/users`, run in jsdom with
 * the global `fetch` stubbed, so no request leaves the test (Local-Only Runtime). Endpoint 16,
 * `GET /api/v1/users`, is the only operation the screen calls; any other request is answered 500
 * and recorded, and every test asserts how many requests were sent.
 *
 * Expected results: `modern/audit/online/COUSR00C.md` and the `COUSR00-nnn` rows of
 * `modern/audit/screens.md`. `UserListRow` has no password member and the names are invented, so
 * no fixture carries a password or cardholder data (Cardholder Data Redaction).
 */

const TOKEN = "test-token";
const LIST_PATH = "/api/v1/users";

/**
 * The classes of the shared map-field box, FieldGrid's `FIELD_INPUT`: `min-w-max` and
 * `field-sizing-content` keep a box never narrower than its value, `px-1` is the one inset every
 * map input shares and `text-sm` the inputs' type size. Restated here because the render-test
 * contract imports no module beside the screen, the session, the routes and the generated types,
 * so a box is checked against this list rather than against the constant that renders it.
 */
const MAP_FIELD_BOX = ["min-w-max", "px-1", "text-sm", "field-sizing-content"] as const;

/** The messages of COUSR00C and of the API, as the audit and the endpoint word them. */
const INVALID_SELECTION = "Invalid selection. Valid values are U and D";
const BOTTOM_REACHED = "You have reached the bottom of the page...";
const TOP_REACHED = "You have reached the top of the page...";
const ALREADY_BOTTOM = "You are already at the bottom of the page...";
const ALREADY_TOP = "You are already at the top of the page...";
const LOOKUP_FAILED = "Unable to lookup User...";
const ADMIN_ONLY = "No access - Admin Only function ...";
/** zod's default message for the 8-character `USRIDIN` length (`src/schemas/COUSR00.ts`). */
const SEARCH_TOO_LONG = "String must contain at most 8 character(s)";
/** zod's default message for the 1-character length of a selector (`src/schemas/COUSR00.ts`). */
const SELECTION_TOO_LONG = "String must contain at most 1 character(s)";
/** Endpoint 16's 400 for a query member longer than its map field (`FIELD_LENGTH_MESSAGE`). */
const FIELD_TOO_LONG = "Input does not fit the screen field...";
/**
 * `CCDA-MSG-INVALID-KEY PIC X(50)` (`CSMSG01Y.cpy:20-21`): the 50-byte field value COUSR00C-005
 * shows, the short `VALUE` literal padded with spaces, written out so the length is visible.
 */
const INVALID_KEY = "Invalid key pressed. Please see below...          ";
/** The 40-byte title values of COUSR00-091 and COUSR00-094 (`COTTL01Y.cpy:18-22`). */
const TITLE01 = "      AWS Mainframe Modernization       ";
const TITLE02 = "              CardDemo                  ";
/** `ERRMSGO PIC X(78)`: the width of the error row (COUSR00-088, COUSR00C-033). */
const ERRMSG_WIDTH = 78;

/** The ten users of the fixture's key order (`DUSRSECJ.jcl:35-44`), with invented names. */
const FIXTURE_USERS: components["schemas"]["UserListRow"][] = [
  { user_id: "ADMIN001", first_name: "Alma", last_name: "Abbott", user_type: "A" },
  { user_id: "ADMIN002", first_name: "Bram", last_name: "Barker", user_type: "A" },
  { user_id: "ADMIN003", first_name: "Cora", last_name: "Carver", user_type: "A" },
  { user_id: "ADMIN004", first_name: "Dale", last_name: "Dunn", user_type: "A" },
  { user_id: "ADMIN005", first_name: "Enid", last_name: "Ellis", user_type: "A" },
  { user_id: "USER0001", first_name: "Finn", last_name: "Foster", user_type: "U" },
  { user_id: "USER0002", first_name: "Gail", last_name: "Grant", user_type: "U" },
  { user_id: "USER0003", first_name: "Hugo", last_name: "Hayes", user_type: "U" },
  { user_id: "USER0004", first_name: "Iris", last_name: "Irwin", user_type: "U" },
  { user_id: "USER0005", first_name: "Jude", last_name: "Jensen", user_type: "U" },
];

/** `n` blank row positions. */
function blankRows(n: number): null[] {
  return Array.from({ length: n }, () => null);
}

/**
 * A first page of two users, rows 3 to 10 blank: the end of the file came before row 10, so the
 * last key keeps the blank value the request carried (COUSR00C-015, COUSR00C-016).
 */
const PAGE: components["schemas"]["UserListResponse"] = {
  rows: [FIXTURE_USERS[0], FIXTURE_USERS[5], ...blankRows(8)],
  first_key: "ADMIN001",
  last_key: null,
  page_no: 1,
  has_next_page: false,
  message: BOTTOM_REACHED,
};

/** Page 1 of a file with more than ten users: ten rows, a following user, no message. */
const FIRST_PAGE: components["schemas"]["UserListResponse"] = {
  rows: [...FIXTURE_USERS],
  first_key: "ADMIN001",
  last_key: "USER0005",
  page_no: 1,
  has_next_page: true,
  message: null,
};

/**
 * F8 from page 1: three more users and the end of the file, so the last key stays `USER0005`
 * (COUSR00C-015, COUSR00C-016).
 */
const SECOND_PAGE: components["schemas"]["UserListResponse"] = {
  rows: [
    { user_id: "USER0006", first_name: "Kurt", last_name: "Keller", user_type: "U" },
    { user_id: "USER0007", first_name: "Lena", last_name: "Lowe", user_type: "U" },
    { user_id: "USER0008", first_name: "Milo", last_name: "Moss", user_type: "A" },
    ...blankRows(7),
  ],
  first_key: "USER0006",
  last_key: "USER0005",
  page_no: 2,
  has_next_page: false,
  message: BOTTOM_REACHED,
};

/** F7 from page 2: page 1 read backward up to the start of the file (COUSR00C-023). */
const TOP_PAGE: components["schemas"]["UserListResponse"] = {
  ...FIRST_PAGE,
  message: TOP_REACHED,
};

/** ENTER from `USER0001`: the five users from that key on, then the end of the file. */
const SEARCH_PAGE: components["schemas"]["UserListResponse"] = {
  rows: [...FIXTURE_USERS.slice(5), ...blankRows(5)],
  first_key: "USER0001",
  last_key: "USER0005",
  page_no: 1,
  has_next_page: false,
  message: BOTTOM_REACHED,
};

/** F8 with next-page indicator `N`: nothing read, the paging state unchanged (COUSR00C-020). */
const BOTTOM_KEPT: components["schemas"]["UserListResponse"] = {
  rows: null,
  first_key: "ADMIN001",
  last_key: null,
  page_no: 1,
  has_next_page: false,
  message: ALREADY_BOTTOM,
};

/** F7 on page 1: nothing read, the next-page indicator set (COUSR00C-021). */
const TOP_KEPT: components["schemas"]["UserListResponse"] = {
  rows: null,
  first_key: "ADMIN001",
  last_key: "USER0005",
  page_no: 1,
  has_next_page: true,
  message: ALREADY_TOP,
};

/** The 403 of the admin gate. */
const GATE_ERROR: components["schemas"]["ScreenError"] = { message: ADMIN_ONLY };

/** The 500 of a failing page read (COUSR00C-026). */
const LOOKUP_ERROR: components["schemas"]["ScreenError"] = { message: LOOKUP_FAILED };

/** A reply with a JSON body, as the API sends every page and every `ScreenError`. */
function json(status: number, body: unknown): () => Response {
  return () =>
    new Response(JSON.stringify(body), {
      status,
      headers: { "Content-Type": "application/json" },
    });
}

/**
 * `answer`, or a rejection with the reason of `signal` once it aborts while `answer` is still
 * outstanding, as the native `fetch` rejects a request whose signal aborts before its answer.
 */
function unlessAborted(
  answer: Response | Promise<Response>,
  signal: AbortSignal,
): Promise<Response> {
  return new Promise<Response>((resolve, reject) => {
    const abort = (): void => {
      reject(signal.reason);
    };
    if (signal.aborted) {
      abort();
      return;
    }
    signal.addEventListener("abort", abort, { once: true });
    Promise.resolve(answer)
      .then(resolve, reject)
      .finally(() => {
        signal.removeEventListener("abort", abort);
      });
  });
}

/**
 * Replaces the global `fetch`. Each `GET /api/v1/users` takes the next of `replies`, and fails
 * with the request signal's reason if that aborts while the reply is outstanding; any other
 * request, and any past the last reply, is answered 500. Returns every request's method, path,
 * full URL, query parameters and `Authorization` header, in order.
 */
function stubFetch(...replies: (() => Response | Promise<Response>)[]) {
  const sent: {
    method: string;
    path: string;
    url: string;
    query: Record<string, string>;
    authorization: string | null;
  }[] = [];
  const pending = [...replies];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      const url = new URL(req.url);
      sent.push({
        method: req.method,
        path: url.pathname,
        url: req.url,
        query: Object.fromEntries(url.searchParams),
        authorization: req.headers.get("Authorization"),
      });
      const reply =
        req.method === "GET" && url.pathname === LIST_PATH ? pending.shift() : undefined;
      if (reply !== undefined) {
        return unlessAborted(reply(), req.signal);
      }
      const unexpected: components["schemas"]["ScreenError"] = {
        message: `Unexpected request ${req.method} ${url.pathname}`,
      };
      return json(500, unexpected)();
    }),
  );
  return sent;
}

/** A navigation target: its name, and the route state the transfer handed over, as JSON. */
function TransferStub({ name }: { name: string }) {
  const location = useLocation();
  return (
    <section aria-label={name}>
      <h1>{name}</h1>
      <output>{JSON.stringify(location.state ?? null)}</output>
    </section>
  );
}

/**
 * Renders the screen at `/users` with `state`, beside stubs at its transfer targets. Returns the
 * render result and the query client, whose `isFetching` and `isMutating` tell when the screen's
 * requests settled.
 */
function renderList(state?: unknown) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const rendered = render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COUSR00, state }]}>
        <Routes>
          <Route path={PATHS.COUSR00} element={<COUSR00 />} />
          <Route path={PATHS.COUSR02} element={<TransferStub name="Update User" />} />
          <Route path={PATHS.COUSR03} element={<TransferStub name="Delete User" />} />
          <Route path={PATHS.COADM01} element={<TransferStub name="Admin Menu" />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { ...rendered, client };
}

/** The route state the transfer stub `name` shows, parsed back from its JSON. */
function transferState(name: string): unknown {
  const stub = screen.getByRole("region", { name });
  return JSON.parse(within(stub).getByRole("status").textContent ?? "null");
}

/** The value of an `<input>`. */
function valueOf(element: HTMLElement): string {
  if (!(element instanceof HTMLInputElement)) {
    throw new TypeError(`${element.tagName} is not an input`);
  }
  return element.value;
}

function searchField(): HTMLElement {
  return screen.getByLabelText("Search User ID:");
}

function pageField(): HTMLElement {
  return screen.getByLabelText("Page:");
}

/** The selector of list row `row`, 1 to 10. */
function selector(row: number): HTMLElement {
  return screen.getByLabelText(`Selection for row ${row}`);
}

/** Types `value` into `field`, as one change event. */
function typeInto(field: HTMLElement, value: string): void {
  fireEvent.change(field, { target: { value } });
}

/** Clicks the row-24 key labelled `label`. */
function press(label: string): void {
  fireEvent.click(screen.getByRole("button", { name: label }));
}

/** Presses the keyboard key `key`, as the window's `usePfKeys` listener receives it. */
function pressKey(key: string, shiftKey = false): void {
  fireEvent.keyDown(window, { key, shiftKey });
}

/**
 * Moves the cursor to `element`, so that the screen's own move of it back to the search field is
 * observable. The move runs inside `waitFor`, which lets React render the blur it causes.
 */
async function moveFocus(element: HTMLElement): Promise<void> {
  await waitFor(() => {
    element.focus();
    expect(document.activeElement).toBe(element);
  });
}

/** The table's two row groups: the caption and rule lines, then the ten list lines. */
function rowGroups(): HTMLElement[] {
  return within(screen.getByRole("table", { name: "Users" })).getAllByRole("rowgroup");
}

/** The ten list lines (screen lines 10 to 19). */
function listLines(): HTMLElement[] {
  const [, body] = rowGroups();
  return within(body).getAllByRole("row");
}

/** The user ID, first name, last name and user type a list line shows, verbatim. */
function lineValues(line: HTMLElement): (string | null)[] {
  return within(line)
    .getAllByRole("cell")
    .slice(1)
    .map((cell) => cell.textContent);
}

/** Asserts that the ten list lines show the rows of `page` exactly, blank rows empty. */
function expectListed(page: components["schemas"]["UserListResponse"]): void {
  const lines = listLines();
  expect(lines).toHaveLength(10);
  lines.forEach((line, index) => {
    const row = page.rows?.[index] ?? null;
    expect(lineValues(line)).toEqual(
      row === null
        ? ["", "", "", ""]
        : [row.user_id, row.first_name, row.last_name, row.user_type],
    );
  });
}

/** The `ERRMSG` row (row 23), the screen's only message row. */
function errorRow(): HTMLElement {
  const rows = screen
    .getAllByRole("alert")
    .filter((alert) => alert.getAttribute("data-message-kind") === "error");
  expect(rows).toHaveLength(1);
  return rows[0];
}

/** The elements the `aria-describedby` of `input` resolves to, in order, dangling ids left out. */
function descriptionsOf(input: HTMLElement): HTMLElement[] {
  const ids = (input.getAttribute("aria-describedby") ?? "").split(" ");
  return ids.flatMap((id) => {
    const element = id === "" ? null : document.getElementById(id);
    return element === null ? [] : [element];
  });
}

/**
 * Asserts that `input` is marked invalid and described by exactly one field message, the
 * `FormMessage` holding `text`, read by assistive technology only: the error row stays the one
 * visible copy of the text.
 */
function expectFieldMessage(input: HTMLElement, text: string): void {
  expect(input.getAttribute("aria-invalid")).toBe("true");
  const descriptions = descriptionsOf(input);
  expect(descriptions).toHaveLength(1);
  expect(descriptions[0].getAttribute("data-slot")).toBe("form-message");
  expect(descriptions[0].textContent).toBe(text);
  expect(descriptions[0].classList.contains("sr-only")).toBe(true);
  const visible = screen
    .getAllByText(text)
    .filter((holder) => !holder.classList.contains("sr-only"));
  expect(visible).toHaveLength(1);
  expect(errorRow().contains(visible[0])).toBe(true);
}

/** Asserts that `input` is marked valid and described by no field message. */
function expectNoFieldMessage(input: HTMLElement): void {
  expect(input.getAttribute("aria-invalid")).toBe("false");
  expect(descriptionsOf(input)).toEqual([]);
}

/** Each id an `aria-describedby` of the document names that resolves to no element. */
function danglingDescriptions(): string[] {
  return [...document.querySelectorAll("[aria-describedby]")].flatMap((element) =>
    (element.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .filter((id) => id === "" || document.getElementById(id) === null),
  );
}

/** How assistive technology reads an input: see {@link readOut}. */
type ReadOut = {
  invalid: string | null;
  described: ({ text: string | null; hidden: boolean } | null)[] | null;
};

/**
 * How assistive technology reads `input`: its `aria-invalid`, and the element each id of its
 * `aria-describedby` names, as its text and whether `sr-only` hides it (`null` for an id that
 * resolves to nothing). `described` is `null` when the input has no `aria-describedby`.
 */
function readOut(input: HTMLElement): ReadOut {
  const ids = input.getAttribute("aria-describedby");
  return {
    invalid: input.getAttribute("aria-invalid"),
    described:
      ids === null
        ? null
        : ids.split(" ").map((id) => {
            const target = id === "" ? null : document.getElementById(id);
            return target === null
              ? null
              : { text: target.textContent, hidden: target.classList.contains("sr-only") };
          }),
  };
}

/** A valid input of the map, none of which has a hint: described by nothing. */
const UNDESCRIBED: ReadOut = { invalid: "false", described: null };

/** An input an error marks: invalid, described only by its hidden field message `text`. */
function markedWith(text: string): ReadOut {
  return { invalid: "true", described: [{ text, hidden: true }] };
}

/**
 * The value of the header field `name` (`TRNNAME`, `CURDATE`, …): the one read-only `Input` of
 * that `data-field` in the `CardHeader`, failing when there is not exactly one.
 */
function headerField(name: string): string {
  const inputs = document.querySelectorAll(
    `[data-slot="card-header"] input[data-field="${name}"]`,
  );
  expect(inputs).toHaveLength(1);
  const [input] = inputs;
  if (!(input instanceof HTMLInputElement)) {
    throw new Error(`The header has no input ${name}`);
  }
  return input.value;
}

/** Waits until `count` requests were sent and row 1 shows `userId`. */
async function listed(sent: readonly unknown[], count: number, userId: string): Promise<void> {
  await waitFor(() => {
    expect(sent).toHaveLength(count);
    expect(lineValues(listLines()[0])[0]).toBe(userId);
  });
}

/** Waits until `count` requests were sent and the error row shows `text`. */
async function shown(sent: readonly unknown[], count: number, text: string): Promise<void> {
  await waitFor(() => {
    expect(sent).toHaveLength(count);
    expect(errorRow().textContent).toBe(text);
  });
}

/** Bounded Pagination: the screen never sends `page_size`. */
function expectNoPageSize(sent: readonly { url: string }[]): void {
  for (const request of sent) {
    expect(request.url).not.toContain("page_size");
  }
}

/** The keys refused while a page request is outstanding. */
const LOCKED_KEYS = ["ENTER=Continue", "F7=Backward", "F8=Forward"] as const;

/**
 * A reply for {@link stubFetch} that stays outstanding until `release`, which answers it with
 * `answer`.
 */
function heldReply(answer: () => Response): {
  reply: () => Promise<Response>;
  release: () => void;
} {
  const resolvers: ((response: Response) => void)[] = [];
  const outstanding = new Promise<Response>((resolve) => {
    resolvers.push(resolve);
  });
  return {
    reply: () => outstanding,
    release: () => {
      for (const resolve of resolvers) {
        resolve(answer());
      }
    },
  };
}

/** The screen's form, which ENTER submits. */
function mapForm(): HTMLFormElement {
  const form = document.querySelector("form");
  if (form === null) {
    throw new Error("COUSR0A renders no form");
  }
  return form;
}

/** Whether the row-24 key labelled `label` is disabled (`aria-disabled="true"`). */
function isDisabled(label: string): boolean {
  return screen.getByRole("button", { name: label }).getAttribute("aria-disabled") === "true";
}

/** Whether each editable input is read-only: Search User ID, then the ten selectors. */
function readOnlyStates(): boolean[] {
  const fields = [searchField(), ...Array.from({ length: 10 }, (_, index) => selector(index + 1))];
  return fields.map((field) => {
    if (!(field instanceof HTMLInputElement)) {
      throw new TypeError(`${field.tagName} is not an input`);
    }
    return field.readOnly;
  });
}

/**
 * Waits until `client` has no query fetching and exactly `outstanding` list requests in flight,
 * the ones the test still holds (none by default), so every request the screen sent before the
 * call has been answered unless the test holds it. A request the screen sends only later is
 * beyond this wait: a test first awaits the visible outcome of its keys, or {@link lingers} when
 * they have none, and then calls this before it counts the requests.
 */
async function settled(client: QueryClient, outstanding = 0): Promise<void> {
  await waitFor(() => {
    expect(client.isFetching()).toBe(0);
    expect(client.isMutating()).toBe(outstanding);
  });
}

/**
 * Waits 50 ms inside `act`, so React renders whatever the keys just pressed, or the route just
 * opened, caused. A refused key or a screen its guard refuses shows nothing a test could wait for,
 * so this bounded window is the only wait the absence of its effect has: it shows that no request
 * or message followed within 50 ms, not that none ever will.
 */
async function lingers(): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => {
      setTimeout(resolve, 50);
    });
  });
}

beforeEach(() => {
  setSession(TOKEN, "ADMIN001", "A");
});

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COUSR00 List Users", () => {
  it("renders the map: header rows, title, Page and Search User ID fields, list captions and rules, instruction, error row and the four key labels", async () => {
    // audit: COUSR00-011
    // audit: COUSR00-012
    // audit: COUSR00-013
    // audit: COUSR00-014
    // audit: COUSR00-016
    // audit: COUSR00-017
    // audit: COUSR00-018
    // audit: COUSR00-019
    // audit: COUSR00-020
    // audit: COUSR00-021
    // audit: COUSR00-022
    // audit: COUSR00-023
    // audit: COUSR00-024
    // audit: COUSR00-025
    // audit: COUSR00-026
    // audit: COUSR00-087
    // audit: COUSR00-088
    // audit: COUSR00-089
    // audit: COUSR00-090
    // audit: COUSR00-091
    // audit: COUSR00-092
    // audit: COUSR00-093
    // audit: COUSR00-094
    // audit: COUSR00-095
    // audit: COUSR00C-033
    const sent = stubFetch(json(200, FIRST_PAGE));
    renderList();
    await listed(sent, 1, "ADMIN001");

    expect(sent[0].authorization).toBe(`Bearer ${TOKEN}`);

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("List Users");
    /*
     * The header labels' placement and the values the program moves (COUSR00-090 to COUSR00-095),
     * read from the read-only `Input`s `ScreenShell` renders in the `CardHeader` title rows. The
     * label rows COUSR00-001, -004, -006 and -009 are proved by NAV (`frontend/e2e/nav.spec.ts`),
     * and the value rows COUSR00-002, -003, -005, -007, -008 and -010 by the test of those inputs
     * below.
     */
    expect(screen.getByText("Tran:").parentElement?.className).toBe(
      "row-start-1 col-start-1 col-span-5",
    );
    expect(screen.getByText("Date:").parentElement?.className).toBe(
      "row-start-1 col-start-65 col-span-5",
    );
    expect(screen.getByText("Prog:").parentElement?.className).toBe(
      "row-start-2 col-start-1 col-span-5",
    );
    expect(screen.getByText("Time:").parentElement?.className).toBe(
      "row-start-2 col-start-65 col-span-5",
    );
    expect(headerField("TRNNAME")).toBe("CU00");
    expect(headerField("PGMNAME")).toBe("COUSR00C");
    expect(TITLE01).toHaveLength(40);
    expect(TITLE02).toHaveLength(40);
    expect(headerField("TITLE01")).toBe(TITLE01);
    expect(headerField("TITLE02")).toBe(TITLE02);
    // Time-dependent fields are compared by format only.
    expect(headerField("CURDATE")).toMatch(/^\d{2}\/\d{2}\/\d{2}$/);
    expect(headerField("CURTIME")).toMatch(/^\d{2}:\d{2}:\d{2}$/);

    // `Page:` POS=(4,65) labels PAGENUM, POS=(4,71) LENGTH=8: an ASKIP output the cursor skips.
    expect(screen.getByText("Page:").parentElement?.className).toBe(
      "row-start-1 col-start-65 col-span-5",
    );
    expect(pageField().hasAttribute("readonly")).toBe(true);
    expect(pageField().getAttribute("tabindex")).toBe("-1");
    expect(pageField().parentElement?.className).toBe("row-start-1 col-start-71 col-span-8");
    expect(valueOf(pageField())).toBe("00000001");

    // `Search User ID:` POS=(6,5) labels USRIDIN, POS=(6,21) LENGTH=8.
    expect(screen.getByText("Search User ID:").parentElement?.className).toBe(
      "row-start-6 col-start-5 col-span-15",
    );
    expect(searchField().getAttribute("maxlength")).toBe("8");
    expect(searchField().parentElement?.className).toBe("row-start-6 col-start-21 col-span-8");
    // The page, the search key and the selectors take the shared map-field box, `MAP_FIELD_BOX`
    // with `size` = their LENGTH, so all eight characters show on the narrowest frame.
    for (const [field, length] of [
      [pageField(), "8"],
      [searchField(), "8"],
      [selector(1), "1"],
    ] as const) {
      expect(field.getAttribute("size")).toBe(length);
      expect(MAP_FIELD_BOX.filter((name) => !field.classList.contains(name))).toEqual([]);
    }
    // The zero-length stopper at POS=(6,30) renders no element.
    expect(document.querySelector(".row-start-6.col-start-30")).toBeNull();

    const [head] = rowGroups();
    const [captions, rules] = within(head).getAllByRole("row", { hidden: true });
    expect(
      within(captions)
        .getAllByRole("columnheader")
        .map((cell) => cell.textContent),
    ).toEqual(["Sel", "User ID ", "     First Name     ", "     Last Name      ", "Type"]);
    expect(rules.getAttribute("aria-hidden")).toBe("true");
    expect(
      within(rules)
        .getAllByRole("columnheader", { hidden: true })
        .map((cell) => cell.textContent),
    ).toEqual(["---", "-".repeat(8), "-".repeat(20), "-".repeat(20), "----"]);

    const instruction = screen.getByText(
      "Type 'U' to Update or 'D' to Delete a User from the list",
    );
    expect(instruction.parentElement?.className).toBe("row-start-21 col-start-12 col-span-56");

    // ERRMSG: the destructive Alert with its live region, empty while no message is shown.
    const message = errorRow();
    expect(message.getAttribute("aria-live")).toBe("polite");
    expect(message.className).toContain("text-destructive");
    expect(message.className).toContain("border-transparent");
    expect(message.textContent).toBe("");

    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(keys.map((key) => key.textContent)).toEqual([
      "ENTER=Continue",
      "F3=Back",
      "F7=Backward",
      "F8=Forward",
    ]);
    const [enter, ...others] = keys;
    expect(enter.getAttribute("type")).toBe("submit");
    expect(enter.getAttribute("data-variant")).toBe("default");
    // The ENTER button is the form's submit button, so the Enter key in a field submits it too.
    expect(enter.getAttribute("form")).toBe(document.querySelector("form")?.id);
    for (const key of others) {
      expect(key.getAttribute("type")).toBe("button");
      expect(key.getAttribute("data-variant")).toBe("outline");
      expect(key.getAttribute("data-size")).toBe("sm");
    }
  });

  it.each([
    // audit: COUSR00-002
    {
      id: "COUSR00-002",
      name: "TRNNAME",
      row: 1,
      col: 7,
      length: 4,
      label: "Tran:",
      shows: "CU00",
    },
    // audit: COUSR00-003
    {
      id: "COUSR00-003",
      name: "TITLE01",
      row: 1,
      col: 21,
      length: 40,
      label: null,
      shows: TITLE01,
    },
    // audit: COUSR00-005
    {
      id: "COUSR00-005",
      name: "CURDATE",
      row: 1,
      col: 71,
      length: 8,
      label: "Date:",
      shows: expect.stringMatching(/^\d{2}\/\d{2}\/\d{2}$/),
    },
    // audit: COUSR00-007
    {
      id: "COUSR00-007",
      name: "PGMNAME",
      row: 2,
      col: 7,
      length: 8,
      label: "Prog:",
      shows: "COUSR00C",
    },
    // audit: COUSR00-008
    {
      id: "COUSR00-008",
      name: "TITLE02",
      row: 2,
      col: 21,
      length: 40,
      label: null,
      shows: TITLE02,
    },
    // audit: COUSR00-010
    {
      id: "COUSR00-010",
      name: "CURTIME",
      row: 2,
      col: 71,
      length: 8,
      label: "Time:",
      shows: expect.stringMatching(/^\d{2}:\d{2}:\d{2}$/),
    },
  ])(
    "$id: header value $name is a read-only Input of the CardHeader title rows at its map POS and LENGTH",
    async ({ name, row, col, length, label, shows }) => {
      // The values these fields show are proved under COUSR00-090 to COUSR00-095.
      const sent = stubFetch(json(200, FIRST_PAGE));
      renderList();
      await listed(sent, 1, "ADMIN001");

      const header = document.querySelector('[data-slot="card-header"]');
      if (!(header instanceof HTMLElement)) {
        throw new Error("The screen has no card header");
      }
      // The field's one element stands in the title rows, not in the body or the footer.
      const shown = document.querySelectorAll(`[data-field="${name}"]`);
      expect(shown).toHaveLength(1);
      const value = shown.item(0);
      expect(header.contains(value)).toBe(true);
      expect(value.closest('[data-slot="card-content"], [data-slot="card-footer"]')).toBeNull();
      // It is an output field of the map: a read-only `Input` as wide as its LENGTH (`size`), in
      // the cell of its POS and LENGTH, holding LENGTH characters, the padding that centres the
      // titles included.
      if (!(value instanceof HTMLInputElement)) {
        throw new Error(`The header field ${name} is not an input`);
      }
      expect(value.dataset.slot).toBe("input");
      expect(value.readOnly).toBe(true);
      expect(value.size).toBe(length);
      expect(value.parentElement?.className).toBe(
        `row-start-${row} col-start-${col} col-span-${length}`,
      );
      expect(value.value).toEqual(shows);
      expect(value.value).toHaveLength(length);
      // The map literal before it is its bound `Label`; a title, which no literal labels, is named
      // by `aria-label`.
      const labels = Array.from(value.labels ?? []);
      expect(labels.map((bound) => bound.textContent)).toEqual(label === null ? [] : [label]);
      expect(labels.every((bound) => bound.dataset.slot === "label")).toBe(true);
      expect(value.getAttribute("aria-label")).toBe(label === null ? `Title row ${row}` : null);
      // The header holds no editable control: its controls are the six read-only values of the
      // title rows and the read-only PAGENUM of the title line.
      const controls = Array.from(
        header.querySelectorAll("input, textarea, select, button, [contenteditable]"),
      );
      expect(controls).toHaveLength(7);
      expect(controls).toContain(pageField());
      expect(
        controls.every((control) => control instanceof HTMLInputElement && control.readOnly),
      ).toBe(true);
    },
  );

  it("shows `Page:` and PAGENUM on the title's line of the card header, beside the level-1 heading", async () => {
    // audit: COUSR00-011
    // audit: COUSR00-012
    // audit: COUSR00-013
    const sent = stubFetch(json(200, FIRST_PAGE));
    renderList();
    await listed(sent, 1, "ADMIN001");

    // Map line 4 holds the title and both fields: they are cells of the one line of the frame's
    // header, each at its map column, and the body gives the line no track.
    const heading = screen.getByRole("heading", { level: 1, name: "List Users" });
    const line = heading.parentElement;
    const label = screen.getByText("Page:").parentElement;
    const value = pageField().parentElement;
    expect(line?.closest('[data-slot="card-header"]')).not.toBeNull();
    expect(label?.parentElement).toBe(line);
    expect(value?.parentElement).toBe(line);
    expect(label?.className).toBe("row-start-1 col-start-65 col-span-5");
    expect(value?.className).toBe("row-start-1 col-start-71 col-span-8");
    expect(document.querySelector("form")?.contains(pageField())).toBe(false);
  });


  it.each([
    { title: "two users and the end-of-file message", page: PAGE, message: BOTTOM_REACHED },
    { title: "ten users and no message", page: FIRST_PAGE, message: "" },
  ])(
    "lists page 1 on entry, $title: rows verbatim, eight-digit page number, empty one-character selectors, focus on the search field, no password",
    async ({ page, message }) => {
      // audit: COUSR00-028
      // audit: COUSR00-029
      // audit: COUSR00-030
      // audit: COUSR00-031
      // audit: COUSR00-032
      // audit: COUSR00-034
      // audit: COUSR00-035
      // audit: COUSR00-036
      // audit: COUSR00-037
      // audit: COUSR00-038
      // audit: COUSR00-040
      // audit: COUSR00-041
      // audit: COUSR00-042
      // audit: COUSR00-043
      // audit: COUSR00-044
      // audit: COUSR00-046
      // audit: COUSR00-047
      // audit: COUSR00-048
      // audit: COUSR00-049
      // audit: COUSR00-050
      // audit: COUSR00-052
      // audit: COUSR00-053
      // audit: COUSR00-054
      // audit: COUSR00-055
      // audit: COUSR00-056
      // audit: COUSR00-058
      // audit: COUSR00-059
      // audit: COUSR00-060
      // audit: COUSR00-061
      // audit: COUSR00-062
      // audit: COUSR00-064
      // audit: COUSR00-065
      // audit: COUSR00-066
      // audit: COUSR00-067
      // audit: COUSR00-068
      // audit: COUSR00-070
      // audit: COUSR00-071
      // audit: COUSR00-072
      // audit: COUSR00-073
      // audit: COUSR00-074
      // audit: COUSR00-076
      // audit: COUSR00-077
      // audit: COUSR00-078
      // audit: COUSR00-079
      // audit: COUSR00-080
      // audit: COUSR00-082
      // audit: COUSR00-083
      // audit: COUSR00-084
      // audit: COUSR00-085
      // audit: COUSR00-086
      // audit: COUSR00-088
      // audit: COUSR00C-029
      // audit: COUSR00C-032
      const sent = stubFetch(json(200, page));
      renderList();
      await listed(sent, 1, "ADMIN001");

      expect(sent[0].method).toBe("GET");
      expect(sent[0].path).toBe(LIST_PATH);
      expect(sent[0].query).toEqual({});

      expectListed(page);
      // Each line holds the selector and the four user fields: the zero-length stopper after
      // each selector renders no element.
      for (const line of listLines()) {
        expect(within(line).getAllByRole("cell")).toHaveLength(5);
      }
      expect(valueOf(pageField())).toBe("00000001");
      for (let row = 1; row <= 10; row += 1) {
        expect(selector(row).getAttribute("maxlength")).toBe("1");
        expect(valueOf(selector(row))).toBe("");
      }
      expect(valueOf(searchField())).toBe("");
      expect(document.activeElement).toBe(searchField());
      expect(errorRow().textContent).toBe(message);

      // No password anywhere: no password field, and no password text on the screen.
      expect(document.querySelector('input[type="password"]')).toBeNull();
      expect(document.body.textContent).not.toMatch(/password/i);
    },
  );

  it.each([
    { title: "first entry from the admin menu", state: { from: PATHS.COADM01 }, query: {} },
    { title: "an entry without route state", state: undefined, query: {} },
    {
      title: "a return from /users/update",
      state: {
        from: PATHS.COUSR02,
        user_id: "USER0003",
        first_key: "ADMIN001",
        last_key: "USER0005",
      },
      query: { first_key: "ADMIN001", last_key: "USER0005" },
    },
    {
      title: "a return from /users/delete",
      state: {
        from: PATHS.COUSR03,
        user_id: "USER0007",
        first_key: "USER0006",
        last_key: "USER0005",
      },
      query: { first_key: "USER0006", last_key: "USER0005" },
    },
    {
      title: "route state whose keys are not text",
      state: { from: PATHS.COUSR02, first_key: 7, last_key: null },
      query: {},
    },
  ])(
    "$title requests page 1 with only the keys the entry carried and starts with blank selectors",
    async ({ state, query }) => {
      // audit: COUSR00C-003
      // audit: COUSR00C-030
      const sent = stubFetch(json(200, FIRST_PAGE));
      renderList(state);
      await listed(sent, 1, "ADMIN001");

      // No search value, page number, indicator or direction: ENTER with a blank search field.
      expect(sent[0].query).toEqual(query);
      expectNoPageSize(sent);
      expect(valueOf(pageField())).toBe("00000001");
      for (let row = 1; row <= 10; row += 1) {
        expect(valueOf(selector(row))).toBe("");
      }
    },
  );

  it.each(
    Array.from({ length: 10 }, (_, index) => index + 1).flatMap((row) => [
      { row, code: "U", target: "Update User" },
      { row, code: "D", target: "Delete User" },
    ]),
  )(
    "$code in row $row's selector opens $target with that row's user ID and no request",
    async ({ row, code, target }) => {
      // audit: COUSR00-027
      // audit: COUSR00-033
      // audit: COUSR00-039
      // audit: COUSR00-045
      // audit: COUSR00-051
      // audit: COUSR00-057
      // audit: COUSR00-063
      // audit: COUSR00-069
      // audit: COUSR00-075
      // audit: COUSR00-081
      // audit: COUSR00-096
      // audit: COUSR00C-008
      // audit: COUSR00C-009
      const sent = stubFetch(json(200, FIRST_PAGE));
      renderList();
      await listed(sent, 1, "ADMIN001");

      typeInto(selector(row), code);
      press("ENTER=Continue");

      await screen.findByRole("region", { name: target });
      expect(transferState(target)).toEqual({
        from: PATHS.COUSR00,
        user_id: FIXTURE_USERS[row - 1].user_id,
        first_key: "ADMIN001",
        last_key: "USER0005",
      });
      // The selector is never sent: the selection only navigates.
      expect(sent).toHaveLength(1);
    },
  );

  it.each([
    { code: "u", target: "Update User" },
    { code: "d", target: "Delete User" },
  ])(
    "the first filled selector decides: $code in row 2 opens $target for row 2 despite later codes, ignoring the search field",
    async ({ code, target }) => {
      // audit: COUSR00C-007
      // audit: COUSR00C-008
      // audit: COUSR00C-009
      const sent = stubFetch(json(200, PAGE));
      renderList();
      await listed(sent, 1, "ADMIN001");

      typeInto(searchField(), "ADMIN003");
      typeInto(selector(2), code);
      typeInto(selector(5), "X");
      typeInto(selector(9), "U");
      press("ENTER=Continue");

      await screen.findByRole("region", { name: target });
      // PAGE left the last key blank, so the transfer carries none.
      expect(transferState(target)).toEqual({
        from: PATHS.COUSR00,
        user_id: "USER0001",
        first_key: "ADMIN001",
      });
      expect(sent).toHaveLength(1);
    },
  );

  it.each([
    {
      answer: "a page without a message",
      reply: json(200, FIRST_PAGE),
      message: INVALID_SELECTION,
      search: "",
    },
    {
      answer: "a page with the end-of-file message",
      reply: json(200, SEARCH_PAGE),
      message: BOTTOM_REACHED,
      search: "",
    },
    {
      answer: "a 500 ScreenError",
      reply: json(500, LOOKUP_ERROR),
      message: LOOKUP_FAILED,
      search: "USER0001",
    },
  ])(
    "an invalid code in the first selector lists from the search field and shows `$message` after $answer, with no transfer",
    async ({ reply, message, search }) => {
      // audit: COUSR00-088
      // audit: COUSR00C-007
      // audit: COUSR00C-010
      // audit: COUSR00C-030
      // audit: COUSR00C-031
      // audit: COUSR00C-032
      const sent = stubFetch(json(200, FIRST_PAGE), reply);
      renderList();
      await listed(sent, 1, "ADMIN001");

      // Row 1's `X` comes first, so row 4's `U` is ignored. The invalid code does not stop the
      // ENTER: the list is read, and a message of its answer replaces the selection message.
      typeInto(searchField(), "USER0001");
      typeInto(selector(1), "X");
      typeInto(selector(4), "U");
      await moveFocus(selector(4));
      press("ENTER=Continue");
      await shown(sent, 2, message);

      expect(sent[1].query).toEqual({
        start_user_id: "USER0001",
        first_key: "ADMIN001",
        last_key: "USER0005",
        page_no: "1",
        has_next_page: "true",
      });
      expectNoPageSize(sent);
      expect(screen.queryByRole("region", { name: "Update User" })).toBeNull();
      expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("List Users");
      // The selectors keep what was typed in them; the search field is cleared only by a page.
      expect(valueOf(selector(1))).toBe("X");
      expect(valueOf(selector(4))).toBe("U");
      expect(valueOf(searchField())).toBe(search);
      expect(document.activeElement).toBe(searchField());
    },
  );

  it("ignores a code on a blank row: ENTER lists and shows no message", async () => {
    // audit: COUSR00C-011
    // audit: COUSR00C-030
    const reread: components["schemas"]["UserListResponse"] = { ...PAGE, message: null };
    const sent = stubFetch(json(200, PAGE), json(200, reread));
    renderList();
    await shown(sent, 1, BOTTOM_REACHED);

    typeInto(selector(5), "U");
    press("ENTER=Continue");
    await shown(sent, 2, "");

    // PAGE left the last key blank, so the request echoes none.
    expect(sent[1].query).toEqual({ first_key: "ADMIN001", page_no: "1", has_next_page: "false" });
    expect(screen.queryByRole("region", { name: "Update User" })).toBeNull();
    expectListed(reread);
    expect(valueOf(selector(5))).toBe("U");
  });

  it.each([
    {
      via: "the ENTER=Continue button",
      enter: () => {
        press("ENTER=Continue");
      },
    },
    {
      via: "the Enter key",
      enter: () => {
        pressKey("Enter");
      },
    },
  ])(
    "ENTER through $via lists from Search User ID and clears the field once the page arrives",
    async ({ enter }) => {
      // audit: COUSR00-096
      // audit: COUSR00C-029
      // audit: COUSR00C-031
      // audit: COUSR00C-032
      const sent = stubFetch(json(200, FIRST_PAGE), json(200, SEARCH_PAGE));
      renderList();
      await listed(sent, 1, "ADMIN001");

      typeInto(searchField(), "USER0001");
      await moveFocus(selector(1));
      enter();
      await listed(sent, 2, "USER0001");

      expect(sent[1].method).toBe("GET");
      expect(sent[1].path).toBe(LIST_PATH);
      expect(sent[1].query).toEqual({
        start_user_id: "USER0001",
        first_key: "ADMIN001",
        last_key: "USER0005",
        page_no: "1",
        has_next_page: "true",
      });
      expectNoPageSize(sent);
      expectListed(SEARCH_PAGE);
      expect(valueOf(pageField())).toBe("00000001");
      expect(errorRow().textContent).toBe(BOTTOM_REACHED);
      expect(valueOf(searchField())).toBe("");
      expect(document.activeElement).toBe(searchField());
    },
  );

  it("pages forward with F8 and back with F7, echoing the paging state the last response left, never page_size", async () => {
    // audit: COUSR00-098
    // audit: COUSR00-099
    // audit: COUSR00C-029
    // audit: COUSR00C-030
    // audit: COUSR00C-031
    // audit: COUSR00C-032
    const sent = stubFetch(json(200, FIRST_PAGE), json(200, SECOND_PAGE), json(200, TOP_PAGE));
    renderList();
    await listed(sent, 1, "ADMIN001");

    // F8 reads on from the last user ID shown and clears the search field.
    typeInto(searchField(), "USER0003");
    typeInto(selector(3), "Q");
    await moveFocus(selector(3));
    press("F8=Forward");
    await listed(sent, 2, "USER0006");

    expect(sent[1].query).toEqual({
      direction: "forward",
      page_key: "USER0005",
      first_key: "ADMIN001",
      last_key: "USER0005",
      page_no: "1",
      has_next_page: "true",
    });
    expectListed(SECOND_PAGE);
    expect(valueOf(pageField())).toBe("00000002");
    expect(errorRow().textContent).toBe(BOTTOM_REACHED);
    expect(valueOf(searchField())).toBe("");
    expect(valueOf(selector(3))).toBe("Q");
    expect(document.activeElement).toBe(searchField());

    // F7 reads back from the first user ID shown and keeps the search field.
    typeInto(searchField(), "USER0003");
    await moveFocus(selector(3));
    press("F7=Backward");
    await listed(sent, 3, "ADMIN001");

    expect(sent[2].query).toEqual({
      direction: "backward",
      page_key: "USER0006",
      first_key: "USER0006",
      last_key: "USER0005",
      page_no: "2",
      has_next_page: "false",
    });
    expectListed(TOP_PAGE);
    expect(valueOf(pageField())).toBe("00000001");
    expect(errorRow().textContent).toBe(TOP_REACHED);
    expect(valueOf(searchField())).toBe("USER0003");
    expect(valueOf(selector(3))).toBe("Q");
    expect(document.activeElement).toBe(searchField());

    expectNoPageSize(sent);
  });

  it.each([
    {
      key: "F8",
      entry: PAGE,
      reply: BOTTOM_KEPT,
      // PAGE left the last key blank, so F8 sends no page key.
      query: { direction: "forward", first_key: "ADMIN001", page_no: "1", has_next_page: "false" },
    },
    {
      key: "F7",
      entry: FIRST_PAGE,
      reply: TOP_KEPT,
      query: {
        direction: "backward",
        page_key: "ADMIN001",
        first_key: "ADMIN001",
        last_key: "USER0005",
        page_no: "1",
        has_next_page: "true",
      },
    },
  ])(
    "the $key key sends its page request; a response without a page keeps the rows, page number, search text and selectors",
    async ({ key, entry, reply, query }) => {
      // audit: COUSR00-088
      // audit: COUSR00-098
      // audit: COUSR00-099
      // audit: COUSR00C-029
      // audit: COUSR00C-030
      // audit: COUSR00C-031
      // audit: COUSR00C-032
      const sent = stubFetch(json(200, entry), json(200, reply));
      renderList();
      await listed(sent, 1, "ADMIN001");

      typeInto(searchField(), "USER0002");
      typeInto(selector(2), "Z");
      await moveFocus(selector(2));
      pressKey(key);
      await shown(sent, 2, reply.message ?? "");

      expect(sent[1].query).toEqual(query);
      expectNoPageSize(sent);
      expectListed(entry);
      expect(valueOf(pageField())).toBe("00000001");
      expect(valueOf(searchField())).toBe("USER0002");
      expect(valueOf(selector(2))).toBe("Z");
      expect(document.activeElement).toBe(searchField());
    },
  );

  it.each([
    {
      via: "the F3=Back button",
      back: () => {
        press("F3=Back");
      },
    },
    {
      via: "the F3 key",
      back: () => {
        pressKey("F3");
      },
    },
  ])(
    "F3 through $via returns to the admin menu with no request and drops the paging state",
    async ({ back }) => {
      // audit: COUSR00-097
      // audit: COUSR00C-004
      const sent = stubFetch(json(200, FIRST_PAGE));
      renderList({ from: PATHS.COUSR02, first_key: "ADMIN001", last_key: "USER0005" });
      await listed(sent, 1, "ADMIN001");

      typeInto(selector(1), "U");
      back();

      await screen.findByRole("region", { name: "Admin Menu" });
      expect(transferState("Admin Menu")).toBeNull();
      expect(sent).toHaveLength(1);
    },
  );

  it.each([
    { key: "F1", shiftKey: false },
    { key: "F2", shiftKey: false },
    { key: "F4", shiftKey: false },
    { key: "F5", shiftKey: false },
    { key: "F6", shiftKey: false },
    { key: "F9", shiftKey: false },
    { key: "F10", shiftKey: false },
    { key: "F11", shiftKey: false },
    { key: "F12", shiftKey: false },
    // Shift+F3, Shift+F7 and Shift+F8 send PF15, PF19 and PF20, which COUSR00C does not serve.
    { key: "F3", shiftKey: true },
    { key: "F7", shiftKey: true },
    { key: "F8", shiftKey: true },
  ])(
    "the unserved key $key (shift: $shiftKey) shows the invalid-key message and changes nothing else",
    async ({ key, shiftKey }) => {
      // audit: COUSR00-088
      // audit: COUSR00C-005
      // audit: COUSR00C-032
      const sent = stubFetch(json(200, FIRST_PAGE));
      renderList();
      await listed(sent, 1, "ADMIN001");

      typeInto(searchField(), "USER0002");
      typeInto(selector(2), "U");
      await moveFocus(selector(2));
      pressKey(key, shiftKey);

      // The 50-byte CSMSG01Y value, trailing spaces included.
      await shown(sent, 1, INVALID_KEY);
      expect(INVALID_KEY).toHaveLength(50);
      expect(screen.queryByRole("region", { name: "Admin Menu" })).toBeNull();
      expectListed(FIRST_PAGE);
      expect(valueOf(pageField())).toBe("00000001");
      expect(valueOf(searchField())).toBe("USER0002");
      expect(valueOf(selector(2))).toBe("U");
      expect(document.activeElement).toBe(searchField());
    },
  );

  it("shows the admin gate's 403 ScreenError in the error row and lists nothing", async () => {
    // audit: COUSR00-088
    // audit: COUSR00C-032
    const sent = stubFetch(json(403, GATE_ERROR));
    renderList();
    await shown(sent, 1, ADMIN_ONLY);

    for (const line of listLines()) {
      expect(lineValues(line)).toEqual(["", "", "", ""]);
    }
    // PAGENUM's INITIAL is a space: no page number until a page arrives.
    expect(valueOf(pageField())).toBe("");
    expect(document.activeElement).toBe(searchField());
  });

  /** An 80-character `WS-MESSAGE`: 78 that fit `ERRMSGO`, then two that the move cuts off. */
  const FULL_MESSAGE = `${"M".repeat(ERRMSG_WIDTH)}XY`;

  it.each([
    {
      answer: "a 500 ScreenError",
      reply: json(500, { message: FULL_MESSAGE } satisfies components["schemas"]["ScreenError"]),
    },
    {
      answer: "a page",
      reply: json(200, {
        ...FIRST_PAGE,
        message: FULL_MESSAGE,
      } satisfies components["schemas"]["UserListResponse"]),
    },
  ])(
    "shows the first 78 characters of a longer message from $answer, as ERRMSGO PIC X(78) holds it",
    async ({ reply }) => {
      // audit: COUSR00-088
      // audit: COUSR00C-033
      const sent = stubFetch(reply);
      renderList();
      await shown(sent, 1, "M".repeat(ERRMSG_WIDTH));

      expect(FULL_MESSAGE).toHaveLength(80);
      expect(errorRow().textContent).toHaveLength(ERRMSG_WIDTH);
      expect(document.activeElement).toBe(searchField());
    },
  );

  /** Endpoint 16's 400 for a search value its query DTO refuses, naming `start_user_id`. */
  const SEARCH_REFUSED: components["schemas"]["ScreenError"] = {
    message: FIELD_TOO_LONG,
    field: "start_user_id",
  };

  // audit: COUSR00-088
  // audit: COUSR00C-032
  // audit: COUSR00C-033
  it.each([
    { title: "its length message", reply: SEARCH_REFUSED, text: FIELD_TOO_LONG },
    {
      title: "an 80-character message",
      reply: {
        message: FULL_MESSAGE,
        field: "start_user_id",
      } satisfies components["schemas"]["ScreenError"],
      text: "M".repeat(ERRMSG_WIDTH),
    },
  ])(
    "a 400 ScreenError naming start_user_id with $title marks Search User ID invalid, its field message the error row's text for assistive technology only",
    async ({ reply, text }) => {
      const sent = stubFetch(json(200, FIRST_PAGE), json(400, reply));
      renderList();
      await listed(sent, 1, "ADMIN001");
      expectNoFieldMessage(searchField());

      typeInto(searchField(), "USER0001");
      await moveFocus(selector(1));
      press("ENTER=Continue");
      await shown(sent, 2, text);

      expect(sent[1].query).toMatchObject({ start_user_id: "USER0001" });
      expectFieldMessage(searchField(), text);
      expect(valueOf(searchField())).toBe("USER0001");
      expect(document.activeElement).toBe(searchField());
      // The selectors are never sent, so no server answer marks one.
      for (let row = 1; row <= 10; row += 1) {
        expectNoFieldMessage(selector(row));
      }
    },
  );

  it.each([
    {
      answer: "a 400 naming the echoed page_key, which no map field holds",
      key: "F8=Forward",
      reply: json(400, {
        message: FIELD_TOO_LONG,
        field: "page_key",
      } satisfies components["schemas"]["ScreenError"]),
      text: FIELD_TOO_LONG,
    },
    {
      answer: "a 500 naming no field",
      key: "ENTER=Continue",
      reply: json(500, LOOKUP_ERROR),
      text: LOOKUP_FAILED,
    },
  ])("$answer shows its message and marks no field", async ({ key, reply, text }) => {
    const sent = stubFetch(json(200, FIRST_PAGE), reply);
    renderList();
    await listed(sent, 1, "ADMIN001");

    press(key);
    await shown(sent, 2, text);

    expectNoFieldMessage(searchField());
    for (let row = 1; row <= 10; row += 1) {
      expectNoFieldMessage(selector(row));
    }
  });

  it.each([
    {
      via: "an edit of Search User ID, with no request",
      act: () => {
        typeInto(searchField(), "USER0002");
      },
      replies: [],
      requests: 2,
    },
    {
      via: "the page the next ENTER returns",
      act: () => {
        press("ENTER=Continue");
      },
      replies: [json(200, SEARCH_PAGE)],
      requests: 3,
    },
    {
      via: "the page F8 returns",
      act: () => {
        press("F8=Forward");
      },
      replies: [json(200, SECOND_PAGE)],
      requests: 3,
    },
    {
      via: "the next request, F7 answered by a 500 naming no field",
      act: () => {
        press("F7=Backward");
      },
      replies: [json(500, LOOKUP_ERROR)],
      requests: 3,
    },
  ])(
    "the server's mark on Search User ID goes after $via",
    async ({ act, replies, requests }) => {
      const sent = stubFetch(json(200, FIRST_PAGE), json(400, SEARCH_REFUSED), ...replies);
      renderList();
      await listed(sent, 1, "ADMIN001");
      typeInto(searchField(), "USER0001");
      press("ENTER=Continue");
      await shown(sent, 2, FIELD_TOO_LONG);
      expectFieldMessage(searchField(), FIELD_TOO_LONG);

      act();
      await waitFor(() => {
        expect(sent).toHaveLength(requests);
        expect(searchField().getAttribute("aria-invalid")).toBe("false");
      });
      expectNoFieldMessage(searchField());
    },
  );

  it.each([
    { trigger: "the entry list", before: [], start: () => undefined, held: FIRST_PAGE },
    {
      trigger: "the ENTER=Continue button",
      before: [FIRST_PAGE],
      start: () => {
        typeInto(searchField(), "USER0001");
        press("ENTER=Continue");
      },
      held: SEARCH_PAGE,
    },
    {
      trigger: "the Enter key",
      before: [FIRST_PAGE],
      start: () => {
        typeInto(searchField(), "USER0001");
        pressKey("Enter");
      },
      held: SEARCH_PAGE,
    },
    {
      trigger: "the F7=Backward button",
      before: [FIRST_PAGE],
      start: () => {
        press("F7=Backward");
      },
      held: TOP_KEPT,
    },
    {
      trigger: "the F8 key",
      before: [FIRST_PAGE],
      start: () => {
        pressKey("F8");
      },
      held: SECOND_PAGE,
    },
  ])(
    "locks ENTER, F7, F8 and the inputs while $trigger is outstanding, keeps F3, then frees them",
    async ({ before, start, held }) => {
      const gate = heldReply(json(200, held));
      const sent = stubFetch(
        ...before.map((page) => json(200, page)),
        gate.reply,
        json(200, BOTTOM_KEPT),
      );
      const { client } = renderList();
      try {
        if (before.length > 0) {
          await listed(sent, 1, "ADMIN001");
        }
        const count = before.length + 1;
        start();
        await waitFor(() => {
          expect(sent).toHaveLength(count);
        });

        const keyBar = screen.getByRole("group", { name: "Function keys" });
        await waitFor(() => {
          expect(keyBar.getAttribute("aria-busy")).toBe("true");
        });
        expect(mapForm().getAttribute("aria-busy")).toBe("true");
        expect(LOCKED_KEYS.map((label) => isDisabled(label))).toEqual([true, true, true]);
        expect(isDisabled("F3=Back")).toBe(false);
        expect(readOnlyStates()).toEqual(Array.from({ length: 11 }, () => true));

        // Every locked key, by button and by keyboard, a direct submit and an unserved key: no
        // request, no message.
        for (const label of LOCKED_KEYS) {
          press(label);
        }
        for (const key of ["Enter", "F7", "F8", "F5"]) {
          pressKey(key);
        }
        fireEvent.submit(mapForm());
        // A refused key shows nothing: a bounded window, then only the held request in flight.
        await lingers();
        await settled(client, 1);
        expect(sent).toHaveLength(count);
        expect(errorRow().textContent).toBe("");

        gate.release();
        await waitFor(() => {
          expect(keyBar.getAttribute("aria-busy")).toBe("false");
        });
        expect(mapForm().getAttribute("aria-busy")).toBe("false");
        expect(LOCKED_KEYS.map((label) => isDisabled(label))).toEqual([false, false, false]);
        expect(readOnlyStates()).toEqual(Array.from({ length: 11 }, () => false));

        // Once answered, the next key is served.
        press("F8=Forward");
        await waitFor(() => {
          expect(sent).toHaveLength(count + 1);
        });
        // The F8 page's answer shows, then nothing is left in flight.
        await shown(sent, count + 1, ALREADY_BOTTOM);
        await settled(client);
        expect(sent).toHaveLength(count + 1);
        expect(sent[count].query).toMatchObject({ direction: "forward" });
      } finally {
        // The held page is answered and every request settled inside the test, pass or fail, so
        // neither a request nor the client's deadline outlives the test. Once the body has
        // released the hold, releasing it again does nothing.
        gate.release();
        await waitFor(() => {
          expect(client.isMutating()).toBe(0);
        });
      }
    },
  );

  it("acts on no selection while a page request is outstanding, then on the page it answered", async () => {
    const gate = heldReply(json(200, SECOND_PAGE));
    const sent = stubFetch(json(200, FIRST_PAGE), gate.reply);
    const { client } = renderList();
    try {
      await listed(sent, 1, "ADMIN001");

      typeInto(selector(1), "U");
      pressKey("F8");
      await waitFor(() => {
        expect(sent).toHaveLength(2);
      });
      press("ENTER=Continue");
      pressKey("Enter");
      fireEvent.submit(mapForm());
      // A refused selection shows nothing: a bounded window, then only the held F8 page in flight.
      await lingers();
      await settled(client, 1);
      expect(screen.queryByRole("region", { name: "Update User" })).toBeNull();
      expect(sent).toHaveLength(2);

      gate.release();
      await listed(sent, 2, "USER0006");
      expect(valueOf(selector(1))).toBe("U");
      press("ENTER=Continue");

      await screen.findByRole("region", { name: "Update User" });
      expect(transferState("Update User")).toEqual({
        from: PATHS.COUSR00,
        user_id: "USER0006",
        first_key: "USER0006",
        last_key: "USER0005",
      });
      expect(sent).toHaveLength(2);
    } finally {
      // The held F8 page is answered and every request settled inside the test, pass or fail, so
      // neither a request nor the client's deadline outlives the test. Once the body has
      // released the hold, releasing it again does nothing.
      gate.release();
      await waitFor(() => {
        expect(client.isMutating()).toBe(0);
      });
    }
  });

  it("F3 returns to the admin menu while the entry list is outstanding", async () => {
    const gate = heldReply(json(200, FIRST_PAGE));
    const sent = stubFetch(gate.reply);
    const { client } = renderList();
    try {
      await waitFor(() => {
        expect(isDisabled("ENTER=Continue")).toBe(true);
      });

      press("F3=Back");

      await screen.findByRole("region", { name: "Admin Menu" });
      expect(sent).toHaveLength(1);
    } finally {
      // The entry list is answered only after F3 left, pass or fail, and the test ends once the
      // request has settled, so neither it nor the client's deadline outlives the test.
      gate.release();
      await waitFor(() => {
        expect(client.isMutating()).toBe(0);
      });
    }
  });

  it("refuses a search value longer than the map's 8 characters with no request", async () => {
    // `USRIDIN` is the `both` row COUSR00-015: its client half is proved, and tagged, by
    // src/schemas/COUSR00.test.ts; this test proves only that the screen shows the refusal.
    const sent = stubFetch(json(200, FIRST_PAGE));
    renderList();
    await listed(sent, 1, "ADMIN001");

    // A change event sets the value past `maxlength`, as a paste through script would.
    typeInto(searchField(), "USER00012");
    await moveFocus(selector(1));
    press("ENTER=Continue");
    await shown(sent, 1, SEARCH_TOO_LONG);

    expect(valueOf(searchField())).toBe("USER00012");
    expectFieldMessage(searchField(), SEARCH_TOO_LONG);
    expect(document.activeElement).toBe(searchField());
  });

  it("refuses a selector longer than its 1 character with no request, its field message holding the error row's text", async () => {
    // The schema's selector length is proved by src/schemas/COUSR00.test.ts; this test proves
    // only that the screen shows the refusal against the selector it concerns.
    const sent = stubFetch(json(200, FIRST_PAGE));
    renderList();
    await listed(sent, 1, "ADMIN001");

    // A change event sets the value past `maxlength`, as a paste through script would.
    typeInto(selector(3), "UU");
    press("ENTER=Continue");
    await shown(sent, 1, SELECTION_TOO_LONG);

    expect(valueOf(selector(3))).toBe("UU");
    expectFieldMessage(selector(3), SELECTION_TOO_LONG);
    for (const row of [1, 2, 4, 5, 6, 7, 8, 9, 10]) {
      expectNoFieldMessage(selector(row));
    }
    expectNoFieldMessage(searchField());
  });

  // The replacement's accessibility wiring, which no audit row states: every `aria-describedby`
  // names only elements the screen renders. No input of the map has a hint, so a valid input is
  // described by nothing, and the input an error marks only by its field message, which holds
  // the error row's text for assistive technology alone.
  it.each([
    {
      state: "on entry",
      replies: [json(200, FIRST_PAGE)],
      act: () => undefined,
      requests: 1,
      text: "",
      search: UNDESCRIBED,
      marked: null,
    },
    {
      state: "after a 400 naming start_user_id",
      replies: [json(200, FIRST_PAGE), json(400, SEARCH_REFUSED)],
      act: () => {
        typeInto(searchField(), "USER0001");
        press("ENTER=Continue");
      },
      requests: 2,
      text: FIELD_TOO_LONG,
      search: markedWith(FIELD_TOO_LONG),
      marked: null,
    },
    {
      state: "after a selector longer than its 1 character",
      replies: [json(200, FIRST_PAGE)],
      act: () => {
        typeInto(selector(3), "UU");
        press("ENTER=Continue");
      },
      requests: 1,
      text: SELECTION_TOO_LONG,
      search: UNDESCRIBED,
      marked: 3,
    },
  ])(
    "describes each input by rendered elements only, $state",
    async ({ replies, act, requests, text, search, marked }) => {
      const sent = stubFetch(...replies);
      renderList();
      await listed(sent, 1, "ADMIN001");
      act();
      await shown(sent, requests, text);

      expect(danglingDescriptions()).toEqual([]);
      expect({
        search: readOut(searchField()),
        selectors: Array.from({ length: 10 }, (_, index) => readOut(selector(index + 1))),
      }).toEqual({
        search,
        selectors: Array.from({ length: 10 }, (_, index) =>
          index + 1 === marked ? markedWith(text) : UNDESCRIBED,
        ),
      });
    },
  );
});

// The sign-on and admin guards live in the application's route tree, not in the screen, so this
// group renders that tree rather than the screen beside stubs.
describe("COUSR00 behind the route guard", () => {
  // audit: COUSR00C-002
  it.each([
    {
      visitor: "a browser with no session",
      signOn: () => {
        clearSession();
      },
      target: PATHS.COSGN00,
      landed: () => screen.getByRole("button", { name: "ENTER=Sign-on" }),
      // The sign-on screen sends nothing until ENTER.
      requested: [],
    },
    {
      visitor: "a session of user type U",
      signOn: () => {
        setSession(TOKEN, "USER0001", "U");
      },
      target: PATHS.COMEN01,
      landed: () => screen.getByRole("heading", { level: 1, name: "Main Menu" }),
      // The main menu loads its own options from endpoint 2 once it mounts (COMEN01C-005).
      requested: ["/api/v1/menus/main"],
    },
  ])(
    "sends $visitor from /users to $target, with no request to endpoint 16",
    async ({ signOn, target, landed, requested }) => {
      signOn();
      const sent = stubFetch();
      const client = new QueryClient({
        defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
      });
      const router = createMemoryRouter(routes, { initialEntries: [PATHS.COUSR00] });
      render(
        <QueryClientProvider client={client}>
          <RouterProvider router={router} />
        </QueryClientProvider>,
      );

      // Only the target screen loads, through the route tree; the refused screen is never imported.
      await waitFor(
        () => {
          expect(router.state.location.pathname).toBe(target);
          expect(landed()).toBeDefined();
        },
        { timeout: 4000 },
      );
      expect(screen.queryByRole("heading", { level: 1, name: "List Users" })).toBeNull();
      expect(screen.queryByLabelText("Search User ID:")).toBeNull();
      await waitFor(() => {
        expect(sent.map((request) => request.path)).toEqual(requested);
      });
      // The refused screen never mounts, so nothing on screen marks the end of a wait: a bounded
      // window, then no request in flight. The count covers only requests sent within that window.
      await lingers();
      await settled(client);
      expect(sent.map((request) => request.path)).toEqual(requested);
      expect(sent.filter((request) => request.path.startsWith(LIST_PATH))).toEqual([]);
      router.dispose();
    },
  );
});

/** The 401 of the `AdminUser` extractor for a missing, invalid or expired token. */
const SIGNON_REQUIRED: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};

/**
 * Renders the application's route table at `/users`, so the admin guard that only the table
 * mounts stands in front of the screen. Each screen is imported by its route's loader, so a wait
 * for one to render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COUSR00] });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

// A 401 clears the stored session (`api/client.ts`), and the admin guard in front of the route
// replaces the still-mounted screen with sign-on at once: after the list request the screen sent,
// the test presses no key and follows no link.
describe("COUSR00 in the route table: a 401 ends the session", () => {
  it.each([
    {
      title: "the list of the first entry",
      replies: [json(401, SIGNON_REQUIRED)],
      drive: async (sent: ReturnType<typeof stubFetch>) => {
        await waitFor(() => {
          expect(sent).toHaveLength(1);
        });
      },
      count: 1,
    },
    {
      title: "the F8 page after page 1",
      replies: [json(200, FIRST_PAGE), json(401, SIGNON_REQUIRED)],
      drive: async (sent: ReturnType<typeof stubFetch>) => {
        await listed(sent, 1, "ADMIN001");
        press("F8=Forward");
      },
      count: 2,
    },
  ])(
    "$title answered 401 replaces the screen with sign-on and clears the session, with no retry",
    async ({ replies, drive, count }) => {
      const sent = stubFetch(...replies);
      const router = renderRouteTable();

      await drive(sent);

      expect(
        await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
      ).toBeDefined();
      expect(router.state.location.pathname).toBe(PATHS.COSGN00);
      expect(screen.queryByRole("heading", { level: 1, name: "List Users" })).toBeNull();
      expect(getSession()).toBeNull();
      expect(sent).toHaveLength(count);
      for (const request of sent) {
        expect([request.method, request.path, request.authorization]).toEqual([
          "GET",
          LIST_PATH,
          `Bearer ${TOKEN}`,
        ]);
      }
      router.dispose();
    },
  );
});

/**
 * A route `lazy` that stays loading until `load` is called, as a screen's module does on a slow
 * network, then renders `Component`.
 */
function slowRoute(Component: ComponentType): {
  readonly lazy: () => Promise<{ Component: ComponentType }>;
  readonly load: () => void;
} {
  let resolveModule: () => void = () => undefined;
  const loaded = new Promise<{ Component: ComponentType }>((resolve) => {
    resolveModule = () => {
      resolve({ Component });
    };
  });
  return {
    lazy: () => loaded,
    load: () => {
      resolveModule();
    },
  };
}

/** The screens COUSR00 transfers to, each a stub showing the route state it received. */
const TRANSFER_TARGETS = [
  { path: PATHS.COUSR02, name: "Update User" },
  { path: PATHS.COUSR03, name: "Delete User" },
  { path: PATHS.COADM01, name: "Admin Menu" },
] as const;

/**
 * Renders `/users` in a data router, as the application does. The target at `slowPath` loads
 * through `slow`; the other targets render at once, so a key that transferred elsewhere would
 * show. With `outlet`, the screens sit under the application's root element, `ScreenOutlet`,
 * which tells the frame that a transfer is under way; without it only the screen's own lock acts.
 * Returns the router, whose location and navigation the test reads, and the query client, whose
 * `isFetching` and `isMutating` tell when the screen's requests settled.
 */
function renderSlowTransfer(
  slowPath: string,
  slow: ReturnType<typeof slowRoute>,
  outlet: boolean,
): { router: ReturnType<typeof createMemoryRouter>; client: QueryClient } {
  const screens: RouteObject[] = [
    { path: PATHS.COUSR00, element: <COUSR00 /> },
    ...TRANSFER_TARGETS.map(
      ({ path, name }): RouteObject =>
        path === slowPath
          ? { path, lazy: slow.lazy }
          : { path, element: <TransferStub name={name} /> },
    ),
  ];
  const router = createMemoryRouter(
    outlet ? [{ Component: ScreenOutlet, children: screens }] : screens,
    { initialEntries: [PATHS.COUSR00] },
  );
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router, client };
}

/** Selects row 6 (`USER0001`) with `code` and presses ENTER. */
function selectRow6(code: string): void {
  typeInto(selector(6), code);
  press("ENTER=Continue");
}

/** The route state a `U` or `D` on row 6 of {@link FIRST_PAGE} hands over (COUSR00C-008, -009). */
const ROW_6_TRANSFER = {
  from: PATHS.COUSR00,
  user_id: "USER0001",
  first_key: "ADMIN001",
  last_key: "USER0005",
};

// A transfer leaves the list on screen until the next screen's module has loaded. The keyboard
// stays locked through that wait, as the terminal's does until the next program sends its map:
// the screen's own lock holds through a selection's transfer, and under the application's root
// element the frame locks every key of any transfer, F3's included.
describe("COUSR00 while the screen it transfers to loads", () => {
  it.each([
    {
      how: "U on row 6 and ENTER",
      outlet: false,
      start: () => {
        selectRow6("U");
      },
      ownLock: true,
      path: PATHS.COUSR02,
      name: "Update User",
      state: ROW_6_TRANSFER,
    },
    {
      how: "D on row 6 and ENTER",
      outlet: false,
      start: () => {
        selectRow6("D");
      },
      ownLock: true,
      path: PATHS.COUSR03,
      name: "Delete User",
      state: ROW_6_TRANSFER,
    },
    {
      how: "U on row 6 and ENTER",
      outlet: true,
      start: () => {
        selectRow6("U");
      },
      ownLock: true,
      path: PATHS.COUSR02,
      name: "Update User",
      state: ROW_6_TRANSFER,
    },
    {
      how: "F3",
      outlet: true,
      start: () => {
        press("F3=Back");
      },
      ownLock: false,
      path: PATHS.COADM01,
      name: "Admin Menu",
      state: null,
    },
  ])(
    "after $how (root element: $outlet) no key pages or transfers again, and $name opens once loaded",
    async ({ outlet, start, ownLock, path, name, state }) => {
      const sent = stubFetch(json(200, FIRST_PAGE), json(200, SECOND_PAGE));
      const slow = slowRoute(() => <TransferStub name={name} />);
      const { router, client } = renderSlowTransfer(path, slow, outlet);
      await listed(sent, 1, "ADMIN001");

      start();
      await waitFor(() => {
        expect(router.state.navigation.state).toBe("loading");
        expect(
          screen.getByRole("group", { name: "Function keys" }).getAttribute("aria-busy"),
        ).toBe("true");
      });
      // A selection's transfer keeps the lock ENTER took: the form is busy, every input read-only.
      expect(mapForm().getAttribute("aria-busy")).toBe(String(ownLock));
      expect(readOnlyStates()).toEqual(Array.from({ length: 11 }, () => ownLock));

      // The keys of the slow-network report: F8 and F7 would page, F2 would show the invalid-key
      // message, and a second selection with ENTER would replace the transfer.
      press("F8=Forward");
      press("F7=Backward");
      pressKey("F8");
      pressKey("F7");
      pressKey("F2");
      typeInto(selector(7), "D");
      press("ENTER=Continue");
      pressKey("Enter");
      // Every key is refused and shows nothing: a bounded window, then no request in flight.
      await lingers();
      await settled(client);

      expect(sent).toHaveLength(1);
      expect(router.state.location.pathname).toBe(PATHS.COUSR00);
      expect(errorRow().textContent).toBe("");

      slow.load();
      await screen.findByRole("region", { name });
      expect(router.state.location.pathname).toBe(path);
      expect(transferState(name)).toEqual(state);
      expect(sent).toHaveLength(1);
      router.dispose();
    },
  );
});
