//! `COACCT01` (CDRA): the account inquiry answered on `CARD.DEMO.REPLY.ACCT`,
//! and the error records of the inquiries that get no reply.
//!
//! `COACCT01` copies each request into `REQUEST-MSG-COPY`, a 4-byte function
//! and an 11-digit account key, and answers function `INQA` with a key above
//! zero by reading the account file `ACCTDAT`
//! [app/app-vsam-mq/cbl/COACCT01.cbl:L390-460]. This module decides the bytes:
//! the 1000-byte reply text, or, when the account read fails other than
//! `NOTFND` or the reply cannot be put, the data of the `MQ-ERR-DISPLAY` record
//! written to `CARD.DEMO.ERROR`. It reads the account through
//! [`AccountReader`] and is otherwise pure; the record's bytes are laid out by
//! the messaging crate's error-queue module.
//!
//! The MQ mechanics belong to the CDRA flow and the shared consumer loop of
//! the messaging crate: the fixed reply queue (P-COACCT01-010), the
//! `message_id` and `correlation_id` echo (P-COACCT01-017), acknowledgement
//! only after the reply's publisher confirm, and rejection without requeue
//! after an error record, the outcome the COBOL commits through
//! `8000-TERMINATION` (P-COACCT01-018, P-COACCT01-032). The queue opens are the
//! flow's (P-COACCT01-030, P-COACCT01-031); the `MQCLOSE` failure paths have no
//! counterpart in a long-running consumer and are not reproduced (E-22,
//! P-COACCT01-019, P-COACCT01-029).
//!
//! # Echoed request bytes
//!
//! The not-found and invalid-request replies echo the request's key, and the
//! invalid-request reply also its function, as `STRING … DELIMITED BY SIZE`
//! copies them. The reply is text, so those bytes are read one character per
//! byte ([`Alnum::as_text`]): ASCII, which every audited request is and which
//! drives parity (E-05), is echoed byte for byte. The COBOL get converts the
//! message to the queue manager's CCSID (`MQGMO-CONVERT`), a setting the
//! repository does not hold, so no byte-exact echo is defined for other bytes;
//! their Latin-1 characters are moved into the reply's `X(11)` and `X(04)`
//! slots, cut at a character boundary.

use super::mq_date_reply::{self, ErrorRecordSpec};
use crate::entities::Account;
use crate::format::{self, AccountResponse};
use crate::messages;
use crate::numeric::{AccountId, Alnum};
use crate::ports::{AccountReader, PortError};

/// `WS-FUNC PIC X(04)`, bytes 1-4 of `REQUEST-MSG-COPY`
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L110] — P-COACCT01-011.
const FUNC_LEN: usize = 4;

/// `WS-KEY PIC 9(11)`, bytes 5-15 of `REQUEST-MSG-COPY`
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L111] — P-COACCT01-011.
const KEY_LEN: usize = 11;

/// `MQ-APPL-CONDITION-CODE` of the read-error record, the `RESP` of the failed
/// read in the COBOL (P-COACCT01-015). The database reports no CICS response
/// code: 00 is the value escalation E-39 proposes for an account read error
/// (open: the user confirms it).
const READ_ERROR_CONDITION_CODE: u32 = 0;

/// `MQ-APPL-REASON-CODE` of the read-error record, the `RESP2` of the failed
/// read in the COBOL (P-COACCT01-015): 00000, as E-39 proposes (open).
const READ_ERROR_REASON_CODE: u32 = 0;

/// `REQUEST-MSG-COPY` [app/app-vsam-mq/cbl/COACCT01.cbl:L109-112] as
/// `3000-GET-REQUEST` fills it: the request's function and account key —
/// P-COACCT01-011.
///
/// Both hold the received bytes unchanged. The `WS-FILLER X(985)` that follows
/// them is read by no statement and is not kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountRequest {
    /// `WS-FUNC PIC X(04)`: bytes 1-4 of the request. Only `INQA` reads an
    /// account.
    pub func: Alnum<4>,
    /// `WS-KEY PIC 9(11)`: bytes 5-15 of the request. Eleven digits name the
    /// account to read; any other byte makes the key test unpredictable
    /// (E-28), see [`process`].
    pub key: [u8; KEY_LEN],
}

