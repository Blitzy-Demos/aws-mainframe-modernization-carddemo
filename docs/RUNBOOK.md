# CardDemo Runbook

This runbook installs, configures, runs and tests the Rust backend and React frontend that replace the CardDemo COBOL application, on one machine with Docker. It is written for an operator who has never seen the repository. Run every command from the repository root unless a step says otherwise, and run the steps in the order given.

## 1. Overview

The system has six parts:

- **PostgreSQL 16.15** holds all application data.
- **RabbitMQ 3.13.7** (the `management` image) carries the three message flows.
- **`api`** (crate `carddemo-api`) is the REST server for the online transactions.
- **`mq`** (crate `carddemo-messaging`) runs the RabbitMQ consumers for the CDRD date, CDRA account and CP00 authorization flows.
- **`batch`** (crate `carddemo-batch`) is a command-line program with one subcommand per batch job, plus `migrate` and `seed`.
- **The single-page application** in `web/` is served by the Vite dev server on `http://localhost:5173`, the browser's only entry point.

The COBOL tree under `app/` is the read-only parity oracle that the new code is proven against. Never edit, move, delete or reformat anything under `app/cbl`, `app/cpy`, `app/bms`, `app/jcl`, `app/proc`, `app/asm`, `app/csd`, `app/ctl`, `app/maclib`, `app/scheduler`, `app/data/ASCII` or `app/data/EBCDIC`, and do not edit `README.md`. The fixture files in `app/data/ASCII` are the seed input.

`docs/parity/` holds the parity evidence: `parity-oracle-audit.md` (the audited COBOL rules and fixture expectations), `validation-audit.md` (every input validation and its message), `function-map.md` (each 3270 key and field, and the web control that replaces it), `escalations.md` (every anomaly and decision) and `signoff.md` (the acceptance checklist).

The system runs in one of two modes. The browser uses `http://localhost:5173` in both.

| Mode | PostgreSQL and RabbitMQ | `api`, `mq`, `batch`, SPA | Sections |
|---|---|---|---|
| Host-toolchain mode | Docker Compose | `cargo` and `pnpm` on the host | 4 to 7 |
| Everything-in-Compose mode | Docker Compose | Docker Compose profile `app` | 8 |

## 2. Requirements

Install each tool at the version shown, then run its check. Every version is exact except Docker's, which needs only the Compose v2 plugin.

| Tool | Version | Why |
|---|---|---|
| Rust (via rustup) | 1.88.0, with `clippy`, `rustfmt`, `llvm-tools-preview` | Backend toolchain, pinned by `rust-toolchain.toml` |
| C build toolchain | `build-essential` on Debian or Ubuntu | Rust crates link native code |
| Node.js | 22.23.4 | Frontend runtime (`web/package.json` requires `22.x`) |
| pnpm (via Corepack) | 9.15.9 | Frontend package manager (`packageManager: pnpm@9.15.9`) |
| Docker Engine | With the Compose plugin, v2 or later | PostgreSQL, RabbitMQ and the integration tests |
| sqlx-cli | 0.8.6 | Migrations and offline query metadata |
| cargo-deny | 0.20.2 | Supply-chain check |
| cargo-llvm-cov | 0.9.1 | Coverage report |

Rust 1.88.0 is the minimum, not a preference: the patched `time` crate (0.3.47 or later, the fix for RUSTSEC-2026-0009) that `jsonwebtoken`, `printpdf` and `lapin` pull in needs Rust 1.88, so 1.85 to 1.87 cannot build the workspace (escalation E-12).

### 2.1 Rust

Install the pinned toolchain with its three components. `rust-toolchain.toml` then selects it for every `cargo` command inside the repository.

```bash
rustup toolchain install 1.88.0 --profile minimal --component clippy --component rustfmt --component llvm-tools-preview
rustc --version
```

Success: `rustc --version` prints `rustc 1.88.0 (6b00bc388 2025-06-23)`.

### 2.2 C build toolchain

Install a C compiler and linker. On Debian or Ubuntu:

```bash
sudo apt-get install -y build-essential
cc --version
```

Success: `cc --version` prints a GCC or Clang version. Without it every Rust build fails with ``linker `cc` not found``.

### 2.3 Node.js and pnpm

Install Node.js 22.23.4 from nodejs.org or with a Node version manager, then let Corepack provide pnpm 9.15.9. `corepack install -g` downloads that pnpm once and makes it the default, so `pnpm --dir web …` run from the repository root uses it too. With a system-wide Node.js installation, run the two `corepack` commands with `sudo`.

```bash
node --version
corepack enable
corepack install -g pnpm@9.15.9
pnpm --version
```

Success: `node --version` prints `v22.23.4` and `pnpm --version` prints `9.15.9`. `web/.npmrc` sets `engine-strict=true`, so `pnpm install` refuses any Node.js outside `22.x`.

### 2.4 Docker

Install Docker Engine with the Compose plugin (`docker compose`, not the legacy `docker-compose` script) and start the daemon.

```bash
docker compose version
docker info --format '{{.ServerVersion}}'
```

Success: the first command prints `Docker Compose version v2…` or later, and the second prints the daemon version rather than a connection error.

### 2.5 Rust tools

Install the three cargo tools at their pinned versions. cargo-llvm-cov 0.9.1 needs Rust 1.87 or later, which 1.88.0 satisfies.

```bash
cargo install sqlx-cli --version 0.8.6 --no-default-features --features postgres,rustls --locked
cargo install cargo-deny --version 0.20.2 --locked
cargo install cargo-llvm-cov --version 0.9.1 --locked
cargo sqlx --version
cargo deny --version
cargo llvm-cov --version
```

Success: the checks print `sqlx-cli-sqlx 0.8.6`, `cargo-deny 0.20.2` and `cargo-llvm-cov 0.9.1`.

### 2.6 Optional: the PostgreSQL client

The database checks below use `psql`. Install it with `sudo apt-get install -y postgresql-client`, or use the `docker compose exec postgres psql …` form each section also shows, which needs nothing on the host.

The first Rust build downloads about 480 crates, the first `pnpm install` downloads the frontend packages, and the Playwright step downloads Chromium, so those steps need network access.

