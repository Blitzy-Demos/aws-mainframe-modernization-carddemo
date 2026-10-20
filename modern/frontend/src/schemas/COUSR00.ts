import { z } from "zod";

/**
 * Input lengths of map `COUSR0A` (`app/bms/COUSR00.bms`): the `USRIDIN` search field, each row
 * selector `SEL0001`–`SEL0010`, and the number of list rows the selectors sit on.
 */
export const COUSR00_LENGTHS = { startUserId: 8, selection: 1, rows: 10 } as const;

/**
 * Form schema of the COUSR00 user list (CU00 / COUSR00C, route `/users`).
 *
 * It enforces the map lengths only:
 * - `startUserId` is `USRIDIN` (COUSR00-015), which endpoint 16 revalidates as `start_user_id`.
 *   A blank value browses from the lowest user ID, and COUSR00C edits nothing else in it.
 * - `selections` holds the ten row selectors in row order. They are never sent: which character
 *   selects a row is the screen's navigation rule (COUSR00C-007 to COUSR00C-011), so every
 *   one-character value passes here.
 *
 * COUSR00C has no program edit the screen repeats before sending, so no refinement is added.
 */
export const cousr00Schema = z.object({
  startUserId: z.string().max(COUSR00_LENGTHS.startUserId),
  selections: z
    .array(z.string().max(COUSR00_LENGTHS.selection))
    .length(COUSR00_LENGTHS.rows),
});

/** Form values of the COUSR00 screen. */
export type Cousr00Values = z.infer<typeof cousr00Schema>;
