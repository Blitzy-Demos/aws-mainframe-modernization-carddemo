import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, configure, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import {
  createMemoryRouter,
  MemoryRouter,
  Route,
  RouterProvider,
  Routes,
  useLocation,
} from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { REQUEST_DEADLINE_MS } from "@/api/client";
import type { components } from "@/api/schema";
import { getSession, setSession } from "@/auth/session";
import { PATHS, routes } from "@/routes";
import { COACTUP } from "./COACTUP";

/*
 * Render test of map COACTUP (`CACTUPA`), transaction CAUP, program COACTUPC, route
 * `/accounts/update`, in jsdom with a stubbed `fetch` (Coverage Floors). It proves the `client`
 * rows of `modern/audit/screens.md` (COACTUP-nnn) and `modern/audit/online/COACTUPC.md`
 * (COACTUPC-nnn) that a render shows: the map's literals and their placement, the key row and its
 * dark keys, the message rows, the keys that send no request (F3, F12, the restart after a
 * finished save) and the requests ENTER and F5 build. `online/CSUTLDTC.md` has no `client` row.
 *
 * Not tagged here: COACTUP-001 to COACTUP-010 describe the header literals and values as `Label`
 * and read-only `Input` elements, while `ScreenShell` renders them as plain text; their content
 * rows (COACTUP-129 to COACTUP-134) and the absence of the unnamed (1,1) field (COACTUP-124) are
 * proved instead.
 *
 * Every value is synthetic. Responses carry only the redacted SSN `***-**-1234`; the SSN the
 * operator re-enters exists only as request input (Cardholder Data Redaction, E32).
 */

/*
 * Each flow re-renders the 43-field form after every typed value and every answer. Alone a test
 * takes under a second, but under coverage on a host shared with other suites it took up to 13 s
 * and its steps several seconds each, past Vitest's 5 s test and Testing Library's 1 s wait
 * defaults. The wider budgets change no assertion: a condition that never holds still fails.
 */
const TEST_TIMEOUT_MS = 30_000;
configure({ asyncUtilTimeout: 10_000 });

/** The account the tests fetch, and the path of endpoints 5 and 6 for it. */
const ACCOUNT = "00000000050";
const ACCOUNT_PATH = `/api/v1/accounts/${ACCOUNT}`;

/** Endpoint 5's answer for the account: the shown values and the snapshot every save echoes. */
const VIEW: components["schemas"]["AccountUpdateView"] = {
  account_id: ACCOUNT,
  active_status: "Y",
  open_date: "2014-11-20",
  expiry_date: "2026-03-14",
  reissue_date: "2024-03-14",
  credit_limit: "+      2,500.00",
  cash_credit_limit: "+        800.00",
  current_balance: "+        613.45",
  current_cycle_credit: "+           .00",
  current_cycle_debit: "+         25.10",
  group_id: "",
  customer_id: "000000050",
  ssn: "***-**-1234",
  dob: "1985-07-04",
  fico_score: "712",
  first_name: "Ada",
  middle_name: "Byron",
  last_name: "Lovelace",
  address_line_1: "12 Synthetic Street",
  address_line_2: "Suite 4",
  city: "Springfield",
  state: "IL",
  zip: "62701",
  country: "USA",
  phone_1_area: "217",
  phone_1_prefix: "555",
  phone_1_line: "0142",
  phone_2_area: "217",
  phone_2_prefix: "555",
  phone_2_line: "0199",
  government_id: "SYN-GOV-0050",
  eft_account_id: "0000005050",
  primary_card_holder: "Y",
  info_message: "Update account details presented above.",
  snapshot: {
    account_id: ACCOUNT,
    active_status: "Y",
    open_date: "2014-11-20",
    expiry_date: "2026-03-14",
    reissue_date: "2024-03-14",
    credit_limit: "2500.00",
    cash_credit_limit: "800.00",
    current_balance: "613.45",
    current_cycle_credit: "0.00",
    current_cycle_debit: "25.10",
    group_id: "",
    customer_id: "000000050",
    ssn_last4: "1234",
    dob: "1985-07-04",
    fico_score: "712",
    first_name: "Ada",
    middle_name: "Byron",
    last_name: "Lovelace",
    address_line_1: "12 Synthetic Street",
    address_line_2: "Suite 4",
    city: "Springfield",
    state_code: "IL",
    zip: "62701",
    country: "USA",
    phone_1: "(217)555-0142",
    phone_2: "(217)555-0199",
    government_id: "SYN-GOV-0050",
    eft_account_id: "0000005050",
    primary_card_holder: "Y",
    state: "S",
  },
};

/** `fields` of an endpoint 6 request for the fetched values unchanged: raw text, no SSN parts. */
const FETCHED_FIELDS: components["schemas"]["AccountUpdateFields"] = {
  active_status: "Y",
  open_year: "2014",
  open_month: "11",
  open_day: "20",
  credit_limit: "+      2,500.00",
  expiry_year: "2026",
  expiry_month: "03",
  expiry_day: "14",
  cash_credit_limit: "+        800.00",
  reissue_year: "2024",
  reissue_month: "03",
  reissue_day: "14",
  current_balance: "+        613.45",
  current_cycle_credit: "+           .00",
  group_id: "",
  current_cycle_debit: "+         25.10",
  customer_id: "000000050",
  ssn_part1: null,
  ssn_part2: null,
  ssn_part3: null,
  dob_year: "1985",
  dob_month: "07",
  dob_day: "04",
  fico_score: "712",
  first_name: "Ada",
  middle_name: "Byron",
  last_name: "Lovelace",
  address_line_1: "12 Synthetic Street",
  state: "IL",
  address_line_2: "Suite 4",
  zip: "62701",
  city: "Springfield",
  country: "USA",
  phone_1_area: "217",
  phone_1_prefix: "555",
  phone_1_line: "0142",
  government_id: "SYN-GOV-0050",
  phone_2_area: "217",
  phone_2_prefix: "555",
  phone_2_line: "0199",
  eft_account_id: "0000005050",
  primary_card_holder: "Y",
};

/** Endpoint 6's answer to ENTER when the changes pass: state `N`, both dark keys shown. */
const VALIDATED: components["schemas"]["AccountUpdateResult"] = {
  state: "N",
  message: "Changes validated.Press F5 to save",
  committed: false,
  save_enabled: true,
  cancel_enabled: true,
  amounts: {
    credit_limit: "+      2,500.00",
    cash_credit_limit: "+        800.00",
    current_balance: "+        613.45",
    current_cycle_credit: "+           .00",
    current_cycle_debit: "+         25.10",
  },
};

