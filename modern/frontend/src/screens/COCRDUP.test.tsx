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
import { getSession, setSession } from "@/auth/session";
import { PATHS, routes } from "@/routes";
import { COCRDUP } from "./COCRDUP";

/*
 * Render test of the card update screen (map CCRDUPA, transaction CCUP, program COCRDUPC) in
 * jsdom, with `fetch` stubbed for endpoint 9 (`GET /api/v1/cards/edit`) and endpoint 10
 * (`PUT /api/v1/cards/{card_ref}`). Expected texts are the audited literals of
 * `modern/audit/screens.md` (`COCRDUP-nnn`) and `modern/audit/online/COCRDUPC.md`
 * (`COCRDUPC-nnn`).
 *
 * Response fixtures carry only the redacted card number. The 16-digit number typed for a search
 * is synthetic request input, assembled at run time so no literal has the shape of one.
 */

const ACCT_ID = "00000000050";
const CARD_REF = 1;
const MASKED_CARD = "************5740";
/** A card number as the operator types it: synthetic, request input only. */
const TYPED_CARD = "0".repeat(12) + "5740";
const STORED_NAME = "TEST HOLDER";
const NEW_NAME = "NEW HOLDER";
const OTHER_NAME = "OTHER HOLDER";
const EXPIRY = "2030-06-30";

const CARDS_EDIT = "GET /api/v1/cards/edit";
const CARD_PUT = `PUT /api/v1/cards/${CARD_REF}`;

/** COCRDUPC `3250-SETUP-INFOMSG`: the info line of each state (`COCRDUPC.md`). */
const INFO_SEARCH = "Please enter Account and Card Number";
const INFO_SHOWN = "Details of selected card shown above";
const INFO_CHANGES = "Update card details presented above.";
const INFO_CONFIRM = "Changes validated.Press F5 to save";
const INFO_DONE = "Changes committed to database";
const INFO_FAILED = "Changes unsuccessful. Please try again";

/** COCRDUP-012 to COCRDUP-026: the map literals that label the six inputs, every space kept. */
const LABELS = {
  account: "Account Number    :",
  card: "Card Number       :",
  name: "Name on card      :",
  status: "Card Active Y/N   : ",
  month: "Expiry Date       : ",
  year: "/",
} as const;

/** COCRDUP-036 and COCRDUP-039: the two 40-byte `COTTL01Y` titles. */
const TITLE01 = " ".repeat(6) + "AWS Mainframe Modernization" + " ".repeat(7);
const TITLE02 = " ".repeat(14) + "CardDemo" + " ".repeat(18);

/** What COCRDLI puts in the route state for a `U` selection (AAP 0.3.4.1). */
const FROM_LIST = { from: PATHS.COCRDLI, acct_id: ACCT_ID, card_ref: CARD_REF };

/** Endpoint 9: the card with its stored 50-byte name, the redacted number and the snapshot. */
const view: components["schemas"]["CardUpdateView"] = {
  acct_id: ACCT_ID,
  card_num: MASKED_CARD,
  card_ref: CARD_REF,
  embossed_name: STORED_NAME.padEnd(50, " "),
  expiration_date: EXPIRY,
  info_message: INFO_SHOWN,
  message: null,
  snapshot: {
    state: "show_details",
    acct_id: ACCT_ID,
    embossed_name: STORED_NAME.padEnd(50, " "),
    expiration_date: EXPIRY,
    status: "Y",
  },
  status: "Y",
};

/** Endpoint 9 again after another user changed the card (COCRDUPC-037). */
const changedView: components["schemas"]["CardUpdateView"] = {
  ...view,
  embossed_name: OTHER_NAME.padEnd(50, " "),
  status: "N",
  snapshot: { ...view.snapshot, embossed_name: OTHER_NAME.padEnd(50, " "), status: "N" },
};

/** Endpoint 10, `aid` ENTER: changes validated, confirmation asked (COCRDUPC-034). */
const validated: components["schemas"]["CardUpdateResult"] = {
  committed: false,
  info_message: INFO_CONFIRM,
  save_enabled: true,
  snapshot: { ...view.snapshot, state: "changes_ok_not_confirmed" },
  state: "changes_ok_not_confirmed",
};

/** Endpoint 10, `aid` PF5: the card rewritten (COCRDUPC-039). */
const committed: components["schemas"]["CardUpdateResult"] = {
  committed: true,
  info_message: INFO_DONE,
  save_enabled: false,
  snapshot: null,
  state: "changes_okayed_and_done",
};

const accountMissing: components["schemas"]["ScreenError"] = {
  message: "Account number not provided",
  field: "acct_id",
};
const accountNotNumeric: components["schemas"]["ScreenError"] = {
  message: "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER",
  field: "acct_id",
};
const cardMissing: components["schemas"]["ScreenError"] = {
  message: "Card number not provided",
  field: "card_num",
};
const notFound: components["schemas"]["ScreenError"] = {
  message: "Did not find cards for this search condition",
  field: "acct_id",
};
const nameMissing: components["schemas"]["ScreenError"] = {
  message: "Card name not provided",
  field: "embossed_name",
};
const noChange: components["schemas"]["ScreenError"] = {
  message: "No change detected with respect to values fetched.",
  field: "embossed_name",
};
const changedByOther: components["schemas"]["ScreenError"] = {
  message: "Record changed by some one else. Please review",
  field: null,
};
const lockFailed: components["schemas"]["ScreenError"] = {
  message: "Could not lock record for update",
  field: null,
};
const rewriteFailed: components["schemas"]["ScreenError"] = {
  message: "Update of record failed",
  field: null,
};
const storeUnavailable: components["schemas"]["ScreenError"] = {
  message: "Service unavailable",
  field: null,
};

/** Text matching that keeps every space, as the map literals do. */
const exactText = { normalizer: (text: string) => text };

/** A JSON response, as the API sends every success and every `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces `fetch`. Each request is routed on `<METHOD> <path>` and answered with the next reply
 * listed for that route; any other request, and one past the end of its list, gets a 500. Every
 * request is recorded with its query, JSON body and `Authorization` header.
 */
function stubApi(replies: Record<string, { status: number; body: unknown }[]>) {
  const queues = new Map(Object.entries(replies).map(([route, list]) => [route, [...list]]));
  const calls: {
    route: string;
    query: URLSearchParams;
    body: unknown;
    authorization: string | null;
  }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      const url = new URL(req.url);
      const route = `${req.method} ${url.pathname}`;
      const body: unknown = req.method === "GET" ? undefined : await req.clone().json();
      calls.push({
        route,
        query: url.searchParams,
        body,
        authorization: req.headers.get("Authorization"),
      });
      const reply = queues.get(route)?.shift();
      if (reply === undefined) {
        const unexpected: components["schemas"]["ScreenError"] = {
          message: `Unexpected request ${route}`,
          field: null,
        };
        return json(500, unexpected);
      }
      return json(reply.status, reply.body);
    }),
  );
  return calls;
}

