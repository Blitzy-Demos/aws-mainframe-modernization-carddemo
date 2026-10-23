import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, configure, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import {
  createMemoryRouter,
  MemoryRouter,
  Route,
  RouterProvider,
  Routes,
  useLocation,
} from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { components } from "@/api/schema";
import { getSession, setSession } from "@/auth/session";
import { PATHS, routes } from "@/routes";
import { COPAU00 } from "./COPAU00";

/**
 * Render test of map `COPAU0A` (transaction CPVS, program COPAUS0C) at `/authorizations`, run in
 * jsdom with the global `fetch` stubbed, so no request leaves the test (Local-Only Runtime).
 * Endpoint 21, `GET /api/v1/accounts/{acct_id}/authorizations`, is the only operation the screen
 * calls; any other request is answered 500 and recorded, and every test asserts how many requests
 * were sent. `AuthSummaryResponse` carries no card number (COPAUS0C-055), so no fixture holds
 * cardholder data (Cardholder Data Redaction).
 *
 * Expectations come from the `client` rows of `modern/audit/online/COPAUS0C.md` and
 * `modern/audit/screens.md`. Rows `COPAU00-001` to `COPAU00-010` place the header labels in a
 * `Label` and the header values in the read-only `Input`s the shared `ScreenShell` renders in the
 * `CardHeader` title rows, skipped by Tab. NAV proves the label rows. {@link expectHeaderValues}
 * proves the six value rows (`COPAU00-002`, `-003`, `-005`, `-007`, `-008`, `-010`): their
 * placement, read-only state and width; their content rows (`COPAU00-105` to `COPAU00-110`) are
 * proved by the values those inputs hold. The rows blocked by halts `X-COPAUS0C-H3` and
 * `X-COPAUS0C-H4` are tested in their own `BLOCKED` groups, which assert only the replacement's
 * path the rows state, never a COBOL result.
 */

/*
 * The paging scenarios drive the real screen through up to 22 request round-trips in jsdom. On a
 * shared host under load one of them can outlast Vitest's 5-second default and a single answer
 * the 1-second default of `waitFor`, so both limits are raised for this file only. A screen that
 * never answers still fails, at the raised limit.
 */
vi.setConfig({ testTimeout: 30_000 });
configure({ asyncUtilTimeout: 10_000 });

const TOKEN = "test-token";

/** The account the scenarios read; its customer is 000000050. */
const ACCOUNT = "00000000050";
/** An account with a customer but no pending-authorization summary (COPAUS0C-029). */
const NO_SUMMARY_ACCOUNT = "00000000051";
/** An account no cross-reference row holds (COPAUS0C-014). */
const UNKNOWN_ACCOUNT = "00000000099";

/**
 * Header rows 1 and 2: `CCDA-TITLE01` and `CCDA-TITLE02`, 40 bytes each with their padding
 * (`app/cpy/COTTL01Y.cpy:18-22`; COPAU00-106, COPAU00-109). Written out here rather than imported
 * from the screen's own header module, so the test checks the audited value, not itself.
 */
const TITLE01 = "      AWS Mainframe Modernization       ";
const TITLE02 = "              CardDemo                  ";

/**
 * COPAUS0C-004: `CCDA-MSG-INVALID-KEY PIC X(50)` (`app/cpy/CSMSG01Y.cpy:20-21`), the literal
 * padded with spaces to all 50 bytes, as the `MOVE` transfers it.
 */
const INVALID_KEY = "Invalid key pressed. Please see below...".padEnd(50, " ");

/** The screen's messages, as `modern/audit/online/COPAUS0C.md` words them. */
const INVALID_SELECTION = "Invalid selection. Valid value is S";
const TOP_OF_PAGE = "You are already at the top of the page...";
const BOTTOM_OF_PAGE = "You are already at the bottom of the page...";
const ENTER_ACCT_ID = "Please enter Acct Id...";
const ACCT_ID_NOT_NUMERIC = "Acct Id must be Numeric ...";

/**
 * COPAUS0C-014, 73 characters. The two 9-digit codes are halt H4 (COPAUS0C-056): the values here
 * are stand-ins the screen shows verbatim, not parity values.
 */
const XREF_NOT_FOUND =
  `Account:${UNKNOWN_ACCOUNT} not found in XREF file. Resp:000000013 Reas:000000001`;

/** COPAUS0C-038, 48 characters with its leading space; the IMS status `AI` is a halt-H4 value. */
const REPOS_FAILED = " System error while repos. AUTH Details: Code:AI";

/** The path of endpoint 21 for `account`. */
function summaryPath(account: string): string {
  return `/api/v1/accounts/${account}/authorizations`;
}

/** Authorization key `n` as the 16 digits endpoint 21 returns (COPAUS0C-034). */
function authKey(n: number): string {
  return String(n).padStart(16, "0");
}

/**
 * The display columns of the five row shapes the pages cycle through, each as COPAUS0C-032 to
 * COPAUS0C-034 build them: `MM/DD/YY` and `HH:MM:SS` strings of 8 characters (`13/99/24` is the
 * audited date the program builds with no calendar check), and the amount through
 * `-zzzzzzz9.99`, 12 characters.
 */
const ROW_SHAPES = [
  {
    date: "01/31/24",
    time: "13:05:09",
    auth_type: "PURC",
    approval: "A",
    match_status: "P",
    amount: "      100.00",
  },
  {
    date: "12/31/23",
    time: "00:00:00",
    auth_type: "CASH",
    approval: "A",
    match_status: "M",
    amount: "     2500.50",
  },
  {
    date: "13/99/24",
    time: "23:59:59",
    auth_type: "PURC",
    approval: "D",
    match_status: "D",
    amount: "        0.00",
  },
  {
    date: "02/29/24",
    time: "07:30:00",
    auth_type: "REFD",
    approval: "A",
    match_status: "E",
    amount: "-      45.10",
  },
  {
    date: "06/15/22",
    time: "18:00:01",
    auth_type: "PURC",
    approval: "A",
    match_status: "P",
    amount: " 99999999.99",
  },
] as const;

/** Authorizations `first` to `first + count - 1` of ACCOUNT, in key order. */
function authRows(first: number, count: number): components["schemas"]["AuthSummaryRow"][] {
  return Array.from({ length: count }, (_, index) => {
    const n = first + index;
    return {
      transaction_id: `TX*********${String(n).padStart(4, "0")} `,
      auth_key: authKey(n),
      ...ROW_SHAPES[(n - 1) % ROW_SHAPES.length],
    };
  });
}

/** The block of ACCOUNT: customer 000000050, every figure as its picture edits it. */
const BLOCK: components["schemas"]["AuthSummaryBlock"] = {
  summary_found: true,
  customer_name: "Jane Q Public",
  customer_id: "000000050",
  address_line_1: "100 Main Street",
  address_line_2: "Springfield IL 62701",
  phone: "(555)010-0050",
  approved_count: "004",
  declined_count: "001",
  credit_limit: "     8500.00",
  cash_limit: "  3400.00",
  approved_amount: "  2645.50 ",
  credit_balance: "      450.00",
  cash_balance: "     0.00",
  declined_amount: "     0.00 ",
};

/**
 * The block endpoint 21 returns with the F7 and F8 pages, after another authorization reached
 * the store: the screen keeps showing BLOCK (COPAUS0C-048).
 */
const UPDATED_BLOCK: components["schemas"]["AuthSummaryBlock"] = {
  ...BLOCK,
  customer_name: "Jane Q Public-Smith",
  approved_count: "005",
  approved_amount: "  2745.50 ",
  credit_balance: "      550.00",
};

/** The block of NO_SUMMARY_ACCOUNT: the figurative `ZERO` fills each figure (COPAUS0C-029). */
const NO_SUMMARY_BLOCK: components["schemas"]["AuthSummaryBlock"] = {
  summary_found: false,
  customer_name: "John R Doe",
  customer_id: "000000051",
  address_line_1: "7 Elm Road",
  address_line_2: "Dayton OH 45402",
  phone: "(555)010-0051",
  approved_count: "000",
  declined_count: "000",
  credit_limit: "     1200.00",
  cash_limit: "   500.00",
  approved_amount: "0000000000",
  credit_balance: "000000000000",
  cash_balance: "000000000",
  declined_amount: "0000000000",
};

