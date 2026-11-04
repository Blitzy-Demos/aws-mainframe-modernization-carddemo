//! `COPAUS1C` with its linked `COPAUS2C` (CPVD): one pending authorization,
//! the step to the next one, and the fraud mark
//! [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl]
//! [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl], served as
//! `GET /authorizations/{id}` and `PUT /authorizations/{id}`.
//!
//! The REST key is the row identity `auth_id` of `pending_auth_details`; the
//! IMS key columns stay in the record. Four COBOL operations are ported:
//!
//! - ENTER, [`detail`]: `PROCESS-ENTER-KEY` and `READ-AUTH-RECORD`, the
//!   `GU PAUTSUM0` by account and the `GNP PAUTDTL1` by the selected key
//!   (P-COPAUS1C-003, P-COPAUS1C-004). The key edit is also [`edit_auth_id`],
//!   which reads nothing, for an adapter to run before it opens the store.
//! - PF8, [`next`]: `PROCESS-PF8-KEY` and `READ-NEXT-AUTH-RECORD`, the
//!   unqualified `GNP` that reaches the next older authorization under the same
//!   summary, from the detail [`detail`] read (P-COPAUS1C-009, P-COPAUS1C-014).
//! - PF5, [`detail_for_update`] then [`toggle_fraud`]: `MARK-AUTH-FRAUD`, whose
//!   `READ-AUTH-RECORD` is the locked read, the `COPAUS2C` fraud report
//!   ([`fraud_row`], insert or `FRAUD-UPDATE`) and `UPDATE-AUTH-DETAILS`
//!   (P-COPAUS1C-006 to P-COPAUS1C-008, P-COPAUS2C-001 to P-COPAUS2C-006).
//! - The reason text `POPULATE-AUTH-DETAILS` shows, [`decline_reason`]
//!   (P-COPAUS1C-005). Its other moves copy stored fields or cut them by
//!   reference modification, which the response mapping does.
//!
//! # Reads
//!
//! `READ-AUTH-RECORD` reads the summary of the account, then the selected
//! detail under it, for ENTER, PF5 and PF8 alike. The REST key names the
//! detail, not the account the COMMAREA carried (P-COPAUS1C-012), so the
//! summary read resolves the detail's account server-side, and a detail of
//! another account than the summary read is not found under it. The two
//! reads and their outcomes follow the `EVALUATE` blocks of the paragraph
//! (P-COPAUS1C-004, P-COPAUS1C-014): see [`detail`].
//!
//! PF8 steps from the detail already read, with one lookup of the twin after
//! its `PAUT9CTS` key; nothing is read twice.
//!
//! # The fraud-mark lock
//!
//! The PF5 detail read takes a row lock held until the caller commits or rolls
//! back, and [`toggle_fraud`] accepts only the [`LockedDetail`] that read
//! returns, so the flag the toggle derives its action from is the one it
//! replaces. `COPAUS1C` takes no lock of its own, reading and replacing within
//! one CICS task; the lock is the concurrency safeguard that keeps two
//! overlapping fraud marks of one authorization from both reading the same
//! flag and both applying `F`. The marks serialize: the second waits for the
//! first to end and toggles the flag it committed.
//!
//! # Unit of work
//!
//! The rules commit nothing. A read path ends with `TAKE-SYNCPOINT`, which
//! commits nothing either. On the fraud path the `AUTHFRDS` write and the IMS
//! detail replace are recoverable resources that persist only together, at the
//! `SYNCPOINT` after the replace; `ROLL-BACK` backs out both (P-COPAUS1C-007,
//! P-COPAUS1C-015). So the caller commits when [`toggle_fraud`] returns `Ok`
//! and rolls the whole unit of work back when it returns `Err`, even though
//! each port write runs under its own savepoint.
//!
//! # Errors and the cursor
//!
//! `SEND-AUTHVIEW-SCREEN` always puts the cursor on `CARDNUM`
//! (`MOVE -1 TO CARDNUML`, P-COPAUS1C-011), so every error names
//! [`CARD_NUM_FIELD`]. Paths the COBOL leaves without a message carry an empty
//! one, with the status of their validation-audit row (E-49). The status code
//! in an IMS message slot is `GE` for a missing record, `II` for a duplicate
//! and `AO` for any other database failure, the codes E-44 proposes.
//!
//! COPAUS2C is a direct call here, not a CICS `LINK`, so the LINK failure that
//! rolls back without a message (V-COPAUS1C-008) has no counterpart. The
//! scheduling failure of `SCHEDULE-PSB` (P-COPAUS1C-016, P-COPAUS1C-017) has
//! none either: no PSB is scheduled, and a database failure is the read
//! failure of the call that met it.

use chrono::{Datelike, NaiveDate, NaiveDateTime};

use crate::clock::Clock;
use crate::entities::{AuthFraud, FraudFlag};
use crate::error::DomainError;
use crate::messages::{
    ALREADY_AT_THE_LAST_AUTHORIZATION, AUTH_FRAUD_REMOVED, AUTH_MARKED_FRAUD,
    SYSTEM_ERROR_DB2_CODE, SYSTEM_ERROR_WHILE_FRAUD_TAGGING_ROLLBACK,
    SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS1C,
    SYSTEM_ERROR_WHILE_READING_AUTH_SUMMARY_COPAUS1C, SYSTEM_ERROR_WHILE_READING_NEXT_AUTH,
    UNEXPECTED_ERROR, UPDT_ERROR_DB2_CODE, fraud_db2_error_message, ims_status_message,
};
use crate::numeric::{AccountId, Alnum, AuthId, CustomerId};
use crate::ports::{Direction, PortError};

/// The detail record and the authorization store [`detail`], [`next`] and
/// [`toggle_fraud`] work on, re-exported from [`crate::entities`] and
/// [`crate::ports`] for the API adapter.
pub use crate::entities::PendingAuthDetail;
pub use crate::ports::AuthorizationStore;

/// `CARDNUM`, the card number field of map `COPAU1A`
/// [app/app-authorization-ims-db2-mq/cpy-bms/COPAU01.cpy:L55-60]: the BMS
/// field of every `COPAUS1C` message (V-COPAUS1C-001 to V-COPAUS1C-013).
pub const CARD_NUM_FIELD: &str = "CARDNUM";

/// The text of a path that sets no message (E-49); its status alone tells the
/// outcome.
const NO_MESSAGE: &str = "";

/// The IMS status of a segment that is not found, `GE`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L81].
const IMS_SEGMENT_NOT_FOUND: &str = "GE";

/// The IMS status of a duplicate segment, `II`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L82].
const IMS_DUPLICATE_SEGMENT: &str = "II";

/// The IMS status `AO`, an I/O error: the code E-44 proposes for any database
/// failure that has no IMS counterpart.
const IMS_IO_ERROR: &str = "AO";

/// SQLSTATE `02000`, no row, as `MOVE SQLSTATE TO WS-SQLSTATE PIC +9(09)`
/// stores it. Db2 and `PostgreSQL` give it to SQLCODE +100 alike.
const SQLSTATE_NO_ROW: u32 = 2_000;

/// SQLSTATE `23505`, unique violation, as `WS-SQLSTATE` stores it. Db2 and
/// `PostgreSQL` give it to SQLCODE -803 alike.
const SQLSTATE_UNIQUE_VIOLATION: u32 = 23_505;

/// `WS-AUTH-TS`, the 23 bytes `YY-MM-DD HH.MI.SSsss000` `COPAUS2C` builds
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L38-51].
const WS_AUTH_TS_LEN: usize = 23;

/// `999999999`, from which `PA-AUTH-TIME-9C` is complemented
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L107].
const AUTH_TIME_COMPLEMENT: i64 = 999_999_999;

/// `10^9`: `WS-AUTH-TIME PIC 9(09)` keeps the low-order nine digits.
const AUTH_TIME_MODULUS: u64 = 1_000_000_000;

/// `AUTHRSNO PIC X(20)` [app/app-authorization-ims-db2-mq/cpy-bms/COPAU01.cpy:L248].
const AUTH_REASON_LEN: usize = 20;

/// `AUTHRSNO(6:)`, the 15 bytes after the code and its hyphen.
const AUTH_REASON_TEXT_LEN: usize = 15;

/// `WS-DECLINE-REASON-TABLE`: each `X(20)` entry's `DECL-CODE X(4)` and
/// `DECL-DESC X(16)`, in its `ASCENDING KEY` order
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L57-73] (P-COPAUS1C-005).
const DECLINE_REASONS: [(&[u8; 4], &str); 10] = [
    (b"0000", "APPROVED"),
    (b"3100", "INVALID CARD"),
    (b"4100", "INSUFFICNT FUND"),
    (b"4200", "CARD NOT ACTIVE"),
    (b"4300", "ACCOUNT CLOSED"),
    (b"4400", "EXCED DAILY LMT"),
    (b"5100", "CARD FRAUD"),
    (b"5200", "MERCHANT FRAUD"),
    (b"5300", "LOST CARD"),
    (b"9000", "UNKNOWN"),
];

