import { describe, expect, it } from "vitest";
import { COUSR00_LENGTHS, type Cousr00Values, cousr00Schema } from "./COUSR00";

/**
 * Unit tests of the `COUSR0A` form schema (`COUSR00`). Lengths, paths and messages are written by
 * hand from `COUSR00.bms` and `modern/audit/screens.md`; the messages are zod's defaults, because
 * the audited row records none. All values are synthetic.
 *
 * `USRIDIN` (COUSR00-015) is the screen's only `both` row, so only its tests carry an audit tag.
 * The selectors are `client` rows, and which character selects a row (COUSR00C-007 to
 * COUSR00C-011) is the screen's navigation rule, which the schema does not repeat, so their tests
 * carry none.
 */

/** `count` blank selectors, with `value` in row `index` when a mark is given. */
function selectors(count: number, mark?: { index: number; value: string }): string[] {
  return Array.from({ length: count }, (_, index) => (index === mark?.index ? mark.value : ""));
}

/** A form value with the given search value and ten blank selectors unless others are given. */
function form(startUserId: string, selections: string[] = selectors(10)): Cousr00Values {
  return { startUserId, selections };
}

/** Asserts that `input` parses successfully and comes back unchanged. */
function expectParsedUnchanged(input: Cousr00Values): void {
  expect(cousr00Schema.safeParse(input)).toEqual({ success: true, data: input });
}

/** Asserts that `input` fails to parse and returns its issues. */
function issuesOf(input: Cousr00Values) {
  const result = cousr00Schema.safeParse(input);
  expect(result.success).toBe(false);
  return result.error?.issues ?? [];
}

/** The ten list rows, `SEL0001` to `SEL0010` (screen lines 10 to 19). */
const ROWS = Array.from({ length: 10 }, (_, index) => ({
  index,
  field: `SEL${String(index + 1).padStart(4, "0")}`,
}));

describe("COUSR00_LENGTHS", () => {
  it("holds the map lengths of USRIDIN and the selectors, and the ten list rows", () => {
    expect(COUSR00_LENGTHS).toEqual({ startUserId: 8, selection: 1, rows: 10 });
  });
});

describe("cousr00Schema", () => {
  it("parses a valid value set unchanged", () => {
    expectParsedUnchanged(form("TSTUSR01", selectors(10, { index: 0, value: "U" })));
  });

  describe("startUserId (USRIDIN)", () => {
    // audit: COUSR00-015
    it("rejects 9 characters, one past the map length", () => {
      const issues = issuesOf(form("TSTUSR012"));
      expect(issues).toHaveLength(1);
      expect(issues[0]).toMatchObject({
        path: ["startUserId"],
        message: "String must contain at most 8 character(s)",
      });
    });

    // audit: COUSR00-015
    it("accepts 8 characters, the map length", () => {
      expectParsedUnchanged(form("TSTUSR01"));
    });

    // A blank search browses from the lowest user ID; the program edits nothing else in the value,
    // so every value of up to 8 characters only positions the list (COUSR00C-012, a server row).
    it.each([
      ["a blank value", ""],
      ["a lower-case value with a space", "tst us1"],
    ])("accepts %s", (_case, startUserId) => {
      expectParsedUnchanged(form(startUserId));
    });
  });

  describe("selections (SEL0001 to SEL0010)", () => {
    it.each(ROWS)("accepts one character in $field", ({ index }) => {
      expectParsedUnchanged(form("", selectors(10, { index, value: "D" })));
    });

    it.each(ROWS)("rejects two characters in $field", ({ index }) => {
      const issues = issuesOf(form("", selectors(10, { index, value: "UD" })));
      expect(issues).toHaveLength(1);
      expect(issues[0]).toMatchObject({
        path: ["selections", index],
        message: "String must contain at most 1 character(s)",
      });
    });

    // Which code selects is the screen's rule, so the schema lets every character through.
    it.each(["U", "u", "D", "d", "S", "X", "1", " "])(
      "accepts the selector character %j",
      (value) => {
        expectParsedUnchanged(form("", selectors(10, { index: 4, value })));
      },
    );

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
