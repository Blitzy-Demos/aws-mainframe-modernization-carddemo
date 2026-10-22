import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { configure, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
import { getSession, setSession } from "@/auth/session";
import { PATHS, routes } from "@/routes";
import { COPAU01 } from "./COPAU01";

/*
 * Render test of the authorization detail (map COPAU1A, transaction CPVD, program COPAUS1C,
 * route `/authorizations/detail`) in jsdom, with `fetch` stubbed for endpoint 22,
 * `GET /api/v1/authorizations/{acct_id}/{auth_key}`, and endpoint 23,
 * `POST /api/v1/authorizations/{acct_id}/{auth_key}/fraud`. Expected values are the audited
 * results of `modern/audit/screens.md` (`COPAU01-nnn`) and `modern/audit/online/COPAUS1C.md`
 * (`COPAUS1C-nnn`). Every card number below is redacted, and no password appears.
 */

/*
 * A flow waits on up to five stubbed round-trips in jsdom. Alone a test takes under 2 s, but under
 * coverage on a host shared with other suites one took 2.8 s, its waits close to Testing
 * Library's 1 s default. The file raises its wait limit to 10 s (the `dom` project of
 * `vite.config.ts` allows 30 s per test); a passing test runs no longer for it.
 */
configure({ asyncUtilTimeout: 10_000 });

/** The account a COPAU00 `S` selection hands over, `CDEMO-ACCT-ID PIC 9(11)`. */
const ACCT_ID = "00000000050";

/**
 * A rendered authorization key: a sign digit and the 5 digits of the inverted date, then a sign
 * digit and the 9 digits of the inverted time. Built from its two parts, so that no line of this
 * file holds a 16-digit run.
 */
function authKey(datePart: string, timePart: string): string {
  return `${datePart}${timePart}`;
}

/** The authorization the COPAU00 selection names. */
const SELECTED_KEY = authKey("075968", "0869490879");
/** The account's next authorization, which F8 reaches from {@link SELECTED_KEY}. */
const NEXT_KEY = authKey("075968", "0869491203");
/** An authorization that reads, but whose F5 and F8 fail on the server. */
const FAILING_CALLS_KEY = authKey("075968", "0869495000");
/** A key no authorization of the account has. */
const UNKNOWN_KEY = authKey("075968", "0000000404");
/** A key whose read fails on the server. */
const FAILING_READ_KEY = authKey("075968", "0000000500");

/** Endpoint 22's path for the account, up to the key segment. */
const ENDPOINT = `/api/v1/authorizations/${ACCT_ID}/`;

/** The route state of a COPAU00 `S` selection of `key`, as COPAU00 sends it (COPAUS0C-010). */
function selection(key: string) {
  return { from: PATHS.COPAU00, acct_id: ACCT_ID, auth_key: key };
}

/** The route state of a COPAU00 `S` selection of {@link SELECTED_KEY}. */
const FROM_LIST = selection(SELECTED_KEY);

/**
 * COPAUS1C-012 to COPAUS1C-023: an approved authorization never reported as fraud, every member
 * at its map field's width as the program builds it.
 */
const selectedDetail: components["schemas"]["AuthDetailResponse"] = {
  acct_id: ACCT_ID,
  auth_key: SELECTED_KEY,
  card_num: "************5740",
  auth_date: "01/31/24  ",
  auth_time: "10:30:45  ",
  response: "A",
  reason: "0000-APPROVED".padEnd(20, " "),
  auth_code: "000000",
  amount: "     1234.56",
  pos_entry_mode: "05  ",
  source: "WEBAPI    ",
  merchant_category_code: "5411",
  card_expiry: "12/27",
  auth_type: "PURC".padEnd(14, " "),
  transaction_id: "TXN********0001",
  match_status: "P",
  fraud_status: "-".padEnd(10, " "),
  merchant_name: "Abshire-Lowe".padEnd(25, " "),
  merchant_id: "ABSHIRELOWE-042",
  merchant_city: "Lake Rubye".padEnd(25, " "),
  merchant_state: "TX",
  merchant_zip: "72112".padEnd(10, " "),
  message: null,
};

/** The account's next authorization, a decline (COPAUS1C-024). */
const nextDetail: components["schemas"]["AuthDetailResponse"] = {
  acct_id: ACCT_ID,
  auth_key: NEXT_KEY,
  card_num: "************0005",
  auth_date: "12/31/23  ",
  auth_time: "23:59:59  ",
  response: "D",
  reason: "4100-INSUFFICNT FUND",
  auth_code: "010000",
  amount: "        0.00",
  pos_entry_mode: "01  ",
  source: "POSTRM    ",
  merchant_category_code: "5999",
  card_expiry: "06/26",
  auth_type: "RFND".padEnd(14, " "),
  transaction_id: "TXN********0002",
  match_status: "D",
  fraud_status: "-".padEnd(10, " "),
  merchant_name: "Kris-Wuckert".padEnd(25, " "),
  merchant_id: "KRISWUCKERT-007",
  merchant_city: "Port Daxton".padEnd(25, " "),
  merchant_state: "NY",
  merchant_zip: "10001".padEnd(10, " "),
  message: null,
};

/** COPAUS1C-025: F8 on the last authorization returns it again, its key unchanged, and says so. */
const atLastDetail: components["schemas"]["AuthDetailResponse"] = {
  ...nextDetail,
  message: "Already at the last Authorization...",
};

/** The authorization of {@link FAILING_CALLS_KEY}, shown as {@link selectedDetail} is. */
const failingCallsDetail: components["schemas"]["AuthDetailResponse"] = {
  ...selectedDetail,
  auth_key: FAILING_CALLS_KEY,
};

/** COPAUS1C-032: the first F5 reports fraud and returns the detail as replaced. */
const markedFraud: components["schemas"]["FraudToggleResult"] = {
  message: "AUTH MARKED FRAUD...",
  detail: { ...selectedDetail, fraud_status: "F-02/01/24" },
};

/** COPAUS1C-028, COPAUS1C-032: the next F5 on the same authorization removes the report. */
const removedFraud: components["schemas"]["FraudToggleResult"] = {
  message: "AUTH FRAUD REMOVED...",
  detail: { ...selectedDetail, fraud_status: "R-02/01/24" },
};

/** A committed F5 whose result carries the audited message and no detail. */
const markedNoDetail: components["schemas"]["FraudToggleResult"] = {
  message: "AUTH MARKED FRAUD...",
};

/** COPAUS1C-009: no detail of the account has the key; the 404 carries a blank message. */
const notFound: components["schemas"]["ScreenError"] = { message: "" };

/**
 * COPAUS1C-011: the detail read fails. The two status characters after the literal are a
 * stand-in: the IMS status is halt H4 (COPAUS1C-038), so only the screen's showing the message
 * as sent is asserted.
 */
const readFailed: components["schemas"]["ScreenError"] = {
  message: " System error while reading Auth Details: Code:XX",
};

/** COPAUS1C-026: the next read fails; the status characters are a stand-in, as above. */
const nextReadFailed: components["schemas"]["ScreenError"] = {
  message: " System error while reading next Auth: Code:XX",
};

/**
 * COPAUS1C-031 with COPAUS2C-010: the `auth_fraud` write fails and everything is rolled back.
 * The SQLCODE and SQLSTATE are stand-ins: their values are halt H3.
 */
const fraudFailed: components["schemas"]["ScreenError"] = {
  message: " SYSTEM ERROR DB2: CODE:-000000, STATE: +000000000",
};

/** The token class of the map's `COLOR=PINK`, which has no token of its own (AAP 0.4.3). */
const PINK = "text-primary";
/** The token class of `COLOR=BLUE` (AAP 0.4.3). */
const BLUE = "text-muted-foreground";
/** The token class of `COLOR=RED` (AAP 0.4.3). */
const RED = "text-destructive";

/**
 * One output field of the map with the label beside it: the label's `INITIAL`, `POS` column and
 * the columns it spans; the field's BMS name, `POS` line and column and `LENGTH`; and the token
 * class of the field's `COLOR`.
 */
function spec(
  label: string,
  labelCol: number,
  labelSpan: number,
  field: string,
  row: number,
  col: number,
  length: number,
  tone: string,
) {
  return { label, labelCol, labelSpan, field, row, col, length, tone };
}

/**
 * The 20 output fields of the map in map order (`COPAU01-012` to `COPAU01-052`). The label span
 * is the label's `LENGTH`, except for `Tran Id:`, whose `LENGTH=18` runs over `TRNID` at column
 * 12: the later field keeps its place and the label ends before it (`COPAU01-036`, X-SCREENS-3).
 */
const FIELDS = [
  spec("Card #:", 2, 7, "CARDNUM", 7, 11, 16, PINK),
  spec("Auth Date:", 31, 10, "AUTHDT", 7, 43, 10, PINK),
  spec("Auth Time:", 56, 10, "AUTHTM", 7, 68, 10, PINK),
  spec("Auth Resp:", 2, 10, "AUTHRSP", 9, 14, 1, PINK),
  spec("Resp Reason:", 18, 12, "AUTHRSN", 9, 32, 20, BLUE),
  spec("Auth Code:", 56, 10, "AUTHCD", 9, 68, 6, BLUE),
  spec("Amount:", 2, 7, "AUTHAMT", 11, 11, 12, BLUE),
  spec("POS Entry Mode:", 29, 15, "POSEMD", 11, 46, 4, BLUE),
  spec("Source   :", 56, 10, "AUTHSRC", 11, 68, 10, BLUE),
  spec("MCC Code:", 2, 9, "MCCCD", 13, 13, 4, BLUE),
  spec("Card Exp. Date:", 25, 15, "CRDEXP", 13, 42, 5, BLUE),
  spec("Auth Type:", 52, 10, "AUTHTYP", 13, 64, 14, BLUE),
  spec("Tran Id:", 2, 10, "TRNID", 15, 12, 15, BLUE),
  spec("Match Status:", 31, 13, "AUTHMTC", 15, 46, 1, RED),
  spec("Fraud Status:", 52, 13, "AUTHFRD", 15, 67, 10, RED),
  spec("Name:", 2, 5, "MERNAME", 19, 9, 25, BLUE),
  spec("Merchant ID:", 41, 12, "MERID", 19, 55, 15, BLUE),
  spec("City:", 2, 5, "MERCITY", 21, 9, 25, BLUE),
  spec("State:", 41, 6, "MERST", 21, 49, 2, BLUE),
  spec("Zip:", 55, 4, "MERZIP", 21, 61, 10, BLUE),
];

/** Every output field blank: before the first read, while it runs and after a failed read. */
const BLANK = Object.fromEntries(FIELDS.map(({ field }) => [field, ""]));

/** The fields showing {@link selectedDetail}: each value exactly as endpoint 22 sends it. */
const SHOWN_SELECTED = {
  CARDNUM: "************5740",
  AUTHDT: "01/31/24  ",
  AUTHTM: "10:30:45  ",
  AUTHRSP: "A",
  AUTHRSN: "0000-APPROVED       ",
  AUTHCD: "000000",
  AUTHAMT: "     1234.56",
  POSEMD: "05  ",
  AUTHSRC: "WEBAPI    ",
  MCCCD: "5411",
  CRDEXP: "12/27",
  AUTHTYP: "PURC          ",
  TRNID: "TXN********0001",
  AUTHMTC: "P",
  AUTHFRD: "-         ",
  MERNAME: "Abshire-Lowe             ",
  MERID: "ABSHIRELOWE-042",
  MERCITY: "Lake Rubye               ",
  MERST: "TX",
  MERZIP: "72112     ",
};

/** The fields showing {@link nextDetail}. */
const SHOWN_NEXT = {
  CARDNUM: "************0005",
  AUTHDT: "12/31/23  ",
  AUTHTM: "23:59:59  ",
  AUTHRSP: "D",
  AUTHRSN: "4100-INSUFFICNT FUND",
  AUTHCD: "010000",
  AUTHAMT: "        0.00",
  POSEMD: "01  ",
  AUTHSRC: "POSTRM    ",
  MCCCD: "5999",
  CRDEXP: "06/26",
  AUTHTYP: "RFND          ",
  TRNID: "TXN********0002",
  AUTHMTC: "D",
  AUTHFRD: "-         ",
  MERNAME: "Kris-Wuckert             ",
  MERID: "KRISWUCKERT-007",
  MERCITY: "Port Daxton              ",
  MERST: "NY",
  MERZIP: "10001     ",
};

/** `CCDA-MSG-INVALID-KEY PIC X(50)`, padded to its 50 bytes as COPAUS1C moves it (COPAUS1C-005). */
const INVALID_KEY_TEXT = "Invalid key pressed. Please see below...".padEnd(50, " ");

/** The F-keys COPAUS1C serves under no branch but `WHEN OTHER`: all but F3, F5 and F8. */
const UNSERVED_F_KEYS = ["F1", "F2", "F4", "F6", "F7", "F9", "F10", "F11", "F12"];

/** A JSON response, as the API sends every success and every `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces `fetch` with endpoints 22 and 23 for {@link ACCT_ID}:
 * - endpoint 22 answers each key with its detail, `direction=forward` with the next one
 *   ({@link NEXT_KEY} is the last, so its F8 answers {@link atLastDetail}), 404 for
 *   {@link UNKNOWN_KEY}, 500 for {@link FAILING_READ_KEY} and for F8 from
 *   {@link FAILING_CALLS_KEY};
 * - endpoint 23 alternates {@link markedFraud} and {@link removedFraud} for
 *   {@link SELECTED_KEY}, answers {@link markedNoDetail} for {@link NEXT_KEY} and 500 for
 *   {@link FAILING_CALLS_KEY}.
 *
 * Any other request gets a 500. Every reply waits for `hold`, when given. Returns every request
 * in the order the screen sent it.
 */
function stubApi(hold?: Promise<void>): Request[] {
  const requests: Request[] = [];
  let toggles = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      requests.push(req);
      await hold;
      const url = new URL(req.url);
      const segment = url.pathname.startsWith(ENDPOINT)
        ? decodeURIComponent(url.pathname.slice(ENDPOINT.length))
        : "";
      const direction = url.searchParams.get("direction");
      if (req.method === "GET" && direction === null) {
        if (segment === SELECTED_KEY) {
          return json(200, selectedDetail);
        }
        if (segment === NEXT_KEY) {
          return json(200, nextDetail);
        }
        if (segment === FAILING_CALLS_KEY) {
          return json(200, failingCallsDetail);
        }
        if (segment === UNKNOWN_KEY) {
          return json(404, notFound);
        }
        if (segment === FAILING_READ_KEY) {
          return json(500, readFailed);
        }
      }
      if (req.method === "GET" && direction === "forward") {
        if (segment === SELECTED_KEY) {
          return json(200, nextDetail);
        }
        if (segment === NEXT_KEY) {
          return json(200, atLastDetail);
        }
        if (segment === FAILING_CALLS_KEY) {
          return json(500, nextReadFailed);
        }
      }
      if (req.method === "POST") {
        if (segment === `${SELECTED_KEY}/fraud`) {
          toggles += 1;
          return json(200, toggles % 2 === 1 ? markedFraud : removedFraud);
        }
        if (segment === `${NEXT_KEY}/fraud`) {
          return json(200, markedNoDetail);
        }
        if (segment === `${FAILING_CALLS_KEY}/fraud`) {
          return json(500, fraudFailed);
        }
      }
      const unexpected: components["schemas"]["ScreenError"] = {
        message: `Unexpected request ${req.method} ${url.pathname}${url.search}`,
      };
      return json(500, unexpected);
    }),
  );
  return requests;
}

