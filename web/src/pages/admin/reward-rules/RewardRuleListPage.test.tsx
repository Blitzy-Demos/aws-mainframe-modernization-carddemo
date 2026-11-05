// FN tests of the admin reward-rule list (pages/admin/reward-rules/
// RewardRuleListPage.tsx): each rule's Status badge and delete text follow the
// `status` the server judged at its processing clock, never the browser's
// clock; the delete's success text follows the outcome the DELETE answer
// names; a failed re-read withholds the rows it no longer answers for, and the
// table stays busy while the list is read again after it; each failed read is
// toasted once, when it settles, and never again by a return to the page that
// reads the list anew; and the list is shown ten rules at a time, Previous and
// Next moving the window without a request.
//
// The network is the global the API client looks up on every request,
// replaced by `fetchMock`, which answers from a table keyed by method and
// path. A request the table does not hold is recorded and fails the test.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
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
import type { RouteHandle } from "@/components/page-header";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";

import RewardRuleListPage from "./RewardRuleListPage";

type Schemas = components["schemas"];
type RewardRule = Schemas["RewardRuleResponse"];
type RewardRuleDeletion = Schemas["RewardRuleDeleteResponse"];
type RuleStatus = Schemas["RewardRuleStatus"];
type ErrorBody = Schemas["ErrorBody"];

// ---------------------------------------------------------------------------
// Fixtures: rules as a server whose processing clock is pinned to
// 2022-07-01T12:00:00Z (CARDDEMO_CLOCK_OVERRIDE) judges them
// ---------------------------------------------------------------------------

// The soft-delete instant of that server.
const PINNED_CLOCK = "2022-07-01T12:00:00Z";

// A rule of type `01` Purchase with category `0001` Regular Sales Draft
// [app/data/ASCII/trantype.txt:L1] [app/data/ASCII/trancatg.txt:L1].
function rule(
  ruleId: string,
  activeFrom: string,
  activeTo: string | null,
  status: RuleStatus,
): RewardRule {
  return {
    rule_id: ruleId,
    transaction_type_cd: "01",
    transaction_category_cd: "0001",
    reward_rate: "1.0000",
    min_transaction_amount: null,
    max_transaction_amount: null,
    active_from: activeFrom,
    active_to: activeTo,
    status,
  };
}

const OPEN_ENDED = rule("1", "2022-01-01T00:00:00Z", null, "active");
const STARTS_2030 = rule("2", "2030-01-01T00:00:00Z", null, "scheduled");
// Still active at the pinned clock, though ended for any later browser clock.
const ENDS_NEXT_DAY = rule(
  "3",
  "2022-01-01T00:00:00Z",
  "2022-07-02T00:00:00Z",
  "active",
);
const ENDED_IN_MARCH = rule(
  "4",
  "2022-01-01T00:00:00Z",
  "2022-03-01T00:00:00Z",
  "ended",
);
const RULES = [OPEN_ENDED, STARTS_2030, ENDS_NEXT_DAY, ENDED_IN_MARCH];

// `count` open-ended active rules with IDs 1 to `count`, in rule order.
function manyRules(count: number): RewardRule[] {
  return Array.from({ length: count }, (_, index) =>
    rule(String(index + 1), "2022-01-01T00:00:00Z", null, "active"),
  );
}

// `target` as a soft delete at the pinned clock leaves it.
function endedNow(target: RewardRule): RewardRule {
  return { ...target, active_to: PINNED_CLOCK, status: "ended" };
}

const UNEXPECTED_ERROR = "Unexpected error";

const READ_FAILED = {
  code: "UNEXPECTED",
  message: UNEXPECTED_ERROR,
} satisfies ErrorBody;

// The page's text in place of the rows when the list could not be read.
const LOAD_FAILED = "Reward rules could not be loaded";

const STATUS_LABELS: Record<RuleStatus, string> = {
  active: "Active",
  scheduled: "Scheduled",
  ended: "Ended",
};

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

const LIST_PATH = "/api/admin/reward-rules";

type Answer = { status: number; body: unknown };

function requestKey(request: Request): string {
  const url = new URL(request.url);
  return `${request.method} ${url.pathname}${url.search}`;
}

let answers = new Map<string, Answer>();
let unmatched: string[] = [];

