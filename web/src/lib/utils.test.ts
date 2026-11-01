// Tests of `cn` (lib/utils.ts), the class-name helper every generated shadcn
// component composes its classes with: clsx builds the class list from
// strings, conditional objects and nested arrays, then tailwind-merge drops
// the earlier of two conflicting Tailwind utilities.
import type { ClassValue } from "clsx";
import { describe, expect, it } from "vitest";

import { cn } from "./utils";

interface CnCase {
  name: string;
  inputs: ClassValue[];
  expected: string;
}

describe("cn", () => {
  it.each<CnCase>([
    // Composition (clsx)
    {
      name: "returns an empty string when called without arguments",
      inputs: [],
      expected: "",
    },
    {
      name: "joins plain class strings with single spaces",
      inputs: ["flex", "items-center", "gap-2"],
      expected: "flex items-center gap-2",
    },
    {
      name: "collapses surrounding and repeated whitespace",
      inputs: ["  flex   items-center ", " gap-2 "],
      expected: "flex items-center gap-2",
    },
    {
      name: "drops false, null, undefined, empty-string and zero values",
      inputs: ["flex", false, null, undefined, "", 0, "gap-2"],
      expected: "flex gap-2",
    },
    {
      name: "returns an empty string when every value is falsy",
      inputs: [false, null, undefined, "", 0],
      expected: "",
    },
    {
      name: "keeps the keys of a conditional object whose values are truthy",
      inputs: [{ flex: true, hidden: false, "items-center": 1, "gap-2": null }],
      expected: "flex items-center",
    },
    {
      name: "mixes strings and conditional objects in argument order",
      inputs: ["btn", { "btn-active": true, "btn-disabled": false }, "rounded"],
      expected: "btn btn-active rounded",
    },
    {
      name: "flattens nested arrays, including falsy members and objects",
      inputs: [["flex", ["items-center", ["gap-2", false]]], [{ "p-4": true }]],
      expected: "flex items-center gap-2 p-4",
    },
    // Conflict resolution (tailwind-merge)
    {
      name: "keeps the later of two conflicting padding utilities",
      inputs: ["px-2", "px-4"],
      expected: "px-4",
    },
    {
      name: "keeps an axis padding that refines an earlier shorthand",
      inputs: ["p-4", "px-2"],
      expected: "p-4 px-2",
    },
    {
      name: "drops an axis padding that a later shorthand overrides",
      inputs: ["px-2", "p-4"],
      expected: "p-4",
    },
    {
      name: "keeps the later of two conflicting text colours",
      inputs: ["text-red-500", "text-blue-500"],
      expected: "text-blue-500",
    },
    {
      name: "resolves conflicts per variant, leaving the hover utility alone",
      inputs: ["bg-red-500 hover:bg-red-600", "bg-blue-500"],
      expected: "hover:bg-red-600 bg-blue-500",
    },
    {
      name: "lets a utility enabled by a conditional object override a string",
      inputs: ["px-2 py-1", { "px-4": true }],
      expected: "py-1 px-4",
    },
    {
      name: "keeps non-conflicting utilities in their original order",
      inputs: ["flex", "p-4", "text-sm", "font-medium", "text-red-500"],
      expected: "flex p-4 text-sm font-medium text-red-500",
    },
  ])("$name", ({ inputs, expected }) => {
    expect(cn(...inputs)).toBe(expected);
  });
});
