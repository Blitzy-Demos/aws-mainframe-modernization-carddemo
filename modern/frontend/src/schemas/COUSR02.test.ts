import { describe, expect, it } from "vitest";
import {
  COUSR02_LENGTHS,
  type Cousr02Values,
  cousr02KeySchema,
  cousr02Schema,
} from "./COUSR02";

/**
 * Client half of the `both` rows the two schemas of map `COUSR2A` repeat: the map `LENGTH` of
 * the five inputs (`COUSR02-013`, `COUSR02-017`, `COUSR02-020`, `COUSR02-023` and `COUSR02-026`
 * of `modern/audit/screens.md`), whose rejections carry zod's default message because no row
 * records one, and the blank user ID of the fetch (`COUSR02C-010`) and of the update
 * (`COUSR02C-018`) of `modern/audit/online/COUSR02C.md`, whose message is the audited literal,
 * written by hand. Blank is empty or all spaces; a user ID with a non-space character, leading
 * spaces included, is not blank. The update's blank first name, last name, password and user
 * type edits (`COUSR02C-019` to `COUSR02C-022`) are `server` rows the schema does not repeat, so
 * those inputs pass here, and the password stays optional (E27). Every value is synthetic, and no
 * test title or message carries a password.
 */

/** `COUSR02C.cbl:148` and `:182`: the blank user ID message of the fetch and of the update. */
const USER_ID_EMPTY = "User ID can NOT be empty...";

/** Zod's default `max` message, which every length rejection carries. */
const tooLong = (length: number): string => `String must contain at most ${length} character(s)`;

/** A synthetic dark-field value of the map's 8 characters. */
const PASSWORD = "pw".repeat(4);

/** Every update field but the dark password, which the screen sends only when typed (E27). */
const UPDATE_VALUES: Cousr02Values = {
  userId: "SYNUSR01",
  firstName: "Synthetic",
  lastName: "Operator",
  userType: "U",
};

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

describe("cousr02KeySchema", () => {
  it.each(["SYNUSR01", "syn1", "S", " SYN"])("accepts the user ID %j unchanged", (userId) => {
    expect(cousr02KeySchema.safeParse({ userId })).toEqual({ success: true, data: { userId } });
  });

  // audit: COUSR02-013
  it("accepts a user ID of the map's 8 characters and rejects one of 9", () => {
    expect(COUSR02_LENGTHS.userId).toBe(8);
    expect(outcome(cousr02KeySchema.safeParse({ userId: "SYNUSR01" }))).toEqual({
      success: true,
      issues: [],
    });
    expect(outcome(cousr02KeySchema.safeParse({ userId: "SYNUSR012" }))).toEqual({
      success: false,
      issues: [{ path: ["userId"], message: tooLong(8) }],
    });
  });

  // audit: COUSR02C-010
  it.each(["", " ", " ".repeat(8)])("rejects the blank user ID %j", (userId) => {
    expect(outcome(cousr02KeySchema.safeParse({ userId }))).toEqual({
      success: false,
      issues: [{ path: ["userId"], message: USER_ID_EMPTY }],
    });
  });

  // audit: COUSR02C-010
  it.each([`${" ".repeat(7)}1`, `1${" ".repeat(7)}`])(
    "accepts the user ID %j, whose one non-space character makes it not blank",
    (userId) => {
      expect(cousr02KeySchema.safeParse({ userId })).toEqual({ success: true, data: { userId } });
    },
  );

  it("reports the length issue before the blank issue for 9 spaces", () => {
    expect(outcome(cousr02KeySchema.safeParse({ userId: " ".repeat(9) }))).toEqual({
      success: false,
      issues: [
        { path: ["userId"], message: tooLong(8) },
        { path: ["userId"], message: USER_ID_EMPTY },
      ],
    });
  });
});

describe("cousr02Schema", () => {
  it.each([
    { name: "every field, the password typed", values: { ...UPDATE_VALUES, password: PASSWORD } },
    {
      name: "an admin type with lower-case and digit names",
      values: { userId: "syn2", firstName: "ada 2", lastName: "o'neil-smith", userType: "A" },
    },
    {
      name: "a user type other than A or U, which COUSR02C stores as typed",
      values: { ...UPDATE_VALUES, userType: "x" },
    },
  ])("passes $name unchanged", ({ values }) => {
    expect(cousr02Schema.safeParse(values)).toEqual({ success: true, data: values });
  });

  it.each([
    { field: "userId", length: 8 }, // audit: COUSR02-013
    { field: "firstName", length: 20 }, // audit: COUSR02-017
    { field: "lastName", length: 20 }, // audit: COUSR02-020
    { field: "password", length: 8 }, // audit: COUSR02-023
    { field: "userType", length: 1 }, // audit: COUSR02-026
  ] as const)(
    "accepts $field at its map LENGTH $length and rejects one character more",
    ({ field, length }) => {
      const typed = { ...UPDATE_VALUES, password: PASSWORD };
      expect(COUSR02_LENGTHS[field]).toBe(length);
      expect(outcome(cousr02Schema.safeParse({ ...typed, [field]: "A".repeat(length) }))).toEqual({
        success: true,
        issues: [],
      });
      expect(
        outcome(cousr02Schema.safeParse({ ...typed, [field]: "A".repeat(length + 1) })),
      ).toEqual({
        success: false,
        issues: [{ path: [field], message: tooLong(length) }],
      });
    },
  );

  // audit: COUSR02C-018
  it.each(["", " ", " ".repeat(8)])(
    "rejects the blank user ID %j, the update's first edit, whatever the other fields hold",
    (userId) => {
      const blankRest = { userId, firstName: "", lastName: "", userType: "" };
      expect(outcome(cousr02Schema.safeParse({ ...UPDATE_VALUES, userId }))).toEqual({
        success: false,
        issues: [{ path: ["userId"], message: USER_ID_EMPTY }],
      });
      expect(outcome(cousr02Schema.safeParse(blankRest))).toEqual({
        success: false,
        issues: [{ path: ["userId"], message: USER_ID_EMPTY }],
      });
    },
  );

  // audit: COUSR02C-018
  it.each([`${" ".repeat(7)}1`, `1${" ".repeat(7)}`])(
    "accepts the user ID %j, whose one non-space character makes it not blank",
    (userId) => {
      const values = { ...UPDATE_VALUES, userId };
      expect(cousr02Schema.safeParse(values)).toEqual({ success: true, data: values });
    },
  );

  it.each([
    { name: "an empty first name", field: "firstName", value: "" },
    { name: "an all-space first name", field: "firstName", value: " ".repeat(20) },
    { name: "an empty last name", field: "lastName", value: "" },
    { name: "an all-space last name", field: "lastName", value: " ".repeat(20) },
    { name: "an empty user type", field: "userType", value: "" },
    { name: "an all-space user type", field: "userType", value: " " },
  ] as const)("passes $name unchanged, an edit the server owns", ({ field, value }) => {
    const values = { ...UPDATE_VALUES, [field]: value };
    expect(cousr02Schema.safeParse(values)).toEqual({ success: true, data: values });
  });

  it.each([
    { name: "an omitted password", values: UPDATE_VALUES },
    { name: "an undefined password", values: { ...UPDATE_VALUES, password: undefined } },
    { name: "an empty password", values: { ...UPDATE_VALUES, password: "" } },
    { name: "an all-space password", values: { ...UPDATE_VALUES, password: " ".repeat(8) } },
  ])("passes $name with every other field valid, as E27 leaves it to the server", ({ values }) => {
    expect(cousr02Schema.safeParse(values)).toEqual({ success: true, data: values });
  });
});
