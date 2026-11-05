// Tests of the rewards screen (pages/rewards/RewardsPage.tsx, route /rewards):
// - the account lookup accepts an 11-digit account number and rejects every
//   other entry with its field message before any request is sent (FN);
// - the balance cards and the history table show the API's decimal integer
//   strings exactly as sent, 9007199254740993 included, which a JavaScript
//   number would read as 9007199254740992 (o8);
// - an unknown account is reported by one toast and leaves the form usable;
// - an account that has earned no points shows the zero balance and the
//   empty-history text, and a failed history read is reported by one toast
//   and shows no history rows or page controls;
// - the history pages forward with Next and back with Previous, both reached
//   and pressed from the keyboard;
// - a repeated lookup of the same account keeps the history table loading
//   until its new answer arrives, from the newest page or a later one, while
//   a page Previous brings back from the cache keeps its rows as it is reread;
// - a lookup shows skeletons, never a figure or row the cache or the previous
//   account holds, until its own answers arrive, and Look up pressed again
//   while the balance is read requests no history of an empty account;
// - an entry rejected after a lookup takes its balance and history off the
//   screen;
// - the page's status region announces each balance lookup, a repeated one
//   included, by its account and never by a points value, and stays empty for
//   a failed one.
//
// The network is the stubbed global the API client looks up on every request,
// answered from the `balances` and `histories` tables below. A request neither
// table answers gets a 500 and fails the test in afterEach.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { components, paths } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import {
  rewardsAccountBmsFieldMap,
  rewardsAccountSchema,
} from "@/schemas/rewards";

import RewardsPage from "./RewardsPage";

type RewardBalance = components["schemas"]["RewardBalanceResponse"];

type RewardHistoryPage =
  paths["/rewards/history/{account_id}"]["get"]["responses"][200]["content"]["application/json"];

type RewardAccrual = RewardHistoryPage["items"][number];

type ErrorBody = components["schemas"]["ErrorBody"];

/** One stubbed answer: its HTTP status and JSON body. */
type Reply = {
  status: number;
  body: RewardBalance | RewardHistoryPage | ErrorBody;
};

/** The first account of app/data/ASCII/acctdata.txt. */
const ACCOUNT = "00000000001";

/** An account number the form accepts and the server has no account for. */
const UNKNOWN_ACCOUNT = "99999999999";

/** 2^53 + 1, the smallest integer a JavaScript number cannot hold exactly. */
const BEYOND_DOUBLE = "9007199254740993";

/** The status region's texts while the balance is read and once it is. */
const BALANCE_LOADING = "Loading reward balance";
const BALANCE_LOADED = `Reward balance of account ${ACCOUNT} loaded`;

const balanceBody: RewardBalance = {
  account_id: ACCOUNT,
  total_points_earned: BEYOND_DOUBLE,
  total_points_redeemed: "0",
  available_points: BEYOND_DOUBLE,
};

function accrual(
  accrual_id: string,
  transaction_id: string,
  points_earned: string,
  account_id: string = ACCOUNT,
): RewardAccrual {
  return {
    account_id,
    accrual_id,
    transaction_id,
    rule_id: "1",
    points_earned,
    created_at: "2022-07-01T12:00:00Z",
  };
}

// The newest history page. `start_key` is inclusive, so the next page starts
// at the accrual after this page's last one.
const PAGE_ONE: RewardHistoryPage = {
  items: [
    accrual("43", "250", "9007199254740995"),
    accrual("42", "249", "100"),
  ],
  next_key: "41",
  prev_key: null,
};

// The page read with PAGE_ONE's next_key, the last one.
const PAGE_TWO: RewardHistoryPage = {
  items: [accrual("41", "248", "57"), accrual("40", "247", "1")],
  next_key: null,
  prev_key: null,
};

// The newest page read again once one more accrual has been earned on top of
// PAGE_ONE's.
const PAGE_ONE_REREAD: RewardHistoryPage = {
  items: [accrual("44", "251", "25"), ...PAGE_ONE.items],
  next_key: "41",
  prev_key: null,
};

// ACCOUNT's balance read again once the accrual PAGE_ONE_REREAD adds (25
// points) has been earned.
const BEYOND_DOUBLE_PLUS_25 = "9007199254741018";
const balanceRereadBody: RewardBalance = {
  account_id: ACCOUNT,
  total_points_earned: BEYOND_DOUBLE_PLUS_25,
  total_points_redeemed: "0",
  available_points: BEYOND_DOUBLE_PLUS_25,
};

/** The second account of app/data/ASCII/acctdata.txt. */
const OTHER_ACCOUNT = "00000000002";

const otherBalanceBody: RewardBalance = {
  account_id: OTHER_ACCOUNT,
  total_points_earned: "57",
  total_points_redeemed: "0",
  available_points: "57",
};

// OTHER_ACCOUNT's only history page.
const OTHER_PAGE: RewardHistoryPage = {
  items: [accrual("39", "246", "57", OTHER_ACCOUNT)],
  next_key: null,
  prev_key: null,
};