function answer(key: string, status: number, body: unknown): void {
  answers.set(key, { status, body });
}

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

// A promise and its resolver, so a test decides when an answer arrives.
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

// While set, every request waits for it before the table answers, so a test
// sees the page while a read is out and sets the answer that read gets before
// releasing it. Each test starts with none.
let hold: Promise<void> | null = null;

const fetchMock = vi.fn(async (request: Request): Promise<Response> => {
  if (hold !== null) {
    await hold;
  }
  const key = requestKey(request);
  const found = answers.get(key);
  if (found === undefined) {
    unmatched.push(key);
    return json(500, READ_FAILED);
  }
  return json(found.status, found.body);
});

function sentTo(method: string, pathname: string): Request[] {
  return fetchMock.mock.calls
    .map(([request]) => request)
    .filter(
      (request) =>
        request.method === method &&
        new URL(request.url).pathname === pathname,
    );
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

let queryClient: QueryClient;
let toastError: MockInstance<typeof toast.error>;
let toastSuccess: MockInstance<typeof toast.success>;

beforeEach(() => {
  answers = new Map<string, Answer>();
  unmatched = [];
  hold = null;
  fetchMock.mockClear();
  vi.stubGlobal("fetch", fetchMock);
  toastError = vi.spyOn(toast, "error");
  toastSuccess = vi.spyOn(toast, "success");
});

afterEach(() => {
  // Unmounted before the cache is cleared, so no observer refetches what the
  // clear removes; clearing ends the cache's garbage-collection timers.
  cleanup();
  queryClient.clear();
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  expect(unmatched).toEqual([]);
});

function newQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
}

// The page with the list answering `list`, over a cache of its own.
function renderPage(list: RewardRule[]) {
  answer(`GET ${LIST_PATH}`, 200, list);
  queryClient = newQueryClient();
  return mountPage(queryClient);
}

// The page as App.tsx routes it, without RequireAdmin: the server's 403 is the
// admin gate, and admin_gate.rs and admin-gate.spec.ts test it. It reads
// `client`'s cache, so a later mount over the same client is a return to the
// page that finds what the earlier one left there. `strict` mounts it under
// StrictMode, as main.tsx mounts the app, which runs each mount effect twice.
function mountPage(client: QueryClient, strict = false) {
  const router = createMemoryRouter(
    [
      {
        path: "/admin/reward-rules",
        element: <RewardRuleListPage />,
        handle: { crumb: "Reward Rules" } satisfies RouteHandle,
      },
      {
        path: "/admin/reward-rules/:ruleId/edit",
        element: <p>Edit reward rule</p>,
        handle: { crumb: "Edit" } satisfies RouteHandle,
      },
    ],
    { initialEntries: ["/admin/reward-rules"] },
  );
  render(
    <ThemeProvider>
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>
      <Toaster />
    </ThemeProvider>,
    { reactStrictMode: strict },
  );
  return { user: userEvent.setup() };
}

function rulesTable(): HTMLElement {
  return screen.getByRole("table", { name: "Reward rules" });
}

// The rows that hold rule cells, without the header row.
function dataRows(): HTMLElement[] {
  return within(rulesTable())
    .getAllByRole("row")
    .filter((row) => within(row).queryAllByRole("cell").length > 0);
}

// The cell of `row` under the column headed `column`.
function cellUnder(row: HTMLElement, column: string): HTMLElement {
  const headers = within(rulesTable())
    .getAllByRole("columnheader")
    .map((header) => header.textContent);
  const index = headers.indexOf(column);
  const cell = within(row).getAllByRole("cell")[index];
  if (index < 0 || cell === undefined) {
    throw new Error(`no ${column} cell in the row`);
  }
  return cell;
}

// The Rule ID of every row the table shows, in screen order.
function shownRuleIds(): string[] {
  return dataRows().map((row) => cellUnder(row, "Rule ID").textContent ?? "");
}

function rowOf(ruleId: string): HTMLElement {
  const row = dataRows().find(
    (candidate) => cellUnder(candidate, "Rule ID").textContent === ruleId,
  );
  if (row === undefined) {
    throw new Error(`rule ${ruleId} is not shown`);
  }
  return row;
}

