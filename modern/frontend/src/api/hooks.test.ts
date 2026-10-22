// @vitest-environment jsdom
import {
  QueryClient,
  QueryClientProvider,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { type ReactNode, createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getSession, setSession } from "@/auth/session";
import * as hooks from "./hooks";
import type { components } from "./schema";

/**
 * Tests of the 27 TanStack Query hooks over the openapi-fetch client of `client.ts`. The file
 * runs in jsdom (the docblock above), because `client.ts` reads `location.origin` when it is
 * imported. The global `fetch` is replaced by a stub that answers in process, so no request
 * leaves the test.
 *
 * Every card number and SSN below is in its redacted form with fake digits, the 16-digit keys
 * fail the card-number check digit, and every token and password is fake.
 */

const TOKEN = "test-token-not-a-real-jwt";
const ACCT_ID = "00000000050";
const TRAN_ID = "0000000000000001";
const AUTH_KEY = "0000000000000002";
const PAGE_AUTH_KEY = "0000000000000003";
const USER_ID = "USER0011";
const TYPE_CODE = "01";
const CARD_REF = 7;
const MASKED_CARD = "************0000";
const MASKED_SSN = "***-**-0000";
/** The page size each list call is handed, which no hook may send. */
const IGNORED_PAGE_SIZE = 50;

/** Replies with a JSON body, as the API sends every success and every `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces the global `fetch` with a stub that records what each request carried and answers it
 * with `answer`. `client.ts` hands `fetch` one `Request`, which the stub requires.
 */
function stubFetch(answer: (request: Request) => Response) {
  const seen: {
    method: string;
    path: string;
    query: Record<string, string>;
    authorization: string | null;
    body: unknown;
  }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      if (!(input instanceof Request)) {
        throw new TypeError("client.ts hands fetch a Request");
      }
      const url = new URL(input.url);
      const text = await input.clone().text();
      seen.push({
        method: input.method,
        path: url.pathname,
        query: Object.fromEntries(url.searchParams),
        authorization: input.headers.get("Authorization"),
        body: text === "" ? undefined : JSON.parse(text),
      });
      return answer(input);
    }),
  );
  return seen;
}

/** A provider around a fresh client that never retries, so a call settles on its first reply. */
function withQueryClient() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return ({ children }: { children: ReactNode }) =>
    createElement(QueryClientProvider, { client }, children);
}

/** Renders a mutation hook, fires it once with `variables` and waits until it settles. */
async function fireMutation<TData, TVariables>(
  useHook: () => UseMutationResult<TData, components["schemas"]["ScreenError"], TVariables>,
  variables: NoInfer<TVariables>,
) {
  const { result } = renderHook(useHook, { wrapper: withQueryClient() });
  act(() => {
    result.current.mutate(variables);
  });
  await waitFor(() => {
    expect(["success", "error"]).toContain(result.current.status);
  });
  return { data: result.current.data, error: result.current.error };
}

/** Renders a query hook, which fetches when it mounts, and waits until it settles. */
async function fireQuery<TData>(
  useHook: () => UseQueryResult<TData, components["schemas"]["ScreenError"]>,
) {
  const { result } = renderHook(useHook, { wrapper: withQueryClient() });
  await waitFor(() => {
    expect(["success", "error"]).toContain(result.current.status);
  });
  return { data: result.current.data, error: result.current.error };
}

const signonBody: components["schemas"]["SignonRequest"] = {
  user_id: "USER0001",
  password: "NOTREAL1",
};

const signonReply: components["schemas"]["SignonResponse"] = {
  next_route: "/menu",
  token: "test-token-issued-by-stub",
  user_id: "USER0001",
  user_type: "U",
};

const mainMenuReply: components["schemas"]["MenuResponse"] = {
  invalid_option_message: "Please enter a valid option number...",
  options: [
    {
      admin_only: false,
      installed: true,
      number: "01",
      program: "COACTVWC",
      text: "01. Account View",
    },
  ],
  user_type: "U",
};

const adminMenuReply: components["schemas"]["MenuResponse"] = {
  invalid_option_message: "Please enter a valid option number...",
  options: [
    {
      admin_only: true,
      installed: true,
      number: "01",
      program: "COUSR00C",
      text: "01. User List (Security)",
    },
  ],
};

