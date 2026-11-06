// FN test of the admin reward-rule form (RewardRuleFormPage) in new mode: valid
// input is sent as the RewardRuleRequest body of `POST /admin/reward-rules`,
// input the schema rejects is never sent, and a server 400 lands on its field
// with a toast and the cursor. In edit mode, the `PUT /admin/reward-rules/{id}`
// body keeps each stored window instant whose date was left unchanged. In both
// modes one Save sends one request, a double click included; Save keeps the
// focus while it runs and after a rejection that names no field; a saved rule
// leaves the form inert until the list route has loaded; Reset restores the
// starting values and clears every error, and Back, reached by Tab after Save
// and Reset, returns to the list; neither writes. An edit route whose rule list
// fails to load, or lacks the rule, shows the message in an alert with only
// Back, never an editable form. An edit route opened over a failed read the
// rule list already holds reports only its own read of the list: no toast and
// no form from rules read earlier while that read is out, then its answer.
// The page runs with the real API client, query hooks and error helper; only
// the network is stubbed, through the global the client looks up on every
// request.
import type { JSX } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import {
  createMemoryRouter,
  RouterProvider,
  type NonIndexRouteObject,
} from "react-router";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useRewardRules } from "@/api/queries/reward-rules";
import type { components } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import { UNEXPECTED_ERROR } from "@/lib/api-errors";
import {
  REWARD_RULE_FORM_MESSAGES,
  REWARD_RULE_MESSAGES,
  rewardRuleSchema,
  type RewardRuleFormInput,
} from "@/schemas/reward-rules";

import RewardRuleFormPage from "./RewardRuleFormPage";

type RewardRuleRequest = components["schemas"]["RewardRuleRequest"];
type RewardRuleResponse = components["schemas"]["RewardRuleResponse"];
type ErrorBody = components["schemas"]["ErrorBody"];

type FieldName = keyof RewardRuleFormInput;

// A value for every form field, as typed; `""` leaves the field blank.
type FormEntry = Record<FieldName, string>;

const NEW_ROUTE = "/admin/reward-rules/new";
const EDIT_ROUTE = "/admin/reward-rules/:ruleId/edit";
const LIST_ROUTE = "/admin/reward-rules";

// What the list route renders in place of RewardRuleListPage, so the page's
// return to the list after a save is visible.
const LIST_MARKER = "Reward rules list";

// Every form field and its label, in screen and tab order.
const FIELDS: ReadonlyArray<readonly [FieldName, string]> = [
  ["transaction_type_cd", "Transaction type code"],
  ["transaction_category_cd", "Transaction category code"],
  ["reward_rate", "Reward rate"],
  ["min_transaction_amount", "Minimum transaction amount"],
  ["max_transaction_amount", "Maximum transaction amount"],
  ["active_from", "Active from"],
  ["active_to", "Active to"],
];

// A rule for the fixture type `01` Purchase and category `0001` Regular Sales
// Draft [app/data/ASCII/trantype.txt:L1] [app/data/ASCII/trancatg.txt:L1],
// with a lower amount bound only and an open-ended window.
const VALID_INPUT: FormEntry = {
  transaction_type_cd: "01",
  transaction_category_cd: "0001",
  reward_rate: "1.5000",
  min_transaction_amount: "10.00",
  max_transaction_amount: "",
  active_from: "2022-01-01",
  active_to: "",
};

// The `format: date-time` of RewardRuleRequest: an RFC 3339 date-time.
const RFC3339_DATE_TIME =
  /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$/;

const fetchMock =
  vi.fn<
    (input: Request | string | URL, init?: RequestInit) => Promise<Response>
  >();

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  fetchMock.mockReset();
});

