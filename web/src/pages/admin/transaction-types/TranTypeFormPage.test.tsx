// FN tests of the CTTU maintenance page (TranTypeFormPage.tsx; COTRTUPC, map
// COTRTUP), control by control as docs/parity/function-map.md lists them:
//
// - /admin/transaction-types/new: "Look up" reads the typed code (F-COTRTUP-001),
//   a code on file opens its edit route and an unknown one enables "Add", the
//   F5 add and the legend's F6=Add (F-COTRTUP-003, -008, -011), with the
//   COTRTUPC not-found text on the key, as the edit route of an unknown code
//   does; "Cancel" resets to the key entry (F-COTRTUP-015).
// - /admin/transaction-types/:code/edit: "Save" is the PUT without `origin`
//   (F-COTRTUP-002, -009); "Delete" opens the confirmation (F-COTRTUP-006),
//   whose "Cancel" closes it (F-COTRTUP-012) and whose "Confirm" sends the
//   DELETE with `origin=form` (F-COTRTUP-007); "Cancel" leaves for the key
//   entry (F-COTRTUP-014).
// - "Back" links to the list on both routes (F-COTRTUP-005).
// - One press sends one request, a double click included; while it runs the
//   form is busy and every button inert, the pressed one keeping the focus.
//
// Input the schema refuses is never sent, and a refused write's `fields` mark
// their form fields, toast the message and focus the first field. A write
// without an answer before the client's deadline is sent once, toasted with
// the deadline text and a note that it may have been carried out, and the
// type is read again. The network
// is a stubbed global answering as the API does over a one-type master (row 1
// of app/data/ASCII/trantype.txt). Every success and failure text is the
// stub's own, and the schema's texts come from the schema module.
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent, {
  PointerEventsCheckLevel,
} from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createMemoryRouter,
  RouterProvider,
  type InitialEntry,
} from "react-router";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { RequestTimeoutError } from "@/api/client";
import { tranTypeKeys } from "@/api/queries/transaction-types";
import type { components } from "@/api/schema";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import {
  TRAN_TYPE_UPSERT_MESSAGES,
  toTranTypeUpdateRequest,
  type TranTypeFormValues,
} from "@/schemas/transaction-types";

import TranTypeFormPage from "./TranTypeFormPage";

type ErrorBody = components["schemas"]["ErrorBody"];
type TranTypeCreateRequest = components["schemas"]["TranTypeCreateRequest"];
type TranTypeMessageResponse = components["schemas"]["TranTypeMessageResponse"];
type TranTypePage = components["schemas"]["Page_TranTypeResponse"];
type TranTypeResponse = components["schemas"]["TranTypeResponse"];
type TranTypeUpdateRequest = components["schemas"]["TranTypeUpdateRequest"];
type TranTypeUpsertField = components["schemas"]["TranTypeUpsertField"];

const LIST_ROUTE = "/admin/transaction-types";
const KEY_ENTRY_ROUTE = "/admin/transaction-types/new";
const EDIT_ROUTE_01 = "/admin/transaction-types/01/edit";

/** The API path of the list and the add, under the client's `/api` base. */
const LIST_PATH = "/api/admin/transaction-types";

/** Row 1 of app/data/ASCII/trantype.txt, the type every test finds on file. */
const PURCHASE = {
  type_code: "01",
  description: "Purchase",
} satisfies TranTypeResponse;

/** A code the fixture master does not hold (it holds 01 to 07). */
const NEW_CODE = "08";
const NEW_DESCRIPTION = "Test type";
const CHANGED_DESCRIPTION = "Retail purchase";

const STUB_NOT_ON_FILE = "Stub: no type matches the filter";
const STUB_ADDED = "Stub: type added";
const STUB_UPDATED = "Stub: type updated";
const STUB_DELETED = "Stub: type deleted";
const STUB_REJECTED = "Stub: write rejected";
const STUB_FIELD_REJECTED = "Stub: field rejected";

/** The client's text for a request past its deadline (api/client.ts). */
const DEADLINE_TEXT = "No answer from the server within 30 seconds";

/** One request the page sent, as the stub received it. */
type SentRequest = {
  method: string;
  url: URL;
  /** The parsed JSON body of a POST or PUT; absent for GET and DELETE. */
  body?: unknown;
};

/** The types on file, changed by the writes the stub accepts. */
let stored: TranTypeResponse[];
let sent: SentRequest[];
/** When set, every POST, PUT and DELETE is answered with this failure. */
let writeFailure: { status: number; body: ErrorBody } | undefined;
/** When set, every request is answered only once this promise settles. */
let gate: Promise<void> | undefined;
/**
 * When set, every POST, PUT and DELETE gets no answer before the client's
 * deadline: "unapplied" leaves the master as it was, as while the server
 * still waits on a lock; "applied" stores the write first, as when only its
 * answer is lost.
 */
let unanswered: "unapplied" | "applied" | undefined;

function answer(body: unknown, status: number): Response {
  return Response.json(body, { status });
}

function notOnFile(): Response {
  return answer(
    { code: "NOT_FOUND", message: STUB_NOT_ON_FILE } satisfies ErrorBody,
    404,
  );
}

/** The `{id}` segment of a `/api/admin/transaction-types/{id}` path. */
function typeCodeOf(url: URL): string {
  return decodeURIComponent(url.pathname.slice(LIST_PATH.length + 1));
}

/**
 * The API over `stored`: the list filtered by `type_code` (404 when nothing
 * matches), the add (201), the update and the delete (200, 404 for a code not
 * on file), each answered with the stored type.
 */
