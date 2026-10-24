# 1. Executive Summary

## 1.1 Project Overview

This project replaces the AWS CardDemo mainframe application (COBOL/CICS, VSAM, IMS, Db2, MQ and JCL under `app/`) with a local-only stack under `modern/`. An eight-crate Rust 1.85 workspace serves 27 REST endpoints and runs three RabbitMQ workers and 15 batch jobs over PostgreSQL 16, and a React/Vite frontend renders all 21 BMS screens. A 3,509-rule audit holds behaviour to COBOL parity, and `scripts/check.sh` enforces the user's rules. It is built for teams retiring the mainframe who need a verified functional twin.

## 1.2 Completion Status

```mermaid
%%{init: {"theme": "base", "themeVariables": {"pie1": "#5B39F3", "pie2": "#FFFFFF", "pieStrokeColor": "#B23AF2", "pieOuterStrokeColor": "#B23AF2", "pieTitleTextColor": "#B23AF2"}}}%%
pie showData title 90.6% Complete
    "Completed Work" : 2560
    "Remaining Work" : 266
```

| Metric | Value |
|---|---|
| Total Hours | 2,826 |
| Completed Hours (AI + Manual) | 2,560 (2,560 + 0) |
| Remaining Hours | 266 |
| Percent Complete | 90.6% |

2,560 hours completed out of 2,826 total hours = 90.6% complete (2,560 ÷ 2,826 × 100).

## 1.3 Key Accomplishments

- ✅ `scripts/check.sh` passes all 18 steps on the delivered tree.
- ✅ 2,311 Rust and 2,278 Vitest tests pass with none failing; line coverage is 95.51% workspace, 98.73% copybook+domain, 98.72% frontend.
- ✅ 27 REST operations match the committed `openapi.json` and the generated `schema.d.ts`.
- ✅ The CDRD, CDRA and CP00 workers keep the i10 and i11 invariants against a live broker and database.
- ✅ 15 batch subcommands; POSTTRAN rejects 38 of 300 fixtures and exits 4.
- ✅ Playwright reaches all 21 screens through on-screen controls for both user types.
- ✅ 3,445 of 3,509 audit rules are satisfied by tagged tests, and 64 are reported BLOCKED.
- ✅ Typed money arithmetic, argon2id/JWT authentication and cardholder-data redaction, each tested.

## 1.4 Critical Unresolved Issues

10 of the 13 `docs/SIGNOFF.md` items, the AAP's definition of Done, remain open; each needs a reviewer or user decision. By AAP requirement, 4 of 41 are open: R15 hygiene and the sign-off record are partial; reviewer acceptance and the i10 answer are not started.

| Issue | Impact | Owner | ETA |
|---|---|---|---|
| A2 per-criterion table (1 item): row 6, R15 hygiene, fails on suppressions and `unsafe` written by macro and derive expansions (Section 5.2, D1); 15 other rows await review or halts | Blocks sign-off | Tech lead + reviewer | ~12 h |
| C1, C3 (2 items): 39 audit files unaccepted; R7 labels unconfirmed | Parity unattested | Parity reviewer | ~66 h |
| C2, C4 (2 items): E29 unsigned; escalations E01–E56 all PROPOSED; 243 candidates not folded in | Departures unratified | Reviewer + product owner | ~44 h |
| C7 (1 item): halts H1–H6 BLOCKED; 33 reporting tests ignored | Six behaviours unspecified | Reviewer, then engineer | ~64 h |
| C5, C6, C8 (3 items): thin-handler and `TRUNCATION:` review; i10 scope answer; scope acknowledgements | Done unmet | Reviewer + user | ~11 h |
| B1 (1 item): fresh-machine run unchecked | Setup unattested | Engineer | ~2 h |

## 1.5 Access Issues

No access issues identified: the full gate, including the advisory download and the Docker-backed suites, ran end to end.

## 1.6 Recommended Next Steps

1. [High] Ratify the macro-expansion escalations E46–E56 and record the 30-enum `derive(PartialEq)` `unsafe`, clearing SIGNOFF row 6.
2. [High] Accept the 39 audit files and the 56 escalations, and sign E29.
3. [High] Resolve or descope halts H1–H6, and turn their 33 BLOCKED tests into assertions.
4. [High] Answer the i10 question, finish B1, C5 and C8, re-run the gate and record a sign-off commit that is in the delivered history.
5. [Medium] Provision production secrets, TLS and managed services, and review the nine ignored advisories.

# 2. Project Hours Breakdown

## 2.1 Completed Work Detail