const accountViewReply: components["schemas"]["AccountViewResponse"] = {
  acct_id: ACCT_ID,
  active_status: "Y",
  address_line_1: "1 TEST STREET",
  address_line_2: "",
  cash_credit_limit: "      1,000.00",
  city: "TESTVILLE",
  country: "USA",
  credit_limit: "      5,000.00",
  current_balance: "        100.00",
  current_cycle_credit: "          0.00",
  current_cycle_debit: "          0.00",
  customer_id: "000000050",
  date_of_birth: "1990-01-01",
  eft_account_id: "0000000001",
  expiration_date: "2030-12-31",
  fico_score: "700",
  first_name: "TEST",
  government_id: "FAKE-ID",
  group_id: "A000000000",
  last_name: "HOLDER",
  middle_name: "",
  open_date: "2010-01-01",
  phone_1: "(555)555-0100",
  phone_2: "",
  primary_card_holder: "Y",
  ssn: MASKED_SSN,
  state: "NC",
  zip: "00000",
};

const accountSnapshot: components["schemas"]["AccountUpdateSnapshot"] = {
  account_id: ACCT_ID,
  active_status: "Y",
  address_line_1: "1 TEST STREET",
  address_line_2: "",
  cash_credit_limit: "1000.00",
  city: "TESTVILLE",
  country: "USA",
  credit_limit: "5000.00",
  current_balance: "100.00",
  current_cycle_credit: "0.00",
  current_cycle_debit: "0.00",
  customer_id: "000000050",
  dob: "1990-01-01",
  eft_account_id: "0000000001",
  expiry_date: "2030-12-31",
  fico_score: "700",
  first_name: "TEST",
  government_id: "FAKE-ID",
  group_id: "A000000000",
  last_name: "HOLDER",
  middle_name: "",
  open_date: "2010-01-01",
  phone_1: "(555)555-0100  ",
  phone_2: "",
  primary_card_holder: "Y",
  ssn_last4: "0000",
  state: "S",
  state_code: "NC",
  zip: "00000",
};

const accountFetchReply: components["schemas"]["AccountUpdateView"] = {
  account_id: ACCT_ID,
  active_status: "Y",
  address_line_1: "1 TEST STREET",
  address_line_2: "",
  cash_credit_limit: "1000.00",
  city: "TESTVILLE",
  country: "USA",
  credit_limit: "5000.00",
  current_balance: "100.00",
  current_cycle_credit: "0.00",
  current_cycle_debit: "0.00",
  customer_id: "000000050",
  dob: "1990-01-01",
  eft_account_id: "0000000001",
  expiry_date: "2030-12-31",
  fico_score: "700",
  first_name: "TEST",
  government_id: "FAKE-ID",
  group_id: "A000000000",
  info_message: "Update account details presented above.",
  last_name: "HOLDER",
  middle_name: "",
  open_date: "2010-01-01",
  phone_1_area: "555",
  phone_1_line: "0100",
  phone_1_prefix: "555",
  phone_2_area: "",
  phone_2_line: "",
  phone_2_prefix: "",
  primary_card_holder: "Y",
  snapshot: accountSnapshot,
  ssn: MASKED_SSN,
  state: "NC",
  zip: "00000",
};

const accountSaveBody: components["schemas"]["AccountUpdateRequest"] = {
  aid: "PF5",
  fields: {
    active_status: "Y",
    address_line_1: "2 TEST STREET",
    address_line_2: "",
    cash_credit_limit: "1000.00",
    city: "TESTVILLE",
    country: "USA",
    credit_limit: "6000.00",
    current_balance: "100.00",
    current_cycle_credit: "0.00",
    current_cycle_debit: "0.00",
    customer_id: "000000050",
    dob_day: "01",
    dob_month: "01",
    dob_year: "1990",
    eft_account_id: "0000000001",
    expiry_day: "31",
    expiry_month: "12",
    expiry_year: "2030",
    fico_score: "700",
    first_name: "TEST",
    government_id: "FAKE-ID",
    group_id: "A000000000",
    last_name: "HOLDER",
    middle_name: "",
    open_day: "01",
    open_month: "01",
    open_year: "2010",
    phone_1_area: "555",
    phone_1_line: "0100",
    phone_1_prefix: "555",
    phone_2_area: "",
    phone_2_line: "",
    phone_2_prefix: "",
    primary_card_holder: "Y",
    reissue_day: "",
    reissue_month: "",
    reissue_year: "",
    ssn_part1: null,
    ssn_part2: null,
    ssn_part3: null,
    state: "NC",
    zip: "00000",
  },
  snapshot: { ...accountSnapshot, state: "N" },
};

const accountSaveReply: components["schemas"]["AccountUpdateResult"] = {
  cancel_enabled: false,
  committed: true,
  message: "Changes committed to database",
  save_enabled: false,
  state: "C",
};