/** As `VALIDATED`, for a credit limit entered as `3000.00` and shown as the program formats it. */
const VALIDATED_NEW_LIMIT: components["schemas"]["AccountUpdateResult"] = {
  state: "N",
  message: "Changes validated.Press F5 to save",
  committed: false,
  save_enabled: true,
  cancel_enabled: true,
  amounts: {
    credit_limit: "+      3,000.00",
    cash_credit_limit: "+        800.00",
    current_balance: "+        613.45",
    current_cycle_credit: "+           .00",
    current_cycle_debit: "+         25.10",
  },
};

/** Endpoint 6's answer to F5 when both rewrites succeed: state `C`. */
const COMMITTED: components["schemas"]["AccountUpdateResult"] = {
  state: "C",
  message: "Changes committed to database",
  committed: true,
  save_enabled: false,
  cancel_enabled: false,
  amounts: {
    credit_limit: "+      2,500.00",
    cash_credit_limit: "+        800.00",
    current_balance: "+        613.45",
    current_cycle_credit: "+           .00",
    current_cycle_debit: "+         25.10",
  },
};

/** COACTUPC-045: endpoint 5 with a blank account number. */
const NO_INPUT: components["schemas"]["ScreenError"] = {
  message: "No input received",
  field: "acct_id",
};

/** COACTUPC-027: the first name edit on a blank first name. */
const FIRST_NAME_BLANK: components["schemas"]["ScreenError"] = {
  message: "First Name must be supplied.",
  field: "first_name",
};

/** COACTUPC-043: ENTER with every field as fetched. */
const NO_CHANGE: components["schemas"]["ScreenError"] = {
  message: "No change detected with respect to values fetched.",
  field: "active_status",
};

/** COACTUPC-113: the 409 of a record another user changed since the fetch. */
const RECORD_CHANGED: components["schemas"]["ScreenError"] = {
  message: "Record changed by some one else. Please review",
};

/** COACTUPC-101: the account lock of the save failed. */
const ACCOUNT_LOCK_FAILED: components["schemas"]["ScreenError"] = {
  message: "Could not lock account record for update",
};

/** COACTUPC-106: the account rewrite of the save failed. */
const REWRITE_FAILED: components["schemas"]["ScreenError"] = {
  message: "Update of record failed",
};

/** COACTUPC-102: the customer lock of the save failed. */
const CUSTOMER_LOCK_FAILED: components["schemas"]["ScreenError"] = {
  message: "Could not lock customer record for update",
};

/** COACTUPC-095: endpoint 6's re-read found no customer; the CICS codes are placeholders (H4). */
const CUSTOMER_NOT_FOUND: components["schemas"]["ScreenError"] = {
  message: "CustId:000000050 not found in customer master.Resp: 0000000013 REAS:0000080",
};

/** The info rows of the states, as COACTUPC-079 words them. */
const INFO_NOT_FETCHED = "Enter or update id of account to update";
const INFO_DETAILS = "Update account details presented above.";

/** Every request the stubbed `fetch` received, in order. */
const sent: { method: string; path: string; authorization: string | null; body: unknown }[] = [];

/** A JSON response with `status`. */
function reply(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** An expected endpoint 5 request and its answer. */
function get(status: number, body: unknown, path = ACCOUNT_PATH) {
  return { method: "GET", path, status, body } as const;
}

/** An expected endpoint 6 request and its answer. */
function put(status: number, body: unknown) {
  return { method: "PUT", path: ACCOUNT_PATH, status, body } as const;
}

/**
 * Replaces `fetch` with a stub that records every request and answers them in order from
 * `replies`, each only when the request has its method and decoded path; any other request gets a
 * 500. No request leaves the process (Local-Only Runtime).
 */
function stubApi(
  ...replies: { method: "GET" | "PUT"; path: string; status: number; body: unknown }[]
): void {
  const pending = [...replies];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      const path = decodeURIComponent(new URL(req.url).pathname);
      const body: unknown = req.method === "GET" ? undefined : await req.clone().json();
      const authorization = req.headers.get("Authorization");
      sent.push({ method: req.method, path, authorization, body });
      const next = pending.at(0);
      if (next === undefined || next.method !== req.method || next.path !== path) {
        return reply(500, { message: `Unexpected ${req.method} ${path}` });
      }
      pending.shift();
      return reply(next.status, next.body);
    }),
  );
}

/** The stub at a transfer target: its map name and the `from` of the route state it received. */
function Arrived({ name }: { name: string }) {
  const state: unknown = useLocation().state;
  const from =
    typeof state === "object" && state !== null && "from" in state ? String(state.from) : "none";
  return <p>{`Arrived at ${name} from ${from}`}</p>;
}

/** Renders the screen at its route, with `state` as the route state the caller set. */
function renderScreen(state?: { from: string }) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COACTUP, state }]}>
        <Routes>
          <Route path={PATHS.COACTUP} element={<COACTUP />} />
          <Route path={PATHS.COMEN01} element={<Arrived name="COMEN01" />} />
          <Route path={PATHS.COACTVW} element={<Arrived name="COACTVW" />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The input whose label or `aria-label` is `name`. */
function input(name: string): HTMLInputElement {
  const element = screen.getByLabelText(name);
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`${name} labels no input`);
  }
  return element;
}

/** Types `value` into the input named `name`. */
function typeInto(name: string, value: string): void {
  fireEvent.change(input(name), { target: { value } });
}

/** The `<label>` bound to `field`. */
function labelOf(field: HTMLInputElement): HTMLLabelElement {
  const label = Array.from(document.querySelectorAll("label")).find(
    (element) => element.htmlFor === field.id,
  );
  if (label === undefined) {
    throw new Error(`no label is bound to ${field.id}`);
  }
  return label;
}

/**
 * A key-row button by its label, hidden or not. A `hidden` element's accessible name is empty
 * (dom-accessibility-api), so `getByRole` with a name cannot reach a dark key; every key is
 * taken from the key row's buttons, hidden ones included, by its text.
 */
function key(label: string): HTMLElement {
  const button = within(screen.getByRole("group", { name: "Function keys" }))
    .getAllByRole("button", { hidden: true })
    .find((element) => element.textContent === label);
  if (button === undefined) {
    throw new Error(`the key row has no ${label} button`);
  }
  return button;
}

/** The `ERRMSG` (`error`) or `INFOMSG` (`info`) row. */
function messageRow(kind: "error" | "info"): HTMLElement {
  const row = screen
    .getAllByRole("alert")
    .find((element) => element.getAttribute("data-message-kind") === kind);
  if (row === undefined) {
    throw new Error(`the screen shows no ${kind} row`);
  }
  return row;
}

/** The screen's form. */
function mapForm(): HTMLFormElement {
  const form = document.querySelector("form");
  if (form === null) {
    throw new Error("the screen renders no form");
  }
  return form;
}

/** The 43 map inputs, in map order. */
function mapInputs(): HTMLInputElement[] {
  return Array.from(mapForm().querySelectorAll("input"));
}