| Component | Hours | Description |
|---|---|---|
| Workspace foundation and toolchain | 28 | Eight-crate Cargo workspace (edition 2024, resolver 3), pinned `Cargo.lock` and `pnpm-lock.yaml`, workspace lints (`unsafe_code` forbid, pedantic deny), `deny.toml`, `rust-toolchain.toml` |
| Copybook codec (`crates/copybook`) | 90 | PIC/COMP-3 field codec, record layouts, Codec A round trips of the ASCII fixtures and USRSEC records, total parsers over 0–1,024-byte input |
| Fixed-point arithmetic (`crates/domain`) | 64 | `Money<N>`, `Rate<N>`, `MoneyExpr`, `RateProduct` with store truncation; trybuild compile-fail cases; interest, bill-payment and category-balance parity |
| Online business rules (`crates/domain/src/online`) | 380 | Rules of the 22 online programs plus CSUTLDTC: edits, messages, paging, typed dates |
| Batch and messaging rules (`crates/domain`) | 170 | POSTTRAN, INTCALC, CREASTMT, TRANREPT, CBPAUP0J, MNTTRDB2 and loads; CODATE01, COACCT01 and COPAUA0C decision logic |
| Persistence (`crates/persistence`) | 190 | Six migrations (0001–0006) with seeded Db2 schema, repositories, 143-entry sqlx offline cache, Codec B, nine paginated query files, EXPLAIN tests |
| Runtime (`crates/runtime`) | 64 | Environment-only `Config`, JSON tracing and optional span export, `carddemo-migrate`, GDG generations, directory locks |
| REST API (`crates/api`) | 380 | 27 operations with per-program services, argon2id sign-on and HS256 JWT, role gate, OpenAPI document and `carddemo-openapi`, redaction, hand-written DTO serde |
| Messaging workers (`crates/messaging`) | 120 | `carddemo-mq` with CDRD, CDRA and CP00, confirmed publish before ack, dead-lettering, `SELECT … FOR UPDATE` credit limit (i10), liveness (i11) |
| Batch jobs (`crates/batch`) | 210 | `carddemo-batch` with 15 subcommands, GDG (+1) outputs, DALYREJS, statements as PS, HTML and PDF, TRNRPT job deck |
| Frontend (`frontend/`) | 320 | 21 screens in `ScreenShell`, seven shadcn components, zod schemas, typed hooks over generated types, PF-key binding, 43 Vitest files |
| Test harness and NAV (`crates/test-support`, `frontend/e2e`) | 80 | Testcontainers harness, audit index, trace capture, repository-naming check, Playwright NAV with pending-authorization seed |
| Parity audit corpus (`audit/`) | 210 | 39 audit files (copybooks, arithmetic, fixtures, screens, 23 online, 7 batch, 3 messaging), escalations E01–E56, halts H1–H6, 3,509 tagged rules |
| Pipeline, scripts and local runtime | 70 | `check.sh` (steps 0–17), `e2e.sh`, `dev-up.sh`, `regen-api.sh`, `protected-diff.sh`, `frontend-policy.mjs`, `compose.yaml`, RabbitMQ definitions with 7 durable queues |
| Documentation and sign-off automation | 44 | `README.md`, `docs/SETUP.md`, `docs/ARCHITECTURE.md`, `docs/API.md`, `docs/SIGNOFF.md` section A from gate output, `.env.example` |
| Integrated verification and hardening | 140 | Runtime validation of all endpoints, broker, batch and screens; concurrency, saturation, redaction and recovery probes |
| **Total** | **2,560** | |

## 2.2 Remaining Work Detail

| Category | Hours | Priority |
|---|---|---|
| Parity audit acceptance: review 39 audit files and 3,509 rules, confirm R7 labels (C1, C3) | 66 | High |
| Escalation decisions: E01–E56, 243 escalation candidates, E29 signature; ratify divergences D2, D4, D5 (C2, C4) | 44 | High |
| Halt resolution: decide H1–H6 and 90 halt candidates, implement, convert 33 BLOCKED tests (C7) | 64 | High |
| R15 hygiene: settle macro-expansion escalations and the 30-enum `derive(PartialEq)` `unsafe` (A2 row 6, D1) | 10 | High |
| Sign-off closure: B1 fresh-machine run, C5 review, C6 i10 answer, C8 acknowledgements, re-gate and record an in-history commit (D7) | 15 | High |
| Screen, endpoint and redaction parity items (D5, D6) | 14 | Medium |
| Concurrency and query-plan decisions: transaction-ID allocation, Db2 description filter, CP00 interim decline (D8) | 9 | Medium |
| Dependency advisories: exposure review and upgrade path for nine ignored advisories (D3) | 8 | Medium |
| Production environment: secrets for JWT, database and broker; amqps TLS; managed PostgreSQL/RabbitMQ; Firefox and Safari check | 20 | Medium |
| Load and resilience baseline for API, workers and batch | 10 | Low |
| Code and documentation hygiene: 23 positional `value_set_all` calls, 8 commit-failure doc passages, one `TRUNCATION:` comment, one `cargo doc` link | 6 | Low |
| **Total** | **266** | |

Halt resolution carries low confidence (range 40–90 h), because the six halts depend on behaviour the COBOL sources leave unspecified. All other estimates carry medium-to-high confidence.

# 3. Test Results

All figures come from one run of `scripts/check.sh` on the delivered tree. It exited 0 with "all steps 0-17 passed": 51 Rust test binaries and 43 Vitest files ran under coverage. The 33 ignored Rust tests are deliberate BLOCKED reporters for halts H1–H6 and their candidates, and they assert no behaviour.

| Area / Category | Framework | Tests | Passed | Failed | Coverage | What This Proves |
|---|---|---|---|---|---|---|
| Copybook codec and domain rules (`crates/copybook`, `crates/domain`) | cargo test, proptest, trybuild | 1,212 | 1,212 | 0 | 98.73% lines (floor 90%) | Fixture records round-trip byte for byte, and the online, batch and messaging rules and the i2 arithmetic match the audit |
| Persistence and runtime (`crates/persistence`, `crates/runtime`) | cargo test, testcontainers PostgreSQL 16 | 259 | 259 | 0 | Workspace 95.51% lines (floor 80%) | The six migrations build the declared schema, the queries plan on indexes, and configuration and telemetry behave as documented |
| REST API (`crates/api`) | cargo test, testcontainers | 518 (+24 ignored) | 518 | 0 | In workspace figure | All 27 operations, the role gate, page-size bounds, the OpenAPI contract and redaction hold against a live database |
| Messaging workers (`crates/messaging`) | cargo test, testcontainers RabbitMQ 3.13 | 88 (+9 ignored) | 88 | 0 | In workspace figure | CDRD, CDRA and CP00 reply, dead-letter and keep the i10 and i11 invariants against a live broker |
| Batch jobs (`crates/batch`) | cargo test, testcontainers | 163 | 163 | 0 | In workspace figure | 15 subcommands, GDG generations, DALYREJS, statements and the TRNRPT deck produce the audited outputs |
| Audit and rule harness (`crates/test-support`) | cargo test | 71 | 71 | 0 | In workspace figure | Every one of 3,509 audit rules carries a test tag or a BLOCKED halt, and repository methods follow the naming rule |
| Frontend units and screens (`frontend/src`) | Vitest 3.2.7 (jsdom, v8 coverage) | 2,278 | 2,278 | 0 | 98.72% lines (floor 70%) | Each of the 21 screens renders its map fields, PF keys and messages, and the zod schemas enforce the map edits |
| Navigation (`frontend/e2e/nav.spec.ts`) | Playwright 1.64.1 (Chromium) | 4 | 4 | 0 | — | Both user branches reach all 21 routes through on-screen controls, and USER0001 has no path to an admin screen |

