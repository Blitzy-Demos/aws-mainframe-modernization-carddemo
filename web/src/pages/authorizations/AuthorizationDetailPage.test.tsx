// FN tests of the CPVD page (pages/authorizations/AuthorizationDetailPage.tsx,
// COPAUS1C with map COPAU01): opening /authorizations/:authId is ENTER, the
// read of the authorization the summary's "View" names (F-COPAU01-001), and
// PF8 "Next" opens the one the summary list reads one row on (F-COPAU01-006,
// E-59). The read's progress reaches assistive technology through one polite
// status the page keeps mounted from its first render: the read in flight,
// then the authorization's identity, its transaction ID, without its amount
// or card number. The skeleton shown meanwhile is hidden from assistive
// technology. A failed read is toasted and announces nothing there.
//
// The network is the global the API client looks up on every request, stubbed
// per test; every answer is typed with the OpenAPI schema, so drift in the
// response or error shapes fails `tsc`.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
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

import type { components } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { UNEXPECTED_ERROR } from "@/lib/api-errors";

import AuthorizationDetailPage from "./AuthorizationDetailPage";

type AuthDetailResponse = components["schemas"]["AuthDetailResponse"];
type AuthSummaryResponse = components["schemas"]["AuthSummaryResponse"];
type AuthDetailField = components["schemas"]["AuthDetailField"];
type ErrorBody = components["schemas"]["ErrorBody"];
type FetchImpl = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

/** `GET /authorizations` under the client's `/api` base path. */
const SUMMARY_PATH = "/api/authorizations";

/** `GET /authorizations/{id}` under the client's `/api` base path. */
const DETAIL_PATH_PREFIX = `${SUMMARY_PATH}/`;

/** What the test router shows at the summary route. */
const SUMMARY_MARKER = "Authorization summary";

/** Account 1 of the fixture [app/data/ASCII/acctdata.txt:L1], zero-padded. */
const ACCOUNT_1 = "00000000001";

/** The BMS field the errors of `GET /authorizations/{id}` name. */
const CARDNUM: AuthDetailField = "CARDNUM";

/** The status text while the authorization is read. */
const LOADING = "Loading authorization";

/** Longer than the page's 150 ms announce delay, so a due announcement is shown. */
const ANNOUNCE_WAIT_MS = 300;

/**
 * An approved authorization CP00 stored for account 1, on its card
 * 9680294154603697 [app/data/ASCII/cardxref.txt:L49], as the API answers it.
 */
function authorization(
  authId: string,
  tranId: string,
  nextId: string | null,
): AuthDetailResponse {
  return {
    auth_id: authId,
    account_id: ACCOUNT_1,
    card_number: "9680294154603697",
    pa_auth_orig_date: "220701",
    pa_auth_orig_time: "120006",
    approved: "A",
    pa_auth_resp_code: "00",
    pa_auth_resp_reason: "0000",
    decline_reason: "0000-APPROVED",
    pa_processing_code: "000000",
    pa_approved_amt: "17.34",
    pa_pos_entry_mode: "01",
    pa_message_source: "POS",
    pa_merchant_catagory_code: "5411",
    pa_card_expiry_date: "0525",
    pa_auth_type: "POS",
    pa_transaction_id: tranId,
    pa_match_status: "P",
    pa_auth_fraud: "",
    pa_fraud_rpt_date: "",
    pa_merchant_name: "QA MERCHANT",
    pa_merchant_id: "000000000000007",
    pa_merchant_city: "SEATTLE",
    pa_merchant_state: "WA",
    pa_merchant_zip: "98101",
    next_id: nextId,
  };
}

/** The authorization the route opens, with a next one under its summary. */
const NEWEST = authorization("7", "QA00000000007", "6");

/** The next older authorization, the one PF8 opens from NEWEST. */
const FOLLOWING = authorization("6", "QA00000000006", "5");

/** The status text once NEWEST is read: its identity alone. */
const NEWEST_LOADED = `Authorization of transaction ${NEWEST.pa_transaction_id} loaded`;

/** The status text once FOLLOWING is read. */
const FOLLOWING_LOADED = `Authorization of transaction ${FOLLOWING.pa_transaction_id} loaded`;

