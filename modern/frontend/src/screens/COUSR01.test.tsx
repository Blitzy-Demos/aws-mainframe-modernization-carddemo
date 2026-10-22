import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
import { clearSession, SESSION_KEY, setSession } from "@/auth/session";
import { INVALID_KEY } from "@/lib/header";
import { PATHS, routes } from "@/routes";
import { COUSR01 } from "./COUSR01";

/*
 * Render test of the Add User screen (map COUSR1A, transaction CU01, program COUSR01C, route
 * `/users/add`) in jsdom, with `fetch` stubbed for endpoint 17 (`POST /api/v1/users`) and a 500
 * for any other request, so no request leaves the test. Expected texts are the audited literals
 * of `modern/audit/screens.md` (`COUSR01-nnn`) and `modern/audit/online/COUSR01C.md`
 * (`COUSR01C-nnn`). The typed password is synthetic request input; no response fixture carries a
 * password.
 */

const USERS_PATH = "/api/v1/users";

/** A synthetic typed password in lower case, sent as typed and never upper-cased (E26). */
const TYPED_PASSWORD = "secret1";

/**
 * The five fields as typed. The first name is lower case with a trailing space and the last name
 * has a leading space: the client trims and case-changes nothing (COUSR01C-006).
 */
const typed: components["schemas"]["UserAddRequest"] = {
  first_name: "alex ",
  last_name: " Morgan",
  user_id: "NEWUSR01",
  password: TYPED_PASSWORD,
  user_type: "U",
};

/** The same entry with User Type left empty, which only the server rejects (COUSR01C-014). */
const typedWithoutType: components["schemas"]["UserAddRequest"] = { ...typed, user_type: "" };

/** Endpoint 17's 200 for `NEWUSR01` (COUSR01C-021). */
const added: components["schemas"]["ScreenResult"] = {
  message: "User NEWUSR01 has been added ...",
};

/** The 400 for a user ID already stored (COUSR01C-023): 400, not 409, per the audit. */
const duplicate: components["schemas"]["ScreenError"] = {
  message: "User ID already exist...",
  field: "user_id",
};

/** The 400 for an empty User Type (COUSR01C-014). */
const typeEmpty: components["schemas"]["ScreenError"] = {
  message: "User Type can NOT be empty...",
  field: "user_type",
};

/** The 500 for a failed insert, with the COBOL's cursor on First Name (COUSR01C-024). */
const writeFailure: components["schemas"]["ScreenError"] = {
  message: "Unable to Add User...",
  field: "first_name",
};

/**
 * The map's label literals, each its exact `INITIAL`, in map order; `User Type: ` keeps its
 * trailing blank.
 */
const LABELS = {
  firstName: "First Name:",
  lastName: "Last Name:",
  userId: "User ID:",
  password: "Password:",
  userType: "User Type: ",
} as const;

/** Text matching that keeps every space, as the map literals do. */
const exactText = { normalizer: (text: string) => text };

/** A JSON response, as the API sends it. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces `fetch`: `POST /api/v1/users` is recorded with its `Authorization` header, URL and JSON
 * body, then gets `reply` once `reply.gate` (when given) settles; any other request, and the add
 * without a `reply`, gets a 500.
 */
function stubFetch(reply?: {
  status: number;
  body: components["schemas"]["ScreenResult"] | components["schemas"]["ScreenError"];
  gate?: Promise<void>;
}) {
  const sent: { authorization: string | null; url: string; body: unknown }[] = [];
  const fetchStub = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const req = input instanceof Request ? input : new Request(input, init);
    if (reply !== undefined && req.method === "POST" && new URL(req.url).pathname === USERS_PATH) {
      sent.push({
        authorization: req.headers.get("Authorization"),
        url: req.url,
        body: await req.clone().json(),
      });
      await reply.gate;
      return json(reply.status, reply.body);
    }
    const unexpected: components["schemas"]["ScreenError"] = {
      message: `Unexpected request ${req.method} ${new URL(req.url).pathname}`,
    };
    return json(500, unexpected);
  });
  vi.stubGlobal("fetch", fetchStub);
  return { fetchStub, sent };
}

