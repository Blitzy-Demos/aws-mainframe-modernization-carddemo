/**
 * Client mirrors of the server's input edits for the transaction resources:
 * the CT00 browse filter (`GET /transactions`, COTRN00C, map COTRN00), the CT01
 * view key (`GET /transactions/{id}`, COTRN01C, map COTRN01) and the CT02 add
 * form (`POST /transactions`, COTRN02C, map COTRN02).
 *
 * The server is authoritative. These schemas keep a form from sending what the
 * server rejects with an audited message, and accept everything it accepts.
 * Lengths are the OpenAPI `maxLength` of each member, which equal the BMS field
 * widths. An over-length entry has no COBOL path (a 3270 field holds no more
 * than its width) and the server answers it with no message (E-49), so it keeps
 * zod's default text and gets no other issue. Every other message is the
 * byte-identical text of its `docs/parity/validation-audit.md` row. Values stay
 * the strings entered, untrimmed and in their case; COTRN02C converts them
 * (`FUNCTION NUMVAL`, `NUMVAL-C`) server-side.
 *
 * Server-only outcomes: the cross-reference look-ups of the account or card
 * (V-COTRN02C-004 to V-COTRN02C-007), the CSUTLDTC calendar checks of both
 * dates with their 2513 exception (V-COTRN02C-024, V-COTRN02C-025), the blank
 * or `N` confirmation prompt (V-COTRN02C-027), the TRAN-ID allocation and the
 * write (V-COTRN02C-029 to V-COTRN02C-033), and every CT00 and CT01 read.
 */
import { z } from "zod";

import type { components, paths } from "@/api/schema";

/** COTRN00C texts of the CT00 browse filter. */
export const transactionListMessages = {
  /** V-COTRN00C-004: start key entered and not 16 digits. */
  tranIdMustBeNumeric: "Tran ID must be Numeric ...",
} as const;

/** COTRN01C texts of the CT01 view key. */
export const transactionViewMessages = {
  /** V-COTRN01C-001: Tran ID blank or LOW-VALUES. */
  tranIdCanNotBeEmpty: "Tran ID can NOT be empty...",
} as const;

/** COTRN02C texts of the CT02 add form, in audit Order. */
export const transactionAddMessages = {
  /** V-COTRN02C-001: account ID entered and not 11 digits. */
  accountIdMustBeNumeric: "Account ID must be Numeric...",
  /** V-COTRN02C-002: account ID blank, card number entered and not 16 digits. */
  cardNumberMustBeNumeric: "Card Number must be Numeric...",
  /** V-COTRN02C-003: account ID and card number both blank. */
  accountOrCardNumberMustBeEntered: "Account or Card Number must be entered...",
  /** V-COTRN02C-008 */
  typeCdCanNotBeEmpty: "Type CD can NOT be empty...",
  /** V-COTRN02C-009 */
  categoryCdCanNotBeEmpty: "Category CD can NOT be empty...",
  /** V-COTRN02C-010 */
  sourceCanNotBeEmpty: "Source can NOT be empty...",
  /** V-COTRN02C-011 */
  descriptionCanNotBeEmpty: "Description can NOT be empty...",
  /** V-COTRN02C-012 */
  amountCanNotBeEmpty: "Amount can NOT be empty...",
  /** V-COTRN02C-013 */
  origDateCanNotBeEmpty: "Orig Date can NOT be empty...",
  /** V-COTRN02C-014 */
  procDateCanNotBeEmpty: "Proc Date can NOT be empty...",
  /** V-COTRN02C-015 */
  merchantIdCanNotBeEmpty: "Merchant ID can NOT be empty...",
  /** V-COTRN02C-016 */
  merchantNameCanNotBeEmpty: "Merchant Name can NOT be empty...",
  /** V-COTRN02C-017 */
  merchantCityCanNotBeEmpty: "Merchant City can NOT be empty...",
  /** V-COTRN02C-018 */
  merchantZipCanNotBeEmpty: "Merchant Zip can NOT be empty...",
  /** V-COTRN02C-019: type code not 2 digits. */
  typeCdMustBeNumeric: "Type CD must be Numeric...",
  /** V-COTRN02C-020: category code not 4 digits. */
  categoryCdMustBeNumeric: "Category CD must be Numeric...",
  /** V-COTRN02C-021: amount not sign, 8 digits, point, 2 digits. */
  amountShouldBeInFormat: "Amount should be in format -99999999.99",
  /** V-COTRN02C-022: original date not `YYYY-MM-DD` digits and dashes. */
  origDateShouldBeInFormat: "Orig Date should be in format YYYY-MM-DD",
  /** V-COTRN02C-023: processing date not `YYYY-MM-DD` digits and dashes. */
  procDateShouldBeInFormat: "Proc Date should be in format YYYY-MM-DD",
  /** V-COTRN02C-026: merchant ID not 9 digits. */
  merchantIdMustBeNumeric: "Merchant ID must be Numeric...",
  /** V-COTRN02C-028: confirmation other than Y, y, N, n or blank. */
  invalidConfirmValue: "Invalid value. Valid values are (Y/N)...",
} as const;

