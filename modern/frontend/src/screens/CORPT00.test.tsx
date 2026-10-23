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
import { CORPT00 } from "./CORPT00";

/*
 * Render test of the transaction reports screen (map CORPT0A, transaction CR00, program
 * CORPT00C, route `/reports`) in jsdom, with `fetch` stubbed for endpoint 14,
 * `POST /api/v1/reports/transactions`. Expected values are the audited results of
 * `modern/audit/screens.md` (`CORPT00-nnn`) and `modern/audit/online/CORPT00C.md`
 * (`CORPT00C-nnn`); every message below is a literal those rows quote, except the 401's, which
 * comes from the API's token extractor, and the digits-only text, which is the request DTOs'.
 *
 * Not tagged here: the `both` rows of the ten inputs (`LENGTH`, `NUM`), whose client half is
 * proved in `src/schemas/CORPT00.test.ts`, and the header label rows `CORPT00-001`, `-004`,
 * `-006` and `-009`, which NAV (`frontend/e2e/nav.spec.ts`) proves. The header values are the
 * read-only `Input`s `ScreenShell` renders in the `CardHeader` title rows: their value rows
 * `CORPT00-002`, `-003`, `-005`, `-007`, `-008` and `-010` are tagged here, and so are their
 * content rows (`CORPT00-043` to `CORPT00-048`), read from those inputs' values. The server rows
 * behind each fixture are proved by the endpoint tests.
 *
 * A `ScreenError.field` is the request member's wire name (`start_month`), as endpoint 14 sends
 * it and the screen maps it; the dotted `start_date.month` of the CORPT00C audit's prose is not
 * what the API emits.
 */

const ENDPOINT = "/api/v1/reports/transactions";

/**
 * The classes of the shared map-field box, FieldGrid's `FIELD_INPUT`: `min-w-max` and
 * `field-sizing-content` keep a box never narrower than its value, `px-1` is the one inset every
 * map input shares and `text-sm` the inputs' type size. Restated here because the render-test
 * contract imports no module beside the screen, the session, the routes and the generated types,
 * so a box is checked against this list rather than against the constant that renders it.
 */
const MAP_FIELD_BOX = ["min-w-max", "px-1", "text-sm", "field-sizing-content"] as const;

/** Accessible names of the inputs the tests address directly. */
const MONTHLY = "Monthly (Current Month)";
const CONFIRM = "The Report will be submitted for printing. Please confirm:";

/**
 * The ten inputs of the map in field order: the request member each fills, its accessible name,
 * the literal bound to it as its `<label>`, its map `LENGTH` and whether it carries `NUM`
 * (`CORPT00-012` to `CORPT00-038`).
 */
const FIELDS = [
  {
    wire: "monthly",
    name: MONTHLY,
    label: "Monthly (Current Month)",
    length: 1,
    numeric: false,
  },
  {
    wire: "yearly",
    name: "Yearly (Current Year)",
    label: "Yearly (Current Year)",
    length: 1,
    numeric: false,
  },
  {
    wire: "custom",
    name: "Custom (Date Range)",
    label: "Custom (Date Range)",
    length: 1,
    numeric: false,
  },
  {
    wire: "start_month",
    name: "Start Date month",
    label: "Start Date :",
    length: 2,
    numeric: true,
  },
  { wire: "start_day", name: "Start Date day", label: "/", length: 2, numeric: true },
  { wire: "start_year", name: "Start Date year", label: "/", length: 4, numeric: true },
  { wire: "end_month", name: "End Date month", label: "  End Date :", length: 2, numeric: true },
  { wire: "end_day", name: "End Date day", label: "/", length: 2, numeric: true },
  { wire: "end_year", name: "End Date year", label: "/", length: 4, numeric: true },
  {
    wire: "confirm",
    name: CONFIRM,
    label: "The Report will be submitted for printing. Please confirm: ",
    length: 1,
    numeric: false,
  },
] as const;

/**
 * Every literal of the body at its map `POS` (row, column): the selector, date and confirmation
 * labels and the three hints (`CORPT00-014` to `CORPT00-040`).
 */
const LITERALS = [
  { row: 7, col: 15, text: "Monthly (Current Month)" },
  { row: 9, col: 15, text: "Yearly (Current Year)" },
  { row: 11, col: 15, text: "Custom (Date Range)" },
  { row: 13, col: 15, text: "Start Date :" },
  { row: 13, col: 32, text: "/" },
  { row: 13, col: 37, text: "/" },
  { row: 13, col: 46, text: "(MM/DD/YYYY)" },
  { row: 14, col: 15, text: "  End Date :" },
  { row: 14, col: 32, text: "/" },
  { row: 14, col: 37, text: "/" },
  { row: 14, col: 46, text: "(MM/DD/YYYY)" },
  { row: 19, col: 6, text: "The Report will be submitted for printing. Please confirm: " },
  { row: 19, col: 69, text: "(Y/N)" },
];

/**
 * The zero-length stoppers that end the selectors, the two year parts and the confirmation
 * (`CORPT00-013`, `-016`, `-019`, `-027`, `-035`, `-039`): `POS` row and column.
 */
const STOPPERS = [
  { row: 7, col: 12 },
  { row: 9, col: 12 },
  { row: 11, col: 12 },
  { row: 13, col: 44 },
  { row: 14, col: 44 },
  { row: 19, col: 68 },
];

/**
 * `FIELD_NUMERIC_MESSAGE` of the request DTOs (`crates/api/src/dto/mod.rs`): the text endpoint 14
 * answers a date part that is not digits with, and the schema refuses the same part with.
 */
const NOT_DIGITS = "Only digits can be entered in this field...";

/** `CCDA-MSG-INVALID-KEY`, the 50-byte value with its padding (CORPT00C-005). */
const INVALID_KEY_TEXT = "Invalid key pressed. Please see below...".padEnd(50, " ");

/** The F-keys CORPT00C serves under no branch but `WHEN OTHER`: all but F3. */
const UNSERVED_F_KEYS = ["F1", "F2", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];

/** Custom, 01/01/2022 to 07/06/2022, confirmed `Y`. */
const customRequest: components["schemas"]["ReportRequest"] = {
  monthly: "",
  yearly: "",
  custom: "X",
  start_month: "01",
  start_day: "01",
  start_year: "2022",
  end_month: "07",
  end_day: "06",
  end_year: "2022",
  confirm: "Y",
};

/** Monthly, confirmed with a lower-case `y`, sent as typed. */
const monthlyRequest: components["schemas"]["ReportRequest"] = {
  monthly: "X",
  yearly: "",
  custom: "",
  start_month: "",
  start_day: "",
  start_year: "",
  end_month: "",
  end_day: "",
  end_year: "",
  confirm: "y",
};

