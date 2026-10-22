import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import type { components } from "@/api/schema";
import { setSession } from "@/auth/session";
import { PATHS } from "@/routes";
import { COTRN02 } from "./COTRN02";

/*
 * Render test of the Add Transaction screen (map COTRN2A, transaction CT02, program COTRN02C,
 * route `/transactions/add`) in jsdom, with `fetch` stubbed for endpoint 13,
 * `POST /api/v1/transactions`. Expected values are the audited results of
 * `modern/audit/screens.md` (`COTRN02-nnn`) and `modern/audit/online/COTRN02C.md`
 * (`COTRN02C-nnn`). Every card number a response carries is redacted; the one full-width card
 * number below is synthetic request input, and no password appears.
 *
 * Not tagged here:
 * - COTRN02-002, -003, -005, -007, -008 and -010 describe the header values as read-only `Input`
 *   elements, while `ScreenShell` renders them as text; their content rows (COTRN02-062 to
 *   COTRN02-067) are proved instead.
 * - COTRN02C-001, the sign-on guard, belongs to `RequireSignon`, which this test does not mount.
 */

const ENDPOINT = "/api/v1/transactions";

/** The account the user keys the transaction by. */
const ACCOUNT = "00000000050";
/** The card COTRN02C fills in from the cross-reference, as endpoint 13 returns it (E32). */
const REDACTED_CARD = "************5740";
/** A synthetic card number the user types: request input only, assembled at run time. */
const TYPED_CARD = ["0000", "0000", "0000", "5740"].join("");

/** The prompt that follows every passed edit with a blank, `N` or `n` confirmation. */
const PROMPT_TEXT = "Confirm to add this transaction...";
/** `CCDA-MSG-INVALID-KEY`, its 50 bytes with the nine trailing spaces (COTRN02C-006). */
const INVALID_KEY_TEXT = "Invalid key pressed. Please see below...".padEnd(50, " ");

/** Each input's label `INITIAL`, by the form member it labels, in map order. */
const LABELS = {
  acctId: "Enter Acct #:",
  cardNum: "Card #:",
  typeCode: "Type CD:",
  categoryCode: "Category CD:",
  source: "Source:",
  description: "Description:",
  amount: "Amount:",
  origDate: "Orig Date:",
  procDate: "Proc Date:",
  merchantId: "Merchant ID:",
  merchantName: "Merchant Name:",
  merchantCity: "Merchant City:",
  merchantZip: "Merchant Zip:",
  confirm: "You are about to add this transaction. Please confirm :",
} as const;

/**
 * `POS=(row,col)` and `LENGTH` of each label and its input (`COTRN02-012` to `COTRN02-057`). The
 * input's `LENGTH` is its `maxLength`: 11, 16, 2, 4, 10, 60, 12, 10, 10, 9, 30, 25, 10 and 1.
 */
const MAP_INPUTS = [
  { name: "acctId", label: [6, 6, 13], input: [6, 21, 11] },
  { name: "cardNum", label: [6, 46, 7], input: [6, 55, 16] },
  { name: "typeCode", label: [10, 6, 8], input: [10, 15, 2] },
  { name: "categoryCode", label: [10, 23, 12], input: [10, 36, 4] },
  { name: "source", label: [10, 46, 7], input: [10, 54, 10] },
  { name: "description", label: [12, 6, 12], input: [12, 19, 60] },
  { name: "amount", label: [14, 6, 7], input: [14, 14, 12] },
  { name: "origDate", label: [14, 31, 10], input: [14, 42, 10] },
  { name: "procDate", label: [14, 57, 10], input: [14, 68, 10] },
  { name: "merchantId", label: [16, 6, 12], input: [16, 19, 9] },
  { name: "merchantName", label: [16, 33, 14], input: [16, 48, 30] },
  { name: "merchantCity", label: [18, 6, 14], input: [18, 21, 25] },
  { name: "merchantZip", label: [18, 53, 13], input: [18, 67, 10] },
  { name: "confirm", label: [21, 6, 55], input: [21, 63, 1] },
] as const;

/** The fourteen inputs in map order, which is the tab order. */
const NAMES = MAP_INPUTS.map((mapInput) => mapInput.name);

/** The format hints, each the description of its input (`COTRN02-041` to `-043`, `-059`). */
const HINTS = [
  { name: "amount", text: "(-99999999.99)", cell: [15, 13, 14] },
  { name: "origDate", text: "(YYYY-MM-DD)", cell: [15, 41, 12] },
  { name: "procDate", text: "(YYYY-MM-DD)", cell: [15, 67, 12] },
  { name: "confirm", text: "(Y/N)", cell: [21, 66, 5] },
] as const;

/** `(or)` between the two keys (`COTRN02-015`). */
const OR_CELL = [6, 37, 4] as const;
/** The rule of 70 dashes under the keys (`COTRN02-019`). */
const RULE_CELL = [8, 6, 70] as const;

/** Every `GridCell` of the screen body: 14 labels, 14 inputs, 4 hints, `(or)` and the rule. */
const BODY_CELLS = [
  ...MAP_INPUTS.flatMap((mapInput) => [mapInput.label, mapInput.input]),
  ...HINTS.map((hint) => hint.cell),
  OR_CELL,
  RULE_CELL,
];

