// Tests of the CTLI schemas (schemas/transaction-types.ts) against the server's
// TranTypeListQuery and TranTypeUpdateRequest constraints. The description
// filter TRDESC holds only its length, with zod's own issue (the server answers
// it with an empty message), and is otherwise searched as entered; the type
// code filter TRTYPE keeps its audited COTRTLIC text and its first place
// (V-COTRTLIC-001). The inline update's row description gets the COTRTLIC
// 1240-EDIT-ALPHANUM-REQD texts for any entry within its length
// (V-COTRTLIC-013, V-COTRTLIC-014), and its row key keeps the ASCII check of
// the {id} segment. Every length of these schemas and of the CTTU add and
// update counts code points, as the server's validator length and the
// OpenAPI maxLength count Unicode scalar values.
import { describe, expect, it } from "vitest";
import { z } from "zod";

import {
  TRAN_TYPE_LIST_MESSAGES,
  TRAN_TYPE_UPSERT_MESSAGES,
  type TranTypeFormValues,
  type TranTypeInlineEditValues,
  type TranTypeListFilterValues,
  tranTypeCreateSchema,
  tranTypeInlineEditSchema,
  tranTypeListFilterSchema,
  tranTypeUpdateSchema,
} from "./transaction-types";

// Every audited COTRTLIC text the schema module can report.
const AUDITED: readonly string[] = Object.values(TRAN_TYPE_LIST_MESSAGES);

// 50 characters, 51 bytes: the last one straddles the server's 50-byte cut.
const SPLIT_AT_BYTE_50 = `${"A".repeat(49)}\u00e9`;

function issuesOf(values: TranTypeListFilterValues): z.ZodIssue[] {
  const result = tranTypeListFilterSchema.safeParse(values);
  return result.success ? [] : result.error.issues;
}

function inlineIssuesOf(values: TranTypeInlineEditValues): z.ZodIssue[] {
  const result = tranTypeInlineEditSchema.safeParse(values);
  return result.success ? [] : result.error.issues;
}

function createIssuesOf(values: TranTypeFormValues): z.ZodIssue[] {
  const result = tranTypeCreateSchema.safeParse(values);
  return result.success ? [] : result.error.issues;
}

function updateIssuesOf(
  values: z.input<typeof tranTypeUpdateSchema>,
): z.ZodIssue[] {
  const result = tranTypeUpdateSchema.safeParse(values);
  return result.success ? [] : result.error.issues;
}

describe("tranTypeListFilterSchema", () => {
  // COTRTLIC 1230-EDIT-DESC applies no content edit, and the server searches
  // the filter as entered, so a filter matching no type is its 404
  // (V-COTRTLIC-002).
  it.each([
    "",
    "   ",
    "\u0000",
    "PURCHASE",
    "%pur_",
    "\u007f",
    "A".repeat(50),
    "\u00e9",
    "\u0080",
    "\u20ac",
    "\u{1f600}",
    "Caf\u00e9",
    SPLIT_AT_BYTE_50,
    "\u00e9".repeat(50),
  ])("accepts the description filter %j", (description) => {
    expect(issuesOf({ description })).toEqual([]);
  });

  it.each(["A".repeat(51), "\u00e9".repeat(51)])(
    "rejects the over-length description filter %j with zod's own issue",
    (description) => {
      const issues = issuesOf({ description });
      expect(issues).toHaveLength(1);
      expect(issues[0]).toMatchObject({
        path: ["description"],
        code: z.ZodIssueCode.too_big,
      });
      expect(AUDITED).not.toContain(issues[0]?.message);
    },
  );

  it("reports the type code filter before the description filter", () => {
    const issues = issuesOf({ type_code: "0A", description: "A".repeat(51) });
    expect(issues.map((issue) => issue.path[0])).toEqual(["type_code", "description"]);
    expect(issues[0]?.message).toBe(TRAN_TYPE_LIST_MESSAGES.typeCodeFilterNotTwoDigits);
  });
});

