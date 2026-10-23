import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { Suspense, use } from "react";
import {
  createMemoryRouter,
  MemoryRouter,
  Route,
  RouterProvider,
  Routes,
  useLocation,
  useNavigate,
} from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { components } from "@/api/schema";
import { getSession, SESSION_KEY, setSession, type StoredSession } from "@/auth/session";
import { PATHS, PROGRAM_ROUTES, routes } from "@/routes";
import { COADM01 } from "./COADM01";

/**
 * Render test of the CA00 admin menu, map `COADM1A` of `app/bms/COADM01.bms`, program COADM01C,
 * route `/admin`, in jsdom. The global `fetch` is a stub that answers endpoint 3
 * (`GET /api/v1/menus/admin`) in process and any other request with 500, so no request leaves the
 * test. Expected texts, routes and header values are the audited ones of
 * `modern/audit/online/COADM01C.md` and the `COADM01-*` rows of `modern/audit/screens.md`.
 *
 * ENTER and every key are served from the response the screen already holds, so each test also
 * counts the requests: one per entry to `/admin`.
 *
 * Rows `COADM01-001` to `COADM01-010` place the header labels in a `Label` and the header values in
 * a read-only `Input`, as the shared `ScreenShell` renders them. The label rows are tagged in NAV.
 * The value rows are tagged here, on each value's read-only `Input`, its width, its map `POS` and
 * `LENGTH`, its label and its content, as are their content rows (`COADM01-029` to `COADM01-034`).
 * `COADM01C-002` is blocked by halt candidate `X-COADM01C-H1`: its group is reported BLOCKED and
 * asserts only the replacement's path the row states, never a COBOL result.
 */

const MENU_PATH = "/api/v1/menus/admin";

/** `'Please enter a valid option number...'` (COADM01C-014, COADM01C-016). */
const INVALID_OPTION = "Please enter a valid option number...";

/** `'This option is not installed ...'` (COADM01C-014, COADM01C-018). */
const NOT_INSTALLED = "This option is not installed ...";

/** `CCDA-MSG-INVALID-KEY PIC X(50)` of `CSMSG01Y`, all 50 bytes, trailing spaces included. */
const INVALID_KEY_TEXT = "Invalid key pressed. Please see below...".padEnd(50, " ");

/** The six option programs of `COADM02Y`, in table order. */
const OPTION_PROGRAMS = ["COUSR00C", "COUSR01C", "COUSR02C", "COUSR03C", "COTRTLIC", "COTRTUPC"];

/**
 * Endpoint 3's body for the six options of `app/cpy/COADM02Y.cpy:26-53`. Each name is the 35
 * bytes as declared, with no user type, and each row text is the 40 bytes `BUILD-MENU-OPTIONS`
 * strings: the two digits, `. `, the 35-byte name and one space. Option 6 is stubbed as not
 * installed, the case of COADM01C-018 that no shipped option has.
 */
const menu: components["schemas"]["MenuResponse"] = {
  invalid_option_message: INVALID_OPTION,
  options: [
    {
      admin_only: true,
      installed: true,
      name: "User List (Security)".padEnd(35, " "),
      number: "01",
      program: "COUSR00C",
      text: "01. User List (Security)".padEnd(40, " "),
      unavailable_message: NOT_INSTALLED,
    },
    {
      admin_only: true,
      installed: true,
      name: "User Add (Security)".padEnd(35, " "),
      number: "02",
      program: "COUSR01C",
      text: "02. User Add (Security)".padEnd(40, " "),
      unavailable_message: NOT_INSTALLED,
    },
    {
      admin_only: true,
      installed: true,
      name: "User Update (Security)".padEnd(35, " "),
      number: "03",
      program: "COUSR02C",
      text: "03. User Update (Security)".padEnd(40, " "),
      unavailable_message: NOT_INSTALLED,
    },
    {
      admin_only: true,
      installed: true,
      name: "User Delete (Security)".padEnd(35, " "),
      number: "04",
      program: "COUSR03C",
      text: "04. User Delete (Security)".padEnd(40, " "),
      unavailable_message: NOT_INSTALLED,
    },
    {
      admin_only: true,
      installed: true,
      name: "Transaction Type List/Update (Db2)".padEnd(35, " "),
      number: "05",
      program: "COTRTLIC",
      text: "05. Transaction Type List/Update (Db2)".padEnd(40, " "),
      unavailable_message: NOT_INSTALLED,
    },
    {
      admin_only: true,
      installed: false,
      name: "Transaction Type Maintenance (Db2)".padEnd(35, " "),
      number: "06",
      program: "COTRTUPC",
      text: "06. Transaction Type Maintenance (Db2)".padEnd(40, " "),
      unavailable_message: NOT_INSTALLED,
    },
  ],
};

/** The menu as the shipped server sends it, every option installed (COADM01C-013). */
const served: components["schemas"]["MenuResponse"] = {
  ...menu,
  options: menu.options.map((option) => ({ ...option, installed: true })),
};

/** The 403 the admin gate answers to a token whose user type is not `A` (COADM01C-001). */
const forbidden: components["schemas"]["ScreenError"] = {
  message: "No access - Admin Only function ...",
};

/** The dev proxy's 502 while the API cannot be reached, shaped as the API's `ScreenError`. */
const unreachable: components["schemas"]["ScreenError"] = {
  message: "The dev proxy cannot reach the API at CARDDEMO_API_BIND",
};

/** Every request the screen sent in the current test, in order. */
let requests: Request[] = [];

/** A JSON reply, as the API sends every body. */
function reply(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces the global `fetch`: `GET` endpoint 3 answers `status` with `body`, and any other request
 * answers 500. No test expects another request, except the main menu's load after the admin guard
 * sends a non-admin there. The request at position `n` of the test (from 0) is answered only once
 * `holds[n]`, when there is one, settles.
 */
function stubMenu(
  status: number,
  body: components["schemas"]["MenuResponse"] | components["schemas"]["ScreenError"],
  holds: readonly Promise<void>[] = [],
): void {
  stubMenuReplies([{ status, body }], holds);
}

/** One answer of endpoint 3: the HTTP status and the body. */
type MenuReply = {
  readonly status: number;
  readonly body: components["schemas"]["MenuResponse"] | components["schemas"]["ScreenError"];
};

/**
 * {@link stubMenu} with an answer per request: the request at position `n` of the test (from 0)
 * gets `replies[n]`, and every request past the last reply gets the last one again.
 */
function stubMenuReplies(
  replies: readonly [MenuReply, ...MenuReply[]],
  holds: readonly Promise<void>[] = [],
): void {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init);
      const position = requests.length;
      requests.push(request);
      if (request.method === "GET" && new URL(request.url).pathname === MENU_PATH) {
        await holds.at(position);
        const { status, body } = replies[Math.min(position, replies.length - 1)];
        return reply(status, body);
      }
      const unexpected: components["schemas"]["ScreenError"] = {
        message: `Unexpected request ${request.method} ${request.url}`,
      };
      return reply(500, unexpected);
    }),
  );
}

/**
 * The screen a transfer reaches: it shows the path and the route state it was given, and its
 * button returns to `/admin` as each option screen's F3 or F12 does.
 */
function Target() {
  const location = useLocation();
  const navigate = useNavigate();
  return (
    <section aria-label="Transfer target">
      <p data-testid="target">{`${location.pathname} ${JSON.stringify(location.state)}`}</p>
      <button
        type="button"
        onClick={() => {
          void navigate(PATHS.COADM01);
        }}
      >
        Return to admin menu
      </button>
    </section>
  );
}

