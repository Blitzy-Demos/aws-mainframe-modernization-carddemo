// FN tests of BillPaymentPage, the CB00 "Bill Payment" screen (COBIL00C, map
// COBIL00) at /payments. They drive the controls of the F-COBIL00 rows of
// docs/parity/function-map.md and check the texts of the V-COBIL00C rows of
// docs/parity/validation-audit.md:
// - ENTER ("Review payment") sends POST /payments with CONFIRMI blank, which
//   answers the balance and 'Confirm to make a bill payment...'; the balance
//   card then reads GET /accounts/{id}, and the confirmation dialog opens. Its
//   "Confirm" (CONFIRM Y) sends POST /payments with `confirm: "Y"`, and the
//   201 shows the COBIL00C success text and blanks the entry
//   (F-COBIL00-001, F-COBIL00-002);
// - "Cancel" and Escape (CONFIRM N) send no payment and clear the screen
//   (F-COBIL00-003);
// - a blank account ID never reaches the network (V-COBIL00C-001);
// - a server refusal, of the review or of the payment, is toasted, set on
//   ACTIDIN and puts the cursor there, as `MOVE -1 TO ACTIDINL` does;
// - "Reset" (F4) clears the screen and "Back" (F3) leaves it, sending nothing
//   (F-COBIL00-005, F-COBIL00-004).
//
// The network is the stubbed global the API client looks up on every request,
// so each test sees the exact requests the page made.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  getDefaultNormalizer,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";

import type { components } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { PAYMENT_MESSAGES } from "@/schemas/payments";

import BillPaymentPage from "./BillPaymentPage";

type AccountResponse = components["schemas"]["AccountResponse"];
type PaymentRequest = components["schemas"]["PaymentRequest"];
type PaymentResponse = components["schemas"]["PaymentResponse"];
type BillPaymentField = components["schemas"]["BillPaymentField"];
type ErrorBody = components["schemas"]["ErrorBody"];

const ACCOUNT_ID = "00000000001";
const ACCOUNT_PATH = `/api/accounts/${ACCOUNT_ID}`;
const PAYMENTS_PATH = "/api/payments";

/** The BMS field every COBIL00C refusal below names [app/cpy-bms/COBIL00.CPY]. */
const ACTIDIN = "ACTIDIN" satisfies BillPaymentField;

/** The label of ACTIDIN, the title of the CURBAL card and of the dialog. */
const ACCOUNT_LABEL = "Account ID";
const BALANCE_TITLE = "Your current balance is";
const DIALOG_TITLE = "Confirm the bill payment";

/**
 * `GET /accounts/00000000001` on the seeded corpus: the account record
 * [app/data/ASCII/acctdata.txt:L1], customer 000000001
 * [app/data/ASCII/custdata.txt:L1] and the account's card from its
 * cross-reference [app/data/ASCII/cardxref.txt:L49].
 */
const account = {
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
} satisfies AccountResponse;

// V-COBIL00C-008: a positive balance and CONFIRMI blank, so PROCESS-ENTER-KEY
// shows the balance, asks for the confirmation and writes nothing (200).
const REVIEW_MESSAGE = "Confirm to make a bill payment...";

const reviewed = {
  account_id: ACCOUNT_ID,
  current_balance: account.acct_curr_bal,
  message: REVIEW_MESSAGE,
} satisfies PaymentResponse;

// WRITE-TRANSACT-FILE NORMAL: 'Payment successful. ' then ' Your Transaction
// ID is ' TRAN-ID '.', two spaces after the period [app/cbl/COBIL00C.cbl:L527-531].
const PAID_MESSAGE =
  "Payment successful.  Your Transaction ID is 0000000000000001.";

// The whole balance paid: ACCT-CURR-BAL minus TRAN-AMT, the balance itself.
const paid = {
  account_id: ACCOUNT_ID,
  current_balance: "0.00",
  tran_id: "0000000000000001",
  message: PAID_MESSAGE,
} satisfies PaymentResponse;

