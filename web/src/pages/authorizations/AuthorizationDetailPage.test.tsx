// FN tests of the CPVD page (pages/authorizations/AuthorizationDetailPage.tsx,
// COPAUS1C with map COPAU01): opening /authorizations/:authId is ENTER, the
// read of the authorization the summary's "View" names (F-COPAU01-001), and
// PF8 "Next" opens the one the summary list reads one row on (F-COPAU01-006,
// E-59). The read's progress reaches assistive technology through one polite
// status the page keeps mounted from its first render: the read in flight,
// then the authorization's identity, its transaction ID, without its amount
// or card number. The skeleton shown meanwhile is hidden from assistive
// technology. A failed read announces nothing there: one with a message is
// toasted and kept as the message line, and the 404 without one (COPAUS1C's
// no-message, no-authorization screen, E-49) shows the header and Back alone.
//
// The network is the global the API client looks up on every request, stubbed
// per test; every answer is typed with the OpenAPI schema, so drift in the
// response or error shapes fails `tsc`.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
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

import { authorizationKeys } from "@/api/queries/authorizations";
import type { components } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";

import AuthorizationDetailPage from "./AuthorizationDetailPage";

type AuthDetailResponse = components["schemas"]["AuthDetailResponse"];
type AuthSummaryResponse = components["schemas"]["AuthSummaryResponse"];
type FraudToggleResponse = components["schemas"]["FraudToggleResponse"];
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
  body: AuthDetailResponse | AuthSummaryResponse | FraudToggleResponse | ErrorBody,
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

