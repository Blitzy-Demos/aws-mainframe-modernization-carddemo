/**
 * CTLI and CTTU form schemas: the client mirror of the transaction type
 * requests over the Db2 master `transaction_types`.
 *
 * | Schema | Request | COBOL edits | Error fields |
 * |---|---|---|---|
 * | `tranTypeListFilterSchema` | `GET /admin/transaction-types` filters `TRTYPE`, `TRDESC` | COTRTLIC 1220-EDIT-TYPECD, 1230-EDIT-DESC | `TranTypeListField` |
 * | `tranTypeInlineEditSchema` | `PUT /admin/transaction-types/{id}?origin=list`, row description `TRTYPD1`–`TRTYPD7` | COTRTLIC 1211-EDIT-ARRAY-DESC, 1240-EDIT-ALPHANUM-REQD | none |
 * | `tranTypeCreateSchema` | `POST /admin/transaction-types`, `TRTYPCD` and `TRTYDSC` | COTRTUPC 1210-EDIT-TRANTYPE, 1245-EDIT-NUM-REQD, 1230-EDIT-ALPHANUM-REQD | `TranTypeUpsertField` |
 * | `tranTypeUpdateSchema` | `PUT /admin/transaction-types/{id}`, `TRTYDSC` | COTRTUPC 1230-EDIT-ALPHANUM-REQD | `TranTypeUpsertField` |
 *
 * The server stays authoritative (docs/parity/validation-audit.md, sections
 * COTRTLIC and COTRTUPC). These schemas block only input the server rejects,
 * with the server's text, in the order the server rejects it:
 *
 * 1. Each field holds at most the characters of its BMS field: `X(2)` for the
 *    type codes, `X(50)` for the descriptions
 *    [app/app-transaction-type-db2/cpy-bms/COTRTLI.cpy:L66-90]
 *    [app/app-transaction-type-db2/cpy-bms/COTRTUP.cpy:L60-66]. The server
 *    checks these lengths before any COBOL edit and answers them with an
 *    empty message, so these issues keep zod's own text and no COBOL edit
 *    runs after one.
 * 2. The COBOL edits that depend on the entry alone, with their audited texts.
 *
 * Server-only outcomes: a code already on file on add (V-COTRTUPC-015), a
 * filter matching no type (V-COTRTLIC-002), a type not found, the Db2
 * failures, the delete outcomes, the key `.5` whose two-digit store E-40
 * leaves unpinned, and the no-change compares (V-COTRTUPC-006,
 * V-COTRTLIC-012). Those compares run before the description edits on the
 * server, so a description equal to the stored one up to case is answered 200
 * there even when it holds a character these schemas refuse; only a type
 * written by the MNTTRDB2 batch can store such a description.
 *
 * Values are never trimmed or case-changed: the server trims and compares
 * them itself, and a value sent changed would reach different COBOL paths.
 * Every write reaches the Db2 master only; reports show an edit after
 * `batch seed --only tranextr`.
 */
import { z } from "zod";

import type { components, paths } from "@/api/schema";

type TranTypeListQuery = NonNullable<
  paths["/admin/transaction-types"]["get"]["parameters"]["query"]
>;
type TranTypeCreateRequest = components["schemas"]["TranTypeCreateRequest"];
type TranTypeUpdateRequest = components["schemas"]["TranTypeUpdateRequest"];
type TranTypeListField = components["schemas"]["TranTypeListField"];
type TranTypeUpsertField = components["schemas"]["TranTypeUpsertField"];

/** `TRTYPEI` of COTRTLI and `TRTYPCDI` of COTRTUP, `PIC X(2)`. */
const TYPE_CODE_LENGTH = 2;

/**
 * `TRDESCI` and `TRTYPD1I` to `TRTYPD7I` of COTRTLI and `TRTYDSCI` of
 * COTRTUP, `PIC X(50)`.
 */
const DESCRIPTION_LENGTH = 50;

/** The COTRTLIC texts of the edits this module repeats, byte for byte. */
export const TRAN_TYPE_LIST_MESSAGES = {
  // V-COTRTLIC-001
  typeCodeFilterNotTwoDigits:
    "TYPE CODE FILTER,IF SUPPLIED MUST BE A 2 DIGIT NUMBER",
  // V-COTRTLIC-013
  descriptionMustBeSupplied: "Transaction Desc must be supplied.",
  // V-COTRTLIC-014
  descriptionAlphanumericOnly:
    "Transaction Desc can have numbers or alphabets only.",
} as const;