// The list has answered once rule `ruleId` replaces the loading skeleton.
async function ruleShown(ruleId: string): Promise<void> {
  await within(rulesTable()).findByRole("cell", { name: ruleId });
}

// A Previous or Next control the table offers enabled; null when it renders
// none or only a disabled one.
function enabledControl(name: "Previous" | "Next"): HTMLElement | null {
  const control = screen.queryByRole("button", { name });
  return control === null || control.getAttribute("aria-disabled") === "true"
    ? null
    : control;
}

async function openDelete(
  user: ReturnType<typeof userEvent.setup>,
  ruleId: string,
): Promise<HTMLElement> {
  await user.click(within(rowOf(ruleId)).getByRole("button", { name: "Delete" }));
  return screen.findByRole("alertdialog", { name: "Delete reward rule" });
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

describe("RewardRuleListPage status", () => {
  // Under a browser clock in 2040 the rule ending 2022-07-02 would look ended
  // and the 2030 rule started; in 2020 the rule ended in March would look
  // active. The badges follow the server either way.
  it.each([
    { browserClock: "2040-01-01T00:00:00Z" },
    { browserClock: "2020-01-01T00:00:00Z" },
  ])(
    "shows the server's status with the browser clock at $browserClock",
    async ({ browserClock }) => {
      vi.setSystemTime(new Date(browserClock));
      renderPage(RULES);
      await ruleShown(ENDED_IN_MARCH.rule_id);

      expect(shownRuleIds()).toEqual(RULES.map((shown) => shown.rule_id));
      for (const shown of RULES) {
        expect(cellUnder(rowOf(shown.rule_id), "Status").textContent).toBe(
          STATUS_LABELS[shown.status],
        );
      }
      expect(toastError).not.toHaveBeenCalled();
    },
  );

  it.each([
    {
      status: "active",
      target: OPEN_ENDED,
      text: "Rule 1 (type 01, category 0001) ends from now on: its Active to becomes the current time, and it no longer matches new transactions.",
    },
    {
      status: "scheduled",
      target: STARTS_2030,
      text: "Rule 2 (type 01, category 0001) has not started yet. Deleting it ends it now, before its Active from, so it never matches a transaction.",
    },
    {
      status: "ended",
      target: ENDED_IN_MARCH,
      text: "Rule 4 (type 01, category 0001) has already ended, so deleting it changes nothing.",
    },
  ])(
    "the delete dialog of a $status rule says what deleting it does",
    async ({ target, text }) => {
      vi.setSystemTime(new Date("2040-01-01T00:00:00Z"));
      const { user } = renderPage(RULES);
      await ruleShown(target.rule_id);

      const dialog = await openDelete(user, target.rule_id);
      expect(within(dialog).getByText(text).textContent).toBe(text);

      await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
      });
      expect(sentTo("DELETE", `${LIST_PATH}/${target.rule_id}`)).toEqual([]);
    },
  );
});

// ---------------------------------------------------------------------------
// Window boundaries
// ---------------------------------------------------------------------------

describe("RewardRuleListPage window boundaries", () => {
  // A soft delete under the live clock stamps an instant with microseconds.
  const SOFT_DELETED = rule(
    "5",
    "2022-01-01T00:00:00Z",
    "2026-11-04T18:53:02.243996Z",
    "ended",
  );

  // Each boundary shows its UTC date, the date the edit form shows, and keeps
  // the stored instant in its time element; an open end shows NONE alone.
  it.each([
    {
      name: "a midnight Active from",
      target: OPEN_ENDED,
      column: "Active from",
      text: "2022-01-01",
      instants: ["2022-01-01T00:00:00Z"],
    },
    {
      name: "a midnight Active to",
      target: ENDS_NEXT_DAY,
      column: "Active to",
      text: "2022-07-02",
      instants: ["2022-07-02T00:00:00Z"],
    },
    {
      name: "a soft-delete Active to with microseconds",
      target: SOFT_DELETED,
      column: "Active to",
      text: "2026-11-04",
      instants: ["2026-11-04T18:53:02.243996Z"],
    },
    {
      name: "an open-ended Active to",
      target: OPEN_ENDED,
      column: "Active to",
      text: "—",
      instants: [],
    },
  ])("shows $name as $text", async ({ target, column, text, instants }) => {
    renderPage([OPEN_ENDED, ENDS_NEXT_DAY, SOFT_DELETED]);
    await ruleShown(SOFT_DELETED.rule_id);

    const cell = cellUnder(rowOf(target.rule_id), column);
    expect(cell.textContent).toBe(text);
    expect(
      Array.from(cell.querySelectorAll("time"), (time) =>
        time.getAttribute("datetime"),
      ),
    ).toEqual(instants);
  });
});