const cardListReply: components["schemas"]["CardListResponse"] = {
  blank_rows_above: 0,
  first_key: 18,
  has_next_page: false,
  last_key: 18,
  last_page_shown: true,
  page_no: 3,
  rows: [{ acct_id: ACCT_ID, card_num: MASKED_CARD, card_ref: CARD_REF, status: "Y" }],
};

const cardViewReply: components["schemas"]["CardViewResponse"] = {
  acct_id: ACCT_ID,
  card_num: MASKED_CARD,
  card_ref: CARD_REF,
  embossed_name: "TEST HOLDER",
  expiration_date: "2030-12-31",
  info_message: "   Displaying requested details",
  status: "Y",
};

const cardSnapshot: components["schemas"]["CardUpdateSnapshot"] = {
  acct_id: ACCT_ID,
  embossed_name: "TEST HOLDER",
  expiration_date: "2030-12-31",
  state: "show_details",
  status: "Y",
};

const cardFetchReply: components["schemas"]["CardUpdateView"] = {
  acct_id: ACCT_ID,
  card_num: MASKED_CARD,
  card_ref: CARD_REF,
  embossed_name: "TEST HOLDER",
  expiration_date: "2030-12-31",
  info_message: "Details of selected card shown above",
  snapshot: cardSnapshot,
  status: "Y",
};

const cardSaveBody: components["schemas"]["CardUpdateRequest"] = {
  aid: "PF5",
  fields: {
    embossed_name: "TEST CARDHOLDER",
    expiry_month: "12",
    expiry_year: "2031",
    status: "Y",
  },
  snapshot: { ...cardSnapshot, state: "changes_ok_not_confirmed" },
};

const cardSaveReply: components["schemas"]["CardUpdateResult"] = {
  committed: true,
  info_message: "Changes committed to database",
  save_enabled: false,
  state: "changes_okayed_and_done",
};

const tranListReply: components["schemas"]["TranListResponse"] = {
  first_key: TRAN_ID,
  has_next_page: false,
  last_key: TRAN_ID,
  page_no: 2,
  rows: [
    {
      amount: "     -10.00",
      description: "TEST PURCHASE",
      display_date: "01/31/24",
      orig_ts: "2024-01-31T10:30:45",
      tran_id: TRAN_ID,
    },
    null,
  ],
  rows_kept: false,
};

const tranViewReply: components["schemas"]["TranViewResponse"] = {
  amount: "     -10.00",
  card_num: MASKED_CARD,
  cat_cd: "0001",
  description: "TEST PURCHASE",
  merchant_city: "TESTVILLE",
  merchant_id: "000000001",
  merchant_name: "TEST MERCHANT",
  merchant_zip: "00000",
  orig_ts: "2024-01-31T10:30:45",
  proc_ts: "2024-01-31T10:30:45",
  source: "POS TERM",
  tran_id: TRAN_ID,
  type_cd: "01",
};

const tranAddBody: components["schemas"]["TranAddRequest"] = {
  acct_id: ACCT_ID,
  aid: "ENTER",
  card_num: null,
  confirm: "Y",
  fields: {
    amount: "-00000010.00",
    cat_cd: "0001",
    description: "TEST PURCHASE",
    merchant_city: "TESTVILLE",
    merchant_id: "000000001",
    merchant_name: "TEST MERCHANT",
    merchant_zip: "00000",
    orig_date: "2024-01-31",
    proc_date: "2024-01-31",
    source: "POS TERM",
    type_cd: "01",
  },
};

const tranAddReply: components["schemas"]["TranAddResult"] = {
  acct_id: ACCT_ID,
  card_num: MASKED_CARD,
  field: "acct_id",
  message: "Transaction added successfully.  Your Tran ID is 0000000000000301.",
  new_tran_id: "0000000000000301",
};

const reportBody: components["schemas"]["ReportRequest"] = { confirm: "Y", monthly: "X" };

const reportReply: components["schemas"]["ReportResult"] = {
  message: "Monthly report submitted for printing ...",
};

const billPaymentBody: components["schemas"]["BillPaymentRequest"] = {
  acct_id: ACCT_ID,
  confirm: "Y",
};

const billPaymentReply: components["schemas"]["BillPaymentResult"] = {
  acct_id: ACCT_ID,
  cleared: false,
  current_balance: "         0.00",
  message: "Payment successful.  Your Transaction ID is 0000000000000302.",
  new_tran_id: "0000000000000302",
};

