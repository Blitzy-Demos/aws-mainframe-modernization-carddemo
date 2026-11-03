// FN tests of the CPVS page (pages/authorizations/AuthorizationSummaryPage.tsx,
// COPAUS0C with map COPAU00): ENTER, the "Search" submit of the "Account ID"
// textbox (F-COPAU00-001), requests the account's summary; an account the
// PROCESS-ENTER-KEY edits reject (V-COPAUS0C-001, V-COPAUS0C-002) is shown on
// the field and requested from nobody; a rejection from the server puts its
// message in a toast and on ACCTID, with the cursor there, as
// `MOVE -1 TO ACCTIDL OF COPAU0AI` does.
//
// The network is the global the API client looks up on every request, stubbed
// per test; every answer is typed with the OpenAPI schema, so drift in the
// response or error shapes fails `tsc`.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  vi,
  type MockInstance,
} from "vitest";

import type { components } from "@/api/schema";

import AuthorizationSummaryPage from "./AuthorizationSummaryPage";

type AuthSummaryResponse = components["schemas"]["AuthSummaryResponse"];
type AuthSummaryItem = components["schemas"]["AuthSummaryItem"];
type AuthSummaryField = components["schemas"]["AuthSummaryField"];
type ErrorBody = components["schemas"]["ErrorBody"];

/** Account 1 of the fixture [app/data/ASCII/acctdata.txt:L1], as `ACCTIDI PIC X(11)` holds it. */
const ACCOUNT_1 = "00000000001";

/** An account no cross-reference names [app/data/ASCII/cardxref.txt]. */
const UNKNOWN_ACCOUNT = "99999999999";

/** The one BMS field every COPAUS0C message puts the cursor on. */
const ACCTID: AuthSummaryField = "ACCTID";

const AUTHORIZATION: AuthSummaryItem = {
  auth_id: "1",
  pa_transaction_id: "220701120000001",
  pa_auth_orig_date: "220701",
  pa_auth_orig_time: "120000",
  pa_auth_type: "0100",
  approved: "A",
  pa_match_status: "P",
  pa_approved_amt: "50.47",
};

/**
 * The first page of account 1: its account record, customer 1 (its
 * cross-reference customer [app/data/ASCII/cardxref.txt:L49],
 * [app/data/ASCII/custdata.txt:L1]) and a summary with one approved
 * authorization. No page precedes or follows it, and no boundary text is set.
 */
const summaryOk: AuthSummaryResponse = {
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
  pa_approved_auth_cnt: 1,
  pa_declined_auth_cnt: 0,
  pa_approved_auth_amt: "50.47",
  pa_declined_auth_amt: "0.00",
  pa_credit_balance: "50.47",
  pa_cash_balance: "0.00",
  items: [AUTHORIZATION],
  next_key: null,
  prev_key: null,
  message: "",
};

