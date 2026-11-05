// Tests of the CC00 sign-on form schema (schemas/auth.ts) against the order
// in which the server rejects a POST /auth/login body: LoginRequest::check
// tests the lengths of both fields first, naming USERID then PASSWD, with
// zod's own text (the server answers with an empty message); only a body
// whose lengths both pass reaches the sign-on rule's blank edits,
// V-COSGN00C-001 then V-COSGN00C-002. Blank is SPACES OR LOW-VALUES of the
// text as typed, and an accepted body comes back untrimmed and in its case.
import { describe, expect, it } from "vitest";
import { z } from "zod";

import type { components } from "@/api/schema";

import {
  LOGIN_MESSAGES,
  type LoginFormValues,
  loginBmsFieldMap,
  loginSchema,
} from "./auth";

type LoginField = components["schemas"]["LoginField"];

// The width of the USERIDI and PASSWDI map fields, PIC X(8).
const WIDTH = 8;

// One issue the schema must report, matched on these members only.
interface ExpectedIssue {
  path: [keyof LoginFormValues];
  code: z.ZodIssueCode;
  message?: string;
  maximum?: number;
}

function tooLong(field: keyof LoginFormValues): ExpectedIssue {
  return { path: [field], code: z.ZodIssueCode.too_big, maximum: WIDTH };
}

const USER_ID_EMPTY: ExpectedIssue = {
  path: ["user_id"],
  code: z.ZodIssueCode.custom,
  message: LOGIN_MESSAGES.userIdEmpty,
};

const PASSWORD_EMPTY: ExpectedIssue = {
  path: ["password"],
  code: z.ZodIssueCode.custom,
  message: LOGIN_MESSAGES.passwordEmpty,
};

// Every audited COSGN00C text the schema module can report.
const AUDITED: readonly string[] = Object.values(LOGIN_MESSAGES);

function issuesOf(values: unknown): z.ZodIssue[] {
  const result = loginSchema.safeParse(values);
  return result.success ? [] : result.error.issues;
}

interface OrderCase {
  name: string;
  values: LoginFormValues;
  expected: ExpectedIssue[];
}

describe("loginSchema", () => {
  it.each<OrderCase>([
    {
      name: "a blank user ID with an over-long password: the length alone",
      values: { user_id: "", password: "123456789" },
      expected: [tooLong("password")],
    },
    {
      name: "an over-long user ID with a blank password: the length alone",
      values: { user_id: "123456789", password: "" },
      expected: [tooLong("user_id")],
    },
    {
      name: "both fields over-long: the user ID's length, then the password's",
      values: { user_id: "123456789", password: "123456789" },
      expected: [tooLong("user_id"), tooLong("password")],
    },
    {
      name: "an over-long user ID of spaces: the length alone",
      values: { user_id: " ".repeat(WIDTH + 1), password: "PASSWORD" },
      expected: [tooLong("user_id")],
    },
    {
      name: "both fields empty: V-COSGN00C-001, then V-COSGN00C-002",
      values: { user_id: "", password: "" },
      expected: [USER_ID_EMPTY, PASSWORD_EMPTY],
    },
    {
      name: "a user ID of spaces with an empty password: V-COSGN00C-001, then V-COSGN00C-002",
      values: { user_id: "   ", password: "" },
      expected: [USER_ID_EMPTY, PASSWORD_EMPTY],
    },
    {
      name: "a user ID with an empty password: V-COSGN00C-002",
      values: { user_id: "ADMIN001", password: "" },
      expected: [PASSWORD_EMPTY],
    },
    {
      name: "a password of spaces filling its map field: V-COSGN00C-002",
      values: { user_id: "ADMIN001", password: " ".repeat(WIDTH) },
      expected: [PASSWORD_EMPTY],
    },
    {
      name: "a user ID of NUL only: V-COSGN00C-001",
      values: { user_id: "\u0000", password: "PASSWORD" },
      expected: [USER_ID_EMPTY],
    },
    {
      name: "a tab user ID, which is not blank: accepted",
      values: { user_id: "\t", password: "PASSWORD" },
      expected: [],
    },
    {
      name: "the seeded administrator: accepted",
      values: { user_id: "ADMIN001", password: "PASSWORD" },
      expected: [],
    },
    {
      name: "lower-case credentials: accepted as typed",
      values: { user_id: "admin001", password: "password" },
      expected: [],
    },
  ])("$name", ({ values, expected }) => {
    const result = loginSchema.safeParse(values);
    const issues = result.success ? [] : result.error.issues;
    expect(issues).toEqual(expected.map((issue) => expect.objectContaining(issue)));
    for (const issue of issues) {
      if (issue.code === z.ZodIssueCode.too_big) {
        expect(AUDITED).not.toContain(issue.message);
      }
    }
    if (expected.length === 0) {
      expect(result.data).toStrictEqual(values);
    }
  });

  // The first issue is the field the form focuses, which must be the first
  // field the server's 400 answer names.
  it.each<[values: LoginFormValues, serverFirst: LoginField]>([
    [{ user_id: "", password: "123456789" }, "PASSWD"],
    [{ user_id: "123456789", password: "" }, "USERID"],
    [{ user_id: "", password: "" }, "USERID"],
    [{ user_id: "   ", password: "" }, "USERID"],
  ])("starts %j with the server's first field %s", (values, serverFirst) => {
    expect(issuesOf(values)[0]?.path).toEqual([loginBmsFieldMap[serverFirst]]);
  });

  it.each<[member: keyof LoginFormValues, values: Partial<LoginFormValues>]>([
    ["user_id", { password: "PASSWORD" }],
    ["password", { user_id: "ADMIN001" }],
  ])("rejects a body without %s as required, alone", (member, values) => {
    expect(issuesOf(values)).toEqual([
      expect.objectContaining({
        path: [member],
        code: z.ZodIssueCode.invalid_type,
        received: "undefined",
      }),
    ]);
  });
});
