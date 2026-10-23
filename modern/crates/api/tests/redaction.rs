//! Cardholder Data Redaction for the REST API (AAP 0.3.4.8, E32): the HTTP half of the proof
//! whose broker half is `crates/messaging/tests/redaction.rs`.
//!
//! No full card number, SSN, CVV or password may leave the API in a response body, an error, a
//! tracing event or span (what the `OpenTelemetry` exporter prints included) or the `OpenAPI`
//! document and its examples. The one disclosure is endpoint 8, `GET /api/v1/cards/view`, which
//! returns all 16 digits for a verified `usr_type` of `A`. The rule's card-owner clause never
//! grants it, because no `CardDemo` user is linked to a customer, account or card (E32).
//!
//! - `no_sensitive_value_leaves_the_api` drives the 27 endpoints from [`STEPS`], success and
//!   error paths alike, first with a `U` token and then with an `A` token, under
//!   `test_support::trace_capture`. It maps every response to its schema through the emitted
//!   document and requires each response schema the document names to have been collected, so
//!   that every response type passes through its real handler and the trace. It searches every
//!   response body and the captured trace, walks every JSON body for CVV and password members,
//!   and checks the redacted forms.
//! - `endpoint_8_is_the_single_disclosure` pins the full number to the `A` token.
//! - `spans_record_route_templates_only` checks that request spans name the route template and
//!   never the URI or its query string.
//! - `openapi_and_types_are_redacted` scans the committed and the emitted document and the
//!   formatting of `CardNumber` and `Ssn`. It needs no Docker.
//! - `every_response_type_is_redacted` serialises one populated value of every response type,
//!   endpoint 8's view both redacted and disclosed, through the needle scan and the member walk.
//!   Its inventory is checked complete against the emitted document: the same schema names as
//!   the responses refer to, and every declared property filled. It needs no Docker.
//!
//! The one disclosure, the full number in the top-level `card_num` of a 200 of endpoint 8 to the
//! `A` token, is the only text any check leaves out: that member is removed once it is found to
//! hold the number asked for, which must appear nowhere else in the body, and the rest is
//! searched and walked in full. Error bodies and the `U` token get no exception.
//!
//! The needles are the 50 fixture card numbers and the unknown card number endpoint 8 is asked
//! for, the 50 fixture SSNs with and without dashes, the fixture user passwords and every
//! password this file submits. Matching is case-sensitive. A failure names the endpoint, the
//! token and each needle's kind and index, never a needle or a body. These are pattern-lane
//! checks of a user rule, so they carry no audit tag.
//!
//! Each Docker-backed test starts its own fixture-loaded `postgres:16` container through
//! `test_support`, which removes it on drop, and seeds one pending-authorization summary with
//! three details for the first `cardxref.txt` record (card `0500024453765740`, customer 50,
//! account `00000000050`) through the repositories. Requests run in process through
//! `tower::ServiceExt::oneshot` on the current-thread runtime, so every span the router's trace
//! layer makes opens on the thread whose default subscriber the capture is. The capture stays in
//! memory; nothing is exported. Endpoint 14's job decks go to a directory of each test's own
//! under `CARGO_TARGET_TMPDIR`, emptied when the test starts.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{Debug, Display};
use std::path::PathBuf;
use std::sync::Arc;
use std::{fs, io};

use api::app::{AppState, app, openapi};
use api::dto::{
    self, coactupc, coactvwc, cobil00c, cocrdlic, cocrdslc, cocrdupc, comen01c, copaus0c, copaus1c,
    corpt00c, cosgn00c, cotrn00c, cotrn01c, cotrn02c, cotrtlic, cotrtupc, cousr00c, cousr02c,
};
use api::error::ScreenError;
use axum::Router;
use axum::body::{self, Body};
use axum::http::{Method, Request, StatusCode, header};
use chrono::{NaiveDate, NaiveDateTime};
use domain::clock::{Clock, FixedClock};
use domain::copybook::{Count, PicS, PicU, PicX};
use domain::money::{Decimal, Money};
use domain::records::{CardXrefRecord, PendingAuthDetailRecord, PendingAuthSummaryRecord};
use domain::{CardNumber, Ssn};
use persistence::{
    CardXrefRepository, PendingAuthDetailRepository, PendingAuthSummaryRepository, PgStore,
};
use runtime::config::Config;
use runtime::telemetry::UNMATCHED_ROUTE;
use serde::Serialize;
use serde_json::{Map, Value, json};
use test_support::fixtures::{
    fixture_card_numbers, fixture_ssn_forms, fixture_ssns, fixture_user_passwords,
    start_loaded_postgres,
};
use test_support::postgres::PostgresGuard;
use test_support::trace_capture;
use tower::ServiceExt as _;
use utoipa::ToSchema;

type TestResult = Result<(), Box<dyn Error>>;
type Fallible<T> = Result<T, Box<dyn Error>>;

/// Runs `test` to completion on a current-thread runtime, the one Tokio's test attribute
/// builds, without the lint attributes that attribute's expansion carries. The runtime stays on
/// the test's own thread, the one whose default subscriber `trace_capture` installs.
fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(test)
}

/// The HS256 secret of the test state: a local-only value.
const SECRET: &str = "redaction-local-only-test-signing-secret";

/// The fixture users of `DUSRSECJ.jcl` whose tokens the tests use, and their password.
const USER_ID: &str = "USER0001";
const ADMIN_ID: &str = "ADMIN001";
const FIXTURE_PASSWORD: &str = "PASSWORD";

/// The passwords this file submits: endpoint 17's, endpoint 19's and a wrong sign-on password.
/// None is a fixture value, so each is a needle of its own.
const ADD_PASSWORD: &str = "SX9KQ2WZ";
const UPDATE_PASSWORD: &str = "TM4PL8ZQ";
const WRONG_PASSWORD: &str = "WRONGPW1";
const SUBMITTED_PASSWORDS: [&str; 3] = [ADD_PASSWORD, UPDATE_PASSWORD, WRONG_PASSWORD];

/// A card number no fixture holds, which endpoint 8 is asked for: its error must not echo it,
/// so it is a needle of its own.
const UNKNOWN_CARD: &str = "4000001234567899";

/// The user endpoint 17 adds, endpoint 19 updates and endpoint 20 deletes.
const NEW_USER: &str = "REDACT01";

/// The card, account and customer of the first `cardxref.txt` record.
const CARD: &str = "0500024453765740";
const CARD_REDACTED: &str = "************5740";
const ACCOUNT: &str = "00000000050";
const AUTH_ACCOUNT: i64 = 50;
const AUTH_CUSTOMER: u64 = 50;

/// `YYDDD` of 2022-07-18, the day of the seeded details, and `HHMMSSmmm` of the first one.
const AUTH_DAY: u64 = 22_199;
const AUTH_TIME: u64 = 103_000_000;

/// The pending-authorization details seeded under account 50, one millisecond apart.
const SEEDED_DETAILS: u64 = 3;

/// An account no `acctdata.txt` record holds, whose seeded cross-reference names customer 50:
/// endpoints 4 and 5 answer its 404 with customer 50's block beside the message, the SSN
/// included (COACTVWC-008, COACTUPC-099).
const UNREAD_ACCOUNT: u64 = 88;
/// The card of that cross-reference: no fixture holds it and no response may show it.
const UNREAD_ACCOUNT_CARD: &str = "4000008888880088";

/// Customer fields endpoint 6 sends in place of the fixture's, which COACTUPC's edits refuse:
/// a FICO score from 300 to 850, a state and ZIP its combination table holds, and
/// general-purpose area codes.
const VALID_CUSTOMER_FIELDS: [(&str, &str); 9] = [
    ("fico_score", "750"),
    ("state", "IL"),
    ("zip", "62701"),
    ("phone_1_area", "312"),
    ("phone_1_prefix", "555"),
    ("phone_1_line", "0100"),
    ("phone_2_area", "415"),
    ("phone_2_prefix", "555"),
    ("phone_2_line", "0199"),
];

/// The positions of COTRTLIC's map, the rows of every endpoint 25 body.
const MAP_ROWS: usize = 7;

/// Endpoint 1 and endpoint 8.
const SIGNON: &str = "/api/v1/signon";
const CARD_VIEW: &str = "/api/v1/cards/view";

/// The masks of the redacted forms: twelve `*` before a card number's last four digits, and
/// `***-**-` before an SSN's.
const CARD_MASK: &str = "************";
const SSN_MASK: &str = "***-**-";

/// The digits a redacted form keeps.
const LAST_DIGITS: usize = 4;

/// The expected status of a step whose status is not asserted.
const ANY_STATUS: u16 = 0;

/// The request method of a step.
#[derive(Debug, Clone, Copy)]
enum Verb {
    Get,
    Post,
    Put,
    Delete,
}

impl Verb {
    fn method(self) -> Method {
        match self {
            Self::Get => Method::GET,
            Self::Post => Method::POST,
            Self::Put => Method::PUT,
            Self::Delete => Method::DELETE,
        }
    }
}

/// Whether a step carries the pass's bearer token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Auth {
    Bearer,
    Without,
}

/// The token a request carried, for the labels of failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    User,
    Admin,
    Missing,
}

impl Token {
    const fn label(self) -> &'static str {
        match self {
            Self::User => "the U token",
            Self::Admin => "the A token",
            Self::Missing => "no token",
        }
    }

    /// The first name endpoint 6 saves in this pass, so that each pass commits a change.
    const fn first_name(self) -> &'static str {
        match self {
            Self::Admin => "Probo",
            Self::User | Self::Missing => "Probe",
        }
    }

    /// The embossed name endpoint 10 saves in this pass, letters and spaces only, so that each
    /// pass commits a change: the `A` pass fetches the name the `U` pass saved.
    const fn embossed_name(self) -> &'static str {
        match self {
            Self::Admin => "Probo Holder",
            Self::User | Self::Missing => "Probe Holder",
        }
    }
}

