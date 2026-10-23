import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createMemoryRouter, MemoryRouter, Route, RouterProvider, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { errorStatus } from "@/api/hooks";
import type { components } from "@/api/schema";
import { getSession, setSession } from "@/auth/session";
import { PATHS, routes } from "@/routes";
import { COCRDSL } from "./COCRDSL";

/**
 * Render test of COCRDSL (`/cards/view`, CCDL / COCRDSLC, map `CCRDSLA`) in jsdom. The global
 * `fetch` is a stub that answers endpoint 8 in process, so no request leaves the test.
 *
 * Every card number a response carries is redacted, as endpoint 8 returns it to a `U` token. The
 * two typed card numbers are synthetic request input, built from parts, that fail the card-number
 * check digit.
 */

const TOKEN = "test-token";
const ACCT_ID = "00000000050";
const TYPED_CARD = "12345678".repeat(2);
const OTHER_TYPED_CARD = "87654321".repeat(2);

/**
 * Header title rows 1 and 2: the 40-byte `CCDA-TITLE01` and `CCDA-TITLE02` of
 * `app/cpy/COTTL01Y.cpy:18-22`, padding kept, as COCRDSL-033 and COCRDSL-036 quote them.
 */
const TITLE01 = `${" ".repeat(6)}AWS Mainframe Modernization${" ".repeat(7)}`;
const TITLE02 = `${" ".repeat(14)}CardDemo${" ".repeat(18)}`;

/** The info row before any request and after every error (COCRDSLC-025). */
const PROMPT = "Please enter Account and Card Number";

/** The information message of a found card, three leading spaces kept (COCRDSLC-020). */
const FOUND_INFO = "   Displaying requested details";

/** Route state of a COCRDLI `S` selection: the caller and the row's account and card handle. */
const FROM_LIST = { from: PATHS.COCRDLI, acct_id: ACCT_ID, card_ref: 1 };

/** The labels of the inputs as Testing Library reads them, runs of spaces collapsed. */
const ACCOUNT = "Account Number :";
const CARD = "Card Number :";
const NAME = "Name on card :";
const STATUS = "Card Active Y/N :";
const MONTH = "Expiry Date :";
const YEAR = "Expiry year";

const LIST_STUB = "COCRDLI stub";
const MENU_STUB = "COMEN01 stub";

const view: components["schemas"]["CardViewResponse"] = {
  acct_id: ACCT_ID,
  card_num: "************5740",
  card_ref: 1,
  embossed_name: "TEST CARDHOLDER",
  expiration_date: "2025-03-31",
  info_message: FOUND_INFO,
  message: null,
  status: "Y",
};

const undatedView: components["schemas"]["CardViewResponse"] = {
  ...view,
  expiration_date: null,
};

const typedView: components["schemas"]["CardViewResponse"] = {
  ...view,
  card_num: "************5678",
  card_ref: 2,
};

const notFound: components["schemas"]["ScreenError"] = {
  message: "Did not find cards for this search condition",
  field: "acct_id",
};

const noInput: components["schemas"]["ScreenError"] = {
  message: "No input received",
  field: "acct_id",
};

const cardNotProvided: components["schemas"]["ScreenError"] = {
  message: "Card number not provided",
  field: "card_num",
};

/**
 * The 500 of a failed card read (COCRDSLC-016): `WS-FILE-ERROR-MESSAGE` in its 75 bytes, the RESP
 * and RESP2 fields left blank, as their values are halted (COCRDSLC-017).
 */
const fileError: components["schemas"]["ScreenError"] = {
  message: [
    "File Error: ",
    "READ".padEnd(8),
    " on ",
    "CARDDAT".padEnd(9),
    " returned RESP ",
    " ".repeat(10),
    ",RESP2 ",
    " ".repeat(10),
  ].join(""),
  field: "acct_id",
};

/** Replies with a JSON body, as the API sends every success and every `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** The reply of a test that expects no request to endpoint 8. */
function unexpected(): Response {
  return json(500, { message: "No request was expected" });
}

/**
 * Replaces the global `fetch` with a stub that records every request and answers endpoint 8 with
 * `answer`. Any other request gets a 500.
 */
function stubFetch(answer: (query: URLSearchParams) => Response | Promise<Response>) {
  const seen: {
    method: string;
    path: string;
    query: URLSearchParams;
    authorization: string | null;
  }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      const url = new URL(req.url);
      seen.push({
        method: req.method,
        path: url.pathname,
        query: url.searchParams,
        authorization: req.headers.get("Authorization"),
      });
      if (req.method === "GET" && url.pathname === "/api/v1/cards/view") {
        return answer(url.searchParams);
      }
      return json(500, { message: `Unexpected request ${req.method} ${url.pathname}` });
    }),
  );
  return seen;
}

/** The query of every request, each checked to be endpoint 8. */
function sentQueries(seen: ReturnType<typeof stubFetch>): Record<string, string>[] {
  return seen.map((request) => {
    expect(`${request.method} ${request.path}`).toBe("GET /api/v1/cards/view");
    return Object.fromEntries(request.query);
  });
}

/**
 * Renders the screen at its route with `state`, and a text stub at each F3 target. Returns the
 * screen's `QueryClient`, whose mutation cache holds what each ENTER's read threw.
 */
function renderScreen(state?: unknown): QueryClient {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COCRDSL, state }]}>
        <Routes>
          <Route path={PATHS.COCRDSL} element={<COCRDSL />} />
          <Route path={PATHS.COCRDLI} element={<p>{LIST_STUB}</p>} />
          <Route path={PATHS.COMEN01} element={<p>{MENU_STUB}</p>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return client;
}

