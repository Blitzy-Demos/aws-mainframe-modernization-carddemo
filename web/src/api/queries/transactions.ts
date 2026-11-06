/**
 * Query hooks of the transaction screens:
 * - CT00, the transaction list (COTRN00C, map COTRN00): `GET /transactions`,
 *   for TransactionListPage and the dashboard's five most recent transactions.
 * - CT01, one transaction (COTRN01C, map COTRN01): `GET /transactions/{id}`,
 *   for TransactionViewPage.
 * - CT02, the transaction add (COTRN02C, map COTRN02): `POST /transactions`,
 *   and its PF5 "Copy last transaction", for TransactionAddPage.
 *
 * TRAN-IDs, card and account numbers, codes and amounts stay the strings the
 * server sends: `TRAN-AMT S9(09)V99` arrives as a decimal string such as
 * `-12.34`. A non-2xx answer rejects with the client's ApiError, unchanged,
 * which the pages show through lib/api-errors.ts. The COBOL texts that are not
 * errors, the CT00 page-boundary line and the CT02 success text, arrive as
 * `message` in the data.
 */
import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
  type InvalidateQueryFilters,
} from "@tanstack/react-query";

import { ApiError, api, unwrap, unwrapWithStatus } from "@/api/client";
import { rewardKeys } from "@/api/queries/rewards";
import type { components, paths } from "@/api/schema";
import { isSpacesOrLowValues } from "@/schemas/primitives";

/**
 * The CT00 browse:
 * - `start_key`, the TRAN-ID a page starts at (`TRNIDINI X(16)`, or the
 *   `next_key`/`prev_key` of the page shown);
 * - `direction`, `forward` for ENTER and PF8, `backward` for PF7;
 * - `page_size`, 1 to 100, the 10 rows of COTRN00 when omitted;
 * - `account_id`, an optional filter COTRN00C does not have (E-17).
 */
export type TransactionListParams = NonNullable<
  paths["/transactions"]["get"]["parameters"]["query"]
>;

/** One `CVTRA05Y` transaction record, keyed by `tran_id`, as COTRN01 shows. */
export type Transaction = components["schemas"]["TransactionResponse"];

/** The COTRN02 input fields as entered, with `confirm` (`CONFIRMI X(1)`). */
export type TransactionAddBody = components["schemas"]["TransactionAddRequest"];

/** The added transaction and the COTRN02C success text with its Tran ID. */
export type TransactionAddResult =
  components["schemas"]["TransactionAddResponse"];

type TransactionPage = components["schemas"]["Page_TransactionResponse"];

/**
 * The PF5 answer: the highest-keyed live transaction, `undefined` when the
 * live master is empty, and the message the browse carried, if it had one.
 */
export type LastTransaction = {
  transaction: Transaction | undefined;
  message: TransactionPage["message"];
};

const all = ["transactions"] as const;

/**
 * Query keys of the transaction caches; a prefix invalidates every key below
 * it. payments.ts refreshes the lists `listsShowingAccount` selects and the
 * new TRAN-ID's detail after a bill payment, whose transaction joins the
 * master, so these shapes are a contract between the query modules.
 */
export const transactionKeys = {
  all,
  lists: () => [...all, "list"] as const,
  list: (params: TransactionListParams) => [...all, "list", params] as const,
  detail: (tranId: string) => [...all, "detail", tranId] as const,
} as const;

/**
 * The transaction lists a new live transaction of `accountId` can appear in:
 * every list without an account filter, a blank filter included, since the
 * server reads `SPACES` or `LOW-VALUES` as none, and every list filtered by
 * that account. A list filtered by another account cannot show it. The
 * account is its 11 digits, the only form the filter and the CT02 and CB00
 * writes accept. With `undefined`, an account the client cannot name, every
 * list is selected.
 *
 * Existing transaction details are never selected: an insert changes no
 * other record.
 */