/** The screen a navigation reached, by its path, with the route state it was handed. */
function RouteStub() {
  const location = useLocation();
  return <p>{`Stub at ${location.pathname}, state ${JSON.stringify(location.state)}`}</p>;
}

/** COCRDUP at `/cards/update` with `state`, and stubs at its two F3 targets. */
function renderScreen(state?: unknown) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COCRDUP, state }]}>
        <Routes>
          <Route path={PATHS.COCRDUP} element={<COCRDUP />} />
          <Route path={PATHS.COCRDLI} element={<RouteStub />} />
          <Route path={PATHS.COMEN01} element={<RouteStub />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** Lets every pending promise settle, so any request the screen would send has been sent. */
async function settle(): Promise<void> {
  await new Promise((resolve) => {
    setTimeout(resolve, 0);
  });
}

/** `element` as an `<input>`, failing the test when it is anything else. */
function asInput(element: HTMLElement): HTMLInputElement {
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`Expected an input, found <${element.tagName.toLowerCase()}>`);
  }
  return element;
}

/** The input the map literal `label` labels. */
function field(label: string): HTMLInputElement {
  return asInput(screen.getByLabelText(label, exactText));
}

/** The values the six inputs show, by `LABELS` key. */
function shownValues(): Record<string, string> {
  return Object.fromEntries(
    Object.entries(LABELS).map(([name, label]) => [name, field(label).value]),
  );
}

/**
 * A PF-key button of row 24 by its label, shown or hidden. Matched by its text: a button with the
 * HTML `hidden` attribute has an empty accessible name, so `getByRole` with `hidden: true` finds
 * it but cannot match it by `name`.
 */
function key(label: string): HTMLButtonElement {
  const group = screen.getByRole("group", { name: "Function keys" });
  const element = within(group).getByText(label, { selector: "button" });
  if (!(element instanceof HTMLButtonElement)) {
    throw new Error(`Expected a button, found <${element.tagName.toLowerCase()}>`);
  }
  return element;
}

/** The one message row of `kind` (`MessageLine`). */
function messageRow(kind: "error" | "info"): HTMLElement {
  const rows = screen
    .getAllByRole("alert")
    .filter((row) => row.getAttribute("data-message-kind") === kind);
  expect(rows).toHaveLength(1);
  const [row] = rows;
  if (row === undefined) {
    throw new Error(`No ${kind} row`);
  }
  return row;
}

function errorText(): string | null {
  return messageRow("error").textContent;
}

function infoText(): string | null {
  return messageRow("info").textContent;
}

/** COCRDUPC-045: whether the account and card, and the four details, are enterable. */
function expectEnterable(keys: boolean, details: boolean) {
  expect([LABELS.account, LABELS.card].map((label) => field(label).readOnly)).toEqual([
    !keys,
    !keys,
  ]);
  expect(
    [LABELS.name, LABELS.status, LABELS.month, LABELS.year].map((label) => field(label).readOnly),
  ).toEqual([!details, !details, !details, !details]);
}

/** COCRDUP-034: `F5=Save F12=Cancel` shown (in view and in the accessibility tree) or hidden. */
function expectConfirmationKeys(shown: boolean) {
  for (const label of ["F5=Save", "F12=Cancel"]) {
    expect(key(label).hidden).toBe(!shown);
    expect(screen.queryByRole("button", { name: label }) !== null).toBe(shown);
  }
}

/** Scenario 6: neither the rendered text nor any input shows a run of 16 digits. */
function expectNoFullCardDigits() {
  expect(document.body.textContent ?? "").not.toMatch(/\d{16}/);
  for (const input of document.querySelectorAll("input")) {
    expect(input.value).not.toMatch(/\d{16}/);
  }
}

/** The text of a header value field (`ScreenShell`), by its BMS name. */
function headerValue(name: string): string | null {
  return document.querySelector(`[data-field="${name}"]`)?.textContent ?? null;
}

/** The `GridCell` classes that place map `POS=(row,col)` with `LENGTH=span`. */
function cellClass(row: number, col: number, span: number): string {
  return `row-start-${row} col-start-${col} col-span-${span}`;
}

/** The header-row literal whose whole text is `text`, inside the `CardHeader` of `ScreenShell`. */
function headerLabel(text: string): HTMLElement {
  const header = document.querySelector<HTMLElement>('[data-slot="card-header"]');
  if (header === null) {
    throw new Error("No card header");
  }
  return within(header).getByText(text, exactText);
}

/** The columns of map row `row` that a `GridCell` of the screen body covers. */
function occupiedColumns(row: number): Set<number> {
  const form = screen.getByRole("form", { name: "Update Credit Card Details" });
  const columns = new Set<number>();
  for (const cell of form.querySelectorAll(`.row-start-${row}`)) {
    const start = /\bcol-start-(\d+)\b/.exec(cell.className)?.[1];
    const span = /\bcol-span-(\d+)\b/.exec(cell.className)?.[1];
    if (start === undefined || span === undefined) {
      throw new Error(`Grid cell without a column: ${cell.className}`);
    }
    const first = Number.parseInt(start, 10);
    for (let column = first; column < first + Number.parseInt(span, 10); column += 1) {
      columns.add(column);
    }
  }
  return columns;
}

/** COCRDUPC-046: the screen moves the cursor in an effect after the render that shows a result. */
async function expectFocus(label: string) {
  await waitFor(() => {
    expect(document.activeElement).toBe(field(label));
  });
}

/**
 * The input `label` labels is invalid and its `aria-describedby` names a field message that
 * exists and reads `message`. That message is `sr-only`, so the error row is the one place the
 * text is visible.
 */
function expectFieldMessage(label: string, message: string) {
  const input = field(label);
  expect(input.getAttribute("aria-invalid")).toBe("true");
  const described = (input.getAttribute("aria-describedby") ?? "")
    .split(" ")
    .filter((id) => id !== "")
    .map((id) => document.getElementById(id))
    .filter((element) => element?.getAttribute("data-slot") === "form-message");
  expect(described).toHaveLength(1);
  const [fieldMessage] = described;
  expect(fieldMessage?.textContent).toBe(message);
  expect(fieldMessage?.classList.contains("sr-only")).toBe(true);
  const visible = screen
    .getAllByText(message, exactText)
    .filter((element) => element.closest(".sr-only") === null);
  expect(visible).toHaveLength(1);
  const [shown] = visible;
  expect(shown !== undefined && messageRow("error").contains(shown)).toBe(true);
}

function typeSearchKeys(account: string, card: string) {
  fireEvent.change(field(LABELS.account), { target: { value: account } });
  fireEvent.change(field(LABELS.card), { target: { value: card } });
}