/** A transfer target that suspends until `opened` settles, as a route waits on its loader. */
function OpensWhen({ opened }: { readonly opened: Promise<void> }) {
  use(opened);
  return <Target />;
}

/** Renders `/admin` under a fresh query client that never retries, with every transfer target. */
function renderAdminMenu() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const targets = [PATHS.COSGN00, ...OPTION_PROGRAMS.map((program) => PROGRAM_ROUTES[program])];
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[PATHS.COADM01]}>
        <Routes>
          <Route path={PATHS.COADM01} element={<COADM01 />} />
          {targets.map((path) => (
            <Route key={path} path={path} element={<Target />} />
          ))}
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** Waits until the six option rows of the response are shown. */
async function loaded(): Promise<void> {
  await waitFor(() => {
    expect(screen.getAllByRole("row")).toHaveLength(6);
  });
}

/** The option input, found through its label (COADM01-024). */
function optionInput(): HTMLInputElement {
  const input = screen.getByLabelText("Please select an option :");
  if (!(input instanceof HTMLInputElement)) {
    throw new TypeError("the option field is not an input element");
  }
  return input;
}

/** The element of a map field, by the `data-field` name the screen gives it. */
function field(name: string): HTMLElement {
  const element = document.querySelector(`[data-field="${name}"]`);
  if (!(element instanceof HTMLElement)) {
    throw new TypeError(`no element carries data-field="${name}"`);
  }
  return element;
}

/**
 * The header field `name` of rows 1 and 2 (`data-field`): the one `Input` of that name in the
 * `CardHeader` title rows of `ScreenShell`, failing the test when there is not exactly one.
 */
function headerInput(name: string): HTMLInputElement {
  const fields = document.querySelectorAll(
    `[data-slot="card-header"] input[data-field="${name}"]`,
  );
  expect(fields).toHaveLength(1);
  const [element] = fields;
  if (!(element instanceof HTMLInputElement)) {
    throw new TypeError(`the header has no input ${name}`);
  }
  return element;
}

/** The message row of `kind`, or `undefined` when the row shows the other kind. */
function messageRow(kind: "error" | "info"): HTMLElement | undefined {
  return screen.getAllByRole("alert").find((row) => row.dataset.messageKind === kind);
}

/** Types `entry` into the option input and clicks `ENTER=Continue`. */
function enter(entry: string): void {
  fireEvent.change(optionInput(), { target: { value: entry } });
  fireEvent.click(screen.getByRole("button", { name: "ENTER=Continue" }));
}

/**
 * The cursor is on the option input with all of its text selected, so the next option overtypes
 * the value shown from the first position.
 */
function expectOptionSelected(): void {
  const input = optionInput();
  expect(document.activeElement).toBe(input);
  expect(input.selectionStart).toBe(0);
  expect(input.selectionEnd).toBe(input.value.length);
}

/**
 * Types `key` into `input` as a browser does: the key replaces the selected text, or goes in at
 * the caret, and a value longer than `maxLength` is refused, leaving the input as it was.
 */
function typeKey(input: HTMLInputElement, key: string): void {
  const start = input.selectionStart ?? input.value.length;
  const end = input.selectionEnd ?? start;
  const next = `${input.value.slice(0, start)}${key}${input.value.slice(end)}`;
  if (input.maxLength < 0 || next.length <= input.maxLength) {
    fireEvent.change(input, { target: { value: next } });
  }
}

/**
 * The screen sent exactly `count` requests, each `GET` endpoint 3, and none carries `page_size`
 * (Bounded Pagination: the menu is a single resource, not a collection).
 */
async function expectMenuRequests(count: number): Promise<void> {
  expect(requests).toHaveLength(count);
  for (const request of requests) {
    expect(request.method).toBe("GET");
    expect(new URL(request.url).pathname).toBe(MENU_PATH);
    expect(request.url).not.toContain("page_size");
    expect(await request.clone().text()).not.toContain("page_size");
  }
}

/** Waits for the transfer target, then returns what it shows: its path and its route state. */
async function transferTarget(): Promise<string | null> {
  return (await screen.findByTestId("target")).textContent;
}

/** The menu is still shown: no transfer happened. */
function expectNoTransfer(): void {
  expect(screen.getByRole("heading", { level: 1, name: "Admin Menu" })).toBeDefined();
  expect(screen.queryByTestId("target")).toBeNull();
}

/** A promise the test settles by calling `release`, to hold a reply back. */
function deferred(): { readonly done: Promise<void>; readonly release: () => void } {
  let release = (): void => undefined;
  const done = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { done, release: () => release() };
}

/** The `ENTER=Continue` button (COADM01-035). */
function enterButton(): HTMLElement {
  return screen.getByRole("button", { name: "ENTER=Continue" });
}

/** The menu's `<form>`, which holds the option input. */
function menuForm(): HTMLFormElement {
  const form = optionInput().form;
  if (form === null) {
    throw new TypeError("the option field is not in a form");
  }
  return form;
}

/**
 * Whether the screen shows the locked keyboard of a load in progress: ENTER disabled, the key row
 * and the form busy, the option input read-only; or, with `busy` false, all of them free. F3 is
 * free either way.
 */
function expectBusy(busy: boolean): void {
  expect(enterButton().getAttribute("aria-disabled")).toBe(busy ? "true" : null);
  expect(screen.getByRole("button", { name: "F3=Exit" }).getAttribute("aria-disabled")).toBeNull();
  expect(screen.getByRole("group", { name: "Function keys" }).getAttribute("aria-busy")).toBe(
    String(busy),
  );
  expect(menuForm().getAttribute("aria-busy")).toBe(String(busy));
  expect(optionInput().readOnly).toBe(busy);
}

/**
 * Gives a refused key the time its submit handler would take to decide and render. The wait runs
 * inside `act`, as the form's own state updates after an ENTER do.
 */
async function lingers(): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => {
      setTimeout(resolve, 50);
    });
  });
}

/** Types `entry`, clicks `ENTER=Continue` and waits until the `kind` row shows `text`. */
async function enterUntil(entry: string, kind: "error" | "info", text: string): Promise<void> {
  enter(entry);
  await waitFor(() => {
    expect(messageRow(kind)?.textContent).toBe(text);
  });
}

