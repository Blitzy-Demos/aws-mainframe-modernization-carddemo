import { describe, expect, it } from "vitest";
import {
  COTRN02_LENGTHS,
  type Cotrn02KeyValues,
  type Cotrn02Values,
  cotrn02KeySchema,
  cotrn02Schema,
} from "./COTRN02";

/**
 * Unit tests of the `COTRN2A` form schemas. They prove the client half of the fourteen `both`
 * rows of `modern/audit/screens.md`, the map `LENGTH` of every input, with zod's default message,
 * as the audit records none. Every COTRN02C edit is a `server` row endpoint 13 owns, and date
 * validity is halt H5, so values those edits would reject pass here and no program message is
 * asserted. Every expected length is written by hand from `app/bms/COTRN02.bms`. All values are
 * synthetic.
 */

/** Zod's default `max` message, which every length rejection carries. */
const tooLong = (length: number): string => `String must contain at most ${length} character(s)`;

/** A synthetic card number, assembled from parts so no literal has the shape of one. */
const CARD_NUM = ["4444", "3333", "2222", "1111"].join("");

const KEY_VALUES: Cotrn02KeyValues = { acctId: "00000000099", cardNum: CARD_NUM };

/** A complete form, every field filled at most to its map length, as typed. */
const FULL_VALUES: Cotrn02Values = {
  ...KEY_VALUES,
  typeCode: "01",
  categoryCode: "0001",
  source: "POS TERM",
  description: "SYNTHETIC PURCHASE",
  amount: "-00000012.34",
  origDate: "2023-01-15",
  procDate: "2023-01-16",
  merchantId: "000000001",
  merchantName: "SAMPLE MERCHANT",
  merchantCity: "SAMPLE CITY",
  merchantZip: "00001",
  confirm: "Y",
};

type KeyParseCase = { name: string; values: Cotrn02KeyValues };
type ParseCase = { name: string; values: Cotrn02Values };
type KeyLengthCase = { field: keyof Cotrn02KeyValues; length: number };
type LengthCase = { field: keyof Cotrn02Values; length: number };

describe("COTRN02_LENGTHS", () => {
  it("holds the map LENGTH of each input", () => {
    expect(COTRN02_LENGTHS).toEqual({
      acctId: 11,
      cardNum: 16,
      typeCode: 2,
      categoryCode: 4,
      source: 10,
      description: 60,
      amount: 12,
      origDate: 10,
      procDate: 10,
      merchantId: 9,
      merchantName: 30,
      merchantCity: 25,
      merchantZip: 10,
      confirm: 1,
    });
  });
});

describe("cotrn02KeySchema", () => {
  // COTRN02C's key edits (COTRN02C-009, -010, -015) are `server` rows, so blank and
  // non-numeric keys are sent as typed.
  it.each<KeyParseCase>([
    { name: "a typed account and card number", values: KEY_VALUES },
    { name: "an account only", values: { acctId: KEY_VALUES.acctId, cardNum: "" } },
    { name: "a card number only", values: { acctId: "", cardNum: CARD_NUM } },
    { name: "a blank account and card number", values: { acctId: "", cardNum: "" } },
    { name: "a non-numeric account and card number", values: { acctId: "ABC", cardNum: "XYZ" } },
  ])("parses $name unchanged", ({ values }) => {
    expect(cotrn02KeySchema.safeParse(values)).toEqual({ success: true, data: values });
  });

  it.each<KeyLengthCase>([
    // audit: COTRN02-013
    { field: "acctId", length: 11 },
    // audit: COTRN02-017
    { field: "cardNum", length: 16 },
  ])("accepts $field at $length characters and rejects one more", ({ field, length }) => {
    const boundary = cotrn02KeySchema.safeParse({ ...KEY_VALUES, [field]: "9".repeat(length) });
    expect(boundary.success).toBe(true);
    const over = cotrn02KeySchema.safeParse({ ...KEY_VALUES, [field]: "9".repeat(length + 1) });
    expect(over.success).toBe(false);
    expect(over.error?.issues).toHaveLength(1);
    expect(over.error?.issues[0]).toMatchObject({ path: [field], message: tooLong(length) });
  });
});

describe("cotrn02Schema", () => {
  // Every data-field and confirmation edit (COTRN02C-020 to COTRN02C-042) is a `server` row, and
  // date validity is halt H5, so the values below parse exactly as typed: nothing is trimmed,
  // converted or case-changed.
  it.each<ParseCase>([
    { name: "a complete form with confirmation Y", values: FULL_VALUES },
    { name: "confirmation y", values: { ...FULL_VALUES, confirm: "y" } },
    { name: "confirmation N", values: { ...FULL_VALUES, confirm: "N" } },
    { name: "confirmation n", values: { ...FULL_VALUES, confirm: "n" } },
    { name: "a blank confirmation", values: { ...FULL_VALUES, confirm: "" } },
    { name: "confirmation X", values: { ...FULL_VALUES, confirm: "X" } },
    {
      name: "an original date that is no calendar date (halt H5)",
      values: { ...FULL_VALUES, origDate: "2023-02-30" },
    },
    {
      name: "data fields COTRN02C edits would reject",
      values: {
        ...FULL_VALUES,
        typeCode: "",
        categoryCode: "AB",
        amount: "12.34",
        merchantId: "MERCHANT",
      },
    },
  ])("parses $name unchanged", ({ values }) => {
    expect(cotrn02Schema.safeParse(values)).toEqual({ success: true, data: values });
  });

  it.each<LengthCase>([
    // audit: COTRN02-013
    { field: "acctId", length: 11 },
    // audit: COTRN02-017
    { field: "cardNum", length: 16 },
    // audit: COTRN02-021
    { field: "typeCode", length: 2 },
    // audit: COTRN02-024
    { field: "categoryCode", length: 4 },
    // audit: COTRN02-027
    { field: "source", length: 10 },
    // audit: COTRN02-030
    { field: "description", length: 60 },
    // audit: COTRN02-033
    { field: "amount", length: 12 },
    // audit: COTRN02-036
    { field: "origDate", length: 10 },
    // audit: COTRN02-039
    { field: "procDate", length: 10 },
    // audit: COTRN02-045
    { field: "merchantId", length: 9 },
    // audit: COTRN02-048
    { field: "merchantName", length: 30 },
    // audit: COTRN02-051
    { field: "merchantCity", length: 25 },
    // audit: COTRN02-054
    { field: "merchantZip", length: 10 },
    // audit: COTRN02-057
    { field: "confirm", length: 1 },
  ])("accepts $field at $length characters and rejects one more", ({ field, length }) => {
    const boundary = cotrn02Schema.safeParse({ ...FULL_VALUES, [field]: "A".repeat(length) });
    expect(boundary.success).toBe(true);
    const over = cotrn02Schema.safeParse({ ...FULL_VALUES, [field]: "A".repeat(length + 1) });
    expect(over.success).toBe(false);
    expect(over.error?.issues).toHaveLength(1);
    expect(over.error?.issues[0]).toMatchObject({ path: [field], message: tooLong(length) });
  });
});