The same run passed fmt, warning-free build and clippy, the source scans, cargo-deny (advisories, bans, licences, sources), the frontend policy and typecheck, contract regeneration, the protected-path diff and the sqlx cache check.

**Not Covered** (test these before release):

- The `amqps` (TLS) branch of the broker connector: every test uses plain AMQP.
- The non-Linux directory-lock path and macOS script variants (`flock`, Bash 3.2).
- Browsers other than Chromium, including the field-sizing fallback in Firefox and Safari.
- Defensive operating-system failures: runtime start-up, signal-handler installation, and an argon2 worker panic.
- Endpoint 4's customer-read failure (COACTVWC-046) and MNTTRDB2's deadlock (−911) path, which run only against test doubles.
- COBIL00 payment-step store failures, which only Vitest exercises.
- Halt paths H1–H6, which are deliberately left unasserted.
- Sustained load: no throughput or latency target is tested beyond functional concurrency probes.

# 4. Runtime Validation & UI Verification

The delivered tree was run on a local Compose stack (PostgreSQL 16 and RabbitMQ 3.13), with `carddemo-api`, `carddemo-mq`, `carddemo-batch` and the Vite dev server started from `modern/`.

- ✅ **Start-up and schema.** `scripts/dev-up.sh` brings both services up healthy. `carddemo-migrate` then applies 6 of 6 migrations, and the nine fixture loads complete: 50 accounts, cards, cross-references and customers, 51 disclosure groups, 50 category balances, 18 categories, 7 types and 10 users. The run takes about 14 s.
- ✅ **Authentication.** ADMIN001 signs on as type A (`/admin`) and USER0001 as type U (`/menu`). A request with no token gets 401 with exactly one `WWW-Authenticate: Bearer`, and a user token on an admin endpoint gets 403.
- ✅ **API contract.** The running API serves `/api-docs/openapi.json` with 22 paths and 27 operations, identical to the offline `carddemo-openapi` output. `page_size=101` is refused with 422.
- ✅ **Data formats.** Account view returns COBOL-edited amounts (for example `+      2,020.00`) and ISO dates.
- ✅ **Batch.** POSTTRAN processes 300 transactions, rejects 38, exits 4 and writes the next `DALYREJS.G<nnnn>` generation under `var/dalyrejs/`.
- ✅ **Messaging.** A CDRD request published to `CARD.DEMO.REQUEST.DATE` returns a 1,000-byte reply starting `SYSTEM DATE :`. On SIGTERM the workers drain and close the AMQP connection and the pool; API shutdown is equally clean.
- ✅ **User journeys.** The Chromium NAV suite drives both menu branches across all 21 routes through Vite and the live API. Browser checks confirm the wrong-password message, a redirect to sign-on after an invalid token with no loop, COTRN02 required-field messages, a COUSR02 user update confirmed by re-fetch, the COTRTUP no-change message, and a uniform 28 px field height.
- ✅ **Dev proxy.** Vite binds `CARDDEMO_WEB_PORT` in strict-port mode and proxies `/api`. When the API is stopped it answers 502 and logs the unreachable target by name.
- ⚠ **Transaction list on an empty table.** `GET /api/v1/transactions?entry_key=pf5` answers 500 while the transactions table is empty, and 200 once POSTTRAN has posted rows.
- ⚠ **Never exercised at runtime:** an `amqps` (TLS) broker, any non-local or multi-host deployment, Firefox and Safari, and CDRA and CP00 through a hand-started worker. CDRA and CP00 are exercised only by the container-backed test suites.

# 5. Compliance & Quality Review

## 5.1 Compliance Matrix

| Deliverable / Benchmark | Evidence | Status | Progress |
|---|---|---|---|
| Authority: create only mapped paths under `modern/` (R16) | 625 added files, none outside `modern/`; `scripts/protected-diff.sh` against `59cc6c2` OK | ✅ PASS | 100% |
| Build quality: fmt, warning-free build, clippy pedantic, no TODO/FIXME | `check.sh` steps 1–6; no markers in the tree | ✅ PASS | 100% |
| R15 Hygiene: zero `unsafe`, zero suppressions, generated code included | None in source; macro and derive expansions write them (D1) | ❌ FAIL | Escalated |
| Single Schema Executor and Bounded Pagination | Migration-macro scans, sqlx cache check, `explain.rs`, `pagination.rs`; 101 → 422 | ✅ PASS | 100% (caveat D8) |
| Coverage Floors (80 / 90 / 70) | 95.51% workspace, 98.73% copybook+domain, 98.72% frontend | ✅ PASS | 100% |
| Generated API Types Only and Single Component System | `regen-api.sh` match; `frontend-policy.mjs` pass; 7 shadcn components | ✅ PASS | 100% |
| Cardholder Data Redaction | `api/tests/redaction.rs`, `messaging/tests/redaction.rs` pass | ✅ PASS | Scope extended, one gap (D5) |
| Local-Only Runtime | Step 0 credential check; Compose runs only PostgreSQL and RabbitMQ | ✅ PASS | 100% |
| Dependency security and licences | `cargo deny check`: advisories, bans, licences, sources ok | ⚠ PASS | 9 advisories ignored (D3) |
| Checked Integer Narrowing and Thin Request Handlers | Step 6 scans pass | ⚠ PASS | Human review C5 open |
| Parity audit: automated proof (a8) and reviewer acceptance (a9) | `audit_coverage.rs`: 3,445 of 3,509 rules satisfied, 64 BLOCKED; 0 of 39 files accepted | ⚠ PARTIAL | a8 done, a9 not started |
| Sign-off checklist (g8) | `docs/SIGNOFF.md`: A1, A3, A4 checked | ⚠ OPEN | 3 of 13 items |