## 3. Configuration

The `api`, `mq` and `batch` binaries read their settings only from environment variables, through `crates/runtime/src/config.rs`. Every variable that is not set takes its default, and the defaults equal the values in the committed template `.env.example`.

| Variable | Default | Used for |
|---|---|---|
| `CARDDEMO_DATABASE_URL` | `postgres://carddemo:carddemo@localhost:5432/carddemo` | The database `api`, `mq` and `batch` connect to |
| `DATABASE_URL` | `postgres://carddemo:carddemo@localhost:5432/carddemo` | Read only at compile time, by the sqlx `query!` macros and `cargo sqlx prepare` |
| `CARDDEMO_AMQP_URL` | `amqp://carddemo:carddemo@localhost:5672/%2f` | The broker `mq` connects to (vhost `/`) |
| `CARDDEMO_HTTP_ADDR` | `127.0.0.1:8080` | The address `api` listens on |
| `CARDDEMO_JWT_SECRET` | `local-dev-secret-change-me` | The HS256 secret of the sign-on tokens |
| `CARDDEMO_JWT_TTL_SECS` | `3600` | Token lifetime in seconds, a positive integer |
| `CARDDEMO_QUEUE_DATE_REQUEST` | `CARD.DEMO.REQUEST.DATE` | CDRD request queue |
| `CARDDEMO_QUEUE_ACCT_REQUEST` | `CARD.DEMO.REQUEST.ACCT` | CDRA request queue |
| `CARDDEMO_QUEUE_PAUTH_REQUEST` | `CARD.DEMO.REQUEST.PAUTH` | CP00 request queue |
| `CARDDEMO_STATEMENT_DIR` | `./var/statements/` | Where `batch creastmt` writes the statements |
| `CARDDEMO_BATCH_DIR` | `./var/batch/` | Where the batch generation files are written and read |
| `CARDDEMO_FIXTURE_DIR` | `app/data/ASCII` | The fixture files `batch seed` loads |
| `CARDDEMO_CORS_ORIGIN` | `http://localhost:5173` | The one browser origin `api` admits through CORS |
| `CARDDEMO_API_PROXY_TARGET` | `http://127.0.0.1:8080` | Read by `web/vite.config.ts`: where the dev server proxies `/api/*` |
| `CARDDEMO_CLOCK_OVERRIDE` | unset (the system clock) | An optional RFC 3339 instant, such as `2022-07-01T12:00:00Z`, that pins every time-dependent output: processing timestamps, CDRD date and time, reward accrual time and soft-delete time |
| `RUST_LOG` | `info` | Filter of the JSON log lines |

These tool variables are not in `.env.example`:

| Variable | Default | Used for |
|---|---|---|
| `SQLX_OFFLINE` | unset | `true` compiles the SQL against the committed `.sqlx/` metadata instead of a live database (section 10) |
| `E2E_BASE_URL` | `http://localhost:5173` | Read by `web/playwright.config.ts`: the address the end-to-end tests open |
| `RUSTFLAGS` | unset | `-D warnings` in the hygiene gates (section 11) |

Relative directories resolve against the current directory, which is why every command runs from the repository root. Generated output under `./var/` is git-ignored and never committed.

### 3.1 Load the settings into your shell

Export the defaults, then your own `.env` if you have one, into every shell that runs `cargo`, `pnpm`, `psql` or `curl`. The commands in this runbook refer to the settings as `${VARIABLE}`, so they need them exported.

```bash
set -a
. ./.env.example
[ -f .env ] && . ./.env
set +a
export SQLX_OFFLINE=true
```

Success: `echo "${CARDDEMO_HTTP_ADDR}"` prints the listen address.

`SQLX_OFFLINE=true` is needed at least until the database is migrated (section 5). With `DATABASE_URL` set and `SQLX_OFFLINE` unset, the sqlx macros check every query against the live schema at compile time, and they fail against a database that has not been migrated yet.

To change a value, copy the template, edit the copy, and load the settings again as above. `.env` is git-ignored; `.env.example` stays the committed template.

```bash
cp .env.example .env
```

Every binary checks every setting when it starts. An unusable value makes `batch` exit with status 16 and makes `api` or `mq` print `api: configuration: …` or `mq: configuration: …` and exit with status 1.

### 3.2 Overrides in the Compose `app` profile

The defaults above serve processes started on the host. The `app` profile of `docker-compose.yml` replaces these values with Compose service names, and leaves every other `CARDDEMO_*` setting at its default:

| Service | Overridden settings |
|---|---|
| `api` | `CARDDEMO_DATABASE_URL=postgres://carddemo:carddemo@postgres:5432/carddemo`, `CARDDEMO_AMQP_URL=amqp://carddemo:carddemo@rabbitmq:5672/%2f`, `CARDDEMO_HTTP_ADDR=0.0.0.0:8080`, `SQLX_OFFLINE=true` |
| `mq` | `CARDDEMO_DATABASE_URL` and `CARDDEMO_AMQP_URL` as for `api`, `SQLX_OFFLINE=true` |
| `web` | `CARDDEMO_API_PROXY_TARGET=http://api:8080` |

### 3.3 Credentials and logs

All credentials are local configuration for a development machine: the PostgreSQL and RabbitMQ user `carddemo` with password `carddemo`, and the JWT secret above. Nothing outside this machine is needed to build, test or run the system. `infra/rabbitmq/definitions.json` stores the RabbitMQ password only as a hash.

`api`, `mq` and `batch` write only JSON lines to standard output: log events and finished tracing spans (`"type": "span"`). `RUST_LOG` filters them, for example `RUST_LOG=debug`. `batch` writes to standard error only the lines that report an abend, a halt or a setup failure.

## 4. Services: PostgreSQL and RabbitMQ

Both services are defined in `docker-compose.yml` and publish their ports on `127.0.0.1` only: PostgreSQL on 5432, AMQP on 5672 and the RabbitMQ management console on 15672. Host-toolchain mode starts only these two; section 8 starts everything.

### 4.1 Start the services

Start PostgreSQL 16.15 and RabbitMQ 3.13.7 in the background.

