// FN tests of CU02, the COUSR02C "Update User" screen (UserEditPage): valid
// input reaches PUT /admin/users/{id} as the request body the API defines,
// input the schema rejects sends nothing, Save marking its field and Save and
// exit returning to the user list as PF3 does whatever the outcome, and a
// server rejection marks the field it names, toasts its message and takes the
// focus.
//
// E-21 runs through every case: GET returns no password, so the password field
// starts empty and disabled, the body leaves `password` out unless "Change
// password" is checked, and a checked password is required. A checked password
// goes out as typed, case kept (E-20).
//
// One save sends one PUT, a double click included, and while it runs the form
// is busy and its four buttons inert, Save keeping the focus after it too.
//
// The network is the stubbed global the API client looks up on every request;
// it answers GET from the DUSRSECJ seed row of USER0001 and PUT with each
// test's answer.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import {
  createMemoryRouter,
  RouterProvider,
  type RouteObject,
} from "react-router";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { userKeys } from "@/api/queries/admin-users";
import type { components } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";

import UserEditPage from "./UserEditPage";

type Schemas = components["schemas"];
type UserSummary = Schemas["UserSummary"];
type UserUpdateRequest = Schemas["UserUpdateRequest"];
type UserUpdateField = Schemas["UserUpdateField"];
type UserMessageResponse = Schemas["UserMessageResponse"];
type ErrorBody = Schemas["ErrorBody"];

// The audited COUSR02C texts [app/cbl/COUSR02C.cbl:L186-243].
const FIRST_NAME_EMPTY = "First Name can NOT be empty..."; // V-COUSR02C-005
const PASSWORD_EMPTY = "Password can NOT be empty..."; // V-COUSR02C-007
const PLEASE_MODIFY = "Please modify to update ..."; // V-COUSR02C-010
const UPDATED = "User USER0001 has been updated ..."; // P-COUSR02C-006

// USER0001 as DUSRSECJ loads it [app/jcl/DUSRSECJ.jcl:L40], without the
// password the API never returns.
const USER0001 = {
  user_id: "USER0001",
  first_name: "LAWRENCE",
  last_name: "THOMAS",
  usr_type: "U",
} as const satisfies UserSummary;

const EDIT_PATH = "/admin/users/USER0001/edit";
const USER_LIST_PATH = "/admin/users";
const API_USER_PATH = "/api/admin/users/USER0001";

// The routes of the user `..`, at its key segment `~..`.
const DOTS_EDIT_PATH = "/admin/users/~../edit";
const DOTS_API_USER_PATH = "/api/admin/users/~..";

// The route table of the admin domain (AAP 0.5.4.1), nested so PageHeader
// builds its trail from the handles. `users/edit` is the edit page reached
// without a user ID.
const routes: RouteObject[] = [
  {
    path: "/admin",
    handle: { crumb: "Administration" } satisfies RouteHandle,
    children: [
      { index: true, element: <p>Administration home</p> },
      {
        path: "users",
        handle: { crumb: "Users" } satisfies RouteHandle,
        children: [
          { index: true, element: <p>User list</p> },
          { path: "edit", element: <UserEditPage /> },
          {
            path: ":userId/edit",
            element: <UserEditPage />,
            handle: { crumb: "Update User" } satisfies RouteHandle,
          },
        ],
      },
    ],
  },
];

type Sent = { method: string; pathname: string; body: unknown };

let sent: Sent[];
let stored: UserSummary;
// The path the stub answers GET and PUT on: USER0001's unless a test opens
// another user.
let apiUserPath: string;
let answerPut: (body: unknown) => Response | Promise<Response>;
let queryClient: QueryClient;

function puts(): Sent[] {
  return sent.filter(({ method }) => method === "PUT");
}

// The JSON object a request carried, so its keys can be checked one by one.
function objectBody({ body }: Sent): Record<string, unknown> {
  if (typeof body !== "object" || body === null || Array.isArray(body)) {
    throw new Error(`Expected a JSON object body, got ${JSON.stringify(body)}`);
  }
  return Object.fromEntries(Object.entries(body));
}

// The write's 200, P-COUSR02C-006. The user is stored too, so the read the
// mutation's invalidation triggers returns what was written.
function updated(user: UserSummary): Response {
  stored = user;
  return Response.json({
    user,
    message: UPDATED,
  } satisfies UserMessageResponse);
}

function rejected(
  status: number,
  code: string,
  message: string,
  field: UserUpdateField,
): Response {
  return Response.json(
    { code, message, fields: [{ field, message }] } satisfies ErrorBody,
    { status },
  );
}

