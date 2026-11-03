/**
 * Query hooks of the reward screens: the balance cards and accrual history of
 * RewardsPage (`/rewards`) and the dashboard's reward card, over
 * `GET /rewards/balance/{account_id}` and `GET /rewards/history/{account_id}`.
 *
 * Rewards have no COBOL program; both endpoints follow the request's Rewards
 * system (AAP 0.3.5). Every points value and identifier arrives as a decimal
 * integer string and reaches the page exactly as sent, never converted or
 * formatted: a total such as 9007199254740993 lies beyond 2^53 − 1 and would
 * lose its last digit as a JavaScript number.
 *
 * A non-2xx answer surfaces as the client's ApiError, raised by `unwrap` and
 * left to the page, which shows it through lib/api-errors.ts: 404
 * 'Account not found' for an unknown account, 400 for a malformed `start_key`
 * or `page_size`, 401 'Authentication required'.
 */
import { keepPreviousData, useQuery } from "@tanstack/react-query";

import { api, unwrap } from "@/api/client";
import type { components, paths } from "@/api/schema";

/**
 * The query of `GET /rewards/history/{account_id}`: `start_key`, the
 * `accrual_id` a page starts at (the previous page's `next_key`), and
 * `page_size`, 1 to 100 (the server's default is 10). History pages forward
 * only, newest first, so there is no `direction`.
 */
export type RewardHistoryParams = NonNullable<
  paths["/rewards/history/{account_id}"]["get"]["parameters"]["query"]
>;

/**
 * The reward balance of one account. An existing account without accruals
 * has the zero balance, all three totals `"0"`.
 */
export type RewardBalance = components["schemas"]["RewardBalanceResponse"];

/** One entry of an account's reward history. */
export type RewardAccrual = components["schemas"]["RewardAccrualResponse"];

// The first element of every reward key. It differs from the reward-rule
// caches' "admin-reward-rules", so invalidating `rewardKeys.all` after a CT02
// add, whose accrual commits with the transaction, refreshes balances and
// histories only. Declared on its own because the factory's members cannot
// name `rewardKeys` inside its own initializer without making its type
// circular.
const ROOT = ["rewards"] as const;

/**
 * The query keys of the reward caches. `all` is a prefix of every key here,
 * and `histories(accountId)` of every history page of that account.
 */
export const rewardKeys = {
  all: ROOT,
  balance: (accountId: string) => [...ROOT, "balance", accountId] as const,
  histories: (accountId: string) => [...ROOT, "history", accountId] as const,
  history: (accountId: string, params: RewardHistoryParams) =>
    [...ROOT, "history", accountId, params] as const,
} as const;

/**
 * The reward balance of `accountId`, read once the account number is
 * entered. The totals are decimal integer strings.
 */
export function useRewardBalance(accountId: string) {
  return useQuery({
    queryKey: rewardKeys.balance(accountId),
    queryFn: async ({ signal }): Promise<RewardBalance> =>
      unwrap(
        await api.GET("/rewards/balance/{account_id}", {
          params: { path: { account_id: accountId } },
          signal,
        }),
      ),
    enabled: accountId !== "",
  });
}

/**
 * One page of the reward history of `accountId`, newest first:
 * `{ items, next_key, prev_key, message }`. The next page is read by passing
 * the page's `next_key` as `start_key`; `next_key` is null on the last page,
 * and `prev_key` is always null.
 *
 * While the next page loads, the current one stays on screen, as when paging
 * with PF8. That holds within one account only: the previous data is kept
 * while the key names the same account, so another account's accruals are
 * never shown, as a placeholder, under a new or cleared account number.
 */
export function useRewardHistory(
  accountId: string,
  params: RewardHistoryParams = {},
) {
  return useQuery({
    queryKey: rewardKeys.history(accountId, params),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET("/rewards/history/{account_id}", {
          params: { path: { account_id: accountId }, query: params },
          signal,
        }),
      ),
    enabled: accountId !== "",
    placeholderData: (previous, previousQuery) =>
      previousQuery?.queryKey[2] === accountId
        ? keepPreviousData(previous)
        : undefined,
  });
}
