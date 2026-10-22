//! The REST layer's request handlers: one module per online program, replacing the receive and
//! send paragraphs of the CICS pseudo-conversational programs (AAP 0.3.4.1, 0.5.4). Each screen
//! exchange of a program becomes one HTTP operation, and the identity the programs carried in the
//! `COCOM01Y` COMMAREA (`CDEMO-USER-ID`, `CDEMO-USER-TYPE`) arrives as a verified bearer token.
//!
//! # Thin Request Handlers
//!
//! Every handler has the same three steps and nothing else:
//!
//! 1. **Extract** the validated request (a `crate::dto::ValidJson` body, a `crate::dto::ValidQuery`
//!    query, a validated path, `crate::dto::ValidPath` or `crate::dto::cotrtupc::TypeCodePath`,
//!    or a raw path segment through `crate::dto::RawPath`) and the verified identity,
//!    `SignedOnUser` or `AdminUser`. Who acts, and with which user type, comes only from the
//!    verified JWT claims, never from a header, a query string or a body field (AAP 0.5.1).
//! 2. **Delegate** to exactly one `crate::services::<program>` function. The service loads through
//!    the repository traits, applies the `domain::online::<program>` rules, persists and returns
//!    the outcome.
//! 3. **Map** the outcome with the `From` conversions of `crate::dto` and
//!    `crate::error::ScreenError`.
//!
//! A handler does no arithmetic on domain values, branches on no domain state, composes no
//! message text, builds no query and logs nothing. Handlers never reference the storage crate or
//! SQL; the layering scan of `scripts/check.sh` step 6 reads this folder, these docs included.
//!
//! # Endpoints
//!
//! The 27 operations of AAP 0.3.4.1, in endpoint order. The handler column names the function
//! under `crate::handlers`, which `crate::app::router` registers; the service column names the
//! one function that handler calls. Identity `AdminUser` marks the admin-gated operations (`[A]`
//! in the AAP). A collection operation is one of Bounded Pagination.
//!
//! | # | Method and path | Transaction / program | Handler | Service | Identity | Collection |
//! |---|---|---|---|---|---|---|
//! | 1 | `POST /api/v1/signon` | CC00 / COSGN00C | `cosgn00c::signon` | `services::cosgn00c::signon` | none | no |
//! | 2 | `GET /api/v1/menus/main` | CM00 / COMEN01C | `comen01c::main_menu` | `services::comen01c::menu` | `SignedOnUser` | no |
//! | 3 | `GET /api/v1/menus/admin` | CA00 / COADM01C | `coadm01c::admin_menu` | `services::coadm01c::menu` | `AdminUser` | no |
//! | 4 | `GET /api/v1/accounts/{acct_id}/view` | CAVW / COACTVWC | `coactvwc::view` | `services::coactvwc::view` | `SignedOnUser` | no |
//! | 5 | `GET /api/v1/accounts/{acct_id}` | CAUP / COACTUPC | `coactupc::fetch` | `services::coactupc::fetch` | `SignedOnUser` | no |
//! | 6 | `PUT /api/v1/accounts/{acct_id}` | CAUP / COACTUPC | `coactupc::save` | `services::coactupc::save` | `SignedOnUser` | no |
//! | 7 | `GET /api/v1/cards` | CCLI / COCRDLIC | `cocrdlic::list` | `services::cocrdlic::list` | `SignedOnUser` | yes |
//! | 8 | `GET /api/v1/cards/view` | CCDL / COCRDSLC | `cocrdslc::view` | `services::cocrdslc::view` | `SignedOnUser` | no |
//! | 9 | `GET /api/v1/cards/edit` | CCUP / COCRDUPC | `cocrdupc::fetch` | `services::cocrdupc::fetch` | `SignedOnUser` | no |
//! | 10 | `PUT /api/v1/cards/{card_ref}` | CCUP / COCRDUPC | `cocrdupc::save` | `services::cocrdupc::save` | `SignedOnUser` | no |
//! | 11 | `GET /api/v1/transactions` | CT00 / COTRN00C | `cotrn00c::list` | `services::cotrn00c::list` | `SignedOnUser` | yes |
//! | 12 | `GET /api/v1/transactions/{tran_id}` | CT01 / COTRN01C | `cotrn01c::view` | `services::cotrn01c::view` | `SignedOnUser` | no |
//! | 13 | `POST /api/v1/transactions` | CT02 / COTRN02C | `cotrn02c::add` | `services::cotrn02c::add` | `SignedOnUser` | no |
//! | 14 | `POST /api/v1/reports/transactions` | CR00 / CORPT00C | `corpt00c::submit` | `services::corpt00c::submit` | `SignedOnUser` | no |
//! | 15 | `POST /api/v1/bill-payments` | CB00 / COBIL00C | `cobil00c::pay` | `services::cobil00c::pay` | `SignedOnUser` | no |
//! | 16 | `GET /api/v1/users` | CU00 / COUSR00C | `cousr00c::list` | `services::cousr00c::list` | `AdminUser` | yes |
//! | 17 | `POST /api/v1/users` | CU01 / COUSR01C | `cousr01c::add` | `services::cousr01c::add` | `AdminUser` | no |
//! | 18 | `GET /api/v1/users/{user_id}` | CU02 / COUSR02C and CU03 / COUSR03C | `cousr02c::fetch` | `services::cousr02c::fetch` | `AdminUser` | no |
//! | 19 | `PUT /api/v1/users/{user_id}` | CU02 / COUSR02C | `cousr02c::update` | `services::cousr02c::update` | `AdminUser` | no |
//! | 20 | `DELETE /api/v1/users/{user_id}` | CU03 / COUSR03C | `cousr03c::delete` | `services::cousr03c::delete` | `AdminUser` | no |
//! | 21 | `GET /api/v1/accounts/{acct_id}/authorizations` | CPVS / COPAUS0C | `copaus0c::summary` | `services::copaus0c::summary` | `SignedOnUser` | yes |
//! | 22 | `GET /api/v1/authorizations/{acct_id}/{auth_key}` | CPVD / COPAUS1C | `copaus1c::detail` | `services::copaus1c::detail` | `SignedOnUser` | no |
//! | 23 | `POST /api/v1/authorizations/{acct_id}/{auth_key}/fraud` | CPVD / COPAUS1C and COPAUS2C | `copaus1c::toggle_fraud` | `services::copaus1c::toggle_fraud` | `SignedOnUser` | no |
//! | 24 | `GET /api/v1/transaction-types` | CTLI / COTRTLIC | `cotrtlic::list` | `services::cotrtlic::list` | `AdminUser` | yes |
//! | 25 | `POST /api/v1/transaction-types/list-actions` | CTLI / COTRTLIC | `cotrtlic::list_actions` | `services::cotrtlic::list_actions` | `AdminUser` | yes |
//! | 26 | `GET /api/v1/transaction-types/{type_code}` | CTTU / COTRTUPC | `cotrtupc::fetch` | `services::cotrtupc::fetch` | `AdminUser` | no |
//! | 27 | `POST /api/v1/transaction-types/{type_code}/actions` | CTTU / COTRTUPC | `cotrtupc::action` | `services::cotrtupc::act` | `AdminUser` | no |
//!
//! - **Shared and absent modules.** Endpoint 18 is the user read COUSR02C and COUSR03C share,
//!   because their read paragraphs carry identical messages, so it lives in `cousr02c` and
//!   `cousr03c` holds only the delete. COPAUS1C links to COPAUS2C for PF5, so COPAUS2C's rules
//!   run inside `services::copaus1c::toggle_fraud` and COPAUS2C has no handler module. CP00, CDRD
//!   and CDRA have no REST endpoint: they are the `RabbitMQ` workers of the `messaging` crate.
//! - **Tokens and the admin gate.** Every operation except endpoint 1, which issues the token,
//!   declares the `bearer_auth` security requirement. The extractor answers a missing or invalid
//!   token with 401, and `AdminUser` answers a token whose user type is not `A` with 403, before
//!   the handler body runs (n5). The frontend's route guard only hides screens; it is not the
//!   control (AAP 0.3.4.6).
//! - **Bounded Pagination.** Collections 7, 11, 16, 21, 24 and 25 take `page_size`, from 1 to 100,
//!   defaulting to the program's screen size. The request extractor answers a value outside that
//!   range with 422 and a `ScreenError`, the only use of 422. The menus (2, 3) are fixed option
//!   tables, single resources rather than collections.
//! - **One path per handler.** This module re-exports nothing, so each handler and the operation
//!   type its `#[utoipa::path(impl_for = …)]` implements `utoipa::Path` on have exactly one name
//!   for `crate::app::router` to register, and the emitted document, the frontend's only type
//!   source, is built from these operations alone (Generated API Types Only). Each operation ID
//!   is the module name and the handler name joined by an underscore, for example
//!   `cotrtupc_action`.
//! - **Redaction.** Handlers return only the DTOs of `crate::dto`, whose card number and SSN
//!   fields carry the redacted forms. Endpoint 8 is the one operation that can return a full card
//!   number, and its service decides that from the verified user type alone (Cardholder Data
//!   Redaction, AAP 0.3.4.8).

