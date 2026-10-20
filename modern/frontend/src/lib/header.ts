/**
 * Values of the BMS header rows 1 and 2 that every online program moves in its header paragraph
 * (`POPULATE-HEADER-INFO` in `app/cbl/COSGN00C.cbl:177-196` and its counterparts in the other 20
 * programs), and the two common messages of `CSMSG01Y` (AAP 0.3.5, 0.5.4).
 *
 * `ScreenShell` places the titles and the formatted date and time; the header labels, the
 * transaction and program names and COSGN00's row 3 come from each screen's rows in
 * `modern/audit/screens.md`, not from here.
 */

/**
 * `CCDA-TITLE01 PIC X(40)` (`app/cpy/COTTL01Y.cpy:18-19`): the 40-character value of header row 1,
 * field `TITLE01` at `POS=(1,21)`, `LENGTH=40` (`app/bms/COSGN00.bms:38-41`). The padding is part
 * of the value and keeps the title centred in its 40 columns. Audit: `COSGN00-039`, `COSGN00C-012`
 * and the matching title-row-1 row of every map in `screens.md`.
 */
export const TITLE01 = "      AWS Mainframe Modernization       ";

/**
 * `CCDA-TITLE02 PIC X(40)` (`app/cpy/COTTL01Y.cpy:20-22`; line 21 is a commented-out older value
 * and is not used): the 40-character value of header row 2, field `TITLE02` at `POS=(2,21)`,
 * `LENGTH=40` (`app/bms/COSGN00.bms:61-64`). Audit: `COSGN00-042`, `COSGN00C-012` and the matching
 * title-row-2 row of every map in `screens.md`.
 */
export const TITLE02 = "              CardDemo                  ";

/** Width of the two `CSMSG01Y` message fields, `PIC X(50)`. */
const MESSAGE_WIDTH = 50;

/**
 * `CCDA-MSG-THANK-YOU PIC X(50)` (`app/cpy/CSMSG01Y.cpy:18-19`): the session-end text COSGN00C
 * sends as plain text on F3 before it ends the conversation (`app/cbl/COSGN00C.cbl:88-90,162-172`).
 * Audit: `COSGN00C-004`, which records the padded field value. The `VALUE` literal is shorter than
 * the field, so the value is padded with spaces to all 50 bytes, as the `MOVE` transfers it.
 * `CCDA-THANK-YOU` of `COTTL01Y.cpy:23-24` is referenced by no program and is not this text.
 */
export const THANK_YOU = "Thank you for using CardDemo application...".padEnd(MESSAGE_WIDTH, " ");

/**
 * `CCDA-MSG-INVALID-KEY PIC X(50)` (`app/cpy/CSMSG01Y.cpy:20-21`): the error-row message for a
 * function key the program does not serve (`app/cbl/COSGN00C.cbl:91-94`; also COMEN01C, COADM01C,
 * COBIL00C, CORPT00C, COTRN00C–COTRN02C, COUSR00C–COUSR03C, COPAUS0C and COPAUS1C). Audit:
 * `COSGN00C-005`, which records the padded field value; padded with spaces to all 50 bytes, as
 * for `THANK_YOU`. Screens whose audit marks the invalid-key path `client` show it through
 * `usePfKeys`' `onInvalidKey`.
 */
export const INVALID_KEY = "Invalid key pressed. Please see below...".padEnd(MESSAGE_WIDTH, " ");

/** A non-negative integer below 100 as two digits with a leading zero, as a `PIC 9(02)` holds it. */
function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/** Rejects an invalid `Date`, whose getters all return `NaN`, so no `NaN` text reaches a screen. */
function checked(d: Date): Date {
  if (Number.isNaN(d.getTime())) {
    throw new RangeError("Header date and time need a valid Date");
  }
  return d;
}

/**
 * The header date `WS-CURDATE-MM-DD-YY` (`app/cpy/CSDAT01Y.cpy:30-35`): month, day and the last two
 * digits of the year, each two digits, separated by `/`, in local time, as `POPULATE-HEADER-INFO`
 * builds it from `FUNCTION CURRENT-DATE` with `WS-CURDATE-YEAR(3:2)` (`app/cbl/COSGN00C.cbl:179-190`).
 * Example: 18 July 2022 → `07/18/22`. The year is taken without its sign, as the unsigned
 * `WS-CURDATE-YEAR PIC 9(04)` stores it. Audit: `COSGN00-040`, `COSGN00C-012` and the matching
 * `CURDATE` row of every map in `screens.md`.
 *
 * @throws RangeError when `d` is an invalid `Date`.
 */
export function formatHeaderDate(d: Date): string {
  const date = checked(d);
  return [
    pad2(date.getMonth() + 1),
    pad2(date.getDate()),
    pad2(Math.abs(date.getFullYear()) % 100),
  ].join("/");
}

/**
 * The header time `WS-CURTIME-HH-MM-SS` (`app/cpy/CSDAT01Y.cpy:36-41`): hours on the 24-hour
 * clock, minutes and seconds, each two digits, separated by `:`, in local time, as
 * `POPULATE-HEADER-INFO` builds it from `FUNCTION CURRENT-DATE` (`app/cbl/COSGN00C.cbl:179,192-196`).
 * Example: 9:05:07 → `09:05:07`. Audit: `COSGN00-043`, `COSGN00C-012` and the matching `CURTIME`
 * row of every map in `screens.md`.
 *
 * @throws RangeError when `d` is an invalid `Date`.
 */
export function formatHeaderTime(d: Date): string {
  const time = checked(d);
  return [pad2(time.getHours()), pad2(time.getMinutes()), pad2(time.getSeconds())].join(":");
}
