// FN tests of the CU00 user list and the CU03 delete dialog
// (pages/admin/users/UserListPage.tsx): each function the page carries from
// COUSR00 and COUSR03 sends what valid input asks for and refuses invalid
// input, and the server's outcome reaches the toast, the USRIDIN field and the
// cursor. Controls are queried by the role and name the F-COUSR00 and
// F-COUSR03 rows of docs/parity/function-map.md give them.
//
// The network is the global the API client looks up on every request,
// replaced by `fetchMock`, which answers from a table keyed by method, path
// and query. A request the table does not hold is recorded and fails the test.
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
  onTestFinished,
  vi,
  type MockInstance,
} from "vitest";

import type { components } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import {
  clearSession,
  getSession,
  setSession,
  type Session,
} from "@/lib/session";
import { RequireAuth } from "@/routes/guards";

import UserListPage from "./UserListPage";

type Schemas = components["schemas"];
type UserSummary = Schemas["UserSummary"];
type UserPage = Schemas["Page_UserSummary"];
type UserMessageResponse = Schemas["UserMessageResponse"];
type ErrorBody = Schemas["ErrorBody"];
type UserListField = Schemas["UserListField"];

// ---------------------------------------------------------------------------
// Fixtures: the DUSRSECJ SYSUT1 rows [app/jcl/DUSRSECJ.jcl:L35-L42]
// ---------------------------------------------------------------------------

const ADMIN001 = {
  user_id: "ADMIN001",
  first_name: "MARGARET",
  last_name: "GOLD",
  usr_type: "A",
} satisfies UserSummary;

const USER0001 = {
  user_id: "USER0001",
  first_name: "LAWRENCE",
  last_name: "THOMAS",
  usr_type: "U",
} satisfies UserSummary;

const USER0002 = {
  user_id: "USER0002",
  first_name: "AJITH",
  last_name: "KUMAR",
  usr_type: "U",
} satisfies UserSummary;

const USER0003 = {
  user_id: "USER0003",
  first_name: "LAURITZ",
  last_name: "ALME",
  usr_type: "U",
} satisfies UserSummary;

// The page read on entry, from the top of the file. The first page carries its
// first key as prev_key, because COUSR00C's PF7 stays live on page 1 (E-54).
const FIRST_PAGE = {
  items: [ADMIN001, USER0001, USER0002],
  next_key: "USER0003",
  prev_key: "ADMIN001",
} satisfies UserPage;

// The page from USER0003, forward: the last seeded rows of this fixture set.
const USER0003_PAGE = {
  items: [USER0003],
  next_key: null,
  prev_key: "USER0003",
} satisfies UserPage;

// PF7 on the first page (V-COUSR00C-007): an empty page with its text.
const TOP_OF_PAGE = "You are already at the top of the page...";
const TOP_PAGE = {
  items: [],
  next_key: null,
  prev_key: null,
  message: TOP_OF_PAGE,
} satisfies UserPage;

// STARTBR NOTFND (V-COUSR00C-003): no user at or after the start key, the
// answer to a search from ZZZZZZZZ, or to PF8 once the next user is gone.
const AT_TOP_OF_PAGE = "You are at the top of the page...";
const NOT_FOUND_PAGE = {
  items: [],
  next_key: null,
  prev_key: null,
  message: AT_TOP_OF_PAGE,
} satisfies UserPage;

// V-COUSR00C-005: a forward page that reaches the end of the user file shows
// its rows with this text.
const BOTTOM_OF_PAGE = "You have reached the bottom of the page...";

// Users `.` and `..`: COUSR01C stores any non-blank ID, and a URL drops
// these two as dot segments, so their links and requests carry the key
// segments `~.` and `~..` (lib/key-segment.ts).
const DOT_USER = {
  user_id: ".",
  first_name: "DOT",
  last_name: "KEY",
  usr_type: "U",
} satisfies UserSummary;

const DOTS_USER = {
  user_id: "..",
  first_name: "DOTS",
  last_name: "KEY",
  usr_type: "U",
} satisfies UserSummary;

const DOT_USERS_PAGE = {
  items: [DOT_USER, DOTS_USER, ADMIN001],
  next_key: null,
  prev_key: DOT_USER.user_id,
} satisfies UserPage;

const DOT_USER_CASES = [
  { listed: DOT_USER, segment: "~.", href: "/admin/users/~./edit" },
  { listed: DOTS_USER, segment: "~..", href: "/admin/users/~../edit" },
];

// The list after USER0001 is deleted.
const AFTER_DELETE_PAGE = {
  items: [ADMIN001, USER0002, USER0003],
  next_key: null,
  prev_key: "ADMIN001",
} satisfies UserPage;

// COUSR03C DELETE-USER-SEC-FILE strings 'User ', SEC-USR-ID and
// ' has been deleted ...' into WS-MESSAGE PIC X(80)
// [app/cbl/COUSR03C.cbl:L317-321]; the API sends the field with its trailing
// spaces, and the page shows it unchanged (P-COUSR03C-005).
const DELETED = {
  message: "User USER0001 has been deleted ...".padEnd(80, " "),
  user: USER0001,
} satisfies UserMessageResponse;

// ADMIN001 signed on for the next hour, as LoginPage stores the login answer.
function admin001Session(): Session {
  return {
    token: "admin001-token",
    user_id: ADMIN001.user_id,
    usr_type: ADMIN001.usr_type,
    landing: "/admin",
    expires_at: new Date(Date.now() + 3_600_000).toISOString(),
  };
}

const USRIDIN: UserListField = "USRIDIN";

// The audit gives the CU00 browse no 400 with a message: every V-COUSR00C
// error row is the 500 'Unable to lookup User...' on USRIDIN, and the API's
// 400 for a start key it refuses carries no text (E-49). The server-error
// cases are therefore that audited 500 and the API's own message-less 400.
const LOOKUP_FAILED = {
  code: "UNEXPECTED",
  message: "Unable to lookup User...",
  fields: [{ field: USRIDIN, message: "Unable to lookup User..." }],
} satisfies ErrorBody; // V-COUSR00C-004

