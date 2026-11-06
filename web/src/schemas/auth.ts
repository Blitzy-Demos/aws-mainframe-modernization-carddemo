/**
 * CC00 sign-on form schema: the client mirror of `LoginRequest`, the body of
 * `POST /auth/login`, which carries the `USERIDI` and `PASSWDI` fields of the
 * COSGN00 map [app/cpy-bms/COSGN00.CPY:L72, L78].
 *
 * The server stays authoritative (COSGN00C, docs/parity/validation-audit.md).
 * This schema blocks only input the server rejects, in the order it rejects
 * it:
 *
 * 1. The lengths of both fields, the user ID's then the password's: each
 *    holds at most the 8 characters of its `PIC X(8)` map field. The server's
 *    `LoginRequest::check` tests both before the sign-on rule runs, names
 *    `USERID` then `PASSWD` for each field that is too long, and answers with
 *    an empty message, so each issue keeps zod's own text.
 * 2. Only when both lengths pass, V-COSGN00C-001, then V-COSGN00C-002: a user
 *    ID, then a password, of `SPACES OR LOW-VALUES`
 *    [app/cbl/COSGN00C.cbl:L117-127]. The server's sign-on rule answers the
 *    first of them alone; when both fields are blank the schema reports both,
 *    the user ID's first, so its first issue is still the server's answer.
 *
 * Values are never trimmed or upper-cased. COSGN00C upper-cases both after
 * the blank edits [app/cbl/COSGN00C.cbl:L132-136], and a password stored in
 * lower or mixed case must fail sign-on exactly as in the COBOL (E-20).
 */
import { z } from "zod";

import type { components } from "@/api/schema";

import { isSpacesOrLowValues } from "./primitives";

type LoginRequest = components["schemas"]["LoginRequest"];
type LoginField = components["schemas"]["LoginField"];

// The width of the `USERIDI` and `PASSWDI` map fields, `PIC X(8)`.
const MAP_FIELD_LENGTH = 8;

/** The COSGN00C texts of the edits this schema repeats, byte for byte. */
export const LOGIN_MESSAGES = {
  // V-COSGN00C-001
  userIdEmpty: "Please enter User ID ...",
  // V-COSGN00C-002
  passwordEmpty: "Please enter Password ...",
} as const;

/**
 * The sign-on form. Keys follow the audit `Order` (USERID 1, PASSWD 2), so
 * zod reports the user ID's length before the password's, as the server's
 * `LoginRequest::check` lists them. The blank edits are object-level and wait
 * on both lengths, because the server answers an over-long field alone,
 * before its sign-on rule tests either field: an over-long password stops the
 * user ID's blank edit, and an over-long user ID the password's. Blank is
 * `SPACES OR LOW-VALUES` as that rule tests it. The values are not trimmed: a
 * tab or other whitespace is not blank to the COBOL, and the server reads it
 * and answers 'User not found. Try again ...'.
 */
export const loginSchema = z
  .object({
    user_id: z.string().max(MAP_FIELD_LENGTH),
    password: z.string().max(MAP_FIELD_LENGTH),
  })
  .superRefine((values, ctx) => {
    if (
      values.user_id.length > MAP_FIELD_LENGTH ||
      values.password.length > MAP_FIELD_LENGTH
    ) {
      return;
    }
    if (isSpacesOrLowValues(values.user_id)) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["user_id"],
        message: LOGIN_MESSAGES.userIdEmpty,
      });
    }
    if (isSpacesOrLowValues(values.password)) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["password"],
        message: LOGIN_MESSAGES.passwordEmpty,
      });
    }
  }) satisfies z.ZodType<LoginRequest>;

export type LoginFormValues = z.infer<typeof loginSchema>;

/** The form field of each BMS field a `POST /auth/login` error names. */
export const loginBmsFieldMap: Record<LoginField, keyof LoginFormValues> = {
  USERID: "user_id",
  PASSWD: "password",
};
