import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createMemoryRouter, MemoryRouter, Route, RouterProvider, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";
import type { components } from "@/api/schema";
import { getSession, setSession } from "@/auth/session";
import { PATHS, routes } from "@/routes";
import { COACTVW } from "./COACTVW";

/*
 * Render test of the account-view screen: map COACTVW (CACTVWA), transaction CAVW, program
 * COACTVWC, route /accounts/view, endpoint 4 (`GET /api/v1/accounts/{acct_id}/view`). It proves
 * the screen's `client` rows of modern/audit/screens.md (COACTVW-011 to COACTVW-108) and of
 * modern/audit/online/COACTVWC.md; every position, length, literal and message below is written
 * by hand from those rows, never read from the screen.
 *
 * Not tagged here:
 * - The header literals COACTVW-001, COACTVW-004, COACTVW-006 and COACTVW-009, which NAV tags.
 *   They and the header values COACTVW-002 to COACTVW-010 are described as `Label` and read-only
 *   `Input` elements, as `ScreenShell` renders them; the value rows are tagged here on each
 *   value's read-only `Input`, width, map `POS`, `LENGTH`, label and content, as are their content
 *   rows (COACTVW-101 to COACTVW-106) and the absence of the unnamed (1,1) field (COACTVW-098).
 * - COACTVW-013 is a `both` row; its client half lives in src/schemas/COACTVW.test.ts.
 *
 * COACTVWC's blank, non-numeric and all-zero edits are `server` rows (COACTVWC-030, COACTVWC-031):
 * the screen sends such a value and shows the endpoint's message, so the stubbed endpoint answers
 * them here with the audited literals. Only a partly filled field is refused before sending
 * (MUSTFILL). F3 returns to the main menu on every visit, with or without route state, because
 * COMEN01C is the only program that transfers here (COACTVWC-004).
 */

const ACCOUNT = "00000000050";
const OTHER_ACCOUNT = "00000000099";

/** The 40-byte `WS-INFO-MSG` prompt in the 45-position `INFOMSG` (COACTVWC-027, COACTVW-096). */
const PROMPT = `Enter or update id of account to display${" ".repeat(5)}`;

/** Title rows 1 and 2, the 40-byte `CCDA-TITLE01` and `CCDA-TITLE02` (COACTVW-102, COACTVW-105). */
const TITLE01 = "      AWS Mainframe Modernization       ";
const TITLE02 = "              CardDemo                  ";

/** Endpoint 4's path; the segment is the account-number text, URI-encoded. */
const VIEW_PATH = /^\/api\/v1\/accounts\/[^/]*\/view$/;

/**
 * An unredacted SSN: nine digits, with or without its dashes, standing as a word of its own, so
 * the digits inside a longer code such as the group ID `A000000000` are no SSN.
 */
const FULL_SSN = /(?<!\w)\d{3}-?\d{2}-?\d{4}(?!\w)/;

/** The account block of ACCOUNT: every value distinct, so a field showing another's is seen. */
const accountBlock: components["schemas"]["AccountViewAccountBlock"] = {
  active_status: "Y",
  open_date: "2014-11-20",
  expiration_date: "2025-05-20",
  reissue_date: "2023-05-21",
  credit_limit: "+      2,020.00",
  cash_credit_limit: "+      1,020.00",
  current_balance: "+        194.00",
  current_cycle_credit: "+           .00",
  current_cycle_debit: "-         50.00",
  group_id: "A000000000",
};

/** The customer block of ACCOUNT's customer, every value distinct as well. */
const customerBlock: components["schemas"]["AccountViewCustomerBlock"] = {
  customer_id: "000000050",
  ssn: "***-**-1234",
  date_of_birth: "1961-06-08",
  fico_score: "782",
  first_name: "Immanuel",
  middle_name: "Madeline",
  last_name: "Kessler",
  address_line_1: "618 Deshaun Route",
  address_line_2: "Apt. 802",
  city: "Altenwerthshire",
  state: "NC",
  zip: "12546",
  country: "USA",
  phone_1: "(908)119-8310",
  phone_2: "(373)693-8684",
  government_id: "DL NC 7731 0050",
  eft_account_id: "0053581756",
  primary_card_holder: "N",
};

/** Endpoint 4's answer for ACCOUNT: the key and both blocks. */
const view: components["schemas"]["AccountViewResponse"] = {
  acct_id: ACCOUNT,
  ...accountBlock,
  ...customerBlock,
};

/** The BMS names of the account block's ten output fields (COACTVWC-010 to COACTVWC-015). */
const ACCOUNT_OUTPUTS: ReadonlySet<string> = new Set([
  "ACSTTUS",
  "ADTOPEN",
  "ACRDLIM",
  "AEXPDT",
  "ACSHLIM",
  "AREISDT",
  "ACURBAL",
  "ACRCYCR",
  "AADDGRP",
  "ACRCYDB",
]);

/** The 15-character `+ZZZ,ZZZ,ZZZ.99` text of zero. */
const ZERO_AMOUNT = "+           .00";

/**
 * An account block whose values stand for the placeholder endpoint 4 sends beside a customer it
 * found when the account read did not succeed (halt candidate X-COACTVWC-H1), and what each of
 * its fields shows: the values as served.
 */
const unreadAccountBlock: components["schemas"]["AccountViewAccountBlock"] = {
  active_status: " ",
  open_date: null,
  expiration_date: null,
  reissue_date: null,
  credit_limit: ZERO_AMOUNT,
  cash_credit_limit: ZERO_AMOUNT,
  current_balance: ZERO_AMOUNT,
  current_cycle_credit: ZERO_AMOUNT,
  current_cycle_debit: ZERO_AMOUNT,
  group_id: " ".repeat(10),
};
const UNREAD_ACCOUNT_SHOWS: Readonly<Record<string, string>> = {
  ACSTTUS: " ",
  ADTOPEN: "",
  ACRDLIM: ZERO_AMOUNT,
  AEXPDT: "",
  ACSHLIM: ZERO_AMOUNT,
  AREISDT: "",
  ACURBAL: ZERO_AMOUNT,
  ACRCYCR: ZERO_AMOUNT,
  AADDGRP: " ".repeat(10),
  ACRCYDB: ZERO_AMOUNT,
};

/**
 * The 28 output fields of the map and their captions, from screens.md COACTVW-015 to
 * COACTVW-094: BMS name, `POS`, `LENGTH`, the caption's `POS`, `LENGTH` and INITIAL, and the
 * value the field shows for `view`. `ACSADL2` (COACTVW-072) has no caption.
 */