type TransactionAddRequest = components["schemas"]["TransactionAddRequest"];
type TransactionListQuery = NonNullable<
  paths["/transactions"]["get"]["parameters"]["query"]
>;
/** The query parameters the CT00 search form sets; paging sets the others. */
type TransactionListFilterParams = Pick<TransactionListQuery, "start_key" | "account_id">;

/**
 * The OpenAPI `maxLength` of every `TransactionAddRequest` member, equal to the
 * width of its `COTRN2AI` field [app/cpy-bms/COTRN02.CPY]. PF5 cuts copied
 * values to the same widths.
 */
const ADD_WIDTH = {
  account_id: 11,
  card_number: 16,
  tran_type_cd: 2,
  tran_cat_cd: 4,
  tran_source: 10,
  tran_desc: 60,
  tran_amt: 12,
  tran_orig_date: 10,
  tran_proc_date: 10,
  tran_merchant_id: 9,
  tran_merchant_name: 30,
  tran_merchant_city: 25,
  tran_merchant_zip: 10,
  confirm: 1,
} as const satisfies Record<keyof TransactionAddRequest, number>;

/** The OpenAPI `maxLength` of the CT00 `account_id` filter, an `ACCT-ID 9(11)`. */
const ACCOUNT_FILTER_WIDTH = 11;

/** `TRNIDINI X(16)` of map COTRN01 [app/cpy-bms/COTRN01.CPY]. */
const TRAN_ID_WIDTH = 16;

// `IS NUMERIC` on a space-padded `PIC X(n)` field holds only when all n bytes
// are digits, so each numeric edit asks for exactly n ASCII digits.
const ACCOUNT_ID = /^\d{11}$/;
const CARD_NUMBER = /^\d{16}$/;
const TRAN_ID = /^\d{16}$/;
const TYPE_CD = /^\d{2}$/;
const CATEGORY_CD = /^\d{4}$/;
const MERCHANT_ID = /^\d{9}$/;
/**
 * TRNAMTI (1:1) `-` or `+`, (2:8) digits, (10:1) `.`, (11:2) digits
 * [app/cbl/COTRN02C.cbl:L339-348].
 */
const AMOUNT = /^[-+]\d{8}\.\d{2}$/;
/**
 * (1:4) digits, (5:1) `-`, (6:2) digits, (8:1) `-`, (9:2) digits
 * [app/cbl/COTRN02C.cbl:L353-378]. The dates are text inputs, not a date
 * picker; calendar validity is CSUTLDTC's, server-side.
 */
const DATE = /^\d{4}-\d{2}-\d{2}$/;
/**
 * The confirmations COTRN02C does not reject: `Y`/`y` add, `N`/`n` prompt
 * [app/cbl/COTRN02C.cbl:L169-187].
 */
const CONFIRM = /^[YyNn]$/;

/**
 * `= SPACES OR LOW-VALUES`: empty, spaces only, or NUL characters only, as the
 * server reads a received map field.
 */
const BLANK = /^(?: *|\u0000+)$/;

function isBlank(value: string): boolean {
  return BLANK.test(value);
}

/**
 * A format or class edit. Without `message` the edit has no audited text and
 * reports zod's own regex issue.
 */
