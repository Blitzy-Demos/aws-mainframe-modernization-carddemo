// Tests of CardEditPage (pages/cards/CardEditPage.tsx), the CCUP screen of
// COCRDUPC on map COCRDUP: "Save" (ENTER and PF5 in one submit,
// F-COCRDUP-002, F-COCRDUP-005) sends the entry as the PUT /cards/{id} body;
// a month 1250-EDIT-EXPIRY-MON rejects blocks the PUT; and a field error the
// server answers is toasted, marked on its control and given focus, as
// `MOVE -1 TO <field>L` places the cursor.
//
// The network is the global the API client looks up on every request,
// stubbed to answer as the API does for card 0500024453765740 of account
// 00000000050, the first app/data/ASCII/carddata.txt record. Expected texts
// are the docs/parity/validation-audit.md COCRDUPC rows, byte for byte.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ErrorBody } from "@/api/client";
import type { components } from "@/api/schema";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";

import CardEditPage from "./CardEditPage";

type CardResponse = components["schemas"]["CardResponse"];
type CardViewResponse = components["schemas"]["CardViewResponse"];
type CardUpdateRequest = components["schemas"]["CardUpdateRequest"];
type CardUpdateResponse = components["schemas"]["CardUpdateResponse"];
type CardUpdateField = components["schemas"]["CardUpdateField"];

const CARD_NUMBER = "0500024453765740";
const ACCOUNT_ID = "00000000050";
const CARD_PATH = `/api/cards/${CARD_NUMBER}`;

/** The COCRDUP title line [app/bms/COCRDUP.bms:L75-78]. */
const TITLE = "Update Credit Card Details";

/**
 * The card as `GET /cards/{id}` returns it: carddata.txt record 1 (name,
 * expiry, status), with the version token a PUT echoes.
 */
const CARD: CardResponse = {
  account_id: ACCOUNT_ID,
  card_number: CARD_NUMBER,
  card_embossed_name: "Aniya Von",
  card_expiraion_date: "2023-03-09",
  card_active_status: "Y",
  version: "0123456789abcdef",
};

/** The card once the rename is rewritten, with its new token. */
const UPDATED_CARD: CardResponse = {
  ...CARD,
  card_embossed_name: "Aniya Vonn",
  version: "fedcba9876543210",
};

/** COCRDSLC's information line for a found card (P-COCRDSLC-010). */
const FOUND_MESSAGE = "   Displaying requested details";

/** COCRDUPC's text after the rewrite (P-COCRDUPC-007). */
const COMMITTED = "Changes committed to database";

/** The labels of the unprotected COCRDUP fields. */
const LABELS = [
  "Name on card",
  "Card Active Y/N",
  "Expiry Month",
  "Expiry Year",
] as const;

type Label = (typeof LABELS)[number];

/** How the stubbed API answers a `PUT /cards/{id}` with `body`. */
type PutAnswer = (body: CardUpdateRequest) => Response;

// The stubbed API's state, reset before each test: the stored card, the
// answer to a PUT, every request as "METHOD /path?query", and the PUT bodies.
let stored: CardResponse;
let answerPut: PutAnswer;
let received: string[];
let putBodies: CardUpdateRequest[];

function errorAnswer(status: number, body: ErrorBody): Response {
  return Response.json(body, { status });
}

/** A rewrite: UPDATED_CARD is stored and returned with COMMITTED. */
function commitRename(): Response {
  stored = UPDATED_CARD;
  const answer: CardUpdateResponse = { card: stored, message: COMMITTED };
  return Response.json(answer);
}

async function serve(request: Request): Promise<Response> {
  const url = new URL(request.url);
  received.push(`${request.method} ${url.pathname}${url.search}`);
  if (url.pathname === CARD_PATH && request.method === "GET") {
    const answer: CardViewResponse = { ...stored, message: FOUND_MESSAGE };
    return Response.json(answer);
  }
  if (url.pathname === CARD_PATH && request.method === "PUT") {
    const body: CardUpdateRequest = await request.clone().json();
    putBodies.push(body);
    return answerPut(body);
  }
  return errorAnswer(404, {
    code: "NOT_FOUND",
    message: `No stubbed answer for ${request.method} ${url.pathname}`,
  });
}

function sentPuts(): string[] {
  return received.filter((line) => line.startsWith("PUT "));
}

/**
 * The page at its App.tsx route, `/cards/:cardNumber/edit`, under the card
 * list and card view routes its breadcrumb leads back to, entered with the
 * card and account keys, beside the app's Toaster.
 */
function renderPage() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const router = createMemoryRouter(
    [
      {
        path: "/cards",
        handle: { crumb: "Credit Cards" },
        children: [
          { index: true, element: <p>Card list</p> },
          {
            path: ":cardNumber",
            handle: { crumb: "Card details" },
            children: [
              { index: true, element: <p>Card details</p> },
              {
                path: "edit",
                handle: { crumb: "Edit", title: TITLE },
                element: <CardEditPage />,
              },
            ],
          },
        ],
      },
    ],
    {
      initialEntries: [`/cards/${CARD_NUMBER}/edit?account_id=${ACCOUNT_ID}`],
    },
  );
  const user = userEvent.setup();
  render(
    <QueryClientProvider client={queryClient}>
      <ThemeProvider>
        <RouterProvider router={router} />
        <Toaster />
      </ThemeProvider>
    </QueryClientProvider>,
  );
  return { user };
}

function input(label: Label): HTMLInputElement {
  const element = screen.getByLabelText(label);
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`"${label}" labels no input`);
  }
  return element;
}

