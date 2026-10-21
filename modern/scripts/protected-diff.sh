#!/usr/bin/env bash
# Protected-path and Authority diff: step 12 of scripts/check.sh, binding R16.
#
# Fails unless, against the starting commit 59cc6c2 (used by its full object name, because git
# would resolve the abbreviated name to a tag or branch called 59cc6c2 if one existed):
#   0. no tracked path outside modern/ carries the assume-unchanged or skip-worktree index flag,
#      either of which makes git skip that path's working-tree file in checks 1-3;
#   1. every tracked file under the protected COBOL paths is unchanged in the working tree
#      (no edit, move, delete, mode change or reformat);
#   2. no path outside modern/ differs, whether the change is committed or not;
#   3. no path outside modern/ is modified, staged or untracked;
#   4. every baseline file outside modern/ is a regular file whose raw bytes and user-execute bit
#      equal the baseline's. git compares the working tree only after clean filters and
#      end-of-line conversion and trusts the index's cached state, so .git/info/attributes or
#      core.autocrlf can hide a byte change from checks 1-3; check 4 reads the raw bytes and
#      mode, as the parity oracle reads the files in place.
# Check 3 does not report paths ignored by .gitignore or .git/info/exclude.
# The baseline check and checks 0-4 run git with --no-replace-objects, core.fileMode=true and
# core.fsmonitor=false, so no replace ref (which can substitute another commit for the baseline),
# local core.fileMode=false (which hides mode changes) or filesystem-monitor hook (which can
# report an edited file as unchanged) can make them pass. On a filesystem without executable
# bits, they therefore report mode changes that git would otherwise ignore.
# Only the local clone is read: nothing is fetched and no file is written. git diff may still
# refresh the index's stat cache, as it does on every run; disabling that (diff.autoRefreshIndex)
# would make check 2 report files whose timestamps changed but whose content did not.
#
# Usage, from modern/: scripts/protected-diff.sh (no arguments). --help prints usage() below,
# which also lists the exit codes: 0 pass, 1 a check or a git or stat command failed, 2 usage error.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
set -a; [ -f .env ] && . ./.env; set +a

# The full object name of 59cc6c2: git resolves an abbreviated name to a same-named tag or branch
# when one exists, but a full name always to the object itself.
BASELINE=59cc6c2fd7ebd7ef7925cad552a01a4b8b6e4d5e
# Pathspecs are relative to the repository root, because every git command runs with -C "$REPO".
PROTECTED=(app/cbl app/cpy app/bms app/jcl app/proc app/asm app/csd app/ctl
  app/maclib app/scheduler app/data/ASCII app/data/EBCDIC)

usage() {
  cat <<EOF
Usage: scripts/protected-diff.sh [-h | --help]

Protected-path and Authority diff (R16), step 12 of scripts/check.sh. Run it from modern/ with
no arguments; it changes to its own modern/ directory first, so any working directory works.
It reads the local clone only: nothing is fetched and no tracked or staged content changes.
Replace refs are ignored and a local core.fileMode=false or core.fsmonitor setting is
overridden, so none of them can hide a change.

Against the baseline commit 59cc6c2, named in full as $BASELINE
so that a tag or branch called 59cc6c2 cannot stand in for it, it checks, in this order,
stopping at the first failure:
  - the baseline commit is present in the local clone;
  - no tracked path outside modern/ is flagged assume-unchanged or skip-worktree in the index
    (git skips the working-tree file of a flagged path, so its edits would go unseen);
  - every tracked file under the protected COBOL paths is unchanged, file mode included:
      ${PROTECTED[*]}
  - no path outside modern/ differs from the baseline, whether the change is committed or not;
  - no path outside modern/ is modified, staged or untracked (ignored paths are not reported);
  - every baseline file outside modern/ is a regular file whose raw bytes and user-execute bit
    equal the baseline's (git compares only after clean filters and end-of-line conversion and
    trusts the index, so .git/info/attributes or core.autocrlf could hide a change; this reads
    the bytes and mode the parity oracle reads in place).