/** The summary page one row on from NEWEST: FOLLOWING's list row. */
const PAGE_AFTER_NEWEST: AuthSummaryResponse = {
  account_id: ACCOUNT_1,
  customer_id: "000000001",
  cust_first_name: "Immanuel",
  cust_middle_name: "Madeline",
  cust_last_name: "Kessler",
  cust_addr_line_1: "618 Deshaun Route",
  cust_addr_line_2: "Apt. 802",
  cust_addr_line_3: "Altenwerthshire",
  cust_addr_state_cd: "NC",
  cust_addr_zip: "12546",
  cust_phone_num_1: "(908)119-8310",
  acct_credit_limit: "2020.00",
  acct_cash_credit_limit: "1020.00",
  pa_approved_auth_cnt: 2,
  pa_declined_auth_cnt: 0,
  pa_approved_auth_amt: "34.68",
  pa_declined_auth_amt: "0.00",
  pa_credit_balance: "34.68",
  pa_cash_balance: "0.00",
  items: [
    {
      auth_id: FOLLOWING.auth_id,
      pa_transaction_id: FOLLOWING.pa_transaction_id,
      pa_auth_orig_date: FOLLOWING.pa_auth_orig_date,
      pa_auth_orig_time: FOLLOWING.pa_auth_orig_time,
      pa_auth_type: FOLLOWING.pa_auth_type,
      approved: FOLLOWING.approved,
      pa_match_status: FOLLOWING.pa_match_status,
      pa_approved_amt: FOLLOWING.pa_approved_amt,
    },
  ],
  next_key: FOLLOWING.auth_id,
  prev_key: FOLLOWING.auth_id,
};

/** The answer to a request no test expects; afterEach reports it. */
const UNEXPECTED: ErrorBody = { code: "UNEXPECTED", message: "Unexpected error" };

function jsonResponse(
  status: number,
  body: AuthDetailResponse | AuthSummaryResponse | ErrorBody,
): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** A response held until the test sends it, so a read stays in flight. */
type HeldResponse = { reply: Promise<Response>; send: (response: Response) => void };

function holdResponse(): HeldResponse {
  let send: (response: Response) => void = () => undefined;
  const reply = new Promise<Response>((resolve) => {
    send = resolve;
  });
  return { reply, send };
}

let fetchMock: Mock<FetchImpl>;
let answer: (request: Request) => Response | Promise<Response>;
let unexpected: string[];
let queryClient: QueryClient;
let toastError: MockInstance<typeof toast.error>;

/** Whether `request` is the read of authorization `authId`. */
function isDetailRead(request: Request, authId: string): boolean {
  const url = new URL(request.url);
  return (
    request.method === "GET" &&
    url.pathname === `${DETAIL_PATH_PREFIX}${authId}` &&
    url.search === ""
  );
}

/** Whether `request` is PF8's summary read one row on from NEWEST. */
function isNextLookup(request: Request): boolean {
  const url = new URL(request.url);
  const query = url.searchParams;
  return (
    request.method === "GET" &&
    url.pathname === SUMMARY_PATH &&
    query.get("account_id") === ACCOUNT_1 &&
    query.get("start_key") === NEWEST.auth_id &&
    query.get("direction") === "forward" &&
    query.get("page_size") === "1"
  );
}

/** The store the API reads: NEWEST, FOLLOWING and the page between them. */
function readStore(request: Request): Response {
  if (isDetailRead(request, NEWEST.auth_id)) {
    return jsonResponse(200, NEWEST);
  }
  if (isDetailRead(request, FOLLOWING.auth_id)) {
    return jsonResponse(200, FOLLOWING);
  }
  if (isNextLookup(request)) {
    return jsonResponse(200, PAGE_AFTER_NEWEST);
  }
  const url = new URL(request.url);
  unexpected.push(`${request.method} ${url.pathname}${url.search}`);
  return jsonResponse(500, UNEXPECTED);
}

/** Every request the page sent, as method, path and query, in the order sent. */
function sent(): string[] {
  return fetchMock.mock.calls.map(([input, init]) => {
    const request = input instanceof Request ? input : new Request(input, init);
    const url = new URL(request.url);
    return `${request.method} ${url.pathname}${url.search}`;
  });
}

/** The polite status the page keeps mounted to announce the read. */
function readStatus(): HTMLElement {
  return screen.getByRole("status");
}

/** Waits out the announce delay, so a status still empty announces nothing. */
async function afterAnnounceDelay(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, ANNOUNCE_WAIT_MS);
    });
  });
}

/** The location of NEWEST as the summary's "View" link opens it. */
const NEWEST_ROUTE = `/authorizations/${NEWEST.auth_id}?account_id=${ACCOUNT_1}`;

/**
 * Renders the page at `path` in a data router nested as the application's
 * route table nests it, which PageHeader's useMatches needs: the summary at
 * /authorizations and the detail at /authorizations/:authId.
 */