/// The code and text `SEARCH ALL … AT END` moves when no entry matches
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L320-323].
const UNKNOWN_REASON: (&[u8; 4], &str) = (b"9999", "ERROR");

/// The result of PF8: the next authorization, or the text shown at the last
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailStep {
    /// The next detail under the same summary in IMS twin order, which is the
    /// next older authorization (P-COPAUS1C-014).
    // Boxed: a detail is over 200 bytes and the other variant 16, a size gap
    // the denied `clippy::large_enum_variant` rejects unboxed.
    Next(Box<PendingAuthDetail>),
    /// No detail follows: `GNP` answered `GE` or `GB`. The current detail
    /// stays shown with this text, `Already at the last Authorization...`,
    /// and no error (V-COPAUS1C-012).
    AtEnd {
        /// The text `PROCESS-PF8-KEY` moves to `WS-MESSAGE`.
        message: &'static str,
    },
}

/// The outcome of a committed fraud toggle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FraudOutcome {
    /// The detail as `UPDATE-AUTH-DETAILS` replaced it: the toggled
    /// `PA-AUTH-FRAUD` and the `PA-FRAUD-RPT-DATE` `COPAUS2C` set.
    pub detail: PendingAuthDetail,
    /// `AUTH MARKED FRAUD...` when the flag became `F`,
    /// `AUTH FRAUD REMOVED...` when it became `R` (P-COPAUS1C-008).
    pub message: &'static str,
}

/// The detail PF5 toggles, read by [`detail_for_update`] under a row lock
/// that lasts until the caller's unit of work ends (P-COPAUS1C-006).
///
/// Only [`detail_for_update`] builds one, and [`toggle_fraud`] consumes it, so
/// a detail from the unlocked view read cannot reach the toggle, and one
/// locked read serves exactly one toggle. It is not `Clone` for that reason.
#[derive(Debug, PartialEq, Eq)]
pub struct LockedDetail(PendingAuthDetail);

impl LockedDetail {
    /// The detail as the locked read returned it.
    #[must_use]
    pub const fn detail(&self) -> &PendingAuthDetail {
        &self.0
    }
}

/// The detail read `READ-AUTH-RECORD` issues after the summary `GU`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailRead {
    /// The view and PF8: a plain read, no lock.
    Plain,
    /// PF5: the read that locks the row until the unit of work ends.
    ForUpdate,
}

/// The IMS status a port failure stands for in an `IMS-RETURN-CODE` slot
/// (E-44): `GE` for a missing record, `II` for a duplicate, `AO` otherwise.
const fn ims_status(error: &PortError) -> &'static str {
    match error {
        PortError::NotFound => IMS_SEGMENT_NOT_FOUND,
        PortError::Duplicate => IMS_DUPLICATE_SEGMENT,
        PortError::Restricted | PortError::Contention | PortError::Unavailable(_) => IMS_IO_ERROR,
    }
}

/// An `Unexpected` error with the IMS text `head` and the status of `error`.
fn ims_failure(head: &str, error: &PortError) -> DomainError {
    DomainError::unexpected(ims_status_message(head, ims_status(error))).with_field(CARD_NUM_FIELD)
}

/// The SQLCODE and SQLSTATE a failed `AUTHFRDS` statement puts in the
/// `WS-SQLCODE` and `WS-SQLSTATE` slots, when the port pins both.
///
/// E-30 fills the SQLCODE slot with the Db2 code of the mapped condition and
/// takes the SQLSTATE from the database. Only a missing row (+100, `02000`)
/// and a duplicate key (-803, `23505`) have a SQLSTATE that Db2 and
/// `PostgreSQL` share; the port carries no SQLSTATE for any other failure, so
/// none is guessed and `None` is returned.
fn db2_slots(error: &PortError) -> Option<(i32, u32)> {
    let sqlstate = match error {
        PortError::NotFound => SQLSTATE_NO_ROW,
        PortError::Duplicate => SQLSTATE_UNIQUE_VIOLATION,
        PortError::Restricted | PortError::Contention | PortError::Unavailable(_) => return None,
    };
    error.sqlcode().map(|sqlcode| (sqlcode, sqlstate))
}

/// COPAUS2C MAIN-PARA, FRAUD-UPDATE — P-COPAUS2C-005, V-COPAUS1C-009,
/// V-COPAUS1C-010 [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L206-243]:
/// the `WS-FRD-ACT-MSG` text of a failed fraud-table write, which
/// `MARK-AUTH-FRAUD` moves to `WS-MESSAGE` before `ROLL-BACK`.
///
/// `head` is ` SYSTEM ERROR DB2: CODE:` for the insert and
/// ` UPDT ERROR DB2: CODE:` for the update. When [`db2_slots`] cannot pin the
/// codes, the text is the unexpected-error text instead.
fn fraud_write_failure(head: &str, error: &PortError) -> DomainError {
    let message = db2_slots(error).map_or_else(
        || UNEXPECTED_ERROR.to_owned(),
        |(sqlcode, sqlstate)| fraud_db2_error_message(head, sqlcode, sqlstate),
    );
    DomainError::unexpected(message).with_field(CARD_NUM_FIELD)
}

/// COPAUS1C PROCESS-ENTER-KEY — V-COPAUS1C-001, E-49
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L208-227]: the key edit
/// before `READ-AUTH-RECORD`.
///
/// `PROCESS-ENTER-KEY` reads only with a numeric account and a selected key;
/// otherwise it sets `WS-ERR-FLG` with no message and reads nothing. A REST
/// identity that is not a positive integer is that case (E-49). The edit reads
/// no store, so an adapter runs it before it opens one, and the rejection
/// precedes the scheduling failure of `SCHEDULE-PSB` (V-COPAUS1C-002).
///
/// # Errors
///
/// [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) with no message on
/// [`CARD_NUM_FIELD`] when `auth_id` is not positive.
pub fn edit_auth_id(auth_id: AuthId) -> Result<AuthId, DomainError> {
    if auth_id.get() <= 0 {
        return Err(DomainError::invalid(NO_MESSAGE).with_field(CARD_NUM_FIELD));
    }
    Ok(auth_id)
}

/// COPAUS1C READ-AUTH-RECORD — P-COPAUS1C-003, P-COPAUS1C-004,
/// P-COPAUS1C-014, V-COPAUS1C-001, V-COPAUS1C-003 to V-COPAUS1C-006
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L431-491]: the summary
/// `GU` by account, then the detail `GNP` by the selected key, for the detail
/// `auth_id` names.
///
/// `PROCESS-ENTER-KEY` reads only with a numeric account and a selected key;
/// a REST identity that is not a positive integer is that case (E-49), which
/// [`edit_auth_id`] rejects before anything is read. The account of the `GU`
/// is the selected detail's own, resolved by
/// [`AuthorizationStore::read_summary_of_detail`], since the request carries
/// no COMMAREA account (P-COPAUS1C-012). `read` chooses the plain detail read
/// or the locking one.
///
/// The outcome follows the two `EVALUATE` blocks:
///
/// 1. The `GU` answers `GE` or `GB`: `AUTHS-EOF`, so the `GNP` is not issued
///    and the answer is not found with no message (V-COPAUS1C-004).
/// 2. Any other `GU` failure moves its text to `WS-MESSAGE` but leaves
///    `AUTHS-NOT-EOF` as it stood, so the `GNP` still runs (P-COPAUS1C-014).
///    When the `GNP` fails too, its detail text overwrites the summary text
///    (V-COPAUS1C-006); otherwise the summary text stands, whatever the `GNP`
///    found (V-COPAUS1C-003). Both carry the IMS status E-44 proposes.
/// 3. With the summary read, a `GNP` that answers `GE` or `GB` is not found
///    with no message (V-COPAUS1C-005), and so is a detail under another
///    account than the summary's, which the `GNP` qualified under that
///    summary cannot reach.
async fn read_auth_record<S: AuthorizationStore>(
    store: &mut S,
    auth_id: AuthId,
    read: DetailRead,
) -> Result<PendingAuthDetail, DomainError> {
    let auth_id = edit_auth_id(auth_id)?;
    let summary = match store.read_summary_of_detail(auth_id).await {
        Err(PortError::NotFound) => return Err(not_found()),
        other => other,
    };
    let detail = match read {
        DetailRead::Plain => store.read_detail(auth_id).await,
        DetailRead::ForUpdate => store.read_detail_for_update(auth_id).await,
    };
    let detail = match detail {
        Ok(detail) => Some(detail),
        Err(PortError::NotFound) => None,
        Err(other) => {
            return Err(ims_failure(
                SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS1C,
                &other,
            ));
        }
    };
    let summary = summary
        .map_err(|error| ims_failure(SYSTEM_ERROR_WHILE_READING_AUTH_SUMMARY_COPAUS1C, &error))?;
    detail
        .filter(|detail| detail.pa_acct_id == summary.pa_acct_id)
        .ok_or_else(not_found)
}