Options:
  -h, --help  Print this help and exit. Any other argument is a usage error.

Exit status:
  0  every check passed
  1  a check failed, the baseline commit is missing, or a git or stat command failed
  2  usage error: an unexpected argument
  Any other non-zero status comes from bash or git, e.g. 128 when run outside a git repository.
EOF
}

# No argument runs the checks (scripts/check.sh calls it that way); -h or --help alone prints
# the usage; anything else is reported on stderr with the usage, before any git command runs.
if [ $# -gt 0 ]; then
  case $1 in
    -h|--help) [ $# -gt 1 ] || { usage; exit 0; }; bad=$2 ;;
    *) bad=$1 ;;
  esac
  printf 'UNEXPECTED ARGUMENT: %q\n' "$bad" >&2
  usage >&2
  exit 2
fi

REPO=$(git rev-parse --show-toplevel)
# For every git command that reads objects, the index or the working tree: a replace ref can
# substitute another commit for the baseline, a local core.fileMode=false hides mode changes,
# and a core.fsmonitor hook can report an edited file as unchanged.
OVERRIDES=(--no-replace-objects -c core.fileMode=true -c core.fsmonitor=false)

no_output() { local out; out=$("$@") || { printf 'COMMAND FAILED: %s\n' "$*"; exit 1; }
  [ -z "$out" ] || { printf 'UNEXPECTED OUTPUT: %s\n%s\n' "$*" "$out"; exit 1; }; }

echo "protected-diff: baseline commit $BASELINE in the local clone"
git "${OVERRIDES[@]}" -C "$REPO" cat-file -e "$BASELINE^{commit}" || {
  printf 'BASELINE MISSING: commit %s is not in the clone at %s\n' "$BASELINE" "$REPO"
  echo 'A shallow clone must fetch it first; the checks cannot run without it.'
  exit 1
}

# ls-files -v tags an assume-unchanged entry in lowercase and a skip-worktree entry S. git skips
# the working-tree file of either, so the checks below would pass over its edits.
echo "protected-diff: assume-unchanged or skip-worktree index flags outside modern/"
entries=$(git "${OVERRIDES[@]}" -C "$REPO" ls-files -v -- . ':!modern') || {
  printf 'COMMAND FAILED: git %s -C %s ls-files -v -- . :!modern\n' "${OVERRIDES[*]}" "$REPO"
  exit 1
}
flagged=$(printf '%s\n' "$entries" | LC_ALL=C awk '/^[a-zS] /') || {
  echo 'COMMAND FAILED: awk filter of git ls-files -v'
  exit 1
}
[ -z "$flagged" ] || {
  echo 'INDEX FLAGS HIDE CHANGES: assume-unchanged (lowercase tag) or skip-worktree (S) entries'
  printf '%s\n' "$flagged"
  echo 'Clear them (git update-index --no-assume-unchanged --no-skip-worktree -- <path>, or'
  echo 'git sparse-checkout disable for a sparse checkout), then run again.'
  exit 1
}

echo "protected-diff: protected COBOL paths vs $BASELINE"
rc=0
git --no-pager "${OVERRIDES[@]}" -C "$REPO" diff --exit-code "$BASELINE" -- "${PROTECTED[@]}" || rc=$?
case $rc in
  0) ;;
  1) printf 'PROTECTED PATHS CHANGED since %s (diff above)\n' "$BASELINE"; exit 1 ;;
  *) printf 'COMMAND FAILED (%s): git diff --exit-code %s -- %s\n' \
       "$rc" "$BASELINE" "${PROTECTED[*]}"
     exit 1 ;;
esac

echo "protected-diff: committed or uncommitted changes outside modern/ vs $BASELINE"
no_output git "${OVERRIDES[@]}" -C "$REPO" diff --name-only "$BASELINE" -- . ':!modern'

# --untracked-files=normal: a local status.showUntrackedFiles=no would otherwise hide new paths.
echo "protected-diff: modified, staged or untracked paths outside modern/"
no_output git "${OVERRIDES[@]}" -C "$REPO" status --porcelain --untracked-files=normal -- . ':!modern'