/** The screen a navigation reached: its path and the route state it was given. */
function RouteStub() {
  const location = useLocation();
  return <p>{`Stub at ${location.pathname} ${JSON.stringify(location.state)}`}</p>;
}

/** COUSR01 at `/users/add`, with a stub at `/admin`, the only route it navigates to. */
function renderScreen() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[PATHS.COUSR01]}>
        <Routes>
          <Route path={PATHS.COUSR01} element={<COUSR01 />} />
          <Route path={PATHS.COADM01} element={<RouteStub />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** `element` as an `<input>`, failing the test when it is anything else. */
function asInput(element: HTMLElement): HTMLInputElement {
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`Expected an input, found <${element.tagName.toLowerCase()}>`);
  }
  return element;
}

/** The input a map label is bound to. */
function inputFor(label: string): HTMLInputElement {
  return asInput(screen.getByLabelText(label, exactText));
}

/** The `<form>` that `input` belongs to, failing the test when there is none. */
function formOf(input: HTMLInputElement): HTMLFormElement {
  if (input.form === null) {
    throw new Error("the input is not inside a form");
  }
  return input.form;
}

/** The five inputs in map order. */
function inputs(): HTMLInputElement[] {
  return Object.values(LABELS).map(inputFor);
}

/** The five values on the screen, in map order. */
function values(): string[] {
  return inputs().map((input) => input.value);
}

/** `entry`'s five values in map order, as `values()` reports them. */
function inMapOrder(entry: components["schemas"]["UserAddRequest"]): string[] {
  return [entry.first_name, entry.last_name, entry.user_id, entry.password, entry.user_type].map(
    (value) => value ?? "",
  );
}

/**
 * Moves the cursor to `input`. The move runs inside `act`, so React renders the blur it causes
 * (the field state the form labels and controls subscribe to) before the test goes on.
 */
function moveFocus(input: HTMLInputElement): void {
  act(() => {
    input.focus();
  });
}

/** Types `entry` into the five fields, then leaves the cursor in Password, away from First Name. */
function typeEntry(entry: components["schemas"]["UserAddRequest"]): void {
  inputs().forEach((input, index) => {
    fireEvent.change(input, { target: { value: inMapOrder(entry)[index] } });
  });
  moveFocus(inputFor(LABELS.password));
}

/** The message rows of `kind` (COUSR1A declares only `ERRMSG`, so at most one exists). */
function messageRows(kind: "error" | "info"): HTMLElement[] {
  return screen
    .getAllByRole("alert")
    .filter((row) => row.getAttribute("data-message-kind") === kind);
}

/** The single message row, which must be of `kind`. */
function messageRow(kind: "error" | "info"): HTMLElement {
  const rows = messageRows(kind);
  expect(rows).toHaveLength(1);
  return rows[0];
}

/** The key button labelled `label` in the row-24 key group. */
function key(label: string): HTMLElement {
  return within(screen.getByRole("group", { name: "Function keys" })).getByRole("button", {
    name: label,
  });
}

/**
 * The error text `input` is marked with, or `null` when it is not marked. A marked input is
 * `aria-invalid` and its `aria-describedby` names one rendered `sr-only` element holding the text,
 * which is no message row: the error row stays the screen's one visible message.
 */
function markOf(input: HTMLElement): string | null {
  const hidden = (input.getAttribute("aria-describedby") ?? "")
    .split(/\s+/)
    .filter((id) => id !== "")
    .map((id) => document.getElementById(id))
    .filter((element) => element?.classList.contains("sr-only") === true);
  if (input.getAttribute("aria-invalid") !== "true") {
    expect(hidden).toEqual([]);
    return null;
  }
  expect(hidden).toHaveLength(1);
  const [message] = hidden;
  expect(message?.closest('[role="alert"]')).toBeNull();
  return message?.textContent ?? null;
}

/** The marks of the five inputs in map order, `null` for each input that is not marked. */
function marks(): (string | null)[] {
  return inputs().map(markOf);
}

/** The text of the header field `name` (`data-field` of `ScreenShell`). */
function headerField(container: HTMLElement, name: string): string | null {
  return container.querySelector(`[data-field="${name}"]`)?.textContent ?? null;
}