/**
 * `POS=(row,col)` of the 14 zero-length auto-skip stoppers, `COTRN02-014`, `-018`, `-022`,
 * `-025`, `-028`, `-031`, `-034`, `-037`, `-040`, `-046`, `-049`, `-052`, `-055` and `-058`.
 */
const STOPPERS = [
  [6, 33],
  [6, 72],
  [10, 18],
  [10, 41],
  [10, 65],
  [12, 80],
  [14, 27],
  [14, 53],
  [14, 79],
  [16, 29],
  [16, 79],
  [18, 47],
  [18, 78],
  [21, 65],
] as const;

/** Every input empty: the first entry (COTRN02C-002), after F4 (COTRN02C-005) and the add. */
const BLANK = Object.fromEntries(NAMES.map((name) => [name, ""]));

/** What the user types for an ENTER keyed by the account, the card left blank. */
const TYPED_BY_ACCOUNT = {
  acctId: ACCOUNT,
  typeCode: "01",
  categoryCode: "0001",
  source: "POS TERM",
  description: "Purchase at Abshire-Lowe",
  amount: "+00000012.34",
  origDate: "2022-06-10",
  procDate: "2022-06-10",
  merchantId: "800000000",
  merchantName: "Abshire-Lowe",
  merchantCity: "North Enoshaven",
  merchantZip: "72112",
};

/** {@link TYPED_BY_ACCOUNT} as endpoint 13 receives it: every value exactly as typed. */
const enterByAccount: components["schemas"]["TranAddRequest"] = {
  aid: "ENTER",
  acct_id: ACCOUNT,
  fields: {
    type_cd: "01",
    cat_cd: "0001",
    source: "POS TERM",
    description: "Purchase at Abshire-Lowe",
    amount: "+00000012.34",
    orig_date: "2022-06-10",
    proc_date: "2022-06-10",
    merchant_id: "800000000",
    merchant_name: "Abshire-Lowe",
    merchant_city: "North Enoshaven",
    merchant_zip: "72112",
  },
  confirm: "",
};

/** COTRN02C-041: every edit passed, the confirmation blank; the fields as the map holds them. */
const promptByAccount: components["schemas"]["TranAddResult"] = {
  acct_id: ACCOUNT,
  card_num: REDACTED_CARD,
  copied: {
    type_cd: "01",
    cat_cd: "0001",
    source: "POS TERM  ",
    description: "Purchase at Abshire-Lowe".padEnd(60, " "),
    amount: "+00000012.34",
    orig_date: "2022-06-10",
    proc_date: "2022-06-10",
    merchant_id: "800000000",
    merchant_name: "Abshire-Lowe".padEnd(30, " "),
    merchant_city: "North Enoshaven".padEnd(25, " "),
    merchant_zip: "72112     ",
  },
  field: "confirm",
  message: PROMPT_TEXT,
  new_tran_id: null,
};

/** The screen after {@link promptByAccount}: the filled-in card redacted, trailing spaces kept. */
const SHOWN_PROMPT = {
  acctId: ACCOUNT,
  cardNum: REDACTED_CARD,
  typeCode: "01",
  categoryCode: "0001",
  source: "POS TERM  ",
  description: "Purchase at Abshire-Lowe".padEnd(60, " "),
  amount: "+00000012.34",
  origDate: "2022-06-10",
  procDate: "2022-06-10",
  merchantId: "800000000",
  merchantName: "Abshire-Lowe".padEnd(30, " "),
  merchantCity: "North Enoshaven".padEnd(25, " "),
  merchantZip: "72112     ",
  confirm: "",
};

/** The ENTER that re-sends {@link SHOWN_PROMPT}: the account keys it, the card is not sent. */
const enterShownPrompt: components["schemas"]["TranAddRequest"] = {
  aid: "ENTER",
  acct_id: ACCOUNT,
  fields: {
    type_cd: "01",
    cat_cd: "0001",
    source: "POS TERM  ",
    description: "Purchase at Abshire-Lowe".padEnd(60, " "),
    amount: "+00000012.34",
    orig_date: "2022-06-10",
    proc_date: "2022-06-10",
    merchant_id: "800000000",
    merchant_name: "Abshire-Lowe".padEnd(30, " "),
    merchant_city: "North Enoshaven".padEnd(25, " "),
    merchant_zip: "72112     ",
  },
  confirm: "",
};

/** COTRN02C-049: the add, every field empty, the 66-character success message. */
const added: components["schemas"]["TranAddResult"] = {
  acct_id: null,
  card_num: null,
  copied: null,
  field: "acct_id",
  message: "Transaction added successfully.  Your Tran ID is 0000000000000301.",
  new_tran_id: "0000000000000301",
};

/**
 * COTRN02C-054: PF5 copied the highest-ID transaction. Its dates sit at the edges of a year, so a
 * date read through a time zone would show a neighbouring day.
 */
const promptCopied: components["schemas"]["TranAddResult"] = {
  acct_id: ACCOUNT,
  card_num: REDACTED_CARD,
  copied: {
    type_cd: "02",
    cat_cd: "0005",
    source: "OPERATOR  ",
    description: "Return of goods".padEnd(60, " "),
    amount: "-00000012.00",
    orig_date: "2022-12-31",
    proc_date: "2023-01-01",
    merchant_id: "000000042",
    merchant_name: "Kris-Wuckert".padEnd(30, " "),
    merchant_city: "Lake Rubye".padEnd(25, " "),
    merchant_zip: "10001     ",
  },
  field: "confirm",
  message: PROMPT_TEXT,
  new_tran_id: null,
};