# git diff and status see a working-tree file only after clean filters and end-of-line conversion
# (from .git/info/attributes or core.autocrlf, say) and trust the index's cached state, so a file
# whose bytes differ can still pass the checks above. hash-object --no-filters hashes the raw bytes
# and stat reads the user-execute bit, as the parity oracle reads the files in place. lastpipe runs
# each mapfile in this shell, and pipefail gives its pipeline the status of a failing git or stat.
echo "protected-diff: working-tree bytes and modes of every baseline file, without git's filters"
shopt -s lastpipe
git "${OVERRIDES[@]}" -C "$REPO" ls-tree -r -z "$BASELINE" | mapfile -d '' -t tree || {
  printf 'COMMAND FAILED: git %s -C %s ls-tree -r -z %s\n' "${OVERRIDES[*]}" "$REPO" "$BASELINE"
  exit 1
}
# Each entry is "<mode> <type> <object>\t<path>". The baseline holds only regular-file blobs.
paths=() modes=() oids=() present=()
for entry in "${tree[@]}"; do
  path=${entry#*$'\t'} meta=${entry%%$'\t'*}
  case $path in modern|modern/*) continue ;; esac
  case $meta in
    '100644 blob '*|'100755 blob '*) ;;
    *) printf 'UNSUPPORTED BASELINE ENTRY: %s %s (only 100644 and 100755 blobs are checked)\n' \
         "$meta" "$path"
       exit 1 ;;
  esac
  paths+=("$path") modes+=("${meta%% *}") oids+=("${meta##* }")
  if [ ! -L "$REPO/$path" ] && [ -f "$REPO/$path" ]; then present+=("$path"); fi
done
perms=() hashes=()
if [ ${#present[@]} -gt 0 ]; then
  (cd "$REPO" && stat -c %A -- "${present[@]}") | mapfile -t perms || {
    printf 'COMMAND FAILED: stat -c %%A of %s baseline files in %s\n' "${#present[@]}" "$REPO"
    exit 1
  }
  git "${OVERRIDES[@]}" -C "$REPO" hash-object --no-filters -- "${present[@]}" \
    | mapfile -t hashes || {
    printf 'COMMAND FAILED: git %s -C %s hash-object --no-filters of %s baseline files\n' \
      "${OVERRIDES[*]}" "$REPO" "${#present[@]}"
    exit 1
  }
fi
if [ ${#perms[@]} -ne ${#present[@]} ] || [ ${#hashes[@]} -ne ${#present[@]} ]; then
  printf 'COMMAND FAILED: %s files, but %s modes from stat and %s hashes from git hash-object\n' \
    "${#present[@]}" "${#perms[@]}" "${#hashes[@]}"
  exit 1
fi
# The fourth character of stat's %A is the user-execute bit: x or s when set, - or S when clear.
differs=() j=0
for i in "${!paths[@]}"; do
  if [ "${present[j]-}" != "${paths[i]}" ]; then
    differs+=("missing or not a regular file: ${paths[i]}")
    continue
  fi
  content=false mode=false
  [ "${hashes[j]}" = "${oids[i]}" ] || content=true
  case ${modes[i]}:${perms[j]:3:1} in 100755:[xs]|100644:[-S]) ;; *) mode=true ;; esac
  modeinfo="(baseline ${modes[i]}, working tree ${perms[j]})"
  if $content && $mode; then differs+=("content and mode differ $modeinfo: ${paths[i]}")
  elif $content; then differs+=("content differs: ${paths[i]}")
  elif $mode; then differs+=("mode differs $modeinfo: ${paths[i]}")
  fi
  j=$((j + 1))
done
[ ${#differs[@]} -eq 0 ] || {
  echo "WORKING TREE DIFFERS FROM BASELINE BYTES (git's comparison did not show it):"
  printf '%s\n' "${differs[@]}"
  exit 1
}

echo "protected-diff: OK"