describe("tranTypeInlineEditSchema", () => {
  // COTRTLIC 1240-EDIT-ALPHANUM-REQD reads every character the server
  // receives, so a character outside ASCII gets the audited text
  // (V-COTRTLIC-014), as any other non-alphanumeric character does.
  it.each([
    "PURCHASE-RETAIL",
    " *",
    "Caf\u00e9",
    "Espa\u00f1a",
    "\u{1f600}",
    "\u20ac",
    SPLIT_AT_BYTE_50,
    "\u00e9".repeat(50),
  ])("refuses the row description %j as not alphanumeric", (description) => {
    const issues = inlineIssuesOf({ description });
    expect(issues).toHaveLength(1);
    expect(issues[0]).toMatchObject({
      path: ["description"],
      code: z.ZodIssueCode.custom,
      message: TRAN_TYPE_LIST_MESSAGES.descriptionAlphanumericOnly,
    });
  });

  it.each(["", "   ", "*", "\u0000".repeat(50)])(
    "refuses the row description %j as not supplied",
    (description) => {
      const issues = inlineIssuesOf({ description });
      expect(issues.map((issue) => issue.message)).toEqual([
        TRAN_TYPE_LIST_MESSAGES.descriptionMustBeSupplied,
      ]);
    },
  );

  it.each(["PURCHASE", "Retail purchase 2", "A".repeat(50)])(
    "accepts the row description %j",
    (description) => {
      expect(inlineIssuesOf({ type_code: "01", description })).toEqual([]);
    },
  );

  it("keeps zod's own issue for an over-length row description", () => {
    const issues = inlineIssuesOf({ description: "\u00e9".repeat(51) });
    expect(issues).toHaveLength(1);
    expect(issues[0]).toMatchObject({
      path: ["description"],
      code: z.ZodIssueCode.too_big,
    });
    expect(AUDITED).not.toContain(issues[0]?.message);
  });

  // COTRTLIC addresses the listed row by its key unedited, so the server holds
  // the {id} segment to ASCII and answers it with an empty message.
  it.each(["\u00e9", "9\u00e9"])("refuses the row key %j with zod's regex issue", (typeCode) => {
    const issues = inlineIssuesOf({ type_code: typeCode, description: "PURCHASE" });
    expect(issues).toHaveLength(1);
    expect(issues[0]).toMatchObject({
      path: ["type_code"],
      code: z.ZodIssueCode.invalid_string,
      validation: "regex",
    });
  });
});