```bash
docker compose up -d postgres rabbitmq
docker compose ps
```

Success: within about 30 seconds `docker compose ps` shows both containers `Up … (healthy)`. Add `--wait` to the first command to have it block until both are healthy.

### 4.2 Check the broker

RabbitMQ imports `infra/rabbitmq/definitions.json` at every boot, as `infra/rabbitmq/rabbitmq.conf` (`load_definitions`) directs. The file declares the vhost `/`, the user `carddemo` and six durable queues, and replaces RabbitMQ's default `guest` user. List the queues to confirm the import.

```bash
docker compose exec rabbitmq rabbitmqctl list_queues name durable messages
```

Success: the list holds `CARD.DEMO.REQUEST.DATE`, `CARD.DEMO.REQUEST.ACCT`, `CARD.DEMO.REQUEST.PAUTH`, `CARD.DEMO.REPLY.DATE`, `CARD.DEMO.REPLY.ACCT` and `CARD.DEMO.ERROR`, each `true` for durable.

The management console is at `http://localhost:15672`. Sign in as user `carddemo` with the password that `CARDDEMO_AMQP_URL` carries (its local default is in section 3). The `rabbitmq-data` volume keeps the messages on the durable queues across restarts.

### 4.3 Check the database

Ask PostgreSQL whether it accepts connections, from the host or from inside the container.

```bash
psql "${CARDDEMO_DATABASE_URL}" -c 'select version()'
docker compose exec postgres pg_isready -U carddemo -d carddemo
```

Success: `psql` prints `PostgreSQL 16.15 …`, and `pg_isready` prints `accepting connections`.

### 4.4 Stop or reset the services

Stop the containers and keep their data, or remove the containers, or remove the data as well.

```bash
docker compose stop
docker compose down
docker compose down -v
```

Success: `docker compose ps` lists nothing after `down`. `down -v` also deletes the `postgres-data` and `rabbitmq-data` volumes, so the next start needs section 5 again. Once the `app` profile has run (section 8), name it as well, `docker compose --profile app down -v`, or its build volumes stay behind.

## 5. Database

The `batch` binary creates the schema and loads the fixture data. The first `cargo run` compiles the workspace, which takes a few minutes; later runs start at once. Each batch subcommand reports its result as its exit status: print it with `echo $?` straight after the command.

### 5.1 Create the schema

Apply the migrations `migrations/0001_reference_data.sql` to `migrations/0005_rewards.sql` to the database at `${CARDDEMO_DATABASE_URL}`. This replaces the CREADB21 job.

```bash
cargo run --bin batch -- migrate
echo $?
```

Success: exit status 0. Running it again on a migrated database changes nothing and also exits 0.

### 5.2 Load the fixture data

On a first start, load the master files. `--if-empty` does nothing when the `accounts` table already holds rows, so it is safe to repeat.

```bash
cargo run --bin batch -- seed --if-empty
echo $?
```

Success: exit status 0. The seed reads the fixture files in `${CARDDEMO_FIXTURE_DIR}`, the 10 user rows of `app/jcl/DUSRSECJ.jcl` (each password stored as an argon2id hash) and the Db2 load files `app/app-transaction-type-db2/ctl/DB2LTTYP.ctl` and `DB2LTCAT.ctl`, and inserts the two seed reward rules. It replaces the README jobs ACCTFILE, CARDFILE, CUSTFILE, XREFFILE, DISCGRP, TCATBALF, TRANTYPE, TRANCATG and DUSRSECJ. The transaction master starts empty.

To refresh the data later, run `seed` without options. In one database transaction it clears the reward accruals, balances and rules, reloads every master file and the processing copies `trantype_file` and `trancatg_file` from the fixtures, and re-inserts the seed reward rules. The Db2 masters `transaction_types` and `transaction_categories` are loaded only while both are empty, so admin edits to them survive a refresh, and the transaction master keeps its rows.

```bash
cargo run --bin batch -- seed
```

Success: exit status 0.

### 5.3 Post the fixture day

Post the 300 daily transactions of `app/data/ASCII/dailytran.txt`. This is the POSTTRAN job, and it also creates the seed reward accruals and balances through the reward engine.

```bash
cargo run --bin batch -- posttran
echo $?
```

Success: exit status **4**, not 0, because 38 of the 300 records are rejected exactly as the COBOL rejects them; 262 are posted. The rejects are in the generation file `DALYREJS.G0001V00` under `${CARDDEMO_BATCH_DIR}` (the number grows by one per run). `posttran` requires an empty live transaction master: a second run exits 16 until `batch tranbkp` has emptied it (section 9).

### 5.4 Check the counts

Count the rows of every seeded table.

```bash
psql "${CARDDEMO_DATABASE_URL}" -c "
  select 'accounts' as t, count(*) from accounts
  union all select 'cards', count(*) from cards
  union all select 'card_xref', count(*) from card_xref
  union all select 'customers', count(*) from customers
  union all select 'disclosure_groups', count(*) from disclosure_groups
  union all select 'transaction_category_balances', count(*) from transaction_category_balances
  union all select 'transaction_types', count(*) from transaction_types
  union all select 'trantype_file', count(*) from trantype_file
  union all select 'transaction_categories', count(*) from transaction_categories
  union all select 'trancatg_file', count(*) from trancatg_file
  union all select 'users', count(*) from users
  union all select 'reward_rules', count(*) from reward_rules
  union all select 'transaction_master', count(*) from transaction_master"
```

Success: accounts, cards, card_xref and customers 50 each; disclosure_groups 51; transaction_types and trantype_file 7 each; transaction_categories and trancatg_file 18 each; users 10; reward_rules 2. Two counts change with `posttran`: transaction_category_balances is 50 before it and 100 after it, because posting creates each category balance record it does not find, as CBTRN02C does, and transaction_master is 0 before it and 262 after it. Without `psql` on the host, run the same query through `docker compose exec postgres psql -U carddemo -d carddemo -c "…"`.

## 6. Backend

Start each long-running process in its own terminal, after loading the settings there (section 3.1). Both run in the foreground; Ctrl-C stops them.

