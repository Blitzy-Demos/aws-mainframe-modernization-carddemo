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
  useNavigate,
  type RouteObject,
} from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { components } from "@/api/schema";
import { clearSession, getSession, setSession } from "@/auth/session";
import { PATHS, routes } from "@/routes";
import { COTRN00 } from "./COTRN00";

/**
 * Render test of map `COTRN0A` (transaction CT00, program COTRN00C) at `/transactions`, run in
 * jsdom with the global `fetch` stubbed, so no request leaves the test (Local-Only Runtime).
 * Endpoint 11, `GET /api/v1/transactions`, is the only operation the screen calls; any other
 * request is answered 500 and recorded, and every test asserts how many requests were sent.
 *
 * The transaction IDs are fake, and `TranListResponse` has no card-number field, so no fixture
 * carries cardholder data (Cardholder Data Redaction).
 */

const TOKEN = "test-token";
const LIST_PATH = "/api/v1/transactions";

/**
 * The classes of the shared map-field box, FieldGrid's `FIELD_INPUT`: `h-7` and `py-0` are the
 * one box height every map field shares (`FIELD_BOX_HEIGHT`, 28px), `min-w-max` and
 * `field-sizing-content` keep a box never narrower than its value, `px-1` is the one inset every
 * map input shares and `text-sm` the inputs' type size. Restated here because the render-test
 * contract imports no module beside the screen, the session, the routes and the generated types,
 * so a box is checked against this list rather than against the constant that renders it.
 */
const MAP_FIELD_BOX = [
  "h-7",
  "py-0",
  "min-w-max",
  "px-1",
  "text-sm",
  "field-sizing-content",
] as const;

/*
 * The header titles and the invalid-key message, restated from their copybooks so the screen is
 * checked against the oracle rather than against the module that renders them: the render-test
 * contract imports no module beside the screen, the session, the routes and the generated types.
 */
/** `CCDA-TITLE01 PIC X(40)` (`app/cpy/COTTL01Y.cpy:18-19`), padding included. */
const TITLE01 = "      AWS Mainframe Modernization       ";
/** `CCDA-TITLE02 PIC X(40)` (`app/cpy/COTTL01Y.cpy:20-22`), padding included. */
const TITLE02 = "              CardDemo                  ";
/** `CCDA-MSG-INVALID-KEY PIC X(50)` (`app/cpy/CSMSG01Y.cpy:20-21`), padded to its 50 bytes. */
const INVALID_KEY = "Invalid key pressed. Please see below...".padEnd(50, " ");

/** The program's messages, as `modern/audit/online/COTRN00C.md` words them. */
const INVALID_SELECTION = "Invalid selection. Valid value is S";
const NOT_NUMERIC = "Tran ID must be Numeric ...";
const BOTTOM_REACHED = "You have reached the bottom of the page...";
const ALREADY_BOTTOM = "You are already at the bottom of the page...";
const ALREADY_TOP = "You are already at the top of the page...";
const LOOKUP_FAILED = "Unable to lookup transaction...";

/** Transaction ID `n` as the 16 characters of `TRAN-ID PIC X(16)`. */
function tranId(n: number): string {
  return String(n).padStart(16, "0");
}

/** `TRAN-AMT` edited by `PIC +99999999.99`: twelve characters each, shown as received. */
const AMOUNTS = [
  "+00000050.47",
  "-00000091.90",
  "+00000006.78",
  "+00001234.56",
  "-00000000.01",
  "+00000100.00",
  "+99999999.99",
  "-00000450.00",
  "+00000012.34",
  "+00000000.00",
] as const;

/**
 * Page 1 of a first entry: ten transactions of 2022-06-10, a following page and no message
 * (COTRN00C-014, COTRN00C-016). Half of them are stamped at midnight and half at the last
 * microsecond of the day, and the eight-character `MM/DD/YY` display date is the same for all.
 */
const FIRST_PAGE: components["schemas"]["TranListResponse"] = {
  rows: AMOUNTS.map((amount, index) => ({
    tran_id: tranId(index + 1),
    orig_ts: index % 2 === 0 ? "2022-06-10T00:00:00" : "2022-06-10T23:59:59.999999",
    display_date: "06/10/22",
    description: `Purchase ${index + 1}`.padEnd(26, " "),
    amount,
  })),
  first_key: tranId(1),
  last_key: tranId(10),
  page_no: 1,
  has_next_page: true,
  message: null,
  rows_kept: false,
};

/** Page 2, the last one: transactions 11 to 20 and the end-of-file message (COTRN00C-017). */
const SECOND_PAGE: components["schemas"]["TranListResponse"] = {
  rows: [...AMOUNTS].reverse().map((amount, index) => ({
    tran_id: tranId(index + 11),
    orig_ts: "2022-06-11T12:00:00",
    display_date: "06/11/22",
    description: `Refund ${index + 11}`.padEnd(26, " "),
    amount,
  })),
  first_key: tranId(11),
  last_key: tranId(20),
  page_no: 2,
  has_next_page: false,
  message: BOTTOM_REACHED,
  rows_kept: false,
};

/**
 * A first page of three transactions, rows 4 to 10 blank; the last key keeps the blank state's
 * value, as only row 10 sets it (COTRN00C-018).
 */
const SHORT_PAGE: components["schemas"]["TranListResponse"] = {
  rows: [...FIRST_PAGE.rows.slice(0, 3), ...Array.from({ length: 7 }, () => null)],
  first_key: tranId(1),
  last_key: null,
  page_no: 1,
  has_next_page: false,
  message: BOTTOM_REACHED,
  rows_kept: false,
};

/** F8 on the last page: nothing read, the screen keeps what it shows (COTRN00C-021). */
const BOTTOM_KEPT: components["schemas"]["TranListResponse"] = {
  rows: [],
  first_key: tranId(11),
  last_key: tranId(20),
  page_no: 2,
  has_next_page: false,
  message: ALREADY_BOTTOM,
  rows_kept: true,
};

/** F7 on page 1: nothing read, the screen keeps what it shows (COTRN00C-022). */
const TOP_KEPT: components["schemas"]["TranListResponse"] = {
  rows: [],
  first_key: tranId(1),
  last_key: tranId(10),
  page_no: 1,
  has_next_page: true,
  message: ALREADY_TOP,
  rows_kept: true,
};

/** The 400 of a search value that is neither blank nor 16 digits (COTRN00C-013). */
const NOT_NUMERIC_ERROR: components["schemas"]["ScreenError"] = {
  message: NOT_NUMERIC,
  field: "start_tran_id",
};

/** The 500 of a page read that failed (COTRN00C-027). */
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
 * `answer`, held back until `release` is called, so its request stays outstanding while a test
 * presses keys.
 */
function heldReply(answer: () => Response): {
  reply: () => Promise<Response>;
  release: () => void;
} {
  const opens: (() => void)[] = [];
  const gate = new Promise<void>((resolve) => {
    opens.push(resolve);
  });
  return {
    reply: async () => {
      await gate;
      return answer();
    },
    release: () => {
      for (const open of opens) {
        open();
      }
    },
  };
}

/**
 * Replaces the global `fetch`. Each `GET /api/v1/transactions` (endpoint 11) takes the next of
 * `replies`; any other request, and any past the last reply, is answered 500. Returns every
 * request's method, path, full URL, query parameters and `Authorization` header, in order.
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

/**
 * Stands in for COTRN01 at `/transactions/view`: its F3 and F5 return to the list with the route
 * state COTRN01C's `XCTL` back carries, the list's paging state passed back unchanged
 * (COTRN00C-002). That COTRN01 returns exactly this state is proved by `COTRN01.test.tsx`; this
 * file proves the list's half: the state it hands out and the state it reads back.
 */
function ViewHandBackStub() {
  const location = useLocation();
  const navigate = useNavigate();
  const state: unknown = location.state;
  const list =
    typeof state === "object" && state !== null && "list" in state ? state.list : undefined;
  const back = (entryKey: components["schemas"]["TranListEntryKey"]) => {
    navigate(PATHS.COTRN00, { state: { from: PATHS.COTRN01, entry_key: entryKey, list } });
  };
  return (
    <section aria-label="View Transaction">
      <h1>View Transaction</h1>
      <button type="button" onClick={() => back("pf3")}>
        F3=Back
      </button>
      <button type="button" onClick={() => back("pf5")}>
        F5=Browse Tran.
      </button>
    </section>
  );
}

/**
 * Renders the screen at `/transactions` with `state`, beside stubs at its transfer targets, or
 * with `view` instead of the stub at `/transactions/view`. Returns the render result and the
 * query client, whose pending requests {@link clientIdle} reads.
 */
function renderList(state?: unknown, view = <TransferStub name="View Transaction" />) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const rendered = render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COTRN00, state }]}>
        <Routes>
          <Route path={PATHS.COTRN00} element={<COTRN00 />} />
          <Route path={PATHS.COTRN01} element={view} />
          <Route path={PATHS.COMEN01} element={<TransferStub name="Main Menu" />} />
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

