//! CP00 card authorization: the rules of `COPAUA0C`
//! [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl], the trigger-started
//! CICS program that takes an authorization request from MQ, decides it,
//! replies and records it in the pending-authorization database
//! (P-COPAUA0C-005).
//!
//! The rules are pure. The consumer `crates/messaging/src/flows/cp00.rs` owns
//! the broker and the database: it publishes the reply, waits for its confirm,
//! acknowledges the request and only then writes, in a database transaction of
//! its own (AAP 0.3.3). The steps it sequences are:
//!
//! 1. [`convert_request`], the `MQGMO-CONVERT` get of `3100-READ-REQUEST-MQ`;
//!    [`parse_request`], the `UNSTRING` of `2100-EXTRACT-REQUEST-MSG`,
//!    [`storable`], which holds back receivers `PostgreSQL` text cannot
//!    store, and [`parse_amount`], its `FUNCTION NUMVAL` of the amount;
//! 2. [`gather`], the reads of `5100-READ-XREF-RECORD` to
//!    `5500-READ-AUTH-SUMMRY`, with the `9500-LOG-ERROR` records they raise;
//! 3. [`decide`] and [`reply_body`], `6000-MAKE-DECISION` and its reply text;
//! 4. [`write_plan`], what `8400-UPDATE-SUMMARY` and `8500-INSERT-AUTH` write,
//!    [`locked_summary_write`], the summary write derived again on the summary
//!    the write unit of work holds under its lock, and [`write_failure_entry`]
//!    for a write that fails.
//!
//! `5600-READ-PROFILE-DATA` is `CONTINUE` only and has no counterpart
//! (P-COPAUA0C-034). PSB scheduling, the per-task queue open and close and the
//! get loop have no counterpart in the long-running consumer (E-22).
//!
//! # Single delivery
//!
//! The consumer handles one request per task, so every request is the first
//! message of its task (E-13, P-COPAUA0C-027) and working storage without a
//! `VALUE` clause still holds nothing a statement set:
//!
//! - a receiver the `UNSTRING` does not reach, the codes a warning record does
//!   not move, and the detail area's `PA-CARD-NUM` that the M003, I002, M004
//!   and I003 records take as their event key before `8500-INSERT-AUTH` sets
//!   it, are never-set bytes, written as spaces (E-25);
//! - the summary flag `WS-PAUT-SMRY-SEG-FLG` is set only for a card the
//!   cross-reference holds, which [`decide`] describes for an unknown card
//!   (E-41, P-COPAUA0C-028).
//!
//! # Halt cases
//!
//! An input whose COBOL result the audit cannot pin gets no guessed result.
//! The step returns [`Unproven`] naming the escalation that holds the case at
//! `halt`; the consumer logs it and, for a request, sends no reply, and for a
//! write plan, writes nothing.

use std::iter::repeat;

use chrono::{DateTime, Utc};

use crate::clock;
use crate::entities::{FraudFlag, MatchStatus, PA_ACCOUNT_STATUS_OCCURS};
use crate::format;
use crate::messages;
use crate::numeric::{Alnum, AuthCount, NumericCode};

/// The records, keys, amounts and ports [`gather`], [`decide`] and
/// [`write_plan`] work with, and the ones the messaging adapter's store
/// implements those ports with, re-exported from [`crate::entities`],
/// [`crate::numeric`] and [`crate::ports`] for the CP00 flow and the store it
/// shares with CDRA.
pub use crate::entities::{
    Account, AuthFraud, CardXref, Customer, NewPendingAuthDetail, PendingAuthDetail,
    PendingAuthSummary,
};
pub use crate::numeric::{AccountId, AuthId, CardNumber, CustomerId, Money};
pub use crate::ports::{
    AccountReader, AuthorizationStore, Browse, CustomerReader, Direction, PortError, PortResult,
    XrefReader,
};

/// `WS-CICS-TRANID PIC X(04) VALUE 'CP00'`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L34], which
/// `9500-LOG-ERROR` moves to `ERR-APPLICATION` — P-COPAUA0C-015.
pub const ERR_APPLICATION: &str = "CP00";

/// `WS-PGM-AUTH PIC X(08) VALUE 'COPAUA0C'`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L33], which
/// `9500-LOG-ERROR` moves to `ERR-PROGRAM` — P-COPAUA0C-015.
pub const ERR_PROGRAM: &str = "COPAUA0C";

/// `W01-GET-BUFFER PIC X(500)`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L103], the buffer
/// `3100-READ-REQUEST-MQ` gets a request into — P-COPAUA0C-002.
pub const REQUEST_BUFFER_LEN: usize = 500;

/// The MQ completion code `MQCC_WARNING`, which `MQGET` returns for a message
/// longer than its buffer.
pub const MQCC_WARNING: i64 = 1;

/// The MQ reason code `MQRC_TRUNCATED_MSG_FAILED`, which `MQGET` returns for a
/// message longer than its buffer when the get options lack
/// `MQGMO-ACCEPT-TRUNCATED-MSG`, as those of `3100-READ-REQUEST-MQ` do
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L389-391].
pub const MQRC_TRUNCATED_MSG_FAILED: i64 = 2080;

/// The number of `UNSTRING` receivers
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L356-373].
const RECEIVERS: usize = 18;

/// The `UNSTRING` delimiter.
const DELIMITER: u8 = b',';

/// The 0-based position of `PA-RQ-PROCESSING-CODE` among the receivers.
const PROCESSING_CODE_RECEIVER: usize = 7;

/// The 0-based position of `WS-TRANSACTION-AMT-AN` among the receivers.
const AMOUNT_RECEIVER: usize = 8;

/// The 0-based position of `PA-RQ-POS-ENTRY-MODE` among the receivers.
const POS_ENTRY_MODE_RECEIVER: usize = 11;

/// The digits of `WS-CODE-DISPLAY PIC 9(9)`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L61], through which every
/// numeric code reaches `ERR-CODE-1` and `ERR-CODE-2`.
const CODE_DISPLAY_DIGITS: usize = 9;

/// `PA-RL-AUTH-RESP-CODE` of an approval
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L693] — P-COPAUA0C-010.
const RESP_APPROVED: [u8; 2] = *b"00";

/// `PA-RL-AUTH-RESP-CODE` of a decline
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L688] — P-COPAUA0C-010.
const RESP_DECLINED: [u8; 2] = *b"05";

/// `PA-RL-AUTH-RESP-REASON` before any decline reason
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L698] — P-COPAUA0C-010.
const REASON_NONE: [u8; 4] = *b"0000";

/// The reason of a card, account or customer not found
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L701-704].
const REASON_NOT_FOUND: [u8; 4] = *b"3100";

/// The reason of insufficient funds
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L705-706].
const REASON_INSUFFICIENT_FUND: [u8; 4] = *b"4100";

/// `WHEN OTHER` of the reason `EVALUATE`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L715-716]. Every decline
/// sets a not-found or the insufficient-funds condition first, so no request
/// reaches it; `4200` to `5200` are unreachable too, because no statement sets
/// their conditions (P-COPAUA0C-010).
const REASON_OTHER: [u8; 4] = *b"9000";

/// The CICS `RESP` and `RESP2` of `NOTFND` on a `READ` (E-44).
const CICS_NOTFND: (u64, u64) = (13, 80);

/// The CICS `RESP` and `RESP2` the E-44 proposal gives a store failure with
/// no CICS counterpart: `IOERR` and 0.
const CICS_FAILURE: (u64, u64) = (17, 0);

/// The IMS status of a segment not found.
const IMS_NOT_FOUND: &str = "GE";

/// The IMS status of a duplicate segment.
const IMS_DUPLICATE: &str = "II";

/// The IMS status the E-44 proposal gives a store failure with no IMS
/// counterpart.
const IMS_FAILURE: &str = "AO";

/// Never-set bytes, written as spaces (E-25): a receiver or an account record
/// no statement filled.
const NEVER_SET: Unproven = Unproven {
    escalation_id: "E-25",
};

/// `FUNCTION NUMVAL` of text it does not accept (E-28).
const NUMVAL_MALFORMED: Unproven = Unproven {
    escalation_id: "E-28",
};

/// `FUNCTION NUMVAL` rounding at a midpoint, or a value wider than the
/// receiver (E-40).
const NUMVAL_UNPINNED: Unproven = Unproven {
    escalation_id: "E-40",
};

/// The declined-amount summary update, which adds a detail field before
/// `8500-INSERT-AUTH` sets it (E-41).
const DECLINED_UPDATE: Unproven = Unproven {
    escalation_id: "E-41",
};

/// The decision for a card the cross-reference lacks, which tests a flag no
/// statement set on the first message of a task (E-41).
const UNKNOWN_CARD_DECISION: Unproven = Unproven {
    escalation_id: "E-41",
};

/// A numeric request field holding non-digits, moved into the detail (E-47).
const NON_DIGITS: Unproven = Unproven {
    escalation_id: "E-47",
};

/// An approved count past the four digits of `S9(04) COMP` (E-78).
const COUNT_PAST_9999: Unproven = Unproven {
    escalation_id: "E-78",
};

/// Request text whose conversion into the program's single-byte code page the
/// repository does not pin: a body that is not UTF-8, or a character outside
/// ISO 8859-1 (E-80).
const UNCONVERTED_TEXT: Unproven = Unproven {
    escalation_id: "E-80",
};

/// A `LOW-VALUE` byte in a request receiver the detail stores as text, which
/// `PostgreSQL` text cannot hold (E-81, E-07).
const UNSTORABLE_TEXT: Unproven = Unproven {
    escalation_id: "E-81",
};

/// The `LOW-VALUE` byte X'00', the one byte `PostgreSQL` text cannot hold.
const LOW_VALUE: u8 = 0x00;

/// A request whose COBOL result the audit cannot pin: the escalation that holds
/// its input class at `halt` (AAP 0.9.6). The consumer logs it with the
/// escalation and writes, or replies, nothing for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("the COPAUA0C result for this input is unproven (escalation {escalation_id} is at halt)")]
pub struct Unproven {
    /// The `E-<nn>` of `docs/parity/escalations.md` that records the case.
    pub escalation_id: &'static str,
}

/// The request receivers after `2100-EXTRACT-REQUEST-MSG`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L354-374]: the
/// `CCPAURQY` fields [app/app-authorization-ims-db2-mq/cpy/CCPAURQY.cpy:L19-36]
/// the `UNSTRING` fills, and `WS-TRANSACTION-AMT-AN` in place of
/// `PA-RQ-TRANSACTION-AMT`, which [`parse_amount`] computes from it.
///
/// Each field holds its receiver's bytes. The two numeric receivers
/// `PA-RQ-PROCESSING-CODE 9(06)` and `PA-RQ-POS-ENTRY-MODE 9(02)` are
/// `Alnum<6>` and `Alnum<2>`, as P-CCPAURQY-008 and P-CCPAURQY-012 type them,
/// because the request text may put a non-digit in them, whose COBOL result is
/// unpinned (E-47). [`write_plan`] reads them as the `NumericCode<6>` of
/// `PA-PROCESSING-CODE` and the `NumericCode<2>` of `PA-POS-ENTRY-MODE` where
/// the detail stores them, and reports [`Unproven`] when one holds a non-digit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRequest {
    /// `PA-RQ-AUTH-DATE PIC X(06)` (P-CCPAURQY-001): the request date, `YYMMDD`.
    pub pa_rq_auth_date: Alnum<6>,
    /// `PA-RQ-AUTH-TIME PIC X(06)` (P-CCPAURQY-002), which is also the reply's
    /// authorization ID code [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L662].
    pub pa_rq_auth_time: Alnum<6>,
    /// `PA-RQ-CARD-NUM PIC X(16)` (P-CCPAURQY-003), the cross-reference key.
    pub pa_rq_card_num: CardNumber,
    /// `PA-RQ-AUTH-TYPE PIC X(04)` (P-CCPAURQY-004).
    pub pa_rq_auth_type: Alnum<4>,
    /// `PA-RQ-CARD-EXPIRY-DATE PIC X(04)` (P-CCPAURQY-005).
    pub pa_rq_card_expiry_date: Alnum<4>,
    /// `PA-RQ-MESSAGE-TYPE PIC X(06)` (P-CCPAURQY-006).
    pub pa_rq_message_type: Alnum<6>,
    /// `PA-RQ-MESSAGE-SOURCE PIC X(06)` (P-CCPAURQY-007).
    pub pa_rq_message_source: Alnum<6>,
    /// `PA-RQ-PROCESSING-CODE PIC 9(06)` (P-CCPAURQY-008): the receiver's six
    /// bytes after the numeric move of its field.
    pub pa_rq_processing_code: Alnum<6>,
    /// `WS-TRANSACTION-AMT-AN PIC X(13)`
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L63]: the amount text
    /// `FUNCTION NUMVAL` reads.
    pub amount_text: Alnum<13>,
    /// `PA-RQ-MERCHANT-CATAGORY-CODE PIC X(04)` (P-CCPAURQY-010), spelled as
    /// the copybook spells it.
    pub pa_rq_merchant_catagory_code: Alnum<4>,
    /// `PA-RQ-ACQR-COUNTRY-CODE PIC X(03)` (P-CCPAURQY-011).
    pub pa_rq_acqr_country_code: Alnum<3>,
    /// `PA-RQ-POS-ENTRY-MODE PIC 9(02)` (P-CCPAURQY-012): the receiver's two
    /// bytes after the numeric move of its field.
    pub pa_rq_pos_entry_mode: Alnum<2>,
    /// `PA-RQ-MERCHANT-ID PIC X(15)` (P-CCPAURQY-013).
    pub pa_rq_merchant_id: Alnum<15>,
    /// `PA-RQ-MERCHANT-NAME PIC X(22)` (P-CCPAURQY-014).
    pub pa_rq_merchant_name: Alnum<22>,
    /// `PA-RQ-MERCHANT-CITY PIC X(13)` (P-CCPAURQY-015).
    pub pa_rq_merchant_city: Alnum<13>,
    /// `PA-RQ-MERCHANT-STATE PIC X(02)` (P-CCPAURQY-016).
    pub pa_rq_merchant_state: Alnum<2>,
    /// `PA-RQ-MERCHANT-ZIP PIC X(09)` (P-CCPAURQY-017).
    pub pa_rq_merchant_zip: Alnum<9>,
    /// `PA-RQ-TRANSACTION-ID PIC X(15)` (P-CCPAURQY-018), echoed in the reply.
    pub pa_rq_transaction_id: Alnum<15>,
    /// How many of the 18 receivers, counted in `UNSTRING` order, received a
    /// field. A receiver beyond them keeps the value it had before, which on
    /// the first message of a task no statement set: it holds spaces here
    /// (E-25), and the steps that need it as a number report [`Unproven`].
    pub fields_received: usize,
}

