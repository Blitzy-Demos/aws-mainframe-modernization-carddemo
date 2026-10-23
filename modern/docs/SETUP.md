# CardDemo replacement: environment setup

This document takes a developer from a fresh Linux or macOS machine to a running CardDemo replacement: PostgreSQL 16 and RabbitMQ 3.13 in Docker Compose, the Rust API, workers and batch jobs, and the React frontend. Everything runs on the local machine; no cloud account, SDK, credential or hosted service is used at any step. Where each component lives and why is in [ARCHITECTURE.md](ARCHITECTURE.md), the endpoints are in [API.md](API.md), and the acceptance checklist with its per-criterion commands is [SIGNOFF.md](SIGNOFF.md).

Every command runs from `modern/`, the directory that holds [`compose.yaml`](../compose.yaml) and the Cargo workspace, unless a step says otherwise.

## Contents

1. [Prerequisites](#1-prerequisites)
2. [Configuration](#2-configuration)
3. [Database and broker](#3-database-and-broker)
4. [API](#4-api)
5. [Messaging workers](#5-messaging-workers)
6. [Batch jobs](#6-batch-jobs)
7. [Frontend](#7-frontend)
8. [Build and test](#8-build-and-test)
9. [Troubleshooting](#9-troubleshooting)

## 1. Prerequisites

Install these once per machine:

- **Docker Engine with the Compose v2 plugin** (`docker compose`, not the legacy `docker-compose`). On macOS, Docker Desktop provides both. The first `docker compose up` and the first container-backed test pull the images `postgres:16` and `rabbitmq:3.13-management`.
- **A C toolchain that provides `cc`.** Rust uses it to link and to build procedural macros. On Debian or Ubuntu that is the `build-essential` package; on macOS, the Xcode Command Line Tools (`xcode-select --install`).
- **rustup.** No toolchain needs installing by hand: the first cargo command inside `modern/` reads [`rust-toolchain.toml`](../rust-toolchain.toml) and installs Rust 1.85.0 with clippy, rustfmt and llvm-tools-preview.
- **Three cargo tools**, each built from its own lockfile:

  ```bash
  cargo install --locked sqlx-cli@0.8.6 --no-default-features --features postgres,rustls
  cargo install --locked cargo-deny@0.18.9
  cargo install --locked cargo-llvm-cov@0.6.21
  ```

  sqlx-cli 0.8.6 matches the workspace's sqlx 0.8.6 and is used only for `cargo sqlx prepare`. cargo-deny is 0.18.9 rather than 0.18.3, because 0.18.3 cannot load the current RustSec advisory database: it stops at the first advisory scored with CVSS 4.0 (`unsupported CVSS version: 4.0`). 0.18.9 stays on the 0.18 line and builds with Rust 1.85.0. `cargo deny check` downloads that public database on its first run.
- **Node.js 22**, the major line [`frontend/.nvmrc`](../frontend/.nvmrc) names; use a current 22.x release. Then pnpm 9.15.9, the version `packageManager` in [`frontend/package.json`](../frontend/package.json) names:

  ```bash
  npm install -g pnpm@9.15.9
  ```

- **Playwright's Chromium**, needed only for the NAV suite. Install it after the frontend dependencies (section 7):

  ```bash
  pnpm --dir frontend exec playwright install --with-deps chromium
  ```

- **macOS only: GNU bash and stat, and flock, for the check scripts.** [`scripts/protected-diff.sh`](../scripts/protected-diff.sh), which [`scripts/check.sh`](../scripts/check.sh) runs, uses bash 4.2 features (`mapfile`, `lastpipe`) and GNU `stat -c`. [`scripts/regen-api.sh`](../scripts/regen-api.sh), which `check.sh` also runs, makes concurrent runs in one checkout wait for each other with `flock`, which util-linux provides on Linux. macOS ships bash 3.2 and BSD `stat` and has no `flock`, so install `bash`, `coreutils` and `flock` (for example with Homebrew) and put them ahead of the system tools on `PATH`; the scripts start through `/usr/bin/env bash`.

`curl` is optional; section 4 uses it for a sign-on check.

## 2. Configuration

Create the local configuration from the committed template, [`.env.example`](../.env.example):

```bash
cp .env.example .env
```

Then edit `.env` and replace the placeholder of `CARDDEMO_JWT_SECRET` with any local value. The API refuses to start when this variable is empty or unset. `.env` is git-ignored; never commit it.

No program loads `.env` by itself. The scripts in [`scripts/`](../scripts/) export it before their first command. In every shell where you run a binary, Vite or Playwright by hand, export it first:

```bash
set -a; . ./.env; set +a
```

Every variable the code reads, with the value `.env.example` gives it:

| Variable | Local default | Read by |
|---|---|---|
| `DATABASE_URL` | `postgres://carddemo:carddemo@localhost:5432/carddemo` | api, messaging, batch, `carddemo-migrate`; the sqlx macros at build time unless `SQLX_OFFLINE=true`; `cargo sqlx prepare` |
| `CARDDEMO_AMQP_URL` | `amqp://carddemo:carddemo@localhost:5672/%2f` (vhost `/` URL-encoded) | messaging |
| `CARDDEMO_QUEUE_REQUEST_DATE` | `CARD.DEMO.REQUEST.DATE` | messaging (CDRD) |
| `CARDDEMO_QUEUE_REQUEST_ACCT` | `CARD.DEMO.REQUEST.ACCT` | messaging (CDRA) |
| `CARDDEMO_QUEUE_REQUEST_PAUTH` | `CARD.DEMO.REQUEST.PAUTH` | messaging (CP00) |
| `CARDDEMO_MQ_POLL_MS` | `250` (1 to 5000; a larger value is refused at startup) | messaging |
| `CARDDEMO_API_BIND` | `127.0.0.1:8080` | api (listen address); the Vite dev server's `/api` proxy target |
| `CARDDEMO_JWT_SECRET` | none in code; `.env.example` carries a local-only placeholder | api |
| `CARDDEMO_JWT_TTL_SECONDS` | `3600` (1 to 9223372036854775807, which is `i64::MAX`; a larger value is refused at startup) | api |
| `CARDDEMO_VAR_DIR` | `./var` | batch (`dalyrejs/`, `systran/`, `reports/` under it); api, as the base of `CARDDEMO_INTRDR_DIR` when that is unset |
| `CARDDEMO_STATEMENT_DIR` | `./var/statements/` | batch (`creastmt`) |
| `CARDDEMO_INTRDR_DIR` | `./var/jobs/` (created by the first submission; a path that names something other than a directory is refused at startup) | api (the job decks of the report request, endpoint 14) |
| `CARDDEMO_FIXTURE_DIR` | `../app/data/ASCII` | batch (the eight file loads) |
| `CARDDEMO_OTEL_EXPORT` | unset (commented out): no span export; `stdout` enables it | every binary except `carddemo-openapi` |
| `RUST_LOG` | `info` | every binary except `carddemo-openapi` (the tracing filter) |

Notes on these values:

- An empty value counts as unset and takes the default. When `CARDDEMO_STATEMENT_DIR` or `CARDDEMO_INTRDR_DIR` is unset, it becomes `statements` or `jobs` under `CARDDEMO_VAR_DIR`. `.env.example` sets both explicitly, so changing `CARDDEMO_VAR_DIR` alone does not move statements or job decks.
- Relative paths resolve against `modern/`, the working directory of every binary.
- The database and broker credentials (`carddemo`/`carddemo`) are local-only values that match `compose.yaml` and [`rabbitmq/definitions.json`](../rabbitmq/definitions.json). The URLs must carry them: the libpq variables (`PGHOST`, `PGUSER`, `PGPASSWORD` and the others) are not read.
- `CARDDEMO_OTEL_EXPORT=stdout` prints each finished span as a multi-line text block between the JSON event lines. Leave it unset when anything reads standard output line by line as JSON. Any other non-empty value is a configuration error.
- `carddemo-mq` records a request's `message_id` and `correlation_id` in its message spans and the CP00 alert as their base64url HMAC-SHA256 keyed by its `CARDDEMO_AMQP_URL`, never as their text, and a CP00 reply's generated `message_id` verbatim as `reply_message_id`. Whoever holds the broker credentials in that URL can already read every request from the broker, so the digest is exactly as protected as the messages it identifies; the URL itself is never logged. To find the lines of a request whose ID you know, compute its digest in a shell whose `CARDDEMO_AMQP_URL` holds the URL `carddemo-mq` ran with (export `.env`, or set the default above if it ran with the variable unset) and search for it: `python3 -c 'import base64,hashlib,hmac,os,sys; print(base64.urlsafe_b64encode(hmac.new(os.environ["CARDDEMO_AMQP_URL"].encode(), sys.argv[1].encode(), hashlib.sha256).digest()).rstrip(b"=").decode())' "$ID"`.

Variables that tools read, outside the table above:

- `SQLX_OFFLINE=true` makes the sqlx macros check queries against the committed `.sqlx/` cache, so the workspace builds without a database. `.env.example` leaves it commented out, because setting it there would change every build.
- `CARDDEMO_WEB_PORT` sets the Vite dev server's port (default `5173`); [`frontend/vite.config.ts`](../frontend/vite.config.ts), [`frontend/playwright.config.ts`](../frontend/playwright.config.ts) and `scripts/e2e.sh` read it.
- `RUST_TEST_THREADS` bounds how many tests, and so how many test containers, run at once.
- `MALLOC_ARENA_MAX` and `MALLOC_TRIM_THRESHOLD_` are glibc's allocator settings: no CardDemo program reads them, and `.env.example` does not set them. glibc keeps the heap a burst of requests used for the next burst rather than returning it, so the resident memory of `carddemo-api` stays at its high-water mark after the load ends (section 4); that is not a leak. To shrink that footprint, set them in the shell that starts the API, for example `MALLOC_ARENA_MAX=2 MALLOC_TRIM_THRESHOLD_=131072 cargo run -p api --bin carddemo-api`. `MALLOC_ARENA_MAX` caps the number of heaps (arenas) that glibc creates for threads allocating at once, and `MALLOC_TRIM_THRESHOLD_` is the free space, in bytes, at the top of the heap above which glibc returns memory to the system; both trade some allocation speed for a smaller footprint.

## 3. Database and broker

Start both services and wait until their healthchecks pass:

```bash
docker compose up -d --wait
```

This starts PostgreSQL 16 on `localhost:5432` and RabbitMQ 3.13 on `localhost:5672`, both bound to 127.0.0.1 only. The database's data lives in the named volume `pgdata`, and the broker's queues and persistent messages in the named volume `rabbitmq-data`. RabbitMQ imports [`rabbitmq/definitions.json`](../rabbitmq/definitions.json) at boot, through [`rabbitmq/rabbitmq.conf`](../rabbitmq/rabbitmq.conf): vhost `/`, user `carddemo` with the local-only password `carddemo`, and seven durable queues. The management console at <http://localhost:15672> accepts that user and lists the queues:

| Queue | Role |
|---|---|
| `CARD.DEMO.REQUEST.DATE` | CDRD requests |
| `CARD.DEMO.REQUEST.ACCT` | CDRA requests |
| `CARD.DEMO.REQUEST.PAUTH` | CP00 requests; dead-letters to `CARD.DEMO.DEADLETTER.PAUTH` |
| `CARD.DEMO.REPLY.DATE` | CDRD replies |
| `CARD.DEMO.REPLY.ACCT` | CDRA replies |
| `CARD.DEMO.ERROR` | CDRD and CDRA error messages |
| `CARD.DEMO.DEADLETTER.PAUTH` | CP00 requests the worker rejects without requeue: an unparseable card number or a reply that was not confirmed |

Apply the schema:

```bash
cargo run -p runtime --bin carddemo-migrate
```

`carddemo-migrate` is the only program that changes the schema; a run against a migrated database applies nothing and succeeds. `carddemo-api`, `carddemo-mq` and `carddemo-batch` never migrate, so against an unmigrated database they fail at their first query. sqlx-cli is used only for `cargo sqlx prepare` (section 8), never for `cargo sqlx migrate run`.

[`scripts/dev-up.sh`](../scripts/dev-up.sh) does both steps, then loads the fixtures:

```bash
scripts/dev-up.sh
```

After the migration it runs nine `carddemo-batch` loads in this order: `acctfile`, `cardfile`, `xreffile`, `custfile`, `discgrp`, `tcatbalf`, `trancatg`, `trantype`, `dusrsecj`. That is the mainframe data refresh order of [`scripts/remote_refresh.sh:16-44`](../../scripts/remote_refresh.sh) without TRANFILE, whose only input exists in EBCDIC; its effect, an empty transaction file, is the `transactions` table the migration creates. The CICS close and open around the mainframe refresh (CLOSEFIL, OPENFIL) have no counterpart. The eight file loads read the ASCII fixtures in place from `CARDDEMO_FIXTURE_DIR`, and `dusrsecj` reads the in-stream user records of [`app/jcl/DUSRSECJ.jcl:35-44`](../../app/jcl/DUSRSECJ.jcl), storing each password as an argon2id hash. Each load replaces one table inside one transaction, so the loads are order-independent and the script can be rerun at any time to reload the same records.

**Renaming the authorization queue.** `CARDDEMO_QUEUE_REQUEST_PAUTH` may name another queue, but that queue must be declared durable with the arguments `x-dead-letter-exchange: ""` and `x-dead-letter-routing-key: CARD.DEMO.DEADLETTER.PAUTH`, as `definitions.json` declares the default; add it there. The workers create no queue: they only check each one with a passive declare. Without those arguments, a request that CP00 rejects, such as one whose card number cannot be parsed, is discarded by the broker instead of being dead-lettered. The other two request queues need only be durable. The reply and error queue names are COBOL literals and cannot be renamed.

## 4. API

With the stack up, the schema applied and `.env` exported:

```bash
cargo run -p api --bin carddemo-api
```

The server listens on `CARDDEMO_API_BIND` (default `127.0.0.1:8080`), serves the 27 operations under `/api/v1`, and serves its OpenAPI document at `GET /api-docs/openapi.json`. It writes JSON log events to standard output and stops gracefully on Ctrl-C or SIGTERM: the requests in flight get 8 seconds to finish and the server exits 0, or, when that deadline passes first, they are abandoned, an `ERROR` event says so and it exits 1, in either case within about 9 seconds of the signal. It connects to PostgreSQL only; no online transaction sends a broker message.

The server works on at most 256 requests at once (`MAX_IN_FLIGHT` in [`crates/api/src/app.rs`](../crates/api/src/app.rs), compiled in; no variable sets it). A request that arrives while 256 are in flight, on any route, waits for a place, in arrival order, for up to 5 seconds (`ADMISSION_WAIT`, also compiled in), and is served as soon as one frees, so a burst beyond the limit is served a little later rather than refused. Only when no place frees within those 5 seconds is it answered `503 Service Unavailable` with `Retry-After: 1` and a `ScreenError` whose message is `SERVER_BUSY` ([API.md, section 4](API.md#4-error-model)); nothing of it has been read or written, and it can be sent again. The 5 seconds match the request pool's wait for a connection, are under the 8-second drain deadline above and far under the frontend's 30-second request deadline. Idle connections hold no place, and the number of connections is not limited. Expect memory to follow the work in flight rather than the number of clients: each open connection costs tens of kilobytes, because the OpenAPI document is held once for the whole process rather than copied per connection, and each request in flight buffers at most its body, which axum caps at 2 MiB, so 512 MiB at the limit. After a burst the resident memory stays near its high-water mark, since glibc keeps freed heap for reuse; the allocator settings of section 2 make it return memory sooner.

After `dusrsecj` has loaded the users, sign on as `ADMIN001` (user type `A`, admin menu) or `USER0001` (user type `U`, main menu), both with the password `PASSWORD`, the local users of the root [README.md:203-204](../../README.md):

```bash
curl -s -X POST "http://$CARDDEMO_API_BIND/api/v1/signon" \
  -H 'content-type: application/json' \
  -d '{"user_id":"ADMIN001","password":"PASSWORD"}'
```

The response carries `token`, `user_id`, `user_type` and `next_route`. Send the token as `Authorization: Bearer <token>` on every other operation; [API.md](API.md#2-endpoints) lists them.

To print the OpenAPI document without a server, database or environment:

```bash
cargo run -p api --bin carddemo-openapi
```

It reads no environment variable and opens no connection; [API.md](API.md#11-contract-and-regeneration) explains how `scripts/regen-api.sh` turns it into the committed contract and the frontend types.

## 5. Messaging workers

With the stack up, the schema applied and `.env` exported:

```bash
cargo run -p messaging --bin carddemo-mq
```

One process runs the three workers on one broker connection: CDRD (CODATE01) on `CARDDEMO_QUEUE_REQUEST_DATE`, CDRA (COACCT01) on `CARDDEMO_QUEUE_REQUEST_ACCT` and CP00 (COPAUA0C) on `CARDDEMO_QUEUE_REQUEST_PAUTH`. Each polls its queue every `CARDDEMO_MQ_POLL_MS` milliseconds while it is empty. Ctrl-C or SIGTERM stops the process after each worker has finished the request it holds.

Two or more `carddemo-mq` processes may run at once. CP00 serialises its decisions for an account with a row lock in PostgreSQL, not inside the process, so the credit-limit guarantee holds across processes and redeliveries; [ARCHITECTURE.md](ARCHITECTURE.md#10-messaging-flows) describes the three flows, their replies and failure paths.

To check the CDRD flow by hand, publish any message to its request queue through the management API, then read the reply. CODATE01 ignores the request content and replies with the system date and time:

```bash
curl -s -u carddemo:carddemo -H 'content-type: application/json' \
  -X POST http://localhost:15672/api/exchanges/%2f/amq.default/publish \
  -d '{"properties":{},"routing_key":"CARD.DEMO.REQUEST.DATE","payload":"DATE","payload_encoding":"string"}'
curl -s -u carddemo:carddemo -H 'content-type: application/json' \
  -X POST http://localhost:15672/api/queues/%2f/CARD.DEMO.REPLY.DATE/get \
  -d '{"count":1,"ackmode":"ack_requeue_false","encoding":"auto"}'
```

The first command answers `{"routed":true}`; the second returns the reply, whose payload begins `SYSTEM DATE : `.

## 6. Batch jobs

Each in-scope JCL job is one subcommand of `carddemo-batch`. With the schema applied and `.env` exported:

```bash
cargo run -p batch --bin carddemo-batch -- <subcommand> [options]
```

`cargo run -p batch --bin carddemo-batch -- --help` lists the subcommands and the environment they read, and `-- <subcommand> --help` shows one subcommand's options. `carddemo-migrate` is a separate binary, not a subcommand.

| Subcommand | JCL job (program) | Options [default] | Effect and output |
|---|---|---|---|
| `acctfile` | ACCTFILE (IDCAMS REPRO) | `--input <PATH>` [`acctdata.txt` in `CARDDEMO_FIXTURE_DIR`] | Replaces `accounts` |
| `cardfile` | CARDFILE (IDCAMS REPRO) | `--input <PATH>` [`carddata.txt` in `CARDDEMO_FIXTURE_DIR`] | Replaces `cards` |
| `xreffile` | XREFFILE (IDCAMS REPRO) | `--input <PATH>` [`cardxref.txt` in `CARDDEMO_FIXTURE_DIR`] | Replaces `card_xref` |
| `custfile` | CUSTFILE (IDCAMS REPRO) | `--input <PATH>` [`custdata.txt` in `CARDDEMO_FIXTURE_DIR`] | Replaces `customers` |
| `discgrp` | DISCGRP (IDCAMS REPRO) | `--input <PATH>` [`discgrp.txt` in `CARDDEMO_FIXTURE_DIR`] | Replaces `disclosure_groups` |
| `tcatbalf` | TCATBALF (IDCAMS REPRO) | `--input <PATH>` [`tcatbal.txt` in `CARDDEMO_FIXTURE_DIR`] | Replaces `tran_cat_balances` |
| `trancatg` | TRANCATG (IDCAMS REPRO) | `--input <PATH>` [`trancatg.txt` in `CARDDEMO_FIXTURE_DIR`] | Replaces `transaction_categories`, the VSAM data version |
| `trantype` | TRANTYPE (IDCAMS REPRO) | `--input <PATH>` [`trantype.txt` in `CARDDEMO_FIXTURE_DIR`] | Replaces `transaction_types`, the VSAM data version |
| `dusrsecj` | DUSRSECJ (IEBGENER) | `--input <PATH>` [`../app/jcl/DUSRSECJ.jcl`] | Replaces `users`, passwords stored as argon2id hashes |
| `posttran` | POSTTRAN (CBTRN02C) | `--dalytran <PATH>` [`../app/data/ASCII/dailytran.txt`] | Posts the daily transactions; rejects go to a new `dalyrejs/DALYREJS.G<nnnn>` under `CARDDEMO_VAR_DIR`; exit status 4 when any record is rejected |
| `intcalc` | INTCALC (CBACT04C) | `--parm-date <PARM>` [`2022071800`] | Adds the monthly interest to the account balances and writes a new `systran/SYSTRAN.G<nnnn>` under `CARDDEMO_VAR_DIR` |
| `creastmt` | CREASTMT (CBSTM03A, CBSTM03B) | none | Writes `STATEMNT.PS`, `STATEMNT.HTML` and `STATEMNT.PDF` in `CARDDEMO_STATEMENT_DIR`, replacing the previous run's |
| `tranrept` | TRANREPT (CBTRN03C) | `--start-date <YYYY-MM-DD>` [`2022-01-01`], `--end-date <YYYY-MM-DD>` [`2022-07-06`] | Writes the transaction detail report for that processing-date window to a new `reports/TRANREPT.G<nnnn>` under `CARDDEMO_VAR_DIR` |
| `cbpaup0j` | CBPAUP0J (CBPAUP0C) | `--sysin <RECORD>` [`00,00001,00001,Y`] | Purges the expired pending authorizations |
| `mnttrdb2` | MNTTRDB2 (COBTUPDT) | `--input <PATH>`, required: one 53-byte record per line (action, type code, description) | Applies the records to `db2.transaction_type`, the Db2 data version; exit status 4 when a record meets an SQL error |

Things to know when running them:

- A job prints every file it wrote, one path per line, among its JSON log events; a path line is any line that does not begin with `{`. With `RUST_LOG=off` only the path lines remain. The loads, `cbpaup0j` and `mnttrdb2` write no file and print no path.
- The `.G<nnnn>` outputs mirror a GDG `(+1)`: each run writes the generation one above the highest one in its directory.
- On the fixture data, `posttran` rejects some records and exits 4. That is the audited result ([`audit/fixtures.md`](../audit/fixtures.md)), not a failure.
- The loads do not empty `transactions` or the pending-authorization tables. To start again from the fixtures, remove the database and broker volumes and reload:

  ```bash
  docker compose down -v
  scripts/dev-up.sh
  ```

- Statements, reports, reject and SYSTRAN files hold full card numbers. They stay on the local filesystem under `var/`, which `modern/.gitignore` ignores; never commit or share them. No endpoint serves them.

A typical run after `scripts/dev-up.sh`:

```bash
set -a; . ./.env; set +a
cargo run -p batch --bin carddemo-batch -- posttran
cargo run -p batch --bin carddemo-batch -- intcalc
cargo run -p batch --bin carddemo-batch -- creastmt
cargo run -p batch --bin carddemo-batch -- tranrept --start-date "$(date +%F)" --end-date "$(date +%F)"
```

`posttran` stamps each posted transaction with the current local time as its processing timestamp, as CBTRN02C does, so the default `tranrept` window (the 2022 dates of the JCL) selects none of them and writes an empty report. The last command reports today's postings instead.

## 7. Frontend

Install the dependencies from the committed lockfile, then start the Vite dev server in a shell where `.env` is exported:

```bash
pnpm --dir frontend install --frozen-lockfile
pnpm --dir frontend dev
```

The dev server listens on <http://127.0.0.1:5173>, or on `CARDDEMO_WEB_PORT` when that is set, and fails rather than move to another port when the port is taken. It proxies `/api` to `CARDDEMO_API_BIND`, so `carddemo-api` must be running (section 4). Open the address and sign on as `ADMIN001`, which lands on the admin menu, or `USER0001`, which lands on the main menu, with the password `PASSWORD` ([README.md:203-204](../../README.md)), after `dusrsecj` has loaded the users. The F-keys of the 3270 screens are buttons labelled as on each map; the keyboard F-keys work where the browser does not reserve them.

The other frontend scripts of [`frontend/package.json`](../frontend/package.json):

| Command | Does |
|---|---|
| `pnpm --dir frontend typecheck` | Strict TypeScript check of the app and the tooling configuration |
| `pnpm --dir frontend build` | Production build into `frontend/dist/` |
| `pnpm --dir frontend test` | Vitest, once, with coverage; fails below 70 percent line coverage |
| `pnpm --dir frontend e2e` | Playwright NAV suite; expects `carddemo-api` and the dev server already running, so use `scripts/e2e.sh` instead (section 8) |

## 8. Build and test

[`scripts/check.sh`](../scripts/check.sh) runs every automated check in order and stops at the first failure:

```bash
scripts/check.sh
```

| Steps | Checks | Needs Docker |
|---|---|---|
| 0 | No cloud credential variable is set (Local-Only Runtime) | No |
| 1–6 | Formatting, warning-free build and clippy against the committed `.sqlx/` (`SQLX_OFFLINE=true`), and the source scans: floating point, `unsafe`, lint suppressions, non-macro SQL, integer casts, the single migration entrypoint, the CP00 lock, thin handlers | No |
| 7 | Frontend install from the lockfile, `scripts/frontend-policy.mjs`, typecheck, build, Vitest with its 70 percent line floor | No |
| 8–10 | `domain` without sqlx, both lockfiles committed and no submodules, `cargo deny check` and the cloud SDK scans | No (step 10 downloads the RustSec database) |
| 11 | `scripts/regen-api.sh`: the regenerated contract equals the committed files | No |
| 12 | `scripts/protected-diff.sh`: the COBOL tree and every path outside `modern/` are unchanged | No |
| 13–14 | `scripts/dev-up.sh`, then `cargo sqlx prepare --workspace --check -- --all-targets` | Yes |
| 15–16 | All tests under `cargo llvm-cov`, then the line floors: 80 percent over the workspace and 90 percent over `copybook` and `domain` | Yes |
| 17 | `scripts/e2e.sh` (NAV) | Yes |

The exit status is 0 only when steps 0 to 17 all pass. Otherwise the last line on standard error names the failing step. Without Docker Engine and Compose v2, the script stops after step 12 with exit status 2 and lists the bindings it left unverified; it never reports them as passed. A coverage floor that is not met fails the run.

Step 0 fails when any of the cloud credential variables the script lists is set, for example `AWS_ACCESS_KEY_ID`, `AZURE_CLIENT_ID` or `GOOGLE_APPLICATION_CREDENTIALS`. Unset them in the shell that runs the check:

```bash
env -u AWS_ACCESS_KEY_ID -u AZURE_CLIENT_ID -u GOOGLE_APPLICATION_CREDENTIALS scripts/check.sh
```

Step 2 fails before it builds when `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS`, a `CARGO_TARGET_<TRIPLE>_RUSTFLAGS`, a `CARGO_PROFILE_<NAME>_RUSTFLAGS` or `CLIPPY_CONF_DIR` is set; when the cargo home (`CARGO_HOME`, or `$HOME/.cargo` when it is unset) is a directory inside the repository; or when an entry named `.cargo`, `clippy.toml` or `.clippy.toml` exists anywhere in the repository, whether a directory, a file or a symbolic link and in any letter case. Each can lower the lint levels that `[workspace.lints]` sets. Unset the variable for the run (`env -u RUSTFLAGS scripts/check.sh`), remove it from `.env` if it is set there, point `CARGO_HOME` outside the repository (the default `~/.cargo` is accepted), and delete the entry.

Narrower commands:

- **Build without a database:** `SQLX_OFFLINE=true cargo build --workspace --locked`. Always build with `--locked`, and change `Cargo.lock` only with `cargo update -p <crate> --precise <version>`; a bare `cargo update` would move pinned crates off the versions Rust 1.85.0 can build.
- **One crate's tests:** `cargo test -p <crate> --locked`, with `SQLX_OFFLINE=true` when the Compose stack is not running. `copybook` and `domain` need no Docker. `persistence`, `api`, `messaging`, `batch` and `test-support` start their own PostgreSQL and RabbitMQ test containers and remove them when the tests end; `RUST_TEST_THREADS=4` keeps the number of containers running at once low.
- **Frontend unit tests:** `pnpm --dir frontend test`.
- **NAV:** `scripts/e2e.sh` recreates the stack, loads the fixtures, runs `posttran`, applies the pending-authorization seed, starts `carddemo-api` and the dev server, runs Playwright, and removes the stack again with `docker compose down -v`. It needs Playwright's Chromium (section 1) and free API and dev-server ports.
- **Contract:** `scripts/regen-api.sh` regenerates `openapi/openapi.json` and `frontend/src/api/schema.d.ts` and fails when either differs from the committed version; commit the regenerated files.
- **Protected paths:** `scripts/protected-diff.sh` fails on any change, staged or not, to a path outside `modern/`, untracked files included.
- **Coverage on its own:** `cargo llvm-cov --workspace --locked --no-report`, then `cargo llvm-cov report --fail-under-lines 80 --ignore-filename-regex '/(tests|benches)/'`. The instrumented build takes several gigabytes under `target/llvm-cov-target`, and `scripts/check.sh` leaves it there too; remove that directory afterwards to reclaim the space.

Every `scripts/*.sh` script accepts `-h` or `--help`, prints its usage and runs nothing else, and so does `scripts/frontend-policy.mjs`, which also takes an optional directory to check in place of `modern/`. [SIGNOFF.md](SIGNOFF.md) gives the command for each acceptance criterion.

**After changing a query or a migration.** Bring the stack up, apply the migrations, then regenerate the offline query cache and commit `.sqlx/` together with the change:

```bash
docker compose up -d --wait
cargo run -p runtime --bin carddemo-migrate
cargo sqlx prepare --workspace -- --all-targets
```

`.sqlx/` is derived from `crates/persistence/migrations/` and the queries, so it is never edited by hand; step 14 of `scripts/check.sh` fails when it is stale. Migrations only add: none drops, renames or retypes a column.

**Stopping.** `docker compose down` stops the stack and keeps both volumes, `pgdata` and `rabbitmq-data`; `docker compose down -v` also removes both, with every loaded row and every queued message.

## 9. Troubleshooting

- **A binary fails at its first query:** the schema has not been applied. Run `cargo run -p runtime --bin carddemo-migrate`, or `scripts/dev-up.sh`.
- **`carddemo-api` exits at start:** `CARDDEMO_JWT_SECRET` is empty, or `.env` was not exported in that shell (`set -a; . ./.env; set +a`). A configuration error names its variable, never its value.
- **A port is already in use** (5432, 5672, 15672, 8080 or 5173): stop the process that holds it, or move the service. The API and the dev server move with `CARDDEMO_API_BIND` and `CARDDEMO_WEB_PORT`. The Compose ports are fixed in `compose.yaml`; to remap them, keep an override file outside the checkout, add `COMPOSE_FILE=compose.yaml:/path/to/override.yaml` to `.env`, and change the ports in `DATABASE_URL` and `CARDDEMO_AMQP_URL` to match. Such a file replaces the port lists with the `!override` tag (Compose 2.24 or later); the management console then answers on the third port:

  ```yaml
  services:
    postgres:
      ports: !override
        - "127.0.0.1:25432:5432"
    rabbitmq:
      ports: !override
        - "127.0.0.1:25672:5672"
        - "127.0.0.1:25673:15672"
  ```

- **Local data disappeared after NAV:** `scripts/e2e.sh` runs `docker compose down -v` before and after the suite. Run `scripts/dev-up.sh` again.
- **`scripts/check.sh` stops at step 0:** a cloud credential variable is set in the environment; unset it as section 8 shows.
- **`scripts/check.sh` stops at step 11:** the handlers now generate a different contract. Review and commit `openapi/openapi.json` and `frontend/src/api/schema.d.ts`, which the failed run left regenerated.
- **`scripts/check.sh` stops at step 12:** a file outside `modern/` is modified or untracked. Move it out of the checkout or restore it; the COBOL tree is the read-only parity oracle.
- **`cargo deny check` cannot load the advisory database** (`unsupported CVSS version`): the installed cargo-deny is older than 0.18.9; reinstall it as section 1 shows.
- **The build fails on a sqlx query macro:** with `.env` exported, `DATABASE_URL` is set, so the macros check every query against that database at compile time. Start the stack and apply the migrations, or build with `SQLX_OFFLINE=true` to use the committed `.sqlx/` cache.
