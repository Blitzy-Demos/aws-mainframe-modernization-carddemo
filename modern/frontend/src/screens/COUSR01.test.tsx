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
import { clearSession, getSession, SESSION_KEY, setSession } from "@/auth/session";
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
 * COUSR01C-009: `CCDA-MSG-INVALID-KEY PIC X(50)` (`app/cpy/CSMSG01Y.cpy:20-21`), its 40-character
 * `VALUE` padded with spaces to the 50 bytes of the field.
 */
const INVALID_KEY = `Invalid key pressed. Please see below...${" ".repeat(10)}`;

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

/**
 * COUSR01 at `/users/add`, with a stub at `/admin`, the only route it navigates to. Returns the
 * render result and the query client, whose pending adds {@link clientIdle} reads.
 */
function renderScreen() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const rendered = render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[PATHS.COUSR01]}>
        <Routes>
          <Route path={PATHS.COUSR01} element={<COUSR01 />} />
          <Route path={PATHS.COADM01} element={<RouteStub />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { ...rendered, client };
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

/** Each id an `aria-describedby` of the document names that resolves to no element. */
function danglingDescriptions(): string[] {
  return [...document.querySelectorAll("[aria-describedby]")].flatMap((element) =>
    (element.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .filter((id) => id === "" || document.getElementById(id) === null),
  );
}

/** How assistive technology reads an input: see {@link readOut}. */
type ReadOut = {
  invalid: string | null;
  described: ({ text: string | null; hidden: boolean } | null)[] | null;
};

/**
 * How assistive technology reads `input`: its `aria-invalid`, and the element each id of its
 * `aria-describedby` names, as its text and whether `sr-only` hides it (`null` for an id that
 * resolves to nothing). `described` is `null` when the input has no `aria-describedby`.
 */
function readOut(input: HTMLElement): ReadOut {
  const ids = input.getAttribute("aria-describedby");
  return {
    invalid: input.getAttribute("aria-invalid"),
    described:
      ids === null
        ? null
        : ids.split(" ").map((id) => {
            const target = id === "" ? null : document.getElementById(id);
            return target === null
              ? null
              : { text: target.textContent, hidden: target.classList.contains("sr-only") };
          }),
  };
}

/**
 * How the five inputs, in map order, read when only the input labelled `marked` holds the error
 * `message`: the map's BLUE hints shown and read with User ID, Password and User Type, and the
 * marked input's field message after its hint, hidden.
 */
function readOutWith(marked?: string, message = ""): ReadOut[] {
  const hints: Readonly<Record<string, string>> = {
    [LABELS.userId]: "(8 Char)",
    [LABELS.password]: "(8 Char)",
    [LABELS.userType]: "(A=Admin, U=User)",
  };
  return Object.values(LABELS).map((label) => {
    const hint = hints[label];
    const described = [
      ...(hint === undefined ? [] : [{ text: hint, hidden: false }]),
      ...(label === marked ? [{ text: message, hidden: true }] : []),
    ];
    return {
      invalid: String(label === marked),
      described: described.length > 0 ? described : null,
    };
  });
}

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
 * (COUSR01-002, -003, -005, -007, -008 and -010). Each entry of `fields` is the BMS name, the `POS`
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

/** The row-24 labels in map order (COUSR01-028). */
const KEY_LABELS = ["ENTER=Add User", "F3=Back", "F4=Clear", "F12=Exit"];

/**
 * The keyboard lock the screen shows: the disabled row-24 buttons, the labels of the read-only
 * inputs, and the `aria-busy` of the key row and of the form.
 */
function lockShown(): { disabled: string[]; readOnly: string[]; busy: (string | null)[] } {
  return {
    disabled: KEY_LABELS.filter((label) => key(label).getAttribute("aria-disabled") === "true"),
    readOnly: Object.values(LABELS).filter((label) => inputFor(label).readOnly),
    busy: [
      screen.getByRole("group", { name: "Function keys" }).getAttribute("aria-busy"),
      formOf(inputFor(LABELS.firstName)).getAttribute("aria-busy"),
    ],
  };
}

/** An idle screen: every key acts, every input is editable, nothing is busy. */
const UNLOCKED = { disabled: [], readOnly: [], busy: ["false", "false"] };

/** An add in flight: every key but F3 refused, the five inputs read-only, busy. */
const SENDING_LOCK = {
  disabled: ["ENTER=Add User", "F4=Clear", "F12=Exit"],
  readOnly: Object.values(LABELS),
  busy: ["true", "true"],
};

/** Waits until the screen shows `lock`. */
async function lockReaches(lock: ReturnType<typeof lockShown>): Promise<void> {
  await waitFor(() => {
    expect(lockShown()).toEqual(lock);
  });
}

/** A held-back answer: `gate` settles once `open` is called. */
function heldAnswer(): { readonly gate: Promise<void>; readonly open: () => void } {
  const releases: (() => void)[] = [];
  const gate = new Promise<void>((resolve) => {
    releases.push(resolve);
  });
  return {
    gate,
    open: () => {
      for (const release of releases) {
        release();
      }
    },
  };
}

/**
 * Waits until `client` has no add pending. Endpoint 17 is a mutation of that client: it stays
 * pending from the moment the screen starts it until its answer has arrived and, while the screen
 * is mounted, has been passed to the screen's `onSuccess` or `onError`. Every add the screen had
 * started when this is called has therefore been answered, and its deadline timer in
 * `api/client.ts` cleared, once it returns. It cannot see an add the screen has not started yet,
 * so a test calls it once it has awaited what the screen shows at the end of a key.
 */
async function clientIdle(client: QueryClient): Promise<void> {
  await waitFor(() => {
    expect(client.isMutating()).toBe(0);
  });
}

/**
 * For keys the lock refuses, which leave nothing on screen to wait for: lets 20 ms pass inside
 * `act`, so whatever the keys cause within that window (a validation, a render, a request) runs
 * and renders first. A request started later escapes it, so a check after it shows only that
 * nothing was sent within those 20 ms.
 */
async function boundedWindow(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 20));
  });
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
    // Every input, the header values included, has the one map-field box height (`h-7`, 28px).
    expect(
      Array.from(document.querySelectorAll("input")).filter(
        (box) => !box.classList.contains("h-7") || !box.classList.contains("py-0"),
      ),
    ).toEqual([]);
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
    expect(messageRow("error").textContent).toHaveLength(50);
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
  it("shows the six header values as read-only Inputs in the CardHeader title rows, each at its POS and LENGTH", () => {
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

  // The replacement's accessibility wiring, which no audit row states: every `aria-describedby`
  // names only elements the screen renders. User ID, Password and User Type are described by
  // their BLUE hints; First and Last Name, which have none, by nothing while valid; and the input
  // a `ScreenError` names also by its field message, which holds the error row's text, hidden.
  it.each([
    { state: "on entry", entry: undefined, failure: undefined, marked: undefined },
    {
      state: "after the 400 of a blank First Name",
      entry: { ...typed, first_name: "" },
      failure: { message: "First Name can NOT be empty...", field: "first_name" },
      marked: LABELS.firstName,
    },
    {
      state: "after the 400 of a blank Last Name",
      entry: { ...typed, last_name: "" },
      failure: { message: "Last Name can NOT be empty...", field: "last_name" },
      marked: LABELS.lastName,
    },
    {
      state: "after the 400 of a duplicate User ID",
      entry: typed,
      failure: duplicate,
      marked: LABELS.userId,
    },
    {
      state: "after the 400 of an empty User Type",
      entry: typedWithoutType,
      failure: typeEmpty,
      marked: LABELS.userType,
    },
  ] satisfies {
    state: string;
    entry?: components["schemas"]["UserAddRequest"];
    failure?: components["schemas"]["ScreenError"];
    marked?: string;
  }[])("describes each input by rendered elements only, $state", async (row) => {
    stubFetch(row.failure === undefined ? undefined : { status: 400, body: row.failure });
    renderScreen();
    if (row.entry !== undefined && row.failure !== undefined) {
      const { message } = row.failure;
      typeEntry(row.entry);
      fireEvent.click(key("ENTER=Add User"));
      await waitFor(() => {
        expect(messageRow("error").textContent).toBe(message);
      });
    }

    expect(danglingDescriptions()).toEqual([]);
    expect(inputs().map(readOut)).toEqual(readOutWith(row.marked, row.failure?.message));
  });

  // audit: COUSR01C-005
  it.each([
    {
      how: "F12=Exit, with the invalid-key message",
      label: "F12=Exit",
      message: INVALID_KEY,
      length: 50,
    },
    { how: "F4=Clear, with the message row emptied", label: "F4=Clear", message: "", length: 0 },
  ])(
    "the next key, $how, unmarks the field a server error named",
    async ({ label, message, length }) => {
      stubFetch({ status: 400, body: duplicate });
      renderScreen();
      typeEntry(typed);
      fireEvent.click(key("ENTER=Add User"));
      await waitFor(() => {
        expect(markOf(inputFor(LABELS.userId))).toBe(duplicate.message);
      });

      fireEvent.click(key(label));

      expect(messageRow("error").textContent).toBe(message);
      expect(messageRow("error").textContent).toHaveLength(length);
      await waitFor(() => {
        expect(marks()).toEqual([null, null, null, null, null]);
      });
    },
  );

  // audit: COUSR01C-005
  it("each key clears the previous message first: ENTER empties the row before its answer arrives", async () => {
    const answer = heldAnswer();
    const { sent } = stubFetch({ status: 400, body: duplicate, gate: answer.gate });
    const { client } = renderScreen();
    try {
      fireEvent.click(key("F12=Exit"));
      expect(messageRow("error").textContent).toBe(INVALID_KEY);
      expect(messageRow("error").textContent).toHaveLength(50);

      typeEntry(typed);
      fireEvent.click(key("ENTER=Add User"));

      // Cleared before the outcome: the request is out, its answer held back, and the row is
      // empty.
      await waitFor(() => {
        expect(sent).toHaveLength(1);
        expect(messageRow("error").textContent).toBe("");
      });
      // A second ENTER while the add is in flight sends nothing, as the locked 3270 keyboard did:
      // within the bounded window, while the first answer is still held back.
      fireEvent.click(key("ENTER=Add User"));
      await boundedWindow();
      expect(sent).toHaveLength(1);
      answer.open();
      await waitFor(() => {
        expect(messageRow("error").textContent).toBe("User ID already exist...");
      });
      expect(sent).toHaveLength(1);
    } finally {
      // Pass or fail, the held answer is let go and the test ends once the add has settled, so
      // neither the request nor the client's deadline outlives the test.
      answer.open();
      await clientIdle(client);
    }
  });

  // audit: COUSR01C-006
  // audit: COUSR01C-022
  it("an add in flight locks ENTER, F4, F12 and the inputs and sends one request; its answer unlocks the screen", async () => {
    const answer = heldAnswer();
    const { sent } = stubFetch({ status: 200, body: added, gate: answer.gate });
    const { client } = renderScreen();
    try {
      expect(lockShown()).toEqual(UNLOCKED);

      typeEntry(typed);
      fireEvent.click(key("ENTER=Add User"));
      await waitFor(() => {
        expect(sent).toHaveLength(1);
      });
      await lockReaches(SENDING_LOCK);

      // ENTER by its button, by the Enter key outside the form and by the form's submission; F4
      // and F12 by button and key; an unserved key. None acts within the bounded window, while
      // the answer is held back.
      fireEvent.click(key("ENTER=Add User"));
      fireEvent.keyDown(document.body, { key: "Enter" });
      fireEvent.submit(formOf(inputFor(LABELS.password)));
      fireEvent.click(key("F4=Clear"));
      fireEvent.keyDown(inputFor(LABELS.password), { key: "F4" });
      fireEvent.click(key("F12=Exit"));
      fireEvent.keyDown(inputFor(LABELS.password), { key: "F12" });
      fireEvent.keyDown(inputFor(LABELS.password), { key: "F5" });
      await boundedWindow();
      expect(sent).toHaveLength(1);
      expect(values()).toEqual(inMapOrder(typed));
      expect(messageRow("error").textContent).toBe("");
      expectNoNavigation();

      answer.open();
      await waitFor(() => {
        expect(messageRow("info").textContent).toBe(added.message);
      });
      await lockReaches(UNLOCKED);
      expect(values()).toEqual(["", "", "", "", ""]);

      // Answered, the keys act again: a later ENTER sends a second add.
      typeEntry(typed);
      fireEvent.click(key("ENTER=Add User"));
      await waitFor(() => {
        expect(sent).toHaveLength(2);
      });
      await lockReaches(UNLOCKED);
    } finally {
      // Pass or fail, the held answer is let go and the test ends once every add has settled, so
      // neither a request nor the client's deadline outlives the test.
      answer.open();
      await clientIdle(client);
    }
  });

  // audit: COUSR01C-007
  it("F3 while the add is in flight stays enabled and returns to the admin menu", async () => {
    const answer = heldAnswer();
    const { sent } = stubFetch({ status: 200, body: added, gate: answer.gate });
    const { client } = renderScreen();
    try {
      typeEntry(typed);
      fireEvent.click(key("ENTER=Add User"));
      await lockReaches(SENDING_LOCK);
      fireEvent.click(key("F3=Back"));

      expect(await screen.findByText(`Stub at ${PATHS.COADM01} null`)).toBeTruthy();
      // The answer arrives after F3 left. Once the client has settled the add, it was the one
      // request sent.
      answer.open();
      await clientIdle(client);
      expect(sent).toHaveLength(1);
    } finally {
      // Pass or fail, the held answer is let go and the test ends once the add has settled, so
      // neither the request nor the client's deadline outlives the test.
      answer.open();
      await clientIdle(client);
    }
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
      expect(messageRow("error").textContent).toHaveLength(50);
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
      expect(messageRow("error").textContent).toHaveLength(50);
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

/** The 401 of the `AdminUser` extractor for a missing, invalid or expired token. */
const signonRequired: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};

/**
 * Renders the application's route table at `/users/add`, so the admin guard that only the table
 * mounts stands in front of the screen. Each screen is imported by its route's loader, so a wait
 * for one to render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COUSR01] });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

// The 401 clears the stored session (`api/client.ts`), and the admin guard in front of the route
// replaces the still-mounted screen with sign-on at once: the test presses no key and follows no
// link after ENTER.
describe("COUSR01 in the route table: a 401 ends the session", () => {
  beforeEach(() => {
    setSession("test-token", "ADMIN001", "A");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  it("an add answered 401 replaces the screen with sign-on and clears the session, with no retry", async () => {
    const { fetchStub, sent } = stubFetch({ status: 401, body: signonRequired });
    const router = renderRouteTable();
    expect(
      await screen.findByRole("heading", { level: 1, name: "Add User" }, { timeout: 5_000 }),
    ).toBeDefined();

    typeEntry(typed);
    fireEvent.click(key("ENTER=Add User"));

    expect(
      await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
    ).toBeDefined();
    expect(router.state.location.pathname).toBe(PATHS.COSGN00);
    expect(screen.queryByRole("heading", { level: 1, name: "Add User" })).toBeNull();
    expect(getSession()).toBeNull();
    expect(fetchStub).toHaveBeenCalledTimes(1);
    expect(sent).toEqual([
      {
        authorization: "Bearer test-token",
        url: `${globalThis.location.origin}${USERS_PATH}`,
        body: typed,
      },
    ]);
    router.dispose();
  });
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

    // Only the sign-on screen loads, through the route tree; the refused screen is never imported.
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

    // The refused screen never mounts, so nothing on screen marks the end of a wait: a bounded
    // window of 20 ms, inside `act`, lets whatever follows within it (a render, a request) run
    // first. A request sent later escapes it, so the check shows only that nothing was sent
    // within those 20 ms.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
    });
    expect(fetchStub).not.toHaveBeenCalled();
    router.dispose();
  });
});

/** A scroll offset some earlier screen left, which no arrival may keep. */
const LEFT_TOP = 360;
const LEFT_LEFT = 24;

/** The page's scrolling element in jsdom, which has no `document.scrollingElement`. */
function page(): HTMLElement {
  return document.documentElement;
}

/** Puts the page at `top` and `left`, as an earlier screen or the operator left it. */
function scrollPage(top: number, left: number): void {
  page().scrollTop = top;
  page().scrollLeft = left;
}

/** Spies on the writes to the page's scroll offset, from here on. */
function spyOnScroll() {
  return {
    top: vi.spyOn(page(), "scrollTop", "set"),
    left: vi.spyOn(page(), "scrollLeft", "set"),
  };
}

// Every screen renders one `ScreenShell`, so its arrival is the frame's mount: the page opens at
// its top-left, as a 3270 map replaces the screen before it whole, the browser's own history
// scroll restoration is off, and the document title names the screen by its map title and README
// transaction (WCAG 2.4.2). The frame has no test file of its own, so the 21 routes are one table
// here, each reached through the application's route table, guard and route loader included, with
// the session its gate requires.
describe("Every screen's arrival through the route table: the page opens at its top, titled by the screen", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    sessionStorage.clear();
    scrollPage(0, 0);
  });

  it.each([
    {
      path: PATHS.COSGN00,
      userType: "U",
      title: "This is a Credit Card Demo Application for Mainframe Modernization – CC00 – CardDemo",
    },
    { path: PATHS.COMEN01, userType: "U", title: "Main Menu – CM00 – CardDemo" },
    { path: PATHS.COADM01, userType: "A", title: "Admin Menu – CA00 – CardDemo" },
    { path: PATHS.COACTVW, userType: "U", title: "View Account – CAVW – CardDemo" },
    { path: PATHS.COACTUP, userType: "U", title: "Update Account – CAUP – CardDemo" },
    { path: PATHS.COCRDLI, userType: "U", title: "List Credit Cards – CCLI – CardDemo" },
    { path: PATHS.COCRDSL, userType: "U", title: "View Credit Card Detail – CCDL – CardDemo" },
    { path: PATHS.COCRDUP, userType: "U", title: "Update Credit Card Details – CCUP – CardDemo" },
    { path: PATHS.COTRN00, userType: "U", title: "List Transactions – CT00 – CardDemo" },
    { path: PATHS.COTRN01, userType: "U", title: "View Transaction – CT01 – CardDemo" },
    { path: PATHS.COTRN02, userType: "U", title: "Add Transaction – CT02 – CardDemo" },
    { path: PATHS.CORPT00, userType: "U", title: "Transaction Reports – CR00 – CardDemo" },
    { path: PATHS.COBIL00, userType: "U", title: "Bill Payment – CB00 – CardDemo" },
    { path: PATHS.COUSR00, userType: "A", title: "List Users – CU00 – CardDemo" },
    { path: PATHS.COUSR01, userType: "A", title: "Add User – CU01 – CardDemo" },
    { path: PATHS.COUSR02, userType: "A", title: "Update User – CU02 – CardDemo" },
    { path: PATHS.COUSR03, userType: "A", title: "Delete User – CU03 – CardDemo" },
    { path: PATHS.COPAU00, userType: "U", title: "View Authorizations – CPVS – CardDemo" },
    // With no route state COPAU01 goes on to COPAU00; an empty selection keeps it on its own map.
    {
      path: PATHS.COPAU01,
      userType: "U",
      state: { acct_id: "", auth_key: "" },
      title: "View Authorization Details – CPVD – CardDemo",
    },
    { path: PATHS.COTRTLI, userType: "A", title: "Maintain Transaction Type – CTLI – CardDemo" },
    { path: PATHS.COTRTUP, userType: "A", title: "Maintain Transaction Type – CTTU – CardDemo" },
  ])(
    "$path opens at the top-left of the page, titled $title",
    async ({ path, userType, state, title }) => {
      stubFetch();
      document.title = "CardDemo";
      setSession("test-token", userType === "A" ? "ADMIN001" : "USER0001", userType);
      scrollPage(LEFT_TOP, LEFT_LEFT);
      window.history.scrollRestoration = "auto";
      const writes = spyOnScroll();
      const client = new QueryClient({
        defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
      });
      const router = createMemoryRouter(routes, { initialEntries: [{ pathname: path, state }] });
      render(
        <QueryClientProvider client={client}>
          <RouterProvider router={router} />
        </QueryClientProvider>,
      );

      expect(await screen.findByRole("heading", { level: 1 }, { timeout: 5_000 })).toBeTruthy();
      expect(router.state.location.pathname).toBe(path);
      expect(writes.top.mock.calls).toEqual([[0]]);
      expect(writes.left.mock.calls).toEqual([[0]]);
      expect([page().scrollTop, page().scrollLeft]).toEqual([0, 0]);
      expect(window.history.scrollRestoration).toBe("manual");
      expect(document.title).toBe(title);
      router.dispose();
    },
  );
});

