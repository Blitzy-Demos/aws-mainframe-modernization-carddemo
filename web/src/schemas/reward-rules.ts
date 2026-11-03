/**
 * The admin reward-rule form: the client mirror of the `RewardRuleRequest`
 * body of `POST /admin/reward-rules` and `PUT /admin/reward-rules/{id}`.
 *
 * Rewards have no COBOL program, BMS map or audit row; the server's rule
 * check (`carddemo_domain::rewards::validate_rule`) is authoritative. This
 * schema only keeps malformed text from being submitted, mirroring the
 * patterns the OpenAPI document publishes. The cross-field checks (minimum not
 * above maximum, `active_to` not before `active_from`) stay on the server and
 * come back as `fields[]` entries, which `rewardRuleBmsFieldMap` places.
 *
 * Decimals stay strings from input to request body: the rate is a
 * `NUMERIC(9,4)` reward rate and the bounds are `S9(09)V99` amounts, and no
 * value is ever converted to a JavaScript number. Dates are entered as
 * `YYYY-MM-DD` (there is no date picker) and sent as midnight UTC instants.
 */
import { z } from "zod";

import type { components } from "@/api/schema";

type RewardRuleRequest = components["schemas"]["RewardRuleRequest"];
type RewardRuleResponse = components["schemas"]["RewardRuleResponse"];
type RewardRuleField = components["schemas"]["RewardRuleField"];

/**
 * The request-written reward-rule texts, byte-identical to
 * `REWARD_RATE_NEGATIVE`, `REWARD_MIN_EXCEEDS_MAX`,
 * `REWARD_ACTIVE_TO_BEFORE_FROM` and `REWARD_RULE_NOT_FOUND` in
 * crates/domain/src/messages.rs. `notFound` is the 404 of
 * `PUT /admin/reward-rules/{id}`, which the edit form also shows when the rule
 * list holds no rule with the routed ID.
 */
export const REWARD_RULE_MESSAGES = {
  rateNegative: "reward_rate must not be negative",
  minExceedsMax:
    "min_transaction_amount must not exceed max_transaction_amount",
  activeToBeforeFrom: "active_to must not precede active_from",
  notFound: "Reward rule not found",
} as const;

// `RewardRuleRequest.transaction_type_cd`: the OpenAPI pattern, characters from
// U+0001 to U+00FF only. The server rejects a NUL, which its text column cannot
// store, and any character outside ISO 8859-1.
const TYPE_CODE_PATTERN = /^[\x01-\xFF]*$/;

// `RewardRuleRequest.transaction_category_cd`: TRAN-CAT-CD PIC 9(04), 1 to 4
// digits as the OpenAPI pattern allows.
const CATEGORY_CODE_PATTERN = /^[0-9]{1,4}$/;

// An unsigned rate whose fraction has no non-zero digit beyond the fourth
// place, the 4 fraction digits of NUMERIC(9,4). Zeros after the fourth place
// are accepted (`1.50000`, `0.00000`), because the server drops trailing
// fraction zeros before it checks the scale; `1.50001` is rejected. The text is
// matched as written and submitted unchanged. The sign is checked separately
// so a negative rate gets its own message. The OpenAPI pattern carries no
// integer-digit limit, so a rate of 100000 or more passes here and is answered
// by the server on field `reward_rate`.
const REWARD_RATE_PATTERN = /^[0-9]+(\.[0-9]{1,4}0*)?$/;

// `RewardRuleRequest.min_transaction_amount` / `max_transaction_amount`: the
// OpenAPI pattern of an S9(09)V99 amount, sign allowed.
const AMOUNT_PATTERN = /^-?[0-9]+(\.[0-9]{1,2})?$/;

const rewardRate = z.string().superRefine((value, ctx) => {
  // Any leading minus, `-0` included, is reported as negative: the server
  // takes a negative zero as zero, but the form has no reason to accept a
  // signed rate.
  if (value.startsWith("-")) {
    ctx.addIssue({
      code: z.ZodIssueCode.custom,
      message: REWARD_RULE_MESSAGES.rateNegative,
    });
  } else if (!REWARD_RATE_PATTERN.test(value)) {
    // No message: zod's default for a failed regex, as `.regex()` gives.
    ctx.addIssue({ code: z.ZodIssueCode.invalid_string, validation: "regex" });
  }
});

const amount = z.string().regex(AMOUNT_PATTERN);

// `.date()` accepts only `YYYY-MM-DD` and only real calendar dates. A shape
// check alone would let `2022-02-30` through to the server, whose RFC 3339
// parse rejects it without naming a field.
const date = z.string().date();

const startOfDayUtc = (value: string): string => `${value}T00:00:00Z`;

// An optional field: the empty input (or an absent key) is omitted from the
// body, which the server reads as `null` (an unbounded amount or an
// open-ended window).
const blankOr = <T extends z.ZodTypeAny>(schema: T) =>
  z.union([z.literal(""), schema]).optional();

const omitBlank = (value: string | undefined): string | undefined =>
  value === undefined || value === "" ? undefined : value;

export const rewardRuleSchema = z.object({
  // TRAN-TYPE-CD PIC X(02): the OpenAPI maxLength and pattern, and not blank.
  transaction_type_cd: z.string().min(1).max(2).regex(TYPE_CODE_PATTERN),
  transaction_category_cd: z.string().regex(CATEGORY_CODE_PATTERN),
  reward_rate: rewardRate,
  min_transaction_amount: blankOr(amount).transform(omitBlank),
  max_transaction_amount: blankOr(amount).transform(omitBlank),
  active_from: date.transform(startOfDayUtc),
  active_to: blankOr(date).transform((value) => {
    const entered = omitBlank(value);
    return entered === undefined ? undefined : startOfDayUtc(entered);
  }),
}) satisfies z.ZodType<RewardRuleRequest>;

/** The form's field values as entered: every field a string. */
export type RewardRuleFormInput = z.input<typeof rewardRuleSchema>;

/** The parsed form: the `RewardRuleRequest` body to submit. */
export type RewardRuleFormValues = z.infer<typeof rewardRuleSchema>;

// The `YYYY-MM-DD` date of an RFC 3339 UTC instant as the API writes it.
const dateOf = (instant: string): string => instant.slice(0, 10);

/**
 * The edit form's starting values for a stored rule. Instants keep only their
 * date, as the form enters dates only, so saving sends midnight UTC of that
 * date; a `null` bound or end becomes the empty input.
 */
export function rewardRuleToFormInput(
  rule: RewardRuleResponse,
): RewardRuleFormInput {
  return {
    transaction_type_cd: rule.transaction_type_cd,
    transaction_category_cd: rule.transaction_category_cd,
    reward_rate: rule.reward_rate,
    min_transaction_amount: rule.min_transaction_amount ?? "",
    max_transaction_amount: rule.max_transaction_amount ?? "",
    active_from: dateOf(rule.active_from),
    active_to: rule.active_to === null ? "" : dateOf(rule.active_to),
  };
}

/**
 * The form field each server `fields[].field` value is shown on. Rewards have
 * no BMS map, so the server names fields by their JSON names, which are the
 * form's own.
 */
export const rewardRuleBmsFieldMap: Record<
  RewardRuleField,
  keyof RewardRuleFormValues
> = {
  reward_rate: "reward_rate",
  min_transaction_amount: "min_transaction_amount",
  active_to: "active_to",
};
