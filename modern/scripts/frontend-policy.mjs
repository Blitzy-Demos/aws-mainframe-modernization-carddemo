#!/usr/bin/env node
// Frontend policy check, run by scripts/check.sh step 7 as `node scripts/frontend-policy.mjs` from
// modern/. It enforces two rules and prints nothing when both hold:
//
// - Single Component System: every package that frontend/package.json names under dependencies,
//   devDependencies, peerDependencies or optionalDependencies must be listed in
//   frontend/approved-dependencies.json, the single source of approved names, and its spec must
//   resolve to the npm registry package of that name: a version, a range, a dist-tag or an npm:
//   alias of the same name. An alias of another package and a git, URL, local path or other
//   protocol spec are findings, there and in pnpm.overrides, resolutions and overrides. shadcn/ui
//   is the only component system.
// - Generated API Types Only: no file under frontend/src other than the generated
//   src/api/schema.d.ts may declare an interface, type alias or enum named after a key of
//   components.schemas in openapi/openapi.json, a type alias of the generated components[…] or
//   paths[…] types, or an interface whose extends clause names one. A generated root counts when
//   it is any top-level union or intersection member, any generic type argument, or the check
//   type or a branch of a conditional type; a conditional's extends type is not one.
//
// Each finding goes to stderr as `<file>:<line>: <message>`, with the path relative to the root
// checked. With no argument that root is the modern/ directory holding this script, whatever the
// working directory; a single ROOT argument names another tree laid out like modern/, so the
// checks can be exercised against a scratch tree. A lone -h or --help prints the usage on stdout.
// Exit status: 0 when there is no finding; 1 when there is any, an unreadable or malformed input
// included; 2 on a usage error (an option other than -h or --help, an empty ROOT or a second
// argument), reported on stderr with the usage before any file is read. The script reads files
// only: it writes nothing and uses no network.

import fs from "node:fs";
import path from "node:path";

// Printed on stdout for -h or --help, and on stderr after a usage error.
const USAGE = `Usage: node scripts/frontend-policy.mjs [ROOT]
       node scripts/frontend-policy.mjs -h | --help

Checks the frontend against two rules and prints nothing when both hold. Single Component System:
every package frontend/package.json names under dependencies, devDependencies, peerDependencies
or optionalDependencies must be listed in frontend/approved-dependencies.json, and its spec must
resolve to the npm registry package of that name: an alias of another package, or a git, URL,
path or other protocol spec, fails, as it does in pnpm.overrides, resolutions and overrides.
Generated API Types Only: no file under frontend/src other than src/api/schema.d.ts may declare
an interface, type alias or enum named after an OpenAPI schema, alias the generated
components[…] or paths[…] types, or declare an interface that extends one. Each finding goes to
stderr as <file>:<line>: <message>, the path relative to the root checked. ROOT names another
tree laid out like modern/ to check in its place; without it, the modern/ directory holding this
script is checked, whatever the working directory. Files are only read.

Options:
  -h, --help  Print this help and exit. Any other option, an empty ROOT or a second argument is a
              usage error.

Exit status:
  0  no finding
  1  any finding, an unreadable or malformed input included
  2  usage error
`;

// The command line: no argument checks the modern/ directory holding this script (scripts/check.sh
// step 7 runs it that way), one ROOT checks that tree instead, and a lone -h or --help asks for
// USAGE. Anything else is a usage error naming the offending argument. Only the check mode reads a
// file; the bottom block acts on the mode.
function parseArguments(args) {
  const [first, second] = args;
  const help = first === "-h" || first === "--help";
  if (args.length === 0) return { mode: "check" };
  if (first === "") return { mode: "usage", problem: "empty ROOT argument", argument: first };
  if (first.startsWith("-") && !help) {
    return { mode: "usage", problem: "unknown option", argument: first };
  }
  if (args.length > 1) {
    return { mode: "usage", problem: "unexpected second argument", argument: second };
  }
  return help ? { mode: "help" } : { mode: "check", root: first };
}

const invocation = parseArguments(process.argv.slice(2));
const root = path.resolve(invocation.root ?? path.join(import.meta.dirname, ".."));

const PACKAGE_JSON = "frontend/package.json";
const APPROVED_JSON = "frontend/approved-dependencies.json";
const OPENAPI_JSON = "openapi/openapi.json";
const SOURCE_DIR = "frontend/src";
const GENERATED_SCHEMA = "frontend/src/api/schema.d.ts";
const SELF = "scripts/frontend-policy.mjs";

const DEPENDENCY_SECTIONS = [
  "dependencies",
  "devDependencies",
  "peerDependencies",
  "optionalDependencies",
];
const PACKAGE_NAME = /^(?:@[a-z0-9~-][a-z0-9._~-]*\/)?[a-z0-9~-][a-z0-9._~-]*$/;
// Named only to sharpen the message; the allow-list rejects every unlisted package either way.
const COMPONENT_LIBRARY =
  /^(?:@(?:mui|material-ui|chakra-ui|ant-design)\/|(?:antd|bootstrap|react-bootstrap|reactstrap)$)/;
const COMPONENT_POLICY =
  "shadcn/ui is the only component system — Material UI, Chakra UI, Ant Design and Bootstrap " +
  "(and any other component library) are prohibited";
// Specs that resolve to the npm registry package of the entry's own name: a semver range set (the
// node-semver grammar: `1.2.3`, `^1.2`, `>=26.1.0 <27`, `1.x`, `*`, `1 || 2`, `1.0.0 - 2.0.0`, or
// empty), a dist-tag, or `npm:<same name>[@<range or tag>]`. Each regular expression is linear.
const VERSION_PART = "(?:[xX*]|0|[1-9]\\d*)";
const VERSION_IDS = "[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*";
const PARTIAL_VERSION =
  `[=v]*${VERSION_PART}(?:\\.${VERSION_PART}(?:\\.${VERSION_PART}` +
  `(?:-${VERSION_IDS})?(?:\\+${VERSION_IDS})?)?)?`;
