import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { onlineManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
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
import { COBIL00 } from "./COBIL00";

/*
 * Render test of the bill-payment screen (map COBIL0A, transaction CB00, program COBIL00C, route
 * `/bill-payment`) in jsdom, with `fetch` stubbed for endpoint 15, `POST /api/v1/bill-payments`.
 * Expected values are the audited results of `modern/audit/screens.md` (`COBIL00-nnn`) and
 * `modern/audit/online/COBIL00C.md` (`COBIL00C-nnn`). The balance is the 14-character text that
 * `PIC +9999999999.99` edits (ARITH-019); it is compared as text, never read as an amount. The
 * endpoint carries no card number, and no SSN or password appears.
 *
 * The header values are the read-only `Input`s the shared `ScreenShell` renders in the
 * `CardHeader` title rows. Their label rows `COBIL00-001`, `-004`, `-006` and `-009` are proved by
 * NAV (`frontend/e2e/nav.spec.ts`). The value rows `COBIL00-002`, `-003`, `-005`, `-007`, `-008`
 * and `-010` are tagged here, and so are their content rows (`COBIL00-025` to `COBIL00-030`), read
 * from those inputs' values.
 */

const ENDPOINT = "/api/v1/bill-payments";

/**
 * The classes of the shared map-field box, FieldGrid's `FIELD_INPUT`: `min-w-max` and
 * `field-sizing-content` keep a box never narrower than its value, `px-1` is the one inset every
 * map input shares and `text-sm` the inputs' type size. Restated here because the render-test
 * contract imports no module beside the screen, the session, the routes and the generated types,
 * so a box is checked against this list rather than against the constant that renders it.
 */
const MAP_FIELD_BOX = ["min-w-max", "px-1", "text-sm", "field-sizing-content"] as const;

/** The account of the last `acctdata.txt` record, whose balance is shown and paid. */
const ACCT = "00000000050";
/** That account's `ACCT-CURR-BAL` (`00000004920{`), as `PIC +9999999999.99` edits it. */
const BALANCE = "+0000000492.00";
/** An account endpoint 15 finds no record for (COBIL00C-014). */
const UNKNOWN_ACCT = "99999999999";
/** An account whose balance is zero (COBIL00C-018). */
const SETTLED_ACCT = "00000000070";
/** An account whose read fails on the server (COBIL00C-015). */
const FAILING_ACCT = "00000000500";
/** An account other than {@link ACCT} whose payment the server fails after reading it. */
const OTHER_ACCT = "00000000049";
/** The transaction ID the payment writes (COBIL00C-023). */
const NEW_TRAN_ID = "0000000000000301";
/** COBIL00C-028: two spaces after `successful.`, then the 16-digit ID and a full stop. */
const PAID_MESSAGE = "Payment successful.  Your Transaction ID is 0000000000000301.";

/** COBIL00C-019: a blank confirmation over a positive balance shows it and asks to confirm. */
const balanceShown: components["schemas"]["BillPaymentResult"] = {
  acct_id: ACCT,
  cleared: false,
  current_balance: BALANCE,
  message: "Confirm to make a bill payment...",
  new_tran_id: null,
};

/** COBIL00C-028: `Y` pays; the program empties the screen and names the new transaction. */
const paid: components["schemas"]["BillPaymentResult"] = {
  acct_id: "",
  cleared: true,
  current_balance: null,
  message: PAID_MESSAGE,
  new_tran_id: NEW_TRAN_ID,
};

/** COBIL00C-010: `N` empties the screen, with no balance, message or transaction. */
const cleared: components["schemas"]["BillPaymentResult"] = {
  acct_id: "",
  cleared: true,
  current_balance: null,
  message: null,
  new_tran_id: null,
};

/** COBIL00C-007: the account is blank. */
const emptyAcct: components["schemas"]["ScreenError"] = {
  message: "Acct ID can NOT be empty...",
  field: "acct_id",
};

/** COBIL00C-012: the confirmation is none of `Y`, `y`, `N`, `n` and blank. */
const invalidConfirm: components["schemas"]["ScreenError"] = {
  message: "Invalid value. Valid values are (Y/N)...",
  field: "confirm",
};

/** COBIL00C-014: no account has the key. */
const notFound: components["schemas"]["ScreenError"] = {
  message: "Account ID NOT found...",
  field: "acct_id",
};

/** COBIL00C-015: the account read fails. */
const lookupFailed: components["schemas"]["ScreenError"] = {
  message: "Unable to lookup Account...",
  field: "acct_id",
};

/** COBIL00C-018: the balance is zero or negative. */
const nothingToPay: components["schemas"]["ScreenError"] = {
  message: "You have nothing to pay...",
  field: "acct_id",
};

/** The payment block's failures, each after the account read: COBIL00C-024 to COBIL00C-033. */
const paymentFailures: components["schemas"]["ScreenError"][] = [
  // COBIL00C-024: the browse for the highest transaction ID fails.
  { message: "Unable to lookup Transaction...", field: "acct_id" },
  // COBIL00C-029: a transaction with the new ID already exists.
  { message: "Tran ID already exist...", field: "acct_id" },
  // COBIL00C-030: the transaction write fails.
  { message: "Unable to Add Bill pay Transaction...", field: "acct_id" },
  // COBIL00C-033: the balance update fails.
  { message: "Unable to Update Account...", field: "acct_id" },
];

/** `CCDA-MSG-INVALID-KEY PIC X(50)`, padded with spaces to its 50 bytes (COBIL00C-006). */
const INVALID_KEY_TEXT = "Invalid key pressed. Please see below...".padEnd(50, " ");

/** The twelve F-keys a browser sends. */
const ALL_F_KEYS = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];

/** The F-keys COBIL00C serves under no branch but `WHEN OTHER`: all but F3 and F4. */
const UNSERVED_F_KEYS = ALL_F_KEYS.filter((key) => key !== "F3" && key !== "F4");

/** Account, balance and confirmation empty: first entry, F4, `N` and a payment leave them so. */
const BLANK = { acct: "", balance: "", confirm: "" };

/** A JSON response, as the API sends every success and every `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** The 500 a request the stub does not expect gets, naming what it was. */
function unexpected(what: string): Response {
  const error: components["schemas"]["ScreenError"] = {
    message: `Unexpected request: ${what}`,
  };
  return json(500, error);
}

/** The string member `name` of a parsed request body, or `undefined` when it holds none. */
function member(body: unknown, name: string): string | undefined {
  const value: unknown =
    typeof body === "object" && body !== null ? Reflect.get(body, name) : undefined;
  return typeof value === "string" ? value : undefined;
}

/**
 * Endpoint 15 in the order COBIL00C's `PROCESS-ENTER-KEY` decides: the blank account first
 * (COBIL00C-007); then the confirmation, `N` or `n` clearing before any read (COBIL00C-010) and
 * any value but `Y`, `y` and blank refused (COBIL00C-012); then the account read (COBIL00C-014,
 * COBIL00C-015) and the zero balance (COBIL00C-018); last the balance (COBIL00C-019) or the
 * payment (COBIL00C-028). A body without the two strings, or an account the stub does not hold,
 * gets a 500.
 */
