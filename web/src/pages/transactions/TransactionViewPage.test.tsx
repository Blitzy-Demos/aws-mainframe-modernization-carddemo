// FN tests of the CT01 transaction view (TransactionViewPage; COTRN01C, map
// COTRN01). The view takes its key from the route, the TRAN-ID the list's
// "View" row action carries (F-COTRN01-001, E-61): the page has no TRNIDIN
// input, no Fetch submit and no Reset (E-56, E-62). So the valid input is a
// route key the COTRN01C edit accepts, read at once with
// GET /transactions/{id} and shown field by field (P-COTRN01C-005); the
// invalid inputs are a blank or over-long key, which the edit rejects under
// "Tran ID" without a request (V-COTRN01C-001); and every server rejection
// (V-COTRN01C-001 to V-COTRN01C-003) reaches the user as a toast and as the
// card's message line, with the display fields cleared. With no input on the
// screen, the server's TRNIDIN entry marks no field and moves no cursor. A
// polite status beside the card announces each read and the transaction's
// identity, and nothing for a key that is not read or a read that fails.
//
// The network is the global the API client looks up on every request, stubbed
// per test, and the assertions read the Request objects it received.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
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

import TransactionViewPage from "./TransactionViewPage";

type ViewResponses = paths["/transactions/{id}"]["get"]["responses"];
type TransactionResponse = ViewResponses[200]["content"]["application/json"];
type ErrorBody = components["schemas"]["ErrorBody"];
type ViewErrorStatus = 400 | 404 | 500;
type ViewErrorBody = ViewResponses[ViewErrorStatus]["content"]["application/json"];
type FetchImpl = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

// `GET /transactions/{id}` under the client's `/api` base path.
const VIEW_PATH_PREFIX = "/api/transactions/";

// The list route, the target of F3=Back and F5=Browse Tran.
const LIST_ROUTE = "/transactions";

// What the test router shows at the list route.
const LIST_MARKER = "Transaction list";

// A static route below /transactions, where the view renders without a key.
// The application's route table has none (E-61); the static segment outranks
// `:tranId`, so the page reads no `tranId` there.
const KEYLESS_ROUTE = "/transactions/lookup";

// The COTRN01 screen title [app/bms/COTRN01.bms:L75-79].
const TITLE = "View Transaction";

// The heading of the card that holds the key and the display fields.
const PANEL_NAME = "Transaction details";

// The caption of the key the route carries, TRNIDIN's place on the map.
const KEY_CAPTION = "Tran ID";

// The BMS field every COTRN01C error places the cursor on (P-COTRN01C-007).
const TRNIDIN: components["schemas"]["TransactionViewField"] = "TRNIDIN";

// COTRN01C's blank-key text (V-COTRN01C-001), byte for byte.
const TRAN_ID_EMPTY = "Tran ID can NOT be empty...";

// COTRN01C's NOTFND text (V-COTRN01C-002), byte for byte.
const TRAN_ID_NOT_FOUND = "Transaction ID NOT found...";

// COTRN01C's text for any other READ failure (V-COTRN01C-003), byte for byte.
const UNABLE_TO_LOOKUP = "Unable to lookup Transaction...";

// zod's text for a key over TRNIDIN's 16 characters, which a 3270 field
// cannot send and no audit row covers.
const OVER_LENGTH = "String must contain at most 16 character(s)";

// The live transaction the view reads: the first key CT02 allocates (E-09).
const TRAN_ID = "0000000000000001";

// The status text while the transaction is read.
const LOADING = "Loading transaction";

// The status text once TRAN_ID is read: its identity alone.
const LOADED = `Transaction ${TRAN_ID} loaded`;

// Longer than the page's 150 ms announce delay, so a due announcement is shown.
const ANNOUNCE_WAIT_MS = 300;

// Record 1 of app/data/ASCII/dailytran.txt (CVTRA06Y) as the transaction
// master holds it once posted: `DALYTRAN-AMT` `0000005047G` decoded from its
// trailing overpunch, the processing timestamp in the CBTRN02C
// Z-GET-DB2-FORMAT-TIMESTAMP form at the processing clock the tests pin.
const TRANSACTION: TransactionResponse = {
  tran_id: TRAN_ID,
  tran_type_cd: "01",
  tran_cat_cd: "0001",
  tran_source: "POS TERM",
  tran_desc: "Purchase at Abshire-Lowe",
  tran_amt: "504.77",
  tran_merchant_id: "800000000",
  tran_merchant_name: "Abshire-Lowe",
  tran_merchant_city: "North Enoshaven",
  tran_merchant_zip: "72112",
  tran_card_num: "4859452612877065",
  tran_orig_ts: "2022-06-10 19:27:53.000000",
  tran_proc_ts: "2022-07-01-12.00.00.000000",
};