// The page alone at its own routes, beside a marker list route, opened at
// `initialEntry`, with a fresh query cache and router per test so no state
// carries over between cases. `list` replaces the marker route's element, for
// a list that loads lazily as the page's chunk does.
function renderPage(
  initialEntry: string = NEW_ROUTE,
  list: Pick<NonIndexRouteObject, "element" | "lazy"> = {
    element: <div>{LIST_MARKER}</div>,
  },
): ReturnType<typeof createMemoryRouter> {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(
    [
      {
        path: NEW_ROUTE,
        element: <RewardRuleFormPage />,
        handle: { crumb: "New rule" } satisfies RouteHandle,
      },
      {
        path: EDIT_ROUTE,
        element: <RewardRuleFormPage />,
        handle: { crumb: "Edit rule" } satisfies RouteHandle,
      },
      { path: LIST_ROUTE, ...list },
    ],
    { initialEntries: [initialEntry] },
  );
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

// A promise the test settles when it chooses, to hold an answer or a load.
function hold(): { held: Promise<void>; release: () => void } {
  let release = (): void => undefined;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { held, release };
}

function button(name: string): HTMLElement {
  return screen.getByRole("button", { name });
}

function textbox(label: string): HTMLElement {
  return screen.getByRole("textbox", { name: label });
}

// Types each non-blank value into its empty field, in tab order.
async function fillForm(user: UserEvent, entry: FormEntry): Promise<void> {
  for (const [name, label] of FIELDS) {
    const value = entry[name];
    if (value !== "") {
      await user.type(textbox(label), value);
    }
  }
}

async function save(user: UserEvent): Promise<void> {
  await user.click(screen.getByRole("button", { name: "Save" }));
}

// The request the client handed to the network last.
function lastRequest(): Request {
  const call = fetchMock.mock.calls.at(-1);
  if (call === undefined) {
    throw new Error("no request reached the network");
  }
  const [input, init] = call;
  return input instanceof Request ? input : new Request(input, init);
}

// The text of the field error that describes `input` (its FieldError, whose id
// the input lists in aria-describedby).
function fieldErrorText(input: HTMLElement): string {
  const describedBy = (input.getAttribute("aria-describedby") ?? "").split(" ");
  const errors = screen
    .getAllByRole("alert")
    .filter((alert) => describedBy.includes(alert.id));
  expect(errors).toHaveLength(1);
  return errors[0].textContent ?? "";
}

describe("RewardRuleFormPage (new rule)", () => {
  it("posts valid input as the RewardRuleRequest body and returns to the list", async () => {
    const stored: RewardRuleResponse = {
      rule_id: "3",
      transaction_type_cd: "01",
      transaction_category_cd: "0001",
      reward_rate: "1.5000",
      min_transaction_amount: "10.00",
      max_transaction_amount: null,
      active_from: "2022-01-01T00:00:00Z",
      active_to: null,
      status: "active",
    };
    fetchMock.mockResolvedValueOnce(Response.json(stored, { status: 201 }));
    const user = userEvent.setup();
    renderPage();

    await fillForm(user, VALID_INPUT);
    await save(user);

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledTimes(1);
    });
    const request = lastRequest();
    expect(request.method).toBe("POST");
    expect(new URL(request.url).pathname).toBe("/api/admin/reward-rules");
    const body: RewardRuleRequest = await request.clone().json();
    expect(body).toEqual(rewardRuleSchema.parse(VALID_INPUT));
    // Decimals travel as the strings typed, never as JSON numbers.
    expect(typeof body.reward_rate).toBe("string");
    expect(body.reward_rate).toBe("1.5000");
    expect(body.min_transaction_amount).toBe("10.00");
    expect(body.active_from).toBe("2022-01-01T00:00:00Z");
    expect(body.active_from).toMatch(RFC3339_DATE_TIME);
    // The blank bound and end are omitted, which the server reads as null.
    expect(Object.hasOwn(body, "max_transaction_amount")).toBe(false);
    expect(Object.hasOwn(body, "active_to")).toBe(false);

    expect(await screen.findByText(LIST_MARKER)).toBeTruthy();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("rejects a rate the schema refuses and sends nothing", async () => {
    const user = userEvent.setup();
    renderPage();

    await fillForm(user, { ...VALID_INPUT, reward_rate: "abc" });
    await save(user);

    const rate = textbox("Reward rate");
    await waitFor(() => {
      expect(rate.getAttribute("aria-invalid")).toBe("true");
    });
    expect(fieldErrorText(rate).trim()).not.toBe("");
    expect(screen.getAllByRole("alert")).toHaveLength(1);
    expect(fetchMock.mock.calls).toHaveLength(0);
    expect(screen.queryByText(LIST_MARKER)).toBeNull();
  });

  // Beyond S9(09)V99 the server's 400 names no field, so the schema refuses
  // the bound and shows the refusal on the field itself.
  it.each<[FieldName, string, string, string]>([
    [
      "max_transaction_amount",
      "Maximum transaction amount",
      "1000000000.00",
      REWARD_RULE_FORM_MESSAGES.maxAmountInvalid,
    ],
    [
      "min_transaction_amount",
      "Minimum transaction amount",
      "-1000000000.00",
      REWARD_RULE_FORM_MESSAGES.minAmountInvalid,
    ],
  ])(
    "rejects an out-of-range %s on its field, moves the cursor there and sends nothing",
    async (field, label, value, message) => {
      const user = userEvent.setup();
      renderPage();

      await fillForm(user, { ...VALID_INPUT, [field]: value });
      await save(user);

      const bound = textbox(label);
      await waitFor(() => {
        expect(document.activeElement).toBe(bound);
      });
      expect(bound.getAttribute("aria-invalid")).toBe("true");
      expect(fieldErrorText(bound)).toBe(message);
      expect(screen.getAllByRole("alert")).toHaveLength(1);
      expect(fetchMock.mock.calls).toHaveLength(0);
    },
  );

  it("places a server 400 on its field, toasts it and moves the cursor there", async () => {
    const message = REWARD_RULE_MESSAGES.rateNegative;
    const rejected: ErrorBody = {
      code: "INVALID",
      message,
      fields: [{ field: "reward_rate", message }],
    };
    fetchMock.mockResolvedValueOnce(Response.json(rejected, { status: 400 }));
    const toastError = vi.spyOn(toast, "error");
    const user = userEvent.setup();
    renderPage();

    await fillForm(user, VALID_INPUT);
    await save(user);

    const rate = textbox("Reward rate");
    // Focus moves last, after the toast and the field error.
    await waitFor(() => {
      expect(document.activeElement).toBe(rate);
    });
    expect(toastError).toHaveBeenCalledTimes(1);
    expect(toastError).toHaveBeenCalledWith(message);
    expect(rate.getAttribute("aria-invalid")).toBe("true");
    expect(fieldErrorText(rate)).toBe(message);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(LIST_MARKER)).toBeNull();
    expect(screen.getByRole("form", { name: "New reward rule" })).toBeTruthy();
  });

  // Two clicks before the page re-renders: a second POST would create the
  // rule twice.
  it("sends one POST for a double click on Save", async () => {
    fetchMock.mockImplementation(async () =>
      Response.json(TIMED_RULE, { status: 201 }),
    );
    const user = userEvent.setup();
    renderPage();
    await fillForm(user, VALID_INPUT);

    fireEvent.click(button("Save"));
    fireEvent.click(button("Save"));

    expect(await screen.findByText(LIST_MARKER)).toBeTruthy();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  // applyApiError places no field for such an answer and leaves the focus
  // where it is: on Save, which stayed focusable while the request ran.
  it("keeps Save focused through a rejection that names no field", async () => {
    const message = "Stub: a rejection naming no field";
    const answer = hold();
    fetchMock.mockImplementation(async () => {
      await answer.held;
      return Response.json(
        { code: "INVALID", message } satisfies ErrorBody,
        { status: 400 },
      );
    });
    const toastError = vi.spyOn(toast, "error");
    const user = userEvent.setup();
    renderPage();
    await fillForm(user, VALID_INPUT);
    const form = screen.getByRole("form", { name: "New reward rule" });

    await save(user);
    await waitFor(() => {
      expect(button("Save").getAttribute("aria-disabled")).toBe("true");
    });
    // Inert but not disabled: a disabled button would hand focus to the body.
    expect(button("Save").hasAttribute("disabled")).toBe(false);
    expect(document.activeElement).toBe(button("Save"));
    expect(form.getAttribute("aria-busy")).toBe("true");
    expect(button("Reset").getAttribute("aria-disabled")).toBe("true");
    // Reset keeps the entry, and Save again sends nothing.
    await user.click(button("Reset"));
    await save(user);
    expect(inputValue("Reward rate")).toBe(VALID_INPUT.reward_rate);
    expect(fetchMock).toHaveBeenCalledTimes(1);

    answer.release();
    await waitFor(() => {
      expect(toastError).toHaveBeenCalledWith(message);
    });
    await waitFor(() => {
      expect(button("Save").getAttribute("aria-disabled")).toBe("false");
    });
    expect(document.activeElement).toBe(button("Save"));
    expect(button("Reset").getAttribute("aria-disabled")).toBe("false");
    expect(form.getAttribute("aria-busy")).toBe("false");
    expect(toastError).toHaveBeenCalledTimes(1);
    for (const [, label] of FIELDS) {
      expect(textbox(label).getAttribute("aria-invalid")).toBe("false");
    }
    expect(screen.queryByText(LIST_MARKER)).toBeNull();
  });
});

// A rule with a non-midnight start and an end with a microsecond fraction, as
// `GET /admin/reward-rules` writes them.
const TIMED_RULE: RewardRuleResponse = {
  rule_id: "3",
  transaction_type_cd: "07",
  transaction_category_cd: "0001",
  reward_rate: "1.2500",
  min_transaction_amount: "5.00",
  max_transaction_amount: null,
  active_from: "2022-07-01T12:00:00Z",
  active_to: "2030-01-01T06:30:00.123456Z",
  status: "active",
};

// A soft-deleted rule: DELETE set its end to the processing clock's instant.
const ENDED_RULE: RewardRuleResponse = {
  rule_id: "4",
  transaction_type_cd: "07",
  transaction_category_cd: "0002",
  reward_rate: "2.0000",
  min_transaction_amount: null,
  max_transaction_amount: null,
  active_from: "2022-03-15T08:15:30Z",
  active_to: "2026-11-04T10:00:14.437498Z",
  status: "ended",
};

// Answers the rule list with `rule` alone, and the PUT that saves it with the
// rule as stored.
function stubRuleApi(rule: RewardRuleResponse): void {
  fetchMock.mockImplementation((input, init) => {
    const request = input instanceof Request ? input : new Request(input, init);
    return Promise.resolve(
      Response.json(request.method === "GET" ? [rule] : rule),
    );
  });
}

// The value shown in the input labelled `label`.
function inputValue(label: string): string {
  const input = textbox(label);
  if (!(input instanceof HTMLInputElement)) {
    throw new Error(`${label} is not an input`);
  }
  return input.value;
}

// The one request with `method` the client handed to the network.
function onlyRequest(method: string): Request {
  const requests = fetchMock.mock.calls
    .map(([input, init]) =>
      input instanceof Request ? input : new Request(input, init),
    )
    .filter((request) => request.method === method);
  expect(requests).toHaveLength(1);
  return requests[0];
}

describe("RewardRuleFormPage (edit rule)", () => {
  it.each<{
    name: string;
    rule: RewardRuleResponse;
    label: string;
    value: string;
    sent: RewardRuleRequest;
  }>([
    {
      name: "keeps a non-midnight start and a microsecond end when only the rate changes",
      rule: TIMED_RULE,
      label: "Reward rate",
      value: "1.5000",
      sent: {
        transaction_type_cd: "07",
        transaction_category_cd: "0001",
        reward_rate: "1.5000",
        min_transaction_amount: "5.00",
        active_from: "2022-07-01T12:00:00Z",
        active_to: "2030-01-01T06:30:00.123456Z",
      },
    },
    {
      name: "keeps a soft-deleted rule's start and end when only the rate changes",
      rule: ENDED_RULE,
      label: "Reward rate",
      value: "2.5000",
      sent: {
        transaction_type_cd: "07",
        transaction_category_cd: "0002",
        reward_rate: "2.5000",
        active_from: "2022-03-15T08:15:30Z",
        active_to: "2026-11-04T10:00:14.437498Z",
      },
    },
    {
      name: "sends a changed start as midnight UTC and keeps the stored end",
      rule: TIMED_RULE,
      label: "Active from",
      value: "2022-08-01",
      sent: {
        transaction_type_cd: "07",
        transaction_category_cd: "0001",
        reward_rate: "1.2500",
        min_transaction_amount: "5.00",
        active_from: "2022-08-01T00:00:00Z",
        active_to: "2030-01-01T06:30:00.123456Z",
      },
    },
    {
      name: "sends a changed end as midnight UTC and keeps the stored start",
      rule: TIMED_RULE,
      label: "Active to",
      value: "2031-06-30",
      sent: {
        transaction_type_cd: "07",
        transaction_category_cd: "0001",
        reward_rate: "1.2500",
        min_transaction_amount: "5.00",
        active_from: "2022-07-01T12:00:00Z",
        active_to: "2031-06-30T00:00:00Z",
      },
    },
  ])("$name", async ({ rule, label, value, sent }) => {
    stubRuleApi(rule);
    const user = userEvent.setup();
    renderPage(`${LIST_ROUTE}/${rule.rule_id}/edit`);

    // The form shows each stored instant as its date only.
    expect(
      await screen.findByRole("form", {
        name: `Edit reward rule ${rule.rule_id}`,
      }),
    ).toBeTruthy();
    expect(inputValue("Active from")).toBe(rule.active_from.slice(0, 10));
    expect(inputValue("Active to")).toBe(rule.active_to?.slice(0, 10));

    await user.clear(textbox(label));
    await user.type(textbox(label), value);
    await save(user);

    expect(await screen.findByText(LIST_MARKER)).toBeTruthy();
    const request = onlyRequest("PUT");
    expect(new URL(request.url).pathname).toBe(
      `/api/admin/reward-rules/${rule.rule_id}`,
    );
    // Strict: the instants byte for byte, and no key beyond these.
    const body: RewardRuleRequest = await request.clone().json();
    expect(body).toStrictEqual(sent);
  });
});

// Every field blank: the new-rule form's starting values.
const BLANK: FormEntry = {
  transaction_type_cd: "",
  transaction_category_cd: "",
  reward_rate: "",
  min_transaction_amount: "",
  max_transaction_amount: "",
  active_from: "",
  active_to: "",
};

// TIMED_RULE as its edit form shows it: each instant as its date, the absent
// maximum as the empty input.
const TIMED_RULE_SHOWN: FormEntry = {
  transaction_type_cd: "07",
  transaction_category_cd: "0001",
  reward_rate: "1.2500",
  min_transaction_amount: "5.00",
  max_transaction_amount: "",
  active_from: "2022-07-01",
  active_to: "2030-01-01",
};

const TIMED_RULE_ROUTE = `${LIST_ROUTE}/${TIMED_RULE.rule_id}/edit`;

// The value shown in every form field.
function shownValues(): FormEntry {
  const shown = { ...BLANK };
  for (const [name, label] of FIELDS) {
    shown[name] = inputValue(label);
  }
  return shown;
}

// The labels of the fields marked aria-invalid, in tab order.
function invalidFields(): string[] {
  return FIELDS.map(([, label]) => label).filter(
    (label) => textbox(label).getAttribute("aria-invalid") === "true",
  );
}

// The method of every request the client handed to the network, in order.
function sentMethods(): string[] {
  return fetchMock.mock.calls.map(
    ([input, init]) =>
      (input instanceof Request ? input : new Request(input, init)).method,
  );
}

describe("RewardRuleFormPage (Reset and Back)", () => {
  it("clears typed values and errors on Reset in a new rule and sends nothing", async () => {
    const user = userEvent.setup();
    renderPage();

    // Two valid values, a malformed rate, and the required category and
    // start left blank.
    await fillForm(user, {
      ...BLANK,
      transaction_type_cd: "01",
      reward_rate: "abc",
      max_transaction_amount: "50.00",
    });
    await save(user);
    await waitFor(() => {
      expect(invalidFields()).toEqual([
        "Transaction category code",
        "Reward rate",
        "Active from",
      ]);
    });
    expect(screen.getAllByRole("alert")).toHaveLength(3);

    await user.click(screen.getByRole("button", { name: "Reset" }));

    await waitFor(() => {
      expect(invalidFields()).toEqual([]);
    });
    expect(shownValues()).toEqual(BLANK);
    expect(screen.queryAllByRole("alert")).toEqual([]);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(screen.getByRole("form", { name: "New reward rule" })).toBeTruthy();
  });

  it("restores the stored rule and clears errors on Reset in an edit and sends no PUT", async () => {
    stubRuleApi(TIMED_RULE);
    const user = userEvent.setup();
    renderPage(TIMED_RULE_ROUTE);

    expect(
      await screen.findByRole("form", {
        name: `Edit reward rule ${TIMED_RULE.rule_id}`,
      }),
    ).toBeTruthy();
    expect(shownValues()).toEqual(TIMED_RULE_SHOWN);

    await user.clear(textbox("Reward rate"));
    await user.type(textbox("Reward rate"), "abc");
    await user.type(textbox("Maximum transaction amount"), "99.00");
    await user.clear(textbox("Active to"));
    await user.type(textbox("Active to"), "2031-06-30");
    await save(user);
    await waitFor(() => {
      expect(invalidFields()).toEqual(["Reward rate"]);
    });

    await user.click(screen.getByRole("button", { name: "Reset" }));

    await waitFor(() => {
      expect(invalidFields()).toEqual([]);
    });
    expect(shownValues()).toEqual(TIMED_RULE_SHOWN);
    expect(screen.queryAllByRole("alert")).toEqual([]);
    expect(sentMethods()).toEqual(["GET"]);
  });

  it.each<{ mode: string; route: string; heading: string; sent: string[] }>([
    {
      mode: "a new rule",
      route: NEW_ROUTE,
      heading: "New reward rule",
      sent: [],
    },
    {
      mode: "an edit",
      route: TIMED_RULE_ROUTE,
      heading: `Edit reward rule ${TIMED_RULE.rule_id}`,
      sent: ["GET"],
    },
  ])(
    "reaches Back by Tab after Save and Reset in $mode and leaves for the list unsaved",
    async ({ route, heading, sent }) => {
      // Only the edit reads the rule list; a new rule sends no request.
      stubRuleApi(TIMED_RULE);
      const user = userEvent.setup();
      renderPage(route);
      await screen.findByRole("form", { name: heading });

      // A change Back must leave unsaved.
      await user.type(textbox("Maximum transaction amount"), "99.00");
      // From the last field: Save, Reset, then Back.
      await user.click(textbox("Active to"));
      await user.tab();
      expect(document.activeElement).toBe(
        screen.getByRole("button", { name: "Save" }),
      );
      await user.tab();
      expect(document.activeElement).toBe(
        screen.getByRole("button", { name: "Reset" }),
      );
      await user.tab();
      const back = screen.getByRole("link", { name: "Back" });
      expect(document.activeElement).toBe(back);
      expect(back.getAttribute("href")).toBe(LIST_ROUTE);

      await user.keyboard("{Enter}");

      expect(await screen.findByText(LIST_MARKER)).toBeTruthy();
      expect(sentMethods()).toEqual(sent);
    },
  );
});

// The edit route shows no editable form: no field, no Save or Reset, only the
// Back link to the list.
function expectNoForm(): void {
  expect(screen.queryByRole("form")).toBeNull();
  expect(screen.queryAllByRole("textbox")).toEqual([]);
  expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Reset" })).toBeNull();
  expect(screen.getByRole("link", { name: "Back" }).getAttribute("href")).toBe(
    LIST_ROUTE,
  );
}

describe("RewardRuleFormPage (edit route without a rule)", () => {
  it.each<{
    name: string;
    answer: () => Promise<Response>;
    message: string;
  }>([
    {
      name: "a 500",
      answer: () =>
        Promise.resolve(
          Response.json(
            {
              code: "UNEXPECTED",
              message: UNEXPECTED_ERROR,
            } satisfies ErrorBody,
            { status: 500 },
          ),
        ),
      message: UNEXPECTED_ERROR,
    },
    {
      // The server's admin gate, whatever the route guard showed.
      name: "a 403",
      answer: () =>
        Promise.resolve(
          Response.json(
            {
              code: "FORBIDDEN",
              message: "Admin access required",
            } satisfies ErrorBody,
            { status: 403 },
          ),
        ),
      message: "Admin access required",
    },
    {
      name: "no answer",
      answer: () => Promise.reject(new TypeError("Failed to fetch")),
      message: UNEXPECTED_ERROR,
    },
  ])(
    "toasts a failed rule-list load ($name) once and keeps its text in an alert, with no form",
    async ({ answer, message }) => {
      fetchMock.mockImplementation(answer);
      const toastError = vi.spyOn(toast, "error");
      const user = userEvent.setup();
      renderPage(TIMED_RULE_ROUTE);

      await waitFor(() => {
        expect(screen.getByRole("alert").textContent).toBe(message);
      });
      expect(toastError).toHaveBeenCalledTimes(1);
      expect(toastError).toHaveBeenCalledWith(message);
      expectNoForm();

      await user.click(screen.getByRole("link", { name: "Back" }));

      expect(await screen.findByText(LIST_MARKER)).toBeTruthy();
      expect(sentMethods()).toEqual(["GET"]);
    },
  );

  it("shows 'Reward rule not found' in an alert, with no form and no toast, for a rule the list lacks", async () => {
    stubRuleApi(TIMED_RULE);
    const toastError = vi.spyOn(toast, "error");
    const user = userEvent.setup();
    renderPage(`${LIST_ROUTE}/9999/edit`);

    expect((await screen.findByRole("alert")).textContent).toBe(
      REWARD_RULE_MESSAGES.notFound,
    );
    expectNoForm();
    expect(toastError).not.toHaveBeenCalled();

    await user.click(screen.getByRole("link", { name: "Back" }));

    expect(await screen.findByText(LIST_MARKER)).toBeTruthy();
    expect(sentMethods()).toEqual(["GET"]);
  });
});

// The list route's element for a cache filled before the edit route opens: it
// observes the rule list as RewardRuleListPage does and shows the state of its
// read, so a test can wait for that read to settle.
function RuleListReader(): JSX.Element {
  const rules = useRewardRules();
  return <div>{`${LIST_MARKER}: ${rules.status}, ${rules.fetchStatus}`}</div>;
}

// The answer of read `read` of the rule list: the rule list holding
// TIMED_RULE, or a 500 whose message names the read, so a toast names the
// read it reports.
function listReadAnswer(status: 200 | 500, read: number): Response {
  return status === 200
    ? Response.json([TIMED_RULE])
    : Response.json(
        {
          code: "UNEXPECTED",
          message: `Read ${String(read)} failed`,
        } satisfies ErrorBody,
        { status },
      );
}

describe("RewardRuleFormPage (edit route over a failed rule-list read)", () => {
  // Each row is a run of list-route visits over one cache, each visit's read
  // answering the status given, so the cache holds a failed read alone or
  // beside the rule list an earlier read returned. The edit route then reads
  // the list again and reports that read's answer alone: the skeleton while
  // it is out, with no toast and no form built from the rules read earlier;
  // then the form with no toast, or the new failure's text in the alert,
  // toasted once.
  it.each<{ cached: readonly (200 | 500)[]; reread: 200 | 500 }>([
    { cached: [500], reread: 200 },
    { cached: [500], reread: 500 },
    { cached: [200, 500], reread: 200 },
    { cached: [200, 500], reread: 500 },
  ])(
    "after list reads answering $cached, reports only its own read answering $reread",
    async ({ cached, reread }) => {
      const toastError = vi.spyOn(toast, "error");
      const out = hold();
      for (const [index, status] of cached.entries()) {
        fetchMock.mockImplementationOnce(() =>
          Promise.resolve(listReadAnswer(status, index + 1)),
        );
      }
      const rereadNumber = cached.length + 1;
      fetchMock.mockImplementationOnce(async () => {
        await out.held;
        return listReadAnswer(reread, rereadNumber);
      });
      const router = renderPage(LIST_ROUTE, { element: <RuleListReader /> });
      for (const [index, status] of cached.entries()) {
        if (index > 0) {
          // A return to the list route reads the list again.
          await act(() => router.navigate(NEW_ROUTE));
          await act(() => router.navigate(LIST_ROUTE));
        }
        expect(
          await screen.findByText(
            `${LIST_MARKER}: ${status === 200 ? "success" : "error"}, idle`,
          ),
        ).toBeTruthy();
      }

      await act(() => router.navigate(TIMED_RULE_ROUTE));

      await waitFor(() => {
        expect(sentMethods()).toHaveLength(rereadNumber);
      });
      expect(screen.getByText("Loading reward rule")).toBeTruthy();
      expect(screen.queryByRole("form")).toBeNull();
      expect(screen.queryAllByRole("textbox")).toEqual([]);
      expect(screen.queryByRole("alert")).toBeNull();
      expect(toastError).not.toHaveBeenCalled();

      out.release();

      if (reread === 200) {
        expect(
          await screen.findByRole("form", {
            name: `Edit reward rule ${TIMED_RULE.rule_id}`,
          }),
        ).toBeTruthy();
        expect(inputValue("Reward rate")).toBe(TIMED_RULE.reward_rate);
        expect(screen.queryByRole("alert")).toBeNull();
        expect(toastError).not.toHaveBeenCalled();
      } else {
        const message = `Read ${String(rereadNumber)} failed`;
        await waitFor(() => {
          expect(screen.getByRole("alert").textContent).toBe(message);
        });
        expect(toastError).toHaveBeenCalledTimes(1);
        expect(toastError).toHaveBeenCalledWith(message);
        expectNoForm();
      }
      expect(screen.queryByText("Loading reward rule")).toBeNull();
      expect(sentMethods()).toEqual(Array(rereadNumber).fill("GET"));
    },
  );
});

// The list page is a lazy chunk: the router keeps the form on screen until it
// has loaded, so a saved rule must leave the form inert meanwhile.
describe("RewardRuleFormPage (saved, before the list loads)", () => {
  it.each([
    {
      mode: "new",
      path: NEW_ROUTE,
      method: "POST",
      enter: async (user: UserEvent) => {
        await fillForm(user, VALID_INPUT);
      },
    },
    {
      mode: "edit",
      path: `${LIST_ROUTE}/${TIMED_RULE.rule_id}/edit`,
      method: "PUT",
      enter: async (user: UserEvent) => {
        await screen.findByRole("form", {
          name: `Edit reward rule ${TIMED_RULE.rule_id}`,
        });
        await user.clear(textbox("Reward rate"));
        await user.type(textbox("Reward rate"), "1.5000");
      },
    },
  ])(
    "keeps Save and Reset inert after the $method succeeds ($mode)",
    async ({ path, method, enter }) => {
      stubRuleApi(TIMED_RULE);
      const list = hold();
      const user = userEvent.setup();
      const router = renderPage(path, {
        lazy: async () => {
          await list.held;
          return { element: <div>{LIST_MARKER}</div> };
        },
      });
      await enter(user);
      const entered = inputValue("Reward rate");

      await save(user);
      await waitFor(() => {
        expect(router.state.navigation.state).toBe("loading");
      });
      onlyRequest(method);
      expect(button("Save").getAttribute("aria-disabled")).toBe("true");
      expect(button("Reset").getAttribute("aria-disabled")).toBe("true");
      expect(screen.getByRole("form").getAttribute("aria-busy")).toBe("true");
      expect(document.activeElement).toBe(button("Save"));

      // Enter on Save, Enter in a field, Reset and Save again: no second
      // write, and the entry stays as saved.
      await user.keyboard("{Enter}");
      await user.click(textbox("Reward rate"));
      await user.keyboard("{Enter}");
      await user.click(button("Reset"));
      await save(user);
      expect(inputValue("Reward rate")).toBe(entered);
      onlyRequest(method);
      expect(screen.queryByText(LIST_MARKER)).toBeNull();

      list.release();
      expect(await screen.findByText(LIST_MARKER)).toBeTruthy();
      onlyRequest(method);
    },
  );
});