/** Page `n` of ACCOUNT: authorizations 5n−4 to 5n, a full page with next-page flag `hasNext`. */
function fullPage(
  n: number,
  hasNext: boolean,
  summary: components["schemas"]["AuthSummaryBlock"] = BLOCK,
): components["schemas"]["AuthSummaryResponse"] {
  const rows = authRows(5 * n - 4, 5);
  return {
    acct_id: ACCOUNT,
    summary,
    rows,
    first_key: rows[0].auth_key,
    last_key: rows[4].auth_key,
    has_next_page: hasNext,
  };
}

/** The first page of ACCOUNT, which another authorization follows. */
const PAGE_1: components["schemas"]["AuthSummaryResponse"] = fullPage(1, true);

/** Page 2, the last: five rows and no authorization after them. */
const PAGE_2: components["schemas"]["AuthSummaryResponse"] = fullPage(2, false, UPDATED_BLOCK);

/** A page of two rows after a full page: the read ended early, so no flag (COPAUS0C-036). */
const SHORT_PAGE: components["schemas"]["AuthSummaryResponse"] = {
  acct_id: ACCOUNT,
  summary: UPDATED_BLOCK,
  rows: authRows(6, 2),
  first_key: authKey(6),
  last_key: authKey(7),
  has_next_page: null,
};

/** A forward read that finds nothing after its key (COPAUS0C-041, COPAUS0C-042). */
const EMPTY_PAGE: components["schemas"]["AuthSummaryResponse"] = {
  acct_id: ACCOUNT,
  summary: UPDATED_BLOCK,
  rows: [],
  first_key: null,
  last_key: null,
  has_next_page: null,
};

/** ENTER on an account without a summary: the block and no rows (COPAUS0C-029). */
const NO_SUMMARY: components["schemas"]["AuthSummaryResponse"] = {
  acct_id: NO_SUMMARY_ACCOUNT,
  summary: NO_SUMMARY_BLOCK,
  rows: [],
  first_key: null,
  last_key: null,
  has_next_page: null,
};

/** The 404 of an account no cross-reference row holds (COPAUS0C-014). */
const NOT_FOUND: components["schemas"]["ScreenError"] = {
  message: XREF_NOT_FOUND,
  field: "acct_id",
};

/** The 500 of a failed repositioning read (COPAUS0C-038). */
const REPOS_ERROR: components["schemas"]["ScreenError"] = { message: REPOS_FAILED };

/** A reply with a JSON body, as the API sends every page and every `ScreenError`. */
function json(status: number, body: unknown): () => Response {
  return () =>
    new Response(JSON.stringify(body), {
      status,
      headers: { "Content-Type": "application/json" },
    });
}

/**
 * Replaces the global `fetch`. Each `GET` of endpoint 21 takes the next of `replies`; any other
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
      const isSummary =
        req.method === "GET" && /^\/api\/v1\/accounts\/[^/]+\/authorizations$/.test(url.pathname);
      const reply = isSummary ? pending.shift() : undefined;
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

/** Renders the screen at `/authorizations` with `state`, beside stubs at its transfer targets. */
function renderScreen(state?: unknown) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COPAU00, state }]}>
        <Routes>
          <Route path={PATHS.COPAU00} element={<COPAU00 />} />
          <Route path={PATHS.COPAU01} element={<TransferStub name="Authorization Details" />} />
          <Route path={PATHS.COMEN01} element={<TransferStub name="Main Menu" />} />
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

/** Waits until `count` requests were sent and the screen shows the answer to the last one. */
async function answered(sent: readonly unknown[], count: number): Promise<void> {
  await waitFor(() => {
    expect(sent).toHaveLength(count);
    expect(document.querySelector("form")?.getAttribute("aria-busy")).toBe("false");
  });
}

/** Lets pending tasks run, so a request the screen should not send would have been recorded. */
async function idle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 20));
}

/** The value of an `<input>`. */
function valueOf(element: HTMLElement): string {
  if (!(element instanceof HTMLInputElement)) {
    throw new TypeError(`${element.tagName} is not an input`);
  }
  return element.value;
}

function accountField(): HTMLElement {
  return screen.getByLabelText("Search Acct Id:");
}

/** The selector of list row `row`, 1 to 5. */
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

/**
 * Checks the keyboard lock as rendered, before any page or after a page of five rows. While
 * `busy`, ENTER, F3, F7 and F8 are disabled, the key row and the form are `aria-busy`, and the
 * account field and the five selectors are read-only; none of the fields is ever disabled.
 */
function expectLocked(busy: boolean): void {
  const labels = ["ENTER=Continue", "F3=Back", "F7=Backward", "F8=Forward"];
  expect(
    labels.map((label) => [
      label,
      screen.getByRole("button", { name: label }).hasAttribute("disabled"),
    ]),
  ).toEqual(labels.map((label) => [label, busy]));
  const group = screen.getByRole("group", { name: "Function keys" });
  expect(group.getAttribute("aria-busy")).toBe(String(busy));
  expect(document.querySelector("form")?.getAttribute("aria-busy")).toBe(String(busy));
  for (const field of [accountField(), ...[1, 2, 3, 4, 5].map(selector)]) {
    expect(field).toHaveProperty("readOnly", busy);
    expect(field).toHaveProperty("disabled", false);
  }
}

/** Types `account` into the account field and presses ENTER. */
function enter(account: string): void {
  typeInto(accountField(), account);
  press("ENTER=Continue");
}

/** The `ERRMSG` row (row 23), the screen's only message row. */
function errorRow(): HTMLElement {
  const rows = screen
    .getAllByRole("alert")
    .filter((alert) => alert.getAttribute("data-message-kind") === "error");
  expect(rows).toHaveLength(1);
  return rows[0];
}

/** The text the `ERRMSG` row shows, verbatim. */
function errorText(): string | null {
  return errorRow().textContent;
}

/**
 * The error text `element` is marked with, or `null` when it is not marked. A marked input is
 * `aria-invalid` and its `aria-describedby` names one rendered `sr-only` element holding the text,
 * which is no message row: `ERRMSG` stays the screen's one visible message.
 */
function markOf(element: HTMLElement): string | null {
  const hidden = (element.getAttribute("aria-describedby") ?? "")
    .split(/\s+/)
    .filter((id) => id !== "")
    .map((id) => document.getElementById(id))
    .filter((described) => described?.classList.contains("sr-only") === true);
  if (element.getAttribute("aria-invalid") !== "true") {
    expect(hidden).toEqual([]);
    return null;
  }
  expect(hidden).toHaveLength(1);
  const [message] = hidden;
  expect(message?.closest('[role="alert"]')).toBeNull();
  return message?.textContent ?? null;
}

/** The marks of the account field and the five selectors, `null` for each one unmarked. */
function marks(): (string | null)[] {
  return [accountField(), ...[1, 2, 3, 4, 5].map(selector)].map(markOf);
}

/** No input marked. */
const UNMARKED = [null, null, null, null, null, null];

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

/**
 * Asserts the header value fields of rows 1 and 2 against their rows of `modern/audit/screens.md`
 * (COPAU00-002, -003, -005, -007, -008 and -010). Each entry of `fields` is the BMS name, the `POS`
 * row and column, the `LENGTH`, the value shown, and whether the map colours the field YELLOW.
 *
 * The rows describe each value as a read-only `Input` of the field's width in the `CardHeader`
 * title rows of `ScreenShell`, as `ScreenShell` renders it. The assertions prove what the rows
 * require of that input: it is the shadcn `Input` (`data-slot="input"`), read-only and out of the
 * tab order, as an `ASKIP` field is skipped; it is `LENGTH` characters wide (`size`), its grid
 * cell is the field box at `POS` with `LENGTH` as its width, and it holds exactly `LENGTH`
 * characters; it stands in the `CardHeader`, not in the body or the footer; and it carries the
 * tokens of AAP 0.4.3: the foreground with `font-semibold` for a YELLOW title, the foreground
 * alone for a `NORM` value.
 */
