// FN tests of the CT00 transaction list (TransactionListPage; COTRN00C, map
// COTRN00). ENTER ("Search", F-COTRN00-001) browses forward from the
// Transaction ID typed in TRNIDIN; a key the client schema rejects sends no
// request ('Tran ID must be Numeric ...', V-COTRN00C-004); a 400 the server
// answers at TRNIDIN reaches the user as a toast, as the field's error and as
// the cursor on the field, the COBOL `MOVE -1 TO TRNIDINL`; and Next (PF8,
// F-COTRN00-004) browses forward from the page's `next_key`, with the "View"
// row action (selector S, F-COTRN00-005) linking each row to its TRAN-ID.
//
// The network is the global the API client looks up on every request, stubbed
// per test, and the assertions read the Request objects it received.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  vi,
  type Mock,
  type MockInstance,
} from "vitest";

import type { components, paths } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { transactionListMessages } from "@/schemas/transactions";

import TransactionListPage from "./TransactionListPage";

type ListResponses = paths["/transactions"]["get"]["responses"];
type TransactionPage = ListResponses[200]["content"]["application/json"];
type ListErrorBody = ListResponses[400]["content"]["application/json"];
type TransactionResponse = components["schemas"]["TransactionResponse"];
type ErrorBody = components["schemas"]["ErrorBody"];
type FetchImpl = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

/** `GET /transactions` under the client's `/api` base path. */
const LIST_PATH = "/api/transactions";

/** COTRN00C's TRNIDIN text (V-COTRN00C-004), byte for byte. */
const TRAN_ID_NOT_NUMERIC = "Tran ID must be Numeric ...";

/** COTRN00C's end-of-master text (V-COTRN00C-007), byte for byte. */
const BOTTOM_REACHED = "You have reached the bottom of the page...";

/** The V-COTRN00C-004 example: 16 characters, the last not a digit. */
const NOT_NUMERIC_KEY = "000000000068358X";

/** A 16-digit key the schema accepts, answered by the server alone. */
const SERVER_REJECTED_KEY = "0000000000000099";

// Synthetic TRAN-IDs of the stubbed master: `transaction(index)` gives fixture
// record index + 1 the 16-digit key index + 1, so the twelve records sit under
// 0000000000000001 to 0000000000000012. These are the first key, the tenth
// (the first page's last row, the PF8 start key) and the eleventh.
const FIRST_KEY = "0000000000000001";
const LAST_KEY_OF_FIRST_PAGE = "0000000000000010";
const FIRST_KEY_OF_SECOND_PAGE = "0000000000000011";

/** `TRAN-ORIG-TS X(26)` of every app/data/ASCII/dailytran.txt record. */
const ORIG_TS = "2022-06-10 19:27:53.000000";

/**
 * `TRAN-PROC-TS X(26)` in the CBTRN02C Z-GET-DB2-FORMAT-TIMESTAMP form,
 * `YYYY-MM-DD-HH.MM.SS.mm0000`, at the processing clock the tests pin.
 */
const PROC_TS = "2022-07-01-12.00.00.000000";

/** The members that differ between the twelve fixture records used. */
type DailyTranContent = Pick<
  TransactionResponse,
  | "tran_type_cd"
  | "tran_source"
  | "tran_desc"
  | "tran_amt"
  | "tran_merchant_name"
  | "tran_merchant_city"
  | "tran_merchant_zip"
  | "tran_card_num"
>;

/**
 * Records 1 to 12 of app/data/ASCII/dailytran.txt (CVTRA06Y), the amount
 * decoded from its trailing overpunch. All twelve have category `0001` and
 * merchant ID `800000000`.
 */