describe("lengths in code points", () => {
  // A character outside the BMP: one code point, two UTF-16 code units.
  const EMOJI = "\u{1f600}";

  // The issues of each field for one entry, the other fields valid.
  const FIELDS = {
    "list description filter": (value: string) => issuesOf({ description: value }),
    "list type code filter": (value: string) => issuesOf({ type_code: value }),
    "row description": (value: string) => inlineIssuesOf({ description: value }),
    "row key": (value: string) =>
      inlineIssuesOf({ type_code: value, description: "PURCHASE" }),
    "CTTU add description": (value: string) =>
      createIssuesOf({ type_code: "01", description: value }),
    "CTTU add type code": (value: string) =>
      createIssuesOf({ type_code: value, description: "PURCHASE" }),
    "CTTU update description": (value: string) =>
      updateIssuesOf({ description: value }),
  } satisfies Record<string, (value: string) => z.ZodIssue[]>;

  // The issues zod's own `check` gives `field` holding `value`.
  function zodIssues(field: string, check: z.ZodString, value: string): z.ZodIssue[] {
    const result = z.object({ [field]: check }).safeParse({ [field]: value });
    if (result.success) {
      throw new Error(`zod accepted ${JSON.stringify(value)} on ${field}`);
    }
    return result.error.issues;
  }

  // zod's own issue of `field` under `.max(maximum)`, for an over-length value.
  function maxIssues(field: string, maximum: number): z.ZodIssue[] {
    return zodIssues(field, z.string().max(maximum), "A".repeat(maximum + 1));
  }

  // zod's own issue of `field` under a failing `.regex`.
  function regexIssues(field: string): z.ZodIssue[] {
    return zodIssues(field, z.string().regex(/^$/), "A");
  }

  // The issue of a COBOL edit on `field`, with its audited text.
  function editIssues(field: string, message: string): z.ZodIssue[] {
    return [{ code: z.ZodIssueCode.custom, path: [field], message }];
  }

  // Within its length every entry reaches what the server answers it with:
  // the description filter is searched (V-COTRTLIC-002 on the server), the
  // COBOL edits refuse a character outside ASCII with their own texts, and the
  // row key keeps its ASCII check. One code point more is zod's own length
  // issue and no COBOL edit text; the row key keeps its ASCII issue after it.
  it.each<{
    field: keyof typeof FIELDS;
    points: number;
    value: string;
    expected: z.ZodIssue[];
  }>([
    {
      field: "list description filter",
      points: 50,
      value: `${"A".repeat(49)}${EMOJI}`,
      expected: [],
    },
    {
      field: "list description filter",
      points: 51,
      value: `${"A".repeat(50)}${EMOJI}`,
      expected: maxIssues("description", 50),
    },
    {
      field: "row description",
      points: 50,
      value: `${"A".repeat(49)}${EMOJI}`,
      expected: editIssues(
        "description",
        TRAN_TYPE_LIST_MESSAGES.descriptionAlphanumericOnly,
      ),
    },
    {
      field: "row description",
      points: 51,
      value: `${"A".repeat(50)}${EMOJI}`,
      expected: maxIssues("description", 50),
    },
    {
      field: "CTTU add description",
      points: 50,
      value: `${"A".repeat(49)}${EMOJI}`,
      expected: editIssues(
        "description",
        TRAN_TYPE_UPSERT_MESSAGES.descriptionAlphanumericOnly,
      ),
    },
    {
      field: "CTTU add description",
      points: 51,
      value: `${"A".repeat(50)}${EMOJI}`,
      expected: maxIssues("description", 50),
    },
    {
      field: "CTTU update description",
      points: 50,
      value: EMOJI.repeat(50),
      expected: editIssues(
        "description",
        TRAN_TYPE_UPSERT_MESSAGES.descriptionAlphanumericOnly,
      ),
    },
    {
      field: "CTTU update description",
      points: 51,
      value: EMOJI.repeat(51),
      expected: maxIssues("description", 50),
    },
    {
      field: "list type code filter",
      points: 2,
      value: EMOJI.repeat(2),
      expected: editIssues(
        "type_code",
        TRAN_TYPE_LIST_MESSAGES.typeCodeFilterNotTwoDigits,
      ),
    },
    {
      field: "list type code filter",
      points: 3,
      value: EMOJI.repeat(3),
      expected: maxIssues("type_code", 2),
    },
    {
      field: "CTTU add type code",
      points: 2,
      value: `1${EMOJI}`,
      expected: editIssues(
        "type_code",
        TRAN_TYPE_UPSERT_MESSAGES.typeCodeMustBeNumeric,
      ),
    },
    {
      field: "CTTU add type code",
      points: 3,
      value: `1${EMOJI}${EMOJI}`,
      expected: maxIssues("type_code", 2),
    },
    {
      field: "row key",
      points: 2,
      value: EMOJI.repeat(2),
      expected: regexIssues("type_code"),
    },
    {
      field: "row key",
      points: 3,
      value: EMOJI.repeat(3),
      expected: [...maxIssues("type_code", 2), ...regexIssues("type_code")],
    },
  ])(
    "checks the $field of $points code points",
    ({ field, points, value, expected }) => {
      expect(Array.from(value)).toHaveLength(points);
      expect(value.length).toBeGreaterThan(points);
      expect(FIELDS[field](value)).toEqual(expected);
    },
  );
});