impl AuthRequest {
    /// Whether the `UNSTRING` reached the receiver at 0-based `position`.
    fn received(&self, position: usize) -> bool {
        position < self.fields_received
    }
}

/// The outcome of one keyed read, as the COBOL flags and `RESP` record it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup<T> {
    /// `DFHRESP(NORMAL)`, or IMS `STATUS-OK`: the record read.
    Found(T),
    /// `DFHRESP(NOTFND)` or IMS `GE`; also a read [`gather`] does not issue
    /// because an earlier read ended the lookups.
    NotFound,
    /// Any other response: the task ends with a critical error record.
    Failed,
}

/// What `5100-READ-XREF-RECORD` to `5500-READ-AUTH-SUMMRY` found for a
/// request, and the error records they raised, in order — P-COPAUA0C-005 to
/// P-COPAUA0C-008.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthContext {
    /// `CCXREF` by card number: `CARD-FOUND-XREF` when found.
    pub xref: Lookup<CardXref>,
    /// `ACCTDAT` by the cross-reference's account: `FOUND-ACCT-IN-MSTR` when
    /// found. An unknown card leaves it [`Lookup::NotFound`], as `5100` sets
    /// `NFOUND-ACCT-IN-MSTR` [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L492].
    pub account: Lookup<Account>,
    /// `CUSTDAT` by the cross-reference's customer: `FOUND-CUST-IN-MSTR` when
    /// found.
    pub customer: Lookup<Customer>,
    /// `PAUTSUM0` by the cross-reference's account: `FOUND-PAUT-SMRY-SEG` when
    /// found.
    pub summary: Lookup<PendingAuthSummary>,
    /// The `9500-LOG-ERROR` records of the reads, in the order raised.
    pub log: Vec<ErrorLogEntry>,
}

impl AuthContext {
    /// The context of a cross-reference outcome before any other read.
    fn with_xref(xref: Lookup<CardXref>) -> Self {
        Self {
            xref,
            account: Lookup::NotFound,
            customer: Lookup::NotFound,
            summary: Lookup::NotFound,
            log: Vec::new(),
        }
    }

    /// The first critical record of the reads. `9500-LOG-ERROR` ends the task
    /// on a critical record through `9990-END-ROUTINE`
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L1008-1023], so a
    /// context holding one gets no decision, no reply and no write: the
    /// consumer rejects the request (P-COPAUA0C-006 to P-COPAUA0C-008).
    #[must_use]
    pub fn critical_entry(&self) -> Option<&ErrorLogEntry> {
        self.log
            .iter()
            .find(|entry| entry.level == ErrorLevel::Critical)
    }
}

/// `ERR-LEVEL PIC X(01)` (P-CCPAUERY-006) as `COPAUA0C` sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorLevel {
    /// 88 `ERR-WARNING VALUE 'W'`: processing continues.
    Warning,
    /// 88 `ERR-CRITICAL VALUE 'C'`: `9500-LOG-ERROR` ends the task.
    Critical,
}

impl ErrorLevel {
    /// The byte `ERR-LEVEL` holds.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Warning => b'W',
            Self::Critical => b'C',
        }
    }
}

/// `ERR-SUBSYSTEM PIC X(01)` (P-CCPAUERY-007) as `COPAUA0C` sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorSource {
    /// 88 `ERR-APP VALUE 'A'`.
    App,
    /// 88 `ERR-CICS VALUE 'C'`.
    Cics,
    /// 88 `ERR-IMS VALUE 'I'`.
    Ims,
    /// 88 `ERR-MQ VALUE 'M'`.
    Mq,
}

impl ErrorSource {
    /// The byte `ERR-SUBSYSTEM` holds.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::App => b'A',
            Self::Cics => b'C',
            Self::Ims => b'I',
            Self::Mq => b'M',
        }
    }
}

/// One `ERROR-LOG-RECORD` (`CCPAUERY`
/// [app/app-authorization-ims-db2-mq/cpy/CCPAUERY.cpy:L19-40]) as the
/// paragraph that raises it fills it, before `9500-LOG-ERROR` stamps it —
/// P-COPAUA0C-015.
///
/// `9500-LOG-ERROR` adds `ERR-DATE` (`clock::yymmdd`), `ERR-TIME`
/// (`clock::hhmmss_compact`), [`ERR_APPLICATION`] and [`ERR_PROGRAM`] when it
/// writes the record, which the replacement logs as one JSON line instead of
/// the TD queue `CSSL`. A field the paragraph does not move holds spaces: on
/// the first message of a task no earlier record set it (E-25).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorLogEntry {
    /// `ERR-LOCATION PIC X(04)` (P-CCPAUERY-005), for example `A001`.
    pub location: &'static str,
    /// `ERR-LEVEL PIC X(01)` (P-CCPAUERY-006).
    pub level: ErrorLevel,
    /// `ERR-SUBSYSTEM PIC X(01)` (P-CCPAUERY-007).
    pub source: ErrorSource,
    /// `ERR-CODE-1 PIC X(09)` (P-CCPAUERY-008): a CICS `RESP` or MQ completion
    /// code as nine digits, or an IMS status.
    pub code_1: Alnum<9>,
    /// `ERR-CODE-2 PIC X(09)` (P-CCPAUERY-009): a CICS `RESP2` or MQ reason
    /// code as nine digits.
    pub code_2: Alnum<9>,
    /// `ERR-MESSAGE PIC X(50)` (P-CCPAUERY-010): the literal the paragraph
    /// moves, from [`crate::messages`].
    pub message: &'static str,
    /// `ERR-EVENT-KEY PIC X(20)` (P-CCPAUERY-011): the card, account or
    /// customer the record concerns. A001, C001 and I004 carry the full card
    /// number, as the COBOL writes it to `CSSL`.
    pub event_key: Alnum<20>,
}

/// The pending-authorization write that failed, for [`write_failure_entry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WriteStep {
    /// `8400-UPDATE-SUMMARY`, the summary `REPL` or `ISRT`: location `I003`.
    Summary,
    /// `8500-INSERT-AUTH`, the detail `ISRT`: location `I004`.
    Detail,
}

/// The decision of `6000-MAKE-DECISION`: the reply fields it sets beyond the
/// request's own — P-COPAUA0C-009, P-COPAUA0C-010.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthDecision {
    /// `AUTH-RESP-APPROVED`; `false` is `AUTH-RESP-DECLINED`.
    pub approved: bool,
    /// `PA-RL-AUTH-RESP-CODE PIC X(02)` (P-CCPAURLY-004): `00` or `05`.
    pub resp_code: Alnum<2>,
    /// `PA-RL-AUTH-RESP-REASON PIC X(04)` (P-CCPAURLY-005): `0000`, `3100` or
    /// `4100`.
    pub reason: Alnum<4>,
    /// `WS-APPROVED-AMT PIC S9(10)V99`
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L65], equal to
    /// `PA-RL-APPROVED-AMT`: the request amount when approved, zero when
    /// declined.
    pub approved_amount: Money<10>,
}

/// The summary write of `8400-UPDATE-SUMMARY`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L824-834].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryWrite {
    /// `ISRT PAUTSUM0`: the account had no summary
    /// (`AuthorizationStore::insert_summary`).
    Insert(PendingAuthSummary),
    /// `REPL PAUTSUM0`: the summary read by `5500` updated
    /// (`AuthorizationStore::update_summary`).
    Replace(PendingAuthSummary),
}

/// The writes of `8000-WRITE-AUTH-TO-DB`, in order: the summary, then the
/// detail under it (`AuthorizationStore::insert_detail`) — P-COPAUA0C-035.
///
/// A failed summary write ends the COBOL task before the detail insert
/// (`I003`); a failed detail insert leaves the summary write in place
/// (`I004`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthWritePlan {
    /// The summary insert or replace.
    pub summary: SummaryWrite,
    /// The detail segment `PAUTDTL1` to insert.
    pub detail: NewPendingAuthDetail,
}

/// `MOVE <code> TO WS-CODE-DISPLAY`, `MOVE WS-CODE-DISPLAY TO ERR-CODE-n`: the
/// unsigned `9(9)` receiver keeps the low-order nine digits of the magnitude,
/// left-justified in `X(09)`.
fn code_display(code: u64) -> Alnum<9> {
    Alnum::move_from(&format::unsigned_display(code, CODE_DISPLAY_DIGITS))
}

/// The CICS `RESP` and `RESP2` a failed read reports (E-44).
const fn cics_codes(error: &PortError) -> (u64, u64) {
    match error {
        PortError::NotFound => CICS_NOTFND,
        _ => CICS_FAILURE,
    }
}

/// The IMS status a failed DL/I call reports (E-44).
const fn ims_status(error: &PortError) -> &'static str {
    match error {
        PortError::NotFound => IMS_NOT_FOUND,
        PortError::Duplicate => IMS_DUPLICATE,
        _ => IMS_FAILURE,
    }
}

/// An `X(n)` key moved to `ERR-EVENT-KEY PIC X(20)`.
fn event_key(key: &[u8]) -> Alnum<20> {
    Alnum::move_from_bytes(key)
}

/// A warning of `5100` to `5300`: application subsystem, no codes moved.
fn warning(location: &'static str, message: &'static str, key: Alnum<20>) -> ErrorLogEntry {
    ErrorLogEntry {
        location,
        level: ErrorLevel::Warning,
        source: ErrorSource::App,
        code_1: Alnum::spaces(),
        code_2: Alnum::spaces(),
        message,
        event_key: key,
    }
}

/// A critical CICS read failure of `5100` to `5300`: `RESP` and `RESP2`
/// through `WS-CODE-DISPLAY`.
fn cics_failure(
    location: &'static str,
    message: &'static str,
    error: &PortError,
    key: Alnum<20>,
) -> ErrorLogEntry {
    let (resp, resp2) = cics_codes(error);
    ErrorLogEntry {
        location,
        level: ErrorLevel::Critical,
        source: ErrorSource::Cics,
        code_1: code_display(resp),
        code_2: code_display(resp2),
        message,
        event_key: key,
    }
}

/// A critical IMS failure: `IMS-RETURN-CODE` in `ERR-CODE-1`, `ERR-CODE-2` not
/// moved.
fn ims_failure(
    location: &'static str,
    message: &'static str,
    error: &PortError,
    key: Alnum<20>,
) -> ErrorLogEntry {
    ErrorLogEntry {
        location,
        level: ErrorLevel::Critical,
        source: ErrorSource::Ims,
        code_1: Alnum::move_from(ims_status(error)),
        code_2: Alnum::spaces(),
        message,
        event_key: key,
    }
}

/// A critical MQ failure: completion and reason codes through
/// `WS-CODE-DISPLAY`, and the detail area's `PA-CARD-NUM` as the event key,
/// which no statement has set before `8500` (E-25).
fn mq_failure(
    location: &'static str,
    source: ErrorSource,
    message: &'static str,
    comp_code: i64,
    reason: i64,
) -> ErrorLogEntry {
    ErrorLogEntry {
        location,
        level: ErrorLevel::Critical,
        source,
        code_1: code_display(comp_code.unsigned_abs()),
        code_2: code_display(reason.unsigned_abs()),
        message,
        event_key: Alnum::spaces(),
    }
}

/// COPAUA0C 5100-READ-XREF-RECORD `WHEN OTHER` — P-COPAUA0C-006: location
/// `C001`, critical, subsystem CICS, `"FAILED TO READ XREF FILE"`, event key
/// `XREF-CARD-NUM`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L501-512].
///
/// The codes follow E-44: `RESP` 17 and `RESP2` 0 for a store failure, 13 and
/// 80 for [`PortError::NotFound`]. The consumer also builds it when it cannot
/// open the read transaction, the failure that stands in for the read.
#[must_use]
pub fn xref_read_failure_entry(card: &CardNumber, error: &PortError) -> ErrorLogEntry {
    cics_failure(
        "C001",
        messages::FAILED_TO_READ_XREF_FILE,
        error,
        event_key(card.as_bytes()),
    )
}

/// COPAUA0C 3100-READ-REQUEST-MQ — P-COPAUA0C-002: location `M003`, critical,
/// `"FAILED TO READ REQUEST MQ"` for a get that fails other than with no
/// message available [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L415-430].
///
/// The COBOL sets subsystem CICS, not MQ, for this record
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L421]; the event key
/// is the never-set `PA-CARD-NUM` (E-25).
#[must_use]
pub fn request_read_failure_entry(comp_code: i64, reason: i64) -> ErrorLogEntry {
    mq_failure(
        "M003",
        ErrorSource::Cics,
        messages::FAILED_TO_READ_REQUEST_MQ,
        comp_code,
        reason,
    )
}