### 6.1 REST API

Start the API server.

```bash
cargo run -p carddemo-api --bin api
```

Success: a JSON line with the message `api listening` and the bound address `${CARDDEMO_HTTP_ADDR}`. A start-up failure is one line on standard error, `api: <stage>: <error>`, and exit status 1. Ctrl-C or SIGTERM drains the requests in flight and exits 0.

From another terminal, sign on as the seeded administrator to prove the API and the database work together.

```bash
curl -s -X POST "http://${CARDDEMO_HTTP_ADDR}/auth/login" \
  -H 'Content-Type: application/json' \
  -d '{"user_id":"ADMIN001","password":"PASSWORD"}'
```

Success: a JSON body with `token`, `expires_at`, `"user_id":"ADMIN001"`, `"usr_type":"A"` and `"landing":"/admin"`. The API has no health endpoint; sign-on is the check.

### 6.2 Message consumers

Start the RabbitMQ consumers for CDRD (`${CARDDEMO_QUEUE_DATE_REQUEST}`), CDRA (`${CARDDEMO_QUEUE_ACCT_REQUEST}`) and CP00 (`${CARDDEMO_QUEUE_PAUTH_REQUEST}`). Each takes one message at a time (prefetch 1), publishes its reply as a persistent message, and acknowledges the request only after the broker confirms the reply.

```bash
cargo run -p carddemo-messaging --bin mq
```

Success: three JSON lines with the message `consumer started`, one per request queue. Confirm them from another terminal:

```bash
docker compose exec rabbitmq rabbitmqctl list_consumers
```

Success: one consumer on each of the three request queues. `mq` does not reconnect by itself: when the broker restarts, `mq` exits with status 1 and must be started again. A stop signal leaves the delivery in flight unsettled, and the broker delivers it again to the next consumer.

### 6.3 Building without a database

With `SQLX_OFFLINE=true` (section 3.1) every crate compiles its SQL against the committed `.sqlx/` metadata and needs no running database. Once the database is migrated, you may `unset SQLX_OFFLINE` so the macros check every query against the live schema; section 10 needs that whenever a query or a migration changes.

## 7. Frontend

### 7.1 Install the packages

Install the exact package versions of `web/pnpm-lock.yaml`.

```bash
pnpm --dir web install --frozen-lockfile
```

Success: pnpm reports the packages added and exits 0. It fails rather than change the lockfile.

### 7.2 Start the dev server

Start Vite in the foreground. It serves the SPA on port 5173 and proxies every `/api/*` request to `${CARDDEMO_API_PROXY_TARGET}`, so the API from section 6.1 must be running.

```bash
pnpm --dir web dev
```

Success: Vite prints `Local: http://localhost:5173/`. It exits at once if port 5173 is taken (`strictPort`).

### 7.3 Sign on

Open `http://localhost:5173` and sign on with a user of `app/jcl/DUSRSECJ.jcl`:

| User ID | Password | Type | Lands on |
|---|---|---|---|
| `ADMIN001` | `PASSWORD` | Administrator (`A`) | `/admin`, the admin menu |
| `USER0001` | `PASSWORD` | User (`U`) | `/`, the main menu |

Sign-on upper-cases the typed user ID and password, as COSGN00C does, so `admin001` and `password` also work. A password an administrator stores in lower or mixed case therefore never signs on, as in the COBOL. Only `A` users see and reach the Administration section; the API answers 403 to a `U` user on every `/admin` endpoint.

The session token and the light or dark theme live only in the browser's `sessionStorage`: closing the tab signs out, and nothing persists beyond the session.

## 8. Everything in Compose

The `app` profile runs `api` and `mq` on `rust:1.88.0-bookworm` and the SPA on `node:22.23.4-bookworm`, next to `postgres` (`postgres:16.15`) and `rabbitmq` (`rabbitmq:3.13.7-management`). The source tree is bind-mounted, so the containers build and run the checked-out code; Compose builds no images. Stop any host-mode `api` or Vite first, because both modes publish ports 8080 and 5173.

### 8.1 Start the stack

Start all five services in the background, then follow the API log.

```bash
docker compose --profile app up -d
docker compose --profile app logs -f api
```

Success: after the first compile inside the container, which takes several minutes, the log shows the JSON line `api listening`; stop following it with Ctrl-C. `docker compose --profile app ps` then lists `postgres` and `rabbitmq` as healthy and `api`, `mq` and `web` as up. `api` and `mq` are restarted after a non-zero exit, so here `mq` comes back by itself after a broker restart.

### 8.2 Migrate and seed

Run the batch steps of section 5 inside a one-off container of the `api` service, which already holds the Compose database URL.

```bash
docker compose --profile app run --rm api cargo run --bin batch -- migrate
docker compose --profile app run --rm api cargo run --bin batch -- seed --if-empty
docker compose --profile app run --rm api cargo run --bin batch -- posttran
```

Success: exit statuses 0, 0 and 4, as in section 5. While the `api` container is still compiling, the first command waits with `Blocking waiting for file lock`, because both share one build volume.

Any other batch subcommand of section 9 runs the same way:

```bash
docker compose --profile app run --rm api cargo run --bin batch -- tranbkp
```

The command exits with the subcommand's own status. Its `./var/` output lands in the checkout's `./var/` through the bind mount.

### 8.3 Use and stop the stack

Open `http://localhost:5173`, the only entry point, and sign on as in section 7.3. The API is also published on `127.0.0.1:8080` for `curl`.

Stop the stack and keep its data and build volumes, or remove all five named volumes (`postgres-data`, `rabbitmq-data`, `cargo-target`, `cargo-registry`, `web-node-modules`) as well.

```bash
docker compose --profile app down
docker compose --profile app down -v
```

Success: `docker compose --profile app ps` lists nothing. After `down -v` the next start compiles from scratch and needs section 8.2 again.

## 9. Batch

### 9.1 The README batch sequence as subcommands

The README section "Running Batch Jobs" [README.md:L213-233] lists the JCL jobs of a full batch cycle. Each job with business logic is a `batch` subcommand; the infrastructure jobs CLOSEFIL, OPENFIL, TRANIDX, WAITSTEP, DEFGDGB, DEFGDGD and ESDSRRDS have no equivalent. In README order:

| README job | Subcommand | Exit status on the fixture cycle |
|---|---|---|
| ACCTFILE, CARDFILE, XREFFILE, CUSTFILE, TRANCATG, TRANTYPE, DISCGRP, TCATBALF, DUSRSECJ | `seed` | 0 |
| TRANBKP (first) | `tranbkp` | 12 when the live transaction master is empty, as on a fresh seed; otherwise 0 |
| TRANEXTR (optional, Db2 module) | `seed --only tranextr` | 0 |
| POSTTRAN | `posttran` | 4 (38 records rejected) |
| INTCALC | `intcalc` | 0 |
| TRANBKP (second) | `tranbkp` | 0 |
| COMBTRAN | `combtran` | 0 |
| CREASTMT | `creastmt` | 0 |
| CBPAUP0J (optional, IMS-DB2-MQ module) | `cbpaup0j` | 0 |

Run the cycle one command at a time and compare each exit status with the table. Do not chain the commands with `&&`: 4 and 12 are expected results, not failures.

```bash
cargo run --bin batch -- seed
cargo run --bin batch -- tranbkp
cargo run --bin batch -- seed --only tranextr
cargo run --bin batch -- posttran
cargo run --bin batch -- intcalc
cargo run --bin batch -- tranbkp
cargo run --bin batch -- combtran
cargo run --bin batch -- creastmt
cargo run --bin batch -- cbpaup0j
```

Success: each exit status matches the table, and the statements of section 9.2 exist under `${CARDDEMO_STATEMENT_DIR}`. Skip `seed --only tranextr` and `cbpaup0j` when you do not need the optional modules. `batch --help` and `batch <subcommand> --help` list what each job reads, writes and exits with.

### 9.2 Each subcommand

Every default below is the value the JCL passes, so a subcommand without options runs the job as the repository submits it.

Post a daily transaction file. The default is the fixture copy of the dataset in the POSTTRAN `DALYTRAN` DD [app/jcl/POSTTRAN.jcl:L30-31]. The live transaction master must be empty, so run `tranbkp` first on a database that holds transactions.

```bash
cargo run --bin batch -- posttran --input app/data/ASCII/dailytran.txt
```

Success: exit 0 with no reject, or 4 with rejects. The run updates accounts, category balances and reward balances, one database transaction per posted record, and writes `DALYREJS(+1)`: 430-byte reject records (the 350-byte input record and an 80-byte reason trailer), each followed by LF.

Compute the monthly interest. `--parm-date` is the JCL `PARM` [app/jcl/INTCALC.jcl:L22], exactly 10 characters, and prefixes the TRAN-ID of every interest transaction.

```bash
cargo run --bin batch -- intcalc --parm-date 2022071800
```

Success: exit 0. The run writes the interest transactions to `SYSTRAN(+1)`, and adds the interest to the current balance and zeroes the cycle credit and debit of every account in the category balance file but the last, which CBACT04C never rewrites (escalation E-14).

Back up the live transaction master, then empty it.

```bash
cargo run --bin batch -- tranbkp
```

Success: exit 0, or 12 when the master was already empty (the empty backup is still written). The backup is `TRANSACT.BKUP(+1)`. Transactions that reward accruals reference stay stored outside the live master, so the reward history is kept. While the live master is empty, the transaction list is empty, "Add transaction" (CT02) refuses every add with `Transaction ID NOT found...` and "Bill payment" (CB00) refuses every payment with `Unable to lookup Transaction...`, as COTRN02C and COBIL00C do; `posttran` or `combtran` fills it again.

Combine the newest backup `TRANSACT.BKUP(0)` and the newest interest file `SYSTRAN(0)`, sorted by TRAN-ID, into `TRANSACT.COMBINED(+1)`, and reload the live master from it.

```bash
cargo run --bin batch -- combtran
```

Success: exit 0. A TRAN-ID that is already live is not loaded and keeps its row, as the IDCAMS REPRO without `REPLACE` does: the run then ends with the REPRO condition code, 8 after one to three duplicate keys, or 12 once REPRO stops at its limit of four.

Produce the account statements.

```bash
cargo run --bin batch -- creastmt
```

Success: exit 0 and three files under `${CARDDEMO_STATEMENT_DIR}`, replacing the previous ones: `STATEMNT.PS` (80-byte text records), `STATEMNT.HTML` (100-byte records) and `STATEMNT.PS.PDF` (the text records as PDF, one line per record). The statements are batch output only; the web application does not serve them. More than 51 cards or 10 transactions per card stops the run with exit 16 (`CAPACITY STOP`) before any file is written, because the COBOL result is undefined there.

Report the transactions processed within a date range. The defaults are the `SYMNAMES` dates [app/jcl/TRANREPT.jcl:L43-44].

```bash
cargo run --bin batch -- tranrept --start-date 2022-01-01 --end-date 2022-07-06
```

Success: exit 0, or 12 when the live master is empty (every step still runs). The run writes `TRANSACT.BKUP(+1)`, `TRANSACT.DALY(+1)` (the records within the range, sorted by card number) and `TRANREPT(+1)` (the report, 133-byte lines). Reports read the processing copies `trantype_file` and `trancatg_file`, never the Db2 masters, so a transaction type edited on the admin screens or by `mnttrdb2` appears in reports only after `seed --only tranextr`. The same report is available online on the Reports screen (CR00).

Purge the expired pending authorizations. `--sysin` is the JCL `SYSIN` card [app/app-authorization-ims-db2-mq/jcl/CBPAUP0J.jcl:L36-37]: expiry days, checkpoint frequency, checkpoint display frequency and debug flag.

```bash
cargo run --bin batch -- cbpaup0j --sysin "00,00001,00001,Y"
```

Success: exit 0 and the expired authorizations deleted.

Maintain the Db2 transaction-type master `transaction_types` from an input file. Each record is a type `X(1)` (`A` add, `U` update, `D` delete, `*` comment), a type code `X(2)` and a description `X(50)`, 53 bytes; shorter lines are padded with spaces. `crates/batch/tests/fixtures/mnttrdb2_input.txt` shows the layout; it is test input and deliberately holds failing records. This writes a one-record file that adds type `10`, then applies it:

```bash
mkdir -p ./var
printf '%-53s\n' 'A10INSTALLMENT' > ./var/mnttrdb2_input.txt
cargo run --bin batch -- mnttrdb2 --input ./var/mnttrdb2_input.txt
```

Success: exit 0, or 4 when some records failed: each failed record is logged and changes nothing, and the others are committed. `--input` has no default, because the JCL reads a dataset the repository does not hold. Only the Db2 master changes, and a `seed` refresh keeps the change; run `seed --only tranextr` to copy it to the processing copies the reports read, and remove the example type again with a `D10` record.

Copy the Db2 masters `transaction_types` and `transaction_categories` over the processing copies `trantype_file` and `trancatg_file`. This is the TRANEXTR job.

```bash
cargo run --bin batch -- seed --only tranextr
```

Success: exit 0. Nothing else changes.

### 9.3 Generation files

A generation data group becomes numbered files under `${CARDDEMO_BATCH_DIR}`, named `<DATASET>.G<nnnn>V00`, for example `DALYREJS.G0001V00`. `(+1)` creates the next number, `(0)` reads the newest, and only the newest 5 generations of each dataset are kept, as `LIMIT(5) SCRATCH` defines [app/jcl/DALYREJS.jcl:L26]. A generation is cataloged only when its step ends normally, so an abend or a halt leaves the previous generation current. Record files use the copybook layouts at full width, each record followed by LF.

List the generations:

```bash
ls -l "${CARDDEMO_BATCH_DIR}"
```

Success: one file per cataloged generation, such as `DALYREJS.G0001V00` and `TRANSACT.BKUP.G0002V00`, at most 5 per dataset.

### 9.4 Exit statuses

| Status | Meaning |
|---|---|
| 0 | Success |
| 2 | Usage error, reported on standard error; nothing runs |
| 4 | Records rejected (`posttran`) or failed (`mnttrdb2`); the run completed |
| 8 | `posttran` only: reward-engine halt. Standard error names the record number and TRAN-ID. That record is not posted, the records before it stay posted, and nothing is added to `DALYREJS` |
| 16 | Abend: the COBOL `DISPLAY` lines on standard error, then `ABEND 999` where the COBOL passes that code (`posttran`, `intcalc`, `tranrept`), with the file status where the COBOL displays one. Also a `creastmt` capacity stop, a `combtran` equivalence halt and every setup failure (an unreachable database, an invalid setting, an unreadable input) |
| 8, 12 | IDCAMS REPRO condition codes, with no other meaning: `tranbkp` and `tranrept` end with 12 on an empty live master, `combtran` with 8 or 12 after duplicate keys |

### 9.5 Reproducible timestamps

The processing timestamps that `posttran` and `intcalc` stamp, the CDRD date and time, reward accrual times and soft-delete times come from the system clock unless `CARDDEMO_CLOCK_OVERRIDE` pins them. `tranrept` filters on the processing date, so with its default range (2022-01-01 to 2022-07-06) it reports the fixture day only when `posttran` ran under a clock inside that range; under today's clock the report is empty.

Pin the clock to the instant the parity tests use, restore the seeded state, post the fixture day and report it, then return to the system clock:

```bash
export CARDDEMO_CLOCK_OVERRIDE=2022-07-01T12:00:00Z
cargo run --bin batch -- seed
cargo run --bin batch -- tranbkp
cargo run --bin batch -- posttran
cargo run --bin batch -- tranrept
unset CARDDEMO_CLOCK_OVERRIDE
```

Success: exit statuses 0, then 0 (or 12 on an empty master), then 4, then 0. Every posted record carries a 2022-07-01 processing timestamp, and the newest `TRANREPT.G<nnnn>V00` under `${CARDDEMO_BATCH_DIR}` lists the posted transactions with their page, account and grand totals.

### 9.6 Lock files

A job that allocates a generation holds the lock file `.<DATASET>.lock` in `${CARDDEMO_BATCH_DIR}`, and `creastmt` holds `.STATEMNT.lock` in `${CARDDEMO_STATEMENT_DIR}`. A second job that needs the same lock exits 16 at once instead of waiting. A job killed with SIGKILL or by a power loss leaves its lock file behind; remove it by hand, and only while no batch job is running.

## 10. API types and query metadata

### 10.1 OpenAPI document and TypeScript types

After any handler or DTO change, regenerate the OpenAPI document from the API and the TypeScript types from the document. The first command needs no database, broker or settings.

```bash
cargo run -p carddemo-api --bin openapi -- --out web/openapi/openapi.json
pnpm --dir web gen:api
```

Success: both exit 0; `web/openapi/openapi.json` and `web/src/api/schema.d.ts` are rewritten. Commit both files.

Check that neither generated file is stale:

```bash
pnpm --dir web gen:api:check
cargo test -p carddemo-api --test openapi_spec --locked
```

Success: both pass. `gen:api:check` fails when `schema.d.ts` differs from what `openapi.json` generates; `openapi_spec` fails when `openapi.json` differs from what the API builds. `pnpm --dir web typecheck` then fails on any page that uses a removed or renamed field.

### 10.2 sqlx offline metadata

`.sqlx/` holds the query metadata that lets the workspace compile with `SQLX_OFFLINE=true`. After a query or a migration changes, apply the migrations to the database `${DATABASE_URL}` names, regenerate the metadata against it, and confirm there is no drift. `-- --all-targets` includes the queries in test sources.

```bash
unset SQLX_OFFLINE
sqlx migrate run --source migrations
cargo sqlx prepare --workspace -- --all-targets
cargo sqlx prepare --workspace --check -- --all-targets
```

Success: the migration step reports each new migration as applied, `prepare` rewrites `.sqlx/`, and `--check` exits 0. Commit `.sqlx/`. Drift detection needs a live, migrated database; without `DATABASE_URL` it cannot run. `export SQLX_OFFLINE=true` again afterwards if you build without the database.

## 11. Testing

