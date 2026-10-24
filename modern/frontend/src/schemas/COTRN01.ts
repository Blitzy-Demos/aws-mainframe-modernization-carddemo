import { z } from "zod";

/**
 * Client-side form schema of map `COTRN01` (`COTRN1A`): transaction `CT01`, program `COTRN01C`,
 * route `/transactions/view`.
 *
 * It repeats only the checks the audits mark `both`, so the screen sends no request the program
 * would reject before reading. Endpoint 12 revalidates both and owns every message: a server
 * `ScreenError` replaces whatever this schema reports.
 * - `COTRN01-013`: the Tran ID input `TRNIDIN` is `LENGTH=16` (`app/bms/COTRN01.bms:85-90`).
 * - `COTRN01C-009`: ENTER with a blank Tran ID (`app/cbl/COTRN01C.cbl:146-152`).
 */
export const cotrn01Schema = z
  .object({
    /** `TRNIDIN`, sent as the `tran_id` path parameter of endpoint 12. */
    tranId: z.string().max(16),
  })
  .superRefine((values, ctx) => {
    // COBOL `SPACES`: empty or all spaces. LOW-VALUES cannot be typed into a browser input, so
    // only the server half of COTRN01C-009 meets them.
    if (/^ *$/.test(values.tranId)) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["tranId"],
        message: "Tran ID can NOT be empty...",
      });
    }
  });

/** The values of the COTRN01 form, as `useForm` holds them and the resolver returns them. */
export type Cotrn01Values = z.infer<typeof cotrn01Schema>;
