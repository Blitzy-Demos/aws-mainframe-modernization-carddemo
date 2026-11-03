// FN tests of DashboardPage (CM00: COMEN01C, map COMEN01). The page's one
// input is the CAVW account lookup, ACCTSID: a valid account number reads the
// account, then its five most recent transactions and its reward balance, and
// shows them beside the COMEN02Y menu (F-COACTVW-001, F-COACTVW-002,
// F-COMEN01-001 to F-COMEN01-011); a number COACTVWC 2210-EDIT-ACCOUNT refuses
// is rejected without a request (V-COACTVWC-001, V-COACTVWC-002); and a server
// rejection at ACCTSID is shown as a toast, as the field error and with the
// cursor on ACCTSID [app/cbl/COACTVWC.cbl:L545-552].
//
// The API is the stubbed global the client sends every Request through, so
// each test sees the exact requests the page made.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import { afterEach, describe, expect, it, vi, type Mock } from "vitest";

import type { components } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { userMenu } from "@/nav/menu";
import { ACCOUNT_KEY_MESSAGES } from "@/schemas/accounts";

import DashboardPage from "./DashboardPage";

type AccountResponse = components["schemas"]["AccountResponse"];
type TransactionResponse = components["schemas"]["TransactionResponse"];
type TransactionPage = components["schemas"]["Page_TransactionResponse"];
type RewardBalanceResponse = components["schemas"]["RewardBalanceResponse"];
type ErrorBody = components["schemas"]["ErrorBody"];

const ACCOUNT_ID = "00000000001";

/**
 * `GET /accounts/00000000001` on the seeded corpus: the account record
 * [app/data/ASCII/acctdata.txt:L1], customer 000000001
 * [app/data/ASCII/custdata.txt:L1] and the account's card from its
 * cross-reference [app/data/ASCII/cardxref.txt:L49].
 */
const account: AccountResponse = {
  account_id: ACCOUNT_ID,
  acct_active_status: "Y",
  acct_cash_credit_limit: "1020.00",
  acct_credit_limit: "2020.00",
  acct_curr_bal: "194.00",
  acct_curr_cyc_credit: "0.00",
  acct_curr_cyc_debit: "0.00",
  acct_expiraion_date: "2025-05-20",
  acct_group_id: "",
  acct_open_date: "2014-11-20",
  acct_reissue_date: "2025-05-20",
  card_number: "9680294154603697",
  cust_addr_country_cd: "USA",
  cust_addr_line_1: "618 Deshaun Route",
  cust_addr_line_2: "Apt. 802",
  cust_addr_line_3: "Altenwerthshire",
  cust_addr_state_cd: "NC",
  cust_addr_zip: "12546",
  cust_dob_yyyy_mm_dd: "1961-06-08",
  cust_eft_account_id: "0053581756",
  cust_fico_credit_score: "274",
  cust_first_name: "Immanuel",
  cust_govt_issued_id: "00000000000049368437",
  cust_last_name: "Kessler",
  cust_middle_name: "Madeline",
  cust_phone_num_1: "(908)119-8310",
  cust_phone_num_2: "(373)693-8684",
  cust_pri_card_holder_ind: "Y",
  cust_ssn: "020973888",
  customer_id: "000000001",
  version: "3f2a9c1e5b7d0846",
};

/**
 * A posted purchase on the account's card: daily transaction 156
 * [app/data/ASCII/dailytran.txt:L156], stamped by POSTTRAN at the pinned
 * processing clock.
 */
const transaction: TransactionResponse = {
  tran_id: "0000000503557384",
  tran_type_cd: "01",
  tran_cat_cd: "0001",
  tran_source: "POS TERM",
  tran_desc: "Purchase at Casper Group",
  tran_amt: "81.44",
  tran_merchant_id: "800000000",
  tran_merchant_name: "Casper Group",
  tran_merchant_city: "Millsborough",
  tran_merchant_zip: "57690",
  tran_card_num: "9680294154603697",
  tran_orig_ts: "2022-06-10 19:27:53.000000",
  tran_proc_ts: "2022-07-01 12:00:00.000000",
};

/** The five most recent transactions: here the one above, with no paging. */
const transactionPage: TransactionPage = {
  items: [transaction],
  next_key: null,
  prev_key: null,
};

/**
 * A points total above 2^53 - 1, which a JavaScript number cannot hold
 * exactly: the page must show the API's string digit for digit.
 */
