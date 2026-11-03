// FN tests of TransactionAddPage, the CT02 "Add Transaction" screen (COTRN02C,
// map COTRN02) at /transactions/new. They drive the controls of the F-COTRN02
// rows of docs/parity/function-map.md and check the texts of the V-COTRN02C
// rows of docs/parity/validation-audit.md:
// - ENTER on a valid entry opens the confirmation and sends nothing; its
//   "Confirm" (CONFIRM Y) sends one POST /transactions holding every input as
//   typed with `confirm: "Y"`, and the 201 answer shows the COTRN02C success
//   text and leaves a cleared screen, as WRITE-TRANSACT-FILE NORMAL performs
//   INITIALIZE-ALL-FIELDS;
// - a malformed amount or date never reaches the network: its audited text
//   appears on the field, which is marked invalid;
// - "Cancel" and Escape (CONFIRM N) send nothing and keep the entry;
// - a server error is toasted, set on the field its `fields[]` names and puts
//   the cursor there, as `MOVE -1 TO ACTIDINL` does;
// - "Copy last transaction" (PF5) reads the highest-keyed transaction, copies
//   its data fields with the screen edit of the amount, and keeps the keys.
//
// The network is the stubbed global the API client looks up on every request.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryRouter, RouterProvider } from "react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";

import type { components, paths } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";

import TransactionAddPage from "./TransactionAddPage";

type TransactionAddRequest = components["schemas"]["TransactionAddRequest"];
type TransactionAddResponse = components["schemas"]["TransactionAddResponse"];
type TransactionResponse = components["schemas"]["TransactionResponse"];
type TransactionAddField = components["schemas"]["TransactionAddField"];
type ErrorBody = components["schemas"]["ErrorBody"];
type TransactionPage =
  paths["/transactions"]["get"]["responses"][200]["content"]["application/json"];

/** A COTRN02 input; CONFIRM has none, the confirmation dialog replaces it. */
type InputField = Exclude<TransactionAddField, "CONFIRM">;

/** What to type into each COTRN02 input; the empty string leaves it untouched. */
type Entry = Record<InputField, string>;

/** A `TransactionAddRequest` with every member present as text. */
type AddBody = { [K in keyof TransactionAddRequest]-?: string };

// The COTRN02 inputs in screen order, with their BMS captions
// [app/bms/COTRN02.bms].
const FIELDS: readonly { bms: InputField; label: string }[] = [
  { bms: "ACTIDIN", label: "Enter Acct #" },
  { bms: "CARDNIN", label: "Card #" },
  { bms: "TTYPCD", label: "Type CD" },
  { bms: "TCATCD", label: "Category CD" },
  { bms: "TRNSRC", label: "Source" },
  { bms: "TDESC", label: "Description" },
  { bms: "TRNAMT", label: "Amount" },
  { bms: "TORIGDT", label: "Orig Date" },
  { bms: "TPROCDT", label: "Proc Date" },
  { bms: "MID", label: "Merchant ID" },
  { bms: "MNAME", label: "Merchant Name" },
  { bms: "MCITY", label: "Merchant City" },
  { bms: "MZIP", label: "Merchant Zip" },
];

// A purchase for fixture account 00000000001 [app/data/ASCII/acctdata.txt:L1],
// keyed by account alone, so the card number stays blank. The amount is the
// `-99999999.99` form COTRN02C asks for [app/cbl/COTRN02C.cbl:L339-348], the
// dates the CSUTLDTC `YYYY-MM-DD` form [app/cbl/COTRN02C.cbl:L353-378].
const VALID_ENTRY = {
  ACTIDIN: "00000000001",
  CARDNIN: "",
  TTYPCD: "01",
  TCATCD: "0001",
  TRNSRC: "POS TERM",
  TDESC: "Purchase at Example Store",
  TRNAMT: "+00000050.47",
  TORIGDT: "2022-07-01",
  TPROCDT: "2022-07-01",
  MID: "800000001",
  MNAME: "Example Store",
  MCITY: "Springfield",
  MZIP: "12345",
} as const satisfies Entry;

