//! `batch cbpaup0j` parity: the purge of expired pending authorizations that the job
//! `CBPAUP0J` runs as the IMS BMP program `CBPAUP0C`
//! [app/app-authorization-ims-db2-mq/jcl/CBPAUP0J.jcl:L24-37]
//! [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L136-180].
//!
//! The fixture corpus holds no pending authorization: the seed loads no IMS unload, so both
//! authorization tables start empty and the fixture expectations of the parity audit (section D)
//! have no CBPAUP0J subsection to derive. The test commits its own summaries and details
//! instead, under fixture accounts with their cross-reference customers and cards, and every
//! expected deletion follows from the reviewed section B, E and F rows of `CBPAUP0C` alone, each
//! of which pins one result:
//!
//! - `1000-INITIALIZE` reads the JCL card as expiry 0 days, frequencies 1 and 1, debug on, and
//!   takes `CURRENT-YYDDD` from the processing clock (P-CBPAUP0C-001, P-CBPAUP0C-012).
//! - `4000-CHECK-IF-EXPIRED` deletes a detail when `CURRENT-YYDDD - (99999 - PA-AUTH-DATE-9C)`
//!   is at least the expiry days, so at expiry 0 every detail dated on the clock's day or earlier
//!   goes, and one dated the day after stays. The approved or declined count and amount fall in
//!   memory only (P-CBPAUP0C-004, P-CBPAUP0C-005, P-CBPAUP0C-011, E-35).
//! - The summary goes when its in-memory approved count is at most zero, whatever its declined
//!   count, and takes every detail still under it (P-CBPAUP0C-006, P-CBPAUP0C-007,
//!   P-CBPAUP0C-010, E-16).
//! - The sweep reads summaries in `ACCNTID` order and the details of each in twin order, and the
//!   end of job displays what it read and deleted (P-CBPAUP0C-002, P-CBPAUP0C-003). The final
//!   checkpoint commits every delete (P-CBPAUP0C-008), so the rows are read back after the run
//!   from another connection.

pub mod common;

use carddemo_domain::entities::{
    FraudFlag, MatchStatus, NewPendingAuthDetail, PA_ACCOUNT_STATUS_OCCURS, PendingAuthSummary,
};
use carddemo_domain::messages::CBPAUP0C_ABENDING;
use carddemo_domain::numeric::{
    AccountId, Alnum, AuthCount, CardNumber, CustomerId, Money, NumericCode,
};
use carddemo_domain::ports::AuthorizationStore;
use carddemo_persistence::Db;
use common::{Harness, TRANREPT_CLOCK, block_on, expected_value, parity, parity_cell};

/// The parity audit rows this test proves: the reviewed `CBPAUP0C` rules of section B, the E-16
/// and E-35 dispositions of section E, and the clock row of section F. Section D has no CBPAUP0J
/// rows to add, because the fixture corpus holds no pending authorization.
const P_IDS: &[&str] = &[
    "P-CBPAUP0C-001",
    "P-CBPAUP0C-002",
    "P-CBPAUP0C-003",
    "P-CBPAUP0C-004",
    "P-CBPAUP0C-005",
    "P-CBPAUP0C-006",
    "P-CBPAUP0C-007",
    "P-CBPAUP0C-008",
    "P-CBPAUP0C-010",
    "P-CBPAUP0C-011",
    "P-CBPAUP0C-012",
];

/// The exit code of a normal end.
const SUCCESS: i32 = 0;

/// The `SYSIN` card of `CBPAUP0J` [app/app-authorization-ims-db2-mq/jcl/CBPAUP0J.jcl:L36-37]:
/// expiry 0 days, checkpoint frequency and checkpoint display frequency `00001`, debug `Y`.
const SYSIN: &str = "00,00001,00001,Y";

/// The processing clock the audit derives its batch expectations under, 2022-07-01T12:00:00Z,
/// handed to the binary as `CARDDEMO_CLOCK_OVERRIDE`. `ACCEPT CURRENT-YYDDD FROM DAY` gives
/// [`TODAY`] from it (P-CBPAUP0C-012).
const CLOCK: &str = TRANREPT_CLOCK;

/// `CURRENT-YYDDD` under [`CLOCK`]: day 182 of 2022.
const TODAY: i32 = 22_182;

/// The complement `COPAUA0C` stores the authorization date under and `CBPAUP0C` takes it back
/// from: `PA-AUTH-DATE-9C = 99999 - YYDDD` (P-CBPAUP0C-004).
const AUTH_DATE_COMPLEMENT: i32 = 99_999;

