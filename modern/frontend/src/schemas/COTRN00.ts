import { z } from "zod";

/**
 * Client-side form schema of map `COTRN0A` (`COTRN00`, transaction `CT00`, program `COTRN00C`).
 *
 * It holds the map's input-field lengths only. The server revalidates every request and owns
 * every message, and a `ScreenError` it returns wins over any message this schema produces.
 */

/** Map `LENGTH` of each input field, and the number of list rows (screen lines 10 to 19). */
export const COTRN00_LENGTHS = {
  /** `TRNIDIN`, `POS=(6,21) LENGTH=16` (`COTRN00.bms:95-99`). */
  startTranId: 16,
  /** `SEL0001` to `SEL0010`, `LENGTH=1` in column 3 (`COTRN00.bms:153-419`). */
  selection: 1,
  rows: 10,
} as const;

export const cotrn00Schema = z.object({
  // Screen rule COTRN00-015, Layer `both`: the map length, which endpoint 11's request DTO
  // revalidates. No numeric edit here: `Tran ID must be Numeric ...` (COTRN00C-013,
  // `COTRN00C.cbl:208-219`) is a `server` row tested on the 16-byte space-padded field, so only
  // the endpoint reproduces it and its message.
  startTranId: z.string().max(COTRN00_LENGTHS.startTranId),
  // Screen rules COTRN00-027 to COTRN00-081, Layer `client`: the selectors are never sent. Any
  // character is accepted, because which code selects is the screen's rule (COTRN00C-007 to
  // COTRN00C-010) and COBOL's invalid-selection message does not stop the ENTER.
  selections: z
    .array(z.string().max(COTRN00_LENGTHS.selection))
    .length(COTRN00_LENGTHS.rows),
});

export type Cotrn00Values = z.infer<typeof cotrn00Schema>;