interface Edit {
  readonly pattern: RegExp;
  readonly message?: string;
}

function checkEdit(value: string, edit: Edit, ctx: z.RefinementCtx): void {
  if (edit.pattern.test(value)) {
    return;
  }
  if (edit.message === undefined) {
    ctx.addIssue({ code: z.ZodIssueCode.invalid_string, validation: "regex" });
  } else {
    ctx.addIssue({ code: z.ZodIssueCode.custom, message: edit.message });
  }
}

/**
 * A field the COBOL requires: at most `width` characters, then `emptyMessage`
 * when blank, then `edit` when given. At most one issue per value, the first
 * the COBOL would raise for the field.
 */
function requiredField(width: number, emptyMessage: string, edit?: Edit) {
  return z
    .string()
    .max(width)
    .superRefine((value, ctx) => {
      if (value.length > width) {
        return;
      }
      if (isBlank(value)) {
        ctx.addIssue({ code: z.ZodIssueCode.custom, message: emptyMessage });
      } else if (edit !== undefined) {
        checkEdit(value, edit, ctx);
      }
    });
}

/** A field the COBOL edits only when entered: at most `width` characters, then `edit`. */
function enteredField(width: number, edit: Edit) {
  return z
    .string()
    .max(width)
    .superRefine((value, ctx) => {
      if (value.length <= width && !isBlank(value)) {
        checkEdit(value, edit, ctx);
      }
    });
}

/** An empty query value is no parameter. */
function omitEmpty(value: string | undefined): string | undefined {
  return value === "" ? undefined : value;
}

/**
 * An optional query parameter: absent or blank lists without it (the server
 * reads a blank value as absent), anything else must pass `edit`. `""` is sent
 * as no parameter; every other value is sent as entered.
 */
function optionalParam(edit: Edit, width?: number) {
  const text = width === undefined ? z.string() : z.string().max(width);
  return text
    .optional()
    .superRefine((value, ctx) => {
      if (value === undefined || isBlank(value)) {
        return;
      }
      if (width === undefined || value.length <= width) {
        checkEdit(value, edit, ctx);
      }
    })
    .transform(omitEmpty);
}

// ---------------------------------------------------------------------------
// CT00 browse filter: COTRN00 TRNIDIN and the E-17 account filter
// ---------------------------------------------------------------------------

// Every query parameter is optional, so `satisfies z.ZodType<…>` alone would
// accept a misspelt key; this `satisfies` ties the keys to the API's.
const transactionListFilterShape = {
  account_id: optionalParam({ pattern: ACCOUNT_ID }, ACCOUNT_FILTER_WIDTH),
  start_key: optionalParam({
    pattern: TRAN_ID,
    message: transactionListMessages.tranIdMustBeNumeric,
  }),
} satisfies Record<keyof TransactionListFilterParams, z.ZodTypeAny>;

/**
 * The search form of `GET /transactions`. Keys in audit Order: the account
 * filter is edited first (V-COTRN00C-001, Order 1), then the start key
 * (V-COTRN00C-004, Order 3). Paging (`direction`, `page_size`) comes from the
 * list's Previous and Next controls, not from this form.
 *
 * - `account_id`: the optional filter the REST resource adds (E-17); COTRN00C
 *   has none and parity is measured without it. When entered it must be 11
 *   digits. Its rejection has no COBOL text (`no-message`, E-49), so the issue
 *   keeps zod's default regex message.
 * - `start_key`: `TRNIDINI X(16)` [app/cpy-bms/COTRN00.CPY]. When entered it
 *   must be 16 digits ('Tran ID must be Numeric ...'). OpenAPI declares no
 *   `maxLength` for it, and the 16-digit edit already answers a longer key
 *   with the audited text.
 *
 * Both may be absent, and both parse `""` to `undefined`, so the submitted
 * query carries no empty parameter.
 */
export const transactionListFilterSchema = z.object(
  transactionListFilterShape,
) satisfies z.ZodType<TransactionListFilterParams>;

/** The CT00 search form's values, as entered. */
export type TransactionListFilterValues = z.input<typeof transactionListFilterSchema>;

