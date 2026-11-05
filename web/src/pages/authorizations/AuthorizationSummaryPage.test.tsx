// FN tests of the CPVS page (pages/authorizations/AuthorizationSummaryPage.tsx,
// COPAUS0C with map COPAU00): ENTER, the "Search" submit of the "Account ID"
// textbox (F-COPAU00-001), requests the account's summary; an account the
// PROCESS-ENTER-KEY edits reject (V-COPAUS0C-001, V-COPAUS0C-002) is shown on
// the field and requested from nobody; a rejection from the server puts its
// message in a toast and on ACCTID, with the cursor there, as
// `MOVE -1 TO ACCTIDL OF COPAU0AI` does.
//
// While a read of the list is in flight (the first read, PF8 to a page not
// read yet, ENTER again on the account shown, PF8 back to a page already
// read), the list is busy: Skeleton rows instead of any held row, Previous and
// Next disabled, and nothing announced until the read settles.
//
// The network is the global the API client looks up on every request, stubbed
// per test; every answer is typed with the OpenAPI schema, so drift in the
// response or error shapes fails `tsc`.
import { onlineManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
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

// COPAUS0C sends every screen with the cursor on ACCTID, `MOVE -1 TO ACCTIDL
// OF COPAU0AI` [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L188, L196]:
// the empty first entry, which reads nothing, and an entry with an account in
// the URL, whose summary is read with the cursor left on the field.
describe("CPVS on arrival", () => {
  it.each([
    { arrival: "the empty first entry", account: null },
    { arrival: "an account in the URL", account: ACCOUNT_1 },
  ])("puts the cursor on ACCTID for $arrival", async ({ account }) => {
    const fetchMock = stubApi(200, summaryOk);
    renderPage(account === null ? "/authorizations" : `/authorizations?account_id=${account}`);

    expect(document.activeElement).toBe(accountField());

    if (account === null) {
      expect(fetchMock).not.toHaveBeenCalled();
    } else {
      expect(
        (await screen.findByRole("heading", { name: `Account ${account}` })).tagName,
      ).toBe("H2");
      expect(fetchMock).toHaveBeenCalledTimes(1);
      expect(new URL(firstRequest(fetchMock).url).searchParams.get("account_id")).toBe(account);
      expect(document.activeElement).toBe(accountField());
    }
    expect(accountField().getAttribute("aria-invalid")).toBe("false");
    expect(toastError).not.toHaveBeenCalled();
  });
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

/** An authorization of account 1's list, told apart by its identity and transaction ID. */
function listedAuthorization(authId: string): AuthSummaryItem {
  return { ...AUTHORIZATION, auth_id: authId, pa_transaction_id: `22070112000000${authId}` };
}

/** Account 1 with seven approved authorizations, as the summary segment counts them. */
const sevenAuthorizations: AuthSummaryResponse = {
  ...summaryOk,
  pa_approved_auth_cnt: 7,
  pa_approved_auth_amt: "353.29",
  pa_credit_balance: "353.29",
};

/** The first five, newest first: ENTER, and PF7 from the second page's `prev_key`. */
const firstPage: AuthSummaryResponse = {
  ...sevenAuthorizations,
  items: ["7", "6", "5", "4", "3"].map(listedAuthorization),
  next_key: "3",
  prev_key: null,
};

/** The last two: PF8 from the first page's `next_key`. */
const secondPage: AuthSummaryResponse = {
  ...sevenAuthorizations,
  items: ["2", "1"].map(listedAuthorization),
  next_key: null,
  prev_key: "2",
};

/** The page a request's cursor names: PF8 from key "3" is the second page, any other the first. */
function pageAt(request: Request): AuthSummaryResponse {
  const query = new URL(request.url).searchParams;
  return query.get("start_key") === "3" && query.get("direction") === "forward"
    ? secondPage
    : firstPage;
}

/**
 * Answers each request with the page its cursor names. `holdNextRead` keeps
 * the answer to the next request back until the function it returns is
 * called, so that read stays in flight in between.
 */
function stubPages() {
  let held: Promise<void> | null = null;
  const fetchMock = vi.fn(async (input: Request | string | URL, _init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input);
    const hold = held;
    held = null;
    if (hold !== null) {
      await hold;
    }
    return jsonResponse(200, pageAt(request));
  });
  vi.stubGlobal("fetch", fetchMock);
  const holdNextRead = (): (() => void) => {
    let release = (): void => undefined;
    held = new Promise<void>((resolve) => {
      release = () => {
        resolve();
      };
    });
    return release;
  };
  return { fetchMock, holdNextRead };
}

/** The request of the mock's last call. */
function lastRequest(fetchMock: FetchMock): Request {
  const call = fetchMock.mock.lastCall;
  if (call === undefined) {
    throw new Error("no request was sent");
  }
  const [input] = call;
  return input instanceof Request ? input : new Request(input);
}

/** The pending-authorization list. */
function list(): HTMLElement {
  return screen.getByRole("table");
}

/** The rows of the list's body. */
function bodyRows(): HTMLTableRowElement[] {
  return Array.from(list().querySelectorAll<HTMLTableRowElement>("tbody tr"));
}

/** The list's polite status region, which announces each completed page. */
function listStatus(): HTMLElement {
  const card = list().closest<HTMLElement>('[data-slot="card"]');
  if (card === null) {
    throw new Error("the list is not inside its card");
  }
  return within(card).getByRole("status");
}

/** The Previous (PF7) or Next (PF8) control. */
function pagingControl(name: "Previous" | "Next"): HTMLElement {
  return screen.getByRole("button", { name });
}

/** ENTER on the empty field, ENTER again with the Search button, PF7 or PF8. */
type Press = "Enter" | "Search" | "Previous" | "Next";

/** The page each press reads from the page it is made on, over the two pages above. */
const READS: Record<Press, AuthSummaryResponse> = {
  Enter: firstPage,
  Search: firstPage,
  Previous: firstPage,
  Next: secondPage,
};

async function press(user: ReturnType<typeof userEvent.setup>, control: Press): Promise<void> {
  if (control === "Enter") {
    await user.type(accountField(), `${ACCOUNT_1}{Enter}`);
  } else if (control === "Search") {
    await user.click(searchButton());
  } else {
    await user.click(pagingControl(control));
  }
}

/** Longer than the delay after which DataTable announces a completed page. */
const ANNOUNCE_WAIT_MS = 200;

/** Lets `ms` pass on the real clock. */
async function elapse(ms: number): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => {
      setTimeout(resolve, ms);
    });
  });
}

