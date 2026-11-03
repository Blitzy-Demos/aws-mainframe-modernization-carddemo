// FN tests of the CTTU maintenance page (TranTypeFormPage.tsx; COTRTUPC, map
// COTRTUP), control by control as docs/parity/function-map.md lists them:
//
// - /admin/transaction-types/new: "Look up" reads the typed code (F-COTRTUP-001),
//   a code on file opens its edit route and an unknown one enables "Add", the
//   F5 add and the legend's F6=Add (F-COTRTUP-003, -008, -011); "Cancel" resets
//   to the key entry (F-COTRTUP-015).
// - /admin/transaction-types/:code/edit: "Save" is the PUT without `origin`
//   (F-COTRTUP-002, -009); "Delete" opens the confirmation (F-COTRTUP-006),
//   whose "Cancel" closes it (F-COTRTUP-012) and whose "Confirm" sends the
//   DELETE with `origin=form` (F-COTRTUP-007); "Cancel" leaves for the key
//   entry (F-COTRTUP-014).
// - "Back" links to the list on both routes (F-COTRTUP-005).
//
// Input the schema refuses is never sent, and a refused write's `fields` mark
// their form fields, toast the message and focus the first field. The network
// is a stubbed global answering as the API does over a one-type master (row 1
// of app/data/ASCII/trantype.txt). Every success and failure text is the
// stub's own, and the schema's texts come from the schema module.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent, {
  PointerEventsCheckLevel,
} from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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

const fetchMock = vi.fn(async (input: Request) => serve(input));

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

beforeEach(() => {
  stored = [{ ...PURCHASE }];
  sent = [];
  writeFailure = undefined;
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
function renderAt(path: string) {
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
  return { router, user };
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

/** Waits until the edit route shows the type on file for update. */
async function waitForPrefill(description: string): Promise<void> {
  await waitFor(() => {
    const input = inputLabelled("Description");
    expect(input.value).toBe(description);
    expect(input.disabled).toBe(false);
  });
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
    // The re-read of the deleted type is the delete's outcome, not an error.
    expect(toast.error).not.toHaveBeenCalled();
    expect(writes()).toHaveLength(1);
    const [deleted] = writes();
    expect(deleted.method).toBe("DELETE");
    expect(deleted.url.pathname).toBe(`${LIST_PATH}/${PURCHASE.type_code}`);
    expect(deleted.url.searchParams.get("origin")).toBe("form");
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
