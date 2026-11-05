//! CPVD, one pending authorization: `GET /authorizations/{id}` (`COPAUS1C`
//! ENTER with the PF8 step after it) and `PUT /authorizations/{id}` (PF5
//! `MARK-AUTH-FRAUD` with the linked `COPAUS2C`).
//!
//! `READ-AUTH-RECORD` reads the authorization summary of the account, then the
//! selected detail under it, and `POPULATE-AUTH-DETAILS` shows it. PF8
//! (`PROCESS-PF8-KEY`, `READ-NEXT-AUTH-RECORD`) reaches the next twin under the
//! same summary, the next older authorization, and the response names it as
//! `next_id`; past the last one the text `Already at the last
//! Authorization...` is shown instead. PF5 toggles the detail's fraud flag,
//! `F` to `R` and any other value to `F`; `COPAUS2C` stamps the report date of
//! the processing clock and inserts the `AUTHFRDS` report, or updates it when
//! the insert answers SQLCODE -803; then the detail is replaced. The Db2 and
//! IMS writes are recoverable: both persist only at the `SYNCPOINT` after the
//! replace, and `ROLL-BACK` backs out both. `COPAUS2C` is a direct call of its
//! rules, so the LINK failure that rolls back with no message has no
//! counterpart: every failure past the read answers with a text (E-82).
//!
//! The seed loads no pending authorization, so each test writes the summary of
//! account 1 and three details under it, the rows the CP00 consumer writes, in
//! an insertion order unlike their `PAUT9CTS` key order, so the identity order
//! cannot stand in for the twin order. Expected values are read back from the
//! stored rows with `fetch_text_rows`. A literal text comes from the
//! `carddemo_domain::messages` catalog. A STRING-built error text is expanded
//! by `common::audit_text` from the reviewed template of its validation row,
//! with the IMS status (E-44) or the Db2 codes (E-30) of the injected failure
//! in the row's pictures and receiving width, never by the production
//! composers; the catalog head of the text is checked to be the template's
//! leading literal. Statuses, error codes and BMS fields come from the
//! `V-COPAUS1C` rows of the validation audit; every row a test proves is
//! checked to have passed its review first.
//!
//! The API carries the stored fields as stored: the original date `YYMMDD`,
//! time `HHMMSS` and card expiry `MMYY`, which `POPULATE-AUTH-DETAILS` cuts into
//! `MM/DD/YY`, `HH:MM:SS` and `MM/YY`, are cut by the SPA. The approval flag and
//! the decline-reason text are the values the program computes.
//!
//! The 3270 mechanics of `COPAUS1C` (the entry from `COPAUS0C`, the AID keys,
//! PF3, the sent and received map and the screen header) are proven against
//! the REST replacements the parity rows and the function map rows of map
//! `COPAU01` name: the published operations of the `OpenAPI` document and the
//! answers they give.
//!
//! Each HTTP test runs on its own seeded `postgres:16.15` container, so Docker
//! must be running. The store that cannot be opened is met only by calling the
//! handlers directly over a closed pool, because over HTTP the read of the
//! signed-on user reaches the database first.

pub mod common;

use std::collections::BTreeSet;

use axum::extract::State;
use carddemo_api::AppState;
use carddemo_api::auth::{AuthUser, UserType};
use carddemo_api::dto::authorizations::AuthIdPath;
use carddemo_api::error::{ApiError, ApiPath};
use carddemo_api::handlers::authorizations::{get_authorization, toggle_fraud};
use carddemo_audit::{
    Document, FUNCTION_MAP_PATH, IdKind, Repo, Row, Table, code_span, id_tokens, is_no_message,
    key_values,
};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_persistence::Db;
use carddemo_runtime::config::Settings;
use carddemo_testkit::{TestStack, block_on, fixtures};
use chrono::{DateTime, Datelike, NaiveDate};
use common::audit_text::{Operand, Template};
use common::{Resp, TestApp, assert_error, parity_audit, validation_audit, validation_ids};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// The processing clock of every test: the fraud-report date `COPAUS2C` takes
/// from `ASKTIME`/`FORMATTIME` and Db2 `CURRENT DATE` is this instant's day.
const CLOCK: &str = "2022-07-01T12:00:00Z";

/// The card of every detail: the card of account 1 in the cross-reference
/// fixture [app/data/ASCII/cardxref.txt:L49].
const CARD: &str = "9680294154603697";

/// The text of a path that sets no message (E-49, E-53).
const NO_MESSAGE: &str = "";

/// `PA-FRAUD-CONFIRMED`, `PA-AUTH-FRAUD` 88-level `F`
/// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L51].
const FRAUD_CONFIRMED: &str = "F";

/// `PA-FRAUD-REMOVED`, `PA-AUTH-FRAUD` 88-level `R`
/// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L52].
const FRAUD_REMOVED: &str = "R";

/// `PA-AUTH-FRAUD` and `PA-FRAUD-RPT-DATE` of a detail never marked: the
/// spaces CP00 stores, kept right-trimmed.
const NEVER_MARKED: &str = "";

/// `AUTHRSPO` of an authorization whose `PA-AUTH-RESP-CODE` is `00`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L311-317].
const APPROVED: &str = "A";

/// `AUTHRSPO` of every other response code.
const DECLINED: &str = "D";

/// The `PA-AUTH-RESP-CODE` of an approved authorization.
const RESP_APPROVED: &str = "00";

/// The IMS status E-44 sets in an `IMS-RETURN-CODE` slot for any database
/// failure: `AO`.
const IMS_IO_ERROR: &str = "AO";

/// `IMS-RETURN-CODE PIC X(02)`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L79], the status slot of
/// every `COPAUS1C` IMS failure text.
const IMS_RETURN_CODE_WIDTH: usize = 2;

/// `WS-MESSAGE PIC X(80)`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L37], the receiver of
/// every `COPAUS1C` IMS failure text.
const WS_MESSAGE_WIDTH: usize = 80;

/// SQLCODE +100, no row, in the `WS-SQLCODE` slot (E-30).
const SQLCODE_NO_ROW: i32 = 100;

/// SQLSTATE `02000`, no row, as `WS-SQLSTATE PIC +9(09)` holds it (E-30).
const SQLSTATE_NO_ROW: u32 = 2_000;

/// The digits of `WS-SQLCODE PIC +9(06)`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L55].
const WS_SQLCODE_DIGITS: usize = 6;

/// The digits of `WS-SQLSTATE PIC +9(09)`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L56].
const WS_SQLSTATE_DIGITS: usize = 9;

/// `WS-FRD-ACT-MSG PIC X(50)`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L86], the receiver of
/// the `COPAUS2C` fraud-table failure texts.
const WS_FRD_ACT_MSG_WIDTH: usize = 50;

/// `999999999`, from which `COPAUS2C` complements `PA-AUTH-TIME-9C`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L107].
const AUTH_TIME_COMPLEMENT: i64 = 999_999_999;

/// Paths that name no authorization identity: not an integer, or not
/// positive.
const NOT_AUTH_IDS: [&str; 4] = ["abc", "1.5", "0", "-2"];

/// A positive identity no detail holds: `i64::MAX`.
const UNKNOWN_AUTH_ID: &str = "9223372036854775807";

/// The summary of account 1 and three details under it, newest first in key
/// order: 2022-07-01 10:30:15.123 (approved), 2022-07-01 09:15:00.000
/// (declined, reason 4100) and 2022-06-29 23:59:59.999 (declined, a reason the
/// decline table lacks). `PA-AUTH-DATE-9C` is `99999 - YYDDD` and
/// `PA-AUTH-TIME-9C` is `999999999 - HHMMSSmmm`, as `COPAUA0C` stores them
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L874-875]. The oldest is
/// inserted first and the newest second, so the identities are not in twin
/// order.
const INSERT_AUTHORIZATIONS: &str = "\
INSERT INTO pending_auth_summaries
    (pa_acct_id, pa_cust_id, pa_auth_status, pa_account_status, pa_credit_limit,
     pa_cash_limit, pa_credit_balance, pa_cash_balance, pa_approved_auth_cnt,
     pa_declined_auth_cnt, pa_approved_auth_amt, pa_declined_auth_amt)
VALUES (1, 1, '', ARRAY['', '', '', '', ''], 2020.00, 1020.00, 132.40, 0.00, 1, 2,
        132.40, 98.75);
INSERT INTO pending_auth_details
    (pa_acct_id, pa_auth_date_9c, pa_auth_time_9c, pa_auth_orig_date, pa_auth_orig_time,
     pa_card_num, pa_auth_type, pa_card_expiry_date, pa_message_type, pa_message_source,
     pa_auth_id_code, pa_auth_resp_code, pa_auth_resp_reason, pa_processing_code,
     pa_transaction_amt, pa_approved_amt, pa_merchant_catagory_code, pa_acqr_country_code,
     pa_pos_entry_mode, pa_merchant_id, pa_merchant_name, pa_merchant_city,
     pa_merchant_state, pa_merchant_zip, pa_transaction_id, pa_match_status,
     pa_auth_fraud, pa_fraud_rpt_date)
VALUES
    (1, 77819, 764040000, '220629', '235959', '9680294154603697', 'REFD', '0525', '0110',
     'WEB', 'Z9Y8X7', '51', '7777', 200000, 11.50, 0.00, '5999', '124', 81,
     '800000000000003', 'Torp-Stark', 'Vancouver', 'BC', 'V6B1A1', 'TXN000000000003',
     'E', '', ''),
    (1, 77817, 896984876, '220701', '103015', '9680294154603697', 'SALE', '0525', '0100',
     'POS', 'A1B2C3', '00', '0000', 42, 132.40, 132.40, '5411', '840', 5,
     '800000000000001', 'Casper Group', 'Seattle', 'WA', '98101', 'TXN000000000001',
     'P', '', ''),
    (1, 77817, 908499999, '220701', '091500', '9680294154603697', 'SALE', '0525', '0100',
     'POS', 'D4E5F6', '05', '4100', 0, 87.25, 0.00, '5812', '840', 1,
     '800000000000002', 'Frami-Hyatt', 'Portland', 'OR', '97201', 'TXN000000000002',
     'D', '', '')";

/// The response fields of `AuthDetailResponse` that show a stored detail
/// field, in the column order of [`SELECT_DETAILS`].
const DETAIL_FIELDS: [&str; 23] = [
    "auth_id",
    "account_id",
    "card_number",
    "pa_auth_orig_date",
    "pa_auth_orig_time",
    "pa_auth_resp_code",
    "pa_auth_resp_reason",
    "pa_processing_code",
    "pa_approved_amt",
    "pa_pos_entry_mode",
    "pa_message_source",
    "pa_merchant_catagory_code",
    "pa_card_expiry_date",
    "pa_auth_type",
    "pa_transaction_id",
    "pa_match_status",
    "pa_auth_fraud",
    "pa_fraud_rpt_date",
    "pa_merchant_name",
    "pa_merchant_id",
    "pa_merchant_city",
    "pa_merchant_state",
    "pa_merchant_zip",
];

/// Every detail of account 1 in `PAUT9CTS` order, the IMS twin order (newest
/// first), as [`DETAIL_FIELDS`] shows it: the `9(11)` account, `9(06)`
/// processing code and `9(02)` entry mode zero-padded, amounts at scale 2.
const SELECT_DETAILS: &str = "\
SELECT auth_id::text, lpad(pa_acct_id::text, 11, '0'), pa_card_num, pa_auth_orig_date,
       pa_auth_orig_time, pa_auth_resp_code, pa_auth_resp_reason,
       lpad(pa_processing_code::text, 6, '0'), pa_approved_amt::text,
       lpad(pa_pos_entry_mode::text, 2, '0'), pa_message_source, pa_merchant_catagory_code,
       pa_card_expiry_date, pa_auth_type, pa_transaction_id, pa_match_status, pa_auth_fraud,
       pa_fraud_rpt_date, pa_merchant_name, pa_merchant_id, pa_merchant_city,
       pa_merchant_state, pa_merchant_zip
  FROM pending_auth_details
 WHERE pa_acct_id = 1
 ORDER BY pa_auth_date_9c, pa_auth_time_9c";