/** Each id an `aria-describedby` of the document names that resolves to no element. */
function danglingDescriptions(): string[] {
  return [...document.querySelectorAll("[aria-describedby]")].flatMap((element) =>
    (element.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .filter((id) => id === "" || document.getElementById(id) === null),
  );
}

/**
 * How assistive technology reads the input: its `aria-invalid`, and the element each id of its
 * `aria-describedby` names, as its text and whether `sr-only` hides it (`null` for an id that
 * resolves to nothing). `described` is `null` when the input has no `aria-describedby`.
 */
function readOut(element: HTMLInputElement): {
  invalid: string | null;
  described: ({ text: string | null; hidden: boolean } | null)[] | null;
} {
  const ids = element.getAttribute("aria-describedby");
  return {
    invalid: element.getAttribute("aria-invalid"),
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

/** Every element that shows exactly `text` and is not hidden by `sr-only`. */
function visibleCopies(text: string): HTMLElement[] {
  return screen
    .queryAllByText(text, { normalizer: (content) => content })
    .filter((element) => element.closest(".sr-only") === null);
}

/** The option input unmarked: valid, and described by nothing, as it has no hint. */
const UNMARKED = { invalid: "false", described: null };

/** The option input marked by `message`: invalid, described by its hidden field message. */
function markedBy(message: string): {
  invalid: string;
  described: { text: string; hidden: boolean }[];
} {
  return { invalid: "true", described: [{ text: message, hidden: true }] };
}


describe("COADM01 admin menu (CA00, COADM01C, /admin)", () => {
  beforeEach(() => {
    requests = [];
    setSession("test-token", "ADMIN001", "A");
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  // audit: COADM01C-003
  // audit: COADM01-011
  // audit: COADM01-012
  // audit: COADM01-013
  // audit: COADM01-014
  // audit: COADM01-015
  // audit: COADM01-016
  // audit: COADM01-017
  // audit: COADM01-018
  // audit: COADM01-019
  // audit: COADM01-020
  // audit: COADM01-021
  // audit: COADM01-022
  // audit: COADM01-023
  // audit: COADM01-024
  // audit: COADM01-025
  // audit: COADM01-026
  // audit: COADM01-027
  // audit: COADM01-028
  // audit: COADM01-035
  // audit: COADM01-036
  it("first entry requests endpoint 3 once and shows the options, the field, an empty message row and the keys", async () => {
    stubMenu(200, menu);
    renderAdminMenu();
    await loaded();

    await expectMenuRequests(1);
    expect(requests[0].headers.get("Authorization")).toBe("Bearer test-token");

    expect(screen.getByRole("heading", { level: 1, name: "Admin Menu" })).toBeDefined();

    // Lines 6 to 11 show the six options in table order, exactly as the response words them.
    const table = screen.getByRole("table", { name: "Admin Menu options" });
    const shown = within(table).getAllByRole("row");
    expect(shown.map((row) => row.textContent)).toEqual(menu.options.map((option) => option.text));
    // Lines 12 to 17 keep the INITIAL of one space and are hidden from assistive technology.
    const lines = within(table).getAllByRole("row", { hidden: true });
    expect(lines).toHaveLength(12);
    lines.forEach((line, index) => {
      const name = `OPTN${String(index + 1).padStart(3, "0")}`;
      expect(within(line).getByRole("cell", { hidden: true }).dataset.field).toBe(name);
      if (index >= 6) {
        expect(field(name).textContent).toBe(" ");
        expect(line.getAttribute("aria-hidden")).toBe("true");
      }
    });

    // OPTION: LENGTH=2, NUM, IC; it starts empty, with the cursor, and its FormLabel carries the
    // map's INITIAL.
    const input = optionInput();
    const label = document.querySelector(`label[for="${input.id}"]`);
    expect(label?.textContent).toBe("Please select an option :");
    expect(label?.getAttribute("data-slot")).toBe("form-label");
    expect(input.dataset.field).toBe("OPTION");
    expect(input.maxLength).toBe(2);
    expect(input.getAttribute("inputmode")).toBe("numeric");
    expect(input.value).toBe("");
    expect(document.activeElement).toBe(input);

    // Line 20 holds the label at column 15 and the input at column 41, and no element for the
    // zero-length stopper at column 44.
    const form = input.closest("form");
    expect(form).not.toBeNull();
    const line20 = Array.from(form?.querySelectorAll('[class*="row-start-20 "]') ?? []);
    expect(line20.map((cell) => /col-start-([0-9]+)/.exec(cell.className)?.[1])).toEqual([
      "15",
      "41",
    ]);
    expect(form?.querySelector(".col-start-44")).toBeNull();

    // ERRMSG: an error row, polite live region, empty on first entry.
    const row = messageRow("error");
    expect(row?.textContent).toBe("");
    expect(row?.getAttribute("aria-live")).toBe("polite");
    expect(row?.classList.contains("text-destructive")).toBe(true);
    expect(messageRow("info")).toBeUndefined();

    // Row 24: `ENTER=Continue  F3=Exit`, in map order.
    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(keys.map((key) => key.textContent)).toEqual(["ENTER=Continue", "F3=Exit"]);
    const [enterKey, exitKey] = keys;
    // The submit button of the option's form, so the Enter key typed in the field submits it too.
    expect(enterKey.getAttribute("type")).toBe("submit");
    expect(enterKey.getAttribute("form")).toBe(form?.id);
    expect(enterKey.dataset.variant).toBe("default");
    expect(exitKey.getAttribute("type")).toBe("button");
    expect(exitKey.dataset.variant).toBe("outline");
    expect(exitKey.dataset.size).toBe("sm");
  });

  // audit: COADM01C-022
  // audit: COADM01-029
  // audit: COADM01-030
  // audit: COADM01-031
  // audit: COADM01-032
  // audit: COADM01-033
  // audit: COADM01-034
  it("fills header rows 1 and 2 on every send: CA00, COADM01C, the titles, the date and time", async () => {
    stubMenu(200, menu);
    renderAdminMenu();
    await loaded();

    const expectHeader = () => {
      expect(headerInput("TRNNAME").value).toBe("CA00");
      expect(headerInput("PGMNAME").value).toBe("COADM01C");
      expect(headerInput("TITLE01").value).toBe("      AWS Mainframe Modernization       ");
      expect(headerInput("TITLE02").value).toBe("              CardDemo                  ");
      // The date and time are the current instant, so only their formats are compared.
      expect(headerInput("CURDATE").value).toMatch(/^[0-9]{2}\/[0-9]{2}\/[0-9]{2}$/);
      expect(headerInput("CURTIME").value).toMatch(/^[0-9]{2}:[0-9]{2}:[0-9]{2}$/);
    };
    expectHeader();

    enter("99");
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(INVALID_OPTION);
    });
    expectHeader();

    enter("6");
    await waitFor(() => {
      expect(messageRow("info")?.textContent).toBe(NOT_INSTALLED);
    });
    expectHeader();

    fireEvent.keyDown(optionInput(), { key: "F5" });
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(INVALID_KEY_TEXT);
    });
    expectHeader();
    await expectMenuRequests(1);
  });

  // audit: COADM01C-022
  it("stamps the time of every send in the header and shows its text anew, a repeated message included", async () => {
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
      stubMenu(200, menu);
      renderAdminMenu();
      await loaded();
      expect(headerInput("CURTIME").value).toBe("03:04:05");
      const row = messageRow("error");
      let shown = row?.querySelector('[data-slot="alert-description"]');

      // Each send repeats the text of the one before: two ENTERs of option 99, then two F5s.
      const sends = [
        { second: 8, time: "03:04:08", text: INVALID_OPTION, key: "Enter" },
        { second: 11, time: "03:04:11", text: INVALID_OPTION, key: "Enter" },
        { second: 14, time: "03:04:14", text: INVALID_KEY_TEXT, key: "F5" },
        { second: 17, time: "03:04:17", text: INVALID_KEY_TEXT, key: "F5" },
      ];
      for (const { second, time, text, key } of sends) {
        vi.setSystemTime(new Date(2024, 0, 2, 3, 4, second));
        if (key === "Enter") {
          enter("99");
        } else {
          fireEvent.keyDown(optionInput(), { key });
        }
        await waitFor(() => {
          expect(headerInput("CURTIME").value).toBe(time);
        });
        // The live region stays; the text inside it is a new element, which it announces.
        expect(messageRow("error")).toBe(row);
        expect(row?.textContent).toBe(text);
        const replaced = row?.querySelector('[data-slot="alert-description"]');
        expect(replaced).toBeInstanceOf(HTMLElement);
        expect(replaced).not.toBe(shown);
        shown = replaced;
      }
      expectNoTransfer();
      await expectMenuRequests(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it.each([
    // audit: COADM01-002
    {
      id: "COADM01-002",
      name: "TRNNAME",
      row: 1,
      col: 7,
      length: 4,
      label: "Tran:",
      shows: "CA00",
    },
    // audit: COADM01-003
    {
      id: "COADM01-003",
      name: "TITLE01",
      row: 1,
      col: 21,
      length: 40,
      label: null,
      shows: "      AWS Mainframe Modernization       ",
    },
    // audit: COADM01-005
    {
      id: "COADM01-005",
      name: "CURDATE",
      row: 1,
      col: 71,
      length: 8,
      label: "Date:",
      shows: expect.stringMatching(/^[0-9]{2}\/[0-9]{2}\/[0-9]{2}$/),
    },
    // audit: COADM01-007
    {
      id: "COADM01-007",
      name: "PGMNAME",
      row: 2,
      col: 7,
      length: 8,
      label: "Prog:",
      shows: "COADM01C",
    },
    // audit: COADM01-008
    {
      id: "COADM01-008",
      name: "TITLE02",
      row: 2,
      col: 21,
      length: 40,
      label: null,
      shows: "              CardDemo                  ",
    },
    // audit: COADM01-010
    {
      id: "COADM01-010",
      name: "CURTIME",
      row: 2,
      col: 71,
      length: 8,
      label: "Time:",
      shows: expect.stringMatching(/^[0-9]{2}:[0-9]{2}:[0-9]{2}$/),
    },
  ])(
    "$id shows $name in a read-only Input at ($row,$col), $length wide, in the card header",
    async ({ name, row, col, length, label, shows }) => {
      // The header value is an output field of the map: a read-only `Input` that holds `LENGTH`
      // characters (`size`), in the cell of its `POS` and `LENGTH` in the frame's title rows. The
      // map's literal before it is its bound `Label`; a title, which no literal labels, is named
      // by `aria-label`.
      stubMenu(200, menu);
      renderAdminMenu();
      await loaded();

      const value = headerInput(name);
      expect(value.dataset.slot).toBe("input");
      expect(value.readOnly).toBe(true);
      expect(value.size).toBe(length);
      expect(value.parentElement?.className).toBe(
        `row-start-${row} col-start-${col} col-span-${length}`,
      );
      expect(value.value).toEqual(shows);
      const labels = Array.from(value.labels ?? []);
      expect(labels.map((bound) => bound.textContent)).toEqual(label === null ? [] : [label]);
      expect(labels.every((bound) => bound.dataset.slot === "label")).toBe(true);
      expect(screen.getByRole("textbox", { name: label ?? `Title row ${row}` })).toBe(value);
      await expectMenuRequests(1);
    },
  );

  it("gives the option input the shared text-sm field box of its 2 characters", async () => {
    stubMenu(200, menu);
    renderAdminMenu();
    await loaded();

    expect(optionInput().classList.contains("field-length-2")).toBe(true);
    expect(optionInput().classList.contains("text-sm")).toBe(true);
    expect(optionInput().classList.contains("text-base")).toBe(false);
  });

  // audit: COADM01C-015
  // audit: COADM01C-017
  // audit: COADM01-025
  // audit: COADM01-035
  it.each([
    { entry: "1", program: "COUSR00C", route: "/users" },
    { entry: "01", program: "COUSR00C", route: "/users" },
    { entry: "02", program: "COUSR01C", route: "/users/add" },
    { entry: "3", program: "COUSR02C", route: "/users/update" },
    { entry: "04", program: "COUSR03C", route: "/users/delete" },
    { entry: "5", program: "COTRTLIC", route: "/transaction-types" },
    { entry: "6", program: "COTRTUPC", route: "/transaction-types/maintain" },
  ])(
    "ENTER $entry transfers to $route ($program) without a request",
    async ({ entry, program, route }) => {
      stubMenu(200, served);
      renderAdminMenu();
      await loaded();

      enter(entry);

      // The route state names only the menu the target returns to: no selection, no identity.
      expect(await transferTarget()).toBe(`${route} {"from":"/admin"}`);
      expect(PROGRAM_ROUTES[program]).toBe(route);
      await expectMenuRequests(1);
    },
  );

  // audit: COADM01C-015
  // audit: COADM01C-016
  // audit: COADM01-025
  // audit: COADM01-027
  it.each([
    { entry: "99", shown: "99" },
    { entry: "7", shown: "07" },
    { entry: "07", shown: "07" },
    { entry: "0", shown: "00" },
    { entry: "00", shown: "00" },
  ])(
    "ENTER $entry shows the invalid-option error, keeps the menu and shows $shown with the cursor",
    async ({ entry, shown }) => {
      stubMenu(200, menu);
      renderAdminMenu();
      await loaded();
      // A click leaves jsdom's focus where it was, so the cursor's return (`IC`) is observed as
      // the screen's call to focus the input.
      const focus = vi.spyOn(optionInput(), "focus");

      enter(entry);

      await waitFor(() => {
        expect(messageRow("error")?.textContent).toBe(INVALID_OPTION);
      });
      expect(messageRow("error")?.classList.contains("text-destructive")).toBe(true);
      await waitFor(() => {
        expect(optionInput().value).toBe(shown);
        expect(focus).toHaveBeenCalled();
        expectOptionSelected();
      });
      expectNoTransfer();
      await expectMenuRequests(1);
    },
  );

  // What the input shows after an empty entry or one that holds `-` or `.` is X-COADM01C-H2, so
  // only the message is asserted for these entries.
  // audit: COADM01C-016
  it.each([{ entry: "" }, { entry: "-" }, { entry: "." }, { entry: "1." }])(
    "ENTER with the entry '$entry' shows the invalid-option error and keeps the menu",
    async ({ entry }) => {
      stubMenu(200, menu);
      renderAdminMenu();
      await loaded();

      enter(entry);

      await waitFor(() => {
        expect(messageRow("error")?.textContent).toBe(INVALID_OPTION);
      });
      expectNoTransfer();
      await expectMenuRequests(1);
    },
  );

  // The screen's not-installed outcome covers both COBOL causes: an installed flag of `false`
  // (COADM01C-018) and a program no screen serves, the counterpart of PGMIDERR (COADM01C-019). In
  // neither case does the screen ignore the next key, unlike the COBOL after PGMIDERR.
  // audit: COADM01C-018
  // audit: COADM01C-019
  it.each([
    { entry: "6", number: "06", program: "COTRTUPC", why: "its installed flag is false" },
    { entry: "5", number: "05", program: "COBTUPDT", why: "no screen serves COBTUPDT" },
    { entry: "05", number: "05", program: "constructor", why: "no screen serves constructor" },
  ])(
    "ENTER $entry shows not-installed as information when $why, and serves the next ENTER",
    async ({ entry, number, program }) => {
      const body: components["schemas"]["MenuResponse"] = {
        ...menu,
        options: menu.options.map((option) =>
          option.number === number ? { ...option, program } : option,
        ),
      };
      stubMenu(200, body);
      renderAdminMenu();
      await loaded();

      enter(entry);

      await waitFor(() => {
        expect(messageRow("info")?.textContent).toBe(NOT_INSTALLED);
      });
      const row = messageRow("info");
      expect(row?.classList.contains("text-card-foreground")).toBe(true);
      expect(row?.classList.contains("text-destructive")).toBe(false);
      expect(messageRow("error")).toBeUndefined();
      await waitFor(() => {
        expect(optionInput().value).toBe(number);
      });
      expectNoTransfer();

      enter("01");

      expect(await transferTarget()).toBe(`/users {"from":"/admin"}`);
      await expectMenuRequests(1);
    },
  );

  // audit: COADM01C-020
  // audit: COADM01-036
  it.each([
    {
      how: "the F3=Exit button",
      press: () => {
        fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));
      },
    },
    {
      how: "the F3 key",
      press: () => {
        fireEvent.keyDown(optionInput(), { key: "F3" });
      },
    },
  ])("$how goes to sign-on without examining the entry or sending a request", async ({ press }) => {
    stubMenu(200, menu);
    renderAdminMenu();
    await loaded();
    fireEvent.change(optionInput(), { target: { value: "99" } });

    press();

    // No route state: sign-on is entered without a COMMAREA.
    expect(await transferTarget()).toBe(`${PATHS.COSGN00} null`);
    expect(PATHS.COSGN00).toBe("/signon");
    await expectMenuRequests(1);
  });

  // audit: COADM01C-020
  // audit: COADM01-036
  it("F3 still goes to sign-on while endpoint 3 loads, and an unserved key is refused", async () => {
    const load = deferred();
    stubMenu(200, served, [load.done]);
    renderAdminMenu();
    expectBusy(true);

    // The locked keyboard takes the unserved key without the invalid-key text.
    fireEvent.keyDown(optionInput(), { key: "F5" });
    await lingers();
    expect(messageRow("error")?.textContent).toBe("");

    fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));
    expect(await transferTarget()).toBe(`${PATHS.COSGN00} null`);
    load.release();
    await expectMenuRequests(1);
  });

  // audit: COADM01-035
  it.each([
    {
      how: "a click on ENTER=Continue",
      press: () => {
        fireEvent.click(enterButton());
      },
    },
    {
      how: "the Enter key",
      press: () => {
        fireEvent.keyDown(window, { key: "Enter" });
      },
    },
    {
      how: "a submit of the form",
      press: () => {
        fireEvent.submit(menuForm());
      },
    },
  ])("$how decides nothing while endpoint 3 loads or reloads", async ({ press }) => {
    const load = deferred();
    const reload = deferred();
    stubMenu(200, served, [load.done, reload.done]);
    renderAdminMenu();

    // First entry: the load is out. The input is read-only, so the entry is set, not typed.
    expectBusy(true);
    fireEvent.change(optionInput(), { target: { value: "1" } });
    press();
    press();
    await lingers();
    expectNoTransfer();
    expect(messageRow("error")?.textContent).toBe("");
    load.release();
    await loaded();
    expectBusy(false);

    enter("01");
    expect(await transferTarget()).toBe(`/users {"from":"/admin"}`);
    fireEvent.click(screen.getByRole("button", { name: "Return to admin menu" }));
    await waitFor(() => {
      expect(requests).toHaveLength(2);
    });

    // A return to /admin: the cached table shows while the reload is out, and neither an entry
    // it would refuse nor one the schema refuses is matched against it.
    await loaded();
    expectBusy(true);
    for (const entry of ["99", "-"]) {
      fireEvent.change(optionInput(), { target: { value: entry } });
      press();
      press();
      await lingers();
      expectNoTransfer();
      expect(messageRow("error")?.textContent).toBe("");
    }

    reload.release();
    await waitFor(() => {
      expectBusy(false);
    });
    fireEvent.change(optionInput(), { target: { value: "5" } });
    press();
    expect(await transferTarget()).toBe(`/transaction-types {"from":"/admin"}`);
    await expectMenuRequests(2);
  });

  // audit: COADM01C-021
  it.each([
    { key: "F5", shiftKey: false, entry: "3", shown: "03" },
    { key: "F12", shiftKey: false, entry: "12", shown: "12" },
    { key: "F3", shiftKey: true, entry: "1", shown: "01" },
  ])(
    "$key (shift $shiftKey) shows the invalid-key error and sends $entry back as $shown",
    async ({ key, shiftKey, entry, shown }) => {
      stubMenu(200, menu);
      renderAdminMenu();
      await loaded();
      fireEvent.change(optionInput(), { target: { value: entry } });

      fireEvent.keyDown(optionInput(), { key, shiftKey });

      await waitFor(() => {
        expect(messageRow("error")?.textContent).toBe(INVALID_KEY_TEXT);
      });
      expect(messageRow("error")?.classList.contains("text-destructive")).toBe(true);
      await waitFor(() => {
        expect(optionInput().value).toBe(shown);
      });
      expectNoTransfer();
      await expectMenuRequests(1);
    },
  );

  // The selection is the screen's own, for overtyping; no audit row covers it.
  it.each([
    {
      how: "ENTER with option 7",
      entry: "7",
      press: () => {
        fireEvent.click(enterButton());
      },
      message: INVALID_OPTION,
      shown: "07",
    },
    {
      how: "F5",
      entry: "3",
      press: () => {
        fireEvent.keyDown(optionInput(), { key: "F5" });
      },
      message: INVALID_KEY_TEXT,
      shown: "03",
    },
  ])(
    "after $how the input shows $shown selected, and the next option typed replaces it",
    async ({ entry, press, message, shown }) => {
      stubMenu(200, served);
      renderAdminMenu();
      await loaded();
      fireEvent.change(optionInput(), { target: { value: entry } });
      press();
      await waitFor(() => {
        expect(messageRow("error")?.textContent).toBe(message);
        expect(optionInput().value).toBe(shown);
        expectOptionSelected();
      });

      typeKey(optionInput(), "1");
      expect(optionInput().value).toBe("1");
      fireEvent.click(enterButton());
      expect(await transferTarget()).toBe(`/users {"from":"/admin"}`);
      await expectMenuRequests(1);
    },
  );

  // audit: COADM01C-004
  it("each key replaces the message of the key before it", async () => {
    stubMenu(200, menu);
    renderAdminMenu();
    await loaded();

    enter("99");
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(INVALID_OPTION);
    });
    expect(messageRow("info")).toBeUndefined();

    enter("6");
    await waitFor(() => {
      expect(messageRow("info")?.textContent).toBe(NOT_INSTALLED);
    });
    expect(messageRow("error")).toBeUndefined();

    fireEvent.keyDown(optionInput(), { key: "F5" });
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(INVALID_KEY_TEXT);
    });
    expect(messageRow("info")).toBeUndefined();

    enter("7");
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(INVALID_OPTION);
    });
    expectNoTransfer();
    await expectMenuRequests(1);
  });

  // The admin gate is the server's (COADM01C-001, a `server` row): the screen shows the
  // `ScreenError` it answers in the error row, builds no option line and never transfers. With no
  // table to serve it from, each ENTER requests endpoint 3 again, and the gate answers it alike.
  // audit: COADM01-027
  it("shows the 403 ScreenError of the admin gate in the error row and offers no option, for each ENTER's new request too", async () => {
    stubMenu(403, forbidden);
    renderAdminMenu();

    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(forbidden.message);
    });
    expect(screen.queryAllByRole("row")).toHaveLength(0);

    for (const [index, entry] of ["01", "-"].entries()) {
      enter(entry);
      await waitFor(() => {
        expect(requests).toHaveLength(index + 2);
        expectBusy(false);
      });
      expect(messageRow("error")?.textContent).toBe(forbidden.message);
      expect(screen.queryAllByRole("row")).toHaveLength(0);
      expectNoTransfer();
    }
    await expectMenuRequests(3);
  });

  // A load that failed leaves no table to serve ENTER from, so ENTER requests endpoint 3 again
  // and decides its entry from the table that answers, as it would have from the first load. The
  // reload is the screen's own recovery from a failed load, which no audit row covers, so these
  // tests carry no tag.
  it("ENTER after a failed load requests endpoint 3 again and transfers to the option it carried", async () => {
    stubMenuReplies([
      { status: 502, body: unreachable },
      { status: 200, body: served },
    ]);
    renderAdminMenu();
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(unreachable.message);
    });
    expectBusy(false);

    enter("1");

    expect(await transferTarget()).toBe(`/users {"from":"/admin"}`);
    await expectMenuRequests(2);
  });

  it.each([{ entry: "7" }, { entry: "-" }])(
    "ENTER '$entry' after a failed load requests endpoint 3 again, and its table refuses the entry",
    async ({ entry }) => {
      stubMenuReplies([
        { status: 502, body: unreachable },
        { status: 200, body: served },
      ]);
      renderAdminMenu();
      await waitFor(() => {
        expect(messageRow("error")?.textContent).toBe(unreachable.message);
      });

      enter(entry);

      // The table's message replaces the failure's, and the six options are shown.
      await waitFor(() => {
        expect(messageRow("error")?.textContent).toBe(INVALID_OPTION);
      });
      await loaded();
      expectBusy(false);
      expectNoTransfer();
      await expectMenuRequests(2);
      // The reload's refusal marks the input as the first load's would, by the rendered message.
      expect(readOut(optionInput())).toEqual(markedBy(INVALID_OPTION));
      expect(danglingDescriptions()).toEqual([]);
    },
  );

  it("a reload that fails again shows its own failure, and the next ENTER requests the menu once more", async () => {
    const again: components["schemas"]["ScreenError"] = { message: "Service unavailable" };
    stubMenuReplies([
      { status: 502, body: unreachable },
      { status: 503, body: again },
      { status: 200, body: served },
    ]);
    renderAdminMenu();
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(unreachable.message);
    });

    enter("1");
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(again.message);
    });
    expectBusy(false);
    expect(screen.queryAllByRole("row")).toHaveLength(0);
    expectNoTransfer();
    await expectMenuRequests(2);

    enter("1");
    expect(await transferTarget()).toBe(`/users {"from":"/admin"}`);
    await expectMenuRequests(3);
  });

  it.each([
    {
      how: "a click on ENTER=Continue",
      press: () => {
        fireEvent.click(enterButton());
      },
    },
    {
      how: "the Enter key",
      press: () => {
        fireEvent.keyDown(window, { key: "Enter" });
      },
    },
    {
      how: "a submit of the form",
      press: () => {
        fireEvent.submit(menuForm());
      },
    },
  ])(
    "$how while ENTER's reload is out sends nothing more and decides nothing",
    async ({ press }) => {
      const reload = deferred();
      stubMenuReplies(
        [
          { status: 502, body: unreachable },
          { status: 200, body: served },
        ],
        [Promise.resolve(), reload.done],
      );
      renderAdminMenu();
      await waitFor(() => {
        expect(messageRow("error")?.textContent).toBe(unreachable.message);
      });

      enter("1");
      await waitFor(() => {
        expect(requests).toHaveLength(2);
        expectBusy(true);
      });
      // The keyboard is locked and the failure's message is gone, cleared for the key; no key
      // decides anything.
      expect(messageRow("error")?.textContent).toBe("");
      press();
      press();
      await lingers();
      expect(messageRow("error")?.textContent).toBe("");
      expectNoTransfer();
      expect(requests).toHaveLength(2);

      reload.release();
      expect(await transferTarget()).toBe(`/users {"from":"/admin"}`);
      await expectMenuRequests(2);
    },
  );

  it("a failed reload on a return to /admin keeps the cached table, and two ENTERs in one turn request endpoint 3 once", async () => {
    stubMenuReplies([
      { status: 200, body: served },
      { status: 502, body: unreachable },
      { status: 200, body: served },
    ]);
    renderAdminMenu();
    await loaded();
    enter("01");
    expect(await transferTarget()).toBe(`/users {"from":"/admin"}`);
    fireEvent.click(screen.getByRole("button", { name: "Return to admin menu" }));
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(unreachable.message);
      expectBusy(false);
    });
    await loaded();

    // Both submits reach the screen before the render that locks the keyboard.
    fireEvent.change(optionInput(), { target: { value: "5" } });
    fireEvent.submit(menuForm());
    fireEvent.submit(menuForm());

    expect(await transferTarget()).toBe(`/transaction-types {"from":"/admin"}`);
    await lingers();
    await expectMenuRequests(3);
  });

  it("F3 while ENTER's reload is out goes to sign-on, and the reload's answer transfers nowhere", async () => {
    const reload = deferred();
    const signon = deferred();
    stubMenuReplies(
      [
        { status: 502, body: unreachable },
        { status: 200, body: served },
      ],
      [Promise.resolve(), reload.done],
    );
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    // Sign-on opens only once `signon` settles, as a route waits on its loader, so the menu stays
    // mounted after F3 while the reload answers.
    render(
      <QueryClientProvider client={client}>
        <MemoryRouter initialEntries={[PATHS.COADM01]}>
          <Suspense fallback={null}>
            <Routes>
              <Route path={PATHS.COADM01} element={<COADM01 />} />
              <Route path={PATHS.COSGN00} element={<OpensWhen opened={signon.done} />} />
              {OPTION_PROGRAMS.map((program) => (
                <Route key={program} path={PROGRAM_ROUTES[program]} element={<Target />} />
              ))}
            </Routes>
          </Suspense>
        </MemoryRouter>
      </QueryClientProvider>,
    );
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(unreachable.message);
    });

    enter("1");
    await waitFor(() => {
      expect(requests).toHaveLength(2);
      expectBusy(true);
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));
    });
    reload.release();
    await waitFor(() => {
      expect(client.isFetching()).toBe(0);
    });
    await lingers();
    expect(screen.queryByTestId("target")).toBeNull();

    signon.release();
    expect(await transferTarget()).toBe(`${PATHS.COSGN00} null`);
    await expectMenuRequests(2);
  });

  // A return from an option screen is a first entry again: the menu is requested once more and
  // sent cleared, so the message of the key before the transfer does not survive it, and its ENTER
  // transfers as before.
  // audit: COADM01C-003
  // audit: COADM01C-004
  it("a return to /admin re-mounts the menu cleared, and its ENTER transfers again", async () => {
    stubMenu(200, served);
    renderAdminMenu();
    await loaded();

    enter("99");
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(INVALID_OPTION);
    });
    enter("01");
    expect(await transferTarget()).toBe(`/users {"from":"/admin"}`);

    fireEvent.click(screen.getByRole("button", { name: "Return to admin menu" }));

    await loaded();
    await waitFor(() => {
      expect(requests).toHaveLength(2);
    });
    expect(optionInput().value).toBe("");
    expect(document.activeElement).toBe(optionInput());
    expect(messageRow("error")?.textContent).toBe("");

    enter("5");
    expect(await transferTarget()).toBe(`/transaction-types {"from":"/admin"}`);
    await expectMenuRequests(2);
  });

  // The replacement's accessibility wiring, which no audit row states: the option input names in
  // `aria-describedby` only elements the screen renders. It has no hint, so it is described by
  // nothing while unmarked. The invalid-option refusal (COADM01C-016), of a digits entry or of one
  // the numeric field refuses, marks it invalid, described by its hidden field message, which
  // repeats the row's text; the not-installed information, an invalid key and an edit of the
  // entry leave it unmarked. The label follows the mark but keeps its colour, as COADM01C never
  // recolours it.
  it.each<{
    state: string;
    reach: () => Promise<void>;
    expected: { invalid: string; described: { text: string; hidden: boolean }[] | null };
  }>([
    { state: "on entry", reach: async () => undefined, expected: UNMARKED },
    {
      state: "after ENTER with 99",
      reach: () => enterUntil("99", "error", INVALID_OPTION),
      expected: markedBy(INVALID_OPTION),
    },
    {
      state: "after ENTER with the entry the numeric field refuses, 1.",
      reach: () => enterUntil("1.", "error", INVALID_OPTION),
      expected: markedBy(INVALID_OPTION),
    },
    {
      state: "after ENTER with the not-installed option 6",
      reach: () => enterUntil("6", "info", NOT_INSTALLED),
      expected: UNMARKED,
    },
    {
      state: "after an invalid key that follows an invalid option",
      reach: async () => {
        await enterUntil("99", "error", INVALID_OPTION);
        fireEvent.keyDown(optionInput(), { key: "F5" });
        await waitFor(() => {
          expect(messageRow("error")?.textContent).toBe(INVALID_KEY_TEXT);
        });
      },
      expected: UNMARKED,
    },
    {
      state: "after the not-installed option that follows an invalid option",
      reach: async () => {
        await enterUntil("99", "error", INVALID_OPTION);
        await enterUntil("6", "info", NOT_INSTALLED);
      },
      expected: UNMARKED,
    },
    {
      state: "after an edit of the entry that follows an invalid option",
      reach: async () => {
        await enterUntil("99", "error", INVALID_OPTION);
        fireEvent.change(optionInput(), { target: { value: "1" } });
      },
      expected: UNMARKED,
    },
  ])(
    "describes the option input by rendered elements only, $state",
    async ({ reach, expected }) => {
      stubMenu(200, menu);
      renderAdminMenu();
      await loaded();
      await reach();

      await waitFor(() => {
        expect(readOut(optionInput())).toEqual(expected);
      });
      expect(danglingDescriptions()).toEqual([]);
      // The message row stays the one visible copy of the text that marks the input.
      for (const { text } of expected.described ?? []) {
        const visible = visibleCopies(text);
        expect(visible).toHaveLength(1);
        expect(messageRow("error")?.contains(visible[0] ?? null)).toBe(true);
      }
      const label = screen.getByText("Please select an option :");
      expect(label.getAttribute("data-error")).toBe(expected.invalid);
      expect(label.className.split(" ")).toEqual(
        expect.arrayContaining(["text-foreground", "data-[error=true]:text-foreground"]),
      );
      expect(label.className).not.toContain("text-destructive");
      expectNoTransfer();
      await expectMenuRequests(1);
    },
  );
});

