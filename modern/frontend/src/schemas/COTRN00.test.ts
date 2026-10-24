import { describe, expect, it } from "vitest";
import { COTRN00_LENGTHS, type Cotrn00Values, cotrn00Schema } from "./COTRN00";

/**
 * Unit tests of the `COTRN0A` form schema (`COTRN00`). Lengths, paths and messages are written by
 * hand from `COTRN00.bms` and `modern/audit/screens.md`; the messages are zod's defaults, because
 * the audited row records none. All values are synthetic.
 *
 * `TRNIDIN` (COTRN00-015) is the screen's only `both` row, so only its tests carry an audit tag.
 * The selectors are `client` rows and the numeric edit of the search value (COTRN00C-013) is a
 * `server` row, so their tests carry none.
 */

/** `count` blank selectors, with `value` in row `index` when a mark is given. */
function selectors(count: number, mark?: { index: number; value: string }): string[] {
  return Array.from({ length: count }, (_, index) => (index === mark?.index ? mark.value : ""));
}

/** A form value with the given search value and ten blank selectors unless others are given. */
function form(startTranId: string, selections: string[] = selectors(10)): Cotrn00Values {
  return { startTranId, selections };
}

/** Asserts that `input` parses successfully and comes back unchanged. */
function expectParsedUnchanged(input: Cotrn00Values): void {
  expect(cotrn00Schema.safeParse(input)).toEqual({ success: true, data: input });
}

/** Asserts that `input` fails to parse and returns its issues. */
function issuesOf(input: Cotrn00Values) {
  const result = cotrn00Schema.safeParse(input);
  expect(result.success).toBe(false);
  return result.error?.issues ?? [];
}

/** The ten list rows, `SEL0001` to `SEL0010` (screen lines 10 to 19). */
const ROWS = Array.from({ length: 10 }, (_, index) => ({
  index,
  field: `SEL${String(index + 1).padStart(4, "0")}`,
}));

describe("COTRN00_LENGTHS", () => {
  it("holds the map lengths of TRNIDIN and the selectors, and the ten list rows", () => {
    expect(COTRN00_LENGTHS).toEqual({ startTranId: 16, selection: 1, rows: 10 });
  });
});

describe("cotrn00Schema", () => {
  it("parses a valid value set unchanged", () => {
    expectParsedUnchanged(form("0000000000000001", selectors(10, { index: 0, value: "S" })));
  });

  describe("startTranId (TRNIDIN)", () => {
    // audit: COTRN00-015
    it("rejects 17 characters, one past the map length", () => {
      const issues = issuesOf(form("00000000000000001"));
      expect(issues).toHaveLength(1);
      expect(issues[0]).toMatchObject({
        path: ["startTranId"],
        message: "String must contain at most 16 character(s)",
      });
    });

    // audit: COTRN00-015
    it("accepts 16 characters, the map length", () => {
      expectParsedUnchanged(form("0000000000000001"));
    });

    // A blank search starts at the lowest ID; a non-numeric value reaches the server, whose edit
    // (COTRN00C-013) owns the message.
    it.each([
      ["a blank value", ""],
      ["a non-numeric 16-character value", "00000000000000AB"],
    ])("accepts %s", (_case, startTranId) => {
      expectParsedUnchanged(form(startTranId));
    });
  });

  describe("selections (SEL0001 to SEL0010)", () => {
    it.each(ROWS)("accepts one character in $field", ({ index }) => {
      expectParsedUnchanged(form("", selectors(10, { index, value: "S" })));
    });

    it.each(ROWS)("rejects two characters in $field", ({ index }) => {
      const issues = issuesOf(form("", selectors(10, { index, value: "SS" })));
      expect(issues).toHaveLength(1);
      expect(issues[0]).toMatchObject({
        path: ["selections", index],
        message: "String must contain at most 1 character(s)",
      });
    });

    // Which code selects is the screen's rule, so the schema lets every character through.
    it.each(["S", "s", "X", "1", " "])("accepts the selector character %j", (value) => {
      expectParsedUnchanged(form("", selectors(10, { index: 4, value })));
    });

    it.each([9, 11])("rejects %i selectors, since the list has exactly ten rows", (count) => {
      const issues = issuesOf(form("", selectors(count)));
      expect(issues).toHaveLength(1);
      expect(issues[0]).toMatchObject({
        path: ["selections"],
        message: "Array must contain exactly 10 element(s)",
      });
    });
  });
});
