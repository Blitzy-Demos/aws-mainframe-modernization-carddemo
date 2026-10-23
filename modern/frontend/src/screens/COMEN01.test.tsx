import { onlineManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import {
  createMemoryRouter,
  Link,
  MemoryRouter,
  Route,
  RouterProvider,
  Routes,
  useLocation,
} from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { REQUEST_DEADLINE_MS } from "@/api/client";
import type { components } from "@/api/schema";
import { SESSION_KEY, getSession, setSession } from "@/auth/session";
import { PATHS, PROGRAM_ROUTES, routes } from "@/routes";
import { COMEN01 } from "./COMEN01";

/**
 * Render test of the main-menu screen: map `COMEN1A` (`app/bms/COMEN01.bms`), transaction `CM00`,
 * program `COMEN01C`, route `/menu`. It proves the `client` rows of `modern/audit/screens.md`
 * (`COMEN01-0nn`) and `modern/audit/online/COMEN01C.md` (`COMEN01C-0nn`) that a stubbed endpoint 2
 * response can drive: the first entry, the ENTER edit and checks (the branches the shipped table
 * never takes driven by stubbed flags), the transfers, F3, the invalid key and the header.
 *
 * The global `fetch` is a stub that answers `GET /api/v1/menus/main` in process and every other
 * request with a 500, so nothing leaves the test. Every navigation target is a stub that shows its
 * path and the route state it received, with a link back to the menu.
 *
 * The rows of `COMEN01C-003` and `COMEN01C-010` are blocked by halt candidates `X-COMEN01C-2` and
 * `X-COMEN01C-1`: their tests are reported BLOCKED and assert only the replacement's path the rows
 * state, never a COBOL result. Rows `COMEN01-001` to `COMEN01-010` place the header labels in a
 * `Label` and the header values in a read-only `Input`, as the shared `ScreenShell` renders them.
 * The label rows are tagged in NAV. The value rows are tagged here, on each value's read-only
 * `Input`, its width, its map `POS` and `LENGTH`, its label and its content, as are their content
 * rows (`COMEN01-029` to `COMEN01-034`).
 */

const TOKEN = "test-token";

/*
 * The header titles and the invalid-key text are written out from their copybooks rather than
 * imported from `@/lib/header`, which the screen itself uses, so a wrong value there fails here.
 */

/** `CCDA-TITLE01 PIC X(40)`, header row 1 (`app/cpy/COTTL01Y.cpy:18-19`, COMEN01-030). */
const TITLE01 = "      AWS Mainframe Modernization       ";

/** `CCDA-TITLE02 PIC X(40)`, header row 2 (`app/cpy/COTTL01Y.cpy:20-22`, COMEN01-033). */
const TITLE02 = "              CardDemo                  ";

/** `CCDA-MSG-INVALID-KEY PIC X(50)`, padded to its 50 bytes (`app/cpy/CSMSG01Y.cpy:20-21`). */
const INVALID_KEY = "Invalid key pressed. Please see below...".padEnd(50, " ");

/** `'Please enter a valid option number...'` (COMEN01C.cbl:131-132, COMEN01C-011). */
const INVALID_OPTION = "Please enter a valid option number...";

/** `'No access - Admin Only option... '`, its trailing space included (COMEN01C-011). */
const ADMIN_ONLY = "No access - Admin Only option... ";

/** Option 11's not-installed text as the `STRING` composes it (COMEN01C-015). */
const NOT_INSTALLED = "This option Pending Authorization View is not installed...";

/** A row no option fills: the 40 spaces of `OPTN012` (COMEN01-023). */
const BLANK_ROW = " ".repeat(40);

/** The 11 entries of `CARDDEMO-MAIN-MENU-OPTIONS` (`app/cpy/COMEN02Y.cpy:25-90`). */
const SHIPPED = [
  ["01", "Account View", "COACTVWC"],
  ["02", "Account Update", "COACTUPC"],
  ["03", "Credit Card List", "COCRDLIC"],
  ["04", "Credit Card View", "COCRDSLC"],
  ["05", "Credit Card Update", "COCRDUPC"],
  ["06", "Transaction List", "COTRN00C"],
  ["07", "Transaction View", "COTRN01C"],
  ["08", "Transaction Add", "COTRN02C"],
  ["09", "Transaction Reports", "CORPT00C"],
  ["10", "Bill Payment", "COBIL00C"],
  ["11", "Pending Authorization View", "COPAUS0C"],
] as const;

/**
 * Endpoint 2's answer for the shipped table (COMEN01C-013, COMEN01C-015, COMEN01C-020,
 * COMEN01C-021): every option installed and of user type `U`, each with its 35-byte name and a
 * text of the 2-digit number, `. `, the 35-byte name and one space.
 */
const MENU: components["schemas"]["MenuResponse"] = {
  options: SHIPPED.map(([number, name, program]) => ({
    number,
    name: name.padEnd(35, " "),
    text: `${number}. ${name.padEnd(35, " ")} `,
    program,
    user_type: "U",
    admin_only: false,
    installed: true,
    unavailable_message: number === "11" ? NOT_INSTALLED : null,
  })),
  invalid_option_message: INVALID_OPTION,
  admin_only_message: ADMIN_ONLY,
  user_type: "U",
};

/** The shipped table with option 11 not installed, which this build never sends (COMEN01C-016). */
const NOT_INSTALLED_MENU: components["schemas"]["MenuResponse"] = {
  ...MENU,
  options: MENU.options.map((option) =>
    option.number === "11" ? { ...option, installed: false } : option,
  ),
};

/** The shipped table with option 05 of user type `A`, as no shipped option is (COMEN01C-012). */
const ADMIN_ONLY_MENU: components["schemas"]["MenuResponse"] = {
  ...MENU,
  options: MENU.options.map((option) =>
    option.number === "05" ? { ...option, user_type: "A", admin_only: true } : option,
  ),
};

/** {@link ADMIN_ONLY_MENU} as endpoint 2 answers an `A` token. */
const ADMIN_ONLY_MENU_FOR_A: components["schemas"]["MenuResponse"] = {
  ...ADMIN_ONLY_MENU,
  user_type: "A",
};

/**
 * The shipped table with two programs the route table lacks: option 03 with an unavailable text,
 * option 04 with none and a name that `Object.prototype` holds.
 */
const UNROUTED_MENU: components["schemas"]["MenuResponse"] = {
  ...MENU,
  options: MENU.options.map((option) => {
    if (option.number === "03") {
      return {
        ...option,
        program: "COXXXX0C",
        unavailable_message: "This option Credit Card List is not installed...",
      };
    }
    return option.number === "04"
      ? { ...option, program: "constructor", unavailable_message: null }
      : option;
  }),
};

/** Every request the screen sent in the current test, in order. */
const requests: Request[] = [];

/** The requests to endpoint 2 among {@link requests}. */
function menuRequests(): Request[] {
  return requests.filter((request) => new URL(request.url).pathname === "/api/v1/menus/main");
}

/** A JSON reply, as the API sends every success and every `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces the global `fetch`: `GET /api/v1/menus/main` gets `answer`'s reply, any other request a
 * 500. Every request is recorded in {@link requests}.
 */
function stubFetch(answer: () => Promise<Response> | Response): void {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init);
      requests.push(request);
      const { pathname } = new URL(request.url);
      if (request.method === "GET" && pathname === "/api/v1/menus/main") {
        return answer();
      }
      const unexpected: components["schemas"]["ScreenError"] = {
        message: `unexpected ${request.method} ${pathname}`,
      };
      return json(500, unexpected);
    }),
  );
}

