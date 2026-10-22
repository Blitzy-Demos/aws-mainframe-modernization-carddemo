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
#   1. docker compose up -d --wait: postgres and rabbitmq from compose.yaml, both healthy;
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
reloads the same records.

Exit status: 0 when every step succeeded; otherwise the status of the first command that failed,
or 2 for an unexpected argument.
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

echo 'dev-up: starting postgres and rabbitmq (docker compose up -d --wait)'
docker compose up -d --wait

echo 'dev-up: applying migrations (carddemo-migrate)'
cargo run -q --locked -p runtime --bin carddemo-migrate

for job in "${LOADS[@]}"; do
  echo "dev-up: loading ${job} (carddemo-batch ${job})"
  cargo run -q --locked -p batch --bin carddemo-batch -- "$job"
done

echo "dev-up: stack up, schema migrated, ${#LOADS[@]} loads done"