/// The answer of a read that finds no detail: `GE` or `GB` sets `AUTHS-EOF`
/// with no message (V-COPAUS1C-004, V-COPAUS1C-005).
fn not_found() -> DomainError {
    DomainError::not_found(NO_MESSAGE).with_field(CARD_NUM_FIELD)
}

/// COPAUS1C PROCESS-ENTER-KEY, READ-AUTH-RECORD — P-COPAUS1C-001,
/// P-COPAUS1C-003, P-COPAUS1C-004, V-COPAUS1C-001, V-COPAUS1C-003 to
/// V-COPAUS1C-006 [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L208-228]:
/// the authorization the view shows, as stored, read under the summary of its
/// account and without a lock.
///
/// # Errors
///
/// Each on [`CARD_NUM_FIELD`], in the precedence `READ-AUTH-RECORD` gives
/// them:
///
/// - [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) with no message
///   when `auth_id` is not a positive integer (V-COPAUS1C-001); nothing is
///   read.
/// - [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) with no
///   message when no summary is found for the detail's account, and then no
///   detail is read (V-COPAUS1C-004).
/// - [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with
///   ` System error while reading Auth Details: Code:` and the IMS status when
///   the detail read fails (V-COPAUS1C-006, E-44), whatever the summary read
///   answered.
/// - [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with
///   ` System error while reading Auth Summary: Code:` and the IMS status when
///   the summary read fails and the detail read does not (V-COPAUS1C-003,
///   E-44).
/// - [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) with no
///   message when no detail has the identity, or the detail is not under the
///   summary read (V-COPAUS1C-005).
pub async fn detail<S: AuthorizationStore>(
    store: &mut S,
    auth_id: AuthId,
) -> Result<PendingAuthDetail, DomainError> {
    read_auth_record(store, auth_id, DetailRead::Plain).await
}

/// COPAUS1C MARK-AUTH-FRAUD, READ-AUTH-RECORD — P-COPAUS1C-003,
/// P-COPAUS1C-004, P-COPAUS1C-006, V-COPAUS1C-001, V-COPAUS1C-003 to
/// V-COPAUS1C-007 [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L230-234]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L431-491]: the detail
/// PF5 toggles, read as [`detail`] reads it but with the row locked until the
/// caller's unit of work ends.
///
/// The lock is taken by the detail read itself, before the flag is seen, so
/// a second fraud mark of the same authorization waits for the first to
/// commit or roll back and then reads the flag it left (see the module's
/// fraud-mark lock).
///
/// # Errors
///
/// Those of [`detail`], in the same precedence. A lock that cannot be taken
/// fails the detail read, so it answers the detail-read text with status `AO`
/// (V-COPAUS1C-006, E-44). `MARK-AUTH-FRAUD` does not test the read; E-53
/// decides that its outcome is returned and nothing is written
/// (V-COPAUS1C-007).
pub async fn detail_for_update<S: AuthorizationStore>(
    store: &mut S,
    auth_id: AuthId,
) -> Result<LockedDetail, DomainError> {
    read_auth_record(store, auth_id, DetailRead::ForUpdate)
        .await
        .map(LockedDetail)
}

/// COPAUS1C PROCESS-PF8-KEY, READ-NEXT-AUTH-RECORD — P-COPAUS1C-009,
/// P-COPAUS1C-014, V-COPAUS1C-012, V-COPAUS1C-013
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L268-289]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L493-518]: the
/// authorization after `current` under the same summary.
///
/// `current` is the detail [`detail`] returned, the `READ-AUTH-RECORD` that
/// `PROCESS-PF8-KEY` performs to position the unqualified `GNP`; the step is
/// one lookup of the twin after its account and `PAUT9CTS` key, with no read
/// of `current` or its summary again. When that read fails, the caller has
/// its outcome and does not step: the COBOL still issues the `GNP` from a
/// position the failed read left, which the audit does not pin, so no next
/// detail is guessed, as E-53 decides for the fraud mark.
///
/// A `GNP` that answers `GE` or `GB`, the port's `None`, is the end of the
/// twins: `Already at the last Authorization...` and no error
/// (V-COPAUS1C-012).
///
/// # Errors
///
/// [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with
/// ` System error while reading next Auth: Code:` and the IMS status when the
/// next-detail read fails (V-COPAUS1C-013, E-44), on [`CARD_NUM_FIELD`].
pub async fn next<S: AuthorizationStore>(
    store: &mut S,
    current: &PendingAuthDetail,
) -> Result<DetailStep, DomainError> {
    let step = store
        .adjacent_detail(
            current.pa_acct_id,
            current.pa_auth_date_9c,
            current.pa_auth_time_9c,
            Direction::Forward,
        )
        .await;
    match step {
        Ok(Some(next)) => Ok(DetailStep::Next(Box::new(next))),
        // GE as well as GB sets AUTHS-EOF in READ-NEXT-AUTH-RECORD.
        Ok(None) | Err(PortError::NotFound) => Ok(DetailStep::AtEnd {
            message: ALREADY_AT_THE_LAST_AUTHORIZATION,
        }),
        Err(other) => Err(ims_failure(SYSTEM_ERROR_WHILE_READING_NEXT_AUTH, &other)),
    }
}

/// COPAUS2C MAIN-PARA — P-COPAUS2C-001
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L107]:
/// `COMPUTE WS-AUTH-TIME = 999999999 - PA-AUTH-TIME-9C`, the authorization
/// time `HHMMSSmmm` the key holds complemented.
///
/// The receiver is the unsigned `PIC 9(09)`, so it keeps the magnitude of the
/// difference and only its nine low-order digits. A key `COPAUA0C` wrote is
/// at most `999999999` and gives the time back unchanged.
fn auth_time(pa_auth_time_9c: i32) -> u64 {
    (AUTH_TIME_COMPLEMENT - i64::from(pa_auth_time_9c)).unsigned_abs() % AUTH_TIME_MODULUS
}

/// COPAUS2C MAIN-PARA — P-COPAUS2C-001
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L103-111]:
/// `WS-AUTH-TS`, `YY-MM-DD HH.MI.SSsss000`.
///
/// Year, month and day are the bytes of `PA-AUTH-ORIG-DATE`, as stored; the
/// time is [`auth_time`] in nine digits, its milliseconds followed by the
/// `FILLER VALUE '000'`.
fn ws_auth_ts(detail: &PendingAuthDetail) -> Alnum<WS_AUTH_TS_LEN> {
    let [y1, y2, m1, m2, d1, d2] = *detail.pa_auth_orig_date.as_bytes();
    let mut bytes = vec![y1, y2, b'-', m1, m2, b'-', d1, d2, b' '];
    let time = format!("{:09}", auth_time(detail.pa_auth_time_9c));
    for (index, digit) in time.bytes().enumerate() {
        // `.` before the minutes (WS-AUTH-MI) and before the seconds (WS-AUTH-SS).
        if matches!(index, 2 | 4) {
            bytes.push(b'.');
        }
        bytes.push(digit);
    }
    bytes.extend_from_slice(b"000");
    Alnum::move_from_bytes(&bytes)
}

/// The value of `bytes` when every one is an ASCII digit.
fn digits(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0_u32, |value, &byte| {
        byte.is_ascii_digit()
            .then(|| value * 10 + u32::from(byte - b'0'))
    })
}

/// COPAUS2C MAIN-PARA, FRAUD-UPDATE — P-COPAUS2C-001
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L171-172]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L227-228]:
/// `TIMESTAMP_FORMAT(:AUTH-TS, 'YY-MM-DD HH24.MI.SSNNNNNN')`, the `AUTH_TS`
/// value Db2 stores and matches.
///
/// `NNNNNN` reads the six digits after the seconds as microseconds, so the
/// milliseconds become microseconds times 1000. A `YY` year takes its first
/// two digits from the current year (IBM Db2 12 for z/OS, `TIMESTAMP_FORMAT`),
/// which is the year of `current_date`, Db2 `CURRENT DATE`. The host
/// variable's trailing spaces are not part of the value.
///
/// `None` when a field is not digits or the values form no timestamp: Db2
/// then rejects the statement with a SQLCODE the audit does not record.
fn timestamp_format(
    text: &Alnum<WS_AUTH_TS_LEN>,
    current_date: NaiveDate,
) -> Option<NaiveDateTime> {
    let field = |start: usize, len: usize| text.ref_mod(start, len).and_then(digits);
    let century = current_date.year().rem_euclid(10_000) / 100;
    let year = century * 100 + i32::try_from(field(1, 2)?).ok()?;
    NaiveDate::from_ymd_opt(year, field(4, 2)?, field(7, 2)?)?.and_hms_micro_opt(
        field(10, 2)?,
        field(13, 2)?,
        field(16, 2)?,
        field(18, 6)?,
    )
}

