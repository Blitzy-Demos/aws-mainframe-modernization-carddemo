import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ComponentType } from "react";
import {
  createMemoryRouter,
  MemoryRouter,
  Route,
  RouterProvider,
  Routes,
  useLocation,
  type RouteObject,
} from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { components } from "@/api/schema";
import { getSession, setSession } from "@/auth/session";
import { PATHS, routes } from "@/routes";
import { COTRTLI } from "./COTRTLI";

/**
 * Render tests of map `COTRTLI` (`CTRTLIA`), the transaction type list of transaction CTLI,
 * program COTRTLIC, endpoints 24 and 25 (`modern/audit/screens.md` COTRTLI rows,
 * `modern/audit/online/COTRTLIC.md`).
 *
 * The global `fetch` is a stub that answers endpoints 24 and 25 in process with the replies each
 * test queues, in order, and 500 to any other request, so nothing leaves the test. The listed
 * types are the seven the Db2 data version seeds (`app/app-transaction-type-db2/ctl/DB2LTTYP.ctl`
 * lines 17 to 23) and the messages are COTRTLIC's. No fixture holds a card number, an SSN or a
 * password.
 */

const LIST_PATH = "/api/v1/transaction-types";
const ACTIONS_PATH = "/api/v1/transaction-types/list-actions";

/**
 * The classes of the shared map-field box, FieldGrid's `FIELD_INPUT`: `min-w-max` and
 * `field-sizing-content` keep a box never narrower than its value, `px-1` is the one inset every
 * map input shares and `text-sm` the inputs' type size. Restated here because the render-test
 * contract imports no module beside the screen, the session, the routes and the generated types,
 * so a box is checked against this list rather than against the constant that renders it.
 */
const MAP_FIELD_BOX = ["min-w-max", "px-1", "text-sm", "field-sizing-content"] as const;

const RECORD_ACTIONS = "Type U to update, D to delete any record";
const DELETE_PROMPT = "Delete HIGHLIGHTED row ? Press F10 to confirm";
const UPDATE_PROMPT = "Update HIGHLIGHTED row. Press F10 to save";
const ROW_UPDATED = "HIGHLIGHTED row was updated";
const DELETE_RESTRICTED = "Please delete associated child records first: SQLCODE:";
const NO_CHANGE_DETECTED = "No change detected with respect to database values.";
const DESCRIPTION_NOT_ALPHANUMERIC = "Transaction Desc can have numbers or alphabets only.";
const INVALID_ACTION_CODE = "Action code selected is invalid";
const TYPE_FILTER_NOT_NUMERIC = "TYPE CODE FILTER,IF SUPPLIED MUST BE A 2 DIGIT NUMBER";
const NO_RECORDS_FOR_FILTERS = "No Records found for these filter conditions";
const NO_MORE_PAGES_FOR_SEARCH = "No more pages for these search conditions";
const NO_PREVIOUS_PAGES = "No previous pages to display";
const DB2_UNREACHABLE = "Db2 access failure. SQLCODE:";
const FORWARD_OPEN_FAILED = "C-TR-TYPE-FORWARD Open SQLCODE:";
const FIELD_TOO_LONG = "Input does not fit the screen field...";
/** The zod schema's length message, the client-side check behind each field's `maxlength`. */
const TYPE_FILTER_TOO_LONG = "String must contain at most 2 character(s)";

/*
 * The info messages as the 45-character `INFOMSG` line centres them (COTRTLIC-058): preceded by
 * (45 minus the length) / 2 spaces, integer part.
 */
const CENTRED_RECORD_ACTIONS = `  ${RECORD_ACTIONS}`; // (45 - 40) / 2 = 2
const CENTRED_DELETE_PROMPT = DELETE_PROMPT; // (45 - 45) / 2 = 0
const CENTRED_UPDATE_PROMPT = `  ${UPDATE_PROMPT}`; // (45 - 41) / 2 = 2
const CENTRED_ROW_UPDATED = `${" ".repeat(9)}${ROW_UPDATED}`; // (45 - 27) / 2 = 9

/** The seven seeded types, code and description, in key order (DB2LTTYP.ctl:17-23). */
const SEEDED_TYPES: readonly (readonly [string, string])[] = [
  ["01", "PURCHASE"],
  ["02", "PAYMENT"],
  ["03", "CREDIT"],
  ["04", "AUTHORIZATION"],
  ["05", "REFUND"],
  ["06", "REVERAL"],
  ["07", "ADJUSTMENT"],
];

/** The seeded types whose description holds an `R`: the page of the description filter `R`. */
const TYPES_WITH_R = SEEDED_TYPES.filter(([, description]) => description.includes("R"));

/** The seeded types with the description of screen row `row` replaced by `description`. */
function renamed(row: number, description: string): (readonly [string, string])[] {
  return SEEDED_TYPES.map(([code, text], index) => [code, index + 1 === row ? description : text]);
}

/** The seven state rows of `types` from row 1, `null` where a position holds no type. */
function stateRows(
  types: readonly (readonly [string, string])[],
): (components["schemas"]["TranTypeListRow"] | null)[] {
  return Array.from({ length: 7 }, (_, index) => {
    const type = types[index];
    return type === undefined ? null : { type_code: type[0], description: type[1] };
  });
}

/**
 * The seven shown rows of `types` from row 1, `null` where a position holds no type. `marks`
 * names, by screen row, the selections and descriptions the response returns, the highlighted
 * row, the row whose description is enterable, and whether every selector is protected.
 */
function shownRows(
  types: readonly (readonly [string, string])[],
  marks: {
    readonly selections?: Readonly<Record<number, string>>;
    readonly descriptions?: Readonly<Record<number, string>>;
    readonly highlighted?: number;
    readonly editable?: number;
    readonly protectSelectors?: boolean;
  } = {},
): (components["schemas"]["TranTypeListShownRow"] | null)[] {
  return Array.from({ length: 7 }, (_, index) => {
    const type = types[index];
    const row = index + 1;
    return type === undefined
      ? null
      : {
          type_code: type[0],
          description: marks.descriptions?.[row] ?? type[1],
          selection: marks.selections?.[row] ?? "",
          selector_enterable: marks.protectSelectors !== true,
          description_enterable: marks.editable === row,
          highlighted: marks.highlighted === row,
        };
  });
}

