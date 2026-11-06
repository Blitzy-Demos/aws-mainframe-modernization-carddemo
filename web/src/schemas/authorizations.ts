/**
 * Client mirrors of the CPVS account filter (`GET /authorizations?account_id=`,
 * map COPAU00, COPAUS0C) and the CPVD key (`{id}` of `GET` and
 * `PUT /authorizations/{id}`, map COPAU01, COPAUS1C). They keep a form or a
 * route from sending what the server would reject; the server stays
 * authoritative.
 *
 * Only the input edits are mirrored. Everything that needs a read stays
 * server-only: the cross-reference, account and customer not-found texts
 * (V-COPAUS0C-004 to V-COPAUS0C-009), the IMS and Db2 failures
 * (V-COPAUS0C-010 to V-COPAUS0C-015, V-COPAUS1C-002 to V-COPAUS1C-013), the
 * row selector (V-COPAUS0C-003, no REST parameter) and every fraud-update
 * outcome. COPAU01 has no unprotected field, so its key arrives from the
 * summary row, never from typing, and `PUT /authorizations/{id}` has no body.
 */
import { z } from "zod";

import type { components, paths } from "@/api/schema";

import { isSpacesOrLowValues } from "./primitives";

/** The COPAUS0C PROCESS-ENTER-KEY texts, byte for byte as the validation audit records them. */
export const authSummaryMessages = {
  /** V-COPAUS0C-001 · COPAUS0C.cbl:L269: the account ID is blank or LOW-VALUES. */
  pleaseEnterAcctId: "Please enter Acct Id...",
  /** V-COPAUS0C-002 · COPAUS0C.cbl:L278: the account ID is not 11 digits. */
  acctIdMustBeNumeric: "Acct Id must be Numeric ...",
} as const;

/** The COPAUS1C PROCESS-ENTER-KEY text of a key the program cannot read. */
export const authDetailMessages = {
  /**
   * V-COPAUS1C-001 · COPAUS1C.cbl:L208-227: no-message (E-49). The program
   * sets its error flag and no text; the server answers 400 with an empty
   * message on CARDNUM, so the client reports the same empty text rather
   * than writing one the COBOL never shows.
   */
  invalidKey: "",
} as const;

// `ACCTIDI PIC X(11)` (COPAU00.cpy:L60), the `maxLength` of the `account_id` query parameter.
const ACCOUNT_ID_LENGTH = 11;

// `ACCTIDI IS NUMERIC` on X(11): exactly eleven ASCII digits, no sign and no spaces.
const ACCOUNT_ID_DIGITS = /^[0-9]{11}$/;

// COPAUS0C PROCESS-ENTER-KEY — V-COPAUS0C-001, V-COPAUS0C-002: the account ID
// as entered, edited in the server's order. A value longer than the screen
// field is "not 11 digits" (the DTO length check runs before the rule), then
// a blank value, then any other value that is not eleven digits. One issue at
// most, so the field shows the single message the server would send.
//
// Blank is `ACCTIDI = SPACES OR LOW-VALUES` (COPAUS0C.cbl:L264). NUL-only text
// is blank too because the server's edit (`auth_summary::edit_account_id`)
// treats LOW-VALUES as the COBOL does.
const accountIdField = z
  .string({ required_error: authSummaryMessages.pleaseEnterAcctId })
  .max(ACCOUNT_ID_LENGTH, { message: authSummaryMessages.acctIdMustBeNumeric })
  .superRefine((value, ctx) => {
    if (value.length > ACCOUNT_ID_LENGTH) {
      return;
    }
    if (isSpacesOrLowValues(value)) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: authSummaryMessages.pleaseEnterAcctId,
      });
    } else if (!ACCOUNT_ID_DIGITS.test(value)) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: authSummaryMessages.acctIdMustBeNumeric,
      });
    }
  });

/**
 * The CPVS search form (COPAU00 `ACCTID`, ENTER). The field is required here
 * although the query parameter is optional, because an absent account is
 * edited as blank: `Please enter Acct Id...`.
 */
export const authSummaryFilterSchema = z.object({
  account_id: accountIdField,
}) satisfies z.ZodType<
  Pick<NonNullable<paths["/authorizations"]["get"]["parameters"]["query"]>, "account_id">
>;

export type AuthSummaryFilterValues = z.infer<typeof authSummaryFilterSchema>;

/** Every COPAUS0C message puts the cursor on ACCTID (`MOVE -1 TO ACCTIDL OF COPAU0AI`). */
export const authSummaryBmsFieldMap: Record<
  components["schemas"]["AuthSummaryField"],
  keyof AuthSummaryFilterValues
> = {
  ACCTID: "account_id",
};

// `i64::MAX`, the largest identity the server's key parse accepts.
const AUTH_ID_MAX = "9223372036854775807";

// An optional `+`, then decimal digits: the text Rust's `i64` parse reads as a non-negative value.
const UNSIGNED_DECIMAL = /^\+?([0-9]+)$/;

// Whether `value` names a positive `i64`, the identity the server reads
// (`parse_auth_id`, then `auth_detail` rejecting a value not above zero).
// Compared as digit strings so no conversion can lose precision above 2^53.
function isPositiveAuthId(value: string): boolean {
  const match = UNSIGNED_DECIMAL.exec(value);
  if (match === null) {
    return false;
  }
  const significant = match[1].replace(/^0+/, "");
  if (significant === "") {
    return false;
  }
  return (
    significant.length < AUTH_ID_MAX.length ||
    (significant.length === AUTH_ID_MAX.length && significant <= AUTH_ID_MAX)
  );
}

/**
 * COPAUS1C PROCESS-ENTER-KEY — V-COPAUS1C-001: the selected authorization,
 * the `auth_id` a summary row carries. The edit mirrors the server's parse,
 * whose OpenAPI path `pattern` admits the same keys: a missing, blank or
 * malformed key, or one that is not a positive integer up to `i64::MAX`, is
 * the COBOL path with no selected key and no message.
 */
export const authDetailKeySchema = z.object({
  auth_id: z
    .string({
      required_error: authDetailMessages.invalidKey,
      invalid_type_error: authDetailMessages.invalidKey,
    })
    .refine(isPositiveAuthId, { message: authDetailMessages.invalidKey }),
}) satisfies z.ZodType<{
  auth_id: paths["/authorizations/{id}"]["get"]["parameters"]["path"]["id"];
}>;

export type AuthDetailKeyValues = z.infer<typeof authDetailKeySchema>;

/**
 * Every COPAUS1C answer puts the cursor on CARDNUM (`MOVE -1 TO CARDNUML`,
 * COPAUS1C.cbl:L373-378). CARDNUM is protected output on COPAU01 and the
 * detail is addressed by its key, so the field maps to `auth_id`, the control
 * a page focuses for the selected authorization.
 */
export const authDetailBmsFieldMap: Record<
  components["schemas"]["AuthDetailField"],
  keyof AuthDetailKeyValues
> = {
  CARDNUM: "auth_id",
};
