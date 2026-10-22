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
import { COTRN01 } from "./COTRN01";

/*
 * Render test of the transaction view (map COTRN1A, transaction CT01, program COTRN01C, route
 * `/transactions/view`) in jsdom, with `fetch` stubbed for endpoint 12,
 * `GET /api/v1/transactions/{tran_id}`. Expected values are the audited results of
 * `modern/audit/screens.md` (`COTRN01-nnn`) and `modern/audit/online/COTRN01C.md`
 * (`COTRN01C-nnn`). Every card number below is redacted, and no password appears.
 */

/** The transaction a COTRN00 `S` selection hands over in route state. */
const SELECTED_ID = "0000000000000001";
/** A second transaction, typed into the Tran ID input and fetched by ENTER. */
const TYPED_ID = "0000000000000002";
/** A Tran ID endpoint 12 finds no transaction for. */
const UNKNOWN_ID = "9999999999999999";
/** A Tran ID whose read fails on the server. */
const FAILING_ID = "0000000000000500";

const ENDPOINT = "/api/v1/transactions/";

/** The paging state COTRN00 sends with a selection, under `list`, beside `from` and `tran_id`. */
const LIST = {
  first_key: SELECTED_ID,
  last_key: "0000000000000010",
  page_no: 1,
  has_next_page: true,
};

/** The route state of a COTRN00 `S` selection of {@link SELECTED_ID}, as COTRN00 sends it. */
const FROM_LIST = { from: PATHS.COTRN00, tran_id: SELECTED_ID, list: LIST };

const selectedView: components["schemas"]["TranViewResponse"] = {
  tran_id: SELECTED_ID,
  card_num: "************5740",
  type_cd: "01",
  cat_cd: "0001",
  source: "POS TERM  ",
  description: "Purchase at Abshire-Lowe".padEnd(60, " "),
  amount: "+00000050.47",
  orig_ts: "2022-06-10T14:22:31.123456",
  proc_ts: null,
  merchant_id: "800000000",
  merchant_name: "Abshire-Lowe".padEnd(30, " "),
  merchant_city: "North Enoshaven".padEnd(25, " "),
  merchant_zip: "72112     ",
  message: null,
};

const typedView: components["schemas"]["TranViewResponse"] = {
  tran_id: TYPED_ID,
  card_num: "************0005",
  type_cd: "02",
  cat_cd: "0005",
  source: "OPERATOR  ",
  description: "Return of goods".padEnd(60, " "),
  amount: "-00000012.00",
  orig_ts: "2022-12-31T23:59:59.999999",
  proc_ts: "2023-01-01T00:00:00",
  merchant_id: "000000042",
  merchant_name: "Kris-Wuckert".padEnd(30, " "),
  merchant_city: "Lake Rubye".padEnd(25, " "),
  merchant_zip: "10001     ",
  message: null,
};

/** COTRN01C-012: the read finds no record. */
const notFound: components["schemas"]["ScreenError"] = {
  message: "Transaction ID NOT found...",
  field: "tran_id",
};

/** COTRN01C-013: the read fails. */
const lookupFailed: components["schemas"]["ScreenError"] = {
  message: "Unable to lookup Transaction...",
  field: "tran_id",
};

/**
 * The 13 detail fields of the map, in map order: each label's `INITIAL`, the BMS field it labels,
 * the field's `LENGTH`, and the `POS` of both, which share a map row (`COTRN01-016` to
 * `COTRN01-053`). Each label's `LENGTH` is the length of its `INITIAL`.
 */
const OUTPUTS = [
  { label: "Transaction ID:", field: "TRNID", length: 16, row: 10, labelCol: 6, col: 22 },
  { label: "Card Number:", field: "CARDNUM", length: 16, row: 10, labelCol: 45, col: 58 },
  { label: "Type CD:", field: "TTYPCD", length: 2, row: 12, labelCol: 6, col: 15 },
  { label: "Category CD:", field: "TCATCD", length: 4, row: 12, labelCol: 23, col: 36 },
  { label: "Source:", field: "TRNSRC", length: 10, row: 12, labelCol: 46, col: 54 },
  { label: "Description:", field: "TDESC", length: 60, row: 14, labelCol: 6, col: 19 },
  { label: "Amount:", field: "TRNAMT", length: 12, row: 16, labelCol: 6, col: 14 },
  { label: "Orig Date:", field: "TORIGDT", length: 10, row: 16, labelCol: 31, col: 42 },
  { label: "Proc Date:", field: "TPROCDT", length: 10, row: 16, labelCol: 57, col: 68 },
  { label: "Merchant ID:", field: "MID", length: 9, row: 18, labelCol: 6, col: 19 },
  { label: "Merchant Name:", field: "MNAME", length: 30, row: 18, labelCol: 33, col: 48 },
  { label: "Merchant City:", field: "MCITY", length: 25, row: 20, labelCol: 6, col: 21 },
  { label: "Merchant Zip:", field: "MZIP", length: 10, row: 20, labelCol: 53, col: 67 },
];

