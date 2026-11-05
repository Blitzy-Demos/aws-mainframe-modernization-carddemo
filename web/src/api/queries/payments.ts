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

import { ApiError, api, unwrapWithStatus } from "@/api/client";
import type { components } from "@/api/schema";
import { isSpacesOrLowValues } from "@/schemas/primitives";

import { accountKeys } from "./accounts";
import { listsShowingAccount, transactionKeys } from "./transactions";

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

// The status of a completed payment: the transaction written and the balance
// reduced.
const CREATED = 201;

// The confirmations COBIL00C pays on. A blank one only asks
// (V-COBIL00C-008) and `N` or `n` cancels (V-COBIL00C-002); any other value
// is refused (V-COBIL00C-003). None of them reaches a write.
const PAYING_CONFIRMATIONS: readonly string[] = ["Y", "y"];

// The failures of a confirmed payment that can follow a persisted write
// (E-19): 404 'Account ID NOT found...' when the account rewrite finds no
// record after the transaction write (V-COBIL00C-016) and 409 'Tran ID
// already exist...' with the balance still rewritten (V-COBIL00C-014). A
// 5xx can too: 'Unable to Add Bill pay Transaction...' (V-COBIL00C-015) and
// 'Unable to Update Account...' (V-COBIL00C-017) are among them. Every other
// 4xx refuses the request before any write.
const FAILURES_AFTER_A_WRITE: readonly number[] = [404, 409];

// Whether a settled CB00 request may have changed a record. Only a confirmed
// one can. Of its answers, a 201 wrote, and so may have:
// - a 201 whose body did not arrive, an ApiError with that status;
// - a failure in FAILURES_AFTER_A_WRITE or any 5xx;
// - a request without a complete answer (the client's RequestTimeoutError, a
//   network failure), which may have reached the server.
function mayHaveWritten(
  confirm: PaymentBody["confirm"],
  result: { status: number } | undefined,
  error: Error | null,
): boolean {
  if (typeof confirm !== "string" || !PAYING_CONFIRMATIONS.includes(confirm)) {
    return false;
  }
  if (result !== undefined) {
    return result.status === CREATED;
  }
  if (error instanceof ApiError) {
    return (
      error.status === CREATED ||
      FAILURES_AFTER_A_WRITE.includes(error.status) ||
      error.status >= 500
    );
  }
  return true;
}

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
 * The caches are refreshed once a request that may have written settles
 * (`mayHaveWritten`), failures included: COBIL00C rewrites the balance
 * whatever the transaction write answered, and the server runs each write
 * under its own savepoint (E-19), so a 404, 409 or 500 can follow a persisted
 * write. What is reloaded then:
 * - the paid account's record;
 * - the transaction lists that can show the payment's transaction
 *   (`listsShowingAccount`);
 * - the new TRAN-ID's detail, once a 201 names it.
 *
 * Other transactions' details are left alone, since an insert changes no
 * other record, and so are reward caches, because a bill payment accrues no
 * points: only CT02 and POSTTRAN call the reward engine. A request that
 * cannot have written refreshes nothing: the 200 prompt, the `N` cancel, a
 * blank account ID and every refusal before the writes. The refresh is
 * returned, so the mutation settles once the active queries it reaches have
 * reloaded.
 */
export function usePaymentMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationKey: paymentKeys.all,
    mutationFn: async (
      body: PaymentBody,
    ): Promise<{ data: PaymentResult; status: number }> =>
      unwrapWithStatus(await api.POST("/payments", { body })),
    onSettled: (result, error, variables) => {
      // An answer names the account as its 11 digits; a failure leaves the
      // account ID as entered.
      const accountId = result?.data.account_id ?? variables.account_id;
      // A blank account ID is refused before any read (V-COBIL00C-001).
      if (
        typeof accountId !== "string" ||
        isSpacesOrLowValues(accountId) ||
        !mayHaveWritten(variables.confirm, result, error)
      ) {
        return undefined;
      }
      const tranId = result?.data.tran_id;
      return Promise.all([
        queryClient.invalidateQueries({
          queryKey: accountKeys.detail(accountId),
        }),
        queryClient.invalidateQueries(listsShowingAccount(accountId)),
        tranId === undefined
          ? undefined
          : queryClient.invalidateQueries({
              queryKey: transactionKeys.detail(tranId),
              exact: true,
            }),
      ]);
    },
  });
}
