//! The purge of expired pending authorizations: `CBPAUP0C`
//! [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl], the IMS BMP program of
//! the job `CBPAUP0J` [app/app-authorization-ims-db2-mq/jcl/CBPAUP0J.jcl:L24-37],
//! replaced by `batch cbpaup0j`.
//!
//! The program sweeps every `PAUTSUM0` summary in account order. Under each
//! one it deletes every `PAUTDTL1` detail whose authorization date is at least
//! the expiry days old, decrementing the summary's approved or declined count
//! and amount in memory for each, and then deletes the summary when its
//! approved count is at most zero. [`purge`] is that sweep over
//! [`AuthorizationStore`], [`parse_sysin`] reads its `SYSIN` parameters and
//! [`is_expired`] is its expiry test.
//!
//! Two reproduced COBOL behaviours shape the result:
//!
//! - The summary-delete condition names the approved count twice and never
//!   tests the declined count (E-16, P-CBPAUP0C-006, P-CBPAUP0C-010).
//! - The decrements change only the summary held in memory: the program never
//!   replaces the summary, so a kept summary keeps its stored counts and
//!   amounts (E-35, P-CBPAUP0C-004, P-CBPAUP0C-011).
//!
//! What the port leaves to the adapter or drops:
//!
//! - `ACCEPT CURRENT-YYDDD FROM DAY` is the `today_yyddd` argument, which the
//!   job takes from `crate::clock::yyddd` (P-CBPAUP0C-012).
//! - No checkpoint is taken here. `9000-TAKE-CHECKPOINT` commits the deletes
//!   so far (P-CBPAUP0C-008), and so do the normal end and `9999-ABEND`, whose
//!   `GOBACK` ends the program normally (P-CBPAUP0C-005, P-CBPAUP0C-009). Every
//!   delete the rule completes therefore persists, wherever the checkpoints
//!   fall, and the adapter commits them once, after the rule returns or
//!   abends. The store has no checkpoint verb that could fail.
//! - Of the `DISPLAY` lines, only the two texts the audit records are
//!   rendered, by [`AbendCause`]'s `Display`. The `PARM RECEIVED` line shows
//!   [`prm_info`], and the counts the end of job displays are
//!   [`PurgeCounts`]; the adapter logs both.

use std::error::Error;
use std::fmt;
use std::ops::Range;

use crate::entities::{PendingAuthDetail, PendingAuthSummary};
use crate::messages::{AUTH_SUMMARY_READ_FAILED, CBPAUP0C_ABENDING};
use crate::numeric::{AccountId, AuthCount, AuthId, Overflow};
use crate::ports::{AuthorizationStore, Browse, Direction, PortError};

/// The fixed-width text the CBPAUP0J job keeps [`prm_info`] in, re-exported
/// from [`crate::numeric`] for the batch adapter.
pub use crate::numeric::Alnum;

/// `PRM-INFO` [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L98-108]: the
/// 17 bytes `ACCEPT PRM-INFO FROM SYSIN` fills.
pub const PRM_INFO_LEN: usize = 17;

/// `P-EXPIRY-DAYS PIC 9(02)`, positions 1-2.
const P_EXPIRY_DAYS: Range<usize> = 0..2;

/// `P-CHKP-FREQ PIC X(05)`, positions 4-8.
const P_CHKP_FREQ: Range<usize> = 3..8;

/// `P-CHKP-DIS-FREQ PIC X(05)`, positions 10-14.
const P_CHKP_DIS_FREQ: Range<usize> = 9..14;

/// `P-DEBUG-FLAG PIC X(01)`, position 16.
const P_DEBUG_FLAG: usize = 15;

/// `MOVE 5 TO WS-EXPIRY-DAYS` when `P-EXPIRY-DAYS` is not numeric.
const DEFAULT_EXPIRY_DAYS: u32 = 5;

/// `MOVE 5 TO P-CHKP-FREQ`.
const DEFAULT_CHKP_FREQ: u32 = 5;

/// `MOVE 10 TO P-CHKP-DIS-FREQ`.
const DEFAULT_CHKP_DIS_FREQ: u32 = 10;

/// The numeric literal `0` of `IF P-CHKP-FREQ = SPACES OR 0 OR LOW-VALUES` as
/// the `X(05)` comparison sees it: an integer literal compared with an
/// alphanumeric item is compared as the text of its one digit, padded with
/// spaces. `00000` is therefore not equal to it.
const ZERO_LITERAL_FIELD: &[u8] = b"0    ";

/// The integer digits a `9(05)` YYDDD field keeps: `CURRENT-YYDDD` and
/// `WS-AUTH-DATE`.
const YYDDD_MODULUS: i64 = 100_000;

/// The complement `COPAUA0C` stores the authorization date under and
/// `4000-CHECK-IF-EXPIRED` takes it back from:
/// `WS-AUTH-DATE = 99999 - PA-AUTH-DATE-9C`.
const AUTH_DATE_COMPLEMENT: i64 = 99_999;

/// The largest magnitude `WS-DAY-DIFF PIC S9(4) COMP` holds whatever the
/// `TRUNC` compiler option.
const DAY_DIFF_LIMIT: i64 = 9_999;

/// The summaries one `GN` sweep step reads.
const SUMMARY_PAGE: u16 = 100;

/// The details of one summary processed per read; one more is read as the
/// start of the next page, so the page start is never a detail already
/// deleted.
const DETAIL_PAGE: u16 = 100;