## 5.2 AAP & Rule Divergences and Gaps

| # | What the AAP/Rule Required | What Was Delivered Instead | Why It Diverged | Impact | Remediation |
|---|---|---|---|---|---|
| D1 | R15: zero `unsafe` and zero lint suppressions, generated code included | sqlx, utoipa, serde and test-harness expansions write suppressions; `derive(PartialEq)` writes `unsafe` on 30 enums | R4 mandates the sqlx macros; expansion output is outside the code's control | SIGNOFF row 6 FAIL; blocks sign-off | Ratify E46–E56; record or hand-write the 30 impls |
| D2 | AAP 0.3.4.1: every DTO derives serde `Serialize`/`Deserialize` | 78 hand-written serde impls in `crates/api/src/dto/` | **Sanctioned** by AAP 0.11.6: restructure when a derive breaks R15 (E53) | More code to maintain; contract unchanged | Ratify E53 |
| D3 | AAP 0.7.1: cargo-deny 0.18.3; no known advisories | cargo-deny 0.18.9; nine advisories ignored by ID | 0.18.3 cannot read CVSS 4.0 entries; fixes need Rust 1.88 or newer printpdf | Known-vulnerable transitive crates | Exposure review; MSRV upgrade plan |
| D4 | AAP 0.5.6 and 0.3.4.1: refuse an empty JWT secret; the listed error statuses | 32-byte secret minimum; 503 + `Retry-After` on password-work saturation and at the in-flight limit; `WWW-Authenticate: Bearer` not in `openapi.json` | Security hardening decided during delivery | Stricter configuration; extra status for clients | Ratify, or declare the header |
| D5 | AAP 0.3.4.3: MNTTRDB2 job log on stdout; redaction of cardholder data | Job log as JSON tracing events; wider redaction; short passwords in short echoed parts not recognised | One JSON stdout stream (0.3.4.7); gap not implemented in this run | Log consumers parse JSON; small exposure gap | Decide X-COACCT01-9 and X-COPAUA0C-11 |
| D6 | AAP 0.3.5: screens and endpoints reproduce map and COBOL behaviour | Eight screen or endpoint departures kept as is | Not implemented in this run | One edge-path 500; cosmetic gaps | Fix the eight items (~14 h) |
| D7 | AAP 0.9.5 and 0.11.4: section A from the gate at the sign-off commit; Done when g8 is fully checked | SIGNOFF records `587c10c`, which is not in the delivered history; 10 of 13 items unchecked | A squashed delivery commit cannot name itself; reviewer items need a human | Recorded commit absent from a fresh clone; Done unmet | Re-record against the release commit; finish the human items |
| D8 | Bounded Pagination plans on an index; i11 liveness; COBOL parity | Transaction ID highest + 1 not serialised; Db2 description filter may sequential-scan; interim CP00 decline during outages | Parity (COTRN02C-043); AAP 0.6.1 maps six migrations only; keeping i11 | Retryable error under concurrent adds; tiny table | Decide serialisation, index migration, interim decline |

**D1, R15 macro and derive output.** The source holds no `unsafe` and no `#[allow]`, but expanded code does. The sqlx query macros wrap each query in `#[allow(clippy::all)]`: 1,047 sites in the persistence library build, 1,062 in its test build and 80 in `schema.rs`. Utoipa, serde, tokio and the test harness add further suppressions (E46, E47, E50–E56 in `modern/audit/ESCALATIONS.md`), and rustc's `derive(PartialEq)` emits `unsafe` for 30 enums: 23 in domain, 6 in api and 1 in messaging. No escalation row covers those 30 enums. R4 requires the macros, so `docs/SIGNOFF.md:56` records row 6 as FAIL. A reviewer must ratify the escalations and either add a row for the derives or hand-write the 30 `PartialEq` impls.

**D2, hand-written DTO serialisation.** AAP 0.3.4.1 says DTOs derive serde, but `serde_derive` wraps every derive in a const block that carries lint suppressions (E53). AAP 0.11.6 orders exactly this response: escalate, then restructure the calling code. `crates/api/src/dto/*.rs` therefore holds 78 hand-written `Serialize` impls, with readers that accept JSON strings only, while `utoipa::ToSchema` stays derived. The published contract is unchanged, because `scripts/regen-api.sh` reproduces `openapi.json` and `schema.d.ts` byte for byte. The cost is maintenance: a field added to a DTO needs its serde impl edited by hand. No change is needed beyond the reviewer ratifying E53 under C4.

**D3, cargo-deny version and ignored advisories.** cargo-deny 0.18.3 stops at the first CVSS 4.0 advisory (`unsupported CVSS version: 4.0`), so the project uses 0.18.9 from the same line, as `docs/SETUP.md` §1 names. `deny.toml:16-23` ignores eight RustSec advisories by ID (E35–E42), and E48 covers a dev-only tinypool advisory. Each is held by an AAP pin or by MSRV 1.85. `time` 0.3.47 needs Rust 1.88, and the `lopdf` fix needs a newer printpdf. Exposure is narrow: `lopdf::Document::load_mem` reads only the PDF the job has just generated (`crates/batch/src/pdf.rs:141-142`). The owner should accept these with a dated review and plan the MSRV upgrade that clears them.

**D4, HTTP hardening beyond the AAP.** Five behaviours go beyond the AAP. `MIN_JWT_SECRET_BYTES = 32` (`crates/runtime/src/config.rs:89`) refuses short HS256 keys. Sign-on, user add and user update answer 503 with `Retry-After` when the argon2id budget is saturated, and a request that waits out the API's in-flight limit gets 503 `SERVER_BUSY` (`crates/api/src/error.rs:8-44`). Every 401 carries `WWW-Authenticate: Bearer` (`error.rs:292`), which `openapi.json` does not declare (`docs/API.md` §1). Request enums accept JSON strings only. These were security decisions taken during delivery, outside the AAP's status list. Clients must handle 503, and operators must supply a 32-byte secret. Ratify them, or declare the header in the contract.