impl AccountRequest {
    /// `WS-KEY` as an `X(11)` item, the form `STRING` copies into a reply.
    fn key_field(&self) -> Alnum<KEY_LEN> {
        Alnum::from_array(self.key)
    }
}

/// What `4000-PROCESS-REQUEST-REPLY` does with one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyOutcome {
    /// The `REPLY-MESSAGE X(1000)` text, exactly
    /// [`REPLY_BUFFER_LEN`](crate::format::REPLY_BUFFER_LEN) bytes, that
    /// `4100-PUT-REPLY` puts to `CARD.DEMO.REPLY.ACCT` with `MQ-BUFFER-LENGTH`
    /// 1000 (P-COACCT01-017).
    Reply(String),
    /// No reply: `9000-ERROR` puts this record to `CARD.DEMO.ERROR` and
    /// `8000-TERMINATION` ends the task, committing the consumed request
    /// (P-COACCT01-015, P-COACCT01-018).
    ErrorRecord(ErrorRecordSpec),
}

/// COACCT01 3000-GET-REQUEST — P-COACCT01-011.
///
/// `MOVE MQ-BUFFER TO REQUEST-MESSAGE` and `MOVE REQUEST-MESSAGE TO
/// REQUEST-MSG-COPY` [app/app-vsam-mq/cbl/COACCT01.cbl:L369-373] are group
/// moves that copy the bytes unchanged: the function is bytes 1-4 of `body`
/// and the key bytes 5-15, digits or not. The `INITIALIZE REQUEST-MSG-COPY
/// REPLACING NUMERIC BY ZEROES` before the get
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L345] is overwritten by that move and
/// leaves nothing.
///
/// A body shorter than 15 bytes leaves the rest of the copy to buffer bytes
/// the get does not write. `MQ-BUFFER` has no `VALUE`, so the COBOL bytes are
/// unpredictable (E-25); they are read as spaces, the fill E-25 records. Bytes
/// after the 15th fall in `WS-FILLER` and decide nothing.
#[must_use]
pub fn parse_request(body: &[u8]) -> AccountRequest {
    let key_bytes = body.get(FUNC_LEN..).unwrap_or_default();
    AccountRequest {
        func: Alnum::move_from_bytes(body),
        key: *Alnum::<KEY_LEN>::move_from_bytes(key_bytes).as_bytes(),
    }
}

/// COACCT01 4000-PROCESS-REQUEST-REPLY — P-COACCT01-012 to P-COACCT01-016,
/// P-COACCT01-020 to P-COACCT01-028.
///
/// The outcome of `request` [app/app-vsam-mq/cbl/COACCT01.cbl:L390-460]:
///
/// - Function `INQA` with a key above zero reads `ACCTDAT` by the 11-digit key
///   (P-COACCT01-012):
///   - found: the 252 bytes of `WS-ACCT-RESPONSE`, labels and record fields
///     with the money fields unedited zoned (P-COACCT01-013,
///     P-COACCT01-020 to P-COACCT01-025);
///   - `NOTFND`: `INVALID REQUEST PARAMETERS ACCT ID : ` and the key, 48 bytes
///     (P-COACCT01-014, P-COACCT01-026);
///   - any other failure: no reply, but [`read_error_record`] for
///     `input_queue`, the `INPUT-QUEUE-NAME` the request came from
///     (P-COACCT01-015).
/// - Any other request reads nothing and gets the not-found text followed by
///   `FUNCTION : ` and the function, 63 bytes with no space before the label
///   (P-COACCT01-016, P-COACCT01-027, P-COACCT01-028).
///
/// Every reply text is padded with spaces to the 1000 bytes the put sends.
///
/// A key holding any byte other than a digit makes `WS-KEY > ZEROES`
/// unpredictable (E-28, halt). The escalation decides that such a request is
/// answered with the invalid-parameters reply and reads nothing, so it takes
/// the second branch whatever its function.
pub async fn process<S: AccountReader>(
    store: &mut S,
    request: &AccountRequest,
    input_queue: &str,
) -> ReplyOutcome {
    let Some(account_id) = inquiry_key(request) else {
        return ReplyOutcome::Reply(invalid_request_reply(request));
    };
    match store.read_account(account_id).await {
        Ok(account) => ReplyOutcome::Reply(account_reply(&account)),
        Err(PortError::NotFound) => ReplyOutcome::Reply(format::pad_reply_1000(
            &messages::cdra_account_not_found_reply(&request.key_field().as_text()),
        )),
        Err(
            PortError::Duplicate
            | PortError::Restricted
            | PortError::Contention
            | PortError::Unavailable(_),
        ) => ReplyOutcome::ErrorRecord(read_error_record(input_queue)),
    }
}

