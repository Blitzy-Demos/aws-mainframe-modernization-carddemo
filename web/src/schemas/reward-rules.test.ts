// Tests of the admin reward-rule form schema (schemas/reward-rules.ts) against
// the server's RewardRuleRequest rate rule: the OpenAPI pattern
// `^-?[0-9]+(\.[0-9]+)?$`, then RewardRate's NUMERIC(9,4) scale check, made
// after trailing fraction zeros are dropped. A rate whose digits beyond the
// fourth fraction place are all zeros is accepted and submitted as entered; a
// non-zero digit there, or text that is not a plain decimal, gets zod's regex
// issue; any leading minus gets the request-written negative-rate message.
import { describe, expect, it } from "vitest";
import { z } from "zod";

import {
  REWARD_RULE_MESSAGES,
  type RewardRuleFormInput,
  rewardRuleSchema,
} from "./reward-rules";

/** Every request-written reward-rule text the schema module can report. */
const REQUEST_WRITTEN: readonly string[] = Object.values(REWARD_RULE_MESSAGES);

/** A rule every client edit accepts but for its rate, `reward_rate`. */
function ruleWith(reward_rate: string): RewardRuleFormInput {
  return {
    transaction_type_cd: "01",
    transaction_category_cd: "0001",
    reward_rate,
    active_from: "2022-01-01",
  };
}

function issuesOf(values: RewardRuleFormInput): z.ZodIssue[] {
  const result = rewardRuleSchema.safeParse(values);
  return result.success ? [] : result.error.issues;
}

describe("rewardRuleSchema reward_rate", () => {
  // The server normalizes the rate (`1.50000` is stored as `1.5000`), so the
  // body carries the text exactly as entered.
  it.each(["1.50000", "0.00000", "1.5", "1", "1.1234", "1.123400000", "3.0000"])(
    "accepts the rate %j and submits it unchanged",
    (reward_rate) => {
      const result = rewardRuleSchema.safeParse(ruleWith(reward_rate));
      expect(result.success).toBe(true);
      expect(result.data).toEqual({
        transaction_type_cd: "01",
        transaction_category_cd: "0001",
        reward_rate,
        active_from: "2022-01-01T00:00:00Z",
      });
    },
  );

  it.each(["1.50001", "0.00001", "1.12345", "1.", ".5", "1.5.0", "abc", ""])(
    "rejects the rate %j with zod's regex issue",
    (reward_rate) => {
      const issues = issuesOf(ruleWith(reward_rate));
      expect(issues).toHaveLength(1);
      expect(issues[0]).toMatchObject({
        path: ["reward_rate"],
        code: z.ZodIssueCode.invalid_string,
        validation: "regex",
      });
      expect(REQUEST_WRITTEN).not.toContain(issues[0]?.message);
    },
  );

  // The sign is checked before the digits, so a negative rate that is also
  // over-precise still gets the negative-rate message.
  it.each(["-1.5", "-0", "-0.00000", "-1.50001"])(
    "rejects the rate %j with the negative-rate message",
    (reward_rate) => {
      const issues = issuesOf(ruleWith(reward_rate));
      expect(issues).toHaveLength(1);
      expect(issues[0]).toMatchObject({
        path: ["reward_rate"],
        code: z.ZodIssueCode.custom,
        message: REWARD_RULE_MESSAGES.rateNegative,
      });
    },
  );
});