/// The rows one detail read asks for: a page and its look-ahead row.
const DETAIL_READ_LIMIT: u16 = DETAIL_PAGE + 1;

/// The `DIBSTAT` a failed store call shows: a `PostgreSQL` failure has no IMS
/// status, and E-44 proposes `AO` for every IMS status slot it fills.
const STORE_FAILURE_DIBSTAT: &str = "AO";

/// The parameters of one purge run: `PRM-INFO` after `1000-INITIALIZE`
/// applies its defaults [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L196-209].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PurgeParams {
    /// `WS-EXPIRY-DAYS`: a detail expires when its day difference is at
    /// least this. 0 to 99 from `P-EXPIRY-DAYS`, or 5.
    pub expiry_days: u32,
    /// `P-CHKP-FREQ`, the summaries between checkpoints. Reported only: the
    /// port takes no checkpoint (P-CBPAUP0C-008).
    pub chkp_freq: u32,
    /// `P-CHKP-DIS-FREQ`, the checkpoints between `CHKP SUCCESS` displays.
    /// Reported only, as [`PurgeParams::chkp_freq`].
    pub chkp_dis_freq: u32,
    /// `DEBUG-ON`: `P-DEBUG-FLAG` is exactly `Y`. It switches on only debug
    /// `DISPLAY` lines, none of which the audit records.
    pub debug: bool,
}

/// CBPAUP0C 1000-INITIALIZE — P-CBPAUP0C-001.
///
/// `ACCEPT PRM-INFO FROM SYSIN` and the defaults that follow it
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L189-209]. `text` is the
/// `SYSIN` data: its first line is the record, of which the first 17 bytes
/// fill `PRM-INFO` and a shorter record is padded with spaces. The `CBPAUP0J`
/// record `00,00001,00001,Y` gives expiry 0, frequencies 1 and 1, debug on.
///
/// - `P-EXPIRY-DAYS` (positions 1-2) is used when both bytes are digits, else
///   5.
/// - `P-CHKP-FREQ` (4-8) and `P-CHKP-DIS-FREQ` (10-14) become 5 and 10 when
///   the field is all spaces, all `LOW-VALUES` or `0` followed by spaces.
///   Otherwise the value is the field's digits before its trailing spaces.
/// - `P-DEBUG-FLAG` (16) other than `Y` is `N`.
///
/// A frequency field holding other text, which the COBOL keeps as text and
/// only ever compares alphanumerically, is given its default value: the
/// frequencies drive nothing in the port, which takes no checkpoint, so the
/// value is only reported.
#[must_use]
pub fn parse_sysin(text: &str) -> PurgeParams {
    let record = prm_info(text);
    PurgeParams {
        expiry_days: digits_value(&record[P_EXPIRY_DAYS]).unwrap_or(DEFAULT_EXPIRY_DAYS),
        chkp_freq: frequency(&record[P_CHKP_FREQ], DEFAULT_CHKP_FREQ),
        chkp_dis_freq: frequency(&record[P_CHKP_DIS_FREQ], DEFAULT_CHKP_DIS_FREQ),
        debug: record[P_DEBUG_FLAG] == b'Y',
    }
}

/// CBPAUP0C 1000-INITIALIZE — P-CBPAUP0C-001.
///
/// The `PRM-INFO` bytes `ACCEPT PRM-INFO FROM SYSIN` fills from the first
/// line of `text` [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L189]:
/// cut at 17 bytes, padded with spaces. The line ends at the first `LF`, and
/// a `CR` just before it is not part of it. These are the bytes
/// [`parse_sysin`] reads and `DISPLAY 'CBPAUP0C PARM RECEIVED :' PRM-INFO`
/// shows [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L192]. The cut is
/// a byte cut, so it may split a multi-byte UTF-8 character.
#[must_use]
pub fn prm_info(text: &str) -> [u8; PRM_INFO_LEN] {
    let mut record = [b' '; PRM_INFO_LEN];
    let line = text.lines().next().unwrap_or_default();
    for (slot, byte) in record.iter_mut().zip(line.bytes()) {
        *slot = byte;
    }
    record
}

/// The unsigned number the ASCII digits of `field` spell, `None` when a byte
/// is not a digit. Fields here hold at most five digits, so the value fits.
fn digits_value(field: &[u8]) -> Option<u32> {
    field.iter().try_fold(0_u32, |value, &byte| {
        byte.is_ascii_digit()
            .then(|| value * 10 + u32::from(byte - b'0'))
    })
}

/// CBPAUP0C 1000-INITIALIZE — P-CBPAUP0C-001.
///
/// A checkpoint frequency field after its `1000-INITIALIZE` default
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L201-206].
fn frequency(field: &[u8], default: u32) -> u32 {
    let defaulted = field.iter().all(|&byte| byte == b' ')
        || field == ZERO_LITERAL_FIELD
        || field.iter().all(|&byte| byte == 0);
    if defaulted {
        return default;
    }
    let end = field
        .iter()
        .rposition(|&byte| byte != b' ')
        .map_or(0, |last| last + 1);
    digits_value(&field[..end]).unwrap_or(default)
}