/** The FieldError `control` names in its aria-describedby. */
function fieldErrorOf(control: HTMLElement): HTMLElement {
  const id = control.getAttribute("aria-describedby");
  const error = id === null ? null : document.getElementById(id);
  if (error === null) {
    throw new Error(`${control.id} describes itself by no element`);
  }
  return error;
}

/** Renders the page and waits until the form holds the fetched card. */
async function renderLoaded() {
  const harness = renderPage();
  await screen.findByLabelText("Name on card");
  await waitFor(() => {
    expect(input("Name on card").value).toBe("Aniya Von");
  });
  return harness;
}

/** Replaces the entry of the field labelled `label` with `value`. */
async function enter(
  user: ReturnType<typeof userEvent.setup>,
  label: Label,
  value: string,
): Promise<HTMLInputElement> {
  const control = input(label);
  await user.clear(control);
  await user.type(control, value);
  return control;
}

beforeEach(() => {
  stored = CARD;
  answerPut = commitRename;
  received = [];
  putBodies = [];
  vi.stubGlobal("fetch", vi.fn(serve));
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("CardEditPage", () => {
  it("reads the card by the keys entered and shows it", async () => {
    await renderLoaded();

    expect(received).toEqual([`GET ${CARD_PATH}?account_id=${ACCOUNT_ID}`]);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(TITLE);
    expect(input("Card Active Y/N").value).toBe("Y");
    expect(input("Expiry Month").value).toBe("03");
    expect(input("Expiry Year").value).toBe("2023");
    for (const label of LABELS) {
      expect(input(label).getAttribute("aria-invalid")).toBe("false");
    }
  });

  it("Save sends one PUT with the entry, the keys and the token", async () => {
    const toastSuccess = vi.spyOn(toast, "success");
    const { user } = await renderLoaded();

    await enter(user, "Name on card", "Aniya Vonn");
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(toastSuccess).toHaveBeenCalledWith(COMMITTED);
    });
    const expected: CardUpdateRequest = {
      version: "0123456789abcdef",
      account_id: ACCOUNT_ID,
      embossed_name: "Aniya Vonn",
      active_status: "Y",
      expiry_month: "03",
      expiry_year: "2023",
    };
    expect(sentPuts()).toEqual([`PUT ${CARD_PATH}`]);
    expect(putBodies).toEqual([expected]);
    expect(await screen.findByText(COMMITTED)).toBeTruthy();
    // The form now holds the card as rewritten.
    expect(input("Name on card").value).toBe("Aniya Vonn");
    expect(input("Name on card").getAttribute("aria-invalid")).toBe("false");
  });

  // V-COCRDUPC-015: '13' fails `88 VALID-MONTH VALUES 1 THRU 12`; 'AB', which
  // the 9(2) redefine cannot read (E-47), gets the same outcome.
  it.each(["13", "AB"])(
    "Expiry Month %j fails 1250-EDIT-EXPIRY-MON and sends no PUT",
    async (month) => {
      const { user } = await renderLoaded();

      const control = await enter(user, "Expiry Month", month);
      await user.click(screen.getByRole("button", { name: "Save" }));

      await waitFor(() => {
        expect(control.getAttribute("aria-invalid")).toBe("true");
      });
      const error = fieldErrorOf(control);
      expect(error.getAttribute("role")).toBe("alert");
      expect(error.textContent).toBe(
        "Card expiry month must be between 1 and 12",
      );
      expect(document.activeElement).toBe(control);
      expect(sentPuts()).toEqual([]);
    },
  );

  // Each row: the BMS field the 400 names, the control it maps to, a valid
  // entry the client sends, and the COCRDUPC text of that field's edit.
  const serverFieldErrors: {
    field: CardUpdateField;
    label: Label;
    entry: string;
    message: string;
  }[] = [
    {
      // V-COCRDUPC-011
      field: "CRDNAME",
      label: "Name on card",
      entry: "Aniya Vonn",
      message: "Card name can only contain alphabets and spaces",
    },
    {
      // V-COCRDUPC-013
      field: "CRDSTCD",
      label: "Card Active Y/N",
      entry: "N",
      message: "Card Active Status must be Y or N",
    },
    {
      // V-COCRDUPC-015
      field: "EXPMON",
      label: "Expiry Month",
      entry: "04",
      message: "Card expiry month must be between 1 and 12",
    },
  ];

  it.each(serverFieldErrors)(
    "a 400 at $field is toasted, marked on $label and focuses it",
    async ({ field, label, entry, message }) => {
      answerPut = () =>
        errorAnswer(400, {
          code: "INVALID",
          message,
          fields: [{ field, message }],
        });
      const toastError = vi.spyOn(toast, "error");
      const { user } = await renderLoaded();

      // Clicking Save moves focus to the button; the error moves it back.
      const control = await enter(user, label, entry);
      await user.click(screen.getByRole("button", { name: "Save" }));

      await waitFor(() => {
        expect(control.getAttribute("aria-invalid")).toBe("true");
      });
      expect(toastError).toHaveBeenCalledWith(message);
      const error = fieldErrorOf(control);
      expect(error.getAttribute("role")).toBe("alert");
      expect(error.textContent).toBe(message);
      await waitFor(() => {
        expect(document.activeElement).toBe(control);
      });
      expect(sentPuts()).toEqual([`PUT ${CARD_PATH}`]);
      // The entry stays on screen for correction.
      expect(control.value).toBe(entry);
    },
  );
});