/** The COTRTUPC texts of the edits this module repeats, byte for byte. */
export const TRAN_TYPE_UPSERT_MESSAGES = {
  // V-COTRTUPC-001, V-COTRTUPC-012
  typeCodeMustBeSupplied: "Tran Type code must be supplied.",
  // V-COTRTUPC-002, V-COTRTUPC-013
  typeCodeMustBeNumeric: "Tran Type code must be numeric.",
  // V-COTRTUPC-003, V-COTRTUPC-014
  typeCodeMustNotBeZero: "Tran Type code must not be zero.",
  // V-COTRTUPC-007, V-COTRTUPC-017
  descriptionMustBeSupplied: "Transaction Desc must be supplied.",
  // V-COTRTUPC-008, V-COTRTUPC-018
  descriptionAlphanumericOnly:
    "Transaction Desc can have numbers or alphabets only.",
} as const;

/** The two description texts, which both programs build alike. */
type DescriptionMessages = {
  readonly descriptionMustBeSupplied: string;
  readonly descriptionAlphanumericOnly: string;
};

/** `IS NUMERIC` on the `X(2)` type code filter: two digits. */
const TWO_DIGITS = /^[0-9]{2}$/;

/**
 * The arguments `FUNCTION TEST-NUMVAL` accepts in the two-byte key:
 * optional spaces, an optional leading `+` or `-` with optional spaces,
 * digits with an optional decimal point (at least one digit), optional
 * spaces, and, without a leading sign, an optional trailing `+` or `-` with
 * optional spaces. The `CR` and `DB` forms need two bytes beside a digit, so
 * no two-byte key holds one.
 */
const NUMVAL_ARGUMENT =
  /^ *(?:[+-] *(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+) *|(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+) *(?:[+-] *)?)$/;

/** A digit other than zero: `FUNCTION NUMVAL(...)` of the argument is not 0. */
const NON_ZERO_DIGIT = /[1-9]/;

/** The `INSPECT ... CONVERTING` of A-Z, a-z and 0-9 leaves only spaces. */
const ALPHANUMERIC_OR_SPACE = /^[A-Za-z0-9 ]*$/;

/** Every byte a space; the empty text included. */
const ALL_SPACES = /^ *$/;

/** Every byte `LOW-VALUES`. */
const ALL_LOW_VALUES = /^\u0000*$/;

/** The entry moved into its `X(width)` map field: padded with spaces. */
function mapField(value: string, width: number): string {
  return value.padEnd(width, " ");
}

/**
 * The not-supplied test of COTRTUPC 1245-EDIT-NUM-REQD and
 * 1230-EDIT-ALPHANUM-REQD and of COTRTLIC 1240-EDIT-ALPHANUM-REQD, after the
 * entry is received: a field equal to `*` or to spaces is `LOW-VALUES`
 * (COTRTUPC 1150-STORE-MAP-IN-NEW, COTRTLIC 1100-RECEIVE-SCREEN), and a
 * field of spaces or of `LOW-VALUES` is not supplied. ` *` is supplied, and
 * so is a field holding both spaces and `LOW-VALUES`.
 */
function isNotSupplied(value: string, width: number): boolean {
  const field = mapField(value, width);
  return (
    field === mapField("*", width) ||
    ALL_SPACES.test(field) ||
    ALL_LOW_VALUES.test(field)
  );
}

/**
 * COTRTLIC 1220-EDIT-TYPECD — V-COTRTLIC-001: spaces or `LOW-VALUES` select
 * every type, two digits select one (`00`, the COBOL `ZEROS`, every type),
 * and anything else (`1`, `0`, `*`, `0A`) is refused.
 */
function typeCodeFilterEdit(value: string, ctx: z.RefinementCtx): void {
  if (value.length > TYPE_CODE_LENGTH) {
    return;
  }
  const field = mapField(value, TYPE_CODE_LENGTH);
  if (
    ALL_SPACES.test(field) ||
    ALL_LOW_VALUES.test(field) ||
    TWO_DIGITS.test(field)
  ) {
    return;
  }
  ctx.addIssue({
    code: z.ZodIssueCode.custom,
    message: TRAN_TYPE_LIST_MESSAGES.typeCodeFilterNotTwoDigits,
  });
}

/**
 * COTRTUPC 1245-EDIT-NUM-REQD on the type code — V-COTRTUPC-012 to
 * V-COTRTUPC-014: not supplied, then not a `NUMVAL` argument, then zero.
 *
 * Every `TEST-NUMVAL` form passes, not only two digits: COTRTUPC stores `1`,
 * ` 1`, `+7`, `1-`, `7.` and `.7` as two digits (P-COTRTUPC-015), so a
 * two-digit rule would refuse input the server accepts, with a text it does
 * not give for that input.
 */