const rewardBalance: RewardBalanceResponse = {
  account_id: ACCOUNT_ID,
  total_points_earned: "9007199254740993",
  total_points_redeemed: "0",
  available_points: "9007199254740993",
};

/** An HTTP status and the JSON body sent with it. */
type Reply<B> = readonly [status: number, body: B];

/** Every body the stubbed routes answer with. */
type StubBody =
  | AccountResponse
  | TransactionPage
  | RewardBalanceResponse
  | ErrorBody;

/** The answer of each API route the dashboard reads. */
type ApiStubs = {
  account: Reply<AccountResponse | ErrorBody>;
  transactions: Reply<TransactionPage | ErrorBody>;
  rewards: Reply<RewardBalanceResponse | ErrorBody>;
};

/** Every route answering with its fixture. */
const SUCCESS: ApiStubs = {
  account: [200, account],
  transactions: [200, transactionPage],
  rewards: [200, rewardBalance],
};

/** The answer to a path no stub serves, which no test expects. */
const NO_ROUTE: Reply<ErrorBody> = [
  404,
  { code: "NOT_FOUND", message: "Not found" },
];

/** The global the API client sends each Request through. */
type FetchFn = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

/** The requests the page sent, in order. */
const requests: Request[] = [];

let fetchMock: Mock<FetchFn>;

function jsonResponse<B>([status, body]: Reply<B>): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** The stub answering `pathname`, the client's `/api`-prefixed path. */
function replyFor(pathname: string, stubs: ApiStubs): Reply<StubBody> {
  if (/^\/api\/accounts\/[^/]+$/.test(pathname)) {
    return stubs.account;
  }
  if (pathname === "/api/transactions") {
    return stubs.transactions;
  }
  if (/^\/api\/rewards\/balance\/[^/]+$/.test(pathname)) {
    return stubs.rewards;
  }
  return NO_ROUTE;
}

/** Installs the API stub: each request is logged, then answered by route. */
function stubApi(stubs: ApiStubs): void {
  fetchMock = vi.fn<FetchFn>(async (input, init) => {
    const request = input instanceof Request ? input : new Request(input, init);
    requests.push(request);
    return jsonResponse(replyFor(new URL(request.url).pathname, stubs));
  });
  vi.stubGlobal("fetch", fetchMock);
}

/** The request sent to `pathname`; throws while there is none. */
function requestTo(pathname: string): Request {
  const sent = requests.find(
    (request) => new URL(request.url).pathname === pathname,
  );
  if (sent === undefined) {
    throw new Error(`No request was sent to ${pathname}`);
  }
  return sent;
}

function renderPage(): void {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  // The dashboard's route as App.tsx declares it; PageHeader's useMatches
  // needs a data router. The catch-all gives every menu route a match.
  const router = createMemoryRouter(
    [
      {
        path: "/",
        element: <DashboardPage />,
        handle: { crumb: "Home" } satisfies RouteHandle,
      },
      { path: "*", element: null },
    ],
    { initialEntries: ["/"] },
  );
  render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
        <Toaster />
      </QueryClientProvider>
    </ThemeProvider>,
  );
}

/** ACCTSID, by the label F-COACTVW-001 gives it. */
function accountInput(): HTMLElement {
  return screen.getByLabelText("Account number");
}

/** The summary card titled `title`, once it is shown. */
async function summaryCard(title: string): Promise<HTMLElement> {
  return screen.findByRole("group", { name: title });
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  requests.length = 0;
});

