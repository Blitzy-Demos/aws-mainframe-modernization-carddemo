// Tests of the CT02 add form schema (schemas/transactions.ts): its issues
// follow the COTRN02C audit (Order, ID) order across fields, so the first
// issue is the message the server answers, and each field gets at most its
// own first issue. Expected texts are the docs/parity/validation-audit.md
// COTRN02C rows.
import { describe, expect, it } from "vitest";

import { transactionAddFormSchema, type TransactionAddFormValues } from "./transactions";

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
  ])("$name", ({ values, expected }) => {
    const issues = parseIssues(values);
    expect(issues.map(({ path, code }) => [String(path[0]), code])).toEqual(expected);
  });

  it("accepts a form every client edit accepts", () => {
    expect(transactionAddFormSchema.safeParse(VALID).success).toBe(true);
  });
});