const userListReply: components["schemas"]["UserListResponse"] = {
  first_key: "USER0011",
  has_next_page: true,
  last_key: "USER0011",
  page_no: 2,
  rows: [{ first_name: "TEST", last_name: "USER", user_id: USER_ID, user_type: "U" }, null],
};

const userAddBody: components["schemas"]["UserAddRequest"] = {
  first_name: "TEST",
  last_name: "USER",
  password: "NOTREAL2",
  user_id: USER_ID,
  user_type: "U",
};

const userAddReply: components["schemas"]["ScreenResult"] = {
  message: `User ${USER_ID} has been added ...`,
};

const userFetchReply: components["schemas"]["UserView"] = {
  delete_prompt: "Press PF5 key to delete this user ...",
  first_name: "TEST",
  last_name: "USER",
  update_prompt: "Press PF5 key to save your updates ...",
  user_id: USER_ID,
  user_type: "U",
};

const userUpdateBody: components["schemas"]["UserUpdateRequest"] = {
  first_name: "TESTER",
  last_name: "USER",
  password: null,
  user_type: "U",
};

const userUpdateReply: components["schemas"]["ScreenResult"] = {
  message: `User ${USER_ID} has been updated ...`,
};

const userDeleteReply: components["schemas"]["ScreenResult"] = {
  message: `User ${USER_ID} has been deleted ...`,
};

const authSummaryReply: components["schemas"]["AuthSummaryResponse"] = {
  acct_id: ACCT_ID,
  first_key: AUTH_KEY,
  has_next_page: false,
  last_key: AUTH_KEY,
  rows: [
    {
      amount: "      100.00",
      approval: "A",
      auth_key: AUTH_KEY,
      auth_type: "PURC",
      date: "01/31/24",
      match_status: "P",
      time: "10:30:45",
      transaction_id: "TX0000000000001 ",
    },
  ],
  summary: {
    address_line_1: "1 TEST STREET",
    address_line_2: "",
    approved_amount: "      100.00",
    approved_count: "001",
    cash_balance: "        0.00",
    cash_limit: "     1000.00",
    credit_balance: "      100.00",
    credit_limit: "     5000.00",
    customer_id: "000000050",
    customer_name: "TEST H HOLDER",
    declined_amount: "        0.00",
    declined_count: "000",
    phone: "(555)555-0100",
    summary_found: true,
  },
};

const authDetailReply: components["schemas"]["AuthDetailResponse"] = {
  acct_id: ACCT_ID,
  amount: "      100.00",
  auth_code: "000001",
  auth_date: "01/31/24  ",
  auth_key: AUTH_KEY,
  auth_time: "10:30:45  ",
  auth_type: "PURC",
  card_expiry: "12/30",
  card_num: MASKED_CARD,
  fraud_status: "N",
  match_status: "P",
  merchant_category_code: "5411",
  merchant_city: "TESTVILLE",
  merchant_id: "000000001",
  merchant_name: "TEST MERCHANT",
  merchant_state: "NC",
  merchant_zip: "00000",
  pos_entry_mode: "01",
  reason: "0000-APPROVED       ",
  response: "A",
  source: "POS",
  transaction_id: "TX0000000000001 ",
};

const fraudToggleReply: components["schemas"]["FraudToggleResult"] = {
  detail: { ...authDetailReply, fraud_status: "F" },
  message: "AUTH MARKED FRAUD...",
};

const tranTypeListState: components["schemas"]["TranTypeListState"] = {
  desc_filter: "PUR",
  first_key: "01",
  last_key: "01",
  last_page_shown: true,
  next_page_exists: false,
  page_no: 1,
  pending_action: "none",
  rows: [{ description: "PURCHASE", type_code: TYPE_CODE }, null],
  screen_rows: [{ description: "PURCHASE", type_code: TYPE_CODE }, null],
  selectors: "open",
  type_filter: "0",
};

const tranTypeShownRow: components["schemas"]["TranTypeListShownRow"] = {
  description: "PURCHASE",
  description_enterable: false,
  highlighted: false,
  selection: "",
  selector_enterable: true,
  type_code: TYPE_CODE,
};

const tranTypeListReply: components["schemas"]["TranTypeListResponse"] = {
  desc_filter: { status: "valid", text: "PUR" },
  has_next_page: false,
  page_key: "01",
  page_no: 1,
  rows: [tranTypeShownRow, null],
  state: tranTypeListState,
  type_filter: { status: "valid", text: "0" },
};

