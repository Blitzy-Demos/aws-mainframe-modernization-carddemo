/**
 * CC00 sign-on form schema: the client mirror of `LoginRequest`, the body of
 * `POST /auth/login`, which carries the `USERIDI` and `PASSWDI` fields of the
 * COSGN00 map [app/cpy-bms/COSGN00.CPY:L72, L78].
 *
 * The server stays authoritative (COSGN00C, docs/parity/validation-audit.md).
 * This schema blocks only input the server rejects, in the order it rejects
 * it:
 *
 * 1. Each field holds at most the 8 characters of its `PIC X(8)` map field.
 *    The server applies this length check before the sign-on rule and answers
 *    it with an empty message, so the issue keeps zod's own text.
 * 2. V-COSGN00C-001, then V-COSGN00C-002: a user ID, then a password, of
 *    `SPACES OR LOW-VALUES` [app/cbl/COSGN00C.cbl:L117-127].
 *
 * Values are never trimmed or upper-cased. COSGN00C upper-cases both after
 * the blank edits [app/cbl/COSGN00C.cbl:L132-136], and a password stored in
 * lower or mixed case must fail sign-on exactly as in the COBOL (E-20).
 */
import { z } from "zod";

import type { components } from "@/api/schema";

type LoginRequest = components["schemas"]["LoginRequest"];
type LoginField = components["schemas"]["LoginField"];

/** The width of the `USERIDI` and `PASSWDI` map fields, `PIC X(8)`. */
const MAP_FIELD_LENGTH = 8;

/** The COSGN00C texts of the edits this schema repeats, byte for byte. */
export const LOGIN_MESSAGES = {
  // V-COSGN00C-001
  userIdEmpty: "Please enter User ID ...",
  // V-COSGN00C-002
  passwordEmpty: "Please enter Password ...",
} as const;

/**
 * `<field> = SPACES OR LOW-VALUES` as the server's sign-on rule tests it:
 * every character a space, or every character NUL; the empty text is both.
 * A mix of the two, a tab or any other whitespace is not blank to the COBOL,
 * so `String.prototype.trim` is not used: it would block input the server
 * reads and answers with 'User not found. Try again ...'.
 */
function isSpacesOrLowValues(value: string): boolean {
  return /^ *$/.test(value) || /^\u0000*$/.test(value);
}

/** One map field: the length check, then the field's COSGN00C blank edit. */
function mapField(emptyMessage: string) {
  return z
    .string()
    .max(MAP_FIELD_LENGTH)
    .refine((value) => !isSpacesOrLowValues(value), {
      message: emptyMessage,
    });
}

/**
 * The sign-on form. Keys follow the audit `Order` (USERID 1, PASSWD 2), so
 * zod reports the user ID's issues before the password's, as COSGN00C tests
 * them.
 */
export const loginSchema = z.object({
  user_id: mapField(LOGIN_MESSAGES.userIdEmpty),
  password: mapField(LOGIN_MESSAGES.passwordEmpty),
}) satisfies z.ZodType<LoginRequest>;

export type LoginFormValues = z.infer<typeof loginSchema>;

/** The form field of each BMS field a `POST /auth/login` error names. */
export const loginBmsFieldMap: Record<LoginField, keyof LoginFormValues> = {
  USERID: "user_id",
  PASSWD: "password",
};