/// The body of a step.
#[derive(Debug, Clone, Copy)]
enum Payload {
    /// No body.
    Empty,
    /// Endpoint 1: a user ID and a password.
    SignOn(&'static str, &'static str),
    /// Endpoint 6 with this key: the kept endpoint 5 view's fields, the SSN parts omitted.
    AccountSave(&'static str),
    /// Endpoint 10's ENTER: the kept endpoint 9 view's fields, unchanged, and its snapshot.
    CardSave,
    /// Endpoint 10 with this key: the kept endpoint 9 view's status and expiry with this pass's
    /// embossed name. ENTER sends the view's snapshot, F5 the one the kept endpoint 10 result
    /// returned.
    CardRename(&'static str),
    /// Endpoint 13's ENTER for account 50 with this confirmation.
    TranAdd(&'static str),
    /// Endpoint 14 with this Monthly selector and this confirmation, the other selectors and
    /// the dates blank.
    Report(&'static str, &'static str),
    /// Endpoint 15 for account 1, confirmation blank.
    BillPayment,
    /// Endpoint 17 adding [`NEW_USER`] with [`ADD_PASSWORD`].
    UserAdd,
    /// Endpoint 19 updating [`NEW_USER`] with [`UPDATE_PASSWORD`].
    UserUpdate,
    /// Endpoint 25's ENTER over the kept endpoint 24 state, nothing typed.
    ListAction,
    /// Endpoint 27 with this key and description.
    TypeAction(&'static str, &'static str),
}

/// What a step's 200 response leaves for the steps after it.
#[derive(Debug, Clone, Copy)]
enum Keep {
    Nothing,
    CardRef,
    AccountView,
    AccountResult,
    CardView,
    CardResult,
    TranId,
    AuthKey,
    ListState,
}

/// What a step's 200 response must show beyond its status, so that the success path the step
/// names is known to have been taken.
#[derive(Debug, Clone, Copy)]
enum Shows {
    Anything,
    /// `true` at this JSON pointer.
    True(&'static str),
    /// A text member at this JSON pointer.
    Text(&'static str),
}

impl Shows {
    /// What the 200 body `body` lacks, by JSON pointer only; `None` when it shows what is
    /// required.
    fn lacking(self, body: &str) -> Option<String> {
        let (pointer, wanted, required): (_, fn(&Value) -> bool, _) = match self {
            Self::Anything => return None,
            Self::True(pointer) => (pointer, |member| member == &Value::Bool(true), "true"),
            Self::Text(pointer) => (pointer, Value::is_string, "a text"),
        };
        let shown = serde_json::from_str::<Value>(body)
            .is_ok_and(|value| value.pointer(pointer).is_some_and(wanted));
        (!shown).then(|| format!("{pointer} is not {required}"))
    }
}

/// One request of a pass. The URI may name `{card_ref}`, `{tran_id}`, `{auth_key}` and
/// `{user_id}`, which [`Kept::resolve`] fills in.
#[derive(Debug, Clone, Copy)]
struct Step {
    /// The endpoint number of AAP 0.3.4.1.
    endpoint: u8,
    verb: Verb,
    uri: &'static str,
    auth: Auth,
    payload: Payload,
    keep: Keep,
    shows: Shows,
    /// The status expected in the `U` pass and in the `A` pass, so that every path the step
    /// names is known to have been taken; [`ANY_STATUS`] asserts none.
    statuses: [u16; 2],
}

const fn step(endpoint: u8, verb: Verb, uri: &'static str, statuses: [u16; 2]) -> Step {
    Step {
        endpoint,
        verb,
        uri,
        auth: Auth::Bearer,
        payload: Payload::Empty,
        keep: Keep::Nothing,
        shows: Shows::Anything,
        statuses,
    }
}

impl Step {
    const fn sending(self, payload: Payload) -> Self {
        Self { payload, ..self }
    }

    const fn keeping(self, keep: Keep) -> Self {
        Self { keep, ..self }
    }

    const fn showing(self, shows: Shows) -> Self {
        Self { shows, ..self }
    }

    const fn anonymous(self) -> Self {
        Self {
            auth: Auth::Without,
            ..self
        }
    }
}

/// The requests of one pass, in the order a screen session issues them: every endpoint at least
/// once, with the success and error paths each can take. Paths a `U` token cannot reach answer
/// 403 in the `U` pass, and the steps without a token answer as they do for anyone.
const STEPS: &[Step] = &[
    // 1: both fixture users, a wrong password and a blank one.
    step(1, Verb::Post, SIGNON, [200, 200])
        .anonymous()
        .sending(Payload::SignOn(USER_ID, FIXTURE_PASSWORD)),
    step(1, Verb::Post, SIGNON, [200, 200])
        .anonymous()
        .sending(Payload::SignOn(ADMIN_ID, FIXTURE_PASSWORD)),
    step(1, Verb::Post, SIGNON, [401, 401])
        .anonymous()
        .sending(Payload::SignOn(USER_ID, WRONG_PASSWORD)),
    step(1, Verb::Post, SIGNON, [400, 400])
        .anonymous()
        .sending(Payload::SignOn(USER_ID, "")),
    // 2 and 3: the menus; the admin menu refuses a U token.
    step(2, Verb::Get, "/api/v1/menus/main", [200, 200]),
    step(3, Verb::Get, "/api/v1/menus/admin", [403, 200]),
    // 4: account 50 with its customer's SSN, an unknown account, and no token at all.
    step(
        4,
        Verb::Get,
        "/api/v1/accounts/00000000050/view",
        [200, 200],
    ),
    step(
        4,
        Verb::Get,
        "/api/v1/accounts/99999999999/view",
        [404, 404],
    ),
    // An account missing behind its cross-reference: the 404 shows customer 50 beside the
    // message.
    step(
        4,
        Verb::Get,
        "/api/v1/accounts/00000000088/view",
        [404, 404],
    ),
    step(
        4,
        Verb::Get,
        "/api/v1/accounts/00000000050/view",
        [401, 401],
    )
    .anonymous(),
    // 5 and 6: the fetch, an unknown account, and ENTER and F5 with the SSN parts omitted.
    // Account 50's stored SSN fails COACTUPC's SSN edit, so its ENTER is the edit's 400 and
    // the save that commits is account 1's.
    step(5, Verb::Get, "/api/v1/accounts/00000000050", [200, 200]).keeping(Keep::AccountView),
    step(5, Verb::Get, "/api/v1/accounts/99999999999", [404, 404]),
    step(5, Verb::Get, "/api/v1/accounts/00000000088", [404, 404]),
    step(6, Verb::Put, "/api/v1/accounts/00000000050", [400, 400])
        .sending(Payload::AccountSave("ENTER")),
    step(5, Verb::Get, "/api/v1/accounts/00000000001", [200, 200]).keeping(Keep::AccountView),
    step(6, Verb::Put, "/api/v1/accounts/00000000001", [200, 200])
        .sending(Payload::AccountSave("ENTER"))
        .keeping(Keep::AccountResult),
    step(6, Verb::Put, "/api/v1/accounts/00000000001", [200, 200])
        .sending(Payload::AccountSave("PF5")),
    // 7: unfiltered, by account, by card number, and a card filter that is not numeric.
    step(7, Verb::Get, "/api/v1/cards", [200, 200]),
    step(
        7,
        Verb::Get,
        "/api/v1/cards?acct_id=00000000050",
        [200, 200],
    ),
    step(
        7,
        Verb::Get,
        "/api/v1/cards?card_num=0500024453765740",
        [200, 200],
    )
    .keeping(Keep::CardRef),
    step(
        7,
        Verb::Get,
        "/api/v1/cards?card_num=ABCDEFGHIJKLMNOP",
        [400, 400],
    ),
    // 8: by number and by handle; the card under another account, which COCRDSLC reads by
    // the card number alone and so finds; a card no fixture holds; and a 15-digit number.
    step(
        8,
        Verb::Get,
        "/api/v1/cards/view?acct_id=00000000050&card_num=0500024453765740",
        [200, 200],
    ),
    step(
        8,
        Verb::Get,
        "/api/v1/cards/view?acct_id=00000000050&card_ref={card_ref}",
        [200, 200],
    ),
    step(
        8,
        Verb::Get,
        "/api/v1/cards/view?acct_id=00000000001&card_num=0500024453765740",
        [200, 200],
    ),
    step(
        8,
        Verb::Get,
        "/api/v1/cards/view?acct_id=00000000050&card_num=4000001234567899",
        [404, 404],
    ),
    step(
        8,
        Verb::Get,
        "/api/v1/cards/view?acct_id=00000000050&card_num=050002445376574",
        [400, 400],
    ),
    // 9 and 10: the fetch by handle, ENTER with the fields as shown, ENTER with this pass's
    // embossed name, which validates, and F5 with that name, which saves it.
    step(
        9,
        Verb::Get,
        "/api/v1/cards/edit?acct_id=00000000050&card_ref={card_ref}",
        [200, 200],
    )
    .keeping(Keep::CardView),
    step(10, Verb::Put, "/api/v1/cards/{card_ref}", [400, 400]).sending(Payload::CardSave),
    step(10, Verb::Put, "/api/v1/cards/{card_ref}", [200, 200])
        .sending(Payload::CardRename("ENTER"))
        .keeping(Keep::CardResult),
    step(10, Verb::Put, "/api/v1/cards/{card_ref}", [200, 200])
        .sending(Payload::CardRename("PF5"))
        .showing(Shows::True("/committed")),
    // 13, 12 and 11: the prompt with the filled card, the add, its view and the list.
    step(13, Verb::Post, "/api/v1/transactions", [200, 200]).sending(Payload::TranAdd("")),
    step(13, Verb::Post, "/api/v1/transactions", [200, 200])
        .sending(Payload::TranAdd("Y"))
        .keeping(Keep::TranId),
    step(12, Verb::Get, "/api/v1/transactions/{tran_id}", [200, 200]),
    step(11, Verb::Get, "/api/v1/transactions", [200, 200]),
    step(
        11,
        Verb::Get,
        "/api/v1/transactions?start_tran_id={tran_id}",
        [200, 200],
    ),
    // 14 and 15: a report with nothing selected, the Monthly report confirmed, which writes the
    // next job deck, and declined, which clears the screen; then account 1's balance.
    step(14, Verb::Post, "/api/v1/reports/transactions", [400, 400])
        .sending(Payload::Report("", "")),
    step(14, Verb::Post, "/api/v1/reports/transactions", [200, 200])
        .sending(Payload::Report("X", "Y"))
        .showing(Shows::Text("/job_deck")),
    step(14, Verb::Post, "/api/v1/reports/transactions", [200, 200])
        .sending(Payload::Report("X", "N")),
    step(15, Verb::Post, "/api/v1/bill-payments", [200, 200]).sending(Payload::BillPayment),
    // 16 to 20: the list, then add, read, update and delete one user, and delete it again.
    step(16, Verb::Get, "/api/v1/users", [403, 200]),
    step(17, Verb::Post, "/api/v1/users", [403, 200]).sending(Payload::UserAdd),
    step(18, Verb::Get, "/api/v1/users/{user_id}", [403, 200]),
    step(19, Verb::Put, "/api/v1/users/{user_id}", [403, 200]).sending(Payload::UserUpdate),
    step(18, Verb::Get, "/api/v1/users/{user_id}", [403, 200]),
    step(20, Verb::Delete, "/api/v1/users/{user_id}", [403, 200]),
    // A delete that finds no user takes COUSR03C's halted not-found path (X-COUSR03C-H1), so
    // only its body is checked, not its status.
    step(
        20,
        Verb::Delete,
        "/api/v1/users/{user_id}",
        [403, ANY_STATUS],
    ),
    // 21 to 23: the summary, the detail and the next one, the fraud toggle, unknown keys.
    step(
        21,
        Verb::Get,
        "/api/v1/accounts/00000000050/authorizations",
        [200, 200],
    )
    .keeping(Keep::AuthKey),
    step(
        21,
        Verb::Get,
        "/api/v1/accounts/99999999999/authorizations",
        [404, 404],
    ),
    step(
        22,
        Verb::Get,
        "/api/v1/authorizations/00000000050/{auth_key}",
        [200, 200],
    ),
    step(
        22,
        Verb::Get,
        "/api/v1/authorizations/00000000050/{auth_key}?direction=forward",
        [200, 200],
    ),
    step(
        23,
        Verb::Post,
        "/api/v1/authorizations/00000000050/{auth_key}/fraud",
        [200, 200],
    ),
    step(
        22,
        Verb::Get,
        "/api/v1/authorizations/00000000050/0778000896990000",
        [404, 404],
    ),
    step(
        23,
        Verb::Post,
        "/api/v1/authorizations/00000000050/0778000896990000/fraud",
        [404, 404],
    ),
    // 24 to 27: the type list, an ENTER over it, the fetch of type 01 and an ENTER on it.
    step(24, Verb::Get, "/api/v1/transaction-types", [403, 200]).keeping(Keep::ListState),
    step(
        25,
        Verb::Post,
        "/api/v1/transaction-types/list-actions",
        [403, 200],
    )
    .sending(Payload::ListAction),
    step(26, Verb::Get, "/api/v1/transaction-types/01", [403, 200]),
    step(
        27,
        Verb::Post,
        "/api/v1/transaction-types/01/actions",
        [403, 400],
    )
    .sending(Payload::TypeAction("ENTER", "")),
    step(
        27,
        Verb::Post,
        "/api/v1/transaction-types/01/actions",
        [403, 200],
    )
    .sending(Payload::TypeAction("ENTER", "Purchase at a shop")),
];

/// What earlier 200 responses of a pass left for the steps after them.
#[derive(Default)]
struct Kept {
    card_ref: Option<i64>,
    tran_id: Option<String>,
    auth_key: Option<String>,
    account_view: Option<Value>,
    account_result: Option<Value>,
    card_view: Option<Value>,
    card_result: Option<Value>,
    list_state: Option<Value>,
}

impl Kept {
    /// Takes what `keep` names from the 200 response body `body`.
    fn keep(&mut self, keep: Keep, body: &str) -> TestResult {
        if matches!(keep, Keep::Nothing) {
            return Ok(());
        }
        let value: Value = serde_json::from_str(body)?;
        match keep {
            Keep::Nothing => {}
            Keep::CardRef => {
                let handle = value.pointer("/rows/0/card_ref").and_then(Value::as_i64);
                self.card_ref = Some(handle.ok_or("endpoint 7 listed no card")?);
            }
            Keep::AccountView => self.account_view = Some(value),
            Keep::AccountResult => self.account_result = Some(value),
            Keep::CardView => self.card_view = Some(value),
            Keep::CardResult => self.card_result = Some(value),
            Keep::TranId => self.tran_id = Some(text_at(&value, "/new_tran_id")?),
            Keep::AuthKey => self.auth_key = Some(text_at(&value, "/rows/0/auth_key")?),
            Keep::ListState => {
                let state = value.get("state").cloned();
                self.list_state = Some(state.ok_or("endpoint 24 sent no state")?);
            }
        }
        Ok(())
    }

    /// `uri` with each placeholder it names filled in.
    fn resolve(&self, uri: &str) -> Fallible<String> {
        let values = [
            ("{card_ref}", self.card_ref.map(|handle| handle.to_string())),
            ("{tran_id}", self.tran_id.clone()),
            ("{auth_key}", self.auth_key.clone()),
            ("{user_id}", Some(NEW_USER.to_owned())),
        ];
        let mut resolved = uri.to_owned();
        for (placeholder, value) in values {
            if resolved.contains(placeholder) {
                let value = value.ok_or_else(|| format!("nothing kept for {placeholder}"))?;
                resolved = resolved.replace(placeholder, &value);
            }
        }
        Ok(resolved)
    }

    /// The JSON body of `payload` in the pass of `token`, `None` for no body.
    fn payload(&self, payload: Payload, token: Token) -> Fallible<Option<Value>> {
        let body = match payload {
            Payload::Empty => return Ok(None),
            Payload::SignOn(user_id, password) => {
                json!({ "user_id": user_id, "password": password })
            }
            Payload::AccountSave(aid) => self.account_save(aid, token)?,
            Payload::CardSave => self.card_save("ENTER", None)?,
            Payload::CardRename(aid) => self.card_save(aid, Some(token.embossed_name()))?,
            Payload::TranAdd(confirm) => json!({
                "aid": "ENTER", "acct_id": ACCOUNT, "confirm": confirm,
                "fields": {
                    "type_cd": "01", "cat_cd": "0005", "source": "POS TERM",
                    "description": "Purchase at Abshire-Lowe", "amount": "+00000012.34",
                    "orig_date": "2022-06-10", "proc_date": "2022-06-11",
                    "merchant_id": "800000000", "merchant_name": "Abshire-Lowe",
                    "merchant_city": "North Enoshaven", "merchant_zip": "72112"
                }
            }),
            Payload::Report(monthly, confirm) => {
                json!({ "monthly": monthly, "yearly": "", "custom": "", "confirm": confirm })
            }
            Payload::BillPayment => json!({ "acct_id": "00000000001", "confirm": "" }),
            Payload::UserAdd => json!({
                "first_name": "REDACT", "last_name": "PROBE", "user_id": NEW_USER,
                "password": ADD_PASSWORD, "user_type": "U"
            }),
            Payload::UserUpdate => json!({
                "first_name": "REDACT", "last_name": "PROBED",
                "password": UPDATE_PASSWORD, "user_type": "U"
            }),
            Payload::ListAction => json!({
                "aid": "ENTER", "type_filter": "", "desc_filter": "",
                "rows": vec![json!({ "selection": "", "description": "" }); MAP_ROWS],
                "state": self.list_state.clone().unwrap_or(Value::Null),
            }),
            Payload::TypeAction(aid, description) => {
                json!({ "aid": aid, "description": description })
            }
        };
        Ok(Some(body))
    }

    /// Endpoint 6 with key `aid`: every field of the kept view as shown, its dates as the
    /// screen's year, month and day parts, the SSN parts omitted (the stored SSN is kept), this
    /// pass's first name and [`VALID_CUSTOMER_FIELDS`]. F5 echoes the state and the amounts the
    /// ENTER before it returned.
    fn account_save(&self, aid: &str, token: Token) -> Fallible<Value> {
        let view = self
            .account_view
            .as_ref()
            .ok_or("no endpoint 5 view kept")?;
        let mut fields = view
            .as_object()
            .cloned()
            .ok_or("the view is not an object")?;
        for shown_only in ["snapshot", "ssn", "info_message", "account_id"] {
            fields.remove(shown_only);
        }
        for (date, prefix) in [
            ("open_date", "open"),
            ("expiry_date", "expiry"),
            ("reissue_date", "reissue"),
            ("dob", "dob"),
        ] {
            insert_date_parts(&mut fields, date, prefix)?;
        }
        fields.insert("first_name".to_owned(), Value::from(token.first_name()));
        for (field, value) in VALID_CUSTOMER_FIELDS {
            fields.insert(field.to_owned(), Value::from(value));
        }
        let mut snapshot = view
            .get("snapshot")
            .cloned()
            .ok_or("the view has no snapshot")?;
        if aid == "PF5" {
            let result = self
                .account_result
                .as_ref()
                .ok_or("no endpoint 6 result kept")?;
            snapshot["state"] = result
                .get("state")
                .cloned()
                .ok_or("the result has no state")?;
            if let Some(Value::Object(amounts)) = result.get("amounts") {
                fields.extend(amounts.clone());
            }
        }
        Ok(json!({ "aid": aid, "fields": fields, "snapshot": snapshot }))
    }

    /// Endpoint 10 with key `aid`: the kept view's status and expiry as shown, its name or
    /// `name` in its place, and the view's snapshot, or for F5 the snapshot of the kept
    /// endpoint 10 result.
    fn card_save(&self, aid: &str, name: Option<&str>) -> Fallible<Value> {
        let view = self.card_view.as_ref().ok_or("no endpoint 9 view kept")?;
        let text = |key: &str| view.get(key).and_then(Value::as_str).unwrap_or_default();
        let expiry = NaiveDate::parse_from_str(text("expiration_date"), "%Y-%m-%d")?;
        let shown = if aid == "PF5" {
            let result = self
                .card_result
                .as_ref()
                .ok_or("no endpoint 10 result kept")?;
            result
                .get("snapshot")
                .filter(|snapshot| snapshot.is_object())
        } else {
            view.get("snapshot")
        };
        Ok(json!({
            "aid": aid,
            "fields": {
                "embossed_name": name.unwrap_or_else(|| text("embossed_name")),
                "status": text("status"),
                "expiry_month": expiry.format("%m").to_string(),
                "expiry_year": expiry.format("%Y").to_string(),
            },
            "snapshot": shown.cloned().ok_or("no snapshot kept for endpoint 10")?,
        }))
    }
}

/// Replaces the typed date `date` of an endpoint 5 view with the screen parts `<prefix>_year`,
/// `<prefix>_month` and `<prefix>_day`, each blank for a blank date.
fn insert_date_parts(fields: &mut Map<String, Value>, date: &str, prefix: &str) -> TestResult {
    let shown = fields.remove(date);
    let parsed = match shown.as_ref().and_then(Value::as_str) {
        Some(text) => Some(NaiveDate::parse_from_str(text, "%Y-%m-%d")?),
        None => None,
    };
    for (suffix, format) in [("year", "%Y"), ("month", "%m"), ("day", "%d")] {
        let part = parsed.map(|day| day.format(format).to_string());
        fields.insert(
            format!("{prefix}_{suffix}"),
            Value::from(part.unwrap_or_default()),
        );
    }
    Ok(())
}

/// The text member at `pointer` of `value`.
fn text_at(value: &Value, pointer: &str) -> Fallible<String> {
    let text = value.pointer(pointer).and_then(Value::as_str);
    Ok(text
        .ok_or_else(|| format!("no text member at {pointer}"))?
        .to_owned())
}

/// The kind of a protected value, for failure labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Card,
    Ssn,
    Password,
}

impl Kind {
    const fn name(self) -> &'static str {
        match self {
            Self::Card => "card number",
            Self::Ssn => "SSN",
            Self::Password => "password",
        }
    }
}

/// One protected value, with its kind and its index among the values of that kind.
struct Needle {
    kind: Kind,
    index: usize,
    text: String,
}

/// Every protected value the scans search for. Deliberately not `Debug`: nothing prints them.
struct Needles(Vec<Needle>);

impl Needles {
    /// The fixture card numbers, [`UNKNOWN_CARD`] and [`UNREAD_ACCOUNT_CARD`], the fixture SSNs
    /// in both forms, the fixture user passwords and [`SUBMITTED_PASSWORDS`].
    fn load() -> Fallible<Self> {
        let mut needles = Self(Vec::new());
        let cards = fixture_card_numbers()?;
        if cards
            .iter()
            .any(|card| card == UNKNOWN_CARD || card == UNREAD_ACCOUNT_CARD)
        {
            return Err("a card number this file adds is a fixture card number".into());
        }
        for card in cards {
            needles.push(Kind::Card, card)?;
        }
        needles.push(Kind::Card, UNKNOWN_CARD.to_owned())?;
        needles.push(Kind::Card, UNREAD_ACCOUNT_CARD.to_owned())?;
        for ssn in fixture_ssn_forms()? {
            needles.push(Kind::Ssn, ssn)?;
        }
        for password in fixture_user_passwords()? {
            needles.push(Kind::Password, String::from_utf8(password)?)?;
        }
        for password in SUBMITTED_PASSWORDS {
            needles.push(Kind::Password, password.to_owned())?;
        }
        Ok(needles)
    }

    /// Adds `text` as the next value of `kind`. An empty value would match everything, so it is
    /// refused rather than skipped.
    fn push(&mut self, kind: Kind, text: String) -> TestResult {
        if text.is_empty() {
            return Err(format!("an empty {} needle", kind.name()).into());
        }
        let index = self.0.iter().filter(|needle| needle.kind == kind).count();
        self.0.push(Needle { kind, index, text });
        Ok(())
    }

    /// The kind and index of each needle `haystack` holds, every needle searched. Never the
    /// needle itself.
    fn found(&self, haystack: &str) -> Vec<String> {
        self.0
            .iter()
            .filter(|needle| haystack.contains(needle.text.as_str()))
            .map(|needle| format!("{} #{}", needle.kind.name(), needle.index))
            .collect()
    }

    /// Whether `text` is one of the card number needles.
    fn holds_card(&self, text: &str) -> bool {
        self.0
            .iter()
            .any(|needle| needle.kind == Kind::Card && needle.text == text)
    }
}

/// Fails when `haystack` holds any needle, naming `label` and each needle by kind and index
/// only.
fn assert_clean(needles: &Needles, haystack: &str, label: &str) {
    let found = needles.found(haystack);
    assert!(
        found.is_empty(),
        "{label} holds protected values: {}",
        found.join(", ")
    );
}

/// The one disclosure the rule allows, taken out of `body`, a 200 of endpoint 8 to the `A`
/// token for `card`: `body` with its top-level `card_num` removed, for the needle scan and the
/// member walk to search in full.
///
/// `card` must be a card number needle that `body` holds exactly once, as the value of that
/// top-level `card_num`; anything else is an error, never a body passed unscanned. The errors
/// name what failed, never the number or the body.
fn disclosed_remainder(body: &str, card: &str, needles: &Needles) -> Fallible<Value> {
    if !needles.holds_card(card) {
        return Err("the card asked for is not a card number needle".into());
    }
    if body.matches(card).count() != 1 {
        return Err("the full number appears other than once in the body".into());
    }
    let mut value: Value =
        serde_json::from_str(body).map_err(|_| "the endpoint 8 body is not JSON".to_owned())?;
    let object = value
        .as_object_mut()
        .ok_or("the endpoint 8 body is not a JSON object")?;
    match object.remove("card_num") {
        Some(Value::String(shown)) if shown == card => Ok(value),
        _ => Err("the top-level card_num is not the full number asked for".into()),
    }
}

/// One collected response, with the method and the concrete path of its request.
struct Exchange {
    endpoint: u8,
    token: Token,
    verb: Verb,
    path: String,
    status: StatusCode,
    body: String,
}

impl Exchange {
    fn label(&self) -> String {
        format!(
            "endpoint {} with {} ({})",
            self.endpoint,
            self.token.label(),
            self.status.as_u16()
        )
    }

    /// The card number this response may hold in full, in its top-level `card_num` only: for a
    /// 200 of endpoint 8 to the `A` token, [`CARD`], which every such step of [`STEPS`] asks
    /// for by number or by its handle. Every other response, an endpoint 8 error included, may
    /// hold none.
    fn disclosure(&self) -> Option<&'static str> {
        (self.endpoint == 8 && self.token == Token::Admin && self.status == StatusCode::OK)
            .then_some(CARD)
    }

    /// The JSON this response's checks search: its body, with the permitted card number taken
    /// out by [`disclosed_remainder`] when [`Exchange::disclosure`] allows one. `None` for a body
    /// that is not JSON and allows none.
    fn scanned_json(&self, needles: &Needles) -> Fallible<Option<Value>> {
        match self.disclosure() {
            Some(card) => disclosed_remainder(&self.body, card, needles).map(Some),
            None => Ok(serde_json::from_str(&self.body).ok()),
        }
    }
}

/// The API over one fixture-loaded and seeded database.
///
/// Fields drop in declaration order, so the router and its pool handle go before the guard
/// removes the container.
struct Harness {
    router: Router,
    _database: PostgresGuard,
}

impl Harness {
    /// Starts the database, seeds the pending authorizations and builds the state from the
    /// configuration with the clock at 2022-07-18 10:30:00.000 and the job decks in the fresh
    /// directory of the test `test`.
    async fn start(test: &str) -> Fallible<Self> {
        let database = start_loaded_postgres().await?;
        let mut store = PgStore::new(database.pool().clone());
        seed_authorizations(&mut store).await?;
        seed_unread_account(&mut store).await?;
        let config = Config {
            jwt_secret: SECRET.to_owned(),
            intrdr_dir: fresh_deck_dir(test)?,
            ..Config::default()
        };
        let at = NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|day| day.and_hms_milli_opt(10, 30, 0, 0))
            .ok_or("an invalid test instant")?;
        let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(at));
        let state = AppState::from_config(store, &config, clock)?;
        Ok(Self {
            router: app(state),
            _database: database,
        })
    }