const START_KEY_REFUSED = {
  code: "INVALID",
  message: "",
  fields: [{ field: USRIDIN, message: "" }],
} satisfies ErrorBody;

// What lib/api-errors.ts toasts for an error body without text: the server's
// own `Unexpected error` (crates/domain/src/messages.rs).
const UNEXPECTED_ERROR = "Unexpected error";

// zod's default issue text for a failed `.regex()`, which the start-key
// schema keeps for its ASCII pattern (schemas/admin-users.ts).
const ZOD_REGEX_MESSAGE = "Invalid";

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

const LIST_PATH = "/api/admin/users";

type Answer = { status: number; body: unknown };

// Method, path and query, with the query sorted so the order the client
// writes its parameters in does not matter.
function requestKey(request: Request): string {
  const url = new URL(request.url);
  url.searchParams.sort();
  return `${request.method} ${url.pathname}${url.search}`;
}

function defaultAnswers(): Map<string, Answer> {
  return new Map<string, Answer>([
    [`GET ${LIST_PATH}`, { status: 200, body: FIRST_PAGE }],
    [
      `GET ${LIST_PATH}?start_key=USER0003`,
      { status: 200, body: USER0003_PAGE },
    ],
    [
      `GET ${LIST_PATH}?direction=forward&start_key=USER0003`,
      { status: 200, body: USER0003_PAGE },
    ],
    [
      `GET ${LIST_PATH}?direction=backward&start_key=ADMIN001`,
      { status: 200, body: TOP_PAGE },
    ],
    [`GET ${LIST_PATH}/USER0001`, { status: 200, body: USER0001 }],
    [`DELETE ${LIST_PATH}/USER0001`, { status: 200, body: DELETED }],
  ]);
}

let answers = defaultAnswers();
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
    return json(500, {
      code: "UNEXPECTED",
      message: UNEXPECTED_ERROR,
    } satisfies ErrorBody);
  }
  return json(found.status, found.body);
});

function sent(): Request[] {
  return fetchMock.mock.calls.map(([request]) => request);
}

function sentTo(method: string, pathname: string): Request[] {
  return sent().filter(
    (request) =>
      request.method === method && new URL(request.url).pathname === pathname,
  );
}

// The query of every list browse sent so far, in order.
function listQueries(): URLSearchParams[] {
  return sentTo("GET", LIST_PATH).map(
    (request) => new URL(request.url).searchParams,
  );
}

// ---------------------------------------------------------------------------
// Toasts
// ---------------------------------------------------------------------------

// A page's text is raised through sonner's `toast()` itself, which a spy
// cannot wrap, and a Toaster mounted again shows the toasts an earlier one
// left open, so the screen cannot tell a new toast from an old one. Toasts are
// therefore counted in toast.getHistory(), sonner's record of each toast
// raised, against a copy of it taken before the test acts: its toasts
// (toastHistory) or their ids (raisedToastIds).
function toastHistory(): unknown[] {
  return [...toast.getHistory()];
}

// The toasts created with the text `title` since `before`, a toastHistory().
function toastsSince(before: readonly unknown[], title: string): number {
  return toast
    .getHistory()
    .filter(
      (created) =>
        !before.includes(created) &&
        "title" in created &&
        created.title === title,
    ).length;
}

type ToastId = ReturnType<typeof toast>;

function raisedToastIds(): Set<ToastId> {
  return new Set(toast.getHistory().map((raised) => raised.id));
}

// The title of each toast raised since `before` was taken, in order.
function toastsRaisedSince(before: Set<ToastId>): unknown[] {
  return toast
    .getHistory()
    .filter((raised) => !before.has(raised.id))
    .map((raised) => ("title" in raised ? raised.title : undefined));
}

// The rendered toasts whose text includes `message`.
function toastsShowing(message: string): HTMLElement[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>("[data-sonner-toast]"),
  ).filter((element) => element.textContent?.includes(message) === true);
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

let queryClient: QueryClient;
let toastError: MockInstance<typeof toast.error>;
let toastSuccess: MockInstance<typeof toast.success>;

beforeEach(() => {
  answers = defaultAnswers();
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
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  expect(unmatched).toEqual([]);
});

function newQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
}

// The page over a cache of its own.
function renderPage() {
  queryClient = newQueryClient();
  return mountPage(queryClient);
}

