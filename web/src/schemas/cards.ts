/**
 * Card schemas: the client mirrors of the CCLI filter
 * (`GET /cards?account_id=&card_number=`, COCRDLIC, map COCRDLI), the CCDL
 * keys (`GET /cards/{id}?account_id=`, COCRDSLC, map COCRDSL) and the CCUP
 * edit form (`PUT /cards/{id}`, COCRDUPC, map COCRDUP).
 *
 * The server stays authoritative (docs/parity/validation-audit.md). These
 * schemas block input the server rejects, in the order it rejects it:
 *
 * 1. Each value holds at most the characters of its map field:
 *    `ACCTSIDI X(11)`, `CARDSIDI X(16)`, `CRDNAMEI X(50)`, `CRDSTCDI X(1)`,
 *    `EXPMONI X(2)` and `EXPYEARI X(4)` [app/cpy-bms/COCRDLI.CPY:L66, L72]
 *    [app/cpy-bms/COCRDSL.CPY:L60, L66] [app/cpy-bms/COCRDUP.CPY:L60-90].
 *    The server checks lengths before the COBOL edits and answers with an
 *    empty message, so these issues keep zod's own text, and a value too long
 *    gets no COBOL edit of its own.
 * 2. The COBOL edits, each with its audited text, byte for byte.
 *
 * They block nothing the server accepts except one case, set out at
 * `checkCardFields`: the card-field edits run even when nothing changed, so an
 * unchanged stored value they reject (a name with an apostrophe) is blocked
 * here, while the server answers it with 'No change detected with respect to
 * values fetched.'
 *
 * Left to the server: the card reads ('Did not find cards for this search
 * condition', the file-error texts), the comparison with the fetched card
 * ('No change detected with respect to values fetched.'), the lock and
 * rewrite failures, the 409 'Record changed by some one else. Please review'
 * against the version token, and the 400 of a missing token (E-24).
 *
 * Values are never trimmed or case-changed: the COBOL tests each field as the
 * map received it, padded with spaces to its width.
 */
import { z } from "zod";

import type { components, paths } from "@/api/schema";

type CardListQuery = NonNullable<paths["/cards"]["get"]["parameters"]["query"]>;
type CardViewQuery = NonNullable<
  paths["/cards/{id}"]["get"]["parameters"]["query"]
>;
type CardPath = paths["/cards/{id}"]["get"]["parameters"]["path"];
type CardUpdateRequest = components["schemas"]["CardUpdateRequest"];
type CardListField = components["schemas"]["CardListField"];
type CardViewField = components["schemas"]["CardViewField"];
type CardUpdateField = components["schemas"]["CardUpdateField"];

// `ACCTSIDI PIC X(11)`, the account number.
const ACCOUNT_ID_LENGTH = 11;
// `CARDSIDI PIC X(16)`, the card number.
const CARD_NUMBER_LENGTH = 16;
// `CRDNAMEI PIC X(50)`, the embossed name.
const EMBOSSED_NAME_LENGTH = 50;
// `CRDSTCDI PIC X(1)`, the active status.
const ACTIVE_STATUS_LENGTH = 1;
// `EXPMONI PIC X(2)`, the expiry month.
const EXPIRY_MONTH_LENGTH = 2;
// `EXPYEARI PIC X(4)`, the expiry year.
const EXPIRY_YEAR_LENGTH = 4;

// The `pattern` of `CardUpdateRequest.version`: 16 lower-case hex digits.
const VERSION_PATTERN = /^[0-9a-f]{16}$/;

// `88 VALID-MONTH VALUES 1 THRU 12` on the `9(2)` redefine of the month
// [app/cbl/COCRDUPC.cbl:L92-95]. A one-digit month is padded with a space, a
// non-digit the redefine cannot read (E-47); it gets the outcome E-47 proposes
// and the server applies, the range test failing.
const VALID_MONTH = /^(0[1-9]|1[0-2])$/;