/**
 * Every cell of the body grid, at its map `POS` and `LENGTH`: the Tran ID label and input
 * (`COTRN01-012`, `COTRN01-013`), the separator (`COTRN01-015`) and each detail label and field.
 * Sorted by row, then column, as {@link bodyCells} returns them.
 */
const BODY_CELLS = [
  { row: 6, col: 6, span: 14, content: "text Enter Tran ID:" },
  { row: 6, col: 21, span: 16, content: "field TRNIDIN" },
  { row: 8, col: 6, span: 70, content: `text ${"-".repeat(70)}` },
  ...OUTPUTS.flatMap((output) => [
    {
      row: output.row,
      col: output.labelCol,
      span: output.label.length,
      content: `text ${output.label}`,
    },
    { row: output.row, col: output.col, span: output.length, content: `field ${output.field}` },
  ]),
].sort((a, b) => a.row - b.row || a.col - b.col);

/**
 * The map's zero-length stoppers (`LENGTH=0`), each ending the field before it on its row, by
 * their `POS` (`COTRN01-014` to `COTRN01-054`).
 */
const STOPPERS = [
  { id: "COTRN01-014", row: 6, col: 38 },
  { id: "COTRN01-018", row: 10, col: 39 },
  { id: "COTRN01-021", row: 10, col: 75 },
  { id: "COTRN01-024", row: 12, col: 18 },
  { id: "COTRN01-027", row: 12, col: 41 },
  { id: "COTRN01-030", row: 12, col: 65 },
  { id: "COTRN01-033", row: 14, col: 80 },
  { id: "COTRN01-036", row: 16, col: 27 },
  { id: "COTRN01-039", row: 16, col: 53 },
  { id: "COTRN01-042", row: 16, col: 79 },
  { id: "COTRN01-045", row: 18, col: 29 },
  { id: "COTRN01-048", row: 18, col: 79 },
  { id: "COTRN01-051", row: 20, col: 47 },
  { id: "COTRN01-054", row: 20, col: 78 },
];

/** Every detail field blank: before a read, while it runs and after a failure. */
const BLANK_DETAILS = Object.fromEntries(OUTPUTS.map(({ field }) => [field, ""]));

/**
 * The detail fields showing {@link selectedView}: every value as the response carries it, and
 * the date part, the first 10 characters, of each timestamp; `proc_ts` is `null`, so blank.
 */
const SHOWN_SELECTED = {
  TRNID: SELECTED_ID,
  CARDNUM: "************5740",
  TTYPCD: "01",
  TCATCD: "0001",
  TRNSRC: "POS TERM  ",
  TDESC: "Purchase at Abshire-Lowe".padEnd(60, " "),
  TRNAMT: "+00000050.47",
  TORIGDT: "2022-06-10",
  TPROCDT: "",
  MID: "800000000",
  MNAME: "Abshire-Lowe".padEnd(30, " "),
  MCITY: "North Enoshaven".padEnd(25, " "),
  MZIP: "72112     ",
};

/**
 * The detail fields showing {@link typedView}. Its timestamps sit at the edges of a day and of a
 * year, so a date read through a time zone would show a neighbouring day.
 */
const SHOWN_TYPED = {
  TRNID: TYPED_ID,
  CARDNUM: "************0005",
  TTYPCD: "02",
  TCATCD: "0005",
  TRNSRC: "OPERATOR  ",
  TDESC: "Return of goods".padEnd(60, " "),
  TRNAMT: "-00000012.00",
  TORIGDT: "2022-12-31",
  TPROCDT: "2023-01-01",
  MID: "000000042",
  MNAME: "Kris-Wuckert".padEnd(30, " "),
  MCITY: "Lake Rubye".padEnd(25, " "),
  MZIP: "10001     ",
};

/** `CCDA-MSG-INVALID-KEY`, the message of an F-key the screen does not serve (COTRN01C-007). */
const INVALID_KEY_TEXT = "Invalid key pressed. Please see below...";

/** The twelve F-keys a browser sends. */
const ALL_F_KEYS = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];

/** The F-keys COTRN01C serves under no branch but `WHEN OTHER`: all but F3, F4 and F5. */
const UNSERVED_F_KEYS = ALL_F_KEYS.filter((key) => !["F3", "F4", "F5"].includes(key));

/** A JSON response, as the API sends every success and every `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces `fetch`. Endpoint 12 answers {@link selectedView} and {@link typedView} by their IDs,
 * 404 {@link notFound} for {@link UNKNOWN_ID} and 500 {@link lookupFailed} for
 * {@link FAILING_ID}; any other request gets a 500. Returns every request in the order the screen
 * sent it.
 */