// The page as App.tsx routes it, without RequireAdmin: the server's 403 is the
// admin gate, and admin_gate.rs and admin-gate.spec.ts test it. The /admin
// menu (PF3) and the CU02 edit route (selector U) are the page's link targets.
// It reads `client`'s cache, so a later mount over the same client is a return
// to the page that finds what the earlier one left there. `strict` mounts it
// under StrictMode, as main.tsx mounts the app, which runs each mount effect
// twice.
function mountPage(client: QueryClient, strict = false) {
  const router = createMemoryRouter(
    [
      {
        path: "/admin",
        element: <p>Admin menu</p>,
        handle: { crumb: "Administration" } satisfies RouteHandle,
      },
      {
        path: "/admin/users",
        element: <UserListPage />,
        handle: { crumb: "Users", title: "List Users" } satisfies RouteHandle,
      },
      {
        path: "/admin/users/:userId/edit",
        element: <p>Update User</p>,
        handle: { crumb: "Update User" } satisfies RouteHandle,
      },
    ],
    { initialEntries: ["/admin/users"] },
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

// The first page has answered once its rows replace the loading skeleton.
async function firstPageShown(): Promise<void> {
  await screen.findByText(ADMIN001.user_id);
  expect(screen.getByText(USER0001.user_id).textContent).toBe(USER0001.user_id);
}

function startKeyInput(): HTMLElement {
  return screen.getByRole("textbox", { name: "User ID" });
}

function usersTable(): HTMLElement {
  return screen.getByRole("table");
}

// The FieldError the input names in aria-describedby, or null when none is
// rendered: a role="alert" element linked to the field it concerns.
function fieldError(input: HTMLElement): HTMLElement | null {
  const ids = (input.getAttribute("aria-describedby") ?? "").split(" ");
  for (const id of ids) {
    const element = id === "" ? null : document.getElementById(id);
    if (element?.getAttribute("role") === "alert") {
      return element;
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Valid input
// ---------------------------------------------------------------------------

describe("UserListPage CU00 browse", () => {
  it("reads the first page from the top of the file on entry", async () => {
    renderPage();
    await firstPageShown();

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "List Users",
    );
    const [entry, ...rest] = listQueries();
    expect(rest).toEqual([]);
    expect(entry?.toString()).toBe("");
  });

  // F-COUSR00-001: ENTER browses forward from USRIDIN, from the top when blank.
  it.each([
    { entered: "USER0003", startKey: "USER0003", shows: USER0003.first_name },
    { entered: "", startKey: null, shows: ADMIN001.first_name },
  ])(
    "Search with User ID $entered browses forward from start_key $startKey",
    async ({ entered, startKey, shows }) => {
      const { user } = renderPage();
      await firstPageShown();

      await user.type(startKeyInput(), `${entered}{Enter}`);

      await waitFor(() => {
        expect(listQueries()).toHaveLength(2);
      });
      const search = listQueries()[1];
      expect(search?.get("start_key") ?? null).toBe(startKey);
      expect(search?.get("direction") ?? "forward").toBe("forward");
      expect(search?.has("page_size")).toBe(false);
      expect(
        (await within(usersTable()).findByText(shows)).textContent,
      ).toBe(shows);
      expect(startKeyInput().getAttribute("aria-invalid")).toBe("false");
      expect(toastError).not.toHaveBeenCalled();
    },
  );

  // F-COUSR00-004 and F-COUSR00-003: PF8 and PF7 pass the page's keys back
  // unchanged with their direction. The answer shows: the next page's rows,
  // or the top-of-page text of PF7 on the first page.
  it.each([
    {
      control: "Next",
      direction: "forward",
      startKey: FIRST_PAGE.next_key,
      shows: USER0003.first_name,
    },
    {
      control: "Previous",
      direction: "backward",
      startKey: FIRST_PAGE.prev_key,
      shows: TOP_OF_PAGE,
    },
  ])(
    "$control browses $direction from the page's key",
    async ({ control, direction, startKey, shows }) => {
      const { user } = renderPage();
      await firstPageShown();

      await user.click(screen.getByRole("button", { name: control }));

      expect((await screen.findByText(shows)).textContent).toBe(shows);
      const browse = listQueries().at(-1);
      expect(browse?.get("direction")).toBe(direction);
      expect(browse?.get("start_key")).toBe(startKey);
      expect(listQueries()).toHaveLength(2);
    },
  );

  // F-COUSR00-003 and F-COUSR00-004: COUSR00C sends PF7 on the first page and
  // PF8 without a next page without ERASE (P-COUSR00C-004, P-COUSR00C-005,
  // P-COUSR00C-013), so a key answered with no rows keeps the rows and both
  // keys, and its text is toasted. Every press reads the file again and shows
  // the text again. ENTER erases the screen: a search that finds no user shows
  // its text in place of the rows.
  it.each([
    {
      control: "Previous",
      direction: "backward",
      startKey: FIRST_PAGE.prev_key,
      boundary: TOP_PAGE,
    },
    {
      control: "Next",
      direction: "forward",
      startKey: FIRST_PAGE.next_key,
      boundary: NOT_FOUND_PAGE,
    },
  ])(
    "$control answered with no rows keeps the page and toasts its text for every press",
    async ({ control, direction, startKey, boundary }) => {
      answer(
        `GET ${LIST_PATH}?direction=${direction}&start_key=${startKey}`,
        200,
        boundary,
      );
      answer(`GET ${LIST_PATH}?start_key=ZZZZZZZZ`, 200, NOT_FOUND_PAGE);
      const { user } = renderPage();
      await firstPageShown();
      const before = toastHistory();

      for (const press of [1, 2]) {
        await user.click(screen.getByRole("button", { name: control }));

        await waitFor(() => {
          expect(toastsSince(before, boundary.message)).toBe(press);
        });
        expect(listQueries()).toHaveLength(1 + press);
        const browse = listQueries().at(-1);
        expect(browse?.get("direction")).toBe(direction);
        expect(browse?.get("start_key")).toBe(startKey);
        const table = usersTable();
        expect(within(table).getAllByRole("row")).toHaveLength(
          FIRST_PAGE.items.length + 1,
        );
        for (const shown of FIRST_PAGE.items) {
          expect(within(table).getByText(shown.user_id).textContent).toBe(
            shown.user_id,
          );
        }
        expect(within(table).queryByText(boundary.message)).toBeNull();
        for (const paging of ["Previous", "Next"]) {
          expect(
            screen
              .getByRole("button", { name: paging })
              .getAttribute("aria-disabled"),
          ).toBeNull();
        }
        expect(document.activeElement).not.toBe(document.body);
      }
      await waitFor(() => {
        expect(queryClient.isFetching()).toBe(0);
      });
      expect(listQueries()).toHaveLength(3);
      expect(toastsSince(before, boundary.message)).toBe(2);

      const beforeSearch = toastHistory();
      await user.type(startKeyInput(), "ZZZZZZZZ{Enter}");

      expect(
        (await within(usersTable()).findByText(AT_TOP_OF_PAGE)).textContent,
      ).toBe(AT_TOP_OF_PAGE);
      expect(toastsSince(beforeSearch, AT_TOP_OF_PAGE)).toBe(0);
      expect(listQueries()).toHaveLength(4);
      expect(toastError).not.toHaveBeenCalled();
    },
  );

  // A page read before comes back from the cache while it is read again: on
  // a return to the list, which mounts the page again over the same cache, and
  // on a Next onto a page already read. The copy shown meanwhile carries the
  // text its earlier read found, which does not answer this browse, so nothing
  // is toasted while the read is out. The read's own answer is then toasted
  // once when it carries a text, and not at all when a user added since
  // means the page no longer ends the file. Every visit mounts under
  // StrictMode, as main.tsx mounts the app, so a toast a mount effect repeats
  // counts twice.
  it.each([
    { reread: "a return to the list", fresh: "no text" },
    { reread: "a return to the list", fresh: "the boundary text" },
    { reread: "a Next onto a page read before", fresh: "no text" },
    { reread: "a Next onto a page read before", fresh: "the boundary text" },
  ] as const)(
    "$reread toasts only its own answer, which carries $fresh",
    async ({ reread, fresh }) => {
      const returning = reread === "a return to the list";
      // The page the browse comes back to, and the request that reads it.
      const pageRequest = returning
        ? `GET ${LIST_PATH}`
        : `GET ${LIST_PATH}?direction=forward&start_key=${USER0003.user_id}`;
      const rows = returning ? FIRST_PAGE.items : USER0003_PAGE.items;
      const prevKey = returning ? FIRST_PAGE.prev_key : USER0003_PAGE.prev_key;
      // The page as the earlier read found it: the end of the file.
      const endOfFile = {
        items: rows,
        next_key: null,
        prev_key: prevKey,
        message: BOTTOM_OF_PAGE,
      } satisfies UserPage;
      // The page read again after a user was added behind it: more users
      // follow, so it carries no text.
      const runsOn = {
        items: rows,
        next_key: "USER0004",
        prev_key: prevKey,
      } satisfies UserPage;
      const freshPage = fresh === "no text" ? runsOn : endOfFile;

      // PF7 from the page from USER0003 reads the first page, whose next key
      // leads back to it.
      answer(
        `GET ${LIST_PATH}?direction=backward&start_key=${USER0003.user_id}`,
        200,
        FIRST_PAGE,
      );
      answer(pageRequest, 200, endOfFile);
      toast.dismiss();
      const beforeVisit = raisedToastIds();
      queryClient = newQueryClient();
      const { user } = mountPage(queryClient, true);
      await firstPageShown();
      if (!returning) {
        await user.click(screen.getByRole("button", { name: "Next" }));
        await within(usersTable()).findByText(USER0003.user_id);
      }
      // The earlier read answered its browse, and its text was toasted once.
      await waitFor(() => {
        expect(toastsRaisedSince(beforeVisit)).toEqual([BOTTOM_OF_PAGE]);
      });
      if (!returning) {
        await user.click(screen.getByRole("button", { name: "Previous" }));
        await within(usersTable()).findByText(ADMIN001.user_id);
        await waitFor(() => {
          expect(queryClient.isFetching()).toBe(0);
        });
      }

      const readsBefore = listQueries().length;
      toast.dismiss();
      const beforeReread = raisedToastIds();
      answer(pageRequest, 200, freshPage);
      const released = deferred<undefined>();
      hold = released.promise;
      if (returning) {
        cleanup();
        mountPage(queryClient, true);
      } else {
        await user.click(screen.getByRole("button", { name: "Next" }));
      }

      // While the read is out, the cache's copy is on screen, untoasted.
      await waitFor(() => {
        expect(listQueries().length).toBeGreaterThan(readsBefore);
      });
      for (const shown of rows) {
        expect(
          within(usersTable()).getByText(shown.user_id).textContent,
        ).toBe(shown.user_id);
      }
      expect(toastsRaisedSince(beforeReread)).toEqual([]);

      hold = null;
      released.resolve(undefined);

      await waitFor(() => {
        expect(queryClient.isFetching()).toBe(0);
      });
      // The answer is on screen once Next follows its next key.
      await waitFor(() => {
        expect(
          screen
            .getByRole("button", { name: "Next" })
            .getAttribute("aria-disabled"),
        ).toBe(freshPage.next_key === null ? "true" : null);
      });
      if (fresh === "no text") {
        expect(toastsRaisedSince(beforeReread)).toEqual([]);
      } else {
        await waitFor(() => {
          expect(toastsRaisedSince(beforeReread)).toEqual([BOTTOM_OF_PAGE]);
        });
        await waitFor(() => {
          expect(toastsShowing(BOTTOM_OF_PAGE)).not.toEqual([]);
        });
      }
    },
  );
});

describe("UserListPage CU03 delete", () => {
  // F-COUSR00-006 and F-COUSR03-001 read the row's user into the dialog;
  // F-COUSR03-004 (PF5) deletes it. The signed-on admin deletes another
  // user, so their session stays.
  it("Delete reads the row's user into the dialog and Confirm deletes it", async () => {
    const signedOn = admin001Session();
    setSession(signedOn);
    onTestFinished(() => {
      clearSession();
    });
    const { user } = renderPage();
    await firstPageShown();

    await user.click(
      screen.getByRole("button", {
        name: "Delete",
        description: USER0001.user_id,
      }),
    );

    const dialog = await screen.findByRole("alertdialog", {
      name: "Delete User",
    });
    expect(sentTo("GET", `${LIST_PATH}/USER0001`)).toHaveLength(1);
    const shown = within(dialog);
    for (const value of [
      USER0001.user_id,
      USER0001.first_name,
      USER0001.last_name,
      USER0001.usr_type,
    ]) {
      expect(shown.getByText(value).textContent).toBe(value);
    }
    expect(sentTo("DELETE", `${LIST_PATH}/USER0001`)).toEqual([]);

    // The re-read that follows the delete answers without the deleted row.
    answer(`GET ${LIST_PATH}`, 200, AFTER_DELETE_PAGE);
    await user.click(shown.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(toastSuccess).toHaveBeenCalledWith(DELETED.message);
    });
    expect(sentTo("DELETE", `${LIST_PATH}/USER0001`)).toHaveLength(1);
    // The toast shows the text as sent, trailing spaces included.
    expect(
      (await screen.findByText(DELETED.message.trim())).textContent,
    ).toBe(DELETED.message);

    // The list is read again after the delete, and the row is gone.
    const deleteIndex = sent().findIndex(
      (request) => request.method === "DELETE",
    );
    await waitFor(() => {
      expect(
        sent()
          .slice(deleteIndex + 1)
          .some((request) => requestKey(request) === `GET ${LIST_PATH}`),
      ).toBe(true);
    });
    await waitFor(() => {
      expect(within(usersTable()).queryByText(USER0001.user_id)).toBeNull();
    });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(toastError).not.toHaveBeenCalled();
    expect(getSession()).toEqual(signedOn);
  });

  // F-COUSR03-002, F-COUSR03-003 and F-COUSR03-005: PF3, PF4 and PF12 are
  // the dialog's Cancel, which deletes nothing.
  it("Cancel closes the dialog without deleting", async () => {
    const { user } = renderPage();
    await firstPageShown();

    await user.click(
      screen.getByRole("button", {
        name: "Delete",
        description: USER0001.user_id,
      }),
    );
    const dialog = await screen.findByRole("alertdialog", {
      name: "Delete User",
    });
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));

    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });
    expect(sentTo("DELETE", `${LIST_PATH}/USER0001`)).toEqual([]);
    expect(toastSuccess).not.toHaveBeenCalled();
  });

  // The delete's re-read of a page kept after PF7 on the first page reads
  // that page again, so the deleted row leaves the rows shown; the re-read
  // toasts no page text.
  it("a delete from the page kept after Previous re-reads that page", async () => {
    const { user } = renderPage();
    await firstPageShown();
    const before = toastHistory();
    await user.click(screen.getByRole("button", { name: "Previous" }));
    await waitFor(() => {
      expect(toastsSince(before, TOP_OF_PAGE)).toBe(1);
    });

    answer(`GET ${LIST_PATH}`, 200, AFTER_DELETE_PAGE);
    await user.click(
      screen.getByRole("button", {
        name: "Delete",
        description: USER0001.user_id,
      }),
    );
    const dialog = await screen.findByRole("alertdialog", {
      name: "Delete User",
    });
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(toastSuccess).toHaveBeenCalledWith(DELETED.message);
    });
    await waitFor(() => {
      expect(within(usersTable()).queryByText(USER0001.user_id)).toBeNull();
    });
    const table = usersTable();
    for (const shown of AFTER_DELETE_PAGE.items) {
      expect(within(table).getByText(shown.user_id).textContent).toBe(
        shown.user_id,
      );
    }
    await waitFor(() => {
      expect(queryClient.isFetching()).toBe(0);
    });
    const last = sent().at(-1);
    expect(last === undefined ? null : requestKey(last)).toBe(
      `GET ${LIST_PATH}`,
    );
    expect(toastsSince(before, TOP_OF_PAGE)).toBe(1);
    expect(
      screen.getByRole("button", { name: "Next" }).getAttribute("aria-disabled"),
    ).toBe("true");
    expect(toastError).not.toHaveBeenCalled();
  });

  // P-COUSR03C-005 for an admin who deletes their own account. The DELETE
  // answers 200 with the COUSR03C text; the session's token then names no
  // stored user, so the server refuses every later request of it 401
  // 'Authentication required'. The page ends the session as it shows the
  // text, without a cause, so RequireAuth replaces the page with the sign-on
  // route and the delete's text is the only toast: the list re-read already
  // sent with the ended session's token is refused 401 and adds none.
  it("Confirm on the signed-on admin's own row shows the deleted text alone and ends the session", async () => {
    const signedOn = admin001Session();
    const selfDeleted = {
      message: `User ${ADMIN001.user_id} has been deleted ...`.padEnd(80, " "),
      user: ADMIN001,
    } satisfies UserMessageResponse;
    const authenticationRequired = {
      code: "UNAUTHENTICATED",
      message: "Authentication required",
    } satisfies ErrorBody;
    answer(`GET ${LIST_PATH}/${ADMIN001.user_id}`, 200, ADMIN001);
    answer(`DELETE ${LIST_PATH}/${ADMIN001.user_id}`, 200, selfDeleted);
    setSession(signedOn);
    onTestFinished(() => {
      clearSession();
    });

    // The page as App.tsx guards it, so the end of the session unmounts it.
    queryClient = newQueryClient();
    const router = createMemoryRouter(
      [
        { path: "/login", element: <p>Sign on</p> },
        {
          element: <RequireAuth />,
          children: [
            {
              path: "/admin/users",
              element: <UserListPage />,
              handle: {
                crumb: "Users",
                title: "List Users",
              } satisfies RouteHandle,
            },
          ],
        },
      ],
      { initialEntries: ["/admin/users"] },
    );
    render(
      <ThemeProvider>
        <QueryClientProvider client={queryClient}>
          <RouterProvider router={router} />
        </QueryClientProvider>
        <Toaster />
      </ThemeProvider>,
    );
    const user = userEvent.setup();
    await firstPageShown();

    await user.click(
      screen.getByRole("button", {
        name: "Delete",
        description: ADMIN001.user_id,
      }),
    );
    const dialog = await screen.findByRole("alertdialog", {
      name: "Delete User",
    });
    answer(`GET ${LIST_PATH}`, 401, authenticationRequired);
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

    expect((await screen.findByText("Sign on")).textContent).toBe("Sign on");
    await waitFor(() => {
      expect(queryClient.isMutating()).toBe(0);
    });
    // The re-read the delete started has been sent with the token of the
    // session it outlived, and answered.
    await waitFor(() => {
      expect(sentTo("GET", LIST_PATH)).toHaveLength(2);
    });
    await waitFor(() => {
      expect(
        fetchMock.mock.settledResults.every(
          (settled) => settled.type === "fulfilled",
        ),
      ).toBe(true);
    });
    expect(sentTo("GET", LIST_PATH)[1]?.headers.get("Authorization")).toBe(
      `Bearer ${signedOn.token}`,
    );
    expect(sentTo("DELETE", `${LIST_PATH}/${ADMIN001.user_id}`)).toHaveLength(
      1,
    );
    expect(getSession()).toBeNull();
    expect(toastSuccess.mock.calls).toEqual([[selfDeleted.message]]);
    expect(toastError).not.toHaveBeenCalled();
    // The text stays on screen over the sign-on route.
    expect(toastsShowing(selfDeleted.message.trim())).not.toEqual([]);
    expect(toastsShowing(authenticationRequired.message)).toEqual([]);
  });
});