function typeCodeEditMessage(value: string): string | undefined {
  if (isNotSupplied(value, TYPE_CODE_LENGTH)) {
    return TRAN_TYPE_UPSERT_MESSAGES.typeCodeMustBeSupplied;
  }
  const field = mapField(value, TYPE_CODE_LENGTH);
  if (!NUMVAL_ARGUMENT.test(field)) {
    return TRAN_TYPE_UPSERT_MESSAGES.typeCodeMustBeNumeric;
  }
  if (!NON_ZERO_DIGIT.test(field)) {
    return TRAN_TYPE_UPSERT_MESSAGES.typeCodeMustNotBeZero;
  }
  return undefined;
}

/**
 * COTRTUPC 1230-EDIT-ALPHANUM-REQD and COTRTLIC 1240-EDIT-ALPHANUM-REQD on a
 * description of length 50: not supplied, then a character other than A-Z,
 * a-z, 0-9 or space.
 */
function descriptionEditMessage(
  value: string,
  messages: DescriptionMessages,
): string | undefined {
  if (isNotSupplied(value, DESCRIPTION_LENGTH)) {
    return messages.descriptionMustBeSupplied;
  }
  if (!ALPHANUMERIC_OR_SPACE.test(value)) {
    return messages.descriptionAlphanumericOnly;
  }
  return undefined;
}

/**
 * A description field edited on its own: the length check, then, within the
 * length, the description edits. An absent description is the empty field.
 */
function descriptionField(messages: DescriptionMessages) {
  return z
    .string({ required_error: messages.descriptionMustBeSupplied })
    .max(DESCRIPTION_LENGTH)
    .superRefine((value, ctx) => {
      if (value.length > DESCRIPTION_LENGTH) {
        return;
      }
      const message = descriptionEditMessage(value, messages);
      if (message !== undefined) {
        ctx.addIssue({ code: z.ZodIssueCode.custom, message });
      }
    });
}

// The shapes below are keyed by the DTOs' own properties: every member of
// these DTOs is optional, so `satisfies z.ZodType<…>` alone still passes after
// a member is renamed, removed or added, while a shape checked against
// `Record<keyof Dto, z.ZodTypeAny>` fails tsc.

// --- CTLI list filters (COTRTLI TRTYPE, TRDESC)

/**
 * The filter members of the `GET /admin/transaction-types` query. `Pick`
 * names them, so a renamed or removed query parameter fails tsc.
 */
export type TranTypeListFilterQuery = Pick<
  TranTypeListQuery,
  "type_code" | "description"
>;

/**
 * The list filters. Both are optional: an absent, empty or blank filter
 * selects every type. Keys follow the audit `Order` (TRTYPE 1, TRDESC 2);
 * the description filter has no COBOL edit beyond its length.
 *
 * A blank filter stays `""` here rather than being transformed to
 * `undefined`: a transform makes the schema's output type differ from its
 * input type, which `useForm<TranTypeListFilterValues>` with `zodResolver`
 * cannot type. `toTranTypeListQuery` drops blank filters from the query.
 */
export const tranTypeListFilterSchema = z.object({
  type_code: z
    .string()
    .max(TYPE_CODE_LENGTH)
    .superRefine(typeCodeFilterEdit)
    .optional(),
  description: z.string().max(DESCRIPTION_LENGTH).optional(),
}) satisfies z.ZodType<TranTypeListFilterQuery>;

export type TranTypeListFilterValues = z.infer<typeof tranTypeListFilterSchema>;

/** A filter as sent: omitted when absent, empty or all spaces. */
function suppliedFilter(value: string | undefined): string | undefined {
  return value === undefined || ALL_SPACES.test(value) ? undefined : value;
}

/**
 * The filter members of the list query: an empty or all-spaces filter is
 * omitted, which COTRTLIC reads as no filter (1220-EDIT-TYPECD,
 * 1230-EDIT-DESC); any other filter is sent as entered.
 */
export function toTranTypeListQuery(
  values: TranTypeListFilterValues,
): TranTypeListFilterQuery {
  const query: TranTypeListFilterQuery = {};
  const typeCode = suppliedFilter(values.type_code);
  if (typeCode !== undefined) {
    query.type_code = typeCode;
  }
  const description = suppliedFilter(values.description);
  if (description !== undefined) {
    query.description = description;
  }
  return query;
}

/** The form field of each BMS field a `GET /admin/transaction-types` error names. */
export const tranTypeListBmsFieldMap: Record<
  TranTypeListField,
  keyof TranTypeListFilterValues
