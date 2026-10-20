import { describe, expect, it } from "vitest";
import { COTRTLI_LENGTHS, type CotrtliValues, cotrtliSchema } from "./COTRTLI";

/**
 * Client half of the sixteen `both` rows of the transaction type list map `CTRTLIA` (mapset
 * `COTRTLI`) in `modern/audit/screens.md`: the map `LENGTH` of the type filter `TRTYPE`
 * (`COTRTLI-015`, 2), of the description filter `TRDESC` (`COTRTLI-018`, 50), and of each list
 * row's selector `TRTSEL1`–`TRTSEL7` (1) and description `TRTYPD1`–`TRTYPD7` (50). The rows
 * record no message, so the expected messages are zod's defaults, and every expected length is
 * written by hand from `app/app-transaction-type-db2/bms/COTRTLI.bms`.
 * `online/COTRTLIC.md` has no `both` row: COTRTLIC's edits, the invalid action code
 * (`COTRTLIC-021`), more than one action (`COTRTLIC-022`) and the type filter that is not two
 * digits (`COTRTLIC-028`) included, are the server's, so those inputs pass here. Every value is
 * synthetic.
 */

/** Zod's default `max` message, which every length rejection carries. */
const tooLong = (length: number): string => `String must contain at most ${length} character(s)`;

/** Zod's default `length` message for a list that does not hold exactly seven rows. */
const NOT_SEVEN_ROWS = "Array must contain exactly 7 element(s)";

/** A page of five synthetic types and two empty rows, with no selection, as ENTER sends it. */
const VALUES: CotrtliValues = {
  typeFilter: "",
  descFilter: "SAMPLE",
  rows: [
    { selection: "", description: "SAMPLE TYPE ALPHA" },
    { selection: "", description: "SAMPLE TYPE BRAVO" },
    { selection: "", description: "SAMPLE TYPE CHARLIE" },
    { selection: "", description: "SAMPLE TYPE DELTA" },
    { selection: "", description: "SAMPLE TYPE ECHO" },
    { selection: "", description: "" },
    { selection: "", description: "" },
  ],
};

/** The seven list rows: the 0-based index in an issue's path and the map fields of that row. */
const LIST_ROWS = [
  { index: 0, selector: "TRTSEL1", description: "TRTYPD1" },
  { index: 1, selector: "TRTSEL2", description: "TRTYPD2" },
  { index: 2, selector: "TRTSEL3", description: "TRTYPD3" },
  { index: 3, selector: "TRTSEL4", description: "TRTYPD4" },
  { index: 4, selector: "TRTSEL5", description: "TRTYPD5" },
  { index: 5, selector: "TRTSEL6", description: "TRTYPD6" },
  { index: 6, selector: "TRTSEL7", description: "TRTYPD7" },
] as const;

/** `VALUES` with each row whose 0-based index `changes` names merged with its change. */
function withRows(
  changes: Readonly<Record<number, Partial<CotrtliValues["rows"][number]>>>,
): CotrtliValues {
  return { ...VALUES, rows: VALUES.rows.map((row, index) => ({ ...row, ...changes[index] })) };
}

/** The `success` flag of parsing `values`, with the path and message of each issue in order. */
function check(values: CotrtliValues) {
  const result = cotrtliSchema.safeParse(values);
  return {
    success: result.success,
    issues: result.error?.issues.map(({ path, message }) => ({ path, message })) ?? [],
  };
}

describe("COTRTLI_LENGTHS", () => {
  it("holds the map LENGTH of each input field and the seven list rows", () => {
    expect(COTRTLI_LENGTHS).toEqual({
      typeFilter: 2,
      descFilter: 50,
      selection: 1,
      description: 50,
      rows: 7,
    });
  });
});

