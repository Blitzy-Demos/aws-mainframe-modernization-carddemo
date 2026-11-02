// Tests of the input rules the form schemas share (schemas/primitives.ts):
// the COBOL `SPACES OR LOW-VALUES` blank test of received map text and the
// `YYYY-MM-DD` shape the date text inputs ask for.
import { describe, expect, it } from "vitest";

import { DATE_PATTERN, isSpacesOrLowValues } from "./primitives";

describe("isSpacesOrLowValues", () => {
  it.each<[name: string, value: string, blank: boolean]>([
    ["the empty text", "", true],
    ["spaces only", "   ", true],
    ["NUL characters only", "\u0000\u0000", true],
    ["a mix of a space and NUL", " \u0000", false],
    ["a tab", "\t", false],
    ["a no-break space", "\u00a0", false],
    ["a letter", "A", false],
    ["a letter between spaces, untrimmed", " A ", false],
  ])("%s → %s", (_name, value, blank) => {
    expect(isSpacesOrLowValues(value)).toBe(blank);
  });
});

describe("DATE_PATTERN", () => {
  it.each<[value: string, matches: boolean]>([
    ["2022-07-01", true],
    ["2022/07/01", false],
    ["20220701", false],
    ["2022-7-01", false],
    ["2022-07-01 ", false],
  ])("%j → %s", (value, matches) => {
    expect(DATE_PATTERN.test(value)).toBe(matches);
  });
});