// CA00 entered without a COMMAREA transfers to whatever the uninitialised `CDEMO-TO-PROGRAM`
// holds, so the COBOL target has no single value. The row's replacement counterpart is the route
// guard in front of `/admin`, which only the application's route table mounts, so this group
// renders that table rather than the screen alone.
describe("BLOCKED X-COADM01C-H1: /admin opened with no session, not a parity result", () => {
  beforeEach(() => {
    requests = [];
    sessionStorage.clear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  // audit: COADM01C-002
  it("the route guard sends /admin to /signon before the menu requests endpoint 3", async () => {
    stubMenu(200, served);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, { initialEntries: [PATHS.COADM01] });
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );

    expect(await screen.findByRole("button", { name: "ENTER=Sign-on" })).toBeDefined();
    expect(router.state.location.pathname).toBe(PATHS.COSGN00);
    expect(screen.queryByRole("heading", { level: 1, name: "Admin Menu" })).toBeNull();
    expect(requests).toHaveLength(0);
    router.dispose();
  });
});

// Endpoint 3 answers a token whose user type is not `A` with a 403 `ScreenError`. The screen
// shows that body's message as it came, and only a 401 ends the session.
describe("COADM01: a 403 keeps the session", () => {
  beforeEach(() => {
    requests = [];
    setSession("test-token", "ADMIN001", "A");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  /** A request as its method, path, query, `Authorization` header and body text. */
  async function shapeOf(request: Request) {
    const url = new URL(request.url);
    return {
      method: request.method,
      path: url.pathname,
      query: Object.fromEntries(url.searchParams),
      authorization: request.headers.get("Authorization"),
      body: await request.clone().text(),
    };
  }

  it("shows the 403 body's message unchanged and keeps the session, after one GET with the bearer token, no query and no body", async () => {
    stubMenu(403, forbidden);
    renderAdminMenu();

    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(forbidden.message);
    });

    expect(getSession()).toEqual({ token: "test-token", userId: "ADMIN001", userType: "A" });
    expect(await Promise.all(requests.map(shapeOf))).toEqual([
      { method: "GET", path: MENU_PATH, query: {}, authorization: "Bearer test-token", body: "" },
    ]);
  });
});