/// CBPAUP0C 4000-CHECK-IF-EXPIRED — P-CBPAUP0C-004.
///
/// `WS-DAY-DIFF`: `CURRENT-YYDDD` minus the detail's authorization date
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L280-282]. Both are
/// YYDDD numbers, so the difference counts days only within one year: from
/// 22365 to 23001 it is 636.
///
/// The date is `WS-AUTH-DATE PIC 9(05) = 99999 - PA-AUTH-DATE-9C`; the
/// unsigned receiver keeps the magnitude and the low-order five digits, and
/// `today_yyddd` is the `9(05)` `CURRENT-YYDDD`.
///
/// # Errors
///
/// [`Overflow`] when the difference exceeds 9999 in magnitude. `WS-DAY-DIFF`
/// is `S9(4) COMP`, the `COMPUTE` has no `ON SIZE ERROR`, and whether the
/// binary field then keeps the value or its low-order four digits depends on
/// the `TRUNC` compiler option, which the repository does not fix. The size
/// error is reported rather than guessed, as `AuthCount` reports its own.
fn day_difference(detail: &PendingAuthDetail, today_yyddd: u32) -> Result<i16, Overflow> {
    let auth_date =
        (AUTH_DATE_COMPLEMENT - i64::from(detail.pa_auth_date_9c)).abs() % YYDDD_MODULUS;
    let today = i64::from(today_yyddd) % YYDDD_MODULUS;
    let difference = today - auth_date;
    if difference.abs() > DAY_DIFF_LIMIT {
        return Err(Overflow);
    }
    i16::try_from(difference).map_err(|_| Overflow)
}

/// CBPAUP0C 4000-CHECK-IF-EXPIRED — P-CBPAUP0C-004.
///
/// `IF WS-DAY-DIFF >= WS-EXPIRY-DAYS`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L284]: whether the
/// detail qualifies for deletion on the day `today_yyddd`. The difference is
/// signed, so a detail dated after `today_yyddd` never qualifies, and an
/// expiry of 0 qualifies every detail dated that day or earlier.
///
/// # Errors
///
/// [`Overflow`] when the day difference does not fit `WS-DAY-DIFF`, as the
/// day-difference rule of this paragraph documents: more than 9999 in
/// magnitude.
pub fn is_expired(
    detail: &PendingAuthDetail,
    today_yyddd: u32,
    expiry_days: u32,
) -> Result<bool, Overflow> {
    let difference = day_difference(detail, today_yyddd)?;
    Ok(i64::from(difference) >= i64::from(expiry_days))
}

/// CBPAUP0C 4000-CHECK-IF-EXPIRED — P-CBPAUP0C-004, P-CBPAUP0C-011.
///
/// `QUALIFIED-FOR-DELETE` for `detail`, and the decrements of the qualified
/// detail [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L284-296]. An
/// approval (response code `00`) subtracts 1 from the approved count and
/// `PA-APPROVED-AMT` from the approved amount; any other detail subtracts 1
/// from the declined count and `PA-TRANSACTION-AMT` from the declined amount.
/// The amounts are stored into their `S9(09)V99` fields without `ROUNDED`.
///
/// Only `summary`, the copy held in memory, changes (E-35).
///
/// # Errors
///
/// [`AbendCause::SizeError`] when the day difference or the decremented count
/// does not fit its field.
fn check_if_expired(
    summary: &mut PendingAuthSummary,
    detail: &PendingAuthDetail,
    today_yyddd: u32,
    expiry_days: u32,
) -> Result<bool, AbendCause> {
    let account = summary.pa_acct_id;
    let size_error = |field: &'static str, source: Overflow| AbendCause::SizeError {
        account,
        auth_id: detail.auth_id,
        field,
        source,
    };
    let qualified = is_expired(detail, today_yyddd, expiry_days)
        .map_err(|source| size_error("WS-DAY-DIFF", source))?;
    if !qualified {
        return Ok(false);
    }
    if detail.is_approved() {
        summary.pa_approved_auth_cnt = summary
            .pa_approved_auth_cnt
            .checked_decrement()
            .map_err(|source| size_error("PA-APPROVED-AUTH-CNT", source))?;
        summary.pa_approved_auth_amt =
            (summary.pa_approved_auth_amt - detail.pa_approved_amt).store::<9>();
    } else {
        summary.pa_declined_auth_cnt = summary
            .pa_declined_auth_cnt
            .checked_decrement()
            .map_err(|source| size_error("PA-DECLINED-AUTH-CNT", source))?;
        summary.pa_declined_auth_amt =
            (summary.pa_declined_auth_amt - detail.pa_transaction_amt).store::<9>();
    }
    Ok(true)
}

/// The `WS-NO-SUMRY-READ`, `WS-NO-SUMRY-DELETED`, `WS-NO-DTL-READ` and
/// `WS-NO-DTL-DELETED` counts of a run
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L54-57], which the
/// normal end displays [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L173-176].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PurgeCounts {
    /// Summaries read by the `GN` sweep (P-CBPAUP0C-002).
    pub summaries_read: u64,
    /// Summaries deleted (P-CBPAUP0C-007).
    pub summaries_deleted: u64,
    /// Details read by the `GNP` loops (P-CBPAUP0C-003). A detail deleted
    /// with its summary was never read and is not counted.
    pub details_read: u64,
    /// Details deleted one by one (P-CBPAUP0C-005). Details deleted with
    /// their summary are not counted.
    pub details_deleted: u64,
}