/**
 * Asserts the header value fields of rows 1 and 2 against their rows of `modern/audit/screens.md`
 * (COUSR01-002, -003, -005, -007, -008 and -010). Each entry of `fields` is the BMS name, the `POS`
 * row and column, the `LENGTH`, the value shown, and whether the map colours the field YELLOW.
 *
 * The rows describe each value as a read-only `Input` of the field's width in the `CardHeader`
 * title rows of `ScreenShell`. `ScreenShell` realises it as read-only text in those title rows,
 * its recorded decision (the header rows belong to the frame's title rows, not to the map's input
 * or output fields). The assertions prove what the rows require of that text: it is read-only and
 * out of the tab order, as an `ASKIP` field is skipped; its grid cell is the field box at `POS`
 * with `LENGTH` as its width, and it shows exactly `LENGTH` characters; it stands in the
 * `CardHeader`, not in the body or the footer; and it carries the tokens of AAP 0.4.3: the
 * foreground with `font-semibold` for a YELLOW title, the foreground alone for a `NORM` value.
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
    if (!(element instanceof HTMLElement)) {
      throw new Error(`the screen renders no header field ${name}`);
    }
    // Placement: in the CardHeader title rows, not in the body or the footer.
    expect(element.closest('[data-slot="card-header"]')).toBe(header);
    expect(element.closest('[data-slot="card-content"], [data-slot="card-footer"]')).toBeNull();
    // The field box: the grid cell at POS, LENGTH columns wide, holding exactly LENGTH characters.
    expect(element.parentElement?.className).toBe(
      `row-start-${row} col-start-${col} col-span-${length}`,
    );
    const shown = element instanceof HTMLInputElement ? element.value : element.textContent;
    expect(shown).toBe(value);
    expect(shown).toHaveLength(length);
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

/** The `POS` columns of the cells the form places on map line `line`, in DOM order. */
function lineColumns(form: HTMLFormElement, line: number): string[] {
  return Array.from(form.querySelectorAll(`[class*="row-start-${line} "]`)).map(
    (cell) => /col-start-([0-9]+)/.exec(cell.className)?.[1] ?? "",
  );
}

/** The screen is still COUSR01: no navigation happened. */
function expectNoNavigation(): void {
  expect(screen.getByRole("heading", { level: 1, name: "Add User" })).toBeTruthy();
  expect(screen.queryByText(/^Stub at /)).toBeNull();
}

