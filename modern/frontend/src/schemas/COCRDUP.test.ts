import { describe, expect, it } from "vitest";
import { COCRDUP_LENGTHS, cocrdupKeySchema, cocrdupSchema } from "./COCRDUP";

/**
 * Client half of the `both` rows of map COCRDUP: the map `LENGTH` of each field the screen
 * sends (`modern/audit/screens.md`). Every expected length is written by hand from
 * `app/bms/COCRDUP.bms`; a rejection carries zod's own message, because no row records one.
 * COCRDUPC's edits are Layer `server`, so inputs those edits reject pass both schemas.
 * All values are synthetic.
 */

/** Zod's default `max` message, which every length rejection carries. */
const tooLong = (length: number): string => `String must contain at most ${length} character(s)`;

/** A synthetic card number, assembled so no literal has the shape of one. */
const CARD_NUM = "0".repeat(12) + "4321";

const KEY_VALUES = { acctId: "00000000099", cardNum: CARD_NUM };

const FULL_VALUES = {
  acctId: "00000000099",
  cardNum: "*".repeat(12) + "4321",
  embossedName: "JANE Q SAMPLE",
  activeStatus: "Y",
  expiryMonth: "03",
  expiryYear: "2031",
};

/** A zod issue reduced to the two parts the screen shows: the field it names and its text. */
type ParseIssue = { path: (string | number)[]; message: string };

/** The path and message of each issue of a parse, in zod's order; empty when it succeeds. */
function issuesOf(result: { error?: { issues: ParseIssue[] } }): ParseIssue[] {
  return result.error?.issues.map(({ path, message }) => ({ path, message })) ?? [];
}

describe("COCRDUP_LENGTHS", () => {
  it("holds the map LENGTH of each field the screen sends", () => {
    expect(COCRDUP_LENGTHS).toEqual({
      acctId: 11,
      cardNum: 16,
      embossedName: 50,
      activeStatus: 1,
      expiryMonth: 2,
      expiryYear: 4,
    });
  });
});

describe("cocrdupKeySchema", () => {
  it("parses a valid search key set unchanged", () => {
    expect(cocrdupKeySchema.parse(KEY_VALUES)).toEqual(KEY_VALUES);
  });

  // audit: COCRDUP-013
  // audit: COCRDUP-016
  it.each([
    ["acctId", 11],
    ["cardNum", 16],
  ] as const)("accepts %s at its map LENGTH %i and rejects one more", (field, length) => {
    expect(cocrdupKeySchema.safeParse({ ...KEY_VALUES, [field]: "1".repeat(length) }).success).toBe(
      true,
    );
    const result = cocrdupKeySchema.safeParse({ ...KEY_VALUES, [field]: "1".repeat(length + 1) });
    expect(result.success).toBe(false);
    expect(result.error?.issues[0]?.path).toEqual([field]);
    expect(result.error?.issues[0]?.message).toBe(tooLong(length));
  });

  it("reports the account before the card when both lengths are exceeded", () => {
    const overLong = { acctId: "1".repeat(12), cardNum: "1".repeat(17) };
    expect(issuesOf(cocrdupKeySchema.safeParse(overLong))).toEqual([
      { path: ["acctId"], message: tooLong(11) },
      { path: ["cardNum"], message: tooLong(16) },
    ]);
  });

  it("leaves blank and non-numeric keys to the server", () => {
    const keys = { acctId: "", cardNum: "ABCD" };
    expect(cocrdupKeySchema.parse(keys)).toEqual(keys);
  });
});

describe("cocrdupSchema", () => {
  it("parses a valid fetched screen unchanged", () => {
    expect(cocrdupSchema.parse(FULL_VALUES)).toEqual(FULL_VALUES);
  });

  // audit: COCRDUP-019
  // audit: COCRDUP-022
  // audit: COCRDUP-025
  // audit: COCRDUP-027
  it.each([
    ["acctId", 11],
    ["cardNum", 16],
    ["embossedName", 50],
    ["activeStatus", 1],
    ["expiryMonth", 2],
    ["expiryYear", 4],
  ] as const)("accepts %s at its map LENGTH %i and rejects one more", (field, length) => {
    expect(cocrdupSchema.safeParse({ ...FULL_VALUES, [field]: "A".repeat(length) }).success).toBe(
      true,
    );
    const result = cocrdupSchema.safeParse({ ...FULL_VALUES, [field]: "A".repeat(length + 1) });
    expect(result.success).toBe(false);
    expect(result.error?.issues[0]?.path).toEqual([field]);
    expect(result.error?.issues[0]?.message).toBe(tooLong(length));
  });

  it("reports every exceeded length in the map's field order, the first field first", () => {
    const overLong = {
      acctId: "A".repeat(12),
      cardNum: "A".repeat(17),
      embossedName: "A".repeat(51),
      activeStatus: "AA",
      expiryMonth: "AAA",
      expiryYear: "AAAAA",
    };
    expect(issuesOf(cocrdupSchema.safeParse(overLong))).toEqual([
      { path: ["acctId"], message: tooLong(11) },
      { path: ["cardNum"], message: tooLong(16) },
      { path: ["embossedName"], message: tooLong(50) },
      { path: ["activeStatus"], message: tooLong(1) },
      { path: ["expiryMonth"], message: tooLong(2) },
      { path: ["expiryYear"], message: tooLong(4) },
    ]);
    expect(
      issuesOf(cocrdupSchema.safeParse({ ...FULL_VALUES, activeStatus: "YY", expiryYear: "20311" })),
    ).toEqual([
      { path: ["activeStatus"], message: tooLong(1) },
      { path: ["expiryYear"], message: tooLong(4) },
    ]);
  });

  it("leaves blank details to the server, which owns their messages", () => {
    const blanks = {
      ...FULL_VALUES,
      embossedName: "",
      activeStatus: "",
      expiryMonth: "00",
      expiryYear: "0000",
    };
    expect(cocrdupSchema.parse(blanks)).toEqual(blanks);
  });

  it("leaves COCRDUPC's edits to the server", () => {
    const edits = {
      ...FULL_VALUES,
      embossedName: "JANE Q SAMPLE 2",
      activeStatus: "X",
      expiryMonth: "13",
      expiryYear: "AB",
    };
    expect(cocrdupSchema.parse(edits)).toEqual(edits);
  });
});
