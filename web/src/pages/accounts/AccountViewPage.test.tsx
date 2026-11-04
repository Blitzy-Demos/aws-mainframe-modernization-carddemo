// FN tests of AccountViewPage (CAVW: COACTVWC, map COACTVW). The page's one
// input, ACCTSID, accepts a valid account number and reads its records
// (F-COACTVW-001, F-COACTVW-002); it rejects the numbers COACTVWC
// 2210-EDIT-ACCOUNT refuses without sending a request (V-COACTVWC-001,
// V-COACTVWC-002); and a server rejection is shown as a toast, as the ACCTSID
// field error and with the cursor on ACCTSID, as 1300-SETUP-SCREEN-ATTRS
// places it [app/cbl/COACTVWC.cbl:L545-552]. The page's status region
// announces each read, a repeated lookup included, by its account number and
// never by a detail value, and is empty after a failed read.
//
// The API is the stubbed global the client calls for every request, so each
// test sees the exact Request the page sent.
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
  type Mock,
} from "vitest";

import type { components } from "@/api/schema";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { ACCOUNT_KEY_MESSAGES } from "@/schemas/accounts";

import AccountViewPage from "./AccountViewPage";

type AccountResponse = components["schemas"]["AccountResponse"];
type ErrorBody = components["schemas"]["ErrorBody"];

/**
 * `GET /accounts/00000000001` on the seeded corpus: account record
 * [app/data/ASCII/acctdata.txt:L1], customer 000000001
 * [app/data/ASCII/custdata.txt:L1] and the account's card from its
 * cross-reference [app/data/ASCII/cardxref.txt:L49], as the API answers them
 * (crates/api/tests/snapshots/cavw_account_view__account_00000000001.snap).
 */
const account: AccountResponse = {
  account_id: "00000000001",
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

/** V-COACTVWC-003: no cross-reference for the account, 404 at ACCTSID. */
const XREF_NOT_FOUND =
  "Account:99999999999 not found in Cross ref file.  Resp:000000013  Reas:0000";

/** V-COACTVWC-002 as the server answers a key that reached it through the URL. */
const ZERO_KEY = ACCOUNT_KEY_MESSAGES.accountFilterMustBeNonZero11Digit;

/** The status region's texts while the account is read and once it is. */
const ACCOUNT_LOADING = "Loading account";
const ACCOUNT_LOADED = `Account ${account.account_id} loaded`;

/** The global the API client sends each Request through. */
type FetchFn = (input: Request, init?: RequestInit) => Promise<Response>;

let fetchMock: Mock<FetchFn>;

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** Answers every request with `status` and `body`. */
function respondWith(status: number, body: AccountResponse | ErrorBody): void {
  fetchMock.mockImplementation(async () => jsonResponse(status, body));
}

/** The requests the page sent, in order. */
function sentRequests(): Request[] {
  return fetchMock.mock.calls.map(([request]) => request);
}

function renderPage(initialEntry: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  // The route shape of the app's account view, with the key optional so the
  // key-entry state renders too; PageHeader's useMatches needs a data router.
  const router = createMemoryRouter(
    [
      {
        path: "/accounts/:accountId?",
        element: <AccountViewPage />,
        handle: { crumb: "Account" },
      },
    ],
    { initialEntries: [initialEntry] },
  );
  render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
        <Toaster />
      </QueryClientProvider>
    </ThemeProvider>,
  );
  return { router };
}

/** ACCTSID, by the label F-COACTVW-001 gives it. */
function accountInput(): HTMLElement {
  return screen.getByLabelText("Account number");
}

/** Each detail-list label with the text of its value. */
function renderedDetails(): Record<string, string | null | undefined> {
  return Object.fromEntries(
    Array.from(document.querySelectorAll("dt"), (term) => [
      term.textContent,
      term.nextElementSibling?.textContent,
    ]),
  );
}

/** A promise and its resolver, so a test decides when an answer arrives. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

/** Holds the next answer back until the returned function sends it. */
function holdNextAnswer(): (
  status: number,
  body: AccountResponse | ErrorBody,
) => void {
  const answer = deferred<Response>();
  fetchMock.mockImplementationOnce(async () => answer.promise);
  return (status, body) => {
    answer.resolve(jsonResponse(status, body));
  };
}