describe("DashboardPage", () => {
  it("summarises the account a valid number names, beside the main menu", async () => {
    stubApi(SUCCESS);
    const user = userEvent.setup();
    renderPage();

    // ENTER on ACCTSID (F-COACTVW-001).
    await user.type(accountInput(), `${ACCOUNT_ID}{Enter}`);

    // The account first, then, once it is found, its five most recent
    // transactions and its reward balance; nothing else.
    await waitFor(() => {
      expect(requestTo(`/api/accounts/${ACCOUNT_ID}`).method).toBe("GET");
      const transactions = requestTo("/api/transactions");
      expect(transactions.method).toBe("GET");
      const query = new URL(transactions.url).searchParams;
      expect(Object.fromEntries(query)).toEqual({
        account_id: ACCOUNT_ID,
        direction: "backward",
        page_size: "5",
      });
      expect(requestTo(`/api/rewards/balance/${ACCOUNT_ID}`).method).toBe(
        "GET",
      );
    });
    expect(requests).toHaveLength(3);

    // Each figure under its card title, verbatim as the API sent it.
    const figures: Array<[title: string, value: string]> = [
      ["Current Balance", account.acct_curr_bal],
      ["Credit Limit", account.acct_credit_limit],
      ["Cash Credit Limit", account.acct_cash_credit_limit],
      ["Reward Points", rewardBalance.available_points],
    ];
    for (const [title, value] of figures) {
      const card = await summaryCard(title);
      expect(within(card).getByText(value).textContent).toBe(value);
    }

    // The full view (F-COACTVW-002) and the update screen (F-COACTUP-001).
    expect(
      screen.getByRole("link", { name: "View account" }).getAttribute("href"),
    ).toBe(`/accounts/${ACCOUNT_ID}`);
    expect(
      screen.getByRole("link", { name: "Edit account" }).getAttribute("href"),
    ).toBe(`/accounts/${ACCOUNT_ID}/edit`);

    // The transaction row links to its CT01 view.
    const tranLink = await screen.findByRole("link", {
      name: transaction.tran_id,
    });
    expect(tranLink.getAttribute("href")).toBe(
      `/transactions/${transaction.tran_id}`,
    );
    expect(screen.getByText(transaction.tran_desc).textContent).toBe(
      transaction.tran_desc,
    );
    expect(screen.getByText(transaction.tran_amt).textContent).toBe(
      transaction.tran_amt,
    );

    // Every COMEN02Y option, then Rewards, in menu order, each linking to its
    // route (F-COMEN01-001 to F-COMEN01-011). The landmark is named by the
    // map title 'Main Menu' [app/bms/COMEN01.bms:L79].
    const menuNav = screen.getByRole("navigation", { name: "Main Menu" });
    const menuLinks = within(menuNav).getAllByRole("link");
    expect(menuLinks).toHaveLength(userMenu.length);
    userMenu.forEach((entry, index) => {
      const link = within(menuNav).getByRole("link", { name: entry.label });
      expect(link).toBe(menuLinks[index]);
      expect(link.getAttribute("href")).toBe(entry.route);
    });

    expect(screen.queryByRole("alert")).toBeNull();
  });

  it.each([
    { entered: "", message: ACCOUNT_KEY_MESSAGES.noInputReceived },
    {
      entered: "abc",
      message: ACCOUNT_KEY_MESSAGES.accountFilterMustBeNonZero11Digit,
    },
  ])(
    "rejects the account number $entered without a request",
    async ({ entered, message }) => {
      stubApi(SUCCESS);
      const user = userEvent.setup();
      renderPage();

      await user.type(accountInput(), `${entered}{Enter}`);
      const fieldError = await screen.findByRole("alert");

      expect(fieldError.textContent).toBe(message);
      expect(accountInput().getAttribute("aria-invalid")).toBe("true");
      expect(fetchMock).not.toHaveBeenCalled();
      expect(
        screen.queryByRole("group", { name: "Current Balance" }),
      ).toBeNull();
    },
  );

  it("shows the server's rejection at ACCTSID as a toast, the field error and the cursor", async () => {
    // V-COACTVWC-002 as the server answers it, with its two spaces.
    const message = ACCOUNT_KEY_MESSAGES.accountFilterMustBeNonZero11Digit;
    const rejection: ErrorBody = {
      code: "INVALID",
      message,
      fields: [{ field: "ACCTSID", message }],
    };
    stubApi({ ...SUCCESS, account: [400, rejection] });
    const toastError = vi.spyOn(toast, "error");
    const user = userEvent.setup();
    renderPage();

    // A number the form accepts, submitted with the button so the cursor
    // starts away from ACCTSID.
    await user.type(accountInput(), "99999999999");
    await user.click(screen.getByRole("button", { name: "Look up account" }));
    const fieldError = await screen.findByRole("alert");

    expect(fieldError.textContent).toBe(message);
    expect(accountInput().getAttribute("aria-invalid")).toBe("true");
    expect(toastError).toHaveBeenCalledTimes(1);
    expect(toastError).toHaveBeenCalledWith(message);
    await waitFor(() => {
      expect(document.activeElement).toBe(accountInput());
    });

    // COACTVWC skips its other reads after a key error, so only the account
    // was requested and no summary is shown.
    expect(requests).toHaveLength(1);
    expect(requestTo("/api/accounts/99999999999").method).toBe("GET");
    expect(
      screen.queryByRole("group", { name: "Current Balance" }),
    ).toBeNull();
  });
});