describe("COUSR01 add user (CU01, COUSR01C, /users/add)", () => {
  beforeEach(() => {
    setSession("test-token", "ADMIN001", "A");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  // audit: COUSR01C-004
  // audit: COUSR01-011
  // audit: COUSR01-012
  // audit: COUSR01-014
  // audit: COUSR01-015
  // audit: COUSR01-017
  // audit: COUSR01-018
  // audit: COUSR01-020
  // audit: COUSR01-021
  // audit: COUSR01-023
  // audit: COUSR01-024
  // audit: COUSR01-026
  // audit: COUSR01-027
  // audit: COUSR01-028
  // audit: COUSR01-035
  // audit: COUSR01-036
  // audit: COUSR01-037
  // audit: COUSR01-038
  it("renders the empty map on first entry: title, labels, inputs, hints, an empty error row and the keys", () => {
    const { fetchStub } = stubFetch();
    renderScreen();

    expect(fetchStub).not.toHaveBeenCalled();

    const title = screen.getByRole("heading", { level: 1 });
    expect(title.textContent).toBe("Add User");
    expect(title.getAttribute("data-slot")).toBe("card-title");

    // Each FormLabel carries its INITIAL and is bound to its input; each input starts empty with
    // its map LENGTH; only PASSWD (DRK) is masked.
    const expected = [
      { label: LABELS.firstName, maxLength: 20, type: "text" },
      { label: LABELS.lastName, maxLength: 20, type: "text" },
      { label: LABELS.userId, maxLength: 8, type: "text" },
      { label: LABELS.password, maxLength: 8, type: "password" },
      { label: LABELS.userType, maxLength: 1, type: "text" },
    ];
    for (const { label, maxLength, type } of expected) {
      const input = inputFor(label);
      const labelElement = document.querySelector(`label[for="${input.id}"]`);
      expect(labelElement?.textContent).toBe(label);
      expect(labelElement?.getAttribute("data-slot")).toBe("form-label");
      expect(input.maxLength).toBe(maxLength);
      expect(input.type).toBe(type);
      expect(input.value).toBe("");
      expect(input.getAttribute("autocomplete")).toBe("off");
    }
    // The cursor starts on FNAME, the map's IC field.
    expect(document.activeElement).toBe(inputFor(LABELS.firstName));

    // The BLUE hints, each tied to its input as the FormItem's description.
    const hints = screen.getAllByText("(8 Char)", exactText);
    expect(hints).toHaveLength(2);
    expect(inputFor(LABELS.userId).getAttribute("aria-describedby")).toBe(hints[0].id);
    expect(inputFor(LABELS.password).getAttribute("aria-describedby")).toBe(hints[1].id);
    const typeHint = screen.getByText("(A=Admin, U=User)", exactText);
    expect(inputFor(LABELS.userType).getAttribute("aria-describedby")).toBe(typeHint.id);

    // Every cell starts at its POS column; the zero-length stoppers at (8,39) and (8,77) have no
    // element, and the `User Type: ` label ends before USRTYPE at column 17.
    const form = formOf(inputFor(LABELS.firstName));
    expect(lineColumns(form, 8)).toEqual(["6", "18", "45", "56"]);
    expect(lineColumns(form, 11)).toEqual(["6", "15", "24", "45", "55", "64"]);
    expect(lineColumns(form, 14)).toEqual(["6", "17", "19"]);
    expect(form.querySelector(".col-start-39")).toBeNull();
    expect(form.querySelector(".col-start-77")).toBeNull();
    expect(form.querySelector('[class*="row-start-14 col-start-6 col-span-11"]')).not.toBeNull();

    // ERRMSG: the one message row, an error row, polite live region, empty on first entry.
    expect(screen.getAllByRole("alert")).toHaveLength(1);
    const row = messageRow("error");
    expect(row.textContent).toBe("");
    expect(row.getAttribute("aria-live")).toBe("polite");
    expect(row.classList.contains("text-destructive")).toBe(true);

    // Row 24 in map order: ENTER submits the form, the others are outline buttons.
    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(keys.map((button) => button.textContent)).toEqual([
      "ENTER=Add User",
      "F3=Back",
      "F4=Clear",
      "F12=Exit",
    ]);
    const [enterKey, ...pfKeys] = keys;
    expect(enterKey.getAttribute("type")).toBe("submit");
    expect(enterKey.getAttribute("form")).toBe(form.id);
    expect(enterKey.dataset.variant).toBe("default");
    for (const pfKey of pfKeys) {
      expect(pfKey.getAttribute("type")).toBe("button");
      expect(pfKey.dataset.variant).toBe("outline");
      expect(pfKey.dataset.size).toBe("sm");
    }
  });

  // audit: COUSR01C-027
  // audit: COUSR01-029
  // audit: COUSR01-030
  // audit: COUSR01-031
  // audit: COUSR01-032
  // audit: COUSR01-033
  // audit: COUSR01-034
  it("fills header rows 1 and 2 on every send: CU01, COUSR01C, the titles, the date and time", () => {
    const { fetchStub } = stubFetch();
    const { container } = renderScreen();

    const expectHeader = () => {
      expect(headerField(container, "TRNNAME")).toBe("CU01");
      expect(headerField(container, "PGMNAME")).toBe("COUSR01C");
      expect(headerField(container, "TITLE01")).toBe("      AWS Mainframe Modernization       ");
      expect(headerField(container, "TITLE02")).toBe("              CardDemo                  ");
      // The date and time are the current instant, so only their formats are compared.
      expect(headerField(container, "CURDATE")).toMatch(/^[0-9]{2}\/[0-9]{2}\/[0-9]{2}$/);
      expect(headerField(container, "CURTIME")).toMatch(/^[0-9]{2}:[0-9]{2}:[0-9]{2}$/);
    };
    expectHeader();

    fireEvent.click(key("F12=Exit"));
    expect(messageRow("error").textContent).toBe(INVALID_KEY);
    expectHeader();

    fireEvent.click(key("F4=Clear"));
    expect(messageRow("error").textContent).toBe("");
    expectHeader();
    expect(fetchStub).not.toHaveBeenCalled();
  });

  // audit: COUSR01-002
  // audit: COUSR01-003
  // audit: COUSR01-005
  // audit: COUSR01-007
  // audit: COUSR01-008
  // audit: COUSR01-010
  it("shows the six header values as read-only text in the CardHeader title rows, each at its POS and LENGTH", () => {
    // Only `Date` is faked, so the timers that React and TanStack Query use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
      const { fetchStub } = stubFetch();
      const { container } = renderScreen();

      expectHeaderValues(container, [
        { name: "TRNNAME", row: 1, col: 7, length: 4, value: "CU01", title: false },
        {
          name: "TITLE01",
          row: 1,
          col: 21,
          length: 40,
          value: "      AWS Mainframe Modernization       ",
          title: true,
        },
        { name: "CURDATE", row: 1, col: 71, length: 8, value: "01/02/24", title: false },
        { name: "PGMNAME", row: 2, col: 7, length: 8, value: "COUSR01C", title: false },
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
      expect(fetchStub).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  // audit: COUSR01C-006
  // audit: COUSR01C-022
  // audit: COUSR01-035
  it.each([
    { how: "the ENTER=Add User button", press: () => fireEvent.click(key("ENTER=Add User")) },
    {
      // jsdom has no implicit submission, so the submit event the Enter key fires in a field
      // is dispatched on the field's form directly.
      how: "the Enter key's form submission",
      press: () => fireEvent.submit(formOf(inputFor(LABELS.password))),
    },
  ])(
    "ENTER by $how sends the five fields as typed; a 200 shows the message as information, empties the fields and focuses First Name",
    async ({ press }) => {
      const { sent } = stubFetch({ status: 200, body: added });
      renderScreen();

      typeEntry(typed);
      press();

      await waitFor(() => {
        expect(messageRow("info").textContent).toBe("User NEWUSR01 has been added ...");
      });
      // One request, with the admin's token and the values unchanged: the lower-case password is
      // not upper-cased and no field is trimmed. Endpoint 17 takes no query.
      expect(sent).toEqual([
        {
          authorization: "Bearer test-token",
          url: `${globalThis.location.origin}${USERS_PATH}`,
          body: typed,
        },
      ]);
      // The program's green on success: the Alert's default variant, not the destructive one.
      expect(messageRows("error")).toHaveLength(0);
      const row = messageRow("info");
      expect(row.classList.contains("text-card-foreground")).toBe(true);
      expect(row.classList.contains("text-destructive")).toBe(false);

      expect(values()).toEqual(["", "", "", "", ""]);
      expect(inputFor(LABELS.password).type).toBe("password");
      await waitFor(() => {
        expect(document.activeElement).toBe(inputFor(LABELS.firstName));
      });
      expectNoNavigation();
      // The password was only request input: the page and the stored session never hold it.
      expect(document.body.textContent).not.toContain(TYPED_PASSWORD);
      expect(sessionStorage.getItem(SESSION_KEY)).not.toContain(TYPED_PASSWORD);
    },
  );

  // audit: COUSR01C-006
  // audit: COUSR01C-025
  // audit: COUSR01-027
  it.each([
    {
      title: "400 duplicate user ID",
      status: 400,
      entry: typed,
      failure: duplicate,
      focus: LABELS.userId,
    },
    {
      title: "400 empty User Type, sent empty rather than checked by the client",
      status: 400,
      entry: typedWithoutType,
      failure: typeEmpty,
      focus: LABELS.userType,
    },
    {
      title: "500 failed insert",
      status: 500,
      entry: typed,
      failure: writeFailure,
      focus: LABELS.firstName,
    },
  ])(
    "a $title shows in the error row, keeps the typed fields and focuses the field it names",
    async ({ status, entry, failure, focus }) => {
      const { sent } = stubFetch({ status, body: failure });
      renderScreen();

      typeEntry(entry);
      fireEvent.click(key("ENTER=Add User"));

      await waitFor(() => {
        expect(messageRow("error").textContent).toBe(failure.message);
      });
      expect(sent.map((request) => request.body)).toEqual([entry]);
      expect(messageRows("info")).toHaveLength(0);
      expect(messageRow("error").classList.contains("text-destructive")).toBe(true);
      // The map is sent again from the fields as typed, the password still masked.
      expect(values()).toEqual(inMapOrder(entry));
      expect(inputFor(LABELS.password).type).toBe("password");
      await waitFor(() => {
        expect(document.activeElement).toBe(inputFor(focus));
      });
      // The field it names is marked invalid with the message, for assistive technology only; the
      // four others are not, and the error row stays the one message row.
      expect(marks()).toEqual(
        Object.values(LABELS).map((label) => (label === focus ? failure.message : null)),
      );
      expectNoNavigation();
    },
  );

  it("a ScreenError that names no field marks no input, with the focus on First Name", async () => {
    const adminOnly: components["schemas"]["ScreenError"] = {
      message: "No access - Admin Only function ...",
    };
    stubFetch({ status: 403, body: adminOnly });
    renderScreen();

    typeEntry(typed);
    fireEvent.click(key("ENTER=Add User"));

    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(adminOnly.message);
    });
    await waitFor(() => {
      expect(document.activeElement).toBe(inputFor(LABELS.firstName));
    });
    expect(marks()).toEqual([null, null, null, null, null]);
  });

  // audit: COUSR01C-005
  it.each([
    { how: "F12=Exit, with the invalid-key message", label: "F12=Exit", message: INVALID_KEY },
    { how: "F4=Clear, with the message row emptied", label: "F4=Clear", message: "" },
  ])("the next key, $how, unmarks the field a server error named", async ({ label, message }) => {
    stubFetch({ status: 400, body: duplicate });
    renderScreen();
    typeEntry(typed);
    fireEvent.click(key("ENTER=Add User"));
    await waitFor(() => {
      expect(markOf(inputFor(LABELS.userId))).toBe(duplicate.message);
    });

    fireEvent.click(key(label));

    expect(messageRow("error").textContent).toBe(message);
    await waitFor(() => {
      expect(marks()).toEqual([null, null, null, null, null]);
    });
  });

  // audit: COUSR01C-005
  it("each key clears the previous message first: ENTER empties the row before its answer arrives", async () => {
    const releases: (() => void)[] = [];
    const gate = new Promise<void>((resolve) => {
      releases.push(resolve);
    });
    const { sent } = stubFetch({ status: 400, body: duplicate, gate });
    renderScreen();

    fireEvent.click(key("F12=Exit"));
    expect(messageRow("error").textContent).toBe(INVALID_KEY);

    typeEntry(typed);
    fireEvent.click(key("ENTER=Add User"));

    // Cleared before the outcome: the request is out, its answer held back, and the row is empty.
    await waitFor(() => {
      expect(sent).toHaveLength(1);
      expect(messageRow("error").textContent).toBe("");
    });
    // A second ENTER while the add is in flight sends nothing, as the locked 3270 keyboard did.
    // Its submit validates in microtasks only, so one timer turn lets it finish while the first
    // answer is still held back; the turn runs inside `act`, which renders the form state the
    // submit updates.
    fireEvent.click(key("ENTER=Add User"));
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(sent).toHaveLength(1);
    for (const release of releases) {
      release();
    }
    await waitFor(() => {
      expect(messageRow("error").textContent).toBe("User ID already exist...");
    });
    expect(sent).toHaveLength(1);
  });

  // audit: COUSR01C-005
  // audit: COUSR01C-008
  // audit: COUSR01-037
  it.each([
    { how: "its button", press: () => fireEvent.click(key("F4=Clear")) },
    { how: "the F4 key", press: () => fireEvent.keyDown(inputFor(LABELS.password), { key: "F4" }) },
  ])(
    "F4 by $how empties the five fields and the message row and focuses First Name, with no request",
    async ({ press }) => {
      const { fetchStub } = stubFetch();
      renderScreen();
      typeEntry(typed);
      fireEvent.click(key("F12=Exit"));
      expect(messageRow("error").textContent).toBe(INVALID_KEY);
      moveFocus(inputFor(LABELS.password));

      press();

      await waitFor(() => {
        expect(values()).toEqual(["", "", "", "", ""]);
      });
      expect(messageRow("error").textContent).toBe("");
      await waitFor(() => {
        expect(document.activeElement).toBe(inputFor(LABELS.firstName));
      });
      expect(fetchStub).not.toHaveBeenCalled();
      expectNoNavigation();
    },
  );

  // audit: COUSR01C-007
  // audit: COUSR01-036
  it.each([
    { how: "its button", press: () => fireEvent.click(key("F3=Back")) },
    { how: "the F3 key", press: () => fireEvent.keyDown(inputFor(LABELS.userId), { key: "F3" }) },
  ])(
    "F3 by $how returns to the admin menu as a first entry, with no request",
    async ({ press }) => {
      const { fetchStub } = stubFetch();
      renderScreen();
      typeEntry(typed);

      press();

      // No route state: the admin menu starts as on its first entry.
      expect(await screen.findByText(`Stub at ${PATHS.COADM01} null`)).toBeTruthy();
      expect(screen.queryByRole("heading", { name: "Add User" })).toBeNull();
      expect(fetchStub).not.toHaveBeenCalled();
    },
  );

  // audit: COUSR01C-009
  // audit: COUSR01-038
  it.each([
    { how: "the F12=Exit button", press: () => fireEvent.click(key("F12=Exit")) },
    {
      how: "the F12 key",
      press: () => fireEvent.keyDown(inputFor(LABELS.password), { key: "F12" }),
    },
    {
      how: "the unlabelled F5 key",
      press: () => fireEvent.keyDown(inputFor(LABELS.password), { key: "F5" }),
    },
  ])(
    "$how shows the invalid-key message, keeps the fields and focuses First Name, with no request or navigation",
    async ({ press }) => {
      const { fetchStub } = stubFetch();
      renderScreen();
      typeEntry(typed);

      press();

      // COUSR01C has no PF12 branch, so `F12=Exit` takes WHEN OTHER: the 50-byte CSMSG01Y text.
      expect(messageRow("error").textContent).toBe(INVALID_KEY);
      expect(messageRow("error").textContent?.trimEnd()).toBe(
        "Invalid key pressed. Please see below...",
      );
      expect(messageRow("error").classList.contains("text-destructive")).toBe(true);
      expect(values()).toEqual(inMapOrder(typed));
      await waitFor(() => {
        expect(document.activeElement).toBe(inputFor(LABELS.firstName));
      });
      expect(fetchStub).not.toHaveBeenCalled();
      expectNoNavigation();
    },
  );
});

// COUSR01C entered with no COMMAREA (`EIBCALEN = 0`) transfers to COSGN00C. The replacement is
// the route guard that only the application's route table mounts in front of `/users/add`, so
// this group renders that table rather than the screen alone, with nobody signed on.
describe("COUSR01 /users/add opened with no session, through the route table", () => {
  beforeEach(() => {
    clearSession();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  // audit: COUSR01C-003
  it("the route guard sends /users/add to /signon before the screen renders, with no request", async () => {
    // This case covers the row's no-session condition through the route table: the real routes
    // and their `RequireSignon` guard, not a stub of either. The guard only hides the screen; the
    // API's admin gate is the control.
    const { fetchStub } = stubFetch();
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, { initialEntries: [PATHS.COUSR01] });
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
    expect(screen.queryByRole("heading", { level: 1, name: "Add User" })).toBeNull();
    for (const label of Object.values(LABELS)) {
      expect(screen.queryByLabelText(label, exactText)).toBeNull();
    }
    expect(screen.queryByRole("button", { name: "ENTER=Add User" })).toBeNull();

    // One timer turn, inside `act`, lets any effect or request the redirect left pending run.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
    });
    expect(fetchStub).not.toHaveBeenCalled();
    router.dispose();
  });
});
