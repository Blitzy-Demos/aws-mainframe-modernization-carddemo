/**
 * CR00 transaction report request (`CORPT00C`, map `CORPT00`): the client
 * mirror of
 * `GET /reports/transactions?report_type=&start_date=&end_date=&confirm=`.
 *
 * The map's three selectors `MONTHLY`, `YEARLY` and `CUSTOM` become one
 * `report_type`, the six date parts `SDTYYYY`/`SDTMM`/`SDTDD` and
 * `EDTYYYY`/`EDTMM`/`EDTDD` are entered as one `YYYY-MM-DD` text per date
 * (there is no date picker), and `CONFIRM` is `confirm` (E-48). The server
 * splits each date at its first two hyphens and runs every `CORPT00C` edit on
 * the parts as entered; it stays authoritative, the `CSUTLDTC` calendar check
 * and the confirmation prompt included.
 *
 * `CORPT00C` answers only the first edit that fails (P-CORPT00C-002), so this
 * schema reports at most one rule issue, the one the server would answer, in
 * the audit order of the `V-CORPT00C` rows: the report type; for a custom
 * report the six empty checks, then the six value edits, then each date's
 * `YYYY-MM-DD` form, start before end; last the confirmation. A query value
 * longer than its OpenAPI `maxLength` fails its own field and stops the rule,
 * as the server checks the lengths before the rule runs.
 */
import { z } from "zod";

import type { components, paths } from "@/api/schema";

type ReportQuery = NonNullable<
  paths["/reports/transactions"]["get"]["parameters"]["query"]
>;

type ReportType = NonNullable<ReportQuery["report_type"]>;

/** The query a valid form produces: a report type is always selected. */
type ReportRequest = {
  report_type: ReportType;
  start_date?: ReportQuery["start_date"];
  end_date?: ReportQuery["end_date"];
  confirm?: ReportQuery["confirm"];
};

/**
 * The audited `CORPT00C` texts the client answers itself, byte for byte as
 * in `docs/parity/validation-audit.md`. The confirmation prompt
 * (V-CORPT00C-016) and the cleared screen of `N` (V-CORPT00C-017, no message)
 * are server answers and have no entry.
 */
const REPORT_MESSAGES = {
  // V-CORPT00C-001
  selectReportType: "Select a report type to print report...",
  // V-CORPT00C-002
  startMonthEmpty: "Start Date - Month can NOT be empty...",
  // V-CORPT00C-003
  startDayEmpty: "Start Date - Day can NOT be empty...",
  // V-CORPT00C-004
  startYearEmpty: "Start Date - Year can NOT be empty...",
  // V-CORPT00C-005
  endMonthEmpty: "End Date - Month can NOT be empty...",
  // V-CORPT00C-006
  endDayEmpty: "End Date - Day can NOT be empty...",
  // V-CORPT00C-007
  endYearEmpty: "End Date - Year can NOT be empty...",
  // V-CORPT00C-008
  startMonthInvalid: "Start Date - Not a valid Month...",
  // V-CORPT00C-009
  startDayInvalid: "Start Date - Not a valid Day...",
  // V-CORPT00C-010
  startYearInvalid: "Start Date - Not a valid Year...",
  // V-CORPT00C-011
  endMonthInvalid: "End Date - Not a valid Month...",
  // V-CORPT00C-012
  endDayInvalid: "End Date - Not a valid Day...",
  // V-CORPT00C-013
  endYearInvalid: "End Date - Not a valid Year...",
  // V-CORPT00C-014
  startDateInvalid: "Start Date - Not a valid date...",
  // V-CORPT00C-015
  endDateInvalid: "End Date - Not a valid date...",
  // V-CORPT00C-018: the opening quote, CONFIRMI DELIMITED BY SPACE, then this
  // tail.
  invalidConfirmQuote: '"',
  invalidConfirmTail: '" is not a valid value to confirm...',
} as const;

/** The OpenAPI `report_type` values, in the map's selector order. */
const REPORT_TYPES = [
  "monthly",
  "yearly",
  "custom",
] as const satisfies readonly ReportType[];

/**
 * The OpenAPI `maxLength` of `start_date` and `end_date`: `SDTYYYYI X(4)`,
 * `SDTMMI X(2)`, `SDTDDI X(2)` and two hyphens.
 */
const DATE_TEXT_MAX_LENGTH = 10;

/** The OpenAPI `maxLength` of `confirm`: `CONFIRMI X(1)`. */
const CONFIRM_MAX_LENGTH = 1;

/**
 * The client's date form, the `CSUTLDTC` picture `YYYY-MM-DD`. The server
 * also accepts parts `NUMVAL-C` reads into its numeric fields, such as the
 * `1` of `2022-1-1`; the client does not reproduce that reading and asks for
 * this form instead. Whether a date in this form is a calendar date stays the
 * server's `CSUTLDTC` check.
 */
