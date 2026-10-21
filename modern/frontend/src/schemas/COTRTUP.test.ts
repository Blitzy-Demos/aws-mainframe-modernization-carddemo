import { describe, expect, it } from "vitest";
import { COTRTUP_LENGTHS, cotrtupKeySchema, cotrtupSchema } from "./COTRTUP";

/**
 * Client half of the three `both` rows the transaction type maintenance schemas repeat: the map
 * `LENGTH` of `TRTYPCD` (`COTRTUP-013`, 2) and of `TRTYDSC` (`COTRTUP-016`, 50) in
 * `modern/audit/screens.md`, and the blank or `*` type code of `modern/audit/online/COTRTUPC.md`
 * (`COTRTUPC-005`). The length rows record no message, so their expected messages are zod's
 * defaults; the type-code message is the audited literal, written by hand. The numeric, zero and
 * normalising type-code edits (`COTRTUPC-006` to `COTRTUPC-008`), the no-change test
 * (`COTRTUPC-017`) and the description edits (`COTRTUPC-018`, `COTRTUPC-019`) are `server` rows
 * the schemas do not repeat, so those values pass here. Every value is synthetic.
 */

const TYPE_CODE_NOT_SUPPLIED = "Tran Type code must be supplied.";

/** zod's default `max(2)` message: the map attribute carries no message of its own. */
const TYPE_CODE_TOO_LONG = "String must contain at most 2 character(s)";

/** zod's default `max(50)` message: the map attribute carries no message of its own. */
const DESCRIPTION_TOO_LONG = "String must contain at most 50 character(s)";

/** A description of the map's full 50 characters. */
const FULL_DESCRIPTION = "D".repeat(50);

/**
 * Type codes the COBOL comparison `TRTYPCDI = '*' OR TRTYPCDI = SPACES` matches once the shorter
 * operand is padded with spaces, all within the map's 2 characters.
 */
const NOT_SUPPLIED_TYPE_CODES = ["", " ", "  ", "*", "* "];

/** The `success` flag of a parse, with the path and message of each issue in zod's order. */
function outcome(result: {
  success: boolean;
  error?: { issues: { path: (string | number)[]; message: string }[] };
}) {
  return {
    success: result.success,
    issues: result.error?.issues.map(({ path, message }) => ({ path, message })) ?? [],
  };
}

describe("cotrtupKeySchema", () => {
  it.each([
    { name: "a two-digit code", typeCode: "01" },
    { name: "the highest two-digit code", typeCode: "99" },
    { name: "a one-digit code", typeCode: "7" },
    { name: "a code with a leading space, which the server trims", typeCode: " 9" },
    { name: "letters, which the server's numeric edit owns", typeCode: "AB" },
    { name: "a digit and a letter, which the server's numeric edit owns", typeCode: "1A" },
    { name: "two asterisks, which the server's numeric edit owns", typeCode: "**" },
    { name: "a zero code, which the server's zero edit owns", typeCode: "00" },
    { name: "a signed code, which the server normalises", typeCode: "-1" },
  ])("passes $name unchanged", ({ typeCode }) => {
    expect(cotrtupKeySchema.safeParse({ typeCode })).toEqual({
      success: true,
      data: { typeCode },
    });
  });

  // audit: COTRTUP-013
  it("accepts a type code of the map's 2 characters and rejects one of 3", () => {
    expect(COTRTUP_LENGTHS.typeCode).toBe(2);
    expect(outcome(cotrtupKeySchema.safeParse({ typeCode: "12" }))).toEqual({
      success: true,
      issues: [],
    });
    expect(outcome(cotrtupKeySchema.safeParse({ typeCode: "123" }))).toEqual({
      success: false,
      issues: [{ path: ["typeCode"], message: TYPE_CODE_TOO_LONG }],
    });
  });

  // audit: COTRTUPC-005
  it.each(NOT_SUPPLIED_TYPE_CODES)("rejects the type code %j as not supplied", (typeCode) => {
    expect(outcome(cotrtupKeySchema.safeParse({ typeCode }))).toEqual({
      success: false,
      issues: [{ path: ["typeCode"], message: TYPE_CODE_NOT_SUPPLIED }],
    });
  });

  // audit: COTRTUPC-005
  it("accepts a space before an asterisk, which the padded comparison does not match", () => {
    expect(cotrtupKeySchema.safeParse({ typeCode: " *" })).toEqual({
      success: true,
      data: { typeCode: " *" },
    });
  });
});