/// The fraud mark of every detail of account 1, in twin order: identity,
/// `PA-AUTH-FRAUD` and `PA-FRAUD-RPT-DATE`.
const SELECT_MARKS: &str = "\
SELECT auth_id::text, pa_auth_fraud, pa_fraud_rpt_date
  FROM pending_auth_details
 WHERE pa_acct_id = 1
 ORDER BY pa_auth_date_9c, pa_auth_time_9c";

/// The original date and the key time of the newest detail, from which
/// `COPAUS2C` builds `AUTH_TS`.
const SELECT_NEWEST_KEY: &str = "\
SELECT pa_auth_orig_date, pa_auth_time_9c::text
  FROM pending_auth_details
 WHERE pa_acct_id = 1
 ORDER BY pa_auth_date_9c, pa_auth_time_9c
 LIMIT 1";

/// The account and customer the cross-reference names for [`CARD`]: the
/// `WS-FRD-ACCT-ID` and `WS-FRD-CUST-ID` `COPAUS1C` passes to `COPAUS2C`.
const SELECT_CARD_OWNER: &str = "\
SELECT xref_acct_id::text, xref_cust_id::text
  FROM card_xref
 WHERE xref_card_num = '9680294154603697'";

/// Every `AUTHFRDS` report by its key, with the columns `COPAUS2C` sets
/// itself: card, `AUTH_TS` to the microsecond, `AUTH_FRAUD`,
/// `FRAUD_RPT_DATE`, `ACCT_ID` and `CUST_ID`.
const SELECT_FRAUD_KEYS: &str = "\
SELECT card_num, to_char(auth_ts, 'YYYY-MM-DD HH24:MI:SS.US'), auth_fraud,
       to_char(fraud_rpt_date, 'YYYY-MM-DD'), acct_id::text, cust_id::text
  FROM auth_fraud
 ORDER BY card_num, auth_ts";

/// The `AUTHFRDS` columns `COPAUS2C` moves from the detail, in the order of
/// [`SELECT_NEWEST_COPIED`].
const SELECT_FRAUD_COPIED: &str = "\
SELECT auth_type, card_expiry_date, message_type, message_source, auth_id_code,
       auth_resp_code, auth_resp_reason, processing_code, transaction_amt::text,
       approved_amt::text, merchant_catagory_code, acqr_country_code, pos_entry_mode::text,
       merchant_id, merchant_name, merchant_city, merchant_state, merchant_zip,
       transaction_id, match_status
  FROM auth_fraud
 ORDER BY card_num, auth_ts";

/// The detail fields of the newest detail that `COPAUS2C` moves to `AUTHFRDS`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L113-136], the
/// `9(06)` processing code as its six digits.
const SELECT_NEWEST_COPIED: &str = "\
SELECT pa_auth_type, pa_card_expiry_date, pa_message_type, pa_message_source,
       pa_auth_id_code, pa_auth_resp_code, pa_auth_resp_reason,
       lpad(pa_processing_code::text, 6, '0'), pa_transaction_amt::text,
       pa_approved_amt::text, pa_merchant_catagory_code, pa_acqr_country_code,
       pa_pos_entry_mode::text, pa_merchant_id, pa_merchant_name, pa_merchant_city,
       pa_merchant_state, pa_merchant_zip, pa_transaction_id, pa_match_status
  FROM pending_auth_details
 WHERE pa_acct_id = 1
 ORDER BY pa_auth_date_9c, pa_auth_time_9c
 LIMIT 1";

/// A trigger function that fails the row it fires for with SQLSTATE `P0001`,
/// a database failure with no Db2 counterpart.
const CREATE_WRITE_FAULT: &str = "\
CREATE FUNCTION cpvd_write_fault() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'injected CPVD write fault';
END
$$";

/// Fails every `AUTHFRDS` insert and update.
const FAIL_FRAUD_WRITES: &str = "\
CREATE TRIGGER cpvd_fail_fraud_writes BEFORE INSERT OR UPDATE ON auth_fraud
    FOR EACH ROW EXECUTE FUNCTION cpvd_write_fault()";

/// Removes [`FAIL_FRAUD_WRITES`].
const RESTORE_FRAUD_WRITES: &str = "DROP TRIGGER cpvd_fail_fraud_writes ON auth_fraud";

/// A trigger function that answers the insert it fires for as a duplicate key,
/// SQLSTATE `23505`, the Db2 SQLCODE -803 that sends `COPAUS2C` to
/// `FRAUD-UPDATE`, although no report exists to update.
const CREATE_DUPLICATE_FAULT: &str = "\
CREATE FUNCTION cpvd_duplicate_fault() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'injected duplicate fraud report' USING ERRCODE = 'unique_violation';
END
$$";

/// Answers every `AUTHFRDS` insert as a duplicate key.
const DUPLICATE_FRAUD_INSERTS: &str = "\
CREATE TRIGGER cpvd_duplicate_fraud_inserts BEFORE INSERT ON auth_fraud
    FOR EACH ROW EXECUTE FUNCTION cpvd_duplicate_fault()";

/// Removes [`DUPLICATE_FRAUD_INSERTS`].
const RESTORE_FRAUD_INSERTS: &str = "DROP TRIGGER cpvd_duplicate_fraud_inserts ON auth_fraud";

/// Fails every replace of a detail, the `REPL PAUTDTL1` of
/// `UPDATE-AUTH-DETAILS`, after `COPAUS2C` has written its report.
const FAIL_DETAIL_REPLACES: &str = "\
CREATE TRIGGER cpvd_fail_detail_replaces BEFORE UPDATE ON pending_auth_details
    FOR EACH ROW EXECUTE FUNCTION cpvd_write_fault()";

/// Removes [`FAIL_DETAIL_REPLACES`].
const RESTORE_DETAIL_REPLACES: &str =
    "DROP TRIGGER cpvd_fail_detail_replaces ON pending_auth_details";

/// Hides the authorization summaries: the summary read then fails on the
/// server with SQLSTATE `42P01`, while the details stay readable.
const HIDE_SUMMARIES: &str =
    "ALTER TABLE pending_auth_summaries RENAME TO pending_auth_summaries_off";

/// Puts the summaries back.
const RESTORE_SUMMARIES: &str =
    "ALTER TABLE pending_auth_summaries_off RENAME TO pending_auth_summaries";

/// Hides the authorization details: the summary read, which joins the detail
/// to its summary, and the detail read both fail with SQLSTATE `42P01`.
const HIDE_DETAILS: &str = "ALTER TABLE pending_auth_details RENAME TO pending_auth_details_off";

/// Puts the details back.
const RESTORE_DETAILS: &str = "ALTER TABLE pending_auth_details_off RENAME TO pending_auth_details";

/// Puts a view in place of the details whose oldest row, original date
/// `220629`, fails as it is read, while every other row reads as stored: the
/// view and PF8 read of the newer details succeed, and only the step to the
/// oldest one fails. Each column keeps its type, length and collation, so a
/// statement prepared on the table reads the view with the same row type.
const BREAK_OLDEST_DETAIL: &str = r#"
ALTER TABLE pending_auth_details RENAME TO pending_auth_details_off;
CREATE FUNCTION cpvd_read_fault() RETURNS varchar LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'injected CPVD next-read fault';
END
$$;
CREATE VIEW pending_auth_details AS
SELECT auth_id, pa_acct_id, pa_auth_date_9c, pa_auth_time_9c, pa_auth_orig_date,
       pa_auth_orig_time, pa_card_num, pa_auth_type, pa_card_expiry_date, pa_message_type,
       pa_message_source, pa_auth_id_code, pa_auth_resp_code, pa_auth_resp_reason,
       pa_processing_code, pa_transaction_amt, pa_approved_amt, pa_merchant_catagory_code,
       pa_acqr_country_code, pa_pos_entry_mode, pa_merchant_id, pa_merchant_name,
       pa_merchant_city, pa_merchant_state, pa_merchant_zip, pa_transaction_id,
       pa_match_status,
       CAST(CASE WHEN pa_auth_orig_date = '220629' THEN cpvd_read_fault()
                 ELSE pa_auth_fraud END AS varchar(1)) COLLATE "C" AS pa_auth_fraud,
       pa_fraud_rpt_date
  FROM pending_auth_details_off"#;

/// Removes [`BREAK_OLDEST_DETAIL`] and puts the details back.
const RESTORE_OLDEST_DETAIL: &str = "
DROP VIEW pending_auth_details;
DROP FUNCTION cpvd_read_fault();
ALTER TABLE pending_auth_details_off RENAME TO pending_auth_details";

/// The `PA-FRAUD-RPT-DATE` format the parity audit records for the
/// `FORMATTIME MMDDYY DATESEP` text (P-COPAUS2C-006).
const REPORT_DATE_FORMAT: &str = "^[0-9]{2}/[0-9]{2}/[0-9]{2}$";

/// The BMS map of `COPAUS1C`, as the function map names it.
const MAP: &str = "COPAU01";

/// The program whose validation rows name the cursor field.
const PROGRAM: &str = "COPAUS1C";

/// The CPVD route of `GET` and `PUT`, as the `OpenAPI` document keys it.
const DETAIL_ROUTE: &str = "/authorizations/{id}";

/// `MAIN-PARA` on the first entry from `COPAUS0C`: `PROCESS-ENTER-KEY`, then
/// the detail is sent.
const FIRST_ENTRY_ROW: &str = "P-COPAUS1C-001";

/// `MAIN-PARA`'s `EVALUATE EIBAID`: each AID key and its action.
const AID_ROW: &str = "P-COPAUS1C-002";

/// `RETURN-TO-PREV-SCREEN`: PF3, and the entry without a COMMAREA.
const RETURN_ROW: &str = "P-COPAUS1C-010";

/// `SEND-AUTHVIEW-SCREEN`: the message, the cursor and the erase choice.
const SEND_ROW: &str = "P-COPAUS1C-011";

/// `RECEIVE-AUTHVIEW-SCREEN`: the received map no field of which is read.
const RECEIVE_ROW: &str = "P-COPAUS1C-012";

/// `POPULATE-HEADER-INFO`: the 3270 header.
const HEADER_ROW: &str = "P-COPAUS1C-013";

/// The word [`AID_ROW`] names every AID key it answers with the invalid-key
/// text by.
const OTHER_KEY: &str = "other";

/// AID keys `MAIN-PARA` answers `WHEN OTHER`.
const OTHER_KEYS: [&str; 3] = ["PF9", "PA1", "CLEAR"];

/// The query parameter a request would name an AID key in: one neither CPVD
/// operation publishes.
const AID_MEMBER: &str = "aid";

/// The bearer of a request without a session: none, or one that does not
/// verify.
const NO_SESSION: [Option<&str>; 2] = [None, Some("not-a-token")];

/// The `code` of the 401 a request without a session answers.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// The keys of an `OpenAPI` path item that name operations.
const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// The response schemas of the CPVD operations.
const RESPONSE_SCHEMAS: [&str; 3] = ["AuthDetailResponse", "FraudToggleResponse", "ErrorBody"];

/// The response member of `PA-FRAUD-RPT-DATE`, the `COPAUS2C` report date
/// (P-COPAUS2C-006), whose `MM/DD/YY` shares the picture of the header date.
const REPORT_DATE_MEMBER: &str = "pa_fraud_rpt_date";