async function serve(request: Request): Promise<Response> {
  const url = new URL(request.url);
  const { method } = request;
  const hasBody = method === "POST" || method === "PUT";
  const body: unknown = hasBody ? await request.clone().json() : undefined;
  sent.push(hasBody ? { method, url, body } : { method, url });
  if (gate !== undefined) {
    await gate;
  }

  if (method === "GET" && url.pathname === LIST_PATH) {
    const code = url.searchParams.get("type_code");
    const items = stored.filter((type) => type.type_code === code);
    if (items.length === 0) {
      return notOnFile();
    }
    return answer(
      { items, next_key: null, prev_key: null } satisfies TranTypePage,
      200,
    );
  }
  if (writeFailure !== undefined) {
    return answer(writeFailure.body, writeFailure.status);
  }
  if (unanswered === "unapplied") {
    throw new RequestTimeoutError();
  }
  if (method === "POST" && url.pathname === LIST_PATH) {
    const created: TranTypeCreateRequest = await request.clone().json();
    const type: TranTypeResponse = {
      type_code: created.type_code ?? "",
      description: created.description ?? "",
    };
    stored.push(type);
    return answer(
      {
        message: STUB_ADDED,
        transaction_type: type,
      } satisfies TranTypeMessageResponse,
      201,
    );
  }
  const index = stored.findIndex((type) => type.type_code === typeCodeOf(url));
  if (index === -1) {
    return notOnFile();
  }
  if (method === "PUT") {
    const update: TranTypeUpdateRequest = await request.clone().json();
    const type: TranTypeResponse = {
      type_code: stored[index].type_code,
      description: update.description ?? "",
    };
    stored[index] = type;
    return answer(
      {
        message: STUB_UPDATED,
        transaction_type: type,
      } satisfies TranTypeMessageResponse,
      200,
    );
  }
  if (method === "DELETE") {
    const [deleted] = stored.splice(index, 1);
    return answer(
      {
        message: STUB_DELETED,
        transaction_type: deleted,
      } satisfies TranTypeMessageResponse,
      200,
    );
  }
  return answer(
    { code: "NOT_FOUND", message: STUB_NOT_ON_FILE } satisfies ErrorBody,
    404,
  );
}

const fetchMock = vi.fn(async (input: Request) => {
  const response = await serve(input);
  if (unanswered === "applied" && input.method !== "GET") {
    throw new RequestTimeoutError();
  }
  return response;
});

/** The POST, PUT and DELETE requests sent so far. */
function writes(): SentRequest[] {
  return sent.filter((request) => request.method !== "GET");
}

/** The 400 of a refused write, naming one BMS field of COTRTUP. */
function rejectWritesOn(field: TranTypeUpsertField): void {
  writeFailure = {
    status: 400,
    body: {
      code: "INVALID",
      message: STUB_REJECTED,
      fields: [{ field, message: STUB_FIELD_REJECTED }],
    } satisfies ErrorBody,
  };
}

/** Holds every request sent from now on until the returned release runs. */
function holdRequests(): () => void {
  let release = (): void => undefined;
  gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  return release;
}

beforeEach(() => {
  stored = [{ ...PURCHASE }];
  sent = [];
  writeFailure = undefined;
  gate = undefined;
  unanswered = undefined;
  fetchMock.mockClear();
  vi.stubGlobal("fetch", fetchMock);
  vi.spyOn(toast, "success");
  vi.spyOn(toast, "error");
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** The page at `path`, with the list route as the target of Back. */
function renderAt(path: InitialEntry) {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const router = createMemoryRouter(
    [
      {
        path: KEY_ENTRY_ROUTE,
        element: <TranTypeFormPage />,
        handle: { crumb: "New" },
      },
      {
        path: "/admin/transaction-types/:code/edit",
        element: <TranTypeFormPage />,
        handle: { crumb: "Edit" },
      },
      {
        path: LIST_ROUTE,
        element: <p>list</p>,
        handle: { crumb: "Transaction Types" },
      },
    ],
    { initialEntries: [path] },
  );
  render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
        <Toaster />
      </QueryClientProvider>
    </ThemeProvider>,
  );
  // The Radix alert dialog disables pointer events outside its content, which
  // user-event's check reads from the body rather than the dialog buttons.
  const user = userEvent.setup({
    pointerEventsCheck: PointerEventsCheckLevel.Never,
  });
  return { router, user, queryClient };
}

function inputLabelled(label: string): HTMLInputElement {
  const element = screen.getByLabelText(label);
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`"${label}" labels no input`);
  }
  return element;
}

/** The text of the FieldError the input names as its description. */
function fieldErrorOf(input: HTMLInputElement): string | null {
  const id = input.getAttribute("aria-describedby");
  const error = id === null ? null : document.getElementById(id);
  if (error === null) {
    return null;
  }
  expect(error.getAttribute("role")).toBe("alert");
  return error.textContent;
}

type User = ReturnType<typeof userEvent.setup>;

/** Looks up `entry` on the key entry and waits for the add of a new code. */
async function lookUpNewCode(user: User, entry: string): Promise<void> {
  await user.type(inputLabelled("Type code"), entry);
  await user.click(screen.getByRole("button", { name: "Look up" }));
  await screen.findByRole("button", { name: "Add" });
}

/**
 * Waits until no read is running, then lets the task pass on which TanStack
 * Query hands the last answer to the page, so its effects have seen it.
 */
async function readsSettled(queryClient: QueryClient): Promise<void> {
  await waitFor(() => {
    expect(queryClient.isFetching()).toBe(0);
  });
  await act(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
  });
}

/** Waits until the edit route shows the type on file for update. */
async function waitForPrefill(description: string): Promise<void> {
  await waitFor(() => {
    const input = inputLabelled("Description");
    expect(input.value).toBe(description);
    expect(input.disabled).toBe(false);
  });
}

function buttonNamed(name: string): HTMLElement {
  return screen.getByRole("button", { name });
}

function maintenanceForm(): HTMLElement {
  return screen.getByRole("form", { name: "Transaction type" });
}

/**
 * The form while its request runs: busy, `submit` and every button in
 * `siblings` inert but not disabled, so `submit` keeps the focus.
 */