/// Why a purge run stopped before the end of the summaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbendCause {
    /// CBPAUP0C 2000-FIND-NEXT-AUTH-SUMMARY — P-CBPAUP0C-002: the `GN` of the
    /// next summary failed.
    SummaryRead(PortError),
    /// CBPAUP0C 3000-FIND-NEXT-AUTH-DTL — P-CBPAUP0C-003: the `GNP` of the
    /// next detail of `account` failed.
    DetailRead {
        /// The summary whose details were being read.
        account: AccountId,
        /// The store failure.
        error: PortError,
    },
    /// CBPAUP0C 5000-DELETE-AUTH-DTL — P-CBPAUP0C-005: the `DLET` of an
    /// expired detail failed.
    DetailDelete {
        /// The summary of the detail.
        account: AccountId,
        /// The detail whose delete failed.
        auth_id: AuthId,
        /// The store failure.
        error: PortError,
    },
    /// CBPAUP0C 6000-DELETE-AUTH-SUMMARY — P-CBPAUP0C-007: the `DLET` of a
    /// qualified summary failed.
    SummaryDelete {
        /// The summary whose delete failed.
        account: AccountId,
        /// The store failure.
        error: PortError,
    },
    /// A `WS-DAY-DIFF` or count result of `4000-CHECK-IF-EXPIRED` that does
    /// not fit its `S9(4) COMP` field. The COBOL goes on with a value the
    /// unfixed `TRUNC` compiler option decides, so the port stops rather than
    /// guess it; there is no COBOL text.
    SizeError {
        /// The summary in process.
        account: AccountId,
        /// The detail being checked.
        auth_id: AuthId,
        /// The COBOL name of the field that cannot hold the result.
        field: &'static str,
        /// The size error.
        source: Overflow,
    },
}

/// The `DISPLAY` lines of the abending path that the audit records, one per
/// line: `AUTH SUMMARY READ FAILED  :` with the status (E-44) for a failed
/// summary read, then `CBPAUP0C ABENDING ...` for every IMS failure
/// (P-CBPAUP0C-002, P-CBPAUP0C-009). A size error has no COBOL text and is
/// described in one line.
impl fmt::Display for AbendCause {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SummaryRead(_) => write!(
                formatter,
                "{AUTH_SUMMARY_READ_FAILED}{STORE_FAILURE_DIBSTAT}\n{CBPAUP0C_ABENDING}"
            ),
            Self::DetailRead { .. } | Self::DetailDelete { .. } | Self::SummaryDelete { .. } => {
                formatter.write_str(CBPAUP0C_ABENDING)
            }
            Self::SizeError {
                account,
                auth_id,
                field,
                source,
            } => write!(
                formatter,
                "{field} of pending authorization {auth_id} under account {account}: {source}"
            ),
        }
    }
}

impl Error for AbendCause {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SummaryRead(error)
            | Self::DetailRead { error, .. }
            | Self::DetailDelete { error, .. }
            | Self::SummaryDelete { error, .. } => Some(error),
            Self::SizeError { source, .. } => Some(source),
        }
    }
}

/// CBPAUP0C 9999-ABEND — P-CBPAUP0C-009.
///
/// A purge run that stopped: `DISPLAY 'CBPAUP0C ABENDING ...'`,
/// `MOVE 16 TO RETURN-CODE` and `GOBACK`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L377-383]. `GOBACK` ends
/// the program normally, so the deletes counted in [`PurgeAbend::counts`]
/// persist and the adapter commits them (P-CBPAUP0C-005); the job exits 16.
///
/// The `Display` text is [`AbendCause`]'s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeAbend {
    /// The failure that stopped the run.
    pub cause: AbendCause,
    /// The work done before it, all of which stands.
    pub counts: PurgeCounts,
}

impl fmt::Display for PurgeAbend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.cause, formatter)
    }
}

impl Error for PurgeAbend {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause.source()
    }
}

/// CBPAUP0C MAIN-PARA — P-CBPAUP0C-002, P-CBPAUP0C-003, P-CBPAUP0C-006.
///
/// The purge [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L136-180]:
/// every summary in account order, each with its details, until the end of
/// the database (`GB`). `today_yyddd` is `CURRENT-YYDDD`. The rule commits
/// nothing; the adapter commits every delete it made, on success and on an
/// abend alike (P-CBPAUP0C-005, P-CBPAUP0C-009).
///
/// The summaries are read in pages of 100 and the details of a summary in
/// pages of 100 plus one look-ahead row; a row is counted when it is
/// processed, so the counts are those of the COBOL's one-row reads.
///
/// # Errors
///
/// [`PurgeAbend`] with the counts so far when a store call fails, the
/// COBOL's `9999-ABEND` for a `DIBSTAT` other than spaces, `GE` or `GB`, or
/// when a `4000-CHECK-IF-EXPIRED` result does not fit its field.
pub async fn purge<S: AuthorizationStore>(
    store: &mut S,
    params: &PurgeParams,
    today_yyddd: u32,
) -> Result<PurgeCounts, PurgeAbend> {
    let mut counts = PurgeCounts::default();
    let mut after = None;
    loop {
        let page = match store.summaries_after(after, SUMMARY_PAGE).await {
            Ok(page) => page,
            Err(error) => {
                return Err(PurgeAbend {
                    cause: AbendCause::SummaryRead(error),
                    counts,
                });
            }
        };
        if page.is_empty() {
            return Ok(counts);
        }
        for summary in page {
            after = Some(summary.pa_acct_id);
            counts.summaries_read += 1;
            purge_summary(store, summary, params.expiry_days, today_yyddd, &mut counts)
                .await
                .map_err(|cause| PurgeAbend { cause, counts })?;
        }
    }
}

