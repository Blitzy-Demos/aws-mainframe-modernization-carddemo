// Tests of the CT02 add form schema (schemas/transactions.ts): its issues
// follow the COTRN02C audit (Order, ID) order across fields, so the first
// issue is the message the server answers, and each field gets at most its
// own first issue; against the server's TransactionAddRequest constraints, the
// free-text members accept ASCII only, with zod's regex issue (no-message,
// E-49), and every member with a class or format edit answers a non-ASCII or
// malformed entry with its audited COTRN02C text. Expected texts are the
// docs/parity/validation-audit.md COTRN02C rows.
import { describe, expect, it } from "vitest";
import { z } from "zod";

import {
  type TransactionAddFormValues,
  transactionAddFormSchema,
  transactionAddMessages,
} from "./transactions";

/** A form every client edit accepts. */
const VALID: TransactionAddFormValues = {
  account_id: "00000000001",
  card_number: "",
  tran_type_cd: "01",
  tran_cat_cd: "0001",
  tran_source: "POS TERM",
  tran_desc: "Purchase",
  tran_amt: "+00000010.00",
  tran_orig_date: "2022-07-01",
  tran_proc_date: "2022-07-01",
  tran_merchant_id: "000000001",
  tran_merchant_name: "Merchant",
  tran_merchant_city: "City",
  tran_merchant_zip: "12345",
  confirm: "Y",
};

/** Every field blank, the confirmation included. */
const ALL_BLANK: TransactionAddFormValues = {
  account_id: "",
  card_number: "",
  tran_type_cd: "",
  tran_cat_cd: "",
  tran_source: "",
  tran_desc: "",
  tran_amt: "",
  tran_orig_date: "",
  tran_proc_date: "",
  tran_merchant_id: "",
  tran_merchant_name: "",
  tran_merchant_city: "",
  tran_merchant_zip: "",
  confirm: "",
};

type IssueSummary = [field: string, message: string];

function parseIssues(values: TransactionAddFormValues) {
  const result = transactionAddFormSchema.safeParse(values);
  return result.success ? [] : result.error.issues;
}

function issueSummaries(values: TransactionAddFormValues): IssueSummary[] {
  return parseIssues(values).map((issue) => [String(issue.path[0]), issue.message]);
}

interface OrderCase {
  name: string;
  values: TransactionAddFormValues;
  expected: IssueSummary[];
}