/** The screen after {@link promptCopied}: every copied value verbatim, the dates unshifted. */
const SHOWN_COPIED = {
  acctId: ACCOUNT,
  cardNum: REDACTED_CARD,
  typeCode: "02",
  categoryCode: "0005",
  source: "OPERATOR  ",
  description: "Return of goods".padEnd(60, " "),
  amount: "-00000012.00",
  origDate: "2022-12-31",
  procDate: "2023-01-01",
  merchantId: "000000042",
  merchantName: "Kris-Wuckert".padEnd(30, " "),
  merchantCity: "Lake Rubye".padEnd(25, " "),
  merchantZip: "10001     ",
  confirm: "",
};

/** COTRN02C-033: the amount edit. */
const amountFormat: components["schemas"]["ScreenError"] = {
  message: "Amount should be in format -99999999.99",
  field: "amount",
};

/** The request DTO's length check names a data field as `fields.<member>`. */
const descriptionTooLong: components["schemas"]["ScreenError"] = {
  message: "Input does not fit the screen field...",
  field: "fields.description",
};

/** COTRN02C-042: a confirmation other than `Y`, `y`, `N`, `n` and blank. */
const confirmInvalid: components["schemas"]["ScreenError"] = {
  message: "Invalid value. Valid values are (Y/N)...",
  field: "confirm",
};

/** COTRN02C-051: the write fails. */
const addFailed: components["schemas"]["ScreenError"] = {
  message: "Unable to Add Transaction...",
  field: "acct_id",
};

/** COTRN02C-018: the card path finds no cross-reference. */
const cardNotFound: components["schemas"]["ScreenError"] = {
  message: "Card Number NOT found...",
  field: "card_num",
};

/** A JSON response, as the API sends every success and every `ScreenError`. */
function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/**
 * A reply that arrives only when released, for the keys pressed while a request is outstanding.
 * The executor runs synchronously, so `release` holds the resolver at once.
 */
function heldReply(): { reply: Promise<Response>; release: (response: Response) => void } {
  const resolvers: ((response: Response) => void)[] = [];
  const reply = new Promise<Response>((resolve) => {
    resolvers.push(resolve);
  });
  return {
    reply,
    release: (response) => {
      for (const resolve of resolvers) {
        resolve(response);
      }
    },
  };
}

/**
 * Replaces `fetch`. Endpoint 13 answers with `replies`, one per request in order; any other
 * request, or one more than `replies` holds, gets a 500. Returns every request in the order the
 * screen sent it, with its JSON body.
 */
function stubApi(
  replies: readonly (Response | Promise<Response>)[],
): { method: string; path: string; authorization: string | null; body: unknown }[] {
  const queue = [...replies];
  const sent: { method: string; path: string; authorization: string | null; body: unknown }[] =
    [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      const { pathname } = new URL(req.url);
      const body: unknown = req.method === "POST" ? await req.clone().json() : null;
      sent.push({
        method: req.method,
        path: pathname,
        authorization: req.headers.get("Authorization"),
        body,
      });
      const reply = req.method === "POST" && pathname === ENDPOINT ? queue.shift() : undefined;
      if (reply !== undefined) {
        return reply;
      }
      const unexpected: components["schemas"]["ScreenError"] = {
        message: `Unexpected request ${req.method} ${pathname}`,
      };
      return json(500, unexpected);
    }),
  );
  return sent;
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

/** COTRN02 at `/transactions/add` with `state`, and stubs at the routes F3 can reach. */
function renderScreen(state?: unknown) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[{ pathname: PATHS.COTRN02, state }]}>
        <Routes>
          <Route path={PATHS.COTRN02} element={<COTRN02 />} />
          <Route path={PATHS.COMEN01} element={<RouteStub />} />
          <Route path={PATHS.COTRN00} element={<RouteStub />} />
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

/** The `<input>` the label of form member `name` names, failing when it names anything else. */
function field(name: keyof typeof LABELS): HTMLInputElement {
  const element = screen.getByLabelText(LABELS[name]);
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`"${LABELS[name]}" labels <${element.tagName.toLowerCase()}>, not an input`);
  }
  return element;
}

/** The value every input shows, by form member. */
function shownValues(): Record<string, string> {
  return Object.fromEntries(NAMES.map((name) => [name, field(name).value]));
}

/** Types each given value into its input; members left out keep what they show. */
function typeInto(values: Partial<Record<keyof typeof LABELS, string>>): void {
  for (const name of NAMES) {
    const value = values[name];
    if (value !== undefined) {
      fireEvent.change(field(name), { target: { value } });
    }
  }
}

/** The `GridCell` classes of a map field at `POS=(row,col)` spanning `LENGTH` columns. */
function cellClass([row, col, span]: readonly [number, number, number]): string {
  return `row-start-${row} col-start-${col} col-span-${span}`;
}

/** The screen's one message row (`COTRN02-060`): its kind, its text and whether it is red. */
function messageRow(): { kind: string | undefined; text: string; red: boolean } {
  const rows = screen.getAllByRole("alert");
  expect(rows).toHaveLength(1);
  const [row] = rows;
  return {
    kind: row?.dataset.messageKind,
    text: row?.textContent ?? "",
    red: row?.classList.contains("text-destructive") ?? false,
  };
}

