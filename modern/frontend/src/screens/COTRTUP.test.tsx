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
import { getSession, setSession } from "@/auth/session";
import { PATHS, routes } from "@/routes";
import { COTRTUP } from "./COTRTUP";

/*
 * Render test of the transaction type maintenance screen (map CTRTUPA, transaction CTTU, program
 * COTRTUPC) in jsdom, with `fetch` stubbed for endpoint 26
 * (`GET /api/v1/transaction-types/{type_code}`) and endpoint 27
 * (`POST /api/v1/transaction-types/{type_code}/actions`). Expected texts are the audited literals
 * of `modern/audit/screens.md` (`COTRTUP-nnn`) and `modern/audit/online/COTRTUPC.md`
 * (`COTRTUPC-nnn`).
 *
 * The screen holds the COTRTUPC state between requests and sends none of it: endpoint 27 takes
 * `{ aid, description? }` and re-reads the row (`TranTypeMaintState`: "no request sends it";
 * COTRTUPC.md, "Endpoint mapping"). Every request body is therefore asserted whole, with no state
 * member.
 */

const CODE = "01";
const STORED = "PURCHASE";
const EDITED = "PURCHASES";
const NEW_CODE = "09";
const NEW_DESCRIPTION = "NEW TYPE";

const FETCH = `GET /api/v1/transaction-types/${CODE}`;
const ACTIONS = `POST /api/v1/transaction-types/${CODE}/actions`;
const FETCH_NEW = `GET /api/v1/transaction-types/${NEW_CODE}`;
const ACTIONS_NEW = `POST /api/v1/transaction-types/${NEW_CODE}/actions`;

const BEARER = "Bearer test-token";

/** COTRTUP-012 and COTRTUP-015: the map literals that label the two inputs, every space kept. */
const LABELS = {
  code: "Transaction Type  :",
  description: "Description       :",
} as const;

/** COTRTUP-027 and COTRTUP-030: the two 40-byte `COTTL01Y` titles. */
const TITLE01 = " ".repeat(6) + "AWS Mainframe Modernization" + " ".repeat(7);
const TITLE02 = " ".repeat(14) + "CardDemo" + " ".repeat(18);

/** COTRTUPC-045: `WS-INFO-MSG` of each state, as a success body carries it. */
const INFO_TEXT = {
  search: "Enter transaction type to be maintained",
  notFound: "Press F05 to add. F12 to cancel",
  newRecord: "Enter new transaction type details.",
  changes: "Update transaction type details shown.",
  validated: "Changes validated.Press F5 to save",
  committed: "Changes committed to database",
  unsuccessful: "Changes unsuccessful",
  confirmDelete: "Delete this record ? Press F4 to confirm",
  deleted: "Delete successful.",
} as const;

/**
 * COTRTUPC-045: the info row as shown, centred in 40 positions after floor((40 − length) / 2)
 * spaces; the counts follow from the lengths 39, 31, 35, 38, 34, 29, 20, 40 and 18.
 */
const INFO = {
  search: INFO_TEXT.search,
  notFound: " ".repeat(4) + INFO_TEXT.notFound,
  newRecord: " ".repeat(2) + INFO_TEXT.newRecord,
  changes: " ".repeat(1) + INFO_TEXT.changes,
  validated: " ".repeat(3) + INFO_TEXT.validated,
  committed: " ".repeat(5) + INFO_TEXT.committed,
  unsuccessful: " ".repeat(10) + INFO_TEXT.unsuccessful,
  confirmDelete: INFO_TEXT.confirmDelete,
  deleted: " ".repeat(11) + INFO_TEXT.deleted,
} as const;

/** Error rows the screen sets without a request (COTRTUPC-039, -040, -042). */
const MSG = {
  notFound: "No record found for this key in database",
  updateCancelled: "Update was cancelled",
  deleteCancelled: "Delete was cancelled",
  invalidKey: "Invalid key pressed",
  codeNotSupplied: "Tran Type code must be supplied.",
} as const;

/** The row-24 labels the screen renders, in map order; `F6=Add` has no button (COTRTUP-036). */
const ALL_KEYS = ["ENTER=Process", "F3=Exit", "F4=Delete", "F5=Save", "F12=Cancel"];

/** COTRTUPC-037: `ABEND-CODE`, `ABEND-CULPRIT`, `ABEND-REASON` and `ABEND-MSG` (4 + 8 + 50 + 72). */
const ABEND_DATA =
  "9999" + "COTRTUPC" + " ".repeat(50) + "UNEXPECTED DATA SCENARIO".padEnd(72, " ");

/** `TRTYDSCO PIC X(50)`: a validated or saved description, padded to 50 (COTRTUPC-020, -023). */
function padded(text: string): string {
  return text.padEnd(50, " ");
}

/**
 * The Db2 text that follows each store-failure prefix is HALTED(H3) (COTRTUPC-049); the screen
 * reads only the prefix, so a stand-in follows it here.
 */
const DB2_TEXT = " -999:stand-in for the Db2 text";

/** Endpoint 26, a row found: state `show_details` (COTRTUPC-009). */
const found: components["schemas"]["TranTypeView"] = {
  type_code: CODE,
  description: STORED,
  info_message: INFO_TEXT.search,
  message: null,
  state: "show_details",
};

/** Endpoint 26, a valid code no row holds: 200 without a description (COTRTUPC-010). */
const notFound: components["schemas"]["TranTypeView"] = {
  type_code: NEW_CODE,
  info_message: INFO_TEXT.notFound,
  message: MSG.notFound,
  state: "details_not_found",
};

/** Endpoint 27, `aid` ENTER with a valid change: nothing written (COTRTUPC-020). */
const validated: components["schemas"]["TranTypeActionResult"] = {
  committed: false,
  deleted: false,
  description: padded(EDITED),
  info_message: INFO_TEXT.validated,
  message: null,
  state: "changes_ok_not_confirmed",
  type_code: CODE,
};

/** Endpoint 27, `aid` PF5: the row updated and committed (COTRTUPC-024). */
const saved: components["schemas"]["TranTypeActionResult"] = {
  committed: true,
  deleted: false,
  description: padded(EDITED),
  info_message: INFO_TEXT.committed,
  message: null,
  state: "changes_okayed_and_done",
  type_code: CODE,
};

/** Endpoint 27, `aid` ENTER for a type no row holds (COTRTUPC-021, -020). */
const newValidated: components["schemas"]["TranTypeActionResult"] = {
  ...validated,
  description: padded(NEW_DESCRIPTION),
  type_code: NEW_CODE,
};

/** Endpoint 27, `aid` PF5 for a type no row holds: the insert (COTRTUPC-026). */
const newSaved: components["schemas"]["TranTypeActionResult"] = {
  ...saved,
  description: padded(NEW_DESCRIPTION),
  type_code: NEW_CODE,
};

/** Endpoint 27, `aid` PF4: the row deleted, no description returned (COTRTUPC-032). */
const deleted: components["schemas"]["TranTypeActionResult"] = {
  committed: true,
  deleted: true,
  info_message: INFO_TEXT.deleted,
  message: null,
  state: "delete_done",
  type_code: CODE,
};

const codeNotNumeric: components["schemas"]["ScreenError"] = {
  message: "Tran Type code must be numeric.",
  field: "type_code",
};
const codeZero: components["schemas"]["ScreenError"] = {
  message: "Tran Type code must not be zero.",
  field: "type_code",
};
const fetchFailed: components["schemas"]["ScreenError"] = {
  message: `Error accessing: TRANSACTION_TYPE table. SQLCODE:${DB2_TEXT}`,
  field: null,
};
const descriptionNotSupplied: components["schemas"]["ScreenError"] = {
  message: "Transaction Desc must be supplied.",
  field: "description",
};
const descriptionNotAlphanumeric: components["schemas"]["ScreenError"] = {
  message: "Transaction Desc can have numbers or alphabets only.",
  field: "description",
};
const noChange: components["schemas"]["ScreenError"] = {
  message: "No change detected with respect to values fetched.",
  field: "description",
};
const lockFailed: components["schemas"]["ScreenError"] = {
  message: "Could not lock record for update",
  field: null,
};
const updateFailed: components["schemas"]["ScreenError"] = {
  message: `Error updating: TRANSACTION_TYPE Table. SQLCODE:${DB2_TEXT}`,
  field: null,
};
const insertFailed: components["schemas"]["ScreenError"] = {
  message: `Error inserting record into: TRANSACTION_TYPE Table. SQLCODE:${DB2_TEXT}`,
  field: null,
};
const deleteRestricted: components["schemas"]["ScreenError"] = {
  message: `Please delete associated child records first:SQLCODE :${DB2_TEXT}`,
  field: null,
};
const deleteFailed: components["schemas"]["ScreenError"] = {
  message: `Delete failed with message:SQLCODE :${DB2_TEXT}`,
  field: null,
};
const signonRequired: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};
const adminRequired: components["schemas"]["ScreenError"] = {
  message: "No access - Admin Only function ...",
  field: null,
};