/// Account 2 of the cross-reference fixture [app/data/ASCII/cardxref.txt:L3],
/// which holds no authorization here: a map field naming it must not be read.
const OTHER_ACCOUNT: &str = "00000000002";

/// The parity rows these tests prove, each checked to have passed review.
const PROVEN_PARITY_ROWS: [&str; 23] = [
    "P-COPAUS1C-001",
    "P-COPAUS1C-002",
    "P-COPAUS1C-003",
    "P-COPAUS1C-004",
    "P-COPAUS1C-005",
    "P-COPAUS1C-006",
    "P-COPAUS1C-007",
    "P-COPAUS1C-008",
    "P-COPAUS1C-009",
    "P-COPAUS1C-010",
    "P-COPAUS1C-011",
    "P-COPAUS1C-012",
    "P-COPAUS1C-013",
    "P-COPAUS1C-014",
    "P-COPAUS1C-015",
    "P-COPAUS1C-016",
    "P-COPAUS1C-017",
    "P-COPAUS2C-001",
    "P-COPAUS2C-002",
    "P-COPAUS2C-003",
    "P-COPAUS2C-004",
    "P-COPAUS2C-005",
    "P-COPAUS2C-006",
];

/// Rows as `fetch_text_rows` returns them.
type Rows = Vec<Vec<Option<String>>>;

/// The answer a validation audit row assigns.
struct Audited {
    /// The `Status` cell.
    status: StatusCode,
    /// The `kind=` of the `Trigger condition` cell, such as `NotFound`.
    kind: String,
    /// The `BMS field` cell.
    field: String,
    /// The `Message text` cell.
    message: String,
}

/// The message [`assert_row`] holds the answer of a validation audit row to.
#[derive(Clone, Copy)]
enum Expected<'a> {
    /// A STRING-built text: the reviewed template of the row, whose leading
    /// literal is the catalog head `head`, expanded with `operands` and cut at
    /// the `receiver` bytes of the receiving field.
    Expanded {
        /// The `carddemo_domain::messages` head the template must lead with.
        head: &'static str,
        /// The template's fields, each holding the value the injected failure
        /// sets.
        operands: &'a [Operand],
        /// The width of the `PIC X` field the text is built into.
        receiver: usize,
    },
    /// `text` in place of the reviewed template of the row, whose leading
    /// literal is the catalog head `head`: the injected failure has no code
    /// the template's slots can carry (E-30).
    Fallback {
        /// The `carddemo_domain::messages` head the template must lead with.
        head: &'static str,
        /// The whole message the answer carries instead.
        text: &'static str,
    },
    /// A `no-message` row: an empty message, on the body and on the field.
    NoMessage,
}

/// The day of [`CLOCK`], as `COPAUS2C` stamps it.
struct ReportDate {
    /// The date itself, whose century completes the two-digit `AUTH_TS` year.
    date: NaiveDate,
    /// `PA-FRAUD-RPT-DATE`, `FORMATTIME MMDDYY DATESEP`: `mm/dd/yy`.
    screen: String,
    /// `FRAUD_RPT_DATE`, Db2 `CURRENT DATE`, as `to_char` gives it: `YYYY-MM-DD`.
    column: String,
}

/// Asserts that the audit row `id` passed its COBOL-informed review: a review
/// log of `doc` gives it the verdict `pass`. Only reviewed rows decide a test.
fn assert_reviewed(doc: &Document, id: &str) {
    let passed = doc
        .tables()
        .iter()
        .filter(|table| table.is_review_log())
        .any(|table| {
            table.rows.iter().any(|row| {
                table.cell(row, "Row ID") == Some(id) && table.cell(row, "Verdict") == Some("pass")
            })
        });
    assert!(passed, "{id} has no passing review verdict");
}

/// The validation audit row `id`, once it has passed review.
fn audited(doc: &Document, id: &str) -> Audited {
    assert_reviewed(doc, id);
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the validation audit has no {id} row"));
    let cell = |name: &str| {
        table
            .cell(row, name)
            .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
            .trim()
    };
    let status = cell("Status")
        .parse::<u16>()
        .ok()
        .and_then(|code| StatusCode::from_u16(code).ok())
        .unwrap_or_else(|| panic!("{id} has no HTTP status: {}", cell("Status")));
    let kind = key_values(cell("Trigger condition"))
        .into_iter()
        .find(|pair| pair.key == "kind")
        .map_or_else(|| panic!("{id} names no kind"), |pair| pair.value);
    Audited {
        status,
        kind,
        field: cell("BMS field").to_owned(),
        message: cell("Message text").to_owned(),
    }
}

/// The error-body `code` of the error kind `kind` of row `id`.
fn error_code(id: &str, kind: &str) -> &'static str {
    let kind = match kind {
        "Invalid" => ErrorKind::Invalid,
        "Empty" => ErrorKind::Empty,
        "NotFound" => ErrorKind::NotFound,
        "Unexpected" => ErrorKind::Unexpected,
        other => panic!("{id} records no error kind but `{other}`"),
    };
    kind.code()
}

/// The reviewed template of the validation row `id` of `doc`, once its leading
/// literal is asserted to be the catalog head `head`.
fn template_led_by(doc: &Document, id: &str, head: &str) -> Template {
    let template = Template::load(doc, id);
    assert!(
        template.text().starts_with(head) && template.literals().first() == Some(&head),
        "{id}: the catalog head {head:?} is not the leading literal of the template {:?}",
        template.text()
    );
    template
}

/// Asserts that `resp` is the error answer of the validation audit row `id`:
/// its status, the code of its kind, and the whole message `expected` derives,
/// byte for byte, both as the body's message and as the only entry, on the
/// row's BMS field.
///
/// The message is the row's template expanded by `common::audit_text`, the
/// fallback text in its place, or empty for a `no-message` row; it is never
/// built with the composers of `carddemo_domain::messages`.
fn assert_row(resp: &Resp, doc: &Document, id: &str, expected: Expected<'_>) {
    let row = audited(doc, id);
    let message = match expected {
        Expected::Expanded {
            head,
            operands,
            receiver,
        } => template_led_by(doc, id, head).expand(operands, receiver),
        Expected::Fallback { head, text } => {
            template_led_by(doc, id, head);
            text.to_owned()
        }
        Expected::NoMessage => {
            assert!(
                is_no_message(&row.message),
                "{id} records a message: {}",
                row.message
            );
            NO_MESSAGE.to_owned()
        }
    };
    assert_error(
        resp,
        row.status,
        error_code(id, &row.kind),
        &message,
        &[(row.field.as_str(), message.as_str())],
    );
}

/// [`assert_row`] for a `no-message` row: the body's message and its field's
/// message are both empty.
fn assert_no_message(resp: &Resp, doc: &Document, id: &str) {
    assert_row(resp, doc, id, Expected::NoMessage);
}

/// [`assert_row`] for a `COPAUS1C` IMS failure row whose template leads with
/// the catalog head `head`: the template expanded with `IMS-RETURN-CODE`
/// holding the status E-44 sets, [`IMS_IO_ERROR`], into `WS-MESSAGE`.
fn assert_ims_row(resp: &Resp, doc: &Document, id: &str, head: &'static str) {
    assert_row(
        resp,
        doc,
        id,
        Expected::Expanded {
            head,
            operands: &[Operand::alphanumeric(
                "IMS-RETURN-CODE",
                IMS_IO_ERROR,
                IMS_RETURN_CODE_WIDTH,
            )],
            receiver: WS_MESSAGE_WIDTH,
        },
    );
}

/// The `Expected result` cell of the parity audit row `id`, once it has
/// passed review.
fn expected_result(doc: &Document, id: &str) -> String {
    assert_reviewed(doc, id);
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    table
        .cell(row, "Expected result")
        .unwrap_or_else(|| panic!("{id} has no `Expected result` cell"))
        .to_owned()
}

/// COPAUS1C POPULATE-AUTH-DETAILS — P-COPAUS1C-005: `AUTHRSNO` for the stored
/// response reason `reason`, from the decline table the row lists inside
/// `SEARCH ALL (…)` as `<code> <text>` pairs: the code, `-` and the text, or
/// the `missing -> '…'` text for a code the table lacks.
fn decline_reason(parity: &Document, reason: &str) -> String {
    let expected = expected_result(parity, "P-COPAUS1C-005");
    let table = expected
        .split_once("SEARCH ALL (")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map_or_else(
            || panic!("P-COPAUS1C-005 lists no decline table: {expected}"),
            |(table, _)| table,
        );
    let listed = table.split(", ").find_map(|entry| {
        let text = entry.strip_prefix(reason)?.strip_prefix(' ')?;
        Some(format!("{reason}-{text}"))
    });
    listed.unwrap_or_else(|| {
        expected
            .split_once("missing -> '")
            .and_then(|(_, rest)| rest.split_once('\''))
            .map_or_else(
                || panic!("P-COPAUS1C-005 gives no text for a missing code: {expected}"),
                |(text, _)| text.to_owned(),
            )
    })
}

/// The day of [`CLOCK`].
fn report_date() -> ReportDate {
    let date = DateTime::parse_from_rfc3339(CLOCK)
        .unwrap_or_else(|error| panic!("{CLOCK} is not an RFC 3339 instant: {error}"))
        .date_naive();
    ReportDate {
        date,
        screen: date.format("%m/%d/%y").to_string(),
        column: date.format("%Y-%m-%d").to_string(),
    }
}

/// Whether `text` has the shape of [`REPORT_DATE_FORMAT`]: two digits, `/`,
/// two digits, `/`, two digits.
fn is_report_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 8
        && bytes.iter().enumerate().all(|(at, byte)| {
            if at == 2 || at == 5 {
                *byte == b'/'
            } else {
                byte.is_ascii_digit()
            }
        })
}

/// The `len` bytes of `text` from `from`.
fn slice(text: &str, from: usize, len: usize) -> &str {
    text.get(from..from + len)
        .unwrap_or_else(|| panic!("{text:?} has no bytes {from}..{}", from + len))
}

/// COPAUS2C MAIN-PARA — P-COPAUS2C-001: the `AUTH_TS` of a detail whose
/// `PA-AUTH-ORIG-DATE` is `orig_date` (`YYMMDD`) and whose `PA-AUTH-TIME-9C`
/// is `time_9c`, as [`SELECT_FRAUD_KEYS`] shows it. `TIMESTAMP_FORMAT` with
/// `YY-MM-DD HH24.MI.SSNNNNNN` completes the year with the century of the
/// report date, and `999999999 - PA-AUTH-TIME-9C` gives `HHMMSSmmm`, whose
/// milliseconds and the `000` filler are read as microseconds.
fn auth_ts(orig_date: &str, time_9c: &str, report: NaiveDate) -> String {
    let key: i64 = time_9c
        .parse()
        .unwrap_or_else(|error| panic!("PA-AUTH-TIME-9C {time_9c:?} is not an integer: {error}"));
    let time = format!("{:09}", AUTH_TIME_COMPLEMENT - key);
    format!(
        "{}{}-{}-{} {}:{}:{}.{}000",
        report.year() / 100,
        slice(orig_date, 0, 2),
        slice(orig_date, 2, 2),
        slice(orig_date, 4, 2),
        slice(&time, 0, 2),
        slice(&time, 2, 2),
        slice(&time, 4, 2),
        slice(&time, 6, 3),
    )
}

/// `/authorizations/{id}`.
fn path(id: &str) -> String {
    format!("/authorizations/{id}")
}

/// `cells` as a row of [`Rows`].
fn row_of(cells: &[&str]) -> Vec<Option<String>> {
    cells.iter().map(|cell| Some((*cell).to_owned())).collect()
}