const tranTypeListActionsBody: components["schemas"]["TranTypeListActionRequest"] = {
  aid: "PF10",
  desc_filter: "PUR",
  page_size: IGNORED_PAGE_SIZE,
  rows: [{ description: "PURCHASES", selection: "U" }, {}],
  state: { ...tranTypeListState, pending_action: "update", selected_row: 1 },
  type_filter: "0",
};

const tranTypeListActionsReply: components["schemas"]["TranTypeListActionResult"] = {
  committed: "updated",
  desc_filter: { status: "valid", text: "PUR" },
  has_next_page: false,
  info_message: "Changes committed to database",
  page_key: "01",
  page_no: 1,
  row_errors: [],
  rows: [{ ...tranTypeShownRow, description: "PURCHASES" }, null],
  state: tranTypeListState,
  type_filter: { status: "valid", text: "0" },
};

const tranTypeFetchReply: components["schemas"]["TranTypeView"] = {
  description: "PURCHASE",
  info_message: "Update transaction type details shown above",
  state: "show_details",
  type_code: TYPE_CODE,
};

const tranTypeActionBody: components["schemas"]["TranTypeActionRequest"] = {
  aid: "PF5",
  description: "PURCHASES",
};

const tranTypeActionReply: components["schemas"]["TranTypeActionResult"] = {
  committed: true,
  deleted: false,
  description: "PURCHASES",
  info_message: "Changes committed to database",
  state: "changes_okayed_and_done",
  type_code: TYPE_CODE,
};

/**
 * One row per hook: the request it must send (method, path with its parameters substituted,
 * query as the server receives it, JSON body) and the success body it must resolve to. List
 * calls are handed a page size, which the expected query and body leave out.
 */
