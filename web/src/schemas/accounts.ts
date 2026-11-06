/**
 * Form schemas of the account screens: the account-number lookup of CAVW
 * (`GET /accounts/{id}`, `COACTVWC`, map `COACTVW`), used by the dashboard
 * and the account view, and the account update form of CAUP
 * (`PUT /accounts/{id}`, `COACTUPC` with `CSUTLDPY`, map `COACTUP`).
 *
 * They mirror the server and do not replace it: each rejection here is a
 * `COACTVWC` or `COACTUPC` row of `docs/parity/validation-audit.md` whose
 * trigger holds for the value as entered, with that row's text byte for byte,
 * so a form never sends input the server is certain to refuse. Values are
 * checked as entered and never trimmed or re-cased, because the server applies
 * the COBOL edits to the text it receives.
 *
 * Left to the server, which answers with the audited text itself: the
 * `CSLKPCDY` state, area-code and state/zip lookups, the `CSUTLDPY` century,
 * month, day and calendar edits, the `CSUTLDTC` call and the birth-date clock
 * check, the FICO range and the SSN area-number edit, the comparison with the
 * fetched values ('No change detected with respect to values fetched.'), the
 * record-changed check (409), the reads, and the `version` token check (400
 * with no message, E-24).
 */
import { z } from "zod";

import type { components, paths } from "@/api/schema";

// The body of `PUT /accounts/{id}`.
type AccountUpdateRequest = components["schemas"]["AccountUpdateRequest"];

// A map field `COACTVWC` 2210-EDIT-ACCOUNT and `COACTUPC` 1100-RECEIVE-MAP
// read as not entered: empty, spaces, or `*` followed only by spaces (the
// space-padded `= '*'` comparison). Only the space is blank, as in the COBOL
// comparison; a tab or any other character is entered text. This is narrower
// than `trim()`, which would also call tabs blank and miss `*`.
const NOT_ENTERED = /^(?: *|\* *)$/;

// Every character a digit (`IS NUMERIC` of the entered characters).
const DIGITS = /^[0-9]+$/;

// Every character the digit zero (`= ZEROES`).
const ALL_ZEROS = /^0+$/;

// Every character a letter or a space: `INSPECT … CONVERTING` turns the 52
// letters into spaces and the rest must then be blank (1225-EDIT-ALPHA-REQD,
// 1235-EDIT-ALPHA-OPT).
const ALPHABETIC = /^[A-Za-z ]*$/;

// Text `FUNCTION TEST-NUMVAL-C` (currency sign `$`) accepts under some reading
// of the IBM formats, `sp` being spaces and `num` digits with an optional
// decimal point and fraction, or a decimal point and digits:
// `[sp][+|-][sp][$][sp]num[sp]` or `[sp][$][sp]num[sp][+|-|CR|DB][sp]`.
// Grouping commas in the integer part and a lower-case `cr` or `db` are
// accepted too: the server holds those inputs at halt (E-40) and answers them
// itself, so only text no reading accepts gets the "is not valid" text.
const NUMVAL_C_ARGUMENT =
  /^ *(?:[+-] *(?:\$ *)?(?:[0-9,]*[0-9][0-9,]*(?:\.[0-9]*)?|,*\.[0-9]+) *|(?:\$ *)?(?:[0-9,]*[0-9][0-9,]*(?:\.[0-9]*)?|,*\.[0-9]+) *(?:(?:[+-]|[Cc][Rr]|[Dd][Bb]) *)?)$/;

// `ACCTSIDI PIC X(11)` [app/cpy-bms/COACTVW.CPY], `CC-ACCT-ID X(11)`.
const ACCOUNT_ID_LENGTH = 11;

// --- COACTVW: the account number lookup (CAVW)

/**
 * The `COACTVWC` texts of the account number, which every lookup of the
 * `ACCT-ID 9(11)` key shows (also `web/src/schemas/rewards.ts`).
 *
 * 2210-EDIT-ACCOUNT sets 'Account number not provided' and the
 * 'Account number must be a non zero 11 digit number' 88-levels, but the
 * screen never shows them: 2200 replaces the first and 2210 moves its own
 * literal for the second [app/cbl/COACTVWC.cbl:L640-676], so these are the
 * audited texts.
 */
export const ACCOUNT_KEY_MESSAGES = {
  /** V-COACTVWC-001: the account number is blank, spaces or `*`. */
  noInputReceived: "No input received",
  /**
   * V-COACTVWC-002: the account number is not 11 digits, or is all zeros.
   * Two spaces after "must", as declared.
   */
  accountFilterMustBeNonZero11Digit:
    "Account Filter must  be a non-zero 11 digit number",
} as const;

