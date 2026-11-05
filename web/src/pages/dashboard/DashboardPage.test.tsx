// FN tests of DashboardPage (CM00: COMEN01C, map COMEN01). The page's one
// input is the CAVW account lookup, ACCTSID: a valid account number reads the
// account, then its five most recent transactions (shown newest first) and its
// reward balance, and shows them beside the COMEN02Y menu (F-COACTVW-001,
// F-COACTVW-002, F-COMEN01-001 to F-COMEN01-011); a number COACTVWC
// 2210-EDIT-ACCOUNT refuses is rejected without a request (V-COACTVWC-001,
// V-COACTVWC-002); and a server rejection at ACCTSID is shown as a toast, as
// the field error and with the cursor on ACCTSID
// [app/cbl/COACTVWC.cbl:L545-552]. The page's status region announces each
// lookup, a repeated one included, by its account number and never by a
// figure, once the account and its reward balance have answered, and is empty
// for a rejected key.
//
// The API is the stubbed global the client sends every Request through, so
// each test sees the exact requests the page made.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import { afterEach, describe, expect, it, vi, type Mock } from "vitest";

import { transactionKeys } from "@/api/queries/transactions";
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

// The status region's texts while a lookup reads and once it has answered.
const ACCOUNT_LOADING = "Loading account";
const SUMMARY_LOADED = `Account ${ACCOUNT_ID} summary loaded`;

// V-COACTVWC-003: no cross-reference for the account, 404 at ACCTSID.
const XREF_NOT_FOUND =
  "Account:99999999999 not found in Cross ref file.  Resp:000000013  Reas:0000";

// `GET /accounts/00000000001` on the seeded corpus: the account record
// [app/data/ASCII/acctdata.txt:L1], customer 000000001
// [app/data/ASCII/custdata.txt:L1] and the account's card from its
// cross-reference [app/data/ASCII/cardxref.txt:L49].
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

// A posted purchase on the account's card: daily transaction 156
// [app/data/ASCII/dailytran.txt:L156], stamped by POSTTRAN at the pinned
// processing clock.
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

// The five most recent transactions: here the one above, with no paging.
const transactionPage: TransactionPage = {
  items: [transaction],
  next_key: null,
  prev_key: null,
};

// A full backward page as the API answers it: the account's five highest
// TRAN-IDs in ascending key order, the last of them a CB00 payment whose
// TRAN-ID is the highest on file plus one.
const ascendingTranIds = [
  "0000000503557384",
  "0000000532120892",
  "0000000555363230",
  "0000000624335286",
  "2022071800000053",
] as const;
const fullPage: TransactionPage = {
  items: ascendingTranIds.map((tranId) => ({
    ...transaction,
    tran_id: tranId,
  })),
  next_key: null,
  prev_key: null,
};

// A points total above 2^53 - 1, which a JavaScript number cannot hold
// exactly: the page must show the API's string digit for digit.
const rewardBalance: RewardBalanceResponse = {
  account_id: ACCOUNT_ID,
  total_points_earned: "9007199254740993",
  total_points_redeemed: "0",
  available_points: "9007199254740993",
};

// An HTTP status and the JSON body sent with it.
type Reply<B> = readonly [status: number, body: B];

// Every body the stubbed routes answer with.
type StubBody =
  | AccountResponse
  | TransactionPage
  | RewardBalanceResponse
  | ErrorBody;

// The answer of each API route the dashboard reads.
type ApiStubs = {
  account: Reply<AccountResponse | ErrorBody>;
  transactions: Reply<TransactionPage | ErrorBody>;
  rewards: Reply<RewardBalanceResponse | ErrorBody>;
};

// Every route answering with its fixture.
const SUCCESS: ApiStubs = {
  account: [200, account],
  transactions: [200, transactionPage],
  rewards: [200, rewardBalance],
};

// The answer to a path no stub serves, which no test expects.
const NO_ROUTE: Reply<ErrorBody> = [
  404,
  { code: "NOT_FOUND", message: "Not found" },
];

// The global the API client sends each Request through.
type FetchFn = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

// The requests the page sent, in order.
const requests: Request[] = [];

// Answers a test holds back, by pathname; each answers one request.
const heldReplies = new Map<string, Promise<Reply<StubBody>>>();

let fetchMock: Mock<FetchFn>;

function jsonResponse<B>([status, body]: Reply<B>): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

// The stub answering `pathname`, the client's `/api`-prefixed path.
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