/** Endpoint 2 answers 200 with `menu` on every call. */
function serve(menu: components["schemas"]["MenuResponse"]): void {
  stubFetch(() => json(200, menu));
}

/** A promise the test settles by calling `release`, to hold a reply back. */
function deferred(): { readonly done: Promise<void>; readonly release: () => void } {
  let release = (): void => undefined;
  const done = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { done, release: () => release() };
}

/** Every screen the menu can reach. */
const TARGETS = Object.values(PATHS).filter((path) => path !== PATHS.COMEN01);

/** A navigation target: its path, the route state it received and a link back to the menu. */
function Target() {
  const { pathname, state } = useLocation();
  return (
    <section aria-label="Target screen">
      <h2>{`arrived at ${pathname}`}</h2>
      <pre aria-label="Route state">{JSON.stringify(state ?? null)}</pre>
      <Link to={PATHS.COMEN01}>Back to the menu</Link>
    </section>
  );
}

/** Renders `/menu`, entered with route `state`, under a fresh client that never retries. */
function renderMenu(state?: Record<string, unknown>): QueryClient {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COMEN01, state }]}>
        <Routes>
          <Route path={PATHS.COMEN01} element={<COMEN01 />} />
          {TARGETS.map((path) => (
            <Route key={path} path={path} element={<Target />} />
          ))}
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return client;
}

/** The 12 option rows, `OPTN001` to `OPTN012`. */
function optionCells(): HTMLElement[] {
  return within(screen.getByRole("table", { name: "Menu options" })).getAllByRole("cell");
}

/** Waits until the rows show `menu`'s 11 texts and the blank 12th row. */
async function loaded(menu: components["schemas"]["MenuResponse"] = MENU): Promise<void> {
  await waitFor(() => {
    expect(optionCells().map((cell) => cell.textContent)).toEqual([
      ...menu.options.map((option) => option.text),
      BLANK_ROW,
    ]);
  });
}

/** The `OPTION` input, found through its label (COMEN01-024). */
function optionInput(): HTMLInputElement {
  const input = screen.getByLabelText("Please select an option :");
  if (!(input instanceof HTMLInputElement)) {
    throw new TypeError("the OPTION field is not an input");
  }
  return input;
}

/** The `ERRMSG` row (COMEN01-027). */
function errorRow(): HTMLElement {
  const rows = screen
    .getAllByRole("alert")
    .filter((row) => row.getAttribute("data-message-kind") === "error");
  expect(rows).toHaveLength(1);
  return rows[0];
}

/** The `ENTER=Continue` button (COMEN01-035). */
function enterButton(): HTMLElement {
  return screen.getByRole("button", { name: "ENTER=Continue" });
}

/**
 * Moves the focus to `element`, as a click on it does. The input's blur re-renders the form's
 * field state; inside `waitFor` that render runs outside React's act environment, which expects it.
 */
async function focusOn(element: HTMLElement): Promise<void> {
  await waitFor(() => {
    element.focus();
    expect(document.activeElement).toBe(element);
  });
}

/** Types `typed` into the option input and clicks ENTER, which takes the focus. */
async function enterOption(typed: string): Promise<void> {
  fireEvent.change(optionInput(), { target: { value: typed } });
  await focusOn(enterButton());
  fireEvent.click(enterButton());
}

/** The menu's `<form>`, which holds the option input. */
function menuForm(): HTMLFormElement {
  const form = optionInput().form;
  if (form === null) {
    throw new TypeError("the OPTION field is not in a form");
  }
  return form;
}

