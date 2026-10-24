import { describe, expect, it } from "vitest";
import { COSGN00_LENGTHS, type Cosgn00Values, cosgn00Schema } from "./COSGN00";

/**
 * Client half of the two `both` rows of the sign-on map `COSGN0A` in `modern/audit/screens.md`:
 * the map `LENGTH` of `USERID` (`COSGN00-027`, 8) and of the dark `PASSWD` (`COSGN00-031`, 8).
 * Both lengths are written by hand from `app/bms/COSGN00.bms:156-160,175-180`, and the rows record
 * no message, so the expected message is zod's default.
 *
 * COSGN00C's blank user ID and blank password edits (`COSGN00C-007`, `COSGN00C-008`) are `server`
 * rows of `modern/audit/online/COSGN00C.md` that the schema does not repeat: blank fields pass
 * here, and their messages, in program order, reach the screen from endpoint 1. Upper-casing and
 * padding to 8 bytes are server rules too (`COSGN00C-003`, `COSGN00C-009`, E26), so every value
 * parses exactly as typed. The two lengths are independent terminal attributes rather than ordered
 * program edits, so both are reported when both are exceeded.
 *
 * Every value is synthetic; no fixture password appears, and no issue message may quote a submitted
 * password.
 */

/** Zod's default `max` message, which every length rejection carries. */
const tooLong = (length: number): string => `String must contain at most ${length} character(s)`;

const VALID: Cosgn00Values = { userId: "TSTADM01", password: "TESTPW01" };

/** The `success` flag of parsing `values`, with the path and message of each issue in order. */
function check(values: Cosgn00Values) {
  const result = cosgn00Schema.safeParse(values);
  return {
    success: result.success,
    issues: result.error?.issues.map(({ path, message }) => ({ path, message })) ?? [],
  };
}

describe("COSGN00_LENGTHS", () => {
  it("holds the map LENGTH of the user ID and password inputs", () => {
    expect(COSGN00_LENGTHS).toEqual({ userId: 8, password: 8 });
  });
});

describe("cosgn00Schema", () => {
  it.each([
    { name: "a complete valid set", values: VALID },
    {
      name: "values shorter than their map LENGTH",
      values: { userId: "TST1", password: "PW1" },
    },
    {
      name: "a lower-case user ID and password, upper-cased only by the server (E26)",
      values: { userId: "tstusr01", password: "testpw01" },
    },
    {
      name: "leading and embedded spaces, never trimmed",
      values: { userId: " TST 01", password: " PW 01 " },
    },
    { name: "an empty user ID, an edit the server owns", values: { ...VALID, userId: "" } },
    {
      name: "an all-space user ID at its map LENGTH, an edit the server owns",
      values: { ...VALID, userId: " ".repeat(8) },
    },
    { name: "an empty password, an edit the server owns", values: { ...VALID, password: "" } },
    {
      name: "an all-space password at its map LENGTH, an edit the server owns",
      values: { ...VALID, password: " ".repeat(8) },
    },
    {
      name: "both fields empty, edits the server owns",
      values: { userId: "", password: "" },
    },
  ])("passes $name unchanged", ({ values }) => {
    expect(cosgn00Schema.safeParse(values)).toEqual({ success: true, data: values });
  });

  // audit: COSGN00-027
  it("accepts a user ID at its map LENGTH 8 and rejects 9 characters", () => {
    const atLength = { ...VALID, userId: "TSTUSR08" };
    expect(cosgn00Schema.safeParse(atLength)).toEqual({ success: true, data: atLength });

    expect(check({ ...VALID, userId: "TSTUSR009" })).toEqual({
      success: false,
      issues: [{ path: ["userId"], message: tooLong(COSGN00_LENGTHS.userId) }],
    });
  });

  // audit: COSGN00-031
  it("accepts a password at its map LENGTH 8 and rejects 9 characters without quoting it", () => {
    const atLength = { ...VALID, password: "testpw08" };
    expect(cosgn00Schema.safeParse(atLength)).toEqual({ success: true, data: atLength });

    const password = "testpw009";
    const result = check({ ...VALID, password });
    expect(result).toEqual({
      success: false,
      issues: [{ path: ["password"], message: tooLong(COSGN00_LENGTHS.password) }],
    });
    expect(result.issues.filter(({ message }) => message.includes(password))).toEqual([]);
  });

  it("reports both exceeded lengths in field order and never quotes the submitted password", () => {
    const password = "testpw0009";
    const result = check({ userId: "TSTUSR0009", password });
    expect(result).toEqual({
      success: false,
      issues: [
        { path: ["userId"], message: tooLong(8) },
        { path: ["password"], message: tooLong(8) },
      ],
    });
    expect(result.issues.filter(({ message }) => message.includes(password))).toEqual([]);
  });
});
