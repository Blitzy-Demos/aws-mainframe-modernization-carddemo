// CB00 bill payment (COBIL00C, map COBIL00): the client mirror of the edits
// `POST /payments` makes before it reads a file. The server
// (crates/api/src/dto/payments.rs and the COBIL00C rules) stays authoritative,
// and every other outcome is its alone: 'You have nothing to pay...', the
// not-found and write failures, and the cancel on `N`, which answers 400 with
// no message. The form first sends `confirm: ""`, which the server answers 200
// with the current balance and 'Confirm to make a bill payment...' and no
// write; the page's confirmation dialog then resubmits with `confirm: "Y"`,
// which pays (201, 'Payment successful.  Your Transaction ID is …').
//
// Entries are checked exactly as typed, never trimmed or case-folded.
import { z } from "zod";

import type { components } from "@/api/schema";

import { isSpacesOrLowValues } from "./primitives";

type PaymentRequest = components["schemas"]["PaymentRequest"];
type BillPaymentField = components["schemas"]["BillPaymentField"];

/** The COBIL00C messages of the edits this schema repeats, byte-identical. */
export const PAYMENT_MESSAGES = {
  // V-COBIL00C-001: account ID blank or LOW-VALUES.
  accountIdEmpty: "Acct ID can NOT be empty...",
  // V-COBIL00C-003: confirm other than Y, y, N, n, blank or LOW-VALUES.
  confirmInvalid: "Invalid value. Valid values are (Y/N)...",
} as const;

// The widths of ACTIDINI X(11) and CONFIRMI X(1), which the PaymentRequest
// maxLength of account_id and confirm declares. A longer entry has no COBOL
// message, so it keeps zod's default text.
const ACCOUNT_ID_LENGTH = 11;
const CONFIRM_LENGTH = 1;

// The confirmations COBIL00C acts on besides a blank one: Y and y pay, N and n
// cancel.
const CONFIRM_ENTRIES: readonly string[] = ["Y", "y", "N", "n"];

// Keyed by the DTO's own properties: every PaymentRequest property is
// optional, so `satisfies z.ZodType<PaymentRequest>` alone would still pass
// after one is renamed, removed or added; this map fails tsc instead.
// account_id is required here although the DTO makes it optional, because the
// server reads a missing account ID as a blank one (V-COBIL00C-001).
const paymentShape = {
  account_id: z
    .string({ required_error: PAYMENT_MESSAGES.accountIdEmpty })
    .max(ACCOUNT_ID_LENGTH),
  confirm: z.string().max(CONFIRM_LENGTH).optional(),
} satisfies Record<keyof PaymentRequest, z.ZodTypeAny>;

const paymentFields = z.object(paymentShape);

// COBIL00C PROCESS-ENTER-KEY edits in the COBOL order, reporting only the
// first that fails, as the program does: a blank account ID stops before the
// confirmation is looked at (V-COBIL00C-001, order 1; V-COBIL00C-003, order
// 2). They are object-level rather than per field because of that order. An
// over-long field skips them, since the server's length check answers alone
// before the rule runs. Blank is SPACES or LOW-VALUES, the test the server's
// bill-payment rule makes of ACTIDINI and CONFIRMI.
function editEnterKey(
  { account_id, confirm = "" }: z.infer<typeof paymentFields>,
  ctx: z.RefinementCtx,
): void {
  if (
    account_id.length > ACCOUNT_ID_LENGTH ||
    confirm.length > CONFIRM_LENGTH
  ) {
    return;
  }
  if (isSpacesOrLowValues(account_id)) {
    ctx.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["account_id"],
      message: PAYMENT_MESSAGES.accountIdEmpty,
    });
    return;
  }
  if (!isSpacesOrLowValues(confirm) && !CONFIRM_ENTRIES.includes(confirm)) {
    ctx.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["confirm"],
      message: PAYMENT_MESSAGES.confirmInvalid,
    });
  }
}

export const paymentSchema = paymentFields.superRefine(
  editEnterKey,
) satisfies z.ZodType<PaymentRequest>;

export type PaymentFormValues = z.infer<typeof paymentSchema>;

/** The form field each BMS field named in an error's `fields[]` belongs to. */
export const billPaymentBmsFieldMap: Record<
  BillPaymentField,
  keyof PaymentFormValues
> = {
  ACTIDIN: "account_id",
  CONFIRM: "confirm",
};