const DATE_PATTERN = /^\d{4}-\d{2}-\d{2}$/;

/** One date part: its map field width, its value limit and its two texts. */
interface DatePartRule {
  readonly width: number;
  /** The `> '12'` or `> '31'` operand; the year has none. */
  readonly limit?: string;
  readonly empty: string;
  readonly invalid: string;
}

/** One date: its form field, its parts in screen order and its date text. */
interface DateRule {
  readonly field: "start_date" | "end_date";
  readonly month: DatePartRule;
  readonly day: DatePartRule;
  readonly year: DatePartRule;
  readonly invalidDate: string;
}

const START_DATE: DateRule = {
  field: "start_date",
  month: {
    width: 2,
    limit: "12",
    empty: REPORT_MESSAGES.startMonthEmpty,
    invalid: REPORT_MESSAGES.startMonthInvalid,
  },
  day: {
    width: 2,
    limit: "31",
    empty: REPORT_MESSAGES.startDayEmpty,
    invalid: REPORT_MESSAGES.startDayInvalid,
  },
  year: {
    width: 4,
    empty: REPORT_MESSAGES.startYearEmpty,
    invalid: REPORT_MESSAGES.startYearInvalid,
  },
  invalidDate: REPORT_MESSAGES.startDateInvalid,
};

const END_DATE: DateRule = {
  field: "end_date",
  month: {
    width: 2,
    limit: "12",
    empty: REPORT_MESSAGES.endMonthEmpty,
    invalid: REPORT_MESSAGES.endMonthInvalid,
  },
  day: {
    width: 2,
    limit: "31",
    empty: REPORT_MESSAGES.endDayEmpty,
    invalid: REPORT_MESSAGES.endDayInvalid,
  },
  year: {
    width: 4,
    empty: REPORT_MESSAGES.endYearEmpty,
    invalid: REPORT_MESSAGES.endYearInvalid,
  },
  invalidDate: REPORT_MESSAGES.endDateInvalid,
};

/** The first rule issue: the form field it stands on and its text. */
interface Rejection {
  readonly field: keyof ReportRequest;
  readonly message: string;
}

/** A date part as the server receives it, with the rule that edits it. */
interface ReceivedPart {
  readonly rule: DatePartRule;
  readonly text: string;
  readonly field: DateRule["field"];
}

/**
 * The COBOL `= SPACES OR LOW-VALUES` test of entered text: empty, all spaces
 * or all NUL characters.
 */
const isBlank = (text: string): boolean =>
  /^ *$/.test(text) || /^\0*$/.test(text);

/**
 * One `YYYY-MM-DD` text split at its first two hyphens into year, month and
 * day exactly as entered, as the server splits it: a missing part is empty,
 * and text after a third hyphen stays in the day.
 */
function splitDate(
  value: string | undefined,
): readonly [year: string, month: string, day: string] {
  const text = value ?? "";
  const first = text.indexOf("-");
  if (first < 0) {
    return [text, "", ""];
  }
  const second = text.indexOf("-", first + 1);
  if (second < 0) {
    return [text.slice(0, first), text.slice(first + 1), ""];
  }
  return [
    text.slice(0, first),
    text.slice(first + 1, second),
    text.slice(second + 1),
  ];
}

/** One entered date with the rules of its parts. */
interface EnteredDate {
  readonly rule: DateRule;
  readonly value: string | undefined;
}

/** The parts of one date in screen order: month, day, year. */
function receiveParts({ rule, value }: EnteredDate): ReceivedPart[] {
  const [year, month, day] = splitDate(value);
  return [
    { rule: rule.month, text: month, field: rule.field },
    { rule: rule.day, text: day, field: rule.field },
    { rule: rule.year, text: year, field: rule.field },
  ];
}

/**
 * A part blank in its map field: no longer than the field and `SPACES` or
 * `LOW-VALUES`. Text longer than the field is never blank.
 */
const isBlankPart = ({ rule, text }: ReceivedPart): boolean =>
  text.length <= rule.width && isBlank(text);

/**
 * The value edit of one part (`IS NOT NUMERIC OR > '12'`, `> '31'`, year
 * `IS NOT NUMERIC`), where the client can decide it. A part longer than its
 * map field fails, as the server rejects it with the part's own text (E-28).
 * A part of exactly its width in digits is compared as the COBOL compares the
 * alphanumeric field. Any other part depends on how `NUMVAL-C` reads it, which
 * only the server decides; the client's date form rejects it instead.
 */
function failsValueEdit({ rule, text }: ReceivedPart): boolean {
  if (text.length > rule.width) {
    return true;
  }
  const digitsOnly = text.length === rule.width && /^\d+$/.test(text);
  return digitsOnly && rule.limit !== undefined && text > rule.limit;
}