function expectBusy(submit: string, siblings: readonly string[]): void {
  expect(maintenanceForm().getAttribute("aria-busy")).toBe("true");
  for (const name of [submit, ...siblings]) {
    expect(buttonNamed(name).getAttribute("aria-disabled")).toBe("true");
    expect(buttonNamed(name).hasAttribute("disabled")).toBe(false);
  }
  expect(document.activeElement).toBe(buttonNamed(submit));
}

/** Every button in `names` live again, and the form no longer busy. */
function expectIdle(names: readonly string[]): void {
  expect(maintenanceForm().getAttribute("aria-busy")).toBe("false");
  for (const name of names) {
    expect(buttonNamed(name).getAttribute("aria-disabled")).toBe("false");
  }
}

describe("key entry and add (/admin/transaction-types/new)", () => {
  it("adds a code not on file with one POST of the code and description", async () => {
    const { user } = renderAt(KEY_ENTRY_ROUTE);
    await lookUpNewCode(user, NEW_CODE);

    expect(sent).toHaveLength(1);
    expect(sent[0].method).toBe("GET");
    expect(sent[0].url.pathname).toBe(LIST_PATH);
    expect(sent[0].url.searchParams.get("type_code")).toBe(NEW_CODE);
    // The not-found answer opens the add; its text belongs to the list.
    expect(toast.error).not.toHaveBeenCalled();
    // The add posts the key the Look up found absent (E-70).
    expect(inputLabelled("Type code").readOnly).toBe(true);

    await user.type(inputLabelled("Description"), NEW_DESCRIPTION);
    await user.click(screen.getByRole("button", { name: "Add" }));

    await waitFor(() => {
      expect(toast.success).toHaveBeenCalledWith(STUB_ADDED);
    });
    expect(writes()).toHaveLength(1);
    const [post] = writes();
    expect(post.method).toBe("POST");
    expect(post.url.pathname).toBe(LIST_PATH);
    expect(post.body).toEqual({
      type_code: NEW_CODE,
      description: NEW_DESCRIPTION,
    });
    // Back to the empty key entry for the next type.
    await screen.findByRole("button", { name: "Look up" });
    expect(inputLabelled("Type code").value).toBe("");
    expect(screen.queryByRole("button", { name: "Add" })).toBeNull();
  });

  it.each([
    { entry: "01", key: "01" },
    { entry: "1", key: "01" },
  ])(
    "opens the edit route of the code on file for $entry",
    async ({ entry, key }) => {
      const { router, user } = renderAt(KEY_ENTRY_ROUTE);
      await user.type(inputLabelled("Type code"), entry);
      await user.click(screen.getByRole("button", { name: "Look up" }));

      await waitForPrefill(PURCHASE.description);
      expect(router.state.location.pathname).toBe(EDIT_ROUTE_01);
      expect(sent[0].url.searchParams.get("type_code")).toBe(key);
      expect(screen.getByRole("button", { name: "Save" })).toBeDefined();
      expect(writes()).toEqual([]);
    },
  );

  it.each([
    { entry: "", message: TRAN_TYPE_UPSERT_MESSAGES.typeCodeMustBeSupplied },
    { entry: "A1", message: TRAN_TYPE_UPSERT_MESSAGES.typeCodeMustBeNumeric },
    { entry: "00", message: TRAN_TYPE_UPSERT_MESSAGES.typeCodeMustNotBeZero },
  ])(
    "refuses the key $entry without a request",
    async ({ entry, message }) => {
      const { user } = renderAt(KEY_ENTRY_ROUTE);
      const typeCode = inputLabelled("Type code");
      if (entry !== "") {
        await user.type(typeCode, entry);
      }
      await user.click(screen.getByRole("button", { name: "Look up" }));

      await waitFor(() => {
        expect(fieldErrorOf(typeCode)).toBe(message);
      });
      expect(typeCode.getAttribute("aria-invalid")).toBe("true");
      expect(document.activeElement).toBe(typeCode);
      expect(sent).toEqual([]);
    },
  );

  it.each([
    {
      description: "",
      message: TRAN_TYPE_UPSERT_MESSAGES.descriptionMustBeSupplied,
    },
    {
      description: "Test-type",
      message: TRAN_TYPE_UPSERT_MESSAGES.descriptionAlphanumericOnly,
    },
  ])(
    "refuses the description $description without a POST",
    async ({ description, message }) => {
      const { user } = renderAt(KEY_ENTRY_ROUTE);
      await lookUpNewCode(user, NEW_CODE);
      const input = inputLabelled("Description");
      if (description !== "") {
        await user.type(input, description);
      }
      await user.click(screen.getByRole("button", { name: "Add" }));

      await waitFor(() => {
        expect(fieldErrorOf(input)).toBe(message);
      });
      expect(document.activeElement).toBe(input);
      expect(writes()).toEqual([]);
    },
  );

  it.each([
    { field: "TRTYPCD", label: "Type code" },
    { field: "TRTYDSC", label: "Description" },
  ] satisfies { field: TranTypeUpsertField; label: string }[])(
    "marks, toasts and focuses $field of a refused POST",
    async ({ field, label }) => {
      rejectWritesOn(field);
      const { user } = renderAt(KEY_ENTRY_ROUTE);
      await lookUpNewCode(user, NEW_CODE);
      await user.type(inputLabelled("Description"), NEW_DESCRIPTION);
      await user.click(screen.getByRole("button", { name: "Add" }));

      const marked = inputLabelled(label);
      await waitFor(() => {
        expect(fieldErrorOf(marked)).toBe(STUB_FIELD_REJECTED);
      });
      expect(marked.getAttribute("aria-invalid")).toBe("true");
      expect(toast.error).toHaveBeenCalledWith(STUB_REJECTED);
      await waitFor(() => {
        expect(document.activeElement).toBe(marked);
      });
      expect(toast.success).not.toHaveBeenCalled();
      expect(writes().map((request) => request.method)).toEqual(["POST"]);
      // The add stays open with the entry, for the user to correct it.
      expect(inputLabelled("Description").value).toBe(NEW_DESCRIPTION);
    },
  );

  it("Cancel abandons the add and returns to the empty key entry (F12)", async () => {
    const { router, user } = renderAt(KEY_ENTRY_ROUTE);
    await lookUpNewCode(user, NEW_CODE);
    await user.type(inputLabelled("Description"), NEW_DESCRIPTION);
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    await screen.findByRole("button", { name: "Look up" });
    expect(router.state.location.pathname).toBe(KEY_ENTRY_ROUTE);
    expect(inputLabelled("Type code").value).toBe("");
    expect(inputLabelled("Type code").readOnly).toBe(false);
    expect(inputLabelled("Description").value).toBe("");
    expect(inputLabelled("Description").disabled).toBe(true);
    expect(writes()).toEqual([]);
  });

  it("offers no delete before a type on file is shown", () => {
    renderAt(KEY_ENTRY_ROUTE);
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Add" })).toBeNull();
  });
});