/// `PA-TRANSACTION-AMT` of every detail and `PA-APPROVED-AMT` of an approval, in cents: 100.00.
const APPROVAL_CENTS: i64 = 10_000;

/// `PA-TRANSACTION-AMT` of a declined detail, in cents: 40.00. Its `PA-APPROVED-AMT` is 0.00.
const DECLINE_CENTS: i64 = 4_000;

/// One `PAUTDTL1` detail to commit.
#[derive(Debug, Clone, Copy)]
struct DetailInput {
    /// The authorization day as `YYDDD`; the stored key part is its complement.
    yyddd: i32,
    /// `PA-AUTH-ORIG-DATE`, the same day as `YYMMDD`.
    orig_date: &'static str,
    /// `PA-AUTH-TIME-9C`, the second key part, distinct within the summary.
    time_9c: i32,
    /// Whether `PA-AUTH-RESP-CODE` is `00`: an approval, else a decline with `05`.
    approved: bool,
}

/// One `PAUTSUM0` summary to commit, with its stored counts and its details.
#[derive(Debug, Clone, Copy)]
struct SummaryInput {
    /// `PA-ACCT-ID`: a fixture account, whose customer has the same number and whose card is
    /// its cross-reference card [app/data/ASCII/cardxref.txt].
    account: u32,
    /// The account's card in the cross-reference fixture.
    card: &'static str,
    /// `PA-APPROVED-AUTH-CNT` as stored; the approved amount is this many times 100.00.
    approved: i16,
    /// `PA-DECLINED-AUTH-CNT` as stored; the declined amount is this many times 40.00.
    declined: i16,
    details: &'static [DetailInput],
}

/// `DetailInput` shorthand.
const fn detail_input(
    yyddd: i32,
    orig_date: &'static str,
    time_9c: i32,
    approved: bool,
) -> DetailInput {
    DetailInput {
        yyddd,
        orig_date,
        time_9c,
        approved,
    }
}

/// The authorizations the test commits, in `ACCNTID` order, all against the clock's day 22182:
///
/// - Account 2: two approvals, from the previous year (21365) and from the clock's day itself
///   (day difference 0), under an approved count of 2. Both expire, the count reaches 0 and the
///   summary goes (P-CBPAUP0C-004 to P-CBPAUP0C-007).
/// - Account 12, the E-16 case: one expired approval under an approved count of 1 and a declined
///   count of 3. The approved count reaches 0 and the summary goes, its declined count untested
///   (P-CBPAUP0C-006, P-CBPAUP0C-010).
/// - Account 20: an expired approval and a decline dated the day after the clock's, which never
///   expires at expiry 0. The approved count reaches 0, so the summary goes and takes the
///   unexpired decline with it, as an IMS `DLET` of a parent does (P-CBPAUP0C-006, E-35).
/// - Account 27: an expired approval, an expired decline and an unexpired approval under counts
///   of 3 and 2. The in-memory counts fall to 2 and 1, so the summary stays with its stored
///   counts and amounts (P-CBPAUP0C-004, P-CBPAUP0C-011, E-35).
/// - Account 50, the mirror of E-16: an expired decline and an unexpired approval under counts of
///   1 and 1. The declined count reaches 0 but the approved count stays 1, so the summary stays.
const AUTHORIZATIONS: [SummaryInput; 5] = [
    SummaryInput {
        account: 2,
        card: "0923877193247330",
        approved: 2,
        declined: 0,
        details: &[
            detail_input(21_365, "211231", 879_999_999, true),
            detail_input(TODAY, "220701", 879_998_999, true),
        ],
    },
    SummaryInput {
        account: 12,
        card: "0982496213629795",
        approved: 1,
        declined: 3,
        details: &[detail_input(22_181, "220630", 879_999_999, true)],
    },
    SummaryInput {
        account: 20,
        card: "0927987108636232",
        approved: 1,
        declined: 1,
        details: &[
            detail_input(22_100, "220410", 879_999_999, true),
            detail_input(22_183, "220702", 879_998_999, false),
        ],
    },
    SummaryInput {
        account: 27,
        card: "0683586198171516",
        approved: 3,
        declined: 2,
        details: &[
            detail_input(22_175, "220624", 879_999_999, true),
            detail_input(22_176, "220625", 879_998_999, false),
            detail_input(22_183, "220702", 879_997_999, true),
        ],
    },
    SummaryInput {
        account: 50,
        card: "0500024453765740",
        approved: 1,
        declined: 1,
        details: &[
            detail_input(TODAY, "220701", 879_999_999, false),
            detail_input(22_183, "220702", 879_998_999, true),
        ],
    },
];