// Installs the API stub: each request is logged, then answered by a held
// answer for its path, when a test holds one, or else by route.
function stubApi(stubs: ApiStubs): void {
  fetchMock = vi.fn<FetchFn>(async (input, init) => {
    const request = input instanceof Request ? input : new Request(input, init);
    requests.push(request);
    const { pathname } = new URL(request.url);
    const held = heldReplies.get(pathname);
    if (held !== undefined) {
      heldReplies.delete(pathname);
      return jsonResponse(await held);
    }
    return jsonResponse(replyFor(pathname, stubs));
  });
  vi.stubGlobal("fetch", fetchMock);
}

// Holds the next answer to `pathname` back; the returned function sends
// `reply` as that answer.
function holdReply(pathname: string): (reply: Reply<StubBody>) => void {
  let release: (reply: Reply<StubBody>) => void = () => undefined;
  heldReplies.set(
    pathname,
    new Promise<Reply<StubBody>>((settle) => {
      release = settle;
    }),
  );
  return release;
}

// The request sent to `pathname`; throws while there is none.
function requestTo(pathname: string): Request {
  const sent = requests.find(
    (request) => new URL(request.url).pathname === pathname,
  );
  if (sent === undefined) {
    throw new Error(`No request was sent to ${pathname}`);
  }
  return sent;
}

// Renders the page at `/`; returns the query cache it reads through.
function renderPage(): QueryClient {
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
  return queryClient;
}

// ACCTSID, by the label F-COACTVW-001 gives it.
function accountInput(): HTMLElement {
  return screen.getByLabelText("Account number");
}

// The summary card titled `title`, once it is shown.
async function summaryCard(title: string): Promise<HTMLElement> {
  return screen.findByRole("group", { name: title });
}

// The page's lookup status line. The transactions table has a status region
// of its own, a div, so the page's paragraph is told apart by its element.
function lookupStatus(): HTMLElement {
  const regions = screen
    .getAllByRole("status")
    .filter((region) => region.tagName === "P");
  expect(regions).toHaveLength(1);
  return regions[0];
}

// The observers recordTexts started, disconnected after each test.
const observers: MutationObserver[] = [];

// Every text `region` takes from now on, in order, so the cleared state
// between two announcements is seen although it lasts only the announcement
// delay.
function recordTexts(region: HTMLElement): string[] {
  const texts: string[] = [];
  const observer = new MutationObserver(() => {
    texts.push(region.textContent ?? "");
  });
  observer.observe(region, {
    childList: true,
    characterData: true,
    subtree: true,
  });
  observers.push(observer);
  return texts;
}