    /// The status and the body text `request` gets, the body read to its end so that the
    /// request's span closes.
    async fn send(&self, request: Request<Body>) -> Fallible<(StatusCode, String)> {
        let response = self.router.clone().oneshot(request).await?;
        let status = response.status();
        let bytes = body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok((status, String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// The bearer token endpoint 1 issues to the fixture user `user_id`.
    async fn sign_on(&self, user_id: &str) -> Fallible<String> {
        let (status, body) = self.send(sign_on_request(user_id)?).await?;
        token_of(status, &body)
    }

    /// Endpoint 8 with the query `query` for `token`: the body text and its JSON, or an error
    /// for any status but 200.
    async fn card_view(&self, query: &str, token: &str) -> Fallible<(String, Value)> {
        let uri = format!("{CARD_VIEW}?{query}");
        let (status, body) = self
            .send(request(Verb::Get, &uri, Some(token), None)?)
            .await?;
        if status != StatusCode::OK {
            return Err(format!("endpoint 8 answered {status}").into());
        }
        let value = serde_json::from_str(&body)?;
        Ok((body, value))
    }
}

/// The job-deck directory of the test `test` under Cargo's temporary directory for integration
/// tests, removed and created again so that every run starts with no generation in it.
fn fresh_deck_dir(test: &str) -> Fallible<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("redaction-jobs")
        .join(test);
    match fs::remove_dir_all(&dir) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// A request of `verb` to `uri`, with `token` as bearer token and `body` as JSON when given.
fn request(
    verb: Verb,
    uri: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> Fallible<Request<Body>> {
    let mut builder = Request::builder().method(verb.method()).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = match body {
        Some(value) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string()))?,
        None => builder.body(Body::empty())?,
    };
    Ok(request)
}

/// Endpoint 1 for the fixture user `user_id` with the fixture password.
fn sign_on_request(user_id: &str) -> Fallible<Request<Body>> {
    let credentials = json!({ "user_id": user_id, "password": FIXTURE_PASSWORD });
    request(Verb::Post, SIGNON, None, Some(&credentials))
}

/// The token of a sign-on response, or an error unless it is a 200 carrying one.
fn token_of(status: StatusCode, body: &str) -> Fallible<String> {
    if status != StatusCode::OK {
        return Err(format!("sign-on answered {status}").into());
    }
    let value: Value = serde_json::from_str(body)?;
    let token = value.get("token").and_then(Value::as_str);
    Ok(token.ok_or("sign-on returned no token")?.to_owned())
}

/// Inserts the summary of account 50 for customer 50 and [`SEEDED_DETAILS`] approved details of
/// 10.00 each under it, for [`CARD`], so that endpoints 21 to 23 show card data.
async fn seed_authorizations(store: &mut PgStore) -> TestResult {
    let account = PicS::new(AUTH_ACCOUNT)?;
    let amount = Decimal::new(1_000, 2);
    let mut summary = PendingAuthSummaryRecord::initialized(account, PicU::new(AUTH_CUSTOMER)?);
    // TRUNCATION: checked, never lossy. `SEEDED_DETAILS` is 3, which `i16` holds; a refusal is
    // a test failure, never a wrapped count.
    summary.approved_auth_cnt = Count::new(i16::try_from(SEEDED_DETAILS)?);
    summary.approved_auth_amt = Money::new(amount * Decimal::from(SEEDED_DETAILS))?;
    PendingAuthSummaryRepository::insert(store, &summary).await?;
    for index in 0..SEEDED_DETAILS {
        let detail = approved_detail(index, Money::new(amount)?)?;
        PendingAuthDetailRepository::insert(store, account, &detail).await?;
    }
    Ok(())
}

/// Inserts the cross-reference of [`UNREAD_ACCOUNT_CARD`] from account [`UNREAD_ACCOUNT`], which
/// no account row holds, to customer [`AUTH_CUSTOMER`], so that the error bodies of endpoints 4
/// and 5 for that account carry the customer's block.
async fn seed_unread_account(store: &mut PgStore) -> TestResult {
    let xref = CardXrefRecord {
        card_num: CardNumber::parse(UNREAD_ACCOUNT_CARD)?,
        cust_id: PicU::new(AUTH_CUSTOMER)?,
        acct_id: PicU::new(UNREAD_ACCOUNT)?,
        filler: PicX::spaces(),
    };
    CardXrefRepository::insert(store, &xref).await?;
    Ok(())
}

/// The approval `index` of the seeded day, keyed as CP00 keys it: the day and the time inverted.
fn approved_detail(index: u64, amount: Money<10>) -> Fallible<PendingAuthDetailRecord> {
    Ok(PendingAuthDetailRecord {
        auth_date_9c: PicS::complement_from(AUTH_DAY)?,
        auth_time_9c: PicS::complement_from(AUTH_TIME + index)?,
        orig_date: PicX::from_text("220718")?,
        orig_time: PicX::from_text(&format!("1030{index:02}"))?,
        card_num: CardNumber::parse(CARD)?,
        auth_type: PicX::from_text("AUTH")?,
        card_expiry_date: PicX::from_text("1227")?,
        message_type: PicX::from_text("0100")?,
        message_source: PicX::from_text("POS")?,
        auth_id_code: PicX::from_text(&format!("{index:06}"))?,
        auth_resp_code: PicX::from_array(*PendingAuthDetailRecord::AUTH_APPROVED),
        auth_resp_reason: PicX::from_text("0000")?,
        processing_code: PicU::new(0)?,
        transaction_amt: amount,
        approved_amt: amount,
        merchant_catagory_code: PicX::from_text("5411")?,
        acqr_country_code: PicX::from_text("840")?,
        pos_entry_mode: PicU::new(5)?,
        merchant_id: PicX::from_text("MERCH0000000001")?,
        merchant_name: PicX::from_text("CORNER GROCERY")?,
        merchant_city: PicX::from_text("SEATTLE")?,
        merchant_state: PicX::from_text("WA")?,
        merchant_zip: PicX::from_text("98101")?,
        transaction_id: PicX::from_text(&format!("TXN{index:012}"))?,
        match_status: PicX::from_array(*PendingAuthDetailRecord::MATCH_PENDING),
        auth_fraud: PicX::spaces(),
        fraud_rpt_date: PicX::spaces(),
        filler: PicX::spaces(),
    })
}

/// The responses of one test, collected as its passes run.
struct Session<'h> {
    harness: &'h Harness,
    kept: Kept,
    exchanges: Vec<Exchange>,
    /// Steps whose status was not the expected one, by step, endpoint and token only.
    unexpected: Vec<String>,
}

impl<'h> Session<'h> {
    fn new(harness: &'h Harness) -> Self {
        Self {
            harness,
            kept: Kept::default(),
            exchanges: Vec::new(),
            unexpected: Vec::new(),
        }
    }