/** What the last ENTER's read threw, as `useCocrdslcView` handed it to the screen. */
function lastFailure(
  client: QueryClient,
): components["schemas"]["ScreenError"] | null | undefined {
  return client.getMutationCache().getAll().at(-1)?.state.error;
}

/** An input or output of the screen by its label. */
function field(label: string): HTMLInputElement {
  return screen.getByLabelText<HTMLInputElement>(label);
}

/** The name, status, expiry month and expiry year the screen shows. */
function cardDetails(): string[] {
  return [NAME, STATUS, MONTH, YEAR].map((label) => field(label).value);
}

/** Sets the text of the input labelled `label`, as typing would. */
function typeInto(label: string, value: string): void {
  fireEvent.change(field(label), { target: { value } });
}

/** The screen's form, which ENTER submits. */
function form(): HTMLFormElement {
  const element = document.querySelector("form");
  if (element === null) {
    throw new Error("The screen renders no form");
  }
  return element;
}

/** The `ENTER=Search Cards` key, the submit button of the form. */
function enterButton(): HTMLElement {
  return screen.getByRole("button", { name: "ENTER=Search Cards" });
}

/** The one message row of `kind`. */
function messageRow(kind: "error" | "info"): HTMLElement {
  const rows = screen.getAllByRole("alert").filter((row) => row.dataset.messageKind === kind);
  expect(rows).toHaveLength(1);
  return rows[0];
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
  const [input] = fields;
  if (!(input instanceof HTMLInputElement)) {
    throw new Error(`The header has no input ${name}`);
  }
  return input;
}

/** The value of the header field `name`, padding kept. */
function headerValue(name: string): string {
  return headerInput(name).value;
}

/** Waits until `count` requests were sent and the last one has been answered. */
async function requestsSettled(seen: ReturnType<typeof stubFetch>, count: number): Promise<void> {
  await waitFor(() => {
    expect(seen).toHaveLength(count);
  });
  await waitFor(() => {
    expect(form().getAttribute("aria-busy")).toBe("false");
  });
}

/** The key row, `PfKeyBar`'s group. */
function keyRow(): HTMLElement {
  return screen.getByRole("group", { name: "Function keys" });
}

/** Whether each key-row button is disabled, by its label. */
function disabledKeys(): Record<string, boolean> {
  return Object.fromEntries(
    within(keyRow())
      .getAllByRole("button")
      .map((button) => [button.textContent ?? "", button.getAttribute("aria-disabled") === "true"]),
  );
}

/** Answers every endpoint 8 request with `reply`, but only once `open` has been called. */
function heldReplies(reply: components["schemas"]["CardViewResponse"]) {
  let release = (): void => {};
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const seen = stubFetch(async () => {
    await held;
    return json(200, reply);
  });
  return {
    seen,
    open: () => {
      release();
    },
  };
}

