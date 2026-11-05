// Stale API-type detector, run by `pnpm --dir web gen:api:check`.
//
// Regenerates the TypeScript API types from the committed openapi/openapi.json
// into a temporary file and fails unless they equal the committed
// src/api/schema.d.ts byte for byte. crates/api/tests/openapi_spec.rs proves
// that openapi.json matches the API, so the two checks together prove the chain
// crates/api -> openapi.json -> schema.d.ts is fresh.
//
// It must run the same generator with the same options as `gen:api`; any other
// option set would report the options' effect as staleness.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const webDir = fileURLToPath(new URL("..", import.meta.url));

const SPEC_REL = "openapi/openapi.json";
const COMMITTED_REL = "src/api/schema.d.ts";
// A plain file path rather than module resolution: the package's exports map
// rewrites "./*.js" to "./*.mjs", which would resolve to a bin/cli.mjs that
// does not exist.
const CLI = path.join(webDir, "node_modules", "openapi-typescript", "bin", "cli.js");

const REGENERATE_SPEC = "cargo run -p carddemo-api --bin openapi -- --out web/openapi/openapi.json";
const REGENERATE_TYPES = "pnpm --dir web gen:api";
const INSTALL = "pnpm --dir web install --frozen-lockfile";
const PREVIEW_MAX_CHARS = 200;

// Runs openapi-typescript into `outFile`; returns 0 on success, else an exit
// code.
function regenerate(outFile) {
  // Exactly the `gen:api` arguments with only the output path changed. Any
  // option later added to `gen:api` in package.json must be added here too.
  const args = [CLI, SPEC_REL, "-o", outFile];
  // The generator's output is captured instead of inherited because its two
  // progress lines would otherwise precede this script's single result line on
  // stdout. Its stderr (warnings and errors) is always relayed; its stdout only
  // when it fails.
  const result = spawnSync(process.execPath, args, {
    cwd: webDir,
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.error) {
    console.error(`Could not run openapi-typescript (${CLI}): ${result.error.message}`);
    return 1;
  }
  const wroteOutput = fs.existsSync(outFile);
  const failed = result.signal !== null || result.status !== 0 || !wroteOutput;
  if (failed && result.stdout.length > 0) {
    process.stderr.write(result.stdout);
  }
  if (result.stderr.length > 0) {
    process.stderr.write(result.stderr);
  }
  if (result.signal !== null) {
    console.error(`openapi-typescript was terminated by signal ${result.signal}.`);
    return 1;
  }
  if (result.status !== 0) {
    console.error(`openapi-typescript failed (exit ${result.status})`);
    return result.status ?? 1;
  }
  if (!wroteOutput) {
    console.error(`openapi-typescript exited 0 but wrote no output file at ${outFile}.`);
    return 1;
  }
  return 0;
}

// 1-based number of the line holding the first differing byte, or of the line
// where the shorter buffer ends when one is a prefix of the other.
// Counting bytes rather than decoded characters keeps the position exact for
// any difference, including line endings and invalid UTF-8.
function firstDifferenceLine(a, b) {
  const limit = Math.min(a.length, b.length);
  let offset = 0;
  while (offset < limit && a[offset] === b[offset]) {
    offset += 1;
  }
  let line = 1;
  for (let i = 0; i < offset; i += 1) {
    if (a[i] === 0x0a) {
      line += 1;
    }
  }
  return line;
}

// The given 1-based line of `buf` with its `\n` terminator, if it has one,
// quoted so that whitespace, `\r` and a missing final newline stay visible.
function linePreview(buf, line) {
  const lines = buf.toString("utf8").split("\n");
  const isLast = line >= lines.length;
  const text = lines[line - 1] ?? "";
  if (isLast && text === "") {
    return "(end of file)";
  }
  const shown = text.length > PREVIEW_MAX_CHARS ? `${text.slice(0, PREVIEW_MAX_CHARS)}...` : text;
  return JSON.stringify(isLast ? shown : `${shown}\n`);
}

function reportStale(detail, previewLines) {
  console.error(`API client types are stale: ${COMMITTED_REL} ${detail}.`);
  for (const previewLine of previewLines) {
    console.error(previewLine);
  }
  console.error("Regenerate both files, from the repository root, in this order:");
  console.error(`  1. ${REGENERATE_SPEC}`);
  console.error(`  2. ${REGENERATE_TYPES}`);
  console.error("Then commit both web/openapi/openapi.json and web/src/api/schema.d.ts.");
}

// Compares the regenerated types with the committed file; returns the exit
// code.
function compare(generatedPath, committedPath) {
  if (!fs.existsSync(committedPath)) {
    reportStale(`is missing (it is generated from ${SPEC_REL})`, []);
    return 1;
  }
  const generatedBuf = fs.readFileSync(generatedPath);
  const committedBuf = fs.readFileSync(committedPath);
  if (generatedBuf.equals(committedBuf)) {
    console.log(`API client types are up to date (${COMMITTED_REL} matches ${SPEC_REL}).`);
    return 0;
  }
  const line = firstDifferenceLine(committedBuf, generatedBuf);
  reportStale(`differs from what ${SPEC_REL} generates (first difference at line ${line})`, [
    `  committed:   ${linePreview(committedBuf, line)}`,
    `  regenerated: ${linePreview(generatedBuf, line)}`,
  ]);
  return 1;
}

// Returns the process exit code; never exits itself, so `finally` always runs.
function main() {
  if (!fs.existsSync(CLI)) {
    console.error(`openapi-typescript is not installed: ${CLI} does not exist.`);
    console.error(`Install the web dependencies from the repository root: ${INSTALL}`);
    return 1;
  }
  if (!fs.existsSync(path.join(webDir, SPEC_REL))) {
    console.error(`Missing ${path.join(webDir, SPEC_REL)}, the OpenAPI document the API types are generated from.`);
    console.error(`Generate it from the repository root: ${REGENERATE_SPEC}`);
    return 1;
  }

  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "carddemo-api-types-"));
  try {
    const generated = path.join(tmpDir, "schema.d.ts");
    const status = regenerate(generated);
    if (status !== 0) {
      return status;
    }
    return compare(generated, path.join(webDir, COMMITTED_REL));
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

try {
  process.exitCode = main();
} catch (err) {
  console.error("check-api-types failed unexpectedly:");
  console.error(err instanceof Error ? (err.stack ?? err.message) : err);
  process.exitCode = 1;
}