// The 13 COTRN01 display fields in map order [app/bms/COTRN01.bms:L100-258]
// with the values COTRN01C moves into them [app/cbl/COTRN01C.cbl:L176-192],
// each the API string as sent.
const DISPLAY_FIELDS: readonly (readonly [string, string])[] = [
  ["Transaction ID", TRANSACTION.tran_id],
  ["Card Number", TRANSACTION.tran_card_num],
  ["Type CD", TRANSACTION.tran_type_cd],
  ["Category CD", TRANSACTION.tran_cat_cd],
  ["Source", TRANSACTION.tran_source],
  ["Description", TRANSACTION.tran_desc],
  ["Amount", TRANSACTION.tran_amt],
  ["Orig Date", TRANSACTION.tran_orig_ts],
  ["Proc Date", TRANSACTION.tran_proc_ts],
  ["Merchant ID", TRANSACTION.tran_merchant_id],
  ["Merchant Name", TRANSACTION.tran_merchant_name],
  ["Merchant City", TRANSACTION.tran_merchant_city],
  ["Merchant Zip", TRANSACTION.tran_merchant_zip],
];

// A COTRN01C error as the server answers it: the kind's code, the message,
// and the one TRNIDIN entry with the same text.
function viewError(code: string, message: string): ViewErrorBody {
  return { code, message, fields: [{ field: TRNIDIN, message }] };
}

// The answer to a request no test expects; afterEach reports it.
const UNEXPECTED: ErrorBody = { code: "UNEXPECTED", message: "Unexpected error" };

function jsonResponse(status: number, body: TransactionResponse | ErrorBody): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

let fetchMock: Mock<FetchImpl>;
let answer: (request: Request) => Response | Promise<Response>;
let unexpected: string[];
let queryClient: QueryClient;
let toastError: MockInstance<typeof toast.error>;

// The view the live master answers: TRAN_ID is TRANSACTION, nothing else is.
function readMaster(request: Request): Response {
  const url = new URL(request.url);
  if (
    request.method === "GET" &&
    url.pathname === `${VIEW_PATH_PREFIX}${TRAN_ID}` &&
    url.search === ""
  ) {
    return jsonResponse(200, TRANSACTION);
  }
  unexpected.push(`${request.method} ${url.pathname}${url.search}`);
  return jsonResponse(500, UNEXPECTED);
}

// A response held until the test sends it, so a read stays in flight.
type HeldResponse = { reply: Promise<Response>; send: (response: Response) => void };

function holdResponse(): HeldResponse {
  let send: (response: Response) => void = () => undefined;
  const reply = new Promise<Response>((resolve) => {
    send = resolve;
  });
  return { reply, send };
}

// Every request the page sent, in the order sent.
function requests(): Request[] {
  return fetchMock.mock.calls.map(([input, init]) =>
    input instanceof Request ? input : new Request(input, init),
  );
}

// Asserts that the page sent exactly one request, the read of `tranId`.
function expectOneRead(tranId: string): void {
  const [read, ...rest] = requests();
  expect(rest).toEqual([]);
  expect(read.method).toBe("GET");
  const url = new URL(read.url);
  expect(url.pathname).toBe(`${VIEW_PATH_PREFIX}${tranId}`);
  expect(url.search).toBe("");
}

// The card that holds the key and the display fields.
function panel(): Promise<HTMLElement> {
  return screen.findByRole("region", { name: PANEL_NAME });
}

// The polite status beside the card that announces the read.
function readStatus(): HTMLElement {
  return screen.getByRole("status");
}

// Waits out the announce delay, so a status still empty announces nothing.
async function afterAnnounceDelay(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, ANNOUNCE_WAIT_MS);
    });
  });
}

// The captions the card shows, in screen order.
function captions(region: HTMLElement): string[] {
  return Array.from(region.querySelectorAll("dt"), (term) => term.textContent ?? "");
}