// ---------------------------------------------------------------------------
// Row actions
// ---------------------------------------------------------------------------

describe("RewardRuleListPage row actions", () => {
  // A cell the table holds at its inline end while the others scroll.
  function isPinned(cell: HTMLElement): boolean {
    return cell.classList.contains("sticky") && cell.classList.contains("end-0");
  }

  it("pins only the Actions column, whose Edit and Delete describe their rule", async () => {
    renderPage(RULES);
    await ruleShown(ENDED_IN_MARCH.rule_id);

    const headers = within(rulesTable()).getAllByRole("columnheader");
    expect(headers.map((header) => header.textContent)).toEqual([
      "Rule ID",
      "Type",
      "Category",
      "Rate",
      "Min amount",
      "Max amount",
      "Active from",
      "Active to",
      "Status",
      "Actions",
    ]);
    expect(headers.filter(isPinned)).toEqual(headers.slice(-1));
    const rows = dataRows();
    expect(
      rows.map((row) => within(row).getAllByRole("cell").filter(isPinned)),
    ).toEqual(rows.map((row) => [cellUnder(row, "Actions")]));
    expect(
      rows.map((row) => {
        const actions = within(cellUnder(row, "Actions"));
        return [
          actions.getByRole("link", { name: "Edit" }),
          actions.getByRole("button", { name: "Delete" }),
        ].map((action) => action.getAttribute("aria-description"));
      }),
    ).toEqual(RULES.map(({ rule_id }) => [`Rule ${rule_id}`, `Rule ${rule_id}`]));
  });
});

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

describe("RewardRuleListPage delete", () => {
  // The row shows an active rule in both cases; only the answer's outcome
  // differs, and it alone picks the text.
  it.each([
    { outcome: "deleted", text: "Reward rule 1 ended" },
    { outcome: "unchanged", text: "Reward rule 1 had already ended" },
  ] as const)(
    "a DELETE answering $outcome toasts $text",
    async ({ outcome, text }) => {
      const { user } = renderPage(RULES);
      await ruleShown(OPEN_ENDED.rule_id);
      const ended = endedNow(OPEN_ENDED);
      answer(`DELETE ${LIST_PATH}/1`, 200, {
        ...ended,
        outcome,
      } satisfies RewardRuleDeletion);

      const dialog = await openDelete(user, OPEN_ENDED.rule_id);
      answer(`GET ${LIST_PATH}`, 200, [ended, ...RULES.slice(1)]);
      await user.click(within(dialog).getByRole("button", { name: "Delete" }));

      await waitFor(() => {
        expect(toastSuccess).toHaveBeenCalledWith(text);
      });
      expect(toastSuccess).toHaveBeenCalledTimes(1);
      expect(sentTo("DELETE", `${LIST_PATH}/1`)).toHaveLength(1);
      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
      });
      // The re-read after the delete shows the rule ended, still listed.
      await waitFor(() => {
        expect(cellUnder(rowOf("1"), "Status").textContent).toBe("Ended");
      });
      // The soft-delete instant, shown as its UTC date.
      const activeTo = cellUnder(rowOf("1"), "Active to");
      expect(activeTo.textContent).toBe("2022-07-01");
      expect(activeTo.querySelector("time")?.getAttribute("datetime")).toBe(
        PINNED_CLOCK,
      );
      expect(toastError).not.toHaveBeenCalled();
    },
  );

  it("a failed re-read after a delete withholds the rows it no longer answers for", async () => {
    const list = manyRules(23);
    const { user } = renderPage(list);
    await ruleShown("1");
    expect(enabledControl("Next")).not.toBeNull();
    answer(`DELETE ${LIST_PATH}/1`, 200, {
      ...endedNow(list[0] ?? OPEN_ENDED),
      outcome: "deleted",
    } satisfies RewardRuleDeletion);

    const dialog = await openDelete(user, "1");
    answer(`GET ${LIST_PATH}`, 500, READ_FAILED);
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));

    const table = rulesTable();
    expect((await within(table).findByText(LOAD_FAILED)).textContent).toBe(
      LOAD_FAILED,
    );
    expect(dataRows().map((row) => row.textContent)).toEqual([LOAD_FAILED]);
    expect(within(table).queryByRole("link", { name: "Edit" })).toBeNull();
    expect(within(table).queryByRole("button", { name: "Delete" })).toBeNull();
    expect(enabledControl("Previous")).toBeNull();
    expect(enabledControl("Next")).toBeNull();
    expect(toastError).toHaveBeenCalledWith(UNEXPECTED_ERROR);
    expect(toastSuccess).toHaveBeenCalledWith("Reward rule 1 ended");
  });
});

