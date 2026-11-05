/**
 * The admin reward-rule form: the client mirror of the `RewardRuleRequest`
 * body of `POST /admin/reward-rules` and `PUT /admin/reward-rules/{id}`.
 *
 * Rewards have no COBOL program, BMS map or audit row; the server's rule
 * check (`carddemo_domain::rewards::validate_rule`) is authoritative. This
 * schema only keeps malformed text from being submitted, mirroring the
 * patterns the OpenAPI document publishes, and reports it with the texts of
 * `REWARD_RULE_FORM_MESSAGES`. The cross-field checks (minimum not
 * above maximum, `active_to` not before `active_from`) stay on the server and
 * come back as `fields[]` entries, which `rewardRuleBmsFieldMap` places.
 *
 * Decimals stay strings from input to request body: the rate is a
 * `NUMERIC(9,4)` reward rate and the bounds are `S9(09)V99` amounts, and no
 * value is ever converted to a JavaScript number. Dates are entered as
 * `YYYY-MM-DD` (there is no date picker) and sent as midnight UTC instants,
 * except in an edit (`rewardRuleEditSchema`): a window boundary whose date is
 * left as the stored instant's is sent as that stored instant, unchanged, so
 * saving an edit never moves a time of day the form cannot show.
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

/**
 * The texts of the form's own checks: client-only English texts with no server
 * counterpart, so none of them is in crates/domain/src/messages.rs and none
 * belongs in `REWARD_RULE_MESSAGES`. Where the server refuses the same input,
 * its 400 names no field, so the form could not show that refusal on the
 * field. Each text names its field as the form labels it and says what the
 * field accepts. The dates keep zod's own `Invalid date`.
 */
export const REWARD_RULE_FORM_MESSAGES = {
  typeRequired: "Transaction type code is required",
  typeTooLong: "Transaction type code must be at most 2 characters",
  typeInvalid:
    "Transaction type code must contain only characters from U+0001 to U+00FF",
  categoryRequired: "Transaction category code is required",
  categoryInvalid: "Transaction category code must be 1 to 4 digits",
  rateRequired: "Reward rate is required",
  rateInvalid:
    "Reward rate must be a decimal number with up to 4 decimal places",
  minAmountInvalid:
    "Minimum transaction amount must be a number from -999999999.99 to 999999999.99 with up to 2 decimal places",
  maxAmountInvalid:
    "Maximum transaction amount must be a number from -999999999.99 to 999999999.99 with up to 2 decimal places",
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
// OpenAPI pattern of an S9(09)V99 amount, sign allowed: at most 9 integer
// digits after any leading zeros and at most 2 fraction digits, so
// -999999999.99 to 999999999.99, the range the server converts.
const AMOUNT_PATTERN = /^-?0*[0-9]{1,9}(\.[0-9]{1,2})?$/;

// TRAN-TYPE-CD PIC X(02): the OpenAPI maxLength and pattern, and not blank.
// The empty input passes the pattern, so it gets the required text alone.
const typeCode = z
  .string({ required_error: REWARD_RULE_FORM_MESSAGES.typeRequired })
  .min(1, { message: REWARD_RULE_FORM_MESSAGES.typeRequired })
  .max(2, { message: REWARD_RULE_FORM_MESSAGES.typeTooLong })
  .regex(TYPE_CODE_PATTERN, { message: REWARD_RULE_FORM_MESSAGES.typeInvalid });

// TRAN-CAT-CD PIC 9(04): the OpenAPI pattern. The empty input gets the
// required text alone, not also the pattern's.
const categoryCode = z
  .string({ required_error: REWARD_RULE_FORM_MESSAGES.categoryRequired })
  .superRefine((value, ctx) => {
    if (value === "") {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: REWARD_RULE_FORM_MESSAGES.categoryRequired,
      });
    } else if (!CATEGORY_CODE_PATTERN.test(value)) {
      ctx.addIssue({
        code: z.ZodIssueCode.invalid_string,
        validation: "regex",
        message: REWARD_RULE_FORM_MESSAGES.categoryInvalid,
      });
    }
  });

// The empty input, then the sign, then `REWARD_RATE_PATTERN`: one issue, with
// the text of the first check that fails.
const rewardRate = z
  .string({ required_error: REWARD_RULE_FORM_MESSAGES.rateRequired })
  .superRefine((value, ctx) => {
    if (value === "") {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: REWARD_RULE_FORM_MESSAGES.rateRequired,
      });
    } else if (value.startsWith("-")) {
      // Any leading minus, `-0` included, is reported as negative: the server
      // takes a negative zero as zero, but the form has no reason to accept a
      // signed rate.
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: REWARD_RULE_MESSAGES.rateNegative,
      });
    } else if (!REWARD_RATE_PATTERN.test(value)) {
      ctx.addIssue({
        code: z.ZodIssueCode.invalid_string,
        validation: "regex",
        message: REWARD_RULE_FORM_MESSAGES.rateInvalid,
      });
    }
  });

