// The cache entries each write hook marks stale once its request settles:
// the record written and the lists that can show it, never another record's
// entry. Every test seeds entries with no observer, runs the mutation against
// the stubbed network and reads which of them the hook invalidated. Without
// an observer nothing is read again, so the only request sent is the write.
//
// Covered: the card update (CCUP), the user add, update and delete (CU01 to
// CU03), the fraud mark (CPVD PF5), the transaction add (CT02) and the bill
// payment (CB00).
import {
  QueryClient,
  QueryClientProvider,
  type QueryKey,
} from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { RequestTimeoutError } from "@/api/client";
import type { components } from "@/api/schema";

import { accountKeys } from "./accounts";
import {
  useUserAddMutation,
  useUserDeleteMutation,
  userKeys,
  useUserUpdateMutation,
} from "./admin-users";
import { authorizationKeys, useFraudToggleMutation } from "./authorizations";
import { cardKeys, useCardUpdateMutation } from "./cards";
import { usePaymentMutation } from "./payments";
import { rewardKeys } from "./rewards";
import { transactionKeys, useTransactionAddMutation } from "./transactions";

type Schemas = components["schemas"];

// Accounts 1 and 50 and the first card of account 50 in app/data/ASCII
// (acctdata.txt, carddata.txt); the other card is account 1's.
const ACCOUNT = "00000000050";
const OTHER_ACCOUNT = "00000000001";
const CARD = "0500024453765740";
const OTHER_CARD = "4859452612877065";

// A TRAN-ID the add or payment answers with, and one already on file.
const NEW_TRAN_ID = "0000000000000301";
const OLD_TRAN_ID = "0000000000000017";

// The users of the DUSRSECJ seed, and one an add creates.
const USER = "USER0001";
const OTHER_USER = "ADMIN001";
const NEW_USER = "NEWUSR01";

/**
 * What the stubbed network answers the one write a test sends: a JSON body;
 * a body sent as the raw `text`, for one the client cannot read as JSON; a
 * network failure; or nothing before the client's deadline.
 */
type Reply =
  | { readonly status: number; readonly body: unknown }
  | {
      readonly status: number;
      readonly text: string;
      readonly headers?: Readonly<Record<string, string>>;
    }
  | "network failure"
  | "deadline";

let queryClient: QueryClient;
// Every request sent, as "METHOD /path".
let sent: string[];
let reply: Reply;

beforeEach(() => {
  queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  sent = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (request: Request) => {
      sent.push(`${request.method} ${new URL(request.url).pathname}`);
      if (reply === "network failure") {
        throw new TypeError("Failed to fetch");
      }
      if (reply === "deadline") {
        throw new RequestTimeoutError();
      }
      if ("text" in reply) {
        return new Response(reply.text, {
          status: reply.status,
          headers: reply.headers,
        });
      }
      return Response.json(reply.body, { status: reply.status });
    }),
  );
});

afterEach(() => {
  queryClient.clear();
  vi.unstubAllGlobals();
});

