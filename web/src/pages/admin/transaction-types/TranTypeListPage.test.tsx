// FN tests of the CTLI list page (TranTypeListPage.tsx, COTRTLIC map COTRTLI):
// each control the function map names (F-COTRTLI-001, -006, -007, -008,
// -009) sends its request for valid input, sends nothing for input the
// schema module rejects, and shows a server rejection through lib/api-errors
// (toast, field error, focus). The API is stubbed at the global the client
// sends through; every body is typed against the generated OpenAPI schemas.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { components } from "@/api/schema";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import {
  tranTypeInlineEditSchema,
  tranTypeListFilterSchema,
} from "@/schemas/transaction-types";

import TranTypeListPage from "./TranTypeListPage";

type Schemas = components["schemas"];

const ROUTE = "/admin/transaction-types";
const API_PATH = `/api${ROUTE}`;

// app/data/ASCII/trantype.txt rows 1 and 2.
const PURCHASE = {
  type_code: "01",
  description: "Purchase",
} satisfies Schemas["TranTypeResponse"];

const PAYMENT = {
  type_code: "02",
  description: "Payment",
} satisfies Schemas["TranTypeResponse"];

const LIST_PAGE = {
  items: [PURCHASE, PAYMENT],
  next_key: null,
  prev_key: null,
} satisfies Schemas["Page_TranTypeResponse"];

const FILTERED_PAGE = {
  items: [PURCHASE],
  next_key: null,
  prev_key: null,
} satisfies Schemas["Page_TranTypeResponse"];

// Placeholder texts: the page shows whatever the server sends.
const UPDATE_ANSWER = {
  message: "Stub update message",
  info: "Stub update info",
  transaction_type: { type_code: "01", description: "Retail purchase" },
} satisfies Schemas["TranTypeMessageResponse"];

const DELETE_ANSWER = {
  message: "Stub delete message",
  transaction_type: null,
} satisfies Schemas["TranTypeMessageResponse"];

const TRTYPE: Schemas["TranTypeListField"] = "TRTYPE";

const FILTER_REJECTION_TEXT = "Stub filter rejection";

const FILTER_REJECTION = {
  code: "INVALID",
  message: FILTER_REJECTION_TEXT,
  fields: [{ field: TRTYPE, message: FILTER_REJECTION_TEXT }],
} satisfies Schemas["ErrorBody"];

const ROW_REJECTION = {
  code: "INVALID",
  message: "Stub row rejection",
} satisfies Schemas["ErrorBody"];

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

// The server's answers when a test sets no other: the filtered page for a
// type filter, the full page otherwise, and the commit texts of the writes.
function defaultRespond(request: Request): Response {
  const url = new URL(request.url);
  switch (request.method) {
    case "GET":
      return json(
        url.searchParams.has("type_code") ? FILTERED_PAGE : LIST_PAGE,
      );
    case "PUT":
      return json(UPDATE_ANSWER);
    case "DELETE":
      return json(DELETE_ANSWER);
    default:
      return json({ code: "UNEXPECTED", message: "Unexpected error" }, 500);
  }
}

let respond: (request: Request) => Response | Promise<Response> =
  defaultRespond;

// The client hands the global its Request and an init carrying only the
// deadline signal, so the Request alone describes the call.
const fetchMock = vi.fn(async (input: Request) => respond(input));

function requests(method: string): Request[] {
  return fetchMock.mock.calls
    .map(([request]) => request)
    .filter((request) => request.method === method);
}

function renderPage() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const router = createMemoryRouter(
    [
      {
        path: ROUTE,
        element: <TranTypeListPage />,
        handle: { crumb: "Transaction Types", title: "Transaction Types" },
      },
    ],
    { initialEntries: [ROUTE] },
  );
  render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
        <Toaster />
      </QueryClientProvider>
    </ThemeProvider>,
  );
  return { user: userEvent.setup() };
}

