// FN tests of the CCLI card list (CardListPage; COCRDLIC, map COCRDLI). ENTER
// ("Search", F-COCRDLI-001) sends the account filter as entered; a filter the
// client schema rejects sends no request (V-COCRDLIC-001, V-COCRDLIC-002),
// whichever key was pressed, and takes the listed cards and paging keys off
// the screen, its text standing in their place; and a 400 the server
// answers at ACCTSID reaches the user as a toast, as the filter's field error
// and as the cursor on the filter, the COBOL `MOVE -1 TO ACCTSIDL`. A return
// to the list shows the page read before at once, not busy, while it is read
// again.
//
// The network is the global fetch the API client looks up on every request,
// stubbed per test, and the assertions read the Request objects it received.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  vi,
  type Mock,
} from "vitest";

import type { components } from "@/api/schema";

import CardListPage from "./CardListPage";

type CardPage = components["schemas"]["Page_CardSummary"];
type ErrorBody = components["schemas"]["ErrorBody"];
type FetchImpl = (input: Request) => Promise<Response>;
type FetchMock = Mock<FetchImpl>;

// The first record of app/data/ASCII/cardxref.txt and carddata.txt.
const FIRST_CARD = "0500024453765740";
const FIRST_ACCOUNT = "00000000050";

// An 11-digit account no fixture record holds: valid to the schema.
const UNKNOWN_ACCOUNT = "99999999999";

// COCRDLIC 2210-EDIT-ACCOUNT's text (V-COCRDLIC-001), byte for byte.
const ACCOUNT_NOT_NUMERIC =
  "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER";

// COCRDLIC 2220-EDIT-CARD's text (V-COCRDLIC-002), byte for byte.
const CARD_NOT_NUMERIC = "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER";

const PAGE: CardPage = {
  items: [
    {
      card_number: FIRST_CARD,
      account_id: FIRST_ACCOUNT,
      card_active_status: "Y",
    },
  ],
  next_key: null,
  prev_key: null,
};

// The same first page with a page after it, which starts at the next card of
// carddata.txt in card-number order, so Next is enabled.
const PAGE_WITH_NEXT: CardPage = { ...PAGE, next_key: "0683586198171516" };

const ACCOUNT_FILTER_ERROR: ErrorBody = {
  code: "INVALID",
  message: ACCOUNT_NOT_NUMERIC,
  fields: [{ field: "ACCTSID", message: ACCOUNT_NOT_NUMERIC }],
};

let fetchMock: FetchMock;

function jsonResponse(body: CardPage | ErrorBody, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

// A promise and its resolver, so a test decides when an answer arrives.
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

// Every request the page sent, in the order sent.
function requests(): Request[] {
  return fetchMock.mock.calls.map(([request]) => request);
}

// The requests for `GET /cards`, under the client's `/api` base path.
function cardListRequests(): Request[] {
  return requests().filter(
    (request) => new URL(request.url).pathname === "/api/cards",
  );
}

function queryOf(request: Request): URLSearchParams {
  return new URL(request.url).searchParams;
}

// The cell texts of the card table's shown body rows, top to bottom, without
// the filler rows, hidden from assistive technology, that pad a short page.
function cardRows(): string[][] {
  const table = screen.getByRole("table", { name: "Credit cards" });
  return Array.from(
    table.querySelectorAll("tbody tr:not([aria-hidden='true'])"),
    (row) =>
      Array.from(row.querySelectorAll("td"), (cell) => cell.textContent ?? ""),
  );
}

// The FieldError that describes `input`: the page links an invalid filter to
// its error through `aria-describedby`.
function fieldError(input: HTMLElement): HTMLElement {
  const id = input.getAttribute("aria-describedby");
  const error = id === null ? null : document.getElementById(id);
  if (error === null) {
    throw new Error("the input is described by no field error");
  }
  expect(error.getAttribute("role")).toBe("alert");
  return error;
}

// Renders the page at /cards in a data router, which PageHeader's
// useMatches needs, and waits for the first entry: one unfiltered request,
// listing every card from the lowest card number, whose row is shown.
async function renderPage(): Promise<{
  user: UserEvent;
  accountInput: HTMLInputElement;
  router: ReturnType<typeof createMemoryRouter>;
  queryClient: QueryClient;
}> {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(
    [
      {
        path: "/cards",
        element: <CardListPage />,
        handle: { crumb: "Credit Cards", title: "List Credit Cards" },
      },
      {
        path: "/cards/:cardNumber",
        element: <p>view</p>,
        handle: { crumb: "View" },
      },
      {
        path: "/cards/:cardNumber/edit",
        element: <p>edit</p>,
        handle: { crumb: "Edit" },
      },
      { path: "/", element: <p>home</p>, handle: { crumb: "Home" } },
    ],
    { initialEntries: ["/cards"] },
  );
  const user = userEvent.setup();
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );

  await screen.findByText(FIRST_CARD);
  const [first] = cardListRequests();
  expect(cardListRequests()).toHaveLength(1);
  expect(first.method).toBe("GET");
  expect([...queryOf(first).keys()]).toEqual([]);

  return {
    user,
    accountInput: screen.getByLabelText<HTMLInputElement>("Account number"),
    router,
    queryClient,
  };
}

