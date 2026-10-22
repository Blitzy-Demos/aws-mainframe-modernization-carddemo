//! The REST replacement of the `CardDemo` online CICS programs (AAP 0.3.4.1): the 27 operations
//! under `/api/v1` that stand in for their pseudo-conversational screen exchanges, plus
//! `GET /api-docs/openapi.json`, which serves the `OpenAPI` document of those operations and is
//! not itself one of them.
//!
//! Two binaries are built on this crate. `carddemo-api` serves [`app::app`] on
//! `CARDDEMO_API_BIND`. `carddemo-openapi` prints [`app::openapi`], the document that
//! `scripts/regen-api.sh` writes to `modern/openapi/openapi.json` and from which the frontend's
//! `schema.d.ts` is generated (Generated API Types Only).
//!
//! # Modules
//!
//! - [`app`]: [`app::AppState`], [`app::router`], which registers every handler and splits the
//!   axum router from the document, [`app::openapi`] and [`app::app`].
//! - [`auth`]: the COMMAREA identity (`CDEMO-USER-ID`, `CDEMO-USER-TYPE`, `COCOM01Y.cpy:25-28`)
//!   as a signed token: [`auth::jwt::JwtKeys`] and [`auth::jwt::Claims`], and the extractors
//!   [`auth::extract::SignedOnUser`] and [`auth::extract::AdminUser`].
//! - [`dto`]: the request and response contracts, one module per program, with the validating
//!   extractors and the `page_size` rule of Bounded Pagination.
//! - [`error`]: [`error::ScreenError`], the `{ message, field? }` body of every non-200 response,
//!   and the status each failure answers with.
//! - [`openapi`]: [`openapi::ApiDoc`], the base document every operation is added to.
//! - `handlers` and `services`, crate-private: one module per program each, reached only through
//!   [`app::router`].
//!
//! Nothing is re-exported, so every item has exactly one path, such as
//! `api::auth::extract::AdminUser` or `api::dto::cocrdslc::CardViewResponse`.
//!
//! # Layering (Thin Request Handlers)
//!
//! - **Handlers.** `handlers::<program>` extracts the validated DTO and the [`SignedOnUser`] or
//!   [`AdminUser`], calls one `services::<program>` function, and maps its result through the
//!   `From` conversions of [`dto`] and [`error`]. A handler does no arithmetic on a domain value,
//!   branches on no domain outcome, composes no message and names neither `persistence` nor
//!   `sqlx`.
//! - **Services.** `services::<program>` loads through the `persistence` repository traits,
//!   applies the rules of `domain::online::<program>`, persists what they decide, and returns the
//!   domain outcome. Services are `pub(crate)`, so only the handlers reach them (AAP 0.5.4).
//! - **Identity.** Who acts, and with which user type, comes only from the verified JWT claims
//!   that the extractors check, never from a body, a query string or another header (AAP 0.5.1).
//!   A missing or invalid token answers 401, and a non-admin token on an `[A]` operation 403.
//!
//! # Boundaries
//!
//! - The crate never applies migrations: the schema is changed only by `carddemo-migrate`, and
//!   the API fails at its first query against an unmigrated database (Single Schema Executor).
//! - It never publishes to `RabbitMQ`, because no online transaction sends a message (AAP
//!   0.5.3).
//! - It issues no SQL itself: every statement runs inside `persistence` (R4).
//! - Its only filesystem write is endpoint 14's job deck, the next `TRNRPT00` generation in
//!   `CARDDEMO_INTRDR_DIR` (AAP 0.3.4.4).
//! - Card numbers and SSNs leave only in redacted form, and passwords and CVVs never leave,
//!   whether in a response, an error, a span or the document. The one exception is endpoint 8
//!   for an admin token, which returns the full card number (Cardholder Data Redaction, AAP
//!   0.3.4.8).
//!
//! # Endpoints
//!
//! `[A]` marks the operations behind the [`AdminUser`] gate; every other operation except 1
//! takes a [`SignedOnUser`]. The module column names the program module of `dto`, `handlers`
//! and `services`.
//!
//! | # | Method and path | Transaction / program | Module |
//! |---|---|---|---|
//! | 1 | `POST /api/v1/signon` | CC00 / COSGN00C | `cosgn00c` |
//! | 2 | `GET /api/v1/menus/main` | CM00 / COMEN01C | `comen01c` |
//! | 3 | `GET /api/v1/menus/admin` `[A]` | CA00 / COADM01C | `coadm01c` |
//! | 4 | `GET /api/v1/accounts/{acct_id}/view` | CAVW / COACTVWC | `coactvwc` |
//! | 5 | `GET /api/v1/accounts/{acct_id}` | CAUP / COACTUPC | `coactupc` |
//! | 6 | `PUT /api/v1/accounts/{acct_id}` | CAUP / COACTUPC | `coactupc` |
//! | 7 | `GET /api/v1/cards` | CCLI / COCRDLIC | `cocrdlic` |
//! | 8 | `GET /api/v1/cards/view` | CCDL / COCRDSLC | `cocrdslc` |
//! | 9 | `GET /api/v1/cards/edit` | CCUP / COCRDUPC | `cocrdupc` |
//! | 10 | `PUT /api/v1/cards/{card_ref}` | CCUP / COCRDUPC | `cocrdupc` |
//! | 11 | `GET /api/v1/transactions` | CT00 / COTRN00C | `cotrn00c` |
//! | 12 | `GET /api/v1/transactions/{tran_id}` | CT01 / COTRN01C | `cotrn01c` |
//! | 13 | `POST /api/v1/transactions` | CT02 / COTRN02C | `cotrn02c` |
//! | 14 | `POST /api/v1/reports/transactions` | CR00 / CORPT00C | `corpt00c` |
//! | 15 | `POST /api/v1/bill-payments` | CB00 / COBIL00C | `cobil00c` |
//! | 16 | `GET /api/v1/users` `[A]` | CU00 / COUSR00C | `cousr00c` |
//! | 17 | `POST /api/v1/users` `[A]` | CU01 / COUSR01C | `cousr01c` |
//! | 18 | `GET /api/v1/users/{user_id}` `[A]` | CU02 / COUSR02C and CU03 / COUSR03C | `cousr02c` |
//! | 19 | `PUT /api/v1/users/{user_id}` `[A]` | CU02 / COUSR02C | `cousr02c` |
//! | 20 | `DELETE /api/v1/users/{user_id}` `[A]` | CU03 / COUSR03C | `cousr03c` |
//! | 21 | `GET /api/v1/accounts/{acct_id}/authorizations` | CPVS / COPAUS0C | `copaus0c` |
//! | 22 | `GET /api/v1/authorizations/{acct_id}/{auth_key}` | CPVD / COPAUS1C | `copaus1c` |
//! | 23 | `POST /api/v1/authorizations/{acct_id}/{auth_key}/fraud` | CPVD / COPAUS1C → COPAUS2C | `copaus1c` |
//! | 24 | `GET /api/v1/transaction-types` `[A]` | CTLI / COTRTLIC | `cotrtlic` |
//! | 25 | `POST /api/v1/transaction-types/list-actions` `[A]` | CTLI / COTRTLIC | `cotrtlic` |
//! | 26 | `GET /api/v1/transaction-types/{type_code}` `[A]` | CTTU / COTRTUPC | `cotrtupc` |
//! | 27 | `POST /api/v1/transaction-types/{type_code}/actions` `[A]` | CTTU / COTRTUPC | `cotrtupc` |
//!
//! - Endpoint 18 is the user read COUSR02C and COUSR03C share, because their read paragraphs
//!   carry identical messages; it lives in `cousr02c`, and `cousr03c` holds only the delete.
//! - Endpoint 23 is COPAUS1C's PF5, which links to COPAUS2C for the fraud toggle, so COPAUS2C's
//!   rules run inside `copaus1c` and COPAUS2C has no module of its own.
//! - CP00, CDRD and CDRA have no REST operation: they are the `RabbitMQ` workers of the
//!   `messaging` crate.
//!
//! [`SignedOnUser`]: auth::extract::SignedOnUser
//! [`AdminUser`]: auth::extract::AdminUser

pub mod app;
pub mod auth;
pub mod dto;
pub mod error;
pub mod openapi;

// Crate-private: only `app::router` reaches the handlers, and only the handlers reach the
// services (Thin Request Handlers).
pub(crate) mod handlers;
pub(crate) mod services;