/** Enters from COCRDLI and waits for endpoint 9's card (state `S`). */
async function showFromList() {
  renderScreen(FROM_LIST);
  await waitFor(() => {
    expect(field(LABELS.card).value).toBe(MASKED_CARD);
  });
}

/** Enters from the menu, searches by the typed keys and waits for endpoint 9's card. */
async function showFromSearch() {
  renderScreen();
  typeSearchKeys(ACCT_ID, TYPED_CARD);
  fireEvent.click(key("ENTER=Process"));
  await waitFor(() => {
    expect(field(LABELS.card).value).toBe(MASKED_CARD);
  });
}

/** Edits the name and presses ENTER; waits for the confirmation (state `N`). */
async function validate() {
  fireEvent.change(field(LABELS.name), { target: { value: NEW_NAME } });
  fireEvent.click(key("ENTER=Process"));
  await waitFor(() => {
    expect(key("F5=Save").hidden).toBe(false);
  });
}

/** From state `S`: ENTER with the name blanked, answered `Card name not provided` (state `E`). */
async function failNameEdit() {
  fireEvent.change(field(LABELS.name), { target: { value: "" } });
  fireEvent.click(key("ENTER=Process"));
  await waitFor(() => {
    expect(infoText()).toBe(INFO_CHANGES);
  });
}

/** From state `N`: F5, waiting for the info line of the answer (state `C`, `L` or `F`). */
async function save(info: string) {
  fireEvent.click(key("F5=Save"));
  await waitFor(() => {
    expect(infoText()).toBe(info);
  });
}

/** A function key pressed on the keyboard outside every field, Shift held or not. */
function pressKey(name: string, shiftKey = false) {
  fireEvent.keyDown(document.body, { key: name, shiftKey });
}