/// COACCT01 4000-PROCESS-REQUEST-REPLY `IF WS-FUNC = 'INQA' AND WS-KEY >
/// ZEROES` [app/app-vsam-mq/cbl/COACCT01.cbl:L393] — P-COACCT01-012,
/// P-COACCT01-016, P-COACCT01-028.
///
/// The account to read, or `None` for the `ELSE` branch. The function is
/// compared byte for byte with `INQA`, case included. A key of eleven digits
/// compares by its value, so all zeros is not above zero. A key holding any
/// other byte does not parse as an [`AccountId`], which accepts digits only,
/// and is `None` as well: E-28 (halt) answers it with the `ELSE` branch's
/// reply rather than a guessed comparison result.
fn inquiry_key(request: &AccountRequest) -> Option<AccountId> {
    if request.func.as_bytes() != messages::CDRA_FUNCTION_INQA.as_bytes() {
        return None;
    }
    let account_id: AccountId = request.key_field().as_text().parse().ok()?;
    (account_id.get() > 0).then_some(account_id)
}

/// COACCT01 4000-PROCESS-REQUEST-REPLY `ELSE` branch
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L448-456] — P-COACCT01-016: the
/// invalid-request text with the key and function as received, padded to the
/// 1000-byte reply.
fn invalid_request_reply(request: &AccountRequest) -> String {
    format::pad_reply_1000(&messages::cdra_invalid_request_reply(
        &request.key_field().as_text(),
        &request.func.as_text(),
    ))
}

/// COACCT01 4000-PROCESS-REQUEST-REPLY `WHEN DFHRESP(NORMAL)`
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L407-427] — P-COACCT01-013: the eleven
/// account fields moved into `WS-ACCT-RESPONSE`, which moves into the
/// space-filled `REPLY-MESSAGE`.
fn account_reply(account: &Account) -> String {
    let active_status = account.acct_active_status.as_text();
    let open_date = account.acct_open_date.as_text();
    let expiration_date = account.acct_expiraion_date.as_text();
    let reissue_date = account.acct_reissue_date.as_text();
    let group_id = account.acct_group_id.as_alnum().as_text();
    let response = format::acct_response(&AccountResponse {
        acct_id: account.acct_id.get(),
        active_status: &active_status,
        curr_bal: account.acct_curr_bal,
        credit_limit: account.acct_credit_limit,
        cash_credit_limit: account.acct_cash_credit_limit,
        open_date: &open_date,
        expiration_date: &expiration_date,
        reissue_date: &reissue_date,
        curr_cyc_credit: account.acct_curr_cyc_credit,
        curr_cyc_debit: account.acct_curr_cyc_debit,
        group_id: &group_id,
    });
    format::pad_reply_1000(&response)
}