**D5, job-log format and redaction scope.** MNTTRDB2's job log is emitted as one JSON tracing event per line (`crates/batch/src/jobs/mnttrdb2.rs:143`), because AAP 0.3.4.7 keeps stdout a single JSON stream. Operators read the `message` field rather than plain lines. Redaction is wider than the rule requires: CDRA masks a not-found key, CP00 withholds field 18 when it holds 8 or more bytes, and approved amounts of 10,000,000.00 or more are withheld. A short password inside a short echoed part is not recognised, because that case was not implemented in this run. Its candidates, X-COACCT01-9 and X-COPAUA0C-11, await the reviewer's decision, and the fix follows from it.

**D6, screen and endpoint parity items.** Eight departures were not changed in this run. COBIL00 leaves the balance blank after "You have nothing to pay...", where the COBOL shows it (X-COBIL00C-3). COPAU00 leaves Acct Status blank. COACTUP has busy Tab stops and shows an SSN placeholder before the fetch, and `/users/delete` shows a bare "F12" label. `GET /api/v1/transactions?entry_key=pf5` answers 500 on an empty table (Section 4), and a COTRN02 account-path 400 leaves Card # empty. Inputs clip one character at 1024 px on COUSR02 and COSGN00, and the COTRTLI page number overflows. `/signon` shows an empty message row after an invalid-token redirect. All eight are small fixes (Section 2.2).

**D7, sign-off record.** `docs/SIGNOFF.md:31` records commit `587c10c` as the gated tree. That tree equals the delivered one except for SIGNOFF.md itself, but the commit is not an ancestor of the delivered head, because a squashed delivery commit cannot name its own hash. A fresh clone cannot check it out. A gate run on the delivered head `e19feeb` reproduces SIGNOFF's figures exactly: 2,278 Vitest tests, 2,311 Rust tests, 95.51% and 98.73% lines, and NAV 4 of 4. Separately, A2, B1 and C1–C8 need a reviewer or a user answer by design, so AAP 0.11.4 Done is unmet. Re-record section A against the release commit and complete the human items.

**D8, accepted concurrency and plan behaviours.** COTRN02 assigns transaction IDs as highest + 1 without serialisation, as the COBOL does (COTRN02C-043, `crates/domain/src/online/cotrn02c.rs:193`). In a run of 10 concurrent adds, one succeeded and nine got "Tran ID already exist...". The Db2 type list's `LIKE '%…%'` description filter cannot use a B-tree index, so the cost planner may choose a sequential scan on that tiny table (at most 9,025 codes). No index migration was added, because AAP 0.6.1 maps six migrations. CP00 answers an interim 05/9000 decline during database outages longer than 30 s, which keeps i11. Each behaviour needs a decision: accept it, or serialise IDs, add a trigram index, or change the decline.

# 6. Risk Assessment

| Risk | Category | Severity | Probability | Mitigation | Status |
|---|---|---|---|---|---|
| Halts H1–H6 leave six COBOL behaviours unspecified, such as COCRDUPC commarea overrun, file error codes and unchecked numerics. Production can reach those paths with no asserted outcome | Technical | High | Medium | Resolve or descope each halt, then turn the 33 BLOCKED reporters into assertions | Open |
| Parity is unattested until a reviewer accepts the 39 audit files, 56 escalations and 333 candidate IDs. Behavioural drift from the mainframe could ship unnoticed | Operational | High | Medium | Complete C1–C4 before cut-over, and run the a9 parity review against `audit/` | Open |
| Eight transitive dependencies carry ignored RustSec advisories (`time` RFC 2822 stack exhaustion, `lopdf`, `tokio-tar`, `rustls-pemfile` and others), plus a dev-only tinypool advisory | Security | Medium | Low | Dated exposure review in `audit/ESCALATIONS.md` E35–E42 and E48; move to Rust 1.88 and a newer printpdf when the pins allow | Accepted |
| Production secrets and transport security are unproven: the `amqps` branch was never exercised, and the database, broker and JWT credentials are local placeholders | Integration | Medium | Medium | Provision secret storage and TLS endpoints, and test `amqps://` and TLS PostgreSQL before go-live | Open |
| Concurrent transaction adds collide on highest + 1 IDs: one succeeds and the rest get "Tran ID already exist..." | Technical | Medium | Medium | Accept as parity, or serialise allocation under an escalation | Accepted (parity) |
| During a database outage longer than 30 s, CP00 answers interim 05/9000 declines, and authorization decisions for an account being purged wait until the purge commits its current interval | Operational | Medium | Low | Alert on `carddemo::alert`; set operating procedures for outage windows | Accepted |
| No load or latency baseline exists. The Db2 description filter may sequential-scan, and POSTTRAN and CREASTMT memory grows with input size | Operational | Low | Medium | Run a load and soak test; add a trigram index if the table grows | Open |
| Expanded code still writes suppressions and `unsafe` (R15). Telemetry builds value sets positionally at 23 sites (`FieldSet::value_set_all`), with undefined results on a field-count mismatch | Technical | Low | Low | Ratify D1; move the 23 sites to `runtime::telemetry::with_value_set` | Open |

# 7. Visual Project Status

```mermaid
%%{init: {"theme": "base", "themeVariables": {"pie1": "#5B39F3", "pie2": "#FFFFFF", "pieStrokeColor": "#B23AF2", "pieOuterStrokeColor": "#B23AF2", "pieTitleTextColor": "#B23AF2"}}}%%
pie showData title Project Hours Breakdown
    "Completed Work" : 2560
    "Remaining Work" : 266
```

Remaining work by priority (266 hours, from Section 2.2):

```mermaid
%%{init: {"theme": "base", "themeVariables": {"pie1": "#B23AF2", "pie2": "#5B39F3", "pie3": "#A8FDD9", "pieStrokeColor": "#B23AF2", "pieOuterStrokeColor": "#B23AF2", "pieTitleTextColor": "#B23AF2"}}}%%
pie showData title Remaining Hours by Priority
    "High" : 199
    "Medium" : 51
    "Low" : 16
```