// A DELETE the server refuses ends nothing: its text is toasted, no "ended"
// text is shown, the dialog closes, and the row stays listed as it was, with
// no re-read, since only a delete that succeeded reads the list again. The
// rule's Delete then asks again, and a confirmed retry ends it.
describe("RewardRuleListPage failed delete", () => {
  it.each([
    {
      status: 404,
      body: {
        code: "NOT_FOUND",
        message: "Reward rule not found",
      } satisfies ErrorBody,
    },
    {
      status: 500,
      body: {
        code: "UNEXPECTED",
        message: UNEXPECTED_ERROR,
      } satisfies ErrorBody,
    },
  ])(
    "a $status DELETE toasts its message and keeps the rule as listed",
    async ({ status, body }) => {
      const { user } = renderPage(RULES);
      await ruleShown(OPEN_ENDED.rule_id);
      const statusBefore = cellUnder(rowOf("1"), "Status").textContent;
      const activeToBefore = cellUnder(rowOf("1"), "Active to").textContent;
      expect(statusBefore).toBe("Active");
      answer(`DELETE ${LIST_PATH}/1`, status, body);

      const dialog = await openDelete(user, OPEN_ENDED.rule_id);
      await user.click(within(dialog).getByRole("button", { name: "Delete" }));

      await waitFor(() => {
        expect(toastError).toHaveBeenCalledWith(body.message);
      });
      expect(toastError).toHaveBeenCalledTimes(1);
      expect(toastSuccess).not.toHaveBeenCalled();
      expect(sentTo("DELETE", `${LIST_PATH}/1`)).toHaveLength(1);
      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
      });
      expect(sentTo("GET", LIST_PATH)).toHaveLength(1);
      expect(shownRuleIds()).toEqual(RULES.map((shown) => shown.rule_id));
      expect(cellUnder(rowOf("1"), "Status").textContent).toBe(statusBefore);
      expect(cellUnder(rowOf("1"), "Active to").textContent).toBe(
        activeToBefore,
      );

      // The next delete is accepted: the rule's Delete asks again, and the
      // confirmed retry ends the rule.
      const ended = endedNow(OPEN_ENDED);
      answer(`DELETE ${LIST_PATH}/1`, 200, {
        ...ended,
        outcome: "deleted",
      } satisfies RewardRuleDeletion);
      answer(`GET ${LIST_PATH}`, 200, [ended, ...RULES.slice(1)]);
      const again = await openDelete(user, OPEN_ENDED.rule_id);
      await user.click(within(again).getByRole("button", { name: "Delete" }));

      await waitFor(() => {
        expect(toastSuccess).toHaveBeenCalledWith("Reward rule 1 ended");
      });
      expect(toastSuccess).toHaveBeenCalledTimes(1);
      expect(sentTo("DELETE", `${LIST_PATH}/1`)).toHaveLength(2);
      await waitFor(() => {
        expect(cellUnder(rowOf("1"), "Status").textContent).toBe("Ended");
      });
      expect(toastError).toHaveBeenCalledTimes(1);
    },
  );
});

// ---------------------------------------------------------------------------
// Read failures
// ---------------------------------------------------------------------------

