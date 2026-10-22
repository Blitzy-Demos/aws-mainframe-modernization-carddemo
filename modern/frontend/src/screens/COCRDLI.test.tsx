import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import {
  createMemoryRouter,
  MemoryRouter,
  Route,
  RouterProvider,
  Routes,
  useLocation,
} from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { REQUEST_DEADLINE_MS } from "@/api/client";
import type { components } from "@/api/schema";
import { getSession, setSession } from "@/auth/session";
import { PATHS, routes } from "@/routes";
import { COCRDLI } from "./COCRDLI";

/**
 * Render tests of map `COCRDLI` (`CCRDLIA`), the card list of transaction CCLI, program COCRDLIC,
 * endpoint 7 (`modern/audit/screens.md` COCRDLI rows, `modern/audit/online/COCRDLIC.md`).
 *
 * The global `fetch` is a stub that answers `GET /api/v1/cards` in process and 500 to any other
 * request, so nothing leaves the test. The listed cards are the first fourteen of `carddata.txt`
 * in card-number order, each card number redacted to its last four digits as the server sends
 * it. The only unredacted digits are typed card filters, which are request input.
 */

const ACCOUNT_FILTER_MESSAGE = "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER";
const CARD_FILTER_MESSAGE = "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER";
const MORE_THAN_ONE_ACTION = "PLEASE SELECT ONLY ONE RECORD TO VIEW OR UPDATE";
const INVALID_ACTION_CODE = "INVALID ACTION CODE";
const INFO_RECORD_ACTIONS = "TYPE S FOR DETAIL, U TO UPDATE ANY RECORD";
const NO_PREVIOUS_PAGES = "NO PREVIOUS PAGES TO DISPLAY";
const NO_MORE_RECORDS = "NO MORE RECORDS TO SHOW";
const NO_RECORDS_FOUND = "NO RECORDS FOUND FOR THIS SEARCH CONDITION.";

/** COCRDLIC's 75-byte file-error line, its RESP and RESP2 fields left blank (COCRDLIC-037). */
const FILE_ERROR = [
  "File Error: ",
  "READ    ",
  " on ",
  "CARDDAT  ",
  " returned RESP ",
  " ".repeat(10),
  ",RESP2 ",
  " ".repeat(10),
].join("");

/** Matches a label's text exactly, its padding included, as the map's INITIAL holds it. */
const EXACT = { normalizer: (text: string) => text };

/** Page 1 of the unfiltered list: cards 1 to 7, with a next page starting at card 8. */
const FIRST_PAGE: components["schemas"]["CardListResponse"] = {
  blank_rows_above: 0,
  first_key: 1,
  last_key: 8,
  has_next_page: true,
  last_page_shown: false,
  info_message: INFO_RECORD_ACTIONS,
  message: null,
  page_no: 1,
  rows: [
    { card_ref: 1, card_num: "************5740", acct_id: "00000000050", status: "Y" },
    { card_ref: 2, card_num: "************1516", acct_id: "00000000027", status: "Y" },
    { card_ref: 3, card_num: "************7330", acct_id: "00000000002", status: "Y" },
    { card_ref: 4, card_num: "************6232", acct_id: "00000000020", status: "Y" },
    { card_ref: 5, card_num: "************9795", acct_id: "00000000012", status: "Y" },
    { card_ref: 6, card_num: "************4350", acct_id: "00000000044", status: "Y" },
    { card_ref: 7, card_num: "************8931", acct_id: "00000000037", status: "Y" },
  ],
};

/** Page 2 of the unfiltered list, F8 from page 1 (COCRDLIC-023): cards 8 to 14. */
const SECOND_PAGE: components["schemas"]["CardListResponse"] = {
  blank_rows_above: 0,
  first_key: 8,
  last_key: 15,
  has_next_page: true,
  last_page_shown: false,
  info_message: INFO_RECORD_ACTIONS,
  message: null,
  page_no: 2,
  rows: [
    { card_ref: 8, card_num: "************1600", acct_id: "00000000035", status: "Y" },
    { card_ref: 9, card_num: "************2090", acct_id: "00000000039", status: "Y" },
    { card_ref: 10, card_num: "************7565", acct_id: "00000000024", status: "Y" },
    { card_ref: 11, card_num: "************2490", acct_id: "00000000006", status: "Y" },
    { card_ref: 12, card_num: "************0449", acct_id: "00000000022", status: "Y" },
    { card_ref: 13, card_num: "************4312", acct_id: "00000000004", status: "Y" },
    { card_ref: 14, card_num: "************7560", acct_id: "00000000010", status: "Y" },
  ],
};

/** F7 from page 2 (COCRDLIC-027, COCRDLIC-028): page 1 again, with no information line. */
const BACK_TO_FIRST_PAGE: components["schemas"]["CardListResponse"] = {
  ...FIRST_PAGE,
  message: NO_PREVIOUS_PAGES,
  info_message: null,
};

/** ENTER with the account filter `00000000050`: its one card, no next page (COCRDLIC-020). */
const ONE_CARD_PAGE: components["schemas"]["CardListResponse"] = {
  blank_rows_above: 0,
  first_key: 1,
  last_key: 1,
  has_next_page: false,
  last_page_shown: false,
  info_message: INFO_RECORD_ACTIONS,
  message: NO_MORE_RECORDS,
  page_no: 1,
  rows: [{ card_ref: 1, card_num: "************5740", acct_id: "00000000050", status: "Y" }],
};

/**
 * F7 from page 2 after the account filter `00000000050` was typed: the backward read reached the
 * start of the card file, so the one card fills row 7 below six empty rows (COCRDLIC-029), and
 * page 1 shows `NO PREVIOUS PAGES TO DISPLAY` (COCRDLIC-028).
 */
const BACKWARD_FILLED_PAGE: components["schemas"]["CardListResponse"] = {
  blank_rows_above: 6,
  first_key: 8,
  last_key: 8,
  has_next_page: true,
  last_page_shown: false,
  info_message: null,
  message: NO_PREVIOUS_PAGES,
  page_no: 1,
  rows: [{ card_ref: 1, card_num: "************5740", acct_id: "00000000050", status: "Y" }],
};

