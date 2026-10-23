import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createMemoryRouter,
  MemoryRouter,
  Route,
  RouterProvider,
  Routes,
  useLocation,
} from "react-router";
import type { components } from "@/api/schema";
import { clearSession, getSession, setSession } from "@/auth/session";
import { PATHS, routes } from "@/routes";
import { COUSR02 } from "./COUSR02";

/*
 * Render test of the user update screen (map COUSR2A, transaction CU02, program COUSR02C, route
 * `/users/update`) in jsdom, with `fetch` stubbed for endpoint 18, `GET /api/v1/users/{user_id}`,
 * and endpoint 19, `PUT /api/v1/users/{user_id}`. Expected values are the audited results of
 * `modern/audit/screens.md` (`COUSR02-nnn`) and `modern/audit/online/COUSR02C.md`
 * (`COUSR02C-nnn`). No response below carries a password; the one typed password is synthetic
 * request input.
 */

/** The user a COUSR00 `U` selection hands over in route state (FIXUSR-006). */
const SELECTED_ID = "USER0001";
/** A second user, typed into the user ID input and fetched by ENTER (FIXUSR-001). */
const TYPED_ID = "ADMIN001";
/** A user ID endpoint 18 and endpoint 19 find no row for. */
const UNKNOWN_ID = "NOUSER01";
/** A user ID whose read fails on the server. */
const FAILING_ID = "FAILUSR1";
/** A synthetic new password, typed into the dark field; it only ever travels as request input. */
const TYPED_PASSWORD = "NEWPASS1";

const ENDPOINT = "/api/v1/users/";

/**
 * The classes of the shared map-field box, FieldGrid's `FIELD_INPUT`: `min-w-max` and
 * `field-sizing-content` keep a box never narrower than its value, `px-1` is the one inset every
 * map input shares and `text-sm` the inputs' type size. Restated here because the render-test
 * contract imports no module beside the screen, the session, the routes and the generated types,
 * so a box is checked against this list rather than against the constant that renders it.
 */
const MAP_FIELD_BOX = ["min-w-max", "px-1", "text-sm", "field-sizing-content"] as const;

/** The route state of a COUSR00 `U` selection of {@link SELECTED_ID}, as COUSR00 sends it. */
const FROM_LIST = {
  from: PATHS.COUSR00,
  user_id: SELECTED_ID,
  first_key: "ADMIN001",
  last_key: "USER0005",
};

/** COUSR02C-014: the prompt the screen shows after a successful read. */
const UPDATE_PROMPT = "Press PF5 key to save your updates ...";
/** COUSR03C-012: the delete screen's prompt, which this screen never shows. */
const DELETE_PROMPT = "Press PF5 key to delete this user ...";

const selectedView: components["schemas"]["UserView"] = {
  user_id: SELECTED_ID,
  first_name: "LAWRENCE",
  last_name: "THOMAS",
  user_type: "U",
  update_prompt: UPDATE_PROMPT,
  delete_prompt: DELETE_PROMPT,
};

const typedView: components["schemas"]["UserView"] = {
  user_id: TYPED_ID,
  first_name: "MARGARET",
  last_name: "GOLD",
  user_type: "A",
  update_prompt: UPDATE_PROMPT,
  delete_prompt: DELETE_PROMPT,
};

/** COUSR02C-015: the read finds no row. */
const notFound: components["schemas"]["ScreenError"] = {
  message: "User ID NOT found...",
  field: "user_id",
};

/** COUSR02C-016: the read fails. */
const lookupFailed: components["schemas"]["ScreenError"] = {
  message: "Unable to lookup User...",
  field: "first_name",
};

/** COUSR02C-029: the update's success. */
const updated: components["schemas"]["ScreenResult"] = {
  message: `User ${SELECTED_ID} has been updated ...`,
};

/** COUSR02C-028: nothing differs from the stored row. */
const notModified: components["schemas"]["ScreenError"] = {
  message: "Please modify to update ...",
  field: "first_name",
};

/** COUSR02C-020: a blank last name. */
const lastNameEmpty: components["schemas"]["ScreenError"] = {
  message: "Last Name can NOT be empty...",
  field: "last_name",
};

/** COUSR02C-031: the rewrite finds no row. */
const updateNotFound: components["schemas"]["ScreenError"] = {
  message: "User ID NOT found...",
  field: "user_id",
};

/** COUSR02C-032: the rewrite fails. */
const updateFailed: components["schemas"]["ScreenError"] = {
  message: "Unable to Update User...",
  field: "first_name",
};

/** A 401 for an expired token: the API's own text, naming no field. */
const signonRequired: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
};

/** `User ID can NOT be empty...`, the schema's refusal of a blank user ID before any request. */
const USER_ID_EMPTY = "User ID can NOT be empty...";

/** `CCDA-MSG-INVALID-KEY`, the message of an F-key the screen does not serve (COUSR02C-009). */
const INVALID_KEY_TEXT = "Invalid key pressed. Please see below...";

/** The twelve F-keys a browser sends. */
const ALL_F_KEYS = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];

/** The F-keys COUSR02C serves under no branch but `WHEN OTHER`: all but F3, F4, F5 and F12. */
const UNSERVED_F_KEYS = ALL_F_KEYS.filter((key) => !["F3", "F4", "F5", "F12"].includes(key));

/** The five inputs in map order, by label: `Testing Library` trims the `User Type: ` label. */
const INPUT_LABELS = {
  userId: "Enter User ID:",
  firstName: "First Name:",
  lastName: "Last Name:",
  password: "Password:",
  userType: "User Type:",
} as const;

/** Every input empty. */
const BLANK_INPUTS = { userId: "", firstName: "", lastName: "", password: "", userType: "" };

/** The inputs showing {@link selectedView}; the password is never pre-filled. */
const SHOWN_SELECTED = {
  userId: SELECTED_ID,
  firstName: "LAWRENCE",
  lastName: "THOMAS",
  password: "",
  userType: "U",
};

/** The inputs showing {@link typedView} after ENTER fetched {@link TYPED_ID}. */
const SHOWN_TYPED = {
  userId: TYPED_ID,
  firstName: "MARGARET",
  lastName: "GOLD",
  password: "",
  userType: "A",
};

/** A JSON response, as the API sends every success and every `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces `fetch`. Endpoint 18 answers {@link selectedView} and {@link typedView} by their IDs,
 * 404 {@link notFound} for {@link UNKNOWN_ID} and 500 {@link lookupFailed} for
 * {@link FAILING_ID}; endpoint 19 answers `update()` for every user ID. The request of
 * `hold.method` for `hold.userId` is answered only once `hold.until` has resolved; any other
 * request gets a 500. Returns every request in the order the screen sent it.
 */
function stubApi(
  update: () => Response = () => json(200, updated),
  hold?: { readonly method: string; readonly userId: string; readonly until: Promise<void> },
): Request[] {
  const requests: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      requests.push(req);
      const { pathname } = new URL(req.url);
      const userId = pathname.startsWith(ENDPOINT)
        ? decodeURIComponent(pathname.slice(ENDPOINT.length))
        : undefined;
      if (hold !== undefined && hold.method === req.method && hold.userId === userId) {
        await hold.until;
      }
      if (req.method === "GET" && userId === SELECTED_ID) {
        return json(200, selectedView);
      }
      if (req.method === "GET" && userId === TYPED_ID) {
        return json(200, typedView);
      }
      if (req.method === "GET" && userId === UNKNOWN_ID) {
        return json(404, notFound);
      }
      if (req.method === "GET" && userId === FAILING_ID) {
        return json(500, lookupFailed);
      }
      if (req.method === "PUT" && userId !== undefined && userId !== "") {
        return update();
      }
      const unexpected: components["schemas"]["ScreenError"] = {
        message: `Unexpected request ${req.method} ${pathname}`,
      };
      return json(500, unexpected);
    }),
  );
  return requests;
}

/** A promise and the function that resolves it, for the `hold` of {@link stubApi}. */
function gate(): { readonly until: Promise<void>; readonly open: () => void } {
  let resolveGate: (() => void) | undefined;
  const until = new Promise<void>((resolve) => {
    resolveGate = resolve;
  });
  return {
    until,
    open: () => {
      resolveGate?.();
    },
  };
}

/** Each request as `METHOD path`, in order. */
function sent(requests: readonly Request[]): string[] {
  return requests.map((request) => `${request.method} ${new URL(request.url).pathname}`);
}

