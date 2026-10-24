import { describe, expect, it } from "vitest";
import { COUSR01_LENGTHS, type Cousr01Values, cousr01Schema } from "./COUSR01";

/**
 * Client half of the five `both` rows of the Add User map `COUSR1A` in `modern/audit/screens.md`:
 * the map `LENGTH` of `FNAME` (`COUSR01-013`, 20), `LNAME` (`COUSR01-016`, 20), `USERID`
 * (`COUSR01-019`, 8), `PASSWD` (`COUSR01-022`, 8) and `USRTYPE` (`COUSR01-025`, 1). Every expected
 * length is written by hand from `app/bms/COUSR01.bms`, and the rows record no message, so the
 * expected messages are zod's defaults. COUSR01C's empty-field edits (`COUSR01C-010` to
 * `COUSR01C-014`) and its acceptance of every non-blank user type (`COUSR01C-015`) are `server`
 * rows the schema does not repeat, so blank fields and user types other than `A` and `U` pass here,
 * and every value is kept as typed (E26). Every value is synthetic, and no issue message may quote
 * a submitted password.
 */

/** Zod's default `max` message, which every length rejection carries. */
const tooLong = (length: number): string => `String must contain at most ${length} character(s)`;

const VALID: Cousr01Values = {
  firstName: "JANE",
  lastName: "SAMPLE",
  userId: "TSTUSR01",
  password: "TESTPW01",
  userType: "U",
};

/** The `success` flag of parsing `values`, with the path and message of each issue in order. */
function check(values: Cousr01Values) {
  const result = cousr01Schema.safeParse(values);
  return {
    success: result.success,
    issues: result.error?.issues.map(({ path, message }) => ({ path, message })) ?? [],
  };
}

describe("COUSR01_LENGTHS", () => {
  it("holds the map LENGTH of each field the screen sends", () => {
    expect(COUSR01_LENGTHS).toEqual({
      firstName: 20,
      lastName: 20,
      userId: 8,
      password: 8,
      userType: 1,
    });
  });
});

describe("cousr01Schema", () => {
  it.each([
    { name: "a valid user set", values: VALID },
    { name: "a valid admin set", values: { ...VALID, userId: "TSTADM01", userType: "A" } },
    {
      name: "a lower-case password, stored as typed (E26)",
      values: { ...VALID, password: "secret12" },
    },
    { name: "a lower-case user ID, stored as typed", values: { ...VALID, userId: "tstusr01" } },
    {
      name: "names with leading and embedded spaces, never trimmed",
      values: { ...VALID, firstName: " JANE Q", lastName: "  SAMPLE JR" },
    },
    { name: "user type a, an edit the server owns", values: { ...VALID, userType: "a" } },
    { name: "user type X, an edit the server owns", values: { ...VALID, userType: "X" } },
    { name: "user type 9, an edit the server owns", values: { ...VALID, userType: "9" } },
    {
      name: "every field empty, edits the server owns",
      values: { firstName: "", lastName: "", userId: "", password: "", userType: "" },
    },
    {
      name: "every field all spaces at its map LENGTH, edits the server owns",
      values: {
        firstName: " ".repeat(20),
        lastName: " ".repeat(20),
        userId: " ".repeat(8),
        password: " ".repeat(8),
        userType: " ",
      },
    },
  ])("passes $name unchanged", ({ values }) => {
    expect(cousr01Schema.safeParse(values)).toEqual({ success: true, data: values });
  });

  // audit: COUSR01-013
  // audit: COUSR01-016
  // audit: COUSR01-019
  // audit: COUSR01-022
  // audit: COUSR01-025
  it.each([
    ["firstName", 20],
    ["lastName", 20],
    ["userId", 8],
    ["password", 8],
    ["userType", 1],
  ] as const)("accepts %s at its map LENGTH %i and rejects one more", (field, length) => {
    const atLength = { ...VALID, [field]: "A".repeat(length) };
    expect(cousr01Schema.safeParse(atLength)).toEqual({ success: true, data: atLength });

    const overLength = { ...VALID, [field]: "A".repeat(length + 1) };
    const result = check(overLength);
    expect(result).toEqual({
      success: false,
      issues: [{ path: [field], message: tooLong(length) }],
    });
    expect(result.issues.filter(({ message }) => message.includes(overLength.password))).toEqual(
      [],
    );
  });

  it("reports every exceeded length in field order and never quotes the submitted password", () => {
    const password = "secret123";
    const result = check({
      firstName: "F".repeat(21),
      lastName: "L".repeat(21),
      userId: "TSTUSR012",
      password,
      userType: "UU",
    });
    expect(result).toEqual({
      success: false,
      issues: [
        { path: ["firstName"], message: tooLong(20) },
        { path: ["lastName"], message: tooLong(20) },
        { path: ["userId"], message: tooLong(8) },
        { path: ["password"], message: tooLong(8) },
        { path: ["userType"], message: tooLong(1) },
      ],
    });
    expect(result.issues.filter(({ message }) => message.includes(password))).toEqual([]);
  });
});
