#!/usr/bin/env bash
# Protected-path and Authority diff: step 12 of scripts/check.sh, binding R16.
#
# Fails unless, against the starting commit 59cc6c2:
#   1. every tracked file under the protected COBOL paths is unchanged in the working tree
#      (no edit, move, delete, mode change or reformat);
#   2. no path outside modern/ differs, whether the change is committed or not;
#   3. no path outside modern/ is modified, staged or untracked.
# Check 3 does not report paths ignored by .gitignore or .git/info/exclude.
# Only the local clone is read: nothing is fetched and no file is written. git diff may still
# refresh the index's stat cache, as it does on every run; disabling that (diff.autoRefreshIndex)
# would make check 2 report files whose timestamps changed but whose content did not.
#
# Usage, from modern/: scripts/protected-diff.sh
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
set -a; [ -f .env ] && . ./.env; set +a

REPO=$(git rev-parse --show-toplevel)
BASELINE=59cc6c2
# Pathspecs are relative to the repository root, because every git command runs with -C "$REPO".
PROTECTED=(app/cbl app/cpy app/bms app/jcl app/proc app/asm app/csd app/ctl
  app/maclib app/scheduler app/data/ASCII app/data/EBCDIC)

no_output() { local out; out=$("$@") || { printf 'COMMAND FAILED: %s\n' "$*"; exit 1; }
  [ -z "$out" ] || { printf 'UNEXPECTED OUTPUT: %s\n%s\n' "$*" "$out"; exit 1; }; }

echo "protected-diff: baseline commit $BASELINE in the local clone"
git -C "$REPO" cat-file -e "$BASELINE^{commit}" || {
  printf 'BASELINE MISSING: commit %s is not in the clone at %s\n' "$BASELINE" "$REPO"
  echo 'A shallow clone must fetch it first; the checks cannot run without it.'
  exit 1
}

echo "protected-diff: protected COBOL paths vs $BASELINE"
rc=0
git --no-pager -C "$REPO" diff --exit-code "$BASELINE" -- "${PROTECTED[@]}" || rc=$?
case $rc in
  0) ;;
  1) printf 'PROTECTED PATHS CHANGED since %s (diff above)\n' "$BASELINE"; exit 1 ;;
  *) printf 'COMMAND FAILED (%s): git diff --exit-code %s -- %s\n' \
       "$rc" "$BASELINE" "${PROTECTED[*]}"
     exit 1 ;;
esac

echo "protected-diff: committed or uncommitted changes outside modern/ vs $BASELINE"
no_output git -C "$REPO" diff --name-only "$BASELINE" -- . ':!modern'

# --untracked-files=normal: a local status.showUntrackedFiles=no would otherwise hide new paths.
echo "protected-diff: modified, staged or untracked paths outside modern/"
no_output git -C "$REPO" status --porcelain --untracked-files=normal -- . ':!modern'

echo "protected-diff: OK"