/**
 * The account number form of the dashboard and the account view: `ACCTSID`,
 * the `{id}` of `GET /accounts/{id}`.
 *
 * COACTVWC 2210-EDIT-ACCOUNT — V-COACTVWC-001, V-COACTVWC-002. An entry
 * longer than the 11-character field has no 3270 counterpart; the server
 * rejects it with the not-11-digits text, as here.
 */
export const accountKeySchema = z.object({
  account_id: z.string().superRefine((value, ctx) => {
    if (value.length <= ACCOUNT_ID_LENGTH && NOT_ENTERED.test(value)) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: ACCOUNT_KEY_MESSAGES.noInputReceived,
      });
    } else if (
      value.length !== ACCOUNT_ID_LENGTH ||
      !DIGITS.test(value) ||
      ALL_ZEROS.test(value)
    ) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: ACCOUNT_KEY_MESSAGES.accountFilterMustBeNonZero11Digit,
      });
    }
  }),
}) satisfies z.ZodType<{
  account_id: paths["/accounts/{id}"]["get"]["parameters"]["path"]["id"];
}>;

/** The values of the account number form. */
export type AccountKeyValues = z.infer<typeof accountKeySchema>;

/**
 * The `accountKeySchema` text of an account number, or `null` when the number
 * passes: the one text 2210-EDIT-ACCOUNT moves, as `GET /accounts/{id}`
 * answers it at ACCTSID. The account pages check the key of their route with
 * it, because COACTVWC and COACTUPC read the records only once the key edits
 * pass [app/cbl/COACTVWC.cbl:L361-374] [app/cbl/COACTUPC.cbl:L2568-2576]: a
 * key with a text here is shown it and never sent.
 */
export function accountKeyError(accountId: string): string | null {
  const check = accountKeySchema.safeParse({ account_id: accountId });
  if (check.success) {
    return null;
  }
  // The edit sets one text per key, so a rejection carries exactly one issue.
  const [issue] = check.error.issues;
  return issue.message;
}

/** The form field of each BMS field a `GET /accounts/{id}` error names. */
export const accountViewBmsFieldMap: Record<
  components["schemas"]["AccountViewField"],
  keyof AccountKeyValues
> = {
  ACCTSID: "account_id",
};

// --- COACTUP: the account update form (CAUP)

