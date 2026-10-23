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
import { clearSession, getSession, setSession } from "@/auth/session";
import { PATHS, routes } from "@/routes";
import { COTRN02 } from "./COTRN02";

/*
 * Render test of the Add Transaction screen (map COTRN2A, transaction CT02, program COTRN02C,
 * route `/transactions/add`) in jsdom, with `fetch` stubbed for endpoint 13,
 * `POST /api/v1/transactions`. Expected values are the audited results of
 * `modern/audit/screens.md` (`COTRN02-nnn`) and `modern/audit/online/COTRN02C.md`
 * (`COTRN02C-nnn`). Every card number a response carries is redacted; the one full-width card
 * number below is synthetic request input, and no password appears.
 *
 * The header values are the read-only `Input`s the shared `ScreenShell` renders in the
 * `CardHeader` title rows. Their label rows COTRN02-001, -004, -006 and -009 are proved by NAV
 * (`frontend/e2e/nav.spec.ts`) and by their placement here; the value rows COTRN02-002, -003,
 * -005, -007, -008 and -010 are tagged here, and so are their content rows (COTRN02-062 to
 * COTRN02-067), read from those inputs' values.
 */

const ENDPOINT = "/api/v1/transactions";

/**
 * The classes of the shared map-field box, FieldGrid's `FIELD_INPUT`: `min-w-max` and
 * `field-sizing-content` keep a box never narrower than its value, `px-1` is the one inset every
 * map input shares and `text-sm` the inputs' type size. Restated here because the render-test
 * contract imports no module beside the screen, the session, the routes and the generated types,
 * so a box is checked against this list rather than against the constant that renders it.
 */
const MAP_FIELD_BOX = ["min-w-max", "px-1", "text-sm", "field-sizing-content"] as const;

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