function expectHeaderValues(
  root: ParentNode,
  fields: readonly {
    readonly name: string;
    readonly row: number;
    readonly col: number;
    readonly length: number;
    readonly value: string;
    readonly title: boolean;
  }[],
): void {
  const header = root.querySelector('[data-slot="card-header"]');
  if (!(header instanceof HTMLElement)) {
    throw new Error("the screen renders no CardHeader");
  }
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
    const found = root.querySelectorAll(`[data-field="${name}"]`);
    expect(found).toHaveLength(1);
    const element = found[0];
    if (!(element instanceof HTMLInputElement)) {
      throw new Error(`the screen renders no header input ${name}`);
    }
    // The shadcn `Input`, read-only and LENGTH characters wide.
    expect(element.getAttribute("data-slot")).toBe("input");
    expect(element.readOnly).toBe(true);
    expect(element.size).toBe(length);
    // Placement: in the CardHeader title rows, not in the body or the footer.
    expect(element.closest('[data-slot="card-header"]')).toBe(header);
    expect(element.closest('[data-slot="card-content"], [data-slot="card-footer"]')).toBeNull();
    // The field box: the grid cell at POS, LENGTH columns wide, holding exactly LENGTH characters.
    expect(element.parentElement?.className).toBe(cell(row, col, length));
    expect(element.value).toBe(value);
    expect(element.value).toHaveLength(length);
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

/** The `GridCell` placement class of map position (`row`, `col`) and `LENGTH` `span`. */
function cell(row: number, col: number, span: number): string {
  return `row-start-${row} col-start-${col} col-span-${span}`;
}

/**
 * The fourteen output fields of map lines 6 to 12 in map order: the accessible name (the bound
 * caption, or the field's own name where the map gives none), `POS`, `LENGTH` and whether the map
 * declares the field `FSET` (COPAUS0C-004).
 */
const BLOCK_FIELDS = [
  { name: "Name:", row: 6, col: 10, span: 25, fset: false },
  { name: "Customer Id:", row: 6, col: 58, span: 9, fset: false },
  { name: "Address line 1", row: 7, col: 10, span: 25, fset: false },
  { name: "Acct Status:", row: 7, col: 58, span: 1, fset: false },
  { name: "Address line 2", row: 8, col: 10, span: 25, fset: false },
  { name: "PH:", row: 9, col: 15, span: 13, fset: false },
  { name: "Approval # :", row: 9, col: 58, span: 3, fset: false },
  { name: "Decline #:", row: 9, col: 76, span: 3, fset: false },
  { name: "Credit Lim:", row: 11, col: 19, span: 12, fset: true },
  { name: "Cash Lim:", row: 11, col: 46, span: 9, fset: true },
  { name: "Appr Amt:", row: 11, col: 69, span: 10, fset: true },
  { name: "Credit Bal:", row: 12, col: 19, span: 12, fset: true },
  { name: "Cash Bal:", row: 12, col: 46, span: 9, fset: true },
  { name: "Decl Amt:", row: 12, col: 69, span: 10, fset: true },
] as const;

/** What the fourteen block fields show, in BLOCK_FIELDS order. */
function shownBlock(): string[] {
  return BLOCK_FIELDS.map((field) => valueOf(screen.getByLabelText(field.name)));
}

/**
 * What the block fields show for `block`, in BLOCK_FIELDS order: Acct Status is always blank
 * (COPAUS0C-026), and after an erasing send the fields that are not `FSET` are blank
 * (COPAUS0C-004).
 */
function expectedBlock(
  block: components["schemas"]["AuthSummaryBlock"] | null,
  erased = false,
): string[] {
  const values = [
    block?.customer_name ?? "",
    block?.customer_id ?? "",
    block?.address_line_1 ?? "",
    "",
    block?.address_line_2 ?? "",
    block?.phone ?? "",
    block?.approved_count ?? "",
    block?.declined_count ?? "",
    block?.credit_limit ?? "",
    block?.cash_limit ?? "",
    block?.approved_amount ?? "",
    block?.credit_balance ?? "",
    block?.cash_balance ?? "",
    block?.declined_amount ?? "",
  ];
  return values.map((value, index) => (erased && !BLOCK_FIELDS[index].fset ? "" : value));
}

/** The list table's two row groups: the caption and dash lines, then the five list lines. */
function rowGroups(): HTMLElement[] {
  return within(screen.getByRole("table")).getAllByRole("rowgroup");
}

/** The five list lines (map lines 16 to 20). */
function listLines(): HTMLElement[] {
  const [, body] = rowGroups();
  return within(body).getAllByRole("row");
}

/** Transaction ID, date, time, type, A/D, STS and amount of a list line, verbatim. */
function lineValues(line: HTMLElement): (string | null)[] {
  return within(line)
    .getAllByRole("cell")
    .slice(1)
    .map((element) => element.textContent);
}

/**
 * Asserts that the five list lines show `rows` exactly, the lines after them blank, and that a
 * line's selector is protected exactly when it holds no authorization (COPAUS0C-035).
 */
function expectListed(rows: components["schemas"]["AuthSummaryRow"][]): void {
  const lines = listLines();
  expect(lines).toHaveLength(5);
  lines.forEach((line, index) => {
    const row = rows.at(index);
    expect(lineValues(line)).toEqual(
      row === undefined
        ? ["", "", "", "", "", "", ""]
        : [
            row.transaction_id,
            row.date,
            row.time,
            row.auth_type,
            row.approval,
            row.match_status,
            row.amount,
          ],
    );
    expect(selector(index + 1).hasAttribute("readonly")).toBe(row === undefined);
    expect(selector(index + 1).getAttribute("tabindex")).toBe(row === undefined ? "-1" : null);
  });
}

/** Asserts the five list lines of the empty map: blank, with every selector blank and enterable. */
function expectListedEmptyMap(): void {
  const lines = listLines();
  expect(lines).toHaveLength(5);
  lines.forEach((line, index) => {
    expect(lineValues(line)).toEqual(["", "", "", "", "", "", ""]);
    expect(selector(index + 1).hasAttribute("readonly")).toBe(false);
    expect(valueOf(selector(index + 1))).toBe("");
  });
}

beforeEach(() => {
  setSession(TOKEN, "USER0001", "U");
});

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COPAU00 View Authorizations", () => {
  it("renders the empty map on entry with no route state: header rows, title, labels, list captions, instruction and the four key labels, with no request", async () => {
    // audit: COPAU00-011
    // audit: COPAU00-012
    // audit: COPAU00-014
    // audit: COPAU00-015
    // audit: COPAU00-017
    // audit: COPAU00-020
    // audit: COPAU00-023
    // audit: COPAU00-025
    // audit: COPAU00-027
    // audit: COPAU00-029
    // audit: COPAU00-031
    // audit: COPAU00-033
    // audit: COPAU00-035
    // audit: COPAU00-037
    // audit: COPAU00-039
    // audit: COPAU00-041
    // audit: COPAU00-042
    // audit: COPAU00-043
    // audit: COPAU00-044
    // audit: COPAU00-045
    // audit: COPAU00-046
    // audit: COPAU00-047
    // audit: COPAU00-048
    // audit: COPAU00-049
    // audit: COPAU00-050
    // audit: COPAU00-051
    // audit: COPAU00-052
    // audit: COPAU00-053
    // audit: COPAU00-054
    // audit: COPAU00-055
    // audit: COPAU00-056
    // audit: COPAU00-058
    // audit: COPAU00-067
    // audit: COPAU00-076
    // audit: COPAU00-085
    // audit: COPAU00-101
    // audit: COPAU00-102
    // audit: COPAU00-103
    // audit: COPAU00-104
    // audit: COPAU00-105
    // audit: COPAU00-106
    // audit: COPAU00-107
    // audit: COPAU00-108
    // audit: COPAU00-109
    // audit: COPAU00-110
    // audit: COPAU00-111
    // audit: COPAU00-112
    // audit: COPAU00-113
    // audit: COPAU00-114
    // audit: COPAUS0C-001
    // audit: COPAUS0C-054
    const sent = stubFetch();
    renderScreen();
    await idle();
    expect(sent).toHaveLength(0);

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("View Authorizations");
    expect(headerField("TRNNAME")).toBe("CPVS");
    expect(headerField("PGMNAME")).toBe("COPAUS0C");
    expect(headerField("TITLE01")).toBe(TITLE01);
    expect(headerField("TITLE02")).toBe(TITLE02);
    // Time-dependent fields are compared by format only.
    expect(headerField("CURDATE")).toMatch(/^\d{2}\/\d{2}\/\d{2}$/);
    expect(headerField("CURTIME")).toMatch(/^\d{2}:\d{2}:\d{2}$/);

    // ACCTID, POS=(5,19) LENGTH=11, bound to its caption at POS=(5,3); the cursor starts on it.
    const caption = screen.getByText("Search Acct Id:");
    expect(caption.tagName).toBe("LABEL");
    expect(caption.parentElement?.className).toBe(cell(5, 3, 15));
    expect(accountField().getAttribute("maxlength")).toBe("11");
    expect(accountField().parentElement?.className).toBe(cell(5, 19, 11));
    expect(valueOf(accountField())).toBe("");
    expect(document.activeElement).toBe(accountField());
    // The zero-length stopper after ACCTID leaves its column empty.
    expect(document.querySelector(".row-start-5.col-start-31")).toBeNull();

    // Each caption is a label bound to its field, at its map position.
    const captions = [
      { text: "Name: ", row: 6, col: 3, span: 6, field: "Name:" },
      { text: "Customer Id: ", row: 6, col: 44, span: 13, field: "Customer Id:" },
      { text: "Acct Status: ", row: 7, col: 44, span: 13, field: "Acct Status:" },
      { text: "PH:", row: 9, col: 10, span: 3, field: "PH:" },
      { text: "Approval # : ", row: 9, col: 44, span: 13, field: "Approval # :" },
      { text: "Decline #:", row: 9, col: 64, span: 10, field: "Decline #:" },
      { text: "Credit Lim:", row: 11, col: 6, span: 11, field: "Credit Lim:" },
      { text: "Cash Lim:", row: 11, col: 35, span: 9, field: "Cash Lim:" },
      { text: "Appr Amt:", row: 11, col: 58, span: 9, field: "Appr Amt:" },
      { text: "Credit Bal:", row: 12, col: 6, span: 11, field: "Credit Bal:" },
      { text: "Cash Bal:", row: 12, col: 35, span: 9, field: "Cash Bal:" },
      { text: "Decl Amt:", row: 12, col: 58, span: 9, field: "Decl Amt:" },
    ];
    for (const label of captions) {
      const element = screen.getByText(label.text.trim());
      expect(element.tagName).toBe("LABEL");
      expect(element.textContent).toBe(label.text);
      expect(element.parentElement?.className).toBe(cell(label.row, label.col, label.span));
      expect(element.getAttribute("for")).toBe(screen.getByLabelText(label.field).id);
    }
    expect(shownBlock()).toEqual(expectedBlock(null));

    const [head] = rowGroups();
    const [captionLine, dashLine] = within(head).getAllByRole("row", { hidden: true });
    expect(
      within(captionLine)
        .getAllByRole("columnheader")
        .map((element) => element.textContent),
    ).toEqual([
      "Sel",
      " Transaction ID ",
      "  Date  ",
      "  Time  ",
      "Type ",
      "A/D",
      "STS",
      "   Amount   ",
    ]);
    expect(dashLine.getAttribute("aria-hidden")).toBe("true");
    expect(
      within(dashLine)
        .getAllByRole("columnheader", { hidden: true })
        .map((element) => element.textContent),
    ).toEqual([
      "---",
      "-".repeat(16),
      "-".repeat(8),
      "-".repeat(8),
      "----",
      "---",
      "---",
      "-".repeat(12),
    ]);
    // Each list line holds its selector and its seven fields; the stopper after the selector is
    // no element.
    for (const line of listLines()) {
      expect(within(line).getAllByRole("cell")).toHaveLength(8);
    }
    expectListedEmptyMap();

    const instruction = screen.getByText("Type 'S' to View Authorization details from the list");
    expect(instruction.parentElement?.className).toBe(cell(22, 12, 52));

    // ERRMSG: an empty destructive alert that announces politely.
    expect(errorText()).toBe("");
    expect(errorRow().getAttribute("aria-live")).toBe("polite");
    expect(errorRow().className).toContain("text-destructive");

    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(keys.map((key) => key.textContent)).toEqual([
      "ENTER=Continue",
      "F3=Back",
      "F7=Backward",
      "F8=Forward",
    ]);
    const [enterKey, ...others] = keys;
    expect(enterKey.getAttribute("type")).toBe("submit");
    expect(enterKey.getAttribute("data-variant")).toBe("default");
    // The ENTER button is the form's submit button, so the Enter key in a field submits it too.
    expect(enterKey.getAttribute("form")).toBe(document.querySelector("form")?.id);
    for (const key of others) {
      expect(key.getAttribute("type")).toBe("button");
      expect(key.getAttribute("data-variant")).toBe("outline");
      expect(key.getAttribute("data-size")).toBe("sm");
    }
  });

  it("shows the six header values as read-only Inputs in the CardHeader title rows, each at its POS and LENGTH", async () => {
    // audit: COPAU00-002
    // audit: COPAU00-003
    // audit: COPAU00-005
    // audit: COPAU00-007
    // audit: COPAU00-008
    // audit: COPAU00-010
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
      const sent = stubFetch();
      renderScreen();

      expectHeaderValues(document, [
        { name: "TRNNAME", row: 1, col: 7, length: 4, value: "CPVS", title: false },
        { name: "TITLE01", row: 1, col: 21, length: 40, value: TITLE01, title: true },
        { name: "CURDATE", row: 1, col: 71, length: 8, value: "01/02/24", title: false },
        { name: "PGMNAME", row: 2, col: 7, length: 8, value: "COPAUS0C", title: false },
        { name: "TITLE02", row: 2, col: 21, length: 40, value: TITLE02, title: true },
        { name: "CURTIME", row: 2, col: 71, length: 8, value: "03:04:05", title: false },
      ]);
      await idle();
      expect(sent).toHaveLength(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("ENTER 00000000050 reads endpoint 21 once, without page_key or page_size, and shows the block and the rows verbatim", async () => {
    // audit: COPAU00-016
    // audit: COPAU00-018
    // audit: COPAU00-019
    // audit: COPAU00-021
    // audit: COPAU00-022
    // audit: COPAU00-024
    // audit: COPAU00-026
    // audit: COPAU00-028
    // audit: COPAU00-030
    // audit: COPAU00-032
    // audit: COPAU00-034
    // audit: COPAU00-036
    // audit: COPAU00-038
    // audit: COPAU00-040
    // audit: COPAU00-059
    // audit: COPAU00-060
    // audit: COPAU00-061
    // audit: COPAU00-062
    // audit: COPAU00-063
    // audit: COPAU00-064
    // audit: COPAU00-065
    // audit: COPAU00-068
    // audit: COPAU00-069
    // audit: COPAU00-070
    // audit: COPAU00-071
    // audit: COPAU00-072
    // audit: COPAU00-073
    // audit: COPAU00-074
    // audit: COPAU00-077
    // audit: COPAU00-078
    // audit: COPAU00-079
    // audit: COPAU00-080
    // audit: COPAU00-081
    // audit: COPAU00-082
    // audit: COPAU00-083
    // audit: COPAU00-086
    // audit: COPAU00-087
    // audit: COPAU00-088
    // audit: COPAU00-089
    // audit: COPAU00-090
    // audit: COPAU00-091
    // audit: COPAU00-092
    // audit: COPAU00-093
    // audit: COPAU00-094
    // audit: COPAU00-095
    // audit: COPAU00-096
    // audit: COPAU00-097
    // audit: COPAU00-098
    // audit: COPAU00-099
    // audit: COPAU00-111
    // audit: COPAUS0C-026
    // audit: COPAUS0C-035
    // audit: COPAUS0C-053
    const sent = stubFetch(json(200, PAGE_1));
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);

    expect(sent[0].method).toBe("GET");
    expect(sent[0].path).toBe(summaryPath(ACCOUNT));
    expect(sent[0].query).toEqual({});
    expect(sent[0].url).not.toContain("page_size");
    expect(sent[0].authorization).toBe(`Bearer ${TOKEN}`);

    expect(valueOf(accountField())).toBe(ACCOUNT);
    // Each output field is a protected input as wide as its map LENGTH, at its POS, out of the
    // tab order; Acct Status stays blank, as no statement of the program fills it.
    expect(shownBlock()).toEqual(expectedBlock(BLOCK));
    expect(valueOf(screen.getByLabelText("Customer Id:"))).toBe("000000050");
    expect(valueOf(screen.getByLabelText("Acct Status:"))).toBe("");
    for (const field of BLOCK_FIELDS) {
      const output = screen.getByLabelText(field.name);
      expect(output.hasAttribute("readonly")).toBe(true);
      expect(output.getAttribute("tabindex")).toBe("-1");
      expect(output.getAttribute("size")).toBe(String(field.span));
      expect(output.parentElement?.className).toBe(cell(field.row, field.col, field.span));
    }

    // The date and time are the eight-character MM/DD/YY and HH:MM:SS strings as received, with
    // no calendar check and no time-zone shift: row 2's 00:00:00 and row 3's 13/99/24 23:59:59
    // show as sent.
    expectListed(PAGE_1.rows);
    const [, , third] = listLines();
    expect(lineValues(third).slice(1, 3)).toEqual(["13/99/24", "23:59:59"]);
    for (const line of listLines()) {
      const [date, time] = lineValues(line).slice(1, 3);
      expect(date).toMatch(/^\d{2}\/\d{2}\/\d{2}$/);
      expect(time).toMatch(/^\d{2}:\d{2}:\d{2}$/);
    }
    for (let row = 1; row <= 5; row += 1) {
      expect(selector(row).getAttribute("maxlength")).toBe("1");
      expect(valueOf(selector(row))).toBe("");
    }

    expect(errorText()).toBe("");
    expect(document.activeElement).toBe(accountField());
  });

  it("a first entry whose route state carries a numeric acct_id, as F3 on COPAU01 sends it, loads that account without typing", async () => {
    // audit: COPAUS0C-002
    // audit: COPAUS0C-053
    const sent = stubFetch(json(200, PAGE_1));
    renderScreen({ from: PATHS.COPAU01, acct_id: ACCOUNT });
    await answered(sent, 1);

    expect(sent[0].path).toBe(summaryPath(ACCOUNT));
    expect(sent[0].query).toEqual({});
    expect(valueOf(accountField())).toBe(ACCOUNT);
    expect(shownBlock()).toEqual(expectedBlock(BLOCK));
    expectListed(PAGE_1.rows);
    expect(errorText()).toBe("");
    expect(document.activeElement).toBe(accountField());

    // The first entry sets the page number to 0, then the page read makes it 1: F7 is at the top.
    press("F7=Backward");
    expect(errorText()).toBe(TOP_OF_PAGE);
    await idle();
    expect(sent).toHaveLength(1);
  });

  it.each([
    { title: "no route state", state: undefined },
    { title: "a route state without acct_id", state: { from: PATHS.COPAU01 } },
    { title: "an acct_id of 10 digits", state: { from: PATHS.COPAU01, acct_id: "0000000005" } },
    {
      title: "an acct_id that is not all digits",
      state: { from: PATHS.COPAU01, acct_id: "0000000005A" },
    },
    { title: "an acct_id that is not text", state: { from: PATHS.COPAU01, acct_id: 50 } },
  ])(
    "a first entry with $title shows every field blank and sends nothing; F7 and F8, by button and by key, give the top and bottom messages",
    async ({ state }) => {
      // audit: COPAUS0C-001
      // audit: COPAUS0C-002
      // audit: COPAUS0C-049
      // audit: COPAU00-113
      // audit: COPAU00-114
      const sent = stubFetch();
      renderScreen(state);
      await idle();

      expect(valueOf(accountField())).toBe("");
      expect(shownBlock()).toEqual(expectedBlock(null));
      expectListedEmptyMap();
      expect(errorText()).toBe("");
      expect(document.activeElement).toBe(accountField());

      press("F7=Backward");
      expect(errorText()).toBe(TOP_OF_PAGE);
      press("F8=Forward");
      expect(errorText()).toBe(BOTTOM_OF_PAGE);
      fireEvent.keyDown(accountField(), { key: "F7" });
      expect(errorText()).toBe(TOP_OF_PAGE);
      fireEvent.keyDown(accountField(), { key: "F8" });
      expect(errorText()).toBe(BOTTOM_OF_PAGE);
      await idle();
      expect(sent).toHaveLength(0);
    },
  );

  it.each([1, 2, 3, 4, 5])(
    "S on row %i plus ENTER opens /authorizations/detail with the account and that row's key, sending nothing",
    async (row) => {
      // audit: COPAU00-057
      // audit: COPAU00-066
      // audit: COPAU00-075
      // audit: COPAU00-084
      // audit: COPAU00-100
      // audit: COPAU00-111
      // audit: COPAUS0C-010
      const sent = stubFetch(json(200, PAGE_1));
      renderScreen();
      enter(ACCOUNT);
      await answered(sent, 1);

      expect(selector(row).getAttribute("maxlength")).toBe("1");
      typeInto(selector(row), "S");
      press("ENTER=Continue");

      await screen.findByRole("region", { name: "Authorization Details" });
      // COPAU01 reads `acct_id` and `auth_key` from this state (its `detailEntry`).
      expect(transferState("Authorization Details")).toEqual({
        from: PATHS.COPAU00,
        acct_id: ACCOUNT,
        auth_key: authKey(row),
      });
      await idle();
      expect(sent).toHaveLength(1);
    },
  );

  it("the transfer carries the account typed on this ENTER, which may differ from the account the rows belong to", async () => {
    // audit: COPAUS0C-010
    const sent = stubFetch(json(200, PAGE_1));
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);

    typeInto(accountField(), NO_SUMMARY_ACCOUNT);
    typeInto(selector(2), "S");
    press("ENTER=Continue");

    await screen.findByRole("region", { name: "Authorization Details" });
    expect(transferState("Authorization Details")).toEqual({
      from: PATHS.COPAU00,
      acct_id: NO_SUMMARY_ACCOUNT,
      auth_key: authKey(2),
    });
    expect(sent).toHaveLength(1);
  });

  it.each([
    {
      title: "a space and a low-value are blank, so s on row 3 counts and the X on row 4 is ignored",
      codes: [" ", "\u0000", "s", "X", ""],
      row: 3,
    },
    {
      title: "S on row 1 counts and the S on row 5 is ignored",
      codes: ["S", "", "", "", "S"],
      row: 1,
    },
  ])("only the first non-blank selector counts: $title", async ({ codes, row }) => {
    // audit: COPAUS0C-009
    // audit: COPAUS0C-010
    const sent = stubFetch(json(200, PAGE_1));
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);

    codes.forEach((code, index) => typeInto(selector(index + 1), code));
    press("ENTER=Continue");

    await screen.findByRole("region", { name: "Authorization Details" });
    expect(transferState("Authorization Details")).toEqual({
      from: PATHS.COPAU00,
      acct_id: ACCOUNT,
      auth_key: authKey(row),
    });
    expect(sent).toHaveLength(1);
  });

  it.each([
    { title: "X on row 2", codes: ["", "X", "", "", ""] },
    { title: "U on row 5", codes: ["", "", "", "", "U"] },
    { title: "1 on row 1, ahead of an S on row 2", codes: ["1", "S", "", "", ""] },
  ])(
    "$title shows the invalid-selection message and reads the account again from its first page",
    async ({ codes }) => {
      // audit: COPAUS0C-009
      // audit: COPAUS0C-011
      // audit: COPAUS0C-035
      const sent = stubFetch(json(200, PAGE_1), json(200, PAGE_1));
      renderScreen();
      enter(ACCOUNT);
      await answered(sent, 1);

      codes.forEach((code, index) => typeInto(selector(index + 1), code));
      press("ENTER=Continue");
      await answered(sent, 2);

      expect(screen.queryByRole("region", { name: "Authorization Details" })).toBeNull();
      expect(sent[1].path).toBe(summaryPath(ACCOUNT));
      expect(sent[1].query).toEqual({});
      expect(errorText()).toBe(INVALID_SELECTION);
      // The message names no field, so neither the account nor a selector is marked.
      expect(marks()).toEqual(UNMARKED);
      // The page read leaves every selector blank.
      expectListed(PAGE_1.rows);
      for (let row = 1; row <= 5; row += 1) {
        expect(valueOf(selector(row))).toBe("");
      }
      expect(document.activeElement).toBe(accountField());
    },
  );

  it("an invalid selection gives way to the message of the read it is sent with", async () => {
    // audit: COPAUS0C-011
    const sent = stubFetch(json(200, PAGE_1), json(404, NOT_FOUND));
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);

    typeInto(selector(1), "X");
    enter(UNKNOWN_ACCOUNT);
    await answered(sent, 2);

    expect(sent[1].path).toBe(summaryPath(UNKNOWN_ACCOUNT));
    expect(errorText()).toBe(XREF_NOT_FOUND);
  });

  it("a selector of the empty map is ignored: S on row 1 plus ENTER reads the account with no transfer and no message", async () => {
    // audit: COPAUS0C-012
    const sent = stubFetch(json(200, PAGE_1));
    renderScreen();

    // Before any read every selector is enterable, as the map declares.
    for (let row = 1; row <= 5; row += 1) {
      expect(selector(row).hasAttribute("readonly")).toBe(false);
    }
    typeInto(selector(1), "S");
    enter(ACCOUNT);
    await answered(sent, 1);

    expect(screen.queryByRole("region", { name: "Authorization Details" })).toBeNull();
    expect(sent[0].path).toBe(summaryPath(ACCOUNT));
    expect(sent[0].query).toEqual({});
    expect(errorText()).toBe("");
    expectListed(PAGE_1.rows);
    expect(valueOf(selector(1))).toBe("");
  });

  it("ENTER on an account without a summary shows its block, an empty list and every selector protected", async () => {
    // audit: COPAUS0C-035
    const sent = stubFetch(json(200, PAGE_1), json(200, NO_SUMMARY));
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);

    enter(NO_SUMMARY_ACCOUNT);
    await answered(sent, 2);

    expect(sent[1].path).toBe(summaryPath(NO_SUMMARY_ACCOUNT));
    expect(valueOf(accountField())).toBe(NO_SUMMARY_ACCOUNT);
    expect(shownBlock()).toEqual(expectedBlock(NO_SUMMARY_BLOCK));
    expectListed([]);
    expect(errorText()).toBe("");
  });

  it("F8 and F7 page the state's account by its keys: the rows change, the block stays, F7 at page 1 and F8 after the last page give their messages, and no URL carries page_size", async () => {
    // audit: COPAUS0C-039
    // audit: COPAUS0C-040
    // audit: COPAUS0C-045
    // audit: COPAUS0C-046
    // audit: COPAUS0C-048
    // audit: COPAUS0C-053
    // audit: COPAU00-113
    // audit: COPAU00-114
    const backToPage1: components["schemas"]["AuthSummaryResponse"] = fullPage(
      1,
      true,
      UPDATED_BLOCK,
    );
    const sent = stubFetch(
      json(200, PAGE_1),
      json(200, PAGE_2),
      json(200, backToPage1),
      json(200, backToPage1),
      json(200, PAGE_2),
    );
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);

    // After the ENTER that showed page 1, F7 is at the top while no F8 has shown a further page.
    press("F7=Backward");
    expect(errorText()).toBe(TOP_OF_PAGE);
    await idle();
    expect(sent).toHaveLength(1);
    expectListed(PAGE_1.rows);

    // F8 pages the state's account, never what the account field now holds.
    typeInto(accountField(), UNKNOWN_ACCOUNT);
    press("F8=Forward");
    await answered(sent, 2);
    expect(sent[1].path).toBe(summaryPath(ACCOUNT));
    expect(sent[1].query).toEqual({ page_key: authKey(5), direction: "forward" });
    expectListed(PAGE_2.rows);
    expect(shownBlock()).toEqual(expectedBlock(BLOCK));
    expect(valueOf(accountField())).toBe(UNKNOWN_ACCOUNT);
    expect(errorText()).toBe("");
    expect(document.activeElement).toBe(accountField());

    // Page 2 had no successor: F8 reads nothing and changes nothing.
    press("F8=Forward");
    expect(errorText()).toBe(BOTTOM_OF_PAGE);
    await idle();
    expect(sent).toHaveLength(2);
    expectListed(PAGE_2.rows);

    // F7 at page 2 reads backward from page 1's first key.
    press("F7=Backward");
    await answered(sent, 3);
    expect(sent[2].path).toBe(summaryPath(ACCOUNT));
    expect(sent[2].query).toEqual({ page_key: authKey(1), direction: "backward" });
    expectListed(PAGE_1.rows);
    expect(shownBlock()).toEqual(expectedBlock(BLOCK));
    expect(errorText()).toBe("");

    // The page read counted page 1 as page 2 again, so a further F7 shows the same page again,
    // with no message, never the top-of-page message (X-COPAUS0C-2, preserved).
    fireEvent.keyDown(accountField(), { key: "F7" });
    await answered(sent, 4);
    expect(sent[3].query).toEqual({ page_key: authKey(1), direction: "backward" });
    expectListed(PAGE_1.rows);
    expect(errorText()).toBe("");

    // F7 set the next-page flag, so F8 goes forward again from page 1's last key.
    fireEvent.keyDown(accountField(), { key: "F8" });
    await answered(sent, 5);
    expect(sent[4].query).toEqual({ page_key: authKey(5), direction: "forward" });
    expectListed(PAGE_2.rows);

    for (const request of sent) {
      expect(request.method).toBe("GET");
      expect(request.url).not.toContain("page_size");
    }
  });

  it("F8 after a page of fewer than five rows keeps the flag: the forward read shows an empty list with no message", async () => {
    // audit: COPAUS0C-035
    // audit: COPAUS0C-039
    // audit: COPAUS0C-042
    const sent = stubFetch(
      json(200, PAGE_1),
      json(200, SHORT_PAGE),
      json(200, EMPTY_PAGE),
      json(200, PAGE_1),
    );
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);

    press("F8=Forward");
    await answered(sent, 2);
    // Rows 3 to 5 stay blank and their selectors protected.
    expectListed(SHORT_PAGE.rows);

    press("F8=Forward");
    await answered(sent, 3);
    expect(sent[2].query).toEqual({ page_key: authKey(7), direction: "forward" });
    expectListed([]);
    expect(errorText()).toBe("");
    expect(shownBlock()).toEqual(expectedBlock(BLOCK));

    // The empty read added no page, so F7 still reads page 1 from its first key.
    press("F7=Backward");
    await answered(sent, 4);
    expect(sent[3].query).toEqual({ page_key: authKey(1), direction: "backward" });
    expectListed(PAGE_1.rows);
  });

  it("an F7 whose backward read finds no row leaves the page number n − 1, so the next F7 reads back from the first key of the page before that", async () => {
    // audit: COPAUS0C-039
    // audit: COPAUS0C-046
    const page2: components["schemas"]["AuthSummaryResponse"] = fullPage(2, true);
    const page3: components["schemas"]["AuthSummaryResponse"] = fullPage(3, false);
    const sent = stubFetch(
      json(200, PAGE_1),
      json(200, page2),
      json(200, page3),
      json(200, EMPTY_PAGE),
      json(200, PAGE_1),
    );
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);
    press("F8=Forward");
    await answered(sent, 2);
    press("F8=Forward");
    await answered(sent, 3);
    expectListed(page3.rows);

    // F7 at page 3 reads back from page 2's first key and finds no row, so no page is added.
    press("F7=Backward");
    await answered(sent, 4);
    expect(sent[3].path).toBe(summaryPath(ACCOUNT));
    expect(sent[3].query).toEqual({ page_key: authKey(6), direction: "backward" });
    expectListed([]);
    expect(errorText()).toBe("");

    // The page number stayed 2, so this F7 reads back from page 1's first key.
    press("F7=Backward");
    await answered(sent, 5);
    expect(sent[4].path).toBe(summaryPath(ACCOUNT));
    expect(sent[4].query).toEqual({ page_key: authKey(1), direction: "backward" });
    expectListed(PAGE_1.rows);
    expect(errorText()).toBe("");
  });

  it("ENTER on another account pages that account from its own first keys, never from the earlier account's", async () => {
    // audit: COPAUS0C-039
    // audit: COPAUS0C-045
    // audit: COPAUS0C-046
    const second = "00000000049";
    /** Page `n` of the second account: authorizations 5n + 96 to 5n + 100, none of ACCOUNT's. */
    const secondPage = (
      n: number,
      hasNext: boolean,
    ): components["schemas"]["AuthSummaryResponse"] => ({
      ...fullPage(20 + n, hasNext),
      acct_id: second,
    });
    const sent = stubFetch(
      json(200, PAGE_1),
      json(200, fullPage(2, true)),
      json(200, fullPage(3, true)),
      json(200, secondPage(1, true)),
      json(200, secondPage(2, true)),
      json(200, secondPage(1, true)),
    );
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);
    press("F8=Forward");
    await answered(sent, 2);
    press("F8=Forward");
    await answered(sent, 3);

    // The ENTER sets the page number to 0 and its page read counts the second account's page 1.
    enter(second);
    await answered(sent, 4);
    expect(sent[3].path).toBe(summaryPath(second));
    expect(sent[3].query).toEqual({});
    expectListed(secondPage(1, true).rows);

    // F7 at page 1 is at the top, whatever pages the earlier account showed.
    press("F7=Backward");
    expect(errorText()).toBe(TOP_OF_PAGE);
    await idle();
    expect(sent).toHaveLength(4);

    press("F8=Forward");
    await answered(sent, 5);
    expect(sent[4].query).toEqual({ page_key: authKey(105), direction: "forward" });
    expectListed(secondPage(2, true).rows);

    // F7 at page 2 reads back from the second account's page-1 key, not ACCOUNT's.
    press("F7=Backward");
    await answered(sent, 6);
    expect(sent[5].path).toBe(summaryPath(second));
    expect(sent[5].query).toEqual({ page_key: authKey(101), direction: "backward" });
    expectListed(secondPage(1, true).rows);
  });

  it.each([
    { title: "a blank account", account: "", message: ENTER_ACCT_ID },
    { title: "an account with a letter", account: "0000000005A", message: ACCT_ID_NOT_NUMERIC },
    { title: "an account of 10 digits", account: "0000000005", message: ACCT_ID_NOT_NUMERIC },
  ])(
    "ENTER with $title blanks the field, keeps the FSET fields, sends nothing and sets the page number to 0",
    async ({ account, message }) => {
      // audit: COPAUS0C-007
      // audit: COPAUS0C-039
      // audit: COPAUS0C-053
      const page2: components["schemas"]["AuthSummaryResponse"] = fullPage(2, true);
      const page3: components["schemas"]["AuthSummaryResponse"] = fullPage(3, false);
      const sent = stubFetch(json(200, PAGE_1), json(200, page2), json(200, page3));
      renderScreen();
      enter(ACCOUNT);
      await answered(sent, 1);
      press("F8=Forward");
      await answered(sent, 2);
      expectListed(page2.rows);

      typeInto(selector(1), "S");
      enter(account);
      await waitFor(() => expect(errorText()).toBe(message));
      // The edit's message also marks the account field, for assistive technology only.
      expect(marks()).toEqual([message, null, null, null, null, null]);
      await idle();
      expect(sent).toHaveLength(2);
      expect(screen.queryByRole("region", { name: "Authorization Details" })).toBeNull();
      expect(valueOf(accountField())).toBe("");
      // The map goes with ERASE: the six amounts and the list rows are FSET and keep their text.
      expect(shownBlock()).toEqual(expectedBlock(BLOCK, true));
      expectListed(page2.rows);
      expect(document.activeElement).toBe(accountField());

      // The page number is 0, so F7 is at the top; its message takes the mark off the account.
      press("F7=Backward");
      expect(errorText()).toBe(TOP_OF_PAGE);
      expect(marks()).toEqual(UNMARKED);
      await idle();
      expect(sent).toHaveLength(2);

      // F8 goes on from the state's last key of the last valid account and counts page 1.
      press("F8=Forward");
      await answered(sent, 3);
      expect(sent[2].path).toBe(summaryPath(ACCOUNT));
      expect(sent[2].query).toEqual({ page_key: authKey(10), direction: "forward" });
      expectListed(page3.rows);
      press("F7=Backward");
      expect(errorText()).toBe(TOP_OF_PAGE);
      await idle();
      expect(sent).toHaveLength(3);
    },
  );

  it.each([
    { title: "F2", init: { key: "F2" } },
    { title: "F12", init: { key: "F12" } },
    { title: "Shift+F3 (PF15)", init: { key: "F3", shiftKey: true } },
  ])(
    "$title is an invalid key: its message, no request, the FSET fields kept and the others blanked",
    async ({ init }) => {
      // audit: COPAUS0C-004
      const sent = stubFetch(json(200, PAGE_1), json(200, PAGE_2));
      renderScreen();
      enter(ACCOUNT);
      await answered(sent, 1);

      fireEvent.keyDown(accountField(), init);
      expect(errorText()).toBe(INVALID_KEY);
      await idle();
      expect(sent).toHaveLength(1);
      expect(screen.queryByRole("region", { name: "Main Menu" })).toBeNull();
      expect(valueOf(accountField())).toBe(ACCOUNT);
      expect(shownBlock()).toEqual(expectedBlock(BLOCK, true));
      expectListed(PAGE_1.rows);

      // The state is unchanged: F8 still pages forward from page 1's last key.
      press("F8=Forward");
      await answered(sent, 2);
      expect(sent[1].query).toEqual({ page_key: authKey(5), direction: "forward" });
    },
  );

  it.each([
    { title: "the F3=Back button", act: () => press("F3=Back") },
    { title: "the F3 key", act: () => fireEvent.keyDown(accountField(), { key: "F3" }) },
  ])("$title returns to the main menu with no request", async ({ act }) => {
    // audit: COPAUS0C-003
    // audit: COPAU00-112
    const sent = stubFetch(json(200, PAGE_1));
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);

    act();

    await screen.findByRole("region", { name: "Main Menu" });
    expect(transferState("Main Menu")).toEqual({ from: PATHS.COPAU00 });
    await idle();
    expect(sent).toHaveLength(1);
  });

  it("a 404 ScreenError shows its message alone, blanks the fields that are not FSET and keeps the cursor on the account", async () => {
    // audit: COPAU00-103
    // audit: COPAUS0C-053
    const sent = stubFetch(json(200, PAGE_1), json(404, NOT_FOUND));
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);

    enter(UNKNOWN_ACCOUNT);
    await answered(sent, 2);

    expect(sent[1].path).toBe(summaryPath(UNKNOWN_ACCOUNT));
    expect(errorText()).toBe(XREF_NOT_FOUND);
    expect(errorRow().className).toContain("text-destructive");
    expect(valueOf(accountField())).toBe(UNKNOWN_ACCOUNT);
    expect(shownBlock()).toEqual(expectedBlock(BLOCK, true));
    expect(document.activeElement).toBe(accountField());
    // The error names `acct_id`: the account field is marked with its message, for assistive
    // technology only, and no selector is.
    expect(marks()).toEqual([XREF_NOT_FOUND, null, null, null, null, null]);
  });

  it.each([
    { title: "F7 at the top", act: () => press("F7=Backward"), message: TOP_OF_PAGE, reads: 1 },
    { title: "F8 at the bottom", act: () => press("F8=Forward"), message: BOTTOM_OF_PAGE, reads: 1 },
    {
      title: "an invalid key",
      act: () => fireEvent.keyDown(accountField(), { key: "F2" }),
      message: INVALID_KEY,
      reads: 1,
    },
    { title: "ENTER on an account found", act: () => enter(ACCOUNT), message: "", reads: 2 },
  ])(
    "the next key, $title, takes off the mark a ScreenError naming acct_id put on the account",
    async ({ act, message, reads }) => {
      const sent = stubFetch(json(404, NOT_FOUND), json(200, PAGE_1));
      renderScreen();
      enter(UNKNOWN_ACCOUNT);
      await answered(sent, 1);
      expect(marks()).toEqual([XREF_NOT_FOUND, null, null, null, null, null]);

      act();

      await answered(sent, reads);
      await waitFor(() => expect(errorText()).toBe(message));
      expect(marks()).toEqual(UNMARKED);
      expect(document.activeElement).toBe(accountField());
    },
  );

  it("an F8 read that fails shows the server's message verbatim, its leading space included", async () => {
    // audit: COPAU00-103
    const sent = stubFetch(json(200, PAGE_1), json(500, REPOS_ERROR));
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);

    press("F8=Forward");
    await answered(sent, 2);

    expect(errorText()).toBe(REPOS_FAILED);
    expect(shownBlock()).toEqual(expectedBlock(BLOCK, true));
    expectListed(PAGE_1.rows);
    expect(document.activeElement).toBe(accountField());
    // The error names no field, so nothing is marked.
    expect(marks()).toEqual(UNMARKED);
  });

  it("locks the keyboard while a read is outstanding: ENTER, F3, F7 and F8 are disabled, the fields read-only and an invalid key does nothing until it answers", async () => {
    const gate = { release: () => {} };
    const held = new Promise<void>((resolve) => {
      gate.release = () => resolve();
    });
    const sent = stubFetch(async () => {
      await held;
      return json(200, PAGE_1)();
    }, json(200, PAGE_2));
    renderScreen();
    expectLocked(false);
    enter(ACCOUNT);
    await waitFor(() => expect(sent).toHaveLength(1));
    expectLocked(true);

    press("ENTER=Continue");
    press("F3=Back");
    press("F7=Backward");
    press("F8=Forward");
    fireEvent.keyDown(accountField(), { key: "F2" });
    fireEvent.keyDown(accountField(), { key: "F3" });
    fireEvent.keyDown(accountField(), { key: "F8" });
    await idle();
    expect(sent).toHaveLength(1);
    expect(screen.queryByRole("region", { name: "Main Menu" })).toBeNull();
    expect(errorText()).toBe("");

    gate.release();
    await answered(sent, 1);
    expectLocked(false);
    expectListed(PAGE_1.rows);
    expect(shownBlock()).toEqual(expectedBlock(BLOCK));

    // The answer unlocks the keyboard: the next key is served.
    press("F8=Forward");
    await answered(sent, 2);
    expect(sent[1].query).toEqual({ page_key: authKey(5), direction: "forward" });
  });

  // The lock is taken by the key itself, before the edits run: keys pressed in one burst, before
  // React renders the disabled keys, send one read, and the answer unlocks the next key.
  it.each([
    {
      title: "three clicks on ENTER=Continue",
      presses: ["ENTER=Continue", "ENTER=Continue", "ENTER=Continue"],
      query: {},
    },
    { title: "three Enter keys", presses: ["Enter", "Enter", "Enter"], query: {} },
    {
      title: "three clicks on F8=Forward",
      presses: ["F8=Forward", "F8=Forward", "F8=Forward"],
      query: { page_key: authKey(5), direction: "forward" },
    },
    {
      title: "three F8 keys",
      presses: ["F8", "F8", "F8"],
      query: { page_key: authKey(5), direction: "forward" },
    },
    {
      title: "clicks on F8=Forward, ENTER=Continue, F7=Backward and F3=Back",
      presses: ["F8=Forward", "ENTER=Continue", "F7=Backward", "F3=Back"],
      query: { page_key: authKey(5), direction: "forward" },
    },
    { title: "the Enter, F8, F7 and F3 keys", presses: ["Enter", "F8", "F7", "F3"], query: {} },
  ])("$title in one burst send one read until it is answered", async ({ presses, query }) => {
    const gate = { release: () => {} };
    const held = new Promise<void>((resolve) => {
      gate.release = () => resolve();
    });
    const sent = stubFetch(
      json(200, PAGE_1),
      async () => {
        await held;
        return json(200, PAGE_2)();
      },
      json(200, PAGE_1),
    );
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);

    // Every press inside one `act`: a click on a key's button, or a keydown that reaches `window`.
    act(() => {
      for (const label of presses) {
        if (label.includes("=")) {
          press(label);
        } else {
          fireEvent.keyDown(window, { key: label });
        }
      }
    });
    await waitFor(() => expect(sent).toHaveLength(2));
    expectLocked(true);
    await idle();
    expect(sent).toHaveLength(2);
    expect(sent[1].path).toBe(summaryPath(ACCOUNT));
    expect(sent[1].query).toEqual(query);
    expect(screen.queryByRole("region", { name: "Main Menu" })).toBeNull();

    gate.release();
    await answered(sent, 2);
    expectLocked(false);

    press("ENTER=Continue");
    await answered(sent, 3);
    expect(sent[2].query).toEqual({});
  });
});

