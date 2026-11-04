// FN tests of the CAUP account update page (pages/accounts/AccountEditPage.tsx,
// COACTUPC on map COACTUP): the form is filled from `GET /accounts/{id}` as
// 3202-SHOW-ORIGINAL-VALUES fills the map; "Save" (F-COACTUP-002,
// F-COACTUP-005) sends the COACTUP fields as split on the map with the
// record's `version` token; a value the form schema rejects is never sent; a
// server rejection marks its BMS fields and puts the cursor on the first; a
// 409 'Record changed by some one else. Please review' reloads the record,
// keeps the message and the cursor on ACCTSID, and the next save echoes the
// reloaded token (AAP 0.3.7); and "Cancel" (F-COACTUP-006) re-reads the
// record and drops unsaved entries, Save acting only once that read is done.
//
// The network is the stubbed global the API client looks up on every request,
// answering by method: GET with the test's records, PUT with its answers.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { components } from "@/api/schema";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";

import AccountEditPage from "./AccountEditPage";

type AccountResponse = components["schemas"]["AccountResponse"];
type AccountUpdateRequest = components["schemas"]["AccountUpdateRequest"];
type AccountUpdateResponse = components["schemas"]["AccountUpdateResponse"];
type ErrorBody = components["schemas"]["ErrorBody"];

const ACCOUNT_ID = "00000000001";

/** Where the client sends `/accounts/{id}`: under the Vite proxy prefix. */
const ACCOUNT_PATH = `/api/accounts/${ACCOUNT_ID}`;

/** The token of the record as first read. */
const VERSION = "0123456789abcdef";

/** The token of the record after a save, or after another writer's change. */
const NEXT_VERSION = "fedcba9876543210";

/** The token of a record saved after Cancel re-read it at NEXT_VERSION. */
const SAVED_VERSION = "00112233445566ff";

/**
 * Account 00000000001 as `GET /accounts/{id}` returns it from the seeded
 * fixtures: the account [app/data/ASCII/acctdata.txt:L1], its customer
 * 000000001 [app/data/ASCII/custdata.txt:L1] and card 9680294154603697
 * [app/data/ASCII/cardxref.txt:L49], as in
 * crates/api/tests/snapshots/cavw_account_view__account_00000000001.snap.
 */
const ACCOUNT: AccountResponse = {
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
  version: VERSION,
};

/**
 * The `PUT /accounts/{id}` body of ACCOUNT unchanged: each value split as
 * COACTUPC moves it to the map, the `X(10)` dates by `(1:4)`, `(6:2)` and
 * `(9:2)`, the SSN by `(1:3)`, `(4:2)` and `(6:4)`, each `(xxx)xxx-xxxx`
 * phone by `(2:3)`, `(6:3)` and `(10:4)` [app/cbl/COACTUPC.cbl:L2826-2851],
 * with the token of the record shown and without the account number, which
 * is the `{id}` path segment. Written out rather than derived, so the test
 * does not repeat the page's own splitting.
 */
const UNCHANGED_BODY: AccountUpdateRequest = {
  version: VERSION,
  acct_active_status: "Y",
  acct_open_year: "2014",
  acct_open_mon: "11",
  acct_open_day: "20",
  acct_credit_limit: "2020.00",
  acct_exp_year: "2025",
  acct_exp_mon: "05",
  acct_exp_day: "20",
  acct_cash_credit_limit: "1020.00",
  acct_reissue_year: "2025",
  acct_reissue_mon: "05",
  acct_reissue_day: "20",
  acct_curr_bal: "194.00",
  acct_curr_cyc_credit: "0.00",
  acct_group_id: "",
  acct_curr_cyc_debit: "0.00",
  customer_id: "000000001",
  cust_ssn_1: "020",
  cust_ssn_2: "97",
  cust_ssn_3: "3888",
  cust_dob_year: "1961",
  cust_dob_mon: "06",
  cust_dob_day: "08",
  cust_fico_credit_score: "274",
  cust_first_name: "Immanuel",
  cust_middle_name: "Madeline",
  cust_last_name: "Kessler",
  cust_addr_line_1: "618 Deshaun Route",
  cust_addr_state_cd: "NC",
  cust_addr_line_2: "Apt. 802",
  cust_addr_zip: "12546",
  cust_addr_line_3: "Altenwerthshire",
  cust_addr_country_cd: "USA",
  cust_phone_num_1a: "908",
  cust_phone_num_1b: "119",
  cust_phone_num_1c: "8310",
  cust_govt_issued_id: "00000000000049368437",
  cust_phone_num_2a: "373",
  cust_phone_num_2b: "693",
  cust_phone_num_2c: "8684",
  cust_eft_account_id: "0053581756",
  cust_pri_card_holder_ind: "Y",
};