// The two ENTERs the page sends: the review with CONFIRMI blank, then the
// dialog's Confirm with CONFIRMI 'Y' [app/cbl/COBIL00C.cbl:L173-184].
const REVIEW_BODY = { account_id: ACCOUNT_ID, confirm: "" } satisfies PaymentRequest;
const PAY_BODY = { account_id: ACCOUNT_ID, confirm: "Y" } satisfies PaymentRequest;

// The COBIL00C texts on ACTIDIN the refusals below answer with, byte-identical
// to their audit rows.
const ACCOUNT_NOT_FOUND = "Account ID NOT found...";
const NOTHING_TO_PAY = "You have nothing to pay...";
const TRAN_ID_EXISTS = "Tran ID already exist...";

/** A COBIL00C refusal whose cursor field is ACTIDIN. */
function refusal(code: string, message: string): ErrorBody {
  return { code, message, fields: [{ field: ACTIDIN, message }] };
}

const PAYMENT_HANDLE = {
  title: "Bill Payment",
  crumb: "Bill Payment",
} satisfies RouteHandle;

/** An HTTP status and the JSON body sent with it. */
type Reply<B> = readonly [status: number, body: B];

/** The answer of each request the screen can send. */
type Stubs = {
  /** `GET /accounts/00000000001`, the balance card. */
  account: Reply<AccountResponse | ErrorBody>;
  /** `POST /payments` with any confirmation but `Y`: the review. */
  review: Reply<PaymentResponse | ErrorBody>;
  /** `POST /payments` with `confirm: "Y"`: the payment. */
  pay: Reply<PaymentResponse | ErrorBody>;
};

type FetchFn = typeof globalThis.fetch;

let stubs: Stubs;
// Every request the page sent, in order.
let sent: Request[];
let fetchMock: Mock<FetchFn>;
let queryClient: QueryClient;

function isPayment(body: unknown): boolean {
  return (
    typeof body === "object" &&
    body !== null &&
    "confirm" in body &&
    body.confirm === PAY_BODY.confirm
  );
}

// Answers by method and pathname, and a POST by its confirmation. Every GET
// of the account is answered alike, because the payment hook reloads it after
// each payment settles.
async function answer(request: Request): Promise<Response> {
  const { pathname } = new URL(request.url);
  let reply: Reply<AccountResponse | PaymentResponse | ErrorBody>;
  if (request.method === "GET" && pathname === ACCOUNT_PATH) {
    reply = stubs.account;
  } else if (request.method === "POST" && pathname === PAYMENTS_PATH) {
    const body: unknown = await request.clone().json();
    reply = isPayment(body) ? stubs.pay : stubs.review;
  } else {
    throw new Error(`No answer stubbed for ${request.method} ${pathname}`);
  }
  const [status, body] = reply;
  return Response.json(body, { status });
}

function renderPage(): UserEvent {
  queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const router = createMemoryRouter(
    [
      {
        path: "/payments",
        element: <BillPaymentPage />,
        handle: PAYMENT_HANDLE,
      },
      {
        path: "/",
        element: <p>Home</p>,
        handle: { crumb: "Home" } satisfies RouteHandle,
      },
    ],
    { initialEntries: ["/payments"] },
  );
  render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
        <Toaster />
      </QueryClientProvider>
    </ThemeProvider>,
  );
  return userEvent.setup();
}

// ACTIDIN by its label. A label query, unlike a role query, still finds the
// input while the modal dialog hides the page from assistive technology.
function accountInput(): HTMLInputElement {
  return screen.getByLabelText<HTMLInputElement>(ACCOUNT_LABEL, {
    selector: "input",
  });
}

// The FieldError announced for `input`: the alert its aria-describedby names,
// so the text is tied to the input for assistive technology, not merely nearby.
function fieldErrorOf(input: HTMLElement): HTMLElement | null {
  const ids = (input.getAttribute("aria-describedby") ?? "")
    .split(" ")
    .filter((id) => id !== "");
  for (const id of ids) {
    const element = document.getElementById(id);
    if (element?.getAttribute("role") === "alert") {
      return element;
    }
  }
  return null;
}