describe("BLOCKED X-COPAUS0C-H4: F8 with the flag set and no last key, not a parity result", () => {
  it("shows the bottom-of-page message without a request after a forward read that found nothing", async () => {
    // audit: COPAUS0C-043
    const sent = stubFetch(json(200, PAGE_1), json(200, SHORT_PAGE), json(200, EMPTY_PAGE));
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);
    press("F8=Forward");
    await answered(sent, 2);
    press("F8=Forward");
    await answered(sent, 3);
    expectListed([]);

    press("F8=Forward");
    expect(errorText()).toBe(BOTTOM_OF_PAGE);
    await idle();
    expect(sent).toHaveLength(3);
  });
});

describe("BLOCKED X-COPAUS0C-H4: F8 after an ENTER that found no summary or no cross-reference, not a parity result", () => {
  it.each([
    {
      title: "an account without a summary",
      account: NO_SUMMARY_ACCOUNT,
      reply: json(200, NO_SUMMARY),
    },
    {
      title: "an account not in the cross-reference",
      account: UNKNOWN_ACCOUNT,
      reply: json(404, NOT_FOUND),
    },
  ])(
    "after $title, F8 sends the earlier last key forward for the state's new account",
    async ({ account, reply }) => {
      // audit: COPAUS0C-044
      const empty: components["schemas"]["AuthSummaryResponse"] = {
        ...EMPTY_PAGE,
        acct_id: account,
      };
      const sent = stubFetch(json(200, PAGE_1), reply, json(200, empty));
      renderScreen();
      enter(ACCOUNT);
      await answered(sent, 1);
      enter(account);
      await answered(sent, 2);

      press("F8=Forward");
      await answered(sent, 3);
      expect(sent[2].path).toBe(summaryPath(account));
      expect(sent[2].query).toEqual({ page_key: authKey(5), direction: "forward" });
      expectListed([]);
      expect(errorText()).toBe("");
    },
  );
});