/** The ids of the enterable map inputs, in map order. */
function enterableIds(): string[] {
  return mapInputs()
    .filter((element) => !element.readOnly)
    .map((element) => element.id);
}

/** The ids of the read-only map inputs, in map order. */
function readOnlyIds(): string[] {
  return mapInputs()
    .filter((element) => element.readOnly)
    .map((element) => element.id);
}

/** The three SSN part inputs. */
function ssnInputs(): HTMLInputElement[] {
  return [input("SSN:"), input("SSN 4th and 5th digits"), input("SSN last 4 digits")];
}

/** Waits until no request of the screen is pending. */
async function settled(): Promise<void> {
  await waitFor(() => {
    expect(mapForm().getAttribute("aria-busy")).toBe("false");
  });
}

/** Types the account number, presses ENTER=Process and waits for the fetched details. */
async function fetchAccount(): Promise<void> {
  typeInto("Account Number :", ACCOUNT);
  fireEvent.click(key("ENTER=Process"));
  await waitFor(() => {
    expect(input("Opened :").value).toBe("2014");
  });
  await settled();
}

/** Presses ENTER=Process on the shown details and waits for `F5=Save` to show. */
async function validate(): Promise<void> {
  fireEvent.click(key("ENTER=Process"));
  await screen.findByRole("button", { name: "F5=Save" });
  await settled();
}

/** Asserts the empty screen of the first entry: initial values and the account field enterable. */
function expectEmptyScreen(acctId: string): void {
  expect(input("Account Number :").value).toBe(acctId);
  expect(ssnInputs().map((element) => element.value)).toEqual(["999", "99", "9999"]);
  const others = mapInputs().filter(
    (element) => element !== input("Account Number :") && !ssnInputs().includes(element),
  );
  expect(others.map((element) => element.value)).toEqual(new Array<string>(39).fill(""));
  expect(enterableIds()).toEqual([input("Account Number :").id]);
  expect(messageRow("info").textContent).toBe(INFO_NOT_FETCHED);
  expect(key("F5=Save").hasAttribute("hidden")).toBe(true);
  expect(key("F12=Cancel").hasAttribute("hidden")).toBe(true);
}

/** Asserts the protection of states `S` and `E`: all but account, customer ID and country. */
function expectDetailsEnterable(): void {
  expect(readOnlyIds()).toEqual([
    input("Account Number :").id,
    input("Customer id :").id,
    input("Country").id,
  ]);
}