/** Waits until `count` requests were sent and the screen shows the answer to the last one. */
async function answered(sent: readonly unknown[], count: number): Promise<void> {
  await waitFor(() => {
    expect(sent).toHaveLength(count);
    expect(document.querySelector("form")?.getAttribute("aria-busy")).toBe("false");
  });
}

/** The value of an `<input>`. */
function valueOf(element: HTMLElement): string {
  if (!(element instanceof HTMLInputElement)) {
    throw new TypeError(`${element.tagName} is not an input`);
  }
  return element.value;
}

function searchField(): HTMLElement {
  return screen.getByLabelText("Search Tran ID:");
}

function pageField(): HTMLElement {
  return screen.getByLabelText("Page:");
}

/** The selector of list row `row`, 1 to 10. */
function selector(row: number): HTMLElement {
  return screen.getByLabelText(`Select row ${row}`);
}

/** Types `value` into `field`, as one change event. */
function typeInto(field: HTMLElement, value: string): void {
  fireEvent.change(field, { target: { value } });
}

/** Clicks the row-24 key button labelled `label`. */
function press(label: string): void {
  fireEvent.click(screen.getByRole("button", { name: label }));
}

/** The row-24 labels, in map order. */
const KEY_LABELS = ["ENTER=Continue", "F3=Back", "F7=Backward", "F8=Forward"];

/** Whether the row-24 button labelled `label` is disabled (`aria-disabled="true"`). */
function keyDisabled(label: string): boolean {
  const button = screen.getByRole("button", { name: label });
  if (!(button instanceof HTMLButtonElement)) {
    throw new TypeError(`${button.tagName} is not a button`);
  }
  return button.getAttribute("aria-disabled") === "true";
}

/** Whether an `<input>` is read-only. */
function readOnlyOf(element: HTMLElement): boolean {
  if (!(element instanceof HTMLInputElement)) {
    throw new TypeError(`${element.tagName} is not an input`);
  }
  return element.readOnly;
}

/**
 * Whether the screen shows itself waiting for a page: `aria-busy` of the "Function keys" group and
 * of the form, whether each row-24 button is `aria-disabled`, by label, and whether the search
 * field and the ten selectors are read-only.
 */
function lockState() {
  return {
    keyRowBusy: screen.getByRole("group", { name: "Function keys" }).getAttribute("aria-busy"),
    formBusy: document.querySelector("form")?.getAttribute("aria-busy") ?? null,
    disabled: Object.fromEntries(KEY_LABELS.map((label) => [label, keyDisabled(label)])),
    readOnly: [searchField(), ...Array.from({ length: 10 }, (_, index) => selector(index + 1))].map(
      readOnlyOf,
    ),
  };
}

/** {@link lockState} while a page is outstanding: ENTER, F7 and F8 refused, F3 available. */
const LOCKED = {
  keyRowBusy: "true",
  formBusy: "true",
  disabled: {
    "ENTER=Continue": true,
    "F3=Back": false,
    "F7=Backward": true,
    "F8=Forward": true,
  },
  readOnly: Array.from({ length: 11 }, () => true),
};

/** {@link lockState} with no page outstanding: every key and input available. */
const UNLOCKED = {
  keyRowBusy: "false",
  formBusy: "false",
  disabled: {
    "ENTER=Continue": false,
    "F3=Back": false,
    "F7=Backward": false,
    "F8=Forward": false,
  },
  readOnly: Array.from({ length: 11 }, () => false),
};

/** The screen's `<form>`. */
function screenForm(): HTMLFormElement {
  const form = document.querySelector("form");
  if (form === null) {
    throw new Error("no form");
  }
  return form;
}

/**
 * Waits until `client` fetches no query and has exactly `held` mutations pending: the replies the
 * test holds back, none by default. Endpoint 11 is a mutation of that client. It stays pending
 * from the moment the screen starts it until its reply has arrived; the screen's own handling of
 * the reply, chained on that mutation's promise, runs after. Every request the screen had started
 * when this is called, the held ones aside, has therefore been answered once it returns. It
 * cannot see a request the screen has not started yet: a test calls it once it has awaited what
 * the screen shows at the end of a key, or after {@link idleAfter50ms}.
 */
async function clientIdle(client: QueryClient, held = 0): Promise<void> {
  await waitFor(() => {
    expect(client.isFetching()).toBe(0);
    expect(client.isMutating()).toBe(held);
  });
}

/**
 * For what must send nothing and leaves nothing on screen to wait for, such as the keys the lock
 * refuses or a screen its guard refuses: lets 50 ms pass inside `act`, so whatever follows within
 * that window (a render, a request) runs and renders first, then waits as {@link clientIdle} does.
 * It shows only that nothing was sent within those 50 ms, not that nothing would be sent later.
 */
async function idleAfter50ms(client: QueryClient, held = 0): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => {
      setTimeout(resolve, 50);
    });
  });
  await clientIdle(client, held);
}

/** The table's two row groups: the caption and rule lines, then the ten list lines. */
function rowGroups(): HTMLElement[] {
  return within(screen.getByRole("table", { name: "Transactions" })).getAllByRole("rowgroup");
}

/** The ten list lines (screen lines 10 to 19). */
function listLines(): HTMLElement[] {
  const [, body] = rowGroups();
  return within(body).getAllByRole("row");
}

/** A column of the list `Table`: the map column it starts at and its width in map columns. */
interface ListColumn {
  readonly start: number;
  readonly width: number;
}

/**
 * The columns of `table` from its `colgroup`, the first starting at map column `first`: each
 * `col` is a `w-k/n` fraction of the list's `span` map columns, and together they fill it.
 */
function listColumns(table: HTMLElement, first: number, span: number): ListColumn[] {
  const columns: ListColumn[] = [];
  let start = first;
  for (const col of table.querySelectorAll("colgroup > col")) {
    const [, width = "", of = ""] = /^w-(\d+)\/(\d+)$/.exec(col.className) ?? [];
    expect(of).toBe(String(span));
    columns.push({ start, width: Number.parseInt(width, 10) });
    start += Number.parseInt(width, 10);
  }
  expect(start).toBe(first + span);
  return columns;
}

/**
 * The map column `element` shows its content at: its column's start, moved by its `left-k/w` or
 * `-left-k/w` class, `k` of the column's `w` map columns.
 */
function shownColumn(element: Element, column: ListColumn | undefined): number {
  if (column === undefined) {
    throw new Error("a list cell stands outside the colgroup's columns");
  }
  const shift = /(?:^|\s)(-?)left-(\d+)\/(\d+)(?:\s|$)/.exec(element.className);
  if (shift === null) {
    return column.start;
  }
  const [, sign, moved, of] = shift;
  expect(element.classList.contains("relative")).toBe(true);
  expect(of).toBe(String(column.width));
  const columns = Number.parseInt(moved, 10);
  return sign === "-" ? column.start - columns : column.start + columns;
}

/** The map column each cell of `row` shows its text at: its own, or its one wrapper's. */
function shownColumns(row: Element | undefined, columns: readonly ListColumn[]): number[] {
  return Array.from(row?.children ?? [], (cell, index) => {
    return shownColumn(cell.firstElementChild ?? cell, columns.at(index));
  });
}

/** The transaction ID, date, description and amount a list line shows, verbatim. */
function lineValues(line: HTMLElement): (string | null)[] {
  return within(line)
    .getAllByRole("cell")
    .slice(1)
    .map((cell) => cell.textContent);
}

