import { describe, expect, it } from "vitest";
import { CORPT00_LENGTHS, type Corpt00Values, corpt00Schema } from "./CORPT00";

/**
 * Unit tests of the `CORPT0A` form schema of the transaction reports screen. They prove the client
 * half of the ten `both` rows of `modern/audit/screens.md`: the map `LENGTH` of the three report
 * selectors and the confirmation (`CORPT00-012`, `CORPT00-015`, `CORPT00-018`, `CORPT00-038`), and
 * the `LENGTH` and `NUM` of the six date parts (`CORPT00-022` to `CORPT00-034`). The rows record
 * no message, so every rejection carries zod's default. `modern/audit/online/CORPT00C.md` and
 * `modern/audit/online/CSUTLDTC.md` have no `both` row: the report selection, the blank-part,
 * month and day edits and the confirmation are `server` rows endpoint 14 owns, and calendar
 * validity is halt H5, so inputs those edits reject pass here and no program message is asserted.
 * Every expected length is written by hand from `app/bms/CORPT00.bms`. All values are synthetic.
 */

/** Zod's default `max` message, which every length rejection carries. */
const tooLong = (length: number): string => `String must contain at most ${length} character(s)`;

/** Zod's default `regex` message, for a date part that is not digits only (`NUM`). */
const NOT_DIGITS = "Invalid";

/** The form as the screen first sends it: every input empty. */
const BLANK: Corpt00Values = {
  monthly: "",
  yearly: "",
  custom: "",
  startMonth: "",
  startDay: "",
  startYear: "",
  endMonth: "",
  endDay: "",
  endYear: "",
  confirm: "",
};

/** A confirmed custom report over a synthetic date range, every part at its map length. */
const CUSTOM_RANGE: Corpt00Values = {
  ...BLANK,
  custom: "X",
  startMonth: "01",
  startDay: "05",
  startYear: "2023",
  endMonth: "03",
  endDay: "31",
  endYear: "2023",
  confirm: "Y",
};

type ParseCase = { name: string; values: Corpt00Values };
type SingleCharacterField = "monthly" | "yearly" | "custom" | "confirm";
type DatePartField = "startMonth" | "startDay" | "startYear" | "endMonth" | "endDay" | "endYear";
type DatePartCase = { field: DatePartField; length: number };

/** The `success` flag of parsing `values`, with the path and message of each issue in order. */
function check(values: Corpt00Values) {
  const result = corpt00Schema.safeParse(values);
  return {
    success: result.success,
    issues: result.error?.issues.map(({ path, message }) => ({ path, message })) ?? [],
  };
}

describe("CORPT00_LENGTHS", () => {
  it("holds the map LENGTH of each of the ten inputs, in map order", () => {
    expect(CORPT00_LENGTHS).toEqual({
      monthly: 1,
      yearly: 1,
      custom: 1,
      startMonth: 2,
      startDay: 2,
      startYear: 4,
      endMonth: 2,
      endDay: 2,
      endYear: 4,
      confirm: 1,
    });
    expect(Object.keys(CORPT00_LENGTHS)).toEqual(Object.keys(corpt00Schema.shape));
  });
});