beforeEach(() => {
  sent.length = 0;
  setSession("test-token", "USER0001", "U");
});

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COACTUP render", { timeout: TEST_TIMEOUT_MS }, () => {
  it("opens with the header, the empty map, the initial values, the key row and no request", () => {
    // audit: COACTUPC-001
    // audit: COACTUPC-072
    // audit: COACTUPC-073
    // audit: COACTUPC-080
    // audit: COACTUP-011
    // audit: COACTUP-057
    // audit: COACTUP-122
    // audit: COACTUP-125
    // audit: COACTUP-126
    // audit: COACTUP-127
    // audit: COACTUP-128
    // audit: COACTUP-129
    // audit: COACTUP-130
    // audit: COACTUP-131
    // audit: COACTUP-132
    // audit: COACTUP-133
    // audit: COACTUP-134
    stubApi();
    renderScreen();

    const header = (field: string): string | null | undefined =>
      document.querySelector(`[data-field="${field}"]`)?.textContent;
    const title01 = `${" ".repeat(6)}AWS Mainframe Modernization${" ".repeat(7)}`;
    const title02 = `${" ".repeat(14)}CardDemo${" ".repeat(18)}`;
    expect([title01.length, title02.length]).toEqual([40, 40]);
    expect(header("TRNNAME")).toBe("CAUP");
    expect(header("PGMNAME")).toBe("COACTUPC");
    expect(header("TITLE01")).toBe(title01);
    expect(header("TITLE02")).toBe(title02);
    expect(header("CURDATE")).toMatch(/^\d{2}\/\d{2}\/\d{2}$/);
    expect(header("CURTIME")).toMatch(/^\d{2}:\d{2}:\d{2}$/);

    const title = screen.getByRole("heading", { level: 1, name: "Update Account" });
    expect(title.getAttribute("data-slot")).toBe("card-title");
    const caption = screen.getByRole("heading", { level: 2, name: "Customer Details" });
    expect(caption.parentElement?.className).toBe("row-start-11 col-start-32 col-span-16");

    const acct = input("Account Number :");
    expect(acct.maxLength).toBe(11);
    expect(document.activeElement).toBe(acct);
    expect(mapInputs()).toHaveLength(43);
    expectEmptyScreen("");
    expect(
      [input("Opened :"), input("Opened month"), input("Opened day")].map((e) => e.maxLength),
    ).toEqual([4, 2, 2]);
    expect(ssnInputs().map((element) => element.maxLength)).toEqual([3, 2, 4]);
    expect(
      [input("Phone 1:"), input("Phone 1 prefix"), input("Phone 1 line number")].map(
        (element) => element.maxLength,
      ),
    ).toEqual([3, 3, 4]);
    expect(input("Credit Limit :").maxLength).toBe(15);
    expect(input("First Name").maxLength).toBe(25);

    const info = messageRow("info");
    expect(info.getAttribute("data-slot")).toBe("alert");
    expect(info.className).not.toContain("text-destructive");
    const error = messageRow("error");
    expect(error.getAttribute("data-slot")).toBe("alert");
    expect(error.className).toContain("text-destructive");
    expect(error.getAttribute("aria-live")).toBe("polite");
    expect(error.textContent).toBe("");

    const keys = within(screen.getByRole("group", { name: "Function keys" }));
    expect(
      keys
        .getAllByRole("button", { hidden: true })
        .slice(0, 4)
        .map((button) => button.textContent),
    ).toEqual(["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"]);
    const enter = screen.getByRole("button", { name: "ENTER=Process" });
    expect(enter.getAttribute("type")).toBe("submit");
    expect(enter.getAttribute("data-variant")).toBe("default");
    expect(screen.getByRole("button", { name: "F3=Exit" }).getAttribute("data-variant")).toBe(
      "outline",
    );
    expect(screen.queryByRole("button", { name: "F5=Save" })).toBeNull();
    expect(screen.queryByRole("button", { name: "F12=Cancel" })).toBeNull();
    expect(key("F5=Save").getAttribute("data-variant")).toBe("outline");
    expect(key("F12=Cancel").getAttribute("data-variant")).toBe("outline");

    expect(sent).toEqual([]);
  });

  it.each([
    // audit: COACTUP-012
    { id: "COACTUP-012", text: "Account Number :", row: 5, col: 19, name: "Account Number :" },
    // audit: COACTUP-015
    { id: "COACTUP-015", text: "Active Y/N: ", row: 5, col: 57, name: "Active Y/N:" },
    // audit: COACTUP-018
    { id: "COACTUP-018", text: "Opened :", row: 6, col: 8, name: "Opened :" },
    // audit: COACTUP-020
    { id: "COACTUP-020", text: "-", row: 6, col: 22, name: "Opened month" },
    // audit: COACTUP-022
    { id: "COACTUP-022", text: "-", row: 6, col: 27, name: "Opened day" },
    // audit: COACTUP-025
    { id: "COACTUP-025", text: "Credit Limit        :", row: 6, col: 39, name: "Credit Limit :" },
    // audit: COACTUP-028
    { id: "COACTUP-028", text: "Expiry :", row: 7, col: 8, name: "Expiry :" },
    // audit: COACTUP-030
    { id: "COACTUP-030", text: "-", row: 7, col: 22, name: "Expiry month" },
    // audit: COACTUP-032
    { id: "COACTUP-032", text: "-", row: 7, col: 27, name: "Expiry day" },
    // audit: COACTUP-035
    {
      id: "COACTUP-035",
      text: "Cash credit Limit   :",
      row: 7,
      col: 39,
      name: "Cash credit Limit :",
    },
    // audit: COACTUP-038
    { id: "COACTUP-038", text: "Reissue:", row: 8, col: 8, name: "Reissue:" },
    // audit: COACTUP-040
    { id: "COACTUP-040", text: "-", row: 8, col: 22, name: "Reissue month" },
    // audit: COACTUP-042
    { id: "COACTUP-042", text: "-", row: 8, col: 27, name: "Reissue day" },
    // audit: COACTUP-045
    {
      id: "COACTUP-045",
      text: "Current Balance     :",
      row: 8,
      col: 39,
      name: "Current Balance :",
    },
    // audit: COACTUP-048
    {
      id: "COACTUP-048",
      text: "Current Cycle Credit:",
      row: 9,
      col: 39,
      name: "Current Cycle Credit:",
    },
    // audit: COACTUP-051
    { id: "COACTUP-051", text: "Account Group:", row: 10, col: 8, name: "Account Group:" },
    // audit: COACTUP-054
    {
      id: "COACTUP-054",
      text: "Current Cycle Debit :",
      row: 10,
      col: 39,
      name: "Current Cycle Debit :",
    },
    // audit: COACTUP-058
    { id: "COACTUP-058", text: "Customer id  :", row: 12, col: 8, name: "Customer id :" },
    // audit: COACTUP-061
    { id: "COACTUP-061", text: "SSN:", row: 12, col: 49, name: "SSN:" },
    // audit: COACTUP-063
    { id: "COACTUP-063", text: "-", row: 12, col: 59, name: "SSN 4th and 5th digits" },
    // audit: COACTUP-065
    { id: "COACTUP-065", text: "-", row: 12, col: 64, name: "SSN last 4 digits" },
    // audit: COACTUP-068
    { id: "COACTUP-068", text: "Date of birth:", row: 13, col: 8, name: "Date of birth:" },
    // audit: COACTUP-070
    { id: "COACTUP-070", text: "-", row: 13, col: 28, name: "Date of birth month" },
    // audit: COACTUP-072
    { id: "COACTUP-072", text: "-", row: 13, col: 33, name: "Date of birth day" },
    // audit: COACTUP-075
    { id: "COACTUP-075", text: "FICO Score:", row: 13, col: 49, name: "FICO Score:" },
    // audit: COACTUP-078
    { id: "COACTUP-078", text: "First Name", row: 14, col: 1, name: "First Name" },
    // audit: COACTUP-079
    { id: "COACTUP-079", text: "Middle Name: ", row: 14, col: 28, name: "Middle Name:" },
    // audit: COACTUP-080
    { id: "COACTUP-080", text: "Last Name : ", row: 14, col: 55, name: "Last Name :" },
    // audit: COACTUP-086
    { id: "COACTUP-086", text: "Address:", row: 16, col: 1, name: "Address:" },
    // audit: COACTUP-089
    { id: "COACTUP-089", text: "State ", row: 16, col: 63, name: "State" },
    // audit: COACTUP-094
    { id: "COACTUP-094", text: "Zip", row: 17, col: 63, name: "Zip" },
    // audit: COACTUP-097
    { id: "COACTUP-097", text: "City ", row: 18, col: 1, name: "City" },
    // audit: COACTUP-100
    { id: "COACTUP-100", text: "Country", row: 18, col: 63, name: "Country" },
    // audit: COACTUP-103
    { id: "COACTUP-103", text: "Phone 1:", row: 19, col: 1, name: "Phone 1:" },
    // audit: COACTUP-108
    {
      id: "COACTUP-108",
      text: "Government Issued Id Ref    : ",
      row: 19,
      col: 24,
      name: "Government Issued Id Ref :",
    },
    // audit: COACTUP-111
    { id: "COACTUP-111", text: "Phone 2:", row: 20, col: 1, name: "Phone 2:" },
    // audit: COACTUP-116
    { id: "COACTUP-116", text: "EFT Account Id: ", row: 20, col: 24, name: "EFT Account Id:" },
    // audit: COACTUP-119
    {
      id: "COACTUP-119",
      text: "Primary Card Holder Y/N:",
      row: 20,
      col: 53,
      name: "Primary Card Holder Y/N:",
    },
  ])("$id shows the literal $text at ($row,$col) as the label of its input", (literal) => {
    stubApi();
    renderScreen();
    const label = labelOf(input(literal.name));
    expect(label.textContent).toBe(literal.text);
    expect(label.parentElement?.className).toBe(
      `row-start-${literal.row} col-start-${literal.col} col-span-${literal.text.length}`,
    );
  });

  it.each([
    // audit: COACTUP-014
    { id: "COACTUP-014", row: 5, col: 50, length: 0 },
    // audit: COACTUP-017
    { id: "COACTUP-017", row: 5, col: 72, length: 0 },
    // audit: COACTUP-024
    { id: "COACTUP-024", row: 6, col: 32, length: 0 },
    // audit: COACTUP-027
    { id: "COACTUP-027", row: 6, col: 77, length: 0 },
    // audit: COACTUP-034
    { id: "COACTUP-034", row: 7, col: 32, length: 0 },
    // audit: COACTUP-037
    { id: "COACTUP-037", row: 7, col: 77, length: 0 },
    // audit: COACTUP-044
    { id: "COACTUP-044", row: 8, col: 32, length: 0 },
    // audit: COACTUP-047
    { id: "COACTUP-047", row: 8, col: 77, length: 0 },
    // audit: COACTUP-050
    { id: "COACTUP-050", row: 9, col: 77, length: 0 },
    // audit: COACTUP-053
    { id: "COACTUP-053", row: 10, col: 34, length: 0 },
    // audit: COACTUP-056
    { id: "COACTUP-056", row: 10, col: 77, length: 0 },
    // audit: COACTUP-060
    { id: "COACTUP-060", row: 12, col: 33, length: 0 },
    // audit: COACTUP-067
    { id: "COACTUP-067", row: 12, col: 71, length: 0 },
    // audit: COACTUP-074
    { id: "COACTUP-074", row: 13, col: 38, length: 0 },
    // audit: COACTUP-077
    { id: "COACTUP-077", row: 13, col: 66, length: 0 },
    // audit: COACTUP-082
    { id: "COACTUP-082", row: 15, col: 27, length: 0 },
    // audit: COACTUP-084
    { id: "COACTUP-084", row: 15, col: 54, length: 0 },
    // audit: COACTUP-088
    { id: "COACTUP-088", row: 16, col: 61, length: 0 },
    // audit: COACTUP-091
    { id: "COACTUP-091", row: 16, col: 76, length: 0 },
    // audit: COACTUP-093
    { id: "COACTUP-093", row: 17, col: 61, length: 0 },
    // audit: COACTUP-096
    { id: "COACTUP-096", row: 17, col: 79, length: 0 },
    // audit: COACTUP-099
    { id: "COACTUP-099", row: 18, col: 61, length: 0 },
    // audit: COACTUP-102
    { id: "COACTUP-102", row: 18, col: 77, length: 0 },
    // audit: COACTUP-107
    { id: "COACTUP-107", row: 19, col: 23, length: 0 },
    // audit: COACTUP-110
    { id: "COACTUP-110", row: 19, col: 79, length: 0 },
    // audit: COACTUP-115
    { id: "COACTUP-115", row: 20, col: 23, length: 0 },
    // audit: COACTUP-118
    { id: "COACTUP-118", row: 20, col: 52, length: 0 },
    // audit: COACTUP-121
    { id: "COACTUP-121", row: 20, col: 80, length: 0 },
    // audit: COACTUP-123
    { id: "COACTUP-123", row: 22, col: 69, length: 0 },
    // audit: COACTUP-124
    { id: "COACTUP-124", row: 1, col: 1, length: 9 },
  ])("$id renders no element for the unnamed field at ($row,$col)", (field) => {
    stubApi();
    renderScreen();
    const cell = `.row-start-${field.row}.col-start-${field.col}`;
    expect(mapForm().querySelector(cell)).toBeNull();
    expect(document.querySelector(`${cell}.col-span-${Math.max(field.length, 1)}`)).toBeNull();
  });

  it("ENTER fetches the account through endpoint 5 and shows it, SSN redacted", async () => {
    // audit: COACTUP-122
    // audit: COACTUP-127
    // audit: COACTUP-128
    // audit: COACTUPC-080
    // audit: COACTUPC-081
    stubApi(get(200, VIEW));
    renderScreen();
    await fetchAccount();

    expect(sent).toEqual([
      { method: "GET", path: ACCOUNT_PATH, authorization: "Bearer test-token", body: undefined },
    ]);
    expect(
      [input("Opened :"), input("Opened month"), input("Opened day")].map((e) => e.value),
    ).toEqual(["2014", "11", "20"]);
    expect(input("Credit Limit :").value).toBe("+      2,500.00");
    expect(input("Cash credit Limit :").value).toBe("+        800.00");
    expect(input("Current Balance :").value).toBe("+        613.45");
    expect(input("Current Cycle Credit:").value).toBe("+           .00");
    expect(input("Current Cycle Debit :").value).toBe("+         25.10");
    expect(ssnInputs().map((element) => element.value)).toEqual(["***", "**", "1234"]);
    expect(input("First Name").value).toBe("Ada");
    expect(
      [input("Phone 2:"), input("Phone 2 prefix"), input("Phone 2 line number")].map(
        (element) => element.value,
      ),
    ).toEqual(["217", "555", "0199"]);
    expect(input("Account Number :").value).toBe(ACCOUNT);

    expect(messageRow("info").textContent).toBe(INFO_DETAILS);
    expect(messageRow("error").textContent).toBe("");
    expectDetailsEnterable();
    expect(document.activeElement).toBe(input("Active Y/N:"));
    expect(key("F5=Save").hasAttribute("hidden")).toBe(true);
    expect(key("F12=Cancel").hasAttribute("hidden")).toBe(true);
  });

  it("ENTER validates the details through endpoint 6: raw fields, snapshot, no SSN", async () => {
    // audit: COACTUP-135
    // audit: COACTUP-127
    // audit: COACTUP-128
    // audit: COACTUPC-080
    // audit: COACTUPC-083
    stubApi(get(200, VIEW), put(200, VALIDATED_NEW_LIMIT));
    renderScreen();
    expect(key("ENTER=Process").getAttribute("type")).toBe("submit");
    expect(key("ENTER=Process").getAttribute("data-variant")).toBe("default");
    await fetchAccount();
    typeInto("Credit Limit :", "3000.00");
    // The Enter key, outside any field, submits the form as the ENTER=Process button does.
    fireEvent.keyDown(window, { key: "Enter" });
    expect(await screen.findByRole("button", { name: "F5=Save" })).toBe(key("F5=Save"));
    await settled();

    expect(sent).toHaveLength(2);
    expect(sent[1]).toEqual({
      method: "PUT",
      path: ACCOUNT_PATH,
      authorization: "Bearer test-token",
      body: {
        aid: "ENTER",
        fields: { ...FETCHED_FIELDS, credit_limit: "3000.00" },
        snapshot: VIEW.snapshot,
      },
    });
    expect(messageRow("info").textContent).toBe("Changes validated.Press F5 to save");
    expect(messageRow("error").textContent).toBe("");
    expect(input("Credit Limit :").value).toBe("+      3,000.00");
    expect(screen.getByRole("button", { name: "F12=Cancel" })).toBe(key("F12=Cancel"));
    expect(enterableIds()).toEqual([]);
    expect(document.activeElement).toBe(input("Active Y/N:"));
  });

  it("sends short date and phone parts zero-filled on the left and empty parts empty", async () => {
    // audit: COACTUPC-008
    stubApi(get(200, VIEW), put(200, VALIDATED));
    renderScreen();
    await fetchAccount();
    typeInto("Expiry month", "6");
    typeInto("Phone 1:", "12");
    typeInto("Date of birth day", "");
    await validate();

    expect(sent[1]?.body).toMatchObject({
      aid: "ENTER",
      fields: { expiry_month: "06", phone_1_area: "012", dob_day: "" },
    });
    expect(
      [input("Expiry month"), input("Phone 1:"), input("Date of birth day")].map((e) => e.value),
    ).toEqual(["06", "012", ""]);
  });

  it("F5=Save saves with the re-entered SSN parts, and the next key starts over", async () => {
    // audit: COACTUP-137
    // audit: COACTUPC-006
    // audit: COACTUPC-080
    // audit: COACTUPC-083
    stubApi(get(200, VIEW), put(200, VALIDATED), put(200, COMMITTED));
    renderScreen();
    await fetchAccount();
    // The SSN is re-entered before ENTER: state N, where F5 applies, protects every field
    // (COACTUPC-080), so the parts F5 sends are those the validation received.
    typeInto("SSN:", "987");
    typeInto("SSN 4th and 5th digits", "65");
    typeInto("SSN last 4 digits", "4321");
    await validate();
    expect(sent[1]?.body).toMatchObject({
      aid: "ENTER",
      fields: { ssn_part1: "987", ssn_part2: "65", ssn_part3: "4321" },
    });
    expect(ssnInputs().every((element) => element.readOnly)).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "F5=Save" }));
    await waitFor(() => {
      expect(messageRow("info").textContent).toBe("Changes committed to database");
    });
    await settled();
    expect(sent).toHaveLength(3);
    expect(sent[2]?.body).toEqual({
      aid: "PF5",
      fields: {
        ...FETCHED_FIELDS,
        ssn_part1: "987",
        ssn_part2: "65",
        ssn_part3: "4321",
      },
      snapshot: { ...VIEW.snapshot, state: "N" },
    });
    expect(messageRow("error").textContent).toBe("");
    expect(enterableIds()).toEqual([]);
    expect(key("F5=Save").hasAttribute("hidden")).toBe(true);
    expect(key("F12=Cancel").hasAttribute("hidden")).toBe(true);
    expect(document.activeElement).toBe(input("Account Number :"));

    fireEvent.click(key("ENTER=Process"));
    expectEmptyScreen("");
    expect(document.activeElement).toBe(input("Account Number :"));
    expect(sent).toHaveLength(3);
  });

  it("a 409 on F5 shows the changed-by-another-user message and the fetched values", async () => {
    // audit: COACTUP-125
    // audit: COACTUP-137
    stubApi(get(200, VIEW), put(200, VALIDATED), put(409, RECORD_CHANGED));
    renderScreen();
    await fetchAccount();
    typeInto("First Name", "Grace");
    await validate();
    // The F5 key, bound like its button, saves in state N.
    fireEvent.keyDown(window, { key: "F5" });
    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(RECORD_CHANGED.message);
    });
    await settled();

    expect(sent.map((request) => request.method)).toEqual(["GET", "PUT", "PUT"]);
    expect(sent[2]?.body).toMatchObject({ aid: "PF5", fields: { first_name: "Grace" } });
    expect(input("First Name").value).toBe("Ada");
    expect(messageRow("info").textContent).toBe(INFO_DETAILS);
    expectDetailsEnterable();
    expect(key("F5=Save").hasAttribute("hidden")).toBe(true);
    expect(key("F12=Cancel").hasAttribute("hidden")).toBe(true);
  });

  it.each([
    {
      title: "a blank first name marks it with * and leaves state E",
      name: "First Name",
      value: "",
      failure: FIRST_NAME_BLANK,
      shown: "*".padEnd(25, " "),
      cancelShown: true,
    },
    {
      title: "no change marks the status and keeps state S",
      name: "Active Y/N:",
      value: undefined,
      failure: NO_CHANGE,
      shown: "Y",
      cancelShown: false,
    },
  ])("a 400 naming a field: $title, with the cursor on it", async (edit) => {
    // audit: COACTUPC-081
    // audit: COACTUPC-083
    // audit: COACTUPC-080
    // audit: COACTUP-125
    stubApi(get(200, VIEW), put(400, edit.failure));
    renderScreen();
    await fetchAccount();
    if (edit.value !== undefined) {
      typeInto(edit.name, edit.value);
    }
    fireEvent.click(key("ENTER=Process"));
    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(edit.failure.message);
    });
    await settled();

    const field = input(edit.name);
    expect(document.activeElement).toBe(field);
    expect(field.value).toBe(edit.shown);
    expect(field.getAttribute("aria-invalid")).toBe("true");
    expect(labelOf(field).getAttribute("data-error")).toBe("true");
    expect(messageRow("info").textContent).toBe(INFO_DETAILS);
    expectDetailsEnterable();
    expect(key("F5=Save").hasAttribute("hidden")).toBe(true);
    expect(key("F12=Cancel").hasAttribute("hidden")).toBe(!edit.cancelShown);
  });

  it("ENTER without an account number shows * in the red account field", async () => {
    // audit: COACTUPC-073
    // audit: COACTUPC-081
    stubApi(get(400, NO_INPUT, `/api/v1/accounts/${" ".repeat(11)}`));
    renderScreen();
    fireEvent.click(key("ENTER=Process"));
    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(NO_INPUT.message);
    });
    await settled();

    expect(sent).toHaveLength(1);
    const acct = input("Account Number :");
    expectEmptyScreen("*".padEnd(11, " "));
    expect(acct.getAttribute("aria-invalid")).toBe("true");
    expect(labelOf(acct).getAttribute("data-error")).toBe("true");
    expect(document.activeElement).toBe(acct);
  });

  it("F12=Cancel discards the edits and shows the fetched values, with no request", async () => {
    // audit: COACTUPC-069
    // audit: COACTUP-138
    stubApi(get(200, VIEW), put(200, VALIDATED_NEW_LIMIT));
    renderScreen();
    await fetchAccount();
    typeInto("First Name", "Grace");
    typeInto("Credit Limit :", "3000.00");
    await validate();
    expect(input("First Name").value).toBe("Grace");
    expect(input("Credit Limit :").value).toBe("+      3,000.00");

    fireEvent.click(screen.getByRole("button", { name: "F12=Cancel" }));
    expect(input("First Name").value).toBe("Ada");
    expect(input("Credit Limit :").value).toBe("+      2,500.00");
    expect(messageRow("info").textContent).toBe(INFO_DETAILS);
    expect(messageRow("error").textContent).toBe("");
    expectDetailsEnterable();
    expect(document.activeElement).toBe(input("Active Y/N:"));
    expect(key("F5=Save").hasAttribute("hidden")).toBe(true);
    expect(key("F12=Cancel").hasAttribute("hidden")).toBe(true);
    expect(sent).toHaveLength(2);
  });

  it.each([
    {
      title: "the F3=Exit button returns to the screen that transferred here",
      state: { from: PATHS.COACTVW },
      viaKey: false,
      arrived: "Arrived at COACTVW from /accounts/update",
    },
    {
      title: "the F3 key opens the main menu when no screen transferred here",
      state: undefined,
      viaKey: true,
      arrived: "Arrived at COMEN01 from /accounts/update",
    },
  ])("$title, dropping the edits, with no request", async (exit) => {
    // audit: COACTUP-136
    // audit: COACTUPC-005
    stubApi();
    renderScreen(exit.state);
    expect(key("F3=Exit").getAttribute("data-variant")).toBe("outline");
    typeInto("Account Number :", ACCOUNT);
    if (exit.viaKey) {
      fireEvent.keyDown(window, { key: "F3" });
    } else {
      fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));
    }
    expect((await screen.findByText(exit.arrived)).textContent).toBe(exit.arrived);
    expect(sent).toEqual([]);
  });

  it("processes the keys COACTUPC does not accept as ENTER, and Shift+F3 as F3", async () => {
    // audit: COACTUPC-003
    // audit: COACTUP-138
    stubApi(get(200, VIEW), put(400, NO_CHANGE), put(400, NO_CHANGE));
    renderScreen();
    typeInto("Account Number :", ACCOUNT);
    // F12 before a fetch is ENTER: endpoint 5.
    fireEvent.keyDown(window, { key: "F12" });
    await waitFor(() => {
      expect(input("Opened :").value).toBe("2014");
    });
    await settled();
    // F5 outside state N is ENTER: endpoint 6 with aid ENTER.
    fireEvent.keyDown(window, { key: "F5" });
    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(NO_CHANGE.message);
    });
    await settled();
    // A key the map does not label is ENTER too.
    fireEvent.keyDown(window, { key: "F7" });
    await waitFor(() => {
      expect(sent).toHaveLength(3);
    });
    await settled();
    expect(sent.map((request) => request.method)).toEqual(["GET", "PUT", "PUT"]);
    expect(sent.slice(1).map((request) => request.body)).toMatchObject([
      { aid: "ENTER" },
      { aid: "ENTER" },
    ]);

    fireEvent.keyDown(window, { key: "F3", shiftKey: true });
    expect(await screen.findByText("Arrived at COMEN01 from /accounts/update")).toBeTruthy();
    expect(sent).toHaveLength(3);
  });

  it.each([
    {
      title: "a failed account lock leaves state L",
      failure: ACCOUNT_LOCK_FAILED,
      info: "Changes unsuccessful. Please try again",
      acctEnterable: true,
      cancelShown: true,
    },
    {
      title: "a failed rewrite leaves state F",
      failure: REWRITE_FAILED,
      info: "Changes unsuccessful. Please try again",
      acctEnterable: true,
      cancelShown: true,
    },
    {
      title: "a failed customer lock leaves state C, as audited",
      failure: CUSTOMER_LOCK_FAILED,
      info: "Changes committed to database",
      acctEnterable: false,
      cancelShown: false,
    },
  ])("a 500 on F5: $title, and F12 then starts over", async (outcome) => {
    // audit: COACTUPC-006
    // audit: COACTUPC-080
    // audit: COACTUPC-083
    stubApi(get(200, VIEW), put(200, VALIDATED), put(500, outcome.failure));
    renderScreen();
    await fetchAccount();
    typeInto("First Name", "Grace");
    await validate();
    fireEvent.click(screen.getByRole("button", { name: "F5=Save" }));
    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(outcome.failure.message);
    });
    await settled();

    expect(messageRow("info").textContent).toBe(outcome.info);
    expect(enterableIds()).toEqual(outcome.acctEnterable ? [input("Account Number :").id] : []);
    expect(document.activeElement).toBe(input("Account Number :"));
    expect(key("F5=Save").hasAttribute("hidden")).toBe(true);
    expect(key("F12=Cancel").hasAttribute("hidden")).toBe(!outcome.cancelShown);

    fireEvent.click(key("F12=Cancel"));
    expectEmptyScreen("");
    expect(messageRow("error").textContent).toBe("");
    expect(sent).toHaveLength(3);
  });

  it("a re-read failure on ENTER leaves the details not fetched, account number kept", async () => {
    // audit: COACTUPC-073
    stubApi(get(200, VIEW), put(404, CUSTOMER_NOT_FOUND));
    renderScreen();
    await fetchAccount();
    typeInto("First Name", "Grace");
    fireEvent.click(key("ENTER=Process"));
    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(CUSTOMER_NOT_FOUND.message);
    });
    await settled();

    expectEmptyScreen(ACCOUNT);
    expect(document.activeElement).toBe(input("Account Number :"));
    expect(sent).toHaveLength(2);
  });
});

