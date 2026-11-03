// The application's single display of API outcomes (shared component "Web
// error display"). It turns the 3270 message line, ERRMSG [app/cpy-bms/
// COSGN00.CPY:L84], into Sonner toasts, and the COBOL cursor placement,
// `MOVE -1 TO <field>L` [app/cbl/COSGN00C.cbl:L121, L126]
// [app/cbl/COBIL00C.cbl:L115, L239], into react-hook-form field errors and
// focus. Pages import it and never toast, set errors or move focus themselves.
//
// - A form page passes its schema module's bmsFieldMap:
//
//     mutation.mutate(values, {
//       onSuccess: (d) => showSuccess(d.message),
//       onError: (e) => applyApiError(e, form, bmsFieldMap),
//     });
//
// - A page without a form (lists, detail views, guards) calls showApiError.
// - A 200 answer that only carries a message, writing nothing (CB00's blank
//   confirm 'Confirm to make a bill payment...' [app/cbl/COBIL00C.cbl:L237],
//   COUSR02C 'Please modify to update ...' [app/cbl/COUSR02C.cbl:L239]), goes
//   to showMessage.
// - A 401 is shown like any other error. The API client has already ended the
//   session, and RequireAuth then redirects to the sign-on page.
//
// Every text is shown exactly as the server sent it, never trimmed, rewritten
// or prefixed: it is byte-identical to the audited COBOL message, and the
// trailing padding some of them carry collapses in HTML. The error `code` and
// the HTTP status are never shown. Every function is synchronous and uses no
// React hook, so TanStack Query callbacks and handleSubmit handlers call them
// directly.
import { toast } from "sonner";
import type { FieldValues, Path, UseFormReturn } from "react-hook-form";

import { ApiError, UNEXPECTED_ERROR_BODY } from "@/api/client";

/**
 * The text shown when a failure carries no usable server message: no answer
 * arrived (a network failure, an abort, the client's deadline), the answer had
 * no error body, or its message was blank. It is the server's own text for a
 * 500 without an audited COBOL message (crates/domain/src/messages.rs), which
 * the API client also uses for an unusable body.
 */
export const UNEXPECTED_ERROR: string = UNEXPECTED_ERROR_BODY.message;

/**
 * The form field each BMS field name of an error's `fields[]` belongs to,
 * keyed by the request DTO's OpenAPI field enum (`LoginField`,
 * `BillPaymentField`, …). The keys are the BMS names without their L/I/O
 * suffix (`USERID`, `ACTIDIN`), or the request's own field names for a reward
 * rule. A schema module declares its map against this type, so an enum value
 * without a form field fails `tsc`.
 */
export type BmsFieldMap<F extends string, T extends FieldValues> = Readonly<
  Record<F, Path<T>>
>;

/**
 * What showApiError displayed: the answer's HTTP status, `null` when no answer
 * arrived, and the toasted text.
 */
export type ShownApiError = {
  status: number | null;
  message: string;
};

/**
 * What applyApiError displayed: the toast as for showApiError, and the form
 * fields it marked, in the order the server listed them. The first of them
 * received focus.
 */
export type AppliedApiError<T extends FieldValues> = ShownApiError & {
  fields: Path<T>[];
};

// Whether `message` has something to show. The test only decides whether a
// toast appears; the text shown is never trimmed.
function hasText(message: unknown): message is string {
  return typeof message === "string" && message.trim() !== "";
}

/**
 * Shows a failed request's message with `toast.error`: the server's message
 * for an ApiError, otherwise UNEXPECTED_ERROR.
 */
export function showApiError(error: unknown): ShownApiError {
  if (error instanceof ApiError && hasText(error.body.message)) {
    const { message } = error.body;
    toast.error(message);
    return { status: error.status, message };
  }
  toast.error(UNEXPECTED_ERROR);
  return {
    status: error instanceof ApiError ? error.status : null,
    message: UNEXPECTED_ERROR,
  };
}

// Places the cursor on the first errored field, as the COBOL `MOVE -1 TO
// <field>L` does. react-hook-form focuses, on its next timer task, the
// element a field registered through its ref (the `field.ref` a Controller
// passes to the shadcn Input, SelectTrigger or Checkbox) and does nothing for
// a field without one. It throws when a register()ed group's ref list is
// empty, which happens once every radio or checkbox of the group has
// unmounted; the cursor then stays where it is, since the toast and the
// FieldError already carry the message.
function focusField<T extends FieldValues>(
  form: Pick<UseFormReturn<T>, "setFocus">,
  name: Path<T>,
): void {
  try {
    form.setFocus(name);
  } catch {
    // No focusable element is registered for the field; see above.
  }
}

/**
 * Shows a failed form submission: the message as a toast (showApiError), each
 * `fields[]` entry as the error of its form field, and focus on the first
 * marked field.
 *
 * Entries are taken in the order received, the order in which the COBOL
 * program rejects the fields, and the first message for a form field wins. An
 * entry whose BMS name the map does not hold is skipped, since its text is the
 * toast's. When no entry maps, focus stays where it is, as on a COBOL screen
 * whose cursor the program does not move.
 *
 * The form only needs `setError` and `setFocus`, so a form whose resolver
 * transforms its values passes as well.
 */
export function applyApiError<T extends FieldValues, F extends string>(
  error: unknown,
  form: Pick<UseFormReturn<T>, "setError" | "setFocus">,
  bmsFieldMap: BmsFieldMap<F, T>,
): AppliedApiError<T> {
  const shown = showApiError(error);
  const applied: Path<T>[] = [];
  if (error instanceof ApiError) {
    const entries = Array.isArray(error.body.fields) ? error.body.fields : [];
    for (const entry of entries) {
      // An own key only: a BMS name such as `constructor` must not reach the
      // object prototype.
      if (!Object.hasOwn(bmsFieldMap, entry.field)) {
        continue;
      }
      const name = bmsFieldMap[entry.field as F];
      if (applied.includes(name)) {
        continue;
      }
      form.setError(name, { type: "server", message: entry.message });
      applied.push(name);
    }
  }
  const [first] = applied;
  if (first !== undefined) {
    focusField(form, first);
  }
  return { ...shown, fields: applied };
}

/**
 * Shows the COBOL success text of a completed write with `toast.success`, for
 * example COBIL00C's 'Payment successful.  Your Transaction ID is <id>.'
 * [app/cbl/COBIL00C.cbl:L524-530], which the COBOL writes in green. A success
 * answer without a message shows nothing.
 */
export function showSuccess(message?: string | null): void {
  if (hasText(message)) {
    toast.success(message);
  }
}

/**
 * Shows the message of a 200 answer that wrote nothing, a COBOL prompt rather
 * than an error, as a neutral toast. A blank or absent message shows nothing.
 */
export function showMessage(message?: string | null): void {
  if (hasText(message)) {
    toast(message);
  }
}