/**
 * The custom range edits in the `CORPT00C` order: the empty checks of the six
 * parts (V-CORPT00C-002 to V-CORPT00C-007), their value edits
 * (V-CORPT00C-008 to V-CORPT00C-013), then the `YYYY-MM-DD` form of the start
 * and the end date (V-CORPT00C-014, V-CORPT00C-015).
 */
function customRangeRejection(
  startDate: string | undefined,
  endDate: string | undefined,
): Rejection | undefined {
  const dates: readonly EnteredDate[] = [
    { rule: START_DATE, value: startDate },
    { rule: END_DATE, value: endDate },
  ];
  // SDTMM, SDTDD, SDTYYYY, EDTMM, EDTDD, EDTYYYY.
  const parts = dates.flatMap(receiveParts);
  const blank = parts.find(isBlankPart);
  if (blank !== undefined) {
    return { field: blank.field, message: blank.rule.empty };
  }
  const invalid = parts.find(failsValueEdit);
  if (invalid !== undefined) {
    return { field: invalid.field, message: invalid.rule.invalid };
  }
  const malformed = dates.find(({ value }) => !DATE_PATTERN.test(value ?? ""));
  if (malformed !== undefined) {
    return { field: malformed.rule.field, message: malformed.rule.invalidDate };
  }
  return undefined;
}

/**
 * `CONFIRMI X(1)` as `DELIMITED BY SPACE` reads it: the one-byte field holds
 * an ASCII character as entered, while a character the server encodes in more
 * than one byte does not fit and leaves the field a space, read as nothing.
 */
const confirmFieldText = (value: string): string =>
  /^[\x00-\x7f]$/.test(value) ? value : "";

/**
 * The confirmation edit (V-CORPT00C-018): `Y`, `y`, `N`, `n` and a blank value
 * are the server's to answer (it runs the report, clears the form, or asks for
 * confirmation); any other value is quoted back.
 */
function confirmRejection(confirm: string | undefined): Rejection | undefined {
  if (confirm === undefined || isBlank(confirm) || /^[YyNn]$/.test(confirm)) {
    return undefined;
  }
  const { invalidConfirmQuote, invalidConfirmTail } = REPORT_MESSAGES;
  return {
    field: "confirm",
    message: `${invalidConfirmQuote}${confirmFieldText(confirm)}${invalidConfirmTail}`,
  };
}

/** An optional query text: `""` is not sent, any other value goes as entered. */
const optionalText = (maxLength: number) =>
  z
    .string()
    .max(maxLength)
    .optional()
    .transform((value) => (value === "" ? undefined : value));

/**
 * The CR00 request form. Its output is the query `GET /reports/transactions`
 * takes: `start_date` and `end_date` only for a custom report, whose dates the
 * server reads alone; monthly and yearly derive theirs from the current date.
 */
export const reportRequestSchema = z
  .object({
    // A missing, blank or unknown selection is no selection (V-CORPT00C-001).
    report_type: z.enum(REPORT_TYPES, {
      errorMap: () => ({ message: REPORT_MESSAGES.selectReportType }),
    }),
    start_date: optionalText(DATE_TEXT_MAX_LENGTH),
    end_date: optionalText(DATE_TEXT_MAX_LENGTH),
    confirm: optionalText(CONFIRM_MAX_LENGTH),
  })
  .superRefine((values, ctx) => {
    const rejection =
      (values.report_type === "custom"
        ? customRangeRejection(values.start_date, values.end_date)
        : undefined) ?? confirmRejection(values.confirm);
    if (rejection !== undefined) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: [rejection.field],
        message: rejection.message,
      });
    }
  })
  .transform(
    ({ start_date, end_date, ...rest }): ReportRequest =>
      rest.report_type === "custom" ? { ...rest, start_date, end_date } : rest,
  ) satisfies z.ZodType<
  Pick<ReportQuery, "report_type" | "start_date" | "end_date" | "confirm">
>;

/** The validated request: the query parameters the page sends. */
export type ReportRequestFormValues = z.infer<typeof reportRequestSchema>;

/** The form's input, for react-hook-form default values. */
export type ReportRequestFormInput = z.input<typeof reportRequestSchema>;

/**
 * The form field each `CORPT0A` field of a server error stands on: the date
 * parts on their date, `MONTHLY` (no report type, and the form the
 * confirmation `N` clears) on the report type.
 */
export const reportRequestBmsFieldMap: Record<
  components["schemas"]["ReportRequestField"],
  keyof ReportRequestFormValues
> = {
  MONTHLY: "report_type",
  SDTMM: "start_date",
  SDTDD: "start_date",
  SDTYYYY: "start_date",
  EDTMM: "end_date",
  EDTDD: "end_date",
  EDTYYYY: "end_date",
  CONFIRM: "confirm",
};