// Endpoint 6 answers a record another user changed since the fetch with a 409 `ScreenError`. The
// screen shows that body's message as it came, and only a 401 ends the session.
describe("COACTUP: a 409 on F5 keeps the session", { timeout: TEST_TIMEOUT_MS }, () => {
  /** Every request `fetch` received: method, path, query, `Authorization` header and JSON body. */
  async function sentRequests() {
    return Promise.all(
      vi.mocked(fetch).mock.calls.map(async ([input, init]) => {
        const request = input instanceof Request ? input : new Request(input, init);
        const url = new URL(request.url);
        const text = await request.clone().text();
        const body: unknown = text === "" ? undefined : JSON.parse(text);
        return {
          method: request.method,
          path: url.pathname,
          query: Object.fromEntries(url.searchParams),
          authorization: request.headers.get("Authorization"),
          body,
        };
      }),
    );
  }

  it("shows the 409 body's message unchanged and keeps the session, every request carrying the bearer token and no query", async () => {
    stubApi(get(200, VIEW), put(200, VALIDATED), put(409, RECORD_CHANGED));
    renderScreen();
    await fetchAccount();
    typeInto("First Name", "Grace");
    await validate();
    fireEvent.click(screen.getByRole("button", { name: "F5=Save" }));
    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(RECORD_CHANGED.message);
    });
    await settled();

    expect(getSession()).toEqual({ token: "test-token", userId: "USER0001", userType: "U" });
    const fields = { ...FETCHED_FIELDS, first_name: "Grace" };
    expect(await sentRequests()).toEqual([
      {
        method: "GET",
        path: ACCOUNT_PATH,
        query: {},
        authorization: "Bearer test-token",
        body: undefined,
      },
      {
        method: "PUT",
        path: ACCOUNT_PATH,
        query: {},
        authorization: "Bearer test-token",
        body: { aid: "ENTER", fields, snapshot: VIEW.snapshot },
      },
      {
        method: "PUT",
        path: ACCOUNT_PATH,
        query: {},
        authorization: "Bearer test-token",
        body: { aid: "PF5", fields, snapshot: { ...VIEW.snapshot, state: "N" } },
      },
    ]);
  });
});