/**
 * Whether the screen shows the locked keyboard of a load in progress: ENTER disabled, the key row
 * and the form busy, the option input read-only; or, with `busy` false, all of them free. F3 is
 * free either way.
 */
function expectBusy(busy: boolean): void {
  expect(enterButton().getAttribute("aria-disabled")).toBe(busy ? "true" : null);
  expect(screen.getByRole("button", { name: "F3=Exit" }).getAttribute("aria-disabled")).toBeNull();
  expect(screen.getByRole("group", { name: "Function keys" }).getAttribute("aria-busy")).toBe(
    String(busy),
  );
  expect(menuForm().getAttribute("aria-busy")).toBe(String(busy));
  expect(optionInput().readOnly).toBe(busy);
}

/**
 * Gives a refused key the time its submit handler would take to decide and render. The wait runs
 * inside `act`, as the form's own state updates after an ENTER do.
 */
async function lingers(): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => {
      setTimeout(resolve, 50);
    });
  });
}

/** Lets a pending submit handler and the renders it schedules finish: `waitFor` drains them. */
async function settled(): Promise<void> {
  await waitFor(() => {
    expect(screen.getByRole("heading", { level: 1, name: "Main Menu" })).toBeTruthy();
  });
}

/** Asserts that the menu is still shown and nothing was transferred to. */
function expectNoTransfer(): void {
  expect(screen.getByRole("heading", { level: 1, name: "Main Menu" })).toBeTruthy();
  expect(screen.queryByRole("region", { name: "Target screen" })).toBeNull();
}

/**
 * ENTER with `typed`: the menu stays, the error row shows `message`, the input shows the 2-digit
 * value `shown`, the cursor is back on the input, and no request follows the load.
 */
async function expectStay(typed: string, message: string, shown: string): Promise<void> {
  await enterOption(typed);
  await waitFor(() => {
    expect(optionInput().value).toBe(shown);
    expect(errorRow().textContent).toBe(message);
  });
  expect(document.activeElement).toBe(optionInput());
  expectNoTransfer();
  expect(requests).toHaveLength(1);
}

/** The `CardHeader` that holds header rows 1 and 2 and the screen title. */
function cardHeader(): HTMLElement {
  const header = document.querySelector('[data-slot="card-header"]');
  if (!(header instanceof HTMLElement)) {
    throw new TypeError("the screen has no card header");
  }
  return header;
}

/**
 * The header field `name` of rows 1 and 2 (`data-field`): the one `Input` of that name in the
 * `CardHeader`, failing the test when there is not exactly one.
 */
function headerInput(name: string): HTMLInputElement {
  const fields = cardHeader().querySelectorAll(`input[data-field="${name}"]`);
  expect(fields).toHaveLength(1);
  const [field] = fields;
  if (!(field instanceof HTMLInputElement)) {
    throw new TypeError(`the header has no input ${name}`);
  }
  return field;
}

/** The value of the header field `name` of rows 1 and 2. */
function headerValue(name: string): string {
  return headerInput(name).value;
}

/** Header rows 1 and 2 as `POPULATE-HEADER-INFO` fills them (COMEN01C-019). */
function expectHeader(): void {
  expect(headerValue("TRNNAME")).toBe("CM00");
  expect(headerValue("PGMNAME")).toBe("COMEN01C");
  expect(headerValue("TITLE01")).toBe(TITLE01);
  expect(headerValue("TITLE02")).toBe(TITLE02);
  expect(headerValue("CURDATE")).toMatch(/^\d{2}\/\d{2}\/\d{2}$/);
  expect(headerValue("CURTIME")).toMatch(/^\d{2}:\d{2}:\d{2}$/);
}

beforeEach(() => {
  requests.length = 0;
  setSession(TOKEN, "USER0001", "U");
});

afterEach(async () => {
  try {
    // Bounded Pagination: the menu is a single resource, so no request carries a page size.
    for (const request of requests) {
      expect(new URL(request.url).searchParams.has("page_size")).toBe(false);
      expect(await request.clone().text()).not.toContain("page_size");
    }
  } finally {
    vi.unstubAllGlobals();
    sessionStorage.clear();
  }
});