/// Every summary's account, stored counts and stored amounts, in `ACCNTID` order.
const SUMMARIES_SQL: &str = "SELECT pa_acct_id, pa_approved_auth_cnt, pa_declined_auth_cnt, \
     pa_approved_auth_amt, pa_declined_auth_amt FROM pending_auth_summaries ORDER BY pa_acct_id";

/// Every detail's account, `PAUT9CTS` key and response code, in account and twin order
/// (ascending complemented key: newest first).
const DETAILS_SQL: &str = "SELECT pa_acct_id, pa_auth_date_9c, pa_auth_time_9c, \
     pa_auth_resp_code FROM pending_auth_details \
     ORDER BY pa_acct_id, pa_auth_date_9c, pa_auth_time_9c";

/// The summaries the purge keeps, as [`SUMMARIES_SQL`] reads them: accounts 27 and 50 with the
/// counts and amounts they were committed with, because the decrements are never written back
/// (P-CBPAUP0C-011, E-35).
const SURVIVING_SUMMARIES: [[&str; 5]; 2] = [
    ["27", "3", "2", "300.00", "80.00"],
    ["50", "1", "1", "100.00", "40.00"],
];

/// The details the purge keeps, as [`DETAILS_SQL`] reads them: the approvals of accounts 27 and
/// 50 dated 22183, the day after the clock's (complemented 77816).
const SURVIVING_DETAILS: [[&str; 4]; 2] = [
    ["27", "77816", "879997999", "00"],
    ["50", "77816", "879998999", "00"],
];

/// The details the purge removes, as [`DETAILS_SQL`] reads them before the run: the seven expired
/// details it deletes one by one, and account 20's unexpired decline (77816), which goes with its
/// summary.
const DELETED_DETAILS: [[&str; 4]; 8] = [
    ["2", "77817", "879998999", "00"],
    ["2", "78634", "879999999", "00"],
    ["12", "77818", "879999999", "00"],
    ["20", "77816", "879998999", "05"],
    ["20", "77899", "879999999", "00"],
    ["27", "77823", "879998999", "05"],
    ["27", "77824", "879999999", "00"],
    ["50", "77817", "879999999", "05"],
];

/// The event of `DISPLAY 'CBPAUP0C PARM RECEIVED :' PRM-INFO`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L192].
const PARM_EVENT: &str = "PARM RECEIVED";

/// Its fields: the `PRM-INFO` receiver padded to 17 bytes and the values `1000-INITIALIZE`
/// derives from it (P-CBPAUP0C-001).
const PARM_FIELDS: [&str; 5] = [
    r#""sysin":"00,00001,00001,Y ""#,
    r#""expiry_days":0"#,
    r#""chkp_freq":1"#,
    r#""chkp_dis_freq":1"#,
    r#""debug":true"#,
];

/// The event of `DISPLAY 'TODAYS DATE            :' CURRENT-YYDDD`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L193], whose `today_yyddd` field is the
/// five-digit day the clock gives (P-CBPAUP0C-012).
const TODAY_EVENT: &str = "TODAYS DATE";

/// The event of the end-of-job `DISPLAY` lines
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L171-178].
const COUNTS_EVENT: &str = "expired pending authorizations purged";

/// Its counts: 5 summaries read and 3 deleted (accounts 2, 12 and 20), 10 details read and 7
/// deleted one by one (P-CBPAUP0C-002, P-CBPAUP0C-003, P-CBPAUP0C-005, P-CBPAUP0C-007).
/// Account 20's unexpired decline goes with its summary and is no detail delete.
const COUNT_FIELDS: [&str; 4] = [
    r#""summaries_read":5"#,
    r#""summaries_deleted":3"#,
    r#""details_read":10"#,
    r#""details_deleted":7"#,
];

/// The `Expected result` column of the parity audit tables.
const EXPECTED_RESULT: &str = "Expected result";

fn account_id(value: u32) -> AccountId {
    AccountId::new(u64::from(value)).expect("a 9(11) account number")
}

fn cents<const D: u8>(value: i64) -> Money<D> {
    Money::from_cents(value).expect("an amount within the field")
}

fn count(value: i16) -> AuthCount {
    AuthCount::new(value).expect("a count of at most four digits")
}