function renderAt(path: string): { user: UserEvent; router: ReturnType<typeof createMemoryRouter> } {
  queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(
    [
      {
        path: "/authorizations",
        handle: { title: "View Authorizations", crumb: "Authorizations" } satisfies RouteHandle,
        children: [
          { index: true, element: <p>{SUMMARY_MARKER}</p> },
          {
            path: ":authId",
            element: <AuthorizationDetailPage />,
            handle: {
              title: "View Authorization Details",
              crumb: "View Authorization Details",
            } satisfies RouteHandle,
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
  return { user, router };
}

beforeEach(() => {
  unexpected = [];
  answer = readStore;
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

describe("AuthorizationDetailPage (CPVD, COPAU01) read status", () => {
  // F-COPAU01-001: the route's read. The status is the page's own, mounted
  // before any record exists and outside the skeleton, which is hidden from
  // assistive technology rather than a status of its own whose removal would
  // announce nothing. A record without a transaction ID is announced by its
  // own authorization ID.
  it.each([
    {
      record: "a transaction ID",
      tranId: NEWEST.pa_transaction_id,
      loaded: NEWEST_LOADED,
    },
    {
      record: "a blank transaction ID",
      tranId: "",
      loaded: `Authorization ${NEWEST.auth_id} loaded`,
    },
  ])("announces the read in flight, then the authorization read, for $record", async ({
    tranId,
    loaded,
  }) => {
    const held = holdResponse();
    answer = (request) =>
      isDetailRead(request, NEWEST.auth_id) ? held.reply : readStore(request);
    renderAt(NEWEST_ROUTE);

    const status = await screen.findByRole("status");
    expect(status.classList.contains("sr-only")).toBe(true);
    await waitFor(() => {
      expect(status.textContent).toBe(LOADING);
    });
    // The skeleton is the only card while the read is held: purely visual.
    const skeleton = document.querySelector("[data-slot='card']");
    expect(skeleton).not.toBeNull();
    expect(skeleton?.getAttribute("aria-hidden")).toBe("true");
    expect(skeleton?.hasAttribute("role")).toBe(false);
    expect(skeleton?.hasAttribute("aria-busy")).toBe(false);
    expect(skeleton?.hasAttribute("aria-label")).toBe(false);
    expect(screen.getAllByRole("status")).toEqual([status]);
    expect(status.closest("[aria-busy='true']")).toBeNull();

    held.send(jsonResponse(200, { ...NEWEST, pa_transaction_id: tranId }));
    await waitFor(() => {
      expect(status.textContent).toBe(loaded);
    });
    expect(screen.getByRole("heading", { level: 2, name: "Authorization" })).not.toBeNull();
    // The one status, mounted from the read's start to its end, outside the card.
    expect(readStatus()).toBe(status);
    expect(status.closest("[data-slot='card']")).toBeNull();
    expect(sent()).toEqual([`GET ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`]);
    expect(toastError).not.toHaveBeenCalled();
  });

  // V-COPAUS1C-005 (an ID no detail holds, a 404 without a message, which the
  // page toasts as UNEXPECTED_ERROR) and V-COPAUS1C-006 (a failed detail read,
  // 500): the toast carries the failure, no record is shown and the status
  // announces nothing.
  it.each([
    {
      status: 404,
      body: { code: "NOT_FOUND", message: "", fields: [{ field: CARDNUM, message: "" }] },
      toasted: UNEXPECTED_ERROR,
    },
    {
      status: 500,
      body: {
        code: "UNEXPECTED",
        message: " System error while reading Auth Details: Code:AO",
        fields: [
          { field: CARDNUM, message: " System error while reading Auth Details: Code:AO" },
        ],
      },
      toasted: " System error while reading Auth Details: Code:AO",
    },
  ])("toasts a $status answer and leaves the status empty", async ({ status, body, toasted }) => {
    answer = (request) =>
      isDetailRead(request, NEWEST.auth_id) ? jsonResponse(status, body) : readStore(request);
    renderAt(NEWEST_ROUTE);

    await waitFor(() => {
      expect(toastError).toHaveBeenCalledWith(toasted);
    });
    await afterAnnounceDelay();
    expect(readStatus().textContent).toBe("");
    expect(toastError).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("heading", { level: 2, name: "Authorization" })).toBeNull();
    expect(document.querySelector("[data-slot='card']")).toBeNull();
    expect(sent()).toEqual([`GET ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`]);
  });

  // F-COPAU01-006, E-59: PF8 opens the next authorization on the same page
  // component, so the one status stays mounted, is emptied and announces the
  // new read and then the next authorization's identity.
  it("announces the read and the identity of the authorization Next opens", async () => {
    const { user, router } = renderAt(NEWEST_ROUTE);
    const status = await screen.findByRole("status");
    await waitFor(() => {
      expect(status.textContent).toBe(NEWEST_LOADED);
    });

    const held = holdResponse();
    answer = (request) =>
      isDetailRead(request, FOLLOWING.auth_id) ? held.reply : readStore(request);
    await user.click(screen.getByRole("button", { name: "Next" }));

    await waitFor(() => {
      expect(router.state.location.pathname).toBe(`/authorizations/${FOLLOWING.auth_id}`);
    });
    await waitFor(() => {
      expect(status.textContent).toBe(LOADING);
    });

    held.send(jsonResponse(200, FOLLOWING));
    await waitFor(() => {
      expect(status.textContent).toBe(FOLLOWING_LOADED);
    });
    expect(readStatus()).toBe(status);
    expect(sent()).toEqual([
      `GET ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`,
      `GET ${SUMMARY_PATH}?account_id=${ACCOUNT_1}&start_key=${NEWEST.auth_id}&direction=forward&page_size=1`,
      `GET ${DETAIL_PATH_PREFIX}${FOLLOWING.auth_id}`,
    ]);
    expect(toastError).not.toHaveBeenCalled();
  });
});