    /// Endpoint 1 for the fixture user `user_id`, collected, and the token it issued.
    async fn sign_on(&mut self, user_id: &str) -> Fallible<String> {
        let (status, body) = self.harness.send(sign_on_request(user_id)?).await?;
        let token = token_of(status, &body)?;
        self.exchanges.push(Exchange {
            endpoint: 1,
            token: Token::Missing,
            verb: Verb::Post,
            path: SIGNON.to_owned(),
            status,
            body,
        });
        Ok(token)
    }

    /// Every step of [`STEPS`] in order, with `bearer` as the token of `token`, collecting each
    /// response. What a pass keeps is its own.
    async fn run(&mut self, token: Token, bearer: &str) -> TestResult {
        self.kept = Kept::default();
        for (index, step) in STEPS.iter().enumerate() {
            let at = format!(
                "step {index} (endpoint {}) in the pass of {}",
                step.endpoint,
                token.label()
            );
            // A step whose input an earlier step failed to leave is reported and skipped, so
            // that one failure does not hide the steps after it.
            let built = self.kept.resolve(step.uri).and_then(|uri| {
                let payload = self.kept.payload(step.payload, token)?;
                Ok((uri, payload))
            });
            let (uri, payload) = match built {
                Ok(built) => built,
                Err(error) => {
                    self.unexpected.push(format!("{at}: {error}"));
                    continue;
                }
            };
            let (sent_with, credential) = match step.auth {
                Auth::Bearer => (token, Some(bearer)),
                Auth::Without => (Token::Missing, None),
            };
            let sent = request(step.verb, &uri, credential, payload.as_ref())?;
            let (status, body) = self.harness.send(sent).await?;
            let [as_user, as_admin] = step.statuses;
            let expected = if token == Token::Admin {
                as_admin
            } else {
                as_user
            };
            if expected != ANY_STATUS && status.as_u16() != expected {
                self.unexpected
                    .push(format!("{at}: {status}, expected {expected}"));
            } else if status == StatusCode::OK {
                if let Some(lacking) = step.shows.lacking(&body) {
                    self.unexpected.push(format!("{at}: {lacking}"));
                }
                self.kept.keep(step.keep, &body)?;
            }
            let path = uri.split('?').next().unwrap_or_default().to_owned();
            self.exchanges.push(Exchange {
                endpoint: step.endpoint,
                token: sent_with,
                verb: step.verb,
                path,
                status,
                body,
            });
        }
        Ok(())
    }
}

/// Whether `text` is a redacted card number: twelve `*` and four digits.
fn is_redacted_card(text: &str) -> bool {
    text.strip_prefix(CARD_MASK)
        .is_some_and(|tail| tail.len() == LAST_DIGITS && tail.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether `text` is a redacted SSN: `***-**-` and four digits.
fn is_redacted_ssn(text: &str) -> bool {
    text.strip_prefix(SSN_MASK)
        .is_some_and(|tail| tail.len() == LAST_DIGITS && tail.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether a member name denotes a CVV or a password, which no body may carry.
fn names_forbidden_member(key: &str) -> bool {
    key.to_ascii_lowercase().contains("cvv") || key == "password" || key == "password_hash"
}

/// Every member of `value` at any depth, as its key and its value.
fn members<'v>(value: &'v Value, found: &mut Vec<(&'v str, &'v Value)>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                found.push((key.as_str(), child));
                members(child, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                members(item, found);
            }
        }
        _ => {}
    }
}

/// What the member walk of the collected bodies found.
#[derive(Default)]
struct Forms {
    /// Endpoint 7 rows whose `card_num` is the redacted form.
    card_rows: usize,
    /// Endpoint 4 responses whose `ssn` is the redacted form.
    ssns: usize,
    /// Error responses of endpoints 4 and 5 whose `ssn` is the redacted form: the customer
    /// shown beside a message.
    error_ssns: usize,
    failures: Vec<String>,
}

/// What the member walk of one JSON value found.
#[derive(Default)]
struct Walk {
    /// `card_num` texts in the redacted form.
    redacted_cards: usize,
    /// `ssn` texts in the redacted form.
    redacted_ssns: usize,
    /// The members that break the rule, by member name only.
    breaks: Vec<String>,
}

/// Walks every member of `value` at any depth: none may name a CVV or a password, and every
/// `card_num` and `ssn` text must be its redacted form.
fn walk_members(value: &Value) -> Walk {
    let mut found = Vec::new();
    members(value, &mut found);
    let mut walk = Walk::default();
    for (key, member) in found {
        if names_forbidden_member(key) {
            walk.breaks.push(format!("a member named {key}"));
        }
        match (key, member.as_str()) {
            ("card_num", Some(text)) => {
                if is_redacted_card(text) {
                    walk.redacted_cards += 1;
                } else {
                    walk.breaks.push("a card_num not redacted".to_owned());
                }
            }
            ("ssn", Some(text)) => {
                if is_redacted_ssn(text) {
                    walk.redacted_ssns += 1;
                } else {
                    walk.breaks.push("an ssn not redacted".to_owned());
                }
            }
            _ => {}
        }
    }
    walk
}

/// Walks every collected JSON body with [`walk_members`]. A 200 of endpoint 8 to the `A` token
/// is walked without its top-level `card_num`, and only once [`disclosed_remainder`] has found
/// the full number there; every nested `card_num` is still walked. Failures name the exchange
/// and the member name only.
fn check_forms(exchanges: &[Exchange], needles: &Needles) -> Forms {
    let mut forms = Forms::default();
    for exchange in exchanges {
        let label = exchange.label();
        let body = match exchange.scanned_json(needles) {
            Ok(Some(body)) => body,
            Ok(None) => continue,
            Err(error) => {
                forms.failures.push(format!("{label}: {error}"));
                continue;
            }
        };
        let walk = walk_members(&body);
        for broken in &walk.breaks {
            forms.failures.push(format!("{label}: {broken}"));
        }
        if exchange.endpoint == 7 {
            forms.card_rows += walk.redacted_cards;
        }
        if exchange.endpoint == 4 {
            forms.ssns += walk.redacted_ssns;
        }
        if matches!(exchange.endpoint, 4 | 5) && !exchange.status.is_success() {
            forms.error_ssns += walk.redacted_ssns;
        }
    }
    forms
}

/// Fails when any collected body holds a needle. A 200 of endpoint 8 to the `A` token is
/// searched without its top-level `card_num`, with every needle, and only once
/// [`disclosed_remainder`] has found the full number there and nowhere else.
fn assert_bodies_clean(exchanges: &[Exchange], needles: &Needles) {
    let mut failures = Vec::new();
    for exchange in exchanges {
        let found = match exchange.disclosure() {
            None => needles.found(&exchange.body),
            Some(card) => match disclosed_remainder(&exchange.body, card, needles) {
                Ok(remainder) => needles.found(&remainder.to_string()),
                Err(error) => vec![error.to_string()],
            },
        };
        for found in found {
            failures.push(format!("{}: {found}", exchange.label()));
        }
    }
    assert!(
        failures.is_empty(),
        "response bodies hold protected values: {}",
        failures.join("; ")
    );
}

/// The prefix of a `$ref` to a component schema.
const SCHEMA_REF: &str = "#/components/schemas/";

/// The component schema `reference`, a `$ref` of `document`, names.
fn component<'d>(document: &'d Value, reference: &str) -> Option<&'d Value> {
    document.pointer(reference.strip_prefix('#')?)
}

