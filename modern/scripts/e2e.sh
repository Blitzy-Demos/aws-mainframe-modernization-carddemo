#!/usr/bin/env bash
# NAV: prepare the navigation data, start carddemo-api and the Vite dev server, run the Playwright
# suite frontend/e2e/nav.spec.ts, then stop both and remove the local stack. Step 17 of
# scripts/check.sh, binding NAV.
#
# The nine fixture loads leave transactions and both pending-authorization tables empty, yet NAV
# selects a transaction (COTRN00 -> COTRN01) and an authorization (COPAU00 -> COPAU01). Steps,
# stopping before NAV at the first result other than the expected one:
#   1. docker compose down -v, then scripts/dev-up.sh: a fresh stack whose schema comes only from
#      carddemo-migrate (Single Schema Executor), loaded by the nine loads;
#   2. carddemo-batch posttran over ../app/data/ASCII/dailytran.txt, whose exit status and posted
#      count must equal the audited constants below;
#   3. frontend/e2e/fixtures/pending-auth-seed.sql through psql, after which each pending-
#      authorization table must hold exactly the seeded row. The seed stands in for the CP00
#      path, which NAV cannot use: halt H6 blocks every CP00 reply body, and CP00 writes those
#      tables only after its reply. It is navigation data only: it carries no parity claim, no
#      binding asserts its values, and nothing but this script loads it;
#   4. carddemo-api (which never migrates) and the Vite dev server, each waited for until it
#      answers. carddemo-mq is not started: NAV needs no broker worker;
#   5. pnpm --dir frontend e2e (playwright test), whose exit status becomes this script's.
# On every exit from step 1 on (success, failure, SIGINT or SIGTERM) both servers are stopped and
# docker compose down -v removes the containers and the database volume, so no NAV data outlives
# the run. A SIGTERM sent to this script alone takes effect once its current foreground command
# returns. NAV itself submits no write, so the data stays as prepared here.
#
# Output: progress lines and the path of the DALYREJS generation posttran wrote. The output of
# posttran and of both servers goes to var/e2e/*.log and is never echoed. The database is read
# only through SELECT count(*), so no row, card number or SSN is printed, and neither .env nor a
# secret nor a URL carrying credentials is printed (Cardholder Data Redaction).
#
# Prerequisites: Docker with Compose v2, the pinned Rust toolchain, Node.js 22, pnpm 9.15.9 and
# Playwright's Chromium (docs/SETUP.md); this script downloads no browser. It writes only under
# modern/: target/, var/, frontend/node_modules/ (installed from the lockfile when absent), and
# Playwright's frontend/test-results/ and frontend/playwright-report/.
#
# Usage: scripts/e2e.sh, from any working directory, with no arguments (-h or --help prints the
# usage). Configuration comes from modern/.env (copied from .env.example), exported before any
# command runs, so the API, Vite and Playwright all see the same CARDDEMO_API_BIND and
# CARDDEMO_WEB_PORT.
set -euo pipefail

# modern/: compose.yaml, the cargo workspace and the frontend directory resolve against it.
cd "$(dirname "${BASH_SOURCE[0]}")/.."
set -a; [ -f .env ] && . ./.env; set +a

