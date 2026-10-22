#!/usr/bin/env bash
# CardDemo check pipeline: every check of the verification bindings, run in order, stopping at the
# first failure with a non-zero exit status. This script is the pipeline that every rule's "CI
# check" or "pipeline" step names; no hosted CI definition exists, so any runner invokes this.
#
# Steps, with the bindings each one proves:
#    0  no cloud credential in the environment, checked after .env is exported
#       (Local-Only Runtime, R14)
#    1  formatting (cargo fmt --check)
#    2  no lint override (a RUSTFLAGS-family or CLIPPY_CONF_DIR variable, a cargo home inside the
#       repository, or any .cargo, clippy.toml or .clippy.toml entry), then a warning-free build
#       of every target, lockfile current (g2, R15, R10)
#    3  warning-free clippy, pedantic and float_arithmetic at deny (g2, R1)
#    4  floating-point scan of financial code; crates/batch/src/pdf.rs is the one exclusion (R1)
#    5  unsafe scan (R15)
#    6  lint-suppression scans, line by line and over each lexed whole file (so a multi-line
#       cfg_attr is caught); every member opts in to the workspace edition, rust-version and
#       lints; no non-macro SQL; no integer `as` casts; one schema executor, expand-only
#       migrations; no process-local lock in the CP00 path; thin API handlers (R15, g2, R4,
#       Checked Integer Narrowing, Single Schema Executor, i10, Thin Request Handlers)
#    7  frontend: install from the lockfile, dependency allow-list and generated-types policy
#       (scripts/frontend-policy.mjs), typecheck, build, Vitest with its 70 % line floor
#       (R10, Single Component System, Generated API Types Only, g2, Coverage Floors)
#    8  domain does not depend on sqlx (R3)
#    9  both lockfiles committed, no submodules (R10)
#   10  cargo deny (licences, advisories, bans, sources) and the cloud SDK lockfile scans
#       (R10, R14)
#   11  contract regeneration: openapi/openapi.json and frontend/src/api/schema.d.ts equal the
#       committed files (scripts/regen-api.sh; o7, Generated API Types Only)
#   12  protected paths and Authority (scripts/protected-diff.sh; R16)
#   13  compose stack up, migrations applied by carddemo-migrate, nine fixture loads
#       (scripts/dev-up.sh)
#   14  query cache matches the schema migrated from crates/persistence/migrations
#       (R4, Single Schema Executor)
#   15  all tests, Docker-backed included, under coverage instrumentation
#   16  line coverage floors: 80 % over the workspace, 90 % over copybook and domain, test and
#       bench sources excluded (Coverage Floors)
#   17  NAV, the Playwright suite (scripts/e2e.sh)
#
# Steps 0-12 need no Docker and build offline against the committed .sqlx/ (SQLX_OFFLINE=true);
# step 10's advisory check downloads the RustSec database. Steps 13-17 need Docker Engine with
# Compose v2. Without them the run stops after step 12 with exit status 2 and names the bindings
# left unverified, because an unverified binding is never reported as passed.
#
# Output: step banners, the tools' own output and the matches of a failing scan. No table row is
# queried or printed (Cardholder Data Redaction).
#
# Usage: scripts/check.sh, from any working directory, with no arguments (-h or --help prints the
# usage). Configuration comes from modern/.env when present, exported before step 0. libtest reads
# RUST_TEST_THREADS, which bounds how many tests, and so how many testcontainers, step 15 runs at
# once.
#
# Exit status: 0 when steps 0-17 all passed. Otherwise the status is non-zero and the last line on
# stderr names the step: at "13-17", exit 2 means steps 0-12 passed and Docker is unavailable; at
# "usage", exit 2 means a usage error, an unexpected argument; at any other step, the status is
# that of the failing command (1 for a forbidden match, a scan error, a cloud credential or a
# lint-level override), which a tool can also make 2.
set -euo pipefail
# modern/: REPO, the .env export and every relative path below resolve against it.
cd "$(dirname "${BASH_SOURCE[0]}")/.."
CURRENT_STEP=-; CURRENT_NAME=preamble
step() { CURRENT_STEP=$1; CURRENT_NAME=$2; printf '\n=== check.sh step %s: %s ===\n' "$1" "$2"; }
trap 'rc=$?; [ "$rc" -eq 0 ] || printf "\ncheck.sh: FAILED at step %s (%s), exit %s\n" "$CURRENT_STEP" "$CURRENT_NAME" "$rc" >&2' EXIT
REPO=$(git rev-parse --show-toplevel)
no_match() { local out rc=0; out=$(grep "$@") || rc=$?
  case $rc in
    1) ;;                                                     # clean: no match
    0) printf 'FORBIDDEN MATCH: grep %s\n%s\n' "$*" "$out"; exit 1 ;;
    *) printf 'SCAN ERROR (%s): grep %s\n' "$rc" "$*"; exit 1 ;;
  esac; }