beforeEach(() => {
  setSession("test-token", "USER0001", "U");
});

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COCRDUP rendering", () => {
  // audit: COCRDUP-011
  // audit: COCRDUP-012
  // audit: COCRDUP-015
  // audit: COCRDUP-018
  // audit: COCRDUP-021
  // audit: COCRDUP-024
  // audit: COCRDUP-026
  // audit: COCRDUP-029
  // audit: COCRDUP-031
  // audit: COCRDUP-032
  // audit: COCRDUP-033
  // audit: COCRDUP-034
  // audit: COCRDUP-035
  // audit: COCRDUP-036
  // audit: COCRDUP-037
  // audit: COCRDUP-038
  // audit: COCRDUP-039
  // audit: COCRDUP-040
  // audit: COCRDUPC-001
  // audit: COCRDUPC-045
  it("first entry from the menu shows the empty search screen and sends no request", async () => {
    const calls = stubApi({});
    renderScreen();
    await settle();

    expect(calls).toEqual([]);
    expect(
      screen
        .getByRole("heading", { level: 1, name: "Update Credit Card Details" })
        .getAttribute("data-slot"),
    ).toBe("card-title");
    expect(headerValue("TRNNAME")).toBe("CCUP");
    expect(headerValue("PGMNAME")).toBe("COCRDUPC");
    expect(headerValue("TITLE01")).toBe(TITLE01);
    expect(headerValue("TITLE02")).toBe(TITLE02);
    expect(TITLE01).toHaveLength(40);
    expect(TITLE02).toHaveLength(40);
    expect(headerValue("CURDATE")).toMatch(/^\d{2}\/\d{2}\/\d{2}$/);
    expect(headerValue("CURTIME")).toMatch(/^\d{2}:\d{2}:\d{2}$/);

    // Each map literal is the FormLabel of its input; the map LENGTHs bound the inputs; EXPDAY
    // is not rendered.
    for (const label of Object.values(LABELS)) {
      const element = screen.getByText(label, { ...exactText, selector: "label" });
      expect(element.getAttribute("data-slot")).toBe("form-label");
      expect(element.getAttribute("for")).toBe(field(label).id);
    }
    expect(screen.getAllByRole("textbox")).toHaveLength(6);
    expect(
      Object.fromEntries(
        Object.entries(LABELS).map(([name, label]) => [name, field(label).maxLength]),
      ),
    ).toEqual({ account: 11, card: 16, name: 50, status: 1, month: 2, year: 4 });
    expect(shownValues()).toEqual({
      account: "",
      card: "",
      name: "",
      status: "",
      month: "",
      year: "",
    });
    expectEnterable(true, false);
    await expectFocus(LABELS.account);

    expect(infoText()).toBe(INFO_SEARCH);
    expect(errorText()).toBe("");
    expect(messageRow("error").getAttribute("aria-live")).toBe("polite");

    // Row 24 in map order: FKEYS, then the dark FKEYSC, then the keys the map labels nowhere.
    const group = screen.getByRole("group", { name: "Function keys" });
    expect(
      within(group)
        .getAllByRole("button", { hidden: true })
        .map((button) => button.textContent),
    ).toEqual([
      "ENTER=Process",
      "F3=Exit",
      "F5=Save",
      "F12=Cancel",
      "F1",
      "F2",
      "F4",
      "F6",
      "F7",
      "F8",
      "F9",
      "F10",
      "F11",
    ]);
    const enter = screen.getByRole("button", { name: "ENTER=Process" });
    expect(enter.getAttribute("type")).toBe("submit");
    expect(enter.getAttribute("data-variant")).toBe("default");
    for (const label of ["F3=Exit", "F5=Save", "F12=Cancel"]) {
      expect(key(label).getAttribute("type")).toBe("button");
      expect(key(label).getAttribute("data-variant")).toBe("outline");
      expect(key(label).getAttribute("data-size")).toBe("sm");
    }
    expect(key("F3=Exit").hidden).toBe(false);
    expectConfirmationKeys(false);
  });

  // audit: COCRDUP-001
  // audit: COCRDUP-004
  // audit: COCRDUP-006
  // audit: COCRDUP-009
  it("header rows 1 and 2 place Tran:, Date:, Prog: and Time: before their values", async () => {
    stubApi({});
    renderScreen();
    await settle();

    // Each BLUE literal at its map POS with LENGTH 5; the value it labels starts at column + 6.
    for (const { text, row, col, field, length } of [
      { text: "Tran:", row: 1, col: 1, field: "TRNNAME", length: 4 },
      { text: "Date:", row: 1, col: 65, field: "CURDATE", length: 8 },
      { text: "Prog:", row: 2, col: 1, field: "PGMNAME", length: 8 },
      { text: "Time:", row: 2, col: 65, field: "CURTIME", length: 8 },
    ]) {
      const label = headerLabel(text);
      expect(label.className).toContain("text-muted-foreground");
      expect(label.parentElement?.className).toBe(cellClass(row, col, 5));
      expect(document.querySelector(`[data-field="${field}"]`)?.parentElement?.className).toBe(
        cellClass(row, col + 6, length),
      );
    }
  });

  // `ScreenShell` realises each header value field as read-only text in its title-row cell rather
  // than as an `Input` with `readOnly`, so these rows are proved on that cell. The values it shows
  // are proved under COCRDUP-035 to COCRDUP-040.
  // audit: COCRDUP-002
  // audit: COCRDUP-003
  // audit: COCRDUP-005
  // audit: COCRDUP-007
  // audit: COCRDUP-008
  // audit: COCRDUP-010
  it("header values TRNNAME, TITLE01, CURDATE, PGMNAME, TITLE02 and CURTIME are read-only cells of the CardHeader title rows at their map POS and LENGTH", async () => {
    stubApi({});
    renderScreen();
    await settle();

    const header = document.querySelector('[data-slot="card-header"]');
    expect(header).not.toBeNull();
    // The title rows hold no form control.
    expect(header?.querySelector("input, textarea, select, button")).toBeNull();
    for (const { name, row, col, length } of [
      { name: "TRNNAME", row: 1, col: 7, length: 4 },
      { name: "TITLE01", row: 1, col: 21, length: 40 },
      { name: "CURDATE", row: 1, col: 71, length: 8 },
      { name: "PGMNAME", row: 2, col: 7, length: 8 },
      { name: "TITLE02", row: 2, col: 21, length: 40 },
      { name: "CURTIME", row: 2, col: 71, length: 8 },
    ]) {
      // The field's one element stands in the title rows, not in the body or the footer.
      const shown = document.querySelectorAll(`[data-field="${name}"]`);
      expect(shown).toHaveLength(1);
      const value = shown.item(0);
      expect(header?.contains(value)).toBe(true);
      // Its cell is the map POS and LENGTH, and its text fills the field box, the padding that
      // centres the titles included.
      const cell = value?.parentElement;
      expect(cell?.className).toBe(cellClass(row, col, length));
      expect(value?.textContent).toHaveLength(length);
      // Neither the value nor anything in its cell takes input or focus.
      expect(cell?.querySelector("input, textarea, select, button, [contenteditable]")).toBeNull();
      for (const element of [value, cell]) {
        expect(element?.hasAttribute("contenteditable")).toBe(false);
        expect(element?.hasAttribute("tabindex")).toBe(false);
      }
    }
  });

  // audit: COCRDUPC-045
  // audit: COCRDUPC-046
  it("entry from COCRDLI fetches the row's card by card_ref and shows it redacted", async () => {
    const calls = stubApi({ [CARDS_EDIT]: [{ status: 200, body: view }] });
    await showFromList();

    expect(calls.map((call) => call.route)).toEqual([CARDS_EDIT]);
    expect(Object.fromEntries(calls[0].query)).toEqual({
      acct_id: ACCT_ID,
      card_ref: String(CARD_REF),
    });
    expect(calls[0].authorization).toBe("Bearer test-token");

    // The stored name loses its trailing spaces on screen; the snapshot keeps them.
    expect(shownValues()).toEqual({
      account: ACCT_ID,
      card: MASKED_CARD,
      name: STORED_NAME,
      status: "Y",
      month: "06",
      year: "2030",
    });
    expect(screen.getAllByRole("textbox")).toHaveLength(6);
    expectEnterable(false, true);
    await expectFocus(LABELS.name);
    expect(infoText()).toBe(INFO_SHOWN);
    expect(errorText()).toBe("");
    expectConfirmationKeys(false);
    expectNoFullCardDigits();
  });

  // audit: COCRDUP-014
  // audit: COCRDUP-017
  // audit: COCRDUP-020
  // audit: COCRDUP-023
  // audit: COCRDUP-028
  // audit: COCRDUP-029
  // audit: COCRDUP-030
  it.each([
    { title: "the stopper after ACCTSID", row: 7, columns: [57] },
    { title: "the stopper after CARDSID", row: 8, columns: [62] },
    { title: "the stopper after CRDNAME", row: 11, columns: [76] },
    { title: "the stopper after CRDSTCD", row: 13, columns: [27] },
    { title: "the stopper after EXPYEAR", row: 15, columns: [35] },
    { title: "the dark EXPDAY", row: 15, columns: [36, 37, 38] },
    { title: "the stopper after EXPDAY", row: 15, columns: [39] },
  ])("no element fills $title on row $row", async ({ row, columns }) => {
    stubApi({ [CARDS_EDIT]: [{ status: 200, body: view }] });
    await showFromList();

    const occupied = occupiedColumns(row);
    expect(occupied.size).toBeGreaterThan(0);
    expect(columns.filter((column) => occupied.has(column))).toEqual([]);
  });

  it("a typed search sends the account and card, then shows only the redacted number", async () => {
    const calls = stubApi({
      [CARDS_EDIT]: [{ status: 200, body: view }],
      [CARD_PUT]: [{ status: 200, body: validated }],
    });
    await showFromSearch();

    expect(calls.map((call) => call.route)).toEqual([CARDS_EDIT]);
    expect(Object.fromEntries(calls[0].query)).toEqual({
      acct_id: ACCT_ID,
      card_num: TYPED_CARD,
    });
    expect(shownValues()).toEqual({
      account: ACCT_ID,
      card: MASKED_CARD,
      name: STORED_NAME,
      status: "Y",
      month: "06",
      year: "2030",
    });
    expectEnterable(false, true);
    expect(infoText()).toBe(INFO_SHOWN);
    expectNoFullCardDigits();

    // From here the card is addressed by `card_ref` alone; the typed number is never sent again.
    await validate();
    expect(calls.map((call) => call.route)).toEqual([CARDS_EDIT, CARD_PUT]);
    expect(JSON.stringify(calls[1].body)).not.toContain(TYPED_CARD);
    expectNoFullCardDigits();
  });

  it("a list entry whose card is not found shows the details empty and enterable", async () => {
    const calls = stubApi({
      [CARDS_EDIT]: [{ status: 404, body: notFound }],
      [CARD_PUT]: [{ status: 200, body: validated }],
    });
    renderScreen(FROM_LIST);
    await waitFor(() => {
      expect(errorText()).toBe(notFound.message);
    });

    expect(shownValues()).toEqual({
      account: ACCT_ID,
      card: "",
      name: "",
      status: "",
      month: "",
      year: "",
    });
    expectEnterable(false, true);
    expect(field(LABELS.account).getAttribute("aria-invalid")).toBe("true");
    expectFieldMessage(LABELS.account, notFound.message);
    expect(infoText()).toBe(INFO_SHOWN);

    // ENTER validates against a blank snapshot of the list row's account.
    fireEvent.change(field(LABELS.name), { target: { value: NEW_NAME } });
    fireEvent.click(key("ENTER=Process"));
    await waitFor(() => {
      expect(calls).toHaveLength(2);
    });
    expect(calls[1].route).toBe(CARD_PUT);
    expect(calls[1].body).toEqual({
      aid: "ENTER",
      fields: { embossed_name: NEW_NAME, status: "", expiry_month: "", expiry_year: "" },
      snapshot: {
        state: "show_details",
        acct_id: ACCT_ID,
        embossed_name: "",
        expiration_date: null,
        status: "",
      },
    });
  });
});