const operations = [
  {
    hook: "useCosgn00cSignon",
    method: "POST",
    path: "/api/v1/signon",
    query: {},
    body: signonBody,
    reply: signonReply,
    fire: () => fireMutation(hooks.useCosgn00cSignon, { body: signonBody }),
  },
  {
    hook: "useComen01cMainMenu",
    method: "GET",
    path: "/api/v1/menus/main",
    query: {},
    body: undefined,
    reply: mainMenuReply,
    fire: () => fireQuery(hooks.useComen01cMainMenu),
  },
  {
    hook: "useCoadm01cAdminMenu",
    method: "GET",
    path: "/api/v1/menus/admin",
    query: {},
    body: undefined,
    reply: adminMenuReply,
    fire: () => fireQuery(hooks.useCoadm01cAdminMenu),
  },
  {
    hook: "useCoactvwcView",
    method: "GET",
    path: `/api/v1/accounts/${ACCT_ID}/view`,
    query: {},
    body: undefined,
    reply: accountViewReply,
    fire: () => fireMutation(hooks.useCoactvwcView, { path: { acct_id: ACCT_ID } }),
  },
  {
    hook: "useCoactupcFetch",
    method: "GET",
    path: `/api/v1/accounts/${ACCT_ID}`,
    query: {},
    body: undefined,
    reply: accountFetchReply,
    fire: () => fireMutation(hooks.useCoactupcFetch, { path: { acct_id: ACCT_ID } }),
  },
  {
    hook: "useCoactupcSave",
    method: "PUT",
    path: `/api/v1/accounts/${ACCT_ID}`,
    query: {},
    body: accountSaveBody,
    reply: accountSaveReply,
    fire: () =>
      fireMutation(hooks.useCoactupcSave, { path: { acct_id: ACCT_ID }, body: accountSaveBody }),
  },
  {
    hook: "useCocrdlicList",
    method: "GET",
    path: "/api/v1/cards",
    query: {
      acct_id: ACCT_ID,
      card_num: "0000",
      direction: "forward",
      page_no: "2",
      first_key: "11",
      last_key: "17",
      has_next_page: "true",
      last_page_shown: "false",
      reread: "false",
    },
    body: undefined,
    reply: cardListReply,
    fire: () =>
      fireMutation(hooks.useCocrdlicList, {
        query: {
          acct_id: ACCT_ID,
          card_num: "0000",
          direction: "forward",
          page_no: 2,
          first_key: 11,
          last_key: 17,
          has_next_page: true,
          last_page_shown: false,
          reread: false,
          page_size: IGNORED_PAGE_SIZE,
        },
      }),
  },
  {
    hook: "useCocrdslcView",
    method: "GET",
    path: "/api/v1/cards/view",
    query: { acct_id: ACCT_ID, card_ref: `${CARD_REF}` },
    body: undefined,
    reply: cardViewReply,
    fire: () =>
      fireMutation(hooks.useCocrdslcView, { query: { acct_id: ACCT_ID, card_ref: CARD_REF } }),
  },
  {
    hook: "useCocrdupcFetch",
    method: "GET",
    path: "/api/v1/cards/edit",
    query: { acct_id: ACCT_ID, card_ref: `${CARD_REF}` },
    body: undefined,
    reply: cardFetchReply,
    fire: () =>
      fireMutation(hooks.useCocrdupcFetch, { query: { acct_id: ACCT_ID, card_ref: CARD_REF } }),
  },
  {
    hook: "useCocrdupcSave",
    method: "PUT",
    path: `/api/v1/cards/${CARD_REF}`,
    query: {},
    body: cardSaveBody,
    reply: cardSaveReply,
    fire: () =>
      fireMutation(hooks.useCocrdupcSave, { path: { card_ref: CARD_REF }, body: cardSaveBody }),
  },
  {
    hook: "useCotrn00cList",
    method: "GET",
    path: "/api/v1/transactions",
    query: {
      start_tran_id: TRAN_ID,
      direction: "backward",
      entry_key: "enter",
      first_key: TRAN_ID,
      last_key: TRAN_ID,
      page_no: "3",
      has_next_page: "false",
    },
    body: undefined,
    reply: tranListReply,
    fire: () =>
      fireMutation(hooks.useCotrn00cList, {
        query: {
          start_tran_id: TRAN_ID,
          direction: "backward",
          entry_key: "enter",
          first_key: TRAN_ID,
          last_key: TRAN_ID,
          page_no: 3,
          has_next_page: false,
          page_size: IGNORED_PAGE_SIZE,
        },
      }),
  },
  {
    hook: "useCotrn01cView",
    method: "GET",
    path: `/api/v1/transactions/${TRAN_ID}`,
    query: {},
    body: undefined,
    reply: tranViewReply,
    fire: () => fireMutation(hooks.useCotrn01cView, { path: { tran_id: TRAN_ID } }),
  },
  {
    hook: "useCotrn02cAdd",
    method: "POST",
    path: "/api/v1/transactions",
    query: {},
    body: tranAddBody,
    reply: tranAddReply,
    fire: () => fireMutation(hooks.useCotrn02cAdd, { body: tranAddBody }),
  },
  {
    hook: "useCorpt00cSubmit",
    method: "POST",
    path: "/api/v1/reports/transactions",
    query: {},
    body: reportBody,
    reply: reportReply,
    fire: () => fireMutation(hooks.useCorpt00cSubmit, { body: reportBody }),
  },
  {
    hook: "useCobil00cPay",
    method: "POST",
    path: "/api/v1/bill-payments",
    query: {},
    body: billPaymentBody,
    reply: billPaymentReply,
    fire: () => fireMutation(hooks.useCobil00cPay, { body: billPaymentBody }),
  },
  {
    hook: "useCousr00cList",
    method: "GET",
    path: "/api/v1/users",
    query: {
      start_user_id: "USER0001",
      page_key: "USER0010",
      direction: "forward",
      first_key: "ADMIN001",
      last_key: "USER0010",
      page_no: "1",
      has_next_page: "true",
    },
    body: undefined,
    reply: userListReply,
    fire: () =>
      fireMutation(hooks.useCousr00cList, {
        query: {
          start_user_id: "USER0001",
          page_key: "USER0010",
          direction: "forward",
          first_key: "ADMIN001",
          last_key: "USER0010",
          page_no: 1,
          has_next_page: true,
          page_size: IGNORED_PAGE_SIZE,
        },
      }),
  },
  {
    hook: "useCousr01cAdd",
    method: "POST",
    path: "/api/v1/users",
    query: {},
    body: userAddBody,
    reply: userAddReply,
    fire: () => fireMutation(hooks.useCousr01cAdd, { body: userAddBody }),
  },
  {
    hook: "useCousr02cFetch",
    method: "GET",
    path: `/api/v1/users/${USER_ID}`,
    query: {},
    body: undefined,
    reply: userFetchReply,
    fire: () => fireMutation(hooks.useCousr02cFetch, { path: { user_id: USER_ID } }),
  },
  {
    hook: "useCousr02cUpdate",
    method: "PUT",
    path: `/api/v1/users/${USER_ID}`,
    query: {},
    body: userUpdateBody,
    reply: userUpdateReply,
    fire: () =>
      fireMutation(hooks.useCousr02cUpdate, { path: { user_id: USER_ID }, body: userUpdateBody }),
  },
  {
    hook: "useCousr03cDelete",
    method: "DELETE",
    path: `/api/v1/users/${USER_ID}`,
    query: {},
    body: undefined,
    reply: userDeleteReply,
    fire: () => fireMutation(hooks.useCousr03cDelete, { path: { user_id: USER_ID } }),
  },
  {
    hook: "useCopaus0cSummary",
    method: "GET",
    path: `/api/v1/accounts/${ACCT_ID}/authorizations`,
    query: { page_key: PAGE_AUTH_KEY, direction: "backward" },
    body: undefined,
    reply: authSummaryReply,
    fire: () =>
      fireMutation(hooks.useCopaus0cSummary, {
        path: { acct_id: ACCT_ID },
        query: { page_key: PAGE_AUTH_KEY, direction: "backward", page_size: IGNORED_PAGE_SIZE },
      }),
  },
  {
    hook: "useCopaus1cDetail",
    method: "GET",
    path: `/api/v1/authorizations/${ACCT_ID}/${AUTH_KEY}`,
    query: { direction: "forward" },
    body: undefined,
    reply: authDetailReply,
    fire: () =>
      fireMutation(hooks.useCopaus1cDetail, {
        path: { acct_id: ACCT_ID, auth_key: AUTH_KEY },
        query: { direction: "forward" },
      }),
  },
  {
    hook: "useCopaus1cToggleFraud",
    method: "POST",
    path: `/api/v1/authorizations/${ACCT_ID}/${AUTH_KEY}/fraud`,
    query: {},
    body: undefined,
    reply: fraudToggleReply,
    fire: () =>
      fireMutation(hooks.useCopaus1cToggleFraud, {
        path: { acct_id: ACCT_ID, auth_key: AUTH_KEY },
      }),
  },
  {
    hook: "useCotrtlicList",
    method: "GET",
    path: "/api/v1/transaction-types",
    query: { type_filter: "0", desc_filter: "PUR", page_key: "01", direction: "forward" },
    body: undefined,
    reply: tranTypeListReply,
    fire: () =>
      fireMutation(hooks.useCotrtlicList, {
        query: {
          type_filter: "0",
          desc_filter: "PUR",
          page_key: "01",
          direction: "forward",
          page_size: IGNORED_PAGE_SIZE,
        },
      }),
  },
  {
    hook: "useCotrtlicListActions",
    method: "POST",
    path: "/api/v1/transaction-types/list-actions",
    query: {},
    body: {
      aid: tranTypeListActionsBody.aid,
      desc_filter: tranTypeListActionsBody.desc_filter,
      rows: tranTypeListActionsBody.rows,
      state: tranTypeListActionsBody.state,
      type_filter: tranTypeListActionsBody.type_filter,
    },
    reply: tranTypeListActionsReply,
    fire: () => fireMutation(hooks.useCotrtlicListActions, { body: tranTypeListActionsBody }),
  },
  {
    hook: "useCotrtupcFetch",
    method: "GET",
    path: `/api/v1/transaction-types/${TYPE_CODE}`,
    query: {},
    body: undefined,
    reply: tranTypeFetchReply,
    fire: () => fireMutation(hooks.useCotrtupcFetch, { path: { type_code: TYPE_CODE } }),
  },
  {
    hook: "useCotrtupcAction",
    method: "POST",
    path: `/api/v1/transaction-types/${TYPE_CODE}/actions`,
    query: {},
    body: tranTypeActionBody,
    reply: tranTypeActionReply,
    fire: () =>
      fireMutation(hooks.useCotrtupcAction, {
        path: { type_code: TYPE_CODE },
        body: tranTypeActionBody,
      }),
  },
];