/// COPAUA0C 3100-READ-REQUEST-MQ — P-COPAUA0C-002: the request text as the
/// `MQGET` with `MQGMO-CONVERT`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L389-391] puts a
/// message of format `MQFMT-STRING`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L397] into
/// `W01-GET-BUFFER`.
///
/// The queue manager converts the characters of a string message into the
/// program's single-byte code page, so every `X(n)` receiver and the reply
/// count characters. The replacement reads `body` as UTF-8, the encoding its
/// replies are published in, and carries each character as the ISO 8859-1
/// byte of the same code point: the code page the repository's EBCDIC data
/// (CP037) maps to one for one, and the one [`Alnum::as_text`] reads back. An
/// ASCII body that fits the buffer is returned unchanged (E-80).
///
/// Every character of `body` is checked, so a character the conversion cannot
/// pin halts the request wherever it stands, even past the buffer, before the
/// size check. Only the first [`REQUEST_BUFFER_LEN`] + 1 converted bytes are
/// kept: the buffer, and the one byte past it that tells
/// [`oversize_request_entry`] the message does not fit. Text past that byte is
/// checked but not kept, so a message the size check rejects allocates at most
/// 501 bytes however long it is, and a message that fits comes back whole.
///
/// # Errors
///
/// [`Unproven`] `E-80` (halt) when `body` is not UTF-8 or holds a character
/// above `U+00FF`: the queue-manager CCSID that would decide their conversion
/// is not in the repository, so the request gets no read, no reply and no
/// write.
pub fn convert_request(body: &[u8]) -> Result<Vec<u8>, Unproven> {
    let text = std::str::from_utf8(body).map_err(|_| UNCONVERTED_TEXT)?;
    let kept = REQUEST_BUFFER_LEN + 1;
    let mut data = Vec::with_capacity(body.len().min(kept));
    for character in text.chars() {
        let byte = u8::try_from(character).map_err(|_| UNCONVERTED_TEXT)?;
        if data.len() < kept {
            data.push(byte);
        }
    }
    Ok(data)
}

/// COPAUA0C 3100-READ-REQUEST-MQ — P-COPAUA0C-002: the outcome of a request
/// whose converted text ([`convert_request`]) is longer than
/// [`REQUEST_BUFFER_LEN`], or `None` for one that fits.
///
/// `MQGMO-CONVERT` converts the message into the 500-byte `W01-GET-BUFFER`,
/// so `data` is the converted text, one byte per character, as
/// [`convert_request`] keeps it: whole when it fits, cut one byte past the
/// buffer when it does not, which is all this check reads. 500 `É`, 1000
/// bytes of UTF-8, fit. Without `MQGMO-ACCEPT-TRUNCATED-MSG` the get fails a
/// longer message with [`MQCC_WARNING`] and [`MQRC_TRUNCATED_MSG_FAILED`],
/// which takes the M003 branch: a critical record and the end of the task,
/// with no reply.
#[must_use]
pub fn oversize_request_entry(data: &[u8]) -> Option<ErrorLogEntry> {
    (data.len() > REQUEST_BUFFER_LEN)
        .then(|| request_read_failure_entry(MQCC_WARNING, MQRC_TRUNCATED_MSG_FAILED))
}

/// COPAUA0C 7100-SEND-RESPONSE — P-COPAUA0C-012: location `M004`, critical,
/// subsystem MQ, `"FAILED TO PUT ON REPLY MQ"`, with the completion and reason
/// codes of the failed put [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L767-778].
///
/// The event key is `PA-CARD-NUM` of the detail area, which `8500` has not
/// set yet, so it holds spaces (E-25). M004 ends the task: no write follows.
#[must_use]
pub fn reply_failure_entry(comp_code: i64, reason: i64) -> ErrorLogEntry {
    mq_failure(
        "M004",
        ErrorSource::Mq,
        messages::FAILED_TO_PUT_ON_REPLY_MQ,
        comp_code,
        reason,
    )
}

/// COPAUA0C 8400-UPDATE-SUMMARY and 8500-INSERT-AUTH — P-COPAUA0C-013,
/// P-COPAUA0C-014, P-COPAUA0C-035: the critical record of a failed write of
/// `plan` [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L837-847]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L922-932].
///
/// [`WriteStep::Summary`] is `I003` `"IMS UPDATE SUMRY FAILED"`, whose event
/// key `PA-CARD-NUM` is not set yet (E-25); the consumer then skips the
/// detail. [`WriteStep::Detail`] is `I004` `"IMS INSERT DETL FAILED"`, keyed
/// by the card `8500` moved into the detail. `ERR-CODE-1` is the IMS status of
/// the failure (E-44); `ERR-CODE-2` is not moved.
#[must_use]
pub fn write_failure_entry(
    step: WriteStep,
    error: &PortError,
    plan: &AuthWritePlan,
) -> ErrorLogEntry {
    match step {
        WriteStep::Summary => ims_failure(
            "I003",
            messages::IMS_UPDATE_SUMRY_FAILED,
            error,
            Alnum::spaces(),
        ),
        WriteStep::Detail => ims_failure(
            "I004",
            messages::IMS_INSERT_DETL_FAILED,
            error,
            event_key(plan.detail.pa_card_num.as_bytes()),
        ),
    }
}

/// The fields `UNSTRING … DELIMITED BY ','` hands to its receivers, in order.
///
/// Each field runs to the next delimiter or to the end of the data. Two
/// contiguous delimiters give an empty field. A delimiter that ends the data
/// gives no further field, so the receivers after it keep their value.
struct Unstring<'a> {
    /// `W01-GET-BUFFER(1:W01-DATALEN)`.
    data: &'a [u8],
    /// The offset of the next field.
    at: usize,
}

impl<'a> Iterator for Unstring<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.data.get(self.at..).filter(|rest| !rest.is_empty())?;
        let len = rest
            .iter()
            .position(|&byte| byte == DELIMITER)
            .unwrap_or(rest.len());
        self.at += len + 1;
        rest.get(..len)
    }
}

/// An alphanumeric receiver: the field moved left-justified, space-padded and
/// cut on the right; an empty field fills it with spaces. A receiver the
/// `UNSTRING` does not reach holds spaces too (E-25).
fn alnum_receiver<const N: usize>(field: Option<&[u8]>) -> Alnum<N> {
    field.map_or_else(Alnum::spaces, Alnum::move_from_bytes)
}

/// A numeric `9(N)` receiver: the field moved as an unsigned integer,
/// right-justified, zero-filled on the left and cut on the left; an empty
/// field fills it with zeros. Non-digits are moved as they are. A receiver the
/// `UNSTRING` does not reach holds spaces (E-25).
fn numeric_receiver<const N: usize>(field: Option<&[u8]>) -> Alnum<N> {
    let Some(field) = field else {
        return Alnum::spaces();
    };
    let kept = field.len().min(N);
    let mut bytes = [b'0'; N];
    bytes[N - kept..].copy_from_slice(&field[field.len() - kept..]);
    Alnum::from_array(bytes)
}

/// COPAUA0C 2100-EXTRACT-REQUEST-MSG — P-COPAUA0C-004: the `UNSTRING` of the
/// request text into its 18 receivers
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L354-374].
///
/// The fields are split at each `,` and given to auth date, auth time, card
/// number, auth type, card expiry date, message type, message source,
/// processing code, amount text, merchant category code, acquirer country
/// code, POS entry mode, merchant ID, name, city, state, zip and transaction
/// ID, each moved to its receiver's width; text after the 18th field is
/// ignored. `body` is the converted request text ([`convert_request`]), one
/// byte per character, so the widths count characters. At most
/// [`REQUEST_BUFFER_LEN`] bytes are read: a longer request never reaches this
/// paragraph ([`oversize_request_entry`]).
#[must_use]
pub fn parse_request(body: &[u8]) -> AuthRequest {
    let data = body.get(..REQUEST_BUFFER_LEN).unwrap_or(body);
    let fields: Vec<&[u8]> = Unstring { data, at: 0 }.take(RECEIVERS).collect();
    let field = |position: usize| fields.get(position).copied();
    AuthRequest {
        pa_rq_auth_date: alnum_receiver(field(0)),
        pa_rq_auth_time: alnum_receiver(field(1)),
        pa_rq_card_num: CardNumber::from_alnum(alnum_receiver(field(2))),
        pa_rq_auth_type: alnum_receiver(field(3)),
        pa_rq_card_expiry_date: alnum_receiver(field(4)),
        pa_rq_message_type: alnum_receiver(field(5)),
        pa_rq_message_source: alnum_receiver(field(6)),
        pa_rq_processing_code: numeric_receiver(field(PROCESSING_CODE_RECEIVER)),
        amount_text: alnum_receiver(field(AMOUNT_RECEIVER)),
        pa_rq_merchant_catagory_code: alnum_receiver(field(9)),
        pa_rq_acqr_country_code: alnum_receiver(field(10)),
        pa_rq_pos_entry_mode: numeric_receiver(field(POS_ENTRY_MODE_RECEIVER)),
        pa_rq_merchant_id: alnum_receiver(field(12)),
        pa_rq_merchant_name: alnum_receiver(field(13)),
        pa_rq_merchant_city: alnum_receiver(field(14)),
        pa_rq_merchant_state: alnum_receiver(field(15)),
        pa_rq_merchant_zip: alnum_receiver(field(16)),
        pa_rq_transaction_id: alnum_receiver(field(17)),
        fields_received: fields.len(),
    }
}

/// COPAUA0C 2100-EXTRACT-REQUEST-MSG / 8500-INSERT-AUTH — P-COPAUA0C-004,
/// P-COPAUA0C-014: whether every receiver the detail stores as text can be
/// stored, checked after the `UNSTRING`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L354-374].
///
/// `8500-INSERT-AUTH` moves the auth date and time, the card number, the auth
/// type, the expiry date, the message type and source, the merchant category
/// and acquirer country codes, the merchant ID, name, city, state and zip and
/// the transaction ID into `PENDING-AUTH-DETAILS` and inserts the segment
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L877-919]. IMS keeps any
/// byte, but `PostgreSQL` text cannot hold X'00' (E-07), and the card number
/// is also the key of the cross-reference read. Only the bytes a receiver
/// holds count: an X'00' cut off at the receiver's width, or after the 18th
/// field, is in no receiver.
///
/// The amount text and the numeric processing code and POS entry mode are not
/// stored as text, and their non-digit bytes keep their own cases:
/// [`parse_amount`] (E-28) and [`write_plan`] (E-47).
///
/// # Errors
///
/// [`Unproven`] `E-81` (halt) when one of those receivers holds X'00': the
/// request gets no read, no reply and no write, so no approval is sent for an
/// authorization whose detail could not be inserted.
pub fn storable(req: &AuthRequest) -> Result<(), Unproven> {
    let stored: [&[u8]; 15] = [
        req.pa_rq_auth_date.as_bytes(),
        req.pa_rq_auth_time.as_bytes(),
        req.pa_rq_card_num.as_bytes(),
        req.pa_rq_auth_type.as_bytes(),
        req.pa_rq_card_expiry_date.as_bytes(),
        req.pa_rq_message_type.as_bytes(),
        req.pa_rq_message_source.as_bytes(),
        req.pa_rq_merchant_catagory_code.as_bytes(),
        req.pa_rq_acqr_country_code.as_bytes(),
        req.pa_rq_merchant_id.as_bytes(),
        req.pa_rq_merchant_name.as_bytes(),
        req.pa_rq_merchant_city.as_bytes(),
        req.pa_rq_merchant_state.as_bytes(),
        req.pa_rq_merchant_zip.as_bytes(),
        req.pa_rq_transaction_id.as_bytes(),
    ];
    if stored.iter().any(|receiver| receiver.contains(&LOW_VALUE)) {
        Err(UNSTORABLE_TEXT)
    } else {
        Ok(())
    }
}

/// A `FUNCTION NUMVAL` argument split into its sign and digit strings.
struct NumvalText<'a> {
    /// A `-`, `CR` or `DB` sign.
    negative: bool,
    /// The digits before the decimal point.
    integer: &'a [u8],
    /// The digits after the decimal point.
    fraction: &'a [u8],
}

/// The offset of the first byte from `at` on that is not a space.
fn skip_spaces(text: &[u8], at: usize) -> usize {
    at + text.get(at..).map_or(0, |rest| {
        rest.iter().take_while(|&&byte| byte == b' ').count()
    })
}

/// The digit string of `text` from `at` on, possibly empty.
fn digits_from(text: &[u8], at: usize) -> &[u8] {
    let rest = text.get(at..).unwrap_or_default();
    let len = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    rest.get(..len).unwrap_or_default()
}

/// The sign and digits of `text` when it is an argument `FUNCTION NUMVAL`
/// accepts, `None` otherwise.
///
/// The two formats of the IBM Enterprise COBOL Language Reference are
/// accepted: `[sp][+|-][sp]{d[.[d]] | .d}[sp]` and
/// `[sp]{d[.[d]] | .d}[sp][+|-|CR|DB][sp]`, where `sp` is one or more spaces
/// and `d` one or more digits, with the period as the decimal point.
/// Lower-case `cr` and `db` are not accepted: the audit pins no result for
/// them (E-28).
fn numval_text(text: &[u8]) -> Option<NumvalText<'_>> {
    let mut at = skip_spaces(text, 0);
    let leading = match text.get(at) {
        Some(b'+') => Some(false),
        Some(b'-') => Some(true),
        _ => None,
    };
    if leading.is_some() {
        at = skip_spaces(text, at + 1);
    }
    let integer = digits_from(text, at);
    at += integer.len();
    let mut fraction: &[u8] = &[];
    if text.get(at) == Some(&b'.') {
        fraction = digits_from(text, at + 1);
        at += 1 + fraction.len();
    }
    if integer.is_empty() && fraction.is_empty() {
        return None;
    }
    at = skip_spaces(text, at);
    let mut trailing = None;
    if leading.is_none() {
        let (negative, len) = match text.get(at..).unwrap_or_default() {
            [b'+', ..] => (Some(false), 1),
            [b'-', ..] => (Some(true), 1),
            [b'C', b'R', ..] | [b'D', b'B', ..] => (Some(true), 2),
            _ => (None, 0),
        };
        trailing = negative;
        at = skip_spaces(text, at + len);
    }
    (at == text.len()).then_some(NumvalText {
        negative: leading.or(trailing).unwrap_or(false),
        integer,
        fraction,
    })
}