function wrapper({ children }: { children: ReactNode }) {
  return (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

/** Caches `keys`, each as an entry no observer reads. */
function seed(keys: readonly QueryKey[]): void {
  for (const key of keys) {
    queryClient.setQueryData(key, { seeded: key });
  }
}

/** The entries of `keys` marked stale, in the order given. */
function invalidated(keys: readonly QueryKey[]): QueryKey[] {
  return keys.filter(
    (key) => queryClient.getQueryState(key)?.isInvalidated === true,
  );
}

/**
 * Runs `mutate` to its end, `reply` answering its request, and reports
 * whether it resolved. Its success and settle callbacks have finished then,
 * since the mutation settles only once their promises have.
 */
async function settle(mutate: () => Promise<unknown>): Promise<boolean> {
  let resolved = false;
  await act(async () => {
    resolved = await mutate().then(
      () => true,
      () => false,
    );
  });
  return resolved;
}

/**
 * Whether the hook resolves on `answer`: a 2xx JSON body does; a failure, a
 * body the client cannot read and a request without an answer reject.
 */
function resolvesOn(answer: Reply): boolean {
  return typeof answer === "object" && "body" in answer && answer.status < 400;
}

function refusal(code: string, message: string): Schemas["ErrorBody"] {
  return { code, message };
}

describe("useCardUpdateMutation (CCUP)", () => {
  const LISTS = [cardKeys.list({}), cardKeys.list({ account_id: OTHER_ACCOUNT })];
  // The card as read with its own account and with another one: COCRDSLC
  // edits both keys, so each pair is an entry of its own.
  const DETAILS = [
    cardKeys.detail(CARD, ACCOUNT),
    cardKeys.detail(CARD, OTHER_ACCOUNT),
  ];
  const OTHER_DETAIL = cardKeys.detail(OTHER_CARD, OTHER_ACCOUNT);
  const SEEDED = [...LISTS, ...DETAILS, OTHER_DETAIL];

  const card: Schemas["CardResponse"] = {
    account_id: ACCOUNT,
    card_number: CARD,
    card_embossed_name: "Aniya Vonn",
    card_expiraion_date: "2023-03-09",
    card_active_status: "Y",
    version: "fedcba9876543210",
  };

  const OUTCOMES: readonly {
    outcome: string;
    reply: Reply;
    stale: readonly QueryKey[];
  }[] = [
    {
      outcome: "the rewrite (200)",
      reply: {
        status: 200,
        body: {
          card,
          message: "Changes committed to database",
        } satisfies Schemas["CardUpdateResponse"],
      },
      stale: [...LISTS, ...DETAILS],
    },
    {
      outcome: "a changed record (409)",
      reply: {
        status: 409,
        body: refusal(
          "CONFLICT",
          "Record changed by some one else. Please review",
        ),
      },
      stale: DETAILS,
    },
    {
      outcome: "a refusal before the rewrite (400, V-COCRDUPC-001)",
      reply: { status: 400, body: refusal("INVALID", "") },
      stale: [],
    },
  ];

  it.each(OUTCOMES)(
    "after $outcome marks only the card's own entries stale",
    async ({ reply: answer, stale }) => {
      seed(SEEDED);
      reply = answer;
      const { result } = renderHook(() => useCardUpdateMutation(CARD), {
        wrapper,
      });

      expect(
        await settle(() =>
          result.current.mutateAsync({
            account_id: ACCOUNT,
            embossed_name: card.card_embossed_name,
            active_status: "Y",
            expiry_month: "03",
            expiry_year: "2023",
            version: "0123456789abcdef",
          }),
        ),
      ).toBe(resolvesOn(answer));

      expect(invalidated(SEEDED)).toEqual(stale);
      expect(sent).toEqual([`PUT /api/cards/${CARD}`]);
    },
  );
});

describe("user administration (CU01 to CU03)", () => {
  const LISTS = [userKeys.list({}), userKeys.list({ start_key: OTHER_USER })];

  function answered(user: Schemas["UserSummary"], message: string): Reply {
    return {
      status: 200,
      body: { user, message } satisfies Schemas["UserMessageResponse"],
    };
  }

  it("marks the lists and the added user's detail stale after an add", async () => {
    // The ID read before it existed, as a lookup of it caches it.
    const added = userKeys.detail(NEW_USER);
    const other = userKeys.detail(USER);
    const seeded = [...LISTS, added, other];
    seed(seeded);
    reply = answered(
      { user_id: NEW_USER, first_name: "Ada", last_name: "Byron", usr_type: "U" },
      `User ${NEW_USER} has been added ...`,
    );
    const { result } = renderHook(() => useUserAddMutation(), { wrapper });

    expect(
      await settle(() =>
        result.current.mutateAsync({
          user_id: NEW_USER,
          first_name: "Ada",
          last_name: "Byron",
          password: "PASSWORD",
          usr_type: "U",
        }),
      ),
    ).toBe(true);

    expect(invalidated(seeded)).toEqual([...LISTS, added]);
    expect(sent).toEqual(["POST /api/admin/users"]);
  });

  it("marks the lists and the updated user's detail stale after an update", async () => {
    const updated = userKeys.detail(USER);
    const other = userKeys.detail(OTHER_USER);
    const seeded = [...LISTS, updated, other];
    seed(seeded);
    reply = answered(
      { user_id: USER, first_name: "Ada", last_name: "Byron", usr_type: "U" },
      `User ${USER} has been updated ...`,
    );
    const { result } = renderHook(() => useUserUpdateMutation(USER), {
      wrapper,
    });

    expect(
      await settle(() =>
        result.current.mutateAsync({ first_name: "Ada", last_name: "Byron" }),
      ),
    ).toBe(true);

    expect(invalidated(seeded)).toEqual([...LISTS, updated]);
    expect(sent).toEqual([`PUT /api/admin/users/${USER}`]);
  });

  it("drops the deleted user's detail and marks only the lists stale after a delete", async () => {
    const deleted = userKeys.detail(USER);
    const other = userKeys.detail(OTHER_USER);
    seed([...LISTS, deleted, other]);
    reply = answered(
      { user_id: USER, first_name: "Ada", last_name: "Byron", usr_type: "U" },
      `User ${USER} has been deleted ...`,
    );
    const { result } = renderHook(() => useUserDeleteMutation(), { wrapper });

    expect(await settle(() => result.current.mutateAsync(USER))).toBe(true);

    expect(queryClient.getQueryState(deleted)).toBeUndefined();
    expect(invalidated([...LISTS, other])).toEqual(LISTS);
    expect(sent).toEqual([`DELETE /api/admin/users/${USER}`]);
  });
});

describe("useFraudToggleMutation (CPVD PF5)", () => {
  const AUTH_ID = "7";

  const detail: Schemas["AuthDetailResponse"] = {
    auth_id: AUTH_ID,
    account_id: ACCOUNT,
    card_number: CARD,
    pa_auth_orig_date: "220701",
    pa_auth_orig_time: "120000",
    approved: "A",
    pa_auth_resp_code: "00",
    pa_auth_resp_reason: "0000",
    decline_reason: "",
    pa_processing_code: "000000",
    pa_approved_amt: "50.47",
    pa_pos_entry_mode: "01",
    pa_message_source: "POS",
    pa_merchant_catagory_code: "5411",
    pa_card_expiry_date: "0325",
    pa_auth_type: "0100",
    pa_transaction_id: "220701120000001",
    pa_match_status: "P",
    pa_auth_fraud: "F",
    pa_fraud_rpt_date: "07/01/22",
    pa_merchant_name: "GROCERY",
    pa_merchant_id: "000000000000001",
    pa_merchant_city: "SEATTLE",
    pa_merchant_state: "WA",
    pa_merchant_zip: "98101",
    next_id: null,
  };

  it("marks the summary pages of the authorization's account stale, and no other account's", async () => {
    const own = [
      authorizationKeys.summary({ account_id: ACCOUNT }),
      authorizationKeys.summary({
        account_id: ACCOUNT,
        start_key: "5",
        direction: "forward",
      }),
    ];
    const other = authorizationKeys.summary({ account_id: OTHER_ACCOUNT });
    const seeded = [...own, other];
    seed(seeded);
    reply = {
      status: 200,
      body: {
        detail,
        message: "AUTH MARKED FRAUD...",
      } satisfies Schemas["FraudToggleResponse"],
    };
    const { result } = renderHook(() => useFraudToggleMutation(), {
      wrapper,
    });

    expect(await settle(() => result.current.mutateAsync(AUTH_ID))).toBe(true);

    expect(invalidated(seeded)).toEqual(own);
    expect(sent).toEqual([`PUT /api/authorizations/${AUTH_ID}`]);
  });

  // The detail shown before the mark, with the PF8 neighbour its read found.
  const shown: Schemas["AuthDetailResponse"] = {
    ...detail,
    pa_auth_fraud: "",
    pa_fraud_rpt_date: "",
    next_id: "6",
  };

  // The server's parse reads an optional `+` and leading zeros, so each key
  // names authorization 7, the `auth_id` the PUT answers with: the replaced
  // detail lands under the key the page reads, with the neighbour it showed.
  it.each([AUTH_ID, `+000${AUTH_ID}`, `000${AUTH_ID}`])(
    "writes the replaced detail under the requested key %s, keeping the shown next_id",
    async (requested) => {
      const key = authorizationKeys.detail(requested);
      queryClient.setQueryData(key, shown);
      reply = {
        status: 200,
        body: {
          detail,
          message: "AUTH MARKED FRAUD...",
        } satisfies Schemas["FraudToggleResponse"],
      };
      const { result } = renderHook(() => useFraudToggleMutation(), {
        wrapper,
      });

      expect(await settle(() => result.current.mutateAsync(requested))).toBe(true);

      expect(queryClient.getQueryData(key)).toEqual({ ...detail, next_id: "6" });
      expect(invalidated([key])).toEqual([]);
      expect(sent).toEqual([
        `PUT /api/authorizations/${encodeURIComponent(requested)}`,
      ]);
    },
  );

  it("never writes a detail of another authorization under the requested key", async () => {
    const key = authorizationKeys.detail(AUTH_ID);
    const answeredKey = authorizationKeys.detail("8");
    queryClient.setQueryData(key, shown);
    reply = {
      status: 200,
      body: {
        detail: { ...detail, auth_id: "8" },
        message: "AUTH MARKED FRAUD...",
      } satisfies Schemas["FraudToggleResponse"],
    };
    const { result } = renderHook(() => useFraudToggleMutation(), {
      wrapper,
    });

    expect(await settle(() => result.current.mutateAsync(AUTH_ID))).toBe(true);

    // The shown detail stays, marked stale so it is read afresh; nothing is
    // cached under the answer's own key either.
    expect(queryClient.getQueryData(key)).toEqual(shown);
    expect(invalidated([key])).toEqual([key]);
    expect(queryClient.getQueryState(answeredKey)).toBeUndefined();
    expect(sent).toEqual([`PUT /api/authorizations/${AUTH_ID}`]);
  });
});

// The transaction caches a CT02 add or a CB00 payment can reach. Lists name
// the dashboard's account filter (`account_id`, backward, five rows) or none;
// a blank filter is no filter to the server.
const UNFILTERED_LISTS = [
  transactionKeys.list({}),
  transactionKeys.list({ start_key: OLD_TRAN_ID, direction: "forward" }),
  transactionKeys.list({ account_id: " " }),
];
const ACCOUNT_LIST = transactionKeys.list({
  account_id: ACCOUNT,
  direction: "backward",
  page_size: 5,
});
const OTHER_ACCOUNT_LIST = transactionKeys.list({
  account_id: OTHER_ACCOUNT,
  direction: "backward",
  page_size: 5,
});
const NEW_DETAIL = transactionKeys.detail(NEW_TRAN_ID);
const OLD_DETAIL = transactionKeys.detail(OLD_TRAN_ID);
const TRANSACTION_CACHES = [
  ...UNFILTERED_LISTS,
  ACCOUNT_LIST,
  OTHER_ACCOUNT_LIST,
  NEW_DETAIL,
  OLD_DETAIL,
];

describe("useTransactionAddMutation (CT02)", () => {
  const ACCOUNT_REWARDS = [
    rewardKeys.balance(ACCOUNT),
    rewardKeys.history(ACCOUNT, {}),
  ];
  const OTHER_REWARDS = [
    rewardKeys.balance(OTHER_ACCOUNT),
    rewardKeys.history(OTHER_ACCOUNT, {}),
  ];
  const SEEDED = [...TRANSACTION_CACHES, ...ACCOUNT_REWARDS, ...OTHER_REWARDS];
  // What an account-keyed add that may have written reloads when no answer
  // names its TRAN-ID: the lists that can show the new row and the account's
  // reward balance and history.
  const RELOADED = [...UNFILTERED_LISTS, ACCOUNT_LIST, ...ACCOUNT_REWARDS];

  const ENTRY: Schemas["TransactionAddRequest"] = {
    tran_type_cd: "01",
    tran_cat_cd: "0001",
    tran_source: "POS TERM",
    tran_desc: "Purchase at Grocery",
    tran_amt: "-00000012.34",
    tran_orig_date: "2022-07-01",
    tran_proc_date: "2022-07-01",
    tran_merchant_id: "000000001",
    tran_merchant_name: "Grocery",
    tran_merchant_city: "Seattle",
    tran_merchant_zip: "98101",
    confirm: "Y",
  };

  const added: Schemas["TransactionAddResponse"] = {
    transaction: {
      tran_id: NEW_TRAN_ID,
      tran_type_cd: "01",
      tran_cat_cd: "0001",
      tran_source: "POS TERM",
      tran_desc: "Purchase at Grocery",
      tran_amt: "-12.34",
      tran_merchant_id: "000000001",
      tran_merchant_name: "Grocery",
      tran_merchant_city: "Seattle",
      tran_merchant_zip: "98101",
      tran_card_num: CARD,
      tran_orig_ts: "2022-07-01 00:00:00.000000",
      tran_proc_ts: "2022-07-01 00:00:00.000000",
    },
    message: `Transaction added successfully.  Your Tran ID is ${NEW_TRAN_ID}.`,
  };

  const BY_ACCOUNT = { account_id: ACCOUNT, card_number: "" };

  const CASES: readonly {
    entry: string;
    keys: Pick<Schemas["TransactionAddRequest"], "account_id" | "card_number">;
    confirm: string;
    reply: Reply;
    stale: readonly QueryKey[];
  }[] = [
    {
      entry: "an account ID, added (201)",
      keys: BY_ACCOUNT,
      confirm: "Y",
      reply: { status: 201, body: added },
      stale: [...UNFILTERED_LISTS, ACCOUNT_LIST, NEW_DETAIL, ...ACCOUNT_REWARDS],
    },
    // The account comes from the cross-reference, which the answer omits.
    {
      entry: "a card number alone, added (201)",
      keys: { account_id: "   ", card_number: CARD },
      confirm: "Y",
      reply: { status: 201, body: added },
      stale: [
        ...UNFILTERED_LISTS,
        ACCOUNT_LIST,
        OTHER_ACCOUNT_LIST,
        NEW_DETAIL,
        ...ACCOUNT_REWARDS,
        ...OTHER_REWARDS,
      ],
    },
    // The server's own 500, 'Unable to Add Transaction...' (V-COTRN02C-033),
    // persists nothing.
    {
      entry:
        "an account ID, refused with a 500 the client cannot tell from a proxy's after a commit",
      keys: BY_ACCOUNT,
      confirm: "Y",
      reply: {
        status: 500,
        body: refusal("UNEXPECTED", "Unable to Add Transaction..."),
      },
      stale: RELOADED,
    },
    // What the Vite proxy answers when the API's connection ends before its
    // answer, the add committed or not.
    {
      entry: "an account ID, answered by the proxy's empty 500",
      keys: BY_ACCOUNT,
      confirm: "Y",
      reply: {
        status: 500,
        text: "",
        headers: { "Content-Type": "text/plain" },
      },
      stale: RELOADED,
    },
    {
      entry: "an account ID, added (201) without a body",
      keys: BY_ACCOUNT,
      confirm: "Y",
      reply: {
        status: 201,
        text: "",
        headers: { "Content-Type": "application/json" },
      },
      stale: RELOADED,
    },
    {
      entry: "an account ID, added (201) with a truncated body",
      keys: BY_ACCOUNT,
      confirm: "Y",
      reply: {
        status: 201,
        text: '{"transaction":',
        headers: { "Content-Type": "application/json" },
      },
      stale: RELOADED,
    },
    {
      entry: "an account ID, confirmed and sent without an answer",
      keys: BY_ACCOUNT,
      confirm: "Y",
      reply: "network failure",
      stale: RELOADED,
    },
    {
      entry: "an account ID, confirmed with y and past the client's deadline",
      keys: BY_ACCOUNT,
      confirm: "y",
      reply: "deadline",
      stale: RELOADED,
    },
    {
      entry: "an account ID, cancelled with N (400, V-COTRN02C-027)",
      keys: BY_ACCOUNT,
      confirm: "N",
      reply: {
        status: 400,
        body: refusal("INVALID", "Confirm to add this transaction..."),
      },
      stale: [],
    },
    {
      entry: "an account ID, refused by an empty master (404, V-COTRN02C-029, E-27)",
      keys: BY_ACCOUNT,
      confirm: "Y",
      reply: {
        status: 404,
        body: refusal("NOT_FOUND", "Transaction ID NOT found..."),
      },
      stale: [],
    },
    {
      entry: "an account ID, refused by a live TRAN-ID (409, V-COTRN02C-032)",
      keys: BY_ACCOUNT,
      confirm: "Y",
      reply: {
        status: 409,
        body: refusal("DUPLICATE", "Tran ID already exist..."),
      },
      stale: [],
    },
    // PF5's check sends CONFIRM blank, which COTRN02C never adds on.
    {
      entry: "an account ID, checked with a blank confirm and no answer",
      keys: BY_ACCOUNT,
      confirm: "",
      reply: "network failure",
      stale: [],
    },
  ];

  it.each(CASES)(
    "after $entry marks only what the insert may have changed stale",
    async ({ keys, confirm, reply: answer, stale }) => {
      seed(SEEDED);
      reply = answer;
      const { result } = renderHook(() => useTransactionAddMutation(), {
        wrapper,
      });

      expect(
        await settle(() =>
          result.current.mutateAsync({ ...ENTRY, ...keys, confirm }),
        ),
      ).toBe(resolvesOn(answer));

      expect(invalidated(SEEDED)).toEqual(stale);
      expect(sent).toEqual(["POST /api/transactions"]);
    },
  );
});

describe("usePaymentMutation (CB00)", () => {
  const ACCOUNT_DETAIL = accountKeys.detail(ACCOUNT);
  const OTHER_ACCOUNT_DETAIL = accountKeys.detail(OTHER_ACCOUNT);
  const SEEDED = [...TRANSACTION_CACHES, ACCOUNT_DETAIL, OTHER_ACCOUNT_DETAIL];
  // What a request that may have written reloads, the new TRAN-ID's detail
  // aside: the paid account and the lists that can show its payment.
  const RELOADED = [...UNFILTERED_LISTS, ACCOUNT_LIST, ACCOUNT_DETAIL];

  const prompt: Schemas["PaymentResponse"] = {
    account_id: ACCOUNT,
    current_balance: "194.00",
    message: "Confirm to make a bill payment...",
  };
  const paid: Schemas["PaymentResponse"] = {
    account_id: ACCOUNT,
    current_balance: "0.00",
    message: `Payment successful.  Your Transaction ID is ${NEW_TRAN_ID}.`,
    tran_id: NEW_TRAN_ID,
  };

  const CASES: readonly {
    outcome: string;
    confirm: string;
    reply: Reply;
    stale: readonly QueryKey[];
  }[] = [
    {
      outcome: "the blank-confirm prompt (200, V-COBIL00C-008)",
      confirm: "",
      reply: { status: 200, body: prompt },
      stale: [],
    },
    {
      outcome: "a blank-confirm missing account (404, V-COBIL00C-004)",
      confirm: "",
      reply: { status: 404, body: refusal("NOT_FOUND", "Account ID NOT found...") },
      stale: [],
    },
    {
      outcome: "a blank-confirm account read failure (500, V-COBIL00C-005)",
      confirm: "",
      reply: { status: 500, body: refusal("UNEXPECTED", "Unable to lookup Account...") },
      stale: [],
    },
    {
      outcome: "the N cancel (400, V-COBIL00C-002)",
      confirm: "N",
      reply: { status: 400, body: refusal("INVALID", "") },
      stale: [],
    },
    {
      outcome: "a blank-confirm request without an answer",
      confirm: "",
      reply: "network failure",
      stale: [],
    },
    {
      outcome: "the payment (201, P-COBIL00C-016)",
      confirm: "Y",
      reply: { status: 201, body: paid },
      stale: [...UNFILTERED_LISTS, ACCOUNT_LIST, NEW_DETAIL, ACCOUNT_DETAIL],
    },
    {
      outcome: "a confirmed nothing-to-pay (400, V-COBIL00C-007)",
      confirm: "Y",
      reply: { status: 400, body: refusal("INVALID", "You have nothing to pay...") },
      stale: [],
    },
    // The text of the failed account rewrite after the transaction write
    // (V-COBIL00C-016); the client cannot tell it from the refusals before
    // any write that share it (V-COBIL00C-004, V-COBIL00C-009).
    {
      outcome: "a confirmed 'Account ID NOT found...' (404, V-COBIL00C-016)",
      confirm: "y",
      reply: { status: 404, body: refusal("NOT_FOUND", "Account ID NOT found...") },
      stale: RELOADED,
    },
    {
      outcome: "a TRAN-ID collision (409, V-COBIL00C-014)",
      confirm: "Y",
      reply: { status: 409, body: refusal("DUPLICATE", "Tran ID already exist...") },
      stale: RELOADED,
    },
    {
      outcome: "a failed transaction insert (500, V-COBIL00C-015)",
      confirm: "Y",
      reply: {
        status: 500,
        body: refusal("UNEXPECTED", "Unable to Add Bill pay Transaction..."),
      },
      stale: RELOADED,
    },
    {
      outcome: "a failed account rewrite (500, V-COBIL00C-017)",
      confirm: "Y",
      reply: { status: 500, body: refusal("UNEXPECTED", "Unable to Update Account...") },
      stale: RELOADED,
    },
    {
      outcome: "a confirmed request without an answer",
      confirm: "Y",
      reply: "network failure",
      stale: RELOADED,
    },
  ];

  it.each(CASES)(
    "after $outcome reloads only what the payment may have changed",
    async ({ confirm, reply: answer, stale }) => {
      seed(SEEDED);
      reply = answer;
      const { result } = renderHook(() => usePaymentMutation(), { wrapper });

      expect(
        await settle(() =>
          result.current.mutateAsync({ account_id: ACCOUNT, confirm }),
        ),
      ).toBe(resolvesOn(answer));

      expect(invalidated(SEEDED)).toEqual(stale);
      expect(sent).toEqual(["POST /api/payments"]);
    },
  );
});
