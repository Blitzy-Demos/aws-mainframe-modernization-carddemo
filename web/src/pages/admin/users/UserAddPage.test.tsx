// FN tests of UserAddPage, the CU01 "Add User" screen (COUSR01C, map COUSR01) at
// /admin/users/new. They drive the controls of the F-COUSR01 rows of
// docs/parity/function-map.md and check the texts of the V-COUSR01C rows of
// docs/parity/validation-audit.md:
// - valid input goes out as one POST /admin/users whose body holds every value
//   exactly as typed, the mixed-case password included (E-20), and the 201
//   answer shows its COUSR01C text and leaves a cleared screen, as
//   WRITE-USER-SEC-FILE NORMAL performs INITIALIZE-ALL-FIELDS;
// - an empty field never reaches the network: its audited text appears on the
//   field, which is marked invalid;
// - a server error is toasted, set on the field its `fields[]` names and puts
//   the cursor there, as `MOVE -1 TO <field>L` does;
// - Reset (PF4) clears the screen without a request.
//
// The network is the stubbed global the API client looks up on every request.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryRouter, RouterProvider } from "react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { components } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";

import UserAddPage from "./UserAddPage";

type UserAddRequest = components["schemas"]["UserAddRequest"];
type UserAddField = components["schemas"]["UserAddField"];
type UserMessageResponse = components["schemas"]["UserMessageResponse"];
type ErrorBody = components["schemas"]["ErrorBody"];

type FieldName = keyof UserAddRequest;

// What to type into each COUSR1A input; the empty string leaves it untouched.
type UserAddEntry = { [K in FieldName]-?: string };

// The COUSR1A inputs in screen order, with their BMS captions.
const FIELDS: readonly { name: FieldName; label: string }[] = [
  { name: "first_name", label: "First Name" },
  { name: "last_name", label: "Last Name" },
  { name: "user_id", label: "User ID" },
  { name: "password", label: "Password" },
  { name: "usr_type", label: "User Type" },
];

// A user the fixtures do not hold. The password is mixed case on purpose: the
// admin programs store it as entered, and only sign-on upper-cases (E-20).
const NEW_USER = {
  first_name: "NEWTON",
  last_name: "ISAAC",
  user_id: "USER0009",
  password: "Pass1234",
  usr_type: "U",
} satisfies UserAddEntry & UserAddRequest;

// COUSR01C WRITE-USER-SEC-FILE NORMAL: 'User ' SEC-USR-ID ' has been added ...'.
const ADDED_MESSAGE = "User USER0009 has been added ...";

const ADDED_REPLY = {
  user: {
    first_name: NEW_USER.first_name,
    last_name: NEW_USER.last_name,
    user_id: NEW_USER.user_id,
    usr_type: NEW_USER.usr_type,
  },
  message: ADDED_MESSAGE,
} satisfies UserMessageResponse;

const ADD_USER_HANDLE = {
  crumb: "Add user",
  title: "Add User",
} satisfies RouteHandle;

const fetchMock = vi.fn<(request: Request) => Promise<Response>>();

let queryClient: QueryClient;

function answerWith(body: UserMessageResponse | ErrorBody, status: number): void {
  fetchMock.mockImplementation(async () => Response.json(body, { status }));
}

function renderPage(): UserEvent {
  queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const router = createMemoryRouter(
    [
      {
        path: "/admin/users/new",
        element: <UserAddPage />,
        handle: ADD_USER_HANDLE,
      },
      { path: "/admin", element: <p>admin</p> },
    ],
    { initialEntries: ["/admin/users/new"] },
  );
  render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
        <Toaster />
      </QueryClientProvider>
    </ThemeProvider>,
  );
  return userEvent.setup();
}

// The password input has no ARIA role, so it is found by its label alone.
function inputOf(name: FieldName): HTMLInputElement {
  const field = FIELDS.find((entry) => entry.name === name);
  if (field === undefined) {
    throw new Error(`No COUSR1A input ${name}`);
  }
  return name === "password"
    ? screen.getByLabelText<HTMLInputElement>(field.label)
    : screen.getByRole<HTMLInputElement>("textbox", { name: field.label });
}