impl NumvalText<'_> {
    /// The signed cent count the `+9(10).99` receiver takes: two fraction
    /// digits kept and the rest rounded to the nearest cent.
    ///
    /// # Errors
    ///
    /// [`NUMVAL_UNPINNED`] when the digits after the second decimal are a 5
    /// followed only by zeros: a midpoint, whose floating-point rounding the
    /// audit does not pin (E-40).
    fn cents(&self) -> Result<i64, Unproven> {
        let value = |total: i64, byte: &u8| total * 10 + i64::from(byte - b'0');
        // At most 13 digits: the X(13) text keeps every sum below 10^15.
        let units = self.integer.iter().fold(0, value);
        let kept = self
            .fraction
            .iter()
            .chain(repeat(&b'0'))
            .take(2)
            .fold(0, value);
        let round_up = match self.fraction.get(2..).unwrap_or_default() {
            [] => false,
            [b'5', rest @ ..] if rest.iter().all(|&byte| byte == b'0') => {
                return Err(NUMVAL_UNPINNED);
            }
            [first, ..] => *first >= b'5',
        };
        let cents = units * 100 + kept + i64::from(round_up);
        Ok(if self.negative { -cents } else { cents })
    }
}

/// COPAUA0C 2100-EXTRACT-REQUEST-MSG — P-COPAUA0C-004, P-COPAUA0C-016,
/// P-COPAUA0C-024, P-COPAUA0C-025: `COMPUTE PA-RQ-TRANSACTION-AMT = FUNCTION
/// NUMVAL(WS-TRANSACTION-AMT-AN)`, then `MOVE PA-RQ-TRANSACTION-AMT TO
/// WS-TRANSACTION-AMT` [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L376-379].
///
/// `NUMVAL` returns a floating-point value, which the `+9(10).99` receiver
/// rounds to the cent. Text with at most two decimals converts exactly
/// (`"100.29"` is 100.29, `"-50.00"` is -50.00), and text with more converts to
/// the nearest cent (`"10.006"` is 10.01).
///
/// # Errors
///
/// [`Unproven`] for the input classes the audit does not pin:
///
/// - `E-25` when the request has no amount field: the receiver keeps bytes no
///   statement set;
/// - `E-28` when `NUMVAL` does not accept the text;
/// - `E-40` for a midpoint beyond the cent (`"0.005"`) and for a value whose
///   integer part needs more than the 10 digits of the receiver.
pub fn parse_amount(req: &AuthRequest) -> Result<Money<10>, Unproven> {
    if !req.received(AMOUNT_RECEIVER) {
        return Err(NEVER_SET);
    }
    let text = numval_text(req.amount_text.as_bytes()).ok_or(NUMVAL_MALFORMED)?;
    Money::from_cents(text.cents()?).map_err(|_| NUMVAL_UNPINNED)
}

/// COPAUA0C 5000-PROCESS-AUTH, 5100-READ-XREF-RECORD to 5500-READ-AUTH-SUMMRY
/// — P-COPAUA0C-005 to P-COPAUA0C-008, P-COPAUA0C-034: the reads that come
/// before the decision, through the ports
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L438-457].
///
/// The cross-reference is read by the request's card number. A card it holds
/// leads to the account and the customer of the cross-reference and to the
/// account's pending-authorization summary, in that order. An unknown card is
/// warning `A001` and nothing more is read; an account or customer not found
/// is warning `A002` or `A003` and the reads go on; a summary not found
/// (`GE`) raises nothing. Any other failure is a critical record (`C001`,
/// `C002`, `C003`, `I002`) after which nothing is read:
/// [`AuthContext::critical_entry`] then names the record that ends the task.
pub async fn gather<S>(store: &mut S, req: &AuthRequest) -> AuthContext
where
    S: XrefReader + AccountReader + CustomerReader + AuthorizationStore,
{
    let card = req.pa_rq_card_num;
    let xref = match store.read_xref_by_card(card).await {
        Ok(xref) => xref,
        Err(PortError::NotFound) => {
            let mut context = AuthContext::with_xref(Lookup::NotFound);
            context.log.push(warning(
                "A001",
                messages::CARD_NOT_FOUND_IN_XREF,
                event_key(card.as_bytes()),
            ));
            return context;
        }
        Err(error) => {
            let mut context = AuthContext::with_xref(Lookup::Failed);
            context.log.push(xref_read_failure_entry(&card, &error));
            return context;
        }
    };
    let mut context = AuthContext::with_xref(Lookup::Found(xref.clone()));
    if read_account(store, &xref, &mut context).await
        && read_customer(store, &xref, &mut context).await
    {
        read_summary(store, &xref, &mut context).await;
    }
    context
}

/// COPAUA0C 5200-READ-ACCT-RECORD — P-COPAUA0C-007: `ACCTDAT` by
/// `XREF-ACCT-ID`, whose 11 digits key the `A002` and `C002` records
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L520-562]. `false` when
/// the read failed and the task ends.
async fn read_account<S: AccountReader>(
    store: &mut S,
    xref: &CardXref,
    context: &mut AuthContext,
) -> bool {
    let key = event_key(xref.xref_acct_id.to_string().as_bytes());
    match store.read_account(xref.xref_acct_id).await {
        Ok(account) => {
            context.account = Lookup::Found(account);
            true
        }
        Err(PortError::NotFound) => {
            context.account = Lookup::NotFound;
            context
                .log
                .push(warning("A002", messages::ACCT_NOT_FOUND_IN_XREF, key));
            true
        }
        Err(error) => {
            context.account = Lookup::Failed;
            context.log.push(cics_failure(
                "C002",
                messages::FAILED_TO_READ_ACCT_FILE,
                &error,
                key,
            ));
            false
        }
    }
}

/// COPAUA0C 5300-READ-CUST-RECORD — P-COPAUA0C-007: `CUSTDAT` by
/// `XREF-CUST-ID`, whose 9 digits key the `A003` and `C003` records
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L568-610]. `false` when
/// the read failed and the task ends.
async fn read_customer<S: CustomerReader>(
    store: &mut S,
    xref: &CardXref,
    context: &mut AuthContext,
) -> bool {
    let key = event_key(xref.xref_cust_id.to_string().as_bytes());
    match store.read_customer(xref.xref_cust_id).await {
        Ok(customer) => {
            context.customer = Lookup::Found(customer);
            true
        }
        Err(PortError::NotFound) => {
            context.customer = Lookup::NotFound;
            context
                .log
                .push(warning("A003", messages::CUST_NOT_FOUND_IN_XREF, key));
            true
        }
        Err(error) => {
            context.customer = Lookup::Failed;
            context.log.push(cics_failure(
                "C003",
                messages::FAILED_TO_READ_CUST_FILE,
                &error,
                key,
            ));
            false
        }
    }
}

/// COPAUA0C 5500-READ-AUTH-SUMMRY — P-COPAUA0C-008: `GU PAUTSUM0 WHERE
/// (ACCNTID = XREF-ACCT-ID)`; `GE` is no summary, any other failure is `I002`,
/// keyed by the never-set `PA-CARD-NUM` (E-25)
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L616-640].
///
/// The `GU` runs with the update intent of `PSBPAUTB` (`PROCOPT=AP`), so a
/// summary a running purge holds is waited for and the decision reads what
/// the purge left, as IMS program isolation orders `COPAUA0C` after the
/// `CBPAUP0C` BMP: [`AuthorizationStore::read_summary_for_update`].
async fn read_summary<S: AuthorizationStore>(
    store: &mut S,
    xref: &CardXref,
    context: &mut AuthContext,
) {
    context.summary = match store.read_summary_for_update(xref.xref_acct_id).await {
        Ok(summary) => Lookup::Found(summary),
        Err(PortError::NotFound) => Lookup::NotFound,
        Err(error) => {
            context.log.push(ims_failure(
                "I002",
                messages::IMS_GET_SUMMARY_FAILED,
                &error,
                Alnum::spaces(),
            ));
            Lookup::Failed
        }
    };
}

/// COPAUA0C 6000-MAKE-DECISION — P-COPAUA0C-009, P-COPAUA0C-010,
/// P-COPAUA0C-017, P-COPAUA0C-018, P-COPAUA0C-021 to P-COPAUA0C-026,
/// P-COPAUA0C-028: the decision on `amount` from what [`gather`] found
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L664-718].
///
/// - A summary found: `WS-AVAILABLE-AMT = PA-CREDIT-LIMIT - PA-CREDIT-BALANCE`
///   (P-COPAUA0C-017). Otherwise an account found: `ACCT-CREDIT-LIMIT -
///   ACCT-CURR-BAL` (P-COPAUA0C-018). Both are stored into the `S9(09)V99`
///   `WS-AVAILABLE-AMT`, which drops high-order digits.
/// - An amount above the available amount declines with insufficient funds;
///   an amount equal to it is approved. No summary and no account declines.
/// - Approved: `00`, reason `0000`, the request amount. Declined: `05`, amount
///   0, reason `3100` when the card, the account or the customer was not
///   found, tested first, else `4100` for insufficient funds. A customer not
///   found declines nothing by itself.
///
/// An unknown card is declined with `3100`. `WS-PAUT-SMRY-SEG-FLG` has no
/// `VALUE` and only `5500` sets it, so for a card the cross-reference lacks the
/// COBOL tests the flag's starting byte: every byte but `Y` takes this decline,
/// which audit row P-COPAUA0C-026 records as the E-41 proposal; `Y` would read
/// summary storage no read filled. E-41 stays at `halt` until the user
/// confirms the decline, so [`decidable`] reports the case and no reply is
/// sent for it.
///
/// The context must hold no critical record ([`AuthContext::critical_entry`]):
/// the COBOL task ends before this paragraph.
#[must_use]
pub fn decide(amount: Money<10>, context: &AuthContext) -> AuthDecision {
    let available = match (&context.summary, &context.account) {
        (Lookup::Found(summary), _) => {
            Some((summary.pa_credit_limit - summary.pa_credit_balance).store::<9>())
        }
        (_, Lookup::Found(account)) => {
            Some((account.acct_credit_limit - account.acct_curr_bal).store::<9>())
        }
        _ => None,
    };
    let insufficient_fund = available.is_some_and(|limit| amount > limit);
    if available.is_some() && !insufficient_fund {
        return AuthDecision {
            approved: true,
            resp_code: Alnum::from_array(RESP_APPROVED),
            reason: Alnum::from_array(REASON_NONE),
            approved_amount: amount,
        };
    }
    let not_found = matches!(context.xref, Lookup::NotFound)
        || !matches!(context.account, Lookup::Found(_))
        || matches!(context.customer, Lookup::NotFound);
    let reason = if not_found {
        REASON_NOT_FOUND
    } else if insufficient_fund {
        REASON_INSUFFICIENT_FUND
    } else {
        REASON_OTHER
    };
    AuthDecision {
        approved: false,
        resp_code: Alnum::from_array(RESP_DECLINED),
        reason: Alnum::from_array(reason),
        approved_amount: Money::ZERO,
    }
}

/// COPAUA0C 6000-MAKE-DECISION — P-COPAUA0C-026, P-COPAUA0C-028: whether the
/// audit pins the decision of a request whose reads found `context`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L664-683].
///
/// A card the cross-reference holds is decided by [`decide`]. A card it lacks
/// is not: `5100` leaves `CARD-NFOUND-XREF` set and skips `5500`, the only
/// paragraph that sets `WS-PAUT-SMRY-SEG-FLG`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L441-457]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L616-640], and the flag
/// has no `VALUE` [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L134-136].
/// The first message of a task therefore branches on storage no statement set
/// and no runtime setting in the repository initializes, so neither the reply
/// nor the decline reason is pinned.
///
/// # Errors
///
/// [`Unproven`] `E-41` (halt) for a card the cross-reference lacks: the
/// consumer sends no reply for it and writes nothing. The `3100` decline
/// [`decide`] gives such a card is the escalation's proposal, for the user to
/// confirm.
pub fn decidable(context: &AuthContext) -> Result<(), Unproven> {
    if matches!(context.xref, Lookup::NotFound) {
        Err(UNKNOWN_CARD_DECISION)
    } else {
        Ok(())
    }
}

/// COPAUA0C 6000-MAKE-DECISION — P-COPAUA0C-011, P-COPAUA0C-020,
/// P-COPAUA0C-021 to P-COPAUA0C-027: the reply text
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L660-662]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L720-731].
///
/// The card number, the transaction ID and, as the authorization ID code, the
/// auth time come from the request; the response code, the reason and the
/// approved amount (`-zzzzzzzzz9.99`) from `decision`. Each is followed by a
/// comma, and byte 64 ends the message: [`format::cp00_reply`] builds the 64
/// bytes of the first-message reply (E-13) from the receivers' bytes, in the
/// code page of the converted request ([`convert_request`]).
///
/// The text returned holds one character per byte of that message, so it is
/// 64 characters whatever they are: the transaction ID `TXNÉ` comes back
/// followed by the 11 spaces of its `X(15)`, and the consumer publishes the
/// text as UTF-8, the encoding the request was read in (E-80).
#[must_use]
pub fn reply_body(req: &AuthRequest, decision: &AuthDecision) -> String {
    format::cp00_reply(
        req.pa_rq_card_num.as_alnum(),
        &req.pa_rq_transaction_id,
        &req.pa_rq_auth_time,
        &decision.resp_code,
        &decision.reason,
        decision.approved_amount,
    )
    .as_text()
    .into_owned()
}

/// COPAUA0C 8000-WRITE-AUTH-TO-DB, 8400-UPDATE-SUMMARY and 8500-INSERT-AUTH —
/// P-COPAUA0C-013, P-COPAUA0C-014, P-COPAUA0C-019, P-COPAUA0C-030,
/// P-COPAUA0C-035: the summary and detail writes of a decided request, or
/// `None` when the card is not in the cross-reference and nothing is written
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L463-465].
///
/// `now` is the instant of the `ASKTIME` in `8500`, which runs after the reply
/// is sent: the consumer reads it from its clock after the acknowledgement.
///
/// # Errors
///
/// [`Unproven`] for a found card whose writes the audit cannot pin:
///
/// - `E-41` for a declined request: `8400` adds the detail area's
///   `PA-TRANSACTION-AMT` to the declined total before `8500` sets it
///   (P-COPAUA0C-028);
/// - `E-25` when the account was not found, so the limits `8400` copies come
///   from an account record no read filled, and when the request had no
///   processing code or POS entry mode field;
/// - `E-47` when the processing code or POS entry mode holds non-digits;
/// - `E-78` when the approval would take the approved count past 9999.
pub fn write_plan(
    req: &AuthRequest,
    amount: Money<10>,
    context: &AuthContext,
    decision: &AuthDecision,
    now: DateTime<Utc>,
) -> Result<Option<AuthWritePlan>, Unproven> {
    let Some(xref) = approval_to_write(context, decision)? else {
        return Ok(None);
    };
    let found = match &context.summary {
        Lookup::Found(summary) => Some(summary),
        Lookup::NotFound | Lookup::Failed => None,
    };
    let summary = approved_summary(xref, context, found, decision)?;
    let detail = new_detail(req, amount, xref, decision, now)?;
    Ok(Some(AuthWritePlan { summary, detail }))
}