function billPayment(body: unknown): Response {
  const acctId = member(body, "acct_id");
  const confirm = member(body, "confirm");
  if (acctId === undefined || confirm === undefined) {
    return unexpected(`body ${JSON.stringify(body)}`);
  }
  if (acctId.trim() === "") {
    return json(400, emptyAcct);
  }
  if (confirm === "N" || confirm === "n") {
    return json(200, cleared);
  }
  const pays = confirm === "Y" || confirm === "y";
  if (!pays && confirm.trim() !== "") {
    return json(400, invalidConfirm);
  }
  switch (acctId) {
    case UNKNOWN_ACCT:
      return json(404, notFound);
    case FAILING_ACCT:
      return json(500, lookupFailed);
    case SETTLED_ACCT:
      return json(400, nothingToPay);
    case ACCT:
      return json(200, pays ? paid : balanceShown);
    default:
      return unexpected(`account ${acctId}`);
  }
}

/**
 * Replaces `fetch` with endpoint 15 as `answer` (by default {@link billPayment}) answers it; any
 * other request gets a 500. Each reply waits for `hold` when one is given. Returns every request
 * in the order the screen sent it.
 */
function stubApi(
  hold?: Promise<void>,
  answer: (body: unknown) => Response = billPayment,
): Request[] {
  const requests: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      requests.push(req);
      const { pathname } = new URL(req.url);
      if (req.method !== "POST" || pathname !== ENDPOINT) {
        return unexpected(`${req.method} ${pathname}`);
      }
      const body: unknown = await req.clone().json();
      await hold;
      return answer(body);
    }),
  );
  return requests;
}

/** The path of each request, in order. */
function paths(requests: readonly Request[]): string[] {
  return requests.map((request) => new URL(request.url).pathname);
}

/** The JSON body of each request, in order. */
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

/** A query client that never retries, so a stubbed failure shows at once. */
function testQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
}

/** COBIL00 at `/bill-payment` with `state`, and stubs at the routes F3 can return to. */
function renderScreen(state?: unknown) {
  const client = testQueryClient();
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COBIL00, state }]}>
        <Routes>
          <Route path={PATHS.COBIL00} element={<COBIL00 />} />
          <Route path={PATHS.COMEN01} element={<RouteStub />} />
          <Route path={PATHS.COTRN00} element={<RouteStub />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The route stub a transfer reached: its path and the route state it was given. */
async function reachedStub(): Promise<{ path: string; state: unknown }> {
  const stub = await screen.findByRole("region", { name: "Route stub" });
  const state: unknown = JSON.parse(within(stub).getByTestId("stub-state").textContent ?? "null");
  return { path: within(stub).getByTestId("stub-path").textContent ?? "", state };
}

/** The `<input>` the label `text` names, failing the test when it names anything else. */
function input(text: string): HTMLInputElement {
  const element = screen.getByLabelText(text);
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`"${text}" labels <${element.tagName.toLowerCase()}>, not an input`);
  }
  return element;
}

/** `ACTIDIN`, the account input (`COBIL00-012`, `COBIL00-013`). */
function acctInput(): HTMLInputElement {
  return input("Enter Acct ID:");
}

/** `CURBAL`, the balance output (`COBIL00-016`, `COBIL00-017`). */
function balanceOutput(): HTMLInputElement {
  return input("Your current balance is:");
}

/** `CONFIRM`, the confirmation input (`COBIL00-019`, `COBIL00-020`). */
function confirmInput(): HTMLInputElement {
  return input("Do you want to pay your balance now. Please confirm:");
}

/** The input a `ScreenError` names by its wire name. */
function inputNamed(wireName: string | null | undefined): HTMLInputElement {
  if (wireName === "acct_id") {
    return acctInput();
  }
  if (wireName === "confirm") {
    return confirmInput();
  }
  throw new Error(`No input of COBIL0A has the wire name ${String(wireName)}`);
}

/** What the account, balance and confirmation fields show. */
function shown(): { acct: string; balance: string; confirm: string } {
  return {
    acct: acctInput().value,
    balance: balanceOutput().value,
    confirm: confirmInput().value,
  };
}

/** The grid cell of a field: its map `POS` as row and column start and its `LENGTH` as span. */
function cell(element: HTMLElement): string | undefined {
  return element.parentElement?.className;
}

/** The message rows of the screen of one kind (`MessageLine`'s `data-message-kind`). */
function messageRows(kind: "error" | "info"): HTMLElement[] {
  return screen.getAllByRole("alert").filter((row) => row.dataset.messageKind === kind);
}

/** The text of the `ERRMSG` row (`COBIL00-023`) while it holds an error-kind message. */
function errorText(): string {
  const rows = messageRows("error");
  expect(rows).toHaveLength(1);
  return rows[0]?.textContent ?? "";
}

/** The map's one message row, `ERRMSG`, whatever kind its message gives it (COBIL00C-035). */
function messageRow(): HTMLElement {
  const rows = screen.getAllByRole("alert");
  expect(rows).toHaveLength(1);
  const [row] = rows;
  if (row === undefined) {
    throw new Error("COBIL0A renders no message row");
  }
  return row;
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

/** The row-24 button labelled `label`, checked to be a plain `outline`, `sm` button. */
function pfKeyButton(label: string): HTMLElement {
  const button = screen.getByRole("button", { name: label });
  expect([button.getAttribute("type"), button.dataset.variant, button.dataset.size]).toEqual([
    "button",
    "outline",
    "sm",
  ]);
  return button;
}

/** A keydown of `key` at the focused element, as the browser sends it; it bubbles to `window`. */
function pressKey(key: string, shiftKey = false): void {
  fireEvent.keyDown(document.activeElement ?? document.body, { key, shiftKey });
}

/**
 * Moves the cursor to `element`, so that the screen's own move of it is observable. The move runs
 * inside `waitFor`, which lets React render the blur it causes.
 */
async function moveFocus(element: HTMLElement): Promise<void> {
  await waitFor(() => {
    element.focus();
    expect(document.activeElement).toBe(element);
  });
}

/** Types the account and the confirmation, as given. */
function typeInputs(acct: string, confirm: string): void {
  fireEvent.change(acctInput(), { target: { value: acct } });
  fireEvent.change(confirmInput(), { target: { value: confirm } });
}

/** Presses ENTER through its `ENTER=Continue` button. */
function pressEnter(): void {
  fireEvent.click(screen.getByRole("button", { name: "ENTER=Continue" }));
}

/** ENTER over {@link ACCT} with a blank confirmation, until the balance shows. */
async function showBalance(): Promise<void> {
  typeInputs(ACCT, "");
  pressEnter();
  await waitFor(() => {
    expect(balanceOutput().value).toBe(BALANCE);
  });
}

/**
 * Lets pending effects, mutations and stubbed responses run, so that a request the screen would
 * send has been sent before a test counts the requests. The wait runs inside `waitFor`, so React
 * renders the updates it lets through, as `act` would.
 */
async function settle(): Promise<void> {
  await waitFor(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, 20);
    });
  });
}