describe("COMEN01 first entry", () => {
  it("loads endpoint 2 once and shows the map with an empty message row", async () => {
    // audit: COMEN01C-005
    // audit: COMEN01C-019
    // audit: COMEN01C-022
    // audit: COMEN01-011
    // audit: COMEN01-012
    // audit: COMEN01-013
    // audit: COMEN01-014
    // audit: COMEN01-015
    // audit: COMEN01-016
    // audit: COMEN01-017
    // audit: COMEN01-018
    // audit: COMEN01-019
    // audit: COMEN01-020
    // audit: COMEN01-021
    // audit: COMEN01-022
    // audit: COMEN01-023
    // audit: COMEN01-024
    // audit: COMEN01-025
    // audit: COMEN01-026
    // audit: COMEN01-028
    // audit: COMEN01-029
    // audit: COMEN01-030
    // audit: COMEN01-031
    // audit: COMEN01-032
    // audit: COMEN01-033
    // audit: COMEN01-034
    serve(MENU);
    renderMenu();
    await loaded();

    const title = screen.getByRole("heading", { level: 1, name: "Main Menu" });
    expect(title.getAttribute("data-slot")).toBe("card-title");
    expect(cardHeader().contains(title)).toBe(true);
    const cells = optionCells();
    expect(cells.map((cell) => cell.getAttribute("data-field"))).toEqual(
      Array.from({ length: 12 }, (_, index) => `OPTN${String(index + 1).padStart(3, "0")}`),
    );
    expect(cells.every((cell) => cell.getAttribute("data-slot") === "table-cell")).toBe(true);

    // The header labels are shown, each the `Label` of the value after it.
    for (const label of ["Tran:", "Date:", "Prog:", "Time:"]) {
      expect(within(cardHeader()).getByText(label)).toBeTruthy();
    }
    expectHeader();
    expect(headerValue("TITLE01")).toHaveLength(40);
    expect(headerValue("TITLE02")).toHaveLength(40);

    const input = optionInput();
    // `FormControl` passes its slot name to the `Input` it wraps, inside the field's `FormItem`.
    expect(input.getAttribute("data-slot")).toBe("form-control");
    expect(input.closest('[data-slot="form-item"]')).not.toBeNull();
    expect(input.maxLength).toBe(2);
    expect(input.getAttribute("inputmode")).toBe("numeric");
    expect(input.value).toBe("");
    expect(document.activeElement).toBe(input);
    const label = screen.getByText("Please select an option :");
    expect(label.getAttribute("data-slot")).toBe("form-label");
    expect(label.getAttribute("for")).toBe(input.id);
    // The zero-length stopper at `POS=(20,44)` has no element: nothing starts in that cell.
    expect(document.querySelector('[class~="row-start-20"][class~="col-start-44"]')).toBeNull();

    const row = errorRow();
    expect(row.getAttribute("data-slot")).toBe("alert");
    expect(row.getAttribute("aria-live")).toBe("polite");
    expect(row.className).toContain("text-destructive");
    expect(row.textContent).toBe("");

    const keyBar = screen.getByRole("group", { name: "Function keys" });
    const keys = within(keyBar).getAllByRole("button");
    expect(keys.map((key) => key.textContent)).toEqual(["ENTER=Continue", "F3=Exit"]);

    expect(requests).toHaveLength(1);
    const [load] = requests;
    expect(load.method).toBe("GET");
    expect(new URL(load.url).pathname).toBe("/api/v1/menus/main");
    expect(new URL(load.url).search).toBe("");
    expect(load.headers.get("Authorization")).toBe(`Bearer ${TOKEN}`);
  });

  it.each([
    // audit: COMEN01-002
    {
      id: "COMEN01-002",
      name: "TRNNAME",
      row: 1,
      col: 7,
      length: 4,
      label: "Tran:",
      shows: "CM00",
    },
    // audit: COMEN01-003
    {
      id: "COMEN01-003",
      name: "TITLE01",
      row: 1,
      col: 21,
      length: 40,
      label: null,
      shows: TITLE01,
    },
    // audit: COMEN01-005
    {
      id: "COMEN01-005",
      name: "CURDATE",
      row: 1,
      col: 71,
      length: 8,
      label: "Date:",
      shows: expect.stringMatching(/^\d{2}\/\d{2}\/\d{2}$/),
    },
    // audit: COMEN01-007
    {
      id: "COMEN01-007",
      name: "PGMNAME",
      row: 2,
      col: 7,
      length: 8,
      label: "Prog:",
      shows: "COMEN01C",
    },
    // audit: COMEN01-008
    {
      id: "COMEN01-008",
      name: "TITLE02",
      row: 2,
      col: 21,
      length: 40,
      label: null,
      shows: TITLE02,
    },
    // audit: COMEN01-010
    {
      id: "COMEN01-010",
      name: "CURTIME",
      row: 2,
      col: 71,
      length: 8,
      label: "Time:",
      shows: expect.stringMatching(/^\d{2}:\d{2}:\d{2}$/),
    },
  ])(
    "$id shows $name in a read-only Input at ($row,$col), $length wide, in the card header",
    async ({ name, row, col, length, label, shows }) => {
      // The header value is an output field of the map: a read-only `Input` that holds `LENGTH`
      // characters (`size`), in the cell of its `POS` and `LENGTH` in the frame's title rows. The
      // map's literal before it is its bound `Label`; a title, which no literal labels, is named
      // by `aria-label`.
      serve(MENU);
      renderMenu();
      await loaded();

      const field = headerInput(name);
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
      expect(within(cardHeader()).getByRole("textbox", { name: label ?? `Title row ${row}` })).toBe(
        field,
      );
      expect(requests).toHaveLength(1);
    },
  );
});