/** Yearly, selected by a lower-case `x`; any non-blank character selects (CORPT00C-007). */
const yearlyRequest: components["schemas"]["ReportRequest"] = {
  monthly: "",
  yearly: "x",
  custom: "",
  start_month: "",
  start_day: "",
  start_year: "",
  end_month: "",
  end_day: "",
  end_year: "",
  confirm: "Y",
};

/** Monthly with the confirmation left blank. */
const unconfirmedRequest: components["schemas"]["ReportRequest"] = {
  monthly: "X",
  yearly: "",
  custom: "",
  start_month: "",
  start_day: "",
  start_year: "",
  end_month: "",
  end_day: "",
  end_year: "",
  confirm: "",
};

/** {@link unconfirmedRequest} answered with `N`. */
const declinedRequest: components["schemas"]["ReportRequest"] = {
  monthly: "X",
  yearly: "",
  custom: "",
  start_month: "",
  start_day: "",
  start_year: "",
  end_month: "",
  end_day: "",
  end_year: "",
  confirm: "N",
};

/** Custom with a start month above 12. */
const badMonthRequest: components["schemas"]["ReportRequest"] = {
  monthly: "",
  yearly: "",
  custom: "X",
  start_month: "13",
  start_day: "01",
  start_year: "2022",
  end_month: "07",
  end_day: "06",
  end_year: "2022",
  confirm: "Y",
};

/** Custom with the end day left blank. */
const noEndDayRequest: components["schemas"]["ReportRequest"] = {
  monthly: "",
  yearly: "",
  custom: "X",
  start_month: "01",
  start_day: "01",
  start_year: "2022",
  end_month: "07",
  end_day: "",
  end_year: "2022",
  confirm: "Y",
};

/** No report selected, with start date parts and a confirmation typed. */
const noReportRequest: components["schemas"]["ReportRequest"] = {
  monthly: "",
  yearly: "",
  custom: "",
  start_month: "01",
  start_day: "01",
  start_year: "2022",
  end_month: "",
  end_day: "",
  end_year: "",
  confirm: "Y",
};

/** Monthly confirmed with a character other than `Y`, `y`, `N` and `n`. */
const badConfirmRequest: components["schemas"]["ReportRequest"] = {
  monthly: "X",
  yearly: "",
  custom: "",
  start_month: "",
  start_day: "",
  start_year: "",
  end_month: "",
  end_day: "",
  end_year: "",
  confirm: "X",
};

/** CORPT00C-038: the custom deck was written. */
const customSubmitted: components["schemas"]["ReportResult"] = {
  message: "Custom report submitted for printing ...",
  job_deck: "TRNRPT00.G0001",
};

/** CORPT00C-038: the monthly deck was written. */
const monthlySubmitted: components["schemas"]["ReportResult"] = {
  message: "Monthly report submitted for printing ...",
  job_deck: "TRNRPT00.G0002",
};

/** CORPT00C-038: the yearly deck was written. */
const yearlySubmitted: components["schemas"]["ReportResult"] = {
  message: "Yearly report submitted for printing ...",
  job_deck: "TRNRPT00.G0003",
};

/** CORPT00C-032: `N` writes no deck and answers an empty message. */
const declined: components["schemas"]["ReportResult"] = { message: "" };

/** CORPT00C-030: the confirmation prompt of the monthly report. */
const confirmPrompt: components["schemas"]["ScreenError"] = {
  message: "Please confirm to print the Monthly report...",
  field: "confirm",
};

/** CORPT00C-018: a start month above 12. */
const badMonth: components["schemas"]["ScreenError"] = {
  message: "Start Date - Not a valid Month...",
  field: "start_month",
};

/** CORPT00C-015: a blank end day. */
const noEndDay: components["schemas"]["ScreenError"] = {
  message: "End Date - Day can NOT be empty...",
  field: "end_day",
};

/** CORPT00C-011: a blank start month, all spaces included. */
const noStartMonth: components["schemas"]["ScreenError"] = {
  message: "Start Date - Month can NOT be empty...",
  field: "start_month",
};

/** CORPT00C-014: a blank end month. */
const noEndMonth: components["schemas"]["ScreenError"] = {
  message: "End Date - Month can NOT be empty...",
  field: "end_month",
};

/** CORPT00C-029: no report selected. */
const noReport: components["schemas"]["ScreenError"] = {
  message: "Select a report type to print report...",
  field: "monthly",
};

/** CORPT00C-033: a confirmation of `X`. */
const badConfirm: components["schemas"]["ScreenError"] = {
  message: '"X" is not a valid value to confirm...',
  field: "confirm",
};

/** CORPT00C-037: the job deck cannot be written. */
const writeFailed: components["schemas"]["ScreenError"] = {
  message: "Unable to Write TDQ (JOBS)...",
  field: "monthly",
};

/**
 * CORPT00C-043: the 401 of the `SignedOnUser` extractor, which names no field. The row quotes no
 * text; this is the extractor's `SIGNON_REQUIRED` (`crates/api/src/auth/extract.rs`).
 */
const signonRequired: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
};

/** A JSON response, as the API sends every success and every `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces `fetch`. The n-th request to endpoint 14 gets the n-th reply, after its `hold`
 * settles; any other request, and any request beyond the replies, gets a 500. Returns every
 * request in the order the screen sent it.
 */
function stubApi(...replies: { status: number; body: unknown; hold?: Promise<void> }[]): Request[] {
  const requests: Request[] = [];
  let served = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      requests.push(req);
      const { pathname } = new URL(req.url);
      const reply = req.method === "POST" && pathname === ENDPOINT ? replies[served] : undefined;
      if (reply !== undefined) {
        served += 1;
        await reply.hold;
        return json(reply.status, reply.body);
      }
      const unexpected: components["schemas"]["ScreenError"] = {
        message: `Unexpected request ${req.method} ${pathname}`,
      };
      return json(500, unexpected);
    }),
  );
  return requests;
}

/** The JSON bodies of the requests, in order. */
async function bodies(requests: readonly Request[]): Promise<unknown[]> {
  return Promise.all(requests.map(async (request): Promise<unknown> => request.clone().json()));
}

/** A navigation target: shows the path it was reached at and the route state it received. */
function RouteStub() {
  const location = useLocation();
  return (
    <section aria-label="Route stub">
      <p data-testid="stub-path">{location.pathname}</p>
      <p data-testid="stub-state">{JSON.stringify(location.state ?? null)}</p>
    </section>
  );
}