/** The JSON body of a request. */
async function bodyOf(request: Request | undefined): Promise<unknown> {
  if (request === undefined) {
    throw new Error("no such request");
  }
  const body: unknown = await request.clone().json();
  return body;
}

/** A navigation target: shows the path it was reached at and the route state it received. */
function RouteStub() {
  const location = useLocation();
  return (
    <section aria-label="Route stub">
      <p data-testid="stub-path">{location.pathname}</p>
      <p data-testid="stub-state">{JSON.stringify(location.state ?? null)}</p>
    </section>
  );
}

/** COUSR02 at `/users/update` with `state`, and stubs at the routes it transfers to. */
function renderScreen(state?: unknown) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COUSR02, state }]}>
        <Routes>
          <Route path={PATHS.COUSR02} element={<COUSR02 />} />
          <Route path={PATHS.COUSR00} element={<RouteStub />} />
          <Route path={PATHS.COADM01} element={<RouteStub />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The route stub a transfer reached: its path and the route state it was given. */
async function reachedStub(): Promise<{ path: string; state: unknown }> {
  const stub = await screen.findByRole("region", { name: "Route stub" });
  const state: unknown = JSON.parse(within(stub).getByTestId("stub-state").textContent ?? "null");
  return { path: within(stub).getByTestId("stub-path").textContent ?? "", state };
}

/** The `<input>` the label `text` names, failing the test when it names anything else. */
function field(text: string): HTMLInputElement {
  const element = screen.getByLabelText(text);
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`"${text}" labels <${element.tagName.toLowerCase()}>, not an input`);
  }
  return element;
}

/** One of the five inputs, by its form name. */
function input(name: keyof typeof INPUT_LABELS): HTMLInputElement {
  return field(INPUT_LABELS[name]);
}

/** The value each input shows, by form name. */
function shownInputs(): Record<keyof typeof INPUT_LABELS, string> {
  return {
    userId: input("userId").value,
    firstName: input("firstName").value,
    lastName: input("lastName").value,
    password: input("password").value,
    userType: input("userType").value,
  };
}

/** The message rows of the screen of one kind (`MessageLine`'s `data-message-kind`). */
function messageRows(kind: "error" | "info"): HTMLElement[] {
  return screen.getAllByRole("alert").filter((row) => row.dataset.messageKind === kind);
}

/**
 * The `ERRMSG` row (`COUSR02-028`), the screen's one message row: its kind, which picks the
 * `destructive` or `default` variant, and its text.
 */
function shownMessage(): { kind: string; text: string } {
  const rows = [...messageRows("error"), ...messageRows("info")];
  expect(rows).toHaveLength(1);
  const [row] = rows;
  return { kind: row?.dataset.messageKind ?? "", text: row?.textContent ?? "" };
}

/**
 * The error text the input `name` is marked with, or `null` when it is not marked. A marked
 * input is `aria-invalid` and its `aria-describedby` names one rendered `sr-only` element holding
 * the text, which is no message row: `ERRMSG` stays the screen's one visible message.
 */
function markOf(name: keyof typeof INPUT_LABELS): string | null {
  const element = input(name);
  const hidden = (element.getAttribute("aria-describedby") ?? "")
    .split(/\s+/)
    .filter((id) => id !== "")
    .map((id) => document.getElementById(id))
    .filter((described) => described?.classList.contains("sr-only") === true);
  if (element.getAttribute("aria-invalid") !== "true") {
    expect(hidden).toEqual([]);
    return null;
  }
  expect(hidden).toHaveLength(1);
  const [message] = hidden;
  expect(message?.closest('[role="alert"]')).toBeNull();
  return message?.textContent ?? null;
}

/** The marks of the five inputs, `null` for each input that is not marked. */
function marks(): Record<keyof typeof INPUT_LABELS, string | null> {
  return {
    userId: markOf("userId"),
    firstName: markOf("firstName"),
    lastName: markOf("lastName"),
    password: markOf("password"),
    userType: markOf("userType"),
  };
}

/** No input marked. */
const UNMARKED = { userId: null, firstName: null, lastName: null, password: null, userType: null };

/**
 * The value of the header field `name` (`data-field` of `ScreenShell`): the one read-only `Input`
 * of that name in the `CardHeader`, failing when there is not exactly one.
 */
function headerField(container: HTMLElement, name: string): string {
  const found = container.querySelectorAll(
    `[data-slot="card-header"] input[data-field="${name}"]`,
  );
  expect(found).toHaveLength(1);
  const [box] = found;
  if (!(box instanceof HTMLInputElement)) {
    throw new Error(`The header has no input ${name}`);
  }
  return box.value;
}

/**
 * Asserts the header value fields of rows 1 and 2 against their rows of `modern/audit/screens.md`
 * (COUSR02-002, -003, -005, -007, -008 and -010). Each entry of `fields` is the BMS name, the `POS`
 * row and column, the `LENGTH`, the value shown, and whether the map colours the field YELLOW.
 *
 * The rows describe each value as a read-only `Input` of the field's width in the `CardHeader`
 * title rows of `ScreenShell`, as `ScreenShell` renders it. The assertions prove what the rows
 * require of that input: it is the shadcn `Input` (`data-slot="input"`), read-only and out of the
 * tab order, as an `ASKIP` field is skipped; it is `LENGTH` characters wide (`size`), its grid
 * cell is the field box at `POS` with `LENGTH` as its width, and it holds exactly `LENGTH`
 * characters; it stands in the `CardHeader`, not in the body or the footer; and it carries the
 * tokens of AAP 0.4.3: the foreground with `font-semibold` for a YELLOW title, the foreground
 * alone for a `NORM` value.
 */
function expectHeaderValues(
  root: ParentNode,
  fields: readonly {
    readonly name: string;
    readonly row: number;
    readonly col: number;
    readonly length: number;
    readonly value: string;
    readonly title: boolean;
  }[],
): void {
  const header = root.querySelector('[data-slot="card-header"]');
  if (!(header instanceof HTMLElement)) {
    throw new Error("the screen renders no CardHeader");
  }
  // The header exposes no textbox that takes typing.
  expect(
    within(header)
      .queryAllByRole("textbox")
      .filter((box) =>
        box instanceof HTMLInputElement || box instanceof HTMLTextAreaElement
          ? !box.readOnly
          : box.getAttribute("aria-readonly") !== "true",
      ),
  ).toEqual([]);
  for (const { name, row, col, length, value, title } of fields) {
    const found = root.querySelectorAll(`[data-field="${name}"]`);
    expect(found).toHaveLength(1);
    const element = found[0];
    if (!(element instanceof HTMLInputElement)) {
      throw new Error(`the screen renders no header input ${name}`);
    }
    // The shadcn `Input`, read-only and LENGTH characters wide.
    expect(element.dataset.slot).toBe("input");
    expect(element.readOnly).toBe(true);
    expect(element.size).toBe(length);
    // Placement: in the CardHeader title rows, not in the body or the footer.
    expect(element.closest('[data-slot="card-header"]')).toBe(header);
    expect(element.closest('[data-slot="card-content"], [data-slot="card-footer"]')).toBeNull();
    // The field box: the grid cell at POS, LENGTH columns wide, holding exactly LENGTH characters.
    expect(element.parentElement?.className).toBe(
      `row-start-${row} col-start-${col} col-span-${length}`,
    );
    expect(element.value).toBe(value);
    expect(element.value).toHaveLength(length);
    // Read-only and skipped by Tab: neither the field nor anything inside it takes typing or is a
    // tab stop.
    const nodes = [element, ...Array.from(element.querySelectorAll("*"))];
    expect(
      nodes.filter(
        (node) =>
          node instanceof HTMLSelectElement ||
          ((node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement) &&
            !node.readOnly) ||
          node.closest('[contenteditable]:not([contenteditable="false"])') !== null,
      ),
    ).toEqual([]);
    expect(element.tabIndex).toBeLessThan(0);
    expect(
      nodes.filter(
        (node) => (node instanceof HTMLElement || node instanceof SVGElement) && node.tabIndex >= 0,
      ),
    ).toEqual([]);
    // Tokens: the foreground colour, with emphasis only on a YELLOW title.
    expect(element.classList.contains("text-foreground")).toBe(true);
    expect(element.classList.contains("font-semibold")).toBe(title);
  }
}

/** The class list of the grid cell that holds `element`: its map row, column and length. */
function cellOf(element: HTMLElement): string {
  return element.parentElement?.className ?? "";
}