function jsonResponse(status: number, body: AuthSummaryResponse | ErrorBody): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** Answers every request with `status` and `body`. */
function stubApi(status: number, body: AuthSummaryResponse | ErrorBody) {
  const fetchMock = vi.fn(
    async (_input: Request | string | URL, _init?: RequestInit) =>
      jsonResponse(status, body),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

type FetchMock = ReturnType<typeof stubApi>;

/** The request of the mock's first call. openapi-fetch hands over a Request. */
function firstRequest(fetchMock: FetchMock): Request {
  const [call] = fetchMock.mock.calls;
  if (call === undefined) {
    throw new Error("no request was sent");
  }
  const [input] = call;
  return input instanceof Request ? input : new Request(input);
}

/** The page on a data router, so PageHeader's useMatches() sees the route handle. */
function renderPage(initialEntry = "/authorizations") {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(
    [
      {
        path: "/authorizations",
        element: <AuthorizationSummaryPage />,
        handle: { title: "View Authorizations", crumb: "Authorizations" },
      },
      { path: "/authorizations/:authId", element: <div /> },
      { path: "/", element: <div /> },
    ],
    { initialEntries: [initialEntry] },
  );
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router };
}

/** The ACCTID input of F-COPAU00-001. */
function accountField(): HTMLElement {
  return screen.getByRole("textbox", { name: "Account ID" });
}

/** The ENTER control of F-COPAU00-001. */
function searchButton(): HTMLElement {
  return screen.getByRole("button", { name: "Search" });
}

let toastError: MockInstance<typeof toast.error>;

beforeEach(() => {
  toastError = vi.spyOn(toast, "error");
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("CPVS ENTER with an account ID that passes the edits", () => {
  it.each([
    { how: "Enter in the field", key: "{Enter}", click: false },
    { how: "the Search button", key: "", click: true },
  ])("requests the account's summary on $how", async ({ key, click }) => {
    const fetchMock = stubApi(200, summaryOk);
    const user = userEvent.setup();
    const { router } = renderPage();

    await user.type(accountField(), `${ACCOUNT_1}${key}`);
    if (click) {
      await user.click(searchButton());
    }

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    const request = firstRequest(fetchMock);
    const url = new URL(request.url);
    expect(request.method).toBe("GET");
    expect(url.origin).toBe(window.location.origin);
    expect(url.pathname).toBe("/api/authorizations");
    expect(url.searchParams.get("account_id")).toBe(ACCOUNT_1);

    // GATHER-ACCOUNT-DETAILS: the header of the account, CNAMEO as the COBOL
    // STRINGs it, and the list row of its authorization.
    expect(
      (await screen.findByRole("heading", { name: `Account ${ACCOUNT_1}` })).tagName,
    ).toBe("H2");
    expect(screen.getByText("Immanuel M Kessler").tagName).toBe("SPAN");
    expect(screen.getByText(AUTHORIZATION.pa_transaction_id).tagName).toBe("SPAN");

    // The account stays in the URL, so a reload and Back return to it.
    expect(new URLSearchParams(router.state.location.search).get("account_id")).toBe(
      ACCOUNT_1,
    );
    expect(accountField().getAttribute("aria-invalid")).toBe("false");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(toastError).not.toHaveBeenCalled();
  });
});

describe("CPVS ENTER with an account ID the edits reject", () => {
  it.each([
    {
      row: "V-COPAUS0C-001",
      label: "an empty field",
      typed: "",
      message: "Please enter Acct Id...",
    },
    {
      row: "V-COPAUS0C-002",
      label: "a non-numeric value",
      typed: "12AB5678901",
      message: "Acct Id must be Numeric ...",
    },
  ])("$row: shows $label on ACCTID and sends nothing", async ({ typed, message }) => {
    const fetchMock = stubApi(200, summaryOk);
    const user = userEvent.setup();
    renderPage();
    const field = accountField();

    await user.type(field, `${typed}{Enter}`);

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(message);
    expect(field.getAttribute("aria-invalid")).toBe("true");
    expect(field.getAttribute("aria-describedby")).toBe(alert.id);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(toastError).not.toHaveBeenCalled();
  });
});

// The server's rejections of an account ID that passes the client edits. Each
// text is the one the API composes for the audited row (crates/domain/src/
// messages.rs auth_lookup_message, with the E-44 RESP and RESP2 slots), cut
// to `WS-MESSAGE PIC X(80)` as STRING leaves it. The statuses are the audit's:
// the unknown account is 404 and a failed read 500. No 400 reaches the page
// for such an account, since its only 400s (V-COPAUS0C-001, V-COPAUS0C-002)
// are the edits the form applies before sending.
describe("CPVS ENTER answered with a server error", () => {
  it.each([
    {
      row: "V-COPAUS0C-004",
      status: 404,
      code: "NOT_FOUND",
      account: UNKNOWN_ACCOUNT,
      message: "Account:99999999999 not found in XREF file. Resp:000000013 Reas:000000080",
    },
    {
      row: "V-COPAUS0C-005",
      status: 500,
      code: "UNEXPECTED",
      account: ACCOUNT_1,
      message: "Account:00000000001 System error while reading XREF file. Resp:000000017 Reas:00",
    },
  ])(
    "$row ($status): toasts the message, marks ACCTID and returns the cursor to it",
    async ({ status, code, account, message }) => {
      const body: ErrorBody = { code, message, fields: [{ field: ACCTID, message }] };
      const fetchMock = stubApi(status, body);
      const user = userEvent.setup();
      renderPage();
      const field = accountField();

      // Submitted with the button, so the focus assertion below proves the
      // cursor was moved back to the field rather than left there.
      await user.type(field, account);
      await user.click(searchButton());

      await waitFor(() => expect(toastError).toHaveBeenCalledWith(message));
      expect(toastError).toHaveBeenCalledTimes(1);
      expect(fetchMock).toHaveBeenCalledTimes(1);
      expect(new URL(firstRequest(fetchMock).url).searchParams.get("account_id")).toBe(
        account,
      );

      const alert = await screen.findByRole("alert");
      expect(alert.textContent).toBe(message);
      expect(field.getAttribute("aria-invalid")).toBe("true");
      await waitFor(() => expect(document.activeElement).toBe(field));

      // A failed first read shows no header and no list.
      expect(screen.queryByRole("heading", { name: `Account ${account}` })).toBeNull();
      expect(screen.queryByRole("table")).toBeNull();
    },
  );
});