// `88 VALID-YEAR VALUES 1950 THRU 2099` on the `9(4)` redefine of the year
// [app/cbl/COCRDUPC.cbl:L96-99], with E-47's outcome for a non-digit, as for
// the month.
const VALID_YEAR = /^(19[5-9][0-9]|20[0-9]{2})$/;

/**
 * The COCRDLIC, COCRDSLC and COCRDUPC texts of the edits these schemas
 * repeat, byte for byte. COCRDSLC and COCRDUPC move the COCRDLIC filter
 * literals for a key that is not numeric [app/cbl/COCRDSLC.cbl:L665-674,
 * L706-715] [app/cbl/COCRDUPC.cbl:L740-750, L784-794], not their unused
 * 88-level texts.
 */
export const CARD_MESSAGES = {
  // V-COCRDLIC-001, V-COCRDSLC-002, V-COCRDUPC-003
  accountNotNumeric: "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER",
  // V-COCRDLIC-002, V-COCRDSLC-004, V-COCRDUPC-005
  cardNotNumeric: "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER",
  // V-COCRDSLC-001, V-COCRDUPC-002
  accountNotProvided: "Account number not provided",
  // V-COCRDSLC-003, V-COCRDUPC-004
  cardNotProvided: "Card number not provided",
  // V-COCRDSLC-005, V-COCRDUPC-006
  noInputReceived: "No input received",
  // V-COCRDUPC-010
  nameNotProvided: "Card name not provided",
  // V-COCRDUPC-011
  nameNotAlphabetic: "Card name can only contain alphabets and spaces",
  // V-COCRDUPC-012, V-COCRDUPC-013
  statusNotYesNo: "Card Active Status must be Y or N",
  // V-COCRDUPC-014, V-COCRDUPC-015
  expiryMonthNotValid: "Card expiry month must be between 1 and 12",
  // V-COCRDUPC-016, V-COCRDUPC-017
  expiryYearNotValid: "Invalid card expiry year",
} as const;

// `= SPACES OR = LOW-VALUES` of an `X(width)` field holding `value`. The MOVE
// pads the entry with spaces, so any run of spaces, the empty text included,
// is SPACES, and only `width` NULs are LOW-VALUES. A tab or any other
// whitespace is not blank to the COBOL, so `String.prototype.trim` is not
// used.
function isSpacesOrLowValues(value: string, width: number): boolean {
  return (
    /^ *$/.test(value) || (value.length === width && /^\u0000+$/.test(value))
  );
}

// `= ZEROS` of an `X(width)` field holding `value`: `width` zeros, since the
// padding spaces are not zeros. The keys' zero test reads their `9(n)`
// redefine, which agrees for a value of digits (E-47 for any other).
function isZeros(value: string, width: number): boolean {
  return value.length === width && /^0+$/.test(value);
}

// `IS NUMERIC` of an `X(width)` field holding `value`: `width` digits.
function isNumeric(value: string, width: number): boolean {
  return value.length === width && /^[0-9]+$/.test(value);
}

/**
 * A map field COCRDSLC 2200-EDIT-MAP-INPUTS — P-COCRDSLC-015 and
 * COCRDUPC 1100-RECEIVE-MAP — P-COCRDUPC-010 replace with LOW-VALUES: `'*'`
 * or SPACES [app/cbl/COCRDSLC.cbl:L615-627] [app/cbl/COCRDUPC.cbl:L588-636].
 * The comparison with `'*'` pads it with spaces, so a `*` followed by spaces
 * matches too.
 */
function isNotEntered(value: string, width: number): boolean {
  return /^\* *$/.test(value) || isSpacesOrLowValues(value, width);
}

// One issue at `field`, with an audited text.
function addIssue(
  ctx: z.RefinementCtx,
  field: string,
  message: string,
): void {
  ctx.addIssue({ code: z.ZodIssueCode.custom, path: [field], message });
}