/** The `page_key` the API issues for `state`: the lowercase hexadecimal of its JSON. */
function pageKeyOf(state: components["schemas"]["TranTypeListState"]): string {
  return Array.from(new TextEncoder().encode(JSON.stringify(state)), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}

/** The state of a first entry (COTRTLIC-002): no filter, page 1 of the seeded types. */
const LOADED_STATE: components["schemas"]["TranTypeListState"] = {
  type_filter: "",
  desc_filter: "",
  rows: stateRows(SEEDED_TYPES),
  screen_rows: stateRows(SEEDED_TYPES),
  selected_row: null,
  first_key: "01",
  last_key: "07",
  page_no: 1,
  next_page_exists: false,
  last_page_shown: false,
  pending_action: "none",
  selectors: "open",
  description_entry: null,
};

/** The screen load, endpoint 24 without paging (COTRTLIC-002): the seven seeded types. */
const LOADED: components["schemas"]["TranTypeListResponse"] = {
  type_filter: { status: "blank", text: "" },
  desc_filter: { status: "blank", text: "" },
  rows: shownRows(SEEDED_TYPES),
  page_no: 1,
  has_next_page: false,
  info_message: RECORD_ACTIONS,
  message: null,
  state: LOADED_STATE,
  page_key: pageKeyOf(LOADED_STATE),
};

/** ENTER with blank selections and unchanged filters re-reads the page (COTRTLIC-006). */
const REFRESHED: components["schemas"]["TranTypeListActionResult"] = {
  ...LOADED,
  row_errors: [],
  committed: "nothing",
};

/** The state after ENTER with a `D` on row 1: a pending delete of row 1 (COTRTLIC-039). */
const DELETE_PENDING_STATE: components["schemas"]["TranTypeListState"] = {
  ...LOADED_STATE,
  selected_row: 1,
  pending_action: "delete",
};

/** ENTER with a `D` on row 1 (COTRTLIC-039): the row highlighted and the delete prompt. */
const DELETE_PROMPTED: components["schemas"]["TranTypeListActionResult"] = {
  ...LOADED,
  rows: shownRows(SEEDED_TYPES, { selections: { 1: "D" }, highlighted: 1 }),
  row_errors: [],
  info_message: DELETE_PROMPT,
  committed: "nothing",
  state: DELETE_PENDING_STATE,
  page_key: pageKeyOf(DELETE_PENDING_STATE),
};

/**
 * The F10 that follows (COTRTLIC-050): every seeded type still has categories, so the delete is
 * refused with 400, the `D` stays at its row and the delete stays pending.
 */
const DELETE_REFUSED: components["schemas"]["TranTypeListActionResult"] = {
  ...DELETE_PROMPTED,
  info_message: RECORD_ACTIONS,
  message: DELETE_RESTRICTED,
};

/** The state after ENTER with a `U` on row 6: a pending update, its description enterable. */
const UPDATE_PENDING_STATE: components["schemas"]["TranTypeListState"] = {
  ...LOADED_STATE,
  selected_row: 6,
  pending_action: "update",
  description_entry: 6,
};

/**
 * ENTER with a `U` on row 6 (COTRTLIC-040): the update prompt, with the no-change message of a
 * first `U`, whose description was protected until now (COTRTLIC-024).
 */
const UPDATE_PROMPTED: components["schemas"]["TranTypeListActionResult"] = {
  ...LOADED,
  rows: shownRows(SEEDED_TYPES, { selections: { 6: "U" }, highlighted: 6, editable: 6 }),
  row_errors: [],
  info_message: UPDATE_PROMPT,
  message: NO_CHANGE_DETECTED,
  committed: "nothing",
  state: UPDATE_PENDING_STATE,
  page_key: pageKeyOf(UPDATE_PENDING_STATE),
};

/** The state after F10 with `REVERSAL!`: the rejected description is what the screen showed. */
const UPDATE_REJECTED_STATE: components["schemas"]["TranTypeListState"] = {
  ...UPDATE_PENDING_STATE,
  screen_rows: stateRows(renamed(6, "REVERSAL!")),
};

/** F10 with `REVERSAL!` on row 6 (COTRTLIC-026): 400, row 6's description in error. */
const UPDATE_REJECTED: components["schemas"]["TranTypeListActionResult"] = {
  ...UPDATE_PROMPTED,
  rows: shownRows(renamed(6, "REVERSAL!"), {
    selections: { 6: "U" },
    highlighted: 6,
    editable: 6,
  }),
  row_errors: [{ row: 6, field: "description", message: DESCRIPTION_NOT_ALPHANUMERIC }],
  message: DESCRIPTION_NOT_ALPHANUMERIC,
  state: UPDATE_REJECTED_STATE,
  page_key: pageKeyOf(UPDATE_REJECTED_STATE),
};

/** The state after the update of row 6 to `REVERSAL`: nothing pending. */
const UPDATED_STATE: components["schemas"]["TranTypeListState"] = {
  ...LOADED_STATE,
  rows: stateRows(renamed(6, "REVERSAL")),
  screen_rows: stateRows(renamed(6, "REVERSAL")),
  selected_row: 6,
};

/** F10 with `REVERSAL` on row 6 (COTRTLIC-045): updated, its selection returned blank. */
const UPDATED: components["schemas"]["TranTypeListActionResult"] = {
  ...LOADED,
  rows: shownRows(renamed(6, "REVERSAL")),
  row_errors: [],
  info_message: ROW_UPDATED,
  committed: "updated",
  state: UPDATED_STATE,
  page_key: pageKeyOf(UPDATED_STATE),
};

/** ENTER with `X` on row 2 (COTRTLIC-021): 400, row 2's selector in error. */
const INVALID_SELECTION: components["schemas"]["TranTypeListActionResult"] = {
  ...LOADED,
  rows: shownRows(SEEDED_TYPES, { selections: { 2: "X" } }),
  row_errors: [{ row: 2, field: "selection", message: INVALID_ACTION_CODE }],
  message: INVALID_ACTION_CODE,
  committed: "nothing",
};

/** The state after F8 on the only page: the last page shown (COTRTLIC-008). */
const LAST_PAGE_STATE: components["schemas"]["TranTypeListState"] = {
  ...LOADED_STATE,
  last_page_shown: true,
};

/** F8 on the only page (COTRTLIC-008): the page re-read with the last-page message. */
const LAST_PAGE_AGAIN: components["schemas"]["TranTypeListResponse"] = {
  ...LOADED,
  message: NO_MORE_PAGES_FOR_SEARCH,
  state: LAST_PAGE_STATE,
  page_key: pageKeyOf(LAST_PAGE_STATE),
};

/** F7 on page 1 (COTRTLIC-010): page 1 re-read, no info message. */
const FIRST_PAGE_AGAIN: components["schemas"]["TranTypeListResponse"] = {
  ...LOADED,
  message: NO_PREVIOUS_PAGES,
  info_message: null,
};

/** The state of page 1 under the description filter `R`. */
const FILTERED_STATE: components["schemas"]["TranTypeListState"] = {
  ...LOADED_STATE,
  desc_filter: "R",
  rows: stateRows(TYPES_WITH_R),
  screen_rows: stateRows(TYPES_WITH_R),
  last_key: "06",
};

/** F8 with the description filter changed to `R`: page 1 of it, five types and two empty rows. */
const FILTERED: components["schemas"]["TranTypeListResponse"] = {
  ...LOADED,
  desc_filter: { status: "valid", text: "R" },
  rows: shownRows(TYPES_WITH_R),
  state: FILTERED_STATE,
  page_key: pageKeyOf(FILTERED_STATE),
};

/** The state after a type filter in error: stored, paging reset, selectors protected. */
const TYPE_FILTER_REJECTED_STATE: components["schemas"]["TranTypeListState"] = {
  ...LOADED_STATE,
  type_filter: "A1",
  first_key: null,
  last_key: null,
  page_no: 0,
  selectors: "protected",
};

/** F8 with the type filter `A1` (COTRTLIC-028): 400, no read, every selector protected. */
const TYPE_FILTER_REJECTED: components["schemas"]["TranTypeListResponse"] = {
  ...LOADED,
  type_filter: { status: "in_error", text: "A1" },
  rows: shownRows(SEEDED_TYPES, { protectSelectors: true }),
  page_no: 0,
  info_message: null,
  message: TYPE_FILTER_NOT_NUMERIC,
  state: TYPE_FILTER_REJECTED_STATE,
  page_key: pageKeyOf(TYPE_FILTER_REJECTED_STATE),
};

/** The state after a description filter that matches no type: stored, paging reset. */
const NO_MATCH_STATE: components["schemas"]["TranTypeListState"] = {
  ...LOADED_STATE,
  desc_filter: "ZZ",
  first_key: null,
  last_key: null,
  page_no: 0,
  selectors: "protected",
};

/** F8 with the description filter `ZZ` (COTRTLIC-031): 400, no read, the filter in error. */
const NO_MATCH: components["schemas"]["TranTypeListResponse"] = {
  ...LOADED,
  desc_filter: { status: "in_error", text: "ZZ" },
  rows: shownRows(SEEDED_TYPES, { protectSelectors: true }),
  page_no: 0,
  info_message: null,
  message: NO_RECORDS_FOR_FILTERS,
  state: NO_MATCH_STATE,
  page_key: pageKeyOf(NO_MATCH_STATE),
};

/** The state of a first entry whose page cursor failed to open: no rows read. */
const OPEN_FAILED_STATE: components["schemas"]["TranTypeListState"] = {
  ...LOADED_STATE,
  rows: stateRows([]),
  screen_rows: stateRows([]),
  first_key: null,
  last_key: null,
};

/** The screen load when the forward cursor fails to open (COTRTLIC-052): 500 with the page. */
const OPEN_FAILED: components["schemas"]["TranTypeListResponse"] = {
  ...LOADED,
  rows: shownRows([]),
  info_message: null,
  message: FORWARD_OPEN_FAILED,
  state: OPEN_FAILED_STATE,
  page_key: pageKeyOf(OPEN_FAILED_STATE),
};

/** The database cannot be reached (COTRTLIC-053): 500, no page and no state. */
const DB2_DOWN: components["schemas"]["ScreenError"] = { message: DB2_UNREACHABLE, field: null };

/** The answer to a request that is neither endpoint 24 nor endpoint 25. */
const UNEXPECTED: components["schemas"]["ScreenError"] = { message: "unexpected request" };

/** The server's length rule on the request field `field`, by its wire name. */
function fieldTooLong(field: string): components["schemas"]["ScreenError"] {
  return { message: FIELD_TOO_LONG, field };
}

/** The seven request rows of `types` as the screen sends them unchanged: blank selections. */
function unchangedRows(
  types: readonly (readonly [string, string])[],
): components["schemas"]["TranTypeListRowInput"][] {
  return Array.from({ length: 7 }, (_, index) => ({
    selection: "",
    description: types[index]?.[1] ?? "",
  }));
}

/** `rows` with screen row `row` replaced by `input`. */
function withRow(
  rows: readonly components["schemas"]["TranTypeListRowInput"][],
  row: number,
  input: components["schemas"]["TranTypeListRowInput"],
): components["schemas"]["TranTypeListRowInput"][] {
  return rows.map((current, index) => (index + 1 === row ? input : current));
}

/** Replies with a JSON body, as the API sends every page, result and `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces the global `fetch` with a stub that records every request and answers endpoints 24
 * and 25 with `replies` in order (a status and body, or a pending response), and any other
 * request, or one past the last reply, with 500.
 */
function stubFetch(replies: readonly (readonly [number, unknown] | Promise<Response>)[]) {
  const seen: { method: string; url: URL; authorization: string | null; body: unknown }[] = [];
  let served = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init);
      const url = new URL(request.url);
      const body: unknown = request.method === "POST" ? await request.clone().json() : undefined;
      seen.push({
        method: request.method,
        url,
        authorization: request.headers.get("Authorization"),
        body,
      });
      const route = `${request.method} ${url.pathname}`;
      if (route !== `GET ${LIST_PATH}` && route !== `POST ${ACTIONS_PATH}`) {
        return json(500, UNEXPECTED);
      }
      const reply = replies[served];
      served += 1;
      if (reply instanceof Promise) {
        return reply;
      }
      return reply === undefined ? json(500, UNEXPECTED) : json(reply[0], reply[1]);
    }),
  );
  return seen;
}

/**
 * The `n`-th request (1-based), after checking that it went to `route` and that no request so far
 * carried `page_size`, which the screen never sends (Bounded Pagination).
 */
function sent(seen: ReturnType<typeof stubFetch>, n: number, route: string) {
  for (const request of seen) {
    expect(request.url.href).not.toContain("page_size");
    expect(JSON.stringify(request.body ?? null)).not.toContain("page_size");
  }
  const request = seen[n - 1];
  if (request === undefined) {
    throw new Error(`request ${n} was not sent`);
  }
  expect(`${request.method} ${request.url.pathname}`).toBe(route);
  return request;
}

/** The query of an endpoint 24 request. */
function queryOf(request: { readonly url: URL }): Record<string, string> {
  return Object.fromEntries(request.url.searchParams);
}