| Remaining category | Hours | Priority |
|---|---|---|
| Parity audit acceptance | 66 | High |
| Halt resolution | 64 | High |
| Escalation decisions | 44 | High |
| Sign-off closure | 15 | High |
| R15 hygiene | 10 | High |
| Production environment | 20 | Medium |
| Screen, endpoint and redaction parity items | 14 | Medium |
| Concurrency and query-plan decisions | 9 | Medium |
| Dependency advisories | 8 | Medium |
| Load and resilience baseline | 10 | Low |
| Code and documentation hygiene | 6 | Low |
| **Total** | **266** | |

# 8. Summary & Recommendations

The CardDemo replacement is 90.6% complete against the AAP: 2,560 of 2,826 hours delivered, with 266 remaining. Every engineering deliverable in the plan exists and is verified. The work covers eight Rust crates and five binaries, 27 REST operations, three broker workers, 15 batch jobs, 21 screens, six migrations, a 39-file parity audit and the full gate. On the delivered tree, `scripts/check.sh` passes all 18 steps with 2,311 Rust tests, 2,278 Vitest tests and 4 NAV journeys passing and none failing. Line coverage is 95.51% for the workspace, 98.73% for copybook+domain and 98.72% for the frontend, well above the 80/90/70 floors.

What remains is mostly human judgement, and the AAP's definition of Done makes it binding. Ten of the 13 sign-off items are open. Reviewers must accept 39 audit files covering 3,509 rules, decide 56 escalations and 333 candidate findings, sign E29, and resolve or descope the six halts whose 33 tests stand ignored as BLOCKED reporters. The user must also answer whether i10 binds the batch writers. The High-priority work, 199 of the 266 remaining hours, is almost entirely these reviewer and user decisions.

The critical path is: (1) ratify the R15 macro-expansion escalations, which clears the only failing row of SIGNOFF A2; (2) run the a9 parity review and escalation decisions in parallel; (3) resolve the halts and turn their tests into assertions; (4) re-run the gate and record a sign-off commit that is in the delivered history. Production readiness then needs environment work that the AAP's local-only scope leaves out: secret storage for the JWT, database and broker credentials, TLS for PostgreSQL and `amqps`, managed services, and a Firefox and Safari check.

Eight divergences are documented in Section 5.2. One is sanctioned by the AAP (hand-written DTO serde), and two block sign-off (R15 expansion output and the sign-off record). Five need an owner decision: the advisory waivers, the HTTP hardening, log format and redaction scope, eight screen parity items, and three concurrency or plan behaviours. None of them alters the committed OpenAPI document.

| Success metric | Target | Observed |
|---|---|---|
| Gate steps passing | 18 of 18 | 18 of 18 |
| Rust / Vitest / NAV tests failing | 0 | 0 / 0 / 0 |
| Workspace, copybook+domain, frontend line coverage | ≥80% / ≥90% / ≥70% | 95.51% / 98.73% / 98.72% |
| Audit rules satisfied or reported BLOCKED | 3,509 of 3,509 | 3,445 satisfied + 64 BLOCKED |
| Sign-off items checked | 13 of 13 | 3 of 13 |

**Production readiness:** ready for reviewer sign-off and staging, but not for production cut-over until the High-priority items in Section 2.2 are closed.

# 9. Development Guide

Every command runs from `modern/` unless stated otherwise. Nothing needs a cloud account. `docs/SETUP.md` is the long-form reference.

## 9.1 System Prerequisites

- Linux (verified) or macOS. On macOS, put GNU `bash` 4.2+, `coreutils` and `flock` ahead of the system tools on `PATH`.
- Docker Engine with the Compose v2 plugin (`docker compose`). Images: `postgres:16` and `rabbitmq:3.13-management`.
- A C toolchain providing `cc` (`build-essential` on Debian/Ubuntu).
- rustup. `rust-toolchain.toml` installs Rust 1.85.0 with clippy, rustfmt and llvm-tools-preview on first use.
- Node.js 22 (`frontend/.nvmrc`) and pnpm 9.15.9.
- Recommended: 8 CPU cores, 16 GB RAM and 20 GB of free disk. A full build plus coverage peaks at about 15 GB in `target/`.

## 9.2 Dependency Installation

```bash
cargo install --locked sqlx-cli@0.8.6 --no-default-features --features postgres,rustls
cargo install --locked cargo-deny@0.18.9
cargo install --locked cargo-llvm-cov@0.6.21
npm install -g pnpm@9.15.9
pnpm --dir frontend install --frozen-lockfile
pnpm --dir frontend exec playwright install --with-deps chromium
```

## 9.3 Environment Setup

Create `.env` from the template, then replace `CARDDEMO_JWT_SECRET` in it with a value of at least 32 bytes, such as the output of the `node` command below:

```bash
cp .env.example .env
node -e "console.log(require('node:crypto').randomBytes(32).toString('base64url'))"
set -a; . ./.env; set +a          # no binary loads .env by itself; the scripts do
```

- `.env` is git-ignored; never commit it.
- `PGOPTIONS` must be unset or blank, or every binary refuses to start.
- Cloud credential variables (`AWS_*`, `AZURE_*`, `GOOGLE_APPLICATION_CREDENTIALS` and similar) make `check.sh` step 0 fail. Run the gate with `env -u <NAME>`.

## 9.4 Application Startup

```bash
scripts/dev-up.sh                                    # compose up --wait, carddemo-migrate, nine fixture loads (~14 s)
cargo run -p api --bin carddemo-api                  # listens on CARDDEMO_API_BIND (default 127.0.0.1:8080); never migrates
cargo run -p messaging --bin carddemo-mq             # CDRD, CDRA, CP00 workers; stops cleanly on SIGTERM
pnpm --dir frontend dev                              # Vite on CARDDEMO_WEB_PORT (default 5173); proxies /api to the API
```