/** `aria-busy` of the screen's form: `"true"` while a request is outstanding. */
function formBusy(container: HTMLElement): string | null {
  return container.querySelector("form")?.getAttribute("aria-busy") ?? null;
}

/** The text of the header field `name` (`data-field` of `ScreenShell`). */
function headerField(container: HTMLElement, name: string): string | null {
  return container.querySelector(`[data-field="${name}"]`)?.textContent ?? null;
}

/** The `GridCell` classes of the element whose whole text is `text`. */
function textCell(text: string): string {
  return screen.getByText(text, { normalizer: (shown) => shown }).parentElement?.className ?? "";
}

/** A keydown of `key` at the focused element, as the browser sends it; it bubbles to `window`. */
function pressKey(key: string, shiftKey = false): void {
  fireEvent.keyDown(document.activeElement ?? document.body, { key, shiftKey });
}

/** The function-key button labelled `label`. */
function keyButton(label: string): HTMLElement {
  return screen.getByRole("button", { name: label });
}

/** `type`, `variant` and `size` of the function-key button labelled `label`. */
function keyKind(label: string): (string | null | undefined)[] {
  const button = keyButton(label);
  return [button.getAttribute("type"), button.dataset.variant, button.dataset.size];
}

/** Presses ENTER through the `ENTER=Continue` button. */
function pressEnter(): void {
  fireEvent.click(keyButton("ENTER=Continue"));
}

/**
 * Moves the cursor to `element`, so that the screen's own move of it is observable. The move runs
 * inside `waitFor`, which lets React render the blur it causes.
 */
async function moveFocus(element: HTMLElement): Promise<void> {
  await waitFor(() => {
    element.focus();
    expect(document.activeElement).toBe(element);
  });
}

/**
 * Lets pending effects, mutations and stubbed responses run, so that a request the screen would
 * send has been sent before a test counts the requests. The wait runs inside `waitFor`, which lets
 * React render the form-state updates of a key the screen ignores while a request is outstanding.
 */
async function settle(): Promise<void> {
  await waitFor(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, 20);
    });
  });
}

/** Types {@link TYPED_BY_ACCOUNT}, presses ENTER and waits for {@link SHOWN_PROMPT}. */
async function reachPrompt(): Promise<void> {
  typeInto(TYPED_BY_ACCOUNT);
  pressEnter();
  await waitFor(() => {
    expect(shownValues()).toEqual(SHOWN_PROMPT);
  });
}