/** Retypes the input `element` returns with `value`, then lets the form re-validate it. */
function retype(element: () => HTMLInputElement, value: string): () => Promise<void> {
  return async () => {
    fireEvent.change(element(), { target: { value } });
    await settle();
  };
}

/** Presses ENTER and waits until the message row shows `text`. */
function enterUntil(text: string): () => Promise<void> {
  return async () => {
    pressEnter();
    await waitFor(() => {
      expect(messageRow().textContent).toBe(text);
    });
  };
}

/** Presses F4=Clear and waits until the screen is empty. */
async function clearScreen(): Promise<void> {
  fireEvent.click(pfKeyButton("F4=Clear"));
  await waitFor(() => {
    expect(shown()).toEqual(BLANK);
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

describe("COBIL00 bill payment screen", () => {
  // audit: COBIL00-011
  // audit: COBIL00-012
  // audit: COBIL00-015
  // audit: COBIL00-016
  // audit: COBIL00-017
  // audit: COBIL00-019
  // audit: COBIL00-022
  // audit: COBIL00-024
  // audit: COBIL00-031
  it("renders the map: title, labelled fields at their map cells, rule, hint, key row", () => {
    stubApi();
    renderScreen({ from: PATHS.COMEN01 });

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Bill Payment");

    // Each label is its INITIAL, trailing space included; each cell is the field's POS and LENGTH.
    const acct = acctInput();
    expect(acct.labels?.[0]?.textContent).toBe("Enter Acct ID:");
    expect(acct.maxLength).toBe(11);
    expect(acct.readOnly).toBe(false);
    expect(cell(acct)).toBe("row-start-6 col-start-21 col-span-11");

    const balance = balanceOutput();
    expect(balance.labels?.[0]?.textContent).toBe("Your current balance is: ");
    expect(balance.readOnly).toBe(true);
    expect(cell(balance)).toBe("row-start-11 col-start-32 col-span-14");

    const confirm = confirmInput();
    expect(confirm.labels?.[0]?.textContent).toBe(
      "Do you want to pay your balance now. Please confirm: ",
    );
    expect(confirm.maxLength).toBe(1);
    expect(confirm.readOnly).toBe(false);
    expect(cell(confirm)).toBe("row-start-15 col-start-60 col-span-1");

    // All three take the shared map-field box, `MAP_FIELD_BOX` with `size` = the LENGTH, so a
    // full-length value shows on the narrowest frame.
    for (const [box, length] of [
      [acct, 11],
      [balance, 14],
      [confirm, 1],
    ] as const) {
      expect(box.size).toBe(length);
      expect(MAP_FIELD_BOX.filter((name) => !box.classList.contains(name))).toEqual([]);
    }

    expect(screen.getByText("-".repeat(70)).textContent).toHaveLength(70);
    expect(screen.getByText("(Y/N)").textContent).toBe("(Y/N)");

    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(
      keys.map((key) => [
        key.textContent,
        key.getAttribute("type"),
        key.dataset.variant,
        key.dataset.size,
      ]),
    ).toEqual([
      ["ENTER=Continue", "submit", "default", "sm"],
      ["F3=Back", "button", "outline", "sm"],
      ["F4=Clear", "button", "outline", "sm"],
    ]);

    // The map declares no INFOMSG row, only ERRMSG.
    expect(messageRows("info")).toHaveLength(0);
    expect(messageRows("error")).toHaveLength(1);
  });

  // audit: COBIL00-014
  // audit: COBIL00-018
  // audit: COBIL00-021
  it.each([
    { field: "ACTIDIN", row: 6, start: 21, stopper: 33 },
    { field: "CURBAL", row: 11, start: 32, stopper: 47 },
    { field: "CONFIRM", row: 15, start: 60, stopper: 62 },
  ])("leaves no element at the zero-length stopper that ends $field", (map) => {
    stubApi();
    renderScreen({ from: PATHS.COMEN01 });

    // The same query finds the field's own cell, so the empty stopper column is no selector slip.
    const cellsAt = (col: number) =>
      document.querySelectorAll(`.row-start-${map.row}.col-start-${col}`);
    expect(cellsAt(map.start)).toHaveLength(1);
    expect(cellsAt(map.stopper)).toHaveLength(0);
  });

  // audit: COBIL00C-001
  it("sends a browser with no session from /bill-payment to /signon, with no request", async () => {
    // The guard lives in the application's route tree, not in the screen, so this test renders
    // that tree rather than the screen with stubs.
    clearSession();
    const requests = stubApi();
    const router = createMemoryRouter(routes, { initialEntries: [PATHS.COBIL00] });
    render(
      <QueryClientProvider client={testQueryClient()}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );

    // Only the sign-on screen loads, through the route tree; the refused screen is never imported.
    await waitFor(
      () => {
        expect(router.state.location.pathname).toBe(PATHS.COSGN00);
        expect(screen.getByRole("heading", { level: 1 }).textContent).not.toBe("Bill Payment");
      },
      { timeout: 4000 },
    );
    expect(screen.queryByText("Enter Acct ID:")).toBeNull();
    await settle();
    expect(requests).toHaveLength(0);
    router.dispose();
  });

  // audit: COBIL00C-002
  it("opens cleared, with the cursor on the account, and sends nothing until ENTER", async () => {
    const requests = stubApi();
    renderScreen({ from: PATHS.COMEN01 });

    expect(shown()).toEqual(BLANK);
    expect(errorText()).toBe("");
    expect(document.activeElement).toBe(acctInput());

    await settle();
    expect(requests).toHaveLength(0);
  });

  // audit: COBIL00C-036
  // audit: COBIL00-025
  // audit: COBIL00-026
  // audit: COBIL00-027
  // audit: COBIL00-028
  // audit: COBIL00-029
  // audit: COBIL00-030
  it("fills header rows 1 and 2 at every send: CB00, COBIL00C, titles, date, time", async () => {
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
    stubApi();
    const { container } = renderScreen({ from: PATHS.COMEN01 });

    expect(headerField(container, "TRNNAME")).toBe("CB00");
    expect(headerField(container, "PGMNAME")).toBe("COBIL00C");
    expect(headerField(container, "TITLE01")).toBe("      AWS Mainframe Modernization       ");
    expect(headerField(container, "TITLE02")).toBe("              CardDemo                  ");
    expect(headerField(container, "CURDATE")).toBe("01/02/24");
    expect(headerField(container, "CURTIME")).toBe("03:04:05");

    vi.setSystemTime(new Date(2025, 11, 31, 23, 59, 58));
    await showBalance();

    expect(headerField(container, "TRNNAME")).toBe("CB00");
    expect(headerField(container, "PGMNAME")).toBe("COBIL00C");
    expect(headerField(container, "CURDATE")).toBe("12/31/25");
    expect(headerField(container, "CURTIME")).toBe("23:59:58");
  });

  // The values these fields show are proved under COBIL00-025 to COBIL00-030.
  it.each([
    // audit: COBIL00-002
    {
      id: "COBIL00-002",
      name: "TRNNAME",
      row: 1,
      col: 7,
      length: 4,
      label: "Tran:",
      shows: "CB00",
    },
    // audit: COBIL00-003
    {
      id: "COBIL00-003",
      name: "TITLE01",
      row: 1,
      col: 21,
      length: 40,
      label: null,
      shows: "      AWS Mainframe Modernization       ",
    },
    // audit: COBIL00-005
    {
      id: "COBIL00-005",
      name: "CURDATE",
      row: 1,
      col: 71,
      length: 8,
      label: "Date:",
      shows: expect.stringMatching(/^\d{2}\/\d{2}\/\d{2}$/),
    },
    // audit: COBIL00-007
    {
      id: "COBIL00-007",
      name: "PGMNAME",
      row: 2,
      col: 7,
      length: 8,
      label: "Prog:",
      shows: "COBIL00C",
    },
    // audit: COBIL00-008
    {
      id: "COBIL00-008",
      name: "TITLE02",
      row: 2,
      col: 21,
      length: 40,
      label: null,
      shows: "              CardDemo                  ",
    },
    // audit: COBIL00-010
    {
      id: "COBIL00-010",
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
      const { container } = renderScreen({ from: PATHS.COMEN01 });

      const header = container.querySelector('[data-slot="card-header"]');
      if (!(header instanceof HTMLElement)) {
        throw new Error("COBIL0A renders no card header");
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

  // audit: COBIL00C-035
  // audit: COBIL00C-037
  // audit: COBIL00-017
  // audit: COBIL00-031
  it("ENTER with a blank confirmation sends both inputs, shows the balance as sent", async () => {
    const requests = stubApi();
    renderScreen({ from: PATHS.COMEN01 });

    typeInputs(ACCT, "");
    pressEnter();

    // The 14 characters of the response, shown unchanged in the read-only CURBAL.
    await waitFor(() => {
      expect(balanceOutput().value).toBe(BALANCE);
    });
    expect(balanceOutput().value).toHaveLength(14);
    expect(balanceOutput().readOnly).toBe(true);
    expect(shown()).toEqual({ acct: ACCT, balance: BALANCE, confirm: "" });
    // The prompt of a 200 is red, as every message but the payment's is (COBIL00C-035).
    expect(errorText()).toBe("Confirm to make a bill payment...");
    expect(messageRows("error")[0]?.classList.contains("text-destructive")).toBe(true);
    await waitFor(() => {
      expect(document.activeElement).toBe(confirmInput());
    });

    expect(requests).toHaveLength(1);
    const [request] = requests;
    expect(request?.method).toBe("POST");
    expect(paths(requests)).toEqual([ENDPOINT]);
    expect(request?.headers.get("Authorization")).toBe("Bearer test-token");
    // Only the two inputs travel; the balance the screen shows is never sent (COBIL00C-037).
    expect(await bodies(requests)).toStrictEqual([{ acct_id: ACCT, confirm: "" }]);
  });

  // audit: COBIL00-017
  it.each([
    { title: "on first display", reach: settle },
    { title: "with a balance shown", reach: showBalance },
  ])("keeps the ASKIP balance out of the Tab order and the two inputs in it $title", async ({
    reach,
  }) => {
    stubApi();
    renderScreen({ from: PATHS.COMEN01 });
    await reach();

    // The 3270 Tab key skips a protected field: CURBAL is read-only and no Tab stop, while
    // ACTIDIN and CONFIRM stay enterable Tab stops.
    expect(
      [acctInput(), balanceOutput(), confirmInput()].map((element) => [
        element.readOnly,
        element.tabIndex,
      ]),
    ).toEqual([
      [false, 0],
      [true, -1],
      [false, 0],
    ]);
  });

  // audit: COBIL00-031
  it("the Enter key with the cursor outside the inputs submits as the button does", async () => {
    const requests = stubApi();
    renderScreen({ from: PATHS.COMEN01 });
    typeInputs(ACCT, "");

    // jsdom performs no implicit submission from a focused input, so the key is pressed where
    // the browser leaves it to the screen: with no element of the form focused. The blur runs
    // inside `waitFor`, which lets React render it.
    await waitFor(() => {
      acctInput().blur();
      expect(document.activeElement).toBe(document.body);
    });
    pressKey("Enter");

    await waitFor(() => {
      expect(balanceOutput().value).toBe(BALANCE);
    });
    expect(errorText()).toBe("Confirm to make a bill payment...");
    expect(await bodies(requests)).toStrictEqual([{ acct_id: ACCT, confirm: "" }]);
  });

  // audit: COBIL00C-035
  // audit: COBIL00C-037
  // audit: COBIL00-023
  it.each([
    { confirm: "Y", title: "pays and names the new transaction in green", text: PAID_MESSAGE },
    { confirm: "y", title: "pays as Y does", text: PAID_MESSAGE },
    { confirm: "N", title: "clears, with an empty message row", text: "" },
    { confirm: "n", title: "clears as N does", text: "" },
  ])("confirmation $confirm $title, emptying inputs and balance", async (row) => {
    const requests = stubApi();
    renderScreen({ from: PATHS.COMEN01 });
    await showBalance();
    await waitFor(() => {
      expect(document.activeElement).toBe(confirmInput());
    });

    fireEvent.change(confirmInput(), { target: { value: row.confirm } });
    pressEnter();

    await waitFor(() => {
      expect(shown()).toEqual(BLANK);
    });
    // Only the payment's message is green: the `default` Alert of an info-kind row. The empty row
    // a cleared screen shows stays the red ERRMSG.
    const green = row.text !== "";
    const message = messageRow();
    expect(message.textContent).toBe(row.text);
    expect(message.dataset.messageKind).toBe(green ? "info" : "error");
    expect(message.classList.contains("text-destructive")).toBe(!green);
    expect(message.classList.contains("text-card-foreground")).toBe(green);
    // ERRMSG is `BRT` whatever its colour, so the green payment text is semibold as an error is.
    const description = message.querySelector('[data-slot="alert-description"]');
    expect(description?.classList.contains("font-semibold")).toBe(true);
    await waitFor(() => {
      expect(document.activeElement).toBe(acctInput());
    });

    expect(paths(requests)).toEqual([ENDPOINT, ENDPOINT]);
    expect(await bodies(requests)).toStrictEqual([
      { acct_id: ACCT, confirm: "" },
      { acct_id: ACCT, confirm: row.confirm },
    ]);
  });

  // audit: COBIL00C-035
  // audit: COBIL00C-037
  // audit: COBIL00-023
  // The errors sent before the account read resend the balance shown, as `CURBAL` is `FSET`,
  // whichever account was typed; the zero balance, sent after it, comes with a balance the
  // screen never showed, so the screen empties its own.
  it.each([
    {
      title: "the 404 of an unknown account",
      acct: UNKNOWN_ACCT,
      confirm: "",
      error: notFound,
      balance: BALANCE,
    },
    {
      title: "the 500 of a failed read",
      acct: FAILING_ACCT,
      confirm: "",
      error: lookupFailed,
      balance: BALANCE,
    },
    {
      title: "the 400 of a zero balance",
      acct: SETTLED_ACCT,
      confirm: "Y",
      error: nothingToPay,
      balance: "",
    },
    {
      title: "the 400 of confirmation X",
      acct: ACCT,
      confirm: "X",
      error: invalidConfirm,
      balance: BALANCE,
    },
    {
      title: "the 400 of confirmation X over another account",
      acct: UNKNOWN_ACCT,
      confirm: "X",
      error: invalidConfirm,
      balance: BALANCE,
    },
    {
      title: "the 400 of a blank account",
      acct: "",
      confirm: "Y",
      error: emptyAcct,
      balance: BALANCE,
    },
  ])("shows $title in red, keeps the inputs, focuses the field it names", async (row) => {
    const requests = stubApi();
    renderScreen({ from: PATHS.COMEN01 });
    await showBalance();

    typeInputs(row.acct, row.confirm);
    // The cursor leaves both inputs, so its move to the field the error names is observable.
    const enter = screen.getByRole("button", { name: "ENTER=Continue" });
    await moveFocus(enter);
    fireEvent.click(enter);

    await waitFor(() => {
      expect(errorText()).toBe(row.error.message);
    });
    // ERRMSG is RED: the `destructive` variant of the shadcn/ui `Alert`, announced politely.
    const [errorRow] = messageRows("error");
    expect(errorRow?.getAttribute("aria-live")).toBe("polite");
    expect(errorRow?.classList.contains("text-destructive")).toBe(true);
    // The inputs stay as typed; the balance shown before the error stays, or goes.
    expect(shown()).toEqual({ acct: row.acct, balance: row.balance, confirm: row.confirm });
    await waitFor(() => {
      expect(document.activeElement).toBe(inputNamed(row.error.field));
    });
    // The input the error names, and only it, is marked invalid; the text shows once, in ERRMSG.
    const named = inputNamed(row.error.field);
    for (const field of [acctInput(), confirmInput()]) {
      expect(field.getAttribute("aria-invalid")).toBe(String(field === named));
    }
    expect(screen.getAllByText(row.error.message)).toHaveLength(1);

    expect(await bodies(requests)).toStrictEqual([
      { acct_id: ACCT, confirm: "" },
      { acct_id: row.acct, confirm: row.confirm },
    ]);
  });

  // The errors COBIL00C sends after the account read come with the balance it read, which a
  // `ScreenError` does not carry: a zero balance is one the screen never showed, and a failed
  // payment's balance is the one shown only over the account it was shown for.
  // audit: COBIL00C-037
  it.each([
    {
      title: "the 400 of a zero balance over the account shown with confirmation Y",
      acct: ACCT,
      confirm: "Y",
      status: 400,
      error: nothingToPay,
      balance: "",
    },
    {
      title: "the 400 of a zero balance over the account shown with a blank confirmation",
      acct: ACCT,
      confirm: "",
      status: 400,
      error: nothingToPay,
      balance: "",
    },
    ...paymentFailures.flatMap((error) => [
      {
        title: `the 500 ${error.message} over the account shown`,
        acct: ACCT,
        confirm: "Y",
        status: 500,
        error,
        balance: BALANCE,
      },
      {
        title: `the 500 ${error.message} over another account`,
        acct: OTHER_ACCT,
        confirm: "Y",
        status: 500,
        error,
        balance: "",
      },
    ]),
  ])('after a balance, $title leaves the balance "$balance", keeps the inputs', async (row) => {
    stubApi();
    renderScreen({ from: PATHS.COMEN01 });
    await showBalance();
    const requests = stubApi(undefined, () => json(row.status, row.error));

    typeInputs(row.acct, row.confirm);
    const enter = screen.getByRole("button", { name: "ENTER=Continue" });
    await moveFocus(enter);
    fireEvent.click(enter);

    await waitFor(() => {
      expect(errorText()).toBe(row.error.message);
    });
    expect(messageRows("error")[0]?.classList.contains("text-destructive")).toBe(true);
    expect(shown()).toEqual({ acct: row.acct, balance: row.balance, confirm: row.confirm });
    await waitFor(() => {
      expect(document.activeElement).toBe(acctInput());
    });
    expect(acctInput().getAttribute("aria-invalid")).toBe("true");
    expect(await bodies(requests)).toStrictEqual([{ acct_id: row.acct, confirm: row.confirm }]);
  });

  // The replacement's form state, which no audit row states: the input a `ScreenError` names stays
  // marked invalid until it is retyped, the next ENTER is answered, or F4 clears the screen, and
  // no 200 marks an input. A failed read is answered 500 once, then the read succeeds.
  it.each([
    {
      title: "the account is retyped",
      acct: "",
      confirm: "",
      answer: billPayment,
      error: emptyAcct,
      clear: retype(acctInput, ACCT),
      sent: 0,
    },
    {
      title: "the confirmation is retyped",
      acct: ACCT,
      confirm: "X",
      answer: billPayment,
      error: invalidConfirm,
      clear: retype(confirmInput, ""),
      sent: 0,
    },
    {
      title: "the next ENTER shows the balance",
      acct: ACCT,
      confirm: "",
      answer: () => json(500, lookupFailed),
      error: lookupFailed,
      clear: enterUntil("Confirm to make a bill payment..."),
      sent: 1,
    },
    {
      title: "the next ENTER pays",
      acct: ACCT,
      confirm: "Y",
      answer: () => json(500, lookupFailed),
      error: lookupFailed,
      clear: enterUntil(PAID_MESSAGE),
      sent: 1,
    },
    {
      title: "F4 clears an account error",
      acct: "",
      confirm: "",
      answer: billPayment,
      error: emptyAcct,
      clear: clearScreen,
      sent: 0,
    },
    {
      title: "F4 clears a confirmation error",
      acct: ACCT,
      confirm: "X",
      answer: billPayment,
      error: invalidConfirm,
      clear: clearScreen,
      sent: 0,
    },
  ])("marks the input a ScreenError names invalid until $title", async (row) => {
    stubApi(undefined, row.answer);
    renderScreen({ from: PATHS.COMEN01 });
    typeInputs(row.acct, row.confirm);
    pressEnter();
    await waitFor(() => {
      expect(errorText()).toBe(row.error.message);
    });
    expect(inputNamed(row.error.field).getAttribute("aria-invalid")).toBe("true");

    // From here endpoint 15 answers as the fixtures do.
    const requests = stubApi();
    await row.clear();

    await waitFor(() => {
      expect(acctInput().getAttribute("aria-invalid")).toBe("false");
      expect(confirmInput().getAttribute("aria-invalid")).toBe("false");
    });
    expect(requests).toHaveLength(row.sent);
  });

  // A failure of the replacement alone, which no audit row covers: a reply without a JSON object
  // body, such as an empty 502 from a gateway, carries no message of the program's, so the row
  // names the status, and no field takes the cursor. openapi-fetch hands over a 204's or a
  // `Content-Length: 0` body as nothing and any other error body as its text or parsed JSON, so
  // an empty body of no declared length, a chunked one, plain text and JSON `null` are no
  // `ScreenError` either; a success without a body has no data.
  it.each([
    {
      reply: "an empty 502 from a gateway",
      response: () => new Response(null, { status: 502, statusText: "Bad Gateway" }),
      message: "HTTP 502 Bad Gateway",
    },
    {
      reply: "an empty 500 of Content-Length 0",
      response: () =>
        new Response(null, {
          status: 500,
          statusText: "Internal Server Error",
          headers: { "Content-Length": "0" },
        }),
      message: "HTTP 500 Internal Server Error",
    },
    {
      reply: "an empty 502 without a reason phrase",
      response: () => new Response(null, { status: 502, headers: { "Content-Length": "0" } }),
      message: "HTTP 502",
    },
    {
      reply: "an empty 500 without Content-Length",
      response: () => new Response(null, { status: 500, statusText: "Internal Server Error" }),
      message: "HTTP 500 Internal Server Error",
    },
    {
      reply: "an empty, chunked 500",
      response: () =>
        new Response(null, {
          status: 500,
          statusText: "Internal Server Error",
          headers: { "Content-Type": "text/plain", "Transfer-Encoding": "chunked" },
        }),
      message: "HTTP 500 Internal Server Error",
    },
    {
      reply: "a plain-text 502",
      response: () =>
        new Response("Bad Gateway", {
          status: 502,
          statusText: "Bad Gateway",
          headers: { "Content-Type": "text/plain" },
        }),
      message: "HTTP 502 Bad Gateway",
    },
    {
      reply: "a 500 whose JSON body is null",
      response: () => json(500, null),
      message: "HTTP 500",
    },
    {
      reply: "a 204",
      response: () => new Response(null, { status: 204, statusText: "No Content" }),
      message: "HTTP 204 No Content",
    },
    {
      reply: "an empty 200",
      response: () => new Response(null, { status: 200, statusText: "OK" }),
      message: "HTTP 200 OK",
    },
  ])("shows $reply by its status, keeps fields, cursor", async ({ response, message }) => {
    stubApi();
    renderScreen({ from: PATHS.COMEN01 });
    await showBalance();
    const requests = stubApi(undefined, response);

    const enter = screen.getByRole("button", { name: "ENTER=Continue" });
    await moveFocus(enter);
    fireEvent.click(enter);

    await waitFor(() => {
      expect(errorText()).toBe(message);
    });
    expect(shown()).toEqual({ acct: ACCT, balance: BALANCE, confirm: "" });
    expect(document.activeElement).toBe(enter);
    // A failure that names no field marks no input invalid.
    expect(acctInput().getAttribute("aria-invalid")).toBe("false");
    expect(confirmInput().getAttribute("aria-invalid")).toBe("false");
    expect(await bodies(requests)).toStrictEqual([{ acct_id: ACCT, confirm: "" }]);
  });

  // audit: COBIL00C-005
  // audit: COBIL00-033
  it("F4 empties every field and the message row, refocuses, sends nothing", async () => {
    const requests = stubApi();
    renderScreen({ from: PATHS.COMEN01 });
    await showBalance();
    fireEvent.change(confirmInput(), { target: { value: "Y" } });

    // F4=Clear by its button, over a balance and its prompt, with the cursor away from the inputs.
    const clear = pfKeyButton("F4=Clear");
    await moveFocus(clear);
    fireEvent.click(clear);
    await waitFor(() => {
      expect(shown()).toEqual(BLANK);
    });
    expect(errorText()).toBe("");
    expect(document.activeElement).toBe(acctInput());

    // The F4 key, over an error message.
    typeInputs(UNKNOWN_ACCT, "");
    pressEnter();
    await waitFor(() => {
      expect(errorText()).toBe(notFound.message);
    });
    await moveFocus(clear);
    pressKey("F4");
    await waitFor(() => {
      expect(errorText()).toBe("");
    });
    expect(shown()).toEqual(BLANK);
    expect(document.activeElement).toBe(acctInput());

    await settle();
    expect(await bodies(requests)).toStrictEqual([
      { acct_id: ACCT, confirm: "" },
      { acct_id: UNKNOWN_ACCT, confirm: "" },
    ]);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Bill Payment");
  });

  // audit: COBIL00C-004
  // audit: COBIL00-032
  it.each([
    {
      title: "F3=Back after entry from the main menu returns to /menu",
      state: { from: PATHS.COMEN01 },
      byKey: false,
      target: "/menu",
    },
    {
      title: "the F3 key after entry from the main menu returns to /menu",
      state: { from: PATHS.COMEN01 },
      byKey: true,
      target: "/menu",
    },
    {
      title: "F3=Back with no recorded origin returns to /menu",
      state: undefined,
      byKey: false,
      target: "/menu",
    },
    {
      title: "F3=Back returns to the screen its route state names as the origin",
      state: { from: PATHS.COTRN00 },
      byKey: false,
      target: "/transactions",
    },
    {
      title: "F3=Back with an origin that is no route of the application returns to /menu",
      state: { from: "https://example.invalid/transactions" },
      byKey: false,
      target: "/menu",
    },
  ])("$title, recording COBIL00 as the origin, with no request", async (row) => {
    const requests = stubApi();
    renderScreen(row.state);

    const back = pfKeyButton("F3=Back");
    if (row.byKey) {
      pressKey("F3");
    } else {
      fireEvent.click(back);
    }

    const stub = await reachedStub();
    expect(stub.path).toBe(row.target);
    expect(stub.state).toEqual({ from: "/bill-payment" });
    await settle();
    expect(requests).toHaveLength(0);
  });

  // audit: COBIL00C-006
  it.each([
    ...UNSERVED_F_KEYS.map((key) => ({ title: key, key, shiftKey: false })),
    // PF13 to PF24: COBIL00C does not fold them onto PF1 to PF12, so Shift+F3 is no F3.
    ...ALL_F_KEYS.map((key) => ({ title: `Shift+${key}`, key, shiftKey: true })),
  ])("$title shows the invalid-key message in red, keeps every field", async (row) => {
    const requests = stubApi();
    renderScreen({ from: PATHS.COMEN01 });
    await showBalance();
    fireEvent.change(confirmInput(), { target: { value: "Y" } });

    pressKey(row.key, row.shiftKey);

    await waitFor(() => {
      expect(errorText()).toBe(INVALID_KEY_TEXT);
    });
    expect(messageRows("error")[0]?.classList.contains("text-destructive")).toBe(true);
    expect(shown()).toEqual({ acct: ACCT, balance: BALANCE, confirm: "Y" });
    expect(screen.queryByRole("region", { name: "Route stub" })).toBeNull();

    await settle();
    expect(requests).toHaveLength(1);
  });

  // The terminal keyboard stays locked until the program's reply arrives; the screen disables
  // ENTER and F4 while its request is in flight, so a second ENTER cannot pay twice.
  it("locks ENTER, F4 and the inputs while its request is in flight, keeps F3, then frees them", async () => {
    const releases: (() => void)[] = [];
    const hold = new Promise<void>((resolve) => {
      releases.push(resolve);
    });
    const requests = stubApi(hold);
    const { container } = renderScreen({ from: PATHS.COMEN01 });
    const form = container.querySelector("form");
    if (form === null) {
      throw new Error("COBIL0A renders no form");
    }
    const keyBar = screen.getByRole("group", { name: "Function keys" });
    const enter = screen.getByRole("button", { name: "ENTER=Continue" });
    const clear = pfKeyButton("F4=Clear");
    const back = pfKeyButton("F3=Back");
    expect(keyBar.getAttribute("aria-busy")).toBe("false");
    expect(form.getAttribute("aria-busy")).toBe("false");

    typeInputs(ACCT, "");
    // The second submit arrives while the first is still being validated, before any request.
    fireEvent.submit(form);
    fireEvent.submit(form);
    await waitFor(() => {
      expect(requests).toHaveLength(1);
    });
    expect(keyBar.getAttribute("aria-busy")).toBe("true");
    expect(form.getAttribute("aria-busy")).toBe("true");
    expect(enter.getAttribute("aria-disabled")).toBe("true");
    expect(clear.getAttribute("aria-disabled")).toBe("true");
    expect(back.getAttribute("aria-disabled")).toBeNull();
    expect([acctInput().readOnly, confirmInput().readOnly]).toEqual([true, true]);

    // The buttons, the keys and an unserved key all do nothing until the reply arrives.
    pressEnter();
    fireEvent.click(clear);
    fireEvent.keyDown(document.body, { key: "Enter" });
    pressKey("F4");
    pressKey("F5");
    await settle();
    expect(requests).toHaveLength(1);
    expect(shown()).toEqual({ acct: ACCT, balance: "", confirm: "" });
    expect(errorText()).toBe("");

    for (const release of releases) {
      release();
    }
    await waitFor(() => {
      expect(balanceOutput().value).toBe(BALANCE);
    });
    expect(errorText()).toBe("Confirm to make a bill payment...");
    expect(keyBar.getAttribute("aria-busy")).toBe("false");
    expect(form.getAttribute("aria-busy")).toBe("false");
    expect([enter.getAttribute("aria-disabled"), clear.getAttribute("aria-disabled")]).toEqual([
      null,
      null,
    ]);
    expect([acctInput().readOnly, confirmInput().readOnly]).toEqual([false, false]);

    // Once answered, the next ENTER is sent.
    fireEvent.change(confirmInput(), { target: { value: "N" } });
    pressEnter();
    await waitFor(() => {
      expect(shown()).toEqual(BLANK);
    });
    await settle();
    expect(await bodies(requests)).toStrictEqual([
      { acct_id: ACCT, confirm: "" },
      { acct_id: ACCT, confirm: "N" },
    ]);
  });

  it("frees the keyboard when the schema refuses ENTER, so a corrected ENTER is sent", async () => {
    const requests = stubApi();
    renderScreen({ from: PATHS.COMEN01 });
    typeInputs(ACCT, "YY");
    pressEnter();
    await waitFor(() => {
      expect(errorText()).toBe("String must contain at most 1 character(s)");
    });
    expect(screen.getByRole("group", { name: "Function keys" }).getAttribute("aria-busy")).toBe(
      "false",
    );
    expect(pfKeyButton("F4=Clear").getAttribute("aria-disabled")).toBeNull();
    expect(confirmInput().readOnly).toBe(false);

    fireEvent.change(confirmInput(), { target: { value: "" } });
    pressEnter();
    await waitFor(() => {
      expect(balanceOutput().value).toBe(BALANCE);
    });
    await settle();
    expect(await bodies(requests)).toStrictEqual([{ acct_id: ACCT, confirm: "" }]);
  });

  it("F3 goes back while ENTER's request is in flight", async () => {
    const requests = stubApi(new Promise<void>(() => undefined));
    renderScreen({ from: PATHS.COMEN01 });
    typeInputs(ACCT, "");
    pressEnter();
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: "ENTER=Continue" }).getAttribute("aria-disabled"),
      ).toBe("true");
    });

    fireEvent.click(pfKeyButton("F3=Back"));

    const stub = await reachedStub();
    expect(stub.path).toBe("/menu");
    expect(requests).toHaveLength(1);
  });

  // COBIL00-013 and COBIL00-020 are `both` rows: the schema's half is proved in
  // src/schemas/COBIL00.test.ts and the server's in the endpoint tests. This shows what the screen
  // does with the schema's refusal of a value `maxLength` would stop at the keyboard.
  it.each([
    {
      title: "a 12-character account",
      acct: "000000000501",
      confirm: "",
      message: "String must contain at most 11 character(s)",
      refused: acctInput,
    },
    {
      title: "a 2-character confirmation",
      acct: ACCT,
      confirm: "YY",
      message: "String must contain at most 1 character(s)",
      refused: confirmInput,
    },
  ])("ENTER with $title sends nothing and shows the schema's message", async (row) => {
    const requests = stubApi();
    renderScreen({ from: PATHS.COMEN01 });

    typeInputs(row.acct, row.confirm);
    const enter = screen.getByRole("button", { name: "ENTER=Continue" });
    await moveFocus(enter);
    fireEvent.click(enter);

    await waitFor(() => {
      expect(errorText()).toBe(row.message);
    });
    await waitFor(() => {
      expect(document.activeElement).toBe(row.refused());
    });
    expect(shown()).toEqual({ acct: row.acct, balance: "", confirm: row.confirm });
    await settle();
    expect(requests).toHaveLength(0);
  });
});

// Endpoint 15 takes the account and the confirmation in its JSON body alone, and every request
// carries the stored session's token.
describe("COBIL00: endpoint 15 request", () => {
  /** A request as its method, path, query, `Authorization` header and JSON body. */
  async function shapeOf(request: Request) {
    const url = new URL(request.url);
    const body: unknown = await request.clone().json();
    return {
      method: request.method,
      path: url.pathname,
      query: Object.fromEntries(url.searchParams),
      authorization: request.headers.get("Authorization"),
      body,
    };
  }

  it("ENTER POSTs the two inputs with the bearer token and no query, and shows the balance", async () => {
    const requests = stubApi();
    renderScreen({ from: PATHS.COMEN01 });

    await showBalance();

    expect(await Promise.all(requests.map(shapeOf))).toEqual([
      {
        method: "POST",
        path: ENDPOINT,
        query: {},
        authorization: "Bearer test-token",
        body: { acct_id: ACCT, confirm: "" },
      },
    ]);
  });
});

/** The 401 of the `SignedOnUser` extractor for a missing, invalid or expired token. */
const signonRequired: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};

/**
 * Renders the application's route table at `/bill-payment`, so the sign-on guard that only the
 * table mounts stands in front of the screen. Each screen is imported by its route's loader, so a
 * wait for one to render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COBIL00] });
  render(
    <QueryClientProvider client={testQueryClient()}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

// A 401 clears the stored session (`api/client.ts`), and the sign-on guard in front of the route
// replaces the still-mounted screen with sign-on at once: after the ENTER that sent the refused
// request, the test presses no key and follows no link.
describe("COBIL00 in the route table: a 401 ends the session", () => {
  it.each([
    {
      title: "the balance of ENTER",
      answer: () => json(401, signonRequired),
      drive: async () => {
        typeInputs(ACCT, "");
        pressEnter();
      },
      sent: [{ acct_id: ACCT, confirm: "" }],
    },
    {
      title: "the payment of ENTER with Y",
      answer: (body: unknown) =>
        member(body, "confirm") === "Y" ? json(401, signonRequired) : billPayment(body),
      drive: async () => {
        await showBalance();
        typeInputs(ACCT, "Y");
        pressEnter();
      },
      sent: [
        { acct_id: ACCT, confirm: "" },
        { acct_id: ACCT, confirm: "Y" },
      ],
    },
  ])(
    "$title answered 401 replaces the screen with sign-on and clears the session, with no retry",
    async ({ answer, drive, sent }) => {
      const requests = stubApi(undefined, answer);
      const router = renderRouteTable();
      expect(
        await screen.findByRole("heading", { level: 1, name: "Bill Payment" }, { timeout: 5_000 }),
      ).toBeDefined();

      await drive();

      expect(
        await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
      ).toBeDefined();
      expect(router.state.location.pathname).toBe(PATHS.COSGN00);
      expect(screen.queryByRole("heading", { level: 1, name: "Bill Payment" })).toBeNull();
      expect(getSession()).toBeNull();
      expect(paths(requests)).toEqual(sent.map(() => ENDPOINT));
      expect(requests.map((request) => request.headers.get("Authorization"))).toEqual(
        sent.map(() => "Bearer test-token"),
      );
      expect(await bodies(requests)).toEqual(sent);
      router.dispose();
    },
  );
});

/** The browser's network `TypeError` message, as Chrome's `fetch` words it with no connection. */
const NO_NETWORK = "Failed to fetch";

/** Endpoint 2, which the main menu loads on entry. */
const MENU_ENDPOINT = "/api/v1/menus/main";

/** The account the offline ENTER is typed with. */
const OFFLINE_ACCT = "00000000002";

/**
 * Replaces `fetch` with a network that `network.up` switches. While it is down, every request is
 * refused with the `TypeError` that `fetch` rejects with when it cannot connect; while it is up,
 * endpoint 15 is answered by {@link billPayment} and any other request gets a 500. Returns every
 * request `fetch` was handed, in order, whether it was answered or refused.
 */
function stubNetwork(network: { readonly up: boolean }): Request[] {
  const requests: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      requests.push(req);
      if (!network.up) {
        throw new TypeError(NO_NETWORK);
      }
      const { pathname } = new URL(req.url);
      if (req.method !== "POST" || pathname !== ENDPOINT) {
        return unexpected(`${req.method} ${pathname}`);
      }
      const body: unknown = await req.clone().json();
      return billPayment(body);
    }),
  );
  return requests;
}