/** The first name the tests enter: alphabetic, so the form schema passes it. */
const NEW_FIRST_NAME = "Imanuel";

/** P-COACTUPC-015: the text after both rewrites. */
const CHANGES_COMMITTED = "Changes committed to database";

/** V-COACTUPC-010: the text when every value equals the fetched one. */
const NO_CHANGE_DETECTED = "No change detected with respect to values fetched.";

/** V-COACTUPC-012: the schema's text for a status other than `Y` or `N`. */
const STATUS_NOT_YES_NO = "Account Status must be Y or N.";

/** V-COACTUPC-081: FICO 274 of customer 000000001 is outside 300 to 850. */
const FICO_OUT_OF_RANGE = "FICO Score: should be between 300 and 850";

/** V-COACTUPC-111: area code 373 is no general purpose area code. */
const PHONE_2_AREA_CODE_NOT_VALID =
  "Phone Number 2: Not valid North America general purpose area code";

/** V-COACTUPC-123: `NC` with a zip starting `12` is no state and zip pair. */
const INVALID_ZIP_FOR_STATE = "Invalid zip code for state";

/** V-COACTUPC-126: the record changed between the read and the save. */
const RECORD_CHANGED = "Record changed by some one else. Please review";

/**
 * The server's answer to ACCOUNT with only a new first name: the fixture's
 * own values fail three server-only edits, the lowest Order supplies the
 * message, and `fields` lists every flagged field in Order, the state and zip
 * pair flagging both (crates/domain/src/rules/account_update.rs).
 */
const FIXTURE_EDIT_ERRORS: ErrorBody = {
  code: "INVALID",
  message: FICO_OUT_OF_RANGE,
  fields: [
    { field: "ACSTFCO", message: FICO_OUT_OF_RANGE },
    { field: "ACSPH2A", message: PHONE_2_AREA_CODE_NOT_VALID },
    { field: "ACSSTTE", message: INVALID_ZIP_FOR_STATE },
    { field: "ACSZIPC", message: INVALID_ZIP_FOR_STATE },
  ],
};

/** The 409 of 9700-CHECK-CHANGE-IN-REC, the cursor on the account number. */
const RECORD_CHANGED_ERROR: ErrorBody = {
  code: "CONFLICT",
  message: RECORD_CHANGED,
  fields: [{ field: "ACCTSID", message: RECORD_CHANGED }],
};

/**
 * ACCOUNT after another writer raised the credit limit by 1 and renamed the
 * customer between the read and the save, as
 * crates/api/tests/caup_account_update.rs does (P-COACTUPC-012).
 */
const CHANGED_ELSEWHERE: AccountResponse = {
  ...ACCOUNT,
  acct_credit_limit: "2021.00",
  cust_first_name: "Elsewhere",
  version: NEXT_VERSION,
};

/** A GET answer; a promise holds the GET until the test settles it. */
type Read = AccountResponse | Promise<AccountResponse>;

/** The answers of one test: GETs take `reads` in turn, the last repeating. */
type Network = {
  reads: readonly [Read, ...Read[]];
  writes?: readonly Response[];
};

function stubNetwork({ reads, writes = [] }: Network) {
  let read = 0;
  let write = 0;
  const network = vi.fn(async (request: Request): Promise<Response> => {
    if (request.method === "GET") {
      const account = reads[Math.min(read, reads.length - 1)];
      read += 1;
      return Response.json(await account);
    }
    if (request.method === "PUT") {
      const answer = writes[write];
      write += 1;
      if (answer === undefined) {
        throw new Error(`PUT ${write} has no answer`);
      }
      return answer;
    }
    throw new Error(`unexpected ${request.method} ${request.url}`);
  });
  vi.stubGlobal("fetch", network);
  return {
    /** The requests sent with `method`, in order. */
    sent(method: "GET" | "PUT"): Request[] {
      return network.mock.calls
        .map(([request]) => request)
        .filter((request) => request.method === method);
    },
  };
}

