import { z } from "zod";

/**
 * Client-side form schemas of the user update screen, map `COUSR2A` of `app/bms/COUSR02.bms`
 * (transaction CU02, program COUSR02C, route `/users/update`).
 *
 * Map `LENGTH` of the five inputs, with zod's default message, since the screen audit records
 * none; no input is `NUM`:
 * - `COUSR02-013` `USRIDIN`, `POS=(6,21) LENGTH=8`, initial cursor (`IC`);
 * - `COUSR02-017` `FNAME`, `POS=(11,18) LENGTH=20`;
 * - `COUSR02-020` `LNAME`, `POS=(11,56) LENGTH=20`;
 * - `COUSR02-023` `PASSWD`, `POS=(13,16) LENGTH=8`, dark (`DRK`);
 * - `COUSR02-026` `USRTYPE`, `POS=(15,17) LENGTH=1`.
 *
 * Program edits: only the two `both` rows of `modern/audit/online/COUSR02C.md`, the blank user
 * ID of the fetch (`COUSR02C-010`, ENTER, endpoint 18) and of the update (`COUSR02C-018`, F5 and
 * the save half of F3, endpoint 19). The update's blank first name, last name, password and user
 * type edits (`COUSR02C-019` to `COUSR02C-022`) are `server` rows, so they are deliberately not
 * repeated here: endpoint 19 applies them in the program's order and its message wins. So do
 * `Please modify to update ...`, which needs the stored row and password hash, and the
 * not-found, lookup and update failure texts. No value edit applies to the user type
 * (`COUSR02C-023`): every non-blank character is stored as typed.
 *
 * Password (E27): the dark field is never pre-filled and never returned, so `password` is
 * optional. The screen sends it only when typed; an omitted password means unchanged, and the
 * server verifies a supplied one against the stored hash for its change check (`COUSR02C-027`).
 * The COBOL `Password can NOT be empty...` edit is the server row `COUSR02C-021` and is never
 * applied here. No message of this file carries a password value.
 */

/** Map `LENGTH` of each input field, for the screen's `Input` `maxLength`. */
export const COUSR02_LENGTHS = {
  userId: 8,
  firstName: 20,
  lastName: 20,
  password: 8,
  userType: 1,
} as const;

/**
 * COBOL `= SPACES OR LOW-VALUES` on a typed input: empty or all spaces, the client half of
 * `COUSR02C-010` and `COUSR02C-018`.
 */
const COBOL_BLANK = /^ *$/;

/** `COUSR02C.cbl:148` and `:182`, moved to `WS-MESSAGE` with the cursor on `USRIDIN`. */
const USER_ID_EMPTY = "User ID can NOT be empty...";

/** The user ID edit both actions run first; it places the issue on `userId`, as `USRIDINL`. */
function editUserId(userId: string, ctx: z.RefinementCtx): void {
  if (COBOL_BLANK.test(userId)) {
    ctx.addIssue({ code: z.ZodIssueCode.custom, path: ["userId"], message: USER_ID_EMPTY });
  }
}

/** ENTER (`ENTER=Fetch`, `PROCESS-ENTER-KEY`): the user ID sent as endpoint 18's path `user_id`. */
export const cousr02KeySchema = z
  .object({
    /** `USRIDIN` (`USRIDINI PIC X(8)`), sent as the `user_id` path parameter. */
    userId: z.string().max(COUSR02_LENGTHS.userId),
  })
  .superRefine((values, ctx) => {
    // COUSR02C-010: the fetch's only edit.
    editUserId(values.userId, ctx);
  });

/** Values of the fetch, exactly as typed. */
export type Cousr02KeyValues = z.infer<typeof cousr02KeySchema>;

/** F5 (`F5=Save`) and F3 (`F3=Save&Exit`, `UPDATE-USER-INFO`): the update of endpoint 19. */
export const cousr02Schema = z
  .object({
    /** `USRIDIN` (`USRIDINI PIC X(8)`), sent as the `user_id` path parameter. */
    userId: z.string().max(COUSR02_LENGTHS.userId),
    /** `FNAME` (`FNAMEI PIC X(20)`), sent as `first_name`. */
    firstName: z.string().max(COUSR02_LENGTHS.firstName),
    /** `LNAME` (`LNAMEI PIC X(20)`), sent as `last_name`. */
    lastName: z.string().max(COUSR02_LENGTHS.lastName),
    /** `PASSWD` (`PASSWDI PIC X(8)`), sent as `password` only when typed (E27). */
    password: z.string().max(COUSR02_LENGTHS.password).optional(),
    /** `USRTYPE` (`USRTYPEI PIC X(1)`), sent as `user_type`. */
    userType: z.string().max(COUSR02_LENGTHS.userType),
  })
  .superRefine((values, ctx) => {
    // COUSR02C-018: the update's first edit; the later ones are server rows.
    editUserId(values.userId, ctx);
  });

/** Values of the update form, exactly as typed. */
export type Cousr02Values = z.infer<typeof cousr02Schema>;