describe("COCRDUP keys", () => {
  // audit: COCRDUP-029
  // audit: COCRDUP-031
  // audit: COCRDUP-034
  // audit: COCRDUP-041
  // audit: COCRDUP-043
  // audit: COCRDUPC-005
  // audit: COCRDUPC-045
  // audit: COCRDUPC-046
  it("ENTER validates and asks to confirm, F5 saves, the next key returns to COCRDLI", async () => {
    const calls = stubApi({
      [CARDS_EDIT]: [{ status: 200, body: view }],
      [CARD_PUT]: [
        { status: 200, body: validated },
        { status: 200, body: committed },
      ],
    });
    await showFromList();
    const fields: components["schemas"]["CardUpdateFields"] = {
      embossed_name: NEW_NAME,
      status: "Y",
      expiry_month: "06",
      expiry_year: "2030",
    };

    await validate();
    expect(calls.map((call) => call.route)).toEqual([CARDS_EDIT, CARD_PUT]);
    // The snapshot echoed is endpoint 9's, byte for byte: the expiry day travels in it.
    expect(calls[1].body).toEqual({ aid: "ENTER", fields, snapshot: view.snapshot });
    expect(infoText()).toBe(INFO_CONFIRM);
    expect(messageRow("info").className).toContain("text-card-foreground");
    expect(errorText()).toBe("");
    expectConfirmationKeys(true);
    expectEnterable(false, false);
    expect(field(LABELS.name).value).toBe(NEW_NAME);
    expect(screen.getAllByRole("textbox")).toHaveLength(6);

    fireEvent.click(screen.getByRole("button", { name: "F5=Save" }));
    await waitFor(() => {
      expect(infoText()).toBe(INFO_DONE);
    });
    expect(calls.map((call) => call.route)).toEqual([CARDS_EDIT, CARD_PUT, CARD_PUT]);
    expect(calls[2].body).toEqual({ aid: "PF5", fields, snapshot: validated.snapshot });
    expect(errorText()).toBe("");
    expectConfirmationKeys(false);
    expectEnterable(false, false);

    // State C: any key but F3 starts over, which for the COCRDLI entry is COCRDLI again.
    fireEvent.click(key("ENTER=Process"));
    expect(await screen.findByText("Stub at /cards, state null")).not.toBeNull();
    await settle();
    expect(calls).toHaveLength(3);
  });

  // audit: COCRDUP-041
  it("the Enter key and the form's submission act as the ENTER=Process button", async () => {
    const calls = stubApi({
      [CARDS_EDIT]: [{ status: 200, body: view }],
      [CARD_PUT]: [{ status: 200, body: validated }],
    });
    renderScreen();
    typeSearchKeys(ACCT_ID, TYPED_CARD);

    // Outside a field, the binding of `PfKeyBar` submits the form.
    pressKey("Enter");
    await waitFor(() => {
      expect(field(LABELS.card).value).toBe(MASKED_CARD);
    });
    expect(calls.map((call) => call.route)).toEqual([CARDS_EDIT]);
    expect(Object.fromEntries(calls[0].query)).toEqual({
      acct_id: ACCT_ID,
      card_num: TYPED_CARD,
    });

    // In a field, the browser's implicit submission submits the form itself.
    fireEvent.change(field(LABELS.name), { target: { value: NEW_NAME } });
    fireEvent.submit(screen.getByRole("form", { name: "Update Credit Card Details" }));
    await waitFor(() => {
      expect(infoText()).toBe(INFO_CONFIRM);
    });
    expect(calls.map((call) => call.route)).toEqual([CARDS_EDIT, CARD_PUT]);
    expect(calls[1].body).toMatchObject({ aid: "ENTER", snapshot: view.snapshot });
  });

  // audit: COCRDUP-032
  // audit: COCRDUP-043
  it("a 409 on F5 shows its message over the card's current values fetched again", async () => {
    const calls = stubApi({
      [CARDS_EDIT]: [
        { status: 200, body: view },
        { status: 200, body: changedView },
      ],
      [CARD_PUT]: [
        { status: 200, body: validated },
        { status: 409, body: changedByOther },
      ],
    });
    await showFromList();
    await validate();

    pressKey("F5");
    await waitFor(() => {
      expect(field(LABELS.name).value).toBe(OTHER_NAME);
    });
    expect(calls.map((call) => call.route)).toEqual([CARDS_EDIT, CARD_PUT, CARD_PUT, CARDS_EDIT]);
    expect(Object.fromEntries(calls[3].query)).toEqual({
      acct_id: ACCT_ID,
      card_ref: String(CARD_REF),
    });
    const errorRow = messageRow("error");
    expect(errorRow.textContent).toBe(changedByOther.message);
    expect(errorRow.getAttribute("aria-live")).toBe("polite");
    expect(errorRow.className).toContain("text-destructive");
    expect(field(LABELS.status).value).toBe("N");
    expect(infoText()).toBe(INFO_SHOWN);
    expectEnterable(false, true);
    expectConfirmationKeys(false);
  });

  // audit: COCRDUP-044
  // audit: COCRDUPC-006
  it.each([
    {
      title: "S, clicked",
      puts: [],
      reach: async () => {
        expect(infoText()).toBe(INFO_SHOWN);
      },
      press: () => fireEvent.click(key("F12=Cancel")),
    },
    {
      title: "E, clicked",
      puts: [{ status: 400, body: nameMissing }],
      reach: failNameEdit,
      press: () => fireEvent.click(key("F12=Cancel")),
    },
    {
      title: "N, clicked",
      puts: [{ status: 200, body: validated }],
      reach: validate,
      press: () => fireEvent.click(key("F12=Cancel")),
    },
    {
      title: "N, the F12 key",
      puts: [{ status: 200, body: validated }],
      reach: validate,
      press: () => pressKey("F12"),
    },
    {
      title: "N, Shift+F12 (PF24 folded onto F12)",
      puts: [{ status: 200, body: validated }],
      reach: validate,
      press: () => pressKey("F12", true),
    },
  ])(
    "F12 in state $title discards the edits and shows endpoint 9's values again, no request",
    async ({ puts, reach, press }) => {
      const calls = stubApi({ [CARDS_EDIT]: [{ status: 200, body: view }], [CARD_PUT]: puts });
      await showFromList();
      fireEvent.change(field(LABELS.name), { target: { value: NEW_NAME } });
      fireEvent.change(field(LABELS.month), { target: { value: "11" } });
      await reach();
      const sent = calls.length;

      press();
      await settle();

      expect(calls).toHaveLength(sent);
      expect(shownValues()).toEqual({
        account: ACCT_ID,
        card: MASKED_CARD,
        name: STORED_NAME,
        status: "Y",
        month: "06",
        year: "2030",
      });
      expect(field(LABELS.name).getAttribute("aria-invalid")).toBe("false");
      expect(infoText()).toBe(INFO_SHOWN);
      expect(errorText()).toBe("");
      expectConfirmationKeys(false);
      expectEnterable(false, true);
      await expectFocus(LABELS.name);
    },
  );

  // audit: COCRDUP-042
  // audit: COCRDUPC-004
  it.each([
    {
      title: "in state not fetched, clicked after the menu entry",
      puts: [],
      show: async () => {
        renderScreen();
        typeSearchKeys(ACCT_ID, TYPED_CARD);
      },
      press: () => fireEvent.click(key("F3=Exit")),
      reached: "Stub at /menu, state null",
    },
    {
      title: "in state not fetched, clicked when the recorded caller is no screen",
      puts: [],
      show: async () => {
        renderScreen({ from: "/not-a-screen" });
        typeSearchKeys(ACCT_ID, TYPED_CARD);
      },
      press: () => fireEvent.click(key("F3=Exit")),
      reached: "Stub at /menu, state null",
    },
    {
      title: "in state S, clicked after the COCRDLI entry",
      puts: [],
      show: async () => {
        await showFromList();
        fireEvent.change(field(LABELS.name), { target: { value: NEW_NAME } });
      },
      press: () => fireEvent.click(key("F3=Exit")),
      reached: "Stub at /cards, state null",
    },
    {
      title: "in state S, the F3 key",
      puts: [],
      show: showFromList,
      press: () => pressKey("F3"),
      reached: "Stub at /cards, state null",
    },
    {
      title: "in state S, Shift+F3 (PF15 folded onto F3)",
      puts: [],
      show: showFromList,
      press: () => pressKey("F3", true),
      reached: "Stub at /cards, state null",
    },
    {
      title: "in state E, clicked",
      puts: [{ status: 400, body: nameMissing }],
      show: async () => {
        await showFromList();
        await failNameEdit();
      },
      press: () => fireEvent.click(key("F3=Exit")),
      reached: "Stub at /cards, state null",
    },
    {
      title: "in state N, clicked",
      puts: [{ status: 200, body: validated }],
      show: async () => {
        await showFromList();
        await validate();
      },
      press: () => fireEvent.click(key("F3=Exit")),
      reached: "Stub at /cards, state null",
    },
    {
      title: "in state C, the F3 key",
      puts: [
        { status: 200, body: validated },
        { status: 200, body: committed },
      ],
      show: async () => {
        await showFromList();
        await validate();
        await save(INFO_DONE);
      },
      press: () => pressKey("F3"),
      reached: "Stub at /cards, state null",
    },
    {
      title: "in state L, clicked after the menu entry",
      puts: [
        { status: 200, body: validated },
        { status: 500, body: lockFailed },
      ],
      show: async () => {
        await showFromSearch();
        await validate();
        await save(INFO_FAILED);
      },
      press: () => fireEvent.click(key("F3=Exit")),
      reached: "Stub at /menu, state null",
    },
    {
      title: "in state F, clicked",
      puts: [
        { status: 200, body: validated },
        { status: 500, body: rewriteFailed },
      ],
      show: async () => {
        await showFromList();
        await validate();
        await save(INFO_FAILED);
      },
      press: () => fireEvent.click(key("F3=Exit")),
      reached: "Stub at /cards, state null",
    },
  ])("F3 $title exits to the caller with no request", async ({ puts, show, press, reached }) => {
    const calls = stubApi({ [CARDS_EDIT]: [{ status: 200, body: view }], [CARD_PUT]: puts });
    await show();
    const sent = calls.length;

    press();
    expect(await screen.findByText(reached)).not.toBeNull();
    await settle();
    expect(calls).toHaveLength(sent);
  });

  // audit: COCRDUPC-008
  it.each(["F1", "F2", "F4", "F5=Save", "F6", "F7", "F8", "F9", "F10", "F11", "F12=Cancel"])(
    "before a fetch, %s searches by the typed keys as ENTER",
    async (label) => {
      const calls = stubApi({ [CARDS_EDIT]: [{ status: 200, body: view }] });
      renderScreen();
      typeSearchKeys(ACCT_ID, TYPED_CARD);

      fireEvent.click(key(label));
      await waitFor(() => {
        expect(field(LABELS.card).value).toBe(MASKED_CARD);
      });
      expect(calls.map((call) => call.route)).toEqual([CARDS_EDIT]);
      expect(Object.fromEntries(calls[0].query)).toEqual({
        acct_id: ACCT_ID,
        card_num: TYPED_CARD,
      });
    },
  );

  // audit: COCRDUPC-008
  it.each([
    { title: "F1", press: () => fireEvent.click(key("F1")) },
    { title: "F2", press: () => fireEvent.click(key("F2")) },
    { title: "F4", press: () => fireEvent.click(key("F4")) },
    { title: "F5 outside the confirmation", press: () => fireEvent.click(key("F5=Save")) },
    { title: "F6", press: () => fireEvent.click(key("F6")) },
    { title: "F7", press: () => fireEvent.click(key("F7")) },
    { title: "F8", press: () => fireEvent.click(key("F8")) },
    { title: "F9", press: () => fireEvent.click(key("F9")) },
    { title: "F10", press: () => fireEvent.click(key("F10")) },
    { title: "F11", press: () => fireEvent.click(key("F11")) },
    {
      title: "Shift+F7 (PF19 folded onto F7)",
      press: () => pressKey("F7", true),
    },
  ])("after a fetch, $title validates as ENTER", async ({ press }) => {
    const calls = stubApi({
      [CARDS_EDIT]: [{ status: 200, body: view }],
      [CARD_PUT]: [{ status: 200, body: validated }],
    });
    await showFromList();
    fireEvent.change(field(LABELS.name), { target: { value: NEW_NAME } });

    press();
    await waitFor(() => {
      expect(infoText()).toBe(INFO_CONFIRM);
    });
    expect(calls.map((call) => call.route)).toEqual([CARDS_EDIT, CARD_PUT]);
    expect(calls[1].body).toMatchObject({ aid: "ENTER", snapshot: view.snapshot });
  });

  // audit: COCRDUPC-005
  it.each([
    {
      title: "after a save entered from COCRDLI, ENTER returns to COCRDLI",
      entry: showFromList,
      outcome: committed,
      press: () => fireEvent.click(key("ENTER=Process")),
      typed: false,
      reached: "Stub at /cards, state null",
    },
    {
      title: "after a rewrite failure entered from COCRDLI, F12 returns to COCRDLI",
      entry: showFromList,
      outcome: rewriteFailed,
      press: () => fireEvent.click(key("F12=Cancel")),
      typed: false,
      reached: "Stub at /cards, state null",
    },
    {
      title: "after a save entered from the menu, F7 starts over on the search screen",
      entry: showFromSearch,
      outcome: committed,
      press: () => fireEvent.click(key("F7")),
      typed: false,
      reached: undefined,
    },
    {
      title: "after a lock failure entered from the menu, ENTER ignores the typed keys",
      entry: showFromSearch,
      outcome: lockFailed,
      press: () => fireEvent.click(key("ENTER=Process")),
      typed: true,
      reached: undefined,
    },
  ])("$title, with no request", async ({ entry, outcome, press, typed, reached }) => {
    const calls = stubApi({
      [CARDS_EDIT]: [{ status: 200, body: view }],
      [CARD_PUT]: [
        { status: 200, body: validated },
        { status: outcome === committed ? 200 : 500, body: outcome },
      ],
    });
    await entry();
    await validate();
    await save(outcome === committed ? INFO_DONE : INFO_FAILED);
    if (typed) {
      typeSearchKeys("00000000099", "");
    }

    press();
    if (reached === undefined) {
      expect(shownValues()).toEqual({
        account: "",
        card: "",
        name: "",
        status: "",
        month: "",
        year: "",
      });
      expectEnterable(true, false);
      expect(infoText()).toBe(INFO_SEARCH);
      expect(errorText()).toBe("");
      expectConfirmationKeys(false);
    } else {
      expect(await screen.findByText(reached)).not.toBeNull();
    }
    await settle();
    expect(calls).toHaveLength(3);
  });

  // audit: COCRDUPC-033
  it.each([
    { month: "3", year: "25", sentMonth: "03", sentYear: "0025" },
    { month: "12", year: "2031", sentMonth: "12", sentYear: "2031" },
    { month: "", year: "", sentMonth: "", sentYear: "" },
  ])(
    "ENTER sends month $month and year $year right-justified and zero-filled",
    async ({ month, year, sentMonth, sentYear }) => {
      const calls = stubApi({
        [CARDS_EDIT]: [{ status: 200, body: view }],
        [CARD_PUT]: [{ status: 200, body: validated }],
      });
      await showFromList();
      fireEvent.change(field(LABELS.month), { target: { value: month } });
      fireEvent.change(field(LABELS.year), { target: { value: year } });

      fireEvent.click(key("ENTER=Process"));
      await waitFor(() => {
        expect(infoText()).toBe(INFO_CONFIRM);
      });
      expect(calls[1].body).toMatchObject({
        aid: "ENTER",
        fields: { expiry_month: sentMonth, expiry_year: sentYear },
      });
      expect([field(LABELS.month).value, field(LABELS.year).value]).toEqual([sentMonth, sentYear]);
    },
  );
});

