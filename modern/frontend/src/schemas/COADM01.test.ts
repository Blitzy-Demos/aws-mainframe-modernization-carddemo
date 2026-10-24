import { describe, expect, it } from "vitest";
import { COADM01_LENGTHS, type Coadm01Values, coadm01Schema } from "./COADM01";

/**
 * Unit tests of the `COADM1A` form schema: the admin menu's one input, `OPTION`
 * (`LENGTH=2`, `NUM`, screens.md COADM01-025). Every expected length, issue code and message is
 * written by hand from that row and from zod's default messages, as the audit records no message
 * for the field; none is read from the schema. The option is never sent, so no `both` row exists
 * for this map. COADM01-025 and the COADM01C rows on the entry are `client` rows: the zero-fill
 * (COADM01C-015), the invalid-option edit (COADM01C-016), the transfer (COADM01C-017) and the
 * not-installed message (COADM01C-018, COADM01C-019) belong to the screen, and the values they
 * answer must pass this schema unchanged. The admin gate and the menu endpoint 3 serves are
 * `server` rows (COADM01C-001, COADM01C-005 to COADM01C-014). All values are synthetic.
 */

type ParseCase = { name: string; values: Coadm01Values };
type RejectCase = { name: string; option: string; code: string; message: string };

const LENGTH_MESSAGE = "String must contain at most 2 character(s)";
const DIGITS_MESSAGE = "Invalid";

describe("coadm01Schema", () => {
  it("holds the map LENGTH of the input", () => {
    expect(COADM01_LENGTHS).toEqual({ option: 2 });
  });

  it.each<ParseCase>([
    { name: "an empty entry", values: { option: "" } },
    { name: "one digit", values: { option: "6" } },
    { name: "two digits at the map LENGTH", values: { option: "06" } },
    { name: "an option above the count (the screen's edit)", values: { option: "07" } },
    { name: "a zero option (the screen's edit)", values: { option: "00" } },
  ])("parses $name unchanged", ({ values }) => {
    expect(coadm01Schema.safeParse(values)).toEqual({ success: true, data: values });
  });

  it.each<RejectCase>([
    {
      name: "three digits, one over the map LENGTH",
      option: "123",
      code: "too_big",
      message: LENGTH_MESSAGE,
    },
    {
      name: "a letter (NUM)",
      option: "x1",
      code: "invalid_string",
      message: DIGITS_MESSAGE,
    },
    {
      name: "a leading space (NUM)",
      option: " 6",
      code: "invalid_string",
      message: DIGITS_MESSAGE,
    },
  ])("rejects $name", ({ option, code, message }) => {
    const result = coadm01Schema.safeParse({ option });
    expect(result.success).toBe(false);
    expect(result.error?.issues).toHaveLength(1);
    expect(result.error?.issues[0]).toMatchObject({ code, path: ["option"], message });
  });

  // LENGTH and NUM are two attributes the terminal enforced on the one field, so an entry that
  // breaks both reports both issues, length first, as the schema declares them.
  it("reports the length issue and then the digits issue for an entry that breaks both", () => {
    const result = coadm01Schema.safeParse({ option: "x12" });
    expect(result.success).toBe(false);
    const issues = result.error?.issues.map(({ code, path, message }) => ({ code, path, message }));
    expect(issues).toEqual([
      { code: "too_big", path: ["option"], message: LENGTH_MESSAGE },
      { code: "invalid_string", path: ["option"], message: DIGITS_MESSAGE },
    ]);
  });
});
