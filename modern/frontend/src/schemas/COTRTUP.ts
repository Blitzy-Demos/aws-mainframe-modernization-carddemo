import { z } from "zod";

/**
 * Client-side form schemas of the transaction type maintenance screen, map `CTRTUPA` of
 * `app/app-transaction-type-db2/bms/COTRTUP.bms` (transaction CTTU, program COTRTUPC).
 *
 * They repeat only what `modern/audit/screens.md` and `modern/audit/online/COTRTUPC.md` mark
 * as layer `both`:
 * - `COTRTUP-013` `TRTYPCD`, `POS=(12,26) LENGTH=2 ATTRB=(IC,UNPROT)` (`COTRTUP.bms:84-87`);
 * - `COTRTUP-016` `TRTYDSC`, `POS=(14,26) LENGTH=50 ATTRB=(UNPROT)` (`COTRTUP.bms:94-97`);
 * - `COTRTUPC-005`, a blank or `*` type code, rejected with `Tran Type code must be supplied.`
 *   before a request is made, so an empty `{type_code}` path segment is never sent.
 *
 * The numeric and zero type-code edits (`COTRTUPC-006` to `COTRTUPC-008`), the no-change test
 * (`COTRTUPC-017`) and the description edits (`COTRTUPC-018`, `COTRTUPC-019`) are `server` rows
 * and are deliberately not repeated: their `TEST-NUMVAL`/`NUMVAL` and `TRIM` semantics, and their
 * messages, belong to endpoints 26 and 27. Values are sent as typed; trimming and normalising the
 * code and description are server rules (`COTRTUPC-004`, `COTRTUPC-023`).
 */

/** Map `LENGTH` of each input field, for the screen's `Input` `maxLength`. */
export const COTRTUP_LENGTHS = {
  typeCode: 2,
  description: 50,
} as const;

/**
 * `COTRTUPC-005`: `1245-EDIT-NUM-REQD` strings `FUNCTION TRIM(WS-EDIT-VARIABLE-NAME)`, here
 * `'Tran Type code'` (`COTRTUPC.cbl:826`), with `' must be supplied.'` (`COTRTUPC.cbl:922-927`).
 */
const TYPE_CODE_NOT_SUPPLIED = "Tran Type code must be supplied.";

/** COBOL `= SPACES`: an empty or all-space field. */
const COBOL_BLANK = /^ *$/;

/**
 * COBOL `= '*'`: the shorter operand is padded with spaces before comparing, so `*` followed only
 * by spaces equals the literal. A leading space (` *`) does not, and reaches the server's numeric
 * edit instead.
 */
const COBOL_ASTERISK = /^\* *$/;

/**
 * The type-code edits that run before a request is made, in program order
 * (`1150-STORE-MAP-IN-NEW`, then `1210-EDIT-TRANTYPE`); it adds at most one issue.
 * `TRTYPCDI = '*' OR TRTYPCDI = SPACES` stores low-values (`COTRTUPC.cbl:667-669`), which
 * `1245-EDIT-NUM-REQD` reports as not supplied (`COTRTUPC.cbl:911-930`).
 */
function addFirstTypeCodeIssue(values: { typeCode: string }, ctx: z.RefinementCtx): void {
  if (COBOL_BLANK.test(values.typeCode) || COBOL_ASTERISK.test(values.typeCode)) {
    ctx.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["typeCode"],
      message: TYPE_CODE_NOT_SUPPLIED,
    });
  }
}

/**
 * The search form of the fresh screen: the type code alone, sent to endpoint 26
 * (`GET /api/v1/transaction-types/{type_code}`).
 */
export const cotrtupKeySchema = z
  .object({
    typeCode: z.string().max(COTRTUP_LENGTHS.typeCode),
  })
  .superRefine(addFirstTypeCodeIssue);

/** Values of the search form, exactly as typed. */
export type CotrtupKeyValues = z.infer<typeof cotrtupKeySchema>;

/**
 * The details form once a type has been fetched or not found: type code and description.
 *
 * The type-code edit stays on this form because ENTER in the not-found state, where the code is
 * editable, still sends endpoint 26 and still gets `COTRTUPC-005` for a blank code
 * (`COTRTUPC-012`). In every other state the code is protected and holds the fetched, non-blank
 * code, so the edit never fires. The description carries only its map `LENGTH`.
 */
export const cotrtupSchema = z
  .object({
    typeCode: z.string().max(COTRTUP_LENGTHS.typeCode),
    description: z.string().max(COTRTUP_LENGTHS.description),
  })
  .superRefine(addFirstTypeCodeIssue);

/** Values of the details form, exactly as typed. */
export type CotrtupValues = z.infer<typeof cotrtupSchema>;