/**
 * The main menu's error row once `stubMenu` has answered its endpoint 2 load with a 500. The admin
 * guard sends every non-admin there.
 */
const MAIN_MENU_ANSWER = `Unexpected request GET ${globalThis.location.origin}/api/v1/menus/main`;

// The admin guard in front of `/admin` lets user type `A` through, and that type only: `U`, a
// lower-case `a` and any other type go to the main menu before endpoint 3 is requested.
describe("COADM01 in the route table: the admin guard", () => {
  beforeEach(() => {
    requests = [];
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  // `shown` is a text the menu shows only once its load on entry is answered.
  it.each([
    {
      userId: "ADMIN001",
      userType: "A",
      path: PATHS.COADM01,
      heading: "Admin Menu",
      loads: 1,
      shown: "01. User List (Security)",
    },
    {
      userId: "USER0001",
      userType: "U",
      path: PATHS.COMEN01,
      heading: "Main Menu",
      loads: 0,
      shown: MAIN_MENU_ANSWER,
    },
    {
      userId: "USER0001",
      userType: "a",
      path: PATHS.COMEN01,
      heading: "Main Menu",
      loads: 0,
      shown: MAIN_MENU_ANSWER,
    },
    {
      userId: "USER0001",
      userType: "X",
      path: PATHS.COMEN01,
      heading: "Main Menu",
      loads: 0,
      shown: MAIN_MENU_ANSWER,
    },
  ])(
    "user type $userType opening /admin is shown $heading at $path",
    async ({ userId, userType, path, heading, loads, shown }) => {
      setSession("test-token", userId, userType);
      stubMenu(200, served);
      const client = new QueryClient({
        defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
      });
      const router = createMemoryRouter(routes, { initialEntries: [PATHS.COADM01] });
      render(
        <QueryClientProvider client={client}>
          <RouterProvider router={router} />
        </QueryClientProvider>,
      );

      expect(await screen.findByRole("heading", { level: 1, name: heading })).toBeDefined();
      expect(router.state.location.pathname).toBe(path);
      await waitFor(() => {
        expect(
          requests.filter((request) => new URL(request.url).pathname === MENU_PATH),
        ).toHaveLength(loads);
      });
      // Waiting for `shown` keeps the menu load's last render inside the test's waits.
      expect(await screen.findByText(shown)).toBeDefined();
      router.dispose();
    },
  );

  // A page restored from the back/forward cache resumes the admin menu it was frozen with, while a
  // later page of the same tab signed off and signed a user on. That user's session reaches this
  // page in the shared `sessionStorage`, unannounced; the restore (`pageshow` with `persisted`)
  // has the admin guard read it and send the non-admin to the main menu.
  it("a page restored after a user's session replaced the admin's goes to the main menu", async () => {
    setSession("test-token", "ADMIN001", "A");
    stubMenu(200, served);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, { initialEntries: [PATHS.COADM01] });
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
    expect(
      await screen.findByText("01. User List (Security)", {}, { timeout: 5_000 }),
    ).toBeDefined();

    const user: StoredSession = { token: "test-token-user", userId: "USER0001", userType: "U" };
    sessionStorage.setItem(SESSION_KEY, JSON.stringify(user));
    // Nothing announced the change, so the guard still shows the admin menu.
    expect(screen.getByRole("heading", { level: 1, name: "Admin Menu" })).toBeDefined();

    act(() => {
      window.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true }));
    });

    expect(
      await screen.findByRole("heading", { level: 1, name: "Main Menu" }, { timeout: 5_000 }),
    ).toBeDefined();
    expect(router.state.location.pathname).toBe(PATHS.COMEN01);
    expect(screen.queryByRole("heading", { level: 1, name: "Admin Menu" })).toBeNull();
    expect(getSession()).toEqual(user);
    // Waiting for the main menu's answer keeps its load's last render inside the test's waits.
    expect(await screen.findByText(MAIN_MENU_ANSWER)).toBeDefined();
    expect(
      requests.filter((request) => new URL(request.url).pathname === MENU_PATH),
    ).toHaveLength(1);
    router.dispose();
  });
});