> = {
  TRTYPE: "type_code",
  TRDESC: "description",
};

// --- CTLI inline description update (COTRTLI TRTYPDn, F10)

const inlineEditBodyShape = {
  description: descriptionField(TRAN_TYPE_LIST_MESSAGES),
} satisfies Record<keyof TranTypeUpdateRequest, z.ZodTypeAny>;

/**
 * The inline update of a listed row. The row key travels in the path, not
 * the body; it is optional here so that a row form holding only the
 * description still submits.
 */
export const tranTypeInlineEditSchema = z.object({
  type_code: z.string().max(TYPE_CODE_LENGTH).optional(),
  ...inlineEditBodyShape,
}) satisfies z.ZodType<TranTypeUpdateRequest>;

export type TranTypeInlineEditValues = z.infer<typeof tranTypeInlineEditSchema>;

/**
 * The BMS fields a `PUT /admin/transaction-types/{id}?origin=list` error
 * names: none. Its rows mark the selected row's `TRTYPDn` or `TRTSELn`, so
 * no single field exists (V-COTRTLIC-011 to V-COTRTLIC-018), and the server
 * sends no `fields`.
 */
export type TranTypeInlineEditField = never;

/** Empty: an inline update error is shown as its message alone. */
export const tranTypeInlineEditBmsFieldMap: Record<
  TranTypeInlineEditField,
  keyof TranTypeInlineEditValues
> = {};

// --- CTTU add and update (COTRTUP TRTYPCD, TRTYDSC)

/**
 * The add. Keys follow the audit `Order` (TRTYPCD 1, TRTYDSC 5). The
 * description is edited only when both lengths pass and the key passes its
 * edits, because COTRTUPC 1200-EDIT-MAP-INPUTS leaves before the description
 * edit when the key is blank or invalid
 * [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L716-737]. An absent member
 * is the empty field, answered with its must-be-supplied text.
 */
const createShape = {
  type_code: z
    .string({
      required_error: TRAN_TYPE_UPSERT_MESSAGES.typeCodeMustBeSupplied,
    })
    .max(TYPE_CODE_LENGTH),
  description: z
    .string({
      required_error: TRAN_TYPE_UPSERT_MESSAGES.descriptionMustBeSupplied,
    })
    .max(DESCRIPTION_LENGTH),
} satisfies Record<keyof TranTypeCreateRequest, z.ZodTypeAny>;

export const tranTypeCreateSchema = z
  .object(createShape)
  .superRefine((values, ctx) => {
    if (
      values.type_code.length > TYPE_CODE_LENGTH ||
      values.description.length > DESCRIPTION_LENGTH
    ) {
      return;
    }
    const keyMessage = typeCodeEditMessage(values.type_code);
    if (keyMessage !== undefined) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["type_code"],
        message: keyMessage,
      });
      return;
    }
    const descriptionMessage = descriptionEditMessage(
      values.description,
      TRAN_TYPE_UPSERT_MESSAGES,
    );
    if (descriptionMessage !== undefined) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["description"],
        message: descriptionMessage,
      });
    }
  }) satisfies z.ZodType<TranTypeCreateRequest>;

/**
 * The update of the maintenance screen, `PUT` without `origin`: the
 * description only, the key being the path of a type already shown.
 */
const updateShape = {
  description: descriptionField(TRAN_TYPE_UPSERT_MESSAGES),
} satisfies Record<keyof TranTypeUpdateRequest, z.ZodTypeAny>;

export const tranTypeUpdateSchema = z.object(updateShape) satisfies z.ZodType<
  TranTypeUpdateRequest
>;

/** The edit form: the update, with the route's type code shown read-only. */
export const tranTypeEditFormSchema = tranTypeUpdateSchema.extend({
  type_code: z.string(),
});

/** The values of the CTTU form in both modes. */
export type TranTypeFormValues = z.infer<typeof tranTypeCreateSchema>;

/**
 * The body of `PUT /admin/transaction-types/{id}` from the values of the CTTU
 * form or of a CTLI row form: the description; the code is the path.
 */
export function toTranTypeUpdateRequest(
  values: TranTypeFormValues | TranTypeInlineEditValues,
): TranTypeUpdateRequest {
  return { description: values.description };
}

/** The form field of each BMS field a CTTU `POST` or `PUT` error names. */
export const tranTypeUpsertBmsFieldMap: Record<
  TranTypeUpsertField,
  keyof TranTypeFormValues
> = {
  TRTYPCD: "type_code",
  TRTYDSC: "description",
};
