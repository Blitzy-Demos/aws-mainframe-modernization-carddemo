import { z } from "zod";

/**
 * Client-side form schema of map `CTRTLIA` (mapset `COTRTLI`, transaction `CTLI`, program
 * `COTRTLIC`): the transaction type list served by endpoints 24 and 25.
 *
 * It enforces only the map `LENGTH` of each input field, the `both` rows of `screens.md`; the
 * request DTOs revalidate the same lengths, and a server `ScreenError` wins on screen.
 */

/**
 * Input field lengths from `app/app-transaction-type-db2/bms/COTRTLI.bms`, and the seven list
 * rows COTRTLIC addresses (`EACH-ROWI OCCURS 7`, `WS-MAX-SCREEN-LINES`); the map's eighth row
 * (`TRTSELA`, row 19) is never written and is not an input.
 */
export const COTRTLI_LENGTHS = {
  /** `TRTYPE` at (6,44), `FSET,IC,NORM,UNPROT` (COTRTLI-015). */
  typeFilter: 2,
  /** `TRDESC` at (8,25), `FSET,NORM,UNPROT` (COTRTLI-018). */
  descFilter: 50,
  /**
   * `TRTSEL1`–`TRTSEL7` at column 6 of rows 12–18 (COTRTLI-026, COTRTLI-032, COTRTLI-038,
   * COTRTLI-044, COTRTLI-050, COTRTLI-056, COTRTLI-062).
   */
  selection: 1,
  /**
   * `TRTYPD1`–`TRTYPD7` at column 25 of rows 12–18 (COTRTLI-030, COTRTLI-036, COTRTLI-042,
   * COTRTLI-048, COTRTLI-054, COTRTLI-060, COTRTLI-066).
   */
  description: 50,
  /** List rows 12–18. */
  rows: 7,
} as const;

/** One list row's inputs: the `U`/`D` selector and the editable description. */
const rowSchema = z.object({
  selection: z.string().max(COTRTLI_LENGTHS.selection),
  description: z.string().max(COTRTLI_LENGTHS.description),
});

/**
 * Values the COTRTLI screen sends with ENTER and F10.
 *
 * No COTRTLIC program edit is repeated here: `online/COTRTLIC.md` marks every edit `server`
 * (no `both` row). The selection edit (COTRTLIC-020 to COTRTLIC-023) reads a selection as
 * blank wherever the echoed list state holds no row (COTRTLIC-018), and its messages are
 * overwritten in row order by state-dependent ones (COTRTLIC-024, COTRTLIC-033); the
 * type-filter message can in turn be replaced by the cross-edit's database count
 * (COTRTLIC-031). So even the invalid-action, single-action and type-filter messages
 * (COTRTLIC-021, COTRTLIC-022, COTRTLIC-028) cannot be decided without the list state or the
 * database, neither of which this schema holds.
 * Values are kept as typed; the server trims and compares them as COTRTLIC does (COTRTLIC-017,
 * COTRTLIC-024).
 */
export const cotrtliSchema = z.object({
  typeFilter: z.string().max(COTRTLI_LENGTHS.typeFilter),
  descFilter: z.string().max(COTRTLI_LENGTHS.descFilter),
  rows: z.array(rowSchema).length(COTRTLI_LENGTHS.rows),
});

/** Form values of the COTRTLI screen, inferred from `cotrtliSchema`. */
export type CotrtliValues = z.infer<typeof cotrtliSchema>;
