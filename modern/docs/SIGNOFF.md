# CardDemo replacement: sign-off checklist (g8)

This is the acceptance checklist of binding g8 (AAP 0.9.5). It records, item by item, the evidence that the Rust, PostgreSQL, RabbitMQ and React replacement is equivalent to the COBOL parity oracle, and the reviewer decisions that only a person who knows the COBOL sources can take.

**Done ≡ o1..o7 ∧ every item checked.** Done is not reached while any item below is unchecked, any audit file in the review status table of [`../audit/README.md`](../audit/README.md#review-status) is not `Accepted`, the E29 row there is unsigned or `Rejected`, any halt in [`../audit/HALTS.md`](../audit/HALTS.md) is `BLOCKED`, or any binding is unverifiable, for example because Docker is unavailable and the container-backed checks could not run (AAP 0.9.4, 0.11.4).

## How this checklist is filled

- **Who fills what.**
  - Section A, automated items: the executing agent, only from a real local run of [`../scripts/check.sh`](../scripts/check.sh) and the commands named per row, at the recorded commit.
  - Section B, environment item: the operator or reviewer who observes it.
  - Section C, review items: the reviewer who knows the COBOL sources. Only that reviewer checks a section C item or writes a reviewer name, a date or a decision; no cell is pre-filled, defaulted or inferred.
- **Start state.** Every item starts unchecked (`- [ ]`) with an empty evidence cell.
- **Evidence cells** hold the commit hash, the command and its result, and for review items the reviewer's name and the date (`YYYY-MM-DD`). Logs are never committed, because only mapped paths may be created (R16): a cell names where its log is kept, either an attachment to the sign-off or a local path under `modern/var/`, which `modern/.gitignore` ignores.
- **Cardholder Data Redaction.** No evidence cell pastes output that holds a card number, SSN, CVV or password; logs are referenced by location only.
- **Local-Only Runtime (R14).** Evidence comes from local runs; no hosted CI run or link counts as evidence.
- **Equivalence failure constraint.** A row or item that a `BLOCKED` halt covers is recorded as blocked, never as passed, and Done cannot be reached while any halt is `BLOCKED` ([`../audit/HALTS.md`](../audit/HALTS.md), AAP 0.8.5).

Result vocabulary for section A:

| Result | Meaning |
|---|---|
| `PASS` | The command ran at the recorded commit and succeeded |
| `FAIL` | The command ran at the recorded commit and failed |
| `BLOCKED (Hn)` | A part of the row is covered by halt `Hn` of [`../audit/HALTS.md`](../audit/HALTS.md), which is `BLOCKED`; that part asserts no parity value and is never passed |
| `UNVERIFIED` | The command did not run, for example because Docker was unavailable or the run stopped before it |
| `PENDING a9` | The automated part passed, but the audit file its expectations come from is not yet `Accepted` in [`../audit/README.md`](../audit/README.md#review-status) |

A row or item is checked only when its result is `PASS` with no `BLOCKED`, `UNVERIFIED` or `PENDING a9` part, and with no review part still open in section C.

**Recorded commit:** `257c7e4d631b0b709d01c8167154b69cc8b7629a`, run locally on 2026-10-22 from `modern/` with the clone's `.env` exported. Every section A result below comes from that commit.

## A. Automated items

### A1. The check pipeline

- [ ] **A1.** [`scripts/check.sh`](../scripts/check.sh) exits 0 at the recorded commit, with its log attached.

  | Evidence | |
  |---|---|
  | Commit, command, exit status, log location | `257c7e4`; `env -u GOOGLE_APPLICATION_CREDENTIALS RUST_TEST_THREADS=4 bash -c 'set -a; . ./.env; set +a; scripts/check.sh'`; `FAIL`: exit 1, `check.sh: FAILED at step 15 (all tests under coverage instrumentation), exit 1`. Steps 0–14 passed; in step 15 every test binary passed except `test-support`'s `audit_coverage` (row 31), so steps 16 and 17 did not run within check.sh. Log `var/signoff/check.log` (under `modern/`, not committed) |

### A2. Each criterion of AAP 0.9.2 passes

- [ ] **A2.** Every row of the table below is checked.

Paths are relative to `modern/`, and commands run from `modern/`. A row's box is in its `#` cell.

| # | Criterion (bindings) | Test or check | Command | Result | Evidence |
|---|---|---|---|---|---|
| [ ] 1 | o1 online parity (g1a, g1b, NAV) | `crates/api/tests/openapi_contract.rs`; `crates/api/tests/endpoints_*.rs` (8 files); `frontend/e2e/nav.spec.ts` and the Vitest files (`frontend/src/schemas/*.test.ts`, `frontend/src/screens/*.test.tsx`, `frontend/src/lib/pfkeys.test.ts`) | `cargo test -p api --test openapi_contract`; `cargo test -p api --test 'endpoints_*'`; `scripts/e2e.sh`; `pnpm --dir frontend test` | `PASS` (automated parts); `BLOCKED (H3, H4, H5)`; `UNVERIFIED` part; `PENDING a9` (g1b, NAV) | Step 15: `openapi_contract` 2 passed; `endpoints_*` 54 passed, 25 ignored: 24 report parts `BLOCKED` by H3, H4, H5 or a halt candidate, and one reports COACTVWC-042/-046 and COACTUPC-093/-097 `UNVERIFIED` (a store read failing after the cross-reference read cannot be produced). Step 7: Vitest 44 files, 1630 tests passed. `scripts/e2e.sh` exit 0, NAV 4 passed. Logs `var/signoff/check.log`, `var/signoff/e2e.log` |
| [ ] 2 | o2 batch parity (i9) | `crates/batch/tests/posttran_i9.rs`; the other subcommands in `crates/batch/tests/*.rs` | `cargo test -p batch --test posttran_i9`; `cargo test -p batch` | `PASS` (automated parts); `BLOCKED (H2, H4)`; `PENDING a9` (i9, g3b) | Step 15: `posttran_i9` 6 passed; `cargo test -p batch` unit 97 and 7 test files (33 tests) passed, 0 ignored. `posttran_i9.rs`, `intcalc.rs` and `cbpaup0j.rs` report H2 and H4 parts `BLOCKED`; candidate parts (`X-POSTTRAN-H2`, `X-INTCALC-H1`–`H3`, `X-TRANREPT-H1`, `X-TRANREPT-H2`, `X-FIXTURES-H3`, `X-CREASTMT-H1`, `X-CREASTMT-H3`, `X-HALTS-2`, `X-MNTTRDB2-H1`) are reported, not asserted. Log `var/signoff/check.log` |
| [ ] 3 | o3 financial exactness (R1, i2) | Float lint and scans (check.sh steps 3–4); `crates/domain/tests/financial_parity.rs` | `cargo clippy --workspace --all-targets --locked`; `cargo test -p domain --test financial_parity` | `PASS`; `PENDING a9` (i2) | Steps 3 and 4 passed; step 15: `financial_parity` 17 passed. Log `var/signoff/check.log` |
| [ ] 4 | o4 data fidelity (i8, d9) | `crates/domain/tests/codec_a_roundtrip.rs`, `crates/copybook/tests/pic_roundtrip.rs`; `crates/persistence/tests/codec_b_roundtrip.rs`, `crates/persistence/tests/schema.rs` | `cargo test -p domain --test codec_a_roundtrip`; `cargo test -p copybook`; `cargo test -p persistence` | `PASS`; `PENDING a9` (i8, d9) | Step 15: `codec_a_roundtrip` 31 passed; copybook unit 78, `pic_roundtrip` 51, `total_parsers` 10 passed; persistence unit 166, `codec_b_roundtrip` 2, `explain` 2, `schema` 3 passed. Log `var/signoff/check.log` |
| [ ] 5 | o5 auth parity (R8, n5) | `crates/api/tests/auth_gate.rs` | `cargo test -p api --test auth_gate` | `PASS`; `PENDING a9` (R8, n5) | Step 15: `auth_gate` 3 passed. Log `var/signoff/check.log` |
| [x] 6 | o6 build hygiene (g2, R15) | Build, clippy, typecheck and scans | `scripts/check.sh` steps 1–7 | `PASS` | Steps 1–7 passed: fmt, build and clippy with warnings denied, float, `unsafe` and suppression scans, frontend typecheck and build. Log `var/signoff/check.log` |
| [ ] 7 | o7 frontend typed from OpenAPI (NAV) | `frontend/e2e/nav.spec.ts`; `scripts/regen-api.sh` (committed `openapi/openapi.json` and `frontend/src/api/schema.d.ts` match the handlers); strict typecheck | `scripts/e2e.sh`; `scripts/regen-api.sh`; `pnpm --dir frontend typecheck` | `PASS`; `PENDING a9` (NAV) | `scripts/e2e.sh` exit 0, NAV 4 passed (`var/signoff/e2e.log`); step 11 `regen-api.sh`: both files match HEAD; step 7 typecheck passed. Log `var/signoff/check.log` |
| [x] 8 | Structural: `domain` does not depend on sqlx (R3) | Dependency-tree and manifest scan | `scripts/check.sh` step 8 | `PASS` | Step 8 passed. Log `var/signoff/check.log` |
| [x] 9 | Structural: compile-time SQL (R4) | Non-macro query scan; `cargo sqlx prepare --check` against the migrated database | `scripts/check.sh` steps 6, 14 | `PASS` | Step 6 non-macro SQL scan and step 14 `cargo sqlx prepare --workspace --check` passed. Log `var/signoff/check.log` |
| [x] 10 | Structural: type algebra (R18) | `crates/domain/tests/money_algebra.rs` running `tests/compile_fail/*.rs` and `tests/compile_pass/i2_formula.rs` | `cargo test -p domain --test money_algebra` | `PASS` | Step 15: `money_algebra` 1 passed (the trybuild `compile_fail` and `compile_pass` cases). Log `var/signoff/check.log` |
| [x] 11 | Structural: lockfiles committed; dependencies and build (R10) | `--locked`, `--frozen-lockfile`, `git ls-files`, `cargo deny check` | `scripts/check.sh` steps 2, 7, 9, 10 | `PASS` | Steps 2, 7, 9 and 10 passed; `cargo deny check` (cargo-deny 0.18.9, the host build, which parses CVSS 4.0 advisories; AAP 0.7.1 names 0.18.3): advisories, bans, licenses and sources ok. Log `var/signoff/check.log` |
| [x] 12 | Local development (R14) | Cloud-credential environment check; cloud SDK lockfile scan; Compose runs only PostgreSQL and RabbitMQ | `scripts/check.sh` steps 0, 10 | `PASS` | Steps 0 and 10 passed. The run removed `GOOGLE_APPLICATION_CREDENTIALS`, which the host platform sets in every shell, with `env -u`; no other listed variable was set. `docker compose config --services`: `postgres`, `rabbitmq`. Log `var/signoff/check.log` |
| [x] 13 | Protected paths unchanged; Authority (R16) | `scripts/protected-diff.sh` | `scripts/protected-diff.sh` | `PASS` | `scripts/protected-diff.sh` exit 0, `protected-diff: OK` (`var/signoff/protected-diff.log`); also step 12. Log `var/signoff/check.log` |
| [ ] 14 | Escalation and anomaly handling (R7) | Typed-error cases in `crates/copybook/tests/pic_roundtrip.rs`; `audit/ESCALATIONS.md` | `cargo test -p copybook --test pic_roundtrip`; the log review is item C4 | `PASS` (automated part); log review open (C4) | Step 15: `pic_roundtrip` 51 passed, typed-error cases included. Log `var/signoff/check.log` |
| [ ] 15 | Messaging (g3a) | `crates/messaging/tests/{cdrd,cdra,cp00}.rs` with `rabbitmq:3.13-management` and `rabbitmq/definitions.json` | `cargo test -p messaging` | `PASS` (automated parts); `BLOCKED (H1, H2, H4, H6)`; `PENDING a9` (g3a) | Step 15: `cargo test -p messaging`: unit 41, `cdrd` 5, `cdra` 7, `cp00` 11 passed; 8 ignored in those files, each reporting a part `BLOCKED` by H1, H2, H4, H6 or a candidate (`X-CODATE01-H2`, `X-COACCT01-H4`). Log `var/signoff/check.log` |
| [ ] 16 | Statements (g3b) | `crates/batch/tests/creastmt.rs`: `STATEMNT.PS` and `STATEMNT.HTML` equal the audited expectations; text extracted from `STATEMNT.PDF` equals the `STATEMNT.PS` lines | `cargo test -p batch --test creastmt` | `PASS`; `PENDING a9` (g3b) | Step 15: `creastmt` 5 passed; candidate parts `X-CREASTMT-H1` and `X-CREASTMT-H3` are reported, not asserted. Log `var/signoff/check.log` |
| [x] 17 | Credit limit (i10) | `crates/messaging/tests/cp00_credit_limit.rs`; process-local lock scan | `cargo test -p messaging --test cp00_credit_limit`; `scripts/check.sh` step 6 | `PASS` | Step 15: `cp00_credit_limit` 1 passed (two `carddemo-mq` processes, doubled delivery); step 6 lock scan passed. Log `var/signoff/check.log` |
| [ ] 18 | Liveness (i11) | `crates/messaging/tests/cp00_liveness.rs` | `cargo test -p messaging --test cp00_liveness` | `PASS` (i11 assertions); `BLOCKED (H6)` | Step 15: `cp00_liveness` 3 passed, 1 ignored, which reports the CP00 reply length and 64th byte `BLOCKED` (H6). Log `var/signoff/check.log` |
| [x] 19 | Cardholder Data Redaction | `crates/api/tests/redaction.rs`, `crates/messaging/tests/redaction.rs` | `cargo test -p api --test redaction`; `cargo test -p messaging --test redaction` | `PASS` | Step 15: api `redaction` 4 passed; messaging `redaction` 3 passed. Log `var/signoff/check.log` |
| [x] 20 | Single Component System | `scripts/frontend-policy.mjs` against `frontend/approved-dependencies.json` | `node scripts/frontend-policy.mjs` | `PASS` | Step 7: `node scripts/frontend-policy.mjs` reported nothing. Log `var/signoff/check.log` |
| [ ] 21 | Checked Integer Narrowing | `cast_possible_truncation` and `cast_sign_loss` at `deny` with warnings denied; integer `as` scan; `// TRUNCATION:` comments, reviewed in item C5 | `scripts/check.sh` steps 2, 3, 6 | `PASS` (automated part); review open (C5) | Steps 2, 3 and 6 passed (cast lints, integer `as` scan). Log `var/signoff/check.log` |
| [x] 22 | Bounded Pagination | `crates/api/tests/pagination.rs` (`page_size=101` → 422 on endpoints 7, 11, 16, 21, 24, 25); `crates/persistence/tests/explain.rs` (index scan for every file in `crates/persistence/queries/`) | `cargo test -p api --test pagination`; `cargo test -p persistence --test explain` | `PASS` | Step 15: `pagination` 3 passed; `explain` 2 passed. Log `var/signoff/check.log` |
| [ ] 23 | Coverage Floors | `cargo llvm-cov` line floors of 80 % over the workspace and 90 % over `copybook` and `domain`; the Vitest 70 % line threshold | `scripts/check.sh` steps 7, 15, 16 | Step 7 `PASS`; step 15 `FAIL` (row 31 only); step 16 `PASS` | Step 7: Vitest 98.65 % lines. Step 15 failed on `audit_coverage` alone; check.sh stopped there. The two step-16 commands, run by hand over the step-15 profiles: workspace 96.20 % lines (80 % floor, exit 0), `copybook` and `domain` 98.73 % (90 % floor, exit 0). Logs `var/signoff/check.log`, `var/signoff/coverage-workspace.log`, `var/signoff/coverage-copybook-domain.log` |
| [x] 24 | Local-Only Runtime | Cloud-credential environment check; cloud SDK lockfile scan; testcontainers for every Docker-backed test | `scripts/check.sh` steps 0, 10 | `PASS` | Steps 0 and 10 passed (see row 12). Step 15 ran every Docker-backed suite on testcontainers started by `test-support`; the Compose stack served steps 13–14 only. Log `var/signoff/check.log` |
| [x] 25 | Single Schema Executor | Migration-macro location, caller and expand-only scans; query cache checked against a database migrated from `crates/persistence/migrations`; previous-release query set planned against the migrated schema | `scripts/check.sh` steps 6, 14; `cargo test -p persistence --test explain` | `PASS` | Step 6 migration-macro, caller and expand-only scans and step 14 cache check passed; step 15: `explain` 2 passed (`compat/previous/BASELINE.md` declares the empty previous query set). Log `var/signoff/check.log` |
| [ ] 26 | Thin Request Handlers | Layering scan of `crates/api/src/handlers`; the three-step shape of every handler, reviewed in item C5 | `scripts/check.sh` step 6 | `PASS` (automated part); review open (C5) | Step 6 handler layering scan passed. Log `var/signoff/check.log` |
| [x] 27 | Repository Method Naming | `crates/test-support/tests/repository_naming.rs` | `cargo test -p test-support --test repository_naming` | `PASS` | `cargo test -p test-support --test repository_naming`, run by hand because check.sh stopped before it: 3 passed (`var/signoff/repository_naming.log`) |
| [x] 28 | Generated API Types Only | Contract regeneration diff; hand-authored payload-type scan | `scripts/regen-api.sh`; `node scripts/frontend-policy.mjs` | `PASS` | Step 11 `regen-api.sh`: both files match HEAD; step 7 `frontend-policy.mjs` reported nothing. Log `var/signoff/check.log` |
| [x] 29 | Typed Dates and Timestamps | `crates/persistence/tests/schema.rs` (`date` and `timestamp without time zone` columns); the sqlx macros, which fail the build when a row field's Rust type does not match its column | `cargo test -p persistence --test schema` | `PASS` | Step 15: `schema` 3 passed; steps 2 and 14 type-checked every query macro. Log `var/signoff/check.log` |
| [x] 30 | Total Parsers Over Untrusted Input | `crates/copybook/tests/total_parsers.rs`, `crates/domain/tests/total_parsers.rs`; the `unwrap_used`, `expect_used` and `indexing_slicing` denials under clippy | `cargo test -p copybook --test total_parsers`; `cargo test -p domain --test total_parsers` | `PASS` | Step 15: copybook `total_parsers` 10 passed, domain `total_parsers` 16 passed; step 3 clippy passed with the three denials. Log `var/signoff/check.log` |
| [ ] 31 | Derive lane: no code without its audit (a8) | `crates/test-support/tests/audit_coverage.rs` | `cargo test -p test-support --test audit_coverage` | `FAIL` | Step 15: `audit_coverage` failed: 3508 rules, 3250 satisfied, 64 `BLOCKED`, 194 failures, every one a rule lacking its required test tags (mostly `client` rows of `audit/screens.md`). Log `var/signoff/check.log` |
| [ ] 32 | Derive lane: audits accepted before dependent code (a9) | Status table in `audit/README.md`, `audit/ESCALATIONS.md`, `audit/HALTS.md` | Reviewer: see section C | Reviewer item (C1, C2, C4, C7) | |
| [ ] 33 | Sign-off (g8) | This file | Reviewer: checked last, when every other item is checked | Reviewer item, checked last | |

Notes on the table:

- **a9 gates the parity rows.** Bindings g1b, g3a, g3b, i2, i9, NAV, R8 and n5 take their expectations from their audit files, and i8 and d9 from `audit/copybooks.md` and `audit/fixtures.md`. Each counts as passed only once those files are `Accepted` in [`../audit/README.md`](../audit/README.md#review-status) (AAP 0.9.1); until then its row records `PENDING a9`, however its tests ran.
- **Halted parts stay blocked.** g1b's rows covered by H3, H4 and H5, and g3a's rows covered by H1, H2, H4 and H6, assert no parity value and are reported `BLOCKED` while those halts are `BLOCKED`. Halt candidates in [`../audit/HALTS.md`](../audit/HALTS.md#halt-candidates) that the reviewer has not yet folded in (`X-HALTS-1`, `X-HALTS-2`) likewise leave the rows they name without a parity assertion.
- **Docker.** Rows 1, 2, 4 (the d9 part), 5, 7, 15–19, 22, 23 (the workspace floor) and 25 (the query-set part) need PostgreSQL or RabbitMQ. Without Docker, check.sh stops after step 12 with exit status 2 and those rows are `UNVERIFIED`.

### A3. Protected paths

- [x] **A3.** The protected-path and Authority diff against `59cc6c2` is empty (R16): [`scripts/protected-diff.sh`](../scripts/protected-diff.sh) exits 0.

  | Evidence | |
  |---|---|
  | Commit, command, exit status | `257c7e4`; `scripts/protected-diff.sh`; `PASS`: exit 0, `protected-diff: OK`. Log `var/signoff/protected-diff.log`; step 12 of the check.sh run passed too |

### A4. Lockfiles

- [x] **A4.** Both lockfiles are committed (R10): `git -C "$(git rev-parse --show-toplevel)" ls-files --error-unmatch modern/Cargo.lock modern/frontend/pnpm-lock.yaml` exits 0.

  | Evidence | |
  |---|---|
  | Commit, command, exit status | `257c7e4`; the command above; `PASS`: exit 0, both paths listed; step 9 of the check.sh run passed too |

## B. Environment item

- [ ] **B1.** `docker compose up -d --wait`, run from `modern/` as [SETUP.md](SETUP.md#3-database-and-broker) describes, brings `postgres` and `rabbitmq` up healthy, and the RabbitMQ management console at `http://localhost:15672` lists the seven durable queues of [`../rabbitmq/definitions.json`](../rabbitmq/definitions.json): `CARD.DEMO.REQUEST.DATE`, `CARD.DEMO.REQUEST.ACCT`, `CARD.DEMO.REQUEST.PAUTH`, `CARD.DEMO.REPLY.DATE`, `CARD.DEMO.REPLY.ACCT`, `CARD.DEMO.ERROR` and `CARD.DEMO.DEADLETTER.PAUTH`. Checked by the operator or reviewer who observes it.

  | Evidence | |
  |---|---|
  | Observer, date | |

## C. Review items

The reviewer who knows the COBOL sources checks these items, under the review procedure of [`../audit/README.md`](../audit/README.md#review-procedure) (AAP 0.8.6). None is checked by an automated run.

### C1. Audit files accepted (a9)

- [ ] **C1.** Every audit file in the [review status table](../audit/README.md#review-status) of `../audit/README.md` is `Accepted`, with the reviewer's name and date. The table is the record; its rows are not copied here.

  | Evidence | |
  |---|---|
  | Reviewer, date, commit reviewed | |

### C2. E29 — CP00 reply before write, signed on its own

- [ ] **C2.** The [E29 row](../audit/README.md#e29--cp00-reply-before-write) of `../audit/README.md` carries an `Accepted` decision with the reviewer's name and date, against this exposure (AAP 0.3.4.2, 0.8.6):
  1. A delivered decision can go unrecorded: after an `I003` summary-write failure, after a commit failure, or after a process or connection loss that follows the acknowledgement, the requester holds the reply while nothing is committed.
  2. An `I004` detail-insert failure commits the summary without its detail.
  3. i10 counts only committed approvals, so the account's real exposure can exceed the committed figure by the approvals replied but never recorded.

  This item is never checked by item C4 or by any general escalation review. The sub-table below must match the E29 row of [`../audit/README.md`](../audit/README.md#e29--cp00-reply-before-write), and [`../audit/ESCALATIONS.md`](../audit/ESCALATIONS.md) shows E29 as `AWAITING SIGNATURE` until that row is signed. A `Rejected` decision reopens the CP00 ordering decision ([ARCHITECTURE.md, section 10.2](ARCHITECTURE.md#102-cp00-reply-then-write-e29)); it does not pass with a note, and this item stays unchecked.

  | Reviewer | Date | Decision (`Accepted` / `Rejected`) |
  |---|---|---|
  | | | |

### C3. Constraint and derive-lane labels

- [ ] **C3.** The definitions of R7 — Anomaly handling, R10 — Dependencies and build, R14 — Local-only runtime and R16 — Authority and protected paths (AAP 0.10), and of a8 and a9 (AAP 0.9.1), are confirmed to label the request constraints and derive-lane checks they name. The user has stated that the R7, R10, R14 and R16 definitions do not reproduce the original rules; that content now arrives as the twelve named rules of AAP 0.10, and where a named rule overlaps a label, the named rule's stricter wording governs (Local-Only Runtime over R14).

  | Evidence | |
  |---|---|
  | Reviewer, date | |

### C4. Escalation resolutions

- [ ] **C4.** Every row of [`../audit/ESCALATIONS.md`](../audit/ESCALATIONS.md) other than E29 (E01–E28, E30–E45 and any row added later) has a resolution the reviewer agrees with, recorded as `AGREED — <reviewer>, <date>` in its `Status` column. E29 is item C2 alone.

  | Evidence | |
  |---|---|
  | Reviewer, date, rows agreed | |

### C5. Thin handlers and integer narrowing

- [ ] **C5.** Every API handler in `crates/api/src/handlers/` has the three-step shape of Thin Request Handlers (extract the validated request and the signed-on or admin user, call one service function, map its outcome), and every integer narrowing that remains carries a `// TRUNCATION:` comment the reviewer accepts (Checked Integer Narrowing).

  | Part | Evidence (reviewer, date) |
  |---|---|
  | Thin Request Handlers: every handler reviewed | |
  | Checked Integer Narrowing: every `// TRUNCATION:` comment accepted | |

### C6. Scope of i10

- [ ] **C6.** The user's answer to whether i10 also binds POSTTRAN, INTCALC, COBIL00C, COACTUPC and CBPAUP0C, which keep their audited behaviour and do not consult pending authorizations today ([ARCHITECTURE.md, section 10.3](ARCHITECTURE.md#103-cp00-i10-credit-limit)), is recorded here, quoted and dated, and any change the answer requires is planned before this item is checked.

  | Evidence | |
  |---|---|
  | The user's answer (quoted), date, planned change | |

### C7. Halts resolved or descoped

- [ ] **C7.** Every case of [`../audit/HALTS.md`](../audit/HALTS.md) is `RESOLVED`, by a source-backed resolution the reviewer has confirmed against its cited source and that is now covered by tagged tests, or `DESCOPED`, with the user's instruction quoted. Acknowledging a halt's analysis, in the [halt acknowledgement table](../audit/README.md#halt-acknowledgements) of `../audit/README.md`, does not check this item and does not change the halt's status: an acknowledged halt stays `BLOCKED`. A halt added from H7 onward, or a halt candidate folded into an existing row, joins this table with status `BLOCKED`.

  | ID | Path (short) | Status | Resolution or quoted instruction | Analysis acknowledged (reviewer, date) |
  |---|---|---|---|---|
  | H1 | CP00 replies after the first message of one CICS trigger task (`COPAUA0C.cbl`, E14, E15) | `BLOCKED` | | |
  | H2 | CP00 declined path: amount added before its first assignment; detail segment never initialised (`COPAUA0C.cbl`, E15) | `BLOCKED` | | |
  | H3 | Db2 messages embedding `SQLCODE`, `SQLERRM` or `DSNTIAC` text (`COTRTUPC.cbl`, `COTRTLIC.cbl`, E19) | `BLOCKED` | | |
  | H4 | CICS `RESP`/`RESP2`, MQ and IMS codes inside error texts and log fields (online, CDRD, CDRA, CP00) | `BLOCKED` | | |
  | H5 | `CSUTLDTC` date validity delegated to Language Environment `CEEDAYS` (`CSUTLDTC.cbl`) | `BLOCKED` | | |
  | H6 | CP00 reply length: the 64th byte comes from a buffer never written (`COPAUA0C.cbl`) | `BLOCKED` | | |

### C8. Scope acknowledgements

- [ ] **C8.** The three scope decisions of AAP 0.2.3 are acknowledged, each with the reviewer's name and date.

  | Acknowledgement | Evidence (reviewer, date) |
  |---|---|
  | The 19 + 2 screen reconciliation: the 19 maps of the request's inventory (17 in `app/bms`, 2 in `app/app-authorization-ims-db2-mq/bms`) plus COTRTLI and COTRTUP for CTLI and CTTU, all 21 built ([E21](../audit/ESCALATIONS.md)) | |
  | The 15-job selection with its corrected rule, applied to the 27 batch jobs of [`README.md:298-326`](../../README.md); the selected jobs are the `carddemo-batch` subcommands of [SETUP.md, section 6](SETUP.md#6-batch-jobs), and the programs they reach are in [ARCHITECTURE.md, section 8](ARCHITECTURE.md#8-legacy-disposition) | |
  | The per-paragraph decomposition into 27 endpoints ([API.md, sections 1–3](API.md#1-overview)) | |

---

Done is reached only when o1..o7 hold and every item of this checklist is checked, which requires every audit file `Accepted`, the E29 row signed `Accepted`, no halt `BLOCKED` and no binding unverified.