describe("COMEN01 ENTER", () => {
  it.each([
    { typed: "1", program: "COACTVWC", path: PATHS.COACTVW },
    { typed: "02", program: "COACTUPC", path: PATHS.COACTUP },
    { typed: "3", program: "COCRDLIC", path: PATHS.COCRDLI },
    { typed: "04", program: "COCRDSLC", path: PATHS.COCRDSL },
    { typed: "5", program: "COCRDUPC", path: PATHS.COCRDUP },
    { typed: "06", program: "COTRN00C", path: PATHS.COTRN00 },
    { typed: "7", program: "COTRN01C", path: PATHS.COTRN01 },
    { typed: "08", program: "COTRN02C", path: PATHS.COTRN02 },
    { typed: "9", program: "CORPT00C", path: PATHS.CORPT00 },
    { typed: "10", program: "COBIL00C", path: PATHS.COBIL00 },
    { typed: "11", program: "COPAUS0C", path: PATHS.COPAU00 },
  ])(
    "$typed transfers to $program at $path with the menu's route state and no request",
    async ({ typed, program, path }) => {
      // audit: COMEN01C-008
      // audit: COMEN01C-014
      // audit: COMEN01C-018
      // audit: COMEN01-035
      expect(PROGRAM_ROUTES[program]).toBe(path);
      serve(MENU);
      renderMenu({ acct_id: "00000000001" });
      await loaded();

      const enter = enterButton();
      expect(enter.getAttribute("type")).toBe("submit");
      expect(enter.getAttribute("data-variant")).toBe("default");
      // The ENTER button submits the form that holds the input, as the Enter key does.
      expect(enter instanceof HTMLButtonElement && enter.form).toBe(optionInput().form);

      await enterOption(typed);
      expect(await screen.findByText(`arrived at ${path}`)).toBeTruthy();
      expect(JSON.parse(screen.getByLabelText("Route state").textContent ?? "")).toEqual({
        acct_id: "00000000001",
        from: PATHS.COMEN01,
      });
      expect(requests).toHaveLength(1);
    },
  );

  it.each(["", "0", "00"])(
    "%j is option 00 and shows the invalid-option text with 00 in the input",
    async (typed) => {
      // audit: COMEN01C-008
      // audit: COMEN01C-009
      serve(MENU);
      renderMenu();
      await loaded();
      await expectStay(typed, INVALID_OPTION, "00");
      expect(errorRow().className).toContain("text-destructive");
    },
  );

  it("refuses an admin-only option to a U user with the admin-only text", async () => {
    // audit: COMEN01C-008
    // audit: COMEN01C-012
    serve(ADMIN_ONLY_MENU);
    renderMenu();
    await loaded(ADMIN_ONLY_MENU);
    await expectStay("5", ADMIN_ONLY, "05");
  });

  it("takes an admin-only option for a user type other than U", async () => {
    // audit: COMEN01C-012
    // The screen compares the user type endpoint 2 states, the verified token's claim.
    setSession(TOKEN, "ADMIN001", "A");
    serve(ADMIN_ONLY_MENU_FOR_A);
    renderMenu();
    await loaded(ADMIN_ONLY_MENU_FOR_A);
    await enterOption("5");
    expect(await screen.findByText(`arrived at ${PATHS.COCRDUP}`)).toBeTruthy();
    expect(requests).toHaveLength(1);
  });

  it("shows a not-installed option's text in red and transfers nowhere", async () => {
    // audit: COMEN01C-016
    serve(NOT_INSTALLED_MENU);
    renderMenu();
    await loaded(NOT_INSTALLED_MENU);
    await expectStay("11", NOT_INSTALLED, "11");
    expect(errorRow().className).toContain("text-destructive");
  });

  it.each([
    { typed: "3", message: "This option Credit Card List is not installed...", shown: "03" },
    { typed: "4", message: "", shown: "04" },
  ])(
    "option $shown, whose program has no screen, shows its unavailable text $message",
    async ({ typed, message, shown }) => {
      // The screen's own fallback for a program the route table lacks; no audit row covers it.
      serve(UNROUTED_MENU);
      renderMenu();
      await loaded(UNROUTED_MENU);
      await expectStay(typed, message, shown);
    },
  );

  it.each([
    {
      how: "a click on ENTER=Continue",
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
        fireEvent.submit(menuForm());
      },
    },
  ])("$how decides nothing while endpoint 2 loads or reloads", async ({ press }) => {
    // audit: COMEN01-035
    const load = deferred();
    const reload = deferred();
    stubFetch(async () => {
      await (menuRequests().length === 1 ? load.done : reload.done);
      return json(200, MENU);
    });
    const client = renderMenu();

    // First entry: the load is out. The input is read-only, so the entry is set, not typed.
    expectBusy(true);
    fireEvent.change(optionInput(), { target: { value: "1" } });
    press();
    press();
    await lingers();
    expectNoTransfer();
    expect(errorRow().textContent).toBe("");
    load.release();
    await loaded();
    expectBusy(false);

    await enterOption("1");
    expect(await screen.findByText(`arrived at ${PATHS.COACTVW}`)).toBeTruthy();
    fireEvent.click(screen.getByRole("link", { name: "Back to the menu" }));
    await waitFor(() => {
      expect(menuRequests()).toHaveLength(2);
    });

    // A return to the menu: the cached table shows while the reload is out.
    expectBusy(true);
    fireEvent.change(optionInput(), { target: { value: "6" } });
    press();
    press();
    await lingers();
    expectNoTransfer();
    expect(errorRow().textContent).toBe("");

    reload.release();
    await waitFor(() => {
      expect(client.isFetching()).toBe(0);
      expectBusy(false);
    });
    expectNoTransfer();
    press();
    expect(await screen.findByText(`arrived at ${PATHS.COTRN00}`)).toBeTruthy();
    expect(menuRequests()).toHaveLength(2);
  });

  it("serves ENTER on a return to the menu once its identical reload has settled", async () => {
    serve(MENU);
    const client = renderMenu();
    await loaded();
    await enterOption("1");
    expect(await screen.findByText(`arrived at ${PATHS.COACTVW}`)).toBeTruthy();

    fireEvent.click(screen.getByRole("link", { name: "Back to the menu" }));
    // The cached table shows at once while endpoint 2 is fetched again with an identical reply.
    expect(optionCells().map((cell) => cell.textContent)).toEqual([
      ...MENU.options.map((option) => option.text),
      BLANK_ROW,
    ]);
    // No ENTER is pressed during the reload: an ENTER that read the fetch state then would
    // subscribe the screen to it and hide a stale flag. A user's ENTER comes after the reload.
    await waitFor(() => {
      expect(menuRequests()).toHaveLength(2);
      expect(client.isFetching()).toBe(0);
    });

    await enterOption("6");
    expect(await screen.findByText(`arrived at ${PATHS.COTRN00}`)).toBeTruthy();
    expect(menuRequests()).toHaveLength(2);
  });
});

