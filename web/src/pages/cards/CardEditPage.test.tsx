// Tests of CardEditPage (pages/cards/CardEditPage.tsx), the CCUP screen of
// COCRDUPC on map COCRDUP: "Save" (ENTER and PF5 in one submit,
// F-COCRDUP-002, F-COCRDUP-005) sends the entry as the PUT /cards/{id} body;
// a month 1250-EDIT-EXPIRY-MON rejects blocks the PUT; keys the COCRDUPC key
// edits reject are marked without a read; and a field or key error the
// server answers is toasted, marked on its control and given focus, as
// `MOVE -1 TO <field>L` places the cursor. A save and PF12 Cancel's re-read
// run one at a time, and the pressed button keeps focus while they run.
//
// The network is the global the API client looks up on every request,
// stubbed to answer as the API does for card 0500024453765740 of account
// 00000000050, the first app/data/ASCII/carddata.txt record. Expected texts
// are the docs/parity/validation-audit.md COCRDUPC rows, byte for byte.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
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

/** The labels of the protected keys, ACCTSID and CARDSID. */
const KEY_LABELS = ["Account Number", "Card Number"] as const;

type KeyLabel = (typeof KEY_LABELS)[number];

/** The URL the card list's "Edit" action opens for the card. */
const EDIT_ENTRY = `/cards/${CARD_NUMBER}/edit?account_id=${ACCOUNT_ID}`;

/**
 * How the stubbed API answers a `PUT /cards/{id}` with `body`; a promise
 * holds the PUT until the test settles it.
 */
type PutAnswer = (body: CardUpdateRequest) => Response | Promise<Response>;

// The stubbed API's state, reset before each test: the stored card, a hold
// that keeps GET answers back while set, the answer to a PUT, every request
// as "METHOD /path?query", and the PUT bodies.
let stored: CardResponse;
let readHold: Promise<void> | undefined;
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
    // The card as stored when the request arrived, answered once any hold
    // ends.
    const answer: CardViewResponse = { ...stored, message: FOUND_MESSAGE };
    if (readHold !== undefined) {
      await readHold;
    }
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

/** A hold on a stubbed answer: the request waits until `release` is called. */
function hold(): { held: Promise<void>; release: () => void } {
  let release: () => void = () => undefined;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { held, release };
}

/**
 * Lets every effect, microtask and zero-delay timer scheduled so far run,
 * among them TanStack Query's notification of the page about new data.
 */