/** The CT00 query parameters a valid search form yields, `""` dropped. */
export type TransactionListFilterQuery = z.output<typeof transactionListFilterSchema>;

/** `fields[].field` of a `GET /transactions` error → form key. */
export const transactionListBmsFieldMap: Record<
  components["schemas"]["TransactionListField"],
  keyof TransactionListFilterValues
> = {
  TRNIDIN: "start_key",
};

// ---------------------------------------------------------------------------
// CT01 view key: COTRN01 TRNIDIN
// ---------------------------------------------------------------------------

/**
 * The key form of `GET /transactions/{id}`: `tran_id` is TRNIDIN, required
 * ('Tran ID can NOT be empty...'). COTRN01C makes no other edit; a key that
 * names no live transaction is the server's 404 (V-COTRN01C-002).
 *
 * The 16-character limit is the TRNIDIN width, which the 3270 field cannot
 * exceed. OpenAPI declares no `maxLength` on the path segment because the
 * server moves a longer key into `TRAN-ID X(16)`; no audited row covers that,
 * so the limit keeps zod's default text.
 */
export const transactionViewKeySchema = z.object({
  tran_id: requiredField(TRAN_ID_WIDTH, transactionViewMessages.tranIdCanNotBeEmpty),
}) satisfies z.ZodType<{
  tran_id: paths["/transactions/{id}"]["get"]["parameters"]["path"]["id"];
}>;

/** The CT01 key form's values. */
export type TransactionViewKeyValues = z.infer<typeof transactionViewKeySchema>;

/** `fields[].field` of a `GET /transactions/{id}` error → form key. */
export const transactionViewBmsFieldMap: Record<
  components["schemas"]["TransactionViewField"],
  keyof TransactionViewKeyValues
> = {
  TRNIDIN: "tran_id",
};

// ---------------------------------------------------------------------------
// CT02 add form: COTRN02
// ---------------------------------------------------------------------------

// Every DTO member is optional, so `satisfies z.ZodType<…>` alone would accept
// a missing or misspelt member; this `satisfies` ties the keys to the DTO's.
const transactionAddShape = {
  account_id: enteredField(ADD_WIDTH.account_id, {
    pattern: ACCOUNT_ID,
    message: transactionAddMessages.accountIdMustBeNumeric,
  }),
  card_number: z.string().max(ADD_WIDTH.card_number),
  tran_type_cd: requiredField(
    ADD_WIDTH.tran_type_cd,
    transactionAddMessages.typeCdCanNotBeEmpty,
    { pattern: TYPE_CD, message: transactionAddMessages.typeCdMustBeNumeric },
  ),
  tran_cat_cd: requiredField(
    ADD_WIDTH.tran_cat_cd,
    transactionAddMessages.categoryCdCanNotBeEmpty,
    { pattern: CATEGORY_CD, message: transactionAddMessages.categoryCdMustBeNumeric },
  ),
  tran_source: requiredField(ADD_WIDTH.tran_source, transactionAddMessages.sourceCanNotBeEmpty),
  tran_desc: requiredField(ADD_WIDTH.tran_desc, transactionAddMessages.descriptionCanNotBeEmpty),
  tran_amt: requiredField(
    ADD_WIDTH.tran_amt,
    transactionAddMessages.amountCanNotBeEmpty,
    { pattern: AMOUNT, message: transactionAddMessages.amountShouldBeInFormat },
  ),
  tran_orig_date: requiredField(
    ADD_WIDTH.tran_orig_date,
    transactionAddMessages.origDateCanNotBeEmpty,
    { pattern: DATE, message: transactionAddMessages.origDateShouldBeInFormat },
  ),
  tran_proc_date: requiredField(
    ADD_WIDTH.tran_proc_date,
    transactionAddMessages.procDateCanNotBeEmpty,
    { pattern: DATE, message: transactionAddMessages.procDateShouldBeInFormat },
  ),
  tran_merchant_id: requiredField(
    ADD_WIDTH.tran_merchant_id,
    transactionAddMessages.merchantIdCanNotBeEmpty,
    { pattern: MERCHANT_ID, message: transactionAddMessages.merchantIdMustBeNumeric },
  ),
  tran_merchant_name: requiredField(
    ADD_WIDTH.tran_merchant_name,
    transactionAddMessages.merchantNameCanNotBeEmpty,
  ),
  tran_merchant_city: requiredField(
    ADD_WIDTH.tran_merchant_city,
    transactionAddMessages.merchantCityCanNotBeEmpty,
  ),
  tran_merchant_zip: requiredField(
    ADD_WIDTH.tran_merchant_zip,
    transactionAddMessages.merchantZipCanNotBeEmpty,
  ),
  confirm: enteredField(ADD_WIDTH.confirm, {
    pattern: CONFIRM,
    message: transactionAddMessages.invalidConfirmValue,
  }),
} satisfies Record<keyof TransactionAddRequest, z.ZodTypeAny>;