// A refused CU03 read or delete is reported and leaves the list as it was: the
// toast carries the audited text, no dialog opens or stays open, the row stays
// listed on this route, nothing reports success, and the row's Delete reads
// and deletes again. The page renders no COUSR03 key field (E-58), so the
// fields an error names mark nothing, the search field's USRIDIN included.
describe("UserListPage CU03 delete failures", () => {
  function deleteOfUser0001(): HTMLElement {
    return screen.getByRole("button", {
      name: "Delete",
      description: USER0001.user_id,
    });
  }

  // Still this route's list, with USER0001 listed and USRIDIN unmarked.
  function listKept(): void {
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "List Users",
    );
    expect(screen.queryByText("Update User")).toBeNull();
    expect(screen.queryByText("Admin menu")).toBeNull();
    expect(within(usersTable()).getByText(USER0001.user_id).textContent).toBe(
      USER0001.user_id,
    );
    expect(startKeyInput().getAttribute("aria-invalid")).toBe("false");
    expect(fieldError(startKeyInput())).toBeNull();
  }

  // The read the row's Delete makes, READ-USER-SEC-FILE, fails.
  it.each([
    {
      row: "V-COUSR03C-002",
      status: 404,
      body: {
        code: "NOT_FOUND",
        message: "User ID NOT found...",
        fields: [
          {
            field: "USRIDIN" satisfies Schemas["UserUpdateField"],
            message: "User ID NOT found...",
          },
        ],
      } satisfies ErrorBody,
    },
    {
      row: "V-COUSR03C-003",
      status: 500,
      body: {
        code: "UNEXPECTED",
        message: "Unable to lookup User...",
        fields: [
          {
            field: "FNAME" satisfies Schemas["UserUpdateField"],
            message: "Unable to lookup User...",
          },
        ],
      } satisfies ErrorBody,
    },
  ])(
    "a $status read ($row) toasts its text and opens no dialog; Delete reads again",
    async ({ status, body }) => {
      answer(`GET ${LIST_PATH}/USER0001`, status, body);
      const { user } = renderPage();
      await firstPageShown();

      await user.click(deleteOfUser0001());

      await waitFor(() => {
        expect(toastError).toHaveBeenCalledWith(body.message);
      });
      expect(toastError).toHaveBeenCalledTimes(1);
      expect(sentTo("GET", `${LIST_PATH}/USER0001`)).toHaveLength(1);
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(sentTo("DELETE", `${LIST_PATH}/USER0001`)).toEqual([]);
      expect(toastSuccess).not.toHaveBeenCalled();
      listKept();

      // The next read is answered: the row's Delete reads the user again and
      // opens the dialog, whose Cancel still deletes nothing.
      answer(`GET ${LIST_PATH}/USER0001`, 200, USER0001);
      await user.click(deleteOfUser0001());

      const dialog = await screen.findByRole("alertdialog", {
        name: "Delete User",
      });
      expect(sentTo("GET", `${LIST_PATH}/USER0001`)).toHaveLength(2);
      expect(within(dialog).getByText(USER0001.first_name).textContent).toBe(
        USER0001.first_name,
      );
      await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
      });
      expect(sentTo("DELETE", `${LIST_PATH}/USER0001`)).toEqual([]);
      expect(toastError).toHaveBeenCalledTimes(1);
      expect(toastSuccess).not.toHaveBeenCalled();
    },
  );

  // The read found the user, and the delete Confirm sends,
  // DELETE-USER-SEC-FILE, fails.
  it.each([
    {
      row: "V-COUSR03C-006",
      status: 404,
      body: {
        code: "NOT_FOUND",
        message: "User ID NOT found...",
        fields: [
          {
            field: "USRIDIN" satisfies Schemas["UserDeleteField"],
            message: "User ID NOT found...",
          },
        ],
      } satisfies ErrorBody,
    },
    {
      row: "V-COUSR03C-007",
      status: 500,
      body: {
        code: "UNEXPECTED",
        message: "Unable to Update User...",
        fields: [
          {
            field: "FNAME" satisfies Schemas["UserDeleteField"],
            message: "Unable to Update User...",
          },
        ],
      } satisfies ErrorBody,
    },
  ])(
    "a $status delete ($row) toasts its text and keeps the row; Confirm deletes again",
    async ({ status, body }) => {
      answer(`DELETE ${LIST_PATH}/USER0001`, status, body);
      const { user } = renderPage();
      await firstPageShown();

      await user.click(deleteOfUser0001());
      const dialog = await screen.findByRole("alertdialog", {
        name: "Delete User",
      });
      await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

      await waitFor(() => {
        expect(toastError).toHaveBeenCalledWith(body.message);
      });
      expect(toastError).toHaveBeenCalledTimes(1);
      expect(sentTo("DELETE", `${LIST_PATH}/USER0001`)).toHaveLength(1);
      expect(toastSuccess).not.toHaveBeenCalled();
      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
      });
      // Only a delete that succeeded reads the list again.
      expect(listQueries()).toHaveLength(1);
      listKept();

      // The next delete is accepted: the row's Delete reads the user again,
      // and Confirm deletes it.
      answer(`DELETE ${LIST_PATH}/USER0001`, 200, DELETED);
      answer(`GET ${LIST_PATH}`, 200, AFTER_DELETE_PAGE);
      await user.click(deleteOfUser0001());
      const again = await screen.findByRole("alertdialog", {
        name: "Delete User",
      });
      expect(sentTo("GET", `${LIST_PATH}/USER0001`)).toHaveLength(2);
      await user.click(within(again).getByRole("button", { name: "Confirm" }));

      await waitFor(() => {
        expect(toastSuccess).toHaveBeenCalledWith(DELETED.message);
      });
      expect(toastSuccess).toHaveBeenCalledTimes(1);
      expect(sentTo("DELETE", `${LIST_PATH}/USER0001`)).toHaveLength(2);
      await waitFor(() => {
        expect(within(usersTable()).queryByText(USER0001.user_id)).toBeNull();
      });
      expect(toastError).toHaveBeenCalledTimes(1);
    },
  );
});

