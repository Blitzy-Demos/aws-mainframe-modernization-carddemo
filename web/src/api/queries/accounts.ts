// Query hooks of the account screens: CAVW (COACTVWC, `GET /accounts/{id}`)
// and CAUP (COACTUPC, `PUT /accounts/{id}`). The dashboard account summary
// (CM00) and the bill-payment balance preview (CB00) reuse useAccount, so
// every screen showing an account reads the same cache entry.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, unwrap } from "@/api/client";
import type { components } from "@/api/schema";

/**
 * Query keys of the account resources. payments.ts invalidates
 * `accountKeys.detail(account_id)` after a payment, so these shapes are a
 * contract between the query modules.
 */
export const accountKeys = {
  all: ["accounts"] as const,
  detail: (id: string) => [...accountKeys.all, "detail", id] as const,
};

/**
 * The account and customer records `GET /accounts/{id}` shows, the account's
 * first card number and the `version` token a save must echo.
 */
export type Account = components["schemas"]["AccountResponse"];

/** The COACTUP input fields as entered, plus the `version` of the edited record. */
export type AccountUpdateBody = components["schemas"]["AccountUpdateRequest"];

/** The records as they stand after a save, and the COACTUPC text to show. */
export type AccountUpdateResult =
  components["schemas"]["AccountUpdateResponse"];

/**
 * Account `id`, the account number as entered. An empty `id` sends no request,
 * because the dashboard and payment pages take it from user input, and the
 * account pages pass `""` for a route key `accountKeySchema` rejects
 * (`accountKeyError`), whose text they show without a read. A COACTVWC
 * rejection is the query's ApiError: 400 at `ACCTSID` for a blank, malformed
 * or zero number, 404 for a missing cross-reference, account or customer.
 */
export function useAccount(id: string) {
  return useQuery({
    queryKey: accountKeys.detail(id),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET("/accounts/{id}", { params: { path: { id } }, signal }),
      ),
    enabled: id !== "",
  });
}

/**
 * Saves account `id` (CAUP). The body is sent as the page built it: the hook
 * neither fills in nor strips `version`, which must be the token of the record
 * the page edited; a body without one is answered 400 with no message (E-24).
 * Failures reach the caller as the server's ApiError, unchanged, with the BMS
 * field names of the COACTUP map in `fields`.
 */
export function useAccountUpdateMutation(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (body: AccountUpdateBody) =>
      unwrap(
        await api.PUT("/accounts/{id}", { params: { path: { id } }, body }),
      ),
    // The answer holds the records as they now stand and their new token, so
    // the next save carries that token without another read.
    onSuccess: (data) => {
      queryClient.setQueryData<Account>(accountKeys.detail(id), data.account);
    },
    // Every failure reloads the record, because the cached copy may no longer
    // be what is on file:
    // - 409 'Record changed by some one else. Please review' (COACTUPC
    //   9700-CHECK-CHANGE-IN-REC): the page needs the changed record and its
    //   new token before the user saves again.
    // - A 500 after the account rewrite: the VSAM files are RECOVERY(NONE), so
    //   the account change stays although the customer rewrite failed.
    // - A request left without a complete answer may still have been applied.
    // - A 400 or 404 wrote nothing. The reload brings back the same record,
    //   which structural sharing keeps as the same object, so a form reset
    //   keyed on it does not fire and the user's entries stay.
    // Returning the promise settles the mutation only once the reloaded record
    // is in the cache, so the page meets the error with the current token.
    onError: () =>
      queryClient.invalidateQueries({ queryKey: accountKeys.detail(id) }),
  });
}