// In endpoint order. Each declaration stands alone, separated by a blank line, because rustfmt
// sorts a run of adjacent `mod` items alphabetically, which would undo the order.
//
// The line above each declaration is a plain comment, not a `///` doc. Rustdoc merges an outer
// doc with the child file's `//!` doc and then resolves the child's intra-doc links from this
// module, where names such as `SignedOnUser` are not in scope, so every such link would break.
// Each child's own `//!` doc names its program and endpoints.

// COSGN00C, sign-on (CC00): endpoint 1.
pub(crate) mod cosgn00c;

// COMEN01C, the main menu (CM00): endpoint 2.
pub(crate) mod comen01c;

// COADM01C, the admin menu (CA00): endpoint 3.
pub(crate) mod coadm01c;

// COACTVWC, account view (CAVW): endpoint 4.
pub(crate) mod coactvwc;

// COACTUPC, account update (CAUP): endpoints 5 and 6.
pub(crate) mod coactupc;

// COCRDLIC, the credit card list (CCLI): endpoint 7.
pub(crate) mod cocrdlic;

// COCRDSLC, credit card view (CCDL): endpoint 8.
pub(crate) mod cocrdslc;

// COCRDUPC, credit card update (CCUP): endpoints 9 and 10.
pub(crate) mod cocrdupc;

