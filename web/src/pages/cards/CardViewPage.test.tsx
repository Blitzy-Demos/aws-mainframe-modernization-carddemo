// FN tests of CardViewPage, CCDL "View Credit Card Detail" (COCRDSLC, map
// COCRDSL): the GET a pair of valid route keys sends and the card it shows,
// the key edits that send nothing, the display of a failed read, and F3=Exit.
//
// The page has no key form, because a detail screen takes its keys from the
// route (E-56, E-61): `/cards/:cardNumber?account_id=`. A key the edits reject
// therefore shows its audited text under the key's caption, not on an input,
// and a failed read is the toast and message line of showApiError, which marks
// no key and moves no focus, since the page holds no input to mark or focus.
//
// The network is the stubbed global the API client looks up on every request.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { components } from "@/api/schema";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";

import CardViewPage from "./CardViewPage";

type CardResponse = components["schemas"]["CardResponse"];
type CardViewResponse = components["schemas"]["CardViewResponse"];
type CardViewField = components["schemas"]["CardViewField"];
type ErrorBody = components["schemas"]["ErrorBody"];

// The COCRDSL screen title [app/bms/COCRDSL.bms:L75-78].
const TITLE = "View Credit Card Detail";

const CARD_NUMBER = "0500024453765740";
const ACCOUNT_ID = "00000000050";

// The route of the first card with its own account as the account key.
const VALID_ENTRY = `/cards/${CARD_NUMBER}?account_id=${ACCOUNT_ID}`;

// The first card master record [app/data/ASCII/carddata.txt:L1]: CARD-NUM,
// CARD-ACCT-ID, CARD-EMBOSSED-NAME, CARD-EXPIRAION-DATE and
// CARD-ACTIVE-STATUS, with a version token of the pattern the API declares.
const CARD: CardResponse = {
  card_number: CARD_NUMBER,
  account_id: ACCOUNT_ID,
  card_embossed_name: "Aniya Von",
  card_expiraion_date: "2023-03-09",
  card_active_status: "Y",
  version: "5d0c3e9a71f2b846",
};

// The answer of a found card: WS-INFO-MSG once FOUND-CARDS-FOR-ACCOUNT is set,
// with its three leading spaces (P-COCRDSLC-010).
const FOUND: CardViewResponse = {
  ...CARD,
  message: "   Displaying requested details",
};

// 9100-GETCARD-BYACCTCARD on NOTFND (V-COCRDSLC-006), byte for byte. The
// validation audit maps it to 404 (kind NotFound), so the stub answers 404.
const NOT_FOUND = "Did not find cards for this search condition";

type Network = (request: Request) => Promise<Response>;

// Stubs the network with `answer` and returns the stub, to read what was sent.
function stubNetwork(answer: (request: Request) => Response) {
  const network = vi.fn<Network>(async (request) => answer(request));
  vi.stubGlobal("fetch", network);
  return network;
}

// The requests the page handed to the network, in the order sent.
function sent(network: ReturnType<typeof stubNetwork>): Request[] {
  return network.mock.calls.map(([request]) => request);
}

// Renders the page at `entry` under a data router (PageHeader reads the
// matched routes' handles), with a fresh query cache that never retries, so a
// failed read reaches the page on its first answer. The card list and card
// edit routes are placeholders the page can navigate to.
function renderPage(entry: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const router = createMemoryRouter(
    [
      {
        path: "/cards",
        element: <p>Card list</p>,
        handle: { crumb: "Credit Cards" },
      },
      {
        path: "/cards/:cardNumber",
        element: <CardViewPage />,
        handle: { crumb: "View", title: TITLE },
      },
      {
        path: "/cards/:cardNumber/edit",
        element: <p>Card edit</p>,
        handle: { crumb: "Edit" },
      },
    ],
    { initialEntries: [entry] },
  );
  render(
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
      <Toaster />
    </ThemeProvider>,
  );
  return router;
}

// The card region, named by its "Card details" heading.
function cardRegion(): HTMLElement {
  return screen.getByRole("region", { name: "Card details" });
}

// The value cell that follows the caption `label` in the card region.
function valueOf(label: string): HTMLElement {
  const caption = within(cardRegion()).getByText(label, { selector: "dt" });
  const value = caption.nextElementSibling;
  if (!(value instanceof HTMLElement) || value.tagName !== "DD") {
    throw new Error(`no value cell follows the caption "${label}"`);
  }
  return value;
}