/// The `PENDING-AUTH-SUMMARY` of `input`: its stored counts, an approved amount of 100.00 and a
/// declined amount of 40.00 per authorization counted, and the other fields as `COPAUA0C`
/// `8400-UPDATE-SUMMARY` fills them for an active account.
fn summary(input: &SummaryInput) -> PendingAuthSummary {
    let approved_amt = i64::from(input.approved) * APPROVAL_CENTS;
    PendingAuthSummary {
        pa_acct_id: account_id(input.account),
        pa_cust_id: CustomerId::new(input.account).expect("a 9(09) customer number"),
        pa_auth_status: Alnum::move_from("A"),
        pa_account_status: [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS],
        pa_credit_limit: cents(500_000),
        pa_cash_limit: cents(200_000),
        pa_credit_balance: cents(approved_amt),
        pa_cash_balance: Money::ZERO,
        pa_approved_auth_cnt: count(input.approved),
        pa_declined_auth_cnt: count(input.declined),
        pa_approved_auth_amt: cents(approved_amt),
        pa_declined_auth_amt: cents(i64::from(input.declined) * DECLINE_CENTS),
    }
}

/// The `PENDING-AUTH-DETAILS` segment `input` describes under the summary of `owner`, keyed by
/// the complemented date `COPAUA0C` stores (P-CBPAUP0C-004). An approval answers `00`/`0000`
/// with match status `P`; a decline answers `05`/`4100` (insufficient funds) with match status
/// `D` and approves 0.00. The request fields are those of the audited CP00 example request.
fn detail(owner: &SummaryInput, input: DetailInput) -> NewPendingAuthDetail {
    let (resp_code, resp_reason, match_status, approved_cents, transaction_cents) =
        if input.approved {
            (
                "00",
                "0000",
                MatchStatus::Pending,
                APPROVAL_CENTS,
                APPROVAL_CENTS,
            )
        } else {
            ("05", "4100", MatchStatus::AuthDeclined, 0, DECLINE_CENTS)
        };
    NewPendingAuthDetail {
        pa_acct_id: account_id(owner.account),
        pa_auth_date_9c: AUTH_DATE_COMPLEMENT - input.yyddd,
        pa_auth_time_9c: input.time_9c,
        pa_auth_orig_date: Alnum::move_from(input.orig_date),
        pa_auth_orig_time: Alnum::move_from("120000"),
        pa_card_num: CardNumber::move_from(owner.card),
        pa_auth_type: Alnum::move_from("0100"),
        pa_card_expiry_date: Alnum::move_from("1225"),
        pa_message_type: Alnum::move_from("0100"),
        pa_message_source: Alnum::move_from("POS"),
        pa_auth_id_code: Alnum::move_from("120000"),
        pa_auth_resp_code: Alnum::move_from(resp_code),
        pa_auth_resp_reason: Alnum::move_from(resp_reason),
        pa_processing_code: NumericCode::new(0).expect("a six-digit code"),
        pa_transaction_amt: cents(transaction_cents),
        pa_approved_amt: cents(approved_cents),
        pa_merchant_catagory_code: Alnum::move_from("5411"),
        pa_acqr_country_code: Alnum::move_from("840"),
        pa_pos_entry_mode: NumericCode::new(1).expect("a two-digit code"),
        pa_merchant_id: Alnum::move_from("M00000000000001"),
        pa_merchant_name: Alnum::move_from("FIXTURE MERCHANT"),
        pa_merchant_city: Alnum::move_from("SEATTLE"),
        pa_merchant_state: Alnum::move_from("WA"),
        pa_merchant_zip: Alnum::move_from("98101"),
        pa_transaction_id: Alnum::move_from("TXN000000000001"),
        pa_match_status: match_status,
        pa_auth_fraud: FraudFlag::Blank,
        pa_fraud_rpt_date: Alnum::spaces(),
    }
}

/// Commits [`AUTHORIZATIONS`] in one unit of work through the `AuthorizationStore` port, each
/// summary before its details, as `COPAUA0C` inserts them.
async fn commit_authorizations(db: &Db) {
    let mut uow = db
        .begin()
        .await
        .unwrap_or_else(|error| panic!("cannot begin a unit of work: {error}"));
    for owner in &AUTHORIZATIONS {
        let account = owner.account;
        uow.store()
            .insert_summary(summary(owner))
            .await
            .unwrap_or_else(|error| panic!("summary of account {account} not inserted: {error}"));
        for &input in owner.details {
            if let Err(error) = uow.store().insert_detail(detail(owner, input)).await {
                panic!("detail {input:?} of account {account} not inserted: {error}");
            }
        }
    }
    uow.commit()
        .await
        .unwrap_or_else(|error| panic!("the authorizations do not commit: {error}"));
}