// A user ID that is a dot segment keeps its own edit route and its own
// requests: the URL holds its key segment, never a path the URL shortens.
describe("UserListPage dot-segment user IDs", () => {
  it.each(DOT_USER_CASES)(
    "Edit of the user '$listed.user_id' opens its CU02 route $href",
    async ({ listed, href }) => {
      answer(`GET ${LIST_PATH}`, 200, DOT_USERS_PAGE);
      const { user } = renderPage();
      await screen.findByText(ADMIN001.user_id);

      const edit = within(usersTable()).getByRole("link", {
        name: "Edit",
        description: listed.user_id,
      });
      expect(edit.getAttribute("href")).toBe(href);
      await user.click(edit);

      expect((await screen.findByText("Update User")).textContent).toBe(
        "Update User",
      );
    },
  );

  it.each(DOT_USER_CASES)(
    "Delete of the user '$listed.user_id' reads and deletes $segment",
    async ({ listed, segment }) => {
      const deleted = {
        message: `User ${listed.user_id} has been deleted ...`.padEnd(80, " "),
        user: listed,
      } satisfies UserMessageResponse;
      answer(`GET ${LIST_PATH}`, 200, DOT_USERS_PAGE);
      answer(`GET ${LIST_PATH}/${segment}`, 200, listed);
      answer(`DELETE ${LIST_PATH}/${segment}`, 200, deleted);
      const { user } = renderPage();
      await screen.findByText(ADMIN001.user_id);

      await user.click(
        screen.getByRole("button", {
          name: "Delete",
          description: listed.user_id,
        }),
      );
      const dialog = await screen.findByRole("alertdialog", {
        name: "Delete User",
      });
      expect(sentTo("GET", `${LIST_PATH}/${segment}`)).toHaveLength(1);
      expect(within(dialog).getByText(listed.first_name).textContent).toBe(
        listed.first_name,
      );
      await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

      await waitFor(() => {
        expect(toastSuccess).toHaveBeenCalledWith(deleted.message);
      });
      expect(sentTo("DELETE", `${LIST_PATH}/${segment}`)).toHaveLength(1);
      expect(toastError).not.toHaveBeenCalled();
    },
  );
});