/// Every operation of `document`: its path template, its lower-case method and its
/// `responses`.
fn operations(document: &Value) -> Vec<(&str, &str, &Map<String, Value>)> {
    let mut found = Vec::new();
    let paths = document.get("paths").and_then(Value::as_object);
    for (template, item) in paths.into_iter().flatten() {
        for (method, operation) in item.as_object().into_iter().flatten() {
            if let Some(responses) = operation.get("responses").and_then(Value::as_object) {
                found.push((template.as_str(), method.as_str(), responses));
            }
        }
    }
    found
}

/// The name of every component schema a response of `document` refers to: the `$ref` of each
/// response body schema, and of each member of one that is an `anyOf`, `oneOf` or `allOf`. A
/// response or body schema of any other shape is an error, so that no response type escapes
/// the checks that read this set.
fn response_schemas(document: &Value) -> Fallible<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for (template, method, responses) in operations(document) {
        for (status, response) in responses {
            let at = format!("{method} {template} {status}");
            if response.get("$ref").is_some() {
                return Err(format!("{at}: a response reference this check does not read").into());
            }
            let contents = response.get("content").and_then(Value::as_object);
            for media in contents.into_iter().flat_map(Map::values) {
                let schema = media
                    .get("schema")
                    .ok_or_else(|| format!("{at}: a body without a schema"))?;
                let composed = ["anyOf", "oneOf", "allOf"]
                    .iter()
                    .find_map(|key| schema.get(*key));
                let referring: Vec<&Value> = match composed {
                    Some(members) => members.as_array().into_iter().flatten().collect(),
                    None => vec![schema],
                };
                if referring.is_empty() {
                    return Err(format!("{at}: a composed body schema without members").into());
                }
                for member in referring {
                    let name = member
                        .get("$ref")
                        .and_then(Value::as_str)
                        .and_then(|reference| reference.strip_prefix(SCHEMA_REF))
                        .ok_or_else(|| format!("{at}: a body schema that names no component"))?;
                    names.insert(name.to_owned());
                }
            }
        }
    }
    Ok(names)
}

/// For a path template, whether each of its segments is a literal, when it matches the
/// segments of a concrete path; `None` when it does not.
fn literal_positions(template: &str, segments: &[&str]) -> Option<Vec<bool>> {
    let parts: Vec<&str> = template.split('/').collect();
    if parts.len() != segments.len() {
        return None;
    }
    parts
        .iter()
        .zip(segments)
        .map(|(part, segment)| {
            if part.starts_with('{') && part.ends_with('}') {
                (!segment.is_empty()).then_some(false)
            } else {
                (part == segment).then_some(true)
            }
        })
        .collect()
}

/// The response schema `document` gives `status` of `verb` on the concrete path `path`, when
/// that status's body schema is a single `$ref`. The operation is the one whose path template
/// matches `path`, a literal segment preferred to a parameter at each position, as the router
/// prefers it.
fn documented_schema(
    document: &Value,
    verb: Verb,
    path: &str,
    status: StatusCode,
) -> Option<String> {
    let method = verb.method().as_str().to_ascii_lowercase();
    let segments: Vec<&str> = path.split('/').collect();
    let (_, responses) = operations(document)
        .into_iter()
        .filter(|(_, key, _)| *key == method)
        .filter_map(|(template, _, responses)| {
            literal_positions(template, &segments).map(|literals| (literals, responses))
        })
        .max_by(|(left, _), (right, _)| left.cmp(right))?;
    let contents = responses
        .get(status.as_str())?
        .get("content")?
        .as_object()?;
    let reference = contents
        .values()
        .find_map(|media| media.pointer("/schema/$ref"))?;
    let name = reference.as_str()?.strip_prefix(SCHEMA_REF)?;
    Some(name.to_owned())
}