const DAILY_TRAN: readonly DailyTranContent[] = [
  { tran_type_cd: "01", tran_source: "POS TERM", tran_desc: "Purchase at Abshire-Lowe", tran_amt: "504.77", tran_merchant_name: "Abshire-Lowe", tran_merchant_city: "North Enoshaven", tran_merchant_zip: "72112", tran_card_num: "4859452612877065" },
  { tran_type_cd: "03", tran_source: "OPERATOR", tran_desc: "Return item at Nitzsche, Nicolas and Lowe", tran_amt: "-919.00", tran_merchant_name: "Nitzsche, Nicolas and Lowe", tran_merchant_city: "Fidelshire", tran_merchant_zip: "53378", tran_card_num: "0927987108636232" },
  { tran_type_cd: "01", tran_source: "POS TERM", tran_desc: "Purchase at Ernser, Roob and Gleason", tran_amt: "67.88", tran_merchant_name: "Ernser, Roob and Gleason", tran_merchant_city: "North Makenziemouth", tran_merchant_zip: "78487-7965", tran_card_num: "6009619150674526" },
  { tran_type_cd: "01", tran_source: "POS TERM", tran_desc: "Purchase at Guann LLC", tran_amt: "281.77", tran_merchant_name: "Guann LLC", tran_merchant_city: "South Lynn", tran_merchant_zip: "51508-9166", tran_card_num: "8040580410348680" },
  { tran_type_cd: "01", tran_source: "POS TERM", tran_desc: "Purchase at Kertzmann-Schoen", tran_amt: "454.66", tran_merchant_name: "Kertzmann-Schoen", tran_merchant_city: "East Eulahstad", tran_merchant_zip: "98754-1089", tran_card_num: "5656830544981216" },
  { tran_type_cd: "01", tran_source: "POS TERM", tran_desc: "Purchase at Gislason-Medhurst", tran_amt: "849.99", tran_merchant_name: "Gislason-Medhurst", tran_merchant_city: "Colleenburgh", tran_merchant_zip: "23712-2080", tran_card_num: "7379335634661142" },
  { tran_type_cd: "03", tran_source: "OPERATOR", tran_desc: "Return item at Sipes Inc", tran_amt: "-56.77", tran_merchant_name: "Sipes Inc", tran_merchant_city: "Emilioside", tran_merchant_zip: "93329", tran_card_num: "4011500891777367" },
  { tran_type_cd: "01", tran_source: "POS TERM", tran_desc: "Purchase at Legros Group", tran_amt: "373.66", tran_merchant_name: "Legros Group", tran_merchant_city: "Carmeloborough", tran_merchant_zip: "34849-5127", tran_card_num: "8040580410348680" },
  { tran_type_cd: "03", tran_source: "OPERATOR", tran_desc: "Return item at Turcotte Group", tran_amt: "-535.88", tran_merchant_name: "Turcotte Group", tran_merchant_city: "Andrewfurt", tran_merchant_zip: "41346-3789", tran_card_num: "6503535181795992" },
  { tran_type_cd: "01", tran_source: "POS TERM", tran_desc: "Purchase at Gleason, Shanahan and Reynolds", tran_amt: "416.11", tran_merchant_name: "Gleason, Shanahan and Reynolds", tran_merchant_city: "Myrticeport", tran_merchant_zip: "21768-0823", tran_card_num: "9501733721429893" },
  { tran_type_cd: "01", tran_source: "POS TERM", tran_desc: "Purchase at Beatty-Hessel", tran_amt: "94.33", tran_merchant_name: "Beatty-Hessel", tran_merchant_city: "Simonisport", tran_merchant_zip: "52595", tran_card_num: "3260763612337560" },
  { tran_type_cd: "01", tran_source: "POS TERM", tran_desc: "Purchase at Wolf, Cruickshank and Bode", tran_amt: "250.22", tran_merchant_name: "Wolf, Cruickshank and Bode", tran_merchant_city: "Fritzchester", tran_merchant_zip: "20195-5156", tran_card_num: "7094142751055551" },
];

/** The transaction master record of fixture record `index` (0-based). */
function transaction(index: number): TransactionResponse {
  return {
    tran_id: `${index + 1}`.padStart(16, "0"),
    tran_cat_cd: "0001",
    tran_merchant_id: "800000000",
    tran_orig_ts: ORIG_TS,
    tran_proc_ts: PROC_TS,
    ...DAILY_TRAN[index],
  };
}

/**
 * The first entry's page, the ten rows of COTRN00 from the lowest key. It is
 * read without a start key, so it offers no backward request, and its last
 * key is the PF8 start key. COTRN00C shows no text with it, so the server
 * omits `message`.
 */