describe("COCRDUP messages", () => {
  // audit: COCRDUPC-046
  it.each([
    {
      title: "a blank account",
      account: "",
      card: "",
      error: accountMissing,
      marked: LABELS.account,
      other: LABELS.card,
      shown: "*",
    },
    {
      title: "a blank card",
      account: ACCT_ID,
      card: "",
      error: cardMissing,
      marked: LABELS.card,
      other: LABELS.account,
      shown: "*",
    },
    {
      title: "an account that is not 11 digits",
      account: "5",
      card: TYPED_CARD,
      error: accountNotNumeric,
      marked: LABELS.account,
      other: LABELS.card,
      shown: "5",
    },
  ])(
    "a search error on $title marks that field and puts the cursor there",
    async ({ account, card, error, marked, other, shown }) => {
      const calls = stubApi({ [CARDS_EDIT]: [{ status: 400, body: error }] });
      renderScreen();
      typeSearchKeys(account, card);

      fireEvent.click(key("ENTER=Process"));
      await waitFor(() => {
        expect(errorText()).toBe(error.message);
      });
      expect(calls.map((call) => call.route)).toEqual([CARDS_EDIT]);
      expect(field(marked).value).toBe(shown);
      expect(field(marked).getAttribute("aria-invalid")).toBe("true");
      expectFieldMessage(marked, error.message);
      expect(field(other).getAttribute("aria-invalid")).toBe("false");
      await expectFocus(marked);
      expect(infoText()).toBe(INFO_SEARCH);
      expectEnterable(true, false);
      expectConfirmationKeys(false);
    },
  );

  // audit: COCRDUPC-005
  // audit: COCRDUPC-045
  // audit: COCRDUPC-046
  it.each([
    {
      title: "a failing detail edit leaves state E with the name marked",
      confirm: false,
      name: "",
      reply: { status: 400, body: nameMissing },
      info: INFO_CHANGES,
      keys: false,
      details: true,
      shownName: "*",
      invalid: "true",
      focus: LABELS.name,
      echoed: "changes_not_ok",
    },
    {
      title: "no change on ENTER keeps state S with the cursor on the name, unmarked",
      confirm: false,
      name: STORED_NAME,
      reply: { status: 400, body: noChange },
      info: INFO_SHOWN,
      keys: false,
      details: true,
      shownName: STORED_NAME,
      invalid: "false",
      focus: LABELS.name,
      echoed: "show_details",
    },
    {
      title: "no change on F5 answers as ENTER in state S",
      confirm: true,
      name: NEW_NAME,
      reply: { status: 400, body: noChange },
      info: INFO_SHOWN,
      keys: false,
      details: true,
      shownName: NEW_NAME,
      invalid: "false",
      focus: LABELS.name,
      echoed: "show_details",
    },
    {
      title: "a lock failure on F5 leaves state L",
      confirm: true,
      name: NEW_NAME,
      reply: { status: 500, body: lockFailed },
      info: INFO_FAILED,
      keys: true,
      details: false,
      shownName: NEW_NAME,
      invalid: "false",
      focus: LABELS.account,
      echoed: undefined,
    },
    {
      title: "a rewrite failure on F5 leaves state F",
      confirm: true,
      name: NEW_NAME,
      reply: { status: 500, body: rewriteFailed },
      info: INFO_FAILED,
      keys: true,
      details: false,
      shownName: NEW_NAME,
      invalid: "false",
      focus: LABELS.account,
      echoed: undefined,
    },
    {
      title: "any other failure keeps state S",
      confirm: false,
      name: NEW_NAME,
      reply: { status: 503, body: storeUnavailable },
      info: INFO_SHOWN,
      keys: false,
      details: true,
      shownName: NEW_NAME,
      invalid: "false",
      focus: undefined,
      echoed: "show_details",
    },
  ])(
    "$title",
    async ({ confirm, name, reply, info, keys, details, shownName, invalid, focus, echoed }) => {
      const puts = confirm
        ? [{ status: 200, body: validated }, reply, { status: 200, body: validated }]
        : [reply, { status: 200, body: validated }];
      const calls = stubApi({ [CARDS_EDIT]: [{ status: 200, body: view }], [CARD_PUT]: puts });
      await showFromList();
      fireEvent.change(field(LABELS.name), { target: { value: name } });
      fireEvent.click(key("ENTER=Process"));
      if (confirm) {
        await waitFor(() => {
          expect(key("F5=Save").hidden).toBe(false);
        });
        fireEvent.click(key("F5=Save"));
      }

      await waitFor(() => {
        expect(errorText()).toBe(reply.body.message);
      });
      expect(infoText()).toBe(info);
      expectEnterable(keys, details);
      expectConfirmationKeys(false);
      expect(field(LABELS.name).value).toBe(shownName);
      expect(field(LABELS.name).getAttribute("aria-invalid")).toBe(invalid);
      if (invalid === "true") {
        expectFieldMessage(LABELS.name, reply.body.message);
      }
      if (focus !== undefined) {
        await expectFocus(focus);
      }
      const sent = calls.length;

      // The next ENTER echoes the state the error left; after L and F it starts over instead.
      fireEvent.click(key("ENTER=Process"));
      if (echoed === undefined) {
        expect(await screen.findByText("Stub at /cards, state null")).not.toBeNull();
        await settle();
        expect(calls).toHaveLength(sent);
      } else {
        await waitFor(() => {
          expect(calls).toHaveLength(sent + 1);
        });
        expect(calls[sent].body).toMatchObject({ aid: "ENTER", snapshot: { state: echoed } });
      }
    },
  );

  it("a name over its 50 characters is refused on the screen without a request", async () => {
    const calls = stubApi({ [CARDS_EDIT]: [{ status: 200, body: view }] });
    await showFromList();
    fireEvent.change(field(LABELS.name), { target: { value: "A".repeat(51) } });

    fireEvent.click(key("ENTER=Process"));
    await waitFor(() => {
      expect(errorText()).toBe("String must contain at most 50 character(s)");
    });
    expectFieldMessage(LABELS.name, "String must contain at most 50 character(s)");
    await settle();
    expect(calls).toHaveLength(1);
    await expectFocus(LABELS.name);
    expect(infoText()).toBe(INFO_SHOWN);
  });
});