/** Text matching that keeps every space, as the map literals do. */
const exactText = { normalizer: (text: string) => text };

/** One stubbed reply: the status and the JSON body. */
function reply(status: number, body: unknown): { status: number; body: unknown } {
  return { status, body };
}

/** A JSON response, as the API sends every success and every `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * Replaces `fetch`. Each request is routed on `<METHOD> <path>` and answered with the next reply
 * listed for that route, a status and body or a response the test holds back (`heldReply`); any
 * other request, and one past the end of its list, gets a 500. Every request is recorded with its
 * JSON body and `Authorization` header. A route listed without replies (as a table row that omits
 * another row's route leaves it) answers like an unlisted one.
 */
function stubApi(replies: {
  readonly [route: string]:
    | readonly ({ status: number; body: unknown } | Promise<Response>)[]
    | undefined;
}) {
  const queues = new Map(
    Object.entries(replies).map(([route, list]) => [route, [...(list ?? [])]]),
  );
  const calls: { route: string; body: unknown; authorization: string | null }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      const route = `${req.method} ${new URL(req.url).pathname}`;
      const body: unknown = req.method === "GET" ? undefined : await req.clone().json();
      calls.push({ route, body, authorization: req.headers.get("Authorization") });
      const next = queues.get(route)?.shift();
      if (next === undefined) {
        const unexpected: components["schemas"]["ScreenError"] = {
          message: `Unexpected request ${route}`,
          field: null,
        };
        return json(500, unexpected);
      }
      if (next instanceof Promise) {
        return next;
      }
      return json(next.status, next.body);
    }),
  );
  return calls;
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

/** The screen a navigation reached, by its path, with the route state it was handed. */
function RouteStub() {
  const location = useLocation();
  return <p>{`Stub at ${location.pathname}, state ${JSON.stringify(location.state)}`}</p>;
}