describe("cotrtliSchema", () => {
  it("parses a valid page of seven rows unchanged", () => {
    expect(cotrtliSchema.safeParse(VALUES)).toEqual({ success: true, data: VALUES });
  });

  // audit: COTRTLI-015
  it("accepts a type filter of the map's 2 characters and rejects one of 3", () => {
    expect(check({ ...VALUES, typeFilter: "12" })).toEqual({ success: true, issues: [] });
    expect(check({ ...VALUES, typeFilter: "123" })).toEqual({
      success: false,
      issues: [{ path: ["typeFilter"], message: tooLong(2) }],
    });
  });

  // audit: COTRTLI-018
  it("accepts a description filter of the map's 50 characters and rejects one of 51", () => {
    expect(check({ ...VALUES, descFilter: "A".repeat(50) })).toEqual({
      success: true,
      issues: [],
    });
    expect(check({ ...VALUES, descFilter: "A".repeat(51) })).toEqual({
      success: false,
      issues: [{ path: ["descFilter"], message: tooLong(50) }],
    });
  });

  // audit: COTRTLI-026
  // audit: COTRTLI-032
  // audit: COTRTLI-038
  // audit: COTRTLI-044
  // audit: COTRTLI-050
  // audit: COTRTLI-056
  // audit: COTRTLI-062
  it.each(LIST_ROWS)(
    "accepts $selector of the map's 1 character and rejects 2 as its row's selection",
    ({ index }) => {
      expect(check(withRows({ [index]: { selection: "D" } }))).toEqual({
        success: true,
        issues: [],
      });
      expect(check(withRows({ [index]: { selection: "DU" } }))).toEqual({
        success: false,
        issues: [{ path: ["rows", index, "selection"], message: tooLong(1) }],
      });
    },
  );

  // audit: COTRTLI-030
  // audit: COTRTLI-036
  // audit: COTRTLI-042
  // audit: COTRTLI-048
  // audit: COTRTLI-054
  // audit: COTRTLI-060
  // audit: COTRTLI-066
  it.each(LIST_ROWS)(
    "accepts $description of the map's 50 characters and rejects 51 as its row's description",
    ({ index }) => {
      expect(check(withRows({ [index]: { description: "B".repeat(50) } }))).toEqual({
        success: true,
        issues: [],
      });
      expect(check(withRows({ [index]: { description: "B".repeat(51) } }))).toEqual({
        success: false,
        issues: [{ path: ["rows", index, "description"], message: tooLong(50) }],
      });
    },
  );

  it.each([
    { count: 6, rows: VALUES.rows.slice(0, 6) },
    { count: 8, rows: [...VALUES.rows, { selection: "", description: "" }] },
  ])("rejects a list of $count rows, since the map addresses exactly seven", ({ count, rows }) => {
    expect(rows).toHaveLength(count);
    expect(check({ ...VALUES, rows })).toEqual({
      success: false,
      issues: [{ path: ["rows"], message: NOT_SEVEN_ROWS }],
    });
  });

  it("reports the type filter, then the description filter, then the rows", () => {
    expect(
      check({
        ...withRows({ 3: { selection: "UU", description: "C".repeat(51) } }),
        typeFilter: "123",
        descFilter: "A".repeat(51),
      }),
    ).toEqual({
      success: false,
      issues: [
        { path: ["typeFilter"], message: tooLong(2) },
        { path: ["descFilter"], message: tooLong(50) },
        { path: ["rows", 3, "selection"], message: tooLong(1) },
        { path: ["rows", 3, "description"], message: tooLong(50) },
      ],
    });
  });

  it.each([
    { name: "an action code other than U or D", values: withRows({ 2: { selection: "X" } }) },
    { name: "a lower-case u, an invalid code", values: withRows({ 0: { selection: "u" } }) },
    {
      name: "two actions, a U and a D",
      values: withRows({ 1: { selection: "U" }, 4: { selection: "D" } }),
    },
    {
      name: "one U with an edited description",
      values: withRows({ 1: { selection: "U", description: "SAMPLE TYPE BRAVO EDITED" } }),
    },
    { name: "a one-digit type filter", values: { ...VALUES, typeFilter: "5" } },
    { name: "a type filter holding a letter", values: { ...VALUES, typeFilter: "A1" } },
    { name: "a type filter with an embedded space", values: { ...VALUES, typeFilter: "1 " } },
    {
      name: "values with surrounding spaces",
      values: {
        ...withRows({ 0: { selection: "U", description: "  SAMPLE TYPE ALPHA  " } }),
        descFilter: "  SAMPLE  ",
      },
    },
  ])("leaves $name to the server, passing it unchanged", ({ values }) => {
    expect(cotrtliSchema.safeParse(values)).toEqual({ success: true, data: values });
  });
});