/** An account on file that has earned no reward points. */
const ZERO_ACCOUNT = "00000000037";

// The zero balance the server answers for an account on file without
// accruals, and its empty history (AAP 0.3.5).
const zeroBalanceBody: RewardBalance = {
  account_id: ZERO_ACCOUNT,
  total_points_earned: "0",
  total_points_redeemed: "0",
  available_points: "0",
};

const EMPTY_PAGE: RewardHistoryPage = {
  items: [],
  next_key: null,
  prev_key: null,
};

/** The text the history table shows in place of the rows of an empty history. */
const EMPTY_HISTORY = "No reward points have been earned on this account.";

// The server's answer for an account not on file (crates/api/src/error.rs
// maps NotFound to 404 with this code).
const accountNotFound: ErrorBody = {
  code: "NOT_FOUND",
  message: "Account not found",
  fields: [],
};

const unexpectedError: ErrorBody = {
  code: "UNEXPECTED",
  message: "Unexpected error",
  fields: [],
};

// The stubbed API: balances by account number, history pages by account
// number and start key (historyKey).
const balances = new Map<string, Reply>();
const histories = new Map<string, Reply>();

// Every request no table answered, as "METHOD path?query".
const unexpected: string[] = [];

// While an endpoint's gate is set, each of its requests waits for that
// promise before it is answered from its table, so a test sees the page while
// the read runs.
const gates: {
  balance: Promise<void> | null;
  history: Promise<void> | null;
} = { balance: null, history: null };

function historyKey(accountId: string, startKey: string | null = null): string {
  return `${accountId} ${startKey ?? "(newest)"}`;
}

const BALANCE_PATH = /^\/api\/rewards\/balance\/([^/]+)$/;
const HISTORY_PATH = /^\/api\/rewards\/history\/([^/]+)$/;

function asRequest(input: RequestInfo | URL, init?: RequestInit): Request {
  return input instanceof Request ? input : new Request(input, init);
}

function describeRequest(request: Request): string {
  const url = new URL(request.url);
  return `${request.method} ${url.pathname}${url.search}`;
}

function answer(request: Request): Reply | undefined {
  if (request.method !== "GET") {
    return undefined;
  }
  const url = new URL(request.url);
  const balance = BALANCE_PATH.exec(url.pathname);
  if (balance !== null) {
    return balances.get(decodeURIComponent(balance[1]));
  }
  const history = HISTORY_PATH.exec(url.pathname);
  if (history !== null) {
    return histories.get(
      historyKey(
        decodeURIComponent(history[1]),
        url.searchParams.get("start_key"),
      ),
    );
  }
  return undefined;
}

// The gate `request` waits on: its endpoint's, null when none is set.
function gateOf(request: Request): Promise<void> | null {
  const { pathname } = new URL(request.url);
  if (BALANCE_PATH.test(pathname)) {
    return gates.balance;
  }
  if (HISTORY_PATH.test(pathname)) {
    return gates.history;
  }
  return null;
}

function jsonResponse({ status, body }: Reply): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

const fetchMock = vi.fn(
  async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const request = asRequest(input, init);
    const gate = gateOf(request);
    if (gate !== null) {
      await gate;
    }
    const reply = answer(request);
    if (reply === undefined) {
      unexpected.push(describeRequest(request));
      return jsonResponse({ status: 500, body: unexpectedError });
    }
    return jsonResponse(reply);
  },
);

/** Every request sent so far, in order, as "METHOD path?query". */
function requestedPaths(): string[] {
  return fetchMock.mock.calls.map(([input, init]) =>
    describeRequest(asRequest(input, init)),
  );
}

/** The requests sent so far without their query strings. */
function requestedPathnames(): string[] {
  return requestedPaths().map((sent) => sent.split("?")[0]);
}

/** The `start_key` of every history request so far; null for none. */
function historyStartKeys(): Array<string | null> {
  return fetchMock.mock.calls
    .map(([input, init]) => new URL(asRequest(input, init).url))
    .filter((url) => HISTORY_PATH.test(url.pathname))
    .map((url) => url.searchParams.get("start_key"));
}

function renderPage(): UserEvent {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const router = createMemoryRouter(
    [
      {
        path: "/rewards",
        element: <RewardsPage />,
        handle: { title: "Rewards", crumb: "Rewards" } satisfies RouteHandle,
      },
    ],
    { initialEntries: ["/rewards"] },
  );
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return userEvent.setup();
}

function accountInput(): HTMLInputElement {
  return screen.getByRole<HTMLInputElement>("textbox", {
    name: "Account number",
  });
}

// Replaces the account number with `entry` and submits it with Enter, as a
// keyboard user does.
async function lookUp(user: UserEvent, entry: string): Promise<void> {
  const input = accountInput();
  await user.clear(input);
  if (entry !== "") {
    await user.type(input, entry);
  }
  await user.keyboard("{Enter}");
}

function summaryCard(title: string): HTMLElement {
  return screen.getByRole("group", { name: title });
}