describe("COMEN01 function keys", () => {
  it.each([
    {
      how: "a click on F3=Exit",
      press: () => {
        fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));
      },
    },
    {
      how: "the F3 key",
      press: () => {
        fireEvent.keyDown(optionInput(), { key: "F3" });
      },
    },
  ])("F3 by $how goes to sign-on with no option check and no request", async ({ press }) => {
    // audit: COMEN01C-006
    // audit: COMEN01-036
    serve(MENU);
    renderMenu({ acct_id: "00000000001" });
    await loaded();
    const exit = screen.getByRole("button", { name: "F3=Exit" });
    expect(exit.getAttribute("type")).toBe("button");
    expect(exit.getAttribute("data-variant")).toBe("outline");
    expect(exit.getAttribute("data-size")).toBe("sm");

    fireEvent.change(optionInput(), { target: { value: "99" } });
    press();
    expect(await screen.findByText(`arrived at ${PATHS.COSGN00}`)).toBeTruthy();
    // COSGN00C is entered with no COMMAREA, and its own first entry ends the session.
    expect(screen.getByLabelText("Route state").textContent).toBe("null");
    expect(getSession()).not.toBeNull();
    expect(requests).toHaveLength(1);
  });

  it("F3 still goes to sign-on while endpoint 2 loads, and an unserved key is refused", async () => {
    // audit: COMEN01C-006
    // audit: COMEN01-036
    const load = deferred();
    stubFetch(async () => {
      await load.done;
      return json(200, MENU);
    });
    renderMenu();
    expectBusy(true);

    // The locked keyboard takes the unserved key without the invalid-key text.
    fireEvent.keyDown(optionInput(), { key: "F5" });
    await lingers();
    expect(errorRow().textContent).toBe("");

    fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));
    expect(await screen.findByText(`arrived at ${PATHS.COSGN00}`)).toBeTruthy();
    expect(screen.getByLabelText("Route state").textContent).toBe("null");
    load.release();
    expect(requests).toHaveLength(1);
  });

  it.each([
    { key: "F5", shiftKey: false },
    { key: "F12", shiftKey: false },
    { key: "F3", shiftKey: true },
  ])(
    "$key (shift $shiftKey) shows the invalid-key text and leaves the input as typed",
    async ({ key, shiftKey }) => {
      // audit: COMEN01C-007
      serve(MENU);
      renderMenu();
      await loaded();
      fireEvent.change(optionInput(), { target: { value: "7" } });
      await focusOn(enterButton());
      fireEvent.keyDown(optionInput(), { key, shiftKey });
      await waitFor(() => {
        expect(errorRow().textContent).toBe(INVALID_KEY);
      });
      expect(errorRow().className).toContain("text-destructive");
      expect(optionInput().value).toBe("7");
      expect(document.activeElement).toBe(optionInput());
      expectNoTransfer();
      expect(requests).toHaveLength(1);
    },
  );

  it("replaces the message of every key with the outcome of the next", async () => {
    // audit: COMEN01C-001
    // audit: COMEN01C-019
    serve(UNROUTED_MENU);
    renderMenu();
    await loaded(UNROUTED_MENU);
    expect(errorRow().textContent).toBe("");

    await expectStay("99", INVALID_OPTION, "99");
    fireEvent.keyDown(optionInput(), { key: "F5" });
    await waitFor(() => {
      expect(errorRow().textContent).toBe(INVALID_KEY);
    });
    // An outcome without a message leaves the row empty, not showing the key before.
    await expectStay("4", "", "04");
    expectHeader();
  });
});

describe("COMEN01 load failure", () => {
  it("shows the ScreenError message in the error row and keeps the session", async () => {
    // audit: COMEN01-027
    const failure: components["schemas"]["ScreenError"] = {
      message: "Unable to read the menu options",
    };
    stubFetch(() => json(500, failure));
    renderMenu();
    await waitFor(() => {
      expect(errorRow().textContent).toBe(failure.message);
    });
    const row = errorRow();
    expect(row.getAttribute("aria-live")).toBe("polite");
    expect(row.className).toContain("text-destructive");
    expect(optionCells().map((cell) => cell.textContent)).toEqual(
      Array.from({ length: 12 }, () => BLANK_ROW),
    );
    expect(getSession()).not.toBeNull();

    // With no table to match, ENTER decides nothing and the load's message stays.
    await enterOption("1");
    await settled();
    expect(errorRow().textContent).toBe(failure.message);
    expectNoTransfer();
    expect(requests).toHaveLength(1);
  });
});

describe("BLOCKED X-COMEN01C-2: a token endpoint 2 refuses, not a parity result", () => {
  it("goes to sign-on after the 401 clears the session, with no further request", async () => {
    // audit: COMEN01C-003
    const refused: components["schemas"]["ScreenError"] = {
      message: "The sign-on token is missing, invalid or expired",
    };
    stubFetch(() => json(401, refused));
    renderMenu();
    expect(await screen.findByText(`arrived at ${PATHS.COSGN00}`)).toBeTruthy();
    expect(getSession()).toBeNull();
    expect(requests).toHaveLength(1);
  });
});

