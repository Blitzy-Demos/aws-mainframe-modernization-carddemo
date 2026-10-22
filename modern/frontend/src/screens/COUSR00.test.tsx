import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { components } from "@/api/schema";
import { setSession } from "@/auth/session";
import { INVALID_KEY, TITLE01, TITLE02 } from "@/lib/header";
import { PATHS } from "@/routes";
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
 * Replaces the global `fetch`. Each `GET /api/v1/users` takes the next of `replies`; any other
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
        return reply();
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

/** Renders the screen at `/users` with `state`, beside stubs at its transfer targets. */
function renderList(state?: unknown) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
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

/** The text of the header field `name` (`TRNNAME`, `CURDATE`, …). */
function headerField(name: string): string | null | undefined {
  return document.querySelector(`[data-field="${name}"]`)?.textContent;
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
    const sent = stubFetch(json(200, FIRST_PAGE));
    renderList();
    await listed(sent, 1, "ADMIN001");

    expect(sent[0].authorization).toBe(`Bearer ${TOKEN}`);

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("List Users");
    /*
     * The header labels and value fields render as plain text rather than the `Label` and
     * read-only `Input` that rows COUSR00-001 to COUSR00-010 name, so only their placement is
     * asserted here, and only the values the program moves (COUSR00-090 to COUSR00-095) are tagged.
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
    expect(headerField("TITLE01")).toBe(TITLE01);
    expect(headerField("TITLE02")).toBe(TITLE02);
    // Time-dependent fields are compared by format only.
    expect(headerField("CURDATE")).toMatch(/^\d{2}\/\d{2}\/\d{2}$/);
    expect(headerField("CURTIME")).toMatch(/^\d{2}:\d{2}:\d{2}$/);

    // `Page:` POS=(4,65) labels PAGENUM, POS=(4,71) LENGTH=8: an ASKIP output the cursor skips.
    expect(screen.getByText("Page:").parentElement?.className).toBe(
      "row-start-4 col-start-65 col-span-5",
    );
    expect(pageField().hasAttribute("readonly")).toBe(true);
    expect(pageField().getAttribute("tabindex")).toBe("-1");
    expect(pageField().parentElement?.className).toBe("row-start-4 col-start-71 col-span-8");
    expect(valueOf(pageField())).toBe("00000001");

    // `Search User ID:` POS=(6,5) labels USRIDIN, POS=(6,21) LENGTH=8.
    expect(screen.getByText("Search User ID:").parentElement?.className).toBe(
      "row-start-6 col-start-5 col-span-15",
    );
    expect(searchField().getAttribute("maxlength")).toBe("8");
    expect(searchField().parentElement?.className).toBe("row-start-6 col-start-21 col-span-8");
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

  it("keeps the keyboard locked while a request is outstanding", async () => {
    const releases: ((reply: Response) => void)[] = [];
    const outstanding = new Promise<Response>((resolve) => {
      releases.push(resolve);
    });
    const sent = stubFetch(() => outstanding, json(200, SECOND_PAGE));
    renderList();
    await waitFor(() => {
      expect(sent).toHaveLength(1);
    });

    // ENTER, F7 and F8 are ignored until the entry's answer arrives.
    press("ENTER=Continue");
    press("F7=Backward");
    press("F8=Forward");
    pressKey("F8");
    expect(releases).toHaveLength(1);
    releases[0](json(200, FIRST_PAGE)());
    await listed(sent, 1, "ADMIN001");
    expect(valueOf(pageField())).toBe("00000001");

    // Once answered, the next key is served.
    press("F8=Forward");
    await listed(sent, 2, "USER0006");
    expect(sent[1].query).toMatchObject({ direction: "forward", page_key: "USER0005" });
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
    expect(document.activeElement).toBe(searchField());
  });
});