const FIRST_PAGE: TransactionPage = {
  items: DAILY_TRAN.slice(0, 10).map((_, index) => transaction(index)),
  next_key: LAST_KEY_OF_FIRST_PAGE,
  prev_key: null,
};

/** PF8 from the first page: the rows left before the end of the master. */
const SECOND_PAGE: TransactionPage = {
  items: [transaction(10), transaction(11)],
  next_key: null,
  prev_key: FIRST_KEY_OF_SECOND_PAGE,
  message: BOTTOM_REACHED,
};

/** V-COTRN00C-004's answer: status 400, the cursor on TRNIDIN. */
const START_KEY_ERROR: ListErrorBody = {
  code: "INVALID",
  message: TRAN_ID_NOT_NUMERIC,
  fields: [{ field: "TRNIDIN", message: TRAN_ID_NOT_NUMERIC }],
};

/** The answer to a request no test expects; afterEach reports it. */
const UNEXPECTED: ErrorBody = { code: "UNEXPECTED", message: "Unexpected error" };

function jsonResponse(status: number, body: TransactionPage | ErrorBody): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** A promise and its resolver, so a test decides when an answer arrives. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

function queryOf(request: Request): URLSearchParams {
  return new URL(request.url).searchParams;
}

let fetchMock: Mock<FetchImpl>;
let answer: (request: Request) => Response | Promise<Response>;
let unexpected: string[];
let queryClient: QueryClient;
let toastError: MockInstance<typeof toast.error>;

/**
 * The browse the fixture master answers: forward from the start (the first
 * entry) or from the first key gives the first page, forward from its last key
 * the second. Anything else is recorded as unexpected.
 */
function browse(request: Request): Response {
  const url = new URL(request.url);
  const startKey = url.searchParams.get("start_key");
  const direction = url.searchParams.get("direction") ?? "forward";
  if (request.method === "GET" && url.pathname === LIST_PATH && direction === "forward") {
    if (startKey === null || startKey === FIRST_KEY) {
      return jsonResponse(200, FIRST_PAGE);
    }
    if (startKey === LAST_KEY_OF_FIRST_PAGE) {
      return jsonResponse(200, SECOND_PAGE);
    }
  }
  unexpected.push(`${request.method} ${url.pathname}${url.search}`);
  return jsonResponse(500, UNEXPECTED);
}

/** Every request the page sent, in the order sent. */
function requests(): Request[] {
  return fetchMock.mock.calls.map(([input, init]) =>
    input instanceof Request ? input : new Request(input, init),
  );
}

/** The requests for `GET /transactions`. */
function listRequests(): Request[] {
  return requests().filter(
    (request) => new URL(request.url).pathname === LIST_PATH,
  );
}

/**
 * The FieldError that describes `input`: the page links an invalid TRNIDIN to
 * its error through `aria-describedby`, inside the field's group.
 */
function fieldError(input: HTMLElement): HTMLElement {
  const id = input.getAttribute("aria-describedby");
  const error = id === null ? null : document.getElementById(id);
  if (error === null) {
    throw new Error("the input is described by no field error");
  }
  expect(error.getAttribute("role")).toBe("alert");
  expect(error.closest('[role="group"]')?.contains(input)).toBe(true);
  return error;
}

/** The "View" row actions, in row order. */
function viewLinks(): HTMLElement[] {
  return within(screen.getByRole("table")).getAllByRole("link", { name: "View" });
}

/**
 * Renders the page at /transactions in a data router, which PageHeader's
 * useMatches needs, and waits for the first entry: one request without
 * parameters, which lists from the lowest key, as COTRN00C's first entry
 * browses from LOW-VALUES, and whose rows are shown.
 */