async function settle(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

/**
 * The page at its App.tsx route, `/cards/:cardNumber/edit`, under the card
 * list and card view routes its breadcrumb leads back to, entered at `entry`
 * (by default with the card and account keys), beside the app's Toaster.
 */
function renderPage(entry: string = EDIT_ENTRY) {
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
    { initialEntries: [entry] },
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

function input(label: Label | KeyLabel): HTMLInputElement {
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
  readHold = undefined;
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
    // The keys show as entered and cannot be edited.
    expect(input("Account Number").value).toBe(ACCOUNT_ID);
    expect(input("Card Number").value).toBe(CARD_NUMBER);
    for (const label of KEY_LABELS) {
      expect(input(label).readOnly).toBe(true);
      expect(input(label).getAttribute("aria-invalid")).toBe("false");
    }
  });

  // Each row: an entry URL whose key 1210-EDIT-ACCOUNT or 1220-EDIT-CARD
  // rejects, the key it marks with the value entered, and the COCRDUPC text.
  // A key error skips the card read, so no request is sent.
  const rejectedKeys: {
    entry: string;
    label: KeyLabel;
    entered: string;
    message: string;
  }[] = [
    {
      // V-COCRDUPC-003
      entry: `/cards/${CARD_NUMBER}/edit?account_id=ABC`,
      label: "Account Number",
      entered: "ABC",
      message: "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER",
    },
    {
      // V-COCRDUPC-005
      entry: `/cards/12345/edit?account_id=${ACCOUNT_ID}`,
      label: "Card Number",
      entered: "12345",
      message: "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER",
    },
  ];

  it.each(rejectedKeys)(
    "a rejected $label is marked and no card is read",
    async ({ entry, label, entered, message }) => {
      renderPage(entry);

      await screen.findByLabelText(label);
      const key = input(label);
      expect(key.getAttribute("aria-invalid")).toBe("true");
      const error = fieldErrorOf(key);
      expect(error.getAttribute("role")).toBe("alert");
      expect(error.textContent).toBe(message);
      expect(key.value).toBe(entered);
      expect(key.readOnly).toBe(true);
      expect(screen.queryByLabelText("Name on card")).toBeNull();
      // A read the page started would reach the stub within a timer task.
      await new Promise((resolve) => {
        setTimeout(resolve, 0);
      });
      expect(received).toEqual([]);
    },
  );

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

  // Each row: the audit row, the key field the error names, its control and
  // the key entered there, the status, and the COCRDUPC text, which puts the
  // cursor on that key.
  const serverKeyErrors: {
    audit: string;
    field: CardUpdateField;
    label: KeyLabel;
    entered: string;
    status: number;
    message: string;
  }[] = [
    {
      audit: "V-COCRDUPC-020",
      field: "ACCTSID",
      label: "Account Number",
      entered: ACCOUNT_ID,
      status: 500,
      message: "Update of record failed",
    },
    {
      audit: "V-COCRDUPC-018",
      field: "ACCTSID",
      label: "Account Number",
      entered: ACCOUNT_ID,
      status: 500,
      message: "Could not lock record for update",
    },
    {
      audit: "V-COCRDUPC-005",
      field: "CARDSID",
      label: "Card Number",
      entered: CARD_NUMBER,
      status: 400,
      message: "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER",
    },
  ];

  it.each(serverKeyErrors)(
    "$audit: a $status at $field is toasted, marked on $label and focuses it",
    async ({ field, label, entered, status, message }) => {
      answerPut = () =>
        errorAnswer(status, {
          code: status === 500 ? "UNEXPECTED" : "INVALID",
          message,
          fields: [{ field, message }],
        });
      const toastError = vi.spyOn(toast, "error");
      const { user } = await renderLoaded();
      const key = input(label);

      await enter(user, "Name on card", "Aniya Vonn");
      await user.click(screen.getByRole("button", { name: "Save" }));

      await waitFor(() => {
        expect(key.getAttribute("aria-invalid")).toBe("true");
      });
      expect(toastError).toHaveBeenCalledWith(message);
      const error = fieldErrorOf(key);
      expect(error.getAttribute("role")).toBe("alert");
      expect(error.textContent).toBe(message);
      await waitFor(() => {
        expect(document.activeElement).toBe(key);
      });
      // The key stays as entered and protected.
      expect(key.readOnly).toBe(true);
      expect(key.value).toBe(entered);
      expect(sentPuts()).toEqual([`PUT ${CARD_PATH}`]);

      // Enter on the focused key submits the form again.
      await user.keyboard("{Enter}");

      await waitFor(() => {
        expect(sentPuts()).toEqual([`PUT ${CARD_PATH}`, `PUT ${CARD_PATH}`]);
      });
    },
  );

  it("holds Save while Cancel reads the card again, then saves with the re-read and saved tokens", async () => {
    const toastSuccess = vi.spyOn(toast, "success");
    const { user } = await renderLoaded();
    const save = screen.getByRole("button", { name: "Save" });
    const cancel = screen.getByRole("button", { name: "Cancel" });

    await enter(user, "Name on card", "Changed Name");
    // The re-read answers the card as stored before any save.
    const reread = hold();
    readHold = reread.held;
    await user.click(cancel);
    await waitFor(() => {
      expect(received).toHaveLength(2);
    });

    // While the re-read is held, neither button acts, by click or by Enter,
    // and the pressed button keeps focus.
    expect(save.getAttribute("aria-disabled")).toBe("true");
    expect(cancel.getAttribute("aria-disabled")).toBe("true");
    expect(save.hasAttribute("disabled")).toBe(false);
    expect(cancel.hasAttribute("disabled")).toBe(false);
    expect(document.activeElement).toBe(cancel);
    await user.click(save);
    await user.click(input("Name on card"));
    await user.keyboard("{Enter}");
    await user.click(cancel);
    await settle();
    const read = `GET ${CARD_PATH}?account_id=${ACCOUNT_ID}`;
    expect(received).toEqual([read, read]);

    readHold = undefined;
    reread.release();
    await waitFor(() => {
      expect(input("Name on card").value).toBe("Aniya Von");
    });
    await waitFor(() => {
      expect(save.getAttribute("aria-disabled")).toBe("false");
    });
    expect(cancel.getAttribute("aria-disabled")).toBe("false");
    await settle();

    await enter(user, "Name on card", "Aniya Vonn");
    await user.click(save);
    await waitFor(() => {
      expect(toastSuccess).toHaveBeenCalledWith(COMMITTED);
    });
    await settle();
    expect(input("Name on card").value).toBe("Aniya Vonn");

    // The next save echoes the saved card's token.
    await user.click(save);
    await waitFor(() => {
      expect(putBodies).toHaveLength(2);
    });
    const entry: Omit<CardUpdateRequest, "version"> = {
      account_id: ACCOUNT_ID,
      embossed_name: "Aniya Vonn",
      active_status: "Y",
      expiry_month: "03",
      expiry_year: "2023",
    };
    expect(putBodies).toEqual([
      { ...entry, version: CARD.version },
      { ...entry, version: UPDATED_CARD.version },
    ]);
    // Each save settles once its re-read of the card has answered.
    await waitFor(() => {
      expect(received).toHaveLength(6);
    });
    await waitFor(() => {
      expect(save.getAttribute("aria-disabled")).toBe("false");
    });
  });

  // Each row: what is pressed while the first save is held. Neither sends a
  // request: not a second PUT, not Cancel's re-read.
  const pressesDuringSave: {
    press: string;
    again: (user: ReturnType<typeof userEvent.setup>) => Promise<void>;
  }[] = [
    {
      press: "Save clicked again",
      again: (user) => user.click(screen.getByRole("button", { name: "Save" })),
    },
    {
      press: "Enter in Name on card",
      again: async (user) => {
        await user.click(input("Name on card"));
        await user.keyboard("{Enter}");
      },
    },
    {
      press: "Cancel clicked",
      again: (user) =>
        user.click(screen.getByRole("button", { name: "Cancel" })),
    },
  ];

  it.each(pressesDuringSave)(
    "$press while a save runs sends nothing more",
    async ({ again }) => {
      const toastSuccess = vi.spyOn(toast, "success");
      const write = hold();
      answerPut = async () => {
        await write.held;
        return commitRename();
      };
      const { user } = await renderLoaded();

      await enter(user, "Name on card", "Aniya Vonn");
      await user.click(screen.getByRole("button", { name: "Save" }));
      await waitFor(() => {
        expect(sentPuts()).toHaveLength(1);
      });
      await again(user);
      await settle();
      const read = `GET ${CARD_PATH}?account_id=${ACCOUNT_ID}`;
      expect(received).toEqual([read, `PUT ${CARD_PATH}`]);

      write.release();
      await waitFor(() => {
        expect(toastSuccess).toHaveBeenCalledWith(COMMITTED);
      });
      // The only read after the PUT is the hook's re-read of the saved card.
      expect(received).toEqual([read, `PUT ${CARD_PATH}`, read]);
      expect(putBodies).toHaveLength(1);
      expect(input("Name on card").value).toBe("Aniya Vonn");
    },
  );

  it("keeps focus on Save while the save runs and after it", async () => {
    const toastSuccess = vi.spyOn(toast, "success");
    const write = hold();
    answerPut = async () => {
      await write.held;
      return commitRename();
    };
    const { user } = await renderLoaded();
    const save = screen.getByRole("button", { name: "Save" });
    const cancel = screen.getByRole("button", { name: "Cancel" });

    await enter(user, "Name on card", "Aniya Vonn");
    await user.click(save);
    await waitFor(() => {
      expect(save.getAttribute("aria-disabled")).toBe("true");
    });

    // Inert but not disabled: a disabled button would hand focus to the body.
    expect(save.hasAttribute("disabled")).toBe(false);
    expect(cancel.getAttribute("aria-disabled")).toBe("true");
    expect(cancel.hasAttribute("disabled")).toBe(false);
    expect(document.activeElement).toBe(save);

    write.release();
    await waitFor(() => {
      expect(toastSuccess).toHaveBeenCalledWith(COMMITTED);
    });
    await waitFor(() => {
      expect(save.getAttribute("aria-disabled")).toBe("false");
    });
    expect(cancel.getAttribute("aria-disabled")).toBe("false");
    expect(document.activeElement).toBe(save);
  });
});