/// The text of column `index` of a stored row.
fn column(row: &[Option<String>], index: usize) -> &str {
    row.get(index)
        .and_then(Option::as_deref)
        .unwrap_or_else(|| panic!("no text in column {index} of {row:?}"))
}

/// The stored text of the response field `field` in a row of
/// [`SELECT_DETAILS`].
fn stored_field<'a>(row: &'a [Option<String>], field: &str) -> &'a str {
    let index = DETAIL_FIELDS
        .iter()
        .position(|name| *name == field)
        .unwrap_or_else(|| panic!("`{field}` is not one of DETAIL_FIELDS"));
    column(row, index)
}

/// The identities of `details`, rows of [`SELECT_DETAILS`], in their order.
fn identities(details: &[Vec<Option<String>>]) -> Vec<String> {
    details
        .iter()
        .map(|row| stored_field(row, "auth_id").to_owned())
        .collect()
}

/// Writes [`INSERT_AUTHORIZATIONS`] and returns the three details as
/// [`SELECT_DETAILS`] reads them, in twin order.
async fn insert_authorizations(app: &TestApp) -> Rows {
    app.stack().exec_sql(INSERT_AUTHORIZATIONS).await;
    let details = app.stack().fetch_text_rows(SELECT_DETAILS).await;
    assert_eq!(
        details.len(),
        3,
        "the fixture holds three details: {details:?}"
    );
    details
}

/// The fraud marks of the details, [`SELECT_MARKS`], and every `AUTHFRDS`
/// report key, [`SELECT_FRAUD_KEYS`].
async fn stored_marks(app: &TestApp) -> (Rows, Rows) {
    let marks = app.stack().fetch_text_rows(SELECT_MARKS).await;
    let reports = app.stack().fetch_text_rows(SELECT_FRAUD_KEYS).await;
    (marks, reports)
}

/// PF5 on `path`: `PUT` with no request data, as PF5 acts on the record shown.
async fn press_pf5(app: &TestApp, token: &str, path: &str) -> Resp {
    app.put(path, Some(token), &json!({})).await
}

/// Asserts that the `GET` answer `body` shows `stored`, a row of
/// [`SELECT_DETAILS`], as `POPULATE-AUTH-DETAILS` does (P-COPAUS1C-005): each
/// stored field as stored, `approved` from the response code and
/// `decline_reason` from the decline table.
fn assert_shows(body: &Value, stored: &[Option<String>], parity: &Document, raw: &str) {
    for (index, field) in DETAIL_FIELDS.iter().enumerate() {
        assert_eq!(
            body[*field].as_str(),
            Some(column(stored, index)),
            "`{field}` of {raw}"
        );
    }
    let approved = if stored_field(stored, "pa_auth_resp_code") == RESP_APPROVED {
        APPROVED
    } else {
        DECLINED
    };
    assert_eq!(
        body["approved"].as_str(),
        Some(approved),
        "`approved` of {raw}"
    );
    let reason = decline_reason(parity, stored_field(stored, "pa_auth_resp_reason"));
    assert_eq!(
        body["decline_reason"].as_str(),
        Some(reason.as_str()),
        "`decline_reason` of {raw}"
    );
}

/// V-COPAUS1C-004, V-COPAUS1C-007: right after the seed no authorization is
/// pending, so ENTER and PF5 on the first identity find no summary: 404 with
/// no message, and PF5 writes no fraud report (E-53).
async fn nothing_pending_after_seed(app: &TestApp, token: &str, validation: &Document) {
    let first = path("1");
    let viewed = app.get(&first, Some(token)).await;
    assert_no_message(&viewed, validation, "V-COPAUS1C-004");
    let marked = press_pf5(app, token, &first).await;
    assert_no_message(&marked, validation, "V-COPAUS1C-007");
    assert_eq!(
        stored_marks(app).await,
        (Vec::new(), Vec::new()),
        "nothing is pending and PF5 must have reported nothing"
    );
}

/// COPAUS1C PROCESS-ENTER-KEY, READ-AUTH-RECORD — P-COPAUS1C-003,
/// V-COPAUS1C-001, V-COPAUS1C-005, V-COPAUS1C-007: a path that names no
/// authorization identity is the COBOL path with no selected key, 400 with no
/// message; an identity no detail holds is not found, 404 with no message; and
/// neither PF5 writes anything.
async fn rejected_keys(app: &TestApp, token: &str, validation: &Document, ids: &[String]) {
    let before = stored_marks(app).await;
    for entered in NOT_AUTH_IDS {
        let viewed = app.get(&path(entered), Some(token)).await;
        assert_no_message(&viewed, validation, "V-COPAUS1C-001");
        let marked = press_pf5(app, token, &path(entered)).await;
        assert_no_message(&marked, validation, "V-COPAUS1C-001");
    }
    assert!(
        !ids.iter().any(|id| id == UNKNOWN_AUTH_ID),
        "{UNKNOWN_AUTH_ID} must be an identity no detail holds: {ids:?}"
    );
    let unknown = path(UNKNOWN_AUTH_ID);
    let viewed = app.get(&unknown, Some(token)).await;
    assert_no_message(&viewed, validation, "V-COPAUS1C-005");
    let marked = press_pf5(app, token, &unknown).await;
    assert_no_message(&marked, validation, "V-COPAUS1C-007");
    assert_eq!(
        stored_marks(app).await,
        before,
        "a rejected or unknown key must change no mark and report nothing"
    );
}

/// COPAUS1C POPULATE-AUTH-DETAILS, PROCESS-PF8-KEY, READ-NEXT-AUTH-RECORD —
/// P-COPAUS1C-003, P-COPAUS1C-005, P-COPAUS1C-009, P-COPAUS1C-014,
/// V-COPAUS1C-012: ENTER on the newest detail, then PF8 through each
/// `next_id`, newest first in `PAUT9CTS` order whatever the identities, to the
/// oldest, where no twin follows: 200 with `Already at the last
/// Authorization...` and no `next_id`.
async fn page_forward(
    app: &TestApp,
    token: &str,
    details: &[Vec<Option<String>>],
    validation: &Document,
    parity: &Document,
) {
    let ids = identities(details);
    let at_end = audited(validation, "V-COPAUS1C-012");
    assert!(
        at_end
            .message
            .contains(messages::ALREADY_AT_THE_LAST_AUTHORIZATION),
        "V-COPAUS1C-012 does not quote the last-authorization text: {}",
        at_end.message
    );
    let mut visited: Vec<String> = Vec::new();
    let mut next = ids.first().cloned();
    while let Some(id) = next {
        let stored = details
            .get(visited.len())
            .unwrap_or_else(|| panic!("PF8 went past the last detail after {visited:?}"));
        let (status, body, raw) = app.get(&path(&id), Some(token)).await;
        assert_shows(&body, stored, parity, &raw);
        let following = ids.get(visited.len() + 1);
        assert_eq!(body["next_id"], json!(following), "`next_id` of {raw}");
        if following.is_some() {
            assert_eq!(status, StatusCode::OK, "GET of {id}: {raw}");
            assert!(
                body.get("message").is_none(),
                "no text before the last: {raw}"
            );
        } else {
            assert_eq!(status, at_end.status, "GET of the last detail {id}: {raw}");
            assert_eq!(
                body["message"].as_str(),
                Some(messages::ALREADY_AT_THE_LAST_AUTHORIZATION),
                "`message` of the last detail: {raw}"
            );
        }
        next = body["next_id"].as_str().map(str::to_owned);
        visited.push(id);
    }
    assert_eq!(visited, ids, "PF8 must visit every detail in twin order");
}

/// The `AUTHFRDS` report key row [`SELECT_FRAUD_KEYS`] gives for the newest
/// detail marked with `flag` on `report`: P-COPAUS2C-001 and P-COPAUS2C-002,
/// with the account and customer the card's cross-reference names.
async fn expected_report(app: &TestApp, flag: &str, report: &ReportDate) -> Vec<Option<String>> {
    let key = app.stack().fetch_text_rows(SELECT_NEWEST_KEY).await;
    let owner = app.stack().fetch_text_rows(SELECT_CARD_OWNER).await;
    let (Some(key), Some(owner)) = (key.first(), owner.first()) else {
        panic!("no newest detail ({key:?}) or no cross-reference of {CARD} ({owner:?})");
    };
    let auth_ts = auth_ts(column(key, 0), column(key, 1), report.date);
    row_of(&[
        CARD,
        &auth_ts,
        flag,
        &report.column,
        column(owner, 0),
        column(owner, 1),
    ])
}

/// COPAUS1C MARK-AUTH-FRAUD, UPDATE-AUTH-DETAILS with COPAUS2C —
/// P-COPAUS1C-006 to P-COPAUS1C-008, P-COPAUS1C-015, P-COPAUS2C-001 to
/// P-COPAUS2C-004, P-COPAUS2C-006: three PF5 presses on the newest detail
/// mark it `F`, remove the mark (`R`) and mark it again. Each commits the
/// replaced detail with the report date of the clock and the one `AUTHFRDS`
/// report, inserted by the first press and updated in place by the others
/// (the -803 path); the other details stay unmarked.
async fn toggle_three_times(app: &TestApp, token: &str, ids: &[String], parity: &Document) {
    let report = report_date();
    let format = expected_result(parity, "P-COPAUS2C-006");
    assert!(
        format.contains(REPORT_DATE_FORMAT) && is_report_date(&report.screen),
        "P-COPAUS2C-006 records {format}, the clock gives {}",
        report.screen
    );
    let newest = &ids[0];
    let presses = [
        (FRAUD_CONFIRMED, messages::AUTH_MARKED_FRAUD),
        (FRAUD_REMOVED, messages::AUTH_FRAUD_REMOVED),
        (FRAUD_CONFIRMED, messages::AUTH_MARKED_FRAUD),
    ];
    for (flag, message) in presses {
        let (status, body, raw) = press_pf5(app, token, &path(newest)).await;
        assert_eq!(status, StatusCode::OK, "PF5 on {newest}: {raw}");
        assert_eq!(body["message"].as_str(), Some(message), "{raw}");
        let detail = &body["detail"];
        assert_eq!(detail["auth_id"].as_str(), Some(newest.as_str()), "{raw}");
        assert_eq!(detail["pa_auth_fraud"].as_str(), Some(flag), "{raw}");
        assert_eq!(
            detail["pa_fraud_rpt_date"].as_str(),
            Some(report.screen.as_str()),
            "{raw}"
        );
        assert_eq!(detail["next_id"], Value::Null, "{raw}");
        let marks: Rows = ids
            .iter()
            .map(|id| {
                if id == newest {
                    row_of(&[id, flag, &report.screen])
                } else {
                    row_of(&[id, NEVER_MARKED, NEVER_MARKED])
                }
            })
            .collect();
        let reports = vec![expected_report(app, flag, &report).await];
        assert_eq!(
            stored_marks(app).await,
            (marks, reports),
            "after the PF5 that answered {message:?}"
        );
    }
    assert_eq!(
        app.stack().fetch_text_rows(SELECT_FRAUD_COPIED).await,
        app.stack().fetch_text_rows(SELECT_NEWEST_COPIED).await,
        "P-COPAUS2C-002: the report must hold the detail's own fields"
    );
}