// The `COACTUPC` 1200-EDIT-MAP-INPUTS texts the form mirrors, by body
// property: each the trimmed `WS-EDIT-VARIABLE-NAME` label and the edit's
// literal, `STRING`ed into `WS-RETURN-MSG X(75)` [app/cbl/COACTUPC.cbl:L1472-1662]
// [app/cpy/CSUTLDPY.cpy:L18-165].
const ACCOUNT_UPDATE_MESSAGES = {
  acct_active_status: {
    // V-COACTUPC-011
    supplied: "Account Status must be supplied.",
    // V-COACTUPC-012
    notYesNo: "Account Status must be Y or N.",
  },
  acct_open_year: {
    // V-COACTUPC-013
    supplied: "Open Date : Year must be supplied.",
    // V-COACTUPC-014
    notNumeric: "Open Date must be 4 digit number.",
  },
  acct_open_mon: {
    // V-COACTUPC-016
    supplied: "Open Date : Month must be supplied.",
  },
  acct_open_day: {
    // V-COACTUPC-018
    supplied: "Open Date : Day must be supplied.",
  },
  acct_credit_limit: {
    // V-COACTUPC-024
    supplied: "Credit Limit must be supplied.",
    // V-COACTUPC-025
    notValid: "Credit Limit is not valid",
  },
  acct_exp_year: {
    // V-COACTUPC-026
    supplied: "Expiry Date : Year must be supplied.",
    // V-COACTUPC-027
    notNumeric: "Expiry Date must be 4 digit number.",
  },
  acct_exp_mon: {
    // V-COACTUPC-029
    supplied: "Expiry Date : Month must be supplied.",
  },
  acct_exp_day: {
    // V-COACTUPC-031
    supplied: "Expiry Date : Day must be supplied.",
  },
  acct_cash_credit_limit: {
    // V-COACTUPC-037
    supplied: "Cash Credit Limit must be supplied.",
    // V-COACTUPC-038
    notValid: "Cash Credit Limit is not valid",
  },
  acct_reissue_year: {
    // V-COACTUPC-039
    supplied: "Reissue Date : Year must be supplied.",
    // V-COACTUPC-040
    notNumeric: "Reissue Date must be 4 digit number.",
  },
  acct_reissue_mon: {
    // V-COACTUPC-042
    supplied: "Reissue Date : Month must be supplied.",
  },
  acct_reissue_day: {
    // V-COACTUPC-044
    supplied: "Reissue Date : Day must be supplied.",
  },
  acct_curr_bal: {
    // V-COACTUPC-050
    supplied: "Current Balance must be supplied.",
    // V-COACTUPC-051
    notValid: "Current Balance is not valid",
  },
  // The 26-byte label 'Current Cycle Credit Limit' is cut to the 25 bytes of
  // WS-EDIT-VARIABLE-NAME (E-45).
  acct_curr_cyc_credit: {
    // V-COACTUPC-052
    supplied: "Current Cycle Credit Limi must be supplied.",
    // V-COACTUPC-053
    notValid: "Current Cycle Credit Limi is not valid",
  },
  acct_curr_cyc_debit: {
    // V-COACTUPC-054
    supplied: "Current Cycle Debit Limit must be supplied.",
    // V-COACTUPC-055
    notValid: "Current Cycle Debit Limit is not valid",
  },
  cust_ssn_1: {
    // V-COACTUPC-056
    supplied: "SSN: First 3 chars must be supplied.",
    // V-COACTUPC-057
    notNumeric: "SSN: First 3 chars must be all numeric.",
    // V-COACTUPC-058
    zero: "SSN: First 3 chars must not be zero.",
  },
  cust_ssn_2: {
    // V-COACTUPC-060
    supplied: "SSN 4th & 5th chars must be supplied.",
    // V-COACTUPC-061
    notNumeric: "SSN 4th & 5th chars must be all numeric.",
    // V-COACTUPC-062
    zero: "SSN 4th & 5th chars must not be zero.",
  },
  cust_ssn_3: {
    // V-COACTUPC-063
    supplied: "SSN Last 4 chars must be supplied.",
    // V-COACTUPC-064
    notNumeric: "SSN Last 4 chars must be all numeric.",
    // V-COACTUPC-065
    zero: "SSN Last 4 chars must not be zero.",
  },
  cust_dob_year: {
    // V-COACTUPC-066
    supplied: "Date of Birth : Year must be supplied.",
    // V-COACTUPC-067
    notNumeric: "Date of Birth must be 4 digit number.",
  },
  cust_dob_mon: {
    // V-COACTUPC-069
    supplied: "Date of Birth : Month must be supplied.",
  },
  cust_dob_day: {
    // V-COACTUPC-071
    supplied: "Date of Birth : Day must be supplied.",
  },
  cust_fico_credit_score: {
    // V-COACTUPC-078
    supplied: "FICO Score must be supplied.",
    // V-COACTUPC-079
    notNumeric: "FICO Score must be all numeric.",
    // V-COACTUPC-080
    zero: "FICO Score must not be zero.",
  },
  cust_first_name: {
    // V-COACTUPC-082
    supplied: "First Name must be supplied.",
    // V-COACTUPC-083
    notAlphabetic: "First Name can have alphabets only.",
  },
  cust_middle_name: {
    // V-COACTUPC-084
    notAlphabetic: "Middle Name can have alphabets only.",
  },
  cust_last_name: {
    // V-COACTUPC-085
    supplied: "Last Name must be supplied.",
    // V-COACTUPC-086
    notAlphabetic: "Last Name can have alphabets only.",
  },
  cust_addr_line_1: {
    // V-COACTUPC-087
    supplied: "Address Line 1 must be supplied.",
  },
  cust_addr_state_cd: {
    // V-COACTUPC-088
    supplied: "State must be supplied.",
    // V-COACTUPC-089
    notAlphabetic: "State can have alphabets only.",
  },
  cust_addr_zip: {
    // V-COACTUPC-091
    supplied: "Zip must be supplied.",
    // V-COACTUPC-092
    notNumeric: "Zip must be all numeric.",
    // V-COACTUPC-093
    zero: "Zip must not be zero.",
  },
  cust_addr_line_3: {
    // V-COACTUPC-094
    supplied: "City must be supplied.",
    // V-COACTUPC-095
    notAlphabetic: "City can have alphabets only.",
  },
  cust_addr_country_cd: {
    // V-COACTUPC-096
    supplied: "Country must be supplied.",
    // V-COACTUPC-097
    notAlphabetic: "Country can have alphabets only.",
  },
  cust_phone_num_1a: {
    // V-COACTUPC-098
    supplied: "Phone Number 1: Area code must be supplied.",
    // V-COACTUPC-099
    notNumeric: "Phone Number 1: Area code must be A 3 digit number.",
    // V-COACTUPC-100
    zero: "Phone Number 1: Area code cannot be zero",
  },
  cust_phone_num_1b: {
    // V-COACTUPC-102
    supplied: "Phone Number 1: Prefix code must be supplied.",
    // V-COACTUPC-103
    notNumeric: "Phone Number 1: Prefix code must be A 3 digit number.",
    // V-COACTUPC-104
    zero: "Phone Number 1: Prefix code cannot be zero",
  },
  cust_phone_num_1c: {
    // V-COACTUPC-105
    supplied: "Phone Number 1: Line number code must be supplied.",
    // V-COACTUPC-106
    notNumeric: "Phone Number 1: Line number code must be A 4 digit number.",
    // V-COACTUPC-107
    zero: "Phone Number 1: Line number code cannot be zero",
  },
  cust_phone_num_2a: {
    // V-COACTUPC-108
    supplied: "Phone Number 2: Area code must be supplied.",
    // V-COACTUPC-109
    notNumeric: "Phone Number 2: Area code must be A 3 digit number.",
    // V-COACTUPC-110
    zero: "Phone Number 2: Area code cannot be zero",
  },
  cust_phone_num_2b: {
    // V-COACTUPC-112
    supplied: "Phone Number 2: Prefix code must be supplied.",
    // V-COACTUPC-113
    notNumeric: "Phone Number 2: Prefix code must be A 3 digit number.",
    // V-COACTUPC-114
    zero: "Phone Number 2: Prefix code cannot be zero",
  },
  cust_phone_num_2c: {
    // V-COACTUPC-115
    supplied: "Phone Number 2: Line number code must be supplied.",
    // V-COACTUPC-116
    notNumeric: "Phone Number 2: Line number code must be A 4 digit number.",
    // V-COACTUPC-117
    zero: "Phone Number 2: Line number code cannot be zero",
  },
  cust_eft_account_id: {
    // V-COACTUPC-118
    supplied: "EFT Account Id must be supplied.",
    // V-COACTUPC-119
    notNumeric: "EFT Account Id must be all numeric.",
    // V-COACTUPC-120
    zero: "EFT Account Id must not be zero.",
  },
  cust_pri_card_holder_ind: {
    // V-COACTUPC-121
    supplied: "Primary Card Holder must be supplied.",
    // V-COACTUPC-122
    notYesNo: "Primary Card Holder must be Y or N.",
  },
} as const satisfies Partial<
  Record<keyof AccountUpdateRequest, Readonly<Record<string, string>>>