/** A row-24 button by its label. */
function key(label: string): HTMLElement {
  return screen.getByRole("button", { name: label });
}

/** A keydown of `name` at the focused element, as the browser sends it; it bubbles to `window`. */
function pressKey(name: string, shiftKey = false): void {
  fireEvent.keyDown(document.activeElement ?? document.body, { key: name, shiftKey });
}

/**
 * Moves the cursor to `element`, so that its return to an input is observable. The move runs
 * inside `waitFor`, which lets React render the blur it causes.
 */
async function moveFocus(element: HTMLElement): Promise<void> {
  await waitFor(() => {
    element.focus();
    expect(document.activeElement).toBe(element);
  });
}

/** Waits until the cursor is in `element`. */
async function focusReaches(element: HTMLElement): Promise<void> {
  await waitFor(() => {
    expect(document.activeElement).toBe(element);
  });
}

/** Types `value` into the input `name`. */
function typeInto(name: keyof typeof INPUT_LABELS, value: string): void {
  fireEvent.change(input(name), { target: { value } });
}

/** Waits until the screen shows {@link selectedView} after the COUSR00 selection's fetch. */
async function selectedShown(): Promise<void> {
  await waitFor(() => {
    expect(shownInputs()).toEqual(SHOWN_SELECTED);
  });
  await waitFor(() => {
    expect(shownMessage()).toEqual({ kind: "info", text: UPDATE_PROMPT });
  });
}

/**
 * Lets pending effects, mutations and stubbed responses run, so that a request the screen would
 * send has been sent before a test counts the requests.
 */
async function settle(): Promise<void> {
  await new Promise((resolve) => {
    setTimeout(resolve, 20);
  });
}

/** The row-24 labels in map order (COUSR02-029). */
const KEY_LABELS = ["ENTER=Fetch", "F3=Save&Exit", "F4=Clear", "F5=Save", "F12=Cancel"];

/** The five inputs in map order, by form name. */
const INPUT_NAMES = ["userId", "firstName", "lastName", "password", "userType"] as const;

/**
 * The keyboard lock the screen shows: the disabled row-24 buttons, the read-only inputs, and the
 * `aria-busy` of the key row and of the form.
 */
function lockShown(): { disabled: string[]; readOnly: string[]; busy: (string | null)[] } {
  return {
    disabled: KEY_LABELS.filter((label) => key(label).getAttribute("aria-disabled") === "true"),
    readOnly: INPUT_NAMES.filter((name) => input(name).readOnly),
    busy: [
      screen.getByRole("group", { name: "Function keys" }).getAttribute("aria-busy"),
      input("userId").form?.getAttribute("aria-busy") ?? null,
    ],
  };
}

/** An idle screen: every key acts, every input is editable, nothing is busy. */
const UNLOCKED = { disabled: [], readOnly: [], busy: ["false", "false"] };

/** A fetch or a save in flight: every key but F12 refused, the inputs read-only, busy. */
const REQUEST_LOCK = {
  disabled: ["ENTER=Fetch", "F3=Save&Exit", "F4=Clear", "F5=Save"],
  readOnly: [...INPUT_NAMES],
  busy: ["true", "true"],
};

/** Once F3 has started: every key refused, F12 included, until the screen has left. */
const LEAVING_LOCK = { ...REQUEST_LOCK, disabled: KEY_LABELS };

/** Waits until the screen shows `lock`. */
async function lockReaches(lock: ReturnType<typeof lockShown>): Promise<void> {
  await waitFor(() => {
    expect(lockShown()).toEqual(lock);
  });
}

