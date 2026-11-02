// Tests of the user administration form schemas (schemas/admin-users.ts) against the
// server's UserAddRequest and UserUpdateRequest constraints: the names and the new user's
// ID refuse NUL characters beside other text, which the COUSR0nC rules would store and
// PostgreSQL text cannot hold (E-07), with zod's regex issue and no audited text
// (no-message, E-49). NUL characters alone pass where the rule reads them as LOW-VALUES:
// exactly the field width for COUSR01C, which moves the entry into its X(n) field first,
// and any number for COUSR02C. The password keeps NUL, being stored as its argon2id hash,
// and so do the user type and the {id} key. Each member's regex issues match the pattern
// the API document publishes for it.
import { describe, expect, it } from "vitest";
import { z } from "zod";

import openapi from "../../openapi/openapi.json";

import {
  USER_ADD_MESSAGES,
  USER_UPDATE_MESSAGES,
  type UserAddFormValues,
  type UserUpdateFormInput,
  userAddSchema,
  userUpdateSchema,
} from "./admin-users";

/** An add form every client edit accepts. */
const VALID_ADD: UserAddFormValues = {
  first_name: "Ann",
  last_name: "Lee",
  user_id: "user0099",
  password: "pass1234",
  usr_type: "U",
};

/** An edit form every client edit accepts, the password switch off. */
const VALID_UPDATE: UserUpdateFormInput = {
  user_id: "USER0001",
  first_name: "Ann",
  last_name: "Lee",
  change_password: false,
  usr_type: "U",
};

/** Every audited COUSR01C and COUSR02C text the schemas can report. */
const AUDITED: readonly string[] = [
  ...Object.values(USER_ADD_MESSAGES),
  ...Object.values(USER_UPDATE_MESSAGES),
];

const PUBLISHED_ADD = openapi.components.schemas.UserAddRequest.properties;
const PUBLISHED_UPDATE = openapi.components.schemas.UserUpdateRequest.properties;

type AddMember = keyof UserAddFormValues;
type UpdateMember = keyof UserUpdateFormInput;

function addIssuesAt(values: UserAddFormValues, member: AddMember): z.ZodIssue[] {
  const result = userAddSchema.safeParse(values);
  return result.success ? [] : result.error.issues.filter((issue) => issue.path[0] === member);
}

function updateIssuesAt(values: UserUpdateFormInput, member: UpdateMember): z.ZodIssue[] {
  const result = userUpdateSchema.safeParse(values);
  return result.success ? [] : result.error.issues.filter((issue) => issue.path[0] === member);
}

/** Whether `issues` hold zod's regex issue. */
function hasRegexIssue(issues: z.ZodIssue[]): boolean {
  return issues.some((issue) => issue.code === z.ZodIssueCode.invalid_string);
}

/** Asserts that `issues` are zod's one regex issue, with no audited text. */
function expectRegexIssueOnly(issues: z.ZodIssue[]): void {
  expect(issues).toHaveLength(1);
  expect(issues[0]).toMatchObject({ code: z.ZodIssueCode.invalid_string, validation: "regex" });
  expect(AUDITED).not.toContain(issues[0]?.message);
}

/**
 * NUL characters beside other text, within `width`: inside, leading, trailing, before and
 * after a space, and a full width with one other character.
 */
function mixedNulEntries(width: number): string[] {
  return [
    "A\u0000B",
    "\u0000A",
    "A\u0000",
    "\u0000 ",
    " \u0000",
    `${"\u0000".repeat(width - 1)}A`,
  ];
}

/** NUL characters alone, fewer than `width`. */
function shortNulEntries(width: number): string[] {
  return ["\u0000", "\u0000".repeat(width - 1)];
}

/** Text without NUL characters that every member of `width` accepts. */
function textEntries(width: number): string[] {
  return ["", " ".repeat(width), "x".repeat(width), "\u007f"];
}

describe("userAddSchema", () => {
  it("accepts the valid entry", () => {
    expect(userAddSchema.safeParse(VALID_ADD).success).toBe(true);
  });

  describe.each([
    ["first_name", 20],
    ["last_name", 20],
    ["user_id", 8],
  ] as const)("%s", (member, width) => {
    it.each([...mixedNulEntries(width), ...shortNulEntries(width)])(
      "rejects %j with zod's regex issue and no audited text",
      (entered) => {
        expectRegexIssueOnly(addIssuesAt({ ...VALID_ADD, [member]: entered }, member));
      },
    );

    it.each([...textEntries(width), "\u0000".repeat(width)])("accepts %j", (entered) => {
      expect(hasRegexIssue(addIssuesAt({ ...VALID_ADD, [member]: entered }, member))).toBe(
        false,
      );
    });

    it("gives its regex issue to exactly the entries the published pattern refuses", () => {
      const published = new RegExp(PUBLISHED_ADD[member].pattern);
      const entries = [
        ...mixedNulEntries(width),
        ...shortNulEntries(width),
        ...textEntries(width),
        "\u0000".repeat(width),
        "\u00e9",
      ];
      for (const entered of entries) {
        const refused = hasRegexIssue(addIssuesAt({ ...VALID_ADD, [member]: entered }, member));
        expect(refused, JSON.stringify(entered)).toBe(!published.test(entered));
      }
    });
  });

  // The password is hashed and never stored as text; a one-byte user type is NUL-free or
  // the full-width LOW-VALUES.
  it.each([
    ["password", "A\u0000"],
    ["password", "\u0000"],
    ["usr_type", "\u0000"],
  ] as const)("keeps NUL in %s %j", (member, entered) => {
    expect(addIssuesAt({ ...VALID_ADD, [member]: entered }, member)).toEqual([]);
  });
});

describe("userUpdateSchema", () => {
  it("accepts the valid entry", () => {
    expect(userUpdateSchema.safeParse(VALID_UPDATE).success).toBe(true);
  });

  describe.each(["first_name", "last_name"] as const)("%s", (member) => {
    const width = 20;

    it.each(mixedNulEntries(width))(
      "rejects %j with zod's regex issue and no audited text",
      (entered) => {
        expectRegexIssueOnly(updateIssuesAt({ ...VALID_UPDATE, [member]: entered }, member));
      },
    );

    it.each([...textEntries(width), ...shortNulEntries(width), "\u0000".repeat(width)])(
      "accepts %j",
      (entered) => {
        expect(
          hasRegexIssue(updateIssuesAt({ ...VALID_UPDATE, [member]: entered }, member)),
        ).toBe(false);
      },
    );

    it("gives its regex issue to exactly the entries the published pattern refuses", () => {
      const published = new RegExp(PUBLISHED_UPDATE[member].pattern);
      const entries = [
        ...mixedNulEntries(width),
        ...shortNulEntries(width),
        ...textEntries(width),
        "\u0000".repeat(width),
        "\u00e9",
      ];
      for (const entered of entries) {
        const refused = hasRegexIssue(
          updateIssuesAt({ ...VALID_UPDATE, [member]: entered }, member),
        );
        expect(refused, JSON.stringify(entered)).toBe(!published.test(entered));
      }
    });
  });

  it("keeps NUL in a changed password, the user type and the {id} key", () => {
    const values: UserUpdateFormInput = {
      ...VALID_UPDATE,
      user_id: "A\u0000",
      change_password: true,
      password: "A\u0000",
      usr_type: "\u0000",
    };
    expect(userUpdateSchema.safeParse(values).success).toBe(true);
  });
});