/// The 27 endpoints with the `U` token and then with the `A` token, success and error paths,
/// under one trace capture: every response schema of the emitted document is the schema of a
/// collected response, no body holds a needle outside the top-level `card_num` of an endpoint 8
/// 200 to the `A` token, the trace holds none, no body names a CVV or a password, and the card
/// list and the account view show the redacted forms.
#[test]
fn no_sensitive_value_leaves_the_api() -> TestResult {
    block_on(async {
        let needles = Needles::load()?;
        let harness = Harness::start("no_sensitive_value_leaves_the_api").await?;
        let capture = trace_capture::capture();
        let mut session = Session::new(&harness);
        let user = session.sign_on(USER_ID).await?;
        let admin = session.sign_on(ADMIN_ID).await?;
        session.run(Token::User, &user).await?;
        session.run(Token::Admin, &admin).await?;
        assert!(
            session.unexpected.is_empty(),
            "steps answered other than expected: {}",
            session.unexpected.join("; ")
        );
        let endpoints: Vec<u8> = session.exchanges.iter().map(|e| e.endpoint).collect();
        assert!(
            (1..=27).all(|number| endpoints.contains(&number)),
            "an endpoint was not called"
        );
        let document = serde_json::to_value(openapi())?;
        let observed: BTreeSet<String> = session
            .exchanges
            .iter()
            .filter_map(|exchange| {
                documented_schema(&document, exchange.verb, &exchange.path, exchange.status)
            })
            .collect();
        let unseen: Vec<String> = response_schemas(&document)?
            .difference(&observed)
            .cloned()
            .collect();
        assert!(
            unseen.is_empty(),
            "no collected response was of the response schemas {}",
            unseen.join(", ")
        );

        assert_bodies_clean(&session.exchanges, &needles);
        let traced = capture.text();
        assert!(
            traced.contains(r#""route":"/api/v1/cards/view""#),
            "the capture recorded no endpoint 8 span"
        );
        assert_clean(&needles, &traced, "the captured trace");

        let forms = check_forms(&session.exchanges, &needles);
        assert!(
            forms.failures.is_empty(),
            "response members break the redaction rule: {}",
            forms.failures.join("; ")
        );
        assert!(
            forms.card_rows > 0,
            "no endpoint 7 row showed a card number"
        );
        assert!(forms.ssns > 0, "no endpoint 4 response showed an SSN");
        assert!(
            forms.error_ssns > 0,
            "no endpoint 4 or 5 error showed a customer's SSN"
        );
        Ok(())
    })
}

/// Endpoint 8 by number and by handle: for the `A` token the full number in the top-level
/// `card_num` and nowhere else, the rest of the body holding no needle and no unredacted
/// member; for the `U` token the redacted form and no needle at all.
#[test]
fn endpoint_8_is_the_single_disclosure() -> TestResult {
    block_on(async {
        let needles = Needles::load()?;
        let harness = Harness::start("endpoint_8_is_the_single_disclosure").await?;
        let user = harness.sign_on(USER_ID).await?;
        let admin = harness.sign_on(ADMIN_ID).await?;
        let by_number = format!("acct_id={ACCOUNT}&card_num={CARD}");
        let (_, first) = harness.card_view(&by_number, &admin).await?;
        let card_ref = first.get("card_ref").and_then(Value::as_i64);
        let by_ref = format!(
            "acct_id={ACCOUNT}&card_ref={}",
            card_ref.ok_or("endpoint 8 returned no card_ref")?
        );
        let cases = [
            ("by number with the A token", &by_number, &admin, true),
            ("by number with the U token", &by_number, &user, false),
            ("by handle with the A token", &by_ref, &admin, true),
            ("by handle with the U token", &by_ref, &user, false),
        ];
        for (label, query, token, full) in cases {
            let (body, view) = harness.card_view(query, token).await?;
            if full {
                let remainder = disclosed_remainder(&body, CARD, &needles)
                    .map_err(|error| format!("{label}: {error}"))?;
                assert_clean(&needles, &remainder.to_string(), label);
                let walk = walk_members(&remainder);
                assert!(
                    walk.breaks.is_empty(),
                    "{label}: {}",
                    walk.breaks.join("; ")
                );
            } else {
                let shown = view.get("card_num").and_then(Value::as_str);
                assert!(
                    shown == Some(CARD_REDACTED),
                    "{label}: card_num is not the redacted form"
                );
                assert_clean(&needles, &body, label);
            }
        }
        Ok(())
    })
}

/// Request spans record the method, the route template and the status, never a concrete path
/// or a query string, and a path no route matches records `UNMATCHED_ROUTE`.
#[test]
fn spans_record_route_templates_only() -> TestResult {
    block_on(async {
        let harness = Harness::start("spans_record_route_templates_only").await?;
        let capture = trace_capture::capture();
        let admin = harness.sign_on(ADMIN_ID).await?;
        let probes = [
            (format!("/api/v1/accounts/{ACCOUNT}/view"), StatusCode::OK),
            (
                format!("{CARD_VIEW}?acct_id={ACCOUNT}&card_num={CARD}"),
                StatusCode::OK,
            ),
            (
                format!(
                    "/api/v1/accounts/{ACCOUNT}/authorizations\
                     ?page_key=0778000896999999&direction=forward"
                ),
                StatusCode::OK,
            ),
            (
                format!("/api/v1/no-such-route/{ACCOUNT}?acct_id={ACCOUNT}"),
                StatusCode::NOT_FOUND,
            ),
        ];
        for (index, (uri, expected)) in probes.iter().enumerate() {
            let (status, _) = harness
                .send(request(Verb::Get, uri, Some(&admin), None)?)
                .await?;
            assert_eq!(status, *expected, "probe {index}");
        }

        let traced = capture.text();
        let unmatched = format!(r#""route":"{UNMATCHED_ROUTE}""#);
        for recorded in [
            r#""route":"/api/v1/signon""#,
            r#""route":"/api/v1/accounts/{acct_id}/view""#,
            r#""route":"/api/v1/cards/view""#,
            r#""route":"/api/v1/accounts/{acct_id}/authorizations""#,
            unmatched.as_str(),
            r#""method":"GET""#,
            r#""method":"POST""#,
            r#""status":200"#,
            r#""status":404"#,
        ] {
            assert!(traced.contains(recorded), "the trace lacks {recorded}");
        }
        for withheld in [
            "/api/v1/accounts/00000000050",
            "/api/v1/no-such-route",
            "card_num=",
            "acct_id=",
            "page_key=",
            "direction=",
        ] {
            assert!(
                !traced.contains(withheld),
                "the trace holds the URI fragment {withheld}"
            );
        }
        assert!(
            !traced.contains(CARD),
            "the trace holds the requested card number"
        );
        Ok(())
    })
}

/// What the walk of one `OpenAPI` document found.
#[derive(Default)]
struct DocumentAudit {
    /// Examples of a `card_num` property, parameter or example member.
    card_examples: usize,
    /// Examples of a property, parameter or example member whose name holds `ssn`.
    ssn_examples: usize,
    failures: Vec<String>,
}

impl DocumentAudit {
    /// Walks `value`, whose examples describe `owner`: the property, parameter or example member
    /// they belong to, `None` above any.
    fn walk(&mut self, value: &Value, owner: Option<&str>) {
        match value {
            Value::Object(map) => {
                // A parameter object names what its schema and example describe.
                let parameter = map
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|_| map.contains_key("in"));
                if let Some(name) = parameter {
                    self.check_name(name);
                }
                let owner = parameter.or(owner);
                for (key, child) in map {
                    self.check_name(key);
                    match (key.as_str(), child) {
                        ("example", _) => self.example(child, owner),
                        ("properties", Value::Object(properties)) => {
                            for (property, schema) in properties {
                                self.check_name(property);
                                self.walk(schema, Some(property));
                            }
                        }
                        _ => self.walk(child, owner),
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.walk(item, owner);
                }
            }
            _ => {}
        }
    }

    /// An example of `owner`: its members describe themselves, its leaves `owner`.
    fn example(&mut self, value: &Value, owner: Option<&str>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    self.check_name(key);
                    self.example(child, Some(key));
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.example(item, owner);
                }
            }
            Value::Null => {}
            _ => self.example_leaf(value, owner),
        }
    }

    /// A card number example must be the redacted form, and an SSN example may show at most
    /// the four digits the redacted form keeps.
    fn example_leaf(&mut self, value: &Value, owner: Option<&str>) {
        let Some(owner) = owner else {
            return;
        };
        if owner.contains("card_num") {
            self.card_examples += 1;
            if !value.as_str().is_some_and(is_redacted_card) {
                let failure = format!("the example of {owner} is not a redacted card number");
                self.failures.push(failure);
            }
        } else if owner.contains("ssn") {
            self.ssn_examples += 1;
            let text = value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned);
            if text.bytes().filter(u8::is_ascii_digit).count() > LAST_DIGITS {
                let failure = format!("the example of {owner} shows more than four digits");
                self.failures.push(failure);
            }
        }
    }

    /// No key, property, parameter or example member may name a CVV.
    fn check_name(&mut self, name: &str) {
        if name.to_ascii_lowercase().contains("cvv") {
            self.failures.push(format!("{name} names a CVV"));
        }
    }
}

/// `serde_json`, `Debug` and `Display` of `value`: each must be `expected`, the JSON form as a
/// string, and none may hold a needle. Failures name `label` and the form only.
fn assert_type_forms<T: Serialize + Debug + Display>(
    value: &T,
    expected: &str,
    label: &str,
    needles: &Needles,
) -> TestResult {
    let quoted = serde_json::to_string(expected)?;
    let forms = [
        ("Serialize", serde_json::to_string(value)?, quoted.as_str()),
        ("Debug", format!("{value:?}"), expected),
        ("Display", format!("{value}"), expected),
    ];
    for (form, shown, wanted) in &forms {
        assert!(shown == wanted, "{label}: {form} is not the redacted form");
        assert_clean(needles, shown, &format!("{label} through {form}"));
    }
    Ok(())
}

/// The committed `openapi/openapi.json` and the document the handlers emit hold no needle, name
/// no CVV, and give every card number and SSN example in its redacted form; and `CardNumber`
/// and `Ssn` of every fixture value serialise, debug-format and display as the redacted form.
#[test]
fn openapi_and_types_are_redacted() -> TestResult {
    let needles = Needles::load()?;
    let committed = fs::read_to_string(test_support::modern_dir().join("openapi/openapi.json"))?;
    let emitted = openapi().to_pretty_json()?;
    for (label, text) in [
        ("the committed document", &committed),
        ("the emitted document", &emitted),
    ] {
        assert_clean(&needles, text, label);
        let document: Value = serde_json::from_str(text)?;
        let mut audit = DocumentAudit::default();
        audit.walk(&document, None);
        assert!(
            audit.failures.is_empty(),
            "{label}: {}",
            audit.failures.join("; ")
        );
        assert!(
            audit.card_examples > 0,
            "{label} has no card number example"
        );
        assert!(audit.ssn_examples > 0, "{label} has no SSN example");
    }

    for (index, card) in fixture_card_numbers()?.iter().enumerate() {
        let tail = card
            .get(CARD_MASK.len()..)
            .ok_or("a fixture card number is short")?;
        let value = CardNumber::parse(card)?;
        let label = format!("card number #{index}");
        assert_type_forms(&value, &format!("{CARD_MASK}{tail}"), &label, &needles)?;
    }
    for (index, ssn) in fixture_ssns()?.iter().enumerate() {
        let start = ssn
            .len()
            .checked_sub(LAST_DIGITS)
            .ok_or("a fixture SSN is short")?;
        let tail = ssn.get(start..).ok_or("a fixture SSN is not ASCII")?;
        let value = Ssn::parse(ssn)?;
        let label = format!("SSN #{index}");
        assert_type_forms(&value, &format!("{SSN_MASK}{tail}"), &label, &needles)?;
    }
    Ok(())
}

/// Fixture card numbers and SSNs for the response values, each field taking the next one.
struct Protected {
    cards: std::vec::IntoIter<String>,
    ssns: std::vec::IntoIter<String>,
}

impl Protected {
    fn load() -> Fallible<Self> {
        Ok(Self {
            cards: fixture_card_numbers()?.into_iter(),
            ssns: fixture_ssns()?.into_iter(),
        })
    }

    /// The next fixture card number, as its text.
    fn card_text(&mut self) -> Fallible<String> {
        Ok(self.cards.next().ok_or("too few fixture card numbers")?)
    }

    /// The next fixture card number.
    fn card(&mut self) -> Fallible<CardNumber> {
        Ok(CardNumber::parse(&self.card_text()?)?)
    }

    /// The next fixture SSN.
    fn ssn(&mut self) -> Fallible<Ssn> {
        let ssn = self.ssns.next().ok_or("too few fixture SSNs")?;
        Ok(Ssn::parse(&ssn)?)
    }
}

/// One populated value of a response type: its schema name, its JSON and, for the disclosed
/// endpoint 8 view only, the card number it shows in full.
struct Entry {
    name: String,
    json: String,
    disclosed: Option<String>,
}

/// The entry of `value`, named by its type's `ToSchema::name`.
fn entry<T: ToSchema + Serialize>(value: &T) -> Fallible<Entry> {
    Ok(Entry {
        name: T::name().into_owned(),
        json: serde_json::to_string(value)?,
        disclosed: None,
    })
}

/// The date every typed date of the response values shows.
fn shown_date() -> Fallible<NaiveDate> {
    NaiveDate::from_ymd_opt(2022, 7, 18).ok_or_else(|| "an invalid shown date".into())
}

/// The timestamp every typed timestamp of the response values shows.
fn shown_instant() -> Fallible<NaiveDateTime> {
    shown_date()?
        .and_hms_milli_opt(10, 30, 0, 0)
        .ok_or_else(|| "an invalid shown instant".into())
}

/// The snapshot of an endpoint 5 view, every date given.
fn account_update_snapshot() -> Fallible<coactupc::AccountUpdateSnapshot> {
    Ok(coactupc::AccountUpdateSnapshot {
        state: "S".into(),
        account_id: "00000000050".into(),
        active_status: "Y".into(),
        current_balance: "+1,020.00".into(),
        credit_limit: "+5,000.00".into(),
        cash_credit_limit: "+1,500.00".into(),
        open_date: Some(shown_date()?),
        expiry_date: Some(shown_date()?),
        reissue_date: Some(shown_date()?),
        current_cycle_credit: "+0.00".into(),
        current_cycle_debit: "+0.00".into(),
        group_id: "A000000000".into(),
        customer_id: "000000050".into(),
        first_name: "Aniya".into(),
        middle_name: "Q".into(),
        last_name: "Von".into(),
        address_line_1: "1 Main Street".into(),
        address_line_2: "Apt 2".into(),
        city: "Springfield".into(),
        state_code: "IL".into(),
        country: "USA".into(),
        zip: "62701".into(),
        phone_1: "(312)555-0100".into(),
        phone_2: "(415)555-0199".into(),
        ssn_last4: "1234".into(),
        government_id: "GOVID0050".into(),
        dob: Some(shown_date()?),
        eft_account_id: "0000000050".into(),
        primary_card_holder: "Y".into(),
        fico_score: "750".into(),
    })
}

