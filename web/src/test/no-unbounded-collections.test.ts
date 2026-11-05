// Unbounded-collection gate (GHSA-mrhx-c7xc-m3w5, CVE-2023-54404, CWE-770): no
// zod schema in the SPA parses a collection, so neither collection parser of the
// pinned zod 3.25.76 runs. The pin stays: 3.25.76 is the newest 3.x, the line
// @hookform/resolvers 5.9.2 accepts, and the advisory's fix starts at 4.6.6.
//
// The 3.25.76 package ships both parsers:
// - v3, behind the root specifier "zod": index.js is
//   `import * as z from "./v3/external.js"`, and the v3 modules import nothing
//   from v4. ZodArray._parse (v3/types.js L1747-1808) checks the exact, minimum
//   and maximum lengths and then still parses every element and merges every
//   element's issues, so .max() bounds none of the work: 200,000 failing
//   strings under .max(3) cost 200,000 element parses and 200,001 issues.
//   ZodSet, ZodMap, ZodRecord, a ZodTuple rest and a ZodObject catchall walk
//   every element or key the same way.
// - v4, the code the advisory names: $ZodArray and its handleArrayResult
//   (v4/core/schemas.js L616-655) push every failing element's issues with no
//   cap. Only the subpaths zod/v4, zod/v4/core, zod/v4-mini, zod/v4/mini and the
//   v4 locales reach it.
//
// Neither is reachable. Every application source imports exactly "zod" and
// declares no collection schema. @hookform/resolvers 5.9.2 imports no zod module
// at runtime: zod/dist/zod.mjs tells the schema versions apart by its _zod
// property. zod 4.6.7, a fixed release, is a private dependency of the shadcn
// CLI and never reaches the bundle. The alert is not a demonstrated denial of
// service.
//
// A static scan of every application and generated TypeScript source under src,
// read through Vite's ?raw glob and scanned whole, comments included, keeps that
// true. It fails on:
// - a zod module specifier other than "zod" in an import, export-from or dynamic
//   import, which would reach the v4 parser;
// - a static import or re-export of "zod" that binds any name but z, because
//   the collection patterns match the z namespace;
// - a collection schema: the z.array, z.set, z.map, z.record and z.tuple calls,
//   the .array, .rest and .catchall methods (a catchall parses every unknown
//   key) and the ZodArray, ZodSet, ZodMap, ZodRecord and ZodTuple classes, with
//   any whitespace between tokens.
// Test sources are left out, because they plant violations to prove the
// matchers, and this file holds the patterns.
//
// A collection schema added later must reject oversized input before any
// element is parsed: a length check ahead of the element schema, sized from the
// BMS field's occurrence count, such as
// z.custom((value) => Array.isArray(value) && value.length <= count).pipe(...),
// whose failed check stops the pipe before the element schema runs. Only then is
// this gate relaxed, for that schema alone. Escalation E-85 logs the disposition.
import { describe, expect, it } from "vitest";

// The one zod specifier the SPA imports: the package root, which is v3.
const ZOD_ROOT = "zod";