/** ENTER with an account filter no card matches, on page 1 (COCRDLIC-021). */
const NO_RECORDS_PAGE: components["schemas"]["CardListResponse"] = {
  blank_rows_above: 0,
  first_key: 1,
  last_key: 8,
  has_next_page: false,
  last_page_shown: false,
  info_message: null,
  message: NO_RECORDS_FOUND,
  page_no: 1,
  rows: [],
};

/** The answer to a request that is not endpoint 7. */
const UNEXPECTED: components["schemas"]["ScreenError"] = { message: "unexpected request" };

/** The paging state of `FIRST_PAGE` and `SECOND_PAGE` as each later request echoes it. */
const FIRST_PAGE_ECHO = {
  page_no: "1",
  first_key: "1",
  last_key: "8",
  has_next_page: "true",
  last_page_shown: "false",
};
const SECOND_PAGE_ECHO = {
  page_no: "2",
  first_key: "8",
  last_key: "15",
  has_next_page: "true",
  last_page_shown: "false",
};

/** A list row with no card: its three cells are empty. */
const EMPTY_ROW = ["", "", ""];

/** Replies with a JSON body, as the API sends every page and every `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces the global `fetch` with a stub that records every request and answers endpoint 7 with
 * `answer`, and any other request with 500.
 */
function stubFetch(answer: (url: URL) => Response | Promise<Response>) {
  const seen: { method: string; url: URL; authorization: string | null }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init);
      const url = new URL(request.url);
      seen.push({
        method: request.method,
        url,
        authorization: request.headers.get("Authorization"),
      });
      if (request.method !== "GET" || url.pathname !== "/api/v1/cards") {
        return json(500, UNEXPECTED);
      }
      return answer(url);
    }),
  );
  return seen;
}

/** Answers the n-th list request with the n-th reply, and every later one with the last. */
function stubReplies(...replies: readonly (readonly [number, unknown])[]) {
  let served = 0;
  return stubFetch(() => {
    const reply = replies.at(Math.min(served, replies.length - 1));
    served += 1;
    return reply === undefined ? json(500, UNEXPECTED) : json(reply[0], reply[1]);
  });
}

/**
 * The query of every request so far, after checking that each one was endpoint 7 and that none
 * carried `page_size`, which the screen never sends (Bounded Pagination).
 */
function listQueries(seen: ReturnType<typeof stubFetch>): Record<string, string>[] {
  for (const request of seen) {
    expect(`${request.method} ${request.url.pathname}`).toBe("GET /api/v1/cards");
    expect(request.url.href).not.toContain("page_size");
  }
  return seen.map((request) => Object.fromEntries(request.url.searchParams));
}

/** A navigation target: a region named after its map, showing the route state it received. */
function Target({ name }: { readonly name: string }) {
  const state: unknown = useLocation().state;
  return (
    <section aria-label={name}>
      <p>{JSON.stringify(state ?? null)}</p>
    </section>
  );
}