/**
 * One CCLI filter, COCRDLIC 2210-EDIT-ACCOUNT — P-COCRDLIC-005 or
 * COCRDLIC 2220-EDIT-CARD — P-COCRDLIC-006 [app/cbl/COCRDLIC.cbl:L1003-1069]:
 * SPACES, LOW-VALUES or zeros is no filter, `width` digits filter, and
 * anything else is rejected with `message`. COCRDLIC has no `'*'` rule, so a
 * `*` is rejected.
 *
 * The output of a SPACES or LOW-VALUES filter is `undefined`, which the
 * server reads exactly as it reads that text; zeros stay as entered, which it
 * also reads as no filter. The output is usable as query parameters as it
 * stands.
 */
function listFilter(width: number, message: string) {
  return z
    .string()
    .max(width)
    .refine(
      (value) =>
        value.length > width ||
        isSpacesOrLowValues(value, width) ||
        isNumeric(value, width),
      { message },
    )
    .transform((value) =>
      isSpacesOrLowValues(value, width) ? undefined : value,
    );
}

/**
 * The CCLI filter form, keys in audit Order: the account filter
 * (V-COCRDLIC-001, Order 1), then the card filter (V-COCRDLIC-002, Order 2).
 * Both edits always run, so each failing filter carries its own text, as
 * 1300-SETUP-SCREEN-ATTRS marks each one.
 */
export const cardListFilterSchema = z.object({
  account_id: listFilter(ACCOUNT_ID_LENGTH, CARD_MESSAGES.accountNotNumeric),
  card_number: listFilter(CARD_NUMBER_LENGTH, CARD_MESSAGES.cardNotNumeric),
}) satisfies z.ZodType<Pick<CardListQuery, "account_id" | "card_number">>;

/** The CCLI filter form's values, as entered. */
export type CardListFilterValues = z.input<typeof cardListFilterSchema>;

/** The `GET /cards` filters of valid form values: a blank filter absent. */
export type CardListFilterQuery = z.output<typeof cardListFilterSchema>;

/** The form field of each BMS field a `GET /cards` error names. */
export const cardListBmsFieldMap: Record<
  CardListField,
  keyof CardListFilterValues
> = {
  ACCTSID: "account_id",
  CARDSID: "card_number",
};

// The outcome of one key edit.
type KeyEdit = "valid" | "blank" | "notNumeric" | "tooLong";

/**
 * COCRDSLC 2210-EDIT-ACCOUNT — P-COCRDSLC-002,
 * COCRDSLC 2220-EDIT-CARD — P-COCRDSLC-003 and
 * COCRDUPC 1210-EDIT-ACCOUNT — P-COCRDUPC-011,
 * COCRDUPC 1220-EDIT-CARD — P-COCRDUPC-012 on a key of `width` characters:
 * `'*'`, SPACES, LOW-VALUES or zeros is not provided, `width` digits pass,
 * anything else is not numeric. A value too long carries its length issue
 * only.
 */
function editKey(value: string, width: number): KeyEdit {
  if (value.length > width) {
    return "tooLong";
  }
  if (isNotEntered(value, width) || isZeros(value, width)) {
    return "blank";
  }
  return isNumeric(value, width) ? "valid" : "notNumeric";
}

// The two keys COCRDSLC and COCRDUPC edit.
interface CardKeyValues {
  account_id: string;
  card_number: string;
}

/**
 * COCRDSLC 2200-EDIT-MAP-INPUTS — P-COCRDSLC-015 and
 * COCRDUPC 1200-EDIT-MAP-INPUTS — P-COCRDUPC-002 key edits
 * [app/cbl/COCRDSLC.cbl:L630-639] [app/cbl/COCRDUPC.cbl:L645-663]: the
 * account, then the card number, each failing key with its own text. When
 * both are not provided, `SET NO-SEARCH-CRITERIA-RECEIVED` replaces the
 * account's text with 'No input received' (V-COCRDSLC-005, V-COCRDUPC-006);
 * the card keeps its own, as COCRDSLC 1300-SETUP-SCREEN-ATTRS and COCRDUPC
 * 3300-SETUP-SCREEN-ATTRS mark each failing key.
 *
 * Returns whether both keys passed.
 */
