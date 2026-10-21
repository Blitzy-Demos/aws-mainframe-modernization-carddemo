# Derive-lane audit

This directory is the parity oracle's derive-lane (#2) audit. No recorded COBOL outputs exist, so every expected result the replacement is tested against is interpreted from the COBOL source and written here before the code and tests that consume it. Code that consumes an audit file is accepted only once that file is `Accepted` in the review status table below (a9; AAP 0.8.6). The COBOL tree is read in place at commit `59cc6c2` and never modified (R16).

This file sets the conventions every other audit file follows, which `modern/crates/test-support/src/audit_index.rs` parses and `modern/crates/test-support/tests/audit_coverage.rs` (a8) enforces, and it holds the reviewer's ledger, which `modern/docs/SIGNOFF.md` (g8) reads.

## Files

Audit paths are relative to `modern/audit/`; `crates/` and `frontend/` are under `modern/`.

- `copybooks.md`: every field of `CSUSR01Y`, `CVACT01Y`, `CVACT02Y`, `CVACT03Y`, `CVCUS01Y`, `CVTRA01Y`–`CVTRA06Y`, `COCOM01Y`, `CIPAUSMY`, `CIPAUDTY`, `CCPAURQY`, `CCPAURLY` and `CCPAUERY` (offset, PIC, Rust type, column), the timestamp layout codes and the redacted forms; supplies `crates/domain/src/records/*.rs`, `crates/persistence/migrations/*.sql`, `crates/persistence/src/rows/*.rs`, `crates/persistence/tests/schema.rs` and the codec tests (i8, d9).
- `arithmetic.md`: the interest, bill-payment and tcatbal statements, the CP00 available amount and the CBPAUP0C adjustments, with the truncation and precision argument; supplies `crates/domain/src/money.rs` and `crates/domain/tests/financial_parity.rs` (i2).
- `fixtures.md`: the framing and anomalies of the nine ASCII fixtures and the ten USRSEC in-stream records, and the results of the audited rules applied to them (loads, POSTTRAN, INTCALC, CREASTMT, TRANREPT); supplies the codec tests (i8, d9) and `crates/batch/tests/*.rs` (i9, g3b).
- `screens.md`: the 21 BMS maps (fields, positions, lengths, attributes, PF-key labels and the program that serves each key); supplies `frontend/src/screens/*.tsx`, `frontend/src/schemas/*.ts`, `crates/api/src/dto/*.rs` and NAV.
- `online/<PROGRAM>.md`, 23 files: `COSGN00C`, `COMEN01C`, `COADM01C`, `COACTVWC`, `COACTUPC`, `COCRDLIC`, `COCRDSLC`, `COCRDUPC`, `COTRN00C`, `COTRN01C`, `COTRN02C`, `CORPT00C`, `COBIL00C`, `COUSR00C`, `COUSR01C`, `COUSR02C`, `COUSR03C`, `COPAUS0C`, `COPAUS1C`, `COPAUS2C`, `COTRTLIC`, `COTRTUPC` and `CSUTLDTC` (with `CSUTLDPY` and `CSUTLDWY`; the `CSLKPCDY` lookups and COACTUPC's inline edits are in `COACTUPC.md`): paragraphs, conditions, messages with their HTTP status, file, SQL and DL/I effects, and transfer targets; supplies `crates/domain/src/online/*.rs`, `crates/api/src/{services,handlers}/*.rs`, `crates/api/tests/*.rs` (g1b, R8, n5) and NAV.
- `batch/<JOB>.md`, 7 files: `POSTTRAN` (CBTRN02C), `INTCALC` (CBACT04C), `CREASTMT` (CBSTM03A, CBSTM03B, COSTM01), `TRANREPT` (CBTRN03C with the `TRANREPT.jcl`/`TRANREPT.prc` sort and include rules), `CBPAUP0J` (CBPAUP0C), `MNTTRDB2` (COBTUPDT) and `LOADS` (the eight IDCAMS loads and DUSRSECJ); supplies `crates/domain/src/batch/*.rs`, `crates/batch/src/jobs/*.rs` and `crates/batch/tests/*.rs` (g3b, i9).
- `messaging/<PROGRAM>.md`, 3 files: `CODATE01`, `COACCT01` and `COPAUA0C`: request parsing, reply and error bodies byte by byte, properties, decision and reason codes and database effects, for COPAUA0C also the i10 guard rows (E30) and the i11 parse outcomes (E31); supplies `crates/domain/src/messaging/*.rs`, `crates/messaging/src/workers/*.rs` and `crates/messaging/tests/*.rs` (g3a, i10, i11).
- `ESCALATIONS.md`: the escalation log (`E` IDs), cited by every lane.
- `HALTS.md`: the halt report (`H` IDs); a `BLOCKED` halt blocks the bindings it lists and g8.

## Rule tables

### Shape

Every audit file except `README.md`, `ESCALATIONS.md` and `HALTS.md` is a rule file: an optional prose preamble, exactly one rule table, then the optional candidate sections described under "Halts and escalations". `audit_index.rs` reads the rule table of every `.md` file under `modern/audit/` except those three, so the illustrations in this file define no rule.

The rule table's header row starts with `| Rule ID |`. Its base form is:

```text
| Rule ID | Program | Paragraph | Copybook field or PIC | Input condition | Expected result | Source lines |
```

Extra columns follow `Source lines`, in this order and only in these files:

| File | Extra columns |
|---|---|
| `fixtures.md` | `Fixture record` |
| `arithmetic.md` | `Statement`, `Rounding clause` |
| `online/*.md`, `screens.md` | `Layer` |

- One row per COBOL rule. The table is the header, the separator row `|---|…|` and every following line that starts with `|`; it ends at the first line that does not. Every row has as many cells as the header, a literal `|` inside a cell is written `\|`, and a cell with nothing to state holds `—`.
- `Source lines` cites `<file>:<lines>`, where `<lines>` is a line, a range `start-end` or a comma list, and several citations are separated by `;`, for example `CBTRN02C.cbl:403-419` or `COPAUA0C.cbl:428,499,511; CCPAUERY.cpy:40`. Short file names resolve through the folders the file's preamble lists.
- The preamble is prose, which the index ignores. The root files use the sections `## Sources`, `## Reading the table` and `## Rules governing this file`, with `## How the rows are proved` where the proving tests need naming; the online, batch and messaging files use the same headings.

Model rows, as `batch/POSTTRAN.md` holds them. The expiration check runs after the limit check and overwrites its reason (E09), so the final result depends on both conditions:

```text
| Rule ID | Program | Paragraph | Copybook field or PIC | Input condition | Expected result | Source lines |
|---|---|---|---|---|---|---|
| CBTRN02C-007 | CBTRN02C | 1500-B-LOOKUP-ACCT | `ACCT-CREDIT-LIMIT S9(10)V99`, `WS-TEMP-BAL S9(09)V99`, `ACCT-EXPIRAION-DATE X(10)` | limit < (cycle credit − cycle debit + amount), after the sum is stored into `S9(09)V99`, and `ACCT-EXPIRAION-DATE ≥ DALYTRAN-ORIG-TS(1:10)` | reason `0102`, description `OVERLIMIT TRANSACTION`, record rejected | CBTRN02C.cbl:403-419 |
| CBTRN02C-008 | CBTRN02C | 1500-B-LOOKUP-ACCT | `ACCT-CREDIT-LIMIT S9(10)V99`, `WS-TEMP-BAL S9(09)V99`, `ACCT-EXPIRAION-DATE X(10)` | `ACCT-EXPIRAION-DATE < DALYTRAN-ORIG-TS(1:10)`, whether or not the limit is exceeded | reason `0103`, description `TRANSACTION RECEIVED AFTER ACCT EXPIRATION`, record rejected; when both conditions hold, the `0102` set at line 410 is overwritten at line 417 and only `0103` reaches the reject trailer (E09) | CBTRN02C.cbl:403-419 |
```

### Rule IDs

An ID is `<MEMBER>-<nnn>`: an upper-case member name, a hyphen and three digits, matched by `^[A-Z][A-Z0-9]*-[0-9]{3}$`, and unique across all rule files.

| File | `<MEMBER>` | Example |
|---|---|---|
| `online/<PROGRAM>.md`, `messaging/<PROGRAM>.md` | the program | `COSGN00C-001` |
| `batch/POSTTRAN.md` | `CBTRN02C` | `CBTRN02C-007` |
| `batch/INTCALC.md` | `CBACT04C` | `CBACT04C-001` |
| `batch/CREASTMT.md` | `CBSTM03A` or `CBSTM03B`, the program whose paragraph the row cites | `CBSTM03B-001` |
| `batch/TRANREPT.md` | `CBTRN03C` | `CBTRN03C-001` |
| `batch/CBPAUP0J.md` | `CBPAUP0C` | `CBPAUP0C-001` |
| `batch/MNTTRDB2.md` | `COBTUPDT` | `COBTUPDT-001` |
| `batch/LOADS.md` | `LOADS` | `LOADS-001` |
| `copybooks.md` | the copybook; `PIC` for a codec rule over a class of fields | `CVACT01Y-007`, `PIC-015` |
| `screens.md` | the map | `COSGN00-001` |
| `arithmetic.md` | `ARITH` | `ARITH-001` |
| `fixtures.md` | `FIXFRM` (framing and corpus scans), `FIXUSR` (USRSEC records), `FIXLOAD` (loads), `FIXPOST` (nnn is the `dailytran.txt` line; 901–903 summaries), `FIXINT` (nnn is the `tcatbal.txt` line; 901–903 summaries), `FIXSTMT` (nnn is the `cardxref.txt` line; 901–904 file rows), `FIXRPT` (nnn is the `cardxref.txt` line of the card block; 901–905 file rows) | `FIXPOST-901` |

- Copybook names end in `Y`, map names lack the program's trailing `C`, and `PIC`, `ARITH`, `LOADS` and the `FIX` members name no program, so no member collides with a program name.
- IDs are stable. Tests and rule modules cite them, so a row is never renumbered and an ID is never reused for another rule; a rejected row is revised under its own ID.

### Test tags and citations

- Each Rust or TypeScript test that proves a rule carries `// audit: <Rule ID>`, one ID per comment; a test that proves several rules carries several comments.
- Each rule module under `crates/domain/src/{online,batch,messaging}/` other than `mod.rs` cites the rule IDs it implements in its doc comments, at least one, and each implementing function names its rule ID in its doc comment (AAP 0.1.2).
- In `crates/*/src/**` every word of the rule-ID form is read as a rule citation and must name a row; in test files the `// audit:` tags are the citations. Words of that shape that are not rule IDs, such as the hash name `SHA-256`, are written without the hyphen in source.

### Layers and required tags

The rule's observable effect decides its layer, never the key that triggers it. A zod schema that repeats a server check adds a client half (`both`); it never makes the row `client`.

- `server`: the outcome is observable through an endpoint: status, message, field, response values or store effects. This covers every key and selection an endpoint serves (CTTU's PF4 delete through endpoint 27, CTLI's `U`/`D` and F10 through endpoint 25) and every program edit the endpoint performs that no zod schema repeats. Requires a tag in `crates/api/tests/endpoints_*.rs`.
- `client`: the behaviour sends no request: transfers and navigation, PF3, PF12, PF4 where it only clears the screen, CTLI's PF2, row selections that only navigate, PF-key labels and bindings, and what a screen renders. Requires a tag in `frontend/e2e/nav.spec.ts` or in a Vitest file (`frontend/src/**/*.test.ts` or `*.test.tsx`).
- `both`: a check the endpoint performs that the screen's zod schema also applies before sending: program edits the schema repeats, and the map `LENGTH` and `NUM` the request DTO revalidates. Requires one tag of each kind: the server half in `endpoints_*.rs`, the client half in a Vitest file such as `frontend/src/schemas/<MAP>.test.ts` or in `nav.spec.ts`.
- Files without a `Layer` column (`copybooks.md`, `arithmetic.md`, `fixtures.md`, `batch/*.md`, `messaging/*.md`) require at least one tag in a Rust test under `crates/*/tests/`.
- A `HALTED(Hn)` row, and a row that a candidate marker `HALTED(X-<FILE>-<n>)` blocks (Halts and escalations), still carries its tags: the tagged test reports the case `BLOCKED` and asserts no parity value.

`audit_coverage.rs` (a8) fails when a row lacks the tags its layer requires, when a tag or a rule ID cited in source names no row, or when a rule module cites no rule ID.

### Messages and HTTP status

Every message row of `online/*.md` gives the literal exactly as the program moves it and the HTTP status the endpoint returns with it: 400 input error, 401 sign-on failure or a missing or invalid token, 403 admin gate, 404 not found, 409 changed by another user, 500 store failure. Status 422 is reserved for an out-of-range `page_size` on the collection endpoints (Bounded Pagination) and appears in no COBOL message row.

### Cardholder data

Under Cardholder Data Redaction, no audit file contains a full card number, SSN, CVV or password value. Fixture records are cited as `file:line`. `fixtures.md` writes the 16 bytes of a card number as `{C:n}` (the card on `cardxref.txt:n`) and of a transaction ID as `{T:n}` (`dailytran.txt:n`); the other files may use the same references. A row that states an API, broker or log result states the redacted form: a card number as twelve `*` followed by its last four digits, an SSN as `***-**-` followed by its last four digits (E32). The one exception is endpoint 8 (`GET /cards/view`) with an `A` token, which returns the full number; its rows say so without quoting one.

### Halts and escalations

- A row whose expected value a halt blocks says `HALTED(Hn)` in Expected result and asserts no value; every binding that covers it is reported `BLOCKED`, never passed (Equivalence failure constraint; `HALTS.md`). A row that names a halt while fixing only what the halt leaves assertable, such as a field's type or offset, cites it as `halt Hn` and stays provable.
- A row whose expected value a halt candidate blocks, a candidate of its own file or of any other, says `HALTED(X-<FILE>-<n>)` in Expected result with exactly the candidate's name (`HALTED(X-COACCT01-H1)`, `HALTED(X-ARITH-5)`), one name per marker. A row that several halts or candidates block writes one marker per name (`HALTED(X-COPAUS1C-H1)`, `HALTED(X-COPAUS1C-H2)`). No other spelling is used: no word such as `candidate`, no ellipsis and no list of names inside one marker. `audit_index.rs` reads only `HALTED(H<digits>)` markers, each of which must name a row of `HALTS.md`, and ignores candidate markers, so a candidate-marked row asserts no parity value until the reviewer folds the candidate in, when its marker becomes the assigned `HALTED(Hn)`, or rejects it. A row that names a candidate's case while asserting the rest cites the candidate by name without `HALTED`, as it would cite `halt Hn`.
- Rows cite escalations by ID (`E09`) and candidates by name (`X-COPYBOOKS-1`) until the reviewer assigns an ID.
- Under R7 — Anomaly handling, an anomaly or a case with no single result found while writing a rule file is never normalised or smoothed over. It is proposed after the rule table, as a bullet under `## Escalation candidates` or `## Halt candidates`, never as a second table; the index ignores everything outside the rule table.
  - Name: `X-<FILE>-<n>`. `<FILE>` is the file's upper-case short name (`COPYBOOKS`, `ARITH`, `FIXTURES`, `SCREENS`, `HALTS`; the program or job name for the online, batch and messaging files), and `<n>` is a number unique within the file, which a file may prefix with `H` for its halt candidates (`X-FIXTURES-H1`).
  - An escalation candidate gives `Found in` (path:lines), `Anomaly class` (a class of `ESCALATIONS.md`), `Finding` and `Proposed resolution`. A halt candidate gives `Path`, `Why no single result`, `Bindings blocked`, `Escalation` and `Candidate resolution`, and says whether it proposes a new halt or joins an existing one.
  - Until the reviewer folds a halt candidate in or rejects it, no parity assertion is written for what it names.
- During review the reviewer folds escalation candidates into `ESCALATIONS.md` under its next free ID (E44 onward; E01–E43 are assigned) and halt candidates into `HALTS.md`, as a new row from H7 onward or into the existing halt whose case they share. The candidate's file then cites the assigned ID. The `## Later entries` sections of both files give the procedure.

### Rules governing the audit

- Total Parsers Over Untrusted Input, Typed Dates and Timestamps and Checked Integer Narrowing: their table-level requirements live in `copybooks.md`, namely the typed-error `PIC-` class rows that every public decoder returns as errors, the date and timestamp mapping (PIC-015 to PIC-021) and the 64-bit integral fields (PIC-023), and in `arithmetic.md`, whose `ARITH-` rows are the COBOL mandate that each `// TRUNCATION:` comment cites.
- Cardholder Data Redaction and Bounded Pagination: as stated under "Cardholder data" and "Messages and HTTP status".
- R16 — Authority and protected paths: `app/**` is read only, and every audit file is markdown under `modern/audit/`.

## Review procedure

A reviewer who knows the COBOL sources compares, for each audit file:

- every rule row against the cited paragraph and lines;
- every PIC-to-type row against the copybook;
- every fixture expectation against the rules applied to the cited record;
- every escalation resolution and every candidate.

The reviewer then records the decision in the status table below: `Accepted` or `Rejected`, with name and date (`YYYY-MM-DD`). Notes give the commit reviewed and, for `Rejected`, the rows to revise. An accepted file that changes after the reviewed commit needs a new decision before code that consumes the change is accepted. Agreement with each escalation resolution is recorded in the `Status` column of `ESCALATIONS.md`, and a halt changes status only as `HALTS.md` allows. Only the reviewer writes in the review status, E29 and halt acknowledgement tables below; no cell is pre-filled, defaulted or inferred.

A binding whose expectations come from an audit file counts as passed only once that file is `Accepted`: g1b, g3a, g3b, i2, i9, NAV, R8 and n5 through their audits, i8 and d9 through `copybooks.md` and `fixtures.md` (a9; AAP 0.9.1).

## Review status

| Audit file | Reviewer | Date | Decision (Accepted/Rejected) | Notes |
|---|---|---|---|---|
| `copybooks.md` | | | | |
| `arithmetic.md` | | | | |
| `fixtures.md` | | | | |
| `screens.md` | | | | |
| `online/COSGN00C.md` | | | | |
| `online/COMEN01C.md` | | | | |
| `online/COADM01C.md` | | | | |
| `online/COACTVWC.md` | | | | |
| `online/COACTUPC.md` | | | | |
| `online/COCRDLIC.md` | | | | |
| `online/COCRDSLC.md` | | | | |
| `online/COCRDUPC.md` | | | | |
| `online/COTRN00C.md` | | | | |
| `online/COTRN01C.md` | | | | |
| `online/COTRN02C.md` | | | | |
| `online/CORPT00C.md` | | | | |
| `online/COBIL00C.md` | | | | |
| `online/COUSR00C.md` | | | | |
| `online/COUSR01C.md` | | | | |
| `online/COUSR02C.md` | | | | |
| `online/COUSR03C.md` | | | | |
| `online/COPAUS0C.md` | | | | |
| `online/COPAUS1C.md` | | | | |
| `online/COPAUS2C.md` | | | | |
| `online/COTRTLIC.md` | | | | |
| `online/COTRTUPC.md` | | | | |
| `online/CSUTLDTC.md` | | | | |
| `batch/POSTTRAN.md` | | | | |
| `batch/INTCALC.md` | | | | |
| `batch/CREASTMT.md` | | | | |
| `batch/TRANREPT.md` | | | | |
| `batch/CBPAUP0J.md` | | | | |
| `batch/MNTTRDB2.md` | | | | |
| `batch/LOADS.md` | | | | |
| `messaging/CODATE01.md` | | | | |
| `messaging/COACCT01.md` | | | | |
| `messaging/COPAUA0C.md` | | | | |
| `ESCALATIONS.md` | | | | |
| `HALTS.md` | | | | |

## E29 — CP00 reply before write

COPAUA0C sends the CP00 reply outside the unit of work, before its IMS writes (E29). The replacement keeps that order by decision (AAP 0.3.4.2): it locks the account's summary row, decides, publishes the reply, waits for the confirmation and acknowledges the request, and only then writes and commits. The exposure that order carries is accepted only by this signature.

| Item | Exposure accepted | Reviewer | Date | Decision (Accepted/Rejected) |
|---|---|---|---|---|
| E29 — CP00 reply before write | (1) A delivered decision can go unrecorded: after an `I003` summary-write failure, after a commit failure, or after a process or connection loss that follows the acknowledgement, the requester holds the reply while nothing is committed. (2) An `I004` detail-insert failure commits the summary without its detail. (3) i10 counts only committed approvals, so the account's real exposure can exceed the committed figure by the approvals replied but never recorded. | | | |

- This row is satisfied only by its own `Accepted` decision with the reviewer's name and date. The general escalation review, the E29 row of `ESCALATIONS.md` and the `ESCALATIONS.md` row of the status table never satisfy it.
- `ESCALATIONS.md` shows E29 as `AWAITING SIGNATURE` until this row is `Accepted` with a name and date.
- A `Rejected` decision reopens the CP00 ordering decision of AAP 0.3.4.2; it does not pass with a note.

## Halt acknowledgements

| Halt | Reviewer | Date | Analysis acknowledged |
|---|---|---|---|
| H1 | | | |
| H2 | | | |
| H3 | | | |
| H4 | | | |
| H5 | | | |
| H6 | | | |

Acknowledging a halt confirms its analysis in `HALTS.md` and does not change its status. Only a source-backed resolution the reviewer confirms against its cited source (`RESOLVED`) or an explicit user scope change (`DESCOPED`) moves a halt out of `BLOCKED`. A halt folded in from H7 onward gets a row here when it is added to `HALTS.md`.

## Gate

`docs/SIGNOFF.md` cannot be fully checked while any file in the review status table is not `Accepted`, the E29 row is unsigned or `Rejected`, or any halt in `HALTS.md` is `BLOCKED` (a9, g8; AAP 0.8.6, 0.9.5).
