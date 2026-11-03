// FN tests of the CCLI card list (CardListPage; COCRDLIC, map COCRDLI). ENTER
// ("Search", F-COCRDLI-001) sends the account filter as entered; a filter the
// client schema rejects sends no request (V-COCRDLIC-001); and a 400 the server
// answers at ACCTSID reaches the user as a toast, as the filter's field error
// and as the cursor on the filter, the COBOL `MOVE -1 TO ACCTSIDL`.
//
// The network is the global fetch the API client looks up on every request,
// stubbed per test, and the assertions read the Request objects it received.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
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

/** The first record of app/data/ASCII/cardxref.txt and carddata.txt. */
const FIRST_CARD = "0500024453765740";
const FIRST_ACCOUNT = "00000000050";

/** An 11-digit account no fixture record holds: valid to the schema. */
const UNKNOWN_ACCOUNT = "99999999999";

/** COCRDLIC 2210-EDIT-ACCOUNT's text (V-COCRDLIC-001), byte for byte. */
const ACCOUNT_NOT_NUMERIC =
  "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER";

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

/** A promise and its resolver, so a test decides when an answer arrives. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

/** Every request the page sent, in the order sent. */
function requests(): Request[] {
  return fetchMock.mock.calls.map(([request]) => request);
}

/** The requests for `GET /cards`, under the client's `/api` base path. */
function cardListRequests(): Request[] {
  return requests().filter(
    (request) => new URL(request.url).pathname === "/api/cards",
  );
}

function queryOf(request: Request): URLSearchParams {
  return new URL(request.url).searchParams;
}

/**
 * The FieldError that describes `input`: the page links an invalid filter to
 * its error through `aria-describedby`.
 */
function fieldError(input: HTMLElement): HTMLElement {
  const id = input.getAttribute("aria-describedby");
  const error = id === null ? null : document.getElementById(id);
  if (error === null) {
    throw new Error("the input is described by no field error");
  }
  expect(error.getAttribute("role")).toBe("alert");
  return error;
}

/**
 * Renders the page at /cards in a data router, which PageHeader's
 * useMatches needs, and waits for the first entry: one unfiltered request,
 * listing every card from the lowest card number, whose row is shown.
 */
async function renderPage(): Promise<{
  user: UserEvent;
  accountInput: HTMLInputElement;
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
});