/// COPAUS2C MAIN-PARA — P-COPAUS2C-001, P-COPAUS2C-006
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L91-101]:
/// `FORMATTIME MMDDYY DATESEP` into `WS-CUR-DATE X(08)`, moved to
/// `PA-FRAUD-RPT-DATE`: `mm/dd/yy`, `/` being the default separator.
fn fraud_report_date_text(date: NaiveDate) -> Alnum<8> {
    let year = date.year().rem_euclid(100);
    Alnum::move_from(&format!("{:02}/{:02}/{year:02}", date.month(), date.day()))
}

/// COPAUS2C MAIN-PARA — P-COPAUS2C-001, P-COPAUS2C-002
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L103-139]: the
/// `CARDDEMO.AUTHFRDS` row `COPAUS2C` inserts for `detail`.
///
/// Every column holds a value, as `COPAUS2C` passes no null indicator. The
/// detail fields are moved unchanged, `PA-PROCESSING-CODE 9(06)` as its six
/// digits and `PA-MERCHANT-NAME` at its full 22 bytes (`MERCHANT-NAME-LEN` is
/// `LENGTH OF PA-MERCHANT-NAME`). `AUTH_TS` is the `TIMESTAMP_FORMAT` of
/// `WS-AUTH-TS`, its two-digit year completed with the century of
/// `report_date`, `AUTH_FRAUD` the requested `action` (`WS-FRD-ACTION`, `F` to
/// report or `R` to remove), `FRAUD_RPT_DATE` the `report_date` that stands
/// for Db2 `CURRENT DATE`, and `ACCT_ID` and `CUST_ID` the account and
/// customer `COPAUS1C` passes.
///
/// `None` when `PA-AUTH-ORIG-DATE` and the complemented `PA-AUTH-TIME-9C` give
/// no timestamp `TIMESTAMP_FORMAT` accepts. The two come from different
/// sources (P-COPAUA0C-014). `COPAUA0C` moves `PA-RQ-AUTH-DATE` to
/// `PA-AUTH-ORIG-DATE` unedited, so a detail exactly as `COPAUA0C` stored it
/// meets this whenever the request's date is not six digits `YYMMDD` that
/// form a calendar date in the century of `report_date`. It complements
/// `PA-AUTH-TIME-9C` from the `ASKTIME` clock, which gives a time of day back
/// for every detail it writes, so the time fails only for a key time no clock
/// gives.
// Returns `Option`, not a bare row: `AUTH_TS` is NOT NULL, and a detail whose
// request-sourced original date or key time forms no timestamp yields none, so
// no row can be built for it.
#[must_use]
pub fn fraud_row(
    detail: &PendingAuthDetail,
    account_id: AccountId,
    customer_id: CustomerId,
    action: FraudFlag,
    report_date: NaiveDate,
) -> Option<AuthFraud> {
    let auth_ts = timestamp_format(&ws_auth_ts(detail), report_date)?;
    Some(AuthFraud {
        card_num: detail.pa_card_num,
        auth_ts,
        auth_type: Some(detail.pa_auth_type),
        card_expiry_date: Some(detail.pa_card_expiry_date),
        message_type: Some(detail.pa_message_type),
        message_source: Some(detail.pa_message_source),
        auth_id_code: Some(detail.pa_auth_id_code),
        auth_resp_code: Some(detail.pa_auth_resp_code),
        auth_resp_reason: Some(detail.pa_auth_resp_reason),
        processing_code: Some(Alnum::move_from(&detail.pa_processing_code.to_string())),
        transaction_amt: Some(detail.pa_transaction_amt),
        approved_amt: Some(detail.pa_approved_amt),
        merchant_catagory_code: Some(detail.pa_merchant_catagory_code),
        acqr_country_code: Some(detail.pa_acqr_country_code),
        pos_entry_mode: Some(detail.pa_pos_entry_mode),
        merchant_id: Some(detail.pa_merchant_id),
        merchant_name: Some(detail.pa_merchant_name),
        merchant_city: Some(detail.pa_merchant_city),
        merchant_state: Some(detail.pa_merchant_state),
        merchant_zip: Some(detail.pa_merchant_zip),
        transaction_id: Some(detail.pa_transaction_id),
        match_status: Some(detail.pa_match_status.to_alnum()),
        auth_fraud: Some(action.to_alnum()),
        fraud_rpt_date: Some(report_date),
        acct_id: Some(account_id),
        cust_id: Some(customer_id),
    })
}

/// COPAUS1C MARK-AUTH-FRAUD — P-COPAUS1C-006
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L236-242]: a detail
/// holding `F` (`PA-FRAUD-CONFIRMED`) becomes `R` and the report is removed;
/// any other detail becomes `F` and fraud is reported. The new flag is also
/// the `WS-FRD-ACTION` passed to `COPAUS2C`.
fn toggled(flag: FraudFlag) -> FraudFlag {
    if flag == FraudFlag::Confirmed {
        FraudFlag::Removed
    } else {
        FraudFlag::Confirmed
    }
}

/// COPAUS2C MAIN-PARA, FRAUD-UPDATE — P-COPAUS2C-003 to P-COPAUS2C-005,
/// V-COPAUS1C-009, V-COPAUS1C-010
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L141-244]: the insert
/// of `fraud`, and on a duplicate key (SQLCODE -803) the update of its
/// `AUTH_FRAUD` and `FRAUD_RPT_DATE`.
///
/// The success texts `ADD SUCCESS` and `UPDT SUCCESS` stay in `COPAUS2C`'s
/// commarea: `COPAUS1C` shows the text of its own replace instead.
async fn report_fraud<S: AuthorizationStore>(
    store: &mut S,
    fraud: AuthFraud,
) -> Result<(), DomainError> {
    match store.insert_fraud(fraud.clone()).await {
        Ok(()) => Ok(()),
        Err(PortError::Duplicate) => store
            .update_fraud(fraud)
            .await
            .map_err(|error| fraud_write_failure(UPDT_ERROR_DB2_CODE, &error)),
        Err(other) => Err(fraud_write_failure(SYSTEM_ERROR_DB2_CODE, &other)),
    }
}

/// COPAUS1C MARK-AUTH-FRAUD, UPDATE-AUTH-DETAILS with COPAUS2C —
/// P-COPAUS1C-006 to P-COPAUS1C-008, P-COPAUS1C-015, P-COPAUS2C-001 to
/// P-COPAUS2C-006, V-COPAUS1C-007, V-COPAUS1C-009 to V-COPAUS1C-011
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L230-266]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L520-552]: toggles the
/// fraud mark of the detail `locked` holds.
///
/// In COBOL order, from the detail [`detail_for_update`] read and locked: its
/// flag is toggled, `F` to `R` and any other value to `F`; `COPAUS2C` stamps
/// `PA-FRAUD-RPT-DATE` with the date of `clock` as `mm/dd/yy` and writes the
/// [`fraud_row`] for the detail's account (`WS-FRD-ACCT-ID`, the
/// `CDEMO-ACCT-ID` of the summary the read was qualified by) and
/// `customer_id`, inserting it or, when the authorization was reported
/// before, updating its flag and report date; then the detail is replaced
/// with the new flag and date. The detail is not read again: the lock keeps
/// the flag the action was derived from in place until the unit of work
/// ends. The instant is read once, so the IMS date and Db2 `CURRENT DATE` are
/// the same day.
///
/// The rule commits nothing: the caller commits on `Ok` and rolls the whole
/// unit of work back on `Err`, which undoes a fraud-table write that a later
/// failure followed and releases the lock (P-COPAUS1C-015).
///
/// # Errors
///
/// Each on [`CARD_NUM_FIELD`], and nothing is written unless a write is
/// named. A detail that cannot be read never gets here: [`detail_for_update`]
/// returns the read's outcome and nothing changes (V-COPAUS1C-007, E-53).
///
/// 1. [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with the
///    unexpected-error text when the detail's date and time give no
///    `AUTH_TS` (see [`fraud_row`]).
/// 2. [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with
///    ` SYSTEM ERROR DB2: CODE:…, STATE: …` when the insert fails other than
///    by a duplicate key, or ` UPDT ERROR DB2: CODE:…, STATE: …` when the
///    update after it fails (V-COPAUS1C-009, V-COPAUS1C-010); the
///    unexpected-error text where the codes are not pinned (E-30).
/// 3. [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with
///    ` System error while FRAUD Tagging, ROLLBACK||` and the IMS status when
///    the detail replace fails after the fraud-table write
///    (V-COPAUS1C-011, E-44).
pub async fn toggle_fraud<S: AuthorizationStore>(
    store: &mut S,
    locked: LockedDetail,
    customer_id: CustomerId,
    clock: &impl Clock,
) -> Result<FraudOutcome, DomainError> {
    let LockedDetail(mut detail) = locked;
    let account_id = detail.pa_acct_id;
    let action = toggled(detail.pa_auth_fraud);
    let today = clock.now().date_naive();
    detail.pa_auth_fraud = action;
    detail.pa_fraud_rpt_date = fraud_report_date_text(today);
    let fraud = fraud_row(&detail, account_id, customer_id, action, today)
        .ok_or_else(|| DomainError::unexpected(UNEXPECTED_ERROR).with_field(CARD_NUM_FIELD))?;
    report_fraud(store, fraud).await?;
    store
        .update_detail(detail.clone())
        .await
        .map_err(|error| ims_failure(SYSTEM_ERROR_WHILE_FRAUD_TAGGING_ROLLBACK, &error))?;
    let message = if action == FraudFlag::Removed {
        AUTH_FRAUD_REMOVED
    } else {
        AUTH_MARKED_FRAUD
    };
    Ok(FraudOutcome { detail, message })
}