beforeEach(() => {
  setSession(TOKEN, "USER0001", "U");
});

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("every hook", () => {
  it("has one row per exported hook, each for its own one of the 27 operations", () => {
    const names = operations.map(({ hook }) => hook);

    expect([...names].sort()).toEqual(Object.keys(hooks).sort());
    expect(new Set(names).size).toBe(27);
    expect(new Set(operations.map(({ method, path }) => `${method} ${path}`)).size).toBe(27);
  });

  it.each(operations)(
    "$hook sends $method $path with the bearer token and resolves to the success body",
    async ({ method, path, query, body, reply, fire }) => {
      const seen = stubFetch((request) =>
        request.method === method && new URL(request.url).pathname === path
          ? json(200, reply)
          : json(404, { message: `no route for ${request.method} ${request.url}` }),
      );

      const outcome = await fire();

      expect(outcome).toEqual({ data: reply, error: null });
      expect(seen).toEqual([
        { method, path, query, authorization: `Bearer ${TOKEN}`, body },
      ]);
    },
  );
});

describe("the bearer token", () => {
  it("is not sent while nobody is signed on", async () => {
    sessionStorage.clear();
    const seen = stubFetch(() => json(200, signonReply));

    const outcome = await fireMutation(hooks.useCosgn00cSignon, { body: signonBody });

    expect(outcome).toEqual({ data: signonReply, error: null });
    expect(seen).toHaveLength(1);
    expect(seen[0]?.authorization).toBeNull();
  });
});