/** The cell texts of the history table's body rows, top to bottom. */
function historyRows(): string[][] {
  const table = screen.getByRole("table", { name: "Reward history" });
  return Array.from(table.querySelectorAll("tbody tr"), (row) =>
    Array.from(row.querySelectorAll("td"), (cell) => cell.textContent ?? ""),
  );
}

/** A page's rows as the table shows them, in the page's column order. */
function rowsOf(page: RewardHistoryPage): string[][] {
  return page.items.map((item) => [
    item.accrual_id,
    item.transaction_id,
    item.rule_id,
    item.points_earned,
    item.created_at,
  ]);
}

function pageControl(name: "Previous" | "Next"): HTMLElement {
  return screen.getByRole("button", { name });
}

function isDisabled(control: HTMLElement): boolean {
  return control.getAttribute("aria-disabled") === "true";
}

// More Tab presses than the page has stops (the account field, Look up and the
// two page controls), so one full pass, wrap-around included, fits.
const MAX_TAB_PRESSES = 10;

// Presses Tab until `target` has focus, failing when it is not reached.
async function tabTo(user: UserEvent, target: HTMLElement): Promise<void> {
  for (
    let press = 0;
    press < MAX_TAB_PRESSES && document.activeElement !== target;
    press += 1
  ) {
    await user.tab();
  }
  expect(document.activeElement).toBe(target);
}

/** A promise and its resolver, so a test decides when an answer arrives. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

// The page's lookup status line. The history table has a status region of its
// own, a div, so the page's paragraph is told apart by its element.
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

// The history table's own status region, DataTable's div.
function historyStatus(): HTMLElement {
  const regions = screen
    .getAllByRole("status")
    .filter((region) => region.tagName === "DIV");
  expect(regions).toHaveLength(1);
  return regions[0];
}

// Every value the `aria-busy` attribute of `element` takes from now on, in
// order, null while it is absent. A record keeps the value its change
// replaced, so a value that lasts only until the next change of the same
// batch is seen too.
function recordBusy(element: HTMLElement): Array<string | null> {
  const states: Array<string | null> = [];
  const observer = new MutationObserver((records) => {
    records.slice(1).forEach((record) => {
      states.push(record.oldValue);
    });
    states.push(element.getAttribute("aria-busy"));
  });
  observer.observe(element, {
    attributeFilter: ["aria-busy"],
    attributeOldValue: true,
  });
  observers.push(observer);
  return states;
}

// Asserts DataTable's loading state on the history table holding `heldRows`
// rows: busy, one skeleton row per held row (so its height holds) with no
// row content, both page controls disabled and no result announced.
function expectHistoryLoading(table: HTMLElement, heldRows: number): void {
  expect(table.getAttribute("aria-busy")).toBe("true");
  const rows = Array.from(table.querySelectorAll("tbody tr"));
  expect(rows).toHaveLength(heldRows);
  rows.forEach((row) => {
    expect(row.getAttribute("aria-hidden")).toBe("true");
    expect(row.querySelector('[data-slot="skeleton"]')).not.toBeNull();
    expect(row.textContent).toBe("");
  });
  expect(isDisabled(pageControl("Previous"))).toBe(true);
  expect(isDisabled(pageControl("Next"))).toBe(true);
  expect(historyStatus().textContent).toBe("");
}

function balanceSection(): HTMLElement {
  return screen.getByRole("region", { name: "Reward balance" });
}

// Asserts the balance section's loading state: busy, its three hidden card
// skeletons in place of the cards, and no card, so no figure, shown.
function expectBalanceLoading(): void {
  const section = balanceSection();
  expect(section.getAttribute("aria-busy")).toBe("true");
  const placeholders = Array.from(section.children);
  expect(placeholders).toHaveLength(3);
  for (const card of placeholders) {
    expect(card.getAttribute("aria-hidden")).toBe("true");
    expect(card.querySelector('[data-slot="skeleton"]')).not.toBeNull();
  }
  expect(within(section).queryAllByRole("group")).toEqual([]);
}

/** The history table's column headers, in screen order. */
function historyHeaders(): string[] {
  const table = screen.getByRole("table", { name: "Reward history" });
  return within(table)
    .getAllByRole("columnheader")
    .map((header) => header.textContent ?? "");
}

beforeEach(() => {
  balances.clear();
  histories.clear();
  unexpected.length = 0;
  gates.balance = null;
  gates.history = null;
  fetchMock.mockClear();
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  observers.splice(0).forEach((observer) => {
    observer.disconnect();
  });
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  expect(unexpected).toEqual([]);
});