// COTRTUPC TTUP-DETAILS-NOT-FOUND (F-COTRTUP-001, -008): a code no type has
// opens its add whether the Look up or its edit route reads it, with the
// read's text on the key (V-COTRTUPC-004) rather than the list's text of the
// 404 that answered the read. Any other read failure stays on the edit route.
describe("a code not on file (COTRTUPC key not found)", () => {
  const EDIT_ROUTE_NEW = `${LIST_ROUTE}/${NEW_CODE}/edit`;
  const STUB_READ_FAILED = "Stub: read failed";

  /** The API, with the list read answered by `read` instead. */
  function stubListRead(read: (url: URL) => Response): void {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (request: Request) => {
        const url = new URL(request.url);
        if (request.method === "GET" && url.pathname === LIST_PATH) {
          sent.push({ method: "GET", url });
          return read(url);
        }
        return serve(request);
      }),
    );
  }

  /** A 200 page of the types on file, none of them the code asked for. */
  function pageWithoutTheCode(): Response {
    return answer(
      { items: stored, next_key: null, prev_key: null } satisfies TranTypePage,
      200,
    );
  }

  it.each([
    { arrival: "Look up", read: "404", pageWithout: false, action: "POP" },
    { arrival: "Look up", read: "200", pageWithout: true, action: "POP" },
    {
      arrival: "edit route",
      read: "404",
      pageWithout: false,
      action: "REPLACE",
    },
    {
      arrival: "edit route",
      read: "200",
      pageWithout: true,
      action: "REPLACE",
    },
  ])(
    "the $arrival read answered $read opens the add of the key, marked not found",
    async ({ arrival, pageWithout, action }) => {
      if (pageWithout) {
        stubListRead(pageWithoutTheCode);
      }
      const { router, user } = renderAt(
        arrival === "Look up" ? KEY_ENTRY_ROUTE : EDIT_ROUTE_NEW,
      );
      if (arrival === "Look up") {
        await lookUpNewCode(user, NEW_CODE);
      }

      await screen.findByRole("button", { name: "Add" });
      const typeCode = inputLabelled("Type code");
      await waitFor(() => {
        expect(fieldErrorOf(typeCode)).toBe(
          TRAN_TYPE_UPSERT_MESSAGES.recordNotFound,
        );
      });
      expect(typeCode.getAttribute("aria-invalid")).toBe("true");
      expect(typeCode.value).toBe(NEW_CODE);
      expect(typeCode.readOnly).toBe(true);
      const description = inputLabelled("Description");
      expect(description.disabled).toBe(false);
      await waitFor(() => {
        expect(document.activeElement).toBe(description);
      });
      expect(screen.getByRole("button", { name: "Cancel" })).toBeDefined();
      expect(screen.queryByRole("button", { name: "Look up" })).toBeNull();
      expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
      // The edit route is replaced, so Back does not return to it, and the
      // hand-over is taken off the history entry once applied.
      await waitFor(() => {
        expect(router.state.location.pathname).toBe(KEY_ENTRY_ROUTE);
        expect(router.state.location.state).toBeNull();
      });
      expect(router.state.historyAction).toBe(action);
      // The list's text for the 404 is never shown.
      expect(toast.error).not.toHaveBeenCalled();
      expect(screen.queryByText(STUB_NOT_ON_FILE)).toBeNull();
      expect(sent.map((request) => request.method)).toEqual(["GET"]);
      expect(sent[0].url.searchParams.get("type_code")).toBe(NEW_CODE);
    },
  );

  it.each([
    { action: "Add", phase: "add" },
    { action: "Cancel", phase: "key entry" },
  ])(
    "$action clears the not-found text, leaving the $phase",
    async ({ action }) => {
      const { user } = renderAt(KEY_ENTRY_ROUTE);
      await lookUpNewCode(user, NEW_CODE);
      const typeCode = inputLabelled("Type code");
      await waitFor(() => {
        expect(fieldErrorOf(typeCode)).toBe(
          TRAN_TYPE_UPSERT_MESSAGES.recordNotFound,
        );
      });

      // The Add re-runs the edits, which refuse the blank description, and
      // Cancel resets the entry: either way the key is no longer marked.
      await user.click(screen.getByRole("button", { name: action }));

      await waitFor(() => {
        expect(typeCode.getAttribute("aria-invalid")).toBe("false");
      });
      expect(fieldErrorOf(typeCode)).toBeNull();
      expect(
        screen.queryByText(TRAN_TYPE_UPSERT_MESSAGES.recordNotFound),
      ).toBeNull();
      expect(writes()).toEqual([]);
    },
  );

  it.each<{ handOver: string; entry: InitialEntry }>([
    { handOver: "the edit route of 00", entry: `${LIST_ROUTE}/00/edit` },
    {
      handOver: "a code that is no string",
      entry: { pathname: KEY_ENTRY_ROUTE, state: { notFoundTypeCode: 8 } },
    },
    {
      handOver: "a code the read would change",
      entry: { pathname: KEY_ENTRY_ROUTE, state: { notFoundTypeCode: "8" } },
    },
    {
      handOver: "a state of another shape",
      entry: { pathname: KEY_ENTRY_ROUTE, state: NEW_CODE },
    },
  ])(
    "$handOver opens the empty key entry, as no add can take it",
    async ({ entry }) => {
      const { router } = renderAt(entry);

      await screen.findByRole("button", { name: "Look up" });
      expect(router.state.location.pathname).toBe(KEY_ENTRY_ROUTE);
      const typeCode = inputLabelled("Type code");
      expect(typeCode.value).toBe("");
      expect(typeCode.readOnly).toBe(false);
      expect(typeCode.getAttribute("aria-invalid")).toBe("false");
      expect(fieldErrorOf(typeCode)).toBeNull();
      expect(inputLabelled("Description").disabled).toBe(true);
      expect(screen.queryByRole("button", { name: "Add" })).toBeNull();
      expect(toast.error).not.toHaveBeenCalled();
      expect(writes()).toEqual([]);
    },
  );

  it("a read failing otherwise stays on the edit route and toasts once", async () => {
    stubListRead(() =>
      answer(
        { code: "UNEXPECTED", message: STUB_READ_FAILED } satisfies ErrorBody,
        500,
      ),
    );
    const { router } = renderAt(EDIT_ROUTE_NEW);

    await waitFor(() => {
      expect(toast.error).toHaveBeenCalledWith(STUB_READ_FAILED);
    });
    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(router.state.location.pathname).toBe(EDIT_ROUTE_NEW);
    expect(screen.queryByRole("button", { name: "Add" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Look up" })).toBeNull();
    expect(writes()).toEqual([]);
  });
});

// The fields show their state as User Update's do: a read-only key on the
// muted surface in both themes, the Input's own dark surface giving way, and
// a disabled description's Field marked so that its label dims with it.
describe("field cues of each state", () => {
  it.each([
    {
      state: "key entry",
      path: KEY_ENTRY_ROUTE,
      keyReadOnly: false,
      descriptionDisabled: true,
    },
    {
      state: "add",
      path: KEY_ENTRY_ROUTE,
      keyReadOnly: true,
      descriptionDisabled: false,
    },
    {
      state: "edit",
      path: EDIT_ROUTE_01,
      keyReadOnly: true,
      descriptionDisabled: false,
    },
  ])(
    "the $state state shows whether the key is read-only and the description disabled",
    async ({ state, path, keyReadOnly, descriptionDisabled }) => {
      const { user } = renderAt(path);
      if (state === "add") {
        await lookUpNewCode(user, NEW_CODE);
      }
      if (state === "edit") {
        await waitForPrefill(PURCHASE.description);
      }

      const typeCode = inputLabelled("Type code");
      expect(typeCode.readOnly).toBe(keyReadOnly);
      expect(typeCode.classList.contains("bg-muted")).toBe(keyReadOnly);
      expect(typeCode.classList.contains("dark:bg-muted")).toBe(keyReadOnly);
      expect(typeCode.classList.contains("dark:bg-input/30")).toBe(
        !keyReadOnly,
      );
      const description = inputLabelled("Description");
      expect(description.disabled).toBe(descriptionDisabled);
      const field = description.closest('[data-slot="field"]');
      expect(field?.getAttribute("data-disabled")).toBe(
        String(descriptionDisabled),
      );
    },
  );
});

describe("update and delete (/admin/transaction-types/:code/edit)", () => {
  it("saves the description with a PUT without origin", async () => {
    const { user } = renderAt(EDIT_ROUTE_01);
    await waitForPrefill(PURCHASE.description);

    expect(sent[0].url.pathname).toBe(LIST_PATH);
    expect(sent[0].url.searchParams.get("type_code")).toBe(PURCHASE.type_code);
    const typeCode = inputLabelled("Type code");
    expect(typeCode.value).toBe(PURCHASE.type_code);
    expect(typeCode.readOnly).toBe(true);

    const description = inputLabelled("Description");
    await user.clear(description);
    await user.type(description, CHANGED_DESCRIPTION);
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(toast.success).toHaveBeenCalledWith(STUB_UPDATED);
    });
    expect(writes()).toHaveLength(1);
    const [put] = writes();
    expect(put.method).toBe("PUT");
    expect(put.url.pathname).toBe(`${LIST_PATH}/${PURCHASE.type_code}`);
    expect(put.url.searchParams.has("origin")).toBe(false);
    const values: TranTypeFormValues = {
      type_code: PURCHASE.type_code,
      description: CHANGED_DESCRIPTION,
    };
    expect(put.body).toEqual(toTranTypeUpdateRequest(values));
    expect(inputLabelled("Description").value).toBe(CHANGED_DESCRIPTION);
  });

  it("refuses a blank description without a PUT", async () => {
    const { user } = renderAt(EDIT_ROUTE_01);
    await waitForPrefill(PURCHASE.description);
    const description = inputLabelled("Description");
    await user.clear(description);
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(fieldErrorOf(description)).toBe(
        TRAN_TYPE_UPSERT_MESSAGES.descriptionMustBeSupplied,
      );
    });
    expect(document.activeElement).toBe(description);
    expect(writes()).toEqual([]);
  });

  it("marks, toasts and focuses TRTYDSC of a refused PUT", async () => {
    rejectWritesOn("TRTYDSC");
    const { user } = renderAt(EDIT_ROUTE_01);
    await waitForPrefill(PURCHASE.description);
    const description = inputLabelled("Description");
    await user.clear(description);
    await user.type(description, CHANGED_DESCRIPTION);
    // Focus leaves the field, so the assertion below sees it come back.
    await user.tab();
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(fieldErrorOf(description)).toBe(STUB_FIELD_REJECTED);
    });
    expect(toast.error).toHaveBeenCalledWith(STUB_REJECTED);
    await waitFor(() => {
      expect(document.activeElement).toBe(description);
    });
    expect(writes().map((request) => request.method)).toEqual(["PUT"]);
  });

  // The server may still store the description after the client gave up, so
  // the toast says so, the PUT is not sent again, and the type is read again.
  // The master still holds the old row, so the form keeps what was typed.
  it("toasts a PUT past the deadline as possibly saved, sends it once and reads the type again", async () => {
    unanswered = "unapplied";
    const { user, queryClient } = renderAt(EDIT_ROUTE_01);
    await waitForPrefill(PURCHASE.description);
    const description = inputLabelled("Description");
    await user.clear(description);
    await user.type(description, CHANGED_DESCRIPTION);
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(toast.error).toHaveBeenCalledWith(DEADLINE_TEXT, {
        description:
          "The description may have been saved. Check the type before saving it again.",
        duration: Infinity,
      });
    });
    await waitFor(() => {
      expect(sent.map((request) => request.method)).toEqual([
        "GET",
        "PUT",
        "GET",
      ]);
    });
    await readsSettled(queryClient);
    const reread = sent[2];
    expect(reread.url.pathname).toBe(LIST_PATH);
    expect(reread.url.searchParams.get("type_code")).toBe(PURCHASE.type_code);
    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(toast.success).not.toHaveBeenCalled();
    expect(inputLabelled("Description").value).toBe(CHANGED_DESCRIPTION);
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Save" }).disabled,
    ).toBe(false);
  });

  // F4 is carried as the COTRTUPC delete (E-23, F-COTRTUP-006, -007, -012):
  // the Delete button only asks for confirmation, the dialog's Cancel sends
  // nothing, and its Confirm sends the one DELETE with origin=form. This
  // follows the function map's rows rather than a "no request" reading of F4.
  it("deletes only once Delete is confirmed, with origin=form (F4)", async () => {
    const { router, user } = renderAt(EDIT_ROUTE_01);
    await waitForPrefill(PURCHASE.description);

    await user.click(screen.getByRole("button", { name: "Delete" }));
    const asked = await screen.findByRole("alertdialog", {
      name: "Delete transaction type",
    });
    expect(writes()).toEqual([]);
    await user.click(within(asked).getByRole("button", { name: "Cancel" }));
    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });
    expect(writes()).toEqual([]);
    expect(router.state.location.pathname).toBe(EDIT_ROUTE_01);

    await user.click(screen.getByRole("button", { name: "Delete" }));
    const confirming = await screen.findByRole("alertdialog", {
      name: "Delete transaction type",
    });
    await user.click(
      within(confirming).getByRole("button", { name: "Confirm" }),
    );

    await waitFor(() => {
      expect(router.state.location.pathname).toBe(KEY_ENTRY_ROUTE);
    });
    await screen.findByRole("button", { name: "Look up" });
    expect(toast.success).toHaveBeenCalledWith(STUB_DELETED);
    expect(toast.error).not.toHaveBeenCalled();
    expect(writes()).toHaveLength(1);
    const [deleted] = writes();
    expect(deleted.method).toBe("DELETE");
    expect(deleted.url.pathname).toBe(`${LIST_PATH}/${PURCHASE.type_code}`);
    expect(deleted.url.searchParams.get("origin")).toBe("form");
    // The page leaves rather than reading the deleted type again.
    const afterDelete = sent.slice(sent.indexOf(deleted) + 1);
    expect(afterDelete.filter((request) => request.method === "GET")).toEqual(
      [],
    );
  });

  // The delete is stored but its answer lost: the toast says the type may
  // have been deleted, and the type read again answers 404, the delete's own
  // outcome, which is not toasted as an error of the page. That read finds
  // the key not on file, so the key entry opens on its add with COTRTUPC's
  // not-found text, which shows that the type is gone.
  it("toasts a DELETE whose answer is lost as possibly done and toasts nothing for the 404 read after it", async () => {
    unanswered = "applied";
    const { router, user, queryClient } = renderAt(EDIT_ROUTE_01);
    await waitForPrefill(PURCHASE.description);
    await user.click(screen.getByRole("button", { name: "Delete" }));
    const confirming = await screen.findByRole("alertdialog", {
      name: "Delete transaction type",
    });
    await user.click(
      within(confirming).getByRole("button", { name: "Confirm" }),
    );

    await waitFor(() => {
      expect(toast.error).toHaveBeenCalledWith(DEADLINE_TEXT, {
        description:
          "The type may have been deleted. Check the list before deleting it again.",
        duration: Infinity,
      });
    });
    await waitFor(() => {
      expect(sent.map((request) => request.method)).toEqual([
        "GET",
        "DELETE",
        "GET",
      ]);
    });
    await readsSettled(queryClient);
    expect(
      queryClient.getQueryState(
        tranTypeKeys.list({ type_code: PURCHASE.type_code }),
      )?.status,
    ).toBe("error");
    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(toast.success).not.toHaveBeenCalled();
    expect(writes()).toHaveLength(1);
    await screen.findByRole("button", { name: "Add" });
    const typeCode = inputLabelled("Type code");
    await waitFor(() => {
      expect(fieldErrorOf(typeCode)).toBe(
        TRAN_TYPE_UPSERT_MESSAGES.recordNotFound,
      );
    });
    expect(typeCode.value).toBe(PURCHASE.type_code);
    expect(router.state.location.pathname).toBe(KEY_ENTRY_ROUTE);
  });

  it("Cancel leaves for the empty key entry without a request (F12)", async () => {
    const { router, user } = renderAt(EDIT_ROUTE_01);
    await waitForPrefill(PURCHASE.description);
    await user.type(inputLabelled("Description"), " unsaved");
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    await screen.findByRole("button", { name: "Look up" });
    expect(router.state.location.pathname).toBe(KEY_ENTRY_ROUTE);
    expect(inputLabelled("Type code").value).toBe("");
    expect(writes()).toEqual([]);
  });
});

