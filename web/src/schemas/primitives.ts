/**
 * Input rules several form schemas share, defined once so they cannot drift
 * apart. Each schema keeps its own field widths, padding, wrappers and audited
 * texts around them.
 */

/**
 * COBOL `<field> = SPACES OR LOW-VALUES` of received map text, as the server
 * reads it: every character a space, or every character NUL; the empty text
 * is both. A mix of spaces and NUL is not blank, and neither is a tab, a
 * no-break space or any other whitespace, so the text is never trimmed.
 */
export function isSpacesOrLowValues(value: string): boolean {
  return /^ *$/.test(value) || /^\u0000*$/.test(value);
}

/**
 * The CSUTLDTC picture `YYYY-MM-DD` that the date text inputs ask for, as
 * there is no date picker: four digits, a hyphen, two digits, a hyphen, two
 * digits. Whether such a text is a calendar date stays the server's CSUTLDTC
 * check.
 */
export const DATE_PATTERN = /^\d{4}-\d{2}-\d{2}$/;