// An amount bound whose malformed or out-of-range text gets `message`.
const amount = (message: string) =>
  z.string().regex(AMOUNT_PATTERN, { message });

// `.date()` accepts only `YYYY-MM-DD` and only real calendar dates. A shape
// check alone would let `2022-02-30` through to the server, whose RFC 3339
// parse rejects it without naming a field.
const date = z.string().date();

const startOfDayUtc = (value: string): string => `${value}T00:00:00Z`;

/**
 * The `YYYY-MM-DD` date of an RFC 3339 UTC instant as the API writes it: the
 * date the form shows for a window boundary, and the date the reward-rule list
 * shows in its Active from and Active to columns. The text is cut, never
 * parsed, so the date is the instant's UTC date whatever the browser's zone.
 */
export const dateOf = (instant: string): string => instant.slice(0, 10);

// The instant a window boundary entered as the date `entered` is sent as: the
// boundary's `stored` instant, verbatim, when `entered` is its date, so an edit
// that leaves the date alone keeps the stored time of day and fraction;
// otherwise midnight UTC of `entered`. `null` is a boundary with no stored
// instant (a new rule, or an open-ended rule's end).
const boundaryInstant = (entered: string, stored: string | null): string =>
  stored !== null && dateOf(stored) === entered
    ? stored
    : startOfDayUtc(entered);

// An optional field: the empty input (or an absent key) is omitted from the
// body, which the server reads as `null` (an unbounded amount or an
// open-ended window).
const blankOr = <T extends z.ZodTypeAny>(schema: T) =>
  z.union([z.literal(""), schema]).optional();

const omitBlank = (value: string | undefined): string | undefined =>
  value === undefined || value === "" ? undefined : value;

// `active_from`, required, against its stored instant (`null` for a new rule).
const activeFrom = (stored: string | null) =>
  date.transform((entered) => boundaryInstant(entered, stored));

// `active_to`, blank for an open-ended window, against its stored instant
// (`null` for a new or open-ended rule). A cleared end is omitted, whatever
// was stored.
const activeTo = (stored: string | null) =>
  blankOr(date).transform((value) => {
    const entered = omitBlank(value);
    return entered === undefined ? undefined : boundaryInstant(entered, stored);
  });

/** The new-rule form: both window boundaries are sent as midnight UTC. */
export const rewardRuleSchema = z.object({
  transaction_type_cd: typeCode,
  transaction_category_cd: categoryCode,
  reward_rate: rewardRate,
  min_transaction_amount: blankOr(
    amount(REWARD_RULE_FORM_MESSAGES.minAmountInvalid),
  ).transform(omitBlank),
  max_transaction_amount: blankOr(
    amount(REWARD_RULE_FORM_MESSAGES.maxAmountInvalid),
  ).transform(omitBlank),
  active_from: activeFrom(null),
  active_to: activeTo(null),
}) satisfies z.ZodType<RewardRuleRequest>;

/** The form's field values as entered: every field a string. */
export type RewardRuleFormInput = z.input<typeof rewardRuleSchema>;

/** The parsed form: the `RewardRuleRequest` body to submit. */
export type RewardRuleFormValues = z.infer<typeof rewardRuleSchema>;

/**
 * The edit form of the stored `rule`: `rewardRuleSchema` but for the window.
 * `PUT /admin/reward-rules/{id}` replaces every column, so each boundary whose
 * entered date is the date of its stored instant is sent as that instant,
 * byte for byte; a changed date is sent as midnight UTC, a cleared end is
 * omitted (open-ended), and a date entered for an open-ended rule is sent as
 * midnight UTC.
 */
export function rewardRuleEditSchema(
  rule: RewardRuleResponse,
): typeof rewardRuleSchema {
  return rewardRuleSchema.extend({
    active_from: activeFrom(rule.active_from),
    active_to: activeTo(rule.active_to),
  }) satisfies z.ZodType<RewardRuleRequest>;
}

/**
 * The edit form's starting values for a stored rule. Instants show only their
 * date, as the form enters dates only; `rewardRuleEditSchema` of the same rule
 * sends a date left unchanged as the stored instant. A `null` bound or end
 * becomes the empty input.
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