/// The `FORMATTIME` text, the report date and the copies of a fraud mark come
/// from the parity rows; the audited answers of ENTER, PF8 and PF5 from the
/// validation rows.
#[test]
fn the_detail_pages_forward_and_pf5_toggles_its_fraud_mark() {
    let validation = validation_audit();
    let parity = parity_audit();
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        let token = app.user_token().await;
        nothing_pending_after_seed(&app, &token, &validation).await;
        let details = insert_authorizations(&app).await;
        let ids = identities(&details);
        rejected_keys(&app, &token, &validation, &ids).await;
        page_forward(&app, &token, &details, &validation, &parity).await;
        toggle_three_times(&app, &token, &ids, &parity).await;
        let (status, body, raw) = app.get(&path(&ids[0]), Some(&token)).await;
        assert_eq!(status, StatusCode::OK, "{raw}");
        let stored = app.stack().fetch_text_rows(SELECT_DETAILS).await;
        assert_shows(&body, &stored[0], &parity, &raw);
        assert_eq!(
            body["pa_auth_fraud"].as_str(),
            Some(FRAUD_CONFIRMED),
            "{raw}"
        );
    });
}

/// COPAUS1C UPDATE-AUTH-DETAILS, ROLL-BACK — V-COPAUS1C-011, P-COPAUS1C-008,
/// P-COPAUS1C-015: with every detail replace failing, PF5 on `path` answers
/// the fraud-tagging text with the IMS status E-44 sets, and the rollback
/// backs out the `AUTHFRDS` write `COPAUS2C` made before it: the marks and the
/// reports stay `before`.
async fn replace_fails(
    app: &TestApp,
    token: &str,
    validation: &Document,
    path: &str,
    before: &(Rows, Rows),
) {
    app.stack().exec_sql(FAIL_DETAIL_REPLACES).await;
    let failed = press_pf5(app, token, path).await;
    assert_ims_row(
        &failed,
        validation,
        "V-COPAUS1C-011",
        messages::SYSTEM_ERROR_WHILE_FRAUD_TAGGING_ROLLBACK,
    );
    assert_eq!(
        &stored_marks(app).await,
        before,
        "a failed replace must back out the fraud report too"
    );
    app.stack().exec_sql(RESTORE_DETAIL_REPLACES).await;
}

/// COPAUS1C MARK-AUTH-FRAUD, ROLL-BACK with COPAUS2C — P-COPAUS1C-007,
/// P-COPAUS1C-008, P-COPAUS1C-015, P-COPAUS2C-004, P-COPAUS2C-005: a fraud mark
/// whose `AUTHFRDS` write or detail replace fails answers the audited failure
/// and leaves the detail's flag and every report as they were, because both
/// writes persist only together.
#[test]
fn a_fraud_mark_that_fails_to_write_rolls_back_both_writes() {
    let validation = validation_audit();
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        let token = app.user_token().await;
        let ids = identities(&insert_authorizations(&app).await);
        let newest = path(&ids[0]);
        app.stack().exec_sql(CREATE_WRITE_FAULT).await;
        app.stack().exec_sql(CREATE_DUPLICATE_FAULT).await;
        let unmarked = stored_marks(&app).await;

        // V-COPAUS1C-009: the insert fails with SQLSTATE P0001, which has no
        // Db2 SQLCODE and does not fit the `WS-SQLSTATE PIC +9(09)` slot, so
        // the codes are not pinned (E-30) and the unexpected-error text is
        // shown in place of the composed one.
        app.stack().exec_sql(FAIL_FRAUD_WRITES).await;
        let failed = press_pf5(&app, &token, &newest).await;
        assert_row(
            &failed,
            &validation,
            "V-COPAUS1C-009",
            Expected::Fallback {
                head: messages::SYSTEM_ERROR_DB2_CODE,
                text: messages::UNEXPECTED_ERROR,
            },
        );
        assert_eq!(stored_marks(&app).await, unmarked, "after a failed insert");
        app.stack().exec_sql(RESTORE_FRAUD_WRITES).await;

        // V-COPAUS1C-010: the insert answers -803, and `FRAUD-UPDATE` finds no
        // report to update: SQLCODE +100, SQLSTATE 02000.
        app.stack().exec_sql(DUPLICATE_FRAUD_INSERTS).await;
        let failed = press_pf5(&app, &token, &newest).await;
        let no_row = [
            Operand::signed_edited("WS-SQLCODE", i64::from(SQLCODE_NO_ROW), WS_SQLCODE_DIGITS),
            Operand::signed_edited(
                "WS-SQLSTATE",
                i64::from(SQLSTATE_NO_ROW),
                WS_SQLSTATE_DIGITS,
            ),
        ];
        assert_row(
            &failed,
            &validation,
            "V-COPAUS1C-010",
            Expected::Expanded {
                head: messages::UPDT_ERROR_DB2_CODE,
                operands: &no_row,
                receiver: WS_FRD_ACT_MSG_WIDTH,
            },
        );
        assert_eq!(stored_marks(&app).await, unmarked, "after a failed update");
        app.stack().exec_sql(RESTORE_FRAUD_INSERTS).await;

        // The report insert is backed out with the failed replace.
        replace_fails(&app, &token, &validation, &newest, &unmarked).await;

        // With every fault removed the mark commits; then the removal's report
        // update is backed out with its failed replace.
        let (status, body, raw) = press_pf5(&app, &token, &newest).await;
        assert_eq!(status, StatusCode::OK, "PF5 after the faults: {raw}");
        assert_eq!(
            body["message"].as_str(),
            Some(messages::AUTH_MARKED_FRAUD),
            "the failed presses must have left the detail unmarked: {raw}"
        );
        let marked = stored_marks(&app).await;
        assert_eq!(marked.1.len(), 1, "one report after the mark: {marked:?}");
        replace_fails(&app, &token, &validation, &newest, &marked).await;
    });
}

/// `MARK-AUTH-FRAUD` with its LINK to `COPAUS2C` not answering NORMAL: the
/// path that rolls back and sets no message.
const LINK_FAILURE_ROW: &str = "V-COPAUS1C-008";

/// The disposition of [`LINK_FAILURE_ROW`]: no LINK exists to fail.
const LINK_NOT_REPRODUCED: &str = "not reproduced (E-82)";

/// The failures a fraud mark can still meet past its read, each injected and
/// then removed: the `AUTHFRDS` table `COPAUS2C` writes taken away, the
/// cross-reference naming the `WS-FRD-CUST-ID` passed to it taken away, and a
/// commit that fails, the `SYNCPOINT` after the replace. Each is listed with
/// whether its text marks the `CARDNUM` field: a rule's failure does, the
/// failed commit, which no rule meets, does not.
const FRAUD_MARK_FAULTS: [(&str, &str, bool); 3] = [
    (
        "ALTER TABLE auth_fraud RENAME TO auth_fraud_off",
        "ALTER TABLE auth_fraud_off RENAME TO auth_fraud",
        true,
    ),
    (
        "ALTER TABLE card_xref RENAME TO card_xref_off",
        "ALTER TABLE card_xref_off RENAME TO card_xref",
        true,
    ),
    (
        "\
CREATE FUNCTION cpvd_commit_fault() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'injected CPVD commit fault';
END
$$;
CREATE CONSTRAINT TRIGGER cpvd_fail_commits AFTER UPDATE ON pending_auth_details
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION cpvd_commit_fault()",
        "\
DROP TRIGGER cpvd_fail_commits ON pending_auth_details;
DROP FUNCTION cpvd_commit_fault()",
        false,
    ),
];

/// COPAUS1C MARK-AUTH-FRAUD — V-COPAUS1C-008 (E-82): `COPAUS2C` is a direct
/// call of its rules, so the LINK failure that rolls back with no message has
/// no counterpart. Every failure the fraud mark meets past its read answers the
/// row's status and kind with a message, `Unexpected error` for these three,
/// never an empty one, and leaves every mark and report as it was; once the
/// fault is removed the same PF5 marks the authorization.
#[test]
fn a_fraud_mark_has_no_link_to_fail_and_every_failure_carries_a_text() {
    let validation = validation_audit();
    let audited_row = audited(&validation, LINK_FAILURE_ROW);
    assert!(
        is_no_message(&audited_row.message),
        "{LINK_FAILURE_ROW} records the COBOL path that sets no message: {}",
        audited_row.message
    );
    let (table, cells) = validation
        .find_row(LINK_FAILURE_ROW)
        .unwrap_or_else(|| panic!("the validation audit has no {LINK_FAILURE_ROW} row"));
    let trigger = table.cell(cells, "Trigger condition").unwrap_or_default();
    assert!(
        trigger.contains(LINK_NOT_REPRODUCED),
        "{LINK_FAILURE_ROW} records no {LINK_NOT_REPRODUCED:?}: {trigger}"
    );
    let code = error_code(LINK_FAILURE_ROW, &audited_row.kind);
    let message = messages::UNEXPECTED_ERROR;
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        let token = app.user_token().await;
        let ids = identities(&insert_authorizations(&app).await);
        let newest = path(&ids[0]);
        let unmarked = stored_marks(&app).await;
        for (inject, remove, on_field) in FRAUD_MARK_FAULTS {
            app.stack().exec_sql(inject).await;
            let failed = press_pf5(&app, &token, &newest).await;
            app.stack().exec_sql(remove).await;
            assert!(
                failed.1["message"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "{LINK_FAILURE_ROW}: {inject}: a 500 without a message: {}",
                failed.2
            );
            let fields: &[(&str, &str)] = if on_field {
                &[(audited_row.field.as_str(), message)]
            } else {
                &[]
            };
            assert_error(&failed, audited_row.status, code, message, fields);
            assert_eq!(
                stored_marks(&app).await,
                unmarked,
                "{LINK_FAILURE_ROW}: {inject}: nothing the failed mark wrote persists"
            );
        }
        let (status, body, raw) = press_pf5(&app, &token, &newest).await;
        assert_eq!(status, StatusCode::OK, "PF5 after the faults: {raw}");
        assert_eq!(
            body["message"].as_str(),
            Some(messages::AUTH_MARKED_FRAUD),
            "PF5 after the faults: {raw}"
        );
    });
}

/// COPAUS1C READ-AUTH-RECORD, READ-NEXT-AUTH-RECORD — P-COPAUS1C-004,
/// P-COPAUS1C-014: a read that fails answers the text of that read with the
/// IMS status E-44 sets. A failed summary read alone gives the summary text; a
/// failed detail read overwrites it with the detail text; a failed step to the
/// next twin gives the next-read text. PF5 answers the outcome of its read and
/// writes nothing (E-53).
#[test]
fn a_failed_read_answers_with_the_text_of_the_read_that_failed() {
    let validation = validation_audit();
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        let token = app.user_token().await;
        let ids = identities(&insert_authorizations(&app).await);
        let untouched = stored_marks(&app).await;
        let newest = path(&ids[0]);
        let faults = [
            (
                HIDE_SUMMARIES,
                RESTORE_SUMMARIES,
                "V-COPAUS1C-003",
                messages::SYSTEM_ERROR_WHILE_READING_AUTH_SUMMARY_COPAUS1C,
            ),
            (
                HIDE_DETAILS,
                RESTORE_DETAILS,
                "V-COPAUS1C-006",
                messages::SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS1C,
            ),
        ];
        for (hide, restore, id, head) in faults {
            app.stack().exec_sql(hide).await;
            let viewed = app.get(&newest, Some(&token)).await;
            assert_ims_row(&viewed, &validation, id, head);
            let marked = press_pf5(&app, &token, &newest).await;
            assert_ims_row(&marked, &validation, id, head);
            app.stack().exec_sql(restore).await;
        }
        assert_eq!(
            stored_marks(&app).await,
            untouched,
            "PF5 after a failed read must write nothing"
        );

        // V-COPAUS1C-013: the middle detail reads, the step to the oldest fails.
        let middle = path(&ids[1]);
        app.stack().exec_sql(BREAK_OLDEST_DETAIL).await;
        let stepped = app.get(&middle, Some(&token)).await;
        assert_ims_row(
            &stepped,
            &validation,
            "V-COPAUS1C-013",
            messages::SYSTEM_ERROR_WHILE_READING_NEXT_AUTH,
        );
        app.stack().exec_sql(RESTORE_OLDEST_DETAIL).await;
        let (status, body, raw) = app.get(&middle, Some(&token)).await;
        assert_eq!(status, StatusCode::OK, "after the restore: {raw}");
        assert_eq!(body["next_id"].as_str(), Some(ids[2].as_str()), "{raw}");
    });
}