/** CORPT00 at `/reports`, and a stub at the main menu, the one route it transfers to. */
function renderScreen() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[PATHS.CORPT00]}>
        <Routes>
          <Route path={PATHS.CORPT00} element={<CORPT00 />} />
          <Route path={PATHS.COMEN01} element={<RouteStub />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The `<input>` whose accessible name is `name`, failing the test when it is anything else. */
function inputOf(name: string): HTMLInputElement {
  const element = screen.getByLabelText(name);
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`"${name}" labels <${element.tagName.toLowerCase()}>, not an input`);
  }
  return element;
}

/** Types every member of `request` into its input; an absent member leaves the input empty. */
function fill(request: components["schemas"]["ReportRequest"]): void {
  for (const field of FIELDS) {
    fireEvent.change(inputOf(field.name), { target: { value: request[field.wire] ?? "" } });
  }
}

/** The value each input shows, by request member. */
function shown(): Record<string, string> {
  return Object.fromEntries(FIELDS.map((field) => [field.wire, inputOf(field.name).value]));
}

/** What {@link shown} reads after {@link fill} of `request`. */
function typed(request: components["schemas"]["ReportRequest"]): Record<string, string> {
  return Object.fromEntries(FIELDS.map((field) => [field.wire, request[field.wire] ?? ""]));
}

/** Every input empty. */
const BLANK = Object.fromEntries(FIELDS.map((field) => [field.wire, ""]));

/** The map's one message row, `ERRMSG`; CORPT00 declares no `INFOMSG`. */
function messageRow(): HTMLElement {
  const rows = screen.getAllByRole("alert");
  expect(rows).toHaveLength(1);
  const [row] = rows;
  if (row === undefined) {
    throw new Error("no message row");
  }
  return row;
}

/** The message row when it is of `kind` (`MessageLine`'s `data-message-kind`), else none. */
function messageRows(kind: "error" | "info"): HTMLElement[] {
  return screen.getAllByRole("alert").filter((row) => row.dataset.messageKind === kind);
}

/** The ids in the input's `aria-describedby` that name a `FormMessage`. */
function messageIdsOf(input: HTMLElement): string[] {
  return (input.getAttribute("aria-describedby") ?? "")
    .split(" ")
    .filter((id) => id.endsWith("-form-item-message"));
}

/** Every `FormMessage` on the screen. */
function fieldMessages(): Element[] {
  return Array.from(document.querySelectorAll('[data-slot="form-message"]'));
}

/**
 * Asserts that the input named `name` alone is marked invalid with `text`: `aria-invalid`, and an
 * `aria-describedby` naming the screen's one `FormMessage`, present, for screen readers only and
 * reading `text`. The message row stays the one place `text` shows.
 */
function expectMarked(name: string, text: string): void {
  const input = inputOf(name);
  expect(input.getAttribute("aria-invalid")).toBe("true");
  const ids = messageIdsOf(input);
  expect(ids).toHaveLength(1);
  const message = document.getElementById(ids[0] ?? "");
  expect(message).not.toBeNull();
  expect(fieldMessages()).toEqual([message]);
  expect(message?.textContent).toBe(text);
  expect(message?.classList.contains("sr-only")).toBe(true);
  for (const field of FIELDS.filter((other) => other.name !== name)) {
    expect(inputOf(field.name).getAttribute("aria-invalid")).toBe("false");
    expect(messageIdsOf(inputOf(field.name))).toEqual([]);
  }
  const visible = screen
    .getAllByText(text)
    .filter((element) => element.closest(".sr-only") === null);
  expect(visible).toHaveLength(1);
  expect(messageRow().contains(visible[0] ?? null)).toBe(true);
}

/** Asserts that no input is marked invalid or described by a message, and that none exists. */
function expectUnmarked(): void {
  for (const field of FIELDS) {
    expect(inputOf(field.name).getAttribute("aria-invalid")).toBe("false");
    expect(messageIdsOf(inputOf(field.name))).toEqual([]);
  }
  expect(fieldMessages()).toEqual([]);
}

/** Every id an `aria-describedby` on the page names that resolves to no element. */
function danglingDescriptions(): string[] {
  return [...document.querySelectorAll("[aria-describedby]")].flatMap((element) =>
    (element.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .filter((id) => id === "" || document.getElementById(id) === null),
  );
}

/** What an input's `aria-describedby` reads: one entry per id, `null` for an id of no element. */
type Described = { text: string | null; hidden: boolean; cell: string | null } | null;

/**
 * How assistive technology reads each input, by request member: its `aria-invalid`, and the
 * element each id of its `aria-describedby` names, as its text, whether `sr-only` hides it and the
 * grid cell it stands in (`null` for an element outside any cell). `described` is `null` when the
 * input has no `aria-describedby`.
 */
function readOuts(): Record<string, { invalid: string | null; described: Described[] | null }> {
  return Object.fromEntries(
    FIELDS.map((field) => {
      const input = inputOf(field.name);
      const ids = input.getAttribute("aria-describedby");
      const described =
        ids === null
          ? null
          : ids.split(" ").map((id): Described => {
              const target = id === "" ? null : document.getElementById(id);
              const parent = target?.parentElement ?? null;
              return target === null
                ? null
                : {
                    text: target.textContent,
                    hidden: target.classList.contains("sr-only"),
                    cell:
                      parent !== null && /\brow-start-\d+\b/.test(parent.className)
                        ? parent.className
                        : null,
                  };
            });
      return [field.wire, { invalid: input.getAttribute("aria-invalid"), described }];
    }),
  );
}

/** The `(MM/DD/YYYY)` of the start date's line (`CORPT00-028`), shown, in its map cell. */
const START_HINT = {
  text: "(MM/DD/YYYY)",
  hidden: false,
  cell: "row-start-13 col-start-46 col-span-12",
};

/** The `(MM/DD/YYYY)` of the end date's line (`CORPT00-036`), shown, in its map cell. */
const END_HINT = {
  text: "(MM/DD/YYYY)",
  hidden: false,
  cell: "row-start-14 col-start-46 col-span-12",
};

/** The confirmation's `(Y/N)` (`CORPT00-040`), shown, in its map cell. */
const CONFIRM_HINT = { text: "(Y/N)", hidden: false, cell: "row-start-19 col-start-69 col-span-5" };

/** The hint each input is read with, by request member; the three report selectors have none. */
const HINTS_OF: Record<string, readonly Described[]> = {
  monthly: [],
  yearly: [],
  custom: [],
  start_month: [START_HINT],
  start_day: [START_HINT],
  start_year: [START_HINT],
  end_month: [END_HINT],
  end_day: [END_HINT],
  end_year: [END_HINT],
  confirm: [CONFIRM_HINT],
};

/**
 * What {@link readOuts} reads when `marked` alone holds `message`: each input described by its
 * hint, and the marked one also by its field message, hidden and in no cell; an input with
 * neither has no `aria-describedby`.
 */
function expectedReadOuts(
  marked: { wire: string; message: string } | null,
): Record<string, { invalid: string; described: Described[] | null }> {
  return Object.fromEntries(
    FIELDS.map((field) => {
      const own = field.wire === marked?.wire;
      const described = [
        ...(HINTS_OF[field.wire] ?? []),
        ...(own ? [{ text: marked.message, hidden: true, cell: null }] : []),
      ];
      return [
        field.wire,
        { invalid: own ? "true" : "false", described: described.length > 0 ? described : null },
      ];
    }),
  );
}

/**
 * The value of the header field `name` (`data-field` of `ScreenShell`): the one read-only `Input`
 * of that name in the `CardHeader`, failing the test when there is not exactly one.
 */
function headerField(container: HTMLElement, name: string): string {
  const fields = container.querySelectorAll(
    `[data-slot="card-header"] input[data-field="${name}"]`,
  );
  expect(fields).toHaveLength(1);
  const [field] = fields;
  if (!(field instanceof HTMLInputElement)) {
    throw new Error(`The header has no input ${name}`);
  }
  return field.value;
}

/** The grid cell that starts at `POS=(row,col)`, or `null`. */
function cellAt(container: HTMLElement, row: number, col: number): Element | null {
  return container.querySelector(
    `[class~="row-start-${String(row)}"][class~="col-start-${String(col)}"]`,
  );
}

/** The ENTER button of the key row. */
function enterButton(): HTMLElement {
  return screen.getByRole("button", { name: "ENTER=Continue" });
}

/** A keydown of `key` at the focused element, as the browser sends it; it bubbles to `window`. */
function pressKey(key: string, shiftKey = false): void {
  fireEvent.keyDown(document.activeElement ?? document.body, { key, shiftKey });
}

/**
 * Moves the cursor to `element`, so that where the screen puts it next is observable. The move
 * runs inside `waitFor`, which lets React render the blur it causes.
 */
async function moveFocus(element: HTMLElement): Promise<void> {
  await waitFor(() => {
    element.focus();
    expect(document.activeElement).toBe(element);
  });
}

/** Presses ENTER through its button, with the cursor moved to that button first. */
async function pressEnter(): Promise<void> {
  const enter = enterButton();
  await moveFocus(enter);
  fireEvent.click(enter);
}

/**
 * Lets pending effects, mutations and stubbed responses run, so that a request the screen would
 * send has been sent before a test counts the requests. The wait runs inside `waitFor`, so the
 * renders it lets through are React's to batch, as in every other wait of these tests.
 */
async function settle(): Promise<void> {
  await waitFor(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, 20);
    });
  });
}