beforeEach(() => {
  setSession("test-token", "ADMIN001", "A");
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COUSR02 user update screen", () => {
  // audit: COUSR02-011
  // audit: COUSR02-012
  // audit: COUSR02-015
  // audit: COUSR02-016
  // audit: COUSR02-019
  // audit: COUSR02-022
  // audit: COUSR02-024
  // audit: COUSR02-025
  // audit: COUSR02-027
  // audit: COUSR02-028
  // audit: COUSR02-029
  it("renders the map: title, five labelled inputs at their POS and LENGTH, hints, key row", () => {
    stubApi();
    renderScreen({ from: PATHS.COADM01 });

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Update User");

    // Each label is the INITIAL, bound to its input, at the map's POS and LENGTH. The inputs'
    // `maxLength` is the client half of the `both` rows COUSR02-013, -017, -020, -023 and -026,
    // tagged in src/schemas/COUSR02.test.ts.
    const layout = [
      {
        label: "Enter User ID:",
        name: "userId",
        labelCell: "row-start-6 col-start-6 col-span-14",
        inputCell: "row-start-6 col-start-21 col-span-8",
        length: 8,
      },
      {
        label: "First Name:",
        name: "firstName",
        labelCell: "row-start-11 col-start-6 col-span-11",
        inputCell: "row-start-11 col-start-18 col-span-20",
        length: 20,
      },
      {
        label: "Last Name:",
        name: "lastName",
        labelCell: "row-start-11 col-start-45 col-span-10",
        inputCell: "row-start-11 col-start-56 col-span-20",
        length: 20,
      },
      {
        label: "Password:",
        name: "password",
        labelCell: "row-start-13 col-start-6 col-span-9",
        inputCell: "row-start-13 col-start-16 col-span-8",
        length: 8,
      },
      {
        label: "User Type: ",
        name: "userType",
        labelCell: "row-start-15 col-start-6 col-span-11",
        inputCell: "row-start-15 col-start-17 col-span-1",
        length: 1,
      },
    ] as const;
    for (const row of layout) {
      const label = screen.getByText(row.label.trimEnd());
      expect(label.tagName).toBe("LABEL");
      expect(label.dataset.slot).toBe("form-label");
      expect(label.textContent).toBe(row.label);
      expect(cellOf(label)).toBe(row.labelCell);
      const box = input(row.name);
      expect(box.maxLength).toBe(row.length);
      expect(box.readOnly).toBe(false);
      expect(box.value).toBe("");
      expect(cellOf(box)).toBe(row.inputCell);
    }
    // Every input of two or more characters takes the shared map-field box, `MAP_FIELD_BOX` with
    // `size` = its LENGTH, so a full-length value shows on the narrowest frame.
    for (const row of layout.filter((field) => field.length > 1)) {
      const box = input(row.name);
      expect(box.size).toBe(row.length);
      expect(MAP_FIELD_BOX.filter((name) => !box.classList.contains(name))).toEqual([]);
    }
    expect(input("userId").type).toBe("text");
    expect(input("firstName").type).toBe("text");
    expect(input("lastName").type).toBe("text");
    expect(input("userType").type).toBe("text");
    // The dark field: a password input, never offered for autofill.
    expect(input("password").type).toBe("password");
    expect(input("password").getAttribute("autocomplete")).toBe("off");
    // `IC`: the cursor starts in the user ID input.
    expect(document.activeElement).toBe(input("userId"));

    const rule = screen.getByText("*".repeat(70));
    expect(rule.textContent).toHaveLength(70);
    expect(cellOf(rule)).toBe("row-start-8 col-start-6 col-span-70");
    expect(cellOf(screen.getByText("(8 Char)"))).toBe("row-start-13 col-start-25 col-span-8");
    expect(cellOf(screen.getByText("(A=Admin, U=User)"))).toBe(
      "row-start-15 col-start-19 col-span-17",
    );

    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(
      keys.map((button) => [
        button.textContent,
        button.getAttribute("type"),
        button.dataset.variant,
        button.dataset.size,
      ]),
    ).toEqual([
      ["ENTER=Fetch", "submit", "default", "sm"],
      ["F3=Save&Exit", "button", "outline", "sm"],
      ["F4=Clear", "button", "outline", "sm"],
      ["F5=Save", "button", "outline", "sm"],
      ["F12=Cancel", "button", "outline", "sm"],
    ]);

    // ERRMSG is the one message row: RED (`destructive`), a polite live region, empty.
    expect(messageRows("info")).toHaveLength(0);
    const [errorRow] = messageRows("error");
    expect(errorRow?.getAttribute("aria-live")).toBe("polite");
    expect(errorRow?.classList.contains("text-destructive")).toBe(true);
    expect(shownMessage()).toEqual({ kind: "error", text: "" });
  });

  it("leaves no element at the zero-length stoppers that end the fields", () => {
    stubApi();
    renderScreen();

    // audit: COUSR02-014
    expect(document.querySelector(".row-start-6.col-start-30")).toBeNull();
    // audit: COUSR02-018
    expect(document.querySelector(".row-start-11.col-start-39")).toBeNull();
    // audit: COUSR02-021
    expect(document.querySelector(".row-start-11.col-start-77")).toBeNull();
  });

  // audit: COUSR02C-034
  // audit: COUSR02-030
  // audit: COUSR02-031
  // audit: COUSR02-032
  // audit: COUSR02-033
  // audit: COUSR02-034
  // audit: COUSR02-035
  // The header values of COUSR02-001 to COUSR02-010 are the read-only `Input`s `ScreenShell`
  // renders in the `CardHeader` title rows, skipped by Tab; NAV proves the label rows. This test
  // proves the values the program moves; the next test proves the six value fields' placement,
  // read-only state and width.
  it("fills header rows 1 and 2 at every send: CU02, COUSR02C, titles, date, time", async () => {
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
    stubApi();
    const { container } = renderScreen();

    expect(headerField(container, "TRNNAME")).toBe("CU02");
    expect(headerField(container, "PGMNAME")).toBe("COUSR02C");
    expect(headerField(container, "TITLE01")).toBe("      AWS Mainframe Modernization       ");
    expect(headerField(container, "TITLE02")).toBe("              CardDemo                  ");
    expect(headerField(container, "CURDATE")).toBe("01/02/24");
    expect(headerField(container, "CURTIME")).toBe("03:04:05");

    vi.setSystemTime(new Date(2025, 11, 31, 23, 59, 58));
    typeInto("userId", TYPED_ID);
    fireEvent.click(key("ENTER=Fetch"));
    await waitFor(() => {
      expect(input("firstName").value).toBe("MARGARET");
    });
    expect(headerField(container, "CURDATE")).toBe("12/31/25");
    expect(headerField(container, "CURTIME")).toBe("23:59:58");
    expect(headerField(container, "TRNNAME")).toBe("CU02");
    expect(headerField(container, "PGMNAME")).toBe("COUSR02C");
  });

  // audit: COUSR02-002
  // audit: COUSR02-003
  // audit: COUSR02-005
  // audit: COUSR02-007
  // audit: COUSR02-008
  // audit: COUSR02-010
  it("shows the six header values as read-only Inputs in the CardHeader title rows, each at its POS and LENGTH", async () => {
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
    const requests = stubApi();
    const { container } = renderScreen();

    expectHeaderValues(container, [
      { name: "TRNNAME", row: 1, col: 7, length: 4, value: "CU02", title: false },
      {
        name: "TITLE01",
        row: 1,
        col: 21,
        length: 40,
        value: "      AWS Mainframe Modernization       ",
        title: true,
      },
      { name: "CURDATE", row: 1, col: 71, length: 8, value: "01/02/24", title: false },
      { name: "PGMNAME", row: 2, col: 7, length: 8, value: "COUSR02C", title: false },
      {
        name: "TITLE02",
        row: 2,
        col: 21,
        length: 40,
        value: "              CardDemo                  ",
        title: true,
      },
      { name: "CURTIME", row: 2, col: 71, length: 8, value: "03:04:05", title: false },
    ]);
    await settle();
    expect(requests).toEqual([]);
  });

  // audit: COUSR02C-004
  // audit: COUSR02C-013
  // audit: COUSR02C-014
  it("entered from a COUSR00 selection, fetches that user at once and shows it", async () => {
    const requests = stubApi();
    renderScreen(FROM_LIST);

    // The selected ID is in the input before the response arrives.
    expect(input("userId").value).toBe(SELECTED_ID);
    await selectedShown();
    // The password is never pre-filled, and the read's prompt is this program's, in `default`.
    // COUSR02C-014 has the screen supply the prompt; endpoint 18 carries both sharing programs'
    // prompts instead (X-COUSR02C-1), so the screen's part is choosing COUSR02C's, not COUSR03C's.
    expect(input("password").value).toBe("");
    expect(screen.queryByText(DELETE_PROMPT)).toBeNull();
    const [infoRow] = messageRows("info");
    expect(infoRow?.classList.contains("text-destructive")).toBe(false);
    expect(infoRow?.classList.contains("text-card-foreground")).toBe(true);
    await focusReaches(input("userId"));

    // One read, exactly as ENTER sends it, with the session's bearer token.
    await settle();
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`]);
    expect(requests[0]?.headers.get("Authorization")).toBe("Bearer test-token");
  });

  // audit: COUSR02C-005
  it.each([
    { title: "from the admin menu", state: { from: PATHS.COADM01 } },
    { title: "with no route state", state: undefined },
    { title: "with a route state that is no object", state: "USER0001" },
  ])("entered $title, shows empty inputs and message, sends nothing", async (row) => {
    const requests = stubApi();
    renderScreen(row.state);

    expect(shownInputs()).toEqual(BLANK_INPUTS);
    expect(shownMessage()).toEqual({ kind: "error", text: "" });
    expect(document.activeElement).toBe(input("userId"));
    await settle();
    expect(requests).toHaveLength(0);
  });

  // audit: COUSR02-036
  // audit: COUSR02C-013
  // audit: COUSR02C-014
  it.each([
    { title: "a click on ENTER=Fetch", submit: false },
    { title: "the form's submission by the Enter key", submit: true },
  ])("$title fetches the typed user ID and shows it, clearing a typed password", async (row) => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await selectedShown();

    typeInto("userId", TYPED_ID);
    typeInto("password", TYPED_PASSWORD);
    const enter = key("ENTER=Fetch");
    await moveFocus(enter);
    if (row.submit) {
      const form = input("userId").form;
      expect(form).not.toBeNull();
      if (form !== null) {
        fireEvent.submit(form);
      }
    } else {
      fireEvent.click(enter);
    }

    await waitFor(() => {
      expect(shownInputs()).toEqual({
        userId: TYPED_ID,
        firstName: "MARGARET",
        lastName: "GOLD",
        password: "",
        userType: "A",
      });
    });
    expect(shownMessage()).toEqual({ kind: "info", text: UPDATE_PROMPT });
    await focusReaches(input("userId"));
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`, `GET ${ENDPOINT}${TYPED_ID}`]);
  });

  // audit: COUSR02C-017
  // audit: COUSR02-028
  it.each([
    { title: "the 404 of COUSR02C-015", userId: UNKNOWN_ID, error: notFound, at: "userId" },
    { title: "the 500 of COUSR02C-016", userId: FAILING_ID, error: lookupFailed, at: "firstName" },
  ] as const)("shows $title in red, empties the four inputs, keeps the user ID", async (row) => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await selectedShown();

    typeInto("userId", row.userId);
    typeInto("password", TYPED_PASSWORD);
    fireEvent.click(key("ENTER=Fetch"));

    await waitFor(() => {
      expect(shownMessage()).toEqual({ kind: "error", text: row.error.message });
    });
    const [errorRow] = messageRows("error");
    expect(errorRow?.classList.contains("text-destructive")).toBe(true);
    expect(shownInputs()).toEqual({ ...BLANK_INPUTS, userId: row.userId });
    // The cursor goes to the input the `ScreenError` names, which is marked in error with the
    // message for assistive technology only; the message row stays the one shown.
    await focusReaches(input(row.at));
    expect(input(row.at).getAttribute("aria-invalid")).toBe("true");
    expect(marks()).toEqual({ ...UNMARKED, [row.at]: row.error.message });
    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `GET ${ENDPOINT}${row.userId}`,
    ]);
  });

  // audit: COUSR02C-017
  // COUSR02C-010 is a `both` row: the schema's half is proved in src/schemas/COUSR02.test.ts and
  // the server's in the endpoint tests. This shows what the screen does with the schema's refusal.
  it("ENTER with a blank user ID sends nothing and keeps what the other inputs show", async () => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await selectedShown();

    typeInto("password", TYPED_PASSWORD);
    typeInto("userId", "        ");
    const enter = key("ENTER=Fetch");
    await moveFocus(enter);
    fireEvent.click(enter);

    await waitFor(() => {
      expect(shownMessage()).toEqual({ kind: "error", text: USER_ID_EMPTY });
    });
    expect(shownInputs()).toEqual({
      ...SHOWN_SELECTED,
      userId: "        ",
      password: TYPED_PASSWORD,
    });
    await focusReaches(input("userId"));
    expect(input("userId").getAttribute("aria-invalid")).toBe("true");
    await settle();
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`]);
  });

  // audit: COUSR02-039
  // audit: COUSR02C-033
  it.each([
    { title: "F5=Save with the password left blank", password: "", byKey: false },
    { title: "the F5 key with a typed password", password: TYPED_PASSWORD, byKey: true },
  ])("$title sends endpoint 19, shows its message, keeps every input as typed", async (row) => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await selectedShown();

    typeInto("firstName", "LARRY");
    typeInto("password", row.password);
    const save = key("F5=Save");
    await moveFocus(save);
    if (row.byKey) {
      pressKey("F5");
    } else {
      fireEvent.click(save);
    }

    await waitFor(() => {
      expect(shownMessage()).toEqual({ kind: "info", text: updated.message });
    });
    expect(messageRows("info")[0]?.classList.contains("text-destructive")).toBe(false);
    expect(shownInputs()).toEqual({
      ...SHOWN_SELECTED,
      firstName: "LARRY",
      password: row.password,
    });
    await focusReaches(input("firstName"));

    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `PUT ${ENDPOINT}${SELECTED_ID}`,
    ]);
    // A blank dark field is omitted, which keeps the stored password (E27); a typed one is sent.
    const expected =
      row.password === ""
        ? { first_name: "LARRY", last_name: "THOMAS", user_type: "U" }
        : { first_name: "LARRY", last_name: "THOMAS", user_type: "U", password: TYPED_PASSWORD };
    expect(await bodyOf(requests[1])).toStrictEqual(expected);
    expect(requests[1]?.headers.get("Authorization")).toBe("Bearer test-token");
  });

  // audit: COUSR02C-033
  it.each([
    {
      title: "the 400 of COUSR02C-028 in red, cursor on first name",
      userId: SELECTED_ID,
      status: 400,
      error: notModified,
      kind: "error",
      at: "firstName",
    },
    {
      title: "the 400 of COUSR02C-020 in red, cursor on last name",
      userId: SELECTED_ID,
      status: 400,
      error: lastNameEmpty,
      kind: "error",
      at: "lastName",
    },
    {
      title: "the write's 404 of COUSR02C-031 in neutral, cursor on user ID",
      userId: UNKNOWN_ID,
      status: 404,
      error: updateNotFound,
      kind: "info",
      at: "userId",
    },
    {
      title: "the write's 500 of COUSR02C-032 in neutral, cursor on first name",
      userId: SELECTED_ID,
      status: 500,
      error: updateFailed,
      kind: "info",
      at: "firstName",
    },
    {
      title: "a 401 naming no field in red, cursor on first name",
      userId: SELECTED_ID,
      status: 401,
      error: signonRequired,
      kind: "error",
      at: "firstName",
    },
  ] as const)("F5 shows $title, every input as typed", async (row) => {
    const requests = stubApi(() => json(row.status, row.error));
    renderScreen(FROM_LIST);
    await selectedShown();

    typeInto("userId", row.userId);
    typeInto("userType", "A");
    const save = key("F5=Save");
    await moveFocus(save);
    fireEvent.click(save);

    await waitFor(() => {
      expect(shownMessage()).toEqual({ kind: row.kind, text: row.error.message });
    });
    const [shownRow] = messageRows(row.kind);
    expect(shownRow?.classList.contains("text-destructive")).toBe(row.kind === "error");
    expect(shownInputs()).toEqual({ ...SHOWN_SELECTED, userId: row.userId, userType: "A" });
    await focusReaches(input(row.at));
    // Only a message in red marks its input in error; a neutral one only moves the cursor.
    expect(input(row.at).getAttribute("aria-invalid")).toBe(
      row.kind === "error" && row.error.field != null ? "true" : "false",
    );
    expect(marks()).toEqual(
      row.kind === "error" && row.error.field != null
        ? { ...UNMARKED, [row.at]: row.error.message }
        : UNMARKED,
    );
    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `PUT ${ENDPOINT}${row.userId}`,
    ]);
    expect(await bodyOf(requests[1])).toStrictEqual({
      first_name: "LAWRENCE",
      last_name: "THOMAS",
      user_type: "A",
    });
  });

  // COUSR02C-018 is a `both` row: the schema's half is proved in src/schemas/COUSR02.test.ts and
  // the server's in the endpoint tests. This shows what the screen does with the schema's refusal.
  it("F5 with a blank user ID sends nothing and shows the schema's message in red", async () => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await selectedShown();

    typeInto("userId", "");
    const save = key("F5=Save");
    await moveFocus(save);
    fireEvent.click(save);

    await waitFor(() => {
      expect(shownMessage()).toEqual({ kind: "error", text: USER_ID_EMPTY });
    });
    expect(shownInputs()).toEqual({ ...SHOWN_SELECTED, userId: "" });
    await focusReaches(input("userId"));
    await settle();
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`]);
  });

  // audit: COUSR02-037
  // audit: COUSR02C-006
  it.each([
    {
      title: "F3=Save&Exit after a COUSR00 selection saves, then returns to /users",
      state: FROM_LIST,
      selected: true,
      userId: SELECTED_ID,
      reply: () => json(200, updated),
      byKey: false,
      saves: true,
      target: "/users",
    },
    {
      title: "the F3 key after a COUSR00 selection returns to /users when the save is refused",
      state: FROM_LIST,
      selected: true,
      userId: SELECTED_ID,
      reply: () => json(400, notModified),
      byKey: true,
      saves: true,
      target: "/users",
    },
    {
      title: "F3=Save&Exit after entry from the admin menu saves, then returns to /admin",
      state: { from: PATHS.COADM01 },
      selected: false,
      userId: TYPED_ID,
      reply: () => json(200, updated),
      byKey: false,
      saves: true,
      target: "/admin",
    },
    {
      title: "F3=Save&Exit with no recorded origin saves, then returns to /admin",
      state: undefined,
      selected: false,
      userId: TYPED_ID,
      reply: () => json(500, updateFailed),
      byKey: false,
      saves: true,
      target: "/admin",
    },
    {
      title: "F3=Save&Exit with an origin that is no route of the application returns to /admin",
      state: { from: "https://example.invalid/users", user_id: SELECTED_ID },
      selected: true,
      userId: SELECTED_ID,
      reply: () => json(200, updated),
      byKey: false,
      saves: true,
      target: "/admin",
    },
    {
      title: "F3=Save&Exit with a blank user ID saves nothing and returns to /users",
      state: { from: PATHS.COUSR00 },
      selected: false,
      userId: "",
      reply: () => json(200, updated),
      byKey: false,
      saves: false,
      target: "/users",
    },
  ])("$title", async (row) => {
    const requests = stubApi(row.reply);
    renderScreen(row.state);
    // The entry's outcome first, so that only the key's effect remains to observe.
    if (row.selected) {
      await selectedShown();
    }
    await settle();
    const before = sent(requests);

    typeInto("userId", row.userId);
    typeInto("firstName", "LARRY");
    typeInto("lastName", "THOMAS");
    typeInto("userType", "U");
    if (row.byKey) {
      pressKey("F3");
    } else {
      fireEvent.click(key("F3=Save&Exit"));
    }

    const stub = await reachedStub();
    expect(stub.path).toBe(row.target);
    expect(stub.state).toEqual({ from: "/users/update" });
    // The response's message is never shown: the screen is gone.
    expect(screen.queryAllByRole("alert")).toHaveLength(0);
    await settle();
    if (row.saves) {
      expect(sent(requests)).toEqual([...before, `PUT ${ENDPOINT}${row.userId}`]);
      expect(await bodyOf(requests.at(-1))).toStrictEqual({
        first_name: "LARRY",
        last_name: "THOMAS",
        user_type: "U",
      });
    } else {
      expect(sent(requests)).toEqual(before);
    }
  });

  // audit: COUSR02C-006
  it("once F3 has started, no key acts until the save settles and the screen leaves", async () => {
    const put = gate();
    const requests = stubApi(() => json(200, updated), {
      method: "PUT",
      userId: SELECTED_ID,
      until: put.until,
    });
    renderScreen(FROM_LIST);
    await selectedShown();

    typeInto("firstName", "LARRY");
    fireEvent.click(key("F3=Save&Exit"));
    await waitFor(() => {
      expect(sent(requests)).toEqual([
        `GET ${ENDPOINT}${SELECTED_ID}`,
        `PUT ${ENDPOINT}${SELECTED_ID}`,
      ]);
    });

    // While the save runs, every key is disabled, F12 included, and the inputs are read-only.
    await lockReaches(LEAVING_LOCK);
    // Every key is ignored: no second save, no clear, no cancel, no fetch, no message.
    fireEvent.click(key("F3=Save&Exit"));
    fireEvent.click(key("F5=Save"));
    fireEvent.click(key("F4=Clear"));
    fireEvent.click(key("F12=Cancel"));
    fireEvent.click(key("ENTER=Fetch"));
    for (const name of ["F3", "F5", "F4", "F12", "F7"]) {
      pressKey(name);
    }
    fireEvent.keyDown(document.body, { key: "Enter" });
    await settle();
    expect(screen.queryByRole("region", { name: "Route stub" })).toBeNull();
    expect(shownInputs()).toEqual({ ...SHOWN_SELECTED, firstName: "LARRY" });
    expect(shownMessage()).toEqual({ kind: "info", text: UPDATE_PROMPT });
    expect(requests).toHaveLength(2);

    put.open();
    const stub = await reachedStub();
    expect(stub.path).toBe("/users");
    expect(stub.state).toEqual({ from: "/users/update" });
    expect(requests).toHaveLength(2);
  });

  // audit: COUSR02-038
  // audit: COUSR02C-007
  it("F4 empties every input and the message, cursor on user ID, sends nothing", async () => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await selectedShown();

    // F4=Clear by its button, over a shown user and a typed password, the cursor elsewhere.
    typeInto("password", TYPED_PASSWORD);
    const clear = key("F4=Clear");
    await moveFocus(clear);
    fireEvent.click(clear);
    await waitFor(() => {
      expect(shownInputs()).toEqual(BLANK_INPUTS);
    });
    expect(shownMessage()).toEqual({ kind: "error", text: "" });
    await focusReaches(input("userId"));

    // The F4 key, over an error that marked the user ID in error.
    typeInto("userId", UNKNOWN_ID);
    fireEvent.click(key("ENTER=Fetch"));
    await waitFor(() => {
      expect(shownMessage()).toEqual({ kind: "error", text: notFound.message });
    });
    expect(input("userId").getAttribute("aria-invalid")).toBe("true");
    await moveFocus(key("F4=Clear"));
    pressKey("F4");
    await waitFor(() => {
      expect(shownMessage()).toEqual({ kind: "error", text: "" });
    });
    expect(shownInputs()).toEqual(BLANK_INPUTS);
    expect(input("userId").getAttribute("aria-invalid")).toBe("false");
    expect(marks()).toEqual(UNMARKED);
    await focusReaches(input("userId"));

    await settle();
    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `GET ${ENDPOINT}${UNKNOWN_ID}`,
    ]);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Update User");
  });

  // audit: COUSR02C-007
  // audit: COUSR02C-017
  it("F4 while a fetch is in flight is refused, and the fetch's answer fills the screen", async () => {
    const get = gate();
    const requests = stubApi(() => json(200, updated), {
      method: "GET",
      userId: TYPED_ID,
      until: get.until,
    });
    renderScreen(FROM_LIST);
    await selectedShown();

    typeInto("userId", TYPED_ID);
    fireEvent.click(key("ENTER=Fetch"));
    await waitFor(() => {
      expect(sent(requests)).toEqual([
        `GET ${ENDPOINT}${SELECTED_ID}`,
        `GET ${ENDPOINT}${TYPED_ID}`,
      ]);
    });
    // The program clears the four inputs before its read.
    expect(shownInputs()).toEqual({ ...BLANK_INPUTS, userId: TYPED_ID });
    await lockReaches(REQUEST_LOCK);

    // The keyboard is locked until the program answers: F4 neither clears nor cancels the read.
    fireEvent.click(key("F4=Clear"));
    pressKey("F4");
    await settle();
    expect(shownInputs()).toEqual({ ...BLANK_INPUTS, userId: TYPED_ID });
    expect(shownMessage()).toEqual({ kind: "info", text: UPDATE_PROMPT });

    get.open();
    await waitFor(() => {
      expect(shownInputs()).toEqual(SHOWN_TYPED);
    });
    expect(shownMessage()).toEqual({ kind: "info", text: UPDATE_PROMPT });
    await lockReaches(UNLOCKED);

    // Answered, F4 clears again.
    fireEvent.click(key("F4=Clear"));
    await waitFor(() => {
      expect(shownInputs()).toEqual(BLANK_INPUTS);
    });
    expect(shownMessage()).toEqual({ kind: "error", text: "" });
    expect(requests).toHaveLength(2);
  });

  // audit: COUSR02C-013
  // audit: COUSR02C-033
  it.each([
    {
      title: "ENTER=Fetch clicks",
      method: "GET",
      press: () => {
        fireEvent.click(key("ENTER=Fetch"));
      },
    },
    {
      title: "Enter keydowns outside the form",
      method: "GET",
      press: () => {
        fireEvent.keyDown(document.body, { key: "Enter" });
      },
    },
    {
      title: "submissions of the form by the Enter key in an input",
      method: "GET",
      press: () => {
        const form = input("userId").form;
        if (form !== null) {
          fireEvent.submit(form);
        }
      },
    },
    {
      title: "F5=Save clicks",
      method: "PUT",
      press: () => {
        fireEvent.click(key("F5=Save"));
      },
    },
    {
      title: "F5 keydowns",
      method: "PUT",
      press: () => {
        pressKey("F5");
      },
    },
  ] as const)(
    "repeated $title before the answer send one request, and a later one is sent again",
    async (row) => {
      const answer = gate();
      const target = row.method === "GET" ? TYPED_ID : SELECTED_ID;
      const requests = stubApi(() => json(200, updated), {
        method: row.method,
        userId: target,
        until: answer.until,
      });
      renderScreen(FROM_LIST);
      await selectedShown();
      await lockReaches(UNLOCKED);
      if (row.method === "GET") {
        typeInto("userId", TYPED_ID);
      } else {
        typeInto("firstName", "LARRY");
      }
      const one = [`GET ${ENDPOINT}${SELECTED_ID}`, `${row.method} ${ENDPOINT}${target}`];

      row.press();
      await waitFor(() => {
        expect(sent(requests)).toEqual(one);
      });
      // While the answer is held back: every key but F12 is disabled and the inputs read-only.
      await lockReaches(REQUEST_LOCK);
      row.press();
      row.press();
      // No other request either, and an unserved key shows nothing.
      fireEvent.click(key("ENTER=Fetch"));
      fireEvent.click(key("F5=Save"));
      fireEvent.click(key("F3=Save&Exit"));
      for (const name of ["F3", "F5", "F7"]) {
        pressKey(name);
      }
      await settle();
      expect(sent(requests)).toEqual(one);
      expect(shownMessage()).toEqual({ kind: "info", text: UPDATE_PROMPT });
      expect(screen.queryByRole("region", { name: "Route stub" })).toBeNull();

      answer.open();
      await waitFor(() => {
        expect(shownMessage()).toEqual({
          kind: "info",
          text: row.method === "GET" ? UPDATE_PROMPT : updated.message,
        });
      });
      await lockReaches(UNLOCKED);
      expect(shownInputs()).toEqual(
        row.method === "GET" ? SHOWN_TYPED : { ...SHOWN_SELECTED, firstName: "LARRY" },
      );

      // Settled, the same key sends its request again.
      row.press();
      await waitFor(() => {
        expect(sent(requests)).toEqual([...one, `${row.method} ${ENDPOINT}${target}`]);
      });
      await lockReaches(UNLOCKED);
    },
  );

  // audit: COUSR02C-006
  it("F3 while a save is in flight is refused: no second save, and a later F3 saves once", async () => {
    const put = gate();
    const requests = stubApi(() => json(200, updated), {
      method: "PUT",
      userId: SELECTED_ID,
      until: put.until,
    });
    renderScreen(FROM_LIST);
    await selectedShown();

    // Save A: F5 with the first name LARRY, its answer held back.
    typeInto("firstName", "LARRY");
    fireEvent.click(key("F5=Save"));
    await waitFor(() => {
      expect(sent(requests)).toEqual([
        `GET ${ENDPOINT}${SELECTED_ID}`,
        `PUT ${ENDPOINT}${SELECTED_ID}`,
      ]);
    });
    await lockReaches(REQUEST_LOCK);
    fireEvent.click(key("F3=Save&Exit"));
    pressKey("F3");
    await settle();
    expect(requests).toHaveLength(2);

    // Save A's answer is shown and the screen stays: the refused F3 was not queued.
    put.open();
    await waitFor(() => {
      expect(shownMessage()).toEqual({ kind: "info", text: updated.message });
    });
    await lockReaches(UNLOCKED);
    expect(screen.queryByRole("region", { name: "Route stub" })).toBeNull();

    // Value B, then F3: exactly one save of B, then the return to the user list.
    typeInto("firstName", "LAURENCE");
    fireEvent.click(key("F3=Save&Exit"));
    const stub = await reachedStub();
    expect(stub.path).toBe("/users");
    await settle();
    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `PUT ${ENDPOINT}${SELECTED_ID}`,
      `PUT ${ENDPOINT}${SELECTED_ID}`,
    ]);
    expect(await bodyOf(requests[1])).toStrictEqual({
      first_name: "LARRY",
      last_name: "THOMAS",
      user_type: "U",
    });
    expect(await bodyOf(requests[2])).toStrictEqual({
      first_name: "LAURENCE",
      last_name: "THOMAS",
      user_type: "U",
    });
  });

  // audit: COUSR02C-008
  it.each([
    { title: "a fetch", method: "GET", userId: TYPED_ID },
    { title: "a save", method: "PUT", userId: SELECTED_ID },
  ] as const)("F12 while $title is in flight still goes to /admin", async (row) => {
    const answer = gate();
    const requests = stubApi(() => json(200, updated), {
      method: row.method,
      userId: row.userId,
      until: answer.until,
    });
    renderScreen(FROM_LIST);
    await selectedShown();

    if (row.method === "GET") {
      typeInto("userId", TYPED_ID);
      fireEvent.click(key("ENTER=Fetch"));
    } else {
      fireEvent.click(key("F5=Save"));
    }
    await lockReaches(REQUEST_LOCK);
    fireEvent.click(key("F12=Cancel"));

    const stub = await reachedStub();
    expect(stub.path).toBe("/admin");
    expect(stub.state).toEqual({ from: "/users/update" });
    answer.open();
    await settle();
    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `${row.method} ${ENDPOINT}${row.userId}`,
    ]);
  });

  // audit: COUSR02C-013
  it("a fetch's answer fills nothing once the user ID shown is no longer the one it was sent for", async () => {
    const get = gate();
    const requests = stubApi(() => json(200, updated), {
      method: "GET",
      userId: SELECTED_ID,
      until: get.until,
    });
    renderScreen(FROM_LIST);
    await waitFor(() => {
      expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`]);
    });
    await lockReaches(REQUEST_LOCK);

    // The input is read-only to the keyboard; a script can still replace its value.
    typeInto("userId", TYPED_ID);
    get.open();
    await lockReaches(UNLOCKED);
    expect(shownInputs()).toEqual({ ...BLANK_INPUTS, userId: TYPED_ID });
    expect(shownMessage()).toEqual({ kind: "error", text: "" });
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`]);
  });

  // audit: COUSR02-040
  // audit: COUSR02C-008
  it.each([
    {
      title: "F12=Cancel after a COUSR00 selection",
      state: FROM_LIST,
      selected: true,
      byKey: false,
    },
    {
      title: "the F12 key after a COUSR00 selection",
      state: FROM_LIST,
      selected: true,
      byKey: true,
    },
    {
      title: "F12=Cancel after entry from the admin menu",
      state: { from: PATHS.COADM01 },
      selected: false,
      byKey: false,
    },
  ])("$title goes to /admin, saving nothing", async (row) => {
    const requests = stubApi();
    renderScreen(row.state);
    // The entry's outcome first, so that only the key's effect remains to observe.
    if (row.selected) {
      await selectedShown();
    }
    await settle();
    const before = sent(requests);

    typeInto("firstName", "LARRY");
    if (row.byKey) {
      pressKey("F12");
    } else {
      fireEvent.click(key("F12=Cancel"));
    }

    const stub = await reachedStub();
    expect(stub.path).toBe("/admin");
    expect(stub.state).toEqual({ from: "/users/update" });
    await settle();
    expect(sent(requests)).toEqual(before);
  });

  // audit: COUSR02C-009
  it.each([
    ...UNSERVED_F_KEYS.map((name) => ({ title: name, name, shiftKey: false })),
    // PF13 to PF24: COUSR02C does not fold them onto PF1 to PF12, so Shift+F5 is no F5.
    ...ALL_F_KEYS.map((name) => ({ title: `Shift+${name}`, name, shiftKey: true })),
  ])("$title shows the invalid-key message in red and keeps every input", async (row) => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await selectedShown();
    typeInto("firstName", "LARRY");

    pressKey(row.name, row.shiftKey);

    await waitFor(() => {
      expect(shownMessage()).toEqual({ kind: "error", text: INVALID_KEY_TEXT });
    });
    expect(shownInputs()).toEqual({ ...SHOWN_SELECTED, firstName: "LARRY" });
    expect(screen.queryByRole("region", { name: "Route stub" })).toBeNull();
    await settle();
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`]);
  });

  it("an invalid key unmarks the input a red ScreenError marked, as its message replaces the error", async () => {
    stubApi(() => json(400, lastNameEmpty));
    renderScreen(FROM_LIST);
    await selectedShown();
    fireEvent.click(key("F5=Save"));
    await waitFor(() => {
      expect(shownMessage()).toEqual({ kind: "error", text: lastNameEmpty.message });
    });
    expect(marks()).toEqual({ ...UNMARKED, lastName: lastNameEmpty.message });

    pressKey("F2");

    await waitFor(() => {
      expect(shownMessage()).toEqual({ kind: "error", text: INVALID_KEY_TEXT });
    });
    expect(marks()).toEqual(UNMARKED);
  });
});

// COUSR02C entered with no COMMAREA (`EIBCALEN = 0`) transfers to COSGN00C. The replacement is
// the route guard that only the application's route table mounts in front of `/users/update`, so
// this group renders that table rather than the screen alone, with nobody signed on. The file's
// `afterEach` removes the `fetch` stub and the session.
describe("COUSR02 /users/update opened with no session, through the route table", () => {
  beforeEach(() => {
    clearSession();
  });

  // audit: COUSR02C-003
  it("the route guard sends a COUSR00 selection to /signon before the entry fetch, with no request", async () => {
    // This case covers the row's no-session condition through the route table: the real routes
    // and their `RequireSignon` guard, not a stub of either. The row's 401 condition is covered
    // by the group "with a request answered 401" below. Entered with a COUSR00 `U` selection, the
    // screen itself fetches the selected user at once, so the empty request list shows that the
    // guard stops that fetch. The guard only hides the screen; the API's admin gate is the
    // control.
    const requests = stubApi();
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, {
      initialEntries: [{ pathname: PATHS.COUSR02, state: FROM_LIST }],
    });
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );

    // Both screens load lazily, through the route tree, before the sign-on screen renders.
    await waitFor(
      () => {
        expect(router.state.location.pathname).toBe(PATHS.COSGN00);
        expect(screen.getByRole("button", { name: "ENTER=Sign-on" })).toBeTruthy();
      },
      { timeout: 4000 },
    );
    expect(screen.queryByRole("heading", { level: 1, name: "Update User" })).toBeNull();
    for (const label of Object.values(INPUT_LABELS)) {
      expect(screen.queryByLabelText(label)).toBeNull();
    }
    expect(screen.queryByRole("button", { name: "F5=Save" })).toBeNull();

    await settle();
    expect(sent(requests)).toEqual([]);
    router.dispose();
  });
});

// The row's other condition: a request of the screen answered 401, the API's form of a token that
// no longer verifies. `api/client.ts` clears the session on it, and the `RequireSignon` guard the
// route table mounts, which follows the session, replaces the mounted screen with `/signon`, as
// the XCTL to COSGN00C does. The file's `beforeEach` signs ADMIN001 on, so every request carries
// a token; its `afterEach` removes the `fetch` stub and the session.
describe("COUSR02 /users/update with a request answered 401, through the route table", () => {
  /**
   * Replaces `fetch`: every request is answered 401 with {@link signonRequired}, except, when
   * `entryServed`, the entry fetch of {@link SELECTED_ID}, which gets {@link selectedView}.
   * Returns every request in the order the screen sent it.
   */
  function stubSignonRequired(entryServed: boolean): Request[] {
    const requests: Request[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const req = input instanceof Request ? input : new Request(input, init);
        requests.push(req);
        const { pathname } = new URL(req.url);
        if (entryServed && req.method === "GET" && pathname === `${ENDPOINT}${SELECTED_ID}`) {
          return json(200, selectedView);
        }
        return json(401, signonRequired);
      }),
    );
    return requests;
  }

  /** The route table at `/users/update`, entered with a COUSR00 `U` selection. */
  function renderRoutes(): ReturnType<typeof createMemoryRouter> {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, {
      initialEntries: [{ pathname: PATHS.COUSR02, state: FROM_LIST }],
    });
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
    return router;
  }

  /**
   * Waits until the browser is at `/signon` and the sign-on screen, loaded lazily through the
   * route tree, shows; then asserts that the update screen is gone and the session ended.
   */
  async function signonReached(router: ReturnType<typeof createMemoryRouter>): Promise<void> {
    await waitFor(
      () => {
        expect(router.state.location.pathname).toBe(PATHS.COSGN00);
        expect(screen.getByRole("button", { name: "ENTER=Sign-on" })).toBeTruthy();
      },
      { timeout: 4000 },
    );
    expect(screen.queryByRole("heading", { level: 1, name: "Update User" })).toBeNull();
    for (const label of Object.values(INPUT_LABELS)) {
      expect(screen.queryByLabelText(label)).toBeNull();
    }
    expect(screen.queryByRole("button", { name: "F5=Save" })).toBeNull();
    expect(getSession()).toBeNull();
  }

  // audit: COUSR02C-003
  it("the entry fetch answered 401 takes the screen to /signon and ends the session", async () => {
    const requests = stubSignonRequired(false);
    const router = renderRoutes();

    await signonReached(router);
    // The guard let the screen mount with the session, so the screen's own entry fetch went out
    // with the token; its 401 alone ended the session, and nothing was sent after it.
    await settle();
    expect(sent(requests)).toEqual([`GET ${ENDPOINT}${SELECTED_ID}`]);
    expect(requests[0]?.headers.get("Authorization")).toBe("Bearer test-token");
    router.dispose();
  });

  // audit: COUSR02C-003
  it("F5=Save answered 401 takes the screen to /signon and ends the session", async () => {
    const requests = stubSignonRequired(true);
    const router = renderRoutes();
    await selectedShown();
    expect(screen.getByRole("heading", { level: 1, name: "Update User" })).toBeTruthy();

    fireEvent.click(key("F5=Save"));

    await signonReached(router);
    await settle();
    expect(sent(requests)).toEqual([
      `GET ${ENDPOINT}${SELECTED_ID}`,
      `PUT ${ENDPOINT}${SELECTED_ID}`,
    ]);
    expect(requests[1]?.headers.get("Authorization")).toBe("Bearer test-token");
    router.dispose();
  });
});

// Endpoints 18 and 19 name the user by the path alone, and every request carries the stored
// session's token.
describe("COUSR02: endpoint 18 and 19 requests", () => {
  /** A request as its method, path, query, `Authorization` header and JSON body. */
  async function shapeOf(request: Request) {
    const url = new URL(request.url);
    const text = await request.clone().text();
    const body: unknown = text === "" ? undefined : JSON.parse(text);
    return {
      method: request.method,
      path: url.pathname,
      query: Object.fromEntries(url.searchParams),
      authorization: request.headers.get("Authorization"),
      body,
    };
  }

  it("the read and F5's update each send the user's path with the bearer token and no query, and the update's message shows", async () => {
    const requests = stubApi();
    renderScreen(FROM_LIST);
    await selectedShown();

    typeInto("firstName", "LARRY");
    fireEvent.click(key("F5=Save"));
    await waitFor(() => {
      expect(shownMessage()).toEqual({ kind: "info", text: updated.message });
    });
    await settle();

    expect(await Promise.all(requests.map(shapeOf))).toEqual([
      {
        method: "GET",
        path: `${ENDPOINT}${SELECTED_ID}`,
        query: {},
        authorization: "Bearer test-token",
        body: undefined,
      },
      {
        method: "PUT",
        path: `${ENDPOINT}${SELECTED_ID}`,
        query: {},
        authorization: "Bearer test-token",
        body: { first_name: "LARRY", last_name: "THOMAS", user_type: "U" },
      },
    ]);
  });
});

/**
 * Renders the application's route table at `/users/update` with `state`, so the admin guard that
 * only the table mounts stands in front of the screen. Its screens load lazily, so a wait for one
 * to render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(state?: unknown): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, {
    initialEntries: [{ pathname: PATHS.COUSR02, state }],
  });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

/** Replaces `fetch` with a stub that answers every request 401; returns them in order sent. */
function stubRefusingAll(): Request[] {
  const requests: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      requests.push(req);
      return json(401, signonRequired);
    }),
  );
  return requests;
}

/** Sign-on replaced the screen: its ENTER key shows at `/signon`, and no `Update User` title. */
async function expectSignonShown(router: ReturnType<typeof createMemoryRouter>): Promise<void> {
  expect(
    await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
  ).toBeDefined();
  expect(router.state.location.pathname).toBe(PATHS.COSGN00);
  expect(screen.queryByRole("heading", { level: 1, name: "Update User" })).toBeNull();
}

// COUSR02C-003: CU02 reached with no COMMAREA transfers to COSGN00C. The replacement's counterpart
// is the admin guard in front of `/users/update`, which only the route table mounts: no session
// on entry, or a session a 401 cleared (`api/client.ts`) while the screen is mounted, both go to
// `/signon`; after the key that sent the refused request, the test presses no key and follows no
// link. A `U` session is sent to the main menu instead, as no menu of its own reaches this route.
describe("COUSR02 in the route table: the admin guard and a 401", () => {
  // audit: COUSR02C-003
  it("opened with no session, goes to sign-on before endpoint 18 gets a request", async () => {
    sessionStorage.clear();
    const requests = stubApi();
    const router = renderRouteTable(FROM_LIST);

    await expectSignonShown(router);
    await settle();
    expect(requests).toHaveLength(0);
    expect(getSession()).toBeNull();
    router.dispose();
  });

  // audit: COUSR02C-003
  it("opened with a U session, goes to the main menu, and endpoints 18 and 19 get no request", async () => {
    setSession("test-token", "USER0001", "U");
    const requests = stubApi();
    const router = renderRouteTable(FROM_LIST);

    await waitFor(
      () => {
        expect(router.state.location.pathname).toBe(PATHS.COMEN01);
      },
      { timeout: 5_000 },
    );
    expect(screen.queryByRole("heading", { level: 1, name: "Update User" })).toBeNull();
    // The main menu loads endpoint 2 on entry, which `stubApi` answers 500. Waiting until the menu
    // shows that answer keeps the load's last render inside the test's waits.
    expect(
      await screen.findByRole("heading", { level: 1, name: "Main Menu" }, { timeout: 5_000 }),
    ).toBeDefined();
    await waitFor(() => {
      expect(shownMessage()).toEqual({
        kind: "error",
        text: "Unexpected request GET /api/v1/menus/main",
      });
    });
    await settle();
    expect(sent(requests).filter((request) => request.includes(ENDPOINT))).toEqual([]);
    router.dispose();
  });

  // audit: COUSR02C-003
  it.each([
    {
      title: "the fetch of ENTER (endpoint 18)",
      state: { from: PATHS.COADM01 },
      stub: stubRefusingAll,
      drive: async () => {
        typeInto("userId", TYPED_ID);
        fireEvent.click(key("ENTER=Fetch"));
      },
      expected: [`GET ${ENDPOINT}${TYPED_ID}`],
    },
    {
      title: "the update of F5 (endpoint 19)",
      state: FROM_LIST,
      stub: () => stubApi(() => json(401, signonRequired)),
      drive: async () => {
        await selectedShown();
        typeInto("firstName", "LARRY");
        fireEvent.click(key("F5=Save"));
      },
      expected: [`GET ${ENDPOINT}${SELECTED_ID}`, `PUT ${ENDPOINT}${SELECTED_ID}`],
    },
  ])(
    "$title answered 401 replaces the screen with sign-on and clears the session, with no retry",
    async ({ state, stub, drive, expected }) => {
      const requests = stub();
      const router = renderRouteTable(state);
      expect(
        await screen.findByRole("heading", { level: 1, name: "Update User" }, { timeout: 5_000 }),
      ).toBeDefined();

      await drive();

      await expectSignonShown(router);
      expect(getSession()).toBeNull();
      expect(sent(requests)).toEqual(expected);
      expect(requests.map((request) => request.headers.get("Authorization"))).toEqual(
        expected.map(() => "Bearer test-token"),
      );
      router.dispose();
    },
  );
});