Start the API, the workers and Vite in separate shells, each with `.env` exported. To run them detached, use `nohup <command> > <log> 2>&1 & echo $!` and stop each one by its pid. Vite started through pnpm keeps running after pnpm exits, so stop it by the port owner: `kill "$(lsof -ti :5173 -sTCP:LISTEN)"`.

## 9.5 Verification Steps

```bash
curl -s -X POST http://127.0.0.1:8080/api/v1/signon -H 'content-type: application/json' \
  -d '{"user_id":"USER0001","password":"PASSWORD"}'           # → user_type "U", next_route "/menu"
curl -s -i http://127.0.0.1:8080/api/v1/accounts/00000000001 | head -n 1   # → HTTP/1.1 401 Unauthorized
curl -s http://127.0.0.1:8080/api-docs/openapi.json | head -c 200          # served contract
cargo run -p api --bin carddemo-openapi                       # the same document, offline
```

In a browser, open `http://127.0.0.1:5173/` and sign on with password `PASSWORD`. USER0001 lands on `/menu` with 11 options, and ADMIN001 on `/admin` with 6.

## 9.6 Example Usage

Run a batch job and list the subcommands:

```bash
cargo run -p batch --bin carddemo-batch -- posttran   # exits 4: 300 processed, 38 rejected → var/dalyrejs/DALYREJS.G<nnnn>
cargo run -p batch --bin carddemo-batch -- --help     # lists all 15 subcommands
```

With `carddemo-mq` running, send a CDRD request through the RabbitMQ management API (user `carddemo`, password `carddemo`), then read the 1,000-byte `SYSTEM DATE : …` reply:

```bash
curl -s -u carddemo:carddemo -H 'content-type: application/json' \
  -X POST 'http://127.0.0.1:15672/api/exchanges/%2f/amq.default/publish' \
  -d '{"properties":{"reply_to":"CARD.DEMO.REPLY.DATE"},"routing_key":"CARD.DEMO.REQUEST.DATE","payload":"DATE","payload_encoding":"string"}'
curl -s -u carddemo:carddemo -H 'content-type: application/json' \
  -X POST 'http://127.0.0.1:15672/api/queues/%2f/CARD.DEMO.REPLY.DATE/get' \
  -d '{"count":1,"ackmode":"ack_requeue_false","encoding":"auto"}'
```

## 9.7 Build and Test

```bash
SQLX_OFFLINE=true cargo build --workspace --all-targets --locked
SQLX_OFFLINE=true cargo clippy --workspace --all-targets --locked
SQLX_OFFLINE=true cargo test -p domain --locked                 # any crate; Docker-backed for api, messaging, batch, persistence
pnpm --dir frontend typecheck && node scripts/frontend-policy.mjs && pnpm --dir frontend test
scripts/e2e.sh                                                  # NAV: recreates the stack, starts API + Vite, runs Playwright, removes the stack
scripts/check.sh; rm -rf target/llvm-cov-target                 # full gate, steps 0-17, about 19 min
```

Set `RUST_TEST_THREADS=4` and `VITEST_MAX_FORKS=4` on shared hosts to bound parallel containers and workers.

## 9.8 Troubleshooting

- **`carddemo-api` exits at start.** `CARDDEMO_JWT_SECRET` is empty, blank or shorter than 32 bytes, or `.env` is not exported in that shell.
- **A binary fails at its first query.** The schema is not applied. Run `cargo run -p runtime --bin carddemo-migrate`.
- **`carddemo-migrate` reports "previously applied but has been modified".** The local volume holds an older schema. Run `docker compose down -v`, then `scripts/dev-up.sh`.
- **A sqlx macro fails at build.** `DATABASE_URL` points at an unmigrated database. Migrate it, or build with `SQLX_OFFLINE=true`.
- **Port already in use** (5432, 5672, 15672, 8080, 5173). Move the API and Vite with `CARDDEMO_API_BIND` and `CARDDEMO_WEB_PORT`, and remap the Compose ports with an override file named in `COMPOSE_FILE` (see `docs/SETUP.md` §9).
- **`e2e.sh` refuses to start.** The API or Vite port already answers. Stop your own API and Vite first.
- **Local data vanished after NAV.** `e2e.sh` removes the stack before and after. Re-run `scripts/dev-up.sh`.
- **`check.sh` stops at step 11 or 12.** Step 11: the handlers produce a different contract, so review and commit `openapi/openapi.json` and `frontend/src/api/schema.d.ts`. Step 12: a file outside `modern/` changed.
- **`cargo deny` reports "unsupported CVSS version".** cargo-deny is older than 0.18.9. Reinstall it.
- **Vite logs "dev proxy: CARDDEMO_API_BIND target … unreachable".** The API is not running on the proxied address.

# 10. Appendices

## A. Command Reference

| Purpose | Command (from `modern/`) |
|---|---|
| Stack, schema and fixtures | `scripts/dev-up.sh` |
| Apply migrations only | `cargo run -p runtime --bin carddemo-migrate` |
| API | `cargo run -p api --bin carddemo-api` |
| OpenAPI document, offline | `cargo run -p api --bin carddemo-openapi` |
| Broker workers | `cargo run -p messaging --bin carddemo-mq` |
| Batch job | `cargo run -p batch --bin carddemo-batch -- <subcommand>` |
| Frontend dev server | `pnpm --dir frontend dev` |
| Regenerate the contract and check it | `scripts/regen-api.sh` |
| Query cache check | `cargo sqlx prepare --workspace --check -- --all-targets` |
| Protected-path check | `scripts/protected-diff.sh` |
| NAV suite | `scripts/e2e.sh` |
| Full gate | `scripts/check.sh; rm -rf target/llvm-cov-target` |
| Tear down and wipe data | `docker compose down -v` |

## B. Port Reference

| Service | Default | Variable |
|---|---|---|
| PostgreSQL 16 | 127.0.0.1:5432 | `compose.yaml` (override with `COMPOSE_FILE`) |
| RabbitMQ AMQP | 127.0.0.1:5672 | `compose.yaml` |
| RabbitMQ management | 127.0.0.1:15672 | `compose.yaml` |
| `carddemo-api` | 127.0.0.1:8080 | `CARDDEMO_API_BIND` |
| Vite dev server | 127.0.0.1:5173 | `CARDDEMO_WEB_PORT` |

