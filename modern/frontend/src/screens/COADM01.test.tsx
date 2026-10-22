import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
import { setSession } from "@/auth/session";
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
 * a read-only `Input`; the shared `ScreenShell` renders both as plain text, so those rows are not
 * tagged here. Their content rows (`COADM01-029` to `COADM01-034`) are. `COADM01C-002` is blocked
 * by halt candidate `X-COADM01C-H1`: its group is reported BLOCKED and asserts only the
 * replacement's path the row states, never a COBOL result.
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
 * Endpoint 3's body for the six options of `app/cpy/COADM02Y.cpy:26-53`. Each row text is the 40
 * bytes `BUILD-MENU-OPTIONS` strings: the two digits, `. `, the 35-byte name and one space. Option
 * 6 is stubbed as not installed, the case of COADM01C-018 that no shipped option has.
 */
const menu: components["schemas"]["MenuResponse"] = {
  invalid_option_message: INVALID_OPTION,
  options: [
    {
      admin_only: true,
      installed: true,
      number: "01",
      program: "COUSR00C",
      text: "01. User List (Security)".padEnd(40, " "),
      unavailable_message: NOT_INSTALLED,
    },
    {
      admin_only: true,
      installed: true,
      number: "02",
      program: "COUSR01C",
      text: "02. User Add (Security)".padEnd(40, " "),
      unavailable_message: NOT_INSTALLED,
    },
    {
      admin_only: true,
      installed: true,
      number: "03",
      program: "COUSR02C",
      text: "03. User Update (Security)".padEnd(40, " "),
      unavailable_message: NOT_INSTALLED,
    },
    {
      admin_only: true,
      installed: true,
      number: "04",
      program: "COUSR03C",
      text: "04. User Delete (Security)".padEnd(40, " "),
      unavailable_message: NOT_INSTALLED,
    },
    {
      admin_only: true,
      installed: true,
      number: "05",
      program: "COTRTLIC",
      text: "05. Transaction Type List/Update (Db2)".padEnd(40, " "),
      unavailable_message: NOT_INSTALLED,
    },
    {
      admin_only: true,
      installed: false,
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
 * answers 500, which no test expects.
 */
function stubMenu(
  status: number,
  body: components["schemas"]["MenuResponse"] | components["schemas"]["ScreenError"],
): void {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init);
      requests.push(request);
      if (request.method === "GET" && new URL(request.url).pathname === MENU_PATH) {
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
      expect(field("TRNNAME").textContent).toBe("CA00");
      expect(field("PGMNAME").textContent).toBe("COADM01C");
      expect(field("TITLE01").textContent).toBe("      AWS Mainframe Modernization       ");
      expect(field("TITLE02").textContent).toBe("              CardDemo                  ");
      // The date and time are the current instant, so only their formats are compared.
      expect(field("CURDATE").textContent).toMatch(/^[0-9]{2}\/[0-9]{2}\/[0-9]{2}$/);
      expect(field("CURTIME").textContent).toMatch(/^[0-9]{2}:[0-9]{2}:[0-9]{2}$/);
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
      });
      expect(focus).toHaveBeenCalled();
      expect(document.activeElement).toBe(optionInput());
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
  // `ScreenError` it answers in the error row, builds no option line and never transfers.
  // audit: COADM01-027
  it("shows the 403 ScreenError of the admin gate in the error row and offers no option", async () => {
    stubMenu(403, forbidden);
    renderAdminMenu();

    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(forbidden.message);
    });
    expect(screen.queryAllByRole("row")).toHaveLength(0);

    enter("01");
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(forbidden.message);
    });
    expectNoTransfer();

    enter("-");
    await waitFor(() => {
      expect(messageRow("error")?.textContent).toBe(forbidden.message);
    });
    expectNoTransfer();
    await expectMenuRequests(1);
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
