import { z } from "zod";

/**
 * Client-side form schemas of map COCRDUP (`CCRDUPA`): the card update screen of
 * transaction CCUP, program COCRDUPC, route `/cards/update`.
 *
 * Each schema repeats only the map `LENGTH` of the fields the screen sends, the rows
 * `modern/audit/screens.md` audits as Layer `both`; the map declares no `NUM`, and a
 * length rejection carries zod's own message because no row records one.
 *
 * Neither schema repeats a COCRDUPC edit. `modern/audit/online/COCRDUPC.md` places every
 * one of them (account, card, name, status, expiry month and year, the change check) in
 * Layer `server`: the endpoint owns each edit's message, field, state and info line, and
 * a client-side check would show a message the program never sends at that point.
 */

/** Map `LENGTH` of each field the screen sends (`app/bms/COCRDUP.bms`). */
export const COCRDUP_LENGTHS = {
  /** `ACCTSID`, screens.md COCRDUP-013. */
  acctId: 11,
  /** `CARDSID`, screens.md COCRDUP-016. */
  cardNum: 16,
  /** `CRDNAME`, screens.md COCRDUP-019. */
  embossedName: 50,
  /** `CRDSTCD`, screens.md COCRDUP-022. */
  activeStatus: 1,
  /** `EXPMON`, screens.md COCRDUP-025. */
  expiryMonth: 2,
  /** `EXPYEAR`, screens.md COCRDUP-027. */
  expiryYear: 4,
} as const;

/**
 * Search keys, enterable while no card is fetched and sent to endpoint 9
 * (`GET /api/v1/cards/edit`) as `acct_id` and `card_num`.
 */
export const cocrdupKeySchema = z.object({
  acctId: z.string().max(COCRDUP_LENGTHS.acctId),
  cardNum: z.string().max(COCRDUP_LENGTHS.cardNum),
});

export type CocrdupKeyValues = z.infer<typeof cocrdupKeySchema>;

/**
 * The whole screen once a card is fetched.
 *
 * Account and card are read-only and hold what endpoint 9 returned: the card number
 * redacted to twelve `*` and its last four digits, which fits the 16-character field.
 * The save addresses the card by `card_ref` (endpoint 10, `PUT /api/v1/cards/{card_ref}`),
 * never by the displayed number. Name, status, month and year are the raw screen text
 * COCRDUPC edits; month and year stay text, because the server edits them as screen
 * parts and assembles the typed expiry date from them.
 */
export const cocrdupSchema = z.object({
  acctId: z.string().max(COCRDUP_LENGTHS.acctId),
  cardNum: z.string().max(COCRDUP_LENGTHS.cardNum),
  embossedName: z.string().max(COCRDUP_LENGTHS.embossedName),
  activeStatus: z.string().max(COCRDUP_LENGTHS.activeStatus),
  expiryMonth: z.string().max(COCRDUP_LENGTHS.expiryMonth),
  expiryYear: z.string().max(COCRDUP_LENGTHS.expiryYear),
});

export type CocrdupValues = z.infer<typeof cocrdupSchema>;
