// Query hooks of the admin reward-rule screens: RewardRuleListPage
// (/admin/reward-rules, with its soft-delete dialog) and RewardRuleFormPage
// (/admin/reward-rules/new, /admin/reward-rules/:ruleId/edit). Every endpoint
// answers only an admin's token; the server's 403 is authoritative, whatever
// the route guard shows.
//
// - Rule IDs, rates, amount bounds and instants stay the strings the API
//   writes. Bodies are sent as given and answers cached as received, so no
//   value passes through a JavaScript number and the server's rule check
//   (`carddemo_domain::rewards::validate_rule`) sees exactly what was entered.
// - A failed call rejects with the client's ApiError, which reaches the page
//   untouched: the form places the 400 rule-check fields (RewardRuleField) and
//   the 404 `Reward rule not found` through applyApiError.
import {
  queryOptions,
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";

import { api, unwrap } from "@/api/client";
import type { components } from "@/api/schema";

/**
 * One stored reward rule, as every reward-rule endpoint answers it, with its
 * `status` at the server's processing clock.
 */
export type RewardRule = components["schemas"]["RewardRuleResponse"];

/**
 * The answer of `DELETE /admin/reward-rules/{id}`: the rule afterwards, plus
 * the `outcome` saying whether the delete ended it or found it already ended.
 */
export type RewardRuleDeletion =
  components["schemas"]["RewardRuleDeleteResponse"];

/** The body of `POST /admin/reward-rules` and `PUT /admin/reward-rules/{id}`. */
export type RewardRuleBody = components["schemas"]["RewardRuleRequest"];

/**
 * Query keys of the reward rules. The root differs from the account-facing
 * `rewards` keys, so a rule change never invalidates a balance or a history,
 * and an accrual never refetches the rules.
 */
export const rewardRuleKeys = {
  all: ["admin-reward-rules"],
} as const;

// The whole rule list: `GET /admin/reward-rules` answers a plain array in
// `rule_id` order, active, future and soft-deleted rules alike, with no paging.
// Both query hooks use this one cache entry, because no GET of a single rule
// exists.
const rewardRulesQuery = queryOptions({
  queryKey: rewardRuleKeys.all,
  queryFn: async ({ signal }) =>
    unwrap(await api.GET("/admin/reward-rules", { signal })),
});

// Marks the rule list stale and refetches it where a screen shows it. The
// returned promise is the mutation's onSuccess result, so the mutation settles
// only once every rule list on screen holds the change, and the list page's
// delete dialog closes over the refreshed rows. A failed refetch does not fail
// the mutation; the list query carries that error itself.
function invalidateRewardRules(queryClient: QueryClient): Promise<void> {
  return queryClient.invalidateQueries({ queryKey: rewardRuleKeys.all });
}

/** Every reward rule, in `rule_id` order, soft-deleted ones included. */
export function useRewardRules(): UseQueryResult<RewardRule[]> {
  return useQuery(rewardRulesQuery);
}

/**
 * The rule whose `rule_id` is `ruleId`, picked from the rule list; `undefined`
 * once the list has loaded without it. IDs are compared as the decimal strings
 * the API writes, so `ruleId` must be written the same way (`"1"`, never
 * `"01"`). An empty `ruleId` sends no request.
 */
export function useRewardRule(
  ruleId: string,
): UseQueryResult<RewardRule | undefined> {
  return useQuery({
    ...rewardRulesQuery,
    select: (rules: RewardRule[]) =>
      rules.find((rule) => rule.rule_id === ruleId),
    enabled: ruleId !== "",
  });
}

/**
 * `POST /admin/reward-rules`: stores a new rule, resolving to it as stored
 * (201) under the `rule_id` the database assigned.
 */
export function useRewardRuleCreateMutation(): UseMutationResult<
  RewardRule,
  Error,
  RewardRuleBody
> {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (body: RewardRuleBody) =>
      unwrap(await api.POST("/admin/reward-rules", { body })),
    onSuccess: () => invalidateRewardRules(queryClient),
  });
}

/**
 * `PUT /admin/reward-rules/{id}`: replaces every column of rule `ruleId` but
 * its ID, resolving to the rule as stored.
 */
export function useRewardRuleUpdateMutation(
  ruleId: string,
): UseMutationResult<RewardRule, Error, RewardRuleBody> {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (body: RewardRuleBody) =>
      unwrap(
        await api.PUT("/admin/reward-rules/{id}", {
          params: { path: { id: ruleId } },
          body,
        }),
      ),
    onSuccess: () => invalidateRewardRules(queryClient),
  });
}

/**
 * `DELETE /admin/reward-rules/{id}`: the soft delete, resolving to the rule
 * afterwards with the server's `outcome`. The server ends a rule that is
 * open-ended or ends later at the processing clock's instant (`deleted`), and
 * answers a rule that has already ended unchanged (`unchanged`), also with
 * 200; both are success. The row is never removed, because accruals reference
 * its `rule_id`, so the list keeps showing it.
 */
export function useRewardRuleDeleteMutation(): UseMutationResult<
  RewardRuleDeletion,
  Error,
  string
> {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (ruleId: string) =>
      unwrap(
        await api.DELETE("/admin/reward-rules/{id}", {
          params: { path: { id: ruleId } },
        }),
      ),
    onSuccess: () => invalidateRewardRules(queryClient),
  });
}