// The reset is the arrival's alone: a key the operator presses and the answer the screen then
// shows leave the page where the operator scrolled it. The title lasts as long as the screen: the
// title it replaced returns when the screen is left for a view that renders no frame.
describe("COUSR01 after its arrival: the scroll offset stays the operator's, the title the screen's", () => {
  beforeEach(() => {
    setSession("test-token", "ADMIN001", "A");
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    sessionStorage.clear();
    scrollPage(0, 0);
  });

  it("an add and its answer move no scroll offset", async () => {
    const { sent } = stubFetch({ status: 200, body: added });
    renderScreen();
    scrollPage(LEFT_TOP, LEFT_LEFT);
    const writes = spyOnScroll();

    typeEntry(typed);
    fireEvent.click(key("ENTER=Add User"));

    await waitFor(() => {
      expect(messageRow("info").textContent).toBe("User NEWUSR01 has been added ...");
    });
    await waitFor(() => {
      expect(document.activeElement).toBe(inputFor(LABELS.firstName));
    });
    expect(sent).toHaveLength(1);
    expect(writes.top).not.toHaveBeenCalled();
    expect(writes.left).not.toHaveBeenCalled();
    expect([page().scrollTop, page().scrollLeft]).toEqual([LEFT_TOP, LEFT_LEFT]);
    expect(document.title).toBe("Add User – CU01 – CardDemo");
  });

  it("F3 to a view with no frame gives back the title the screen replaced", async () => {
    stubFetch();
    document.title = "CardDemo";
    renderScreen();
    expect(document.title).toBe("Add User – CU01 – CardDemo");

    fireEvent.click(key("F3=Back"));

    expect(await screen.findByText(`Stub at ${PATHS.COADM01} null`)).toBeTruthy();
    expect(document.title).toBe("CardDemo");
  });
});