/** Each request as `<method> <path><query>`, in order. */
function calls(requests: readonly Request[]): string[] {
  return requests.map((request) => {
    const url = new URL(request.url);
    return `${request.method} ${url.pathname}${url.search}`;
  });
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

/**
 * COPAU01 at `/authorizations/detail` with `state`, and a stub at COPAU00, its one target.
 * `reactStrictMode` runs every mount effect twice, as the development build does.
 */
function renderScreen(state?: unknown, reactStrictMode = false) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COPAU01, state }]}>
        <Routes>
          <Route path={PATHS.COPAU01} element={<COPAU01 />} />
          <Route path={PATHS.COPAU00} element={<RouteStub />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
    { reactStrictMode },
  );
}

/** The route stub a transfer reached: its path and the route state it was given. */
async function reachedStub(): Promise<{ path: string; state: unknown }> {
  const stub = await screen.findByRole("region", { name: "Route stub" });
  const state: unknown = JSON.parse(within(stub).getByTestId("stub-state").textContent ?? "null");
  return { path: within(stub).getByTestId("stub-path").textContent ?? "", state };
}

/**
 * The `<input>` the label `text` names, matched with its spacing kept (`Source   :`), failing
 * the test when it names anything else.
 */
function field(text: string): HTMLInputElement {
  const element = screen.getByLabelText(text, { normalizer: (label) => label });
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`"${text}" labels <${element.tagName.toLowerCase()}>, not an input`);
  }
  return element;
}