afterEach(() => {
  observers.splice(0).forEach((observer) => {
    observer.disconnect();
  });
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  requests.length = 0;
  heldReplies.clear();
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

  it("lists the five most recent transactions newest first, leaving the page as answered", async () => {
    stubApi({ ...SUCCESS, transactions: [200, fullPage] });
    const user = userEvent.setup();
    const queryClient = renderPage();

    await user.type(accountInput(), `${ACCOUNT_ID}{Enter}`);
    const table = await screen.findByRole("table", {
      name: "Recent transactions",
    });

    // Each row's TRAN-ID link, top to bottom: the highest key first.
    await waitFor(() => {
      expect(
        within(table)
          .getAllByRole("link")
          .map((link) => link.textContent),
      ).toEqual([...ascendingTranIds].reverse());
    });

    // The cached page keeps the API's ascending order: the rows are a
    // reversed copy, never the cached array reversed in place.
    const cached = queryClient.getQueryData<TransactionPage>(
      transactionKeys.list({
        account_id: ACCOUNT_ID,
        direction: "backward",
        page_size: 5,
      }),
    );
    expect(cached?.items.map((item) => item.tran_id)).toEqual([
      ...ascendingTranIds,
    ]);
  });

  // A TRAN-DESC X(100) at its full length without a space breaks anywhere and
  // is clamped to one line, so it cannot widen the table past its card; the
  // title and the text keep it whole.
  it("clamps a 100-character description to one line with the full text as its title", async () => {
    const longDescription = "ABCDEFGHIJ".repeat(10);
    const longPage: TransactionPage = {
      ...transactionPage,
      items: [{ ...transaction, tran_desc: longDescription }],
    };
    stubApi({ ...SUCCESS, transactions: [200, longPage] });
    const toastError = vi.spyOn(toast, "error");
    const user = userEvent.setup();
    renderPage();

    await user.type(accountInput(), `${ACCOUNT_ID}{Enter}`);
    const table = await screen.findByRole("table", {
      name: "Recent transactions",
    });
    const description = await within(table).findByText(longDescription);
    const [headerRow, firstRow] = within(table).getAllByRole("row");
    expect(within(headerRow).getAllByRole("columnheader")[2].textContent).toBe(
      "Description",
    );
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

  // The money column aligns to the end, as on the Transaction List page: its
  // header, each skeleton row's placeholder and each amount.
  it("aligns the Amount header, placeholders and amounts to the end", async () => {
    stubApi(SUCCESS);
    const releaseTransactions = holdReply("/api/transactions");
    const user = userEvent.setup();
    renderPage();

    await user.type(accountInput(), `${ACCOUNT_ID}{Enter}`);
    // The account found, its transactions are read under the table's header.
    const table = await screen.findByRole("table", {
      name: "Recent transactions",
    });
    await waitFor(() => {
      expect(table.getAttribute("aria-busy")).toBe("true");
    });
    const header = within(table).getAllByRole("columnheader")[3];
    const headerText = within(header).getByText("Amount");
    expect(headerText.tagName).toBe("SPAN");
    expect([...headerText.classList]).toEqual(
      expect.arrayContaining(["block", "text-end"]),
    );
    const skeletonRows = table.querySelectorAll("tbody tr");
    expect(skeletonRows).toHaveLength(5);
    skeletonRows.forEach((row) => {
      const placeholder = row.querySelectorAll("td")[3].querySelector("span");
      expect(placeholder?.textContent).toBe("-999.99");
      expect([...(placeholder?.classList ?? [])]).toEqual(
        expect.arrayContaining(["block", "text-end", "tabular-nums"]),
      );
    });

    releaseTransactions([200, fullPage]);
    await waitFor(() => {
      expect(table.hasAttribute("aria-busy")).toBe(false);
    });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(fullPage.items.length);
    rows.forEach((row, index) => {
      const amount = within(row).getAllByRole("cell")[3].firstElementChild;
      expect(amount?.tagName).toBe("SPAN");
      expect(amount?.textContent).toBe(
        fullPage.items[fullPage.items.length - 1 - index].tran_amt,
      );
      expect([...(amount?.classList ?? [])]).toEqual(
        expect.arrayContaining(["block", "text-end", "tabular-nums"]),
      );
    });
  });

  it("holds the summary's place with its loading shells while the account and then its reward balance are read", async () => {
    stubApi(SUCCESS);
    const releaseAccount = holdReply(`/api/accounts/${ACCOUNT_ID}`);
    const releaseRewards = holdReply(`/api/rewards/balance/${ACCOUNT_ID}`);
    const user = userEvent.setup();
    renderPage();

    await user.type(accountInput(), `${ACCOUNT_ID}{Enter}`);

    // While the account is read, all hidden from assistive technology: the
    // heading row, four card skeletons (title and value lines) and the
    // transactions table in its loading state, under its caption.
    const table = await screen.findByRole("table", {
      name: "Recent transactions",
      hidden: true,
    });
    const overview = table.closest('[aria-hidden="true"]');
    if (overview === null) {
      throw new Error("the loading summary is not hidden");
    }
    const tiles = overview.querySelectorAll(
      '[data-slot="card"][aria-hidden="true"]',
    );
    expect(tiles).toHaveLength(4);
    tiles.forEach((tile) => {
      expect(tile.querySelectorAll('[data-slot="skeleton"]')).toHaveLength(2);
    });
    expect(table.getAttribute("aria-busy")).toBe("true");
    expect(table.querySelectorAll("tbody tr").length).toBeGreaterThan(0);
    expect(
      screen.queryByRole("group", { name: "Current Balance" }),
    ).toBeNull();

    // Once the account is found, the reward card's skeleton takes its place
    // in the grid until the reward balance answers.
    releaseAccount(SUCCESS.account);
    const balance = await summaryCard("Current Balance");
    const grid = balance.parentElement;
    if (grid === null) {
      throw new Error("the summary card is in no grid");
    }
    expect(grid.children).toHaveLength(4);
    const rewardTile = grid.children[3];
    expect(rewardTile.getAttribute("aria-hidden")).toBe("true");
    expect(rewardTile.querySelectorAll('[data-slot="skeleton"]')).toHaveLength(
      2,
    );

    releaseRewards(SUCCESS.rewards);
    const reward = await summaryCard("Reward Points");
    expect(reward.parentElement).toBe(grid);
    expect(grid.children).toHaveLength(4);
    expect(grid.querySelector('[data-slot="skeleton"]')).toBeNull();
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

  it("announces each lookup by its account once its reward balance has answered, a repeated one included", async () => {
    stubApi(SUCCESS);
    const user = userEvent.setup();
    renderPage();

    // Present and empty before any lookup, so assistive technology tracks the
    // region before it first changes.
    const region = lookupStatus();
    expect(region.textContent).toBe("");

    // With the reward balance held, the account's figures are shown and the
    // lookup is still loading.
    const releaseRewards = holdReply(`/api/rewards/balance/${ACCOUNT_ID}`);
    await user.type(accountInput(), `${ACCOUNT_ID}{Enter}`);
    await summaryCard("Current Balance");
    await waitFor(() => {
      expect(region.textContent).toBe(ACCOUNT_LOADING);
    });
    releaseRewards(SUCCESS.rewards);
    await waitFor(() => {
      expect(region.textContent).toBe(SUMMARY_LOADED);
    });
    // The same element throughout, naming the account and never a figure.
    expect(lookupStatus()).toBe(region);
    for (const figure of [
      account.acct_curr_bal,
      account.acct_credit_limit,
      account.acct_cash_credit_limit,
      rewardBalance.available_points,
    ]) {
      expect(region.textContent).not.toContain(figure);
    }

    // The same account again, its reward balance held once more.
    const releaseRepeat = holdReply(`/api/rewards/balance/${ACCOUNT_ID}`);
    const texts = recordTexts(region);
    await user.click(screen.getByRole("button", { name: "Look up account" }));
    await waitFor(() => {
      expect(region.textContent).toBe(ACCOUNT_LOADING);
    });
    releaseRepeat(SUCCESS.rewards);
    await waitFor(() => {
      expect(region.textContent).toBe(SUMMARY_LOADED);
    });
    // Cleared before each text, so the unchanged result is announced again.
    expect(texts).toEqual(["", ACCOUNT_LOADING, "", SUMMARY_LOADED]);
    expect(requests).toHaveLength(6);
  });

  it.each<{
    name: string;
    entered: string;
    rejection: Reply<ErrorBody> | null;
    message: string;
    texts: string[];
  }>([
    {
      name: "a rejection by the server, V-COACTVWC-003",
      entered: "99999999999",
      rejection: [
        404,
        {
          code: "NOT_FOUND",
          message: XREF_NOT_FOUND,
          fields: [{ field: "ACCTSID", message: XREF_NOT_FOUND }],
        },
      ],
      message: XREF_NOT_FOUND,
      texts: ["", ACCOUNT_LOADING, ""],
    },
    {
      name: "a rejection by the schema, V-COACTVWC-002",
      entered: "abc",
      rejection: null,
      message: ACCOUNT_KEY_MESSAGES.accountFilterMustBeNonZero11Digit,
      texts: [""],
    },
  ])(
    "empties the status region for $name of a lookup after a found account",
    async ({ entered, rejection, message, texts: expectedTexts }) => {
      stubApi(SUCCESS);
      const toastError = vi.spyOn(toast, "error");
      const user = userEvent.setup();
      renderPage();
      await user.type(accountInput(), `${ACCOUNT_ID}{Enter}`);
      const region = lookupStatus();
      await waitFor(() => {
        expect(region.textContent).toBe(SUMMARY_LOADED);
      });

      const texts = recordTexts(region);
      await user.clear(accountInput());
      if (rejection === null) {
        await user.type(accountInput(), `${entered}{Enter}`);
      } else {
        // The rejection is held so that the lookup's progress is seen.
        const release = holdReply(`/api/accounts/${entered}`);
        await user.type(accountInput(), `${entered}{Enter}`);
        await waitFor(() => {
          expect(region.textContent).toBe(ACCOUNT_LOADING);
        });
        release(rejection);
      }

      // The field error and, for the server's answer, the toast report the
      // rejection; the status region holds nothing for it.
      const fieldError = await screen.findByRole("alert");
      expect(fieldError.textContent).toBe(message);
      await waitFor(() => {
        expect(region.textContent).toBe("");
      });
      expect(texts).toEqual(expectedTexts);
      expect(toastError.mock.calls).toEqual(
        rejection === null ? [] : [[message]],
      );
      expect(
        screen.queryByRole("group", { name: "Current Balance" }),
      ).toBeNull();
    },
  );
});