/** Clicks `ENTER=Search Cards` and waits until the error row shows `text` and the keys are free. */
async function enterUntil(text: string): Promise<void> {
  fireEvent.click(enterButton());
  await waitFor(() => {
    expect(messageRow("error").textContent).toBe(text);
    expect(form().getAttribute("aria-busy")).toBe("false");
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

/** How assistive technology reads an input: its `aria-invalid` and what describes it. */
type ReadOut = {
  invalid: string | null;
  described: ({ text: string | null; hidden: boolean } | null)[] | null;
};

/**
 * How assistive technology reads `element`: its `aria-invalid`, and the element each id of its
 * `aria-describedby` names, as its text and whether `sr-only` hides it (`null` for an id that
 * resolves to nothing). `described` is `null` when the input has no `aria-describedby`.
 */
function readOut(element: HTMLElement): ReadOut {
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

/**
 * An input as assistive technology reads it: unmarked, valid and described by nothing, as
 * neither input has a hint; or marked by `message`, invalid and described by its hidden field
 * message.
 */
function readOutOf(message: string | undefined): ReadOut {
  return message === undefined
    ? { invalid: "false", described: null }
    : { invalid: "true", described: [{ text: message, hidden: true }] };
}

/** Every element that shows exactly `text` and is not hidden by `sr-only`. */
function visibleCopies(text: string): HTMLElement[] {
  return screen
    .queryAllByText(text, { normalizer: (content) => content })
    .filter((element) => element.closest(".sr-only") === null);
}


describe("COCRDSL", () => {
  beforeEach(() => {
    setSession(TOKEN, "USER0001", "U");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  it("shows the screen title, the two inputs and the key row of the map", () => {
    stubFetch(unexpected);
    renderScreen();

    // audit: COCRDSL-011
    const title = screen.getByRole("heading", { level: 1 });
    expect(title.textContent).toBe("View Credit Card Detail");
    expect(title.dataset.slot).toBe("card-title");
    // The map has no other field on the title's line, so the title stands alone on it.
    expect(title.parentElement?.dataset.slot).toBe("card-header");

    // The lengths are `both` rows, whose client half the schema test proves.
    expect(field(ACCOUNT).maxLength).toBe(11);
    expect(field(ACCOUNT).parentElement?.className).toBe("row-start-7 col-start-45 col-span-11");
    expect(field(CARD).maxLength).toBe(16);
    expect(field(CARD).parentElement?.className).toBe("row-start-8 col-start-45 col-span-16");

    // audit: COCRDSL-031
    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(keys.map((key) => key.textContent)).toEqual(["ENTER=Search Cards", "F3=Exit"]);
    expect(keys.map((key) => key.dataset.slot)).toEqual(["button", "button"]);
  });

  it.each([
    // audit: COCRDSL-012
    {
      id: "COCRDSL-012",
      label: ACCOUNT,
      text: "Account Number    :",
      cell: "row-start-7 col-start-23 col-span-19",
      slot: "form-label",
    },
    // audit: COCRDSL-015
    {
      id: "COCRDSL-015",
      label: CARD,
      text: "Card Number       :",
      cell: "row-start-8 col-start-23 col-span-19",
      slot: "form-label",
    },
    // audit: COCRDSL-018
    {
      id: "COCRDSL-018",
      label: NAME,
      text: "Name on card      :",
      cell: "row-start-11 col-start-4 col-span-20",
      slot: "label",
    },
    // audit: COCRDSL-021
    {
      id: "COCRDSL-021",
      label: STATUS,
      text: "Card Active Y/N   : ",
      cell: "row-start-13 col-start-4 col-span-20",
      slot: "label",
    },
    // audit: COCRDSL-024
    {
      id: "COCRDSL-024",
      label: MONTH,
      text: "Expiry Date       : ",
      cell: "row-start-15 col-start-4 col-span-20",
      slot: "label",
    },
    // audit: COCRDSL-026
    {
      id: "COCRDSL-026",
      label: YEAR,
      text: "/",
      cell: "row-start-15 col-start-28 col-span-1",
      slot: "label",
    },
  ])("$id labels its field with the map literal at its POS", ({ label, text, cell, slot }) => {
    stubFetch(unexpected);
    renderScreen();

    const labels = Array.from(field(label).labels ?? []);
    expect(labels.map((element) => element.textContent)).toEqual([text]);
    expect(labels[0].dataset.slot).toBe(slot);
    expect(labels[0].parentElement?.className).toBe(cell);
  });

  it.each([
    // audit: COCRDSL-019
    {
      id: "COCRDSL-019",
      label: NAME,
      cell: "row-start-11 col-start-25 col-span-50",
      size: 50,
      value: "TEST CARDHOLDER",
    },
    // audit: COCRDSL-022
    {
      id: "COCRDSL-022",
      label: STATUS,
      cell: "row-start-13 col-start-25 col-span-1",
      size: 1,
      value: "Y",
    },
    // audit: COCRDSL-025
    {
      id: "COCRDSL-025",
      label: MONTH,
      cell: "row-start-15 col-start-25 col-span-2",
      size: 2,
      value: "03",
    },
    // audit: COCRDSL-027
    {
      id: "COCRDSL-027",
      label: YEAR,
      cell: "row-start-15 col-start-30 col-span-4",
      size: 4,
      value: "2025",
    },
  ])(
    "$id shows its endpoint 8 value in a read-only output of the map LENGTH",
    async ({ label, cell, size, value }) => {
      const seen = stubFetch(() => json(200, view));
      renderScreen(FROM_LIST);

      const output = field(label);
      expect(output.dataset.slot).toBe("input");
      expect(output.readOnly).toBe(true);
      // `ASKIP`, which the 3270 cursor skips: out of the Tab order.
      expect(output.tabIndex).toBe(-1);
      expect(output.size).toBe(size);
      expect(output.parentElement?.className).toBe(cell);
      await requestsSettled(seen, 1);
      expect(output.value).toBe(value);
    },
  );

  it.each([
    { label: ACCOUNT, length: 11 },
    { label: CARD, length: 16 },
    { label: NAME, length: 50 },
    { label: STATUS, length: 1 },
    { label: MONTH, length: 2 },
    { label: YEAR, length: 4 },
  ])("the box of $label is the shared text-sm field box of LENGTH $length", ({
    label,
    length,
  }) => {
    stubFetch(unexpected);
    renderScreen();

    const box = field(label);
    expect(box.classList.contains(`field-length-${length}`)).toBe(true);
    expect(box.classList.contains("text-sm")).toBe(true);
    expect(box.classList.contains("text-base")).toBe(false);
  });

  it("leaves no element at the zero-length stoppers that end the fields", () => {
    stubFetch(unexpected);
    renderScreen();

    // audit: COCRDSL-014
    expect(document.querySelector(".row-start-7.col-start-57")).toBeNull();
    // audit: COCRDSL-017
    expect(document.querySelector(".row-start-8.col-start-62")).toBeNull();
    // audit: COCRDSL-020
    expect(document.querySelector(".row-start-11.col-start-76")).toBeNull();
    // audit: COCRDSL-023
    expect(document.querySelector(".row-start-13.col-start-27")).toBeNull();
    // audit: COCRDSL-028
    expect(document.querySelector(".row-start-15.col-start-35")).toBeNull();
  });

  it("shows the header rows of every send", () => {
    stubFetch(unexpected);
    renderScreen();

    /*
     * The header labels render as `Label`s and the value fields as read-only `Input`s. NAV tags
     * the label rows, and the next case the value rows; this one tags the content the program
     * moves into each value.
     */
    expect(screen.getByText("Tran:").parentElement?.className).toBe(
      "row-start-1 col-start-1 col-span-5",
    );
    expect(screen.getByText("Date:").parentElement?.className).toBe(
      "row-start-1 col-start-65 col-span-5",
    );
    expect(screen.getByText("Prog:").parentElement?.className).toBe(
      "row-start-2 col-start-1 col-span-5",
    );
    expect(screen.getByText("Time:").parentElement?.className).toBe(
      "row-start-2 col-start-65 col-span-5",
    );
    // audit: COCRDSL-032
    expect(headerValue("TRNNAME")).toBe("CCDL");
    // audit: COCRDSL-033
    expect(TITLE01).toHaveLength(40);
    expect(headerValue("TITLE01")).toBe(TITLE01);
    // audit: COCRDSL-034
    expect(headerValue("CURDATE")).toMatch(/^[0-9]{2}\/[0-9]{2}\/[0-9]{2}$/);
    // audit: COCRDSL-035
    expect(headerValue("PGMNAME")).toBe("COCRDSLC");
    // audit: COCRDSL-036
    expect(TITLE02).toHaveLength(40);
    expect(headerValue("TITLE02")).toBe(TITLE02);
    // audit: COCRDSL-037
    expect(headerValue("CURTIME")).toMatch(/^[0-9]{2}:[0-9]{2}:[0-9]{2}$/);
  });

  it.each([
    // audit: COCRDSL-002
    {
      id: "COCRDSL-002",
      name: "TRNNAME",
      row: 1,
      col: 7,
      length: 4,
      label: "Tran:",
      shows: "CCDL",
    },
    // audit: COCRDSL-003
    {
      id: "COCRDSL-003",
      name: "TITLE01",
      row: 1,
      col: 21,
      length: 40,
      label: null,
      shows: TITLE01,
    },
    // audit: COCRDSL-005
    {
      id: "COCRDSL-005",
      name: "CURDATE",
      row: 1,
      col: 71,
      length: 8,
      label: "Date:",
      shows: expect.stringMatching(/^[0-9]{2}\/[0-9]{2}\/[0-9]{2}$/),
    },
    // audit: COCRDSL-007
    {
      id: "COCRDSL-007",
      name: "PGMNAME",
      row: 2,
      col: 7,
      length: 8,
      label: "Prog:",
      shows: "COCRDSLC",
    },
    // audit: COCRDSL-008
    {
      id: "COCRDSL-008",
      name: "TITLE02",
      row: 2,
      col: 21,
      length: 40,
      label: null,
      shows: TITLE02,
    },
    // audit: COCRDSL-010
    {
      id: "COCRDSL-010",
      name: "CURTIME",
      row: 2,
      col: 71,
      length: 8,
      label: "Time:",
      shows: expect.stringMatching(/^[0-9]{2}:[0-9]{2}:[0-9]{2}$/),
    },
  ])(
    "$id shows $name in a read-only Input at ($row,$col), $length wide, in the card header",
    ({ name, row, col, length, label, shows }) => {
      // The header value is an output field of the map: a read-only `Input` that holds `LENGTH`
      // characters (`size`), in the cell of its `POS` and `LENGTH` in the frame's title rows. The
      // map's literal before it is its bound `Label`; a title, which no literal labels, is named
      // by `aria-label`.
      const seen = stubFetch(unexpected);
      renderScreen();

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
      expect(seen).toEqual([]);
    },
  );

  it.each([
    { title: "without route state", state: undefined },
    { title: "from the main menu", state: { from: PATHS.COMEN01 } },
    // The menu forwards the route state it received, card-list handles included.
    {
      title: "from the main menu with an account and card a card-list visit left",
      state: { ...FROM_LIST, from: PATHS.COMEN01 },
    },
    {
      title: "with an account and card but no caller",
      state: { acct_id: ACCT_ID, card_ref: 1 },
    },
    {
      title: "when the account exceeds 11 characters",
      state: { ...FROM_LIST, acct_id: "0".repeat(12) },
    },
    { title: "when the account is not text", state: { ...FROM_LIST, acct_id: 50 } },
    { title: "when the card_ref is not a number", state: { ...FROM_LIST, card_ref: "1" } },
    { title: "when the card_ref is not an integer", state: { ...FROM_LIST, card_ref: 1.5 } },
  ])("opens empty and sends nothing $title", ({ state }) => {
    // audit: COCRDSLC-001
    // audit: COCRDSLC-025
    const seen = stubFetch(unexpected);
    renderScreen(state);

    expect(seen).toEqual([]);
    expect(field(ACCOUNT).value).toBe("");
    expect(field(ACCOUNT).readOnly).toBe(false);
    expect(field(CARD).value).toBe("");
    expect(field(CARD).readOnly).toBe(false);
    expect([field(ACCOUNT).tabIndex, field(CARD).tabIndex]).toEqual([0, 0]);
    expect(document.activeElement).toBe(field(ACCOUNT));
    expect(cardDetails()).toEqual(["", "", "", ""]);
    expect(messageRow("error").textContent).toBe("");
    const info = messageRow("info");
    expect(info.textContent).toBe(PROMPT);
    expect(info.className).not.toContain("text-destructive");
  });

  it.each([
    { title: "with an expiry date", response: view, month: "03", year: "2025" },
    { title: "without an expiry date", response: undatedView, month: "", year: "" },
  ])(
    "reads the card a card-list selection names at once, $title",
    async ({ response, month, year }) => {
      // audit: COCRDSLC-002
      // audit: COCRDSLC-024
      const seen = stubFetch(() => json(200, response));
      renderScreen(FROM_LIST);

      expect(field(ACCOUNT).value).toBe(ACCT_ID);
      await requestsSettled(seen, 1);
      expect(sentQueries(seen)).toEqual([{ acct_id: ACCT_ID, card_ref: "1" }]);
      expect(seen[0].authorization).toBe(`Bearer ${TOKEN}`);

      expect(field(ACCOUNT).value).toBe(ACCT_ID);
      expect(field(CARD).value).toBe("************5740");
      expect(field(NAME).value).toBe("TEST CARDHOLDER");
      expect(field(STATUS).value).toBe("Y");
      expect(field(MONTH).value).toBe(month);
      expect(field(YEAR).value).toBe(year);
      expect(screen.queryByDisplayValue("31")).toBeNull();
      // Protected for the visit (`DFHBMPRF`): read-only and skipped by Tab.
      for (const label of [ACCOUNT, CARD]) {
        expect(field(label).readOnly).toBe(true);
        expect(field(label).tabIndex).toBe(-1);
        expect(field(label).className).not.toContain("text-destructive");
      }
      expect(messageRow("info").textContent).toBe(FOUND_INFO);
      expect(messageRow("error").textContent).toBe("");
    },
  );

  it("shows no information box during the entry read, then the response's message", async () => {
    // audit: COCRDSL-029
    let release = (): void => {};
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const seen = stubFetch(async () => {
      await held;
      return json(200, view);
    });
    renderScreen(FROM_LIST);

    // Entered from the card list, no message is set until endpoint 8 answers.
    await waitFor(() => {
      expect(form().getAttribute("aria-busy")).toBe("true");
    });
    const pending = messageRow("info");
    expect(pending.textContent).toBe("");
    expect(pending.className).toContain("border-transparent");

    release();
    await requestsSettled(seen, 1);

    const info = messageRow("info");
    expect(info.dataset.slot).toBe("alert");
    expect(info.className).toContain("text-card-foreground");
    expect(info.className).not.toContain("text-destructive");
    expect(info.className).not.toContain("border-transparent");
    const description = info.querySelector('[data-slot="alert-description"]');
    expect(description?.textContent).toBe(FOUND_INFO);
    expect(FOUND_INFO.length).toBeLessThanOrEqual(40);
  });

  it("looks up a typed card number, then reads the shown card again by its card_ref", async () => {
    // audit: COCRDSL-038
    // audit: COCRDSLC-026
    const seen = stubFetch(() => json(200, typedView));
    renderScreen();

    const enter = enterButton();
    expect(enter.getAttribute("type")).toBe("submit");
    expect(enter.dataset.variant).toBe("default");
    expect(enter.getAttribute("form")).toBe(form().id);

    typeInto(ACCOUNT, ACCT_ID);
    typeInto(CARD, TYPED_CARD);
    fireEvent.click(enter);
    await requestsSettled(seen, 1);
    expect(field(CARD).value).toBe("************5678");
    expect(messageRow("info").textContent).toBe(FOUND_INFO);

    // Unchanged, the card input names the card shown: its redacted number is never sent.
    fireEvent.click(enter);
    await requestsSettled(seen, 2);

    // Changed, the card input is sent as typed.
    typeInto(CARD, OTHER_TYPED_CARD);
    fireEvent.click(enter);
    await requestsSettled(seen, 3);

    expect(sentQueries(seen)).toEqual([
      { acct_id: ACCT_ID, card_num: TYPED_CARD },
      { acct_id: ACCT_ID, card_ref: "2" },
      { acct_id: ACCT_ID, card_num: OTHER_TYPED_CARD },
    ]);
    expect(seen.some((request) => request.query.get("card_num")?.includes("*"))).toBe(false);
  });

  it("shows a not-found ScreenError in the error row and marks both inputs", async () => {
    // audit: COCRDSL-030
    // audit: COCRDSLC-025
    // audit: COCRDSLC-027
    const seen = stubFetch((query) =>
      query.has("card_num") ? json(404, notFound) : json(400, cardNotProvided),
    );
    const client = renderScreen();

    const error = messageRow("error");
    expect(error.textContent).toBe("");
    expect(error.dataset.slot).toBe("alert");
    expect(error.getAttribute("aria-live")).toBe("polite");
    expect(error.className).toContain("text-destructive");
    // The info row (map line 20), the error row (line 23) and the key row (line 24), in order.
    expect(Array.from(document.querySelectorAll('[role="alert"], [role="group"]'))).toEqual([
      messageRow("info"),
      error,
      screen.getByRole("group", { name: "Function keys" }),
    ]);

    // A card edit error first, which puts the cursor in the card input.
    typeInto(ACCOUNT, ACCT_ID);
    fireEvent.click(enterButton());
    await requestsSettled(seen, 1);
    expect(document.activeElement).toBe(field(CARD));

    typeInto(CARD, TYPED_CARD);
    fireEvent.click(enterButton());
    await requestsSettled(seen, 2);

    expect(sentQueries(seen)).toEqual([
      { acct_id: ACCT_ID },
      { acct_id: ACCT_ID, card_num: TYPED_CARD },
    ]);
    expect(messageRow("error").textContent).toBe(notFound.message);
    expect(messageRow("info").textContent).toBe(PROMPT);
    expect(field(ACCOUNT).value).toBe(ACCT_ID);
    expect(field(CARD).value).toBe(TYPED_CARD);
    expect(field(ACCOUNT).className).toContain("text-destructive");
    expect(field(CARD).className).toContain("text-destructive");
    expect(document.activeElement).toBe(field(ACCOUNT));
    expect(cardDetails()).toEqual(["", "", "", ""]);
    // The body reaches the screen as the API sent it, and its status travels beside it.
    const failure = lastFailure(client);
    expect(failure).toEqual(notFound);
    expect(Reflect.ownKeys(failure ?? {})).toEqual(Object.keys(notFound));
    expect(errorStatus(failure)).toBe(404);
  });

  it("marks only the account input after a file error", async () => {
    // audit: COCRDSLC-027
    const seen = stubFetch(() => json(500, fileError));
    const client = renderScreen();

    typeInto(ACCOUNT, ACCT_ID);
    typeInto(CARD, TYPED_CARD);
    // The cursor starts in the card input, so the response is what moves it.
    act(() => {
      field(CARD).focus();
    });
    fireEvent.click(enterButton());
    await requestsSettled(seen, 1);

    expect(sentQueries(seen)).toEqual([{ acct_id: ACCT_ID, card_num: TYPED_CARD }]);
    expect(fileError.message).toHaveLength(75);
    expect(messageRow("error").textContent).toBe(fileError.message);
    expect(messageRow("info").textContent).toBe(PROMPT);
    expect(field(ACCOUNT).value).toBe(ACCT_ID);
    expect(field(ACCOUNT).className).toContain("text-destructive");
    expect(field(CARD).value).toBe(TYPED_CARD);
    expect(field(CARD).className).not.toContain("text-destructive");
    expect(document.activeElement).toBe(field(ACCOUNT));
    expect(cardDetails()).toEqual(["", "", "", ""]);
    const failure = lastFailure(client);
    expect(failure).toEqual(fileError);
    expect(Reflect.ownKeys(failure ?? {})).toEqual(Object.keys(fileError));
    expect(errorStatus(failure)).toBe(500);
  });

  /*
   * COCRDSLC-027 shows an input sent neither blank nor valid empty and in red; that case is
   * HALTED(X-COCRDSLC-H1), so no case here asserts it.
   */
  it("shows * in red in an input sent blank and never sends the marker", async () => {
    // audit: COCRDSLC-027
    const seen = stubFetch((query) => json(400, query.has("acct_id") ? cardNotProvided : noInput));
    renderScreen();

    fireEvent.click(enterButton());
    await requestsSettled(seen, 1);
    expect(messageRow("error").textContent).toBe(noInput.message);
    expect(messageRow("info").textContent).toBe(PROMPT);
    for (const label of [ACCOUNT, CARD]) {
      expect(field(label).value).toBe("*");
      expect(field(label).className).toContain("text-destructive");
    }
    expect(document.activeElement).toBe(field(ACCOUNT));

    // `*` alone reads as not supplied, so ENTER again sends neither input.
    fireEvent.click(enterButton());
    await requestsSettled(seen, 2);

    typeInto(ACCOUNT, ACCT_ID);
    fireEvent.click(enterButton());
    await requestsSettled(seen, 3);

    expect(sentQueries(seen)).toEqual([{}, {}, { acct_id: ACCT_ID }]);
    expect(messageRow("error").textContent).toBe(cardNotProvided.message);
    expect(field(ACCOUNT).value).toBe(ACCT_ID);
    expect(field(ACCOUNT).className).not.toContain("text-destructive");
    expect(field(CARD).value).toBe("*");
    expect(field(CARD).className).toContain("text-destructive");
    expect(document.activeElement).toBe(field(CARD));
  });

  it.each([
    {
      failure: "an empty 502",
      answer: (): Response => new Response(null, { status: 502, statusText: "Bad Gateway" }),
      message: "HTTP 502 Bad Gateway",
      status: 502,
    },
    {
      failure: "a request that got no response",
      answer: (): Response => {
        throw new TypeError("Failed to fetch");
      },
      message: "Failed to fetch",
      status: undefined,
    },
  ])(
    "shows $failure without a ScreenError body and leaves the inputs as sent",
    async ({ answer, message, status }) => {
      const seen = stubFetch(answer);
      const client = renderScreen();

      typeInto(ACCOUNT, ACCT_ID);
      typeInto(CARD, TYPED_CARD);
      fireEvent.click(enterButton());
      await requestsSettled(seen, 1);

      expect(messageRow("error").textContent).toBe(message);
      expect(messageRow("info").textContent).toBe(PROMPT);
      expect(field(ACCOUNT).value).toBe(ACCT_ID);
      expect(field(CARD).value).toBe(TYPED_CARD);
      for (const label of [ACCOUNT, CARD]) {
        expect(field(label).className).not.toContain("text-destructive");
      }
      // `settle` records the status of a reply it turned into an `Error`; no reply, no status.
      expect(errorStatus(lastFailure(client))).toBe(status);
    },
  );

  it.each([
    {
      title: "an account",
      label: ACCOUNT,
      value: "0".repeat(12),
      message: "String must contain at most 11 character(s)",
    },
    {
      title: "a card number",
      label: CARD,
      value: "0".repeat(17),
      message: "String must contain at most 16 character(s)",
    },
  ])(
    "rejects $title longer than its map LENGTH without a request",
    async ({ label, value, message }) => {
      const seen = stubFetch(unexpected);
      renderScreen();

      typeInto(label, value);
      fireEvent.click(enterButton());

      await waitFor(() => {
        expect(messageRow("error").textContent).toBe(message);
      });
      expect(seen).toEqual([]);
    },
  );

  it("sends one request at a time", async () => {
    let release = (): void => {};
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const seen = stubFetch(async () => {
      await held;
      return json(200, typedView);
    });
    renderScreen();

    typeInto(ACCOUNT, ACCT_ID);
    typeInto(CARD, TYPED_CARD);
    fireEvent.click(enterButton());
    await waitFor(() => {
      expect(form().getAttribute("aria-busy")).toBe("true");
    });
    fireEvent.click(enterButton());
    release();
    await requestsSettled(seen, 1);

    expect(field(CARD).value).toBe("************5678");
    expect(seen).toHaveLength(1);
  });

  it.each([
    {
      title: "the entry read from the card list",
      state: FROM_LIST,
      reply: view,
      send: undefined,
      editable: false,
    },
    {
      title: "a click on ENTER",
      state: undefined,
      reply: typedView,
      send: () => fireEvent.click(enterButton()),
      editable: true,
    },
    {
      title: "the key F5, processed as ENTER",
      state: undefined,
      reply: typedView,
      send: () => fireEvent.keyDown(window, { key: "F5" }),
      editable: true,
    },
  ])("locks every key but F3 and the inputs during $title, then frees them", async ({
    state,
    reply,
    send,
    editable,
  }) => {
    const { seen, open } = heldReplies(reply);
    renderScreen(state);
    if (send !== undefined) {
      typeInto(ACCOUNT, ACCT_ID);
      typeInto(CARD, TYPED_CARD);
      send();
    }
    await waitFor(() => {
      expect(keyRow().getAttribute("aria-busy")).toBe("true");
    });

    expect(disabledKeys()).toEqual({ "ENTER=Search Cards": true, "F3=Exit": false });
    expect(form().getAttribute("aria-busy")).toBe("true");
    expect([field(ACCOUNT).readOnly, field(CARD).readOnly]).toEqual([true, true]);
    // The lock leaves the Tab order as it was: only the list's protection takes them out of it.
    const tabOrder = editable ? [0, 0] : [-1, -1];
    expect([field(ACCOUNT).tabIndex, field(CARD).tabIndex]).toEqual(tabOrder);
    // Neither the button nor the keys send a second request, and no key counts as invalid.
    fireEvent.click(enterButton());
    for (const key of ["Enter", "F5"]) {
      fireEvent.keyDown(window, { key });
    }
    fireEvent.keyDown(window, { key: "F7", shiftKey: true });
    fireEvent.submit(form());
    expect(messageRow("error").textContent).toBe("");

    open();
    await requestsSettled(seen, 1);
    expect(keyRow().getAttribute("aria-busy")).toBe("false");
    expect(disabledKeys()).toEqual({ "ENTER=Search Cards": false, "F3=Exit": false });
    // Entered from the list, both inputs stay protected for the visit (COCRDSLC-002).
    expect([field(ACCOUNT).readOnly, field(CARD).readOnly]).toEqual([!editable, !editable]);
    expect([field(ACCOUNT).tabIndex, field(CARD).tabIndex]).toEqual(tabOrder);

    // The next ENTER reads again.
    fireEvent.click(enterButton());
    await requestsSettled(seen, 2);
    expect(sentQueries(seen)).toHaveLength(2);
  });

  it.each([
    {
      title: "ENTER clicked twice",
      keys: () => {
        fireEvent.click(enterButton());
        fireEvent.click(enterButton());
      },
    },
    {
      title: "the keys Enter and F5",
      keys: () => {
        fireEvent.keyDown(window, { key: "Enter" });
        fireEvent.keyDown(window, { key: "F5" });
      },
    },
    {
      title: "ENTER clicked and the form submitted",
      keys: () => {
        fireEvent.click(enterButton());
        fireEvent.submit(form());
      },
    },
  ])("sends one request for $title before the screen shows the lock", async ({ keys }) => {
    const seen = stubFetch(() => json(200, typedView));
    renderScreen();
    typeInto(ACCOUNT, ACCT_ID);
    typeInto(CARD, TYPED_CARD);

    // One act: the screen renders only after both keys, so the second meets the enabled keys.
    act(() => {
      keys();
    });

    await requestsSettled(seen, 1);
    expect(sentQueries(seen)).toEqual([{ acct_id: ACCT_ID, card_num: TYPED_CARD }]);
  });

  it("F3 returns to the card list while the entry read is out", async () => {
    // audit: COCRDSLC-004
    const { seen, open } = heldReplies(view);
    renderScreen(FROM_LIST);
    await waitFor(() => {
      expect(keyRow().getAttribute("aria-busy")).toBe("true");
    });

    fireEvent.click(screen.getByRole("button", { name: "F3=Exit" }));

    expect(await screen.findByText(LIST_STUB)).toBeTruthy();
    open();
    expect(seen).toHaveLength(1);
  });

  it("serves every key but F3 as ENTER, and Shift+F3 as F3", async () => {
    // audit: COCRDSLC-003
    // audit: COCRDSLC-004
    // audit: COCRDSL-038
    const seen = stubFetch(() => json(200, typedView));
    renderScreen();

    typeInto(ACCOUNT, ACCT_ID);
    typeInto(CARD, TYPED_CARD);
    fireEvent.keyDown(window, { key: "Enter" });
    await requestsSettled(seen, 1);
    fireEvent.keyDown(window, { key: "F5" });
    await requestsSettled(seen, 2);
    // Shift+F7 is PF19, which the program folds onto PF7.
    fireEvent.keyDown(window, { key: "F7", shiftKey: true });
    await requestsSettled(seen, 3);

    expect(sentQueries(seen)).toEqual([
      { acct_id: ACCT_ID, card_num: TYPED_CARD },
      { acct_id: ACCT_ID, card_ref: "2" },
      { acct_id: ACCT_ID, card_ref: "2" },
    ]);

    // Shift+F3 is PF15, which exits as F3 does.
    fireEvent.keyDown(window, { key: "F3", shiftKey: true });
    expect(await screen.findByText(MENU_STUB)).toBeTruthy();
    expect(seen).toHaveLength(3);
  });

  it.each([
    {
      title: "to the card list that selected the card",
      state: FROM_LIST,
      press: "click",
      stub: LIST_STUB,
      reads: 1,
    },
    {
      title: "to the card list on the F3 key",
      state: FROM_LIST,
      press: "key",
      stub: LIST_STUB,
      reads: 1,
    },
    {
      title: "to the main menu that opened it",
      state: { from: PATHS.COMEN01 },
      press: "click",
      stub: MENU_STUB,
      reads: 0,
    },
    {
      title: "to the main menu without a caller",
      state: undefined,
      press: "click",
      stub: MENU_STUB,
      reads: 0,
    },
    {
      title: "to the main menu from a path no screen has",
      state: { from: "/not-a-screen" },
      press: "click",
      stub: MENU_STUB,
      reads: 0,
    },
  ])("F3 returns $title without a request", async ({ state, press, stub, reads }) => {
    // audit: COCRDSLC-004
    // audit: COCRDSL-039
    const seen = stubFetch(() => json(200, view));
    renderScreen(state);
    await requestsSettled(seen, reads);

    const exit = screen.getByRole("button", { name: "F3=Exit" });
    expect(exit.getAttribute("type")).toBe("button");
    expect(exit.dataset.variant).toBe("outline");
    expect(exit.dataset.size).toBe("sm");
    if (press === "key") {
      fireEvent.keyDown(window, { key: "F3" });
    } else {
      fireEvent.click(exit);
    }

    expect(await screen.findByText(stub)).toBeTruthy();
    expect(seen).toHaveLength(reads);
    // The identity the next screen works with is unchanged.
    expect(getSession()).toEqual({ token: TOKEN, userId: "USER0001", userType: "U" });
  });

  // The replacement's accessibility wiring, which no audit row states: both inputs name in
  // `aria-describedby` only elements the screen renders. Neither has a hint, so each is described
  // by nothing while unmarked. An input COCRDSLC marks after a `ScreenError` (COCRDSLC-027) is
  // invalid and described by its hidden field message, which repeats the error row's text; a
  // found card leaves both unmarked.
  it.each<{
    state: string;
    answer: (query: URLSearchParams) => Response;
    reach: () => Promise<void>;
    marks: { readonly acctId?: string; readonly cardNum?: string };
  }>([
    { state: "on entry", answer: unexpected, reach: async () => undefined, marks: {} },
    {
      state: "after 'No input received' for both inputs sent blank",
      answer: () => json(400, noInput),
      reach: () => enterUntil(noInput.message),
      marks: { acctId: noInput.message, cardNum: noInput.message },
    },
    {
      state: "after 'Card number not provided'",
      answer: () => json(400, cardNotProvided),
      reach: async () => {
        typeInto(ACCOUNT, ACCT_ID);
        await enterUntil(cardNotProvided.message);
      },
      marks: { cardNum: cardNotProvided.message },
    },
    {
      state: "after the 404 of a search that finds no card",
      answer: () => json(404, notFound),
      reach: async () => {
        typeInto(ACCOUNT, ACCT_ID);
        typeInto(CARD, TYPED_CARD);
        await enterUntil(notFound.message);
      },
      marks: { acctId: notFound.message, cardNum: notFound.message },
    },
    {
      state: "after the 500 of a file error",
      answer: () => json(500, fileError),
      reach: async () => {
        typeInto(ACCOUNT, ACCT_ID);
        typeInto(CARD, TYPED_CARD);
        await enterUntil(fileError.message);
      },
      marks: { acctId: fileError.message },
    },
    {
      state: "after a found card that follows 'No input received'",
      answer: (query) => (query.has("acct_id") ? json(200, typedView) : json(400, noInput)),
      reach: async () => {
        await enterUntil(noInput.message);
        typeInto(ACCOUNT, ACCT_ID);
        typeInto(CARD, TYPED_CARD);
        await enterUntil("");
        await waitFor(() => {
          expect(field(NAME).value).toBe(typedView.embossed_name);
        });
      },
      marks: {},
    },
  ])("describes each input by rendered elements only, $state", async ({ answer, reach, marks }) => {
    stubFetch(answer);
    renderScreen();
    await reach();

    await waitFor(() => {
      expect({ acctId: readOut(field(ACCOUNT)), cardNum: readOut(field(CARD)) }).toEqual({
        acctId: readOutOf(marks.acctId),
        cardNum: readOutOf(marks.cardNum),
      });
    });
    expect(danglingDescriptions()).toEqual([]);
    // The error row stays the one visible copy of each text that marks an input.
    const texts = [marks.acctId, marks.cardNum].filter((text) => text !== undefined);
    for (const text of new Set(texts)) {
      const visible = visibleCopies(text);
      expect(visible).toHaveLength(1);
      expect(messageRow("error").contains(visible[0] ?? null)).toBe(true);
    }
  });

});

/** The 401 of the `SignedOnUser` extractor for a missing, invalid or expired token. */
const signonRequired: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};

/**
 * Renders the application's route table at `/cards/view`, so the sign-on guard that only the
 * table mounts stands in front of the screen. Each screen is imported by its route's loader, so a
 * wait for one to render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COCRDSL] });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

// The 401 clears the stored session (`api/client.ts`), and the sign-on guard in front of the
// route replaces the still-mounted screen with sign-on at once: the test presses no key and
// follows no link after ENTER.
describe("COCRDSL in the route table: a 401 ends the session", () => {
  beforeEach(() => {
    setSession(TOKEN, "USER0001", "U");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
  });

  it("ENTER answered 401 replaces the screen with sign-on and clears the session, with no retry", async () => {
    const seen = stubFetch(() => json(401, signonRequired));
    const router = renderRouteTable();
    expect(
      await screen.findByRole(
        "heading",
        { level: 1, name: "View Credit Card Detail" },
        { timeout: 5_000 },
      ),
    ).toBeDefined();

    typeInto(ACCOUNT, ACCT_ID);
    typeInto(CARD, TYPED_CARD);
    fireEvent.click(enterButton());

    expect(
      await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
    ).toBeDefined();
    expect(router.state.location.pathname).toBe(PATHS.COSGN00);
    expect(
      screen.queryByRole("heading", { level: 1, name: "View Credit Card Detail" }),
    ).toBeNull();
    expect(getSession()).toBeNull();
    expect(sentQueries(seen)).toEqual([{ acct_id: ACCT_ID, card_num: TYPED_CARD }]);
    expect(seen.map((request) => request.authorization)).toEqual([`Bearer ${TOKEN}`]);
    router.dispose();
  });
});