beforeEach(() => {
  setSession("test-token", "USER0001", "U");
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

describe("COTRN02 Add Transaction screen", () => {
  // audit: COTRN02-011
  // audit: COTRN02-012
  // audit: COTRN02-015
  // audit: COTRN02-016
  // audit: COTRN02-019
  // audit: COTRN02-020
  // audit: COTRN02-023
  // audit: COTRN02-026
  // audit: COTRN02-029
  // audit: COTRN02-032
  // audit: COTRN02-035
  // audit: COTRN02-038
  // audit: COTRN02-041
  // audit: COTRN02-042
  // audit: COTRN02-043
  // audit: COTRN02-044
  // audit: COTRN02-047
  // audit: COTRN02-050
  // audit: COTRN02-053
  // audit: COTRN02-056
  // audit: COTRN02-059
  // audit: COTRN02-061
  // audit: COTRN02C-002
  it("renders the map empty: title, 14 labelled inputs, hints, keys, cursor on Acct", async () => {
    const requests = stubApi([]);
    renderScreen({ from: PATHS.COMEN01 });

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Add Transaction");

    for (const mapInput of MAP_INPUTS) {
      const box = field(mapInput.name);
      const label = screen.getByText(LABELS[mapInput.name], { normalizer: (shown) => shown });
      expect(label.tagName).toBe("LABEL");
      expect(label.getAttribute("for")).toBe(box.id);
      expect(label.parentElement?.className).toBe(cellClass(mapInput.label));
      expect(box.parentElement?.className).toBe(cellClass(mapInput.input));
      expect(box.maxLength).toBe(mapInput.input[2]);
      expect(box.readOnly).toBe(false);
      // No input is `NUM` on the map, so none is narrowed to digits.
      expect(box.getAttribute("inputmode")).toBeNull();
    }
    expect(NAMES.map((name) => field(name).maxLength)).toEqual([
      11, 16, 2, 4, 10, 60, 12, 10, 10, 9, 30, 25, 10, 1,
    ]);

    for (const hint of HINTS) {
      const [describedBy] = (field(hint.name).getAttribute("aria-describedby") ?? "").split(" ");
      const description = document.getElementById(describedBy ?? "");
      expect(description?.textContent).toBe(hint.text);
      expect(description?.parentElement?.className).toBe(cellClass(hint.cell));
    }

    expect(textCell("(or)")).toBe(cellClass(OR_CELL));
    const rule = screen.getByText("-".repeat(70));
    expect(rule.getAttribute("aria-hidden")).toBe("true");
    expect(rule.parentElement?.className).toBe(cellClass(RULE_CELL));

    const keys = within(screen.getByRole("group", { name: "Function keys" })).getAllByRole(
      "button",
    );
    expect(
      keys.map((key) => [
        key.textContent,
        key.getAttribute("type"),
        key.dataset.variant,
        key.dataset.size,
      ]),
    ).toEqual([
      ["ENTER=Continue", "submit", "default", "sm"],
      ["F3=Back", "button", "outline", "sm"],
      ["F4=Clear", "button", "outline", "sm"],
      ["F5=Copy Last Tran.", "button", "outline", "sm"],
    ]);

    // First entry: every field and the message row empty, the cursor on ACTIDIN, no request.
    expect(shownValues()).toEqual(BLANK);
    expect(messageRow()).toEqual({ kind: "error", text: "", red: true });
    expect(document.activeElement).toBe(field("acctId"));
    await settle();
    expect(requests).toHaveLength(0);
  });

  // audit: COTRN02-014
  // audit: COTRN02-018
  // audit: COTRN02-022
  // audit: COTRN02-025
  // audit: COTRN02-028
  // audit: COTRN02-031
  // audit: COTRN02-034
  // audit: COTRN02-037
  // audit: COTRN02-040
  // audit: COTRN02-046
  // audit: COTRN02-049
  // audit: COTRN02-052
  // audit: COTRN02-055
  // audit: COTRN02-058
  it("places only the map's 34 body fields, so no element stands at a zero-length stopper", () => {
    stubApi([]);
    const { container } = renderScreen({ from: PATHS.COMEN01 });

    const shown = Array.from(
      container.querySelectorAll("form [class*='row-start-']"),
      (cell) => cell.className,
    );
    expect(shown.sort()).toEqual(BODY_CELLS.map(cellClass).sort());
    for (const [row, col] of STOPPERS) {
      const covering = BODY_CELLS.filter(
        ([cellRow, start, span]) => cellRow === row && start <= col && col < start + span,
      );
      expect(covering).toEqual([]);
    }
  });

  // audit: COTRN02-001
  // audit: COTRN02-004
  // audit: COTRN02-006
  // audit: COTRN02-009
  // audit: COTRN02-062
  // audit: COTRN02-063
  // audit: COTRN02-064
  // audit: COTRN02-065
  // audit: COTRN02-066
  // audit: COTRN02-067
  // audit: COTRN02C-059
  it("fills header rows 1 and 2 at every send: CT02, COTRN02C, titles, date, time", async () => {
    // Only `Date` is faked, so the timers that React, TanStack Query and `waitFor` use stay real.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2024, 0, 2, 3, 4, 5));
    stubApi([json(200, promptByAccount)]);
    const { container } = renderScreen({ from: PATHS.COMEN01 });

    expect(textCell("Tran:")).toBe(cellClass([1, 1, 5]));
    expect(textCell("Date:")).toBe(cellClass([1, 65, 5]));
    expect(textCell("Prog:")).toBe(cellClass([2, 1, 5]));
    expect(textCell("Time:")).toBe(cellClass([2, 65, 5]));
    expect(headerField(container, "TRNNAME")).toBe("CT02");
    expect(headerField(container, "PGMNAME")).toBe("COTRN02C");
    expect(headerField(container, "TITLE01")).toBe("      AWS Mainframe Modernization       ");
    expect(headerField(container, "TITLE02")).toBe("              CardDemo                  ");
    expect(headerField(container, "CURDATE")).toBe("01/02/24");
    expect(headerField(container, "CURTIME")).toBe("03:04:05");

    vi.setSystemTime(new Date(2025, 11, 31, 23, 59, 58));
    await reachPrompt();

    expect(headerField(container, "TRNNAME")).toBe("CT02");
    expect(headerField(container, "PGMNAME")).toBe("COTRN02C");
    expect(headerField(container, "CURDATE")).toBe("12/31/25");
    expect(headerField(container, "CURTIME")).toBe("23:59:58");
  });

  // audit: COTRN02-068
  // audit: COTRN02C-058
  // audit: COTRN02C-060
  it.each([
    { title: "the ENTER=Continue button", byButton: true },
    { title: "Enter in a field, the form's implicit submission", byButton: false },
  ])("ENTER by $title sends the typed fields, shows the prompt, then adds", async (row) => {
    // The second answer repeats the prompt and changes nothing shown, so it is held: the test
    // releases it and waits for the form to stop being busy before the next key.
    const second = heldReply();
    const requests = stubApi([json(200, promptByAccount), second.reply, json(200, added)]);
    const { container } = renderScreen({ from: PATHS.COMEN01 });
    expect(keyKind("ENTER=Continue")).toEqual(["submit", "default", "sm"]);

    // The account typed, the card left blank, the dates typed as `YYYY-MM-DD`.
    typeInto(TYPED_BY_ACCOUNT);
    if (row.byButton) {
      pressEnter();
    } else {
      const form = container.querySelector("form");
      expect(form).not.toBeNull();
      fireEvent.submit(form ?? document.body);
    }

    await waitFor(() => {
      expect(shownValues()).toEqual(SHOWN_PROMPT);
    });
    expect(requests).toHaveLength(1);
    expect(requests[0]?.method).toBe("POST");
    expect(requests[0]?.path).toBe(ENDPOINT);
    expect(requests[0]?.authorization).toBe("Bearer test-token");
    // No `card_num` member: a blank card is not sent, and the dates go exactly as typed.
    expect(requests[0]?.body).toStrictEqual(enterByAccount);
    expect(field("cardNum").value).toBe(REDACTED_CARD);
    // The 200 prompt is red, as every message but the success message (COTRN02C-058).
    expect(messageRow()).toEqual({ kind: "error", text: PROMPT_TEXT, red: true });
    await waitFor(() => {
      expect(document.activeElement).toBe(field("confirm"));
    });

    // A second ENTER with the card untouched: the filled-in redacted card is never sent back.
    pressEnter();
    await waitFor(() => {
      expect(requests).toHaveLength(2);
    });
    await waitFor(() => {
      expect(formBusy(container)).toBe("true");
    });
    expect(requests[1]?.body).toStrictEqual(enterShownPrompt);
    expect(JSON.stringify(requests[1]?.body)).not.toContain("*");
    second.release(json(200, promptByAccount));
    await waitFor(() => {
      expect(formBusy(container)).toBe("false");
    });
    await waitFor(() => {
      expect(shownValues()).toEqual(SHOWN_PROMPT);
    });
    expect(messageRow()).toEqual({ kind: "error", text: PROMPT_TEXT, red: true });

    // `Y` confirms: the add empties every field and shows the green success message.
    typeInto({ confirm: "Y" });
    pressEnter();
    await waitFor(() => {
      expect(shownValues()).toEqual(BLANK);
    });
    const confirmed: components["schemas"]["TranAddRequest"] = {
      ...enterShownPrompt,
      confirm: "Y",
    };
    expect(requests[2]?.body).toStrictEqual(confirmed);
    expect(JSON.stringify(requests[2]?.body)).not.toContain("*");
    const success = messageRow();
    expect(success).toEqual({ kind: "info", text: added.message, red: false });
    // The longest COTRN02C message, 66 characters, fits the 78 columns of ERRMSG uncut.
    expect(success.text).toHaveLength(66);
    await waitFor(() => {
      expect(document.activeElement).toBe(field("acctId"));
    });
    expect(requests).toHaveLength(3);
  });

  // audit: COTRN02C-060
  it("sends a typed card number as input, also when retyped over the redacted one", async () => {
    // The card path's prompt carries the account filled in beside the redacted card; with the
    // account filled, the next ENTER takes the account path whatever the card holds (COTRN02C-011).
    const requests = stubApi([json(200, promptByAccount), json(200, promptByAccount)]);
    renderScreen({ from: PATHS.COMEN01 });

    // The account blank, the card typed: the body carries the card and no `acct_id` member.
    typeInto({ ...TYPED_BY_ACCOUNT, acctId: "", cardNum: TYPED_CARD });
    pressEnter();
    await waitFor(() => {
      expect(shownValues()).toEqual(SHOWN_PROMPT);
    });
    const byCard: components["schemas"]["TranAddRequest"] = {
      aid: "ENTER",
      card_num: TYPED_CARD,
      fields: enterByAccount.fields,
      confirm: "",
    };
    expect(requests[0]?.body).toStrictEqual(byCard);

    // The program filled the account and showed the card redacted; retyping the card sends it,
    // and the answer shows the card redacted again.
    typeInto({ cardNum: TYPED_CARD });
    expect(field("cardNum").value).toBe(TYPED_CARD);
    pressEnter();
    await waitFor(() => {
      expect(shownValues()).toEqual(SHOWN_PROMPT);
    });
    const retyped: components["schemas"]["TranAddRequest"] = {
      ...enterShownPrompt,
      card_num: TYPED_CARD,
    };
    expect(requests[1]?.body).toStrictEqual(retyped);
    expect(requests).toHaveLength(2);
  });

  // audit: COTRN02-071
  // audit: COTRN02C-060
  it.each([
    { title: "the F5=Copy Last Tran. button", byKey: false },
    { title: "the F5 key", byKey: true },
  ])("$title sends PF5 and shows the copied fields verbatim, dates unshifted", async (row) => {
    const requests = stubApi([json(200, promptCopied), json(200, added)]);
    renderScreen({ from: PATHS.COMEN01 });
    expect(keyKind("F5=Copy Last Tran.")).toEqual(["button", "outline", "sm"]);

    typeInto({ acctId: ACCOUNT, description: "ignored by the copy" });
    if (row.byKey) {
      pressKey("F5");
    } else {
      fireEvent.click(keyButton("F5=Copy Last Tran."));
    }

    await waitFor(() => {
      expect(shownValues()).toEqual(SHOWN_COPIED);
    });
    const copyLast: components["schemas"]["TranAddRequest"] = {
      aid: "PF5",
      acct_id: ACCOUNT,
      fields: {
        type_cd: "",
        cat_cd: "",
        source: "",
        description: "ignored by the copy",
        amount: "",
        orig_date: "",
        proc_date: "",
        merchant_id: "",
        merchant_name: "",
        merchant_city: "",
        merchant_zip: "",
      },
      confirm: "",
    };
    expect(requests[0]?.body).toStrictEqual(copyLast);
    expect(messageRow()).toEqual({ kind: "error", text: PROMPT_TEXT, red: true });
    await waitFor(() => {
      expect(document.activeElement).toBe(field("confirm"));
    });

    // ENTER with `Y` adds the copy: the copied fields go back as shown, the card is not sent.
    typeInto({ confirm: "Y" });
    pressEnter();
    await waitFor(() => {
      expect(shownValues()).toEqual(BLANK);
    });
    const addCopy: components["schemas"]["TranAddRequest"] = {
      aid: "ENTER",
      acct_id: ACCOUNT,
      fields: {
        type_cd: "02",
        cat_cd: "0005",
        source: "OPERATOR  ",
        description: "Return of goods".padEnd(60, " "),
        amount: "-00000012.00",
        orig_date: "2022-12-31",
        proc_date: "2023-01-01",
        merchant_id: "000000042",
        merchant_name: "Kris-Wuckert".padEnd(30, " "),
        merchant_city: "Lake Rubye".padEnd(25, " "),
        merchant_zip: "10001     ",
      },
      confirm: "Y",
    };
    expect(requests[1]?.body).toStrictEqual(addCopy);
    expect(requests).toHaveLength(2);
  });

  it.each([
    {
      title: "F5 with an account longer than its 11 columns",
      key: "F5=Copy Last Tran.",
      typed: { acctId: `${ACCOUNT}0` },
      message: "String must contain at most 11 character(s)",
      focus: "acctId",
    },
    {
      title: "F5 with a card number longer than its 16 columns",
      key: "F5=Copy Last Tran.",
      typed: { cardNum: `${TYPED_CARD}0` },
      message: "String must contain at most 16 character(s)",
      focus: "cardNum",
    },
    {
      title: "ENTER with two data fields too long, the first in map order",
      key: "ENTER=Continue",
      typed: { ...TYPED_BY_ACCOUNT, description: "D".repeat(61), merchantZip: "Z".repeat(11) },
      message: "String must contain at most 60 character(s)",
      focus: "description",
    },
  ] as const)("$title shows the map-length message and sends nothing", async (row) => {
    const requests = stubApi([]);
    renderScreen({ from: PATHS.COMEN01 });
    const key = keyButton(row.key);
    await moveFocus(key);

    // Assigning `value` bypasses `maxLength`, as a scripted paste would.
    typeInto(row.typed);
    fireEvent.click(key);

    await waitFor(() => {
      expect(messageRow()).toEqual({ kind: "error", text: row.message, red: true });
    });
    await waitFor(() => {
      expect(document.activeElement).toBe(field(row.focus));
    });
    await settle();
    expect(requests).toHaveLength(0);
  });

  // audit: COTRN02-060
  // audit: COTRN02C-060
  it.each([
    {
      title: "a 400 amount-format ScreenError",
      typed: { ...TYPED_BY_ACCOUNT, amount: "12.34" },
      reply: json(400, amountFormat),
      message: amountFormat.message,
      focus: "amount",
    },
    {
      // The zod schema keeps such input from being sent; the stub answers it all the same, to
      // show that a `fields.<member>` name moves the cursor to that member's input.
      title: "a 400 ScreenError naming fields.description",
      typed: TYPED_BY_ACCOUNT,
      reply: json(400, descriptionTooLong),
      message: descriptionTooLong.message,
      focus: "description",
    },
    {
      title: "a 400 confirmation-value ScreenError",
      typed: { ...TYPED_BY_ACCOUNT, confirm: "X" },
      reply: json(400, confirmInvalid),
      message: confirmInvalid.message,
      focus: "confirm",
    },
    {
      title: "a 404 card-not-found ScreenError on the card path",
      typed: { ...TYPED_BY_ACCOUNT, acctId: "", cardNum: TYPED_CARD },
      reply: json(404, cardNotFound),
      message: cardNotFound.message,
      focus: "cardNum",
    },
    {
      title: "a 500 add-failure ScreenError",
      typed: { ...TYPED_BY_ACCOUNT, confirm: "Y" },
      reply: json(500, addFailed),
      message: addFailed.message,
      focus: "acctId",
    },
    {
      title: "a 500 with no ScreenError body, which names no field",
      typed: TYPED_BY_ACCOUNT,
      reply: new Response(null, { status: 500, statusText: "Internal Server Error" }),
      message: "HTTP 500 Internal Server Error",
      focus: undefined,
    },
  ] as const)("$title shows its message, keeps every field, moves the cursor", async (row) => {
    const requests = stubApi([row.reply]);
    renderScreen({ from: PATHS.COMEN01 });
    typeInto(row.typed);
    const enter = keyButton("ENTER=Continue");
    await moveFocus(enter);
    // The ERRMSG row is a live region before its first message, empty and red.
    const [errmsg] = screen.getAllByRole("alert");
    expect(errmsg?.getAttribute("aria-live")).toBe("polite");
    expect(messageRow()).toEqual({ kind: "error", text: "", red: true });

    fireEvent.click(enter);

    await waitFor(() => {
      expect(messageRow()).toEqual({ kind: "error", text: row.message, red: true });
    });
    expect(shownValues()).toEqual({ ...BLANK, ...row.typed });
    // The named input gets the cursor; with no field named, it stays where it was.
    await waitFor(() => {
      expect(document.activeElement).toBe(row.focus === undefined ? enter : field(row.focus));
    });
    await settle();
    expect(document.activeElement).toBe(row.focus === undefined ? enter : field(row.focus));
    expect(requests).toHaveLength(1);
  });

  // audit: COTRN02-070
  // audit: COTRN02C-005
  it.each([
    { title: "the F4=Clear button", byKey: false },
    { title: "the F4 key", byKey: true },
  ])("$title empties every field and the message, refocuses Acct, no request", async (row) => {
    const requests = stubApi([json(200, promptByAccount)]);
    renderScreen({ from: PATHS.COMEN01 });
    expect(keyKind("F4=Clear")).toEqual(["button", "outline", "sm"]);
    await reachPrompt();
    await waitFor(() => {
      expect(document.activeElement).toBe(field("confirm"));
    });
    typeInto({ confirm: "N" });

    if (row.byKey) {
      pressKey("F4");
    } else {
      fireEvent.click(keyButton("F4=Clear"));
    }

    await waitFor(() => {
      expect(shownValues()).toEqual(BLANK);
    });
    expect(messageRow()).toEqual({ kind: "error", text: "", red: true });
    await waitFor(() => {
      expect(document.activeElement).toBe(field("acctId"));
    });
    await settle();
    expect(requests).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Add Transaction");
  });

  // audit: COTRN02-069
  // audit: COTRN02C-004
  it.each([
    {
      title: "F3=Back after entry from the main menu returns to /menu",
      state: { from: PATHS.COMEN01 },
      byKey: false,
      target: PATHS.COMEN01,
    },
    {
      title: "the F3 key after entry from the main menu returns to /menu",
      state: { from: PATHS.COMEN01 },
      byKey: true,
      target: PATHS.COMEN01,
    },
    {
      title: "F3=Back with no recorded origin returns to /menu",
      state: undefined,
      byKey: false,
      target: PATHS.COMEN01,
    },
    {
      title: "F3=Back with an origin that is no route of the application returns to /menu",
      state: { from: "https://example.invalid/menu" },
      byKey: false,
      target: PATHS.COMEN01,
    },
    {
      title: "F3=Back returns to the screen that transferred here",
      state: { from: PATHS.COTRN00 },
      byKey: false,
      target: PATHS.COTRN00,
    },
  ])("$title, recording COTRN02 as the origin, with no request", async (row) => {
    const requests = stubApi([]);
    renderScreen(row.state);
    expect(keyKind("F3=Back")).toEqual(["button", "outline", "sm"]);
    typeInto(TYPED_BY_ACCOUNT);

    if (row.byKey) {
      pressKey("F3");
    } else {
      fireEvent.click(keyButton("F3=Back"));
    }

    const stub = await reachedStub();
    expect(stub.path).toBe(row.target);
    expect(stub.state).toEqual({ from: PATHS.COTRN02 });
    await settle();
    expect(requests).toHaveLength(0);
  });

  // audit: COTRN02C-006
  it.each([
    { title: "F7", key: "F7", shiftKey: false },
    { title: "F2", key: "F2", shiftKey: false },
    { title: "F12", key: "F12", shiftKey: false },
    { title: "Shift+F3, PF15, which COTRN02C does not fold onto PF3", key: "F3", shiftKey: true },
  ])("$title shows the invalid-key message and keeps every field, with no request", async (row) => {
    const requests = stubApi([]);
    renderScreen({ from: PATHS.COMEN01 });
    typeInto(TYPED_BY_ACCOUNT);

    pressKey(row.key, row.shiftKey);

    await waitFor(() => {
      expect(messageRow()).toEqual({ kind: "error", text: INVALID_KEY_TEXT, red: true });
    });
    expect(messageRow().text).toHaveLength(50);
    expect(shownValues()).toEqual({ ...BLANK, ...TYPED_BY_ACCOUNT });
    expect(screen.queryByRole("region", { name: "Route stub" })).toBeNull();
    await settle();
    expect(requests).toHaveLength(0);
  });

  it("ignores ENTER, F4, F5 and invalid keys while a request is outstanding", async () => {
    const held = heldReply();
    const requests = stubApi([held.reply]);
    const { container } = renderScreen({ from: PATHS.COMEN01 });
    typeInto(TYPED_BY_ACCOUNT);
    pressEnter();
    await waitFor(() => {
      expect(requests).toHaveLength(1);
    });
    await waitFor(() => {
      expect(formBusy(container)).toBe("true");
    });

    pressEnter();
    fireEvent.click(keyButton("F5=Copy Last Tran."));
    fireEvent.click(keyButton("F4=Clear"));
    pressKey("F7");
    await settle();
    expect(requests).toHaveLength(1);
    expect(shownValues()).toEqual({ ...BLANK, ...TYPED_BY_ACCOUNT });
    expect(messageRow().text).toBe("");

    held.release(json(200, promptByAccount));
    await waitFor(() => {
      expect(formBusy(container)).toBe("false");
    });
    await waitFor(() => {
      expect(shownValues()).toEqual(SHOWN_PROMPT);
    });
    expect(messageRow().text).toBe(PROMPT_TEXT);
    await settle();
    expect(requests).toHaveLength(1);
  });

  it("F3 still leaves while a request is outstanding", async () => {
    const held = heldReply();
    const requests = stubApi([held.reply]);
    renderScreen({ from: PATHS.COMEN01 });
    typeInto(TYPED_BY_ACCOUNT);
    pressEnter();
    await waitFor(() => {
      expect(requests).toHaveLength(1);
    });

    fireEvent.click(keyButton("F3=Back"));

    const stub = await reachedStub();
    expect(stub.path).toBe(PATHS.COMEN01);
    held.release(json(200, promptByAccount));
    await settle();
    expect(requests).toHaveLength(1);
  });
});