function stubApi(): Request[] {
  const requests: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      requests.push(req);
      const { pathname } = new URL(req.url);
      const tranId =
        req.method === "GET" && pathname.startsWith(ENDPOINT)
          ? decodeURIComponent(pathname.slice(ENDPOINT.length))
          : undefined;
      if (tranId === SELECTED_ID) {
        return json(200, selectedView);
      }
      if (tranId === TYPED_ID) {
        return json(200, typedView);
      }
      if (tranId === UNKNOWN_ID) {
        return json(404, notFound);
      }
      if (tranId === FAILING_ID) {
        return json(500, lookupFailed);
      }
      const unexpected: components["schemas"]["ScreenError"] = {
        message: `Unexpected request ${req.method} ${pathname}`,
      };
      return json(500, unexpected);
    }),
  );
  return requests;
}

/** The path of each request, in order. */
function paths(requests: readonly Request[]): string[] {
  return requests.map((request) => new URL(request.url).pathname);
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

/** COTRN01 at `/transactions/view` with `state`, and stubs at the routes it transfers to. */
function renderScreen(state?: unknown) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COTRN01, state }]}>
        <Routes>
          <Route path={PATHS.COTRN01} element={<COTRN01 />} />
          <Route path={PATHS.COTRN00} element={<RouteStub />} />
          <Route path={PATHS.COMEN01} element={<RouteStub />} />
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
function field(text: string): HTMLInputElement {
  const element = screen.getByLabelText(text);
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`"${text}" labels <${element.tagName.toLowerCase()}>, not an input`);
  }
  return element;
}

/** `TRNIDIN`, the Tran ID input (`COTRN01-012`, `COTRN01-013`). */
function tranIdInput(): HTMLInputElement {
  return field("Enter Tran ID:");
}

/** The value each detail field shows, by BMS field name. */
function shownDetails(): Record<string, string> {
  return Object.fromEntries(OUTPUTS.map(({ label, field: name }) => [name, field(label).value]));
}

/** The message rows of the screen of one kind (`MessageLine`'s `data-message-kind`). */
function messageRows(kind: "error" | "info"): HTMLElement[] {
  return screen.getAllByRole("alert").filter((row) => row.dataset.messageKind === kind);
}

/** The text of the `ERRMSG` row (`COTRN01-055`), the screen's one error-kind message row. */
function errorText(): string {
  const rows = messageRows("error");
  expect(rows).toHaveLength(1);
  return rows[0]?.textContent ?? "";
}

/**
 * The error state of the Tran ID input: its `aria-invalid`, and each element its
 * `aria-describedby` resolves to, with its text and whether it is visually hidden (`sr-only`).
 * An id that resolves to no element is left out, as assistive technology ignores it.
 */
function tranIdErrorState() {
  const input = tranIdInput();
  const described = (input.getAttribute("aria-describedby") ?? "")
    .split(" ")
    .flatMap((id) => {
      const element = id === "" ? null : document.getElementById(id);
      return element === null
        ? []
        : [{ text: element.textContent, hidden: element.classList.contains("sr-only") }];
    });
  return { invalid: input.getAttribute("aria-invalid"), described };
}

/** The Tran ID input marked invalid, its field message holding `message`, visually hidden. */
function markedWith(message: string) {
  return { invalid: "true", described: [{ text: message, hidden: true }] };
}

/** The Tran ID input with no error: valid, and described by no message. */
const UNMARKED = { invalid: "false", described: [] };

/**
 * Checks that `message` is shown once, in the `ERRMSG` row: every other element holding it is
 * visually hidden, so the field's own message is read only by assistive technology.
 */
function expectShownOnce(message: string): void {
  const [row] = messageRows("error");
  const visible = screen
    .getAllByText(message)
    .filter((element) => element.closest(".sr-only") === null);
  expect(visible).toHaveLength(1);
  expect(row?.contains(visible[0] ?? null)).toBe(true);
}

/** The text of the header field `name` (`data-field` of `ScreenShell`). */
function headerField(container: HTMLElement, name: string): string | null {
  return container.querySelector(`[data-field="${name}"]`)?.textContent ?? null;
}

/**
 * Every `GridCell` of the body grid (the `FieldGrid` inside the form), as its map row, `POS`
 * column and the columns it spans, read from its placement classes, and its content: the
 * `data-field` it shows, or else its text. Sorted by row, then column.
 */
