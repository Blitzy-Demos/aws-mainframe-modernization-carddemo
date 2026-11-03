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
} from "@tanstack/react-query";

import { api, unwrap, unwrapWithStatus } from "@/api/client";
import { rewardKeys } from "@/api/queries/rewards";
import type { components, paths } from "@/api/schema";

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
 * it. payments.ts invalidates `transactionKeys.all` after a bill payment,
 * whose transaction joins the master, so these shapes are a contract between
 * the query modules.
 */
export const transactionKeys = {
  all,
  lists: () => [...all, "list"] as const,
  list: (params: TransactionListParams) => [...all, "list", params] as const,
  detail: (tranId: string) => [...all, "detail", tranId] as const,
} as const;

// The status of a completed add: the handler answers StatusCode::CREATED with
// the transaction written and its reward accrual committed.
const CREATED = 201;

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
    page_size: 5,
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
 * Only a 201 wrote, so only a 201 refreshes: every transaction query, and
 * every reward balance and history, because the accrual commits in the same
 * database transaction as the insert. Both refreshes are returned, so the
 * mutation settles once the active queries have reloaded.
 */
export function useTransactionAddMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (body: TransactionAddBody) =>
      unwrapWithStatus(await api.POST("/transactions", { body })),
    onSuccess: (result) =>
      result.status === CREATED
        ? Promise.all([
            queryClient.invalidateQueries({ queryKey: transactionKeys.all }),
            queryClient.invalidateQueries({ queryKey: rewardKeys.all }),
          ])
        : undefined,
  });
}