describe("a failure with a ScreenError body", () => {
  it.each([
    { status: 400, field: "acct_id", message: "Account number not provided", session: "kept" },
    {
      status: 401,
      field: null,
      message: "Sign-on required. Please sign on ...",
      session: "cleared",
    },
    { status: 403, field: null, message: "No access - Admin Only function ...", session: "kept" },
    {
      status: 404,
      field: "acct_id",
      message: "Did not find this account in account master file",
      session: "kept",
    },
    {
      status: 409,
      field: null,
      message: "Record changed by some one else. Please review",
      session: "kept",
    },
    { status: 500, field: null, message: "Error reading account card xref File", session: "kept" },
  ])(
    "rejects a $status with that body unchanged and leaves the session $session",
    async ({ status, field, message, session }) => {
      const screenError: components["schemas"]["ScreenError"] = { field, message };
      const seen = stubFetch(() => json(status, screenError));

      const outcome = await fireMutation(hooks.useCoactvwcView, { path: { acct_id: ACCT_ID } });

      expect(outcome).toEqual({ data: undefined, error: screenError });
      expect(seen[0]?.authorization).toBe(`Bearer ${TOKEN}`);
      expect(getSession() === null ? "cleared" : "kept").toBe(session);
    },
  );

  it("rejects a query hook with that body unchanged", async () => {
    const screenError: components["schemas"]["ScreenError"] = {
      message: "No access - Admin Only function ...",
    };
    stubFetch(() => json(403, screenError));

    const outcome = await fireQuery(hooks.useCoadm01cAdminMenu);

    expect(outcome).toEqual({ data: undefined, error: screenError });
  });
});

/*
 * openapi-fetch reports an error status as bodiless only when it is 204 or declares
 * `Content-Length: 0` without chunked encoding. It reads any other error body as text and parses
 * it as JSON where it can, so an empty body that declares no length, or arrives chunked, comes
 * back as "", a plain-text body as its text and a JSON `null` as `null`. None of these is a JSON
 * object, so each rejects with the same status `Error` as a bodiless reply.
 */
describe("a reply without a JSON object body", () => {
  it.each([
    {
      reply: "an empty 500 of Content-Length 0",
      response: () =>
        new Response(null, {
          status: 500,
          statusText: "Internal Server Error",
          headers: { "Content-Length": "0" },
        }),
      message: "HTTP 500 Internal Server Error",
    },
    {
      reply: "an empty 502 without a reason phrase",
      response: () => new Response(null, { status: 502, headers: { "Content-Length": "0" } }),
      message: "HTTP 502",
    },
    {
      reply: "an empty 500 without Content-Length",
      response: () => new Response(null, { status: 500, statusText: "Internal Server Error" }),
      message: "HTTP 500 Internal Server Error",
    },
    {
      reply: "the dev proxy's empty, chunked 500",
      response: () =>
        new Response(null, {
          status: 500,
          statusText: "Internal Server Error",
          headers: { "Content-Type": "text/plain", "Transfer-Encoding": "chunked" },
        }),
      message: "HTTP 500 Internal Server Error",
    },
    {
      reply: "a plain-text 502",
      response: () =>
        new Response("Bad Gateway", {
          status: 502,
          statusText: "Bad Gateway",
          headers: { "Content-Type": "text/plain" },
        }),
      message: "HTTP 502 Bad Gateway",
    },
    {
      reply: "a 500 whose JSON body is null",
      response: () => json(500, null),
      message: "HTTP 500",
    },
    {
      reply: "a 204",
      response: () => new Response(null, { status: 204, statusText: "No Content" }),
      message: "HTTP 204 No Content",
    },
    {
      reply: "an empty 200",
      response: () => new Response(null, { status: 200, statusText: "OK" }),
      message: "HTTP 200 OK",
    },
  ])("rejects $reply with an Error naming the status", async ({ response, message }) => {
    stubFetch(response);

    const outcome = await fireMutation(hooks.useCobil00cPay, { body: billPaymentBody });

    expect(outcome.data).toBeUndefined();
    expect(outcome.error).toBeInstanceOf(Error);
    expect(outcome.error?.message).toBe(message);
  });
});
