#!/usr/bin/env node
// Frontend policy check, run by scripts/check.sh step 7 as `node scripts/frontend-policy.mjs` from
// modern/. It enforces two rules and prints nothing when both hold:
//
// - Single Component System: every package that frontend/package.json names under dependencies,
//   devDependencies, peerDependencies or optionalDependencies must be listed in
//   frontend/approved-dependencies.json, the single source of approved names. shadcn/ui is the only
//   component system.
// - Generated API Types Only: no file under frontend/src other than the generated
//   src/api/schema.d.ts may declare an interface, type alias or enum named after a key of
//   components.schemas in openapi/openapi.json, or a type alias of the generated components[…] or
//   paths[…] types.
//
// Each finding goes to stderr as `<file>:<line>: <message>`, with the path relative to modern/, and
// the exit status is 1 when there is any. An unreadable or malformed input is a finding too. The
// optional argument names another root laid out like modern/, so the checks can be exercised
// against a scratch tree. The script reads files only: it writes nothing and uses no network.

import fs from "node:fs";
import path from "node:path";

const root = path.resolve(process.argv[2] ?? path.join(import.meta.dirname, ".."));

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

// Besides an interface or type alias named after a schema key and an alias of components[…] or
// paths[…], these count as findings: an enum named after a schema key, an alias of a bare root
// (`type C = components`), nested utility wrappers, and roots reached through a namespace import or
// an inline `import(…)` of the generated module. Each restates a generated type by hand, and a bare
// root alias would let every later alias of it pass unseen.
const SOURCE_FILE = /\.(?:ts|tsx|mts|cts)$/;
const GENERATED_ROOTS = new Set(["components", "paths"]);
const SCHEMA_MODULE = /schema(?:\.d)?(?:\.[cm]?[jt]sx?)?$/;
const DECLARATION = /(?<![\w$.])(interface|type|enum)\s+([A-Za-z_$][\w$]*)/g;
const IMPORT_DECLARATION =
  /(?<![\w$.])import\s+(?:type\s+)?([^'";]*?)\s*\bfrom\s*(["'])([^"'\n]*)\2/g;
const IMPORT_SPECIFIER = /^\s*(?:type\s+)?([A-Za-z_$][\w$]*)(?:\s+as\s+([A-Za-z_$][\w$]*))?\s*$/;
const WRAPPER = /(?:NonNullable|Readonly|Partial|Required|Pick|Omit|Exclude|Extract)\s*</y;
const STATEMENT_END = /[ \t]*(?:;|\r|\n|$)/y;
const INLINE_IMPORT_ROOT =
  /import\s*\(\s*(["'])([^"'\n]*)\1\s*\)\s*\.\s*(components|paths)(?![\w$])/y;
const REGEX_AFTER_KEYWORD = new Set([
  "return", "typeof", "instanceof", "in", "of", "new", "delete", "void", "throw", "case", "do",
  "else", "yield", "await",
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
    for (const name of Object.keys(entries)) {
      if (approved.has(name)) continue;
      const prefix = COMPONENT_LIBRARY.test(name) ? "prohibited component library: " : "";
      report(
        PACKAGE_JSON,
        keyLine(manifest.lines, name, sectionLine - 1),
        `${prefix}${printable(name)} (${section}) is not in ${APPROVED_JSON}; ${COMPONENT_POLICY}`,
      );
    }
  }
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
// blanked, and `skeleton`, with the contents of string, template and regular-expression literals
// blanked as well. Template `${…}` expressions stay code, nested templates included.
//
// Limits of this lexer, which has no TypeScript parser behind it:
// - A `/` is read as a regular expression after an operator, an opening bracket, a separator or a
//   keyword such as `return`, and as division otherwise, so a regular expression right after `)`
//   (`if (x) /re/`) is read as code. A regular expression ends at a line break at the latest.
// - JSX text is read as code. An apostrophe in it opens a quoted string, so quoted strings end at
//   an unescaped line break (where a real one would be a syntax error), and `//` in it blanks to
//   the end of its line: both effects end with the line. A `/*` in JSX text blanks to the next
//   `*/`, or to the end of the file, and would hide any declaration in between.
function maskSource(text) {
  const n = text.length;
  const code = text.split("");
  const skeleton = text.split("");
  const blank = (chars, at) => {
    if (chars[at] !== "\n" && chars[at] !== "\r") chars[at] = " ";
  };
  const blankBoth = (at) => {
    blank(code, at);
    blank(skeleton, at);
  };
  const lineBreak = (at) => text[at] === "\n" || text[at] === "\r";
  // One brace depth per open `${` expression, innermost last.
  const expressions = [];
  let inTemplate = false;
  let last = -1;
  let i = 0;

  const regexAllowed = () => {
    if (last < 0) return true;
    const previous = text[last];
    if (/[(,=:[!&|?{};+\-*%<>~^]/.test(previous)) return true;
    if (!/[\w$]/.test(previous)) return false;
    let start = last;
    while (start > 0 && /[\w$]/.test(text[start - 1])) start -= 1;
    return REGEX_AFTER_KEYWORD.has(text.slice(start, last + 1));
  };

  while (i < n) {
    const c = text[i];
    if (inTemplate) {
      if (c === "\\") {
        blank(skeleton, i);
        if (i + 1 < n) blank(skeleton, i + 1);
        i += 2;
      } else if (c === "`") {
        inTemplate = false;
        last = i;
        i += 1;
      } else if (c === "$" && text[i + 1] === "{") {
        expressions.push(0);
        inTemplate = false;
        last = i + 1;
        i += 2;
      } else {
        blank(skeleton, i);
        i += 1;
      }
      continue;
    }
    const next = text[i + 1];
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
      inTemplate = true;
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
    } else {
      if (expressions.length > 0 && c === "{") {
        expressions[expressions.length - 1] += 1;
      } else if (expressions.length > 0 && c === "}") {
        if (expressions[expressions.length - 1] === 0) {
          expressions.pop();
          inTemplate = true;
          i += 1;
          continue;
        }
        expressions[expressions.length - 1] -= 1;
      }
      if (!/\s/.test(c)) last = i;
      i += 1;
    }
  }
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

// Local names of the generated roots, mapped to the root they stand for: `components` and `paths`
// themselves, every `as` rename imported from a module whose path ends in `schema`, and
// `<namespace>.components` / `<namespace>.paths` for a namespace import of such a module.
function generatedRoots(code) {
  const roots = new Map([...GENERATED_ROOTS].map((name) => [name, name]));
  for (const [, clause, , source] of code.matchAll(IMPORT_DECLARATION)) {
    if (!SCHEMA_MODULE.test(source)) continue;
    const namespace = /\*\s*as\s+([A-Za-z_$][\w$]*)/.exec(clause);
    if (namespace) {
      for (const name of GENERATED_ROOTS) roots.set(`${namespace[1]}.${name}`, name);
    }
    const named = /\{([^}]*)\}/.exec(clause);
    for (const specifier of named ? named[1].split(",") : []) {
      const binding = IMPORT_SPECIFIER.exec(specifier);
      if (binding && GENERATED_ROOTS.has(binding[1])) {
        roots.set(binding[2] ?? binding[1], binding[1]);
      }
    }
  }
  return roots;
}

function rootPattern(roots) {
  const alternatives = [...roots.keys()]
    .sort((a, b) => b.length - a.length)
    .map((name) => name.split(".").map(escapeRegExp).join("\\s*\\.\\s*"));
  return new RegExp(`(${alternatives.join("|")})(?![\\w$])`, "y");
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

// The root a type alias body begins with, after one leading `|` or `&`, grouping parentheses and
// the listed utility wrappers: `{ shown, generated }`, or null when the body starts otherwise.
// A root followed, past the parentheses that close around it, by `[` aliases a generated type; a
// root that ends its statement aliases the root itself.
function aliasedRoot(code, start, roots, pattern) {
  let i = skipSpace(code, start);
  if (code[i] === "|" || code[i] === "&") i = skipSpace(code, i + 1);
  let groups = 0;
  for (;;) {
    if (code[i] === "(") {
      groups += 1;
      i = skipSpace(code, i + 1);
      continue;
    }
    WRAPPER.lastIndex = i;
    const wrapper = WRAPPER.exec(code);
    if (!wrapper) break;
    i = skipSpace(code, i + wrapper[0].length);
  }
  let shown;
  let generated;
  let end;
  INLINE_IMPORT_ROOT.lastIndex = i;
  const inline = INLINE_IMPORT_ROOT.exec(code);
  if (inline && SCHEMA_MODULE.test(inline[2])) {
    shown = `import(…).${inline[3]}`;
    generated = inline[3];
    end = i + inline[0].length;
  } else {
    pattern.lastIndex = i;
    const match = pattern.exec(code);
    if (!match) return null;
    shown = match[1].replace(/\s+/g, "");
    generated = roots.get(shown);
    end = i + match[0].length;
  }
  let after = skipSpace(code, end);
  while (groups > 0 && code[after] === ")") {
    end = after + 1;
    groups -= 1;
    after = skipSpace(code, end);
  }
  if (code[after] === "[") return { shown: `${shown}[…]`, generated: `${generated}[…]` };
  STATEMENT_END.lastIndex = end;
  return STATEMENT_END.test(code) ? { shown, generated } : null;
}

// Generated API Types Only, for one source file.
function scanSource(file, text, names) {
  const { code, skeleton } = maskSource(text);
  const lineAt = lineLocator(text);
  const roots = generatedRoots(code);
  const pattern = rootPattern(roots);
  for (const match of skeleton.matchAll(DECLARATION)) {
    const [whole, keyword, name] = match;
    const line = lineAt(match.index);
    const bodyStart = keyword === "type" ? aliasBodyStart(skeleton, match.index + whole.length) : 0;
    if (bodyStart < 0) continue;
    if (names.has(name)) {
      const generated = `components["schemas"]["${name}"]`;
      const message = `${keyword} ${name} restates generated schema ${generated}`;
      report(file, line, `${message}; use the generated type`);
    }
    if (keyword !== "type") continue;
    const aliased = aliasedRoot(code, bodyStart, roots, pattern);
    if (aliased === null) continue;
    const via = aliased.shown === aliased.generated ? "" : `, the generated ${aliased.generated}`;
    const message = `type ${name} aliases ${aliased.shown}${via}`;
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

try {
  checkDependencies();
  checkGeneratedTypes();
} catch (error) {
  report(SELF, 1, `internal error: ${printable(error?.message ?? error)}`);
}
for (const { file, line, message } of findings) {
  process.stderr.write(`${file}:${line}: ${message}\n`);
}
// exitCode rather than process.exit(), so stderr is flushed before the process ends.
process.exitCode = findings.length > 0 ? 1 : 0;