/** The 401 of the `SignedOnUser` extractor for a missing, invalid or expired token. */
const SIGNON_REQUIRED: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};

/**
 * Renders the application's route table at `/accounts/update`, so the sign-on guard that only the
 * table mounts stands in front of the screen. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COACTUP] });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

// A 401 clears the stored session (`api/client.ts`), and the sign-on guard in front of the route
// replaces the still-mounted screen with sign-on at once: after the key that sent the refused
// request, the test presses no key and follows no link.
describe("COACTUP in the route table: a 401 ends the session", { timeout: TEST_TIMEOUT_MS }, () => {
  it.each([
    {
      title: "the fetch of ENTER (endpoint 5)",
      replies: [get(401, SIGNON_REQUIRED)],
      drive: async () => {
        typeInto("Account Number :", ACCOUNT);
        fireEvent.click(key("ENTER=Process"));
      },
      methods: ["GET"],
    },
    {
      title: "the validation of ENTER (endpoint 6)",
      replies: [get(200, VIEW), put(401, SIGNON_REQUIRED)],
      drive: async () => {
        await fetchAccount();
        typeInto("First Name", "Grace");
        fireEvent.click(key("ENTER=Process"));
      },
      methods: ["GET", "PUT"],
    },
    {
      title: "the save of F5 (endpoint 6)",
      replies: [get(200, VIEW), put(200, VALIDATED), put(401, SIGNON_REQUIRED)],
      drive: async () => {
        await fetchAccount();
        typeInto("First Name", "Grace");
        await validate();
        fireEvent.click(screen.getByRole("button", { name: "F5=Save" }));
      },
      methods: ["GET", "PUT", "PUT"],
    },
  ])(
    "$title answered 401 replaces the screen with sign-on and clears the session, with no retry",
    async ({ replies, drive, methods }) => {
      stubApi(...replies);
      const router = renderRouteTable();
      expect(
        await screen.findByRole("heading", { level: 1, name: "Update Account" }),
      ).toBeDefined();

      await drive();

      expect(await screen.findByRole("button", { name: "ENTER=Sign-on" })).toBeDefined();
      expect(router.state.location.pathname).toBe(PATHS.COSGN00);
      expect(screen.queryByRole("heading", { level: 1, name: "Update Account" })).toBeNull();
      expect(getSession()).toBeNull();
      expect(sent.map((request) => request.method)).toEqual(methods);
      expect(sent.every((request) => request.path === ACCOUNT_PATH)).toBe(true);
      expect(sent.every((request) => request.authorization === "Bearer test-token")).toBe(true);
      router.dispose();
    },
  );
});

/** The message of the shared request deadline's `TimeoutError` (`api/client.ts`). */
const TIMED_OUT = "No answer from the server within 30 seconds";