describe("BLOCKED X-COMEN01C-1: options past the table and non-digits, not a parity result", () => {
  it.each([
    { typed: "12", shown: "12" },
    { typed: "99", shown: "99" },
    { typed: "a", shown: "0a" },
    { typed: "1x", shown: "1x" },
  ])("$typed shows the invalid-option text with $shown in the input", async ({ typed, shown }) => {
    // audit: COMEN01C-010
    serve(MENU);
    renderMenu();
    await loaded();
    await expectStay(typed, INVALID_OPTION, shown);
  });
});

/** A stored value that is no session, and what is wrong with it. */
const UNUSABLE_STORED_VALUES = [
  { stored: "not json", why: "is not JSON" },
  { stored: '{"token":', why: "is truncated JSON" },
  { stored: "null", why: "is JSON null" },
  { stored: '"text"', why: "is a JSON string" },
  { stored: "[]", why: "is a JSON array" },
  { stored: '{"userId":"USER0001","userType":"U"}', why: "lacks the token" },
  { stored: '{"token":"test-token","userType":"U"}', why: "lacks the user ID" },
  { stored: '{"token":"test-token","userId":"USER0001"}', why: "lacks the user type" },
  { stored: '{"token":"","userId":"USER0001","userType":"U"}', why: "has an empty token" },
  { stored: '{"token":"test-token","userId":"","userType":"U"}', why: "has an empty user ID" },
  {
    stored: '{"token":"test-token","userId":"USER0001","userType":""}',
    why: "has an empty user type",
  },
  { stored: '{"token":7,"userId":"USER0001","userType":"U"}', why: "has a numeric token" },
];

/** The error a browser raises when it blocks storage. */
function storageBlocked(): DOMException {
  return new DOMException("storage blocked", "SecurityError");
}

// The sign-on guard in front of `/menu` reads the stored session afresh at every render. Absent,
// unreadable, corrupt or wrongly shaped data, and a `sessionStorage` that is missing or refuses
// access, are no session: the visit goes to sign-on before the menu requests endpoint 2.
describe("COMEN01 in the route table: no usable session", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  /** Renders the application's route table at `/menu`; returns the router, for its location. */
  function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, { initialEntries: [PATHS.COMEN01] });
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
    return router;
  }

  it.each([
    {
      why: "nothing is stored",
      install: () => {
        sessionStorage.removeItem(SESSION_KEY);
      },
    },
    ...UNUSABLE_STORED_VALUES.map(({ stored, why }) => ({
      why: `the stored value ${why}`,
      install: () => {
        sessionStorage.setItem(SESSION_KEY, stored);
      },
    })),
    {
      why: "reading storage throws",
      install: () => {
        vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
          throw storageBlocked();
        });
      },
    },
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
  ])("sends /menu to /signon with no request when $why", async ({ install }) => {
    serve(MENU);
    install();
    const router = renderRouteTable();

    expect(await screen.findByRole("button", { name: "ENTER=Sign-on" })).toBeDefined();
    expect(router.state.location.pathname).toBe(PATHS.COSGN00);
    expect(screen.queryByRole("heading", { level: 1, name: "Main Menu" })).toBeNull();
    expect(requests).toHaveLength(0);
    router.dispose();
  });

  it("reads the session afresh: one removed from storage directly is gone at the next navigation", async () => {
    serve(MENU);
    const router = renderRouteTable();
    await loaded();
    expect(menuRequests()).toHaveLength(1);

    sessionStorage.removeItem(SESSION_KEY);
    await act(async () => {
      await router.navigate(PATHS.COACTVW);
    });

    expect(await screen.findByRole("button", { name: "ENTER=Sign-on" })).toBeDefined();
    expect(router.state.location.pathname).toBe(PATHS.COSGN00);
    expect(requests).toHaveLength(1);
    router.dispose();
  });
});

/**
 * The answer `fetch` gives a request the server accepted and never completes: it settles only when
 * the request's signal aborts, at once for a signal already aborted when `fetch` receives it, and
 * then rejects with the abort reason, as `fetch` does.
 */
function unansweredUntilAborted(request: Request): Promise<Response> {
  return new Promise((_answer, fail) => {
    const abandon = (): void => {
      fail(request.signal.reason);
    };
    if (request.signal.aborted) {
      abandon();
    } else {
      request.signal.addEventListener("abort", abandon, { once: true });
    }
  });
}

/** The `name` of `signal`'s abort reason; `undefined` for a reason that is no `DOMException`. */
function reasonName(signal: AbortSignal): string | undefined {
  const reason: unknown = signal.reason;
  return reason instanceof DOMException ? reason.name : undefined;
}

/**
 * Spies on `setTimeout` and `clearTimeout`, both passing through. Returns a function that tells,
 * for every request-deadline timer armed since (the `setTimeout` calls of `REQUEST_DEADLINE_MS`),
 * in arming order, whether it has been cleared.
 */
function watchDeadlines(): () => boolean[] {
  const armed = vi.spyOn(globalThis, "setTimeout");
  const cleared = vi.spyOn(globalThis, "clearTimeout");
  return () =>
    armed.mock.calls.flatMap((call, index) => {
      if (call[1] !== REQUEST_DEADLINE_MS) {
        return [];
      }
      const timer: unknown = armed.mock.results[index].value;
      return [cleared.mock.calls.some(([id]) => id === timer)];
    });
}