/** A navigation target: a region named after its map, showing the route state it received. */
function Target({ name }: { readonly name: string }) {
  const state: unknown = useLocation().state;
  return (
    <section aria-label={name}>
      <p>{JSON.stringify(state ?? null)}</p>
    </section>
  );
}

/** Renders `/transaction-types`, arriving with `arrival` as route state, with stub targets. */
function renderScreen(arrival: { readonly from: string } | null = null): void {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COTRTLI, state: arrival }]}>
        <Routes>
          <Route path={PATHS.COTRTLI} element={<COTRTLI />} />
          <Route path={PATHS.COTRTUP} element={<Target name="COTRTUP" />} />
          <Route path={PATHS.COADM01} element={<Target name="COADM01" />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The route state the target `name` received, once the screen has opened it. */
async function arrivedAt(name: string): Promise<unknown> {
  const target = await screen.findByRole("region", { name });
  const state: unknown = JSON.parse(target.textContent ?? "");
  return state;
}

/** The screen's form. */
function form(): HTMLFormElement {
  const found = document.querySelector("form");
  if (found === null) {
    throw new Error("the screen has no form");
  }
  return found;
}

/** Waits until `count` requests were sent and the last one is answered. */
async function answered(seen: ReturnType<typeof stubFetch>, count: number): Promise<void> {
  await waitFor(() => {
    expect(seen).toHaveLength(count);
    expect(form().getAttribute("aria-busy")).toBe("false");
  });
}

/**
 * Lets every pending promise and timer run, so a request a key would send has been sent. Run
 * inside `waitFor`, so any state update it lets through lands where React expects it.
 */
async function settle(): Promise<void> {
  await waitFor(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
  });
}

/** A reply held back until the test passes it to `release`. */
function heldReply(): {
  readonly reply: Promise<Response>;
  readonly release: (response: Response) => void;
} {
  let resolveReply: (response: Response) => void = () => undefined;
  const reply = new Promise<Response>((resolve) => {
    resolveReply = resolve;
  });
  return {
    reply,
    release: (response) => {
      resolveReply(response);
    },
  };
}

/** The row-24 key group, busy while a request is out. */
function functionKeys(): HTMLElement {
  return screen.getByRole("group", { name: "Function keys" });
}

/** The key-row button labelled `name`. */
function keyButton(name: string): HTMLButtonElement {
  const button = within(functionKeys()).getByRole("button", { name });
  if (!(button instanceof HTMLButtonElement)) {
    throw new Error(`the key ${name} is not a button`);
  }
  return button;
}

/**
 * Waits until the cursor is on the field `target` finds: the screen moves it in an effect after
 * it shows a response, so it can arrive after the response's text.
 */
async function cursorOn(target: () => HTMLElement): Promise<void> {
  await waitFor(() => {
    expect(document.activeElement).toBe(target());
  });
}

/** `TRTYPE`, found by its label `Type Filter:`. */
function typeFilter(): HTMLElement {
  return screen.getByLabelText("Type Filter:");
}

/** `TRDESC`, found by its label `Description Filter:`. */
function descFilter(): HTMLElement {
  return screen.getByLabelText("Description Filter:");
}

/** `PAGENO`, found by its label `Page`. */
function pageNo(): HTMLElement {
  return screen.getByLabelText("Page");
}

/** The seven list rows, top to bottom, empty ones included. */
function bodyRows(): HTMLTableRowElement[] {
  const table = screen.getByRole("table", { name: "Transaction types" });
  return Array.from(table.querySelectorAll<HTMLTableRowElement>("tbody > tr"));
}

/** List row `row`, 1 to 7. */
function bodyRow(row: number): HTMLTableRowElement {
  const found = bodyRows().at(row - 1);
  if (found === undefined) {
    throw new Error(`no list row ${row}`);
  }
  return found;
}

/**
 * Asserts whether list row `row` is the highlighted row: shaded (`data-state`), current for
 * assistive technology (`aria-current`) and emphasised (`font-semibold`), or none of the three.
 */
function expectHighlighted(row: number, highlighted: boolean): void {
  const line = bodyRow(row);
  expect(line.getAttribute("data-state")).toBe(highlighted ? "selected" : null);
  expect(line.getAttribute("aria-current")).toBe(highlighted ? "true" : null);
  expect(line.classList.contains("font-semibold")).toBe(highlighted);
}

/** A column of the list `Table`: the map column it starts at and its width in map columns. */
interface ListColumn {
  readonly start: number;
  readonly width: number;
}

/**
 * The columns of `table` from its `colgroup`, the first starting at map column `first`: each
 * `col` is a `w-k/n` fraction of the list's `span` map columns, and together they fill it.
 */
function listColumns(table: HTMLElement, first: number, span: number): ListColumn[] {
  const columns: ListColumn[] = [];
  let start = first;
  for (const col of table.querySelectorAll("colgroup > col")) {
    const [, width = "", of = ""] = /^w-(\d+)\/(\d+)$/.exec(col.className) ?? [];
    expect(of).toBe(String(span));
    columns.push({ start, width: Number.parseInt(width, 10) });
    start += Number.parseInt(width, 10);
  }
  expect(start).toBe(first + span);
  return columns;
}

/**
 * The map column `element` shows its content at: its column's start, moved by its `left-k/w` or
 * `-left-k/w` class, `k` of the column's `w` map columns.
 */
function shownColumn(element: Element, column: ListColumn | undefined): number {
  if (column === undefined) {
    throw new Error("a list cell stands outside the colgroup's columns");
  }
  const shift = /(?:^|\s)(-?)left-(\d+)\/(\d+)(?:\s|$)/.exec(element.className);
  if (shift === null) {
    return column.start;
  }
  const [, sign, moved, of] = shift;
  expect(element.classList.contains("relative")).toBe(true);
  expect(of).toBe(String(column.width));
  const columns = Number.parseInt(moved, 10);
  return sign === "-" ? column.start - columns : column.start + columns;
}

/** The map column each cell of `row` shows its text at: its own, or its one wrapper's. */
function shownColumns(row: Element | undefined, columns: readonly ListColumn[]): number[] {
  return Array.from(row?.children ?? [], (cell, index) => {
    return shownColumn(cell.firstElementChild ?? cell, columns.at(index));
  });
}

/** The input of list row `row`: its selector (`0`) or its description (`1`). */
function rowInput(row: number, which: 0 | 1): HTMLInputElement {
  const input = bodyRow(row).querySelectorAll("input").item(which);
  if (input === null) {
    throw new Error(`list row ${row} has no input ${which}`);
  }
  return input;
}

/** `TRTSELn` of list row `row`. */
function selectorAt(row: number): HTMLInputElement {
  return rowInput(row, 0);
}

/** `TRTYPDn` of list row `row`. */
function descriptionAt(row: number): HTMLInputElement {
  return rowInput(row, 1);
}

/** The type code cell (`TRTTYPn`) of each list row, top to bottom. */
function codesShown(): string[] {
  return bodyRows().map((row) => row.querySelectorAll("td").item(1)?.textContent ?? "");
}

