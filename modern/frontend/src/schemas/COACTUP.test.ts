import { describe, expect, it } from "vitest";
import {
  COACTUP_LENGTHS,
  type CoactupKeyValues,
  type CoactupValues,
  coactupKeySchema,
  coactupSchema,
} from "./COACTUP";

/**
 * Client half of the `both` rows the two schemas of map `CACTUPA` repeat: the map `LENGTH` of
 * the 43 inputs the screen sends (`COACTUP-013` to `COACTUP-120` of `modern/audit/screens.md`),
 * whose rejections carry zod's default message because no row records one. Every expected
 * length is written by hand from the row's `LENGTH` operand. `modern/audit/online/COACTUPC.md` and
 * `modern/audit/online/CSUTLDTC.md` have no `both` row: the account, field, date-part, SSN and
 * change-check edits are the endpoint's, and calendar validity is halt H5, so inputs those edits
 * reject pass here and no program message is asserted. The SSN parts are optional (E32). Every
 * value is synthetic, and no test title or message carries a fixture value.
 */

/** Zod's default `max` message, which every length rejection carries. */
const tooLong = (length: number): string => `String must contain at most ${length} character(s)`;

/** A synthetic account number of the map's 11 characters, outside the fixture accounts. */
const ACCT_ID = "99999999901";

/** Every field of the update form except the SSN parts, which the screen sends only re-entered. */
const SSN_OMITTED: CoactupValues = {
  acctId: ACCT_ID,
  activeStatus: "Y",
  openYear: "2015",
  openMonth: "06",
  openDay: "15",
  creditLimit: "5,000.00",
  expiryYear: "2031",
  expiryMonth: "12",
  expiryDay: "31",
  cashCreditLimit: "1,500.00",
  reissueYear: "2025",
  reissueMonth: "01",
  reissueDay: "01",
  currentBalance: "1,234.56",
  currentCycleCredit: "0.00",
  groupId: "SYNGRP0001",
  currentCycleDebit: "250.00",
  custId: "000000901",
  dobYear: "1980",
  dobMonth: "07",
  dobDay: "04",
  ficoScore: "750",
  firstName: "Synthetic",
  middleName: "Q",
  lastName: "Sample",
  addressLine1: "1 Synthetic Way",
  state: "NY",
  addressLine2: "Suite 100",
  zip: "10001",
  city: "Testville",
  country: "USA",
  phone1Area: "555",
  phone1Prefix: "010",
  phone1Line: "0199",
  govtId: "SYN-GOVT-0001",
  phone2Area: "555",
  phone2Prefix: "010",
  phone2Line: "0198",
  eftAccountId: "9900000001",
  primaryCardHolder: "Y",
};

/** Synthetic SSN parts as an operator re-enters them, each at its map length. */
const SSN_PARTS = { ssnPart1: "321", ssnPart2: "54", ssnPart3: "9876" };

/** A complete update form: every field typed, the SSN re-entered. */
const VALID: CoactupValues = { ...SSN_OMITTED, ...SSN_PARTS };

type ParseCase = { name: string; values: CoactupValues };
type LengthCase = { field: keyof CoactupValues; length: number };

describe("coactupKeySchema", () => {
  it.each<CoactupKeyValues>([
    { acctId: ACCT_ID },
    { acctId: "" },
    { acctId: "0".repeat(11) },
    { acctId: "12A" },
  ])("parses the account number $acctId unchanged, its edits being the server's", (values) => {
    expect(coactupKeySchema.safeParse(values)).toEqual({ success: true, data: values });
  });

  // audit: COACTUP-013
  it("accepts an account number of the map's 11 characters and rejects one of 12", () => {
    expect(COACTUP_LENGTHS.acctId).toBe(11);
    const boundary = { acctId: "9".repeat(11) };
    expect(coactupKeySchema.safeParse(boundary)).toEqual({ success: true, data: boundary });
    const over = coactupKeySchema.safeParse({ acctId: "9".repeat(12) });
    expect(over.success).toBe(false);
    expect(over.error?.issues.map(({ path, message }) => ({ path, message }))).toEqual([
      { path: ["acctId"], message: tooLong(11) },
    ]);
  });
});

