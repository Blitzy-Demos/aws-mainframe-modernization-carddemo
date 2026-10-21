import { z } from "zod";

/**
 * Client-side form schema of the admin menu, map `COADM1A` of `app/bms/COADM01.bms`
 * (transaction CA00, program COADM01C, route `/admin`).
 *
 * It repeats only what the 3270 terminal enforced on the map's one input, `COADM01-025`
 * `OPTION`, `POS=(20,41) LENGTH=2 ATTRB=(FSET,IC,NORM,NUM,UNPROT) JUSTIFY=(RIGHT,ZERO)`
 * (`app/bms/COADM01.bms:145-149`): at most 2 characters, digits only. screens.md records the
 * field as digits only and records no message for it, so a space fails like any other non-digit
 * and both issues carry zod's own messages.
 *
 * The option is never sent: ENTER is served from the endpoint 3 `MenuResponse` the screen already
 * holds. COADM01C's edits are therefore `client` rows of `modern/audit/online/COADM01C.md`, and
 * none of them is repeated here:
 * - the right-justify and zero-fill of the entry (`COADM01C-015`, `COADM01C.cbl:121-129`);
 * - the invalid-option edit, `'Please enter a valid option number...'` for `00` or above the
 *   option count (`COADM01C-016`, `COADM01C.cbl:131-137`), which needs the response's table;
 * - the not-installed message (`COADM01C-018`, `COADM01C-019`), which needs its installed flags.
 * So an empty value and any two digits pass, and the value is returned exactly as typed.
 */

/** Map `LENGTH` of the input field, for the screen's `Input` `maxLength`. */
export const COADM01_LENGTHS = { option: 2 } as const;

/** `NUM`: the numeric lock accepts digits only; an empty field is a valid entry. */
const DIGITS_ONLY = /^[0-9]*$/;

/** The admin menu form: the option number, at most its map `LENGTH` and digits only. */
export const coadm01Schema = z.object({
  option: z.string().max(COADM01_LENGTHS.option).regex(DIGITS_ONLY),
});

/** Values of the admin menu form, exactly as typed. */
export type Coadm01Values = z.infer<typeof coadm01Schema>;