/** The page after its first list request has answered. */
async function renderLoadedPage() {
  const view = renderPage();
  await screen.findByText(PURCHASE.description);
  return view;
}

function filterForm(): HTMLElement {
  return screen.getByRole("search", { name: "Transaction type filters" });
}

function typeFilter(): HTMLElement {
  return screen.getByRole("textbox", { name: "Type filter" });
}

function rowButton(name: "Edit" | "Delete", typeCode: string): HTMLElement {
  return screen.getByRole("button", { name, description: typeCode });
}

function rowDescription(): HTMLElement {
  return screen.getByRole("textbox", { name: "Description" });
}

function rowForm(): HTMLElement {
  const form = rowDescription().closest("form");
  if (form === null) {
    throw new Error("the row description is outside a form");
  }
  return form;
}

/** The first issue message the schema gives for `values`. */
function schemaMessage(
  schema: typeof tranTypeListFilterSchema | typeof tranTypeInlineEditSchema,
  values: Record<string, string>,
): string {
  const result = schema.safeParse(values);
  if (result.success) {
    throw new Error(`the schema accepts ${JSON.stringify(values)}`);
  }
  const [issue] = result.error.issues;
  if (issue === undefined) {
    throw new Error("the schema rejected without an issue");
  }
  return issue.message;
}