/// The 200 of endpoint 5, the SSN a fixture value.
fn account_update_view(protected: &mut Protected) -> Fallible<coactupc::AccountUpdateView> {
    Ok(coactupc::AccountUpdateView {
        account_id: "00000000050".into(),
        active_status: "Y".into(),
        open_date: Some(shown_date()?),
        credit_limit: "+5,000.00".into(),
        expiry_date: Some(shown_date()?),
        cash_credit_limit: "+1,500.00".into(),
        reissue_date: Some(shown_date()?),
        current_balance: "+1,020.00".into(),
        current_cycle_credit: "+0.00".into(),
        group_id: "A000000000".into(),
        current_cycle_debit: "+0.00".into(),
        customer_id: "000000050".into(),
        ssn: protected.ssn()?,
        dob: Some(shown_date()?),
        fico_score: "750".into(),
        first_name: "Aniya".into(),
        middle_name: "Q".into(),
        last_name: "Von".into(),
        address_line_1: "1 Main Street".into(),
        state: "IL".into(),
        address_line_2: "Apt 2".into(),
        zip: "62701".into(),
        city: "Springfield".into(),
        country: "USA".into(),
        phone_1_area: "312".into(),
        phone_1_prefix: "555".into(),
        phone_1_line: "0100".into(),
        government_id: "GOVID0050".into(),
        phone_2_area: "415".into(),
        phone_2_prefix: "555".into(),
        phone_2_line: "0199".into(),
        eft_account_id: "0000000050".into(),
        primary_card_holder: "Y".into(),
        info_message: "Update account details presented above.".into(),
        snapshot: account_update_snapshot()?,
    })
}

/// The account and customer blocks of endpoint 4, the SSN a fixture value.
fn account_view_blocks(
    protected: &mut Protected,
) -> Fallible<(
    coactvwc::AccountViewAccountBlock,
    coactvwc::AccountViewCustomerBlock,
)> {
    let account = coactvwc::AccountViewAccountBlock {
        active_status: "Y".into(),
        open_date: Some(shown_date()?),
        expiration_date: Some(shown_date()?),
        reissue_date: Some(shown_date()?),
        credit_limit: "+5,000.00".into(),
        cash_credit_limit: "+1,500.00".into(),
        current_balance: "+1,020.00".into(),
        current_cycle_credit: "+0.00".into(),
        current_cycle_debit: "+0.00".into(),
        group_id: "A000000000".into(),
    };
    let customer = coactvwc::AccountViewCustomerBlock {
        customer_id: "000000050".into(),
        ssn: protected.ssn()?,
        date_of_birth: Some(shown_date()?),
        fico_score: "750".into(),
        first_name: "Aniya".into(),
        middle_name: "Q".into(),
        last_name: "Von".into(),
        address_line_1: "1 Main Street".into(),
        address_line_2: "Apt 2".into(),
        city: "Springfield".into(),
        state: "IL".into(),
        zip: "62701".into(),
        country: "USA".into(),
        phone_1: "(312)555-0100".into(),
        phone_2: "(415)555-0199".into(),
        government_id: "GOVID0050".into(),
        eft_account_id: "0000000050".into(),
        primary_card_holder: "Y".into(),
    };
    Ok((account, customer))
}

/// The response values of endpoints 4 to 6, the errors beside their blocks included.
fn account_entries(protected: &mut Protected) -> Fallible<Vec<Entry>> {
    let (account, customer) = account_view_blocks(protected)?;
    let view = coactvwc::AccountViewResponse {
        acct_id: "00000000050".into(),
        account: account.clone(),
        customer,
    };
    let (_, error_customer) = account_view_blocks(protected)?;
    let view_error = coactvwc::AccountViewError {
        error: ScreenError::not_found(
            "Did not find this account in account master file",
            Some("acct_id".into()),
        ),
        account: Some(account),
        customer: Some(error_customer),
    };
    let update_error = coactupc::AccountUpdateViewError {
        error: ScreenError::not_found(
            "Did not find this account in account master file",
            Some("acct_id".into()),
        ),
        state: Some("S".into()),
        details: Some(account_update_view(protected)?),
    };
    let result = coactupc::AccountUpdateResult {
        message: "Changes committed to database".into(),
        state: "C".into(),
        save_enabled: false,
        cancel_enabled: true,
        committed: true,
        amounts: Some(coactupc::AccountUpdateAmounts {
            credit_limit: "+5,000.00".into(),
            cash_credit_limit: "+1,500.00".into(),
            current_balance: "+1,020.00".into(),
            current_cycle_credit: "+0.00".into(),
            current_cycle_debit: "+0.00".into(),
        }),
    };
    Ok(vec![
        entry(&view)?,
        entry(&view_error)?,
        entry(&account_update_view(protected)?)?,
        entry(&update_error)?,
        entry(&result)?,
    ])
}

/// The card snapshot of endpoints 9 and 10.
fn card_snapshot() -> Fallible<cocrdupc::CardUpdateSnapshot> {
    Ok(cocrdupc::CardUpdateSnapshot {
        state: cocrdupc::CardUpdateState::ChangesOkNotConfirmed,
        acct_id: "00000000050".into(),
        embossed_name: "ANIYA VON".into(),
        expiration_date: Some(shown_date()?),
        status: "Y".into(),
    })
}

/// The endpoint 8 view showing `card_num`.
fn card_view(card_num: cocrdslc::CardViewNumber) -> Fallible<cocrdslc::CardViewResponse> {
    Ok(cocrdslc::CardViewResponse {
        acct_id: "00000000050".into(),
        card_ref: 1,
        card_num,
        embossed_name: "Aniya Von".into(),
        expiration_date: Some(shown_date()?),
        status: "Y".into(),
        message: Some("Displaying requested details".into()),
        info_message: Some("Enter or update id of card to search".into()),
    })
}

/// The response values of endpoints 7 to 10: endpoint 8's view redacted, as the `U` token gets
/// it, and disclosed, as the `A` token gets it.
fn card_entries(protected: &mut Protected) -> Fallible<Vec<Entry>> {
    let list = cocrdlic::CardListResponse {
        rows: vec![cocrdlic::CardListRow {
            card_ref: 1,
            card_num: protected.card()?,
            acct_id: "00000000050".into(),
            status: "Y".into(),
        }],
        blank_rows_above: 0,
        page_no: 1,
        first_key: Some(1),
        last_key: Some(1),
        has_next_page: false,
        last_page_shown: true,
        message: Some("NO MORE RECORDS TO SHOW".into()),
        info_message: Some("TYPE S FOR DETAIL, U TO UPDATE ANY RECORD".into()),
    };
    let redacted = card_view(cocrdslc::CardViewNumber::Redacted(protected.card()?))?;
    let shown = protected.card_text()?;
    let full = card_view(cocrdslc::CardViewNumber::Full(
        CardNumber::parse(&shown)?.disclose(),
    ))?;
    let edit = cocrdupc::CardUpdateView {
        card_ref: 1,
        acct_id: "00000000050".into(),
        card_num: protected.card()?,
        embossed_name: "ANIYA VON".into(),
        expiration_date: Some(shown_date()?),
        status: "Y".into(),
        snapshot: card_snapshot()?,
        message: Some("Details of selected card shown above".into()),
        info_message: Some("Update card details presented above.".into()),
    };
    let result = cocrdupc::CardUpdateResult {
        info_message: "Changes validated.Press F5 to save".into(),
        state: cocrdupc::CardUpdateState::ChangesOkNotConfirmed,
        save_enabled: true,
        committed: false,
        snapshot: Some(card_snapshot()?),
    };
    Ok(vec![
        entry(&list)?,
        entry(&redacted)?,
        Entry {
            disclosed: Some(shown),
            ..entry(&full)?
        },
        entry(&edit)?,
        entry(&result)?,
    ])
}

/// The response values of endpoints 11 to 15.
fn transaction_entries(protected: &mut Protected) -> Fallible<Vec<Entry>> {
    let list = cotrn00c::TranListResponse {
        rows: vec![Some(cotrn00c::TranListRow {
            tran_id: "0000000000000001".into(),
            orig_ts: Some(shown_instant()?),
            display_date: Some("07/18/22".into()),
            description: "Purchase at Abshire-Lowe  ".into(),
            amount: "+00000012.34".into(),
        })],
        rows_kept: false,
        first_key: Some("0000000000000001".into()),
        last_key: Some("0000000000000001".into()),
        page_no: 1,
        has_next_page: false,
        message: Some("You have reached the bottom of the page...".into()),
    };
    let view = cotrn01c::TranViewResponse {
        tran_id: "0000000000000001".into(),
        card_num: protected.card()?,
        type_cd: "01".into(),
        cat_cd: "0005".into(),
        source: "POS TERM".into(),
        description: "Purchase at Abshire-Lowe".into(),
        amount: "+00000012.34".into(),
        orig_ts: Some(shown_instant()?),
        proc_ts: Some(shown_instant()?),
        merchant_id: "800000000".into(),
        merchant_name: "Abshire-Lowe".into(),
        merchant_city: "North Enoshaven".into(),
        merchant_zip: "72112".into(),
        message: Some("Tran ID can NOT be empty...".into()),
    };
    let added = cotrn02c::TranAddResult {
        acct_id: Some("00000000050".into()),
        card_num: Some(protected.card()?),
        copied: Some(cotrn02c::TranAddCopiedFields {
            type_cd: "01".into(),
            cat_cd: "0005".into(),
            source: "POS TERM".into(),
            description: "Purchase at Abshire-Lowe".into(),
            amount: "+00000012.34".into(),
            orig_date: Some(shown_date()?),
            proc_date: Some(shown_date()?),
            merchant_id: "800000000".into(),
            merchant_name: "Abshire-Lowe".into(),
            merchant_city: "North Enoshaven".into(),
            merchant_zip: "72112".into(),
        }),
        field: "confirm".into(),
        message: "Transaction added successfully.".into(),
        new_tran_id: Some("0000000000000002".into()),
    };
    let report = corpt00c::ReportResult {
        message: "Monthly report submitted for printing ...".into(),
        job_deck: Some("TRNRPT00.G0001".into()),
    };
    let payment = cobil00c::BillPaymentResult {
        acct_id: "00000000001".into(),
        current_balance: Some("+1,020.00".into()),
        message: Some("Payment successful.".into()),
        new_tran_id: Some("0000000000000003".into()),
        cleared: true,
    };
    Ok(vec![
        entry(&list)?,
        entry(&view)?,
        entry(&added)?,
        entry(&report)?,
        entry(&payment)?,
    ])
}

/// The 200 of endpoint 22 for a fixture card number.
fn auth_detail(protected: &mut Protected) -> Fallible<copaus1c::AuthDetailResponse> {
    Ok(copaus1c::AuthDetailResponse {
        acct_id: "00000000050".into(),
        auth_key: "0778000896999999".into(),
        card_num: protected.card()?,
        auth_date: "07/18/22  ".into(),
        auth_time: "10:30:00  ".into(),
        response: "A".into(),
        reason: "0000-APPROVED       ".into(),
        auth_code: "000000".into(),
        amount: "       10.00".into(),
        pos_entry_mode: "0005".into(),
        source: "POS".into(),
        merchant_category_code: "5411".into(),
        card_expiry: "12/27".into(),
        auth_type: "AUTH".into(),
        transaction_id: "TXN000000000000".into(),
        match_status: "P".into(),
        fraud_status: "N".into(),
        merchant_name: "CORNER GROCERY".into(),
        merchant_id: "MERCH0000000001".into(),
        merchant_city: "SEATTLE".into(),
        merchant_state: "WA".into(),
        merchant_zip: "98101".into(),
        message: Some("Already at the last Authorization...".into()),
    })
}