const VERSION = new RegExp(`^${PARTIAL_VERSION}$`);
const COMPARATOR = new RegExp(`^(?:[<>]=?|~>?|\\^)?${PARTIAL_VERSION}$`);
const HYPHEN_RANGE = /^(\S+)\s+-\s+(\S+)$/;
const OPERATOR_SPACE = /([<>]?=|[<>]|~>?|\^)\s+/g;
const DIST_TAG = /^[A-Za-z0-9_-][A-Za-z0-9._-]*$/;
const NPM_ALIAS = /^npm:/i;
// Anything else names another source; these sort it for the message.
const LOCAL_PATH = /^(?:\.|~[\\/]|[\\/]|[A-Za-z]:[\\/])/;
const TARBALL = /\.(?:tgz|tar\.gz|tar)$/i;
const PROTOCOL = /^([A-Za-z][A-Za-z0-9+.-]*):/;
const GIT_PROTOCOL = /^(?:git|git\+.*|github|gitlab|bitbucket|gist|https?|ssh)$/;
const PATH_PROTOCOL = new Set(["file", "link", "portal"]);
const SCP_GIT = /^[^\s/:@]+@[^\s/:]+:/;
const GITHUB_SHORTHAND = /^[^@%/\s.-][^:@%/\s]*\/[^:@\s/%#]+(?:#.*)?$/;
// An override selector's parent>child delimiter: a `>` that is not a range operator (pnpm's rule).
const SELECTOR_DELIMITER = /[^ |@]>/g;

// Generated API Types Only. Besides an interface or type alias named after a schema key and an
// alias of components[…] or paths[…], these count as findings: an enum named after a schema key;
// an alias of a bare root (`type C = components`); an alias with a generated root as any member
// of its top-level union or intersection, as a type argument of a generic reference of any name
// (`Awaited<…>`, `Readonly<Partial<…>>`), or as the check type or a branch of a conditional type,
// however nested; an interface whose extends clause names one in the same way; and roots reached
// through a renamed or namespace import or an inline `import(…)` of the generated module. Each
// restates a generated type by hand, and a bare root alias would let every later alias of it pass
// unseen. A conditional's extends type is not one: it tests a type against the generated type.
// Names are whole TypeScript identifiers, Unicode letters and `\u` escapes included, and are
// compared with their escapes decoded.
const SOURCE_FILE = /\.(?:ts|tsx|mts|cts)$/;
const GENERATED_ROOTS = new Set(["components", "paths"]);
const SCHEMA_MODULE = /schema(?:\.d)?(?:\.[cm]?[jt]sx?)?$/;
const ESCAPE = String.raw`\\u(?:[0-9A-Fa-f]{4}|\{[0-9A-Fa-f]+\})`;
const IDENTIFIER =
  String.raw`(?:[\p{ID_Start}$_]|${ESCAPE})` +
  String.raw`(?:[\p{ID_Continue}$\u200C\u200D]|${ESCAPE})*`;
// Not preceded by an identifier character or a `.`, so a keyword is not the tail of a longer name.
const WORD_START = String.raw`(?<![\p{ID_Continue}$\u200C\u200D.])`;
const DECLARATION = new RegExp(
  String.raw`${WORD_START}(interface|type|enum)\s+(${IDENTIFIER})`,
  "gu",
);
const IMPORT_DECLARATION = new RegExp(
  String.raw`${WORD_START}import(?=[\s{*])\s*(?:type(?=[\s{*])\s*)?([^'";]*?)\s*` +
    String.raw`${WORD_START}from\s*(["'])([^"'\n]*)\2`,
  "dgu",
);
const IMPORT_SPECIFIER = new RegExp(
  String.raw`^\s*(?:type\s+)?(${IDENTIFIER})(?:\s+as\s+(${IDENTIFIER}))?\s*$`,
  "u",
);
const NAMESPACE_IMPORT = new RegExp(String.raw`\*\s*as\s+(${IDENTIFIER})`, "u");
const IDENTIFIER_AT = new RegExp(IDENTIFIER, "uy");
const IDENTIFIER_UNIT = /\\u\{([0-9A-Fa-f]+)\}|\\u([0-9A-Fa-f]{4})|[^]/gu;
const ID_START = /^[\p{ID_Start}$_]$/u;
const ID_PART = /^[\p{ID_Continue}$\u200C\u200D]$/u;
const INLINE_IMPORT = /import\s*\(\s*(["'])([^"'\n]*)\1\s*\)/dy;
const NUMBER = /-?\s*\.?\d[\w.]*/y;
const TYPE_OPERATORS = new Set(["keyof", "typeof", "unique", "infer", "readonly"]);
const BRACKETS = new Map([["(", ")"], ["[", "]"], ["{", "}"], ["<", ">"]]);
const REGEX_AFTER_KEYWORD = new Set([
  "return", "typeof", "instanceof", "in", "of", "new", "delete", "void", "throw", "case", "do",
  "else", "yield", "await", "default",
]);

const findings = [];

function report(file, line, message) {
  findings.push({ file, line, message });
}

const relative = (absolute) => path.relative(root, absolute).split(path.sep).join("/");
// Names come from the files under check; control characters are replaced so they cannot reach the
// terminal as escape sequences.
const printable = (value) => String(value).replace(/[\u0000-\u001f\u007f-\u009f]/g, "\uFFFD");
const escapeRegExp = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const byName = (a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);

function readText(file) {
  try {
    return fs.readFileSync(path.join(root, file), "utf8").replace(/^\uFEFF/, "");
  } catch (error) {
    report(file, 1, `cannot read file (${error?.code ?? "unknown error"})`);
    return null;
  }
}

// The parser's own message can quote file content, so only the position it names is reported.
function jsonErrorLine(text, message) {
  const lineColumn = /line (\d+) column \d+/.exec(message);
  if (lineColumn) return Number(lineColumn[1]);
  const position = /position (\d+)/.exec(message);
  if (position) return text.slice(0, Number(position[1])).split("\n").length;
  return 1;
}

function readJson(file) {
  const text = readText(file);
  if (text === null) return null;
  try {
    return { text, lines: text.split("\n"), value: JSON.parse(text) };
  } catch (error) {
    report(file, jsonErrorLine(text, String(error?.message)), "invalid JSON");
    return null;
  }
}

// 1-based line of the first `"<key>":` at or after line index `from`, then anywhere; 1 if absent.
function keyLine(lines, key, from = 0) {
  const pattern = new RegExp(`${escapeRegExp(JSON.stringify(key))}\\s*:`);
  const at = (start) => lines.findIndex((line, index) => index >= start && pattern.test(line));
  const found = at(from) >= 0 ? at(from) : at(0);
  return found >= 0 ? found + 1 : 1;
}

function valueLine(lines, value, from) {
  const quoted = JSON.stringify(value);
  const found = lines.findIndex((line, index) => index >= from && line.includes(quoted));
  return found >= 0 ? found + 1 : from + 1;
}

function readApproved() {
  const file = readJson(APPROVED_JSON);
  if (file === null) return null;
  const { value, lines } = file;
  const keys = isObject(value) ? Object.keys(value) : [];
  if (keys.length !== 1 || keys[0] !== "packages" || !Array.isArray(value.packages)) {
    report(
      APPROVED_JSON,
      1,
      'expected an object whose only key is "packages", an array of package names',
    );
    return null;
  }
  const listLine = keyLine(lines, "packages");
  const approved = new Set();
  value.packages.forEach((name, index) => {
    if (typeof name === "string" && PACKAGE_NAME.test(name)) {
      approved.add(name);
    } else {
      const line = typeof name === "string" ? valueLine(lines, name, listLine - 1) : listLine;
      const message = `packages[${index}] is not a package name (names only, no versions)`;
      report(APPROVED_JSON, line, message);
    }
  });
  return approved;
}

// Whether `text` is a registry version: a semver range set or a dist-tag.
function registryVersion(text) {
  const trimmed = text.trim();
  const range = trimmed.split("||").every((part) => {
    const comparators = part.trim();
    if (comparators === "") return true;
    const hyphen = HYPHEN_RANGE.exec(comparators);
    if (hyphen) return VERSION.test(hyphen[1]) && VERSION.test(hyphen[2]);
    return comparators
      .replace(OPERATOR_SPACE, "$1")
      .split(/\s+/)
      .every((comparator) => COMPARATOR.test(comparator));
  });
  return range || (DIST_TAG.test(trimmed) && !TARBALL.test(trimmed));
}

// What `spec` is when it does not resolve to the npm registry package `name`, or null when it does;
// `name` is null for an override selector that names no package. An override may also take a
// direct dependency's spec by reference (`$<name>`). The spec is never quoted, since it can carry
// URL credentials or local paths: only an alias target that is a package name reaches the message.
function foreignSpec(spec, name, override) {
  if (typeof spec !== "string") return { kind: "a non-string value" };
  const text = spec.trim();
  if (NPM_ALIAS.test(text)) {
    const alias = text.slice("npm:".length);
    const at = alias.indexOf("@", 1);
    const target = at < 0 ? alias : alias.slice(0, at);
    if (!PACKAGE_NAME.test(target)) return { kind: "a malformed npm alias" };
    if (target !== name) return { kind: `an npm alias of ${printable(target)}`, target };
    if (at < 0 || registryVersion(alias.slice(at + 1))) return null;
    return { kind: "a malformed npm alias" };
  }
  if (registryVersion(text)) return null;
  if (override && text.startsWith("$") && PACKAGE_NAME.test(text.slice(1))) return null;
  return { kind: sourceKind(text) };
}

// The kind of source a spec that is neither a registry version nor an npm alias names.
function sourceKind(text) {
  if (LOCAL_PATH.test(text)) return "a local path spec";
  const protocol = PROTOCOL.exec(text)?.[1].toLowerCase();
  if (protocol !== undefined) {
    if (GIT_PROTOCOL.test(protocol)) return "a git or URL spec";
    if (PATH_PROTOCOL.has(protocol)) return "a local path spec";
    return protocol === "workspace" ? "a workspace protocol spec" : "a non-registry protocol spec";
  }
  if (TARBALL.test(text)) return "a local path spec";
  if (SCP_GIT.test(text) || GITHUB_SHORTHAND.test(text)) return "a git or URL spec";
  return /[\\/]/.test(text) ? "a local path spec" : "an unrecognised spec";
}

// `subject` names the registry package the spec should resolve to; `name` is that package, or null.
function reportSpec(line, label, section, name, subject, { kind, target }) {
  const prohibited = [name, target].some(
    (candidate) => typeof candidate === "string" && COMPONENT_LIBRARY.test(candidate),
  );
  const prefix = prohibited ? "prohibited component library: " : "";
  report(
    PACKAGE_JSON,
    line,
    `${prefix}${printable(label)} (${section}) is ${kind}, which does not resolve to the npm ` +
      `registry package ${subject}; ${COMPONENT_POLICY}`,
  );
}

// Single Component System.
function checkDependencies() {
  const approved = readApproved();
  const manifest = readJson(PACKAGE_JSON);
  if (approved === null || manifest === null) return;
  if (!isObject(manifest.value)) {
    report(PACKAGE_JSON, 1, "expected a JSON object");
    return;
  }
  for (const section of DEPENDENCY_SECTIONS) {
    if (!Object.hasOwn(manifest.value, section)) continue;
    const sectionLine = keyLine(manifest.lines, section);
    const entries = manifest.value[section];
    if (!isObject(entries)) {
      report(PACKAGE_JSON, sectionLine, `${section} must be an object keyed by package name`);
      continue;
    }
    // An approved name passes only when its spec resolves to the npm registry package of that
    // name, so an alias or a git, URL or path spec cannot swap another library in under it. An
    // unapproved name is reported once, unless its spec also aliases a component library.
    for (const [name, spec] of Object.entries(entries)) {
      const line = keyLine(manifest.lines, name, sectionLine - 1);
      const foreign = foreignSpec(spec, name, false);
      if (!approved.has(name)) {
        const prefix = COMPONENT_LIBRARY.test(name) ? "prohibited component library: " : "";
        const message = `${printable(name)} (${section}) is not in ${APPROVED_JSON}`;
        report(PACKAGE_JSON, line, `${prefix}${message}; ${COMPONENT_POLICY}`);
        if (!COMPONENT_LIBRARY.test(foreign?.target ?? "")) continue;
      }
      if (foreign !== null) reportSpec(line, name, section, name, "of that name", foreign);
    }
  }
  checkOverrides(manifest);
}

// Overrides and resolutions replace what a package resolves to, so their specs obey the same rule,
// judged against the package each selector targets; a `$<name>` reference and pnpm's `-` removal
// are allowed too. npm overrides nest: an object value holds overrides for the key's subtree, in
// which `.` stands for the key's own package.
function checkOverrides({ value, lines }) {
  const pnpm = isObject(value.pnpm) && Object.hasOwn(value.pnpm, "overrides") ? value.pnpm : null;
  const pnpmLine = pnpm === null ? 0 : keyLine(lines, "overrides", keyLine(lines, "pnpm") - 1);
  let npmLine = keyLine(lines, "overrides");
  if (npmLine === pnpmLine) npmLine = keyLine(lines, "overrides", npmLine);
  const resolutionsLine = keyLine(lines, "resolutions");
  const sections = [
    ["pnpm.overrides", pnpm !== null, pnpm?.overrides, pnpmLine],
    ["resolutions", Object.hasOwn(value, "resolutions"), value.resolutions, resolutionsLine],
    ["overrides", Object.hasOwn(value, "overrides"), value.overrides, npmLine],
  ];
  for (const [section, present, entries, sectionLine] of sections) {
    if (!present) continue;
    if (isObject(entries)) {
      checkOverrideEntries(lines, section, entries, sectionLine, [], null);
    } else {
      report(PACKAGE_JSON, sectionLine, `${section} must be an object keyed by package selector`);
    }
  }
}

function checkOverrideEntries(lines, section, entries, parentLine, parents, parentTarget) {
  for (const [selector, spec] of Object.entries(entries)) {
    const line = keyLine(lines, selector, parentLine - 1);
    const keys = [...parents, selector];
    const target = selector === "." && parents.length > 0 ? parentTarget : overrideTarget(selector);
    if (section === "overrides" && isObject(spec)) {
      checkOverrideEntries(lines, section, spec, line, keys, target);
      continue;
    }
    const foreign = foreignSpec(spec, target, true);
    if (foreign === null) continue;
    const subject = target === null ? "it overrides" : printable(target);
    reportSpec(line, keys.join(" > "), section, target, subject, foreign);
  }
}

// The package an override selector targets: the last name in a pnpm `parent>child` or a yarn
// `a/b` selector, without its version range; null when the selector names no package.
function overrideTarget(selector) {
  let start = 0;
  for (const match of selector.matchAll(SELECTOR_DELIMITER)) start = match.index + 2;
  const parts = selector.slice(start).trim().split("/");
  let name = parts.at(-1);
  if (parts.length > 1 && parts.at(-2).startsWith("@")) name = `${parts.at(-2)}/${name}`;
  const at = name.indexOf("@", 1);
  if (at > 0) name = name.slice(0, at);
  return PACKAGE_NAME.test(name) ? name : null;
}

function schemaNames() {
  const file = readJson(OPENAPI_JSON);
  if (file === null) return new Set();
  if (!isObject(file.value)) {
    report(OPENAPI_JSON, 1, "expected a JSON object");
    return new Set();
  }
  const components = file.value.components ?? {};
  const schemas = isObject(components) ? (components.schemas ?? {}) : null;
  if (!isObject(schemas)) {
    report(OPENAPI_JSON, keyLine(file.lines, "components"), "components.schemas must be an object");
    return new Set();
  }
  return new Set(Object.keys(schemas));
}

// Every TypeScript source under `directory`, in code-unit order of names. node_modules is skipped.
// Symbolic links are followed; a directory reached twice through links is walked once.
function sourceFiles(directory) {
  const files = [];
  const walked = new Set();
  const walk = (current) => {
    let entries;
    try {
      const real = fs.realpathSync(current);
      if (walked.has(real)) return;
      walked.add(real);
      entries = fs.readdirSync(current, { withFileTypes: true }).sort(byName);
    } catch (error) {
      report(relative(current), 1, `cannot read directory (${error?.code ?? "unknown error"})`);
      return;
    }
    for (const entry of entries) {
      const full = path.join(current, entry.name);
      let kind = entry;
      if (entry.isSymbolicLink()) {
        try {
          kind = fs.statSync(full);
        } catch (error) {
          report(relative(full), 1, `cannot follow link (${error?.code ?? "unknown error"})`);
          continue;
        }
      }
      if (kind.isDirectory()) {
        if (entry.name !== "node_modules") walk(full);
      } else if (kind.isFile() && SOURCE_FILE.test(entry.name)) {
        files.push(full);
      }
    }
  };
  walk(directory);
  return files;
}

// Returns two copies of `text` with identical offsets and line breaks: `code`, with comments
// blanked, and `skeleton`, with the contents of string, template and regular-expression literals,
// of JSX text and of JSX attribute strings blanked as well. Template `${…}` and JSX `{…}`
// expressions stay code, nested templates and elements included. `jsx` is set for a .tsx file.
//
// In a .tsx file, a `<` where an expression may begin (where a regular expression may, below)
// opens a JSX element or fragment when a name or `>` follows it, unless TypeScript reads it as the
// type parameters of a generic arrow function (`<T,>`, `<T = U>`, `<T extends U>`). In a tag the
// lexer reads names, attributes, attribute strings, comments, type arguments, `{…}` and elements
// given as attribute values; in children, text, `{…}`, nested elements and a closing tag, which
// must name its element. Comments exist only in code and in tags, so a `/*`, `//` or quote in JSX
// text hides nothing. An element that meets a character JSX text forbids (`>` or `}`), a
// mismatched closing tag or any other malformed token is abandoned, with the elements it lies in
// up to the nearest one opened from code; the end of the file abandons every open element. The
// lexer then reads that `<` and what follows it again as code, so `let f: <T>(x: T) => T` is code.
// An abandoned `<` never opens an element again, and the `{…}` expressions closed inside abandoned
// elements keep their masks, so the work stays linear in the length of the file.
//
// Limits of this lexer, which has no TypeScript parser behind it:
// - After an operator, an opening bracket, a separator, `...`, a keyword such as `return` or
//   `default`, or the `)` of an `if`, `for`, `while` or `with` header, a `/` begins a regular
//   expression and a `<` may begin an element; after an operand (a name, a literal, `)`, `]`, an
//   element, the type arguments of a type after `as` or `satisfies`, or a postfix `!`, `++` or
//   `--`) both are operators. So a `/` after the `}` of an object literal begins a regular
//   expression, as does one after other type arguments (`f<T> / 2`). A regular expression ends
//   at a line break at the latest, and a quoted string at an unescaped line break, where a real
//   one would be a syntax error.
// - A file that is not valid TypeScript can be misread. One that makes the lexer read more than
//   `budget` characters again is left unmasked from the first offset whose masks are not final,
//   which hides nothing from the scan but exposes what its comments and strings quote.
function maskSource(text, jsx = false) {
  // Sticky matchers, which read by code point (the `u` flag), never by code unit: `identifierStart`
  // tests for one character that can begin an identifier (`$`, `_` or an ID_Start character);
  // `identifier` reads a whole identifier, whose later characters are ID_Continue characters, `$`
  // or the joiners U+200C and U+200D; `jsxName` reads one part of a JSX element or attribute name,
  // which may also contain `-`.
  const identifierStart = /[$_\p{ID_Start}]/uy;
  const identifier = /[$_\p{ID_Start}][$\u200c\u200d\p{ID_Continue}]*/uy;
  const jsxName = /[$_\p{ID_Start}][$\u200c\u200d\p{ID_Continue}-]*/uy;
  // The `)` that ends the header of one of these is followed by a statement, not by an operator.
  const headerKeywords = new Set(["if", "for", "while", "with"]);
  const n = text.length;
  const code = text.split("");
  const skeleton = text.split("");
  // `spans`: the JSX `{…}` expressions closed so far and not inside another closed one, as
  // { start, end } in order. `pending`: those of abandoned elements whose masks still stand,
  // nearest last; the code reading passes over each one it reaches at its `{` and restores any
  // other it enters. Every offset from `reach` on holds its character of `text`, except in a
  // pending span.
  const spans = [];
  const pending = [];
  let pendingFrom = Infinity;
  let reach = 0;
  // Characters read again after an element is abandoned; valid TypeScript stays below its length.
  let reread = 0;
  const budget = 4 * n + 65536;
  const restore = (from, to) => {
    for (let k = from; k < to; k += 1) {
      code[k] = text[k];
      skeleton[k] = text[k];
    }
  };
  // Restores every pending span that starts at or before `at`.
  const settle = (at) => {
    while (pendingFrom <= at) {
      const span = pending.pop();
      restore(span.start, span.end);
      reread += span.end - span.start;
      pendingFrom = pending.length > 0 ? pending.at(-1).start : Infinity;
    }
  };
  const blank = (chars, at) => {
    if (at >= pendingFrom) settle(at);
    if (chars[at] !== "\n" && chars[at] !== "\r") chars[at] = " ";
    if (at >= reach) reach = at + 1;
  };
  const blankBoth = (at) => {
    blank(code, at);
    blank(skeleton, at);
  };
  const lineBreak = (at) => text[at] === "\n" || text[at] === "\r";
  // Open contexts, innermost last: `${…}` ("template") and JSX `{…}` ("container") expressions
  // with their brace depth, JSX type arguments ("typeArgs") with their angle depth, and JSX
  // elements. Each expression keeps its own `(` and `<` stacks, and `topParens` and `topAngles`
  // are the file's.
  const frames = [];
  const topParens = [];
  const topAngles = [];
  // Offsets of the `<` of every abandoned element.
  const abandoned = new Set();
  let elements = 0;
  let mode = "code";
  let last = -1;
  // The `>` that ended an element read from code or the type arguments of a type after `as` or
  // `satisfies` (`x as A<B>`), and the `)` that ended a control header.
  let closedAt = -1;
  let controlAt = -1;
  let i = 0;

  // Offset where the identifier ending just before `end` begins, walking back by code point.
  const wordStart = (end) => {
    let start = end;
    while (start > 0) {
      const low = text.charCodeAt(start - 1);
      const high = start > 1 ? text.charCodeAt(start - 2) : 0;
      const width = low >= 0xdc00 && low <= 0xdfff && high >= 0xd800 && high <= 0xdbff ? 2 : 1;
      if (!ID_PART.test(text.slice(start - width, start))) break;
      start -= width;
    }
    return start;
  };
  // Offset of the last character before `at` that is neither white space nor in a comment.
  const previousAt = (at) => {
    let before = at - 1;
    while (before >= 0 && /\s/.test(code[before])) before -= 1;
    return before;
  };
  // The word ending at offset `end`, or "" when none does or it names a property (`x.default`).
  const keywordAt = (end) => {
    const start = wordStart(end + 1);
    if (start > end) return "";
    const before = previousAt(start);
    if (text[before] === "#" || (text[before] === "." && text[before - 1] !== ".")) return "";
    return text.slice(start, end + 1);
  };
  // Whether the `(` at `i` opens an `if`, `for`, `for await`, `while` or `with` header.
  const opensHeader = () => {
    if (last < 0 || !"efhrt".includes(text[last])) return false;
    const word = keywordAt(last);
    if (word === "await") return keywordAt(previousAt(wordStart(last + 1))) === "for";
    return headerKeywords.has(word);
  };
  // Whether the `<` at `i` follows a type name after `as` or `satisfies` (`x as ns.A<`).
  const opensTypeArguments = () => {
    let start = wordStart(last + 1);
    if (start > last) return false;
    let before = previousAt(start);
    while (text[before] === "." && text[before - 1] !== ".") {
      const end = previousAt(before);
      start = wordStart(end + 1);
      if (start > end) return false;
      before = previousAt(start);
    }
    const word = keywordAt(before);
    return word === "as" || word === "satisfies";
  };

  // Whether the character at `at` ends an operand, so that an operator, not an operand, follows.
  const endsOperand = (at) => {
    if (at === closedAt || text[at] === "]" || (text[at] === ")" && at !== controlAt)) return true;
    return wordStart(at + 1) <= at && !REGEX_AFTER_KEYWORD.has(keywordAt(at));
  };
  const regexAllowed = () => {
    if (last < 0) return true;
    if (last === closedAt) return false;
    if (last === controlAt) return true;
    const previous = text[last];
    // `x++`, `x--` and the non-null assertion `x!` end an operand.
    if ((previous === "+" || previous === "-") && text[last - 1] === previous) return false;
    if (previous === "!" && endsOperand(previousAt(last))) return false;
    // A spread or rest `...` comes before an operand.
    if (previous === "." && text[last - 1] === "." && text[last - 2] === ".") return true;
    if (/[(,=:[!&|?{};+\-*%<>~^]/.test(previous)) return true;
    return REGEX_AFTER_KEYWORD.has(keywordAt(last));
  };

  // Offset of the first character at or after `at` that is neither white space nor in a comment.
  // With `apply`, the comments passed over are blanked in both copies.
  const trivia = (at, apply) => {
    let j = at;
    for (;;) {
      while (j < n && /\s/.test(text[j])) j += 1;
      if (text[j] !== "/" || (text[j + 1] !== "/" && text[j + 1] !== "*")) return j;
      let end = j + 2;
      if (text[j + 1] === "/") {
        while (end < n && !lineBreak(end)) end += 1;
      } else {
        const close = text.indexOf("*/", j + 2);
        end = close < 0 ? n : close + 2;
      }
      if (apply) for (let k = j; k < end; k += 1) blankBoth(k);
      j = end;
    }
  };

  // The JSX element name at `at` (`a`, `a-b`, `a.b.c` or `a:b`) without white space or comments,
  // and the offset after it; null when no name starts there.
  const readName = (at) => {
    jsxName.lastIndex = at;
    const first = jsxName.exec(text);
    if (first === null) return null;
    let name = first[0];
    let end = at + name.length;
    for (;;) {
      const separator = trivia(end, false);
      if (text[separator] !== "." && text[separator] !== ":") break;
      const partAt = trivia(separator + 1, false);
      jsxName.lastIndex = partAt;
      const part = jsxName.exec(text);
      if (part === null) break;
      trivia(end, true);
      trivia(separator + 1, true);
      name += text[separator] + part[0];
      end = partAt + part[0].length;
    }
    return { name, end };
  };

  // Whether the `<` at `i`, in code, opens a JSX element. TypeScript reads `<T,>`, `<T = U>` and
  // `<T extends U>`, each with an optional `const` before `T`, as type parameters instead.
  const opensElement = () => {
    if (!jsx || abandoned.has(i) || frames.at(-1)?.kind === "typeArgs" || !regexAllowed()) {
      return false;
    }
    const at = trivia(i + 1, false);
    if (text[at] === ">") return true;
    identifier.lastIndex = at;
    let word = identifier.exec(text);
    if (word === null) return false;
    let after = at + word[0].length;
    if (word[0] === "const") {
      const nameAt = trivia(after, false);
      identifier.lastIndex = nameAt;
      word = identifier.exec(text);
      if (word !== null) after = nameAt + word[0].length;
    }
    after = trivia(after, false);
    if (text[after] === ",") return false;
    if (text[after] === "=") return text[after + 1] === "=" || text[after + 1] === ">";
    identifier.lastIndex = after;
    if (identifier.exec(text)?.[0] !== "extends") return true;
    const fourth = text[trivia(after + "extends".length, false)];
    return fourth === "=" || fourth === ">" || fourth === "/";
  };
  // Whether the `<` at `i`, in a tag or in children, can open a nested element.
  const opensNested = () => {
    if (abandoned.has(i)) return false;
    const at = trivia(i + 1, false);
    identifierStart.lastIndex = at;
    return text[at] === ">" || identifierStart.test(text);
  };

  // Opens the element whose `<` is at `i`, read from `back`: "code", "children" or "tag".
  const openElement = (back) => {
    const element = { kind: "element", start: i, spans: spans.length, back, name: "" };
    Object.assign(element, { afterName: false, afterEquals: false, last, closedAt, controlAt });
    frames.push(element);
    elements += 1;
    const at = trivia(i + 1, true);
    if (text[at] === ">") {
      i = at + 1;
      mode = "children";
      return;
    }
    const tag = readName(at);
    element.name = tag.name;
    element.afterName = true;
    i = tag.end;
    mode = "tag";
  };
  const closeElement = () => {
    const element = frames.pop();
    elements -= 1;
    mode = element.back;
    if (mode === "code") {
      last = i - 1;
      closedAt = i - 1;
    }
  };
  // Abandons the innermost element and the elements it lies in up to the nearest one opened from
  // code (at the end of the file, every open element), and resumes reading that one's `<` as code.
  // Both copies are restored from the `<` on, except in the `{…}` expressions closed meanwhile:
  // read as code from the same state, they lex as they did, so the reading passes over each one it
  // reaches at its `{` and restores the others when it enters them.
  const abandon = () => {
    settle(i - 1);
    let at = frames.length - 1;
    if (i >= n) {
      at = frames.findIndex((frame) => frame.kind === "element");
    } else {
      while (frames[at].kind !== "element") at -= 1;
      while (frames[at].back !== "code") at -= 1;
    }
    const element = frames[at];
    for (const frame of frames.slice(at)) {
      if (frame.kind !== "element") continue;
      abandoned.add(frame.start);
      elements -= 1;
    }
    const to = Math.min(reach, pendingFrom);
    let from = element.start;
    reread += Math.min(i, n) - from;
    for (const span of spans.slice(element.spans)) {
      restore(from, Math.min(span.start, to));
      from = span.end;
      reread -= span.end - span.start;
    }
    restore(from, to);
    for (let k = spans.length - 1; k >= element.spans; k -= 1) pending.push(spans[k]);
    spans.length = element.spans;
    if (pending.length > 0) pendingFrom = pending.at(-1).start;
    reach = Math.min(reach, element.start);
    frames.length = at;
    ({ last, closedAt, controlAt } = element);
    i = element.start;
    mode = "code";
  };
  const openExpression = (back) => {
    const container = { kind: "container", start: i, spans: spans.length, depth: 0, back };
    frames.push(Object.assign(container, { parens: [], angles: [] }));
    mode = "code";
    last = i;
    i += 1;
  };

  while (i < n || elements > 0) {
    if (reread > budget) {
      const open = frames.find((frame) => frame.kind === "element");
      restore(Math.min(i, pendingFrom, open?.start ?? n), n);
      break;
    }
    if (i >= n) {
      abandon();
      continue;
    }
    if (i >= pendingFrom) {
      if (i === pendingFrom && mode === "code") {
        const span = pending.pop();
        pendingFrom = pending.length > 0 ? pending.at(-1).start : Infinity;
        reach = Math.max(reach, span.end);
        last = span.end - 1;
        i = span.end;
        continue;
      }
      settle(i);
    }
    const c = text[i];
    const next = text[i + 1];
    if (mode === "template") {
      if (c === "\\") {
        blank(skeleton, i);
        if (i + 1 < n) blank(skeleton, i + 1);
        i += 2;
      } else if (c === "`") {
        mode = "code";
        last = i;
        i += 1;
      } else if (c === "$" && next === "{") {
        frames.push({ kind: "template", depth: 0, parens: [], angles: [] });
        mode = "code";
        last = i + 1;
        i += 2;
      } else {
        blank(skeleton, i);
        i += 1;
      }
      continue;
    }
    if (mode === "tag") {
      const element = frames.at(-1);
      if (/\s/.test(c) || (c === "/" && (next === "/" || next === "*"))) {
        i = trivia(i, true);
        continue;
      }
      const afterName = element.afterName;
      const afterEquals = element.afterEquals;
      element.afterName = false;
      element.afterEquals = false;
      jsxName.lastIndex = i;
      const name = jsxName.exec(text);
      if (c === "/" && next === ">") {
        i += 2;
        closeElement();
      } else if (c === ">") {
        i += 1;
        mode = "children";
      } else if (c === "<" && afterName) {
        frames.push({ kind: "typeArgs", depth: 1, parens: [] });
        mode = "code";
        last = i;
        i += 1;
      } else if (c === "<" && afterEquals && opensNested()) {
        openElement("tag");
      } else if (c === '"' || c === "'") {
        i += 1;
        while (i < n && text[i] !== c) blank(skeleton, i++);
        i += 1;
      } else if (c === "{") {
        openExpression("tag");
      } else if (c === "=") {
        element.afterEquals = true;
        i += 1;
      } else if (name !== null) {
        i += name[0].length;
      } else if (c === ":") {
        i += 1;
      } else {
        abandon();
      }
      continue;
    }
    if (mode === "children") {
      if (c === "{") {
        openExpression("children");
      } else if (c === "<" && next === "/") {
        let at = trivia(i + 2, true);
        const tag = text[at] === ">" ? { name: "", end: at } : readName(at);
        if (tag !== null) at = trivia(tag.end, true);
        if (tag === null || text[at] !== ">" || tag.name !== frames.at(-1).name) {
          abandon();
        } else {
          i = at + 1;
          closeElement();
        }
      } else if (c === "<" && opensNested()) {
        openElement("children");
      } else if (c === "<" || c === ">" || c === "}") {
        abandon();
      } else {
        blank(skeleton, i);
        i += 1;
      }
      continue;
    }
    if (c === "/" && next === "/") {
      while (i < n && !lineBreak(i)) blankBoth(i++);
    } else if (c === "/" && next === "*") {
      const close = text.indexOf("*/", i + 2);
      const end = close < 0 ? n : close + 2;
      while (i < end) blankBoth(i++);
    } else if (c === '"' || c === "'") {
      i += 1;
      while (i < n && text[i] !== c && !lineBreak(i)) {
        if (text[i] === "\\") {
          blank(skeleton, i);
          i += 1;
          if (text[i] === "\r" && text[i + 1] === "\n") i += 2;
          else if (i < n) blank(skeleton, i++);
        } else {
          blank(skeleton, i++);
        }
      }
      last = i < n && text[i] === c ? i++ : i - 1;
    } else if (c === "`") {
      mode = "template";
      i += 1;
    } else if (c === "/" && regexAllowed()) {
      let inClass = false;
      i += 1;
      while (i < n && !lineBreak(i)) {
        const r = text[i];
        if (r === "/" && !inClass) break;
        if (r === "[") inClass = true;
        else if (r === "]") inClass = false;
        blank(skeleton, i);
        if (r === "\\" && i + 1 < n && !lineBreak(i + 1)) blank(skeleton, ++i);
        i += 1;
      }
      if (i < n && text[i] === "/") {
        i += 1;
        while (i < n && /[\w$]/.test(text[i])) i += 1;
      }
      last = i - 1;
    } else if (c === "<" && opensElement()) {
      openElement("code");
    } else {
      const frame = frames.at(-1);
      if (frame?.kind === "typeArgs") {
        if (c === "<") frame.depth += 1;
        else if (c === ">" && text[i - 1] !== "=") frame.depth -= 1;
        if (frame.depth === 0) {
          frames.pop();
          mode = "tag";
          i += 1;
          continue;
        }
      } else if (frame !== undefined && c === "{") {
        frame.depth += 1;
      } else if (frame !== undefined && c === "}") {
        if (frame.depth === 0) {
          frames.pop();
          mode = frame.kind === "template" ? "template" : frame.back;
          i += 1;
          if (frame.kind === "container") {
            spans.length = frame.spans;
            spans.push({ start: frame.start, end: i });
          }
          continue;
        }
        frame.depth -= 1;
      } else if (c === "<") {
        (frame?.angles ?? topAngles).push(opensTypeArguments());
      } else if (c === ">" && text[i - 1] !== "=" && (frame?.angles ?? topAngles).pop()) {
        closedAt = i;
      }
      const parens = frame?.parens ?? topParens;
      if (c === "(") {
        parens.push(opensHeader());
      } else if (c === ")" && parens.pop()) {
        controlAt = i;
      }
      if (!/\s/.test(c)) last = i;
      i += 1;
    }
  }
  settle(n);
  return { code: code.join(""), skeleton: skeleton.join("") };
}

function lineLocator(text) {
  const starts = [0];
  for (let at = text.indexOf("\n"); at >= 0; at = text.indexOf("\n", at + 1)) starts.push(at + 1);
  return (offset) => {
    let low = 0;
    let high = starts.length - 1;
    while (low < high) {
      const middle = (low + high + 1) >> 1;
      if (starts[middle] <= offset) low = middle;
      else high = middle - 1;
    }
    return low + 1;
  };
}

const skipSpace = (text, at) => {
  let i = at;
  while (i < text.length && /\s/.test(text[i])) i += 1;
  return i;
};

const hasLineBreak = (text, from, to) => /[\n\r\u2028\u2029]/.test(text.slice(from, to));

// The identifier `raw` spells, with its `\u` escapes decoded, or null when an escape stands for a
// character an identifier cannot hold at that place.
function canonicalName(raw) {
  if (!raw.includes("\\")) return raw;
  let name = "";
  for (const [unit, braced, plain] of raw.matchAll(IDENTIFIER_UNIT)) {
    const hex = braced ?? plain;
    const point = hex === undefined ? unit.codePointAt(0) : Number.parseInt(hex, 16);
    if (point > 0x10ffff) return null;
    const char = String.fromCodePoint(point);
    if (!(name === "" ? ID_START : ID_PART).test(char)) return null;
    name += char;
  }
  return name;
}

// The whole identifier at `at`: its decoded name (null when invalid) and the offset past it.
function identifierAt(text, at) {
  IDENTIFIER_AT.lastIndex = at;
  const match = IDENTIFIER_AT.exec(text);
  return match === null ? null : { name: canonicalName(match[0]), end: at + match[0].length };
}

// Local names of the generated roots, mapped to the root they stand for: `components` and `paths`
// themselves, every `as` rename imported from a module whose path ends in `schema`, and
// `<namespace>.components` / `<namespace>.paths` for a namespace import of such a module. Imports
// are found in the skeleton, so import-shaped text inside a literal is not one, and the module
// path is read from `code` at the same offsets. Names are kept decoded.
function generatedRoots(skeleton, code) {
  const roots = new Map([...GENERATED_ROOTS].map((name) => [name, name]));
  for (const match of skeleton.matchAll(IMPORT_DECLARATION)) {
    if (!SCHEMA_MODULE.test(code.slice(...match.indices[3]))) continue;
    const clause = match[1];
    const namespace = NAMESPACE_IMPORT.exec(clause);
    const prefix = namespace && canonicalName(namespace[1]);
    if (prefix) {
      for (const name of GENERATED_ROOTS) roots.set(`${prefix}.${name}`, name);
    }
    const named = /\{([^}]*)\}/.exec(clause);
    for (const specifier of named ? named[1].split(",") : []) {
      const binding = IMPORT_SPECIFIER.exec(specifier);
      if (binding === null) continue;
      const imported = canonicalName(binding[1]);
      const local = canonicalName(binding[2] ?? binding[1]);
      if (GENERATED_ROOTS.has(imported) && local !== null) roots.set(local, imported);
    }
  }
  return roots;
}

// Offset just past the `=` of a type alias whose name ends at `at`, or -1 when `type <Name>` is not
// followed by optional type parameters and `=` (an import or export specifier, for instance).
function aliasBodyStart(skeleton, at) {
  let i = skipSpace(skeleton, at);
  if (skeleton[i] === "<") {
    let depth = 0;
    for (; i < skeleton.length; i += 1) {
      if (skeleton[i] === "<") depth += 1;
      else if (skeleton[i] === ">" && skeleton[i - 1] !== "=") depth -= 1;
      if (depth === 0) break;
    }
    if (depth !== 0) return -1;
    i = skipSpace(skeleton, i + 1);
  }
  if (skeleton[i] !== "=" || skeleton[i + 1] === "=" || skeleton[i + 1] === ">") return -1;
  return i + 1;
}

// The type reader below works on the skeleton, so literal contents cannot pose as syntax, and
// follows TypeScript's precedence and line-break rules: `|` and `&` continue a type across lines,
// while `[…]` and `<…>` suffixes and a conditional `extends` must stay on the line. For a type it
// returns `found`, the generated root the type amounts to, as { shown, generated, bare }: the
// reference as the file names it, the root it stands for, and whether it is the root itself rather
// than a type indexed out of it; or null. A union or intersection amounts to its first member that
// amounts to one, a generic reference of any name to its first type argument that does, and a
// conditional type to its check type or else a branch that does, but never to its extends type.
// Composite uses amount to none: object, tuple, literal and template types, function and
// constructor types, and `keyof`/`typeof`/`unique`/`infer` operands.

// Offset of the closing quote of the literal opened at `at`, whose contents the skeleton blanks,
// or of the line break that ends it unterminated.
function quoteClose(s, at) {
  let i = at + 1;
  while (i < s.length && s[i] !== s[at] && s[i] !== "\n" && s[i] !== "\r") i += 1;
  return i;
}

// Offset of the backtick that closes the template opened at `at`, or -1.
function templateClose(s, at) {
  for (let i = at + 1; i < s.length; i += 1) {
    if (s[i] === "`") return i;
    if (s[i] === "$" && s[i + 1] === "{") {
      i = closingBracket(s, i + 1);
      if (i < 0) return -1;
    }
  }
  return -1;
}

// Offset of the bracket that closes the one at `at`, or -1. All four kinds nest, the `>` of `=>`
// closes nothing, literals are skipped, and a `;` outside braces ends the search, so an unclosed
// bracket cannot run past its statement.
function closingBracket(s, at) {
  const expected = [];
  for (let i = at; i < s.length; i += 1) {
    const c = s[i];
    if (BRACKETS.has(c)) {
      expected.push(BRACKETS.get(c));
    } else if (c === ")" || c === "]" || c === "}" || (c === ">" && s[i - 1] !== "=")) {
      if (expected.pop() !== c) return -1;
      if (expected.length === 0) return i;
    } else if (c === '"' || c === "'") {
      i = quoteClose(s, i);
    } else if (c === "`") {
      i = templateClose(s, i);
      if (i < 0) return -1;
    } else if (c === ";" && expected.at(-1) !== "}") {
      return -1;
    }
  }
  return -1;
}

// Start offsets of the comma-separated items between the bracket at `open` and its partner.
function listItems(s, open, close) {
  const starts = [open + 1];
  for (let i = open + 1; i < close; i += 1) {
    const c = s[i];
    if (BRACKETS.has(c)) {
      i = closingBracket(s, i);
      if (i < 0) break;
    } else if (c === '"' || c === "'") {
      i = quoteClose(s, i);
    } else if (c === "`") {
      i = templateClose(s, i);
      if (i < 0) break;
    } else if (c === ",") {
      starts.push(i + 1);
    }
  }
  return starts;
}

// The type at `at`, as { end, found }; `end` is the offset where it stops. `conditional` is false
// where TypeScript admits no conditional type, as in the extends type of one. A conditional type
// `C extends E ? X : Y` amounts to the first of C, X and Y that amounts to a generated root, so a
// conditional nested in a branch follows the same rule. E never counts: a type tested against a
// generated type, or against its keys, is not an alias of it.
function parseType(scan, at, conditional = true) {
  const s = scan.skeleton;
  let i = skipSpace(s, at);
  if (s[i] === "|" || s[i] === "&") i += 1;
  let found = null;
  for (;;) {
    const start = skipSpace(s, i);
    const member = parseMember(scan, start, conditional);
    if (member.end <= start) return { end: start, found };
    found ??= member.found;
    i = member.end;
    const next = skipSpace(s, i);
    if (s[next] !== "|" && s[next] !== "&") break;
    i = next + 1;
  }
  if (!conditional) return { end: i, found };
  const after = skipSpace(s, i);
  const word = identifierAt(s, after);
  if (word?.name !== "extends" || hasLineBreak(s, i, after)) return { end: i, found };
  let end = extendsTypeEnd(scan, word.end);
  for (const separator of ["?", ":"]) {
    const next = skipSpace(s, end);
    if (s[next] !== separator) break;
    const branch = parseType(scan, next + 1);
    found ??= branch.found;
    end = branch.end;
  }
  return { end, found };
}

// End of the type at `at` where no conditional type may stand: the extends type of a conditional
// or the constraint of an `infer`. Ends are kept per offset: `infer U extends C ? …` reads C first
// as the constraint and then as the extends type, so without them each such `infer` nested in C
// would double the work.
function extendsTypeEnd(scan, at) {
  let end = scan.extendsEnds.get(at);
  if (end === undefined) {
    end = parseType(scan, at, false).end;
    scan.extendsEnds.set(at, end);
  }
  return end;
}

// End of the type parameter of an `infer` whose name ends at `at`, past its `extends` constraint
// when it has one. As in TypeScript, the constraint holds no conditional type, and where one may
// follow (`conditional`) it stands only when no `?` comes next: there `infer U extends C ? X : Y`
// is a conditional type checking `infer U`.
function inferEnd(scan, at, conditional) {
  const s = scan.skeleton;
  const word = identifierAt(s, skipSpace(s, at));
  if (word?.name !== "extends") return at;
  const end = extendsTypeEnd(scan, word.end);
  return conditional && s[skipSpace(s, end)] === "?" ? at : end;
}

// One member of a union or intersection: a type operator with its operand, a function or
// constructor type, or a primary type with its `[…]` suffixes. `conditional` passes through the
// operators to an `infer` constraint, as parseType states.
function parseMember(scan, at, conditional = true) {
  const s = scan.skeleton;
  const word = identifierAt(s, at);
  if (word !== null && TYPE_OPERATORS.has(word.name)) {
    const operand = parseMember(scan, skipSpace(s, word.end), conditional);
    const end = word.name === "infer" ? inferEnd(scan, operand.end, conditional) : operand.end;
    return { end, found: word.name === "readonly" ? operand.found : null };
  }
  const signature = signatureEnd(scan, at);
  if (signature >= 0) return { end: signature, found: null };
  let { end, found } = primaryType(scan, at);
  for (;;) {
    const next = skipSpace(s, end);
    if (s[next] !== "[" || hasLineBreak(s, end, next)) break;
    const close = closingBracket(s, next);
    if (close < 0) break;
    if (found?.bare) found = { shown: `${found.shown}[…]`, generated: `${found.generated}[…]` };
    end = close + 1;
  }
  return { end, found };
}

// End of the function or constructor type `[abstract] [new] [<…>] (…) => T` at `at`, or -1.
function signatureEnd(scan, at) {
  const s = scan.skeleton;
  let i = at;
  for (const keyword of ["abstract", "new"]) {
    const word = identifierAt(s, i);
    if (word?.name === keyword) i = skipSpace(s, word.end);
  }
  if (s[i] === "<") {
    const close = closingBracket(s, i);
    if (close < 0) return -1;
    i = skipSpace(s, close + 1);
  }
  if (s[i] !== "(") return -1;
  const close = closingBracket(s, i);
  if (close < 0) return -1;
  const arrow = skipSpace(s, close + 1);
  return s[arrow] === "=" && s[arrow + 1] === ">" ? parseType(scan, arrow + 2).end : -1;
}

// A parenthesised, object, tuple, literal or template type, or a type reference.
function primaryType(scan, at) {
  const s = scan.skeleton;
  const c = s[at];
  if (c === "(" || c === "{" || c === "[") {
    const close = closingBracket(s, at);
    if (close < 0) return { end: at, found: null };
    return { end: close + 1, found: c === "(" ? parseType(scan, at + 1).found : null };
  }
  if (c === '"' || c === "'") return { end: quoteClose(s, at) + 1, found: null };
  if (c === "`") {
    const close = templateClose(s, at);
    return { end: close < 0 ? at : close + 1, found: null };
  }
  NUMBER.lastIndex = at;
  if (NUMBER.test(s)) return { end: NUMBER.lastIndex, found: null };
  return typeReference(scan, at);
}

// A type reference, `Name`, `A.B` or `import("…").A`, with its type arguments. A reference to a
// generated root amounts to that root; any other amounts to its first type argument that does.
function typeReference(scan, at) {
  const { skeleton: s, code, roots } = scan;
  INLINE_IMPORT.lastIndex = at;
  const inline = INLINE_IMPORT.exec(s);
  let end = inline === null ? at : INLINE_IMPORT.lastIndex;
  const names = [];
  for (;;) {
    let start = end;
    if (inline !== null || names.length > 0) {
      const dot = skipSpace(s, end);
      if (s[dot] !== ".") break;
      start = skipSpace(s, dot + 1);
    }
    const word = identifierAt(s, start);
    if (word === null) break;
    names.push(word.name);
    end = word.end;
  }
  let found = null;
  if (inline === null) {
    const shown = names.join(".");
    if (roots.has(shown)) found = { shown, generated: roots.get(shown), bare: true };
  } else if (names.length === 1 && GENERATED_ROOTS.has(names[0])) {
    if (SCHEMA_MODULE.test(code.slice(...inline.indices[2]))) {
      found = { shown: `import(…).${names[0]}`, generated: names[0], bare: true };
    }
  }
  const open = skipSpace(s, end);
  if (s[open] !== "<" || hasLineBreak(s, end, open)) return { end, found };
  const close = closingBracket(s, open);
  if (close < 0) return { end, found };
  found = null;
  for (const start of listItems(s, open, close)) found ??= parseType(scan, start).found;
  return { end: close + 1, found };
}

// The generated root that a heritage type of the interface whose name ends at `at` amounts to, or
// null. The interface's own type parameter list is skipped, so the `extends` constraints inside it
// are not taken for heritage.
function heritageRoot(scan, at) {
  const s = scan.skeleton;
  let i = skipSpace(s, at);
  if (s[i] === "<") {
    const close = closingBracket(s, i);
    if (close < 0) return null;
    i = skipSpace(s, close + 1);
  }
  const word = identifierAt(s, i);
  if (word?.name !== "extends") return null;
  for (let next = word.end; ; ) {
    const start = skipSpace(s, next);
    const heritage = parseMember(scan, start);
    if (heritage.found !== null) return heritage.found;
    next = skipSpace(s, heritage.end);
    if (heritage.end <= start || s[next] !== ",") return null;
    next += 1;
  }
}

// Generated API Types Only, for one source file.
function scanSource(file, text, names) {
  const { code, skeleton } = maskSource(text, file.endsWith(".tsx"));
  const lineAt = lineLocator(text);
  const scan = { skeleton, code, roots: generatedRoots(skeleton, code), extendsEnds: new Map() };
  for (const match of skeleton.matchAll(DECLARATION)) {
    const [whole, keyword, written] = match;
    const name = canonicalName(written);
    if (name === null) continue;
    const nameEnd = match.index + whole.length;
    const bodyStart = keyword === "type" ? aliasBodyStart(skeleton, nameEnd) : 0;
    if (bodyStart < 0) continue;
    const line = lineAt(match.index);
    if (names.has(name)) {
      const generated = `components["schemas"]["${name}"]`;
      const message = `${keyword} ${name} restates generated schema ${generated}`;
      report(file, line, `${message}; use the generated type`);
    }
    let found = null;
    if (keyword === "type") found = parseType(scan, bodyStart).found;
    else if (keyword === "interface") found = heritageRoot(scan, nameEnd);
    if (found === null) continue;
    const via = found.shown === found.generated ? "" : `, the generated ${found.generated}`;
    const message =
      keyword === "type"
        ? `type ${name} aliases ${found.shown}${via}`
        : `interface ${name} extends ${found.shown}${via}, so it restates a generated type`;
    report(file, line, `${message}; reference the generated type inline`);
  }
}

function checkGeneratedTypes() {
  const names = schemaNames();
  for (const absolute of sourceFiles(path.join(root, SOURCE_DIR))) {
    const file = relative(absolute);
    if (file === GENERATED_SCHEMA) continue;
    const text = readText(file);
    if (text !== null) scanSource(file, text, names);
  }
}

// exitCode rather than process.exit(), so stdout and stderr are flushed before the process ends.
if (invocation.mode === "help") {
  process.stdout.write(USAGE);
  process.exitCode = 0;
} else if (invocation.mode === "usage") {
  // The argument is echoed through printable(), so control characters cannot reach the terminal.
  const shown = JSON.stringify(printable(invocation.argument));
  process.stderr.write(`frontend-policy: ${invocation.problem}: ${shown}\n${USAGE}`);
  process.exitCode = 2;
} else {
  try {
    checkDependencies();
    checkGeneratedTypes();
  } catch (error) {
    report(SELF, 1, `internal error: ${printable(error?.message ?? error)}`);
  }
  for (const { file, line, message } of findings) {
    process.stderr.write(`${file}:${line}: ${message}\n`);
  }
  process.exitCode = findings.length > 0 ? 1 : 0;
}