// A refused confirmed delete (F-COTRTUP-007) keeps the type shown for update:
// the toast carries the server's text, the field the answer names is marked
// and takes the cursor (COTRTUPC names TRTYPCD for every delete failure,
// V-COTRTUPC-025, -027, -028), the route keeps its Save and Delete, and nothing
// reports success. Delete then asks again, and a confirmed retry deletes.
describe("refused delete (/admin/transaction-types/:code/edit)", () => {
  const STUB_DELETE_REFUSED = "Stub: delete refused";
  const STUB_KEY_REFUSED = "Stub: key refused";

  // A 4xx refuses the delete before anything is stored; a 5xx may follow a
  // stored delete, so the type is read again (api/queries/transaction-types.ts
  // mayHaveWritten), and it is still on file.
  it.each([
    { row: "V-COTRTUPC-025", status: 404, code: "NOT_FOUND", reread: false },
    { row: "V-COTRTUPC-027", status: 409, code: "CONFLICT", reread: false },
    { row: "V-COTRTUPC-028", status: 500, code: "UNEXPECTED", reread: true },
  ])(
    "a $status DELETE ($row) marks TRTYPCD and keeps the type for update",
    async ({ status, code, reread }) => {
      const field: TranTypeUpsertField = "TRTYPCD";
      writeFailure = {
        status,
        body: {
          code,
          message: STUB_DELETE_REFUSED,
          fields: [{ field, message: STUB_KEY_REFUSED }],
        } satisfies ErrorBody,
      };
      const { router, user } = renderAt(EDIT_ROUTE_01);
      await waitForPrefill(PURCHASE.description);

      await user.click(screen.getByRole("button", { name: "Delete" }));
      const asked = await screen.findByRole("alertdialog", {
        name: "Delete transaction type",
      });
      await user.click(within(asked).getByRole("button", { name: "Confirm" }));

      const typeCode = inputLabelled("Type code");
      await waitFor(() => {
        expect(fieldErrorOf(typeCode)).toBe(STUB_KEY_REFUSED);
      });
      expect(typeCode.getAttribute("aria-invalid")).toBe("true");
      expect(toast.error).toHaveBeenCalledWith(STUB_DELETE_REFUSED);
      expect(toast.error).toHaveBeenCalledTimes(1);
      await waitFor(() => {
        expect(document.activeElement).toBe(typeCode);
      });
      expect(toast.success).not.toHaveBeenCalled();
      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
      });
      // The type and its edit route stay; the page has not left for the key
      // entry, and only a 5xx reads the type again.
      expect(router.state.location.pathname).toBe(EDIT_ROUTE_01);
      expect(typeCode.value).toBe(PURCHASE.type_code);
      expect(inputLabelled("Description").value).toBe(PURCHASE.description);
      expect(screen.getByRole("button", { name: "Save" }).textContent).toBe(
        "Save",
      );
      expect(screen.queryByRole("button", { name: "Look up" })).toBeNull();
      const [refused] = writes();
      expect(writes()).toHaveLength(1);
      expect(refused.method).toBe("DELETE");
      expect(refused.url.pathname).toBe(`${LIST_PATH}/${PURCHASE.type_code}`);
      expect(refused.url.searchParams.get("origin")).toBe("form");
      await waitFor(() => {
        expect(
          sent
            .slice(sent.indexOf(refused) + 1)
            .map((request) => [
              request.method,
              request.url.searchParams.get("type_code"),
            ]),
        ).toEqual(reread ? [["GET", PURCHASE.type_code]] : []);
      });
      expect(fieldErrorOf(typeCode)).toBe(STUB_KEY_REFUSED);

      // Delete asks again, and the confirmed retry, now accepted, deletes.
      writeFailure = undefined;
      const remove = screen.getByRole("button", { name: "Delete" });
      expect(remove.hasAttribute("disabled")).toBe(false);
      await user.click(remove);
      const again = await screen.findByRole("alertdialog", {
        name: "Delete transaction type",
      });
      await user.click(within(again).getByRole("button", { name: "Confirm" }));

      await waitFor(() => {
        expect(router.state.location.pathname).toBe(KEY_ENTRY_ROUTE);
      });
      expect(toast.success).toHaveBeenCalledWith(STUB_DELETED);
      expect(writes().map((request) => request.method)).toEqual([
        "DELETE",
        "DELETE",
      ]);
      expect(toast.error).toHaveBeenCalledTimes(1);
    },
  );
});