async function renderPage(): Promise<{
  user: UserEvent;
  startKeyInput: HTMLInputElement;
}> {
  queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(
    [
      {
        path: "/transactions",
        element: <TransactionListPage />,
        handle: { title: "Transactions", crumb: "Transactions" } satisfies RouteHandle,
      },
      {
        path: "/transactions/:tranId",
        element: <p>detail</p>,
        handle: { crumb: "Transaction" } satisfies RouteHandle,
      },
      { path: "/", element: <p>home</p>, handle: { crumb: "Home" } satisfies RouteHandle },
    ],
    { initialEntries: ["/transactions"] },
  );
  const user = userEvent.setup();
  render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
      <Toaster />
    </ThemeProvider>,
  );

  await screen.findByText(FIRST_KEY);
  const [entry, ...rest] = listRequests();
  expect(rest).toEqual([]);
  expect(entry.method).toBe("GET");
  expect([...queryOf(entry).keys()]).toEqual([]);

  return {
    user,
    startKeyInput: screen.getByRole<HTMLInputElement>("textbox", {
      name: "Transaction ID",
    }),
  };
}

beforeEach(() => {
  unexpected = [];
  answer = browse;
  fetchMock = vi.fn<FetchImpl>(async (input, init) =>
    answer(input instanceof Request ? input : new Request(input, init)),
  );
  vi.stubGlobal("fetch", fetchMock);
  toastError = vi.spyOn(toast, "error");
});

afterEach(() => {
  // Unmounted before the cache is cleared, so no observer refetches what the
  // clear removes; clearing ends the cache's garbage-collection timers.
  cleanup();
  queryClient.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  expect(unexpected).toEqual([]);
});