function bodyCells(container: HTMLElement) {
  const grid = container.querySelector("form")?.firstElementChild;
  if (grid === null || grid === undefined) {
    throw new Error("no body grid");
  }
  return Array.from(grid.querySelectorAll('[class^="row-start-"]'), (cell) => {
    const place = /^row-start-(\d+) col-start-(\d+) col-span-(\d+)$/.exec(cell.className);
    if (place === null) {
      throw new Error(`a cell placed as "${cell.className}"`);
    }
    const [, row, col, span] = place;
    const named = cell.querySelector("[data-field]");
    return {
      row: Number.parseInt(row, 10),
      col: Number.parseInt(col, 10),
      span: Number.parseInt(span, 10),
      content:
        named instanceof HTMLElement
          ? `field ${named.dataset.field ?? ""}`
          : `text ${cell.textContent ?? ""}`,
    };
  }).sort((a, b) => a.row - b.row || a.col - b.col);
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
 * Moves the cursor to `element`, so that its return to the Tran ID input is observable. The move
 * runs inside `waitFor`, which lets React render the blur it causes.
 */
async function moveFocus(element: HTMLElement): Promise<void> {
  await waitFor(() => {
    element.focus();
    expect(document.activeElement).toBe(element);
  });
}

/** Types `value` into the Tran ID input and presses ENTER through its button. */
function fetchTyped(value: string): void {
  fireEvent.change(tranIdInput(), { target: { value } });
  fireEvent.click(screen.getByRole("button", { name: "ENTER=Fetch" }));
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

beforeEach(() => {
  setSession("test-token", "USER0001", "U");
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COTRN01 transaction view screen", () => {
  // audit: COTRN01-011
  // audit: COTRN01-012
  // audit: COTRN01-015
  // audit: COTRN01-016
  // audit: COTRN01-019
  // audit: COTRN01-022
  // audit: COTRN01-025
  // audit: COTRN01-028
  // audit: COTRN01-031
  // audit: COTRN01-034
  // audit: COTRN01-037
  // audit: COTRN01-040
  // audit: COTRN01-043
  // audit: COTRN01-046
  // audit: COTRN01-049
  // audit: COTRN01-052
  // audit: COTRN01-056
  it("renders the map: title, labelled Tran ID input, 13 read-only fields, key row", () => {
    stubApi();
    renderScreen({ from: PATHS.COMEN01 });

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("View Transaction");

    const tranId = tranIdInput();
    expect(tranId.dataset.field).toBe("TRNIDIN");
    expect(tranId.maxLength).toBe(16);
    expect(tranId.readOnly).toBe(false);

    expect(screen.getByText("-".repeat(70)).textContent).toHaveLength(70);

    for (const output of OUTPUTS) {
      const box = field(output.label);
      expect(box.dataset.field).toBe(output.field);
      expect(box.readOnly).toBe(true);
      expect(box.tabIndex).toBe(-1);
      expect(box.size).toBe(output.length);
    }

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
      ["ENTER=Fetch", "submit", "default", "sm"],
      ["F3=Back", "button", "outline", "sm"],
      ["F4=Clear", "button", "outline", "sm"],
      ["F5=Browse Tran.", "button", "outline", "sm"],
    ]);

    // The map declares no INFOMSG row, only ERRMSG.
    expect(messageRows("info")).toHaveLength(0);
    expect(messageRows("error")).toHaveLength(1);
  });

  // audit: COTRN01-014
  // audit: COTRN01-018
  // audit: COTRN01-021
  // audit: COTRN01-024
  // audit: COTRN01-027
  // audit: COTRN01-030
  // audit: COTRN01-033
  // audit: COTRN01-036
  // audit: COTRN01-039
  // audit: COTRN01-042
  // audit: COTRN01-045
  // audit: COTRN01-048
  // audit: COTRN01-051
  // audit: COTRN01-054
  it("places each literal and field at its map POS and LENGTH, and nothing at the stoppers", () => {
    stubApi();
    const { container } = renderScreen({ from: PATHS.COMEN01 });

    const cells = bodyCells(container);
    expect(cells).toEqual(BODY_CELLS);

    // A stopper has no element, and no cell reaches into its attribute column.
    for (const stopper of STOPPERS) {
      expect({
        id: stopper.id,
        covering: cells.filter(
          (cell) =>
            cell.row === stopper.row &&
            cell.col <= stopper.col &&
            stopper.col < cell.col + cell.span,
        ),
      }).toEqual({ id: stopper.id, covering: [] });
    }
  });

  // audit: COTRN01C-029
  // audit: COTRN01-057
  // audit: COTRN01-058
  // audit: COTRN01-059
  // audit: COTRN01-060
  // audit: COTRN01-061
  // audit: COTRN01-062
  it("fills header rows 1 and 2 at every send: CT01, COTRN01C, titles, date, time", async () => {
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
    stubApi();
    const { container } = renderScreen({ from: PATHS.COMEN01 });

    expect(headerField(container, "TRNNAME")).toBe("CT01");
    expect(headerField(container, "PGMNAME")).toBe("COTRN01C");
    expect(headerField(container, "TITLE01")).toBe("      AWS Mainframe Modernization       ");
    expect(headerField(container, "TITLE02")).toBe("              CardDemo                  ");
    expect(headerField(container, "CURDATE")).toBe("01/02/24");
    expect(headerField(container, "CURTIME")).toBe("03:04:05");

    vi.setSystemTime(new Date(2025, 11, 31, 23, 59, 58));
    fetchTyped(TYPED_ID);
    await waitFor(() => {
      expect(shownDetails()).toEqual(SHOWN_TYPED);
    });

    expect(headerField(container, "TRNNAME")).toBe("CT01");
    expect(headerField(container, "PGMNAME")).toBe("COTRN01C");
    expect(headerField(container, "CURDATE")).toBe("12/31/25");
    expect(headerField(container, "CURTIME")).toBe("23:59:58");
  });

  // `ScreenShell` realises each header value field as read-only text in its title-row cell rather
  // than as an `Input` with `readOnly`, so these rows are proved on that cell. The values it shows
  // are proved under COTRN01-057 to COTRN01-062.
  // audit: COTRN01-002
  // audit: COTRN01-003
  // audit: COTRN01-005
  // audit: COTRN01-007
  // audit: COTRN01-008
  // audit: COTRN01-010
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

  // audit: COTRN01C-002
  it.each([
    { title: "from the main menu", state: { from: PATHS.COMEN01 } },
    { title: "with no route state", state: undefined },
    {
      title: "with a selected ID of spaces",
      state: { from: PATHS.COTRN00, tran_id: " ".repeat(16) },
    },
    {
      title: "with a selected ID that is not text",
      state: { from: PATHS.COTRN00, tran_id: 1 },
    },
  ])("opens empty $title, with the cursor on the Tran ID, sending nothing", async ({ state }) => {
    const requests = stubApi();
    renderScreen(state);

    expect(tranIdInput().value).toBe("");
    expect(document.activeElement).toBe(tranIdInput());
    expect(shownDetails()).toEqual(BLANK_DETAILS);
    expect(errorText()).toBe("");

    await settle();
    expect(requests).toHaveLength(0);
  });

  // audit: COTRN01C-003
  // audit: COTRN01-017
  // audit: COTRN01-020
  // audit: COTRN01-023
  // audit: COTRN01-026
  // audit: COTRN01-029
  // audit: COTRN01-032
  // audit: COTRN01-035
  // audit: COTRN01-038
  // audit: COTRN01-044
  // audit: COTRN01-047
  // audit: COTRN01-050
  // audit: COTRN01-053
  it("reads a COTRN00 selection at once, with no ENTER, showing values as received", async () => {
    const requests = stubApi();
    renderScreen(FROM_LIST);

    expect(tranIdInput().value).toBe(SELECTED_ID);
    await waitFor(() => {
      expect(shownDetails()).toEqual(SHOWN_SELECTED);
    });
    for (const output of OUTPUTS) {
      const box = field(output.label);
      expect(box.readOnly).toBe(true);
      expect(box.size).toBe(output.length);
    }
    expect(errorText()).toBe("");
    expect(document.activeElement).toBe(tranIdInput());

    // One read, rendered once: the entry sends endpoint 12 exactly one request.
    await settle();
    expect(requests).toHaveLength(1);
    const [request] = requests;
    expect(request?.method).toBe("GET");
    expect(paths(requests)).toEqual([`${ENDPOINT}${SELECTED_ID}`]);
    expect(request?.headers.get("Authorization")).toBe("Bearer test-token");
  });

  // audit: COTRN01-041
  it("ENTER sends the typed Tran ID as the path, then shows that transaction", async () => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await waitFor(() => {
      expect(shownDetails()).toEqual(SHOWN_SELECTED);
    });

    fetchTyped(TYPED_ID);

    await waitFor(() => {
      expect(shownDetails()).toEqual(SHOWN_TYPED);
    });
    const procDate = field("Proc Date:");
    expect(procDate.readOnly).toBe(true);
    expect(procDate.size).toBe(10);
    expect(tranIdInput().value).toBe(TYPED_ID);
    expect(errorText()).toBe("");
    await waitFor(() => {
      expect(document.activeElement).toBe(tranIdInput());
    });
    expect(paths(requests)).toEqual([`${ENDPOINT}${SELECTED_ID}`, `${ENDPOINT}${TYPED_ID}`]);
    expect(requests.map((request) => request.method)).toEqual(["GET", "GET"]);
  });

  // audit: COTRN01C-014
  // audit: COTRN01-055
  // COTRN01C-009's 400 cannot be reached from this screen: the schema refuses a blank Tran ID
  // before any request, and LOW-VALUES cannot be typed.
  it.each([
    { title: "the 404 of COTRN01C-012", tranId: UNKNOWN_ID, error: notFound },
    { title: "the 500 of COTRN01C-013", tranId: FAILING_ID, error: lookupFailed },
  ])("shows $title, keeps the typed ID focused and marked invalid, blanks details", async (row) => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await waitFor(() => {
      expect(shownDetails()).toEqual(SHOWN_SELECTED);
    });
    // ERRMSG is RED: the `destructive` variant of the shadcn/ui `Alert`.
    const [errorRow] = messageRows("error");
    expect(errorRow?.getAttribute("aria-live")).toBe("polite");
    expect(errorRow?.classList.contains("text-destructive")).toBe(true);
    expect(errorText()).toBe("");
    expect(tranIdErrorState()).toEqual(UNMARKED);

    const enter = screen.getByRole("button", { name: "ENTER=Fetch" });
    fireEvent.change(tranIdInput(), { target: { value: row.tranId } });
    await moveFocus(enter);
    fireEvent.click(enter);

    await waitFor(() => {
      expect(errorText()).toBe(row.error.message);
    });
    // The `tran_id` the error names marks the input, whose field message is the row's text.
    await waitFor(() => {
      expect(tranIdErrorState()).toEqual(markedWith(row.error.message));
    });
    expectShownOnce(row.error.message);
    expect(shownDetails()).toEqual(BLANK_DETAILS);
    expect(tranIdInput().value).toBe(row.tranId);
    await waitFor(() => {
      expect(document.activeElement).toBe(tranIdInput());
    });
    expect(paths(requests)).toEqual([`${ENDPOINT}${SELECTED_ID}`, `${ENDPOINT}${row.tranId}`]);
  });

  // COTRN01C-009 is a `both` row: the schema's half is proved in src/schemas/COTRN01.test.ts and
  // the server's in the endpoint tests. This shows what the screen does with the schema's refusal.
  it("ENTER with a blank Tran ID sends nothing, shows its message, blanks details", async () => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await waitFor(() => {
      expect(shownDetails()).toEqual(SHOWN_SELECTED);
    });

    fetchTyped("    ");

    await waitFor(() => {
      expect(errorText()).toBe("Tran ID can NOT be empty...");
    });
    // The refused input is marked, and its field message is the row's text.
    expect(tranIdErrorState()).toEqual(markedWith("Tran ID can NOT be empty..."));
    expectShownOnce("Tran ID can NOT be empty...");
    await waitFor(() => {
      expect(shownDetails()).toEqual(BLANK_DETAILS);
    });
    await settle();
    expect(paths(requests)).toEqual([`${ENDPOINT}${SELECTED_ID}`]);
  });

  // The input's mark is the form's error state, which no audit row records, so none is tagged.
  // A server error marks the value it rejected: an edit of it, or the answer to the next key,
  // removes the mark, while the message row keeps the program's last message.
  it.each([
    {
      title: "an edit after the 404 of the entry read",
      byEntry: true,
      act: () => {
        fireEvent.change(tranIdInput(), { target: { value: TYPED_ID } });
      },
      shown: BLANK_DETAILS,
      message: notFound.message,
    },
    {
      title: "an edit after the 404 of an ENTER",
      byEntry: false,
      act: () => {
        fireEvent.change(tranIdInput(), { target: { value: TYPED_ID } });
      },
      shown: BLANK_DETAILS,
      message: notFound.message,
    },
    {
      title: "an ENTER that finds the transaction",
      byEntry: false,
      act: () => {
        fetchTyped(TYPED_ID);
      },
      shown: SHOWN_TYPED,
      message: "",
    },
    {
      title: "F4=Clear",
      byEntry: false,
      act: () => {
        fireEvent.click(pfKeyButton("F4=Clear"));
      },
      shown: BLANK_DETAILS,
      message: "",
    },
    {
      title: "an unserved F-key",
      byEntry: false,
      act: () => {
        pressKey("F7");
      },
      shown: BLANK_DETAILS,
      message: INVALID_KEY_TEXT,
    },
  ])("$title unmarks the Tran ID input a server error marked", async (row) => {
    stubApi();
    renderScreen(row.byEntry ? { ...FROM_LIST, tran_id: UNKNOWN_ID } : FROM_LIST);
    if (!row.byEntry) {
      await waitFor(() => {
        expect(shownDetails()).toEqual(SHOWN_SELECTED);
      });
      fetchTyped(UNKNOWN_ID);
    }
    await waitFor(() => {
      expect(tranIdErrorState()).toEqual(markedWith(notFound.message));
    });

    row.act();

    await waitFor(() => {
      expect(shownDetails()).toEqual(row.shown);
    });
    await waitFor(() => {
      expect(tranIdErrorState()).toEqual(UNMARKED);
    });
    expect(errorText().trimEnd()).toBe(row.message);
  });

  // audit: COTRN01C-005
  // audit: COTRN01-065
  it("F4 empties the Tran ID, details and message row, refocuses, sends nothing", async () => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await waitFor(() => {
      expect(shownDetails()).toEqual(SHOWN_SELECTED);
    });

    // F4=Clear by its button, over a shown transaction, with the cursor away from the input.
    const clear = pfKeyButton("F4=Clear");
    await moveFocus(clear);
    fireEvent.click(clear);
    await waitFor(() => {
      expect(shownDetails()).toEqual(BLANK_DETAILS);
    });
    expect(tranIdInput().value).toBe("");
    expect(document.activeElement).toBe(tranIdInput());

    // The F4 key, over an error message.
    fetchTyped(UNKNOWN_ID);
    await waitFor(() => {
      expect(errorText()).toBe("Transaction ID NOT found...");
    });
    pressKey("F4");
    await waitFor(() => {
      expect(errorText()).toBe("");
    });
    expect(tranIdInput().value).toBe("");
    expect(shownDetails()).toEqual(BLANK_DETAILS);
    expect(document.activeElement).toBe(tranIdInput());

    await settle();
    expect(paths(requests)).toEqual([`${ENDPOINT}${SELECTED_ID}`, `${ENDPOINT}${UNKNOWN_ID}`]);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("View Transaction");
  });

  // audit: COTRN01C-004
  // audit: COTRN01-064
  // A return to /transactions also hands back, unchanged, the paging state COTRN00 sent, as
  // COTRN01C passes back the COMMAREA extension it received (COTRN00C-002); a return to /menu,
  // whose COMEN01C passes on only its 160-byte COMMAREA, carries none.
  it.each([
    {
      title: "F3=Back after a COTRN00 selection returns to /transactions",
      state: FROM_LIST,
      shown: SHOWN_SELECTED,
      byKey: false,
      target: "/transactions",
      list: LIST,
    },
    {
      title: "the F3 key after a COTRN00 selection returns to /transactions",
      state: FROM_LIST,
      shown: SHOWN_SELECTED,
      byKey: true,
      target: "/transactions",
      list: LIST,
    },
    {
      title: "F3=Back after entry from the main menu returns to /menu",
      state: { from: PATHS.COMEN01 },
      shown: BLANK_DETAILS,
      byKey: false,
      target: "/menu",
      list: undefined,
    },
    {
      title: "F3=Back with no recorded origin returns to /menu",
      state: undefined,
      shown: BLANK_DETAILS,
      byKey: false,
      target: "/menu",
      list: undefined,
    },
    {
      title: "F3=Back with an origin that is no route of the application returns to /menu",
      state: { from: "https://example.invalid/transactions", tran_id: SELECTED_ID, list: LIST },
      shown: SHOWN_SELECTED,
      byKey: false,
      target: "/menu",
      list: undefined,
    },
  ])("$title, recording COTRN01 and PF3 as the origin, with no request", async (row) => {
    const requests = stubApi();
    renderScreen(row.state);
    // The entry's outcome first, so that only the key's effect remains to observe.
    await waitFor(() => {
      expect(shownDetails()).toEqual(row.shown);
    });
    await settle();
    const sent = requests.length;

    const back = pfKeyButton("F3=Back");
    if (row.byKey) {
      pressKey("F3");
    } else {
      fireEvent.click(back);
    }

    const stub = await reachedStub();
    expect(stub.path).toBe(row.target);
    expect(stub.state).toMatchObject({ from: "/transactions/view", entry_key: "pf3" });
    if (row.list === undefined) {
      expect(stub.state).not.toHaveProperty("list");
    } else {
      expect(stub.state).toMatchObject({ list: row.list });
    }
    await settle();
    expect(requests).toHaveLength(sent);
  });

  // audit: COTRN01C-006
  // audit: COTRN01-066
  // The list gets back, unchanged, the paging state it sent (COTRN00C-002); after entry from the
  // main menu, or with a `list` that is no record, none is invented, so the list starts blank.
  it.each([
    {
      title: "F5=Browse Tran. after a COTRN00 selection",
      state: FROM_LIST,
      shown: SHOWN_SELECTED,
      byKey: false,
      list: LIST,
    },
    {
      title: "the F5 key after entry from the main menu",
      state: { from: PATHS.COMEN01 },
      shown: BLANK_DETAILS,
      byKey: true,
      list: undefined,
    },
    {
      title: "F5=Browse Tran. after a selection whose list state is an array",
      state: { ...FROM_LIST, list: [LIST] },
      shown: SHOWN_SELECTED,
      byKey: false,
      list: undefined,
    },
    {
      title: "F5=Browse Tran. after a selection whose list state is null",
      state: { ...FROM_LIST, list: null },
      shown: SHOWN_SELECTED,
      byKey: false,
      list: undefined,
    },
  ])("$title goes to /transactions as PF5 from COTRN01, with no request", async (row) => {
    const requests = stubApi();
    renderScreen(row.state);
    // The entry's outcome first, so that only the key's effect remains to observe.
    await waitFor(() => {
      expect(shownDetails()).toEqual(row.shown);
    });
    await settle();
    const sent = requests.length;

    const browse = pfKeyButton("F5=Browse Tran.");
    if (row.byKey) {
      pressKey("F5");
    } else {
      fireEvent.click(browse);
    }

    const stub = await reachedStub();
    expect(stub.path).toBe("/transactions");
    expect(stub.state).toMatchObject({ from: "/transactions/view", entry_key: "pf5" });
    if (row.list === undefined) {
      expect(stub.state).not.toHaveProperty("list");
    } else {
      expect(stub.state).toMatchObject({ list: row.list });
    }
    await settle();
    expect(requests).toHaveLength(sent);
  });

  // audit: COTRN01C-007
  // audit: COTRN01C-030
  it.each([
    ...UNSERVED_F_KEYS.map((key) => ({ title: key, key, shiftKey: false })),
    // PF13 to PF24: COTRN01C does not fold them onto PF1 to PF12, so Shift+F3 is no F3.
    ...ALL_F_KEYS.map((key) => ({ title: `Shift+${key}`, key, shiftKey: true })),
  ])("$title shows the invalid-key message, keeps the Tran ID, blanks the details", async (row) => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await waitFor(() => {
      expect(shownDetails()).toEqual(SHOWN_SELECTED);
    });
    fireEvent.change(tranIdInput(), { target: { value: TYPED_ID } });

    pressKey(row.key, row.shiftKey);

    await waitFor(() => {
      expect(errorText().trimEnd()).toBe(INVALID_KEY_TEXT);
    });
    // The longest message COTRN01C moves fits the 78 columns of ERRMSG uncut.
    expect(errorText().length).toBeLessThanOrEqual(78);
    expect(tranIdInput().value).toBe(TYPED_ID);
    expect(shownDetails()).toEqual(BLANK_DETAILS);
    expect(screen.queryByRole("region", { name: "Route stub" })).toBeNull();

    await settle();
    expect(paths(requests)).toEqual([`${ENDPOINT}${SELECTED_ID}`]);
  });

  // audit: COTRN01C-008
  // The list's paging state is kept beside the origin, so F3 still hands it back (COTRN00C-002).
  it("keeps its origin across ENTER, F4 and invalid keys: F3 returns to COTRN00", async () => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await waitFor(() => {
      expect(shownDetails()).toEqual(SHOWN_SELECTED);
    });

    fetchTyped(TYPED_ID);
    await waitFor(() => {
      expect(shownDetails()).toEqual(SHOWN_TYPED);
    });
    fireEvent.click(screen.getByRole("button", { name: "F4=Clear" }));
    await waitFor(() => {
      expect(shownDetails()).toEqual(BLANK_DETAILS);
    });
    pressKey("F7");
    await waitFor(() => {
      expect(errorText().trimEnd()).toBe(INVALID_KEY_TEXT);
    });

    fireEvent.click(screen.getByRole("button", { name: "F3=Back" }));

    const stub = await reachedStub();
    expect(stub.path).toBe("/transactions");
    expect(stub.state).toMatchObject({ from: "/transactions/view", entry_key: "pf3", list: LIST });
    expect(paths(requests)).toEqual([`${ENDPOINT}${SELECTED_ID}`, `${ENDPOINT}${TYPED_ID}`]);
  });
});