it.each([KEY_ENTRY_ROUTE, EDIT_ROUTE_01])(
  "titles the form on %s with its level-2 card heading",
  async (path) => {
    renderAt(path);
    if (path === EDIT_ROUTE_01) {
      await waitForPrefill(PURCHASE.description);
    }

    expect(
      screen.getByRole("heading", { level: 2, name: "Transaction type" }),
    ).toBeTruthy();
    expect(screen.getByRole("form", { name: "Transaction type" })).toBeTruthy();
  },
);

it.each([KEY_ENTRY_ROUTE, EDIT_ROUTE_01])(
  "Back on %s returns to the list (F3)",
  async (path) => {
    const { router, user } = renderAt(path);
    if (path === EDIT_ROUTE_01) {
      await waitForPrefill(PURCHASE.description);
    }
    const back = screen.getByRole("link", { name: "Back" });
    expect(back.getAttribute("href")).toBe(LIST_ROUTE);
    await user.click(back);

    await screen.findByText("list");
    expect(router.state.location.pathname).toBe(LIST_ROUTE);
    expect(writes()).toEqual([]);
  },
);

describe("one request at a time, the submit keeping the focus", () => {
  // Two clicks before the page re-renders send one request: the second Add
  // of the same code would otherwise be answered 409 for the type the first
  // one added.
  it.each([
    {
      control: "Look up",
      path: KEY_ENTRY_ROUTE,
      prepare: async (user: User) => {
        await user.type(inputLabelled("Type code"), NEW_CODE);
      },
      settled: async () => {
        await screen.findByRole("button", { name: "Add" });
      },
      requests: () => sent.filter((request) => request.method === "GET"),
      success: undefined,
    },
    {
      control: "Add",
      path: KEY_ENTRY_ROUTE,
      prepare: async (user: User) => {
        await lookUpNewCode(user, NEW_CODE);
        await user.type(inputLabelled("Description"), NEW_DESCRIPTION);
      },
      settled: async () => {
        await screen.findByRole("button", { name: "Look up" });
      },
      requests: writes,
      success: STUB_ADDED,
    },
    {
      control: "Save",
      path: EDIT_ROUTE_01,
      prepare: async (user: User) => {
        await waitForPrefill(PURCHASE.description);
        await user.clear(inputLabelled("Description"));
        await user.type(inputLabelled("Description"), CHANGED_DESCRIPTION);
      },
      settled: async () => {
        await waitFor(() => {
          expect(buttonNamed("Save").getAttribute("aria-disabled")).toBe(
            "false",
          );
        });
      },
      requests: writes,
      success: STUB_UPDATED,
    },
  ])(
    "sends one request for a double click on $control",
    async ({ control, path, prepare, settled, requests, success }) => {
      const { user } = renderAt(path);
      await prepare(user);
      const before = requests().length;

      fireEvent.click(buttonNamed(control));
      fireEvent.click(buttonNamed(control));

      await settled();
      if (success !== undefined) {
        await waitFor(() => {
          expect(toast.success).toHaveBeenCalledWith(success);
        });
        expect(toast.success).toHaveBeenCalledTimes(1);
      }
      expect(requests()).toHaveLength(before + 1);
      expect(toast.error).not.toHaveBeenCalled();
    },
  );

  it("keeps Look up focused and inert while the key is read", async () => {
    const { user } = renderAt(KEY_ENTRY_ROUTE);
    await user.type(inputLabelled("Type code"), NEW_CODE);
    const release = holdRequests();

    await user.click(buttonNamed("Look up"));
    await waitFor(() => {
      expect(buttonNamed("Look up").getAttribute("aria-disabled")).toBe("true");
    });
    expectBusy("Look up", []);
    await user.keyboard("{Enter}");
    await user.click(buttonNamed("Look up"));
    expect(sent).toHaveLength(1);

    release();
    await screen.findByRole("button", { name: "Add" });
    expectIdle(["Add", "Cancel"]);
    expect(sent).toHaveLength(1);
  });

  it("keeps Add focused and Cancel inert while the add runs", async () => {
    const { user } = renderAt(KEY_ENTRY_ROUTE);
    await lookUpNewCode(user, NEW_CODE);
    await user.type(inputLabelled("Description"), NEW_DESCRIPTION);
    const release = holdRequests();

    await user.click(buttonNamed("Add"));
    await waitFor(() => {
      expect(buttonNamed("Add").getAttribute("aria-disabled")).toBe("true");
    });
    expectBusy("Add", ["Cancel"]);
    // Cancel leaves the add as it is, and a second Add sends nothing.
    await user.click(buttonNamed("Cancel"));
    await user.click(buttonNamed("Add"));
    expect(inputLabelled("Description").value).toBe(NEW_DESCRIPTION);
    expect(inputLabelled("Type code").value).toBe(NEW_CODE);
    expect(writes()).toHaveLength(1);

    release();
    await waitFor(() => {
      expect(toast.success).toHaveBeenCalledWith(STUB_ADDED);
    });
    await screen.findByRole("button", { name: "Look up" });
    expectIdle(["Look up"]);
    expect(writes()).toHaveLength(1);
  });

  it("keeps Save focused, and Delete and Cancel inert, while the update runs and after it", async () => {
    const { router, user } = renderAt(EDIT_ROUTE_01);
    await waitForPrefill(PURCHASE.description);
    const description = inputLabelled("Description");
    await user.clear(description);
    await user.type(description, CHANGED_DESCRIPTION);
    const release = holdRequests();

    await user.click(buttonNamed("Save"));
    await waitFor(() => {
      expect(buttonNamed("Save").getAttribute("aria-disabled")).toBe("true");
    });
    expectBusy("Save", ["Delete", "Cancel"]);
    // Delete asks nothing, Cancel stays, and Save again sends nothing.
    await user.click(buttonNamed("Delete"));
    await user.click(buttonNamed("Cancel"));
    await user.click(buttonNamed("Save"));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(router.state.location.pathname).toBe(EDIT_ROUTE_01);
    expect(writes()).toHaveLength(1);

    release();
    await waitFor(() => {
      expect(toast.success).toHaveBeenCalledWith(STUB_UPDATED);
    });
    await waitFor(() => {
      expectIdle(["Save", "Delete", "Cancel"]);
    });
    expect(document.activeElement).toBe(buttonNamed("Save"));
    expect(writes().map((request) => request.method)).toEqual(["PUT"]);
  });

  // Enter in a field submits through an aria-disabled Save, so the update
  // itself refuses to run beside a confirmed delete.
  it("sends no PUT for Enter in the description while a confirmed delete runs", async () => {
    const { router, user } = renderAt(EDIT_ROUTE_01);
    await waitForPrefill(PURCHASE.description);
    const release = holdRequests();

    await user.click(buttonNamed("Delete"));
    const confirming = await screen.findByRole("alertdialog", {
      name: "Delete transaction type",
    });
    await user.click(
      within(confirming).getByRole("button", { name: "Confirm" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });
    await waitFor(() => {
      expect(buttonNamed("Delete").getAttribute("aria-disabled")).toBe("true");
    });
    expect(maintenanceForm().getAttribute("aria-busy")).toBe("true");
    expect(document.activeElement).toBe(buttonNamed("Delete"));
    for (const name of ["Save", "Cancel"]) {
      expect(buttonNamed(name).getAttribute("aria-disabled")).toBe("true");
    }

    await user.type(inputLabelled("Description"), " more{Enter}");
    expect(writes().map((request) => request.method)).toEqual(["DELETE"]);

    release();
    await waitFor(() => {
      expect(router.state.location.pathname).toBe(KEY_ENTRY_ROUTE);
    });
    expect(toast.success).toHaveBeenCalledWith(STUB_DELETED);
    expect(writes().map((request) => request.method)).toEqual(["DELETE"]);
  });
});
