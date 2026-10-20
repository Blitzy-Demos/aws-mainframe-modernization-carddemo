import { z } from "zod";

/**
 * Client-side form schema of the bill-payment screen, map `COBIL0A` of `app/bms/COBIL00.bms`
 * (transaction CB00, program COBIL00C, endpoint 15).
 *
 * It repeats only what the 3270 terminal enforced on the two inputs, their `LENGTH`:
 * - `COBIL00-013` `ACTIDIN`, `POS=(6,21) LENGTH=11 ATTRB=(FSET,IC,NORM,UNPROT)`
 *   (`app/bms/COBIL00.bms:85-89`);
 * - `COBIL00-020` `CONFIRM`, `POS=(15,60) LENGTH=1 ATTRB=(FSET,NORM,UNPROT)`
 *   (`app/bms/COBIL00.bms:115-119`).
 * Neither field is `NUM`, and the balance `CURBAL` is an output field the request never carries.
 *
 * The program's own edits, the blank account (`COBIL00C-007`, `Acct ID can NOT be empty...`)
 * and the confirmation other than `Y`, `y`, `N`, `n` or blank (`COBIL00C-012`,
 * `Invalid value. Valid values are (Y/N)...`), are `server` rows of
 * `modern/audit/online/COBIL00C.md`, so they are deliberately not repeated here: the account
 * edit runs before the confirmation edit and their messages reach the screen from endpoint 15
 * unchanged. Every confirmation value of one character passes, because a blank one asks the
 * server for the balance and `N`/`n` clears the screen. Values are sent as typed.
 */

/** Map `LENGTH` of each input field, for the screen's `Input` `maxLength`. */
export const COBIL00_LENGTHS = {
  acctId: 11,
  confirm: 1,
} as const;

/** The bill-payment form: account ID and confirmation, each at most its map `LENGTH`. */
export const cobil00Schema = z.object({
  acctId: z.string().max(COBIL00_LENGTHS.acctId),
  confirm: z.string().max(COBIL00_LENGTHS.confirm),
});

/** Values of the bill-payment form, exactly as typed. */
export type Cobil00Values = z.infer<typeof cobil00Schema>;