// The body the dialog's "Confirm" sends: every input exactly as typed, the
// blank card number included, and CONFIRMI 'Y' [app/cbl/COTRN02C.cbl:L169-172].
const EXPECTED_ADD_BODY = {
  account_id: VALID_ENTRY.ACTIDIN,
  card_number: "",
  tran_type_cd: VALID_ENTRY.TTYPCD,
  tran_cat_cd: VALID_ENTRY.TCATCD,
  tran_source: VALID_ENTRY.TRNSRC,
  tran_desc: VALID_ENTRY.TDESC,
  tran_amt: VALID_ENTRY.TRNAMT,
  tran_orig_date: VALID_ENTRY.TORIGDT,
  tran_proc_date: VALID_ENTRY.TPROCDT,
  tran_merchant_id: VALID_ENTRY.MID,
  tran_merchant_name: VALID_ENTRY.MNAME,
  tran_merchant_city: VALID_ENTRY.MCITY,
  tran_merchant_zip: VALID_ENTRY.MZIP,
  confirm: "Y",
} satisfies AddBody & TransactionAddRequest;

// WRITE-TRANSACT-FILE NORMAL: 'Transaction added successfully. ' then
// ' Your Tran ID is ' TRAN-ID '.' [app/cbl/COTRN02C.cbl:L723-731].
const ADDED_MESSAGE =
  "Transaction added successfully.  Your Tran ID is 0000000000000301.";

// The record ADD-TRANSACTION writes: the dates moved whole into the
// timestamps [app/cbl/COTRN02C.cbl:L450-466], the card that the cross-reference
// gives account 00000000001 [app/data/ASCII/cardxref.txt:L49].
const ADDED_TRANSACTION = {
  tran_id: "0000000000000301",
  tran_type_cd: "01",
  tran_cat_cd: "0001",
  tran_source: "POS TERM",
  tran_desc: "Purchase at Example Store",
  tran_amt: "50.47",
  tran_merchant_id: "800000001",
  tran_merchant_name: "Example Store",
  tran_merchant_city: "Springfield",
  tran_merchant_zip: "12345",
  tran_card_num: "9680294154603697",
  tran_orig_ts: "2022-07-01",
  tran_proc_ts: "2022-07-01",
} satisfies TransactionResponse;

const ADDED_REPLY = {
  transaction: ADDED_TRANSACTION,
  message: ADDED_MESSAGE,
} satisfies TransactionAddResponse;

// The highest-keyed transaction PF5 reads: a return (type 03) shaped like the
// OPERATOR records of app/data/ASCII/dailytran.txt, with a negative amount and
// full CVTRA05Y timestamps.
const LAST_TRANSACTION = {
  tran_id: "0000000016259484",
  tran_type_cd: "03",
  tran_cat_cd: "0001",
  tran_source: "OPERATOR",
  tran_desc: "Return item at Sipes Inc",
  tran_amt: "-91.07",
  tran_merchant_id: "800000000",
  tran_merchant_name: "Sipes Inc",
  tran_merchant_city: "Emilioside",
  tran_merchant_zip: "93329",
  tran_card_num: "4011500891777367",
  tran_orig_ts: "2022-06-10 19:27:53.000000",
  tran_proc_ts: "2022-06-10 19:27:53.000000",
} satisfies TransactionResponse;

const LAST_PAGE = {
  items: [LAST_TRANSACTION],
  next_key: null,
  prev_key: null,
} satisfies TransactionPage;