beforeEach(() => {
  sent = [];
  stored = USER0001;
  apiUserPath = API_USER_PATH;
  answerPut = () => {
    throw new Error("Unexpected PUT");
  };
  queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const network = vi.fn(async (request: Request): Promise<Response> => {
    const { pathname } = new URL(request.url);
    const body: unknown =
      request.method === "PUT" ? await request.clone().json() : undefined;
    sent.push({ method: request.method, pathname, body });
    if (pathname === apiUserPath && request.method === "GET") {
      return Response.json(stored);
    }
    if (pathname === apiUserPath && request.method === "PUT") {
      return answerPut(body);
    }
    return Response.json(
      { code: "NOT_FOUND", message: `No stub for ${request.method} ${pathname}` },
      { status: 404 },
    );
  });
  vi.stubGlobal("fetch", network);
});

afterEach(() => {
  queryClient.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function renderAt(path: string) {
  const router = createMemoryRouter(routes, { initialEntries: [path] });
  render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
      <Toaster />
    </ThemeProvider>,
  );
  return { router, user: userEvent.setup() };
}

function asInput(element: HTMLElement): HTMLInputElement {
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`Expected an input, got <${element.tagName}>`);
  }
  return element;
}

function textbox(name: string): HTMLInputElement {
  return asInput(screen.getByRole("textbox", { name }));
}

// A password input has no ARIA role, so it is found by its label. The exact
// match leaves out the "Change password" checkbox.
function passwordInput(): HTMLInputElement {
  return asInput(screen.getByLabelText("Password", { selector: "input" }));
}

function changePassword(): HTMLElement {
  return screen.getByRole("checkbox", { name: "Change password" });
}

// The FieldError of an input: the role="alert" element its
// aria-describedby names.
function fieldErrorOf(input: HTMLElement): string | null {
  const ids = (input.getAttribute("aria-describedby") ?? "")
    .split(" ")
    .filter((id) => id !== "");
  const alert = ids
    .map((id) => document.getElementById(id))
    .find((element) => element?.getAttribute("role") === "alert");
  return alert?.textContent ?? null;
}

// The FieldDescription of an input: the field-description element its
// aria-describedby names.
function fieldDescriptionOf(input: HTMLElement): string | null {
  const ids = (input.getAttribute("aria-describedby") ?? "")
    .split(" ")
    .filter((id) => id !== "");
  const description = ids
    .map((id) => document.getElementById(id))
    .find(
      (element) => element?.getAttribute("data-slot") === "field-description",
    );
  return description?.textContent ?? null;
}

// The Sonner toast showing `message`, once it has appeared.
async function toastShowing(message: string): Promise<HTMLElement> {
  return waitFor(() => {
    const shown = Array.from(
      document.querySelectorAll<HTMLElement>("[data-sonner-toast]"),
    ).find((element) => element.textContent?.includes(message) === true);
    if (shown === undefined) {
      throw new Error(`No toast shows "${message}"`);
    }
    return shown;
  });
}

// The form once GET /admin/users/USER0001 has filled it (P-COUSR02C-002).
async function loadedForm(): Promise<void> {
  const firstName = asInput(
    await screen.findByRole("textbox", { name: "First Name" }),
  );
  await waitFor(() => {
    expect(firstName.value).toBe(USER0001.first_name);
  });
}

async function replaceText(
  user: UserEvent,
  input: HTMLInputElement,
  text: string,
): Promise<void> {
  await user.clear(input);
  await user.paste(text);
}

function button(name: string): HTMLElement {
  return screen.getByRole("button", { name });
}

// The buttons beside Save: PF3, PF4 and PF12.
const SAVE_SIBLINGS = ["Save and exit", "Reset", "Cancel"] as const;

// The form Save submits; it has no accessible name.
function editForm(): HTMLFormElement {
  const form = button("Save").closest("form");
  if (form === null) {
    throw new Error("Save is outside a form");
  }
  return form;
}