// The menu load is the one request that also carries a signal of TanStack Query's, which aborts it
// when the query is cancelled. `api/client.ts` composes that signal with the shared request
// deadline: a cancelled load aborts the request handed to `fetch` with the cancellation's own
// reason, never the deadline's `TimeoutError`, and its deadline timer is cleared at once rather
// than left to fire. The cancelled load leaves no message behind.
describe("COMEN01 request deadline: a cancelled load", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("F3 while endpoint 2 loads cancels the load at once, not at the deadline, and the menu entered again shows no error", async () => {
    stubFetch(() => {
      const sent = menuRequests();
      return sent.length === 1 ? unansweredUntilAborted(sent[0]) : json(200, MENU);
    });
    const deadlines = watchDeadlines();
    renderMenu();
    await waitFor(() => {
      expect(menuRequests()).toHaveLength(1);
    });
    const [load] = menuRequests();
    expect(load.signal.aborted).toBe(false);
    expect(deadlines()).toEqual([false]);

    fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));
    // Leaving the screen unmounts its query, and TanStack Query cancels it: the request is aborted
    // before the click returns, so no timer, the deadline's included, has had a turn to run.
    expect(load.signal.aborted).toBe(true);
    expect(reasonName(load.signal)).not.toBe("TimeoutError");
    expect(reasonName(load.signal)).toBe("AbortError");
    expect(await screen.findByText(`arrived at ${PATHS.COSGN00}`)).toBeTruthy();
    await waitFor(() => {
      expect(deadlines()).toEqual([true]);
    });

    // Back on the menu, a fresh load fills the table; the cancellation shows nowhere.
    fireEvent.click(screen.getByRole("link", { name: "Back to the menu" }));
    await loaded();
    expect(errorRow().textContent).toBe("");
    expect(menuRequests()).toHaveLength(2);
    expect(menuRequests()[1].signal.aborted).toBe(false);
    await waitFor(() => {
      expect(deadlines()).toEqual([true, true]);
    });
    expect(reasonName(load.signal)).toBe("AbortError");
  });

  it("StrictMode's double mount cancels the first load before fetch receives it, clears its deadline and shows the second load's table", async () => {
    // `main.tsx` renders the application in `StrictMode`, so the development build mounts the menu,
    // unmounts it and mounts it again in one commit. The first load's signal is aborted before
    // openapi-fetch hands its request to the client's `fetch`, which then aborts the request at
    // once with the cancellation's reason; `fetch` rejects such a request without sending it.
    const abortedAtHandOff: boolean[] = [];
    stubFetch(() => {
      const sent = menuRequests();
      const request = sent[sent.length - 1];
      abortedAtHandOff.push(request.signal.aborted);
      return request.signal.aborted ? unansweredUntilAborted(request) : json(200, MENU);
    });
    const deadlines = watchDeadlines();
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    render(
      <QueryClientProvider client={client}>
        <MemoryRouter initialEntries={[PATHS.COMEN01]}>
          <Routes>
            <Route path={PATHS.COMEN01} element={<COMEN01 />} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
      { reactStrictMode: true },
    );

    await loaded();
    expect(abortedAtHandOff).toEqual([true, false]);
    const [cancelled, served] = menuRequests();
    expect(reasonName(cancelled.signal)).not.toBe("TimeoutError");
    expect(reasonName(cancelled.signal)).toBe("AbortError");
    expect(served.signal.aborted).toBe(false);
    expect(errorRow().textContent).toBe("");
    await waitFor(() => {
      expect(deadlines()).toEqual([true, true]);
    });
    expect(client.isFetching()).toBe(0);
  });
});

/** The browser's network `TypeError` message, as Chrome's `fetch` words it with no connection. */
const NO_NETWORK = "Failed to fetch";

/** The browser's `offline` or `online` event, which TanStack Query's `onlineManager` follows. */
function fireNetworkEvent(type: "offline" | "online"): void {
  act(() => {
    window.dispatchEvent(new Event(type));
  });
}

// COMEN01C sends its option table when the menu is entered, and ENTER is decided from the table
// that load brought (`hooks.ts`). The application's own query client (`App.tsx`), not a test
// client, decides whether the browser's return to the network reads the table again. It does
// not: with no key pressed nothing is sent, the table stays as loaded and ENTER stays free.
describe("COMEN01 in the application: the network drops and returns", () => {
  afterEach(() => {
    // The online state lives in TanStack Query's process-wide `onlineManager`: the next test
    // starts online.
    onlineManager.setOnline(true);
  });

  it("loads endpoint 2 once on entry and not again when the network returns", async () => {
    let up = true;
    stubFetch(() => (up ? json(200, MENU) : Promise.reject(new TypeError(NO_NETWORK))));
    const startUrl = window.location.href;
    window.history.replaceState(null, "", PATHS.COMEN01);
    try {
      // `App` creates its router from `window.location` when its module is first evaluated. No
      // other test in this file imports it, so this import evaluates it here, at `/menu`.
      const { App } = await import("@/App");
      render(<App />);
      expect(
        await screen.findByRole("heading", { level: 1, name: "Main Menu" }, { timeout: 5_000 }),
      ).toBeDefined();
      await loaded();
      expect(menuRequests()).toHaveLength(1);

      // The network drops and returns twice, and no key is pressed.
      for (let round = 1; round <= 2; round += 1) {
        up = false;
        fireNetworkEvent("offline");
        expect(onlineManager.isOnline()).toBe(false);
        up = true;
        fireNetworkEvent("online");
        expect(onlineManager.isOnline()).toBe(true);
        await lingers();
      }

      expect(menuRequests()).toHaveLength(1);
      expect(requests).toHaveLength(1);
      await loaded();
      expect(errorRow().textContent).toBe("");
      expectBusy(false);
    } finally {
      window.history.replaceState(null, "", startUrl);
    }
  });
});