>;

// The `COACTUP` form values in the order the validation audit ranks them: the
// `version` token (Order 1), the account number (Orders 2 and 3), then the
// input fields in map order, which is the order 1200-EDIT-MAP-INPUTS edits
// them in. Each bound is the BMS field length [app/cpy-bms/COACTUP.CPY], the
// OpenAPI `maxLength`.
//
// Keyed by the DTO's own properties plus `account_id`, so a property the DTO
// adds or drops fails `tsc` here: `satisfies z.ZodType<AccountUpdateRequest>`
// alone would accept a form value the DTO no longer has.
const accountUpdateShape = {
  // V-COACTUPC-001: the token GET /accounts/{id} returned, echoed unchanged.
  // OpenAPI declares no pattern for it, and the server answers a missing or
  // malformed token itself with 400 and no message (E-24).
  version: z.string(),
  // ACCTSID, the {id} path segment: a read-only copy of the route key, kept
  // so that every AccountUpdateField maps to a form value, and stripped from
  // the body by toAccountUpdateRequest. It is not edited here: the GET that
  // filled the form already accepted it, and the server edits it again
  // (V-COACTUPC-002, V-COACTUPC-003).
  account_id: z.string(),
  acct_active_status: z.string().max(1),
  acct_open_year: z.string().max(4),
  acct_open_mon: z.string().max(2),
  acct_open_day: z.string().max(2),
  acct_credit_limit: z.string().max(15),
  acct_exp_year: z.string().max(4),
  acct_exp_mon: z.string().max(2),
  acct_exp_day: z.string().max(2),
  acct_cash_credit_limit: z.string().max(15),
  acct_reissue_year: z.string().max(4),
  acct_reissue_mon: z.string().max(2),
  acct_reissue_day: z.string().max(2),
  acct_curr_bal: z.string().max(15),
  acct_curr_cyc_credit: z.string().max(15),
  // AADDGRP: no edit.
  acct_group_id: z.string().max(10),
  acct_curr_cyc_debit: z.string().max(15),
  // ACSTNUM: no edit.
  customer_id: z.string().max(9),
  cust_ssn_1: z.string().max(3),
  cust_ssn_2: z.string().max(2),
  cust_ssn_3: z.string().max(4),
  cust_dob_year: z.string().max(4),
  cust_dob_mon: z.string().max(2),
  cust_dob_day: z.string().max(2),
  cust_fico_credit_score: z.string().max(3),
  cust_first_name: z.string().max(25),
  cust_middle_name: z.string().max(25),
  cust_last_name: z.string().max(25),
  cust_addr_line_1: z.string().max(50),
  cust_addr_state_cd: z.string().max(2),
  // ACSADL2: no edit.
  cust_addr_line_2: z.string().max(50),
  cust_addr_zip: z.string().max(5),
  cust_addr_line_3: z.string().max(50),
  cust_addr_country_cd: z.string().max(3),
  cust_phone_num_1a: z.string().max(3),
  cust_phone_num_1b: z.string().max(3),
  cust_phone_num_1c: z.string().max(4),
  // ACSGOVT: no edit.
  cust_govt_issued_id: z.string().max(20),
  cust_phone_num_2a: z.string().max(3),
  cust_phone_num_2b: z.string().max(3),
  cust_phone_num_2c: z.string().max(4),
  cust_eft_account_id: z.string().max(10),
  cust_pri_card_holder_ind: z.string().max(1),
} satisfies Record<keyof AccountUpdateRequest | "account_id", z.ZodTypeAny>;