/** The value each output field shows, by BMS field name. */
function shown(): Record<string, string> {
  return Object.fromEntries(FIELDS.map(({ label, field: name }) => [name, field(label).value]));
}

/** The message rows of the screen of one kind (`MessageLine`'s `data-message-kind`). */
function messageRows(kind: "error" | "info"): HTMLElement[] {
  return screen.getAllByRole("alert").filter((row) => row.dataset.messageKind === kind);
}

/** The text of the `ERRMSG` row (`COPAU01-053`), the screen's one message row. */
function errorText(): string {
  const rows = messageRows("error");
  expect(rows).toHaveLength(1);
  return rows[0]?.textContent ?? "";
}

/** The text of the header field `name` (`data-field` of `ScreenShell`). */
function headerField(container: HTMLElement, name: string): string | null {
  return container.querySelector(`[data-field="${name}"]`)?.textContent ?? null;
}

/**
 * The label literals of header rows 1 and 2 (`COPAU01-001`, `COPAU01-004`, `COPAU01-006`,
 * `COPAU01-009`): each `INITIAL` with the grid cell of its `POS` and `LENGTH`, and the value
 * field it labels with that field's cell.
 *
 * The six value rows of those lines (`COPAU01-002`, `-003`, `-005`, `-007`, `-008`, `-010`)
 * describe read-only `Input` elements, which `ScreenShell` renders as read-only text; their
 * placement and read-only state are proved by {@link expectHeaderValues}, and their content rows,
 * `COPAU01-055` to `COPAU01-060`, by the values shown.
 */
const HEADER_LABELS = [
  {
    label: "Tran:",
    labelCell: "row-start-1 col-start-1 col-span-5",
    name: "TRNNAME",
    fieldCell: "row-start-1 col-start-7 col-span-4",
  },
  {
    label: "Date:",
    labelCell: "row-start-1 col-start-65 col-span-5",
    name: "CURDATE",
    fieldCell: "row-start-1 col-start-71 col-span-8",
  },
  {
    label: "Prog:",
    labelCell: "row-start-2 col-start-1 col-span-5",
    name: "PGMNAME",
    fieldCell: "row-start-2 col-start-7 col-span-8",
  },
  {
    label: "Time:",
    labelCell: "row-start-2 col-start-65 col-span-5",
    name: "CURTIME",
    fieldCell: "row-start-2 col-start-71 col-span-8",
  },
];

/** The screen's `CardHeader`, which holds header rows 1 and 2 and the screen title. */
function cardHeader(container: HTMLElement): HTMLElement {
  const header = container.querySelector('[data-slot="card-header"]');
  if (!(header instanceof HTMLElement)) {
    throw new Error("the screen renders no CardHeader");
  }
  return header;
}