// Lets every effect, microtask and zero-delay timer the render scheduled run,
// so a query the page enabled would have reached the network by now.
async function settle(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

// Unmounted before the globals are restored, so no request of a mounted page
// can reach the real network. Sonner keeps its toasts in module state and
// replays every one not dismissed to the next Toaster that mounts, so each
// test's toasts are dismissed before the next test renders.
afterEach(() => {
  cleanup();
  toast.dismiss();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("CardViewPage with valid keys", () => {
  it("sends GET /cards/{id}?account_id= and shows the card (P-COCRDSLC-010)", async () => {
    const error = vi.spyOn(toast, "error");
    const network = stubNetwork(() => Response.json(FOUND));
    renderPage(VALID_ENTRY);

    await waitFor(() => {
      expect(network).toHaveBeenCalledTimes(1);
    });
    const [request] = sent(network);
    expect(request.method).toBe("GET");
    const url = new URL(request.url);
    expect(url.pathname).toBe(`/api/cards/${CARD_NUMBER}`);
    expect(url.searchParams.get("account_id")).toBe(ACCOUNT_ID);
    expect([...url.searchParams.keys()]).toEqual(["account_id"]);

    const region = cardRegion();
    expect(await within(region).findByText("Aniya Von")).toBeTruthy();
    expect(
      screen.getByRole("heading", { level: 1, name: TITLE }),
    ).toBeTruthy();
    // ACCTSIDO and CARDSIDO, then CRDNAME, CRDSTCD and EXPMON '/' EXPYEAR.
    expect(valueOf("Account Number").textContent).toBe(ACCOUNT_ID);
    expect(valueOf("Card Number").textContent).toBe(CARD_NUMBER);
    expect(valueOf("Name on card").textContent).toBe("Aniya Von");
    expect(valueOf("Card Active Y/N").textContent).toBe("Y");
    expect(valueOf("Expiry Date").textContent).toBe("03/2023");
    // INFOMSG exactly as the server sent it, leading spaces included.
    const info = within(region).getByText(FOUND.message.trim());
    expect(info.textContent).toBe(FOUND.message);
    expect(region.getAttribute("aria-busy")).toBe("false");
    expect(within(region).queryByRole("alert")).toBeNull();
    expect(error).not.toHaveBeenCalled();
    expect(network).toHaveBeenCalledTimes(1);
  });

  // CRDSTCD as a badge carrying its letter: N, the inactive state, is
  // destructive.
  it.each([
    { flag: "Y", variant: "secondary" },
    { flag: "N", variant: "destructive" },
  ])(
    "shows the active flag $flag as a $variant badge",
    async ({ flag, variant }) => {
      stubNetwork(() => Response.json({ ...FOUND, card_active_status: flag }));
      renderPage(VALID_ENTRY);
      await within(cardRegion()).findByText("Aniya Von");

      const badge = valueOf("Card Active Y/N").querySelector(
        '[data-slot="badge"]',
      );
      expect(badge?.textContent).toBe(flag);
      expect(badge?.getAttribute("data-variant")).toBe(variant);
    },
  );

  it("holds the message line's place while the card is read", async () => {
    let answer: (response: Response) => void = () => undefined;
    const held = new Promise<Response>((settle) => {
      answer = settle;
    });
    vi.stubGlobal(
      "fetch",
      vi.fn<Network>(async () => held),
    );
    renderPage(VALID_ENTRY);

    // One message line throughout: a hidden placeholder line while busy,
    // then INFOMSG.
    const region = cardRegion();
    expect(region.getAttribute("aria-busy")).toBe("true");
    const placeholder = region.querySelectorAll(
      '[data-slot="card-description"]',
    );
    expect(placeholder).toHaveLength(1);
    expect(placeholder[0].getAttribute("aria-hidden")).toBe("true");
    expect(placeholder[0].querySelector('[data-slot="skeleton"]')).not.toBeNull();

    answer(Response.json(FOUND));
    await within(region).findByText("Aniya Von");
    const line = region.querySelectorAll('[data-slot="card-description"]');
    expect(line).toHaveLength(1);
    expect(line[0].textContent).toBe(FOUND.message);
    expect(line[0].hasAttribute("aria-hidden")).toBe(false);
    expect(region.getAttribute("aria-busy")).toBe("false");
  });

  it("leaves for the card list by Back, F3=Exit (F-COCRDSL-002)", async () => {
    stubNetwork(() => Response.json(FOUND));
    const router = renderPage(VALID_ENTRY);
    await within(cardRegion()).findByText("Aniya Von");

    const user = userEvent.setup();
    await user.click(screen.getByRole("link", { name: "Back" }));

    expect(await screen.findByText("Card list")).toBeTruthy();
    expect(router.state.location.pathname).toBe("/cards");
  });
});

// A pair of route keys the COCRDSLC edits reject, and each key's text.
type KeyCase = {
  name: string;
  entry: string;
  account: string | null;
  card: string | null;
};

// Values of digits or blanks only: a key holding any other character is the
// unpinned class of E-47.
const KEY_CASES: KeyCase[] = [
  {
    name: "an absent account number (V-COCRDSLC-001)",
    entry: `/cards/${CARD_NUMBER}`,
    account: "Account number not provided",
    card: null,
  },
  {
    name: "an all-zero account number (V-COCRDSLC-001)",
    entry: `/cards/${CARD_NUMBER}?account_id=00000000000`,
    account: "Account number not provided",
    card: null,
  },
  {
    name: "an all-zero card number (V-COCRDSLC-003)",
    entry: `/cards/0000000000000000?account_id=${ACCOUNT_ID}`,
    account: null,
    card: "Card number not provided",
  },
  {
    name: "both keys blank (V-COCRDSLC-005)",
    entry: "/cards/0000000000000000",
    account: "No input received",
    card: "Card number not provided",
  },
];

// The key edit text under `label`, or `null` when the key passed.
function keyError(label: string): string | null {
  const alert = within(valueOf(label)).queryByRole("alert");
  return alert === null ? null : alert.textContent;
}

describe("CardViewPage with keys the edits reject", () => {
  it.each(KEY_CASES)(
    "sends nothing and shows the key texts for $name",
    async ({ entry, account, card }) => {
      const error = vi.spyOn(toast, "error");
      const network = stubNetwork(() => Response.json(FOUND));
      renderPage(entry);

      await screen.findByRole("heading", { level: 1, name: TITLE });
      await settle();

      expect(network).not.toHaveBeenCalled();
      expect(keyError("Account Number")).toBe(account);
      expect(keyError("Card Number")).toBe(card);
      const region = cardRegion();
      expect(region.getAttribute("aria-busy")).toBe("false");
      expect(within(region).queryByText("Name on card")).toBeNull();
      expect(error).not.toHaveBeenCalled();
    },
  );
});

describe("CardViewPage when the read fails", () => {
  it.each<{ field: CardViewField }>([{ field: "ACCTSID" }, { field: "CARDSID" }])(
    "toasts the message and keeps it as the message line, the error naming $field (V-COCRDSLC-006)",
    async ({ field }) => {
      const error = vi.spyOn(toast, "error");
      const body: ErrorBody = {
        code: "NOT_FOUND",
        message: NOT_FOUND,
        fields: [{ field, message: NOT_FOUND }],
      };
      const network = stubNetwork(() => Response.json(body, { status: 404 }));
      renderPage(VALID_ENTRY);

      await waitFor(() => {
        expect(error).toHaveBeenCalledTimes(1);
      });
      expect(error).toHaveBeenCalledWith(NOT_FOUND);
      const notifications = screen.getByRole("region", {
        name: /^Notifications/,
      });
      expect(await within(notifications).findByText(NOT_FOUND)).toBeTruthy();

      const region = cardRegion();
      expect(within(region).getByText(NOT_FOUND).textContent).toBe(NOT_FOUND);
      // ACCTSIDO and CARDSIDO keep the keys as entered; no card field shows.
      expect(valueOf("Account Number").textContent).toBe(ACCOUNT_ID);
      expect(valueOf("Card Number").textContent).toBe(CARD_NUMBER);
      expect(within(region).queryByText("Name on card")).toBeNull();
      expect(region.getAttribute("aria-busy")).toBe("false");
      // The named field has no input on this page to mark or to focus.
      expect(within(region).queryByRole("alert")).toBeNull();
      expect(document.activeElement).toBe(document.body);

      const [request] = sent(network);
      expect(new URL(request.url).pathname).toBe(`/api/cards/${CARD_NUMBER}`);
      expect(network).toHaveBeenCalledTimes(1);
    },
  );
});
