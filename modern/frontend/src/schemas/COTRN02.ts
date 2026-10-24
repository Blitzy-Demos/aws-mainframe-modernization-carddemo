import { z } from "zod";

/**
 * Client-side form schema of the Add Transaction screen, map `COTRN2A` of `app/bms/COTRN02.bms`
 * (transaction CT02, program COTRN02C, route `/transactions/add`, endpoint 13).
 *
 * It repeats only what the 3270 terminal enforced on the fourteen inputs, their map `LENGTH`:
 * the `both` rows `COTRN02-013`, `-017`, `-021`, `-024`, `-027`, `-030`, `-033`, `-036`,
 * `-039`, `-045`, `-048`, `-051`, `-054` and `-057` of `modern/audit/screens.md`. Every input is
 * `ATTRB=(FSET,NORM,UNPROT)` (`ACTIDIN` adds `IC`) and none is `NUM`, so no field is narrowed to
 * digits. The rows record no message, so a too-long value gets zod's own message. Endpoint 13's
 * request DTO revalidates every length, and a server `ScreenError` replaces whatever this schema
 * reports.
 *
 * None of COTRN02C's edits is repeated, deliberately: `modern/audit/online/COTRN02C.md` marks
 * every one `server`. The key edits (COTRN02C-009, -010, -015) precede every read, yet the audit
 * repeats none of them: the request is always sent and the server's message governs. The
 * data-field edits (COTRN02C-020 to COTRN02C-039) run only after the cross-reference read, so a
 * client check of an empty type code or a malformed amount would show its own message where the
 * program shows `Account ID NOT found...` or `Card Number NOT found...` (COTRN02C-013,
 * COTRN02C-018). The confirmation edits (COTRN02C-040 to COTRN02C-042) run after every data edit,
 * so every one-character confirmation is sent as typed. Date validity is `HALTED(H5)`
 * (`modern/audit/online/CSUTLDTC.md`), so the original and processing dates stay raw text that
 * only the server's edit turns into typed dates (the raw-edit-input exception of Typed Dates and
 * Timestamps), and the amount stays the text the program edits; no value is trimmed, converted
 * or case-changed.
 */

/** Map `LENGTH` of each input field, for the screen's `Input` `maxLength`. */
export const COTRN02_LENGTHS = {
  /** `ACTIDIN`, `POS=(6,21) LENGTH=11`, `COTRN02-013` (`COTRN02.bms:85-90`). */
  acctId: 11,
  /** `CARDNIN`, `POS=(6,55) LENGTH=16`, `COTRN02-017` (`COTRN02.bms:104-108`). */
  cardNum: 16,
  /** `TTYPCD`, `POS=(10,15) LENGTH=2`, `COTRN02-021` (`COTRN02.bms:122-127`). */
  typeCode: 2,
  /** `TCATCD`, `POS=(10,36) LENGTH=4`, `COTRN02-024` (`COTRN02.bms:135-140`). */
  categoryCode: 4,
  /** `TRNSRC`, `POS=(10,54) LENGTH=10`, `COTRN02-027` (`COTRN02.bms:148-153`). */
  source: 10,
  /** `TDESC`, `POS=(12,19) LENGTH=60`, `COTRN02-030` (`COTRN02.bms:161-166`). */
  description: 60,
  /** `TRNAMT`, `POS=(14,14) LENGTH=12`, `COTRN02-033` (`COTRN02.bms:174-179`). */
  amount: 12,
  /** `TORIGDT`, `POS=(14,42) LENGTH=10`, `COTRN02-036` (`COTRN02.bms:187-192`). */
  origDate: 10,
  /** `TPROCDT`, `POS=(14,68) LENGTH=10`, `COTRN02-039` (`COTRN02.bms:200-205`). */
  procDate: 10,
  /** `MID`, `POS=(16,19) LENGTH=9`, `COTRN02-045` (`COTRN02.bms:228-233`). */
  merchantId: 9,
  /** `MNAME`, `POS=(16,48) LENGTH=30`, `COTRN02-048` (`COTRN02.bms:241-246`). */
  merchantName: 30,
  /** `MCITY`, `POS=(18,21) LENGTH=25`, `COTRN02-051` (`COTRN02.bms:254-259`). */
  merchantCity: 25,
  /** `MZIP`, `POS=(18,67) LENGTH=10`, `COTRN02-054` (`COTRN02.bms:267-272`). */
  merchantZip: 10,
  /** `CONFIRM`, `POS=(21,63) LENGTH=1`, `COTRN02-057` (`COTRN02.bms:281-285`). */
  confirm: 1,
} as const;

/**
 * The two key inputs `VALIDATE-INPUT-KEY-FIELDS` reads, sent as `acct_id` and `card_num` of
 * endpoint 13. Either may be blank: the account takes precedence when both are entered, and the
 * program fills the other from the cross-reference (COTRN02C-011 to COTRN02C-017). It serves a
 * form step that holds only the key, such as F5, whose data fields the program replaces with the
 * copied transaction whatever the request carried (COTRN02C-052, COTRN02C-054).
 */
export const cotrn02KeySchema = z.object({
  acctId: z.string().max(COTRN02_LENGTHS.acctId),
  cardNum: z.string().max(COTRN02_LENGTHS.cardNum),
});

/** The key fields of the Add Transaction form, exactly as typed. */
export type Cotrn02KeyValues = z.infer<typeof cotrn02KeySchema>;

/**
 * The whole Add Transaction form: the key, the eleven data fields endpoint 13 carries in
 * `fields`, and the confirmation, each at most its map `LENGTH`.
 */
export const cotrn02Schema = cotrn02KeySchema.extend({
  typeCode: z.string().max(COTRN02_LENGTHS.typeCode),
  categoryCode: z.string().max(COTRN02_LENGTHS.categoryCode),
  source: z.string().max(COTRN02_LENGTHS.source),
  description: z.string().max(COTRN02_LENGTHS.description),
  // The amount is the text COTRN02C edits as a sign, eight digits, `.` and two digits
  // (COTRN02C-033); it is never parsed here.
  amount: z.string().max(COTRN02_LENGTHS.amount),
  origDate: z.string().max(COTRN02_LENGTHS.origDate),
  procDate: z.string().max(COTRN02_LENGTHS.procDate),
  merchantId: z.string().max(COTRN02_LENGTHS.merchantId),
  merchantName: z.string().max(COTRN02_LENGTHS.merchantName),
  merchantCity: z.string().max(COTRN02_LENGTHS.merchantCity),
  merchantZip: z.string().max(COTRN02_LENGTHS.merchantZip),
  confirm: z.string().max(COTRN02_LENGTHS.confirm),
});

/** The values of the Add Transaction form, exactly as typed. */
export type Cotrn02Values = z.infer<typeof cotrn02Schema>;