describe("cotrtupSchema", () => {
  it.each([
    { name: "a code and a description", typeCode: "01", description: "Synthetic purchase" },
    { name: "a description of 50 characters", typeCode: "99", description: FULL_DESCRIPTION },
    { name: "a description with outer spaces", typeCode: "7", description: "  Synthetic  " },
    { name: "a blank description, an edit the server owns", typeCode: "01", description: "" },
    { name: "a description of spaces, an edit the server owns", typeCode: "01", description: "  " },
    { name: "a description `*`, an edit the server owns", typeCode: "01", description: "*" },
    {
      name: "a description with punctuation, an edit the server owns",
      typeCode: "01",
      description: "Synthetic type!",
    },
    {
      name: "a description in another case, a comparison the server owns",
      typeCode: "01",
      description: "SYNTHETIC PURCHASE",
    },
    { name: "a letter code, an edit the server owns", typeCode: "AB", description: "Synthetic" },
    { name: "a zero code, an edit the server owns", typeCode: "00", description: "Synthetic" },
  ])("passes $name unchanged", ({ typeCode, description }) => {
    expect(cotrtupSchema.safeParse({ typeCode, description })).toEqual({
      success: true,
      data: { typeCode, description },
    });
  });

  // audit: COTRTUP-013
  it("accepts a type code of the map's 2 characters and rejects one of 3", () => {
    expect(COTRTUP_LENGTHS.typeCode).toBe(2);
    expect(outcome(cotrtupSchema.safeParse({ typeCode: "12", description: "Synthetic" }))).toEqual(
      { success: true, issues: [] },
    );
    expect(
      outcome(cotrtupSchema.safeParse({ typeCode: "123", description: "Synthetic" })),
    ).toEqual({
      success: false,
      issues: [{ path: ["typeCode"], message: TYPE_CODE_TOO_LONG }],
    });
  });

  // audit: COTRTUP-016
  it("accepts a description of the map's 50 characters and rejects one of 51", () => {
    expect(COTRTUP_LENGTHS.description).toBe(50);
    expect(
      outcome(cotrtupSchema.safeParse({ typeCode: "01", description: FULL_DESCRIPTION })),
    ).toEqual({ success: true, issues: [] });
    expect(
      outcome(cotrtupSchema.safeParse({ typeCode: "01", description: "D".repeat(51) })),
    ).toEqual({
      success: false,
      issues: [{ path: ["description"], message: DESCRIPTION_TOO_LONG }],
    });
  });

  // audit: COTRTUPC-005
  it.each(NOT_SUPPLIED_TYPE_CODES)("rejects the type code %j as not supplied", (typeCode) => {
    expect(outcome(cotrtupSchema.safeParse({ typeCode, description: "Synthetic" }))).toEqual({
      success: false,
      issues: [{ path: ["typeCode"], message: TYPE_CODE_NOT_SUPPLIED }],
    });
  });

  // audit: COTRTUPC-005
  it("accepts a space before an asterisk, which the padded comparison does not match", () => {
    expect(cotrtupSchema.safeParse({ typeCode: " *", description: "Synthetic" })).toEqual({
      success: true,
      data: { typeCode: " *", description: "Synthetic" },
    });
  });

  // `1245-EDIT-NUM-REQD` leaves `1200-EDIT-MAP-INPUTS` after a blank code, so the description
  // edits never run and the type code's message is the only one, whatever the description holds.
  // audit: COTRTUPC-005
  it.each([
    { name: "empty", description: "" },
    { name: "`*`", description: "*" },
    { name: "with punctuation", description: "Synthetic type!" },
  ])("reports only the type code when the description is $name", ({ description }) => {
    expect(outcome(cotrtupSchema.safeParse({ typeCode: "*", description }))).toEqual({
      success: false,
      issues: [{ path: ["typeCode"], message: TYPE_CODE_NOT_SUPPLIED }],
    });
  });

  // "First error wins" binds the program edits, which the one refinement adds at most one of
  // (above). The map lengths are zod field checks: zod lists every field issue of the object
  // before the issue its object-level refinement adds, so a too-long description comes first;
  // this pins the schema's actual order. The input cannot reach the schema from the screen, whose
  // maxLength of 50 stops the 51st character, and the program's own edit order is the server's.
  it("reports the description length before a type code that is not supplied", () => {
    expect(outcome(cotrtupSchema.safeParse({ typeCode: "", description: "D".repeat(51) }))).toEqual(
      {
        success: false,
        issues: [
          { path: ["description"], message: DESCRIPTION_TOO_LONG },
          { path: ["typeCode"], message: TYPE_CODE_NOT_SUPPLIED },
        ],
      },
    );
  });

  it("reports the type code length before the description length, in field order", () => {
    expect(
      outcome(cotrtupSchema.safeParse({ typeCode: "123", description: "D".repeat(51) })),
    ).toEqual({
      success: false,
      issues: [
        { path: ["typeCode"], message: TYPE_CODE_TOO_LONG },
        { path: ["description"], message: DESCRIPTION_TOO_LONG },
      ],
    });
  });
});
