import { describe, expect, it } from "vitest";
import { COMEN01_LENGTHS, type Comen01Values, comen01Schema } from "./COMEN01";

/**
 * Unit tests of the `COMEN1A` form schema. The schema repeats only the map `LENGTH` and `NUM` of
 * `OPTION` (screens.md `COMEN01-025`), with zod's default messages, as the audit records none.
 * That row is a `client` row, and online/COMEN01C.md has no `both` row because the option is never
 * sent, so no test here carries an audit tag. COMEN01C's ENTER edits are `client` rows the screen
 * applies with the endpoint 2 response (the zero fill of `COMEN01C-008`, the invalid option of
 * `COMEN01C-009` and `COMEN01C-010`, the refusals of `COMEN01C-012` and `COMEN01C-016`), so an
 * empty entry and an out-of-range number pass here. All values are synthetic.
 */

/** zod's default `max` message at the map `LENGTH` of `OPTION`. */
const LENGTH_MESSAGE = "String must contain at most 2 character(s)";

/** zod's default `regex` message, for an entry that is not digits only (`NUM`). */
const NUM_MESSAGE = "Invalid";

type ParseCase = { name: string; values: Comen01Values };
type RejectCase = { name: string; option: string; message: string };

describe("comen01Schema", () => {
  it("holds the map LENGTH of the option input", () => {
    expect(COMEN01_LENGTHS).toEqual({ option: 2 });
  });

  it.each<ParseCase>([
    { name: "an empty option", values: { option: "" } },
    { name: "a one-digit option", values: { option: "1" } },
    { name: "a zero-filled option", values: { option: "01" } },
    { name: "a two-digit option at the map LENGTH", values: { option: "11" } },
    { name: "an out-of-range numeric option", values: { option: "99" } },
  ])("parses $name unchanged", ({ values }) => {
    expect(comen01Schema.safeParse(values)).toEqual({ success: true, data: values });
  });

  it.each<RejectCase>([
    { name: "three digits, one past the map LENGTH", option: "123", message: LENGTH_MESSAGE },
    { name: "a digit and a letter", option: "1a", message: NUM_MESSAGE },
    { name: "a letter", option: "A", message: NUM_MESSAGE },
    { name: "a space before a digit", option: " 1", message: NUM_MESSAGE },
  ])("rejects $name with one issue on the option", ({ option, message }) => {
    const result = comen01Schema.safeParse({ option });
    expect(result.success).toBe(false);
    expect(result.error?.issues).toHaveLength(1);
    expect(result.error?.issues[0]).toMatchObject({ path: ["option"], message });
  });

  // LENGTH and NUM are two attributes the terminal enforced on the one field, so an entry that
  // breaks both reports both issues, in the schema's order: the length first.
  it("reports the length and then the digits when an entry breaks both", () => {
    const result = comen01Schema.safeParse({ option: "abc" });
    expect(result.success).toBe(false);
    expect(result.error?.issues.map(({ path, message }) => ({ path, message }))).toEqual([
      { path: ["option"], message: LENGTH_MESSAGE },
      { path: ["option"], message: NUM_MESSAGE },
    ]);
  });
});
