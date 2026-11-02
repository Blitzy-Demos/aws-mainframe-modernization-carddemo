// Tests of the CTLI list filter schema (schemas/transaction-types.ts) against
// the server's TranTypeListQuery constraints: the description filter TRDESC
// accepts ASCII text only, with zod's own regex issue (the server answers it
// with an empty message), and the type code filter TRTYPE keeps its audited
// COTRTLIC text and its first place (V-COTRTLIC-001).
import { describe, expect, it } from "vitest";
import { z } from "zod";

import {
  TRAN_TYPE_LIST_MESSAGES,
  type TranTypeListFilterValues,
  tranTypeListFilterSchema,
} from "./transaction-types";

/** Every audited COTRTLIC text the schema module can report. */
const AUDITED: readonly string[] = Object.values(TRAN_TYPE_LIST_MESSAGES);

function issuesOf(values: TranTypeListFilterValues): z.ZodIssue[] {
  const result = tranTypeListFilterSchema.safeParse(values);
  return result.success ? [] : result.error.issues;
}

describe("tranTypeListFilterSchema", () => {
  it.each(["", "   ", "\u0000", "PURCHASE", "%pur_", "\u007f", "A".repeat(50)])(
    "accepts the ASCII description filter %j",
    (description) => {
      expect(issuesOf({ description })).toEqual([]);
    },
  );

  // COTRTLIC 1230-EDIT-DESC moves the filter into X(50) byte for byte, so a
  // non-ASCII filter could be cut short of the text entered.
  it.each([
    "\u00e9",
    "\u0080",
    "\u20ac",
    "\u{1f600}",
    `${"A".repeat(49)}\u00e9`,
    "\u00e9".repeat(50),
  ])("rejects the description filter %j with zod's regex issue", (description) => {
    const issues = issuesOf({ description });
    expect(issues).toHaveLength(1);
    expect(issues[0]).toMatchObject({
      path: ["description"],
      code: z.ZodIssueCode.invalid_string,
      validation: "regex",
    });
    expect(AUDITED).not.toContain(issues[0]?.message);
  });

  it("reports the type code filter before the description filter", () => {
    const issues = issuesOf({ type_code: "0A", description: "\u00e9" });
    expect(issues.map((issue) => issue.path[0])).toEqual(["type_code", "description"]);
    expect(issues[0]?.message).toBe(TRAN_TYPE_LIST_MESSAGES.typeCodeFilterNotTwoDigits);
  });
});