/// Every parity row these tests prove exists and passed its review, so no
/// unreviewed row decides an expectation here.
#[test]
fn the_parity_rows_these_tests_prove_passed_review() {
    let parity = parity_audit();
    for id in PROVEN_PARITY_ROWS {
        assert!(
            parity.find_row(id).is_some(),
            "{id} is not a parity audit row"
        );
        assert_reviewed(&parity, id);
    }
}

/// The answer of a handler called directly, as the harness gives an HTTP
/// answer: the status, the error body and its text.
fn handler_answer<T>(result: Result<T, ApiError>) -> Resp {
    let Err(error) = result else {
        panic!("the handler answered 2xx although the store cannot be opened");
    };
    let body = serde_json::to_value(error.body())
        .unwrap_or_else(|failure| panic!("the error body does not serialize: {failure}"));
    let raw = body.to_string();
    (error.status(), body, raw)
}

/// The signed-on user the bearer-token extractor gives for `USER0001`.
fn signed_on_user() -> AuthUser {
    AuthUser {
        user_id: "USER0001".to_owned(),
        usr_type: UserType::User,
    }
}

/// COPAUS1C SCHEDULE-PSB — V-COPAUS1C-002, P-COPAUS1C-016, P-COPAUS1C-017: an
/// authorization store that cannot be opened answers ENTER and PF5 with the
/// scheduling text and the IMS status E-44 sets, and nothing abends.
#[test]
fn a_store_that_cannot_be_opened_answers_the_psb_scheduling_text() {
    // Over HTTP the extractor's read of the signed-on user reaches the
    // database before the handler does, so a database the handler cannot open
    // fails that read first. Only a direct call with the user the extractor
    // would give, over a pool closed before the call, meets the store opening
    // of the handler itself.
    let validation = validation_audit();
    let stack = block_on(TestStack::postgres());
    let settings = Settings {
        database_url: stack.database_url().to_owned(),
        clock_override: Some(CLOCK.to_owned()),
        ..Settings::default()
    };
    let state = AppState::new(Db::from_pool(stack.pool().clone()), settings)
        .unwrap_or_else(|error| panic!("the test settings were rejected: {error}"));
    block_on(async {
        state.db().close().await;
        let selected = || ApiPath(AuthIdPath { id: "1".to_owned() });
        let viewed = get_authorization(State(state.clone()), signed_on_user(), selected()).await;
        let marked = toggle_fraud(State(state.clone()), signed_on_user(), selected()).await;
        for answer in [handler_answer(viewed), handler_answer(marked)] {
            assert_ims_row(
                &answer,
                &validation,
                "V-COPAUS1C-002",
                messages::SYSTEM_ERROR_WHILE_SCHEDULING_PSB,
            );
        }
    });
}

/// A request a function map `Endpoint` cell names.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    /// The HTTP method, lower case as the `OpenAPI` document keys it.
    method: String,
    /// The route, as the `OpenAPI` document keys it.
    path: String,
    /// The `name=value` pairs of the query, in order; an empty value is
    /// filled from the screen shown.
    query: Vec<(String, String)>,
}

impl Request {
    /// `method` on [`DETAIL_ROUTE`] with no query.
    fn detail(method: &str) -> Self {
        Self {
            method: method.to_owned(),
            path: DETAIL_ROUTE.to_owned(),
            query: Vec::new(),
        }
    }
}

/// A function map row of [`MAP`].
struct MapRow {
    /// The `F-COPAU01-nnn` ID.
    id: String,
    /// The `UI control` cell: `route=…; control=…; target=…`.
    control: String,
    /// The request of the `Endpoint` cell; `None` for `—` and
    /// `— (navigation)`, a control that sends nothing.
    request: Option<Request>,
}

impl MapRow {
    /// The value of the `UI control` pair `key`.
    fn control_value(&self, key: &str) -> String {
        key_values(&self.control)
            .into_iter()
            .find(|pair| pair.key == key)
            .map_or_else(
                || panic!("{} gives no {key}= control: {}", self.id, self.control),
                |pair| pair.value,
            )
    }
}

/// The 3270 header [`HEADER_ROW`] gives: the member names of its output
/// fields (`TITLE01O` is `title01`), the texts it shows (titles, transaction
/// and program name) and the pictures of its date and time.
struct Header {
    /// The output fields, as member names.
    members: BTreeSet<String>,
    /// The titles, the transaction and the program name, trimmed.
    texts: BTreeSet<String>,
    /// The `curdate=` picture, `MM/DD/YY`.
    date: String,
    /// The `curtime=` picture, `HH:MM:SS`.
    time: String,
}

impl Header {
    /// The header of [`HEADER_ROW`], once it has passed review.
    fn read(parity: &Document) -> Self {
        let id = HEADER_ROW;
        let fields = parity_cell(parity, id, "Copybook field or PIC clause");
        let shown = fields.split(" of ").next().unwrap_or_default();
        let members = shown
            .split(", ")
            .map(|name| {
                let stem = name.trim().strip_suffix('O');
                stem.unwrap_or_else(|| panic!("{id}: {name} is not an output field"))
                    .to_ascii_lowercase()
            })
            .collect();
        let result = expected_result(parity, id);
        assert!(
            result.contains("disposition=no API field carries the 3270 header"),
            "{id} gives the header a REST disposition other than none: {result}"
        );
        let pairs = key_values(&result);
        let value = |key: &str| {
            pairs
                .iter()
                .find(|pair| pair.key == key)
                .map_or_else(|| panic!("{id} gives no {key}="), |pair| pair.value.clone())
        };
        let quoted = |key: &str| {
            code_span(&value(key))
                .and_then(|span| unquoted(&span.text).map(|text| text.trim().to_owned()))
                .unwrap_or_else(|| panic!("{id} quotes no {key} text"))
        };
        let first = |key: &str| first_word(&value(key), id, key);
        let picture = |key: &str| {
            let (_, rest) = result
                .split_once(&format!("{key}="))
                .unwrap_or_else(|| panic!("{id} gives no {key}="));
            first_word(rest, id, key)
        };
        Self {
            members,
            texts: BTreeSet::from([
                quoted("title01"),
                quoted("title02"),
                first("trnname"),
                first("pgmname"),
            ]),
            date: picture("curdate"),
            time: picture("curtime"),
        }
    }

    /// Asserts that no body of `bodies` carries a header member or a header
    /// text, that none carries a value shaped as the header time, and that a
    /// value shaped as the header date is only ever the report date.
    fn assert_absent(&self, bodies: &[String]) {
        let id = HEADER_ROW;
        assert!(!bodies.is_empty(), "{id}: no response body was kept");
        for raw in bodies {
            let body: Value = serde_json::from_str(raw)
                .unwrap_or_else(|error| panic!("{id}: a body is not JSON ({error}): {raw}"));
            let mut members = Vec::new();
            let mut strings = Vec::new();
            walk(&body, None, &mut members, &mut strings);
            for name in members {
                let name = name.to_ascii_lowercase();
                assert!(!self.members.contains(&name), "{id}: member {name}: {raw}");
            }
            for (member, text) in strings {
                let text = text.trim();
                assert!(!self.texts.contains(text), "{id}: text {text:?}: {raw}");
                assert!(
                    !matches_picture(text, &self.time),
                    "{id}: {} {text:?}: {raw}",
                    self.time
                );
                if matches_picture(text, &self.date) {
                    assert_eq!(
                        member,
                        Some(REPORT_DATE_MEMBER),
                        "{id}: {} {text:?} outside the report date: {raw}",
                        self.date
                    );
                }
            }
        }
    }
}

/// The first word of `text`, the value of the pair `key` of row `id`.
fn first_word(text: &str, id: &str, key: &str) -> String {
    text.split_whitespace()
        .next()
        .unwrap_or_else(|| panic!("{id} gives {key}= no value"))
        .to_owned()
}

/// The text between the double quotes that open and close `text`, the form
/// the audits quote a COBOL literal in.
fn unquoted(text: &str) -> Option<&str> {
    text.strip_prefix('"')?.strip_suffix('"')
}

/// Every member name of `value` into `members`, and every string, with the
/// name of the member holding it (`None` at the top level), into `strings`, at
/// any depth.
fn walk<'v>(
    value: &'v Value,
    member: Option<&'v str>,
    members: &mut Vec<&'v str>,
    strings: &mut Vec<(Option<&'v str>, &'v str)>,
) {
    match value {
        Value::Object(items) => {
            for (name, inner) in items {
                members.push(name);
                walk(inner, Some(name), members, strings);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, member, members, strings);
            }
        }
        Value::String(text) => strings.push((member, text)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// Whether `text` has the shape of the date or time `picture` (`MM/DD/YY`,
/// `HH:MM:SS`): a digit for each picture letter and the picture's own
/// separators.
fn matches_picture(text: &str, picture: &str) -> bool {
    text.chars().count() == picture.chars().count()
        && text.chars().zip(picture.chars()).all(|(ch, pic)| {
            if pic.is_ascii_uppercase() {
                ch.is_ascii_digit()
            } else {
                ch == pic
            }
        })
}

/// The cell `name` of the parity row `id`, once it has passed review.
fn parity_cell(parity: &Document, id: &str, name: &str) -> String {
    assert_reviewed(parity, id);
    let (table, row) = parity
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    table
        .cell(row, name)
        .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
        .to_owned()
}

/// The parsed `docs/parity/function-map.md` (`F-` rows).
fn function_map() -> Document {
    Repo::new(fixtures::repo_root())
        .function_map()
        .unwrap_or_else(|error| panic!("{FUNCTION_MAP_PATH} cannot be read: {error}"))
}

/// The request of a function map `Endpoint` cell: `None` for `—` and
/// `— (navigation)`, else `METHOD /path` with its query pairs.
fn request(cell: &str) -> Option<Request> {
    if cell.starts_with('—') {
        return None;
    }
    let (method, target) = cell
        .split_once(' ')
        .unwrap_or_else(|| panic!("a function map endpoint is not `METHOD /path`: {cell}"));
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (name.to_owned(), value.to_owned())
        })
        .collect();
    Some(Request {
        method: method.to_ascii_lowercase(),
        path: path.to_owned(),
        query,
    })
}

/// The function map row `row` of `table`, which must be one of [`MAP`].
fn map_row(table: &Table, row: &Row) -> MapRow {
    let cell = |name: &str| {
        table
            .cell(row, name)
            .unwrap_or_else(|| panic!("a {FUNCTION_MAP_PATH} row has no `{name}` cell"))
            .trim()
    };
    let id = cell("ID").to_owned();
    assert_eq!(cell("BMS map"), MAP, "{id} is not a {MAP} row");
    MapRow {
        control: cell("UI control").to_owned(),
        request: request(cell("Endpoint")),
        id,
    }
}

/// The function map rows of [`MAP`] that `keep` selects by their `Key or
/// field` and `COBOL action` cells.
fn map_rows(map_doc: &Document, keep: impl Fn(&str, &str) -> bool) -> Vec<MapRow> {
    map_doc
        .tables()
        .iter()
        .filter(|table| table.is_data_table())
        .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
        .filter(|(table, row)| {
            let cell = |name: &str| table.cell(row, name).map_or("", str::trim);
            cell("BMS map") == MAP && keep(cell("Key or field"), cell("COBOL action"))
        })
        .map(|(table, row)| map_row(table, row))
        .collect()
}