/** The `aid` an endpoint 6 request body carries; `undefined` for a request without one. */
function aidOf(body: unknown): unknown {
  return typeof body === "object" && body !== null && "aid" in body ? body.aid : undefined;
}

/**
 * Replaces `fetch` as `stubApi` does, except that the save (`aid` PF5) gets no answer: like a
 * request the server accepted and never completed, it settles only when its signal aborts, and
 * then rejects with the abort reason, as `fetch` does. Resolves with that signal once the save
 * was sent.
 */
function stubUnansweredSave(
  ...replies: { method: "GET" | "PUT"; path: string; status: number; body: unknown }[]
): Promise<AbortSignal> {
  const pending = [...replies];
  return new Promise((saveSent) => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const req = input instanceof Request ? input : new Request(input, init);
        const path = decodeURIComponent(new URL(req.url).pathname);
        const body: unknown = req.method === "GET" ? undefined : await req.clone().json();
        const authorization = req.headers.get("Authorization");
        sent.push({ method: req.method, path, authorization, body });
        if (req.method === "PUT" && aidOf(body) === "PF5") {
          saveSent(req.signal);
          return new Promise<Response>((_answer, fail) => {
            const abandon = (): void => {
              fail(req.signal.reason);
            };
            if (req.signal.aborted) {
              abandon();
            } else {
              req.signal.addEventListener("abort", abandon, { once: true });
            }
          });
        }
        const next = pending.at(0);
        if (next === undefined || next.method !== req.method || next.path !== path) {
          return reply(500, { message: `Unexpected ${req.method} ${path}` });
        }
        pending.shift();
        return reply(next.status, next.body);
      }),
    );
  });
}