/// COACCT01 4000-PROCESS-REQUEST-REPLY `WHEN OTHER`
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L437-445] — P-COACCT01-015.
///
/// The error record of an account read that failed other than `NOTFND`:
/// `ERROR WHILE READING ACCTFILE` as the return message, which its `X(25)`
/// field cuts to `ERROR WHILE READING ACCTF` when the record is laid out, and
/// `input_queue`, the `INPUT-QUEUE-NAME` of the request, as the queue. No
/// reply is put.
///
/// The COBOL moves the read's `RESP` and `RESP2` into the condition and reason
/// codes. The database reports no CICS codes, so the record carries 00 and
/// 00000, the codes escalation E-39 proposes for an account read error (open:
/// the user confirms them), whatever the failure. `MQ-ERROR-PARA` stays as
/// `INITIALIZE MQ-ERR-DISPLAY` left it, spaces
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L185]: its only assignment,
/// `'CICS RETREIVE'` [app/app-vsam-mq/cbl/COACCT01.cbl:L200], ends the task
/// before any request is read.
///
/// The read is the program's only file access, so a read that cannot start,
/// for example because no unit of work can be opened, has this outcome too.
#[must_use]
pub fn read_error_record(input_queue: &str) -> ErrorRecordSpec {
    ErrorRecordSpec {
        error_para: "",
        return_message: messages::ERROR_WHILE_READING_ACCTFILE,
        condition_code: READ_ERROR_CONDITION_CODE,
        reason_code: READ_ERROR_REASON_CODE,
        queue_name: input_queue.to_owned(),
    }
}

