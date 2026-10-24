import { z } from "zod";

/**
 * Form schema of map `CCRDSLA` (mapset `COCRDSL`, transaction `CCDL`, program COCRDSLC):
 * the account and card inputs of the credit card detail screen at `/cards/view`.
 *
 * Both inputs are unprotected `X(n)` fields without `NUM`, so the schema enforces only the map
 * `LENGTH` of each (COCRDSL-013, COCRDSL-016), with zod's default message, since the screen
 * audit records none. Endpoint 8 revalidates both lengths.
 *
 * COCRDSLC's own edits (`'No input received'`, the two not-provided texts and the two
 * `IF SUPPLIED MUST BE` texts) are deliberately absent: no COCRDSLC audit row is `both`, because
 * their precedence (account first, the first error's text stands, then the cross-field
 * override) is the endpoint's to decide and its message wins. A blank card number therefore
 * passes here. That also keeps the two requests that carry no typed number valid: entry from a
 * COCRDLI selection sends the row's `card_ref`, and an unchanged re-entry after a response
 * resends its `card_ref` while the card input may show the redacted number.
 */
export const COCRDSL_LENGTHS = { acctId: 11, cardNum: 16 } as const;

export const cocrdslSchema = z.object({
  /** `ACCTSID` (`ACCTSIDI PIC X(11)`), sent as `CardLookupQuery.acct_id`. */
  acctId: z.string().max(COCRDSL_LENGTHS.acctId),
  /** `CARDSID` (`CARDSIDI PIC X(16)`), sent as `CardLookupQuery.card_num` when typed. */
  cardNum: z.string().max(COCRDSL_LENGTHS.cardNum),
});

export type CocrdslValues = z.infer<typeof cocrdslSchema>;
