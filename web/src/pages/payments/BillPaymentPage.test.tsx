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
// - a confirmed payment without an answer before the client's deadline is
//   sent once and toasted with the deadline text and a note that it may have
//   been made;
// - an answer to the review or to the payment, or a failed balance reload,
//   that arrives after the entry was replaced is still toasted but neither
//   blanks nor marks the newer entry;
// - "Reset" (F4) clears the screen and "Back" (F3) leaves it, sending nothing
//   (F-COBIL00-005, F-COBIL00-004).
//
// The network is the stubbed global the API client looks up on every request,
// so each test sees the exact requests the page made.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
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

import { RequestTimeoutError } from "@/api/client";
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
const UNABLE_TO_ADD = "Unable to Add Bill pay Transaction...";
const UNABLE_TO_UPDATE = "Unable to Update Account...";

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
  /**
   * `POST /payments` with any confirmation but `Y`: the review, or a promise
   * of its answer while a test holds it.
   */
  review:
    | Reply<PaymentResponse | ErrorBody>
    | Promise<Reply<PaymentResponse | ErrorBody>>;
  /**
   * `POST /payments` with `confirm: "Y"`: the payment, or a promise of its
   * answer while a test holds it.
   */
  pay:
    | Reply<PaymentResponse | ErrorBody>
    | Promise<Reply<PaymentResponse | ErrorBody>>;
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
// a confirmed payment that may have written settles.
async function answer(request: Request): Promise<Response> {
  const { pathname } = new URL(request.url);
  let reply: Reply<AccountResponse | PaymentResponse | ErrorBody>;
  if (request.method === "GET" && pathname === ACCOUNT_PATH) {
    reply = stubs.account;
  } else if (request.method === "POST" && pathname === PAYMENTS_PATH) {
    const body: unknown = await request.clone().json();
    reply = await (isPayment(body) ? stubs.pay : stubs.review);
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

/** The status text of a CURBAL card whose balance may be about to change. */
const BALANCE_UPDATING = "Updating the current balance";

// The busy cues of the CURBAL card: its wrapper's aria-busy and status text
// (the status is rendered throughout, so its text is announced when it
// appears), and the card's dimming and spinning icon. `hidden` lets the query
// reach the status behind the open dialog.
function balanceCuesOf(card: HTMLElement): {
  busy: string | null;
  status: string | null;
  dimmed: boolean;
  spinning: boolean;
} {
  const wrapper = card.parentElement;
  if (wrapper === null) {
    throw new Error("The balance card has no wrapper");
  }
  return {
    busy: wrapper.getAttribute("aria-busy"),
    status: within(wrapper).getByRole("status", { hidden: true }).textContent,
    dimmed: card.classList.contains("opacity-60"),
    spinning: card.querySelector('[class*="animate-spin"]') !== null,
  };
}

// ENTER on `entry` typed into ACTIDIN, answered by the review stub; resolves
// to the confirmation dialog the 200 opens.
async function review(user: UserEvent, entry: string = ACCOUNT_ID): Promise<HTMLElement> {
  await user.type(accountInput(), `${entry}{Enter}`);
  return screen.findByRole("alertdialog", { name: DIALOG_TITLE });
}

// Holds the answer to the review or to the payment until the returned
// function releases it.
function hold(request: "review" | "pay"): (reply: Reply<PaymentResponse | ErrorBody>) => void {
  let release: (reply: Reply<PaymentResponse | ErrorBody>) => void = () => undefined;
  stubs[request] = new Promise((resolve) => {
    release = resolve;
  });
  return (reply) => {
    release(reply);
  };
}

// Lets the timer task pass on which react-hook-form's setFocus moves focus.
async function nextTask(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
  });
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

  // Two activations in one tick, before the page re-renders, as a double click
  // can send them. The mutations' isPending only changes with the next
  // render, so only the page's synchronous guard drops the second. `bodies`
  // is every POST /payments the page then sends.
  const DOUBLE_ACTIVATIONS: readonly {
    control: string;
    activateTwice: (user: UserEvent) => Promise<void>;
    bodies: readonly PaymentRequest[];
  }[] = [
    {
      control: "Review payment (F-COBIL00-001)",
      activateTwice: async (user) => {
        await user.type(accountInput(), ACCOUNT_ID);
        const form = accountInput().form;
        if (form === null) {
          throw new Error("The account input is outside a form");
        }
        // handleSubmit runs the schema asynchronously, so both submits reach
        // the review after this tick.
        fireEvent.submit(form);
        fireEvent.submit(form);
        await screen.findByRole("alertdialog", { name: DIALOG_TITLE });
        await findBalanceCard();
      },
      bodies: [REVIEW_BODY],
    },
    {
      control: "Confirm (F-COBIL00-002)",
      activateTwice: async (user) => {
        const dialog = await review(user);
        await findBalanceCard();
        const confirm = within(dialog).getByRole("button", { name: "Confirm" });
        // One act scope: the dialog neither closes nor disables its action
        // between the presses.
        act(() => {
          fireEvent.click(confirm);
          fireEvent.click(confirm);
        });
        await waitFor(() => {
          expect(toast.success).toHaveBeenCalledWith(PAID_MESSAGE);
        });
      },
      bodies: [REVIEW_BODY, PAY_BODY],
    },
  ];

  it.each(DOUBLE_ACTIVATIONS)(
    "sends one request for two activations of $control in one tick",
    async ({ activateTwice, bodies }) => {
      const user = renderPage();

      await activateTwice(user);

      await nextTask();
      expect(await bodiesOf(requestsOf("POST"))).toEqual(bodies);
      expect(toast.error).not.toHaveBeenCalled();
    },
  );

  it("marks the submit busy while the review runs and lets Reset abandon it (F-COBIL00-001, F-COBIL00-005)", async () => {
    const release = hold("review");
    const user = renderPage();

    await user.type(accountInput(), `${ACCOUNT_ID}{Enter}`);
    // The exact function-map name, the spinner being hidden from it.
    const submit = screen.getByRole<HTMLButtonElement>("button", { name: "Review payment" });
    const reset = screen.getByRole<HTMLButtonElement>("button", { name: "Reset" });
    await waitFor(() => {
      expect(requestsOf("POST")).toHaveLength(1);
      expect(submit.getAttribute("aria-busy")).toBe("true");
    });
    // Inert but focusable, and Reset, the way to abandon the read-only
    // review, stays usable.
    expect(submit.getAttribute("aria-disabled")).toBe("true");
    expect(submit.disabled).toBe(false);
    expect(reset.disabled).toBe(false);

    await user.click(reset);

    const input = accountInput();
    await waitFor(() => {
      expect(input.value).toBe("");
      expect(document.activeElement).toBe(input);
      expect(submit.getAttribute("aria-busy")).toBe("false");
    });
    expect(submit.getAttribute("aria-disabled")).toBe("false");
    // The abandoned review's answer opens nothing over the cleared screen.
    release([200, reviewed]);
    await nextTask();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.queryByRole("group", { name: BALANCE_TITLE })).toBeNull();

    // The next ENTER is sent and answered.
    stubs.review = [200, reviewed];
    await review(user);
    await findBalanceCard();
    expect(await bodiesOf(requestsOf("POST"))).toEqual([REVIEW_BODY, REVIEW_BODY]);
    expect(toast.error).not.toHaveBeenCalled();
  });

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

  it.each(REVIEW_REFUSALS)(
    "keeps an account typed while $entry is reviewed when its $status answer arrives ($row)",
    async ({ entry, status, code, message }) => {
      const release = hold("review");
      const user = renderPage();

      await user.type(accountInput(), `${entry}{Enter}`);
      await waitFor(() => {
        expect(requestsOf("POST")).toHaveLength(1);
      });
      await user.clear(accountInput());
      await user.type(accountInput(), ACCOUNT_ID);
      release([status, refusal(code, message)]);

      // The refusal is still toasted.
      await waitFor(() => {
        expect(toast.error).toHaveBeenCalledWith(message);
      });
      await nextTask();
      // The newer entry is neither changed nor marked.
      const input = accountInput();
      expect(input.value).toBe(ACCOUNT_ID);
      expect(input.getAttribute("aria-invalid")).toBe("false");
      expect(fieldErrorOf(input)).toBeNull();
      expect(document.activeElement).toBe(input);
      expect(await bodiesOf(sent)).toEqual([
        { account_id: entry, confirm: "" } satisfies PaymentRequest,
      ]);
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(screen.queryByRole("group", { name: BALANCE_TITLE })).toBeNull();
    },
  );

  // The ACTIDIN refusals of the confirmed payment: the balance paid off since
  // the review (the guard runs on every ENTER), which writes nothing, and the
  // failed writes after which the other write still stands (E-19): a TRAN-ID
  // collision, the transaction insert failing, the account rewrite finding no
  // record or failing. `reads` is the account reads: the card's first read,
  // then, after a refusal that may follow a write, the reload of the balance
  // on file.
  const PAYMENT_REFUSALS: readonly {
    row: string;
    status: number;
    code: string;
    message: string;
    reads: number;
  }[] = [
    { row: "V-COBIL00C-007", status: 400, code: "INVALID", message: NOTHING_TO_PAY, reads: 1 },
    { row: "V-COBIL00C-014", status: 409, code: "DUPLICATE", message: TRAN_ID_EXISTS, reads: 2 },
    { row: "V-COBIL00C-015", status: 500, code: "UNEXPECTED", message: UNABLE_TO_ADD, reads: 2 },
    { row: "V-COBIL00C-016", status: 404, code: "NOT_FOUND", message: ACCOUNT_NOT_FOUND, reads: 2 },
    { row: "V-COBIL00C-017", status: 500, code: "UNEXPECTED", message: UNABLE_TO_UPDATE, reads: 2 },
  ];

  it.each(PAYMENT_REFUSALS)(
    "shows a $status answer to Confirm at the account ID and keeps the entry ($row)",
    async ({ status, code, message, reads }) => {
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
      expect(requestsOf("GET").map(pathnameOf)).toEqual(
        Array.from({ length: reads }, () => ACCOUNT_PATH),
      );
      expect(toast.success).not.toHaveBeenCalled();
    },
  );

  // The client gives up at its deadline while the server may still make the
  // payment: the toast says the payment may have been made, the request is
  // not sent again, and the balance on file is read again.
  it("sends an unanswered payment once and toasts the deadline with its unknown outcome", async () => {
    // The payment's fetch rejects as at the deadline. The handler only marks
    // the rejection handled until the stub awaits it.
    const deadline = Promise.reject(new RequestTimeoutError());
    deadline.catch(() => undefined);
    stubs.pay = deadline;
    const user = renderPage();

    const dialog = await review(user);
    await findBalanceCard();
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(toast.error).toHaveBeenCalledWith(
        "No answer from the server within 30 seconds",
        {
          description:
            "The payment may have been made. Check the account balance before paying again.",
          duration: Infinity,
        },
      );
    });
    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(await bodiesOf(requestsOf("POST"))).toEqual([REVIEW_BODY, PAY_BODY]);
    await waitFor(() => {
      expect(requestsOf("GET").map(pathnameOf)).toEqual([ACCOUNT_PATH, ACCOUNT_PATH]);
    });
    expect(accountInput().value).toBe(ACCOUNT_ID);
    expect(toast.success).not.toHaveBeenCalled();
  });

  // The entry typed while the review or the confirmed payment is still running.
  const NEWER_ACCOUNT = "00000000002";

  // Ways the entry is replaced while the payment runs, with the balance cards
  // then shown. Reset is not among them: it is disabled while the payment
  // writes (see the busy-state test of the payment).
  const ENTRY_CHANGES: readonly {
    how: string;
    change: (user: UserEvent) => Promise<void>;
    cards: number;
  }[] = [
    {
      how: "an account typed over the paid one",
      change: async (user) => {
        await user.clear(accountInput());
        await user.type(accountInput(), NEWER_ACCOUNT);
      },
      cards: 1,
    },
  ];

  // Answers to the confirmed payment: the toast each still raises and the
  // balance a shown card then holds (the 201's, or the reloaded record's).
  const LATE_PAYMENT_ANSWERS: readonly {
    outcome: string;
    reply: Reply<PaymentResponse | ErrorBody>;
    toast: "success" | "error";
    message: string;
    balance: string;
  }[] = [
    {
      outcome: "201",
      reply: [201, paid],
      toast: "success",
      message: PAID_MESSAGE,
      balance: paid.current_balance,
    },
    {
      outcome: "409 refusal (V-COBIL00C-014)",
      reply: [409, refusal("DUPLICATE", TRAN_ID_EXISTS)],
      toast: "error",
      message: TRAN_ID_EXISTS,
      balance: account.acct_curr_bal,
    },
  ];

  it.each(
    ENTRY_CHANGES.flatMap((change) =>
      LATE_PAYMENT_ANSWERS.map((late) => ({ ...change, ...late })),
    ),
  )(
    "keeps $how while the payment runs when its $outcome arrives",
    async ({ change, cards, reply, toast: kind, message, balance }) => {
      const release = hold("pay");
      const user = renderPage();

      const dialog = await review(user);
      await findBalanceCard();
      await user.click(within(dialog).getByRole("button", { name: "Confirm" }));
      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
        expect(requestsOf("POST")).toHaveLength(2);
      });
      await change(user);
      release(reply);

      // The payment's outcome is still shown.
      await waitFor(() => {
        expect(toast[kind]).toHaveBeenCalledWith(message);
      });
      await nextTask();
      expect(await bodiesOf(requestsOf("POST"))).toEqual([REVIEW_BODY, PAY_BODY]);
      // The newer entry is neither blanked nor marked.
      const input = accountInput();
      expect(input.value).toBe(NEWER_ACCOUNT);
      expect(input.getAttribute("aria-invalid")).toBe("false");
      expect(fieldErrorOf(input)).toBeNull();
      expect(document.activeElement).toBe(input);
      // A card still shown names the paid account, not the newer entry.
      const shown = screen.queryAllByRole("group", { name: BALANCE_TITLE });
      expect(shown).toHaveLength(cards);
      for (const card of shown) {
        expect(within(card).getByText(balance)).toBeTruthy();
        expect(within(card).getByText(`Account ${ACCOUNT_ID}`)).toBeTruthy();
      }
    },
  );

  it.each(LATE_PAYMENT_ANSWERS)(
    "keeps an account typed while the review ran when the reviewed account's $outcome arrives",
    async ({ reply, toast: kind, message, balance }) => {
      const releaseReview = hold("review");
      stubs.pay = reply;
      const user = renderPage();

      await user.type(accountInput(), `${ACCOUNT_ID}{Enter}`);
      await waitFor(() => {
        expect(requestsOf("POST")).toHaveLength(1);
      });
      await user.clear(accountInput());
      await user.type(accountInput(), NEWER_ACCOUNT);
      releaseReview([200, reviewed]);

      // The dialog asks about the reviewed account, which Confirm pays.
      const dialog = await screen.findByRole("alertdialog", { name: DIALOG_TITLE });
      expect(within(dialog).getByText(ACCOUNT_ID)).toBeTruthy();
      await findBalanceCard();
      await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

      await waitFor(() => {
        expect(toast[kind]).toHaveBeenCalledWith(message);
        expect(screen.queryByRole("alertdialog")).toBeNull();
      });
      await nextTask();
      expect(await bodiesOf(requestsOf("POST"))).toEqual([REVIEW_BODY, PAY_BODY]);
      // The entry typed during the review is neither blanked nor marked.
      const input = accountInput();
      expect(input.value).toBe(NEWER_ACCOUNT);
      expect(input.getAttribute("aria-invalid")).toBe("false");
      expect(fieldErrorOf(input)).toBeNull();
      expect(document.activeElement).toBe(input);
      const card = screen.getByRole("group", { name: BALANCE_TITLE });
      expect(within(card).getByText(balance)).toBeTruthy();
      expect(within(card).getByText(`Account ${ACCOUNT_ID}`)).toBeTruthy();
    },
  );

  it.each(LATE_PAYMENT_ANSWERS)(
    "marks the submit, Reset and the balance busy while the payment runs until its $outcome settles (F-COBIL00-002)",
    async ({ reply, toast: kind, message, balance }) => {
      const release = hold("pay");
      const user = renderPage();

      const dialog = await review(user);
      const card = await findBalanceCard();
      expect(balanceCuesOf(card)).toEqual({
        busy: "false",
        status: "",
        dimmed: false,
        spinning: false,
      });
      await user.click(within(dialog).getByRole("button", { name: "Confirm" }));
      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
        expect(requestsOf("POST")).toHaveLength(2);
      });

      const submit = screen.getByRole<HTMLButtonElement>("button", { name: "Review payment" });
      const reset = screen.getByRole<HTMLButtonElement>("button", { name: "Reset" });
      await waitFor(() => {
        expect(submit.getAttribute("aria-busy")).toBe("true");
      });
      expect(submit.getAttribute("aria-disabled")).toBe("true");
      expect(submit.disabled).toBe(false);
      // The write is in flight, so the screen cannot be cleared under it.
      expect(reset.disabled).toBe(true);
      // The balance on file, which the payment may change, is shown busy.
      const paying = screen.getByRole("group", { name: BALANCE_TITLE });
      expect(within(paying).getByText(account.acct_curr_bal)).toBeTruthy();
      expect(balanceCuesOf(paying)).toEqual({
        busy: "true",
        status: BALANCE_UPDATING,
        dimmed: true,
        spinning: true,
      });

      release(reply);

      await waitFor(() => {
        expect(toast[kind]).toHaveBeenCalledWith(message);
      });
      // Once the payment and the reload after it have settled: the 201's
      // balance, or the reloaded one after a refusal (E-19), and nothing busy.
      await waitFor(() => {
        const settled = screen.getByRole("group", { name: BALANCE_TITLE });
        expect(within(settled).getByText(balance)).toBeTruthy();
        expect(balanceCuesOf(settled)).toEqual({
          busy: "false",
          status: "",
          dimmed: false,
          spinning: false,
        });
        expect(submit.getAttribute("aria-busy")).toBe("false");
        expect(submit.getAttribute("aria-disabled")).toBe("false");
        expect(reset.disabled).toBe(false);
      });
      expect(await bodiesOf(requestsOf("POST"))).toEqual([REVIEW_BODY, PAY_BODY]);
    },
  );

  it("keeps an account typed while the payment runs unmarked when the balance reload fails", async () => {
    // V-COACTVWC-005 at ACCTSID, as the reload after the payment answers it.
    const reloadFailure =
      "Account:00000000001 not found in Acct Master file.Resp:000000013  Reas:0000";
    const release = hold("pay");
    const user = renderPage();

    const dialog = await review(user);
    await findBalanceCard();
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));
    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(requestsOf("POST")).toHaveLength(2);
    });
    await user.clear(accountInput());
    await user.type(accountInput(), NEWER_ACCOUNT);
    stubs.account = [
      404,
      {
        code: "NOT_FOUND",
        message: reloadFailure,
        fields: [{ field: "ACCTSID", message: reloadFailure }],
      },
    ];
    release([201, paid]);

    // The reload's failure is still toasted, after the card's first read.
    await waitFor(() => {
      expect(toast.success).toHaveBeenCalledWith(PAID_MESSAGE);
      expect(toast.error).toHaveBeenCalledWith(reloadFailure);
    });
    await nextTask();
    expect(requestsOf("GET").map(pathnameOf)).toEqual([ACCOUNT_PATH, ACCOUNT_PATH]);
    const input = accountInput();
    expect(input.value).toBe(NEWER_ACCOUNT);
    expect(input.getAttribute("aria-invalid")).toBe("false");
    expect(fieldErrorOf(input)).toBeNull();
    expect(document.activeElement).toBe(input);
    const card = screen.getByRole("group", { name: BALANCE_TITLE });
    expect(within(card).getByText(paid.current_balance)).toBeTruthy();
  });

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
    expect(screen.getByRole("heading", { level: 2, name: "Account" })).toBeTruthy();
    await user.click(screen.getByRole("link", { name: "Back" }));

    expect(await screen.findByText("Home")).toBeTruthy();
    expect(screen.queryByLabelText(ACCOUNT_LABEL)).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

