// The application's single display of API outcomes (shared component "Web
// error display"). It turns the 3270 message line, ERRMSG [app/cpy-bms/
// COSGN00.CPY:L84], into Sonner toasts, and the COBOL cursor placement,
// `MOVE -1 TO <field>L` [app/cbl/COSGN00C.cbl:L121, L126]
// [app/cbl/COBIL00C.cbl:L115, L239], into react-hook-form field errors and
// focus. Pages call its functions and never themselves toast an API outcome,
// set the server's `fields[]` errors on a form or move focus to them. Focus a
// page places on its own, such as its initial field or the field after a
// reset, and the client-side validation of its input through its schema stay
// with the page.
//
// - A form page's mutation callbacks pass showSuccess the answer's message,
//   and pass applyApiError the error, the form and its schema module's
//   bmsFieldMap.
// - A page without a form (lists, detail views, guards) calls showApiError.
// - A 200 answer that only carries a message, writing nothing (CB00's blank
//   confirm 'Confirm to make a bill payment...' [app/cbl/COBIL00C.cbl:L237],
//   COUSR02C 'Please modify to update ...' [app/cbl/COUSR02C.cbl:L239]), goes
//   to showMessage.
// - A 401 that ended the session (ApiError.endedSession) is shown by the
//   guard that sends the user to the sign-on page, and may also be shown by a
//   caller still awaiting that answer. Its toast carries one fixed id, so
//   Sonner updates the shown toast instead of adding a second one.
// - A request past the client's deadline shows the client's own text, 'No
//   answer from the server within 30 seconds' (RequestTimeoutError).
// - A request that got no usable answer (noUsableAnswer) leaves a write's
//   outcome unknown: the server may still carry it out, and sending it again
//   may repeat it. A caller whose request may write passes an
//   `unknownOutcome` note in the options of applyApiError or showApiError
//   (ApiErrorOptions), which the toast shows under the message and keeps on
//   screen until the user dismisses it.
//
//   A 4xx or 5xx error body is the server's answer, so its text is shown
//   alone, as a plain `toast.error(message)` like every other error.
//
// Every text is shown exactly as the server sent it, never trimmed, rewritten
// or prefixed: it is byte-identical to the audited COBOL message, and the
// trailing padding some of them carry collapses in HTML. The error `code` and
// the HTTP status are never shown, nor any detail of a failed request beyond
// the deadline text. Every function is synchronous and uses no React hook, so
// TanStack Query callbacks and handleSubmit handlers call them directly.
import { toast } from "sonner";
import type { FieldValues, Path, UseFormReturn } from "react-hook-form";

import {
  ApiError,
  RequestTimeoutError,
  UNEXPECTED_ERROR_BODY,
} from "@/api/client";

/**
 * The text shown when a failure carries no usable server message: no answer
 * arrived (a network failure, an abort), the answer had no error body, or its
 * message was blank. It is the server's own text for a 500 without an audited
 * COBOL message (crates/domain/src/messages.rs), which the API client also
 * uses for an unusable body. A request past the client's deadline shows the
 * RequestTimeoutError text instead.
 */
export const UNEXPECTED_ERROR: string = UNEXPECTED_ERROR_BODY.message;

/** How showApiError and applyApiError display a failure. */
export type ApiErrorOptions = {
  /**
   * For a caller whose request may write: the note shown under the message
   * when the request got no usable answer (noUsableAnswer), so whether the
   * server carried the write out is unknown. Its toast stays until the user
   * dismisses it. Absent or blank, nothing is added.
   */
  unknownOutcome?: string;
};

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

// The id of the toast for a 401 that ended the session; see the header.
const SESSION_ENDED_TOAST_ID = "session-ended";

/**
 * Whether a failed request got no usable answer from the server: none
 * arrived in time or at all (a RequestTimeoutError, a network failure), or a
 * success status came with a body that could not be read (a JSON parse
 * error) or was absent (an ApiError with a 2xx status). Whether a write it
 * carried was applied is then unknown. An ApiError with a 4xx or 5xx status
 * is the server's answer and does not count.
 */
export function noUsableAnswer(error: unknown): boolean {
  if (!(error instanceof ApiError)) {
    return true;
  }
  return error.status >= 200 && error.status < 300;
}

// The text a failure is shown with; see showApiError.
function messageOf(error: unknown): string {
  if (error instanceof ApiError) {
    return hasText(error.body.message) ? error.body.message : UNEXPECTED_ERROR;
  }
  if (error instanceof RequestTimeoutError) {
    return error.message;
  }
  return UNEXPECTED_ERROR;
}

/**
 * Shows a failed request's message with `toast.error`: the server's message
 * for an ApiError, the client's deadline text for a RequestTimeoutError, and
 * otherwise UNEXPECTED_ERROR. With `options.unknownOutcome`, a failure without
 * a usable answer (noUsableAnswer) shows that note under the message, and its
 * toast stays until the user dismisses it. The error of a 401 that ended the
 * session is toasted under one fixed id, so it is shown once however many
 * callers show it.
 */
export function showApiError(
  error: unknown,
  options: ApiErrorOptions = {},
): ShownApiError {
  const apiError = error instanceof ApiError ? error : null;
  const message = messageOf(error);
  const { unknownOutcome } = options;
  if (apiError?.endedSession === true) {
    toast.error(message, { id: SESSION_ENDED_TOAST_ID });
  } else if (hasText(unknownOutcome) && noUsableAnswer(error)) {
    toast.error(message, { description: unknownOutcome, duration: Infinity });
  } else {
    toast.error(message);
  }
  return { status: apiError?.status ?? null, message };
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
 * transforms its values passes as well. `options` reach the toast as for
 * showApiError.
 */
export function applyApiError<T extends FieldValues, F extends string>(
  error: unknown,
  form: Pick<UseFormReturn<T>, "setError" | "setFocus">,
  bmsFieldMap: BmsFieldMap<F, T>,
  options: ApiErrorOptions = {},
): AppliedApiError<T> {
  const shown = showApiError(error, options);
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