const OUTPUTS: readonly {
  readonly name: string;
  readonly row: number;
  readonly col: number;
  readonly length: number;
  readonly label: {
    readonly text: string;
    readonly row: number;
    readonly col: number;
    readonly length: number;
  } | null;
  readonly shows: string;
}[] = [
  {
    name: "ACSTTUS",
    row: 5,
    col: 70,
    length: 1,
    label: { text: "Active Y/N: ", row: 5, col: 57, length: 12 },
    shows: "Y",
  },
  {
    name: "ADTOPEN",
    row: 6,
    col: 17,
    length: 10,
    label: { text: "Opened:", row: 6, col: 8, length: 7 },
    shows: "2014-11-20",
  },
  {
    name: "ACRDLIM",
    row: 6,
    col: 61,
    length: 15,
    label: { text: "Credit Limit        :", row: 6, col: 39, length: 21 },
    shows: "+      2,020.00",
  },
  {
    name: "AEXPDT",
    row: 7,
    col: 17,
    length: 10,
    label: { text: "Expiry:", row: 7, col: 8, length: 7 },
    shows: "2025-05-20",
  },
  {
    name: "ACSHLIM",
    row: 7,
    col: 61,
    length: 15,
    label: { text: "Cash credit Limit   :", row: 7, col: 39, length: 21 },
    shows: "+      1,020.00",
  },
  {
    name: "AREISDT",
    row: 8,
    col: 17,
    length: 10,
    label: { text: "Reissue:", row: 8, col: 8, length: 8 },
    shows: "2023-05-21",
  },
  {
    name: "ACURBAL",
    row: 8,
    col: 61,
    length: 15,
    label: { text: "Current Balance     :", row: 8, col: 39, length: 21 },
    shows: "+        194.00",
  },
  {
    name: "ACRCYCR",
    row: 9,
    col: 61,
    length: 15,
    label: { text: "Current Cycle Credit:", row: 9, col: 39, length: 21 },
    shows: "+           .00",
  },
  {
    name: "AADDGRP",
    row: 10,
    col: 23,
    length: 10,
    label: { text: "Account Group:", row: 10, col: 8, length: 14 },
    shows: "A000000000",
  },
  {
    name: "ACRCYDB",
    row: 10,
    col: 61,
    length: 15,
    label: { text: "Current Cycle Debit :", row: 10, col: 39, length: 21 },
    shows: "-         50.00",
  },
  {
    name: "ACSTNUM",
    row: 12,
    col: 23,
    length: 9,
    label: { text: "Customer id  :", row: 12, col: 8, length: 14 },
    shows: "000000050",
  },
  {
    name: "ACSTSSN",
    row: 12,
    col: 54,
    length: 12,
    label: { text: "SSN:", row: 12, col: 49, length: 4 },
    shows: "***-**-1234",
  },
  {
    name: "ACSTDOB",
    row: 13,
    col: 23,
    length: 10,
    label: { text: "Date of birth:", row: 13, col: 8, length: 14 },
    shows: "1961-06-08",
  },
  {
    name: "ACSTFCO",
    row: 13,
    col: 61,
    length: 3,
    label: { text: "FICO Score:", row: 13, col: 49, length: 11 },
    shows: "782",
  },
  {
    name: "ACSFNAM",
    row: 15,
    col: 1,
    length: 25,
    label: { text: "First Name", row: 14, col: 1, length: 10 },
    shows: "Immanuel",
  },
  {
    name: "ACSMNAM",
    row: 15,
    col: 28,
    length: 25,
    label: { text: "Middle Name: ", row: 14, col: 28, length: 13 },
    shows: "Madeline",
  },
  {
    name: "ACSLNAM",
    row: 15,
    col: 55,
    length: 25,
    label: { text: "Last Name : ", row: 14, col: 55, length: 12 },
    shows: "Kessler",
  },
  {
    name: "ACSADL1",
    row: 16,
    col: 10,
    length: 50,
    label: { text: "Address:", row: 16, col: 1, length: 8 },
    shows: "618 Deshaun Route",
  },
  {
    name: "ACSSTTE",
    row: 16,
    col: 73,
    length: 2,
    label: { text: "State ", row: 16, col: 63, length: 6 },
    shows: "NC",
  },
  { name: "ACSADL2", row: 17, col: 10, length: 50, label: null, shows: "Apt. 802" },
  {
    name: "ACSZIPC",
    row: 17,
    col: 73,
    length: 5,
    label: { text: "Zip", row: 17, col: 63, length: 3 },
    shows: "12546",
  },
  {
    name: "ACSCITY",
    row: 18,
    col: 10,
    length: 50,
    label: { text: "City ", row: 18, col: 1, length: 5 },
    shows: "Altenwerthshire",
  },
  {
    name: "ACSCTRY",
    row: 18,
    col: 73,
    length: 3,
    label: { text: "Country", row: 18, col: 63, length: 7 },
    shows: "USA",
  },
  {
    name: "ACSPHN1",
    row: 19,
    col: 10,
    length: 13,
    label: { text: "Phone 1:", row: 19, col: 1, length: 8 },
    shows: "(908)119-8310",
  },
  {
    name: "ACSGOVT",
    row: 19,
    col: 58,
    length: 20,
    label: { text: "Government Issued Id Ref    : ", row: 19, col: 24, length: 30 },
    shows: "DL NC 7731 0050",
  },
  {
    name: "ACSPHN2",
    row: 20,
    col: 10,
    length: 13,
    label: { text: "Phone 2:", row: 20, col: 1, length: 8 },
    shows: "(373)693-8684",
  },
  {
    name: "ACSEFTC",
    row: 20,
    col: 41,
    length: 10,
    label: { text: "EFT Account Id: ", row: 20, col: 24, length: 16 },
    shows: "0053581756",
  },
  {
    name: "ACSPFLG",
    row: 20,
    col: 78,
    length: 1,
    label: { text: "Primary Card Holder Y/N:", row: 20, col: 53, length: 24 },
    shows: "N",
  },
];

/** A JSON reply of the stubbed API. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces `fetch` with a stub of endpoint 4 that answers with `answer` for the decoded account
 * segment, and 500 for any other request. Returns every request received, in order.
 */
function stubFetch(
  answer: (acctId: string) => Response | Promise<Response> = () =>
    json(500, { message: "unexpected request" }),
): { method: string; path: string; authorization: string | null }[] {
  const requests: { method: string; path: string; authorization: string | null }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      const { pathname } = new URL(req.url);
      requests.push({
        method: req.method,
        path: pathname,
        authorization: req.headers.get("Authorization"),
      });
      if (req.method !== "GET" || !VIEW_PATH.test(pathname)) {
        return json(500, { message: `unexpected ${req.method} ${pathname}` });
      }
      return answer(decodeURIComponent(pathname.split("/")[4] ?? ""));
    }),
  );
  return requests;
}