/** COTRN02C-009: ENTER or PF5 with the account and the card both blank. */
const keyMissing: components["schemas"]["ScreenError"] = {
  message: "Account or Card Number must be entered...",
  field: "acct_id",
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

/**
 * COTRN02 at `/transactions/add` with `state`, and stubs at the routes F3 can reach. Returns the
 * render result and the query client, whose pending requests {@link clientIdle} reads.
 */
function renderScreen(state?: unknown) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const rendered = render(
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
  return { ...rendered, client };
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

/**
 * The error state of the input of form member `name`: its `aria-invalid`, the text of the form
 * message its `aria-describedby` names (`null` when it names none that exists), and whether that
 * message is for assistive technology only.
 */
function fieldError(name: keyof typeof LABELS): {
  invalid: string | null;
  message: string | null;
  srOnly: boolean;
} {
  const input = field(name);
  const message = (input.getAttribute("aria-describedby") ?? "")
    .split(" ")
    .map((id) => document.getElementById(id))
    .find((element) => element?.dataset.slot === "form-message");
  return {
    invalid: input.getAttribute("aria-invalid"),
    message: message?.textContent ?? null,
    srOnly: message?.classList.contains("sr-only") ?? false,
  };
}

/** Every id an `aria-describedby` on the page names that resolves to no element. */
function danglingDescriptions(): string[] {
  return [...document.querySelectorAll("[aria-describedby]")].flatMap((element) =>
    (element.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .filter((id) => id === "" || document.getElementById(id) === null),
  );
}

/**
 * How assistive technology reads the input of form member `name`: its `aria-invalid`, and the
 * element each id of its `aria-describedby` names, as its text and whether `sr-only` hides it
 * (`null` for an id that resolves to nothing). `described` is `null` when the input has no
 * `aria-describedby`.
 */
function readOut(name: keyof typeof LABELS): {
  invalid: string | null;
  described: ({ text: string | null; hidden: boolean } | null)[] | null;
} {
  const input = field(name);
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

/** The form members whose input is marked invalid, in map order. */
function markedInputs(): string[] {
  return NAMES.filter((name) => field(name).getAttribute("aria-invalid") === "true");
}

/** Every element that shows exactly `text` and is not hidden by `sr-only`. */
function visibleCopies(text: string): HTMLElement[] {
  return screen
    .queryAllByText(text, { normalizer: (shown) => shown })
    .filter((element) => element.closest(".sr-only") === null);
}

/** `aria-busy` of the screen's form: `"true"` while a request is outstanding. */
function formBusy(container: HTMLElement): string | null {
  return container.querySelector("form")?.getAttribute("aria-busy") ?? null;
}

/** The row-24 labels, in map order. */
const KEY_LABELS = ["ENTER=Continue", "F3=Back", "F4=Clear", "F5=Copy Last Tran."];

/** Whether the function-key button labelled `label` is disabled (`aria-disabled="true"`). */
function keyDisabled(label: string): boolean {
  const button = screen.getByRole("button", { name: label });
  if (!(button instanceof HTMLButtonElement)) {
    throw new Error(`"${label}" names <${button.tagName.toLowerCase()}>, not a button`);
  }
  return button.getAttribute("aria-disabled") === "true";
}

/**
 * Whether the screen shows itself waiting for an answer: `aria-busy` of the "Function keys" group
 * and of the form, whether each row-24 button is `aria-disabled`, by label, and whether each
 * input is read-only, by form member.
 */
function lockState() {
  return {
    keyRowBusy: screen.getByRole("group", { name: "Function keys" }).getAttribute("aria-busy"),
    formBusy: document.querySelector("form")?.getAttribute("aria-busy") ?? null,
    disabled: Object.fromEntries(KEY_LABELS.map((label) => [label, keyDisabled(label)])),
    readOnly: Object.fromEntries(NAMES.map((name) => [name, field(name).readOnly])),
  };
}

/** {@link lockState} while a request is outstanding: ENTER, F4 and F5 refused, F3 available. */
const LOCKED = {
  keyRowBusy: "true",
  formBusy: "true",
  disabled: {
    "ENTER=Continue": true,
    "F3=Back": false,
    "F4=Clear": true,
    "F5=Copy Last Tran.": true,
  },
  readOnly: Object.fromEntries(NAMES.map((name) => [name, true])),
};

/** {@link lockState} with no request outstanding: every key and input available. */
const UNLOCKED = {
  keyRowBusy: "false",
  formBusy: "false",
  disabled: {
    "ENTER=Continue": false,
    "F3=Back": false,
    "F4=Clear": false,
    "F5=Copy Last Tran.": false,
  },
  readOnly: Object.fromEntries(NAMES.map((name) => [name, false])),
};

/**
 * The value of the header field `name` (`data-field` of `ScreenShell`): the one read-only `Input`
 * of that name in the `CardHeader`, failing when there is not exactly one.
 */
function headerField(container: HTMLElement, name: string): string {
  const inputs = container.querySelectorAll(
    `[data-slot="card-header"] input[data-field="${name}"]`,
  );
  expect(inputs).toHaveLength(1);
  const [input] = inputs;
  if (!(input instanceof HTMLInputElement)) {
    throw new Error(`The header has no input ${name}`);
  }
  return input.value;
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
 * Waits until `client` fetches no query and has exactly `held` mutations pending: the replies the
 * test holds back, none by default. Endpoint 13 is a mutation of that client. It stays pending
 * from the moment the screen starts it until its reply has arrived and, while the screen is
 * mounted, has been passed to the screen's `onSuccess` or `onError`. Every request the screen had
 * started when this is called, the held ones aside, has therefore been answered once it returns.
 * It cannot see a request the screen has not started yet: a test calls it once it has awaited
 * what the screen shows at the end of a key, or after {@link idleAfter50ms}.
 */
async function clientIdle(client: QueryClient, held = 0): Promise<void> {
  await waitFor(() => {
    expect(client.isFetching()).toBe(0);
    expect(client.isMutating()).toBe(held);
  });
}

/**
 * For keys that must send nothing and leave nothing on screen to wait for, such as the first
 * entry or the keys the lock refuses: lets 50 ms pass inside `act`, so whatever the screen does
 * within that window (a validation, a render, a request) runs and renders first, then waits as
 * {@link clientIdle} does. It shows only that nothing was sent within those 50 ms, not that
 * nothing would be sent later.
 */
async function idleAfter50ms(client: QueryClient, held = 0): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => {
      setTimeout(resolve, 50);
    });
  });
  await clientIdle(client, held);
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
    const { client } = renderScreen({ from: PATHS.COMEN01 });

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Add Transaction");

    for (const mapInput of MAP_INPUTS) {
      const box = field(mapInput.name);
      const label = screen.getByText(LABELS[mapInput.name], { normalizer: (shown) => shown });
      expect(label.tagName).toBe("LABEL");
      expect(label.getAttribute("for")).toBe(box.id);
      expect(label.parentElement?.className).toBe(cellClass(mapInput.label));
      expect(box.parentElement?.className).toBe(cellClass(mapInput.input));
      expect(box.maxLength).toBe(mapInput.input[2]);
      // The shared map-field box, `MAP_FIELD_BOX` with `size` = the LENGTH, so a full-length value
      // shows on the narrowest frame.
      expect(box.size).toBe(mapInput.input[2]);
      expect(MAP_FIELD_BOX.filter((name) => !box.classList.contains(name))).toEqual([]);
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
    await idleAfter50ms(client);
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

  // The values these fields show are proved under COTRN02-062 to COTRN02-067.
  it.each([
    // audit: COTRN02-002
    {
      id: "COTRN02-002",
      name: "TRNNAME",
      row: 1,
      col: 7,
      length: 4,
      label: "Tran:",
      shows: "CT02",
    },
    // audit: COTRN02-003
    {
      id: "COTRN02-003",
      name: "TITLE01",
      row: 1,
      col: 21,
      length: 40,
      label: null,
      shows: "      AWS Mainframe Modernization       ",
    },
    // audit: COTRN02-005
    {
      id: "COTRN02-005",
      name: "CURDATE",
      row: 1,
      col: 71,
      length: 8,
      label: "Date:",
      shows: expect.stringMatching(/^\d{2}\/\d{2}\/\d{2}$/),
    },
    // audit: COTRN02-007
    {
      id: "COTRN02-007",
      name: "PGMNAME",
      row: 2,
      col: 7,
      length: 8,
      label: "Prog:",
      shows: "COTRN02C",
    },
    // audit: COTRN02-008
    {
      id: "COTRN02-008",
      name: "TITLE02",
      row: 2,
      col: 21,
      length: 40,
      label: null,
      shows: "              CardDemo                  ",
    },
    // audit: COTRN02-010
    {
      id: "COTRN02-010",
      name: "CURTIME",
      row: 2,
      col: 71,
      length: 8,
      label: "Time:",
      shows: expect.stringMatching(/^\d{2}:\d{2}:\d{2}$/),
    },
  ])(
    "$id: header value $name is a read-only Input of the CardHeader title rows at its map POS and LENGTH",
    ({ name, row, col, length, label, shows }) => {
      stubApi([]);
      const { container } = renderScreen({ from: PATHS.COMEN01 });

      const header = container.querySelector('[data-slot="card-header"]');
      if (!(header instanceof HTMLElement)) {
        throw new Error("COTRN2A renders no card header");
      }
      // The field's one element stands in the title rows, not in the body or the footer.
      const shown = document.querySelectorAll(`[data-field="${name}"]`);
      expect(shown).toHaveLength(1);
      const value = shown.item(0);
      expect(header.contains(value)).toBe(true);
      expect(value.closest('[data-slot="card-content"], [data-slot="card-footer"]')).toBeNull();
      // It is an output field of the map: a read-only `Input` as wide as its LENGTH (`size`), in
      // the cell of its POS and LENGTH, holding LENGTH characters, the padding that centres the
      // titles included.
      if (!(value instanceof HTMLInputElement)) {
        throw new Error(`The header field ${name} is not an input`);
      }
      expect(value.dataset.slot).toBe("input");
      expect(value.readOnly).toBe(true);
      expect(value.size).toBe(length);
      expect(value.parentElement?.className).toBe(cellClass([row, col, length]));
      expect(value.value).toEqual(shows);
      expect(value.value).toHaveLength(length);
      // The map literal before it is its bound `Label`; a title, which no literal labels, is named
      // by `aria-label`.
      const labels = Array.from(value.labels ?? []);
      expect(labels.map((bound) => bound.textContent)).toEqual(label === null ? [] : [label]);
      expect(labels.every((bound) => bound.dataset.slot === "label")).toBe(true);
      expect(value.getAttribute("aria-label")).toBe(label === null ? `Title row ${row}` : null);
      // The title rows hold no editable control: their six controls are the read-only values.
      const controls = Array.from(
        header.querySelectorAll("input, textarea, select, button, [contenteditable]"),
      );
      expect(controls).toHaveLength(6);
      expect(
        controls.every((control) => control instanceof HTMLInputElement && control.readOnly),
      ).toBe(true);
    },
  );

  // audit: COTRN02-060
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
    // ERRMSG is `BRT` whatever its colour, so the green success text is semibold.
    const description = screen.getByRole("alert").querySelector('[data-slot="alert-description"]');
    expect(description?.classList.contains("font-semibold")).toBe(true);
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
      marked: [["acctId", "String must contain at most 11 character(s)"]],
    },
    {
      title: "F5 with a card number longer than its 16 columns",
      key: "F5=Copy Last Tran.",
      typed: { cardNum: `${TYPED_CARD}0` },
      message: "String must contain at most 16 character(s)",
      focus: "cardNum",
      marked: [["cardNum", "String must contain at most 16 character(s)"]],
    },
    {
      title: "ENTER with two data fields too long, the first in map order",
      key: "ENTER=Continue",
      typed: { ...TYPED_BY_ACCOUNT, description: "D".repeat(61), merchantZip: "Z".repeat(11) },
      message: "String must contain at most 60 character(s)",
      focus: "description",
      marked: [
        ["description", "String must contain at most 60 character(s)"],
        ["merchantZip", "String must contain at most 10 character(s)"],
      ],
    },
  ] as const)("$title shows the map-length message and sends nothing", async (row) => {
    const requests = stubApi([]);
    const { client } = renderScreen({ from: PATHS.COMEN01 });
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
    // Each input the schema rejects is invalid and names a form message with its own text, which
    // only assistive technology reads: the message row stays the one visible copy.
    const marked = new Map<string, string>(row.marked);
    for (const name of NAMES) {
      const message = marked.get(name);
      expect(fieldError(name)).toEqual(
        message === undefined
          ? { invalid: "false", message: null, srOnly: false }
          : { invalid: "true", message, srOnly: true },
      );
    }
    expect(
      visibleCopies(row.message).map((copy) => copy.closest("[role='alert']") !== null),
    ).toEqual([true]);
    await clientIdle(client);
    expect(requests).toHaveLength(0);
    // The refusal releases the keyboard the key took.
    expect(lockState()).toEqual(UNLOCKED);
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
  ] as const)("$title shows its message, keeps every field, marks its input", async (row) => {
    const requests = stubApi([row.reply]);
    const { client } = renderScreen({ from: PATHS.COMEN01 });
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
    await clientIdle(client);
    expect(document.activeElement).toBe(row.focus === undefined ? enter : field(row.focus));
    // The named input, by its bare or `fields.` wire name, is invalid and names a form message
    // with the row's text for assistive technology only; with no field named, none is marked.
    for (const name of NAMES) {
      expect(fieldError(name)).toEqual(
        name === row.focus
          ? { invalid: "true", message: row.message, srOnly: true }
          : { invalid: "false", message: null, srOnly: false },
      );
    }
    expect(
      visibleCopies(row.message).map((copy) => copy.closest("[role='alert']") !== null),
    ).toEqual([true]);
    expect(requests).toHaveLength(1);
  });

  // The replacement's accessibility wiring, which no audit row states: every `aria-describedby`
  // names only elements the screen renders. The four inputs the map gives a hint are described by
  // it, the ten others by nothing while they are valid, and the input a `ScreenError` names also
  // by its field message, which holds the server's text, hidden.
  it.each([
    { state: "on entry", typed: null, error: null, marked: null },
    {
      state: "after ENTER on the blank screen, answered by the 400 naming acct_id",
      typed: BLANK,
      error: keyMissing,
      marked: "acctId",
    },
    {
      state: "after the 400 naming amount",
      typed: { ...TYPED_BY_ACCOUNT, amount: "12.34" },
      error: amountFormat,
      marked: "amount",
    },
  ])("describes each input by rendered elements only, $state", async (row) => {
    stubApi(row.error === null ? [] : [json(400, row.error)]);
    const { client } = renderScreen({ from: PATHS.COMEN01 });
    if (row.typed === null || row.error === null) {
      // On entry nothing is pressed, so nothing on screen marks the end of a wait.
      await idleAfter50ms(client);
    } else {
      typeInto(row.typed);
      fireEvent.click(keyButton("ENTER=Continue"));
    }

    const expected = Object.fromEntries(
      NAMES.map((name) => {
        const hint = HINTS.find((candidate) => candidate.name === name);
        const own = name === row.marked && row.error !== null;
        const described = [
          ...(hint === undefined ? [] : [{ text: hint.text, hidden: false }]),
          ...(own ? [{ text: row.error?.message ?? "", hidden: true }] : []),
        ];
        return [
          name,
          { invalid: own ? "true" : "false", described: described.length > 0 ? described : null },
        ];
      }),
    );
    await waitFor(() => {
      expect(messageRow().text).toBe(row.error?.message ?? "");
      expect(Object.fromEntries(NAMES.map((name) => [name, readOut(name)]))).toEqual(expected);
    });
    expect(danglingDescriptions()).toEqual([]);
  });


  it.each([
    {
      title: "typing in it, after ENTER",
      typed: { ...TYPED_BY_ACCOUNT, acctId: "" },
      rejection: json(400, keyMissing),
      message: keyMissing.message,
      via: "ENTER=Continue",
      then: "type",
      answer: undefined,
    },
    {
      // F5 runs no resolver, so only the correction itself can clear the mark.
      title: "typing in it, after F5",
      typed: {},
      rejection: json(400, keyMissing),
      message: keyMissing.message,
      via: "F5=Copy Last Tran.",
      then: "type",
      answer: undefined,
    },
    {
      title: "the next ENTER as soon as it is sent, the add that answers it included",
      typed: { ...TYPED_BY_ACCOUNT, confirm: "Y" },
      rejection: json(500, addFailed),
      message: addFailed.message,
      via: "ENTER=Continue",
      then: "ENTER=Continue",
      answer: added,
    },
    {
      title: "an F5 as soon as it is sent, the prompt that answers it included",
      typed: { ...TYPED_BY_ACCOUNT, confirm: "Y" },
      rejection: json(500, addFailed),
      message: addFailed.message,
      via: "ENTER=Continue",
      then: "F5=Copy Last Tran.",
      answer: promptCopied,
    },
    {
      title: "F4=Clear",
      typed: { ...TYPED_BY_ACCOUNT, acctId: "" },
      rejection: json(400, keyMissing),
      message: keyMissing.message,
      via: "ENTER=Continue",
      then: "F4=Clear",
      answer: undefined,
    },
  ] as const)("a server-marked input is unmarked by $title", async (row) => {
    const held = heldReply();
    const requests = stubApi([row.rejection, held.reply]);
    const { container, client } = renderScreen({ from: PATHS.COMEN01 });
    typeInto(row.typed);
    fireEvent.click(keyButton(row.via));
    await waitFor(() => {
      expect(fieldError("acctId")).toEqual({
        invalid: "true",
        message: row.message,
        srOnly: true,
      });
    });
    expect(markedInputs()).toEqual(["acctId"]);

    if (row.then === "type") {
      typeInto({ acctId: ACCOUNT });
    } else {
      fireEvent.click(keyButton(row.then));
    }

    const unmarked = { invalid: "false", message: null, srOnly: false };
    await waitFor(() => {
      expect(fieldError("acctId")).toEqual(unmarked);
    });
    expect(markedInputs()).toEqual([]);
    const { answer } = row;
    if (answer === undefined) {
      await clientIdle(client);
      expect(requests).toHaveLength(1);
      return;
    }
    // The send itself clears the mark: its answer is still held.
    await waitFor(() => {
      expect(requests).toHaveLength(2);
    });
    await waitFor(() => {
      expect(formBusy(container)).toBe("true");
    });
    expect(markedInputs()).toEqual([]);
    held.release(json(200, answer));
    await waitFor(() => {
      expect(messageRow().text).toBe(answer.message);
    });
    // Neither the add nor the confirmation prompt marks an input: the prompt rejects no value.
    expect(markedInputs()).toEqual([]);
    expect(fieldError("confirm")).toEqual(unmarked);
  });

  // audit: COTRN02-070
  // audit: COTRN02C-005
  it.each([
    { title: "the F4=Clear button", byKey: false },
    { title: "the F4 key", byKey: true },
  ])("$title empties every field and the message, refocuses Acct, no request", async (row) => {
    const requests = stubApi([json(200, promptByAccount)]);
    const { client } = renderScreen({ from: PATHS.COMEN01 });
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
    await clientIdle(client);
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
    const { client } = renderScreen(row.state);
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
    await clientIdle(client);
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
    const { client } = renderScreen({ from: PATHS.COMEN01 });
    typeInto(TYPED_BY_ACCOUNT);

    pressKey(row.key, row.shiftKey);

    await waitFor(() => {
      expect(messageRow()).toEqual({ kind: "error", text: INVALID_KEY_TEXT, red: true });
    });
    expect(messageRow().text).toHaveLength(50);
    expect(shownValues()).toEqual({ ...BLANK, ...TYPED_BY_ACCOUNT });
    expect(screen.queryByRole("region", { name: "Route stub" })).toBeNull();
    await clientIdle(client);
    expect(requests).toHaveLength(0);
  });

  it.each([
    { title: "an ENTER sent by its button", start: pressEnter, aid: "ENTER" },
    {
      title: "an F5 sent by its key",
      start: () => {
        pressKey("F5");
      },
      aid: "PF5",
    },
  ])(
    "ignores ENTER, F4, F5 and invalid keys while $title is outstanding, then takes the next ENTER",
    async (row) => {
      const held = heldReply();
      const requests = stubApi([held.reply, json(200, promptByAccount)]);
      const { container, client } = renderScreen({ from: PATHS.COMEN01 });
      typeInto(TYPED_BY_ACCOUNT);
      row.start();
      await waitFor(() => {
        expect(requests).toHaveLength(1);
        expect(lockState()).toEqual(LOCKED);
      });
      expect(requests[0]?.body).toMatchObject({ aid: row.aid });
      const form = container.querySelector("form");
      if (form === null) {
        throw new Error("no form");
      }

      pressEnter();
      pressEnter();
      fireEvent.keyDown(document.body, { key: "Enter" });
      fireEvent.submit(form);
      fireEvent.submit(form);
      fireEvent.click(keyButton("F5=Copy Last Tran."));
      pressKey("F5");
      fireEvent.click(keyButton("F4=Clear"));
      pressKey("F4");
      pressKey("F7");
      // The held reply stays the one request pending.
      await idleAfter50ms(client, 1);
      expect(requests).toHaveLength(1);
      expect(shownValues()).toEqual({ ...BLANK, ...TYPED_BY_ACCOUNT });
      expect(messageRow().text).toBe("");
      expect(lockState()).toEqual(LOCKED);

      held.release(json(200, promptByAccount));
      await waitFor(() => {
        expect(formBusy(container)).toBe("false");
        expect(lockState()).toEqual(UNLOCKED);
      });
      await waitFor(() => {
        expect(shownValues()).toEqual(SHOWN_PROMPT);
      });
      expect(messageRow().text).toBe(PROMPT_TEXT);
      await clientIdle(client);
      expect(requests).toHaveLength(1);

      // Answered, the keyboard takes the next ENTER.
      pressEnter();
      await waitFor(() => {
        expect(requests).toHaveLength(2);
      });
      await waitFor(() => {
        expect(lockState()).toEqual(UNLOCKED);
        expect(shownValues()).toEqual(SHOWN_PROMPT);
      });
      expect(requests[1]?.body).toMatchObject({ aid: "ENTER" });
    },
  );

  it("takes one key of several pressed before the screen renders again", async () => {
    const held = heldReply();
    const requests = stubApi([held.reply]);
    const { container, client } = renderScreen({ from: PATHS.COMEN01 });
    const form = container.querySelector("form");
    if (form === null) {
      throw new Error("no form");
    }
    typeInto(TYPED_BY_ACCOUNT);

    // One batch: React renders the lock only after all five, so only the screen's ref stops them.
    act(() => {
      const enter = keyButton("ENTER=Continue");
      enter.click();
      enter.click();
      form.requestSubmit();
      document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "F5", bubbles: true }));
    });
    await waitFor(() => {
      expect(requests).toHaveLength(1);
    });
    expect(lockState()).toEqual(LOCKED);
    // The held reply of the one key taken stays the one request pending.
    await idleAfter50ms(client, 1);
    expect(requests).toHaveLength(1);
    expect(requests[0]?.body).toMatchObject({ aid: "ENTER" });

    held.release(json(200, promptByAccount));
    await waitFor(() => {
      expect(shownValues()).toEqual(SHOWN_PROMPT);
      expect(lockState()).toEqual(UNLOCKED);
    });
  });

  it("F3 still leaves while a request is outstanding", async () => {
    const held = heldReply();
    const requests = stubApi([held.reply]);
    const { client } = renderScreen({ from: PATHS.COMEN01 });
    typeInto(TYPED_BY_ACCOUNT);
    pressEnter();
    await waitFor(() => {
      expect(requests).toHaveLength(1);
    });

    fireEvent.click(keyButton("F3=Back"));

    const stub = await reachedStub();
    expect(stub.path).toBe(PATHS.COMEN01);
    held.release(json(200, promptByAccount));
    // The count waits for the released reply's answer.
    await clientIdle(client);
    expect(requests).toHaveLength(1);
  });
});

