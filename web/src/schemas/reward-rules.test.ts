// Tests of the admin reward-rule form schema (schemas/reward-rules.ts) against
// the server's RewardRuleRequest rate rule: the OpenAPI pattern
// `^-?[0-9]+(\.[0-9]+)?$`, then RewardRate's NUMERIC(9,4) scale check, made
// after trailing fraction zeros are dropped. A rate whose digits beyond the
// fourth fraction place are all zeros is accepted and submitted as entered; a
// non-zero digit there, or text that is not a plain decimal, gets a regex issue
// with the form's own rate text; any leading minus gets the request-written
// negative-rate message.
//
// Every field check reports an explicit text: the request-written texts of
// REWARD_RULE_MESSAGES, or the client-only texts of REWARD_RULE_FORM_MESSAGES,
// never a zod default, but for the dates' settled `Invalid date`.
//
// The window tests hold the edit schema (rewardRuleEditSchema) to the PUT that
// replaces a stored rule: a boundary date left unchanged is sent as the stored
// instant byte for byte, and only a changed or newly entered date becomes
// midnight UTC.
import { describe, expect, it } from "vitest";
import { z } from "zod";

import type { components } from "@/api/schema";

import {
  REWARD_RULE_FORM_MESSAGES,
  REWARD_RULE_MESSAGES,
  type RewardRuleFormInput,
  type RewardRuleFormValues,
  rewardRuleEditSchema,
  rewardRuleSchema,
  rewardRuleToFormInput,
} from "./reward-rules";