function saved(account: AccountResponse, message: string): Response {
  const body: AccountUpdateResponse = { account, message };
  return Response.json(body);
}

function rejected(status: number, body: ErrorBody): Response {
  return Response.json(body, { status });
}

function pathOf(request: Request): string {
  return new URL(request.url).pathname;
}

/** The path and parsed JSON body of a sent PUT. */
async function contentOf(
  request: Request,
): Promise<{ path: string; body: unknown }> {
  const body: unknown = await request.clone().json();
  return { path: pathOf(request), body };
}

/**
 * Renders the page at /accounts/00000000001/edit beside the account view it
 * links back to, and waits for the record to fill the form.
 */
async function renderPage() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const router = createMemoryRouter(
    [
      {
        path: "/accounts/:accountId/edit",
        element: <AccountEditPage />,
        handle: { crumb: "Update" },
      },
      {
        path: "/accounts/:accountId",
        element: <div>view</div>,
        handle: { crumb: "Account" },
      },
    ],
    { initialEntries: [`/accounts/${ACCOUNT_ID}/edit`] },
  );
  const user = userEvent.setup();
  render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
        <Toaster />
      </QueryClientProvider>
    </ThemeProvider>,
  );
  await screen.findByDisplayValue("Kessler");
  return user;
}

function input(name: string): HTMLInputElement {
  const element = screen.getByRole("textbox", { name });
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`"${name}" is not an input`);
  }
  return element;
}

/**
 * The field error an input is described by, the FieldError beneath it
 * (`role="alert"`), or null while the input has none.
 */
function errorOf(field: HTMLInputElement): string | null {
  const id = field.getAttribute("aria-describedby");
  if (id === null) {
    return null;
  }
  const message = document.getElementById(id);
  expect(message?.getAttribute("role")).toBe("alert");
  return message?.textContent ?? null;
}

/**
 * Lets every effect, microtask and zero-delay timer scheduled so far run,
 * among them TanStack Query's notification of the page about new data.
 */