// Each caption with the text of its value, in screen order.
function captionedValues(region: HTMLElement): [string, string][] {
  return Array.from(region.querySelectorAll("dt"), (term) => [
    term.textContent ?? "",
    term.nextElementSibling?.textContent ?? "",
  ]);
}

// The value element of `caption` in the card.
function valueOf(region: HTMLElement, caption: string): HTMLElement {
  const term = within(region).getByText(caption, { selector: "dt" });
  const value = term.nextElementSibling;
  if (!(value instanceof HTMLElement) || value.tagName !== "DD") {
    throw new Error(`the caption ${caption} has no value`);
  }
  return value;
}

// Renders the page at `path` in a data router nested as the application's
// route table nests it, which PageHeader's useMatches needs: the list at
// /transactions, the view at /transactions/:tranId, and the test-only
// key-less route.
function renderAt(path: string): UserEvent {
  queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const viewHandle = { title: TITLE, crumb: TITLE } satisfies RouteHandle;
  const router = createMemoryRouter(
    [
      {
        path: "/",
        handle: { crumb: "Home" } satisfies RouteHandle,
        children: [
          { index: true, element: <p>home</p> },
          {
            path: "transactions",
            handle: { title: "List Transactions", crumb: "Transactions" } satisfies RouteHandle,
            children: [
              { index: true, element: <p>{LIST_MARKER}</p> },
              { path: "lookup", element: <TransactionViewPage />, handle: viewHandle },
              { path: ":tranId", element: <TransactionViewPage />, handle: viewHandle },
            ],
          },
        ],
      },
    ],
    { initialEntries: [path] },
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
  return user;
}

beforeEach(() => {
  unexpected = [];
  answer = readMaster;
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

describe("TransactionViewPage (CT01, COTRN01)", () => {
  // F-COTRN01-001, P-COTRN01C-005: the key the list selected is read at once,
  // and every display field shows the record's value as the API sent it.
  it("reads the route's Tran ID with GET /transactions/{id} and shows every field", async () => {
    renderAt(`${LIST_ROUTE}/${TRAN_ID}`);

    const region = await panel();
    await waitFor(() => {
      expect(region.getAttribute("aria-busy")).toBe("false");
    });
    expectOneRead(TRAN_ID);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(TITLE);
    expect(captionedValues(region)).toEqual([[KEY_CAPTION, TRAN_ID], ...DISPLAY_FIELDS]);
    // TRAN-AMT S9(09)V99 stays the decimal text, never a reformatted number.
    expect(valueOf(region, "Amount").textContent).toBe("504.77");
    expect(within(region).queryByRole("alert")).toBeNull();
    expect(toastError).not.toHaveBeenCalled();
  });

  // The read's progress reaches assistive technology through a polite status
  // that stays mounted beside the card, outside its busy subtree: the read in
  // flight, then the transaction's identity alone, without its amount or card
  // number.
  it("announces the read in flight, then the transaction read, in a status outside the card", async () => {
    const held = holdResponse();
    answer = (request) =>
      new URL(request.url).pathname === `${VIEW_PATH_PREFIX}${TRAN_ID}`
        ? held.reply
        : readMaster(request);
    renderAt(`${LIST_ROUTE}/${TRAN_ID}`);

    const region = await panel();
    const status = readStatus();
    expect(status.classList.contains("sr-only")).toBe(true);
    expect(region.contains(status)).toBe(false);
    await waitFor(() => {
      expect(status.textContent).toBe(LOADING);
    });
    expect(region.getAttribute("aria-busy")).toBe("true");
    expect(status.closest("[aria-busy='true']")).toBeNull();

    held.send(jsonResponse(200, TRANSACTION));
    await waitFor(() => {
      expect(status.textContent).toBe(LOADED);
    });
    expect(region.getAttribute("aria-busy")).toBe("false");
    // The one status, mounted from the read's start to its end.
    expect(readStatus()).toBe(status);
    expectOneRead(TRAN_ID);
  });

  // A repeated read of the key, such as the refetch when the window regains
  // focus, empties the status, announces the read in flight and then the
  // same transaction again, though the record read is unchanged.
  it("empties the status and announces a repeated read of the same key again", async () => {
    renderAt(`${LIST_ROUTE}/${TRAN_ID}`);
    await panel();
    const status = readStatus();
    await waitFor(() => {
      expect(status.textContent).toBe(LOADED);
    });

    const held = holdResponse();
    answer = (request) =>
      new URL(request.url).pathname === `${VIEW_PATH_PREFIX}${TRAN_ID}`
        ? held.reply
        : readMaster(request);
    let refetched: Promise<void> = Promise.resolve();
    await act(async () => {
      refetched = queryClient.refetchQueries();
    });
    await waitFor(() => {
      expect(status.textContent).toBe("");
    });
    await waitFor(() => {
      expect(status.textContent).toBe(LOADING);
    });

    held.send(jsonResponse(200, TRANSACTION));
    await act(async () => {
      await refetched;
    });
    await waitFor(() => {
      expect(status.textContent).toBe(LOADED);
    });
    expect(requests().map((read) => `${read.method} ${new URL(read.url).pathname}`)).toEqual([
      `GET ${VIEW_PATH_PREFIX}${TRAN_ID}`,
      `GET ${VIEW_PATH_PREFIX}${TRAN_ID}`,
    ]);
  });

  // V-COTRN01C-001 (blank or LOW-VALUES) and TRNIDIN's 16-character width: the
  // edit rejects the key under its caption, as the COBOL message line and
  // `MOVE -1 TO TRNIDINL` mark it, and nothing is read.
  it.each([
    {
      input: "a Tran ID of spaces (V-COTRN01C-001)",
      segment: "%20%20",
      key: "  ",
      message: TRAN_ID_EMPTY,
    },
    {
      input: "a Tran ID of LOW-VALUES (V-COTRN01C-001)",
      segment: "%00%00",
      key: "\u0000\u0000",
      message: TRAN_ID_EMPTY,
    },
    {
      input: "a Tran ID longer than TRNIDIN",
      segment: "00000000000000001",
      key: "00000000000000001",
      message: OVER_LENGTH,
    },
  ])("reads nothing for $input and shows the edit's text under the key", async ({
    segment,
    key,
    message,
  }) => {
    renderAt(`${LIST_ROUTE}/${segment}`);

    const region = await panel();
    const keyValue = valueOf(region, KEY_CAPTION);
    expect(keyValue.querySelector("span")?.textContent).toBe(key);
    const error = within(keyValue).getByRole("alert");
    expect(error.getAttribute("data-slot")).toBe("field-error");
    expect(error.textContent).toBe(message);
    // The rejected key reads no record, so no display field is shown.
    expect(captions(region)).toEqual([KEY_CAPTION]);
    expect(region.getAttribute("aria-busy")).toBe("false");
    expect(fetchMock).not.toHaveBeenCalled();
    expect(toastError).not.toHaveBeenCalled();
    // Nothing is read, so the status beside the card announces nothing.
    await afterAnnounceDelay();
    expect(readStatus().textContent).toBe("");
  });

  // Every audited COTRN01C rejection the server answers: a toast and the
  // card's message line carry its text, and the display fields stay cleared
  // [app/cbl/COTRN01C.cbl:L158-171]. The 400 is the server's own blank-key
  // edit; the client schema stops a blank key first, but the server is
  // authoritative and its answer is shown like any other.
  it.each([
    {
      status: 400,
      row: "V-COTRN01C-001",
      tranId: "0000000000000002",
      body: viewError("EMPTY", TRAN_ID_EMPTY),
    },
    {
      status: 404,
      row: "V-COTRN01C-002",
      tranId: "9999999999999999",
      body: viewError("NOT_FOUND", TRAN_ID_NOT_FOUND),
    },
    {
      status: 500,
      row: "V-COTRN01C-003",
      tranId: TRAN_ID,
      body: viewError("UNEXPECTED", UNABLE_TO_LOOKUP),
    },
  ])("shows the server's $status ($row) as a toast and the message line, with no fields", async ({
    status,
    tranId,
    body,
  }) => {
    answer = (request) =>
      new URL(request.url).pathname === `${VIEW_PATH_PREFIX}${tranId}`
        ? jsonResponse(status, body)
        : readMaster(request);
    renderAt(`${LIST_ROUTE}/${tranId}`);

    const region = await panel();
    await waitFor(() => {
      expect(toastError).toHaveBeenCalledWith(body.message);
    });
    expectOneRead(tranId);
    const line = await within(region).findByText(body.message);
    expect(line.getAttribute("data-slot")).toBe("card-description");
    expect(captionedValues(region)).toEqual([[KEY_CAPTION, tranId]]);
    // The key passed the client edit, so no field error stands beside it.
    expect(within(region).queryByRole("alert")).toBeNull();
    expect(region.getAttribute("aria-busy")).toBe("false");
    // One toast per failed read, however often the page renders.
    expect(toastError).toHaveBeenCalledTimes(1);
    // The toast and the message line carry the failure; the status announces
    // no transaction.
    await afterAnnounceDelay();
    expect(readStatus().textContent).toBe("");
  });

  // A later read of the same key, such as the refetch when the window regains
  // focus, that fails sends the map with its display fields cleared
  // [app/cbl/COTRN01C.cbl:L158-171]: the record read earlier is not shown
  // beside the failure.
  it("clears the fields an earlier read showed when a later read of the key fails", async () => {
    renderAt(`${LIST_ROUTE}/${TRAN_ID}`);
    const region = await panel();
    await waitFor(() => {
      expect(captionedValues(region)).toEqual([[KEY_CAPTION, TRAN_ID], ...DISPLAY_FIELDS]);
    });
    await waitFor(() => {
      expect(readStatus().textContent).toBe(LOADED);
    });

    const failure = viewError("UNEXPECTED", UNABLE_TO_LOOKUP);
    answer = () => jsonResponse(500, failure);
    await act(async () => {
      await queryClient.refetchQueries();
    });

    await waitFor(() => {
      expect(toastError).toHaveBeenCalledWith(UNABLE_TO_LOOKUP);
    });
    expect(toastError).toHaveBeenCalledTimes(1);
    const reads = requests();
    expect(reads.map((read) => `${read.method} ${new URL(read.url).pathname}`)).toEqual([
      `GET ${VIEW_PATH_PREFIX}${TRAN_ID}`,
      `GET ${VIEW_PATH_PREFIX}${TRAN_ID}`,
    ]);
    expect(within(region).getByText(UNABLE_TO_LOOKUP).getAttribute("data-slot")).toBe(
      "card-description",
    );
    expect(captionedValues(region)).toEqual([[KEY_CAPTION, TRAN_ID]]);
    // The cleared record is no longer announced.
    await afterAnnounceDelay();
    expect(readStatus().textContent).toBe("");
  });

  // E-61: the route table gives the view no key-less route. Rendered without
  // a key, the page sends what COTRN01C's first entry without a selected
  // TRAN-ID sends, an empty map, and reads nothing
  // [app/cbl/COTRN01C.cbl:L99-109].
  it("reads nothing without a route key and shows only the header", async () => {
    renderAt(KEYLESS_ROUTE);

    const heading = await screen.findByRole("heading", { level: 1 });
    expect(heading.textContent).toBe(TITLE);
    expect(screen.queryByRole("region", { name: PANEL_NAME })).toBeNull();
    expect(screen.getByRole("link", { name: "Back" }).getAttribute("href")).toBe(LIST_ROUTE);
    expect(
      screen.getByRole("link", { name: "Browse transactions" }).getAttribute("href"),
    ).toBe(LIST_ROUTE);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(toastError).not.toHaveBeenCalled();
    // No read starts, so no read status is shown or announced.
    await afterAnnounceDelay();
    expect(screen.queryByRole("status")).toBeNull();
  });

  // F-COTRN01-002 (F3=Back) and F-COTRN01-004 (F5=Browse Tran.) both return to
  // the transaction list, COTRN00C [app/cbl/COTRN01C.cbl:L115-127].
  it.each(["Back", "Browse transactions"])("%s returns to the transaction list", async (name) => {
    const user = renderAt(`${LIST_ROUTE}/${TRAN_ID}`);
    const region = await panel();
    await waitFor(() => {
      expect(region.getAttribute("aria-busy")).toBe("false");
    });

    const link = screen.getByRole("link", { name });
    expect(link.getAttribute("href")).toBe(LIST_ROUTE);
    await user.click(link);

    await screen.findByText(LIST_MARKER);
    expect(screen.queryByRole("region", { name: PANEL_NAME })).toBeNull();
    expectOneRead(TRAN_ID);
  });
});