/** Renders `/cards` with a stub at every route the screen opens. */
function renderScreen(): void {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[PATHS.COCRDLI]}>
        <Routes>
          <Route path={PATHS.COCRDLI} element={<COCRDLI />} />
          <Route path={PATHS.COCRDSL} element={<Target name="COCRDSL" />} />
          <Route path={PATHS.COCRDUP} element={<Target name="COCRDUP" />} />
          <Route path={PATHS.COMEN01} element={<Target name="COMEN01" />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The route state the target `name` received, once the screen has opened it. */
async function arrivedAt(name: string): Promise<unknown> {
  const target = await screen.findByRole("region", { name });
  const state: unknown = JSON.parse(target.textContent ?? "");
  return state;
}

/** Waits until row 1 lists the first card of `FIRST_PAGE`. */
async function firstPageShown(): Promise<void> {
  await waitFor(() => {
    expect(listedRows().at(0)).toEqual(["00000000050", "************5740", "Y"]);
  });
}

/** Waits until `count` requests were sent and the last one is answered. */
async function answered(seen: ReturnType<typeof stubFetch>, count: number): Promise<void> {
  await waitFor(() => {
    expect(seen).toHaveLength(count);
    expect(document.querySelector("form")?.getAttribute("aria-busy")).toBe("false");
  });
}

/** `ACCTSID`, found by its label `Account Number    :`. */
function accountFilter(): HTMLElement {
  return screen.getByLabelText("Account Number    :", EXACT);
}

/** `CARDSID`, found by its label `Credit Card Number:`. */
function cardFilter(): HTMLElement {
  return screen.getByLabelText("Credit Card Number:", EXACT);
}

/** `PAGENO`, found by its label `Page `. */
function pageNo(): HTMLElement {
  return screen.getByLabelText("Page ", EXACT);
}

/** The selection code input of list row `row`, 1 to 7. */
function selector(row: number): HTMLElement {
  return screen.getByRole("textbox", { name: `Select row ${row}` });
}

/** The seven list rows, empty ones included. */
function bodyRows(): Element[] {
  const table = screen.getByRole("table", { name: "Credit cards" });
  return Array.from(table.querySelectorAll("tbody > tr"));
}

/** The account, card number and status cells of each list row, in screen order. */
function listedRows(): string[][] {
  return bodyRows().map((row) =>
    Array.from(row.querySelectorAll("td"), (cell) => cell.textContent ?? "").slice(1),
  );
}

/** The cells `listedRows` shows for each card of `page`, from row 1. */
function cellsOf(page: components["schemas"]["CardListResponse"]): string[][] {
  return page.rows.map((card) => [card.acct_id, card.card_num, card.status]);
}

/** The one message row of `kind`, `ERRMSG` (`error`) or `INFOMSG` (`info`). */
function messageRow(kind: "error" | "info"): HTMLElement {
  const rows = screen
    .getAllByRole("alert")
    .filter((row) => row.getAttribute("data-message-kind") === kind);
  const [row, ...others] = rows;
  if (row === undefined || others.length > 0) {
    throw new Error(`expected one ${kind} row, found ${rows.length}`);
  }
  return row;
}

/** Replaces what `field` holds with `value`, as typing over it does. */
function type(field: HTMLElement, value: string): void {
  fireEvent.change(field, { target: { value } });
}

/** Clicks the key-row button labelled `name`. */
function press(name: string): void {
  fireEvent.click(screen.getByRole("button", { name }));
}

/** Types each non-empty code at its row position, row 1 first. */
function typeCodes(codes: readonly string[]): void {
  for (const [index, code] of codes.entries()) {
    if (code !== "") {
      type(selector(index + 1), code);
    }
  }
}

beforeEach(() => {
  setSession("test-token", "USER0001", "U");
});

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COCRDLI", () => {
  it("renders the map's fields and key row, then lists page 1 of a fresh start", async () => {
    // audit: COCRDLIC-002
    // audit: COCRDLI-011
    // audit: COCRDLI-012
    // audit: COCRDLI-014
    // audit: COCRDLI-017
    // audit: COCRDLI-020
    // audit: COCRDLI-021
    // audit: COCRDLI-022
    // audit: COCRDLI-023
    // audit: COCRDLI-024
    // audit: COCRDLI-025
    // audit: COCRDLI-026
    // audit: COCRDLI-027
    // audit: COCRDLI-030
    // audit: COCRDLI-032
    // audit: COCRDLI-036
    // audit: COCRDLI-038
    // audit: COCRDLI-042
    // audit: COCRDLI-044
    // audit: COCRDLI-048
    // audit: COCRDLI-050
    // audit: COCRDLI-054
    // audit: COCRDLI-056
    // audit: COCRDLI-060
    // audit: COCRDLI-062
    // audit: COCRDLI-066
    // audit: COCRDLI-068
    // audit: COCRDLI-072
    // audit: COCRDLI-073
    // audit: COCRDLI-074
    // audit: COCRDLI-075
    // audit: COCRDLI-076
    // audit: COCRDLI-077
    // audit: COCRDLI-078
    const requests = stubReplies([200, FIRST_PAGE]);
    renderScreen();

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("List Credit Cards");
    const header = (field: string) =>
      document.querySelector(`[data-field="${field}"]`)?.textContent;
    expect(header("TRNNAME")).toBe("CCLI");
    expect(header("PGMNAME")).toBe("COCRDLIC");
    expect(header("TITLE01")).toBe("      AWS Mainframe Modernization       ");
    expect(header("TITLE02")).toBe("              CardDemo                  ");
    expect(header("CURDATE")).toMatch(/^\d{2}\/\d{2}\/\d{2}$/);
    expect(header("CURTIME")).toMatch(/^\d{2}:\d{2}:\d{2}$/);

    expect(accountFilter().getAttribute("maxlength")).toBe("11");
    expect(cardFilter().getAttribute("maxlength")).toBe("16");
    expect(pageNo()).toHaveProperty("readOnly", true);

    const table = screen.getByRole("table", { name: "Credit cards" });
    expect(within(table).getAllByRole("columnheader").map((head) => head.textContent)).toEqual([
      "Select    ",
      "Account Number",
      " Card Number ",
      "Active ",
    ]);
    expect(
      Array.from(table.querySelectorAll('thead > tr[aria-hidden="true"] > th'), (head) => {
        return head.textContent;
      }),
    ).toEqual(["------", "---------------", "---------------", "--------"]);

    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(keys.map((key) => key.textContent)).toEqual([
      "ENTER",
      "F3=Exit",
      "F7=Backward",
      "F8=Forward",
    ]);
    expect(
      keys.map((key) => [
        key.getAttribute("type"),
        key.getAttribute("data-variant"),
        key.getAttribute("data-size"),
      ]),
    ).toEqual([
      ["submit", "default", "sm"],
      ["button", "outline", "sm"],
      ["button", "outline", "sm"],
      ["button", "outline", "sm"],
    ]);

    await firstPageShown();
    expect(listedRows()).toEqual(cellsOf(FIRST_PAGE));
    for (const row of [1, 2, 3, 4, 5, 6, 7]) {
      expect(selector(row).getAttribute("maxlength")).toBe("1");
      expect(selector(row)).toHaveProperty("value", "");
      expect(selector(row)).toHaveProperty("readOnly", false);
    }
    expect(pageNo()).toHaveProperty("value", "1");
    expect(accountFilter()).toHaveProperty("value", "");
    expect(cardFilter()).toHaveProperty("value", "");
    expect(messageRow("info").textContent).toBe(INFO_RECORD_ACTIONS);
    expect(messageRow("error").textContent).toBe("");
    expect(document.body.textContent).not.toMatch(/\d{16}/);
    await waitFor(() => {
      expect(document.activeElement).toBe(accountFilter());
    });

    // The fresh start sends no filter and no paging state (COCRDLIC-017).
    expect(listQueries(requests)).toEqual([{}]);
    expect(requests.at(0)?.authorization).toBe("Bearer test-token");
  });

  it("places the fields on their map lines and renders no stopper or dark field", async () => {
    // audit: COCRDLI-013
    // audit: COCRDLI-016
    // audit: COCRDLI-019
    // audit: COCRDLI-029
    // audit: COCRDLI-034
    // audit: COCRDLI-035
    // audit: COCRDLI-040
    // audit: COCRDLI-041
    // audit: COCRDLI-046
    // audit: COCRDLI-047
    // audit: COCRDLI-052
    // audit: COCRDLI-053
    // audit: COCRDLI-058
    // audit: COCRDLI-059
    // audit: COCRDLI-064
    // audit: COCRDLI-065
    // audit: COCRDLI-070
    stubReplies([200, FIRST_PAGE]);
    renderScreen();
    await firstPageShown();
    const form = document.querySelector("form");
    if (form === null) {
      throw new Error("the screen rendered no form");
    }

    // `PAGENO`, `POS=(4,76) LENGTH=3`: read-only, showing the page number endpoint 7 sent.
    expect(pageNo().parentElement?.className).toBe("row-start-4 col-start-76 col-span-3");
    expect(pageNo()).toHaveProperty("readOnly", true);
    expect(pageNo()).toHaveProperty("value", String(FIRST_PAGE.page_no));

    // Every placed element of the body grid as its line, column and width. No cell stands at the
    // zero-length stoppers (6,56), (7,61) and (20,65): their columns stay empty.
    const placed = Array.from(form.querySelectorAll('[class*="row-start-"]'), (cell) => [
      /\brow-start-(\d+)\b/.exec(cell.className)?.[1],
      /\bcol-start-(\d+)\b/.exec(cell.className)?.[1],
      /\bcol-span-(\d+)\b/.exec(cell.className)?.[1],
    ]);
    expect(placed).toEqual([
      ["4", "70", "5"],
      ["4", "76", "3"],
      ["6", "22", "19"],
      ["6", "44", "11"],
      ["7", "22", "19"],
      ["7", "44", "16"],
      // The list table, map lines 9 to 17, columns 10 to 72.
      ["9", "10", "63"],
    ]);

    // Each list row holds its selector and three output cells and nothing else: neither the
    // stopper after the selector (column 14) nor the dark `CRDSTP2` to `CRDSTP7` is rendered.
    for (const row of bodyRows()) {
      const cells = Array.from(row.children);
      expect(cells.map((cell) => cell.tagName)).toEqual(["TD", "TD", "TD", "TD"]);
      expect(cells.at(0)?.querySelectorAll("input")).toHaveLength(1);
      expect(cells.at(0)?.textContent).toBe("");
      expect(row.querySelectorAll("input")).toHaveLength(1);
    }
    // The form's inputs are the page number, the two filters and the seven selectors.
    expect(form.querySelectorAll("input")).toHaveLength(10);
    expect(form.querySelectorAll('input[type="hidden"]')).toHaveLength(0);
  });

  it.each(
    FIRST_PAGE.rows.flatMap((card, index) => [
      { code: "S", target: "COCRDSL", row: index + 1, card },
      { code: "U", target: "COCRDUP", row: index + 1, card },
    ]),
  )("ENTER with $code on row $row opens $target with that row's card", async ({
    code,
    target,
    row,
    card,
  }) => {
    // audit: COCRDLIC-015
    // audit: COCRDLI-028
    // audit: COCRDLI-031
    // audit: COCRDLI-033
    // audit: COCRDLI-037
    // audit: COCRDLI-039
    // audit: COCRDLI-043
    // audit: COCRDLI-045
    // audit: COCRDLI-049
    // audit: COCRDLI-051
    // audit: COCRDLI-055
    // audit: COCRDLI-057
    // audit: COCRDLI-061
    // audit: COCRDLI-063
    // audit: COCRDLI-067
    // audit: COCRDLI-082
    const requests = stubReplies([200, FIRST_PAGE]);
    renderScreen();
    await firstPageShown();

    const enter = screen.getByRole("button", { name: "ENTER" });
    expect(enter.getAttribute("type")).toBe("submit");
    expect(listedRows().at(row - 1)).toEqual([card.acct_id, card.card_num, card.status]);
    expect(document.body.textContent).not.toMatch(/\d{16}/);
    const codeInput = selector(row);
    expect(codeInput).toHaveProperty("readOnly", false);
    expect(codeInput.closest("td")?.closest("table")?.getAttribute("aria-label")).toBe(
      "Credit cards",
    );
    expect(codeInput.getAttribute("maxlength")).toBe("1");

    // A filter typed with the same ENTER is edited, then ignored by the transfer.
    type(accountFilter(), "00000000050");
    type(codeInput, code);
    fireEvent.click(enter);

    expect(await arrivedAt(target)).toEqual({
      from: PATHS.COCRDLI,
      acct_id: card.acct_id,
      card_ref: card.card_ref,
    });
    // The selection only navigates: nothing is sent after the first page.
    expect(listQueries(requests)).toEqual([{}]);
  });

  it("submits the form on the Enter key and reads the list with the filters", async () => {
    // audit: COCRDLI-082
    const requests = stubReplies([200, FIRST_PAGE], [200, ONE_CARD_PAGE]);
    renderScreen();
    await firstPageShown();

    type(accountFilter(), "00000000050");
    // Outside a field the key binding submits the form; in a field the browser does.
    fireEvent.keyDown(window, { key: "Enter" });

    await answered(requests, 2);
    expect(listQueries(requests)).toEqual([{}, { acct_id: "00000000050", ...FIRST_PAGE_ECHO }]);
    expect(listedRows()).toEqual([
      ...cellsOf(ONE_CARD_PAGE),
      ...Array.from({ length: 6 }, () => EMPTY_ROW),
    ]);
    expect(messageRow("error").textContent).toBe(NO_MORE_RECORDS);
  });

  it.each([
    {
      title: "S and U on two rows",
      key: "ENTER",
      codes: ["S", "U", "", "", "", "", ""],
      message: MORE_THAN_ONE_ACTION,
      marked: [1, 2],
      cursor: 2,
      reread: { ...FIRST_PAGE_ECHO, reread: "true" },
    },
    {
      title: "two actions and an unknown code",
      key: "ENTER",
      codes: ["S", "", "X", "", "", "U", ""],
      message: MORE_THAN_ONE_ACTION,
      marked: [1, 3, 6],
      cursor: 3,
      reread: { ...FIRST_PAGE_ECHO, reread: "true" },
    },
    {
      title: "an unknown code",
      key: "ENTER",
      codes: ["", "", "X", "", "", "", ""],
      message: INVALID_ACTION_CODE,
      marked: [3],
      cursor: 3,
      reread: { ...FIRST_PAGE_ECHO, reread: "true" },
    },
    {
      title: "a lower-case s on row 1, which never takes the cursor",
      key: "ENTER",
      codes: ["s", "", "", "", "", "", ""],
      message: INVALID_ACTION_CODE,
      marked: [1],
      cursor: undefined,
      reread: { ...FIRST_PAGE_ECHO, reread: "true" },
    },
    {
      title: "F8 with an S and a lower-case u",
      key: "F8=Forward",
      codes: ["", "", "", "S", "u", "", ""],
      message: INVALID_ACTION_CODE,
      marked: [5],
      cursor: 5,
      reread: { ...FIRST_PAGE_ECHO, direction: "forward", reread: "true" },
    },
    {
      title: "F7 with U on two rows",
      key: "F7=Backward",
      codes: ["", "", "", "", "", "U", "U"],
      message: MORE_THAN_ONE_ACTION,
      marked: [6, 7],
      cursor: 6,
      reread: { ...FIRST_PAGE_ECHO, direction: "backward", reread: "true" },
    },
  ])("rejects a selection of $title and re-reads the list", async ({
    key,
    codes,
    message,
    marked,
    cursor,
    reread,
  }) => {
    // audit: COCRDLIC-012
    // audit: COCRDLIC-013
    // audit: COCRDLIC-014
    // audit: COCRDLIC-035
    const requests = stubReplies([200, FIRST_PAGE]);
    renderScreen();
    await firstPageShown();

    typeCodes(codes);
    press(key);

    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(message);
    });
    await answered(requests, 2);
    expect(screen.queryByRole("region")).toBeNull();
    // The re-read carries the filters and the paging state, never the selection codes.
    expect(listQueries(requests)).toEqual([{}, reread]);
    for (const [index, code] of codes.entries()) {
      const row = index + 1;
      expect(selector(row)).toHaveProperty("value", code);
      expect(selector(row).getAttribute("aria-invalid")).toBe(String(marked.includes(row)));
      expect(selector(row).className).toContain("aria-invalid:text-destructive");
    }
    expect(listedRows()).toEqual(cellsOf(FIRST_PAGE));
    expect(pageNo()).toHaveProperty("value", "1");
    expect(messageRow("info").textContent).toBe(INFO_RECORD_ACTIONS);
    await waitFor(() => {
      expect(document.activeElement).toBe(
        cursor === undefined ? accountFilter() : selector(cursor),
      );
    });
  });

  it("pages with F8 and F7, ignoring the keys while a read is in flight", async () => {
    // audit: COCRDLI-080
    // audit: COCRDLI-081
    const gate: { open?: () => void } = {};
    const held = new Promise<void>((resolve) => {
      gate.open = resolve;
    });
    const requests = stubFetch(async (url) => {
      const direction = url.searchParams.get("direction");
      if (direction === "forward") {
        await held;
        return json(200, SECOND_PAGE);
      }
      return json(200, direction === "backward" ? BACK_TO_FIRST_PAGE : FIRST_PAGE);
    });
    renderScreen();
    await firstPageShown();

    press("F8=Forward");
    await waitFor(() => {
      expect(document.querySelector("form")?.getAttribute("aria-busy")).toBe("true");
    });
    // The 3270 keyboard stays locked until the program answers: these send nothing.
    press("F8=Forward");
    press("F7=Backward");
    press("ENTER");
    gate.open?.();

    await waitFor(() => {
      expect(listedRows()).toEqual(cellsOf(SECOND_PAGE));
    });
    expect(pageNo()).toHaveProperty("value", "2");
    expect(messageRow("error").textContent).toBe("");

    press("F7=Backward");
    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(NO_PREVIOUS_PAGES);
    });
    expect(listedRows()).toEqual(cellsOf(FIRST_PAGE));
    expect(pageNo()).toHaveProperty("value", "1");
    expect(messageRow("info").textContent).toBe("");

    // COCRDLI-080 and COCRDLI-081 name the page's first and last keys `page_key`; endpoint 7
    // takes them as `first_key` and `last_key`, echoed with the rest of the paging state.
    expect(listQueries(requests)).toEqual([
      {},
      { ...FIRST_PAGE_ECHO, direction: "forward" },
      { ...SECOND_PAGE_ECHO, direction: "backward" },
    ]);
  });

  it("keeps a selection code at its row across F8, then opens the card now there", async () => {
    // audit: COCRDLIC-016
    const requests = stubReplies([200, FIRST_PAGE], [200, SECOND_PAGE]);
    renderScreen();
    await firstPageShown();

    type(selector(3), "S");
    press("F8=Forward");
    await waitFor(() => {
      expect(listedRows()).toEqual(cellsOf(SECOND_PAGE));
    });
    expect(screen.queryByRole("region")).toBeNull();
    expect(selector(3)).toHaveProperty("value", "S");
    expect(listQueries(requests)).toEqual([{}, { ...FIRST_PAGE_ECHO, direction: "forward" }]);

    press("ENTER");
    expect(await arrivedAt("COCRDSL")).toEqual({
      from: PATHS.COCRDLI,
      acct_id: "00000000024",
      card_ref: 10,
    });
    expect(requests).toHaveLength(2);
  });

  it.each([
    { label: "F1", key: "F1", shiftKey: false },
    { label: "F2", key: "F2", shiftKey: false },
    { label: "F4", key: "F4", shiftKey: false },
    { label: "F5", key: "F5", shiftKey: false },
    { label: "F6", key: "F6", shiftKey: false },
    { label: "F9", key: "F9", shiftKey: false },
    { label: "F10", key: "F10", shiftKey: false },
    { label: "F11", key: "F11", shiftKey: false },
    { label: "F12", key: "F12", shiftKey: false },
    { label: "PF13 (Shift+F1)", key: "F1", shiftKey: true },
    { label: "PF14 (Shift+F2)", key: "F2", shiftKey: true },
    { label: "PF16 (Shift+F4)", key: "F4", shiftKey: true },
    { label: "PF24 (Shift+F12)", key: "F12", shiftKey: true },
  ])("processes $label exactly as ENTER", async ({ key, shiftKey }) => {
    // audit: COCRDLIC-003
    const requests = stubReplies([200, FIRST_PAGE]);
    renderScreen();
    await firstPageShown();

    fireEvent.keyDown(window, { key, shiftKey });

    await answered(requests, 2);
    expect(listQueries(requests)).toEqual([{}, FIRST_PAGE_ECHO]);
    expect(messageRow("error").textContent).toBe("");
    expect(listedRows()).toEqual(cellsOf(FIRST_PAGE));
  });

  it.each([
    { label: "F7", key: "F7", shiftKey: false, direction: "backward", reply: BACK_TO_FIRST_PAGE },
    {
      label: "PF19 (Shift+F7)",
      key: "F7",
      shiftKey: true,
      direction: "backward",
      reply: BACK_TO_FIRST_PAGE,
    },
    { label: "F8", key: "F8", shiftKey: false, direction: "forward", reply: SECOND_PAGE },
    {
      label: "PF20 (Shift+F8)",
      key: "F8",
      shiftKey: true,
      direction: "forward",
      reply: SECOND_PAGE,
    },
  ])("pages $direction on the key $label", async ({ key, shiftKey, direction, reply }) => {
    // audit: COCRDLIC-003
    // audit: COCRDLI-080
    // audit: COCRDLI-081
    const requests = stubReplies([200, FIRST_PAGE], [200, reply]);
    renderScreen();
    await firstPageShown();

    fireEvent.keyDown(window, { key, shiftKey });

    await answered(requests, 2);
    expect(listQueries(requests)).toEqual([{}, { ...FIRST_PAGE_ECHO, direction }]);
    expect(listedRows()).toEqual(cellsOf(reply));
    expect(pageNo()).toHaveProperty("value", String(reply.page_no));
  });

  it.each([
    { label: "a click on F3=Exit", exit: () => press("F3=Exit") },
    { label: "the key F3", exit: () => fireEvent.keyDown(window, { key: "F3" }) },
    {
      label: "the key PF15 (Shift+F3)",
      exit: () => fireEvent.keyDown(window, { key: "F3", shiftKey: true }),
    },
  ])("exits to the main menu on $label, whatever the inputs hold", async ({ exit }) => {
    // audit: COCRDLIC-003
    // audit: COCRDLIC-004
    // audit: COCRDLI-079
    const requests = stubReplies([200, FIRST_PAGE]);
    renderScreen();
    await firstPageShown();

    type(accountFilter(), "12");
    type(selector(1), "X");
    type(selector(2), "S");
    exit();

    expect(await arrivedAt("COMEN01")).toBeNull();
    expect(listQueries(requests)).toEqual([{}]);
  });

  it.each([
    {
      title: "a short account filter, on ENTER",
      key: "ENTER",
      account: "123",
      card: "",
      message: ACCOUNT_FILTER_MESSAGE,
      accountInError: true,
      cardInError: false,
    },
    {
      title: "a card filter holding a letter, on F8",
      key: "F8=Forward",
      account: "",
      card: "111122223333444X",
      message: CARD_FILTER_MESSAGE,
      accountInError: false,
      cardInError: true,
    },
    {
      title: "both filters, on F7",
      key: "F7=Backward",
      account: "0000000005A",
      card: "12",
      message: ACCOUNT_FILTER_MESSAGE,
      accountInError: true,
      cardInError: true,
    },
  ])("rejects $title without sending it and protects every selector", async ({
    key,
    account,
    card,
    message,
    accountInError,
    cardInError,
  }) => {
    // audit: COCRDLIC-011
    // audit: COCRDLI-069
    const requests = stubReplies([200, FIRST_PAGE]);
    renderScreen();
    await firstPageShown();
    expect(messageRow("info").textContent).toBe(INFO_RECORD_ACTIONS);
    expect(messageRow("info").className).toContain("text-card-foreground");
    expect(messageRow("info").className).not.toContain("text-destructive");

    // The filters are edited before the selection, so this S opens nothing.
    type(selector(2), "S");
    type(accountFilter(), account);
    type(cardFilter(), card);
    press(key);

    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(message);
    });
    expect(listQueries(requests)).toEqual([{}]);
    expect(screen.queryByRole("region")).toBeNull();
    expect(listedRows()).toEqual(cellsOf(FIRST_PAGE));
    expect(pageNo()).toHaveProperty("value", "1");
    for (const row of [1, 2, 3, 4, 5, 6, 7]) {
      expect(selector(row)).toHaveProperty("readOnly", true);
      expect(selector(row).getAttribute("aria-invalid")).toBe("false");
    }
    expect(selector(2)).toHaveProperty("value", "S");
    expect(accountFilter().getAttribute("aria-invalid")).toBe(String(accountInError));
    expect(cardFilter().getAttribute("aria-invalid")).toBe(String(cardInError));
    // No information line: the row stays, empty and without a visible box.
    expect(messageRow("info").textContent).toBe("");
    expect(messageRow("info").className).toContain("border-transparent");
    await waitFor(() => {
      expect(document.activeElement).toBe(accountInError ? accountFilter() : cardFilter());
    });
  });

  it.each([
    { field: "acct_id", message: ACCOUNT_FILTER_MESSAGE, accountInError: true },
    { field: "card_num", message: CARD_FILTER_MESSAGE, accountInError: false },
  ])("treats the server's 400 for $field as a filter error", async ({
    field,
    message,
    accountInError,
  }) => {
    // audit: COCRDLIC-011
    // Endpoint 7 repeats the filter edits and owns their messages; its 400 names the field.
    const rejection: components["schemas"]["ScreenError"] = { message, field };
    const requests = stubReplies([200, FIRST_PAGE], [400, rejection]);
    renderScreen();
    await firstPageShown();

    type(accountFilter(), "00000000050");
    type(cardFilter(), "1111222233334444");
    press("ENTER");

    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(message);
    });
    expect(listQueries(requests)).toEqual([
      {},
      { acct_id: "00000000050", card_num: "1111222233334444", ...FIRST_PAGE_ECHO },
    ]);
    expect(listedRows()).toEqual(cellsOf(FIRST_PAGE));
    expect(pageNo()).toHaveProperty("value", "1");
    for (const row of [1, 2, 3, 4, 5, 6, 7]) {
      expect(selector(row)).toHaveProperty("readOnly", true);
    }
    expect(accountFilter().getAttribute("aria-invalid")).toBe(String(accountInError));
    expect(cardFilter().getAttribute("aria-invalid")).toBe(String(!accountInError));
    expect(messageRow("info").textContent).toBe("");
    await waitFor(() => {
      expect(document.activeElement).toBe(accountInError ? accountFilter() : cardFilter());
    });
  });

  it("shows a failed card read's message and keeps the page and the selection marks", async () => {
    // audit: COCRDLI-071
    // Endpoint 7 answers a failed read with 500 and COCRDLIC's file-error line (COCRDLIC-031);
    // it declares no 404.
    const failure: components["schemas"]["ScreenError"] = { message: FILE_ERROR };
    const requests = stubReplies([200, FIRST_PAGE], [500, failure]);
    renderScreen();
    await firstPageShown();
    expect(messageRow("error").textContent).toBe("");

    type(selector(4), "X");
    press("ENTER");

    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(FILE_ERROR);
    });
    expect(messageRow("error").getAttribute("aria-live")).toBe("polite");
    expect(messageRow("error").className).toContain("text-destructive");
    expect(listQueries(requests)).toEqual([{}, { ...FIRST_PAGE_ECHO, reread: "true" }]);
    expect(listedRows()).toEqual(cellsOf(FIRST_PAGE));
    expect(pageNo()).toHaveProperty("value", "1");
    expect(selector(4)).toHaveProperty("value", "X");
    expect(selector(4).getAttribute("aria-invalid")).toBe("true");
    expect(selector(4)).toHaveProperty("readOnly", false);
    expect(messageRow("info").textContent).toBe(INFO_RECORD_ACTIONS);
    expect(document.body.textContent).not.toMatch(/\d{16}/);
  });

  it("shows a failed first read's message, then ENTER starts afresh with no paging", async () => {
    // audit: COCRDLI-071
    const failure: components["schemas"]["ScreenError"] = { message: FILE_ERROR };
    const requests = stubReplies([500, failure], [200, FIRST_PAGE]);
    renderScreen();

    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(FILE_ERROR);
    });
    await answered(requests, 1);
    expect(listedRows()).toEqual(Array.from({ length: 7 }, () => EMPTY_ROW));
    expect(bodyRows().some((row) => row.querySelector("input") !== null)).toBe(false);
    expect(pageNo()).toHaveProperty("value", "");
    expect(messageRow("info").textContent).toBe("");

    // No page is displayed, so nothing is echoed and the server reads a fresh start.
    press("ENTER");
    await firstPageShown();
    expect(listQueries(requests)).toEqual([{}, {}]);
    expect(pageNo()).toHaveProperty("value", "1");
    expect(messageRow("error").textContent).toBe("");
    expect(messageRow("info").textContent).toBe(INFO_RECORD_ACTIONS);
  });

  it.each([
    {
      title: "all-zero filters, shown empty",
      account: "00000000000",
      card: "0000000000000000",
      sent: { acct_id: "00000000000", card_num: "0000000000000000" },
      shownAccount: "",
      shownCard: "",
    },
    {
      title: "blank filters, not sent and shown empty",
      account: "   ",
      card: "    ",
      sent: {},
      shownAccount: "",
      shownCard: "",
    },
    {
      title: "an account and a card filter, shown as sent",
      account: "00000000050",
      card: "1111222233334444",
      sent: { acct_id: "00000000050", card_num: "1111222233334444" },
      shownAccount: "00000000050",
      shownCard: "1111222233334444",
    },
  ])("sends ENTER with $title", async ({ account, card, sent, shownAccount, shownCard }) => {
    // audit: COCRDLIC-036
    const requests = stubReplies([200, FIRST_PAGE], [200, ONE_CARD_PAGE]);
    renderScreen();
    await firstPageShown();

    type(accountFilter(), account);
    type(cardFilter(), card);
    press("ENTER");

    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(NO_MORE_RECORDS);
    });
    expect(listQueries(requests)).toEqual([{}, { ...sent, ...FIRST_PAGE_ECHO }]);
    expect(accountFilter()).toHaveProperty("value", shownAccount);
    expect(cardFilter()).toHaveProperty("value", shownCard);
  });

  it.each([
    {
      title: "a filtered page of one card",
      keys: ["ENTER"],
      replies: [FIRST_PAGE, ONE_CARD_PAGE],
      listed: [...cellsOf(ONE_CARD_PAGE), ...Array.from({ length: 6 }, () => EMPTY_ROW)],
    },
    {
      title: "a backward read that reached the start of the card file",
      keys: ["F8=Forward", "F7=Backward"],
      replies: [FIRST_PAGE, SECOND_PAGE, BACKWARD_FILLED_PAGE],
      listed: [...Array.from({ length: 6 }, () => EMPTY_ROW), ...cellsOf(BACKWARD_FILLED_PAGE)],
    },
    {
      title: "a search no card matches",
      keys: ["ENTER"],
      replies: [FIRST_PAGE, NO_RECORDS_PAGE],
      listed: Array.from({ length: 7 }, () => EMPTY_ROW),
    },
  ])("offers a selector only on the rows that hold a card, after $title", async ({
    keys,
    replies,
    listed,
  }) => {
    // audit: COCRDLIC-035
    // audit: COCRDLI-028
    // audit: COCRDLI-033
    // audit: COCRDLI-039
    // audit: COCRDLI-045
    // audit: COCRDLI-051
    // audit: COCRDLI-057
    // audit: COCRDLI-063
    const requests = stubReplies(...replies.map((page) => [200, page] as const));
    renderScreen();
    await firstPageShown();

    for (const [index, key] of keys.entries()) {
      // The account filter is typed before the last key, as the reply to it assumes.
      if (index === keys.length - 1) {
        type(accountFilter(), "00000000050");
      }
      press(key);
      await answered(requests, index + 2);
    }

    expect(listQueries(requests)).toHaveLength(replies.length);
    const last = replies.at(-1);
    expect(messageRow("error").textContent).toBe(last?.message ?? "");
    expect(messageRow("info").textContent).toBe(last?.info_message ?? "");
    expect(pageNo()).toHaveProperty("value", "1");
    expect(listedRows()).toEqual(listed);
    const holdsCard = listed.map((cells) => cells.join("") !== "");
    const rows = bodyRows();
    expect(rows.map((row) => row.querySelector("input") !== null)).toEqual(holdsCard);
    expect(rows.map((row) => row.getAttribute("aria-hidden"))).toEqual(
      holdsCard.map((holds) => (holds ? null : "true")),
    );
    expect(rows.map((row) => row.textContent === "")).toEqual(holdsCard.map((holds) => !holds));
    expect(accountFilter()).toHaveProperty("value", "00000000050");
  });
});