/** COTRTUP at `/transaction-types/maintain` with `state`, and stubs at its two F3 targets. */
function renderScreen(state?: unknown) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COTRTUP, state }]}>
        <Routes>
          <Route path={PATHS.COTRTUP} element={<COTRTUP />} />
          <Route path={PATHS.COADM01} element={<RouteStub />} />
          <Route path={PATHS.COTRTLI} element={<RouteStub />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/**
 * Lets every pending promise and timer settle, so any request the screen would send has been sent.
 * Run inside `waitFor`, so the form library's own deferred state updates (its validity pass after
 * a reset that follows an edit, which renders nothing different) land where React expects them.
 */
async function settle(): Promise<void> {
  await waitFor(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
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

/** The `<form>` the inputs belong to, which ENTER submits. */
function screenForm(): HTMLFormElement {
  const form = field(LABELS.code).form;
  if (form === null) {
    throw new Error("The type code input belongs to no form");
  }
  return form;
}

/** The values the two inputs show. */
function shownValues(): { code: string; description: string } {
  return { code: field(LABELS.code).value, description: field(LABELS.description).value };
}

/** COTRTUPC-046: which of the two inputs take typing. */
function expectEditable(editable: { code: boolean; description: boolean }) {
  expect({
    code: !field(LABELS.code).readOnly,
    description: !field(LABELS.description).readOnly,
  }).toEqual(editable);
}

/**
 * Which of the two inputs are Tab stops (`tabIndex` 0); the others are skipped (`tabIndex` -1), as
 * the 3270 Tab key skips protected fields.
 */
function expectTabStops(stops: { code: boolean; description: boolean }) {
  const tabIndex = (stop: boolean) => (stop ? 0 : -1);
  expect({
    code: field(LABELS.code).tabIndex,
    description: field(LABELS.description).tabIndex,
  }).toEqual({ code: tabIndex(stops.code), description: tabIndex(stops.description) });
}

/** COTRTUPC-047: which of the two inputs show in red (the destructive token). */
function redFields(): { code: boolean; description: boolean } {
  return {
    code: field(LABELS.code).classList.contains("text-destructive"),
    description: field(LABELS.description).classList.contains("text-destructive"),
  };
}

/**
 * The error text `input` is marked with, or `null` when it is not marked. A marked input is
 * `aria-invalid` and its `aria-describedby` names one rendered `sr-only` element holding the text,
 * which is no message row: `ERRMSG` stays the screen's one visible error message.
 */
function markOf(input: HTMLElement): string | null {
  const hidden = (input.getAttribute("aria-describedby") ?? "")
    .split(/\s+/)
    .filter((id) => id !== "")
    .map((id) => document.getElementById(id))
    .filter((described) => described?.classList.contains("sr-only") === true);
  if (input.getAttribute("aria-invalid") !== "true") {
    expect(hidden).toEqual([]);
    return null;
  }
  expect(hidden).toHaveLength(1);
  const [message] = hidden;
  expect(message?.closest('[role="alert"]')).toBeNull();
  return message?.textContent ?? null;
}

/** The marks of the two inputs, `null` for an input that is not marked. */
function marks(): { code: string | null; description: string | null } {
  return { code: markOf(field(LABELS.code)), description: markOf(field(LABELS.description)) };
}

/** The marks a screen with the error row `error` and the red fields `red` gives the inputs. */
function marksFor(
  red: { code: boolean; description: boolean },
  error: string,
): { code: string | null; description: string | null } {
  return { code: red.code ? error : null, description: red.description ? error : null };
}

/** COTRTUPC-046: the screen moves the cursor in a layout effect after each screen it shows. */
async function expectFocus(label: string) {
  await waitFor(() => {
    expect(document.activeElement).toBe(field(label));
  });
}

/** The row-24 key group. */
function functionKeys(): HTMLElement {
  return screen.getByRole("group", { name: "Function keys" });
}

/**
 * A PF-key button of row 24 by its label, shown or hidden. Matched by its text: a button with the
 * HTML `hidden` attribute has an empty accessible name, so `getByRole` with `hidden: true` finds
 * it but cannot match it by `name`.
 */
function key(label: string): HTMLButtonElement {
  const element = within(functionKeys()).getByText(label, { selector: "button" });
  if (!(element instanceof HTMLButtonElement)) {
    throw new Error(`Expected a button, found <${element.tagName.toLowerCase()}>`);
  }
  return element;
}

/** Every row-24 button, hidden ones included, in order. */
function allKeys(): (string | null)[] {
  return within(functionKeys())
    .getAllByRole("button", { hidden: true })
    .map((button) => button.textContent);
}

/** The row-24 labels shown: a hidden label is out of view and out of the accessibility tree. */
function shownKeys(): (string | null)[] {
  return within(functionKeys())
    .queryAllByRole("button")
    .map((button) => button.textContent);
}

/** A click on the row-24 button labelled `label`. */
function press(label: string) {
  fireEvent.click(key(label));
}

/** A key pressed on the keyboard outside every field, Shift held or not. */
function pressKey(name: string, shiftKey = false) {
  fireEvent.keyDown(document.body, { key: name, shiftKey });
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

/**
 * The value of a header value field (`ScreenShell`), by its BMS name: the one read-only `Input`
 * of that `data-field` in the `CardHeader`, failing when there is not exactly one.
 */
function headerValue(name: string): string {
  const inputs = document.querySelectorAll(
    `[data-slot="card-header"] input[data-field="${name}"]`,
  );
  expect(inputs).toHaveLength(1);
  const [input] = inputs;
  if (!(input instanceof HTMLInputElement)) {
    throw new Error(`The header has no input ${name}`);
  }
  return input.value;
}

/**
 * Asserts the header value fields of rows 1 and 2 against their rows of `modern/audit/screens.md`
 * (COTRTUP-002, -003, -005, -007, -008 and -010). Each entry of `fields` is the BMS name, the `POS`
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
    expect(cellOf(element)).toBe(`row-start-${row} col-start-${col} col-span-${length}`);
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

/** The `GridCell` placement of the cell that holds `element`. */
function cellOf(element: HTMLElement): string | undefined {
  return element.parentElement?.className;
}

/** The columns of map row `row` that a `GridCell` of the screen body covers. */
function occupiedColumns(row: number): Set<number> {
  const columns = new Set<number>();
  for (const cell of screenForm().querySelectorAll(`.row-start-${row}`)) {
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

/** COTRTUPC-037: the map erased and the abend data written from the top left, 80 per line. */
function expectAbend() {
  const main = screen.getByRole("main", { name: "CTTU ended abnormally" });
  expect(Array.from(main.querySelectorAll("p"), (line) => line.textContent)).toEqual([
    ABEND_DATA.slice(0, 80),
    ABEND_DATA.slice(80),
  ]);
  expect(screen.queryByRole("group", { name: "Function keys" })).toBeNull();
  expect(screen.queryByRole("textbox")).toBeNull();
}

/** The fresh screen, once the cursor is on the code. */
async function open() {
  const result = renderScreen();
  await expectFocus(LABELS.code);
  return result;
}

/** The fresh screen, then ENTER with code 01, which endpoint 26 finds: state S. */
async function showFound() {
  const result = await open();
  fireEvent.change(field(LABELS.code), { target: { value: CODE } });
  press("ENTER=Process");
  await waitFor(() => {
    expect(field(LABELS.description).value).toBe(STORED);
  });
  return result;
}

/** The fresh screen, then ENTER with code 09, which no row holds: state X. */
async function showNotFound() {
  const result = await open();
  fireEvent.change(field(LABELS.code), { target: { value: NEW_CODE } });
  press("ENTER=Process");
  await waitFor(() => {
    expect(infoText()).toBe(INFO.notFound);
  });
  return result;
}

/** State X, then F5: state R, without a request. */
async function startAdd() {
  const result = await showNotFound();
  press("F5=Save");
  await waitFor(() => {
    expect(infoText()).toBe(INFO.newRecord);
  });
  return result;
}

/** State S, the description edited, then ENTER, which endpoint 27 validates: state N. */
async function validateEdit() {
  const result = await showFound();
  fireEvent.change(field(LABELS.description), { target: { value: EDITED } });
  press("ENTER=Process");
  await waitFor(() => {
    expect(infoText()).toBe(INFO.validated);
  });
  return result;
}

/** State R, a description typed, then ENTER, which endpoint 27 validates: state N. */
async function validateNew() {
  const result = await startAdd();
  fireEvent.change(field(LABELS.description), { target: { value: NEW_DESCRIPTION } });
  press("ENTER=Process");
  await waitFor(() => {
    expect(infoText()).toBe(INFO.validated);
  });
  return result;
}

/** State N, then F5, which endpoint 27 saves: state C. */
async function saveEdit() {
  const result = await validateEdit();
  press("F5=Save");
  await waitFor(() => {
    expect(infoText()).toBe(INFO.committed);
  });
  return result;
}

/** State N, then F12: state B over the fetched values, without a request. */
async function cancelEdit() {
  const result = await validateEdit();
  press("F12=Cancel");
  await waitFor(() => {
    expect(errorText()).toBe(MSG.updateCancelled);
  });
  return result;
}

/** State S, then F4: state 9, without a request. */
async function confirmDelete() {
  const result = await showFound();
  press("F4=Delete");
  await waitFor(() => {
    expect(infoText()).toBe(INFO.confirmDelete);
  });
  return result;
}

/** State 9, then F4, which endpoint 27 deletes: state 7. */
async function deleteShown() {
  const result = await confirmDelete();
  press("F4=Delete");
  await waitFor(() => {
    expect(infoText()).toBe(INFO.deleted);
  });
  return result;
}

/** State 9, then F4, which endpoint 27 refuses for the type's categories: state 8. */
async function deleteRefused() {
  const result = await confirmDelete();
  press("F4=Delete");
  await waitFor(() => {
    expect(errorText()).toBe(deleteRestricted.message);
  });
  return result;
}

beforeEach(() => {
  setSession("test-token", "ADMIN001", "A");
});

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COTRTUP rendering", () => {
  // audit: COTRTUP-011
  // audit: COTRTUP-012
  // audit: COTRTUP-014
  // audit: COTRTUP-015
  // audit: COTRTUP-017
  // audit: COTRTUP-018
  // audit: COTRTUP-019
  // audit: COTRTUP-020
  // audit: COTRTUP-024
  // audit: COTRTUP-026
  // audit: COTRTUP-027
  // audit: COTRTUP-028
  // audit: COTRTUP-029
  // audit: COTRTUP-030
  // audit: COTRTUP-031
  // audit: COTRTUPC-003
  // audit: COTRTUPC-048
  it("the fresh screen renders the map's title, header, labels, inputs, message rows and keys and sends nothing", async () => {
    const calls = stubApi({});
    await open();
    await settle();
    expect(calls).toEqual([]);

    expect(
      screen
        .getByRole("heading", { level: 1, name: "Maintain Transaction Type" })
        .getAttribute("data-slot"),
    ).toBe("card-title");
    expect(headerValue("TRNNAME")).toBe("CTTU");
    expect(headerValue("PGMNAME")).toBe("COTRTUPC");
    expect(headerValue("TITLE01")).toBe(TITLE01);
    expect(headerValue("TITLE02")).toBe(TITLE02);
    expect(TITLE01).toHaveLength(40);
    expect(TITLE02).toHaveLength(40);
    expect(headerValue("CURDATE")).toMatch(/^\d{2}\/\d{2}\/\d{2}$/);
    expect(headerValue("CURTIME")).toMatch(/^\d{2}:\d{2}:\d{2}$/);

    // Each label is the map literal at its POS and LENGTH, bound to its input.
    for (const label of Object.values(LABELS)) {
      const element = screen.getByText(label, { ...exactText, selector: "label" });
      expect(element.getAttribute("for")).toBe(field(label).id);
    }
    expect(cellOf(screen.getByText(LABELS.code, { ...exactText, selector: "label" }))).toBe(
      "row-start-12 col-start-4 col-span-19",
    );
    expect(
      cellOf(screen.getByText(LABELS.description, { ...exactText, selector: "label" })),
    ).toBe("row-start-14 col-start-4 col-span-19");
    // The body's text boxes; the header's read-only values are counted by none.
    expect(within(screenForm()).getAllByRole("textbox")).toHaveLength(2);
    expect(field(LABELS.code).maxLength).toBe(2);
    expect(field(LABELS.description).maxLength).toBe(50);
    expect(cellOf(field(LABELS.code))).toBe("row-start-12 col-start-26 col-span-2");
    expect(cellOf(field(LABELS.description))).toBe("row-start-14 col-start-26 col-span-50");
    // The zero-length stoppers after TRTYPCD (12,29) and TRTYDSC (14,77) render no element.
    expect(occupiedColumns(12).has(29)).toBe(false);
    expect(occupiedColumns(14).has(77)).toBe(false);
    expect(shownValues()).toEqual({ code: "", description: "" });
    expectEditable({ code: true, description: false });

    // Rows 22 to 24: the info row, the error row and the keys, and no element for the stopper
    // after INFOMSG (22,69).
    const info = messageRow("info");
    const error = messageRow("error");
    expect(Array.from(info.parentElement?.children ?? [])).toEqual([
      info,
      error,
      functionKeys(),
    ]);
    expect(infoText()).toBe(INFO.search);
    expect(info.classList.contains("text-card-foreground")).toBe(true);
    expect(errorText()).toBe("");
    expect(error.classList.contains("text-destructive")).toBe(true);
    expect(error.getAttribute("aria-live")).toBe("polite");

    // Row 24 in map order, F6=Add without a button; only ENTER=Process and F3=Exit shown.
    expect(allKeys()).toEqual(ALL_KEYS);
    expect(within(functionKeys()).queryByText("F6=Add")).toBeNull();
    expect(shownKeys()).toEqual(["ENTER=Process", "F3=Exit"]);
    const enter = key("ENTER=Process");
    expect(enter.getAttribute("type")).toBe("submit");
    expect(enter.getAttribute("data-variant")).toBe("default");
    expect(enter.getAttribute("form")).toBe(screenForm().id);
    for (const label of ["F3=Exit", "F4=Delete", "F5=Save", "F12=Cancel"]) {
      expect(key(label).getAttribute("type")).toBe("button");
      expect(key(label).getAttribute("data-variant")).toBe("outline");
      expect(key(label).getAttribute("data-size")).toBe("sm");
    }
  });

  // audit: COTRTUP-002
  // audit: COTRTUP-003
  // audit: COTRTUP-005
  // audit: COTRTUP-007
  // audit: COTRTUP-008
  // audit: COTRTUP-010
  it("shows the six header values as read-only Inputs in the CardHeader title rows, each at its POS and LENGTH", async () => {
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
      const calls = stubApi({});
      await open();
      await settle();
      expect(calls).toEqual([]);

      expectHeaderValues(document, [
        { name: "TRNNAME", row: 1, col: 7, length: 4, value: "CTTU", title: false },
        { name: "TITLE01", row: 1, col: 21, length: 40, value: TITLE01, title: true },
        { name: "CURDATE", row: 1, col: 71, length: 8, value: "01/02/24", title: false },
        { name: "PGMNAME", row: 2, col: 7, length: 8, value: "COTRTUPC", title: false },
        { name: "TITLE02", row: 2, col: 21, length: 40, value: TITLE02, title: true },
        { name: "CURTIME", row: 2, col: 71, length: 8, value: "03:04:05", title: false },
      ]);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("COTRTUP states", () => {
  // audit: COTRTUP-021
  // audit: COTRTUP-022
  // audit: COTRTUP-023
  // audit: COTRTUP-024
  // audit: COTRTUP-025
  // audit: COTRTUPC-003
  // audit: COTRTUPC-013
  // audit: COTRTUPC-014
  // audit: COTRTUPC-015
  // audit: COTRTUPC-031
  // audit: COTRTUPC-045
  // audit: COTRTUPC-046
  // audit: COTRTUPC-047
  it.each([
    {
      state: "fresh",
      replies: {},
      reach: open,
      requests: [],
      values: { code: "", description: "" },
      editable: { code: true, description: false },
      red: { code: false, description: false },
      cursor: LABELS.code,
      info: INFO.search,
      error: "",
      keys: ["ENTER=Process", "F3=Exit"],
    },
    {
      state: "S",
      replies: { [FETCH]: [reply(200, found)] },
      reach: showFound,
      requests: [FETCH],
      values: { code: CODE, description: STORED },
      editable: { code: false, description: true },
      red: { code: false, description: false },
      cursor: LABELS.description,
      info: INFO.search,
      error: "",
      keys: ["ENTER=Process", "F3=Exit", "F4=Delete", "F12=Cancel"],
    },
    {
      state: "X",
      replies: { [FETCH_NEW]: [reply(200, notFound)] },
      reach: showNotFound,
      requests: [FETCH_NEW],
      values: { code: NEW_CODE, description: "" },
      editable: { code: true, description: false },
      red: { code: true, description: false },
      cursor: LABELS.code,
      info: INFO.notFound,
      error: MSG.notFound,
      keys: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
    },
    {
      state: "R",
      replies: { [FETCH_NEW]: [reply(200, notFound)] },
      reach: startAdd,
      requests: [FETCH_NEW],
      values: { code: NEW_CODE, description: "" },
      editable: { code: false, description: true },
      red: { code: false, description: false },
      cursor: LABELS.description,
      info: INFO.newRecord,
      error: "",
      keys: ["ENTER=Process", "F3=Exit", "F12=Cancel"],
    },
    {
      state: "N",
      replies: { [FETCH]: [reply(200, found)], [ACTIONS]: [reply(200, validated)] },
      reach: validateEdit,
      requests: [FETCH, ACTIONS],
      values: { code: CODE, description: padded(EDITED) },
      editable: { code: false, description: false },
      red: { code: false, description: false },
      cursor: LABELS.description,
      info: INFO.validated,
      error: "",
      keys: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
    },
    {
      state: "C",
      replies: {
        [FETCH]: [reply(200, found)],
        [ACTIONS]: [reply(200, validated), reply(200, saved)],
      },
      reach: saveEdit,
      requests: [FETCH, ACTIONS, ACTIONS],
      values: { code: CODE, description: padded(EDITED) },
      editable: { code: false, description: false },
      red: { code: false, description: false },
      cursor: LABELS.code,
      info: INFO.committed,
      error: "",
      keys: ["ENTER=Process", "F3=Exit"],
    },
    {
      state: "B",
      replies: { [FETCH]: [reply(200, found)], [ACTIONS]: [reply(200, validated)] },
      reach: cancelEdit,
      requests: [FETCH, ACTIONS],
      values: { code: CODE, description: STORED },
      editable: { code: false, description: true },
      red: { code: false, description: false },
      cursor: LABELS.description,
      info: INFO.changes,
      error: MSG.updateCancelled,
      keys: ["ENTER=Process", "F3=Exit"],
    },
    {
      state: "9",
      replies: { [FETCH]: [reply(200, found)] },
      reach: confirmDelete,
      requests: [FETCH],
      values: { code: CODE, description: STORED },
      editable: { code: false, description: false },
      red: { code: false, description: false },
      cursor: LABELS.code,
      info: INFO.confirmDelete,
      error: "",
      keys: ["F4=Delete", "F12=Cancel"],
    },
    {
      state: "7",
      replies: { [FETCH]: [reply(200, found)], [ACTIONS]: [reply(200, deleted)] },
      reach: deleteShown,
      requests: [FETCH, ACTIONS],
      values: { code: CODE, description: STORED },
      editable: { code: false, description: false },
      red: { code: false, description: false },
      cursor: LABELS.code,
      info: INFO.deleted,
      error: "",
      keys: ["ENTER=Process", "F3=Exit"],
    },
  ])(
    "state $state: the values, protection, highlight, cursor, message rows and row-24 keys",
    async ({ replies, reach, requests, values, editable, red, cursor, info, error, keys }) => {
      const calls = stubApi(replies);
      await reach();
      await settle();

      expect(calls.map((call) => call.route)).toEqual(requests);
      expect(shownValues()).toEqual(values);
      expectEditable(editable);
      // A protected input is out of the Tab order; the program's cursor still reaches it.
      expectTabStops(editable);
      expect(redFields()).toEqual(red);
      // A red input is also marked invalid, described by the error row's text.
      expect(marks()).toEqual(marksFor(red, error));
      await expectFocus(cursor);
      expect(infoText()).toBe(info);
      // Every state has an info text, which the program shows bright (COTRTUPC-045).
      const infoDescription = messageRow("info").querySelector('[data-slot="alert-description"]');
      expect(infoDescription?.classList.contains("font-semibold")).toBe(true);
      expect(errorText()).toBe(error);
      expect(shownKeys()).toEqual(keys);
      expect(allKeys()).toEqual(ALL_KEYS);
    },
  );
});

describe("COTRTUP fetch, endpoint 26", () => {
  // audit: COTRTUP-032
  it("ENTER sends the typed code as the path of endpoint 26, with the bearer token", async () => {
    const calls = stubApi({ [FETCH]: [reply(200, found)] });
    await showFound();

    expect(calls).toEqual([{ route: FETCH, body: undefined, authorization: BEARER }]);
    expect(errorText()).toBe("");
  });

  // audit: COTRTUP-032
  it("a submit of the form, as an Enter typed in the code field sends it, is the screen's ENTER", async () => {
    const calls = stubApi({ [FETCH]: [reply(200, found)] });
    await open();
    fireEvent.change(field(LABELS.code), { target: { value: CODE } });
    fireEvent.submit(screenForm());
    await waitFor(() => {
      expect(field(LABELS.description).value).toBe(STORED);
    });

    expect(calls.map((call) => call.route)).toEqual([FETCH]);
  });

  // audit: COTRTUPC-013
  // audit: COTRTUPC-039
  it("a stored description padded to 50 is shown without its trailing spaces, so it takes typing in state S and again in state B", async () => {
    const appended = `${STORED} X`;
    const paddedFound: components["schemas"]["TranTypeView"] = {
      ...found,
      description: padded(STORED),
    };
    const appendedValidated: components["schemas"]["TranTypeActionResult"] = {
      ...validated,
      description: padded(appended),
    };
    const calls = stubApi({
      [FETCH]: [reply(200, paddedFound)],
      [ACTIONS]: [reply(200, appendedValidated)],
    });
    await showFound();
    // A full `maxLength` input takes no keystroke: the shown text leaves the pad's room to type.
    expect(field(LABELS.description).value).toBe(STORED);
    expect(field(LABELS.description).maxLength).toBe(50);
    expectEditable({ code: false, description: true });

    fireEvent.change(field(LABELS.description), { target: { value: appended } });
    press("ENTER=Process");
    await waitFor(() => {
      expect(infoText()).toBe(INFO.validated);
    });
    expect(calls[1]).toEqual({
      route: ACTIONS,
      body: { aid: "ENTER", description: appended },
      authorization: BEARER,
    });

    // F12 backs the change out: state B shows the fetched description, still without the pad.
    press("F12=Cancel");
    await waitFor(() => {
      expect(errorText()).toBe(MSG.updateCancelled);
    });
    expect(shownValues()).toEqual({ code: CODE, description: STORED });
    expectEditable({ code: false, description: true });
  });

  // audit: COTRTUP-035
  // audit: COTRTUPC-014
  // audit: COTRTUPC-015
  it("a code no row holds answers 200 without a description; F5 then starts the add under that code, sending nothing", async () => {
    const calls = stubApi({ [FETCH_NEW]: [reply(200, notFound)] });
    await showNotFound();
    expect(errorText()).toBe(MSG.notFound);
    expect(shownValues()).toEqual({ code: NEW_CODE, description: "" });
    expect(key("F5=Save").hidden).toBe(false);

    // Nothing typed is received at F5: a code typed over the shown one does not become the key.
    fireEvent.change(field(LABELS.code), { target: { value: "05" } });
    press("F5=Save");
    expect(infoText()).toBe(INFO.newRecord);
    expect(errorText()).toBe("");
    expect(shownValues()).toEqual({ code: NEW_CODE, description: "" });
    await expectFocus(LABELS.description);
    await settle();
    expect(calls.map((call) => call.route)).toEqual([FETCH_NEW]);
  });

  // audit: COTRTUPC-046
  // audit: COTRTUPC-047
  it.each([
    { state: "fresh", replies: {}, reach: open, requests: [] },
    {
      state: "X",
      replies: { [FETCH_NEW]: [reply(200, notFound)] },
      reach: showNotFound,
      requests: [FETCH_NEW],
    },
  ])(
    "a code blank at ENTER in state $state is refused before any request: the fresh screen with * in red",
    async ({ replies, reach, requests }) => {
      const calls = stubApi(replies);
      await reach();
      fireEvent.change(field(LABELS.code), { target: { value: "" } });
      press("ENTER=Process");
      await waitFor(() => {
        expect(errorText()).toBe(MSG.codeNotSupplied);
      });

      expect(shownValues()).toEqual({ code: "*", description: "" });
      expect(redFields()).toEqual({ code: true, description: false });
      // The `*` flag is a screen mark, not a form error: it still links the error row's text.
      expect(marks()).toEqual({ code: MSG.codeNotSupplied, description: null });
      expectEditable({ code: true, description: false });
      await expectFocus(LABELS.code);
      expect(infoText()).toBe(INFO.search);
      await settle();
      expect(calls.map((call) => call.route)).toEqual(requests);
    },
  );

  // audit: COTRTUPC-014
  it("the code stays editable in state X: ENTER fetches the code then typed through endpoint 26", async () => {
    const calls = stubApi({ [FETCH_NEW]: [reply(200, notFound)], [FETCH]: [reply(200, found)] });
    await showNotFound();
    fireEvent.change(field(LABELS.code), { target: { value: CODE } });
    press("ENTER=Process");
    await waitFor(() => {
      expect(field(LABELS.description).value).toBe(STORED);
    });

    expect(calls.map((call) => call.route)).toEqual([FETCH_NEW, FETCH]);
    expect(shownValues()).toEqual({ code: CODE, description: STORED });
    expect(redFields()).toEqual({ code: false, description: false });
    expect(marks()).toEqual({ code: null, description: null });
    expect(errorText()).toBe("");
    expect(infoText()).toBe(INFO.search);
  });

  // audit: COTRTUP-020
  // audit: COTRTUPC-014
  // audit: COTRTUPC-045
  // audit: COTRTUPC-046
  // audit: COTRTUPC-047
  it.each([
    {
      title: "a code the numeric edit refuses (400)",
      typed: "AB",
      route: "GET /api/v1/transaction-types/AB",
      status: 400,
      error: codeNotNumeric,
      values: { code: "", description: "" },
      info: INFO.search,
      keys: ["ENTER=Process", "F3=Exit"],
    },
    {
      title: "a code the zero edit refuses (400)",
      typed: "00",
      route: "GET /api/v1/transaction-types/00",
      status: 400,
      error: codeZero,
      values: { code: "", description: "" },
      info: INFO.search,
      keys: ["ENTER=Process", "F3=Exit"],
    },
    {
      title: "a read that fails (500)",
      typed: CODE,
      route: FETCH,
      status: 500,
      error: fetchFailed,
      values: { code: CODE, description: "" },
      info: INFO.notFound,
      keys: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
    },
  ])(
    "ENTER answered by $title: the message, the code red with the cursor, and the state's info row",
    async ({ typed, route, status, error, values, info, keys }) => {
      const calls = stubApi({ [route]: [reply(status, error)] });
      await open();
      fireEvent.change(field(LABELS.code), { target: { value: typed } });
      press("ENTER=Process");
      await waitFor(() => {
        expect(errorText()).toBe(error.message);
      });

      expect(calls.map((call) => call.route)).toEqual([route]);
      expect(shownValues()).toEqual(values);
      expect(redFields()).toEqual({ code: true, description: false });
      expect(marks()).toEqual({ code: error.message, description: null });
      expectEditable({ code: true, description: false });
      await expectFocus(LABELS.code);
      expect(infoText()).toBe(info);
      expect(shownKeys()).toEqual(keys);
    },
  );

  it.each([
    { title: "no valid token (401)", status: 401, error: signonRequired },
    { title: "a token whose user type is not A (403)", status: 403, error: adminRequired },
  ])(
    "a refusal no COTRTUPC branch names, $title, leaves the screen as it was with the message",
    async ({ status, error }) => {
      const calls = stubApi({ [FETCH]: [reply(status, error)] });
      await open();
      fireEvent.change(field(LABELS.code), { target: { value: CODE } });
      press("ENTER=Process");
      await waitFor(() => {
        expect(errorText()).toBe(error.message);
      });

      expect(calls.map((call) => call.route)).toEqual([FETCH]);
      expect(shownValues()).toEqual({ code: CODE, description: "" });
      expectEditable({ code: true, description: false });
      expect(infoText()).toBe(INFO.search);
      expect(shownKeys()).toEqual(["ENTER=Process", "F3=Exit"]);
    },
  );

});

describe("COTRTUP while a request is out", () => {
  /** After a completed save or delete, the next key starts over; then ENTER fetches `CODE`. */
  function startOverAndFetch(): void {
    press("ENTER=Process");
    fireEvent.change(field(LABELS.code), { target: { value: CODE } });
    press("ENTER=Process");
  }

  /** Every row-24 button, hidden ones included, and every key on the keyboard that has a role. */
  function pressEveryKey(): void {
    for (const label of ALL_KEYS) {
      fireEvent.click(key(label));
    }
    for (const name of ["Enter", "F2", "F3", "F4", "F5", "F12"]) {
      pressKey(name);
    }
  }

  it.each([
    {
      title: "ENTER fetching a code (endpoint 26)",
      replies: (held: Promise<Response>) => ({
        [FETCH]: [held],
        [ACTIONS]: [reply(200, validated)],
      }),
      arrange: async () => {
        await open();
        fireEvent.change(field(LABELS.code), { target: { value: CODE } });
      },
      start: () => {
        press("ENTER=Process");
      },
      route: FETCH,
      answer: reply(200, found),
      info: INFO.search,
      before: { code: true, description: false },
      after: { code: false, description: true },
      later: () => {
        press("ENTER=Process");
      },
      laterRoute: ACTIONS,
    },
    {
      title: "Enter fetching again after a code no row holds (endpoint 26)",
      replies: (held: Promise<Response>) => ({
        [FETCH_NEW]: [reply(200, notFound)],
        [FETCH]: [held],
        [ACTIONS]: [reply(200, validated)],
      }),
      arrange: async () => {
        await showNotFound();
        fireEvent.change(field(LABELS.code), { target: { value: CODE } });
      },
      start: () => {
        pressKey("Enter");
      },
      route: FETCH,
      answer: reply(200, found),
      info: INFO.search,
      before: { code: true, description: false },
      after: { code: false, description: true },
      later: () => {
        press("ENTER=Process");
      },
      laterRoute: ACTIONS,
    },
    {
      title: "ENTER validating an edit (endpoint 27)",
      replies: (held: Promise<Response>) => ({
        [FETCH]: [reply(200, found)],
        [ACTIONS]: [held, reply(200, saved)],
      }),
      arrange: async () => {
        await showFound();
        fireEvent.change(field(LABELS.description), { target: { value: EDITED } });
      },
      start: () => {
        press("ENTER=Process");
      },
      route: ACTIONS,
      answer: reply(200, validated),
      info: INFO.validated,
      before: { code: false, description: true },
      after: { code: false, description: false },
      later: () => {
        press("F5=Save");
      },
      laterRoute: ACTIONS,
    },
    {
      title: "F5 saving validated changes (endpoint 27)",
      replies: (held: Promise<Response>) => ({
        [FETCH]: [reply(200, found), reply(200, found)],
        [ACTIONS]: [reply(200, validated), held],
      }),
      arrange: validateEdit,
      start: () => {
        pressKey("F5");
      },
      route: ACTIONS,
      answer: reply(200, saved),
      info: INFO.committed,
      before: { code: false, description: false },
      after: { code: false, description: false },
      later: startOverAndFetch,
      laterRoute: FETCH,
    },
    {
      title: "the second F4 deleting the type (endpoint 27)",
      replies: (held: Promise<Response>) => ({
        [FETCH]: [reply(200, found), reply(200, found)],
        [ACTIONS]: [held],
      }),
      arrange: confirmDelete,
      start: () => {
        press("F4=Delete");
      },
      route: ACTIONS,
      answer: reply(200, deleted),
      info: INFO.deleted,
      before: { code: false, description: false },
      after: { code: false, description: false },
      later: startOverAndFetch,
      laterRoute: FETCH,
    },
  ])(
    "while $title is out, every key is disabled and refused, the key row and form are busy and both inputs read-only",
    async ({ replies, arrange, start, route, answer, info, before, after, later, laterRoute }) => {
      const held = heldReply();
      const calls = stubApi(replies(held.reply));
      await arrange();
      const sentBefore = calls.length;
      const shownBefore = shownKeys();
      expectEditable(before);

      // The key twice and every other key in one batch, before the screen re-renders: only the
      // lock the first key took refuses the rest.
      act(() => {
        start();
        start();
        pressEveryKey();
      });
      await waitFor(() => {
        expect(calls).toHaveLength(sentBefore + 1);
      });
      expect(calls[sentBefore]?.route).toBe(route);

      expect(functionKeys().getAttribute("aria-busy")).toBe("true");
      expect(screenForm().getAttribute("aria-busy")).toBe("true");
      expect(ALL_KEYS.map((label) => key(label).disabled)).toEqual(ALL_KEYS.map(() => true));
      expect(shownKeys()).toEqual(shownBefore);
      expectEditable({ code: false, description: false });
      // The lock makes no input protected: an input the state leaves enterable stays a Tab stop.
      expectTabStops(before);

      pressEveryKey();
      await settle();
      expect(calls).toHaveLength(sentBefore + 1);
      expect(screen.queryByText(/^Stub at/)).toBeNull();

      held.release(json(answer.status, answer.body));
      await waitFor(() => {
        expect(screenForm().getAttribute("aria-busy")).toBe("false");
      });
      expect(infoText()).toBe(info);
      expect(functionKeys().getAttribute("aria-busy")).toBe("false");
      expect(ALL_KEYS.map((label) => key(label).disabled)).toEqual(ALL_KEYS.map(() => false));
      expectEditable(after);
      expectTabStops(after);

      // The keyboard is open again: the next request is sent.
      later();
      await waitFor(() => {
        expect(calls).toHaveLength(sentBefore + 2);
      });
      expect(calls[sentBefore + 1]?.route).toBe(laterRoute);
    },
  );
});

describe("COTRTUP save, endpoint 27", () => {
  // audit: COTRTUP-035
  // audit: COTRTUPC-022
  // audit: COTRTUPC-030
  it("ENTER validates the edit, ENTER again sends nothing, F5 saves what was validated, and the next key starts over", async () => {
    const calls = stubApi({
      [FETCH]: [reply(200, found)],
      [ACTIONS]: [reply(200, validated), reply(200, saved)],
    });
    // F5 saves only changes ENTER has validated (state N); F5 straight after an edit in state S
    // is an invalid key (COTRTUPC-042, asserted below), so the edit goes through ENTER first.
    await validateEdit();
    expect(calls[1]).toEqual({
      route: ACTIONS,
      body: { aid: "ENTER", description: EDITED },
      authorization: BEARER,
    });
    expect(shownValues()).toEqual({ code: CODE, description: padded(EDITED) });
    expectEditable({ code: false, description: false });

    // State N: no edit runs, so ENTER shows the same screen again without a request.
    press("ENTER=Process");
    await settle();
    expect(calls).toHaveLength(2);
    expect(infoText()).toBe(INFO.validated);
    expect(shownKeys()).toEqual(["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"]);

    // F5 sends the key and the validated description; the screen's state is not sent.
    press("F5=Save");
    await waitFor(() => {
      expect(infoText()).toBe(INFO.committed);
    });
    expect(calls[2]).toEqual({
      route: ACTIONS,
      body: { aid: "PF5", description: padded(EDITED) },
      authorization: BEARER,
    });
    expect(errorText()).toBe("");
    expect(shownValues()).toEqual({ code: CODE, description: padded(EDITED) });
    expect(shownKeys()).toEqual(["ENTER=Process", "F3=Exit"]);

    // State C holds until the next key, which shows the fresh screen without a request.
    press("ENTER=Process");
    expect(shownValues()).toEqual({ code: "", description: "" });
    expect(infoText()).toBe(INFO.search);
    expectEditable({ code: true, description: false });
    await settle();
    expect(calls).toHaveLength(3);
  });

  // audit: COTRTUPC-015
  // audit: COTRTUPC-022
  it("the add path validates and saves the new type under the code of the not-found result", async () => {
    const calls = stubApi({
      [FETCH_NEW]: [reply(200, notFound)],
      [ACTIONS_NEW]: [reply(200, newValidated), reply(200, newSaved)],
    });
    await validateNew();
    press("F5=Save");
    await waitFor(() => {
      expect(infoText()).toBe(INFO.committed);
    });

    expect(calls).toEqual([
      { route: FETCH_NEW, body: undefined, authorization: BEARER },
      {
        route: ACTIONS_NEW,
        body: { aid: "ENTER", description: NEW_DESCRIPTION },
        authorization: BEARER,
      },
      {
        route: ACTIONS_NEW,
        body: { aid: "PF5", description: padded(NEW_DESCRIPTION) },
        authorization: BEARER,
      },
    ]);
    expect(shownValues()).toEqual({ code: NEW_CODE, description: padded(NEW_DESCRIPTION) });
  });

  // audit: COTRTUPC-045
  // audit: COTRTUPC-046
  // audit: COTRTUPC-047
  it.each([
    {
      title: "a blank description",
      typed: "",
      error: descriptionNotSupplied,
      values: { code: CODE, description: "*" },
      red: { code: false, description: true },
      info: INFO.changes,
    },
    {
      title: "a description with other characters",
      typed: "PURCHASE-1",
      error: descriptionNotAlphanumeric,
      values: { code: CODE, description: "PURCHASE-1" },
      red: { code: false, description: true },
      info: INFO.changes,
    },
    {
      title: "no change from the fetched values",
      typed: "purchase",
      error: noChange,
      values: { code: CODE, description: STORED },
      red: { code: false, description: false },
      info: INFO.search,
    },
  ])(
    "ENTER with $title: the 400 naming the description, the state's screen with the cursor on it",
    async ({ typed, error, values, red, info }) => {
      const calls = stubApi({ [FETCH]: [reply(200, found)], [ACTIONS]: [reply(400, error)] });
      await showFound();
      fireEvent.change(field(LABELS.description), { target: { value: typed } });
      press("ENTER=Process");
      await waitFor(() => {
        expect(errorText()).toBe(error.message);
      });

      expect(calls[1]).toEqual({
        route: ACTIONS,
        body: { aid: "ENTER", description: typed },
        authorization: BEARER,
      });
      expect(shownValues()).toEqual(values);
      expect(redFields()).toEqual(red);
      expect(marks()).toEqual(marksFor(red, error.message));
      expectEditable({ code: false, description: true });
      await expectFocus(LABELS.description);
      expect(infoText()).toBe(info);
    },
  );

  // audit: COTRTUPC-030
  // audit: COTRTUPC-045
  it.each([
    {
      title: "the row cannot be locked",
      error: lockFailed,
      info: INFO.unsuccessful,
      editable: { code: true, description: false },
      cursor: LABELS.description,
    },
    {
      title: "the update fails",
      error: updateFailed,
      info: INFO.committed,
      editable: { code: false, description: false },
      cursor: LABELS.code,
    },
    {
      title: "the insert after the update fails",
      error: insertFailed,
      info: INFO.committed,
      editable: { code: false, description: false },
      cursor: LABELS.code,
    },
  ])(
    "F5 when $title: the 500's message, the state's info row, and the next key starts over",
    async ({ error, info, editable, cursor }) => {
      const calls = stubApi({
        [FETCH]: [reply(200, found)],
        [ACTIONS]: [reply(200, validated), reply(500, error)],
      });
      await validateEdit();
      press("F5=Save");
      await waitFor(() => {
        expect(errorText()).toBe(error.message);
      });

      expect(calls.map((call) => call.route)).toEqual([FETCH, ACTIONS, ACTIONS]);
      expect(infoText()).toBe(info);
      expect(shownValues()).toEqual({ code: CODE, description: padded(EDITED) });
      expectEditable(editable);
      await expectFocus(cursor);
      expect(shownKeys()).toEqual(["ENTER=Process", "F3=Exit"]);

      pressKey("F12");
      expect(shownValues()).toEqual({ code: "", description: "" });
      expect(errorText()).toBe("");
      expect(infoText()).toBe(INFO.search);
      await settle();
      expect(calls).toHaveLength(3);
    },
  );
});

describe("COTRTUP delete", () => {
  // audit: COTRTUP-034
  // audit: COTRTUPC-030
  // audit: COTRTUPC-031
  it("F4 asks to confirm without a request, the second F4 deletes the fetched code, and the next key clears the screen", async () => {
    const calls = stubApi({ [FETCH]: [reply(200, found)], [ACTIONS]: [reply(200, deleted)] });
    await showFound();
    expect(key("F4=Delete").hidden).toBe(false);

    // Nothing typed is received at F4: the edited description does not reach the prompt.
    fireEvent.change(field(LABELS.description), { target: { value: EDITED } });
    press("F4=Delete");
    expect(infoText()).toBe(INFO.confirmDelete);
    expect(shownValues()).toEqual({ code: CODE, description: STORED });
    await settle();
    expect(calls).toHaveLength(1);

    // The second F4 sends the key alone: the delete takes the fetched code, and no state is sent.
    press("F4=Delete");
    await waitFor(() => {
      expect(infoText()).toBe(INFO.deleted);
    });
    expect(calls[1]).toEqual({ route: ACTIONS, body: { aid: "PF4" }, authorization: BEARER });
    expect(errorText()).toBe("");

    // `deleted: true` does not clear the fields at once: state 7 shows the deleted, fetched
    // values (COTRTUPC-030, COTRTUPC-032) until the next key, which shows the fresh screen.
    expect(shownValues()).toEqual({ code: CODE, description: STORED });
    press("ENTER=Process");
    expect(shownValues()).toEqual({ code: "", description: "" });
    expect(infoText()).toBe(INFO.search);
    expectEditable({ code: true, description: false });
    await settle();
    expect(calls).toHaveLength(2);
  });

  // audit: COTRTUP-032
  // audit: COTRTUPC-042
  it("ENTER is refused while a delete awaits confirmation", async () => {
    const calls = stubApi({ [FETCH]: [reply(200, found)] });
    await confirmDelete();
    expect(key("ENTER=Process").hidden).toBe(true);

    pressKey("Enter");
    expect(errorText()).toBe(MSG.invalidKey);
    expect(infoText()).toBe(INFO.confirmDelete);
    expect(shownValues()).toEqual({ code: CODE, description: STORED });
    expect(shownKeys()).toEqual(["F4=Delete", "F12=Cancel"]);
    await settle();
    expect(calls).toHaveLength(1);
  });

  // audit: COTRTUPC-036
  // audit: COTRTUPC-037
  it("F5 while a delete awaits confirmation ends the conversation with the abend data; the next visit is fresh", async () => {
    const calls = stubApi({ [FETCH]: [reply(200, found)] });
    const { unmount } = await confirmDelete();

    pressKey("F5");
    expectAbend();
    await settle();
    expect(calls).toHaveLength(1);

    unmount();
    await open();
    expect(shownValues()).toEqual({ code: "", description: "" });
    expect(infoText()).toBe(INFO.search);
  });

  // audit: COTRTUPC-035
  // audit: COTRTUPC-037
  it.each([{ name: "Enter" }, { name: "F5" }])(
    "after a delete refused for its categories (state 8), $name ends in the abend without a request",
    async ({ name }) => {
      const calls = stubApi({
        [FETCH]: [reply(200, found)],
        [ACTIONS]: [reply(500, deleteRestricted)],
      });
      await deleteRefused();
      expect(infoText()).toBe(INFO.search);
      expect(shownValues()).toEqual({ code: CODE, description: STORED });
      expectEditable({ code: false, description: false });
      await expectFocus(LABELS.code);
      expect(shownKeys()).toEqual(["ENTER=Process", "F3=Exit"]);

      pressKey(name);
      expectAbend();
      await settle();
      expect(calls.map((call) => call.route)).toEqual([FETCH, ACTIONS]);
    },
  );

  // audit: COTRTUPC-035
  // audit: COTRTUPC-042
  it.each([{ name: "F4" }, { name: "F12" }, { name: "F7" }])(
    "after a delete refused for its categories (state 8), $name is an invalid key",
    async ({ name }) => {
      const calls = stubApi({
        [FETCH]: [reply(200, found)],
        [ACTIONS]: [reply(500, deleteRestricted)],
      });
      await deleteRefused();

      pressKey(name);
      expect(errorText()).toBe(MSG.invalidKey);
      expect(infoText()).toBe(INFO.search);
      expect(shownValues()).toEqual({ code: CODE, description: STORED });
      expect(shownKeys()).toEqual(["ENTER=Process", "F3=Exit"]);
      await settle();
      expect(calls).toHaveLength(2);
    },
  );

  // audit: COTRTUPC-030
  // audit: COTRTUPC-047
  it("a delete that fails leaves state 6: the fetched values, the code red, and the next key starts over", async () => {
    const calls = stubApi({ [FETCH]: [reply(200, found)], [ACTIONS]: [reply(500, deleteFailed)] });
    await confirmDelete();
    press("F4=Delete");
    await waitFor(() => {
      expect(errorText()).toBe(deleteFailed.message);
    });

    expect(infoText()).toBe(INFO.unsuccessful);
    expect(shownValues()).toEqual({ code: CODE, description: STORED });
    expect(redFields()).toEqual({ code: true, description: false });
    expect(marks()).toEqual({ code: deleteFailed.message, description: null });
    expectEditable({ code: false, description: false });
    await expectFocus(LABELS.code);
    expect(shownKeys()).toEqual(["ENTER=Process", "F3=Exit"]);

    pressKey("F12");
    expect(shownValues()).toEqual({ code: "", description: "" });
    expect(errorText()).toBe("");
    expect(redFields()).toEqual({ code: false, description: false });
    expect(marks()).toEqual({ code: null, description: null });
    await settle();
    expect(calls).toHaveLength(2);
  });
});

describe("COTRTUP cancel, exit and refused keys", () => {
  // audit: COTRTUP-037
  // audit: COTRTUPC-041
  it.each([
    { state: "S", replies: { [FETCH]: [reply(200, found)] }, reach: showFound },
    { state: "X", replies: { [FETCH_NEW]: [reply(200, notFound)] }, reach: showNotFound },
    { state: "R", replies: { [FETCH_NEW]: [reply(200, notFound)] }, reach: startAdd },
  ])(
    "F12 in state $state shows the fresh screen without a request or a message",
    async ({ replies, reach }) => {
      const calls = stubApi(replies);
      await reach();
      press("F12=Cancel");

      expect(shownValues()).toEqual({ code: "", description: "" });
      expect(errorText()).toBe("");
      expect(infoText()).toBe(INFO.search);
      expectEditable({ code: true, description: false });
      await expectFocus(LABELS.code);
      await settle();
      expect(calls).toHaveLength(1);
    },
  );

  // audit: COTRTUP-037
  // audit: COTRTUPC-039
  it("F12 with validated changes backs them out without a request: state B over the fetched values", async () => {
    const calls = stubApi({
      [FETCH]: [reply(200, found)],
      [ACTIONS]: [reply(200, validated), reply(400, noChange)],
    });
    await cancelEdit();

    expect(calls).toHaveLength(2);
    expect(shownValues()).toEqual({ code: CODE, description: STORED });
    expectEditable({ code: false, description: true });
    await expectFocus(LABELS.description);
    expect(infoText()).toBe(INFO.changes);
    expect(shownKeys()).toEqual(["ENTER=Process", "F3=Exit"]);

    // ENTER in B runs the edits on endpoint 27; no change from B leaves state E, not S.
    press("ENTER=Process");
    await waitFor(() => {
      expect(errorText()).toBe(noChange.message);
    });
    expect(calls[2]).toEqual({
      route: ACTIONS,
      body: { aid: "ENTER", description: STORED },
      authorization: BEARER,
    });
    expect(infoText()).toBe(INFO.changes);
    expect(redFields()).toEqual({ code: false, description: false });
  });

  // audit: COTRTUPC-039
  it("F12 with validated changes to a new type backs out to blank values; the next key starts over", async () => {
    const calls = stubApi({
      [FETCH_NEW]: [reply(200, notFound)],
      [ACTIONS_NEW]: [reply(200, newValidated)],
    });
    await validateNew();
    press("F12=Cancel");

    expect(errorText()).toBe(MSG.updateCancelled);
    expect(shownValues()).toEqual({ code: "", description: "" });
    expectEditable({ code: true, description: false });
    await expectFocus(LABELS.code);
    expect(infoText()).toBe(INFO.search);

    fireEvent.change(field(LABELS.code), { target: { value: CODE } });
    press("ENTER=Process");
    expect(errorText()).toBe("");
    expect(shownValues()).toEqual({ code: "", description: "" });
    await settle();
    expect(calls.map((call) => call.route)).toEqual([FETCH_NEW, ACTIONS_NEW]);
  });

  // audit: COTRTUP-037
  // audit: COTRTUPC-040
  it("F12 with a delete to confirm drops it: Delete was cancelled on the fresh blank screen", async () => {
    const calls = stubApi({ [FETCH]: [reply(200, found)] });
    await confirmDelete();
    press("F12=Cancel");

    expect(errorText()).toBe(MSG.deleteCancelled);
    expect(shownValues()).toEqual({ code: "", description: "" });
    expectEditable({ code: true, description: false });
    await expectFocus(LABELS.code);
    expect(infoText()).toBe(INFO.search);
    expect(shownKeys()).toEqual(["ENTER=Process", "F3=Exit"]);
    await settle();
    expect(calls).toHaveLength(1);
  });

  // audit: COTRTUP-033
  // audit: COTRTUPC-003
  // audit: COTRTUPC-044
  it.each([
    { title: "opened from the admin menu", state: undefined, target: PATHS.COADM01 },
    { title: "opened by COTRTLI's F2", state: { from: PATHS.COTRTLI }, target: PATHS.COTRTLI },
    {
      title: "handed a caller that is no route",
      state: { from: "/elsewhere" },
      target: PATHS.COADM01,
    },
  ])("F3 when $title returns to $target without a request", async ({ state, target }) => {
    const calls = stubApi({});
    renderScreen(state);
    await expectFocus(LABELS.code);
    // Whatever the caller, the screen opens fresh.
    expect(shownValues()).toEqual({ code: "", description: "" });
    expect(infoText()).toBe(INFO.search);

    press("F3=Exit");
    expect(await screen.findByText(`Stub at ${target}, state null`)).toBeInstanceOf(
      HTMLParagraphElement,
    );
    await settle();
    expect(calls).toEqual([]);
  });

  // audit: COTRTUP-033
  // audit: COTRTUPC-044
  it("F3 exits while a delete awaits confirmation, although its label is dark", async () => {
    const calls = stubApi({ [FETCH]: [reply(200, found)] });
    await confirmDelete();
    expect(key("F3=Exit").hidden).toBe(true);

    pressKey("F3");
    expect(
      await screen.findByText(`Stub at ${PATHS.COADM01}, state null`),
    ).toBeInstanceOf(HTMLParagraphElement);
    await settle();
    expect(calls).toHaveLength(1);
  });

  // audit: COTRTUP-036
  // audit: COTRTUPC-042
  it.each([
    {
      title: "F4 on the fresh screen",
      replies: {},
      reach: open,
      label: LABELS.code,
      typed: CODE,
      name: "F4",
      values: { code: "", description: "" },
      requests: 0,
    },
    {
      title: "F5 with details shown",
      replies: { [FETCH]: [reply(200, found)] },
      reach: showFound,
      label: LABELS.description,
      typed: EDITED,
      name: "F5",
      values: { code: CODE, description: STORED },
      requests: 1,
    },
    {
      title: "F6, which the map labels and no state accepts",
      replies: { [FETCH]: [reply(200, found)] },
      reach: showFound,
      label: LABELS.description,
      typed: EDITED,
      name: "F6",
      values: { code: CODE, description: STORED },
      requests: 1,
    },
    {
      title: "F7, which the map does not label",
      replies: {},
      reach: open,
      label: LABELS.code,
      typed: CODE,
      name: "F7",
      values: { code: "", description: "" },
      requests: 0,
    },
  ])(
    "$title shows Invalid key pressed over the state's screen, discarding what was typed",
    async ({ replies, reach, label, typed, name, values, requests }) => {
      const calls = stubApi(replies);
      await reach();
      fireEvent.change(field(label), { target: { value: typed } });

      pressKey(name);
      expect(errorText()).toBe(MSG.invalidKey);
      expect(shownValues()).toEqual(values);
      expect(infoText()).toBe(INFO.search);
      await settle();
      expect(calls).toHaveLength(requests);
    },
  );

  // audit: COTRTUP-034
  // audit: COTRTUP-037
  // audit: COTRTUPC-042
  it("the keyboard's F4 and F12 are the screen's F4 and F12, Shift held or not", async () => {
    const calls = stubApi({ [FETCH]: [reply(200, found)] });
    await showFound();

    // Shift+F4 sends PF16, which COTRTUPC takes as PF4.
    pressKey("F4", true);
    expect(infoText()).toBe(INFO.confirmDelete);
    pressKey("F12");
    expect(errorText()).toBe(MSG.deleteCancelled);
    expect(shownValues()).toEqual({ code: "", description: "" });
    await settle();
    expect(calls).toHaveLength(1);
  });
});

// Endpoints 26 and 27 name the transaction type by the path alone, and every request carries the
// stored session's token.
describe("COTRTUP: endpoint 26 and 27 requests", () => {
  /** Every request `fetch` received: method, path, query, `Authorization` header and JSON body. */
  async function sentRequests() {
    return Promise.all(
      vi.mocked(fetch).mock.calls.map(async ([input, init]) => {
        const request = input instanceof Request ? input : new Request(input, init);
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
      }),
    );
  }

  it("the fetch and ENTER's action each send the type's path with the bearer token and no query, and the validation shows", async () => {
    stubApi({
      [FETCH]: [reply(200, found)],
      [ACTIONS]: [reply(200, validated)],
    });

    await validateEdit();

    expect(await sentRequests()).toEqual([
      {
        method: "GET",
        path: `/api/v1/transaction-types/${CODE}`,
        query: {},
        authorization: BEARER,
        body: undefined,
      },
      {
        method: "POST",
        path: `/api/v1/transaction-types/${CODE}/actions`,
        query: {},
        authorization: BEARER,
        body: { aid: "ENTER", description: EDITED },
      },
    ]);
  });
});

/**
 * Renders the application's route table at `/transaction-types/maintain`, so the admin guard that
 * only the table mounts stands in front of the screen. Its screens load lazily, so a wait for one
 * to render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COTRTUP] });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

// A 401 clears the stored session (`api/client.ts`), and the admin guard in front of the route
// replaces the still-mounted screen with sign-on at once: after the ENTER that sent the refused
// request, the test presses no key and follows no link.
describe("COTRTUP in the route table: a 401 ends the session", () => {
  it.each([
    {
      title: "the fetch of ENTER (endpoint 26)",
      replies: { [FETCH]: [reply(401, signonRequired)] },
      drive: async () => {
        fireEvent.change(field(LABELS.code), { target: { value: CODE } });
        press("ENTER=Process");
      },
      calls: [FETCH],
    },
    {
      title: "the validation action of ENTER (endpoint 27)",
      replies: { [FETCH]: [reply(200, found)], [ACTIONS]: [reply(401, signonRequired)] },
      drive: async () => {
        fireEvent.change(field(LABELS.code), { target: { value: CODE } });
        press("ENTER=Process");
        await waitFor(() => {
          expect(field(LABELS.description).value).toBe(STORED);
        });
        fireEvent.change(field(LABELS.description), { target: { value: EDITED } });
        press("ENTER=Process");
      },
      calls: [FETCH, ACTIONS],
    },
  ])(
    "$title answered 401 replaces the screen with sign-on and clears the session, with no retry",
    async ({ replies, drive, calls: expected }) => {
      const calls = stubApi(replies);
      const router = renderRouteTable();
      expect(
        await screen.findByRole(
          "heading",
          { level: 1, name: "Maintain Transaction Type" },
          { timeout: 5_000 },
        ),
      ).toBeDefined();

      await drive();

      expect(
        await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
      ).toBeDefined();
      expect(router.state.location.pathname).toBe(PATHS.COSGN00);
      expect(
        screen.queryByRole("heading", { level: 1, name: "Maintain Transaction Type" }),
      ).toBeNull();
      expect(getSession()).toBeNull();
      expect(calls.map((call) => call.route)).toEqual(expected);
      expect(calls.every((call) => call.authorization === BEARER)).toBe(true);
      router.dispose();
    },
  );
});
