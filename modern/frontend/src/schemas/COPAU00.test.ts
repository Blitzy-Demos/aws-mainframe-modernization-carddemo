import { describe, expect, it } from "vitest";
import { COPAU00_LENGTHS, type Copau00Values, copau00Schema } from "./COPAU00";

/**
 * Client half of the three `both` rows `copau00Schema` repeats: the `ACCTID` length of
 * `modern/audit/screens.md` (`COPAU00-013`, 11, no message of its own, so zod's default) and the
 * two account edits of `modern/audit/online/COPAUS0C.md`, the blank account (`COPAUS0C-005`) and
 * the account that is not 11 ASCII digits (`COPAUS0C-006`). The expected messages are the audited
 * literals, written by hand. The five selectors are `client` rows the schema checks only for the
 * map length and the row count; which code selects is the screen's rule, so every character
 * passes here. Every value is synthetic.
 */

const ENTER_ACCT_ID_MESSAGE = "Please enter Acct Id...";

/** The space before `...` is part of the COBOL literal. */
const NOT_NUMERIC_MESSAGE = "Acct Id must be Numeric ...";

/** zod's default `max(11)` message: the map attribute carries no message of its own. */
const ACCT_ID_TOO_LONG_MESSAGE = "String must contain at most 11 character(s)";

/** zod's default `max(1)` message for a selector. */
const SELECTION_TOO_LONG_MESSAGE = "String must contain at most 1 character(s)";

/** zod's default `length(5)` message for the selector array. */
const ROW_COUNT_MESSAGE = "Array must contain exactly 5 element(s)";

/** Five blank selectors, as the map sends them when the operator selects nothing. */
const NO_SELECTION = ["", "", "", "", ""];

/** The `success` flag of parsing `values`, with the path and message of each issue in order. */
function check(values: Copau00Values) {
  const result = copau00Schema.safeParse(values);
  return {
    success: result.success,
    issues: result.error?.issues.map(({ path, message }) => ({ path, message })) ?? [],
  };
}

describe("copau00Schema", () => {
  it("declares the map lengths of the account field and the selectors, and five list rows", () => {
    expect(COPAU00_LENGTHS).toEqual({ acctId: 11, selection: 1, rows: 5 });
  });

  it.each([
    { name: "an account and no selection", acctId: "00000000050", selections: NO_SELECTION },
    { name: "an account and S on row 1", acctId: "12345678901", selections: ["S", "", "", "", ""] },
    { name: "an account and s on row 5", acctId: "12345678901", selections: ["", "", "", "", "s"] },
    {
      name: "an account and a selector on every row",
      acctId: "98765432109",
      selections: ["S", "s", "X", "1", " "],
    },
  ])("passes $name unchanged", ({ acctId, selections }) => {
    expect(copau00Schema.safeParse({ acctId, selections })).toEqual({
      success: true,
      data: { acctId, selections },
    });
  });

  // audit: COPAU00-013
  it("accepts an account ID of the map's 11 characters and rejects one of 12", () => {
    expect(check({ acctId: "12345678901", selections: NO_SELECTION })).toEqual({
      success: true,
      issues: [],
    });
    expect(check({ acctId: "123456789012", selections: NO_SELECTION })).toEqual({
      success: false,
      issues: [
        { path: ["acctId"], message: ACCT_ID_TOO_LONG_MESSAGE },
        { path: ["acctId"], message: NOT_NUMERIC_MESSAGE },
      ],
    });
  });

  // audit: COPAUS0C-005
  it.each([
    { name: "an empty field", acctId: "" },
    { name: "one space", acctId: " " },
    { name: "11 spaces", acctId: " ".repeat(11) },
    { name: "one low-value", acctId: "\u0000" },
    { name: "11 low-values", acctId: "\u0000".repeat(11) },
  ])("rejects the blank account ID of $name with the blank message alone", ({ acctId }) => {
    expect(check({ acctId, selections: NO_SELECTION })).toEqual({
      success: false,
      issues: [{ path: ["acctId"], message: ENTER_ACCT_ID_MESSAGE }],
    });
  });

  // audit: COPAUS0C-005
  it("accepts an account ID of 11 zeros, which is not blank", () => {
    expect(check({ acctId: "00000000000", selections: NO_SELECTION })).toEqual({
      success: true,
      issues: [],
    });
  });

  // audit: COPAUS0C-006
  it.each([
    { name: "a letter in the last position", acctId: "0000000005A" },
    { name: "a shorter digit string", acctId: "5" },
    { name: "10 digits and a trailing space", acctId: "0000000005 " },
    { name: "a leading space", acctId: " 0000000005" },
    { name: "10 spaces and one digit, which is not blank", acctId: `${" ".repeat(10)}5` },
    { name: "an embedded space", acctId: "00000 00005" },
    { name: "a sign", acctId: "-0000000005" },
    { name: "non-ASCII digits", acctId: "\u0660".repeat(11) },
    {
      name: "a space and a low-value, which is neither all spaces nor all low-values",
      acctId: " \u0000",
    },
  ])("rejects the non-numeric account ID of $name", ({ acctId }) => {
    expect(check({ acctId, selections: NO_SELECTION })).toEqual({
      success: false,
      issues: [{ path: ["acctId"], message: NOT_NUMERIC_MESSAGE }],
    });
  });

  // audit: COPAUS0C-006
  it.each(["00000000050", "99999999999"])("accepts the 11-digit account ID %j", (acctId) => {
    expect(check({ acctId, selections: NO_SELECTION })).toEqual({ success: true, issues: [] });
  });

  it("reports the length issue, then the blank message alone, for 12 spaces", () => {
    expect(check({ acctId: " ".repeat(12), selections: NO_SELECTION })).toEqual({
      success: false,
      issues: [
        { path: ["acctId"], message: ACCT_ID_TOO_LONG_MESSAGE },
        { path: ["acctId"], message: ENTER_ACCT_ID_MESSAGE },
      ],
    });
  });

  it.each([0, 1, 2, 3, 4])("rejects a selector of 2 characters on row index %i", (row) => {
    const selections = NO_SELECTION.map((value, index) => (index === row ? "SS" : value));
    expect(check({ acctId: "00000000050", selections })).toEqual({
      success: false,
      issues: [{ path: ["selections", row], message: SELECTION_TOO_LONG_MESSAGE }],
    });
  });

  it.each([
    { name: "4", selections: ["", "", "", ""] },
    { name: "6", selections: ["", "", "", "", "", ""] },
  ])("rejects $name selectors, where the map has 5 rows", ({ selections }) => {
    expect(check({ acctId: "00000000050", selections })).toEqual({
      success: false,
      issues: [{ path: ["selections"], message: ROW_COUNT_MESSAGE }],
    });
  });
});
