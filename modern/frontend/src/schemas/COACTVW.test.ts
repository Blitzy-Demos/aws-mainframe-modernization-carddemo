import { describe, expect, it } from "vitest";
import { COACTVW_LENGTHS, coactvwSchema } from "./COACTVW";

/**
 * Unit tests of the account-view form schema: the client half of screens.md COACTVW-013, the
 * `ACCTSID` input (`LENGTH=11`, `VALIDN=(MUSTFILL)`). Every expected length, issue code and
 * message is written by hand from that row and from zod's default length messages; none is read
 * from the schema. COACTVWC's `2210-EDIT-ACCOUNT` edits are `server` rows, proved through
 * endpoint 4, so the values they answer must pass this schema unchanged.
 */

describe("coactvwSchema", () => {
  // audit: COACTVW-013
  it("accepts an account number that fills all 11 positions of ACCTSID", () => {
    expect(COACTVW_LENGTHS).toEqual({ acctId: 11 });
    expect(coactvwSchema.safeParse({ acctId: "12345678901" })).toEqual({
      success: true,
      data: { acctId: "12345678901" },
    });
  });

  // audit: COACTVW-013
  it.each([
    {
      label: "a value one character over the map LENGTH",
      acctId: "123456789012",
      code: "too_big",
      message: "String must contain at most 11 character(s)",
    },
    {
      label: "a partly filled field (MUSTFILL)",
      acctId: "123",
      code: "too_small",
      message: "String must contain exactly 11 character(s)",
    },
    {
      label: "a single character (MUSTFILL)",
      acctId: "1",
      code: "too_small",
      message: "String must contain exactly 11 character(s)",
    },
    {
      label: "a value one character short of the map LENGTH (MUSTFILL)",
      acctId: "1234567890",
      code: "too_small",
      message: "String must contain exactly 11 character(s)",
    },
  ])("rejects $label on acctId", ({ acctId, code, message }) => {
    const result = coactvwSchema.safeParse({ acctId });
    expect(result.success).toBe(false);
    expect(result.error?.issues).toHaveLength(1);
    expect(result.error?.issues[0]).toMatchObject({ code, path: ["acctId"], message });
  });

  it.each([
    { label: "an empty field", acctId: "" },
    { label: "a full-length non-numeric value", acctId: "ABCDEFGHIJK" },
    { label: "eleven zeros", acctId: "00000000000" },
  ])("passes $label to the server unchanged", ({ acctId }) => {
    expect(coactvwSchema.safeParse({ acctId })).toEqual({ success: true, data: { acctId } });
  });
});