/// The function map rows of [`MAP`] whose `Key or field` starts with the word
/// `key`.
fn key_rows(map_doc: &Document, key: &str) -> Vec<MapRow> {
    let rows = map_rows(map_doc, |field, _| {
        field.split_whitespace().next() == Some(key)
    });
    assert!(
        !rows.is_empty(),
        "{FUNCTION_MAP_PATH} has no {key} row on {MAP}"
    );
    rows
}

/// The function map rows of [`MAP`] whose `COBOL action` cites the parity row
/// `id`.
fn citing_rows(map_doc: &Document, id: &str) -> Vec<MapRow> {
    let rows = map_rows(map_doc, |_, action| {
        id_tokens(action)
            .iter()
            .any(|(kind, token)| *kind == IdKind::Parity && token == id)
    });
    assert!(
        !rows.is_empty(),
        "no {MAP} row of {FUNCTION_MAP_PATH} cites {id}"
    );
    rows
}

/// The function map rows the `Expected result` of the parity row `id` names.
fn named_rows(parity: &Document, map_doc: &Document, id: &str) -> Vec<MapRow> {
    let rows: Vec<MapRow> = id_tokens(&expected_result(parity, id))
        .into_iter()
        .filter(|(kind, _)| *kind == IdKind::Function)
        .map(|(_, function)| {
            let (table, row) = map_doc
                .find_row(&function)
                .unwrap_or_else(|| panic!("{FUNCTION_MAP_PATH} defines no {function} row"));
            map_row(table, row)
        })
        .collect();
    assert!(!rows.is_empty(), "{id} names no function map row");
    rows
}

/// The published `OpenAPI` document.
fn spec() -> Value {
    serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"))
}

/// The HTTP methods `spec` publishes an operation under on `route`.
fn operations<'s>(spec: &'s Value, route: &str) -> BTreeSet<&'s str> {
    spec["paths"][route]
        .as_object()
        .map(|item| {
            item.keys()
                .map(String::as_str)
                .filter(|name| HTTP_METHODS.contains(name))
                .collect()
        })
        .unwrap_or_default()
}