/// The text of every row of `sql`, a null as `NULL`.
fn text_rows(h: &Harness, sql: &str) -> Vec<Vec<String>> {
    block_on(h.stack().fetch_text_rows(sql))
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|value| value.unwrap_or_else(|| "NULL".to_owned()))
                .collect()
        })
        .collect()
}

/// The stdout line of the job's event whose message is `message`.
fn event_line<'o>(stdout: &'o str, message: &str) -> &'o str {
    let marker = format!(r#""message":"{message}""#);
    stdout
        .lines()
        .find(|line| line.contains(&marker))
        .unwrap_or_else(|| panic!("no {message:?} event on stdout:\n{stdout}"))
}

/// Checks that the event `line` holds every one of `fields`.
fn assert_fields(line: &str, fields: &[&str]) {
    for field in fields {
        assert!(line.contains(field), "{field} missing from {line}");
    }
}

/// Checks the audit rows the test rests on: every row of [`P_IDS`] exists with an expected
/// result, P-CBPAUP0C-001 reads the card [`SYSIN`], P-CBPAUP0C-010 reproduces E-16, and
/// P-CBPAUP0C-012 gives `CURRENT-YYDDD` five digits.
fn check_audit_rows() {
    let doc = parity();
    for id in P_IDS {
        let expected = parity_cell(&doc, id, EXPECTED_RESULT);
        assert!(
            !expected.trim().is_empty(),
            "parity row {id} expects nothing"
        );
    }
    let card = parity_cell(&doc, "P-CBPAUP0C-001", "Input condition");
    assert!(
        card.contains(&format!("'{SYSIN}'")),
        "P-CBPAUP0C-001 reads another card: {card}"
    );
    let e16 = parity_cell(&doc, "P-CBPAUP0C-010", EXPECTED_RESULT);
    assert!(
        e16.contains("escalation=E-16") && e16.contains("disposition=reproduced"),
        "P-CBPAUP0C-010 does not reproduce E-16: {e16}"
    );
    let formats = expected_value(&doc, "P-CBPAUP0C-012", "format_regex");
    assert!(
        formats.contains("^[0-9]{5}$"),
        "P-CBPAUP0C-012 gives CURRENT-YYDDD another format: {formats}"
    );
}

#[test]
fn purge_deletes_audited_details_and_summaries() {
    check_audit_rows();
    let h = Harness::new();
    block_on(commit_authorizations(&h.db()));
    let summaries_before = text_rows(&h, SUMMARIES_SQL);
    let details_before = text_rows(&h, DETAILS_SQL);
    assert_eq!(
        summaries_before.len(),
        AUTHORIZATIONS.len(),
        "the seeded state holds no summary of its own: {summaries_before:?}"
    );
    assert_eq!(
        details_before.len(),
        SURVIVING_DETAILS.len() + DELETED_DETAILS.len(),
        "the seeded state holds no detail of its own: {details_before:?}"
    );

    let output = h.run_expect(&["cbpaup0j", "--sysin", SYSIN], Some(CLOCK), SUCCESS);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.is_empty(),
        "a normal end writes no stderr, and no {CBPAUP0C_ABENDING:?}: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_fields(event_line(&stdout, PARM_EVENT), &PARM_FIELDS);
    let today = format!(r#""today_yyddd":{TODAY}"#);
    assert_fields(event_line(&stdout, TODAY_EVENT), &[today.as_str()]);
    assert_fields(event_line(&stdout, COUNTS_EVENT), &COUNT_FIELDS);

    // Read from the test's own pool after the child exited: only committed deletes show
    // (P-CBPAUP0C-008).
    let summaries_after = text_rows(&h, SUMMARIES_SQL);
    assert_eq!(
        summaries_after, SURVIVING_SUMMARIES,
        "accounts 2 and 20 lose their summaries, and so does account 12 with 3 declines left \
         (E-16); accounts 27 and 50 keep theirs"
    );
    assert!(
        summaries_after
            .iter()
            .all(|row| summaries_before.contains(row)),
        "a kept summary keeps its stored counts and amounts (E-35); before the run: \
         {summaries_before:?}"
    );
    let details_after = text_rows(&h, DETAILS_SQL);
    assert_eq!(
        details_after, SURVIVING_DETAILS,
        "only the approvals dated after the clock's day stay under the kept summaries"
    );
    let deleted: Vec<Vec<String>> = details_before
        .into_iter()
        .filter(|row| !details_after.contains(row))
        .collect();
    assert_eq!(
        deleted, DELETED_DETAILS,
        "every expired detail goes, and account 20's unexpired decline with its summary"
    );
}