// Endpoint 13 takes the transaction in its JSON body alone, and every request carries the stored
// session's token.
describe("COTRN02: endpoint 13 request", () => {
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

  it("ENTER POSTs the typed fields with the bearer token and no query, and shows the prompt", async () => {
    stubApi([json(200, promptByAccount)]);
    renderScreen({ from: PATHS.COMEN01 });

    await reachPrompt();

    expect(await sentRequests()).toEqual([
      {
        method: "POST",
        path: ENDPOINT,
        query: {},
        authorization: "Bearer test-token",
        body: enterByAccount,
      },
    ]);
  });
});

/** The 401 of the `SignedOnUser` extractor for a missing, invalid or expired token. */
const signonRequired: components["schemas"]["ScreenError"] = {
  message: "Sign-on required. Please sign on ...",
  field: null,
};

/**
 * Renders the application's route table at `/transactions/add`, so the sign-on guard that only
 * the table mounts stands in front of the screen. Each screen is imported by its route's loader, so
 * a wait for one to render allows 5 s. Returns the router, whose location the test reads.
 */
function renderRouteTable(): ReturnType<typeof createMemoryRouter> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(routes, { initialEntries: [PATHS.COTRN02] });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

// A 401 clears the stored session (`api/client.ts`), and the sign-on guard in front of the route
// replaces the still-mounted screen with sign-on at once: after the ENTER that sent the refused
// request, the test presses no key and follows no link.
describe("COTRN02 in the route table: a 401 ends the session", () => {
  it.each([
    {
      title: "the validation of ENTER",
      replies: [json(401, signonRequired)],
      drive: async () => {
        typeInto(TYPED_BY_ACCOUNT);
        pressEnter();
      },
      count: 1,
    },
    {
      title: "the add of the confirmed ENTER",
      replies: [json(200, promptByAccount), json(401, signonRequired)],
      drive: async () => {
        await reachPrompt();
        await waitFor(() => {
          expect(document.querySelector("form")?.getAttribute("aria-busy")).toBe("false");
        });
        typeInto({ confirm: "Y" });
        pressEnter();
      },
      count: 2,
    },
  ])(
    "$title answered 401 replaces the screen with sign-on and clears the session, with no retry",
    async ({ replies, drive, count }) => {
      const sent = stubApi(replies);
      const router = renderRouteTable();
      expect(
        await screen.findByRole(
          "heading",
          { level: 1, name: "Add Transaction" },
          { timeout: 5_000 },
        ),
      ).toBeDefined();

      await drive();

      expect(
        await screen.findByRole("button", { name: "ENTER=Sign-on" }, { timeout: 5_000 }),
      ).toBeDefined();
      expect(router.state.location.pathname).toBe(PATHS.COSGN00);
      expect(screen.queryByRole("heading", { level: 1, name: "Add Transaction" })).toBeNull();
      expect(getSession()).toBeNull();
      expect(sent).toHaveLength(count);
      for (const request of sent) {
        expect([request.method, request.path, request.authorization]).toEqual([
          "POST",
          ENDPOINT,
          "Bearer test-token",
        ]);
      }
      router.dispose();
    },
  );
});

// The sign-on guard lives in the application's route tree, not in the screen, so this group
// renders that tree rather than the screen beside stubs.
describe("COTRN02 behind the route guard", () => {
  // audit: COTRN02C-001
  it("sends a browser with no session from /transactions/add to /signon, with no request", async () => {
    clearSession();
    const requests = stubApi([]);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const router = createMemoryRouter(routes, { initialEntries: [PATHS.COTRN02] });
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );

    // Only the sign-on screen loads, through the route tree; the refused screen is never imported.
    await waitFor(
      () => {
        expect(router.state.location.pathname).toBe(PATHS.COSGN00);
        expect(screen.getByRole("button", { name: "ENTER=Sign-on" })).toBeDefined();
      },
      { timeout: 4000 },
    );
    expect(screen.queryByRole("heading", { level: 1, name: "Add Transaction" })).toBeNull();
    expect(screen.queryByLabelText(LABELS.acctId)).toBeNull();
    // Gives a request the guarded screen would send time to leave before the count.
    await idleAfter50ms(client);
    expect(requests).toHaveLength(0);
    router.dispose();
  });
});
