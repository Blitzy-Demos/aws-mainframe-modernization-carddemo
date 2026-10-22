# CardDemo replacement

This directory replaces the CardDemo CICS/VSAM/IMS/Db2/MQ application with a Rust workspace of eight crates, a PostgreSQL 16 database, a RabbitMQ 3.13 broker and a React 19 + shadcn/ui single-page application, all of which build, test and run on the local machine. The COBOL tree under [`../app/`](../app/) is not retired: it is the permanent parity oracle, read in place, byte-unchanged and still compilable on a mainframe, and nothing in this directory writes to it. The COBOL application itself is described in the root [README.md](../README.md#application-inventory), whose Application Inventory (lines 265-326) lists its transactions, maps, programs and jobs.

## Status

Done means outcomes o1 to o7 hold and every item of [docs/SIGNOFF.md](docs/SIGNOFF.md) is checked (AAP 0.11.4). A test that passes here is not a signed-off parity result:

- Halts H1–H6 in [audit/HALTS.md](audit/HALTS.md) are `BLOCKED`. No parity is asserted for those cases, the bindings they list cannot pass, and Done cannot be reached until each one is `RESOLVED` from source or `DESCOPED` by the user.
- E29, CP00 replying before its database writes as COPAUA0C does, awaits its own named and dated signature in [audit/README.md](audit/README.md#e29--cp00-reply-before-write). The general escalation review never satisfies it.
- Code that consumes an audit file counts only once that file is `Accepted` in the [review status table](audit/README.md#review-status) (a9).
- The latest recorded run of every check, with its evidence and its blocked or unverified parts, is in [docs/SIGNOFF.md](docs/SIGNOFF.md).

## Prerequisites

Install commands and platform notes are in [docs/SETUP.md](docs/SETUP.md#1-prerequisites). No cloud account, SDK or credential is used at any step.

- Docker Engine with the Compose v2 plugin (`docker compose`).
- A C toolchain that provides `cc`, for linking and procedural macros.
- rustup: the first cargo command in this directory installs Rust 1.85.0 from [`rust-toolchain.toml`](rust-toolchain.toml).
- `sqlx-cli` 0.8.6, `cargo-deny` 0.18.9 and `cargo-llvm-cov` 0.6.21. cargo-deny is 0.18.9 rather than 0.18.3, because 0.18.3 cannot read the CVSS 4.0 advisories of the current RustSec database.
- Node.js 22 ([`frontend/.nvmrc`](frontend/.nvmrc)) and pnpm 9.15.9.
- Playwright's Chromium, for the NAV suite only.

## Quick start

Every command runs from `modern/`.

1. Create the local configuration, then set `CARDDEMO_JWT_SECRET` in `.env` to any local value; the API refuses to start while it is empty:

   ```bash
   cp .env.example .env
   ```

2. Start the stack, apply the schema and load the fixtures:

   ```bash
   scripts/dev-up.sh
   ```

   It runs `docker compose up -d --wait`, then `carddemo-migrate`, then the loads `acctfile cardfile xreffile custfile discgrp tcatbalf trancatg trantype dusrsecj`. That is the refresh order of [`../scripts/remote_refresh.sh`](../scripts/remote_refresh.sh) (lines 16-44) less TRANFILE, whose only input is EBCDIC. Rerunning it reloads the same records.

3. Start the API in a shell with `.env` exported:

   ```bash
   set -a; . ./.env; set +a
   cargo run -p api --bin carddemo-api
   ```

4. Optional: start the CDRD, CDRA and CP00 workers in another shell, after the same export:

   ```bash
   cargo run -p messaging --bin carddemo-mq
   ```

5. Start the frontend in another shell, after the same export. The dev server listens on `http://127.0.0.1:5173` (or `CARDDEMO_WEB_PORT`) and proxies `/api` to `CARDDEMO_API_BIND`:

   ```bash
   pnpm --dir frontend install --frozen-lockfile && pnpm --dir frontend dev
   ```

6. Sign on as `ADMIN001` (admin menu) or `USER0001` (main menu) with the password `PASSWORD`, the fixture users the root [README.md](../README.md) publishes at lines 203-204.

Only `carddemo-migrate` applies migrations. `carddemo-api`, `carddemo-mq` and `carddemo-batch` never migrate, so against an unmigrated database they fail at their first query.

## Command index

Commands run from `modern/` in a shell where `.env` is exported; the scripts export it themselves.

### Binaries

| Command | What it does |
|---|---|
| `cargo run -p runtime --bin carddemo-migrate` | Applies `crates/persistence/migrations/`, the only path by which the schema changes; a rerun applies nothing |
| `cargo run -p api --bin carddemo-api` | Serves the 27 operations under `/api/v1` on `CARDDEMO_API_BIND`, and the contract at `/api-docs/openapi.json` |
| `cargo run -p api --bin carddemo-openapi` | Prints the OpenAPI document; needs no database, broker or environment |
| `cargo run -p messaging --bin carddemo-mq` | Runs the CDRD, CDRA and CP00 workers on the three request queues |
| `cargo run -p batch --bin carddemo-batch -- <subcommand>` | Runs one of the 15 batch jobs below; `--help` in place of the subcommand lists them |

The 15 `carddemo-batch` subcommands, one per in-scope JCL job:

| Kind | Subcommands |
|---|---|
| Loads, each replacing one table | `dusrsecj`, `acctfile`, `cardfile`, `custfile`, `xreffile`, `discgrp`, `tcatbalf`, `trancatg`, `trantype` |
| Jobs | `posttran`, `intcalc`, `creastmt`, `tranrept`, `cbpaup0j`, `mnttrdb2` |

A job that writes files prints each path it wrote, one per line. Outputs stay on the local filesystem under `var/` (`CARDDEMO_VAR_DIR`, `CARDDEMO_STATEMENT_DIR`), which is git-ignored; statements, reports and rejects hold full card numbers, so never commit or share them. Options, defaults and outputs per subcommand are in [docs/SETUP.md](docs/SETUP.md#6-batch-jobs).

### Scripts

| Command | What it does |
|---|---|
| `scripts/dev-up.sh` | Starts PostgreSQL and RabbitMQ, runs `carddemo-migrate`, then the nine loads |
| `scripts/check.sh` | Runs every check of AAP 0.9.3, steps 0–17, and stops at the first failure. It enforces the line-coverage floors: 80 % over the workspace, 90 % over `copybook` and `domain`, 70 % over the frontend. Without Docker it stops after step 12 with exit status 2 and names the bindings left unverified |
| `scripts/regen-api.sh` | Regenerates `openapi/openapi.json` and `frontend/src/api/schema.d.ts`, then fails when either differs from the committed file |
| `scripts/e2e.sh` | NAV: recreates the stack, loads the fixtures, runs `posttran`, applies the pending-authorization seed, starts the API and the dev server, runs Playwright, then removes the stack with `docker compose down -v` |
| `scripts/protected-diff.sh` | Fails on any change to the COBOL tree or to any other path outside `modern/` |
| `node scripts/frontend-policy.mjs` | Fails on a frontend dependency missing from `frontend/approved-dependencies.json`, or a hand-written type that restates a generated API type |

`dev-up.sh`, `regen-api.sh`, `e2e.sh` and `protected-diff.sh` print their usage with `--help`. `check.sh` ignores its arguments and runs the whole pipeline, and `frontend-policy.mjs` reads its first argument as the directory to check in place of `modern/`, so pass neither of them `--help`.

### Frontend

| Command | What it does |
|---|---|
| `pnpm --dir frontend dev` | Vite dev server, proxying `/api` to `CARDDEMO_API_BIND` |
| `pnpm --dir frontend build` | Production build into `frontend/dist/` |
| `pnpm --dir frontend typecheck` | Strict TypeScript check |
| `pnpm --dir frontend test` | Vitest, once, with coverage; fails below 70 % line coverage |
| `pnpm --dir frontend e2e` | Playwright NAV suite against an API and dev server already running; `scripts/e2e.sh` prepares both |
| `pnpm --dir frontend gen:api` | Generates `src/api/schema.d.ts` from `../openapi/openapi.json` |

### Build and stack

| Command | What it does |
|---|---|
| `SQLX_OFFLINE=true cargo build --workspace --locked` | Builds without a database, against the committed `.sqlx/` |
| `cargo test -p <crate> --locked` | One crate's tests, with `SQLX_OFFLINE=true` when the stack is not running. `copybook` and `domain` need no Docker; `persistence`, `api`, `messaging`, `batch` and `test-support` start their own test containers and remove them afterwards |
| `docker compose up -d --wait` | Starts PostgreSQL on `localhost:5432` and RabbitMQ on `localhost:5672`, with its management console at `http://localhost:15672` |
| `docker compose down [-v]` | Stops the stack; `-v` also removes the database and broker volumes, with every loaded row and every queued message |

## Layout

| Path | Holds |
|---|---|
| [`crates/`](crates/) | The eight Rust crates |
| [`frontend/`](frontend/) | The React 19, Vite 6 and shadcn/ui application: 21 screens, generated API types, Vitest and Playwright suites |
| [`audit/`](audit/) | Derive-lane audit tables, the escalation log and the halt report |
| [`docs/`](docs/) | Setup, architecture, API and sign-off documents |
| [`scripts/`](scripts/) | The scripts of the command index |
| [`rabbitmq/`](rabbitmq/) | Broker configuration and definitions: vhost, user and the seven durable queues |
| [`openapi/`](openapi/) | `openapi.json`, the contract `carddemo-openapi` emits; committed |
| [`.sqlx/`](.sqlx/) | sqlx offline query metadata, derived from the migrations by `cargo sqlx prepare`; never edited by hand |
| [`compose.yaml`](compose.yaml) | The two local services, `postgres:16` and `rabbitmq:3.13-management`, on loopback ports |
| [`Cargo.toml`](Cargo.toml), [`Cargo.lock`](Cargo.lock) | Workspace manifest with every dependency version and lint, and the committed lockfile |
| [`rust-toolchain.toml`](rust-toolchain.toml), [`rustfmt.toml`](rustfmt.toml), [`deny.toml`](deny.toml) | Toolchain pin, formatting and dependency policy |
| [`.env.example`](.env.example), [`.gitignore`](.gitignore) | Every configuration variable with its local default; ignore rules for build output, `var/` and `.env` |

The crates are `copybook` (PIC codecs), `domain` (records, money types and every business rule, with no I/O and no sqlx), `persistence` (migrations, repositories and the only SQL), `runtime` (configuration, telemetry, the database pool and `carddemo-migrate`), the binary crates `api`, `messaging` and `batch`, and the dev-only `test-support`. Dependencies point one way: `api`, `messaging` and `batch` depend on `domain`, `persistence` and `runtime`, `persistence` depends on `domain`, `domain` depends on `copybook`, and `runtime` depends on no workspace crate ([docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#2-crate-graph)).

## Further reading

- [docs/SETUP.md](docs/SETUP.md): environment setup, configuration variables, every service and the build and test commands.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): crate graph, layering rules, identity, data reach and messaging flows.
- [docs/API.md](docs/API.md): the 27 endpoints and the transactions and programs they replace.
- [docs/SIGNOFF.md](docs/SIGNOFF.md): the g8 acceptance checklist and its evidence.
- [audit/README.md](audit/README.md): audit conventions, the review status table and the E29 signature.
- [audit/ESCALATIONS.md](audit/ESCALATIONS.md): the escalation log.
- [audit/HALTS.md](audit/HALTS.md): the halt report.