no_output() { local out; out=$("$@") || { printf 'COMMAND FAILED: %s\n' "$*"; exit 1; }
  [ -z "$out" ] || { printf 'UNEXPECTED OUTPUT: %s\n%s\n' "$*" "$out"; exit 1; }; }
set -a; [ -f .env ] && . ./.env; set +a

usage() {
  cat <<'EOF'
Usage: scripts/check.sh [-h | --help]

Runs steps 0-17 of the CardDemo check pipeline in order and stops at the first failure. It
changes to its own modern/ directory first and exports modern/.env when present. Steps 13-17
need Docker Engine with Compose v2. RUST_TEST_THREADS bounds how many tests, and so how many
testcontainers, step 15 runs at once.

Exit status:
  0  steps 0-17 all passed
  1  a forbidden match, a scan error, a cloud credential, or a lint-level override (a
     RUSTFLAGS-family or CLIPPY_CONF_DIR variable, or a cargo or clippy configuration file)
  2  steps 0-12 passed and Docker is unavailable (the last stderr line names step 13-17), or a
     usage error: an unexpected argument
  Any other non-zero status is that of the failing tool, which can also be 1 or 2. The last line
  on stderr names the failing step.
EOF
}

if [ $# -eq 1 ] && { [ "$1" = -h ] || [ "$1" = --help ]; }; then
  usage
  exit 0
fi
if [ $# -gt 0 ]; then
  # Named for the EXIT trap, so its last stderr line tells this exit 2 from the Docker one.
  CURRENT_STEP=usage; CURRENT_NAME='unexpected argument'
  echo 'check.sh: no arguments are taken other than a single -h or --help' >&2
  usage >&2
  exit 2
fi

# 0 no cloud credential in the environment, checked after .env is exported (Local-Only Runtime)
step 0 "no cloud credentials (Local-Only Runtime, R14)"
for v in AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN AWS_PROFILE AWS_ROLE_ARN \
  AWS_WEB_IDENTITY_TOKEN_FILE AWS_CONTAINER_CREDENTIALS_FULL_URI AWS_CONTAINER_CREDENTIALS_RELATIVE_URI \
  AZURE_CLIENT_ID AZURE_CLIENT_SECRET AZURE_TENANT_ID AZURE_FEDERATED_TOKEN_FILE \
  GOOGLE_APPLICATION_CREDENTIALS GOOGLE_OAUTH_ACCESS_TOKEN CLOUDSDK_AUTH_ACCESS_TOKEN; do
  [ -z "${!v:-}" ] || { echo "CLOUD CREDENTIAL SET: $v"; exit 1; }
done
# 1-3 (no Docker; offline query cache)
step 1 "formatting"
cargo fmt --all -- --check
step 2 "warning-free build, lockfile current (g2, R10)"
# Lint levels come only from [workspace.lints] and the sources' own deny attributes. A rustc flag
# variable, CLIPPY_CONF_DIR or a cargo or clippy configuration file can pass -A, --cap-lints or
# --force-warn to rustc or relax clippy, so that steps 2-3 pass with warnings. Each fails the run
# here, before any build: an exported variable with a value (its name is printed, never its value;
# an empty one counts as unset, as in step 0); a cargo home inside the repository, whose
# config.toml cargo reads (CARGO_HOME, or $HOME/.cargo when it is unset, compared by physical
# path, so a home outside the repository such as ~/.cargo passes); and any entry named .cargo,
# clippy.toml or .clippy.toml anywhere in the repository. Entries match by name, in any letter
# case and whatever their type, so a symlinked .cargo that find does not follow still fails.
exported=$(compgen -e)
while read -r v; do
  case $v in
    RUSTFLAGS | CARGO_ENCODED_RUSTFLAGS | CARGO_BUILD_RUSTFLAGS | CARGO_TARGET_*_RUSTFLAGS \
      | CARGO_PROFILE_*_RUSTFLAGS | CLIPPY_CONF_DIR)
      [ -z "${!v:-}" ] || { echo "LINT OVERRIDE SET: $v"; exit 1; } ;;
  esac
done <<< "$exported"
repo_dir=$(CDPATH='' cd -- "$REPO" && pwd -P)
# Succeeds when $1 names an existing directory whose physical path lies inside the repository.
# cd's own error is discarded because it would print the path.
in_repo_dir() { local dir; dir=$(CDPATH='' cd -- "$1" 2>/dev/null && pwd -P) || return 1
  case $dir/ in "${repo_dir%/}"/*) return 0 ;; esac
  return 1; }
if [ -n "${CARGO_HOME:-}" ]; then
  if in_repo_dir "$CARGO_HOME"; then
    echo 'LINT OVERRIDE SET: CARGO_HOME (inside the repository)'; exit 1
  fi
elif [ -n "${HOME:-}" ] && in_repo_dir "$HOME/.cargo"; then
  echo 'LINT OVERRIDE SET: HOME (its .cargo is inside the repository)'; exit 1
fi
no_output find "$REPO" \( -path "$REPO/.git" -o -path "$REPO/modern/target" \
  -o -name node_modules \) -prune -o \( -iname .cargo -o -iname clippy.toml \
  -o -iname .clippy.toml \) -print
SQLX_OFFLINE=true cargo build --workspace --all-targets --locked
step 3 "warning-free clippy (g2, R1)"
SQLX_OFFLINE=true cargo clippy --workspace --all-targets --locked
# 4 floating point (pdf.rs is the only exclusion, and only one file may carry that name)
step 4 "floating-point scan (R1)"
no_match -rnwE 'f32|f64' crates --include='*.rs' --exclude=pdf.rs
no_output find crates -name pdf.rs ! -path crates/batch/src/pdf.rs
no_match -rniwE 'real|float[0-9]*|double precision' crates/persistence/migrations
no_match -rnE 'parseFloat|toFixed|Number\(' frontend/src
# 5 unsafe
step 5 "unsafe scan (R15)"
no_match -rnw unsafe crates --include='*.rs'
# 6 suppressions, member opt-in to workspace settings and lints, non-macro SQL
step 6 "suppressions, member opt-in, SQL, casts, schema executor, CP00 lock, thin handlers \
(R15, g2, R4, Checked Integer Narrowing, Single Schema Executor, i10, Thin Request Handlers)"
no_match -rnE '#!?\[(allow|expect)\(|cfg_attr\(.*(allow|expect)\(' crates --include='*.rs'
# The scan above reads one line at a time and so misses an attribute that rustfmt wraps over
# several lines. This one lexes each whole file as rustc does and blanks every comment and
# literal, so that no comment or literal can end or hide an attribute. It reports #[allow(,
# #[expect( and #[warn( with any whitespace or comment between their tokens or as a raw
# identifier (r#allow), and a cfg_attr( that holds one of them at any depth. warn counts because
# #[warn(warnings)] turns the workspace's denied warnings back into warnings; deny and forbid only
# tighten and never match. It follows symlinks, as cargo does, and fails on a dangling one or a
# file that is not UTF-8. Any line it prints fails the step.
cat > target/check-suppressions.cjs <<'JS'
'use strict';
// node check-suppressions.cjs <dir>: prints <path>:<line>: <line> for each #[allow(, #[expect( or
// #[warn( and each cfg_attr( holding one, in every .rs file under <dir>; prints nothing if none.
const fs = require('fs');
const path = require('path');

const WS = '\\s\\u0085\\u200e\\u200f'; // Rust's whitespace includes U+0085, U+200E and U+200F
const S = `[${WS}]*`;
const LEVEL = `(?:allow|expect|warn)${S}\\(`;
const OPENER = new RegExp(`#${S}!?${S}\\[${S}(?:r#)?(?:(${LEVEL})|cfg_attr${S}\\()`, 'g');
const NESTED = new RegExp(`(?<!\\p{XID_Continue})(?:r#)?${LEVEL}`, 'u');
const TOKEN = new RegExp(`[^${WS}]`);
const ID_START = /[\p{XID_Start}_]/u;
const ID_CONTINUE = /\p{XID_Continue}/u;

// A copy of src of the same length, newlines kept, with every comment blanked and every literal
// blanked but its first character; identifiers, raw identifiers, lifetimes and labels stay. The
// token rules are rustc_lexer's, so a comment or literal can neither end nor hide an attribute.
function lex(src) {
  const parts = [];
  let last = 0;
  const blank = (a, b) => {
    parts.push(src.slice(last, a), src.slice(a, b).replace(/[^\n]/g, ' '));
    last = b;
  };
  const width = (i) => (src.codePointAt(i) > 0xffff ? 2 : 1);
  const is = (re, i) => re.test(String.fromCodePoint(src.codePointAt(i)));
  const digit = (i) => src.charCodeAt(i) >= 48 && src.charCodeAt(i) <= 57; // false past the end
  const start = (i) => {
    const c = src.charCodeAt(i);
    return ((c | 32) >= 97 && (c | 32) <= 122) || c === 95 || (c > 127 && is(ID_START, i));
  };
  const cont = (i) => start(i) || digit(i) || (src.charCodeAt(i) > 127 && is(ID_CONTINUE, i));
  const ident = (i) => {
    while (cont(i)) i += width(i);
    return i;
  };
  const quoted = (i) => { // after the opening "; only \\ and \" can hide a closing quote
    while (i < src.length) {
      const c = src[i++];
      if (c === '"') return i;
      if (c === '\\' && (src[i] === '\\' || src[i] === '"')) i++;
    }
    return i;
  };
  const raw = (i) => { // at the first # or " after the r
    const hashes = i;
    while (src[i] === '#') i++;
    if (src[i] !== '"') return i;
    const close = `"${'#'.repeat(i - hashes)}`;
    const end = src.indexOf(close, i + 1);
    return end < 0 ? src.length : end + close.length;
  };
  const single = (i) => { // after the opening '
    if (src[i + width(i)] === "'" && src[i] !== '\\') return i + width(i) + 1;
    while (i < src.length) {
      const c = src[i];
      if (c === "'") return i + 1;
      if (c === '/' || (c === '\n' && src[i + 1] !== "'")) return i;
      i += c === '\\' ? 1 + width(i + 1) : width(i);
    }
    return i;
  };
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    const n = src[i + 1];
    const bc = c === 'b' || c === 'c';
    let end;
    if (c === '/' && n === '/') {
      end = src.indexOf('\n', i);
      blank(i, (end = end < 0 ? src.length : end));
    } else if (c === '/' && n === '*') {
      let depth = 1;
      for (end = i + 2; end < src.length && depth > 0; end++) {
        if (src[end] === '/' && src[end + 1] === '*') depth++, end++;
        else if (src[end] === '*' && src[end + 1] === '/') depth--, end++;
      }
      blank(i, end);
    } else if (c === '"') {
      blank(i + 1, (end = quoted(i + 1)));
    } else if (c === "'") { // a char literal, or a lifetime or label, which stays
      const f = i + 1;
      if (src[f] === 'r' && src[f + 1] === '#' && start(f + 2)) end = ident(f + 2); // 'r#name
      else if (src[f + width(f)] === "'" || !(start(f) || digit(f))) blank(f, (end = single(f)));
      else if (src[(end = ident(f + width(f)))] === "'") blank(f, ++end);
    } else if (digit(i)) {
      end = ident(i); // a number and its suffix
    } else if (c === 'r' && n === '#' && start(i + 2)) {
      end = ident(i + 2); // a raw identifier
    } else if (c === 'r' && (n === '#' || n === '"')) {
      blank(i + 1, (end = raw(i + 1)));
    } else if (bc && n === '"') {
      blank(i + 1, (end = quoted(i + 2)));
    } else if (c === 'b' && n === "'") {
      blank(i + 1, (end = single(i + 2)));
    } else if (bc && n === 'r' && (src[i + 2] === '#' || src[i + 2] === '"')) {
      blank(i + 1, (end = raw(i + 2)));
    } else if (start(i)) {
      end = ident(i); // whole, so a string prefix is only ever read at an identifier's start
    } else {
      end = i + width(i);
    }
    i = end;
  }
  parts.push(src.slice(last));
  return parts.join('');
}

// rustc drops a first line that starts with #! unless its next token is [, as in #![attr].
function lexFile(src) {
  const code = lex(src);
  if (!src.startsWith('#!')) return code;
  const next = code.slice(2).search(TOKEN);
  if (next >= 0 && code[2 + next] === '[') return code;
  const eol = src.indexOf('\n');
  return eol < 0 ? ' '.repeat(src.length) : ' '.repeat(eol) + lex(src.slice(eol));
}

function scan(file, out) {
  const src = new TextDecoder('utf-8', { fatal: true }).decode(fs.readFileSync(file));
  const code = lexFile(src);
  const lines = src.split('\n');
  for (const m of code.matchAll(OPENER)) {
    if (m[1] === undefined) { // cfg_attr: its balanced body, or the rest of the file
      const body = m.index + m[0].length;
      let end = body;
      for (let depth = 1; end < code.length && depth > 0; end++) {
        if (code[end] === '(') depth++;
        else if (code[end] === ')') depth--;
      }
      if (!NESTED.test(code.slice(body, end))) continue;
    }
    const line = code.slice(0, m.index).split('\n').length;
    out.push(`${file}:${line}: ${lines[line - 1].trim()}`);
  }
}

// Follows symlinks, as cargo does; a dangling one throws. A directory seen before is skipped.
function walk(dir, seen, files) {
  const real = fs.realpathSync(dir);
  if (seen.has(real)) return;
  seen.add(real);
  for (const name of fs.readdirSync(dir).sort()) {
    const entry = path.join(dir, name);
    const stat = fs.statSync(entry);
    if (stat.isDirectory()) walk(entry, seen, files);
    else if (name.endsWith('.rs') && stat.isFile()) files.push(entry);
    else if (name.endsWith('.rs')) throw new Error(`${entry}: not a regular file`);
  }
}

try {
  if (process.argv.length !== 3) throw new Error('usage: node check-suppressions.cjs <dir>');
  const files = [];
  walk(process.argv[2], new Set(), files);
  const out = [];
  for (const file of files) scan(file, out);
  if (out.length > 0) process.stdout.write(`${out.join('\n')}\n`);
} catch (err) {
  process.stderr.write(`check-suppressions: ${err.message}\n`);
  process.exit(1);
}
JS
no_output node target/check-suppressions.cjs crates
no_match -nE '= *"allow"|level *= *"allow"' Cargo.toml crates/*/Cargo.toml
no_match -rnE '@ts-(ignore|expect-error|nocheck)|eslint-disable' frontend/src frontend/e2e
test "$(ls crates/*/Cargo.toml | wc -l)" -eq 8
for m in crates/*/Cargo.toml; do
  grep -qE '^edition\.workspace *= *true' "$m" && grep -qE '^rust-version\.workspace *= *true' "$m" \
    || { echo "MISSING workspace edition/rust-version: $m"; exit 1; }
  awk '/^\[lints\]/{f=1;next} /^\[/{f=0} f && /^workspace *= *true/{ok=1} END{exit !ok}' "$m" \
    || { echo "MISSING [lints] workspace = true: $m"; exit 1; }
done
no_match -rnE 'sqlx::(query|query_as|query_scalar|raw_sql)\(' crates --include='*.rs' --exclude=explain.rs
no_output find crates -name explain.rs ! -path crates/persistence/tests/explain.rs
# 6 integer casts; one schema executor; no process-local lock in CP00; thin handlers
no_match -rnE '\bas +(u|i)(8|16|32|64|128|size)\b' crates --include='*.rs'
no_match -rn 'migrate!' crates --include='*.rs' --exclude=migrate.rs
no_output find crates -name migrate.rs ! -path crates/runtime/src/migrate.rs
no_output sh -c "grep -rl 'migrate::apply' crates --include='*.rs' \
  | grep -vE '^crates/(runtime/src/bin/carddemo-migrate|test-support/src/postgres)\.rs$' || true"
no_match -rniE '\bdrop\b|\brename\b|alter +column.*\btype\b' crates/persistence/migrations
no_match -rnwE 'Mutex|RwLock|Semaphore' crates/messaging/src crates/domain/src/messaging
no_match -rnE 'persistence::|sqlx|"(SELECT|INSERT|UPDATE|DELETE) ' crates/api/src/handlers
# 7 frontend
step 7 "frontend install, policy, types, build, tests with 70 % coverage floor \
(R10, Single Component System, Generated API Types Only, g2, Coverage Floors)"
pnpm --dir frontend install --frozen-lockfile
node scripts/frontend-policy.mjs
# One command per line rather than `typecheck && build && test`: set -e ignores a failure of any
# command of an && list but the last, so a type or build error would not stop the run there.
pnpm --dir frontend typecheck
pnpm --dir frontend build
pnpm --dir frontend test
# 8 domain without sqlx
step 8 "domain without sqlx (R3)"
cargo tree -p domain -e normal,build,dev --prefix none > target/domain-tree.txt
no_match -E '^sqlx' target/domain-tree.txt
no_match -n sqlx crates/domain/Cargo.toml
# 9 lockfiles, submodules
step 9 "lockfiles committed, no submodules (R10)"
git -C "$REPO" ls-files --error-unmatch modern/Cargo.lock modern/frontend/pnpm-lock.yaml > /dev/null
no_output git -C "$REPO" submodule status
test ! -e "$REPO/.gitmodules"
# 10 dependency policy (R10) and cloud SDK scan (R14); 11 contract regeneration
# The pattern names SDK crates only, so crypto crates such as aws-lc-rs do not match
step 10 "cargo deny and cloud SDK scan (R10, R14)"
cargo deny check
no_match -nE '^name = "(aws-sdk-|aws-config|aws-smithy-|aws-types|aws-credential-types|azure_|google-cloud-|gcp_auth)' Cargo.lock
no_match -nE '@aws-sdk/|@smithy/|@azure/|@google-cloud/|firebase' frontend/pnpm-lock.yaml
step 11 "contract regeneration (o7, Generated API Types Only)"
scripts/regen-api.sh
# 12 protected paths and Authority (scripts/protected-diff.sh)
step 12 "protected paths and Authority (R16)"
# The single copy of this step's three git checks (protected COBOL paths unchanged since 59cc6c2,
# no change outside modern/ committed or not, no modified or untracked path outside modern/).
scripts/protected-diff.sh
# 13-17 (Docker)
# Without Docker Engine and Compose v2 the run cannot pass: it stops here with exit status 2 and
# names what stayed unverified, never reporting those bindings as passed.
if ! { docker info >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; }; then
  CURRENT_STEP=13-17; CURRENT_NAME='Docker unavailable, not run'
  {
    echo 'check.sh: steps 0-12 passed; steps 13-17 UNVERIFIED (Docker unavailable)'
    echo 'Unverified bindings: d9, i9/g3b, g1a, g1b, g3a, i10, i11, R8, n5, NAV, Statements,'
    echo '  the step 14 query-cache check, Cardholder Data Redaction, Bounded Pagination,'
    echo '  the 80 % workspace floor of Coverage Floors, the query-set part of Single Schema Executor'
  } >&2
  exit 2
fi
step 13 "compose stack, migrations, fixture loads"
scripts/dev-up.sh
step 14 "query cache matches migrated schema (R4, Single Schema Executor)"
cargo sqlx prepare --workspace --check -- --all-targets
step 15 "all tests under coverage instrumentation"
cargo llvm-cov --workspace --locked --no-report
step 16 "coverage floors 80 % workspace / 90 % copybook+domain (Coverage Floors)"
cargo llvm-cov report --fail-under-lines 80 --ignore-filename-regex '/(tests|benches)/'
cargo llvm-cov report --fail-under-lines 90 \
  --ignore-filename-regex '/(tests|benches)/|crates/(persistence|runtime|api|messaging|batch|test-support)/'
step 17 "NAV (Playwright)"
scripts/e2e.sh
echo 'check.sh: all steps 0-17 passed'
exit 0