/** Renders the screen at its route, with a text stub at the main menu, its only exit. */
function renderScreen(state?: unknown): ReturnType<typeof render> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COACTVW, state }]}>
        <Routes>
          <Route path={PATHS.COACTVW} element={<COACTVW />} />
          <Route path={PATHS.COMEN01} element={<p>COMEN01 main menu stub</p>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The input whose `data-field` is the BMS field `name`. */
function field(name: string): HTMLInputElement {
  const input = document.querySelector(`input[data-field="${name}"]`);
  if (!(input instanceof HTMLInputElement)) {
    throw new Error(`no input for BMS field ${name}`);
  }
  return input;
}

/**
 * The header field whose `data-field` is the BMS field `name`: the one `Input` of that name in the
 * `CardHeader` title rows of `ScreenShell`, failing the test when there is not exactly one.
 */
function headerField(name: string): HTMLInputElement {
  const fields = document.querySelectorAll(
    `[data-slot="card-header"] input[data-field="${name}"]`,
  );
  expect(fields).toHaveLength(1);
  const [input] = fields;
  if (!(input instanceof HTMLInputElement)) {
    throw new Error(`no header input for BMS field ${name}`);
  }
  return input;
}

/** The value of the header field whose `data-field` is the BMS field `name`. */
function headerValue(name: string): string {
  return headerField(name).value;
}

/** The one message row of `kind` (MessageLine). */
function messageRow(kind: "error" | "info"): HTMLElement {
  const rows = screen.getAllByRole("alert").filter((row) => row.dataset.messageKind === kind);
  expect(rows).toHaveLength(1);
  return rows[0];
}

/** The element whose shadcn/ui `data-slot` is `name`. */
function slot(container: HTMLElement, name: string): Element {
  const element = container.querySelector(`[data-slot="${name}"]`);
  if (element === null) {
    throw new Error(`no ${name}`);
  }
  return element;
}

/**
 * Every grid cell placed under `grid`, as its placement classes and its content: the
 * `data-field` it shows, or else its text. Sorted, so DOM order does not matter.
 */
function cellsOf(grid: Element): string[] {
  return Array.from(grid.querySelectorAll('[class^="row-start-"]'), (cell) => {
    const named = cell.querySelector("[data-field]");
    const content =
      named instanceof HTMLElement
        ? `field ${named.dataset.field ?? ""}`
        : `text ${cell.textContent ?? ""}`;
    return `${cell.className} ${content}`;
  }).sort();
}

/**
 * Every grid cell placed under the body grid `body`, in document order, which is the reading
 * order: its map row (the body grid's line plus the four header rows) and its content as
 * `cellsOf` describes it.
 */
function bodyCellsInOrder(body: Element): { row: number; content: string }[] {
  return Array.from(body.querySelectorAll('[class^="row-start-"]'), (cell) => {
    const line = /(?:^|\s)row-start-(\d+)(?:\s|$)/.exec(cell.className)?.[1];
    if (line === undefined) {
      throw new Error(`no row-start class on ${cell.className}`);
    }
    const named = cell.querySelector("[data-field]");
    const content =
      named instanceof HTMLElement
        ? `field ${named.dataset.field ?? ""}`
        : `text ${cell.textContent ?? ""}`;
    return { row: Number.parseInt(line, 10) + 4, content };
  });
}

/** A grid cell at a map `POS` and `LENGTH`, as `cellsOf` describes it. */
function cell(gridRow: number, col: number, length: number, content: string): string {
  return `row-start-${gridRow} col-start-${col} col-span-${length} ${content}`;
}

/**
 * A body cell at map row `row`: the body grid's line 1 is map row 5, because `ScreenShell`'s
 * header renders map rows 1 to 4.
 */
function bodyCell(row: number, col: number, length: number, content: string): string {
  return cell(row - 4, col, length, content);
}

/** Types `value` into the account number, replacing its text. */
function typeAccount(value: string): void {
  fireEvent.change(field("ACCTSID"), { target: { value } });
}

/** Clicks the ENTER button of the key row. */
function pressEnterButton(): void {
  fireEvent.click(screen.getByRole("button", { name: "ENTER" }));
}

/** The values of the 28 output fields, by BMS name. */
function outputValues(): Record<string, string> {
  return Object.fromEntries(OUTPUTS.map((output) => [output.name, field(output.name).value]));
}

/** Every output field blank: no account or customer block shown. */
const NO_BLOCK: Record<string, string> = Object.fromEntries(
  OUTPUTS.map((output) => [output.name, ""]),
);

/** The screen's form, which holds the account number. */
function accountForm(): HTMLFormElement {
  const form = field("ACCTSID").form;
  if (form === null) {
    throw new Error("the account number belongs to no form");
  }
  return form;
}

/**
 * The keyboard locked while a view is awaited, or open again: ENTER disabled, the key row and the
 * form busy and the account number read-only. F3 sends no request and is available either way.
 */
function expectLocked(locked: boolean): void {
  const keyRow = screen.getByRole("group", { name: "Function keys" });
  const ariaDisabled = (name: string): string | null =>
    within(keyRow).getByRole("button", { name }).getAttribute("aria-disabled");
  expect({
    enterDisabled: ariaDisabled("ENTER"),
    exitDisabled: ariaDisabled("F3=Exit"),
    keyRowBusy: keyRow.getAttribute("aria-busy"),
    formBusy: accountForm().getAttribute("aria-busy"),
    accountReadOnly: field("ACCTSID").readOnly,
  }).toEqual({
    enterDisabled: locked ? "true" : null,
    exitDisabled: null,
    keyRowBusy: String(locked),
    formBusy: String(locked),
    accountReadOnly: locked,
  });
}

/**
 * Replaces `fetch` with a stub of endpoint 4 that holds each view until the test answers it.
 * Returns the requests received and the answers still awaited, in order.
 */
function stubHeldFetch(): {
  requests: ReturnType<typeof stubFetch>;
  answers: ((response: Response) => void)[];
} {
  const answers: ((response: Response) => void)[] = [];
  const requests = stubFetch(
    () =>
      new Promise<Response>((resolve) => {
        answers.push(resolve);
      }),
  );
  return { requests, answers };
}

/** The account-number field in the red of an error, or in its default colour. */
function expectAccountColour(red: boolean): void {
  const account = field("ACCTSID");
  expect(account.getAttribute("aria-invalid")).toBe(String(red));
  expect(account.classList.contains("text-destructive")).toBe(red);
}

/**
 * The cursor in the account-number field, its text selected for overtyping. The screen moves it
 * there after each response through react-hook-form's `setFocus`, which acts on a timer.
 */
async function expectCursorInAccount(): Promise<void> {
  const account = field("ACCTSID");
  await waitFor(() => {
    expect(document.activeElement).toBe(account);
    expect([account.selectionStart, account.selectionEnd]).toEqual([0, account.value.length]);
  });
}

/** Endpoint 4's 400 for an account number sent blank (COACTVWC-007). */
const NO_INPUT: components["schemas"]["ScreenError"] = {
  message: "No input received",
  field: "acct_id",
};

/** Endpoint 4's 404 of the cross-reference read, which names the account number (COACTVWC-035). */
const XREF_NOT_FOUND: components["schemas"]["ScreenError"] = {
  message: `Account:${OTHER_ACCOUNT} not found in Cross ref file.  Resp:000000000  Reas:0000`,
  field: "acct_id",
};

/** An account whose customer read fails, answered by {@link CUSTOMER_NOT_FOUND}. */
const MISSING_CUSTOMER_ACCOUNT = "00000000098";

/** Endpoint 4's 404 of the customer read, which names no field (COACTVWC-044). */
const CUSTOMER_NOT_FOUND: components["schemas"]["ScreenError"] = {
  message: "CustId:000000098 not found in customer master.Resp: 000000000  REAS:0000000",
};

/** The schema's MUSTFILL message for a partly filled account number (COACTVW-013). */
const MUSTFILL = "String must contain exactly 11 character(s)";

/** Types `typed` as the account number, clicks ENTER and waits until the error row shows `text`. */
async function enterUntil(typed: string, text: string): Promise<void> {
  typeAccount(typed);
  pressEnterButton();
  await waitFor(() => {
    expect(messageRow("error").textContent).toBe(text);
  });
}

/** Each id an `aria-describedby` of the document names that resolves to no element. */
function danglingDescriptions(): string[] {
  return [...document.querySelectorAll("[aria-describedby]")].flatMap((element) =>
    (element.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .filter((id) => id === "" || document.getElementById(id) === null),
  );
}

/**
 * How assistive technology reads the input: its `aria-invalid`, and the element each id of its
 * `aria-describedby` names, as its text and whether `sr-only` hides it (`null` for an id that
 * resolves to nothing). `described` is `null` when the input has no `aria-describedby`.
 */
function readOut(element: HTMLInputElement): {
  invalid: string | null;
  described: ({ text: string | null; hidden: boolean } | null)[] | null;
} {
  const ids = element.getAttribute("aria-describedby");
  return {
    invalid: element.getAttribute("aria-invalid"),
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

/** Every element that shows exactly `text` and is not hidden by `sr-only`. */
function visibleCopies(text: string): HTMLElement[] {
  return screen
    .queryAllByText(text, { normalizer: (content) => content })
    .filter((element) => element.closest(".sr-only") === null);
}

/** The account number unmarked: valid, and described by nothing, as it has no hint. */
const UNMARKED = { invalid: "false", described: null };

/** The account number marked by `message`: invalid, described by its hidden field message. */
function markedBy(message: string): {
  invalid: string;
  described: { text: string; hidden: boolean }[];
} {
  return { invalid: "true", described: [{ text: message, hidden: true }] };
}


describe("COACTVW: account view (CAVW, COACTVWC)", () => {
  beforeEach(() => {
    setSession("test-token", "USER0001", "U");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
    sessionStorage.clear();
  });

  // audit: COACTVWC-001
  // audit: COACTVWC-006
  // audit: COACTVWC-027
  // audit: COACTVW-011
  // audit: COACTVW-096
  // audit: COACTVW-100
  // audit: COACTVW-101
  // audit: COACTVW-102
  // audit: COACTVW-103
  // audit: COACTVW-104
  // audit: COACTVW-105
  // audit: COACTVW-106
  it("opens on an empty account number holding the cursor, no values, the prompt, an empty error row and the map's keys, sending no request", () => {
    const requests = stubFetch();
    renderScreen();

    const title = screen.getByRole("heading", { level: 1, name: "View Account" });
    expect(title.dataset.slot).toBe("card-title");
    const account = screen.getByRole("textbox", { name: "Account Number :" });
    expect(account).toBe(field("ACCTSID"));
    expect(field("ACCTSID").value).toBe("");
    expect(field("ACCTSID").maxLength).toBe(11);
    expect(document.activeElement).toBe(account);
    expect(outputValues()).toEqual(NO_BLOCK);

    const info = messageRow("info");
    expect(info.textContent).toBe(PROMPT);
    expect(info.className).toContain("text-card-foreground");
    expect(info.getAttribute("aria-live")).toBe("polite");
    // INFOMSG is not `BRT` on the map, and COACTVWC never brightens it: normal weight.
    const description = info.querySelector('[data-slot="alert-description"]');
    expect(description?.classList.contains("font-semibold")).toBe(false);
    expect(messageRow("error").textContent).toBe("");

    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(keys.map((key) => key.textContent)).toEqual(["ENTER", "F3=Exit"]);

    expect(headerValue("TRNNAME")).toBe("CAVW");
    expect(headerValue("PGMNAME")).toBe("COACTVWC");
    expect(headerValue("TITLE01")).toBe(TITLE01);
    expect(headerValue("TITLE02")).toBe(TITLE02);
    expect(headerValue("CURDATE")).toMatch(/^\d{2}\/\d{2}\/\d{2}$/);
    expect(headerValue("CURTIME")).toMatch(/^\d{2}:\d{2}:\d{2}$/);

    expect(requests).toEqual([]);
  });

  // audit: COACTVW-012
  // audit: COACTVW-014
  // audit: COACTVW-015
  // audit: COACTVW-017
  // audit: COACTVW-018
  // audit: COACTVW-020
  // audit: COACTVW-021
  // audit: COACTVW-023
  // audit: COACTVW-024
  // audit: COACTVW-026
  // audit: COACTVW-027
  // audit: COACTVW-029
  // audit: COACTVW-030
  // audit: COACTVW-032
  // audit: COACTVW-033
  // audit: COACTVW-035
  // audit: COACTVW-036
  // audit: COACTVW-038
  // audit: COACTVW-039
  // audit: COACTVW-041
  // audit: COACTVW-042
  // audit: COACTVW-044
  // audit: COACTVW-045
  // audit: COACTVW-046
  // audit: COACTVW-048
  // audit: COACTVW-049
  // audit: COACTVW-051
  // audit: COACTVW-052
  // audit: COACTVW-054
  // audit: COACTVW-055
  // audit: COACTVW-057
  // audit: COACTVW-058
  // audit: COACTVW-059
  // audit: COACTVW-060
  // audit: COACTVW-062
  // audit: COACTVW-064
  // audit: COACTVW-066
  // audit: COACTVW-068
  // audit: COACTVW-069
  // audit: COACTVW-071
  // audit: COACTVW-073
  // audit: COACTVW-074
  // audit: COACTVW-076
  // audit: COACTVW-077
  // audit: COACTVW-079
  // audit: COACTVW-080
  // audit: COACTVW-082
  // audit: COACTVW-083
  // audit: COACTVW-085
  // audit: COACTVW-087
  // audit: COACTVW-088
  // audit: COACTVW-090
  // audit: COACTVW-092
  // audit: COACTVW-093
  // audit: COACTVW-095
  // audit: COACTVW-097
  // audit: COACTVW-098
  it("places each literal and field at its map POS and LENGTH, binds each caption to its field, and renders nothing for the stoppers or the unnamed (1,1) field", () => {
    stubFetch();
    const { container } = renderScreen();

    // The body grid holds exactly the map's literals and named fields: the zero-length stoppers
    // between them have no element.
    const body = container.querySelector("form")?.firstElementChild;
    if (body === null || body === undefined) {
      throw new Error("no body grid");
    }
    expect(cellsOf(body)).toEqual(
      [
        bodyCell(5, 19, 16, "text Account Number :"),
        bodyCell(5, 38, 11, "field ACCTSID"),
        bodyCell(11, 32, 16, "text Customer Details"),
        ...OUTPUTS.flatMap((output) => [
          ...(output.label === null
            ? []
            : [
                bodyCell(
                  output.label.row,
                  output.label.col,
                  output.label.length,
                  `text ${output.label.text}`,
                ),
              ]),
          bodyCell(output.row, output.col, output.length, `field ${output.name}`),
        ]),
      ].sort(),
    );

    // Header rows 1 and 2 hold their ten fields only: nothing for the unnamed nine-position
    // field declared at (1,1) over `Tran:`.
    const header = slot(container, "card-header").firstElementChild;
    if (header === null) {
      throw new Error("no header grid");
    }
    expect(cellsOf(header)).toEqual(
      [
        cell(1, 1, 5, "text Tran:"),
        cell(1, 7, 4, "field TRNNAME"),
        cell(1, 21, 40, "field TITLE01"),
        cell(1, 65, 5, "text Date:"),
        cell(1, 71, 8, "field CURDATE"),
        cell(2, 1, 5, "text Prog:"),
        cell(2, 7, 8, "field PGMNAME"),
        cell(2, 21, 40, "field TITLE02"),
        cell(2, 65, 5, "text Time:"),
        cell(2, 71, 8, "field CURTIME"),
      ].sort(),
    );

    // Row 22 holds the information row only, row 23 the error row, row 24 the keys.
    const footer = Array.from(slot(container, "card-footer").children);
    expect(footer).toHaveLength(3);
    expect(footer[0]).toBe(messageRow("info"));
    expect(footer[1]).toBe(messageRow("error"));
    expect(footer[2]).toBe(screen.getByRole("group", { name: "Function keys" }));

    const accountLabels = Array.from(field("ACCTSID").labels ?? []);
    expect(accountLabels.map((label) => [label.textContent, label.dataset.slot])).toEqual([
      ["Account Number :", "form-label"],
    ]);
    expect(screen.getByRole("heading", { level: 2, name: "Customer Details" })).toBeTruthy();
    for (const output of OUTPUTS) {
      const labels = Array.from(field(output.name).labels ?? []);
      expect({
        name: output.name,
        captions: labels.map((label) => [label.textContent, label.dataset.slot]),
      }).toEqual({
        name: output.name,
        captions: output.label === null ? [] : [[output.label.text, "label"]],
      });
    }
    expect(field("ACSADL2").getAttribute("aria-label")).toBe("Address line 2");
  });

  it("reads in map row order: the account block, the Customer Details heading, then the customer block, each caption just before its field", () => {
    stubFetch();
    const { container } = renderScreen();

    const body = container.querySelector("form")?.firstElementChild;
    if (body === null || body === undefined) {
      throw new Error("no body grid");
    }
    const cells = bodyCellsInOrder(body);
    const contents = cells.map((placed) => placed.content);
    const heading = contents.indexOf("text Customer Details");

    // The heading follows the last account field and precedes the first customer caption and
    // field, as it stands between the two blocks on the map.
    expect(contents.slice(heading - 2, heading + 3)).toEqual([
      "text Current Cycle Debit :",
      "field ACRCYDB",
      "text Customer Details",
      "text Customer id  :",
      "field ACSTNUM",
    ]);
    expect(cells[heading]?.row).toBe(11);
    expect(cells.slice(0, heading).filter((placed) => placed.row >= 11)).toEqual([]);
    expect(cells.slice(heading + 1).filter((placed) => placed.row <= 11)).toEqual([]);

    // The fields come in map row order, and each caption is read right before the field it
    // labels, the row-14 name captions included.
    const fieldRows = cells
      .filter((placed) => placed.content.startsWith("field "))
      .map((placed) => placed.row);
    expect(fieldRows).toEqual([...fieldRows].sort((a, b) => a - b));
    const captionsAfterTheirField = cells.flatMap((placed, index) => {
      if (!placed.content.startsWith("field ")) {
        return [];
      }
      const [label] = Array.from(field(placed.content.slice("field ".length)).labels ?? []);
      const before = cells[index - 1]?.content;
      return label === undefined || before === `text ${label.textContent ?? ""}`
        ? []
        : [placed.content];
    });
    expect(captionsAfterTheirField).toEqual([]);

    // The accessibility tree agrees: the level-2 heading lies between the two fields.
    const caption = screen.getByRole("heading", { level: 2, name: "Customer Details" });
    expect(
      field("ACRCYDB").compareDocumentPosition(caption) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      caption.compareDocumentPosition(field("ACSTNUM")) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it.each([
    // audit: COACTVW-002
    {
      id: "COACTVW-002",
      name: "TRNNAME",
      row: 1,
      col: 7,
      length: 4,
      label: "Tran:",
      shows: "CAVW",
    },
    // audit: COACTVW-003
    {
      id: "COACTVW-003",
      name: "TITLE01",
      row: 1,
      col: 21,
      length: 40,
      label: null,
      shows: TITLE01,
    },
    // audit: COACTVW-005
    {
      id: "COACTVW-005",
      name: "CURDATE",
      row: 1,
      col: 71,
      length: 8,
      label: "Date:",
      shows: expect.stringMatching(/^\d{2}\/\d{2}\/\d{2}$/),
    },
    // audit: COACTVW-007
    {
      id: "COACTVW-007",
      name: "PGMNAME",
      row: 2,
      col: 7,
      length: 8,
      label: "Prog:",
      shows: "COACTVWC",
    },
    // audit: COACTVW-008
    {
      id: "COACTVW-008",
      name: "TITLE02",
      row: 2,
      col: 21,
      length: 40,
      label: null,
      shows: TITLE02,
    },
    // audit: COACTVW-010
    {
      id: "COACTVW-010",
      name: "CURTIME",
      row: 2,
      col: 71,
      length: 8,
      label: "Time:",
      shows: expect.stringMatching(/^\d{2}:\d{2}:\d{2}$/),
    },
  ])(
    "$id shows $name in a read-only Input at ($row,$col), $length wide, in the card header",
    ({ name, row, col, length, label, shows }) => {
      // The header value is an output field of the map: a read-only `Input` that holds `LENGTH`
      // characters (`size`), in the cell of its `POS` and `LENGTH` in the frame's title rows. The
      // map's literal before it is its bound `Label`; a title, which no literal labels, is named
      // by `aria-label`.
      const requests = stubFetch();
      renderScreen();

      const value = headerField(name);
      expect(value.dataset.slot).toBe("input");
      expect(value.readOnly).toBe(true);
      expect(value.size).toBe(length);
      expect(value.parentElement?.className).toBe(
        `row-start-${row} col-start-${col} col-span-${length}`,
      );
      expect(value.value).toEqual(shows);
      const labels = Array.from(value.labels ?? []);
      expect(labels.map((bound) => bound.textContent)).toEqual(label === null ? [] : [label]);
      expect(labels.every((bound) => bound.dataset.slot === "label")).toBe(true);
      expect(screen.getByRole("textbox", { name: label ?? `Title row ${row}` })).toBe(value);
      expect(requests).toEqual([]);
    },
  );

  // audit: COACTVW-016
  // audit: COACTVW-019
  // audit: COACTVW-022
  // audit: COACTVW-025
  // audit: COACTVW-028
  // audit: COACTVW-031
  // audit: COACTVW-034
  // audit: COACTVW-037
  // audit: COACTVW-040
  // audit: COACTVW-043
  // audit: COACTVW-047
  // audit: COACTVW-050
  // audit: COACTVW-053
  // audit: COACTVW-056
  // audit: COACTVW-061
  // audit: COACTVW-063
  // audit: COACTVW-065
  // audit: COACTVW-067
  // audit: COACTVW-070
  // audit: COACTVW-072
  // audit: COACTVW-075
  // audit: COACTVW-078
  // audit: COACTVW-081
  // audit: COACTVW-084
  // audit: COACTVW-086
  // audit: COACTVW-089
  // audit: COACTVW-091
  // audit: COACTVW-094
  // audit: COACTVW-096
  // audit: COACTVWC-007
  // audit: COACTVWC-027
  // audit: COACTVWC-028
  it("ENTER views the account through endpoint 4 with the bearer token, each output read-only at its width showing the response value verbatim", async () => {
    // West of UTC, a `Date` parsed from "2014-11-20" (UTC midnight) falls on 19 November in local
    // time, so the date assertions below fail for any `Date`-based rendering of the typed dates.
    vi.stubEnv("TZ", "Pacific/Honolulu");
    const requests = stubFetch(() => json(200, view));
    renderScreen();

    typeAccount(ACCOUNT);
    pressEnterButton();
    await waitFor(() => {
      expect(field("ACSTSSN").value).toBe("***-**-1234");
    });

    expect(requests).toEqual([
      {
        method: "GET",
        path: `/api/v1/accounts/${ACCOUNT}/view`,
        authorization: "Bearer test-token",
      },
    ]);
    for (const output of OUTPUTS) {
      const input = field(output.name);
      expect({
        name: output.name,
        value: input.value,
        readOnly: input.readOnly,
        size: input.size,
        slot: input.dataset.slot,
      }).toEqual({
        name: output.name,
        value: output.shows,
        readOnly: true,
        size: output.length,
        slot: "input",
      });
    }
    // The typed dates show as their own YYYY-MM-DD text, so no time zone shifts the day.
    expect(field("ADTOPEN").value).toBe("2014-11-20");

    expect(field("ACCTSID").value).toBe(ACCOUNT);
    expectAccountColour(false);
    await expectCursorInAccount();
    expect(messageRow("info").textContent).toBe(PROMPT);
    expect(messageRow("error").textContent).toBe("");
  });

  it("gives the account number and every output the shared text-sm field box of its LENGTH", () => {
    stubFetch();
    renderScreen();
    const boxes = [{ name: "ACCTSID", length: 11 }, ...OUTPUTS].map(({ name, length }) => ({
      name,
      sized: field(name).classList.contains(`field-length-${length}`),
      small: field(name).classList.contains("text-sm"),
    }));
    expect(boxes).toHaveLength(OUTPUTS.length + 1);
    expect(boxes.filter((box) => !box.sized || !box.small)).toEqual([]);
  });

  it("shows a date the endpoint sends as null as a blank field", async () => {
    const blankDates: components["schemas"]["AccountViewResponse"] = {
      ...view,
      open_date: null,
      expiration_date: null,
      reissue_date: null,
      date_of_birth: null,
    };
    stubFetch(() => json(200, blankDates));
    renderScreen();

    typeAccount(ACCOUNT);
    pressEnterButton();
    await waitFor(() => {
      expect(field("ACSTSSN").value).toBe("***-**-1234");
    });

    expect(
      ["ADTOPEN", "AEXPDT", "AREISDT", "ACSTDOB"].map((name) => [name, field(name).value]),
    ).toEqual([
      ["ADTOPEN", ""],
      ["AEXPDT", ""],
      ["AREISDT", ""],
      ["ACSTDOB", ""],
    ]);
  });

  it("shows the SSN only redacted: no nine-digit SSN in the page text or in any output but the customer ID", async () => {
    stubFetch(() => json(200, view));
    const { container } = renderScreen();

    typeAccount(ACCOUNT);
    pressEnterButton();
    await waitFor(() => {
      expect(field("ACSTSSN").value).toBe("***-**-1234");
    });

    expect(container.textContent).not.toMatch(FULL_SSN);
    // ACSTNUM is the nine-digit customer ID (COACTVW-047), not an SSN.
    const shown = OUTPUTS.filter((output) => output.name !== "ACSTNUM").map((output) => [
      output.name,
      field(output.name).value,
    ]);
    expect(shown.filter(([, value]) => FULL_SSN.test(value))).toEqual([]);
  });

  // audit: COACTVW-096
  // audit: COACTVW-099
  // audit: COACTVWC-007
  // audit: COACTVWC-028
  it.each<{
    label: string;
    status: number;
    body: components["schemas"]["ScreenError"];
    red: boolean;
  }>([
    {
      // COACTVWC-035; positions 56-65 and 72-75 hold the interim codes of halt H4.
      label: "cross-reference not-found message, naming acct_id",
      status: 404,
      body: {
        message: `Account:${OTHER_ACCOUNT} not found in Cross ref file.  Resp:000000000  Reas:0000`,
        field: "acct_id",
      },
      red: true,
    },
    {
      // COACTVWC-044; positions 53-62 and 69-75 hold the interim codes of halt H4.
      label: "customer not-found message, naming no field",
      status: 404,
      body: {
        message: "CustId:000000099 not found in customer master.Resp: 000000000  REAS:0000000",
      },
      red: false,
    },
  ])(
    "shows the $label in the error row, clears both blocks and keeps the typed account number",
    async ({ status, body, red }) => {
      const requests = stubFetch((acctId) =>
        acctId === ACCOUNT ? json(200, view) : json(status, body),
      );
      renderScreen();

      typeAccount(ACCOUNT);
      pressEnterButton();
      await waitFor(() => {
        expect(field("ACSTSSN").value).toBe("***-**-1234");
      });
      expect(messageRow("error").textContent).toBe("");

      typeAccount(OTHER_ACCOUNT);
      pressEnterButton();
      await waitFor(() => {
        expect(messageRow("error").textContent).toBe(body.message);
      });

      expect(requests.map((request) => request.path)).toEqual([
        `/api/v1/accounts/${ACCOUNT}/view`,
        `/api/v1/accounts/${OTHER_ACCOUNT}/view`,
      ]);
      const error = messageRow("error");
      expect(error.className).toContain("text-destructive");
      expect(error.getAttribute("aria-live")).toBe("polite");
      expect(outputValues()).toEqual(NO_BLOCK);
      expect(field("ACCTSID").value).toBe(OTHER_ACCOUNT);
      expectAccountColour(red);
      await expectCursorInAccount();
      expect(messageRow("info").textContent).toBe(PROMPT);
    },
  );

  // audit: COACTVW-099
  // audit: COACTVWC-007
  // audit: COACTVWC-028
  it.each<{
    label: string;
    status: number;
    body: components["schemas"]["AccountViewError"];
    accountShows: Readonly<Record<string, string>> | null;
    customerShown: boolean;
    red: boolean;
  }>([
    {
      // COACTVWC-044; positions 53-62 and 69-75 hold the interim codes of halt H4.
      label: "account block beside the customer not-found message",
      status: 404,
      body: {
        message: "CustId:000000099 not found in customer master.Resp: 000000000  REAS:0000000",
        account: accountBlock,
      },
      accountShows: null,
      customerShown: false,
      red: false,
    },
    {
      // COACTVWC-046; positions 49-58 and 66-75 hold the interim codes of halt H4.
      label: "account block beside the customer file error",
      status: 500,
      body: {
        message: "File Error: READ     on CUSTDAT   returned RESP 000000000 ,RESP2 000000000",
        account: accountBlock,
      },
      accountShows: null,
      customerShown: false,
      red: false,
    },
    {
      // COACTVWC-040 and COACTVWC-009: the customer found, the account block a placeholder.
      label: "both blocks beside the account not-found message, naming acct_id",
      status: 404,
      body: {
        message: `Account:${OTHER_ACCOUNT} not found in Acct Master file.Resp:000000000  Reas:0000`,
        field: "acct_id",
        account: unreadAccountBlock,
        customer: customerBlock,
      },
      accountShows: UNREAD_ACCOUNT_SHOWS,
      customerShown: true,
      red: true,
    },
  ])(
    "shows the $label as served, the block not sent blank, beside the message",
    async ({ status, body, accountShows, customerShown, red }) => {
      stubFetch((acctId) => (acctId === ACCOUNT ? json(200, view) : json(status, body)));
      renderScreen();

      typeAccount(ACCOUNT);
      pressEnterButton();
      await waitFor(() => {
        expect(field("ACSTSSN").value).toBe("***-**-1234");
      });

      typeAccount(OTHER_ACCOUNT);
      pressEnterButton();
      await waitFor(() => {
        expect(messageRow("error").textContent).toBe(body.message);
      });

      expect(outputValues()).toEqual(
        Object.fromEntries(
          OUTPUTS.map((output) => {
            if (ACCOUNT_OUTPUTS.has(output.name)) {
              return [output.name, accountShows?.[output.name] ?? output.shows];
            }
            return [output.name, customerShown ? output.shows : ""];
          }),
        ),
      );
      expect(field("ACCTSID").value).toBe(OTHER_ACCOUNT);
      expectAccountColour(red);
      await expectCursorInAccount();
      expect(messageRow("info").textContent).toBe(PROMPT);
    },
  );

  // audit: COACTVWC-007
  // audit: COACTVWC-028
  it("sends an empty field as eleven spaces, and after 'No input received' shows * in the red field", async () => {
    const noInput: components["schemas"]["ScreenError"] = {
      message: "No input received",
      field: "acct_id",
    };
    const requests = stubFetch((acctId) =>
      acctId === " ".repeat(11) ? json(400, noInput) : json(200, view),
    );
    renderScreen();

    pressEnterButton();
    await waitFor(() => {
      expect(messageRow("error").textContent).toBe("No input received");
    });

    expect(requests).toEqual([
      {
        method: "GET",
        path: `/api/v1/accounts/${"%20".repeat(11)}/view`,
        authorization: "Bearer test-token",
      },
    ]);
    expect(field("ACCTSID").value).toBe(`*${" ".repeat(10)}`);
    expectAccountColour(true);
    await expectCursorInAccount();
    expect(outputValues()).toEqual(NO_BLOCK);
  });

  // audit: COACTVWC-007
  // audit: COACTVWC-028
  it.each([
    { label: "non-numeric", acctId: "ABCDEFGHIJ1" },
    { label: "all-zero", acctId: "00000000000" },
  ])(
    "sends a full-length $label account number as typed and shows the endpoint's numeric-edit message",
    async ({ acctId }) => {
      const notNumeric: components["schemas"]["ScreenError"] = {
        message: "Account Filter must  be a non-zero 11 digit number",
        field: "acct_id",
      };
      const requests = stubFetch(() => json(400, notNumeric));
      renderScreen();

      typeAccount(acctId);
      pressEnterButton();
      await waitFor(() => {
        expect(messageRow("error").textContent).toBe(
          "Account Filter must  be a non-zero 11 digit number",
        );
      });

      expect(requests.map((request) => request.path)).toEqual([
        `/api/v1/accounts/${acctId}/view`,
      ]);
      expect(field("ACCTSID").value).toBe(acctId);
      expectAccountColour(true);
      await expectCursorInAccount();
    },
  );

  it("refuses a partly filled account number before sending (MUSTFILL), keeping the blocks shown", async () => {
    const requests = stubFetch(() => json(200, view));
    renderScreen();

    typeAccount(ACCOUNT);
    pressEnterButton();
    await waitFor(() => {
      expect(field("ACSTSSN").value).toBe("***-**-1234");
    });

    typeAccount("123");
    pressEnterButton();
    await waitFor(() => {
      expect(messageRow("error").textContent).toBe("String must contain exactly 11 character(s)");
    });

    expect(requests).toHaveLength(1);
    expect(outputValues()).toEqual(
      Object.fromEntries(OUTPUTS.map((output) => [output.name, output.shows])),
    );
    expect(field("ACCTSID").value).toBe("123");
    expectAccountColour(true);
    // Nothing was sent, so the keyboard is not locked: the next full number is sent.
    expectLocked(false);
    typeAccount(OTHER_ACCOUNT);
    pressEnterButton();
    await waitFor(() => {
      expect(requests).toHaveLength(2);
    });
    expect(requests[1]?.path).toBe(`/api/v1/accounts/${OTHER_ACCOUNT}/view`);
  });

  it("locks the keyboard while a view is awaited: only the first ENTER is sent, every key repeated before the answer is refused, and the next ENTER is sent", async () => {
    const { requests, answers } = stubHeldFetch();
    renderScreen();
    expectLocked(false);

    typeAccount(ACCOUNT);
    const enter = screen.getByRole("button", { name: "ENTER" });
    // Presses in one task, before the screen renders the lock: the button twice, the Enter key
    // and a key COACTVWC processes as ENTER. The lock taken by the first refuses the others.
    act(() => {
      enter.click();
      enter.click();
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "F5" }));
    });
    await waitFor(() => {
      expect(requests).toHaveLength(1);
    });
    expectLocked(true);

    // The same presses on the locked screen, and a submission of its form, send nothing.
    pressEnterButton();
    fireEvent.keyDown(window, { key: "Enter" });
    fireEvent.keyDown(window, { key: "F5" });
    fireEvent.submit(accountForm());
    expectLocked(true);

    act(() => {
      answers[0]?.(json(200, view));
    });
    await waitFor(() => {
      expect(field("ACSTSSN").value).toBe("***-**-1234");
    });
    await waitFor(() => {
      expectLocked(false);
    });

    typeAccount(OTHER_ACCOUNT);
    pressEnterButton();
    await waitFor(() => {
      expect(requests).toHaveLength(2);
    });
    expectLocked(true);
    act(() => {
      answers[1]?.(json(200, { ...view, acct_id: OTHER_ACCOUNT }));
    });
    await waitFor(() => {
      expectLocked(false);
    });

    expect(requests.map((request) => [request.method, request.path])).toEqual([
      ["GET", `/api/v1/accounts/${ACCOUNT}/view`],
      ["GET", `/api/v1/accounts/${OTHER_ACCOUNT}/view`],
    ]);
  });

  it.each([
    {
      label: "the F3=Exit button",
      press: () => {
        fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));
      },
    },
    {
      label: "the F3 key",
      press: () => {
        fireEvent.keyDown(window, { key: "F3" });
      },
    },
  ])("$label stays available while a view is awaited and returns to the main menu", async ({ press }) => {
    const { requests, answers } = stubHeldFetch();
    renderScreen();

    typeAccount(ACCOUNT);
    pressEnterButton();
    await waitFor(() => {
      expect(requests).toHaveLength(1);
    });
    expectLocked(true);
    press();

    expect(await screen.findByText("COMEN01 main menu stub")).toBeTruthy();
    act(() => {
      answers[0]?.(json(200, view));
    });
    expect(requests).toHaveLength(1);
  });

  // audit: COACTVW-107
  // audit: COACTVWC-004
  it.each([
    { label: "route state from the main menu", state: { from: PATHS.COMEN01 } },
    { label: "no route state", state: undefined },
  ])("F3=Exit returns to the main menu with $label, sending no request", async ({ state }) => {
    const requests = stubFetch();
    renderScreen(state);

    const exit = screen.getByRole("button", { name: "F3=Exit" });
    expect([exit.getAttribute("type"), exit.dataset.variant, exit.dataset.size]).toEqual([
      "button",
      "outline",
      "sm",
    ]);
    typeAccount(ACCOUNT);
    fireEvent.click(exit);

    expect(await screen.findByText("COMEN01 main menu stub")).toBeTruthy();
    expect(requests).toEqual([]);
  });

  // audit: COACTVW-107
  // audit: COACTVWC-003
  // audit: COACTVWC-004
  it.each([
    { label: "F3", shiftKey: false },
    { label: "Shift+F3 (PF15)", shiftKey: true },
  ])("the $label key exits to the main menu, sending no request", async ({ shiftKey }) => {
    const requests = stubFetch();
    renderScreen();

    typeAccount(ACCOUNT);
    fireEvent.keyDown(document.body, { key: "F3", shiftKey });

    expect(await screen.findByText("COMEN01 main menu stub")).toBeTruthy();
    expect(requests).toEqual([]);
  });

  // audit: COACTVW-108
  // audit: COACTVWC-003
  it.each([
    { label: "the ENTER button", press: pressEnterButton },
    {
      label: "the Enter key",
      press: () => {
        fireEvent.keyDown(document.body, { key: "Enter" });
      },
    },
    {
      label: "an unserved key (F5)",
      press: () => {
        fireEvent.keyDown(document.body, { key: "F5" });
      },
    },
    {
      label: "an unserved shifted key (Shift+F5, PF17)",
      press: () => {
        fireEvent.keyDown(document.body, { key: "F5", shiftKey: true });
      },
    },
  ])("$label sends the account number to endpoint 4", async ({ press }) => {
    const requests = stubFetch(() => json(200, view));
    renderScreen();

    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    const enter = keys[0];
    expect([
      enter.textContent,
      enter.getAttribute("type"),
      enter.dataset.variant,
      enter.dataset.size,
    ]).toEqual(["ENTER", "submit", "default", "sm"]);

    typeAccount(ACCOUNT);
    press();
    await waitFor(() => {
      expect(field("ACSTSSN").value).toBe("***-**-1234");
    });

    expect(requests.map((request) => [request.method, request.path])).toEqual([
      ["GET", `/api/v1/accounts/${ACCOUNT}/view`],
    ]);
  });

  // The replacement's accessibility wiring, which no audit row states: the account number names in
  // `aria-describedby` only elements the screen renders. It has no hint, so it is described by
  // nothing while unmarked. A message that names it, the server's or the schema's MUSTFILL
  // message, marks it invalid and describes it by its hidden field message, which repeats the
  // error row's text; a message that names no field, and a view, leave it unmarked.
  it.each<{
    state: string;
    reach: () => Promise<void>;
    expected: { invalid: string; described: { text: string; hidden: boolean }[] | null };
  }>([
    { state: "on entry", reach: async () => undefined, expected: UNMARKED },
    {
      state: "after the 400 'No input received' of a blank entry",
      reach: () => enterUntil("", NO_INPUT.message),
      expected: markedBy(NO_INPUT.message),
    },
    {
      state: "after the 404 of the cross-reference, naming acct_id",
      reach: () => enterUntil(OTHER_ACCOUNT, XREF_NOT_FOUND.message),
      expected: markedBy(XREF_NOT_FOUND.message),
    },
    {
      state: "after the 404 of the customer, naming no field",
      reach: () => enterUntil(MISSING_CUSTOMER_ACCOUNT, CUSTOMER_NOT_FOUND.message),
      expected: UNMARKED,
    },
    {
      state: "after the MUSTFILL rejection of a partly filled number",
      reach: () => enterUntil("123", MUSTFILL),
      expected: markedBy(MUSTFILL),
    },
    {
      state: "after a view that follows 'No input received'",
      reach: async () => {
        await enterUntil("", NO_INPUT.message);
        typeAccount(ACCOUNT);
        pressEnterButton();
        await waitFor(() => {
          expect(field("ACSTSSN").value).toBe("***-**-1234");
        });
      },
      expected: UNMARKED,
    },
  ])(
    "describes the account number by rendered elements only, $state",
    async ({ reach, expected }) => {
      stubFetch((acctId) => {
        if (acctId === " ".repeat(11)) {
          return json(400, NO_INPUT);
        }
        if (acctId === OTHER_ACCOUNT) {
          return json(404, XREF_NOT_FOUND);
        }
        return acctId === MISSING_CUSTOMER_ACCOUNT
          ? json(404, CUSTOMER_NOT_FOUND)
          : json(200, view);
      });
      renderScreen();
      await reach();

      await waitFor(() => {
        expect(readOut(field("ACCTSID"))).toEqual(expected);
      });
      expect(danglingDescriptions()).toEqual([]);
      // The error row stays the one visible copy of the text that marks the input.
      for (const { text } of expected.described ?? []) {
        const visible = visibleCopies(text);
        expect(visible).toHaveLength(1);
        expect(messageRow("error").contains(visible[0] ?? null)).toBe(true);
      }
    },
  );

});

/** The 401 of the `SignedOnUser` extractor for a missing, invalid or expired token. */
const signonRequired: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};

/**
 * Renders the application's route table at `/accounts/view`, so the sign-on guard that only the
 * table mounts stands in front of the screen. Each screen is imported by its route's loader, so a
 * wait for one to render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COACTVW] });
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
describe("COACTVW in the route table: a 401 ends the session", () => {
  beforeEach(() => {
    setSession("test-token", "USER0001", "U");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  it("ENTER answered 401 replaces the screen with sign-on and clears the session, with no retry", async () => {
    const requests = stubFetch(() => json(401, signonRequired));
    const router = renderRouteTable();
    expect(
      await screen.findByRole("heading", { level: 1, name: "View Account" }, { timeout: 5_000 }),
    ).toBeDefined();

    typeAccount(ACCOUNT);
    pressEnterButton();

    expect(
      await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
    ).toBeDefined();
    expect(router.state.location.pathname).toBe(PATHS.COSGN00);
    expect(screen.queryByRole("heading", { level: 1, name: "View Account" })).toBeNull();
    expect(getSession()).toBeNull();
    expect(requests).toEqual([
      {
        method: "GET",
        path: `/api/v1/accounts/${ACCOUNT}/view`,
        authorization: "Bearer test-token",
      },
    ]);
    router.dispose();
  });

  /**
   * The body of a 401 whose status and headers have arrived: its first bytes come, and then the
   * rest never does, or the connection fails. `release` fails a body still open, so the request
   * settles, and its deadline timer is cleared, once the test is done.
   */
  function partialBody(fails: boolean): { body: ReadableStream<Uint8Array>; release: () => void } {
    let open: ReadableStreamDefaultController<Uint8Array> | undefined;
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        open = controller;
        controller.enqueue(new TextEncoder().encode(JSON.stringify(signonRequired).slice(0, 20)));
      },
      // Asked for more once the first bytes are read: the connection fails, or nothing comes.
      pull(controller) {
        if (fails) {
          controller.error(new TypeError("network error"));
        }
      },
    });
    return {
      body,
      release: () => {
        open?.error(new TypeError("the test is done"));
      },
    };
  }

  // The session ends on the 401's status, before its body is awaited, so it ends long before the
  // request's 30 s deadline: within `waitFor`'s 1 s default.
  it.each([
    { title: "whose body never finishes", fails: false },
    { title: "whose body fails part-way", fails: true },
  ])(
    "ENTER answered 401 $title clears the session at once and replaces the screen with sign-on, with no retry",
    async ({ fails }) => {
      const { body, release } = partialBody(fails);
      onTestFinished(release);
      const requests = stubFetch(
        () => new Response(body, { status: 401, headers: { "Content-Type": "application/json" } }),
      );
      const router = renderRouteTable();
      expect(
        await screen.findByRole("heading", { level: 1, name: "View Account" }, { timeout: 5_000 }),
      ).toBeDefined();

      typeAccount(ACCOUNT);
      pressEnterButton();

      await waitFor(() => {
        expect(getSession()).toBeNull();
      });
      expect(
        await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
      ).toBeDefined();
      expect(router.state.location.pathname).toBe(PATHS.COSGN00);
      expect(screen.queryByRole("heading", { level: 1, name: "View Account" })).toBeNull();
      expect(requests).toEqual([
        {
          method: "GET",
          path: `/api/v1/accounts/${ACCOUNT}/view`,
          authorization: "Bearer test-token",
        },
      ]);
      router.dispose();
    },
  );
});