/** The row-error text the screen shows inside list row `row`, if any. */
function rowErrorText(row: number): string | null | undefined {
  return bodyRow(row).querySelector('[data-slot="form-message"]')?.textContent;
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

/** The map's sixteen inputs by name: the two filters, then each row's selector and description. */
function namedInputs(): Record<string, HTMLElement> {
  const inputs: Record<string, HTMLElement> = {
    typeFilter: typeFilter(),
    descFilter: descFilter(),
  };
  for (let row = 1; row <= 7; row += 1) {
    inputs[`selector ${row}`] = selectorAt(row);
    inputs[`description ${row}`] = descriptionAt(row);
  }
  return inputs;
}

/**
 * How the sixteen inputs read when only the input `marked` (a name of {@link namedInputs}) holds
 * the error `message`. No input of the map has a hint, so an unmarked one is described by
 * nothing, and the marked one only by its field message, hidden.
 */
function readOutsWith(marked?: string, message = ""): Record<string, ReadOut> {
  return Object.fromEntries(
    Object.keys(namedInputs()).map((name) => [
      name,
      name === marked
        ? { invalid: "true", described: [{ text: message, hidden: true }] }
        : { invalid: "false", described: null },
    ]),
  );
}

/** The one message row of `kind`, `ERRMSG` (`error`) or `INFOMSG` (`info`). */
function messageRow(kind: "error" | "info"): HTMLElement {
  const rows = screen
    .getAllByRole("alert")
    .filter((row) => row.getAttribute("data-message-kind") === kind);
  const [row, ...others] = rows;
  if (row === undefined || others.length > 0) {
    throw new Error(`expected one ${kind} row, found ${rows.length}`);
  }
  return row;
}

/** Replaces what `field` holds with `value`, as typing over it does. */
function type(field: HTMLElement, value: string): void {
  fireEvent.change(field, { target: { value } });
}

/** Clicks the key-row button labelled `name`. */
function press(name: string): void {
  fireEvent.click(screen.getByRole("button", { name }));
}

/**
 * Moves the cursor to `element`, so that the screen's own move of it afterwards is observable.
 * The blur this causes re-renders the form's field state, which `waitFor` lets React render.
 */
async function moveCursor(element: HTMLElement): Promise<void> {
  await waitFor(() => {
    element.focus();
    expect(document.activeElement).toBe(element);
  });
}

/** Presses the keyboard key `key` where the cursor is, as the 3270 sends its AID. */
function pressKey(key: string, shiftKey = false): void {
  fireEvent.keyDown(document.activeElement ?? window, { key, shiftKey });
}

/**
 * Asserts the header value fields of rows 1 and 2 against their rows of `modern/audit/screens.md`
 * (COTRTLI-002, -003, -005, -007, -008 and -010). Each entry of `fields` is the BMS name, the `POS`
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
    expect(element.getAttribute("data-slot")).toBe("input");
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

beforeEach(() => {
  setSession("test-token", "ADMIN001", "A");
});

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COTRTLI", () => {
  it("renders the map's fields and key row, then shows page 1 of the first entry", async () => {
    // audit: COTRTLI-011
    // audit: COTRTLI-012
    // audit: COTRTLI-013
    // audit: COTRTLI-014
    // audit: COTRTLI-016
    // audit: COTRTLI-017
    // audit: COTRTLI-019
    // audit: COTRTLI-020
    // audit: COTRTLI-021
    // audit: COTRTLI-022
    // audit: COTRTLI-023
    // audit: COTRTLI-024
    // audit: COTRTLI-025
    // audit: COTRTLI-027
    // audit: COTRTLI-028
    // audit: COTRTLI-029
    // audit: COTRTLI-031
    // audit: COTRTLI-033
    // audit: COTRTLI-034
    // audit: COTRTLI-035
    // audit: COTRTLI-037
    // audit: COTRTLI-039
    // audit: COTRTLI-040
    // audit: COTRTLI-041
    // audit: COTRTLI-043
    // audit: COTRTLI-045
    // audit: COTRTLI-046
    // audit: COTRTLI-047
    // audit: COTRTLI-049
    // audit: COTRTLI-051
    // audit: COTRTLI-052
    // audit: COTRTLI-053
    // audit: COTRTLI-055
    // audit: COTRTLI-057
    // audit: COTRTLI-058
    // audit: COTRTLI-059
    // audit: COTRTLI-061
    // audit: COTRTLI-063
    // audit: COTRTLI-064
    // audit: COTRTLI-065
    // audit: COTRTLI-067
    // audit: COTRTLI-068
    // audit: COTRTLI-069
    // audit: COTRTLI-070
    // audit: COTRTLI-071
    // audit: COTRTLI-072
    // audit: COTRTLI-073
    // audit: COTRTLI-075
    // audit: COTRTLI-076
    // audit: COTRTLI-077
    // audit: COTRTLI-078
    // audit: COTRTLI-079
    // audit: COTRTLI-080
    // audit: COTRTLI-081
    // audit: COTRTLI-082
    // audit: COTRTLI-083
    // audit: COTRTLI-084
    // audit: COTRTLI-085
    // audit: COTRTLI-086
    // audit: COTRTLI-087
    // audit: COTRTLIC-057
    // audit: COTRTLIC-058
    // audit: COTRTLIC-059
    const seen = stubFetch([[200, LOADED]]);
    renderScreen();

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "Maintain Transaction Type",
    );
    // A header value is the one read-only `Input` of its `data-field` in the `CardHeader`.
    const header = (field: string) => {
      const inputs = document.querySelectorAll(
        `[data-slot="card-header"] input[data-field="${field}"]`,
      );
      expect(inputs).toHaveLength(1);
      const [input] = inputs;
      if (!(input instanceof HTMLInputElement)) {
        throw new Error(`The header has no input ${field}`);
      }
      return input.value;
    };
    expect(header("TRNNAME")).toBe("CTLI");
    expect(header("PGMNAME")).toBe("COTRTLIC");
    expect(header("TITLE01")).toBe("      AWS Mainframe Modernization       ");
    expect(header("TITLE02")).toBe("              CardDemo                  ");
    expect(header("CURDATE")).toMatch(/^\d{2}\/\d{2}\/\d{2}$/);
    expect(header("CURTIME")).toMatch(/^\d{2}:\d{2}:\d{2}$/);

    // The first display: both filters empty and enterable, the cursor on the type filter (`IC`).
    // They take typing once the first entry's request is answered.
    expect(typeFilter().getAttribute("maxlength")).toBe("2");
    expect(descFilter().getAttribute("maxlength")).toBe("50");
    for (const filter of [typeFilter(), descFilter()]) {
      expect(filter).toHaveProperty("value", "");
    }
    expect(document.activeElement).toBe(typeFilter());

    await answered(seen, 1);
    for (const filter of [typeFilter(), descFilter()]) {
      expect(filter).toHaveProperty("readOnly", false);
    }
    const load = sent(seen, 1, `GET ${LIST_PATH}`);
    // The first entry sends no member of the query at all (COTRTLIC-002).
    expect(load.url.search).toBe("");
    expect(load.authorization).toBe("Bearer test-token");

    // `pageNo()` finds the field by the label `Page`: the map's `Page ` and `Select    ` end in
    // padding, which HTML does not show, so the visible text is the map's.
    expect(pageNo()).toHaveProperty("readOnly", true);
    expect(pageNo().getAttribute("size")).toBe("3");
    expect(pageNo()).toHaveProperty("value", "1");
    // Skipped by Tab, as the 3270 cursor skips the `ASKIP` field.
    expect(pageNo().tabIndex).toBe(-1);
    // `Page ` (4,70) and `PAGENO` (4,76) share the title's map line 4: the heading's line of the
    // CardHeader, on its 80 columns, outside the heading, whose name stays the title, and outside
    // the body form.
    const heading = screen.getByRole("heading", { level: 1, name: "Maintain Transaction Type" });
    const titleLine = heading.parentElement;
    expect(titleLine?.parentElement?.dataset.slot).toBe("card-header");
    expect(titleLine?.className).toBe("grid grid-cols-80 items-center gap-x-2");
    expect(heading.classList.contains("col-span-full")).toBe(true);
    expect(heading.classList.contains("row-start-1")).toBe(true);
    const pageLabel = screen.getByText("Page ".trimEnd());
    expect(pageLabel.getAttribute("for")).toBe(pageNo().id);
    expect(pageLabel.parentElement?.className).toBe("row-start-1 col-start-70 col-span-5");
    expect(pageNo().parentElement?.className).toBe("row-start-1 col-start-76 col-span-3");
    for (const element of [pageLabel, pageNo()]) {
      expect(element.parentElement?.parentElement).toBe(titleLine);
      expect(heading.contains(element)).toBe(false);
      expect(form().contains(element)).toBe(false);
    }

    // Every input takes the shared map-field box, `MAP_FIELD_BOX` with `size` = its LENGTH, so a
    // full-length value, the 50-character row description included, shows on the narrowest frame.
    for (const [box, length] of [
      [pageNo(), "3"],
      [typeFilter(), "2"],
      [descFilter(), "50"],
      [selectorAt(1), "1"],
      [descriptionAt(1), "50"],
    ] as const) {
      expect(box.getAttribute("size")).toBe(length);
      expect(MAP_FIELD_BOX.filter((name) => !box.classList.contains(name))).toEqual([]);
    }

    const table = screen.getByRole("table", { name: "Transaction types" });
    expect(within(table).getAllByRole("columnheader").map((head) => head.textContent)).toEqual([
      "Select    ".trimEnd(),
      "Type",
      "Description",
    ]);
    // The dash line is decoration hidden from assistive technology: plain cells, no headers.
    expect(
      Array.from(table.querySelectorAll('thead > tr[aria-hidden="true"] > td'), (cell) => {
        return cell.textContent;
      }),
    ).toEqual(["------", "-----", "-".repeat(50)]);
    expect(table.querySelectorAll("th")).toHaveLength(3);
    for (const head of table.querySelectorAll("th")) {
      expect(head.getAttribute("scope")).toBe("col");
    }

    // Seven list rows of three cells each: the map's eighth row (line 19) is not rendered.
    expect(bodyRows()).toHaveLength(7);
    for (const row of bodyRows()) {
      expect(row.querySelectorAll("td")).toHaveLength(3);
    }
    expect(codesShown()).toEqual(SEEDED_TYPES.map(([code]) => code));
    for (const [index, [code, description]] of SEEDED_TYPES.entries()) {
      const row = index + 1;
      expect(selectorAt(row).getAttribute("aria-label")).toBe(`Select type ${code}`);
      expect(selectorAt(row).getAttribute("maxlength")).toBe("1");
      expect(selectorAt(row)).toHaveProperty("value", "");
      expect(selectorAt(row)).toHaveProperty("readOnly", false);
      expect(descriptionAt(row).getAttribute("aria-label")).toBe(`Description type ${code}`);
      expect(descriptionAt(row).getAttribute("maxlength")).toBe("50");
      expect(descriptionAt(row)).toHaveProperty("value", description);
      expect(descriptionAt(row)).toHaveProperty("readOnly", true);
      expectHighlighted(row, false);
    }

    // The form's inputs are exactly the map's input and output fields below the title's line, in
    // map order: no element stands for a zero-length stopper, and none for the eighth row.
    const names = Array.from(form().querySelectorAll("input"), (input) => {
      return input.labels?.item(0)?.textContent ?? input.getAttribute("aria-label");
    });
    expect(names).toEqual([
      "Type Filter:",
      "Description Filter:",
      ...SEEDED_TYPES.flatMap(([code]) => [`Select type ${code}`, `Description type ${code}`]),
    ]);

    expect(messageRow("info").textContent).toBe(CENTRED_RECORD_ACTIONS);
    expect(messageRow("error").textContent).toBe("");
    expect(messageRow("error").getAttribute("aria-live")).toBe("polite");

    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(keys.map((key) => key.textContent)).toEqual([
      "ENTER",
      "F2=Add",
      "F3=Exit",
      "F7=Page Up",
      "F8=Page Dn",
      "F10=Save",
    ]);
    expect(
      keys.map((key) => [
        key.getAttribute("type"),
        key.getAttribute("data-variant"),
        key.getAttribute("data-size"),
      ]),
    ).toEqual([
      ["submit", "default", "sm"],
      ["button", "outline", "sm"],
      ["button", "outline", "sm"],
      ["button", "outline", "sm"],
      ["button", "outline", "sm"],
      ["button", "outline", "sm"],
    ]);
    expect(keys.every((key) => !key.hidden)).toBe(true);
  });

  it.each([
    { shown: "seven types", page: LOADED, types: 7 },
    { shown: "five types and two empty rows", page: FILTERED, types: 5 },
  ])("keeps the list on the map's columns and lines with $shown", async ({ page, types }) => {
    // audit: COTRTLI-020
    // audit: COTRTLI-021
    // audit: COTRTLI-022
    // audit: COTRTLI-023
    // audit: COTRTLI-024
    // audit: COTRTLI-025
    const seen = stubFetch([[200, page]]);
    renderScreen();
    await answered(seen, 1);
    expect(codesShown().filter((code) => code !== "")).toHaveLength(types);

    // Fixed columns from the line-12 fields' `POS`: the selector column at 4, the type at 17
    // and the description at 25, to the list's end at column 74.
    const table = screen.getByRole("table", { name: "Transaction types" });
    expect(table.classList.contains("table-fixed")).toBe(true);
    const columns = listColumns(table, 4, 71);
    expect(columns.map((column) => column.start)).toEqual([4, 17, 25]);
    // Captions at (10,4), (10,16), (10,42); dashes at (11,4), (11,15), (11,25).
    const [captions, dashes] = Array.from(table.querySelectorAll("thead > tr"));
    expect(shownColumns(captions, columns)).toEqual([4, 16, 42]);
    expect(shownColumns(dashes, columns)).toEqual([4, 15, 25]);
    // The description column is the Description Filter's span, `POS=(8,25) LENGTH=50`; its
    // input fills the column but the trailing column gap, as the filter fills its cell.
    expect(descFilter().parentElement?.className).toBe("row-start-8 col-start-25 col-span-50");
    expect(columns.at(2)).toEqual({ start: 25, width: 50 });
    for (const row of Array.from({ length: 7 }, (_, index) => index + 1)) {
      // A one-character selector box at column 6, `POS=(12..18,6)`.
      expect(shownColumn(selectorAt(row), columns.at(0))).toBe(6);
      expect(selectorAt(row).classList.contains("w-9")).toBe(true);
      expect(descriptionAt(row).closest("td")?.classList.contains("pr-2")).toBe(true);
      expect(shownColumn(descriptionAt(row), columns.at(2))).toBe(25);
    }
    // Nine rows of one line pitch each, whatever they hold, and no cell padding on the line.
    expect(table.querySelectorAll("tr")).toHaveLength(9);
    for (const row of table.querySelectorAll("tr")) {
      expect(row.classList.contains("h-10")).toBe(true);
    }
    for (const cell of table.querySelectorAll("th, td")) {
      expect(cell.classList.contains("px-0")).toBe(true);
      expect(cell.classList.contains("pt-0")).toBe(true);
    }
  });

  it("shows the six header values as read-only Inputs in the CardHeader title rows, each at its POS and LENGTH", async () => {
    // audit: COTRTLI-002
    // audit: COTRTLI-003
    // audit: COTRTLI-005
    // audit: COTRTLI-007
    // audit: COTRTLI-008
    // audit: COTRTLI-010
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
      const seen = stubFetch([[200, LOADED]]);
      renderScreen();
      // The first entry's page read, answered in process by the stub.
      await answered(seen, 1);
      sent(seen, 1, `GET ${LIST_PATH}`);

      expectHeaderValues(document, [
        { name: "TRNNAME", row: 1, col: 7, length: 4, value: "CTLI", title: false },
        {
          name: "TITLE01",
          row: 1,
          col: 21,
          length: 40,
          value: "      AWS Mainframe Modernization       ",
          title: true,
        },
        { name: "CURDATE", row: 1, col: 71, length: 8, value: "01/02/24", title: false },
        { name: "PGMNAME", row: 2, col: 7, length: 8, value: "COTRTLIC", title: false },
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
    } finally {
      vi.useRealTimers();
    }
  });

  it("ENTER sends the filters, seven rows and loaded state, then shows the prompt", async () => {
    // audit: COTRTLI-013
    // audit: COTRTLI-028
    // audit: COTRTLI-093
    // audit: COTRTLIC-056
    // audit: COTRTLIC-057
    // audit: COTRTLIC-058
    const seen = stubFetch([
      [200, LOADED],
      [200, DELETE_PROMPTED],
    ]);
    renderScreen();
    await answered(seen, 1);

    type(selectorAt(1), "D");
    press("ENTER");
    await answered(seen, 2);

    const enter = sent(seen, 2, `POST ${ACTIONS_PATH}`);
    expect(enter.body).toEqual({
      aid: "ENTER",
      type_filter: "",
      desc_filter: "",
      rows: withRow(unchangedRows(SEEDED_TYPES), 1, { selection: "D", description: "PURCHASE" }),
      state: LOADED_STATE,
    });

    expect(messageRow("info").textContent).toBe(CENTRED_DELETE_PROMPT);
    expect(messageRow("error").textContent).toBe("");
    expect(pageNo()).toHaveProperty("value", "1");
    expect(codesShown()).toEqual(SEEDED_TYPES.map(([code]) => code));
    // The single valid `D`: its row highlighted, the cursor on its selector, every description
    // read-only.
    expect(selectorAt(1)).toHaveProperty("value", "D");
    expectHighlighted(1, true);
    await cursorOn(() => selectorAt(1));
    for (const row of [2, 3, 4, 5, 6, 7]) {
      expectHighlighted(row, false);
    }
    for (const row of [1, 2, 3, 4, 5, 6, 7]) {
      expect(descriptionAt(row)).toHaveProperty("readOnly", true);
    }
    // The request carried a selection, so both filters are shown read-only.
    for (const filter of [typeFilter(), descFilter()]) {
      expect(filter).toHaveProperty("readOnly", true);
      expect(filter.getAttribute("tabindex")).toBe("-1");
    }
  });

  it("F10 echoes the prompt's new state unchanged and shows the refused delete", async () => {
    // audit: COTRTLI-076
    // audit: COTRTLI-092
    const seen = stubFetch([
      [200, LOADED],
      [200, DELETE_PROMPTED],
      [400, DELETE_REFUSED],
    ]);
    renderScreen();
    await answered(seen, 1);
    type(selectorAt(1), "D");
    press("ENTER");
    await answered(seen, 2);

    press("F10=Save");
    await answered(seen, 3);

    // The state is the prompt's, which differs from the loaded one in its pending delete.
    expect(sent(seen, 3, `POST ${ACTIONS_PATH}`).body).toEqual({
      aid: "PF10",
      type_filter: "",
      desc_filter: "",
      rows: withRow(unchangedRows(SEEDED_TYPES), 1, { selection: "D", description: "PURCHASE" }),
      state: DELETE_PENDING_STATE,
    });
    expect(messageRow("error").textContent).toBe(DELETE_RESTRICTED);
    expect(messageRow("info").textContent).toBe(CENTRED_RECORD_ACTIONS);
    expect(selectorAt(1)).toHaveProperty("value", "D");
    expectHighlighted(1, true);
  });

  it("a U opens its description; F10 flags a bad one at its row, then saves", async () => {
    // audit: COTRTLI-092
    // audit: COTRTLIC-056
    // audit: COTRTLIC-058
    const seen = stubFetch([
      [200, LOADED],
      [200, UPDATE_PROMPTED],
      [400, UPDATE_REJECTED],
      [200, UPDATED],
    ]);
    renderScreen();
    await answered(seen, 1);

    type(selectorAt(6), "U");
    press("ENTER");
    await answered(seen, 2);
    expect(sent(seen, 2, `POST ${ACTIONS_PATH}`).body).toEqual({
      aid: "ENTER",
      type_filter: "",
      desc_filter: "",
      rows: withRow(unchangedRows(SEEDED_TYPES), 6, { selection: "U", description: "REVERAL" }),
      state: LOADED_STATE,
    });
    expect(messageRow("info").textContent).toBe(CENTRED_UPDATE_PROMPT);
    expect(messageRow("error").textContent).toBe(NO_CHANGE_DETECTED);
    // The pending update's description is the one enterable description, with the cursor.
    expect(descriptionAt(6)).toHaveProperty("readOnly", false);
    await cursorOn(() => descriptionAt(6));
    for (const row of [1, 2, 3, 4, 5, 7]) {
      expect(descriptionAt(row)).toHaveProperty("readOnly", true);
    }

    type(descriptionAt(6), "REVERSAL!");
    // F10 pressed with the cursor moved back to the row's selector.
    await moveCursor(selectorAt(6));
    pressKey("F10");
    await answered(seen, 3);
    expect(sent(seen, 3, `POST ${ACTIONS_PATH}`).body).toEqual({
      aid: "PF10",
      type_filter: "",
      desc_filter: "",
      rows: withRow(unchangedRows(SEEDED_TYPES), 6, {
        selection: "U",
        description: "REVERSAL!",
      }),
      state: UPDATE_PENDING_STATE,
    });
    expect(messageRow("error").textContent).toBe(DESCRIPTION_NOT_ALPHANUMERIC);
    // The row error stands beside its row, on the field it flags.
    expect(rowErrorText(6)).toBe(DESCRIPTION_NOT_ALPHANUMERIC);
    expect(descriptionAt(6).getAttribute("aria-invalid")).toBe("true");
    expect(descriptionAt(6)).toHaveProperty("value", "REVERSAL!");
    await cursorOn(() => descriptionAt(6));
    for (const row of [1, 2, 3, 4, 5, 7]) {
      expect(rowErrorText(row)).toBeUndefined();
    }

    type(descriptionAt(6), "REVERSAL");
    pressKey("F10");
    await answered(seen, 4);
    expect(sent(seen, 4, `POST ${ACTIONS_PATH}`).body).toEqual({
      aid: "PF10",
      type_filter: "",
      desc_filter: "",
      rows: withRow(unchangedRows(SEEDED_TYPES), 6, { selection: "U", description: "REVERSAL" }),
      state: UPDATE_REJECTED_STATE,
    });
    expect(messageRow("info").textContent).toBe(CENTRED_ROW_UPDATED);
    expect(messageRow("error").textContent).toBe("");
    expect(selectorAt(6)).toHaveProperty("value", "");
    expect(descriptionAt(6)).toHaveProperty("value", "REVERSAL");
    expect(descriptionAt(6)).toHaveProperty("readOnly", true);
    expect(descriptionAt(6).getAttribute("aria-invalid")).toBe("false");
    expect(rowErrorText(6)).toBeUndefined();
  });

  it("an invalid action code is flagged beside its row, the cursor on it", async () => {
    // audit: COTRTLIC-056
    const seen = stubFetch([
      [200, LOADED],
      [400, INVALID_SELECTION],
    ]);
    renderScreen();
    await answered(seen, 1);

    type(selectorAt(2), "X");
    fireEvent.submit(form());
    await answered(seen, 2);

    expect(sent(seen, 2, `POST ${ACTIONS_PATH}`).body).toEqual({
      aid: "ENTER",
      type_filter: "",
      desc_filter: "",
      rows: withRow(unchangedRows(SEEDED_TYPES), 2, { selection: "X", description: "PAYMENT" }),
      state: LOADED_STATE,
    });
    expect(messageRow("error").textContent).toBe(INVALID_ACTION_CODE);
    expect(messageRow("info").textContent).toBe(CENTRED_RECORD_ACTIONS);
    expect(rowErrorText(2)).toBe(INVALID_ACTION_CODE);
    expect(selectorAt(2).getAttribute("aria-invalid")).toBe("true");
    expect(selectorAt(2)).toHaveProperty("value", "X");
    await cursorOn(() => selectorAt(2));
    for (const row of [1, 3, 4, 5, 6, 7]) {
      expect(selectorAt(row).getAttribute("aria-invalid")).toBe("false");
      expect(rowErrorText(row)).toBeUndefined();
    }
  });

  it.each([
    { field: "type_filter", label: "Type Filter:" },
    { field: "desc_filter", label: "Description Filter:" },
    { field: "rows[2].selection", label: "Select type 03" },
    { field: "rows[2].description", label: "Description type 03" },
  ])(
    "a 400 ScreenError naming $field shows its message, with the cursor on $label",
    async ({ field, label }) => {
      // audit: COTRTLI-076
      const seen = stubFetch([
        [200, LOADED],
        [400, fieldTooLong(field)],
      ]);
      renderScreen();
      await answered(seen, 1);

      // ENTER pressed with the cursor on row 1's selector, a field no case names.
      await moveCursor(selectorAt(1));
      press("ENTER");
      await answered(seen, 2);

      sent(seen, 2, `POST ${ACTIONS_PATH}`);
      expect(messageRow("error").textContent).toBe(FIELD_TOO_LONG);
      expect(screen.getByLabelText(label).getAttribute("aria-invalid")).toBe("true");
      await cursorOn(() => screen.getByLabelText(label));
      // The page already shown stays.
      expect(codesShown()).toEqual(SEEDED_TYPES.map(([code]) => code));
    },
  );

  it("F8 and F7 send the shown filters and last page_key, never a page size", async () => {
    // audit: COTRTLI-074
    // audit: COTRTLI-090
    // audit: COTRTLI-091
    // audit: COTRTLIC-056
    const seen = stubFetch([
      [200, LOADED],
      [200, LAST_PAGE_AGAIN],
      [200, FIRST_PAGE_AGAIN],
      [200, FILTERED],
    ]);
    renderScreen();
    await answered(seen, 1);

    press("F8=Page Dn");
    await answered(seen, 2);
    expect(queryOf(sent(seen, 2, `GET ${LIST_PATH}`))).toEqual({
      type_filter: "",
      desc_filter: "",
      page_key: LOADED.page_key,
      direction: "forward",
    });
    expect(messageRow("error").textContent).toBe(NO_MORE_PAGES_FOR_SEARCH);
    expect(messageRow("info").textContent).toBe(CENTRED_RECORD_ACTIONS);
    expect(pageNo()).toHaveProperty("value", "1");

    pressKey("F7");
    await answered(seen, 3);
    expect(queryOf(sent(seen, 3, `GET ${LIST_PATH}`))).toEqual({
      type_filter: "",
      desc_filter: "",
      page_key: LAST_PAGE_AGAIN.page_key,
      direction: "backward",
    });
    expect(messageRow("error").textContent).toBe(NO_PREVIOUS_PAGES);
    // No info message: the row stays empty, with no visible box.
    expect(messageRow("info").textContent).toBe("");
    expect(messageRow("info").className).toContain("border-transparent");

    await moveCursor(descFilter());
    type(descFilter(), "R");
    pressKey("F8");
    await answered(seen, 4);
    expect(queryOf(sent(seen, 4, `GET ${LIST_PATH}`))).toEqual({
      type_filter: "",
      desc_filter: "R",
      page_key: FIRST_PAGE_AGAIN.page_key,
      direction: "forward",
    });
    expect(descFilter()).toHaveProperty("value", "R");
    expect(messageRow("error").textContent).toBe("");
    expect(messageRow("info").textContent).toBe(CENTRED_RECORD_ACTIONS);
    expect(codesShown()).toEqual(["01", "03", "04", "05", "06", "", ""]);
    // A selector is enterable only on a row that holds a type.
    for (const row of [1, 2, 3, 4, 5]) {
      expect(selectorAt(row)).toHaveProperty("readOnly", false);
    }
    for (const row of [6, 7]) {
      expect(selectorAt(row).getAttribute("aria-label")).toBe(`Select row ${row}`);
      expect(selectorAt(row)).toHaveProperty("readOnly", true);
      expect(selectorAt(row).getAttribute("tabindex")).toBe("-1");
      expect(descriptionAt(row)).toHaveProperty("value", "");
    }
    // Paging with no selection leaves the filters enterable, the cursor on the type filter.
    expect(typeFilter()).toHaveProperty("readOnly", false);
    await cursorOn(typeFilter);
  });

  it.each([
    {
      name: "the type filter A1",
      label: "Type Filter:",
      typed: "A1",
      reply: TYPE_FILTER_REJECTED,
      message: TYPE_FILTER_NOT_NUMERIC,
      query: { type_filter: "A1", desc_filter: "" },
    },
    {
      name: "the description filter ZZ",
      label: "Description Filter:",
      typed: "ZZ",
      reply: NO_MATCH,
      message: NO_RECORDS_FOR_FILTERS,
      query: { type_filter: "", desc_filter: "ZZ" },
    },
  ])(
    "F8 with $name shows the 400 page with that filter in error",
    async ({ label, typed, reply, message, query }) => {
      // audit: COTRTLI-013
      // audit: COTRTLIC-056
      // audit: COTRTLIC-057
      const seen = stubFetch([
        [200, LOADED],
        [400, reply],
      ]);
      renderScreen();
      await answered(seen, 1);

      type(screen.getByLabelText(label), typed);
      // F8 clicked, which takes the cursor to its button.
      await moveCursor(screen.getByRole("button", { name: "F8=Page Dn" }));
      press("F8=Page Dn");
      await answered(seen, 2);

      expect(queryOf(sent(seen, 2, `GET ${LIST_PATH}`))).toEqual({
        ...query,
        page_key: LOADED.page_key,
        direction: "forward",
      });
      expect(messageRow("error").textContent).toBe(message);
      expect(messageRow("info").textContent).toBe("");
      const filter = screen.getByLabelText(label);
      expect(filter).toHaveProperty("value", typed);
      expect(filter.getAttribute("aria-invalid")).toBe("true");
      await cursorOn(() => screen.getByLabelText(label));
      expect(pageNo()).toHaveProperty("value", "0");
      // After a filter error every selector is read-only.
      for (const row of [1, 2, 3, 4, 5, 6, 7]) {
        expect(selectorAt(row)).toHaveProperty("readOnly", true);
      }
    },
  );

  it("a ScreenError on F8 shows its message over the page already shown", async () => {
    const seen = stubFetch([
      [200, LOADED],
      [500, DB2_DOWN],
    ]);
    renderScreen();
    await answered(seen, 1);

    press("F8=Page Dn");
    await answered(seen, 2);

    sent(seen, 2, `GET ${LIST_PATH}`);
    expect(messageRow("error").textContent).toBe(DB2_UNREACHABLE);
    expect(codesShown()).toEqual(SEEDED_TYPES.map(([code]) => code));
    expect(pageNo()).toHaveProperty("value", "1");
  });

  it("a first entry whose page read fails shows the page and its Db2 message", async () => {
    const seen = stubFetch([[500, OPEN_FAILED]]);
    renderScreen();
    await answered(seen, 1);

    expect(messageRow("error").textContent).toBe(FORWARD_OPEN_FAILED);
    expect(codesShown()).toEqual(["", "", "", "", "", "", ""]);
    expect(pageNo()).toHaveProperty("value", "1");
    for (const row of [1, 2, 3, 4, 5, 6, 7]) {
      expect(selectorAt(row)).toHaveProperty("readOnly", true);
    }
  });

  it.each(["ENTER", "F7=Page Up", "F8=Page Dn", "F10=Save"])(
    "after a first entry that reached no database, %s reads the first page again",
    async (label) => {
      const seen = stubFetch([
        [500, DB2_DOWN],
        [200, LOADED],
      ]);
      renderScreen();
      await answered(seen, 1);
      expect(messageRow("error").textContent).toBe(DB2_UNREACHABLE);
      expect(codesShown()).toEqual(["", "", "", "", "", "", ""]);
      expect(pageNo()).toHaveProperty("value", "");

      press(label);
      await answered(seen, 2);

      expect(sent(seen, 2, `GET ${LIST_PATH}`).url.search).toBe("");
      expect(codesShown()).toEqual(SEEDED_TYPES.map(([code]) => code));
      expect(messageRow("error").textContent).toBe("");
    },
  );

  it.each([
    { title: "the first entry", prior: [], start: null, answer: [200, LOADED], filtersOpen: true },
    {
      title: "ENTER by its button",
      prior: [LOADED],
      start: { via: "button", name: "ENTER" },
      answer: [200, REFRESHED],
      filtersOpen: true,
    },
    {
      title: "Enter on the keyboard",
      prior: [LOADED],
      start: { via: "key", name: "Enter" },
      answer: [200, REFRESHED],
      filtersOpen: true,
    },
    {
      title: "F10 by its button",
      prior: [LOADED],
      start: { via: "button", name: "F10=Save" },
      answer: [200, REFRESHED],
      filtersOpen: true,
    },
    {
      title: "F8 by its button",
      prior: [LOADED],
      start: { via: "button", name: "F8=Page Dn" },
      answer: [200, LAST_PAGE_AGAIN],
      filtersOpen: true,
    },
    {
      title: "F7 on the keyboard",
      prior: [LOADED],
      start: { via: "key", name: "F7" },
      answer: [200, FIRST_PAGE_AGAIN],
      filtersOpen: true,
    },
    {
      title: "F10 on the keyboard with an update pending",
      prior: [LOADED, UPDATE_PROMPTED],
      start: { via: "key", name: "F10" },
      answer: [400, UPDATE_REJECTED],
      filtersOpen: false,
      description: 6,
    },
  ] satisfies readonly {
    readonly title: string;
    readonly prior: readonly components["schemas"]["TranTypeListResponse"][];
    readonly start: { readonly via: "button" | "key"; readonly name: string } | null;
    readonly answer: readonly [number, unknown];
    readonly filtersOpen: boolean;
    readonly description?: number;
  }[])(
    "while $title is out, its keys and F2 are disabled and send nothing, F3 stays, and the inputs wait",
    async (example) => {
      const { prior, start, answer, filtersOpen } = example;
      const description = "description" in example ? example.description : undefined;
      const held = heldReply();
      const seen = stubFetch([
        ...prior.map((page): readonly [number, unknown] => [200, page]),
        held.reply,
        [200, REFRESHED],
      ]);
      renderScreen();
      for (let shown = 1; shown <= prior.length; shown += 1) {
        if (shown > 1) {
          press("ENTER");
        }
        await answered(seen, shown);
      }
      if (description !== undefined) {
        expect(descriptionAt(description)).toHaveProperty("readOnly", false);
      }
      // F2 sends no request, but its transfer takes the same lock, so it waits for the answer too.
      const refused = ["ENTER", "F2=Add", "F7=Page Up", "F8=Page Dn", "F10=Save"];
      const available = ["F3=Exit"];
      /**
       * Every key that sends a request, and F2, by its button and on the keyboard; F1 counts as
       * ENTER.
       */
      function pressRequestKeys(): void {
        for (const label of refused) {
          press(label);
        }
        for (const key of ["Enter", "F2", "F7", "F8", "F10", "F1"]) {
          fireEvent.keyDown(window, { key });
        }
      }
      /** The key that starts the request, by its button or on the keyboard. */
      function startRequest(): void {
        if (start?.via === "button") {
          press(start.name);
        } else if (start?.via === "key") {
          fireEvent.keyDown(window, { key: start.name });
        }
      }

      // The key twice and every other request key in one batch, before the screen re-renders:
      // only the lock the first key took refuses the rest.
      act(() => {
        startRequest();
        startRequest();
        pressRequestKeys();
      });
      await waitFor(() => {
        expect(seen).toHaveLength(prior.length + 1);
      });

      expect(functionKeys().getAttribute("aria-busy")).toBe("true");
      expect(form().getAttribute("aria-busy")).toBe("true");
      for (const label of refused) {
        expect(keyButton(label).getAttribute("aria-disabled")).toBe("true");
      }
      for (const label of available) {
        expect(keyButton(label).getAttribute("aria-disabled")).toBeNull();
      }
      expect(typeFilter()).toHaveProperty("readOnly", true);
      expect(descFilter()).toHaveProperty("readOnly", true);
      for (const row of [1, 2, 3, 4, 5, 6, 7]) {
        expect(selectorAt(row)).toHaveProperty("readOnly", true);
        expect(descriptionAt(row)).toHaveProperty("readOnly", true);
      }

      pressRequestKeys();
      await settle();
      expect(seen).toHaveLength(prior.length + 1);

      held.release(json(answer[0], answer[1]));
      await answered(seen, prior.length + 1);
      expect(functionKeys().getAttribute("aria-busy")).toBe("false");
      for (const label of [...refused, ...available]) {
        expect(keyButton(label).getAttribute("aria-disabled")).toBeNull();
      }
      expect(typeFilter()).toHaveProperty("readOnly", !filtersOpen);
      expect(descFilter()).toHaveProperty("readOnly", !filtersOpen);
      for (const row of [1, 2, 3, 4, 5, 6, 7]) {
        expect(selectorAt(row)).toHaveProperty("readOnly", false);
        expect(descriptionAt(row)).toHaveProperty("readOnly", row !== description);
      }

      // The keyboard is open again: the next ENTER is sent.
      press("ENTER");
      await answered(seen, prior.length + 2);
      expect(sent(seen, prior.length + 2, `POST ${ACTIONS_PATH}`).body).toHaveProperty(
        "aid",
        "ENTER",
      );
    },
  );

  it.each([
    { name: "Enter", key: "Enter", shiftKey: false, aid: "ENTER" },
    { name: "F1", key: "F1", shiftKey: false, aid: "ENTER" },
    { name: "F4", key: "F4", shiftKey: false, aid: "ENTER" },
    { name: "F5", key: "F5", shiftKey: false, aid: "ENTER" },
    { name: "F6", key: "F6", shiftKey: false, aid: "ENTER" },
    { name: "F9", key: "F9", shiftKey: false, aid: "ENTER" },
    { name: "F11", key: "F11", shiftKey: false, aid: "ENTER" },
    { name: "F12", key: "F12", shiftKey: false, aid: "ENTER" },
    { name: "Shift+F4 (PF16)", key: "F4", shiftKey: true, aid: "ENTER" },
    { name: "F10", key: "F10", shiftKey: false, aid: "PF10" },
    { name: "Shift+F10 (PF22)", key: "F10", shiftKey: true, aid: "PF10" },
  ])("$name is sent to endpoint 25 as $aid", async ({ key, shiftKey, aid }) => {
    // audit: COTRTLI-093
    // audit: COTRTLIC-014
    const seen = stubFetch([
      [200, LOADED],
      [200, REFRESHED],
    ]);
    renderScreen();
    await answered(seen, 1);

    // The key goes to the window, outside the form's fields, where no browser action takes it.
    fireEvent.keyDown(window, { key, shiftKey });
    await answered(seen, 2);

    expect(sent(seen, 2, `POST ${ACTIONS_PATH}`).body).toEqual({
      aid,
      type_filter: "",
      desc_filter: "",
      rows: unchangedRows(SEEDED_TYPES),
      state: LOADED_STATE,
    });
    expect(messageRow("info").textContent).toBe(CENTRED_RECORD_ACTIONS);
  });

  it("an input longer than its map field is refused on screen and sends nothing", async () => {
    const seen = stubFetch([[200, LOADED]]);
    renderScreen();
    await answered(seen, 1);

    // `maxlength` stops typing at 2; a value set past it meets the schema's length check.
    type(typeFilter(), "A12");
    press("ENTER");

    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(TYPE_FILTER_TOO_LONG);
    });
    expect(seen).toHaveLength(1);
  });

  it.each([
    { label: "F2=Add", key: "F2", via: "button", target: "COTRTUP" },
    { label: "F2=Add", key: "F2", via: "key", target: "COTRTUP" },
    { label: "F3=Exit", key: "F3", via: "button", target: "COADM01" },
    { label: "F3=Exit", key: "F3", via: "key", target: "COADM01" },
  ])("$label by its $via opens $target and sends nothing", async ({ label, key, via, target }) => {
    // audit: COTRTLI-088
    // audit: COTRTLI-089
    // audit: COTRTLIC-015
    // audit: COTRTLIC-016
    const seen = stubFetch([[200, LOADED]]);
    // Arrived from CTTU's F3. COTRTLIC records itself as the caller on every display, so its
    // "back to the screen that transferred here, else /admin" (COTRTLI-089) always takes the
    // admin menu, never the screen it came from (COTRTLIC-015).
    renderScreen({ from: PATHS.COTRTUP });
    await answered(seen, 1);

    if (via === "button") {
      press(label);
    } else {
      pressKey(key);
    }

    // Only `from` travels: the list state is discarded.
    expect(await arrivedAt(target)).toEqual({ from: PATHS.COTRTLI });
    expect(seen).toHaveLength(1);
  });

  // The replacement's accessibility wiring, which no audit row states: every `aria-describedby`
  // names only elements the screen renders. No input of the map has a hint, so a valid input is
  // described by nothing, and the input an error marks only by its field message, which holds
  // the error line's text for assistive technology alone.
  it.each([
    { state: "on entry", replies: [[200, LOADED]], act: async () => undefined, message: "" },
    {
      state: "after ENTER answered by the 400 of an invalid action code",
      replies: [
        [200, LOADED],
        [400, INVALID_SELECTION],
      ],
      act: async () => {
        type(selectorAt(2), "X");
        press("ENTER");
      },
      marked: "selector 2",
      message: INVALID_ACTION_CODE,
    },
    {
      state: "after F10 answered by the 400 of a rejected description",
      replies: [
        [200, LOADED],
        [200, UPDATE_PROMPTED],
        [400, UPDATE_REJECTED],
      ],
      act: async (seen: ReturnType<typeof stubFetch>) => {
        type(selectorAt(6), "U");
        press("ENTER");
        await answered(seen, 2);
        type(descriptionAt(6), "REVERSAL!");
        press("F10=Save");
      },
      marked: "description 6",
      message: DESCRIPTION_NOT_ALPHANUMERIC,
    },
    {
      state: "after ENTER answered by a 400 naming a protected description",
      replies: [
        [200, LOADED],
        [400, fieldTooLong("rows[2].description")],
      ],
      act: async () => {
        press("ENTER");
      },
      marked: "description 3",
      message: FIELD_TOO_LONG,
    },
    {
      state: "after F8 answered by the 400 of a type filter in error",
      replies: [
        [200, LOADED],
        [400, TYPE_FILTER_REJECTED],
      ],
      act: async () => {
        type(typeFilter(), "A1");
        press("F8=Page Dn");
      },
      marked: "typeFilter",
      message: TYPE_FILTER_NOT_NUMERIC,
    },
    {
      state: "after F8 answered by the 400 of a description filter matching nothing",
      replies: [
        [200, LOADED],
        [400, NO_MATCH],
      ],
      act: async () => {
        type(descFilter(), "ZZ");
        press("F8=Page Dn");
      },
      marked: "descFilter",
      message: NO_RECORDS_FOR_FILTERS,
    },
    {
      state: "after a type filter longer than its map field, with no request",
      replies: [[200, LOADED]],
      act: async () => {
        type(typeFilter(), "A12");
        press("ENTER");
      },
      marked: "typeFilter",
      message: TYPE_FILTER_TOO_LONG,
    },
  ] satisfies {
    state: string;
    replies: (readonly [number, unknown])[];
    act: (seen: ReturnType<typeof stubFetch>) => Promise<void>;
    marked?: string;
    message: string;
  }[])("describes each input by rendered elements only, $state", async (row) => {
    const seen = stubFetch(row.replies);
    renderScreen();
    await answered(seen, 1);
    await row.act(seen);
    await answered(seen, row.replies.length);
    await waitFor(() => {
      expect(messageRow("error").textContent).toBe(row.message);
    });

    expect(danglingDescriptions()).toEqual([]);
    expect(
      Object.fromEntries(
        Object.entries(namedInputs()).map(([name, input]) => [name, readOut(input)]),
      ),
    ).toEqual(readOutsWith(row.marked, row.message));
  });
});

