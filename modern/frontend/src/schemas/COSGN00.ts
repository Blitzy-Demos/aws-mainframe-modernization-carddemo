import { z } from "zod";

/**
 * Client-side form schema of the sign-on screen, map `COSGN0A` of `app/bms/COSGN00.bms`
 * (transaction CC00, program COSGN00C).
 *
 * It repeats only what the 3270 terminal enforced on the two inputs, their `LENGTH`:
 * - `COSGN00-027` `USERID`, `POS=(19,43) LENGTH=8` (`app/bms/COSGN00.bms:156-160`);
 * - `COSGN00-031` `PASSWD`, `POS=(20,43) LENGTH=8`, dark (`app/bms/COSGN00.bms:175-180`).
 *
 * The blank user ID and blank password edits (`COSGN00C-007`, `COSGN00C-008`) are server rows
 * of `modern/audit/online/COSGN00C.md`, so they are deliberately not repeated here: their
 * messages reach the screen from endpoint 1 unchanged. Values are sent as typed; upper-casing
 * and padding to 8 bytes are server rules (E26).
 */

/** Map `LENGTH` of each input field, for the screen's `Input` `maxLength`. */
export const COSGN00_LENGTHS = {
  userId: 8,
  password: 8,
} as const;

/** The sign-on form: user ID and password, each at most its map `LENGTH`. */
export const cosgn00Schema = z.object({
  userId: z.string().max(COSGN00_LENGTHS.userId),
  password: z.string().max(COSGN00_LENGTHS.password),
});

/** Values of the sign-on form, exactly as typed. */
export type Cosgn00Values = z.infer<typeof cosgn00Schema>;