// Endpoint 4 answers a failed edit 400, a missing record 404 and a failed read 500, each with a
// `ScreenError`. The screen shows that body's message as it came, and only a 401 ends the session.
describe("COACTVW: a ScreenError reply keeps the session", () => {
  beforeEach(() => {
    setSession("test-token", "USER0001", "U");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  /** Every request `fetch` received: method, path, query, `Authorization` header and body text. */
  async function sentRequests() {
    return Promise.all(
      vi.mocked(fetch).mock.calls.map(async ([input, init]) => {
        const request = input instanceof Request ? input : new Request(input, init);
        const url = new URL(request.url);
        return {
          method: request.method,
          path: url.pathname,
          query: Object.fromEntries(url.searchParams),
          authorization: request.headers.get("Authorization"),
          body: await request.clone().text(),
        };
      }),
    );
  }

  it.each<{ status: number; typed: string; body: components["schemas"]["ScreenError"] }>([
    {
      status: 400,
      typed: "00000000000",
      body: { message: "Account Filter must  be a non-zero 11 digit number", field: "acct_id" },
    },
    {
      status: 404,
      typed: ACCOUNT,
      body: {
        message: `Account:${ACCOUNT} not found in Cross ref file.  Resp:000000000  Reas:0000`,
        field: "acct_id",
      },
    },
    {
      status: 500,
      typed: ACCOUNT,
      body: {
        message: "File Error: READ     on CXACAIX   returned RESP 0000000013,RESP2 0000000000",
        field: "acct_id",
      },
    },
  ])(
    "a $status shows its message unchanged, keeps the session, after one GET with the bearer token and no query or body",
    async ({ status, typed, body }) => {
      stubFetch(() => json(status, body));
      renderScreen();

      typeAccount(typed);
      pressEnterButton();
      await waitFor(() => {
        expect(messageRow("error").textContent).toBe(body.message);
      });

      expect(getSession()).toEqual({ token: "test-token", userId: "USER0001", userType: "U" });
      expect(outputValues()).toEqual(NO_BLOCK);
      expect(await sentRequests()).toEqual([
        {
          method: "GET",
          path: `/api/v1/accounts/${typed}/view`,
          query: {},
          authorization: "Bearer test-token",
          body: "",
        },
      ]);
    },
  );
});