/** The list settled on `page`: its rows, no busy state, its paging keys, and the status naming it. */
async function expectSettledOn(page: AuthSummaryResponse): Promise<void> {
  for (const item of page.items) {
    expect((await screen.findByText(item.pa_transaction_id)).tagName).toBe("SPAN");
  }
  await waitFor(() =>
    expect(listStatus().textContent).toBe(`Pending authorizations: ${page.items.length} rows`),
  );
  expect(list().hasAttribute("aria-busy")).toBe(false);
  expect(bodyRows().map((row) => row.getAttribute("aria-hidden"))).toEqual(
    page.items.map(() => null),
  );
  expect(pagingControl("Previous").getAttribute("aria-disabled")).toBe(
    page.prev_key === null ? "true" : null,
  );
  expect(pagingControl("Next").getAttribute("aria-disabled")).toBe(
    page.next_key === null ? "true" : null,
  );
}

/**
 * The list while a read is in flight, checked again after the announcement
 * delay has passed: busy, `skeletonRows` Skeleton rows without content in
 * place of any held row, Previous and Next disabled, and the status empty.
 */
async function expectBusy(skeletonRows: number): Promise<void> {
  await waitFor(() => expect(list().getAttribute("aria-busy")).toBe("true"));
  await elapse(ANNOUNCE_WAIT_MS);
  expect(list().getAttribute("aria-busy")).toBe("true");
  const rows = bodyRows();
  expect(rows).toHaveLength(skeletonRows);
  for (const row of rows) {
    expect(row.getAttribute("aria-hidden")).toBe("true");
    expect(row.textContent).toBe("");
    expect(row.querySelector('[data-slot="skeleton"]')).not.toBeNull();
  }
  for (const item of [...firstPage.items, ...secondPage.items]) {
    expect(screen.queryByText(item.pa_transaction_id)).toBeNull();
  }
  expect(pagingControl("Previous").getAttribute("aria-disabled")).toBe("true");
  expect(pagingControl("Next").getAttribute("aria-disabled")).toBe("true");
  expect(listStatus().textContent).toBe("");
}