/** The 401 of the `AdminUser` extractor for a missing, invalid or expired token. */
const signonRequired: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};

/**
 * Renders the application's route table at `/admin`, so the admin guard that only the table
 * mounts stands in front of the screen. Each screen is imported by its route's loader, so a wait
 * for one to render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COADM01] });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

// The menu load of a stored `A` session, refused by the API. A 401 clears the session
// (`api/client.ts`), and the admin guard then replaces the still-mounted menu with sign-on at
// once, with no key or navigation of the test. A 403 is the gate's refusal (COADM01C-001): it
// clears nothing, so the guard keeps the menu, which shows the gate's message.
describe("COADM01 in the route table: a 401 ends the session, a 403 keeps it", () => {
  beforeEach(() => {
    requests = [];
    setSession("test-token", "ADMIN001", "A");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  it("the menu load answered 401 replaces the menu with sign-on and clears the session, with no retry", async () => {
    stubMenu(401, signonRequired);
    const router = renderRouteTable();

    expect(
      await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
    ).toBeDefined();
    expect(router.state.location.pathname).toBe(PATHS.COSGN00);
    expect(screen.queryByRole("heading", { level: 1, name: "Admin Menu" })).toBeNull();
    expect(getSession()).toBeNull();
    await expectMenuRequests(1);
    expect(requests[0].headers.get("Authorization")).toBe("Bearer test-token");
    router.dispose();
  });

  it("the menu load answered 403 keeps the menu with the gate's message and the stored session", async () => {
    stubMenu(403, forbidden);
    const router = renderRouteTable();

    expect(
      await screen.findByRole("heading", { level: 1, name: "Admin Menu" }, { timeout: 5_000 }),
    ).toBeDefined();
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(forbidden.message);
    });
    expect(router.state.location.pathname).toBe(PATHS.COADM01);
    expect(screen.queryByRole("button", { name: "ENTER=Sign-on" })).toBeNull();
    expect(getSession()).toEqual({ token: "test-token", userId: "ADMIN001", userType: "A" });
    await expectMenuRequests(1);
    router.dispose();
  });
});

// A line no option fills draws no row rule. Its `TableRow` keeps the component's `border-b`, so
// the line keeps a filled line's height, and `border-transparent` hides that rule; a line with an
// option keeps the component default, the rule under option 06 included. jsdom loads no
// stylesheet, so the rule is read from the row's border classes; the browser check measures it.
describe("COADM01: option lines without an option draw no row rule", () => {
  beforeEach(() => {
    requests = [];
    setSession("test-token", "ADMIN001", "A");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  /** The map field names of the option lines, `OPTN001` to `OPTN012`. */
  const LINE_FIELDS = Array.from(
    { length: 12 },
    (_, index) => `OPTN${String(index + 1).padStart(3, "0")}`,
  );

  /** The component's rule of a line with an option. */
  const RULED = "border-b";

  /** The rule of a line without one: the same width, drawn transparent. */
  const UNRULED = "border-b border-transparent";

  /** The `TableRow` that holds the option line `name`. */
  function optionLine(name: string): HTMLElement {
    const line = field(name).closest("tr");
    if (line === null) {
      throw new TypeError(`the option line ${name} is in no table row`);
    }
    return line;
  }

  /** Each option line's border classes, in line order. */
  function lineRules(): string[] {
    return LINE_FIELDS.map((name) =>
      Array.from(optionLine(name).classList)
        .filter((utility) => utility.startsWith("border-"))
        .join(" "),
    );
  }

  /** The component's hover band of a line with an option. */
  const BANDED = "hover:bg-muted/50";

  /** The hover band of a line without one: none, so its blank field draws nothing on hover. */
  const UNBANDED = "hover:bg-transparent";

  /** Each option line's hover background classes, in line order. */
  function lineBands(): string[] {
    return LINE_FIELDS.map((name) =>
      Array.from(optionLine(name).classList)
        .filter((utility) => utility.startsWith("hover:bg-"))
        .join(" "),
    );
  }

  // audit: COADM01-018
  // audit: COADM01-019
  // audit: COADM01-020
  // audit: COADM01-021
  // audit: COADM01-022
  // audit: COADM01-023
  it("every line is unruled and unbanded while endpoint 3 loads; once loaded, lines 6 to 11 keep the rule and hover band and the blank lines 12 to 17 stay unruled and unbanded", async () => {
    const load = deferred();
    stubMenu(200, served, [load.done]);
    renderAdminMenu();
    expectBusy(true);
    expect(lineRules()).toEqual(LINE_FIELDS.map(() => UNRULED));
    expect(lineBands()).toEqual(LINE_FIELDS.map(() => UNBANDED));

    load.release();
    await loaded();
    expect(lineRules()).toEqual([
      ...served.options.map(() => RULED),
      ...LINE_FIELDS.slice(served.options.length).map(() => UNRULED),
    ]);
    expect(lineBands()).toEqual([
      ...served.options.map(() => BANDED),
      ...LINE_FIELDS.slice(served.options.length).map(() => UNBANDED),
    ]);
    // Lines 12 to 17 (OPTN007 to OPTN012) are the blank TableCells of the map, hidden from
    // assistive technology, each showing the INITIAL of one space.
    for (const name of LINE_FIELDS.slice(6)) {
      expect(field(name).dataset.slot).toBe("table-cell");
      expect(field(name).textContent).toBe(" ");
      expect(optionLine(name).getAttribute("aria-hidden")).toBe("true");
    }
    await expectMenuRequests(1);
  });

  it("every line stays unruled after endpoint 3 refuses the load", async () => {
    stubMenu(403, forbidden);
    renderAdminMenu();
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(forbidden.message);
    });

    expect(lineRules()).toEqual(LINE_FIELDS.map(() => UNRULED));
    await expectMenuRequests(1);
  });
});