function checkKeys(keys: CardKeyValues, ctx: z.RefinementCtx): boolean {
  const account = editKey(keys.account_id, ACCOUNT_ID_LENGTH);
  const card = editKey(keys.card_number, CARD_NUMBER_LENGTH);
  if (account === "blank") {
    addIssue(
      ctx,
      "account_id",
      card === "blank"
        ? CARD_MESSAGES.noInputReceived
        : CARD_MESSAGES.accountNotProvided,
    );
  } else if (account === "notNumeric") {
    addIssue(ctx, "account_id", CARD_MESSAGES.accountNotNumeric);
  }
  if (card === "blank") {
    addIssue(ctx, "card_number", CARD_MESSAGES.cardNotProvided);
  } else if (card === "notNumeric") {
    addIssue(ctx, "card_number", CARD_MESSAGES.cardNotNumeric);
  }
  return account === "valid" && card === "valid";
}

/**
 * The CCDL keys: the `account_id` query parameter and the card number of the
 * `{id}` path segment, both required (E-55), keys in audit Order (ACCTSID
 * from Order 1, CARDSID from Order 3). The key edits are one object-level
 * check because 'No input received' depends on both keys.
 */
export const cardViewKeySchema = z
  .object({
    account_id: z.string().max(ACCOUNT_ID_LENGTH),
    card_number: z.string().max(CARD_NUMBER_LENGTH),
  })
  .superRefine((keys, ctx) => {
    checkKeys(keys, ctx);
  }) satisfies z.ZodType<
  Pick<CardViewQuery, "account_id"> & { card_number: CardPath["id"] }
>;

/** The CCDL keys, as entered. */
export type CardViewKeyValues = z.infer<typeof cardViewKeySchema>;

/** The form field of each BMS field a `GET /cards/{id}` error names. */
export const cardViewBmsFieldMap: Record<
  CardViewField,
  keyof CardViewKeyValues
> = {
  ACCTSID: "account_id",
  CARDSID: "card_number",
};

// The card fields COCRDUPC edits once both keys pass.
interface CardFieldValues {
  embossed_name: string;
  active_status: string;
  expiry_month: string;
  expiry_year: string;
}

/**
 * COCRDUPC 1230-EDIT-NAME — P-COCRDUPC-013,
 * COCRDUPC 1240-EDIT-CARDSTATUS — P-COCRDUPC-014,
 * COCRDUPC 1250-EDIT-EXPIRY-MON — P-COCRDUPC-015 and
 * COCRDUPC 1260-EDIT-EXPIRY-YEAR — P-COCRDUPC-016
 * [app/cbl/COCRDUPC.cbl:L806-945], in that order, every edit running. A value
 * too long carries its length issue only.
 *
 * The COBOL skips these edits when the entry equals the fetched card
 * [app/cbl/COCRDUPC.cbl:L680-693], which this schema cannot see: an
 * unchanged stored value these edits reject (a name with an apostrophe) is
 * blocked here, while the server answers it with 'No change detected with
 * respect to values fetched.' and writes nothing.
 */