describe("transactionAddFormSchema issue order", () => {
  it.each<OrderCase>([
    {
      name: "a blank category (V-009, Order 3) before a non-numeric type (V-019, Order 4)",
      values: { ...VALID, tran_type_cd: "0A", tran_cat_cd: "" },
      expected: [
        ["tran_cat_cd", "Category CD can NOT be empty..."],
        ["tran_type_cd", "Type CD must be Numeric..."],
      ],
    },
    {
      name: "a blank zip (V-018, Order 3) before a malformed amount (V-021, Order 5)",
      values: { ...VALID, tran_amt: "12.50", tran_merchant_zip: "" },
      expected: [
        ["tran_merchant_zip", "Merchant Zip can NOT be empty..."],
        ["tran_amt", "Amount should be in format -99999999.99"],
      ],
    },
    {
      name: "a blank merchant name (V-016, Order 3) before a malformed original date (V-022, Order 6)",
      values: { ...VALID, tran_orig_date: "2022/07/01", tran_merchant_name: "" },
      expected: [
        ["tran_merchant_name", "Merchant Name can NOT be empty..."],
        ["tran_orig_date", "Orig Date should be in format YYYY-MM-DD"],
      ],
    },
    {
      name: "a malformed processing date (V-023, Order 7) before a non-numeric merchant ID (V-026, Order 10)",
      values: { ...VALID, tran_merchant_id: "12345678A", tran_proc_date: "20220701" },
      expected: [
        ["tran_proc_date", "Proc Date should be in format YYYY-MM-DD"],
        ["tran_merchant_id", "Merchant ID must be Numeric..."],
      ],
    },
    {
      name: "a non-numeric type (V-019) before a non-numeric category (V-020), both Order 4",
      values: { ...VALID, tran_type_cd: "0A", tran_cat_cd: "00A1" },
      expected: [
        ["tran_type_cd", "Type CD must be Numeric..."],
        ["tran_cat_cd", "Category CD must be Numeric..."],
      ],
    },
    {
      name: "a malformed amount (V-021, Order 5) before an invalid confirmation (V-028, Order 11)",
      values: { ...VALID, confirm: "X", tran_amt: "1" },
      expected: [
        ["tran_amt", "Amount should be in format -99999999.99"],
        ["confirm", "Invalid value. Valid values are (Y/N)..."],
      ],
    },
    {
      name: "every field blank: the key issue (V-003, Order 1), then each empty text in screen order",
      values: ALL_BLANK,
      expected: [
        ["account_id", "Account or Card Number must be entered..."],
        ["tran_type_cd", "Type CD can NOT be empty..."],
        ["tran_cat_cd", "Category CD can NOT be empty..."],
        ["tran_source", "Source can NOT be empty..."],
        ["tran_desc", "Description can NOT be empty..."],
        ["tran_amt", "Amount can NOT be empty..."],
        ["tran_orig_date", "Orig Date can NOT be empty..."],
        ["tran_proc_date", "Proc Date can NOT be empty..."],
        ["tran_merchant_id", "Merchant ID can NOT be empty..."],
        ["tran_merchant_name", "Merchant Name can NOT be empty..."],
        ["tran_merchant_city", "Merchant City can NOT be empty..."],
        ["tran_merchant_zip", "Merchant Zip can NOT be empty..."],
      ],
    },
    {
      name: "a non-numeric account ID (V-001, Order 1) before a blank type (V-008, Order 3)",
      values: { ...VALID, account_id: "0000000000A", tran_type_cd: "" },
      expected: [
        ["account_id", "Account ID must be Numeric..."],
        ["tran_type_cd", "Type CD can NOT be empty..."],
      ],
    },
  ])("$name", ({ values, expected }) => {
    expect(issueSummaries(values)).toEqual(expected);
  });

  it.each<{ name: string; values: TransactionAddFormValues; expected: string[][] }>([
    {
      name: "an over-length type code gets only its width issue",
      values: { ...VALID, tran_type_cd: "999" },
      expected: [["tran_type_cd", "too_big"]],
    },
    {
      name: "a width issue comes before the COBOL edits of other fields",
      values: { ...VALID, tran_desc: "D".repeat(61), tran_type_cd: "" },
      expected: [
        ["tran_desc", "too_big"],
        ["tran_type_cd", "custom"],
      ],
    },
    {
      name: "a non-ASCII free-text entry, as a width issue, comes before the COBOL edits of other fields",
      values: { ...VALID, tran_desc: "Caf\u00e9", tran_type_cd: "" },
      expected: [
        ["tran_desc", "invalid_string"],
        ["tran_type_cd", "custom"],
      ],
    },
  ])("$name", ({ values, expected }) => {
    const issues = parseIssues(values);
    expect(issues.map(({ path, code }) => [String(path[0]), code])).toEqual(expected);
  });

  it("accepts a form every client edit accepts", () => {
    expect(transactionAddFormSchema.safeParse(VALID).success).toBe(true);
  });
});

/** `[member, map width, can NOT be empty text]` of each CT02 free-text field. */
const FREE_TEXT = [
  ["tran_source", 10, transactionAddMessages.sourceCanNotBeEmpty],
  ["tran_desc", 60, transactionAddMessages.descriptionCanNotBeEmpty],
  ["tran_merchant_name", 30, transactionAddMessages.merchantNameCanNotBeEmpty],
  ["tran_merchant_city", 25, transactionAddMessages.merchantCityCanNotBeEmpty],
  ["tran_merchant_zip", 10, transactionAddMessages.merchantZipCanNotBeEmpty],
] as const;

/** Every audited COTRN02C text the schema can report. */
const AUDITED: readonly string[] = Object.values(transactionAddMessages);

function issuesOf(values: TransactionAddFormValues): z.ZodIssue[] {
  const result = transactionAddFormSchema.safeParse(values);
  return result.success ? [] : result.error.issues;
}

function issuesAt(
  values: TransactionAddFormValues,
  member: keyof TransactionAddFormValues,
): z.ZodIssue[] {
  return issuesOf(values).filter((issue) => issue.path[0] === member);
}