/** Asserts that the ten list lines show the rows of `page` exactly, blank rows empty. */
function expectListed(page: components["schemas"]["TranListResponse"]): void {
  const lines = listLines();
  expect(lines).toHaveLength(10);
  lines.forEach((line, index) => {
    const row = page.rows[index] ?? null;
    expect(lineValues(line)).toEqual(
      row === null
        ? ["", "", "", ""]
        : [row.tran_id, row.display_date, row.description, row.amount],
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

/**
 * The message element `field`'s `aria-describedby` names. The form names it only while the
 * field is invalid, so this is `undefined` for a valid field and fails when the name it gives
 * resolves to no element.
 */
function fieldMessage(field: HTMLElement): HTMLElement | undefined {
  const id = field
    .getAttribute("aria-describedby")
    ?.split(" ")
    .find((name) => name.endsWith("-form-item-message"));
  if (id === undefined) {
    return undefined;
  }
  const message = document.getElementById(id);
  expect(message).not.toBeNull();
  return message ?? undefined;
}

/** Every id an `aria-describedby` on the page names that resolves to no element. */
function danglingDescriptions(): string[] {
  return [...document.querySelectorAll("[aria-describedby]")].flatMap((element) =>
    (element.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .filter((id) => id === "" || document.getElementById(id) === null),
  );
}

/**
 * How assistive technology reads `field`: its `aria-invalid`, and the element each id of its
 * `aria-describedby` names, as its text and whether `sr-only` hides it (`null` for an id that
 * resolves to nothing). `described` is `null` when the input has no `aria-describedby`.
 */
function readOut(field: HTMLElement): {
  invalid: string | null;
  described: ({ text: string | null; hidden: boolean } | null)[] | null;
} {
  const ids = field.getAttribute("aria-describedby");
  return {
    invalid: field.getAttribute("aria-invalid"),
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

/** Asserts that `text` is visible once, in the error row; any other copy is `sr-only`. */
function expectShownOnceInErrorRow(text: string): void {
  const visible = screen
    .getAllByText(text)
    .filter((element) => element.closest(".sr-only") === null);
  expect(visible).toHaveLength(1);
  expect(errorRow().contains(visible[0])).toBe(true);
}

/**
 * The value of the header field `name` (`TRNNAME`, `CURDATE`, …): the one read-only `Input` of
 * that name in the `CardHeader`, failing the test when there is not exactly one.
 */
function headerField(name: string): string {
  const fields = document.querySelectorAll(
    `[data-slot="card-header"] input[data-field="${name}"]`,
  );
  expect(fields).toHaveLength(1);
  const [field] = fields;
  if (!(field instanceof HTMLInputElement)) {
    throw new Error(`The header has no input ${name}`);
  }
  return field.value;
}

beforeEach(() => {
  setSession(TOKEN, "USER0001", "U");
});

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COTRN00 List Transactions", () => {
  it("renders the map: header rows, title, Page and Search Tran ID fields, list captions, instruction and the four key labels", async () => {
    // audit: COTRN00-011
    // audit: COTRN00-012
    // audit: COTRN00-013
    // audit: COTRN00-014
    // audit: COTRN00-017
    // audit: COTRN00-018
    // audit: COTRN00-019
    // audit: COTRN00-020
    // audit: COTRN00-021
    // audit: COTRN00-022
    // audit: COTRN00-023
    // audit: COTRN00-024
    // audit: COTRN00-025
    // audit: COTRN00-026
    // audit: COTRN00-087
    // audit: COTRN00-089
    // audit: COTRN00-090
    // audit: COTRN00-091
    // audit: COTRN00-092
    // audit: COTRN00-093
    // audit: COTRN00-094
    // audit: COTRN00-095
    // audit: COTRN00C-039
    const sent = stubFetch(json(200, FIRST_PAGE));
    renderList();
    await answered(sent, 1);

    expect(sent[0].authorization).toBe(`Bearer ${TOKEN}`);

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("List Transactions");
    expect(headerField("TRNNAME")).toBe("CT00");
    expect(headerField("PGMNAME")).toBe("COTRN00C");
    expect(headerField("TITLE01")).toBe(TITLE01);
    expect(headerField("TITLE02")).toBe(TITLE02);
    // Time-dependent fields are compared by format only.
    expect(headerField("CURDATE")).toMatch(/^\d{2}\/\d{2}\/\d{2}$/);
    expect(headerField("CURTIME")).toMatch(/^\d{2}:\d{2}:\d{2}$/);

    // PAGENUM, POS=(4,71) LENGTH=8: ASKIP output the cursor skips, showing endpoint 11's page.
    expect(pageField().hasAttribute("readonly")).toBe(true);
    expect(pageField().getAttribute("tabindex")).toBe("-1");
    expect(pageField().parentElement?.className).toBe("row-start-1 col-start-71 col-span-8");
    expect(valueOf(pageField())).toBe("00000001");
    // TRNIDIN, POS=(6,21) LENGTH=16.
    expect(searchField().getAttribute("maxlength")).toBe("16");
    expect(searchField().parentElement?.className).toBe("row-start-6 col-start-21 col-span-16");
    // The page, the search key and the selectors take the shared map-field box, `MAP_FIELD_BOX`
    // with `size` = their LENGTH, so all eight page digits show on the narrowest frame.
    for (const [field, length] of [
      [pageField(), "8"],
      [searchField(), "16"],
      [selector(1), "1"],
    ] as const) {
      expect(field.getAttribute("size")).toBe(length);
      expect(MAP_FIELD_BOX.filter((name) => !field.classList.contains(name))).toEqual([]);
    }

    const [head] = rowGroups();
    const [captions, rules] = within(head).getAllByRole("row", { hidden: true });
    expect(
      within(captions)
        .getAllByRole("columnheader")
        .map((cell) => cell.textContent),
    ).toEqual([
      "Sel",
      " Transaction ID ",
      "  Date  ",
      "     Description          ",
      "   Amount   ",
    ]);
    // Line 9's rules are decoration hidden from assistive technology: plain cells, no headers.
    expect(rules.getAttribute("aria-hidden")).toBe("true");
    expect(within(rules).queryAllByRole("columnheader", { hidden: true })).toEqual([]);
    expect(
      within(rules)
        .getAllByRole("cell", { hidden: true })
        .map((cell) => cell.textContent),
    ).toEqual(["---", "-".repeat(16), "-".repeat(8), "-".repeat(26), "-".repeat(12)]);

    expect(
      screen.getByText("Type 'S' to View Transaction details from the list").textContent,
    ).toBe("Type 'S' to View Transaction details from the list");

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
    // audit: COTRN00-002
    {
      id: "COTRN00-002",
      name: "TRNNAME",
      row: 1,
      col: 7,
      length: 4,
      label: "Tran:",
      shows: "CT00",
    },
    // audit: COTRN00-003
    {
      id: "COTRN00-003",
      name: "TITLE01",
      row: 1,
      col: 21,
      length: 40,
      label: null,
      shows: TITLE01,
    },
    // audit: COTRN00-005
    {
      id: "COTRN00-005",
      name: "CURDATE",
      row: 1,
      col: 71,
      length: 8,
      label: "Date:",
      shows: expect.stringMatching(/^\d{2}\/\d{2}\/\d{2}$/),
    },
    // audit: COTRN00-007
    {
      id: "COTRN00-007",
      name: "PGMNAME",
      row: 2,
      col: 7,
      length: 8,
      label: "Prog:",
      shows: "COTRN00C",
    },
    // audit: COTRN00-008
    {
      id: "COTRN00-008",
      name: "TITLE02",
      row: 2,
      col: 21,
      length: 40,
      label: null,
      shows: TITLE02,
    },
    // audit: COTRN00-010
    {
      id: "COTRN00-010",
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
      // The values these fields show are proved under COTRN00-090 to COTRN00-095.
      const sent = stubFetch(json(200, FIRST_PAGE));
      renderList();
      await answered(sent, 1);

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
    // audit: COTRN00-011
    // audit: COTRN00-012
    // audit: COTRN00-013
    const sent = stubFetch(json(200, FIRST_PAGE));
    renderList();
    await answered(sent, 1);

    // Map line 4 holds the title and both fields: they are cells of the one line of the frame's
    // header, each at its map column, and the body gives the line no track.
    const heading = screen.getByRole("heading", { level: 1, name: "List Transactions" });
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


  it("reads page 1 on entry from the menu and shows its rows verbatim, with empty one-character selectors", async () => {
    // audit: COTRN00-029
    // audit: COTRN00-030
    // audit: COTRN00-031
    // audit: COTRN00-032
    // audit: COTRN00-035
    // audit: COTRN00-036
    // audit: COTRN00-037
    // audit: COTRN00-038
    // audit: COTRN00-041
    // audit: COTRN00-042
    // audit: COTRN00-043
    // audit: COTRN00-044
    // audit: COTRN00-047
    // audit: COTRN00-048
    // audit: COTRN00-049
    // audit: COTRN00-050
    // audit: COTRN00-053
    // audit: COTRN00-054
    // audit: COTRN00-055
    // audit: COTRN00-056
    // audit: COTRN00-059
    // audit: COTRN00-060
    // audit: COTRN00-061
    // audit: COTRN00-062
    // audit: COTRN00-065
    // audit: COTRN00-066
    // audit: COTRN00-067
    // audit: COTRN00-068
    // audit: COTRN00-071
    // audit: COTRN00-072
    // audit: COTRN00-073
    // audit: COTRN00-074
    // audit: COTRN00-077
    // audit: COTRN00-078
    // audit: COTRN00-079
    // audit: COTRN00-080
    // audit: COTRN00-083
    // audit: COTRN00-084
    // audit: COTRN00-085
    // audit: COTRN00-086
    const sent = stubFetch(json(200, FIRST_PAGE));
    renderList();
    await answered(sent, 1);

    // A menu entry starts from the blank state: no keys, page 0, next-page flag N (COTRN00C-002).
    expect(sent[0].method).toBe("GET");
    expect(sent[0].path).toBe(LIST_PATH);
    expect(sent[0].query).toEqual({ entry_key: "enter", page_no: "0", has_next_page: "false" });

    const lines = listLines();
    expect(lines).toHaveLength(10);
    lines.forEach((line, index) => {
      // The date is the server's eight-character MM/DD/YY, the same for midnight and for the
      // day's last microsecond; the amount and the 26-character description are as received.
      expect(lineValues(line)).toEqual([
        tranId(index + 1),
        "06/10/22",
        `Purchase ${index + 1}`.padEnd(26, " "),
        AMOUNTS[index],
      ]);
    });
    expectListed(FIRST_PAGE);
    expect(valueOf(pageField())).toBe("00000001");

    for (let row = 1; row <= 10; row += 1) {
      expect(selector(row).getAttribute("maxlength")).toBe("1");
      expect(valueOf(selector(row))).toBe("");
    }
    expect(valueOf(searchField())).toBe("");
    expect(document.activeElement).toBe(searchField());
    expect(errorRow().textContent).toBe("");
  });

  it("leaves no element at the zero-length stoppers after the search field and each selector", async () => {
    // audit: COTRN00-016
    // audit: COTRN00-028
    // audit: COTRN00-034
    // audit: COTRN00-040
    // audit: COTRN00-046
    // audit: COTRN00-052
    // audit: COTRN00-058
    // audit: COTRN00-064
    // audit: COTRN00-070
    // audit: COTRN00-076
    // audit: COTRN00-082
    const sent = stubFetch(json(200, FIRST_PAGE));
    renderList();
    await answered(sent, 1);

    // POS=(6,38) ends the search field: the grid has no cell there.
    expect(document.querySelector(".row-start-6.col-start-38")).toBeNull();
    // POS=(10..19,5) end the selectors: no grid cell sits there, and each list line holds only
    // the selector's cell and the four value cells, the selector alone in its cell.
    listLines().forEach((line, index) => {
      expect(document.querySelector(`.row-start-${index + 10}.col-start-5`)).toBeNull();
      const cells = within(line).getAllByRole("cell");
      expect(cells).toHaveLength(5);
      expect(within(cells[0]).getByRole("textbox")).toBe(selector(index + 1));
    });
  });

  it.each([
    { shown: "ten transactions", page: FIRST_PAGE },
    { shown: "three transactions", page: SHORT_PAGE },
  ])("keeps the list on the map's columns and lines with $shown", async ({ page }) => {
    // audit: COTRN00-017
    // audit: COTRN00-018
    // audit: COTRN00-019
    // audit: COTRN00-020
    // audit: COTRN00-021
    // audit: COTRN00-022
    // audit: COTRN00-023
    // audit: COTRN00-024
    // audit: COTRN00-025
    // audit: COTRN00-026
    // audit: COTRN00-027
    const sent = stubFetch(json(200, page));
    renderList();
    await answered(sent, 1);

    // Fixed columns from the fields' `POS`: the selector column at 2, the transaction ID at 8,
    // the date at 27, the description at 38 and the amount at 67, to the list's end at 78.
    const table = screen.getByRole("table", { name: "Transactions" });
    expect(table.classList.contains("table-fixed")).toBe(true);
    const columns = listColumns(table, 2, 77);
    expect(columns.map((column) => column.start)).toEqual([2, 8, 27, 38, 67]);
    // Captions (line 8) and rules (line 9) start on their columns.
    const [captions, rules] = Array.from(table.querySelectorAll("thead > tr"));
    expect(shownColumns(captions, columns)).toEqual([2, 8, 27, 38, 67]);
    expect(shownColumns(rules, columns)).toEqual([2, 8, 27, 38, 67]);
    // Every selector at column 3, on every line, blank lines included.
    for (const row of Array.from({ length: 10 }, (_, index) => index + 1)) {
      expect(shownColumn(selector(row), columns.at(0))).toBe(3);
    }
    // Twelve rows of one compact map line each, whatever they hold, and no cell padding on the line.
    expect(table.querySelectorAll("tr")).toHaveLength(12);
    for (const row of table.querySelectorAll("tr")) {
      expect(row.classList.contains("h-7")).toBe(true);
    }
    for (const cell of table.querySelectorAll("th, td")) {
      expect(cell.classList.contains("px-0")).toBe(true);
      expect(cell.classList.contains("py-0")).toBe(true);
    }
  });

  it.each(Array.from({ length: 10 }, (_, index) => index + 1))(
    "S in row %i's selector opens that row's transaction at /transactions/view with no request",
    async (row) => {
      // audit: COTRN00-027
      // audit: COTRN00-033
      // audit: COTRN00-039
      // audit: COTRN00-045
      // audit: COTRN00-051
      // audit: COTRN00-057
      // audit: COTRN00-063
      // audit: COTRN00-069
      // audit: COTRN00-075
      // audit: COTRN00-081
      // audit: COTRN00-096
      // audit: COTRN00C-008
      const sent = stubFetch(json(200, FIRST_PAGE));
      renderList();
      await answered(sent, 1);

      expect(selector(row).getAttribute("maxlength")).toBe("1");
      typeInto(selector(row), "S");
      press("ENTER=Continue");

      await screen.findByRole("region", { name: "View Transaction" });
      expect(transferState("View Transaction")).toEqual({
        from: PATHS.COTRN00,
        tran_id: tranId(row),
        list: { first_key: tranId(1), last_key: tranId(10), page_no: 1, has_next_page: true },
      });
      // The selector is never sent: the selection only navigates.
      expect(sent).toHaveLength(1);
    },
  );

  it.each(["S", "s"])(
    "the first selector decides: %s in row 3 opens row 3's transaction despite later codes and a non-numeric search value",
    async (code) => {
      // audit: COTRN00C-007
      // audit: COTRN00C-008
      const sent = stubFetch(json(200, FIRST_PAGE));
      renderList();
      await answered(sent, 1);

      typeInto(searchField(), "ABC");
      typeInto(selector(3), code);
      typeInto(selector(5), "X");
      typeInto(selector(9), "S");
      press("ENTER=Continue");

      await screen.findByRole("region", { name: "View Transaction" });
      expect(transferState("View Transaction")).toEqual({
        from: PATHS.COTRN00,
        tran_id: tranId(3),
        list: { first_key: tranId(1), last_key: tranId(10), page_no: 1, has_next_page: true },
      });
      expect(sent).toHaveLength(1);
    },
  );

  it.each([
    {
      answer: "a page without a message",
      search: "",
      reply: json(200, FIRST_PAGE),
      shown: INVALID_SELECTION,
    },
    {
      answer: "a page with the end-of-file message",
      search: "",
      reply: json(200, SHORT_PAGE),
      shown: BOTTOM_REACHED,
    },
    {
      answer: "a 400 ScreenError",
      search: "ABC",
      reply: json(400, NOT_NUMERIC_ERROR),
      shown: NOT_NUMERIC,
    },
  ])(
    "an invalid code in the first selector sends ENTER and shows `$shown` after $answer, with no transfer",
    async ({ search, reply, shown }) => {
      // audit: COTRN00C-007
      // audit: COTRN00C-010
      const sent = stubFetch(json(200, FIRST_PAGE), reply);
      renderList();
      await answered(sent, 1);

      // Row 1's `X` comes first, so row 4's `S` is ignored. The invalid code does not stop the
      // ENTER: the search is sent, and a message of its answer replaces the selection message.
      typeInto(searchField(), search);
      typeInto(selector(1), "X");
      typeInto(selector(4), "S");
      press("ENTER=Continue");
      await answered(sent, 2);

      expect(sent[1].query).toEqual({
        ...(search === "" ? {} : { start_tran_id: search }),
        first_key: tranId(1),
        last_key: tranId(10),
        page_no: "1",
        has_next_page: "true",
      });
      expect(errorRow().textContent).toBe(shown);
      expect(screen.queryByRole("region", { name: "View Transaction" })).toBeNull();
      expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("List Transactions");
      expect(valueOf(selector(1))).toBe("X");
      expect(valueOf(selector(4))).toBe("S");
      expect(document.activeElement).toBe(searchField());
    },
  );

  it("ignores a code on a blank row: ENTER searches and shows no selection message", async () => {
    // audit: COTRN00C-009
    const reread: components["schemas"]["TranListResponse"] = { ...SHORT_PAGE, message: null };
    const sent = stubFetch(json(200, SHORT_PAGE), json(200, reread));
    renderList();
    await answered(sent, 1);
    expectListed(SHORT_PAGE);

    typeInto(selector(7), "S");
    press("ENTER=Continue");
    await answered(sent, 2);

    // The blank last key is left out of the echoed state.
    expect(sent[1].query).toEqual({ first_key: tranId(1), page_no: "1", has_next_page: "false" });
    expect(errorRow().textContent).toBe("");
    expect(screen.queryByRole("region", { name: "View Transaction" })).toBeNull();
    expectListed(SHORT_PAGE);
  });

  it("ENTER with no selection reads the page from the typed ID, echoing the state, and clears the search field", async () => {
    // audit: COTRN00-096
    // audit: COTRN00C-037
    const searched: components["schemas"]["TranListResponse"] = { ...SECOND_PAGE, page_no: 1 };
    const sent = stubFetch(json(200, FIRST_PAGE), json(200, searched));
    renderList();
    await answered(sent, 1);

    typeInto(searchField(), tranId(11));
    press("ENTER=Continue");
    await answered(sent, 2);

    expect(sent[1].query).toEqual({
      start_tran_id: tranId(11),
      first_key: tranId(1),
      last_key: tranId(10),
      page_no: "1",
      has_next_page: "true",
    });
    expectListed(searched);
    expect(valueOf(pageField())).toBe("00000001");
    expect(errorRow().textContent).toBe(BOTTOM_REACHED);
    expect(valueOf(searchField())).toBe("");
    expect(document.activeElement).toBe(searchField());
  });

  it("F8 and F7 page forward and backward with the keys echoed, keep the selectors, and never send page_size", async () => {
    // audit: COTRN00-098
    // audit: COTRN00-099
    // audit: COTRN00C-036
    // audit: COTRN00C-037
    // audit: COTRN00C-038
    const sent = stubFetch(
      json(200, FIRST_PAGE),
      json(200, SECOND_PAGE),
      json(200, BOTTOM_KEPT),
      json(200, FIRST_PAGE),
      json(200, TOP_KEPT),
    );
    renderList();
    await answered(sent, 1);
    typeInto(selector(4), "X");

    // F8 from the keyboard; the button pressed while the request runs sends nothing more, as the
    // terminal keyboard locks until the program answers.
    typeInto(searchField(), tranId(5));
    fireEvent.keyDown(searchField(), { key: "F8" });
    press("F8=Forward");
    await answered(sent, 2);
    expect(sent[1].query).toEqual({
      direction: "forward",
      first_key: tranId(1),
      last_key: tranId(10),
      page_no: "1",
      has_next_page: "true",
    });
    expectListed(SECOND_PAGE);
    expect(valueOf(pageField())).toBe("00000002");
    expect(errorRow().textContent).toBe(BOTTOM_REACHED);
    // A forward page clears the search field; the selectors stay as typed.
    expect(valueOf(searchField())).toBe("");
    expect(valueOf(selector(4))).toBe("X");
    expect(document.activeElement).toBe(searchField());

    // F8 at the bottom reads nothing: rows, page number and search value stay as shown.
    typeInto(searchField(), "123");
    press("F8=Forward");
    await answered(sent, 3);
    expect(sent[2].query).toEqual({
      direction: "forward",
      first_key: tranId(11),
      last_key: tranId(20),
      page_no: "2",
      has_next_page: "false",
    });
    expectListed(SECOND_PAGE);
    expect(valueOf(pageField())).toBe("00000002");
    expect(errorRow().textContent).toBe(ALREADY_BOTTOM);
    expect(valueOf(searchField())).toBe("123");

    // A backward page leaves the search field as typed.
    press("F7=Backward");
    await answered(sent, 4);
    expect(sent[3].query).toEqual({
      direction: "backward",
      first_key: tranId(11),
      last_key: tranId(20),
      page_no: "2",
      has_next_page: "false",
    });
    expectListed(FIRST_PAGE);
    expect(valueOf(pageField())).toBe("00000001");
    expect(errorRow().textContent).toBe("");
    expect(valueOf(searchField())).toBe("123");
    expect(valueOf(selector(4))).toBe("X");
    expect(document.activeElement).toBe(searchField());

    // F7 at the top, from the keyboard: nothing read.
    fireEvent.keyDown(searchField(), { key: "F7" });
    await answered(sent, 5);
    expect(sent[4].query).toEqual({
      direction: "backward",
      first_key: tranId(1),
      last_key: tranId(10),
      page_no: "1",
      has_next_page: "true",
    });
    expectListed(FIRST_PAGE);
    expect(valueOf(pageField())).toBe("00000001");
    expect(errorRow().textContent).toBe(ALREADY_TOP);
    expect(valueOf(searchField())).toBe("123");
    expect(valueOf(selector(4))).toBe("X");

    // Bounded Pagination: the screen never sends a page size.
    for (const request of sent) {
      expect(request.url).not.toContain("page_size");
    }
  });

  it("shows a 400 ScreenError in the error row, keeps the rows, page number and search value, and echoes page 0 next", async () => {
    // audit: COTRN00-088
    // audit: COTRN00C-036
    // audit: COTRN00C-037
    const afterReject: components["schemas"]["TranListResponse"] = { ...SECOND_PAGE, page_no: 1 };
    const sent = stubFetch(
      json(200, FIRST_PAGE),
      json(400, NOT_NUMERIC_ERROR),
      json(200, afterReject),
    );
    renderList();
    await answered(sent, 1);
    const message = errorRow();
    expect(message.getAttribute("aria-live")).toBe("polite");
    expect(message.textContent).toBe("");

    typeInto(searchField(), "12345");
    press("ENTER=Continue");
    await answered(sent, 2);
    expect(sent[1].query).toEqual({
      start_tran_id: "12345",
      first_key: tranId(1),
      last_key: tranId(10),
      page_no: "1",
      has_next_page: "true",
    });
    expect(errorRow().textContent).toBe(NOT_NUMERIC);
    expectListed(FIRST_PAGE);
    expect(valueOf(pageField())).toBe("00000001");
    expect(valueOf(searchField())).toBe("12345");
    expect(document.activeElement).toBe(searchField());

    // The rejected search reset the state's page number to 0, while the screen kept 00000001.
    press("F8=Forward");
    await answered(sent, 3);
    expect(sent[2].query).toEqual({
      direction: "forward",
      first_key: tranId(1),
      last_key: tranId(10),
      page_no: "0",
      has_next_page: "true",
    });
    expectListed(afterReject);
    expect(valueOf(pageField())).toBe("00000001");
    expect(errorRow().textContent).toBe(BOTTOM_REACHED);
  });

  // audit: COTRN00-088
  it.each([
    {
      clearance: "a correction, which sends nothing",
      act: () => {
        typeInto(searchField(), tranId(11));
      },
      replies: [],
      requests: 2,
      shown: NOT_NUMERIC,
    },
    {
      clearance: "the next page",
      act: () => {
        press("F8=Forward");
      },
      replies: [json(200, SECOND_PAGE)],
      requests: 3,
      shown: BOTTOM_REACHED,
    },
    {
      clearance: "the next failure, which names no field",
      act: () => {
        press("F7=Backward");
      },
      replies: [json(500, LOOKUP_ERROR)],
      requests: 3,
      shown: LOOKUP_FAILED,
    },
  ])(
    "a 400 ScreenError on start_tran_id marks the search field invalid with the server message until $clearance",
    async ({ act, replies, requests, shown }) => {
      const sent = stubFetch(json(200, FIRST_PAGE), json(400, NOT_NUMERIC_ERROR), ...replies);
      renderList();
      await answered(sent, 1);
      expect(searchField().getAttribute("aria-invalid")).toBe("false");

      typeInto(searchField(), "12AB");
      press("ENTER=Continue");
      await answered(sent, 2);
      expect(sent[1].query.start_tran_id).toBe("12AB");
      // The input is invalid and its description names the field message, which holds the
      // server's text for assistive technology only: the error row is the one place it shows.
      expect(searchField().getAttribute("aria-invalid")).toBe("true");
      const message = fieldMessage(searchField());
      expect(message?.textContent).toBe(NOT_NUMERIC);
      expect(message?.classList.contains("sr-only")).toBe(true);
      expectShownOnceInErrorRow(NOT_NUMERIC);
      expect(document.activeElement).toBe(searchField());

      act();
      await answered(sent, requests);
      await waitFor(() => {
        expect(searchField().getAttribute("aria-invalid")).toBe("false");
      });
      expect(fieldMessage(searchField())).toBeUndefined();
      // The error row keeps the server's message until the next answer replaces it.
      expect(errorRow().textContent).toBe(shown);
    },
  );

  // The replacement's accessibility wiring, which no audit row states: every `aria-describedby`
  // names only elements the screen renders. The map gives the search field and the ten selectors
  // no hint, so each is described by nothing while it is valid, and the search field a
  // `ScreenError` names by its field message, which holds the server's text, hidden.
  it.each([
    { state: "on entry", replies: [], search: null, described: null },
    {
      state: "after the 400 naming start_tran_id",
      replies: [json(400, NOT_NUMERIC_ERROR)],
      search: "12AB",
      described: [{ text: NOT_NUMERIC, hidden: true }],
    },
  ])(
    "describes each input by rendered elements only, $state",
    async ({ replies, search, described }) => {
      const sent = stubFetch(json(200, FIRST_PAGE), ...replies);
      renderList();
      await answered(sent, 1);
      if (search !== null) {
        typeInto(searchField(), search);
        press("ENTER=Continue");
        await answered(sent, 2);
        expect(errorRow().textContent).toBe(NOT_NUMERIC);
      }

      expect(danglingDescriptions()).toEqual([]);
      expect(readOut(searchField())).toEqual({
        invalid: search === null ? "false" : "true",
        described,
      });
      const selectors = Array.from({ length: 10 }, (_, index) => readOut(selector(index + 1)));
      expect(selectors).toEqual(Array(10).fill({ invalid: "false", described: null }));
    },
  );

  it.each([
    { failure: "a 500 ScreenError", reply: json(500, LOOKUP_ERROR), shown: LOOKUP_FAILED },
    {
      failure: "a 502 without a body",
      reply: () => new Response(null, { status: 502, statusText: "Bad Gateway" }),
      shown: "HTTP 502 Bad Gateway",
    },
  ])(
    "F7 answered by $failure shows `$shown`, keeps the page and search value, and echoes next-page flag Y next",
    async ({ reply, shown }) => {
      // audit: COTRN00C-037
      const sent = stubFetch(
        json(200, FIRST_PAGE),
        json(200, SECOND_PAGE),
        reply,
        json(200, BOTTOM_KEPT),
      );
      renderList();
      await answered(sent, 1);
      press("F8=Forward");
      await answered(sent, 2);

      typeInto(searchField(), "777");
      press("F7=Backward");
      await answered(sent, 3);
      expect(errorRow().textContent).toBe(shown);
      expectListed(SECOND_PAGE);
      expect(valueOf(pageField())).toBe("00000002");
      expect(valueOf(searchField())).toBe("777");
      expect(document.activeElement).toBe(searchField());

      // F7 sets the next-page flag before its read, so the failed read leaves it Y.
      press("F8=Forward");
      await answered(sent, 4);
      expect(sent[3].query).toEqual({
        direction: "forward",
        first_key: tranId(11),
        last_key: tranId(20),
        page_no: "2",
        has_next_page: "true",
      });
      expect(errorRow().textContent).toBe(ALREADY_BOTTOM);
    },
  );

  it.each([
    {
      trigger: "the F3=Back button",
      origin: "the menu",
      state: undefined,
      act: () => {
        press("F3=Back");
      },
    },
    {
      trigger: "the F3 key",
      origin: "the menu",
      state: undefined,
      act: () => {
        fireEvent.keyDown(searchField(), { key: "F3" });
      },
    },
    {
      trigger: "the F3=Back button",
      origin: "COTRN01",
      state: { from: PATHS.COTRN01, entry_key: "pf5" },
      act: () => {
        press("F3=Back");
      },
    },
  ])(
    "$trigger after entry from $origin returns to the main menu with no request",
    async ({ state, act }) => {
      // audit: COTRN00-097
      // audit: COTRN00C-005
      const sent = stubFetch(json(200, FIRST_PAGE));
      renderList(state);
      await answered(sent, 1);

      act();

      await screen.findByRole("region", { name: "Main Menu" });
      expect(transferState("Main Menu")).toBeNull();
      expect(sent).toHaveLength(1);
    },
  );

  it.each([
    { key: "F2", init: { key: "F2" } },
    { key: "Shift+F8", init: { key: "F8", shiftKey: true } },
  ])(
    "$key shows the invalid-key message with no request and keeps the screen and the state",
    async ({ init }) => {
      // audit: COTRN00C-006
      const sent = stubFetch(json(200, FIRST_PAGE), json(200, SECOND_PAGE));
      renderList();
      await answered(sent, 1);
      typeInto(searchField(), "ABC");
      typeInto(selector(2), "X");

      fireEvent.keyDown(searchField(), init);

      expect(errorRow().textContent).toBe(INVALID_KEY);
      expect(sent).toHaveLength(1);
      expectListed(FIRST_PAGE);
      expect(valueOf(pageField())).toBe("00000001");
      expect(valueOf(searchField())).toBe("ABC");
      expect(valueOf(selector(2))).toBe("X");
      expect(document.activeElement).toBe(searchField());

      // The next key echoes the state page 1 left.
      press("F8=Forward");
      await answered(sent, 2);
      expect(sent[1].query).toEqual({
        direction: "forward",
        first_key: tranId(1),
        last_key: tranId(10),
        page_no: "1",
        has_next_page: "true",
      });
    },
  );

  // How each route state is read on entry. COTRN00C-002 is tagged on the round trip below, whose
  // `list` comes back through the view's hand-back rather than being written by the test.
  it.each([
    {
      entry: "no route state",
      state: undefined,
      query: { entry_key: "enter", page_no: "0", has_next_page: "false" },
    },
    {
      entry: "the menu",
      state: { from: PATHS.COMEN01 },
      query: { entry_key: "enter", page_no: "0", has_next_page: "false" },
    },
    {
      entry: "an unknown entry key",
      state: { entry_key: "pf9" },
      query: { entry_key: "enter", page_no: "0", has_next_page: "false" },
    },
    {
      entry: "COTRN01's F3",
      state: { from: PATHS.COTRN01, entry_key: "pf3" },
      query: { entry_key: "pf3", page_no: "0", has_next_page: "false" },
    },
    {
      entry: "COTRN01 without an entry key",
      state: { from: PATHS.COTRN01 },
      query: { entry_key: "pf5", page_no: "0", has_next_page: "false" },
    },
    {
      entry: "COTRN01's F5 with the list state",
      state: {
        from: PATHS.COTRN01,
        entry_key: "pf5",
        list: { first_key: tranId(11), last_key: tranId(20), page_no: 2, has_next_page: true },
      },
      query: {
        entry_key: "pf5",
        first_key: tranId(11),
        last_key: tranId(20),
        page_no: "2",
        has_next_page: "true",
      },
    },
    {
      entry: "a list state with blank keys",
      state: {
        entry_key: "pf5",
        list: { first_key: null, page_no: 0, has_next_page: false },
      },
      query: { entry_key: "pf5", page_no: "0", has_next_page: "false" },
    },
    {
      entry: "a malformed list state",
      state: {
        entry_key: "pf5",
        list: { first_key: 11, last_key: tranId(20), page_no: 1.5, has_next_page: "Y" },
      },
      query: { entry_key: "pf5", page_no: "0", has_next_page: "false" },
    },
  ])(
    "entry from $entry sends its entry key and echoes the state it carries",
    async ({ state, query }) => {
      const sent = stubFetch(json(200, FIRST_PAGE));
      renderList(state);
      await answered(sent, 1);

      expect(sent[0].query).toEqual(query);
      expectListed(FIRST_PAGE);
      expect(valueOf(pageField())).toBe("00000001");
    },
  );

  it.each([
    { key: "F3=Back", entryKey: "pf3" },
    { key: "F5=Browse Tran.", entryKey: "pf5" },
  ])(
    "keeps its paging state across the round trip to /transactions/view and back by the view's $key",
    async ({ key, entryKey }) => {
      // audit: COTRN00C-002
      const sent = stubFetch(json(200, FIRST_PAGE), json(200, SECOND_PAGE));
      renderList(undefined, <ViewHandBackStub />);
      await answered(sent, 1);

      // Page 1 leaves keys 1 and 10, page 1 and the next-page flag Y: each differs from the
      // blank state a menu entry starts from.
      typeInto(selector(3), "S");
      press("ENTER=Continue");
      await screen.findByRole("region", { name: "View Transaction" });
      expect(sent).toHaveLength(1);

      press(key);

      await answered(sent, 2);
      expect(sent.map(({ path }) => path)).toEqual([LIST_PATH, LIST_PATH]);
      // Back on the list, the entry request echoes the state the list held when it left.
      expect(sent[1].query).toEqual({
        entry_key: entryKey,
        first_key: tranId(1),
        last_key: tranId(10),
        page_no: "1",
        has_next_page: "true",
      });
      expectListed(SECOND_PAGE);
      expect(valueOf(pageField())).toBe("00000002");
    },
  );

  // The map lengths stop typing beyond them; a longer value set programmatically is refused
  // by the form schema before any request.
  it.each([
    {
      field: "the search field",
      target: searchField,
      value: "0".repeat(17),
      shown: "String must contain at most 16 character(s)",
    },
    {
      field: "row 2's selector",
      target: () => selector(2),
      value: "SS",
      shown: "String must contain at most 1 character(s)",
    },
  ])(
    "ENTER with more characters in $field than the map allows sends nothing and marks the field invalid with the schema message",
    async ({ target, value, shown }) => {
      const sent = stubFetch(json(200, FIRST_PAGE));
      renderList();
      await answered(sent, 1);
      expect(target().getAttribute("aria-invalid")).toBe("false");
      expect(fieldMessage(target())).toBeUndefined();

      typeInto(target(), value);
      press("ENTER=Continue");

      await waitFor(() => {
        expect(errorRow().textContent).toBe(shown);
      });
      expect(sent).toHaveLength(1);
      expect(screen.queryByRole("region", { name: "View Transaction" })).toBeNull();
      // The refusal releases the keyboard it took for the ENTER.
      expect(lockState()).toEqual(UNLOCKED);
      // The field's message, which its description names, holds the schema text for assistive
      // technology only; the error row is the one place it shows.
      expect(target().getAttribute("aria-invalid")).toBe("true");
      const message = fieldMessage(target());
      expect(message?.textContent).toBe(shown);
      expect(message?.classList.contains("sr-only")).toBe(true);
      expectShownOnceInErrorRow(shown);
    },
  );

  // The terminal keyboard stays locked until the program answers: no key acts on the screen, and
  // a selection typed beforehand is acted on only against the page that answer shows.
  it.each([
    {
      title: "the entry read",
      before: [],
      start: () => undefined,
      page: FIRST_PAGE,
      list: { first_key: tranId(1), last_key: tranId(10), page_no: 1, has_next_page: true },
    },
    {
      title: "an ENTER search by its button",
      before: [json(200, FIRST_PAGE)],
      start: () => {
        typeInto(searchField(), tranId(11));
        press("ENTER=Continue");
      },
      page: SECOND_PAGE,
      list: { first_key: tranId(11), last_key: tranId(20), page_no: 2, has_next_page: false },
    },
    {
      title: "an F8 page by its key",
      before: [json(200, FIRST_PAGE)],
      start: () => {
        fireEvent.keyDown(document.body, { key: "F8" });
      },
      page: SECOND_PAGE,
      list: { first_key: tranId(11), last_key: tranId(20), page_no: 2, has_next_page: false },
    },
  ])(
    "while $title is outstanding, ENTER, F7, F8 and a row's S send and open nothing; the answer opens the S row",
    async (row) => {
      const held = heldReply(json(200, row.page));
      const sent = stubFetch(...row.before, held.reply);
      const { client } = renderList();
      try {
        if (row.before.length > 0) {
          await answered(sent, row.before.length);
        }
        row.start();
        await waitFor(() => {
          expect(sent).toHaveLength(row.before.length + 1);
          expect(lockState()).toEqual(LOCKED);
        });

        // The read-only selector takes no typing; a value set past it is still not acted on. Once
        // ENTER has submitted the form, it re-validates on every change, so the change runs inside
        // an awaited `act`, where the render of that validation lands.
        await act(async () => {
          typeInto(selector(3), "S");
        });
        press("ENTER=Continue");
        press("ENTER=Continue");
        fireEvent.keyDown(document.body, { key: "Enter" });
        fireEvent.submit(screenForm());
        fireEvent.submit(screenForm());
        press("F7=Backward");
        press("F8=Forward");
        fireEvent.keyDown(document.body, { key: "F7" });
        fireEvent.keyDown(document.body, { key: "F8" });
        fireEvent.keyDown(document.body, { key: "F2" });
        // The held page stays the one request pending.
        await idleAfter50ms(client, 1);

        expect(sent).toHaveLength(row.before.length + 1);
        expect(screen.queryByRole("region", { name: "View Transaction" })).toBeNull();
        expect(errorRow().textContent).toBe("");
        expect(lockState()).toEqual(LOCKED);

        held.release();
        await answered(sent, row.before.length + 1);
        expectListed(row.page);
        expect(lockState()).toEqual(UNLOCKED);
        expect(document.activeElement).toBe(searchField());

        // Answered, ENTER acts on the selection kept in row 3, against the page now shown.
        press("ENTER=Continue");
        await screen.findByRole("region", { name: "View Transaction" });
        expect(transferState("View Transaction")).toEqual({
          from: PATHS.COTRN00,
          tran_id: row.page.rows[2]?.tran_id,
          list: row.list,
        });
        expect(sent).toHaveLength(row.before.length + 1);
      } finally {
        // Pass or fail, the held page is answered and the test ends once no request is pending,
        // so neither the request nor the client's deadline outlives the test.
        held.release();
        await clientIdle(client);
      }
    },
  );

  it("takes one ENTER of several pressed before the screen renders again", async () => {
    const held = heldReply(json(200, SECOND_PAGE));
    const sent = stubFetch(json(200, FIRST_PAGE), held.reply);
    const { client } = renderList();
    try {
      await answered(sent, 1);
      typeInto(searchField(), tranId(11));

      // One batch: React renders the lock only after all four, so only the screen's ref stops them.
      act(() => {
        const enter = screen.getByRole("button", { name: "ENTER=Continue" });
        enter.click();
        enter.click();
        screenForm().requestSubmit();
        document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      });
      await waitFor(() => {
        expect(sent).toHaveLength(2);
      });
      expect(lockState()).toEqual(LOCKED);
      // The held page of the one ENTER taken stays the one request pending.
      await idleAfter50ms(client, 1);
      expect(sent).toHaveLength(2);

      held.release();
      await answered(sent, 2);
      expectListed(SECOND_PAGE);
      expect(lockState()).toEqual(UNLOCKED);
    } finally {
      // Pass or fail, the held page is answered and the test ends once no request is pending, so
      // neither the request nor the client's deadline outlives the test.
      held.release();
      await clientIdle(client);
    }
  });

  it("F3 still returns to the main menu while a page is outstanding", async () => {
    const held = heldReply(json(200, FIRST_PAGE));
    const sent = stubFetch(held.reply);
    const { client } = renderList();
    try {
      await waitFor(() => {
        expect(lockState()).toEqual(LOCKED);
      });

      press("F3=Back");

      await screen.findByRole("region", { name: "Main Menu" });
      held.release();
      // The count waits for the released page's answer.
      await clientIdle(client);
      expect(sent).toHaveLength(1);
    } finally {
      // Pass or fail, the held page is answered and the test ends once no request is pending, so
      // neither the request nor the client's deadline outlives the test.
      held.release();
      await clientIdle(client);
    }
  });
});

/** The 401 of the `SignedOnUser` extractor for a missing, invalid or expired token. */
const SIGNON_REQUIRED: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};

/**
 * Renders the application's route table at `/transactions`, so the sign-on guard that only the
 * table mounts stands in front of the screen. Each screen is imported by its route's loader, so a
 * wait for one to render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COTRN00] });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

// A 401 clears the stored session (`api/client.ts`), and the sign-on guard in front of the route
// replaces the still-mounted screen with sign-on at once: after the list request the screen sent,
// the test presses no key and follows no link.
describe("COTRN00 in the route table: a 401 ends the session", () => {
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
        await answered(sent, 1);
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
      expect(screen.queryByRole("heading", { level: 1, name: "List Transactions" })).toBeNull();
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

// The sign-on guard lives in the application's route tree, not in the screen, so this group
// renders that tree rather than the screen beside stubs.
describe("COTRN00 behind the route guard", () => {
  // audit: COTRN00C-001
  it("sends a visitor with no session from /transactions to /signon, with no request", async () => {
    clearSession();
    const sent = stubFetch();
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, { initialEntries: [PATHS.COTRN00] });
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
    expect(screen.queryByRole("heading", { level: 1, name: "List Transactions" })).toBeNull();
    expect(screen.queryByLabelText("Search Tran ID:")).toBeNull();
    // The refused screen never mounts, so nothing on screen marks the end of a wait: a bounded
    // window, then no request in flight. The count covers only requests sent within that window.
    await idleAfter50ms(client);
    expect(sent).toHaveLength(0);
    router.dispose();
  });
});

/** The screen title of the frame the route table shows for a route that failed. */
const UNAVAILABLE_TITLE = "Screen Unavailable";

/** That frame's error row. */
const UNAVAILABLE = "Unable to load the screen. Press ENTER to try again or F3 to go back.";

/**
 * Replaces the module of screen `name` with one whose import fails, as a browser's import of a
 * screen's chunk fails while the network is down, until `restore` is called; from then on each
 * import gives the real module. `attempts` counts the imports tried so far.
 */
function failingImport(name: string): { attempts: () => number; restore: () => void } {
  let failing = true;
  let attempts = 0;
  vi.doMock(`./${name}`, async () => {
    attempts += 1;
    if (failing) {
      throw new TypeError(`Failed to fetch dynamically imported module: /src/screens/${name}.tsx`);
    }
    return vi.importActual(`./${name}`);
  });
  return {
    attempts: () => attempts,
    restore: () => {
      failing = false;
    },
  };
}

/**
 * Asserts the frame shown in place of a screen that could not be loaded: the shared `ScreenShell`
 * headed with the failed route's transaction and program, the frame's title and error row, and
 * the keys ENTER=Retry and F3=Back.
 */
function expectUnavailableFrame(transaction: string, program: string): void {
  expect(screen.getByRole("heading", { level: 1, name: UNAVAILABLE_TITLE })).toBeDefined();
  expect(headerField("TRNNAME")).toBe(transaction);
  expect(headerField("PGMNAME")).toBe(program);
  expect(errorRow().textContent).toBe(UNAVAILABLE);
  expect(
    within(screen.getByRole("group", { name: "Function keys" }))
      .getAllByRole("button")
      .map((button) => button.textContent),
  ).toEqual(["ENTER=Retry", "F3=Back"]);
  expect(screen.queryByText(/Unexpected Application Error/)).toBeNull();
}

/**
 * Waits inside `act` until `router` has finished its first load, which the route's loader ends
 * only after `render` returns. The frame or screen that load shows is then rendered and its effects
 * have run, among them the binding of its function keys to the window, so a key the test presses
 * next reaches it. Outside `act` those effects can still be pending once the frame is in the
 * document, and a key pressed then is lost.
 */
async function firstLoadDone(router: ReturnType<typeof createMemoryRouter>): Promise<void> {
  await act(async () => {
    await vi.waitUntil(() => router.state.initialized, { timeout: 5_000 });
  });
}

// A screen's module is imported by its route's loader, which keeps no failed import: a screen whose
// import failed shows the root route's frame inside `ScreenShell`, and the next navigation to it,
// by the frame's keys or otherwise, imports the module again. Each wait for a route's loader to
// render its screen or the frame allows 5 s, as one for a screen of the route table does: a module
// this file has not imported yet, such as a menu, is transformed on its first import. The first
// load is awaited with `firstLoadDone`, so the frame's keys are bound before one is pressed.
describe("COTRN00 in the route table: a module that cannot be loaded", () => {
  afterEach(() => {
    vi.doUnmock("./COTRN00");
    vi.doUnmock("./COSGN00");
  });

  it.each([
    {
      how: "a click on ENTER=Retry imports it again in place, with the same route state",
      recover: async () => {
        press("ENTER=Retry");
      },
      entryKey: "pf3",
      count: 1,
      action: "REPLACE",
    },
    {
      how: "the Enter key imports it again in place, with the same route state",
      recover: async () => {
        fireEvent.keyDown(document.body, { key: "Enter" });
      },
      entryKey: "pf3",
      count: 1,
      action: "REPLACE",
    },
    {
      how: "F3=Back to the menu and a new visit from there import it again",
      recover: async (router: ReturnType<typeof createMemoryRouter>) => {
        press("F3=Back");
        expect(
          await screen.findByRole("heading", { level: 1, name: "Main Menu" }, { timeout: 5_000 }),
        ).toBeDefined();
        await act(async () => {
          await router.navigate(PATHS.COTRN00, { state: { from: PATHS.COMEN01 } });
        });
      },
      entryKey: "enter",
      // The menu's own load, endpoint 2, answered 500 by the stub, comes before the list.
      count: 2,
      action: "PUSH",
    },
  ])(
    "a failed import shows the frame, not the screen, and $how",
    async ({ recover, entryKey, count, action }) => {
      const module = failingImport("COTRN00");
      const sent = stubFetch(json(200, FIRST_PAGE));
      const client = new QueryClient({
        defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
      });
      const router = createMemoryRouter(routes, {
        initialEntries: [
          { pathname: PATHS.COTRN00, state: { from: PATHS.COTRN01, entry_key: "pf3" } },
        ],
      });
      render(
        <QueryClientProvider client={client}>
          <RouterProvider router={router} />
        </QueryClientProvider>,
      );

      await firstLoadDone(router);

      expect(
        await screen.findByRole("heading", { level: 1, name: UNAVAILABLE_TITLE }),
      ).toBeDefined();
      expectUnavailableFrame("CT00", "COTRN00C");
      expect(router.state.location.pathname).toBe(PATHS.COTRN00);
      expect(screen.queryByRole("heading", { level: 1, name: "List Transactions" })).toBeNull();
      expect(module.attempts()).toBe(1);
      expect(sent).toHaveLength(0);

      module.restore();
      await recover(router);

      expect(
        await screen.findByRole(
          "heading",
          { level: 1, name: "List Transactions" },
          { timeout: 5_000 },
        ),
      ).toBeDefined();
      await answered(sent, count);
      expect([sent.at(-1)?.path, sent.at(-1)?.query]).toEqual([
        LIST_PATH,
        { entry_key: entryKey, page_no: "0", has_next_page: "false" },
      ]);
      expect(router.state.location.pathname).toBe(PATHS.COTRN00);
      expect(router.state.historyAction).toBe(action);
      expect(module.attempts()).toBe(2);
      expect(screen.queryByRole("heading", { level: 1, name: UNAVAILABLE_TITLE })).toBeNull();
      router.dispose();
    },
  );

  it.each([
    {
      who: "USER0001, user type U",
      install: () => {
        setSession(TOKEN, "USER0001", "U");
      },
      name: "COTRN00",
      path: PATHS.COTRN00,
      header: ["CT00", "COTRN00C"],
      target: PATHS.COMEN01,
      landmark: { role: "heading", name: "Main Menu" },
    },
    {
      who: "ADMIN001, user type A",
      install: () => {
        setSession(TOKEN, "ADMIN001", "A");
      },
      name: "COTRN00",
      path: PATHS.COTRN00,
      header: ["CT00", "COTRN00C"],
      target: PATHS.COADM01,
      landmark: { role: "heading", name: "Admin Menu" },
    },
    {
      who: "nobody signed on",
      install: () => {
        clearSession();
      },
      name: "COSGN00",
      path: PATHS.COSGN00,
      header: ["CC00", "COSGN00C"],
      target: PATHS.COSGN00,
      landmark: { role: "button", name: "ENTER=Sign-on" },
    },
  ])(
    "the F3 key on the frame of $name for $who goes to $target, the menu sign-on opens for that user",
    async ({ install, name, path, header, target, landmark }) => {
      install();
      const module = failingImport(name);
      stubFetch();
      const client = new QueryClient({
        defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
      });
      const router = createMemoryRouter(routes, { initialEntries: [path] });
      render(
        <QueryClientProvider client={client}>
          <RouterProvider router={router} />
        </QueryClientProvider>,
      );

      await firstLoadDone(router);

      expect(
        await screen.findByRole("heading", { level: 1, name: UNAVAILABLE_TITLE }),
      ).toBeDefined();
      expectUnavailableFrame(header[0], header[1]);

      module.restore();
      fireEvent.keyDown(document.body, { key: "F3" });

      expect(
        await screen.findByRole(landmark.role, { name: landmark.name }, { timeout: 5_000 }),
      ).toBeDefined();
      expect(router.state.location.pathname).toBe(target);
      expect(screen.queryByRole("heading", { level: 1, name: UNAVAILABLE_TITLE })).toBeNull();
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

// An `S` selection leaves the list on screen until COTRN01's module has loaded. The lock ENTER
// took holds through that wait, as the terminal keyboard stays locked until the next program
// sends its map, so no key pages the list or selects again meanwhile. The screen sits in a data
// router without the application's root element, so only the screen's own lock acts.
describe("COTRN00 while the transaction view loads", () => {
  it("after S on row 3 and ENTER the keyboard stays locked until COTRN01 replaces the list, and opens row 3", async () => {
    const sent = stubFetch(json(200, FIRST_PAGE), json(200, SECOND_PAGE));
    const slow = slowRoute(() => <TransferStub name="View Transaction" />);
    const screens: RouteObject[] = [
      { path: PATHS.COTRN00, element: <COTRN00 /> },
      { path: PATHS.COTRN01, lazy: slow.lazy },
      { path: PATHS.COMEN01, element: <TransferStub name="Main Menu" /> },
    ];
    const router = createMemoryRouter(screens, { initialEntries: [PATHS.COTRN00] });
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
    await answered(sent, 1);

    typeInto(selector(3), "S");
    press("ENTER=Continue");
    await waitFor(() => {
      expect(router.state.navigation.state).toBe("loading");
      expect(screenForm().getAttribute("aria-busy")).toBe("true");
    });
    expect(screen.getByRole("group", { name: "Function keys" }).getAttribute("aria-busy")).toBe(
      "true",
    );
    expect(lockState().readOnly).toEqual(Array.from({ length: 11 }, () => true));

    // F8 and F7 would page, F2 would show the invalid-key message, and a second selection with
    // ENTER would open another transaction.
    press("F8=Forward");
    press("F7=Backward");
    for (const key of ["F8", "F7", "F2", "Enter"]) {
      fireEvent.keyDown(window, { key });
    }
    // ENTER has submitted the form, so it re-validates on this change: the change runs inside an
    // awaited `act`, where the render of that validation lands.
    await act(async () => {
      typeInto(selector(5), "S");
    });
    press("ENTER=Continue");
    fireEvent.submit(screenForm());
    await idleAfter50ms(client);

    expect(sent).toHaveLength(1);
    expect(router.state.location.pathname).toBe(PATHS.COTRN00);
    expect(errorRow().textContent).toBe("");

    slow.load();
    await screen.findByRole("region", { name: "View Transaction" });
    expect(router.state.location.pathname).toBe(PATHS.COTRN01);
    expect(transferState("View Transaction")).toEqual({
      from: PATHS.COTRN00,
      tran_id: tranId(3),
      list: { first_key: tranId(1), last_key: tranId(10), page_no: 1, has_next_page: true },
    });
    expect(sent).toHaveLength(1);
    router.dispose();
  });
});