## C. Key File Locations

| Path | Contents |
|---|---|
| `modern/crates/{copybook,domain,persistence,runtime,api,messaging,batch,test-support}` | The eight workspace crates |
| `modern/crates/persistence/migrations/0001–0006*.sql` | Schema, the only source of its shape |
| `modern/.sqlx/` | Offline query cache (143 entries) |
| `modern/openapi/openapi.json`, `modern/frontend/src/api/schema.d.ts` | Generated contract and client types |
| `modern/frontend/src/screens/` | 21 screens and their render tests |
| `modern/audit/` | Parity audit, `ESCALATIONS.md` (E01–E56), `HALTS.md` (H1–H6) |
| `modern/docs/SIGNOFF.md` | Acceptance checklist (sections A–C) |
| `modern/scripts/check.sh` | The gate, steps 0–17 |
| `modern/compose.yaml`, `modern/rabbitmq/definitions.json` | Local stack and the seven durable queues |
| `modern/deny.toml` | Advisory, licence and source policy |

## D. Technology Versions

| Component | Version |
|---|---|
| Rust | 1.85.0 (edition 2024) |
| axum / sqlx / tokio | 0.8.9 / 0.8.6 / 1.53.3 |
| utoipa / lapin / argon2 / jsonwebtoken | 5.5.0 / 2.5.5 / 0.5.3 / 9.3.1 |
| printpdf / testcontainers / time | 0.7.0 / 0.23.3 / 0.3.41 |
| PostgreSQL / RabbitMQ | 16 / 3.13-management |
| Node.js / pnpm | 22 / 9.15.9 |
| React / react-router / Vite / TypeScript | 19.3.1 / 7.18.4 / 6.4.4 / 5.9.3 |
| Tailwind CSS / zod / TanStack Query | 4.3.3 / 3.25.76 / 5.104.4 |
| openapi-fetch / openapi-typescript | 0.17.0 / 7.13.0 |
| Vitest / Playwright | 3.2.7 / 1.64.1 |
| sqlx-cli / cargo-deny / cargo-llvm-cov | 0.8.6 / 0.18.9 / 0.6.21 |

## E. Environment Variable Reference

| Variable | Default (`.env.example`) | Read by |
|---|---|---|
| `DATABASE_URL` | `postgres://carddemo:carddemo@localhost:5432/carddemo` | api, messaging, batch, migrate; sqlx at build time |
| `CARDDEMO_AMQP_URL` | `amqp://carddemo:carddemo@localhost:5672/%2f` | messaging |
| `CARDDEMO_QUEUE_REQUEST_{DATE,ACCT,PAUTH}` | `CARD.DEMO.REQUEST.{DATE,ACCT,PAUTH}` | messaging |
| `CARDDEMO_MQ_POLL_MS` | `250` (1–5000) | messaging |
| `CARDDEMO_API_BIND` | `127.0.0.1:8080` | api, Vite proxy |
| `CARDDEMO_JWT_SECRET` | local placeholder; at least 32 bytes required | api |
| `CARDDEMO_JWT_TTL_SECONDS` | `3600` | api |
| `CARDDEMO_VAR_DIR` | `./var` | batch, api |
| `CARDDEMO_STATEMENT_DIR` | `./var/statements/` | batch (`creastmt`) |
| `CARDDEMO_INTRDR_DIR` | `./var/jobs/` | api (endpoint 14 job decks) |
| `CARDDEMO_FIXTURE_DIR` | `../app/data/ASCII` | batch loads |
| `CARDDEMO_OTEL_EXPORT` | unset; `stdout` enables span export | all binaries except `carddemo-openapi` |
| `RUST_LOG` | `info` | all binaries except `carddemo-openapi` |
| `CARDDEMO_WEB_PORT` | `5173` (not in `.env.example`) | Vite, Playwright, `e2e.sh` |
| `SQLX_OFFLINE` | unset; `true` builds from `.sqlx/` | sqlx macros |

## F. Developer Tools Guide

- **Contract changes.** Edit the handlers or DTOs, run `scripts/regen-api.sh`, and commit `openapi/openapi.json` together with `frontend/src/api/schema.d.ts`. Never hand-edit either file.
- **Schema changes.** Add a new expand-only migration, run `carddemo-migrate`, then `cargo sqlx prepare --workspace -- --all-targets`, and commit `.sqlx/`.
- **Compile-fail cases.** `TRYBUILD=overwrite cargo test -p domain --test money_algebra` refreshes `.stderr` files.
- **Snapshots.** `INSTA_UPDATE=always` rewrites `.snap` files. Review the diff before committing.
- **Coverage.** `cargo llvm-cov … --no-report`, then `cargo llvm-cov report --fail-under-lines <N>`, then remove `target/llvm-cov-target`.
- **Log search by message ID.** `carddemo-mq` logs base64url HMAC-SHA256 digests of `message_id`/`correlation_id`, keyed by `CARDDEMO_AMQP_URL` (`docs/SETUP.md` §2).

## G. Glossary

| Term | Meaning |
|---|---|
| Parity | Byte- and message-level equivalence with the COBOL program the code replaces |
| Escalation (E-row) | A recorded conflict or departure awaiting reviewer decision in `audit/ESCALATIONS.md` |
| Halt (H-row) | A COBOL behaviour too undefined to assert; its tests report BLOCKED until resolved or descoped |
| GDG generation | Versioned output file (`NAME.G<nnnn>`) standing in for a z/OS generation data group |
| CDRD / CDRA / CP00 | Broker transactions for date inquiry, account inquiry and payment authorization |
| i10 / i11 | Invariants: CP00 never exceeds the credit limit under concurrency; CP00 always answers or dead-letters |
| NAV | Playwright suite that walks all 21 routes through on-screen controls |
| g8 | The sign-off gate: every item of `docs/SIGNOFF.md` checked with evidence |