// Endpoint 25 takes the filters, rows and state in its JSON body alone, and every request
// carries the stored session's token.
describe("COTRTLI: endpoint 24 and 25 requests", () => {
  it("the first page and ENTER's list action each carry the bearer token, the action no query, and the prompt shows", async () => {
    const seen = stubFetch([
      [200, LOADED],
      [200, DELETE_PROMPTED],
    ]);
    renderScreen();
    await answered(seen, 1);

    type(selectorAt(1), "D");
    press("ENTER");
    await answered(seen, 2);
    expect(messageRow("info").textContent).toBe(CENTRED_DELETE_PROMPT);

    expect(
      seen.map(({ method, url, authorization }) => ({
        route: `${method} ${url.pathname}`,
        query: queryOf({ url }),
        authorization,
      })),
    ).toEqual([
      { route: `GET ${LIST_PATH}`, query: {}, authorization: "Bearer test-token" },
      { route: `POST ${ACTIONS_PATH}`, query: {}, authorization: "Bearer test-token" },
    ]);
  });
});

/** The 401 of the `AdminUser` extractor for a missing, invalid or expired token. */
const SIGNON_REQUIRED: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};

/**
 * Renders the application's route table at `/transaction-types`, so the admin guard that only the
 * table mounts stands in front of the screen. Each screen is imported by its route's loader, so a
 * wait for one to render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COTRTLI] });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

// A 401 clears the stored session (`api/client.ts`), and the admin guard in front of the route
// replaces the still-mounted screen with sign-on at once: after the request the screen sent, the
// test presses no key and follows no link.
describe("COTRTLI in the route table: a 401 ends the session", () => {
  it.each([
    {
      title: "the list of the first entry (endpoint 24)",
      replies: [[401, SIGNON_REQUIRED]] as const,
      drive: async (seen: ReturnType<typeof stubFetch>) => {
        await waitFor(() => {
          expect(seen).toHaveLength(1);
        });
      },
      calls: [`GET ${LIST_PATH}`],
    },
    {
      title: "the list action of ENTER (endpoint 25)",
      replies: [
        [200, LOADED],
        [401, SIGNON_REQUIRED],
      ] as const,
      drive: async (seen: ReturnType<typeof stubFetch>) => {
        await answered(seen, 1);
        type(selectorAt(1), "D");
        press("ENTER");
      },
      calls: [`GET ${LIST_PATH}`, `POST ${ACTIONS_PATH}`],
    },
  ])(
    "$title answered 401 replaces the screen with sign-on and clears the session, with no retry",
    async ({ replies, drive, calls }) => {
      const seen = stubFetch(replies);
      const router = renderRouteTable();

      await drive(seen);

      expect(
        await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
      ).toBeDefined();
      expect(router.state.location.pathname).toBe(PATHS.COSGN00);
      expect(screen.queryByRole("table", { name: "Transaction types" })).toBeNull();
      expect(
        screen.queryByRole("heading", { level: 1, name: "Maintain Transaction Type" }),
      ).toBeNull();
      expect(getSession()).toBeNull();
      expect(seen.map((request) => `${request.method} ${request.url.pathname}`)).toEqual(calls);
      expect(seen.every((request) => request.authorization === "Bearer test-token")).toBe(true);
      router.dispose();
    },
  );
});

/**
 * A route `lazy` that stays loading until `load` is called, as a screen's module does on a slow
 * network, then renders `Component`.
 */