/// COPAUS1C POPULATE-AUTH-DETAILS — P-COPAUS1C-005
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L319-328]: `AUTHRSNO`,
/// the response reason as the detail view shows it.
///
/// `SEARCH ALL WS-DECLINE-REASON-TAB` looks `PA-AUTH-RESP-REASON` up in the
/// ten-entry table. A match gives the reason code, `-` and its `DECL-DESC`
/// cut to the 15 bytes of `AUTHRSNO(6:)`, for example `4100-INSUFFICNT FUND`;
/// no match gives `9999-ERROR`. The field is padded with spaces to its 20
/// bytes.
#[must_use]
pub fn decline_reason(detail: &PendingAuthDetail) -> Alnum<AUTH_REASON_LEN> {
    let reason = detail.pa_auth_resp_reason.as_bytes();
    let (code, text) = DECLINE_REASONS
        .iter()
        .copied()
        .find(|&(code, _)| code == reason)
        .unwrap_or(UNKNOWN_REASON);
    let mut bytes = code.to_vec();
    bytes.push(b'-');
    bytes.extend_from_slice(Alnum::<AUTH_REASON_TEXT_LEN>::move_from(text).as_bytes());
    Alnum::move_from_bytes(&bytes)
}

#[cfg(test)]
mod tests {
    use std::future::Future;

    use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

    use super::{
        CARD_NUM_FIELD, DetailStep, FraudOutcome, NO_MESSAGE, auth_time, decline_reason, detail,
        detail_for_update, edit_auth_id, fraud_row, next, timestamp_format, toggle_fraud, toggled,
        ws_auth_ts,
    };
    use crate::clock::FixedClock;
    use crate::entities::{
        AuthFraud, FraudFlag, MatchStatus, NewPendingAuthDetail, PA_ACCOUNT_STATUS_OCCURS,
        PendingAuthDetail, PendingAuthSummary,
    };
    use crate::error::DomainError;
    use crate::messages::{
        ALREADY_AT_THE_LAST_AUTHORIZATION, AUTH_FRAUD_REMOVED, AUTH_MARKED_FRAUD,
        SYSTEM_ERROR_DB2_CODE, SYSTEM_ERROR_WHILE_FRAUD_TAGGING_ROLLBACK,
        SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS1C,
        SYSTEM_ERROR_WHILE_READING_AUTH_SUMMARY_COPAUS1C, SYSTEM_ERROR_WHILE_READING_NEXT_AUTH,
        UNEXPECTED_ERROR, UPDT_ERROR_DB2_CODE, fraud_db2_error_message, ims_status_message,
    };
    use crate::numeric::{
        AccountId, Alnum, AuthCount, AuthId, CardNumber, CustomerId, Money, NumericCode,
    };
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    /// The account whose summary holds details 1 and 2.
    const ACCOUNT: u64 = 50;

    /// The account whose summary holds detail 3.
    const OTHER_ACCOUNT: u64 = 51;

    /// An account with no summary: [`orphan`] seeds a detail under it.
    const ORPHAN_ACCOUNT: u64 = 52;

    /// The customer `COPAUS0C` passes for [`ACCOUNT`].
    const CUSTOMER: u32 = 50;

    /// The card of every detail.
    const CARD: &str = "4859452612877065";

    /// `999999999 - 123456789`: the key of an authorization at 12:34:56.789.
    const TIME_9C_NEWER: i32 = 876_543_210;

