import { describe, expect, it } from "vitest";
import { COUSR03_LENGTHS, type Cousr03Values, cousr03Schema } from "./COUSR03";

/**
 * Client half of the `both` rows the schema of map `COUSR3A` repeats: the map `LENGTH` of the
 * user ID input (`COUSR03-013` of `modern/audit/screens.md`), whose rejection carries zod's
 * default message because no row records one, and the blank user ID of the fetch
 * (`COUSR03C-010`) and of the delete (`COUSR03C-016`) of `modern/audit/online/COUSR03C.md`,
 * whose message is the audited literal, written by hand. `COUSR03C-010` is the reference row of
 * `COUSR02C-010` (shared endpoint 18), whose client half names the `COUSR03` schema, so its tag
 * sits beside that row's tag. ENTER and F5 run the same edit before their request, so one schema
 * and one set of cases serve both keys. Blank is empty or all spaces; a user ID with a non-space
 * character, leading spaces included, is not blank, and COUSR03C upper-cases nothing, so every
 * accepted user ID passes exactly as typed. Every value is synthetic.
 */

/** `COUSR03C.cbl:147` and `:179`: the blank user ID message of the fetch and of the delete. */
const USER_ID_EMPTY = "User ID can NOT be empty...";

/** Zod's default `max` message, which the length rejection carries. */
const tooLong = (length: number): string => `String must contain at most ${length} character(s)`;

/** A synthetic user ID of the map's 8 characters, as ENTER or F5 sends it. */
const TYPED: Cousr03Values = { userId: "SYNUSR03" };

/** A parse result as its `success` flag and the path and message of each issue, in zod's order. */
function outcome(result: {
  success: boolean;
  error?: { issues: { path: (string | number)[]; message: string }[] };
}) {
  return {
    success: result.success,
    issues: result.error?.issues.map(({ path, message }) => ({ path, message })) ?? [],
  };
}

describe("cousr03Schema", () => {
  it.each([TYPED.userId, "syn3", "S", "user0003"])("accepts the user ID %j unchanged", (userId) => {
    expect(cousr03Schema.safeParse({ userId })).toEqual({ success: true, data: { userId } });
  });

  // audit: COUSR03-013
  it("accepts a user ID of the map's 8 characters and rejects one of 9", () => {
    expect(COUSR03_LENGTHS.userId).toBe(8);
    expect(outcome(cousr03Schema.safeParse(TYPED))).toEqual({ success: true, issues: [] });
    expect(outcome(cousr03Schema.safeParse({ userId: `${TYPED.userId}9` }))).toEqual({
      success: false,
      issues: [{ path: ["userId"], message: tooLong(8) }],
    });
  });

  // audit: COUSR02C-010
  // audit: COUSR03C-010
  // audit: COUSR03C-016
  it.each(["", " ", " ".repeat(8)])("rejects the blank user ID %j of ENTER and F5", (userId) => {
    expect(outcome(cousr03Schema.safeParse({ userId }))).toEqual({
      success: false,
      issues: [{ path: ["userId"], message: USER_ID_EMPTY }],
    });
  });

  // audit: COUSR02C-010
  // audit: COUSR03C-010
  // audit: COUSR03C-016
  it.each([`${" ".repeat(7)}3`, `3${" ".repeat(7)}`])(
    "accepts the user ID %j, whose one non-space character makes it not blank",
    (userId) => {
      expect(cousr03Schema.safeParse({ userId })).toEqual({ success: true, data: { userId } });
    },
  );

  it("reports the length issue before the blank issue for 9 spaces", () => {
    expect(outcome(cousr03Schema.safeParse({ userId: " ".repeat(9) }))).toEqual({
      success: false,
      issues: [
        { path: ["userId"], message: tooLong(8) },
        { path: ["userId"], message: USER_ID_EMPTY },
      ],
    });
  });
});