/** The 401 of the `SignedOnUser` extractor for a missing, invalid or expired token. */
const SIGNON_REQUIRED: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};

/**
 * Renders the application's route table at `/cards`, so the sign-on guard that only the table
 * mounts stands in front of the screen. Its screens load lazily, so a wait for one to render
 * allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COCRDLI] });
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
describe("COCRDLI in the route table: a 401 ends the session", () => {
  it.each([
    {
      title: "the list of the first entry",
      replies: [[401, SIGNON_REQUIRED]] as const,
      drive: async (seen: ReturnType<typeof stubFetch>) => {
        await waitFor(() => {
          expect(seen).toHaveLength(1);
        });
      },
      count: 1,
    },
    {
      title: "the F8 page after page 1",
      replies: [
        [200, FIRST_PAGE],
        [401, SIGNON_REQUIRED],
      ] as const,
      drive: async () => {
        await firstPageShown();
        press("F8=Forward");
      },
      count: 2,
    },
  ])(
    "$title answered 401 replaces the screen with sign-on and clears the session, with no retry",
    async ({ replies, drive, count }) => {
      const seen = stubReplies(...replies);
      const router = renderRouteTable();

      await drive(seen);

      expect(
        await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
      ).toBeDefined();
      expect(router.state.location.pathname).toBe(PATHS.COSGN00);
      expect(screen.queryByRole("heading", { level: 1, name: "List Credit Cards" })).toBeNull();
      expect(getSession()).toBeNull();
      expect(listQueries(seen)).toHaveLength(count);
      expect(seen.every((request) => request.authorization === "Bearer test-token")).toBe(true);
      router.dispose();
    },
  );
});

/** The message of the shared request deadline's `TimeoutError` (`api/client.ts`). */
const TIMED_OUT = "No answer from the server within 30 seconds";