/**
 * The single-field edits of `POST /transactions`, keyed in COTRN02 screen
 * order, which is the audit Order of VALIDATE-INPUT-KEY-FIELDS and
 * VALIDATE-INPUT-DATA-FIELDS [app/cbl/COTRN02C.cbl:L164-437].
 *
 * - `account_id`: optional; when entered it must be 11 digits
 *   (V-COTRN02C-001).
 * - `card_number`: only its width. Its numeric edit and the rule that one of
 *   the two keys is entered depend on the account field, so they are
 *   {@link transactionAddFormSchema}'s.
 * - Data fields: each required with its `can NOT be empty` text
 *   (V-COTRN02C-008 to V-COTRN02C-018), then its class or format edit: type
 *   and category codes 2 and 4 digits, amount `-99999999.99`, dates
 *   `YYYY-MM-DD`, merchant ID 9 digits (V-COTRN02C-019 to V-COTRN02C-023,
 *   V-COTRN02C-026).
 * - `confirm`: blank, `Y`, `y`, `N` or `n`; anything else is V-COTRN02C-028.
 *   Blank and `N` are the server's 'Confirm to add this transaction...'
 *   answer (V-COTRN02C-027), not a client error: the confirmation dialog sends
 *   `Y`.
 *
 * COTRN02C stops at the first failing edit; a form shows each field's own
 * first issue. Forms resolve with {@link transactionAddFormSchema}, which adds
 * the key-field rules to these edits.
 */
export const transactionAddSchema = z.object(
  transactionAddShape,
) satisfies z.ZodType<TransactionAddRequest>;

/**
 * VALIDATE-INPUT-KEY-FIELDS [app/cbl/COTRN02C.cbl:L193-230] for a blank account
 * ID. An entered account ID is the key and the card number is then ignored, so
 * the card's numeric edit (V-COTRN02C-002) applies only without one, and with
 * neither entered the account field gets V-COTRN02C-003. An over-length key is
 * left to the width check, which the server answers before any COBOL edit.
 */
const transactionAddKeyRules = z
  .object({ account_id: z.string(), card_number: z.string() })
  .superRefine(({ account_id, card_number }, ctx) => {
    const withinWidths =
      account_id.length <= ADD_WIDTH.account_id && card_number.length <= ADD_WIDTH.card_number;
    if (!withinWidths || !isBlank(account_id)) {
      return;
    }
    if (isBlank(card_number)) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: transactionAddMessages.accountOrCardNumberMustBeEntered,
        path: ["account_id"],
      });
    } else if (!CARD_NUMBER.test(card_number)) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: transactionAddMessages.cardNumberMustBeNumeric,
        path: ["card_number"],
      });
    }
  });

/**
 * The schema the CT02 form submits with: the key-field rules, then every
 * single-field edit of {@link transactionAddSchema}.
 *
 * It is an intersection rather than a `.superRefine` on the object because a
 * refinement runs after the object and would append the key-field issue
 * behind every data-field issue. Parsing the key rules first keeps the issues
 * in COBOL order: with every field blank the first issue is 'Account or Card
 * Number must be entered...', the next 'Type CD can NOT be empty...'.
 */
export const transactionAddFormSchema = transactionAddKeyRules.and(
  transactionAddSchema,
) satisfies z.ZodType<TransactionAddRequest>;

/** The CT02 form's values: every COTRN2AI input field as a string. */
export type TransactionAddFormValues = z.infer<typeof transactionAddFormSchema>;

