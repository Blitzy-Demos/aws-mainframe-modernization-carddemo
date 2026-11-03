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
  vi,
  type MockInstance,
} from "vitest";

import type { components } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";

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

const fetchMock = vi.fn(async (request: Request): Promise<Response> => {
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
// Rendering
// ---------------------------------------------------------------------------

let queryClient: QueryClient;
let toastError: MockInstance<typeof toast.error>;
let toastSuccess: MockInstance<typeof toast.success>;

beforeEach(() => {
  answers = defaultAnswers();
  unmatched = [];
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

// The page as App.tsx routes it, without RequireAdmin: the server's 403 is the
// admin gate, and admin_gate.rs and admin-gate.spec.ts test it. The /admin
// menu (PF3) and the CU02 edit route (selector U) are the page's link targets.
function renderPage() {
  queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
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
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
      <Toaster />
    </ThemeProvider>,
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
  // unchanged with their direction.
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

      expect(
        (await within(usersTable()).findByText(shows)).textContent,
      ).toBe(shows);
      const browse = listQueries().at(-1);
      expect(browse?.get("direction")).toBe(direction);
      expect(browse?.get("start_key")).toBe(startKey);
      expect(listQueries()).toHaveLength(2);
    },
  );
});

describe("UserListPage CU03 delete", () => {
  // F-COUSR00-006 and F-COUSR03-001 read the row's user into the dialog;
  // F-COUSR03-004 (PF5) deletes it.
  it("Delete reads the row's user into the dialog and Confirm deletes it", async () => {
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
});