describe("BLOCKED X-COPAUS0C-H3: pages after the 20th, not a parity result", () => {
  it("F8 goes on to page 21 from page 20's last key, and F7 there reads back from page 20's first key", async () => {
    // audit: COPAUS0C-050
    const pages: components["schemas"]["AuthSummaryResponse"][] = Array.from(
      { length: 21 },
      (_, index) => fullPage(index + 1, true),
    );
    const sent = stubFetch(...pages.map((page) => json(200, page)), json(200, pages[19]));
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);

    for (let page = 2; page <= 21; page += 1) {
      press("F8=Forward");
      await answered(sent, page);
      expect(sent[page - 1].query).toEqual({
        page_key: authKey(5 * (page - 1)),
        direction: "forward",
      });
    }
    expectListed(pages[20].rows);

    press("F7=Backward");
    await answered(sent, 22);
    expect(sent[21].query).toEqual({ page_key: authKey(96), direction: "backward" });
    expectListed(pages[19].rows);
  });

  it("the screen holds the first keys of the last 20 pages: F7 at page 25 reads back from page 24's first key, and the F7 that would need page 5's key sends nothing", async () => {
    // audit: COPAUS0C-050
    const pages: components["schemas"]["AuthSummaryResponse"][] = Array.from(
      { length: 25 },
      (_, index) => fullPage(index + 1, true),
    );
    const sent = stubFetch(
      ...pages.map((page) => json(200, page)),
      json(200, pages[23]),
      ...Array.from({ length: 19 }, () => json(200, EMPTY_PAGE)),
    );
    renderScreen();
    enter(ACCOUNT);
    await answered(sent, 1);
    for (let page = 2; page <= 25; page += 1) {
      press("F8=Forward");
      await answered(sent, page);
    }
    expect(sent[24].query).toEqual({ page_key: authKey(120), direction: "forward" });
    expectListed(pages[24].rows);

    // F7 at page 25 reads back from page 24's first key, and that read counts page 25 again.
    press("F7=Backward");
    await answered(sent, 26);
    expect(sent[25].query).toEqual({ page_key: authKey(116), direction: "backward" });
    expectListed(pages[23].rows);

    // Backward reads that find no row take the page number down one at a time, each from the
    // first key of the page before it: pages 24 to 6, the earliest of the 20 pages held.
    for (let page = 24; page >= 6; page -= 1) {
      press("F7=Backward");
      await answered(sent, 26 + 25 - page);
      expect(sent.at(-1)?.query).toEqual({
        page_key: authKey(5 * page - 4),
        direction: "backward",
      });
    }
    expectListed([]);

    // Page 5's first key is no longer held: the top-of-page message, with no request.
    press("F7=Backward");
    expect(errorText()).toBe(TOP_OF_PAGE);
    await idle();
    expect(sent).toHaveLength(45);
    expectListed([]);
  });
});