// ---------------------------------------------------------------------------
// Invalid input
// ---------------------------------------------------------------------------

// The page renders no COUSR03 key field: the delete dialog is bound to its row
// (E-58), so the start key is the only field a user types, and its schema is
// the one that can refuse input.
describe("UserListPage start-key edits", () => {
  it("a start key outside ASCII sends no request and marks USRIDIN", async () => {
    const { user } = renderPage();
    await firstPageShown();
    const before = sent().length;

    const input = startKeyInput();
    await user.type(input, "USÉR001");
    await user.click(screen.getByRole("button", { name: "Search" }));

    await waitFor(() => {
      expect(input.getAttribute("aria-invalid")).toBe("true");
    });
    expect(fieldError(input)?.textContent).toBe(ZOD_REGEX_MESSAGE);
    expect(document.activeElement).toBe(input);
    expect(sent()).toHaveLength(before);
    expect(toastError).not.toHaveBeenCalled();
  });

  it("keeps the start key to the 8 characters of USRIDINI", async () => {
    const { user } = renderPage();
    await firstPageShown();

    const input = startKeyInput();
    await user.type(input, "USER00031{Enter}");

    await waitFor(() => {
      expect(listQueries()).toHaveLength(2);
    });
    expect(input.getAttribute("maxlength")).toBe("8");
    expect(listQueries()[1]?.get("start_key")).toBe("USER0003");
  });
});

