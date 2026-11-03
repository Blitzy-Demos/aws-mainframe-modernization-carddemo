/**
 * CB00 bill payment (COBIL00C, map COBIL00): the hook of BillPaymentPage
 * (`/payments`) over `POST /payments`. The page previews the balance with
 * `useAccount` from accounts.ts, then pays through the confirmation dialog.
 *
 * `account_id` (`ACTIDINI X(11)`), `current_balance` (`ACCT-CURR-BAL
 * S9(10)V99`, a scale-2 decimal string) and `tran_id` stay the strings the
 * server sends. A non-2xx answer rejects with the client's ApiError,
 * unchanged, which the page shows through lib/api-errors.ts.
 */
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { api, unwrapWithStatus } from "@/api/client";
import type { components } from "@/api/schema";

import { accountKeys } from "./accounts";
import { transactionKeys } from "./transactions";

/** The key root of the bill-payment mutation. */
export const paymentKeys = { all: ["payments"] } as const;

/**
 * One ENTER on COBIL00: the account to pay (`ACTIDIN`) and the confirmation
 * (`CONFIRM`, `CONFIRMI X(1)`).
 */
export type PaymentBody = components["schemas"]["PaymentRequest"];

/**
 * The confirmation prompt or the completed payment: the account, its balance
 * (before the payment with the prompt, after it once paid), the COBIL00C
 * message line and, for a completed payment only, its TRAN-ID.
 */
export type PaymentResult = components["schemas"]["PaymentResponse"];

/**
 * Pays an account's balance in full (CB00). The body is sent as the page
 * built it, `confirm` included as entered (`Y`, `N` or blank), and the answer
 * is `{ data, status }`, because the two success statuses mean different
 * things:
 * - 200: `confirm` blank. COBIL00C reads the account and asks
 *   'Confirm to make a bill payment...' without writing; `tran_id` is absent.
 * - 201: the payment was made. `data.message` is the COBIL00C success text
 *   with the TRAN-ID, and `current_balance` the balance after the payment.
 *
 * Every other outcome rejects with the ApiError: 400 for an entry edit or the
 * `N` cancel (no message), 404 for a missing account or cross-reference, 409
 * 'Tran ID already exist...', 500 'Unable to Add Bill pay Transaction...' or
 * 'Unable to Update Account...'.
 *
 * The caches are refreshed once the request settles, failures included:
 * COBIL00C rewrites the balance whatever the transaction write answered, and
 * the server runs each write under its own savepoint (E-19), so a 500 can
 * follow a persisted write. The entered account's record and every
 * transaction query are therefore reloaded whatever the answer; the account
 * only when an account ID was sent, since an empty one names no cache entry.
 * Reward caches are left alone, because a bill payment accrues no points:
 * only CT02 and POSTTRAN call the reward engine. The refresh is returned, so
 * the mutation settles once the active queries have reloaded.
 */
export function usePaymentMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationKey: paymentKeys.all,
    mutationFn: async (
      body: PaymentBody,
    ): Promise<{ data: PaymentResult; status: number }> =>
      unwrapWithStatus(await api.POST("/payments", { body })),
    onSettled: (_data, _error, variables) => {
      const accountId = variables.account_id;
      return Promise.all([
        typeof accountId === "string" && accountId !== ""
          ? queryClient.invalidateQueries({
              queryKey: accountKeys.detail(accountId),
            })
          : undefined,
        queryClient.invalidateQueries({ queryKey: transactionKeys.all }),
      ]);
    },
  });
}