/**
 * Asserts the header value fields of rows 1 and 2 against their rows of `modern/audit/screens.md`
 * (COPAU01-002, -003, -005, -007, -008 and -010). Each entry of `fields` is the BMS name, the `POS`
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
  container: HTMLElement,
  fields: readonly {
    readonly name: string;
    readonly row: number;
    readonly col: number;
    readonly length: number;
    readonly value: string;
    readonly title: boolean;
  }[],
): void {
  const header = cardHeader(container);
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
    const found = container.querySelectorAll(`[data-field="${name}"]`);
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

/** The ENTER button the screen adds at the start of the key row (`COPAU01-064`). */
function enterButton(): HTMLElement {
  return screen.getByRole("button", { name: "ENTER" });
}

/** A keydown of `key` at the focused element, as the browser sends it; it bubbles to `window`. */
function pressKey(key: string, shiftKey = false): void {
  fireEvent.keyDown(document.activeElement ?? document.body, { key, shiftKey });
}

/**
 * Moves the focus to the key-row button `label`, as a click on it or a Tab to it does in the
 * browser, and checks that it left Card #.
 */
function focusKey(label: string): HTMLElement {
  const button = screen.getByRole("button", { name: label });
  button.focus();
  expect(document.activeElement).toBe(button);
  return button;
}

/** Clicks the key-row button `label` as the browser does: jsdom's click alone does not focus it. */
function clickKey(label: string): void {
  fireEvent.click(focusKey(label));
}

/**
 * Lets pending effects, mutations and stubbed responses run, so that a request the screen would
 * send has been sent before a test counts the requests.
 */
async function settle(): Promise<void> {
  await new Promise((resolve) => {
    setTimeout(resolve, 20);
  });
}

/** Everything the screen renders as text: the document's text and every input's value. */
function renderedText(): string {
  const values = Array.from(document.querySelectorAll("input"), (input) => input.value);
  return [document.body.textContent ?? "", ...values].join("\n");
}

/** Renders the screen from {@link FROM_LIST} and waits for its entry read to be shown. */
async function renderSelected(): Promise<Request[]> {
  const requests = stubApi();
  renderScreen(FROM_LIST);
  await waitFor(() => {
    expect(shown()).toEqual(SHOWN_SELECTED);
  });
  return requests;
}