/**
 * Replaces `fetch` as `stubFetch` does and answers the list requests in order from `replies`,
 * except list request number `unanswered` (from 1): like a request the server accepted and never
 * completed, it settles only when its signal aborts, and then rejects with the abort reason, as
 * `fetch` does. `held` resolves with that signal once the request was sent.
 */
function stubUnansweredRead(
  unanswered: number,
  ...replies: readonly (readonly [number, unknown])[]
): { seen: ReturnType<typeof stubFetch>; held: Promise<AbortSignal> } {
  const seen: ReturnType<typeof stubFetch> = [];
  const pending = [...replies];
  const held = new Promise<AbortSignal>((sent) => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const request = input instanceof Request ? input : new Request(input, init);
        const url = new URL(request.url);
        seen.push({
          method: request.method,
          url,
          authorization: request.headers.get("Authorization"),
        });
        if (request.method !== "GET" || url.pathname !== "/api/v1/cards") {
          return json(500, UNEXPECTED);
        }
        if (seen.length === unanswered) {
          sent(request.signal);
          return new Promise<Response>((_answer, fail) => {
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
        const reply = pending.shift();
        return reply === undefined ? json(500, UNEXPECTED) : json(reply[0], reply[1]);
      }),
    );
  });
  return { seen, held };
}