/// The `(name, in)` pairs of the parameters of `method` on `route`.
fn parameters<'s>(spec: &'s Value, route: &str, method: &str) -> BTreeSet<(&'s str, &'s str)> {
    spec["paths"][route][method]["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("{method} {route} publishes no parameters"))
        .iter()
        .map(|parameter| {
            let text = |name: &str| parameter[name].as_str().unwrap_or_default();
            (text("name"), text("in"))
        })
        .collect()
}

/// The property names of the component schema `name` of `spec`.
fn schema_members(spec: &Value, name: &str) -> BTreeSet<String> {
    spec["components"]["schemas"][name]["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("the OpenAPI document has no {name} schema properties"))
        .keys()
        .cloned()
        .collect()
}

/// COPAUS1C MARK-AUTH-FRAUD — P-COPAUS1C-006: the flag and text PF5 gives a
/// detail holding `flag`: `R` and `AUTH FRAUD REMOVED...` for `F`, else `F`
/// and `AUTH MARKED FRAUD...`.
fn toggle_of(flag: &str) -> (&'static str, &'static str) {
    if flag == FRAUD_CONFIRMED {
        (FRAUD_REMOVED, messages::AUTH_FRAUD_REMOVED)
    } else {
        (FRAUD_CONFIRMED, messages::AUTH_MARKED_FRAUD)
    }
}

/// The stored `PA-AUTH-FRAUD` of the newest detail, the first of
/// [`SELECT_MARKS`].
async fn newest_flag(app: &TestApp) -> String {
    let marks = app.stack().fetch_text_rows(SELECT_MARKS).await;
    let newest = marks
        .first()
        .unwrap_or_else(|| panic!("no detail is stored: {marks:?}"));
    column(newest, 1).to_owned()
}

/// Asserts that `resp`, a PF5 answer on the newest detail `newest`, toggled
/// its stored flag from `flag` as [`toggle_of`] gives it.
async fn assert_toggled(app: &TestApp, resp: &Resp, newest: &str, flag: &str) {
    let (status, body, raw) = resp;
    let (toggled, message) = toggle_of(flag);
    assert_eq!(*status, StatusCode::OK, "PF5 on {newest}: {raw}");
    assert_eq!(body["message"].as_str(), Some(message), "{raw}");
    assert_eq!(body["detail"]["auth_id"].as_str(), Some(newest), "{raw}");
    assert_eq!(
        body["detail"]["pa_auth_fraud"].as_str(),
        Some(toggled),
        "{raw}"
    );
    assert_eq!(
        newest_flag(app).await,
        toggled,
        "the stored mark after {raw}"
    );
}

/// COPAUS1C MAIN-PARA, PROCESS-ENTER-KEY — P-COPAUS1C-001: the entry from
/// `COPAUS0C` reads and sends the selected authorization at once. The
/// function map row citing it is the list's link to the detail route, served
/// by `GET /authorizations/{id}`: every identity the list of account 1 gives
/// answers its authorization as stored on its first request, and ENTER after
/// it answers the same.
async fn first_entry(
    app: &TestApp,
    token: &str,
    details: &[Vec<Option<String>>],
    (parity, map_doc): (&Document, &Document),
    bodies: &mut Vec<String>,
) {
    let expected = expected_result(parity, FIRST_ENTRY_ROW);
    assert!(
        expected.contains("PROCESS-ENTER-KEY") && expected.contains("send detail"),
        "{FIRST_ENTRY_ROW}: {expected}"
    );
    let links: Vec<MapRow> = citing_rows(map_doc, FIRST_ENTRY_ROW)
        .into_iter()
        .filter(|row| row.request.is_some())
        .collect();
    let [link] = links.as_slice() else {
        panic!("{FIRST_ENTRY_ROW}: one request must serve the entry");
    };
    assert_eq!(link.request, Some(Request::detail("get")), "{}", link.id);
    let list_route = link.control_value("route");
    assert!(
        operations(&spec(), &list_route).contains("get"),
        "{}: the list route {list_route} is published",
        link.id
    );
    let account = stored_field(&details[0], "account_id");
    let listing = format!("{list_route}?account_id={account}");
    let (status, list, raw) = app.get(&listing, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{listing}: {raw}");
    let listed: Vec<String> = list["items"]
        .as_array()
        .unwrap_or_else(|| panic!("{listing} lists no items: {raw}"))
        .iter()
        .map(|item| text_of(&item["auth_id"]))
        .collect();
    assert_eq!(listed, identities(details), "{listing}: {raw}");
    for (id, stored) in listed.iter().zip(details) {
        let entered = app.get(&path(id), Some(token)).await;
        assert_eq!(
            entered.0,
            StatusCode::OK,
            "the entry with {id}: {}",
            entered.2
        );
        assert_shows(&entered.1, stored, parity, &entered.2);
        let again = app.get(&path(id), Some(token)).await;
        assert_eq!(
            (again.0, &again.1),
            (entered.0, &entered.1),
            "ENTER after the entry with {id}"
        );
        bodies.extend([entered.2, again.2]);
    }
}

/// The JSON string `value`.
fn text_of(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("expected a JSON string, got {value}"))
        .to_owned()
}

/// The keys of [`AID_ROW`] in the order its input condition lists them, each
/// with its action.
fn aid_keys(parity: &Document) -> Vec<(String, String)> {
    let keys: Vec<String> = parity_cell(parity, AID_ROW, "Input condition")
        .split(" / ")
        .map(|key| key.trim().to_owned())
        .collect();
    let actions = key_values(&expected_result(parity, AID_ROW))
        .into_iter()
        .find(|pair| pair.key == "action")
        .unwrap_or_else(|| panic!("{AID_ROW} gives no action= result"))
        .value;
    let actions: Vec<String> = actions
        .split(" / ")
        .map(|action| action.trim().to_owned())
        .collect();
    assert_eq!(keys.len(), actions.len(), "{AID_ROW}: one action per key");
    keys.into_iter().zip(actions).collect()
}

/// COPAUS1C MAIN-PARA — P-COPAUS1C-002: each AID key of `EVALUATE EIBAID`
/// against the requests of its function map rows. ENTER is the published
/// view and PF5 the published fraud mark; PF3's `XCTL COPAUS0C` sends no
/// request; PF8 is the summary list's page of one after the authorization
/// shown (E-59). The other keys have no control (E-56). No PF7 path exists,
/// so the view names no previous authorization.
async fn aid_keys_map_to_requests(
    app: &TestApp,
    token: &str,
    ids: &[String],
    (parity, map_doc): (&Document, &Document),
    bodies: &mut Vec<String>,
) {
    let spec = spec();
    assert_eq!(
        operations(&spec, DETAIL_ROUTE),
        BTreeSet::from(["get", "put"]),
        "{AID_ROW}: only ENTER and PF5 address the detail route"
    );
    let newest = &ids[0];
    for (key, action) in aid_keys(parity) {
        let requests: Vec<Request> = key_rows(map_doc, &key)
            .into_iter()
            .filter_map(|row| row.request)
            .collect();
        if key == OTHER_KEY {
            assert!(requests.is_empty(), "{AID_ROW} {key}: {requests:?}");
            other_keys(app, token, &action, newest, bodies).await;
        } else if action.starts_with("XCTL") {
            assert!(requests.is_empty(), "{AID_ROW} {key}: {requests:?}");
        } else if action == "detail" {
            assert_eq!(requests, [Request::detail("get")], "{AID_ROW} {key}");
            let (status, body, raw) = app.get(&path(newest), Some(token)).await;
            assert_eq!(status, StatusCode::OK, "{AID_ROW} {key}: {raw}");
            assert_eq!(body["auth_id"].as_str(), Some(newest.as_str()), "{raw}");
            bodies.push(raw);
        } else if action == "MARK-AUTH-FRAUD" {
            assert_eq!(requests, [Request::detail("put")], "{AID_ROW} {key}");
            let flag = newest_flag(app).await;
            let marked = press_pf5(app, token, &path(newest)).await;
            assert_toggled(app, &marked, newest, &flag).await;
            bodies.push(marked.2);
        } else if action == "PROCESS-PF8-KEY" {
            next_pages(app, token, &requests, ids, bodies).await;
        } else {
            panic!("{AID_ROW} {key}: no REST replacement is known for {action}");
        }
    }
    let result = expected_result(parity, AID_ROW);
    assert!(result.contains("no PF7 (previous) path exists"), "{result}");
    let members = schema_members(&spec, "AuthDetailResponse");
    assert!(
        members.contains("next_id") && !members.iter().any(|name| name.starts_with("prev")),
        "{AID_ROW}: the view names the next authorization and no previous one: {members:?}"
    );
}

/// `MAIN-PARA` `WHEN OTHER` — P-COPAUS1C-002, E-56: `action` quotes
/// `CCDA-MSG-INVALID-KEY` within its `X(50)` width, and a view naming one of
/// [`OTHER_KEYS`] answers as the view without it, never with that text: the
/// view takes no key input.
async fn other_keys(
    app: &TestApp,
    token: &str,
    action: &str,
    newest: &str,
    bodies: &mut Vec<String>,
) {
    let invalid =
        code_span(action).unwrap_or_else(|| panic!("{AID_ROW} quotes no invalid-key text"));
    assert_eq!(
        invalid.text,
        format!("\"{}\"", messages::CCDA_MSG_INVALID_KEY),
        "{AID_ROW}"
    );
    assert!(
        invalid.width >= Some(messages::CCDA_MSG_INVALID_KEY.len()),
        "{AID_ROW}: the invalid-key text fits its width"
    );
    let entered = app.get(&path(newest), Some(token)).await;
    assert_eq!(entered.0, StatusCode::OK, "{}", entered.2);
    for aid in OTHER_KEYS {
        let keyed = format!("{}?{AID_MEMBER}={aid}", path(newest));
        let answer = app.get(&keyed, Some(token)).await;
        assert_eq!(
            (answer.0, &answer.1),
            (entered.0, &entered.1),
            "{AID_ROW} {aid}"
        );
        assert!(
            !answer.2.contains(messages::CCDA_MSG_INVALID_KEY.trim_end()),
            "{AID_ROW} {aid}: the invalid-key text: {}",
            answer.2
        );
        bodies.push(answer.2);
    }
    bodies.push(entered.2);
}

/// PF8's replacement — P-COPAUS1C-002, F-COPAU01-006, E-59: the one request
/// `requests` holds, the summary list's forward page of one from the
/// authorization shown, lists the view's `next_id` for each authorization of
/// `ids` in twin order, and nothing after the last.
async fn next_pages(
    app: &TestApp,
    token: &str,
    requests: &[Request],
    ids: &[String],
    bodies: &mut Vec<String>,
) {
    let [next] = requests else {
        panic!("{AID_ROW} PF8: one request, not {requests:?}");
    };
    assert_eq!(next.method, "get", "{AID_ROW} PF8: {next:?}");
    for (at, id) in ids.iter().enumerate() {
        let (status, view, raw) = app.get(&path(id), Some(token)).await;
        assert_eq!(status, StatusCode::OK, "{raw}");
        assert_eq!(view["next_id"], json!(ids.get(at + 1)), "{raw}");
        let account = text_of(&view["account_id"]);
        let query: Vec<String> = next
            .query
            .iter()
            .map(|(name, value)| {
                let value = match (name.as_str(), value.is_empty()) {
                    (_, false) => value.as_str(),
                    ("account_id", true) => account.as_str(),
                    ("start_key", true) => id.as_str(),
                    (other, true) => panic!("{AID_ROW} PF8 leaves `{other}` unfilled"),
                };
                format!("{name}={value}")
            })
            .collect();
        let page_path = format!("{}?{}", next.path, query.join("&"));
        let (status, page, page_raw) = app.get(&page_path, Some(token)).await;
        assert_eq!(status, StatusCode::OK, "{page_path}: {page_raw}");
        let listed: Vec<Value> = page["items"]
            .as_array()
            .unwrap_or_else(|| panic!("{page_path} lists no items: {page_raw}"))
            .iter()
            .map(|item| item["auth_id"].clone())
            .collect();
        let following: Vec<Value> = view["next_id"]
            .as_str()
            .map(|next_id| json!(next_id))
            .into_iter()
            .collect();
        assert_eq!(listed, following, "{page_path} after {id}: {page_raw}");
        bodies.push(raw);
    }
}

/// COPAUS1C RECEIVE-AUTHVIEW-SCREEN — P-COPAUS1C-012: no field of the
/// received map is read, and the route parameter replaces both the map and
/// the COMMAREA key. Both operations publish exactly the `id` path parameter
/// and the fraud mark no body; a view naming map fields in its query answers
/// as the plain view, and a fraud mark toggles the flag it read whatever
/// body it carries, a JSON body naming another account and flag or a body
/// that is no JSON at all, as the receive condition is ignored.
async fn received_map(
    app: &TestApp,
    token: &str,
    newest: &str,
    parity: &Document,
    bodies: &mut Vec<String>,
) {
    let expected = expected_result(parity, RECEIVE_ROW);
    assert!(
        expected.contains("no field of COPAU1AI is read")
            && expected.contains("the route parameter replaces the received map"),
        "{RECEIVE_ROW}: {expected}"
    );
    let spec = spec();
    for method in ["get", "put"] {
        assert_eq!(
            parameters(&spec, DETAIL_ROUTE, method),
            BTreeSet::from([("id", "path")]),
            "{RECEIVE_ROW}: {method} {DETAIL_ROUTE}"
        );
    }
    assert!(
        spec["paths"][DETAIL_ROUTE]["put"]
            .get("requestBody")
            .is_none(),
        "{RECEIVE_ROW}: PF5 publishes no body"
    );
    let plain = app.get(&path(newest), Some(token)).await;
    let mapped = format!("{}?account_id={OTHER_ACCOUNT}&CARDNUM={CARD}", path(newest));
    let viewed = app.get(&mapped, Some(token)).await;
    assert_eq!(
        (viewed.0, &viewed.1),
        (plain.0, &plain.1),
        "{RECEIVE_ROW}: {mapped}"
    );
    let flag = newest_flag(app).await;
    let fields = json!({ "account_id": OTHER_ACCOUNT, "CARDNUM": CARD, "pa_auth_fraud": flag });
    let marked = app.put(&path(newest), Some(token), &fields).await;
    assert_toggled(app, &marked, newest, &flag).await;
    let flag = newest_flag(app).await;
    let unparsed = app
        .client()
        .put(app.url(&path(newest)))
        .header("content-type", "text/plain")
        .body("PF5");
    let unparsed = common::send(unparsed, Some(token)).await;
    assert_toggled(app, &unparsed, newest, &flag).await;
    bodies.extend([plain.2, viewed.2, marked.2, unparsed.2]);
}

/// COPAUS1C SEND-AUTHVIEW-SCREEN — P-COPAUS1C-011: the answer replaces the
/// map, `WS-MESSAGE` being `message` and the cursor the one field of every
/// error. The cursor the row names is the BMS field of every `COPAUS1C`
/// validation row, and a rejected key answers on it. A send with ERASE,
/// ENTER on an authorization a twin follows, carries the detail and no
/// message; the send without ERASE after PF8 at the last authorization keeps
/// the detail shown and rewrites only the message.
async fn sent_screens(
    app: &TestApp,
    token: &str,
    (validation, parity): (&Document, &Document),
    bodies: &mut Vec<String>,
) {
    let expected = expected_result(parity, SEND_ROW);
    assert!(expected.contains("SEND-ERASE-NO"), "{SEND_ROW}: {expected}");
    let (_, rest) = expected
        .split_once("cursor ")
        .unwrap_or_else(|| panic!("{SEND_ROW} names no cursor: {expected}"));
    let cursor: String = rest
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    for id in validation_ids(PROGRAM) {
        assert_eq!(
            audited(validation, &id).field,
            cursor,
            "{id} and {SEND_ROW}"
        );
    }
    let rejected = app.get(&path(NOT_AUTH_IDS[0]), Some(token)).await;
    let row = audited(validation, "V-COPAUS1C-001");
    assert_error(
        &rejected,
        row.status,
        error_code("V-COPAUS1C-001", &row.kind),
        NO_MESSAGE,
        &[(cursor.as_str(), NO_MESSAGE)],
    );
    let details = app.stack().fetch_text_rows(SELECT_DETAILS).await;
    let (Some(newest), Some(oldest)) = (details.first(), details.last()) else {
        panic!("{SEND_ROW}: no detail is stored");
    };
    let erased = app
        .get(&path(stored_field(newest, "auth_id")), Some(token))
        .await;
    assert_eq!(erased.0, StatusCode::OK, "{}", erased.2);
    assert_shows(&erased.1, newest, parity, &erased.2);
    assert!(
        erased.1.get("message").is_none(),
        "{SEND_ROW}: {}",
        erased.2
    );
    let kept = app
        .get(&path(stored_field(oldest, "auth_id")), Some(token))
        .await;
    assert_eq!(kept.0, StatusCode::OK, "{}", kept.2);
    assert_shows(&kept.1, oldest, parity, &kept.2);
    assert_eq!(
        kept.1["message"].as_str(),
        Some(messages::ALREADY_AT_THE_LAST_AUTHORIZATION),
        "{SEND_ROW}: {}",
        kept.2
    );
    bodies.extend([rejected.2, erased.2, kept.2]);
}

/// COPAUS1C RETURN-TO-PREV-SCREEN — P-COPAUS1C-010: PF3 is the control the
/// row names, which sends no request and targets the `COPAUS0C` list route,
/// published for `GET`. The entry without a COMMAREA (`EIBCALEN = 0`) is a
/// request without a session: the view and the fraud mark answer 401
/// `Authentication required` with no field, and no mark or report changes.
async fn returned_screens(
    app: &TestApp,
    newest: &str,
    (parity, map_doc): (&Document, &Document),
    bodies: &mut Vec<String>,
) {
    let expected = expected_result(parity, RETURN_ROW);
    assert!(
        expected.contains("XCTL COPAUS0C") && expected.contains("no IMS access"),
        "{RETURN_ROW}: {expected}"
    );
    let condition = parity_cell(parity, RETURN_ROW, "Input condition");
    assert!(
        condition.contains("PF3") && condition.contains("EIBCALEN = 0"),
        "{RETURN_ROW}: {condition}"
    );
    let spec = spec();
    for row in named_rows(parity, map_doc, RETURN_ROW) {
        assert_eq!(
            row.request, None,
            "{RETURN_ROW}: {} sends a request",
            row.id
        );
        let target = row.control_value("target");
        assert!(
            operations(&spec, &target).contains("get"),
            "{RETURN_ROW}: {} targets {target}, which is not the published list",
            row.id
        );
    }
    let before = stored_marks(app).await;
    for token in NO_SESSION {
        let viewed = app.get(&path(newest), token).await;
        let marked = app.put(&path(newest), token, &json!({})).await;
        for resp in [viewed, marked] {
            assert_error(
                &resp,
                StatusCode::UNAUTHORIZED,
                UNAUTHENTICATED,
                messages::AUTHENTICATION_REQUIRED,
                &[],
            );
            bodies.push(resp.2);
        }
    }
    assert_eq!(
        stored_marks(app).await,
        before,
        "{RETURN_ROW}: a request without a session changed a mark or a report"
    );
}

/// COPAUS1C POPULATE-HEADER-INFO — P-COPAUS1C-013: no body of `bodies`, and
/// no response schema of the CPVD operations, carries the 3270 header.
fn assert_no_header(bodies: &[String], parity: &Document) {
    let header = Header::read(parity);
    header.assert_absent(bodies);
    let spec = spec();
    for schema in RESPONSE_SCHEMAS {
        let shown: Vec<String> = schema_members(&spec, schema)
            .into_iter()
            .filter(|name| header.members.contains(&name.to_ascii_lowercase()))
            .collect();
        assert!(shown.is_empty(), "{HEADER_ROW}: {schema} carries {shown:?}");
    }
}

/// The 3270 mechanics of `COPAUS1C` and their REST replacements, on one
/// seeded database with the three details: the entry from `COPAUS0C`, the AID
/// keys of `MAIN-PARA`, the received and the sent map, PF3 with the entry
/// without a session, and, over every answer they read, the absent header.
#[test]
fn the_cpvd_screen_mechanics_have_their_rest_replacements() {
    let validation = validation_audit();
    let parity = parity_audit();
    let map_doc = function_map();
    let app = TestApp::start_with_clock(CLOCK);
    let bodies = block_on(async {
        let token = app.user_token().await;
        let details = insert_authorizations(&app).await;
        let ids = identities(&details);
        let docs = (&parity, &map_doc);
        let mut bodies = Vec::new();
        first_entry(&app, &token, &details, docs, &mut bodies).await;
        aid_keys_map_to_requests(&app, &token, &ids, docs, &mut bodies).await;
        received_map(&app, &token, &ids[0], &parity, &mut bodies).await;
        sent_screens(&app, &token, (&validation, &parity), &mut bodies).await;
        returned_screens(&app, &ids[0], docs, &mut bodies).await;
        bodies
    });
    assert_no_header(&bodies, &parity);
}