// COTRN00C, the transaction list (CT00): endpoint 11.
pub(crate) mod cotrn00c;

// COTRN01C, transaction view (CT01): endpoint 12.
pub(crate) mod cotrn01c;

// COTRN02C, transaction add (CT02): endpoint 13.
pub(crate) mod cotrn02c;

// CORPT00C, transaction report submission (CR00): endpoint 14.
pub(crate) mod corpt00c;

// COBIL00C, bill payment (CB00): endpoint 15.
pub(crate) mod cobil00c;

// COUSR00C, the user list (CU00): endpoint 16.
pub(crate) mod cousr00c;

// COUSR01C, user add (CU01): endpoint 17.
pub(crate) mod cousr01c;

// COUSR02C, user update (CU02), with the user read COUSR03C shares: endpoints 18 and 19.
pub(crate) mod cousr02c;

// COUSR03C, user delete (CU03): endpoint 20.
pub(crate) mod cousr03c;

// COPAUS0C, the pending authorization summary (CPVS): endpoint 21.
pub(crate) mod copaus0c;

// COPAUS1C, authorization details (CPVD) and COPAUS2C's fraud toggle: endpoints 22 and 23.
pub(crate) mod copaus1c;

// COTRTLIC, the transaction type list (CTLI): endpoints 24 and 25.
pub(crate) mod cotrtlic;

// COTRTUPC, transaction type maintenance (CTTU): endpoints 26 and 27.
pub(crate) mod cotrtupc;