// Endpoint 12 names the transaction by its path alone, and every request carries the stored
// session's token.
describe("COTRN01: endpoint 12 request", () => {
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

  it("the read of a COTRN00 selection is one GET of its path with the bearer token, no query and no body", async () => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await waitFor(() => {
      expect(shownDetails()).toEqual(SHOWN_SELECTED);
    });
    await settle();

    expect(await Promise.all(requests.map(shapeOf))).toEqual([
      {
        method: "GET",
        path: `${ENDPOINT}${SELECTED_ID}`,
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
 * Renders the application's route table at `/transactions/view`, so the sign-on guard that only
 * the table mounts stands in front of the screen. Its screens load lazily, so a wait for one to
 * render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COTRN01] });
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
describe("COTRN01 in the route table: a 401 ends the session", () => {
  it("ENTER answered 401 replaces the screen with sign-on and clears the session, with no retry", async () => {
    const requests: Request[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const req = input instanceof Request ? input : new Request(input, init);
        requests.push(req);
        return json(401, signonRequired);
      }),
    );
    const router = renderRouteTable();
    expect(
      await screen.findByRole(
        "heading",
        { level: 1, name: "View Transaction" },
        { timeout: 5_000 },
      ),
    ).toBeDefined();

    fetchTyped(TYPED_ID);

    expect(
      await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
    ).toBeDefined();
    expect(router.state.location.pathname).toBe(PATHS.COSGN00);
    expect(screen.queryByRole("heading", { level: 1, name: "View Transaction" })).toBeNull();
    expect(getSession()).toBeNull();
    expect(paths(requests)).toEqual([`${ENDPOINT}${TYPED_ID}`]);
    expect(
      requests.map((request) => [request.method, request.headers.get("Authorization")]),
    ).toEqual([["GET", "Bearer test-token"]]);
    router.dispose();
  });
});

// The sign-on guard lives in the application's route tree, not in the screen, so this group
// renders that tree rather than the screen beside stubs.
describe("COTRN01 behind the route guard", () => {
  // audit: COTRN01C-001
  it("sends a user with no session from /transactions/view to /signon, with no request", async () => {
    clearSession();
    const requests = stubApi();
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, { initialEntries: [PATHS.COTRN01] });
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );

    // Both screens load lazily, through the route tree, before the sign-on screen renders.
    await waitFor(
      () => {
        expect(router.state.location.pathname).toBe(PATHS.COSGN00);
        expect(screen.getByRole("button", { name: "ENTER=Sign-on" })).toBeDefined();
      },
      { timeout: 4000 },
    );
    expect(screen.queryByRole("heading", { level: 1, name: "View Transaction" })).toBeNull();
    expect(screen.queryByLabelText("Enter Tran ID:")).toBeNull();
    await settle();
    expect(requests).toHaveLength(0);
    router.dispose();
  });
});
