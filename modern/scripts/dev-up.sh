#!/usr/bin/env bash
# Local CardDemo stack: start PostgreSQL and RabbitMQ, apply the schema, load the fixtures.
#
# This replaces the mainframe data refresh scripts/remote_refresh.sh, whose jobs 2-11 (lines
# 16-44) are ACCTFILE, CARDFILE, XREFFILE, CUSTFILE, TRANFILE, DISCGRP, TCATBALF, TRANCATG,
# TRANTYPE and DUSRSECJ. Each of those jobs except TRANFILE is a carddemo-batch subcommand of the
# same name in lower case. TRANFILE is out of scope: its only input exists in EBCDIC, and its
# effect, an initialised transaction file, is the empty transactions table that the migration
# creates. The CICS close and open of the files (CLOSEFIL and OPENFIL around the refresh, and the
# SDSF steps inside CARDFILE and CUSTFILE) have no counterpart, because nothing holds the tables
# open between requests.
#
# Steps, stopping at the first failure with that command's non-zero exit status:
#   1. docker compose up -d --wait: postgres and rabbitmq from compose.yaml, both healthy, then a
#      check that each container port compose.yaml publishes (postgres 5432, rabbitmq 5672 and
#      15672) has a host address. A container left created by an earlier run whose host port
#      was taken starts without one: its healthcheck passes inside the container, yet nothing on
#      the host reaches it. Each service with an unpublished port is recreated once
#      (--force-recreate, which keeps the named volumes), and a port still unpublished after
#      that stops the script with exit status 1 and the remedy;
#   2. carddemo-migrate: the only path by which the schema reaches the database (Single Schema
#      Executor); a run against a migrated database applies nothing and succeeds;
#   3. the nine loads, each replacing one table with its fixture file or, for dusrsecj, with the
#      in-stream records of ../app/jcl/DUSRSECJ.jcl.
# Nothing else is started: no API, worker, frontend or other batch job.
#
# Usage: scripts/dev-up.sh, from any working directory, with no arguments (-h or --help prints
# the usage). Configuration comes from modern/.env (copied from .env.example), exported before
# any command runs. SQLX_OFFLINE is left as the caller set it: the database migrated in step 2
# satisfies the sqlx query macros when the batch crate is compiled, and the committed .sqlx/
# serves offline builds.
set -euo pipefail

# modern/: relative configuration such as CARDDEMO_FIXTURE_DIR=../app/data/ASCII, the default
# DUSRSECJ deck ../app/jcl/DUSRSECJ.jcl and CARDDEMO_VAR_DIR=./var resolves against it.
cd "$(dirname "${BASH_SOURCE[0]}")/.."
set -a; [ -f .env ] && . ./.env; set +a

usage() {
  cat <<'EOF'
Usage: scripts/dev-up.sh [-h | --help]

Starts the local PostgreSQL 16 and RabbitMQ 3.13 services (docker compose up -d --wait), applies
the migrations with carddemo-migrate, then runs the nine fixture loads of carddemo-batch:
acctfile cardfile xreffile custfile discgrp tcatbalf trancatg trantype dusrsecj.
It changes to its own modern/ directory first and exports modern/.env when present. Rerunning it
reloads the same records. A service whose container port (postgres 5432, rabbitmq 5672 or 15672)
has no host address after the start, as when an earlier run found its host port taken, is
recreated once with docker compose up -d --wait --force-recreate, which keeps the data volumes.

Configuration: modern/.env is exported after the caller's environment, so a variable it sets wins
over the caller's value of the same name; a variable .env leaves unset keeps the caller's value.
  DATABASE_URL          the database carddemo-migrate and the loads connect to, also read by the
                        sqlx query macros when carddemo-batch compiles without SQLX_OFFLINE=true;
                        .env.example points it at the Compose postgres database carddemo on
                        localhost:5432
  CARDDEMO_FIXTURE_DIR  the fixture directory, resolved against modern/ (.env.example:
                        ../app/data/ASCII); the eight IDCAMS loads read <name>.txt in it for
                        acctdata, carddata, cardxref, custdata, discgrp, tcatbal, trancatg and
                        trantype, and dusrsecj reads ../app/jcl/DUSRSECJ.jcl
  COMPOSE_PROJECT_NAME  the Compose project (default carddemo, the name: of compose.yaml)
  COMPOSE_FILE          the Compose files (default compose.yaml); an override file remaps the
                        host ports, with the port in DATABASE_URL changed to match, as section 9
                        of docs/SETUP.md shows
  SQLX_OFFLINE          left as the caller set it (.env.example leaves it unset); true compiles
                        carddemo-batch against the committed .sqlx/ instead of the database

Example, compiling against the committed .sqlx/ (the caller's value holds, because .env.example
leaves SQLX_OFFLINE unset):
  SQLX_OFFLINE=true scripts/dev-up.sh

Exit status:
  0    every step succeeded
  1    a container port is still not published on the host after its service was recreated;
       the message names the port and the remedy: docker compose down, free the host port,
       then rerun scripts/dev-up.sh
  2    usage error: an unexpected argument
  Any other failure exits with the status of the first docker or cargo command that failed,
  which can also be 1; a failed recreate prints the same message and remedy first.
EOF
}