function requestsOf(method: "GET" | "POST"): Request[] {
  return sent.filter((request) => request.method === method);
}

function pathnameOf(request: Request): string {
  return new URL(request.url).pathname;
}

function bodiesOf(requests: Request[]): Promise<unknown[]> {
  return Promise.all(requests.map((request) => request.clone().json()));
}

// The CURBAL card, once its read has answered. `hidden` lets the query reach
// it behind the open dialog.
function findBalanceCard(): Promise<HTMLElement> {
  return screen.findByRole("group", { name: BALANCE_TITLE, hidden: true });
}

// ENTER on `entry` typed into ACTIDIN, answered by the review stub; resolves
// to the confirmation dialog the 200 opens.
async function review(user: UserEvent, entry: string = ACCOUNT_ID): Promise<HTMLElement> {
  await user.type(accountInput(), `${entry}{Enter}`);
  return screen.findByRole("alertdialog", { name: DIALOG_TITLE });
}

beforeEach(() => {
  stubs = {
    account: [200, account],
    review: [200, reviewed],
    pay: [201, paid],
  };
  sent = [];
  // The client passes a Request; anything else is normalised to one.
  fetchMock = vi.fn<FetchFn>(async (input, init) => {
    const request = input instanceof Request ? input : new Request(input, init);
    sent.push(request);
    return answer(request);
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.spyOn(toast, "success");
  vi.spyOn(toast, "error");
});

afterEach(() => {
  toast.dismiss();
  queryClient.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("BillPaymentPage (CB00, COBIL00)", () => {
  it(
    "reviews the balance on ENTER and pays it on Confirm (F-COBIL00-001, F-COBIL00-002)",
    async () => {
      const user = renderPage();

      // The IC attribute of ACTIDIN [app/bms/COBIL00.bms:L85]: the screen opens
      // with the cursor there, so ENTER needs nothing but the keyboard.
      expect(document.activeElement).toBe(accountInput());
      await user.keyboard(`${ACCOUNT_ID}{Enter}`);
      const dialog = await screen.findByRole("alertdialog", {
        name: DIALOG_TITLE,
      });

      // The review: CONFIRMI blank, then the card's read of the account.
      const card = await findBalanceCard();
      expect(within(card).getByText(account.acct_curr_bal)).toBeTruthy();
      expect(sent.map((request) => `${request.method} ${pathnameOf(request)}`)).toEqual([
        `POST ${PAYMENTS_PATH}`,
        `GET ${ACCOUNT_PATH}`,
      ]);
      expect(await bodiesOf(requestsOf("POST"))).toEqual([REVIEW_BODY]);
      expect(await screen.findByText(REVIEW_MESSAGE)).toBeTruthy();
      // The dialog asks about the entry and the balance the review answered.
      expect(within(dialog).getByText(ACCOUNT_ID)).toBeTruthy();
      expect(within(dialog).getByText(reviewed.current_balance)).toBeTruthy();

      await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

      await waitFor(() => {
        expect(toast.success).toHaveBeenCalledWith(PAID_MESSAGE);
      });
      const posts = requestsOf("POST");
      expect(posts.map(pathnameOf)).toEqual([PAYMENTS_PATH, PAYMENTS_PATH]);
      expect(await bodiesOf(posts)).toEqual([REVIEW_BODY, PAY_BODY]);
      for (const read of requestsOf("GET")) {
        expect(pathnameOf(read)).toBe(ACCOUNT_PATH);
      }
      // The success text reaches the screen byte for byte, both spaces kept.
      expect(
        await screen.findByText(PAID_MESSAGE, {
          normalizer: getDefaultNormalizer({ collapseWhitespace: false }),
        }),
      ).toBeTruthy();

      // INITIALIZE-ALL-FIELDS blanks the entry, the cursor is back on ACTIDIN,
      // and the card shows the balance the 201 answered with, although the
      // reloaded record still holds the old one.
      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
        expect(accountInput().value).toBe("");
        expect(document.activeElement).toBe(accountInput());
      });
      const paidCard = screen.getByRole("group", { name: BALANCE_TITLE });
      expect(within(paidCard).getByText(paid.current_balance)).toBeTruthy();
      expect(within(paidCard).queryByText(account.acct_curr_bal)).toBeNull();
      expect(toast.error).not.toHaveBeenCalled();
    },
  );

  // CONFIRM N: CLEAR-CURRENT-SCREEN, no write [app/cbl/COBIL00C.cbl:L178-181].
  const DISMISSALS: readonly {
    how: string;
    dismiss: (user: UserEvent, dialog: HTMLElement) => Promise<void>;
  }[] = [
    {
      how: "Cancel",
      dismiss: async (user, dialog) => {
        await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
      },
    },
    {
      how: "the Escape key",
      dismiss: async (user) => {
        await user.keyboard("{Escape}");
      },
    },
  ];

  it.each(DISMISSALS)(
    "sends no payment and clears the screen when $how answers the confirmation (F-COBIL00-003)",
    async ({ dismiss }) => {
      const user = renderPage();

      const dialog = await review(user);
      await findBalanceCard();
      await dismiss(user, dialog);

      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
        expect(document.activeElement).toBe(accountInput());
      });
      expect(await bodiesOf(requestsOf("POST"))).toEqual([REVIEW_BODY]);
      expect(accountInput().value).toBe("");
      expect(screen.queryByRole("group", { name: BALANCE_TITLE })).toBeNull();
      expect(toast.success).not.toHaveBeenCalled();
      expect(toast.error).not.toHaveBeenCalled();
    },
  );

  // ACTIDINI = SPACES OR LOW-VALUES, the one COBIL00C edit the form repeats.
  const BLANK_ENTRIES: readonly {
    what: string;
    entry: string;
    submit: (user: UserEvent) => Promise<void>;
  }[] = [
    {
      what: "nothing entered, Review payment clicked",
      entry: "",
      submit: async (user) => {
        await user.click(screen.getByRole("button", { name: "Review payment" }));
      },
    },
    {
      what: "spaces entered, Enter pressed",
      entry: "   ",
      submit: async (user) => {
        await user.keyboard("{Enter}");
      },
    },
  ];

  it.each(BLANK_ENTRIES)(
    "keeps a blank account ID off the network and shows its text ($what, V-COBIL00C-001)",
    async ({ entry, submit }) => {
      const user = renderPage();

      if (entry !== "") {
        await user.type(accountInput(), entry);
      }
      await submit(user);

      const input = accountInput();
      await waitFor(() => {
        expect(input.getAttribute("aria-invalid")).toBe("true");
        expect(document.activeElement).toBe(input);
      });
      expect(fieldErrorOf(input)?.textContent).toBe(PAYMENT_MESSAGES.accountIdEmpty);
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  // The ACTIDIN refusals of the review, each with the status of its audit row.
  // A non-digit entry is not a client edit: COBIL00C tests ACTIDINI only for
  // blanks, and the server answers a non-digit as the account NOTFND row, so
  // the entry goes out exactly as typed.
  const REVIEW_REFUSALS: readonly {
    row: string;
    entry: string;
    status: number;
    code: string;
    message: string;
  }[] = [
    {
      row: "V-COBIL00C-006",
      entry: "ABCDEFGHIJK",
      status: 404,
      code: "NOT_FOUND",
      message: ACCOUNT_NOT_FOUND,
    },
    {
      row: "V-COBIL00C-007",
      entry: "00000000004",
      status: 400,
      code: "INVALID",
      message: NOTHING_TO_PAY,
    },
  ];

  it.each(REVIEW_REFUSALS)(
    "sends $entry as typed and shows the $status answer at the account ID ($row)",
    async ({ entry, status, code, message }) => {
      stubs.review = [status, refusal(code, message)];
      const user = renderPage();

      await user.type(accountInput(), `${entry}{Enter}`);

      await waitFor(() => {
        expect(toast.error).toHaveBeenCalledWith(message);
      });
      const input = accountInput();
      await waitFor(() => {
        expect(input.getAttribute("aria-invalid")).toBe("true");
        expect(document.activeElement).toBe(input);
      });
      expect(fieldErrorOf(input)?.textContent).toBe(message);
      // SEND-BILLPAY-SCREEN on an error keeps what was entered.
      expect(input.value).toBe(entry);
      expect(sent.map((request) => request.method)).toEqual(["POST"]);
      expect(await bodiesOf(sent)).toEqual([
        { account_id: entry, confirm: "" } satisfies PaymentRequest,
      ]);
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(screen.queryByRole("group", { name: BALANCE_TITLE })).toBeNull();
      expect(toast.success).not.toHaveBeenCalled();
    },
  );

  // The ACTIDIN refusals of the confirmed payment: the balance paid off since
  // the review (the guard runs on every ENTER), or a TRAN-ID collision.
  const PAYMENT_REFUSALS: readonly {
    row: string;
    status: number;
    code: string;
    message: string;
  }[] = [
    { row: "V-COBIL00C-007", status: 400, code: "INVALID", message: NOTHING_TO_PAY },
    { row: "V-COBIL00C-014", status: 409, code: "DUPLICATE", message: TRAN_ID_EXISTS },
  ];

  it.each(PAYMENT_REFUSALS)(
    "shows a $status answer to Confirm at the account ID and keeps the entry ($row)",
    async ({ status, code, message }) => {
      stubs.pay = [status, refusal(code, message)];
      const user = renderPage();

      const dialog = await review(user);
      await findBalanceCard();
      await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

      await waitFor(() => {
        expect(toast.error).toHaveBeenCalledWith(message);
      });
      const input = accountInput();
      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
        expect(input.getAttribute("aria-invalid")).toBe("true");
        expect(document.activeElement).toBe(input);
      });
      expect(fieldErrorOf(input)?.textContent).toBe(message);
      expect(input.value).toBe(ACCOUNT_ID);
      expect(await bodiesOf(requestsOf("POST"))).toEqual([REVIEW_BODY, PAY_BODY]);
      // A refused payment may still have written (E-19), so the balance on
      // file is read again: the card's first read, then the reload.
      expect(requestsOf("GET").map(pathnameOf)).toEqual([ACCOUNT_PATH, ACCOUNT_PATH]);
      expect(toast.success).not.toHaveBeenCalled();
    },
  );

  it("clears the entry, its error and the balance on Reset and sends nothing (F-COBIL00-005)", async () => {
    stubs.pay = [409, refusal("DUPLICATE", TRAN_ID_EXISTS)];
    const user = renderPage();

    const dialog = await review(user);
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));
    await waitFor(() => {
      expect(toast.error).toHaveBeenCalledWith(TRAN_ID_EXISTS);
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(accountInput().getAttribute("aria-invalid")).toBe("true");
    });
    await findBalanceCard();
    const before = sent.length;

    await user.click(screen.getByRole("button", { name: "Reset" }));

    const input = accountInput();
    await waitFor(() => {
      expect(input.value).toBe("");
      expect(input.getAttribute("aria-invalid")).toBe("false");
      expect(document.activeElement).toBe(input);
    });
    expect(fieldErrorOf(input)).toBeNull();
    expect(screen.queryByRole("group", { name: BALANCE_TITLE })).toBeNull();
    expect(sent).toHaveLength(before);
  });

  it("returns to the main menu on Back and sends nothing (F-COBIL00-004)", async () => {
    const user = renderPage();

    expect(screen.getByRole("heading", { level: 1, name: "Bill Payment" })).toBeTruthy();
    await user.click(screen.getByRole("link", { name: "Back" }));

    expect(await screen.findByText("Home")).toBeTruthy();
    expect(screen.queryByLabelText(ACCOUNT_LABEL)).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