describe("RewardRuleListPage read failures", () => {
  // Each row is a run of visits to the page over one cache, each visit's list
  // read answering the status given. A failed read stays in the cache with
  // any rows read before it, and a return to the page reads the list again:
  // the return reports that read's answer alone, the rows and no toast once
  // the server has recovered, one new toast when it fails again. Each failed
  // read answers a message of its own, so a toast names the read it reports.
  // Every visit mounts under StrictMode, so a toast a mount effect repeats
  // counts twice.
  it.each([
    { reads: [500, 200] },
    { reads: [500, 500] },
    { reads: [200, 500, 200] },
    { reads: [200, 500, 500] },
  ])(
    "visits whose reads answer $reads toast each failed read once",
    async ({ reads }) => {
      queryClient = newQueryClient();
      for (const [visit, status] of reads.entries()) {
        const earlierToasts = toastError.mock.calls.length;
        const failed = {
          code: "UNEXPECTED",
          message: `Read ${String(visit + 1)} failed`,
        } satisfies ErrorBody;
        answer(`GET ${LIST_PATH}`, status, status === 200 ? RULES : failed);
        mountPage(queryClient, true);

        if (status === 200) {
          await ruleShown(ENDED_IN_MARCH.rule_id);
          expect(shownRuleIds()).toEqual(RULES.map((shown) => shown.rule_id));
        } else {
          await waitFor(() => {
            expect(toastError.mock.calls.length).toBeGreaterThan(earlierToasts);
          });
          await waitFor(() => {
            expect(queryClient.isFetching()).toBe(0);
          });
          expect(dataRows().map((row) => row.textContent)).toEqual([
            LOAD_FAILED,
          ]);
        }
        expect(toastError.mock.calls.slice(earlierToasts)).toEqual(
          status === 200 ? [] : [[failed.message]],
        );
        cleanup();
      }
      expect(sentTo("GET", LIST_PATH).length).toBeGreaterThanOrEqual(
        reads.length,
      );
    },
  );

  // Cancelling a read that is out reverts the query to the state before it,
  // the failed read's error included. That error was toasted when its read
  // settled, so its return is not toasted again.
  it("a re-read cancelled while out does not toast the error it restores", async () => {
    answer(`GET ${LIST_PATH}`, 500, READ_FAILED);
    queryClient = newQueryClient();
    mountPage(queryClient, true);
    await within(rulesTable()).findByText(LOAD_FAILED);
    await waitFor(() => {
      expect(toastError).toHaveBeenCalledTimes(1);
    });

    // The re-read gets no answer; like fetch, it rejects once aborted.
    fetchMock.mockImplementationOnce(
      (request) =>
        new Promise<Response>((_, reject) => {
          request.signal.addEventListener(
            "abort",
            () => {
              reject(request.signal.reason);
            },
            { once: true },
          );
        }),
    );
    void queryClient.refetchQueries();
    await waitFor(() => {
      expect(within(rulesTable()).queryByText(LOAD_FAILED)).toBeNull();
    });
    await queryClient.cancelQueries();

    await within(rulesTable()).findByText(LOAD_FAILED);
    expect(queryClient.isFetching()).toBe(0);
    expect(toastError.mock.calls).toEqual([[UNEXPECTED_ERROR]]);
  });

  // A return to the page after a failed re-read reads the list again while
  // the query still holds that failure beside the rules read before it.
  // Neither answers the new read, so until it answers the table is busy,
  // shows neither LOAD_FAILED nor those rules and offers no Previous or Next;
  // then it shows that read's own answer: the rules, or LOAD_FAILED with one
  // new toast naming the new failure.
  it.each([{ status: 200 }, { status: 500 }])(
    "a return after a failed re-read is busy until its own read answers $status",
    async ({ status }) => {
      const list = manyRules(23);
      queryClient = newQueryClient();
      answer(`GET ${LIST_PATH}`, 200, list);
      mountPage(queryClient, true);
      await ruleShown("1");
      cleanup();

      answer(`GET ${LIST_PATH}`, 500, READ_FAILED);
      mountPage(queryClient, true);
      await waitFor(() => {
        expect(toastError).toHaveBeenCalledTimes(1);
      });
      await within(rulesTable()).findByText(LOAD_FAILED);
      cleanup();

      const readsBefore = sentTo("GET", LIST_PATH).length;
      const released = deferred<undefined>();
      hold = released.promise;
      mountPage(queryClient, true);
      await waitFor(() => {
        expect(sentTo("GET", LIST_PATH).length).toBeGreaterThan(readsBefore);
      });
      const busy = rulesTable();
      expect(busy.getAttribute("aria-busy")).toBe("true");
      expect(within(busy).queryByText(LOAD_FAILED)).toBeNull();
      expect(dataRows()).toEqual([]);
      expect(within(busy).queryByRole("link", { name: "Edit" })).toBeNull();
      expect(within(busy).queryByRole("button", { name: "Delete" })).toBeNull();
      expect(enabledControl("Previous")).toBeNull();
      expect(enabledControl("Next")).toBeNull();
      expect(toastError).toHaveBeenCalledTimes(1);

      const failedAgain = {
        code: "UNEXPECTED",
        message: "Read 3 failed",
      } satisfies ErrorBody;
      answer(`GET ${LIST_PATH}`, status, status === 200 ? list : failedAgain);
      hold = null;
      released.resolve(undefined);

      await waitFor(() => {
        expect(queryClient.isFetching()).toBe(0);
      });
      if (status === 200) {
        await ruleShown("1");
        expect(shownRuleIds()).toEqual(
          list.slice(0, 10).map((shown) => shown.rule_id),
        );
        expect(rulesTable().hasAttribute("aria-busy")).toBe(false);
        expect(enabledControl("Next")).not.toBeNull();
      } else {
        await waitFor(() => {
          expect(toastError).toHaveBeenCalledTimes(2);
        });
        expect(dataRows().map((row) => row.textContent)).toEqual([
          LOAD_FAILED,
        ]);
        expect(enabledControl("Next")).toBeNull();
      }
      expect(toastError.mock.calls.slice(1)).toEqual(
        status === 200 ? [] : [[failedAgain.message]],
      );
    },
  );
});

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