/// The response values of endpoints 21 to 23.
fn authorization_entries(protected: &mut Protected) -> Fallible<Vec<Entry>> {
    let summary = copaus0c::AuthSummaryResponse {
        acct_id: "00000000050".into(),
        summary: copaus0c::AuthSummaryBlock {
            customer_id: "000000050".into(),
            customer_name: "Aniya Q Von".into(),
            address_line_1: "1 Main Street".into(),
            address_line_2: "Apt 2".into(),
            phone: "(312)555-0100".into(),
            credit_limit: "+5,000.00".into(),
            cash_limit: "+1,500.00".into(),
            summary_found: true,
            approved_count: "003".into(),
            declined_count: "000".into(),
            credit_balance: "+30.00".into(),
            cash_balance: "+0.00".into(),
            approved_amount: "+30.00".into(),
            declined_amount: "+0.00".into(),
        },
        rows: vec![copaus0c::AuthSummaryRow {
            auth_key: "0778000896999999".into(),
            transaction_id: "TXN000000000000".into(),
            date: "07/18/22".into(),
            time: "10:30:00".into(),
            auth_type: "AUTH".into(),
            approval: "A".into(),
            match_status: "P".into(),
            amount: "       10.00".into(),
        }],
        first_key: Some("0778000896999999".into()),
        last_key: Some("0778000896999999".into()),
        has_next_page: Some(false),
    };
    let toggled = copaus1c::FraudToggleResult {
        message: "AUTH FRAUD REMOVED...".into(),
        detail: Some(auth_detail(protected)?),
    };
    Ok(vec![
        entry(&summary)?,
        entry(&auth_detail(protected)?)?,
        entry(&toggled)?,
    ])
}

/// The state endpoints 24 and 25 return, every member given.
fn tran_type_list_state() -> cotrtlic::TranTypeListState {
    let row = Some(cotrtlic::TranTypeListRow {
        type_code: "01".into(),
        description: "Purchase".into(),
    });
    cotrtlic::TranTypeListState {
        type_filter: "01".into(),
        desc_filter: "Purchase".into(),
        rows: vec![row.clone()],
        selected_row: Some(1),
        first_key: Some("01".into()),
        last_key: Some("01".into()),
        page_no: 1,
        next_page_exists: false,
        last_page_shown: true,
        pending_action: cotrtlic::TranTypeListPendingAction::Update,
        screen_rows: vec![row],
        selectors: cotrtlic::TranTypeListSelectors::Open,
        description_entry: Some(1),
    }
}

/// The 200 bodies of endpoints 24 and 25 as JSON members. Both types keep their status
/// private, so they are read back from these members rather than built.
fn tran_type_list_members() -> Fallible<Map<String, Value>> {
    let row = cotrtlic::TranTypeListShownRow {
        type_code: "01".into(),
        description: "Purchase".into(),
        selection: "U".into(),
        highlighted: true,
        selector_enterable: true,
        description_enterable: true,
    };
    let filter = |text: &str| cotrtlic::TranTypeListShownFilter {
        text: text.into(),
        status: cotrtlic::TranTypeListFilterStatus::Valid,
    };
    let mut members = Map::new();
    members.insert("rows".into(), serde_json::to_value([Some(row)])?);
    members.insert("type_filter".into(), serde_json::to_value(filter("01"))?);
    members.insert(
        "desc_filter".into(),
        serde_json::to_value(filter("Purchase"))?,
    );
    members.insert("page_no".into(), Value::from(1));
    members.insert("has_next_page".into(), Value::from(false));
    members.insert("message".into(), Value::from("No more pages to display"));
    members.insert(
        "info_message".into(),
        Value::from("Type U to update, D to delete"),
    );
    members.insert(
        "state".into(),
        serde_json::to_value(tran_type_list_state())?,
    );
    // An opaque key: endpoint 25 reads it back, and its content is no part of these checks.
    members.insert("page_key".into(), Value::from("7b7d"));
    Ok(members)
}

/// The response values of endpoints 16 to 20 and 24 to 27.
fn admin_entries() -> Fallible<Vec<Entry>> {
    let list_members = tran_type_list_members()?;
    let type_list: cotrtlic::TranTypeListResponse =
        serde_json::from_value(Value::Object(list_members.clone()))?;
    let mut action_members = list_members;
    let row_error = cotrtlic::TranTypeListRowError {
        row: 1,
        field: "rows[0].description".into(),
        message: "Transaction Desc must be supplied.".into(),
    };
    action_members.insert("row_errors".into(), serde_json::to_value([row_error])?);
    let committed = cotrtlic::TranTypeListCommitted::Updated;
    action_members.insert("committed".into(), serde_json::to_value(committed)?);
    let type_action: cotrtlic::TranTypeListActionResult =
        serde_json::from_value(Value::Object(action_members))?;
    let type_view = cotrtupc::TranTypeView {
        type_code: "01".into(),
        description: Some("Purchase".into()),
        message: Some("Press F5 to save, F4 to delete".into()),
        info_message: "Update transaction type details shown.".into(),
        state: cotrtupc::TranTypeMaintState::ShowDetails,
    };
    let type_result = cotrtupc::TranTypeActionResult {
        type_code: "01".into(),
        description: Some("Purchase at a shop".into()),
        message: Some("Changes committed to database".into()),
        info_message: "Changes committed to database".into(),
        state: cotrtupc::TranTypeMaintState::ChangesOkayedAndDone,
        committed: true,
        deleted: false,
    };
    let users = cousr00c::UserListResponse {
        rows: Some(vec![Some(cousr00c::UserListRow {
            user_id: "USER0001".into(),
            first_name: "USER".into(),
            last_name: "ONE".into(),
            user_type: "U".into(),
        })]),
        page_no: 1,
        first_key: Some("USER0001".into()),
        last_key: Some("USER0001".into()),
        has_next_page: false,
        message: Some("You have reached the bottom of the page...".into()),
    };
    let user = cousr02c::UserView {
        user_id: "USER0001".into(),
        first_name: "USER".into(),
        last_name: "ONE".into(),
        user_type: "U".into(),
        update_prompt: "Press PF5 key to save your updates ...".into(),
        delete_prompt: "Press PF5 key to delete this user ...".into(),
    };
    let done = dto::ScreenResult {
        message: "User USER0001 has been updated ...".into(),
    };
    Ok(vec![
        entry(&users)?,
        entry(&done)?,
        entry(&user)?,
        entry(&type_list)?,
        entry(&type_action)?,
        entry(&type_view)?,
        entry(&type_result)?,
    ])
}

/// The response values of endpoints 1 to 3, and the error body every endpoint shares.
fn session_entries() -> Fallible<Vec<Entry>> {
    let signon = cosgn00c::SignonResponse {
        token: "header.payload.signature".into(),
        user_id: "ADMIN001".into(),
        user_type: "A".into(),
        next_route: "/admin".into(),
    };
    let menu = comen01c::MenuResponse {
        options: vec![comen01c::MenuOptionDto {
            number: "01".into(),
            name: "Account View".into(),
            text: "01. Account View".into(),
            program: "COACTVWC".into(),
            user_type: Some("U".into()),
            admin_only: false,
            installed: true,
            unavailable_message: Some("This option is not installed ...".into()),
        }],
        user_type: Some("U".into()),
        invalid_option_message: "Please enter a valid option number...".into(),
        admin_only_message: Some("No access - Admin Only option... ".into()),
    };
    let error = ScreenError::input("Please enter User ID ...", Some("user_id".into()));
    Ok(vec![entry(&signon)?, entry(&menu)?, entry(&error)?])
}

/// One populated value of every response type of the API, endpoint 8's view twice.
fn response_inventory(protected: &mut Protected) -> Fallible<Vec<Entry>> {
    let mut inventory = session_entries()?;
    inventory.extend(account_entries(protected)?);
    inventory.extend(card_entries(protected)?);
    inventory.extend(transaction_entries(protected)?);
    inventory.extend(authorization_entries(protected)?);
    inventory.extend(admin_entries()?);
    Ok(inventory)
}

/// Whether `schema` admits only `null`.
fn is_null_schema(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("null")
}

/// Records in `gaps` each place below `at` where `value` leaves what `schema` declares unfilled:
/// a `null`, a declared property absent, an array empty, or a value of another shape. It follows
/// `$ref` into `document` and reads `allOf`, `oneOf`, `anyOf`, `properties` and `items`. The
/// gaps name schema paths, never values.
fn unpopulated(document: &Value, schema: &Value, value: &Value, at: &str, gaps: &mut Vec<String>) {
    if value.is_null() {
        gaps.push(format!("{at} is null"));
        return;
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        match component(document, reference) {
            Some(target) => unpopulated(document, target, value, at, gaps),
            None => gaps.push(format!("{at} refers to no component")),
        }
        return;
    }
    for member in schema
        .get("allOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        unpopulated(document, member, value, at, gaps);
    }
    for key in ["oneOf", "anyOf"] {
        let Some(alternatives) = schema.get(key).and_then(Value::as_array) else {
            continue;
        };
        let filled = alternatives
            .iter()
            .filter(|alternative| !is_null_schema(alternative))
            .any(|alternative| {
                let mut trial = Vec::new();
                unpopulated(document, alternative, value, at, &mut trial);
                trial.is_empty()
            });
        if !filled {
            gaps.push(format!("{at} fills none of its {key} schemas"));
        }
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        let Some(object) = value.as_object() else {
            gaps.push(format!("{at} is not an object"));
            return;
        };
        for (name, property) in properties {
            let path = format!("{at}.{name}");
            match object.get(name) {
                Some(member) => unpopulated(document, property, member, &path, gaps),
                None => gaps.push(format!("{path} is absent")),
            }
        }
    }
    if let Some(items) = schema.get("items") {
        match value.as_array() {
            Some(elements) if elements.is_empty() => gaps.push(format!("{at} is empty")),
            Some(elements) => {
                for (index, element) in elements.iter().enumerate() {
                    unpopulated(document, items, element, &format!("{at}[{index}]"), gaps);
                }
            }
            None => gaps.push(format!("{at} is not an array")),
        }
    }
}

/// Every response type serialised: one populated value of each, every `Option` given, every
/// list holding an element and every card number and SSN a fixture value, holds no needle and
/// no member that breaks the rule. Endpoint 8's view is checked redacted, and disclosed through
/// [`disclosed_remainder`], outside its top-level `card_num` only.
///
/// The inventory is complete: its schema names are exactly the schemas the responses of the
/// emitted document refer to, and each value fills every property its schema declares, through
/// every reference, nested shapes included. It needs no Docker.
#[test]
fn every_response_type_is_redacted() -> TestResult {
    let needles = Needles::load()?;
    let document = serde_json::to_value(openapi())?;
    let inventory = response_inventory(&mut Protected::load()?)?;
    let mut gaps = Vec::new();
    let mut failures = Vec::new();
    for entry in &inventory {
        let value: Value = serde_json::from_str(&entry.json)?;
        let schema = component(&document, &format!("{SCHEMA_REF}{}", entry.name))
            .ok_or_else(|| format!("{} is no component schema", entry.name))?;
        unpopulated(&document, schema, &value, &entry.name, &mut gaps);
        let (scanned, walked) = match &entry.disclosed {
            Some(card) => {
                let remainder = disclosed_remainder(&entry.json, card, &needles)
                    .map_err(|error| format!("{}: {error}", entry.name))?;
                (remainder.to_string(), remainder)
            }
            None => (entry.json.clone(), value),
        };
        for found in needles.found(&scanned) {
            failures.push(format!("{}: {found}", entry.name));
        }
        for broken in walk_members(&walked).breaks {
            failures.push(format!("{}: {broken}", entry.name));
        }
    }
    assert!(
        failures.is_empty(),
        "response values break the redaction rule: {}",
        failures.join("; ")
    );
    assert!(
        gaps.is_empty(),
        "response values leave their schemas unfilled: {}",
        gaps.join("; ")
    );
    assert!(
        inventory.iter().any(|entry| entry.disclosed.is_some()),
        "the inventory holds no disclosed endpoint 8 view"
    );

    let named: BTreeSet<String> = inventory.iter().map(|entry| entry.name.clone()).collect();
    let referenced = response_schemas(&document)?;
    let missing: Vec<&str> = referenced.difference(&named).map(String::as_str).collect();
    let extra: Vec<&str> = named.difference(&referenced).map(String::as_str).collect();
    assert!(
        missing.is_empty(),
        "the inventory lacks the response schemas {}",
        missing.join(", ")
    );
    assert!(
        extra.is_empty(),
        "the inventory holds schemas no response refers to: {}",
        extra.join(", ")
    );
    Ok(())
}
