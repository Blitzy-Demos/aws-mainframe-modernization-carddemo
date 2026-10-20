import { z } from "zod";

/**
 * Client-side form schema of the account-view screen: map `COACTVW` (`CACTVWA`), transaction
 * `CAVW`, program `COACTVWC`, route `/accounts/view`.
 *
 * The map has one input, `ACCTSID` (`POS=(5,38) LENGTH=11`, `VALIDN=(MUSTFILL)`, no `NUM`),
 * audited as screens.md COACTVW-013, the screen's only `both` row. The schema repeats what the
 * terminal enforced on that field and nothing else; endpoint 4 re-validates the value and owns
 * every message, and a server `ScreenError` replaces the message shown from here.
 *
 * COACTVWC's `2210-EDIT-ACCOUNT` edits (blank, not numeric, all zeros) are deliberately absent:
 * online/COACTVWC.md places them on the `server` layer only (COACTVWC-030, COACTVWC-031), so the
 * endpoint must receive a blank or non-numeric value and answer it with the program's own text.
 */

/** Map `LENGTH` of each input field. */
export const COACTVW_LENGTHS = { acctId: 11 } as const;

export const coactvwSchema = z
  .object({
    acctId: z.string().max(COACTVW_LENGTHS.acctId),
  })
  .superRefine((values, ctx) => {
    // MUSTFILL (COACTVW-013, X-SCREENS-5): a field left empty is sent as is, while a partly
    // filled one is refused, as the terminal refused to transmit it. MUSTFILL is a map attribute
    // rather than a program edit, and screens.md records no message for it, so the issue is
    // zod's own length issue and carries zod's default message instead of a custom literal.
    const { length } = values.acctId;
    if (length > 0 && length < COACTVW_LENGTHS.acctId) {
      ctx.addIssue({
        code: z.ZodIssueCode.too_small,
        type: "string",
        minimum: COACTVW_LENGTHS.acctId,
        inclusive: true,
        exact: true,
        path: ["acctId"],
      });
    }
  });

export type CoactvwValues = z.infer<typeof coactvwSchema>;
