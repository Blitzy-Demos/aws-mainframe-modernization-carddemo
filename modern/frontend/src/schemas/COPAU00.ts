import { z } from "zod";

/**
 * Client-side form schema of the authorization-summary screen: map `COPAU0A` of
 * `app/app-authorization-ims-db2-mq/bms/COPAU00.bms` (transaction `CPVS`, program `COPAUS0C`,
 * route `/authorizations`).
 *
 * It is the ENTER edit, the only key whose input COPAUS0C edits (`PROCESS-ENTER-KEY`); F7 and F8
 * page the account held in the screen's state and do not read the field. It repeats the map
 * `LENGTH` of the inputs and the account edits audited as Layer `both`, in program order:
 * - `COPAU00-013` `ACCTID`, `POS=(5,19) LENGTH=11`, no `NUM` (`COPAU00.bms:84-88`);
 * - `COPAUS0C-005`, a blank account (`COPAUS0C.cbl:264-271`);
 * - `COPAUS0C-006`, an account that is not 11 digits (`COPAUS0C.cbl:272-280`).
 *
 * The selectors only choose a route and are never sent (Layer `client`), so the selection rules
 * (`COPAUS0C-009` to `COPAUS0C-012`), the invalid-selection message among them, belong to the
 * screen and are not repeated here. Endpoint 21 re-validates `acct_id` and owns every message;
 * a server `ScreenError` replaces the message shown from here.
 */

/** Map `LENGTH` of each input field, and the number of list rows (screen lines 16 to 20). */
export const COPAU00_LENGTHS = {
  /** `ACCTID`, `POS=(5,19) LENGTH=11` (`COPAU00.bms:84-88`). */
  acctId: 11,
  /** `SEL0001` to `SEL0005`, `LENGTH=1` in column 3 (`COPAU00.bms:277-493`). */
  selection: 1,
  rows: 5,
} as const;

/**
 * COBOL `ACCTIDI OF COPAU0AI = SPACES OR LOW-VALUES`: empty, all spaces, or all low-values.
 * The low-values alternative follows the audited condition of COPAUS0C-005 rather than a
 * spaces-only test, so a field the COBOL calls blank never gets the not-numeric message instead.
 */
const BLANK_ACCT_ID = /^(?: *|\u0000+)$/;

/** COBOL `ACCTIDI OF COPAU0AI IS NUMERIC` on `PIC X(11)`: all 11 positions ASCII digits. */
const NUMERIC_ACCT_ID = /^[0-9]{11}$/;

/** `COPAUS0C-005`, `COPAUS0C.cbl:268-269`. */
const MSG_ENTER_ACCT_ID = "Please enter Acct Id...";

/** `COPAUS0C-006`, `COPAUS0C.cbl:277-278`; the space before `...` is part of the literal. */
const MSG_ACCT_ID_NOT_NUMERIC = "Acct Id must be Numeric ...";

export const copau00Schema = z
  .object({
    // Screen rule COPAU00-013, Layer `both`: the map length, which endpoint 21 revalidates on its
    // `acct_id` path parameter. screens.md records no message for it, so zod's default stands.
    acctId: z.string().max(COPAU00_LENGTHS.acctId),
    // Screen rules COPAU00-057, -066, -075, -084 and -100, Layer `client`: the selectors are never
    // sent. Any character is accepted, because which code selects (`S` or `s`) and the
    // invalid-selection message are the screen's rules (COPAUS0C-009 to COPAUS0C-012).
    selections: z
      .array(z.string().max(COPAU00_LENGTHS.selection))
      .length(COPAU00_LENGTHS.rows),
  })
  .superRefine(({ acctId }, ctx) => {
    // COPAUS0C-005 then COPAUS0C-006, as `PROCESS-ENTER-KEY` tests them: the first that fails is
    // the only message, and the selectors are not looked at when the account is rejected.
    if (BLANK_ACCT_ID.test(acctId)) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["acctId"],
        message: MSG_ENTER_ACCT_ID,
      });
    } else if (!NUMERIC_ACCT_ID.test(acctId)) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["acctId"],
        message: MSG_ACCT_ID_NOT_NUMERIC,
      });
    }
  });

/** Values of the COPAU00 form, exactly as typed. */
export type Copau00Values = z.infer<typeof copau00Schema>;
