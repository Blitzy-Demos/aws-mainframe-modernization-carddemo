import { z } from "zod";

/**
 * Form schema of map `COCRDLI` (`CCRDLIA`), the card list served by COCRDLIC (transaction
 * `CCLI`, endpoint 7, route `/cards`).
 *
 * It holds the client half of the map's `both` rows only: the `LENGTH` of the two filters
 * (COCRDLI-015, COCRDLI-018) and COCRDLIC's filter edits (COCRDLIC-006, COCRDLIC-008,
 * COCRDLIC-009). The server revalidates every field and owns every message; a `ScreenError`
 * it returns replaces what this schema reports. The map declares no `NUM` field.
 *
 * The seven row selectors carry their map `LENGTH` only. Their edits (one selection at most,
 * `S` or `U` only) are `client` rows that the screen applies, because a selection only
 * navigates and no request carries it.
 */

/** Map `LENGTH` of each input field (`app/bms/COCRDLI.bms`) and the number of list rows. */
export const COCRDLI_LENGTHS = {
  acctId: 11,
  cardNum: 16,
  selection: 1,
  rows: 7,
} as const;

const ACCOUNT_FILTER_MESSAGE = "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER";
const CARD_FILTER_MESSAGE = "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER";

/** COBOL `EQUAL SPACES` on a field the terminal pads with blanks: empty or spaces only. */
const BLANK = /^ *$/;

const DIGITS = /^[0-9]*$/;

/**
 * Whether a filter fails COCRDLIC's edit (`2210-EDIT-ACCOUNT`, `2220-EDIT-CARD`): supplied,
 * yet not `width` digits. `IS NUMERIC` on `X(width)` needs every position to be a digit, and the
 * terminal pads a shorter entry with blanks, so a short entry fails too. All zeros is numeric and
 * passes; the server then lists the cards with no such filter (COCRDLIC-005, COCRDLIC-007).
 *
 * HALTED(X-COCRDLIC-H2): the program compares the zoned-decimal redefinition with zero before the
 * class test, which IBM leaves unpredictable for non-numeric data. This is the program's evident
 * path, the class test, and is not a parity result.
 *
 * An entry longer than the field cannot reach the program; `.max` already rejects it, so the edit
 * leaves it alone and the field reports one issue.
 */
function filterInError(value: string, width: number): boolean {
  if (value.length > width || BLANK.test(value)) {
    return false;
  }
  return value.length !== width || !DIGITS.test(value);
}

/** The `COCRDLI` form: map lengths, then COCRDLIC's filter edits in program order. */
export const cocrdliSchema = z
  .object({
    acctId: z.string().max(COCRDLI_LENGTHS.acctId),
    cardNum: z.string().max(COCRDLI_LENGTHS.cardNum),
    selections: z.array(z.string().max(COCRDLI_LENGTHS.selection)).length(COCRDLI_LENGTHS.rows),
  })
  .superRefine((values, ctx) => {
    // Both edits run, account first, and each marks its own filter in error (COCRDLIC-009,
    // COCRDLIC-011), so an issue is added per failing field rather than only the first. The
    // card edit sets the error line only while it is empty (`IF WS-ERROR-MSG-OFF`), so each
    // issue carries the error line as it stands after its edit: with both filters in error,
    // both issues carry the account message, which the first issue puts on the error line.
    let errorLine: string | undefined;
    if (filterInError(values.acctId, COCRDLI_LENGTHS.acctId)) {
      errorLine = ACCOUNT_FILTER_MESSAGE;
      ctx.addIssue({ code: z.ZodIssueCode.custom, path: ["acctId"], message: errorLine });
    }
    if (filterInError(values.cardNum, COCRDLI_LENGTHS.cardNum)) {
      errorLine ??= CARD_FILTER_MESSAGE;
      ctx.addIssue({ code: z.ZodIssueCode.custom, path: ["cardNum"], message: errorLine });
    }
  });

/** Values of the `COCRDLI` form: the two filters as typed and the seven selection codes. */
export type CocrdliValues = z.infer<typeof cocrdliSchema>;