/** Whether `request` is PF5's fraud toggle of authorization `authId`. */
function isFraudToggle(request: Request, authId: string): boolean {
  const url = new URL(request.url);
  return (
    request.method === "PUT" &&
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

/**
 * The elements outside the toasts whose whole text is `text`: the screen's
 * message line, compared byte for byte, a leading space included.
 */
function messageLines(text: string): HTMLElement[] {
  return screen.queryAllByText(
    (_content, element) =>
      element !== null &&
      element.children.length === 0 &&
      element.textContent === text &&
      element.closest("[data-sonner-toaster]") === null,
  );
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

/** V-COPAUS1C-006's text for IMS status `AO` (E-44). */
const DETAIL_READ_FAILED = " System error while reading Auth Details: Code:AO";

/** The API's 404 text for a path no route serves (crates/domain/src/messages.rs ROUTE_NOT_FOUND). */
const ROUTE_NOT_FOUND = "Resource not found";

/**
 * The failed reads of the record, each with the text the page toasts and
 * keeps as its message line, or `null` when it shows none:
 *
 * - V-COPAUS1C-005, an ID no detail holds (a purged authorization, say): a
 *   404 whose message is blank, empty or spaces. COPAUS1C sets no message and
 *   shows no authorization (E-49), so nothing is toasted or shown.
 * - A 404 that carries a text, such as the API's answer to a path no route
 *   serves, shown as every other message is.
 * - V-COPAUS1C-006, a failed detail read, 500.
 */
const READ_FAILURES: ReadonlyArray<{
  answer: string;
  status: number;
  body: ErrorBody;
  shown: string | null;
}> = [
  {
    answer: "404 without a message",
    status: 404,
    body: { code: "NOT_FOUND", message: "", fields: [{ field: CARDNUM, message: "" }] },
    shown: null,
  },
  {
    answer: "404 whose message is spaces",
    status: 404,
    body: { code: "NOT_FOUND", message: "   ", fields: [{ field: CARDNUM, message: "   " }] },
    shown: null,
  },
  {
    answer: "404 with a message",
    status: 404,
    body: { code: "NOT_FOUND", message: ROUTE_NOT_FOUND },
    shown: ROUTE_NOT_FOUND,
  },
  {
    answer: "500 with a message",
    status: 500,
    body: {
      code: "UNEXPECTED",
      message: DETAIL_READ_FAILED,
      fields: [{ field: CARDNUM, message: DETAIL_READ_FAILED }],
    },
    shown: DETAIL_READ_FAILED,
  },
];

/**
 * The text the page shows outside its header, its read status and the
 * toasts, leaf element by leaf element: the message line, or a record's
 * fields. The skeleton's blocks hold none.
 */
function pageText(): string[] {
  const header = document.querySelector("header");
  return screen
    .queryAllByText(
      (_content, element) =>
        element !== null &&
        element.children.length === 0 &&
        element.textContent !== "" &&
        header?.contains(element) !== true &&
        element.closest("[role='status']") === null &&
        element.closest("[data-sonner-toaster]") === null,
    )
    .map((element) => element.textContent ?? "");
}

/**
 * The screen once a read of NEWEST failed, the first read or a read again
 * alike: nothing announced, no record, PF5 or PF8, and Back to the account's
 * summary. A failure with a message is toasted once and its text kept as the
 * message line, the only text outside the header. The 404 without one
 * (`shown` null) is neither toasted nor shown: the header and Back alone, as
 * for a rejected key.
 */
async function expectFailedRead(shown: string | null): Promise<void> {
  if (shown === null) {
    await waitFor(() => {
      expect(queryClient.getQueryState(authorizationKeys.detail(NEWEST.auth_id))).toMatchObject({
        status: "error",
        fetchStatus: "idle",
      });
    });
  } else {
    await waitFor(() => {
      expect(toastError).toHaveBeenCalledWith(shown);
    });
    await waitFor(() => {
      expect(messageLines(shown)).toHaveLength(1);
    });
  }
  await afterAnnounceDelay();
  expect(readStatus().textContent).toBe("");
  expect(toastError).toHaveBeenCalledTimes(shown === null ? 0 : 1);
  expect(pageText()).toEqual(shown === null ? [] : [shown]);
  expect(screen.queryByRole("heading", { level: 2, name: "Authorization" })).toBeNull();
  expect(document.querySelector("[data-slot='card']")).toBeNull();
  expect(screen.queryByText(NEWEST.pa_transaction_id)).toBeNull();
  expect(screen.queryByRole("button", { name: "Mark fraud" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Next" })).toBeNull();
  expect(screen.getByRole("link", { name: "Back" }).getAttribute("href")).toBe(
    `/authorizations?account_id=${ACCOUNT_1}`,
  );
}

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

  // V-COPAUS1C-005 and V-COPAUS1C-006 on the first read: no record is shown
  // and the status announces nothing. A failure with a message is carried by
  // the toast and the message line; the 404 without one shows neither (E-49).
  it.each(READ_FAILURES)(
    "shows a $answer as COPAUS1C does and leaves the status empty",
    async ({ status, body, shown }) => {
      answer = (request) =>
        isDetailRead(request, NEWEST.auth_id) ? jsonResponse(status, body) : readStore(request);
      renderAt(NEWEST_ROUTE);

      await expectFailedRead(shown);
      expect(sent()).toEqual([`GET ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`]);
    },
  );

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

/** The detail route of `authId`, from the summary of account 1, as the router receives it. */
function routeOf(authId: string): string {
  return `/authorizations/${encodeURIComponent(authId)}?account_id=${ACCOUNT_1}`;
}

describe("AuthorizationDetailPage (CPVD, COPAU01) route key", () => {
  // V-COPAUS1C-001, E-49: a key that is not a positive integer up to
  // i64::MAX is the COBOL path that sets the error flag with no message and
  // leaves the detail fields empty. The page sends nothing and shows its
  // header with Back to the account's summary, and nothing else.
  it.each([
    { key: "abc", why: "not a number" },
    { key: "0", why: "not positive" },
    { key: "-1", why: "negative" },
    { key: "9223372036854775808", why: "above i64::MAX" },
    { key: "+", why: "a sign without digits" },
    { key: " ", why: "blank" },
  ])("sends nothing for the key $key, $why, and shows Back alone", async ({ key }) => {
    renderAt(routeOf(key));

    const back = await screen.findByRole("link", { name: "Back" });
    expect(back.getAttribute("href")).toBe(`/authorizations?account_id=${ACCOUNT_1}`);
    expect(
      screen.getByRole("heading", { level: 1, name: "View Authorization Details" }),
    ).not.toBeNull();
    await afterAnnounceDelay();
    expect(sent()).toEqual([]);
    expect(document.querySelector("[data-slot='card']")).toBeNull();
    expect(document.querySelector("[data-slot='skeleton']")).toBeNull();
    expect(screen.queryByRole("button", { name: "Mark fraud" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Remove fraud" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Next" })).toBeNull();
    expect(readStatus().textContent).toBe("");
    expect(toastError).not.toHaveBeenCalled();
  });

  // The server's parse reads an optional `+` and leading zeros, so these keys
  // name authorization 7: each is sent exactly as routed, never rewritten.
  it.each(["+0007", "0007"])("sends the accepted key '%s' as routed and shows its record", async (key) => {
    answer = (request) =>
      isDetailRead(request, encodeURIComponent(key))
        ? jsonResponse(200, NEWEST)
        : readStore(request);
    renderAt(routeOf(key));

    await waitFor(() => {
      expect(readStatus().textContent).toBe(NEWEST_LOADED);
    });
    expect(screen.getByRole("heading", { level: 2, name: "Authorization" })).not.toBeNull();
    expect(screen.getByText(NEWEST.pa_transaction_id)).not.toBeNull();
    expect(screen.getByRole("button", { name: "Mark fraud" })).not.toBeNull();
    expect(sent()).toEqual([`GET ${DETAIL_PATH_PREFIX}${encodeURIComponent(key)}`]);
    expect(toastError).not.toHaveBeenCalled();
  });
});

/** The COPAUS1C UPDATE-AUTH-DETAILS text of a committed mark (P-COPAUS1C-008). */
const MARKED_TEXT = "AUTH MARKED FRAUD...";

/** NEWEST as the PUT answers it once marked: the replaced detail, read with no PF8 neighbour. */
const MARKED_NEWEST: AuthDetailResponse = {
  ...NEWEST,
  pa_auth_fraud: "F",
  pa_fraud_rpt_date: "07/01/22",
  next_id: null,
};

/** V-COPAUS1C-011's text for IMS status `AO`: the detail replace failed, so the mark rolled back (E-44). */
const FRAUD_TAGGING_FAILED = " System error while FRAUD Tagging, ROLLBACK||AO";

describe("AuthorizationDetailPage (CPVD, COPAU01) PF5 fraud mark", () => {
  // F-COPAU01-005 and F-COPAU01-006: Next stays offered while a mark is sent,
  // so PF8 can open the next authorization before the PUT answers. The
  // answer belongs to the authorization it was sent for: the one Next opened
  // keeps its own record and its own Next, and the marked one is cached with
  // the neighbour it showed.
  it("keeps the authorization Next opened while the mark of the one left is in flight", async () => {
    const toastSuccess = vi.spyOn(toast, "success");
    const held = holdResponse();
    answer = (request) =>
      isFraudToggle(request, NEWEST.auth_id) ? held.reply : readStore(request);
    const { user, router } = renderAt(NEWEST_ROUTE);
    await waitFor(() => {
      expect(readStatus().textContent).toBe(NEWEST_LOADED);
    });

    await user.click(screen.getByRole("button", { name: "Mark fraud" }));
    await user.click(await screen.findByRole("button", { name: "Confirm" }));
    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });
    await user.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() => {
      expect(router.state.location.pathname).toBe(`/authorizations/${FOLLOWING.auth_id}`);
    });
    await waitFor(() => {
      expect(readStatus().textContent).toBe(FOLLOWING_LOADED);
    });

    held.send(jsonResponse(200, { detail: MARKED_NEWEST, message: MARKED_TEXT }));
    await waitFor(() => {
      expect(toastSuccess).toHaveBeenCalledWith(MARKED_TEXT);
    });
    await waitFor(() => {
      expect(queryClient.isMutating()).toBe(0);
    });

    // FOLLOWING's screen: its record, its unmarked PF5 and its own Next.
    expect(screen.getByText(FOLLOWING.pa_transaction_id)).not.toBeNull();
    expect(screen.queryByText(NEWEST.pa_transaction_id)).toBeNull();
    expect(screen.getByRole("button", { name: "Mark fraud" })).not.toBeNull();
    expect(screen.getByRole("button", { name: "Next" }).hasAttribute("disabled")).toBe(false);
    expect(queryClient.getQueryData(authorizationKeys.detail(FOLLOWING.auth_id))).toEqual(
      FOLLOWING,
    );
    expect(queryClient.getQueryData(authorizationKeys.detail(NEWEST.auth_id))).toEqual({
      ...MARKED_NEWEST,
      next_id: NEWEST.next_id,
    });
    expect(sent()).toEqual([
      `GET ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`,
      `PUT ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`,
      `GET ${SUMMARY_PATH}?account_id=${ACCOUNT_1}&start_key=${NEWEST.auth_id}&direction=forward&page_size=1`,
      `GET ${DETAIL_PATH_PREFIX}${FOLLOWING.auth_id}`,
    ]);
    expect(toastError).not.toHaveBeenCalled();
  });

  /**
   * NEWEST shown, its mark confirmed with the PUT held, and the confirm opened
   * again from the fraud button, which stays enabled while the mark is sent:
   * the mark's confirm, with both buttons disabled. `sendMark` answers the PUT.
   */
  async function reopenConfirmDuringMark(): Promise<{
    user: UserEvent;
    trigger: HTMLElement;
    reopened: HTMLElement;
    sendMark: (response: Response) => void;
  }> {
    const held = holdResponse();
    answer = (request) =>
      isFraudToggle(request, NEWEST.auth_id) ? held.reply : readStore(request);
    const { user } = renderAt(NEWEST_ROUTE);
    await waitFor(() => {
      expect(readStatus().textContent).toBe(NEWEST_LOADED);
    });

    await user.click(screen.getByRole("button", { name: "Mark fraud" }));
    await user.click(await screen.findByRole("button", { name: "Confirm" }));
    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });
    const trigger = screen.getByRole("button", { name: "Mark fraud" });
    expect(trigger.hasAttribute("disabled")).toBe(false);
    await user.click(trigger);
    const reopened = await screen.findByRole("alertdialog", {
      name: "Mark this authorization as fraud?",
    });
    expect(within(reopened).getByRole("button", { name: "Confirm" }).hasAttribute("disabled")).toBe(
      true,
    );
    expect(within(reopened).getByRole("button", { name: "Cancel" }).hasAttribute("disabled")).toBe(
      true,
    );
    return { user, trigger, reopened, sendMark: held.send };
  }

  // F-COPAU01-003 to F-COPAU01-005, E-60: a confirm offers the PF5 its record
  // names. The mark landing turns the record `F`, so the confirm opened
  // again meanwhile closes rather than offer Confirm, which would send the
  // toggle that removes the mark; focus returns to the fraud button, now
  // Remove fraud, whose own confirm offers the removal.
  it("closes a confirm opened again while the mark is sent once the mark lands", async () => {
    const toastSuccess = vi.spyOn(toast, "success");
    const { user, trigger, sendMark } = await reopenConfirmDuringMark();

    sendMark(jsonResponse(200, { detail: MARKED_NEWEST, message: MARKED_TEXT }));
    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });
    expect(screen.queryByRole("button", { name: "Confirm" })).toBeNull();
    expect(screen.getByRole("button", { name: "Remove fraud" })).toBe(trigger);
    await waitFor(() => {
      expect(document.activeElement).toBe(trigger);
    });
    await waitFor(() => {
      expect(toastSuccess).toHaveBeenCalledWith(MARKED_TEXT);
    });
    await waitFor(() => {
      expect(queryClient.isMutating()).toBe(0);
    });

    await user.click(trigger);
    const removal = await screen.findByRole("alertdialog", { name: "Remove the fraud mark?" });
    expect(
      within(removal).getByText(
        `The fraud mark of the authorization of transaction ${NEWEST.pa_transaction_id} is removed.`,
      ),
    ).not.toBeNull();
    expect(within(removal).getByRole("button", { name: "Confirm" }).hasAttribute("disabled")).toBe(
      false,
    );
    expect(sent()).toEqual([
      `GET ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`,
      `PUT ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`,
    ]);
    expect(toastError).not.toHaveBeenCalled();
  });

  // V-COPAUS1C-011: a failed mark leaves the record unmarked, so the confirm
  // opened again meanwhile still offers the mark, and its Confirm is enabled
  // once the PUT has answered.
  it("keeps a confirm opened again while the mark is sent once the mark fails", async () => {
    const { trigger, reopened, sendMark } = await reopenConfirmDuringMark();

    sendMark(
      jsonResponse(500, {
        code: "UNEXPECTED",
        message: FRAUD_TAGGING_FAILED,
        fields: [{ field: CARDNUM, message: FRAUD_TAGGING_FAILED }],
      }),
    );
    await waitFor(() => {
      expect(toastError).toHaveBeenCalledWith(FRAUD_TAGGING_FAILED);
    });
    const confirm = within(reopened).getByRole("button", { name: "Confirm" });
    await waitFor(() => {
      expect(confirm.hasAttribute("disabled")).toBe(false);
    });
    expect(screen.getByRole("alertdialog", { name: "Mark this authorization as fraud?" })).toBe(
      reopened,
    );
    expect(
      within(reopened).getByText(
        `The authorization of transaction ${NEWEST.pa_transaction_id} is reported as fraud.`,
      ),
    ).not.toBeNull();
    expect(trigger.textContent).toBe("Mark fraud");
    expect(sent()).toEqual([
      `GET ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`,
      `PUT ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`,
    ]);
    expect(toastError).toHaveBeenCalledTimes(1);
  });
});