// Types every non-empty entry into its input, in screen order.
async function fill(user: UserEvent, entry: UserAddEntry): Promise<void> {
  for (const { name } of FIELDS) {
    const value = entry[name];
    if (value !== "") {
      await user.type(inputOf(name), value);
    }
  }
}

// The FieldError announced for `input`: the alert its aria-describedby names,
// so the text is tied to the input for assistive technology, not merely nearby.
function fieldErrorOf(input: HTMLElement): HTMLElement | null {
  const ids = (input.getAttribute("aria-describedby") ?? "")
    .split(" ")
    .filter((id) => id !== "");
  for (const id of ids) {
    const element = document.getElementById(id);
    if (element?.getAttribute("role") === "alert") {
      return element;
    }
  }
  return null;
}

function notifications(): HTMLElement {
  return screen.getByRole("region", { name: /^Notifications/ });
}

function sentRequests(): Request[] {
  return fetchMock.mock.calls.map(([request]) => request);
}

function addUserButton(): HTMLElement {
  return screen.getByRole("button", { name: "Add user" });
}

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock);
  vi.spyOn(toast, "success");
  vi.spyOn(toast, "error");
});

afterEach(() => {
  toast.dismiss();
  queryClient.clear();
  fetchMock.mockReset();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("UserAddPage (CU01, COUSR01)", () => {
  it("opens on the empty Add User screen with the cursor on First Name", async () => {
    renderPage();

    expect(screen.getByRole("heading", { level: 1, name: "Add User" })).toBeTruthy();
    for (const { name } of FIELDS) {
      expect(inputOf(name).value).toBe("");
      expect(inputOf(name).getAttribute("aria-invalid")).toBe("false");
    }
    // F-COUSR01-001 to F-COUSR01-003: ENTER, PF4 and PF3.
    expect(addUserButton().getAttribute("type")).toBe("submit");
    expect(screen.getByRole("button", { name: "Reset" }).getAttribute("type")).toBe(
      "button",
    );
    expect(screen.getByRole("link", { name: "Back" }).getAttribute("href")).toBe(
      "/admin/users",
    );
    await waitFor(() => {
      expect(document.activeElement).toBe(inputOf("first_name"));
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  const SUBMITS: readonly {
    how: string;
    submit: (user: UserEvent) => Promise<void>;
  }[] = [
    {
      how: "the Enter key in a focused field",
      submit: async (user) => {
        await user.click(inputOf("last_name"));
        await user.keyboard("{Enter}");
      },
    },
    {
      how: "the Add user button",
      submit: async (user) => {
        await user.click(addUserButton());
      },
    },
  ];

  it.each(SUBMITS)(
    "sends the values exactly as typed and clears the screen on $how",
    async ({ submit }) => {
      answerWith(ADDED_REPLY, 201);
      const user = renderPage();

      await fill(user, NEW_USER);
      await submit(user);

      await waitFor(() => {
        expect(toast.success).toHaveBeenCalledWith(ADDED_MESSAGE);
      });
      await within(notifications()).findByText(ADDED_MESSAGE);

      const requests = sentRequests();
      expect(requests).toHaveLength(1);
      const [request] = requests;
      expect(request?.method).toBe("POST");
      expect(new URL(request?.url ?? "").pathname).toBe("/api/admin/users");
      // The password keeps its case: never upper-cased, trimmed or padded (E-20).
      expect(await request?.clone().json()).toEqual(NEW_USER satisfies UserAddRequest);

      // INITIALIZE-ALL-FIELDS: every input SPACES, the cursor on FNAME.
      await waitFor(() => {
        for (const { name } of FIELDS) {
          expect(inputOf(name).value).toBe("");
        }
        expect(document.activeElement).toBe(inputOf("first_name"));
      });
      expect(toast.error).not.toHaveBeenCalled();
    },
  );

  // PROCESS-ENTER-KEY's empty-field edits, one EVALUATE in screen order.
  const EMPTY_FIELDS: readonly {
    row: string;
    name: FieldName;
    value: string;
    message: string;
  }[] = [
    {
      row: "V-COUSR01C-001",
      name: "first_name",
      value: "",
      message: "First Name can NOT be empty...",
    },
    {
      row: "V-COUSR01C-001 (spaces)",
      name: "first_name",
      value: "   ",
      message: "First Name can NOT be empty...",
    },
    {
      row: "V-COUSR01C-002",
      name: "last_name",
      value: "",
      message: "Last Name can NOT be empty...",
    },
    {
      row: "V-COUSR01C-003",
      name: "user_id",
      value: "",
      message: "User ID can NOT be empty...",
    },
    {
      row: "V-COUSR01C-004",
      name: "password",
      value: "",
      message: "Password can NOT be empty...",
    },
    {
      row: "V-COUSR01C-005",
      name: "usr_type",
      value: "",
      message: "User Type can NOT be empty...",
    },
  ];

  it.each(EMPTY_FIELDS)(
    "keeps a blank $name off the network and shows its text ($row)",
    async ({ name, value, message }) => {
      const user = renderPage();

      await fill(user, { ...NEW_USER, [name]: value });
      await user.click(addUserButton());

      const input = inputOf(name);
      await waitFor(() => {
        expect(input.getAttribute("aria-invalid")).toBe("true");
        expect(document.activeElement).toBe(input);
      });
      expect(fieldErrorOf(input)?.textContent).toBe(message);
      for (const other of FIELDS.filter((field) => field.name !== name)) {
        expect(inputOf(other.name).getAttribute("aria-invalid")).toBe("false");
      }
      expect(fetchMock).not.toHaveBeenCalled();
      expect(toast.success).not.toHaveBeenCalled();
    },
  );

  // The record-state and I/O outcomes only the server knows, and the API's own
  // empty-field edit, which also answers a LOW-VALUES first name.
  const SERVER_ERRORS: readonly {
    row: string;
    status: number;
    code: string;
    field: UserAddField;
    name: FieldName;
    message: string;
  }[] = [
    {
      row: "V-COUSR01C-006",
      status: 409,
      code: "DUPLICATE",
      field: "USERID",
      name: "user_id",
      message: "User ID already exist...",
    },
    {
      row: "V-COUSR01C-001",
      status: 400,
      code: "EMPTY",
      field: "FNAME",
      name: "first_name",
      message: "First Name can NOT be empty...",
    },
    {
      row: "V-COUSR01C-007",
      status: 500,
      code: "UNEXPECTED",
      field: "FNAME",
      name: "first_name",
      message: "Unable to Add User...",
    },
  ];

  it.each(SERVER_ERRORS)(
    "toasts a $status $field answer, marks $name and moves the cursor there ($row)",
    async ({ status, code, field, name, message }) => {
      answerWith(
        { code, message, fields: [{ field, message }] } satisfies ErrorBody,
        status,
      );
      const user = renderPage();

      await fill(user, NEW_USER);
      await user.click(addUserButton());

      await waitFor(() => {
        expect(toast.error).toHaveBeenCalledWith(message);
      });
      await within(notifications()).findByText(message);

      const input = inputOf(name);
      await waitFor(() => {
        expect(input.getAttribute("aria-invalid")).toBe("true");
        expect(document.activeElement).toBe(input);
      });
      expect(fieldErrorOf(input)?.textContent).toBe(message);
      expect(sentRequests()).toHaveLength(1);
      // SEND-USRADD-SCREEN on an error keeps what was entered.
      for (const { name: kept } of FIELDS) {
        expect(inputOf(kept).value).toBe(NEW_USER[kept]);
      }
      expect(toast.success).not.toHaveBeenCalled();
    },
  );

  it("clears values and errors on Reset without a request (F-COUSR01-003)", async () => {
    const user = renderPage();

    await fill(user, { ...NEW_USER, first_name: "" });
    await user.click(addUserButton());
    await waitFor(() => {
      expect(inputOf("first_name").getAttribute("aria-invalid")).toBe("true");
    });
    await user.type(inputOf("first_name"), NEW_USER.first_name);

    await user.click(screen.getByRole("button", { name: "Reset" }));

    await waitFor(() => {
      for (const { name } of FIELDS) {
        expect(inputOf(name).value).toBe("");
        expect(inputOf(name).getAttribute("aria-invalid")).toBe("false");
      }
      expect(document.activeElement).toBe(inputOf("first_name"));
    });
    expect(fieldErrorOf(inputOf("first_name"))).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