async function settle(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

async function replace(
  user: ReturnType<typeof userEvent.setup>,
  name: string,
  value: string,
): Promise<void> {
  const field = input(name);
  await user.clear(field);
  await user.type(field, value);
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("AccountEditPage (CAUP, COACTUPC)", () => {
  it("shows the record split into the map's fields, with the protected ones read-only", async () => {
    stubNetwork({ reads: [ACCOUNT] });
    await renderPage();

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "Update Account",
    );
    expect(input("Account Number").value).toBe(ACCOUNT_ID);
    expect(input("Opened Year").value).toBe("2014");
    expect(input("Opened Month").value).toBe("11");
    expect(input("Opened Day").value).toBe("20");
    expect(input("SSN First 3").value).toBe("020");
    expect(input("SSN 4th & 5th").value).toBe("97");
    expect(input("SSN Last 4").value).toBe("3888");
    expect(input("Phone 2 Area code").value).toBe("373");
    expect(input("Phone 2 Prefix").value).toBe("693");
    expect(input("Phone 2 Line number").value).toBe("8684");
    for (const name of ["Account Number", "Customer id", "Country"]) {
      expect(input(name).readOnly).toBe(true);
    }
    expect(input("First Name").readOnly).toBe(false);
  });

  it("saves the edited fields with the record's version and shows the COBOL text", async () => {
    const success = vi.spyOn(toast, "success");
    const network = stubNetwork({
      reads: [ACCOUNT],
      writes: [
        saved(
          {
            ...ACCOUNT,
            cust_first_name: NEW_FIRST_NAME,
            version: NEXT_VERSION,
          },
          CHANGES_COMMITTED,
        ),
      ],
    });
    const user = await renderPage();

    await replace(user, "First Name", NEW_FIRST_NAME);
    await user.click(screen.getByRole("button", { name: "Save" }));

    await screen.findByText(CHANGES_COMMITTED);
    expect(success).toHaveBeenCalledWith(CHANGES_COMMITTED);
    const [put, ...morePuts] = network.sent("PUT");
    expect(morePuts).toEqual([]);
    if (put === undefined) {
      throw new Error("Save sent no PUT");
    }
    const expected: AccountUpdateRequest = {
      ...UNCHANGED_BODY,
      cust_first_name: NEW_FIRST_NAME,
    };
    expect(await contentOf(put)).toEqual({
      path: ACCOUNT_PATH,
      body: expected,
    });
  });

  it("sends nothing while the form schema rejects a value, and marks the field", async () => {
    const network = stubNetwork({ reads: [ACCOUNT] });
    const user = await renderPage();

    await replace(user, "Active Y/N", "X");
    await user.click(screen.getByRole("button", { name: "Save" }));

    const status = input("Active Y/N");
    await waitFor(() => {
      expect(errorOf(status)).toBe(STATUS_NOT_YES_NO);
    });
    expect(
      screen.getAllByRole("alert").map((alert) => alert.textContent),
    ).toEqual([STATUS_NOT_YES_NO]);
    expect(status.getAttribute("aria-invalid")).toBe("true");
    expect(document.activeElement).toBe(status);
    expect(network.sent("PUT")).toEqual([]);
    expect(network.sent("GET")).toHaveLength(1);
  });

  it("shows a server rejection on its fields and puts the cursor on the first", async () => {
    const error = vi.spyOn(toast, "error");
    const network = stubNetwork({
      reads: [ACCOUNT],
      writes: [rejected(400, FIXTURE_EDIT_ERRORS)],
    });
    const user = await renderPage();

    await replace(user, "First Name", NEW_FIRST_NAME);
    await user.click(screen.getByRole("button", { name: "Save" }));

    const fico = input("FICO Score");
    await waitFor(() => {
      expect(document.activeElement).toBe(fico);
    });
    expect(error).toHaveBeenCalledTimes(1);
    expect(error).toHaveBeenCalledWith(FICO_OUT_OF_RANGE);
    // The message line's text twice: the toast and the field's own error.
    expect(await screen.findAllByText(FICO_OUT_OF_RANGE)).toHaveLength(2);
    expect(errorOf(fico)).toBe(FICO_OUT_OF_RANGE);
    expect(errorOf(input("Phone 2 Area code"))).toBe(
      PHONE_2_AREA_CODE_NOT_VALID,
    );
    expect(errorOf(input("State"))).toBe(INVALID_ZIP_FOR_STATE);
    expect(errorOf(input("Zip"))).toBe(INVALID_ZIP_FOR_STATE);
    expect(screen.getAllByRole("alert")).toHaveLength(4);
    // The failure reloads the record; an unchanged record keeps the entries.
    expect(network.sent("GET")).toHaveLength(2);
    expect(input("First Name").value).toBe(NEW_FIRST_NAME);
  });

  it("reloads a record changed by someone else and saves it with the new version", async () => {
    const error = vi.spyOn(toast, "error");
    const success = vi.spyOn(toast, "success");
    const network = stubNetwork({
      reads: [ACCOUNT, CHANGED_ELSEWHERE],
      writes: [
        rejected(409, RECORD_CHANGED_ERROR),
        saved(CHANGED_ELSEWHERE, NO_CHANGE_DETECTED),
      ],
    });
    const user = await renderPage();

    await replace(user, "First Name", NEW_FIRST_NAME);
    await user.click(screen.getByRole("button", { name: "Save" }));

    await screen.findByDisplayValue("Elsewhere");
    await screen.findAllByText(RECORD_CHANGED);
    expect(error).toHaveBeenCalledWith(RECORD_CHANGED);
    expect(input("Credit Limit").value).toBe("2021.00");
    // The changed record is shown with the message on ACCTSID and the cursor
    // there, as COACTUPC redisplays it.
    const accountNumber = input("Account Number");
    await waitFor(() => {
      expect(errorOf(accountNumber)).toBe(RECORD_CHANGED);
      expect(document.activeElement).toBe(accountNumber);
    });
    expect(accountNumber.getAttribute("aria-invalid")).toBe("true");
    // The page hears of the reloaded record on a later timer task than the
    // failed save; the error and the cursor outlast that.
    await settle();
    expect(errorOf(accountNumber)).toBe(RECORD_CHANGED);
    expect(accountNumber.getAttribute("aria-invalid")).toBe("true");
    expect(document.activeElement).toBe(accountNumber);
    expect(network.sent("GET").map(pathOf)).toEqual([
      ACCOUNT_PATH,
      ACCOUNT_PATH,
    ]);

    await user.click(screen.getByRole("button", { name: "Save" }));

    await screen.findByText(NO_CHANGE_DETECTED);
    expect(success).toHaveBeenCalledWith(NO_CHANGE_DETECTED);
    const puts = await Promise.all(network.sent("PUT").map(contentOf));
    const resent: AccountUpdateRequest = {
      ...UNCHANGED_BODY,
      acct_credit_limit: "2021.00",
      cust_first_name: "Elsewhere",
      version: NEXT_VERSION,
    };
    expect(puts.map(({ body }) => body)).toEqual([
      { ...UNCHANGED_BODY, cust_first_name: NEW_FIRST_NAME },
      resent,
    ]);
  });

  it("drops unsaved entries on Cancel by reading the record again", async () => {
    const network = stubNetwork({ reads: [ACCOUNT] });
    const user = await renderPage();

    await replace(user, "First Name", NEW_FIRST_NAME);
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    await screen.findByDisplayValue(ACCOUNT.cust_first_name);
    expect(network.sent("GET")).toHaveLength(2);
    expect(network.sent("PUT")).toEqual([]);
  });

  it("holds Save while Cancel reads the record again, then saves the re-read version", async () => {
    const success = vi.spyOn(toast, "success");
    let release: (account: AccountResponse) => void = () => undefined;
    const reread = new Promise<AccountResponse>((resolve) => {
      release = resolve;
    });
    const savedAfterReread: AccountResponse = {
      ...CHANGED_ELSEWHERE,
      cust_first_name: NEW_FIRST_NAME,
      version: SAVED_VERSION,
    };
    const network = stubNetwork({
      reads: [ACCOUNT, reread],
      writes: [
        saved(savedAfterReread, CHANGES_COMMITTED),
        saved(savedAfterReread, NO_CHANGE_DETECTED),
      ],
    });
    const user = await renderPage();

    await replace(user, "First Name", NEW_FIRST_NAME);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => {
      expect(network.sent("GET")).toHaveLength(2);
    });

    // While the re-read is held, neither button acts, by click or by Enter.
    const save = screen.getByRole("button", { name: "Save" });
    const cancel = screen.getByRole("button", { name: "Cancel" });
    expect(save.getAttribute("aria-disabled")).toBe("true");
    expect(cancel.getAttribute("aria-disabled")).toBe("true");
    await user.click(save);
    await user.click(input("First Name"));
    await user.keyboard("{Enter}");
    await user.click(cancel);
    await settle();
    expect(network.sent("PUT")).toEqual([]);
    expect(network.sent("GET")).toHaveLength(2);

    release(CHANGED_ELSEWHERE);
    await screen.findByDisplayValue("Elsewhere");
    expect(input("Credit Limit").value).toBe("2021.00");
    await waitFor(() => {
      expect(save.getAttribute("aria-disabled")).toBe("false");
    });
    expect(cancel.getAttribute("aria-disabled")).toBe("false");
    // The page hears of the re-read record on a later timer task and resets
    // the form to the same values then, so entries wait for that.
    await settle();

    await replace(user, "First Name", NEW_FIRST_NAME);
    await user.click(save);
    await waitFor(() => {
      expect(success).toHaveBeenCalledWith(CHANGES_COMMITTED);
    });
    await settle();
    expect(input("First Name").value).toBe(NEW_FIRST_NAME);
    expect(input("Credit Limit").value).toBe("2021.00");

    // The next save echoes the saved record's token.
    await user.click(save);
    await waitFor(() => {
      expect(success).toHaveBeenCalledWith(NO_CHANGE_DETECTED);
    });
    const savedBody: AccountUpdateRequest = {
      ...UNCHANGED_BODY,
      acct_credit_limit: "2021.00",
      cust_first_name: NEW_FIRST_NAME,
      version: NEXT_VERSION,
    };
    expect(await Promise.all(network.sent("PUT").map(contentOf))).toEqual([
      { path: ACCOUNT_PATH, body: savedBody },
      { path: ACCOUNT_PATH, body: { ...savedBody, version: SAVED_VERSION } },
    ]);
    expect(network.sent("GET")).toHaveLength(2);
  });
});