describe("TransactionListPage (CT00, COTRN00)", () => {
  // F-COTRN00-001: ENTER browses forward from the key typed in TRNIDIN.
  it("sends a valid start key with GET /transactions on ENTER", async () => {
    const { user, startKeyInput } = await renderPage();

    await user.type(startKeyInput, `${FIRST_KEY}{Enter}`);

    await waitFor(() => {
      expect(listRequests()).toHaveLength(2);
    });
    const [, search] = listRequests();
    expect(search.method).toBe("GET");
    expect(new URL(search.url).pathname).toBe(LIST_PATH);
    const query = queryOf(search);
    expect(query.get("start_key")).toBe(FIRST_KEY);
    // ENTER reads forward, the server's default; the page size stays the ten
    // rows of COTRN00, and the browse is unfiltered (E-17).
    expect(query.get("direction") ?? "forward").toBe("forward");
    expect(query.has("page_size")).toBe(false);
    expect(query.has("account_id")).toBe(false);
    expect(startKeyInput.getAttribute("aria-invalid")).toBe("false");
    expect(toastError).not.toHaveBeenCalled();
  });

  // V-COTRN00C-004: a start key that is not 16 digits browses nothing.
  it("sends no request for a start key that is not numeric", async () => {
    const { user, startKeyInput } = await renderPage();

    await user.type(startKeyInput, `${NOT_NUMERIC_KEY}{Enter}`);

    const error = await waitFor(() => fieldError(startKeyInput));
    expect(transactionListMessages.tranIdMustBeNumeric).toBe(TRAN_ID_NOT_NUMERIC);
    expect(error.textContent).toBe(transactionListMessages.tranIdMustBeNumeric);
    expect(startKeyInput.getAttribute("aria-invalid")).toBe("true");
    // The rejected key keeps the cursor, as `MOVE -1 TO TRNIDINL` places it.
    expect(document.activeElement).toBe(startKeyInput);
    expect(
      listRequests().filter(
        (request) => queryOf(request).get("start_key") === NOT_NUMERIC_KEY,
      ),
    ).toEqual([]);
    expect(listRequests()).toHaveLength(1);
    expect(toastError).not.toHaveBeenCalled();
  });

  // The server's error at TRNIDIN reaches the toast, the field and the cursor.
  it("shows a 400 at TRNIDIN as a toast, a field error and the cursor on the field", async () => {
    // The schema accepts this key, so only the server's answer can mark
    // TRNIDIN. The answer waits until the test has seen the cursor leave the
    // field, so the focus assertion proves the error moved it back.
    const rejected = deferred<Response>();
    answer = (request) =>
      queryOf(request).get("start_key") === SERVER_REJECTED_KEY
        ? rejected.promise
        : browse(request);
    const { user, startKeyInput } = await renderPage();

    await user.type(startKeyInput, SERVER_REJECTED_KEY);
    const search = screen.getByRole("button", { name: "Search" });
    await user.click(search);

    await waitFor(() => {
      expect(listRequests()).toHaveLength(2);
    });
    const [, sent] = listRequests();
    expect(sent.method).toBe("GET");
    expect(queryOf(sent).get("start_key")).toBe(SERVER_REJECTED_KEY);
    expect(document.activeElement).toBe(search);
    expect(toastError).not.toHaveBeenCalled();

    rejected.resolve(jsonResponse(400, START_KEY_ERROR));

    await waitFor(() => {
      expect(toastError).toHaveBeenCalledWith(TRAN_ID_NOT_NUMERIC);
    });
    expect(toastError).toHaveBeenCalledTimes(1);
    const error = await waitFor(() => fieldError(startKeyInput));
    expect(error.textContent).toBe(TRAN_ID_NOT_NUMERIC);
    expect(startKeyInput.getAttribute("aria-invalid")).toBe("true");
    await waitFor(() => {
      expect(document.activeElement).toBe(startKeyInput);
    });
  });

  // F-COTRN00-004 and F-COTRN00-005: PF8 passes the page's last key back with
  // `direction=forward`, and each row's View link carries its TRAN-ID.
  it("Next browses forward from the page's next_key and View links each row", async () => {
    const { user } = await renderPage();
    const firstPageLinks = viewLinks();
    expect(firstPageLinks).toHaveLength(10);
    expect(firstPageLinks[0].getAttribute("href")).toMatch(
      /\/transactions\/0000000000000001$/,
    );

    await user.click(screen.getByRole("button", { name: "Next" }));

    await waitFor(() => {
      expect(listRequests()).toHaveLength(2);
    });
    const [, next] = listRequests();
    expect(next.method).toBe("GET");
    const query = queryOf(next);
    expect(query.get("start_key")).toBe(LAST_KEY_OF_FIRST_PAGE);
    expect(query.get("direction")).toBe("forward");

    await screen.findByText(FIRST_KEY_OF_SECOND_PAGE);
    const secondPageLinks = viewLinks();
    expect(secondPageLinks).toHaveLength(2);
    expect(secondPageLinks[0].getAttribute("href")).toMatch(
      /\/transactions\/0000000000000011$/,
    );
    // The end of the master leaves no next page, so Next is disabled.
    expect(
      screen.getByRole("button", { name: "Next" }).getAttribute("aria-disabled"),
    ).toBe("true");
    expect(toastError).not.toHaveBeenCalled();
  });

  // A TRAN-DESC X(100) at its full length without a space breaks anywhere and
  // is clamped to one line, so it cannot widen the table past its card; the
  // title and the text keep it whole.
  it("clamps a 100-character description to one line with the full text as its title", async () => {
    const longDescription = "ABCDEFGHIJ".repeat(10);
    const longPage: TransactionPage = {
      ...FIRST_PAGE,
      items: [
        { ...transaction(0), tran_desc: longDescription },
        ...FIRST_PAGE.items.slice(1),
      ],
    };
    answer = (request) =>
      new URL(request.url).pathname === LIST_PATH &&
      !queryOf(request).has("start_key")
        ? jsonResponse(200, longPage)
        : browse(request);
    await renderPage();

    const table = screen.getByRole("table");
    const [headerRow, firstRow] = within(table).getAllByRole("row");
    expect(within(headerRow).getAllByRole("columnheader")[2].textContent).toBe(
      "Description",
    );
    const description = within(table).getByText(longDescription);
    expect(within(firstRow).getAllByRole("cell")[2].contains(description)).toBe(
      true,
    );
    expect(longDescription).toHaveLength(100);
    expect(description.getAttribute("title")).toBe(longDescription);
    expect(description.textContent).toBe(longDescription);
    expect([...description.classList]).toEqual(
      expect.arrayContaining(["line-clamp-1", "whitespace-normal", "wrap-anywhere"]),
    );
    expect(toastError).not.toHaveBeenCalled();
  });
});