// COPY-LAST-TRAN-DATA's copy [app/cbl/COTRN02C.cbl:L479-492]: the amount
// through WS-TRAN-AMT-E `PIC +99999999.99`, each timestamp cut to its 10-byte
// date field. The key fields keep what was typed.
const COPIED_ENTRY = {
  ACTIDIN: VALID_ENTRY.ACTIDIN,
  CARDNIN: "",
  TTYPCD: "03",
  TCATCD: "0001",
  TRNSRC: "OPERATOR",
  TDESC: "Return item at Sipes Inc",
  TRNAMT: "-00000091.07",
  TORIGDT: "2022-06-10",
  TPROCDT: "2022-06-10",
  MID: "800000000",
  MNAME: "Sipes Inc",
  MCITY: "Emilioside",
  MZIP: "93329",
} as const satisfies Entry;

// V-COTRN02C-027: every edit passed and CONFIRMI blank, so PROCESS-ENTER-KEY
// asks for the confirmation and writes nothing (400, cursor on CONFIRM).
const CONFIRM_PROMPT = "Confirm to add this transaction...";

const ADD_TRANSACTION_HANDLE = {
  title: "Add Transaction",
  crumb: "Add Transaction",
} satisfies RouteHandle;

type FetchFn = typeof globalThis.fetch;

type Answer = (request: Request) => Response;

// The answers of the stubbed network, keyed by method and pathname.
let answers: Map<string, Answer>;
// Every request the page sent, in order.
let sent: Request[];
let fetchMock: Mock<FetchFn>;
let queryClient: QueryClient;

function answerWith(
  method: "GET" | "POST",
  body: TransactionAddResponse | TransactionPage | ErrorBody,
  status: number,
): void {
  answers.set(`${method} /api/transactions`, () => Response.json(body, { status }));
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
        path: "/transactions/new",
        element: <TransactionAddPage />,
        handle: ADD_TRANSACTION_HANDLE,
      },
      {
        path: "/",
        element: <p>home</p>,
        handle: { crumb: "Home" } satisfies RouteHandle,
      },
    ],
    { initialEntries: ["/transactions/new"] },
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

function inputOf(bms: InputField): HTMLInputElement {
  const field = FIELDS.find((entry) => entry.bms === bms);
  if (field === undefined) {
    throw new Error(`No COTRN02 input ${bms}`);
  }
  return screen.getByRole<HTMLInputElement>("textbox", { name: field.label });
}

// Types every non-empty entry into its input, in screen order.
async function fill(user: UserEvent, entry: Entry): Promise<void> {
  for (const { bms } of FIELDS) {
    const value = entry[bms];
    if (value !== "") {
      await user.type(inputOf(bms), value);
    }
  }
}