    /// The key of an authorization 10 ms earlier, the next twin.
    const TIME_9C_OLDER: i32 = 876_543_220;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("an RFC 3339 instant")
            .with_timezone(&Utc)
    }

    /// The processing clock the CPVD tests pin.
    fn clock() -> FixedClock {
        FixedClock(at("2022-07-01T12:00:00Z"))
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("a valid date")
    }

    fn timestamp(day: NaiveDate, hour: u32, minute: u32, second: u32, micro: u32) -> NaiveDateTime {
        day.and_hms_micro_opt(hour, minute, second, micro)
            .expect("a valid time")
    }

    fn account_id(value: u64) -> AccountId {
        AccountId::new(value).expect("an account number of at most 11 digits")
    }

    fn customer_id() -> CustomerId {
        CustomerId::new(CUSTOMER).expect("a customer number of at most 9 digits")
    }

    /// An approved detail as `COPAUA0C` inserts it, fraud flag and report date
    /// blank, authorized on 2022-07-18 at the time `time_9c` complements.
    fn new_detail(account: u64, time_9c: i32) -> NewPendingAuthDetail {
        NewPendingAuthDetail {
            pa_acct_id: account_id(account),
            pa_auth_date_9c: 77_800,
            pa_auth_time_9c: time_9c,
            pa_auth_orig_date: Alnum::move_from("220718"),
            pa_auth_orig_time: Alnum::move_from("123456"),
            pa_card_num: CardNumber::move_from(CARD),
            pa_auth_type: Alnum::move_from("SALE"),
            pa_card_expiry_date: Alnum::move_from("0225"),
            pa_message_type: Alnum::move_from("0100"),
            pa_message_source: Alnum::move_from("POS"),
            pa_auth_id_code: Alnum::move_from("123456"),
            pa_auth_resp_code: Alnum::move_from("00"),
            pa_auth_resp_reason: Alnum::move_from("0000"),
            pa_processing_code: NumericCode::new(42).expect("a six-digit code"),
            pa_transaction_amt: Money::from_cents(12_345).expect("an amount within S9(10)V99"),
            pa_approved_amt: Money::from_cents(12_300).expect("an amount within S9(10)V99"),
            pa_merchant_catagory_code: Alnum::move_from("5411"),
            pa_acqr_country_code: Alnum::move_from("840"),
            pa_pos_entry_mode: NumericCode::new(5).expect("a two-digit code"),
            pa_merchant_id: Alnum::move_from("000000000000001"),
            pa_merchant_name: Alnum::move_from("CORNER GROCERY"),
            pa_merchant_city: Alnum::move_from("SEATTLE"),
            pa_merchant_state: Alnum::move_from("WA"),
            pa_merchant_zip: Alnum::move_from("98101"),
            pa_transaction_id: Alnum::move_from("TX0000000000001"),
            pa_match_status: MatchStatus::Pending,
            pa_auth_fraud: FraudFlag::Blank,
            pa_fraud_rpt_date: Alnum::spaces(),
        }
    }

    /// The `PAUTSUM0` summary of `account`.
    fn summary(account: u64) -> PendingAuthSummary {
        let cents = |value| Money::from_cents(value).expect("an amount within S9(09)V99");
        PendingAuthSummary {
            pa_acct_id: account_id(account),
            pa_cust_id: customer_id(),
            pa_auth_status: Alnum::move_from("A"),
            pa_account_status: [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS],
            pa_credit_limit: cents(2_020_000),
            pa_cash_limit: cents(1_020_000),
            pa_credit_balance: cents(24_645),
            pa_cash_balance: Money::ZERO,
            pa_approved_auth_cnt: AuthCount::new(3).expect("a count of three"),
            pa_declined_auth_cnt: AuthCount::ZERO,
            pa_approved_auth_amt: cents(24_645),
            pa_declined_auth_amt: Money::ZERO,
        }
    }

    /// Details 1 (newest) and 2 under the summary of [`ACCOUNT`], 3 under the
    /// summary of [`OTHER_ACCOUNT`].
    fn store() -> FakeStore {
        FakeStore::default()
            .with_summary(summary(ACCOUNT))
            .with_summary(summary(OTHER_ACCOUNT))
            .with_detail(new_detail(ACCOUNT, TIME_9C_NEWER))
            .with_detail(new_detail(ACCOUNT, TIME_9C_OLDER))
            .with_detail(new_detail(OTHER_ACCOUNT, TIME_9C_NEWER))
    }

    /// [`store`] with detail 4 under [`ORPHAN_ACCOUNT`], which has no summary.
    fn orphan() -> FakeStore {
        store().with_detail(new_detail(ORPHAN_ACCOUNT, TIME_9C_NEWER))
    }

    fn stored(store: &FakeStore, id: i64) -> PendingAuthDetail {
        store.details[&AuthId::from(id)].clone()
    }

    fn not_found() -> DomainError {
        DomainError::not_found(NO_MESSAGE).with_field(CARD_NUM_FIELD)
    }

    fn unexpected(message: impl Into<String>) -> DomainError {
        DomainError::unexpected(message).with_field(CARD_NUM_FIELD)
    }

    fn unavailable() -> PortError {
        PortError::Unavailable("connection reset".to_owned())
    }

    /// `AUTH_TS` of detail 1: 2022-07-18 12:34:56.789000.
    fn newer_auth_ts() -> NaiveDateTime {
        timestamp(date(2022, 7, 18), 12, 34, 56, 789_000)
    }

    /// PF5 on the detail `id`: the locked read, then the toggle on `clock`.
    fn mark(
        store: &mut FakeStore,
        id: i64,
        clock: &FixedClock,
    ) -> Result<FraudOutcome, DomainError> {
        block_on(async {
            let locked = detail_for_update(store, AuthId::from(id)).await?;
            toggle_fraud(store, locked, customer_id(), clock).await
        })
    }

    /// PF5 on detail 1 of [`ACCOUNT`] on `clock`.
    fn toggle(store: &mut FakeStore, clock: &FixedClock) -> Result<FraudOutcome, DomainError> {
        mark(store, 1, clock)
    }

    #[test]
    fn the_rule_futures_are_send() {
        // The adapters drive the rules on a multi-threaded runtime.
        fn require_send<F: Future + Send>(future: F) -> F {
            future
        }
        let mut store = store();

        let read =
            block_on(require_send(detail(&mut store, AuthId::from(1)))).expect("a readable detail");
        let step = block_on(require_send(next(&mut store, &read)));
        let locked = block_on(require_send(detail_for_update(&mut store, AuthId::from(1))))
            .expect("a lockable detail");
        let clock = clock();
        let toggle = require_send(toggle_fraud(&mut store, locked, customer_id(), &clock));

        assert!(step.is_ok());
        assert!(block_on(toggle).is_ok());
    }

    #[test]
    fn detail_returns_the_stored_authorization_read_under_its_summary() {
        // P-COPAUS1C-003, P-COPAUS1C-004: the summary GU, then the selected
        // detail, shown as stored and read without a lock.
        let mut store = store();
        let expected = stored(&store, 2);

        assert_eq!(block_on(detail(&mut store, AuthId::from(2))), Ok(expected));
        assert_eq!(store.calls, ["read_summary_of_detail", "read_detail"]);
    }

    #[test]
    fn read_auth_record_answers_in_the_order_of_its_two_evaluates() {
        // P-COPAUS1C-004, P-COPAUS1C-014, E-44: a GU that finds no summary
        // skips the GNP (V-COPAUS1C-004); any other GU failure leaves
        // AUTHS-NOT-EOF, so the GNP still runs and its own failure text
        // overwrites the summary text (V-COPAUS1C-006), else the summary text
        // stands (V-COPAUS1C-003); a GNP that finds nothing is not found
        // (V-COPAUS1C-005). The view read and the PF5 read answer alike, and
        // PF5 writes nothing on any of them (E-53).
        let summary_text =
            ims_status_message(SYSTEM_ERROR_WHILE_READING_AUTH_SUMMARY_COPAUS1C, "AO");
        let details_text =
            ims_status_message(SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS1C, "AO");
        let cases = [
            (Some(PortError::NotFound), None, not_found(), false),
            (
                Some(unavailable()),
                None,
                unexpected(summary_text.clone()),
                true,
            ),
            (
                Some(PortError::Contention),
                Some(PortError::NotFound),
                unexpected(summary_text),
                true,
            ),
            (
                Some(unavailable()),
                Some(unavailable()),
                unexpected(details_text.clone()),
                true,
            ),
            (None, Some(unavailable()), unexpected(details_text), true),
            (None, Some(PortError::NotFound), not_found(), true),
        ];
        for (summary_failure, detail_failure, expected, reads_detail) in cases {
            for (detail_read, pf5) in [("read_detail", false), ("read_detail_for_update", true)] {
                let mut store = store();
                let before = store.clone();
                if let Some(failure) = summary_failure.clone() {
                    store.fail_next("read_summary_of_detail", failure);
                }
                if let Some(failure) = detail_failure.clone() {
                    store.fail_next(detail_read, failure);
                }

                let outcome = if pf5 {
                    toggle(&mut store, &clock()).map(|outcome| outcome.detail)
                } else {
                    block_on(detail(&mut store, AuthId::from(1)))
                };

                let case = format!("{summary_failure:?} {detail_failure:?} {detail_read}");
                assert_eq!(outcome, Err(expected.clone()), "{case}");
                let calls: &[&str] = if reads_detail {
                    &["read_summary_of_detail", detail_read]
                } else {
                    &["read_summary_of_detail"]
                };
                assert_eq!(store.calls, calls, "{case}");
                assert_eq!(store.details, before.details, "{case}");
                assert_eq!(store.frauds, before.frauds, "{case}");
            }
        }
    }

    #[test]
    fn a_detail_outside_the_summary_read_is_not_found_under_it() {
        // P-COPAUS1C-003, V-COPAUS1C-005: the GNP is qualified under the
        // summary the GU reached, so a detail of another account than that
        // summary's is not reached, for the view read as for PF5.
        for (detail_read, pf5) in [("read_detail", false), ("read_detail_for_update", true)] {
            let mut store = store();
            store
                .summaries
                .insert(account_id(ACCOUNT), summary(OTHER_ACCOUNT));
            let before = store.details.clone();

            let outcome = if pf5 {
                toggle(&mut store, &clock()).map(|outcome| outcome.detail)
            } else {
                block_on(detail(&mut store, AuthId::from(1)))
            };

            assert_eq!(outcome, Err(not_found()), "{detail_read}");
            assert_eq!(
                store.calls,
                ["read_summary_of_detail", detail_read],
                "{detail_read}"
            );
            assert_eq!(store.details, before, "{detail_read}");
            assert!(store.frauds.is_empty(), "{detail_read}");
        }
    }

    #[test]
    fn an_identity_that_is_not_positive_is_invalid_before_any_read() {
        // V-COPAUS1C-001, E-49: no message, 400.
        for id in [0, -1, i64::MIN] {
            let mut store = store();

            let error = block_on(detail(&mut store, AuthId::from(id))).expect_err("a bad key");

            assert_eq!(
                error,
                DomainError::invalid(NO_MESSAGE).with_field(CARD_NUM_FIELD)
            );
            assert!(store.calls.is_empty(), "id {id}");
        }
    }

    #[test]
    fn the_key_edit_passes_a_positive_identity_unchanged_and_rejects_any_other() {
        // COPAUS1C PROCESS-ENTER-KEY, V-COPAUS1C-001, E-49: the store-free
        // edit the adapter runs before it opens the store, and the PF5 read
        // rejects the same keys before any read.
        let rejected = DomainError::invalid(NO_MESSAGE).with_field(CARD_NUM_FIELD);
        let cases = [
            (1, Ok(AuthId::from(1))),
            (i64::MAX, Ok(AuthId::from(i64::MAX))),
            (0, Err(rejected.clone())),
            (-5, Err(rejected.clone())),
            (i64::MIN, Err(rejected)),
        ];
        for (id, expected) in cases {
            assert_eq!(edit_auth_id(AuthId::from(id)), expected, "id {id}");
            if let Err(error) = expected {
                let mut store = store();
                let locked = block_on(detail_for_update(&mut store, AuthId::from(id)));
                assert_eq!(locked, Err(error), "id {id}");
                assert!(store.calls.is_empty(), "id {id}");
            }
        }
    }

    #[test]
    fn no_summary_is_not_found_without_a_message_and_reads_no_detail() {
        // V-COPAUS1C-004, E-49: the GU answers GE, AUTHS-EOF is set and the
        // GNP is not issued; 404. No detail has identity 99, and detail 4 has
        // no summary over it.
        for (mut store, id) in [(store(), 99), (orphan(), 4)] {
            assert_eq!(
                block_on(detail(&mut store, AuthId::from(id))),
                Err(not_found()),
                "id {id}"
            );
            assert_eq!(store.calls, ["read_summary_of_detail"], "id {id}");
        }
    }

    #[test]
    fn a_failed_read_is_the_auth_details_text_with_its_ims_status() {
        // V-COPAUS1C-006, E-44: AO for a database failure without IMS code.
        for (failure, status) in [
            (unavailable(), "AO"),
            (PortError::Contention, "AO"),
            (PortError::Restricted, "AO"),
            (PortError::Duplicate, "II"),
        ] {
            let mut store = store();
            store.fail_next("read_detail", failure);

            let error = block_on(detail(&mut store, AuthId::from(1))).expect_err("a failed read");

            let text = ims_status_message(SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS1C, status);
            assert_eq!(error, unexpected(text));
        }
    }

    #[test]
    fn pf8_steps_from_the_detail_read_with_one_twin_lookup() {
        // P-COPAUS1C-009, P-COPAUS1C-014: the unqualified GNP in twin order
        // under the detail's own summary, from the detail already read: one
        // lookup and no read of the detail or its summary again.
        let mut store = store();
        let older = stored(&store, 2);
        let current = stored(&store, 1);

        let step = block_on(next(&mut store, &current));

        assert_eq!(step, Ok(DetailStep::Next(Box::new(older))));
        assert_eq!(store.calls, ["adjacent_detail"]);
    }

    #[test]
    fn pf8_at_the_last_authorization_shows_the_last_authorization_text() {
        // V-COPAUS1C-012: GE or GB, 200 with the text and no error. Detail 2
        // is the last twin of its summary; detail 3 is the only twin of its
        // own, though detail 2 of another account has a greater key; a GE
        // from the store ends the twins as well.
        let message = ALREADY_AT_THE_LAST_AUTHORIZATION;
        for (id, failure) in [(2, None), (3, None), (1, Some(PortError::NotFound))] {
            let mut store = store();
            let current = stored(&store, id);
            if let Some(failure) = failure {
                store.fail_next("adjacent_detail", failure);
            }

            let step = block_on(next(&mut store, &current));

            assert_eq!(step, Ok(DetailStep::AtEnd { message }), "id {id}");
            assert_eq!(store.calls, ["adjacent_detail"], "id {id}");
        }
    }

    #[test]
    fn a_failed_next_read_is_the_next_auth_text() {
        // V-COPAUS1C-013, E-44.
        let mut store = store();
        let current = stored(&store, 1);
        store.fail_next("adjacent_detail", unavailable());

        let step = block_on(next(&mut store, &current));

        let text = ims_status_message(SYSTEM_ERROR_WHILE_READING_NEXT_AUTH, "AO");
        assert_eq!(step, Err(unexpected(text)));
    }

    #[test]
    fn pf5_on_an_unreported_authorization_reports_fraud_for_its_own_account() {
        // P-COPAUS1C-006 to P-COPAUS1C-008, P-COPAUS2C-001 to P-COPAUS2C-003,
        // P-COPAUS2C-006: flag F, report date mm/dd/yy, one AUTHFRDS row whose
        // ACCT_ID is the detail's account. The flag comes from the one locked
        // read: never the unlocked read, and no read again before the writes.
        for (id, account) in [(1, ACCOUNT), (3, OTHER_ACCOUNT)] {
            let mut store = store();
            let mut expected = stored(&store, id);
            expected.pa_auth_fraud = FraudFlag::Confirmed;
            expected.pa_fraud_rpt_date = Alnum::move_from("07/01/22");
            let mut untouched = store.details.clone();
            untouched.remove(&AuthId::from(id));

            let outcome = mark(&mut store, id, &clock());

            let message = AUTH_MARKED_FRAUD;
            let detail = expected.clone();
            assert_eq!(outcome, Ok(FraudOutcome { detail, message }), "id {id}");
            assert_eq!(stored(&store, id), expected, "id {id}");
            assert_eq!(
                store.calls,
                [
                    "read_summary_of_detail",
                    "read_detail_for_update",
                    "insert_fraud",
                    "update_detail"
                ],
                "id {id}"
            );
            let row = fraud_row(
                &expected,
                account_id(account),
                customer_id(),
                FraudFlag::Confirmed,
                date(2022, 7, 1),
            );
            let key = (CardNumber::move_from(CARD), newer_auth_ts());
            assert_eq!(store.frauds.len(), 1, "id {id}");
            assert_eq!(store.frauds.get(&key), row.as_ref(), "id {id}");
            let mut others = store.details.clone();
            others.remove(&AuthId::from(id));
            assert_eq!(others, untouched, "id {id}: the other details untouched");
        }
    }

    #[test]
    fn pf5_toggles_between_reported_and_removed_on_the_one_report() {
        // P-COPAUS1C-006, P-COPAUS1C-008, P-COPAUS2C-004: the second report of
        // the same authorization meets -803 and updates flag and date.
        let mut store = store();
        let key = (CardNumber::move_from(CARD), newer_auth_ts());
        toggle(&mut store, &clock()).expect("the first mark");

        let later = FixedClock(at("2022-07-05T08:00:00Z"));
        let removed = toggle(&mut store, &later).expect("the removal");

        assert_eq!(removed.message, AUTH_FRAUD_REMOVED);
        assert_eq!(removed.detail.pa_auth_fraud, FraudFlag::Removed);
        assert_eq!(
            removed.detail.pa_fraud_rpt_date,
            Alnum::move_from("07/05/22")
        );
        assert_eq!(stored(&store, 1), removed.detail);
        let report = &store.frauds[&key];
        assert_eq!(store.frauds.len(), 1);
        assert_eq!(report.auth_fraud, Some(Alnum::move_from("R")));
        assert_eq!(report.fraud_rpt_date, Some(date(2022, 7, 5)));
        assert_eq!(
            store.calls[4..],
            [
                "read_summary_of_detail",
                "read_detail_for_update",
                "insert_fraud",
                "update_fraud",
                "update_detail"
            ]
        );

        let again = toggle(&mut store, &later).expect("the second mark");

        assert_eq!(again.message, AUTH_MARKED_FRAUD);
        assert_eq!(again.detail.pa_auth_fraud, FraudFlag::Confirmed);
        assert_eq!(store.frauds[&key].auth_fraud, Some(Alnum::move_from("F")));
    }

    #[test]
    fn only_a_confirmed_flag_is_removed() {
        // P-COPAUS1C-006: 'F' becomes 'R'; every other value becomes 'F'.
        for (flag, expected) in [
            (FraudFlag::Confirmed, FraudFlag::Removed),
            (FraudFlag::Removed, FraudFlag::Confirmed),
            (FraudFlag::Blank, FraudFlag::Confirmed),
            (FraudFlag::Other(b'X'), FraudFlag::Confirmed),
        ] {
            assert_eq!(toggled(flag), expected, "{flag:?}");
        }
    }

    #[test]
    fn a_failed_fraud_insert_stops_before_the_detail_replace() {
        // V-COPAUS1C-009, P-COPAUS2C-005, E-30: the Db2 text where the port
        // pins SQLCODE and SQLSTATE, the unexpected-error text otherwise.
        let pinned = fraud_db2_error_message(SYSTEM_ERROR_DB2_CODE, 100, 2_000);
        for (failure, message) in [
            (PortError::NotFound, pinned),
            (unavailable(), UNEXPECTED_ERROR.to_owned()),
            (PortError::Contention, UNEXPECTED_ERROR.to_owned()),
        ] {
            let mut store = store();
            let before = stored(&store, 1);
            store.fail_next("insert_fraud", failure);

            assert_eq!(toggle(&mut store, &clock()), Err(unexpected(message)));
            assert_eq!(
                store.calls,
                [
                    "read_summary_of_detail",
                    "read_detail_for_update",
                    "insert_fraud"
                ]
            );
            assert_eq!(stored(&store, 1), before);
            assert!(store.frauds.is_empty());
        }
    }

    #[test]
    fn a_failed_fraud_update_stops_before_the_detail_replace() {
        // V-COPAUS1C-010, P-COPAUS2C-005, E-30: the insert met -803, the
        // update failed.
        let pinned = fraud_db2_error_message(UPDT_ERROR_DB2_CODE, 100, 2_000);
        for (failure, message) in [
            (PortError::NotFound, pinned),
            (unavailable(), UNEXPECTED_ERROR.to_owned()),
        ] {
            let reported = date(2022, 6, 30);
            let detail = stored(&store(), 1);
            let row = fraud_row(
                &detail,
                account_id(ACCOUNT),
                customer_id(),
                FraudFlag::Confirmed,
                reported,
            )
            .expect("a valid AUTH_TS");
            let mut store = store().with_fraud(row.clone());
            store.fail_next("update_fraud", failure);

            assert_eq!(toggle(&mut store, &clock()), Err(unexpected(message)));
            assert_eq!(
                store.calls,
                [
                    "read_summary_of_detail",
                    "read_detail_for_update",
                    "insert_fraud",
                    "update_fraud"
                ]
            );
            assert_eq!(stored(&store, 1), detail);
            assert_eq!(store.frauds.values().collect::<Vec<_>>(), [&row]);
        }
    }

    #[test]
    fn a_failed_detail_replace_is_the_fraud_tagging_rollback_text() {
        // V-COPAUS1C-011, E-44: the fraud row was written first; the caller
        // rolls both back (P-COPAUS1C-015).
        let mut store = store();
        let before = stored(&store, 1);
        store.fail_next("update_detail", unavailable());

        let outcome = toggle(&mut store, &clock());

        let text = ims_status_message(SYSTEM_ERROR_WHILE_FRAUD_TAGGING_ROLLBACK, "AO");
        assert_eq!(outcome, Err(unexpected(text)));
        assert_eq!(
            store.calls,
            [
                "read_summary_of_detail",
                "read_detail_for_update",
                "insert_fraud",
                "update_detail"
            ]
        );
        assert_eq!(stored(&store, 1), before);
    }

    #[test]
    fn pf5_on_an_authorization_that_cannot_be_read_changes_nothing() {
        // V-COPAUS1C-007, E-53: the read's own outcome, and no write. Detail
        // 4 has no summary; a lock that cannot be taken fails the detail read
        // with status AO (V-COPAUS1C-006, E-44).
        let failed = ims_status_message(SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS1C, "AO");
        for (id, failure, expected) in [
            (99, None, not_found()),
            (4, None, not_found()),
            (
                0,
                None,
                DomainError::invalid(NO_MESSAGE).with_field(CARD_NUM_FIELD),
            ),
            (1, Some(unavailable()), unexpected(failed.clone())),
            (1, Some(PortError::Contention), unexpected(failed)),
        ] {
            let mut store = orphan();
            let before = store.details.clone();
            if let Some(failure) = failure {
                store.fail_next("read_detail_for_update", failure);
            }

            let outcome = mark(&mut store, id, &clock());

            assert_eq!(outcome, Err(expected), "id {id}");
            assert!(!store.calls.contains(&"insert_fraud"), "id {id}");
            assert!(!store.calls.contains(&"update_detail"), "id {id}");
            assert_eq!(store.details, before, "id {id}");
            assert!(store.frauds.is_empty(), "id {id}");
        }
    }

    #[test]
    fn auth_ts_is_built_from_the_original_date_and_the_complemented_time() {
        // P-COPAUS2C-001: WS-AUTH-TS, then TIMESTAMP_FORMAT with the century
        // of the current year.
        let today = date(2022, 7, 1);
        let cases = [
            (
                "220718",
                TIME_9C_NEWER,
                today,
                "22-07-18 12.34.56789000",
                Some(newer_auth_ts()),
            ),
            (
                "220701",
                879_999_999,
                today,
                "22-07-01 12.00.00000000",
                Some(timestamp(today, 12, 0, 0, 0)),
            ),
            (
                "220718",
                999_999_999,
                today,
                "22-07-18 00.00.00000000",
                Some(timestamp(date(2022, 7, 18), 0, 0, 0, 0)),
            ),
            // The unsigned 9(09) receiver keeps the magnitude of 999999999 - 1000000000.
            (
                "220718",
                1_000_000_000,
                today,
                "22-07-18 00.00.00001000",
                Some(timestamp(date(2022, 7, 18), 0, 0, 0, 1_000)),
            ),
            (
                "220718",
                TIME_9C_NEWER,
                date(2105, 1, 1),
                "22-07-18 12.34.56789000",
                Some(timestamp(date(2122, 7, 18), 12, 34, 56, 789_000)),
            ),
            (
                "221318",
                TIME_9C_NEWER,
                today,
                "22-13-18 12.34.56789000",
                None,
            ),
            (
                "22 718",
                TIME_9C_NEWER,
                today,
                "22- 7-18 12.34.56789000",
                None,
            ),
            ("220718", 0, today, "22-07-18 99.99.99999000", None),
        ];
        for (orig_date, time_9c, current_date, text, expected) in cases {
            let detail = PendingAuthDetail {
                pa_auth_orig_date: Alnum::move_from(orig_date),
                pa_auth_time_9c: time_9c,
                ..stored(&store(), 1)
            };

            let ts = ws_auth_ts(&detail);

            assert_eq!(ts, Alnum::move_from(text), "{orig_date} {time_9c}");
            assert_eq!(timestamp_format(&ts, current_date), expected, "{text}");
        }
        assert_eq!(auth_time(TIME_9C_NEWER), 123_456_789);
    }

    #[test]
    fn fraud_row_moves_every_detail_field() {
        // P-COPAUS2C-002: every column holds a value; AUTH_FRAUD is the action,
        // FRAUD_RPT_DATE the CURRENT DATE, ACCT_ID and CUST_ID as passed.
        let detail = stored(&store(), 1);
        let report_date = date(2022, 7, 1);

        let row = fraud_row(
            &detail,
            account_id(ACCOUNT),
            customer_id(),
            FraudFlag::Removed,
            report_date,
        );

        let expected = AuthFraud {
            card_num: CardNumber::move_from(CARD),
            auth_ts: newer_auth_ts(),
            auth_type: Some(Alnum::move_from("SALE")),
            card_expiry_date: Some(Alnum::move_from("0225")),
            message_type: Some(Alnum::move_from("0100")),
            message_source: Some(Alnum::move_from("POS")),
            auth_id_code: Some(Alnum::move_from("123456")),
            auth_resp_code: Some(Alnum::move_from("00")),
            auth_resp_reason: Some(Alnum::move_from("0000")),
            processing_code: Some(Alnum::move_from("000042")),
            transaction_amt: Some(detail.pa_transaction_amt),
            approved_amt: Some(detail.pa_approved_amt),
            merchant_catagory_code: Some(Alnum::move_from("5411")),
            acqr_country_code: Some(Alnum::move_from("840")),
            pos_entry_mode: Some(detail.pa_pos_entry_mode),
            merchant_id: Some(Alnum::move_from("000000000000001")),
            merchant_name: Some(Alnum::move_from("CORNER GROCERY")),
            merchant_city: Some(Alnum::move_from("SEATTLE")),
            merchant_state: Some(Alnum::move_from("WA")),
            merchant_zip: Some(Alnum::move_from("98101")),
            transaction_id: Some(Alnum::move_from("TX0000000000001")),
            match_status: Some(Alnum::move_from("P")),
            auth_fraud: Some(Alnum::move_from("R")),
            fraud_rpt_date: Some(report_date),
            acct_id: Some(account_id(ACCOUNT)),
            cust_id: Some(customer_id()),
        };
        assert_eq!(row, Some(expected));
    }

    #[test]
    fn pf5_on_a_detail_without_a_valid_auth_ts_writes_nothing() {
        // P-COPAUS2C-001: TIMESTAMP_FORMAT rejects the text with a SQLCODE the
        // audit does not record, so the unexpected-error text answers.
        let mut store = FakeStore::default()
            .with_summary(summary(ACCOUNT))
            .with_detail(NewPendingAuthDetail {
                pa_auth_orig_date: Alnum::spaces(),
                ..new_detail(ACCOUNT, TIME_9C_NEWER)
            });
        let before = stored(&store, 1);

        assert_eq!(
            toggle(&mut store, &clock()),
            Err(unexpected(UNEXPECTED_ERROR))
        );
        assert_eq!(
            store.calls,
            ["read_summary_of_detail", "read_detail_for_update"]
        );
        assert_eq!(stored(&store, 1), before);
    }

    #[test]
    fn decline_reason_follows_the_reason_table() {
        // P-COPAUS1C-005: SEARCH ALL WS-DECLINE-REASON-TAB, AT END 9999-ERROR.
        for (reason, expected) in [
            ("0000", "0000-APPROVED"),
            ("3100", "3100-INVALID CARD"),
            ("4100", "4100-INSUFFICNT FUND"),
            ("4200", "4200-CARD NOT ACTIVE"),
            ("4300", "4300-ACCOUNT CLOSED"),
            ("4400", "4400-EXCED DAILY LMT"),
            ("5100", "5100-CARD FRAUD"),
            ("5200", "5200-MERCHANT FRAUD"),
            ("5300", "5300-LOST CARD"),
            ("9000", "9000-UNKNOWN"),
            ("1234", "9999-ERROR"),
            ("    ", "9999-ERROR"),
        ] {
            let detail = PendingAuthDetail {
                pa_auth_resp_reason: Alnum::move_from(reason),
                ..stored(&store(), 1)
            };

            assert_eq!(
                decline_reason(&detail),
                Alnum::move_from(expected),
                "{reason:?}"
            );
        }
    }
}