beforeEach(() => {
  fetchMock = vi.fn<FetchImpl>(async () => jsonResponse(PAGE));
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("CardListPage (CCLI, COCRDLI)", () => {
  it("sends a valid account filter with GET /cards on ENTER", async () => {
    const { user, accountInput } = await renderPage();

    await user.type(accountInput, `${FIRST_ACCOUNT}{Enter}`);

    await waitFor(() => {
      expect(cardListRequests()).toHaveLength(2);
    });
    const [, filtered] = cardListRequests();
    expect(filtered.method).toBe("GET");
    const query = queryOf(filtered);
    expect(query.get("account_id")).toBe(FIRST_ACCOUNT);
    // ENTER reads forward; the blank card filter is no filter.
    expect(query.get("direction")).toBe("forward");
    expect(query.has("card_number")).toBe(false);
    expect(accountInput.getAttribute("aria-invalid")).toBe("false");
  });

  it("sends no request for an account filter that is not 11 digits", async () => {
    const { user, accountInput } = await renderPage();

    await user.type(accountInput, "ABC");
    await user.click(screen.getByRole("button", { name: "Search" }));

    const error = await waitFor(() => fieldError(accountInput));
    expect(error.textContent).toBe(ACCOUNT_NOT_NUMERIC);
    expect(accountInput.getAttribute("aria-invalid")).toBe("true");
    // The rejected filter takes the cursor, as INPUT-ERROR places it.
    await waitFor(() => {
      expect(document.activeElement).toBe(accountInput);
    });
    expect(
      cardListRequests().filter((request) =>
        queryOf(request).has("account_id"),
      ),
    ).toEqual([]);
    expect(cardListRequests()).toHaveLength(1);
  });

  it.each([
    {
      why: "SQL text in the account filter",
      key: "Search",
      label: "Account number",
      entry: "' OR 1=1 --",
      message: ACCOUNT_NOT_NUMERIC,
      listed: PAGE,
    },
    {
      why: "an account filter of fewer than 11 digits",
      key: "Search",
      label: "Account number",
      entry: "123",
      message: ACCOUNT_NOT_NUMERIC,
      listed: PAGE,
    },
    {
      why: "a card filter that is not 16 digits",
      key: "Next",
      label: "Card number",
      entry: "050002445376574X",
      message: CARD_NOT_NUMERIC,
      listed: PAGE_WITH_NEXT,
    },
  ] as const)(
    "takes the listed cards off the screen when $key is pressed with $why, and lists them again on a valid Search",
    async ({ key, label, entry, message, listed }) => {
      fetchMock.mockImplementation(async () => jsonResponse(listed));
      const { user } = await renderPage();
      const input = screen.getByLabelText<HTMLInputElement>(label);
      expect(cardRows().map((cells) => cells[1])).toEqual([FIRST_CARD]);
      expect(screen.getByRole("button", { name: "Next" })).toBeDefined();

      await user.type(input, entry);
      await user.click(screen.getByRole("button", { name: key }));

      const error = await waitFor(() => fieldError(input));
      expect(error.textContent).toBe(message);
      // The filter's text stands in place of the rows, and no paging key is
      // offered under it: both controls stay shown, disabled.
      await waitFor(() => {
        expect(cardRows()).toEqual([[message]]);
      });
      expect(screen.queryByText(FIRST_CARD)).toBeNull();
      expect(screen.queryByRole("link", { name: "View" })).toBeNull();
      for (const name of ["Previous", "Next"]) {
        expect(
          screen.getByRole("button", { name }).getAttribute("aria-disabled"),
        ).toBe("true");
      }
      expect(cardListRequests()).toHaveLength(1);

      await user.clear(input);
      await user.click(screen.getByRole("button", { name: "Search" }));

      await waitFor(() => {
        expect(cardRows().map((cells) => cells[1])).toEqual([FIRST_CARD]);
      });
      expect(input.getAttribute("aria-invalid")).toBe("false");
      const sent = cardListRequests();
      expect(sent).toHaveLength(2);
      // Search reads forward from the first card shown, which the page's
      // null prev_key leaves unset, with no filter.
      expect([...queryOf(sent[1]).entries()]).toEqual([
        ["direction", "forward"],
      ]);
    },
  );

  it("shows a 400 at ACCTSID as a toast, a field error and the cursor on the filter", async () => {
    const errorSpy = vi.spyOn(toast, "error");
    // The filtered answer waits until the test has seen the cursor leave the
    // filter, so the focus assertion below proves the error moved it back.
    const filteredAnswer = deferred<Response>();
    fetchMock.mockImplementation((request) =>
      queryOf(request).has("account_id")
        ? filteredAnswer.promise
        : Promise.resolve(jsonResponse(PAGE)),
    );
    const { user, accountInput } = await renderPage();

    await user.type(accountInput, UNKNOWN_ACCOUNT);
    const search = screen.getByRole("button", { name: "Search" });
    await user.click(search);

    await waitFor(() => {
      expect(cardListRequests()).toHaveLength(2);
    });
    const [, filtered] = cardListRequests();
    expect(filtered.method).toBe("GET");
    expect(queryOf(filtered).get("account_id")).toBe(UNKNOWN_ACCOUNT);
    expect(document.activeElement).toBe(search);
    expect(errorSpy).not.toHaveBeenCalled();

    filteredAnswer.resolve(jsonResponse(ACCOUNT_FILTER_ERROR, 400));

    await waitFor(() => {
      expect(errorSpy).toHaveBeenCalledWith(ACCOUNT_NOT_NUMERIC);
    });
    expect(errorSpy).toHaveBeenCalledTimes(1);
    const error = await waitFor(() => fieldError(accountInput));
    expect(error.textContent).toBe(ACCOUNT_NOT_NUMERIC);
    expect(accountInput.getAttribute("aria-invalid")).toBe("true");
    await waitFor(() => {
      expect(document.activeElement).toBe(accountInput);
    });
  });

  it("shows the page read before at once, not busy, when the list is opened again", async () => {
    const { user, router, queryClient } = await renderPage();
    await user.click(screen.getByRole("link", { name: "View" }));
    await screen.findByText("view");

    // The read the return starts is held, so only the cached page can show.
    const held = deferred<Response>();
    fetchMock.mockImplementation(() => held.promise);
    await act(async () => {
      await router.navigate("/cards");
    });

    const table = screen.getByRole("table");
    await waitFor(() => {
      expect(cardListRequests()).toHaveLength(2);
    });
    expect(within(table).getByText(FIRST_CARD).tagName).toBe("SPAN");
    expect(table.hasAttribute("aria-busy")).toBe(false);
    expect(table.querySelector('[data-slot="skeleton"]')).toBeNull();

    held.resolve(jsonResponse(PAGE));
    await waitFor(() => {
      expect(queryClient.isFetching()).toBe(0);
    });
    expect(table.hasAttribute("aria-busy")).toBe(false);
    expect(within(table).getByText(FIRST_CARD).tagName).toBe("SPAN");
  });

  // The Active column's flag as a badge carrying its letter: N, the inactive
  // state, is destructive.
  it.each([
    { flag: "Y", variant: "secondary" },
    { flag: "N", variant: "destructive" },
  ])(
    "shows the Active flag $flag as a $variant badge",
    async ({ flag, variant }) => {
      fetchMock.mockImplementation(async () =>
        jsonResponse({
          ...PAGE,
          items: PAGE.items.map((card) => ({
            ...card,
            card_active_status: flag,
          })),
        }),
      );
      await renderPage();

      const row = screen.getByText(FIRST_CARD).closest("tr");
      const badge = row?.querySelector('[data-slot="badge"]');
      expect(badge?.textContent).toBe(flag);
      expect(badge?.getAttribute("data-variant")).toBe(variant);
    },
  );
});