/** Every printable ASCII character, repeated to `width`. */
function printableAscii(width: number): string {
  const printable = Array.from({ length: 0x7e - 0x20 + 1 }, (_, index) =>
    String.fromCharCode(0x20 + index),
  ).join("");
  return printable.repeat(Math.ceil(width / printable.length)).slice(0, width);
}

describe("transactionAddFormSchema", () => {
  it("accepts the valid entry", () => {
    expect(issuesOf(VALID)).toEqual([]);
  });

  describe.each(FREE_TEXT)("%s", (member, width, emptyMessage) => {
    it.each([printableAscii(width), "\u007f", "Cafe"])(
      "accepts the ASCII entry %j",
      (entered) => {
        expect(issuesAt({ ...VALID, [member]: entered }, member)).toEqual([]);
      },
    );

    it.each([
      "\u00e9",
      "\u0080",
      "\u20ac",
      "\u{1f600}",
      "Caf\u00e9",
      "\u00e9".repeat(width),
    ])("rejects %j with zod's regex issue and no audited text", (entered) => {
      const issues = issuesAt({ ...VALID, [member]: entered }, member);
      expect(issues).toHaveLength(1);
      expect(issues[0]).toMatchObject({
        code: z.ZodIssueCode.invalid_string,
        validation: "regex",
      });
      expect(AUDITED).not.toContain(issues[0]?.message);
    });

    it.each(["", " ".repeat(width), "\u0000".repeat(width)])(
      "answers the blank entry %j with its can NOT be empty text",
      (entered) => {
        expect(
          issuesAt({ ...VALID, [member]: entered }, member).map((issue) => issue.message),
        ).toEqual([emptyMessage]);
      },
    );
  });

  // The members whose class or format edit rejects a non-ASCII entry, which
  // the server leaves to that edit and its audited text.
  it.each([
    ["account_id", "\u00e9", transactionAddMessages.accountIdMustBeNumeric],
    ["tran_type_cd", "\u00e9", transactionAddMessages.typeCdMustBeNumeric],
    ["tran_type_cd", "1A", transactionAddMessages.typeCdMustBeNumeric],
    ["tran_cat_cd", "00\u00e9", transactionAddMessages.categoryCdMustBeNumeric],
    ["tran_cat_cd", "001", transactionAddMessages.categoryCdMustBeNumeric],
    ["tran_amt", "-0000001\u00e9.34", transactionAddMessages.amountShouldBeInFormat],
    ["tran_amt", "00000012.345", transactionAddMessages.amountShouldBeInFormat],
    ["tran_orig_date", "2022-07-\u00e9", transactionAddMessages.origDateShouldBeInFormat],
    ["tran_orig_date", "2022/07/01", transactionAddMessages.origDateShouldBeInFormat],
    ["tran_proc_date", "\u{1f600}", transactionAddMessages.procDateShouldBeInFormat],
    ["tran_proc_date", "22-07-0001", transactionAddMessages.procDateShouldBeInFormat],
    ["tran_merchant_id", "80000000\u00e9", transactionAddMessages.merchantIdMustBeNumeric],
    ["tran_merchant_id", "8000000-1", transactionAddMessages.merchantIdMustBeNumeric],
    ["confirm", "\u00e9", transactionAddMessages.invalidConfirmValue],
  ] as const)("answers %s %j with its audited text", (member, entered, message) => {
    expect(
      issuesAt({ ...VALID, [member]: entered }, member).map((issue) => issue.message),
    ).toEqual([message]);
  });

  it("answers a non-ASCII card number with its audited text when no account ID is entered", () => {
    expect(
      issuesAt({ ...VALID, account_id: "", card_number: "\u20ac" }, "card_number").map(
        (issue) => issue.message,
      ),
    ).toEqual([transactionAddMessages.cardNumberMustBeNumeric]);
  });

  // The dates' published pattern is their shape only: the calendar check and
  // its 2513 exception are CSUTLDTC's, server-side.
  it.each(["2022-13-45", "0000-00-00", "9999-99-99"])(
    "leaves the calendar of the date %j to the server",
    (entered) => {
      expect(issuesOf({ ...VALID, tran_orig_date: entered, tran_proc_date: entered })).toEqual(
        [],
      );
    },
  );
});
