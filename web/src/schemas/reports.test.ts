// Tests of the CR00 request schema (schemas/reports.ts) at the encoding
// checks it mirrors from the server's ReportQuery: a confirmation outside its
// ASCII pattern fails its own field with zod's regex issue (no message, E-49)
// and stops the rule, as the server rejects it before CORPT00C's edits run; an
// ASCII confirmation other than Y, y, N, n or blank gets the V-CORPT00C-018
// text with the value quoted as entered; the dates have no ASCII pattern, so
// a non-ASCII date gets a date edit's issue. Expected texts are the
// docs/parity/validation-audit.md CORPT00C rows.
import { describe, expect, it } from "vitest";
import { z } from "zod";

import { type ReportRequestFormInput, reportRequestSchema } from "./reports";

// A custom request every client edit accepts: the TRANREPT range, confirmed.
const CONFIRMED: ReportRequestFormInput = {
  report_type: "custom",
  start_date: "2022-01-01",
  end_date: "2022-07-06",
  confirm: "Y",
};

// V-CORPT00C-018 after the quoted `CONFIRMI`.
const INVALID_CONFIRM_TAIL = '" is not a valid value to confirm...';

type IssueSummary = [field: string, code: string];

function parseIssues(values: ReportRequestFormInput) {
  const result = reportRequestSchema.safeParse(values);
  return result.success ? [] : result.error.issues;
}

function issueSummaries(values: ReportRequestFormInput): IssueSummary[] {
  return parseIssues(values).map((issue) => [String(issue.path[0]), issue.code]);
}

// The issues of a confirmation that fails its own field checks: every one on
// `confirm` with zod's code, the regex issue among them, and no rule issue.
// The code lists the over-width issue too for a character of two UTF-16 code
// units, which `maxLength` counts as two.
function expectFieldChecksOnly(values: ReportRequestFormInput, codes: string[]) {
  const issues = parseIssues(values);
  expect(issues.map((issue) => [String(issue.path[0]), issue.code])).toEqual(
    codes.map((code) => ["confirm", code]),
  );
  expect(issues).toContainEqual(
    expect.objectContaining({ code: z.ZodIssueCode.invalid_string, validation: "regex" }),
  );
  for (const issue of issues) {
    expect(issue.message).not.toContain(INVALID_CONFIRM_TAIL);
  }
}

const REGEX_ONLY = [z.ZodIssueCode.invalid_string];
const OVER_WIDTH_AND_REGEX = [z.ZodIssueCode.too_big, z.ZodIssueCode.invalid_string];

describe("reportRequestSchema confirmation", () => {
  // Characters of two, three and four UTF-8 bytes, which `CONFIRMI X(1)`
  // cannot hold.
  it.each([
    ["\u00e9", REGEX_ONLY],
    ["\u20ac", REGEX_ONLY],
    ["\u00ff", REGEX_ONLY],
    ["\u0080", REGEX_ONLY],
    ["\u{1f600}", OVER_WIDTH_AND_REGEX],
  ])("rejects the non-ASCII confirmation %j with zod's field issues alone", (confirm, codes) => {
    expectFieldChecksOnly({ ...CONFIRMED, confirm }, codes);
  });

  interface StopCase {
    name: string;
    values: ReportRequestFormInput;
    codes: string[];
  }

  it.each<StopCase>([
    {
      name: "a monthly report",
      values: { report_type: "monthly", confirm: "\u00e9" },
      codes: REGEX_ONLY,
    },
    {
      name: "an empty start month, which the rule would answer first",
      values: { ...CONFIRMED, start_date: "2022--01", confirm: "\u20ac" },
      codes: REGEX_ONLY,
    },
    {
      name: "a malformed end date, which the rule would answer first",
      values: { ...CONFIRMED, end_date: "2022/07/06", confirm: "\u{1f600}" },
      codes: OVER_WIDTH_AND_REGEX,
    },
  ])("answers a non-ASCII confirmation on $name before the rule", ({ values, codes }) => {
    expectFieldChecksOnly(values, codes);
  });

  it.each([
    ["X", '"X" is not a valid value to confirm...'],
    ["1", '"1" is not a valid value to confirm...'],
    ['"', '""" is not a valid value to confirm...'],
    ["\u007f", '"\u007f" is not a valid value to confirm...'],
  ])("quotes the ASCII confirmation %j as entered (V-CORPT00C-018)", (confirm, message) => {
    expect(
      parseIssues({ ...CONFIRMED, confirm }).map((issue) => [String(issue.path[0]), issue.message]),
    ).toEqual([["confirm", message]]);
  });

  // Y and y run the report, N and n clear the form, and a blank or missing
  // confirmation asks for it: all the server's answers.
  it.each(["Y", "y", "N", "n", "", " ", "\u0000", undefined])(
    "leaves the confirmation %j to the server",
    (confirm) => {
      expect(parseIssues({ ...CONFIRMED, confirm })).toEqual([]);
    },
  );

  it("sends a confirmed custom request as entered", () => {
    expect(reportRequestSchema.parse(CONFIRMED)).toEqual(CONFIRMED);
  });
});

describe("reportRequestSchema dates", () => {
  // The server's value edits read every date part and answer a non-ASCII one
  // with its own text, so the client checks no date encoding.
  it.each(["2022-0\u00e9-01", "\u00e9022-01-01", "2022-01-\u00e9", "\u{1f600}"])(
    "gives the non-ASCII start date %j a date edit's issue, not an encoding issue",
    (start_date) => {
      expect(issueSummaries({ ...CONFIRMED, start_date })).toEqual([
        ["start_date", z.ZodIssueCode.custom],
      ]);
    },
  );
});