// Every request shares one deadline (`api/client.ts`). A page read the server never answers fails
// at the deadline like any other failure: its message in the error row, the page kept, the keys
// unlocked, and the read not sent again until a key asks for it.
describe("COCRDLI request deadline", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("fails an unanswered F8 read at the deadline, sends it once and serves the next F8", async () => {
    const { seen, held } = stubUnansweredRead(2, [200, FIRST_PAGE], [200, SECOND_PAGE]);
    renderScreen();
    await firstPageShown();
    await answered(seen, 1);

    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    press("F8=Forward");
    const signal = await act(() => held);
    expect(signal.aborted).toBe(false);
    await act(() => vi.advanceTimersByTimeAsync(REQUEST_DEADLINE_MS));
    vi.useRealTimers();

    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(TIMED_OUT);
    });
    await answered(seen, 2);
    expect(signal.aborted).toBe(true);
    const reason: unknown = signal.reason;
    expect(reason instanceof DOMException ? [reason.name, reason.message] : reason).toEqual([
      "TimeoutError",
      TIMED_OUT,
    ]);
    expect(listQueries(seen)).toEqual([{}, { ...FIRST_PAGE_ECHO, direction: "forward" }]);
    expect(listedRows()).toEqual(cellsOf(FIRST_PAGE));
    expect(pageNo()).toHaveProperty("value", "1");

    const armed = vi.spyOn(globalThis, "setTimeout");
    const cleared = vi.spyOn(globalThis, "clearTimeout");
    press("F8=Forward");
    await waitFor(() => {
      expect(pageNo()).toHaveProperty("value", "2");
    });
    await answered(seen, 3);
    expect(listQueries(seen)).toEqual([
      {},
      { ...FIRST_PAGE_ECHO, direction: "forward" },
      { ...FIRST_PAGE_ECHO, direction: "forward" },
    ]);
    expect(listedRows()).toEqual(cellsOf(SECOND_PAGE));
    expect(messageRow("error").textContent).toBe("");
    const deadlines = armed.mock.calls.flatMap((call, index) =>
      call[1] === REQUEST_DEADLINE_MS ? [armed.mock.results[index].value] : [],
    );
    expect(deadlines).toHaveLength(1);
    expect(cleared).toHaveBeenCalledWith(deadlines[0]);
  });
});