describe("corpt00Schema", () => {
  // Every CORPT00C edit is the server's, so values the report selection, the blank-part, month,
  // day and confirmation edits reject pass here exactly as typed.
  it.each<ParseCase>([
    { name: "every input empty, the screen as first sent", values: BLANK },
    { name: "a confirmed monthly report", values: { ...BLANK, monthly: "X", confirm: "Y" } },
    { name: "a yearly report confirmed with y", values: { ...BLANK, yearly: "X", confirm: "y" } },
    { name: "a confirmed custom report with both dates", values: CUSTOM_RANGE },
    { name: "a custom report awaiting its confirmation", values: { ...CUSTOM_RANGE, confirm: "" } },
    {
      name: "a custom report with one-digit parts, which the server zero-fills",
      values: { ...CUSTOM_RANGE, startMonth: "1", startDay: "5", startYear: "23", endDay: "9" },
    },
    {
      name: "no report type selected, an edit the server owns",
      values: { ...BLANK, confirm: "Y" },
    },
    {
      name: "a custom report with every date part blank, edits the server owns",
      values: { ...BLANK, custom: "X" },
    },
    {
      name: "a custom report with only the end year blank, an edit the server owns",
      values: { ...CUSTOM_RANGE, endYear: "" },
    },
    {
      name: "a start month of 13 and an end day of 32, edits the server owns",
      values: { ...CUSTOM_RANGE, startMonth: "13", endDay: "32" },
    },
    {
      name: "a month and a day of 00, which only the server's date edit can reject",
      values: { ...CUSTOM_RANGE, startMonth: "00", endDay: "00" },
    },
    {
      name: "an end date before the start date, which no edit compares",
      values: { ...CUSTOM_RANGE, startYear: "2024", endYear: "2023" },
    },
    {
      name: "a confirmation other than Y or N, an edit the server owns",
      values: { ...CUSTOM_RANGE, confirm: "X" },
    },
    {
      name: "a selector holding a space, read as blank by the server",
      values: { ...BLANK, monthly: " " },
    },
    {
      name: "every selector marked at once, the server taking the first",
      values: { ...CUSTOM_RANGE, monthly: "X", yearly: "X" },
    },
  ])("parses $name unchanged", ({ values }) => {
    expect(corpt00Schema.safeParse(values)).toEqual({ success: true, data: values });
  });

  // Calendar validity is CSUTLDTC's verdict on the server, halt H5, so impossible dates pass.
  it.each<ParseCase>([
    {
      name: "a custom start date of 02/30/2023",
      values: { ...CUSTOM_RANGE, startMonth: "02", startDay: "30", startYear: "2023" },
    },
    {
      name: "a custom end date of 04/31/2023",
      values: { ...CUSTOM_RANGE, endMonth: "04", endDay: "31", endYear: "2023" },
    },
    {
      name: "a custom start date of 02/29 in a common year",
      values: { ...CUSTOM_RANGE, startMonth: "02", startDay: "29", startYear: "2023" },
    },
  ])("parses $name unchanged, its validity being the server's", ({ values }) => {
    expect(corpt00Schema.safeParse(values)).toEqual({ success: true, data: values });
  });

  // The selectors and the confirmation carry no NUM attribute, so a letter or a digit fits alike.
  it.each<{ field: SingleCharacterField }>([
    { field: "monthly" }, // audit: CORPT00-012
    { field: "yearly" }, // audit: CORPT00-015
    { field: "custom" }, // audit: CORPT00-018
    { field: "confirm" }, // audit: CORPT00-038
  ])("accepts $field at its map LENGTH 1 and rejects one character more", ({ field }) => {
    expect(CORPT00_LENGTHS[field]).toBe(1);
    for (const value of ["", "X", "y", "1"]) {
      const boundary = { ...CUSTOM_RANGE, [field]: value };
      expect(corpt00Schema.safeParse(boundary)).toEqual({ success: true, data: boundary });
    }
    expect(check({ ...CUSTOM_RANGE, [field]: "XY" })).toEqual({
      success: false,
      issues: [{ path: [field], message: tooLong(1) }],
    });
  });

  it.each<DatePartCase>([
    { field: "startMonth", length: 2 }, // audit: CORPT00-022
    { field: "startDay", length: 2 }, // audit: CORPT00-024
    { field: "startYear", length: 4 }, // audit: CORPT00-026
    { field: "endMonth", length: 2 }, // audit: CORPT00-030
    { field: "endDay", length: 2 }, // audit: CORPT00-032
    { field: "endYear", length: 4 }, // audit: CORPT00-034
  ])(
    "accepts digits in $field up to its map LENGTH $length and rejects more or a non-digit",
    ({ field, length }) => {
      expect(CORPT00_LENGTHS[field]).toBe(length);
      for (const value of ["", "1", "0".repeat(length), "9".repeat(length)]) {
        const accepted = { ...CUSTOM_RANGE, [field]: value };
        expect(corpt00Schema.safeParse(accepted)).toEqual({ success: true, data: accepted });
      }
      expect(check({ ...CUSTOM_RANGE, [field]: "9".repeat(length + 1) })).toEqual({
        success: false,
        issues: [{ path: [field], message: tooLong(length) }],
      });
      // NUMVAL-C would read a sign, a decimal point or spaces; the map's NUM sends digits only.
      for (const value of ["1a", "A", " 1", "-1", "1."]) {
        expect(check({ ...CUSTOM_RANGE, [field]: value })).toEqual({
          success: false,
          issues: [{ path: [field], message: NOT_DIGITS }],
        });
      }
    },
  );

  // LENGTH and NUM are two attributes the terminal enforced on one field, so a part breaking both
  // reports both issues, in the schema's order: the length first.
  it("reports the length and then the digits when a date part breaks both", () => {
    expect(check({ ...CUSTOM_RANGE, startYear: "20a23" })).toEqual({
      success: false,
      issues: [
        { path: ["startYear"], message: tooLong(4) },
        { path: ["startYear"], message: NOT_DIGITS },
      ],
    });
  });

  // The date parts are reported in the program's order: start month, day and year, then end
  // month, day and year, so the first issue is the part the server would also name first.
  it("reports every non-digit date part in program order", () => {
    const result = check({
      ...CUSTOM_RANGE,
      startMonth: "1a",
      startDay: "1a",
      startYear: "1a",
      endMonth: "1a",
      endDay: "1a",
      endYear: "1a",
    });
    expect(result.success).toBe(false);
    expect(result.issues).toEqual(
      (["startMonth", "startDay", "startYear", "endMonth", "endDay", "endYear"] as const).map(
        (field) => ({ path: [field], message: NOT_DIGITS }),
      ),
    );
  });

  it("reports one issue for each input over its map LENGTH, in map order", () => {
    const lengths = Object.entries(CORPT00_LENGTHS);
    const over = Object.fromEntries(
      lengths.map(([field, length]) => [field, "9".repeat(length + 1)]),
    );
    const result = corpt00Schema.safeParse(over);
    expect(result.success).toBe(false);
    expect(result.error?.issues.map(({ path, message }) => ({ path, message }))).toEqual(
      lengths.map(([field, length]) => ({ path: [field], message: tooLong(length) })),
    );
  });
});
