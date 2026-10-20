import { describe, expect, it } from "vitest";
import { cotrn01Schema } from "./COTRN01";

/**
 * Client half of the two `both` rows `cotrn01Schema` repeats: the `TRNIDIN` length of
 * `modern/audit/screens.md` and the blank Tran ID edit of `modern/audit/online/COTRN01C.md`. The
 * expected message is the audited literal, written by hand. Every Tran ID is synthetic.
 */

const EMPTY_MESSAGE = "Tran ID can NOT be empty...";

/** zod's default `max(16)` message: the map attribute carries no message of its own. */
const TOO_LONG_MESSAGE = "String must contain at most 16 character(s)";

/** The issues of a rejected parse, as path and message pairs in zod's order. */
function issuesOf(tranId: string): { path: (string | number)[]; message: string }[] {
  const result = cotrn01Schema.safeParse({ tranId });
  expect(result.success).toBe(false);
  return (result.error?.issues ?? []).map(({ path, message }) => ({ path, message }));
}

describe("cotrn01Schema", () => {
  it.each(["0000000000000001", "ABCDEFGHIJKLMNOP", "TX-0001", " 000000000000001"])(
    "accepts %j and returns it unchanged",
    (tranId) => {
      expect(cotrn01Schema.safeParse({ tranId })).toEqual({ success: true, data: { tranId } });
    },
  );

  // audit: COTRN01-013
  it("accepts a 16-character Tran ID and rejects a 17-character one", () => {
    expect(cotrn01Schema.safeParse({ tranId: "1234567890123456" }).success).toBe(true);
    expect(issuesOf("12345678901234567")).toEqual([
      { path: ["tranId"], message: TOO_LONG_MESSAGE },
    ]);
  });

  // audit: COTRN01C-009
  it.each(["", "   ", " ".repeat(16)])("rejects the blank Tran ID %j", (tranId) => {
    expect(issuesOf(tranId)).toEqual([{ path: ["tranId"], message: EMPTY_MESSAGE }]);
  });

  // audit: COTRN01C-009
  it("accepts a Tran ID of 15 spaces and one non-space character", () => {
    expect(cotrn01Schema.safeParse({ tranId: `${" ".repeat(15)}1` }).success).toBe(true);
  });

  it("reports the length issue before the blank issue for 17 spaces", () => {
    expect(issuesOf(" ".repeat(17))).toEqual([
      { path: ["tranId"], message: TOO_LONG_MESSAGE },
      { path: ["tranId"], message: EMPTY_MESSAGE },
    ]);
  });
});
