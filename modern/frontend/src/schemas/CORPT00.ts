import { z } from "zod";

/**
 * Client-side form schema of the transaction reports screen, map `CORPT0A` of
 * `app/bms/CORPT00.bms` (transaction CR00, program CORPT00C, endpoint 14, route `/reports`).
 *
 * It repeats only what the 3270 terminal enforced on the ten inputs, the `both` rows of
 * `modern/audit/screens.md`:
 * - `CORPT00-012` `MONTHLY`, `POS=(7,10) LENGTH=1`, the initial cursor (`CORPT00.bms:80-85`);
 * - `CORPT00-015` `YEARLY`, `POS=(9,10) LENGTH=1` (`CORPT00.bms:94-99`);
 * - `CORPT00-018` `CUSTOM`, `POS=(11,10) LENGTH=1` (`CORPT00.bms:108-113`);
 * - `CORPT00-022`, `CORPT00-024`, `CORPT00-026` `SDTMM`, `SDTDD`, `SDTYYYY`, `LENGTH` 2, 2 and 4,
 *   each `NUM` (`CORPT00.bms:127-154`);
 * - `CORPT00-030`, `CORPT00-032`, `CORPT00-034` `EDTMM`, `EDTDD`, `EDTYYYY`, `LENGTH` 2, 2 and 4,
 *   each `NUM` (`CORPT00.bms:166-193`);
 * - `CORPT00-038` `CONFIRM`, `POS=(19,66) LENGTH=1` (`CORPT00.bms:206-210`).
 * screens.md records no message for these rows, and the server re-validates the same two
 * attributes and owns every message, so each issue carries the text endpoint 14 answers the same
 * condition with: {@link FIELD_LENGTH_MESSAGE} for a value longer than its field and
 * {@link FIELD_NUMERIC_MESSAGE} for a date part that is not digits. The request DTO alone also
 * refuses text its `PIC X(n)` field cannot hold, outside printable ASCII, with the field-length
 * text; that is no map attribute, so such text in a date part is refused here as not digits.
 *
 * CORPT00C's edits are deliberately absent: `modern/audit/online/CORPT00C.md` places every one of
 * them on the `server` layer, so the request must reach endpoint 14 for the program's own text to
 * answer it. They are the report selection and `Select a report type to print report...`
 * (`CORPT00C-007`, `CORPT00C-029`), the six `... can NOT be empty...` edits of a custom request
 * (`CORPT00C-011` to `CORPT00C-016`), the month and day range edits (`CORPT00C-018` to
 * `CORPT00C-023`) and the confirmation prompts (`CORPT00C-030`, `CORPT00C-033`). The server
 * reports the first that fails, and its `ScreenError` replaces the message shown from here.
 *
 * The date parts stay raw text, the raw-edit-input exception to Typed Dates and Timestamps: which
 * custom dates are valid is CSUTLDTC's verdict on the server, halt H5 (`CORPT00C-027`), so no
 * part is checked against a calendar here, and the Monthly and Yearly windows come from the
 * server's clock. Values are sent as typed; the server pads and normalises them (`CORPT00C-017`).
 */

/** Map `LENGTH` of each input field, for the screen's `Input` `maxLength`. */
export const CORPT00_LENGTHS = {
  monthly: 1,
  yearly: 1,
  custom: 1,
  startMonth: 2,
  startDay: 2,
  startYear: 4,
  endMonth: 2,
  endDay: 2,
  endYear: 4,
  confirm: 1,
} as const;

/**
 * The text endpoint 14 answers a value longer than its map field with: `FIELD_LENGTH_MESSAGE` of
 * `crates/api/src/dto/mod.rs`, repeated here because the server owns the message.
 */
const FIELD_LENGTH_MESSAGE = "Input does not fit the screen field...";

/**
 * The text endpoint 14 answers a date part that is not digits with: `FIELD_NUMERIC_MESSAGE` of
 * `crates/api/src/dto/mod.rs`, repeated here because the server owns the message.
 */
const FIELD_NUMERIC_MESSAGE = "Only digits can be entered in this field...";

/**
 * The map's `NUM` attribute as the request DTO revalidates it (`numeric` in
 * `crates/api/src/dto/mod.rs`, X-CORPT00C-5): digits with any spaces before and after them, which
 * NUMVAL-C reads as the value of the digits (`CORPT00C-017`). An empty or all-space part passes,
 * so the server's blank-part edits still answer it; a space between two digits, a sign, a
 * decimal point or a letter does not.
 */
const DIGITS = /^ *[0-9]* *$/;

/** One `NUM` date part of the map: at most `length` characters, digits with spaces around them. */
function datePart(length: number) {
  return z.string().max(length, FIELD_LENGTH_MESSAGE).regex(DIGITS, FIELD_NUMERIC_MESSAGE);
}

/** One field of the map without `NUM`: at most `length` characters. */
function textField(length: number) {
  return z.string().max(length, FIELD_LENGTH_MESSAGE);
}

/**
 * The report form: the three report selectors, the start and end date parts and the
 * confirmation, in map order, each at most its map `LENGTH`.
 */
export const corpt00Schema = z.object({
  monthly: textField(CORPT00_LENGTHS.monthly),
  yearly: textField(CORPT00_LENGTHS.yearly),
  custom: textField(CORPT00_LENGTHS.custom),
  startMonth: datePart(CORPT00_LENGTHS.startMonth),
  startDay: datePart(CORPT00_LENGTHS.startDay),
  startYear: datePart(CORPT00_LENGTHS.startYear),
  endMonth: datePart(CORPT00_LENGTHS.endMonth),
  endDay: datePart(CORPT00_LENGTHS.endDay),
  endYear: datePart(CORPT00_LENGTHS.endYear),
  confirm: textField(CORPT00_LENGTHS.confirm),
});

/** Values of the report form, exactly as typed. */
export type Corpt00Values = z.infer<typeof corpt00Schema>;
