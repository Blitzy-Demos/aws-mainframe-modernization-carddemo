# CardDemo REST API

This document explains the REST API that replaces the CardDemo online CICS transactions. It is written for frontend developers, testers and the reviewer who knows the COBOL programs. The contract is [`../openapi/openapi.json`](../openapi/openapi.json): every path, method, operation ID, schema name and status below is taken from it, and where this document and the JSON differ, the JSON is right and this document is the defect. Message literals are not listed here; each lives in its owning program's audit file under [`../audit/online/`](../audit/online/). Where the crates sit and why is in [ARCHITECTURE.md](ARCHITECTURE.md), environment variables and commands are in [SETUP.md](SETUP.md), and the acceptance checklist is [SIGNOFF.md](SIGNOFF.md).

## Contents

1. [Overview](#1-overview)
2. [Endpoints](#2-endpoints)
3. [README transactions and their endpoints](#3-readme-transactions-and-their-endpoints)
4. [Error model](#4-error-model)
5. [Data formats](#5-data-formats)
6. [Pagination](#6-pagination)
7. [Cardholder data redaction](#7-cardholder-data-redaction)
8. [Sign-on](#8-sign-on)
9. [Endpoint-specific notes](#9-endpoint-specific-notes)
10. [Client-side keys](#10-client-side-keys)
11. [Contract and regeneration](#11-contract-and-regeneration)

## 1. Overview

- **Base path and format.** Every operation is under `/api/v1`, and every request and response body is JSON. The API listens on `CARDDEMO_API_BIND` (default `127.0.0.1:8080`); in development the browser reaches it through the Vite dev server, which proxies `/api` to that address, so the frontend sees one local origin. The document declares no `servers` entry and names no hosted service.
- **Tokens.** Every operation except endpoint 1 (`POST /api/v1/signon`, which issues the token) declares the security requirement `bearer_auth`, an HTTP bearer scheme with format `JWT`. Send `Authorization: Bearer <JWT>`, the `token` of the sign-on response.
- **Gates.** Each handler declares its gate as an extractor of [`crates/api/src/auth/extract.rs`](../crates/api/src/auth/extract.rs):
  - `SignedOnUser` (gate `user` in the table below) refuses a request with no valid token (missing, malformed, wrongly signed, another algorithm or expired) with 401.
  - `AdminUser` (gate `[A]`) refuses a request with no valid token with 401, exactly as `SignedOnUser` does, and a valid token whose `usr_type` is not `A` with 403, before the handler body runs. The frontend's route guard only hides the admin screens; this extractor is the control.
- **Identity.** The acting user comes only from the verified claims `sub` (user ID) and `usr_type`, never from a body, query string, path segment or other header, as the COBOL takes the user ID and type only from the COMMAREA sign-on set ([ARCHITECTURE.md, section 4](ARCHITECTURE.md#4-identity-and-trust)). Request fields that name users (the credentials of endpoint 1, the target user of endpoints 17–20, the start user ID of endpoint 16) are validated input and grant no authority.
- **The served document.** The running API also serves the contract at `GET /api-docs/openapi.json`. That route is added after the operations are collected, so it is not an operation of the document it serves.
- **Response headers.** Every response, errors, the router-level answers of section 4 and the document included, carries `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`. Responses carry bearer tokens (endpoint 1), the full card number of endpoint 8 for an admin and other cardholder and personal data, so no browser or intermediate cache may store them, and every body is JSON that a browser must not sniff as another type. The document does not declare these headers, because they are the same on every operation.
- **One endpoint per program paragraph.** The decomposition rule is one endpoint per online-program read paragraph and one per write paragraph, not one per resource and method:
  - AID keys that one paragraph serves become a body field `aid` (for example ENTER and PF5 of COACTUPC, `CoactupcAid`), rather than separate operations.
  - COUSR02C and COUSR03C share one user read, endpoint 18, because their read paragraphs carry identical failure literals.
  - The main and admin menus are separate endpoints because they carry different gates.
  - Decomposing per program keeps every response's messages owned by one program: COACTVWC's view (4) and COACTUPC's fetch (5) read the same account, and COCRDSLC (8) and COCRDUPC (9) the same card, yet each has its own edits and literals.

## 2. Endpoints

The 27 operations, in endpoint order. Gate `—` is open, `user` is `SignedOnUser`, `[A]` is `AdminUser`. Each operation's tag in the document is its program name, and its `operationId` is the handler module and the handler name joined by an underscore. `?` marks an optional member.

| # | Method and path | Gate | Transaction / program | AID actions served | Request → response | `operationId` |
|---|---|---|---|---|---|---|
| 1 | `POST /api/v1/signon` | — | CC00 / COSGN00C | ENTER | `SignonRequest` → `SignonResponse` (token, user ID, user type, next route) | `cosgn00c_signon` |
| 2 | `GET /api/v1/menus/main` | user | CM00 / COMEN01C | Screen load; the ENTER option checks are served from the response | → `MenuResponse` (the 11 options of `COMEN02Y`, each with its number, 35-byte name, program, user type `U`, display text and admin-only and installed flags, and the program's rejection texts) | `comen01c_main_menu` |
| 3 | `GET /api/v1/menus/admin` | [A] | CA00 / COADM01C | As 2 | → `MenuResponse` (the 6 options of `COADM02Y`, with no per-option user type: that table has no user-type column) | `coadm01c_admin_menu` |
| 4 | `GET /api/v1/accounts/{acct_id}/view` | user | CAVW / COACTVWC | ENTER | path `acct_id` → `AccountViewResponse` (account and customer); 404 and 500 → `AccountViewError` (section 4) | `coactvwc_view` |
| 5 | `GET /api/v1/accounts/{acct_id}` | user | CAUP / COACTUPC | ENTER (fetch) | path `acct_id` → `AccountUpdateView` (fields and the `AccountUpdateSnapshot` for change detection); 404 and 500 → `AccountUpdateViewError` (section 4) | `coactupc_fetch` |
| 6 | `PUT /api/v1/accounts/{acct_id}` | user | CAUP / COACTUPC | ENTER (validate), PF5 (save) | `AccountUpdateRequest { aid, fields, snapshot }` → `AccountUpdateResult` | `coactupc_save` |
| 7 | `GET /api/v1/cards` | user | CCLI / COCRDLIC | ENTER (filters), PF7/PF8 (paging) | `CardListQuery { acct_id?, card_num?, direction?, page_no?, first_key?, last_key?, has_next_page?, last_page_shown?, reread?, page_size? }` → `CardListResponse` (up to 7 `CardListRow`s of `{ card_ref, card_num (redacted), acct_id, status }`, page number, first and last keys as `card_ref` values) | `cocrdlic_list` |
| 8 | `GET /api/v1/cards/view` | user | CCDL / COCRDSLC | ENTER | `CardLookupQuery { acct_id, card_num? \| card_ref? }` (exactly one of the typed number and the list row's handle) → `CardViewResponse` | `cocrdslc_view` |
| 9 | `GET /api/v1/cards/edit` | user | CCUP / COCRDUPC | ENTER (fetch) | `CardLookupQuery` → `CardUpdateView` (redacted number, `card_ref`, fields, `CardUpdateSnapshot`) | `cocrdupc_fetch` |
| 10 | `PUT /api/v1/cards/{card_ref}` | user | CCUP / COCRDUPC | ENTER (validate), PF5 (save) | `CardUpdateRequest { aid, fields, snapshot }` → `CardUpdateResult` | `cocrdupc_save` |
| 11 | `GET /api/v1/transactions` | user | CT00 / COTRN00C | ENTER, PF7/PF8 | `TranListQuery { start_tran_id?, direction?, entry_key?, first_key?, last_key?, page_no?, has_next_page?, page_size? }` → `TranListResponse` (10 row slots) | `cotrn00c_list` |
| 12 | `GET /api/v1/transactions/{tran_id}` | user | CT01 / COTRN01C | ENTER | path `tran_id` → `TranViewResponse` | `cotrn01c_view` |
| 13 | `POST /api/v1/transactions` | user | CT02 / COTRN02C | ENTER (validate, confirm), PF5 (copy the last transaction, then ENTER) | `TranAddRequest { aid, acct_id?, card_num?, fields, confirm }` → `TranAddResult` (filled key, copied fields, message, new ID) | `cotrn02c_add` |
| 14 | `POST /api/v1/reports/transactions` | user | CR00 / CORPT00C | ENTER with confirmation | `ReportRequest` (monthly, yearly, custom, start and end date parts, confirm) → `ReportResult { message, job_deck? }` | `corpt00c_submit` |
| 15 | `POST /api/v1/bill-payments` | user | CB00 / COBIL00C | ENTER (blank confirmation shows the balance; `Y` pays; `N` clears) | `BillPaymentRequest { acct_id, confirm }` → `BillPaymentResult` (balance, message, new transaction ID) | `cobil00c_pay` |
| 16 | `GET /api/v1/users` | [A] | CU00 / COUSR00C | ENTER, PF7/PF8 | `UserListQuery { start_user_id?, page_key?, direction?, first_key?, last_key?, page_no?, has_next_page?, page_size? }` → `UserListResponse` (10 row slots) | `cousr00c_list` |
| 17 | `POST /api/v1/users` | [A] | CU01 / COUSR01C | ENTER | `UserAddRequest` → `ScreenResult` | `cousr01c_add` |
| 18 | `GET /api/v1/users/{user_id}` | [A] | CU02 / COUSR02C and CU03 / COUSR03C | ENTER (fetch) | path `user_id` → `UserView` (never a password) | `cousr02c_fetch` |
| 19 | `PUT /api/v1/users/{user_id}` | [A] | CU02 / COUSR02C | PF5 (update) | `UserUpdateRequest { first_name, last_name, password?, user_type }` → `ScreenResult` | `cousr02c_update` |
| 20 | `DELETE /api/v1/users/{user_id}` | [A] | CU03 / COUSR03C | PF5 (delete) | path `user_id` → `ScreenResult` | `cousr03c_delete` |
| 21 | `GET /api/v1/accounts/{acct_id}/authorizations` | user | CPVS / COPAUS0C | ENTER, PF7/PF8 | `AuthSummaryQuery { page_key?, direction?, page_size? }` → `AuthSummaryResponse` (customer and summary block, up to 5 rows) | `copaus0c_summary` |
| 22 | `GET /api/v1/authorizations/{acct_id}/{auth_key}` | user | CPVD / COPAUS1C | ENTER, PF8 (next authorization) | `AuthDetailQuery { direction? }` → `AuthDetailResponse` | `copaus1c_detail` |
| 23 | `POST /api/v1/authorizations/{acct_id}/{auth_key}/fraud` | user | CPVD / COPAUS1C → COPAUS2C | PF5 (mark or unmark fraud) | no body → `FraudToggleResult` | `copaus1c_toggle_fraud` |
| 24 | `GET /api/v1/transaction-types` | [A] | CTLI / COTRTLIC | Screen load, PF7/PF8 (paging under the displayed filters) | `TranTypeListQuery { type_filter?, desc_filter?, page_key?, direction?, page_size? }` → `TranTypeListResponse` (up to 7 rows of the Db2 data version, page number, `state`, `page_key`) | `cotrtlic_list` |
| 25 | `POST /api/v1/transaction-types/list-actions` | [A] | CTLI / COTRTLIC | ENTER, F10 | `TranTypeListActionRequest { aid: ENTER \| PF10, type_filter, desc_filter, rows: [up to 7 × { selection, description }], state, page_size? }` → `TranTypeListActionResult` (re-read page, per-row errors, message, what was committed, new `state`) | `cotrtlic_list_actions` |
| 26 | `GET /api/v1/transaction-types/{type_code}` | [A] | CTTU / COTRTUPC | ENTER (fetch) | path `type_code` → `TranTypeView` | `cotrtupc_fetch` |
| 27 | `POST /api/v1/transaction-types/{type_code}/actions` | [A] | CTTU / COTRTUPC | ENTER (validate the description), PF5 (save: update, insert when not found), PF4 (confirmed delete) | `TranTypeActionRequest { aid, description? }` → `TranTypeActionResult` | `cotrtupc_action` |

Notes on the table:

- **Schema names** are the `components.schemas` keys of the document, spelled exactly. The seven query types (`CardListQuery`, `CardLookupQuery`, `TranListQuery`, `UserListQuery`, `AuthSummaryQuery`, `AuthDetailQuery`, `TranTypeListQuery`) appear twice: as the operation's query parameters and, registered by [`crates/api/src/openapi.rs`](../crates/api/src/openapi.rs), as schemas, so the frontend can name them. Row types (`CardListRow`, `TranListRow`, `UserListRow`, `AuthSummaryRow`, `TranTypeListRow`), the snapshots and the state enums are schemas of their own, referenced from the bodies above.
- **Attention keys** are per-program enums: `CoactupcAid` and `CocrdupcAid` (ENTER, PF5), `Cotrn02cAid` (ENTER, PF5), `CotrtlicAid` (ENTER, PF10) and `CotrtupcAid` (ENTER, PF4, PF5). Paging keys are `PageDirection`: `forward` for F8, `backward` for F7, absent for ENTER. COTRN00C's screen bring-up key is `TranListEntryKey`.
- **No hand-written TypeScript restates these types.** The frontend refers to them only as `components["schemas"][…]` of the generated [`../frontend/src/api/schema.d.ts`](../frontend/src/api/schema.d.ts) or by inference from its `paths` (section 11).

## 3. README transactions and their endpoints

The 24 online transactions of the README inventory ([`README.md:269-294`](../../README.md)) map to the 27 endpoints as follows. The decomposition rule of section 1 produces the count; it is not a list agreed separately.

| README transactions | Endpoints |
|---|---|
| CC00 | 1 |
| CM00, CA00 | 2, 3 |
| CAVW | 4 |
| CAUP | 5, 6 |
| CCLI | 7 |
| CCDL | 8 |
| CCUP | 9, 10 |
| CT00 | 11 |
| CT01 | 12 |
| CT02 | 13 |
| CR00 | 14 |
| CB00 | 15 |
| CU00 | 16 |
| CU01 | 17 |
| CU02 | 18, 19 |
| CU03 | 18, 20 |
| CPVS | 21 |
| CPVD | 22, 23 |
| CTLI | 24, 25 |
| CTTU | 26, 27 |
| CP00, CDRD, CDRA | No REST endpoint: they are the RabbitMQ workers of `carddemo-mq` ([ARCHITECTURE.md, section 10](ARCHITECTURE.md#10-messaging-flows)) |

No online transaction sends a broker message, so the API never publishes to RabbitMQ.

## 4. Error model

Every non-200 response that the request layer, the gate, a program or the router produces has the body `ScreenError { message, field? }`, with `content-type: application/json`; `field` is omitted when absent. The HTTP status travels as the response status only and is no member of the body. Endpoints 4 and 5 add optional members beside `message` and `field` on their 404 and 500, and endpoints 24 and 25 are the one exception to the body shape; both are described after the table. The router's own answers follow the table.

| Status | Meaning | Declared by endpoints |
|---|---|---|
| 400 | Input: a program edit rejected the request, or the request layer rejected a body, query or path that does not deserialise or breaks a map length | 1, 4–27 |
| 401 | Sign-on failure (endpoint 1), or no valid token (every other endpoint) | 1–27 |
| 403 | Admin gate: a valid token whose `usr_type` is not `A` | 3, 16–20, 24–27 |
| 404 | A read found no record | 4–6, 8, 9, 12, 13, 15, 18–23, 25 |
| 409 | The record changed after it was fetched, by another user | 6, 10 |
| 422 | `page_size` that is not an integer from 1 to 100 (section 6); the only use of 422 | 7, 11, 16, 21, 24, 25 |
| 500 | Store failure | 1, 4–27 |
| 503 | Password work saturated: the argon2id computation found no place in the API's password-work budget (`PasswordWork` in [`crates/api/src/app.rs`](../crates/api/src/app.rs)) within its 3-second wait, so nothing was verified or written; the body is `UNABLE_TO_PROCESS` with no `field`, and `Retry-After: 3` tells the client when to send the request again | 1, 17, 19 |

- **Messages are the owning program's literals.** A program's message is moved into `message` exactly as the program moves it, trailing spaces included, with the status that program's audit assigns to it. The literals, their conditions and their statuses are the rule rows of [`../audit/online/`](../audit/online/), one file per program. `field` is the wire name of the request member the program puts the cursor on, as a path such as `user_id`, `fields.zip` or `rows[3].description`.
- **Texts that are not COBOL literals.** Four kinds of message belong to the API crate, not to a program, and are named by constant rather than listed here: the gate texts `SIGNON_REQUIRED` and `ADMIN_REQUIRED` ([`crates/api/src/auth/extract.rs`](../crates/api/src/auth/extract.rs)); the request-layer rejections `INVALID_BODY`, `INVALID_QUERY`, `INVALID_PATH`, `FIELD_LENGTH_MESSAGE`, `FIELD_NUMERIC_MESSAGE`, `FIELD_RANGE_MESSAGE` and `PAGE_SIZE_MESSAGE` ([`crates/api/src/dto/mod.rs`](../crates/api/src/dto/mod.rs)); `UNABLE_TO_PROCESS`, the 500 of a store or internal failure that no program words and the 503 of a saturated password-work budget; and the router-level texts `NO_SUCH_ROUTE`, `METHOD_NOT_SERVED` and `SERVER_BUSY` (both in [`crates/api/src/error.rs`](../crates/api/src/error.rs)). Every gate 401 and every 403 has the same text whatever the cause, so the response reveals nothing about the token it refuses; the 401s of endpoint 1 are COSGN00C's own sign-on messages (section 8).
- **No store detail.** A store failure never carries SQL text, an SQLSTATE, a constraint or column name, or a key value: the persistence layer keeps only the SQLSTATE and constraint name, and the API logs those once and answers with the program's own text or `UNABLE_TO_PROCESS`. No rejection quotes the value it rejected.
- **Endpoints 4 and 5 show the blocks the program shows beside a message.** After a partial read COACTVWC and COACTUPC still show the record a read found beside the other read's message ([`../audit/online/COACTVWC.md`](../audit/online/COACTVWC.md) COACTVWC-008, X-COACTVWC-6; [`../audit/online/COACTUPC.md`](../audit/online/COACTUPC.md) COACTUPC-099). Their 404 and 500 bodies are therefore `AccountViewError` and `AccountUpdateViewError`: the `ScreenError` members unchanged, each schema `allOf` `ScreenError` and optional members beside it, so a body with none of them is exactly a `ScreenError`. Endpoint 4 adds `account` (the `AccountViewAccountBlock`) when the account was found and the customer read did not succeed, and both `account` and `customer` (the `AccountViewCustomerBlock`) when the account read did not succeed and the customer was found; that account block holds placeholder values, halt candidate X-COACTVWC-H1, which no test asserts. Endpoint 5 adds `state` `S` and `details`, an `AccountUpdateView` with its snapshot, only when the account read did not succeed and the customer was found; the screen then shows the details with their fields enterable, and the account part of the details is a placeholder, halt candidate X-COACTUPC-H1. Every other outcome, and every 400 and 401, is a bare `ScreenError`. The SSN in these blocks is the redacted form of section 7.
- **Endpoints 24 and 25 answer with the page.** COTRTLIC sends the whole screen with every message ([`app/app-transaction-type-db2/cbl/COTRTLIC.cbl:699-720`](../../app/app-transaction-type-db2/cbl/COTRTLIC.cbl)), so on 400, 404 and 500 these endpoints return `TranTypeListResponse` or `TranTypeListActionResult`, carrying the message, the per-row errors and the `state`, under the audited status. A request the request layer rejects, a `page_key` or `state` this API did not issue, a database that cannot be reached at all, an F10 commit whose transaction cannot be opened, committed or rolled back or whose re-read of the target row fails, and every 401, 403 and 422 answer with a `ScreenError` instead. The document therefore declares the 400 and 500 of both endpoints as `anyOf` the page schema and `ScreenError`, and endpoint 25's 404 as the page alone. The union is `anyOf`, not `oneOf`, because a page that carries its message also satisfies `ScreenError`, whose schema requires only a string `message`. A client reads the status first and never assumes that a page body means success.
- **Endpoints 26 and 27 have no 404 or 409.** A type code no row holds is a 200 that offers the add path, and COTRTUPC never sets its changed-by-another-user text; the delete refused while categories still exist is a 500.

**Router-level answers.** These come from the router in [`crates/api/src/app.rs`](../crates/api/src/app.rs), before any handler, extractor or token check runs, so they are not operations of the document and no entry declares them. Each is a `ScreenError` with a fixed text of [`crates/api/src/error.rs`](../crates/api/src/error.rs) and no `field`; none echoes the method, path or query.

| Status | When | `message` | Other headers |
|---|---|---|---|
| 404 | No route matches the path, such as `/api/v1/nope` or `/api/v1/users/` (trailing slash) | `NO_SUCH_ROUTE` | — |
| 405 | A route matches the path but serves no operation for the method, such as `GET /api/v1/signon` | `METHOD_NOT_SERVED` | `Allow`, listing the route's methods; `GET` implies `HEAD`, so `PATCH /api/v1/accounts/{acct_id}` gets `Allow: GET,HEAD,PUT` |
| 503 | The API already works on `MAX_IN_FLIGHT` (256) requests at once, on any route; the request waits, in arrival order and before its token, parameters or body are read, up to `ADMISSION_WAIT` (5 s) for a place, is served as soon as one frees, and gets this answer only when none frees in that time | `SERVER_BUSY` | `Retry-After: 1` |

A 503 is not an answer of the operation: nothing was read or written, so the client may send the same request again once the `Retry-After` second has passed. [SETUP.md, section 4](SETUP.md#4-api) explains the limit.

**Halted message rows are `BLOCKED`, not equivalent.** Some messages contain text the COBOL does not define. The API returns them as the domain renders them, but no test asserts such a message as a parity result, and every binding that covers it is reported `BLOCKED` until [`../audit/HALTS.md`](../audit/HALTS.md) marks the halt `RESOLVED` from a cited source or `DESCOPED` by the user:

- **H3:** Db2 `SQLCODE` and `SQLERRM` text after the program's own prefix in COTRTUPC's and COTRTLIC's unexpected-SQL messages (endpoints 24–27), the delete-restricted message of endpoint 27 included.
- **H4:** CICS `RESP`/`RESP2` codes and IMS status codes inside error texts, such as COACTVWC's `File Error:` text and the not-found texts of COACTVWC, COACTUPC and COPAUS0C (endpoints 4–6, 8–10, 21). Only the code positions are blocked; the fixed text, status and field around them stay asserted.
- **H5:** the date-validity outcome of COACTUPC, COTRN02C and CORPT00C (endpoints 6, 13, 14), which CSUTLDTC delegates to Language Environment `CEEDAYS`.

## 5. Data formats

- **Money is text.** Every amount, balance and limit is a string already formatted with the COBOL output picture its program's audit records, at the map field's width: for example the 12 characters of `PIC +99999999.99` for a transaction amount (endpoints 11, 12) and the 14 characters of `PIC +9999999999.99` for the bill-payment balance (endpoint 15). Amounts in requests are the raw text the screen field holds, which the program edits with its own messages. No JSON number carries money, and the frontend does no numeric formatting. The integers of the document are keys, handles and counters only: `card_ref`, `page_no`, `page_size`, row positions.
- **Dates and timestamps are typed.** Fields that come from the `PIC X(10)` date and `PIC X(26)` timestamp copybook fields are `format: date` or `format: date-time`, and `null` when the stored field is blank (Typed Dates and Timestamps): the account dates of `AccountViewResponse`, `AccountUpdateView` and `AccountUpdateSnapshot`; `expiration_date` of `CardViewResponse`, `CardUpdateView` and `CardUpdateSnapshot`; `orig_date` and `proc_date` of `TranAddCopiedFields`; `orig_ts` of `TranListRow`; `orig_ts` and `proc_ts` of `TranViewResponse`. The timestamp's stored text-layout code is a storage detail and is not sent.
- **Raw screen input to a COBOL date edit stays text.** Typing these request members would replace the program's own edit messages with a deserialisation error, so they carry the field as typed and the program's edit turns them into a date:
  - endpoint 6: the open, expiry, reissue and birth date parts of `AccountUpdateFields` (`*_day`, `*_month`, `*_year`), edited by COACTUPC;
  - endpoint 13: `orig_date` and `proc_date` of `TranAddFields`, edited by COTRN02C;
  - endpoint 14: the start and end date parts of `ReportRequest`, edited by CORPT00C;
  - endpoint 10 follows the same rule for `expiry_month` and `expiry_year` of `CardUpdateFields`, the separate numeric fields COCRDUPC edits with its own messages.

  The dates the responses of these endpoints carry are typed.
- **Display strings that are not copybook dates.** The authorization date and time of endpoints 21 and 22 come from `PIC X(06)` fields ([`app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:22-23`](../../app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy)) and travel as the text the program shows. `TranListRow.display_date` is COTRN00C's 8-character rendering, sent beside the typed `orig_ts`.
- **Text keeps its width.** A display field carries every byte of its map field, trailing spaces and leading zeros included, so the screen pads and trims nothing.
- **Validation lengths come from the BMS maps.** Each request member's `maxLength` is its map field's `LENGTH` ([`../audit/screens.md`](../audit/screens.md)), and numeric-only map fields are checked as digits. A longer value is a 400 naming the member. Most request members are optional in the schema, so an omitted one reaches the program as an empty field and gets the program's own blank-field message, not a deserialisation error.

## 6. Pagination

Six operations are collections under Bounded Pagination. Each takes `page_size`, an integer from 1 to 100 (`minimum: 1`, `maximum: 100` in the document), and reads the program's screen size when it is absent:

| # | Operation | Program | Default `page_size` | `page_size` travels in | Paging state the client echoes |
|---|---|---|---|---|---|
| 7 | `GET /api/v1/cards` | COCRDLIC | 7 | query | `page_no`, `first_key` and `last_key` (`card_ref` handles), `has_next_page` and `last_page_shown`; `direction` for F7/F8 and `reread` for the re-read after a selection error |
| 11 | `GET /api/v1/transactions` | COTRN00C | 10 | query | `first_key` and `last_key` (transaction IDs), `page_no` and `has_next_page`; `direction` for F7/F8, `entry_key` on the request that brings the screen up, neither for ENTER |
| 16 | `GET /api/v1/users` | COUSR00C | 10 | query | `first_key` and `last_key` (user IDs), `page_no` and `has_next_page`; `page_key`, when sent, is the key an F7 or F8 reads from and takes the place of the echoed key of its direction |
| 21 | `GET /api/v1/accounts/{acct_id}/authorizations` | COPAUS0C | 5 | query | `page_key` and `direction` only; the page number and each page's first key are the screen's own paging state |
| 24 | `GET /api/v1/transaction-types` | COTRTLIC | 7 | query | `page_key`, an opaque encoding of the `TranTypeListState` of the previous response, with `direction` |
| 25 | `POST /api/v1/transaction-types/list-actions` | COTRTLIC | 7 | body | `state`, the `TranTypeListState` of the previous response, unchanged |

- **Out of range is 422.** Every `page_size` that is not an integer from 1 to 100 is answered 422 with a `ScreenError` whose message is `page_size must be from 1 to 100...` and whose `field` is `page_size`, whatever other member rule failed: 0, a negative value or 101; an integer beyond the 64-bit range however many digits it has, such as `9223372036854775808` or `-9223372036854775809`; and a value that is no integer at all, such as `abc`, an empty `page_size=` or `1.5`, and in endpoint 25's body also a number beyond the range the JSON parser represents, such as `1e400`, a string, a boolean, an array or an object. A JSON `null` is an absent `page_size`. The rejection never echoes the value. A request with another member that does not deserialise, such as an unknown `direction`, is still the request layer's 400, whatever its `page_size`. No other condition produces 422.
- **Parity is unchanged.** The screens never send `page_size`, so every paging result the audits state holds for the program's screen size. A page holds at most `page_size` rows and never more than are stored; COTRTLIC's page is also bounded by the seven row positions of its map.
- **Paging state is echoed, not trusted.** The COBOL kept its paging state in the COMMAREA between screens. Each collection response returns that state, and the screen sends it back with its next request in the members the table lists; keys, `page_key` and `state` are opaque values that the client forwards exactly as a response carried them. The state grants nothing: every request re-reads the table under the filters it carries.
- **Every page is served by an index.** Each paginated query is a committed SQL file under [`../crates/persistence/queries/`](../crates/persistence/queries/), read by `sqlx::query_file_as!`. [`crates/persistence/tests/explain.rs`](../crates/persistence/tests/explain.rs) plans every file with `enable_seqscan = off` and fails on any sequential scan, and [`crates/api/tests/pagination.rs`](../crates/api/tests/pagination.rs) sends `page_size=101` to all six operations, asserts 422, and checks each default.
- **Not collections.** The menus (2, 3) are fixed option tables compiled from their copybooks, single resources with no stored rows to page. Endpoints 8, 9, 12, 18, 22 and 26 address one record; endpoint 22's `direction=forward` (F8) moves to the next authorization one at a time and takes no `page_size`.

## 7. Cardholder data redaction

No full card number, SSN, CVV or password leaves the API in a response body, an error, a trace or the OpenAPI document, with one disclosure on endpoint 8. The COBOL shows card numbers and SSNs in full; the redacted forms are the departure escalation E32 records ([`../audit/ESCALATIONS.md`](../audit/ESCALATIONS.md)). The types behind it are in [`crates/domain/src/sensitive.rs`](../crates/domain/src/sensitive.rs), and [ARCHITECTURE.md, section 11](ARCHITECTURE.md#11-cardholder-data-redaction) covers the other egress paths.

- **Redacted forms.** A card number is twelve `*` followed by its last four digits, 16 characters, such as `************5740`. An SSN is `***-**-` followed by its last four digits, such as `***-**-1234`.
- **Where each field is redacted:**

| Endpoint | Field | What is sent |
|---|---|---|
| 4 | `AccountViewResponse.ssn` | Redacted SSN |
| 5 | `AccountUpdateView.ssn`; `AccountUpdateSnapshot.ssn_last4` | Redacted SSN; the snapshot holds the last four digits only |
| 6 | `AccountUpdateFields.ssn_part1`, `ssn_part2`, `ssn_part3` | Write-only input, sent only when re-entered; omitting all three means unchanged. COACTUPC's changed-by-another-user check compares the SSN by its last four digits, because the echoed snapshot holds no more (E32) |
| 7 | `CardListRow.card_num`; `first_key`, `last_key` | Redacted card number; keys are `card_ref` handles |
| 8 | `CardViewResponse.card_num` | All 16 digits for a token with `usr_type` `A`; the redacted form for any other token |
| 9 | `CardUpdateView.card_num` | Redacted card number, beside the `card_ref` endpoint 10 takes |
| 10 | — | No card number: the card is the path's `card_ref`, and `CardUpdateSnapshot` holds none |
| 12 | `TranViewResponse.card_num` | Redacted card number, for every user type |
| 13 | `TranAddResult.card_num` | Redacted card number, the key COTRN02C fills in from the cross-reference |
| 21 | `AuthSummaryRow.transaction_id` (also `auth_type`, `match_status`) | No card number. The free-text Transaction ID CP00 stored from the request is masked as on 22, so COPAU00 and COPAU01 show one stored ID the same way: `TXN000000000001` shows as `TXN********0001` on both |
| 22, 23 | `AuthDetailResponse.card_num` (23 returns it inside `FraudToggleResult.detail`) | Redacted card number. The free-text fields CP00 stored from the request (source, merchant fields, transaction ID and others) are masked too: a written run of nine digits shows in the SSN form, one of sixteen in the card form |

- **The single card-detail endpoint.** Endpoint 8 is the only operation that may return all 16 digits, through `DisclosedCardNumber`, the workspace's one full-value serialiser. `api::services::cocrdslc` decides the disclosure from the verified token's `usr_type` alone, so no request field can ask for it. The rule's owner clause never grants disclosure, because no CardDemo user is linked to a customer, account or card.
- **Handles instead of numbers.** Card list rows and paging keys carry `card_ref`, the opaque identity of the `cards` row. A COCRDLI selection passes it to COCRDSL and COCRDUP, and endpoints 8 and 9 accept it in place of a typed number; endpoint 10 addresses the card by it.
- **Input is not egress.** A card number typed into a filter or key field (`card_num` of `CardListQuery`, `CardLookupQuery` and `TranAddRequest`) is accepted, marked `writeOnly` in the schema, never serialised back and masked in every diagnostic.
- **Passwords.** `SignonRequest.password`, `UserAddRequest.password` and `UserUpdateRequest.password` are write-only. Endpoint 18's `UserView` has no password member, so the update screen never pre-fills one; on endpoint 19 an omitted or `null` password means unchanged, and a supplied one is verified against the stored argon2id hash to decide whether it changed (E27). Passwords are stored only as that hash.
- **No CVV.** No schema property or JSON key names a card verification code.
- **Documentation and traces.** Every `example` in the document uses redacted values. A request span records the method, the matched route template, the status and the latency, never the URI, the query string, a header or a body, because paths and queries carry account IDs and card numbers.
- **Proof.** [`crates/api/tests/redaction.rs`](../crates/api/tests/redaction.rs) drives all 27 endpoints with a `U` token and an `A` token under trace capture, searches every body and trace for the fixture card numbers, SSNs and submitted passwords, checks that no member names a CVV or password, scans the emitted and committed documents, and pins the full number to endpoint 8's `A` response.

## 8. Sign-on

Endpoint 1 reproduces COSGN00C ([`app/cbl/COSGN00C.cbl:117-136,221-257`](../../app/cbl/COSGN00C.cbl)):

1. **Blank checks.** A blank user ID is rejected first, then a blank password, each with 400 and the field it names (COSGN00C-007, COSGN00C-008). An omitted member counts as blank.
2. **Upper-casing.** The user ID and the password are upper-cased as 8-byte fields.
3. **Verification.** The user row is read by the upper-cased ID, and the upper-cased password, space-padded to 8 bytes, is verified with argon2id against `users.password_hash`. A password that does not verify and an unknown user ID are 401; a failed read is 500 (COSGN00C-018 to COSGN00C-020). A user stored with lower-case letters never signs on, as in the COBOL (E26).
4. **Success.** The response carries an HS256 JWT whose claims are `sub` (the user ID), `usr_type`, `iat` and `exp`. The lifetime is `CARDDEMO_JWT_TTL_SECONDS` (default 3600), with no leeway after `exp`, and the signing secret is `CARDDEMO_JWT_SECRET`, without which the API refuses to start. `next_route` is `/admin` for user type `A` and `/menu` for every other type.

The token is the whole session: the API reads no store to check it, so it stays valid until `exp` even if the user is changed or deleted meanwhile, as the COMMAREA identity did. The local users `ADMIN001` (type `A`) and `USER0001` (type `U`) come from the `dusrsecj` load; [SETUP.md](SETUP.md) describes how to load them.

## 9. Endpoint-specific notes

- **Endpoint 14, report submission.** CORPT00C writes an in-line JCL deck to the transient data queue `JOBS`, one 80-byte record per line up to `/*EOF` ([`app/cbl/CORPT00C.cbl:125-127,501-518`](../../app/cbl/CORPT00C.cbl)). Endpoint 14 writes the same records, as the audit derives them for the chosen dates, to `CARDDEMO_INTRDR_DIR` (default `./var/jobs/`) as the next generation file `TRNRPT00.G<nnnn>`. `job_deck` is present only when a deck was written and holds that file name alone, such as `TRNRPT00.G0001`, never an absolute path. A confirmation of `N` clears the screen with an empty message and no `job_deck`. The endpoint does not run the report: the operator runs `carddemo-batch tranrept`, as JES would have. No endpoint serves report or statement files, because they hold full card numbers.
- **Endpoints 5, 6, 9 and 10, update snapshots.** COACTUPC and COCRDUPC kept the fetched values and their change state in the COMMAREA. The fetch (5, 9) returns them as `AccountUpdateSnapshot` or `CardUpdateSnapshot`, and every validate or save request echoes the snapshot: on endpoint 6 the client writes the result's `state` into `snapshot.state` and echoes every other value unchanged; on endpoint 10 it echoes the `snapshot` of the last 200, setting only `state` after an error. The snapshot grants nothing: a save re-reads the record and compares it with the snapshot, and a difference is the 409.
- **Endpoint 18, the shared user read.** `UserView` carries both programs' success prompts, `update_prompt` (COUSR02C) and `delete_prompt` (COUSR03C), each verbatim from its own program; the update screen shows one and the delete screen the other.
- **Endpoints 21 to 23, authorization keys.** `auth_key` is the key an `AuthSummaryRow` carries; the screen forwards it unchanged into the paths of 22 and 23. On endpoint 22, `backward` is no key of the screen and is answered as ENTER.
- **Endpoints 24 and 25, the transaction-type list.** Both read the Db2 data version of the types, `db2.transaction_type`, never the VSAM version that `tranrept` reads ([ARCHITECTURE.md, section 12](ARCHITECTURE.md#12-two-transaction-type-data-versions)).
  - `TranTypeListState` stands in for COTRTLIC's COMMAREA: the filters of the displayed page, the first and last keys and the page number, the seven displayed rows, the selected row and the pending action. Every list response returns it as `state`, which the client echoes unchanged with ENTER and F10, and as the opaque `page_key`, which it echoes with F7 and F8. The state carries no authority: every commit re-reads its row and the request is edited again.
  - ENTER runs COTRTLIC's edits in its order: the selections, then the description filter, then the type filter ([`app/app-transaction-type-db2/cbl/COTRTLIC.cbl:960-971`](../../app/app-transaction-type-db2/cbl/COTRTLIC.cbl)). A valid single `U` or `D` returns the confirmation prompt with the action pending in `state`.
  - F10 (`aid` `PF10`) commits the pending update or delete only when one is pending and the type filter, the description filter and the selected row are all unchanged; otherwise it is processed exactly as ENTER ([`app/app-transaction-type-db2/cbl/COTRTLIC.cbl:666-675`](../../app/app-transaction-type-db2/cbl/COTRTLIC.cbl)).
  - Selections are never cleared when a filter changes. They are edited against the state as received, then keep their row positions on page 1 of the new filter, because the program's clearing branch is never taken (E28).
  - The F2 transfer to the maintenance screen and F3 call no endpoint.
- **Endpoints 26 and 27, transaction-type maintenance.** Both work on the Db2 data version. `TranTypeView` and `TranTypeActionResult` return COTRTUPC's `TranTypeMaintState`, which the screen holds between requests, but the request does not carry it back: the server derives the state each key is served in from the key and the row it re-reads for `{type_code}`, so an echoed state decides nothing, and a `state` member a body still sends is ignored. ENTER edits the typed description and writes nothing; PF5 saves it, updating the row or inserting one when no row holds the code; PF4 is the second F4, after the confirmation prompt the screen shows on the first without a request, and deletes the row. Any other key name gets COTRTUPC's own invalid-key 400, not a body rejection.

## 10. Client-side keys

These keys and selections send no request; the screens perform them from the routes of [`../frontend/src/routes.tsx`](../frontend/src/routes.tsx):

- PF3, back or exit, on every screen, to the screen that transferred to it or to its menu;
- PF4 where it only clears the screen (COTRN01, COTRN02, COBIL00, COUSR01, COUSR02, COUSR03);
- PF12, cancel;
- PF2 on COTRTLI, to COTRTUP;
- the selections that only navigate: `S` and `U` on COCRDLI, `S` on COTRN00 and COPAU00, `U` and `D` on COUSR00;
- the ENTER option choice on both menus, checked against the `MenuResponse` already loaded.

A COCRDLI selection passes the row's `acct_id` and `card_ref` in route state to COCRDSL or COCRDUP, which call endpoint 8 or 9 with `card_ref`. Browsers reserve some function keys (F5 reloads, F12 opens developer tools), so every PF action is also a button labelled as on the map, and the end-to-end tests click the buttons.

## 11. Contract and regeneration

Both contract files are generated and never edited by hand (Generated API Types Only):

1. `cargo run -p api --bin carddemo-openapi` prints the document built from the handlers: `api::app::router()` registers each handler in utoipa-axum's `OpenApiRouter` through one `api::app::operation` call over the `utoipa::Path` metadata its `#[utoipa::path]` generates, the equivalent of `routes!` without the lint suppressions of that macro's expansion ([E46](../audit/ESCALATIONS.md)), so every operation in the document is exactly one handler. The binary reads no environment and opens no connection, so it runs offline and emits the same bytes on every build.
2. [`../scripts/regen-api.sh`](../scripts/regen-api.sh), run from `modern/`, writes that output to [`../openapi/openapi.json`](../openapi/openapi.json), runs openapi-typescript 7.13.0 (`pnpm --dir frontend exec openapi-typescript ../openapi/openapi.json -o src/api/schema.d.ts`) to write [`../frontend/src/api/schema.d.ts`](../frontend/src/api/schema.d.ts), and fails when either file differs from the committed version. Step 11 of `scripts/check.sh` runs it. After a handler or DTO change, one run leaves the new contract in the working tree to review and commit, and the next run passes.

The frontend reaches the API only through [`../frontend/src/api/client.ts`](../frontend/src/api/client.ts), an openapi-fetch `createClient<paths>()` client that adds the bearer token and clears the session on a 401, and [`../frontend/src/api/hooks.ts`](../frontend/src/api/hooks.ts), one TanStack Query hook per operation keyed by its `operationId`. Payload types are `components["schemas"][…]` of `schema.d.ts` or inferred from its `paths`; [`../scripts/frontend-policy.mjs`](../scripts/frontend-policy.mjs) fails the build when any other file under `frontend/src` declares a type named after a schema or aliases a generated type.

The API's own tests:

| Test | Binding | What it proves |
|---|---|---|
| [`crates/api/tests/openapi_contract.rs`](../crates/api/tests/openapi_contract.rs) | g1a | The document holds exactly the 27 operations with these operation IDs and gates, the committed file equals the emitted one, and every operation answers with a status its entry declares |
| [`crates/api/tests/auth_gate.rs`](../crates/api/tests/auth_gate.rs) | R8, n5 | `ADMIN001` and `USER0001` sign on with user types `A` and `U` and next routes `/admin` and `/menu`; the sign-on failures hold; every bearer operation answers 401 without a valid token, every `[A]` operation 403 to a `U` token |
| `crates/api/tests/endpoints_*.rs` (8 files) | g1b | Every `server`-layer rule row of [`../audit/online/`](../audit/online/), and the server half of every `both` row there and in [`../audit/screens.md`](../audit/screens.md), through the endpoint that serves it; rows a halt covers are reported `BLOCKED` |
| [`crates/api/tests/pagination.rs`](../crates/api/tests/pagination.rs) | Bounded Pagination | Section 6 |
| [`crates/api/tests/redaction.rs`](../crates/api/tests/redaction.rs) | Cardholder Data Redaction | Section 7 |
