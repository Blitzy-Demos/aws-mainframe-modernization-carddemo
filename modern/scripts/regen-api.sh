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
#   3. git compares both files with HEAD. An untracked file, a staged but uncommitted change and
#      a working-tree change all fail, and git's diff shows what the handlers now generate.
# Regeneration comes before the comparison on purpose: after a handler or DTO change, one run
# leaves the new contract in the working tree to review and commit, and the next run passes.
#
# It writes only openapi/openapi.json, frontend/src/api/schema.d.ts and target/regen-api/, plus
# frontend/node_modules/ when that is absent, installed from the committed lockfile. It needs
# cargo, pnpm and git, and no database, broker or network beyond the package registry the
# lockfiles pin. Its output is progress lines and git's diff of the two generated files, whose
# examples carry only redacted values.
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
modern/frontend/src/api/schema.d.ts with openapi-typescript, then compares both with HEAD.
It changes to its own modern/ directory first and exports modern/.env when present. The api
crate is built against the committed .sqlx/ (SQLX_OFFLINE=true) unless SQLX_OFFLINE is already
set. frontend/node_modules is installed with pnpm install --frozen-lockfile when absent.

Exit status:
  0  both regenerated files equal their committed versions
  1  a file is untracked or differs from HEAD (the regenerated file stays in the working tree to
     commit), carddemo-openapi failed (openapi/openapi.json is left unchanged), or git failed
  2  usage error: an unexpected argument
  Any other non-zero status is that of the cargo, pnpm or openapi-typescript command that failed.
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

# check.sh runs steps 0-12 without Docker, so the api crate builds against the committed .sqlx/
# by default; the document is built from the handlers and depends on no database. A caller or
# .env that sets SQLX_OFFLINE keeps its value.
export SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

echo 'regen-api: emitting openapi/openapi.json (carddemo-openapi)'
mkdir -p target/regen-api openapi
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
# Against HEAD rather than the index, so a staged but uncommitted change fails as well.
rc=0
git --no-pager -C "$REPO" diff --no-ext-diff --exit-code HEAD -- "${GENERATED[@]}" || rc=$?
case $rc in
  0) ;;
  1) echo 'regen-api: generated contract differs from the committed files;' \
       'commit the regenerated output' >&2
     exit 1 ;;
  *) printf 'regen-api: git diff failed with status %s\n' "$rc" >&2
     exit 1 ;;
esac

echo 'regen-api: openapi.json and schema.d.ts match the committed files'