### 11.1 What each suite needs

| Suite | Command | Needs |
|---|---|---|
| Audit structure check of `docs/parity/` | `cargo test -p carddemo-audit --test audit_structure --locked` | Rust and a C linker |
| Domain rules, newtype compile-fail cases, reward engine, manifest boundary | `cargo test -p carddemo-domain --locked` | Rust and a C linker |
| ASCII and EBCDIC codec round trips | `cargo test -p carddemo-codec --locked` | Rust and a C linker |
| JSON log and span output | `cargo test -p carddemo-runtime --locked` | Rust and a C linker |
| Hygiene scans and Compose tags | `cargo test -p carddemo-testkit --locked` | Rust and a C linker |
| Persistence | `cargo test -p carddemo-persistence --locked` | Docker: testcontainers starts `postgres:16.15` |
| REST API (`openapi_spec` needs only Rust) | `cargo test -p carddemo-api --locked` | Docker: `postgres:16.15` |
| Batch jobs and parity expectations | `cargo test -p carddemo-batch --locked` | Docker: `postgres:16.15` |
| Message flows | `cargo test -p carddemo-messaging --locked` | Docker: `rabbitmq:3.13.7-management` with `infra/rabbitmq/*` mounted, and `postgres:16.15` |
| Frontend unit and form tests | `pnpm --dir web test` | Node.js and pnpm |
| End-to-end | `pnpm --dir web e2e` | The running stack and Chromium (section 11.4) |

The Docker-based suites start and remove their own containers; they do not use the Compose services of section 4. Run each suite from the repository root with the settings loaded (section 3.1).

Run one suite, for example the domain rules:

```bash
cargo test -p carddemo-domain --locked
```

Success: `test result: ok` for every test binary of the crate.

### 11.2 Hygiene gates

Run the full Rust gate: formatting, a warning-free build, clippy, every test, the supply-chain policy of `deny.toml` and the coverage report. `RUSTFLAGS` is exported once so that every command shares one build cache; with it exported, `cargo build` and `cargo test` here are the gate commands `RUSTFLAGS="-D warnings" cargo build --workspace --all-targets --locked` and `RUSTFLAGS="-D warnings" cargo test --workspace --locked`. The test step needs Docker.

```bash
export RUSTFLAGS="-D warnings"
cargo fmt --all -- --check
cargo build --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check
cargo llvm-cov --workspace --lcov --output-path target/lcov.info
```

Success: every command exits 0 and prints no warning. `cargo deny check` prints `advisories ok, bans ok, licenses ok, sources ok`; add `--disable-fetch` to reuse the local advisory database when GitHub is unreachable. `cargo llvm-cov` writes `target/lcov.info`; no coverage threshold applies.

Also run the drift check of section 10.2 (`cargo sqlx prepare --workspace --check -- --all-targets`, with `SQLX_OFFLINE` unset and `DATABASE_URL` naming a migrated database), and confirm the domain crate pulls in no database library:

```bash
cargo tree -p carddemo-domain -e normal | grep -c sqlx
```

Success: `0`.

### 11.3 Frontend checks

Type-check, run the Vitest suite, and build.

```bash
pnpm --dir web typecheck
pnpm --dir web test
pnpm --dir web build
pnpm --dir web gen:api:check
```

Success: every command exits 0 and prints no warning. If `vite build` reports a chunk above its 500 kB warning limit, split that chunk further in `web/vite.config.ts` (`manualChunks`); raising `chunkSizeWarningLimit` would hide the warning and is not allowed.

### 11.4 End-to-end tests

Download the Chromium build that Playwright 1.64.1 drives, with its operating-system libraries (this step uses `sudo apt-get` on Debian or Ubuntu).

```bash
pnpm --dir web exec playwright install --with-deps chromium
```

Success: Playwright reports Chromium downloaded.

The tests need the whole host-mode stack: the Compose services (section 4), a migrated and seeded database with the fixture day posted (sections 5.1, 5.2 and 5.3), the API (section 6.1) and Vite (section 7.2). The test fixtures change rows through `docker compose exec postgres psql`, run from the repository root in the Compose project that `COMPOSE_PROJECT_NAME` and `COMPOSE_FILE` name when set, so the API under test must use that Compose PostgreSQL. Every fixture restores the rows it changed.

The posted fixture day is required. The first seed leaves the live transaction master empty, while the specs list and page existing transactions and add transactions and bill payments, which "Add transaction" and "Bill payment" refuse on an empty master (section 9.2). Count the transactions in the live master, through the same `psql` route the fixtures use:

```bash
docker compose exec postgres psql -U carddemo -d carddemo -tA -c 'select count(*) from transaction_master'
```

Success: one number. `0` means the master is empty, as after a fresh seed: post the fixture day once.

```bash
cargo run --bin batch -- posttran
echo $?
```

Success: exit status **4**, the correct result: 38 of the 300 records are rejected and 262 are posted (section 5.3). Do not chain the command with `&&`, because 4 is not a failure. A count above `0` means the prerequisite is already met: do not run `posttran` again, because on a master that holds transactions it abends with exit 16 and changes nothing.

To return a database that earlier runs or manual changes have altered to the seeded and posted state, refresh the seed, empty the live master and post the fixture day again, one command at a time:

```bash
cargo run --bin batch -- seed
cargo run --bin batch -- tranbkp
cargo run --bin batch -- posttran
```

Success: exit statuses 0, then 0 (or 12 when the master was already empty), then 4, and the count above prints `262`.

The specs take the processing clock to be `CARDDEMO_CLOCK_OVERRIDE` when it is set, and the database clock when it is not: they predict the reward accrual of each transaction they add at that instant, and start each rule they create and then soft-delete on its UTC day, because the API stamps accruals and soft deletes with its own processing clock. Export the same value in the shell that starts the API and in the shell that runs Playwright, or leave it unset in both, its default; the reward tests fail with a message that names `CARDDEMO_CLOCK_OVERRIDE` when the API stamped an accrual at another clock. A pinned instant must lie inside a seed reward rule's window: the retail rule is active from 2022-01-01T00:00:00Z, so `2022-07-01T12:00:00Z` serves. Post the fixture day under the same pin, as in section 9.5, because the reward history lists the latest accrual time first, and accruals posted under a later clock would sort above the ones the specs add.

