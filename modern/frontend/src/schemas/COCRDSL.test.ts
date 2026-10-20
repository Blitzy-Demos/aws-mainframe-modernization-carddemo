import { describe, expect, it } from "vitest";
import { COCRDSL_LENGTHS, type CocrdslValues, cocrdslSchema } from "./COCRDSL";

/**
 * Unit tests of the `CCRDSLA` form schema. They prove the client half of the two `both` rows of
 * `modern/audit/screens.md`, the map `LENGTH` of `ACCTSID` and `CARDSID`, with zod's default
 * message, as the audit records none. Every COCRDSLC edit is a `server` row the endpoint owns, so
 * blank inputs pass here and no program message is asserted. All values are synthetic.
 */

/** An account ID and a card number at their full map lengths. */
const ACCT_ID = "00000000001";
const CARD_NUM = ["4444", "3333", "2222", "1111"].join("");

type ParseCase = { name: string; values: CocrdslValues };
type LengthCase = { field: keyof CocrdslValues; length: number; message: string };

describe("cocrdslSchema", () => {
  it("holds the map LENGTH of each input", () => {
    expect(COCRDSL_LENGTHS).toEqual({ acctId: 11, cardNum: 16 });
  });

  it.each<ParseCase>([
    { name: "a typed account and card number", values: { acctId: ACCT_ID, cardNum: CARD_NUM } },
    { name: "a blank card number (card_ref path)", values: { acctId: ACCT_ID, cardNum: "" } },
    { name: "a blank account and card number", values: { acctId: "", cardNum: "" } },
  ])("parses $name unchanged", ({ values }) => {
    expect(cocrdslSchema.safeParse(values)).toEqual({ success: true, data: values });
  });

  it.each<LengthCase>([
    // audit: COCRDSL-013
    { field: "acctId", length: 11, message: "String must contain at most 11 character(s)" },
    // audit: COCRDSL-016
    { field: "cardNum", length: 16, message: "String must contain at most 16 character(s)" },
  ])("accepts $field at $length characters and rejects one more", ({ field, length, message }) => {
    const valid: CocrdslValues = { acctId: ACCT_ID, cardNum: CARD_NUM };
    const boundary = cocrdslSchema.safeParse({ ...valid, [field]: "9".repeat(length) });
    expect(boundary.success).toBe(true);
    const over = cocrdslSchema.safeParse({ ...valid, [field]: "9".repeat(length + 1) });
    expect(over.success).toBe(false);
    expect(over.error?.issues[0]).toMatchObject({ path: [field], message });
  });
});