const accountUpdateFields = z.object(accountUpdateShape);

// The form values before the edits.
type AccountUpdateFieldValues = z.infer<typeof accountUpdateFields>;

// The BMS length of each bounded form value, from its `.max()`.
const BMS_LENGTHS: ReadonlyMap<string, number> = new Map(
  Object.entries(accountUpdateFields.shape).flatMap(([name, field]) =>
    field.maxLength === null ? [] : [[name, field.maxLength] as const],
  ),
);

// A form value 1200-EDIT-MAP-INPUTS edits.
type EditedField = keyof typeof ACCOUNT_UPDATE_MESSAGES;

// The edited fields whose texts include those of `T`.
type FieldsWith<T> = {
  [K in EditedField]: (typeof ACCOUNT_UPDATE_MESSAGES)[K] extends T
    ? K
    : never;
}[EditedField];

// The texts of an edit that rejects a value not entered.
type SuppliedTexts = { readonly supplied: string };

// The texts of 1220-EDIT-YESNO.
type YesNoTexts = SuppliedTexts & { readonly notYesNo: string };

// The texts of CSUTLDPY EDIT-YEAR-CCYY that the form mirrors.
type YearTexts = SuppliedTexts & { readonly notNumeric: string };

// The texts of 1250-EDIT-SIGNED-9V2.
type AmountTexts = SuppliedTexts & { readonly notValid: string };

// The texts of 1245-EDIT-NUM-REQD and of the three phone-part edits.
type DigitTexts = YearTexts & { readonly zero: string };

// The texts of the alphabetic test of 1225 and 1235.
type AlphaTexts = { readonly notAlphabetic: string };

// Flags `field` with `message`, as `MOVE -1 TO <field>L` marks it.
function reject(
  ctx: z.RefinementCtx,
  field: EditedField,
  message: string,
): void {
  ctx.addIssue({ code: z.ZodIssueCode.custom, path: [field], message });
}

// `IS NUMERIC` of the field as received: the entry is padded with spaces to
// the BMS length, so a shorter entry is not numeric.
function fillsWithDigits(value: string, field: EditedField): boolean {
  return value.length === BMS_LENGTHS.get(field) && DIGITS.test(value);
}

/**
 * COACTUPC 1220-EDIT-YESNO — P-COACTUPC-021
 *
 * Not entered, a space or `0` is not supplied, and anything but an upper-case
 * `Y` or `N` (88 FLG-YES-NO-ISVALID) is rejected.
 */
function editYesNo(
  values: AccountUpdateFieldValues,
  ctx: z.RefinementCtx,
  field: FieldsWith<YesNoTexts>,
): void {
  const value = values[field];
  const texts = ACCOUNT_UPDATE_MESSAGES[field];
  if (NOT_ENTERED.test(value) || value === "0") {
    reject(ctx, field, texts.supplied);
  } else if (value !== "Y" && value !== "N") {
    reject(ctx, field, texts.notYesNo);
  }
}