type RewardRuleRequest = components["schemas"]["RewardRuleRequest"];
type RewardRuleResponse = components["schemas"]["RewardRuleResponse"];

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

  it.each(["1.50001", "0.00001", "1.12345", "1.", ".5", "1.5.0", "abc", " 1"])(
    "rejects the rate %j with a regex issue and the form's rate text",
    (reward_rate) => {
      const issues = issuesOf(ruleWith(reward_rate));
      expect(issues).toHaveLength(1);
      expect(issues[0]).toMatchObject({
        path: ["reward_rate"],
        code: z.ZodIssueCode.invalid_string,
        validation: "regex",
        message: REWARD_RULE_FORM_MESSAGES.rateInvalid,
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

type AmountField = "min_transaction_amount" | "max_transaction_amount";

const AMOUNT_FIELDS: readonly AmountField[] = [
  "min_transaction_amount",
  "max_transaction_amount",
];

/** The text each amount bound's malformed or out-of-range entry gets. */
const AMOUNT_MESSAGES: Record<AmountField, string> = {
  min_transaction_amount: REWARD_RULE_FORM_MESSAGES.minAmountInvalid,
  max_transaction_amount: REWARD_RULE_FORM_MESSAGES.maxAmountInvalid,
};

/** A rule every client edit accepts but for the amount bound `field`. */
function ruleWithBound(field: AmountField, value: string): RewardRuleFormInput {
  return { ...ruleWith("1.5"), [field]: value };
}

// The S9(09)V99 range of the published pattern
// `^-?0*[0-9]{1,9}(\.[0-9]{1,2})?$`, the text the server converts: at most 9
// integer digits after any leading zeros and at most 2 fraction digits.
describe.each(AMOUNT_FIELDS)("rewardRuleSchema %s", (field) => {
  it.each([
    "999999999.99",
    "-999999999.99",
    "0000000001.00",
    "000000000999999999.99",
    "0",
    "-0.00",
    "10.5",
  ])("accepts the amount %j and submits it unchanged", (value) => {
    const result = rewardRuleSchema.safeParse(ruleWithBound(field, value));
    expect(result.success).toBe(true);
    expect(result.data?.[field]).toBe(value);
  });

  it.each([
    "1000000000.00",
    "-1000000000.00",
    "1000000000",
    "01000000000",
    "1.234",
    "1.",
    ".5",
    "+1",
    "1e3",
    "ten",
  ])("rejects the amount %j on its field with the range text", (value) => {
    const issues = issuesOf(ruleWithBound(field, value));
    expect(issues).toHaveLength(1);
    expect(issues[0]).toMatchObject({
      path: [field],
      message: AMOUNT_MESSAGES[field],
    });
  });
});

// The form's empty entry, as the new-rule page starts.
const EMPTY_ENTRY: RewardRuleFormInput = {
  transaction_type_cd: "",
  transaction_category_cd: "",
  reward_rate: "",
  min_transaction_amount: "",
  max_transaction_amount: "",
  active_from: "",
  active_to: "",
};

// Each field's issue texts, keyed by the field.
function messagesByField(
  values: Partial<RewardRuleFormInput>,
): Record<string, string[]> {
  const result = rewardRuleSchema.safeParse(values);
  const messages: Record<string, string[]> = {};
  for (const issue of result.success ? [] : result.error.issues) {
    const field = String(issue.path[0]);
    messages[field] = [...(messages[field] ?? []), issue.message];
  }
  return messages;
}

describe("rewardRuleSchema field texts", () => {
  const texts = REWARD_RULE_FORM_MESSAGES;

  // The empty optional fields are omitted from the body; each required field
  // gets one text, its own, and the start date keeps zod's `Invalid date`.
  it("reports each empty required field with its required text", () => {
    expect(messagesByField(EMPTY_ENTRY)).toEqual({
      transaction_type_cd: [texts.typeRequired],
      transaction_category_cd: [texts.categoryRequired],
      reward_rate: [texts.rateRequired],
      active_from: ["Invalid date"],
    });
  });

  // An absent member is the empty field.
  it("reports each absent required code and the rate with its required text", () => {
    expect(messagesByField({ active_from: "2022-01-01" })).toEqual({
      transaction_type_cd: [texts.typeRequired],
      transaction_category_cd: [texts.categoryRequired],
      reward_rate: [texts.rateRequired],
    });
  });

  it.each<[keyof RewardRuleFormInput, string, string]>([
    ["transaction_type_cd", "\u20ac", texts.typeInvalid],
    ["transaction_type_cd", "\u0000", texts.typeInvalid],
    ["transaction_type_cd", "012", texts.typeTooLong],
    ["transaction_category_cd", "12345", texts.categoryInvalid],
    ["transaction_category_cd", "1a", texts.categoryInvalid],
    ["transaction_category_cd", "-1", texts.categoryInvalid],
    ["transaction_category_cd", " ", texts.categoryInvalid],
  ])("rejects %s %j with its text alone", (field, value, message) => {
    expect(messagesByField({ ...ruleWith("1.5"), [field]: value })).toEqual({
      [field]: [message],
    });
  });
});

// A stored rule of type `07`, category `0001`, as `GET /admin/reward-rules`
// writes it, with the given window. The form conversion never reads the
// server's `status`, so every rule carries the same one.
function storedRule(
  active_from: string,
  active_to: string | null,
): RewardRuleResponse {
  return {
    rule_id: "3",
    transaction_type_cd: "07",
    transaction_category_cd: "0001",
    reward_rate: "1.2500",
    min_transaction_amount: "5.00",
    max_transaction_amount: null,
    active_from,
    active_to,
    status: "active",
  };
}

// The JSON body the client sends for the parsed form: a key whose value is
// `undefined` is left out, which the server reads as `null`.
function sentBody(values: RewardRuleFormValues): RewardRuleRequest {
  return JSON.parse(JSON.stringify(values));
}

// A non-midnight start and an end with a microsecond fraction.
const TIMED_FROM = "2022-07-01T12:00:00Z";
const TIMED_TO = "2030-01-01T06:30:00.123456Z";
// A soft-deleted rule: DELETE set the end to the processing clock's instant.
const ENDED_FROM = "2022-03-15T08:15:30Z";
const ENDED_TO = "2026-11-04T10:00:14.437498Z";

describe("rewardRuleEditSchema active window", () => {
  it.each<{
    name: string;
    stored: [string, string | null];
    entered: [string, string];
    sent: Pick<RewardRuleRequest, "active_from" | "active_to">;
  }>([
    {
      name: "keeps a non-midnight start and a microsecond end whose dates are unchanged",
      stored: [TIMED_FROM, TIMED_TO],
      entered: ["2022-07-01", "2030-01-01"],
      sent: { active_from: TIMED_FROM, active_to: TIMED_TO },
    },
    {
      name: "keeps a soft-deleted rule's start and end whose dates are unchanged",
      stored: [ENDED_FROM, ENDED_TO],
      entered: ["2022-03-15", "2026-11-04"],
      sent: { active_from: ENDED_FROM, active_to: ENDED_TO },
    },
    {
      name: "sends a changed start as midnight UTC and keeps the end",
      stored: [TIMED_FROM, TIMED_TO],
      entered: ["2022-08-01", "2030-01-01"],
      sent: { active_from: "2022-08-01T00:00:00Z", active_to: TIMED_TO },
    },
    {
      name: "sends a changed end as midnight UTC and keeps the start",
      stored: [TIMED_FROM, TIMED_TO],
      entered: ["2022-07-01", "2031-06-30"],
      sent: { active_from: TIMED_FROM, active_to: "2031-06-30T00:00:00Z" },
    },
    {
      // Each boundary is compared with its own stored instant only.
      name: "sends an end moved to the start's date as midnight UTC",
      stored: [TIMED_FROM, TIMED_TO],
      entered: ["2022-07-01", "2022-07-01"],
      sent: { active_from: TIMED_FROM, active_to: "2022-07-01T00:00:00Z" },
    },
    {
      name: "omits a cleared end, leaving the rule open-ended",
      stored: [TIMED_FROM, TIMED_TO],
      entered: ["2022-07-01", ""],
      sent: { active_from: TIMED_FROM },
    },
    {
      name: "omits the blank end of an open-ended rule",
      stored: [TIMED_FROM, null],
      entered: ["2022-07-01", ""],
      sent: { active_from: TIMED_FROM },
    },
    {
      name: "sends an end entered for an open-ended rule as midnight UTC",
      stored: [TIMED_FROM, null],
      entered: ["2022-07-01", "2030-01-01"],
      sent: { active_from: TIMED_FROM, active_to: "2030-01-01T00:00:00Z" },
    },
  ])("$name", ({ stored, entered, sent }) => {
    const rule = storedRule(...stored);
    const entry: RewardRuleFormInput = {
      ...rewardRuleToFormInput(rule),
      active_from: entered[0],
      active_to: entered[1],
    };

    const result = rewardRuleEditSchema(rule).safeParse(entry);

    expect(result.success).toBe(true);
    if (!result.success) {
      return;
    }
    expect(sentBody(result.data)).toStrictEqual({
      transaction_type_cd: "07",
      transaction_category_cd: "0001",
      reward_rate: "1.2500",
      min_transaction_amount: "5.00",
      ...sent,
    });
  });

  it("shows each stored instant as its date", () => {
    expect(rewardRuleToFormInput(storedRule(TIMED_FROM, TIMED_TO))).toMatchObject(
      { active_from: "2022-07-01", active_to: "2030-01-01" },
    );
    expect(rewardRuleToFormInput(storedRule(TIMED_FROM, null))).toMatchObject({
      active_to: "",
    });
  });

  // A new rule has no stored instant, so the same dates become midnight UTC.
  it("leaves rewardRuleSchema sending both dates as midnight UTC", () => {
    const entry = rewardRuleToFormInput(storedRule(TIMED_FROM, TIMED_TO));

    const result = rewardRuleSchema.safeParse(entry);

    expect(result.success).toBe(true);
    if (!result.success) {
      return;
    }
    expect(sentBody(result.data)).toMatchObject({
      active_from: "2022-07-01T00:00:00Z",
      active_to: "2030-01-01T00:00:00Z",
    });
  });

  // The edit schema replaces only the window's conversion, never a check.
  it.each<[string, Partial<RewardRuleFormInput>]>([
    ["a malformed rate", { reward_rate: "abc" }],
    ["a negative rate", { reward_rate: "-1" }],
    ["a blank type code", { transaction_type_cd: "" }],
    ["a start that is no calendar date", { active_from: "2022-02-30" }],
    ["an end that is not YYYY-MM-DD", { active_to: "2030-1-1" }],
  ])("rejects %s with the issues rewardRuleSchema reports", (_, change) => {
    const rule = storedRule(TIMED_FROM, TIMED_TO);
    const entry: RewardRuleFormInput = {
      ...rewardRuleToFormInput(rule),
      ...change,
    };

    const edited = rewardRuleEditSchema(rule).safeParse(entry);
    const created = rewardRuleSchema.safeParse(entry);

    expect(edited.success).toBe(false);
    expect(edited.error?.issues).toEqual(created.error?.issues);
  });
});