function expectEntry(entry: Entry): void {
  for (const { bms } of FIELDS) {
    expect(inputOf(bms).value, bms).toBe(entry[bms]);
  }
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

function pathnameOf(request: Request | undefined): string {
  return new URL(request?.url ?? "").pathname;
}

// ENTER (F-COTRN02-001) from the last input typed, which still has the focus.
async function pressEnter(user: UserEvent): Promise<void> {
  await user.keyboard("{Enter}");
}

async function openConfirmation(user: UserEvent): Promise<HTMLElement> {
  await fill(user, VALID_ENTRY);
  await pressEnter(user);
  return screen.findByRole("alertdialog", { name: "Confirm the transaction" });
}

beforeEach(() => {
  answers = new Map();
  sent = [];
  // The client passes a Request; anything else is normalised to one.
  fetchMock = vi.fn<FetchFn>(async (input, init) => {
    const request = input instanceof Request ? input : new Request(input, init);
    sent.push(request);
    const key = `${request.method} ${new URL(request.url).pathname}`;
    const answer = answers.get(key);
    if (answer === undefined) {
      throw new Error(`No answer stubbed for ${key}`);
    }
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

describe("TransactionAddPage (CT02, COTRN02)", () => {
  it(
    "confirms on Enter, adds on Confirm and clears the screen (F-COTRN02-001, F-COTRN02-002)",
    async () => {
      answerWith("POST", ADDED_REPLY, 201);
      const user = renderPage();

      const dialog = await openConfirmation(user);

      // The confirmation replaces the CONFIRM field: nothing is sent before it.
      expect(fetchMock).not.toHaveBeenCalled();
      expect(within(dialog).getByText(VALID_ENTRY.ACTIDIN)).toBeTruthy();
      expect(within(dialog).getByText(VALID_ENTRY.TRNAMT)).toBeTruthy();

      await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

      await waitFor(() => {
        expect(toast.success).toHaveBeenCalledWith(ADDED_MESSAGE);
      });
      const posts = requestsOf("POST");
      expect(posts).toHaveLength(1);
      expect(sent).toHaveLength(1);
      const [post] = posts;
      expect(pathnameOf(post)).toBe("/api/transactions");
      expect(await post?.clone().json()).toEqual(EXPECTED_ADD_BODY);

      // INITIALIZE-ALL-FIELDS: every input SPACES, the cursor on ACTIDIN.
      await waitFor(() => {
        for (const { bms } of FIELDS) {
          expect(inputOf(bms).value, bms).toBe("");
        }
        expect(document.activeElement).toBe(inputOf("ACTIDIN"));
        expect(screen.queryByRole("alertdialog")).toBeNull();
      });
      expect(toast.error).not.toHaveBeenCalled();
    },
  );

  // The format edits of VALIDATE-INPUT-DATA-FIELDS the form decides itself.
  const FORMAT_EDITS: readonly {
    row: string;
    bms: InputField;
    value: string;
    message: string;
  }[] = [
    {
      row: "V-COTRN02C-021",
      bms: "TRNAMT",
      value: "50.47",
      message: "Amount should be in format -99999999.99",
    },
    {
      row: "V-COTRN02C-022",
      bms: "TORIGDT",
      value: "2022/07/01",
      message: "Orig Date should be in format YYYY-MM-DD",
    },
  ];

  it.each(FORMAT_EDITS)(
    "keeps a malformed $bms off the network and shows its text ($row)",
    async ({ bms, value, message }) => {
      const user = renderPage();

      await fill(user, { ...VALID_ENTRY, [bms]: value });
      await pressEnter(user);

      const input = inputOf(bms);
      await waitFor(() => {
        expect(input.getAttribute("aria-invalid")).toBe("true");
        expect(document.activeElement).toBe(input);
      });
      expect(fieldErrorOf(input)?.textContent).toBe(message);
      for (const other of FIELDS.filter((field) => field.bms !== bms)) {
        expect(inputOf(other.bms).getAttribute("aria-invalid"), other.bms).toBe("false");
      }
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  // CONFIRM N: no write, and the screen keeps what was entered
  // [app/cbl/COTRN02C.cbl:L173-181].
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
    "sends nothing and keeps the entry when $how closes the confirmation (F-COTRN02-003)",
    async ({ dismiss }) => {
      const user = renderPage();

      const dialog = await openConfirmation(user);
      await dismiss(user, dialog);

      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
      });
      expect(fetchMock).not.toHaveBeenCalled();
      expectEntry(VALID_ENTRY);
      expect(toast.success).not.toHaveBeenCalled();
      expect(toast.error).not.toHaveBeenCalled();
    },
  );

  // The ACTIDIN outcomes only the server decides, each with the status its
  // audit row assigns. 'Account ID NOT found...' is a 404 there, not a 400:
  // the audit's status mapping outranks the 400 the test plan named for it.
  const SERVER_ERRORS: readonly {
    row: string;
    status: number;
    code: string;
    message: string;
  }[] = [
    {
      row: "V-COTRN02C-004",
      status: 404,
      code: "NOT_FOUND",
      message: "Account ID NOT found...",
    },
    {
      row: "V-COTRN02C-032",
      status: 409,
      code: "DUPLICATE",
      message: "Tran ID already exist...",
    },
    {
      row: "V-COTRN02C-033",
      status: 500,
      code: "UNEXPECTED",
      message: "Unable to Add Transaction...",
    },
  ];

  it.each(SERVER_ERRORS)(
    "toasts a $status ACTIDIN answer, marks the account and moves the cursor there ($row)",
    async ({ status, code, message }) => {
      answerWith(
        "POST",
        { code, message, fields: [{ field: "ACTIDIN", message }] } satisfies ErrorBody,
        status,
      );
      const user = renderPage();

      const dialog = await openConfirmation(user);
      await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

      await waitFor(() => {
        expect(toast.error).toHaveBeenCalledWith(message);
      });
      const input = inputOf("ACTIDIN");
      await waitFor(() => {
        expect(input.getAttribute("aria-invalid")).toBe("true");
        expect(document.activeElement).toBe(input);
      });
      expect(fieldErrorOf(input)?.textContent).toBe(message);
      expect(requestsOf("POST")).toHaveLength(1);
      expect(await requestsOf("POST")[0]?.clone().json()).toEqual(EXPECTED_ADD_BODY);
      // SEND-TRNADD-SCREEN on an error keeps what was entered.
      expectEntry(VALID_ENTRY);
      expect(toast.success).not.toHaveBeenCalled();
    },
  );

  it(
    "copies the last transaction into the data fields and keeps the keys (F-COTRN02-006)",
    async () => {
      answerWith("GET", LAST_PAGE, 200);
      answerWith(
        "POST",
        {
          code: "INVALID",
          message: CONFIRM_PROMPT,
          fields: [{ field: "CONFIRM", message: CONFIRM_PROMPT }],
        } satisfies ErrorBody,
        400,
      );
      const user = renderPage();

      await user.type(inputOf("ACTIDIN"), VALID_ENTRY.ACTIDIN);
      await user.click(screen.getByRole("button", { name: "Copy last transaction" }));

      await waitFor(() => {
        expectEntry(COPIED_ENTRY);
      });

      // STARTBR at HIGH-VALUES and READPREV: one row read backward from the end
      // of the whole file, whatever the entered account.
      expect(sent.map((request) => request.method)).toEqual(["GET", "POST"]);
      const [read] = requestsOf("GET");
      expect(pathnameOf(read)).toBe("/api/transactions");
      const query = new URL(read?.url ?? "").searchParams;
      expect(query.get("direction")).toBe("backward");
      expect(query.get("page_size")).toBe("1");
      expect(query.has("account_id")).toBe(false);

      // PROCESS-ENTER-KEY on the copy with CONFIRMI blank: checked, not written.
      const [check] = requestsOf("POST");
      expect(await check?.clone().json()).toEqual({
        ...EXPECTED_ADD_BODY,
        tran_type_cd: COPIED_ENTRY.TTYPCD,
        tran_cat_cd: COPIED_ENTRY.TCATCD,
        tran_source: COPIED_ENTRY.TRNSRC,
        tran_desc: COPIED_ENTRY.TDESC,
        tran_amt: COPIED_ENTRY.TRNAMT,
        tran_orig_date: COPIED_ENTRY.TORIGDT,
        tran_proc_date: COPIED_ENTRY.TPROCDT,
        tran_merchant_id: COPIED_ENTRY.MID,
        tran_merchant_name: COPIED_ENTRY.MNAME,
        tran_merchant_city: COPIED_ENTRY.MCITY,
        tran_merchant_zip: COPIED_ENTRY.MZIP,
        confirm: "",
      } satisfies AddBody);

      // The answer asks for the confirmation: its text is shown, and the cursor
      // goes where CONFIRM was, the submit that opens the dialog.
      await waitFor(() => {
        expect(toast.error).toHaveBeenCalledWith(CONFIRM_PROMPT);
        expect(document.activeElement).toBe(
          screen.getByRole("button", { name: "Add transaction" }),
        );
      });
      for (const { bms } of FIELDS) {
        expect(inputOf(bms).getAttribute("aria-invalid"), bms).toBe("false");
      }
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(toast.success).not.toHaveBeenCalled();
    },
  );
});