usage() {
  cat <<'EOF'
Usage: scripts/e2e.sh [-h | --help]

Runs NAV, the Playwright suite frontend/e2e/nav.spec.ts, against freshly prepared local data:
docker compose down -v and scripts/dev-up.sh (migration and the nine loads), carddemo-batch
posttran, then the pending-authorization seed frontend/e2e/fixtures/pending-auth-seed.sql. It
then starts carddemo-api on CARDDEMO_API_BIND and the Vite dev server on
127.0.0.1:CARDDEMO_WEB_PORT (default 5173), and stops both and removes the stack
(docker compose down -v) on every exit. It changes to its own modern/ directory first and
exports modern/.env when present; CARDDEMO_JWT_SECRET must be set. The output of posttran and of
both servers goes to var/e2e/*.log.

Exit status:
  0    NAV passed
  1    CARDDEMO_JWT_SECRET is empty; posttran's exit status or a row count differs from the
       audited one; a server exited or did not answer within 120 s; or its URL already answered
       before it was started
  2    usage error: an unexpected argument
  130  interrupted (SIGINT); 143 terminated (SIGTERM)
  Any other non-zero status is that of the docker, cargo, pnpm or Playwright command that failed.
EOF
}

if [ $# -eq 1 ] && { [ "$1" = -h ] || [ "$1" = --help ]; }; then
  usage
  exit 0
fi
if [ $# -gt 0 ]; then
  echo 'e2e: no arguments are taken other than a single -h or --help' >&2
  usage >&2
  exit 2
fi

# Audited results of posttran over dailytran.txt on the freshly loaded database, copied from
# modern/audit/fixtures.md. A different value fails the run before NAV starts.
# FIXPOST-903: exit status 4, because the expected reject set FIXPOST-902 (38 records) is not
# empty; CBTRN02C moves 4 to RETURN-CODE when WS-REJECT-COUNT > 0 (CBTRN02C.cbl:227-231).
EXPECTED_POSTTRAN_RC=4
# FIXPOST-901: posted count 262, the rows SELECT count(*) FROM transactions must return.
EXPECTED_POSTED_COUNT=262

# NAV's COTRN00 -> COTRN01 selection needs posted rows, so a zero expectation is itself an error.
if ! [[ $EXPECTED_POSTED_COUNT =~ ^[1-9][0-9]*$ ]]; then
  echo 'e2e: EXPECTED_POSTED_COUNT (FIXPOST-901) must be greater than 0' >&2
  exit 1
fi

# Checked before any Docker command: carddemo-api refuses to start without it.
if [ -z "${CARDDEMO_JWT_SECRET:-}" ]; then
  echo 'e2e: CARDDEMO_JWT_SECRET is empty or unset; carddemo-api refuses to start without it.' >&2
  echo 'e2e: in modern/, run cp .env.example .env and set CARDDEMO_JWT_SECRET to a local value.' >&2
  exit 1
fi

# The API's own default when the variable is unset or empty, as runtime::config and the Vite
# proxy read it.
API_BIND=${CARDDEMO_API_BIND:-127.0.0.1:8080}
API_URL=http://$API_BIND/api-docs/openapi.json
# The dev server's port, decided as vite.config.ts decides it: CARDDEMO_WEB_PORT when it is 1 to 5
# decimal digits within 1..65535, otherwise 5173. Vite binds 127.0.0.1 with strictPort, and
# playwright.config.ts takes its baseURL from the same configuration.
WEB_PORT=5173
if [[ ${CARDDEMO_WEB_PORT:-} =~ ^[0-9]{1,5}$ ]] \
  && ((10#$CARDDEMO_WEB_PORT >= 1 && 10#$CARDDEMO_WEB_PORT <= 65535)); then
  WEB_PORT=$((10#$CARDDEMO_WEB_PORT))
fi
VITE_URL=http://127.0.0.1:$WEB_PORT
# Under var/, which modern/.gitignore ignores. Each run overwrites the previous run's logs.
LOG_DIR=var/e2e
mkdir -p "$LOG_DIR"

API_PID=''
VITE_PID=''

# Stops a server this script started: SIGTERM (both stop gracefully on it), then SIGKILL when it
# is still running 10 s later.
stop_server() {
  local pid=$1 name=$2 i
  kill "$pid" 2>/dev/null || return 0
  for ((i = 0; i < 100; i++)); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  if kill -0 "$pid" 2>/dev/null; then
    echo "e2e: $name still running 10 s after SIGTERM; sending SIGKILL" >&2
    kill -KILL "$pid" 2>/dev/null || true
  fi
  wait "$pid" 2>/dev/null || true
}

# Runs on every exit once installed, keeping the exit status. SIGINT and SIGTERM are ignored while
# it runs, and by the commands it starts, so a second interrupt cannot leave the stack behind.
cleanup() {
  local rc=$?
  trap '' INT TERM
  if [ -n "$API_PID" ]; then stop_server "$API_PID" carddemo-api; fi
  if [ -n "$VITE_PID" ]; then stop_server "$VITE_PID" 'the Vite dev server'; fi
  echo 'e2e: removing the stack and its data (docker compose down -v)'
  docker compose down -v || true
  exit "$rc"
}
trap cleanup EXIT
# exit runs the EXIT trap, which then sees the conventional status of each signal.
trap 'exit 130' INT
trap 'exit 143' TERM

# psql inside the Compose postgres container, on the database dev-up.sh migrated and loaded.
# -X skips any psqlrc; ON_ERROR_STOP makes the first failing statement fail psql.
psql_db() {
  docker compose exec -T postgres psql -X -q -v ON_ERROR_STOP=1 -U carddemo -d carddemo "$@"
}

# Fails unless TABLE holds exactly EXPECTED rows. Only the count is read and printed, never a row.
expect_rows() {
  local table=$1 expected=$2 count
  count=$(psql_db -tAc "SELECT count(*) FROM $table")
  count=${count//[[:space:]]/}
  if ! [[ $count =~ ^[0-9]+$ ]]; then
    echo "e2e: SELECT count(*) FROM $table did not return an integer" >&2
    return 1
  fi
  if [ "$count" -ne "$expected" ]; then
    echo "e2e: $table row count is $count; expected $expected" >&2
    return 1
  fi
  echo "e2e: $table row count is $count, as expected"
}

# Exit status 0 when URL answers with a 2xx status within 5 s, 1 otherwise. node is a
# prerequisite of the frontend; curl is not.
probe() {
  node -e 'fetch(process.argv[1], { signal: AbortSignal.timeout(5000) })
    .then((r) => process.exit(r.ok ? 0 : 1), () => process.exit(1))' "$1"
}

# Fails unless nothing answers at URL yet: a server left over from another run would otherwise
# answer for the one about to start, which would then exit on its occupied port.
require_unanswered() {
  local url=$1 name=$2
  if probe "$url"; then
    echo "e2e: $url already answers before $name is started; stop the process serving it" >&2
    return 1
  fi
}

# Polls URL once a second until it answers, for at most 120 s, printing progress every 10 s.
# Fails at once when PID, the server started for URL, has exited. Names LOG on failure, never its
# contents, which can hold request data.
wait_for() {
  local url=$1 pid=$2 name=$3 log=$4 start=$SECONDS next=0 elapsed
  until probe "$url"; do
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "e2e: $name exited before answering at $url; see $log" >&2
      return 1
    fi
    elapsed=$((SECONDS - start))
    if [ "$elapsed" -ge 120 ]; then
      echo "e2e: $name did not answer at $url within 120 s; see $log" >&2
      return 1
    fi
    if [ "$elapsed" -ge "$next" ]; then
      echo "e2e: waiting for $name at $url (${elapsed} s)"
      next=$((elapsed + 10))
    fi
    sleep 1
  done
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "e2e: $name exited, yet $url answers: another process serves it; see $log" >&2
    return 1
  fi
  echo "e2e: $name answers at $url"
}

# 1. A fresh, migrated and loaded database.
echo 'e2e: removing any previous stack and its data (docker compose down -v)'
docker compose down -v
scripts/dev-up.sh

# 2. posttran's status is captured rather than left to set -e, because the audited one is 4.
echo "e2e: posting the daily transactions (carddemo-batch posttran; log: $LOG_DIR/posttran.log)"
rc=0
cargo run -q --locked -p batch --bin carddemo-batch -- posttran > "$LOG_DIR/posttran.log" || rc=$?
if [ "$rc" -ne "$EXPECTED_POSTTRAN_RC" ]; then
  echo "e2e: posttran exited with status $rc; expected $EXPECTED_POSTTRAN_RC (FIXPOST-903);" \
    "see $LOG_DIR/posttran.log" >&2
  exit 1
fi
echo "e2e: posttran exited with status $rc, as expected"
# carddemo-batch prints each file it wrote as a line that does not begin with "{", among its JSON
# events; only the DALYREJS path is shown, never the file's records.
awk '!/^\{/ && /DALYREJS\.G[0-9]+$/ { print "e2e: posttran wrote " $0 }' "$LOG_DIR/posttran.log"
expect_rows transactions "$EXPECTED_POSTED_COUNT"

# 3. The pending-authorization seed: one transaction holding one summary and one detail.
echo 'e2e: applying frontend/e2e/fixtures/pending-auth-seed.sql'
psql_db < frontend/e2e/fixtures/pending-auth-seed.sql
expect_rows pending_auth_summaries 1
expect_rows pending_auth_details 1

# 4. The API and the dev server, each started so that $! is the server process itself.
if [ ! -d frontend/node_modules ]; then
  echo 'e2e: installing frontend dependencies (pnpm install --frozen-lockfile)'
  pnpm --dir frontend install --frozen-lockfile
fi

echo 'e2e: building carddemo-api'
cargo build -q --locked -p api --bin carddemo-api
API_BIN=${CARGO_TARGET_DIR:-target}/debug/carddemo-api
if [ ! -x "$API_BIN" ]; then
  echo "e2e: $API_BIN is missing after the build (is CARGO_BUILD_TARGET set?)" >&2
  exit 1
fi

require_unanswered "$API_URL" carddemo-api
echo "e2e: starting carddemo-api on $API_BIND (log: $LOG_DIR/api.log)"
"$API_BIN" > "$LOG_DIR/api.log" 2>&1 &
API_PID=$!
wait_for "$API_URL" "$API_PID" carddemo-api "$LOG_DIR/api.log"

# The pnpm shim in node_modules/.bin execs node, so the subshell's pid becomes Vite's.
require_unanswered "$VITE_URL/" 'the Vite dev server'
echo "e2e: starting the Vite dev server on $VITE_URL (log: $LOG_DIR/vite.log)"
(cd frontend && exec node_modules/.bin/vite) > "$LOG_DIR/vite.log" 2>&1 &
VITE_PID=$!
wait_for "$VITE_URL/" "$VITE_PID" 'the Vite dev server' "$LOG_DIR/vite.log"

# 5. NAV. A failure exits with Playwright's status, and the EXIT trap tears everything down.
echo 'e2e: running NAV (pnpm --dir frontend e2e)'
pnpm --dir frontend e2e
echo 'e2e: NAV passed'