describe("RewardsPage", () => {
  it("shows the balance and the newest history page of an account exactly as sent", async () => {
    balances.set(ACCOUNT, { status: 200, body: balanceBody });
    histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE });
    const user = renderPage();

    await lookUp(user, ACCOUNT);

    // The history is read once the balance has answered.
    await waitFor(() => {
      expect(requestedPathnames()).toEqual([
        `GET /api/rewards/balance/${ACCOUNT}`,
        `GET /api/rewards/history/${ACCOUNT}`,
      ]);
    });
    await screen.findAllByText(BEYOND_DOUBLE);
    expect(screen.getAllByText(BEYOND_DOUBLE).length).toBeGreaterThanOrEqual(2);
    expect(
      within(summaryCard("Points earned")).getByText(BEYOND_DOUBLE),
    ).toBeDefined();
    expect(
      within(summaryCard("Available points")).getByText(BEYOND_DOUBLE),
    ).toBeDefined();
    expect(within(summaryCard("Points redeemed")).getByText("0")).toBeDefined();
    await waitFor(() => {
      expect(historyRows()).toEqual(rowsOf(PAGE_ONE));
    });
    // `transaction_id` is the accrued transaction's row identity, so its
    // column is not headed like the 16-digit TRAN-ID of the transaction
    // screens.
    expect(historyHeaders()).toEqual([
      "Accrual ID",
      "Transaction row",
      "Rule ID",
      "Points earned",
      "Created at",
    ]);
  });

  it.each([
    { entry: "", why: "nothing entered" },
    { entry: "abcdefghijk", why: "letters" },
    { entry: "00000000000", why: "all zeros" },
    { entry: "1234", why: "fewer than 11 digits" },
  ])(
    "rejects $why with the field message and sends nothing",
    async ({ entry }) => {
      const parsed = rewardsAccountSchema.safeParse({ account_id: entry });
      expect(parsed.success).toBe(false);
      const expected = parsed.error?.issues[0]?.message;
      const user = renderPage();

      await lookUp(user, entry);

      const alert = await screen.findByRole("alert");
      expect(alert.textContent).toBe(expected);
      const input = accountInput();
      expect(input.getAttribute("aria-invalid")).toBe("true");
      expect(document.activeElement).toBe(input);
      expect(fetchMock).not.toHaveBeenCalled();
      expect(screen.queryByRole("group", { name: "Points earned" })).toBeNull();
    },
  );

  it("reports an unknown account with one toast and keeps the form usable", async () => {
    balances.set(UNKNOWN_ACCOUNT, { status: 404, body: accountNotFound });
    const toastError = vi.spyOn(toast, "error");
    const user = renderPage();

    await lookUp(user, UNKNOWN_ACCOUNT);

    await waitFor(() => {
      expect(toastError.mock.calls.map(([message]) => message)).toContain(
        "Account not found",
      );
    });
    expect(toastError).toHaveBeenCalledTimes(1);
    expect(requestedPaths()).toEqual([
      `GET /api/rewards/balance/${UNKNOWN_ACCOUNT}`,
    ]);
    // The reward endpoints name no field, so the schema's BMS field map is
    // empty and a server error marks no field: this case is the whole error
    // path. A field entry added to the map needs a case of its own for the
    // field error and the focus it sets.
    expect(Object.keys(rewardsAccountBmsFieldMap)).toEqual([]);
    const input = accountInput();
    expect(input.getAttribute("aria-invalid")).toBe("false");
    expect(input.disabled).toBe(false);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByRole("table", { name: "Reward history" })).toBeNull();

    balances.set(ACCOUNT, { status: 200, body: balanceBody });
    histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE });
    await lookUp(user, ACCOUNT);

    expect(
      (await screen.findAllByText(BEYOND_DOUBLE)).length,
    ).toBeGreaterThanOrEqual(2);
    await waitFor(() => {
      expect(historyRows()).toEqual(rowsOf(PAGE_ONE));
    });
    expect(toastError).toHaveBeenCalledTimes(1);
  });

  it("pages the history forward with Next and back with Previous from the keyboard", async () => {
    balances.set(ACCOUNT, { status: 200, body: balanceBody });
    histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE });
    histories.set(historyKey(ACCOUNT, "41"), { status: 200, body: PAGE_TWO });
    const user = renderPage();

    await lookUp(user, ACCOUNT);

    await waitFor(() => {
      expect(historyRows()).toEqual(rowsOf(PAGE_ONE));
    });
    expect(isDisabled(pageControl("Previous"))).toBe(true);
    expect(isDisabled(pageControl("Next"))).toBe(false);

    await tabTo(user, pageControl("Next"));
    await user.keyboard("{Enter}");

    await waitFor(() => {
      expect(historyRows()).toEqual(rowsOf(PAGE_TWO));
    });
    expect(historyStartKeys()).toEqual([null, "41"]);
    expect(isDisabled(pageControl("Previous"))).toBe(false);
    expect(isDisabled(pageControl("Next"))).toBe(true);

    // The reread below is held, so the page is seen before any answer.
    const heldHistory = deferred<void>();
    gates.history = heldHistory.promise;
    await tabTo(user, pageControl("Previous"));
    await user.keyboard("{Enter}");

    // The newest page comes back from the cache at once, before any answer.
    expect(historyRows()).toEqual(rowsOf(PAGE_ONE));
    expect(isDisabled(pageControl("Previous"))).toBe(true);
    expect(isDisabled(pageControl("Next"))).toBe(false);
    // The cached page is stale (the query client's default staleTime of 0),
    // so it is reread behind the shown rows, which stay settled meanwhile;
    // page two is never read again.
    await waitFor(() => {
      expect(historyStartKeys()).toEqual([null, "41", null]);
    });
    expect(historyRows()).toEqual(rowsOf(PAGE_ONE));
    expect(
      screen
        .getByRole("table", { name: "Reward history" })
        .hasAttribute("aria-busy"),
    ).toBe(false);
    expect(isDisabled(pageControl("Next"))).toBe(false);
    heldHistory.resolve();
  });

  it.each([
    { from: "the newest page", pagesForward: 0, startKeys: [null, null] },
    { from: "a later page", pagesForward: 1, startKeys: [null, "41", null] },
  ])(
    "keeps the history table loading while a repeated lookup from $from reads the newest page again",
    async ({ pagesForward, startKeys }) => {
      balances.set(ACCOUNT, { status: 200, body: balanceBody });
      histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE });
      histories.set(historyKey(ACCOUNT, "41"), { status: 200, body: PAGE_TWO });
      const user = renderPage();
      await lookUp(user, ACCOUNT);
      await waitFor(() => {
        expect(historyRows()).toEqual(rowsOf(PAGE_ONE));
      });
      if (pagesForward > 0) {
        await user.click(pageControl("Next"));
        await waitFor(() => {
          expect(historyRows()).toEqual(rowsOf(PAGE_TWO));
        });
      }
      const table = screen.getByRole("table", { name: "Reward history" });
      const busy = recordBusy(table);

      // The same account again, with both reads held. The newest page has
      // gained an accrual since it was read.
      histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE_REREAD });
      const heldBalance = deferred<void>();
      const heldHistory = deferred<void>();
      gates.balance = heldBalance.promise;
      gates.history = heldHistory.promise;
      await lookUp(user, ACCOUNT);

      // The table loads from the lookup on, while the balance is read again,
      await waitFor(() => {
        expect(lookupStatus().textContent).toBe(BALANCE_LOADING);
      });
      expectHistoryLoading(table, PAGE_ONE.items.length);

      // and still once the balance has answered and been announced, while the
      // newest page is read again.
      heldBalance.resolve();
      await waitFor(() => {
        expect(historyStartKeys()).toEqual(startKeys);
      });
      await waitFor(() => {
        expect(lookupStatus().textContent).toBe(BALANCE_LOADED);
      });
      expectHistoryLoading(table, PAGE_ONE.items.length);

      heldHistory.resolve();

      await waitFor(() => {
        expect(historyRows()).toEqual(rowsOf(PAGE_ONE_REREAD));
      });
      expect(table.hasAttribute("aria-busy")).toBe(false);
      expect(isDisabled(pageControl("Previous"))).toBe(true);
      expect(isDisabled(pageControl("Next"))).toBe(false);
      await waitFor(() => {
        expect(historyStatus().textContent).toBe("Reward history: 3 rows");
      });
      // One busy span, from the lookup to the held answer: the rows held
      // before it are never shown as its result.
      expect(busy).toEqual(["true", null]);
      expect(historyStartKeys()).toEqual(startKeys);
    },
  );

  it("reads the balance and the newest history page once, and no account-less page, when Look up is pressed again while the balance is read", async () => {
    balances.set(ACCOUNT, { status: 200, body: balanceBody });
    histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE });
    const toastError = vi.spyOn(toast, "error");
    const user = renderPage();
    const heldBalance = deferred<void>();
    gates.balance = heldBalance.promise;
    const lookUpButton = screen.getByRole("button", { name: "Look up" });

    // A double click: the second press arrives while the first press's
    // balance read runs, before the history has been keyed to the account.
    await user.type(accountInput(), ACCOUNT);
    await user.click(lookUpButton);
    await waitFor(() => {
      expect(lookupStatus().textContent).toBe(BALANCE_LOADING);
    });
    await user.click(lookUpButton);
    heldBalance.resolve();

    await waitFor(() => {
      expect(historyRows()).toEqual(rowsOf(PAGE_ONE));
    });
    expect(requestedPaths()).toEqual([
      `GET /api/rewards/balance/${ACCOUNT}`,
      `GET /api/rewards/history/${ACCOUNT}?page_size=10`,
    ]);
    expect(toastError).not.toHaveBeenCalled();
  });

  it("shows skeletons, never the balance the cache holds, while a lookup of an account read before reads its balance again", async () => {
    balances.set(ACCOUNT, { status: 200, body: balanceBody });
    histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE });
    balances.set(OTHER_ACCOUNT, { status: 200, body: otherBalanceBody });
    histories.set(historyKey(OTHER_ACCOUNT), { status: 200, body: OTHER_PAGE });
    const user = renderPage();
    await lookUp(user, ACCOUNT);
    await waitFor(() => {
      expect(historyRows()).toEqual(rowsOf(PAGE_ONE));
    });
    await lookUp(user, OTHER_ACCOUNT);
    await waitFor(() => {
      expect(historyRows()).toEqual(rowsOf(OTHER_PAGE));
    });
    const section = balanceSection();
    const texts = recordTexts(section);
    const busy = recordBusy(section);

    // The first account again, as after its balance was read elsewhere (the
    // dashboard's reward card fills the same cache): the cache holds its
    // earlier balance, and both reads are held while it has grown.
    balances.set(ACCOUNT, { status: 200, body: balanceRereadBody });
    histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE_REREAD });
    const heldBalance = deferred<void>();
    const heldHistory = deferred<void>();
    gates.balance = heldBalance.promise;
    gates.history = heldHistory.promise;
    await lookUp(user, ACCOUNT);

    await waitFor(() => {
      expect(lookupStatus().textContent).toBe(BALANCE_LOADING);
    });
    expectBalanceLoading();
    expectHistoryLoading(
      screen.getByRole("table", { name: "Reward history" }),
      PAGE_ONE.items.length,
    );
    expect(screen.queryByText(BEYOND_DOUBLE)).toBeNull();

    heldBalance.resolve();
    heldHistory.resolve();

    await waitFor(() => {
      expect(historyRows()).toEqual(rowsOf(PAGE_ONE_REREAD));
    });
    expect(
      within(summaryCard("Points earned")).getByText(BEYOND_DOUBLE_PLUS_25),
    ).toBeDefined();
    expect(
      within(summaryCard("Available points")).getByText(BEYOND_DOUBLE_PLUS_25),
    ).toBeDefined();
    expect(section.hasAttribute("aria-busy")).toBe(false);
    // The cached figure was never shown, and the section was busy once, from
    // the lookup to its answer.
    expect(texts.filter((text) => text.includes(BEYOND_DOUBLE))).toEqual([]);
    expect(busy).toEqual(["true", null]);
  });

  it("shows the zero balance and the empty-history text of an account that has earned no points", async () => {
    balances.set(ZERO_ACCOUNT, { status: 200, body: zeroBalanceBody });
    histories.set(historyKey(ZERO_ACCOUNT), { status: 200, body: EMPTY_PAGE });
    const toastError = vi.spyOn(toast, "error");
    const user = renderPage();

    await lookUp(user, ZERO_ACCOUNT);

    await waitFor(() => {
      expect(historyRows()).toEqual([[EMPTY_HISTORY]]);
    });
    for (const title of ["Points earned", "Points redeemed", "Available points"]) {
      expect(within(summaryCard(title)).getByText("0")).toBeDefined();
    }
    // An empty history has neither key, so no page control is offered.
    expect(screen.queryByRole("button", { name: "Previous" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Next" })).toBeNull();
    await waitFor(() => {
      expect(historyStatus().textContent).toBe(
        `Reward history: ${EMPTY_HISTORY}`,
      );
    });
    expect(toastError).not.toHaveBeenCalled();
    expect(requestedPaths()).toEqual([
      `GET /api/rewards/balance/${ZERO_ACCOUNT}`,
      `GET /api/rewards/history/${ZERO_ACCOUNT}?page_size=10`,
    ]);
  });

  it.each([
    { when: "on a first lookup", before: [] },
    { when: "after a lookup that showed rows", before: [OTHER_ACCOUNT] },
  ])(
    "reports a failed history read with one toast and shows its balance with no history rows or page controls $when",
    async ({ before }) => {
      balances.set(OTHER_ACCOUNT, { status: 200, body: otherBalanceBody });
      histories.set(historyKey(OTHER_ACCOUNT), {
        status: 200,
        body: OTHER_PAGE,
      });
      balances.set(ACCOUNT, { status: 200, body: balanceBody });
      histories.set(historyKey(ACCOUNT), { status: 500, body: unexpectedError });
      const toastError = vi.spyOn(toast, "error");
      const user = renderPage();
      for (const account of before) {
        await lookUp(user, account);
        await waitFor(() => {
          expect(historyRows()).toEqual(rowsOf(OTHER_PAGE));
        });
      }

      await lookUp(user, ACCOUNT);

      await waitFor(() => {
        expect(toastError).toHaveBeenCalledWith(unexpectedError.message);
      });
      expect(toastError).toHaveBeenCalledTimes(1);
      expect(
        within(summaryCard("Points earned")).getByText(BEYOND_DOUBLE),
      ).toBeDefined();
      expect(
        within(summaryCard("Available points")).getByText(BEYOND_DOUBLE),
      ).toBeDefined();
      expect(balanceSection().hasAttribute("aria-busy")).toBe(false);
      expect(
        screen.queryByRole("table", { name: "Reward history" }),
      ).toBeNull();
      expect(screen.queryByRole("button", { name: "Previous" })).toBeNull();
      expect(screen.queryByRole("button", { name: "Next" })).toBeNull();
      expect(requestedPaths()).toEqual([
        ...before.flatMap((account) => [
          `GET /api/rewards/balance/${account}`,
          `GET /api/rewards/history/${account}?page_size=10`,
        ]),
        `GET /api/rewards/balance/${ACCOUNT}`,
        `GET /api/rewards/history/${ACCOUNT}?page_size=10`,
      ]);
    },
  );

  it("shows none of the previous account's figures or rows while another account's reads are held, then exactly that account's", async () => {
    balances.set(ACCOUNT, { status: 200, body: balanceBody });
    histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE });
    balances.set(OTHER_ACCOUNT, { status: 200, body: otherBalanceBody });
    histories.set(historyKey(OTHER_ACCOUNT), { status: 200, body: OTHER_PAGE });
    const user = renderPage();
    await lookUp(user, ACCOUNT);
    await waitFor(() => {
      expect(historyRows()).toEqual(rowsOf(PAGE_ONE));
    });
    const previousTexts = [
      BEYOND_DOUBLE,
      `Account ${ACCOUNT}`,
      ...PAGE_ONE.items.map((item) => item.points_earned),
    ];
    const expectNoPreviousAccount = (): void => {
      previousTexts.forEach((text) => {
        expect(screen.queryByText(text)).toBeNull();
      });
    };

    const heldBalance = deferred<void>();
    const heldHistory = deferred<void>();
    gates.balance = heldBalance.promise;
    gates.history = heldHistory.promise;
    await lookUp(user, OTHER_ACCOUNT);

    // While the balance is held, the history waits for it.
    await waitFor(() => {
      expect(lookupStatus().textContent).toBe(BALANCE_LOADING);
    });
    expectBalanceLoading();
    const table = screen.getByRole("table", { name: "Reward history" });
    expect(table.getAttribute("aria-busy")).toBe("true");
    expect(historyRows().flat().join("")).toBe("");
    expectNoPreviousAccount();

    // Once the balance has answered, the history is read for that account.
    heldBalance.resolve();
    await waitFor(() => {
      expect(
        within(summaryCard("Available points")).getByText(
          otherBalanceBody.available_points,
        ),
      ).toBeDefined();
    });
    expect(table.getAttribute("aria-busy")).toBe("true");
    expect(historyRows().flat().join("")).toBe("");
    expectNoPreviousAccount();

    heldHistory.resolve();

    await waitFor(() => {
      expect(historyRows()).toEqual(rowsOf(OTHER_PAGE));
    });
    expectNoPreviousAccount();
    expect(requestedPaths()).toEqual([
      `GET /api/rewards/balance/${ACCOUNT}`,
      `GET /api/rewards/history/${ACCOUNT}?page_size=10`,
      `GET /api/rewards/balance/${OTHER_ACCOUNT}`,
      `GET /api/rewards/history/${OTHER_ACCOUNT}?page_size=10`,
    ]);
  });

  it("takes the balance and history off the screen when an entry is rejected after a lookup, and looks the account up again afterwards", async () => {
    balances.set(ACCOUNT, { status: 200, body: balanceBody });
    histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE });
    const user = renderPage();
    await lookUp(user, ACCOUNT);
    await waitFor(() => {
      expect(historyRows()).toEqual(rowsOf(PAGE_ONE));
    });
    const region = lookupStatus();
    await waitFor(() => {
      expect(region.textContent).toBe(BALANCE_LOADED);
    });

    const rejectedEntry = "1234";
    await lookUp(user, rejectedEntry);

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(
      rewardsAccountSchema.safeParse({ account_id: rejectedEntry }).error
        ?.issues[0]?.message,
    );
    expect(screen.queryByRole("region", { name: "Reward balance" })).toBeNull();
    expect(screen.queryByRole("table", { name: "Reward history" })).toBeNull();
    expect(screen.queryByText(BEYOND_DOUBLE)).toBeNull();
    await waitFor(() => {
      expect(region.textContent).toBe("");
    });
    // The rejected entry sends nothing.
    expect(requestedPaths()).toHaveLength(2);

    // The account again, now a new lookup whose earlier balance and rows
    // the cache holds: neither is shown before the held answers arrive.
    balances.set(ACCOUNT, { status: 200, body: balanceRereadBody });
    histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE_REREAD });
    const heldBalance = deferred<void>();
    const heldHistory = deferred<void>();
    gates.balance = heldBalance.promise;
    gates.history = heldHistory.promise;
    await lookUp(user, ACCOUNT);

    await waitFor(() => {
      expect(region.textContent).toBe(BALANCE_LOADING);
    });
    expect(screen.queryByRole("alert")).toBeNull();
    expectBalanceLoading();
    expectHistoryLoading(
      screen.getByRole("table", { name: "Reward history" }),
      PAGE_ONE.items.length,
    );
    expect(screen.queryByText(BEYOND_DOUBLE)).toBeNull();

    heldBalance.resolve();
    heldHistory.resolve();

    await waitFor(() => {
      expect(historyRows()).toEqual(rowsOf(PAGE_ONE_REREAD));
    });
    expect(
      within(summaryCard("Points earned")).getByText(BEYOND_DOUBLE_PLUS_25),
    ).toBeDefined();
    await waitFor(() => {
      expect(region.textContent).toBe(BALANCE_LOADED);
    });
    expect(requestedPaths()).toEqual([
      `GET /api/rewards/balance/${ACCOUNT}`,
      `GET /api/rewards/history/${ACCOUNT}?page_size=10`,
      `GET /api/rewards/balance/${ACCOUNT}`,
      `GET /api/rewards/history/${ACCOUNT}?page_size=10`,
    ]);
  });

  it("announces each balance lookup by its account, a repeated one included", async () => {
    balances.set(ACCOUNT, { status: 200, body: balanceBody });
    histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE });
    const user = renderPage();

    // Present and empty before any lookup, so assistive technology tracks the
    // region before it first changes.
    const region = lookupStatus();
    expect(region.textContent).toBe("");

    await lookUp(user, ACCOUNT);

    await waitFor(() => {
      expect(region.textContent).toBe(BALANCE_LOADED);
    });
    // The same element throughout: the region is never remounted.
    expect(lookupStatus()).toBe(region);
    // It names the account looked up, never a points value.
    expect(region.textContent).not.toContain(BEYOND_DOUBLE);

    // The same account again, its balance answer held so that the progress
    // is seen.
    const heldBalance = deferred<Response>();
    fetchMock.mockImplementationOnce(async () => heldBalance.promise);
    const texts = recordTexts(region);
    await lookUp(user, ACCOUNT);

    await waitFor(() => {
      expect(region.textContent).toBe(BALANCE_LOADING);
    });
    heldBalance.resolve(jsonResponse({ status: 200, body: balanceBody }));
    await waitFor(() => {
      expect(region.textContent).toBe(BALANCE_LOADED);
    });
    // Cleared before each text, so the unchanged result is announced again.
    expect(texts).toEqual(["", BALANCE_LOADING, "", BALANCE_LOADED]);
    await waitFor(() => {
      expect(requestedPathnames()).toEqual([
        `GET /api/rewards/balance/${ACCOUNT}`,
        `GET /api/rewards/history/${ACCOUNT}`,
        `GET /api/rewards/balance/${ACCOUNT}`,
        `GET /api/rewards/history/${ACCOUNT}`,
      ]);
    });
  });

  it("empties the status region when a lookup after a found account fails, and its toast reports it", async () => {
    balances.set(ACCOUNT, { status: 200, body: balanceBody });
    histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE });
    const toastError = vi.spyOn(toast, "error");
    const user = renderPage();
    await lookUp(user, ACCOUNT);
    const region = lookupStatus();
    await waitFor(() => {
      expect(region.textContent).toBe(BALANCE_LOADED);
    });

    // The unknown account's 404 is held so that the progress is seen.
    const heldBalance = deferred<Response>();
    fetchMock.mockImplementationOnce(async () => heldBalance.promise);
    const texts = recordTexts(region);
    await lookUp(user, UNKNOWN_ACCOUNT);

    await waitFor(() => {
      expect(region.textContent).toBe(BALANCE_LOADING);
    });
    heldBalance.resolve(jsonResponse({ status: 404, body: accountNotFound }));
    await waitFor(() => {
      expect(toastError).toHaveBeenCalledWith("Account not found");
    });
    await waitFor(() => {
      expect(region.textContent).toBe("");
    });
    expect(texts).toEqual(["", BALANCE_LOADING, ""]);
    expect(toastError).toHaveBeenCalledTimes(1);
    expect(requestedPathnames()).toEqual([
      `GET /api/rewards/balance/${ACCOUNT}`,
      `GET /api/rewards/history/${ACCOUNT}`,
      `GET /api/rewards/balance/${UNKNOWN_ACCOUNT}`,
    ]);
  });

  it("holds each balance card's lines with a card skeleton while the balance is read", async () => {
    balances.set(ACCOUNT, { status: 200, body: balanceBody });
    histories.set(historyKey(ACCOUNT), { status: 200, body: PAGE_ONE });
    const balanceGate = deferred<void>();
    gates.balance = balanceGate.promise;
    const user = renderPage();

    await lookUp(user, ACCOUNT);

    // The busy section holds three hidden cards, each with the title, value
    // and description lines of the card it stands in for.
    const section = await screen.findByRole("region", {
      name: "Reward balance",
    });
    expect(section.getAttribute("aria-busy")).toBe("true");
    const placeholders = Array.from(section.children);
    expect(placeholders).toHaveLength(3);
    for (const card of placeholders) {
      expect(card.getAttribute("data-slot")).toBe("card");
      expect(card.getAttribute("aria-hidden")).toBe("true");
      expect(card.querySelectorAll('[data-slot="skeleton"]')).toHaveLength(3);
    }
    expect(screen.queryByRole("group", { name: "Points earned" })).toBeNull();

    balanceGate.resolve();
    await screen.findByRole("group", { name: "Points earned" });
    expect(section.hasAttribute("aria-busy")).toBe(false);
    expect(within(section).getAllByRole("group")).toHaveLength(3);
    expect(section.querySelector('[data-slot="skeleton"]')).toBeNull();
    await waitFor(() => {
      expect(historyRows()).toEqual(rowsOf(PAGE_ONE));
    });
  });
});