/** The 401 of the `SignedOnUser` extractor for a missing, invalid or expired token. */
const SIGNON_REQUIRED: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};

/**
 * Renders the application's route table at `/authorizations`, so the sign-on guard that only the
 * table mounts stands in front of the screen. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COPAU00] });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

// A 401 clears the stored session (`api/client.ts`), and the sign-on guard in front of the route
// replaces the still-mounted screen with sign-on at once: after the key that sent the refused
// request, the test presses no key and follows no link.
describe("COPAU00 in the route table: a 401 ends the session", () => {
  it.each([
    {
      title: "the summary of ENTER",
      replies: [json(401, SIGNON_REQUIRED)],
      drive: async () => {
        enter(ACCOUNT);
      },
      count: 1,
    },
    {
      title: "the F8 page after page 1",
      replies: [json(200, PAGE_1), json(401, SIGNON_REQUIRED)],
      drive: async (sent: ReturnType<typeof stubFetch>) => {
        enter(ACCOUNT);
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
      expect(
        await screen.findByRole("heading", { level: 1, name: "View Authorizations" }),
      ).toBeDefined();

      await drive(sent);

      expect(await screen.findByRole("button", { name: "ENTER=Sign-on" })).toBeDefined();
      expect(router.state.location.pathname).toBe(PATHS.COSGN00);
      expect(screen.queryByRole("heading", { level: 1, name: "View Authorizations" })).toBeNull();
      expect(getSession()).toBeNull();
      expect(sent).toHaveLength(count);
      for (const request of sent) {
        expect([request.method, request.path, request.authorization]).toEqual([
          "GET",
          summaryPath(ACCOUNT),
          `Bearer ${TOKEN}`,
        ]);
      }
      router.dispose();
    },
  );
});
