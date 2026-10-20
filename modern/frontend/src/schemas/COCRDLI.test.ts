import { describe, expect, it } from "vitest";
import { COCRDLI_LENGTHS, type CocrdliValues, cocrdliSchema } from "./COCRDLI";

/**
 * Client half of the five `both` rows of the card-list map `CCRDLIA` (`COCRDLI`): the map `LENGTH`
 * of `ACCTSID` (`COCRDLI-015`, 11) and of `CARDSID` (`COCRDLI-018`, 16) in
 * `modern/audit/screens.md`, which record no message, so the expected messages are zod's defaults;
 * and COCRDLIC's filter edits (`COCRDLIC-006`, `COCRDLIC-008`, `COCRDLIC-009`) in
 * `modern/audit/online/COCRDLIC.md`, whose messages are the audited literals, written by hand.
 *
 * The filter-edit rows are HALTED(X-COCRDLIC-H2): the COBOL result for a filter that is neither
 * blank nor all digits is unpredictable, so their tests are reported BLOCKED and assert only the
 * replacement's evident path that the rows state, never a parity result.
 *
 * The selectors' map `LENGTH` and the seven list rows are checked untagged: those rows, and the
 * selection edits (`COCRDLIC-012`, `COCRDLIC-013`), are `client` rows the screen applies, so every
 * selection code passes here. Every value is synthetic.
 */

const ACCOUNT_FILTER_MESSAGE = "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER";
const CARD_FILTER_MESSAGE = "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER";

/** Zod's default `max` message: the map attribute carries no message of its own. */
const tooLong = (length: number): string => `String must contain at most ${length} character(s)`;

/** Zod's default `length` message for the selection array. */
const ROWS_MESSAGE = "Array must contain exactly 7 element(s)";

/** An account ID and a card number at their full map lengths; the card is assembled from parts. */
const ACCT_ID = "12345678901";
const CARD_NUM = ["4444", "3333", "2222", "1111"].join("");

/** Seven blank selection codes, one per list row. */
const NO_SELECTIONS = ["", "", "", "", "", "", ""];

const VALID: CocrdliValues = { acctId: ACCT_ID, cardNum: CARD_NUM, selections: NO_SELECTIONS };

type Issue = { path: (string | number)[]; message: string };
type ParseCase = { name: string; values: CocrdliValues };
type FilterCase = { name: string; acctId: string; cardNum: string };

/** The `success` flag of parsing `values`, with the path and message of each issue in order. */
function check(values: CocrdliValues): { success: boolean; issues: Issue[] } {
  const result = cocrdliSchema.safeParse(values);
  return {
    success: result.success,
    issues: result.error?.issues.map(({ path, message }) => ({ path, message })) ?? [],
  };
}

/** The seven selection codes with `code` on the 0-based row `row` and blanks elsewhere. */
function selectionsWith(row: number, code: string): string[] {
  return NO_SELECTIONS.map((blank, at) => (at === row ? code : blank));
}

describe("cocrdliSchema", () => {
  it("holds the map LENGTH of each input and the number of list rows", () => {
    expect(COCRDLI_LENGTHS).toEqual({ acctId: 11, cardNum: 16, selection: 1, rows: 7 });
  });

  it.each<ParseCase>([
    { name: "both filters at their full width of digits", values: VALID },
    { name: "blank filters", values: { ...VALID, acctId: "", cardNum: "" } },
    {
      name: "filters of spaces only, which the program reads as blank",
      values: { ...VALID, acctId: " ".repeat(11), cardNum: "   " },
    },
    {
      name: "all-zero filters, which are numeric",
      values: { ...VALID, acctId: "0".repeat(11), cardNum: "0".repeat(16) },
    },
    {
      name: "selection codes S, U, X and s, which only the screen edits",
      values: { ...VALID, selections: ["S", "U", "X", "s", "", "", ""] },
    },
  ])("parses $name unchanged", ({ values }) => {
    expect(cocrdliSchema.safeParse(values)).toEqual({ success: true, data: values });
  });

  it.each([
    // audit: COCRDLI-015
    { field: "acctId", length: 11 },
    // audit: COCRDLI-018
    { field: "cardNum", length: 16 },
  ] as const)("accepts $field at $length characters and rejects one more", ({ field, length }) => {
    expect(check({ ...VALID, [field]: "9".repeat(length) })).toEqual({ success: true, issues: [] });
    expect(check({ ...VALID, [field]: "9".repeat(length + 1) })).toEqual({
      success: false,
      issues: [{ path: [field], message: tooLong(length) }],
    });
  });

  it.each([
    { field: "acctId", value: "ABCDEFGHIJKL", length: 11 },
    { field: "cardNum", value: "ABCDEFGHIJKLMNOPQ", length: 16 },
  ] as const)(
    "reports the over-long $field $value by its length alone",
    ({ field, value, length }) => {
      expect(check({ ...VALID, [field]: value })).toEqual({
        success: false,
        issues: [{ path: [field], message: tooLong(length) }],
      });
    },
  );

  it("keeps the card message when the account is rejected for its length alone", () => {
    expect(check({ ...VALID, acctId: "ABCDEFGHIJKL", cardNum: "ABC" })).toEqual({
      success: false,
      issues: [
        { path: ["acctId"], message: tooLong(11) },
        { path: ["cardNum"], message: CARD_FILTER_MESSAGE },
      ],
    });
  });

  it.each([0, 1, 2, 3, 4, 5, 6])("accepts 1 character on selection %i and rejects 2", (row) => {
    expect(check({ ...VALID, selections: selectionsWith(row, "S") })).toEqual({
      success: true,
      issues: [],
    });
    expect(check({ ...VALID, selections: selectionsWith(row, "SS") })).toEqual({
      success: false,
      issues: [{ path: ["selections", row], message: tooLong(1) }],
    });
  });

  it.each([6, 8])("rejects %i selection codes, as the list has exactly 7 rows", (count) => {
    const selections = Array.from({ length: count }, () => "");
    expect(check({ ...VALID, selections })).toEqual({
      success: false,
      issues: [{ path: ["selections"], message: ROWS_MESSAGE }],
    });
  });
});

