#!/usr/bin/env bash
# Contract regeneration: step 11 of scripts/check.sh, bindings o7 and Generated API Types Only.
#
# Regenerates the two generated contract files from the API handlers, then fails unless both
# equal their committed versions:
#   1. carddemo-openapi (crates/api/src/bin/carddemo-openapi.rs) prints the OpenAPI document of
#      the 27 operations. Its output replaces openapi/openapi.json only when the binary succeeds,
#      so a failed build or run leaves the committed file as it was;
#   2. openapi-typescript, at the version frontend/pnpm-lock.yaml pins, turns that document into
#      frontend/src/api/schema.d.ts, kept exactly as generated: the file is never edited by hand;
#   3. git compares both files twice: the working tree, which now holds the regenerated files,
#      with HEAD, and the index with HEAD. An untracked file, a regenerated file that differs
#      from HEAD, staged or not, and a staged change HEAD does not hold all fail; git's diffs
#      show what the handlers now generate and what the index holds.
# Regeneration comes before the comparison on purpose: after a handler or DTO change, one run
# leaves the new contract in the working tree to review and commit, and the next run passes.
#
# It writes only openapi/openapi.json, frontend/src/api/schema.d.ts and target/regen-api/, plus
# frontend/node_modules/ when that is absent, installed from the committed lockfile. It needs
# cargo, pnpm, git and flock (util-linux), and no database, broker or network beyond the package
# registry the lockfiles pin. Runs in one checkout are serialised: each holds an exclusive flock
# on target/regen-api/lock from before generation until it exits, so a second run waits for the
# first. Its output is progress lines and git's diffs of the two generated files, whose examples
# carry only redacted values.
#
# Usage: scripts/regen-api.sh, from any working directory, with no arguments (-h or --help prints
# the usage). Configuration comes from modern/.env when present, exported before any command runs.
set -euo pipefail

# modern/: the cargo workspace root, and the base of every relative path below.
cd "$(dirname "${BASH_SOURCE[0]}")/.."
set -a; [ -f .env ] && . ./.env; set +a

usage() {
  cat <<'EOF'
Usage: scripts/regen-api.sh [-h | --help]

Regenerates modern/openapi/openapi.json with carddemo-openapi and
modern/frontend/src/api/schema.d.ts with openapi-typescript, then compares both files in the
working tree and in the index with HEAD. It changes to its own modern/ directory first and
exports modern/.env when present. The api crate is always built against the committed .sqlx/
(SQLX_OFFLINE=true, whatever the caller or modern/.env sets), so no database is needed.
frontend/node_modules is installed with pnpm install --frozen-lockfile when absent. Concurrent
runs in one checkout wait for each other: each holds an exclusive flock on
target/regen-api/lock from before generation until it exits.

Exit status:
  0  both regenerated files equal their committed versions
  1  a file is untracked or differs from HEAD (the regenerated file stays in the working tree to
     commit), the index holds a change to a file that HEAD does not (stage the regenerated file
     or unstage that change), carddemo-openapi failed (openapi/openapi.json is left unchanged),
     or git failed
  2  usage error: an unexpected argument
  Any other non-zero status is that of the flock, cargo, pnpm or openapi-typescript command that
  failed, or 127 when one of them is not installed.
EOF
}

if [ $# -eq 1 ] && { [ "$1" = -h ] || [ "$1" = --help ]; }; then
  usage
  exit 0
fi
if [ $# -gt 0 ]; then
  echo 'regen-api: no arguments are taken other than a single -h or --help' >&2
  usage >&2
  exit 2
fi

REPO=$(git rev-parse --show-toplevel)
# Repository-root paths of the generated files, because every git command runs with -C "$REPO".
GENERATED=(modern/openapi/openapi.json modern/frontend/src/api/schema.d.ts)

# check.sh runs steps 0-12 without Docker, so the api crate always builds against the committed
# .sqlx/; the document is built from the handlers and depends on no database. Set after .env is
# exported, so neither a caller nor .env can turn it off. Checking that cache against the schema
# migrated from crates/persistence/migrations is step 14 of check.sh, not this script.
export SQLX_OFFLINE=true

# One run at a time per checkout. From here until the script exits, this run holds an exclusive
# flock on target/regen-api/lock through file descriptor 9, so a concurrent run waits instead of
# truncating or moving the temporary document below, or comparing a schema.d.ts that this run is
# still writing. The lock is released when the script exits.
command -v flock > /dev/null || {
  echo 'regen-api: flock is not installed; it comes with util-linux (on macOS: Homebrew flock)' >&2
  exit 127
}
mkdir -p target/regen-api openapi
exec 9> target/regen-api/lock
if ! flock -n 9; then
  echo 'regen-api: waiting for another regen-api run in this checkout'
  flock 9
fi

echo 'regen-api: emitting openapi/openapi.json (carddemo-openapi)'
if ! cargo run -q --locked -p api --bin carddemo-openapi > target/regen-api/openapi.json; then
  echo 'regen-api: carddemo-openapi failed; openapi/openapi.json is left unchanged' >&2
  exit 1
fi
mv target/regen-api/openapi.json openapi/openapi.json

if [ ! -d frontend/node_modules ]; then
  echo 'regen-api: installing frontend dependencies (pnpm install --frozen-lockfile)'
  pnpm --dir frontend install --frozen-lockfile
fi

# pnpm runs the command in frontend/, so both paths resolve from there.
echo 'regen-api: generating frontend/src/api/schema.d.ts (openapi-typescript)'
pnpm --dir frontend exec openapi-typescript ../openapi/openapi.json -o src/api/schema.d.ts

echo 'regen-api: comparing openapi.json and schema.d.ts with HEAD'
git -C "$REPO" ls-files --error-unmatch -- "${GENERATED[@]}" > /dev/null || {
  echo 'regen-api: openapi.json or schema.d.ts is not tracked; commit the regenerated files' >&2
  exit 1
}
# Two comparisons, both always run so both diffs are shown: the working tree, which now holds the
# regenerated files, against HEAD; then the index against HEAD, because regeneration rewrites only
# the working tree and a staged change would otherwise pass here and reach the next commit.
rc=0
git --no-pager -C "$REPO" diff --no-ext-diff --exit-code HEAD -- "${GENERATED[@]}" || rc=$?
cached_rc=0
git --no-pager -C "$REPO" diff --no-ext-diff --cached --exit-code HEAD -- "${GENERATED[@]}" \
  || cached_rc=$?
for status in "$rc" "$cached_rc"; do
  case $status in
    0 | 1) ;;
    *) printf 'regen-api: git diff failed with status %s\n' "$status" >&2
       exit 1 ;;
  esac
done
if [ "$rc" -eq 1 ]; then
  echo 'regen-api: generated contract differs from the committed files;' \
    'commit the regenerated output' >&2
fi
if [ "$cached_rc" -eq 1 ]; then
  echo 'regen-api: the index holds a change to the generated files that HEAD does not;' \
    'stage the regenerated files (git add) or unstage that change' >&2
fi
if [ "$rc" -eq 1 ] || [ "$cached_rc" -eq 1 ]; then
  exit 1
fi

echo 'regen-api: openapi.json and schema.d.ts match the committed files'
