import { z } from "zod";

/**
 * Client-side form schema of the user delete screen, map `COUSR3A` of `app/bms/COUSR03.bms`
 * (transaction CU03, program COUSR03C, route `/users/delete`).
 *
 * The map has one input, `USRIDIN` (`COUSR03-013`, `POS=(6,21) LENGTH=8`, initial cursor `IC`),
 * which both actions send as the `user_id` path parameter: ENTER (`ENTER=Fetch`,
 * `PROCESS-ENTER-KEY`) to endpoint 18 and F5 (`F5=Delete`, `DELETE-USER-INFO`) to endpoint 20.
 * Its map `LENGTH` carries zod's own message, since the screen audit records none; the field is
 * not `NUM`. First name, last name and user type are protected display fields (`COUSR03-017`,
 * `COUSR03-020`, `COUSR03-023`) filled from endpoint 18, so they are not form values.
 *
 * Program edits: only the two `both` rows of `modern/audit/online/COUSR03C.md`, the blank user
 * ID of the fetch (`COUSR03C-010`, which equals `COUSR02C-010` of the shared read) and of the
 * delete (`COUSR03C-016`). Both paragraphs run the same edit with the same literal, so one
 * schema serves both keys. The read's not-found and lookup texts (`COUSR03C-013`,
 * `COUSR03C-014`) and every delete outcome (`COUSR03C-017` to `COUSR03C-024`) are `server`
 * rows and are deliberately not repeated: the endpoint's `ScreenError` wins on screen. COUSR03C
 * upper-cases nothing, so the user ID passes exactly as typed.
 */

/** Map `LENGTH` of the input field, for the screen's `Input` `maxLength`. */
export const COUSR03_LENGTHS = { userId: 8 } as const;

/**
 * COBOL `= SPACES OR LOW-VALUES` on a typed input: empty or all spaces. Any other character,
 * a leading space before it included, makes the user ID non-blank.
 */
const COBOL_BLANK = /^ *$/;

/** `COUSR03C.cbl:147` and `:179`, moved to `WS-MESSAGE` with the cursor on `USRIDIN`. */
const USER_ID_EMPTY = "User ID can NOT be empty...";

/** ENTER (fetch, endpoint 18) and F5 (delete, endpoint 20) of `COUSR3A`. */
export const cousr03Schema = z
  .object({
    /** `USRIDIN` (`USRIDINI PIC X(8)`), sent as the `user_id` path parameter. */
    userId: z.string().max(COUSR03_LENGTHS.userId),
  })
  .superRefine((values, ctx) => {
    // COUSR03C-010 (ENTER) and COUSR03C-016 (F5): the only edit either key runs before its
    // request; the issue sits on `userId`, as `MOVE -1 TO USRIDINL` places the cursor.
    if (COBOL_BLANK.test(values.userId)) {
      ctx.addIssue({ code: z.ZodIssueCode.custom, path: ["userId"], message: USER_ID_EMPTY });
    }
  });

/** Values of the user delete form, exactly as typed. */
export type Cousr03Values = z.infer<typeof cousr03Schema>;