export function listsShowingAccount(
  accountId: string | undefined,
): InvalidateQueryFilters {
  if (accountId === undefined) {
    return { queryKey: transactionKeys.lists() };
  }
  return {
    queryKey: transactionKeys.lists(),
    predicate: (query) => {
      const params = query.queryKey[2];
      if (
        typeof params !== "object" ||
        params === null ||
        !("account_id" in params)
      ) {
        return true;
      }
      const filter = params.account_id;
      return (
        filter === undefined ||
        filter === null ||
        (typeof filter === "string" &&
          (isSpacesOrLowValues(filter) || filter === accountId))
      );
    },
  };
}

// The status of a completed add: the handler answers StatusCode::CREATED with
// the transaction written and its reward accrual committed.
const CREATED = 201;

// The confirmations COTRN02C adds on. `N`, `n`, blank or LOW-VALUES asks for
// confirmation (V-COTRN02C-027) and any other value is refused
// (V-COTRN02C-028); both set WS-ERR-FLG and write nothing. The PF5 check
// sends a blank one.
const ADDING_CONFIRMATIONS: readonly string[] = ["Y", "y"];

// Whether a settled CT02 request may have added a transaction. Only a
// confirmed one can. Of its answers, a 201 added, and so may have:
// - a 201 whose body did not arrive (an ApiError with that status) or could
//   not be read (the parse or read failure itself);
// - any 5xx. The server's own 500 rolls the add back, but the Vite proxy
//   answers 500 when the API's answer is lost after the commit, and the
//   client cannot tell the two apart;
// - a request without a complete answer (the client's RequestTimeoutError, a
//   network failure), which may have reached the server.
// Every 4xx refuses the add before the commit: a field edit or an unconfirmed
// add (400), no valid session (401), a missing cross-reference or an empty
// master (404, E-27), a live TRAN-ID (409).
function mayHaveAdded(
  confirm: TransactionAddBody["confirm"],
  result: { status: number } | undefined,
  error: Error | null,
): boolean {
  if (typeof confirm !== "string" || !ADDING_CONFIRMATIONS.includes(confirm)) {
    return false;
  }
  if (result !== undefined) {
    return result.status === CREATED;
  }
  if (error instanceof ApiError) {
    return error.status === CREATED || error.status >= 500;
  }
  return true;
}

// The account a CT02 add writes to, when the request names it. COTRN02C uses
// an entered ACTIDIN, blank being SPACES or LOW-VALUES, and reads the card
// number only without one; on that path a write proves the entry the
// account's 11 digits, since a non-numeric account ID is refused first
// (V-COTRN02C-001). A card-only add gives `undefined`: its account comes from
// the cross-reference, which the answer does not carry.
function accountOfAdd(body: TransactionAddBody): string | undefined {
  const entered = body.account_id;
  return typeof entered === "string" && !isSpacesOrLowValues(entered)
    ? entered
    : undefined;
}

/**
 * One page of the live transaction master in TRAN-ID order (CT00), as
 * `{ items, next_key, prev_key, message }`. Always enabled, because COTRN00C
 * browses every transaction from a start key; without one, `forward` reads
 * from the start of the file and `backward` from its end. An empty master is a
 * page without items. The page shown stays on screen while the next one
 * loads, as the 3270 screen did between PF7/PF8 presses.
 */
export function useTransactions(params: TransactionListParams = {}) {
  return useQuery({
    queryKey: transactionKeys.list(params),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET("/transactions", { params: { query: params }, signal }),
      ),
    placeholderData: keepPreviousData,
  });
}

/** The rows of the dashboard's most recent transactions, its one page. */
export const RECENT_TRANSACTION_ROWS = 5;

/**
 * The dashboard's five most recent transactions of `accountId`: the
 * highest-keyed page of the account read backward from the end of the file.
 * Nothing is requested while the account number is empty, and no other
 * account's rows stand in while a new account loads.
 */
export function useRecentTransactions(accountId: string) {
  const params: TransactionListParams = {
    account_id: accountId,
    direction: "backward",
    page_size: RECENT_TRANSACTION_ROWS,
  };
  return useQuery({
    queryKey: transactionKeys.list(params),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET("/transactions", { params: { query: params }, signal }),
      ),
    enabled: accountId !== "",
  });
}

/**
 * The live transaction `tranId` (CT01), the TRAN-ID as entered. Nothing is
 * requested while it is empty. After `batch tranbkp` empties the live master,
 * every TRAN-ID answers 404 (E-09), which reaches the page as the ApiError.
 */