/// COPAUA0C 8400-UPDATE-SUMMARY — P-COPAUA0C-008, P-COPAUA0C-013,
/// P-COPAUA0C-019: the summary write of [`write_plan`] derived again on
/// `locked`, the summary the `8000-WRITE-AUTH-TO-DB` unit of work read under
/// its lock (`AuthorizationStore::read_summary_for_update`)
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L801-834].
///
/// `COPAUA0C` replaces or inserts the summary its update-intent `GU` holds,
/// and IMS program isolation keeps the `CBPAUP0C` BMP away from it until the
/// task's syncpoint. The replacement writes in a unit of work of its own,
/// once the reply is confirmed and the request acknowledged, so a purge can
/// change or delete the summary between the read that decided the request
/// and the write. The write is therefore derived on the summary as the write
/// unit holds it, by the same `8400` statements:
///
/// - `Some(summary)`, the `GU` found it: [`SummaryWrite::Replace`] of that
///   summary with the account's limits stored into `S9(09)V99`, the approved
///   count up by one, the approved total and the credit balance up by the
///   approved amount and the cash balance zero;
/// - `None`, the `GE` of a summary that is gone or was never written:
///   [`SummaryWrite::Insert`] of the `INITIALIZE`d summary of the
///   cross-reference with the same updates.
///
/// The decision and the detail stay those of the reply already sent; only
/// the summary write follows what the lock found.
///
/// # Errors
///
/// [`Unproven`] exactly where [`write_plan`] reports it for the summary of
/// `context` and `decision`: `E-41` for a declined request, `E-25` for an
/// account not found, and `E-78` when `locked` already holds 9999
/// approvals. `Ok(None)` exactly when [`write_plan`] plans nothing for
/// `context`: a card the cross-reference lacks, or a context with a critical
/// record.
pub fn locked_summary_write(
    context: &AuthContext,
    decision: &AuthDecision,
    locked: Option<&PendingAuthSummary>,
) -> Result<Option<SummaryWrite>, Unproven> {
    let Some(xref) = approval_to_write(context, decision)? else {
        return Ok(None);
    };
    approved_summary(xref, context, locked, decision).map(Some)
}

/// COPAUA0C 8000-WRITE-AUTH-TO-DB — P-COPAUA0C-028, P-COPAUA0C-035: the
/// cross-reference whose account `8400` and `8500` write under, `None` when
/// the task writes nothing: a card the cross-reference lacks
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L463-465], or a context
/// whose critical record ended the task before `8000`.
///
/// # Errors
///
/// `E-41` for a declined request: `8400` adds the detail area's
/// `PA-TRANSACTION-AMT` to the declined total before `8500` sets it.
fn approval_to_write<'c>(
    context: &'c AuthContext,
    decision: &AuthDecision,
) -> Result<Option<&'c CardXref>, Unproven> {
    let Lookup::Found(xref) = &context.xref else {
        return Ok(None);
    };
    if context.critical_entry().is_some() {
        return Ok(None);
    }
    if !decision.approved {
        return Err(DECLINED_UPDATE);
    }
    Ok(Some(xref))
}

/// COPAUA0C 8400-UPDATE-SUMMARY — P-COPAUA0C-013: `INITIALIZE
/// PENDING-AUTH-SUMMARY REPLACING NUMERIC DATA BY ZERO`, then the account and
/// customer of the cross-reference
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L801-808].
///
/// `REPLACING NUMERIC DATA` leaves the alphanumeric `PA-AUTH-STATUS` and
/// `PA-ACCOUNT-STATUS` as they were, and nothing has set them on the first
/// message of a task: they hold spaces (E-25).
fn initialized_summary(xref: &CardXref) -> PendingAuthSummary {
    PendingAuthSummary {
        pa_acct_id: xref.xref_acct_id,
        pa_cust_id: xref.xref_cust_id,
        pa_auth_status: Alnum::spaces(),
        pa_account_status: [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS],
        pa_credit_limit: Money::ZERO,
        pa_cash_limit: Money::ZERO,
        pa_credit_balance: Money::ZERO,
        pa_cash_balance: Money::ZERO,
        pa_approved_auth_cnt: AuthCount::ZERO,
        pa_declined_auth_cnt: AuthCount::ZERO,
        pa_approved_auth_amt: Money::ZERO,
        pa_declined_auth_amt: Money::ZERO,
    }
}

/// COPAUA0C 8400-UPDATE-SUMMARY — P-COPAUA0C-013, P-COPAUA0C-019: the summary
/// of an approved request [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L801-834].
///
/// `found` is the summary the `GU` found (`FOUND-PAUT-SMRY-SEG`), `None` for
/// its `GE`: the summary `5500` read for [`write_plan`], or the one the write
/// unit holds for [`locked_summary_write`]. A summary found is replaced;
/// without one, a new summary is inserted. The credit and cash limits are
/// moved from the account's `S9(10)V99` limits into `S9(09)V99`, which drops
/// high-order digits; the approved count goes up by one, the approved total
/// and the credit balance by the approved amount (stored into `S9(09)V99`),
/// and the cash balance is set to zero.
fn approved_summary(
    xref: &CardXref,
    context: &AuthContext,
    found: Option<&PendingAuthSummary>,
    decision: &AuthDecision,
) -> Result<SummaryWrite, Unproven> {
    let Lookup::Found(account) = &context.account else {
        return Err(NEVER_SET);
    };
    let exists = found.is_some();
    let mut summary = found.cloned().unwrap_or_else(|| initialized_summary(xref));
    summary.pa_credit_limit = account.acct_credit_limit.store::<9>();
    summary.pa_cash_limit = account.acct_cash_credit_limit.store::<9>();
    // `ADD 1` without `ON SIZE ERROR` into `S9(04) COMP`: past 9999 the TRUNC
    // compiler option, which the repository does not fix, decides the result.
    summary.pa_approved_auth_cnt = summary
        .pa_approved_auth_cnt
        .checked_increment()
        .map_err(|_| COUNT_PAST_9999)?;
    summary.pa_approved_auth_amt =
        (summary.pa_approved_auth_amt + decision.approved_amount).store::<9>();
    summary.pa_credit_balance = (summary.pa_credit_balance + decision.approved_amount).store::<9>();
    summary.pa_cash_balance = Money::ZERO;
    Ok(if exists {
        SummaryWrite::Replace(summary)
    } else {
        SummaryWrite::Insert(summary)
    })
}

/// The detail key `(PA-AUTH-DATE-9C, PA-AUTH-TIME-9C)` of `now` —
/// P-COPAUA0C-030.
fn detail_key(now: DateTime<Utc>) -> (i32, i32) {
    let (date, time) = clock::auth_key_complement(now);
    // HHMMSS * 1000 + milliseconds is at most 235_959_999, so the complement
    // lies in 764_040_000..=999_999_999, inside the i32 range.
    let time = i32::try_from(time).expect("a complemented PA-AUTH-TIME-9C has nine digits");
    (date, time)
}

/// A numeric request receiver as the `9(N)` detail field it is moved to.
///
/// # Errors
///
/// `E-25` when the `UNSTRING` did not reach the receiver; `E-47` when it holds
/// a byte other than a digit.
fn numeric_field<const N: u8, const W: usize>(
    field: &Alnum<W>,
    received: bool,
) -> Result<NumericCode<N>, Unproven> {
    if !received {
        return Err(NEVER_SET);
    }
    field.as_text().parse().map_err(|_| NON_DIGITS)
}

