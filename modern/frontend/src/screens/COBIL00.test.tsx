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
import { clearSession, setSession } from "@/auth/session";
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
 * Of the header rows, the label rows `COBIL00-001`, `-004`, `-006` and `-009` are proved by NAV
 * (`frontend/e2e/nav.spec.ts`). The value rows `COBIL00-002`, `-003`, `-005`, `-007`, `-008` and
 * `-010` are tagged here, on the read-only title-row cells the shared `ScreenShell` renders them
 * in, and so are their content rows (`COBIL00-025` to `COBIL00-030`).
 */

const ENDPOINT = "/api/v1/bill-payments";

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

/** The text of the header field `name` (`data-field` of `ScreenShell`). */
function headerField(container: HTMLElement, name: string): string | null {
  return container.querySelector(`[data-field="${name}"]`)?.textContent ?? null;
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

    // Both screens load lazily, through the route tree, before the sign-on screen renders.
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

  // `ScreenShell` realises each header value field as read-only text in its title-row cell rather
  // than as an `Input` with `readOnly`, so these rows are proved on that cell. The values it shows
  // are proved under COBIL00-025 to COBIL00-030.
  // audit: COBIL00-002
  // audit: COBIL00-003
  // audit: COBIL00-005
  // audit: COBIL00-007
  // audit: COBIL00-008
  // audit: COBIL00-010
  it("header values TRNNAME, TITLE01, CURDATE, PGMNAME, TITLE02 and CURTIME are read-only cells of the CardHeader title rows at their map POS and LENGTH", () => {
    stubApi();
    const { container } = renderScreen({ from: PATHS.COMEN01 });

    const header = container.querySelector('[data-slot="card-header"]');
    expect(header).not.toBeNull();
    // The title rows hold no form control.
    expect(header?.querySelector("input, textarea, select, button")).toBeNull();
    for (const { name, row, col, length } of [
      { name: "TRNNAME", row: 1, col: 7, length: 4 },
      { name: "TITLE01", row: 1, col: 21, length: 40 },
      { name: "CURDATE", row: 1, col: 71, length: 8 },
      { name: "PGMNAME", row: 2, col: 7, length: 8 },
      { name: "TITLE02", row: 2, col: 21, length: 40 },
      { name: "CURTIME", row: 2, col: 71, length: 8 },
    ]) {
      // The field's one element stands in the title rows, not in the body or the footer.
      const shown = container.querySelectorAll(`[data-field="${name}"]`);
      expect(shown).toHaveLength(1);
      const value = shown.item(0);
      expect(header?.contains(value)).toBe(true);
      // Its cell is the map POS and LENGTH, and its text fills the field box, the padding that
      // centres the titles included.
      const cell = value?.parentElement;
      expect(cell?.className).toBe(`row-start-${row} col-start-${col} col-span-${length}`);
      expect(value?.textContent).toHaveLength(length);
      // Neither the value nor anything in its cell takes input or focus.
      expect(cell?.querySelector("input, textarea, select, button, [contenteditable]")).toBeNull();
      for (const element of [value, cell]) {
        expect(element?.hasAttribute("contenteditable")).toBe(false);
        expect(element?.hasAttribute("tabindex")).toBe(false);
      }
    }
  });

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
  it.each([
    { title: "the 404 of an unknown account", acct: UNKNOWN_ACCT, confirm: "", error: notFound },
    { title: "the 500 of a failed read", acct: FAILING_ACCT, confirm: "", error: lookupFailed },
    { title: "the 400 of a zero balance", acct: SETTLED_ACCT, confirm: "Y", error: nothingToPay },
    { title: "the 400 of confirmation X", acct: ACCT, confirm: "X", error: invalidConfirm },
    { title: "the 400 of a blank account", acct: "", confirm: "Y", error: emptyAcct },
  ])("shows $title in red, keeps every field, focuses the field it names", async (row) => {
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
    // The balance shown before the error stays, as do the inputs as typed.
    expect(shown()).toEqual({ acct: row.acct, balance: BALANCE, confirm: row.confirm });
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

  // A failure of the replacement alone, which no audit row covers: the dev proxy answers an empty
  // 502 while the API is down, so the message names the status, and no field takes the cursor.
  it("shows a failure with no ScreenError body by its status, keeps fields, cursor", async () => {
    stubApi();
    renderScreen({ from: PATHS.COMEN01 });
    await showBalance();
    const requests = stubApi(
      undefined,
      () => new Response(null, { status: 502, statusText: "Bad Gateway" }),
    );

    const enter = screen.getByRole("button", { name: "ENTER=Continue" });
    await moveFocus(enter);
    fireEvent.click(enter);

    await waitFor(() => {
      expect(errorText()).toBe("HTTP 502 Bad Gateway");
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

  // The terminal keyboard stays locked until the program's reply arrives; the screen holds ENTER
  // and F4 while its request is in flight, so a second ENTER cannot pay twice.
  it("ignores ENTER and F4 while its request is in flight, then shows the reply", async () => {
    const releases: (() => void)[] = [];
    const hold = new Promise<void>((resolve) => {
      releases.push(resolve);
    });
    const requests = stubApi(hold);
    renderScreen({ from: PATHS.COMEN01 });

    typeInputs(ACCT, "");
    pressEnter();
    await waitFor(() => {
      expect(requests).toHaveLength(1);
    });
    await settle();

    pressEnter();
    fireEvent.click(pfKeyButton("F4=Clear"));
    await settle();
    expect(requests).toHaveLength(1);
    expect(shown()).toEqual({ acct: ACCT, balance: "", confirm: "" });

    for (const release of releases) {
      release();
    }
    await waitFor(() => {
      expect(balanceOutput().value).toBe(BALANCE);
    });
    expect(errorText()).toBe("Confirm to make a bill payment...");
    await settle();
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