describe("CPVS while a read of the list is in flight", () => {
  afterEach(() => {
    onlineManager.setOnline(true);
  });

  it.each<{
    read: string;
    /** The presses made, each read to completion, before the one whose read is held. */
    before: readonly Press[];
    held: Press;
    /** The cursor of the held request. */
    startKey: string | null;
    direction: string | null;
    /** The rows held while it is in flight: none (5 Skeleton rows), the page left, or the cached page. */
    skeletonRows: number;
    /** Whether the account header is already shown, from an earlier read. */
    header: boolean;
  }>([
    {
      read: "the first read of the account (ENTER)",
      before: [],
      held: "Enter",
      startKey: null,
      direction: null,
      skeletonRows: 5,
      header: false,
    },
    {
      read: "Next to a page not read yet (PF8)",
      before: ["Enter"],
      held: "Next",
      startKey: "3",
      direction: "forward",
      skeletonRows: firstPage.items.length,
      header: true,
    },
    {
      read: "a Search of the account already shown (ENTER again)",
      before: ["Enter"],
      held: "Search",
      startKey: null,
      direction: null,
      skeletonRows: firstPage.items.length,
      header: true,
    },
    {
      read: "Next back to a page already read, after Previous (PF8 after PF7)",
      before: ["Enter", "Next", "Previous"],
      held: "Next",
      startKey: "3",
      direction: "forward",
      skeletonRows: secondPage.items.length,
      header: true,
    },
  ])(
    "shows $read as loading until its answer arrives",
    async ({ before, held, startKey, direction, skeletonRows, header }) => {
      const { fetchMock, holdNextRead } = stubPages();
      const user = userEvent.setup();
      renderPage();
      for (const control of before) {
        await press(user, control);
        await expectSettledOn(READS[control]);
      }

      const release = holdNextRead();
      await press(user, held);

      await expectBusy(skeletonRows);
      expect(fetchMock).toHaveBeenCalledTimes(before.length + 1);
      const query = new URL(lastRequest(fetchMock).url).searchParams;
      expect(query.get("account_id")).toBe(ACCOUNT_1);
      expect(query.get("start_key")).toBe(startKey);
      expect(query.get("direction")).toBe(direction);
      expect(screen.queryByRole("heading", { name: `Account ${ACCOUNT_1}` }) !== null).toBe(
        header,
      );

      // Neither paging control requests anything while the read is pending.
      await user.click(pagingControl("Previous"));
      await user.click(pagingControl("Next"));
      expect(fetchMock).toHaveBeenCalledTimes(before.length + 1);

      act(() => {
        release();
      });
      await expectSettledOn(READS[held]);
      expect(screen.getByRole("heading", { name: `Account ${ACCOUNT_1}` }).tagName).toBe("H2");
      expect(fetchMock).toHaveBeenCalledTimes(before.length + 1);
      expect(toastError).not.toHaveBeenCalled();
    },
  );

  it("keeps Next's read loading while it waits offline, then shows the page", async () => {
    const { fetchMock } = stubPages();
    const user = userEvent.setup();
    renderPage();
    await press(user, "Enter");
    await expectSettledOn(firstPage);

    onlineManager.setOnline(false);
    await press(user, "Next");

    // The read is paused before its request: the page left is held, unsent.
    await expectBusy(firstPage.items.length);
    expect(fetchMock).toHaveBeenCalledTimes(1);

    act(() => {
      onlineManager.setOnline(true);
    });
    await expectSettledOn(secondPage);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(new URL(lastRequest(fetchMock).url).searchParams.get("start_key")).toBe("3");
  });
});