/// COPAUA0C 8500-INSERT-AUTH — P-COPAUA0C-014, P-COPAUA0C-030: the detail
/// segment [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L857-911].
///
/// The key is complemented from `now`; the request fields, the request amount
/// and the reply's authorization ID code, response code, reason and approved
/// amount are moved in; the match status is pending when approved and
/// declined otherwise; the fraud flag and report date are spaces; the parent
/// account is `XREF-ACCT-ID`.
fn new_detail(
    req: &AuthRequest,
    amount: Money<10>,
    xref: &CardXref,
    decision: &AuthDecision,
    now: DateTime<Utc>,
) -> Result<NewPendingAuthDetail, Unproven> {
    let (pa_auth_date_9c, pa_auth_time_9c) = detail_key(now);
    Ok(NewPendingAuthDetail {
        pa_acct_id: xref.xref_acct_id,
        pa_auth_date_9c,
        pa_auth_time_9c,
        pa_auth_orig_date: req.pa_rq_auth_date,
        pa_auth_orig_time: req.pa_rq_auth_time,
        pa_card_num: req.pa_rq_card_num,
        pa_auth_type: req.pa_rq_auth_type,
        pa_card_expiry_date: req.pa_rq_card_expiry_date,
        pa_message_type: req.pa_rq_message_type,
        pa_message_source: req.pa_rq_message_source,
        pa_auth_id_code: req.pa_rq_auth_time,
        pa_auth_resp_code: decision.resp_code,
        pa_auth_resp_reason: decision.reason,
        pa_processing_code: numeric_field(
            &req.pa_rq_processing_code,
            req.received(PROCESSING_CODE_RECEIVER),
        )?,
        pa_transaction_amt: amount,
        pa_approved_amt: decision.approved_amount,
        pa_merchant_catagory_code: req.pa_rq_merchant_catagory_code,
        pa_acqr_country_code: req.pa_rq_acqr_country_code,
        pa_pos_entry_mode: numeric_field(
            &req.pa_rq_pos_entry_mode,
            req.received(POS_ENTRY_MODE_RECEIVER),
        )?,
        pa_merchant_id: req.pa_rq_merchant_id,
        pa_merchant_name: req.pa_rq_merchant_name,
        pa_merchant_city: req.pa_rq_merchant_city,
        pa_merchant_state: req.pa_rq_merchant_state,
        pa_merchant_zip: req.pa_rq_merchant_zip,
        pa_transaction_id: req.pa_rq_transaction_id,
        pa_match_status: if decision.approved {
            MatchStatus::Pending
        } else {
            MatchStatus::AuthDeclined
        },
        pa_auth_fraud: FraudFlag::Blank,
        pa_fraud_rpt_date: Alnum::spaces(),
    })
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::{
        AuthContext, AuthDecision, AuthRequest, ERR_APPLICATION, ERR_PROGRAM, ErrorLevel,
        ErrorLogEntry, ErrorSource, Lookup, REQUEST_BUFFER_LEN, SummaryWrite, WriteStep,
        convert_request, decidable, decide, gather, locked_summary_write, oversize_request_entry,
        parse_amount, parse_request, reply_body, reply_failure_entry, request_read_failure_entry,
        storable, write_failure_entry, write_plan, xref_read_failure_entry,
    };
    use crate::entities::{
        Account, CardXref, Customer, FraudFlag, MatchStatus, PA_ACCOUNT_STATUS_OCCURS,
        PendingAuthSummary,
    };
    use crate::messages;
    use crate::numeric::{
        AccountGroupId, AccountId, Alnum, AuthCount, CardNumber, CustSsn, CustomerId, FicoScore,
        Money, NumericCode,
    };
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    /// The card of the first cross-reference fixture record, on customer and
    /// account 50 [app/data/ASCII/cardxref.txt:L1].
    const FIXTURE_CARD: &str = "0500024453765740";

    /// The card of P-COPAUA0C-026, which no cross-reference holds.
    const UNKNOWN_CARD: &str = "9999999999999999";

    /// The instant P-COPAUA0C-030 pins the detail key to.
    const PINNED: &str = "2022-07-01T12:00:00Z";

    fn cents<const D: u8>(value: i64) -> Money<D> {
        Money::from_cents(value).expect("an amount within the field")
    }

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("an RFC 3339 instant")
            .with_timezone(&Utc)
    }

    fn pad(text: &str, width: usize) -> String {
        format!("{text:<width$}")
    }

    /// The D.10 request text of P-COPAUA0C-021 with its card, amount and
    /// transaction ID replaced.
    fn request_text(card: &str, amount: &str, transaction: &str) -> String {
        format!(
            "220701,120000,{card},0100,1225,0100,POS,000000,{amount},5411,840,01,\
             M00000000000001,FIXTURE MERCHANT,SEATTLE,WA,98101,{transaction}"
        )
    }

    fn fixture_request(amount: &str) -> AuthRequest {
        parse_request(request_text(FIXTURE_CARD, amount, "TXN000000000001").as_bytes())
    }

    /// Account 50 as `acctdata.txt` L50 decodes: credit limit 6169.00, balance
    /// 492.00, cash limit 4587.00, so 5677.00 is available (P-COPAUA0C-021).
    fn fixture_account() -> Account {
        Account {
            acct_id: AccountId::new(50).expect("an account number"),
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: cents(49_200),
            acct_credit_limit: cents(616_900),
            acct_cash_credit_limit: cents(458_700),
            acct_open_date: Alnum::move_from("2011-04-22"),
            acct_expiraion_date: Alnum::move_from("2023-03-09"),
            acct_reissue_date: Alnum::move_from("2023-03-09"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        }
    }

    fn fixture_customer() -> Customer {
        Customer {
            cust_id: CustomerId::new(50).expect("a customer number"),
            cust_first_name: Alnum::move_from("Aniya"),
            cust_middle_name: Alnum::move_from("Von"),
            cust_last_name: Alnum::move_from("Kessler"),
            cust_addr_line_1: Alnum::move_from("618 Deshaun Route"),
            cust_addr_line_2: Alnum::move_from("Apt. 802"),
            cust_addr_line_3: Alnum::move_from("Altenwerthshire"),
            cust_addr_state_cd: Alnum::move_from("NC"),
            cust_addr_country_cd: Alnum::move_from("USA"),
            cust_addr_zip: Alnum::move_from("12546"),
            cust_phone_num_1: Alnum::move_from("(908)119-8310"),
            cust_phone_num_2: Alnum::move_from("(373)693-8684"),
            cust_ssn: CustSsn::new(20_973_888).expect("a nine-digit number"),
            cust_govt_issued_id: Alnum::move_from("00000000000049368437"),
            cust_dob_yyyy_mm_dd: Alnum::move_from("1961-06-08"),
            cust_eft_account_id: Alnum::move_from("0053581756"),
            cust_pri_card_holder_ind: Alnum::move_from("Y"),
            cust_fico_credit_score: FicoScore::new(274).expect("a three-digit score"),
        }
    }

    fn fixture_xref() -> CardXref {
        CardXref {
            xref_card_num: CardNumber::move_from(FIXTURE_CARD),
            xref_cust_id: CustomerId::new(50).expect("a customer number"),
            xref_acct_id: AccountId::new(50).expect("an account number"),
        }
    }

    /// The seeded state the D.10 rows start from: the card's cross-reference,
    /// account and customer, and no pending authorization.
    fn fixture_store() -> FakeStore {
        FakeStore::default()
            .with_xref(fixture_xref())
            .with_account(fixture_account())
            .with_customer(fixture_customer())
    }

    /// A summary of account 50 with credit limit `limit` and balance `balance`
    /// cents and earlier totals in every other field.
    fn summary(limit: i64, balance: i64) -> PendingAuthSummary {
        PendingAuthSummary {
            pa_acct_id: AccountId::new(50).expect("an account number"),
            pa_cust_id: CustomerId::new(50).expect("a customer number"),
            pa_auth_status: Alnum::move_from("A"),
            pa_account_status: [Alnum::move_from("01"); PA_ACCOUNT_STATUS_OCCURS],
            pa_credit_limit: cents(limit),
            pa_cash_limit: cents(20_000),
            pa_credit_balance: cents(balance),
            pa_cash_balance: cents(1_000),
            pa_approved_auth_cnt: AuthCount::new(2).expect("a count"),
            pa_declined_auth_cnt: AuthCount::new(1).expect("a count"),
            pa_approved_auth_amt: cents(20_000),
            pa_declined_auth_amt: cents(5_000),
        }
    }

    /// Converts and parses `body`, reads `store` for it and decides it.
    fn authorize(
        store: &mut FakeStore,
        body: &str,
    ) -> (AuthRequest, Money<10>, AuthContext, AuthDecision) {
        let data = convert_request(body.as_bytes()).expect("convertible request text");
        let request = parse_request(&data);
        let amount = parse_amount(&request).expect("a pinned amount");
        let context = block_on(gather(store, &request));
        let decision = decide(amount, &context);
        (request, amount, context, decision)
    }

    /// The fields of `entry` as text, for one comparison per record.
    fn fields(entry: &ErrorLogEntry) -> (&str, ErrorLevel, ErrorSource, String, String, String) {
        (
            entry.location,
            entry.level,
            entry.source,
            entry.code_1.to_string(),
            entry.code_2.to_string(),
            entry.event_key.to_string(),
        )
    }

    /// The plan of an approved request against `store` at the pinned instant.
    fn approved_plan(store: &mut FakeStore, amount: &str) -> super::AuthWritePlan {
        let body = request_text(FIXTURE_CARD, amount, "TXN000000000001");
        let (request, amount, context, decision) = authorize(store, &body);
        assert!(decision.approved, "{decision:?}");
        write_plan(&request, amount, &context, &decision, at(PINNED))
            .expect("a pinned plan")
            .expect("a plan for a found card")
    }

    #[test]
    fn a_full_request_fills_the_eighteen_receivers() {
        // P-COPAUA0C-004 with the P-COPAUA0C-021 request.
        let request = fixture_request("100.00");
        assert_eq!(request.fields_received, 18);
        let texts = [
            request.pa_rq_auth_date.to_string(),
            request.pa_rq_auth_time.to_string(),
            request.pa_rq_card_num.to_string(),
            request.pa_rq_auth_type.to_string(),
            request.pa_rq_card_expiry_date.to_string(),
            request.pa_rq_message_type.to_string(),
            request.pa_rq_message_source.to_string(),
            request.pa_rq_processing_code.to_string(),
            request.amount_text.to_string(),
            request.pa_rq_merchant_catagory_code.to_string(),
            request.pa_rq_acqr_country_code.to_string(),
            request.pa_rq_pos_entry_mode.to_string(),
            request.pa_rq_merchant_id.to_string(),
            request.pa_rq_merchant_name.to_string(),
            request.pa_rq_merchant_city.to_string(),
            request.pa_rq_merchant_state.to_string(),
            request.pa_rq_merchant_zip.to_string(),
            request.pa_rq_transaction_id.to_string(),
        ];
        let expected = [
            "220701",
            "120000",
            FIXTURE_CARD,
            "0100",
            "1225",
            "0100  ",
            "POS   ",
            "000000",
            "100.00       ",
            "5411",
            "840",
            "01",
            "M00000000000001",
            "FIXTURE MERCHANT      ",
            "SEATTLE      ",
            "WA",
            "98101    ",
            "TXN000000000001",
        ];
        assert_eq!(texts, expected);
    }

    #[test]
    fn each_field_moves_to_its_receiver_as_unstring_moves_it() {
        // P-COPAUA0C-004: alphanumeric receivers cut on the right, numeric ones
        // keep the low-order digits, text after the 18th field is ignored, and
        // contiguous delimiters fill a receiver with spaces or zeros.
        let long = "220701,120000,0500024453765740,0100,1225,0100,POS,1234567,100.00,5411,\
                    840,1,M00000000000001,A MERCHANT NAME LONGER THAN 22,SEATTLE,WA,98101,\
                    TXN000000000001,EXTRA";
        let request = parse_request(long.as_bytes());
        assert_eq!(request.fields_received, 18);
        assert_eq!(request.pa_rq_processing_code.to_string(), "234567");
        assert_eq!(request.pa_rq_pos_entry_mode.to_string(), "01");
        assert_eq!(
            request.pa_rq_merchant_name.to_string(),
            "A MERCHANT NAME LONGER"
        );
        assert_eq!(request.pa_rq_transaction_id.to_string(), "TXN000000000001");

        let gaps = parse_request(b",120000,0500024453765740,,1225,0100,POS,,100.00");
        assert_eq!(gaps.fields_received, 9);
        assert_eq!(gaps.pa_rq_auth_date.to_string(), "      ");
        assert_eq!(gaps.pa_rq_auth_type.to_string(), "    ");
        assert_eq!(gaps.pa_rq_processing_code.to_string(), "000000");
        assert_eq!(gaps.pa_rq_pos_entry_mode.to_string(), "  ", "not reached");
        assert_eq!(parse_amount(&gaps).map(Money::cents), Ok(10_000));
    }

    #[test]
    fn receivers_a_short_request_does_not_reach_hold_spaces() {
        // P-COPAUA0C-004; E-25 for the receivers never reached.
        for (body, received) in [
            ("220701,120000,0500024453765740,0100", 4),
            ("220701,", 1),
            ("", 0),
        ] {
            let request = parse_request(body.as_bytes());
            assert_eq!(request.fields_received, received, "{body:?}");
            assert!(request.amount_text.is_blank(), "{body:?}");
            assert!(request.pa_rq_transaction_id.is_blank(), "{body:?}");
            assert_eq!(
                parse_amount(&request).map_err(|unproven| unproven.escalation_id),
                Err("E-25"),
                "{body:?}"
            );
        }
        let short = parse_request(b"220701,120000,0500024453765740,0100");
        assert_eq!(short.pa_rq_card_num, CardNumber::move_from(FIXTURE_CARD));
        assert_eq!(short.pa_rq_auth_type.to_string(), "0100");
    }

    #[test]
    fn only_the_500_byte_buffer_is_read_and_a_longer_request_fails_the_get() {
        // P-COPAUA0C-002: W01-GET-BUFFER X(500).
        let body = request_text(FIXTURE_CARD, "100.00", "TXN000000000001")
            .replace("FIXTURE MERCHANT", &"N".repeat(450));
        assert!(body.len() > REQUEST_BUFFER_LEN);
        let request = parse_request(body.as_bytes());
        assert_eq!(
            request.fields_received, 14,
            "the buffer ends inside the name"
        );
        assert!(request.pa_rq_merchant_city.is_blank());

        let entry = oversize_request_entry(body.as_bytes()).expect("an oversize request");
        assert_eq!(
            fields(&entry),
            (
                "M003",
                ErrorLevel::Critical,
                ErrorSource::Cics,
                pad("000000001", 9),
                pad("000002080", 9),
                pad("", 20),
            )
        );
        assert_eq!(entry.message, messages::FAILED_TO_READ_REQUEST_MQ);
        let fits = vec![b' '; REQUEST_BUFFER_LEN];
        assert_eq!(oversize_request_entry(&fits), None);
    }

    #[test]
    fn the_get_converts_each_character_to_one_iso_8859_1_byte_and_halts_the_rest() {
        // P-COPAUA0C-002: MQGMO-CONVERT of an MQFMT-STRING request; E-80 for
        // text the conversion cannot pin, even when it lies past the buffer of
        // a message too long for it.
        /// A body and the converted text, or the escalation, it gives.
        type Case<'a> = (&'a [u8], Result<&'a [u8], &'static str>);
        let ascii = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
        let oversize = "N".repeat(1 << 20);
        let euro_past_the_buffer = format!("{oversize}€");
        let invalid_past_the_buffer = [oversize.as_bytes(), b"\xC3"].concat();
        let cases: [Case<'_>; 13] = [
            (ascii.as_bytes(), Ok(ascii.as_bytes())),
            (b"", Ok(b"")),
            ("É".as_bytes(), Ok(b"\xC9")),
            ("Ü".as_bytes(), Ok(b"\xDC")),
            ("CAFÉ MÜNCHEN".as_bytes(), Ok(b"CAF\xC9 M\xDCNCHEN")),
            ("\u{80}\u{FF}".as_bytes(), Ok(b"\x80\xFF")),
            ("€".as_bytes(), Err("E-80")),
            ("CAFÉ MÜNCHEN €".as_bytes(), Err("E-80")),
            ("\u{100}".as_bytes(), Err("E-80")),
            (b"\xC9", Err("E-80")),
            (b"TXN\xC3", Err("E-80")),
            (euro_past_the_buffer.as_bytes(), Err("E-80")),
            (&invalid_past_the_buffer, Err("E-80")),
        ];
        for (body, expected) in cases {
            assert_eq!(
                convert_request(body)
                    .as_deref()
                    .map_err(|unproven| unproven.escalation_id),
                expected,
                "{body:?}"
            );
        }
    }

    #[test]
    fn the_500_byte_buffer_counts_converted_characters() {
        // P-COPAUA0C-002: MQGMO-CONVERT puts the converted text into
        // W01-GET-BUFFER X(500), so 500 'É' (1000 bytes of UTF-8) fit. A
        // longer message, however long, keeps the buffer and the one byte past
        // it that fails the get.
        let kept = REQUEST_BUFFER_LEN + 1;
        for (character, count, oversize) in [
            ('É', REQUEST_BUFFER_LEN, false),
            ('É', REQUEST_BUFFER_LEN + 1, true),
            ('É', 1 << 20, true),
            ('N', REQUEST_BUFFER_LEN, false),
            ('N', REQUEST_BUFFER_LEN + 1, true),
            ('N', 1 << 20, true),
        ] {
            let body = character.to_string().repeat(count);
            let data = convert_request(body.as_bytes()).expect("ISO 8859-1 text");
            assert_eq!(data.len(), count.min(kept), "{character} x {count}");
            assert!(data.capacity() <= kept, "{character} x {count}");
            let entry = oversize_request_entry(&data);
            assert_eq!(
                entry.map(|entry| entry.location),
                oversize.then_some("M003"),
                "{character} x {count}"
            );
        }
    }

    #[test]
    fn a_low_value_in_a_receiver_the_detail_stores_as_text_is_held_at_e81() {
        // P-COPAUA0C-004, P-COPAUA0C-014 (E-81): PostgreSQL text cannot hold
        // X'00'. The amount (E-28) and the numeric receivers (E-47) keep their
        // own cases, and an X'00' cut off at a receiver's width or sent after
        // the 18th field is in no receiver.
        let text = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
        let with_field = |position: usize, field: &str| {
            let mut fields: Vec<&str> = text.split(',').collect();
            fields[position] = field;
            fields.join(",")
        };
        let stored = [0, 1, 2, 3, 4, 5, 6, 9, 10, 12, 13, 14, 15, 16, 17];
        let mut cases: Vec<(String, Result<(), &str>)> = stored
            .into_iter()
            .map(|position| (with_field(position, "\0"), Err("E-81")))
            .collect();
        cases.extend([
            (text.clone(), Ok(())),
            (with_field(7, "\0"), Ok(())),
            (with_field(8, "\0"), Ok(())),
            (with_field(11, "\0"), Ok(())),
            (with_field(13, "FIXTURE MERCHANT 22 CH\0"), Ok(())),
            (with_field(17, "TXN000000000001\0"), Ok(())),
            (format!("{text},\0EXTRA"), Ok(())),
        ]);
        for (body, expected) in cases {
            let request = parse_request(body.as_bytes());
            assert_eq!(
                storable(&request).map_err(|unproven| unproven.escalation_id),
                expected,
                "{body:?}"
            );
        }

        let amount = parse_request(with_field(8, "5.\u{0}0").as_bytes());
        assert_eq!(
            parse_amount(&amount).map_err(|unproven| unproven.escalation_id),
            Err("E-28")
        );
        for position in [7, 11] {
            let body = with_field(position, "0\0");
            let (request, amount, context, decision) = authorize(&mut fixture_store(), &body);
            assert_eq!(storable(&request), Ok(()), "{body:?}");
            assert_eq!(
                write_plan(&request, amount, &context, &decision, at(PINNED))
                    .map_err(|unproven| unproven.escalation_id),
                Err("E-47"),
                "{body:?}"
            );
        }
    }

    #[test]
    fn non_ascii_text_keeps_its_characters_at_the_receiver_widths() {
        // P-COPAUA0C-004, P-COPAUA0C-011, P-COPAUA0C-014 (E-80): X(n) counts
        // characters, so the reply is 64 characters and the detail stores the
        // characters sent; byte 64 is written as a space (E-25).
        let long_name = "Ü".repeat(23);
        let long_transaction = "É".repeat(16);
        let cut_name = "Ü".repeat(22);
        let cut_transaction = "É".repeat(15);
        let cases = [
            ("CAFÉ MÜNCHEN", "TXNÉ", "CAFÉ MÜNCHEN", "TXNÉ           "),
            (
                long_name.as_str(),
                long_transaction.as_str(),
                cut_name.as_str(),
                cut_transaction.as_str(),
            ),
        ];
        for (name, transaction, stored_name, echoed) in cases {
            let body =
                request_text(FIXTURE_CARD, "100.00", transaction).replace("FIXTURE MERCHANT", name);
            let mut store = fixture_store();
            let (request, amount, context, decision) = authorize(&mut store, &body);
            let reply = reply_body(&request, &decision);
            assert_eq!(
                reply,
                format!("{FIXTURE_CARD},{echoed},120000,00,0000,        100.00, ")
            );
            assert_eq!(reply.chars().count(), 64, "{transaction}");
            let plan = write_plan(&request, amount, &context, &decision, at(PINNED))
                .expect("a pinned plan")
                .expect("a plan for a found card");
            assert_eq!(plan.detail.pa_merchant_name.trimmed(), stored_name);
            assert_eq!(plan.detail.pa_transaction_id.trimmed(), echoed.trim_end());
        }
        let body = request_text(FIXTURE_CARD, "100.00", "TXNÉ");
        let (request, _, _, decision) = authorize(&mut fixture_store(), &body);
        let reply = reply_body(&request, &decision).into_bytes();
        assert_eq!(reply.get(17..22), Some(&b"TXN\xC3\x89"[..]));
        assert_eq!(reply.get(22..34), Some(&b"           ,"[..]));
    }

    #[test]
    fn numval_converts_the_audited_input_classes_and_halts_the_rest() {
        // P-COPAUA0C-016, P-COPAUA0C-024, P-COPAUA0C-025; E-28 and E-40.
        let cases: [(&str, Result<i64, &str>); 30] = [
            ("100.00", Ok(10_000)),
            ("100.29", Ok(10_029)),
            ("-50.00", Ok(-5_000)),
            ("10.006", Ok(1_001)),
            ("10.004", Ok(1_000)),
            ("10.0051", Ok(1_001)),
            ("5", Ok(500)),
            ("5.", Ok(500)),
            (".5", Ok(50)),
            ("+ 7.25", Ok(725)),
            (" 7.25 -", Ok(-725)),
            ("7.25+", Ok(725)),
            ("7.25CR", Ok(-725)),
            ("7.25 DB", Ok(-725)),
            ("-0.00", Ok(0)),
            ("1234567890.123", Ok(123_456_789_012)),
            ("999999999.999", Ok(100_000_000_000)),
            ("0.005", Err("E-40")),
            ("2.12500", Err("E-40")),
            ("-0.015", Err("E-40")),
            ("12345678901", Err("E-40")),
            ("", Err("E-28")),
            ("abc", Err("E-28")),
            ("1 2", Err("E-28")),
            ("--5", Err("E-28")),
            ("-5-", Err("E-28")),
            ("7.25cr", Err("E-28")),
            (".", Err("E-28")),
            ("+", Err("E-28")),
            ("1.2.3", Err("E-28")),
        ];
        for (text, expected) in cases {
            let request = fixture_request(text);
            let amount = parse_amount(&request)
                .map(Money::cents)
                .map_err(|unproven| unproven.escalation_id);
            assert_eq!(amount, expected, "{text:?}");
        }
    }

    #[test]
    fn fixture_requests_get_the_audited_replies() {
        // Section D.10: P-COPAUA0C-021 to P-COPAUA0C-025; P-COPAUA0C-011 and
        // P-COPAUA0C-020 for the reply layout, byte 64 written as a space (E-25).
        let head = "0500024453765740,TXN000000000001,120000,";
        let cases = [
            ("100.00", true, "00,0000,        100.00,{UNDEF:1}"),
            ("5677.00", true, "00,0000,       5677.00,{UNDEF:1}"),
            ("5677.01", false, "05,4100,          0.00,{UNDEF:1}"),
            ("-50.00", true, "00,0000,-        50.00,{UNDEF:1}"),
            ("10.006", true, "00,0000,         10.01,{UNDEF:1}"),
        ];
        for (amount, approved, tail) in cases {
            let mut store = fixture_store();
            let body = request_text(FIXTURE_CARD, amount, "TXN000000000001");
            let (request, _, context, decision) = authorize(&mut store, &body);
            assert_eq!(context.log, [], "{amount}");
            assert_eq!(decision.approved, approved, "{amount}");
            let reply = reply_body(&request, &decision);
            assert_eq!(reply, [head, tail].concat().replace("{UNDEF:1}", " "));
            assert_eq!(reply.len(), 64, "{amount}");
            assert_eq!(
                store.calls,
                [
                    "read_xref_by_card",
                    "read_account",
                    "read_customer",
                    "read_summary_for_update"
                ]
            );
        }
    }

    #[test]
    fn an_unknown_card_is_declined_with_3100_and_writes_nothing() {
        // P-COPAUA0C-026 (the E-41 proposal), P-COPAUA0C-006, P-COPAUA0C-035.
        let mut store = fixture_store();
        let body = request_text(UNKNOWN_CARD, "100.00", "TXN000000000002");
        let (request, amount, context, decision) = authorize(&mut store, &body);
        assert_eq!(
            reply_body(&request, &decision),
            "9999999999999999,TXN000000000002,120000,05,3100,          0.00, "
        );
        assert_eq!(store.calls, ["read_xref_by_card"]);
        assert_eq!(context.xref, Lookup::NotFound);
        let [warning] = context.log.as_slice() else {
            panic!("one record: {:?}", context.log);
        };
        assert_eq!(
            fields(warning),
            (
                "A001",
                ErrorLevel::Warning,
                ErrorSource::App,
                pad("", 9),
                pad("", 9),
                pad(UNKNOWN_CARD, 20),
            )
        );
        assert_eq!(warning.message, messages::CARD_NOT_FOUND_IN_XREF);
        assert_eq!(context.critical_entry(), None);
        assert_eq!(
            write_plan(&request, amount, &context, &decision, at(PINNED)),
            Ok(None)
        );
    }

    #[test]
    fn the_unknown_card_decision_is_held_at_e41_and_a_found_card_is_decided() {
        // P-COPAUA0C-028 (E-41 halt), P-COPAUA0C-026: the decision of a card the
        // cross-reference lacks reads a flag no statement set; a found card's
        // decision is pinned (P-COPAUA0C-021).
        let mut unknown = fixture_store();
        let body = request_text(UNKNOWN_CARD, "100.00", "TXN000000000002");
        let (_, _, context, _) = authorize(&mut unknown, &body);
        assert_eq!(
            decidable(&context).map_err(|unproven| unproven.escalation_id),
            Err("E-41")
        );
        let mut found = fixture_store();
        let body = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
        let (_, _, context, _) = authorize(&mut found, &body);
        assert_eq!(decidable(&context), Ok(()));
    }

    #[test]
    fn a_summary_found_decides_the_available_amount() {
        // P-COPAUA0C-009, P-COPAUA0C-017: 1000.00 - 900.00 leaves 100.00,
        // whatever the account would allow.
        for (amount, approved, code, reason) in [
            ("100.00", true, "00", "0000"),
            ("100.01", false, "05", "4100"),
        ] {
            let mut store = fixture_store().with_summary(summary(100_000, 90_000));
            let body = request_text(FIXTURE_CARD, amount, "TXN000000000001");
            let (_, _, context, decision) = authorize(&mut store, &body);
            assert!(matches!(context.summary, Lookup::Found(_)));
            assert_eq!(decision.approved, approved, "{amount}");
            assert_eq!(decision.resp_code.to_string(), code, "{amount}");
            assert_eq!(decision.reason.to_string(), reason, "{amount}");
        }
    }

    #[test]
    fn available_amounts_and_summary_limits_drop_high_order_digits() {
        // P-COPAUA0C-018, P-COPAUA0C-019: 1500000000.00 into S9(09)V99.
        let wide = Account {
            acct_credit_limit: cents(150_000_000_000),
            acct_cash_credit_limit: cents(150_000_000_000),
            acct_curr_bal: Money::ZERO,
            ..fixture_account()
        };
        for (amount, approved) in [("500000000.00", true), ("500000000.01", false)] {
            let mut store = fixture_store().with_account(wide.clone());
            let body = request_text(FIXTURE_CARD, amount, "TXN000000000001");
            let (_, _, _, decision) = authorize(&mut store, &body);
            assert_eq!(decision.approved, approved, "{amount}");
        }
        let mut store = fixture_store().with_account(wide);
        let SummaryWrite::Insert(inserted) = approved_plan(&mut store, "100.00").summary else {
            panic!("no summary existed");
        };
        assert_eq!(inserted.pa_credit_limit, cents::<9>(50_000_000_000));
        assert_eq!(inserted.pa_cash_limit, cents::<9>(50_000_000_000));
    }

    #[test]
    fn missing_records_warn_and_decline_with_3100_first() {
        // P-COPAUA0C-007, P-COPAUA0C-010: not-found reasons are tested before
        // insufficient funds, and a missing customer declines nothing alone.
        let no_account = FakeStore::default()
            .with_xref(fixture_xref())
            .with_customer(fixture_customer());
        let no_customer = FakeStore::default()
            .with_xref(fixture_xref())
            .with_account(fixture_account());
        let a002 = ("A002", messages::ACCT_NOT_FOUND_IN_XREF, "00000000050");
        let a003 = ("A003", messages::CUST_NOT_FOUND_IN_XREF, "000000050");
        let cases = [
            (no_account, "100.00", a002, "05", "3100"),
            (no_customer.clone(), "100.00", a003, "00", "0000"),
            (no_customer, "6000.00", a003, "05", "3100"),
        ];
        for (mut store, amount, (location, message, key), code, reason) in cases {
            let body = request_text(FIXTURE_CARD, amount, "TXN000000000001");
            let (_, _, context, decision) = authorize(&mut store, &body);
            let [warning] = context.log.as_slice() else {
                panic!("one record: {:?}", context.log);
            };
            assert_eq!(
                fields(warning),
                (
                    location,
                    ErrorLevel::Warning,
                    ErrorSource::App,
                    pad("", 9),
                    pad("", 9),
                    pad(key, 20),
                )
            );
            assert_eq!(warning.message, message);
            assert_eq!(store.calls.len(), 4, "the reads go on after a warning");
            assert_eq!(decision.resp_code.to_string(), code, "{location} {amount}");
            assert_eq!(decision.reason.to_string(), reason, "{location} {amount}");
        }
    }

    #[test]
    fn read_failures_raise_critical_records_and_end_the_reads() {
        // P-COPAUA0C-006 to P-COPAUA0C-008; codes per E-44.
        let cases = [
            (
                "read_xref_by_card",
                "C001",
                ErrorSource::Cics,
                FIXTURE_CARD,
                1,
            ),
            ("read_account", "C002", ErrorSource::Cics, "00000000050", 2),
            ("read_customer", "C003", ErrorSource::Cics, "000000050", 3),
            ("read_summary_for_update", "I002", ErrorSource::Ims, "", 4),
        ];
        let texts = [
            messages::FAILED_TO_READ_XREF_FILE,
            messages::FAILED_TO_READ_ACCT_FILE,
            messages::FAILED_TO_READ_CUST_FILE,
            messages::IMS_GET_SUMMARY_FAILED,
        ];
        for ((method, location, source, key, reads), message) in cases.into_iter().zip(texts) {
            let mut store = fixture_store().with_summary(summary(100_000, 0));
            store.fail_next(method, PortError::Unavailable("store down".to_owned()));
            let body = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
            let (request, amount, context, decision) = authorize(&mut store, &body);
            assert_eq!(store.calls.len(), reads, "{method}");
            let [entry] = context.log.as_slice() else {
                panic!("one record: {:?}", context.log);
            };
            let (code_1, code_2) = match source {
                ErrorSource::Ims => (pad("AO", 9), pad("", 9)),
                _ => ("000000017".to_owned(), "000000000".to_owned()),
            };
            assert_eq!(
                fields(entry),
                (
                    location,
                    ErrorLevel::Critical,
                    source,
                    code_1,
                    code_2,
                    pad(key, 20)
                )
            );
            assert_eq!(entry.message, message);
            assert_eq!(context.critical_entry(), Some(entry));
            assert_eq!(
                write_plan(&request, amount, &context, &decision, at(PINNED)),
                Ok(None),
                "{method}"
            );
        }
        let not_found =
            xref_read_failure_entry(&CardNumber::move_from(FIXTURE_CARD), &PortError::NotFound);
        assert_eq!(
            not_found.code_1.to_string(),
            "000000013",
            "NOTFND RESP (E-44)"
        );
        assert_eq!(
            not_found.code_2.to_string(),
            "000000080",
            "NOTFND RESP2 (E-44)"
        );
    }

    #[test]
    fn an_approval_without_a_summary_inserts_one_and_its_detail() {
        // P-COPAUA0C-013, P-COPAUA0C-014, P-COPAUA0C-019, P-COPAUA0C-030.
        let plan = approved_plan(&mut fixture_store(), "100.00");
        let expected = PendingAuthSummary {
            pa_acct_id: AccountId::new(50).expect("an account number"),
            pa_cust_id: CustomerId::new(50).expect("a customer number"),
            pa_auth_status: Alnum::spaces(),
            pa_account_status: [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS],
            pa_credit_limit: cents(616_900),
            pa_cash_limit: cents(458_700),
            pa_credit_balance: cents(10_000),
            pa_cash_balance: Money::ZERO,
            pa_approved_auth_cnt: AuthCount::new(1).expect("a count"),
            pa_declined_auth_cnt: AuthCount::ZERO,
            pa_approved_auth_amt: cents(10_000),
            pa_declined_auth_amt: Money::ZERO,
        };
        assert_eq!(plan.summary, SummaryWrite::Insert(expected));

        let detail = &plan.detail;
        assert_eq!(
            (detail.pa_auth_date_9c, detail.pa_auth_time_9c),
            (77_817, 879_999_999)
        );
        assert_eq!(detail.pa_acct_id, AccountId::new(50).expect("an account"));
        assert_eq!(detail.pa_card_num, CardNumber::move_from(FIXTURE_CARD));
        let texts = [
            detail.pa_auth_orig_date.to_string(),
            detail.pa_auth_orig_time.to_string(),
            detail.pa_auth_type.to_string(),
            detail.pa_card_expiry_date.to_string(),
            detail.pa_message_type.to_string(),
            detail.pa_message_source.to_string(),
            detail.pa_auth_id_code.to_string(),
            detail.pa_auth_resp_code.to_string(),
            detail.pa_auth_resp_reason.to_string(),
            detail.pa_merchant_catagory_code.to_string(),
            detail.pa_acqr_country_code.to_string(),
            detail.pa_merchant_id.to_string(),
            detail.pa_merchant_name.trimmed().into_owned(),
            detail.pa_merchant_city.trimmed().into_owned(),
            detail.pa_merchant_state.to_string(),
            detail.pa_merchant_zip.trimmed().into_owned(),
            detail.pa_transaction_id.to_string(),
            detail.pa_fraud_rpt_date.to_string(),
        ];
        let expected_texts = [
            "220701",
            "120000",
            "0100",
            "1225",
            "0100  ",
            "POS   ",
            "120000",
            "00",
            "0000",
            "5411",
            "840",
            "M00000000000001",
            "FIXTURE MERCHANT",
            "SEATTLE",
            "WA",
            "98101",
            "TXN000000000001",
            "        ",
        ];
        assert_eq!(texts, expected_texts);
        assert_eq!(
            detail.pa_processing_code,
            NumericCode::new(0).expect("a code")
        );
        assert_eq!(
            detail.pa_pos_entry_mode,
            NumericCode::new(1).expect("a code")
        );
        assert_eq!(detail.pa_transaction_amt, cents::<10>(10_000));
        assert_eq!(detail.pa_approved_amt, cents::<10>(10_000));
        assert_eq!(detail.pa_match_status, MatchStatus::Pending);
        assert_eq!(detail.pa_auth_fraud, FraudFlag::Blank);
        assert!(detail.is_approved());
    }

    #[test]
    fn an_approval_with_a_summary_replaces_it_with_updated_totals() {
        // P-COPAUA0C-013, P-COPAUA0C-019: limits refreshed from the account,
        // approved count, total and credit balance increased, cash balance
        // zeroed, the declined totals and the status bytes kept.
        let mut store = fixture_store().with_summary(summary(100_000, 20_000));
        let plan = approved_plan(&mut store, "300.00");
        let expected = PendingAuthSummary {
            pa_credit_limit: cents(616_900),
            pa_cash_limit: cents(458_700),
            pa_credit_balance: cents(50_000),
            pa_cash_balance: Money::ZERO,
            pa_approved_auth_cnt: AuthCount::new(3).expect("a count"),
            pa_approved_auth_amt: cents(50_000),
            ..summary(100_000, 20_000)
        };
        assert_eq!(plan.summary, SummaryWrite::Replace(expected));
    }

    #[test]
    fn the_write_unit_derives_the_summary_write_again_on_the_summary_it_locked() {
        // P-COPAUA0C-008, P-COPAUA0C-013, P-COPAUA0C-019, E-78: 5500 decided
        // 300.00 on a summary of 2 approvals; under the write unit's lock the
        // summary holds 4, or a purge deleted it, and 8400 applies to that.
        let mut store = fixture_store().with_summary(summary(100_000, 20_000));
        let body = request_text(FIXTURE_CARD, "300.00", "TXN000000000001");
        let (request, amount, context, decision) = authorize(&mut store, &body);
        let plan = write_plan(&request, amount, &context, &decision, at(PINNED))
            .expect("a pinned plan")
            .expect("a plan for a found card");
        let SummaryWrite::Replace(stale) = &plan.summary else {
            panic!("5500 found a summary: {plan:?}");
        };
        assert_eq!(
            stale.pa_approved_auth_cnt.get(),
            3,
            "the plan of the 5500 read"
        );

        let fresh = PendingAuthSummary {
            pa_approved_auth_cnt: AuthCount::new(4).expect("a count"),
            pa_approved_auth_amt: cents(40_000),
            ..summary(100_000, 40_000)
        };
        let replaced = PendingAuthSummary {
            pa_credit_limit: cents(616_900),
            pa_cash_limit: cents(458_700),
            pa_credit_balance: cents(70_000),
            pa_cash_balance: Money::ZERO,
            pa_approved_auth_cnt: AuthCount::new(5).expect("a count"),
            pa_approved_auth_amt: cents(70_000),
            ..fresh.clone()
        };
        let inserted = PendingAuthSummary {
            pa_acct_id: AccountId::new(50).expect("an account number"),
            pa_cust_id: CustomerId::new(50).expect("a customer number"),
            pa_auth_status: Alnum::spaces(),
            pa_account_status: [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS],
            pa_credit_limit: cents(616_900),
            pa_cash_limit: cents(458_700),
            pa_credit_balance: cents(30_000),
            pa_cash_balance: Money::ZERO,
            pa_approved_auth_cnt: AuthCount::new(1).expect("a count"),
            pa_declined_auth_cnt: AuthCount::ZERO,
            pa_approved_auth_amt: cents(30_000),
            pa_declined_auth_amt: Money::ZERO,
        };
        let at_9999 = PendingAuthSummary {
            pa_approved_auth_cnt: AuthCount::new(9_999).expect("a count"),
            ..fresh.clone()
        };
        let cases = [
            (
                "found",
                Some(&fresh),
                Ok(Some(SummaryWrite::Replace(replaced))),
            ),
            ("not found", None, Ok(Some(SummaryWrite::Insert(inserted)))),
            ("9999 approvals", Some(&at_9999), Err("E-78")),
        ];
        for (label, locked, expected) in cases {
            assert_eq!(
                locked_summary_write(&context, &decision, locked)
                    .map_err(|unproven| unproven.escalation_id),
                expected,
                "{label}"
            );
        }
    }

    #[test]
    fn the_locked_summary_write_halts_and_plans_nothing_where_the_plan_does() {
        // P-COPAUA0C-028 (E-41), E-25, P-COPAUA0C-008 (I002), P-COPAUA0C-026:
        // whatever the lock finds, the write unit writes a summary only for
        // the requests write_plan plans one for.
        let no_account = FakeStore::default()
            .with_xref(fixture_xref())
            .with_customer(fixture_customer())
            .with_summary(summary(100_000, 0));
        let mut failing = fixture_store();
        failing.fail_next(
            "read_summary_for_update",
            PortError::Unavailable("store down".to_owned()),
        );
        let found = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
        let cases = [
            (
                fixture_store(),
                request_text(FIXTURE_CARD, "5677.01", "TXN000000000001"),
                Err("E-41"),
            ),
            (no_account, found.clone(), Err("E-25")),
            (failing, found, Ok(None)),
            (
                fixture_store(),
                request_text(UNKNOWN_CARD, "100.00", "TXN000000000002"),
                Ok(None),
            ),
        ];
        for (mut store, body, expected) in cases {
            let (request, amount, context, decision) = authorize(&mut store, &body);
            let planned = write_plan(&request, amount, &context, &decision, at(PINNED))
                .map(|plan| plan.map(|_| ()))
                .map_err(|unproven| unproven.escalation_id);
            assert_eq!(planned, expected, "{body}");
            for locked in [None, Some(&summary(100_000, 0))] {
                assert_eq!(
                    locked_summary_write(&context, &decision, locked)
                        .map(|write| write.map(|_| ()))
                        .map_err(|unproven| unproven.escalation_id),
                    expected,
                    "{body} with {locked:?}"
                );
            }
        }
    }

    #[test]
    fn write_plans_the_audit_cannot_pin_are_unproven() {
        // P-COPAUA0C-028 (E-41), E-25, E-47, E-78; the reply stands in every case.
        let full = |processing: &str| {
            request_text(FIXTURE_CARD, "100.00", "TXN000000000001")
                .replace(",000000,", &format!(",{processing},"))
        };
        let at_9999 = PendingAuthSummary {
            pa_approved_auth_cnt: AuthCount::new(9_999).expect("a count"),
            ..summary(100_000, 0)
        };
        let no_account = FakeStore::default()
            .with_xref(fixture_xref())
            .with_customer(fixture_customer())
            .with_summary(summary(100_000, 0));
        let cases = [
            (
                fixture_store(),
                request_text(FIXTURE_CARD, "5677.01", "TXN000000000001"),
                "E-41",
            ),
            (no_account, full("000000"), "E-25"),
            (
                fixture_store().with_summary(at_9999),
                full("000000"),
                "E-78",
            ),
            (fixture_store(), full("00A000"), "E-47"),
            (
                fixture_store(),
                "220701,120000,0500024453765740,0100,1225,0100,POS,000000,100.00,5411,840"
                    .to_owned(),
                "E-25",
            ),
        ];
        for (mut store, body, escalation) in cases {
            let (request, amount, context, decision) = authorize(&mut store, &body);
            assert_eq!(context.critical_entry(), None, "{escalation}");
            let plan = write_plan(&request, amount, &context, &decision, at(PINNED));
            assert_eq!(
                plan.map_err(|unproven| unproven.escalation_id),
                Err(escalation),
                "{body}"
            );
        }
    }

    #[test]
    fn error_records_fill_the_ccpauery_fields_their_paragraphs_move() {
        // P-COPAUA0C-002, P-COPAUA0C-012 to P-COPAUA0C-015, P-COPAUA0C-035;
        // never-set event keys and codes are spaces (E-25), IMS statuses per
        // E-44.
        let plan = approved_plan(&mut fixture_store(), "100.00");
        let blank_key = pad("", 20);
        let card_key = pad(FIXTURE_CARD, 20);
        let unavailable = PortError::Unavailable("store down".to_owned());
        let cases = [
            (
                reply_failure_entry(2, 2_053),
                (
                    "M004",
                    ErrorSource::Mq,
                    "000000002",
                    "000002053",
                    &blank_key,
                ),
                messages::FAILED_TO_PUT_ON_REPLY_MQ,
            ),
            (
                reply_failure_entry(-2, -2_053),
                (
                    "M004",
                    ErrorSource::Mq,
                    "000000002",
                    "000002053",
                    &blank_key,
                ),
                messages::FAILED_TO_PUT_ON_REPLY_MQ,
            ),
            (
                request_read_failure_entry(2, 2_009),
                (
                    "M003",
                    ErrorSource::Cics,
                    "000000002",
                    "000002009",
                    &blank_key,
                ),
                messages::FAILED_TO_READ_REQUEST_MQ,
            ),
            (
                write_failure_entry(WriteStep::Summary, &PortError::Duplicate, &plan),
                ("I003", ErrorSource::Ims, "II", "", &blank_key),
                messages::IMS_UPDATE_SUMRY_FAILED,
            ),
            (
                write_failure_entry(WriteStep::Summary, &PortError::NotFound, &plan),
                ("I003", ErrorSource::Ims, "GE", "", &blank_key),
                messages::IMS_UPDATE_SUMRY_FAILED,
            ),
            (
                write_failure_entry(WriteStep::Detail, &unavailable, &plan),
                ("I004", ErrorSource::Ims, "AO", "", &card_key),
                messages::IMS_INSERT_DETL_FAILED,
            ),
        ];
        for (entry, (location, source, code_1, code_2, key), message) in cases {
            assert_eq!(
                fields(&entry),
                (
                    location,
                    ErrorLevel::Critical,
                    source,
                    pad(code_1, 9),
                    pad(code_2, 9),
                    key.clone(),
                ),
                "{location}"
            );
            assert_eq!(entry.message, message, "{location}");
        }
        assert_eq!((ERR_APPLICATION, ERR_PROGRAM), ("CP00", "COPAUA0C"));
        assert_eq!(
            [ErrorLevel::Warning, ErrorLevel::Critical].map(ErrorLevel::code),
            *b"WC"
        );
        let sources = [
            ErrorSource::App,
            ErrorSource::Cics,
            ErrorSource::Ims,
            ErrorSource::Mq,
        ];
        assert_eq!(sources.map(ErrorSource::code), *b"ACIM");
    }

    #[test]
    fn the_detail_key_complements_the_processing_instant() {
        // P-COPAUA0C-030: later instants give smaller keys, so the newest
        // detail sorts first.
        let cases = [
            ("2022-07-01T12:00:00Z", (77_817, 879_999_999)),
            ("2022-07-01T12:00:00.123Z", (77_817, 879_999_876)),
            ("2022-12-31T23:59:59.999Z", (77_634, 764_040_000)),
            ("2023-01-01T00:00:00Z", (76_998, 999_999_999)),
        ];
        for (instant, key) in cases {
            assert_eq!(super::detail_key(at(instant)), key, "{instant}");
        }
    }

    #[test]
    fn a_declined_detail_would_carry_the_declined_match_status() {
        // P-COPAUA0C-014 [COPAUA0C.cbl:L902-906]: the detail builder keeps the
        // declined branch, which write_plan withholds while E-41 is at halt.
        let request = fixture_request("5677.01");
        let declined = AuthDecision {
            approved: false,
            resp_code: Alnum::move_from("05"),
            reason: Alnum::move_from("4100"),
            approved_amount: Money::ZERO,
        };
        let detail = super::new_detail(
            &request,
            cents(567_701),
            &fixture_xref(),
            &declined,
            at(PINNED),
        )
        .expect("numeric request fields");
        assert_eq!(detail.pa_match_status, MatchStatus::AuthDeclined);
        assert_eq!(detail.pa_approved_amt, Money::<10>::ZERO);
        assert_eq!(detail.pa_transaction_amt, cents::<10>(567_701));
        assert!(!detail.is_approved());
    }
}