/**
 * COACTUPC 1215-EDIT-MANDATORY — P-COACTUPC-020
 * CSUTLDPY EDIT-MONTH — P-CSUTLDPY-005
 * CSUTLDPY EDIT-DAY — P-CSUTLDPY-007
 *
 * An edit whose only mirrored rejection is a value not entered: address line 1
 * (1215-EDIT-MANDATORY), and the month and the day of a date (EDIT-MONTH,
 * EDIT-DAY).
 */
function editSupplied(
  values: AccountUpdateFieldValues,
  ctx: z.RefinementCtx,
  field: FieldsWith<SuppliedTexts>,
): void {
  if (NOT_ENTERED.test(values[field])) {
    reject(ctx, field, ACCOUNT_UPDATE_MESSAGES[field].supplied);
  }
}

/**
 * CSUTLDPY EDIT-DATE-CCYYMMDD — P-CSUTLDPY-001
 * CSUTLDPY EDIT-YEAR-CCYY — P-CSUTLDPY-002, P-CSUTLDPY-003
 * CSUTLDPY EDIT-MONTH — P-CSUTLDPY-005
 * CSUTLDPY EDIT-DAY — P-CSUTLDPY-007
 *
 * EDIT-DATE-CCYYMMDD as far as the form mirrors it: the year not entered or
 * not four digits (EDIT-YEAR-CCYY), the month and the day not entered
 * (EDIT-MONTH, EDIT-DAY). Each `GO TO <paragraph>-EXIT` leaves only its own
 * paragraph, so every part is edited whatever the others hold. The century,
 * the month and day values and the calendar are the server's.
 */
function editDate(
  values: AccountUpdateFieldValues,
  ctx: z.RefinementCtx,
  year: FieldsWith<YearTexts>,
  month: FieldsWith<SuppliedTexts>,
  day: FieldsWith<SuppliedTexts>,
): void {
  const value = values[year];
  const texts = ACCOUNT_UPDATE_MESSAGES[year];
  if (NOT_ENTERED.test(value)) {
    reject(ctx, year, texts.supplied);
  } else if (!fillsWithDigits(value, year)) {
    reject(ctx, year, texts.notNumeric);
  }
  editSupplied(values, ctx, month);
  editSupplied(values, ctx, day);
}

/**
 * COACTUPC 1250-EDIT-SIGNED-9V2 — P-COACTUPC-026
 *
 * Not entered is not supplied, and text `TEST-NUMVAL-C` rejects is not valid.
 */
function editAmount(
  values: AccountUpdateFieldValues,
  ctx: z.RefinementCtx,
  field: FieldsWith<AmountTexts>,
): void {
  const value = values[field];
  const texts = ACCOUNT_UPDATE_MESSAGES[field];
  if (NOT_ENTERED.test(value)) {
    reject(ctx, field, texts.supplied);
  } else if (!NUMVAL_C_ARGUMENT.test(value)) {
    reject(ctx, field, texts.notValid);
  }
}

/**
 * COACTUPC 1245-EDIT-NUM-REQD — P-COACTUPC-025
 * COACTUPC EDIT-AREA-CODE — P-COACTUPC-028
 * COACTUPC EDIT-US-PHONE-PREFIX — P-COACTUPC-029
 * COACTUPC EDIT-US-PHONE-LINENUM — P-COACTUPC-030
 *
 * 1245-EDIT-NUM-REQD, EDIT-AREA-CODE up to its `CSLKPCDY` lookup, and
 * EDIT-US-PHONE-PREFIX and EDIT-US-PHONE-LINENUM, which have none: not
 * entered, not all digits, then all zeros. Whether the edit passed
 * (FLG-ALPHNANUM-ISVALID).
 */
function editDigits(
  values: AccountUpdateFieldValues,
  ctx: z.RefinementCtx,
  field: FieldsWith<DigitTexts>,
): boolean {
  const value = values[field];
  const texts = ACCOUNT_UPDATE_MESSAGES[field];
  if (NOT_ENTERED.test(value)) {
    reject(ctx, field, texts.supplied);
  } else if (!fillsWithDigits(value, field)) {
    reject(ctx, field, texts.notNumeric);
  } else if (ALL_ZEROS.test(value)) {
    reject(ctx, field, texts.zero);
  } else {
    return true;
  }
  return false;
}