/** The page's status region, the one on the page. */
function lookupStatus(): HTMLElement {
  return screen.getByRole("status");
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

beforeEach(() => {
  fetchMock = vi.fn<FetchFn>();
  respondWith(200, account);
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  observers.splice(0).forEach((observer) => {
    observer.disconnect();
  });
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("AccountViewPage", () => {
  it("reads the account a valid number names and shows its records", async () => {
    const user = userEvent.setup();
    const { router } = renderPage("/accounts");

    // ENTER on ACCTSID (F-COACTVW-001) opens the account's view
    // (F-COACTVW-002).
    await user.type(accountInput(), "00000000001{Enter}");
    await screen.findByText(account.cust_last_name);

    expect(router.state.location.pathname).toBe("/accounts/00000000001");
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [request] = sentRequests();
    expect(request.method).toBe("GET");
    expect(new URL(request.url).pathname).toBe("/api/accounts/00000000001");

    // Every COACTVW output field 1200-SETUP-SCREEN-VARS fills, under its map
    // caption, plus the cross-referenced card. Values are shown as received;
    // only the SSN gets the map's separators.
    expect(renderedDetails()).toEqual({
      "Active Y/N": account.acct_active_status,
      Opened: account.acct_open_date,
      Expiry: account.acct_expiraion_date,
      Reissue: account.acct_reissue_date,
      "Account Group": account.acct_group_id,
      "Credit Limit": account.acct_credit_limit,
      "Cash credit Limit": account.acct_cash_credit_limit,
      "Current Balance": account.acct_curr_bal,
      "Current Cycle Credit": account.acct_curr_cyc_credit,
      "Current Cycle Debit": account.acct_curr_cyc_debit,
      "Customer id": account.customer_id,
      SSN: "020-97-3888",
      "Date of birth": account.cust_dob_yyyy_mm_dd,
      "FICO Score": account.cust_fico_credit_score,
      "First Name": account.cust_first_name,
      "Middle Name": account.cust_middle_name,
      "Last Name": account.cust_last_name,
      // ACSADL1 and ACSADL2 are two block lines of the one Address value.
      Address: `${account.cust_addr_line_1}${account.cust_addr_line_2}`,
      City: account.cust_addr_line_3,
      State: account.cust_addr_state_cd,
      Zip: account.cust_addr_zip,
      Country: account.cust_addr_country_cd,
      "Phone 1": account.cust_phone_num_1,
      "Government Issued Id Ref": account.cust_govt_issued_id,
      "Phone 2": account.cust_phone_num_2,
      "EFT Account Id": account.cust_eft_account_id,
      "Primary Card Holder Y/N": account.cust_pri_card_holder_ind,
      "Card Number": account.card_number,
    });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it.each([
    { entered: "", message: ACCOUNT_KEY_MESSAGES.noInputReceived },
    { entered: "   ", message: ACCOUNT_KEY_MESSAGES.noInputReceived },
    { entered: "*", message: ACCOUNT_KEY_MESSAGES.noInputReceived },
    {
      entered: "abc",
      message: ACCOUNT_KEY_MESSAGES.accountFilterMustBeNonZero11Digit,
    },
    {
      entered: "0000000001",
      message: ACCOUNT_KEY_MESSAGES.accountFilterMustBeNonZero11Digit,
    },
    {
      entered: "0000000000A",
      message: ACCOUNT_KEY_MESSAGES.accountFilterMustBeNonZero11Digit,
    },
    {
      entered: "00000000000",
      message: ACCOUNT_KEY_MESSAGES.accountFilterMustBeNonZero11Digit,
    },
  ])(
    "rejects the account number $entered without a request",
    async ({ entered, message }) => {
      const user = userEvent.setup();
      const { router } = renderPage("/accounts");

      await user.type(accountInput(), `${entered}{Enter}`);
      const fieldError = await screen.findByRole("alert");

      expect(fieldError.textContent).toBe(message);
      expect(accountInput().getAttribute("aria-invalid")).toBe("true");
      expect(router.state.location.pathname).toBe("/accounts");
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  // `entered` is typed into ACCTSID and submitted; without it the key reaches
  // the server through the route alone.
  it.each<{
    name: string;
    initialEntry: string;
    entered: string | null;
    key: string;
    status: number;
    code: ErrorBody["code"];
    message: string;
  }>([
    {
      name: "typed key, V-COACTVWC-003",
      initialEntry: "/accounts",
      entered: "99999999999",
      key: "99999999999",
      status: 404,
      code: "NOT_FOUND",
      message: XREF_NOT_FOUND,
    },
    {
      name: "route key, V-COACTVWC-002",
      initialEntry: "/accounts/00000000000",
      entered: null,
      key: "00000000000",
      status: 400,
      code: "INVALID",
      message: ZERO_KEY,
    },
  ])(
    "shows the server's rejection at ACCTSID: $name",
    async ({ initialEntry, entered, key, status, code, message }) => {
      const toastError = vi.spyOn(toast, "error");
      respondWith(status, {
        code,
        message,
        fields: [{ field: "ACCTSID", message }],
      });
      const user = userEvent.setup();
      const { router } = renderPage(initialEntry);

      if (entered !== null) {
        // Submitted with the button, so the cursor starts away from ACCTSID.
        await user.type(accountInput(), entered);
        await user.click(
          screen.getByRole("button", { name: "Look up account" }),
        );
      }
      const fieldError = await screen.findByRole("alert");

      // The text is compared as sent: its double spaces are part of it.
      expect(fieldError.textContent).toBe(message);
      expect(toastError).toHaveBeenCalledTimes(1);
      expect(toastError).toHaveBeenCalledWith(message);
      await waitFor(() => {
        expect(document.activeElement).toBe(accountInput());
      });
      expect(router.state.location.pathname).toBe(`/accounts/${key}`);
      expect(fetchMock).toHaveBeenCalledTimes(1);
      const [request] = sentRequests();
      expect(request.method).toBe("GET");
      expect(new URL(request.url).pathname).toBe(`/api/accounts/${key}`);
      expect(document.querySelector("dl")).toBeNull();
    },
  );

  it("announces each read by its account, a repeated lookup included", async () => {
    const user = userEvent.setup();
    renderPage("/accounts");

    // Present and empty before any lookup, so assistive technology tracks the
    // region before it first changes.
    const region = lookupStatus();
    expect(region.textContent).toBe("");

    await user.type(accountInput(), `${account.account_id}{Enter}`);

    await waitFor(() => {
      expect(region.textContent).toBe(ACCOUNT_LOADED);
    });
    // The same element throughout, naming the account and never a detail
    // value such as the SSN, a balance or the card number.
    expect(lookupStatus()).toBe(region);
    for (const detail of [
      account.cust_ssn,
      "020-97-3888",
      account.acct_curr_bal,
      account.card_number,
      account.cust_last_name,
    ]) {
      expect(region.textContent).not.toContain(detail);
    }

    // ENTER on the key on screen reads it again; its answer is held so that
    // the progress is seen.
    const release = holdNextAnswer();
    const texts = recordTexts(region);
    await user.click(screen.getByRole("button", { name: "Look up account" }));

    await waitFor(() => {
      expect(region.textContent).toBe(ACCOUNT_LOADING);
    });
    release(200, account);
    await waitFor(() => {
      expect(region.textContent).toBe(ACCOUNT_LOADED);
    });
    // Cleared before each text, so the unchanged result is announced again.
    expect(texts).toEqual(["", ACCOUNT_LOADING, "", ACCOUNT_LOADED]);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("empties the status region when a lookup after a found account fails, and its toast and field error report it", async () => {
    const toastError = vi.spyOn(toast, "error");
    const user = userEvent.setup();
    const { router } = renderPage(`/accounts/${account.account_id}`);
    const region = lookupStatus();
    await waitFor(() => {
      expect(region.textContent).toBe(ACCOUNT_LOADED);
    });

    // The rejection is held so that the lookup's progress is seen.
    const release = holdNextAnswer();
    const texts = recordTexts(region);
    await user.clear(accountInput());
    await user.type(accountInput(), "99999999999{Enter}");

    await waitFor(() => {
      expect(region.textContent).toBe(ACCOUNT_LOADING);
    });
    release(404, {
      code: "NOT_FOUND",
      message: XREF_NOT_FOUND,
      fields: [{ field: "ACCTSID", message: XREF_NOT_FOUND }],
    });

    const fieldError = await screen.findByRole("alert");
    expect(fieldError.textContent).toBe(XREF_NOT_FOUND);
    expect(toastError).toHaveBeenCalledTimes(1);
    expect(toastError).toHaveBeenCalledWith(XREF_NOT_FOUND);
    await waitFor(() => {
      expect(region.textContent).toBe("");
    });
    expect(texts).toEqual(["", ACCOUNT_LOADING, ""]);
    expect(router.state.location.pathname).toBe("/accounts/99999999999");
    expect(document.querySelector("dl")).toBeNull();
  });
});