beforeEach(() => {
  setSession("test-token", "USER0001", "U");
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COPAU01 authorization detail screen", () => {
  // audit: COPAU01-011
  // audit: COPAU01-012
  // audit: COPAU01-013
  // audit: COPAU01-014
  // audit: COPAU01-015
  // audit: COPAU01-016
  // audit: COPAU01-017
  // audit: COPAU01-018
  // audit: COPAU01-019
  // audit: COPAU01-020
  // audit: COPAU01-021
  // audit: COPAU01-022
  // audit: COPAU01-023
  // audit: COPAU01-024
  // audit: COPAU01-025
  // audit: COPAU01-026
  // audit: COPAU01-027
  // audit: COPAU01-028
  // audit: COPAU01-029
  // audit: COPAU01-030
  // audit: COPAU01-031
  // audit: COPAU01-032
  // audit: COPAU01-033
  // audit: COPAU01-034
  // audit: COPAU01-035
  // audit: COPAU01-036
  // audit: COPAU01-037
  // audit: COPAU01-038
  // audit: COPAU01-039
  // audit: COPAU01-040
  // audit: COPAU01-041
  // audit: COPAU01-042
  // audit: COPAU01-043
  // audit: COPAU01-044
  // audit: COPAU01-045
  // audit: COPAU01-046
  // audit: COPAU01-047
  // audit: COPAU01-048
  // audit: COPAU01-049
  // audit: COPAU01-050
  // audit: COPAU01-051
  // audit: COPAU01-052
  // audit: COPAU01-054
  // audit: COPAU01-064
  it("renders the map: title, 20 read-only fields on the grid, caption, key row", async () => {
    stubApi();
    renderScreen(FROM_LIST);

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "View Authorization Details",
    );

    // The map has no input field: every field is an output, so the screen has no form.
    expect(document.querySelector("form")).toBeNull();
    for (const spec of FIELDS) {
      const box = field(spec.label);
      expect(box.dataset.field).toBe(spec.field);
      expect(box.readOnly).toBe(true);
      expect(box.size).toBe(spec.length);
      expect(box.className.split(" ")).toContain(spec.tone);
      expect(box.parentElement?.className).toBe(
        `row-start-${spec.row} col-start-${spec.col} col-span-${spec.length}`,
      );
      const label = box.labels?.[0];
      expect(label?.textContent).toBe(spec.label);
      // Every label of the map is TURQUOISE, which maps to `--foreground`.
      expect(label?.className.split(" ")).toContain("text-foreground");
      expect(label?.parentElement?.className).toBe(
        `row-start-${spec.row} col-start-${spec.labelCol} col-span-${spec.labelSpan}`,
      );
    }
    expect(document.querySelectorAll("input")).toHaveLength(FIELDS.length);

    const caption = screen.getByText(`Merchant Details ${"-".repeat(59)}`);
    expect(caption.textContent).toHaveLength(76);
    expect(caption.parentElement?.className).toBe("row-start-17 col-start-2 col-span-76");

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
      ["ENTER", "submit", "default", "sm"],
      ["F3=Back", "button", "outline", "sm"],
      ["F5=Mark/Remove Fraud", "button", "outline", "sm"],
      ["F8=Next Auth", "button", "outline", "sm"],
    ]);

    // The map declares no INFOMSG row, only ERRMSG.
    expect(messageRows("info")).toHaveLength(0);
    expect(messageRows("error")).toHaveLength(1);

    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_SELECTED);
    });
  });

  // audit: COPAUS1C-002
  // audit: COPAU01-013
  // audit: COPAU01-015
  // audit: COPAU01-017
  // audit: COPAU01-019
  // audit: COPAU01-021
  // audit: COPAU01-023
  // audit: COPAU01-025
  // audit: COPAU01-027
  // audit: COPAU01-029
  // audit: COPAU01-031
  // audit: COPAU01-033
  // audit: COPAU01-035
  // audit: COPAU01-037
  // audit: COPAU01-039
  // audit: COPAU01-041
  // audit: COPAU01-044
  // audit: COPAU01-046
  // audit: COPAU01-048
  // audit: COPAU01-050
  // audit: COPAU01-052
  it("reads the selected authorization on entry and shows every value as sent", async () => {
    const requests = stubApi();
    renderScreen(FROM_LIST);

    // Blank until the read answers.
    expect(shown()).toEqual(BLANK);
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_SELECTED);
    });

    // One read of the selected key with no `direction`, as ENTER sends it.
    expect(calls(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_KEY}`]);
    expect(requests[0]?.headers.get("Authorization")).toBe("Bearer test-token");

    // The screen formats nothing: Auth Date and Auth Time are the `MM/DD/YY` and `HH:MM:SS`
    // display strings COPAUS1C builds from `X(06)` text, two spaces included, shown as sent
    // rather than sliced or parsed into dates (COPAU01-015, COPAUS1C-013, COPAUS1C-014); the
    // amount keeps the edited picture's leading spaces (COPAUS1C-015).
    expect(field("Auth Date:").value).toMatch(/^\d{2}\/\d{2}\/\d{2} {2}$/);
    expect(field("Auth Time:").value).toMatch(/^\d{2}:\d{2}:\d{2} {2}$/);
    expect(field("Amount:").value).toHaveLength(12);

    // Card # shows the redacted form only, and no 16-digit run reaches the page: the key the
    // screen keeps is never rendered.
    expect(field("Card #:").value).toBe("************5740");
    expect(renderedText()).not.toMatch(/\d{16}/);

    // No message after a found read (COPAUS1C-035).
    expect(errorText()).toBe("");
  });

  // audit: COPAUS1C-002
  it("sends the entry read once when the mount effect runs twice", async () => {
    const requests = stubApi();
    renderScreen(FROM_LIST, true);

    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_SELECTED);
    });
    await settle();
    expect(calls(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_KEY}`]);
  });

  // audit: COPAUS1C-037
  // audit: COPAU01-001
  // audit: COPAU01-004
  // audit: COPAU01-006
  // audit: COPAU01-009
  // audit: COPAU01-055
  // audit: COPAU01-056
  // audit: COPAU01-057
  // audit: COPAU01-058
  // audit: COPAU01-059
  // audit: COPAU01-060
  it("fills header rows 1 and 2 at every send: CPVD, COPAUS1C, titles, date, time", async () => {
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
    stubApi();
    const { container } = renderScreen(FROM_LIST);

    // Each label stands in the CardHeader at its map POS and LENGTH, on the line of its field.
    const header = cardHeader(container);
    for (const { label, labelCell, name, fieldCell } of HEADER_LABELS) {
      const text = within(header).getByText(label, { normalizer: (shownText) => shownText });
      expect(text.parentElement?.className).toBe(labelCell);
      expect(header.querySelector(`[data-field="${name}"]`)?.parentElement?.className).toBe(
        fieldCell,
      );
    }

    expect(headerField(container, "TRNNAME")).toBe("CPVD");
    expect(headerField(container, "PGMNAME")).toBe("COPAUS1C");
    expect(headerField(container, "TITLE01")).toBe("      AWS Mainframe Modernization       ");
    expect(headerField(container, "TITLE02")).toBe("              CardDemo                  ");
    expect(headerField(container, "CURDATE")).toBe("01/02/24");
    expect(headerField(container, "CURTIME")).toBe("03:04:05");
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_SELECTED);
    });

    vi.setSystemTime(new Date(2025, 11, 31, 23, 59, 58));
    fireEvent.click(pfKeyButton("F8=Next Auth"));
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_NEXT);
    });
    expect(headerField(container, "CURDATE")).toBe("12/31/25");
    expect(headerField(container, "CURTIME")).toBe("23:59:58");
  });

  // audit: COPAU01-002
  // audit: COPAU01-003
  // audit: COPAU01-005
  // audit: COPAU01-007
  // audit: COPAU01-008
  // audit: COPAU01-010
  it("shows the six header values as read-only text in the CardHeader title rows, each at its POS and LENGTH", async () => {
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
    const requests = stubApi();
    const { container } = renderScreen(FROM_LIST);
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_SELECTED);
    });

    expectHeaderValues(container, [
      { name: "TRNNAME", row: 1, col: 7, length: 4, value: "CPVD", title: false },
      {
        name: "TITLE01",
        row: 1,
        col: 21,
        length: 40,
        value: "      AWS Mainframe Modernization       ",
        title: true,
      },
      { name: "CURDATE", row: 1, col: 71, length: 8, value: "01/02/24", title: false },
      { name: "PGMNAME", row: 2, col: 7, length: 8, value: "COPAUS1C", title: false },
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
    // Only the entry read was sent, and the stub answered it in process.
    expect(calls(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_KEY}`]);
  });

  // audit: COPAUS1C-003
  // audit: COPAU01-063
  // audit: COPAU01-064
  it("F8 shows the next authorization and keeps its key for ENTER, F8 and F5", async () => {
    const requests = await renderSelected();

    // The generated contract names F8's direction `forward` (`PageDirection`); the audit rows'
    // `next` is the same request.
    fireEvent.click(pfKeyButton("F8=Next Auth"));
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_NEXT);
    });
    expect(errorText()).toBe("");

    // ENTER reads the key F8 returned, with no `direction`.
    fireEvent.click(enterButton());
    await waitFor(() => {
      expect(requests).toHaveLength(3);
    });
    await settle();
    expect(shown()).toEqual(SHOWN_NEXT);

    // F8 at the account's last authorization keeps it on display and says so; the key stays.
    fireEvent.click(pfKeyButton("F8=Next Auth"));
    await waitFor(() => {
      expect(errorText()).toBe("Already at the last Authorization...");
    });
    expect(shown()).toEqual(SHOWN_NEXT);

    // F5 acts on the kept key. A result without a detail keeps the fields shown.
    fireEvent.click(pfKeyButton("F5=Mark/Remove Fraud"));
    await waitFor(() => {
      expect(errorText()).toBe("AUTH MARKED FRAUD...");
    });
    expect(shown()).toEqual(SHOWN_NEXT);

    expect(calls(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_KEY}`,
      `GET ${ENDPOINT}${SELECTED_KEY}?direction=forward`,
      `GET ${ENDPOINT}${NEXT_KEY}`,
      `GET ${ENDPOINT}${NEXT_KEY}?direction=forward`,
      `POST ${ENDPOINT}${NEXT_KEY}/fraud`,
    ]);
    // Endpoint 22 addresses one authorization: no request pages (Bounded Pagination).
    for (const request of requests) {
      expect(request.url).not.toContain("page_size");
    }
  });

  // audit: COPAUS1C-003
  // audit: COPAU01-062
  it("F5 marks, then removes, fraud on the kept key and shows it as replaced", async () => {
    const requests = await renderSelected();

    fireEvent.click(pfKeyButton("F5=Mark/Remove Fraud"));
    await waitFor(() => {
      expect(errorText()).toBe("AUTH MARKED FRAUD...");
    });
    expect(shown()).toEqual({ ...SHOWN_SELECTED, AUTHFRD: "F-02/01/24" });

    fireEvent.click(pfKeyButton("F5=Mark/Remove Fraud"));
    await waitFor(() => {
      expect(errorText()).toBe("AUTH FRAUD REMOVED...");
    });
    expect(shown()).toEqual({ ...SHOWN_SELECTED, AUTHFRD: "R-02/01/24" });

    // F5 leaves the key: ENTER reads the same authorization, and the read clears the message.
    fireEvent.click(enterButton());
    await waitFor(() => {
      expect(errorText()).toBe("");
    });

    expect(calls(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_KEY}`,
      `POST ${ENDPOINT}${SELECTED_KEY}/fraud`,
      `POST ${ENDPOINT}${SELECTED_KEY}/fraud`,
      `GET ${ENDPOINT}${SELECTED_KEY}`,
    ]);
    // Endpoint 23 sends no body: the server decides the flag from the stored detail.
    for (const request of requests) {
      expect(await request.clone().text()).toBe("");
    }
  });

  // audit: COPAUS1C-003
  // audit: COPAU01-053
  it("a failed F5 keeps the fields, a failed F8 blanks them, and the key stays", async () => {
    const requests = stubApi();
    renderScreen(selection(FAILING_CALLS_KEY));
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_SELECTED);
    });

    // Nothing was stored and the error carries no detail, so the fields already shown stay.
    fireEvent.click(pfKeyButton("F5=Mark/Remove Fraud"));
    await waitFor(() => {
      expect(errorText()).toBe(fraudFailed.message);
    });
    expect(shown()).toEqual(SHOWN_SELECTED);

    // A failed next read shows no fields and its message.
    fireEvent.click(pfKeyButton("F8=Next Auth"));
    await waitFor(() => {
      expect(errorText()).toBe(nextReadFailed.message);
    });
    expect(shown()).toEqual(BLANK);

    // The key the failure left is the one ENTER reads again.
    fireEvent.click(enterButton());
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_SELECTED);
    });
    expect(errorText()).toBe("");

    expect(calls(requests)).toEqual([
      `GET ${ENDPOINT}${FAILING_CALLS_KEY}`,
      `POST ${ENDPOINT}${FAILING_CALLS_KEY}/fraud`,
      `GET ${ENDPOINT}${FAILING_CALLS_KEY}?direction=forward`,
      `GET ${ENDPOINT}${FAILING_CALLS_KEY}`,
    ]);
  });

  // audit: COPAU01-053
  it.each([
    // The scenario list asks for an error row on a 404; COPAUS1C-009 audits that 404 with a blank
    // message, so the audited row asserted here is empty.
    { title: "a key no authorization has (404, blank message)", key: UNKNOWN_KEY, text: "" },
    { title: "a read that fails (500)", key: FAILING_READ_KEY, text: readFailed.message },
  ])("$title shows every field blank and the ScreenError message", async ({ key, text }) => {
    const requests = stubApi();
    renderScreen(selection(key));

    await waitFor(() => {
      expect(requests).toHaveLength(1);
    });
    await settle();
    expect(errorText()).toBe(text);
    expect(shown()).toEqual(BLANK);
    expect(calls(requests)).toEqual([`GET ${ENDPOINT}${key}`]);

    // ERRMSG is the destructive `Alert`, a polite live region, its text in `AlertDescription`.
    const row = messageRows("error")[0];
    expect(row?.dataset.slot).toBe("alert");
    expect(row?.getAttribute("aria-live")).toBe("polite");
    expect(row?.className.split(" ")).toContain("text-destructive");
    expect(row?.querySelector('[data-slot="alert-description"]')?.textContent).toBe(text);
  });

  // audit: COPAUS1C-004
  // audit: COPAU01-061
  it.each([
    { how: "the F3=Back button", press: () => fireEvent.click(pfKeyButton("F3=Back")) },
    { how: "the F3 key", press: () => pressKey("F3") },
  ])("$how returns to COPAU00 with the account and sends nothing", async ({ press }) => {
    const requests = await renderSelected();

    press();

    expect(await reachedStub()).toEqual({
      path: PATHS.COPAU00,
      state: { from: PATHS.COPAU01, acct_id: ACCT_ID },
    });
    await settle();
    expect(calls(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_KEY}`]);
  });

  // audit: COPAUS1C-001
  it.each([
    { title: "no route state", state: undefined },
    { title: "a null route state", state: null },
  ])("with $title goes to COPAU00 with the initialised account, reading nothing", async ({
    state,
  }) => {
    const requests = stubApi();
    renderScreen(state);

    expect(await reachedStub()).toEqual({
      path: PATHS.COPAU00,
      state: { from: PATHS.COPAU01, acct_id: "00000000000" },
    });
    await settle();
    expect(requests).toHaveLength(0);
  });

  it.each([
    { title: "no key", state: { from: PATHS.COPAU00, acct_id: ACCT_ID }, acct: ACCT_ID },
    {
      title: "an account that is not text",
      state: { from: PATHS.COPAU00, acct_id: 50, auth_key: SELECTED_KEY },
      acct: "",
    },
  ])("a route state with $title reads nothing and its keys send nothing", async ({
    state,
    acct,
  }) => {
    const requests = stubApi();
    renderScreen(state);
    await settle();

    // COPAUS1C reads nothing for such a state: every field blank, the message row empty.
    expect(shown()).toEqual(BLANK);
    expect(errorText()).toBe("");

    fireEvent.click(enterButton());
    fireEvent.click(pfKeyButton("F8=Next Auth"));
    fireEvent.click(pfKeyButton("F5=Mark/Remove Fraud"));
    await settle();
    expect(shown()).toEqual(BLANK);
    expect(errorText()).toBe("");

    // An unserved key still shows the invalid-key message.
    pressKey("F2");
    await waitFor(() => {
      expect(errorText()).toBe(INVALID_KEY_TEXT);
    });
    expect(requests).toHaveLength(0);

    fireEvent.click(pfKeyButton("F3=Back"));
    expect(await reachedStub()).toEqual({
      path: PATHS.COPAU00,
      state: { from: PATHS.COPAU01, acct_id: acct },
    });
    expect(requests).toHaveLength(0);
  });

  // audit: COPAUS1C-005
  it.each([
    ...UNSERVED_F_KEYS.map((key) => ({ title: key, key, shiftKey: false })),
    // The screen folds no shifted key: Shift+F3, Shift+F5 and Shift+F8 are PF15, PF17 and PF20.
    ...["F3", "F5", "F8"].map((key) => ({ title: `Shift+${key}`, key, shiftKey: true })),
  ])("$title reads as ENTER, then shows the invalid-key message", async ({ key, shiftKey }) => {
    const requests = await renderSelected();

    pressKey(key, shiftKey);
    await waitFor(() => {
      expect(errorText()).toBe(INVALID_KEY_TEXT);
    });
    expect(shown()).toEqual(SHOWN_SELECTED);
    expect(calls(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_KEY}`,
      `GET ${ENDPOINT}${SELECTED_KEY}`,
    ]);
  });

  // audit: COPAUS1C-005
  it("an unserved key after a failed read blanks the fields and shows its message", async () => {
    const requests = stubApi();
    renderScreen(selection(FAILING_READ_KEY));
    await waitFor(() => {
      expect(errorText()).toBe(readFailed.message);
    });

    pressKey("F7");
    await waitFor(() => {
      expect(errorText()).toBe(INVALID_KEY_TEXT);
    });
    expect(shown()).toEqual(BLANK);
    expect(calls(requests)).toEqual([
      `GET ${ENDPOINT}${FAILING_READ_KEY}`,
      `GET ${ENDPOINT}${FAILING_READ_KEY}`,
    ]);
  });

  // audit: COPAU01-062
  // audit: COPAU01-063
  // audit: COPAU01-064
  it.each([
    { key: "Enter", sent: `GET ${ENDPOINT}${SELECTED_KEY}`, text: "" },
    { key: "F5", sent: `POST ${ENDPOINT}${SELECTED_KEY}/fraud`, text: "AUTH MARKED FRAUD..." },
    { key: "F8", sent: `GET ${ENDPOINT}${SELECTED_KEY}?direction=forward`, text: "" },
  ])("the $key key acts as its button", async ({ key, sent, text }) => {
    const requests = await renderSelected();

    pressKey(key);
    await waitFor(() => {
      expect(requests).toHaveLength(2);
    });
    await settle();
    expect(calls(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_KEY}`, sent]);
    expect(errorText()).toBe(text);
  });

  it("ignores ENTER, F5, F8 and unserved keys while a call is in flight", async () => {
    let release: () => void = () => undefined;
    const hold = new Promise<void>((resolve) => {
      release = resolve;
    });
    const requests = stubApi(hold);
    renderScreen(FROM_LIST);
    await waitFor(() => {
      expect(requests).toHaveLength(1);
    });

    fireEvent.click(enterButton());
    fireEvent.click(pfKeyButton("F8=Next Auth"));
    fireEvent.click(pfKeyButton("F5=Mark/Remove Fraud"));
    pressKey("F2");
    await settle();
    expect(requests).toHaveLength(1);
    expect(shown()).toEqual(BLANK);

    release();
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_SELECTED);
    });
    expect(errorText()).toBe("");
    expect(calls(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_KEY}`]);
  });

  it("F3 leaves while a call is in flight", async () => {
    let release: () => void = () => undefined;
    const hold = new Promise<void>((resolve) => {
      release = resolve;
    });
    const requests = stubApi(hold);
    renderScreen(FROM_LIST);
    await waitFor(() => {
      expect(requests).toHaveLength(1);
    });

    fireEvent.click(pfKeyButton("F3=Back"));
    expect(await reachedStub()).toEqual({
      path: PATHS.COPAU00,
      state: { from: PATHS.COPAU01, acct_id: ACCT_ID },
    });
    release();
    await settle();
    expect(requests).toHaveLength(1);
  });

  // audit: COPAUS1C-036
  it("puts the cursor on Card # on entry and colours Auth Resp by its value", async () => {
    await renderSelected();

    expect(document.activeElement).toBe(field("Card #:"));
    // `A` takes DFHGREEN, `--foreground`, in place of the map's PINK; the other PINK fields keep
    // `--primary`.
    expect(field("Auth Resp:").className.split(" ")).toContain("text-foreground");
    expect(field("Auth Resp:").className.split(" ")).not.toContain("text-primary");
    for (const label of ["Card #:", "Auth Date:", "Auth Time:"]) {
      expect(field(label).className.split(" ")).toContain("text-primary");
    }

    // `D` takes DFHRED, `--destructive`.
    fireEvent.click(pfKeyButton("F8=Next Auth"));
    await waitFor(() => {
      expect(shown()).toEqual(SHOWN_NEXT);
    });
    expect(field("Auth Resp:").className.split(" ")).toContain("text-destructive");
    expect(field("Auth Resp:").className.split(" ")).not.toContain("text-primary");
  });

  // audit: COPAUS1C-036
  it.each([
    {
      title: "a found F8",
      state: FROM_LIST,
      entry: [`GET ${ENDPOINT}${SELECTED_KEY}`],
      press: () => clickKey("F8=Next Auth"),
      sent: [`GET ${ENDPOINT}${SELECTED_KEY}?direction=forward`],
      fields: SHOWN_NEXT,
      text: "",
    },
    {
      // The one send without ERASE (COPAUS1C-025): the screen keeps the authorization shown.
      title: "F8 at the last authorization",
      state: selection(NEXT_KEY),
      entry: [`GET ${ENDPOINT}${NEXT_KEY}`],
      press: () => clickKey("F8=Next Auth"),
      sent: [`GET ${ENDPOINT}${NEXT_KEY}?direction=forward`],
      fields: SHOWN_NEXT,
      text: "Already at the last Authorization...",
    },
    {
      title: "a failed F8 (500)",
      state: selection(FAILING_CALLS_KEY),
      entry: [`GET ${ENDPOINT}${FAILING_CALLS_KEY}`],
      press: () => clickKey("F8=Next Auth"),
      sent: [`GET ${ENDPOINT}${FAILING_CALLS_KEY}?direction=forward`],
      fields: BLANK,
      text: nextReadFailed.message,
    },
    {
      title: "a found ENTER",
      state: FROM_LIST,
      entry: [`GET ${ENDPOINT}${SELECTED_KEY}`],
      press: () => clickKey("ENTER"),
      sent: [`GET ${ENDPOINT}${SELECTED_KEY}`],
      fields: SHOWN_SELECTED,
      text: "",
    },
    {
      title: "a failed ENTER (500)",
      state: selection(FAILING_READ_KEY),
      entry: [`GET ${ENDPOINT}${FAILING_READ_KEY}`],
      press: () => clickKey("ENTER"),
      sent: [`GET ${ENDPOINT}${FAILING_READ_KEY}`],
      fields: BLANK,
      text: readFailed.message,
    },
    {
      title: "a committed F5",
      state: FROM_LIST,
      entry: [`GET ${ENDPOINT}${SELECTED_KEY}`],
      press: () => clickKey("F5=Mark/Remove Fraud"),
      sent: [`POST ${ENDPOINT}${SELECTED_KEY}/fraud`],
      fields: { ...SHOWN_SELECTED, AUTHFRD: "F-02/01/24" },
      text: "AUTH MARKED FRAUD...",
    },
    {
      title: "a failed F5 (500)",
      state: selection(FAILING_CALLS_KEY),
      entry: [`GET ${ENDPOINT}${FAILING_CALLS_KEY}`],
      press: () => clickKey("F5=Mark/Remove Fraud"),
      sent: [`POST ${ENDPOINT}${FAILING_CALLS_KEY}/fraud`],
      fields: SHOWN_SELECTED,
      text: fraudFailed.message,
    },
    {
      // The operator has tabbed to F8 and presses an unserved key there.
      title: "an unserved key",
      state: FROM_LIST,
      entry: [`GET ${ENDPOINT}${SELECTED_KEY}`],
      press: () => {
        focusKey("F8=Next Auth");
        pressKey("F2");
      },
      sent: [`GET ${ENDPOINT}${SELECTED_KEY}`],
      fields: SHOWN_SELECTED,
      text: INVALID_KEY_TEXT,
    },
    {
      // COPAUS1C-006: nothing is read, and the map is still sent.
      title: "ENTER on a state with no key",
      state: { from: PATHS.COPAU00, acct_id: ACCT_ID },
      entry: [],
      press: () => clickKey("ENTER"),
      sent: [],
      fields: BLANK,
      text: "",
    },
    {
      title: "F5 on a state with no key",
      state: { from: PATHS.COPAU00, acct_id: ACCT_ID },
      entry: [],
      press: () => clickKey("F5=Mark/Remove Fraud"),
      sent: [],
      fields: BLANK,
      text: "",
    },
  ])("puts the cursor back on Card # after $title", async ({
    state,
    entry,
    press,
    sent,
    fields,
    text,
  }) => {
    const requests = stubApi();
    renderScreen(state);
    await waitFor(() => {
      expect(calls(requests)).toEqual(entry);
    });
    await settle();

    // Every send of the map puts the cursor on Card #, wherever the pressed key had it.
    press();
    await waitFor(() => {
      expect(document.activeElement).toBe(field("Card #:"));
    });
    await settle();
    expect(calls(requests)).toEqual([...entry, ...sent]);
    expect(errorText()).toBe(text);
    expect(shown()).toEqual(fields);
    expect(document.activeElement).toBe(field("Card #:"));
  });
});

// Endpoints 22 and 23 name the authorization by the path alone, and every request carries the
// stored session's token.
describe("COPAU01: endpoint 22 and 23 requests", () => {
  /** A request as its method, path, query, `Authorization` header and body text. */
  async function shapeOf(request: Request) {
    const url = new URL(request.url);
    return {
      method: request.method,
      path: url.pathname,
      query: Object.fromEntries(url.searchParams),
      authorization: request.headers.get("Authorization"),
      body: await request.clone().text(),
    };
  }

  it("the read and F5's fraud toggle each send the authorization's path with the bearer token, no query and no body, and the toggle shows", async () => {
    const requests = await renderSelected();

    fireEvent.click(pfKeyButton("F5=Mark/Remove Fraud"));
    await waitFor(() => {
      expect(errorText()).toBe(markedFraud.message);
    });
    await settle();

    expect(shown()).toEqual({ ...SHOWN_SELECTED, AUTHFRD: "F-02/01/24" });
    expect(await Promise.all(requests.map(shapeOf))).toEqual([
      {
        method: "GET",
        path: `${ENDPOINT}${SELECTED_KEY}`,
        query: {},
        authorization: "Bearer test-token",
        body: "",
      },
      {
        method: "POST",
        path: `${ENDPOINT}${SELECTED_KEY}/fraud`,
        query: {},
        authorization: "Bearer test-token",
        body: "",
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
 * Renders the application's route table at `/authorizations/detail` with the route state of a
 * COPAU00 selection, so the sign-on guard that only the table mounts stands in front of the
 * screen. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, {
    initialEntries: [{ pathname: PATHS.COPAU01, state: FROM_LIST }],
  });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

/**
 * Replaces `fetch`: every request of `method` is answered 401, and an entry read of
 * {@link SELECTED_KEY} otherwise {@link selectedDetail}; any other request gets a 500. Returns
 * every request in the order the screen sent it.
 */
function stubRefusing(method: "GET" | "POST"): Request[] {
  const requests: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      requests.push(req);
      const { pathname } = new URL(req.url);
      if (req.method === method) {
        return json(401, signonRequired);
      }
      if (req.method === "GET" && pathname === `${ENDPOINT}${SELECTED_KEY}`) {
        return json(200, selectedDetail);
      }
      const unexpected: components["schemas"]["ScreenError"] = {
        message: `Unexpected request ${req.method} ${pathname}`,
      };
      return json(500, unexpected);
    }),
  );
  return requests;
}

// A 401 clears the stored session (`api/client.ts`), and the sign-on guard in front of the route
// replaces the still-mounted screen with sign-on at once: after the request the screen sent, the
// test presses no key and follows no link.
describe("COPAU01 in the route table: a 401 ends the session", () => {
  it.each([
    {
      title: "the detail read of the entry (endpoint 22)",
      refused: "GET" as const,
      drive: async (requests: readonly Request[]) => {
        await waitFor(() => {
          expect(requests).toHaveLength(1);
        });
      },
      expected: [`GET ${ENDPOINT}${SELECTED_KEY}`],
    },
    {
      title: "the fraud toggle of F5 (endpoint 23)",
      refused: "POST" as const,
      drive: async () => {
        await waitFor(() => {
          expect(shown()).toEqual(SHOWN_SELECTED);
        });
        clickKey("F5=Mark/Remove Fraud");
      },
      expected: [`GET ${ENDPOINT}${SELECTED_KEY}`, `POST ${ENDPOINT}${SELECTED_KEY}/fraud`],
    },
  ])(
    "$title answered 401 replaces the screen with sign-on and clears the session, with no retry",
    async ({ refused, drive, expected }) => {
      const requests = stubRefusing(refused);
      const router = renderRouteTable();

      await drive(requests);

      expect(await screen.findByRole("button", { name: "ENTER=Sign-on" })).toBeDefined();
      expect(router.state.location.pathname).toBe(PATHS.COSGN00);
      expect(
        screen.queryByRole("heading", { level: 1, name: "View Authorization Details" }),
      ).toBeNull();
      expect(getSession()).toBeNull();
      expect(calls(requests)).toEqual(expected);
      expect(requests.map((request) => request.headers.get("Authorization"))).toEqual(
        expected.map(() => "Bearer test-token"),
      );
      router.dispose();
    },
  );
});