/** `fields[].field` of a `POST /transactions` error → form key. */
export const transactionAddBmsFieldMap: Record<
  components["schemas"]["TransactionAddField"],
  keyof TransactionAddFormValues
> = {
  ACTIDIN: "account_id",
  CARDNIN: "card_number",
  TTYPCD: "tran_type_cd",
  TCATCD: "tran_cat_cd",
  TRNSRC: "tran_source",
  TDESC: "tran_desc",
  TRNAMT: "tran_amt",
  TORIGDT: "tran_orig_date",
  TPROCDT: "tran_proc_date",
  MID: "tran_merchant_id",
  MNAME: "tran_merchant_name",
  MCITY: "tran_merchant_city",
  MZIP: "tran_merchant_zip",
  CONFIRM: "confirm",
};

/**
 * The data fields PF5 fills. The key fields and the confirmation are not
 * copied: COPY-LAST-TRAN-DATA keeps what the user entered in them
 * [app/cbl/COTRN02C.cbl:L471-495].
 */
export type TransactionAddCopiedValues = Omit<
  TransactionAddFormValues,
  "account_id" | "card_number" | "confirm"
>;

/** `MOVE` of a text into a `PIC X(width)` map field: cut at the width. */
function moveText(text: string, width: number): string {
  return text.slice(0, width);
}

/**
 * `MOVE TRAN-AMT TO WS-TRAN-AMT-E` (`PIC +99999999.99`) [app/cbl/COTRN02C.cbl:L59]
 * [app/cbl/COTRN02C.cbl:L479] of the API's scale-2 decimal string, by string
 * operations: the low-order eight integer digits (the MOVE drops high-order
 * digits), two decimals, and `-` only for a negative amount whose kept digits
 * are not all zero, as the server renders the picture. An already edited
 * value comes back unchanged.
 */
function editTranAmt(amount: string): string {
  const negative = amount.startsWith("-");
  const unsigned = negative || amount.startsWith("+") ? amount.slice(1) : amount;
  const [whole = "", fraction = ""] = unsigned.split(".");
  const kept = `${whole.padStart(8, "0").slice(-8)}.${fraction.padEnd(2, "0").slice(0, 2)}`;
  return `${negative && /[1-9]/.test(kept) ? "-" : "+"}${kept}`;
}

/**
 * CT02 PF5 "copy last transaction" (P-COTRN02C-026): the form values
 * COPY-LAST-TRAN-DATA moves from the highest-keyed transaction
 * [app/cbl/COTRN02C.cbl:L479-492]. The amount takes its screen edit, each
 * timestamp its first 10 characters (the date), and every text is cut to its
 * map width (`TRAN-DESC X(100)` into `TDESCI X(60)`, for example), so the
 * copy passes the form's length checks.
 */
export function transactionToAddFormValues(
  transaction: components["schemas"]["TransactionResponse"],
): TransactionAddCopiedValues {
  return {
    tran_type_cd: moveText(transaction.tran_type_cd, ADD_WIDTH.tran_type_cd),
    tran_cat_cd: moveText(transaction.tran_cat_cd, ADD_WIDTH.tran_cat_cd),
    tran_source: moveText(transaction.tran_source, ADD_WIDTH.tran_source),
    tran_desc: moveText(transaction.tran_desc, ADD_WIDTH.tran_desc),
    tran_amt: editTranAmt(transaction.tran_amt),
    tran_orig_date: moveText(transaction.tran_orig_ts, ADD_WIDTH.tran_orig_date),
    tran_proc_date: moveText(transaction.tran_proc_ts, ADD_WIDTH.tran_proc_date),
    tran_merchant_id: moveText(transaction.tran_merchant_id, ADD_WIDTH.tran_merchant_id),
    tran_merchant_name: moveText(transaction.tran_merchant_name, ADD_WIDTH.tran_merchant_name),
    tran_merchant_city: moveText(transaction.tran_merchant_city, ADD_WIDTH.tran_merchant_city),
    tran_merchant_zip: moveText(transaction.tran_merchant_zip, ADD_WIDTH.tran_merchant_zip),
  };
}