describe("UserEditPage (CU02, COUSR02C)", () => {
  it.each([
    {
      variant: "unchecked switch, Save",
      password: undefined,
      expected: {
        first_name: "LARRY",
        last_name: "THOMAS",
        usr_type: "U",
      } satisfies UserUpdateRequest,
    },
    {
      variant: "checked switch, Enter in the password field",
      password: "NewPass1",
      expected: {
        first_name: "LARRY",
        last_name: "THOMAS",
        password: "NewPass1",
        usr_type: "U",
      } satisfies UserUpdateRequest,
    },
  ])(
    "shows USER0001 without its password and sends the PUT the switch decides ($variant)",
    async ({ password, expected }) => {
      const { user, router } = renderAt(EDIT_PATH);
      await loadedForm();

      expect(
        screen.getByRole("heading", { level: 1, name: "Update User" }),
      ).toBeTruthy();
      expect(textbox("User ID").value).toBe("USER0001");
      expect(textbox("User ID").readOnly).toBe(true);
      // jsdom computes no Tailwind, so the read-only cue is held to its class
      // tokens: bg-muted in both themes, the Input's dark:bg-input/30 replaced.
      const userIdClasses = Array.from(textbox("User ID").classList);
      expect(userIdClasses).toEqual(
        expect.arrayContaining(["bg-muted", "dark:bg-muted"]),
      );
      expect(userIdClasses).not.toContain("dark:bg-input/30");
      expect(textbox("Last Name").value).toBe("THOMAS");
      expect(textbox("User Type").value).toBe("U");
      // The COUSR02 hint beside USRTYPE [app/bms/COUSR02.bms:L150-154].
      expect(fieldDescriptionOf(textbox("User Type"))).toBe(
        "(A=Admin, U=User)",
      );
      // E-21: the current password is never shown.
      expect(passwordInput().value).toBe("");
      expect(passwordInput().disabled).toBe(true);
      expect(changePassword().getAttribute("aria-checked")).toBe("false");

      answerPut = () => updated({ ...USER0001, first_name: "LARRY" });
      await replaceText(user, textbox("First Name"), "LARRY");
      if (password === undefined) {
        await user.click(screen.getByRole("button", { name: "Save" }));
      } else {
        await user.click(changePassword());
        expect(passwordInput().disabled).toBe(false);
        await user.type(passwordInput(), `${password}{Enter}`);
      }

      await waitFor(() => {
        expect(puts()).toHaveLength(1);
      });
      const [put] = puts();
      expect(put.pathname).toBe(API_USER_PATH);
      const body = objectBody(put);
      expect(body).toStrictEqual(expected);
      expect(Object.hasOwn(body, "password")).toBe(password !== undefined);

      const success = await toastShowing(UPDATED);
      expect(success.getAttribute("data-type")).toBe("success");
      // The form holds the saved user, the password emptied behind the switch.
      await waitFor(() => {
        expect(textbox("First Name").value).toBe("LARRY");
        expect(passwordInput().value).toBe("");
        expect(passwordInput().disabled).toBe(true);
      });
      expect(router.state.location.pathname).toBe(EDIT_PATH);
    },
  );

  it("saves with Save and exit, then returns to the user list (PF3)", async () => {
    const { user, router } = renderAt(EDIT_PATH);
    await loadedForm();
    answerPut = () => updated({ ...USER0001, last_name: "THOMSON" });

    await replaceText(user, textbox("Last Name"), "THOMSON");
    await user.click(screen.getByRole("button", { name: "Save and exit" }));

    await waitFor(() => {
      expect(router.state.location.pathname).toBe(USER_LIST_PATH);
    });
    expect(puts()).toHaveLength(1);
    expect(objectBody(puts()[0])).toStrictEqual({
      first_name: "LAWRENCE",
      last_name: "THOMSON",
      usr_type: "U",
    } satisfies UserUpdateRequest);
    expect((await toastShowing(UPDATED)).getAttribute("data-type")).toBe(
      "success",
    );
  });

  // PF3 returns to the list whatever the update outcome (F-COUSR02-002,
  // P-COUSR02C-008); the input the schema rejects is never sent.
  it.each([
    {
      variant: "Last Name cleared",
      enter: async (user: UserEvent) => {
        await user.clear(textbox("Last Name"));
      },
    },
    {
      variant: "Reset pressed",
      enter: async (user: UserEvent) => {
        await user.click(screen.getByRole("button", { name: "Reset" }));
      },
    },
  ])(
    "returns to the user list without a PUT when Save and exit has input the schema rejects ($variant)",
    async ({ enter }) => {
      const { user, router } = renderAt(EDIT_PATH);
      await loadedForm();

      await enter(user);
      expect(textbox("Last Name").value).toBe("");
      await user.click(screen.getByRole("button", { name: "Save and exit" }));

      await waitFor(() => {
        expect(router.state.location.pathname).toBe(USER_LIST_PATH);
      });
      expect(await screen.findByText("User list")).toBeTruthy();
      expect(puts()).toHaveLength(0);
    },
  );

  it.each([
    {
      variant: "Change password checked, password left empty",
      enter: async (user: UserEvent) => {
        await user.click(changePassword());
      },
      field: passwordInput,
      message: PASSWORD_EMPTY,
    },
    {
      variant: "first name cleared",
      enter: async (user: UserEvent) => {
        await user.clear(textbox("First Name"));
      },
      field: () => textbox("First Name"),
      message: FIRST_NAME_EMPTY,
    },
  ])(
    "sends nothing and marks the field when the schema rejects the input ($variant)",
    async ({ enter, field, message }) => {
      const { user } = renderAt(EDIT_PATH);
      await loadedForm();

      await enter(user);
      await user.click(screen.getByRole("button", { name: "Save" }));

      await waitFor(() => {
        expect(field().getAttribute("aria-invalid")).toBe("true");
      });
      expect(fieldErrorOf(field())).toBe(message);
      expect(document.activeElement).toBe(field());
      expect(puts()).toHaveLength(0);
    },
  );

  // The page has no USRIDIN key field to submit: the user ID comes only from
  // the route, which the user list's row actions supply (E-61, E-56), so the
  // edit page reached without one returns to that list without a request.
  it("returns to the user list without a request when the route has no user ID", async () => {
    const { router } = renderAt("/admin/users/edit");

    expect(await screen.findByText("User list")).toBeTruthy();
    expect(router.state.location.pathname).toBe(USER_LIST_PATH);
    expect(sent).toHaveLength(0);
  });

  // The user ID `..`, which COUSR01C stores and a URL drops as a dot segment,
  // has its route at the key segment `~..` (lib/key-segment.ts): the page
  // reads and saves the stored user `..` through that segment.
  it("reads and saves the user '..' through its key segment ~..", async () => {
    const dots = {
      user_id: "..",
      first_name: "DOTS",
      last_name: "KEY",
      usr_type: "U",
    } as const satisfies UserSummary;
    const saved = { ...dots, first_name: "DOTTED" } satisfies UserSummary;
    const savedMessage = "User .. has been updated ...";
    stored = dots;
    apiUserPath = DOTS_API_USER_PATH;
    answerPut = () => {
      stored = saved;
      return Response.json({
        user: saved,
        message: savedMessage,
      } satisfies UserMessageResponse);
    };
    const { user, router } = renderAt(DOTS_EDIT_PATH);

    const firstName = asInput(
      await screen.findByRole("textbox", { name: "First Name" }),
    );
    await waitFor(() => {
      expect(firstName.value).toBe(dots.first_name);
    });
    expect(textbox("User ID").value).toBe(dots.user_id);
    // The page keys its read by the stored ID, not by the segment.
    expect(queryClient.getQueryData(userKeys.detail(dots.user_id))).toEqual(
      dots,
    );

    await replaceText(user, textbox("First Name"), saved.first_name);
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(puts()).toHaveLength(1);
    });
    const [put] = puts();
    expect(put.pathname).toBe(DOTS_API_USER_PATH);
    expect(objectBody(put)).toStrictEqual({
      first_name: saved.first_name,
      last_name: saved.last_name,
      usr_type: saved.usr_type,
    } satisfies UserUpdateRequest);
    await toastShowing(savedMessage);
    // Every request, the read and the save, addressed the user `..`.
    expect(sent.some(({ method }) => method === "GET")).toBe(true);
    expect(sent.map(({ pathname }) => pathname)).toEqual(
      sent.map(() => DOTS_API_USER_PATH),
    );
    expect(router.state.location.pathname).toBe(DOTS_EDIT_PATH);
  });

  it("marks, toasts and focuses the field a server rejection names", async () => {
    const { user } = renderAt(EDIT_PATH);
    await loadedForm();
    // LOW-VALUES: a first name of NUL characters passes the schema, and the
    // server answers it as the empty field (V-COUSR02C-005).
    answerPut = () => rejected(400, "EMPTY", FIRST_NAME_EMPTY, "FNAME");

    await replaceText(user, textbox("First Name"), "\u0000\u0000");
    await user.click(screen.getByRole("button", { name: "Save" }));

    const error = await toastShowing(FIRST_NAME_EMPTY);
    expect(error.getAttribute("data-type")).toBe("error");
    expect(objectBody(puts()[0])).toStrictEqual({
      first_name: "\u0000\u0000",
      last_name: "THOMAS",
      usr_type: "U",
    } satisfies UserUpdateRequest);
    await waitFor(() => {
      expect(document.activeElement).toBe(textbox("First Name"));
    });
    expect(textbox("First Name").getAttribute("aria-invalid")).toBe("true");
    expect(fieldErrorOf(textbox("First Name"))).toBe(FIRST_NAME_EMPTY);
    expect(puts()).toHaveLength(1);
  });

  it("shows the unchanged-save answer as a prompt, not a success", async () => {
    const { user } = renderAt(EDIT_PATH);
    await loadedForm();
    answerPut = () =>
      Response.json({
        user: stored,
        message: PLEASE_MODIFY,
      } satisfies UserMessageResponse);

    await user.click(screen.getByRole("button", { name: "Save" }));

    const prompt = await toastShowing(PLEASE_MODIFY);
    expect(prompt.getAttribute("data-type")).not.toBe("success");
    expect(prompt.getAttribute("data-type")).not.toBe("error");
    expect(objectBody(puts()[0])).toStrictEqual({
      first_name: "LAWRENCE",
      last_name: "THOMAS",
      usr_type: "U",
    } satisfies UserUpdateRequest);
    expect(textbox("First Name").getAttribute("aria-invalid")).toBe("false");
  });

  // Two clicks before the page re-renders: the second PUT of the same values
  // would find them stored and answer V-COUSR02C-010, which, as the last
  // write, would be shown instead of the update that succeeded.
  it("sends one PUT for a double click on Save and shows only its success", async () => {
    const { user } = renderAt(EDIT_PATH);
    await loadedForm();
    answerPut = () => {
      answerPut = () =>
        Response.json({
          user: stored,
          message: PLEASE_MODIFY,
        } satisfies UserMessageResponse);
      return updated({ ...USER0001, first_name: "LARRY" });
    };
    await replaceText(user, textbox("First Name"), "LARRY");
    const toastSuccess = vi.spyOn(toast, "success");
    const toastError = vi.spyOn(toast, "error");

    fireEvent.click(button("Save"));
    fireEvent.click(button("Save"));

    await waitFor(() => {
      expect(toastSuccess).toHaveBeenCalledWith(UPDATED);
    });
    await waitFor(() => {
      expect(button("Save").getAttribute("aria-disabled")).toBe("false");
    });
    // The one PUT took the first answer, so the unchanged-save prompt the
    // stub keeps for a second one was never sent back.
    expect(puts()).toHaveLength(1);
    expect(toastSuccess).toHaveBeenCalledTimes(1);
    expect(toastError).not.toHaveBeenCalled();
    expect(textbox("First Name").value).toBe("LARRY");
  });

  it("keeps Save focused, the form busy and every button inert while the save runs and after it", async () => {
    let release = (): void => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const { user, router } = renderAt(EDIT_PATH);
    await loadedForm();
    // The PUT is answered only once the test releases it.
    answerPut = async () => {
      await held;
      return updated({ ...USER0001, first_name: "LARRY" });
    };
    await replaceText(user, textbox("First Name"), "LARRY");

    await user.click(button("Save"));
    await waitFor(() => {
      expect(button("Save").getAttribute("aria-disabled")).toBe("true");
    });
    // Inert but not disabled: a disabled button would hand focus to the body.
    expect(button("Save").hasAttribute("disabled")).toBe(false);
    expect(document.activeElement).toBe(button("Save"));
    expect(editForm().getAttribute("aria-busy")).toBe("true");
    for (const name of SAVE_SIBLINGS) {
      expect(button(name).getAttribute("aria-disabled")).toBe("true");
      expect(button(name).hasAttribute("disabled")).toBe(false);
    }

    // Enter on the focused Save, each other button and Save again do nothing:
    // no second PUT, no exit to the list, no cleared field.
    await user.keyboard("{Enter}");
    for (const name of SAVE_SIBLINGS) {
      await user.click(button(name));
    }
    await user.click(button("Save"));
    expect(router.state.location.pathname).toBe(EDIT_PATH);
    expect(textbox("First Name").value).toBe("LARRY");
    expect(puts()).toHaveLength(1);

    release();
    expect((await toastShowing(UPDATED)).getAttribute("data-type")).toBe(
      "success",
    );
    await waitFor(() => {
      expect(button("Save").getAttribute("aria-disabled")).toBe("false");
    });
    for (const name of SAVE_SIBLINGS) {
      expect(button(name).getAttribute("aria-disabled")).toBe("false");
    }
    expect(editForm().getAttribute("aria-busy")).toBe("false");
    expect(document.activeElement).toBe(button("Save"));
    expect(router.state.location.pathname).toBe(EDIT_PATH);
    expect(puts()).toHaveLength(1);
  });
});