function slowRoute(Component: ComponentType): {
  readonly lazy: () => Promise<{ Component: ComponentType }>;
  readonly load: () => void;
} {
  let resolveModule: () => void = () => undefined;
  const loaded = new Promise<{ Component: ComponentType }>((resolve) => {
    resolveModule = () => {
      resolve({ Component });
    };
  });
  return {
    lazy: () => loaded,
    load: () => {
      resolveModule();
    },
  };
}

// F2 leaves the list on screen until COTRTUP's module has loaded. F2 takes the keyboard lock, as
// a request does, and holds it through that wait, as the terminal keyboard stays locked until the
// next program sends its map. The screen sits in a data router without the application's root
// element, so only the screen's own lock acts.
describe("COTRTLI: F2 while the maintenance screen loads", () => {
  it.each([{ via: "button" }, { via: "key" }])(
    "F2 by its $via locks the keyboard until COTRTUP replaces the list, and no key sends a request meanwhile",
    async ({ via }) => {
      const seen = stubFetch([
        [200, LOADED],
        [200, REFRESHED],
      ]);
      const slow = slowRoute(() => <Target name="COTRTUP" />);
      const screens: RouteObject[] = [
        { path: PATHS.COTRTLI, element: <COTRTLI /> },
        { path: PATHS.COTRTUP, lazy: slow.lazy },
      ];
      const router = createMemoryRouter(screens, { initialEntries: [PATHS.COTRTLI] });
      const client = new QueryClient({
        defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
      });
      render(
        <QueryClientProvider client={client}>
          <RouterProvider router={router} />
        </QueryClientProvider>,
      );
      await answered(seen, 1);

      if (via === "button") {
        press("F2=Add");
      } else {
        fireEvent.keyDown(window, { key: "F2" });
      }
      await waitFor(() => {
        expect(router.state.navigation.state).toBe("loading");
        expect(form().getAttribute("aria-busy")).toBe("true");
      });
      expect(functionKeys().getAttribute("aria-busy")).toBe("true");
      expect(typeFilter()).toHaveProperty("readOnly", true);
      expect(descFilter()).toHaveProperty("readOnly", true);
      for (const row of [1, 2, 3, 4, 5, 6, 7]) {
        expect(selectorAt(row)).toHaveProperty("readOnly", true);
        expect(descriptionAt(row)).toHaveProperty("readOnly", true);
      }

      // Every key that sends a request, and F2 again, by its button and on the keyboard; F1 is
      // ENTER (COTRTLIC-014).
      for (const label of ["ENTER", "F2=Add", "F7=Page Up", "F8=Page Dn", "F10=Save"]) {
        press(label);
      }
      for (const key of ["Enter", "F1", "F2", "F7", "F8", "F10"]) {
        fireEvent.keyDown(window, { key });
      }
      await settle();

      expect(seen).toHaveLength(1);
      expect(router.state.location.pathname).toBe(PATHS.COTRTLI);

      slow.load();
      expect(await arrivedAt("COTRTUP")).toEqual({ from: PATHS.COTRTLI });
      expect(router.state.location.pathname).toBe(PATHS.COTRTUP);
      expect(seen).toHaveLength(1);
      router.dispose();
    },
  );
});