// Every endpoint 10 request carries the stored session's token, and none a query: the path's
// `card_ref` alone names the card.
describe("COCRDUP: endpoint 10 requests", () => {
  it("ENTER and F5 each PUT the card's path with the bearer token and no query, and the save shows", async () => {
    const calls = stubApi({
      [CARDS_EDIT]: [{ status: 200, body: view }],
      [CARD_PUT]: [
        { status: 200, body: validated },
        { status: 200, body: committed },
      ],
    });
    await showFromList();
    await validate();
    await save(INFO_DONE);

    expect(
      calls.map(({ route, query, authorization }) => ({
        route,
        query: Object.fromEntries(query),
        authorization,
      })),
    ).toEqual([
      {
        route: CARDS_EDIT,
        query: { acct_id: ACCT_ID, card_ref: String(CARD_REF) },
        authorization: "Bearer test-token",
      },
      { route: CARD_PUT, query: {}, authorization: "Bearer test-token" },
      { route: CARD_PUT, query: {}, authorization: "Bearer test-token" },
    ]);
  });
});

/** The 401 of the `SignedOnUser` extractor for a missing, invalid or expired token. */
const signonRequired: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};

/**
 * Renders the application's route table at `/cards/update`, so the sign-on guard that only the
 * table mounts stands in front of the screen. Its screens load lazily, so a wait for one to
 * render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COCRDUP] });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

/** Searches by the typed keys with ENTER and waits for endpoint 9's card (state `S`). */
async function searchShown(): Promise<void> {
  typeSearchKeys(ACCT_ID, TYPED_CARD);
  fireEvent.click(key("ENTER=Process"));
  await waitFor(() => {
    expect(field(LABELS.card).value).toBe(MASKED_CARD);
  });
}