/** Previous and Next, and every row of either page, are gone from the screen. */
function expectNoListShown(): void {
  expect(screen.queryByRole("table")).toBeNull();
  expect(screen.queryByRole("button", { name: "Previous" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Next" })).toBeNull();
  for (const item of [...firstPage.items, ...secondPage.items]) {
    expect(screen.queryByText(item.pa_transaction_id)).toBeNull();
  }
}

/**
 * A page already shown, read again: ENTER on the account shown, or PF8 back
 * to a page read before. `heldHeader` tells whether the Search that follows
 * the failure holds a first page read earlier, busy, while it reads: only
 * when the failure was another page's.
 */
const REREADS: ReadonlyArray<{
  read: string;
  before: readonly Press[];
  failed: Press;
  heldHeader: boolean;
}> = [
  {
    read: "a Search of the account shown (ENTER again)",
    before: ["Enter"],
    failed: "Search",
    heldHeader: false,
  },
  {
    read: "Next back to a page already read (PF8 after PF7)",
    before: ["Enter", "Next", "Previous"],
    failed: "Next",
    heldHeader: true,
  },
];

/** The server's rejections of account 1 a read again can meet (V-COPAUS0C-004, V-COPAUS0C-005). */
const REREAD_FAILURES: ReadonlyArray<{ row: string; status: number; code: string; message: string }> = [
  {
    row: "V-COPAUS0C-004",
    status: 404,
    code: "NOT_FOUND",
    message: "Account:00000000001 not found in XREF file. Resp:000000013 Reas:000000080",
  },
  {
    row: "V-COPAUS0C-005",
    status: 500,
    code: "UNEXPECTED",
    message: "Account:00000000001 System error while reading XREF file. Resp:000000017 Reas:00",
  },
];

// The answer to a read again is the screen's result, and a failed one is an
// ErrorBody, not a refreshed page: the header, rows and paging of the page
// read before are withheld, as on a failed first read, and the search form's
// field and one toast carry the message, with PF3 Back still offered. A
// Search then reads the account again, loading as a first read does, and its
// page brings the header and list back.
describe("CPVS when a read again of a page already shown fails", () => {
  it.each(REREADS.flatMap((reread) => REREAD_FAILURES.map((failure) => ({ ...reread, ...failure }))))(
    "$row ($status) on $read withholds the page read before until a Search succeeds",
    async ({ before, failed, heldHeader, status, code, message }) => {
      let failure: ErrorBody | null = null;
      let held: Promise<void> | null = null;
      const fetchMock = vi.fn(async (input: Request | string | URL, _init?: RequestInit) => {
        const request = input instanceof Request ? input : new Request(input);
        if (held !== null) {
          await held;
        }
        return failure === null ? jsonResponse(200, pageAt(request)) : jsonResponse(status, failure);
      });
      vi.stubGlobal("fetch", fetchMock);
      const user = userEvent.setup();
      renderPage();
      for (const control of before) {
        await press(user, control);
        await expectSettledOn(READS[control]);
      }

      failure = { code, message, fields: [{ field: ACCTID, message }] };
      await press(user, failed);

      await waitFor(() => expect(toastError).toHaveBeenCalledWith(message));
      await waitFor(() => expect(screen.queryByRole("table")).toBeNull());
      await elapse(ANNOUNCE_WAIT_MS);
      expectNoListShown();
      expect(screen.queryByRole("heading", { name: `Account ${ACCOUNT_1}` })).toBeNull();
      expect(document.querySelector('[data-slot="skeleton"]')).toBeNull();
      const alert = await screen.findByRole("alert");
      expect(alert.textContent).toBe(message);
      expect(accountField().getAttribute("aria-invalid")).toBe("true");
      expect(screen.getByRole("link", { name: "Back" }).getAttribute("href")).toBe("/");
      expect(toastError).toHaveBeenCalledTimes(1);
      expect(fetchMock).toHaveBeenCalledTimes(before.length + 1);

      failure = null;
      let release = (): void => undefined;
      held = new Promise<void>((resolve) => {
        release = () => {
          resolve();
        };
      });
      await press(user, "Search");

      await expectBusy(firstPage.items.length);
      expect(screen.queryByRole("heading", { name: `Account ${ACCOUNT_1}` }) !== null).toBe(
        heldHeader,
      );
      act(() => {
        release();
      });
      await expectSettledOn(firstPage);
      expect(screen.getByRole("heading", { name: `Account ${ACCOUNT_1}` }).tagName).toBe("H2");
      expect(screen.queryByRole("alert")).toBeNull();
      expect(toastError).toHaveBeenCalledTimes(1);
      expect(fetchMock).toHaveBeenCalledTimes(before.length + 2);
      const query = new URL(lastRequest(fetchMock).url).searchParams;
      expect(query.get("account_id")).toBe(ACCOUNT_1);
      expect(query.get("start_key")).toBeNull();
    },
  );
});
