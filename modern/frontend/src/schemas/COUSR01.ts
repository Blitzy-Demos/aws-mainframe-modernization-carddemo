import { z } from "zod";

/**
 * Map `LENGTH` of each input field of `COUSR1A` (`app/bms/COUSR01.bms:84-145`): First Name
 * `FNAME` (COUSR01-013), Last Name `LNAME` (COUSR01-016), User ID `USERID` (COUSR01-019),
 * Password `PASSWD` (COUSR01-022) and User Type `USRTYPE` (COUSR01-025). The screen passes each
 * value as its `Input` `maxLength`.
 */
export const COUSR01_LENGTHS = {
  firstName: 20,
  lastName: 20,
  userId: 8,
  password: 8,
  userType: 1,
} as const;

/**
 * Client form schema of the Add User screen (`COUSR01`, route `/users/add`, endpoint 17).
 *
 * It enforces only the map `LENGTH` the terminal enforced, with zod's own message: those are the
 * map's only `both` rows. COUSR01C's empty-field edits (COUSR01C-010 to COUSR01C-014) are
 * `server` rows, so they are deliberately absent here: the COBOL shows only the first failing
 * edit, and every empty field must reach the server so that its message is the program's. For
 * the same reason the user type is not narrowed to `A` or `U` (COUSR01C-015), and no value is
 * trimmed or case-changed: the password and user ID are stored exactly as typed (E26,
 * COUSR01C-006).
 */
export const cousr01Schema = z.object({
  firstName: z.string().max(COUSR01_LENGTHS.firstName),
  lastName: z.string().max(COUSR01_LENGTHS.lastName),
  userId: z.string().max(COUSR01_LENGTHS.userId),
  password: z.string().max(COUSR01_LENGTHS.password),
  userType: z.string().max(COUSR01_LENGTHS.userType),
});

/** The Add User form's values, as typed on the screen. */
export type Cousr01Values = z.infer<typeof cousr01Schema>;
