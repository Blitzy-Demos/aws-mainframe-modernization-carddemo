import { z } from "zod";

/**
 * Client-side form schema of the main-menu screen: map `COMEN1A` of `app/bms/COMEN01.bms`
 * (transaction `CM00`, program `COMEN01C`, route `/menu`).
 *
 * The map has one input, `OPTION` (`OPTIONI PIC X(2)` in `app/cpy-bms/COMEN01.CPY`), audited as
 * screens.md `COMEN01-025`: `POS=(20,41) LENGTH=2 ATTRB=(FSET,IC,NORM,NUM,UNPROT)`
 * (`app/bms/COMEN01.bms:145-149`). The schema repeats only what the terminal enforced on it, its
 * `LENGTH` and `NUM`; screens.md records no message for either, so zod's defaults stand.
 *
 * The option is never sent: ENTER is decided on the screen from the endpoint 2 `MenuResponse`
 * already loaded, and online/COMEN01C.md has no `both` row. COMEN01C's ENTER edits are `client`
 * rows that `screens/COMEN01.tsx` applies with the response's texts, so none is repeated here:
 * the right-justified zero fill (`COMEN01C-008`), the invalid option, `0`/`00` and empty input
 * included (`COMEN01C-009`, `COMEN01C-010`), the admin-only refusal (`COMEN01C-012`) and the
 * not-installed text (`COMEN01C-016`). An empty entry and every numeric entry, out of range or
 * not, therefore pass, and the value is returned exactly as typed.
 */

/** Map `LENGTH` of each input field, for the screen's `Input` `maxLength`. */
export const COMEN01_LENGTHS = {
  /** `OPTION`, `POS=(20,41) LENGTH=2` (`app/bms/COMEN01.bms:145-149`). */
  option: 2,
} as const;

/**
 * `NUM`: digits only (`COMEN01-025`), as the screen's numeric field accepts them. Empty passes,
 * because COMEN01C answers an empty option itself (`COMEN01C-009`). Spaces are not accepted:
 * screens.md records the field as digits only, and the blank handling of `COMEN01C-008` applies
 * to what the terminal delivered, which the screen reproduces when it normalises the entry.
 */
const DIGITS_ONLY = /^[0-9]*$/;

/** The main-menu form: the option number, at most its map `LENGTH`, digits only. */
export const comen01Schema = z.object({
  option: z.string().max(COMEN01_LENGTHS.option).regex(DIGITS_ONLY),
});

/** Values of the main-menu form, exactly as typed. */
export type Comen01Values = z.infer<typeof comen01Schema>;