/** The browser's `offline` or `online` event, which TanStack Query's `onlineManager` follows. */
function fireNetworkEvent(type: "offline" | "online"): void {
  act(() => {
    window.dispatchEvent(new Event(type));
  });
}

// The application's own query client (`App.tsx`), not a test client, decides whether a request
// made with no network is sent at once or held until the network returns. It is sent: `fetch`
// rejects with the browser's network `TypeError`, which the message row shows as any failure, and
// the keyboard is free again. Nothing is held, so the network's return after F3 sends nothing:
// only the operator's next key sends a request again (`api/client.ts`).
describe("COBIL00 in the application: ENTER with no network", () => {
  afterEach(() => {
    // The online state lives in TanStack Query's process-wide `onlineManager`: the next test
    // starts online.
    onlineManager.setOnline(true);
  });

  it("fails at once with the browser's message, frees the keys, and sends nothing when the network returns", async () => {
    const network = { up: true };
    const requests = stubNetwork(network);
    const startUrl = window.location.href;
    window.history.replaceState(null, "", PATHS.COBIL00);
    try {
      // `App` creates its router from `window.location` when its module is first evaluated. No
      // other test in this file imports it, so this import evaluates it here, at `/bill-payment`.
      const { App } = await import("@/App");
      const { container } = render(<App />);
      expect(
        await screen.findByRole("heading", { level: 1, name: "Bill Payment" }, { timeout: 5_000 }),
      ).toBeDefined();

      network.up = false;
      fireNetworkEvent("offline");
      expect(onlineManager.isOnline()).toBe(false);
      typeInputs(OFFLINE_ACCT, "");
      pressEnter();

      // The request is attempted at once and fails at once: the message row shows why, and ENTER,
      // F4 and the inputs are free, the typed account kept.
      await waitFor(() => {
        expect(messageRow().textContent).toBe(NO_NETWORK);
      });
      expect(paths(requests)).toEqual([ENDPOINT]);
      expect(await bodies(requests)).toStrictEqual([{ acct_id: OFFLINE_ACCT, confirm: "" }]);
      expect(container.querySelector("form")?.getAttribute("aria-busy")).toBe("false");
      expect(screen.getByRole("group", { name: "Function keys" }).getAttribute("aria-busy")).toBe(
        "false",
      );
      expect(screen.getByRole("button", { name: "ENTER=Continue" }).hasAttribute("disabled")).toBe(
        false,
      );
      expect(pfKeyButton("F4=Clear").hasAttribute("disabled")).toBe(false);
      expect([acctInput().readOnly, confirmInput().readOnly]).toEqual([false, false]);
      expect(shown()).toEqual({ acct: OFFLINE_ACCT, balance: "", confirm: "" });

      // F3 to the main menu, whose load on entry is attempted and fails at once in the same way.
      fireEvent.click(pfKeyButton("F3=Back"));
      expect(
        await screen.findByRole("heading", { level: 1, name: "Main Menu" }, { timeout: 5_000 }),
      ).toBeDefined();
      await waitFor(() => {
        expect(errorText()).toBe(NO_NETWORK);
      });
      expect(paths(requests)).toEqual([ENDPOINT, MENU_ENDPOINT]);

      // The network returns: the abandoned ENTER is not sent from the menu.
      network.up = true;
      fireNetworkEvent("online");
      expect(onlineManager.isOnline()).toBe(true);
      await settle();
      expect(paths(requests).filter((path) => path === ENDPOINT)).toHaveLength(1);
      expect(requests.filter((request) => request.method === "POST")).toHaveLength(1);
    } finally {
      window.history.replaceState(null, "", startUrl);
    }
  });
});