describe("coactupSchema", () => {
  it("holds a map LENGTH for exactly the 43 fields of the form, in map order", () => {
    expect(Object.keys(COACTUP_LENGTHS)).toEqual(Object.keys(coactupSchema.shape));
    expect(Object.keys(COACTUP_LENGTHS)).toHaveLength(43);
  });

  // Calendar validity (halt H5) and every COACTUPC edit are the server's, so the impossible dates
  // and the values those edits reject pass with the complete form.
  it.each<ParseCase>([
    { name: "a complete update form", values: VALID },
    {
      name: "every field empty",
      values: { ...VALID, ...Object.fromEntries(Object.keys(VALID).map((field) => [field, ""])) },
    },
    {
      name: "an opening date of 30 February",
      values: { ...VALID, openYear: "2023", openMonth: "02", openDay: "30" },
    },
    {
      name: "an expiry date of 31 April",
      values: { ...VALID, expiryYear: "2031", expiryMonth: "04", expiryDay: "31" },
    },
    {
      name: "a reissue date of 29 February in a common year",
      values: { ...VALID, reissueYear: "2023", reissueMonth: "02", reissueDay: "29" },
    },
    {
      name: "a date of birth of 31 June",
      values: { ...VALID, dobYear: "1990", dobMonth: "06", dobDay: "31" },
    },
    { name: "an all-zero account number", values: { ...VALID, acctId: "0".repeat(11) } },
    { name: "an account status other than Y or N", values: { ...VALID, activeStatus: "X" } },
    { name: "an amount NUMVAL-C rejects", values: { ...VALID, creditLimit: "1.2.3" } },
    { name: "a first name with a digit", values: { ...VALID, firstName: "Syn7" } },
    { name: "a blank address line 1", values: { ...VALID, addressLine1: "" } },
    { name: "a FICO score above 850", values: { ...VALID, ficoScore: "999" } },
    { name: "a ZIP code with a letter", values: { ...VALID, zip: "1000A" } },
    { name: "a lower-case state code", values: { ...VALID, state: "ny" } },
    { name: "an all-zero area code", values: { ...VALID, phone1Area: "000" } },
    { name: "an SSN part 1 of 666", values: { ...VALID, ssnPart1: "666" } },
  ])("parses $name unchanged", ({ values }) => {
    expect(coactupSchema.safeParse(values)).toEqual({ success: true, data: values });
  });

  it.each<LengthCase>([
    { field: "acctId", length: 11 }, // audit: COACTUP-013
    { field: "activeStatus", length: 1 }, // audit: COACTUP-016
    { field: "openYear", length: 4 }, // audit: COACTUP-019
    { field: "openMonth", length: 2 }, // audit: COACTUP-021
    { field: "openDay", length: 2 }, // audit: COACTUP-023
    { field: "creditLimit", length: 15 }, // audit: COACTUP-026
    { field: "expiryYear", length: 4 }, // audit: COACTUP-029
    { field: "expiryMonth", length: 2 }, // audit: COACTUP-031
    { field: "expiryDay", length: 2 }, // audit: COACTUP-033
    { field: "cashCreditLimit", length: 15 }, // audit: COACTUP-036
    { field: "reissueYear", length: 4 }, // audit: COACTUP-039
    { field: "reissueMonth", length: 2 }, // audit: COACTUP-041
    { field: "reissueDay", length: 2 }, // audit: COACTUP-043
    { field: "currentBalance", length: 15 }, // audit: COACTUP-046
    { field: "currentCycleCredit", length: 15 }, // audit: COACTUP-049
    { field: "groupId", length: 10 }, // audit: COACTUP-052
    { field: "currentCycleDebit", length: 15 }, // audit: COACTUP-055
    { field: "custId", length: 9 }, // audit: COACTUP-059
    { field: "ssnPart1", length: 3 }, // audit: COACTUP-062
    { field: "ssnPart2", length: 2 }, // audit: COACTUP-064
    { field: "ssnPart3", length: 4 }, // audit: COACTUP-066
    { field: "dobYear", length: 4 }, // audit: COACTUP-069
    { field: "dobMonth", length: 2 }, // audit: COACTUP-071
    { field: "dobDay", length: 2 }, // audit: COACTUP-073
    { field: "ficoScore", length: 3 }, // audit: COACTUP-076
    { field: "firstName", length: 25 }, // audit: COACTUP-081
    { field: "middleName", length: 25 }, // audit: COACTUP-083
    { field: "lastName", length: 25 }, // audit: COACTUP-085
    { field: "addressLine1", length: 50 }, // audit: COACTUP-087
    { field: "state", length: 2 }, // audit: COACTUP-090
    { field: "addressLine2", length: 50 }, // audit: COACTUP-092
    { field: "zip", length: 5 }, // audit: COACTUP-095
    { field: "city", length: 50 }, // audit: COACTUP-098
    { field: "country", length: 3 }, // audit: COACTUP-101
    { field: "phone1Area", length: 3 }, // audit: COACTUP-104
    { field: "phone1Prefix", length: 3 }, // audit: COACTUP-105
    { field: "phone1Line", length: 4 }, // audit: COACTUP-106
    { field: "govtId", length: 20 }, // audit: COACTUP-109
    { field: "phone2Area", length: 3 }, // audit: COACTUP-112
    { field: "phone2Prefix", length: 3 }, // audit: COACTUP-113
    { field: "phone2Line", length: 4 }, // audit: COACTUP-114
    { field: "eftAccountId", length: 10 }, // audit: COACTUP-117
    { field: "primaryCardHolder", length: 1 }, // audit: COACTUP-120
  ])(
    "accepts $field at its map LENGTH $length and rejects one character more",
    ({ field, length }) => {
      expect(COACTUP_LENGTHS[field]).toBe(length);
      const boundary = { ...VALID, [field]: "9".repeat(length) };
      expect(coactupSchema.safeParse(boundary)).toEqual({ success: true, data: boundary });
      const over = coactupSchema.safeParse({ ...VALID, [field]: "9".repeat(length + 1) });
      expect(over.success).toBe(false);
      expect(over.error?.issues).toHaveLength(1);
      expect(over.error?.issues[0]).toMatchObject({ path: [field], message: tooLong(length) });
    },
  );

  // Each map LENGTH is a field attribute the terminal enforced on its own field, not a program
  // edit whose first error stands, so every over-length field is reported, in map order.
  it("reports one issue for each field over its map LENGTH, in map order", () => {
    const lengths = Object.entries(COACTUP_LENGTHS);
    const over = Object.fromEntries(
      lengths.map(([field, length]) => [field, "9".repeat(length + 1)]),
    );
    const result = coactupSchema.safeParse(over);
    expect(result.success).toBe(false);
    expect(result.error?.issues.map(({ path, message }) => ({ path, message }))).toEqual(
      lengths.map(([field, length]) => ({ path: [field], message: tooLong(length) })),
    );
  });

  // The SSN is shown redacted and sent only when re-entered; omitted means unchanged (E32).
  it.each<ParseCase>([
    { name: "all three SSN parts omitted", values: SSN_OMITTED },
    {
      name: "all three SSN parts undefined",
      values: { ...SSN_OMITTED, ssnPart1: undefined, ssnPart2: undefined, ssnPart3: undefined },
    },
    {
      name: "all three SSN parts empty",
      values: { ...SSN_OMITTED, ssnPart1: "", ssnPart2: "", ssnPart3: "" },
    },
    {
      name: "the redacted SSN parts a fetch shows",
      values: { ...SSN_OMITTED, ssnPart1: "***", ssnPart2: "**", ssnPart3: SSN_PARTS.ssnPart3 },
    },
  ])("passes $name with no SSN issue", ({ values }) => {
    expect(coactupSchema.safeParse(values)).toEqual({ success: true, data: values });
    const otherFieldOver = coactupSchema.safeParse({ ...values, firstName: "A".repeat(26) });
    expect(otherFieldOver.error?.issues.map(({ path, message }) => ({ path, message }))).toEqual([
      { path: ["firstName"], message: tooLong(25) },
    ]);
  });
});