/// CBPAUP0C MAIN-PARA — P-CBPAUP0C-003 to P-CBPAUP0C-007, P-CBPAUP0C-010.
///
/// One summary [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L144-158]:
/// each detail in twin order is checked and, when it qualifies, deleted; then
/// the summary is deleted when its in-memory approved count is at most zero.
/// The store's summary delete takes the details still under it, as the IMS
/// `DLET` of a parent does (P-CBPAUP0C-006).
///
/// # Errors
///
/// [`AbendCause`] for a failed detail read, detail delete or summary delete,
/// or a size error.
async fn purge_summary<S: AuthorizationStore>(
    store: &mut S,
    mut summary: PendingAuthSummary,
    expiry_days: u32,
    today_yyddd: u32,
    counts: &mut PurgeCounts,
) -> Result<(), AbendCause> {
    let account = summary.pa_acct_id;
    let mut start_key = None;
    loop {
        let browse = Browse {
            start_key,
            direction: Direction::Forward,
            limit: DETAIL_READ_LIMIT,
        };
        let mut twins = store
            .details_for_account(account, browse)
            .await
            .map_err(|error| AbendCause::DetailRead { account, error })?;
        let look_ahead = if twins.len() > usize::from(DETAIL_PAGE) {
            twins.pop()
        } else {
            None
        };
        for detail in twins {
            counts.details_read += 1;
            if check_if_expired(&mut summary, &detail, today_yyddd, expiry_days)? {
                let auth_id = detail.auth_id;
                store
                    .delete_detail(auth_id)
                    .await
                    .map_err(|error| AbendCause::DetailDelete {
                        account,
                        auth_id,
                        error,
                    })?;
                counts.details_deleted += 1;
            }
        }
        let Some(next) = look_ahead else {
            break;
        };
        start_key = Some(next.auth_id);
    }
    // `IF PA-APPROVED-AUTH-CNT <= 0 AND PA-APPROVED-AUTH-CNT <= 0`: the COBOL
    // repeats the approved count and never tests the declined one, and the
    // port keeps that (E-16), so a summary with declined authorizations left
    // is deleted with them. Writing the repeated operand once is the same test.
    if summary.pa_approved_auth_cnt <= AuthCount::ZERO {
        store
            .delete_summary(account)
            .await
            .map_err(|error| AbendCause::SummaryDelete { account, error })?;
        counts.summaries_deleted += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::{
        AbendCause, PurgeAbend, PurgeCounts, PurgeParams, check_if_expired, day_difference,
        is_expired, parse_sysin, purge,
    };
    use crate::entities::{
        FraudFlag, MatchStatus, NewPendingAuthDetail, PA_ACCOUNT_STATUS_OCCURS, PendingAuthDetail,
        PendingAuthSummary,
    };
    use crate::messages::{AUTH_SUMMARY_READ_FAILED, CBPAUP0C_ABENDING};
    use crate::numeric::{
        AccountId, Alnum, AuthCount, AuthId, CardNumber, CustomerId, Money, NumericCode, Overflow,
    };
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    /// The `CBPAUP0J` SYSIN record
    /// [app/app-authorization-ims-db2-mq/jcl/CBPAUP0J.jcl:L37].
    const JCL_SYSIN: &str = "00,00001,00001,Y";

    /// 2022-07-01 as YYDDD.
    const TODAY: u32 = 22_182;

    /// The YYDDD of the day after [`TODAY`]; a detail dated then is not yet
    /// expired.
    const TOMORROW: i32 = 22_183;

    fn account_id(value: u64) -> AccountId {
        AccountId::new(value).expect("an account number of at most 11 digits")
    }

    fn count(value: i16) -> AuthCount {
        AuthCount::new(value).expect("a count of at most four digits")
    }

    fn cents<const D: u8>(value: i64) -> Money<D> {
        Money::from_cents(value).expect("an amount within the field")
    }

    fn yyddd(value: u32) -> i32 {
        i32::try_from(value).expect("a five-digit YYDDD")
    }

    /// A summary of `account` with the given counts and amounts of 100.00
    /// approved and 50.00 declined.
    fn summary(account: u64, approved: i16, declined: i16) -> PendingAuthSummary {
        PendingAuthSummary {
            pa_acct_id: account_id(account),
            pa_cust_id: CustomerId::new(1).expect("a customer number"),
            pa_auth_status: Alnum::move_from("A"),
            pa_account_status: [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS],
            pa_credit_limit: cents(2_020_000),
            pa_cash_limit: cents(1_020_000),
            pa_credit_balance: cents(10_000),
            pa_cash_balance: Money::ZERO,
            pa_approved_auth_cnt: count(approved),
            pa_declined_auth_cnt: count(declined),
            pa_approved_auth_amt: cents(10_000),
            pa_declined_auth_amt: cents(5_000),
        }
    }

    /// A detail of `account` authorized on the YYDDD `date`, keyed as
    /// `COPAUA0C` keys it, approved (response `00`, 30.00 of 40.00) or
    /// declined (response `05`, 20.00 asked).
    fn new_detail(account: u64, date: i32, time_9c: i32, approved: bool) -> NewPendingAuthDetail {
        let (resp_code, transaction_amt, approved_amt) = if approved {
            ("00", 4_000, 3_000)
        } else {
            ("05", 2_000, 0)
        };
        NewPendingAuthDetail {
            pa_acct_id: account_id(account),
            pa_auth_date_9c: 99_999 - date,
            pa_auth_time_9c: time_9c,
            pa_auth_orig_date: Alnum::move_from("220701"),
            pa_auth_orig_time: Alnum::move_from("120000"),
            pa_card_num: CardNumber::move_from("4111000000000001"),
            pa_auth_type: Alnum::move_from("SALE"),
            pa_card_expiry_date: Alnum::move_from("0525"),
            pa_message_type: Alnum::move_from("0100"),
            pa_message_source: Alnum::move_from("POS"),
            pa_auth_id_code: Alnum::move_from("120000"),
            pa_auth_resp_code: Alnum::move_from(resp_code),
            pa_auth_resp_reason: Alnum::move_from("0000"),
            pa_processing_code: NumericCode::new(0).expect("a six-digit code"),
            pa_transaction_amt: cents(transaction_amt),
            pa_approved_amt: cents(approved_amt),
            pa_merchant_catagory_code: Alnum::move_from("5411"),
            pa_acqr_country_code: Alnum::move_from("840"),
            pa_pos_entry_mode: NumericCode::new(1).expect("a two-digit code"),
            pa_merchant_id: Alnum::move_from("800000000"),
            pa_merchant_name: Alnum::move_from("Abshire-Lowe"),
            pa_merchant_city: Alnum::move_from("Enoshaven"),
            pa_merchant_state: Alnum::move_from("NC"),
            pa_merchant_zip: Alnum::move_from("72112"),
            pa_transaction_id: Alnum::move_from("000000000000001"),
            pa_match_status: if approved {
                MatchStatus::Pending
            } else {
                MatchStatus::AuthDeclined
            },
            pa_auth_fraud: FraudFlag::Blank,
            pa_fraud_rpt_date: Alnum::spaces(),
        }
    }

    /// A stored detail of account 1 authorized on the YYDDD `date`.
    fn detail(date: i32, approved: bool) -> PendingAuthDetail {
        PendingAuthDetail::from_new(AuthId::from(1), new_detail(1, date, 1, approved))
    }

    fn params(expiry_days: u32) -> PurgeParams {
        PurgeParams {
            expiry_days,
            ..parse_sysin(JCL_SYSIN)
        }
    }

    fn counts(read: u64, deleted: u64, details_read: u64, details_deleted: u64) -> PurgeCounts {
        PurgeCounts {
            summaries_read: read,
            summaries_deleted: deleted,
            details_read,
            details_deleted,
        }
    }

    fn remaining_details(store: &FakeStore) -> Vec<i64> {
        store.details.keys().map(|auth_id| auth_id.get()).collect()
    }

    #[test]
    fn sysin_fills_prm_info_by_position_and_applies_the_defaults() {
        // P-CBPAUP0C-001.
        let cases = [
            (JCL_SYSIN, (0, 1, 1, true)),
            ("30,     ,     ,N", (30, 5, 10, false)),
            ("", (5, 5, 10, false)),
            ("AB,00001,00001,Y", (5, 1, 1, true)),
            (" 7,00001,00001,Y", (5, 1, 1, true)),
            ("7 ,00001,00001,Y", (5, 1, 1, true)),
            ("05,0    ,0    ,y", (5, 5, 10, false)),
            ("05,00000,00000,Y", (5, 0, 0, true)),
            ("05,\0\0\0\0\0,\0\0\0\0\0,Y", (5, 5, 10, true)),
            ("12,250  ,7    ,Y", (12, 250, 7, true)),
            ("12,X1   , 1   ,Y", (12, 5, 10, true)),
            (
                "99,99999,12345,YNEXT FIELDS IGNORED",
                (99, 99_999, 12_345, true),
            ),
            ("15,00010,00020,Y\n00,00001,00001,N", (15, 10, 20, true)),
            ("15,00010,00020,Y\r\n", (15, 10, 20, true)),
            ("01", (1, 5, 10, false)),
        ];
        for (sysin, (expiry_days, chkp_freq, chkp_dis_freq, debug)) in cases {
            let expected = PurgeParams {
                expiry_days,
                chkp_freq,
                chkp_dis_freq,
                debug,
            };
            assert_eq!(parse_sysin(sysin), expected, "{sysin:?}");
        }
    }

    #[test]
    fn the_day_difference_is_yyddd_arithmetic_within_s9_4() {
        // P-CBPAUP0C-004: plain YYDDD subtraction, including across a year end.
        let cases = [
            (TODAY, 22_182, Ok(0)),
            (22_187, 22_182, Ok(5)),
            (23_001, 22_365, Ok(636)),
            (22_181, 22_182, Ok(-1)),
            (22_182, 12_183, Ok(9_999)),
            (12_183, 22_182, Ok(-9_999)),
            (22_182, 12_182, Err(Overflow)),
            (12_182, 22_182, Err(Overflow)),
        ];
        for (today, date, expected) in cases {
            assert_eq!(
                day_difference(&detail(date, true), today),
                expected,
                "{today} - {date}"
            );
        }

        // WS-AUTH-DATE PIC 9(05) keeps the low-order digits of 99999 - (-1).
        let mut below_range = detail(0, true);
        below_range.pa_auth_date_9c = -1;
        assert_eq!(day_difference(&below_range, 5), Ok(5));
    }

    #[test]
    fn a_detail_expires_once_its_difference_reaches_the_expiry() {
        // P-CBPAUP0C-004; expiry 0 is the JCL value (P-CBPAUP0C-001).
        let cases = [
            (TODAY, 22_182, 0, Ok(true)),
            (TODAY, 22_182, 1, Ok(false)),
            (22_187, 22_182, 5, Ok(true)),
            (22_186, 22_182, 5, Ok(false)),
            (22_181, 22_182, 0, Ok(false)),
            (23_001, 22_365, 5, Ok(true)),
            (23_001, 22_365, 637, Ok(false)),
            (TODAY, 12_182, 0, Err(Overflow)),
        ];
        for (today, date, expiry_days, expected) in cases {
            assert_eq!(
                is_expired(&detail(date, false), today, expiry_days),
                expected,
                "{today} - {date} against {expiry_days}"
            );
        }
    }

    #[test]
    fn a_qualified_detail_decrements_only_the_summary_in_memory() {
        // P-CBPAUP0C-004, P-CBPAUP0C-011 (E-35).
        let mut held = summary(1, 2, 1);
        let approved = detail(yyddd(TODAY), true);
        assert_eq!(check_if_expired(&mut held, &approved, TODAY, 0), Ok(true));
        assert_eq!(held.pa_approved_auth_cnt, count(1));
        assert_eq!(held.pa_approved_auth_amt, cents::<9>(7_000));
        assert_eq!(held.pa_declined_auth_cnt, count(1));
        assert_eq!(held.pa_declined_auth_amt, cents::<9>(5_000));

        let declined = detail(yyddd(TODAY), false);
        assert_eq!(check_if_expired(&mut held, &declined, TODAY, 0), Ok(true));
        assert_eq!(held.pa_declined_auth_cnt, count(0));
        assert_eq!(held.pa_declined_auth_amt, cents::<9>(3_000));

        let unchanged = held.clone();
        let future = detail(TOMORROW, true);
        assert_eq!(check_if_expired(&mut held, &future, TODAY, 0), Ok(false));
        assert_eq!(held, unchanged);

        // SUBTRACT into S9(09)V99 without ON SIZE ERROR drops the tenth digit.
        held.pa_approved_auth_amt = cents(-99_999_999_999);
        let cent = PendingAuthDetail {
            pa_approved_amt: cents(1),
            ..detail(yyddd(TODAY), true)
        };
        assert_eq!(check_if_expired(&mut held, &cent, TODAY, 0), Ok(true));
        assert_eq!(held.pa_approved_auth_amt, Money::<9>::ZERO);
    }

    #[test]
    fn the_jcl_parameters_purge_every_detail_dated_today_or_earlier() {
        // P-CBPAUP0C-001 to P-CBPAUP0C-007.
        let mut store = FakeStore::default()
            .with_summary(summary(1, 2, 1))
            .with_summary(summary(2, 1, 0))
            .with_detail(new_detail(1, yyddd(TODAY), 3, true))
            .with_detail(new_detail(1, yyddd(TODAY), 2, false))
            .with_detail(new_detail(1, 22_001, 1, true))
            .with_detail(new_detail(2, TOMORROW, 1, true));
        let kept = store.summaries[&account_id(2)].clone();

        let outcome = block_on(purge(&mut store, &parse_sysin(JCL_SYSIN), TODAY));

        assert_eq!(outcome, Ok(counts(2, 1, 4, 3)));
        assert!(!store.summaries.contains_key(&account_id(1)));
        assert_eq!(store.summaries.get(&account_id(2)), Some(&kept));
        assert_eq!(remaining_details(&store), [4]);
    }

    #[test]
    fn a_summary_is_deleted_on_its_approved_count_alone() {
        // P-CBPAUP0C-006, P-CBPAUP0C-010 (E-16): three declined authorizations
        // remain in memory, and the summary goes with its unexpired one.
        let mut store = FakeStore::default()
            .with_summary(summary(1, 1, 3))
            .with_detail(new_detail(1, 22_177, 3, true))
            .with_detail(new_detail(1, 22_177, 2, false))
            .with_detail(new_detail(1, yyddd(TODAY), 1, false));

        let outcome = block_on(purge(&mut store, &params(5), TODAY));

        assert_eq!(outcome, Ok(counts(1, 1, 3, 2)));
        assert!(store.summaries.is_empty());
        assert!(store.details.is_empty(), "the unexpired detail went too");
        assert!(store.calls.contains(&"delete_summary"));
    }

    #[test]
    fn a_kept_summary_keeps_its_stored_counts_and_amounts() {
        // P-CBPAUP0C-004, P-CBPAUP0C-011 (E-35): no REPL of the summary.
        let mut store = FakeStore::default()
            .with_summary(summary(1, 2, 1))
            .with_detail(new_detail(1, yyddd(TODAY), 2, true))
            .with_detail(new_detail(1, yyddd(TODAY), 1, false));
        let stored = store.summaries[&account_id(1)].clone();

        let outcome = block_on(purge(&mut store, &params(0), TODAY));

        assert_eq!(outcome, Ok(counts(1, 0, 2, 2)));
        assert_eq!(store.summaries.get(&account_id(1)), Some(&stored));
        assert!(store.details.is_empty());
        assert!(!store.calls.contains(&"update_summary"));
    }

    #[test]
    fn a_summary_without_details_is_judged_on_its_stored_approved_count() {
        // P-CBPAUP0C-006: the condition runs after an empty GNP loop too.
        for (approved, deleted) in [(-1, true), (0, true), (1, false)] {
            let mut store = FakeStore::default().with_summary(summary(1, approved, 4));

            let outcome = block_on(purge(&mut store, &params(0), TODAY));

            let summaries_deleted = u64::from(deleted);
            assert_eq!(
                outcome,
                Ok(counts(1, summaries_deleted, 0, 0)),
                "{approved}"
            );
            assert_eq!(store.summaries.is_empty(), deleted, "{approved}");
        }
    }

    #[test]
    fn the_paged_reads_reach_every_summary_and_detail() {
        // P-CBPAUP0C-002, P-CBPAUP0C-003: 230 summaries and 205 details under
        // the first span several pages; every odd detail is dated today.
        let mut store = FakeStore::default().with_summary(summary(1, 200, 0));
        for account in 2..=230 {
            store = store.with_summary(summary(account, 0, 0));
        }
        for index in 1..=205 {
            let date = if index % 2 == 1 {
                yyddd(TODAY)
            } else {
                TOMORROW
            };
            store = store.with_detail(new_detail(1, date, index, true));
        }

        let outcome = block_on(purge(&mut store, &params(0), TODAY));

        assert_eq!(outcome, Ok(counts(230, 229, 205, 103)));
        assert_eq!(store.summaries.keys().collect::<Vec<_>>(), [&account_id(1)]);
        let left = remaining_details(&store);
        assert_eq!(left.len(), 102);
        assert!(left.iter().all(|auth_id| auth_id % 2 == 0), "{left:?}");
    }

    /// A store whose purge makes, in order: `summaries_after`,
    /// `details_for_account` (account 1), two `delete_detail` (2, then 1, in
    /// twin order), `delete_summary` (1), `details_for_account` (account 2),
    /// `delete_summary` (2) and a last `summaries_after`.
    fn two_summaries() -> FakeStore {
        FakeStore::default()
            .with_summary(summary(1, 2, 0))
            .with_summary(summary(2, 0, 0))
            .with_detail(new_detail(1, yyddd(TODAY), 2, true))
            .with_detail(new_detail(1, yyddd(TODAY), 1, true))
    }

    #[test]
    fn a_failed_store_call_abends_and_the_work_before_it_stands() {
        // P-CBPAUP0C-002, -003, -005, -007 and -009.
        let failure = PortError::Unavailable("connection reset".to_owned());
        let read_failed = format!("{AUTH_SUMMARY_READ_FAILED}AO\n{CBPAUP0C_ABENDING}");
        let account = account_id(1);
        let cases = [
            (
                "summaries_after",
                AbendCause::SummaryRead(failure.clone()),
                counts(0, 0, 0, 0),
                read_failed.as_str(),
                vec![1, 2],
            ),
            (
                "details_for_account",
                AbendCause::DetailRead {
                    account,
                    error: failure.clone(),
                },
                counts(1, 0, 0, 0),
                CBPAUP0C_ABENDING,
                vec![1, 2],
            ),
            (
                "delete_detail",
                AbendCause::DetailDelete {
                    account,
                    auth_id: AuthId::from(2),
                    error: failure.clone(),
                },
                counts(1, 0, 1, 0),
                CBPAUP0C_ABENDING,
                vec![1, 2],
            ),
            (
                "delete_summary",
                AbendCause::SummaryDelete {
                    account,
                    error: failure.clone(),
                },
                counts(1, 0, 2, 2),
                CBPAUP0C_ABENDING,
                vec![],
            ),
        ];
        for (method, cause, done, text, details_left) in cases {
            let mut store = two_summaries();
            store.fail_next(method, failure.clone());

            let abend = block_on(purge(&mut store, &params(0), TODAY))
                .expect_err("the injected failure abends the run");

            let expected = PurgeAbend {
                cause,
                counts: done,
            };
            assert_eq!(abend, expected, "{method}");
            assert_eq!(abend.to_string(), text, "{method}");
            let source = abend.source().map(ToString::to_string);
            assert_eq!(source, Some(failure.to_string()), "{method}");
            assert_eq!(store.summaries.len(), 2, "{method}: no summary was deleted");
            assert_eq!(remaining_details(&store), details_left, "{method}");
        }
    }

    #[test]
    fn a_result_that_does_not_fit_s9_4_comp_stops_the_run() {
        // 4000-CHECK-IF-EXPIRED (P-CBPAUP0C-004) with results whose value
        // depends on the TRUNC compiler option.
        let cases = [
            (summary(1, -9_999, 0), yyddd(TODAY), "PA-APPROVED-AUTH-CNT"),
            (summary(1, 1, 0), 12_182, "WS-DAY-DIFF"),
        ];
        for (held, date, field) in cases {
            let mut store = FakeStore::default()
                .with_summary(held)
                .with_detail(new_detail(1, date, 1, true));

            let abend = block_on(purge(&mut store, &params(0), TODAY))
                .expect_err("the size error stops the run");

            let expected = PurgeAbend {
                cause: AbendCause::SizeError {
                    account: account_id(1),
                    auth_id: AuthId::from(1),
                    field,
                    source: Overflow,
                },
                counts: counts(1, 0, 1, 0),
            };
            assert_eq!(abend, expected, "{field}");
            assert_eq!(
                abend.to_string(),
                format!(
                    "{field} of pending authorization 1 under account 00000000001: \
                     value does not fit the receiving field"
                )
            );
            assert_eq!(store.summaries.len(), 1, "{field}");
            assert_eq!(remaining_details(&store), [1], "{field}");
        }
    }

    #[test]
    fn the_purge_future_is_send_for_a_send_store() {
        fn assert_send<T: Send>(_: &T) {}

        let mut store = two_summaries();
        let params = parse_sysin(JCL_SYSIN);
        let run = purge(&mut store, &params, TODAY);
        assert_send(&run);
        assert_eq!(block_on(run), Ok(counts(2, 2, 2, 2)));
    }
}