function checkCardFields(values: CardFieldValues, ctx: z.RefinementCtx): void {
  const name = values.embossed_name;
  if (name.length <= EMBOSSED_NAME_LENGTH) {
    // `INSPECT … CONVERTING` turns the 52 ASCII letters into spaces and the
    // trimmed remainder must be empty [app/cbl/COCRDUPC.cbl:L823-837].
    if (
      isNotEntered(name, EMBOSSED_NAME_LENGTH) ||
      isZeros(name, EMBOSSED_NAME_LENGTH)
    ) {
      addIssue(ctx, "embossed_name", CARD_MESSAGES.nameNotProvided);
    } else if (!/^[A-Za-z ]+$/.test(name)) {
      addIssue(ctx, "embossed_name", CARD_MESSAGES.nameNotAlphabetic);
    }
  }
  // `88 FLG-YES-NO-VALID VALUES 'Y', 'N'` [app/cbl/COCRDUPC.cbl:L89-91]; a
  // status not provided gets the same text.
  const status = values.active_status;
  if (
    status.length <= ACTIVE_STATUS_LENGTH &&
    status !== "Y" &&
    status !== "N"
  ) {
    addIssue(ctx, "active_status", CARD_MESSAGES.statusNotYesNo);
  }
  // A month or year not provided gets its range text, which no value of
  // spaces, `*` or zeros passes.
  const month = values.expiry_month;
  if (month.length <= EXPIRY_MONTH_LENGTH && !VALID_MONTH.test(month)) {
    addIssue(ctx, "expiry_month", CARD_MESSAGES.expiryMonthNotValid);
  }
  const year = values.expiry_year;
  if (year.length <= EXPIRY_YEAR_LENGTH && !VALID_YEAR.test(year)) {
    addIssue(ctx, "expiry_year", CARD_MESSAGES.expiryYearNotValid);
  }
}

/**
 * The CCUP edit form, keys in audit Order: the version token
 * (V-COCRDUPC-001, Order 1), the account number (from Order 2), the card
 * number (from Order 4), then the name, status, month and year (Orders 9 to
 * 16).
 *
 * `version` is the hidden token of the fetched card: its pattern carries no
 * audited text, so it keeps zod's own. `card_number` is the read-only key of
 * the route, the `{id}` path segment, held here because `CardUpdateField`
 * names CARDSID; {@link toCardUpdateRequest} leaves it out of the body.
 *
 * The COBOL edits are one object-level check because the key edits depend on
 * both keys and a failing key skips the card-field edits
 * (`GO TO 1200-EDIT-MAP-INPUTS-EXIT` [app/cbl/COCRDUPC.cbl:L645-663]).
 *
 * Every `CardUpdateRequest` field is optional on the wire, so the schema is
 * checked against `Required<CardUpdateRequest>`, which implies the plain type
 * and also fails `tsc` when the DTO gains a field the form does not hold.
 */
export const cardUpdateSchema = z
  .object({
    version: z.string().regex(VERSION_PATTERN),
    account_id: z.string().max(ACCOUNT_ID_LENGTH),
    card_number: z.string().max(CARD_NUMBER_LENGTH),
    embossed_name: z.string().max(EMBOSSED_NAME_LENGTH),
    active_status: z.string().max(ACTIVE_STATUS_LENGTH),
    expiry_month: z.string().max(EXPIRY_MONTH_LENGTH),
    expiry_year: z.string().max(EXPIRY_YEAR_LENGTH),
  })
  .superRefine((values, ctx) => {
    if (checkKeys(values, ctx)) {
      checkCardFields(values, ctx);
    }
  }) satisfies z.ZodType<Required<CardUpdateRequest>>;

/** The CCUP edit form's values, as entered. */
export type CardUpdateFormValues = z.infer<typeof cardUpdateSchema>;

/**
 * The `PUT /cards/{id}` body of valid form values: every map field unchanged,
 * without `card_number`, which travels as the `{id}` path segment.
 */
export function toCardUpdateRequest(
  values: CardUpdateFormValues,
): CardUpdateRequest {
  return {
    version: values.version,
    account_id: values.account_id,
    embossed_name: values.embossed_name,
    active_status: values.active_status,
    expiry_month: values.expiry_month,
    expiry_year: values.expiry_year,
  };
}

/** The form field of each BMS field a `PUT /cards/{id}` error names. */
export const cardUpdateBmsFieldMap: Record<
  CardUpdateField,
  keyof CardUpdateFormValues
> = {
  ACCTSID: "account_id",
  CARDSID: "card_number",
  CRDNAME: "embossed_name",
  CRDSTCD: "active_status",
  EXPMON: "expiry_month",
  EXPYEAR: "expiry_year",
};