// A module specifier after `from` (import and export-from), after `import` (a
// side-effect import) or inside `import(` (a dynamic import), with any
// whitespace between tokens. Group 2 is the specifier.
const SPECIFIER = /\b(?:from\s*|import\s*(?:\(\s*)?)(["'`])([^"'`\n]*)\1/g;

// The zod package root or any of its subpaths, with or without a query.
const ZOD_PACKAGE = /^zod(?:[/?#]|$)/;

// A static import or export-from of the zod root. Group 1 is its clause.
const ZOD_ROOT_CLAUSE =
  /\b(?:import|export)\s+([^;'"`]*?)\s*\bfrom\s*(["'`])zod\2/g;

// The clauses that bind the name z and nothing else, normalized by clauseOf.
const Z_ONLY_CLAUSES: ReadonlySet<string> = new Set([
  "{ z }",
  "{ type z }",
  "z",
  "* as z",
]);

// Each collection schema with its pattern. The .array pattern skips z.array,
// which has its own entry, so one call is reported once.
const COLLECTION_PATTERNS: readonly (readonly [token: string, RegExp])[] = [
  ["z.array(", /\bz\s*\.\s*array\s*\(/g],
  ["z.set(", /\bz\s*\.\s*set\s*\(/g],
  ["z.map(", /\bz\s*\.\s*map\s*\(/g],
  ["z.record(", /\bz\s*\.\s*record\s*\(/g],
  ["z.tuple(", /\bz\s*\.\s*tuple\s*\(/g],
  [".array(", /(?<!\bz\s*)\.\s*array\s*\(/g],
  [".rest(", /\.\s*rest\s*\(/g],
  [".catchall(", /\.\s*catchall\s*\(/g],
  ["ZodArray", /\bZodArray\b/g],
  ["ZodSet", /\bZodSet\b/g],
  ["ZodMap", /\bZodMap\b/g],
  ["ZodRecord", /\bZodRecord\b/g],
  ["ZodTuple", /\bZodTuple\b/g],
];

/** One violation in a source: its 1-based line, what was found and the trimmed line. */
interface Violation {
  readonly line: number;
  readonly found: string;
  readonly text: string;
}

// The bound names of an import or export clause as space-separated tokens,
// without a leading `type` and a trailing comma: `type {\n  z,\n}` is `{ z }`.
function clauseOf(clause: string): string {
  const tokens = clause.replace(/,\s*}/g, "}").match(/[\w$]+|[{}*,]/g) ?? [];
  return (tokens[0] === "type" ? tokens.slice(1) : tokens).join(" ");
}

function violations(source: string): Violation[] {
  const hits: { at: number; found: string }[] = [];
  for (const match of source.matchAll(SPECIFIER)) {
    const specifier = match[2];
    if (ZOD_PACKAGE.test(specifier) && specifier !== ZOD_ROOT) {
      hits.push({ at: match.index, found: `specifier ${specifier}` });
    }
  }
  for (const match of source.matchAll(ZOD_ROOT_CLAUSE)) {
    if (!Z_ONLY_CLAUSES.has(clauseOf(match[1]))) {
      const clause = match[1].replace(/\s+/g, " ").trim();
      hits.push({ at: match.index, found: `binding ${clause}` });
    }
  }
  for (const [token, pattern] of COLLECTION_PATTERNS) {
    for (const match of source.matchAll(pattern)) {
      hits.push({ at: match.index, found: token });
    }
  }
  const lines = source.split("\n");
  return hits
    .sort((a, b) => a.at - b.at)
    .map(({ at, found }) => {
      const line = source.slice(0, at).split("\n").length;
      return { line, found, text: lines[line - 1].trim() };
    });
}

/** What `violations` finds in `sample`, in source order. */
function found(sample: string): string[] {
  return violations(sample).map((violation) => violation.found);
}

/** The zod specifiers a source imports, root included. */
function zodSpecifiers(source: string): string[] {
  return Array.from(source.matchAll(SPECIFIER), (match) => match[2]).filter(
    (specifier) => ZOD_PACKAGE.test(specifier),
  );
}

function isTestSource(file: string): boolean {
  return /\.test\.tsx?$/.test(file) || file.startsWith("/src/test/");
}

// Keys are project-absolute paths such as /src/schemas/auth.ts. The pattern
// also takes declaration files such as the generated api/schema.d.ts.
const sources = import.meta.glob<string>("/src/**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
});

const byFile = (
  [a]: readonly [string, string],
  [b]: readonly [string, string],
): number => (a < b ? -1 : a > b ? 1 : 0);

const scanned: readonly (readonly [file: string, source: string])[] =
  Object.entries(sources)
    .filter(([file]) => !isTestSource(file))
    .sort(byFile);

// The schema modules that import zod. A glob or specifier matcher that misses
// any of them would let the scan pass vacuously.
const ZOD_IMPORTERS: readonly string[] = [
  "/src/schemas/accounts.ts",
  "/src/schemas/admin-users.ts",
  "/src/schemas/auth.ts",
  "/src/schemas/authorizations.ts",
  "/src/schemas/cards.ts",
  "/src/schemas/index.ts",
  "/src/schemas/payments.ts",
  "/src/schemas/reports.ts",
  "/src/schemas/reward-rules.ts",
  "/src/schemas/rewards.ts",
  "/src/schemas/transaction-types.ts",
  "/src/schemas/transactions.ts",
];

// The other schema module and two generated sources the scan must also read.
const REQUIRED_SOURCES: readonly string[] = [
  ...ZOD_IMPORTERS,
  "/src/schemas/primitives.ts",
  "/src/api/schema.d.ts",
  "/src/lib/utils.ts",
];

// The subpaths of the zod 3.25.76 package and the mini entry of the zod 4 line.
const ZOD_SUBPATHS = [
  "zod/v3",
  "zod/v4",
  "zod/v4/core",
  "zod/v4-mini",
  "zod/v4/mini",
  "zod/mini",
  "zod/v4/locales",
  "zod/v4/locales/en.js",
] as const;

// Each way a module names a specifier: a named and a type-only import, an
// export-from, a side-effect import, and a dynamic import spread over lines or
// written as a template literal.
const IMPORT_FORMS: readonly ((specifier: string) => string)[] = [
  (specifier) => `import { z } from "${specifier}";`,
  (specifier) => `import type { z } from '${specifier}';`,
  (specifier) => `export * from "${specifier}";`,
  (specifier) => `import "${specifier}";`,
  (specifier) => `const zod = await import(\n  "${specifier}"\n);`,
  (specifier) => `void import (\`${specifier}\`);`,
];

describe("zod imports and collection schemas in sources", () => {
  it("scans every application and generated source and no test source", () => {
    const files = scanned.map(([file]) => file);
    expect(files).toEqual(expect.arrayContaining([...REQUIRED_SOURCES]));

    // Checked independently of isTestSource, so a broken filter fails here.
    const tests = files.filter(
      (file) =>
        file.startsWith("/src/test/") ||
        file.endsWith(".test.ts") ||
        file.endsWith(".test.tsx"),
    );
    expect(tests).toEqual([]);

    // The glob reaches the test sources, which plant zod imports of their own,
    // and the filter is what leaves them out. Vite omits the importing module,
    // this file, from its own glob.
    const excluded = Object.keys(sources).filter(isTestSource);
    expect(excluded).toEqual(
      expect.arrayContaining([
        "/src/test/setup.ts",
        "/src/schemas/schemas.test.ts",
        "/src/schemas/auth.test.ts",
      ]),
    );

    const empty = scanned
      .filter(([, source]) => typeof source !== "string" || source.trim() === "")
      .map(([file]) => file);
    expect(empty).toEqual([]);
  });

  it("finds the zod import of every schema module", () => {
    const importers = scanned
      .filter(([, source]) => zodSpecifiers(source).length > 0)
      .map(([file]) => file);
    expect(importers).toEqual(expect.arrayContaining([...ZOD_IMPORTERS]));
  });

  it.each(
    ZOD_SUBPATHS.flatMap((subpath) =>
      IMPORT_FORMS.map((form) => [form(subpath), subpath] as const),
    ),
  )("flags the zod subpath in %j", (sample, subpath) => {
    expect(found(sample)).toEqual([`specifier ${subpath}`]);
  });

  it.each([
    ['import * as zod from "zod";', "binding * as zod"],
    ['import zod from "zod";', "binding zod"],
    ['import { z as zod } from "zod";', "binding { z as zod }"],
    ['import { array } from "zod";', "binding { array }"],
    ['import {\n  z,\n  array,\n} from "zod";', "binding { z, array, }"],
    ["import type { z, ZodType } from 'zod';", "binding type { z, ZodType }"],
    ['export * from "zod";', "binding *"],
    ['export { z as zod } from "zod";', "binding { z as zod }"],
  ] as const)("flags the binding of %j", (sample, binding) => {
    expect(found(sample)).toEqual([binding]);
  });

  it.each([
    ["z.array(z.string())", "z.array("],
    ["z\n  .array (z.string())", "z.array("],
    ["z . set(z.string())", "z.set("],
    ["z.map(z.string(), z.string())", "z.map("],
    ["z.record(\n  z.string(),\n)", "z.record("],
    ["z.tuple([z.string()])", "z.tuple("],
    ["z.string().array()", ".array("],
    ["z.string()\n  .array\n  ()", ".array("],
    ["xz.array(z.string())", ".array("],
    ["schema.rest (z.string())", ".rest("],
    ["z.object({}).catchall(z.string())", ".catchall("],
    ["z.ZodArray.create(z.string())", "ZodArray"],
    ["new z.ZodSet({})", "ZodSet"],
    ["value instanceof z.ZodMap", "ZodMap"],
    ["z.ZodRecord.create(z.string())", "ZodRecord"],
    ["z.ZodTuple.create([])", "ZodTuple"],
  ] as const)("flags the collection schema in %j", (sample, token) => {
    expect(found(sample)).toEqual([token]);
  });

  it("passes the zod root bound to z, scalar schemas and collection methods of plain values", () => {
    const ignored = [
      'import { z } from "zod";',
      'import type { z } from "zod";',
      "import { type z } from 'zod';",
      'import {z} from "zod"',
      'import {\n  z,\n} from "zod";',
      'import * as z from "zod";',
      'import z from "zod";',
      'export { z } from "zod";',
      'const { z } = await import("zod");',
      'import { zodResolver } from "@hookform/resolvers/zod";',
      'import { zodToJsonSchema } from "zod-to-json-schema";',
      'z.object({ ACCTSID: z.string().max(11).regex(/^[0-9]*$/) })',
      'z.enum(["A", "U"])',
      "rows.map((row) => row.id)",
      'params.set("page", "2")',
      "new Map<string, string>()",
      "Array.from(rows)",
      "response.arrayBuffer()",
      "snapshot.restore()",
      "zArray.length",
    ];
    expect(ignored.filter((sample) => found(sample).length > 0)).toEqual([]);
  });

  it("reports each violation at its line in source order", () => {
    expect(
      violations(
        'import { z } from "zod";\n\nexport const rows = z\n  .array(z.string());\nvoid import("zod/v4");\n',
      ),
    ).toEqual([
      { line: 3, found: "z.array(", text: "export const rows = z" },
      { line: 5, found: "specifier zod/v4", text: 'void import("zod/v4");' },
    ]);
  });

  it("finds no zod subpath, foreign binding or collection schema in any source", () => {
    const hits = scanned.flatMap(([file, source]) =>
      violations(source).map((violation) => ({ file, ...violation })),
    );
    expect(hits).toEqual([]);
  });
});