beforeEach(() => {
  respond = defaultRespond;
  fetchMock.mockClear();
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("TranTypeListPage filters (F-COTRTLI-001)", () => {
  it("lists the first page without filters", async () => {
    await renderLoadedPage();

    const [initial] = requests("GET");
    if (initial === undefined) {
      throw new Error("no list request was sent");
    }
    const url = new URL(initial.url);
    expect(url.pathname).toBe(API_PATH);
    expect([...url.searchParams.keys()]).toEqual([]);
    // One row, with its own Edit, per listed type.
    expect(screen.getAllByRole("button", { name: "Edit" })).toHaveLength(2);
  });

  it("sends a valid type filter on Enter and shows the filtered page", async () => {
    const { user } = await renderLoadedPage();

    await user.type(typeFilter(), "01{Enter}");

    await waitFor(() => {
      const filtered = requests("GET").filter(
        (request) =>
          new URL(request.url).searchParams.get("type_code") === "01",
      );
      expect(filtered).toHaveLength(1);
    });
    const [request] = requests("GET").slice(-1);
    if (request === undefined) {
      throw new Error("no filtered list request was sent");
    }
    const url = new URL(request.url);
    expect(url.pathname).toBe(API_PATH);
    // A blank description filter is left out of the query.
    expect(url.searchParams.has("description")).toBe(false);
    await waitFor(() => {
      expect(screen.queryByText(PAYMENT.description)).toBeNull();
    });
  });

  it("sends no request for a type filter the schema rejects", async () => {
    const { user } = await renderLoadedPage();
    const expected = schemaMessage(tranTypeListFilterSchema, {
      type_code: "0A",
    });

    await user.type(typeFilter(), "0A{Enter}");

    const alert = await within(filterForm()).findByRole("alert");
    expect(alert.textContent).toBe(expected);
    expect(typeFilter().getAttribute("aria-invalid")).toBe("true");
    expect(
      requests("GET").some((request) =>
        new URL(request.url).searchParams.has("type_code"),
      ),
    ).toBe(false);
  });

  it("maps a 400 on TRTYPE to the field, toasts it and focuses the field", async () => {
    const errorSpy = vi.spyOn(toast, "error");
    // The rejection is held back until focus has left the field, so the
    // focus asserted afterwards is the one the error placed.
    let answer = (): void => undefined;
    const answered = new Promise<void>((resolve) => {
      answer = resolve;
    });
    respond = async (request) => {
      if (
        request.method === "GET" &&
        new URL(request.url).searchParams.has("type_code")
      ) {
        await answered;
        return json(FILTER_REJECTION, 400);
      }
      return defaultRespond(request);
    };
    const { user } = await renderLoadedPage();

    await user.type(typeFilter(), "01");
    const search = screen.getByRole("button", { name: "Search" });
    await user.click(search);
    expect(document.activeElement).toBe(search);
    answer();

    const alert = await within(filterForm()).findByRole("alert");
    expect(alert.textContent).toBe(FILTER_REJECTION_TEXT);
    expect(errorSpy).toHaveBeenCalledWith(FILTER_REJECTION_TEXT);
    await waitFor(() => {
      expect(document.activeElement).toBe(typeFilter());
    });
  });
});

describe("TranTypeListPage inline update (F-COTRTLI-006, F-COTRTLI-007)", () => {
  it("sends the new description with origin=list and toasts the answer", async () => {
    const successSpy = vi.spyOn(toast, "success");
    const { user } = await renderLoadedPage();

    await user.click(rowButton("Edit", PURCHASE.type_code));
    const description = rowDescription();
    expect(document.activeElement).toBe(description);
    await user.clear(description);
    await user.type(description, "Retail purchase");
    await user.click(within(rowForm()).getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(requests("PUT")).toHaveLength(1);
    });
    const [request] = requests("PUT");
    if (request === undefined) {
      throw new Error("no update request was sent");
    }
    const url = new URL(request.url);
    expect(url.pathname).toBe(`${API_PATH}/${PURCHASE.type_code}`);
    expect(url.searchParams.get("origin")).toBe("list");
    expect(await request.clone().json()).toEqual({
      description: "Retail purchase",
    });
    await waitFor(() => {
      expect(successSpy).toHaveBeenCalledWith(UPDATE_ANSWER.message);
    });
    await waitFor(() => {
      expect(screen.queryByRole("textbox", { name: "Description" })).toBeNull();
    });
  });

  it("sends no request for a blank description and shows the field error", async () => {
    const { user } = await renderLoadedPage();
    const expected = schemaMessage(tranTypeInlineEditSchema, {
      description: "",
    });

    await user.click(rowButton("Edit", PURCHASE.type_code));
    await user.clear(rowDescription());
    await user.click(within(rowForm()).getByRole("button", { name: "Save" }));

    const alert = await within(rowForm()).findByRole("alert");
    expect(alert.textContent).toBe(expected);
    expect(requests("PUT")).toHaveLength(0);
  });

  it("toasts a rejected update and keeps the row open", async () => {
    const errorSpy = vi.spyOn(toast, "error");
    respond = (request) =>
      request.method === "PUT"
        ? json(ROW_REJECTION, 400)
        : defaultRespond(request);
    const { user } = await renderLoadedPage();

    await user.click(rowButton("Edit", PURCHASE.type_code));
    await user.click(within(rowForm()).getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(errorSpy).toHaveBeenCalledWith(ROW_REJECTION.message);
    });
    expect(requests("PUT")).toHaveLength(1);
    // The inline update names no BMS field, so only the toast carries the text.
    expect(within(rowForm()).queryByRole("alert")).toBeNull();
  });
});

describe("TranTypeListPage delete (F-COTRTLI-008, F-COTRTLI-009)", () => {
  it("deletes the row once confirmed and toasts the answer", async () => {
    const successSpy = vi.spyOn(toast, "success");
    const { user } = await renderLoadedPage();

    await user.click(rowButton("Delete", PAYMENT.type_code));
    const dialog = await screen.findByRole("alertdialog", {
      name: "Delete transaction type",
    });
    expect(requests("DELETE")).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(requests("DELETE")).toHaveLength(1);
    });
    const [request] = requests("DELETE");
    if (request === undefined) {
      throw new Error("no delete request was sent");
    }
    const url = new URL(request.url);
    expect(url.pathname).toBe(`${API_PATH}/${PAYMENT.type_code}`);
    // Without `origin` the server applies the COTRTLIC row delete.
    expect(url.searchParams.has("origin")).toBe(false);
    await waitFor(() => {
      expect(successSpy).toHaveBeenCalledWith(DELETE_ANSWER.message);
    });
  });
});