/**
 * COACTUPC 1225-EDIT-ALPHA-REQD — P-COACTUPC-022
 *
 * Not entered is not supplied, and a character other than a letter or a space
 * is rejected.
 */
function editAlphaRequired(
  values: AccountUpdateFieldValues,
  ctx: z.RefinementCtx,
  field: FieldsWith<SuppliedTexts & AlphaTexts>,
): void {
  const value = values[field];
  const texts = ACCOUNT_UPDATE_MESSAGES[field];
  if (NOT_ENTERED.test(value)) {
    reject(ctx, field, texts.supplied);
  } else if (!ALPHABETIC.test(value)) {
    reject(ctx, field, texts.notAlphabetic);
  }
}

/**
 * COACTUPC 1235-EDIT-ALPHA-OPT — P-COACTUPC-024
 *
 * A value not entered passes; otherwise the alphabetic test of 1225.
 */
function editAlphaOptional(
  values: AccountUpdateFieldValues,
  ctx: z.RefinementCtx,
  field: FieldsWith<AlphaTexts>,
): void {
  const value = values[field];
  if (!NOT_ENTERED.test(value) && !ALPHABETIC.test(value)) {
    reject(ctx, field, ACCOUNT_UPDATE_MESSAGES[field].notAlphabetic);
  }
}

/**
 * COACTUPC 1265-EDIT-US-SSN — P-COACTUPC-031
 *
 * Part 1 edited as three required digits. The IF that tests part 1 closes only
 * at the paragraph's period [app/cbl/COACTUPC.cbl:L2448-2488], so parts 2 and
 * 3 are edited only when part 1 passed that edit (E-45).
 *
 * The 666 and 900-999 test of part 1 (V-COACTUPC-059) stays with the server,
 * as the FICO range does: 4 and 21 of the 50 fixture customers fail them, and
 * the server answers an unchanged submit of their values with 200 'No change
 * detected with respect to values fetched.', which a client rejection would
 * replace.
 */
function editSsn(values: AccountUpdateFieldValues, ctx: z.RefinementCtx): void {
  if (editDigits(values, ctx, "cust_ssn_1")) {
    editDigits(values, ctx, "cust_ssn_2");
    editDigits(values, ctx, "cust_ssn_3");
  }
}

/**
 * COACTUPC 1260-EDIT-US-PHONE-NUM — P-COACTUPC-027
 *
 * A phone number with no part entered is valid and absent; otherwise the area
 * code, prefix and line number are each edited. The area-code lookup in
 * `CSLKPCDY` is the server's.
 */
function editPhone(
  values: AccountUpdateFieldValues,
  ctx: z.RefinementCtx,
  parts: readonly [
    FieldsWith<DigitTexts>,
    FieldsWith<DigitTexts>,
    FieldsWith<DigitTexts>,
  ],
): void {
  if (parts.every((part) => NOT_ENTERED.test(values[part]))) {
    return;
  }
  for (const part of parts) {
    editDigits(values, ctx, part);
  }
}

/**
 * COACTUPC 1200-EDIT-MAP-INPUTS — P-COACTUPC-006, V-COACTUPC-011 to V-COACTUPC-122
 *
 * The mirrored edits in the paragraph's order [app/cbl/COACTUPC.cbl:L1472-1669],
 * each failing edit flagging its own field, so the issues follow the audit
 * Order. None runs while a field exceeds its BMS length: the server rejects
 * such a body with 400 and no text before any COBOL edit
 * (`AccountUpdateRequest::check`), and only the length issues stand.
 */