beforeEach(() => {
  setSession("test-token", "USER0001", "U");
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("CORPT00 transaction reports screen", () => {
  // audit: CORPT00C-002
  // audit: CORPT00-011
  // audit: CORPT00-013
  // audit: CORPT00-014
  // audit: CORPT00-016
  // audit: CORPT00-017
  // audit: CORPT00-019
  // audit: CORPT00-020
  // audit: CORPT00-021
  // audit: CORPT00-023
  // audit: CORPT00-025
  // audit: CORPT00-027
  // audit: CORPT00-028
  // audit: CORPT00-029
  // audit: CORPT00-031
  // audit: CORPT00-033
  // audit: CORPT00-035
  // audit: CORPT00-036
  // audit: CORPT00-037
  // audit: CORPT00-039
  // audit: CORPT00-040
  // audit: CORPT00-042
  it("opens cleared, cursor on Monthly, message row empty, sending nothing", async () => {
    const requests = stubApi();
    const { container } = renderScreen();

    const title = screen.getByRole("heading", { level: 1, name: "Transaction Reports" });
    expect(title.textContent).toBe("Transaction Reports");
    expect(title.getAttribute("data-slot")).toBe("card-title");

    // Each input in a `FormItem`, at its map `LENGTH`, `NUM` only on the six date parts, empty,
    // and bound to the literal before or after it as its `<label>`.
    for (const field of FIELDS) {
      const box = inputOf(field.name);
      expect(box.getAttribute("data-slot")).toBe("form-control");
      expect(box.closest('[data-slot="form-item"]')).not.toBeNull();
      expect(box.maxLength).toBe(field.length);
      // The shared map-field box, `MAP_FIELD_BOX` with `size` = the LENGTH, so a full-length value
      // shows on the narrowest frame; the one- and two-character fields keep a `w-6` box, empty
      // or not, the four-character years fill their cells.
      expect(box.size).toBe(field.length);
      expect(MAP_FIELD_BOX.filter((name) => !box.classList.contains(name))).toEqual([]);
      expect(box.classList.contains("w-6")).toBe(field.length <= 2);
      expect(box.getAttribute("inputmode")).toBe(field.numeric ? "numeric" : null);
      expect(box.value).toBe("");
      const labels = Array.from(box.labels ?? []);
      expect(labels.map((label) => label.textContent)).toEqual([field.label]);
      expect(labels.map((label) => label.getAttribute("data-slot"))).toEqual(["form-label"]);
    }
    expect(document.activeElement).toBe(inputOf(MONTHLY));

    for (const literal of LITERALS) {
      expect(cellAt(container, literal.row, literal.col)?.textContent).toBe(literal.text);
    }
    // A zero-length stopper has no element: nothing starts in its cell.
    for (const stopper of STOPPERS) {
      expect(cellAt(container, stopper.row, stopper.col)).toBeNull();
    }

    const row = messageRow();
    expect(row.dataset.messageKind).toBe("error");
    expect(row.textContent).toBe("");

    // Row 24 in map order: ENTER submits the screen's form, F3 is a plain key button.
    const keyBar = screen.getByRole("group", { name: "Function keys" });
    const keys = within(keyBar).getAllByRole("button");
    expect(keys.map((key) => key.textContent)).toEqual(["ENTER=Continue", "F3=Back"]);
    const [enter, back] = keys;
    expect([enter?.getAttribute("type"), enter?.dataset.variant]).toEqual(["submit", "default"]);
    expect(enter?.getAttribute("form")).toBe(container.querySelector("form")?.id);
    expect([back?.getAttribute("type"), back?.dataset.variant, back?.dataset.size]).toEqual([
      "button",
      "outline",
      "sm",
    ]);

    await settle();
    expect(requests).toHaveLength(0);
  });

  // audit: CORPT00C-041
  // audit: CORPT00-043
  // audit: CORPT00-044
  // audit: CORPT00-045
  // audit: CORPT00-046
  // audit: CORPT00-047
  // audit: CORPT00-048
  it("fills the header rows at every send: CR00, CORPT00C, titles, date, time", async () => {
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
    stubApi({ status: 400, body: confirmPrompt });
    const { container } = renderScreen();

    expect(headerField(container, "TRNNAME")).toBe("CR00");
    expect(headerField(container, "PGMNAME")).toBe("CORPT00C");
    expect(headerField(container, "TITLE01")).toBe("      AWS Mainframe Modernization       ");
    expect(headerField(container, "TITLE02")).toBe("              CardDemo                  ");
    expect(headerField(container, "CURDATE")).toBe("01/02/24");
    expect(headerField(container, "CURTIME")).toBe("03:04:05");

    vi.setSystemTime(new Date(2025, 11, 31, 23, 59, 58));
    fill(unconfirmedRequest);
    await pressEnter();
    await waitFor(() => {
      expect(messageRow().textContent).toBe(confirmPrompt.message);
    });

    expect(headerField(container, "TRNNAME")).toBe("CR00");
    expect(headerField(container, "PGMNAME")).toBe("CORPT00C");
    expect(headerField(container, "CURDATE")).toBe("12/31/25");
    expect(headerField(container, "CURTIME")).toBe("23:59:58");
  });

  // The values these fields show are proved under CORPT00-043 to CORPT00-048.
  it.each([
    // audit: CORPT00-002
    {
      id: "CORPT00-002",
      name: "TRNNAME",
      row: 1,
      col: 7,
      length: 4,
      label: "Tran:",
      shows: "CR00",
    },
    // audit: CORPT00-003
    {
      id: "CORPT00-003",
      name: "TITLE01",
      row: 1,
      col: 21,
      length: 40,
      label: null,
      shows: "      AWS Mainframe Modernization       ",
    },
    // audit: CORPT00-005
    {
      id: "CORPT00-005",
      name: "CURDATE",
      row: 1,
      col: 71,
      length: 8,
      label: "Date:",
      shows: expect.stringMatching(/^\d{2}\/\d{2}\/\d{2}$/),
    },
    // audit: CORPT00-007
    {
      id: "CORPT00-007",
      name: "PGMNAME",
      row: 2,
      col: 7,
      length: 8,
      label: "Prog:",
      shows: "CORPT00C",
    },
    // audit: CORPT00-008
    {
      id: "CORPT00-008",
      name: "TITLE02",
      row: 2,
      col: 21,
      length: 40,
      label: null,
      shows: "              CardDemo                  ",
    },
    // audit: CORPT00-010
    {
      id: "CORPT00-010",
      name: "CURTIME",
      row: 2,
      col: 71,
      length: 8,
      label: "Time:",
      shows: expect.stringMatching(/^\d{2}:\d{2}:\d{2}$/),
    },
  ])(
    "$id: header value $name is a read-only Input of the CardHeader title rows at its map POS and LENGTH",
    ({ name, row, col, length, label, shows }) => {
      stubApi();
      const { container } = renderScreen();

      const header = container.querySelector('[data-slot="card-header"]');
      if (!(header instanceof HTMLElement)) {
        throw new Error("no card header");
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
        throw new Error(`header field ${name} is not an input`);
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
      // The title rows hold no editable control: their six controls are the read-only values.
      const controls = Array.from(
        header.querySelectorAll("input, textarea, select, button, [contenteditable]"),
      );
      expect(controls).toHaveLength(6);
      expect(
        controls.every((control) => control instanceof HTMLInputElement && control.readOnly),
      ).toBe(true);
    },
  );

  // audit: CORPT00-041
  // audit: CORPT00-049
  // audit: CORPT00C-040
  // audit: CORPT00C-042
  it.each([
    {
      report: "Custom",
      via: "the ENTER=Continue button",
      request: customRequest,
      reply: customSubmitted,
    },
    {
      report: "Monthly",
      via: "the ENTER=Continue button",
      request: monthlyRequest,
      reply: monthlySubmitted,
    },
    { report: "Yearly", via: "the Enter key", request: yearlyRequest, reply: yearlySubmitted },
  ])(
    "$via submits the $report report as typed, shows its message as information, empties the form",
    async ({ via, request, reply }) => {
      const requests = stubApi({ status: 200, body: reply });
      renderScreen();
      fill(request);
      expect(shown()).toEqual(typed(request));
      const enter = enterButton();
      expect([enter.getAttribute("type"), enter.dataset.variant]).toEqual(["submit", "default"]);

      if (via === "the Enter key") {
        // Outside the form the key bar's binding submits it, as implicit submission does inside.
        await waitFor(() => {
          if (document.activeElement instanceof HTMLElement) {
            document.activeElement.blur();
          }
          expect(document.activeElement).toBe(document.body);
        });
        pressKey("Enter");
      } else {
        await pressEnter();
      }

      // The submission text is the one message CORPT00C colours green, so the row shows it in the
      // `Alert`'s default (information) variant, not the red destructive one.
      await waitFor(() => {
        expect(messageRow().textContent).toBe(reply.message);
      });
      const [info] = messageRows("info");
      expect(info?.className).toContain("text-card-foreground");
      expect(info?.className).not.toContain("text-destructive");
      // ERRMSG is `BRT` whatever its colour, so the information text is semibold.
      const description = info?.querySelector('[data-slot="alert-description"]');
      expect(description?.classList.contains("font-semibold")).toBe(true);
      expect(messageRows("error")).toHaveLength(0);
      expect(shown()).toEqual(BLANK);
      await waitFor(() => {
        expect(document.activeElement).toBe(inputOf(MONTHLY));
      });
      // The map has no field for the deck's name, so nothing on the screen shows it or a path.
      expect(document.body.textContent).not.toMatch(/TRNRPT00|var\/jobs/);

      await settle();
      expect(requests).toHaveLength(1);
      const [sent] = requests;
      expect(sent?.method).toBe("POST");
      expect(new URL(sent?.url ?? "").pathname).toBe(ENDPOINT);
      expect(new URL(sent?.url ?? "").search).toBe("");
      expect(sent?.headers.get("Authorization")).toBe("Bearer test-token");
      expect(await bodies(requests)).toEqual([request]);
    },
  );

  // audit: CORPT00C-040
  // audit: CORPT00C-042
  it("shows the blank-confirmation prompt in red on Confirm, then N empties the form", async () => {
    const requests = stubApi({ status: 400, body: confirmPrompt }, { status: 200, body: declined });
    renderScreen();
    fill(unconfirmedRequest);
    await pressEnter();

    await waitFor(() => {
      expect(messageRow().textContent).toBe(confirmPrompt.message);
    });
    const [error] = messageRows("error");
    expect(error?.className).toContain("text-destructive");
    expect(shown()).toEqual(typed(unconfirmedRequest));
    await waitFor(() => {
      expect(document.activeElement).toBe(inputOf(CONFIRM));
    });

    fireEvent.change(inputOf(CONFIRM), { target: { value: "N" } });
    await pressEnter();

    await waitFor(() => {
      expect(shown()).toEqual(BLANK);
    });
    expect(messageRow().textContent).toBe("");
    await waitFor(() => {
      expect(document.activeElement).toBe(inputOf(MONTHLY));
    });
    await settle();
    expect(await bodies(requests)).toEqual([unconfirmedRequest, declinedRequest]);
  });

  // audit: CORPT00-041
  // audit: CORPT00C-040
  // audit: CORPT00C-042
  it.each([
    {
      title: "a start month above 12",
      request: badMonthRequest,
      status: 400,
      error: badMonth,
      focus: "Start Date month",
    },
    {
      title: "a blank end day",
      request: noEndDayRequest,
      status: 400,
      error: noEndDay,
      focus: "End Date day",
    },
    {
      title: "no report selected",
      request: noReportRequest,
      status: 400,
      error: noReport,
      focus: MONTHLY,
    },
    {
      title: "a confirmation of X",
      request: badConfirmRequest,
      status: 400,
      error: badConfirm,
      focus: CONFIRM,
    },
    {
      title: "an unwritable job deck",
      request: monthlyRequest,
      status: 500,
      error: writeFailed,
      focus: MONTHLY,
    },
    {
      title: "a refused token, naming no field",
      request: monthlyRequest,
      status: 401,
      error: signonRequired,
      focus: MONTHLY,
    },
  ])(
    "shows the $status for $title in the red row, keeps every input, focuses its field",
    async ({ request, status, error, focus }) => {
      const requests = stubApi({ status, body: error });
      renderScreen();
      expect(messageRow().textContent).toBe("");
      fill(request);
      await pressEnter();

      await waitFor(() => {
        expect(messageRow().textContent).toBe(error.message);
      });
      // ERRMSG is RED: the `destructive` variant of the shadcn/ui `Alert`, announced politely.
      const [row] = messageRows("error");
      expect(row?.getAttribute("data-slot")).toBe("alert");
      expect(row?.getAttribute("aria-live")).toBe("polite");
      expect(row?.className).toContain("text-destructive");
      expect(row?.querySelector('[data-slot="alert-description"]')?.textContent).toBe(
        error.message,
      );
      expect(shown()).toEqual(typed(request));
      await waitFor(() => {
        expect(document.activeElement).toBe(inputOf(focus));
      });
      await settle();
      expect(await bodies(requests)).toEqual([request]);
    },
  );

  it.each([
    { part: "Start Date month", wire: "start_month", value: "AB" },
    { part: "Start Date day", wire: "start_day", value: "1a" },
    { part: "End Date year", wire: "end_year", value: "20 2" },
  ] as const)(
    "refuses $value in $part before any request with the server's digits-only text, focusing it",
    async ({ part, wire, value }) => {
      // The `NUM` refusal itself is the client half of a `both` row, proved in the schema test;
      // this shows what the screen does with it: the text endpoint 14 answers the same part with
      // (`FIELD_NUMERIC_MESSAGE`) in the red row, the inputs kept, and the part marked invalid
      // with that message for screen readers.
      const requests = stubApi();
      renderScreen();
      const request: components["schemas"]["ReportRequest"] = { ...customRequest, [wire]: value };
      fill(request);
      await pressEnter();

      await waitFor(() => {
        expect(messageRow().textContent).toBe(NOT_DIGITS);
      });
      expect(messageRow().dataset.messageKind).toBe("error");
      expectMarked(part, NOT_DIGITS);
      expect(shown()).toEqual(typed(request));
      await waitFor(() => {
        expect(document.activeElement).toBe(inputOf(part));
      });
      await settle();
      expect(requests).toHaveLength(0);
    },
  );

  // audit: CORPT00C-042
  it.each([
    {
      title: "Monthly with one space in Start Month, whose parts Monthly ignores",
      request: { ...monthlyRequest, start_month: " " },
      status: 200,
      reply: monthlySubmitted,
    },
    {
      title: "Custom with an all-space Start Month, which the server's blank edit answers",
      request: { ...customRequest, start_month: "  " },
      status: 400,
      reply: noStartMonth,
    },
    {
      title: "Custom with digits and spaces around them, which the server reads as their digits",
      request: {
        ...customRequest,
        start_month: " 1",
        start_day: "1 ",
        start_year: "2022",
        end_month: "7 ",
        end_day: " 6",
        end_year: "2022",
      },
      status: 200,
      reply: customSubmitted,
    },
  ])(
    "sends $title in exactly one request, every part as typed",
    async ({ request, status, reply }) => {
      const requests = stubApi({ status, body: reply });
      renderScreen();
      fill(request);
      await pressEnter();

      await waitFor(() => {
        expect(messageRow().textContent).toBe(reply.message);
      });
      await settle();
      expect(requests).toHaveLength(1);
      expect(await bodies(requests)).toEqual([request]);
    },
  );

  it.each([
    {
      title: "a start month above 12",
      request: badMonthRequest,
      status: 400,
      error: badMonth,
      marked: "Start Date month",
    },
    {
      title: "a blank end day",
      request: noEndDayRequest,
      status: 400,
      error: noEndDay,
      marked: "End Date day",
    },
    {
      title: "no report selected",
      request: noReportRequest,
      status: 400,
      error: noReport,
      marked: MONTHLY,
    },
    {
      title: "a confirmation of X",
      request: badConfirmRequest,
      status: 400,
      error: badConfirm,
      marked: CONFIRM,
    },
    {
      title: "an unwritable job deck",
      request: monthlyRequest,
      status: 500,
      error: writeFailed,
      marked: MONTHLY,
    },
    // The two shapes below probe the screen's field mapping, not an audited answer: a failure
    // whose `field` is absent, or names a member the map has no input for, marks no input.
    {
      title: "a failure naming no field",
      request: monthlyRequest,
      status: 500,
      error: { message: writeFailed.message },
      marked: null,
    },
    {
      title: "a failure naming no input of the map",
      request: monthlyRequest,
      status: 500,
      error: { message: writeFailed.message, field: "constructor" },
      marked: null,
    },
  ])(
    "the $status for $title marks the input it names, if any, its message for screen readers only",
    async ({ request, status, error, marked }) => {
      stubApi({ status, body: error });
      renderScreen();
      fill(request);
      await pressEnter();

      await waitFor(() => {
        expect(messageRow().textContent).toBe(error.message);
        if (marked === null) {
          expectUnmarked();
        } else {
          expectMarked(marked, error.message);
        }
      });
      expect(shown()).toEqual(typed(request));
      await waitFor(() => {
        expect(document.activeElement).toBe(inputOf(marked ?? MONTHLY));
      });
    },
  );

  // The replacement's accessibility wiring, which no audit row states: every `aria-describedby`
  // names only elements the screen renders. The six date parts are described by the
  // `(MM/DD/YYYY)` of their own line, the confirmation by its `(Y/N)`, the three report selectors
  // by nothing while they are valid, and the input a `ScreenError` names also by its field
  // message, which holds the server's text, hidden.
  it.each([
    { state: "on entry", request: null, error: null },
    { state: "after the 400 naming start_month", request: badMonthRequest, error: badMonth },
    { state: "after the 400 naming confirm", request: badConfirmRequest, error: badConfirm },
    { state: "after the 400 naming monthly", request: noReportRequest, error: noReport },
  ])("describes each input by rendered elements only, $state", async ({ request, error }) => {
    stubApi(...(error === null ? [] : [{ status: 400, body: error }]));
    renderScreen();
    if (request === null || error === null) {
      await settle();
    } else {
      fill(request);
      await pressEnter();
    }

    const marked = error?.field == null ? null : { wire: error.field, message: error.message };
    await waitFor(() => {
      expect(messageRow().textContent).toBe(error?.message ?? "");
      expect(readOuts()).toEqual(expectedReadOuts(marked));
    });
    expect(danglingDescriptions()).toEqual([]);
  });

  it("places each hint after the inputs of its line in the DOM, before the next line", () => {
    stubApi();
    const { container } = renderScreen();
    /** The one element of the grid cell at `POS=(row,col)`. */
    const cellContent = (row: number, col: number): Element => {
      const cell = cellAt(container, row, col);
      expect(cell?.childElementCount).toBe(1);
      const element = cell?.firstElementChild ?? null;
      if (element === null) {
        throw new Error(`no element at (${String(row)},${String(col)})`);
      }
      return element;
    };

    // A linear reading, as a screen reader's, meets the map's lines in order: each date's parts,
    // then its format; the confirmation's prompt and input, then its `(Y/N)`.
    const reading = [
      inputOf("Start Date month"),
      inputOf("Start Date day"),
      inputOf("Start Date year"),
      cellContent(13, 46),
      cellContent(14, 15),
      inputOf("End Date month"),
      inputOf("End Date day"),
      inputOf("End Date year"),
      cellContent(14, 46),
      cellContent(19, 6),
      inputOf(CONFIRM),
      cellContent(19, 69),
    ];
    expect(reading.map((element) => element.textContent)).toEqual([
      "",
      "",
      "",
      "(MM/DD/YYYY)",
      "  End Date :",
      "",
      "",
      "",
      "(MM/DD/YYYY)",
      "The Report will be submitted for printing. Please confirm: ",
      "",
      "(Y/N)",
    ]);
    for (const [index, element] of reading.entries()) {
      const next = reading[index + 1];
      if (next !== undefined) {
        expect(element.compareDocumentPosition(next) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
          Node.DOCUMENT_POSITION_FOLLOWING,
        );
      }
    }
  });

  it("drops a server mark when corrected and at the next ENTER; a 200 leaves none", async () => {
    /** {@link noReportRequest} with Custom chosen, its end date still blank. */
    const noEndRequest: components["schemas"]["ReportRequest"] = {
      ...noReportRequest,
      custom: "X",
    };
    const requests = stubApi(
      { status: 400, body: noReport },
      { status: 400, body: noEndMonth },
      { status: 200, body: customSubmitted },
    );
    renderScreen();
    fill(noReportRequest);
    await pressEnter();
    await waitFor(() => {
      expect(messageRow().textContent).toBe(noReport.message);
      expectMarked(MONTHLY, noReport.message);
    });

    // An edit of another input keeps the mark; the next ENTER drops it, and its answer marks the
    // input it names.
    fireEvent.change(inputOf("Custom (Date Range)"), { target: { value: "X" } });
    await settle();
    expectMarked(MONTHLY, noReport.message);
    await pressEnter();
    await waitFor(() => {
      expect(messageRow().textContent).toBe(noEndMonth.message);
      expectMarked("End Date month", noEndMonth.message);
    });

    // Correcting the marked input drops its mark at once; the row keeps the server's message
    // until the next answer replaces it.
    fireEvent.change(inputOf("End Date month"), { target: { value: "07" } });
    await waitFor(() => {
      expectUnmarked();
    });
    expect(messageRow().textContent).toBe(noEndMonth.message);
    fireEvent.change(inputOf("End Date day"), { target: { value: "06" } });
    fireEvent.change(inputOf("End Date year"), { target: { value: "2022" } });
    await pressEnter();

    await waitFor(() => {
      expect(messageRow().textContent).toBe(customSubmitted.message);
    });
    expectUnmarked();
    expect(shown()).toEqual(BLANK);
    await settle();
    expect(await bodies(requests)).toEqual([noReportRequest, noEndRequest, customRequest]);
  });

  it("locks ENTER and the inputs while its request is in flight, keeps F3, then frees them", async () => {
    let release: () => void = () => undefined;
    const hold = new Promise<void>((resolve) => {
      release = resolve;
    });
    const requests = stubApi(
      { status: 200, body: monthlySubmitted, hold },
      { status: 400, body: confirmPrompt },
    );
    const { container } = renderScreen();
    const form = container.querySelector("form");
    if (form === null) {
      throw new Error("CORPT0A renders no form");
    }
    const keyBar = screen.getByRole("group", { name: "Function keys" });
    const back = screen.getByRole("button", { name: "F3=Back" });
    expect(keyBar.getAttribute("aria-busy")).toBe("false");
    expect(form.getAttribute("aria-busy")).toBe("false");
    fill(monthlyRequest);

    // The second submit arrives while the first is still being validated, before any request.
    fireEvent.submit(form);
    fireEvent.submit(form);
    await waitFor(() => {
      expect(requests).toHaveLength(1);
    });
    expect(keyBar.getAttribute("aria-busy")).toBe("true");
    expect(form.getAttribute("aria-busy")).toBe("true");
    expect(enterButton().getAttribute("aria-disabled")).toBe("true");
    expect(back.getAttribute("aria-disabled")).toBeNull();
    for (const field of FIELDS) {
      expect(inputOf(field.name).readOnly).toBe(true);
    }

    // The ENTER button, the Enter key outside the form and an unserved key all do nothing.
    fireEvent.click(enterButton());
    fireEvent.keyDown(document.body, { key: "Enter" });
    fireEvent.keyDown(document.body, { key: "F5" });
    await settle();
    expect(requests).toHaveLength(1);
    expect(messageRow().textContent).toBe("");

    release();
    await waitFor(() => {
      expect(messageRow().textContent).toBe(monthlySubmitted.message);
    });
    expect(keyBar.getAttribute("aria-busy")).toBe("false");
    expect(form.getAttribute("aria-busy")).toBe("false");
    expect(enterButton().getAttribute("aria-disabled")).toBeNull();
    for (const field of FIELDS) {
      expect(inputOf(field.name).readOnly).toBe(false);
    }

    // Once answered, the next ENTER is sent.
    fill(unconfirmedRequest);
    await pressEnter();
    await waitFor(() => {
      expect(messageRow().textContent).toBe(confirmPrompt.message);
    });
    await settle();
    expect(await bodies(requests)).toEqual([monthlyRequest, unconfirmedRequest]);
  });

  it("frees the keyboard when the schema refuses ENTER, so a corrected ENTER is sent", async () => {
    const requests = stubApi({ status: 200, body: customSubmitted });
    renderScreen();
    fill({ ...customRequest, start_day: "1a" });
    await pressEnter();
    await waitFor(() => {
      expect(messageRow().textContent).toBe(NOT_DIGITS);
    });
    expect(screen.getByRole("group", { name: "Function keys" }).getAttribute("aria-busy")).toBe(
      "false",
    );
    expect(enterButton().getAttribute("aria-disabled")).toBeNull();
    expect(inputOf(MONTHLY).readOnly).toBe(false);

    fill(customRequest);
    await pressEnter();
    await waitFor(() => {
      expect(messageRow().textContent).toBe(customSubmitted.message);
    });
    await settle();
    expect(await bodies(requests)).toEqual([customRequest]);
  });

  it("F3 goes back to the main menu while ENTER's request is in flight", async () => {
    const hold = new Promise<void>(() => undefined);
    const requests = stubApi({ status: 200, body: monthlySubmitted, hold });
    renderScreen();
    fill(monthlyRequest);
    await pressEnter();
    await waitFor(() => {
      expect(enterButton().getAttribute("aria-disabled")).toBe("true");
    });

    fireEvent.click(screen.getByRole("button", { name: "F3=Back" }));

    const stub = await screen.findByRole("region", { name: "Route stub" });
    expect(within(stub).getByTestId("stub-path").textContent).toBe("/menu");
    expect(requests).toHaveLength(1);
  });

  // audit: CORPT00C-005
  it.each([
    ...UNSERVED_F_KEYS.map((key) => ({ key, shiftKey: false })),
    // Shift+F3 is PF15, which CORPT00C does not serve: it does not go back.
    { key: "F3", shiftKey: true },
  ])(
    "$key (shift: $shiftKey) shows the invalid-key message, keeps the inputs, focuses Monthly",
    async ({ key, shiftKey }) => {
      const requests = stubApi();
      renderScreen();
      fill(customRequest);
      await moveFocus(inputOf(CONFIRM));

      pressKey(key, shiftKey);

      await waitFor(() => {
        expect(messageRow().textContent).toBe(INVALID_KEY_TEXT);
      });
      expect(messageRow().dataset.messageKind).toBe("error");
      await waitFor(() => {
        expect(document.activeElement).toBe(inputOf(MONTHLY));
      });
      expect(shown()).toEqual(typed(customRequest));
      expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Transaction Reports");
      await settle();
      expect(requests).toHaveLength(0);
    },
  );

  // audit: CORPT00C-004
  // audit: CORPT00-050
  it.each([
    { via: "the F3=Back button", byKey: false },
    { via: "the F3 key", byKey: true },
  ])("$via goes back to the main menu at /menu, sending nothing", async ({ byKey }) => {
    const requests = stubApi();
    renderScreen();
    fill(customRequest);
    const back = screen.getByRole("button", { name: "F3=Back" });
    expect([back.getAttribute("type"), back.dataset.variant, back.dataset.size]).toEqual([
      "button",
      "outline",
      "sm",
    ]);

    if (byKey) {
      pressKey("F3");
    } else {
      fireEvent.click(back);
    }

    const stub = await screen.findByRole("region", { name: "Route stub" });
    expect(within(stub).getByTestId("stub-path").textContent).toBe("/menu");
    expect(within(stub).getByTestId("stub-state").textContent).toBe("null");
    await settle();
    expect(requests).toHaveLength(0);
  });
});

/**
 * Renders the application's route table at `/reports`, so the sign-on guard that only the table
 * mounts stands in front of the screen. Each screen is imported by its route's loader, so a wait
 * for one to render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.CORPT00] });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

// The 401 clears the stored session (`api/client.ts`), and the sign-on guard in front of the
// route replaces the still-mounted screen with sign-on at once: the test presses no key and
// follows no link after ENTER.
describe("CORPT00 in the route table: a 401 ends the session", () => {
  it("a submission answered 401 replaces the screen with sign-on and clears the session, with no retry", async () => {
    const requests = stubApi({ status: 401, body: signonRequired });
    const router = renderRouteTable();
    expect(
      await screen.findByRole(
        "heading",
        { level: 1, name: "Transaction Reports" },
        { timeout: 5_000 },
      ),
    ).toBeDefined();

    fill(monthlyRequest);
    await pressEnter();

    expect(
      await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
    ).toBeDefined();
    expect(router.state.location.pathname).toBe(PATHS.COSGN00);
    expect(screen.queryByRole("heading", { level: 1, name: "Transaction Reports" })).toBeNull();
    expect(getSession()).toBeNull();
    expect(
      requests.map((request) => [
        request.method,
        new URL(request.url).pathname,
        request.headers.get("Authorization"),
      ]),
    ).toEqual([["POST", ENDPOINT, "Bearer test-token"]]);
    expect(await bodies(requests)).toEqual([monthlyRequest]);
    router.dispose();
  });
});

// The sign-on guard lives in the application's route tree, not in the screen, so this group
// renders that tree rather than the screen beside stubs.
describe("CORPT00 behind the route guard", () => {
  // audit: CORPT00C-001
  it("sends a browser with no session from /reports to /signon, with no request", async () => {
    clearSession();
    const requests = stubApi();
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, { initialEntries: [PATHS.CORPT00] });
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );

    // Only the sign-on screen loads, through the route tree; the refused screen is never imported.
    await waitFor(
      () => {
        expect(router.state.location.pathname).toBe(PATHS.COSGN00);
        expect(screen.getByRole("button", { name: "ENTER=Sign-on" })).toBeDefined();
      },
      { timeout: 4000 },
    );
    expect(screen.queryByRole("heading", { level: 1, name: "Transaction Reports" })).toBeNull();
    expect(screen.queryByLabelText(MONTHLY)).toBeNull();
    await settle();
    expect(requests).toHaveLength(0);
    router.dispose();
  });
});