describe("UserListPage server errors", () => {
  it.each([
    {
      name: "the audited browse failure (V-COUSR00C-004)",
      status: 500,
      body: LOOKUP_FAILED,
      toastText: LOOKUP_FAILED.message,
      fieldText: LOOKUP_FAILED.message,
    },
    {
      name: "the API's message-less start-key refusal (E-49)",
      status: 400,
      body: START_KEY_REFUSED,
      toastText: UNEXPECTED_ERROR,
      fieldText: null,
    },
  ])(
    "$name marks USRIDIN, toasts and moves the cursor there",
    async ({ status, body, toastText, fieldText }) => {
      answer(`GET ${LIST_PATH}?start_key=USER0003`, status, body);
      const { user } = renderPage();
      await firstPageShown();

      const input = startKeyInput();
      await user.type(input, USER0003.user_id);
      // Submitted from the button, so the cursor starts away from USRIDIN.
      const search = screen.getByRole("button", { name: "Search" });
      await user.click(search);

      await waitFor(() => {
        expect(input.getAttribute("aria-invalid")).toBe("true");
      });
      expect(fieldError(input)?.textContent ?? null).toBe(fieldText);
      expect(toastError).toHaveBeenCalledTimes(1);
      expect(toastError).toHaveBeenCalledWith(toastText);
      await waitFor(() => {
        expect(document.activeElement).toBe(input);
      });
      // The failed read supplied no rows, so its text stands in for them.
      expect(
        within(usersTable()).getByText(toastText).textContent,
      ).toBe(toastText);
      expect(listQueries().at(-1)?.get("start_key")).toBe(USER0003.user_id);
    },
  );

  // The same request read again keeps the query's last page in its cache; a
  // failure of that re-read must not leave the page shown as its answer.
  it.each([
    {
      name: "a repeated Search from the top",
      reread: async (user: ReturnType<typeof userEvent.setup>) => {
        await user.click(screen.getByRole("button", { name: "Search" }));
      },
    },
    {
      name: "the re-read after a delete",
      reread: async (user: ReturnType<typeof userEvent.setup>) => {
        await user.click(
          screen.getByRole("button", {
            name: "Delete",
            description: USER0001.user_id,
          }),
        );
        const dialog = await screen.findByRole("alertdialog", {
          name: "Delete User",
        });
        await user.click(
          within(dialog).getByRole("button", { name: "Confirm" }),
        );
      },
    },
  ])(
    "$name failing withholds the page it read before",
    async ({ reread }) => {
      const { user } = renderPage();
      await firstPageShown();
      expect(
        screen.getByRole("button", { name: "Next" }).getAttribute(
          "aria-disabled",
        ),
      ).toBeNull();

      answer(`GET ${LIST_PATH}`, 500, LOOKUP_FAILED);
      await reread(user);

      const table = usersTable();
      expect(
        (await within(table).findByText(LOOKUP_FAILED.message)).textContent,
      ).toBe(LOOKUP_FAILED.message);
      expect(listQueries()).toHaveLength(2);
      expect(listQueries()[1]?.toString()).toBe("");
      for (const shown of FIRST_PAGE.items) {
        expect(within(table).queryByText(shown.user_id)).toBeNull();
      }
      expect(within(table).queryByRole("link", { name: "Edit" })).toBeNull();
      expect(within(table).queryByRole("button", { name: "Delete" })).toBeNull();
      for (const control of ["Previous", "Next"]) {
        expect(
          screen
            .queryAllByRole("button", { name: control })
            .filter(
              (button) => button.getAttribute("aria-disabled") !== "true",
            ),
        ).toEqual([]);
      }
      expect(toastError).toHaveBeenCalledWith(LOOKUP_FAILED.message);
    },
  );

  // A return to the page and a repeated Search read a failed browse again
  // while the query still holds that failure beside the page read before it.
  // Until the re-read answers, the table is busy and shows neither the failure
  // nor the old page, and nothing is toasted or set on USRIDIN. Its answer
  // alone is then reported: the rows, with no toast and USRIDIN untouched,
  // once the server has recovered; one new toast, set on USRIDIN, when it
  // fails again. That failure carries a text of its own, so the toast names
  // the read it reports. Every visit mounts under StrictMode, as main.tsx
  // mounts the app, so a toast a mount effect repeats counts twice.
  it.each([
    { reread: "a return to the page", status: 200 },
    { reread: "a return to the page", status: 500 },
    { reread: "a repeated Search", status: 200 },
    { reread: "a repeated Search", status: 500 },
  ] as const)(
    "$reread after a failed browse reports its own $status answer alone",
    async ({ reread, status }) => {
      queryClient = newQueryClient();
      let { user } = mountPage(queryClient, true);
      await firstPageShown();
      answer(`GET ${LIST_PATH}`, 500, LOOKUP_FAILED);
      await user.click(screen.getByRole("button", { name: "Search" }));
      await within(usersTable()).findByText(LOOKUP_FAILED.message);
      expect(toastError.mock.calls).toEqual([[LOOKUP_FAILED.message]]);

      const readsBefore = listQueries().length;
      const released = deferred<undefined>();
      hold = released.promise;
      if (reread === "a return to the page") {
        cleanup();
        ({ user } = mountPage(queryClient, true));
      } else {
        await user.click(screen.getByRole("button", { name: "Search" }));
      }

      await waitFor(() => {
        expect(listQueries().length).toBeGreaterThan(readsBefore);
      });
      const busy = usersTable();
      expect(busy.getAttribute("aria-busy")).toBe("true");
      expect(within(busy).queryByText(LOOKUP_FAILED.message)).toBeNull();
      for (const shown of FIRST_PAGE.items) {
        expect(within(busy).queryByText(shown.user_id)).toBeNull();
      }
      for (const control of ["Previous", "Next"]) {
        expect(
          screen
            .getByRole("button", { name: control })
            .getAttribute("aria-disabled"),
        ).toBe("true");
      }
      expect(toastError).toHaveBeenCalledTimes(1);
      const input = startKeyInput();
      expect(input.getAttribute("aria-invalid")).toBe("false");
      expect(document.activeElement).not.toBe(input);

      const failedAgain = {
        code: "UNEXPECTED",
        message: "Read 2 failed",
        fields: [{ field: USRIDIN, message: "Read 2 failed" }],
      } satisfies ErrorBody;
      answer(
        `GET ${LIST_PATH}`,
        status,
        status === 200 ? FIRST_PAGE : failedAgain,
      );
      hold = null;
      released.resolve(undefined);

      await waitFor(() => {
        expect(queryClient.isFetching()).toBe(0);
      });
      if (status === 200) {
        await firstPageShown();
        expect(usersTable().hasAttribute("aria-busy")).toBe(false);
        expect(input.getAttribute("aria-invalid")).toBe("false");
        expect(fieldError(input)).toBeNull();
        expect(document.activeElement).not.toBe(input);
      } else {
        const table = usersTable();
        expect(
          (await within(table).findByText(failedAgain.message)).textContent,
        ).toBe(failedAgain.message);
        for (const shown of FIRST_PAGE.items) {
          expect(within(table).queryByText(shown.user_id)).toBeNull();
        }
        expect(input.getAttribute("aria-invalid")).toBe("true");
        expect(fieldError(input)?.textContent).toBe(failedAgain.message);
        await waitFor(() => {
          expect(document.activeElement).toBe(input);
        });
      }
      expect(toastError.mock.calls.slice(1)).toEqual(
        status === 200 ? [] : [[failedAgain.message]],
      );
    },
  );
});