export function useTransaction(tranId: string) {
  return useQuery({
    queryKey: transactionKeys.detail(tranId),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET("/transactions/{id}", {
          params: { path: { id: tranId } },
          signal,
        }),
      ),
    enabled: tranId !== "",
  });
}

/**
 * CT02 PF5, "Copy last transaction": COTRN02C COPY-LAST-TRAN-DATA starts the
 * browse at HIGH-VALUES and reads one record back
 * [app/cbl/COTRN02C.cbl:L471-495], so the copy is the highest-keyed
 * transaction in the whole file, never one of the entered account's; the
 * request therefore carries no `account_id`.
 *
 * A mutation rather than a query, because each press reads the file afresh
 * and the page copies the answer into its form: nothing is cached, and the
 * call takes no variables.
 */
export function useCopyLastTransactionMutation() {
  return useMutation({
    mutationFn: async (): Promise<LastTransaction> => {
      const page = unwrap(
        await api.GET("/transactions", {
          params: { query: { direction: "backward", page_size: 1 } },
        }),
      );
      return { transaction: page.items.at(0), message: page.message };
    },
  });
}

/**
 * The CT02 add. The body is sent as the page built it, `confirm` included as
 * the confirm dialog set it (`Y`, `N` or blank). The answer is
 * `{ data, status }`: 201 carries the transaction written and the COTRN02C
 * success text in `data.message`. Every other COTRN02C outcome, the `N` and
 * blank confirmations included, sets `WS-ERR-FLG`, writes nothing and rejects
 * with the ApiError: 400 for a field edit or an unconfirmed add, 404 for a
 * missing cross-reference or an empty master, 409 for a live TRAN-ID, 500
 * when a read, the write, the reward accrual or the commit fails, with nothing
 * persisted.
 *
 * The caches are refreshed once a request that may have added settles
 * (`mayHaveAdded`), failures included: a 201, a 201 whose body did not arrive
 * or could not be read, a 5xx and a request without a complete answer, since
 * the insert and its accrual may have committed before the answer was lost.
 * A request that cannot have added refreshes nothing: a confirmation other
 * than `Y` or `y`, the PF5 check's blank one included, whatever its answer,
 * and every 4xx. What is reloaded is only what the insert and its accrual,
 * committed in the same database transaction, change:
 * - the transaction lists that can show the new row (`listsShowingAccount`);
 * - the new TRAN-ID's detail, once a 201 names it. It may hold a 404, or
 *   another row's content, from before `batch tranbkp` let the TRAN-ID be
 *   issued again (E-09). Other transactions' details are left alone, since
 *   an insert changes no other record;
 * - the reward balance and history of the account written to.
 *
 * The account is known when the request entered it (`accountOfAdd`). A
 * card-only add takes its account from the cross-reference, which the answer
 * does not carry, so every transaction list and every account's reward
 * balance and history are refreshed then. The refreshes are returned, so the
 * mutation settles once the active queries they reach have reloaded; a
 * failure still rejects with its own error, and the request is never sent
 * again.
 */
export function useTransactionAddMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (body: TransactionAddBody) =>
      unwrapWithStatus(await api.POST("/transactions", { body })),
    onSettled: (result, error, body) => {
      if (!mayHaveAdded(body.confirm, result, error)) {
        return undefined;
      }
      const accountId = accountOfAdd(body);
      const tranId = result?.data.transaction.tran_id;
      const rewards =
        accountId === undefined
          ? queryClient.invalidateQueries({ queryKey: rewardKeys.all })
          : Promise.all([
              queryClient.invalidateQueries({
                queryKey: rewardKeys.balance(accountId),
                exact: true,
              }),
              queryClient.invalidateQueries({
                queryKey: rewardKeys.histories(accountId),
              }),
            ]);
      return Promise.all([
        queryClient.invalidateQueries(listsShowingAccount(accountId)),
        tranId === undefined
          ? undefined
          : queryClient.invalidateQueries({
              queryKey: transactionKeys.detail(tranId),
              exact: true,
            }),
        rewards,
      ]);
    },
  });
}
