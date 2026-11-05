/**
 * Server state of the pending-authorization screens:
 * - CPVS, the authorization summary of one account (COPAUS0C, map COPAU00):
 *   `GET /authorizations?account_id=`, five rows a page.
 * - CPVD, one authorization (COPAUS1C, map COPAU01): `GET /authorizations/{id}`
 *   with the PF8 neighbour as `next_id`, and PF5, the fraud mark through
 *   COPAUS2C: `PUT /authorizations/{id}`.
 *
 * The CP00 consumer writes these records; the browser reads them only through
 * these endpoints. A non-2xx answer rejects with the client's ApiError,
 * unchanged, which the pages show through lib/api-errors.ts. The COBOL texts
 * that are not errors (the page-boundary line, `Already at the last
 * Authorization...`, the fraud-mark result) arrive as `message` in the data.
 */
import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
  type Query,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";

import { api, unwrap } from "@/api/client";
import type { components, paths } from "@/api/schema";

type AuthSummaryQuery = NonNullable<
  paths["/authorizations"]["get"]["parameters"]["query"]
>;

/**
 * The CPVS request: the account ID as entered (`ACCTIDI PIC X(11)`) and the
 * page position.
 * - `account_id` is optional on the server, which edits an absent account as
 *   blank, but the screen always sends what was typed, so the hook requires
 *   it. An empty value sends nothing (see useAuthorizationSummary).
 * - `start_key` is a previous page's `next_key` with `direction: "forward"`
 *   (PF8) or its `prev_key` with `direction: "backward"` (PF7).
 * - Without `page_size` the page holds the 5 rows of the COPAU00 list.
 */
export type AuthSummaryParams = AuthSummaryQuery &
  Required<Pick<AuthSummaryQuery, "account_id">>;

/** The COPAU00 header (customer, account, summary segment) with one page of authorizations. */
export type AuthSummary = components["schemas"]["AuthSummaryResponse"];

/** The COPAU01 screen of one authorization, with `next_id`, the one PF8 shows. */
export type AuthDetail = components["schemas"]["AuthDetailResponse"];

/** The PF5 answer: the replaced detail and `AUTH MARKED FRAUD...` or `AUTH FRAUD REMOVED...`. */
export type FraudToggleResult = components["schemas"]["FraudToggleResponse"];

const all = ["authorizations"] as const;

/** Query keys: a prefix invalidates every key below it. */
export const authorizationKeys = {
  all,
  summaries: () => [...all, "summary"] as const,
  summary: (params: AuthSummaryParams) => [...all, "summary", params] as const,
  details: () => [...all, "detail"] as const,
  detail: (authId: string) => [...all, "detail", authId] as const,
} as const;

// Whether `query`, under `authorizationKeys.summaries()`, holds a summary page
// of `accountId`: its key's params name that account. Both sides are the 11
// zero-padded digits, the only form the summary query is sent with and the one
// a detail's `account_id` takes.
function isSummaryOf(query: Query, accountId: string): boolean {
  const params = query.queryKey[2];
  return (
    typeof params === "object" &&
    params !== null &&
    "account_id" in params &&
    params.account_id === accountId
  );
}

/**
 * COPAUS0C ENTER, PF7 and PF8: the summary of `params.account_id` and one page
 * of its authorizations, newest first, returned whole: the header fields come
 * with `items`, `next_key`, `prev_key` and `message`.
 *
 * Nothing is requested while the account ID is empty; the page's form edits
 * the field first (schemas/authorizations.ts). The previous page stays shown
 * while the next one loads.
 */
export function useAuthorizationSummary(
  params: AuthSummaryParams,
): UseQueryResult<AuthSummary> {
  return useQuery({
    queryKey: authorizationKeys.summary(params),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET("/authorizations", {
          params: { query: params },
          signal,
        }),
      ),
    enabled: params.account_id !== "",
    placeholderData: keepPreviousData,
  });
}

/**
 * COPAUS1C ENTER: the authorization `authId` names, the `auth_id` of a
 * summary row. `next_id` is the next older authorization under the same
 * summary, `null` at the last one, where `message` carries
 * `Already at the last Authorization...`.
 */
export function useAuthorizationDetail(
  authId: string,
): UseQueryResult<AuthDetail> {
  return useQuery({
    queryKey: authorizationKeys.detail(authId),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET("/authorizations/{id}", {
          params: { path: { id: authId } },
          signal,
        }),
      ),
    enabled: authId !== "",
  });
}

// The identity a key names, as the server's `i64` parse reads it: the decimal
// text without its optional `+` and its leading zeros, so the route aliases
// `+0007` and `0007` name the `auth_id` `7` the server answers with. Text in,
// text out, so no digit of an ID above 2^53 is lost to a conversion.
function authIdentity(authId: string): string {
  return authId.replace(/^\+?0*/, "");
}

/**
 * COPAUS1C PF5, MARK-AUTH-FRAUD with COPAUS2C: toggles the fraud mark of the
 * authorization whose key is passed to `mutate`, the key of the record shown
 * when the toggle is confirmed. The request has no body, because PF5 acts on
 * the record shown. The result carries `message`, the success text the page
 * shows.
 *
 * The key is the mutation's variables, fixed when the toggle starts: the page
 * stays mounted when its route moves on to another authorization (PF8) while
 * the PUT is in flight, and the answer still belongs to the key it was sent
 * for, never to the one shown by the time it arrives.
 *
 * On success the requested key's detail becomes the replaced one, and the
 * summary pages of the authorization's account are marked stale, so CPVS
 * reads that account again after the write, as PF3 back to COPAUS0C does.
 * Other accounts' summaries are left alone, since the write touched only this
 * account's records. On failure the server has rolled both writes back
 * (ROLL-BACK), so the cache is left alone and the ApiError reaches the page.
 */
export function useFraudToggleMutation(): UseMutationResult<
  FraudToggleResult,
  Error,
  string
> {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (authId: string) =>
      unwrap(
        await api.PUT("/authorizations/{id}", {
          params: { path: { id: authId } },
        }),
      ),
    onSuccess: async (data, authId) => {
      const detailKey = authorizationKeys.detail(authId);
      // A read of the detail still in flight may have been served before the
      // mark committed, so it is stopped rather than allowed to overwrite the
      // replaced detail with the old flag.
      await queryClient.cancelQueries({ queryKey: detailKey });
      const shown = queryClient.getQueryData<AuthDetail>(detailKey);
      const summaries = queryClient.invalidateQueries({
        queryKey: authorizationKeys.summaries(),
        predicate: (query) => isSummaryOf(query, data.detail.account_id),
      });
      // The server answers with the detail its path names. A detail of any
      // other authorization is never written under this key; the key is read
      // afresh instead, as is one with no detail to keep a PF8 neighbour from.
      const sameAuthorization =
        authIdentity(data.detail.auth_id) === authIdentity(authId);
      if (shown === undefined || !sameAuthorization) {
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: detailKey }),
          summaries,
        ]);
        return;
      }
      // The PUT reads no PF8 neighbour, so its detail has `next_id` null.
      // COPAUS1C keeps the selected key after PF5 and PF8 still reads on from
      // it, so the neighbour already shown is kept. The record's own message
      // is the PUT's (none): the fraud text replaces the last-authorization
      // line, as on the screen.
      queryClient.setQueryData<AuthDetail>(detailKey, {
        ...data.detail,
        next_id: data.detail.next_id ?? shown.next_id,
      });
      await summaries;
    },
  });
}