// A 401 clears the stored session (`api/client.ts`), and the sign-on guard in front of the route
// replaces the still-mounted screen with sign-on at once: after the key that sent the refused
// request, the test presses no key and follows no link.
describe("COCRDUP in the route table: a 401 ends the session", () => {
  it.each<{
    title: string;
    replies: Parameters<typeof stubApi>[0];
    drive: () => Promise<void>;
    sent: string[];
  }>([
    {
      title: "the fetch of ENTER (endpoint 9)",
      replies: { [CARDS_EDIT]: [{ status: 401, body: signonRequired }] },
      drive: async () => {
        typeSearchKeys(ACCT_ID, TYPED_CARD);
        fireEvent.click(key("ENTER=Process"));
      },
      sent: [CARDS_EDIT],
    },
    {
      title: "the validation of ENTER (endpoint 10)",
      replies: {
        [CARDS_EDIT]: [{ status: 200, body: view }],
        [CARD_PUT]: [{ status: 401, body: signonRequired }],
      },
      drive: async () => {
        await searchShown();
        fireEvent.change(field(LABELS.name), { target: { value: NEW_NAME } });
        fireEvent.click(key("ENTER=Process"));
      },
      sent: [CARDS_EDIT, CARD_PUT],
    },
    {
      title: "the save of F5 (endpoint 10)",
      replies: {
        [CARDS_EDIT]: [{ status: 200, body: view }],
        [CARD_PUT]: [
          { status: 200, body: validated },
          { status: 401, body: signonRequired },
        ],
      },
      drive: async () => {
        await searchShown();
        await validate();
        fireEvent.click(key("F5=Save"));
      },
      sent: [CARDS_EDIT, CARD_PUT, CARD_PUT],
    },
  ])(
    "$title answered 401 replaces the screen with sign-on and clears the session, with no retry",
    async ({ replies, drive, sent }) => {
      const calls = stubApi(replies);
      const router = renderRouteTable();
      expect(
        await screen.findByRole(
          "heading",
          { level: 1, name: "Update Credit Card Details" },
          { timeout: 5_000 },
        ),
      ).toBeDefined();

      await drive();

      expect(
        await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
      ).toBeDefined();
      expect(router.state.location.pathname).toBe(PATHS.COSGN00);
      expect(
        screen.queryByRole("heading", { level: 1, name: "Update Credit Card Details" }),
      ).toBeNull();
      expect(getSession()).toBeNull();
      expect(calls.map((call) => call.route)).toEqual(sent);
      expect(calls.every((call) => call.authorization === "Bearer test-token")).toBe(true);
      router.dispose();
    },
  );
});