// Every request shares one deadline (`api/client.ts`). A save the server never answers fails at
// the deadline like any other failure: its message in the error row, the keys unlocked, and the
// write not sent again, since it may have committed.
describe("COACTUP request deadline", { timeout: TEST_TIMEOUT_MS }, () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("fails an unanswered F5 save at the deadline, sends it once and serves the next ENTER", async () => {
    const saveSent = stubUnansweredSave(get(200, VIEW), put(200, VALIDATED), put(200, VALIDATED));
    renderScreen();
    await fetchAccount();
    typeInto("First Name", "Grace");
    await validate();

    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    fireEvent.click(screen.getByRole("button", { name: "F5=Save" }));
    const signal = await act(() => saveSent);
    expect(signal.aborted).toBe(false);
    await act(() => vi.advanceTimersByTimeAsync(REQUEST_DEADLINE_MS));
    vi.useRealTimers();

    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(TIMED_OUT);
    });
    await settled();
    expect(signal.aborted).toBe(true);
    const reason: unknown = signal.reason;
    expect(reason instanceof DOMException ? [reason.name, reason.message] : reason).toEqual([
      "TimeoutError",
      TIMED_OUT,
    ]);
    expect(sent.map((request) => [request.method, aidOf(request.body)])).toEqual([
      ["GET", undefined],
      ["PUT", "ENTER"],
      ["PUT", "PF5"],
    ]);
    expect(messageRow("info").textContent).toBe(VALIDATED.message);
    expect(input("First Name").value).toBe("Grace");

    const armed = vi.spyOn(globalThis, "setTimeout");
    const cleared = vi.spyOn(globalThis, "clearTimeout");
    fireEvent.click(key("ENTER=Process"));
    await waitFor(() => {
      expect(messageRow("error").textContent).toBe("");
    });
    await settled();
    expect(sent.map((request) => [request.method, aidOf(request.body)])).toEqual([
      ["GET", undefined],
      ["PUT", "ENTER"],
      ["PUT", "PF5"],
      ["PUT", "ENTER"],
    ]);
    expect(messageRow("info").textContent).toBe(VALIDATED.message);
    expect(key("F5=Save").hasAttribute("hidden")).toBe(false);
    const deadlines = armed.mock.calls.flatMap((call, index) =>
      call[1] === REQUEST_DEADLINE_MS ? [armed.mock.results[index].value] : [],
    );
    expect(deadlines).toHaveLength(1);
    expect(cleared).toHaveBeenCalledWith(deadlines[0]);
  });
});