Run the specs:

```bash
pnpm --dir web e2e
```

Success: every spec passes in both projects, `md-768x1024` and `lg-1280x800`. The specs run one at a time against `${E2E_BASE_URL}` (default `http://localhost:5173`), and a failure leaves its trace and screenshot in `web/test-results/` and the HTML report in `web/playwright-report/`, both git-ignored.

### 11.5 Startup smoke test in both modes

Prove each operating mode starts: the `@smoke` test signs on as `ADMIN001` and loads the dashboard. Run it once with the host-mode stack of sections 4 to 7 running, and once with the Compose stack of section 8 running instead.

```bash
pnpm --dir web exec playwright test e2e/navigation.spec.ts --grep @smoke
```

Success: the test passes in both projects, in both modes.

### 11.6 Protected paths and lockfiles

Prove that the COBOL oracle is untouched since the starting commit `59cc6c2`, list every change, and confirm both lockfiles are tracked.

```bash
git diff --stat 59cc6c2 -- app/cbl app/cpy app/bms app/jcl app/proc app/asm app/csd app/ctl app/maclib app/scheduler app/data/ASCII app/data/EBCDIC
git diff --name-status 59cc6c2
git ls-files Cargo.lock web/pnpm-lock.yaml
```

Success: the first command prints nothing; the second lists only `A` (added) lines for the new paths and `M .gitignore`; the third prints `Cargo.lock` and `web/pnpm-lock.yaml`.

### 11.7 Recording the results

Record each check, the command that ran it and its result in `docs/parity/signoff.md`. A suite that could not run, for example without Docker or a C linker, is recorded as unverified, never as passed.

## 12. Pitfalls

- **No C linker.** Every Rust build fails with ``linker `cc` not found``. Install `build-essential` (section 2.2).
- **No Docker daemon.** The persistence, API, batch and messaging tests fail when they start their containers. Start Docker and check it with `docker info`.
- **sqlx and the database.** With `DATABASE_URL` set and `SQLX_OFFLINE` unset, the `query!` macros check every query against the live schema at compile time, and fail when the database is down or not migrated. The macros also read `DATABASE_URL` from a `.env` file in the repository root on their own, even when it is not exported. Use `SQLX_OFFLINE=true` when no migrated database is up.
- **Edited migration.** Once a migration is applied, editing its file makes `batch migrate` and `sqlx migrate run` fail with `was previously applied but has been modified`. Stop `api` and `mq`, recreate the development database with `sqlx database drop -y && sqlx database create && sqlx migrate run --source migrations` (it acts on the database `DATABASE_URL` names), then repeat sections 5.2 and 5.3.
- **Settings not exported.** The binaries read only the environment, never `.env` itself. Load the settings into every new shell (section 3.1).
- **Busy ports.** 5432, 5672, 15672, 8080 and 5173 must be free. Host mode and Compose mode use the same ports, so stop one mode before starting the other. Vite exits at once rather than move to another port.
- **No `guest` user.** The definitions file replaces RabbitMQ's default `guest` user and `/` vhost; connect as `carddemo`.
- **Changing `infra/rabbitmq/definitions.json`.** The broker imports the file at every boot, so `docker compose restart rabbitmq` applies added or changed users, permissions and queues. The import never deletes anything: removing a queue or user needs a fresh broker data volume, which `docker compose down -v` gives at the cost of the database as well (repeat section 5).
- **`seed` without `--if-empty` is a refresh.** It clears every reward accrual, balance and rule and reloads the fixtures, but keeps the transaction master. To regenerate the seed accruals, run `tranbkp` and then `posttran`.
- **`posttran` needs an empty live master.** Run on a master that holds transactions, it abends with exit 16. Run `tranbkp` first.
- **Empty live master online.** After a fresh seed or a `tranbkp`, "Add transaction" and "Bill payment" refuse to write, as the COBOL does, until `posttran` or `combtran` fills the master.
- **Empty transaction report.** `tranrept` and the Reports screen filter on the processing date `posttran` stamped. Under today's clock the default 2022 range reports nothing; pin `CARDDEMO_CLOCK_OVERRIDE` before `posttran` (section 9.5).
- **Admin transaction-type edits and reports.** Edits on the admin screens and by `mnttrdb2` change only the Db2 masters; reports show them only after `seed --only tranextr`.
- **Stale batch lock files.** A batch job killed with SIGKILL leaves `.<DATASET>.lock` or `.STATEMNT.lock` behind, and later jobs exit 16 until it is removed, which is safe only while no batch job runs (section 9.6).
- **`mq` on the host does not reconnect.** After a broker restart it exits with status 1; start it again. In Compose mode it is restarted automatically.
- **CP00 and RabbitMQ direct reply-to.** CP00 answers no requester whose `reply_to` is `amq.rabbitmq.reply-to`: RabbitMQ returns a mandatory publish to that pseudo-queue as unroutable while still delivering it, so `mq` logs a warning and the `M004` record, rejects the request without requeue and writes no authorization. Name a reply queue of your own, for example a server-named exclusive queue.
- **End-to-end tests against another database.** The fixtures write through `docker compose exec postgres psql`, so an API pointed at any other PostgreSQL fails the specs.
- **Changing `RUSTFLAGS` rebuilds everything.** Keep it identical across `build`, `test`, `clippy` and `llvm-cov`, for example exported once as in section 11.2.
- **Lockfiles.** Never run `cargo update` or a `pnpm install` without `--frozen-lockfile`; the gates build with `--locked` against the committed `Cargo.lock` and `web/pnpm-lock.yaml`.
- **Disk space.** `target/` grows to several GB, and `cargo llvm-cov` builds a second tree in `target/llvm-cov-target`; remove that directory after a coverage run, or run `cargo clean` to free everything.
- **Compose volumes.** `docker compose down -v` without `--profile app` leaves the `cargo-target`, `cargo-registry` and `web-node-modules` volumes behind; use `docker compose --profile app down -v` to remove them too.