describe("RewardRuleListPage windows", () => {
  it("shows 23 rules ten at a time; Previous and Next move the window without a request", async () => {
    const { user } = renderPage(manyRules(23));
    await ruleShown("1");
    const ids = (from: number, to: number): string[] =>
      Array.from({ length: to - from + 1 }, (_, index) => String(from + index));

    expect(shownRuleIds()).toEqual(ids(1, 10));
    expect(enabledControl("Previous")).toBeNull();

    // (control, rule IDs shown after it, Previous enabled, Next enabled)
    const steps = [
      { control: "Next", shows: ids(11, 20), previous: true, next: true },
      { control: "Next", shows: ids(21, 23), previous: true, next: false },
      { control: "Previous", shows: ids(11, 20), previous: true, next: true },
      { control: "Previous", shows: ids(1, 10), previous: false, next: true },
    ] as const;
    for (const step of steps) {
      const control = enabledControl(step.control);
      if (control === null) {
        throw new Error(`${step.control} is not enabled`);
      }
      await user.click(control);
      await waitFor(() => {
        expect(shownRuleIds()).toEqual(step.shows);
      });
      expect(enabledControl("Previous") !== null).toBe(step.previous);
      expect(enabledControl("Next") !== null).toBe(step.next);
    }
    expect(sentTo("GET", LIST_PATH)).toHaveLength(1);
  });

  it("a re-read that shrinks the list below the window moves it to the last rules", async () => {
    const list = manyRules(23);
    const { user } = renderPage(list);
    await ruleShown("1");
    for (let page = 0; page < 2; page += 1) {
      const next = enabledControl("Next");
      if (next === null) {
        throw new Error("Next is not enabled");
      }
      await user.click(next);
    }
    await waitFor(() => {
      expect(shownRuleIds()).toEqual(["21", "22", "23"]);
    });

    const target = list[20] ?? OPEN_ENDED;
    answer(`DELETE ${LIST_PATH}/21`, 200, {
      ...endedNow(target),
      outcome: "deleted",
    } satisfies RewardRuleDeletion);
    const dialog = await openDelete(user, "21");
    // Rules past the twelfth are gone from the store when the list is read
    // again, so the third window holds none.
    answer(`GET ${LIST_PATH}`, 200, list.slice(0, 12));
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));

    await waitFor(() => {
      expect(shownRuleIds()).toEqual(["11", "12"]);
    });
    expect(enabledControl("Next")).toBeNull();
    expect(enabledControl("Previous")).not.toBeNull();
  });
});