/// COACCT01 4100-PUT-REPLY `WHEN OTHER`
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L492-498] — P-COACCT01-017.
///
/// The error record of a reply put that failed: the put's condition and reason
/// codes, `reply_queue` (`REPLY-QUEUE-NAME`) as the queue and `MQPUT ERR` as
/// the return message, with `MQ-ERROR-PARA` spaces as in
/// [`read_error_record`]. `9000-ERROR` then puts it to `CARD.DEMO.ERROR`; the
/// request is consumed and no reply exists.
///
/// `CODATE01` builds the same record for its own reply put, so this is
/// [`mq_date_reply::put_reply_failure`]. AMQP has no MQ codes; the caller
/// passes the codes its publish failure maps to (E-39).
#[must_use]
pub fn put_reply_failure(
    condition_code: u32,
    reason_code: u32,
    reply_queue: &str,
) -> ErrorRecordSpec {
    mq_date_reply::put_reply_failure(condition_code, reason_code, reply_queue)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::ops::Range;

    use super::{
        AccountRequest, ReplyOutcome, parse_request, process, put_reply_failure, read_error_record,
    };
    use crate::entities::Account;
    use crate::format::{self, REPLY_BUFFER_LEN};
    use crate::messages;
    use crate::numeric::{AccountGroupId, Alnum, Money};
    use crate::ports::PortError;
    use crate::rules::mq_date_reply::{self, ErrorRecordSpec};
    use crate::rules::test_support::{FakeStore, block_on};

    type TestResult = Result<(), Box<dyn Error>>;

    /// The 50 `ACCOUNT-RECORD`s of the fixture corpus, as the seed loads them.
    const ACCTDATA: &str = include_str!("../../../../app/data/ASCII/acctdata.txt");

    /// Section D of the parity audit, which holds the expected CDRA replies.
    const AUDIT: &str = include_str!("../../../../docs/parity/parity-oracle-audit.md");

    /// `INPUT-QUEUE-NAME`, the request queue's local default.
    const INPUT_QUEUE: &str = "CARD.DEMO.REQUEST.ACCT";

    /// `REPLY-QUEUE-NAME` [app/app-vsam-mq/cbl/COACCT01.cbl:L198].
    const REPLY_QUEUE: &str = "CARD.DEMO.REPLY.ACCT";

    /// The `WS-ACCT-RESPONSE` labels and the `CVACT01Y` offsets of the fields
    /// they precede (P-COACCT01-013); `ACCT-ADDR-ZIP` at 102-111 is not sent.
    const RESPONSE_FIELDS: [(&str, Range<usize>); 11] = [
        ("ACCOUNT ID : ", 0..11),
        ("ACCOUNT STATUS : ", 11..12),
        ("BALANCE : ", 12..24),
        ("CREDIT LIMIT : ", 24..36),
        ("CASH LIMIT : ", 36..48),
        ("OPEN DATE : ", 48..58),
        ("EXPR DATE : ", 58..68),
        ("REIS DATE : ", 68..78),
        ("CREDIT BAL : ", 78..90),
        ("DEBIT BAL : ", 90..102),
        ("GROUP ID : ", 112..122),
    ];

    /// The cents of a `S9(10)V99` zoned field with its sign as a trailing
    /// overpunch: `{ABCDEFGHI` for +0 to +9, `}JKLMNOPQR` for -0 to -9.
    fn zoned_cents(field: &str) -> Result<i64, Box<dyn Error>> {
        let (digits, last) = field.split_at_checked(11).ok_or("a 12-byte zoned field")?;
        let (last_digit, negative) = match "{ABCDEFGHI".find(last) {
            Some(digit) => (digit, false),
            None => ("}JKLMNOPQR".find(last).ok_or("an overpunch sign")?, true),
        };
        let magnitude: i64 = format!("{digits}{last_digit}").parse()?;
        Ok(if negative { -magnitude } else { magnitude })
    }

    /// One `acctdata.txt` record decoded by the `CVACT01Y` offsets.
    fn fixture_account(line: &str) -> Result<Account, Box<dyn Error>> {
        let field = |range: Range<usize>| {
            line.get(range)
                .ok_or("acctdata.txt record shorter than ACCOUNT-RECORD")
        };
        let money = |range: Range<usize>| -> Result<Money<10>, Box<dyn Error>> {
            Ok(Money::<10>::from_cents(zoned_cents(field(range)?)?)?)
        };
        Ok(Account {
            acct_id: field(0..11)?.parse()?,
            acct_active_status: Alnum::move_from(field(11..12)?),
            acct_curr_bal: money(12..24)?,
            acct_credit_limit: money(24..36)?,
            acct_cash_credit_limit: money(36..48)?,
            acct_open_date: Alnum::move_from(field(48..58)?),
            acct_expiraion_date: Alnum::move_from(field(58..68)?),
            acct_reissue_date: Alnum::move_from(field(68..78)?),
            acct_curr_cyc_credit: money(78..90)?,
            acct_curr_cyc_debit: money(90..102)?,
            acct_addr_zip: Alnum::move_from(field(102..112)?),
            acct_group_id: AccountGroupId::move_from(field(112..122)?),
        })
    }

    /// `ACCTDAT` as the seed loads it from the fixture corpus.
    fn fixture_store() -> Result<FakeStore, Box<dyn Error>> {
        ACCTDATA
            .lines()
            .try_fold(FakeStore::default(), |store, line| {
                Ok(store.with_account(fixture_account(line)?))
            })
    }

    /// The outcome of `body` against `store`.
    fn answer(store: &mut FakeStore, body: &[u8]) -> ReplyOutcome {
        block_on(process(store, &parse_request(body), INPUT_QUEUE))
    }

    /// A reply of `text` padded with spaces to the 1000-byte buffer.
    fn reply(text: &str) -> ReplyOutcome {
        ReplyOutcome::Reply(format::pad_reply_1000(text))
    }

    /// The section D rows of `COACCT01`: each row ID, its request and its
    /// expected reply, right-trimmed in the audit and padded here to the
    /// `width=1000` the rows record.
    fn audit_replies() -> Vec<(String, String, String)> {
        AUDIT
            .lines()
            .filter(|row| row.starts_with("| P-COACCT01-") && row.contains("request=`\""))
            .filter_map(|row| {
                let id = row.get(2..16)?;
                let request = row.split("request=`\"").nth(1)?.split("\"`").next()?;
                let text = row
                    .split("reply=`\"")
                    .nth(1)?
                    .split("\"` width=1000")
                    .next()?;
                Some((
                    id.to_owned(),
                    request.to_owned(),
                    format::pad_reply_1000(text),
                ))
            })
            .collect()
    }

    fn assert_send<T: Send>(_: &T) {}

    #[test]
    fn audited_requests_get_their_audited_replies_byte_for_byte() -> TestResult {
        // P-COACCT01-020 to P-COACCT01-025: six fixture accounts found;
        // P-COACCT01-026: no such account (NOTFND); P-COACCT01-027: function
        // INQX; P-COACCT01-028: key zero. All replies are 1000 bytes.
        let rows = audit_replies();
        let ids: Vec<&str> = rows.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            (20..=28)
                .map(|row| format!("P-COACCT01-{row:03}"))
                .collect::<Vec<_>>()
        );
        let mut store = fixture_store()?;
        for (id, request, expected) in &rows {
            assert_eq!(expected.len(), REPLY_BUFFER_LEN, "{id}");
            let outcome = answer(&mut store, request.as_bytes());
            assert_eq!(outcome, ReplyOutcome::Reply(expected.clone()), "{id}");
        }
        Ok(())
    }

    #[test]
    fn account_one_replies_with_the_full_zoned_response() -> TestResult {
        // P-COACCT01-012: INQA with a key above zero reads ACCTDAT once;
        // P-COACCT01-020, P-COACCT01-013: the 252 response bytes, money
        // fields unedited zoned with the sign overpunched, then 748 spaces.
        let mut store = fixture_store()?;
        let ReplyOutcome::Reply(text) = answer(&mut store, b"INQA00000000001") else {
            panic!("account 00000000001 is in the fixture corpus");
        };
        assert_eq!(text.len(), REPLY_BUFFER_LEN);
        assert_eq!(
            &text[..252],
            "ACCOUNT ID : 00000000001ACCOUNT STATUS : YBALANCE : 00000001940{CREDIT LIMIT : 00000020200{CASH LIMIT : 00000010200{OPEN DATE : 2014-11-20EXPR DATE : 2025-05-20REIS DATE : 2025-05-20CREDIT BAL : 00000000000{DEBIT BAL : 00000000000{GROUP ID :           "
        );
        assert!(text[252..].bytes().all(|byte| byte == b' '));
        assert_eq!(store.calls, ["read_account"]);
        Ok(())
    }

    #[test]
    fn every_fixture_account_replies_with_its_own_record_bytes() -> TestResult {
        // P-COACCT01-013: each field moves unchanged into a field of the same
        // PIC, so the reply carries the record's own bytes after each label.
        let mut store = fixture_store()?;
        let mut answered = 0;
        for line in ACCTDATA.lines() {
            let mut expected = String::new();
            for (label, range) in RESPONSE_FIELDS {
                expected.push_str(label);
                expected.push_str(line.get(range).ok_or("a full record")?);
            }
            let key = line.get(..11).ok_or("an account key")?;
            let outcome = answer(&mut store, format!("INQA{key}").as_bytes());
            assert_eq!(outcome, reply(&expected), "account {key}");
            answered += 1;
        }
        assert_eq!(answered, 50);
        Ok(())
    }

    #[test]
    fn a_negative_balance_is_overpunched_negative() -> TestResult {
        // P-COACCT01-013: signed fields keep their sign as the trailing
        // overpunch `}JKLMNOPQR`; a stored group ID is sent as stored.
        let mut account = fixture_account(ACCTDATA.lines().next().ok_or("a record")?)?;
        account.acct_curr_bal = Money::<10>::from_cents(-1_234)?;
        account.acct_curr_cyc_debit = Money::<10>::from_cents(-10)?;
        account.acct_group_id = AccountGroupId::move_from("DEFAULT");
        let mut store = FakeStore::default().with_account(account);
        let ReplyOutcome::Reply(text) = answer(&mut store, b"INQA00000000001") else {
            panic!("the account is stored");
        };
        assert_eq!(&text[52..64], "00000000123M");
        assert_eq!(&text[219..231], "00000000001}");
        assert_eq!(&text[242..252], "DEFAULT   ");
        Ok(())
    }

    #[test]
    fn a_missing_account_gets_the_not_found_text_after_one_read() -> TestResult {
        // P-COACCT01-014, P-COACCT01-026: 48 STRING bytes, then spaces.
        let mut store = fixture_store()?;
        let outcome = answer(&mut store, b"INQA00000000051");
        assert_eq!(
            outcome,
            reply("INVALID REQUEST PARAMETERS ACCT ID : 00000000051")
        );
        assert_eq!(store.calls, ["read_account"]);
        Ok(())
    }

    #[test]
    fn requests_that_are_not_an_inquiry_read_nothing() -> TestResult {
        // P-COACCT01-016, P-COACCT01-027, P-COACCT01-028: function not INQA or
        // key zero; E-28 (halt): a key with a non-digit byte gets the same
        // invalid-parameters reply; E-25: the bytes a short body leaves unset
        // are spaces. Nothing is read in any case.
        let cases: [(&[u8], &str); 9] = [
            (
                b"INQX00000000001",
                "INVALID REQUEST PARAMETERS ACCT ID : 00000000001FUNCTION : INQX",
            ),
            (
                b"inqa00000000001",
                "INVALID REQUEST PARAMETERS ACCT ID : 00000000001FUNCTION : inqa",
            ),
            (
                b"INQA00000000000",
                "INVALID REQUEST PARAMETERS ACCT ID : 00000000000FUNCTION : INQA",
            ),
            (
                b"INQA0000000001A",
                "INVALID REQUEST PARAMETERS ACCT ID : 0000000001AFUNCTION : INQA",
            ),
            (
                b"INQA 0000000001",
                "INVALID REQUEST PARAMETERS ACCT ID :  0000000001FUNCTION : INQA",
            ),
            (
                b"INQX0000000000A",
                "INVALID REQUEST PARAMETERS ACCT ID : 0000000000AFUNCTION : INQX",
            ),
            (
                b"INQA1",
                "INVALID REQUEST PARAMETERS ACCT ID : 1          FUNCTION : INQA",
            ),
            (
                b"IN",
                "INVALID REQUEST PARAMETERS ACCT ID :            FUNCTION : IN  ",
            ),
            (
                b"",
                "INVALID REQUEST PARAMETERS ACCT ID :            FUNCTION :     ",
            ),
        ];
        for (body, expected) in cases {
            let mut store = fixture_store()?;
            let outcome = answer(&mut store, body);
            assert_eq!(outcome, reply(expected), "{body:?}");
            assert!(store.calls.is_empty(), "{body:?}");
        }
        Ok(())
    }

    #[test]
    fn a_read_failure_other_than_not_found_gives_the_error_record() -> TestResult {
        // P-COACCT01-015: no reply; the record names the input queue and the
        // return message, cut to its X(25) field when laid out; E-39 (open):
        // condition 00 and reason 00000 for any account read error.
        let failures = [
            PortError::Unavailable("connection reset".to_owned()),
            PortError::Contention,
            PortError::Restricted,
            PortError::Duplicate,
        ];
        for failure in failures {
            let mut store = fixture_store()?;
            store.fail_next("read_account", failure.clone());
            let outcome = answer(&mut store, b"INQA00000000001");
            let ReplyOutcome::ErrorRecord(spec) = outcome else {
                panic!("{failure:?} gives no reply");
            };
            assert_eq!(spec, read_error_record(INPUT_QUEUE), "{failure:?}");
            assert_eq!(
                format::move_alnum(spec.return_message, 25),
                "ERROR WHILE READING ACCTF"
            );
            assert_eq!(store.calls, ["read_account"], "{failure:?}");
        }
        Ok(())
    }

    #[test]
    fn the_read_error_record_carries_the_literal_the_codes_and_the_queue() {
        // P-COACCT01-015: 'ERROR WHILE READING ACCTFILE', INPUT-QUEUE-NAME;
        // MQ-ERROR-PARA keeps the spaces of the INITIALIZE (P-COACCT01-001);
        // E-39: codes 00 and 00000.
        for queue in [INPUT_QUEUE, "ANOTHER.REQUEST.QUEUE"] {
            assert_eq!(
                read_error_record(queue),
                ErrorRecordSpec {
                    error_para: "",
                    return_message: messages::ERROR_WHILE_READING_ACCTFILE,
                    condition_code: 0,
                    reason_code: 0,
                    queue_name: queue.to_owned(),
                }
            );
        }
        assert_eq!(
            messages::ERROR_WHILE_READING_ACCTFILE,
            "ERROR WHILE READING ACCTFILE"
        );
    }

    #[test]
    fn a_failed_reply_put_gives_the_mqput_err_record() {
        // P-COACCT01-017: 'MQPUT ERR', the put's codes and REPLY-QUEUE-NAME,
        // the same record CODATE01 builds (P-CODATE01-015); E-39: the codes
        // are the caller's, passed through unchanged.
        for (condition_code, reason_code) in [(2, 2085), (0, 0), (2, 123_456)] {
            let spec = put_reply_failure(condition_code, reason_code, REPLY_QUEUE);
            assert_eq!(
                spec,
                ErrorRecordSpec {
                    error_para: "",
                    return_message: messages::MQPUT_ERR,
                    condition_code,
                    reason_code,
                    queue_name: REPLY_QUEUE.to_owned(),
                }
            );
            assert_eq!(
                spec,
                mq_date_reply::put_reply_failure(condition_code, reason_code, REPLY_QUEUE)
            );
        }
    }

    #[test]
    fn the_request_copy_takes_bytes_one_to_fifteen() {
        // P-COACCT01-011: bytes 1-4 the function, 5-15 the key, as received;
        // E-25: unset bytes of a short body are spaces; WS-FILLER is not read.
        let long = [b"INQA00000000042".as_slice(), &[b'X'; 2000]].concat();
        let cases: [(&[u8], &[u8; 4], &[u8; 11]); 4] = [
            (b"INQA00000000042", b"INQA", b"00000000042"),
            (long.as_slice(), b"INQA", b"00000000042"),
            (b"INQA42", b"INQA", b"42         "),
            (b"IN", b"IN  ", b"           "),
        ];
        for (body, func, key) in cases {
            assert_eq!(
                parse_request(body),
                AccountRequest {
                    func: Alnum::from_array(*func),
                    key: *key,
                },
                "{:?}",
                body.get(..15)
            );
        }
    }

    #[test]
    fn non_ascii_request_bytes_are_echoed_as_latin_1_within_the_1000_bytes() -> TestResult {
        // Module docs, "Echoed request bytes": no byte-exact echo exists for
        // bytes the COBOL get converts through the queue manager's CCSID.
        let mut store = fixture_store()?;
        let outcome = answer(&mut store, b"IN\xC9A00000000001");
        assert_eq!(
            outcome,
            reply("INVALID REQUEST PARAMETERS ACCT ID : 00000000001FUNCTION : IN\u{c9}")
        );
        let ReplyOutcome::Reply(text) = outcome else {
            panic!("an invalid request is answered");
        };
        assert_eq!(text.len(), REPLY_BUFFER_LEN);
        assert!(store.calls.is_empty());
        Ok(())
    }

    #[test]
    fn the_inquiry_future_is_send() -> TestResult {
        // The consumer runs flows on a multi-threaded runtime.
        let mut store = fixture_store()?;
        let request = parse_request(b"INQA00000000001");
        let future = process(&mut store, &request, INPUT_QUEUE);
        assert_send(&future);
        assert!(matches!(block_on(future), ReplyOutcome::Reply(_)));
        Ok(())
    }
}