if [ $# -eq 1 ] && { [ "$1" = -h ] || [ "$1" = --help ]; }; then
  usage
  exit 0
fi
if [ $# -gt 0 ]; then
  echo 'dev-up: no arguments are taken other than a single -h or --help' >&2
  usage >&2
  exit 2
fi

# Loads are order-independent and rerunnable: each replaces one table inside one transaction
# (delete, then insert every record), as IDCAMS DELETE/DEFINE/REPRO does, and no foreign key
# joins the loaded tables. This order only mirrors the source refresh, less TRANFILE.
LOADS=(acctfile cardfile xreffile custfile discgrp tcatbalf trancatg trantype dusrsecj)

# Container ports compose.yaml publishes on the host, as service:port.
PUBLISHED=(postgres:5432 rabbitmq:5672 rabbitmq:15672)

# Sets UNPUBLISHED to each service:port of PUBLISHED for which docker compose port gives no host
# address; its error for such a port ("no port ... for container ...") goes to standard error.
find_unpublished() {
  local entry address
  UNPUBLISHED=()
  for entry in "${PUBLISHED[@]}"; do
    address=$(docker compose port "${entry%%:*}" "${entry#*:}") || address=''
    if [ -z "$address" ]; then
      UNPUBLISHED+=("$entry")
    fi
  done
}

# Prints, to standard error, the unpublished ports and the remedy.
unpublished_remedy() {
  local entry
  for entry in "${UNPUBLISHED[@]}"; do
    echo "dev-up: ${entry%%:*} container port ${entry#*:} is not published on the host" >&2
  done
  echo 'dev-up: remedy: in modern/ with modern/.env exported, run docker compose down, free the' \
    'host port that compose.yaml (or an override in COMPOSE_FILE) maps it to, then rerun' \
    'scripts/dev-up.sh' >&2
}

echo 'dev-up: starting postgres and rabbitmq (docker compose up -d --wait)'
docker compose up -d --wait

find_unpublished 2>/dev/null
if [ "${#UNPUBLISHED[@]}" -gt 0 ]; then
  recreate=()
  for entry in "${UNPUBLISHED[@]}"; do
    case " ${recreate[*]} " in
      *" ${entry%%:*} "*) ;;
      *) recreate+=("${entry%%:*}") ;;
    esac
  done
  echo "dev-up: ${UNPUBLISHED[*]} not published on the host; recreating ${recreate[*]}" \
    "(docker compose up -d --wait --force-recreate ${recreate[*]})"
  rc=0
  docker compose up -d --wait --force-recreate "${recreate[@]}" || rc=$?
  if [ "$rc" -ne 0 ]; then
    unpublished_remedy
    exit "$rc"
  fi
  find_unpublished
  if [ "${#UNPUBLISHED[@]}" -gt 0 ]; then
    unpublished_remedy
    exit 1
  fi
fi

echo 'dev-up: applying migrations (carddemo-migrate)'
cargo run -q --locked -p runtime --bin carddemo-migrate

for job in "${LOADS[@]}"; do
  echo "dev-up: loading ${job} (carddemo-batch ${job})"
  cargo run -q --locked -p batch --bin carddemo-batch -- "$job"
done

echo "dev-up: stack up, schema migrated, ${#LOADS[@]} loads done"