/** PF8's summary read one row on from NEWEST once no authorization follows it. */
const NO_PAGE_AFTER_NEWEST: AuthSummaryResponse = {
  ...PAGE_AFTER_NEWEST,
  items: [],
  next_key: null,
  prev_key: null,
};

describe("AuthorizationDetailPage (CPVD, COPAU01) read again after a record was shown", () => {
  // E-59: PF8 at a record whose follower is gone finds no row and reads the
  // record again. When that read fails, the record purged meanwhile (404) or
  // its read failing (500), the record read earlier is withheld exactly as
  // on a failed first read, with no toast or message line for the 404
  // without a message. Returning to the route reads the key again: the
  // skeleton stands in while that read is in flight, the failure is neither
  // toasted again nor shown, and a successful answer brings the record back.
  it.each(READ_FAILURES)(
    "withholds the record when its read again answers a $answer, until a read succeeds",
    async ({ status, body, shown }) => {
      let failing = false;
      answer = (request) => {
        if (isNextLookup(request)) {
          return jsonResponse(200, NO_PAGE_AFTER_NEWEST);
        }
        if (failing && isDetailRead(request, NEWEST.auth_id)) {
          return jsonResponse(status, body);
        }
        return readStore(request);
      };
      const { user, router } = renderAt(NEWEST_ROUTE);
      await waitFor(() => {
        expect(readStatus().textContent).toBe(NEWEST_LOADED);
      });

      failing = true;
      await user.click(screen.getByRole("button", { name: "Next" }));
      await expectFailedRead(shown);
      expect(router.state.location.pathname).toBe(`/authorizations/${NEWEST.auth_id}`);

      const held = holdResponse();
      answer = (request) =>
        isDetailRead(request, NEWEST.auth_id) ? held.reply : readStore(request);
      await user.click(screen.getByRole("link", { name: "Back" }));
      expect(await screen.findByText(SUMMARY_MARKER)).not.toBeNull();
      await act(async () => {
        await router.navigate(-1);
      });

      await waitFor(() => {
        expect(readStatus().textContent).toBe(LOADING);
      });
      expect(document.querySelector("[data-slot='card']")?.getAttribute("aria-hidden")).toBe(
        "true",
      );
      expect(pageText()).toEqual([]);
      expect(screen.queryByText(NEWEST.pa_transaction_id)).toBeNull();
      expect(screen.queryByRole("button", { name: "Mark fraud" })).toBeNull();
      expect(screen.queryByRole("button", { name: "Next" })).toBeNull();

      held.send(jsonResponse(200, NEWEST));
      await waitFor(() => {
        expect(readStatus().textContent).toBe(NEWEST_LOADED);
      });
      expect(screen.getByText(NEWEST.pa_transaction_id)).not.toBeNull();
      expect(screen.getByRole("button", { name: "Mark fraud" })).not.toBeNull();
      expect(screen.getByRole("button", { name: "Next" })).not.toBeNull();
      expect(pageText()).not.toContain(shown);
      expect(toastError).toHaveBeenCalledTimes(shown === null ? 0 : 1);
      expect(sent()).toEqual([
        `GET ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`,
        `GET ${SUMMARY_PATH}?account_id=${ACCOUNT_1}&start_key=${NEWEST.auth_id}&direction=forward&page_size=1`,
        `GET ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`,
        `GET ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`,
      ]);
    },
  );

  /**
   * NEWEST shown, Next pressed with its lookup held, and PF5's confirm opened
   * meanwhile. `sendLookup` answers the lookup with the dialog open: no row
   * after NEWEST, whose read again then fails as a purge's 404, when
   * `purged`; FOLLOWING's row otherwise. The application reads only on
   * navigation or action, so PF8's answer is what can change the record
   * under the dialog.
   */
  async function openConfirmDuringNext(purged: boolean): Promise<{
    user: UserEvent;
    router: ReturnType<typeof createMemoryRouter>;
    sendLookup: () => void;
  }> {
    const lookup = holdResponse();
    let shownOnce = false;
    answer = (request) => {
      if (isNextLookup(request)) {
        return lookup.reply;
      }
      if (purged && shownOnce && isDetailRead(request, NEWEST.auth_id)) {
        return jsonResponse(READ_FAILURES[0].status, READ_FAILURES[0].body);
      }
      return readStore(request);
    };
    const { user, router } = renderAt(NEWEST_ROUTE);
    await waitFor(() => {
      expect(readStatus().textContent).toBe(NEWEST_LOADED);
    });
    shownOnce = true;
    await user.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() => {
      expect(sent()).toHaveLength(2);
    });
    await user.click(screen.getByRole("button", { name: "Mark fraud" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(
      within(dialog).getByText(
        `The authorization of transaction ${NEWEST.pa_transaction_id} is reported as fraud.`,
      ),
    ).not.toBeNull();
    const page = purged ? NO_PAGE_AFTER_NEWEST : PAGE_AFTER_NEWEST;
    return { user, router, sendLookup: () => lookup.send(jsonResponse(200, page)) };
  }

  // F-COPAU01-003, F-COPAU01-005: the confirm belongs to the record it was
  // opened on. PF8 finding no row reads the record again; that read failing
  // (a purge's 404, without a message, so with no toast) withholds the
  // record, and the confirm closes with it, so no Confirm is offered for it
  // and no PUT is sent.
  it("closes an open fraud confirm when a read again withholds its record", async () => {
    const { router, sendLookup } = await openConfirmDuringNext(true);

    act(() => {
      sendLookup();
    });
    await expectFailedRead(READ_FAILURES[0].shown);
    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });
    expect(screen.queryByRole("button", { name: "Confirm" })).toBeNull();
    expect(router.state.location.pathname).toBe(`/authorizations/${NEWEST.auth_id}`);
    expect(sent()).toEqual([
      `GET ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`,
      `GET ${SUMMARY_PATH}?account_id=${ACCOUNT_1}&start_key=${NEWEST.auth_id}&direction=forward&page_size=1`,
      `GET ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`,
    ]);
  });

  // The same confirm, PF8 finding the next row: the route moves on to
  // FOLLOWING, and the confirm opened for NEWEST closes rather than offer
  // Confirm for the record now shown; FOLLOWING's own PF5 opens its own.
  it("closes an open fraud confirm when Next opens another authorization", async () => {
    const { user, router, sendLookup } = await openConfirmDuringNext(false);

    act(() => {
      sendLookup();
    });
    await waitFor(() => {
      expect(router.state.location.pathname).toBe(`/authorizations/${FOLLOWING.auth_id}`);
    });
    await waitFor(() => {
      expect(readStatus().textContent).toBe(FOLLOWING_LOADED);
    });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.queryByRole("button", { name: "Confirm" })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Mark fraud" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(
      within(dialog).getByText(
        `The authorization of transaction ${FOLLOWING.pa_transaction_id} is reported as fraud.`,
      ),
    ).not.toBeNull();
    expect(sent()).toEqual([
      `GET ${DETAIL_PATH_PREFIX}${NEWEST.auth_id}`,
      `GET ${SUMMARY_PATH}?account_id=${ACCOUNT_1}&start_key=${NEWEST.auth_id}&direction=forward&page_size=1`,
      `GET ${DETAIL_PATH_PREFIX}${FOLLOWING.auth_id}`,
    ]);
    expect(toastError).not.toHaveBeenCalled();
  });
});
