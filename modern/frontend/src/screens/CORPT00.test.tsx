import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import type { components } from "@/api/schema";
import { setSession } from "@/auth/session";
import { PATHS } from "@/routes";
import { CORPT00 } from "./CORPT00";

/*
 * Render test of the transaction reports screen (map CORPT0A, transaction CR00, program
 * CORPT00C, route `/reports`) in jsdom, with `fetch` stubbed for endpoint 14,
 * `POST /api/v1/reports/transactions`. Expected values are the audited results of
 * `modern/audit/screens.md` (`CORPT00-nnn`) and `modern/audit/online/CORPT00C.md`
 * (`CORPT00C-nnn`); every message below is a literal those rows quote, except the 401's, which
 * comes from the API's token extractor.
 *
 * Not tagged here: the `both` rows of the ten inputs (`LENGTH`, `NUM`), whose client half is
 * proved in `src/schemas/CORPT00.test.ts`, and the header label and value rows `CORPT00-001` to
 * `CORPT00-010`, which `ScreenShell` renders as plain text rather than the `Label` and read-only
 * `Input` those rows name. The server rows behind each fixture are proved by the endpoint tests.
 *
 * A `ScreenError.field` is the request member's wire name (`start_month`), as endpoint 14 sends
 * it and the screen maps it; the dotted `start_date.month` of the CORPT00C audit's prose is not
 * what the API emits.
 */

const ENDPOINT = "/api/v1/reports/transactions";

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

/** The text of the header field `name` (`data-field` of `ScreenShell`). */
function headerField(container: HTMLElement, name: string): string | null {
  return container.querySelector(`[data-field="${name}"]`)?.textContent ?? null;
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

  it("refuses a date part that is not digits before any request, focusing that part", async () => {
    // The `NUM` refusal itself is the client half of a `both` row, proved in the schema test;
    // this shows what the screen does with it: zod's message in the red row, the inputs kept.
    // screens.md records no message for the row, so the text is zod 3's default for a `regex`.
    const requests = stubApi();
    renderScreen();
    const request: components["schemas"]["ReportRequest"] = { ...customRequest, start_day: "1a" };
    fill(request);
    await pressEnter();

    await waitFor(() => {
      expect(messageRow().textContent).toBe("Invalid");
    });
    expect(messageRow().dataset.messageKind).toBe("error");
    expect(shown()).toEqual(typed(request));
    await waitFor(() => {
      expect(document.activeElement).toBe(inputOf("Start Date day"));
    });
    await settle();
    expect(requests).toHaveLength(0);
  });

  it("sends no second request for an ENTER while the first is in flight", async () => {
    let release: () => void = () => undefined;
    const hold = new Promise<void>((resolve) => {
      release = resolve;
    });
    const requests = stubApi({ status: 200, body: monthlySubmitted, hold });
    renderScreen();
    fill(monthlyRequest);
    await pressEnter();
    await waitFor(() => {
      expect(requests).toHaveLength(1);
    });
    await settle();

    fireEvent.click(enterButton());
    await settle();
    expect(requests).toHaveLength(1);

    release();
    await waitFor(() => {
      expect(messageRow().textContent).toBe(monthlySubmitted.message);
    });
    await settle();
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