describe("BLOCKED X-COCRDLIC-H2: filter edits on the evident path, not a parity result", () => {
  // audit: COCRDLIC-006
  it.each(["1234567890", "1234567890A", " 1234567890", "ABCDEFGHIJK"])(
    "rejects the account filter %j with the account message",
    (acctId) => {
      expect(check({ ...VALID, acctId })).toEqual({
        success: false,
        issues: [{ path: ["acctId"], message: ACCOUNT_FILTER_MESSAGE }],
      });
    },
  );

  // audit: COCRDLIC-006
  it.each([
    { name: "empty", acctId: "" },
    { name: "of 11 spaces", acctId: " ".repeat(11) },
    { name: "of 11 digits", acctId: ACCT_ID },
  ])("accepts an account filter $name", ({ acctId }) => {
    expect(check({ ...VALID, acctId })).toEqual({ success: true, issues: [] });
  });

  // audit: COCRDLIC-008
  it.each<FilterCase>([
    { name: "15 digits with a valid account", acctId: ACCT_ID, cardNum: CARD_NUM.slice(0, 15) },
    {
      name: "a trailing letter with a blank account",
      acctId: "",
      cardNum: `${CARD_NUM.slice(1)}X`,
    },
    {
      name: "a leading space with an account of spaces",
      acctId: " ".repeat(11),
      cardNum: ` ${CARD_NUM.slice(1)}`,
    },
    {
      name: "letters with an all-zero account",
      acctId: "0".repeat(11),
      cardNum: "ABCDEFGHIJKLMNOP",
    },
  ])("rejects a card filter of $name with the card message", ({ acctId, cardNum }) => {
    expect(check({ ...VALID, acctId, cardNum })).toEqual({
      success: false,
      issues: [{ path: ["cardNum"], message: CARD_FILTER_MESSAGE }],
    });
  });

  // audit: COCRDLIC-008
  it.each([
    { name: "empty", cardNum: "" },
    { name: "of 16 spaces", cardNum: " ".repeat(16) },
    { name: "of 16 digits", cardNum: CARD_NUM },
  ])("accepts a card filter $name", ({ cardNum }) => {
    expect(check({ ...VALID, cardNum })).toEqual({ success: true, issues: [] });
  });

  // audit: COCRDLIC-009
  it.each<FilterCase>([
    { name: "short digits", acctId: "12345", cardNum: CARD_NUM.slice(0, 8) },
    { name: "letters", acctId: "ABCDEFGHIJK", cardNum: "ABCDEFGHIJKLMNOP" },
  ])(
    "reports each filter of $name with the account message, account first",
    ({ acctId, cardNum }) => {
      expect(check({ ...VALID, acctId, cardNum })).toEqual({
        success: false,
        issues: [
          { path: ["acctId"], message: ACCOUNT_FILTER_MESSAGE },
          { path: ["cardNum"], message: ACCOUNT_FILTER_MESSAGE },
        ],
      });
    },
  );

  // audit: COCRDLIC-009
  it("accepts both filters at their full width of digits", () => {
    expect(check({ ...VALID, acctId: ACCT_ID, cardNum: CARD_NUM })).toEqual({
      success: true,
      issues: [],
    });
  });
});
