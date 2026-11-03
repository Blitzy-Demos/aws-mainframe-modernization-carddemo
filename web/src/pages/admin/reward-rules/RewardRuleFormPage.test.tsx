// FN test of the admin reward-rule form (RewardRuleFormPage) in new mode: valid
// input is sent as the RewardRuleRequest body of `POST /admin/reward-rules`,
// input the schema rejects is never sent, and a server 400 lands on its field
// with a toast and the cursor. The page runs with the real API client, query
// hooks and error helper; only the network is stubbed, through the global the
// client looks up on every request.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { components } from "@/api/schema";
import type { RouteHandle } from "@/components/page-header";
import {
  REWARD_RULE_MESSAGES,
  rewardRuleSchema,
  type RewardRuleFormInput,
} from "@/schemas/reward-rules";

import RewardRuleFormPage from "./RewardRuleFormPage";

type RewardRuleRequest = components["schemas"]["RewardRuleRequest"];
type RewardRuleResponse = components["schemas"]["RewardRuleResponse"];
type ErrorBody = components["schemas"]["ErrorBody"];

type FieldName = keyof RewardRuleFormInput;

/** A value for every form field, as typed; `""` leaves the field blank. */
type FormEntry = Record<FieldName, string>;

const NEW_ROUTE = "/admin/reward-rules/new";
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

// The page alone at its own route, beside a marker list route, with a fresh
// query cache and router per test so no state carries over between cases.
function renderPage(): void {
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
      { path: LIST_ROUTE, element: <div>{LIST_MARKER}</div> },
    ],
    { initialEntries: [NEW_ROUTE] },
  );
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
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
});