function editMapInputs(
  values: AccountUpdateFieldValues,
  ctx: z.RefinementCtx,
): void {
  const overLength = Object.entries(values).some(
    ([name, value]) => value.length > (BMS_LENGTHS.get(name) ?? value.length),
  );
  if (overLength) {
    return;
  }
  editYesNo(values, ctx, "acct_active_status");
  editDate(values, ctx, "acct_open_year", "acct_open_mon", "acct_open_day");
  editAmount(values, ctx, "acct_credit_limit");
  editDate(values, ctx, "acct_exp_year", "acct_exp_mon", "acct_exp_day");
  editAmount(values, ctx, "acct_cash_credit_limit");
  editDate(
    values,
    ctx,
    "acct_reissue_year",
    "acct_reissue_mon",
    "acct_reissue_day",
  );
  editAmount(values, ctx, "acct_curr_bal");
  editAmount(values, ctx, "acct_curr_cyc_credit");
  editAmount(values, ctx, "acct_curr_cyc_debit");
  editSsn(values, ctx);
  editDate(values, ctx, "cust_dob_year", "cust_dob_mon", "cust_dob_day");
  // The 300-850 range (V-COACTUPC-081) is the server's; see editSsn.
  editDigits(values, ctx, "cust_fico_credit_score");
  editAlphaRequired(values, ctx, "cust_first_name");
  editAlphaOptional(values, ctx, "cust_middle_name");
  editAlphaRequired(values, ctx, "cust_last_name");
  editSupplied(values, ctx, "cust_addr_line_1");
  editAlphaRequired(values, ctx, "cust_addr_state_cd");
  editDigits(values, ctx, "cust_addr_zip");
  editAlphaRequired(values, ctx, "cust_addr_line_3");
  editAlphaRequired(values, ctx, "cust_addr_country_cd");
  editPhone(values, ctx, [
    "cust_phone_num_1a",
    "cust_phone_num_1b",
    "cust_phone_num_1c",
  ]);
  editPhone(values, ctx, [
    "cust_phone_num_2a",
    "cust_phone_num_2b",
    "cust_phone_num_2c",
  ]);
  editDigits(values, ctx, "cust_eft_account_id");
  editYesNo(values, ctx, "cust_pri_card_holder_ind");
}

/**
 * The account update form: the `PUT /accounts/{id}` body plus the read-only
 * `account_id`, with the BMS lengths and the mirrored 1200-EDIT-MAP-INPUTS
 * edits.
 */
export const accountUpdateSchema = accountUpdateFields.superRefine(
  editMapInputs,
) satisfies z.ZodType<AccountUpdateRequest>;

/** The values of the account update form. */
export type AccountUpdateFormValues = z.infer<typeof accountUpdateSchema>;

/**
 * The form field of each BMS field a `PUT /accounts/{id}` error names.
 * `ACCTSID` is the read-only account number.
 */
export const accountUpdateBmsFieldMap: Record<
  components["schemas"]["AccountUpdateField"],
  keyof AccountUpdateFormValues
> = {
  ACCTSID: "account_id",
  ACSTTUS: "acct_active_status",
  OPNYEAR: "acct_open_year",
  OPNMON: "acct_open_mon",
  OPNDAY: "acct_open_day",
  ACRDLIM: "acct_credit_limit",
  EXPYEAR: "acct_exp_year",
  EXPMON: "acct_exp_mon",
  EXPDAY: "acct_exp_day",
  ACSHLIM: "acct_cash_credit_limit",
  RISYEAR: "acct_reissue_year",
  RISMON: "acct_reissue_mon",
  RISDAY: "acct_reissue_day",
  ACURBAL: "acct_curr_bal",
  ACRCYCR: "acct_curr_cyc_credit",
  AADDGRP: "acct_group_id",
  ACRCYDB: "acct_curr_cyc_debit",
  ACSTNUM: "customer_id",
  ACTSSN1: "cust_ssn_1",
  ACTSSN2: "cust_ssn_2",
  ACTSSN3: "cust_ssn_3",
  DOBYEAR: "cust_dob_year",
  DOBMON: "cust_dob_mon",
  DOBDAY: "cust_dob_day",
  ACSTFCO: "cust_fico_credit_score",
  ACSFNAM: "cust_first_name",
  ACSMNAM: "cust_middle_name",
  ACSLNAM: "cust_last_name",
  ACSADL1: "cust_addr_line_1",
  ACSSTTE: "cust_addr_state_cd",
  ACSADL2: "cust_addr_line_2",
  ACSZIPC: "cust_addr_zip",
  ACSCITY: "cust_addr_line_3",
  ACSCTRY: "cust_addr_country_cd",
  ACSPH1A: "cust_phone_num_1a",
  ACSPH1B: "cust_phone_num_1b",
  ACSPH1C: "cust_phone_num_1c",
  ACSGOVT: "cust_govt_issued_id",
  ACSPH2A: "cust_phone_num_2a",
  ACSPH2B: "cust_phone_num_2b",
  ACSPH2C: "cust_phone_num_2c",
  ACSEFTC: "cust_eft_account_id",
  ACSPFLG: "cust_pri_card_holder_ind",
};

/**
 * The `PUT /accounts/{id}` body of `values`: every value as entered, without
 * the read-only `account_id`, which travels as the `{id}` path segment.
 */
export function toAccountUpdateRequest(
  values: AccountUpdateFormValues,
): AccountUpdateRequest {
  const { account_id: _pathSegment, ...body } = values;
  return body;
}
